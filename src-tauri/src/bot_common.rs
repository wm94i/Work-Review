use crate::localhost_api::LOCALHOST_API_HOST;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use work_review_core::config::{AppConfig, DEFAULT_LOCALHOST_API_PORT};

/// 四个 Bot 回调共用的签名防重放时间窗口（±5 分钟）。
pub const BOT_SIGN_WINDOW_SECS: i64 = 5 * 60;
pub const BOT_SIGN_WINDOW_MS: i64 = BOT_SIGN_WINDOW_SECS * 1000;

/// 四个 Bot 共享的 HTTP 客户端（复用连接池与 TLS 会话，避免每次回调重建 Client）。
static SHARED_BOT_HTTP_CLIENT: once_cell::sync::Lazy<Client> = once_cell::sync::Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(35))
        .build()
        .expect("初始化共享 Bot HTTP 客户端失败")
});

/// 返回四个 Bot 共用的连接池 HTTP 客户端（`reqwest::Client` 内部使用 `Arc`，克隆为 O(1)）。
pub fn shared_bot_http_client() -> Client {
    SHARED_BOT_HTTP_CLIENT.clone()
}

fn timestamp_within_window_at(now: i64, timestamp_str: &str, window: i64) -> bool {
    let ts: i64 = match timestamp_str.trim().parse() {
        Ok(t) if t >= 0 => t,
        _ => return false,
    };
    now.abs_diff(ts) <= window.max(0) as u64
}

/// 校验秒级 Unix 时间戳字符串是否在 `now ± window_secs` 窗口内（防重放与 i64 减法溢出）。
pub fn timestamp_within_window_secs(timestamp_secs_str: &str, window_secs: i64) -> bool {
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return false,
    };
    timestamp_within_window_at(now, timestamp_secs_str, window_secs)
}

/// 校验毫秒级 Unix 时间戳字符串是否在 `now ± window_ms` 窗口内（防重放与 i64 减法溢出）。
pub fn timestamp_within_window_ms(timestamp_ms_str: &str, window_ms: i64) -> bool {
    let now_ms = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(_) => return false,
    };
    timestamp_within_window_at(now_ms, timestamp_ms_str, window_ms)
}

pub const HELP: &str = "\
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

pub const UNKNOWN_CMD_REPLY: &str = "未知命令。发送 /help 查看帮助，例如：/generate today";
pub const EMPTY_TEXT_REPLY: &str = "消息为空";
pub const NON_TEXT_REPLY: &str = "暂不支持非文本消息，发送 /help 查看帮助";
pub const OUTPUT_DIVIDER: &str = "────────────";
pub const BOT_NL_THINKING_REPLY: &str = "⏳ 正在整理你的工作记录，请稍候...";
pub const BOT_NL_BUSY_REPLY: &str = "⏳ 当前问答请求较多，请稍候几秒后再试。";
pub const BOT_NL_RATE_LIMIT_REPLY: &str =
    "⏳ 提问过于频繁，请稍后再试（每分钟最多 6 次自然语言问答）。";
pub const BOT_NL_SCOPE_FOOTER: &str = "\n\nℹ️ 以上回答仅基于本机已脱敏工作记录（只读模式）";
const BOT_NL_SYSTEM_PROMPT: &str = "你是 Work Review 聊天机器人内的只读工作复盘助手。仅基于已脱敏的本机工作记录回答，绝不声称已执行任何修改操作（如新建待办、修改分类、暂停录制或生成日报）；如用户希望执行写操作，请提示其使用对应快捷命令或在 Work Review 桌面客户端操作。回答请简洁清晰，适合聊天窗口阅读。";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEndpoint {
    pub name: String,
    pub url: String,
    pub token: String,
    pub is_local: bool,
}

/// 解析本机自调用地址：即使本地 API 在局域网模式下监听 `0.0.0.0` / `::` 或局域网地址，
/// 本机 Bot 进程自调用应固定走回环 `127.0.0.1`（或明确绑定的非通配单网卡 IP）。
pub fn effective_host(config: &AppConfig) -> String {
    let raw = config
        .localhost_api_host
        .as_deref()
        .map(str::trim)
        .unwrap_or("");
    if raw.is_empty()
        || raw == "0.0.0.0"
        || raw == "::"
        || raw == "[::]"
        || raw.eq_ignore_ascii_case("localhost")
    {
        LOCALHOST_API_HOST.to_string()
    } else {
        raw.to_string()
    }
}

pub fn read_api_token(data_dir: &Path) -> Option<String> {
    let path = data_dir.join("localhost_api_token.txt");
    std::fs::read_to_string(&path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

pub fn build_device_list(config: &AppConfig, data_dir: &Path) -> Vec<DeviceEndpoint> {
    let mut devices = Vec::new();
    let host = effective_host(config);
    let port = if config.localhost_api_port == 0 {
        DEFAULT_LOCALHOST_API_PORT
    } else {
        config.localhost_api_port
    };
    if let Some(token) = read_api_token(data_dir) {
        devices.push(DeviceEndpoint {
            name: "本机".to_string(),
            url: format!("http://{host}:{port}"),
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

pub fn find_device<'a>(devices: &'a [DeviceEndpoint], name: &str) -> Option<&'a DeviceEndpoint> {
    if name.is_empty() || name == "本机" || name == "local" {
        return devices.iter().find(|d| d.is_local);
    }
    devices.iter().find(|d| d.name == name)
}

pub fn no_available_device_reply() -> String {
    "❌ 无可用设备\n请先启用本地 API 并生成 Token。".to_string()
}

pub fn device_not_found_reply(name: &str) -> String {
    format!("❌ 未找到设备 {name}\n请使用 /devices 查看设备列表。")
}

/// 按名称选择设备：显式指定了设备名但未匹配时返回错误提示（Err 为回复文案），
/// 仅在未指定设备名时回退到第一台设备。
fn select_device<'a>(
    devices: &'a [DeviceEndpoint],
    name: &str,
) -> Result<&'a DeviceEndpoint, String> {
    if let Some(device) = find_device(devices, name) {
        return Ok(device);
    }
    if name.is_empty() || name == "本机" || name == "local" {
        return devices.first().ok_or_else(no_available_device_reply);
    }
    Err(device_not_found_reply(name))
}

pub fn connection_failed_reply(device_name: &str) -> String {
    format!("❌ 连接失败\n设备：{device_name}\n请检查地址、Token 与网络连通性。")
}

pub fn invalid_date_reply() -> String {
    format!(
        "❌ 日期格式不正确\n{OUTPUT_DIVIDER}\n支持：YYYY-MM-DD / today / yesterday，例如 /report 2026-04-25"
    )
}

pub fn report_generate_timeout(config: &AppConfig) -> Duration {
    Duration::from_secs(config.report_generation_timeout_secs.max(60))
}

pub fn progress_text_for_command(cmd: &str) -> Option<&'static str> {
    match cmd {
        "devices" | "设备列表" => Some("⏳ 正在获取设备列表，请稍候..."),
        "device" | "设备" => Some("⏳ 正在获取设备状态，请稍候..."),
        "reports" | "日报列表" => Some("⏳ 正在获取日报列表，请稍候..."),
        "report" | "日报" => Some("⏳ 正在获取日报详情，请稍候..."),
        "generate" | "生成日报" => Some("⏳ 正在生成日报，请稍候..."),
        _ => None,
    }
}

pub fn normalize_command(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('/')
        .split('@')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

pub fn status_payload(status: &str, reason: &str, detail: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "status": status,
        "reason": reason,
        "detail": detail.unwrap_or(""),
    })
}

/// GET 设备 API。token 走 `Authorization: Bearer` 头而非 `?token=` 查询串——
/// 查询串会进代理与访问日志;服务端两种都支持,改为与 `/reports/generate` POST 一致。
pub async fn api_get(client: &Client, url: &str, token: Option<&str>) -> Option<serde_json::Value> {
    let mut request = client.get(url).timeout(Duration::from_secs(10));
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request
        .send()
        .await
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()
}

pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_none() {
        return head;
    }
    let mut trimmed: String = head.chars().take(max.saturating_sub(1)).collect();
    trimmed.push('…');
    trimmed
}

pub fn parse_table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .filter(|cell| !cell.is_empty())
        .collect()
}

pub fn is_table_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells
            .iter()
            .all(|cell| cell.chars().all(|ch| ch == '-' || ch == ':'))
}

pub fn normalize_report_for_chat(content: &str) -> String {
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

fn build_generate_report_request(
    client: &Client,
    device: &DeviceEndpoint,
    date: &str,
    generate_timeout: Duration,
) -> reqwest::RequestBuilder {
    let url = format!("{}/v1/reports/generate", device.url.trim_end_matches('/'));
    client
        .post(url)
        .bearer_auth(&device.token)
        .json(&serde_json::json!({ "date": date }))
        .timeout(generate_timeout)
}

pub async fn handle_cmd(
    client: &Client,
    devices: &[DeviceEndpoint],
    text: &str,
    generate_timeout: Duration,
) -> Option<String> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let cmd = normalize_command(parts.first().copied().unwrap_or(""));
    if cmd.is_empty() {
        return Some(UNKNOWN_CMD_REPLY.to_string());
    }

    match cmd.as_str() {
        "help" | "帮助" => Some(HELP.to_string()),
        "devices" | "设备列表" => {
            if devices.is_empty() {
                return Some(no_available_device_reply());
            }
            let mut lines = vec!["🧭 设备列表".to_string(), OUTPUT_DIVIDER.to_string()];
            for (idx, d) in devices.iter().enumerate() {
                let tag = if d.is_local { " (本机)" } else { "" };
                let health = api_get(client, &format!("{}/health", d.url), None).await;
                let status = match health {
                    Some(h) if h.get("status").and_then(|v| v.as_str()) == Some("ok") => "✅",
                    Some(_) => "⚠️",
                    None => "❌",
                };
                lines.push(format!("{}. {status} {}{}", idx + 1, d.name, tag));
            }
            Some(lines.join("\n"))
        }
        "device" | "设备" => {
            let device = match select_device(devices, parts.get(1).copied().unwrap_or("")) {
                Ok(d) => d,
                Err(reply) => return Some(reply),
            };
            let url = format!("{}/v1/device", device.url);
            match api_get(client, &url, Some(device.token.as_str())).await {
                Some(data) => Some(format!(
                    "🖥 设备状态\n{OUTPUT_DIVIDER}\n设备：{}\nID：{}\n名称：{}\n平台：{}\n录制：{}",
                    device.name,
                    data.get("deviceId").and_then(|v| v.as_str()).unwrap_or("-"),
                    data.get("deviceName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-"),
                    data.get("platform").and_then(|v| v.as_str()).unwrap_or("-"),
                    if data
                        .get("recording")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        "是"
                    } else {
                        "否"
                    },
                )),
                None => Some(connection_failed_reply(&device.name)),
            }
        }
        "reports" | "日报列表" => {
            let device = match select_device(devices, parts.get(1).copied().unwrap_or("")) {
                Ok(d) => d,
                Err(reply) => return Some(reply),
            };
            let url = format!("{}/v1/reports?limit=10", device.url);
            match api_get(client, &url, Some(device.token.as_str())).await {
                Some(json) => {
                    let dates = match json.get("dates").and_then(|v| v.as_array()) {
                        Some(d) => d,
                        None => return None,
                    };
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
        "report" | "日报" => {
            let date = crate::commands::resolve_single_date(parts.get(1).copied());
            if !crate::commands::is_iso_date(&date) {
                return Some(invalid_date_reply());
            }
            let device = match select_device(devices, parts.get(2).copied().unwrap_or("")) {
                Ok(d) => d,
                Err(reply) => return Some(reply),
            };
            let url = format!("{}/v1/reports/{}", device.url, date);
            match api_get(client, &url, Some(device.token.as_str())).await {
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
        "generate" | "生成日报" => {
            let date = crate::commands::resolve_single_date(parts.get(1).copied());
            if !crate::commands::is_iso_date(&date) {
                return Some(invalid_date_reply());
            }
            let device = match select_device(devices, parts.get(2).copied().unwrap_or("")) {
                Ok(d) => d,
                Err(reply) => return Some(reply),
            };
            // 日报生成可长达数分钟，而企微 HTTP 回调 5 秒即超时；
            // 因此改为后台任务生成 + 立即回执，结果通过 /report 查询。
            let request = build_generate_report_request(client, device, &date, generate_timeout);
            let device_name = device.name.clone();
            let task_date = date.clone();
            tokio::spawn(async move {
                match request.send().await {
                    Ok(response) => match response.json::<serde_json::Value>().await {
                        Ok(data) => {
                            if let Some(err) = data.get("error") {
                                log::warn!(
                                    "机器人后台生成日报失败: 设备={device_name} 日期={task_date} 原因={}",
                                    err.as_str().unwrap_or("未知错误")
                                );
                            } else {
                                log::info!(
                                    "机器人后台生成日报完成: 设备={device_name} 日期={task_date}"
                                );
                            }
                        }
                        Err(e) => log::warn!(
                            "机器人后台生成日报响应解析失败: 设备={device_name} 日期={task_date} 原因={e}"
                        ),
                    },
                    Err(e) => log::warn!(
                        "机器人后台生成日报请求失败: 设备={device_name} 日期={task_date} 原因={e}"
                    ),
                }
            });
            Some(format!(
                "⏳ 已开始生成 {date} 日报（{}），完成后可用 /report {date} 查看",
                device.name
            ))
        }
        _ => Some(UNKNOWN_CMD_REPLY.to_string()),
    }
}

// ── 发送者授权（飞书 / 钉钉 / 企业微信）─────────────────────────────

/// 发送者绑定所用的平台标识（`AppConfig.bot_sender_bindings` 的 key）。
pub const BOT_PLATFORM_FEISHU: &str = "feishu";
pub const BOT_PLATFORM_DINGTALK: &str = "dingtalk";
/// 企业微信回调模式与智能机器人长连接共用同一份绑定名单。
pub const BOT_PLATFORM_WECOM: &str = "wecom";
pub const BOT_SENDER_PLATFORMS: &[&str] = &[
    BOT_PLATFORM_FEISHU,
    BOT_PLATFORM_DINGTALK,
    BOT_PLATFORM_WECOM,
];

/// 绑定码有效期（秒）。
pub const BOT_BIND_CODE_TTL_SECONDS: i64 = 10 * 60;
/// 单个绑定码允许的错误尝试次数，达到后作废需重新生成。
pub const BOT_BIND_MAX_FAILED_ATTEMPTS: u32 = 5;

/// 发送者校验结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotSenderDecision {
    /// 已授权，继续处理命令。
    Allowed,
    /// 本条消息完成了绑定。
    Bound,
    /// 未授权，提示先绑定。
    NeedBind,
    /// 绑定码错误（剩余次数）。
    InvalidCode(u32),
    /// 绑定码已过期或因错误次数过多作废。
    CodeUnavailable,
    /// 本机尚未生成绑定码。
    NotConfigured,
}

impl BotSenderDecision {
    /// 非 `Allowed` 时需要回复给发送者的文本。
    pub fn reply(&self) -> Option<String> {
        let text = match self {
            BotSenderDecision::Allowed => return None,
            BotSenderDecision::Bound => {
                format!("✅ 绑定成功\n{OUTPUT_DIVIDER}\n现在可以发送 /help 查看可用命令。")
            }
            BotSenderDecision::NeedBind => format!(
                "🔒 尚未授权\n{OUTPUT_DIVIDER}\n请在 Work Review「设置 → 接入管理」中生成绑定码，然后发送：/bind 绑定码"
            ),
            BotSenderDecision::InvalidCode(remaining) => {
                format!("❌ 绑定码不正确\n{OUTPUT_DIVIDER}\n还可尝试 {remaining} 次。")
            }
            BotSenderDecision::CodeUnavailable => format!(
                "⌛ 绑定码已失效\n{OUTPUT_DIVIDER}\n请在 Work Review 设置页重新生成绑定码。"
            ),
            BotSenderDecision::NotConfigured => format!(
                "🔒 尚未授权\n{OUTPUT_DIVIDER}\n本机还没有生成绑定码，请先在 Work Review「设置 → 接入管理」中生成。"
            ),
        };
        Some(text)
    }
}

/// 入站消息闸门结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotMessageGate {
    /// 平台重推的重复消息，直接确认、不再处理。
    Duplicate,
    /// 未授权 / 绑定流程：把该文本回复给发送者，不执行命令。
    Reply(String),
    /// 已授权，继续执行命令。
    Proceed,
}

/// 判断发送者是否已在指定平台完成授权绑定（纯只读查询，用于 EnterChat 等非命令事件）。
pub fn is_sender_authorized(
    state: &std::sync::Arc<std::sync::Mutex<crate::AppState>>,
    platform: &str,
    sender_id: &str,
) -> bool {
    let sender_id = sender_id.trim();
    if sender_id.is_empty() {
        return false;
    }
    state
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .config
                .bot_sender_bindings
                .get(platform)
                .map(|binding| binding.allowed_senders.iter().any(|id| id == sender_id))
        })
        .unwrap_or(false)
}

/// 所有 Bot 共用的入站闸门：先区分已授权与未授权去重桶（防止未绑定成员灌水挤掉已绑定用户的去重缓存），
/// 再按消息 ID 去重并校验发送者授权。
pub fn gate_bot_message(
    state: &std::sync::Arc<std::sync::Mutex<crate::AppState>>,
    platform: &str,
    message_id: &str,
    sender_id: &str,
    text: &str,
) -> BotMessageGate {
    let authorized = is_sender_authorized(state, platform, sender_id);
    if is_duplicate_bot_message_in_bucket(platform, message_id, authorized) {
        return BotMessageGate::Duplicate;
    }
    match check_bot_sender(state, platform, sender_id, text).reply() {
        Some(reply) => BotMessageGate::Reply(reply),
        None => BotMessageGate::Proceed,
    }
}

/// 四个 Bot 平台归一化后的入站消息（M4-3 统一管线输入）。
#[derive(Debug, Clone)]
pub struct InboundBotMessage<'a> {
    pub(crate) platform: &'a str,
    pub(crate) message_id: &'a str,
    pub(crate) sender_id: &'a str,
    pub(crate) text: &'a str,
    pub(crate) is_text: bool,
    /// 私有授权状态标记：只能通过显式构造器设置，防止调用方误填 true 绕过通用绑定校验。
    pre_authorized: bool,
}

impl<'a> InboundBotMessage<'a> {
    /// 构造需要走通用 `gate_bot_message` 绑定授权闸门的入站消息（飞书 / 钉钉 / 企业微信）。
    pub fn standard(
        platform: &'a str,
        message_id: &'a str,
        sender_id: &'a str,
        text: &'a str,
        is_text: bool,
    ) -> Self {
        Self {
            platform,
            message_id,
            sender_id,
            text,
            is_text,
            pre_authorized: false,
        }
    }

    /// 构造已通过平台专属白名单校验的入站消息（仅允许在 Telegram 等完成 chat_id 白名单校验后调用）。
    pub(crate) fn pre_authorized(
        platform: &'a str,
        message_id: &'a str,
        sender_id: &'a str,
        text: &'a str,
        is_text: bool,
    ) -> Self {
        Self {
            platform,
            message_id,
            sender_id,
            text,
            is_text,
            pre_authorized: true,
        }
    }
}

/// 四个 Bot 统一的入站路由决策结果：`InboundMessage → 授权 → 去重 → 命令/问答 → 回复`。
pub enum BotPipelineAction {
    /// 平台重推的重复消息，直接确认忽略。
    Duplicate,
    /// 立即回复的固定文案（未授权/绑定提示、非文本提示、空文本提示、并发已满、触发限流）。
    ImmediateReply { reason: &'static str, text: String },
    /// 已通过并发信号量与发送者配额校验的只读自然语言问答任务（附带已获取的并发许可）。
    NaturalLanguage {
        permit: tokio::sync::OwnedSemaphorePermit,
        question: String,
    },
    /// 常规 Bot 命令（附带可选进度提示文本）。
    Command {
        text: String,
        progress_hint: Option<&'static str>,
    },
}

/// 统一处理入站 Bot 消息：`授权/去重 → 非文本与空文本兜底 → NL 并发/配额闸门 → 命令分发`。
/// 平台侧只需负责协议验签、消息解包与按 `BotPipelineAction` 发送回复。
pub fn route_inbound_bot_message(
    state: &std::sync::Arc<std::sync::Mutex<crate::AppState>>,
    inbound: &InboundBotMessage<'_>,
    nl_semaphore: &std::sync::Arc<tokio::sync::Semaphore>,
    nl_enabled: bool,
    empty_text_reply: Option<&str>,
) -> BotPipelineAction {
    if inbound.pre_authorized {
        if inbound.sender_id.trim().is_empty() {
            return BotPipelineAction::ImmediateReply {
                reason: "sender_authorization",
                text: BotSenderDecision::NeedBind.reply().unwrap_or_default(),
            };
        }
        if is_duplicate_bot_message_in_bucket(inbound.platform, inbound.message_id, true) {
            return BotPipelineAction::Duplicate;
        }
    } else {
        match gate_bot_message(
            state,
            inbound.platform,
            inbound.message_id,
            inbound.sender_id,
            inbound.text,
        ) {
            BotMessageGate::Duplicate => return BotPipelineAction::Duplicate,
            BotMessageGate::Reply(reply) => {
                return BotPipelineAction::ImmediateReply {
                    reason: "sender_authorization",
                    text: reply,
                };
            }
            BotMessageGate::Proceed => {}
        }
    }

    if !inbound.is_text {
        return BotPipelineAction::ImmediateReply {
            reason: "non_text",
            text: NON_TEXT_REPLY.to_string(),
        };
    }

    let trimmed = inbound.text.trim();
    if trimmed.is_empty() {
        return BotPipelineAction::ImmediateReply {
            reason: "empty_text",
            text: empty_text_reply.unwrap_or("").to_string(),
        };
    }

    if nl_enabled && !is_known_or_slash_command(trimmed) {
        let Ok(permit) = nl_semaphore.clone().try_acquire_owned() else {
            return BotPipelineAction::ImmediateReply {
                reason: "nl_busy",
                text: BOT_NL_BUSY_REPLY.to_string(),
            };
        };
        if let Err(limit_msg) = try_consume_bot_nl_quota(inbound.platform, inbound.sender_id) {
            drop(permit);
            return BotPipelineAction::ImmediateReply {
                reason: "nl_rate_limited",
                text: limit_msg.to_string(),
            };
        }
        return BotPipelineAction::NaturalLanguage {
            permit,
            question: trimmed.to_string(),
        };
    }

    let first_token = trimmed.split_whitespace().next().unwrap_or("");
    let cmd = normalize_command(first_token);
    BotPipelineAction::Command {
        text: trimmed.to_string(),
        progress_hint: progress_text_for_command(&cmd),
    }
}

/// 飞书事件中的发送者 ID（优先 open_id）。
pub fn feishu_sender_id(event_body: &serde_json::Value) -> &str {
    event_body
        .get("sender")
        .and_then(|sender| sender.get("sender_id"))
        .and_then(|ids| {
            ["open_id", "union_id", "user_id"].iter().find_map(|key| {
                ids.get(*key)
                    .and_then(|value| value.as_str())
                    .filter(|id| !id.trim().is_empty())
            })
        })
        .unwrap_or("")
}

/// 钉钉回调中的发送者 ID 与消息 ID（优先企业内 staffId）。
pub fn dingtalk_sender_and_message_id(event: &serde_json::Value) -> (&str, &str) {
    let sender = ["senderStaffId", "senderId"]
        .iter()
        .find_map(|key| {
            event
                .get(*key)
                .and_then(|value| value.as_str())
                .filter(|id| !id.trim().is_empty())
        })
        .unwrap_or("");
    let message_id = event.get("msgId").and_then(|v| v.as_str()).unwrap_or("");
    (sender, message_id)
}

fn parse_bind_command(text: &str) -> Option<String> {
    let mut parts = text.split_whitespace();
    let command = normalize_command(parts.next().unwrap_or(""));
    if command != "bind" && command != "绑定" {
        return None;
    }
    let code = parts.next().unwrap_or("").trim().to_ascii_uppercase();
    // 裸 `/bind`（未带绑定码）视为查看用法，不消耗错误尝试次数
    if code.is_empty() {
        return None;
    }
    Some(code)
}

/// 纯函数：按绑定状态判断发送者是否可访问，必要时修改绑定（新增发送者、累计失败、作废绑定码）。
/// 返回 `(决策, 绑定是否被修改)`。
pub fn evaluate_bot_sender(
    binding: &mut work_review_core::config::BotSenderBinding,
    sender_id: &str,
    text: &str,
    now_ts: i64,
) -> (BotSenderDecision, bool) {
    let sender_id = sender_id.trim();
    if !sender_id.is_empty() && binding.allowed_senders.iter().any(|id| id == sender_id) {
        return (BotSenderDecision::Allowed, false);
    }

    let Some(input_code) = parse_bind_command(text) else {
        return (BotSenderDecision::NeedBind, false);
    };
    if sender_id.is_empty() {
        return (BotSenderDecision::NeedBind, false);
    }

    let Some(expected) = binding
        .bind_code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(str::to_ascii_uppercase)
    else {
        return (BotSenderDecision::NotConfigured, false);
    };

    let expired = binding
        .bind_code_expires_at
        .map(|expires_at| expires_at < now_ts)
        .unwrap_or(true);
    if expired || binding.failed_attempts >= BOT_BIND_MAX_FAILED_ATTEMPTS {
        binding.bind_code = None;
        binding.bind_code_expires_at = None;
        binding.failed_attempts = 0;
        return (BotSenderDecision::CodeUnavailable, true);
    }

    use subtle::ConstantTimeEq;
    let matches = input_code.len() == expected.len()
        && bool::from(input_code.as_bytes().ct_eq(expected.as_bytes()));
    if !matches {
        binding.failed_attempts += 1;
        if binding.failed_attempts >= BOT_BIND_MAX_FAILED_ATTEMPTS {
            binding.bind_code = None;
            binding.bind_code_expires_at = None;
            binding.failed_attempts = 0;
            return (BotSenderDecision::CodeUnavailable, true);
        }
        let remaining = BOT_BIND_MAX_FAILED_ATTEMPTS - binding.failed_attempts;
        return (BotSenderDecision::InvalidCode(remaining), true);
    }

    binding.allowed_senders.push(sender_id.to_string());
    binding.bind_code = None;
    binding.bind_code_expires_at = None;
    binding.failed_attempts = 0;
    (BotSenderDecision::Bound, true)
}

/// 校验并（必要时）持久化发送者绑定。配置写入失败时拒绝访问。
pub fn check_bot_sender(
    state: &std::sync::Arc<std::sync::Mutex<crate::AppState>>,
    platform: &str,
    sender_id: &str,
    text: &str,
) -> BotSenderDecision {
    let now_ts = chrono::Local::now().timestamp();
    let mut guard = match state.lock() {
        Ok(guard) => guard,
        Err(error) => {
            log::warn!("Bot 发送者校验失败（状态锁异常）: {error}");
            return BotSenderDecision::NeedBind;
        }
    };
    let mut next_config = guard.config.clone();
    let binding = next_config
        .bot_sender_bindings
        .entry(platform.to_string())
        .or_default();
    let (decision, changed) = evaluate_bot_sender(binding, sender_id, text, now_ts);
    if changed {
        let config_path = guard.config_path.clone();
        match next_config.save(&config_path) {
            Ok(()) => guard.config = next_config,
            Err(error) => {
                log::warn!("保存 {platform} Bot 绑定状态失败: {error}");
                if decision == BotSenderDecision::Bound {
                    // 新授权必须落盘才生效；绑定码不作废，可在磁盘恢复后重试
                    return BotSenderDecision::NeedBind;
                }
                // 错误次数累计 / 绑定码作废属于收紧操作：即使落盘失败也在内存中生效，
                // 避免磁盘不可写时绑定码可被无限次猜测
                guard.config = next_config;
            }
        }
    }
    if decision == BotSenderDecision::Bound {
        log::info!("{platform} Bot 新增授权发送者");
    }
    decision
}

// ── 消息去重：平台超时重试时同一消息只处理一次 ─────────────────────

const BOT_MESSAGE_DEDUP_CAPACITY: usize = 512;
const BOT_MESSAGE_DEDUP_TTL: Duration = Duration::from_secs(10 * 60);

type BotMessageDedupQueue =
    std::sync::Mutex<std::collections::VecDeque<([u8; 32], std::time::Instant)>>;

/// 定长 32 字节 SHA-256 摘要作为去重键，避免攻击者发送超长 `message_id` 膨胀内存；
/// 已授权与未授权分桶存储，避免未绑定成员发 512 条垃圾消息挤掉已绑定用户的去重记录。
static RECENT_AUTHORIZED_BOT_MESSAGES: once_cell::sync::Lazy<BotMessageDedupQueue> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::VecDeque::new()));

static RECENT_UNAUTHORIZED_BOT_MESSAGES: once_cell::sync::Lazy<BotMessageDedupQueue> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::VecDeque::new()));

fn hash_bot_dedup_key(platform: &str, message_id: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(platform.as_bytes());
    hasher.update(b":");
    hasher.update(message_id.as_bytes());
    hasher.finalize().into()
}

fn record_bot_message_in<K: PartialEq>(
    recent: &mut std::collections::VecDeque<(K, std::time::Instant)>,
    key: K,
    now: std::time::Instant,
) -> bool {
    while recent
        .front()
        .is_some_and(|(_, seen_at)| now.duration_since(*seen_at) > BOT_MESSAGE_DEDUP_TTL)
    {
        recent.pop_front();
    }
    if recent.iter().any(|(existing, _)| *existing == key) {
        return true;
    }
    if recent.len() >= BOT_MESSAGE_DEDUP_CAPACITY {
        recent.pop_front();
    }
    recent.push_back((key, now));
    false
}

fn is_duplicate_bot_message_in_bucket(platform: &str, message_id: &str, authorized: bool) -> bool {
    let message_id = message_id.trim();
    if message_id.is_empty() {
        return false;
    }
    let key = hash_bot_dedup_key(platform, message_id);
    let bucket = if authorized {
        &RECENT_AUTHORIZED_BOT_MESSAGES
    } else {
        &RECENT_UNAUTHORIZED_BOT_MESSAGES
    };
    match bucket.lock() {
        Ok(mut recent) => record_bot_message_in(&mut recent, key, std::time::Instant::now()),
        // 去重缓存锁中毒时有意 fail-open（宁可偶发重复回复，也不静默丢弃合法用户消息）。
        Err(_) => false,
    }
}

/// 记录消息并返回是否为重复消息。消息 ID 为空时不去重。
#[allow(dead_code)]
pub fn is_duplicate_bot_message(platform: &str, message_id: &str) -> bool {
    is_duplicate_bot_message_in_bucket(platform, message_id, true)
}

// ── 自然语言只读问答（M3-7：默认关闭，零写工具、零联网、每发送者限流）─────────

const BOT_NL_RATE_LIMIT_MAX: usize = 6;
const BOT_NL_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const BOT_NL_RATE_LIMIT_MAX_SENDERS: usize = 1024;
const BOT_NL_MAX_QUESTION_CHARS: usize = 1000;
const BOT_NL_TIMEOUT_SECS: u64 = 45;

type BotNlRateLimitMap =
    std::collections::HashMap<[u8; 32], std::collections::VecDeque<std::time::Instant>>;

static BOT_NL_RATE_LIMITER: once_cell::sync::Lazy<std::sync::Mutex<BotNlRateLimitMap>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// 判断文本是已知命令或 `/` 开头的斜杠命令（未知 `/foo` 命令仍应走 `UNKNOWN_CMD_REPLY`，不转交 AI）。
pub fn is_known_or_slash_command(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.starts_with('/') {
        return true;
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    let cmd = normalize_command(first);
    matches!(
        cmd.as_str(),
        "help"
            | "帮助"
            | "devices"
            | "设备列表"
            | "device"
            | "设备"
            | "reports"
            | "日报列表"
            | "report"
            | "日报"
            | "generate"
            | "生成日报"
            | "bind"
            | "绑定"
            | "start"
    )
}

fn check_bot_nl_rate_limit_in<K: Eq + std::hash::Hash + Clone>(
    buckets: &mut std::collections::HashMap<K, std::collections::VecDeque<std::time::Instant>>,
    key: K,
    now: std::time::Instant,
) -> Result<(), &'static str> {
    if let Some(queue) = buckets.get_mut(&key) {
        while queue
            .front()
            .is_some_and(|seen| now.duration_since(*seen) >= BOT_NL_RATE_LIMIT_WINDOW)
        {
            queue.pop_front();
        }
        if queue.len() >= BOT_NL_RATE_LIMIT_MAX {
            return Err(BOT_NL_RATE_LIMIT_REPLY);
        }
        queue.push_back(now);
        return Ok(());
    }

    // 清理空桶 / 过期桶；仍超容量时淘汰最久未活跃发送者，保证内存有界
    if buckets.len() >= BOT_NL_RATE_LIMIT_MAX_SENDERS {
        buckets.retain(|_, queue| {
            while queue
                .front()
                .is_some_and(|seen| now.duration_since(*seen) >= BOT_NL_RATE_LIMIT_WINDOW)
            {
                queue.pop_front();
            }
            !queue.is_empty()
        });
        if buckets.len() >= BOT_NL_RATE_LIMIT_MAX_SENDERS {
            let oldest_key = buckets
                .iter()
                .min_by_key(|(_, queue)| queue.back().copied())
                .map(|(k, _)| k.clone());
            if let Some(target_key) = oldest_key {
                buckets.remove(&target_key);
            }
        }
    }

    let mut queue = std::collections::VecDeque::with_capacity(BOT_NL_RATE_LIMIT_MAX);
    queue.push_back(now);
    buckets.insert(key, queue);
    Ok(())
}

/// 消耗一次自然语言问答配额（每发送者每 60 秒最多 6 次；空发送者或锁中毒时 fail-closed 拒绝放行）。
pub fn try_consume_bot_nl_quota(platform: &str, sender_id: &str) -> Result<(), &'static str> {
    let sender = sender_id.trim();
    if sender.is_empty() {
        return Err(BOT_NL_RATE_LIMIT_REPLY);
    }
    let key = hash_bot_dedup_key(platform, sender);
    match BOT_NL_RATE_LIMITER.lock() {
        Ok(mut buckets) => check_bot_nl_rate_limit_in(&mut buckets, key, std::time::Instant::now()),
        Err(_) => Err(BOT_NL_RATE_LIMIT_REPLY),
    }
}

/// 查询当前配置是否已开启 Bot 自然语言问答（默认关闭）。
pub fn is_bot_natural_language_enabled(
    state: &std::sync::Arc<std::sync::Mutex<crate::AppState>>,
) -> bool {
    state
        .lock()
        .map(|guard| guard.config.bot_natural_language_enabled)
        .unwrap_or(false)
}

/// 构造 Bot 只读问答使用的 `AssistantRuntime`：无行动桥、无确认桥、无语义外发、附当前隐私过滤器。
pub(crate) fn build_bot_readonly_runtime(
    avatar_followups: Vec<work_review_core::config::AvatarFollowupItem>,
    projects: Vec<work_review_core::project::ProjectDefinition>,
    privacy_filter: std::sync::Arc<work_review_core::privacy::PrivacyFilter>,
) -> crate::agent::AssistantRuntime {
    crate::agent::AssistantRuntime {
        avatar_followups,
        projects,
        actions: None,
        confirm: None,
        current_context: None,
        semantic_search: None,
        cancel: None,
        privacy_filter: Some(privacy_filter),
    }
}

/// 对 Bot 只读问答输出做纵深防御脱敏、格式规整、长度截断并追加数据范围说明。
fn finalize_bot_nl_reply(
    privacy_filter: &work_review_core::privacy::PrivacyFilter,
    raw_answer: &str,
) -> String {
    let sanitized = privacy_filter.filter_text(raw_answer);
    let formatted = truncate(&normalize_report_for_chat(&sanitized), 3200);
    format!("{formatted}{BOT_NL_SCOPE_FOOTER}")
}

/// 在调用方已完成并发许可与限流扣减后执行 Bot 只读自然语言问答。
pub async fn run_bot_natural_language_query(
    state: &std::sync::Arc<std::sync::Mutex<crate::AppState>>,
    question: &str,
    platform: &str,
) -> String {
    let trimmed = question.trim();
    if trimmed.is_empty() {
        return UNKNOWN_CMD_REPLY.to_string();
    }

    let snapshot = {
        let Ok(guard) = state.lock() else {
            return "❌ 服务暂不可用，请稍后重试。".to_string();
        };
        if !guard.config.bot_natural_language_enabled {
            return UNKNOWN_CMD_REPLY.to_string();
        }
        let (ignored_apps, excluded_domains) = crate::commands::collect_privacy_filters(&guard);
        let candidate_model = guard
            .config
            .resolve_model_for_task(work_review_core::config::AiTaskKind::Assistant);
        let model = if !candidate_model.endpoint.trim().is_empty()
            && !candidate_model.model.trim().is_empty()
        {
            Some(candidate_model)
        } else {
            None
        };
        (
            guard.database.clone(),
            ignored_apps,
            excluded_domains,
            guard.config.privacy.clone(),
            guard.config.projects.clone(),
            guard.config.avatar_followups.clone(),
            model,
        )
    };

    let (
        database,
        ignored_apps,
        excluded_domains,
        privacy_config,
        projects,
        avatar_followups,
        model,
    ) = snapshot;
    let privacy_filter = std::sync::Arc::new(
        work_review_core::privacy::PrivacyFilter::from_config(&privacy_config),
    );
    let runtime = build_bot_readonly_runtime(avatar_followups, projects, privacy_filter.clone());

    let bounded_question = truncate(trimmed, BOT_NL_MAX_QUESTION_CHARS);
    let deadline =
        crate::agent::deadline::Deadline::after(Duration::from_secs(BOT_NL_TIMEOUT_SECS));

    let result = crate::agent::tools::with_user_memory_tool_capabilities(
        crate::agent::tools::UserMemoryToolCapabilities::default(),
        crate::agent::Orchestrator::handle(
            &bounded_question,
            crate::agent::AssistantRequestClassification::work_review(false),
            model.as_ref(),
            &database,
            &[],
            Some(BOT_NL_SYSTEM_PROMPT),
            &ignored_apps,
            &excluded_domains,
            None,
            runtime,
            None,
            deadline,
        ),
    )
    .await;

    match result {
        Ok(res) => finalize_bot_nl_reply(&privacy_filter, &res.answer),
        Err(error) => {
            log::warn!("Bot 自然语言问答失败 ({platform}): {error}");
            "❌ 暂时无法回答该问题，请稍后重试或检查本机 AI 模型配置。".to_string()
        }
    }
}

#[cfg(test)]
mod sender_tests {
    use super::*;
    use work_review_core::config::BotSenderBinding;

    #[test]
    fn 应从飞书事件提取发送者() {
        let event = serde_json::json!({
            "sender": { "sender_id": { "open_id": "ou_abc", "union_id": "on_x", "user_id": "u1" } },
            "message": { "message_id": "om_1" }
        });
        assert_eq!(feishu_sender_id(&event), "ou_abc");
        let union_only =
            serde_json::json!({ "sender": { "sender_id": { "open_id": "", "union_id": "on_x" } } });
        assert_eq!(feishu_sender_id(&union_only), "on_x");
        assert_eq!(feishu_sender_id(&serde_json::json!({})), "");
    }

    #[test]
    fn 应从钉钉回调提取发送者与消息id() {
        let event = serde_json::json!({
            "senderStaffId": "staff_1", "senderId": "$:LWCP_v1:$x", "msgId": "msg_1"
        });
        assert_eq!(dingtalk_sender_and_message_id(&event), ("staff_1", "msg_1"));
        let external = serde_json::json!({ "senderStaffId": "", "senderId": "$:LWCP_v1:$x" });
        assert_eq!(
            dingtalk_sender_and_message_id(&external),
            ("$:LWCP_v1:$x", "")
        );
    }

    #[test]
    fn 平台标识应与绑定名单一致() {
        assert_eq!(
            BOT_SENDER_PLATFORMS,
            &[
                BOT_PLATFORM_FEISHU,
                BOT_PLATFORM_DINGTALK,
                BOT_PLATFORM_WECOM
            ]
        );
        assert!(BotSenderDecision::NeedBind
            .reply()
            .unwrap()
            .contains(OUTPUT_DIVIDER));
    }

    fn binding_with_code(code: &str, expires_at: i64) -> BotSenderBinding {
        BotSenderBinding {
            bind_code: Some(code.to_string()),
            bind_code_expires_at: Some(expires_at),
            ..BotSenderBinding::default()
        }
    }

    #[test]
    fn 未绑定发送者应被拒绝() {
        let mut binding = BotSenderBinding::default();
        let (decision, changed) = evaluate_bot_sender(&mut binding, "ou_1", "/report today", 100);
        assert_eq!(decision, BotSenderDecision::NeedBind);
        assert!(!changed);
        assert!(decision.reply().is_some());
    }

    #[test]
    fn 正确绑定码应授权发送者并作废绑定码() {
        let mut binding = binding_with_code("WR-ABC123", 1_000);
        let (decision, changed) = evaluate_bot_sender(&mut binding, "ou_1", "/bind wr-abc123", 100);
        assert_eq!(decision, BotSenderDecision::Bound);
        assert!(changed);
        assert_eq!(binding.allowed_senders, vec!["ou_1".to_string()]);
        assert!(binding.bind_code.is_none());

        let (decision, _) = evaluate_bot_sender(&mut binding, "ou_1", "/report today", 200);
        assert_eq!(decision, BotSenderDecision::Allowed);
        // 绑定码已作废，其他人无法复用
        let (decision, _) = evaluate_bot_sender(&mut binding, "ou_2", "/bind WR-ABC123", 200);
        assert_eq!(decision, BotSenderDecision::NotConfigured);
    }

    #[test]
    fn 过期绑定码不可用() {
        let mut binding = binding_with_code("WR-ABC123", 100);
        let (decision, changed) = evaluate_bot_sender(&mut binding, "ou_1", "/bind WR-ABC123", 200);
        assert_eq!(decision, BotSenderDecision::CodeUnavailable);
        assert!(changed);
        assert!(binding.bind_code.is_none());
    }

    #[test]
    fn 错误次数达到上限后绑定码作废() {
        let mut binding = binding_with_code("WR-ABC123", 10_000);
        for attempt in 1..BOT_BIND_MAX_FAILED_ATTEMPTS {
            let (decision, _) = evaluate_bot_sender(&mut binding, "ou_x", "/bind WRONG", 100);
            assert_eq!(
                decision,
                BotSenderDecision::InvalidCode(BOT_BIND_MAX_FAILED_ATTEMPTS - attempt)
            );
        }
        let (decision, _) = evaluate_bot_sender(&mut binding, "ou_x", "/bind WRONG", 100);
        assert_eq!(decision, BotSenderDecision::CodeUnavailable);
        assert!(binding.bind_code.is_none());
        // 作废后即使输入正确也无法绑定
        let (decision, _) = evaluate_bot_sender(&mut binding, "ou_x", "/bind WR-ABC123", 100);
        assert_eq!(decision, BotSenderDecision::NotConfigured);
        assert!(binding.allowed_senders.is_empty());
    }

    #[test]
    fn 空发送者不能绑定() {
        let mut binding = binding_with_code("WR-ABC123", 10_000);
        let (decision, changed) = evaluate_bot_sender(&mut binding, "  ", "/bind WR-ABC123", 100);
        assert_eq!(decision, BotSenderDecision::NeedBind);
        assert!(!changed);
    }

    #[test]
    fn 同一消息只处理一次且过期后可再次处理() {
        let mut recent: std::collections::VecDeque<(String, std::time::Instant)> =
            std::collections::VecDeque::new();
        let start = std::time::Instant::now();
        assert!(!record_bot_message_in(
            &mut recent,
            "feishu:m1".into(),
            start
        ));
        assert!(record_bot_message_in(
            &mut recent,
            "feishu:m1".into(),
            start
        ));
        assert!(!record_bot_message_in(
            &mut recent,
            "dingtalk:m1".into(),
            start
        ));
        let later = start + BOT_MESSAGE_DEDUP_TTL + Duration::from_secs(1);
        assert!(!record_bot_message_in(
            &mut recent,
            "feishu:m1".into(),
            later
        ));
    }

    #[test]
    fn 去重队列容量有上限() {
        let mut recent = std::collections::VecDeque::new();
        let now = std::time::Instant::now();
        for index in 0..(BOT_MESSAGE_DEDUP_CAPACITY + 10) {
            record_bot_message_in(&mut recent, format!("k{index}"), now);
        }
        assert_eq!(recent.len(), BOT_MESSAGE_DEDUP_CAPACITY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot生成日报请求应使用post_json和bearer鉴权() {
        let client = dummy_client();
        let device = DeviceEndpoint {
            name: "本机".to_string(),
            url: "http://127.0.0.1:47831/".to_string(),
            token: "wr-local-secret".to_string(),
            is_local: true,
        };

        let request =
            build_generate_report_request(&client, &device, "2026-07-21", Duration::from_secs(300))
                .build()
                .expect("请求应可构造");

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "http://127.0.0.1:47831/v1/reports/generate"
        );
        assert!(!request.url().as_str().contains("wr-local-secret"));
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer wr-local-secret")
        );
        let body = request
            .body()
            .and_then(|body| body.as_bytes())
            .expect("JSON Body 应存在");
        let payload: serde_json::Value = serde_json::from_slice(body).expect("JSON Body 应合法");
        assert_eq!(payload, serde_json::json!({ "date": "2026-07-21" }));
        assert_eq!(request.timeout(), Some(&Duration::from_secs(300)));
    }

    #[test]
    fn 命令应支持斜杠和机器人后缀() {
        assert_eq!(normalize_command("/help"), "help");
        assert_eq!(normalize_command("/reports@work_review_bot"), "reports");
        assert_eq!(normalize_command("帮助"), "帮助");
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
        assert!(progress_text_for_command("reports").is_some());
        assert!(progress_text_for_command("generate").is_some());
        assert!(progress_text_for_command("help").is_none());
    }

    /// 测试用：构造一个不会被使用的 reqwest::Client（handle_cmd 在不需要网络的
    /// 命令分支里不会真的发请求）。
    fn dummy_client() -> Client {
        Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("build client")
    }

    fn run_cmd<'a>(
        client: &'a Client,
        devices: &'a [DeviceEndpoint],
        text: &'a str,
    ) -> impl std::future::Future<Output = Option<String>> + 'a {
        handle_cmd(client, devices, text, Duration::from_secs(120))
    }

    #[tokio::test]
    async fn handle_cmd_help命令应返回帮助文本() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        let reply = run_cmd(&client, &devices, "/help").await.unwrap();
        assert!(reply.contains("Work Review Bot"));
        assert!(reply.contains("/devices"));
    }

    #[tokio::test]
    async fn handle_cmd_中文帮助别名应等价() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        let reply = run_cmd(&client, &devices, "帮助").await.unwrap();
        assert!(reply.contains("Work Review Bot"));
    }

    #[tokio::test]
    async fn handle_cmd_未知命令应返回未知提示() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        let reply = run_cmd(&client, &devices, "/random_garbage").await.unwrap();
        assert_eq!(reply, UNKNOWN_CMD_REPLY);
    }

    #[tokio::test]
    async fn handle_cmd_空文本应返回未知提示() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        let reply = run_cmd(&client, &devices, "").await.unwrap();
        assert_eq!(reply, UNKNOWN_CMD_REPLY);
    }

    #[tokio::test]
    async fn handle_cmd_无设备时设备类命令应返回无可用设备() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        // /devices 命中 devices.is_empty() 分支
        let reply = run_cmd(&client, &devices, "/devices").await.unwrap();
        assert!(reply.contains("无可用设备"));
        // /device 应通过 find_device 找不到设备时回落到无可用设备提示
        let reply = run_cmd(&client, &devices, "/device").await.unwrap();
        assert!(reply.contains("无可用设备"));
        // /reports
        let reply = run_cmd(&client, &devices, "/reports").await.unwrap();
        assert!(reply.contains("无可用设备"));
        // /report
        let reply = run_cmd(&client, &devices, "/report today").await.unwrap();
        assert!(reply.contains("无可用设备"));
        // /generate
        let reply = run_cmd(&client, &devices, "/generate today").await.unwrap();
        assert!(reply.contains("无可用设备"));
    }

    #[tokio::test]
    async fn handle_cmd_中文别名也应触发分发() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        // 这些命令都没有设备，全部应返回"无可用设备"，证明中英文别名都正确分发了。
        for cmd in ["设备列表", "设备", "日报列表", "日报", "生成日报"] {
            let reply = run_cmd(&client, &devices, cmd).await.unwrap();
            assert!(
                reply.contains("无可用设备"),
                "命令 {cmd} 应该分发到无设备分支，实际：{reply}"
            );
        }
    }

    #[tokio::test]
    async fn handle_cmd_命令大小写与多余空白应被规范化() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        // 大写 + 多空白 + @机器人后缀都应被 normalize_command 处理
        let reply = run_cmd(&client, &devices, "  /HELP@work_review_bot  ")
            .await
            .unwrap();
        assert!(reply.contains("Work Review Bot"));
    }

    #[test]
    fn 局域网通配监听地址下本机自调用应固定走回环127() {
        let mut config = AppConfig {
            localhost_api_host: Some("0.0.0.0".to_string()),
            ..AppConfig::default()
        };
        assert_eq!(effective_host(&config), "127.0.0.1");

        config.localhost_api_host = Some("::".to_string());
        assert_eq!(effective_host(&config), "127.0.0.1");

        config.localhost_api_host = Some("  ".to_string());
        assert_eq!(effective_host(&config), "127.0.0.1");

        config.localhost_api_host = Some("192.168.1.50".to_string());
        assert_eq!(effective_host(&config), "192.168.1.50");
    }

    #[test]
    fn 已知命令与斜杠命令不应误判为自然语言问答() {
        for cmd in [
            "",
            "   ",
            "/help",
            "/report today",
            "/unknown_cmd",
            "/bind WR-1234",
            "帮助",
            "设备列表",
            "日报 today",
            "生成日报 yesterday",
            "bind WR-1234",
        ] {
            assert!(
                is_known_or_slash_command(cmd),
                "「{cmd}」应识别为命令而非自然语言"
            );
        }
        for question in [
            "今天我写了多久代码？",
            "本周开发占比多少",
            "帮我总结一下昨天主要在忙什么",
            "what did I work on today?",
        ] {
            assert!(
                !is_known_or_slash_command(question),
                "「{question}」应识别为自然语言问题"
            );
        }
    }

    #[test]
    fn bot自然语言限流器应按窗口拦截并限制桶容量() {
        let mut buckets: std::collections::HashMap<
            String,
            std::collections::VecDeque<std::time::Instant>,
        > = std::collections::HashMap::new();
        let t0 = std::time::Instant::now();
        for _ in 0..BOT_NL_RATE_LIMIT_MAX {
            assert!(check_bot_nl_rate_limit_in(&mut buckets, "feishu:u1".into(), t0).is_ok());
        }
        assert!(check_bot_nl_rate_limit_in(&mut buckets, "feishu:u1".into(), t0).is_err());
        // 其他发送者不受影响
        assert!(check_bot_nl_rate_limit_in(&mut buckets, "feishu:u2".into(), t0).is_ok());
        // 超过 60 秒窗口后恢复配额
        let t1 = t0 + BOT_NL_RATE_LIMIT_WINDOW + Duration::from_secs(1);
        assert!(check_bot_nl_rate_limit_in(&mut buckets, "feishu:u1".into(), t1).is_ok());

        // 超过 1024 个发送者时容量应保持有界
        let mut bounded: std::collections::HashMap<
            String,
            std::collections::VecDeque<std::time::Instant>,
        > = std::collections::HashMap::new();
        for i in 0..(BOT_NL_RATE_LIMIT_MAX_SENDERS + 20) {
            let ts = t0 + Duration::from_millis(i as u64);
            assert!(check_bot_nl_rate_limit_in(&mut bounded, format!("u{i}"), ts).is_ok());
        }
        assert!(bounded.len() <= BOT_NL_RATE_LIMIT_MAX_SENDERS);
    }

    #[tokio::test]
    async fn bot只读工具集不得暴露任何写工具或联网工具() {
        let privacy_filter =
            std::sync::Arc::new(work_review_core::privacy::PrivacyFilter::from_config(
                &work_review_core::config::PrivacyConfig::default(),
            ));
        let runtime = build_bot_readonly_runtime(Vec::new(), Vec::new(), privacy_filter);
        let registry = crate::agent::tools::with_user_memory_tool_capabilities(
            crate::agent::tools::UserMemoryToolCapabilities::default(),
            async {
                crate::agent::executor::AgentExecutor::build_tool_registry_for_runtime(
                    crate::agent::AssistantRequestMode::WorkReview,
                    None,
                    &runtime,
                )
            },
        )
        .await;
        let names: Vec<String> = registry
            .to_openai_tools()
            .iter()
            .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
            .collect();
        let mut actual_sorted: Vec<&str> = names.iter().map(String::as_str).collect();
        actual_sorted.sort_unstable();
        let mut expected_readonly = vec![
            "aggregate_stats",
            "analyze_intents",
            "category_search",
            "extract_todos",
            "get_current_context",
            "get_daily_report",
            "get_hourly_summaries",
            "get_insights",
            "get_reports_in_range",
            "get_work_sessions",
            "list_reports",
            "list_todos",
            "project_stats",
            "query_activities",
            "search_memory",
            "trend_comparison",
            "weekly_review",
        ];
        expected_readonly.sort_unstable();
        assert_eq!(
            actual_sorted, expected_readonly,
            "Bot 只读工具集必须严格等于白名单只读工具集合"
        );
    }

    #[test]
    fn 共享时间戳窗口校验应覆盖秒与毫秒及边界值() {
        // 确定性边界测试：精确覆盖过去与未来方向的 ±window 和 ±(window + 1)
        let fixed_now = 1_700_000_000i64;
        assert!(timestamp_within_window_at(
            fixed_now,
            &(fixed_now - BOT_SIGN_WINDOW_SECS).to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(timestamp_within_window_at(
            fixed_now,
            &(fixed_now + BOT_SIGN_WINDOW_SECS).to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(!timestamp_within_window_at(
            fixed_now,
            &(fixed_now - BOT_SIGN_WINDOW_SECS - 1).to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(!timestamp_within_window_at(
            fixed_now,
            &(fixed_now + BOT_SIGN_WINDOW_SECS + 1).to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(!timestamp_within_window_at(
            fixed_now,
            "-1",
            BOT_SIGN_WINDOW_SECS
        ));

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(timestamp_within_window_secs(
            &now_secs.to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(timestamp_within_window_secs(
            &(now_secs - 120).to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(!timestamp_within_window_secs(
            &(now_secs - 3600).to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(!timestamp_within_window_secs(
            &(now_secs + 3600).to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(!timestamp_within_window_secs(
            &i64::MIN.to_string(),
            BOT_SIGN_WINDOW_SECS
        ));
        assert!(!timestamp_within_window_secs("bad", BOT_SIGN_WINDOW_SECS));

        let now_ms = now_secs * 1000;
        assert!(timestamp_within_window_ms(
            &now_ms.to_string(),
            BOT_SIGN_WINDOW_MS
        ));
        assert!(!timestamp_within_window_ms(
            &(now_ms - 10 * 60 * 1000).to_string(),
            BOT_SIGN_WINDOW_MS
        ));
        assert!(!timestamp_within_window_ms(
            &(now_ms + 10 * 60 * 1000).to_string(),
            BOT_SIGN_WINDOW_MS
        ));
        assert!(!timestamp_within_window_ms(
            &i64::MIN.to_string(),
            BOT_SIGN_WINDOW_MS
        ));
    }

    #[test]
    fn 统一bot入站管线应按授权去重并发与命令顺序路由() {
        let dir = std::env::temp_dir().join(format!(
            "work-review-bot-pipeline-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = work_review_core::database::Database::new(&dir.join("test.db")).unwrap();
        let mut config = AppConfig::default();
        config
            .bot_sender_bindings
            .entry(BOT_PLATFORM_FEISHU.to_string())
            .or_default()
            .allowed_senders
            .push("u-authorized".to_string());
        let state = std::sync::Arc::new(std::sync::Mutex::new(crate::AppState {
            config: config.clone(),
            config_load_status: work_review_core::config::ConfigLoadStatus::Loaded,
            database: db.clone(),
            privacy_filter: work_review_core::privacy::PrivacyFilter::from_config(&config.privacy),
            screenshot_service: crate::screenshot::ScreenshotService::new(&dir, &config.storage),
            storage_manager: crate::storage::StorageManager::new(&dir, config.storage.clone()),
            data_dir: dir.clone(),
            config_path: dir.join("config.json"),
            is_recording: false,
            is_paused: false,
            avatar_state: crate::avatar_engine::default_avatar_state(),
            avatar_generating_report: false,
            generating_report: false,
            localhost_api_runtime: crate::localhost_api::LocalhostApiRuntime::default(),
            telegram_bot_runtime: crate::telegram_bot::TelegramBotRuntime::default(),
            wecom_bot_runtime: crate::wecom_aibot::WecomBotRuntime::default(),
            cached_active_window: None,
        }));
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let unique_prefix = format!(
            "m4-pipe-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let rate_limit_sender = format!("u-rate-{unique_prefix}");
        state
            .lock()
            .unwrap()
            .config
            .bot_sender_bindings
            .entry(BOT_PLATFORM_FEISHU.to_string())
            .or_default()
            .allowed_senders
            .push(rate_limit_sender.clone());

        // 1. 未授权用户返回 NeedBind，且同 message_id 第二次命中 Duplicate
        let unauth_id = format!("{unique_prefix}-unauth");
        let unauth_msg = InboundBotMessage::standard(
            BOT_PLATFORM_FEISHU,
            &unauth_id,
            "u-stranger",
            "/help",
            true,
        );
        assert!(matches!(
            route_inbound_bot_message(&state, &unauth_msg, &sem, true, None),
            BotPipelineAction::ImmediateReply {
                reason: "sender_authorization",
                ..
            }
        ));
        assert!(matches!(
            route_inbound_bot_message(&state, &unauth_msg, &sem, true, None),
            BotPipelineAction::Duplicate
        ));

        // 2. 空 message_id 不得互相误判为重复消息
        let empty_id_msg =
            InboundBotMessage::standard(BOT_PLATFORM_FEISHU, "", "u-authorized", "/help", true);
        assert!(matches!(
            route_inbound_bot_message(&state, &empty_id_msg, &sem, true, None),
            BotPipelineAction::Command { .. }
        ));
        assert!(matches!(
            route_inbound_bot_message(&state, &empty_id_msg, &sem, true, None),
            BotPipelineAction::Command { .. }
        ));

        // 3. pre_authorized 路径：空 sender_id 必须拒绝，合法 sender_id 走已授权去重桶
        let pre_empty_id = format!("{unique_prefix}-tg-empty");
        let pre_empty_sender =
            InboundBotMessage::pre_authorized("telegram", &pre_empty_id, "  ", "/help", true);
        assert!(matches!(
            route_inbound_bot_message(&state, &pre_empty_sender, &sem, true, None),
            BotPipelineAction::ImmediateReply {
                reason: "sender_authorization",
                ..
            }
        ));
        let pre_id = format!("{unique_prefix}-tg-ok");
        let pre_ok = InboundBotMessage::pre_authorized("telegram", &pre_id, "12345", "   ", true);
        assert!(matches!(
            route_inbound_bot_message(&state, &pre_ok, &sem, true, Some(EMPTY_TEXT_REPLY)),
            BotPipelineAction::ImmediateReply {
                reason: "empty_text",
                ref text,
            } if text == EMPTY_TEXT_REPLY
        ));
        assert!(matches!(
            route_inbound_bot_message(&state, &pre_ok, &sem, true, Some(EMPTY_TEXT_REPLY)),
            BotPipelineAction::Duplicate
        ));

        // 4. 已授权用户的已知命令走 Command 并携带进度提示
        let cmd_id = format!("{unique_prefix}-cmd");
        let cmd_msg = InboundBotMessage::standard(
            BOT_PLATFORM_FEISHU,
            &cmd_id,
            "u-authorized",
            "/report today",
            true,
        );
        match route_inbound_bot_message(&state, &cmd_msg, &sem, true, None) {
            BotPipelineAction::Command {
                text,
                progress_hint,
            } => {
                assert_eq!(text, "/report today");
                assert!(progress_hint.is_some());
            }
            _ => panic!("已知命令应路由到 Command"),
        }

        // 5. 已授权用户的自然语言提问获取并发许可；许可占满时返回 nl_busy 且不耗尽配额
        let nl_id_1 = format!("{unique_prefix}-nl-1");
        let nl_msg = InboundBotMessage::standard(
            BOT_PLATFORM_FEISHU,
            &nl_id_1,
            &rate_limit_sender,
            "我今天在哪些项目花了最多时间",
            true,
        );
        let held_permit = match route_inbound_bot_message(&state, &nl_msg, &sem, true, None) {
            BotPipelineAction::NaturalLanguage { permit, question } => {
                assert_eq!(question, "我今天在哪些项目花了最多时间");
                permit
            }
            _ => panic!("自然语言应路由到 NaturalLanguage"),
        };
        let nl_id_busy = format!("{unique_prefix}-nl-busy");
        let nl_msg_busy = InboundBotMessage::standard(
            BOT_PLATFORM_FEISHU,
            &nl_id_busy,
            &rate_limit_sender,
            "昨天写了多久代码",
            true,
        );
        assert!(matches!(
            route_inbound_bot_message(&state, &nl_msg_busy, &sem, true, None),
            BotPipelineAction::ImmediateReply {
                reason: "nl_busy",
                ..
            }
        ));
        drop(held_permit);

        // 6. 连续触发 NL 限流后必须立即释放信号量许可，不发生泄漏
        for i in 2..=BOT_NL_RATE_LIMIT_MAX {
            let id = format!("{unique_prefix}-nl-{i}");
            let msg = InboundBotMessage::standard(
                BOT_PLATFORM_FEISHU,
                &id,
                &rate_limit_sender,
                "分析一下今天效率",
                true,
            );
            match route_inbound_bot_message(&state, &msg, &sem, true, None) {
                BotPipelineAction::NaturalLanguage { permit, .. } => drop(permit),
                _ => panic!("第 {i} 次应在配额内放行"),
            }
        }
        let limited_id = format!("{unique_prefix}-nl-limited");
        let limited_msg = InboundBotMessage::standard(
            BOT_PLATFORM_FEISHU,
            &limited_id,
            &rate_limit_sender,
            "再问一次触发限流",
            true,
        );
        assert!(matches!(
            route_inbound_bot_message(&state, &limited_msg, &sem, true, None),
            BotPipelineAction::ImmediateReply {
                reason: "nl_rate_limited",
                ..
            }
        ));
        assert_eq!(
            sem.available_permits(),
            1,
            "触发 nl_rate_limited 后必须归还并发许可"
        );

        // 7. 非文本消息在已授权后返回 non_text
        let non_text_id = format!("{unique_prefix}-img");
        let non_text = InboundBotMessage::standard(
            BOT_PLATFORM_FEISHU,
            &non_text_id,
            "u-authorized",
            "",
            false,
        );
        assert!(matches!(
            route_inbound_bot_message(&state, &non_text, &sem, true, None),
            BotPipelineAction::ImmediateReply {
                reason: "non_text",
                ..
            }
        ));

        drop(state);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bot自然语言输出应经隐私过滤器纵深脱敏并默认关闭开关() {
        assert!(!work_review_core::config::AppConfig::default().bot_natural_language_enabled);
        let filter = work_review_core::privacy::PrivacyFilter::from_config(
            &work_review_core::config::PrivacyConfig::default(),
        );
        let reply = finalize_bot_nl_reply(
            &filter,
            "今日处理了配置，其中包含 api_key=sk-1234567890abcdef1234567890 密钥。",
        );
        assert!(
            !reply.contains("sk-1234567890abcdef1234567890"),
            "敏感密钥不得泄露到 Bot 回复: {reply}"
        );
        assert!(reply.ends_with(BOT_NL_SCOPE_FOOTER));
    }
}
