use crate::commands;
use crate::config::DEFAULT_LOCALHOST_API_PORT;
use crate::error::{AppError, Result};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use uuid::Uuid;

pub const LOCALHOST_API_HOST: &str = "127.0.0.1";

pub fn effective_api_host(config_host: Option<&str>) -> String {
    config_host
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(LOCALHOST_API_HOST)
        .to_string()
}
const LOCALHOST_API_TOKEN_FILE: &str = "localhost_api_token.txt";
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_BODY_BYTES: usize = 128 * 1024;

pub struct LocalhostApiRuntime {
    pub running: bool,
    pub bound_host: Option<String>,
    pub bound_port: Option<u16>,
    pub last_error: Option<String>,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
}

impl Default for LocalhostApiRuntime {
    fn default() -> Self {
        Self {
            running: false,
            bound_host: None,
            bound_port: None,
            last_error: None,
            shutdown_tx: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalhostApiStatusPayload {
    pub enabled: bool,
    pub running: bool,
    pub host: String,
    pub port: u16,
    pub base_url: String,
    pub token_configured: bool,
    pub token_preview: Option<String>,
    pub last_error: Option<String>,
    pub recording: bool,
    pub paused: bool,
    pub app_version: String,
    pub platform: String,
    pub device_id: String,
    pub device_name: String,
}

#[derive(Debug, Deserialize)]
struct GenerateReportRequest {
    date: String,
    #[serde(default)]
    force: Option<bool>,
    #[serde(default)]
    locale: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExportReportRequest {
    date: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    export_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestAuthMode {
    None,
    LocalApiToken,
}

#[derive(Debug)]
struct ParsedRequest {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct HttpResponse {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json<T: Serialize>(status: u16, payload: &T) -> Self {
        let reason = reason_phrase(status);
        let body = serde_json::to_vec(payload).unwrap_or_else(|_| {
            serde_json::to_vec(&serde_json::json!({
                "error": "响应序列化失败",
            }))
            .unwrap_or_else(|_| b"{\"error\":\"serialization failed\"}".to_vec())
        });
        Self {
            status,
            reason,
            content_type: "application/json; charset=utf-8",
            body,
        }
    }

    fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(
            status,
            &serde_json::json!({
                "error": message.into(),
            }),
        )
    }

    fn text(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            reason: reason_phrase(status),
            content_type: "text/plain; charset=utf-8",
            body: message.into().into_bytes(),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let headers = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
            self.status,
            self.reason,
            self.content_type,
            self.body.len()
        );
        let mut bytes = headers.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn localhost_api_token_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOCALHOST_API_TOKEN_FILE)
}

fn generate_localhost_api_token() -> String {
    format!("wr-local-{}", Uuid::new_v4().simple())
}

#[cfg(unix)]
fn open_secret_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    options.open(path)
}

#[cfg(not(unix))]
fn open_secret_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.open(path)
}

fn write_localhost_api_token(path: &Path, token: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = open_secret_file(path)?;
    file.write_all(token.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn read_localhost_api_token_from_path(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(path)?;
    let token = content.trim().to_string();
    if token.is_empty() {
        Ok(None)
    } else {
        Ok(Some(token))
    }
}

fn extract_bearer_token(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

fn mask_localhost_api_token(token: &str) -> String {
    if token.len() <= 12 {
        return "已生成".to_string();
    }

    format!("{}…{}", &token[..8], &token[token.len() - 4..])
}

pub fn ensure_localhost_api_token(state: &Arc<Mutex<AppState>>) -> Result<String> {
    let token_path = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        localhost_api_token_path(&state.data_dir)
    };

    if let Some(token) = read_localhost_api_token_from_path(&token_path)? {
        return Ok(token);
    }

    let token = generate_localhost_api_token();
    write_localhost_api_token(&token_path, &token)?;
    Ok(token)
}

pub fn rotate_localhost_api_token(state: &Arc<Mutex<AppState>>) -> Result<String> {
    let token_path = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        localhost_api_token_path(&state.data_dir)
    };

    let token = generate_localhost_api_token();
    write_localhost_api_token(&token_path, &token)?;
    Ok(token)
}

pub fn reveal_localhost_api_token(state: &Arc<Mutex<AppState>>) -> Result<String> {
    ensure_localhost_api_token(state)
}

pub fn get_localhost_api_status(state: &Arc<Mutex<AppState>>) -> Result<LocalhostApiStatusPayload> {
    let node_status = crate::node_gateway::get_node_gateway_status(state)?;
    let (
        config,
        runtime_running,
        runtime_host,
        runtime_port,
        last_error,
        is_recording,
        is_paused,
        data_dir,
    ) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (
            state.config.clone(),
            state.localhost_api_runtime.running,
            state.localhost_api_runtime.bound_host.clone(),
            state.localhost_api_runtime.bound_port,
            state.localhost_api_runtime.last_error.clone(),
            state.is_recording,
            state.is_paused,
            state.data_dir.clone(),
        )
    };

    let token = read_localhost_api_token_from_path(&localhost_api_token_path(&data_dir))?;
    let configured_port = if config.localhost_api_port == 0 {
        DEFAULT_LOCALHOST_API_PORT
    } else {
        config.localhost_api_port
    };
    let port = runtime_port.unwrap_or(configured_port);
    let host = runtime_host.unwrap_or_else(|| effective_api_host(config.localhost_api_host.as_deref()));

    Ok(LocalhostApiStatusPayload {
        enabled: config.localhost_api_enabled,
        running: runtime_running,
        host: host.clone(),
        port,
        base_url: format!("http://{host}:{port}"),
        token_configured: token.is_some(),
        token_preview: token.as_deref().map(mask_localhost_api_token),
        last_error,
        recording: is_recording,
        paused: is_paused,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: runtime_platform().to_string(),
        device_id: node_status.device_id,
        device_name: node_status.device_name,
    })
}

fn runtime_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    return "macos";
    #[cfg(target_os = "windows")]
    return "windows";
    #[cfg(target_os = "linux")]
    return "linux";
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    return "unknown";
}

fn stop_runtime_locked(runtime: &mut LocalhostApiRuntime) -> Option<oneshot::Sender<()>> {
    runtime.running = false;
    runtime.bound_host = None;
    runtime.bound_port = None;
    runtime.shutdown_tx.take()
}

fn record_runtime_error(state: &Arc<Mutex<AppState>>, message: impl Into<String>) {
    if let Ok(mut state) = state.lock() {
        state.localhost_api_runtime.running = false;
        state.localhost_api_runtime.bound_host = None;
        state.localhost_api_runtime.bound_port = None;
        state.localhost_api_runtime.shutdown_tx = None;
        state.localhost_api_runtime.last_error = Some(message.into());
    }
}

fn bind_localhost_api_listener(host: &str, port: u16) -> std::io::Result<StdTcpListener> {
    let mut last_error = None;
    for attempt in 0..5 {
        match StdTcpListener::bind((host, port)) {
            Ok(listener) => return Ok(listener),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse && attempt < 4 => {
                last_error = Some(err);
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_error.expect("bind retry loop records address-in-use error"))
}

pub fn sync_localhost_api_runtime(app: &AppHandle, state: &Arc<Mutex<AppState>>) -> Result<()> {
    let (enabled, host, port, should_restart, shutdown_tx) = {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let enabled = state.config.localhost_api_enabled;
        let host = effective_api_host(state.config.localhost_api_host.as_deref());
        let port = if state.config.localhost_api_port == 0 {
            DEFAULT_LOCALHOST_API_PORT
        } else {
            state.config.localhost_api_port
        };

        if !enabled {
            state.localhost_api_runtime.last_error = None;
            let shutdown_tx = stop_runtime_locked(&mut state.localhost_api_runtime);
            return {
                if let Some(shutdown_tx) = shutdown_tx {
                    let _ = shutdown_tx.send(());
                }
                Ok(())
            };
        }

        if state.localhost_api_runtime.running
            && state.localhost_api_runtime.bound_host.as_deref() == Some(host.as_str())
            && state.localhost_api_runtime.bound_port == Some(port)
        {
            return Ok(());
        }

        let shutdown_tx = stop_runtime_locked(&mut state.localhost_api_runtime);
        (enabled, host, port, true, shutdown_tx)
    };

    if let Some(shutdown_tx) = shutdown_tx {
        let _ = shutdown_tx.send(());
    }

    if !enabled || !should_restart {
        return Ok(());
    }

    let token = ensure_localhost_api_token(state)?;

    let std_listener = bind_localhost_api_listener(host.as_str(), port).map_err(|e| {
        let message = format!("启动本地 API 失败: {e}");
        record_runtime_error(state, &message);
        AppError::Config(message)
    })?;
    std_listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(std_listener)
        .map_err(|e| AppError::Unknown(format!("接管本地 API 监听器失败: {e}")))?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.localhost_api_runtime.running = true;
        state.localhost_api_runtime.bound_host = Some(host.clone());
        state.localhost_api_runtime.bound_port = Some(port);
        state.localhost_api_runtime.last_error = None;
        state.localhost_api_runtime.shutdown_tx = Some(shutdown_tx);
    }

    let app_handle = app.clone();
    let state_handle = state.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) =
            run_localhost_api(listener, shutdown_rx, app_handle, state_handle.clone()).await
        {
            record_runtime_error(&state_handle, format!("本地 API 异常退出: {e}"));
        } else if let Ok(mut state) = state_handle.lock() {
            state.localhost_api_runtime.running = false;
            state.localhost_api_runtime.bound_host = None;
            state.localhost_api_runtime.bound_port = None;
            state.localhost_api_runtime.shutdown_tx = None;
        }
    });

    log::info!(
        "本地 API 已监听在 http://{host}:{port}，token={}",
        mask_localhost_api_token(&token)
    );
    Ok(())
}

async fn run_localhost_api(
    listener: TcpListener,
    mut shutdown_rx: oneshot::Receiver<()>,
    app: AppHandle,
    state: Arc<Mutex<AppState>>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                return Ok(());
            }
            accept_result = listener.accept() => {
                let (stream, _) = accept_result.map_err(|e| AppError::Unknown(format!("接受本地 API 连接失败: {e}")))?;
                let app = app.clone();
                let state = state.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = handle_connection(stream, app, state).await {
                        log::warn!("处理本地 API 请求失败: {e}");
                    }
                });
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    app: AppHandle,
    state: Arc<Mutex<AppState>>,
) -> Result<()> {
    let response = match read_request(&mut stream).await {
        Ok(Some(request)) => route_request(request, &app, &state).await,
        Ok(None) => return Ok(()),
        Err(err) => HttpResponse::error(400, err.to_string()),
    };

    stream.write_all(&response.to_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn read_request(stream: &mut TcpStream) -> Result<Option<ParsedRequest>> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let header_end;

    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(AppError::Config("本地 API 请求头不完整".to_string()));
        }

        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(AppError::Config("本地 API 请求体过大".to_string()));
        }

        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = position + 4;
            break;
        }
    }

    let header_text = String::from_utf8(bytes[..header_end].to_vec())
        .map_err(|_| AppError::Config("本地 API 请求头不是合法 UTF-8".to_string()))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| AppError::Config("本地 API 缺少请求行".to_string()))?;
    let mut request_line_parts = request_line.split_whitespace();
    let method = request_line_parts
        .next()
        .ok_or_else(|| AppError::Config("本地 API 请求方法缺失".to_string()))?
        .to_string();
    let target = request_line_parts
        .next()
        .ok_or_else(|| AppError::Config("本地 API 请求路径缺失".to_string()))?;

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(AppError::Config("本地 API 请求头格式非法".to_string()));
        };
        headers.insert(name.trim().to_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| AppError::Config("本地 API Content-Length 非法".to_string()))?
        .unwrap_or(0);

    if content_length > MAX_BODY_BYTES {
        return Err(AppError::Config("本地 API 请求体超过限制".to_string()));
    }

    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Err(AppError::Config("本地 API 请求体不完整".to_string()));
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(AppError::Config("本地 API 请求体过大".to_string()));
        }
    }

    let body = bytes[header_end..header_end + content_length].to_vec();
    let parsed_url = reqwest::Url::parse(&format!("http://localhost{target}"))
        .map_err(|e| AppError::Config(format!("本地 API 请求路径非法: {e}")))?;
    let query = parsed_url
        .query_pairs()
        .into_owned()
        .collect::<HashMap<_, _>>();

    Ok(Some(ParsedRequest {
        method,
        path: parsed_url.path().to_string(),
        query,
        headers,
        body,
    }))
}

fn handle_device_info(state: &Arc<Mutex<AppState>>) -> Result<HttpResponse> {
    let (is_recording, is_paused, config_host, config_port) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (
            s.is_recording,
            s.is_paused,
            s.config.localhost_api_host.clone(),
            s.config.localhost_api_port,
        )
    };
    let node_status = crate::node_gateway::get_node_gateway_status(state)?;
    let host = effective_api_host(config_host.as_deref());
    let port = if config_port == 0 {
        DEFAULT_LOCALHOST_API_PORT
    } else {
        config_port
    };
    Ok(HttpResponse::json(
        200,
        &serde_json::json!({
            "deviceId": node_status.device_id,
            "deviceName": node_status.device_name,
            "platform": runtime_platform(),
            "appVersion": env!("CARGO_PKG_VERSION"),
            "protocolVersion": node_status.protocol_version,
            "recording": is_recording,
            "paused": is_paused,
            "apiEndpoint": format!("http://{host}:{port}"),
        }),
    ))
}

async fn route_request(
    request: ParsedRequest,
    app: &AppHandle,
    state: &Arc<Mutex<AppState>>,
) -> HttpResponse {
    match authorize_request(&request, state) {
        Ok(()) => {}
        Err(err) => {
            let status = if matches!(err, AppError::Config(_)) {
                401
            } else {
                500
            };
            return HttpResponse::error(status, err.to_string());
        }
    }

    let result = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/reports/generate") => {
            let date = request.query.get("date").cloned().unwrap_or_default();
            let force = request.query.get("force").and_then(|v| v.parse().ok());
            let locale = request.query.get("locale").cloned();
            if date.is_empty() {
                Err(AppError::Config("date 参数不能为空".to_string()))
            } else {
                commands::generate_report_inner(date, force, locale, app, state)
                    .await
                    .map(|content| {
                        HttpResponse::json(200, &serde_json::json!({ "content": content }))
                    })
            }
        }
        ("POST", "/v1/reports/export-markdown") => parse_json_body::<ExportReportRequest>(&request)
            .and_then(|body| {
                commands::export_report_markdown_inner(
                    body.date,
                    body.content,
                    body.export_dir,
                    state,
                )
                .map(|path| {
                    HttpResponse::json(
                        200,
                        &serde_json::json!({
                            "path": path,
                        }),
                    )
                })
            }),
        _ if request.method == "GET" && request.path.starts_with("/v1/reports/") => {
            let date = request.path.trim_start_matches("/v1/reports/").trim();
            if date.is_empty() {
                Err(AppError::Config("日报日期不能为空".to_string()))
            } else {
                commands::get_saved_report_inner(
                    date.to_string(),
                    request.query.get("locale").cloned(),
                    state,
                )
                .and_then(|report| {
                    report.ok_or_else(|| AppError::Config("未找到该日期的日报".to_string()))
                })
                .map(|report| HttpResponse::json(200, &report))
            }
        }
        ("GET", "/v1/reports") => {
            let limit: usize = request
                .query
                .get("limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30)
                .min(100);
            let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()));
            match guard {
                Ok(s) => s
                    .database
                    .list_report_dates(limit)
                    .map(|dates| HttpResponse::json(200, &serde_json::json!({ "dates": dates }))),
                Err(e) => Err(e),
            }
        }
        ("GET", "/v1/stats/today") => {
            commands::get_today_stats_inner(state)
                .map(|stats| HttpResponse::json(200, &stats))
        }
        ("GET", "/v1/stats/overview") => {
            let mode = request.query.get("mode").cloned().unwrap_or_else(|| "today".to_string());
            let date = request.query.get("date").cloned();
            let date_from = request.query.get("date_from").cloned();
            let date_to = request.query.get("date_to").cloned();
            commands::get_overview_stats_inner(mode, date, date_from, date_to, state)
                .map(|stats| HttpResponse::json(200, &stats))
        }
        _ if request.method == "GET" && request.path.starts_with("/v1/stats/daily/") => {
            let date = request.path.trim_start_matches("/v1/stats/daily/").trim();
            if date.is_empty() {
                Err(AppError::Config("日期不能为空".to_string()))
            } else {
                commands::get_daily_stats_inner(date, state)
                    .map(|stats| HttpResponse::json(200, &stats))
            }
        }
        ("GET", "/v1/apps/recent") => {
            commands::get_recent_app_usage_inner(state).map(|items| {
                let apps = items
                    .iter()
                    .map(|item| item.app_name.clone())
                    .collect::<Vec<_>>();
                HttpResponse::json(200, &serde_json::json!({
                    "apps": apps,
                    "items": items,
                }))
            })
        }
        ("GET", "/v1/apps/category-overview") => {
            commands::get_app_category_overview_inner(state)
                .map(|overview| HttpResponse::json(200, &overview))
        }
        ("GET", "/v1/categories") => {
            commands::get_categories_inner(state)
                .map(|categories| HttpResponse::json(200, &categories))
        }
        ("GET", "/v1/categories/semantic") => {
            commands::get_semantic_categories_inner(state)
                .map(|categories| HttpResponse::json(200, &categories))
        }
        _ if request.method == "GET" && request.path.starts_with("/v1/hourly-summaries/") => {
            let date = request.path.trim_start_matches("/v1/hourly-summaries/").trim();
            if date.is_empty() {
                Err(AppError::Config("日期不能为空".to_string()))
            } else {
                commands::get_hourly_summaries_inner(date, state)
                    .map(|summaries| HttpResponse::json(200, &summaries))
            }
        }
        ("GET", "/v1/storage/stats") => {
            commands::get_storage_stats_inner(state)
                .map(|stats| HttpResponse::json(200, &stats))
        }
        ("GET", "/v1/device") => handle_device_info(state),
        ("GET", "/v1/weekly-review") => {
            let date_from = request.query.get("date_from").cloned();
            let date_to = request.query.get("date_to").cloned();
            let limit = request.query.get("limit").and_then(|v| v.parse().ok());
            commands::generate_weekly_review_inner(date_from, date_to, limit, state)
                .map(|result| HttpResponse::json(200, &result))
        }
        _ if request.method == "GET" && request.path.starts_with("/v1/timeline/") => {
            let date = request.path.trim_start_matches("/v1/timeline/").trim();
            if date.is_empty() {
                Err(AppError::Config("时间线日期不能为空".to_string()))
            } else {
                let limit = request.query.get("limit").and_then(|v| v.parse().ok());
                let offset = request.query.get("offset").and_then(|v| v.parse().ok());
                commands::get_timeline_inner(date.to_string(), limit, offset, state)
                    .map(|activities| HttpResponse::json(200, &activities))
            }
        }
        _ if request.method == "GET" && request.path.starts_with("/v1/hourly-app-breakdown/") => {
            let date = request.path.trim_start_matches("/v1/hourly-app-breakdown/").trim();
            if date.is_empty() {
                Err(AppError::Config("日期不能为空".to_string()))
            } else {
                match state.lock() {
                    Ok(s) => s.database.get_hourly_app_breakdown(date)
                        .map(|result| HttpResponse::json(200, &result)),
                    Err(e) => Err(AppError::Unknown(e.to_string())),
                }
            }
        }
        _ if request.method == "GET" && request.path.starts_with("/v1/activities/") => {
            let date = request.path.trim_start_matches("/v1/activities/").trim();
            if date.is_empty() {
                Err(AppError::Config("日期不能为空".to_string()))
            } else {
                let limit: usize = request.query
                    .get("limit")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(10000);
                match state.lock() {
                    Ok(s) => s.database.get_activities_in_range(Some(date), Some(date), limit)
                        .map(|result| HttpResponse::json(200, &result)),
                    Err(e) => Err(AppError::Unknown(e.to_string())),
                }
            }
        }
        ("GET", "/health") => {
            let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()));
            match guard {
                Ok(s) => Ok(HttpResponse::json(
                    200,
                    &serde_json::json!({
                        "status": "ok",
                        "recording": s.is_recording,
                        "paused": s.is_paused,
                        "version": env!("CARGO_PKG_VERSION"),
                    }),
                )),
                Err(e) => Err(e),
            }
        }
        ("POST", "/feishu/event") => {
            let (config, data_dir) = {
                let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()));
                match guard {
                    Ok(s) => (s.config.clone(), s.data_dir.clone()),
                    Err(e) => return HttpResponse::error(500, e.to_string()),
                }
            };
            if !config.feishu_bot_enabled {
                return HttpResponse::error(404, "飞书 Bot 未启用");
            }
            let body_str = String::from_utf8_lossy(&request.body);
            let resp =
                crate::feishu_bot::handle_feishu_webhook(&body_str, &config, &data_dir).await;
            Ok(HttpResponse {
                status: resp.status,
                reason: reason_phrase(resp.status),
                content_type: "application/json; charset=utf-8",
                body: resp.body.into_bytes(),
            })
        }
        _ => Ok(HttpResponse::error(404, "未找到本地 API 路径")),
    };

    result.unwrap_or_else(|error| {
        let status = if matches!(error, AppError::Config(_)) {
            400
        } else {
            500
        };
        HttpResponse::error(status, error.to_string())
    })
}

fn parse_json_body<T: for<'de> Deserialize<'de>>(request: &ParsedRequest) -> Result<T> {
    serde_json::from_slice(&request.body)
        .map_err(|e| AppError::Config(format!("本地 API JSON 请求体非法: {e}")))
}

fn request_auth_mode(method: &str, path: &str) -> RequestAuthMode {
    if method == "GET" && path == "/health" {
        return RequestAuthMode::None;
    }
    if method == "POST" && path == "/feishu/event" {
        return RequestAuthMode::None;
    }
    RequestAuthMode::LocalApiToken
}

fn authorize_request(request: &ParsedRequest, state: &Arc<Mutex<AppState>>) -> Result<()> {
    match request_auth_mode(&request.method, &request.path) {
        RequestAuthMode::None => return Ok(()),
        RequestAuthMode::LocalApiToken => {}
    }

    let token_path = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        localhost_api_token_path(&state.data_dir)
    };
    let Some(expected_token) = read_localhost_api_token_from_path(&token_path)? else {
        return Err(AppError::Config("缺少或无效的本地 API token".to_string()));
    };

    let from_header = request
        .headers
        .get("authorization")
        .and_then(|value| extract_bearer_token(value));
    let from_query = request.query.get("token").map(|s| s.as_str());

    let provided = from_header.or(from_query);

    if provided == Some(expected_token.as_str()) {
        Ok(())
    } else {
        Err(AppError::Config("缺少或无效的本地 API token".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        effective_api_host, extract_bearer_token, mask_localhost_api_token, request_auth_mode,
        RequestAuthMode,
    };

    #[test]
    fn 监听地址应允许局域网与全部网卡绑定() {
        assert_eq!(effective_api_host(None), "127.0.0.1");
        assert_eq!(effective_api_host(Some(" 0.0.0.0 ")), "0.0.0.0");
        assert_eq!(effective_api_host(Some("192.168.1.23")), "192.168.1.23");
        assert_eq!(effective_api_host(Some("::")), "::");
    }

    #[test]
    fn bearer_token解析应忽略前后空白() {
        assert_eq!(extract_bearer_token("Bearer abc123 "), Some("abc123"));
        assert_eq!(extract_bearer_token("bearer xyz"), Some("xyz"));
        assert_eq!(extract_bearer_token("Basic nope"), None);
    }

    #[test]
    fn token预览应避免泄露完整密钥() {
        let masked = mask_localhost_api_token("wr-local-1234567890abcdef");
        assert!(masked.starts_with("wr-local"));
        assert!(masked.contains('…'));
        assert!(!masked.contains("1234567890abcdef"));
    }

    #[test]
    fn 鉴权模式应将健康检查标记为免鉴权() {
        assert_eq!(request_auth_mode("GET", "/health"), RequestAuthMode::None);
    }

    #[test]
    fn 鉴权模式应将其余路由标记为本地api_token鉴权() {
        assert_eq!(
            request_auth_mode("GET", "/v1/device"),
            RequestAuthMode::LocalApiToken
        );
    }

    #[test]
    fn 周报路由应被纳入本地api_token鉴权() {
        // /v1/weekly-review 不在 None 白名单里，应当回落到默认的 LocalApiToken。
        assert_eq!(
            request_auth_mode("GET", "/v1/weekly-review"),
            RequestAuthMode::LocalApiToken
        );
    }

    #[test]
    fn 时间线路由各种日期形态都应触发鉴权() {
        // 防止某些日期格式误命中 None 白名单（白名单仅 /health 与 /feishu/event）。
        for path in [
            "/v1/timeline/2026-05-08",
            "/v1/timeline/2026-01-01",
            "/v1/timeline/", // 空日期：路由处理器会自行返回 400，但鉴权仍应触发
        ] {
            assert_eq!(
                request_auth_mode("GET", path),
                RequestAuthMode::LocalApiToken,
                "路径 {path} 应需要本地 API token"
            );
        }
    }

    #[test]
    fn 健康检查与飞书事件外的所有方法都应被鉴权() {
        // 巩固现有契约：白名单是 {GET /health, POST /feishu/event}，其它一切 = LocalApiToken。
        assert_eq!(
            request_auth_mode("POST", "/v1/weekly-review"),
            RequestAuthMode::LocalApiToken
        );
        assert_eq!(
            request_auth_mode("DELETE", "/v1/timeline/2026-05-08"),
            RequestAuthMode::LocalApiToken
        );
        assert_eq!(
            request_auth_mode("GET", "/v1/anything-new-we-add-later"),
            RequestAuthMode::LocalApiToken
        );
    }
}
