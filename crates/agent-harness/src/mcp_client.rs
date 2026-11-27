//! 外部 HTTP MCP Client 配置持久化、安全 URL 校验与 JSON-RPC 2.0 协议编解码。
//!
//! 安全边界：
//! - 仅支持 `http://` / `https://` 远程或显式开启 `allow_localhost` 的本机回环 MCP 服务，
//!   绝不执行任意本地 shell 命令（遵守最小权限安全不变量）。
//! - 配置层即拒绝内网私网段、链路本地/云元数据与未授权回环字面量目标；传输层进一步做 DNS 解析校验与
//!   SocketAddr 固定连接防 DNS-rebinding。
//! - 认证 Token 仅持久化在本地 `<data_dir>/mcp_clients.json`（Unix 下权限 `0600`），对外返回视图
//!   `McpClientServerView` 永远屏蔽明文 Token，仅暴露 `hasAuthToken: bool`。
//! - 所有外部 MCP 工具统一命名空间化为 `mcp__<server_id>__<tool_name>`，且禁止两段包含 `__`，
//!   保证映射双射、无碰撞且绝不可覆盖内置工具。

use crate::skills::is_valid_skill_name;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MCP_CLIENTS_FILE_NAME: &str = "mcp_clients.json";
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const MAX_MCP_SERVERS: usize = 8;
const MAX_MCP_FILE_BYTES: u64 = 64 * 1024;
pub const MAX_TOOLS_PER_MCP_SERVER: usize = 24;
const MAX_MCP_INPUT_SCHEMA_BYTES: usize = 16 * 1024;
const MAX_MCP_TOOL_OUTPUT_CHARS: usize = 32_000;

static MCP_CLIENTS_IO_LOCK: Mutex<()> = Mutex::new(());

/// 落盘在本地数据目录的完整外部 MCP Server 配置（含可选认证 Token，不直接透出前端）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpClientServerConfig {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    /// 是否允许连接本机回环地址（`127.0.0.1` / `localhost` / `::1`）。
    /// 关闭时仅允许公网地址，且无论开关均拒绝内网私网段与云元数据地址。
    pub allow_localhost: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
}

/// 暴露给前端的安全视图（永不包含明文 `auth_token`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpClientServerView {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    pub allow_localhost: bool,
    pub has_auth_token: bool,
}

impl From<&McpClientServerConfig> for McpClientServerView {
    fn from(cfg: &McpClientServerConfig) -> Self {
        Self {
            id: cfg.id.clone(),
            name: cfg.name.clone(),
            endpoint: cfg.endpoint.clone(),
            enabled: cfg.enabled,
            allow_localhost: cfg.allow_localhost,
            has_auth_token: cfg
                .auth_token
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty()),
        }
    }
}

/// 创建/更新外部 MCP Server 的前端输入。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveMcpClientServerInput {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    pub allow_localhost: bool,
    /// `None` 表示更新时保留原 Token；`Some("")` 表示清空 Token；`Some(non_empty)` 表示设置新 Token。
    pub auth_token: Option<String>,
}

/// 单个外部 MCP 工具的发现元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredMcpTool {
    pub server_id: String,
    pub server_name: String,
    pub remote_name: String,
    /// 注入给模型与策略引擎的全局唯一名称：`mcp__<server_id>__<remote_name>`。
    pub qualified_name: String,
    pub description: String,
    pub input_schema: Value,
}

/// 校验 MCP 命名空间段（`server_id` 或 `remote_name`）：在合法标识基础上禁止包含 `__`
/// 且禁止首尾为 `_`，确保 `mcp__<server_id>__<remote_name>` 严格双射、无三下划线歧义与跨服务碰撞。
pub fn is_valid_mcp_segment(name: &str) -> bool {
    is_valid_skill_name(name)
        && !name.contains("__")
        && !name.starts_with('_')
        && !name.ends_with('_')
}

/// 构造命名空间化的外部 MCP 工具名（严格校验两侧标识，防注入、碰撞或覆盖内置工具）。
pub fn build_qualified_mcp_tool_name(server_id: &str, remote_name: &str) -> Option<String> {
    if !is_valid_mcp_segment(server_id) || !is_valid_mcp_segment(remote_name) {
        return None;
    }
    Some(format!("mcp__{server_id}__{remote_name}"))
}

/// 解析命名空间化的 MCP 工具名，返回 `(server_id, remote_name)`。
pub fn parse_qualified_mcp_tool_name(qualified: &str) -> Option<(&str, &str)> {
    let rest = qualified.strip_prefix("mcp__")?;
    let (server_id, remote_name) = rest.split_once("__")?;
    if !is_valid_mcp_segment(server_id) || !is_valid_mcp_segment(remote_name) {
        return None;
    }
    Some((server_id, remote_name))
}

/// 构造 MCP `initialize` JSON-RPC 2.0 请求体。
pub fn build_mcp_initialize_request(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "work-review-personal-agent",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

/// 构造 MCP `notifications/initialized` JSON-RPC 2.0 通知（无 `id`，Streamable HTTP 握手必需）。
pub fn build_mcp_initialized_notification() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
}

/// 构造 MCP `tools/list` JSON-RPC 2.0 请求体。
pub fn build_mcp_tools_list_request(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/list",
        "params": {}
    })
}

/// 构造 MCP `tools/call` JSON-RPC 2.0 请求体。
pub fn build_mcp_tools_call_request(id: u64, remote_tool_name: &str, arguments: Value) -> Value {
    let args = if arguments.is_object() {
        arguments
    } else {
        json!({})
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {
            "name": remote_tool_name,
            "arguments": args
        }
    })
}

/// 校验 JSON-RPC 2.0 响应信封的版本与请求 `id` 一致性。
pub fn validate_jsonrpc_response_envelope(payload: &Value, expected_id: u64) -> Result<(), String> {
    if payload.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("MCP 响应缺少合法的 jsonrpc=\"2.0\" 字段".to_string());
    }
    if payload.get("id").and_then(Value::as_u64) != Some(expected_id) {
        return Err(format!("MCP 响应 id 与请求 id({expected_id}) 不匹配"));
    }
    Ok(())
}

/// 从 MCP `tools/list` JSON-RPC 响应中解析并清洗合法工具列表（上限 `MAX_TOOLS_PER_MCP_SERVER`）。
pub fn parse_mcp_tools_list_response(
    server_id: &str,
    server_name: &str,
    expected_id: u64,
    payload: &Value,
) -> Result<Vec<DiscoveredMcpTool>, String> {
    validate_jsonrpc_response_envelope(payload, expected_id)?;
    if let Some(err) = payload.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("MCP 服务返回错误");
        return Err(format!("MCP tools/list 失败: {msg}"));
    }
    let tools = payload
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Value::as_array)
        .ok_or_else(|| "MCP tools/list 响应缺少 result.tools 数组".to_string())?;

    let mut out = Vec::new();
    for item in tools.iter().take(MAX_TOOLS_PER_MCP_SERVER) {
        let Some(remote_name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(qualified_name) = build_qualified_mcp_tool_name(server_id, remote_name) else {
            continue;
        };
        let raw_desc = item
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("外部 MCP 工具");
        let description: String = raw_desc
            .chars()
            .filter(|c| !c.is_control())
            .take(240)
            .collect();
        let input_schema = item
            .get("inputSchema")
            .filter(|v| v.is_object())
            .filter(|v| {
                serde_json::to_string(v)
                    .map(|s| s.len() <= MAX_MCP_INPUT_SCHEMA_BYTES)
                    .unwrap_or(false)
            })
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
        out.push(DiscoveredMcpTool {
            server_id: server_id.to_string(),
            server_name: server_name.to_string(),
            remote_name: remote_name.to_string(),
            qualified_name,
            description,
            input_schema,
        });
    }
    Ok(out)
}

/// 从 MCP `tools/call` JSON-RPC 响应中提取受限长度的纯文本结果。
pub fn parse_mcp_tools_call_response(expected_id: u64, payload: &Value) -> Result<String, String> {
    validate_jsonrpc_response_envelope(payload, expected_id)?;
    if let Some(err) = payload.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("MCP 工具调用返回错误");
        return Err(format!("MCP 工具错误: {msg}"));
    }
    let result = payload
        .get("result")
        .ok_or_else(|| "MCP tools/call 响应缺少 result 字段".to_string())?;
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for block in content {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }
    let raw_text = if parts.is_empty() {
        result.to_string()
    } else {
        parts.join("\n")
    };
    let text = if raw_text.chars().count() > MAX_MCP_TOOL_OUTPUT_CHARS {
        let clipped: String = raw_text.chars().take(MAX_MCP_TOOL_OUTPUT_CHARS).collect();
        format!("{clipped}\n...[外部 MCP 输出已按上限截断]")
    } else {
        raw_text
    };
    if is_error {
        Err(format!("MCP 工具返回错误: {text}"))
    } else {
        Ok(text)
    }
}

fn mcp_clients_path(data_dir: &Path) -> PathBuf {
    data_dir.join(MCP_CLIENTS_FILE_NAME)
}

fn sanitize_single_line(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

/// 规范化 IP（将 IPv4-mapped IPv6 如 `::ffff:127.0.0.1` 还原为 IPv4）以统一回环/私网判断。
pub fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        IpAddr::V4(v4) => IpAddr::V4(v4),
    }
}

fn is_literal_non_public_ip(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            (segments[0] & 0xe000) != 0x2000
                || (segments[0] == 0x2001
                    && (segments[1] == 0x0000
                        || (segments[1] == 0x0002 && segments[2] == 0x0000)
                        || (0x0010..=0x002f).contains(&segments[1])
                        || segments[1] == 0x0db8))
                || segments[0] == 0x2002
                || (segments[0] == 0x3fff && (segments[1] & 0xf000) == 0x0000)
        }
    }
}

fn extract_authority_host(authority: &str) -> Option<String> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        return Some(authority[1..end].to_string());
    }
    let host = authority.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// 配置层纵深校验 Endpoint 语法与字面量私网/回环/元数据主机（不依赖外部 URL crate）。
fn validate_endpoint_config(endpoint: &str, allow_localhost: bool) -> Result<String, String> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() || trimmed.len() > 512 {
        return Err("MCP Endpoint 不能为空且不超过 512 字符".to_string());
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("MCP Endpoint 包含非法空白或控制字符".to_string());
    }
    let lower = trimmed.to_ascii_lowercase();
    let is_http_plaintext = lower.starts_with("http://");
    let is_https = lower.starts_with("https://");
    if !is_http_plaintext && !is_https {
        return Err("MCP Endpoint 仅支持 http:// 或 https:// 协议".to_string());
    }
    let after_scheme = trimmed.split_once("://").map(|(_, r)| r).unwrap_or("");
    let authority = after_scheme.split('/').next().unwrap_or("");
    if authority.contains('@') || authority.is_empty() {
        return Err("MCP Endpoint 禁止包含内联认证信息且必须包含主机名".to_string());
    }
    let host = extract_authority_host(authority)
        .ok_or_else(|| "MCP Endpoint 缺少有效主机名".to_string())?;
    let host_clean = host.trim_end_matches('.').to_ascii_lowercase();

    // 拒绝纯数字/十六进制短写 IPv4（防 2130706433 / 0x7f000001 等字面量绕过）
    if host_clean.starts_with("0x") || host_clean.chars().all(|c| c.is_ascii_digit()) {
        return Err("MCP Endpoint 禁止使用数字编码 IP 地址".to_string());
    }

    if let Ok(ip) = host_clean.parse::<IpAddr>() {
        let norm = normalize_ip(ip);
        if norm.is_loopback() {
            if !allow_localhost {
                return Err("未开启允许本机回环时禁止配置回环 MCP 地址".to_string());
            }
            return Ok(trimmed.to_string());
        }
        if is_literal_non_public_ip(norm) {
            return Err("MCP Endpoint 禁止指向内网私网段、链路本地或云元数据地址".to_string());
        }
        if is_http_plaintext {
            return Err(
                "公网 MCP Endpoint 必须使用 https://，明文 http:// 仅允许本机回环".to_string(),
            );
        }
        return Ok(trimmed.to_string());
    }

    if host_clean == "localhost" || host_clean.ends_with(".localhost") {
        if !allow_localhost {
            return Err("未开启允许本机回环时禁止配置 localhost MCP 地址".to_string());
        }
        return Ok(trimmed.to_string());
    }
    if !host_clean.contains('.')
        || host_clean.ends_with(".local")
        || host_clean.ends_with(".internal")
        || host_clean.ends_with(".lan")
        || host_clean.ends_with(".home")
        || host_clean.ends_with(".arpa")
    {
        return Err("MCP Endpoint 禁止指向本机单标签或局域网域名".to_string());
    }
    if is_http_plaintext {
        return Err("公网 MCP Endpoint 必须使用 https://，明文 http:// 仅允许本机回环".to_string());
    }
    Ok(trimmed.to_string())
}

/// 加载全部已配置的外部 MCP Server（含 Token，仅供后端执行桥使用）。
pub fn load_mcp_servers_from_dir(data_dir: &Path) -> Vec<McpClientServerConfig> {
    let path = mcp_clients_path(data_dir);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return Vec::new();
    };
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_MCP_FILE_BYTES {
        return Vec::new();
    }
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(mut items) = serde_json::from_str::<Vec<McpClientServerConfig>>(&raw) else {
        return Vec::new();
    };
    items.retain(|item| {
        is_valid_mcp_segment(&item.id)
            && validate_endpoint_config(&item.endpoint, item.allow_localhost).is_ok()
    });
    items.truncate(MAX_MCP_SERVERS);
    items
}

fn cleanup_stale_mcp_tmp_files(data_dir: &Path) {
    let prefix = format!(".{MCP_CLIENTS_FILE_NAME}.");
    let Ok(entries) = fs::read_dir(data_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str.starts_with(&prefix) && name_str.ends_with(".tmp") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn write_mcp_servers_atomic(
    data_dir: &Path,
    servers: &[McpClientServerConfig],
) -> Result<(), String> {
    if let Ok(dir_meta) = fs::symlink_metadata(data_dir) {
        if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
            return Err("数据目录为符号链接或非法路径，拒绝写入 MCP 配置".to_string());
        }
    } else {
        fs::create_dir_all(data_dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }

    let path = mcp_clients_path(data_dir);
    if let Ok(file_meta) = fs::symlink_metadata(&path) {
        if file_meta.file_type().is_symlink() || !file_meta.is_file() {
            return Err("MCP 配置文件为符号链接或非普通文件，拒绝覆盖".to_string());
        }
    }

    let payload =
        serde_json::to_string_pretty(servers).map_err(|e| format!("序列化 MCP 配置失败: {e}"))?;
    if payload.len() as u64 > MAX_MCP_FILE_BYTES {
        return Err("MCP 配置文件体积超出上限".to_string());
    }

    let tmp = data_dir.join(format!(
        ".{MCP_CLIENTS_FILE_NAME}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let write_res = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(payload.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp);
        return Err(format!("写入 MCP 临时文件失败: {err}"));
    }
    if let Err(err) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("提交 MCP 配置失败: {err}"));
    }
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(data_dir) {
        let _ = dir.sync_all();
    }
    cleanup_stale_mcp_tmp_files(data_dir);
    Ok(())
}

/// 创建或更新一个外部 MCP Server，返回脱敏视图列表。
pub fn save_mcp_server_in_dir(
    data_dir: &Path,
    input: SaveMcpClientServerInput,
) -> Result<Vec<McpClientServerView>, String> {
    let _io_guard = MCP_CLIENTS_IO_LOCK
        .lock()
        .map_err(|_| "MCP 配置锁不可用".to_string())?;
    let id = input.id.trim().to_string();
    if !is_valid_mcp_segment(&id) {
        return Err(
            "MCP 服务标识仅支持 1-64 位字母、数字、连字符或单下划线（禁止双下划线 __）".to_string(),
        );
    }
    let name = sanitize_single_line(&input.name, 80);
    if name.is_empty() {
        return Err("MCP 服务名称不能为空".to_string());
    }
    let endpoint = validate_endpoint_config(&input.endpoint, input.allow_localhost)?;

    let mut servers = load_mcp_servers_from_dir(data_dir);
    if let Some(existing) = servers.iter_mut().find(|s| s.id == id) {
        let endpoint_changed = existing.endpoint != endpoint;
        existing.name = name;
        existing.endpoint = endpoint;
        existing.enabled = input.enabled;
        existing.allow_localhost = input.allow_localhost;
        if let Some(token) = input.auth_token {
            let trimmed = token.trim();
            if trimmed.chars().any(|c| c.is_control()) {
                return Err("认证 Token 不得包含控制字符".to_string());
            }
            existing.auth_token = (!trimmed.is_empty()).then(|| trimmed.to_string());
        } else if endpoint_changed {
            // 凭证与原 Endpoint 绑定：更换地址且未显式输入新 Token 时必须清空旧 Token，防止凭证泄露到新地址。
            existing.auth_token = None;
        }
    } else {
        if servers.len() >= MAX_MCP_SERVERS {
            return Err(format!("最多支持配置 {MAX_MCP_SERVERS} 个外部 MCP 服务"));
        }
        let auth_token = match input.auth_token {
            Some(token) => {
                let trimmed = token.trim();
                if trimmed.chars().any(|c| c.is_control()) {
                    return Err("认证 Token 不得包含控制字符".to_string());
                }
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            None => None,
        };
        servers.push(McpClientServerConfig {
            id,
            name,
            endpoint,
            enabled: input.enabled,
            allow_localhost: input.allow_localhost,
            auth_token,
        });
    }

    write_mcp_servers_atomic(data_dir, &servers)?;
    Ok(servers.iter().map(McpClientServerView::from).collect())
}

/// 删除指定外部 MCP Server，返回脱敏视图列表。
pub fn delete_mcp_server_in_dir(
    data_dir: &Path,
    id: &str,
) -> Result<Vec<McpClientServerView>, String> {
    let _io_guard = MCP_CLIENTS_IO_LOCK
        .lock()
        .map_err(|_| "MCP 配置锁不可用".to_string())?;
    let trimmed = id.trim();
    if !is_valid_mcp_segment(trimmed) {
        return Err("MCP 服务标识非法".to_string());
    }
    let mut servers = load_mcp_servers_from_dir(data_dir);
    let before = servers.len();
    servers.retain(|s| s.id != trimmed);
    if servers.len() == before {
        return Err(format!("未找到 MCP 服务: {trimmed}"));
    }
    write_mcp_servers_atomic(data_dir, &servers)?;
    Ok(servers.iter().map(McpClientServerView::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp工具命名空间解析应双射无碰撞并校验jsonrpc信封id() {
        let q = build_qualified_mcp_tool_name("notion", "search_pages").unwrap();
        assert_eq!(q, "mcp__notion__search_pages");
        assert_eq!(
            parse_qualified_mcp_tool_name(&q),
            Some(("notion", "search_pages"))
        );
        // 禁止双下划线与首尾下划线，彻底消除 ("a__b","c") / ("foo_","bar") 跨服务碰撞
        assert!(build_qualified_mcp_tool_name("a__b", "c").is_none());
        assert!(build_qualified_mcp_tool_name("a", "b__c").is_none());
        assert!(build_qualified_mcp_tool_name("foo_", "bar").is_none());
        assert!(build_qualified_mcp_tool_name("foo", "_bar").is_none());
        assert!(parse_qualified_mcp_tool_name("mcp__a__b__c").is_none());
        assert!(parse_qualified_mcp_tool_name("mcp__foo___bar").is_none());
        assert!(build_qualified_mcp_tool_name("../bad", "x").is_none());
        assert!(parse_qualified_mcp_tool_name("search_memory").is_none());

        let resp = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [
                    { "name": "search_pages", "description": "搜索页面\n注入", "inputSchema": { "type": "object" } },
                    { "name": "bad__tool", "description": "double underscore" },
                    { "name": "../evil", "description": "bad" }
                ]
            }
        });
        // id 不匹配必须拒绝
        assert!(parse_mcp_tools_list_response("notion", "Notion", 99, &resp).is_err());

        let tools = parse_mcp_tools_list_response("notion", "Notion", 2, &resp).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].qualified_name, "mcp__notion__search_pages");
        assert!(!tools[0].description.contains('\n'));

        let call_ok = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "result": { "content": [{ "type": "text", "text": "hello mcp" }], "isError": false }
        });
        assert!(parse_mcp_tools_call_response(4, &call_ok).is_err());
        assert_eq!(
            parse_mcp_tools_call_response(3, &call_ok).unwrap(),
            "hello mcp"
        );
    }

    #[test]
    fn mcp服务配置持久化应隐藏明文token并拦截私网与非法endpoint() {
        let root = std::env::temp_dir().join(format!("wr-mcp-client-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();

        for (endpoint, allow_localhost) in [
            ("file:///etc/passwd", false),
            ("https://user:pass@example.com/mcp", false),
            ("http://mcp.example.com/rpc", false),
            ("http://8.8.8.8/rpc", false),
            ("http://169.254.169.254/latest", true),
            ("http://192.168.1.1/mcp", true),
            ("http://2130706433/mcp", true),
            ("http://[::ffff:127.0.0.1]:8080/mcp", false),
            ("http://127.0.0.1:27124/mcp", false),
            ("http://localhost./mcp", false),
        ] {
            assert!(
                save_mcp_server_in_dir(
                    &root,
                    SaveMcpClientServerInput {
                        id: "bad".into(),
                        name: "Bad".into(),
                        endpoint: endpoint.into(),
                        enabled: true,
                        allow_localhost,
                        auth_token: None,
                    }
                )
                .is_err(),
                "应拒绝非法地址: {endpoint} (allow_localhost={allow_localhost})"
            );
        }

        let views = save_mcp_server_in_dir(
            &root,
            SaveMcpClientServerInput {
                id: "obsidian".into(),
                name: "Obsidian Bridge".into(),
                endpoint: "http://127.0.0.1:27124/mcp".into(),
                enabled: true,
                allow_localhost: true,
                auth_token: Some("secret-token-123".into()),
            },
        )
        .expect("保存合法 MCP 配置应成功");
        assert_eq!(views.len(), 1);
        assert!(views[0].has_auth_token);

        // 同一地址更新且传 auth_token: None 应保留已有 Token
        save_mcp_server_in_dir(
            &root,
            SaveMcpClientServerInput {
                id: "obsidian".into(),
                name: "Obsidian Local".into(),
                endpoint: "http://127.0.0.1:27124/mcp".into(),
                enabled: false,
                allow_localhost: true,
                auth_token: None,
            },
        )
        .expect("同地址更新应成功");
        let full = load_mcp_servers_from_dir(&root);
        assert_eq!(full[0].auth_token.as_deref(), Some("secret-token-123"));
        assert!(!full[0].enabled);

        // 更换 endpoint 且未提供新 Token 时必须清空旧 Token（防跨主机泄露）
        let changed_views = save_mcp_server_in_dir(
            &root,
            SaveMcpClientServerInput {
                id: "obsidian".into(),
                name: "Obsidian Remote".into(),
                endpoint: "https://mcp.example.com/rpc".into(),
                enabled: true,
                allow_localhost: false,
                auth_token: None,
            },
        )
        .expect("更换地址更新应成功");
        assert!(!changed_views[0].has_auth_token);
        let full_after_move = load_mcp_servers_from_dir(&root);
        assert!(full_after_move[0].auth_token.is_none());

        let left = delete_mcp_server_in_dir(&root, "obsidian").expect("删除应成功");
        assert!(left.is_empty());

        let _ = fs::remove_dir_all(&root);
    }
}
