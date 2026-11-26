//! 后台活动采样、截图归档、OCR 与浮窗追踪主循环。

use crate::recording::*;
use crate::window::{should_skip_system_window, should_skip_transient_window};
use crate::{
    activity_clock, idle_detector, monitor, ocr, remote_upload, screen_lock, screenshot, AppState,
    ScreenshotResult, ScreenshotService, MAIN_WINDOW_LABEL,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
pub(crate) async fn capture_screenshot_for_window_with_options(
    state: &Arc<Mutex<AppState>>,
    active_window: &monitor::ActiveWindow,
    sample: activity_clock::Sample,
    need_ocr_source: bool,
) -> work_review_core::error::Result<ScreenshotResult> {
    let screenshot_service = {
        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        state_guard.screenshot_service.clone()
    };
    let active_window = active_window.clone();
    let result = tokio::task::spawn_blocking(move || {
        screenshot_service
            .capture_for_window_with_options(Some(&active_window), need_ocr_source)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| {
        work_review_core::error::AppError::Unknown(format!("截图任务异常: {error}"))
    })?;

    let result = result.map_err(work_review_core::error::AppError::Screenshot)?;
    if !activity_clock::Sample::now().follows(sample) {
        if let Some(path) = result
            .ocr_source_path
            .as_ref()
            .filter(|path| *path != &result.path)
        {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(&result.path);
        let thumb = ScreenshotService::thumbnail_cache_path(&result.path);
        if thumb != result.path {
            let _ = std::fs::remove_file(&thumb);
        }
        return Err(work_review_core::error::AppError::Screenshot(
            "截图跨越待机或采样中断，已丢弃本次结果".to_string(),
        ));
    }
    Ok(result)
}

pub(crate) async fn capture_merge_ocr_for_window(
    state: &Arc<Mutex<AppState>>,
    active_window: &monitor::ActiveWindow,
    sample: activity_clock::Sample,
    last_hash: u64,
    need_ocr: bool,
) -> work_review_core::error::Result<screenshot::MergeCaptureResult> {
    let screenshot_service = {
        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        state_guard.screenshot_service.clone()
    };
    let active_window = active_window.clone();
    let result = tokio::task::spawn_blocking(move || {
        screenshot_service
            .capture_for_merge_ocr(Some(&active_window), last_hash, need_ocr)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| {
        work_review_core::error::AppError::Unknown(format!("合并截图任务异常: {error}"))
    })?;

    let result = result.map_err(work_review_core::error::AppError::Screenshot)?;
    if !activity_clock::Sample::now().follows(sample) {
        if let Some(path) = result.ocr_source_path.as_ref() {
            let _ = std::fs::remove_file(path);
        }
        return Err(work_review_core::error::AppError::Screenshot(
            "合并截图跨越待机或采样中断，已丢弃本次结果".to_string(),
        ));
    }
    Ok(result)
}

/// 后台截屏任务
/// 使用 Arc<Mutex<AppState>> 而非 tauri::State，因为 State 无法在 async move 块中手动构造
pub(crate) async fn background_screenshot_task(state: Arc<Mutex<AppState>>, app: AppHandle) {
    // ===== 状态变量 =====
    let mut last_app_name: Option<String> = None;
    let mut last_app_window_title: Option<String> = None;
    let mut last_browser_url: Option<String> = None;

    let _power_monitor = activity_clock::PowerMonitor::new();
    let mut activity_clock = activity_clock::ActivityClock::new();
    let mut last_capture_time = std::time::Instant::now();
    let mut overlay_sessions = std::collections::HashMap::new();

    // ===== 空闲检测器与初始轮询间隔 =====
    // 先以输入空闲进入”疑似空闲”，再结合前台变化做短时保留。
    // 使用用户配置的空闲阈值与轮询间隔，每轮从配置热同步，修改后即时生效。
    let (mut idle_threshold_minutes, mut last_logged_screenshot_interval) = {
        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
        (
            (guard.config.idle_threshold_minutes as u64).max(1),
            guard.config.screenshot_interval,
        )
    };
    let idle_detector = idle_detector::IdleDetector::new(idle_threshold_minutes);
    let mut last_idle_log_time = std::time::Instant::now();
    let mut is_currently_idle = false; // 当前是否处于空闲状态

    let poll_interval_ms = monitoring_poll_interval_ms(); // 桌宠状态和窗口切换检测优先更快反馈
    log::info!(
        "活动监控任务启动: 定时轮询间隔={last_logged_screenshot_interval}秒, 窗口检测节拍={poll_interval_ms}ms, 空闲阈值={idle_threshold_minutes}分钟"
    );

    // OCR 并发限制：最多 2 个 OCR 任务同时运行，防止任务堆积消耗内存
    let ocr_semaphore = Arc::new(tokio::sync::Semaphore::new(2));

    // 合并路径的截图哈希去重：用 Arc 共享给异步任务，避免 static 跨活动污染
    let merge_screenshot_hash = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut last_ocr_time: Option<std::time::Instant> = None;

    // 锁屏检测器（无内部状态，复用同一实例避免重复分配）
    let screen_lock_monitor = screen_lock::ScreenLockMonitor::new();
    let mut last_screen_lock_check = std::time::Instant::now()
        .checked_sub(Duration::from_millis(screen_lock_check_interval_ms()))
        .unwrap_or_else(std::time::Instant::now);
    let mut cached_screen_locked = false;

    loop {
        // 首先检查录制状态并热同步配置（包含截图间隔与空闲阈值）
        let (decision, configured_idle_minutes) = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                recording_loop_decision(
                    state_guard.is_recording,
                    state_guard.is_paused,
                    state_guard.config.screenshot_interval,
                ),
                (state_guard.config.idle_threshold_minutes as u64).max(1),
            )
        };
        if configured_idle_minutes != idle_threshold_minutes {
            idle_threshold_minutes = configured_idle_minutes;
            idle_detector.set_threshold_minutes(idle_threshold_minutes);
            log::info!("空闲阈值配置已热更新为 {idle_threshold_minutes} 分钟");
        }
        if decision.should_continue
            && decision.screenshot_interval != last_logged_screenshot_interval
        {
            last_logged_screenshot_interval = decision.screenshot_interval;
            log::info!("活动轮询间隔配置已热更新为 {last_logged_screenshot_interval} 秒");
        }

        if decision.reset_capture_clock {
            let last_observed = activity_clock.previous_sample();
            flush_observed_tail_and_reset(
                &state,
                &mut last_app_name,
                &mut last_app_window_title,
                &mut last_browser_url,
                &mut last_capture_time,
                &mut overlay_sessions,
                last_observed,
                is_currently_idle,
                "录制暂停",
                std::time::Instant::now(),
            );
        }

        if !decision.should_continue {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }

        if last_screen_lock_check.elapsed()
            >= Duration::from_millis(screen_lock_check_interval_ms())
        {
            cached_screen_locked = screen_lock_monitor.is_locked();
            last_screen_lock_check = std::time::Instant::now();
        }

        // 检测屏幕锁定状态，锁屏前先回补已观察到的尾段再停止统计
        if cached_screen_locked {
            log::info!("🔒 屏幕已锁定，暂停活动统计");
            let last_observed = activity_clock.previous_sample();
            flush_observed_tail_and_reset(
                &state,
                &mut last_app_name,
                &mut last_app_window_title,
                &mut last_browser_url,
                &mut last_capture_time,
                &mut overlay_sessions,
                last_observed,
                is_currently_idle,
                "屏幕锁定",
                std::time::Instant::now(),
            );
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        let screenshot_interval = decision.screenshot_interval;

        // 按平台节拍检测活动窗口切换（定时采样间隔由 screenshot_interval 控制）
        tokio::time::sleep(Duration::from_millis(poll_interval_ms)).await;

        let last_observed = activity_clock.previous_sample();
        let Some(poll_sample) = activity_clock.sample() else {
            // 仅结算最后一次连续采样前已观察到的尾段，绝不结算随后未知的空档。
            flush_observed_tail_and_reset(
                &state,
                &mut last_app_name,
                &mut last_app_window_title,
                &mut last_browser_url,
                &mut last_capture_time,
                &mut overlay_sessions,
                last_observed,
                is_currently_idle,
                "采样中断",
                std::time::Instant::now(),
            );
            idle_detector.reset();
            is_currently_idle = false;
            cached_screen_locked = false;
            last_screen_lock_check = std::time::Instant::now()
                .checked_sub(Duration::from_millis(screen_lock_check_interval_ms()))
                .unwrap_or_else(std::time::Instant::now);
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .cached_active_window = None;
            log::debug!("电源状态变化或采样不连续，已切断活动会话");
            continue;
        };

        // 获取当前活动窗口（放入 spawn_blocking，避免 UIA/进程 Win32 调用阻塞 tokio worker）
        // 失败原因：Windows 睡眠/待机/UAC 时无前台窗口、macOS 权限不足等
        // 此时先回补已观察尾段再重置计时器，避免时长丢失或错归下一个应用
        let active_window_now = std::time::Instant::now();
        let cached_active_window = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            reusable_cached_active_window(
                state_guard.cached_active_window.as_ref(),
                active_window_now,
            )
        };
        let (mut active_window, used_full_window_lookup) =
            if let Some(window) = cached_active_window {
                // 头像循环缓存不含浏览器 URL；浏览器窗口需要走完整采集路径。
                if work_review_core::categorize::is_browser_app(&window.app_name)
                    && window.browser_url.is_none()
                {
                    match tokio::task::spawn_blocking(monitor::get_active_window).await {
                        Ok(Ok(window)) => (window, true),
                        _ => (window, false),
                    }
                } else {
                    (window, false)
                }
            } else {
                (
                    match tokio::task::spawn_blocking(monitor::get_active_window).await {
                        Ok(Ok(w)) => w,
                        _ => {
                            flush_observed_tail_and_reset(
                                &state,
                                &mut last_app_name,
                                &mut last_app_window_title,
                                &mut last_browser_url,
                                &mut last_capture_time,
                                &mut overlay_sessions,
                                last_observed,
                                is_currently_idle,
                                "窗口查询失败",
                                std::time::Instant::now(),
                            );
                            continue;
                        }
                    },
                    true,
                )
            };
        let mut browser_url_probe_attempted = browser_url_probe_attempted_after_full_lookup(
            used_full_window_lookup,
            work_review_core::categorize::is_browser_app(&active_window.app_name),
        );

        // 再次检查状态
        let should_capture = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            state_guard.is_recording && !state_guard.is_paused
        };

        if !should_capture {
            overlay_sessions.clear();
            continue;
        }

        // macOS 系统进程在用户切换应用、点击 Dock 时会短暂成为前台应用
        // 重置前先回补已观察尾段，再切断会话
        {
            if should_skip_transient_window(&active_window) {
                log::debug!("跳过系统瞬态进程: {}", active_window.app_name);
                flush_observed_tail_and_reset(
                    &state,
                    &mut last_app_name,
                    &mut last_app_window_title,
                    &mut last_browser_url,
                    &mut last_capture_time,
                    &mut overlay_sessions,
                    last_observed,
                    is_currently_idle,
                    "瞬态系统窗口",
                    std::time::Instant::now(),
                );
                continue;
            }
        }

        // 跳过系统 shell / 锁屏 / 桌面进程，避免睡眠/唤醒时累积虚假时长
        // 注意 explorer 特殊处理：有窗口标题时是文件管理器，应该记录
        {
            if should_skip_system_window(&active_window) {
                log::debug!(
                    "跳过系统/桌面窗口: {} (title={}, minimized={})",
                    active_window.app_name,
                    active_window.window_title,
                    active_window.is_minimized
                );
                flush_observed_tail_and_reset(
                    &state,
                    &mut last_app_name,
                    &mut last_app_window_title,
                    &mut last_browser_url,
                    &mut last_capture_time,
                    &mut overlay_sessions,
                    last_observed,
                    is_currently_idle,
                    "系统桌面窗口",
                    std::time::Instant::now(),
                );
                continue;
            }
        }

        let should_probe_browser_url = should_probe_browser_url_before_change_detection(
            &active_window.app_name,
            &active_window.window_title,
            last_app_name.as_deref(),
            last_app_window_title.as_deref(),
            active_window.browser_url.as_deref(),
        );
        if claim_browser_url_probe(&mut browser_url_probe_attempted, should_probe_browser_url) {
            let app_for_probe = active_window.app_name.clone();
            let title_for_probe = active_window.window_title.clone();
            if let Ok(Some(resolved_url)) = tokio::task::spawn_blocking(move || {
                monitor::resolve_browser_url_for_window(&app_for_probe, &title_for_probe)
            })
            .await
            {
                if last_browser_url.as_deref() != Some(resolved_url.as_str()) {
                    log::debug!(
                        "浏览器 URL 预探测命中: {} | {} -> {}",
                        active_window.app_name,
                        active_window.window_title,
                        resolved_url
                    );
                }
                active_window.browser_url = Some(resolved_url);
            }
        }

        // 浏览器 URL 存在瞬时采集失败时，尽量复用同窗口最近一次成功值，减少统计断裂。
        const BROWSER_URL_STICKY_GAP_SECS: i64 = 120;
        if active_window.browser_url.is_none()
            && work_review_core::categorize::is_browser_app(&active_window.app_name)
            && !active_window.window_title.is_empty()
        {
            let now_ts = chrono::Local::now().timestamp();

            let recovered_url = if last_app_name.as_deref() == Some(active_window.app_name.as_str())
                && last_app_window_title.as_deref() == Some(active_window.window_title.as_str())
            {
                last_browser_url.clone()
            } else {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                recover_recent_browser_url(
                    &state_guard.database,
                    &active_window.app_name,
                    &active_window.window_title,
                    now_ts,
                    BROWSER_URL_STICKY_GAP_SECS,
                )
            };

            if let Some(recovered_url) = recovered_url {
                log::debug!(
                    "恢复浏览器 URL: {} | {} -> {}",
                    active_window.app_name,
                    active_window.window_title,
                    recovered_url
                );
                active_window.browser_url = Some(recovered_url);
            }
        }

        // ===== 检测应用切换 =====
        let previous_window_title = last_app_window_title.clone();
        let previous_browser_url = last_browser_url.clone();

        let mut url_changed = match (&last_browser_url, &active_window.browser_url) {
            (Some(l), Some(r)) => l != r,
            (None, None) => false,
            _ => true,
        };

        // 只有当两个标题不同时才算切换
        let title_changed = match (&last_app_window_title, &active_window.window_title) {
            (Some(last_title), active_title) => last_title != active_title,
            (None, _) => true,
        };

        let mut app_changed = match &last_app_name {
            Some(last) => last != &active_window.app_name || url_changed || title_changed,
            None => true,
        };
        let capture_min_interval_ms = browser_change_capture_min_interval_ms(
            &active_window.app_name,
            title_changed,
            url_changed,
        );

        // 窗口/URL 查询也可能跨越待机；使用同一采样时刻计算时长与落库终点。
        let accounting_sample = activity_clock::Sample::now();
        if !accounting_sample.follows(poll_sample) {
            continue;
        }
        let elapsed_since_capture = accounting_sample
            .instant
            .saturating_duration_since(last_capture_time);
        let elapsed_secs = elapsed_since_capture.as_secs();

        // ===== 应用切换日志 =====
        if app_changed && last_app_name.is_some() {
            log::info!(
                "📊 应用切换: {} [{}] → {} [{}]",
                last_app_name.as_deref().unwrap_or("无"),
                previous_window_title.as_deref().unwrap_or(""),
                active_window.app_name,
                active_window.window_title,
            );
        }

        // ===== 空闲检测第一阶段：键鼠活动检查 =====
        let input_idle_seconds = idle_detector.get_idle_seconds();
        let input_idle = input_idle_seconds >= idle_threshold_minutes * 60;

        let was_input_idle = is_currently_idle;
        // 每 30 秒打印一次空闲状态日志（避免刷屏）；状态切换本身每轮都要处理
        let should_log_idle = last_idle_log_time.elapsed() >= Duration::from_secs(30);
        if input_idle != is_currently_idle {
            if input_idle {
                if should_log_idle {
                    log::info!("⏸️  键鼠超时，等待截图确认空闲状态...");
                }
            } else {
                if should_log_idle {
                    log::info!("▶️  检测到用户活动，恢复正常记录");
                }
                idle_detector.reset();
            }
        }
        if should_log_idle {
            last_idle_log_time = std::time::Instant::now();
        }
        is_currently_idle = input_idle;

        // ===== 判断是否截图 =====
        // 1. 定时触发：到达配置的间隔时间（键鼠空闲期间放宽采样周期以降低 CPU/磁盘消耗）
        // 2. 应用切换触发：满足最小间隔
        let effective_interval =
            effective_screenshot_interval_secs(screenshot_interval, input_idle);
        let should_take_screenshot = if elapsed_secs >= effective_interval {
            log::debug!("定时截图触发");
            true
        } else if app_changed && elapsed_since_capture.as_millis() >= capture_min_interval_ms {
            if capture_min_interval_ms < MIN_CAPTURE_INTERVAL_MS {
                log::debug!("浏览器导航截图触发");
            } else {
                log::debug!("应用切换截图触发");
            }
            true
        } else {
            false
        };

        // 实时刷新进行中的会话快照，供进程退出时精确落盘尾段时长
        update_active_recording_session_snapshot(Some(ActiveRecordingSession {
            app_name: last_app_name
                .clone()
                .unwrap_or_else(|| active_window.app_name.clone()),
            window_title: last_app_window_title
                .clone()
                .unwrap_or_else(|| active_window.window_title.clone()),
            browser_url: last_browser_url
                .clone()
                .or_else(|| active_window.browser_url.clone()),
            last_capture_time,
            last_observed_instant: accounting_sample.instant,
            last_observed_timestamp: accounting_sample.timestamp(),
            is_idle: input_idle,
        }));

        // 保存 app_name 副本供浮动窗口检测使用（在 move 之前）
        let frontmost_app_name = active_window.app_name.clone();

        if !should_take_screenshot {
            // 如果是因为冷却时间未到而没有截图，但应用/标签页实际上已经变化了
            // 那么我们不要更新 last_* 变量，这样下一个轮询周期 app_changed 仍然为 true
            if !app_changed {
                last_app_name = Some(active_window.app_name.clone());
                last_app_window_title = Some(active_window.window_title.clone());
                last_browser_url = active_window.browser_url.clone();
            }
            continue;
        }

        let should_refresh_browser_url = should_refresh_browser_url_before_record(
            &active_window.app_name,
            &active_window.window_title,
        );
        if claim_browser_url_probe(&mut browser_url_probe_attempted, should_refresh_browser_url) {
            let app_for_refresh = active_window.app_name.clone();
            let title_for_refresh = active_window.window_title.clone();
            if let Ok(Some(resolved_url)) = tokio::task::spawn_blocking(move || {
                monitor::resolve_browser_url_for_window(&app_for_refresh, &title_for_refresh)
            })
            .await
            {
                if active_window.browser_url.as_deref() != Some(resolved_url.as_str()) {
                    log::debug!(
                        "浏览器 URL 落库前刷新: {} | {} -> {}",
                        active_window.app_name,
                        active_window.window_title,
                        resolved_url
                    );
                }
                active_window.browser_url = Some(resolved_url);
                url_changed = match (&last_browser_url, &active_window.browser_url) {
                    (Some(l), Some(r)) => l != r,
                    (None, None) => false,
                    _ => true,
                };
                app_changed = match &last_app_name {
                    Some(last) => last != &active_window.app_name || url_changed || title_changed,
                    None => true,
                };
            }
        }

        // 保存切换前的应用名，用于时长归属修正
        let previous_app_name = if app_changed {
            last_app_name.clone()
        } else {
            None
        };

        // 取决定截图后，才更新上一个应用的信息
        last_app_name = Some(active_window.app_name.clone());
        last_app_window_title = Some(active_window.window_title.clone());
        last_browser_url = active_window.browser_url.clone();

        // 更新截图时间
        last_capture_time = accounting_sample.instant;

        // 使用距离上次截图的实际经过时间作为本次记录的时长
        // 而非固定的轮询间隔，避免截图间隔大于轮询间隔时丢失时长
        let (privacy_action, duration_to_record) = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            let action = state_guard.privacy_filter.check_privacy_with_private(
                &active_window.app_name,
                &active_window.window_title,
                active_window.browser_url.as_deref(),
                active_window.is_private,
            );
            // 窗口标题与 OCR 文本同等过滤后再向下游传递(入库/事件/分类)：
            // 邮件主题、IM 会话名里常出现邮箱/手机号等敏感信息,此前只过滤 OCR 文本。
            // 隐私判定(上方)与逐 tick 变化检测(last_* 变量)仍使用原始标题。
            active_window.window_title = state_guard
                .privacy_filter
                .filter_text(&active_window.window_title);
            // elapsed_secs 是距离上次截图的真实秒数，确保时长不丢失
            let duration = elapsed_secs.max(1) as i64;
            (action, duration)
        };
        // 锁已释放

        let current_timestamp = accounting_sample.timestamp();
        let previous_activity_to_backfill = if app_changed {
            resolve_previous_activity_to_backfill(
                &state,
                previous_app_name.as_deref(),
                previous_browser_url.as_deref(),
                previous_window_title.as_deref(),
            )
        } else {
            None
        };
        let adjusted_duration = if app_changed {
            0i64
        } else {
            duration_to_record
        };

        use work_review_core::privacy::PrivacyAction;
        let result: Option<work_review_core::database::Activity> = match privacy_action {
            PrivacyAction::Skip => {
                log::debug!(
                    "完全跳过: {} - {}",
                    active_window.app_name,
                    active_window.window_title
                );
                // 切到忽略应用不应吞掉上一应用的最后一段经过时间：
                // 与 Anonymize/Record 路径一致,先回补给上一活动再跳过本条
                let skip_is_confirmed_idle =
                    should_confirm_idle(input_idle, input_idle_seconds, false, false);
                let previous_effective_duration = previous_app_backfill_duration(
                    app_changed,
                    duration_to_record,
                    was_input_idle,
                    skip_is_confirmed_idle,
                );
                backfill_previous_activity_if_needed(
                    &state,
                    previous_activity_to_backfill.as_ref(),
                    previous_app_name.as_deref(),
                    previous_window_title.as_deref(),
                    previous_browser_url.as_deref(),
                    previous_effective_duration,
                    current_timestamp,
                    &active_window.app_name,
                );
                None
            }
            PrivacyAction::Anonymize => {
                log::debug!(
                    "内容脱敏: {} - {}",
                    active_window.app_name,
                    active_window.window_title
                );
                let classification = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    crate::resolve_activity_classification(
                        &state_guard.config,
                        &active_window.app_name,
                        &active_window.window_title,
                        active_window.browser_url.as_deref(),
                    )
                };
                let anonymized_is_confirmed_idle =
                    should_confirm_idle(input_idle, input_idle_seconds, false, false);
                let previous_effective_duration = previous_app_backfill_duration(
                    app_changed,
                    duration_to_record,
                    was_input_idle,
                    anonymized_is_confirmed_idle,
                );
                let previous_backfill_succeeded = backfill_previous_activity_if_needed(
                    &state,
                    previous_activity_to_backfill.as_ref(),
                    previous_app_name.as_deref(),
                    previous_window_title.as_deref(),
                    previous_browser_url.as_deref(),
                    previous_effective_duration,
                    current_timestamp,
                    &active_window.app_name,
                );
                let effective_duration = if anonymized_is_confirmed_idle {
                    log::debug!("空闲确认: 脱敏活动跳过时长记录");
                    0
                } else if app_changed
                    && previous_effective_duration > 0
                    && !previous_backfill_succeeded
                {
                    log::warn!(
                        "上一应用回补失败，改为把切换间隔记入脱敏活动 {}",
                        active_window.app_name
                    );
                    duration_to_record
                } else {
                    adjusted_duration
                };

                if effective_duration <= 0 && !app_changed {
                    None
                } else {
                    let activity = work_review_core::database::Activity {
                        id: None,
                        timestamp: current_timestamp,
                        app_name: active_window.app_name,
                        window_title: "[内容已脱敏]".to_string(),
                        screenshot_path: String::new(),
                        ocr_text: None,
                        category: classification.base_category,
                        duration: effective_duration,
                        browser_url: None,
                        executable_path: active_window.executable_path,
                        semantic_category: Some(classification.semantic_category),
                        semantic_confidence: Some(i32::from(classification.confidence)),
                        screenshot_url: None,
                    };

                    // 短暂获取锁写入数据库
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    match state_guard.database.insert_activity(&activity) {
                        Ok(_) => Some(activity),
                        Err(e) => {
                            log::error!("保存活动记录失败: {e}");
                            None
                        }
                    }
                }
            }
            PrivacyAction::Record => {
                let (classification, screenshots_enabled) = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        crate::resolve_activity_classification(
                            &state_guard.config,
                            &active_window.app_name,
                            &active_window.window_title,
                            active_window.browser_url.as_deref(),
                        ),
                        state_guard.config.storage.screenshots_enabled,
                    )
                };
                let category = classification.base_category.clone();

                // 先检查是否有可合并的记录（在截屏之前判断，避免不必要的截图保存）
                let latest_activity = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(url) = active_window
                        .browser_url
                        .as_deref()
                        .filter(|url| !url.is_empty())
                    {
                        state_guard
                            .database
                            .get_latest_activity_by_url(url)
                            .ok()
                            .flatten()
                    } else if work_review_core::categorize::is_browser_app(&active_window.app_name)
                        && !active_window.window_title.is_empty()
                    {
                        state_guard
                            .database
                            .get_latest_activity_by_app_title(
                                &active_window.app_name,
                                &active_window.window_title,
                            )
                            .ok()
                            .flatten()
                    } else {
                        state_guard
                            .database
                            .get_latest_activity_by_app(&active_window.app_name)
                            .ok()
                            .flatten()
                    }
                };

                // 只合并当前连续会话。切回此前使用过的应用/页面必须新建记录，
                // 否则累计 duration 配合新 timestamp 会把中间会话覆盖成重叠区间。
                let is_merge = if let Some(ref latest) = latest_activity {
                    let mut merge = should_merge_contiguous_activity(
                        app_changed,
                        &active_window.app_name,
                        current_timestamp,
                        latest.timestamp,
                    );

                    // 如果由于某种原因 browser_url 获取失败，但它确实是一个浏览器
                    // 我们必须更严格地判断合并条件，否则不同标签页的切换会被错误合并。
                    if merge
                        && active_window.browser_url.is_none()
                        && work_review_core::categorize::is_browser_app(&active_window.app_name)
                        && (latest.window_title != active_window.window_title
                            || latest.browser_url.is_some())
                    {
                        merge = false;
                    }

                    merge
                } else {
                    false
                };

                // 新建与合并共用 OCR 冷却，窗口切换仍保留截图。
                // ponytail: 冷却期内的短活动可能没有 OCR；需要逐条索引时再加有界后台队列。
                let should_run_ocr = screenshots_enabled
                    && should_run_ocr(
                        last_ocr_time.map(|last| last.elapsed()),
                        screenshot_interval,
                    );

                if is_merge {
                    // === 合并路径：内存中先算 dHash 判重复，不写删归档 JPEG；仅画面变化且需 OCR 时写 PNG ===
                    let latest = latest_activity.unwrap();
                    let latest_id = match latest.id {
                        Some(id) => id,
                        None => {
                            log::error!("合并活动记录缺少 id，跳过");
                            continue;
                        }
                    };
                    let previous_screenshot_path = latest.screenshot_path.clone();

                    // 只有开启截图且（需要做空闲画面确认 OR 需要 OCR）时才抓屏
                    let last_merge_hash =
                        merge_screenshot_hash.load(std::sync::atomic::Ordering::Relaxed);
                    let merge_capture = if screenshots_enabled && (input_idle || should_run_ocr) {
                        capture_merge_ocr_for_window(
                            &state,
                            &active_window,
                            accounting_sample,
                            last_merge_hash,
                            should_run_ocr,
                        )
                        .await
                        .ok()
                    } else {
                        None
                    };

                    // ===== 空闲检测第二阶段：复用内存 dHash（零磁盘重解码） =====
                    let screenshot_idle = if input_idle {
                        if let Some(ref captured) = merge_capture {
                            idle_detector.confirm_idle_with_hash(captured.image_hash)
                        } else {
                            false
                        }
                    } else {
                        idle_detector.reset();
                        false
                    };
                    if input_idle && screenshot_idle && screenshots_enabled {
                        log::debug!(
                            "键鼠超时且画面静止，但未到硬超时，保留时长等待键鼠恢复 (idle_secs={input_idle_seconds})"
                        );
                    }
                    let is_confirmed_idle = should_confirm_idle(
                        input_idle,
                        input_idle_seconds,
                        screenshots_enabled,
                        screenshot_idle,
                    );
                    let previous_effective_duration = previous_app_backfill_duration(
                        app_changed,
                        duration_to_record,
                        was_input_idle,
                        is_confirmed_idle,
                    );
                    backfill_previous_activity_if_needed(
                        &state,
                        previous_activity_to_backfill.as_ref(),
                        previous_app_name.as_deref(),
                        previous_window_title.as_deref(),
                        previous_browser_url.as_deref(),
                        previous_effective_duration,
                        current_timestamp,
                        &active_window.app_name,
                    );

                    let effective_duration = if is_confirmed_idle {
                        log::debug!("空闲确认: 跳过本次时长记录");
                        0
                    } else {
                        adjusted_duration
                    };

                    let persisted_screenshot_path = previous_screenshot_path.clone();
                    let mut persisted_duration = latest.duration;

                    if should_persist_merge_update(effective_duration) {
                        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                        match state_guard.database.merge_activity(
                            latest_id,
                            effective_duration,
                            None,
                            &previous_screenshot_path,
                            current_timestamp,
                            active_window.browser_url.as_deref(),
                        ) {
                            Ok(_) => {
                                persisted_duration += effective_duration;
                                log::info!(
                                    "✅ 合并成功: {} (id={}, 新时长={}s)",
                                    active_window.app_name,
                                    latest_id,
                                    latest.duration + effective_duration
                                );
                            }
                            Err(e) => {
                                log::error!("合并活动记录失败: {e}");
                            }
                        }
                    }

                    // 仅在内存 dHash 判定画面确实变化并写出 OCR 源图时，才触发异步 OCR
                    if let Some(captured) = merge_capture {
                        merge_screenshot_hash
                            .store(captured.image_hash, std::sync::atomic::Ordering::Relaxed);
                        if let Some(ocr_temp_path) = captured.ocr_source_path {
                            last_ocr_time = Some(std::time::Instant::now());
                            let state_clone = state.clone();
                            let data_dir_clone = {
                                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                                state_guard.data_dir.clone()
                            };
                            let ocr_sem = ocr_semaphore.clone();

                            tokio::spawn(async move {
                                let _permit = match ocr_sem.try_acquire_owned() {
                                    Ok(p) => p,
                                    Err(_) => {
                                        log::debug!("OCR 并发已满，跳过合并路径 OCR");
                                        let _ = std::fs::remove_file(&ocr_temp_path);
                                        return;
                                    }
                                };

                                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                                let data_dir_for_ocr = data_dir_clone.clone();
                                let ocr_source_path = ocr_temp_path.clone();
                                let ocr_outcome = tokio::task::spawn_blocking(move || {
                                    let ocr_service = ocr::OcrService::new(&data_dir_for_ocr);
                                    ocr_service.extract_text(&ocr_source_path)
                                })
                                .await;
                                match ocr_outcome {
                                    Ok(Ok(Some(ocr_result))) => {
                                        if !ocr_result.text.is_empty() {
                                            if let Ok(state_guard) = state_clone.lock() {
                                                let filtered_text = ocr::redact_ocr_for_storage(
                                                    &ocr_result.text,
                                                    &state_guard.privacy_filter,
                                                );
                                                if let Err(error) =
                                                    state_guard.database.update_activity_ocr(
                                                        latest_id,
                                                        Some(filtered_text),
                                                        &state_guard.config.website_semantic_rules,
                                                    )
                                                {
                                                    log::warn!(
                                                        "保存 OCR 与网站分类失败(合并): {error}"
                                                    );
                                                }
                                                log::info!(
                                                    "OCR 完成(合并): 活动 {} 识别到 {} 个字符",
                                                    latest_id,
                                                    ocr_result.text.len()
                                                );
                                            }
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        log::warn!("OCR 后台任务执行失败(合并): {e}");
                                    }
                                }

                                let _ = std::fs::remove_file(&ocr_temp_path);
                            });
                        }
                    }

                    Some(work_review_core::database::Activity {
                        id: Some(latest_id),
                        timestamp: current_timestamp,
                        app_name: active_window.app_name.clone(),
                        window_title: active_window.window_title,
                        screenshot_path: persisted_screenshot_path,
                        ocr_text: None,
                        category,
                        duration: persisted_duration,
                        browser_url: active_window.browser_url,
                        executable_path: active_window.executable_path,
                        semantic_category: Some(classification.semantic_category.clone()),
                        semantic_confidence: Some(i32::from(classification.confidence)),
                        screenshot_url: None,
                    })
                } else {
                    // === 新建路径：正常截屏并保存（仅在需要 OCR 时编码 PNG，内存复用 dHash） ===
                    if screenshots_enabled {
                        let screenshot_result = capture_screenshot_for_window_with_options(
                            &state,
                            &active_window,
                            accounting_sample,
                            should_run_ocr,
                        )
                        .await;

                        match screenshot_result {
                            Ok(screenshot_result) => {
                                // ===== 空闲检测第二阶段：直接复用采集阶段计算的 dHash =====
                                let screenshot_idle = if input_idle {
                                    let hash = screenshot_result.image_hash.unwrap_or_else(|| {
                                        screenshot::ScreenshotService::calculate_image_hash(
                                            &screenshot_result.path,
                                        )
                                        .unwrap_or(0)
                                    });
                                    idle_detector.confirm_idle_with_hash(hash)
                                } else {
                                    idle_detector.reset();
                                    false
                                };
                                let is_confirmed_idle = should_confirm_idle(
                                    input_idle,
                                    input_idle_seconds,
                                    screenshots_enabled,
                                    screenshot_idle,
                                );
                                let previous_effective_duration = previous_app_backfill_duration(
                                    app_changed,
                                    duration_to_record,
                                    was_input_idle,
                                    is_confirmed_idle,
                                );
                                let previous_backfill_succeeded =
                                    backfill_previous_activity_if_needed(
                                        &state,
                                        previous_activity_to_backfill.as_ref(),
                                        previous_app_name.as_deref(),
                                        previous_window_title.as_deref(),
                                        previous_browser_url.as_deref(),
                                        previous_effective_duration,
                                        current_timestamp,
                                        &active_window.app_name,
                                    );

                                // 如果确认空闲，跳过时长记录（但仍创建活动记录以保持截图）
                                let effective_duration = if is_confirmed_idle {
                                    log::debug!("空闲确认: 新活动时长设为 0");
                                    0
                                } else if app_changed
                                    && previous_effective_duration > 0
                                    && !previous_backfill_succeeded
                                {
                                    log::warn!(
                                        "上一应用回补失败，改为把切换间隔记入当前活动 {}",
                                        active_window.app_name
                                    );
                                    duration_to_record
                                } else {
                                    adjusted_duration
                                };

                                let (
                                    relative_path,
                                    archive_path,
                                    ocr_input_path,
                                    temporary_ocr_source_path,
                                    data_dir_clone,
                                ) = {
                                    let state_guard =
                                        state.lock().unwrap_or_else(|e| e.into_inner());
                                    (
                                        state_guard
                                            .screenshot_service
                                            .get_relative_path(&screenshot_result.path),
                                        screenshot_result.path.clone(),
                                        screenshot_result
                                            .ocr_source_path
                                            .clone()
                                            .unwrap_or_else(|| screenshot_result.path.clone()),
                                        screenshot_result
                                            .ocr_source_path
                                            .clone()
                                            .filter(|path| path != &screenshot_result.path),
                                        state_guard.data_dir.clone(),
                                    )
                                };

                                let activity = work_review_core::database::Activity {
                                    id: None,
                                    timestamp: current_timestamp,
                                    app_name: active_window.app_name.clone(),
                                    window_title: active_window.window_title,
                                    screenshot_path: relative_path.clone(),
                                    ocr_text: None,
                                    category,
                                    duration: effective_duration,
                                    browser_url: active_window.browser_url,
                                    executable_path: active_window.executable_path,
                                    semantic_category: Some(
                                        classification.semantic_category.clone(),
                                    ),
                                    semantic_confidence: Some(i32::from(classification.confidence)),
                                    screenshot_url: None,
                                };

                                let inserted = {
                                    let state_guard =
                                        state.lock().unwrap_or_else(|e| e.into_inner());
                                    state_guard.database.insert_activity(&activity)
                                };

                                match inserted {
                                    Ok(activity_id) => {
                                        log::info!(
                                            "📝 新建活动: {} (id={})",
                                            active_window.app_name,
                                            activity_id
                                        );

                                        // 异步 OCR（新建活动的截图已保存，不删除）
                                        if should_run_ocr {
                                            last_ocr_time = Some(std::time::Instant::now());
                                        }
                                        let state_clone = state.clone();
                                        let ocr_sem = ocr_semaphore.clone();
                                        tokio::spawn(async move {
                                            if !should_run_ocr {
                                                if let Some(temp_path) = temporary_ocr_source_path {
                                                    let _ = std::fs::remove_file(temp_path);
                                                }
                                                return;
                                            }

                                            // 非阻塞获取 permit，满载时跳过 OCR
                                            let _permit = match ocr_sem.try_acquire_owned() {
                                                Ok(p) => p,
                                                Err(_) => {
                                                    log::debug!("OCR 并发已满，跳过新建路径 OCR");
                                                    if let Some(temp_path) =
                                                        temporary_ocr_source_path.clone()
                                                    {
                                                        let _ = std::fs::remove_file(&temp_path);
                                                    }
                                                    return;
                                                }
                                            };

                                            tokio::time::sleep(tokio::time::Duration::from_secs(1))
                                                .await;

                                            // OCR 是阻塞调用，放入 spawn_blocking 避免阻塞异步运行时
                                            let data_dir_for_ocr = data_dir_clone.clone();
                                            let ocr_source_path = ocr_input_path.clone();
                                            let ocr_outcome =
                                                tokio::task::spawn_blocking(move || {
                                                    let ocr_service =
                                                        ocr::OcrService::new(&data_dir_for_ocr);
                                                    ocr_service.extract_text(&ocr_source_path)
                                                })
                                                .await;

                                            match ocr_outcome {
                                                Ok(Ok(Some(ocr_result))) => {
                                                    if !ocr_result.text.is_empty() {
                                                        if let Ok(state_guard) = state_clone.lock()
                                                        {
                                                            let filtered_text =
                                                                ocr::redact_ocr_for_storage(
                                                                    &ocr_result.text,
                                                                    &state_guard.privacy_filter,
                                                                );
                                                            if let Err(error) = state_guard
                                                                .database
                                                                .update_activity_ocr(
                                                                    activity_id,
                                                                    Some(filtered_text),
                                                                    &state_guard
                                                                        .config
                                                                        .website_semantic_rules,
                                                                )
                                                            {
                                                                log::warn!("保存 OCR 与网站分类失败(新建): {error}");
                                                            }
                                                            log::info!(
                                                        "OCR 完成(新建): 活动 {} 识别到 {} 个字符",
                                                        activity_id,
                                                        ocr_result.text.len()
                                                    );
                                                        }
                                                    }
                                                }
                                                Ok(_) => {}
                                                Err(e) => {
                                                    log::warn!("OCR 后台任务执行失败(新建): {e}");
                                                }
                                            }

                                            if let Some(temp_path) = temporary_ocr_source_path {
                                                let _ = std::fs::remove_file(&temp_path);
                                            }
                                        });

                                        // 异步远程上传截图
                                        {
                                            let remote_cfg = {
                                                let g =
                                                    state.lock().unwrap_or_else(|e| e.into_inner());
                                                g.config.remote_storage.clone()
                                            };
                                            if remote_cfg.provider != work_review_core::config::RemoteStorageProvider::None {
                                                let st = state.clone();
                                                let ap = archive_path.clone();
                                                let rp = relative_path.clone();
                                                tokio::spawn(async move {
                                                    match remote_upload::upload_screenshot(&remote_cfg, &ap, &rp).await {
                                                        Ok(url) => {
                                                            log::info!("远程上传成功: {url}");
                                                            if let Ok(g) = st.lock() {
                                                                let _ = g.database.update_activity_screenshot_url(activity_id, &url);
                                                            }
                                                        }
                                                        Err(e) => log::warn!("远程上传失败: {e}"),
                                                    }
                                                });
                                            }
                                        }

                                        Some(work_review_core::database::Activity {
                                            id: Some(activity_id),
                                            ..activity
                                        })
                                    }
                                    Err(e) => {
                                        log::error!("保存活动记录失败: {e}");
                                        let _ = std::fs::remove_file(&archive_path);
                                        if let Some(temp_path) = temporary_ocr_source_path {
                                            let _ = std::fs::remove_file(&temp_path);
                                        }
                                        None
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("截屏失败: {e}");
                                None
                            }
                        }
                    } else {
                        let is_confirmed_idle = should_confirm_idle(
                            input_idle,
                            input_idle_seconds,
                            screenshots_enabled,
                            false,
                        );
                        let previous_effective_duration = previous_app_backfill_duration(
                            app_changed,
                            duration_to_record,
                            was_input_idle,
                            is_confirmed_idle,
                        );
                        let previous_backfill_succeeded = backfill_previous_activity_if_needed(
                            &state,
                            previous_activity_to_backfill.as_ref(),
                            previous_app_name.as_deref(),
                            previous_window_title.as_deref(),
                            previous_browser_url.as_deref(),
                            previous_effective_duration,
                            current_timestamp,
                            &active_window.app_name,
                        );
                        let effective_duration = if is_confirmed_idle {
                            log::debug!("关闭截图后按输入空闲判定，新活动时长设为 0");
                            0
                        } else if app_changed
                            && previous_effective_duration > 0
                            && !previous_backfill_succeeded
                        {
                            log::warn!(
                                "上一应用回补失败，改为把切换间隔记入当前活动 {}",
                                active_window.app_name
                            );
                            duration_to_record
                        } else {
                            adjusted_duration
                        };

                        let activity = work_review_core::database::Activity {
                            id: None,
                            timestamp: current_timestamp,
                            app_name: active_window.app_name.clone(),
                            window_title: active_window.window_title,
                            screenshot_path: String::new(),
                            ocr_text: None,
                            category,
                            duration: effective_duration,
                            browser_url: active_window.browser_url,
                            executable_path: active_window.executable_path,
                            semantic_category: Some(classification.semantic_category.clone()),
                            semantic_confidence: Some(i32::from(classification.confidence)),
                            screenshot_url: None,
                        };

                        let inserted = {
                            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            state_guard.database.insert_activity(&activity)
                        };

                        match inserted {
                            Ok(activity_id) => {
                                log::info!(
                                    "📝 新建无截图活动: {} (id={})",
                                    active_window.app_name,
                                    activity_id
                                );
                                Some(work_review_core::database::Activity {
                                    id: Some(activity_id),
                                    ..activity
                                })
                            }
                            Err(e) => {
                                log::error!("保存无截图活动记录失败: {e}");
                                None
                            }
                        }
                    }
                }
            }
        };

        // 发送事件到前端
        if let Some(activity) = result {
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                let _ = window.emit("screenshot-taken", &activity);
            }
        }

        if !activity_clock::Sample::now().follows(accounting_sample) {
            overlay_sessions.clear();
            continue;
        }

        // ===== 浮动窗口（PiP 画中画）检测 =====
        // 检测 layer > 0 的浮动窗口（如视频小窗），为它们记录使用时长
        // 浮动窗口不截图（截图已由主活动管理），仅记录时长
        // ponytail: 按截图周期采样，短于一个周期的浮窗不估算时长；需要精确计时再独立高频采样。
        let overlay_windows = monitor::get_overlay_windows(&frontmost_app_name);
        let observed_at = std::time::Instant::now();
        let mut next_overlay_sessions = std::collections::HashMap::new();
        for ow in &overlay_windows {
            // 同一应用可能有多个不相邻的浮窗，每轮只能计时一次。
            if next_overlay_sessions.contains_key(&ow.app_name) {
                continue;
            }
            // 隐私检查
            let ow_privacy = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                state_guard
                    .privacy_filter
                    .check_privacy(&ow.app_name, &ow.window_title)
            };

            if ow_privacy == work_review_core::privacy::PrivacyAction::Skip {
                log::debug!("浮动窗口跳过(隐私): {}", ow.app_name);
                continue;
            }

            let overlay_is_confirmed_idle =
                should_confirm_idle(input_idle, input_idle_seconds, false, false);
            if overlay_is_confirmed_idle {
                log::debug!("浮动窗口空闲确认，跳过时长记录: {}", ow.app_name);
                continue;
            }

            let classification = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                crate::resolve_activity_classification(
                    &state_guard.config,
                    &ow.app_name,
                    &ow.window_title,
                    ow.browser_url.as_deref(),
                )
            };
            let ow_category = classification.base_category.clone();
            let current_ts = chrono::Local::now().timestamp();

            // 查找该应用的最近活动记录，尝试合并
            let latest = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                state_guard
                    .database
                    .get_latest_activity_by_app(&ow.app_name)
                    .ok()
                    .flatten()
            };

            let increment = latest.as_ref().and_then(|activity| {
                overlay_session_increment(
                    overlay_sessions.get(&ow.app_name).copied(),
                    activity,
                    observed_at,
                    current_ts,
                )
            });

            if let Some((ow_duration, accounted_at)) = increment {
                let act = latest.unwrap();
                if let Some(act_id) = act.id {
                    if ow_duration == 0 {
                        next_overlay_sessions.insert(ow.app_name.clone(), (act_id, accounted_at));
                        continue;
                    }
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    match state_guard.database.merge_activity(
                        act_id,
                        ow_duration,
                        None,
                        &act.screenshot_path,
                        current_ts,
                        None,
                    ) {
                        Ok(_) => {
                            next_overlay_sessions
                                .insert(ow.app_name.clone(), (act_id, accounted_at));
                            log::info!(
                                "🪟 浮动窗口合并: {} (id={}, +{}s, 总{}s)",
                                ow.app_name,
                                act_id,
                                ow_duration,
                                act.duration + ow_duration
                            );
                        }
                        Err(e) => log::error!("浮动窗口合并失败: {e}"),
                    }
                }
            } else {
                // 首次观察不能反推浮窗何时出现，先以 0 秒创建连续会话。
                let ow_duration = 0;
                let ow_title = if ow_privacy == work_review_core::privacy::PrivacyAction::Anonymize
                {
                    "[内容已脱敏]".to_string()
                } else {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    state_guard.privacy_filter.filter_text(&ow.window_title)
                };

                let activity = work_review_core::database::Activity {
                    id: None,
                    timestamp: current_ts,
                    app_name: ow.app_name.clone(),
                    window_title: ow_title,
                    screenshot_path: String::new(),
                    ocr_text: None,
                    category: ow_category,
                    duration: ow_duration,
                    browser_url: None,
                    executable_path: ow.executable_path.clone(),
                    semantic_category: Some(classification.semantic_category),
                    semantic_confidence: Some(i32::from(classification.confidence)),
                    screenshot_url: None,
                };

                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                match state_guard.database.insert_activity(&activity) {
                    Ok(id) => {
                        next_overlay_sessions.insert(ow.app_name.clone(), (id, observed_at));
                        log::info!(
                            "🪟 浮动窗口新建: {} (id={}, {}s)",
                            ow.app_name,
                            id,
                            ow_duration
                        );
                    }
                    Err(e) => log::error!("浮动窗口记录失败: {e}"),
                }
            }
        }
        overlay_sessions = next_overlay_sessions;
    }
}
