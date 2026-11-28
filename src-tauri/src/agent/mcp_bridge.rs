//! 外部 HTTP MCP Client 运行时桥接（Streamable HTTP / JSON-RPC 2.0）。
//!
//! 安全不变量：
//! - 复用 `tools::ensure_public_http_url` 与 `tools::is_non_public_ip` 进行 SSRF 防护，并先对
//!   IPv4-mapped IPv6（如 `::ffff:127.0.0.1`）做规范化，避免绕过回环判定与自调用拦截。
//! - 公网 MCP 端点强制要求 `https://`（明文 `http://` 仅允许显式开启 `allow_localhost` 的本机回环）。
//! - 每次请求前预解析 DNS 并校验全部解析 IP，通过 `Client::builder().no_proxy().resolve_to_addrs(...)`
//!   强制直连已验证的 `SocketAddr`，彻底消除系统代理绕过与 DNS-rebinding TOCTOU 窗口。
//! - 完整实现 MCP Streamable HTTP 握手：`initialize`（采集 `Mcp-Session-Id` 与协商协议版本）
//!   → `notifications/initialized` → `tools/list` → 后续 `tools/call` 沿用会话。
//! - 工具调用直接按注册时保存的 `(server_id, remote_name)` 路由表分发，不再二次反解字符串。

use super::tools::{ensure_public_http_url, is_non_public_ip};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use work_review_agent_harness::mcp_client::{
    build_mcp_initialize_request, build_mcp_initialized_notification, build_mcp_tools_call_request,
    build_mcp_tools_list_request, load_mcp_servers_from_dir, normalize_ip,
    parse_mcp_tools_call_response, parse_mcp_tools_list_response,
    validate_jsonrpc_response_envelope, DiscoveredMcpTool, McpClientServerConfig,
    MCP_PROTOCOL_VERSION,
};

const MCP_HTTP_TIMEOUT_SECS: u64 = 12;
const MCP_DISCOVERY_TOTAL_BUDGET_SECS: u64 = 8;
const MCP_DISCOVERY_CACHE_TTL_SECS: u64 = 60;
const MCP_DISCOVERY_EMPTY_CACHE_TTL_SECS: u64 = 20;
const MCP_DISCOVERY_CACHE_MAX_ENTRIES: usize = 16;
const MCP_MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";
const MCP_PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

struct McpDiscoveryCacheEntry {
    /// `(blocked_self_port, enabled_servers)` 的 SHA-256 指纹：避免在缓存键中滞留明文 auth_token。
    config_fingerprint: [u8; 32],
    bridge: Option<Arc<McpToolBridge>>,
    expires_at: Instant,
}

static MCP_DISCOVERY_CACHE: OnceLock<Mutex<HashMap<PathBuf, McpDiscoveryCacheEntry>>> =
    OnceLock::new();

fn mcp_discovery_cache() -> &'static Mutex<HashMap<PathBuf, McpDiscoveryCacheEntry>> {
    MCP_DISCOVERY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn compute_discovery_fingerprint(
    blocked_self_port: Option<u16>,
    enabled_servers: &[McpClientServerConfig],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    match blocked_self_port {
        Some(port) => {
            hasher.update([1u8]);
            hasher.update(port.to_le_bytes());
        }
        None => hasher.update([0u8]),
    }
    hasher.update((enabled_servers.len() as u64).to_le_bytes());
    for server in enabled_servers {
        for field in [&server.id, &server.name, &server.endpoint] {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        hasher.update([u8::from(server.enabled), u8::from(server.allow_localhost)]);
        match &server.auth_token {
            Some(tok) => {
                hasher.update([1u8]);
                hasher.update((tok.len() as u64).to_le_bytes());
                hasher.update(tok.as_bytes());
            }
            None => hasher.update([0u8]),
        }
    }
    hasher.finalize().into()
}

/// 主动失效指定数据目录的外部 MCP 发现缓存（保存/删除/测试 MCP 配置后调用）。
pub fn invalidate_mcp_discovery_cache(data_dir: &Path) {
    if let Ok(mut guard) = mcp_discovery_cache().lock() {
        guard.remove(data_dir);
    }
}

/// 测试外部 MCP Server 连通性与工具发现的返回结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerTestResult {
    pub server_id: String,
    pub ok: bool,
    pub tool_count: usize,
    pub tool_names: Vec<String>,
    pub message: String,
}

struct McpServerSessionState {
    config: McpClientServerConfig,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<String>,
}

/// 绑定到单次桌面会话的外部 MCP 工具执行桥。
pub struct McpToolBridge {
    servers: HashMap<String, McpServerSessionState>,
    /// 注册时固化的工具路由表：`qualified_name -> (server_id, remote_name)`，避免运行时反解字符串。
    tool_routes: HashMap<String, (String, String)>,
    discovered_tools: Vec<DiscoveredMcpTool>,
    blocked_self_port: Option<u16>,
    next_rpc_id: AtomicU64,
}

/// 静态校验 MCP Endpoint 的主机、协议、内联凭证、回环权限与自调用端口（同时规范化 IPv4-mapped IPv6）。
///
/// 返回 `(host_for_dns, port, is_loopback_target)` 供后续 DNS 固定解析使用。
pub fn validate_mcp_endpoint_target(
    endpoint: &str,
    allow_localhost: bool,
    blocked_self_port: Option<u16>,
) -> Result<(String, u16, bool), String> {
    let parsed = reqwest::Url::parse(endpoint.trim())
        .map_err(|_| "MCP Endpoint URL 格式无效".to_string())?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("MCP Endpoint 仅支持 http:// 或 https://".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("MCP Endpoint 禁止包含内联账号密码".to_string());
    }
    let Some(raw_host) = parsed.host_str() else {
        return Err("MCP Endpoint 缺少主机名".to_string());
    };
    let host_unbracketed = raw_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host_unbracketed.is_empty() {
        return Err("MCP Endpoint 缺少主机名".to_string());
    }
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| "MCP Endpoint 缺少有效端口".to_string())?;

    let is_loopback_host = host_unbracketed == "localhost"
        || host_unbracketed.ends_with(".localhost")
        || host_unbracketed
            .parse::<IpAddr>()
            .map(normalize_ip)
            .is_ok_and(|ip| ip.is_loopback());

    if is_loopback_host {
        if !allow_localhost {
            return Err("未开启“允许本机回环”时禁止连接 localhost/127.0.0.1".to_string());
        }
        if let Some(self_port) = blocked_self_port {
            if port == self_port {
                return Err("禁止外部 MCP Client 回调 Work Review 自身本地 API 端口".to_string());
            }
        }
        return Ok((host_unbracketed, port, true));
    }

    if scheme != "https" {
        return Err("公网 MCP Endpoint 必须使用 https://，明文 http:// 仅允许本机回环".to_string());
    }
    ensure_public_http_url(endpoint)?;
    Ok((host_unbracketed, port, false))
}

/// 预解析目标域名并校验每一个解析出的 IP，返回强制直连（`no_proxy`）且固定 `SocketAddr` 的客户端。
async fn build_pinned_mcp_client(
    endpoint: &str,
    allow_localhost: bool,
    blocked_self_port: Option<u16>,
) -> Result<Client, String> {
    let (host, port, is_loopback_target) =
        validate_mcp_endpoint_target(endpoint, allow_localhost, blocked_self_port)?;

    let validated_addrs: Vec<SocketAddr> = if let Ok(literal_ip) = host.parse::<IpAddr>() {
        let norm = normalize_ip(literal_ip);
        if is_loopback_target {
            if !norm.is_loopback() {
                return Err("回环目标解析结果异常".to_string());
            }
        } else if is_non_public_ip(norm) {
            return Err("禁止访问本机或内网私网 IP".to_string());
        }
        vec![SocketAddr::new(norm, port)]
    } else {
        let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| format!("MCP 域名解析失败: {e}"))?
            .collect();
        if resolved.is_empty() {
            return Err("MCP 域名解析无结果".to_string());
        }
        let mut addrs = Vec::with_capacity(resolved.len());
        for addr in resolved {
            let norm = normalize_ip(addr.ip());
            if is_loopback_target {
                if !norm.is_loopback() {
                    return Err("本机回环域名解析到了非回环地址，已拒绝连接".to_string());
                }
            } else if is_non_public_ip(norm) {
                return Err("目标 MCP 域名解析到本机/内网地址，已拒绝连接".to_string());
            }
            addrs.push(SocketAddr::new(norm, port));
        }
        addrs
    };

    // 统一禁用系统代理并固定解析地址：防止代理绕过预解析 IP 校验（DNS-rebinding / 内网穿透防护）。
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(MCP_HTTP_TIMEOUT_SECS))
        .resolve_to_addrs(&host, &validated_addrs)
        .build()
        .map_err(|e| format!("初始化固定解析 MCP HTTP 客户端失败: {e}"))
}

/// 解析 MCP HTTP 响应体：兼容 `application/json` 与多行 `text/event-stream` SSE 事件帧，
/// 并严格按 `jsonrpc == "2.0"` 与 `expected_id` 精确匹配响应，忽略无关通知帧。
pub(crate) fn parse_mcp_http_payload(raw_bytes: &[u8], expected_id: u64) -> Result<Value, String> {
    let text = std::str::from_utf8(raw_bytes)
        .map_err(|_| "MCP 服务端返回了非 UTF-8 响应".to_string())?
        .trim();
    if text.is_empty() {
        return Err("MCP 服务端返回了空响应".to_string());
    }
    if let Ok(val) = serde_json::from_str::<Value>(text) {
        validate_jsonrpc_response_envelope(&val, expected_id)?;
        return Ok(val);
    }

    // 兼容 Streamable HTTP SSE：按空行切分事件块，支持单个事件内多行 `data:` 拼接
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut matched: Option<Value> = None;
    for block in normalized.split("\n\n") {
        let mut data_lines = Vec::new();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        if data_lines.is_empty() {
            continue;
        }
        let joined = data_lines.join("\n");
        let Ok(val) = serde_json::from_str::<Value>(joined.trim()) else {
            continue;
        };
        if validate_jsonrpc_response_envelope(&val, expected_id).is_ok() {
            // 若同 id 出现 error 帧，立即优先返回 error 防止被后续帧遮蔽
            if val.get("error").is_some() {
                return Ok(val);
            }
            matched = Some(val);
        }
    }
    matched
        .ok_or_else(|| format!("未在 MCP 响应中找到匹配请求 id({expected_id}) 的 JSON-RPC 2.0 帧"))
}

fn sanitize_header_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 256
        || !trimmed.is_ascii()
        || trimmed.chars().any(|c| c.is_control())
    {
        return None;
    }
    Some(trimmed.to_string())
}

struct RawMcpHttpResponse {
    status: StatusCode,
    session_id: Option<String>,
    body: Vec<u8>,
}

async fn post_mcp_raw(
    server: &McpClientServerConfig,
    blocked_self_port: Option<u16>,
    session_id: Option<&str>,
    protocol_version: Option<&str>,
    request_body: &Value,
) -> Result<RawMcpHttpResponse, String> {
    let client =
        build_pinned_mcp_client(&server.endpoint, server.allow_localhost, blocked_self_port)
            .await?;

    let mut req = client
        .post(&server.endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(request_body);

    if let Some(ver) = protocol_version.and_then(sanitize_header_value) {
        req = req.header(MCP_PROTOCOL_VERSION_HEADER, ver);
    }
    if let Some(sid) = session_id.and_then(sanitize_header_value) {
        req = req.header(MCP_SESSION_ID_HEADER, sid);
    }
    if let Some(token) = server
        .auth_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        req = req.bearer_auth(token);
    }

    let mut resp = req
        .send()
        .await
        .map_err(|e| format!("连接 MCP 服务 {} 失败: {e}", server.name))?;

    let status = resp.status();
    let returned_session_id = resp
        .headers()
        .get(MCP_SESSION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(sanitize_header_value);

    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("text/event-stream")
            })
        });
    let expected_id = request_body.get("id").and_then(Value::as_u64);
    let mut buf = Vec::new();
    let mut frame_start = 0;
    'read: while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("读取 MCP 响应流失败: {e}"))?
    {
        if buf.len().saturating_add(chunk.len()) > MCP_MAX_RESPONSE_BYTES {
            return Err("MCP 响应体超过 256 KiB 安全上限".to_string());
        }
        let old_len = buf.len();
        buf.extend_from_slice(&chunk);
        if let Some(expected_id) = expected_id.filter(|_| is_sse && status.is_success()) {
            // 只扫描新增字节；完整匹配帧到达即返回，不等待 SSE 连接结束。
            for end in old_len + 1..=buf.len() {
                let frame = &buf[frame_start..end];
                if frame.ends_with(b"\n\n")
                    || frame.ends_with(b"\r\n\r\n")
                    || frame.ends_with(b"\r\r")
                {
                    if parse_mcp_http_payload(frame, expected_id).is_ok() {
                        buf.truncate(end);
                        break 'read;
                    }
                    frame_start = end;
                }
            }
        }
    }

    Ok(RawMcpHttpResponse {
        status,
        session_id: returned_session_id,
        body: buf,
    })
}

async fn post_mcp_rpc(
    server: &McpClientServerConfig,
    blocked_self_port: Option<u16>,
    session_id: Option<&str>,
    protocol_version: Option<&str>,
    expected_id: u64,
    request_body: &Value,
) -> Result<(Option<String>, Value), String> {
    let raw = post_mcp_raw(
        server,
        blocked_self_port,
        session_id,
        protocol_version,
        request_body,
    )
    .await?;

    if !raw.status.is_success() {
        return Err(format!(
            "MCP 服务 {} 返回 HTTP 状态码 {}",
            server.name,
            raw.status.as_u16()
        ));
    }

    let payload = parse_mcp_http_payload(&raw.body, expected_id)?;
    Ok((raw.session_id, payload))
}

/// 完成标准 Streamable HTTP MCP 握手：`initialize` → 采集 `Mcp-Session-Id` → 发送 `notifications/initialized`。
async fn handshake_mcp_session(
    server: &McpClientServerConfig,
    blocked_self_port: Option<u16>,
) -> Result<(Option<String>, String), String> {
    let init_id = 1u64;
    let init_req = build_mcp_initialize_request(init_id);
    let (session_id, init_resp) = post_mcp_rpc(
        server,
        blocked_self_port,
        None,
        Some(MCP_PROTOCOL_VERSION),
        init_id,
        &init_req,
    )
    .await?;
    if let Some(err) = init_resp.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("initialize 失败");
        return Err(format!("MCP initialize 返回错误: {msg}"));
    }
    let negotiated_version = init_resp
        .get("result")
        .and_then(|r| r.get("protocolVersion"))
        .and_then(Value::as_str)
        .and_then(sanitize_header_value)
        .unwrap_or_else(|| MCP_PROTOCOL_VERSION.to_string());

    // Streamable HTTP 规范要求客户端在 initialize 成功后发送 notifications/initialized 通知。
    let notif = build_mcp_initialized_notification();
    let notif_raw = post_mcp_raw(
        server,
        blocked_self_port,
        session_id.as_deref(),
        Some(&negotiated_version),
        &notif,
    )
    .await?;
    if !notif_raw.status.is_success() {
        return Err(format!(
            "MCP notifications/initialized 返回 HTTP 状态码 {}",
            notif_raw.status.as_u16()
        ));
    }
    let effective_session = notif_raw.session_id.or(session_id);
    Ok((effective_session, negotiated_version))
}

async fn discover_server_tools(
    server: &McpClientServerConfig,
    blocked_self_port: Option<u16>,
) -> Result<(Option<String>, String, Vec<DiscoveredMcpTool>), String> {
    let (session_id, protocol_version) = handshake_mcp_session(server, blocked_self_port).await?;

    let list_id = 2u64;
    let list_req = build_mcp_tools_list_request(list_id);
    let (updated_sid, list_resp) = post_mcp_rpc(
        server,
        blocked_self_port,
        session_id.as_deref(),
        Some(&protocol_version),
        list_id,
        &list_req,
    )
    .await?;
    let tools = parse_mcp_tools_list_response(&server.id, &server.name, list_id, &list_resp)?;
    Ok((updated_sid.or(session_id), protocol_version, tools))
}

/// 测试单个已配置 MCP Server 的连通性与工具列表（供前端“测试连接”按钮使用）。
pub async fn test_mcp_server_connection(
    server: &McpClientServerConfig,
    blocked_self_port: Option<u16>,
) -> McpServerTestResult {
    match discover_server_tools(server, blocked_self_port).await {
        Ok((_, _, tools)) => {
            let names: Vec<String> = tools.iter().map(|t| t.remote_name.clone()).collect();
            let count = names.len();
            McpServerTestResult {
                server_id: server.id.clone(),
                ok: true,
                tool_count: count,
                tool_names: names,
                message: format!("连接成功，发现 {count} 个工具"),
            }
        }
        Err(err) => McpServerTestResult {
            server_id: server.id.clone(),
            ok: false,
            tool_count: 0,
            tool_names: Vec::new(),
            message: err,
        },
    }
}

impl McpToolBridge {
    /// 带短时 TTL 缓存的外部 MCP 发现入口（桌面每轮工作复盘使用）。
    ///
    /// - 未启用任何 MCP 服务时零网络开销且不污染缓存；
    /// - 命中缓存时复用已握手的 `Arc<McpToolBridge>` 与 `Mcp-Session-Id`，避免每轮提问重跑握手；
    /// - 配置内容变化或调用 `invalidate_mcp_discovery_cache` 时立即失效。
    pub async fn discover_cached_for_data_dir(
        data_dir: &Path,
        blocked_self_port: Option<u16>,
    ) -> Option<Arc<Self>> {
        let dir = data_dir.to_path_buf();
        let enabled_servers: Vec<McpClientServerConfig> = tokio::task::spawn_blocking(move || {
            load_mcp_servers_from_dir(&dir)
                .into_iter()
                .filter(|s| s.enabled)
                .collect()
        })
        .await
        .ok()?;

        if enabled_servers.is_empty() {
            invalidate_mcp_discovery_cache(data_dir);
            return None;
        }

        let fingerprint = compute_discovery_fingerprint(blocked_self_port, &enabled_servers);
        let now = Instant::now();
        if let Ok(mut guard) = mcp_discovery_cache().lock() {
            if let Some(entry) = guard.get(data_dir) {
                if entry.expires_at > now && entry.config_fingerprint == fingerprint {
                    return entry.bridge.clone();
                }
                // 读路径遇到过期或指纹变化的条目立即移除，避免旧条目滞留内存。
                guard.remove(data_dir);
            }
        }

        let discovered = Self::discover_for_servers(enabled_servers, blocked_self_port)
            .await
            .map(Arc::new);
        let ttl = if discovered.is_some() {
            Duration::from_secs(MCP_DISCOVERY_CACHE_TTL_SECS)
        } else {
            Duration::from_secs(MCP_DISCOVERY_EMPTY_CACHE_TTL_SECS)
        };

        if let Ok(mut guard) = mcp_discovery_cache().lock() {
            let now_store = Instant::now();
            guard.retain(|_, entry| entry.expires_at > now_store);
            if guard.len() >= MCP_DISCOVERY_CACHE_MAX_ENTRIES && !guard.contains_key(data_dir) {
                if let Some(oldest_key) = guard
                    .iter()
                    .min_by_key(|(_, entry)| entry.expires_at)
                    .map(|(k, _)| k.clone())
                {
                    guard.remove(&oldest_key);
                }
            }
            guard.insert(
                data_dir.to_path_buf(),
                McpDiscoveryCacheEntry {
                    config_fingerprint: fingerprint,
                    bridge: discovered.clone(),
                    expires_at: now_store + ttl,
                },
            );
        }

        discovered
    }

    /// 按数据目录并行发现所有已启用外部 MCP Server 的工具（总预算上限 8 秒）。
    /// 若未配置任何已启用服务，直接返回 `None`（零网络开销）。
    #[cfg(test)]
    pub async fn discover_for_data_dir(
        data_dir: &Path,
        blocked_self_port: Option<u16>,
    ) -> Option<Self> {
        let dir = data_dir.to_path_buf();
        let enabled_servers: Vec<McpClientServerConfig> = tokio::task::spawn_blocking(move || {
            load_mcp_servers_from_dir(&dir)
                .into_iter()
                .filter(|s| s.enabled)
                .collect()
        })
        .await
        .ok()?;

        if enabled_servers.is_empty() {
            return None;
        }

        Self::discover_for_servers(enabled_servers, blocked_self_port).await
    }

    async fn discover_for_servers(
        enabled_servers: Vec<McpClientServerConfig>,
        blocked_self_port: Option<u16>,
    ) -> Option<Self> {
        let mut join_set = tokio::task::JoinSet::new();
        for server in enabled_servers {
            join_set.spawn(async move {
                let res = discover_server_tools(&server, blocked_self_port).await;
                (server, res)
            });
        }

        let mut servers = HashMap::new();
        let mut tool_routes = HashMap::new();
        let mut discovered_tools = Vec::new();
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(MCP_DISCOVERY_TOTAL_BUDGET_SECS);

        while let Ok(Some(joined)) = tokio::time::timeout_at(deadline, join_set.join_next()).await {
            if let Ok((server, res)) = joined {
                match res {
                    Ok((session_id, protocol_version, tools)) if !tools.is_empty() => {
                        for tool in &tools {
                            tool_routes.insert(
                                tool.qualified_name.clone(),
                                (tool.server_id.clone(), tool.remote_name.clone()),
                            );
                        }
                        discovered_tools.extend(tools);
                        servers.insert(
                            server.id.clone(),
                            McpServerSessionState {
                                config: server,
                                session_id: Mutex::new(session_id),
                                protocol_version: Mutex::new(protocol_version),
                            },
                        );
                    }
                    Ok(_) => {
                        log::info!("外部 MCP 服务 {} 未返回任何可用工具", server.id);
                    }
                    Err(err) => {
                        log::warn!("外部 MCP 服务 {} 发现失败（已跳过）: {err}", server.id);
                    }
                }
            }
        }
        join_set.abort_all();

        if discovered_tools.is_empty() {
            return None;
        }
        Some(Self {
            servers,
            tool_routes,
            discovered_tools,
            blocked_self_port,
            next_rpc_id: AtomicU64::new(100),
        })
    }

    pub fn discovered_tools(&self) -> &[DiscoveredMcpTool] {
        &self.discovered_tools
    }

    /// 执行单个已注册的外部 MCP 工具（直接查注册表路由并沿用/续期 `Mcp-Session-Id`）。
    pub async fn call_qualified_tool(
        &self,
        qualified_name: &str,
        arguments: Value,
    ) -> Result<String, String> {
        let Some((server_id, remote_name)) = self.tool_routes.get(qualified_name) else {
            return Err("未在当前会话注册表中找到该外部 MCP 工具".to_string());
        };
        let Some(server_state) = self.servers.get(server_id) else {
            return Err(format!("未找到已启用的外部 MCP 服务: {server_id}"));
        };

        let (current_sid, current_ver) = {
            let sid = server_state
                .session_id
                .lock()
                .map_err(|_| "MCP 会话状态锁不可用".to_string())?
                .clone();
            let ver = server_state
                .protocol_version
                .lock()
                .map_err(|_| "MCP 协议版本锁不可用".to_string())?
                .clone();
            (sid, ver)
        };

        let rpc_id = self.next_rpc_id.fetch_add(1, Ordering::Relaxed);
        let req = build_mcp_tools_call_request(rpc_id, remote_name, arguments.clone());
        let raw = post_mcp_raw(
            &server_state.config,
            self.blocked_self_port,
            current_sid.as_deref(),
            Some(&current_ver),
            &req,
        )
        .await?;

        // 若服务端返回 HTTP 404（会话过期或服务端刚切换为有状态），尝试单次重新握手并重发 tools/call
        let final_raw = if raw.status == StatusCode::NOT_FOUND {
            let (new_sid, new_ver) =
                handshake_mcp_session(&server_state.config, self.blocked_self_port).await?;
            if let Ok(mut guard) = server_state.session_id.lock() {
                *guard = new_sid.clone();
            }
            if let Ok(mut guard) = server_state.protocol_version.lock() {
                *guard = new_ver.clone();
            }
            let retry_id = self.next_rpc_id.fetch_add(1, Ordering::Relaxed);
            let retry_req = build_mcp_tools_call_request(retry_id, remote_name, arguments);
            let retry_raw = post_mcp_raw(
                &server_state.config,
                self.blocked_self_port,
                new_sid.as_deref(),
                Some(&new_ver),
                &retry_req,
            )
            .await?;
            if !retry_raw.status.is_success() {
                return Err(format!(
                    "MCP 服务 {} 重建会话后仍返回 HTTP 状态码 {}",
                    server_state.config.name,
                    retry_raw.status.as_u16()
                ));
            }
            if let Some(rotated) = retry_raw.session_id {
                if let Ok(mut guard) = server_state.session_id.lock() {
                    *guard = Some(rotated);
                }
            }
            let payload = parse_mcp_http_payload(&retry_raw.body, retry_id)?;
            return parse_mcp_tools_call_response(retry_id, &payload);
        } else {
            raw
        };

        if !final_raw.status.is_success() {
            return Err(format!(
                "MCP 服务 {} 返回 HTTP 状态码 {}",
                server_state.config.name,
                final_raw.status.as_u16()
            ));
        }
        if let Some(rotated) = final_raw.session_id {
            if let Ok(mut guard) = server_state.session_id.lock() {
                *guard = Some(rotated);
            }
        }
        let payload = parse_mcp_http_payload(&final_raw.body, rpc_id)?;
        parse_mcp_tools_call_response(rpc_id, &payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn mcp目标地址校验应拦截未授权回环公网明文http与mapped_ipv6自调用() {
        // 默认禁止 localhost 与 IPv4-mapped IPv6 回环
        assert!(
            validate_mcp_endpoint_target("http://127.0.0.1:27124/mcp", false, Some(47831)).is_err()
        );
        assert!(validate_mcp_endpoint_target(
            "http://[::ffff:127.0.0.1]:27124/mcp",
            false,
            Some(47831)
        )
        .is_err());
        // 公网明文 http 必须拒绝（防止 Bearer Token 与数据明文传输）
        assert!(
            validate_mcp_endpoint_target("http://mcp.example.com/v1", false, Some(47831)).is_err()
        );
        // 开启 allow_localhost 后放行其他回环端口，但仍拦截 Work Review 自身端口（含 IPv4-mapped IPv6）
        assert!(
            validate_mcp_endpoint_target("http://127.0.0.1:27124/mcp", true, Some(47831)).is_ok()
        );
        assert!(
            validate_mcp_endpoint_target("http://127.0.0.1:47831/mcp", true, Some(47831)).is_err()
        );
        assert!(validate_mcp_endpoint_target(
            "http://[::ffff:127.0.0.1]:47831/mcp",
            true,
            Some(47831)
        )
        .is_err());
        assert!(
            validate_mcp_endpoint_target("http://localhost:47831/mcp", true, Some(47831)).is_err()
        );
        // 即使开启 allow_localhost 也绝不放行私网/未指定/云元数据
        assert!(
            validate_mcp_endpoint_target("http://0.0.0.0:8080/mcp", true, Some(47831)).is_err()
        );
        assert!(
            validate_mcp_endpoint_target("http://169.254.169.254/latest", true, Some(47831))
                .is_err()
        );
        assert!(
            validate_mcp_endpoint_target("http://192.168.1.10:8080/mcp", true, Some(47831))
                .is_err()
        );
        // 公网 HTTPS 放行
        assert!(
            validate_mcp_endpoint_target("https://mcp.example.com/v1", false, Some(47831)).is_ok()
        );
    }

    #[test]
    fn mcp响应解析应严格匹配jsonrpc_id并忽略通知帧且支持多行sse() {
        let json_bytes = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        assert!(parse_mcp_http_payload(json_bytes, 1).is_ok());
        assert!(parse_mcp_http_payload(json_bytes, 2).is_err());

        // 包含无关通知帧与多行 data: 拼接的有效响应
        let sse_bytes = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"result\":\"noise\"}}\n\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\n",
            "data: \"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n\n"
        )
        .as_bytes();
        let parsed = parse_mcp_http_payload(sse_bytes, 7).expect("应匹配 id=7 的多行 SSE 帧");
        assert_eq!(parse_mcp_tools_call_response(7, &parsed).unwrap(), "ok");
        assert!(parse_mcp_http_payload(sse_bytes, 8).is_err());

        // 同 id 的 error 帧应优先返回并被识别为 Err
        let err_sse = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"id\":9,\"error\":{\"code\":-32000,\"message\":\"denied\"}}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"late\"}]}}\n\n"
        )
        .as_bytes();
        let err_parsed = parse_mcp_http_payload(err_sse, 9).expect("应解析 error 帧");
        assert!(parse_mcp_tools_call_response(9, &err_parsed).is_err());
    }

    #[tokio::test]
    async fn mcp_sse完整匹配帧应在连接保持打开时立即返回且保留大小限制() {
        for (ending, oversized) in [("\n", false), ("\r\n", false), ("\r", false), ("\n", true)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
            let (close_tx, close_rx) = tokio::sync::oneshot::channel::<()>();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") {
                    headers.push(socket.read_u8().await.unwrap());
                    assert!(headers.len() < 8192);
                }
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\nMcp-Session-Id: open-session\r\n\r\n").await.unwrap();
                let payload = if oversized {
                    "x".repeat(MCP_MAX_RESPONSE_BYTES + 1)
                } else {
                    format!(
                        "data: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}{ending}{ending}\
                         data: {{\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{{}}}}{ending}{ending}\
                         data: {{\"jsonrpc\":\"2.0\",\"id\":7,{ending}\
                         data: \"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"成功\"}}]}}}}{ending}{ending}"
                    )
                };
                // 分块切断 UTF-8 和事件结束符，且不发送 HTTP chunked 终止块。
                for bytes in payload.as_bytes().chunks(if oversized { 8192 } else { 7 }) {
                    let prefix = format!("{:x}\r\n", bytes.len());
                    if socket.write_all(prefix.as_bytes()).await.is_err()
                        || socket.write_all(bytes).await.is_err()
                        || socket.write_all(b"\r\n").await.is_err()
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                let _ = close_rx.await;
            });
            let config = McpClientServerConfig {
                id: "open-sse".into(),
                name: "Open SSE".into(),
                endpoint,
                enabled: true,
                allow_localhost: true,
                auth_token: None,
            };
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                post_mcp_rpc(
                    &config,
                    None,
                    None,
                    Some(MCP_PROTOCOL_VERSION),
                    7,
                    &serde_json::json!({"jsonrpc":"2.0", "id":7, "method":"tools/call"}),
                ),
            )
            .await;
            let _ = close_tx.send(());
            server.await.unwrap();
            let result = result.expect("完整 SSE 帧不得等待连接关闭");
            if oversized {
                assert!(result.unwrap_err().contains("256 KiB"));
            } else {
                let (session, payload) = result.unwrap();
                assert_eq!(session.as_deref(), Some("open-session"));
                assert_eq!(parse_mcp_tools_call_response(7, &payload).unwrap(), "成功");
            }
        }
    }

    #[tokio::test]
    async fn 带会话的streamable_http_mcp服务应完成完整握手并沿用session_id() {
        use std::sync::Arc;
        use work_review_agent_harness::mcp_client::{
            save_mcp_server_in_dir, SaveMcpClientServerInput,
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定本地模拟 MCP 监听端口失败");
        let port = listener.local_addr().unwrap().port();
        let initialized_seen = Arc::new(AtomicBool::new(false));
        let initialized_flag = initialized_seen.clone();

        let server_task = tokio::spawn(async move {
            // 处理 4 次请求：initialize -> notifications/initialized -> tools/list -> tools/call
            for _ in 0..4 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let head_str = String::from_utf8_lossy(&raw[..pos]);
                            for line in head_str.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(end) = header_end {
                        if raw.len() >= end + content_len {
                            break;
                        }
                    }
                }
                let Some(end) = header_end else {
                    continue;
                };
                let head_str = String::from_utf8_lossy(&raw[..end]).to_string();
                let body_val: Value =
                    serde_json::from_slice(&raw[end..end + content_len]).unwrap_or(Value::Null);
                let method = body_val.get("method").and_then(Value::as_str).unwrap_or("");
                let id = body_val.get("id").and_then(Value::as_u64);
                let has_session = head_str.lines().any(|l| {
                    l.to_ascii_lowercase().starts_with("mcp-session-id:") && l.contains("sess-42")
                });

                let response_bytes = match method {
                    "initialize" => {
                        let resp_body = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id.unwrap_or(1),
                            "result": {
                                "protocolVersion": MCP_PROTOCOL_VERSION,
                                "capabilities": { "tools": {} },
                                "serverInfo": { "name": "mock-mcp", "version": "1.0" }
                            }
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: sess-42\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            resp_body.len(),
                            resp_body
                        )
                    }
                    "notifications/initialized" if has_session => {
                        initialized_flag.store(true, Ordering::SeqCst);
                        "HTTP/1.1 202 Accepted\r\nMcp-Session-Id: sess-42\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                    "tools/list" if has_session && initialized_flag.load(Ordering::SeqCst) => {
                        let resp_body = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id.unwrap_or(2),
                            "result": {
                                "tools": [{
                                    "name": "ping_note",
                                    "description": "Echo test",
                                    "inputSchema": { "type": "object" }
                                }]
                            }
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: sess-42\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            resp_body.len(),
                            resp_body
                        )
                    }
                    "tools/call" if has_session && initialized_flag.load(Ordering::SeqCst) => {
                        let resp_body = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id.unwrap_or(100),
                            "result": {
                                "content": [{ "type": "text", "text": "session-pong" }],
                                "isError": false
                            }
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: sess-42\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            resp_body.len(),
                            resp_body
                        )
                    }
                    _ => {
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                };
                let _ = socket.write_all(response_bytes.as_bytes()).await;
            }
        });

        let root = std::env::temp_dir().join(format!("wr-mcp-sess-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        save_mcp_server_in_dir(
            &root,
            SaveMcpClientServerInput {
                id: "mock".into(),
                name: "Mock Stateful MCP".into(),
                endpoint: format!("http://127.0.0.1:{port}/mcp"),
                enabled: true,
                allow_localhost: true,
                auth_token: None,
            },
        )
        .expect("保存模拟 MCP 配置失败");

        let bridge = McpToolBridge::discover_for_data_dir(&root, Some(47831))
            .await
            .expect("带会话 MCP 服务应发现成功");
        assert!(initialized_seen.load(Ordering::SeqCst));
        assert_eq!(bridge.discovered_tools().len(), 1);
        assert_eq!(
            bridge.discovered_tools()[0].qualified_name,
            "mcp__mock__ping_note"
        );

        let reply = bridge
            .call_qualified_tool("mcp__mock__ping_note", serde_json::json!({}))
            .await
            .expect("带会话 tools/call 应成功");
        assert_eq!(reply, "session-pong");

        let _ = server_task.await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn mcp发现缓存应复用会话且支持主动失效重建() {
        use work_review_agent_harness::mcp_client::{
            save_mcp_server_in_dir, SaveMcpClientServerInput,
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let init_count = std::sync::Arc::new(AtomicU64::new(0));
        let init_count_server = init_count.clone();

        let server_task = tokio::spawn(async move {
            // 预期共 4 轮握手（首次 + 主动失效 + 端口变更 + 配置变更，每轮 3 个请求）= 12 次连接
            for _ in 0..12 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let mut content_len = 0usize;
                let mut header_end = None;
                loop {
                    let n = socket.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let header_str = String::from_utf8_lossy(&buf[..pos]);
                            for line in header_str.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(h_end) = header_end {
                        if buf.len() >= h_end + content_len {
                            break;
                        }
                    }
                }
                let h_end = header_end.unwrap_or(buf.len());
                let body_str = String::from_utf8_lossy(&buf[h_end..]);
                let req_json: Value = serde_json::from_str(&body_str).unwrap_or(Value::Null);
                let method = req_json
                    .get("method")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let id = req_json.get("id").cloned().unwrap_or(Value::Null);

                let response_bytes = match method {
                    "initialize" => {
                        init_count_server.fetch_add(1, Ordering::SeqCst);
                        let resp_body = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": {} }
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: cache-sess\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            resp_body.len(),
                            resp_body
                        )
                    }
                    "notifications/initialized" => {
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                    "tools/list" => {
                        let resp_body = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "tools": [{
                                    "name": "cached_tool",
                                    "description": "cached",
                                    "inputSchema": { "type": "object", "properties": {} }
                                }]
                            }
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: cache-sess\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            resp_body.len(),
                            resp_body
                        )
                    }
                    _ => {
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                };
                let _ = socket.write_all(response_bytes.as_bytes()).await;
            }
        });

        let root = std::env::temp_dir().join(format!("wr-mcp-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        save_mcp_server_in_dir(
            &root,
            SaveMcpClientServerInput {
                id: "cached".into(),
                name: "Cached MCP".into(),
                endpoint: format!("http://127.0.0.1:{port}/mcp"),
                enabled: true,
                allow_localhost: true,
                auth_token: None,
            },
        )
        .unwrap();

        let first = McpToolBridge::discover_cached_for_data_dir(&root, Some(47831))
            .await
            .expect("首次发现应成功");
        let second = McpToolBridge::discover_cached_for_data_dir(&root, Some(47831))
            .await
            .expect("第二次应命中缓存");
        assert!(
            Arc::ptr_eq(&first, &second),
            "命中缓存应复用同一个 Arc 实例"
        );
        assert_eq!(init_count.load(Ordering::SeqCst), 1, "缓存命中不应重复握手");

        invalidate_mcp_discovery_cache(&root);
        let third = McpToolBridge::discover_cached_for_data_dir(&root, Some(47831))
            .await
            .expect("失效后应重新发现");
        assert!(!Arc::ptr_eq(&first, &third));
        assert_eq!(init_count.load(Ordering::SeqCst), 2, "失效后应重新发起握手");

        // blocked_self_port 变化应自动击穿缓存
        let fourth = McpToolBridge::discover_cached_for_data_dir(&root, Some(47832))
            .await
            .expect("自调用保护端口变更后应重新发现");
        assert!(!Arc::ptr_eq(&third, &fourth));
        assert_eq!(
            init_count.load(Ordering::SeqCst),
            3,
            "端口变化应触发重新握手"
        );

        // 配置内容（如服务名或 token）变更应自动击穿缓存
        save_mcp_server_in_dir(
            &root,
            SaveMcpClientServerInput {
                id: "cached".into(),
                name: "Cached MCP v2".into(),
                endpoint: format!("http://127.0.0.1:{port}/mcp"),
                enabled: true,
                allow_localhost: true,
                auth_token: Some("tok-rotated".into()),
            },
        )
        .unwrap();
        let fifth = McpToolBridge::discover_cached_for_data_dir(&root, Some(47832))
            .await
            .expect("配置变更后应重新发现");
        assert!(!Arc::ptr_eq(&fourth, &fifth));
        assert_eq!(
            init_count.load(Ordering::SeqCst),
            4,
            "配置变化应触发重新握手"
        );

        // 禁用所有服务时应直接返回 None 并清理缓存
        save_mcp_server_in_dir(
            &root,
            SaveMcpClientServerInput {
                id: "cached".into(),
                name: "Cached MCP v2".into(),
                endpoint: format!("http://127.0.0.1:{port}/mcp"),
                enabled: false,
                allow_localhost: true,
                auth_token: None,
            },
        )
        .unwrap();
        assert!(
            McpToolBridge::discover_cached_for_data_dir(&root, Some(47832))
                .await
                .is_none()
        );

        invalidate_mcp_discovery_cache(&root);
        let _ = server_task.await;
        let _ = std::fs::remove_dir_all(&root);
    }
}
