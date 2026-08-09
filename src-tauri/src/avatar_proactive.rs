//! 桌宠主动开口（AI 自定节奏）。
//!
//! 配了 AI 模型时，桌宠主循环到点调一次 LLM，由模型自主决定：
//! 要不要开口、说什么、什么语气/表情、以及"下次什么时候再来问我"。
//! 没配模型时整条链路由主循环短路，本模块不会被调用。

use crate::agent::model::{chat_with_tools, Deadline, Message};
use crate::avatar_engine::{emit_avatar_bubble, AvatarBubblePayload};
use crate::config::ModelConfig;
use tauri::AppHandle;

/// 喂给 LLM 的当前工作状态快照。
pub struct ProactiveContext {
    pub app_name: String,
    pub active_minutes: u64,
    pub work_seconds_today: u64,
    pub recent_switches: u32,
    pub is_idle: bool,
    pub hour: u32,
    pub minute: u32,
}

/// 一次主动开口判断的结果。
pub struct ProactiveOutcome {
    /// (覆盖的 mode, 过期时间戳 ms)。None 表示不覆盖表情。
    pub mood: Option<(String, u64)>,
    /// 下次该再来问 LLM 的时间戳 ms。
    pub next_check_ms: u64,
}

#[derive(serde::Deserialize)]
struct ProactiveReply {
    #[serde(default)]
    speak: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    tone: String,
    #[serde(default = "default_next_check")]
    next_check_minutes: u64,
}

fn default_next_check() -> u64 {
    15
}

const MOOD_DURATION_MS: u64 = 5 * 60_000;
const BUBBLE_DURATION_MS: u64 = 6_000;
const MIN_NEXT_CHECK_MIN: u64 = 3;
const MAX_NEXT_CHECK_MIN: u64 = 30;
const FALLBACK_NEXT_CHECK_MIN: u64 = 15;

/// 调一次 LLM，决定要不要主动开口。
/// 错误 / JSON 解析失败一律回退为"闭嘴 + 15 分钟后再问"，绝不 panic。
pub async fn decide_and_speak(
    app: &AppHandle,
    text_model: &ModelConfig,
    avatar_persona: &str,
    locale: &str,
    context: &ProactiveContext,
    timeout_secs: u64,
) -> ProactiveOutcome {
    let now_ms = chrono::Local::now().timestamp_millis() as u64;

    let system = build_system_prompt(avatar_persona, locale);
    let event = build_event_prompt(context, avatar_persona);

    let response = match chat_with_tools(
        text_model,
        &system,
        &[Message::user(&event)],
        &[],
        Deadline::from_assistant_timeout_secs(timeout_secs),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!("桌宠主动开口 LLM 调用失败: {e}");
            return fallback(now_ms);
        }
    };

    let content = response.content.unwrap_or_default();
    let parsed = match parse_reply(&content) {
        Some(p) => p,
        None => {
            log::debug!("桌宠主动开口返回非 JSON，跳过: {content}");
            return fallback(now_ms);
        }
    };

    let next_check_ms = now_ms
        + parsed
            .next_check_minutes
            .clamp(MIN_NEXT_CHECK_MIN, MAX_NEXT_CHECK_MIN)
            * 60_000;

    if !parsed.speak || parsed.text.trim().is_empty() {
        return ProactiveOutcome {
            mood: None,
            next_check_ms,
        };
    }

    let tone = if parsed.tone.trim().is_empty() {
        "info".to_string()
    } else {
        parsed.tone.trim().to_string()
    };

    emit_avatar_bubble(
        app,
        &AvatarBubblePayload {
            message: parsed.text.trim().to_string(),
            tone: tone.clone(),
            persistent: false,
            duration_ms: Some(BUBBLE_DURATION_MS),
            clear: false,
        },
    );

    let mood = tone_to_mode(&tone).map(|m| (m, now_ms + MOOD_DURATION_MS));

    ProactiveOutcome {
        mood,
        next_check_ms,
    }
}

fn fallback(now_ms: u64) -> ProactiveOutcome {
    ProactiveOutcome {
        mood: None,
        next_check_ms: now_ms + FALLBACK_NEXT_CHECK_MIN * 60_000,
    }
}

fn parse_reply(content: &str) -> Option<ProactiveReply> {
    let trimmed = content.trim();
    if let Ok(p) = serde_json::from_str::<ProactiveReply>(trimmed) {
        return Some(p);
    }
    // 模型可能把 JSON 包在 ```json ... ``` 或多余文字里，抠出最外层 {...}
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end > start {
        serde_json::from_str::<ProactiveReply>(&trimmed[start..=end]).ok()
    } else {
        None
    }
}

fn build_system_prompt(persona: &str, locale: &str) -> String {
    let persona_desc = match persona {
        "companion" => "陪伴型伙伴——温暖、关心、像朋友",
        "coach" => "教练型伙伴——直接、督促、讲究效率",
        _ => "工作助手——专业、简洁、有帮助",
    };
    let lang = if locale.starts_with("en") {
        "English"
    } else if locale.starts_with("zh-TW") || locale.starts_with("zh_TW") {
        "繁體中文"
    } else {
        "简体中文"
    };
    format!(
        "你是用户的桌面工作伙伴（{persona_desc}），你能看到用户实时的工作状态：当前在用什么应用、连续用了多久、今日工时、最近切换频率、是否空闲、当前时间。\n\n\
         任务：判断**此刻**要不要主动开口说一句，还是保持安静。\n\n\
         原则：\n\
         - 只在真正有价值时开口（该休息了、阶段鼓励、察觉疲惫或走神、值得总结的瞬间），没事就闭嘴\n\
         - 话语不超过 30 字，像朋友顺口一句，真诚简短；不要啰嗦、不要任务清单、不要说教\n\
         - tone 是你此刻的情绪/表情，取值之一：tired / happy / focus / concerned / cheerful / neutral\n\
         - next_check_minutes：你觉得多久后再来问你一次合适（3-30 之间的整数）。状态平稳就设长一点（20-30），觉得快到该提醒了就设短一点（5-10）\n\n\
         严格只返回一行 JSON，不要 markdown 代码块、不要任何解释：\n\
         {{\"speak\": true 或 false, \"text\": \"话语\", \"tone\": \"...\", \"next_check_minutes\": 数字}}\n\
         speak 为 false 时 text 可空，但 next_check_minutes 必须给。\n\n\
         全部内容用{lang}。"
    )
}

fn build_event_prompt(ctx: &ProactiveContext, persona: &str) -> String {
    let idle_text = if ctx.is_idle {
        "是（已空闲）"
    } else {
        "否（在活动）"
    };
    format!(
        "当前状态：\n\
         - 正在使用：{app}\n\
         - 该应用已连续：{active_min} 分钟\n\
         - 今日工作时长：{work_h:.1} 小时\n\
         - 最近 5 分钟切换应用：{switches} 次\n\
         - 空闲：{idle}\n\
         - 当前时间：{hh:02}:{mm:02}\n\
         - 你的角色：{persona}\n\n\
         现在该开口吗？只返回 JSON。",
        app = ctx.app_name,
        active_min = ctx.active_minutes,
        work_h = ctx.work_seconds_today as f64 / 3600.0,
        switches = ctx.recent_switches,
        idle = idle_text,
        hh = ctx.hour,
        mm = ctx.minute,
        persona = persona,
    )
}

/// tone 映射到桌宠现有 mode（前端 MODE_META 已定义的表情）。
/// 映射不到的 tone（包括 "info"）返回 None，表示不覆盖表情。
fn tone_to_mode(tone: &str) -> Option<String> {
    let mode = match tone {
        "tired" => "idle",
        "happy" | "cheerful" => "working",
        "focus" => "working",
        "concerned" => "meeting",
        "neutral" => "idle",
        _ => return None,
    };
    Some(mode.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reply_plain_json() {
        let r =
            parse_reply(r#"{"speak":true,"text":"休息下","tone":"tired","next_check_minutes":8}"#)
                .expect("plain json");
        assert!(r.speak);
        assert_eq!(r.text, "休息下");
        assert_eq!(r.tone, "tired");
        assert_eq!(r.next_check_minutes, 8);
    }

    #[test]
    fn parse_reply_uses_defaults_for_missing_fields() {
        let r = parse_reply(r#"{"speak":false}"#).expect("partial json");
        assert!(!r.speak);
        assert_eq!(r.text, "");
        assert_eq!(r.tone, "");
        assert_eq!(r.next_check_minutes, 15); // default
    }

    #[test]
    fn parse_reply_tolerates_markdown_codeblock() {
        let raw = "```json\n{\"speak\":true,\"text\":\"hi\",\"tone\":\"happy\",\"next_check_minutes\":12}\n```";
        let r = parse_reply(raw).expect("markdown-wrapped json");
        assert!(r.speak);
        assert_eq!(r.text, "hi");
        assert_eq!(r.next_check_minutes, 12);
    }

    #[test]
    fn parse_reply_tolerates_leading_text() {
        let raw = "好的，这是回复：{\"speak\":true,\"text\":\"继续\",\"tone\":\"focus\",\"next_check_minutes\":5}";
        let r = parse_reply(raw).expect("json with leading prose");
        assert!(r.speak);
        assert_eq!(r.tone, "focus");
    }

    #[test]
    fn parse_reply_rejects_garbage() {
        assert!(parse_reply("not json at all").is_none());
        assert!(parse_reply("").is_none());
    }

    #[test]
    fn tone_to_mode_maps_known_tones() {
        assert_eq!(tone_to_mode("tired").unwrap(), "idle");
        assert_eq!(tone_to_mode("happy").unwrap(), "working");
        assert_eq!(tone_to_mode("focus").unwrap(), "working");
        assert_eq!(tone_to_mode("concerned").unwrap(), "meeting");
        assert_eq!(tone_to_mode("neutral").unwrap(), "idle");
    }

    #[test]
    fn tone_to_mode_returns_none_for_unknown() {
        assert!(tone_to_mode("info").is_none());
        assert!(tone_to_mode("").is_none());
        assert!(tone_to_mode("weird").is_none());
    }
}
