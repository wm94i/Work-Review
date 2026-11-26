//! 企业微信智能机器人 WebSocket 长连接。
//!
//! 桌面端没有稳定公网回调，因此默认走官方长连接：
//! `wss://openws.work.weixin.qq.com` + Bot ID / Secret。
//! HTTP 回调（`wecom_bot.rs`）仍保留给已有公网地址的用户。

use crate::bot_common::{build_device_list, handle_cmd, HELP, NON_TEXT_REPLY, UNKNOWN_CMD_REPLY};
use crate::AppState;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message;
use work_review_core::config::AppConfig;
use work_review_core::error::AppError;

pub const WECOM_WS_URL: &str = "wss://openws.work.weixin.qq.com";
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(15);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const MARKDOWN_MAX_BYTES: usize = 20_480;
const RECONNECT_RETRY_SECONDS: u64 = 3;
const RECONNECT_RETRY_MAX_SECONDS: u64 = 60;
const KICKED_RETRY_SECONDS: u64 = 60;
static WECOM_AIBOT_NL_SEMAPHORE: once_cell::sync::Lazy<Arc<tokio::sync::Semaphore>> =
    once_cell::sync::Lazy::new(|| Arc::new(tokio::sync::Semaphore::new(4)));

#[derive(Default)]
struct SharedBotStatus {
    running: bool,
    starting: bool,
    last_error: Option<String>,
}

pub struct WecomBotRuntime {
    handle: Option<JoinHandle<()>>,
    shared: Arc<std::sync::Mutex<SharedBotStatus>>,
    identity: Option<(String, String)>,
}

impl Default for WecomBotRuntime {
    fn default() -> Self {
        Self {
            handle: None,
            shared: Arc::new(std::sync::Mutex::new(SharedBotStatus::default())),
            identity: None,
        }
    }
}

impl WecomBotRuntime {
    fn stop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
        self.identity = None;
        if let Ok(mut s) = self.shared.lock() {
            s.running = false;
            s.starting = false;
            s.last_error = None;
        }
    }

    fn should_keep(&self, bot_id: &str, secret: &str) -> bool {
        should_reuse_wecom_session(
            self.handle
                .as_ref()
                .is_some_and(|handle| !handle.is_finished()),
            self.identity.as_ref(),
            bot_id,
            secret,
        )
    }

    fn start(&mut self, state: Arc<Mutex<AppState>>, bot_id: String, secret: String) {
        self.stop();
        if let Ok(mut s) = self.shared.lock() {
            s.starting = true;
            s.running = false;
            s.last_error = None;
        }
        self.identity = Some((bot_id.clone(), secret.clone()));
        let shared = self.shared.clone();
        self.handle = Some(tokio::spawn(async move {
            run(state, bot_id, secret, shared).await;
        }));
    }

    pub fn is_starting(&self) -> bool {
        self.shared.lock().map(|s| s.starting).unwrap_or(false)
    }

    pub fn is_running(&self) -> bool {
        self.shared.lock().map(|s| s.running).unwrap_or(false)
    }

    pub fn last_error(&self) -> Option<String> {
        self.shared.lock().ok().and_then(|s| s.last_error.clone())
    }
}

impl Drop for WecomBotRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn long_connection_configured(config: &AppConfig) -> bool {
    optional_nonempty(config.wecom_bot_id.as_deref())
        && optional_nonempty(config.wecom_bot_secret.as_deref())
}

pub fn callback_configured(config: &AppConfig) -> bool {
    optional_nonempty(config.wecom_corp_id.as_deref())
        && optional_nonempty(config.wecom_token.as_deref())
        && optional_nonempty(config.wecom_encoding_aes_key.as_deref())
}

fn optional_nonempty(value: Option<&str>) -> bool {
    value.is_some_and(|s| !s.trim().is_empty())
}

pub fn sync_wecom_bot_runtime(state: &Arc<Mutex<AppState>>) -> Result<(), AppError> {
    let (enabled, bot_id, secret, callback_only) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let enabled = s.config.wecom_bot_enabled;
        let long_conn = long_connection_configured(&s.config);
        let callback_only = enabled && !long_conn && callback_configured(&s.config);
        (
            enabled,
            s.config.wecom_bot_id.clone(),
            s.config.wecom_bot_secret.clone(),
            callback_only,
        )
    };

    let mut s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;

    if !enabled {
        s.wecom_bot_runtime.stop();
        return Ok(());
    }

    if callback_only {
        s.wecom_bot_runtime.stop();
        return Ok(());
    }

    let bot_id = bot_id.filter(|v| !v.trim().is_empty());
    let secret = secret.filter(|v| !v.trim().is_empty());
    if bot_id.is_none() || secret.is_none() {
        s.wecom_bot_runtime.stop();
        if let Ok(mut st) = s.wecom_bot_runtime.shared.lock() {
            st.last_error = Some("请填写智能机器人 Bot ID 和 Secret（长连接无需公网）".to_string());
        }
        return Ok(());
    }

    let bot_id = bot_id.unwrap().trim().to_string();
    let secret = secret.unwrap().trim().to_string();
    if s.wecom_bot_runtime.should_keep(&bot_id, &secret) {
        return Ok(());
    }

    s.wecom_bot_runtime.start(state.clone(), bot_id, secret);
    log::info!("企业微信智能机器人长连接已启动");
    Ok(())
}

pub(crate) fn should_reuse_wecom_session(
    task_alive: bool,
    current: Option<&(String, String)>,
    bot_id: &str,
    secret: &str,
) -> bool {
    task_alive
        && current.is_some_and(|(id, sec)| id == bot_id && sec == secret)
        && !bot_id.is_empty()
        && !secret.is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEnd {
    Closed,
    Kicked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncomingKind {
    SubscribeAck,
    PingAck,
    TextMessage,
    NonTextMessage,
    EnterChat,
    Disconnected,
    Ignored,
}

#[derive(Debug, Deserialize)]
struct WsFrame {
    #[serde(default)]
    cmd: Option<String>,
    #[serde(default)]
    headers: Option<WsHeaders>,
    #[serde(default)]
    body: Option<serde_json::Value>,
    #[serde(default)]
    errcode: Option<i64>,
    #[serde(default)]
    errmsg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsHeaders {
    #[serde(default)]
    req_id: Option<String>,
}

pub fn extract_command_text(content: &str) -> String {
    let mut s = content.trim();
    while let Some(stripped) = s.strip_prefix('@') {
        match stripped.find(char::is_whitespace) {
            Some(index) => s = stripped[index..].trim_start(),
            None => return String::new(),
        }
    }
    s.to_string()
}

pub fn truncate_utf8_bytes(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

pub fn redact_secret(message: &str, secret: &str) -> String {
    if secret.is_empty() {
        message.to_string()
    } else {
        message.replace(secret, "[REDACTED]")
    }
}

pub fn reconnect_backoff_seconds(consecutive_errors: u32) -> u64 {
    let shift = consecutive_errors.saturating_sub(1).min(5);
    (RECONNECT_RETRY_SECONDS << shift).min(RECONNECT_RETRY_MAX_SECONDS)
}

fn new_req_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn build_subscribe_frame(req_id: &str, bot_id: &str, secret: &str) -> String {
    serde_json::json!({
        "cmd": "aibot_subscribe",
        "headers": { "req_id": req_id },
        "body": { "bot_id": bot_id, "secret": secret },
    })
    .to_string()
}

pub fn build_ping_frame(req_id: &str) -> String {
    serde_json::json!({
        "cmd": "ping",
        "headers": { "req_id": req_id },
    })
    .to_string()
}

pub fn build_respond_markdown_frame(req_id: &str, content: &str) -> String {
    let content = truncate_utf8_bytes(content, MARKDOWN_MAX_BYTES);
    serde_json::json!({
        "cmd": "aibot_respond_msg",
        "headers": { "req_id": req_id },
        "body": {
            "msgtype": "markdown",
            "markdown": { "content": content },
        },
    })
    .to_string()
}

pub fn build_welcome_frame(req_id: &str, content: &str) -> String {
    serde_json::json!({
        "cmd": "aibot_respond_welcome_msg",
        "headers": { "req_id": req_id },
        "body": {
            "msgtype": "text",
            "text": { "content": content },
        },
    })
    .to_string()
}

fn parse_frame(text: &str) -> Option<WsFrame> {
    serde_json::from_str(text).ok()
}

fn frame_req_id(frame: &WsFrame) -> Option<&str> {
    frame
        .headers
        .as_ref()
        .and_then(|h| h.req_id.as_deref())
        .filter(|id| !id.is_empty())
}

pub(crate) fn classify_incoming(text: &str, pending_subscribe_id: Option<&str>) -> IncomingKind {
    let Some(frame) = parse_frame(text) else {
        return IncomingKind::Ignored;
    };
    let cmd = frame.cmd.as_deref().unwrap_or("");
    if cmd == "aibot_event_callback" {
        let event_type = frame
            .body
            .as_ref()
            .and_then(|b| b.get("event"))
            .and_then(|e| e.get("eventtype"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return match event_type {
            "disconnected_event" => IncomingKind::Disconnected,
            "enter_chat" => IncomingKind::EnterChat,
            _ => IncomingKind::Ignored,
        };
    }
    if cmd == "aibot_msg_callback" {
        let msg_type = frame
            .body
            .as_ref()
            .and_then(|b| b.get("msgtype"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return if msg_type == "text" {
            IncomingKind::TextMessage
        } else {
            IncomingKind::NonTextMessage
        };
    }
    if pending_subscribe_id.is_some()
        && frame_req_id(&frame) == pending_subscribe_id
        && frame.errcode.is_some()
    {
        return IncomingKind::SubscribeAck;
    }
    if cmd.is_empty() && frame.errcode.is_some() {
        return IncomingKind::PingAck;
    }
    IncomingKind::Ignored
}

pub fn subscribe_ack_ok(text: &str) -> Result<(), String> {
    let frame = parse_frame(text).ok_or_else(|| "订阅响应无法解析".to_string())?;
    match frame.errcode.unwrap_or(-1) {
        0 => Ok(()),
        code => {
            let detail = frame.errmsg.unwrap_or_else(|| "unknown".to_string());
            Err(format!("订阅失败 (errcode {code}): {detail}"))
        }
    }
}

pub fn extract_callback_req_id(text: &str) -> Option<String> {
    parse_frame(text).and_then(|frame| frame_req_id(&frame).map(str::to_string))
}

/// 提取消息体中的发送者 userid 与消息 ID（用于发送者授权与去重）。
pub fn extract_sender_and_message_id(text: &str) -> (String, String) {
    let Some(frame) = parse_frame(text) else {
        return (String::new(), String::new());
    };
    let Some(body) = frame.body.as_ref() else {
        return (String::new(), String::new());
    };
    let sender = body
        .get("from")
        .and_then(|from| from.get("userid"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let message_id = body
        .get("msgid")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    (sender, message_id)
}

pub fn extract_text_content(text: &str) -> Option<String> {
    let frame = parse_frame(text)?;
    let content = frame
        .body
        .as_ref()?
        .get("text")?
        .get("content")?
        .as_str()?
        .to_string();
    Some(extract_command_text(&content))
}

fn set_error(shared: &Arc<std::sync::Mutex<SharedBotStatus>>, msg: String) {
    if let Ok(mut s) = shared.lock() {
        s.running = false;
        s.starting = false;
        s.last_error = Some(msg);
    }
}

fn set_transient_error(shared: &Arc<std::sync::Mutex<SharedBotStatus>>, msg: &str) {
    if let Ok(mut s) = shared.lock() {
        s.last_error = Some(msg.to_string());
    }
}

fn set_running(shared: &Arc<std::sync::Mutex<SharedBotStatus>>, running: bool) {
    if let Ok(mut s) = shared.lock() {
        s.running = running;
        s.starting = false;
        if running {
            s.last_error = None;
        }
    }
}

async fn run(
    state: Arc<Mutex<AppState>>,
    bot_id: String,
    secret: String,
    shared: Arc<std::sync::Mutex<SharedBotStatus>>,
) {
    let mut consecutive_errors = 0u32;
    loop {
        match connect_and_serve(&state, &bot_id, &secret, &shared).await {
            Ok(SessionEnd::Kicked) => {
                consecutive_errors += 1;
                let wait = KICKED_RETRY_SECONDS;
                let msg = format!(
                    "长连接被其他实例占用，{wait} 秒后重试（同一机器人只能保持一条长连接）"
                );
                log::warn!("企业微信 Bot {msg}");
                set_transient_error(&shared, &msg);
                set_running(&shared, false);
                if let Ok(mut s) = shared.lock() {
                    s.starting = true;
                }
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            Ok(SessionEnd::Closed) => {
                consecutive_errors += 1;
                let wait = reconnect_backoff_seconds(consecutive_errors);
                let msg = format!("长连接已断开，{wait} 秒后重连");
                log::warn!("企业微信 Bot {msg}");
                set_transient_error(&shared, &msg);
                if let Ok(mut s) = shared.lock() {
                    s.running = false;
                    s.starting = true;
                }
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            Err(err) => {
                let err = redact_secret(&err, &secret);
                if err.starts_with("订阅失败") {
                    log::error!("企业微信 Bot {err}");
                    set_error(&shared, err);
                    return;
                }
                consecutive_errors += 1;
                let wait = reconnect_backoff_seconds(consecutive_errors);
                let msg = format!("{err}；{wait} 秒后重连");
                log::warn!("企业微信 Bot {msg}");
                set_transient_error(&shared, &msg);
                if let Ok(mut s) = shared.lock() {
                    s.running = false;
                    s.starting = true;
                }
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        }
    }
}

async fn connect_and_serve(
    state: &Arc<Mutex<AppState>>,
    bot_id: &str,
    secret: &str,
    shared: &Arc<std::sync::Mutex<SharedBotStatus>>,
) -> Result<SessionEnd, String> {
    let (ws, _) = tokio_tungstenite::connect_async(WECOM_WS_URL)
        .await
        .map_err(|e| redact_secret(&format!("连接企业微信长连接失败: {e}"), secret))?;
    let (mut write, mut read) = ws.split();

    let subscribe_id = new_req_id();
    write
        .send(Message::Text(build_subscribe_frame(
            &subscribe_id,
            bot_id,
            secret,
        )))
        .await
        .map_err(|e| redact_secret(&format!("发送订阅请求失败: {e}"), secret))?;

    let ack = wait_subscribe_ack(&mut read, &subscribe_id).await?;
    subscribe_ack_ok(&ack).map_err(|e| redact_secret(&e, secret))?;
    set_running(shared, true);
    log::info!("企业微信智能机器人长连接已订阅");

    let http = crate::bot_common::shared_bot_http_client();

    let mut ping_ticks = interval(HEARTBEAT_INTERVAL);
    ping_ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let (out_tx, mut out_rx) = mpsc::channel::<String>(16);

    loop {
        tokio::select! {
            _ = ping_ticks.tick() => {
                if let Err(e) = write.send(Message::Text(build_ping_frame(&new_req_id()))).await {
                    return Err(redact_secret(&format!("心跳发送失败: {e}"), secret));
                }
            }
            outgoing = out_rx.recv() => {
                let Some(frame) = outgoing else {
                    return Ok(SessionEnd::Closed);
                };
                if let Err(e) = write.send(Message::Text(frame)).await {
                    return Err(redact_secret(&format!("发送回复失败: {e}"), secret));
                }
            }
            incoming = read.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match classify_incoming(&text, None) {
                            IncomingKind::Disconnected => return Ok(SessionEnd::Kicked),
                            IncomingKind::TextMessage => {
                                let state = state.clone();
                                let http = http.clone();
                                let out_tx = out_tx.clone();
                                tokio::spawn(async move {
                                    if let Some(frame) =
                                        handle_text_message(&state, &http, &text).await
                                    {
                                        let _ = out_tx.send(frame).await;
                                    }
                                });
                            }
                            IncomingKind::NonTextMessage => {
                                if let Some(req_id) = extract_callback_req_id(&text) {
                                    let (sender_id, message_id) =
                                        extract_sender_and_message_id(&text);
                                    match crate::bot_common::gate_bot_message(
                                        state,
                                        crate::bot_common::BOT_PLATFORM_WECOM,
                                        &message_id,
                                        &sender_id,
                                        "",
                                    ) {
                                        crate::bot_common::BotMessageGate::Duplicate => {}
                                        crate::bot_common::BotMessageGate::Reply(reply) => {
                                            let _ = write
                                                .send(Message::Text(build_respond_markdown_frame(
                                                    &req_id, &reply,
                                                )))
                                                .await;
                                        }
                                        crate::bot_common::BotMessageGate::Proceed => {
                                            let _ = write
                                                .send(Message::Text(build_respond_markdown_frame(
                                                    &req_id,
                                                    NON_TEXT_REPLY,
                                                )))
                                                .await;
                                        }
                                    }
                                }
                            }
                            IncomingKind::EnterChat => {
                                if let Some(req_id) = extract_callback_req_id(&text) {
                                    let (sender_id, _) = extract_sender_and_message_id(&text);
                                    let welcome = if crate::bot_common::is_sender_authorized(
                                        state,
                                        crate::bot_common::BOT_PLATFORM_WECOM,
                                        &sender_id,
                                    ) {
                                        HELP.to_string()
                                    } else {
                                        crate::bot_common::BotSenderDecision::NeedBind
                                            .reply()
                                            .unwrap_or_default()
                                    };
                                    let _ = write
                                        .send(Message::Text(build_welcome_frame(
                                            &req_id, &welcome,
                                        )))
                                        .await;
                                }
                            }
                            IncomingKind::SubscribeAck | IncomingKind::PingAck | IncomingKind::Ignored => {}
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = write.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => return Ok(SessionEnd::Closed),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        return Err(redact_secret(&format!("长连接读取失败: {e}"), secret));
                    }
                }
            }
        }
    }
}

async fn wait_subscribe_ack<S>(read: &mut S, subscribe_id: &str) -> Result<String, String>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    timeout(SUBSCRIBE_TIMEOUT, async {
        while let Some(item) = read.next().await {
            let message = item.map_err(|e| format!("读取订阅响应失败: {e}"))?;
            if let Message::Text(text) = message {
                if classify_incoming(&text, Some(subscribe_id)) == IncomingKind::SubscribeAck {
                    return Ok(text);
                }
            }
        }
        Err("订阅响应前连接已关闭".to_string())
    })
    .await
    .map_err(|_| "订阅企业微信长连接超时".to_string())?
}

async fn handle_text_message(
    state: &Arc<Mutex<AppState>>,
    http: &reqwest::Client,
    text: &str,
) -> Option<String> {
    let req_id = extract_callback_req_id(text)?;
    let content = extract_text_content(text).unwrap_or_default();

    // 统一入站管线：去重 + 发送者授权 + 空文本 + NL 并发/限流 + 命令分发
    let (sender_id, message_id) = extract_sender_and_message_id(text);
    let inbound = crate::bot_common::InboundBotMessage::standard(
        crate::bot_common::BOT_PLATFORM_WECOM,
        &message_id,
        &sender_id,
        &content,
        true,
    );
    let nl_enabled = crate::bot_common::is_bot_natural_language_enabled(state);

    match crate::bot_common::route_inbound_bot_message(
        state,
        &inbound,
        &WECOM_AIBOT_NL_SEMAPHORE,
        nl_enabled,
        Some(crate::bot_common::EMPTY_TEXT_REPLY),
    ) {
        crate::bot_common::BotPipelineAction::Duplicate => None,
        crate::bot_common::BotPipelineAction::ImmediateReply { text: reply, .. } => {
            Some(build_respond_markdown_frame(&req_id, &reply))
        }
        crate::bot_common::BotPipelineAction::NaturalLanguage { permit, question } => {
            let _permit = permit;
            // 已由 connect_and_serve 中的 tokio::spawn 异步执行并通过 out_tx 回写，不阻塞 WS 心跳与读循环
            let reply = crate::bot_common::run_bot_natural_language_query(
                state,
                &question,
                crate::bot_common::BOT_PLATFORM_WECOM,
            )
            .await;
            Some(build_respond_markdown_frame(&req_id, &reply))
        }
        crate::bot_common::BotPipelineAction::Command { text: cmd_text, .. } => {
            let (devices, generate_timeout) = match state.lock() {
                Ok(s) => (
                    build_device_list(&s.config, &s.data_dir),
                    Duration::from_secs(s.config.report_generation_timeout_secs.max(60)),
                ),
                Err(e) => {
                    log::warn!("企业微信 Bot 获取设备列表失败: {e}");
                    (Vec::new(), Duration::from_secs(120))
                }
            };

            let reply = handle_cmd(http, &devices, &cmd_text, generate_timeout)
                .await
                .unwrap_or_else(|| UNKNOWN_CMD_REPLY.to_string());
            Some(build_respond_markdown_frame(&req_id, &reply))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 应能提取发送者与消息id() {
        let frame = r#"{"cmd":"aibot_msg_callback","headers":{"req_id":"r1"},"body":{"msgid":"m-1","from":{"userid":"zhangsan"},"msgtype":"text","text":{"content":"/help"}}}"#;
        assert_eq!(
            extract_sender_and_message_id(frame),
            ("zhangsan".to_string(), "m-1".to_string())
        );
        assert_eq!(
            extract_sender_and_message_id("not json"),
            (String::new(), String::new())
        );
    }

    #[test]
    fn 群聊at前缀应剥离后留下命令() {
        assert_eq!(extract_command_text("@RobotA /help"), "/help");
        assert_eq!(
            extract_command_text("@机器人  /report today"),
            "/report today"
        );
        assert_eq!(extract_command_text("/generate today"), "/generate today");
        assert_eq!(extract_command_text("  @bot"), "");
        assert_eq!(extract_command_text("@a @b /devices"), "/devices");
    }

    #[test]
    fn 长连接配置应同时要求bot_id与secret() {
        let mut config = AppConfig::default();
        assert!(!long_connection_configured(&config));
        config.wecom_bot_id = Some("bot".to_string());
        assert!(!long_connection_configured(&config));
        config.wecom_bot_secret = Some("secret".to_string());
        assert!(long_connection_configured(&config));
        config.wecom_bot_secret = Some("   ".to_string());
        assert!(!long_connection_configured(&config));
    }

    #[test]
    fn 订阅与回复帧应包含协议字段() {
        let subscribe = build_subscribe_frame("req-1", "bot-id", "s3cret");
        assert!(subscribe.contains("\"cmd\":\"aibot_subscribe\""));
        assert!(subscribe.contains("\"bot_id\":\"bot-id\""));
        assert!(subscribe.contains("\"secret\":\"s3cret\""));
        assert!(subscribe.contains("\"req_id\":\"req-1\""));

        let ping = build_ping_frame("req-2");
        assert!(ping.contains("\"cmd\":\"ping\""));

        let reply = build_respond_markdown_frame("req-3", "hello **bot**");
        assert!(reply.contains("\"cmd\":\"aibot_respond_msg\""));
        assert!(reply.contains("\"msgtype\":\"markdown\""));
        assert!(reply.contains("hello **bot**"));
        assert!(reply.contains("\"req_id\":\"req-3\""));
    }

    #[test]
    fn markdown超长应在utf8边界截断() {
        let content = "你好".repeat(20_000);
        let frame = build_respond_markdown_frame("r", &content);
        let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
        let clipped = parsed["body"]["markdown"]["content"].as_str().unwrap();
        assert!(clipped.len() <= MARKDOWN_MAX_BYTES);
        assert!(clipped.is_char_boundary(clipped.len()));
    }

    #[test]
    fn 应识别文本消息踢线与进入会话() {
        let text_msg = r#"{"cmd":"aibot_msg_callback","headers":{"req_id":"r1"},"body":{"msgtype":"text","text":{"content":"@Bot /help"}}}"#;
        assert_eq!(classify_incoming(text_msg, None), IncomingKind::TextMessage);
        assert_eq!(extract_text_content(text_msg).as_deref(), Some("/help"));
        assert_eq!(extract_callback_req_id(text_msg).as_deref(), Some("r1"));

        let image_msg =
            r#"{"cmd":"aibot_msg_callback","headers":{"req_id":"r2"},"body":{"msgtype":"image"}}"#;
        assert_eq!(
            classify_incoming(image_msg, None),
            IncomingKind::NonTextMessage
        );

        let kicked = r#"{"cmd":"aibot_event_callback","body":{"msgtype":"event","event":{"eventtype":"disconnected_event"}}}"#;
        assert_eq!(classify_incoming(kicked, None), IncomingKind::Disconnected);

        let enter = r#"{"cmd":"aibot_event_callback","headers":{"req_id":"r3"},"body":{"msgtype":"event","event":{"eventtype":"enter_chat"}}}"#;
        assert_eq!(classify_incoming(enter, None), IncomingKind::EnterChat);
    }

    #[test]
    fn 订阅响应应按errcode判定成败() {
        let ok = r#"{"headers":{"req_id":"sub-1"},"errcode":0,"errmsg":"ok"}"#;
        assert_eq!(
            classify_incoming(ok, Some("sub-1")),
            IncomingKind::SubscribeAck
        );
        assert!(subscribe_ack_ok(ok).is_ok());

        let bad = r#"{"headers":{"req_id":"sub-1"},"errcode":40001,"errmsg":"invalid secret"}"#;
        assert!(subscribe_ack_ok(bad).unwrap_err().contains("40001"));
    }

    #[test]
    fn 密钥不得出现在错误信息中() {
        let secret = "super-secret-value";
        let raw = format!("connect failed secret={secret}");
        let redacted = redact_secret(&raw, secret);
        assert!(!redacted.contains(secret));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn 重连退避应指数上升并封顶() {
        assert_eq!(reconnect_backoff_seconds(1), 3);
        assert_eq!(reconnect_backoff_seconds(2), 6);
        assert_eq!(reconnect_backoff_seconds(4), 24);
        assert_eq!(reconnect_backoff_seconds(10), 60);
    }

    #[test]
    fn 欢迎语帧应走text类型() {
        let frame = build_welcome_frame("r", HELP);
        assert!(frame.contains("aibot_respond_welcome_msg"));
        assert!(frame.contains("\"msgtype\":\"text\""));
        assert!(frame.contains("/help"));
    }

    #[test]
    fn 凭证未变且任务仍存活时不应重连() {
        let identity = ("bot".to_string(), "secret".to_string());
        assert!(should_reuse_wecom_session(
            true,
            Some(&identity),
            "bot",
            "secret"
        ));
        assert!(!should_reuse_wecom_session(
            false,
            Some(&identity),
            "bot",
            "secret"
        ));
        assert!(!should_reuse_wecom_session(
            true,
            Some(&identity),
            "bot",
            "other"
        ));
        assert!(!should_reuse_wecom_session(true, None, "bot", "secret"));
    }
}
