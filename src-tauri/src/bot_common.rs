use crate::config::{AppConfig, DEFAULT_LOCALHOST_API_PORT};
use crate::localhost_api::LOCALHOST_API_HOST;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

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
pub const NON_TEXT_REPLY: &str = "暂不支持非文本消息，发送 /help 查看帮助";
pub const OUTPUT_DIVIDER: &str = "────────────";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEndpoint {
    pub name: String,
    pub url: String,
    pub token: String,
    pub is_local: bool,
}

pub fn effective_host(config: &AppConfig) -> String {
    config
        .localhost_api_host
        .as_deref()
        .map(|h| h.trim())
        .filter(|h| !h.is_empty())
        .unwrap_or(LOCALHOST_API_HOST)
        .to_string()
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

pub fn progress_text_for_command(cmd: &str) -> Option<&'static str> {
    match cmd {
        "devices" | "设备列表" => Some("⏳ 正在获取设备列表，请稍候..."),
        "device" | "设备" => Some("⏳ 正在获取设备状态，请稍候..."),
        "reports" | "日报列表" => Some("⏳ 正在获取日报列表，请稍候..."),
        "report" | "日报" => Some("⏳ 正在获取日报详情，请稍候..."),
        "generate" | "生成日报" => Some("⏳ 正在生成日报，预计需要 30-120 秒..."),
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
pub async fn api_get(
    client: &Client,
    url: &str,
    token: Option<&str>,
) -> Option<serde_json::Value> {
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
    report_timeout_secs: u64,
) -> reqwest::RequestBuilder {
    let url = format!("{}/v1/reports/generate", device.url.trim_end_matches('/'));
    client
        .post(url)
        .bearer_auth(&device.token)
        .json(&serde_json::json!({ "date": date }))
        // 设备侧有自己的日报生成超时；委托方多留 30s 缓冲，避免设备还没收尾委托先超时
        .timeout(Duration::from_secs(report_timeout_secs.saturating_add(30)))
}

pub async fn handle_cmd(
    client: &Client,
    devices: &[DeviceEndpoint],
    text: &str,
    report_timeout_secs: u64,
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
            let device = match select_device(devices, parts.get(2).copied().unwrap_or("")) {
                Ok(d) => d,
                Err(reply) => return Some(reply),
            };
            // 日报生成耗时由用户配置（可达数百秒），而企微等被动回复通道 5 秒即超时；
            // 因此改为后台任务生成 + 立即回执，结果通过 /report 查询。
            let request = build_generate_report_request(client, device, &date, report_timeout_secs);
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

        let request = build_generate_report_request(&client, &device, "2026-07-21", 300)
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
        assert_eq!(request.timeout(), Some(&Duration::from_secs(330)));
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

    #[tokio::test]
    async fn handle_cmd_help命令应返回帮助文本() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        let reply = handle_cmd(&client, &devices, "/help", 300).await.unwrap();
        assert!(reply.contains("Work Review Bot"));
        assert!(reply.contains("/devices"));
    }

    #[tokio::test]
    async fn handle_cmd_中文帮助别名应等价() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        let reply = handle_cmd(&client, &devices, "帮助", 300).await.unwrap();
        assert!(reply.contains("Work Review Bot"));
    }

    #[tokio::test]
    async fn handle_cmd_未知命令应返回未知提示() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        let reply = handle_cmd(&client, &devices, "/random_garbage", 300)
            .await
            .unwrap();
        assert_eq!(reply, UNKNOWN_CMD_REPLY);
    }

    #[tokio::test]
    async fn handle_cmd_空文本应返回未知提示() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        let reply = handle_cmd(&client, &devices, "", 300).await.unwrap();
        assert_eq!(reply, UNKNOWN_CMD_REPLY);
    }

    #[tokio::test]
    async fn handle_cmd_无设备时设备类命令应返回无可用设备() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        // /devices 命中 devices.is_empty() 分支
        let reply = handle_cmd(&client, &devices, "/devices", 300).await.unwrap();
        assert!(reply.contains("无可用设备"));
        // /device 应通过 find_device 找不到设备时回落到无可用设备提示
        let reply = handle_cmd(&client, &devices, "/device", 300).await.unwrap();
        assert!(reply.contains("无可用设备"));
        // /reports
        let reply = handle_cmd(&client, &devices, "/reports", 300).await.unwrap();
        assert!(reply.contains("无可用设备"));
        // /report
        let reply = handle_cmd(&client, &devices, "/report today", 300)
            .await
            .unwrap();
        assert!(reply.contains("无可用设备"));
        // /generate
        let reply = handle_cmd(&client, &devices, "/generate today", 300)
            .await
            .unwrap();
        assert!(reply.contains("无可用设备"));
    }

    #[tokio::test]
    async fn handle_cmd_中文别名也应触发分发() {
        let client = dummy_client();
        let devices: Vec<DeviceEndpoint> = Vec::new();
        // 这些命令都没有设备，全部应返回"无可用设备"，证明中英文别名都正确分发了。
        for cmd in ["设备列表", "设备", "日报列表", "日报", "生成日报"] {
            let reply = handle_cmd(&client, &devices, cmd, 300).await.unwrap();
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
        let reply = handle_cmd(&client, &devices, "  /HELP@work_review_bot  ", 300)
            .await
            .unwrap();
        assert!(reply.contains("Work Review Bot"));
    }
}
