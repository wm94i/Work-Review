//! 桌宠状态监控、切换防抖、主动 AI 提醒与久坐提醒后台任务。

use crate::break_reminder::*;
use crate::window::{should_skip_system_window, should_skip_transient_window};
use crate::{avatar_engine, avatar_proactive, idle_detector, monitor, AppState};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::AppHandle;
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

pub(crate) async fn background_avatar_task(state: Arc<Mutex<AppState>>, app: AppHandle) {
    let mut last_avatar_state: Option<avatar_engine::AvatarStatePayload> = None;
    let mut pending_avatar_state: Option<avatar_engine::AvatarStatePayload> = None;
    let mut pending_avatar_hits: u8 = 0;
    let mut last_window_signature: Option<String> = None;
    let mut break_reminder_runtime = BreakReminderRuntime::new();
    let mut avatar_nudge_runtime = AvatarNudgeRuntime::default();
    // 频繁切换检测运行时
    let mut last_switch_signature: Option<String> = None;
    let mut recent_switches_ms: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
    let mut last_focus_nudge_ms: u64 = 0;
    // 工作目标庆祝：防止同一天重复庆祝
    let mut goal_celebrated_date: String = String::new();
    let mut goal_check_counter: u32 = 0;
    let mut cached_rules: Vec<work_review_core::config::AppCategoryRule> = Vec::new();
    let mut cached_custom_categories: Vec<work_review_core::config::CustomCategory> = Vec::new();
    let mut cached_rules_signature: u64 = 0;
    const IDLE_TIMEOUT_MINUTES: u64 = 3;
    let idle_detector = idle_detector::IdleDetector::new(IDLE_TIMEOUT_MINUTES);
    // 桌宠模型生成提醒运行时状态
    let task_start_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
    let mut next_proactive_check_ms: u64 = task_start_ms + 10 * 60_000; // 启动后 10 分钟首次
    let mut proactive_mood: Option<(String, u64)> = None; // (mode, expires_ms)
    let mut active_app_since_ms: u64 = task_start_ms;
    let mut last_proactive_app: Option<String> = None;

    loop {
        let (
            avatar_enabled,
            avatar_proactive_ai_enabled,
            avatar_generating_report,
            avatar_opacity,
            avatar_preset,
            avatar_persona,
            avatar_body_hidden,
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
                state_guard.is_recording,
                state_guard.is_paused,
                state_guard.config.break_reminder_enabled,
                state_guard.config.break_reminder_interval_minutes,
            )
        };

        let text_model = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            state_guard
                .config
                .resolve_model_for_task(work_review_core::config::AiTaskKind::Proactive)
        };

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

        if !activity_decision.should_continue {
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

        let sampled_at = std::time::Instant::now();
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
            let sig = rules.len() as u64
                | rules.last().map_or(0u64, |r| {
                    let mut h: u64 = 0;
                    for b in r.app_name.as_bytes() {
                        h = h.wrapping_add(*b as u64);
                    }
                    for b in r.category.as_bytes() {
                        h = h.wrapping_mul(31).wrapping_add(*b as u64);
                    }
                    h
                });
            if sig != cached_rules_signature {
                cached_rules = rules.clone();
                cached_custom_categories = cats.clone();
                cached_rules_signature = sig;
            }
        };
        let app_category_rules = &cached_rules;
        let app_custom_categories = &cached_custom_categories;

        if should_skip_transient_window(&active_window) || should_skip_system_window(&active_window)
        {
            continue;
        }

        let input_idle = idle_detector.is_input_idle();
        // 工作目标庆祝：每 ~60 轮检查一次（约 1-2 分钟）
        goal_check_counter = goal_check_counter.wrapping_add(1);
        if goal_check_counter.is_multiple_of(60) && avatar_enabled {
            let (goal_minutes, goal_notify) = {
                let s = state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    s.config.daily_work_goal_minutes,
                    s.config.goal_notifications,
                )
            };
            if goal_notify {
                if let Some(goal) = goal_minutes {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    if today != goal_celebrated_date {
                        let work_secs = {
                            let s = state.lock().unwrap_or_else(|e| e.into_inner());
                            let segments = s.config.effective_work_segments();
                            s.database
                                .get_daily_stats_with_segments(&today, &segments)
                                .map(|st| st.work_time_duration)
                                .unwrap_or(0)
                        };
                        if work_secs >= (goal as i64 * 60) {
                            avatar_engine::emit_avatar_bubble(
                                &app,
                                &avatar_engine::AvatarBubblePayload::success(
                                    "🎉 今日工作目标达成！继续保持！",
                                ),
                            );
                            goal_celebrated_date = today;
                        }
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
                BreakReminderSignal::TickMillis(poll_interval_ms),
            )
        } else if input_idle {
            break_reminder_runtime.reset_active_cycle();
            BreakReminderAdvanceResult::default()
        } else {
            advance_break_reminder(
                &mut break_reminder_runtime,
                true,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(poll_interval_ms),
            )
        };
        if let Some(payload) = reminder_result.payload.as_ref() {
            avatar_engine::emit_avatar_bubble(&app, payload);
        }

        let avatar_state = avatar_engine::apply_avatar_visual_settings(
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

        let window_signature = format!(
            "{}|{}|{}",
            active_window.app_name,
            active_window.window_title,
            active_window.browser_url.as_deref().unwrap_or_default()
        );
        let window_changed = last_window_signature.as_deref() != Some(window_signature.as_str());
        if last_proactive_app.as_deref() != Some(active_window.app_name.as_str()) {
            active_app_since_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
            last_proactive_app = Some(active_window.app_name.clone());
        }

        // 方向 2：频繁切换检测 —— 5 分钟内切换 ≥6 次 → 桌宠提醒专注（30 分钟冷却）
        let actually_switched = last_switch_signature.as_deref() != Some(window_signature.as_str());
        last_switch_signature = Some(window_signature.clone());
        if actually_switched && is_recording && !is_paused {
            let switch_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
            recent_switches_ms.push_back(switch_ms);
            while let Some(&old) = recent_switches_ms.front() {
                if switch_ms.saturating_sub(old) > 5 * 60 * 1000 {
                    recent_switches_ms.pop_front();
                } else {
                    break;
                }
            }
            if recent_switches_ms.len() >= 6
                && switch_ms.saturating_sub(last_focus_nudge_ms) > 30 * 60 * 1000
            {
                avatar_engine::emit_avatar_bubble(
                    &app,
                    &avatar_engine::AvatarBubblePayload::info(
                        "检测到频繁切换应用，要试试专注一段时间吗？",
                    ),
                );
                last_focus_nudge_ms = switch_ms;
                recent_switches_ms.clear();
            }
        }

        let transition_decision = avatar_transition_decision(
            last_avatar_state.as_ref(),
            pending_avatar_state.as_ref(),
            pending_avatar_hits,
            &avatar_state,
        );

        pending_avatar_state = transition_decision.pending_state;
        pending_avatar_hits = transition_decision.pending_hits;

        if let Some(mut next_avatar_state) = transition_decision.emit_state {
            // 桌宠模型生成提醒的情绪覆盖：AI 给的表情未过期则覆盖 mode
            let mood_now_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
            match &proactive_mood {
                Some((mood_mode, expires_ms)) if mood_now_ms < *expires_ms => {
                    next_avatar_state.mode = mood_mode.clone();
                }
                Some(_) => {
                    proactive_mood = None; // 过期清空
                }
                None => {}
            }
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
            let date_from_owned = date_from.clone();
            let activities_result = tokio::task::spawn_blocking(move || {
                let raw = database.get_recent_activities_without_ocr(
                    Some(date_from_owned.as_str()),
                    None,
                    480,
                )?;
                Ok::<_, work_review_core::error::AppError>(
                    crate::commands::filter_activities_by_privacy(
                        raw,
                        &ignored_apps,
                        &excluded_domains,
                    ),
                )
            })
            .await;

            if let Ok(Ok(activities)) = activities_result {
                let mut emitted_followup = false;
                if let Some(payload) = crate::avatar_followup::find_followup_suggestion(
                    &activities,
                    &active_window,
                    &persona,
                    &manual_followups,
                    now_ts,
                    now_ms,
                ) {
                    if crate::avatar_followup::should_emit_followup(&payload.project_key, now_ms) {
                        crate::avatar_followup::emit_followup_suggestion(&app, &payload);
                        crate::avatar_followup::note_followup_emitted(&payload.project_key, now_ms);
                        emitted_followup = true;
                    }
                }

                if !emitted_followup && switch_nudge_ready {
                    avatar_engine::emit_avatar_bubble(
                        &app,
                        &avatar_engine::AvatarBubblePayload::info(avatar_switch_nudge_message_key(
                            &persona,
                        )),
                    );
                } else if !emitted_followup {
                    if let Some(count) = should_emit_avatar_backlog_nudge(
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
        }

        // 桌宠模型生成提醒：到点才调一次文本模型，模型自主决定是否提示和提示内容
        let proactive_now_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
        if avatar_proactive_ai_should_run(
            avatar_enabled,
            avatar_proactive_ai_enabled,
            is_paused,
            &text_model,
            proactive_now_ms,
            next_proactive_check_ms,
        ) {
            let active_minutes = proactive_now_ms.saturating_sub(active_app_since_ms) / 60_000;
            let recent_switches = recent_switches_ms.len() as u32;
            let (work_seconds_today, hour, minute) = {
                let now = chrono::Local::now();
                let today = now.format("%Y-%m-%d").to_string();
                let h = now.format("%H").to_string().parse::<u32>().unwrap_or(0);
                let m = now.format("%M").to_string().parse::<u32>().unwrap_or(0);
                let (database, segments) = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        state_guard.database.clone(),
                        state_guard.config.effective_work_segments(),
                    )
                };
                let work_secs = tokio::task::spawn_blocking(move || {
                    database
                        .get_daily_stats_with_segments(&today, &segments)
                        .map(|st| st.work_time_duration.max(0) as u64)
                        .unwrap_or(0)
                })
                .await
                .unwrap_or(0);
                (work_secs, h, m)
            };
            let context = avatar_proactive::ProactiveContext {
                app_name: active_window.app_name.clone(),
                active_minutes,
                work_seconds_today,
                recent_switches,
                is_idle: input_idle,
                hour,
                minute,
            };
            let outcome = avatar_proactive::decide_and_speak(
                &app,
                &text_model,
                &avatar_persona,
                "zh-CN",
                &context,
            )
            .await;
            if let Some((mode, expires)) = outcome.mood {
                proactive_mood = Some((mode, expires));
            }
            next_proactive_check_ms = outcome.next_check_ms;
        }
    }
}
