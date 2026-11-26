//! 活动录制合并、尾段回补、会话快照退出落盘与轮询节奏决策。

use crate::{activity_clock, monitor, AppState};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use work_review_core::config::AppConfig;
use work_review_core::database::Database;
use work_review_core::privacy::PrivacyFilter;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecordingLoopDecision {
    pub(crate) should_continue: bool,
    pub(crate) screenshot_interval: u64,
    pub(crate) reset_capture_clock: bool,
}

pub(crate) const ACTIVITY_INPUT_IDLE_HARD_STOP_MINUTES: u64 = 10;
pub(crate) const ACTIVITY_INPUT_IDLE_HARD_STOP_SECS: u64 =
    ACTIVITY_INPUT_IDLE_HARD_STOP_MINUTES * 60;

pub(crate) fn should_confirm_idle(
    input_idle: bool,
    input_idle_seconds: u64,
    screenshots_enabled: bool,
    _screenshot_confirmed: bool,
) -> bool {
    if !input_idle {
        return false;
    }

    // 长时间无输入时直接切断时长，避免后台程序或动态页面无限续时。
    if input_idle_seconds >= ACTIVITY_INPUT_IDLE_HARD_STOP_SECS {
        return true;
    }

    // 键鼠超时但未到硬超时：不再因"画面无变化"判空闲。
    // 创意类应用（PS/C4D/Blender）、AI 网页阅读、代码思考等场景下，画面可能长时间
    // 几乎不变但用户确实在工作。哈希相似度对这类场景是反信号，故废弃其"确认空闲"作用。
    // 关闭截图时无法看画面，回退到"键鼠超时即空闲"（保留原行为）。
    !screenshots_enabled
}

pub(crate) fn previous_app_backfill_duration(
    app_changed: bool,
    duration_to_record: i64,
    was_input_idle: bool,
    is_confirmed_idle: bool,
) -> i64 {
    if !app_changed || duration_to_record <= 0 || was_input_idle || is_confirmed_idle {
        0
    } else {
        duration_to_record
    }
}

pub(crate) fn should_persist_merge_update(effective_duration: i64) -> bool {
    effective_duration > 0
}

pub(crate) fn should_merge_observed_tail(
    latest_timestamp: i64,
    observed_timestamp: i64,
    duration: i64,
) -> bool {
    // 两次取整最多相差一秒；旧记录若在更早的空闲前结束，必须新建尾段而非移动旧区间。
    duration > 0
        && latest_timestamp <= observed_timestamp
        && latest_timestamp.abs_diff(observed_timestamp.saturating_sub(duration)) <= 1
}

pub(crate) const ACTIVITY_MERGE_GAP_SECS: i64 = 600;

pub(crate) fn should_merge_contiguous_activity(
    app_changed: bool,
    app_name: &str,
    current_timestamp: i64,
    latest_timestamp: i64,
) -> bool {
    !app_changed
        && app_name != "Unknown"
        && current_timestamp >= latest_timestamp
        && current_timestamp - latest_timestamp <= ACTIVITY_MERGE_GAP_SECS
}

pub(crate) fn resolve_previous_activity_to_backfill(
    state: &Arc<Mutex<AppState>>,
    previous_app_name: Option<&str>,
    previous_browser_url: Option<&str>,
    previous_window_title: Option<&str>,
) -> Option<work_review_core::database::Activity> {
    let previous_app_name = previous_app_name?;

    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(previous_url) = previous_browser_url.filter(|url| !url.is_empty()) {
        state_guard
            .database
            .get_latest_activity_by_url(previous_url)
            .ok()
            .flatten()
    } else if work_review_core::categorize::is_browser_app(previous_app_name) {
        previous_window_title
            .filter(|title| !title.is_empty())
            .and_then(|title| {
                state_guard
                    .database
                    .get_latest_activity_by_app_title(previous_app_name, title)
                    .ok()
                    .flatten()
            })
            .or_else(|| {
                state_guard
                    .database
                    .get_latest_activity_by_app(previous_app_name)
                    .ok()
                    .flatten()
            })
    } else {
        state_guard
            .database
            .get_latest_activity_by_app(previous_app_name)
            .ok()
            .flatten()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn persist_previous_activity_backfill(
    database: &Database,
    config: &AppConfig,
    privacy_filter: &PrivacyFilter,
    previous_activity: Option<&work_review_core::database::Activity>,
    previous_app_name: Option<&str>,
    previous_window_title: Option<&str>,
    previous_browser_url: Option<&str>,
    duration_delta: i64,
    current_timestamp: i64,
    current_app_name: &str,
) -> Option<i64> {
    if duration_delta <= 0 {
        return None;
    }

    if let Some(previous_activity) = previous_activity {
        let previous_id = previous_activity.id?;

        return match database.merge_activity(
            previous_id,
            duration_delta,
            None,
            &previous_activity.screenshot_path,
            current_timestamp,
            None,
        ) {
            Ok(()) => {
                log::debug!(
                    "⏱️ 时长回补: {} +{}s (切换到 {})",
                    previous_activity.app_name,
                    duration_delta,
                    current_app_name
                );
                Some(previous_id)
            }
            Err(error) => {
                log::error!("上一应用时长回补失败: {error}");
                None
            }
        };
    }

    let previous_app_name = previous_app_name?.trim();
    if previous_app_name.is_empty() {
        return None;
    }

    let previous_window_title = previous_window_title.unwrap_or("");
    let previous_browser_url = previous_browser_url.filter(|url| !url.is_empty());
    let privacy_action = privacy_filter.check_privacy_full(
        previous_app_name,
        previous_window_title,
        previous_browser_url,
    );
    if privacy_action == work_review_core::privacy::PrivacyAction::Skip {
        log::debug!("上一应用回补跳过(隐私): {previous_app_name}");
        return None;
    }

    let classification = crate::resolve_activity_classification(
        config,
        previous_app_name,
        previous_window_title,
        previous_browser_url,
    );
    let (window_title, browser_url) =
        if privacy_action == work_review_core::privacy::PrivacyAction::Anonymize {
            ("[内容已脱敏]".to_string(), None)
        } else {
            (
                previous_window_title.to_string(),
                previous_browser_url.map(ToString::to_string),
            )
        };

    let activity = work_review_core::database::Activity {
        id: None,
        timestamp: current_timestamp,
        app_name: previous_app_name.to_string(),
        window_title,
        screenshot_path: String::new(),
        ocr_text: None,
        category: classification.base_category,
        duration: duration_delta,
        browser_url,
        executable_path: None,
        semantic_category: Some(classification.semantic_category),
        semantic_confidence: Some(i32::from(classification.confidence)),
        screenshot_url: None,
    };

    match database.insert_activity(&activity) {
        Ok(activity_id) => {
            log::debug!(
                "⏱️ 新建上一应用回补: {previous_app_name} +{duration_delta}s (id={activity_id}, 切换到 {current_app_name})"
            );
            Some(activity_id)
        }
        Err(e) => {
            log::error!("上一应用回补记录失败: {e}");
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn backfill_previous_activity_if_needed(
    state: &Arc<Mutex<AppState>>,
    previous_activity: Option<&work_review_core::database::Activity>,
    previous_app_name: Option<&str>,
    previous_window_title: Option<&str>,
    previous_browser_url: Option<&str>,
    duration_delta: i64,
    current_timestamp: i64,
    current_app_name: &str,
) -> bool {
    if duration_delta <= 0 {
        return true;
    }

    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
    persist_previous_activity_backfill(
        &state_guard.database,
        &state_guard.config,
        &state_guard.privacy_filter,
        previous_activity,
        previous_app_name,
        previous_window_title,
        previous_browser_url,
        duration_delta,
        current_timestamp,
        current_app_name,
    )
    .is_some()
}

pub(crate) fn recording_loop_decision(
    is_recording: bool,
    is_paused: bool,
    screenshot_interval: u64,
) -> RecordingLoopDecision {
    if !is_recording || is_paused {
        RecordingLoopDecision {
            should_continue: false,
            screenshot_interval: 1,
            reset_capture_clock: true,
        }
    } else {
        RecordingLoopDecision {
            should_continue: true,
            screenshot_interval,
            reset_capture_clock: false,
        }
    }
}

/// 进行中的录制会话快照，用于在应用退出（ExitRequested / Exit）时落盘尚未结算的尾段时长。
#[derive(Debug, Clone)]
pub(crate) struct ActiveRecordingSession {
    pub(crate) app_name: String,
    pub(crate) window_title: String,
    pub(crate) browser_url: Option<String>,
    pub(crate) last_capture_time: std::time::Instant,
    pub(crate) last_observed_instant: std::time::Instant,
    pub(crate) last_observed_timestamp: i64,
    pub(crate) is_idle: bool,
}

static ACTIVE_RECORDING_SESSION: once_cell::sync::Lazy<Mutex<Option<ActiveRecordingSession>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

pub(crate) fn update_active_recording_session_snapshot(snapshot: Option<ActiveRecordingSession>) {
    if let Ok(mut guard) = ACTIVE_RECORDING_SESSION.lock() {
        *guard = snapshot;
    }
}

/// 进程退出时将当前进行中的活动尾段回补落盘，避免最后一段活跃时长在关闭应用时丢失。
pub(crate) fn flush_active_recording_session_on_exit(state: &Arc<Mutex<AppState>>) {
    let snapshot = ACTIVE_RECORDING_SESSION
        .lock()
        .ok()
        .and_then(|mut guard| guard.take());
    let Some(session) = snapshot else {
        return;
    };
    if session.is_idle {
        return;
    }
    let tail_duration = session
        .last_observed_instant
        .saturating_duration_since(session.last_capture_time)
        .as_secs() as i64;
    if tail_duration <= 0 {
        return;
    }
    let can_persist = {
        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.privacy_filter.check_privacy_full(
            &session.app_name,
            &session.window_title,
            session.browser_url.as_deref(),
        ) != work_review_core::privacy::PrivacyAction::Skip
    };
    if !can_persist {
        return;
    }
    let previous = resolve_previous_activity_to_backfill(
        state,
        Some(session.app_name.as_str()),
        session.browser_url.as_deref(),
        Some(session.window_title.as_str()),
    )
    .filter(|activity| {
        should_merge_observed_tail(
            activity.timestamp,
            session.last_observed_timestamp,
            tail_duration,
        )
    });
    backfill_previous_activity_if_needed(
        state,
        previous.as_ref(),
        Some(session.app_name.as_str()),
        Some(session.window_title.as_str()),
        session.browser_url.as_deref(),
        tail_duration,
        session.last_observed_timestamp,
        "应用退出",
    );
}

/// 统一在重置录制会话（暂停、锁屏、采样中断、窗口查询失败、系统/瞬态窗口）前，
/// 先把已连续观察到的尾段时长回补给上一个活动，绝不吞掉已观察尾段，也绝不捏造未知空档。
#[allow(clippy::too_many_arguments)]
pub(crate) fn flush_observed_tail_and_reset(
    state: &Arc<Mutex<AppState>>,
    last_app_name: &mut Option<String>,
    last_window_title: &mut Option<String>,
    last_browser_url: &mut Option<String>,
    last_capture_time: &mut std::time::Instant,
    overlay_sessions: &mut std::collections::HashMap<String, (i64, std::time::Instant)>,
    observed_sample: activity_clock::Sample,
    is_currently_idle: bool,
    reason: &str,
    now: std::time::Instant,
) {
    let tail_duration = observed_sample
        .instant
        .saturating_duration_since(*last_capture_time)
        .as_secs() as i64;
    let can_persist_tail = tail_duration > 0 && !is_currently_idle && {
        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
        last_app_name.as_deref().is_some_and(|name| {
            guard.privacy_filter.check_privacy_full(
                name,
                last_window_title.as_deref().unwrap_or(""),
                last_browser_url.as_deref(),
            ) != work_review_core::privacy::PrivacyAction::Skip
        })
    };
    if can_persist_tail {
        let previous = resolve_previous_activity_to_backfill(
            state,
            last_app_name.as_deref(),
            last_browser_url.as_deref(),
            last_window_title.as_deref(),
        )
        .filter(|activity| {
            should_merge_observed_tail(
                activity.timestamp,
                observed_sample.timestamp(),
                tail_duration,
            )
        });
        backfill_previous_activity_if_needed(
            state,
            previous.as_ref(),
            last_app_name.as_deref(),
            last_window_title.as_deref(),
            last_browser_url.as_deref(),
            tail_duration,
            observed_sample.timestamp(),
            reason,
        );
    }

    reset_recording_session(
        last_app_name,
        last_window_title,
        last_browser_url,
        last_capture_time,
        overlay_sessions,
        now,
    );
    update_active_recording_session_snapshot(None);
}

/// 空闲期间自动放宽截图采样周期（3 倍且不低于 90 秒），降低离席时的无效截屏与 CPU 消耗。
pub(crate) fn effective_screenshot_interval_secs(base_interval: u64, input_idle: bool) -> u64 {
    if input_idle {
        base_interval.saturating_mul(3).max(90)
    } else {
        base_interval
    }
}

pub(crate) fn reset_recording_session(
    last_app_name: &mut Option<String>,
    last_window_title: &mut Option<String>,
    last_browser_url: &mut Option<String>,
    last_capture_time: &mut std::time::Instant,
    overlay_sessions: &mut std::collections::HashMap<String, (i64, std::time::Instant)>,
    now: std::time::Instant,
) {
    *last_app_name = None;
    *last_window_title = None;
    *last_browser_url = None;
    *last_capture_time = now;
    overlay_sessions.clear();
}

pub(crate) fn should_run_ocr(
    elapsed_since_ocr: Option<Duration>,
    screenshot_interval: u64,
) -> bool {
    match elapsed_since_ocr {
        None => true,
        Some(elapsed) => elapsed >= Duration::from_secs(screenshot_interval),
    }
}

/// 浮窗只合并连续观察到的同一条记录，并保留不足一秒的余量。
pub(crate) fn overlay_session_increment(
    previous: Option<(i64, std::time::Instant)>,
    latest: &work_review_core::database::Activity,
    now: std::time::Instant,
    timestamp: i64,
) -> Option<(i64, std::time::Instant)> {
    let (activity_id, accounted_at) = previous?;
    if latest.id != Some(activity_id)
        || latest.app_name == "Unknown"
        || !(0..=600).contains(&timestamp.saturating_sub(latest.timestamp))
    {
        return None;
    }
    let seconds = now.checked_duration_since(accounted_at)?.as_secs();
    Some((seconds as i64, accounted_at + Duration::from_secs(seconds)))
}

pub(crate) fn monitoring_poll_interval_ms_for_platform(is_macos: bool) -> u64 {
    if is_macos {
        1500
    } else if cfg!(target_os = "linux") {
        1000
    } else {
        500
    }
}

pub(crate) fn monitoring_poll_interval_ms() -> u64 {
    monitoring_poll_interval_ms_for_platform(cfg!(target_os = "macos"))
}

pub(crate) const ACTIVE_WINDOW_CACHE_MAX_AGE_MS: u64 = 1250;
pub(crate) const MIN_CAPTURE_INTERVAL_MS: u128 = 3000;
pub(crate) const MIN_BROWSER_CHANGE_CAPTURE_INTERVAL_MS: u128 = 1200;

pub(crate) fn reusable_cached_active_window(
    cached: Option<&(std::time::Instant, monitor::ActiveWindow)>,
    now: std::time::Instant,
) -> Option<monitor::ActiveWindow> {
    let (sampled_at, active_window) = cached?;
    let age = now.checked_duration_since(*sampled_at)?;

    if age > Duration::from_millis(ACTIVE_WINDOW_CACHE_MAX_AGE_MS) {
        return None;
    }

    Some(active_window.clone())
}

pub(crate) fn screen_lock_check_interval_ms_for_platform(is_macos: bool) -> u64 {
    if cfg!(target_os = "linux") || is_macos {
        // Linux 命令检测有超时，macOS 为 FFI；两者都不需要秒级轮询。
        5000
    } else {
        1000
    }
}

pub(crate) fn screen_lock_check_interval_ms() -> u64 {
    screen_lock_check_interval_ms_for_platform(cfg!(target_os = "macos"))
}
