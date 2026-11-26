//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use work_review_core::analysis::obsidian::{render_obsidian_export, ExportFrontmatterMeta};
use work_review_core::analysis::report_blocks::wrap_block;
use work_review_core::analysis::AppLocale;
use work_review_core::config::{AiTaskKind, AppConfig};
use work_review_core::database::{Activity, DailyReport, DailyStats};
use work_review_core::error::AppError;
use work_review_core::git_commits::{
    collect_git_commits_for_date, format_git_commits_markdown, format_git_commits_prompt_context,
    sanitize_git_commits, total_commit_count, GitRepoSummary,
};
use work_review_core::privacy::PrivacyFilter;

use super::backup::write_export_file_atomic;
use super::shared::{collect_privacy_filters, filter_activities_by_privacy, persist_app_config};

fn resolve_saved_report_metadata(
    configured_mode: &work_review_core::config::AiMode,
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

fn build_daily_report_export_path(export_dir: &Path, date: &str) -> PathBuf {
    let safe_date = date.replace(['/', '\\'], "-");
    export_dir.join(format!("{safe_date}.md"))
}

fn build_export_frontmatter_meta(
    date: &str,
    stats: &DailyStats,
    activities: &[Activity],
    git_commit_count: usize,
) -> ExportFrontmatterMeta {
    let focus = work_review_core::work_intelligence::compute_focus_metrics(activities);
    ExportFrontmatterMeta {
        date: date.to_string(),
        total_minutes: (stats.total_duration / 60).max(0),
        work_minutes: (stats.work_time_duration / 60).max(0),
        focus_score: (focus.total_duration > 0).then_some(focus.focus_score),
        top_apps: stats
            .app_usage
            .iter()
            .take(5)
            .map(|app| app.app_name.clone())
            .collect(),
        git_commit_count,
    }
}

fn insert_git_commits_section(report: &str, git_markdown: &str) -> String {
    let trimmed_git = git_markdown.trim();
    if trimmed_git.is_empty() {
        return report.to_string();
    }
    let wrapped = wrap_block("GIT_COMMITS", trimmed_git);
    if let Some(pos) = report.find("\n<details>\n<summary>") {
        let before = report[..pos].trim_end();
        let after = report[pos..].trim_start();
        format!("{before}\n\n{wrapped}\n{after}")
    } else {
        format!("{}\n\n{wrapped}", report.trim_end())
    }
}

fn export_daily_report_markdown_with_options(
    export_dir: &Path,
    date: &str,
    content: &str,
    meta: Option<&ExportFrontmatterMeta>,
    include_frontmatter: bool,
    section_heading: Option<&str>,
) -> Result<(), AppError> {
    let output_path = build_daily_report_export_path(export_dir, date);
    if let Ok(file_meta) = std::fs::symlink_metadata(&output_path) {
        if file_meta.file_type().is_symlink() {
            return Err(AppError::Config(
                "导出目标文件为符号链接，已拒绝写入".to_string(),
            ));
        }
    }

    let trimmed_heading = section_heading.map(str::trim).filter(|s| !s.is_empty());
    let final_content = if include_frontmatter || trimmed_heading.is_some() {
        let existing = if trimmed_heading.is_some() {
            match std::fs::read_to_string(&output_path) {
                Ok(content) => Some(content),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            }
        } else {
            None
        };
        let fallback_meta = ExportFrontmatterMeta {
            date: date.to_string(),
            total_minutes: 0,
            work_minutes: 0,
            focus_score: None,
            top_apps: Vec::new(),
            git_commit_count: 0,
        };
        render_obsidian_export(
            existing.as_deref(),
            content,
            meta.unwrap_or(&fallback_meta),
            include_frontmatter,
            trimmed_heading,
        )
    } else {
        content.to_string()
    };

    let file_name = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Config("日报导出文件名无效".to_string()))?;
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
                last_err = Some(AppError::Io(e));
                continue;
            }
        }
        match write_export_file_atomic(export_dir, file_name, final_content.as_bytes()) {
            Ok(_) => {
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

#[cfg(test)]
fn export_daily_report_markdown(
    export_dir: &Path,
    date: &str,
    content: &str,
) -> Result<(), AppError> {
    export_daily_report_markdown_with_options(export_dir, date, content, None, false, None)
}

// JoinHandle 默认在 drop 时分离；父请求取消和超时都必须停止分析器。
struct AbortReportOnDrop(tokio::task::AbortHandle);

impl Drop for AbortReportOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn await_report_task<T>(
    task: tokio::task::JoinHandle<T>,
    timeout: std::time::Duration,
) -> Result<Result<T, tokio::task::JoinError>, tokio::time::error::Elapsed> {
    let _abort = AbortReportOnDrop(task.abort_handle());
    tokio::time::timeout(timeout, task).await
}

struct ReportAvatarGuard<'a> {
    state: &'a Mutex<AppState>,
    active: bool,
}

impl<'a> ReportAvatarGuard<'a> {
    fn start(
        state: &'a Mutex<AppState>,
    ) -> Result<(Self, Option<crate::avatar_engine::AvatarStatePayload>), AppError> {
        let avatar_start_state = {
            let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
            // localhost API 没有 command / assistant 的外层 guard，也要避免互相清理标志。
            if state.avatar_generating_report {
                return Err(AppError::Unknown("日报正在生成中，请稍候".to_string()));
            }
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
                state.config.avatar_body_hidden,
            );
            let avatar_start_state = state.config.avatar_enabled.then(|| avatar_state.clone());
            state.avatar_state = avatar_state;
            state.avatar_generating_report = true;
            avatar_start_state
        };
        Ok((
            Self {
                state,
                active: true,
            },
            avatar_start_state,
        ))
    }

    fn finish(&mut self) -> Result<Option<crate::avatar_engine::AvatarStatePayload>, AppError> {
        if !self.active {
            return Ok(None);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|e| AppError::Unknown(e.to_string()))?;
        state.avatar_generating_report = false;
        // 监控循环或配置更新可能已换成暂停等状态，不要用日报收尾覆盖它们。
        if state.avatar_state.is_generating_report {
            state.avatar_state = crate::avatar_engine::apply_avatar_visual_settings(
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
                state.config.avatar_body_hidden,
            );
        }
        self.active = false;
        Ok(state
            .config
            .avatar_enabled
            .then(|| state.avatar_state.clone()))
    }
}

impl Drop for ReportAvatarGuard<'_> {
    fn drop(&mut self) {
        // 取消也须立即发布复位状态：短任务可能尚未被监控循环观察到。
        // 只发状态，不发送成功/失败气泡；测试环境没有运行中的 AppHandle。
        match self.finish() {
            Ok(Some(avatar_state)) => {
                if let Some(app) = crate::APP_HANDLE.get() {
                    crate::avatar_engine::emit_avatar_state(app, &avatar_state);
                }
            }
            Ok(None) => {}
            Err(error) => log::warn!("清理日报桌宠状态失败: {error}"),
        }
    }
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
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
        let stats = super::stats::load_daily_stats_for_overview(&state, &date)?;
        // 生成日报时获取最多 2000 条记录
        let raw_activities = state.database.get_timeline(&date, Some(2000), None)?;
        let activities =
            filter_activities_by_privacy(raw_activities, &ignored_apps, &excluded_domains);
        // 日报上下文可能发给云端模型：采集后才新增的脱敏规则同样作用于历史记录。
        let activities =
            super::sanitize_activities_for_external(activities, &state.privacy_filter, true);
        (
            state.config.clone(),
            stats,
            activities,
            state.data_dir.clone(),
        )
    };

    let (mut avatar_guard, avatar_start_state) = ReportAvatarGuard::start(state)?;

    if let Some(avatar_state) = avatar_start_state.as_ref() {
        crate::avatar_engine::emit_avatar_state(app, avatar_state);
        crate::avatar_engine::emit_avatar_bubble(
            app,
            &crate::avatar_engine::AvatarBubblePayload::info(match report_locale {
                AppLocale::ZhCn => "开始整理日报，稍等我一下。",
                AppLocale::ZhTw => "開始整理日報，稍等我一下。",
                AppLocale::En => "I'm preparing your daily report. Give me a moment.",
                AppLocale::Ar => "أقوم بإعداد تقريرك اليومي. لحظة من فضلك.",
            }),
        );
    }

    let report_timeout =
        std::time::Duration::from_secs(config.report_generation_timeout_secs.max(60));
    let report_model = config.resolve_model_for_task(AiTaskKind::Report);

    // 若开启 Git 提交关联，先在阻塞池采集并按当前隐私规则脱敏
    let git_summaries = if config.git_report_enabled && !config.git_repo_paths.is_empty() {
        let repo_paths = config.git_repo_paths.clone();
        let author_filter = config.git_author_filter.clone();
        let date_for_git = date.clone();
        let raw = tokio::task::spawn_blocking(move || {
            collect_git_commits_for_date(&repo_paths, &date_for_git, author_filter.as_deref())
        })
        .await
        .unwrap_or_default();
        let privacy_filter = PrivacyFilter::from_config(&config.privacy);
        sanitize_git_commits(raw, &privacy_filter)
    } else {
        Vec::new()
    };

    let git_commit_count = total_commit_count(&git_summaries);
    let export_meta = build_export_frontmatter_meta(&date, &stats, &activities, git_commit_count);
    let git_prompt_context = format_git_commits_prompt_context(&git_summaries, report_locale);
    let effective_custom_prompt = match (
        config.daily_report_custom_prompt.trim(),
        git_prompt_context.trim(),
    ) {
        ("", "") => String::new(),
        (custom, "") => custom.to_string(),
        ("", git_ctx) => git_ctx.to_string(),
        (custom, git_ctx) => format!("{custom}\n\n{git_ctx}"),
    };

    // 创建分析器（按日报场景路由模型配置）
    let analyzer = work_review_core::analysis::create_analyzer(
        config.ai_mode,
        report_model.provider,
        &report_model.endpoint,
        &report_model.model,
        report_model.api_key.as_deref(),
        &effective_custom_prompt,
        config.daily_report_system_prompt_override.as_deref(),
        report_locale,
        config.daily_report_pinned_blocks.clone(),
        if config.daily_report_last_ai_order.is_empty() {
            None
        } else {
            Some(config.daily_report_last_ai_order.clone())
        },
        report_timeout,
    );

    // 生成报告（spawn 隔离 panic，防止内部错误杀死整个 tokio 线程）
    // 外层超时可配置（默认 300 秒），超时或父 Future 取消都会 abort 后台任务。
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
    let report_result = match await_report_task(spawn_result, report_timeout).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(work_review_core::error::AppError::Analysis(
            match report_locale {
                AppLocale::ZhCn => "日报生成过程中发生内部错误，请重试".to_string(),
                AppLocale::ZhTw => "日報生成過程中發生內部錯誤，請重試".to_string(),
                AppLocale::En => {
                    "Internal error during report generation, please retry".to_string()
                }
                AppLocale::Ar => {
                    "حدث خطأ داخلي أثناء إنشاء التقرير، يرجى المحاولة مرة أخرى".to_string()
                }
            },
        )),
        Err(_) => Err(work_review_core::error::AppError::Analysis(
            match report_locale {
                AppLocale::ZhCn => "日报生成超时，请稍后重试".to_string(),
                AppLocale::ZhTw => "日報生成逾時，請稍後重試".to_string(),
                AppLocale::En => "Report generation timed out, please try again later".to_string(),
                AppLocale::Ar => {
                    "انتهت مهلة إنشاء التقرير، يرجى المحاولة مرة أخرى لاحقاً".to_string()
                }
            },
        )),
    };

    let avatar_finish_state = avatar_guard.finish()?;

    if let Some(avatar_state) = avatar_finish_state.as_ref() {
        crate::avatar_engine::emit_avatar_state(app, avatar_state);
        let bubble = if report_result.is_ok() {
            crate::avatar_engine::AvatarBubblePayload::success(match report_locale {
                AppLocale::ZhCn => "日报整理好了，可以回来看看。",
                AppLocale::ZhTw => "日報整理好了，可以回來看看。",
                AppLocale::En => "Your daily report is ready. You can check it now.",
                AppLocale::Ar => "تقريرك اليومي جاهز. يمكنك التحقق منه الآن.",
            })
        } else {
            crate::avatar_engine::AvatarBubblePayload::info(match report_locale {
                AppLocale::ZhCn => "这次日报整理失败了，稍后可以再试。",
                AppLocale::ZhTw => "這次日報整理失敗了，稍後可以再試。",
                AppLocale::En => "This report run failed. Please try again later.",
                AppLocale::Ar => "فشل تشغيل هذا التقرير. يرجى المحاولة مرة أخرى لاحقاً.",
            })
        };
        crate::avatar_engine::emit_avatar_bubble(app, &bubble);
    }

    let generated_report = report_result?;
    let git_markdown = format_git_commits_markdown(&git_summaries, report_locale);
    let report = insert_git_commits_section(&generated_report.content, &git_markdown);
    let (saved_ai_mode, saved_model_name) = resolve_saved_report_metadata(
        &config.ai_mode,
        &report_model.model,
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
            let export_result = validate_export_dir(Path::new(export_dir)).and_then(|dir| {
                export_daily_report_markdown_with_options(
                    &dir,
                    &date,
                    &report,
                    Some(&export_meta),
                    config.daily_report_export_frontmatter,
                    config.daily_report_export_section_heading.as_deref(),
                )
            });
            if let Err(e) = export_result {
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
    if let Ok(live_stats) = super::stats::load_daily_stats_for_overview(&state, &date) {
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
        report.content = work_review_core::analysis::report_blocks::render_report_with_live_stats(
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

/// 校验规范化后的导出目录路径组件是否安全（纯函数，便于测试）：
/// - 任何组件不得以 '.' 开头（.ssh/.config/.config/autostart 等隐藏目录一并覆盖）
/// - 拒绝 Library/LaunchAgents、Library/LaunchDaemons 等自启动敏感目录
fn ensure_export_dir_components_safe(canonical: &Path) -> Result<(), AppError> {
    let components: Vec<String> = canonical
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();

    if components.iter().any(|name| name.starts_with('.')) {
        return Err(AppError::Config(
            "导出目录不能位于以 . 开头的隐藏目录内（如 .ssh、.config）".to_string(),
        ));
    }

    for pair in components.windows(2) {
        let parent = pair[0].to_ascii_lowercase();
        let child = pair[1].to_ascii_lowercase();
        if (parent == "library" && (child == "launchagents" || child == "launchdaemons"))
            || (parent == "programs" && child == "startup")
        {
            return Err(AppError::Config(
                "导出目录位于系统自启动目录，已拒绝写入".to_string(),
            ));
        }
    }
    Ok(())
}

/// 校验日报导出目录：必须是已存在的真实目录，且不落在隐藏目录 / 自启动等敏感位置。
/// 导出目录可由本地 API 请求方或前端传入，写入前必须校验，防止向 .ssh、
/// LaunchAgents 等位置写文件。返回规范化（已解析符号链接）后的路径。
pub(crate) fn validate_export_dir(dir: &Path) -> Result<PathBuf, AppError> {
    let canonical = dir
        .canonicalize()
        .map_err(|_| AppError::Config("导出目录不存在或无法访问，请先创建该目录".to_string()))?;
    if !canonical.is_dir() {
        return Err(AppError::Config("导出路径必须是已存在的目录".to_string()));
    }
    ensure_export_dir_components_safe(&canonical)?;
    Ok(canonical)
}

pub(crate) fn export_report_markdown_inner(
    date: String,
    content: Option<String>,
    export_dir: Option<String>,
    locale: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<String, AppError> {
    let date = date.trim().to_string();
    ensure_iso_date(&date, "日报日期")?;
    let report_locale = AppLocale::from_option(locale.as_deref());
    let (
        export_dir,
        saved_content,
        include_frontmatter,
        section_heading,
        frontmatter_source,
        git_source,
    ) = {
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
                .get_report(&date, Some(report_locale.as_code()))?
                .ok_or_else(|| AppError::Config("未找到可导出的日报".to_string()))?
                .content
        };
        let include_frontmatter = state.config.daily_report_export_frontmatter;
        let section_heading = state.config.daily_report_export_section_heading.clone();
        let frontmatter_source = if include_frontmatter {
            let stats =
                super::stats::load_daily_stats_for_overview(&state, &date).unwrap_or_default();
            let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
            let raw_activities = state
                .database
                .get_timeline(&date, Some(2000), None)
                .unwrap_or_default();
            let activities =
                filter_activities_by_privacy(raw_activities, &ignored_apps, &excluded_domains);
            Some((stats, activities))
        } else {
            None
        };
        let git_source = if include_frontmatter
            && state.config.git_report_enabled
            && !state.config.git_repo_paths.is_empty()
        {
            Some((
                state.config.git_repo_paths.clone(),
                state.config.git_author_filter.clone(),
                state.config.privacy.clone(),
            ))
        } else {
            None
        };
        (
            export_dir,
            saved_content,
            include_frontmatter,
            section_heading,
            frontmatter_source,
            git_source,
        )
    };

    let export_meta = frontmatter_source.map(|(stats, activities)| {
        let git_commit_count = git_source
            .map(|(paths, author, privacy)| {
                let raw = collect_git_commits_for_date(&paths, &date, author.as_deref());
                let filter = PrivacyFilter::from_config(&privacy);
                total_commit_count(&sanitize_git_commits(raw, &filter))
            })
            .unwrap_or(0);
        build_export_frontmatter_meta(&date, &stats, &activities, git_commit_count)
    });

    // 目录来自请求方/配置，写入前校验（要求已存在，拒绝隐藏/敏感目录）
    let export_dir_path = validate_export_dir(Path::new(&export_dir))?;
    export_daily_report_markdown_with_options(
        &export_dir_path,
        &date,
        &saved_content,
        export_meta.as_ref(),
        include_frontmatter,
        section_heading.as_deref(),
    )?;
    Ok(build_daily_report_export_path(&export_dir_path, &date)
        .to_string_lossy()
        .to_string())
}

/// 获取指定日期的本地 Git 仓库提交（脱敏后返回，供设置页预览与按需核对）
#[tauri::command]
pub async fn get_git_commits_for_date(
    date: String,
    repo_paths: Option<Vec<String>>,
    author_filter: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<GitRepoSummary>, AppError> {
    let date = date.trim().to_string();
    ensure_iso_date(&date, "Git 提交日期")?;
    let (paths, author, privacy_config) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let paths = repo_paths.unwrap_or_else(|| s.config.git_repo_paths.clone());
        let author = match author_filter {
            Some(value) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            None => s.config.git_author_filter.clone(),
        };
        (paths, author, s.config.privacy.clone())
    };
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let raw = tokio::task::spawn_blocking(move || {
        collect_git_commits_for_date(&paths, &date, author.as_deref())
    })
    .await
    .map_err(|e| AppError::Unknown(format!("读取 Git 提交失败: {e}")))?;
    let privacy_filter = PrivacyFilter::from_config(&privacy_config);
    Ok(sanitize_git_commits(raw, &privacy_filter))
}

#[tauri::command]
pub async fn export_report_markdown(
    date: String,
    content: Option<String>,
    export_dir: Option<String>,
    locale: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    export_report_markdown_inner(date, content, export_dir, locale, state.inner())
}

/// 验证 ISO `YYYY-MM-DD` 日期字符串
fn ensure_iso_date(value: &str, field: &str) -> Result<(), AppError> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|_| ())
        .map_err(|_| AppError::Config(format!("{field} 日期格式应为 YYYY-MM-DD")))
}

/// 合并导出 Markdown 的脚手架文案（标题、字段名、冒号），按界面语言输出：
/// [标题, 日期范围, 导出时间, 日报数量, 语言, 冒号]
fn range_export_scaffold_labels(locale: AppLocale) -> [&'static str; 6] {
    match locale {
        AppLocale::ZhCn => [
            "工作日报合并导出",
            "日期范围",
            "导出时间",
            "日报数量",
            "语言",
            "：",
        ],
        AppLocale::ZhTw => [
            "工作日報合併導出",
            "日期範圍",
            "導出時間",
            "日報數量",
            "語言",
            "：",
        ],
        AppLocale::En => [
            "Merged Daily Reports Export",
            "Date range",
            "Exported at",
            "Report count",
            "Language",
            ": ",
        ],
        AppLocale::Ar => [
            "تصدير التقارير اليومية المدمجة",
            "نطاق التاريخ",
            "وقت التصدير",
            "عدد التقارير",
            "اللغة",
            ": ",
        ],
    }
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

    let report_locale =
        AppLocale::from_option(locale.as_deref().map(str::trim).filter(|s| !s.is_empty()));
    let locale_code = report_locale.as_code();

    let reports = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state
            .database
            .get_reports_in_range(start, end, Some(locale_code))?
    };

    if reports.is_empty() {
        return Err(AppError::Config(format!(
            "{start} 至 {end} 范围内未找到日报"
        )));
    }

    // 脚手架文案随界面语言切换，避免非中文用户导出中文标题/字段名
    let [title, range_label, exported_at_label, count_label, language_label, colon] =
        range_export_scaffold_labels(report_locale);
    let mut markdown = String::new();
    markdown.push_str(&format!("# {title}\n\n"));
    markdown.push_str(&format!("- {range_label}{colon}{start} ~ {end}\n"));
    markdown.push_str(&format!(
        "- {exported_at_label}{colon}{}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    markdown.push_str(&format!("- {count_label}{colon}{}\n", reports.len()));
    markdown.push_str(&format!("- {language_label}{colon}{locale_code}\n\n"));
    markdown.push_str("---\n\n");

    for report in &reports {
        markdown.push_str(&format!("## {}\n\n", report.date));
        markdown.push_str(report.content.trim());
        markdown.push_str("\n\n---\n\n");
    }

    let target_path_buf = prepare_export_file_target(Path::new(target), &["md", "markdown"])?;
    std::fs::write(&target_path_buf, markdown)?;
    Ok((
        super::config::path_for_display(&target_path_buf),
        reports.len(),
    ))
}

/// 校验并准备用户选择的导出文件路径：
/// - 必须是绝对路径，扩展名在允许列表内，文件名不得以 `.` 开头；
/// - 路径中不得出现 `..`；父目录不存在时创建，随后按 [`validate_export_dir`] 校验
///   （拒绝隐藏目录、LaunchAgents 等敏感位置）。
pub(crate) fn prepare_export_file_target(
    target: &Path,
    allowed_extensions: &[&str],
) -> Result<PathBuf, AppError> {
    if !target.is_absolute() {
        return Err(AppError::Config("导出文件路径必须是绝对路径".to_string()));
    }
    if target
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(AppError::Config("导出文件路径不能包含 ..".to_string()));
    }
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| AppError::Config("导出文件名无效".to_string()))?;
    if file_name.starts_with('.') {
        return Err(AppError::Config("导出文件名不能以 . 开头".to_string()));
    }
    let extension_ok = target
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            allowed_extensions
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
        .unwrap_or(false);
    if !extension_ok {
        return Err(AppError::Config(format!(
            "导出文件扩展名应为 {}",
            allowed_extensions
                .iter()
                .map(|ext| format!(".{ext}"))
                .collect::<Vec<_>>()
                .join(" / ")
        )));
    }
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| AppError::Config("导出文件路径缺少所在目录".to_string()))?;
    // 先做不依赖文件系统的组件检查，再规范化最深已存在祖先目录，避免沿符号链接在敏感目录内创建子目录
    ensure_export_dir_components_safe(parent)?;
    for ancestor in parent.ancestors() {
        if ancestor.exists() {
            validate_export_dir(ancestor)?;
            break;
        }
    }
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
    }
    let canonical_parent = validate_export_dir(parent)?;
    let final_target = canonical_parent.join(file_name);
    if let Ok(meta) = std::fs::symlink_metadata(&final_target) {
        if meta.file_type().is_symlink() {
            return Err(AppError::Config(
                "导出目标文件为符号链接，已拒绝写入".to_string(),
            ));
        }
    }
    Ok(final_target)
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
    use std::path::{Path, PathBuf};
    use work_review_core::config::AiMode;

    #[test]
    fn 导出文件路径应校验扩展名隐藏目录与越界() {
        let base = std::env::temp_dir().join(format!("wr-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();

        let ok = prepare_export_file_target(&base.join("reports.md"), &["md"]).unwrap();
        assert!(ok.ends_with("reports.md"));
        // 父目录不存在时创建
        let nested =
            prepare_export_file_target(&base.join("sub").join("t.json"), &["json"]).unwrap();
        assert!(nested.parent().unwrap().is_dir());

        assert!(prepare_export_file_target(&base.join("reports.exe"), &["md"]).is_err());
        assert!(prepare_export_file_target(&base.join(".bashrc.md"), &["md"]).is_err());
        assert!(prepare_export_file_target(&base.join(".ssh").join("a.md"), &["md"]).is_err());
        assert!(!base.join(".ssh").exists(), "被拒绝的路径不应创建目录");
        assert!(
            prepare_export_file_target(&base.join("sub").join("..").join("a.md"), &["md"]).is_err()
        );
        assert!(prepare_export_file_target(Path::new("relative.md"), &["md"]).is_err());

        let _ = std::fs::remove_dir_all(&base);
    }

    // 只构造内存状态，不启动 Tauri、截图、后台服务或模型请求。
    fn report_test_state() -> Arc<Mutex<AppState>> {
        let config = AppConfig::default();
        let data_dir = std::env::temp_dir().join(format!("report-test-{}", uuid::Uuid::new_v4()));
        Arc::new(Mutex::new(AppState {
            config_load_status: work_review_core::config::ConfigLoadStatus::Loaded,
            database: work_review_core::database::Database::new(Path::new(":memory:")).unwrap(),
            privacy_filter: work_review_core::privacy::PrivacyFilter::from_config(&config.privacy),
            screenshot_service: crate::screenshot::ScreenshotService::new(
                &data_dir,
                &config.storage,
            ),
            storage_manager: crate::storage::StorageManager::new(&data_dir, config.storage.clone()),
            config,
            config_path: data_dir.join("config.json"),
            data_dir,
            is_recording: false,
            is_paused: false,
            avatar_state: crate::avatar_engine::default_avatar_state(),
            avatar_generating_report: false,
            generating_report: false,
            localhost_api_runtime: Default::default(),
            telegram_bot_runtime: Default::default(),
            wecom_bot_runtime: Default::default(),
            cached_active_window: None,
        }))
    }

    struct SignalOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for SignalOnDrop {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn 取消父请求应停止分析器并清理内外生成标志() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::oneshot;
        use tokio::time::{timeout, Duration};

        let state = report_test_state();
        state.lock().unwrap().generating_report = true;
        let calls = Arc::new(AtomicUsize::new(0));
        let child_calls = calls.clone();
        let (started_tx, started_rx) = oneshot::channel();
        let (proceed_tx, proceed_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let mut parent = Box::pin(async {
            // 与 command / assistant 一样，外层 guard 在共享 Future 被取消后释放。
            let _outer_guard = ReportGenerationGuard {
                state: state.clone(),
            };
            let (_avatar_guard, _) = ReportAvatarGuard::start(&state).unwrap();
            await_report_task(
                tokio::spawn(async move {
                    let _dropped = SignalOnDrop(Some(dropped_tx));
                    let _ = started_tx.send(());
                    let _ = proceed_rx.await;
                    child_calls.fetch_add(1, Ordering::SeqCst); // 模拟下一次模型请求
                }),
                Duration::from_secs(60),
            )
            .await
        });
        timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = &mut parent => panic!("父请求不应提前结束: {result:?}"),
                started = started_rx => started.unwrap(),
            }
        })
        .await
        .unwrap();
        assert!(state.lock().unwrap().avatar_generating_report);
        drop(parent);
        {
            let state = state.lock().unwrap();
            assert!(!state.generating_report);
            assert!(!state.avatar_generating_report);
            assert!(!state.avatar_state.is_generating_report);
        }
        let _ = proceed_tx.send(());
        timeout(Duration::from_secs(2), dropped_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn 超时也应停止分析器且正常结果和_panic_仍可区分() {
        use tokio::sync::oneshot;
        use tokio::time::{timeout, Duration};

        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _dropped = SignalOnDrop(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(await_report_task(task, Duration::ZERO).await.is_err());
        timeout(Duration::from_secs(2), dropped_rx)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            await_report_task(
                tokio::spawn(async { Ok::<_, &str>("report") }),
                Duration::from_secs(2)
            )
            .await
            .unwrap()
            .unwrap(),
            Ok("report")
        );
        assert_eq!(
            await_report_task(
                tokio::spawn(async { Err::<(), _>("analysis failed") }),
                Duration::from_secs(2)
            )
            .await
            .unwrap()
            .unwrap(),
            Err("analysis failed")
        );
        assert!(await_report_task(
            tokio::spawn(async { panic!("analyzer panic") }),
            Duration::from_secs(2)
        )
        .await
        .unwrap()
        .unwrap_err()
        .is_panic());
    }

    #[test]
    fn 头像收尾不覆盖新状态且旧_guard_不清理下一次生成() {
        let state = report_test_state();
        let (mut guard, _) = ReportAvatarGuard::start(&state).unwrap();
        // 共享层也拦截 localhost 等入口的重复生成，失败不能清理现有任务。
        assert!(ReportAvatarGuard::start(&state).is_err());
        assert!(state.lock().unwrap().avatar_generating_report);
        let newer_state = crate::avatar_engine::default_avatar_state();
        state.lock().unwrap().avatar_state = newer_state.clone();
        guard.finish().unwrap();
        assert_eq!(state.lock().unwrap().avatar_state, newer_state);
        assert!(!state.lock().unwrap().avatar_generating_report);

        let (next_guard, _) = ReportAvatarGuard::start(&state).unwrap();
        drop(guard);
        assert!(state.lock().unwrap().avatar_generating_report);
        state.lock().unwrap().avatar_state = newer_state.clone();
        drop(next_guard);
        assert_eq!(state.lock().unwrap().avatar_state, newer_state);
        assert!(!state.lock().unwrap().avatar_generating_report);
    }

    #[test]
    fn summary回退到基础模板时不应保留_ai_模型标签() {
        let (ai_mode, model_name) =
            resolve_saved_report_metadata(&AiMode::Summary, "gpt-5.4", false);

        assert_eq!(ai_mode, "local");
        assert_eq!(model_name, None);
    }

    #[test]
    fn 新_ai_段落顺序应缓存到配置且空结果不覆盖已有缓存() {
        let mut config = work_review_core::config::AppConfig::default();
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

    #[test]
    fn 导出目录组件校验应拦截隐藏目录与自启动目录() {
        use super::ensure_export_dir_components_safe;

        assert!(
            ensure_export_dir_components_safe(Path::new("/Users/demo/Documents/reports")).is_ok()
        );
        assert!(ensure_export_dir_components_safe(Path::new("/Users/demo/.ssh")).is_err());
        assert!(
            ensure_export_dir_components_safe(Path::new("/home/demo/.config/autostart")).is_err()
        );
        assert!(
            ensure_export_dir_components_safe(Path::new("/Users/demo/Library/LaunchAgents"))
                .is_err()
        );
        assert!(ensure_export_dir_components_safe(Path::new(
            "/Users/demo/Library/LaunchDaemons/sub"
        ))
        .is_err());
        // Library 下的普通目录不受影响
        assert!(ensure_export_dir_components_safe(Path::new(
            "/Users/demo/Library/CloudStorage/reports"
        ))
        .is_ok());
    }

    #[test]
    fn 导出目录必须已存在() {
        use super::validate_export_dir;

        assert!(
            validate_export_dir(Path::new("/nonexistent-work-review-export-dir-a1b2c3")).is_err()
        );
    }

    #[test]
    fn obsidian导出应支持frontmatter与指定标题区块增量更新() {
        let temp_dir =
            std::env::temp_dir().join(format!("work-review-obsidian-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let output_path = temp_dir.join("2026-06-22.md");
        std::fs::write(
            &output_path,
            "---\ntags:\n  - daily\n---\n\n# 2026-06-22\n\n## 我的手写笔记\n保留这段内容。\n",
        )
        .unwrap();

        let meta = ExportFrontmatterMeta {
            date: "2026-06-22".to_string(),
            total_minutes: 320,
            work_minutes: 280,
            focus_score: Some(84),
            top_apps: vec!["VS Code".to_string()],
            git_commit_count: 3,
        };
        export_daily_report_markdown_with_options(
            &temp_dir,
            "2026-06-22",
            "# 工作日报\n\n## 今日总结\n完成功能开发。",
            Some(&meta),
            true,
            Some("## Work Review"),
        )
        .expect("Obsidian 增量导出应成功");

        let rendered = std::fs::read_to_string(&output_path).unwrap();
        assert!(rendered.contains("tags:\n  - daily"));
        assert!(rendered.contains("work_review_focus_score: 84"));
        assert!(rendered.contains("work_review_git_commits: 3"));
        assert!(rendered.contains("## 我的手写笔记\n保留这段内容。"));
        assert!(rendered.contains("## Work Review"));
        assert!(rendered.contains("<!-- work-review:start -->"));
        assert!(rendered.contains("完成功能开发。"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn obsidian增量导出读取失败时应保留原笔记() {
        let temp_dir = std::env::temp_dir().join(format!(
            "work-review-obsidian-unreadable-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let output_path = temp_dir.join("2026-06-22.md");
        let mut original = vec![0xff, 0xfe];
        for unit in "# 每日笔记\n手写内容必须保留。".encode_utf16() {
            original.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&output_path, &original).unwrap();

        for include_frontmatter in [false, true] {
            let result = export_daily_report_markdown_with_options(
                &temp_dir,
                "2026-06-22",
                "# 工作日报\n新生成的正文。",
                None,
                include_frontmatter,
                Some("## Work Review"),
            );
            assert!(matches!(
                result,
                Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData
            ));
            assert_eq!(std::fs::read(&output_path).unwrap(), original);
            assert_eq!(std::fs::read_dir(&temp_dir).unwrap().count(), 1);
        }
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn git提交区块应插入到折叠时间线之前() {
        let report = "# 工作日报\n\n## 一、AI 分析\n内容。\n\n<details>\n<summary>活动时间线（2条记录）</summary>\n</details>";
        let git_md =
            "## Git 提交产出\n\n### Work_Review (1 commits)\n- `abc1234` feat: add focus metrics";
        let merged = insert_git_commits_section(report, git_md);

        let git_pos = merged
            .find("<!-- WR_BLOCK_START:GIT_COMMITS -->")
            .expect("应包含 GIT_COMMITS 区块标记");
        let timeline_pos = merged
            .find("<details>\n<summary>")
            .expect("应保留折叠活动时间线");
        assert!(git_pos < timeline_pos);
        assert!(merged.contains("abc1234"));
    }
}
