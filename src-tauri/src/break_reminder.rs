//! 久坐/休息提醒状态机与桌宠轻量切换/待办积压提醒。

use crate::avatar_engine;
use std::collections::VecDeque;
use work_review_core::config::AvatarFollowupItem;
pub(crate) const BREAK_REMINDER_BUFFER_MINUTES: u64 = 5;
pub(crate) const BREAK_REMINDER_MESSAGE: &str = "该休息一下了，起来活动活动吧。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakReminderPhase {
    Counting,
    Cooldown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreakReminderRuntime {
    pub(crate) phase: BreakReminderPhase,
    pub(crate) elapsed_ms: u64,
    pub(crate) bubble_visible: bool,
}

impl BreakReminderRuntime {
    pub(crate) fn new() -> Self {
        Self {
            phase: BreakReminderPhase::Counting,
            elapsed_ms: 0,
            bubble_visible: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.phase = BreakReminderPhase::Counting;
        self.elapsed_ms = 0;
        self.bubble_visible = false;
    }

    pub(crate) fn reset_active_cycle(&mut self) {
        if self.phase == BreakReminderPhase::Counting {
            self.elapsed_ms = 0;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakReminderSignal {
    TickMillis(u64),
    #[allow(dead_code)]
    TickMinutes(u64),
    #[allow(dead_code)]
    Dismiss,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BreakReminderAdvanceResult {
    pub(crate) should_emit: bool,
    pub(crate) should_clear: bool,
    pub(crate) payload: Option<avatar_engine::AvatarBubblePayload>,
}

pub(crate) fn advance_break_reminder(
    state: &mut BreakReminderRuntime,
    enabled: bool,
    interval_minutes: u64,
    signal: BreakReminderSignal,
) -> BreakReminderAdvanceResult {
    let mut result = BreakReminderAdvanceResult::default();

    if !enabled {
        if state.bubble_visible {
            result.should_clear = true;
            result.payload = Some(avatar_engine::AvatarBubblePayload::clear());
        }
        state.reset();
        return result;
    }

    match signal {
        BreakReminderSignal::Dismiss => {
            if state.bubble_visible {
                state.bubble_visible = false;
                result.should_clear = true;
                result.payload = Some(avatar_engine::AvatarBubblePayload::clear());
            }
            return result;
        }
        BreakReminderSignal::TickMillis(0) | BreakReminderSignal::TickMinutes(0) => return result,
        _ => {}
    }

    let delta_ms = match signal {
        BreakReminderSignal::TickMillis(value) => value,
        BreakReminderSignal::TickMinutes(value) => value.saturating_mul(60_000),
        BreakReminderSignal::Dismiss => 0,
    };

    match state.phase {
        BreakReminderPhase::Counting => {
            state.elapsed_ms = state.elapsed_ms.saturating_add(delta_ms);
            if state.elapsed_ms >= interval_minutes.saturating_mul(60_000) {
                state.phase = BreakReminderPhase::Cooldown;
                state.elapsed_ms = 0;
                state.bubble_visible = true;
                result.should_emit = true;
                result.payload = Some(avatar_engine::AvatarBubblePayload::persistent_info(
                    BREAK_REMINDER_MESSAGE,
                ));
            }
        }
        BreakReminderPhase::Cooldown => {
            state.elapsed_ms = state.elapsed_ms.saturating_add(delta_ms);
            if state.elapsed_ms >= BREAK_REMINDER_BUFFER_MINUTES.saturating_mul(60_000) {
                state.phase = BreakReminderPhase::Counting;
                state.elapsed_ms = 0;
            }
        }
    }

    result
}

pub(crate) const AVATAR_SWITCH_NUDGE_WINDOW_MS: u64 = 3 * 60 * 1000;
pub(crate) const AVATAR_SWITCH_NUDGE_THRESHOLD: usize = 8;
pub(crate) const AVATAR_SWITCH_NUDGE_COOLDOWN_MS: u64 = 20 * 60 * 1000;
pub(crate) const AVATAR_BACKLOG_NUDGE_COOLDOWN_MS: u64 = 90 * 60 * 1000;
pub(crate) const AVATAR_BACKLOG_NUDGE_MIN_AGE_SECS: i64 = 30 * 60;
pub(crate) const AVATAR_NUDGE_SWITCH_COMPANION: &str = "__avatar_nudge_switch_companion__";
pub(crate) const AVATAR_NUDGE_SWITCH_ASSISTANT: &str = "__avatar_nudge_switch_assistant__";
pub(crate) const AVATAR_NUDGE_SWITCH_COACH: &str = "__avatar_nudge_switch_coach__";

#[derive(Default)]
pub(crate) struct AvatarNudgeRuntime {
    pub(crate) recent_switches_ms: VecDeque<u64>,
    last_switch_nudge_at_ms: u64,
    last_backlog_nudge_at_ms: u64,
}

pub(crate) fn avatar_switch_nudge_message_key(persona: &str) -> &'static str {
    match persona.trim() {
        "companion" => AVATAR_NUDGE_SWITCH_COMPANION,
        "coach" => AVATAR_NUDGE_SWITCH_COACH,
        _ => AVATAR_NUDGE_SWITCH_ASSISTANT,
    }
}

pub(crate) fn avatar_backlog_nudge_message_key(persona: &str, count: usize) -> String {
    format!("__avatar_backlog_nudge__:{}:{}", persona.trim(), count)
}

pub(crate) fn record_avatar_window_switch(runtime: &mut AvatarNudgeRuntime, now_ms: u64) -> bool {
    runtime.recent_switches_ms.push_back(now_ms);

    while runtime
        .recent_switches_ms
        .front()
        .is_some_and(|timestamp| now_ms.saturating_sub(*timestamp) > AVATAR_SWITCH_NUDGE_WINDOW_MS)
    {
        runtime.recent_switches_ms.pop_front();
    }

    if runtime.recent_switches_ms.len() < AVATAR_SWITCH_NUDGE_THRESHOLD {
        return false;
    }

    if runtime.last_switch_nudge_at_ms != 0
        && now_ms.saturating_sub(runtime.last_switch_nudge_at_ms) < AVATAR_SWITCH_NUDGE_COOLDOWN_MS
    {
        return false;
    }

    runtime.last_switch_nudge_at_ms = now_ms;
    true
}

pub(crate) fn count_open_avatar_followups_for_nudge(
    items: &[AvatarFollowupItem],
    now_ts: i64,
) -> usize {
    items
        .iter()
        .filter(|item| item.status == "open")
        .filter(|item| now_ts.saturating_sub(item.created_at) >= AVATAR_BACKLOG_NUDGE_MIN_AGE_SECS)
        .count()
}

pub(crate) fn should_emit_avatar_backlog_nudge(
    runtime: &mut AvatarNudgeRuntime,
    items: &[AvatarFollowupItem],
    now_ts: i64,
    now_ms: u64,
) -> Option<usize> {
    let count = count_open_avatar_followups_for_nudge(items, now_ts);
    if count == 0 {
        return None;
    }

    if runtime.last_backlog_nudge_at_ms != 0
        && now_ms.saturating_sub(runtime.last_backlog_nudge_at_ms)
            < AVATAR_BACKLOG_NUDGE_COOLDOWN_MS
    {
        return None;
    }

    runtime.last_backlog_nudge_at_ms = now_ms;
    Some(count)
}
