//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::analysis::AppLocale;
use crate::config::AppConfig;
use crate::database::DailyReport;
use crate::error::AppError;
use crate::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

use super::shared::{collect_privacy_filters, filter_activities_by_privacy, persist_app_config};

fn resolve_saved_report_metadata(
    configured_mode: &crate::config::AiMode,
    configured_model_name: &str,
    used_ai: bool,
) -> (String, Option<String>) {
    let configured_mode = format!("{configured_mode:?}").to_lowercase();

    match (configured_mode.as_str(), used_ai) {
        ("summary", false) => ("local".to_string(), None),
        ("cloud", false) => ("local".to_string(), None),
        (_, false) => (configured_mode, None),
        _ => {
            let model_name = configured_model_name.trim();
            (
                configured_mode,
                if model_name.is_empty() {
                    None
                } else {
                    Some(model_name.to_string())
                },
            )
        }
    }
}

fn update_daily_report_ai_order_cache(
    config: &mut AppConfig,
    ai_order: Option<Vec<String>>,
) -> bool {
    let Some(ai_order) = ai_order else {
        return false;
    };
    if ai_order.is_empty() || config.daily_report_last_ai_order == ai_order {
        return false;
    }

    config.daily_report_last_ai_order = ai_order;
    true
}

#[allow(dead_code)]
fn normalize_saved_report_ai_mode(value: &str) -> String {
    value.trim().to_lowercase()
}

fn build_daily_report_export_path(export_dir: &Path, date: &str) -> PathBuf {
    let safe_date = date.replace(['/', '\\'], "-");
    export_dir.join(format!("{safe_date}.md"))
}

fn export_daily_report_markdown(
    export_dir: &Path,
    date: &str,
    content: &str,
) -> Result<(), AppError> {
    let output_path = build_daily_report_export_path(export_dir, date);
    // Retry up to 3 times to handle transient issues (disk busy, antivirus lock, etc.)
    let mut last_err = None;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(500 * attempt as u64));
            log::warn!(
                "日报自动导出重试第 {} 次: {}",
                attempt,
                output_path.display()
            );
        }
        match std::fs::create_dir_all(export_dir) {
            Ok(()) => {}
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        }
        match std::fs::write(&output_path, content) {
            Ok(()) => {
                log::info!("日报自动导出成功: {}", output_path.display());
                return Ok(());
            }
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        }
    }
    Err(AppError::Unknown(format!(
        "日报导出失败（已重试3次）: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "未知错误".to_string())
    )))
}

/// 生成日报
pub(crate) async fn generate_report_inner(
    date: String,
    force: Option<bool>,
    locale: Option<String>,
    app: &AppHandle,
    state: &Arc<Mutex<AppState>>,
) -> Result<String, AppError> {
    let report_locale = AppLocale::from_option(locale.as_deref());
    let report_locale_code = report_locale.as_code();
    // 如果不是强制重新生成，先检查缓存
    if !force.unwrap_or(false) {
        let state_guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        if let Ok(Some(cached)) = state_guard
            .database
            .get_report(&date, Some(report_locale_code))
        {
            log::info!("使用缓存日报: {date}");
            return Ok(cached.content);
        }
    }

    let (config, stats, activities, data_dir) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let segments = state.config.effective_work_segments();
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
        let stats = state.database.get_daily_stats_with_segments_filtered(
            &date,
            &segments,
            &ignored_apps,
            &excluded_domains,
        )?;
        // 生成日报时获取最多 2000 条记录
        let raw_activities = state.database.get_timeline(&date, Some(2000), None)?;
        let activities =
            filter_activities_by_privacy(raw_activities, &ignored_apps, &excluded_domains);
        (
            state.config.clone(),
            stats,
            activities,
            state.data_dir.clone(),
        )
    };

    let avatar_start_state = {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.avatar_generating_report = true;
        let avatar_state = crate::avatar_engine::apply_avatar_visual_settings(
            crate::avatar_engine::derive_avatar_state(
                &state.avatar_state.app_name,
                "",
                None,
                state.avatar_state.is_idle,
                true,
            ),
            state.config.avatar_opacity,
            &state.config.avatar_preset,
            &state.config.avatar_persona,
        );
        state.avatar_state = avatar_state.clone();
        if state.config.avatar_enabled {
            Some(avatar_state)
        } else {
            None
        }
    };

    if let Some(avatar_state) = avatar_start_state.as_ref() {
        crate::avatar_engine::emit_avatar_state(app, avatar_state);
        crate::avatar_engine::emit_avatar_bubble(
            app,
            &crate::avatar_engine::AvatarBubblePayload::info(match report_locale {
                AppLocale::ZhCn => "开始整理日报，稍等我一下。",
                AppLocale::ZhTw => "開始整理日報，稍等我一下。",
                AppLocale::En => "I'm preparing your daily report. Give me a moment.",
            }),
        );
    }

    // 创建分析器（使用 text_model 配置）
    let analyzer = crate::analysis::create_analyzer(
        config.ai_mode,
        config.text_model.provider,
        &config.text_model.endpoint,
        &config.text_model.model,
        config.text_model.api_key.as_deref(),
        &config.daily_report_custom_prompt,
        config.daily_report_system_prompt_override.as_deref(),
        report_locale,
        config.daily_report_pinned_blocks.clone(),
        if config.daily_report_last_ai_order.is_empty() {
            None
        } else {
            Some(config.daily_report_last_ai_order.clone())
        },
    );

    // 生成报告（spawn 隔离 panic，防止内部错误杀死整个 tokio 线程）
    // 外层加 300 秒总超时，防止 AI 调用卡死后前端永远等待
    let screenshots_dir = data_dir.clone();
    let date_gen = date.clone();
    let category_name_overrides: std::collections::HashMap<String, String> = config
        .custom_categories
        .iter()
        .map(|c| (c.key.clone(), c.name.clone()))
        .collect();
    let semantic_name_overrides: std::collections::HashMap<String, String> = config
        .custom_semantic_categories
        .iter()
        .map(|c| (c.key.clone(), c.name.clone()))
        .collect();
    let spawn_result = tokio::spawn(async move {
        analyzer
            .generate_report(
                &date_gen,
                &stats,
                &activities,
                &screenshots_dir,
                report_locale,
                category_name_overrides,
                semantic_name_overrides,
            )
            .await
    });

    let report_result =
        match tokio::time::timeout(std::time::Duration::from_secs(300), spawn_result).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(work_review_core::error::AppError::Analysis(
                match report_locale {
                    AppLocale::ZhCn => "日报生成过程中发生内部错误，请重试".to_string(),
                    AppLocale::ZhTw => "日報生成過程中發生內部錯誤，請重試".to_string(),
                    AppLocale::En => {
                        "Internal error during report generation, please retry".to_string()
                    }
                },
            )),
            Err(_) => Err(work_review_core::error::AppError::Analysis(
                match report_locale {
                    AppLocale::ZhCn => "日报生成超时，请稍后重试".to_string(),
                    AppLocale::ZhTw => "日報生成逾時，請稍後重試".to_string(),
                    AppLocale::En => {
                        "Report generation timed out, please try again later".to_string()
                    }
                },
            )),
        };

    let avatar_finish_state = {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.avatar_generating_report = false;
        let avatar_state = crate::avatar_engine::apply_avatar_visual_settings(
            crate::avatar_engine::derive_avatar_state(
                &state.avatar_state.app_name,
                "",
                None,
                state.avatar_state.is_idle,
                false,
            ),
            state.config.avatar_opacity,
            &state.config.avatar_preset,
            &state.config.avatar_persona,
        );
        state.avatar_state = avatar_state.clone();
        if state.config.avatar_enabled {
            Some(avatar_state)
        } else {
            None
        }
    };

    if let Some(avatar_state) = avatar_finish_state.as_ref() {
        crate::avatar_engine::emit_avatar_state(app, avatar_state);
        let bubble = if report_result.is_ok() {
            crate::avatar_engine::AvatarBubblePayload::success(match report_locale {
                AppLocale::ZhCn => "日报整理好了，可以回来看看。",
                AppLocale::ZhTw => "日報整理好了，可以回來看看。",
                AppLocale::En => "Your daily report is ready. You can check it now.",
            })
        } else {
            crate::avatar_engine::AvatarBubblePayload::info(match report_locale {
                AppLocale::ZhCn => "这次日报整理失败了，稍后可以再试。",
                AppLocale::ZhTw => "這次日報整理失敗了，稍後可以再試。",
                AppLocale::En => "This report run failed. Please try again later.",
            })
        };
        crate::avatar_engine::emit_avatar_bubble(app, &bubble);
    }

    let generated_report = report_result?;
    let report = generated_report.content.clone();
    let (saved_ai_mode, saved_model_name) = resolve_saved_report_metadata(
        &config.ai_mode,
        &config.text_model.model,
        generated_report.used_ai,
    );

    // 保存报告
    {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let daily_report = DailyReport {
            date: date.clone(),
            locale: report_locale_code.to_string(),
            content: report.clone(),
            ai_mode: saved_ai_mode,
            model_name: saved_model_name,
            fallback_reason: generated_report.fallback_reason.clone(),
            created_at: chrono::Utc::now().timestamp(),
        };
        state.database.save_report(&daily_report)?;
    }

    if let Some(ai_order) = generated_report.ai_order.clone() {
        let config_to_persist = {
            let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
            let mut next_config = state.config.clone();
            if update_daily_report_ai_order_cache(&mut next_config, Some(ai_order)) {
                Some(next_config)
            } else {
                None
            }
        };

        if let Some(next_config) = config_to_persist {
            if let Err(error) = persist_app_config(next_config, app.clone(), state) {
                log::warn!("缓存 AI 段落编排顺序失败: {error}");
            }
        }
    }

    if config.daily_report_auto_export {
        if let Some(export_dir) = config.daily_report_export_dir.as_deref() {
            if let Err(e) = export_daily_report_markdown(Path::new(export_dir), &date, &report) {
                log::error!("日报自动导出失败（日报已保存到数据库，仅导出文件失败）: {e:?}");
                // Do NOT propagate the error — report is already saved in the database.
                // Export failure should not make the entire generation appear to have failed.
            }
        }
    }

    Ok(report)
}

struct ReportGenerationGuard {
    state: Arc<Mutex<AppState>>,
}

impl Drop for ReportGenerationGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.generating_report = false;
        }
    }
}

#[tauri::command]
pub async fn generate_report(
    date: String,
    force: Option<bool>,
    locale: Option<String>,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    {
        let mut s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        if s.generating_report {
            return Err(AppError::Unknown("日报正在生成中，请稍候".to_string()));
        }
        s.generating_report = true;
    }
    let _guard = ReportGenerationGuard {
        state: state.inner().clone(),
    };
    generate_report_inner(date, force, locale, &app, state.inner()).await
}

/// 获取已保存的日报
pub(crate) fn get_saved_report_inner(
    date: String,
    locale: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<Option<DailyReport>, AppError> {
    let report_locale = AppLocale::from_option(locale.as_deref());
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let saved = state
        .database
        .get_report(&date, Some(report_locale.as_code()))?;
    let Some(mut report) = saved else {
        return Ok(None);
    };

    // 用最新的 stats 重新渲染统计区块，解决 issue #80：保存的 markdown 里固化的时长
    // 数字会随着工作日继续推进而变得陈旧。老报告若没有占位符标记则原样返回。
    let segments = state.config.effective_work_segments();
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
    if let Ok(live_stats) = state.database.get_daily_stats_with_segments_filtered(
        &date,
        &segments,
        &ignored_apps,
        &excluded_domains,
    ) {
        let category_name_overrides: std::collections::HashMap<String, String> = state
            .config
            .custom_categories
            .iter()
            .map(|c| (c.key.clone(), c.name.clone()))
            .collect();
        let semantic_name_overrides: std::collections::HashMap<String, String> = state
            .config
            .custom_semantic_categories
            .iter()
            .map(|c| (c.key.clone(), c.name.clone()))
            .collect();
        report.content = crate::analysis::report_blocks::render_report_with_live_stats(
            &report.content,
            &live_stats,
            report_locale,
            &category_name_overrides,
            &semantic_name_overrides,
        );
    }

    Ok(Some(report))
}

#[tauri::command]
pub async fn get_saved_report(
    date: String,
    locale: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<DailyReport>, AppError> {
    get_saved_report_inner(date, locale, state.inner())
}

/// 更新已保存日报的内容（用于结构化编辑）
#[tauri::command]
pub async fn update_report_content(
    date: String,
    locale: Option<String>,
    content: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let report_locale = AppLocale::from_option(locale.as_deref());
    let locale_code = report_locale.as_code();
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let existing = state
        .database
        .get_report(&date, Some(locale_code))?
        .ok_or_else(|| {
            AppError::Database(rusqlite::Error::InvalidParameterName(
                "报告不存在".to_string(),
            ))
        })?;
    let updated = DailyReport {
        content,
        ..existing
    };
    state.database.save_report(&updated)?;
    Ok(())
}

/// 设置日报段落的钉选/隐藏偏好
#[tauri::command]
pub async fn set_report_block_preference(
    pinned_blocks: Vec<String>,
    hidden_blocks: Vec<String>,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let config = {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.config.daily_report_pinned_blocks = pinned_blocks;
        state.config.daily_report_hidden_blocks = hidden_blocks;
        state.config.clone()
    };
    persist_app_config(config, app, state.inner())?;
    Ok(())
}

pub(crate) fn export_report_markdown_inner(
    date: String,
    content: Option<String>,
    export_dir: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<String, AppError> {
    let (export_dir, saved_content) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let requested_export_dir = export_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(|dir| dir.to_string());
        let configured_export_dir = state
            .config
            .daily_report_export_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(|dir| dir.to_string());
        let export_dir = requested_export_dir
            .or(configured_export_dir)
            .ok_or_else(|| {
                AppError::Config(
                    "请先选择导出目录，或在设置中配置日报 Markdown 导出目录".to_string(),
                )
            })?;
        let saved_content = if let Some(content) = content {
            content
        } else {
            state
                .database
                .get_report(&date, Some("zh-CN"))?
                .ok_or_else(|| AppError::Config("未找到可导出的日报".to_string()))?
                .content
        };
        (export_dir, saved_content)
    };

    let export_dir_path = Path::new(&export_dir);
    export_daily_report_markdown(export_dir_path, &date, &saved_content)?;
    Ok(build_daily_report_export_path(export_dir_path, &date)
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub async fn export_report_markdown(
    date: String,
    content: Option<String>,
    export_dir: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    export_report_markdown_inner(date, content, export_dir, state.inner())
}

/// 验证 ISO `YYYY-MM-DD` 日期字符串
fn ensure_iso_date(value: &str, field: &str) -> Result<(), AppError> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|_| ())
        .map_err(|_| AppError::Config(format!("{field} 日期格式应为 YYYY-MM-DD")))
}

/// 将范围内的日报合并成一个 Markdown 文件
///
/// - 范围按 ISO 日期字符串比较（lexicographic 与日期序一致）
/// - locale 默认 zh-CN，与 `get_report` 一致
/// - 范围内一个日报都没有时返回错误，避免写出空文件让用户困惑
pub(crate) fn export_reports_range_inner(
    start_date: String,
    end_date: String,
    target_path: String,
    locale: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<(String, usize), AppError> {
    let start = start_date.trim();
    let end = end_date.trim();
    let target = target_path.trim();
    if start.is_empty() || end.is_empty() {
        return Err(AppError::Config("起止日期不能为空".to_string()));
    }
    if target.is_empty() {
        return Err(AppError::Config("请先选择导出文件路径".to_string()));
    }
    ensure_iso_date(start, "起始")?;
    ensure_iso_date(end, "结束")?;
    if start > end {
        return Err(AppError::Config("起始日期不能晚于结束日期".to_string()));
    }

    let locale_code = locale
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("zh-CN")
        .to_string();

    let reports = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state
            .database
            .get_reports_in_range(start, end, Some(&locale_code))?
    };

    if reports.is_empty() {
        return Err(AppError::Config(format!(
            "{start} 至 {end} 范围内未找到日报"
        )));
    }

    let mut markdown = String::new();
    markdown.push_str("# 工作日报合并导出\n\n");
    markdown.push_str(&format!("- 日期范围：{start} ~ {end}\n"));
    markdown.push_str(&format!(
        "- 导出时间：{}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    markdown.push_str(&format!("- 日报数量：{}\n", reports.len()));
    markdown.push_str(&format!("- 语言：{locale_code}\n\n"));
    markdown.push_str("---\n\n");

    for report in &reports {
        markdown.push_str(&format!("## {}\n\n", report.date));
        markdown.push_str(report.content.trim());
        markdown.push_str("\n\n---\n\n");
    }

    let target_path_buf = PathBuf::from(target);
    if let Some(parent) = target_path_buf.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&target_path_buf, markdown)?;
    Ok((target_path_buf.to_string_lossy().to_string(), reports.len()))
}

#[tauri::command]
pub async fn export_reports_range(
    start_date: String,
    end_date: String,
    target_path: String,
    locale: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<ExportReportsRangeResult, AppError> {
    let (path, count) =
        export_reports_range_inner(start_date, end_date, target_path, locale, state.inner())?;
    Ok(ExportReportsRangeResult { path, count })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReportsRangeResult {
    pub path: String,
    pub count: usize,
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiMode;
    use std::path::{Path, PathBuf};

    #[test]
    fn summary回退到基础模板时不应保留_ai_模型标签() {
        let (ai_mode, model_name) =
            resolve_saved_report_metadata(&AiMode::Summary, "gpt-5.4", false);

        assert_eq!(ai_mode, "local");
        assert_eq!(model_name, None);
    }

    #[test]
    fn 新_ai_段落顺序应缓存到配置且空结果不覆盖已有缓存() {
        let mut config = crate::config::AppConfig::default();
        let first_order = vec!["APP_USAGE_TABLE".to_string(), "CATEGORY_TABLE".to_string()];

        assert!(update_daily_report_ai_order_cache(
            &mut config,
            Some(first_order.clone())
        ));
        assert_eq!(config.daily_report_last_ai_order, first_order);

        assert!(!update_daily_report_ai_order_cache(&mut config, None));
        assert_eq!(
            config.daily_report_last_ai_order,
            vec!["APP_USAGE_TABLE".to_string(), "CATEGORY_TABLE".to_string(),]
        );

        assert!(!update_daily_report_ai_order_cache(
            &mut config,
            Some(Vec::new())
        ));
        assert_eq!(
            config.daily_report_last_ai_order,
            vec!["APP_USAGE_TABLE".to_string(), "CATEGORY_TABLE".to_string(),]
        );
    }

    #[test]
    fn ai成功生成时应保留实际配置的模式与模型() {
        let (ai_mode, model_name) =
            resolve_saved_report_metadata(&AiMode::Summary, "gpt-5.4", true);

        assert_eq!(ai_mode, "summary");
        assert_eq!(model_name, Some("gpt-5.4".to_string()));
    }

    #[test]
    fn 保存的日报模式应统一转为小写() {
        assert_eq!(normalize_saved_report_ai_mode("Summary"), "summary");
        assert_eq!(normalize_saved_report_ai_mode(" local "), "local");
    }

    #[test]
    fn 日报导出路径应按日期生成_markdown_文件名() {
        let export_path = build_daily_report_export_path(Path::new("/tmp/reports"), "2026-03-29");

        assert_eq!(
            export_path,
            PathBuf::from("/tmp/reports").join("2026-03-29.md")
        );
    }

    #[test]
    fn 日报导出应写入_markdown_文件() {
        let temp_dir =
            std::env::temp_dir().join(format!("work-review-export-{}", uuid::Uuid::new_v4()));
        export_daily_report_markdown(&temp_dir, "2026-03-29", "# 工作日报\n\n测试内容")
            .expect("应能导出 Markdown");

        let output_path = temp_dir.join("2026-03-29.md");
        let content = std::fs::read_to_string(&output_path).expect("应能读取导出内容");
        assert_eq!(content, "# 工作日报\n\n测试内容");

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

}
