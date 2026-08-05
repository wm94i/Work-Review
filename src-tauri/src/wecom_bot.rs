use crate::bot_common::{build_device_list, handle_cmd, NON_TEXT_REPLY, UNKNOWN_CMD_REPLY};
use crate::config::AppConfig;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::Client;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

/// 企业微信回调允许的 timestamp 窗口：±5 分钟（WeCom timestamp 单位为秒）。
const WECOM_SIGN_WINDOW_SECS: i64 = 5 * 60;

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

/// 校验 WeCom timestamp 是否在允许窗口内（防重放）。
/// WeCom timestamp 是秒级 Unix 时间戳的字符串。
fn timestamp_within_window(timestamp_secs_str: &str, window_secs: i64) -> bool {
    let ts: i64 = match timestamp_secs_str.parse() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return false,
    };
    (now - ts).abs() <= window_secs
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
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use cbc::Decryptor;
    type Aes256CbcDec = Decryptor<Aes256>;

    let decryptor = Aes256CbcDec::new(key.into(), iv.into());
    let mut buf = ciphertext;
    let plaintext = decryptor.decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?;

    // Format: 16 random bytes + 4 bytes msg_len (big-endian) + msg + receiving_id
    if plaintext.len() < 20 {
        return None;
    }
    let msg_len =
        u32::from_be_bytes([plaintext[16], plaintext[17], plaintext[18], plaintext[19]]) as usize;
    if plaintext.len() < 20 + msg_len {
        return None;
    }
    let msg = std::str::from_utf8(&plaintext[20..20 + msg_len])
        .ok()?
        .to_string();
    let receiving_id = std::str::from_utf8(&plaintext[20 + msg_len..])
        .ok()?
        .to_string();
    Some((msg, receiving_id))
}

/// AES-256-CBC encrypt reply for WeCom
fn aes_encrypt(key: &[u8; 32], plaintext: &str, corp_id: &str) -> Option<String> {
    use aes::Aes256;
    use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    use cbc::Encryptor;
    type Aes256CbcEnc = Encryptor<Aes256>;

    let iv = &key[..16];

    let msg_bytes = plaintext.as_bytes();
    let corp_bytes = corp_id.as_bytes();
    let msg_len = msg_bytes.len() as u32;

    let mut content = Vec::with_capacity(16 + 4 + msg_bytes.len() + corp_bytes.len());
    let random_bytes = uuid::Uuid::new_v4();
    content.extend_from_slice(random_bytes.as_bytes());
    content.extend_from_slice(&msg_len.to_be_bytes());
    content.extend_from_slice(msg_bytes);
    content.extend_from_slice(corp_bytes);

    let msg_len_total = content.len();
    // Pad to block boundary (add at least 1 byte, up to 16)
    let pad_len = 16 - (msg_len_total % 16);
    content.resize(msg_len_total + pad_len, 0);

    let encryptor = Aes256CbcEnc::new(key.into(), iv.into());
    let ciphertext = encryptor
        .encrypt_padded_mut::<Pkcs7>(&mut content, msg_len_total)
        .ok()?;

    Some(BASE64.encode(ciphertext))
}

/// Extract CDATA content from simple flat XML by tag name
fn extract_cdata(xml: &str, tag: &str) -> Option<String> {
    let open_cdata = format!("<{tag}><![CDATA[");
    let close_cdata = "]]></";
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");

    if let Some(start) = xml.find(&open_cdata) {
        let content_start = start + open_cdata.len();
        if let Some(end) = xml[content_start..].find(close_cdata) {
            return Some(xml[content_start..content_start + end].to_string());
        }
    }

    if let Some(start) = xml.find(&open) {
        let content_start = start + open.len();
        if let Some(end) = xml[content_start..].find(&close) {
            return Some(xml[content_start..content_start + end].to_string());
        }
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
        Some((plain, _)) => WecomResponse::text(200, plain),
        None => WecomResponse::error(500, "failed to decrypt echostr"),
    }
}

/// Handle POST request: message callback
pub async fn handle_wecom_callback(
    query: &std::collections::HashMap<String, String>,
    body: &str,
    config: &AppConfig,
    data_dir: &Path,
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
    if msg_type != "text" {
        let reply_xml = encrypt_reply(&key, token, corp_id, NON_TEXT_REPLY, timestamp, nonce);
        return WecomResponse::xml(200, reply_xml);
    }

    let text = extract_cdata(&decrypted, "Content")
        .unwrap_or_default()
        .trim()
        .to_string();

    if text.is_empty() {
        return WecomResponse::xml(
            200,
            encrypt_reply(&key, token, corp_id, "消息为空", timestamp, nonce),
        );
    }

    let devices = build_device_list(config, data_dir);
    let client = match Client::builder().timeout(Duration::from_secs(35)).build() {
        Ok(c) => c,
        Err(e) => return WecomResponse::error(500, format!("HTTP client error: {e}")),
    };

    // WeCom 被动回复必须在 5 秒内返回，所以不发送"处理中"提示，直接执行命令并回包。
    let reply = handle_cmd(&client, &devices, &text, config.report_generation_timeout_secs)
        .await
        .unwrap_or_else(|| UNKNOWN_CMD_REPLY.to_string());

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
