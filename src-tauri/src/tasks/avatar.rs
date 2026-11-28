//! 桌宠状态监控、切换防抖、主动 AI 提醒与久坐提醒后台任务。

use crate::break_reminder::*;
use crate::window::{should_skip_system_window, should_skip_transient_window};
use crate::{avatar_engine, avatar_proactive, idle_detector, monitor, screen_lock, AppState};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;

const FOLLOWUP_SCAN_MIN_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn avatar_monitor_poll_interval_ms_for_platform(is_macos: bool, active: bool) -> u64 {
    if is_macos {
        if active {
            750
        } else {
            2000
        }
    } else if active {
        if cfg!(target_os = "linux") {
            1000
        } else {
            180
        }
    } else {
        750
    }
}

#[allow(dead_code)]
pub(crate) fn avatar_monitor_poll_interval_ms() -> u64 {
    avatar_monitor_poll_interval_ms_for_platform(cfg!(target_os = "macos"), true)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AvatarActivityDecision {
    pub(crate) should_continue: bool,
    pub(crate) reset_state: Option<avatar_engine::AvatarStatePayload>,
}

pub(crate) fn avatar_activity_decision(
    avatar_enabled: bool,
    is_recording: bool,
    is_paused: bool,
    avatar_opacity: f64,
    avatar_preset: &str,
    avatar_persona: &str,
    avatar_body_hidden: bool,
) -> AvatarActivityDecision {
    if !avatar_enabled {
        return AvatarActivityDecision {
            should_continue: false,
            reset_state: Some(avatar_engine::apply_avatar_visual_settings(
                avatar_engine::default_avatar_state(),
                avatar_opacity,
                avatar_preset,
                avatar_persona,
                avatar_body_hidden,
            )),
        };
    }

    if !is_recording || is_paused {
        return AvatarActivityDecision {
            should_continue: false,
            reset_state: Some(avatar_engine::apply_avatar_visual_settings(
                avatar_engine::default_avatar_state(),
                avatar_opacity,
                avatar_preset,
                avatar_persona,
                avatar_body_hidden,
            )),
        };
    }

    AvatarActivityDecision {
        should_continue: true,
        reset_state: None,
    }
}

pub(crate) fn avatar_proactive_ai_should_run(
    avatar_enabled: bool,
    avatar_proactive_ai_enabled: bool,
    is_paused: bool,
    text_model: &work_review_core::config::ModelConfig,
    now_ms: u64,
    next_check_ms: u64,
) -> bool {
    avatar_enabled
        && avatar_proactive_ai_enabled
        && !is_paused
        && !text_model.endpoint.trim().is_empty()
        && !text_model.model.trim().is_empty()
        && now_ms >= next_check_ms
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AvatarTransitionDecision {
    pub(crate) emit_state: Option<avatar_engine::AvatarStatePayload>,
    pub(crate) pending_state: Option<avatar_engine::AvatarStatePayload>,
    pub(crate) pending_hits: u8,
}

pub(crate) fn avatar_transition_decision(
    current: Option<&avatar_engine::AvatarStatePayload>,
    pending: Option<&avatar_engine::AvatarStatePayload>,
    pending_hits: u8,
    candidate: &avatar_engine::AvatarStatePayload,
) -> AvatarTransitionDecision {
    const AVATAR_MODE_STABILITY_THRESHOLD: u8 = 2;

    match current {
        None => AvatarTransitionDecision {
            emit_state: Some(candidate.clone()),
            pending_state: None,
            pending_hits: 0,
        },
        Some(current_state) if current_state == candidate => AvatarTransitionDecision {
            emit_state: None,
            pending_state: None,
            pending_hits: 0,
        },
        Some(current_state) if current_state.mode == candidate.mode => AvatarTransitionDecision {
            emit_state: Some(candidate.clone()),
            pending_state: None,
            pending_hits: 0,
        },
        Some(_) => {
            let next_hits = if pending == Some(candidate) {
                pending_hits.saturating_add(1)
            } else {
                1
            };

            if next_hits >= AVATAR_MODE_STABILITY_THRESHOLD {
                AvatarTransitionDecision {
                    emit_state: Some(candidate.clone()),
                    pending_state: None,
                    pending_hits: 0,
                }
            } else {
                AvatarTransitionDecision {
                    emit_state: None,
                    pending_state: Some(candidate.clone()),
                    pending_hits: next_hits,
                }
            }
        }
    }
}

fn fnv1a_update(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= b as u64;
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    *hash ^= 0xff;
    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
}

/// 对全量应用分类规则与自定义分类做 FNV-1a 签名，确保任意条目修改都能触发缓存刷新。
pub(crate) fn compute_category_rules_signature(
    rules: &[work_review_core::config::AppCategoryRule],
    custom_categories: &[work_review_core::config::CustomCategory],
) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    fnv1a_update(&mut hash, &(rules.len() as u64).to_le_bytes());
    for rule in rules {
        fnv1a_update(&mut hash, rule.app_name.as_bytes());
        fnv1a_update(&mut hash, rule.category.as_bytes());
    }
    fnv1a_update(&mut hash, &(custom_categories.len() as u64).to_le_bytes());
    for cat in custom_categories {
        fnv1a_update(&mut hash, cat.key.as_bytes());
        fnv1a_update(&mut hash, cat.name.as_bytes());
        fnv1a_update(&mut hash, cat.color.as_bytes());
        fnv1a_update(&mut hash, cat.icon.as_bytes());
    }
    hash
}

fn compute_privacy_filter_signature(ignored_apps: &[String], excluded_domains: &[String]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let feed = |h: &mut u64, bytes: &[u8]| {
        for &b in bytes {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0100_0000_01b3);
        }
        *h ^= 0xff;
        *h = h.wrapping_mul(0x0100_0000_01b3);
    };
    feed(&mut hash, &(ignored_apps.len() as u64).to_le_bytes());
    for app in ignored_apps {
        feed(&mut hash, app.as_bytes());
    }
    feed(&mut hash, &(excluded_domains.len() as u64).to_le_bytes());
    for domain in excluded_domains {
        feed(&mut hash, domain.as_bytes());
    }
    hash
}

/// 在防抖决策 `avatar_transition_decision` **之前**应用或过期清理主动 AI 情绪覆盖，
/// 既确保用户不切窗口时也能立刻生效，又避免与派生模式冲突造成每 360ms 重复广播。
pub(crate) fn apply_proactive_mood(
    state: &mut avatar_engine::AvatarStatePayload,
    proactive_mood: &mut Option<(String, u64)>,
    now_ms: u64,
) {
    match proactive_mood.as_ref() {
        Some((mood_mode, expires_ms)) if now_ms < *expires_ms => {
            state.mode = mood_mode.clone();
        }
        Some(_) => {
            *proactive_mood = None;
        }
        None => {}
    }
}

fn followup_nudge_ready(last_emitted_at: Option<Instant>, now: Instant) -> bool {
    last_emitted_at
        .is_none_or(|last| now.saturating_duration_since(last) >= FOLLOWUP_SCAN_MIN_INTERVAL)
}

pub(crate) async fn background_avatar_task(state: Arc<Mutex<AppState>>, app: AppHandle) {
    const GOAL_CHECK_INTERVAL: Duration = Duration::from_secs(60);
    const FOLLOWUP_SESSIONS_TTL: Duration = Duration::from_secs(30);
    const LOCK_CHECK_INTERVAL: Duration = Duration::from_secs(3);
    const MAX_TICK_ELAPSED_MS: u64 = 5_000;
    const SLEEP_GAP_THRESHOLD_MS: u64 = 15_000;
    const WAKE_PROACTIVE_GRACE_MS: u64 = 3 * 60_000;

    let mut last_avatar_state: Option<avatar_engine::AvatarStatePayload> = None;
    let mut pending_avatar_state: Option<avatar_engine::AvatarStatePayload> = None;
    let mut pending_avatar_hits: u8 = 0;
    let mut last_window_signature: Option<String> = None;
    let mut break_reminder_runtime = BreakReminderRuntime::new();
    let mut avatar_nudge_runtime = AvatarNudgeRuntime::default();
    // 仅供 ProactiveContext 统计最近 5 分钟应用级切换次数（不重复弹出中文硬编码气泡）
    let mut recent_app_switches_ms: std::collections::VecDeque<u64> =
        std::collections::VecDeque::new();
    // 工作目标庆祝：按墙钟日期去重 + 真实时间节流（60s），避免受轮询间隔影响
    let mut goal_celebrated_date: String = String::new();
    let mut last_goal_check_at = Instant::now()
        .checked_sub(GOAL_CHECK_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut cached_rules: Vec<work_review_core::config::AppCategoryRule> = Vec::new();
    let mut cached_custom_categories: Vec<work_review_core::config::CustomCategory> = Vec::new();
    let mut cached_rules_signature: Option<u64> = None;
    let idle_detector = idle_detector::IdleDetector::new(3);
    let mut last_idle_threshold_minutes: u64 = 3;
    let screen_lock_monitor = Arc::new(screen_lock::ScreenLockMonitor::new());
    let mut last_lock_check_at = Instant::now()
        .checked_sub(LOCK_CHECK_INTERVAL)
        .unwrap_or_else(Instant::now);
    #[cfg_attr(
        not(any(target_os = "windows", target_os = "macos")),
        allow(unused_mut)
    )]
    let mut cached_screen_locked = false;
    let mut was_unavailable = false;
    let mut last_tick_at = Instant::now();
    let task_start_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
    let mut last_wall_ms = task_start_ms;
    // 桌宠模型生成提醒运行时状态（异步后台执行，绝不阻塞主循环）
    let mut next_proactive_check_ms: u64 = task_start_ms + 10 * 60_000;
    let mut proactive_mood: Option<(String, u64)> = None; // (mode, expires_ms)
    let mut proactive_handle: Option<tokio::task::JoinHandle<avatar_proactive::ProactiveOutcome>> =
        None;
    let mut active_app_since_ms: u64 = task_start_ms;
    let mut last_proactive_app: Option<String> = None;
    // 待跟进会话缓存（含日期与隐私过滤签名）与切窗防抖，避免切标签时高频扫 480 条数据库记录
    let mut cached_followup_sessions: Option<(
        Instant,
        String,
        u64,
        Arc<Vec<work_review_core::work_intelligence::WorkSession>>,
    )> = None;
    let mut last_followup_scan_at = Instant::now()
        .checked_sub(FOLLOWUP_SCAN_MIN_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut last_followup_emitted_at: Option<Instant> = None;

    loop {
        // 先非阻塞回收已完成的主动 AI 任务结果
        if proactive_handle.as_ref().is_some_and(|h| h.is_finished()) {
            if let Some(handle) = proactive_handle.take() {
                match handle.await {
                    Ok(outcome) => {
                        if let Some((mode, expires)) = outcome.mood {
                            proactive_mood = Some((mode, expires));
                        }
                        next_proactive_check_ms = outcome.next_check_ms;
                    }
                    Err(err) => {
                        log::warn!("桌宠主动开口后台任务异常结束: {err}");
                        let now_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
                        next_proactive_check_ms = now_ms + 15 * 60_000;
                    }
                }
            }
        }

        let (
            avatar_enabled,
            avatar_proactive_ai_enabled,
            avatar_generating_report,
            avatar_opacity,
            avatar_preset,
            avatar_persona,
            avatar_body_hidden,
            locale,
            idle_threshold_minutes,
            is_recording,
            is_paused,
            break_reminder_enabled,
            break_reminder_interval_minutes,
        ) = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state_guard.config.avatar_enabled,
                state_guard.config.avatar_proactive_ai_enabled,
                state_guard.avatar_generating_report,
                state_guard.config.avatar_opacity,
                state_guard.config.avatar_preset.clone(),
                state_guard.config.avatar_persona.clone(),
                state_guard.config.avatar_body_hidden,
                state_guard.config.locale.clone(),
                u64::from(state_guard.config.idle_threshold_minutes),
                state_guard.is_recording,
                state_guard.is_paused,
                state_guard.config.break_reminder_enabled,
                state_guard.config.break_reminder_interval_minutes,
            )
        };

        if idle_threshold_minutes != last_idle_threshold_minutes {
            idle_detector.set_threshold_minutes(idle_threshold_minutes);
            last_idle_threshold_minutes = idle_threshold_minutes;
        }

        let activity_decision = avatar_activity_decision(
            avatar_enabled,
            is_recording,
            is_paused,
            avatar_opacity,
            &avatar_preset,
            &avatar_persona,
            avatar_body_hidden,
        );
        let poll_interval_ms = avatar_monitor_poll_interval_ms_for_platform(
            cfg!(target_os = "macos"),
            activity_decision.should_continue,
        );
        tokio::time::sleep(Duration::from_millis(poll_interval_ms)).await;

        let now_instant = Instant::now();
        let tick_elapsed_ms = (now_instant
            .saturating_duration_since(last_tick_at)
            .as_millis() as u64)
            .min(MAX_TICK_ELAPSED_MS);
        last_tick_at = now_instant;

        let now_wall_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
        let wall_gap_ms = now_wall_ms.saturating_sub(last_wall_ms);
        last_wall_ms = now_wall_ms;

        if !activity_decision.should_continue {
            if let Some(handle) = proactive_handle.take() {
                handle.abort();
            }
            let reminder_result = advance_break_reminder(
                &mut break_reminder_runtime,
                false,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(0),
            );
            if let Some(payload) = reminder_result.payload.as_ref() {
                avatar_engine::emit_avatar_bubble(&app, payload);
            }

            pending_avatar_state = None;
            pending_avatar_hits = 0;
            last_window_signature = None;
            recent_app_switches_ms.clear();
            avatar_nudge_runtime.recent_switches_ms.clear();

            if let Some(reset_state) = activity_decision.reset_state {
                let should_emit_reset = last_avatar_state.as_ref() != Some(&reset_state);
                {
                    let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    state_guard.avatar_state = reset_state.clone();
                }

                if avatar_enabled && should_emit_reset {
                    avatar_engine::emit_avatar_state(&app, &reset_state);
                }

                last_avatar_state = Some(reset_state);
            } else {
                last_avatar_state = None;
            }
            continue;
        }

        // 熄屏 / 锁屏感知：不可用时暂停前台采集与久坐累计，唤醒/解锁后重置连续时长并延迟主动 AI
        let display_available = crate::activity_clock::display_is_available();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            if now_instant.saturating_duration_since(last_lock_check_at) >= LOCK_CHECK_INTERVAL {
                last_lock_check_at = now_instant;
                let monitor_clone = Arc::clone(&screen_lock_monitor);
                let prev_locked = cached_screen_locked;
                cached_screen_locked =
                    tokio::task::spawn_blocking(move || monitor_clone.is_locked())
                        .await
                        .unwrap_or(false);
                #[cfg(target_os = "windows")]
                if prev_locked && !cached_screen_locked && avatar_enabled {
                    // 无熄屏快锁后解锁时补一次原生透明恢复
                    crate::avatar_transparency::schedule(&app);
                }
                let _ = prev_locked;
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = (&screen_lock_monitor, &mut last_lock_check_at);
        }

        if !display_available || cached_screen_locked {
            if !was_unavailable {
                was_unavailable = true;
                if let Some(handle) = proactive_handle.take() {
                    handle.abort();
                }
                break_reminder_runtime.reset_active_cycle();
                pending_avatar_state = None;
                pending_avatar_hits = 0;
                last_window_signature = None;
                recent_app_switches_ms.clear();
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
            continue;
        }

        if was_unavailable || wall_gap_ms > SLEEP_GAP_THRESHOLD_MS {
            was_unavailable = false;
            active_app_since_ms = now_wall_ms;
            next_proactive_check_ms =
                next_proactive_check_ms.max(now_wall_ms.saturating_add(WAKE_PROACTIVE_GRACE_MS));
            recent_app_switches_ms.clear();
            break_reminder_runtime.reset_active_cycle();
        }

        let sampled_at = Instant::now();
        let active_window = match tokio::task::spawn_blocking(monitor::get_active_window_fast).await
        {
            Ok(Ok(window)) => window,
            _ => continue,
        };

        {
            let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            state_guard.cached_active_window = Some((sampled_at, active_window.clone()));
            let rules = &state_guard.config.app_category_rules;
            let cats = &state_guard.config.custom_categories;
            let sig = compute_category_rules_signature(rules, cats);
            if cached_rules_signature != Some(sig) {
                cached_rules = rules.clone();
                cached_custom_categories = cats.clone();
                cached_rules_signature = Some(sig);
            }
        };
        let app_category_rules = &cached_rules;
        let app_custom_categories = &cached_custom_categories;

        if should_skip_transient_window(&active_window) || should_skip_system_window(&active_window)
        {
            continue;
        }

        let input_idle = idle_detector.is_input_idle();

        // 工作目标庆祝：按真实时间每 60 秒检查一次，且 SQLite 查询在 spawn_blocking 内执行（绝不持锁查库）
        if avatar_enabled
            && now_instant.saturating_duration_since(last_goal_check_at) >= GOAL_CHECK_INTERVAL
        {
            last_goal_check_at = now_instant;
            let goal_query = {
                let s = state.lock().unwrap_or_else(|e| e.into_inner());
                if s.config.goal_notifications {
                    s.config
                        .daily_work_goal_minutes
                        .map(|goal| (goal, s.database.clone(), s.config.effective_work_segments()))
                } else {
                    None
                }
            };
            if let Some((goal, database, segments)) = goal_query {
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                if today != goal_celebrated_date {
                    let today_for_db = today.clone();
                    let work_secs = tokio::task::spawn_blocking(move || {
                        database
                            .get_daily_stats_with_segments(&today_for_db, &segments)
                            .map(|st| st.work_time_duration)
                            .unwrap_or(0)
                    })
                    .await
                    .unwrap_or(0);
                    if work_secs >= (goal as i64 * 60) {
                        avatar_engine::emit_avatar_bubble(
                            &app,
                            &avatar_engine::AvatarBubblePayload::success(
                                "__avatar_goal_achieved__",
                            ),
                        );
                        goal_celebrated_date = today;
                    }
                }
            }
        }

        let reminder_result = if !(avatar_enabled && break_reminder_enabled) {
            advance_break_reminder(
                &mut break_reminder_runtime,
                false,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(0),
            )
        } else if break_reminder_runtime.phase == BreakReminderPhase::Cooldown {
            advance_break_reminder(
                &mut break_reminder_runtime,
                true,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(tick_elapsed_ms),
            )
        } else if input_idle {
            break_reminder_runtime.reset_active_cycle();
            BreakReminderAdvanceResult::default()
        } else {
            advance_break_reminder(
                &mut break_reminder_runtime,
                true,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(tick_elapsed_ms),
            )
        };
        if let Some(payload) = reminder_result.payload.as_ref() {
            avatar_engine::emit_avatar_bubble(&app, payload);
        }

        let mut avatar_state = avatar_engine::apply_avatar_visual_settings(
            avatar_engine::derive_avatar_state_with_rules(
                app_category_rules,
                app_custom_categories,
                &active_window.app_name,
                &active_window.window_title,
                active_window.browser_url.as_deref(),
                input_idle,
                avatar_generating_report,
            ),
            avatar_opacity,
            &avatar_preset,
            &avatar_persona,
            avatar_body_hidden,
        );
        // 在防抖决策之前应用主动 AI 情绪，防止与派生模式冲突造成 360ms 反复广播。
        apply_proactive_mood(&mut avatar_state, &mut proactive_mood, now_wall_ms);

        let window_signature = format!(
            "{}|{}|{}",
            active_window.app_name,
            active_window.window_title,
            active_window.browser_url.as_deref().unwrap_or_default()
        );
        let window_changed = last_window_signature.as_deref() != Some(window_signature.as_str());
        if last_proactive_app.as_deref() != Some(active_window.app_name.as_str()) {
            let switch_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
            if last_proactive_app.is_some() {
                recent_app_switches_ms.push_back(switch_ms);
            }
            while let Some(&old) = recent_app_switches_ms.front() {
                if switch_ms.saturating_sub(old) > 5 * 60 * 1000 {
                    recent_app_switches_ms.pop_front();
                } else {
                    break;
                }
            }
            active_app_since_ms = switch_ms;
            last_proactive_app = Some(active_window.app_name.clone());
        }

        let transition_decision = avatar_transition_decision(
            last_avatar_state.as_ref(),
            pending_avatar_state.as_ref(),
            pending_avatar_hits,
            &avatar_state,
        );

        pending_avatar_state = transition_decision.pending_state;
        pending_avatar_hits = transition_decision.pending_hits;

        if let Some(next_avatar_state) = transition_decision.emit_state {
            let collect_cost_ms = sampled_at.elapsed().as_millis();
            let previous_mode = last_avatar_state
                .as_ref()
                .map(|state| state.mode.as_str())
                .unwrap_or("none");

            {
                let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                state_guard.avatar_state = next_avatar_state.clone();
            }

            avatar_engine::emit_avatar_state(&app, &next_avatar_state);

            let entered_idle = match &last_avatar_state {
                Some(previous) => !previous.is_idle && next_avatar_state.is_idle,
                None => next_avatar_state.is_idle,
            };

            if entered_idle {
                avatar_engine::emit_avatar_bubble(
                    &app,
                    &avatar_engine::AvatarBubblePayload::info("先放松一下，待会再继续推进。"),
                );
            }

            log::info!(
                "🐾 桌宠状态切换: {} -> {} | 窗口={} | 采集耗时={}ms",
                previous_mode,
                next_avatar_state.mode,
                window_signature,
                collect_cost_ms
            );

            last_avatar_state = Some(next_avatar_state);
            last_window_signature = Some(window_signature);
        } else if window_changed {
            log::debug!(
                "🐾 桌宠检测到前台切换，但状态未变: {} | 采集耗时={}ms",
                window_signature,
                sampled_at.elapsed().as_millis()
            );
            last_window_signature = Some(window_signature);
        }

        if avatar_enabled && window_changed {
            let now = chrono::Local::now();
            let now_ts = now.timestamp();
            let now_ms = now.timestamp_millis().max(0) as u64;
            let switch_nudge_ready = record_avatar_window_switch(&mut avatar_nudge_runtime, now_ms);
            let date_from = (now - chrono::Duration::days(3))
                .format("%Y-%m-%d")
                .to_string();

            let (database, ignored_apps, excluded_domains, persona, manual_followups) = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                let (ignored_apps, excluded_domains) =
                    crate::commands::collect_privacy_filters(&state_guard);
                (
                    state_guard.database.clone(),
                    ignored_apps,
                    excluded_domains,
                    state_guard.config.avatar_persona.clone(),
                    state_guard.config.avatar_followups.clone(),
                )
            };

            if now_instant.saturating_duration_since(last_followup_scan_at)
                >= FOLLOWUP_SCAN_MIN_INTERVAL
            {
                last_followup_scan_at = now_instant;
                let privacy_sig =
                    compute_privacy_filter_signature(&ignored_apps, &excluded_domains);
                let cached_sessions = cached_followup_sessions
                    .as_ref()
                    .filter(|(ts, cached_date, cached_sig, _)| {
                        cached_date == &date_from
                            && *cached_sig == privacy_sig
                            && now_instant.saturating_duration_since(*ts) < FOLLOWUP_SESSIONS_TTL
                    })
                    .map(|(_, _, _, sessions)| Arc::clone(sessions));

                let active_window_clone = active_window.clone();
                let persona_clone = persona.clone();
                let manual_followups_clone = manual_followups.clone();
                let date_from_owned = date_from.clone();

                let followup_result = tokio::task::spawn_blocking(move || {
                    let sessions = match cached_sessions {
                        Some(existing) => existing,
                        None => {
                            let raw = database.get_recent_activities_without_ocr(
                                Some(date_from_owned.as_str()),
                                None,
                                480,
                            )?;
                            let filtered = crate::commands::filter_activities_by_privacy(
                                raw,
                                &ignored_apps,
                                &excluded_domains,
                            );
                            Arc::new(work_review_core::work_intelligence::build_work_sessions(
                                &filtered,
                            ))
                        }
                    };
                    let suggestion = crate::avatar_followup::find_followup_suggestion_from_sessions(
                        &sessions,
                        &active_window_clone,
                        &persona_clone,
                        &manual_followups_clone,
                        now_ts,
                        now_ms,
                    );
                    Ok::<_, work_review_core::error::AppError>((sessions, suggestion))
                })
                .await;

                if let Ok(Ok((sessions, suggestion))) = followup_result {
                    cached_followup_sessions =
                        Some((now_instant, date_from, privacy_sig, sessions));
                    if let Some(payload) = suggestion {
                        let emit_now_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
                        if crate::avatar_followup::should_emit_followup(
                            &payload.project_key,
                            emit_now_ms,
                        ) {
                            crate::avatar_followup::emit_followup_suggestion(&app, &payload);
                            crate::avatar_followup::note_followup_emitted(
                                &payload.project_key,
                                emit_now_ms,
                            );
                            last_followup_emitted_at = Some(Instant::now());
                        }
                    }
                }
            }

            if followup_nudge_ready(last_followup_emitted_at, Instant::now()) {
                if switch_nudge_ready {
                    avatar_engine::emit_avatar_bubble(
                        &app,
                        &avatar_engine::AvatarBubblePayload::info(avatar_switch_nudge_message_key(
                            &persona,
                        )),
                    );
                } else if let Some(count) = should_emit_avatar_backlog_nudge(
                    &mut avatar_nudge_runtime,
                    &manual_followups,
                    now_ts,
                    now_ms,
                ) {
                    avatar_engine::emit_avatar_bubble(
                        &app,
                        &avatar_engine::AvatarBubblePayload::info(
                            avatar_backlog_nudge_message_key(&persona, count),
                        ),
                    );
                }
            }
        }

        // 桌宠模型生成提醒：仅到点且无在途任务时懒解析模型，并交由独立异步任务执行，绝不阻塞主循环
        let proactive_now_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
        if proactive_handle.is_none()
            && avatar_proactive_ai_enabled
            && proactive_now_ms >= next_proactive_check_ms
        {
            let text_model = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                state_guard
                    .config
                    .resolve_model_for_task(work_review_core::config::AiTaskKind::Proactive)
            };
            if avatar_proactive_ai_should_run(
                avatar_enabled,
                avatar_proactive_ai_enabled,
                is_paused,
                &text_model,
                proactive_now_ms,
                next_proactive_check_ms,
            ) {
                let active_minutes = proactive_now_ms.saturating_sub(active_app_since_ms) / 60_000;
                while let Some(&old) = recent_app_switches_ms.front() {
                    if proactive_now_ms.saturating_sub(old) > 5 * 60 * 1000 {
                        recent_app_switches_ms.pop_front();
                    } else {
                        break;
                    }
                }
                let recent_switches = recent_app_switches_ms.len() as u32;
                let now = chrono::Local::now();
                let today = now.format("%Y-%m-%d").to_string();
                let hour = now.format("%H").to_string().parse::<u32>().unwrap_or(0);
                let minute = now.format("%M").to_string().parse::<u32>().unwrap_or(0);
                let (database, segments) = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        state_guard.database.clone(),
                        state_guard.config.effective_work_segments(),
                    )
                };
                let app_clone = app.clone();
                let app_name = active_window.app_name.clone();
                let persona_clone = avatar_persona.clone();
                let locale_clone = locale.clone();
                proactive_handle = Some(tokio::spawn(async move {
                    let work_seconds_today = tokio::task::spawn_blocking(move || {
                        database
                            .get_daily_stats_with_segments(&today, &segments)
                            .map(|st| st.work_time_duration.max(0) as u64)
                            .unwrap_or(0)
                    })
                    .await
                    .unwrap_or(0);
                    let context = avatar_proactive::ProactiveContext {
                        app_name,
                        active_minutes,
                        work_seconds_today,
                        recent_switches,
                        is_idle: input_idle,
                        hour,
                        minute,
                    };
                    avatar_proactive::decide_and_speak(
                        &app_clone,
                        &text_model,
                        &persona_clone,
                        &locale_clone,
                        &context,
                    )
                    .await
                }));
            } else {
                // 未配置可用模型等原因暂不触发时退避 60s，避免每 360ms 重复锁配置解析模型。
                next_proactive_check_ms = proactive_now_ms.saturating_add(60_000);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use work_review_core::config::{AppCategoryRule, CustomCategory};

    #[test]
    fn 跟进提示的静默期应跨过未重新扫描会话的后续帧() {
        let emitted_at = Instant::now();
        assert!(followup_nudge_ready(None, emitted_at));
        for delay in [0, 180, 750, 1999] {
            assert!(!followup_nudge_ready(
                Some(emitted_at),
                emitted_at + Duration::from_millis(delay)
            ));
        }
        assert!(followup_nudge_ready(
            Some(emitted_at),
            emitted_at + FOLLOWUP_SCAN_MIN_INTERVAL
        ));
    }

    #[test]
    fn proactive_mood应在防抖前应用且不触发重复切换死循环() {
        let mut mood = Some(("idle".to_string(), 20_000));
        let mut raw = avatar_engine::default_avatar_state();
        raw.mode = "working".to_string();
        let current = raw.clone();

        // 第 1 帧应用 mood 后进入 pending
        let mut s1 = raw.clone();
        apply_proactive_mood(&mut s1, &mut mood, 10_000);
        assert_eq!(s1.mode, "idle");
        let d1 = avatar_transition_decision(Some(&current), None, 0, &s1);
        assert!(d1.emit_state.is_none());
        assert_eq!(d1.pending_hits, 1);

        // 第 2 帧确认切入 idle
        let mut s2 = raw.clone();
        apply_proactive_mood(&mut s2, &mut mood, 10_200);
        let d2 = avatar_transition_decision(
            Some(&current),
            d1.pending_state.as_ref(),
            d1.pending_hits,
            &s2,
        );
        let emitted = d2.emit_state.expect("第 2 帧应切入 mood");
        assert_eq!(emitted.mode, "idle");

        // 第 3 帧起 current == s3，绝不再重复触发广播或 pending
        let mut s3 = raw.clone();
        apply_proactive_mood(&mut s3, &mut mood, 11_000);
        let d3 = avatar_transition_decision(Some(&emitted), None, 0, &s3);
        assert!(d3.emit_state.is_none());
        assert!(d3.pending_state.is_none());
        assert_eq!(d3.pending_hits, 0);

        // 过期后清空 mood，恢复派生模式
        let mut s4 = raw.clone();
        apply_proactive_mood(&mut s4, &mut mood, 25_000);
        assert!(mood.is_none());
        assert_eq!(s4.mode, "working");
    }

    #[test]
    fn 分类规则签名应对非末尾规则与自定义分类变更敏感() {
        let r1 = vec![
            AppCategoryRule {
                app_name: "Code".to_string(),
                category: "development".to_string(),
            },
            AppCategoryRule {
                app_name: "Chrome".to_string(),
                category: "browser".to_string(),
            },
        ];
        let mut r2 = r1.clone();
        r2[0].category = "office".to_string();
        assert_ne!(
            compute_category_rules_signature(&r1, &[]),
            compute_category_rules_signature(&r2, &[])
        );

        let c1 = vec![CustomCategory {
            key: "focus".to_string(),
            name: "专注".to_string(),
            color: "#3b82f6".to_string(),
            icon: "🎯".to_string(),
        }];
        assert_ne!(
            compute_category_rules_signature(&[], &[]),
            compute_category_rules_signature(&[], &c1)
        );
    }

    #[test]
    fn 隐私过滤签名应在忽略应用或排除域名变更时失效() {
        let base = compute_privacy_filter_signature(&[], &[]);
        let with_app = compute_privacy_filter_signature(&["WeChat".to_string()], &[]);
        let with_domain = compute_privacy_filter_signature(&[], &["bank.example".to_string()]);
        assert_ne!(base, with_app);
        assert_ne!(base, with_domain);
        assert_ne!(with_app, with_domain);
    }
}
