use crate::bot_common::{
    build_device_list, handle_cmd, route_inbound_bot_message, shared_bot_http_client,
    timestamp_within_window_secs as timestamp_within_window, BotPipelineAction, InboundBotMessage,
    BOT_SIGN_WINDOW_SECS as WECOM_SIGN_WINDOW_SECS, UNKNOWN_CMD_REPLY,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::path::Path;
use std::time::Duration;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use work_review_core::config::AppConfig;

const WECOM_CALLBACK_NL_TIMEOUT_MS: u64 = 3500;
const WECOM_CALLBACK_NL_TIMEOUT_REPLY: &str = "⏳ 问题分析耗时较长，超过企业微信 HTTP 回调 5 秒时限。建议在「设置 → 接入管理」切换为企业微信智能机器人长连接模式，或改用 /report 等快捷命令。";
static WECOM_CALLBACK_NL_SEMAPHORE: once_cell::sync::Lazy<std::sync::Arc<tokio::sync::Semaphore>> =
    once_cell::sync::Lazy::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));

pub struct WecomResponse {
    pub status: u16,
    pub body: String,
    pub content_type: String,
}

impl WecomResponse {
    pub fn xml(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            content_type: "application/xml; charset=utf-8".to_string(),
        }
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            body: value.to_string(),
            content_type: "application/json; charset=utf-8".to_string(),
        }
    }

    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, &serde_json::json!({"error": message.into()}))
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            content_type: "text/plain; charset=utf-8".to_string(),
        }
    }
}

/// Decode WeCom EncodingAESKey (43 chars) to 32-byte AES key
fn decode_aes_key(encoding_aes_key: &str) -> Option<[u8; 32]> {
    let padded = format!("{encoding_aes_key}=");
    let decoded = BASE64.decode(&padded).ok()?;
    if decoded.len() != 32 {
        return None;
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&decoded);
    Some(key)
}

/// Verify WeCom callback signature: SHA1(sort([token, timestamp, nonce, encrypt]))
fn verify_signature(
    token: &str,
    timestamp: &str,
    nonce: &str,
    encrypt: &str,
    signature: &str,
) -> bool {
    let mut parts = [
        token.to_string(),
        timestamp.to_string(),
        nonce.to_string(),
        encrypt.to_string(),
    ];
    parts.sort();
    let joined = parts.join("");
    use sha1::{Digest, Sha1};
    let result = Sha1::digest(joined.as_bytes());
    let computed = hex::encode(result);
    // 常量时间比较，防签名前缀的时序侧信道。
    computed.as_bytes().ct_eq(signature.as_bytes()).into()
}

/// AES-256-CBC decrypt WeCom encrypted content.
/// Returns (message_xml, receiving_id) after decryption.
///
/// 注意：WeCom 协议规定 IV 取 AESKey 前 16 字节（确定性 IV）。这是协议要求，
/// 不要"修复"为随机 IV——会与服务端不兼容。明文头部的 16 字节随机前缀已经
/// 保证了同样明文每次加密的密文不同，IV 确定不会造成实际可用的密码学弱点。
fn aes_decrypt_raw(key: &[u8; 32], ciphertext_b64: &str) -> Option<(String, String)> {
    let ciphertext = BASE64.decode(ciphertext_b64).ok()?;
    let iv = &key[..16];

    use aes::Aes256;
    use cbc::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
    use cbc::Decryptor;
    type Aes256CbcDec = Decryptor<Aes256>;

    let decryptor = Aes256CbcDec::new(key.into(), iv.into());
    let mut buf = ciphertext;
    let decrypted = decryptor.decrypt_padded_mut::<NoPadding>(&mut buf).ok()?;
    // 企业微信按 32 字节块做 PKCS#7 填充（填充值 1..=32），标准 16 字节 PKCS#7 解包会拒绝
    // 填充值大于 16 的消息，约一半回调因此解密失败。这里按企业微信规范手动去填充。
    let plaintext = strip_wecom_pkcs7(decrypted)?;

    // Format: 16 random bytes + 4 bytes msg_len (big-endian) + msg + receiving_id
    if plaintext.len() < 20 {
        return None;
    }
    let msg_len =
        u32::from_be_bytes([plaintext[16], plaintext[17], plaintext[18], plaintext[19]]) as usize;
    let msg_end = 20usize.checked_add(msg_len)?;
    if plaintext.len() < msg_end {
        return None;
    }
    let msg = std::str::from_utf8(&plaintext[20..msg_end])
        .ok()?
        .to_string();
    let receiving_id = std::str::from_utf8(&plaintext[msg_end..]).ok()?.to_string();
    Some((msg, receiving_id))
}

/// 解密后的回调 XML 中的发送者（FromUserName）与消息 ID（MsgId）。
fn wecom_callback_sender_and_message_id(decrypted_xml: &str) -> (String, String) {
    (
        extract_cdata(decrypted_xml, "FromUserName").unwrap_or_default(),
        extract_cdata(decrypted_xml, "MsgId").unwrap_or_default(),
    )
}

/// 去除企业微信的 PKCS#7 填充（块大小 32，填充值 1..=32，且所有填充字节相同）。
fn strip_wecom_pkcs7(data: &[u8]) -> Option<&[u8]> {
    const WECOM_PKCS7_BLOCK_SIZE: usize = 32;
    let pad = *data.last()? as usize;
    if pad == 0 || pad > WECOM_PKCS7_BLOCK_SIZE || pad > data.len() {
        return None;
    }
    let (content, padding) = data.split_at(data.len() - pad);
    if padding.iter().any(|byte| *byte as usize != pad) {
        return None;
    }
    Some(content)
}

/// AES-256-CBC encrypt reply for WeCom（按企业微信 32 字节块规范做 PKCS#7 填充）
fn aes_encrypt(key: &[u8; 32], plaintext: &str, corp_id: &str) -> Option<String> {
    use aes::Aes256;
    use cbc::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
    use cbc::Encryptor;
    type Aes256CbcEnc = Encryptor<Aes256>;
    const WECOM_PKCS7_BLOCK_SIZE: usize = 32;

    let iv = &key[..16];

    let msg_bytes = plaintext.as_bytes();
    let corp_bytes = corp_id.as_bytes();
    let msg_len = u32::try_from(msg_bytes.len()).ok()?;

    let mut content =
        Vec::with_capacity(16 + 4 + msg_bytes.len() + corp_bytes.len() + WECOM_PKCS7_BLOCK_SIZE);
    let random_bytes = uuid::Uuid::new_v4();
    content.extend_from_slice(random_bytes.as_bytes());
    content.extend_from_slice(&msg_len.to_be_bytes());
    content.extend_from_slice(msg_bytes);
    content.extend_from_slice(corp_bytes);

    let pad_len = WECOM_PKCS7_BLOCK_SIZE - (content.len() % WECOM_PKCS7_BLOCK_SIZE);
    content.extend(std::iter::repeat_n(pad_len as u8, pad_len));
    let total_len = content.len();

    let encryptor = Aes256CbcEnc::new(key.into(), iv.into());
    let ciphertext = encryptor
        .encrypt_padded_mut::<NoPadding>(&mut content, total_len)
        .ok()?;

    Some(BASE64.encode(ciphertext))
}

/// 仅从顶层 `<xml>` 的直接子节点提取指定标签文本（正确跳过 `<![CDATA[...]]>` 内容），
/// 防止用户消息 `<Content><![CDATA[...<MsgId>...</MsgId>...]]></Content>` 劫持同级字段。
fn extract_cdata(xml: &str, tag: &str) -> Option<String> {
    let bytes = xml.as_bytes();
    let mut pos = 0;
    let mut tag_stack: Vec<&str> = Vec::new();

    while pos < bytes.len() {
        let Some(rel_lt) = xml[pos..].find('<') else {
            break;
        };
        pos += rel_lt;

        if xml[pos..].starts_with("<![CDATA[") {
            let after_open = pos + 9;
            let end = xml[after_open..].find("]]>")?;
            pos = after_open + end + 3;
            continue;
        }
        if xml[pos..].starts_with("<!--") {
            let after_open = pos + 4;
            let end = xml[after_open..].find("-->")?;
            pos = after_open + end + 3;
            continue;
        }
        if xml[pos..].starts_with("<?") {
            let after_open = pos + 2;
            let end = xml[after_open..].find("?>")?;
            pos = after_open + end + 2;
            continue;
        }

        let rel_gt = xml[pos..].find('>')?;
        let raw_tag = &xml[pos + 1..pos + rel_gt];
        let tag_end = pos + rel_gt + 1;

        if let Some(close_name) = raw_tag.strip_prefix('/') {
            let close_name = close_name.trim();
            if tag_stack.last().copied() == Some(close_name) {
                tag_stack.pop();
            }
            pos = tag_end;
            continue;
        }
        let self_closing = raw_tag.ends_with('/');
        let name = raw_tag
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");

        // tag_stack.len() == 0 是根元素 `<xml>`；== 1 是根元素的直接子元素（FromUserName/MsgId/Content/...）
        if tag_stack.len() == 1 && name == tag && !self_closing {
            let close_marker = format!("</{tag}>");
            let mut inner = String::new();
            let mut cur = tag_end;
            let mut found_close = false;
            while cur < xml.len() {
                if xml[cur..].starts_with("<![CDATA[") {
                    let cdata_start = cur + 9;
                    let cdata_end = xml[cdata_start..].find("]]>")?;
                    inner.push_str(&xml[cdata_start..cdata_start + cdata_end]);
                    cur = cdata_start + cdata_end + 3;
                } else if xml[cur..].starts_with(&close_marker) {
                    found_close = true;
                    break;
                } else {
                    let next_special = xml[cur..].find('<').map(|i| cur + i).unwrap_or(xml.len());
                    if next_special == cur {
                        // 遇到非 CDATA 的嵌套/未知 `<` 字符，按字面推进 1 字节避免死循环
                        inner.push('<');
                        cur += 1;
                    } else {
                        inner.push_str(&xml[cur..next_special]);
                        cur = next_special;
                    }
                }
            }
            if found_close {
                return Some(inner);
            }
            return None;
        }

        if !self_closing {
            tag_stack.push(name);
        }
        pos = tag_end;
    }

    None
}

/// Build encrypted reply XML
fn build_reply_xml(encrypt: &str, signature: &str, timestamp: &str, nonce: &str) -> String {
    format!(
        "<xml>\
<Encrypt><![CDATA[{encrypt}]]></Encrypt>\
<MsgSignature><![CDATA[{signature}]]></MsgSignature>\
<TimeStamp>{timestamp}</TimeStamp>\
<Nonce><![CDATA[{nonce}]]></Nonce>\
</xml>"
    )
}

/// Handle GET request: URL verification (verify and return echostr plaintext)
pub fn handle_wecom_verify(
    query: &std::collections::HashMap<String, String>,
    config: &AppConfig,
) -> WecomResponse {
    let token = match config.wecom_token.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return WecomResponse::error(403, "wecom_token not configured"),
    };
    let encoding_aes_key = match config.wecom_encoding_aes_key.as_deref() {
        Some(k) if !k.is_empty() => k,
        _ => return WecomResponse::error(403, "wecom_encoding_aes_key not configured"),
    };
    let corp_id = match config.wecom_corp_id.as_deref() {
        Some(c) if !c.is_empty() => c,
        _ => return WecomResponse::error(403, "wecom_corp_id not configured"),
    };

    let msg_signature = query.get("msg_signature").map(|s| s.as_str()).unwrap_or("");
    let timestamp = query.get("timestamp").map(|s| s.as_str()).unwrap_or("");
    let nonce = query.get("nonce").map(|s| s.as_str()).unwrap_or("");
    let echostr = query.get("echostr").map(|s| s.as_str()).unwrap_or("");

    if timestamp.is_empty() || nonce.is_empty() || msg_signature.is_empty() || echostr.is_empty() {
        return WecomResponse::error(400, "missing required query parameter");
    }
    if !timestamp_within_window(timestamp, WECOM_SIGN_WINDOW_SECS) {
        return WecomResponse::error(401, "timestamp out of allowed window");
    }
    if !verify_signature(token, timestamp, nonce, echostr, msg_signature) {
        return WecomResponse::error(403, "signature verification failed");
    }

    let key = match decode_aes_key(encoding_aes_key) {
        Some(k) => k,
        None => return WecomResponse::error(500, "invalid EncodingAESKey"),
    };

    match aes_decrypt_raw(&key, echostr) {
        Some((plain, receiving_id)) if receiving_id == corp_id => WecomResponse::text(200, plain),
        Some(_) => WecomResponse::error(403, "corp_id mismatch"),
        None => WecomResponse::error(500, "failed to decrypt echostr"),
    }
}

/// Handle POST request: message callback
pub async fn handle_wecom_callback(
    query: &std::collections::HashMap<String, String>,
    body: &str,
    config: &AppConfig,
    data_dir: &Path,
    state: &std::sync::Arc<std::sync::Mutex<crate::AppState>>,
) -> WecomResponse {
    let token = match config.wecom_token.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return WecomResponse::error(403, "wecom_token not configured"),
    };
    let encoding_aes_key = match config.wecom_encoding_aes_key.as_deref() {
        Some(k) if !k.is_empty() => k,
        _ => return WecomResponse::error(403, "wecom_encoding_aes_key not configured"),
    };
    let corp_id = match config.wecom_corp_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return WecomResponse::error(403, "wecom_corp_id not configured"),
    };

    let msg_signature = query.get("msg_signature").map(|s| s.as_str()).unwrap_or("");
    let timestamp = query.get("timestamp").map(|s| s.as_str()).unwrap_or("");
    let nonce = query.get("nonce").map(|s| s.as_str()).unwrap_or("");

    if timestamp.is_empty() || nonce.is_empty() || msg_signature.is_empty() {
        return WecomResponse::error(400, "missing required query parameter");
    }
    if !timestamp_within_window(timestamp, WECOM_SIGN_WINDOW_SECS) {
        return WecomResponse::error(401, "timestamp out of allowed window");
    }

    let encrypt = match extract_cdata(body, "Encrypt") {
        Some(e) => e,
        None => return WecomResponse::error(400, "missing Encrypt field in XML"),
    };

    if !verify_signature(token, timestamp, nonce, &encrypt, msg_signature) {
        return WecomResponse::error(403, "signature verification failed");
    }

    let key = match decode_aes_key(encoding_aes_key) {
        Some(k) => k,
        None => return WecomResponse::error(500, "invalid EncodingAESKey"),
    };

    let (decrypted, receiving_id) = match aes_decrypt_raw(&key, &encrypt) {
        Some(d) => d,
        None => return WecomResponse::error(500, "failed to decrypt message"),
    };

    if receiving_id != corp_id {
        return WecomResponse::error(403, "corp_id mismatch");
    }

    let msg_type = extract_cdata(&decrypted, "MsgType").unwrap_or_default();
    let text = if msg_type == "text" {
        extract_cdata(&decrypted, "Content")
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        String::new()
    };

    // 统一入站管线：去重 + 发送者授权 + 非文本/空文本 + NL 并发/限流 + 命令分发
    let (sender_id, message_id) = wecom_callback_sender_and_message_id(&decrypted);
    let inbound = InboundBotMessage::standard(
        crate::bot_common::BOT_PLATFORM_WECOM,
        &message_id,
        &sender_id,
        &text,
        msg_type == "text",
    );

    let reply = match route_inbound_bot_message(
        state,
        &inbound,
        &WECOM_CALLBACK_NL_SEMAPHORE,
        config.bot_natural_language_enabled,
        Some(crate::bot_common::EMPTY_TEXT_REPLY),
    ) {
        BotPipelineAction::Duplicate => return WecomResponse::xml(200, String::new()),
        BotPipelineAction::ImmediateReply { text: reply, .. } => reply,
        BotPipelineAction::NaturalLanguage { permit, question } => {
            let _permit = permit;
            match tokio::time::timeout(
                Duration::from_millis(WECOM_CALLBACK_NL_TIMEOUT_MS),
                crate::bot_common::run_bot_natural_language_query(
                    state,
                    &question,
                    crate::bot_common::BOT_PLATFORM_WECOM,
                ),
            )
            .await
            {
                Ok(answer) => answer,
                Err(_) => WECOM_CALLBACK_NL_TIMEOUT_REPLY.to_string(),
            }
        }
        BotPipelineAction::Command { text: cmd_text, .. } => {
            let devices = build_device_list(config, data_dir);
            let client = shared_bot_http_client();
            // WeCom 被动回复必须在 5 秒内返回，所以不发送"处理中"提示，直接执行命令并回包。
            handle_cmd(
                &client,
                &devices,
                &cmd_text,
                crate::bot_common::report_generate_timeout(config),
            )
            .await
            .unwrap_or_else(|| UNKNOWN_CMD_REPLY.to_string())
        }
    };

    let reply_xml = encrypt_reply(&key, token, corp_id, &reply, timestamp, nonce);
    WecomResponse::xml(200, reply_xml)
}

fn encrypt_reply(
    key: &[u8; 32],
    token: &str,
    corp_id: &str,
    reply_text: &str,
    timestamp: &str,
    nonce: &str,
) -> String {
    let encrypted = match aes_encrypt(key, reply_text, corp_id) {
        Some(e) => e,
        None => return "<xml><Encrypt><![CDATA[]]></Encrypt></xml>".to_string(),
    };

    let ts = if timestamp.is_empty() {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string()
    } else {
        timestamp.to_string()
    };

    let new_nonce = if nonce.is_empty() {
        uuid::Uuid::new_v4().to_string().replace('-', "")
    } else {
        nonce.to_string()
    };

    let signature = {
        let mut parts = [
            token.to_string(),
            ts.clone(),
            new_nonce.clone(),
            encrypted.clone(),
        ];
        parts.sort();
        let joined = parts.join("");
        use sha1::{Digest, Sha1};
        hex::encode(Sha1::digest(joined.as_bytes()))
    };

    build_reply_xml(&encrypted, &signature, &ts, &new_nonce)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 签名验证应匹配企业微信算法() {
        let token = "test_token";
        let timestamp = "1234567890";
        let nonce = "nonce123";
        let encrypt = "encrypted_content";
        let mut parts = [token, timestamp, nonce, encrypt];
        parts.sort();
        let joined = parts.join("");
        use sha1::{Digest, Sha1};
        let expected = hex::encode(Sha1::digest(joined.as_bytes()));
        assert!(verify_signature(
            token, timestamp, nonce, encrypt, &expected
        ));
        assert!(!verify_signature(
            token,
            timestamp,
            nonce,
            encrypt,
            "wrong_signature"
        ));
    }

    #[test]
    fn cdata提取应支持标准格式() {
        let xml = "<xml><ToUserName><![CDATA[corp123]]></ToUserName><Encrypt><![CDATA[abc123]]></Encrypt></xml>";
        assert_eq!(extract_cdata(xml, "Encrypt"), Some("abc123".to_string()));
        assert_eq!(
            extract_cdata(xml, "ToUserName"),
            Some("corp123".to_string())
        );
        assert_eq!(extract_cdata(xml, "Missing"), None);
    }

    #[test]
    fn cdata提取应支持非cdata格式() {
        let xml = "<xml><AgentID>1000001</AgentID></xml>";
        assert_eq!(extract_cdata(xml, "AgentID"), Some("1000001".to_string()));
    }

    #[test]
    fn aes加解密应可逆() {
        let key_b64 = "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY";
        let key = decode_aes_key(key_b64).unwrap();
        let plaintext = "Hello 企业微信";
        let corp_id = "test_corp";
        let encrypted = aes_encrypt(&key, plaintext, corp_id).unwrap();
        let (decrypted_msg, decrypted_corp) = aes_decrypt_raw(&key, &encrypted).unwrap();
        assert_eq!(decrypted_msg, plaintext);
        assert_eq!(decrypted_corp, corp_id);
    }

    #[test]
    fn 应正确构建回复xml() {
        let xml = build_reply_xml("enc123", "sig456", "789", "nonce");
        assert!(xml.contains("<Encrypt><![CDATA[enc123]]></Encrypt>"));
        assert!(xml.contains("<MsgSignature><![CDATA[sig456]]></MsgSignature>"));
        assert!(xml.contains("<TimeStamp>789</TimeStamp>"));
        assert!(xml.contains("<Nonce><![CDATA[nonce]]></Nonce>"));
    }

    #[test]
    fn 企微时间戳窗口校验应防重放() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        // 当前 → 允许
        assert!(timestamp_within_window(&now.to_string(), 300));
        // 2 分钟前 → 允许
        assert!(timestamp_within_window(&(now - 120).to_string(), 300));
        // 1 小时前 → 拒绝
        assert!(!timestamp_within_window(&(now - 3600).to_string(), 300));
        // 非法 → 拒绝
        assert!(!timestamp_within_window("garbage", 300));
        assert!(!timestamp_within_window("", 300));
    }
}

#[cfg(test)]
mod wecom_padding_tests {
    use super::*;

    #[test]
    fn 应从回调xml提取发送者与消息id() {
        let xml = "<xml><ToUserName><![CDATA[ww_corp]]></ToUserName><FromUserName><![CDATA[zhangsan]]></FromUserName><MsgType><![CDATA[text]]></MsgType><Content><![CDATA[/help]]></Content><MsgId>1234567890</MsgId></xml>";
        assert_eq!(
            wecom_callback_sender_and_message_id(xml),
            ("zhangsan".to_string(), "1234567890".to_string())
        );
        assert_eq!(
            wecom_callback_sender_and_message_id("<xml></xml>"),
            (String::new(), String::new())
        );
    }

    /// 按企业微信官方 SDK 的方式加密：PKCS#7 块大小 32（填充值 1..=32）。
    fn wecom_style_encrypt(
        key: &[u8; 32],
        random16: &[u8; 16],
        msg: &str,
        corp_id: &str,
    ) -> String {
        use aes::Aes256;
        use cbc::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
        type Aes256CbcEnc = cbc::Encryptor<Aes256>;

        let mut content = Vec::new();
        content.extend_from_slice(random16);
        content.extend_from_slice(&(msg.len() as u32).to_be_bytes());
        content.extend_from_slice(msg.as_bytes());
        content.extend_from_slice(corp_id.as_bytes());
        let pad = 32 - (content.len() % 32);
        content.extend(std::iter::repeat_n(pad as u8, pad));
        let len = content.len();
        let ciphertext = Aes256CbcEnc::new(key.into(), key[..16].into())
            .encrypt_padded_mut::<NoPadding>(&mut content, len)
            .unwrap()
            .to_vec();
        BASE64.encode(ciphertext)
    }

    #[test]
    fn 应能解密企业微信32字节块填充的所有长度() {
        let key = [7u8; 32];
        let random16 = [1u8; 16];
        let corp_id = "ww_corp";
        // 覆盖填充值 1..=32 的全部情况（其中 17..=32 在旧实现下会解密失败）
        for extra in 0..32 {
            let msg = format!("<xml>{}</xml>", "a".repeat(extra));
            let encrypted = wecom_style_encrypt(&key, &random16, &msg, corp_id);
            let (decrypted, receiving_id) =
                aes_decrypt_raw(&key, &encrypted).unwrap_or_else(|| panic!("extra={extra}"));
            assert_eq!(decrypted, msg);
            assert_eq!(receiving_id, corp_id);
        }
    }

    #[test]
    fn 非法填充应被拒绝() {
        assert!(strip_wecom_pkcs7(&[1, 2, 3, 0]).is_none());
        assert!(strip_wecom_pkcs7(&[1, 2, 3, 33]).is_none());
        assert!(strip_wecom_pkcs7(&[1, 2, 2, 3]).is_none());
        assert_eq!(strip_wecom_pkcs7(&[9, 9, 2, 2]), Some(&[9u8, 9][..]));
        assert!(strip_wecom_pkcs7(&[]).is_none());
    }

    #[test]
    fn 加密回复应按32字节块填充并可往返解密() {
        let key = [11u8; 32];
        let corp_id = "ww_corp_test";
        for extra in 0..32 {
            let reply = format!("回复内容-{}", "x".repeat(extra));
            let encrypted = aes_encrypt(&key, &reply, corp_id).unwrap();
            let raw = BASE64.decode(&encrypted).unwrap();
            assert_eq!(raw.len() % 32, 0, "密文长度应为 32 字节倍数 extra={extra}");
            let (decrypted, receiving_id) = aes_decrypt_raw(&key, &encrypted).unwrap();
            assert_eq!(decrypted, reply);
            assert_eq!(receiving_id, corp_id);
        }
    }

    #[test]
    fn 正文夹带xml标签不得劫持msgid或fromusername() {
        let xml = "<xml><ToUserName><![CDATA[ww_corp]]></ToUserName><FromUserName><![CDATA[real_user]]></FromUserName><MsgType><![CDATA[text]]></MsgType><Content><![CDATA[/bind WR-1234 <MsgId>evil</MsgId><FromUserName>admin</FromUserName>]]></Content><MsgId>987654321</MsgId></xml>";
        assert_eq!(
            wecom_callback_sender_and_message_id(xml),
            ("real_user".to_string(), "987654321".to_string())
        );
        // 含拆段 CDATA 与伪造标签的正文同样不得干扰顶层字段
        let split_cdata_xml = "<xml><Content><![CDATA[<MsgId>fake</MsgId>]]]]><![CDATA[><FromUserName>admin</FromUserName>]]></Content><FromUserName><![CDATA[real_user]]></FromUserName><MsgId>112233</MsgId></xml>";
        assert_eq!(
            wecom_callback_sender_and_message_id(split_cdata_xml),
            ("real_user".to_string(), "112233".to_string())
        );
        // i64::MIN 时间戳不得溢出 panic 或通过校验
        assert!(!timestamp_within_window(&i64::MIN.to_string(), 300));
    }
}
