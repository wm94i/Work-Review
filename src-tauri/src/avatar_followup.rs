use crate::monitor::ActiveWindow;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};
use work_review_core::config::AvatarFollowupItem;
#[cfg(test)]
use work_review_core::database::Activity;
#[cfg(test)]
use work_review_core::work_intelligence::build_work_sessions;
use work_review_core::work_intelligence::WorkSession;

pub const AVATAR_FOLLOWUP_EVENT: &str = "avatar-followup-suggestion";
const FOLLOWUP_LOOKBACK_DAYS: i64 = 3;
const MIN_SESSION_DURATION_SECONDS: i64 = 5 * 60;
const MIN_SESSION_AGE_SECONDS: i64 = 8 * 60;
const SAME_PROJECT_EMIT_COOLDOWN_MS: u64 = 20 * 60 * 1000;
const REMEMBER_COOLDOWN_MS: u64 = 36 * 60 * 60 * 1000;
const TIMELINE_COOLDOWN_MS: u64 = 90 * 60 * 1000;
const FOCUS_COOLDOWN_MS: u64 = 25 * 60 * 1000;
const DISMISS_COOLDOWN_MS: u64 = 6 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvatarFollowupSuggestionPayload {
    pub project_key: String,
    pub date: String,
    pub title: String,
    pub source_app: String,
    pub source_title: String,
    pub intent_label: String,
    pub confidence: i32,
    pub persona: String,
    pub session_age_hours: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarFollowupAction {
    Timeline,
    Focus,
    Remember,
    Snooze,
    Dismiss,
}

#[derive(Default)]
struct AvatarFollowupRuntime {
    snoozed_until_by_project_key: HashMap<String, u64>,
    last_emitted_project_key: Option<String>,
    last_emitted_at_ms: u64,
}

static FOLLOWUP_RUNTIME: Lazy<Mutex<AvatarFollowupRuntime>> =
    Lazy::new(|| Mutex::new(AvatarFollowupRuntime::default()));

pub fn emit_followup_suggestion(app: &AppHandle, payload: &AvatarFollowupSuggestionPayload) {
    let _ = app.emit_to(
        crate::avatar_engine::AVATAR_WINDOW_LABEL,
        AVATAR_FOLLOWUP_EVENT,
        payload,
    );
}

pub fn should_emit_followup(project_key: &str, now_ms: u64) -> bool {
    let runtime = FOLLOWUP_RUNTIME.lock().unwrap_or_else(|e| e.into_inner());

    if runtime
        .snoozed_until_by_project_key
        .get(project_key)
        .is_some_and(|until| *until > now_ms)
    {
        return false;
    }

    if runtime.last_emitted_project_key.as_deref() == Some(project_key)
        && now_ms.saturating_sub(runtime.last_emitted_at_ms) < SAME_PROJECT_EMIT_COOLDOWN_MS
    {
        return false;
    }

    true
}

pub fn note_followup_emitted(project_key: &str, now_ms: u64) {
    let mut runtime = FOLLOWUP_RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    runtime.last_emitted_project_key = Some(project_key.to_string());
    runtime.last_emitted_at_ms = now_ms;
}

pub fn apply_followup_action(project_key: &str, action: AvatarFollowupAction, persona: &str) {
    let now_ms = now_ms();
    let cooldown_ms = match action {
        AvatarFollowupAction::Timeline => TIMELINE_COOLDOWN_MS,
        AvatarFollowupAction::Focus => FOCUS_COOLDOWN_MS,
        AvatarFollowupAction::Remember => REMEMBER_COOLDOWN_MS,
        AvatarFollowupAction::Snooze => persona_snooze_minutes(persona) * 60 * 1000,
        AvatarFollowupAction::Dismiss => DISMISS_COOLDOWN_MS,
    };

    let mut runtime = FOLLOWUP_RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    runtime
        .snoozed_until_by_project_key
        .retain(|_, until| *until > now_ms);
    runtime
        .snoozed_until_by_project_key
        .insert(project_key.to_string(), now_ms.saturating_add(cooldown_ms));
    runtime.last_emitted_project_key = Some(project_key.to_string());
    runtime.last_emitted_at_ms = now_ms;
}

#[cfg(test)]
pub fn find_followup_suggestion(
    activities: &[Activity],
    active_window: &ActiveWindow,
    persona: &str,
    manual_followups: &[AvatarFollowupItem],
    now_ts: i64,
    now_ms: u64,
) -> Option<AvatarFollowupSuggestionPayload> {
    let sessions = build_work_sessions(activities);
    find_followup_suggestion_from_sessions(
        &sessions,
        active_window,
        persona,
        manual_followups,
        now_ts,
        now_ms,
    )
}

/// 基于已构建的 `WorkSession` 切片查找最匹配的待跟进建议（避免高频切窗时重复重算会话与分词）。
pub fn find_followup_suggestion_from_sessions(
    sessions: &[WorkSession],
    active_window: &ActiveWindow,
    persona: &str,
    manual_followups: &[AvatarFollowupItem],
    now_ts: i64,
    now_ms: u64,
) -> Option<AvatarFollowupSuggestionPayload> {
    let current_context = CurrentContext::from_active_window(active_window)?;
    let threshold = persona_score_threshold(persona);

    let mut best: Option<(i32, AvatarFollowupSuggestionPayload)> = None;

    for session in sessions {
        if !is_session_basic_eligible(session, now_ts) {
            continue;
        }

        // 每个会话只执行一次正则分词与 project_key 构造，避免原先 4 次重复正则扫描。
        let session_title_tokens = significant_tokens(&session.title);
        let project_key = session_project_key_with_tokens(session, &session_title_tokens);
        if has_open_manual_followup(&project_key, manual_followups, now_ts) {
            continue;
        }

        let score = score_session_against_context(
            session,
            &project_key,
            &session_title_tokens,
            &current_context,
        );
        if score < threshold {
            continue;
        }

        // 跳过冷却中的项目（被 snooze 或同项目刚提醒过），避免"高分但冷却中"的项
        // 占据唯一推荐位、遮蔽掉次高分但仍可提醒的项目。
        if !should_emit_followup(&project_key, now_ms) {
            continue;
        }

        let payload = AvatarFollowupSuggestionPayload {
            project_key,
            date: session.date.clone(),
            title: session.title.clone(),
            source_app: session.dominant_app.clone(),
            source_title: session.title.clone(),
            intent_label: session.intent_label.clone(),
            confidence: score.clamp(0, 100),
            persona: persona.to_string(),
            session_age_hours: session_age_hours(session, now_ts),
        };

        let replace = best
            .as_ref()
            .map(|(best_score, best_payload)| {
                score > *best_score || (score == *best_score && payload.date >= best_payload.date)
            })
            .unwrap_or(true);

        if replace {
            best = Some((score, payload));
        }
    }

    best.map(|(_, payload)| payload)
}

pub fn persona_snooze_minutes(persona: &str) -> u64 {
    match persona.trim() {
        "companion" => 120,
        "coach" => 30,
        _ => 60,
    }
}

fn persona_score_threshold(persona: &str) -> i32 {
    match persona.trim() {
        "companion" => 74,
        "coach" => 52,
        _ => 62,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone)]
struct CurrentContext {
    app_name: String,
    project_key: String,
    title_tokens: Vec<String>,
    domain: Option<String>,
}

impl CurrentContext {
    fn from_active_window(active_window: &ActiveWindow) -> Option<Self> {
        let app_name = active_window.app_name.trim().to_string();
        if app_name.is_empty() {
            return None;
        }

        let title_tokens = significant_tokens(&active_window.window_title);
        let domain = active_window
            .browser_url
            .as_deref()
            .and_then(normalize_domain_from_url);
        let project_key = build_project_key(
            &app_name,
            domain.as_deref(),
            &title_tokens,
            active_window.window_title.as_str(),
        )?;

        Some(Self {
            app_name,
            project_key,
            title_tokens,
            domain,
        })
    }
}

fn is_session_basic_eligible(session: &WorkSession, now_ts: i64) -> bool {
    if session.duration < MIN_SESSION_DURATION_SECONDS {
        return false;
    }

    if now_ts - session.end_timestamp < MIN_SESSION_AGE_SECONDS {
        return false;
    }

    if !matches!(
        session.intent_label.as_str(),
        "编码开发"
            | "代码评审"
            | "需求文档"
            | "问题排查"
            | "测试验证"
            | "学习调研"
            | "AI 协作"
            | "项目管理"
            | "通用工作"
    ) {
        return false;
    }

    !session.date.is_empty() && !session.title.trim().is_empty()
}

fn has_open_manual_followup(
    project_key: &str,
    manual_followups: &[AvatarFollowupItem],
    now_ts: i64,
) -> bool {
    manual_followups.iter().any(|item| {
        item.status == "open"
            && item.project_key == project_key
            && now_ts - item.created_at <= FOLLOWUP_LOOKBACK_DAYS * 24 * 60 * 60
    })
}

fn score_session_against_context(
    session: &WorkSession,
    session_project_key: &str,
    session_title_tokens: &[String],
    current: &CurrentContext,
) -> i32 {
    let mut score = 0;

    if normalize_match_text(&session.dominant_app) == normalize_match_text(&current.app_name) {
        score += 18;
    }

    if session_project_key == current.project_key {
        score += 44;
    } else if session_project_key.contains(&current.project_key)
        || current.project_key.contains(session_project_key)
    {
        score += 26;
    }

    if current
        .domain
        .as_ref()
        .is_some_and(|domain| session.browser_domains.iter().any(|item| item == domain))
    {
        score += 32;
    }

    let title_overlap = overlap_count(&current.title_tokens, session_title_tokens);
    score += (title_overlap as i32 * 9).min(27);

    let keyword_overlap = overlap_count(&current.title_tokens, &session.top_keywords);
    score += (keyword_overlap as i32 * 4).min(12);

    if title_overlap > 0 && keyword_overlap > 0 {
        score += 6;
    }

    score
}

fn session_age_hours(session: &WorkSession, now_ts: i64) -> u32 {
    let diff = (now_ts - session.end_timestamp).max(0);
    ((diff + 3599) / 3600).try_into().unwrap_or(u32::MAX)
}

#[cfg(test)]
fn session_project_key(session: &WorkSession) -> String {
    let tokens = significant_tokens(&session.title);
    session_project_key_with_tokens(session, &tokens)
}

fn session_project_key_with_tokens(session: &WorkSession, title_tokens: &[String]) -> String {
    build_project_key(
        &session.dominant_app,
        session.browser_domains.first().map(String::as_str),
        title_tokens,
        &session.title,
    )
    .unwrap_or_else(|| normalize_match_text(&session.dominant_app))
}

fn build_project_key(
    app_name: &str,
    domain: Option<&str>,
    title_tokens: &[String],
    fallback_title: &str,
) -> Option<String> {
    let app = normalize_match_text(app_name);
    if app.is_empty() {
        return None;
    }

    let mut parts = vec![app];
    if let Some(domain) = domain {
        let normalized_domain = normalize_match_text(domain);
        if !normalized_domain.is_empty() {
            parts.push(normalized_domain);
        }
    }

    let title_part = title_tokens
        .iter()
        .take(3)
        .map(|item| normalize_match_text(item))
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();

    if title_part.is_empty() {
        let fallback = significant_tokens(fallback_title)
            .into_iter()
            .take(2)
            .collect::<Vec<_>>();
        parts.extend(fallback);
    } else {
        parts.extend(title_part);
    }

    Some(parts.join("::"))
}

fn overlap_count(left: &[String], right: &[String]) -> usize {
    left.iter()
        .filter(|item| right.iter().any(|candidate| candidate == *item))
        .count()
}

fn normalize_match_text(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .replace(
            |ch: char| !ch.is_ascii_alphanumeric() && !('\u{4e00}'..='\u{9fff}').contains(&ch),
            " ",
        )
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_domain_from_url(value: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(value).ok()?;
    let host = parsed.host_str()?.trim().to_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn significant_tokens(value: &str) -> Vec<String> {
    static TOKEN_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = TOKEN_REGEX.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9]+|[\u4e00-\u9fff]{2,}").expect("token regex should compile")
    });
    let stop_words = [
        "google",
        "chrome",
        "mozilla",
        "firefox",
        "safari",
        "edge",
        "cursor",
        "code",
        "visual",
        "studio",
        "work",
        "review",
        "文档",
        "页面",
        "项目",
        "任务",
        "工作",
        "窗口",
        "编辑器",
        "today",
        "issue",
        "pull",
        "request",
    ];

    regex
        .find_iter(value)
        .map(|mat| mat.as_str().trim().to_lowercase())
        .filter(|token| token.len() >= 2 && !stop_words.contains(&token.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        apply_followup_action, find_followup_suggestion, normalize_domain_from_url,
        session_project_key, should_emit_followup, significant_tokens, AvatarFollowupAction,
        AvatarFollowupRuntime, FOLLOWUP_RUNTIME,
    };
    use crate::monitor::ActiveWindow;
    use chrono::TimeZone;
    use once_cell::sync::Lazy;
    use std::sync::Mutex;
    use work_review_core::config::AvatarFollowupItem;
    use work_review_core::database::Activity;
    use work_review_core::work_intelligence::build_work_sessions;

    static FOLLOWUP_TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn sample_activity(
        timestamp: i64,
        app_name: &str,
        title: &str,
        url: Option<&str>,
        duration: i64,
    ) -> Activity {
        Activity {
            id: None,
            timestamp,
            app_name: app_name.to_string(),
            window_title: title.to_string(),
            screenshot_path: String::new(),
            ocr_text: None,
            category: "development".to_string(),
            duration,
            browser_url: url.map(str::to_string),
            executable_path: None,
            semantic_category: Some("编码开发".to_string()),
            semantic_confidence: Some(90),
            screenshot_url: None,
        }
    }

    fn reset_followup_runtime() {
        let mut runtime = FOLLOWUP_RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
        *runtime = AvatarFollowupRuntime::default();
    }

    #[test]
    fn 显著token提取应保留项目关键信号() {
        let tokens = significant_tokens("PR #128 · 支付回调修复 - Google Chrome");
        assert!(tokens.iter().any(|item| item == "128"));
        assert!(tokens.iter().any(|item| item == "支付回调修复"));
    }

    #[test]
    fn 域名提取应返回标准host() {
        assert_eq!(
            normalize_domain_from_url("https://github.com/wm94i/Work_Review/pull/128"),
            Some("github.com".to_string())
        );
    }

    #[test]
    fn 会话项目key应稳定包含应用和域名() {
        let sessions = build_work_sessions(&[sample_activity(
            1_710_000_000,
            "Google Chrome",
            "PR #128 · 支付回调修复",
            Some("https://github.com/wm94i/Work_Review/pull/128"),
            900,
        )]);

        let key = session_project_key(&sessions[0]);
        assert!(key.contains("google-chrome"));
        assert!(key.contains("github-com"));
    }

    #[test]
    fn 回到同类项目时应命中继续建议() {
        let _guard = FOLLOWUP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_followup_runtime();
        let activities = vec![
            sample_activity(
                1_710_000_000,
                "Google Chrome",
                "PR #128 · 支付回调修复",
                Some("https://github.com/wm94i/Work_Review/pull/128"),
                1200,
            ),
            sample_activity(1_710_000_100, "Cursor", "payments.ts", None, 900),
        ];
        let active_window = ActiveWindow {
            app_name: "Google Chrome".to_string(),
            window_title: "PR #128 · 支付回调修复".to_string(),
            browser_url: Some("https://github.com/wm94i/Work_Review/pull/128".to_string()),
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
            is_private: false,
        };

        let suggestion = find_followup_suggestion(
            &activities,
            &active_window,
            "assistant",
            &[],
            1_710_020_000,
            0,
        )
        .expect("should match");

        // session.date 基于 start_timestamp（activity.timestamp - duration），
        // 因为 work session 语义已改为 [timestamp-duration, timestamp]。
        // 第一个 activity 的 duration=1200，start = 1710000000 - 1200 = 1709998800。
        let expected_date = chrono::Local
            .timestamp_opt(1_710_000_000 - 1200, 0)
            .earliest()
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        assert_eq!(suggestion.date, expected_date);
        assert_eq!(suggestion.persona, "assistant");
        assert!(suggestion.confidence >= 62);
    }

    #[test]
    fn 已手动记为待跟进的项目不应重复提醒() {
        let _guard = FOLLOWUP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_followup_runtime();
        let activities = vec![sample_activity(
            1_710_000_000,
            "Google Chrome",
            "PR #128 · 支付回调修复",
            Some("https://github.com/wm94i/Work_Review/pull/128"),
            1200,
        )];
        let active_window = ActiveWindow {
            app_name: "Google Chrome".to_string(),
            window_title: "PR #128 · 支付回调修复".to_string(),
            browser_url: Some("https://github.com/wm94i/Work_Review/pull/128".to_string()),
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
            is_private: false,
        };
        let sessions = build_work_sessions(&activities);
        let project_key = session_project_key(&sessions[0]);
        let followups = vec![AvatarFollowupItem {
            id: "1".to_string(),
            title: "修复支付回调".to_string(),
            date: "2024-03-09".to_string(),
            source_app: "Google Chrome".to_string(),
            source_title: "PR #128 · 支付回调修复".to_string(),
            project_key,
            created_at: 1_710_010_000,
            status: "open".to_string(),
        }];

        assert!(find_followup_suggestion(
            &activities,
            &active_window,
            "assistant",
            &followups,
            1_710_020_000,
            0
        )
        .is_none());
    }

    #[test]
    fn 稍后提醒动作应阻止短时间重复触发() {
        let _guard = FOLLOWUP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_followup_runtime();
        let project_key = "cursor::github-com::支付回调修复";
        assert!(should_emit_followup(project_key, 10_000));
        apply_followup_action(project_key, AvatarFollowupAction::Snooze, "assistant");
        assert!(!should_emit_followup(project_key, 10_001));
    }

    #[test]
    fn 开始专注动作应在专注时段内阻止重复提醒() {
        let _guard = FOLLOWUP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_followup_runtime();
        let project_key = "cursor::github-com::支付回调修复";
        assert!(should_emit_followup(project_key, 20_000));
        apply_followup_action(project_key, AvatarFollowupAction::Focus, "assistant");
        assert!(!should_emit_followup(project_key, 20_001));
    }

    #[test]
    fn 冷却期内重复扫描同一会话不应再次返回建议() {
        let _guard = FOLLOWUP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_followup_runtime();
        let activities = vec![sample_activity(
            1_710_000_000,
            "Google Chrome",
            "PR #128 · 支付回调修复",
            Some("https://github.com/wm94i/Work_Review/pull/128"),
            1200,
        )];
        let active_window = ActiveWindow {
            app_name: "Google Chrome".to_string(),
            window_title: "PR #128 · 支付回调修复".to_string(),
            browser_url: Some("https://github.com/wm94i/Work_Review/pull/128".to_string()),
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
            is_private: false,
        };
        let sessions = build_work_sessions(&activities);
        let first = super::find_followup_suggestion_from_sessions(
            &sessions,
            &active_window,
            "assistant",
            &[],
            1_710_020_000,
            100_000,
        )
        .expect("首次扫描应命中待跟进建议");
        super::note_followup_emitted(&first.project_key, 100_000);

        let second = super::find_followup_suggestion_from_sessions(
            &sessions,
            &active_window,
            "assistant",
            &[],
            1_710_020_000,
            102_000,
        );
        assert!(second.is_none(), "冷却期内二次扫描必须跳过同一项目");
    }
}
