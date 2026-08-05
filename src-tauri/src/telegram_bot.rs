use crate::bot_common::{
    build_device_list, handle_cmd, normalize_command, progress_text_for_command, DeviceEndpoint,
    NON_TEXT_REPLY, OUTPUT_DIVIDER,
};
use crate::config::AppConfig;
use crate::error::AppError;
use crate::AppState;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use subtle::ConstantTimeEq;
use tokio::task::JoinHandle;

#[derive(Deserialize)]
struct TgResp<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    error_code: Option<i64>,
}

#[derive(Deserialize)]
struct TgUpdate {
    update_id: i64,
    message: Option<TgMsg>,
}

#[derive(Deserialize)]
struct TgMsg {
    chat: TgChat,
    text: Option<String>,
}

#[derive(Deserialize)]
struct TgChat {
    id: i64,
}

const TELEGRAM_POLL_RETRY_SECONDS: u64 = 3;
const TELEGRAM_POLL_RETRY_MAX_SECONDS: u64 = 60;
const BINDING_COMMAND: &str = "start";
const BIND_COMMAND: &str = "bind";

#[derive(Default)]
struct SharedBotStatus {
    running: bool,
    starting: bool,
    last_error: Option<String>,
}

pub struct TelegramBotRuntime {
    handle: Option<JoinHandle<()>>,
    shared: Arc<std::sync::Mutex<SharedBotStatus>>,
}

impl Default for TelegramBotRuntime {
    fn default() -> Self {
        Self {
            handle: None,
            shared: Arc::new(std::sync::Mutex::new(SharedBotStatus::default())),
        }
    }
}

impl TelegramBotRuntime {
    fn stop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
        if let Ok(mut s) = self.shared.lock() {
            s.running = false;
            s.starting = false;
            s.last_error = None;
        }
    }

    fn start(
        &mut self,
        state: Arc<Mutex<AppState>>,
        bot_token: String,
        devices: Vec<DeviceEndpoint>,
        proxy: Option<String>,
        allowed_chat_ids: Vec<i64>,
    ) {
        self.stop();
        if let Ok(mut s) = self.shared.lock() {
            s.starting = true;
            s.running = false;
            s.last_error = None;
        }
        let shared = self.shared.clone();
        self.handle = Some(tokio::spawn(async move {
            run(
                state,
                &bot_token,
                &devices,
                &shared,
                proxy.as_deref(),
                allowed_chat_ids,
            )
            .await;
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

impl Drop for TelegramBotRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn sync_telegram_bot_runtime(state: &Arc<Mutex<AppState>>) -> Result<(), AppError> {
    let (enabled, bot_token, devices, proxy, allowed_chat_ids) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let enabled = s.config.telegram_bot_enabled;
        let bot_token = s.config.telegram_bot_token.clone();
        let proxy = s.config.telegram_bot_proxy.clone();
        let allowed_chat_ids = s.config.telegram_bot_allowed_chat_ids.clone();
        let devices = build_device_list(&s.config, &s.data_dir);
        (enabled, bot_token, devices, proxy, allowed_chat_ids)
    };

    let mut s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;

    if !enabled {
        s.telegram_bot_runtime.stop();
        return Ok(());
    }

    if bot_token.is_none() || bot_token.as_ref().is_none_or(|t| t.trim().is_empty()) {
        s.telegram_bot_runtime.stop();
        if let Ok(mut st) = s.telegram_bot_runtime.shared.lock() {
            st.last_error = Some("Bot Token 未填写".to_string());
        }
        return Ok(());
    }

    if devices.is_empty() {
        s.telegram_bot_runtime.stop();
        if let Ok(mut st) = s.telegram_bot_runtime.shared.lock() {
            st.last_error = Some("无可用设备（本地 API 未启用或 Token 未生成）".to_string());
        }
        return Ok(());
    }

    s.telegram_bot_runtime.start(
        state.clone(),
        bot_token.unwrap(),
        devices,
        proxy,
        allowed_chat_ids,
    );
    log::info!(
        "Telegram Bot 已启动 ({} 台设备)",
        s.config.node_devices.len() + 1
    );
    Ok(())
}

async fn run(
    state: Arc<Mutex<AppState>>,
    bot_token: &str,
    devices: &[DeviceEndpoint],
    shared: &Arc<std::sync::Mutex<SharedBotStatus>>,
    proxy: Option<&str>,
    mut allowed_chat_ids: Vec<i64>,
) {
    let mut builder = Client::builder().timeout(std::time::Duration::from_secs(35));
    if let Some(p) = proxy {
        if !p.trim().is_empty() {
            match reqwest::Proxy::all(p.trim()) {
                Ok(px) => {
                    builder = builder.proxy(px);
                }
                Err(e) => {
                    let msg = format!("代理配置无效: {e}");
                    log::error!("Telegram Bot {msg}");
                    set_error(shared, msg);
                    return;
                }
            }
        }
    }
    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            log::error!("创建 HTTP 客户端失败: {e}");
            set_error(shared, format!("HTTP 客户端创建失败: {e}"));
            return;
        }
    };

    let verify = format!("https://api.telegram.org/bot{bot_token}/getMe");
    match client
        .get(&verify)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            match resp.json::<TgResp<serde_json::Value>>().await {
                Ok(payload) if status.is_success() && payload.ok => {
                    log::info!("Telegram Bot token 验证通过");
                    set_running(shared, true);
                }
                Ok(payload) => {
                    let msg = format_telegram_http_error(
                        "Token 校验失败",
                        status,
                        payload.description.as_deref(),
                    );
                    log::error!("Telegram Bot {msg}");
                    set_error(shared, msg);
                    return;
                }
                Err(e) => {
                    let msg = format!("Token 校验响应解析失败: {e}");
                    log::error!("Telegram Bot {msg}");
                    set_error(shared, msg);
                    return;
                }
            }
        }
        Err(e) => {
            let msg = if e.is_connect() || e.is_timeout() {
                "无法连接 Telegram API（可能需要代理/VPN）".to_string()
            } else {
                format!(
                    "连接失败: {}",
                    redact_telegram_token(&e.to_string(), bot_token)
                )
            };
            log::error!("Telegram Bot {msg}");
            set_error(shared, msg);
            return;
        }
    }

    let devices = devices.to_vec();
    let mut offset = match consume_pending_updates(&client, bot_token).await {
        Ok(next_offset) => next_offset,
        Err(err) => {
            log::warn!("Telegram Bot 启动时清理历史更新失败，回退到 offset=0: {err}");
            0
        }
    };
    let mut consecutive_errors = 0u32;

    loop {
        let url = format!(
            "https://api.telegram.org/bot{bot_token}/getUpdates?offset={offset}&timeout=30"
        );
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.json::<TgResp<Vec<TgUpdate>>>().await {
                    Ok(body) => {
                        if !status.is_success() || !body.ok {
                            consecutive_errors += 1;
                            let msg = format_telegram_http_error(
                                "轮询失败",
                                status,
                                body.description.as_deref(),
                            );
                            if should_abort_polling(status, body.error_code) {
                                set_error(shared, msg.clone());
                                log::error!(
                                    "Telegram Bot 轮询遇到不可恢复错误，停止轮询: {msg}"
                                );
                                return;
                            }
                            let wait = poll_backoff_seconds(consecutive_errors);
                            set_transient_error(shared, &msg);
                            log::warn!(
                                "Telegram Bot 轮询异常(第{consecutive_errors}次): {msg}，{wait}秒后重试"
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                            continue;
                        }

                        if consecutive_errors > 0 {
                            consecutive_errors = 0;
                            // 从瞬态故障中恢复,清除状态面板上的错误提示
                            set_running(shared, true);
                        }
                        if let Some(updates) = body.result {
                            for u in updates {
                                offset = u.update_id + 1;
                                if let Some(msg) = u.message {
                                    let text = msg.text.as_deref();
                                    let cmd = text
                                        .map(|t| t.split_whitespace().next().unwrap_or(""))
                                        .map(normalize_command)
                                        .unwrap_or_default();
                                    let authorized = allowed_chat_ids.contains(&msg.chat.id);

                                    if !authorized {
                                        log::warn!("TG Bot 忽略未授权 chat_id: {}", msg.chat.id);
                                        let reply = if cmd == BINDING_COMMAND {
                                            binding_reply(msg.chat.id)
                                        } else if cmd == BIND_COMMAND {
                                            let result =
                                                handle_bind_command(&state, msg.chat.id, text);
                                            if let Some(bound_chat_id) = result.authorized_chat_id {
                                                if !allowed_chat_ids.contains(&bound_chat_id) {
                                                    allowed_chat_ids.push(bound_chat_id);
                                                }
                                            }
                                            result.reply
                                        } else {
                                            unauthorized_reply(msg.chat.id)
                                        };
                                        send_text(&client, bot_token, msg.chat.id, &reply).await;
                                        continue;
                                    }

                                    let reply = if let Some(text) = text {
                                        log::debug!("TG Bot 收到命令: {cmd}");
                                        if let Some(progress) = progress_text_for_command(&cmd) {
                                            send_chat_action(
                                                &client,
                                                bot_token,
                                                msg.chat.id,
                                                "typing",
                                            )
                                            .await;
                                            send_text(&client, bot_token, msg.chat.id, progress)
                                                .await;
                                        }
                                        handle_cmd(
                                            &client,
                                            &devices,
                                            text,
                                            state
                                                .lock()
                                                .map(|s| s.config.report_generation_timeout_secs)
                                                .unwrap_or(300),
                                        )
                                        .await
                                    } else {
                                        Some(NON_TEXT_REPLY.to_string())
                                    };
                                    if let Some(r) = reply {
                                        send_text(&client, bot_token, msg.chat.id, &r).await;
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        // 响应体解析失败但 HTTP 状态可用:鉴权/冲突类仍需停机,其余退避重试
                        if should_abort_polling(status, None) {
                            let msg = format!("轮询响应解析失败(HTTP {status}): {e}");
                            set_error(shared, msg.clone());
                            log::error!("Telegram Bot {msg}，停止轮询");
                            return;
                        }
                        let wait = poll_backoff_seconds(consecutive_errors);
                        set_transient_error(shared, &format!("轮询响应解析失败: {e}"));
                        log::warn!(
                            "Telegram Bot 轮询响应解析失败(第{consecutive_errors}次): {e}，{wait}秒后重试"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    }
                }
            }
            Err(e) => {
                consecutive_errors += 1;
                let msg = if e.is_connect() || e.is_timeout() {
                    "无法连接 Telegram API（可能需要代理/VPN）".to_string()
                } else {
                    format!(
                        "轮询失败: {}",
                        redact_telegram_token(&e.to_string(), bot_token)
                    )
                };
                let wait = poll_backoff_seconds(consecutive_errors);
                set_transient_error(shared, &msg);
                log::warn!("Telegram Bot {msg}(第{consecutive_errors}次)，{wait}秒后重试");
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
        }
    }
}

async fn consume_pending_updates(client: &Client, bot_token: &str) -> Result<i64, String> {
    let url =
        format!("https://api.telegram.org/bot{bot_token}/getUpdates?offset=-1&limit=1&timeout=0");
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            format!(
                "请求失败: {}",
                redact_telegram_token(&e.to_string(), bot_token)
            )
        })?;

    let status = resp.status();
    let body = resp
        .json::<TgResp<Vec<TgUpdate>>>()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))?;

    if !status.is_success() || !body.ok {
        return Err(format_telegram_http_error(
            "清理历史更新失败",
            status,
            body.description.as_deref(),
        ));
    }

    Ok(body
        .result
        .as_ref()
        .and_then(|updates| updates.last())
        .map(|u| u.update_id + 1)
        .unwrap_or(0))
}

// ── Bind code system ──

fn binding_reply(chat_id: i64) -> String {
    format!(
        "🔐 Work Review Bot 绑定\n{OUTPUT_DIVIDER}\n当前 Chat ID：{chat_id}\n请在 Work Review 设置页生成一次性绑定码，然后发送 /bind 绑定码完成授权。"
    )
}

fn unauthorized_reply(chat_id: i64) -> String {
    format!(
        "⛔ 该会话未被授权\n{OUTPUT_DIVIDER}\n当前 Chat ID：{chat_id}\n请发送 /start 查看绑定说明，或在 Work Review 设置页生成绑定码后发送 /bind 绑定码。"
    )
}

fn bind_usage_reply(chat_id: i64) -> String {
    format!(
        "🔐 请输入绑定码\n{OUTPUT_DIVIDER}\n当前 Chat ID：{chat_id}\n请在 Work Review 设置页生成一次性绑定码，然后发送 /bind 绑定码。"
    )
}

fn bind_not_configured_reply(chat_id: i64) -> String {
    format!(
        "⛔ 绑定码未启用\n{OUTPUT_DIVIDER}\n当前 Chat ID：{chat_id}\n请先在 Work Review 设置页生成一次性绑定码。"
    )
}

fn bind_expired_reply(chat_id: i64) -> String {
    format!(
        "⛔ 绑定码已过期\n{OUTPUT_DIVIDER}\n当前 Chat ID：{chat_id}\n请回到 Work Review 设置页重新生成绑定码。"
    )
}

fn bind_invalid_reply(chat_id: i64) -> String {
    format!(
        "⛔ 绑定码不正确\n{OUTPUT_DIVIDER}\n当前 Chat ID：{chat_id}\n请检查后重新发送 /bind 绑定码。"
    )
}

fn bind_success_reply(chat_id: i64) -> String {
    format!(
        "✅ 绑定成功\n{OUTPUT_DIVIDER}\nChat ID {chat_id} 已加入授权列表，现在可以使用 /report、/generate 和 /devices。"
    )
}

fn normalize_bind_code(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

/// 常量时间比较绑定码，防时序侧信道（与飞书/本地 API token 校验基线一致）。
/// 长度不同直接判不等，但不对内容做提前返回的逐字节比较。
fn bind_code_matches(provided: &str, expected: &str) -> bool {
    provided.as_bytes().ct_eq(expected.as_bytes()).into()
}

fn clear_bind_code(config: &mut AppConfig) {
    config.telegram_bot_bind_code = None;
    config.telegram_bot_bind_code_expires_at = None;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BindCodeStatus {
    NotConfigured,
    Expired,
    Ready(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindCodeApplyResult {
    NotConfigured,
    Expired,
    Invalid,
    Success,
}

fn configured_bind_code_status(
    expected_code: Option<&str>,
    expires_at: Option<i64>,
    now_ts: i64,
) -> BindCodeStatus {
    let Some(expected_code) = expected_code.map(str::trim).filter(|code| !code.is_empty()) else {
        return BindCodeStatus::NotConfigured;
    };

    match expires_at {
        Some(expires_at) if expires_at < now_ts => BindCodeStatus::Expired,
        _ => BindCodeStatus::Ready(expected_code.to_ascii_uppercase()),
    }
}

fn apply_bind_code_to_config(
    config: &mut AppConfig,
    chat_id: i64,
    input_code: &str,
    now_ts: i64,
) -> BindCodeApplyResult {
    let input_code = normalize_bind_code(input_code);
    let status = configured_bind_code_status(
        config.telegram_bot_bind_code.as_deref(),
        config.telegram_bot_bind_code_expires_at,
        now_ts,
    );

    match status {
        BindCodeStatus::NotConfigured => BindCodeApplyResult::NotConfigured,
        BindCodeStatus::Expired => {
            clear_bind_code(config);
            BindCodeApplyResult::Expired
        }
        BindCodeStatus::Ready(expected_code) if !bind_code_matches(&input_code, &expected_code) => {
            BindCodeApplyResult::Invalid
        }
        BindCodeStatus::Ready(_) => {
            if !config.telegram_bot_allowed_chat_ids.contains(&chat_id) {
                config.telegram_bot_allowed_chat_ids.push(chat_id);
            }
            clear_bind_code(config);
            BindCodeApplyResult::Success
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindCommandResult {
    reply: String,
    authorized_chat_id: Option<i64>,
}

impl BindCommandResult {
    fn reply(reply: String) -> Self {
        Self {
            reply,
            authorized_chat_id: None,
        }
    }

    fn success(reply: String, chat_id: i64) -> Self {
        Self {
            reply,
            authorized_chat_id: Some(chat_id),
        }
    }
}

fn handle_bind_command(
    state: &Arc<Mutex<AppState>>,
    chat_id: i64,
    text: Option<&str>,
) -> BindCommandResult {
    let Some(input_code) = text
        .and_then(|raw| raw.split_whitespace().nth(1))
        .map(normalize_bind_code)
        .filter(|code| !code.is_empty())
    else {
        return BindCommandResult::reply(bind_usage_reply(chat_id));
    };

    let now_ts = chrono::Local::now().timestamp();
    let mut state = match state.lock() {
        Ok(state) => state,
        Err(e) => {
            return BindCommandResult::reply(format!(
                "❌ 绑定失败\n{OUTPUT_DIVIDER}\n无法读取配置：{e}"
            ))
        }
    };

    let mut next_config = state.config.clone();
    let result = apply_bind_code_to_config(&mut next_config, chat_id, &input_code, now_ts);

    match result {
        BindCodeApplyResult::NotConfigured => {
            BindCommandResult::reply(bind_not_configured_reply(chat_id))
        }
        BindCodeApplyResult::Expired => {
            let config_path = state.config_path.clone();
            if let Err(e) = next_config.save(&config_path) {
                log::warn!("清理过期 Telegram Bot 绑定码失败: {e}");
            } else {
                state.config = next_config;
            }
            BindCommandResult::reply(bind_expired_reply(chat_id))
        }
        BindCodeApplyResult::Invalid => BindCommandResult::reply(bind_invalid_reply(chat_id)),
        BindCodeApplyResult::Success => {
            let config_path = state.config_path.clone();
            match next_config.save(&config_path) {
                Ok(_) => {
                    state.config = next_config;
                    BindCommandResult::success(bind_success_reply(chat_id), chat_id)
                }
                Err(e) => BindCommandResult::reply(format!(
                    "❌ 绑定失败\n{OUTPUT_DIVIDER}\n配置保存失败：{e}"
                )),
            }
        }
    }
}

// ── TG-specific helpers ──

async fn send_text(client: &Client, bot_token: &str, chat_id: i64, text: &str) {
    let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
    match client
        .post(&url)
        .json(&serde_json::json!({"chat_id": chat_id, "text": text}))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => log::warn!("Telegram sendMessage 失败 (HTTP {})", r.status()),
        Err(e) => log::warn!(
            "Telegram sendMessage 错误: {}",
            redact_telegram_token(&e.to_string(), bot_token)
        ),
    }
}

async fn send_chat_action(client: &Client, bot_token: &str, chat_id: i64, action: &str) {
    let url = format!("https://api.telegram.org/bot{bot_token}/sendChatAction");
    match client
        .post(&url)
        .json(&serde_json::json!({"chat_id": chat_id, "action": action}))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => log::warn!("Telegram sendChatAction 失败 (HTTP {})", r.status()),
        Err(e) => log::warn!(
            "Telegram sendChatAction 错误: {}",
            redact_telegram_token(&e.to_string(), bot_token)
        ),
    }
}

/// 仅鉴权/冲突类错误需要停止轮询（401/403 token 无效、409 webhook 冲突）。
/// 网络抖动、5xx、429 等瞬态错误交给调用方按 poll_backoff_seconds 退避重试——
/// 此前连续 3 次失败即永久停机,笔记本睡眠唤醒后 Wi-Fi 未就绪约 9 秒就会静默失联。
fn should_abort_polling(status: StatusCode, error_code: Option<i64>) -> bool {
    if matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::CONFLICT
    ) {
        return true;
    }
    matches!(error_code, Some(401) | Some(403) | Some(409))
}

fn redact_telegram_token(message: &str, bot_token: &str) -> String {
    if bot_token.is_empty() {
        message.to_string()
    } else {
        message.replace(bot_token, "[REDACTED]")
    }
}

fn format_telegram_http_error(
    action: &str,
    status: StatusCode,
    description: Option<&str>,
) -> String {
    let mut message = format!("{action} (HTTP {status})");
    if let Some(desc) = description.map(str::trim).filter(|d| !d.is_empty()) {
        message.push_str(": ");
        message.push_str(desc);
    }
    if status == StatusCode::CONFLICT {
        message.push_str("；请确认未配置 webhook 且仅运行一个 Bot 实例");
    }
    message
}

fn set_error(shared: &Arc<std::sync::Mutex<SharedBotStatus>>, msg: String) {
    if let Ok(mut s) = shared.lock() {
        s.running = false;
        s.starting = false;
        s.last_error = Some(msg);
    }
}

/// 记录瞬态错误但保持 running=true：轮询仍在退避重试,
/// 状态面板显示最近一次错误而非"已停止"。恢复后由 set_running 清除。
fn set_transient_error(shared: &Arc<std::sync::Mutex<SharedBotStatus>>, msg: &str) {
    if let Ok(mut s) = shared.lock() {
        s.last_error = Some(msg.to_string());
    }
}

/// 瞬态失败的退避间隔：3s 起步逐次翻倍,封顶 60s。
fn poll_backoff_seconds(consecutive_errors: u32) -> u64 {
    let shift = consecutive_errors.saturating_sub(1).min(5);
    (TELEGRAM_POLL_RETRY_SECONDS << shift).min(TELEGRAM_POLL_RETRY_MAX_SECONDS)
}

fn set_running(shared: &Arc<std::sync::Mutex<SharedBotStatus>>, running: bool) {
    if let Ok(mut s) = shared.lock() {
        s.running = running;
        s.starting = false;
        s.last_error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn 命令应支持带机器人用户名后缀() {
        assert_eq!(normalize_command("/start@WorkReviewBot"), "start");
        assert_eq!(normalize_command("/reports@work_review_bot"), "reports");
    }

    #[test]
    fn 网络类失败应退避重试而非停机() {
        assert!(!should_abort_polling(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(500)
        ));
        assert!(!should_abort_polling(StatusCode::TOO_MANY_REQUESTS, Some(429)));
        assert!(should_abort_polling(StatusCode::UNAUTHORIZED, Some(401)));

        assert_eq!(poll_backoff_seconds(1), 3);
        assert_eq!(poll_backoff_seconds(2), 6);
        assert_eq!(poll_backoff_seconds(4), 24);
        assert_eq!(poll_backoff_seconds(10), 60);
    }

    #[test]
    fn 轮询冲突应立即中止并提示() {
        assert!(should_abort_polling(StatusCode::CONFLICT, Some(409)));
        let message = format_telegram_http_error(
            "轮询失败",
            StatusCode::CONFLICT,
            Some("Conflict: terminated by other getUpdates request"),
        );
        assert!(message.contains("HTTP 409"));
        assert!(message.contains("webhook"));
    }

    #[test]
    fn 传输错误不得把_bot_token_写入日志或状态() {
        let token = "123456:secret-token";
        let error = format!("request failed for https://api.telegram.org/bot{token}/getUpdates");
        let redacted = redact_telegram_token(&error, token);

        assert!(!redacted.contains(token));
        assert!(redacted.contains("bot[REDACTED]/getUpdates"));
    }

    #[test]
    fn 非文本消息应返回帮助提示() {
        assert!(NON_TEXT_REPLY.contains("/help"));
    }

    #[test]
    fn 绑定与未授权提示应包含当前_chat_id() {
        let binding = binding_reply(123456789);
        let unauthorized = unauthorized_reply(123456789);

        assert!(binding.contains("当前 Chat ID：123456789"));
        assert!(binding.contains("/bind"));
        assert!(unauthorized.contains("该会话未被授权"));
        assert!(unauthorized.contains("当前 Chat ID：123456789"));
    }

    #[test]
    fn 绑定码状态应区分未配置_过期与可用() {
        assert_eq!(
            configured_bind_code_status(None, None, 100),
            BindCodeStatus::NotConfigured
        );
        assert_eq!(
            configured_bind_code_status(Some("  "), Some(200), 100),
            BindCodeStatus::NotConfigured
        );
        assert_eq!(
            configured_bind_code_status(Some("wr-ab12"), Some(99), 100),
            BindCodeStatus::Expired
        );
        assert_eq!(
            configured_bind_code_status(Some("wr-ab12"), Some(100), 100),
            BindCodeStatus::Ready("WR-AB12".to_string())
        );
    }

    #[test]
    fn 绑定码应忽略大小写与首尾空白() {
        assert_eq!(normalize_bind_code(" wr-a1b2 "), "WR-A1B2");
    }

    #[test]
    fn 正确绑定码应添加_chat_id_并立即失效() {
        let mut config = AppConfig {
            telegram_bot_bind_code: Some("wr-a1b2".to_string()),
            telegram_bot_bind_code_expires_at: Some(200),
            ..AppConfig::default()
        };

        let result = apply_bind_code_to_config(&mut config, 123456789, " WR-A1B2 ", 100);

        assert_eq!(result, BindCodeApplyResult::Success);
        assert_eq!(config.telegram_bot_allowed_chat_ids, vec![123456789]);
        assert_eq!(config.telegram_bot_bind_code, None);
        assert_eq!(config.telegram_bot_bind_code_expires_at, None);
    }

    #[test]
    fn 错误绑定码不应修改授权列表或消耗绑定码() {
        let mut config = AppConfig {
            telegram_bot_bind_code: Some("WR-A1B2".to_string()),
            telegram_bot_bind_code_expires_at: Some(200),
            ..AppConfig::default()
        };

        let result = apply_bind_code_to_config(&mut config, 123456789, "WR-FFFF", 100);

        assert_eq!(result, BindCodeApplyResult::Invalid);
        assert!(config.telegram_bot_allowed_chat_ids.is_empty());
        assert_eq!(config.telegram_bot_bind_code.as_deref(), Some("WR-A1B2"));
        assert_eq!(config.telegram_bot_bind_code_expires_at, Some(200));
    }

    #[test]
    fn 过期绑定码应清理但不授权() {
        let mut config = AppConfig {
            telegram_bot_bind_code: Some("WR-A1B2".to_string()),
            telegram_bot_bind_code_expires_at: Some(99),
            ..AppConfig::default()
        };

        let result = apply_bind_code_to_config(&mut config, 123456789, "WR-A1B2", 100);

        assert_eq!(result, BindCodeApplyResult::Expired);
        assert!(config.telegram_bot_allowed_chat_ids.is_empty());
        assert_eq!(config.telegram_bot_bind_code, None);
        assert_eq!(config.telegram_bot_bind_code_expires_at, None);
    }

    #[test]
    fn 应能从清理结果中计算下一次轮询offset() {
        let payload = TgResp {
            ok: true,
            result: Some(vec![
                TgUpdate {
                    update_id: 100,
                    message: None,
                },
                TgUpdate {
                    update_id: 101,
                    message: None,
                },
            ]),
            description: None,
            error_code: None,
        };
        let next = payload
            .result
            .as_ref()
            .and_then(|updates| updates.last())
            .map(|u| u.update_id + 1)
            .unwrap_or(0);
        assert_eq!(next, 102);
    }
}
