use crate::config::{AppConfig, DEFAULT_LOCALHOST_API_PORT};
use crate::error::AppError;
use crate::localhost_api::LOCALHOST_API_HOST;
use crate::AppState;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceEndpoint {
    name: String,
    url: String,
    token: String,
    is_local: bool,
}

const HELP: &str = "\
📊 Work Review Bot（多设备）

常用命令
/help                      查看帮助
/devices                   查看所有设备
/device [设备名]           查看设备状态（默认本机）
/reports [设备名]          查看最近日报日期
/report [日期] [设备名]    查看指定日报
/generate [日期] [设备名]  生成日报

参数说明
- [设备名] 可选，不填默认本机
- [日期] 可选，不填默认 today
- 日期支持：YYYY-MM-DD / today / yesterday

示例
- /generate today
- /report 2026-04-25
- /reports 本机";
const UNKNOWN_CMD_REPLY: &str = "未知命令。发送 /help 查看帮助，例如：/generate today";
const NON_TEXT_REPLY: &str = "暂不支持非文本消息，发送 /help 查看帮助";
const TELEGRAM_POLL_MAX_ERRORS: u32 = 3;
const TELEGRAM_POLL_RETRY_SECONDS: u64 = 3;
const OUTPUT_DIVIDER: &str = "────────────";
const BINDING_COMMAND: &str = "/start";
const BIND_COMMAND: &str = "/bind";

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

fn effective_host(config: &AppConfig) -> String {
    config
        .localhost_api_host
        .as_deref()
        .map(|h| h.trim())
        .filter(|h| !h.is_empty())
        .unwrap_or(LOCALHOST_API_HOST)
        .to_string()
}

fn read_api_token(data_dir: &Path) -> Option<String> {
    let path = data_dir.join("localhost_api_token.txt");
    std::fs::read_to_string(&path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn build_device_list(config: &AppConfig, data_dir: &Path) -> Vec<DeviceEndpoint> {
    let mut devices = Vec::new();

    let host = effective_host(config);
    let port = if config.localhost_api_port == 0 {
        DEFAULT_LOCALHOST_API_PORT
    } else {
        config.localhost_api_port
    };
    let token = read_api_token(data_dir).unwrap_or_default();
    if !token.is_empty() {
        devices.push(DeviceEndpoint {
            name: "本机".to_string(),
            url: format!("http://{}:{}", host, port),
            token,
            is_local: true,
        });
    }

    for d in &config.node_devices {
        devices.push(DeviceEndpoint {
            name: d.name.clone(),
            url: d.url.trim_end_matches('/').to_string(),
            token: d.token.clone(),
            is_local: false,
        });
    }

    devices
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

    if bot_token.is_none() || bot_token.as_ref().map_or(true, |t| t.trim().is_empty()) {
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
                format!("连接失败: {e}")
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
                            if should_abort_polling(status, body.error_code, consecutive_errors) {
                                set_error(shared, msg.clone());
                                log::error!(
                                    "Telegram Bot 连续 {} 次轮询异常，停止轮询: {}",
                                    consecutive_errors,
                                    msg
                                );
                                return;
                            }
                            log::warn!(
                                "Telegram Bot 轮询异常(第{}次): {}，{}秒后重试",
                                consecutive_errors,
                                msg,
                                TELEGRAM_POLL_RETRY_SECONDS
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(
                                TELEGRAM_POLL_RETRY_SECONDS,
                            ))
                            .await;
                            continue;
                        }

                        consecutive_errors = 0;
                        if let Some(updates) = body.result {
                            for u in updates {
                                offset = u.update_id + 1;
                                if let Some(msg) = u.message {
                                    let text = msg.text.as_deref();
                                    let cmd = text
                                        .map(first_command)
                                        .map(normalize_command)
                                        .unwrap_or_default();
                                    let authorized =
                                        is_authorized_chat(&allowed_chat_ids, msg.chat.id);

                                    // 未授权会话只允许 /start 查看说明，或通过 /bind 一次性绑定码授权。
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
                                        log::info!("TG Bot 收到消息: {text}");
                                        if let Some(progress) = progress_text_for_command(cmd) {
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
                                        handle_cmd(&client, &devices, text, msg.chat.id).await
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
                        if consecutive_errors >= TELEGRAM_POLL_MAX_ERRORS {
                            let msg = format!("轮询响应解析失败: {e}");
                            set_error(shared, msg.clone());
                            log::error!("Telegram Bot {msg}");
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(
                            TELEGRAM_POLL_RETRY_SECONDS,
                        ))
                        .await;
                    }
                }
            }
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors >= TELEGRAM_POLL_MAX_ERRORS {
                    let msg = if e.is_connect() || e.is_timeout() {
                        "无法连接 Telegram API（可能需要代理/VPN）".to_string()
                    } else {
                        format!("轮询失败: {e}")
                    };
                    set_error(shared, msg);
                    log::error!("Telegram Bot 连续 {consecutive_errors} 次失败，停止轮询");
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(TELEGRAM_POLL_RETRY_SECONDS))
                    .await;
            }
        }
    }
}

async fn consume_pending_updates(client: &Client, bot_token: &str) -> Result<i64, String> {
    // 启动时丢弃历史积压更新，避免重启后重复回复旧消息。
    let url =
        format!("https://api.telegram.org/bot{bot_token}/getUpdates?offset=-1&limit=1&timeout=0");
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

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

    let next_offset = body
        .result
        .as_ref()
        .and_then(|updates| updates.last())
        .map(|u| u.update_id + 1)
        .unwrap_or(0);
    Ok(next_offset)
}

fn find_device<'a>(devices: &'a [DeviceEndpoint], name: &str) -> Option<&'a DeviceEndpoint> {
    if name.is_empty() || name == "本机" || name == "local" {
        return devices.iter().find(|d| d.is_local);
    }
    devices.iter().find(|d| d.name == name)
}

fn no_available_device_reply() -> String {
    "❌ 无可用设备\n请先启用本地 API 并生成 Token。".to_string()
}

fn connection_failed_reply(device_name: &str) -> String {
    format!("❌ 连接失败\n设备：{device_name}\n请检查地址、Token 与网络连通性。")
}

fn progress_text_for_command(cmd: &str) -> Option<&'static str> {
    match cmd {
        "/devices" => Some("⏳ 正在获取设备列表，请稍候..."),
        "/device" => Some("⏳ 正在获取设备状态，请稍候..."),
        "/reports" => Some("⏳ 正在获取日报列表，请稍候..."),
        "/report" => Some("⏳ 正在获取日报详情，请稍候..."),
        "/generate" => Some("⏳ 正在生成日报，预计需要 30-120 秒..."),
        _ => None,
    }
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

fn first_command(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or("")
}

fn is_authorized_chat(allowed_chat_ids: &[i64], chat_id: i64) -> bool {
    allowed_chat_ids.contains(&chat_id)
}

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

fn clear_bind_code(config: &mut AppConfig) {
    config.telegram_bot_bind_code = None;
    config.telegram_bot_bind_code_expires_at = None;
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
        BindCodeStatus::Ready(expected_code) if expected_code != input_code => {
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

fn help_reply(chat_id: i64) -> String {
    format!("{HELP}\n\n{OUTPUT_DIVIDER}\n当前 Chat ID：{chat_id}")
}

async fn handle_cmd(
    client: &Client,
    devices: &[DeviceEndpoint],
    text: &str,
    chat_id: i64,
) -> Option<String> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let cmd = normalize_command(parts.first().copied().unwrap_or(""));
    if cmd.is_empty() {
        return Some(UNKNOWN_CMD_REPLY.to_string());
    }

    match cmd {
        "/start" | "/help" => Some(help_reply(chat_id)),
        "/devices" => {
            if devices.is_empty() {
                return Some(no_available_device_reply());
            }
            let mut lines = vec!["🧭 设备列表".to_string(), OUTPUT_DIVIDER.to_string()];
            for (idx, d) in devices.iter().enumerate() {
                let tag = if d.is_local { " (本机)" } else { "" };
                let health = api_get(client, &format!("{}/health", d.url)).await;
                let status = match health {
                    Some(h) if h.get("status").and_then(|v| v.as_str()) == Some("ok") => "✅",
                    Some(_) => "⚠️",
                    None => "❌",
                };
                lines.push(format!("{}. {status} {}{}", idx + 1, d.name, tag));
            }
            Some(lines.join("\n"))
        }
        "/reports" => {
            let device = find_device(devices, parts.get(1).copied().unwrap_or(""))
                .or_else(|| devices.first());
            let device = match device {
                Some(d) => d,
                None => return Some(no_available_device_reply()),
            };
            let url = format!("{}/v1/reports?token={}&limit=10", device.url, device.token);
            match api_get(client, &url).await {
                Some(json) => {
                    let dates = json.get("dates")?.as_array()?;
                    let mut lines = vec![
                        "📚 最近日报".to_string(),
                        OUTPUT_DIVIDER.to_string(),
                        format!("设备：{}", device.name),
                    ];
                    if dates.is_empty() {
                        lines.push("暂无日报记录".to_string());
                        return Some(lines.join("\n"));
                    }
                    let items: Vec<String> = dates
                        .iter()
                        .enumerate()
                        .map(|(i, d)| format!("{}. {}", i + 1, d.as_str().unwrap_or("-")))
                        .collect();
                    lines.extend(items);
                    Some(lines.join("\n"))
                }
                None => Some(connection_failed_reply(&device.name)),
            }
        }
        "/report" => {
            let date = crate::commands::resolve_single_date(parts.get(1).copied());
            let device = find_device(devices, parts.get(2).copied().unwrap_or(""))
                .or_else(|| devices.first());
            let device = match device {
                Some(d) => d,
                None => return Some(no_available_device_reply()),
            };
            let url = format!("{}/v1/reports/{date}?token={}", device.url, device.token);
            match api_get(client, &url).await {
                Some(data) => {
                    if let Some(err) = data.get("error") {
                        return Some(format!(
                            "❌ 查询失败\n设备：{}\n日期：{}\n原因：{}",
                            device.name,
                            date,
                            err.as_str().unwrap_or("未知错误")
                        ));
                    }
                    match data.get("content").and_then(|v| v.as_str()) {
                        Some(content) => {
                            let content = normalize_report_for_chat(content);
                            Some(format!(
                                "📄 日报详情\n{OUTPUT_DIVIDER}\n设备：{}\n日期：{}\n\n{}",
                                device.name,
                                date,
                                truncate(&content, 3900)
                            ))
                        }
                        None => Some(format!(
                            "❌ 设备返回数据格式异常\n设备：{}\n日期：{}",
                            device.name, date
                        )),
                    }
                }
                None => Some(connection_failed_reply(&device.name)),
            }
        }
        "/generate" => {
            let date = crate::commands::resolve_single_date(parts.get(1).copied());
            let device = find_device(devices, parts.get(2).copied().unwrap_or(""))
                .or_else(|| devices.first());
            let device = match device {
                Some(d) => d,
                None => return Some(no_available_device_reply()),
            };
            let url = format!("{}/v1/reports/generate?token={}", device.url, device.token);
            match client
                .post(&url)
                .json(&serde_json::json!({"date": date}))
                .timeout(std::time::Duration::from_secs(120))
                .send()
                .await
            {
                Ok(r) => {
                    let data: serde_json::Value = match r.json().await {
                        Ok(d) => d,
                        Err(e) => {
                            return Some(format!(
                                "❌ 解析设备响应失败\n设备：{}\n日期：{}\n原因：{e}",
                                device.name, date
                            ))
                        }
                    };
                    if let Some(err) = data.get("error") {
                        return Some(format!(
                            "❌ 生成失败\n设备：{}\n日期：{}\n原因：{}",
                            device.name,
                            date,
                            err.as_str().unwrap_or("未知错误")
                        ));
                    }
                    match data.get("content").and_then(|v| v.as_str()) {
                        Some(content) => {
                            let content = normalize_report_for_chat(content);
                            Some(format!(
                                "✅ 生成完成\n{OUTPUT_DIVIDER}\n设备：{}\n日期：{}\n\n{}",
                                device.name,
                                date,
                                truncate(&content, 3800)
                            ))
                        }
                        None => Some(format!(
                            "❌ 设备返回数据格式异常\n设备：{}\n日期：{}",
                            device.name, date
                        )),
                    }
                }
                Err(e) => Some(format!(
                    "❌ 生成失败\n设备：{}\n日期：{}\n原因：{}",
                    device.name, date, e
                )),
            }
        }
        "/device" => {
            let device = find_device(devices, parts.get(1).copied().unwrap_or(""))
                .or_else(|| devices.first());
            let device = match device {
                Some(d) => d,
                None => return Some(no_available_device_reply()),
            };
            let url = format!("{}/v1/device?token={}", device.url, device.token);
            match api_get(client, &url).await {
                Some(data) => {
                    Some(format!(
                    "🖥 设备状态\n{OUTPUT_DIVIDER}\n设备：{}\nID：{}\n名称：{}\n平台：{}\n录制：{}",
                    device.name,
                    data.get("deviceId").and_then(|v| v.as_str()).unwrap_or("-"),
                    data.get("deviceName").and_then(|v| v.as_str()).unwrap_or("-"),
                    data.get("platform").and_then(|v| v.as_str()).unwrap_or("-"),
                    if data.get("recording").and_then(|v| v.as_bool()).unwrap_or(false) {
                        "是"
                    } else {
                        "否"
                    },
                ))
                }
                None => Some(connection_failed_reply(&device.name)),
            }
        }
        _ => Some(UNKNOWN_CMD_REPLY.to_string()),
    }
}

async fn api_get(client: &Client, url: &str) -> Option<serde_json::Value> {
    client
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()
}

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
        Err(e) => log::warn!("Telegram sendMessage 错误: {e}"),
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
        Err(e) => log::warn!("Telegram sendChatAction 错误: {e}"),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }

    // 按字符截断，避免中文/emoji 在 UTF-8 边界被截断导致 panic。
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_none() {
        return head;
    }

    let mut trimmed: String = head.chars().take(max.saturating_sub(1)).collect();
    trimmed.push('…');
    trimmed
}

fn parse_table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .filter(|cell| !cell.is_empty())
        .collect()
}

fn is_table_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells
            .iter()
            .all(|cell| cell.chars().all(|ch| ch == '-' || ch == ':'))
}

fn normalize_report_for_chat(content: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_table = false;
    let mut table_headers: Vec<String> = Vec::new();
    let mut last_non_empty = String::new();

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            if in_table {
                in_table = false;
                table_headers.clear();
            }
            if lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(String::new());
            }
            continue;
        }

        if line.starts_with('|') && line.ends_with('|') {
            let cells = parse_table_cells(line);
            if cells.is_empty() {
                continue;
            }
            if is_table_separator(&cells) {
                continue;
            }
            if !in_table {
                in_table = true;
                table_headers = cells;
                continue;
            }
            let row =
                if table_headers.first().is_some_and(|h| h.contains("序号")) && cells.len() >= 3 {
                    format!("- {}. {}（{}）", cells[0], cells[1], cells[2])
                } else if cells.len() >= 2 {
                    format!("- {}：{}", cells[0], cells[1..].join(" / "))
                } else {
                    format!("- {}", cells.join(" / "))
                };
            if row != last_non_empty {
                last_non_empty = row.clone();
                lines.push(row);
            }
            continue;
        }

        if in_table {
            in_table = false;
            table_headers.clear();
        }

        let mut converted = line
            .trim_start_matches('#')
            .trim()
            .replace("**", "")
            .replace("*   ", "- ")
            .replace("* ", "- ");
        if converted.starts_with("- - ") {
            converted = converted.replacen("- - ", "- ", 1);
        }
        if converted != last_non_empty {
            last_non_empty = converted.clone();
            lines.push(converted);
        }
    }

    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

fn normalize_command(raw: &str) -> &str {
    raw.split('@').next().unwrap_or(raw)
}

fn should_abort_polling(
    status: StatusCode,
    error_code: Option<i64>,
    consecutive_errors: u32,
) -> bool {
    if matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::CONFLICT
    ) {
        return true;
    }
    if matches!(error_code, Some(401) | Some(403) | Some(409)) {
        return true;
    }
    consecutive_errors >= TELEGRAM_POLL_MAX_ERRORS
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

fn set_running(shared: &Arc<std::sync::Mutex<SharedBotStatus>>, running: bool) {
    if let Ok(mut s) = shared.lock() {
        s.running = running;
        s.starting = false;
        s.last_error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_bind_code_to_config, binding_reply, configured_bind_code_status, first_command,
        format_telegram_http_error, help_reply, is_authorized_chat, normalize_bind_code,
        normalize_command, normalize_report_for_chat, progress_text_for_command,
        should_abort_polling, truncate, unauthorized_reply, BindCodeApplyResult, BindCodeStatus,
        TgResp, TgUpdate, NON_TEXT_REPLY,
    };
    use crate::config::AppConfig;
    use reqwest::StatusCode;

    #[test]
    fn 命令应支持带机器人用户名后缀() {
        assert_eq!(normalize_command("/start@WorkReviewBot"), "/start");
        assert_eq!(normalize_command("/reports@work_review_bot"), "/reports");
    }

    #[test]
    fn 轮询冲突应立即中止并提示() {
        assert!(should_abort_polling(StatusCode::CONFLICT, Some(409), 1));
        let message = format_telegram_http_error(
            "轮询失败",
            StatusCode::CONFLICT,
            Some("Conflict: terminated by other getUpdates request"),
        );
        assert!(message.contains("HTTP 409"));
        assert!(message.contains("webhook"));
    }

    #[test]
    fn 非文本消息应返回帮助提示() {
        assert!(NON_TEXT_REPLY.contains("/help"));
    }

    #[test]
    fn 空白名单不应授权任何会话() {
        assert!(!is_authorized_chat(&[], 123456789));
        assert!(is_authorized_chat(&[123456789], 123456789));
        assert!(!is_authorized_chat(&[987654321], 123456789));
    }

    #[test]
    fn 绑定与未授权提示应包含当前_chat_id() {
        let binding = binding_reply(123456789);
        let unauthorized = unauthorized_reply(123456789);
        let help = help_reply(123456789);

        assert!(binding.contains("当前 Chat ID：123456789"));
        assert!(binding.contains("/bind"));
        assert!(unauthorized.contains("该会话未被授权"));
        assert!(unauthorized.contains("当前 Chat ID：123456789"));
        assert!(unauthorized.contains("/bind"));
        assert!(help.contains("/report"));
        assert!(help.contains("当前 Chat ID：123456789"));
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
    fn 首个命令解析应忽略后续参数() {
        assert_eq!(first_command("/start hello"), "/start");
        assert_eq!(
            normalize_command(first_command("/start@WorkReviewBot hello")),
            "/start"
        );
    }

    #[test]
    fn 中文内容截断不应触发utf8边界panic() {
        let content = "# 工作日报\n\n整体进展顺利";
        let got = truncate(content, 8);
        assert_eq!(got.chars().count(), 8);
        assert!(got.ends_with('…'));
    }

    #[test]
    fn 报告格式应在聊天中转为条目文本() {
        let source = "## 一、今日概览\n| 指标 | 数值 |\n|:--|--:|\n| 总工作时长 | 3小时 |\n";
        let rendered = normalize_report_for_chat(source);
        assert!(rendered.contains("一、今日概览"));
        assert!(rendered.contains("总工作时长：3小时"));
        assert!(!rendered.contains("| 指标 |"));
    }

    #[test]
    fn 查询命令应有处理中提示() {
        assert!(progress_text_for_command("/reports").is_some());
        assert!(progress_text_for_command("/generate").is_some());
        assert!(progress_text_for_command("/help").is_none());
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
