use crate::bot_common::{
    build_device_list, handle_cmd, normalize_command, progress_text_for_command, status_payload,
    NON_TEXT_REPLY, UNKNOWN_CMD_REPLY,
};
use crate::config::AppConfig;
use reqwest::Client;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

/// 飞书事件签名允许的时间窗口：±5 分钟（与钉钉/企微一致），防重放。
const FEISHU_SIGN_WINDOW_SECS: i64 = 300;

/// 常量时间比较 verification_token，防止时序侧信道。
fn token_matches(provided: &str, expected: &str) -> bool {
    provided.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// 校验飞书事件签名：X-Lark-Signature = sha256hex(timestamp + nonce + encrypt_key + body)，
/// 配套请求头 X-Lark-Request-Timestamp（秒级）与 X-Lark-Request-Nonce。
/// 仅在配置了 feishu_encrypt_key 时强制校验（向后兼容旧配置）。
/// 注：本实现只做签名与时间戳校验，不解密 encrypt 字段的加密事件体——
/// 若飞书平台侧同时开启了事件加密，需保持明文事件订阅方式。
fn verify_feishu_signature(
    headers: &HashMap<String, String>,
    body: &str,
    encrypt_key: &str,
) -> Result<(), &'static str> {
    use sha2::{Digest, Sha256};

    let timestamp = headers
        .get("x-lark-request-timestamp")
        .map(String::as_str)
        .unwrap_or("");
    let nonce = headers
        .get("x-lark-request-nonce")
        .map(String::as_str)
        .unwrap_or("");
    let signature = headers
        .get("x-lark-signature")
        .map(String::as_str)
        .unwrap_or("");
    if timestamp.is_empty() || nonce.is_empty() || signature.is_empty() {
        return Err("缺少飞书签名请求头");
    }

    // 时间戳新鲜度校验，拒绝超过窗口的重放请求
    let ts: i64 = timestamp.parse().map_err(|_| "飞书时间戳格式非法")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "系统时间异常")?
        .as_secs() as i64;
    if (now - ts).abs() > FEISHU_SIGN_WINDOW_SECS {
        return Err("飞书时间戳超出允许窗口");
    }

    let mut hasher = Sha256::new();
    hasher.update(timestamp.as_bytes());
    hasher.update(nonce.as_bytes());
    hasher.update(encrypt_key.as_bytes());
    hasher.update(body.as_bytes());
    let computed = hex::encode(hasher.finalize());
    if token_matches(signature, &computed) {
        Ok(())
    } else {
        Err("飞书签名不匹配")
    }
}

pub struct FeishuResponse {
    pub status: u16,
    pub body: String,
}

impl FeishuResponse {
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

// Token cache: (app_id, token, expires_at)
static TOKEN_CACHE: Mutex<Option<(String, String, Instant)>> = Mutex::new(None);

async fn get_tenant_token(client: &Client, app_id: &str, app_secret: &str) -> Option<String> {
    {
        let cache = TOKEN_CACHE.lock().ok()?;
        if let Some((cached_app_id, token, expires)) = cache.as_ref() {
            if cached_app_id == app_id && expires > &Instant::now() {
                return Some(token.clone());
            }
        }
    }
    let resp = client
        .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
        .json(&serde_json::json!({"app_id": app_id, "app_secret": app_secret}))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    let data: serde_json::Value = resp.json().await.ok()?;
    let token = data.get("tenant_access_token")?.as_str()?.to_string();
    let expire = data.get("expire").and_then(|v| v.as_u64()).unwrap_or(7200);
    let cache_ttl = expire.saturating_sub(60);
    if let Ok(mut cache) = TOKEN_CACHE.lock() {
        *cache = Some((
            app_id.to_string(),
            token.clone(),
            Instant::now() + Duration::from_secs(cache_ttl.max(60)),
        ));
    }
    Some(token)
}

async fn reply_message(client: &Client, token: &str, message_id: &str, text: &str) -> Option<()> {
    let url = format!("https://open.feishu.cn/open-apis/im/v1/messages/{message_id}/reply");
    let content = serde_json::json!({"text": text}).to_string();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({"content_type": "text", "content": content}))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let body_preview = body.chars().take(500).collect::<String>();
        log::warn!("飞书回复消息失败 (HTTP {status}): {body_preview}");
        return None;
    }
    Some(())
}

pub async fn handle_feishu_webhook(
    headers: &HashMap<String, String>,
    body: &str,
    config: &AppConfig,
    data_dir: &Path,
) -> FeishuResponse {
    // 配置了 encrypt_key 时，先对原始请求体做签名 + 时间戳校验（防伪造/重放）
    if let Some(encrypt_key) = config
        .feishu_encrypt_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        if let Err(reason) = verify_feishu_signature(headers, body, encrypt_key) {
            log::warn!("飞书事件签名校验失败: {reason}");
            return FeishuResponse::error(403, reason);
        }
    }

    let event: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return FeishuResponse::error(400, format!("JSON parse error: {e}")),
    };

    // URL verification challenge
    if event.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        let challenge = event
            .get("challenge")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let expected = config.feishu_verification_token.as_deref().unwrap_or("");
        if expected.is_empty() {
            return FeishuResponse::error(403, "verification token not configured");
        }
        let token = event.get("token").and_then(|v| v.as_str()).unwrap_or("");
        if !token_matches(token, expected) {
            return FeishuResponse::error(403, "verification token mismatch");
        }
        return FeishuResponse::json(200, &serde_json::json!({"challenge": challenge}));
    }

    // Message event
    let header = match event.get("header") {
        Some(h) => h,
        None => return FeishuResponse::error(400, "missing header"),
    };

    if header.get("event_type").and_then(|v| v.as_str()) != Some("im.message.receive_v1") {
        return FeishuResponse::json(
            200,
            &status_payload("ignored", "event_type_not_supported", None),
        );
    }

    let expected = config.feishu_verification_token.as_deref().unwrap_or("");
    if expected.is_empty() {
        return FeishuResponse::error(403, "verification token not configured");
    }
    let token = header.get("token").and_then(|v| v.as_str()).unwrap_or("");
    if !token_matches(token, expected) {
        return FeishuResponse::error(403, "token mismatch");
    }

    let event_body = match event.get("event") {
        Some(b) => b,
        None => return FeishuResponse::error(400, "missing event body"),
    };

    let message = match event_body.get("message") {
        Some(m) => m,
        None => return FeishuResponse::error(400, "missing message"),
    };

    let message_id = match message.get("message_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return FeishuResponse::error(400, "missing message_id"),
    };

    let msg_type = message
        .get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if msg_type != "text" {
        let reply = NON_TEXT_REPLY.to_string();
        let app_id = match config.feishu_app_id.as_deref() {
            Some(id) if !id.is_empty() => id,
            _ => {
                return FeishuResponse::json(
                    200,
                    &status_payload("ignored", "non_text_message", Some("feishu_app_id 未配置")),
                )
            }
        };
        let app_secret = match config.feishu_app_secret.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => {
                return FeishuResponse::json(
                    200,
                    &status_payload(
                        "ignored",
                        "non_text_message",
                        Some("feishu_app_secret 未配置"),
                    ),
                )
            }
        };
        let client = match Client::builder().timeout(Duration::from_secs(35)).build() {
            Ok(c) => c,
            Err(e) => return FeishuResponse::error(500, format!("HTTP client error: {e}")),
        };
        if let Some(tenant_token) = get_tenant_token(&client, app_id, app_secret).await {
            let _ = reply_message(&client, &tenant_token, message_id, &reply).await;
        }
        return FeishuResponse::json(
            200,
            &status_payload("ok", "non_text_replied", Some("已提示使用 /help")),
        );
    }

    let content_str = message
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("{}");
    let content: serde_json::Value = serde_json::from_str(content_str).unwrap_or_default();
    let text = content
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    if text.is_empty() {
        return FeishuResponse::json(200, &status_payload("ignored", "empty_text", None));
    }

    let app_id = match config.feishu_app_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return FeishuResponse::error(500, "feishu_app_id not configured"),
    };
    let app_secret = match config.feishu_app_secret.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => return FeishuResponse::error(500, "feishu_app_secret not configured"),
    };

    let devices = build_device_list(config, data_dir);
    let client = match Client::builder().timeout(Duration::from_secs(35)).build() {
        Ok(c) => c,
        Err(e) => return FeishuResponse::error(500, format!("HTTP client error: {e}")),
    };

    let command = normalize_command(text.split_whitespace().next().unwrap_or(""));
    if let Some(progress) = progress_text_for_command(&command) {
        if let Some(token) = get_tenant_token(&client, app_id, app_secret).await {
            let _ = reply_message(&client, &token, message_id, progress).await;
        }
    }

    let reply = handle_cmd(&client, &devices, text, config.report_generation_timeout_secs)
        .await
        .unwrap_or_else(|| UNKNOWN_CMD_REPLY.to_string());

    let tenant_token = match get_tenant_token(&client, app_id, app_secret).await {
        Some(t) => t,
        None => return FeishuResponse::error(500, "failed to get tenant_access_token"),
    };

    match reply_message(&client, &tenant_token, message_id, &reply).await {
        Some(_) => FeishuResponse::json(
            200,
            &status_payload("ok", "replied", Some("已发送回复消息")),
        ),
        None => FeishuResponse::error(500, "failed to send reply"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 飞书命令应支持斜杠和机器人后缀() {
        assert_eq!(normalize_command("/help"), "help");
        assert_eq!(normalize_command("/reports@work_review_bot"), "reports");
        assert_eq!(normalize_command("帮助"), "帮助");
    }

    #[test]
    fn 飞书签名校验应验证签名与时间戳() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        let nonce = "rand-nonce-123";
        let key = "test-encrypt-key";
        let body = "{\"foo\":\"bar\"}";

        let expected = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(now.as_bytes());
            hasher.update(nonce.as_bytes());
            hasher.update(key.as_bytes());
            hasher.update(body.as_bytes());
            hex::encode(hasher.finalize())
        };

        let mut headers = HashMap::new();
        headers.insert("x-lark-request-timestamp".to_string(), now.clone());
        headers.insert("x-lark-request-nonce".to_string(), nonce.to_string());
        headers.insert("x-lark-signature".to_string(), expected.clone());
        assert!(verify_feishu_signature(&headers, body, key).is_ok());

        // 签名不匹配
        headers.insert("x-lark-signature".to_string(), "deadbeef".to_string());
        assert!(verify_feishu_signature(&headers, body, key).is_err());

        // 时间戳过期（超出 300 秒窗口）
        headers.insert("x-lark-signature".to_string(), expected);
        headers.insert("x-lark-request-timestamp".to_string(), "1000".to_string());
        assert!(verify_feishu_signature(&headers, body, key).is_err());

        // 缺少签名头
        assert!(verify_feishu_signature(&HashMap::new(), body, key).is_err());
    }
}
