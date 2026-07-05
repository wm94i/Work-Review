//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::database::{AppUsage, BrowserUsage, CategoryUsage, DailyStats, DomainUsage, HourlyActivityBucket, HourlyAppBucket, UrlDetail, UrlUsage};
use crate::error::AppError;
use crate::privacy::{apply_excluded_domains_to_stats, apply_ignored_apps_to_stats, matches_ignored_app};
use crate::AppState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{State};

use super::shared::collect_privacy_filters;

fn load_daily_stats_for_overview(state: &AppState, date: &str) -> Result<DailyStats, AppError> {
    let segments = state.config.effective_work_segments();
    let (ignored_apps, excluded_domains) = collect_privacy_filters(state);
    let mut stats = state.database.get_daily_stats_with_segments_filtered(
        date,
        &segments,
        &ignored_apps,
        &excluded_domains,
    )?;

    // 加班时长：数据库已按秒级精度计算了"最后工作时段结束后的活动量"。
    // 仅当未启用工作时段（弹性工时）时，退回到标准工时方案。
    if !state.config.work_time_enabled {
        let standard_seconds = (state.config.standard_work_hours * 3600.0).round() as i64;
        stats.overtime_duration = (stats.work_time_duration - standard_seconds).max(0);
    }

    Ok(stats)
}

fn overview_week_bounds_for_date(anchor: chrono::NaiveDate) -> (String, String) {
    use chrono::Datelike;

    let monday = anchor - chrono::Duration::days(anchor.weekday().num_days_from_monday() as i64);
    (
        monday.format("%Y-%m-%d").to_string(),
        anchor.format("%Y-%m-%d").to_string(),
    )
}

#[derive(Default)]
struct DomainAggregate {
    duration: i64,
    semantic_votes: HashMap<String, i64>,
    urls: HashMap<String, i64>,
}

#[derive(Default)]
struct BrowserAggregate {
    duration: i64,
    executable_path: Option<String>,
    domains: HashMap<String, DomainAggregate>,
}

fn update_preferred_path(target: &mut Option<String>, candidate: Option<String>) {
    if target.is_none() {
        *target = candidate.filter(|value| !value.trim().is_empty());
    }
}

fn record_semantic_vote(
    votes: &mut HashMap<String, i64>,
    semantic_category: Option<String>,
    duration: i64,
) {
    if duration <= 0 {
        return;
    }

    if let Some(category) = semantic_category
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        *votes.entry(category).or_insert(0) += duration;
    }
}

fn resolve_primary_semantic(votes: HashMap<String, i64>) -> Option<String> {
    votes
        .into_iter()
        .max_by(
            |(left_label, left_duration), (right_label, right_duration)| {
                left_duration
                    .cmp(right_duration)
                    .then_with(|| right_label.cmp(left_label))
            },
        )
        .map(|(label, _)| label)
}

fn sort_url_details(items: &mut [UrlDetail]) {
    items.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| right.url.cmp(&left.url))
    });
}

fn sort_domain_usage(items: &mut [DomainUsage]) {
    items.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.domain.cmp(&right.domain))
    });

    for item in items {
        sort_url_details(&mut item.urls);
    }
}

fn build_domain_usage_from_aggregate(domain: String, aggregate: DomainAggregate) -> DomainUsage {
    let mut urls = aggregate
        .urls
        .into_iter()
        .map(|(url, duration)| UrlDetail { url, duration })
        .collect::<Vec<_>>();
    sort_url_details(&mut urls);

    DomainUsage {
        domain,
        duration: aggregate.duration,
        semantic_category: resolve_primary_semantic(aggregate.semantic_votes),
        urls,
    }
}

fn merge_domain_usage_maps(
    target: &mut HashMap<String, DomainAggregate>,
    domains: Vec<DomainUsage>,
) {
    for domain in domains {
        let domain_key = domain.domain.clone();
        let entry = target.entry(domain_key).or_default();
        entry.duration += domain.duration;
        record_semantic_vote(
            &mut entry.semantic_votes,
            domain.semantic_category.clone(),
            domain.duration,
        );

        for url in domain.urls {
            *entry.urls.entry(url.url).or_insert(0) += url.duration;
        }
    }
}

fn sum_daily_stats(days: Vec<DailyStats>) -> DailyStats {
    let mut total_duration = 0;
    let mut screenshot_count = 0;
    let mut browser_duration = 0;
    let mut work_time_duration = 0;
    let mut overtime_duration = 0;

    let mut app_usage_map: HashMap<String, AppUsage> = HashMap::new();
    let mut category_usage_map: HashMap<String, i64> = HashMap::new();
    let mut url_usage_map: HashMap<String, UrlUsage> = HashMap::new();
    let mut domain_usage_map: HashMap<String, DomainAggregate> = HashMap::new();
    let mut browser_usage_map: HashMap<String, BrowserAggregate> = HashMap::new();
    let mut hourly_activity_distribution: Vec<HourlyActivityBucket> = (0..24)
        .map(|hour| HourlyActivityBucket { hour, duration: 0 })
        .collect();

    for day in days {
        total_duration += day.total_duration;
        screenshot_count += day.screenshot_count;
        browser_duration += day.browser_duration;
        work_time_duration += day.work_time_duration;
        overtime_duration += day.overtime_duration;

        for app in day.app_usage {
            let entry = app_usage_map
                .entry(app.app_name.clone())
                .or_insert(AppUsage {
                    app_name: app.app_name.clone(),
                    duration: 0,
                    count: 0,
                    executable_path: None,
                    screenshot_url: None,
                });
            entry.duration += app.duration;
            entry.count += app.count;
            if entry.screenshot_url.is_none() && app.screenshot_url.is_some() {
                entry.screenshot_url = app.screenshot_url;
            }
            update_preferred_path(&mut entry.executable_path, app.executable_path);
        }

        for category in day.category_usage {
            *category_usage_map.entry(category.category).or_insert(0) += category.duration;
        }

        for url in day.url_usage {
            let entry = url_usage_map.entry(url.url.clone()).or_insert(UrlUsage {
                url: url.url.clone(),
                domain: url.domain.clone(),
                duration: 0,
            });
            entry.duration += url.duration;
            if entry.domain.trim().is_empty() {
                entry.domain = url.domain;
            }
        }

        merge_domain_usage_maps(&mut domain_usage_map, day.domain_usage);

        for browser in day.browser_usage {
            let entry = browser_usage_map
                .entry(browser.browser_name.clone())
                .or_default();
            entry.duration += browser.duration;
            update_preferred_path(&mut entry.executable_path, browser.executable_path);
            merge_domain_usage_maps(&mut entry.domains, browser.domains);
        }

        for bucket in day.hourly_activity_distribution {
            if (0..24).contains(&bucket.hour) {
                hourly_activity_distribution[bucket.hour as usize].duration += bucket.duration;
            }
        }
    }

    let mut app_usage = app_usage_map.into_values().collect::<Vec<_>>();
    app_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.app_name.cmp(&right.app_name))
    });

    let mut category_usage = category_usage_map
        .into_iter()
        .map(|(category, duration)| CategoryUsage { category, duration })
        .collect::<Vec<_>>();
    category_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.category.cmp(&right.category))
    });

    let mut url_usage = url_usage_map.into_values().collect::<Vec<_>>();
    url_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| right.url.cmp(&left.url))
    });

    let mut domain_usage = domain_usage_map
        .into_iter()
        .map(|(domain, aggregate)| build_domain_usage_from_aggregate(domain, aggregate))
        .collect::<Vec<_>>();
    sort_domain_usage(&mut domain_usage);

    let mut browser_usage = browser_usage_map
        .into_iter()
        .map(|(browser_name, aggregate)| {
            let mut domains = aggregate
                .domains
                .into_iter()
                .map(|(domain, domain_aggregate)| {
                    build_domain_usage_from_aggregate(domain, domain_aggregate)
                })
                .collect::<Vec<_>>();
            sort_domain_usage(&mut domains);

            BrowserUsage {
                browser_name,
                duration: aggregate.duration,
                executable_path: aggregate.executable_path,
                domains,
            }
        })
        .collect::<Vec<_>>();
    browser_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.browser_name.cmp(&right.browser_name))
    });

    DailyStats {
        total_duration,
        screenshot_count,
        app_usage,
        category_usage,
        browser_duration,
        url_usage,
        domain_usage,
        browser_usage,
        work_time_duration,
        overtime_duration,
        hourly_activity_distribution,
    }
}

fn resolve_overview_anchor_date(date: Option<&str>) -> Result<chrono::NaiveDate, AppError> {
    match date.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|e| AppError::Config(format!("解析概览日期失败: {e}"))),
        None => Ok(chrono::Local::now().date_naive()),
    }
}

fn resolve_overview_date_span(
    date: Option<&str>,
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Result<(chrono::NaiveDate, chrono::NaiveDate), AppError> {
    let fallback = resolve_overview_anchor_date(date)?;
    let start = date_from
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析概览开始日期失败: {e}")))
        })
        .transpose()?
        .unwrap_or(fallback);
    let end = date_to
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析概览结束日期失败: {e}")))
        })
        .transpose()?
        .unwrap_or(start);

    Ok(if start <= end {
        (start, end)
    } else {
        (end, start)
    })
}

/// 获取今日统计 —— 内部复用版（供 Tauri 命令与 localhost API 共用）
pub(crate) fn get_today_stats_inner(state: &Arc<Mutex<AppState>>) -> Result<DailyStats, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let stats = load_daily_stats_for_overview(&state, &today)?;
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
    let stats = apply_ignored_apps_to_stats(stats, &ignored_apps);
    Ok(apply_excluded_domains_to_stats(stats, &excluded_domains))
}

/// 获取今日统计
#[tauri::command]
pub async fn get_today_stats(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<DailyStats, AppError> {
    get_today_stats_inner(state.inner())
}

/// 获取概览统计 —— 内部复用版
pub(crate) fn get_overview_stats_inner(
    mode: String,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<DailyStats, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let normalized_mode = mode.trim().to_lowercase();
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);

    let stats = match normalized_mode.as_str() {
        "date" => {
            let (start, end) = resolve_overview_date_span(
                date.as_deref(),
                date_from.as_deref(),
                date_to.as_deref(),
            )?;

            if start == end {
                let date_value = start.format("%Y-%m-%d").to_string();
                load_daily_stats_for_overview(&state, &date_value)?
            } else {
                let mut daily_stats = Vec::new();
                let mut current = start;
                while current <= end {
                    let current_date = current.format("%Y-%m-%d").to_string();
                    daily_stats.push(load_daily_stats_for_overview(&state, &current_date)?);
                    current = current
                        .succ_opt()
                        .ok_or_else(|| AppError::Config("计算概览日期范围失败".to_string()))?;
                }
                sum_daily_stats(daily_stats)
            }
        }
        "week" => {
            let anchor = resolve_overview_anchor_date(date.as_deref())?;
            let (date_from, date_to) = overview_week_bounds_for_date(anchor);
            let start = chrono::NaiveDate::parse_from_str(&date_from, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析周概览开始日期失败: {e}")))?;
            let end = chrono::NaiveDate::parse_from_str(&date_to, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析周概览结束日期失败: {e}")))?;

            let mut daily_stats = Vec::new();
            let mut current = start;
            while current <= end {
                let current_date = current.format("%Y-%m-%d").to_string();
                daily_stats.push(load_daily_stats_for_overview(&state, &current_date)?);
                current = current
                    .succ_opt()
                    .ok_or_else(|| AppError::Config("计算周概览日期范围失败".to_string()))?;
            }
            sum_daily_stats(daily_stats)
        }
        _ => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            load_daily_stats_for_overview(&state, &today)?
        }
    };

    let stats = apply_ignored_apps_to_stats(stats, &ignored_apps);
    Ok(apply_excluded_domains_to_stats(stats, &excluded_domains))
}

/// 获取概览统计（支持今日 / 指定日期 / 本周）
#[tauri::command]
pub async fn get_overview_stats(
    mode: String,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<DailyStats, AppError> {
    get_overview_stats_inner(mode, date, date_from, date_to, state.inner())
}

/// 获取指定日期的统计 —— 内部复用版
pub(crate) fn get_daily_stats_inner(
    date: &str,
    state: &Arc<Mutex<AppState>>,
) -> Result<DailyStats, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let segments = s.config.effective_work_segments();
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&s);
    s.database.get_daily_stats_with_segments_filtered(
        date,
        &segments,
        &ignored_apps,
        &excluded_domains,
    )
}

/// 获取指定日期的统计
#[tauri::command]
pub async fn get_daily_stats(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<DailyStats, AppError> {
    get_daily_stats_inner(&date, state.inner())
}

pub(crate) fn get_hourly_app_breakdown_inner(
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    mode: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<HourlyAppBucket>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let normalized_mode = mode.unwrap_or_else(|| "date".to_string());
    let (date_from, date_to) = match normalized_mode.trim().to_lowercase().as_str() {
        "week" => {
            let anchor = resolve_overview_anchor_date(date.as_deref())?;
            overview_week_bounds_for_date(anchor)
        }
        "today" => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            (today.clone(), today)
        }
        _ => {
            let (start, end) = resolve_overview_date_span(
                date.as_deref(),
                date_from.as_deref(),
                date_to.as_deref(),
            )?;
            (
                start.format("%Y-%m-%d").to_string(),
                end.format("%Y-%m-%d").to_string(),
            )
        }
    };

    let (ignored_apps, _) = collect_privacy_filters(&state);
    let mut buckets = state
        .database
        .get_hourly_app_breakdown_range(&date_from, &date_to)?;
    if !ignored_apps.is_empty() {
        for bucket in &mut buckets {
            bucket
                .apps
                .retain(|app| !matches_ignored_app(&app.app_name, &ignored_apps));
            bucket.total_duration = bucket.apps.iter().map(|app| app.duration).sum();
        }
    }
    Ok(buckets)
}

/// 获取每小时×应用的时长分布
#[tauri::command]
pub async fn get_hourly_app_breakdown(
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    mode: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<HourlyAppBucket>, AppError> {
    get_hourly_app_breakdown_inner(date, date_from, date_to, mode, state.inner())
}

/// 获取历史应用列表 —— 内部复用版
pub(crate) fn get_recent_apps_inner(state: &Arc<Mutex<AppState>>) -> Result<Vec<String>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.get_recent_apps(50)
}

/// 历史应用详情 —— 内部复用版，供 localhost API 返回截图 URL
pub(crate) fn get_recent_app_usage_inner(
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<AppUsage>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.get_recent_app_usage(50)
}

/// 获取历史应用列表（从数据库）
#[tauri::command]
pub async fn get_recent_apps(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<String>, AppError> {
    get_recent_apps_inner(state.inner())
}

/// 获取当前运行的应用列表
#[tauri::command]
pub async fn get_running_apps() -> Result<Vec<String>, AppError> {
    get_running_apps_impl()
}

/// macOS 实现
#[cfg(target_os = "macos")]
fn get_running_apps_impl() -> Result<Vec<String>, AppError> {
    use std::process::Command;

    // 使用 AppleScript 获取运行中的应用
    let output = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to get name of every process whose background only is false"#
        ])
        .output()
        .map_err(|e| AppError::Unknown(format!("执行 AppleScript 失败: {e}")))?;

    if output.status.success() {
        let apps_str = String::from_utf8_lossy(&output.stdout);
        let mut apps: Vec<String> = apps_str
            .split(", ")
            .map(crate::monitor::normalize_display_app_name)
            .filter(|s| !s.is_empty())
            .collect();
        apps.sort();
        apps.dedup();
        Ok(apps)
    } else {
        Err(AppError::Unknown("获取应用列表失败".to_string()))
    }
}

/// Windows 实现
#[cfg(target_os = "windows")]
fn get_running_apps_impl() -> Result<Vec<String>, AppError> {
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut apps = HashSet::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() {
            return Ok(vec![]);
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                // 获取进程名
                let name_len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = OsString::from_wide(&entry.szExeFile[..name_len])
                    .to_string_lossy()
                    .to_string();

                // 排除系统进程
                let name_lower = name.to_lowercase();
                if !name_lower.ends_with(".exe") {
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                    continue;
                }

                // 排除常见系统进程
                let excluded = [
                    "svchost.exe",
                    "csrss.exe",
                    "wininit.exe",
                    "services.exe",
                    "lsass.exe",
                    "smss.exe",
                    "winlogon.exe",
                    "dwm.exe",
                    "fontdrvhost.exe",
                    "sihost.exe",
                    "taskhostw.exe",
                    "runtimebroker.exe",
                    "searchhost.exe",
                    "startmenuexperiencehost.exe",
                    "textinputhost.exe",
                    "ctfmon.exe",
                    "conhost.exe",
                ];

                if !excluded.contains(&name_lower.as_str()) {
                    // 移除 .exe 后缀
                    let display_name = crate::monitor::normalize_display_app_name(&name);
                    apps.insert(display_name);
                }

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
    }

    let mut result: Vec<String> = apps.into_iter().collect();
    result.sort();
    Ok(result)
}

/// 其他平台
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn get_running_apps_impl() -> Result<Vec<String>, AppError> {
    Ok(vec![])
}

/// 获取存储统计信息 —— 内部复用版
pub(crate) fn get_storage_stats_inner(
    state: &Arc<Mutex<AppState>>,
) -> Result<serde_json::Value, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let stats = s
        .storage_manager
        .get_stats()
        .map_err(|e| AppError::Unknown(e.to_string()))?;

    Ok(serde_json::json!({
        "total_files": stats.total_files,
        "total_size_mb": format!("{:.1}", stats.total_size_mb),
        "storage_limit_mb": stats.storage_limit_mb,
        "retention_days": stats.retention_days,
    }))
}

/// 获取存储统计信息
#[tauri::command]
pub async fn get_storage_stats(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    get_storage_stats_inner(state.inner())
}

/// 获取指定日期的小时摘要 —— 内部复用版
pub(crate) fn get_hourly_summaries_inner(
    date: &str,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<serde_json::Value>, AppError> {
    let app_state = state.clone();

    for hour in 0..24 {
        crate::generate_and_save_summary(&app_state, date, hour);
    }

    let s = app_state
        .lock()
        .map_err(|e| AppError::Unknown(e.to_string()))?;
    let summaries = s.database.get_hourly_summaries(date)?;

    Ok(summaries
        .iter()
        .map(|s| {
            serde_json::json!({
                "hour": s.hour,
                "summary": s.summary,
                "main_apps": s.main_apps,
                "activity_count": s.activity_count,
                "total_duration": s.total_duration,
            })
        })
        .collect())
}

/// 获取指定日期的小时摘要
#[tauri::command]
pub async fn get_hourly_summaries(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<serde_json::Value>, AppError> {
    get_hourly_summaries_inner(&date, state.inner())
}

/// 清理今天之前的所有活动记录
#[tauri::command]
pub async fn clear_old_activities(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let data_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.data_dir.clone()
    };

    // 获取要保留的日期（今天和昨天）
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let yesterday = (now - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    let mut deleted_screenshots = 0;

    // 删除旧截图目录（保留今天和昨天）
    let screenshots_dir = data_dir.join("screenshots");
    if screenshots_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&screenshots_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    // 保留今天和昨天的目录
                    if name != today && name != yesterday && entry.path().is_dir() {
                        if let Ok(dir_entries) = std::fs::read_dir(entry.path()) {
                            for file_entry in dir_entries.flatten() {
                                if file_entry.path().is_file() {
                                    deleted_screenshots += 1;
                                }
                            }
                        }
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
    }

    // 同步清理数据库中对应的旧记录
    {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        if let Err(e) = state.database.delete_activities_before_date(&today) {
            log::warn!("清理旧活动记录失败: {e}");
        }
    }

    Ok(serde_json::json!({
        "deleted_screenshots": deleted_screenshots,
        "kept_dates": [today, yesterday],
        "message": format!("已清理 {} 张旧截图和对应活动记录，保留今天和昨天的数据", deleted_screenshots)
    }))
}

/// 按相对 data_dir 的路径删除截图文件，返回成功删除的文件数（单文件失败只 log，不中断）
fn remove_screenshot_files(data_dir: &std::path::Path, paths: Vec<String>) -> usize {
    let mut removed = 0usize;
    for p in paths {
        if p.is_empty() {
            continue;
        }
        let path = data_dir.join(&p);
        if path.exists() {
            match std::fs::remove_file(&path) {
                Ok(_) => removed += 1,
                Err(e) => log::warn!("删除截图文件失败 {}: {e}", path.display()),
            }
        }
    }
    removed
}

/// 删除单条活动记录（连带删除对应截图文件，OCR 文本随行删除）
#[tauri::command]
pub async fn delete_activity(
    id: i64,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let (data_dir, paths, deleted) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (deleted, paths) = s.database.delete_activity_by_id(id)?;
        (s.data_dir.clone(), paths, deleted)
    };
    let removed = remove_screenshot_files(&data_dir, paths);
    log::info!("删除单条活动 id={id}: {deleted} 条记录, {removed} 张截图");
    Ok(serde_json::json!({ "deleted": deleted, "removed_screenshots": removed }))
}

/// 删除指定日期（本地时区全天）的全部活动记录（连带截图）
#[tauri::command]
pub async fn delete_activities_by_date(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let (data_dir, paths, deleted) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (deleted, paths) = s.database.delete_activities_by_date(&date)?;
        (s.data_dir.clone(), paths, deleted)
    };
    let removed = remove_screenshot_files(&data_dir, paths);
    log::info!("按日期删除 {date}: {deleted} 条记录, {removed} 张截图");
    Ok(serde_json::json!({ "deleted": deleted, "removed_screenshots": removed }))
}

/// 删除指定时间段 [start_ts, end_ts)（Unix 秒）内的全部活动记录（连带截图）
#[tauri::command]
pub async fn delete_activities_by_range(
    start_ts: i64,
    end_ts: i64,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let (data_dir, paths, deleted) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (deleted, paths) = s.database.delete_activities_by_range(start_ts, end_ts)?;
        (s.data_dir.clone(), paths, deleted)
    };
    let removed = remove_screenshot_files(&data_dir, paths);
    log::info!("按时间段删除 [{start_ts}, {end_ts}): {deleted} 条记录, {removed} 张截图");
    Ok(serde_json::json!({ "deleted": deleted, "removed_screenshots": removed }))
}

/// 删除指定应用的活动记录；start_ts/end_ts 同时给定时只删该时间段内的，否则删该应用全部（连带截图）
#[tauri::command]
pub async fn delete_activities_by_app(
    app_name: String,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let date_range = match (start_ts, end_ts) {
        (Some(s), Some(e)) => Some((s, e)),
        _ => None,
    };
    let (data_dir, paths, deleted) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (deleted, paths) = s.database.delete_activities_by_app(&app_name, date_range)?;
        (s.data_dir.clone(), paths, deleted)
    };
    let removed = remove_screenshot_files(&data_dir, paths);
    log::info!("按应用删除 {app_name}: {deleted} 条记录, {removed} 张截图");
    Ok(serde_json::json!({ "deleted": deleted, "removed_screenshots": removed }))
}

/// 检查是否在工作时间内
#[tauri::command]
pub async fn is_work_time(state: State<'_, Arc<Mutex<AppState>>>) -> Result<bool, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let segments = state.config.effective_work_segments();
    Ok(crate::screen_lock::ScreenLockMonitor::is_work_time_in_segments(&segments))
}



#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use crate::database::HourlyActivityBucket;

    #[test]
    fn 概览本周范围应从周一开始到锚点日期结束() {
        let anchor = NaiveDate::from_ymd_opt(2026, 4, 1).expect("valid anchor date");

        let (date_from, date_to) = overview_week_bounds_for_date(anchor);

        assert_eq!(date_from, "2026-03-30");
        assert_eq!(date_to, "2026-04-01");
    }

    #[test]
    fn 周概览统计应合并重复应用浏览器与小时分布() {
        let hourly = |hour, duration| HourlyActivityBucket { hour, duration };
        let day_one = DailyStats {
            total_duration: 120,
            screenshot_count: 2,
            app_usage: vec![AppUsage {
                app_name: "Cursor".to_string(),
                duration: 120,
                count: 2,
                executable_path: Some("/Applications/Cursor.app".to_string()),
                screenshot_url: Some("https://cdn.example.com/cursor-day-one.jpg".to_string()),
            }],
            category_usage: vec![CategoryUsage {
                category: "development".to_string(),
                duration: 120,
            }],
            browser_duration: 60,
            url_usage: vec![UrlUsage {
                url: "https://docs.example.com/a".to_string(),
                domain: "docs.example.com".to_string(),
                duration: 60,
            }],
            domain_usage: vec![DomainUsage {
                domain: "docs.example.com".to_string(),
                duration: 60,
                semantic_category: Some("资料阅读".to_string()),
                urls: vec![UrlDetail {
                    url: "https://docs.example.com/a".to_string(),
                    duration: 60,
                }],
            }],
            browser_usage: vec![BrowserUsage {
                browser_name: "Google Chrome".to_string(),
                duration: 60,
                executable_path: Some("/Applications/Google Chrome.app".to_string()),
                domains: vec![DomainUsage {
                    domain: "docs.example.com".to_string(),
                    duration: 60,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://docs.example.com/a".to_string(),
                        duration: 60,
                    }],
                }],
            }],
            work_time_duration: 100,
            overtime_duration: 0,
            hourly_activity_distribution: vec![hourly(9, 60), hourly(10, 60)],
        };
        let day_two = DailyStats {
            total_duration: 180,
            screenshot_count: 3,
            app_usage: vec![
                AppUsage {
                    app_name: "Cursor".to_string(),
                    duration: 120,
                    count: 1,
                    executable_path: Some("/Applications/Cursor.app".to_string()),
                    screenshot_url: None,
                },
                AppUsage {
                    app_name: "Google Chrome".to_string(),
                    duration: 60,
                    count: 1,
                    executable_path: Some("/Applications/Google Chrome.app".to_string()),
                    screenshot_url: Some("https://cdn.example.com/chrome-day-two.jpg".to_string()),
                },
            ],
            category_usage: vec![
                CategoryUsage {
                    category: "development".to_string(),
                    duration: 120,
                },
                CategoryUsage {
                    category: "browser".to_string(),
                    duration: 60,
                },
            ],
            browser_duration: 120,
            url_usage: vec![
                UrlUsage {
                    url: "https://docs.example.com/a".to_string(),
                    domain: "docs.example.com".to_string(),
                    duration: 30,
                },
                UrlUsage {
                    url: "https://news.example.com/b".to_string(),
                    domain: "news.example.com".to_string(),
                    duration: 90,
                },
            ],
            domain_usage: vec![
                DomainUsage {
                    domain: "docs.example.com".to_string(),
                    duration: 30,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://docs.example.com/a".to_string(),
                        duration: 30,
                    }],
                },
                DomainUsage {
                    domain: "news.example.com".to_string(),
                    duration: 90,
                    semantic_category: Some("资料调研".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://news.example.com/b".to_string(),
                        duration: 90,
                    }],
                },
            ],
            browser_usage: vec![BrowserUsage {
                browser_name: "Google Chrome".to_string(),
                duration: 120,
                executable_path: Some("/Applications/Google Chrome.app".to_string()),
                domains: vec![
                    DomainUsage {
                        domain: "docs.example.com".to_string(),
                        duration: 30,
                        semantic_category: Some("资料阅读".to_string()),
                        urls: vec![UrlDetail {
                            url: "https://docs.example.com/a".to_string(),
                            duration: 30,
                        }],
                    },
                    DomainUsage {
                        domain: "news.example.com".to_string(),
                        duration: 90,
                        semantic_category: Some("资料调研".to_string()),
                        urls: vec![UrlDetail {
                            url: "https://news.example.com/b".to_string(),
                            duration: 90,
                        }],
                    },
                ],
            }],
            work_time_duration: 160,
            overtime_duration: 0,
            hourly_activity_distribution: vec![hourly(9, 30), hourly(10, 90), hourly(11, 60)],
        };

        let merged = sum_daily_stats(vec![day_one, day_two]);

        assert_eq!(merged.total_duration, 300);
        assert_eq!(merged.screenshot_count, 5);
        assert_eq!(merged.work_time_duration, 260);
        assert_eq!(merged.browser_duration, 180);
        assert_eq!(merged.app_usage.len(), 2);
        assert_eq!(merged.app_usage[0].app_name, "Cursor");
        assert_eq!(merged.app_usage[0].duration, 240);
        assert_eq!(merged.app_usage[0].count, 3);
        assert_eq!(merged.hourly_activity_distribution[9].duration, 90);
        assert_eq!(merged.hourly_activity_distribution[10].duration, 150);
        assert_eq!(merged.hourly_activity_distribution[11].duration, 60);
        assert_eq!(merged.browser_usage.len(), 1);
        assert_eq!(merged.browser_usage[0].duration, 180);
        assert_eq!(merged.browser_usage[0].domains.len(), 2);
        assert_eq!(merged.domain_usage[0].domain, "docs.example.com");
        assert_eq!(merged.domain_usage[0].duration, 90);
        assert_eq!(merged.url_usage[0].url, "https://news.example.com/b");
        assert_eq!(merged.url_usage[0].duration, 90);
    }

}
