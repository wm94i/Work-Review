//! Agent Run / Event / Audit 状态机与持久化 DTO。
//!
//! 严禁把原始 OCR、完整窗口标题列表、未脱敏工具正文或明文凭证写入持久化账本；
//! 所有落盘文本预览均经 [`scrub_preview`] 做凭证脱敏、空白折叠与安全截断。

use serde::{Deserialize, Serialize};

/// 单次 Agent Run 的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    WaitingApproval,
    Completed,
    Cancelled,
    Failed,
    TimedOut,
}

impl RunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        }
    }
}

/// 持久化事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessEventKind {
    RunStarted,
    ContextCompacted,
    PolicyEvaluated,
    ApprovalRequested,
    ApprovalResolved,
    ArtifactStored,
    ToolCompleted,
    RunCompleted,
    RunCancelled,
    RunFailed,
}

impl HarnessEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunStarted => "run_started",
            Self::ContextCompacted => "context_compacted",
            Self::PolicyEvaluated => "policy_evaluated",
            Self::ApprovalRequested => "approval_requested",
            Self::ApprovalResolved => "approval_resolved",
            Self::ArtifactStored => "artifact_stored",
            Self::ToolCompleted => "tool_completed",
            Self::RunCompleted => "run_completed",
            Self::RunCancelled => "run_cancelled",
            Self::RunFailed => "run_failed",
        }
    }
}

/// 单次 Agent Run 概要记录（对应 SQLite `agent_runs` 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunSummary {
    pub id: String,
    pub conversation_id: Option<i64>,
    pub source: String,
    pub request_mode: String,
    pub status: String,
    pub model_name: Option<String>,
    pub input_preview: String,
    pub answer_preview: Option<String>,
    pub taint_flags: String,
    pub compaction_count: usize,
    pub tool_call_count: usize,
    pub error_message: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// 单条持久化 Run 事件（对应 SQLite `agent_run_events` 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunEvent {
    pub id: i64,
    pub run_id: String,
    pub seq: i64,
    pub event_kind: String,
    pub tool_name: Option<String>,
    pub payload_json: String,
    pub created_at: i64,
}

/// 单条工具权限与执行审计记录（对应 SQLite `agent_tool_audit` 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolAuditEntry {
    pub id: i64,
    pub run_id: String,
    pub tool_name: String,
    pub risk_tier: String,
    pub decision: String,
    pub taint_flags: String,
    pub ok: bool,
    pub duration_ms: i64,
    pub reason: Option<String>,
    pub created_at: i64,
}

/// 完整单次 Run Trace 视图（供前端/调试检查器读取）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunTrace {
    pub run: AgentRunSummary,
    pub events: Vec<AgentRunEvent>,
    pub audits: Vec<AgentToolAuditEntry>,
}

fn looks_like_secret_token(raw_token: &str) -> bool {
    let trimmed = raw_token.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | ',' | ';' | ')' | '(' | ']' | '[' | '{' | '}' | '.' | '。' | '，'
        )
    });
    let known_prefixes = [
        "sk-", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "glpat-", "xoxb-", "xoxp-", "xapp-", "AIza",
        "AKIA", "tvly-", "hf_",
    ];
    if trimmed.len() >= 8
        && known_prefixes
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
    {
        return true;
    }
    if trimmed.len() < 12 {
        return false;
    }
    // JWT 三段式特征：eyJ... . ... . ...
    if trimmed.starts_with("eyJ") {
        let mut parts = trimmed.split('.');
        if let (Some(h), Some(p), Some(s), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        {
            if h.len() >= 6 && p.len() >= 6 && s.len() >= 6 {
                return true;
            }
        }
    }
    false
}

fn is_sensitive_key_label(label: &str) -> bool {
    let compact: String = label
        .to_lowercase()
        .chars()
        .filter(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '_' | '-'
                        | ':'
                        | '：'
                        | '='
                        | '"'
                        | '\''
                        | '{'
                        | '}'
                        | '['
                        | ']'
                        | ','
                        | '('
                        | ')'
                )
        })
        .collect();
    matches!(
        compact.as_str(),
        "password"
            | "passwd"
            | "passcode"
            | "passphrase"
            | "pwd"
            | "secret"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "apikey"
            | "authorization"
            | "cookie"
            | "sessionid"
            | "privatekey"
            | "密码"
            | "口令"
            | "密钥"
            | "令牌"
            | "私钥"
    ) || ["apikey", "secret", "secretkey", "secretaccesskey", "token"]
        .iter()
        .any(|suffix| compact.ends_with(suffix))
}

fn is_auth_scheme_token(raw: &str) -> bool {
    let scheme_trimmed = raw
        .trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '(' | '[' | '{' | ':' | '：' | ',' | ';' | '.'
            )
        })
        .to_ascii_lowercase();
    scheme_trimmed == "bearer" || scheme_trimmed == "basic"
}

fn redact_kv_segment(seg: &str, depth: u8) -> (String, bool) {
    if is_auth_scheme_token(seg) {
        return (seg.to_string(), true);
    }
    if looks_like_secret_token(seg) {
        return ("[REDACTED_TOKEN]".to_string(), false);
    }

    // 处理形如 `api_key=xxx` / `{"password":"xxx"}` / `{"nested":{"token":"xxx"}}` / `Authorization:Bearer xxx`
    if let Some((sep_idx, sep_ch)) = seg
        .char_indices()
        .find(|(_, ch)| matches!(*ch, '=' | ':' | '：'))
    {
        let key_part = &seg[..sep_idx];
        let val_part = &seg[sep_idx + sep_ch.len_utf8()..];
        if is_sensitive_key_label(key_part) {
            let val_trimmed = val_part.trim_matches(|c: char| matches!(c, '"' | '\'' | ' '));
            if val_trimmed.is_empty()
                || val_trimmed.eq_ignore_ascii_case("bearer")
                || val_trimmed.eq_ignore_ascii_case("basic")
            {
                return (format!("{key_part}{sep_ch}"), true);
            }
            return (format!("{key_part}{sep_ch}[REDACTED]"), false);
        }
        if looks_like_secret_token(val_part) {
            return (format!("{key_part}{sep_ch}[REDACTED_TOKEN]"), false);
        }
        if depth < 8 && val_part.contains([':', '=', '：']) {
            let (redacted_val, next) = redact_kv_segment(val_part, depth + 1);
            return (format!("{key_part}{sep_ch}{redacted_val}"), next);
        }
    }

    if is_sensitive_key_label(seg) {
        return (seg.to_string(), true);
    }

    (seg.to_string(), false)
}

fn redact_compound_token(tok: &str) -> (String, bool) {
    // 先按常见紧凑结构分隔符（JSON 逗号、查询串 `?`/`&`、分号）分段，保留原分隔符
    if !tok.contains([',', ';', '&', '?']) {
        return redact_kv_segment(tok, 0);
    }
    let mut out = String::with_capacity(tok.len());
    let mut redact_next = false;
    let mut start = 0usize;

    for (idx, ch) in tok.char_indices() {
        if matches!(ch, ',' | ';' | '&' | '?') {
            let seg = &tok[start..idx];
            if !seg.is_empty() {
                if redact_next && !is_auth_scheme_token(seg) {
                    out.push_str("[REDACTED]");
                    redact_next = false;
                } else {
                    let (redacted, next) = redact_kv_segment(seg, 0);
                    out.push_str(&redacted);
                    redact_next = next;
                }
            }
            out.push(ch);
            start = idx + ch.len_utf8();
        }
    }
    let tail = &tok[start..];
    if !tail.is_empty() {
        if redact_next && !is_auth_scheme_token(tail) {
            out.push_str("[REDACTED]");
            redact_next = false;
        } else {
            let (redacted, next) = redact_kv_segment(tail, 0);
            out.push_str(&redacted);
            redact_next = next;
        }
    }

    (out, redact_next)
}

// 在空白及逗号分词前清洗完整引号值，避免多词口令只遮住第一段。
fn redact_quoted_values(text: &str) -> String {
    let mut chars = text.char_indices().peekable();
    let mut sensitive_key = false;
    let mut sensitive_value = false;
    let mut copied_through = 0;
    let mut out = String::with_capacity(text.len());
    while let Some((start, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        match ch {
            '\'' if start > 0 && text[..start].ends_with(char::is_alphanumeric) => {
                sensitive_key = false;
                sensitive_value = false;
            }
            '"' | '\'' => {
                let mut end = text.len();
                let mut escaped = false;
                for (idx, next) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if next == '\\' {
                        escaped = true;
                    } else if next == ch {
                        end = idx + next.len_utf8();
                        break;
                    }
                }
                if sensitive_value {
                    out.push_str(&text[copied_through..start]);
                    out.push_str("\"[REDACTED]\"");
                    copied_through = end;
                    sensitive_key = false;
                } else {
                    let raw = &text[start..end];
                    let label = serde_json::from_str::<String>(raw)
                        .unwrap_or_else(|_| raw.trim_matches(['"', '\'']).to_string());
                    sensitive_key = is_sensitive_key_label(&label);
                }
                sensitive_value = false;
            }
            ':' | '=' | '：' => {
                sensitive_value = sensitive_key;
                sensitive_key = false;
            }
            _ if ch.is_alphanumeric() || matches!(ch, '_' | '-') => {
                let mut end = start + ch.len_utf8();
                while let Some(&(idx, next)) = chars.peek() {
                    if !next.is_alphanumeric() && !matches!(next, '_' | '-') {
                        break;
                    }
                    chars.next();
                    end = idx + next.len_utf8();
                }
                sensitive_key = is_sensitive_key_label(&text[start..end]);
                sensitive_value = false;
            }
            _ => {
                sensitive_key = false;
                sensitive_value = false;
            }
        }
    }
    out.push_str(&text[copied_through..]);
    out
}

/// 对文本中的常见明文凭证（API Key、Bearer/Basic、JWT、PEM 私钥、键值与紧凑 JSON 口令）做保守脱敏。
pub fn redact_secrets(text: &str) -> String {
    if text.contains("-----BEGIN ") && text.contains(" PRIVATE KEY-----") {
        return "[REDACTED_PRIVATE_KEY]".to_string();
    }

    let quoted_redacted = redact_quoted_values(text);
    let tokens = quoted_redacted.split_whitespace();
    let mut out: Vec<String> = Vec::new();
    let mut redact_next = false;

    for tok in tokens {
        if is_auth_scheme_token(tok) {
            out.push(tok.to_string());
            redact_next = true;
            continue;
        }

        if redact_next {
            out.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }

        let (redacted, next) = redact_compound_token(tok);
        out.push(redacted);
        redact_next = next;
    }

    out.join(" ")
}

/// 将任意文本做凭证脱敏、空白折叠并在 UTF-8 字符边界安全截断为短预览。
pub fn scrub_preview(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let cleaned: String = text
        .chars()
        .filter(|ch| !ch.is_control() || ch.is_whitespace())
        .collect();
    let collapsed = redact_secrets(&cleaned);
    let count = collapsed.chars().count();
    if count <= max_chars {
        collapsed
    } else {
        let mut head: String = collapsed
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect();
        head.push('…');
        head
    }
}

/// 生成新的 Run ID。
pub fn new_run_id() -> String {
    format!("run_{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_preview应折叠空白并在utf8边界截断() {
        let s = "第一行\n\n  第二行   中文内容测试";
        assert_eq!(scrub_preview(s, 8), "第一行 第二行…");
        assert_eq!(scrub_preview("短文本", 20), "短文本");
        assert_eq!(scrub_preview("任意文本", 0), "");
    }

    #[test]
    fn 多词及含转义符的引号凭证必须完整脱敏() {
        let samples = [
            r#"{"passcode":"purple horse battery staple"}"#,
            r#"错误: {"nested":{"token": "purple,horse;battery&staple"}}"#,
            r#"token='purple horse battery staple'"#,
            r#"api_key = "purple \"horse\" battery\\staple""#,
            r#"{"pass\u0063ode":"purple horse battery staple"}"#,
            r#"{"client_secret":"purple horse battery staple"}"#,
            r#"{"api_token":"purple horse battery staple"}"#,
            r#"{"passphrase":"purple horse battery staple"}"#,
            "user's error: token=\"purple horse battery staple\"",
            "token=\"purple horse battery staple",
            "口令：\"purple horse battery staple\"",
        ];
        for raw in samples {
            let safe = scrub_preview(raw, 512);
            assert!(safe.contains("REDACTED"), "应清洗凭证: {safe}");
            for part in ["purple", "horse", "battery", "staple"] {
                assert!(!safe.contains(part), "引号凭证残留: {safe}");
            }
        }
        let normal = r#"{"title":"normal weekly review","count":3}"#;
        assert_eq!(scrub_preview(normal, 512), normal);
    }

    #[test]
    fn scrub_preview应脱敏常见短凭证密钥与私钥() {
        let samples = [
            ("Bearer super_secret_token_999", "super_secret_token_999"),
            ("Bearer,comma_tok_999", "comma_tok_999"),
            ("Basic;semi_tok_999", "semi_tok_999"),
            ("Authorization: Bearer short_tok_999", "short_tok_999"),
            ("Authorization:Bearer short_tok_888", "short_tok_888"),
            (
                "{\"Authorization\": \"Bearer short_tok_777\"}",
                "short_tok_777",
            ),
            (
                "{\"Authorization\":\"Bearer short_tok_666\"}",
                "short_tok_666",
            ),
            ("{\"password\":\"hunter2\"}", "hunter2"),
            (
                "{\"user\":\"alice\",\"api_key\":\"abc123xyz\"}",
                "abc123xyz",
            ),
            ("{\"nested\":{\"token\":\"deepSecret1\"}}", "deepSecret1"),
            ("https://example.com/rpc?token=qtok999&a=1", "qtok999"),
            ("令牌 Bearer secret_tok_999", "secret_tok_999"),
            ("我的 key 是 sk-12345678", "sk-12345678"),
            (
                "我的 key 是 sk-proj-abcdef1234567890",
                "sk-proj-abcdef1234567890",
            ),
            ("token=ghp_1234567890abcdef", "ghp_1234567890abcdef"),
            ("password=myShortPass", "myShortPass"),
            ("密码：abc12345", "abc12345"),
            (
                "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.signature123",
                "signature123",
            ),
            (
                "-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----",
                "MIIE",
            ),
        ];
        for (raw, secret) in samples {
            let scrubbed = scrub_preview(raw, 160);
            assert!(
                !scrubbed.contains(secret),
                "凭证未被脱敏: raw={raw} scrubbed={scrubbed}"
            );
            assert!(
                scrubbed.contains("REDACTED"),
                "应标记 REDACTED: scrubbed={scrubbed}"
            );
        }
    }
}
