use crate::AppState;
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::State;
use work_review_core::analysis::AppLocale;
use work_review_core::database::{Activity, Database};
use work_review_core::error::AppError;
use work_review_core::privacy::{sanitize_activity_for_external, PrivacyAction, PrivacyFilter};
use work_review_core::work_intelligence::{
    generate_period_review as build_period_review, ReviewGranularity,
};

use super::backup::write_export_file_atomic_with;
use super::config::path_for_display;
use super::report::validate_export_dir;
use super::shared::{collect_privacy_filters, filter_activities_by_privacy};
use std::io::Write;

const PERIOD_ACTIVITY_PAGE_SIZE: usize = 5_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodReviewPayload {
    pub period_type: String,
    pub period_key: String,
    pub date_from: String,
    pub date_to: String,
    pub locale: String,
    pub content: String,
    pub created_at: i64,
    pub cached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPeriodBounds {
    pub granularity: ReviewGranularity,
    pub period_type: &'static str,
    pub period_key: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub prev_start: NaiveDate,
    pub prev_end: NaiveDate,
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_first = NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    Some((next_first - first).num_days() as u32)
}

/// 严格校验并解析周期类型（`week` / `month`）与周期键（`YYYY-Www` / `YYYY-MM`），
/// 同时计算当前周期与上一对比周期的精确起止日期（含闰年 2 月、大小月及跨年 12→1 月 / ISO W01 回绕）。
pub(crate) fn parse_period_bounds(
    period_type: &str,
    period_key: &str,
) -> Result<ParsedPeriodBounds, AppError> {
    let kind = period_type.trim().to_ascii_lowercase();
    let key = period_key.trim();

    match kind.as_str() {
        "month" => {
            if key.len() != 7 || key.as_bytes()[4] != b'-' {
                return Err(AppError::Config(format!(
                    "月报周期格式无效（期望 YYYY-MM）: {key}"
                )));
            }
            let year: i32 = key[0..4]
                .parse()
                .map_err(|_| AppError::Config(format!("月报年份无效: {key}")))?;
            let month: u32 = key[5..7]
                .parse()
                .map_err(|_| AppError::Config(format!("月报月份无效: {key}")))?;
            if !(1970..=2100).contains(&year) || !(1..=12).contains(&month) {
                return Err(AppError::Config(format!("月报日期超出有效范围: {key}")));
            }
            let last_day = days_in_month(year, month)
                .ok_or_else(|| AppError::Config(format!("无法计算月末日期: {key}")))?;
            let start = NaiveDate::from_ymd_opt(year, month, 1)
                .ok_or_else(|| AppError::Config(format!("无效月初日期: {key}")))?;
            let end = NaiveDate::from_ymd_opt(year, month, last_day)
                .ok_or_else(|| AppError::Config(format!("无效月末日期: {key}")))?;

            let (prev_year, prev_month) = if month == 1 {
                (year - 1, 12)
            } else {
                (year, month - 1)
            };
            let prev_last_day = days_in_month(prev_year, prev_month)
                .ok_or_else(|| AppError::Config("无法计算上月末日期".to_string()))?;
            let prev_start = NaiveDate::from_ymd_opt(prev_year, prev_month, 1)
                .ok_or_else(|| AppError::Config("无法计算上月初日期".to_string()))?;
            let prev_end = NaiveDate::from_ymd_opt(prev_year, prev_month, prev_last_day)
                .ok_or_else(|| AppError::Config("无法计算上月末日期".to_string()))?;

            Ok(ParsedPeriodBounds {
                granularity: ReviewGranularity::Month,
                period_type: "month",
                period_key: format!("{year:04}-{month:02}"),
                start,
                end,
                prev_start,
                prev_end,
            })
        }
        "week" => {
            let upper = key.to_ascii_uppercase();
            if upper.len() != 8 || upper.as_bytes()[4] != b'-' || upper.as_bytes()[5] != b'W' {
                return Err(AppError::Config(format!(
                    "周报周期格式无效（期望 YYYY-Www）: {key}"
                )));
            }
            let year: i32 = upper[0..4]
                .parse()
                .map_err(|_| AppError::Config(format!("周报年份无效: {key}")))?;
            let week: u32 = upper[6..8]
                .parse()
                .map_err(|_| AppError::Config(format!("周报周数无效: {key}")))?;
            if !(1970..=2100).contains(&year) || !(1..=53).contains(&week) {
                return Err(AppError::Config(format!("周报周期超出有效范围: {key}")));
            }
            let start = NaiveDate::from_isoywd_opt(year, week, Weekday::Mon).ok_or_else(|| {
                AppError::Config(format!("无效 ISO 周（该年不存在第 {week} 周）: {key}"))
            })?;
            let end = start + Duration::days(6);
            let prev_start = start - Duration::days(7);
            let prev_end = start - Duration::days(1);
            let iso = start.iso_week();
            Ok(ParsedPeriodBounds {
                granularity: ReviewGranularity::Week,
                period_type: "week",
                period_key: format!("{:04}-W{:02}", iso.year(), iso.week()),
                start,
                end,
                prev_start,
                prev_end,
            })
        }
        _ => Err(AppError::Config(format!(
            "不支持的复盘周期类型: {period_type}（仅支持 week 或 month）"
        ))),
    }
}

fn load_sanitized_activities_for_range(
    database: &Database,
    start: NaiveDate,
    end: NaiveDate,
    ignored_apps: &[String],
    excluded_domains: &[String],
    filter: &PrivacyFilter,
) -> Result<Vec<Activity>, AppError> {
    let from_str = start.format("%Y-%m-%d").to_string();
    let to_str = end.format("%Y-%m-%d").to_string();
    let mut out = Vec::new();
    let mut after_id: i64 = 0;

    loop {
        let page = database.get_activities_in_date_range_after_id(
            &from_str,
            &to_str,
            after_id,
            PERIOD_ACTIVITY_PAGE_SIZE,
        )?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        let last_id = page.last().and_then(|a| a.id);
        let filtered = filter_activities_by_privacy(page, ignored_apps, excluded_domains);
        for mut act in filtered {
            if filter.check_privacy_full(
                &act.app_name,
                &act.window_title,
                act.browser_url.as_deref(),
            ) == PrivacyAction::Skip
            {
                continue;
            }
            sanitize_activity_for_external(filter, &mut act, false);
            out.push(act);
        }
        if page_len < PERIOD_ACTIVITY_PAGE_SIZE {
            break;
        }
        match last_id {
            Some(id) if id > after_id => after_id = id,
            _ => break,
        }
    }
    Ok(out)
}

/// 读取或生成周报 / 月报周期复盘（供 Tauri 命令与本地 API `GET /v1/period-review` 复用）。
pub(crate) fn get_or_generate_period_review_inner(
    state: &Arc<Mutex<AppState>>,
    period_type: &str,
    period_key: &str,
    locale: Option<&str>,
    force: bool,
) -> Result<PeriodReviewPayload, AppError> {
    let bounds = parse_period_bounds(period_type, period_key)?;
    let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&guard);
    let database = guard.database.clone();
    let privacy_config = guard.config.privacy.clone();
    let projects = guard.config.projects.clone();
    let app_locale = AppLocale::from_code(
        locale
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&guard.config.locale),
    );
    let locale_code = app_locale.as_code().to_string();
    let date_from = bounds.start.format("%Y-%m-%d").to_string();
    let date_to = bounds.end.format("%Y-%m-%d").to_string();

    let today = chrono::Local::now().date_naive();
    let period_finished = bounds.end < today;

    // 进行中的周期（包含今天或未来）不复用历史缓存，确保新增/合并活动即时反映在复盘中
    if !force && period_finished {
        if let Some(saved) =
            database.get_period_review(bounds.period_type, &bounds.period_key, &locale_code)?
        {
            return Ok(PeriodReviewPayload {
                period_type: saved.period_type,
                period_key: saved.period_key,
                date_from,
                date_to,
                locale: saved.locale,
                content: saved.content,
                created_at: saved.created_at,
                cached: true,
            });
        }
    }

    drop(guard);
    let filter = PrivacyFilter::from_config(&privacy_config);
    let activities = load_sanitized_activities_for_range(
        &database,
        bounds.start,
        bounds.end,
        &ignored_apps,
        &excluded_domains,
        &filter,
    )?;
    let prev_activities = load_sanitized_activities_for_range(
        &database,
        bounds.prev_start,
        bounds.prev_end,
        &ignored_apps,
        &excluded_domains,
        &filter,
    )?;

    let generated = build_period_review(
        &activities,
        Some(&prev_activities),
        &date_from,
        &date_to,
        bounds.granularity,
        &bounds.period_key,
        app_locale,
        &projects,
    );
    // 与配置保存共用状态锁，防止旧规则生成的结果在缓存清空后重新写回。
    let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    if super::semantic_memory::privacy_fingerprint(&guard.config.privacy)
        != super::semantic_memory::privacy_fingerprint(&privacy_config)
    {
        return Err(AppError::Privacy(
            "隐私规则已变更，请重新生成周期复盘".to_string(),
        ));
    }
    let saved = database.save_period_review(
        bounds.period_type,
        &bounds.period_key,
        &locale_code,
        &generated.markdown,
    )?;

    Ok(PeriodReviewPayload {
        period_type: saved.period_type,
        period_key: saved.period_key,
        date_from,
        date_to,
        locale: saved.locale,
        content: saved.content,
        created_at: saved.created_at,
        cached: false,
    })
}

/// 获取已保存的周报 / 月报复盘（不自动生成）。
#[tauri::command]
pub async fn get_saved_period_review(
    period_type: String,
    period_key: String,
    locale: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<PeriodReviewPayload>, AppError> {
    let bounds = parse_period_bounds(&period_type, &period_key)?;
    let state_arc = state.inner().clone();
    let date_from = bounds.start.format("%Y-%m-%d").to_string();
    let date_to = bounds.end.format("%Y-%m-%d").to_string();

    tokio::task::spawn_blocking(move || {
        let guard = state_arc
            .lock()
            .map_err(|e| AppError::Unknown(e.to_string()))?;
        let app_locale = AppLocale::from_code(
            locale
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&guard.config.locale),
        );
        let saved = guard.database.get_period_review(
            bounds.period_type,
            &bounds.period_key,
            app_locale.as_code(),
        )?;
        Ok(saved.map(|record| PeriodReviewPayload {
            period_type: record.period_type,
            period_key: record.period_key,
            date_from,
            date_to,
            locale: record.locale,
            content: record.content,
            created_at: record.created_at,
            cached: true,
        }))
    })
    .await
    .map_err(|e| AppError::Unknown(format!("读取周期复盘任务异常: {e}")))?
}

/// 生成并保存周报 / 月报周期复盘（`force = true` 时强制重算覆盖）。
#[tauri::command]
pub async fn generate_period_review(
    period_type: String,
    period_key: String,
    locale: Option<String>,
    force: Option<bool>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<PeriodReviewPayload, AppError> {
    let state_arc = state.inner().clone();
    let force_regen = force.unwrap_or(false);
    tokio::task::spawn_blocking(move || {
        get_or_generate_period_review_inner(
            &state_arc,
            &period_type,
            &period_key,
            locale.as_deref(),
            force_regen,
        )
    })
    .await
    .map_err(|e| AppError::Unknown(format!("生成周期复盘任务异常: {e}")))?
}

/// 导出周期复盘 Markdown 到已校验目录（使用独占临时文件原子替换）。
#[tauri::command]
pub async fn export_period_review_markdown(
    period_type: String,
    period_key: String,
    content: String,
    export_dir: String,
    locale: Option<String>,
) -> Result<String, AppError> {
    let bounds = parse_period_bounds(&period_type, &period_key)?;
    if content.trim().is_empty() {
        return Err(AppError::Config("导出内容不能为空".to_string()));
    }
    let validated_dir = validate_export_dir(Path::new(export_dir.trim()))?;
    let locale_suffix = locale
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|raw| format!("-{}", AppLocale::from_code(raw).as_code()))
        .unwrap_or_default();
    let file_name = format!(
        "work-review-{}-{}{}.md",
        bounds.period_type, bounds.period_key, locale_suffix
    );
    tokio::task::spawn_blocking(move || {
        let (path, ()) = write_export_file_atomic_with(&validated_dir, &file_name, |writer| {
            writer.write_all(content.as_bytes())?;
            Ok(())
        })?;
        Ok(path_for_display(&path))
    })
    .await
    .map_err(|e| AppError::Unknown(format!("导出周期复盘任务异常: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 周期解析应精确处理闰年大小月跨年回绕与非法iso周() {
        // 闰年 2024-02（29 天）与平年 2026-02（28 天）
        let leap = parse_period_bounds("month", "2024-02").unwrap();
        assert_eq!(leap.start.to_string(), "2024-02-01");
        assert_eq!(leap.end.to_string(), "2024-02-29");
        assert_eq!(leap.prev_start.to_string(), "2024-01-01");
        assert_eq!(leap.prev_end.to_string(), "2024-01-31");

        let non_leap = parse_period_bounds("month", "2026-02").unwrap();
        assert_eq!(non_leap.end.to_string(), "2026-02-28");

        // 1 月向去年 12 月回绕，30 天/31 天月份准确
        let jan = parse_period_bounds("month", "2026-01").unwrap();
        assert_eq!(jan.prev_start.to_string(), "2025-12-01");
        assert_eq!(jan.prev_end.to_string(), "2025-12-31");

        let apr = parse_period_bounds("month", "2026-04").unwrap();
        assert_eq!(apr.end.to_string(), "2026-04-30");

        // ISO 周：2026-W01 的上一周是 2025-W52；2026 有 53 周而 2025 无第 53 周
        let w1 = parse_period_bounds("week", "2026-W01").unwrap();
        assert_eq!(w1.start.to_string(), "2025-12-29");
        assert_eq!(w1.end.to_string(), "2026-01-04");
        assert_eq!(w1.prev_start.to_string(), "2025-12-22");
        assert_eq!(w1.prev_end.to_string(), "2025-12-28");
        assert!(parse_period_bounds("week", "2026-W53").is_ok());
        assert!(parse_period_bounds("week", "2025-W53").is_err());

        // 非法格式 / 越界拒绝
        assert!(parse_period_bounds("month", "2026-13").is_err());
        assert!(parse_period_bounds("month", "../2026-01").is_err());
        assert!(parse_period_bounds("day", "2026-01-01").is_err());
    }

    fn period_test_state() -> (std::path::PathBuf, Arc<Mutex<AppState>>) {
        let dir =
            std::env::temp_dir().join(format!("work-review-period-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::new(&dir.join("test.db")).unwrap();
        let config = work_review_core::config::AppConfig::default();
        let state = Arc::new(Mutex::new(AppState {
            config: config.clone(),
            config_load_status: work_review_core::config::ConfigLoadStatus::Loaded,
            database: db,
            privacy_filter: PrivacyFilter::from_config(&config.privacy),
            screenshot_service: crate::screenshot::ScreenshotService::new(&dir, &config.storage),
            storage_manager: crate::storage::StorageManager::new(&dir, config.storage.clone()),
            data_dir: dir.clone(),
            config_path: dir.join("config.json"),
            is_recording: false,
            is_paused: false,
            avatar_state: crate::avatar_engine::default_avatar_state(),
            avatar_generating_report: false,
            generating_report: false,
            localhost_api_runtime: crate::localhost_api::LocalhostApiRuntime::default(),
            telegram_bot_runtime: crate::telegram_bot::TelegramBotRuntime::default(),
            wecom_bot_runtime: crate::wecom_aibot::WecomBotRuntime::default(),
            cached_active_window: None,
        }));
        (dir, state)
    }

    #[test]
    fn 历史已结束周期应命中缓存而进行中周期应始终实时重算() {
        let (dir, state) = period_test_state();

        // 1. 历史已结束月份（2024-02）首次生成后落库，二次 force=false 命中缓存，force=true 强制重算
        let first_past =
            get_or_generate_period_review_inner(&state, "month", "2024-02", Some("zh-CN"), false)
                .unwrap();
        assert!(!first_past.cached);
        let second_past =
            get_or_generate_period_review_inner(&state, "month", "2024-02", Some("zh-CN"), false)
                .unwrap();
        assert!(second_past.cached);
        let forced_past =
            get_or_generate_period_review_inner(&state, "month", "2024-02", Some("zh-CN"), true)
                .unwrap();
        assert!(!forced_past.cached);

        // 2. 当前进行中的月份始终实时计算，不返回过期缓存快照
        let current_month = chrono::Local::now().format("%Y-%m").to_string();
        let first_cur = get_or_generate_period_review_inner(
            &state,
            "month",
            &current_month,
            Some("zh-CN"),
            false,
        )
        .unwrap();
        assert!(!first_cur.cached);
        let second_cur = get_or_generate_period_review_inner(
            &state,
            "month",
            &current_month,
            Some("zh-CN"),
            false,
        )
        .unwrap();
        assert!(!second_cur.cached);

        drop(state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 隐私规则变更后周月报应清空旧缓存并按新规则生成() {
        use chrono::TimeZone;

        let (dir, state) = period_test_state();
        let database = state.lock().unwrap().database.clone();
        let activity_id = database
            .insert_activity(&Activity {
                id: None,
                timestamp: chrono::Local
                    .with_ymd_and_hms(2024, 2, 10, 12, 0, 0)
                    .earliest()
                    .unwrap()
                    .timestamp(),
                app_name: "SensitiveReviewApp".to_string(),
                window_title: "Private work".to_string(),
                screenshot_path: String::new(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 3600,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            })
            .unwrap();
        for (kind, key) in [("week", "2024-W06"), ("month", "2024-02")] {
            for locale in ["zh-CN", "en"] {
                let first =
                    get_or_generate_period_review_inner(&state, kind, key, Some(locale), false)
                        .unwrap();
                assert!(!first.cached);
                assert!(first.content.contains("SensitiveReviewApp"));
                let cached =
                    get_or_generate_period_review_inner(&state, kind, key, Some(locale), false)
                        .unwrap();
                assert!(cached.cached);
            }
        }

        {
            let mut guard = state.lock().unwrap();
            guard.database.clear_privacy_caches().unwrap();
            super::super::config::apply_app_privacy_level(
                &mut guard.config,
                "SensitiveReviewApp",
                "ignored",
            )
            .unwrap();
        }
        for (kind, key) in [("week", "2024-W06"), ("month", "2024-02")] {
            for locale in ["zh-CN", "en"] {
                assert!(database
                    .get_period_review(kind, key, locale)
                    .unwrap()
                    .is_none());
                let generated =
                    get_or_generate_period_review_inner(&state, kind, key, Some(locale), false)
                        .unwrap();
                assert!(!generated.cached);
                assert!(!generated.content.contains("SensitiveReviewApp"));
                let cached =
                    get_or_generate_period_review_inner(&state, kind, key, Some(locale), false)
                        .unwrap();
                assert!(cached.cached);
                assert!(!cached.content.contains("SensitiveReviewApp"));
            }
        }
        assert!(database.get_activity_by_id(activity_id).unwrap().is_some());
        drop(state);
        drop(database);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
