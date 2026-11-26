use crate::AppState;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use work_review_core::database::Activity;
use work_review_core::error::AppError;
use work_review_core::privacy::{sanitize_activity_for_external, PrivacyAction, PrivacyFilter};
use work_review_core::project::{
    validate_and_normalize_project, validate_project_rule, ProjectDefinition, ProjectResolver,
    ProjectRule, ProjectRuleOp, ProjectUsage, MAX_PROJECT_COUNT, MAX_RULES_PER_PROJECT,
    UNASSIGNED_PROJECT_KEY,
};

use super::shared::{
    collect_privacy_filters, filter_activities_by_privacy, is_iso_date,
    update_app_config_with_source, ConfigWriteSource,
};

const PREVIEW_ACTIVITY_PAGE_SIZE: usize = 5_000;

fn allocate_unique_project_key(base_key: &str, existing: &[ProjectDefinition]) -> String {
    if !base_key.is_empty()
        && base_key != UNASSIGNED_PROJECT_KEY
        && !existing.iter().any(|p| p.key == base_key)
    {
        return base_key.to_string();
    }
    for n in 2usize..=10_000 {
        let suffix = format!("-{n}");
        let max_base_len = 48usize.saturating_sub(suffix.len());
        let trimmed_base: String = base_key
            .chars()
            .take(max_base_len)
            .collect::<String>()
            .trim_end_matches('-')
            .to_string();
        let candidate = if trimmed_base.is_empty() {
            format!("project{suffix}")
        } else {
            format!("{trimmed_base}{suffix}")
        };
        if candidate != UNASSIGNED_PROJECT_KEY && !existing.iter().any(|p| p.key == candidate) {
            return candidate;
        }
    }
    format!(
        "project-{}",
        uuid::Uuid::new_v4().simple().to_string()[..8].to_lowercase()
    )
}

fn upsert_project_in_list(
    projects: &mut Vec<ProjectDefinition>,
    project: ProjectDefinition,
) -> Result<(), AppError> {
    let is_create = project.key.trim().is_empty();
    let mut validated = validate_and_normalize_project(project)?;
    if is_create {
        if projects.len() >= MAX_PROJECT_COUNT {
            return Err(AppError::Config(format!(
                "最多支持配置 {MAX_PROJECT_COUNT} 个项目"
            )));
        }
        validated.key = allocate_unique_project_key(&validated.key, projects);
        projects.push(validated);
    } else if let Some(existing) = projects.iter_mut().find(|p| p.key == validated.key) {
        *existing = validated;
    } else {
        if projects.len() >= MAX_PROJECT_COUNT {
            return Err(AppError::Config(format!(
                "最多支持配置 {MAX_PROJECT_COUNT} 个项目"
            )));
        }
        projects.push(validated);
    }
    Ok(())
}

fn is_duplicate_project_rule(existing: &ProjectRule, candidate: &ProjectRule) -> bool {
    existing.field == candidate.field
        && existing.op == candidate.op
        && match candidate.op {
            ProjectRuleOp::Regex => existing.value.trim() == candidate.value.trim(),
            ProjectRuleOp::Equals | ProjectRuleOp::Contains => existing
                .value
                .trim()
                .eq_ignore_ascii_case(candidate.value.trim()),
        }
}

/// 隐私优先预处理：在执行项目规则匹配之前按当前隐私策略丢弃 `Skip` 活动，
/// 并将 `Anonymize` 活动的窗口标题 / URL / 可执行路径置空，避免通过项目归类间接推断受保护内容。
fn apply_privacy_for_project_matching(filter: &PrivacyFilter, act: &mut Activity) -> bool {
    match filter.check_privacy_full(&act.app_name, &act.window_title, act.browser_url.as_deref()) {
        PrivacyAction::Skip => false,
        PrivacyAction::Anonymize => {
            act.window_title.clear();
            act.browser_url = None;
            act.executable_path = None;
            true
        }
        PrivacyAction::Record => true,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRulePreviewSample {
    pub date: String,
    pub app_name: String,
    pub window_title: String,
    pub browser_url: Option<String>,
    pub duration: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRulePreviewResult {
    pub date_from: String,
    pub date_to: String,
    pub matched_count: usize,
    pub total_duration: i64,
    pub sample_activities: Vec<ProjectRulePreviewSample>,
}

/// 获取项目 / 标签工时归集定义列表
#[tauri::command]
pub async fn get_projects(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<ProjectDefinition>, AppError> {
    let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(guard.config.projects.clone())
}

/// 新增或更新项目定义（校验名称、颜色与正则安全上限；新建空 key 自动分配唯一标识避免覆盖已有项目）
#[tauri::command]
pub async fn save_project(
    project: ProjectDefinition,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<ProjectDefinition>, AppError> {
    let is_create = project.key.trim().is_empty();
    let validated = validate_and_normalize_project(project)?;
    let updated = update_app_config_with_source(
        app,
        state.inner(),
        ConfigWriteSource::Local,
        move |previous| {
            let mut config = previous.clone();
            let mut input = validated.clone();
            if is_create {
                input.key.clear();
            }
            upsert_project_in_list(&mut config.projects, input)?;
            Ok(config)
        },
    )?;
    Ok(updated.projects)
}

/// 原子地为指定项目追加一条规则（服务端读-改-写，避免前端持有旧快照覆盖并发修改）
#[tauri::command]
pub async fn add_project_rule(
    project_key: String,
    rule: ProjectRule,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<ProjectDefinition>, AppError> {
    let target_key = project_key.trim().to_string();
    if target_key.is_empty() {
        return Err(AppError::Config("项目标识不能为空".to_string()));
    }
    validate_project_rule(&rule)?;
    let normalized_rule = ProjectRule {
        field: rule.field,
        op: rule.op,
        value: rule.value.trim().to_string(),
    };
    let updated = update_app_config_with_source(
        app,
        state.inner(),
        ConfigWriteSource::Local,
        move |previous| {
            let mut config = previous.clone();
            let Some(existing) = config.projects.iter_mut().find(|p| p.key == target_key) else {
                return Err(AppError::Config(format!("未找到项目: {target_key}")));
            };
            let duplicate = existing
                .rules
                .iter()
                .any(|r| is_duplicate_project_rule(r, &normalized_rule));
            if !duplicate {
                if existing.rules.len() >= MAX_RULES_PER_PROJECT {
                    return Err(AppError::Config(format!(
                        "单个项目最多支持 {MAX_RULES_PER_PROJECT} 条规则"
                    )));
                }
                let mut candidate = existing.clone();
                candidate.rules.push(normalized_rule.clone());
                *existing = validate_and_normalize_project(candidate)?;
            }
            Ok(config)
        },
    )?;
    Ok(updated.projects)
}

/// 删除项目定义
#[tauri::command]
pub async fn delete_project(
    key: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<ProjectDefinition>, AppError> {
    let trimmed = key.trim().to_string();
    if trimmed.is_empty() {
        return Err(AppError::Config("项目标识不能为空".to_string()));
    }
    let updated = update_app_config_with_source(
        app,
        state.inner(),
        ConfigWriteSource::Local,
        move |previous| {
            let mut config = previous.clone();
            config.projects.retain(|p| p.key != trimmed);
            Ok(config)
        },
    )?;
    Ok(updated.projects)
}

/// 由 Rust 后端按隐私优先策略解析单条活动的归属项目（避免前端重复实现正则引擎导致语义漂移或 ReDoS）。
#[tauri::command]
pub async fn resolve_activity_project(
    id: i64,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<ProjectDefinition>, AppError> {
    let (database, ignored_apps, excluded_domains, privacy_config, projects) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&guard);
        (
            guard.database.clone(),
            ignored_apps,
            excluded_domains,
            guard.config.privacy.clone(),
            guard.config.projects.clone(),
        )
    };

    tokio::task::spawn_blocking(move || {
        let Some(activity) = database.get_activity_by_id(id)? else {
            return Ok(None);
        };
        let mut filtered =
            filter_activities_by_privacy(vec![activity], &ignored_apps, &excluded_domains);
        let Some(mut act) = filtered.pop() else {
            return Ok(None);
        };
        let filter = PrivacyFilter::from_config(&privacy_config);
        if !apply_privacy_for_project_matching(&filter, &mut act) {
            return Ok(None);
        }
        let resolver = ProjectResolver::from_projects(&projects);
        Ok(resolver.resolve(&act).cloned())
    })
    .await
    .map_err(|e| AppError::Unknown(format!("解析活动项目归属任务异常: {e}")))?
}

/// 预览单条规则在近 7 天（截止至 `date`，默认今天）命中的活动与时长；隐私规则优先于项目规则。
#[tauri::command]
pub async fn preview_project_rule(
    rule: ProjectRule,
    date: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<ProjectRulePreviewResult, AppError> {
    validate_project_rule(&rule)?;
    let normalized_rule = ProjectRule {
        field: rule.field,
        op: rule.op,
        value: rule.value.trim().to_string(),
    };
    let anchor = match date.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => {
            if !is_iso_date(value) {
                return Err(AppError::Config(format!("预览日期格式无效: {value}")));
            }
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("无效日期: {e}")))?
        }
        None => chrono::Local::now().date_naive(),
    };
    let start = anchor - chrono::Duration::days(6);
    let date_from = start.format("%Y-%m-%d").to_string();
    let date_to = anchor.format("%Y-%m-%d").to_string();

    let (database, ignored_apps, excluded_domains, privacy_config) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&guard);
        (
            guard.database.clone(),
            ignored_apps,
            excluded_domains,
            guard.config.privacy.clone(),
        )
    };

    let date_from_bg = date_from.clone();
    let date_to_bg = date_to.clone();
    tokio::task::spawn_blocking(move || {
        let filter = PrivacyFilter::from_config(&privacy_config);

        // 循环外一次性预编译正则规则，并游标分页扫描完整 7 天记录避免行数截断。
        let preview_project = ProjectDefinition {
            key: "__preview__".to_string(),
            name: "__preview__".to_string(),
            color: "#3B82F6".to_string(),
            archived: false,
            rules: vec![normalized_rule],
        };
        let resolver = ProjectResolver::from_projects(&[preview_project]);

        let mut matched_count = 0usize;
        let mut total_duration = 0i64;
        let mut samples = Vec::new();
        let mut after_id: i64 = 0;

        loop {
            let page = database.get_activities_in_date_range_after_id(
                &date_from_bg,
                &date_to_bg,
                after_id,
                PREVIEW_ACTIVITY_PAGE_SIZE,
            )?;
            if page.is_empty() {
                break;
            }
            let page_len = page.len();
            let last_id = page.last().and_then(|a| a.id);
            let filtered = filter_activities_by_privacy(page, &ignored_apps, &excluded_domains);

            for mut act in filtered {
                if !apply_privacy_for_project_matching(&filter, &mut act) {
                    continue;
                }
                if resolver.resolve(&act).is_none() {
                    continue;
                }
                matched_count += 1;
                total_duration += act.duration.max(0);
                if samples.len() < 12 {
                    sanitize_activity_for_external(&filter, &mut act, false);
                    let day_str = chrono::DateTime::from_timestamp(act.timestamp, 0)
                        .map(|dt| {
                            dt.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d")
                                .to_string()
                        })
                        .unwrap_or_default();
                    samples.push(ProjectRulePreviewSample {
                        date: day_str,
                        app_name: act.app_name,
                        window_title: act.window_title,
                        browser_url: act.browser_url,
                        duration: act.duration,
                    });
                }
            }

            if page_len < PREVIEW_ACTIVITY_PAGE_SIZE {
                break;
            }
            match last_id {
                Some(id) if id > after_id => after_id = id,
                _ => break,
            }
        }

        Ok(ProjectRulePreviewResult {
            date_from: date_from_bg,
            date_to: date_to_bg,
            matched_count,
            total_duration,
            sample_activities: samples,
        })
    })
    .await
    .map_err(|e| AppError::Unknown(format!("规则预览任务异常: {e}")))?
}

/// 按日期范围聚合项目工时统计（供本地 API `GET /v1/stats/projects` 复用；隐私规则优先）。
pub(crate) fn get_project_stats_inner(
    date_from: Option<String>,
    date_to: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<(String, String, Vec<ProjectUsage>), AppError> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let start_str = date_from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&today)
        .to_string();
    let end_str = date_to
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&start_str)
        .to_string();
    if !is_iso_date(&start_str) {
        return Err(AppError::Config(format!("开始日期格式无效: {start_str}")));
    }
    if !is_iso_date(&end_str) {
        return Err(AppError::Config(format!("结束日期格式无效: {end_str}")));
    }

    let start_date = chrono::NaiveDate::parse_from_str(&start_str, "%Y-%m-%d")
        .map_err(|e| AppError::Config(format!("开始日期无效: {e}")))?;
    let end_date = chrono::NaiveDate::parse_from_str(&end_str, "%Y-%m-%d")
        .map_err(|e| AppError::Config(format!("结束日期无效: {e}")))?;
    let (start_date, end_date) = if start_date <= end_date {
        (start_date, end_date)
    } else {
        (end_date, start_date)
    };
    if (end_date - start_date).num_days() > 3650 {
        return Err(AppError::Config("查询范围不能超过 3650 天".to_string()));
    }

    let (database, segments, ignored_apps, excluded_domains, privacy_config, projects) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&guard);
        (
            guard.database.clone(),
            guard.config.effective_work_segments(),
            ignored_apps,
            excluded_domains,
            guard.config.privacy.clone(),
            guard.config.projects.clone(),
        )
    };

    let per_day = database.get_daily_stats_range_with_projects_filtered(
        start_date,
        end_date,
        &segments,
        &ignored_apps,
        &excluded_domains,
        &projects,
        Some(&privacy_config),
    )?;

    let mut merged: std::collections::HashMap<String, ProjectUsage> =
        std::collections::HashMap::new();
    for (_, day) in per_day {
        for p in day.project_usage {
            let entry = merged.entry(p.key.clone()).or_insert_with(|| ProjectUsage {
                key: p.key.clone(),
                name: p.name.clone(),
                color: p.color.clone(),
                duration: 0,
                activity_count: 0,
            });
            entry.duration += p.duration;
            entry.activity_count += p.activity_count;
        }
    }
    let mut projects_out: Vec<ProjectUsage> = merged.into_values().collect();
    work_review_core::project::sort_project_usage(&mut projects_out);

    Ok((
        start_date.format("%Y-%m-%d").to_string(),
        end_date.format("%Y-%m-%d").to_string(),
        projects_out,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use work_review_core::config::{AppPrivacyRule, PrivacyConfig, PrivacyLevel};
    use work_review_core::database::{Activity, Database};
    use work_review_core::project::{ProjectRuleField, ProjectRuleOp};

    #[test]
    fn 隐私规则应优先于项目规则且支持跨天范围归集() {
        let db = Database::new(std::path::Path::new(":memory:")).expect("in-memory db");
        let d1 = chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let d2 = chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let ts1 = chrono::TimeZone::from_local_datetime(
            &chrono::Local,
            &d1.and_hms_opt(10, 0, 0).unwrap(),
        )
        .earliest()
        .unwrap()
        .timestamp();
        let ts2 = chrono::TimeZone::from_local_datetime(
            &chrono::Local,
            &d2.and_hms_opt(11, 0, 0).unwrap(),
        )
        .earliest()
        .unwrap()
        .timestamp();

        let make_act = |ts: i64, app: &str, title: &str, dur: i64| Activity {
            id: None,
            timestamp: ts,
            app_name: app.to_string(),
            window_title: title.to_string(),
            screenshot_path: String::new(),
            ocr_text: None,
            category: "development".to_string(),
            duration: dur,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };
        db.insert_activity(&make_act(ts1, "VS Code", "Work_Review - main.rs", 600))
            .unwrap();
        db.insert_activity(&make_act(ts2, "1Password", "Work_Review vault", 300))
            .unwrap();
        // 脱敏应用的窗口标题不应命中基于标题的项目规则，也不应泄漏到项目统计中
        db.insert_activity(&make_act(ts2, "WeChat", "Work_Review 秘密讨论", 250))
            .unwrap();
        db.insert_activity(&make_act(ts2, "VS Code", "Work_Review - lib.rs", 400))
            .unwrap();

        let privacy_config = PrivacyConfig {
            app_rules: vec![
                AppPrivacyRule {
                    app_name: "1Password".to_string(),
                    level: PrivacyLevel::Ignored,
                },
                AppPrivacyRule {
                    app_name: "WeChat".to_string(),
                    level: PrivacyLevel::Anonymized,
                },
            ],
            ..PrivacyConfig::default()
        };

        let projects = vec![ProjectDefinition {
            key: "wr".to_string(),
            name: "Work Review".to_string(),
            color: "#3B82F6".to_string(),
            archived: false,
            rules: vec![ProjectRule {
                field: ProjectRuleField::Title,
                op: ProjectRuleOp::Contains,
                value: "Work_Review".to_string(),
            }],
        }];
        let per_day = db
            .get_daily_stats_range_with_projects_filtered(
                d1,
                d2,
                &[],
                &["1password".to_string()],
                &[],
                &projects,
                Some(&privacy_config),
            )
            .unwrap();
        let wr_total: i64 = per_day
            .iter()
            .flat_map(|(_, s)| s.project_usage.iter())
            .filter(|p| p.key == "wr")
            .map(|p| p.duration)
            .sum();
        // 1Password 被隐私忽略，WeChat 被设为仅记时长（脱敏），两者均不得通过标题命中项目（600 + 400 = 1000）
        assert_eq!(wr_total, 1000);
    }

    #[test]
    fn 新建同名项目应分配唯一key而不覆盖已有项目且正则去重保持大小写敏感() {
        let mut projects = Vec::new();
        let first = ProjectDefinition {
            key: String::new(),
            name: "Work Review".to_string(),
            color: "#3B82F6".to_string(),
            archived: true,
            rules: vec![ProjectRule {
                field: ProjectRuleField::Title,
                op: ProjectRuleOp::Contains,
                value: "Work_Review".to_string(),
            }],
        };
        upsert_project_in_list(&mut projects, first).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].key, "work-review");

        // 同名新建（空 key）必须自动后缀去重，不得覆盖原项目的规则与归档状态
        let second = ProjectDefinition {
            key: String::new(),
            name: "Work Review".to_string(),
            color: "#10B981".to_string(),
            archived: false,
            rules: vec![],
        };
        upsert_project_in_list(&mut projects, second).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].key, "work-review");
        assert!(projects[0].archived);
        assert_eq!(projects[0].rules.len(), 1);
        assert_eq!(projects[1].key, "work-review-2");
        assert!(!projects[1].archived);

        // 显式 key 更新应精确覆盖对应项目
        let update_first = ProjectDefinition {
            key: "work-review".to_string(),
            name: "Work Review Renamed".to_string(),
            color: "#EF4444".to_string(),
            archived: false,
            rules: vec![],
        };
        upsert_project_in_list(&mut projects, update_first).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].name, "Work Review Renamed");
        assert_eq!(projects[1].key, "work-review-2");

        // 正则去重不得把 \d+ 与 \D+ 视为同一条规则；Contains 仍不区分大小写
        let regex_digit = ProjectRule {
            field: ProjectRuleField::Title,
            op: ProjectRuleOp::Regex,
            value: r"\d+".to_string(),
        };
        let regex_non_digit = ProjectRule {
            field: ProjectRuleField::Title,
            op: ProjectRuleOp::Regex,
            value: r"\D+".to_string(),
        };
        assert!(!is_duplicate_project_rule(&regex_digit, &regex_non_digit));
        assert!(is_duplicate_project_rule(&regex_digit, &regex_digit));

        let contains_upper = ProjectRule {
            field: ProjectRuleField::Domain,
            op: ProjectRuleOp::Contains,
            value: "GitHub.com".to_string(),
        };
        let contains_lower = ProjectRule {
            field: ProjectRuleField::Domain,
            op: ProjectRuleOp::Contains,
            value: "github.com".to_string(),
        };
        assert!(is_duplicate_project_rule(&contains_upper, &contains_lower));
    }
}
