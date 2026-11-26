#[cfg(test)]
use crate::bot_common::normalize_command;
use crate::bot_common::{
    build_device_list, handle_cmd, route_inbound_bot_message, shared_bot_http_client,
    status_payload, timestamp_within_window_secs, BotPipelineAction, InboundBotMessage,
    BOT_SIGN_WINDOW_SECS, UNKNOWN_CMD_REPLY,
};
use reqwest::Client;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use work_review_core::config::AppConfig;

/// 常量时间比较 verification_token，防止时序侧信道。
fn token_matches(provided: &str, expected: &str) -> bool {
    provided.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// 校验飞书事件签名：X-Lark-Signature = sha256hex(timestamp + nonce + encrypt_key + body)，
/// 配套请求头 X-Lark-Request-Timestamp（秒级）与 X-Lark-Request-Nonce。
/// 仅在配置了 feishu_encrypt_key 时强制校验（向后兼容旧配置）。
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

    // 统一时间窗口校验（±5 分钟，防重放与 i64 减法溢出）
    if !timestamp_within_window_secs(timestamp, BOT_SIGN_WINDOW_SECS) {
        return Err("飞书时间戳非法或超出允许窗口");
    }

    let mut hasher = Sha256::new();
    hasher.update(timestamp.as_bytes());
    hasher.update(nonce.as_bytes());
    hasher.update(encrypt_key.as_bytes());
    hasher.update(body.as_bytes());
    let computed = hex::encode(hasher.finalize());
    if token_matches(&signature.to_ascii_lowercase(), &computed) {
        Ok(())
    } else {
        Err("飞书签名不匹配")
    }
}

/// 解密飞书事件订阅的加密事件体（`{"encrypt": "<base64>"}`）：
/// 密钥 = `SHA-256(encrypt_key)`，解码后前 16 字节为 IV，余下为 AES-256-CBC + PKCS#7 密文。
fn decrypt_feishu_event(encrypt_key: &str, encrypted_b64: &str) -> Result<String, &'static str> {
    use aes::Aes256;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use sha2::{Digest, Sha256};
    type Aes256CbcDec = cbc::Decryptor<Aes256>;

    let raw = BASE64
        .decode(encrypted_b64.trim())
        .map_err(|_| "飞书加密事件体 Base64 非法")?;
    if raw.len() <= 16 || !(raw.len() - 16).is_multiple_of(16) {
        return Err("飞书加密事件体长度非法");
    }
    let key: [u8; 32] = Sha256::digest(encrypt_key.as_bytes()).into();
    let iv: [u8; 16] = raw[..16].try_into().map_err(|_| "飞书加密事件体 IV 非法")?;
    let mut ciphertext = raw[16..].to_vec();
    let plaintext = Aes256CbcDec::new(&key.into(), &iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut ciphertext)
        .map_err(|_| "飞书加密事件体解密失败")?;
    String::from_utf8(plaintext.to_vec()).map_err(|_| "飞书解密结果不是合法 UTF-8")
}

/// 校验飞书 message_id 仅含安全字符，防止拼入回复 URL 时发生路径穿越。
fn is_valid_feishu_message_id(message_id: &str) -> bool {
    !message_id.is_empty()
        && message_id.len() <= 128
        && message_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// 剥离飞书群聊文本开头的 `@_user_1` 等提及占位符。
fn strip_feishu_leading_mentions(text: &str) -> &str {
    let mut rest = text.trim();
    while let Some(after_at) = rest.strip_prefix("@_user_") {
        let digits_end = after_at
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_at.len());
        if digits_end == 0 {
            break;
        }
        rest = after_at[digits_end..].trim_start();
    }
    rest
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
static FEISHU_NL_SEMAPHORE: once_cell::sync::Lazy<std::sync::Arc<tokio::sync::Semaphore>> =
    once_cell::sync::Lazy::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));

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
    if !is_valid_feishu_message_id(message_id) {
        log::warn!("拒绝非法的飞书 message_id");
        return None;
    }
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
    state: &std::sync::Arc<std::sync::Mutex<crate::AppState>>,
) -> FeishuResponse {
    // 配置了 encrypt_key 时，先对原始请求体做签名 + 时间戳校验（防伪造/重放）
    let configured_encrypt_key = config
        .feishu_encrypt_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty());
    if let Some(encrypt_key) = configured_encrypt_key {
        if let Err(reason) = verify_feishu_signature(headers, body, encrypt_key) {
            log::warn!("飞书事件签名校验失败: {reason}");
            return FeishuResponse::error(403, reason);
        }
    }

    let raw_event: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return FeishuResponse::error(400, format!("JSON parse error: {e}")),
    };

    // 飞书开放平台开启 Encrypt Key 后会将回调加密为 `{"encrypt": "<base64>"}`，在此解包
    let event: serde_json::Value = if let Some(encrypted) =
        raw_event.get("encrypt").and_then(|v| v.as_str())
    {
        let Some(encrypt_key) = configured_encrypt_key else {
            return FeishuResponse::error(403, "收到飞书加密事件，但本机未配置 feishu_encrypt_key");
        };
        let decrypted = match decrypt_feishu_event(encrypt_key, encrypted) {
            Ok(plain) => plain,
            Err(reason) => {
                log::warn!("飞书加密事件解密失败: {reason}");
                return FeishuResponse::error(403, reason);
            }
        };
        match serde_json::from_str(&decrypted) {
            Ok(v) => v,
            Err(e) => {
                return FeishuResponse::error(400, format!("decrypted JSON parse error: {e}"))
            }
        }
    } else {
        raw_event
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
        Some(id) if is_valid_feishu_message_id(id) => id,
        _ => return FeishuResponse::error(400, "missing or invalid message_id"),
    };

    let msg_type = message
        .get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let text = if msg_type == "text" {
        let content_str = message
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("{}");
        let content: serde_json::Value = serde_json::from_str(content_str).unwrap_or_default();
        strip_feishu_leading_mentions(content.get("text").and_then(|v| v.as_str()).unwrap_or(""))
            .to_string()
    } else {
        String::new()
    };

    let app_id = match config.feishu_app_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return FeishuResponse::error(500, "feishu_app_id not configured"),
    };
    let app_secret = match config.feishu_app_secret.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => return FeishuResponse::error(500, "feishu_app_secret not configured"),
    };

    let client = shared_bot_http_client();
    let sender_id = crate::bot_common::feishu_sender_id(event_body);
    let inbound = InboundBotMessage::standard(
        crate::bot_common::BOT_PLATFORM_FEISHU,
        message_id,
        sender_id,
        &text,
        msg_type == "text",
    );

    match route_inbound_bot_message(
        state,
        &inbound,
        &FEISHU_NL_SEMAPHORE,
        config.bot_natural_language_enabled,
        None,
    ) {
        BotPipelineAction::Duplicate => {
            FeishuResponse::json(200, &status_payload("ignored", "duplicate_message", None))
        }
        BotPipelineAction::ImmediateReply {
            reason,
            text: reply,
        } => {
            if reply.is_empty() {
                return FeishuResponse::json(200, &status_payload("ignored", reason, None));
            }
            if matches!(reason, "nl_busy" | "nl_rate_limited") {
                let client_bg = client.clone();
                let app_id_bg = app_id.to_string();
                let app_secret_bg = app_secret.to_string();
                let message_id_bg = message_id.to_string();
                tokio::spawn(async move {
                    if let Some(tok) =
                        get_tenant_token(&client_bg, &app_id_bg, &app_secret_bg).await
                    {
                        let _ = reply_message(&client_bg, &tok, &message_id_bg, &reply).await;
                    }
                });
                return FeishuResponse::json(
                    200,
                    &status_payload("ok", reason, Some("已受理提示")),
                );
            }
            if let Some(token) = get_tenant_token(&client, app_id, app_secret).await {
                let _ = reply_message(&client, &token, message_id, &reply).await;
            }
            FeishuResponse::json(200, &status_payload("ok", reason, Some("已回复提示")))
        }
        BotPipelineAction::NaturalLanguage { permit, question } => {
            // 飞书 Webhook 要求 3 秒内返回 HTTP 200；立即回执并在后台推送分析结果
            let state_bg = state.clone();
            let client_bg = client.clone();
            let app_id_bg = app_id.to_string();
            let app_secret_bg = app_secret.to_string();
            let message_id_bg = message_id.to_string();
            tokio::spawn(async move {
                let _permit = permit;
                if let Some(tok) = get_tenant_token(&client_bg, &app_id_bg, &app_secret_bg).await {
                    let _ = reply_message(
                        &client_bg,
                        &tok,
                        &message_id_bg,
                        crate::bot_common::BOT_NL_THINKING_REPLY,
                    )
                    .await;
                }
                let answer = crate::bot_common::run_bot_natural_language_query(
                    &state_bg,
                    &question,
                    crate::bot_common::BOT_PLATFORM_FEISHU,
                )
                .await;
                if let Some(tok) = get_tenant_token(&client_bg, &app_id_bg, &app_secret_bg).await {
                    let _ = reply_message(&client_bg, &tok, &message_id_bg, &answer).await;
                }
            });
            FeishuResponse::json(
                200,
                &status_payload("ok", "nl_accepted", Some("已受理自然语言问答")),
            )
        }
        BotPipelineAction::Command {
            text: cmd_text,
            progress_hint,
        } => {
            let devices = build_device_list(config, data_dir);
            if let Some(progress) = progress_hint {
                if let Some(token) = get_tenant_token(&client, app_id, app_secret).await {
                    let _ = reply_message(&client, &token, message_id, progress).await;
                }
            }
            let reply = handle_cmd(
                &client,
                &devices,
                &cmd_text,
                crate::bot_common::report_generate_timeout(config),
            )
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

        // 极端负时间戳不得溢出
        headers.insert("x-lark-request-timestamp".to_string(), i64::MIN.to_string());
        assert!(verify_feishu_signature(&headers, body, key).is_err());
    }

    #[test]
    fn 飞书加密事件体应能正确解密并校验message_id与群提及() {
        use aes::Aes256;
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
        use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        use sha2::{Digest, Sha256};
        type Aes256CbcEnc = cbc::Encryptor<Aes256>;

        let encrypt_key = "feishu-secret-encrypt-key";
        let plain = r#"{"type":"url_verification","challenge":"ch_123","token":"tok_abc"}"#;
        let key: [u8; 32] = Sha256::digest(encrypt_key.as_bytes()).into();
        let iv = [42u8; 16];
        let mut buf = plain.as_bytes().to_vec();
        let msg_len = buf.len();
        buf.resize(msg_len + 16, 0);
        let cipher = Aes256CbcEnc::new(&key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, msg_len)
            .unwrap();
        let mut combined = iv.to_vec();
        combined.extend_from_slice(cipher);
        let b64 = BASE64.encode(combined);

        let decrypted = decrypt_feishu_event(encrypt_key, &b64).unwrap();
        assert_eq!(decrypted, plain);
        assert!(decrypt_feishu_event("wrong-key", &b64).is_err());

        assert!(is_valid_feishu_message_id("om_123abc-XYZ"));
        assert!(!is_valid_feishu_message_id(""));
        assert!(!is_valid_feishu_message_id("../other/endpoint"));

        assert_eq!(
            strip_feishu_leading_mentions("@_user_1 /bind WR-ABCD1234"),
            "/bind WR-ABCD1234"
        );
    }
}
