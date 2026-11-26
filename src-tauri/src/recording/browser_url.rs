//! 浏览器 URL 探测节奏控制与同标题最近 URL 兜底恢复。

use super::merge::{MIN_BROWSER_CHANGE_CAPTURE_INTERVAL_MS, MIN_CAPTURE_INTERVAL_MS};
use work_review_core::database::Database;
/// 浏览器 URL 采集偶发失败时，尝试从最近同窗口标题的活动里恢复 URL。
/// 这是近似统计兜底：优先减少同一页面被切碎成多段或掉成 0 站点 0 页面。
pub(crate) fn recover_recent_browser_url(
    database: &Database,
    app_name: &str,
    window_title: &str,
    now_ts: i64,
    max_age_secs: i64,
) -> Option<String> {
    if !work_review_core::categorize::is_browser_app(app_name) || window_title.is_empty() {
        return None;
    }

    database
        .get_latest_activity_by_app_title(app_name, window_title)
        .ok()
        .flatten()
        .and_then(|activity| {
            let age = now_ts - activity.timestamp;
            if age <= max_age_secs {
                activity.browser_url.filter(|url| !url.is_empty())
            } else {
                None
            }
        })
}

pub(crate) fn should_probe_browser_url_before_change_detection(
    app_name: &str,
    window_title: &str,
    last_app_name: Option<&str>,
    last_window_title: Option<&str>,
    current_browser_url: Option<&str>,
) -> bool {
    if !work_review_core::categorize::is_browser_app(app_name) || window_title.is_empty() {
        return false;
    }
    // 首次遇到浏览器窗口时（last 为 None），也需要探测 URL
    if last_app_name.is_none() || last_window_title.is_none() {
        return current_browser_url.is_none();
    }
    // 同窗口持续使用时，探测 URL 变化
    last_app_name == Some(app_name) && last_window_title == Some(window_title)
}

pub(crate) fn claim_browser_url_probe(
    browser_url_probe_attempted: &mut bool,
    eligible: bool,
) -> bool {
    if *browser_url_probe_attempted || !eligible {
        return false;
    }

    // 查询开始即消耗本轮预算。即使 URL 获取失败，也不能立即重复扫描 UIA。
    *browser_url_probe_attempted = true;
    true
}

#[cfg(test)]
pub(crate) fn resolve_browser_url_once<F>(
    browser_url_probe_attempted: &mut bool,
    eligible: bool,
    resolve: F,
) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    claim_browser_url_probe(browser_url_probe_attempted, eligible).then(resolve)?
}

pub(crate) fn browser_url_probe_attempted_after_full_lookup(
    used_full_window_lookup: bool,
    is_browser: bool,
) -> bool {
    used_full_window_lookup && is_browser
}

pub(crate) fn browser_change_capture_min_interval_ms(
    app_name: &str,
    title_changed: bool,
    url_changed: bool,
) -> u128 {
    if work_review_core::categorize::is_browser_app(app_name) && (title_changed || url_changed) {
        MIN_BROWSER_CHANGE_CAPTURE_INTERVAL_MS
    } else {
        MIN_CAPTURE_INTERVAL_MS
    }
}

pub(crate) fn should_refresh_browser_url_before_record(app_name: &str, window_title: &str) -> bool {
    work_review_core::categorize::is_browser_app(app_name) && !window_title.is_empty()
}
