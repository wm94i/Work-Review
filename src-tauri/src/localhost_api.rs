use crate::commands;
use crate::screenshot::ScreenshotResult;
use crate::AppState;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tauri::AppHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Semaphore};
use uuid::Uuid;
use work_review_core::config::DEFAULT_LOCALHOST_API_PORT;
use work_review_core::error::{AppError, Result};
use work_review_core::privacy::{PrivacyAction, PrivacyFilter};

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
const API_REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);
const API_MAX_CONNECTIONS: usize = 64;
static API_CAPTURE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static API_CONNECTIONS: Lazy<Arc<Semaphore>> =
    Lazy::new(|| Arc::new(Semaphore::new(API_MAX_CONNECTIONS)));

#[derive(Debug)]
struct ApiCapturePermit;

fn acquire_api_capture_permit() -> Result<ApiCapturePermit> {
    API_CAPTURE_IN_PROGRESS
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map(|_| ApiCapturePermit)
        .map_err(|_| AppError::Config("即时截图正在进行，请稍后重试".to_string()))
}

impl Drop for ApiCapturePermit {
    fn drop(&mut self) {
        API_CAPTURE_IN_PROGRESS.store(false, Ordering::Release);
    }
}

#[derive(Default)]
pub struct LocalhostApiRuntime {
    pub running: bool,
    pub bound_host: Option<String>,
    pub bound_port: Option<u16>,
    pub last_error: Option<String>,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
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
struct ExportReportRequest {
    date: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    export_dir: Option<String>,
    #[serde(default)]
    locale: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenerateReportRequest {
    #[serde(default)]
    date: String,
    #[serde(default)]
    force: Option<bool>,
    #[serde(default)]
    locale: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScreenshotCapturePayload {
    captured_at: i64,
    width: u32,
    height: u32,
    mime_type: &'static str,
    image_base64: String,
    relative_path: String,
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
        408 => "Request Timeout",
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
    // 主动刷新缓存，保证生成/轮换后新 token 立即生效
    if let Ok(mut cache) = LOCALHOST_API_TOKEN_CACHE.lock() {
        *cache = Some((path.to_path_buf(), token.to_string(), Instant::now()));
    }
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

/// 本地 API token 内存缓存：(token 文件路径, token, 读取时刻)。
/// 避免每个请求都做磁盘 IO；TTL 内轮换的旧 token 仍短暂可用（约 5 秒），可接受。
/// 写入/轮换 token 时会主动刷新缓存，新 token 立即生效。
static LOCALHOST_API_TOKEN_CACHE: Mutex<Option<(PathBuf, String, Instant)>> = Mutex::new(None);
const LOCALHOST_API_TOKEN_CACHE_TTL: Duration = Duration::from_secs(5);

fn read_localhost_api_token_cached(path: &Path) -> Result<Option<String>> {
    if let Ok(cache) = LOCALHOST_API_TOKEN_CACHE.lock() {
        if let Some((cached_path, token, read_at)) = cache.as_ref() {
            if cached_path == path && read_at.elapsed() < LOCALHOST_API_TOKEN_CACHE_TTL {
                return Ok(Some(token.clone()));
            }
        }
    }

    let token = read_localhost_api_token_from_path(path)?;
    if let Some(token) = token.as_ref() {
        if let Ok(mut cache) = LOCALHOST_API_TOKEN_CACHE.lock() {
            *cache = Some((path.to_path_buf(), token.clone(), Instant::now()));
        }
    }
    Ok(token)
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
    let host =
        runtime_host.unwrap_or_else(|| effective_api_host(config.localhost_api_host.as_deref()));

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
                let Ok(connection_permit) = API_CONNECTIONS.clone().try_acquire_owned() else {
                    log::warn!("本地 API 并发连接已达上限，拒绝新连接");
                    continue;
                };
                let app = app.clone();
                let state = state.clone();
                tauri::async_runtime::spawn(async move {
                    let _connection_permit = connection_permit;
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
    let response =
        match tokio::time::timeout(API_REQUEST_READ_TIMEOUT, read_request(&mut stream)).await {
            Ok(Ok(Some(request))) => route_request(request, &app, &state).await,
            Ok(Ok(None)) => return Ok(()),
            Ok(Err(err)) => HttpResponse::error(400, err.to_string()),
            Err(_) => HttpResponse::error(408, "本地 API 请求读取超时"),
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

/// GET /v1/context — 返回用户当前工作上下文（真实前台窗口 + 最近应用）。
/// 供 MCP Server 的 get_current_context 工具委托调用，拿到独立进程无法直接采集的前台窗口。
fn handle_current_context(state: &Arc<Mutex<AppState>>) -> Result<HttpResponse> {
    let active_window = crate::monitor::get_active_window_fast().ok();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = s
        .database
        .get_timeline(&today, Some(10), None)
        .unwrap_or_default();
    let (ignored_apps, excluded_domains) = commands::collect_privacy_filters(&s);
    let activities =
        commands::filter_activities_by_privacy(activities, &ignored_apps, &excluded_domains);
    let mut seen = std::collections::HashSet::new();
    let recent_apps = activities
        .iter()
        .filter(|a| seen.insert(a.app_name.clone()))
        .map(|a| a.app_name.clone())
        .take(5)
        .collect();

    Ok(build_current_context_response(
        active_window,
        recent_apps,
        &s.privacy_filter,
    ))
}

fn build_current_context_response(
    active_window: Option<crate::monitor::ActiveWindow>,
    recent_apps: Vec<String>,
    privacy_filter: &PrivacyFilter,
) -> HttpResponse {
    let active_window = active_window.and_then(|mut window| {
        match privacy_filter.check_privacy_full(
            &window.app_name,
            &window.window_title,
            window.browser_url.as_deref(),
        ) {
            PrivacyAction::Skip => None,
            PrivacyAction::Anonymize => {
                window.window_title = "[内容已脱敏]".to_string();
                window.browser_url = None;
                Some(window)
            }
            PrivacyAction::Record => {
                window.window_title = privacy_filter.filter_text(&window.window_title);
                Some(window)
            }
        }
    });
    let (primary_app, window_title, browser_url) = match &active_window {
        Some(w) => (
            w.app_name.as_str(),
            w.window_title.as_str(),
            w.browser_url.as_deref(),
        ),
        None => ("", "", None),
    };

    HttpResponse::json(
        200,
        &serde_json::json!({
            "primary_app": primary_app,
            "window_title": window_title,
            "browser_url": browser_url,
            "recent_apps": recent_apps,
            "is_live": active_window.is_some(),
        }),
    )
}

fn ensure_api_capture_enabled(screenshots_enabled: bool) -> Result<()> {
    if screenshots_enabled {
        Ok(())
    } else {
        Err(AppError::Config(
            "截图与 OCR 已关闭，请先在存储设置中启用后再调用即时截图 API".to_string(),
        ))
    }
}

fn build_capture_payload(
    data_dir: &Path,
    result: ScreenshotResult,
    image_base64: String,
) -> ScreenshotCapturePayload {
    let relative_path = result
        .path
        .strip_prefix(data_dir)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| result.path.file_name().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("capture.jpg"))
        .to_string_lossy()
        .replace('\\', "/");

    ScreenshotCapturePayload {
        captured_at: result.timestamp,
        width: result.width,
        height: result.height,
        mime_type: "image/jpeg",
        image_base64,
        relative_path,
    }
}

/// 立即截取当前活动界面。强制仅表示绕过定时采集，不绕过用户的截图隐私开关。
async fn handle_capture_screenshot(state: &Arc<Mutex<AppState>>) -> Result<HttpResponse> {
    let permit = acquire_api_capture_permit()?;
    let active_window = crate::monitor::get_active_window_fast().ok();
    let (data_dir, screenshot_service) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        ensure_api_capture_enabled(guard.config.storage.screenshots_enabled)?;
        (guard.data_dir.clone(), guard.screenshot_service.clone())
    };

    let payload = tokio::task::spawn_blocking(move || -> Result<ScreenshotCapturePayload> {
        let _permit = permit;
        let result = screenshot_service.capture_for_window(active_window.as_ref())?;
        let encoded_result = screenshot_service.generate_full_image_base64(&result.path);

        if let Some(temp_path) = result
            .ocr_source_path
            .as_ref()
            .filter(|path| *path != &result.path)
        {
            if let Err(error) = std::fs::remove_file(temp_path) {
                log::warn!(
                    "清理即时截图 OCR 临时文件失败 {}: {error}",
                    temp_path.display()
                );
            }
        }

        let image_base64 = encoded_result?;
        Ok(build_capture_payload(&data_dir, result, image_base64))
    })
    .await
    .map_err(|e| AppError::Unknown(format!("即时截图任务异常: {e}")))??;

    Ok(HttpResponse::json(200, &payload))
}

fn parse_generate_report_params(
    request: &ParsedRequest,
) -> Result<(String, Option<bool>, Option<String>)> {
    let (date, force, locale) = match request.method.as_str() {
        "GET" => (
            request.query.get("date").cloned().unwrap_or_default(),
            request
                .query
                .get("force")
                .and_then(|value| value.parse().ok()),
            request.query.get("locale").cloned(),
        ),
        "POST" => {
            let body = parse_json_body::<GenerateReportRequest>(request)?;
            (body.date, body.force, body.locale)
        }
        _ => {
            return Err(AppError::Config(
                "日报生成接口仅支持 GET 或 POST".to_string(),
            ))
        }
    };

    if date.trim().is_empty() {
        Err(AppError::Config("date 参数不能为空".to_string()))
    } else {
        Ok((date, force, locale))
    }
}

async fn handle_generate_report_request(
    request: &ParsedRequest,
    app: &AppHandle,
    state: &Arc<Mutex<AppState>>,
) -> Result<HttpResponse> {
    let (date, force, locale) = parse_generate_report_params(request)?;
    commands::generate_report_inner(date, force, locale, app, state)
        .await
        .map(|content| HttpResponse::json(200, &serde_json::json!({ "content": content })))
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
        ("GET" | "POST", "/v1/reports/generate") => {
            handle_generate_report_request(&request, app, state).await
        }
        ("POST", "/v1/reports/export-markdown") => parse_json_body::<ExportReportRequest>(&request)
            .and_then(|body| {
                commands::export_report_markdown_inner(
                    body.date,
                    body.content,
                    body.export_dir,
                    body.locale,
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
            commands::get_today_stats_inner(state).map(|stats| HttpResponse::json(200, &stats))
        }
        ("GET", "/v1/stats/overview") => {
            let mode = request
                .query
                .get("mode")
                .cloned()
                .unwrap_or_else(|| "today".to_string());
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
        ("GET", "/v1/apps/recent") => commands::get_recent_app_usage_inner(state).map(|items| {
            let apps = items
                .iter()
                .map(|item| item.app_name.clone())
                .collect::<Vec<_>>();
            HttpResponse::json(
                200,
                &serde_json::json!({
                    "apps": apps,
                    "items": items,
                }),
            )
        }),
        ("GET", "/v1/apps/category-overview") => commands::get_app_category_overview_inner(state)
            .map(|overview| HttpResponse::json(200, &overview)),
        ("GET", "/v1/categories") => commands::get_categories_inner(state)
            .map(|categories| HttpResponse::json(200, &categories)),
        ("GET", "/v1/categories/semantic") => commands::get_semantic_categories_inner(state)
            .map(|categories| HttpResponse::json(200, &categories)),
        _ if request.method == "GET" && request.path.starts_with("/v1/hourly-summaries/") => {
            let date = request
                .path
                .trim_start_matches("/v1/hourly-summaries/")
                .trim();
            if date.is_empty() {
                Err(AppError::Config("日期不能为空".to_string()))
            } else {
                commands::get_hourly_summaries_inner(date, state)
                    .map(|summaries| HttpResponse::json(200, &summaries))
            }
        }
        ("GET", "/v1/storage/stats") => {
            commands::get_storage_stats_inner(state).map(|stats| HttpResponse::json(200, &stats))
        }
        ("GET", "/v1/device") => handle_device_info(state),
        ("GET", "/v1/context") => handle_current_context(state),
        ("POST", "/v1/screenshots/capture") => handle_capture_screenshot(state).await,
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
            let date = request
                .path
                .trim_start_matches("/v1/hourly-app-breakdown/")
                .trim();
            if date.is_empty() {
                Err(AppError::Config("日期不能为空".to_string()))
            } else {
                let mode = request.query.get("mode").cloned();
                let date_from = request.query.get("date_from").cloned();
                let date_to = request.query.get("date_to").cloned();
                commands::get_hourly_app_breakdown_inner(
                    Some(date.to_string()),
                    date_from,
                    date_to,
                    mode,
                    state,
                )
                .map(|result| HttpResponse::json(200, &result))
            }
        }
        _ if request.method == "GET" && request.path.starts_with("/v1/activities/") => {
            let date = request.path.trim_start_matches("/v1/activities/").trim();
            match validate_activity_date(date) {
                Err(error) => Err(error),
                Ok(()) => {
                    let limit: usize = request
                        .query
                        .get("limit")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(10000);
                    match state.lock() {
                        Ok(s) => commands::load_filtered_activities_in_range(
                            &s,
                            Some(date),
                            Some(date),
                            limit,
                        )
                        .map(|result| HttpResponse::json(200, &result)),
                        Err(e) => Err(AppError::Unknown(e.to_string())),
                    }
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
            let resp = crate::feishu_bot::handle_feishu_webhook(
                &request.headers,
                &body_str,
                &config,
                &data_dir,
            )
            .await;
            Ok(HttpResponse {
                status: resp.status,
                reason: reason_phrase(resp.status),
                content_type: "application/json; charset=utf-8",
                body: resp.body.into_bytes(),
            })
        }
        ("GET", "/wecom/callback") => {
            let config = {
                let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()));
                match guard {
                    Ok(s) => s.config.clone(),
                    Err(e) => return HttpResponse::error(500, e.to_string()),
                }
            };
            if !config.wecom_bot_enabled {
                return HttpResponse::error(404, "企业微信 Bot 未启用");
            }
            let resp = crate::wecom_bot::handle_wecom_verify(&request.query, &config);
            let content_type: &'static str = match resp.content_type.as_str() {
                ct if ct.contains("xml") => "application/xml; charset=utf-8",
                ct if ct.contains("text") => "text/plain; charset=utf-8",
                _ => "application/json; charset=utf-8",
            };
            Ok(HttpResponse {
                status: resp.status,
                reason: reason_phrase(resp.status),
                content_type,
                body: resp.body.into_bytes(),
            })
        }
        ("POST", "/wecom/callback") => {
            let (config, data_dir) = {
                let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()));
                match guard {
                    Ok(s) => (s.config.clone(), s.data_dir.clone()),
                    Err(e) => return HttpResponse::error(500, e.to_string()),
                }
            };
            if !config.wecom_bot_enabled {
                return HttpResponse::error(404, "企业微信 Bot 未启用");
            }
            let body_str = String::from_utf8_lossy(&request.body);
            let resp = crate::wecom_bot::handle_wecom_callback(
                &request.query,
                &body_str,
                &config,
                &data_dir,
            )
            .await;
            let content_type: &'static str = if resp.content_type.contains("xml") {
                "application/xml; charset=utf-8"
            } else {
                "application/json; charset=utf-8"
            };
            Ok(HttpResponse {
                status: resp.status,
                reason: reason_phrase(resp.status),
                content_type,
                body: resp.body.into_bytes(),
            })
        }
        ("POST", "/dingtalk/callback") => {
            let (config, data_dir) = {
                let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()));
                match guard {
                    Ok(s) => (s.config.clone(), s.data_dir.clone()),
                    Err(e) => return HttpResponse::error(500, e.to_string()),
                }
            };
            if !config.dingtalk_bot_enabled {
                return HttpResponse::error(404, "钉钉 Bot 未启用");
            }
            let body_str = String::from_utf8_lossy(&request.body);
            let resp = crate::dingtalk_bot::handle_dingtalk_callback(
                &request.headers,
                &body_str,
                &config,
                &data_dir,
            )
            .await;
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

fn validate_activity_date(date: &str) -> Result<()> {
    if date.len() != 10 || chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return Err(AppError::Config("活动日期格式应为 YYYY-MM-DD".to_string()));
    }
    Ok(())
}

fn request_auth_mode(method: &str, path: &str) -> RequestAuthMode {
    if method == "GET" && path == "/health" {
        return RequestAuthMode::None;
    }
    if method == "POST" && path == "/feishu/event" {
        return RequestAuthMode::None;
    }
    if method == "GET" && path == "/wecom/callback" {
        return RequestAuthMode::None;
    }
    if method == "POST" && path == "/wecom/callback" {
        return RequestAuthMode::None;
    }
    if method == "POST" && path == "/dingtalk/callback" {
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
    let Some(expected_token) = read_localhost_api_token_cached(&token_path)? else {
        return Err(AppError::Config("缺少或无效的本地 API token".to_string()));
    };

    let from_header = request
        .headers
        .get("authorization")
        .and_then(|value| extract_bearer_token(value));
    let from_query = request.query.get("token").map(|s| s.as_str());

    let provided = from_header.or(from_query);

    // 常量时间比较，与飞书/钉钉/企微的签名校验保持一致的安全基线，
    // 避免理论上对本地 token 的时序侧信道。
    let matched = provided
        .map(|p| bool::from(p.as_bytes().ct_eq(expected_token.as_bytes())))
        .unwrap_or(false);
    if matched {
        Ok(())
    } else {
        Err(AppError::Config("缺少或无效的本地 API token".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_api_capture_permit, build_capture_payload, build_current_context_response,
        effective_api_host, ensure_api_capture_enabled, extract_bearer_token,
        mask_localhost_api_token, parse_generate_report_params, request_auth_mode,
        validate_activity_date, ParsedRequest, RequestAuthMode,
    };
    use crate::monitor::ActiveWindow;
    use crate::screenshot::ScreenshotResult;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use work_review_core::config::{AppPrivacyRule, PrivacyConfig, PrivacyLevel};
    use work_review_core::privacy::PrivacyFilter;

    #[test]
    fn 实时上下文响应应尊重忽略脱敏域名与敏感文本规则() {
        let config = PrivacyConfig {
            app_rules: vec![
                AppPrivacyRule {
                    app_name: "1Password".into(),
                    level: PrivacyLevel::Ignored,
                },
                AppPrivacyRule {
                    app_name: "Private App".into(),
                    level: PrivacyLevel::Anonymized,
                },
            ],
            excluded_domains: vec!["private.example".into()],
            ..PrivacyConfig::default()
        };
        let filter = PrivacyFilter::from_config(&config);
        let window = |app: &str, title: &str, url: Option<&str>| ActiveWindow {
            app_name: app.into(),
            window_title: title.into(),
            browser_url: url.map(str::to_string),
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
        };
        let response = |window| {
            let response = build_current_context_response(window, vec!["Editor".into()], &filter);
            serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()
        };

        for window in [
            None,
            Some(window("1Password", "Vault", None)),
            Some(window(
                "Google Chrome",
                "Internal",
                Some("https://private.example/page"),
            )),
        ] {
            let json = response(window);
            assert_eq!(json["primary_app"], "");
            assert_eq!(json["window_title"], "");
            assert!(json["browser_url"].is_null());
            assert_eq!(json["is_live"], false);
            assert_eq!(json["recent_apps"], serde_json::json!(["Editor"]));
        }
        let json = response(Some(window(
            "Private App",
            "Private title",
            Some("https://example.org"),
        )));
        assert_eq!(json["primary_app"], "Private App");
        assert_eq!(json["window_title"], "[内容已脱敏]");
        assert!(json["browser_url"].is_null());
        assert_eq!(json["is_live"], true);

        let json = response(Some(window("Editor", "Contact person@example.org", None)));
        assert_eq!(json["window_title"], "Contact [已过滤]");
        assert_eq!(json["primary_app"], "Editor");
        assert_eq!(json["is_live"], true);
    }

    fn parsed_request(
        method: &str,
        query: &[(&str, &str)],
        body: serde_json::Value,
    ) -> ParsedRequest {
        ParsedRequest {
            method: method.to_string(),
            path: "/v1/reports/generate".to_string(),
            query: query
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            headers: HashMap::new(),
            body: serde_json::to_vec(&body).expect("请求体应可序列化"),
        }
    }

    #[test]
    fn 即时截图响应应返回相对路径和可直接使用的图像数据() {
        let payload = build_capture_payload(
            Path::new("/tmp/work-review-data"),
            ScreenshotResult {
                path: PathBuf::from("/tmp/work-review-data/screenshots/2026-08-10/shot.jpg"),
                ocr_source_path: None,
                timestamp: 1_786_334_400,
                width: 1440,
                height: 900,
            },
            "encoded-jpeg".to_string(),
        );
        let json = serde_json::to_value(payload).expect("截图响应应可序列化");

        assert_eq!(json["capturedAt"], 1_786_334_400);
        assert_eq!(json["width"], 1440);
        assert_eq!(json["height"], 900);
        assert_eq!(json["mimeType"], "image/jpeg");
        assert_eq!(json["imageBase64"], "encoded-jpeg");
        assert_eq!(json["relativePath"], "screenshots/2026-08-10/shot.jpg");
        assert!(json.get("path").is_none(), "不得暴露绝对文件路径");
    }

    #[test]
    fn 即时截图路径异常时也不得暴露绝对路径() {
        let payload = build_capture_payload(
            Path::new("/tmp/work-review-data"),
            ScreenshotResult {
                path: PathBuf::from("/private/tmp/unexpected-shot.jpg"),
                ocr_source_path: None,
                timestamp: 1_786_334_400,
                width: 1440,
                height: 900,
            },
            "encoded-jpeg".to_string(),
        );
        let json = serde_json::to_value(payload).expect("截图响应应可序列化");

        assert_eq!(json["relativePath"], "unexpected-shot.jpg");
        assert!(!json["relativePath"]
            .as_str()
            .unwrap_or_default()
            .starts_with('/'));
    }

    #[test]
    fn 关闭截图功能时即时截图api不应绕过隐私开关() {
        let error = ensure_api_capture_enabled(false).expect_err("关闭截图时必须拒绝请求");
        assert!(error.to_string().contains("截图与 OCR"));
        assert!(ensure_api_capture_enabled(true).is_ok());
    }

    #[test]
    fn 即时截图应限制为单个并发任务并在结束后释放许可() {
        let permit = acquire_api_capture_permit().expect("首个截图任务应取得许可");
        let error = acquire_api_capture_permit().expect_err("并发截图任务必须被拒绝");
        assert!(error.to_string().contains("正在进行"));

        drop(permit);
        assert!(acquire_api_capture_permit().is_ok(), "任务结束后应释放许可");
    }

    #[test]
    fn post日报生成参数应从json请求体解析() {
        let request = parsed_request(
            "POST",
            &[],
            serde_json::json!({ "date": "2026-07-21", "force": true, "locale": "zh-CN" }),
        );

        assert_eq!(
            parse_generate_report_params(&request).expect("POST 参数应合法"),
            (
                "2026-07-21".to_string(),
                Some(true),
                Some("zh-CN".to_string())
            )
        );
    }

    #[test]
    fn post日报生成参数可省略force和locale() {
        let request = parsed_request("POST", &[], serde_json::json!({ "date": "2026-07-21" }));

        assert_eq!(
            parse_generate_report_params(&request).expect("可选参数应允许省略"),
            ("2026-07-21".to_string(), None, None)
        );
    }

    #[test]
    fn get日报生成参数应继续从query解析() {
        let request = parsed_request(
            "GET",
            &[("date", "2026-07-21"), ("force", "false"), ("locale", "en")],
            serde_json::Value::Null,
        );

        assert_eq!(
            parse_generate_report_params(&request).expect("GET 参数应保持兼容"),
            (
                "2026-07-21".to_string(),
                Some(false),
                Some("en".to_string())
            )
        );
    }

    #[test]
    fn 日报生成参数缺少日期时应返回配置错误() {
        let request = parsed_request("POST", &[], serde_json::json!({ "force": true }));
        let error = parse_generate_report_params(&request).expect_err("缺少日期必须失败");
        assert!(error.to_string().contains("date 参数不能为空"));
    }

    #[test]
    fn 活动日期必须是合法的零填充本地日期() {
        assert!(validate_activity_date("2026-10-28").is_ok());
        assert!(validate_activity_date("2026-2-8").is_err());
        assert!(validate_activity_date("not-a-date").is_err());
        assert!(validate_activity_date("2026-02-30").is_err());
    }

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
        // 防止某些日期格式误命中 None 白名单。
        // 当前免鉴权白名单：/health、/feishu/event、/wecom/callback (GET+POST)、/dingtalk/callback。
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
    fn 健康检查与机器人回调外的所有方法都应被鉴权() {
        // 巩固现有契约：白名单是 {GET /health, POST /feishu/event,
        // GET/POST /wecom/callback, POST /dingtalk/callback}，其它一切 = LocalApiToken。
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

    #[test]
    fn 机器人回调路由应在免鉴权白名单内() {
        // 三个 webhook 路由本身依赖各自的签名校验，不走 localhost token。
        assert_eq!(
            request_auth_mode("GET", "/wecom/callback"),
            RequestAuthMode::None
        );
        assert_eq!(
            request_auth_mode("POST", "/wecom/callback"),
            RequestAuthMode::None
        );
        assert_eq!(
            request_auth_mode("POST", "/dingtalk/callback"),
            RequestAuthMode::None
        );
        assert_eq!(
            request_auth_mode("POST", "/feishu/event"),
            RequestAuthMode::None
        );
        // 错误的方法仍要鉴权（防止误开放）
        assert_eq!(
            request_auth_mode("DELETE", "/wecom/callback"),
            RequestAuthMode::LocalApiToken
        );
        assert_eq!(
            request_auth_mode("GET", "/dingtalk/callback"),
            RequestAuthMode::LocalApiToken
        );
    }
}
