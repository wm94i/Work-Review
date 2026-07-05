//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::config::{AppCategoryRule, AppConfig, CustomSemanticCategory, WebsiteSemanticRule};
use crate::error::AppError;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

use super::shared::persist_app_config;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppCategoryOverviewItem {
    pub app_name: String,
    pub category: String,
    pub total_duration: i64,
    pub is_overridden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_url: Option<String>,
}

/// 应用分类概览 —— 内部复用版
pub(crate) fn get_app_category_overview_inner(
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<AppCategoryOverviewItem>, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let overview = s.database.get_app_category_overview()?;

    Ok(overview
        .into_iter()
        .map(|item| {
            let override_category = crate::monitor::find_category_override(
                &s.config.app_category_rules,
                &item.app_name,
                &s.config.custom_categories,
            );
            let is_overridden = override_category.is_some();
            AppCategoryOverviewItem {
                app_name: item.app_name,
                category: override_category.unwrap_or(item.category),
                total_duration: item.total_duration,
                is_overridden,
                screenshot_url: item.screenshot_url,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn get_app_category_overview(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<AppCategoryOverviewItem>, AppError> {
    get_app_category_overview_inner(state.inner())
}

fn upsert_app_category_rule(config: &mut AppConfig, app_name: &str, category: &str) {
    let normalized_app_name = crate::monitor::normalize_display_app_name(app_name);
    let custom_keys: Vec<String> = config
        .custom_categories
        .iter()
        .map(|c| c.key.clone())
        .collect();
    let normalized_category = crate::config::normalize_category_key_private(category, &custom_keys);
    let match_key = normalized_app_name.to_lowercase();

    if let Some(rule) = config.app_category_rules.iter_mut().find(|rule| {
        crate::monitor::normalize_display_app_name(&rule.app_name).to_lowercase() == match_key
    }) {
        rule.app_name = normalized_app_name;
        rule.category = normalized_category;
        return;
    }

    config.app_category_rules.push(AppCategoryRule {
        app_name: normalized_app_name,
        category: normalized_category,
    });
}

fn reclassify_app_history_in_state(
    state: &AppState,
    app_name: &str,
    category: &str,
) -> Result<usize, AppError> {
    let custom_keys: Vec<String> = state
        .config
        .custom_categories
        .iter()
        .map(|c| c.key.clone())
        .collect();
    let target_category = crate::config::normalize_category_key_private(category, &custom_keys);
    let activities = state
        .database
        .get_activities_by_normalized_app_name(app_name)?;

    for activity in &activities {
        let classification = crate::activity_classifier::classify_activity_with_base_category(
            &activity.app_name,
            &activity.window_title,
            activity.browser_url.as_deref(),
            &target_category,
        );
        state.database.update_activity_classification(
            activity.id.expect("活动记录应包含主键"),
            &classification.base_category,
            Some(&classification.semantic_category),
            Some(i32::from(classification.confidence)),
        )?;
    }

    Ok(activities.len())
}

fn upsert_domain_semantic_rule(config: &mut AppConfig, domain: &str, semantic_category: &str) {
    let Some(normalized_domain) = crate::monitor::normalize_domain_rule(domain) else {
        return;
    };
    let normalized_semantic_category = semantic_category.trim().to_string();

    if let Some(rule) = config.website_semantic_rules.iter_mut().find(|rule| {
        crate::monitor::normalize_domain_rule(&rule.domain).as_deref()
            == Some(normalized_domain.as_str())
    }) {
        rule.domain = normalized_domain;
        rule.semantic_category = normalized_semantic_category;
        return;
    }

    config.website_semantic_rules.push(WebsiteSemanticRule {
        domain: normalized_domain,
        semantic_category: normalized_semantic_category,
    });
}

fn reclassify_domain_history_in_state(
    state: &AppState,
    domain: &str,
    semantic_category: &str,
) -> Result<usize, AppError> {
    let activities = state.database.get_activities_by_domain(domain)?;
    let semantic_category = semantic_category.trim();

    for activity in &activities {
        let next_base_category = crate::monitor::semantic_category_to_base_category(
            semantic_category,
            &activity.category,
        );
        state.database.update_activity_classification(
            activity.id.expect("活动记录应包含主键"),
            &next_base_category,
            Some(semantic_category),
            Some(100),
        )?;
    }

    Ok(activities.len())
}

#[tauri::command]
pub async fn set_app_category_rule(
    app_name: String,
    category: String,
    sync_history: bool,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let trimmed_app_name = app_name.trim();
    if trimmed_app_name.is_empty() {
        return Err(AppError::Unknown("应用名称不能为空".to_string()));
    }

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();
        upsert_app_category_rule(&mut next_config, trimmed_app_name, &category);
        next_config
    };

    persist_app_config(next_config, app, state.inner())?;

    if !sync_history {
        return Ok(0);
    }

    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    reclassify_app_history_in_state(&state, trimmed_app_name, &category)
}

#[tauri::command]
pub async fn reclassify_app_history(
    app_name: String,
    category: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    reclassify_app_history_in_state(&state, &app_name, &category)
}

/// 分类信息（前端展示用）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CategoryInfo {
    pub key: String,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub is_system: bool,
}

/// 分类信息 —— 内部复用版
pub(crate) fn get_categories_inner(
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<CategoryInfo>, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let mut result = Vec::new();
    for c in &s.config.custom_categories {
        result.push(CategoryInfo {
            key: c.key.clone(),
            name: c.name.clone(),
            color: c.color.clone(),
            icon: c.icon.clone(),
            is_system: c.key == "other",
        });
    }
    Ok(result)
}

#[tauri::command]
pub async fn get_categories(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<CategoryInfo>, AppError> {
    get_categories_inner(state.inner())
}

#[tauri::command]
pub async fn save_custom_category(
    key: String,
    name: String,
    color: String,
    icon: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let key = key.trim().to_lowercase();
    let name = name.trim().to_string();
    let color = color.trim().to_string();
    let icon = icon.trim().to_string();

    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(AppError::Unknown(
            "分类标识只能包含小写字母、数字和连字符".to_string(),
        ));
    }
    if name.is_empty() {
        return Err(AppError::Unknown("分类名称不能为空".to_string()));
    }
    if !color.starts_with('#') || color.len() != 7 {
        return Err(AppError::Unknown("颜色格式无效，需为 #RRGGBB".to_string()));
    }

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();

        let custom = crate::config::CustomCategory {
            key: key.clone(),
            name: name.clone(),
            color: color.clone(),
            icon: icon.clone(),
        };

        if let Some(existing) = next_config
            .custom_categories
            .iter_mut()
            .find(|c| c.key == key)
        {
            *existing = custom;
        } else {
            next_config.custom_categories.push(custom);
        }

        next_config
    };

    persist_app_config(next_config, app, state.inner())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_custom_category(
    key: String,
    reassign_to: Option<String>,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let key = key.trim().to_lowercase();
    let fallback = reassign_to.unwrap_or_else(|| "other".to_string());

    if key == "other" {
        return Err(AppError::Unknown("不能删除默认分类「其他」".to_string()));
    }

    let affected = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        // 统计引用该分类的规则数
        state
            .config
            .app_category_rules
            .iter()
            .filter(|r| r.category == key)
            .count()
    };

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();

        // 删除分类
        next_config.custom_categories.retain(|c| c.key != key);

        // 记录已删除的内置分类 key，防止 seed 复活
        if crate::config::DEFAULT_CATEGORY_KEYS.contains(&key.as_str())
            && !next_config.deleted_default_categories.contains(&key)
        {
            next_config.deleted_default_categories.push(key.clone());
        }

        // 重定向引用该分类的规则
        for rule in &mut next_config.app_category_rules {
            if rule.category == key {
                rule.category = fallback.clone();
            }
        }

        next_config
    };

    persist_app_config(next_config, app, state.inner())?;
    Ok(affected)
}

/// 语义分类信息（前端展示用）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SemanticCategoryInfo {
    pub key: String,
    pub name: String,
    pub is_system: bool,
}

/// 语义分类信息 —— 内部复用版
pub(crate) fn get_semantic_categories_inner(
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<SemanticCategoryInfo>, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let mut result = Vec::new();
    for c in &s.config.custom_semantic_categories {
        result.push(SemanticCategoryInfo {
            key: c.key.clone(),
            name: c.name.clone(),
            is_system: c.key == "未知活动",
        });
    }
    Ok(result)
}

#[tauri::command]
pub async fn get_semantic_categories(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<SemanticCategoryInfo>, AppError> {
    get_semantic_categories_inner(state.inner())
}

#[tauri::command]
pub async fn save_custom_semantic_category(
    key: String,
    name: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let key = key.trim().to_string();
    let name = name.trim().to_string();

    if key.is_empty() {
        return Err(AppError::Unknown("分类标识不能为空".to_string()));
    }
    if name.is_empty() {
        return Err(AppError::Unknown("分类名称不能为空".to_string()));
    }

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();

        let custom = CustomSemanticCategory {
            key: key.clone(),
            name: name.clone(),
        };

        if let Some(existing) = next_config
            .custom_semantic_categories
            .iter_mut()
            .find(|c| c.key == key)
        {
            *existing = custom;
        } else {
            next_config.custom_semantic_categories.push(custom);
        }

        next_config
    };

    persist_app_config(next_config, app, state.inner())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_custom_semantic_category(
    key: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let key = key.trim().to_string();

    if key == "未知活动" {
        return Err(AppError::Unknown(
            "不能删除默认分类「未知活动」".to_string(),
        ));
    }

    let affected = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        // 统计引用该分类的规则数
        state
            .config
            .website_semantic_rules
            .iter()
            .filter(|r| r.semantic_category == key)
            .count()
    };

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();

        // 删除自定义语义分类
        next_config
            .custom_semantic_categories
            .retain(|c| c.key != key);

        // 记录已删除的内置语义分类 key，防止 seed 复活
        if crate::config::DEFAULT_SEMANTIC_CATEGORY_KEYS.contains(&key.as_str())
            && !next_config
                .deleted_default_semantic_categories
                .contains(&key)
        {
            next_config
                .deleted_default_semantic_categories
                .push(key.clone());
        }

        // 重定向引用该分类的规则到"未知活动"
        for rule in &mut next_config.website_semantic_rules {
            if rule.semantic_category == key {
                rule.semantic_category = "未知活动".to_string();
            }
        }

        next_config
    };

    persist_app_config(next_config, app, state.inner())?;
    Ok(affected)
}

#[tauri::command]
pub async fn set_domain_semantic_rule(
    domain: String,
    semantic_category: String,
    sync_history: bool,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let normalized_domain = crate::monitor::normalize_domain_rule(&domain)
        .ok_or_else(|| AppError::Unknown("域名不能为空".to_string()))?;
    let trimmed_semantic_category = semantic_category.trim();
    if trimmed_semantic_category.is_empty() {
        return Err(AppError::Unknown("语义分类不能为空".to_string()));
    }

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();
        upsert_domain_semantic_rule(
            &mut next_config,
            &normalized_domain,
            trimmed_semantic_category,
        );
        next_config
    };

    persist_app_config(next_config, app, state.inner())?;

    if !sync_history {
        return Ok(0);
    }

    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    reclassify_domain_history_in_state(&state, &normalized_domain, trimmed_semantic_category)
}

