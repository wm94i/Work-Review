use crate::bot_common::{build_device_list, handle_cmd, status_payload, UNKNOWN_CMD_REPLY};
use crate::config::AppConfig;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::Client;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

/// 钉钉签名允许的时间窗口：±5 分钟（与企微一致；钉钉 timestamp 单位为毫秒）。
/// 钉钉回调通常秒级到达，5 分钟窗口足够；收紧后避免 ±1 小时窗口内被截获请求重放。
const DINGTALK_SIGN_WINDOW_MS: i64 = 5 * 60 * 1000;

pub struct DingtalkResponse {
    pub status: u16,
    pub body: String,
}

impl DingtalkResponse {
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            body: value.to_string(),
        }
    }

    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, &serde_json::json!({"error": message.into()}))
    }
}

/// Verify DingTalk outgoing signature: Base64(HmacSHA256(timestamp + "\n" + app_secret))
fn verify_dingtalk_sign(app_secret: &str, timestamp: &str, sign: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let message = format!("{timestamp}\n{app_secret}");
    let mut mac = match HmacSha256::new_from_slice(app_secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(message.as_bytes());
    let result = mac.finalize().into_bytes();
    let computed = BASE64.encode(result);
    // 常量时间比较，防止签名前缀的时序侧信道。
    computed.as_bytes().ct_eq(sign.as_bytes()).into()
}

/// 校验钉钉 timestamp 是否在允许窗口内（防重放）。
/// 钉钉 timestamp 是毫秒级 Unix 时间戳的字符串。
fn timestamp_within_window(timestamp_ms_str: &str, window_ms: i64) -> bool {
    let ts: i64 = match timestamp_ms_str.parse() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let now_ms = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(_) => return false,
    };
    (now_ms - ts).abs() <= window_ms
}

/// 校验 sessionWebhook 是否落在钉钉官方域名下，避免 SSRF。
fn is_allowed_dingtalk_webhook(url: &str) -> bool {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let host = match parsed.host_str() {
        Some(h) => h.to_lowercase(),
        None => return false,
    };
    host == "oapi.dingtalk.com" || host.ends_with(".dingtalk.com")
}

/// 从钉钉文本消息中剥离 @机器人 标记，但不破坏邮箱地址等中间出现的 @。
/// 规则：只剥离开头 "@xxx " 或结尾 " @xxx" 这种 xxx 内不含空格的 @标记。
fn extract_text_content(content: &str) -> String {
    let mut text = content.trim().to_string();
    // 剥离开头形如 "@bot text..." 的标记
    if let Some(stripped) = text.strip_prefix('@') {
        if let Some(space_pos) = stripped.find(char::is_whitespace) {
            text = stripped[space_pos..].trim_start().to_string();
        } else {
            // 整条消息只有 "@xxx"
            return String::new();
        }
    }
    // 剥离结尾形如 "text... @bot"（@后不含空白）
    if let Some(at_pos) = text.rfind(" @") {
        let after_at = &text[at_pos + 2..];
        if !after_at.is_empty() && !after_at.chars().any(char::is_whitespace) {
            text = text[..at_pos].to_string();
        }
    }
    text.trim().to_string()
}

pub async fn handle_dingtalk_callback(
    headers: &std::collections::HashMap<String, String>,
    body: &str,
    config: &AppConfig,
    data_dir: &Path,
) -> DingtalkResponse {
    let app_secret = match config.dingtalk_app_secret.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => return DingtalkResponse::error(403, "dingtalk_app_secret not configured"),
    };

    // 签名头必须存在；缺一即拒。
    let timestamp = headers.get("timestamp").map(|s| s.as_str()).unwrap_or("");
    let sign = headers.get("sign").map(|s| s.as_str()).unwrap_or("");
    if timestamp.is_empty() || sign.is_empty() {
        return DingtalkResponse::error(401, "missing timestamp or sign header");
    }
    // 时间窗口校验，防重放。
    if !timestamp_within_window(timestamp, DINGTALK_SIGN_WINDOW_MS) {
        return DingtalkResponse::error(401, "timestamp out of allowed window");
    }
    if !verify_dingtalk_sign(app_secret, timestamp, sign) {
        return DingtalkResponse::error(403, "signature verification failed");
    }

    let event: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return DingtalkResponse::error(400, format!("JSON parse error: {e}")),
    };

    let msg_type = event.get("msgtype").and_then(|v| v.as_str()).unwrap_or("");
    if msg_type != "text" {
        // For non-text messages, we can't reply via sessionWebhook with a text response
        // Just return ok to acknowledge
        return DingtalkResponse::json(200, &status_payload("ignored", "non_text_message", None));
    }

    let raw_content = event
        .get("text")
        .and_then(|t| t.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    let text = extract_text_content(raw_content);
    if text.is_empty() {
        return DingtalkResponse::json(200, &status_payload("ignored", "empty_text", None));
    }

    let session_webhook = event
        .get("sessionWebhook")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let devices = build_device_list(config, data_dir);
    let client = match Client::builder().timeout(Duration::from_secs(35)).build() {
        Ok(c) => c,
        Err(e) => return DingtalkResponse::error(500, format!("HTTP client error: {e}")),
    };

    let reply = handle_cmd(&client, &devices, &text, config.report_generation_timeout_secs)
        .await
        .unwrap_or_else(|| UNKNOWN_CMD_REPLY.to_string());

    // 通过 sessionWebhook 回复，但必须先校验目标域名属于钉钉，避免 SSRF。
    if session_webhook.is_empty() {
        return DingtalkResponse::json(
            200,
            &status_payload("ok", "processed", Some("无 sessionWebhook，无法回复")),
        );
    }
    if !is_allowed_dingtalk_webhook(session_webhook) {
        return DingtalkResponse::error(400, "sessionWebhook 域名不在钉钉白名单内，已拒绝出站请求");
    }

    let reply_body = serde_json::json!({
        "msgtype": "text",
        "text": { "content": reply }
    });
    match client
        .post(session_webhook)
        .json(&reply_body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => DingtalkResponse::json(
            200,
            &status_payload("ok", "replied", Some("已通过 sessionWebhook 回复")),
        ),
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let body_preview = body.chars().take(500).collect::<String>();
            log::warn!("钉钉 sessionWebhook 回复失败 (HTTP {status}): {body_preview}");
            DingtalkResponse::error(
                500,
                format!("failed to send reply via sessionWebhook: HTTP {status}"),
            )
        }
        Err(e) => {
            DingtalkResponse::error(500, format!("failed to send reply via sessionWebhook: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 钉钉签名验证应匹配算法() {
        let app_secret = "test_secret";
        let timestamp = "1234567890";
        // Manually compute: Base64(HmacSHA256(timestamp + "\n" + app_secret))
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let message = format!("{timestamp}\n{app_secret}");
        let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes()).unwrap();
        mac.update(message.as_bytes());
        let expected_sign = BASE64.encode(mac.finalize().into_bytes());
        assert!(verify_dingtalk_sign(app_secret, timestamp, &expected_sign));
        assert!(!verify_dingtalk_sign(app_secret, timestamp, "wrong_sign"));
    }

    #[test]
    fn 应正确去除钉钉at机器人标记但保留邮箱() {
        // 结尾 @bot
        assert_eq!(extract_text_content("hello @botname"), "hello");
        // 无标记
        assert_eq!(extract_text_content("hello"), "hello");
        // 开头 @bot + 命令 + 结尾 @bot
        assert_eq!(extract_text_content("@Bot /help @Bot"), "/help");
        // 邮箱地址不应被截断
        assert_eq!(
            extract_text_content("send to admin@example.com please"),
            "send to admin@example.com please"
        );
        assert_eq!(
            extract_text_content("/report admin@example.com"),
            "/report admin@example.com"
        );
        // 整条消息仅是 @机器人
        assert_eq!(extract_text_content("@botname"), "");
        // 中文 @
        assert_eq!(extract_text_content("/help @机器人"), "/help");
    }

    #[test]
    fn 时间戳窗口校验应拒绝过期和未来太远的时间戳() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        // 当前时间 → 允许
        assert!(timestamp_within_window(&now_ms.to_string(), 3600 * 1000));
        // 30 分钟前 → 允许
        let half_hour_ago = now_ms - 30 * 60 * 1000;
        assert!(timestamp_within_window(
            &half_hour_ago.to_string(),
            3600 * 1000
        ));
        // 2 小时前 → 拒绝
        let two_hours_ago = now_ms - 2 * 3600 * 1000;
        assert!(!timestamp_within_window(
            &two_hours_ago.to_string(),
            3600 * 1000
        ));
        // 非法字符串 → 拒绝
        assert!(!timestamp_within_window("not_a_number", 3600 * 1000));
        // 空 → 拒绝
        assert!(!timestamp_within_window("", 3600 * 1000));
    }

    #[test]
    fn sessionwebhook域名白名单应只允许钉钉() {
        assert!(is_allowed_dingtalk_webhook(
            "https://oapi.dingtalk.com/robot/sendBySession?session=abc"
        ));
        assert!(is_allowed_dingtalk_webhook(
            "https://api.dingtalk.com/v1/send"
        ));
        // 非 https
        assert!(!is_allowed_dingtalk_webhook(
            "http://oapi.dingtalk.com/robot/sendBySession?session=abc"
        ));
        // 内网 IP
        assert!(!is_allowed_dingtalk_webhook("https://127.0.0.1/evil"));
        assert!(!is_allowed_dingtalk_webhook(
            "https://169.254.169.254/latest"
        ));
        // 仿冒域名
        assert!(!is_allowed_dingtalk_webhook(
            "https://dingtalk.com.evil.io/path"
        ));
        assert!(!is_allowed_dingtalk_webhook(
            "https://oapi.dingtalk.com.evil.io/path"
        ));
        assert!(!is_allowed_dingtalk_webhook("https://example.com/path"));
        // 非法 URL
        assert!(!is_allowed_dingtalk_webhook("not a url"));
        assert!(!is_allowed_dingtalk_webhook(""));
    }
}
