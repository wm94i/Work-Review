//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::AppState;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use work_review_core::config::{AppConfig, AvatarFollowupItem, PrivacyConfig};
use work_review_core::database::Activity;
use work_review_core::error::AppError;
use work_review_core::work_intelligence::TodoExtractionResult;

/// 从问题中提取时间范围关键词，返回 (date_from, date_to)
///
/// 支持同时匹配多个时间段关键词，合并为最宽的日期范围。
/// 例如"这个月和上个月" → (上月1号, 今天)。
pub fn parse_temporal_range(question: &str) -> (Option<String>, Option<String>) {
    use chrono::{Datelike, Local};

    let normalized = question.trim().to_lowercase();
    let today = Local::now().date_naive();
    let fmt = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();

    // 收集所有匹配的时间段，最后合并
    let mut ranges: Vec<(chrono::NaiveDate, chrono::NaiveDate)> = Vec::new();

    // 今天/今日
    if normalized.contains("今天") || normalized.contains("今日") {
        ranges.push((today, today));
    }

    // 昨天/昨日
    if normalized.contains("昨天") || normalized.contains("昨日") {
        let d = today - chrono::Duration::days(1);
        ranges.push((d, d));
    }

    // 前天
    if normalized.contains("前天") {
        let d = today - chrono::Duration::days(2);
        ranges.push((d, d));
    }

    // 最近N天/近N天/过去N天 — 用 regex 提取所有数字
    if let Ok(re) = regex::Regex::new(r"(?:最近|近|过去)\s*(\d+)\s*天") {
        for caps in re.captures_iter(&normalized) {
            if let Ok(n) = caps[1].parse::<i64>() {
                ranges.push((today - chrono::Duration::days(n), today));
            }
        }
    }

    // 含"最近"但无数字 → 默认 7 天（避免与"最近N天"重复：只有当无数字匹配时才追加）
    if normalized.contains("最近") {
        let has_numeric = regex::Regex::new(r"最近\s*\d+\s*天")
            .ok()
            .map(|re| re.is_match(&normalized))
            .unwrap_or(false);
        if !has_numeric {
            ranges.push((today - chrono::Duration::days(7), today));
        }
    }

    // 本周/这周
    if normalized.contains("本周") || normalized.contains("这周") {
        let wd = today.weekday().num_days_from_monday() as i64;
        let monday = today - chrono::Duration::days(wd);
        ranges.push((monday, today));
    }

    // 上周/上一周
    if normalized.contains("上周") || normalized.contains("上一周") {
        let wd = today.weekday().num_days_from_monday() as i64;
        let this_monday = today - chrono::Duration::days(wd);
        let last_monday = this_monday - chrono::Duration::days(7);
        let last_sunday = this_monday - chrono::Duration::days(1);
        ranges.push((last_monday, last_sunday));
    }

    // 本月/这个月
    if normalized.contains("本月") || normalized.contains("这个月") {
        let first = today.with_day(1).unwrap_or(today);
        ranges.push((first, today));
    }

    // 上月/上个月
    if normalized.contains("上月") || normalized.contains("上个月") {
        let first_this = today.with_day(1).unwrap_or(today);
        let last_day_prev = first_this - chrono::Duration::days(1);
        let first_prev = last_day_prev.with_day(1).unwrap_or(last_day_prev);
        ranges.push((first_prev, last_day_prev));
    }

    if ranges.is_empty() {
        return (None, None);
    }

    // 合并所有范围：取最早的开始日期和最晚的结束日期
    let earliest = ranges.iter().map(|(s, _)| *s).min().unwrap_or(today);
    let latest = ranges.iter().map(|(_, e)| *e).max().unwrap_or(today);

    (Some(fmt(earliest)), Some(fmt(latest)))
}

/// Parse a single date from user input for bot commands (e.g. `/report 昨天`).
/// Returns the resolved date string in YYYY-MM-DD format, or the input unchanged.
pub fn resolve_single_date(input: Option<&str>) -> String {
    use chrono::Datelike;

    let s = input.unwrap_or("today").to_lowercase();
    let today = chrono::Local::now().date_naive();
    let fmt = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();

    match s.as_str() {
        "today" | "今天" | "今日" => fmt(today),
        "yesterday" | "昨天" | "昨日" => fmt(today - chrono::Duration::days(1)),
        "前天" => fmt(today - chrono::Duration::days(2)),
        "本周" | "这周" => {
            let wd = today.weekday().num_days_from_monday() as i64;
            fmt(today - chrono::Duration::days(wd))
        }
        "上周" => {
            let wd = today.weekday().num_days_from_monday() as i64;
            let this_monday = today - chrono::Duration::days(wd);
            fmt(this_monday - chrono::Duration::days(7))
        }
        _ => s,
    }
}

/// 校验调用方传入的截图相对路径，防止绝对路径或 `..` 越界访问 data_dir 之外的文件
pub(crate) fn validate_relative_path(path: &str) -> Result<(), AppError> {
    use std::path::{Component, Path};

    let p = Path::new(path);
    if p.is_absolute() {
        return Err(AppError::Unknown(
            "非法路径：不允许使用绝对路径".to_string(),
        ));
    }
    if p.components()
        .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(AppError::Unknown(
            "非法路径：不允许包含 .. 等越界路径".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn collect_privacy_filters(state: &AppState) -> (Vec<String>, Vec<String>) {
    work_review_core::privacy::collect_privacy_filters(&state.config)
}

pub(crate) fn filter_activities_by_privacy(
    activities: Vec<Activity>,
    ignored_apps: &[String],
    excluded_domains: &[String],
) -> Vec<Activity> {
    let no_app_filter = ignored_apps.is_empty();
    let no_domain_filter = excluded_domains.is_empty();

    if no_app_filter && no_domain_filter {
        return activities;
    }

    activities
        .into_iter()
        .filter(|activity| {
            let app_lower = activity.app_name.to_lowercase();
            // 单向小写子串匹配，与 privacy::matches_ignored_app 保持一致：
            // 不做反向包含，避免忽略长名称时误伤短名称应用。
            if !no_app_filter
                && ignored_apps
                    .iter()
                    .any(|ignored| !ignored.is_empty() && app_lower.contains(ignored))
            {
                return false;
            }

            if !no_domain_filter {
                if let Some(url) = &activity.browser_url {
                    let domain = PrivacyConfig::extract_domain(url);
                    if excluded_domains
                        .iter()
                        .any(|excluded| PrivacyConfig::domain_matches(&domain, excluded))
                    {
                        return false;
                    }
                }
            }

            true
        })
        .collect()
}

pub(crate) fn load_filtered_activities_in_range(
    state: &AppState,
    date_from: Option<&str>,
    date_to: Option<&str>,
    limit: usize,
) -> Result<Vec<Activity>, AppError> {
    let activities = state
        .database
        .get_activities_in_range(date_from, date_to, limit)?;
    let (ignored_apps, excluded_domains) = collect_privacy_filters(state);
    Ok(filter_activities_by_privacy(
        activities,
        &ignored_apps,
        &excluded_domains,
    ))
}

fn manual_followups_in_range(
    items: &[AvatarFollowupItem],
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Vec<AvatarFollowupItem> {
    items
        .iter()
        .filter(|item| item.status == "open")
        .filter(|item| {
            date_from
                .map(|start| item.date.as_str() >= start)
                .unwrap_or(true)
                && date_to.map(|end| item.date.as_str() <= end).unwrap_or(true)
        })
        .cloned()
        .collect()
}

pub(crate) fn merge_manual_followups_into_todos(
    mut extracted: TodoExtractionResult,
    manual_items: &[AvatarFollowupItem],
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> TodoExtractionResult {
    let manual_items = manual_followups_in_range(manual_items, date_from, date_to);
    if manual_items.is_empty() {
        return extracted;
    }

    let mut seen = std::collections::HashSet::new();
    for item in &extracted.items {
        seen.insert(item.title.trim().to_lowercase());
    }

    for item in manual_items {
        let normalized = item.title.trim().to_lowercase();
        if normalized.is_empty() || !seen.insert(normalized) {
            continue;
        }

        extracted
            .items
            .push(work_review_core::work_intelligence::TodoItem {
                title: item.title.clone(),
                date: item.date.clone(),
                source_title: item.source_title.clone(),
                source_app: item.source_app.clone(),
                confidence: 96,
                reason: "桌宠手动加入待跟进".to_string(),
            });
    }

    extracted.items.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| b.date.cmp(&a.date))
            .then_with(|| a.title.cmp(&b.title))
    });
    extracted.items.truncate(20);
    extracted.summary = format!(
        "共整理出 {} 条待跟进项（含桌宠手动加入）。",
        extracted.items.len()
    );
    extracted
}

pub(crate) fn persist_app_config(
    mut config: AppConfig,
    app: AppHandle,
    state: &Arc<Mutex<AppState>>,
) -> Result<(), AppError> {
    config.normalize();
    let new_embedding_config_fingerprint =
        super::semantic_memory::embedding_config_fingerprint(&config);
    let new_privacy_fingerprint = super::semantic_memory::privacy_fingerprint(&config.privacy);
    let (
        previous_avatar_enabled,
        previous_avatar_scale,
        previous_avatar_opacity,
        previous_avatar_preset,
        previous_avatar_click_through,
        previous_avatar_body_hidden,
        previous_avatar_x,
        previous_avatar_y,
        previous_hide_dock_icon,
        previous_lightweight_mode,
        avatar_state,
    ) = {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let previous_config = state.config.clone();
        let previous_embedding_config_fingerprint =
            super::semantic_memory::embedding_config_fingerprint(&previous_config);
        let previous_privacy_fingerprint =
            super::semantic_memory::privacy_fingerprint(&previous_config.privacy);

        // Embedding 或隐私规则变化时必须先失效旧向量，再保存并启用新配置。
        // API Key 不参与指纹，因此单纯轮换密钥不会触发无必要的全量重建。
        if previous_embedding_config_fingerprint != new_embedding_config_fingerprint
            || previous_privacy_fingerprint != new_privacy_fingerprint
        {
            super::semantic_memory::invalidate_semantic_memory_index(&state.database)?;
        }

        // 先持久化文件；失败时内存配置和运行时隐私过滤器保持旧值。
        let config_path = state.config_path.clone();
        crate::config_sync::stamp_local_sync(&mut config);
        config.save(&config_path)?;

        state.config = config.clone();
        state.storage_manager.update_config(config.storage.clone());
        state.screenshot_service.update_config(&config.storage);
        state.privacy_filter.update_config(&config.privacy);
        state.avatar_state = crate::avatar_engine::apply_avatar_visual_settings(
            state.avatar_state.clone(),
            config.avatar_opacity,
            &config.avatar_preset,
            &config.avatar_persona,
            config.avatar_body_hidden,
        );
        (
            previous_config.avatar_enabled,
            previous_config.avatar_scale,
            previous_config.avatar_opacity,
            previous_config.avatar_preset,
            previous_config.avatar_click_through,
            previous_config.avatar_body_hidden,
            previous_config.avatar_x,
            previous_config.avatar_y,
            previous_config.hide_dock_icon,
            previous_config.lightweight_mode,
            state.avatar_state.clone(),
        )
    };

    let avatar_window_changed = previous_avatar_enabled != config.avatar_enabled
        || previous_avatar_scale != config.avatar_scale
        || previous_avatar_body_hidden != config.avatar_body_hidden
        || previous_avatar_x != config.avatar_x
        || previous_avatar_y != config.avatar_y;
    let avatar_click_through_changed = previous_avatar_click_through != config.avatar_click_through;
    let avatar_visual_changed = previous_avatar_opacity != config.avatar_opacity
        || previous_avatar_preset != config.avatar_preset;
    // 隐藏本体既是窗口尺寸变化也是视觉变化（前端需重新渲染 canvas 显隐）
    let avatar_body_hidden_changed = previous_avatar_body_hidden != config.avatar_body_hidden;
    let dock_visibility_changed = previous_hide_dock_icon != config.hide_dock_icon
        || previous_lightweight_mode != config.lightweight_mode;

    if avatar_window_changed {
        crate::avatar_engine::sync_avatar_window(
            &app,
            config.avatar_enabled,
            config.avatar_scale,
            config.avatar_x.zip(config.avatar_y),
            false,
            config.avatar_body_hidden,
        )
        .map_err(|e| AppError::Unknown(format!("同步桌宠窗口失败: {e}")))?;

        // 窗口创建/显示后应用鼠标穿透设置
        if config.avatar_enabled && config.avatar_click_through {
            crate::avatar_engine::set_avatar_click_through(&app, true);
        }
    }

    if config.avatar_enabled
        && (avatar_window_changed || avatar_visual_changed || avatar_body_hidden_changed)
        && !refresh_avatar_state_for_current_window(&app, state)
    {
        crate::avatar_engine::emit_avatar_state(&app, &avatar_state);
    }

    if avatar_click_through_changed && config.avatar_enabled {
        crate::avatar_engine::set_avatar_click_through(&app, config.avatar_click_through);
    }

    // 同步智能穿透运行时 flag（供 spawn_avatar_input_bridge 轮询无锁读）
    crate::avatar_input::set_avatar_enabled_flag(config.avatar_enabled);
    crate::avatar_input::set_avatar_click_through_flag(config.avatar_click_through);
    if config.avatar_enabled && !previous_avatar_enabled {
        crate::avatar_input::start_avatar_input_monitor(&app);
        crate::avatar_input::spawn_avatar_input_monitor_retry(app.clone());
        crate::avatar_input::spawn_avatar_input_bridge(app.clone());
    }
    if avatar_window_changed || avatar_click_through_changed {
        crate::avatar_input::force_resync_click_through();
    }

    if dock_visibility_changed {
        crate::sync_effective_dock_visibility(&app);
    }

    crate::localhost_api::sync_localhost_api_runtime(&app, state)?;
    crate::telegram_bot::sync_telegram_bot_runtime(state)?;
    crate::wecom_aibot::sync_wecom_bot_runtime(state)?;
    crate::config_sync::spawn_push(config.clone());
    crate::emit_config_changed(&app, &config);

    log::info!("配置已保存");
    Ok(())
}

fn refresh_avatar_state_for_current_window(app: &AppHandle, state: &Arc<Mutex<AppState>>) -> bool {
    let active_window = match crate::monitor::get_active_window_fast() {
        Ok(window) => window,
        Err(_) => return false,
    };

    let next_avatar_state = {
        let mut state_guard = match state.lock() {
            Ok(guard) => guard,
            Err(e) => {
                log::warn!("刷新桌宠状态时获取状态锁失败: {e}");
                return false;
            }
        };

        if !state_guard.config.avatar_enabled {
            return false;
        }

        let next_state = crate::avatar_engine::apply_avatar_visual_settings(
            crate::avatar_engine::derive_avatar_state_with_rules(
                &state_guard.config.app_category_rules,
                &state_guard.config.custom_categories,
                &active_window.app_name,
                &active_window.window_title,
                active_window.browser_url.as_deref(),
                state_guard.avatar_state.is_idle,
                state_guard.avatar_generating_report,
            ),
            state_guard.config.avatar_opacity,
            &state_guard.config.avatar_preset,
            &state_guard.config.avatar_persona,
            state_guard.config.avatar_body_hidden,
        );
        state_guard.avatar_state = next_state.clone();
        next_state
    };

    crate::avatar_engine::emit_avatar_state(app, &next_avatar_state);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use work_review_core::database::{BrowserUsage, DailyStats, DomainUsage, UrlDetail, UrlUsage};
    use work_review_core::privacy::apply_excluded_domains_to_stats;

    #[test]
    fn 概览统计应过滤排除域名并重算浏览器时长() {
        let stats = DailyStats {
            total_duration: 360,
            screenshot_count: 6,
            app_usage: vec![],
            category_usage: vec![],
            browser_duration: 210,
            url_usage: vec![
                UrlUsage {
                    url: "https://linux.do/latest".to_string(),
                    domain: "linux.do".to_string(),
                    duration: 120,
                },
                UrlUsage {
                    url: "linux.dolatest".to_string(),
                    domain: "linux.dolatest".to_string(),
                    duration: 30,
                },
                UrlUsage {
                    url: "https://github.com/issues".to_string(),
                    domain: "github.com".to_string(),
                    duration: 60,
                },
            ],
            domain_total_count: 3,
            domain_usage: vec![
                DomainUsage {
                    domain: "linux.do".to_string(),
                    duration: 120,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://linux.do/latest".to_string(),
                        duration: 120,
                    }],
                },
                DomainUsage {
                    domain: "linux.dolatest".to_string(),
                    duration: 30,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: vec![UrlDetail {
                        url: "linux.dolatest".to_string(),
                        duration: 30,
                    }],
                },
                DomainUsage {
                    domain: "github.com".to_string(),
                    duration: 60,
                    semantic_category: Some("编码开发".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://github.com/issues".to_string(),
                        duration: 60,
                    }],
                },
            ],
            browser_usage: vec![BrowserUsage {
                browser_name: "Google Chrome".to_string(),
                duration: 210,
                executable_path: Some("/Applications/Google Chrome.app".to_string()),
                domains: vec![
                    DomainUsage {
                        domain: "linux.do".to_string(),
                        duration: 120,
                        semantic_category: Some("资料阅读".to_string()),
                        urls: vec![UrlDetail {
                            url: "https://linux.do/latest".to_string(),
                            duration: 120,
                        }],
                    },
                    DomainUsage {
                        domain: "linux.dolatest".to_string(),
                        duration: 30,
                        semantic_category: Some("资料阅读".to_string()),
                        urls: vec![UrlDetail {
                            url: "linux.dolatest".to_string(),
                            duration: 30,
                        }],
                    },
                    DomainUsage {
                        domain: "github.com".to_string(),
                        duration: 60,
                        semantic_category: Some("编码开发".to_string()),
                        urls: vec![UrlDetail {
                            url: "https://github.com/issues".to_string(),
                            duration: 60,
                        }],
                    },
                ],
            }],
            work_time_duration: 0,
            overtime_duration: 0,
            hourly_activity_distribution: vec![],
        };

        let filtered = apply_excluded_domains_to_stats(stats, &["linux.do".to_string()]);

        assert_eq!(filtered.browser_duration, 60);
        assert_eq!(filtered.browser_usage.len(), 1);
        assert_eq!(filtered.browser_usage[0].domains.len(), 1);
        assert_eq!(filtered.browser_usage[0].domains[0].domain, "github.com");
        assert_eq!(filtered.url_usage.len(), 1);
        assert_eq!(filtered.url_usage[0].domain, "github.com");
        assert_eq!(filtered.domain_usage.len(), 1);
        assert_eq!(filtered.domain_usage[0].domain, "github.com");
    }

    /// 辅助：获取今天日期字符串
    fn today_str() -> String {
        use chrono::Local;
        Local::now().date_naive().format("%Y-%m-%d").to_string()
    }

    /// 辅助：获取 N 天前的日期字符串
    fn days_ago_str(n: i64) -> String {
        use chrono::Local;
        let today = Local::now().date_naive();
        (today - chrono::Duration::days(n))
            .format("%Y-%m-%d")
            .to_string()
    }

    /// 辅助：获取本月1号的日期字符串
    fn first_of_month_str() -> String {
        use chrono::{Datelike, Local};
        let today = Local::now().date_naive();
        today
            .with_day(1)
            .unwrap_or(today)
            .format("%Y-%m-%d")
            .to_string()
    }

    /// 辅助：获取上月1号和上月最后一天的日期字符串
    fn prev_month_bounds() -> (String, String) {
        use chrono::{Datelike, Local};
        let today = Local::now().date_naive();
        let first_this = today.with_day(1).unwrap_or(today);
        let last_day_prev = first_this - chrono::Duration::days(1);
        let first_prev = last_day_prev.with_day(1).unwrap_or(last_day_prev);
        (
            first_prev.format("%Y-%m-%d").to_string(),
            last_day_prev.format("%Y-%m-%d").to_string(),
        )
    }

    #[test]
    fn test_temporal_single_today() {
        let (from, to) = parse_temporal_range("今天做了什么");
        assert_eq!(from.as_deref(), Some(today_str().as_str()));
        assert_eq!(to.as_deref(), Some(today_str().as_str()));
    }

    #[test]
    fn test_temporal_single_yesterday() {
        let (from, to) = parse_temporal_range("昨天做了什么");
        assert_eq!(from.as_deref(), Some(days_ago_str(1).as_str()));
        assert_eq!(to.as_deref(), Some(days_ago_str(1).as_str()));
    }

    #[test]
    fn test_temporal_single_this_month() {
        let (from, to) = parse_temporal_range("这个月的工作总结");
        assert_eq!(from.as_deref(), Some(first_of_month_str().as_str()));
        assert_eq!(to.as_deref(), Some(today_str().as_str()));
    }

    #[test]
    fn test_temporal_single_last_month() {
        let (from, to) = parse_temporal_range("上个月做了什么");
        let (prev_first, prev_last) = prev_month_bounds();
        assert_eq!(from.as_deref(), Some(prev_first.as_str()));
        assert_eq!(to.as_deref(), Some(prev_last.as_str()));
    }

    #[test]
    fn test_temporal_recent_3_days() {
        let (from, to) = parse_temporal_range("最近3天的工作");
        assert_eq!(from.as_deref(), Some(days_ago_str(3).as_str()));
        assert_eq!(to.as_deref(), Some(today_str().as_str()));
    }

    #[test]
    fn test_temporal_recent_no_number() {
        let (from, to) = parse_temporal_range("最近的工作情况");
        assert_eq!(from.as_deref(), Some(days_ago_str(7).as_str()));
        assert_eq!(to.as_deref(), Some(today_str().as_str()));
    }

    #[test]
    fn test_temporal_no_match() {
        let (from, to) = parse_temporal_range("帮我分析一下工作效率");
        assert!(from.is_none());
        assert!(to.is_none());
    }

    #[test]
    fn test_temporal_this_month_plus_last_month() {
        // 核心场景：问"这个月加上上个月"应返回 (上月1号, 今天)
        let (from, to) = parse_temporal_range("这个月加上上个月做了什么");
        let (prev_first, _) = prev_month_bounds();
        assert_eq!(
            from.as_deref(),
            Some(prev_first.as_str()),
            "起始日期应为上月1号"
        );
        assert_eq!(
            to.as_deref(),
            Some(today_str().as_str()),
            "结束日期应为今天"
        );
    }

    #[test]
    fn test_temporal_today_and_yesterday() {
        let (from, to) = parse_temporal_range("今天和昨天做了什么");
        assert_eq!(from.as_deref(), Some(days_ago_str(1).as_str()));
        assert_eq!(to.as_deref(), Some(today_str().as_str()));
    }

    #[test]
    fn test_temporal_this_week_and_last_week() {
        use chrono::{Datelike, Local};
        let today = Local::now().date_naive();
        let wd = today.weekday().num_days_from_monday() as i64;
        let this_monday = today - chrono::Duration::days(wd);
        let last_monday = this_monday - chrono::Duration::days(7);

        let (from, to) = parse_temporal_range("本周和上周的工作");
        assert_eq!(
            from.as_deref(),
            Some(last_monday.format("%Y-%m-%d").to_string().as_str()),
            "起始日期应为上周一"
        );
        assert_eq!(
            to.as_deref(),
            Some(today_str().as_str()),
            "结束日期应为今天"
        );
    }

    #[test]
    fn test_temporal_this_month_and_last_month_and_yesterday() {
        // 三个时间段合并：上个月最早，今天最晚
        let (from, to) = parse_temporal_range("这个月、上个月还有昨天的内容");
        let (prev_first, _) = prev_month_bounds();
        assert_eq!(from.as_deref(), Some(prev_first.as_str()));
        assert_eq!(to.as_deref(), Some(today_str().as_str()));
    }

    #[test]
    fn test_temporal_ben_yue_and_shang_yue() {
        // 用"本月"和"上月"变体关键词
        let (from, to) = parse_temporal_range("本月和上月有什么区别");
        let (prev_first, _) = prev_month_bounds();
        assert_eq!(from.as_deref(), Some(prev_first.as_str()));
        assert_eq!(to.as_deref(), Some(today_str().as_str()));
    }
}
