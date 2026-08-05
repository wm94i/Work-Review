use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use work_review_core::config::AppConfig;
use work_review_core::database::{
    apply_flex_overtime_correction, Activity, DailyStats, Database, MemorySearchItem,
};
use work_review_core::policy::{CallSource, Permission, PolicyDecision, PolicyEnforcer};
use work_review_core::privacy::{
    collect_privacy_filters, matches_excluded_domain, matches_ignored_app,
};
use work_review_skills_engine::engine::SkillEngine;
use work_review_skills_engine::executor::{ExecutionContext, OutputContentType};
use work_review_skills_engine::model::Permission as SkillPermission;

struct AppState {
    db: Database,
    config: AppConfig,
    localhost_api_token_path: std::path::PathBuf,
    policy: PolicyEnforcer,
    skills: SkillEngine,
}

fn main() {
    env_logger::init();

    let db_path = std::env::var("WORK_REVIEW_DB_PATH").unwrap_or_else(|_| {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("work-review");
        data_dir
            .join("work_review.db")
            .to_string_lossy()
            .to_string()
    });

    let config_path = std::env::var("WORK_REVIEW_CONFIG_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let data_dir = dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("work-review");
            data_dir.join("config.json")
        });

    let db = match Database::new(std::path::Path::new(&db_path)) {
        Ok(db) => db,
        Err(e) => {
            log::error!("无法打开数据库 {db_path}: {e}");
            std::process::exit(1);
        }
    };

    let config = match AppConfig::load(&config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            // 显式提示配置加载失败，避免静默回退到 default 让用户误以为配置生效。
            log::error!(
                "无法加载配置文件 {}: {}（将使用默认配置，工作时段/AI 提供方等可能与 UI 显示不一致）",
                config_path.display(),
                e
            );
            AppConfig::default()
        }
    };

    // 用户在 UI 上把 MCP Server 关闭后，即使客户端尝试启动本 binary 也立刻退出。
    // 这样开关才是"真开关"，而不是仅供查看的状态标签。
    if !config.mcp_server_enabled {
        log::error!(
            "MCP Server 已在 Work Review 设置中关闭。如需启用，请打开 Work Review → 设置 → 接入管理 → MCP Server 开关。"
        );
        eprintln!(
            "MCP Server is disabled in Work Review settings. Enable it via Settings → Integrations → MCP Server."
        );
        std::process::exit(2);
    }

    let mut policy = PolicyEnforcer::new(&config);
    let skills = SkillEngine::new();

    // 注册所有内置技能的权限到策略层
    for pkg in skills.list_skills() {
        let perms: Vec<Permission> = pkg
            .required_permissions
            .iter()
            .map(|p| match p {
                SkillPermission::ReadActivities => Permission::ReadActivities,
                SkillPermission::ReadReports => Permission::ReadReports,
                SkillPermission::ReadStats => Permission::ReadStats,
                SkillPermission::ReadSessions => Permission::ReadSessions,
                SkillPermission::ReadConfig => Permission::ReadConfig,
                SkillPermission::WriteReport => Permission::WriteReport,
                SkillPermission::WriteConfig => Permission::WriteConfig,
                SkillPermission::ExecuteAi => Permission::ExecuteAi,
                SkillPermission::ReadDeviceStatus => Permission::ReadDeviceStatus,
            })
            .collect();
        policy.register_skill_permissions(&pkg.id, perms);
    }

    let state = Arc::new(Mutex::new(AppState {
        db,
        config,
        localhost_api_token_path: localhost_api_token_path(&config_path),
        policy,
        skills,
    }));

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log::error!("读取 stdin 失败，退出: {e}");
                break;
            }
        };
        // 空行（心跳/缓冲清空）直接跳过，不算解析错误
        if line.trim().is_empty() {
            continue;
        }
        // 解析失败必须按 JSON-RPC 2.0 规范返回 -32700 Parse error，
        // 不能静默丢弃，否则客户端会一直等响应。
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                // JSON-RPC 通知（有 method 但无 id）不需要响应。MCP 的
                // notifications/initialized 就是通知——回复无效消息（无 id/result/error）
                // 会让严格客户端（Claude Code / Codex 等）解析报错、连接异常。
                let is_notification = request.get("id").is_none();
                let resp = handle_request(&request, &state);
                if is_notification {
                    None
                } else {
                    Some(resp)
                }
            }
            Err(parse_err) => {
                log::warn!("收到不可解析的 JSON-RPC 请求: {parse_err}");
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {parse_err}")
                    }
                }))
            }
        };
        let Some(response) = response else {
            continue;
        };
        match serde_json::to_string(&response) {
            Ok(output) => {
                if writeln!(stdout, "{output}").is_err() || stdout.flush().is_err() {
                    log::error!("写 stdout 失败，退出");
                    break;
                }
            }
            Err(e) => log::error!("序列化响应失败: {e}"),
        }
    }
}

fn handle_request(request: &Value, state: &Arc<Mutex<AppState>>) -> Value {
    let method = request["method"].as_str().unwrap_or("");
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let params = request.get("params").cloned().unwrap_or(json!({}));

    match method {
        "initialize" => {
            // 版本协商：接受客户端请求的版本（如果在支持列表里），否则回退到最新。
            // Cherry Studio / Claude Code 等客户端会校验返回的 protocolVersion。
            const SUPPORTED: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"];
            let client_version = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("2024-11-05");
            let negotiated = if SUPPORTED.contains(&client_version) { client_version } else { "2025-11-25" };
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": negotiated,
                    "capabilities": {
                        "tools": { "listChanged": false },
                        "resources": { "subscribe": false, "listChanged": false },
                        "prompts": { "listChanged": false }
                    },
                    "serverInfo": {
                        "name": "work-review-mcp-server",
                        "version": "0.1.0"
                    }
                }
            })
        }
        "notifications/initialized" => json!(Value::Null),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": tools_list() }
        }),
        "tools/call" => {
            let tool_name = params["name"].as_str().unwrap_or("").to_string();
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let result = handle_tool_call(&tool_name, &arguments, state);
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        }
        "resources/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "resources": resources_list() }
        }),
        "resources/read" => {
            let uri = params["uri"].as_str().unwrap_or("").to_string();
            let result = handle_resource_read(&uri, state);
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        }
        "prompts/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "prompts": prompts_list() }
        }),
        "prompts/get" => {
            let name = params["name"].as_str().unwrap_or("").to_string();
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let result = handle_prompt_get(&name, &arguments);
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        }
        "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("Method not found: {}", method) }
        }),
    }
}

// ══════════════════════════════════════════════════════════
// Localhost API 委托 —— MCP 进程通过 HTTP 调主应用的 localhost API
// 获取独立进程无法直接采集的能力（真实前台窗口、AI 报表生成）
// ══════════════════════════════════════════════════════════

fn localhost_api_token_path(config_path: &std::path::Path) -> std::path::PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("localhost_api_token.txt")
}

fn read_localhost_api_token(token_path: &std::path::Path) -> Option<String> {
    let token = std::fs::read_to_string(token_path)
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())?;
    let suffix = token.strip_prefix("wr-local-")?;
    if suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(token)
    } else {
        None
    }
}

/// 返回主应用 localhost API 的 base URL（如 "http://127.0.0.1:47831"）。
/// 仅当 localhost_api_enabled = true 且端口有效时返回 Some。
fn localhost_api_base(config: &AppConfig) -> Option<String> {
    if !config.localhost_api_enabled {
        return None;
    }
    let port = if config.localhost_api_port == 0 {
        work_review_core::config::DEFAULT_LOCALHOST_API_PORT
    } else {
        config.localhost_api_port
    };
    let host = config
        .localhost_api_host
        .as_deref()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or("127.0.0.1");
    Some(format!("http://{host}:{port}"))
}

/// 低成本委托（live context 等）的快速失败超时。
const LOCALHOST_QUICK_TIMEOUT_SECS: u64 = 2;
/// AI 日报生成委托的额外缓冲（秒）：主应用侧有自己的生成超时，
/// 委托方在其基础上多留一点余量，避免主应用还没收尾委托就先超时。
const LOCALHOST_GENERATE_TIMEOUT_BUFFER_SECS: u64 = 30;

fn build_localhost_get_request(
    config: &AppConfig,
    token_path: &std::path::Path,
    path: &str,
    timeout_secs: u64,
) -> Option<ureq::Request> {
    let base = localhost_api_base(config)?;
    let token = read_localhost_api_token(token_path)?;
    let url = format!("{base}{path}");
    Some(
        ureq::get(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(std::time::Duration::from_secs(timeout_secs)),
    )
}

/// 尝试 GET 主应用的 localhost API（快速超时）。失败返回 None（不阻塞 MCP 主流程）。
fn try_localhost_get(
    config: &AppConfig,
    token_path: &std::path::Path,
    path: &str,
) -> Option<Value> {
    try_localhost_get_with_timeout(config, token_path, path, LOCALHOST_QUICK_TIMEOUT_SECS)
}

/// 尝试 GET 主应用的 localhost API，可指定超时。失败返回 None（不阻塞 MCP 主流程）。
fn try_localhost_get_with_timeout(
    config: &AppConfig,
    token_path: &std::path::Path,
    path: &str,
    timeout_secs: u64,
) -> Option<Value> {
    let request = build_localhost_get_request(config, token_path, path, timeout_secs)?;
    let url = request.url().to_string();
    match request.call() {
        Ok(response) => response.into_json::<Value>().ok(),
        Err(ureq::Error::Status(status, _)) => {
            log::debug!("localhost API 委托失败 ({url}): HTTP {status}");
            None
        }
        Err(ureq::Error::Transport(_)) => {
            // 不记录第三方错误详情，避免非法请求头把 Bearer Token 带入日志。
            log::debug!("localhost API 委托失败 ({url}): 传输错误");
            None
        }
    }
}

fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "query_timeline",
            "description": "查询指定日期的活动时间线",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日期，格式 YYYY-MM-DD" },
                    "limit": { "type": "integer", "description": "返回数量限制" },
                    "offset": { "type": "integer", "description": "偏移量" }
                },
                "required": ["date"]
            }
        }),
        json!({
            "name": "get_daily_stats",
            "description": "获取指定日期的工作统计数据",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日期，格式 YYYY-MM-DD" }
                },
                "required": ["date"]
            }
        }),
        json!({
            "name": "search_activities",
            "description": "搜索工作活动记录",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索关键词" },
                    "date_from": { "type": "string", "description": "起始日期" },
                    "date_to": { "type": "string", "description": "结束日期" },
                    "limit": { "type": "integer", "description": "返回数量限制" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "get_work_sessions",
            "description": "获取指定日期的工作会话分析",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日期，格式 YYYY-MM-DD" }
                },
                "required": ["date"]
            }
        }),
        json!({
            "name": "analyze_intents",
            "description": "分析指定日期的工作意图",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日期，格式 YYYY-MM-DD" }
                },
                "required": ["date"]
            }
        }),
        json!({
            "name": "generate_report",
            "description": "生成指定日期的工作日报",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日期，格式 YYYY-MM-DD" },
                    "locale": { "type": "string", "description": "语言，zh-CN/en/zh-TW" }
                },
                "required": ["date"]
            }
        }),
        json!({
            "name": "get_report",
            "description": "获取已生成的日报",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "日期，格式 YYYY-MM-DD" },
                    "locale": { "type": "string", "description": "语言" }
                },
                "required": ["date"]
            }
        }),
        json!({
            "name": "get_device_status",
            "description": "获取当前设备状态信息",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "execute_skill",
            "description": "执行指定的 Skills Engine 技能",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "skill_id": { "type": "string", "description": "技能 ID" },
                    "params": { "type": "object", "description": "技能参数" }
                },
                "required": ["skill_id"]
            }
        }),
        json!({
            "name": "list_skills",
            "description": "列出所有可用技能",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "get_skill_stats",
            "description": "获取技能执行统计",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "skill_id": { "type": "string", "description": "技能 ID，不传则返回所有" }
                }
            }
        }),
        json!({
            "name": "get_current_context",
            "description": "获取用户当前工作上下文：正在使用的应用、窗口标题、分类、持续时间、最近切换的应用。让 AI 工具一步了解用户在做什么，无需先查时间线再分析。",
            "inputSchema": { "type": "object", "properties": {} }
        }),
    ]
}

fn with_policy_check<F>(
    state: &Arc<Mutex<AppState>>,
    tool_name: &str,
    permission: Permission,
    f: F,
) -> Value
where
    F: FnOnce(&mut AppState) -> Value,
{
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    let source = CallSource::McpTool {
        tool_name: tool_name.to_string(),
        client_id: None,
    };
    match s.policy.check_permission(&source, permission) {
        PolicyDecision::Allow => f(&mut s),
        PolicyDecision::AllowSanitized => {
            let mut result = f(&mut s);
            sanitize_result(&mut result);
            result
        }
        PolicyDecision::Deny => tool_error(&format!("权限被拒绝: 无 {permission:?} 权限")),
    }
}

/// 对 MCP tool 返回值做脱敏处理。MCP content 数组里的 text 字段通常是 JSON 字符串，
/// 把它解析后递归走一遍，删/截关键敏感字段，再重新序列化。
fn sanitize_result(result: &mut Value) {
    let Some(content) = result.get_mut("content").and_then(|c| c.as_array_mut()) else {
        return;
    };
    for item in content.iter_mut() {
        // text 字段是 JSON 字符串，需要解析后再脱敏
        let Some(text_str) = item
            .get("text")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        let Ok(mut parsed) = serde_json::from_str::<Value>(&text_str) else {
            // 不是 JSON（比如纯文本日报），跳过——这种情况通常不含结构化敏感字段
            continue;
        };
        sanitize_value(&mut parsed);
        if let Ok(serialized) = serde_json::to_string_pretty(&parsed) {
            item["text"] = Value::String(serialized);
        }
    }
}

/// 递归脱敏 JSON Value：删除已知敏感字段，截断潜在敏感字段。
fn sanitize_value(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            // 完全删除：截图路径/URL、可执行文件路径、OCR 提取文本（屏幕内容可能含密码/私信等）
            obj.remove("screenshot_path");
            obj.remove("screenshot_url");
            obj.remove("executable_path");
            obj.remove("ocr_text");
            // 截断：窗口标题常包含具体文件名/网页标题，截前 40 字符保留 app 上下文即可
            if let Some(title) = obj.get_mut("window_title") {
                if let Some(s) = title.as_str() {
                    let head: String = s.chars().take(40).collect();
                    let suffix = if s.chars().count() > 40 { "…" } else { "" };
                    *title = Value::String(format!("{head}{suffix}"));
                }
            }
            // 递归处理嵌套对象
            for (_, v) in obj.iter_mut() {
                sanitize_value(v);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                sanitize_value(item);
            }
        }
        _ => {}
    }
}

/// URL query 参数百分号编码：保留字母数字与 - _ . ~，其余字节转 %XX。
/// mcp-server 未依赖 urlencoding crate，这里用极简实现覆盖 query 场景。
fn encode_query_param(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn load_daily_stats_for_mcp(
    state: &AppState,
    date: &str,
) -> work_review_core::error::Result<DailyStats> {
    let segments = state.config.effective_work_segments();
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state.config);
    let mut stats = state.db.get_daily_stats_with_segments_filtered(
        date,
        &segments,
        &ignored_apps,
        &excluded_domains,
    )?;
    apply_flex_overtime_correction(
        &mut stats,
        state.config.work_time_enabled,
        state.config.standard_work_hours,
    );
    Ok(stats)
}

fn filter_activities_by_privacy(activities: Vec<Activity>, config: &AppConfig) -> Vec<Activity> {
    let (ignored_apps, excluded_domains) = collect_privacy_filters(config);
    use work_review_core::privacy::{PrivacyAction, PrivacyFilter};

    let privacy = PrivacyFilter::from_config(&config.privacy);

    activities
        .into_iter()
        .filter(|activity| {
            // 第一层：ignored 级别 → 完全排除
            !matches_ignored_app(&activity.app_name, &ignored_apps)
                && activity
                    .browser_url
                    .as_deref()
                    .map(|url| !matches_excluded_domain(url, &excluded_domains))
                    .unwrap_or(true)
        })
        // 第二层：anonymized 级别 → 脱敏标题/URL（与主应用行为一致）
        .map(|mut activity| {
            let action = privacy.check_privacy_full(
                &activity.app_name,
                &activity.window_title,
                activity.browser_url.as_deref(),
            );
            if action == PrivacyAction::Anonymize {
                activity.window_title = "[内容已脱敏]".to_string();
                activity.browser_url = None;
                // 可执行文件路径与远程截图 URL 同样可能暴露脱敏应用的信息，一并清空
                activity.executable_path = None;
                activity.screenshot_url = None;
            }
            activity
        })
        .collect()
}

fn filter_memory_results_by_privacy(
    results: Vec<MemorySearchItem>,
    config: &AppConfig,
) -> Vec<MemorySearchItem> {
    let (ignored_apps, excluded_domains) = collect_privacy_filters(config);
    use work_review_core::privacy::{PrivacyAction, PrivacyFilter};

    let privacy = PrivacyFilter::from_config(&config.privacy);

    results
        .into_iter()
        .filter(|item| {
            item.app_name
                .as_deref()
                .map(|app| !matches_ignored_app(app, &ignored_apps))
                .unwrap_or(true)
                && item
                    .browser_url
                    .as_deref()
                    .map(|url| !matches_excluded_domain(url, &excluded_domains))
                    .unwrap_or(true)
        })
        .map(|mut item| {
            let app = item.app_name.as_deref().unwrap_or("");
            let action = privacy.check_privacy_full(app, &item.title, item.browser_url.as_deref());
            if action == PrivacyAction::Anonymize {
                item.title = "[内容已脱敏]".to_string();
                item.browser_url = None;
            }
            item
        })
        .collect()
}

fn handle_tool_call(name: &str, args: &Value, state: &Arc<Mutex<AppState>>) -> Value {
    match name {
        "query_timeline" => with_policy_check(state, name, Permission::ReadActivities, |s| {
            let date = args["date"].as_str().unwrap_or("");
            let limit = args["limit"].as_u64().map(|l| l as u32);
            let offset = args["offset"].as_u64().map(|o| o as u32);
            match s.db.get_timeline(date, limit, offset) {
                Ok(activities) => {
                    let activities = filter_activities_by_privacy(activities, &s.config);
                    json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&activities).unwrap_or_default() }]
                    })
                }
                Err(e) => tool_error(&format!("查询时间线失败: {e}")),
            }
        }),
        "get_daily_stats" => with_policy_check(state, name, Permission::ReadStats, |s| {
            let date = args["date"].as_str().unwrap_or("");
            match load_daily_stats_for_mcp(s, date) {
                Ok(stats) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&stats).unwrap_or_default() }]
                }),
                Err(e) => tool_error(&format!("获取统计失败: {e}")),
            }
        }),
        "search_activities" => with_policy_check(state, name, Permission::ReadActivities, |s| {
            let query = args["query"].as_str().unwrap_or("");
            let date_from = args["date_from"].as_str();
            let date_to = args["date_to"].as_str();
            let limit = args["limit"].as_u64().unwrap_or(50) as usize;
            match s.db.search_memory(query, date_from, date_to, limit) {
                Ok(results) => {
                    let results = filter_memory_results_by_privacy(results, &s.config);
                    json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&results).unwrap_or_default() }]
                    })
                }
                Err(e) => tool_error(&format!("搜索失败: {e}")),
            }
        }),
        "get_work_sessions" => with_policy_check(state, name, Permission::ReadSessions, |s| {
            let date = args["date"].as_str().unwrap_or("");
            match s.db.get_timeline(date, None, None) {
                Ok(activities) => {
                    let activities = filter_activities_by_privacy(activities, &s.config);
                    let sessions =
                        work_review_core::work_intelligence::build_work_sessions(&activities);
                    json!({
                        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&sessions).unwrap_or_default() }]
                    })
                }
                Err(e) => tool_error(&format!("获取会话失败: {e}")),
            }
        }),
        "analyze_intents" => with_policy_check(state, name, Permission::ReadSessions, |s| {
            let date = args["date"].as_str().unwrap_or("");
            match s.db.get_timeline(date, None, None) {
                Ok(activities) => {
                    let activities = filter_activities_by_privacy(activities, &s.config);
                    let intents = work_review_core::work_intelligence::analyze_intents(&activities);
                    json!({
                        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&intents.summary).unwrap_or_default() }]
                    })
                }
                Err(e) => tool_error(&format!("意图分析失败: {e}")),
            }
        }),
        "generate_report" => with_policy_check(state, name, Permission::WriteReport, |s| {
            let date = args["date"].as_str().unwrap_or("");
            let locale =
                work_review_core::analysis::AppLocale::from_option(args["locale"].as_str());

            // 坑2修复：用户开了 AI 增强（summary 模式）时，委托主应用 localhost API 生成 AI 报表。
            // 主应用有完整的 AI 调用链（尊重 ai_mode/工作时段/自定义 prompt/隐私过滤）。
            // 失败时回退本地模板。
            if s.config.ai_mode == work_review_core::config::AiMode::Summary {
                let locale_code = locale.as_code();
                // date 来自外部工具入参，必须做 query 编码，防止注入额外参数
                let path = format!(
                    "/v1/reports/generate?date={}&locale={}",
                    encode_query_param(date),
                    encode_query_param(locale_code)
                );
                if let Some(resp) = try_localhost_get_with_timeout(
                    &s.config,
                    &s.localhost_api_token_path,
                    &path,
                    s.config
                        .report_generation_timeout_secs
                        .saturating_add(LOCALHOST_GENERATE_TIMEOUT_BUFFER_SECS),
                ) {
                    if let Some(content) = resp.get("content").and_then(|c| c.as_str()) {
                        return json!({
                            "content": [{ "type": "text", "text": content }]
                        });
                    }
                }
                log::debug!("AI 模式报表委托 localhost API 失败，回退本地模板");
            }

            // 回退/默认：本地模板（使用用户配置的工作时段）
            match load_daily_stats_for_mcp(s, date) {
                Ok(stats) => {
                    let summary = work_review_core::analysis::generate_stats_summary_for_locale(
                        &stats,
                        locale,
                        &std::collections::HashMap::new(),
                    );
                    json!({
                        "content": [{ "type": "text", "text": format!("工作日报 - {}\n\n{}", date, summary) }]
                    })
                }
                Err(e) => tool_error(&format!("生成报告失败: {e}")),
            }
        }),
        "get_report" => with_policy_check(state, name, Permission::ReadReports, |s| {
            let date = args["date"].as_str().unwrap_or("");
            let locale = args["locale"].as_str();
            match s.db.get_report(date, locale) {
                Ok(Some(report)) => json!({
                    "content": [{ "type": "text", "text": report.content }]
                }),
                Ok(None) => tool_error(&format!("未找到 {date} 的报告")),
                Err(e) => tool_error(&format!("获取报告失败: {e}")),
            }
        }),
        "get_current_context" => with_policy_check(state, name, Permission::ReadActivities, |s| {
            // 坑1修复：先尝试委托 localhost API 拿真实前台窗口（主应用在跑时）
            if let Some(ctx) =
                try_localhost_get(&s.config, &s.localhost_api_token_path, "/v1/context")
            {
                let primary_app = ctx.get("primary_app").and_then(|v| v.as_str()).unwrap_or("");
                let window_title = ctx.get("window_title").and_then(|v| v.as_str()).unwrap_or("");
                let is_live = ctx.get("is_live").and_then(|v| v.as_bool()).unwrap_or(false);
                if !primary_app.is_empty() {
                    let context = json!({
                        "primary_app": primary_app,
                        "window_title": window_title,
                        "browser_url": ctx.get("browser_url"),
                        "recent_apps": ctx.get("recent_apps"),
                        "is_live": is_live,
                        "hint": format!("用户当前正在使用 {}{}", primary_app,
                            if window_title.is_empty() { String::new() }
                            else { format!("（{window_title}）") }),
                    });
                    return json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&context).unwrap_or_default()
                        }]
                    });
                }
            }

            // 回退：主应用未运行，读数据库最新记录（标注为历史数据）
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            match s.db.get_timeline(&today, Some(10), None) {
                Ok(activities) if !activities.is_empty() => {
                    let activities = filter_activities_by_privacy(activities, &s.config);
                    if activities.is_empty() {
                        return tool_error("暂无活动记录");
                    }
                    let current = &activities[0];
                    let primary_app = &current.app_name;
                    let mut duration_secs: i64 = current.duration;
                    for a in activities.iter().skip(1) {
                        if a.app_name == *primary_app {
                            duration_secs += a.duration;
                        } else {
                            break;
                        }
                    }
                    let mut recent_apps: Vec<&str> = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for a in &activities {
                        if seen.insert(a.app_name.as_str()) {
                            recent_apps.push(&a.app_name);
                            if recent_apps.len() >= 5 {
                                break;
                            }
                        }
                    }
                    let context = json!({
                        "primary_app": primary_app,
                        "window_title": current.window_title,
                        "category": current.category,
                        "duration_minutes": duration_secs / 60,
                        "browser_url": current.browser_url,
                        "recent_apps": recent_apps,
                        "is_live": false,
                        "source": "history",
                        "hint": format!("最近使用的应用是 {}（主应用未运行，数据来自历史记录）{}", primary_app,
                            if current.window_title.is_empty() { String::new() }
                            else { format!("（{}）", current.window_title) }),
                    });
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&context).unwrap_or_default()
                        }]
                    })
                }
                Ok(_) => tool_error("暂无活动记录"),
                Err(e) => tool_error(&format!("获取当前上下文失败: {e}")),
            }
        }),
        "get_device_status" => with_policy_check(state, name, Permission::ReadDeviceStatus, |s| {
            let audit_stats = s.policy.get_call_stats();
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&json!({
                        "status": "running",
                        "platform": std::env::consts::OS,
                        "arch": std::env::consts::ARCH,
                        "version": env!("CARGO_PKG_VERSION"),
                        "skills_count": s.skills.list_skills().len(),
                        "audit_summary": audit_stats,
                    })).unwrap_or_default()
                }]
            })
        }),
        "execute_skill" => with_policy_check(state, name, Permission::ExecuteSkill, |s| {
            let skill_id = args["skill_id"].as_str().unwrap_or("").to_string();
            let params_map: HashMap<String, Value> = args
                .get("params")
                .and_then(|p| serde_json::from_value(p.clone()).ok())
                .unwrap_or_default();

            let ctx = ExecutionContext {
                params: params_map,
                db_path: String::new(),
                ai_endpoint: Some(s.config.text_model.endpoint.clone()),
                ai_api_key: s.config.text_model.api_key.clone(),
                ai_model: Some(s.config.text_model.model.clone()),
                // MCP 客户端属外部调用方，默认拒绝含 Script 步骤的技能
                allow_script_skills: false,
            };

            let result = s.skills.execute(&skill_id, &ctx);
            let content_type = match result.content_type {
                OutputContentType::Text => "text",
                OutputContentType::Markdown => "markdown",
                OutputContentType::Json => "json",
            };
            json!({
                "content": [{
                    "type": "text",
                    "text": format!("Skill: {} | Type: {} | Duration: {}ms | Success: {}\n\n{}",
                        result.skill_id, content_type, result.duration_ms, result.success,
                        result.output)
                }],
                "isError": !result.success
            })
        }),
        "list_skills" => with_policy_check(state, name, Permission::ExecuteSkill, |s| {
            let skills: Vec<Value> = s
                .skills
                .list_skills()
                .iter()
                .map(|pkg| {
                    json!({
                        "id": pkg.id,
                        "name": pkg.name,
                        "description": pkg.description,
                        "category": format!("{:?}", pkg.category),
                        "enabled": pkg.enabled,
                        "version": pkg.version,
                        "adaptive_enabled": pkg.adaptive.enabled,
                    })
                })
                .collect();
            json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&skills).unwrap_or_default() }]
            })
        }),
        "get_skill_stats" => with_policy_check(state, name, Permission::ExecuteSkill, |s| {
            let skill_id = args["skill_id"].as_str();
            let stats = if let Some(id) = skill_id {
                match s.skills.get_skill_state(id) {
                    Some(state) => vec![(id, &state.stats)],
                    None => return tool_error(&format!("技能未找到: {id}")),
                }
            } else {
                s.skills.get_all_stats()
            };
            let stats_json: Vec<Value> = stats
                .iter()
                .map(|(id, stat)| {
                    json!({
                        "skill_id": id,
                        "total_executions": stat.total_executions,
                        "success_count": stat.success_count,
                        "failure_count": stat.failure_count,
                        "avg_duration_ms": stat.avg_duration_ms,
                        "last_executed_at": stat.last_executed_at,
                    })
                })
                .collect();
            json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&stats_json).unwrap_or_default() }]
            })
        }),
        _ => tool_error(&format!("未知工具: {name}")),
    }
}

fn resources_list() -> Vec<Value> {
    vec![
        json!({ "uri": "timeline/today", "name": "今日时间线", "description": "获取今天的活动时间线", "mimeType": "application/json" }),
        json!({ "uri": "sessions/current", "name": "当前工作会话", "description": "获取当前进行中的工作会话", "mimeType": "application/json" }),
        json!({ "uri": "stats/weekly", "name": "本周统计", "description": "获取本周工作统计数据", "mimeType": "application/json" }),
    ]
}

fn handle_resource_read(uri: &str, state: &Arc<Mutex<AppState>>) -> Value {
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // resources/read 必须与 tools/call 一样经过权限策略 + 脱敏。之前直接 get_timeline
    // 会把含 OCR 文本/窗口标题/截图路径的完整记录未经 PolicyEnforcer 过滤地返回给
    // MCP 客户端（AI 编码工具），绕过用户配置的脱敏规则。
    let permission = match uri {
        "timeline/today" => Permission::ReadActivities,
        "sessions/current" => Permission::ReadSessions,
        "stats/weekly" => Permission::ReadStats,
        _ => return resource_result(uri, &format!("Unknown resource: {uri}")),
    };
    let source = CallSource::McpTool {
        tool_name: format!("resource:{uri}"),
        client_id: None,
    };
    let need_sanitize = match s.policy.check_permission(&source, permission) {
        PolicyDecision::Deny => {
            return resource_result(uri, &format!("权限被拒绝: 无 {permission:?} 权限"))
        }
        PolicyDecision::Allow => false,
        PolicyDecision::AllowSanitized => true,
    };

    let raw_text = match uri {
        "timeline/today" => match s.db.get_timeline(&today, Some(50), None) {
            Ok(activities) => {
                let activities = filter_activities_by_privacy(activities, &s.config);
                serde_json::to_string_pretty(&activities).unwrap_or_default()
            }
            Err(e) => return resource_result(uri, &format!("Error: {e}")),
        },
        "sessions/current" => match s.db.get_timeline(&today, None, None) {
            Ok(activities) => {
                let activities = filter_activities_by_privacy(activities, &s.config);
                let sessions =
                    work_review_core::work_intelligence::build_work_sessions(&activities);
                serde_json::to_string_pretty(&sessions).unwrap_or_default()
            }
            Err(e) => return resource_result(uri, &format!("Error: {e}")),
        },
        "stats/weekly" => {
            let mut weekly = Vec::new();
            for i in 0..7 {
                let date = (chrono::Local::now() - chrono::Duration::days(i))
                    .format("%Y-%m-%d")
                    .to_string();
                if let Ok(stats) = load_daily_stats_for_mcp(&s, &date) {
                    weekly.push(json!({ "date": date, "total_duration": stats.total_duration, "screenshot_count": stats.screenshot_count }));
                }
            }
            serde_json::to_string_pretty(&weekly).unwrap_or_default()
        }
        _ => return resource_result(uri, &format!("Unknown resource: {uri}")),
    };

    // 必要时脱敏：解析 JSON → sanitize_value（删 screenshot_path/ocr_text、截断 window_title）→ 重新序列化
    let final_text = if need_sanitize {
        serde_json::from_str::<Value>(&raw_text)
            .map(|mut v| {
                sanitize_value(&mut v);
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw_text.clone())
            })
            .unwrap_or_else(|_| raw_text.clone())
    } else {
        raw_text
    };
    resource_result(uri, &final_text)
}

fn resource_result(uri: &str, text: &str) -> Value {
    json!({
        "contents": [{ "uri": uri, "mimeType": "application/json", "text": text }]
    })
}

fn prompts_list() -> Vec<Value> {
    vec![
        json!({ "name": "daily_review", "description": "每日工作回顾提示词", "arguments": [{ "name": "date", "description": "回顾日期，默认今天", "required": false }] }),
        json!({ "name": "weekly_summary", "description": "每周工作总结提示词", "arguments": [{ "name": "week_start", "description": "周一开始日期", "required": false }] }),
        json!({ "name": "project_time_audit", "description": "项目时间审计提示词", "arguments": [{ "name": "project", "description": "项目关键词", "required": true }] }),
    ]
}

fn handle_prompt_get(name: &str, args: &Value) -> Value {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let (desc, text) = match name {
        "daily_review" => {
            let date = args["date"].as_str().unwrap_or(&today);
            ("每日工作回顾", format!("请帮我回顾 {date} 的工作情况。先使用 get_daily_stats 获取统计数据，然后使用 query_timeline 获取时间线，最后给出今日工作总结和改进建议。"))
        }
        "weekly_summary" => {
            let week_start = args["week_start"].as_str().unwrap_or(&today);
            ("每周工作总结", format!("请帮我总结从 {week_start} 开始的这一周的工作。逐日获取统计数据，分析工作模式、效率变化，并给出下周建议。"))
        }
        "project_time_audit" => {
            let project = args["project"].as_str().unwrap_or("");
            ("项目时间审计", format!("请帮我审计项目「{project}」的时间投入。使用 search_activities 搜索相关活动，分析时间分布和效率，给出时间管理建议。"))
        }
        _ => ("未知提示词", String::new()),
    };
    json!({
        "description": desc,
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }]
    })
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    use work_review_core::privacy::{apply_excluded_domains_to_stats, apply_ignored_apps_to_stats};

    const TEST_LOCALHOST_API_TOKEN: &str =
        "wr-local-0123456789abcdef0123456789abcdef";
    const ROTATED_LOCALHOST_API_TOKEN: &str =
        "wr-local-fedcba9876543210fedcba9876543210";

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{unique}", std::process::id()))
    }

    #[test]
    fn token路径应跟随自定义配置文件目录() {
        let config_path = std::path::Path::new("/tmp/work-review-custom/config.json");
        assert_eq!(
            localhost_api_token_path(config_path),
            std::path::PathBuf::from("/tmp/work-review-custom/localhost_api_token.txt")
        );
    }

    #[test]
    fn token读取应去除空白并拒绝缺失读取失败或空内容() {
        let dir = unique_temp_dir("mcp-token");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let token_path = dir.join("localhost_api_token.txt");

        assert_eq!(read_localhost_api_token(&token_path), None);
        assert_eq!(read_localhost_api_token(&dir), None);

        std::fs::write(&token_path, format!("  {TEST_LOCALHOST_API_TOKEN}\n"))
            .expect("应写入 Token");
        assert_eq!(
            read_localhost_api_token(&token_path).as_deref(),
            Some(TEST_LOCALHOST_API_TOKEN)
        );

        std::fs::write(&token_path, " \n ").expect("应覆盖 Token");
        assert_eq!(read_localhost_api_token(&token_path), None);
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn token读取应拒绝可能污染请求头或日志的非法内容() {
        let dir = unique_temp_dir("mcp-invalid-token");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let token_path = dir.join("localhost_api_token.txt");

        for invalid_token in [
            "wr-local-secret",
            "wr-local-0123456789abcdef\n0123456789abcdef",
            "wr-local-0123456789abcdef\r0123456789abcdef",
            "wr-local-0123456789abcdef0123456789abcde中",
            "wr-local-0123456789abcdef0123456789abcdeg",
        ] {
            std::fs::write(&token_path, invalid_token).expect("应写入非法 Token");
            assert_eq!(
                read_localhost_api_token(&token_path),
                None,
                "非法 Token 不应进入 Authorization 请求头"
            );
        }

        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn localhost委托请求应携带bearer且url不含token() {
        let dir = unique_temp_dir("mcp-request");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let token_path = dir.join("localhost_api_token.txt");
        std::fs::write(&token_path, TEST_LOCALHOST_API_TOKEN).expect("应写入 Token");
        let mut config = AppConfig::default();
        config.localhost_api_enabled = true;
        config.localhost_api_host = Some("127.0.0.1".to_string());
        config.localhost_api_port = 47_831;

        let request = build_localhost_get_request(
            &config,
            &token_path,
            "/v1/context",
            LOCALHOST_QUICK_TIMEOUT_SECS,
        )
        .expect("有效配置和 Token 应构造请求");
        assert_eq!(request.method(), "GET");
        assert_eq!(
            request.header("Authorization"),
            Some("Bearer wr-local-0123456789abcdef0123456789abcdef")
        );
        assert_eq!(request.url(), "http://127.0.0.1:47831/v1/context");
        assert!(!request.url().contains(TEST_LOCALHOST_API_TOKEN));
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn localhost委托请求每次构造应重新读取token() {
        let dir = unique_temp_dir("mcp-token-rotation");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let token_path = dir.join("localhost_api_token.txt");
        let mut config = AppConfig::default();
        config.localhost_api_enabled = true;

        std::fs::write(&token_path, TEST_LOCALHOST_API_TOKEN).expect("应写入初始 Token");
        let first = build_localhost_get_request(
            &config,
            &token_path,
            "/v1/context",
            LOCALHOST_QUICK_TIMEOUT_SECS,
        )
        .expect("初始 Token 应构造请求");
        assert_eq!(
            first.header("Authorization"),
            Some("Bearer wr-local-0123456789abcdef0123456789abcdef")
        );

        std::fs::write(&token_path, ROTATED_LOCALHOST_API_TOKEN).expect("应轮换 Token");
        let second = build_localhost_get_request(
            &config,
            &token_path,
            "/v1/context",
            LOCALHOST_QUICK_TIMEOUT_SECS,
        )
        .expect("轮换后的 Token 应构造请求");
        assert_eq!(
            second.header("Authorization"),
            Some("Bearer wr-local-fedcba9876543210fedcba9876543210")
        );
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn localhost委托缺失或空token时不应构造请求() {
        let dir = unique_temp_dir("mcp-missing-token");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let token_path = dir.join("localhost_api_token.txt");
        let mut config = AppConfig::default();
        config.localhost_api_enabled = true;

        assert!(build_localhost_get_request(
            &config,
            &token_path,
            "/v1/context",
            LOCALHOST_QUICK_TIMEOUT_SECS
        )
        .is_none());

        std::fs::write(&token_path, " \n ").expect("应写入空 Token");
        assert!(build_localhost_get_request(
            &config,
            &token_path,
            "/v1/context",
            LOCALHOST_QUICK_TIMEOUT_SECS
        )
        .is_none());
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn tools_list_应包含已注册的12个工具() {
        let tools = tools_list();
        assert_eq!(tools.len(), 12);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
            .collect();
        for required in [
            "query_timeline",
            "get_daily_stats",
            "search_activities",
            "get_work_sessions",
            "analyze_intents",
            "generate_report",
            "get_report",
            "get_device_status",
            "execute_skill",
            "list_skills",
            "get_skill_stats",
            "get_current_context",
        ] {
            assert!(
                names.contains(&required),
                "缺少工具 {required}，实际：{names:?}"
            );
        }
    }

    #[test]
    fn resources_list_应包含3个资源() {
        let resources = resources_list();
        assert_eq!(resources.len(), 3);
        let uris: Vec<&str> = resources
            .iter()
            .filter_map(|r| r.get("uri").and_then(|v| v.as_str()))
            .collect();
        assert!(uris.contains(&"timeline/today"));
        assert!(uris.contains(&"sessions/current"));
        assert!(uris.contains(&"stats/weekly"));
    }

    #[test]
    fn prompts_list_应包含3个提示词模板() {
        let prompts = prompts_list();
        assert_eq!(prompts.len(), 3);
        let names: Vec<&str> = prompts
            .iter()
            .filter_map(|p| p.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"daily_review"));
        assert!(names.contains(&"weekly_summary"));
        assert!(names.contains(&"project_time_audit"));
    }

    #[test]
    fn tool_error_应返回标准错误格式() {
        let err = tool_error("拒绝访问");
        assert_eq!(err["isError"], json!(true));
        assert_eq!(err["content"][0]["type"], json!("text"));
        assert_eq!(err["content"][0]["text"], json!("拒绝访问"));
    }

    #[test]
    fn handle_prompt_get_应正确填充模板参数() {
        let res = handle_prompt_get("project_time_audit", &json!({ "project": "Work Review" }));
        assert_eq!(res["description"], json!("项目时间审计"));
        let text = res["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("Work Review"));
    }

    #[test]
    fn handle_prompt_get_对未知名称应返回空文本() {
        let res = handle_prompt_get("nonexistent", &json!({}));
        assert_eq!(res["description"], json!("未知提示词"));
        let text = res["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.is_empty());
    }

    #[test]
    fn apply_privacy_to_stats_应过滤忽略应用和排除域名() {
        let mut config = AppConfig::default();
        config
            .privacy
            .app_rules
            .push(work_review_core::config::AppPrivacyRule {
                app_name: "SecretApp".to_string(),
                level: work_review_core::config::PrivacyLevel::Ignored,
            });
        config.privacy.excluded_domains = vec!["secret.example.com".to_string()];

        let stats = DailyStats {
            total_duration: 180,
            work_time_duration: 180,
            app_usage: vec![
                work_review_core::database::AppUsage {
                    app_name: "SecretApp".to_string(),
                    duration: 60,
                    count: 1,
                    executable_path: None,
                    screenshot_url: None,
                },
                work_review_core::database::AppUsage {
                    app_name: "Code".to_string(),
                    duration: 120,
                    count: 1,
                    executable_path: None,
                    screenshot_url: None,
                },
            ],
            domain_usage: vec![
                work_review_core::database::DomainUsage {
                    domain: "secret.example.com".to_string(),
                    duration: 60,
                    semantic_category: None,
                    urls: vec![],
                },
                work_review_core::database::DomainUsage {
                    domain: "docs.example.com".to_string(),
                    duration: 120,
                    semantic_category: None,
                    urls: vec![],
                },
            ],
            browser_usage: vec![work_review_core::database::BrowserUsage {
                browser_name: "Google Chrome".to_string(),
                duration: 180,
                executable_path: None,
                domains: vec![
                    work_review_core::database::DomainUsage {
                        domain: "secret.example.com".to_string(),
                        duration: 60,
                        semantic_category: None,
                        urls: vec![],
                    },
                    work_review_core::database::DomainUsage {
                        domain: "docs.example.com".to_string(),
                        duration: 120,
                        semantic_category: None,
                        urls: vec![],
                    },
                ],
            }],
            ..Default::default()
        };

        let filtered = {
            let (ignored_apps, excluded_domains) = collect_privacy_filters(&config);
            apply_excluded_domains_to_stats(
                apply_ignored_apps_to_stats(stats, &ignored_apps),
                &excluded_domains,
            )
        };

        assert_eq!(filtered.total_duration, 120);
        assert_eq!(filtered.app_usage.len(), 1);
        assert_eq!(filtered.app_usage[0].app_name, "Code");
        assert_eq!(filtered.domain_usage.len(), 1);
        assert_eq!(filtered.domain_usage[0].domain, "docs.example.com");
        assert_eq!(filtered.browser_duration, 120);
        assert_eq!(filtered.browser_usage[0].domains.len(), 1);
    }

    #[test]
    fn filter_activities_by_privacy_应过滤忽略应用和排除域名() {
        let mut config = AppConfig::default();
        config
            .privacy
            .app_rules
            .push(work_review_core::config::AppPrivacyRule {
                app_name: "SecretApp".to_string(),
                level: work_review_core::config::PrivacyLevel::Ignored,
            });
        config.privacy.excluded_domains = vec!["secret.example.com".to_string()];

        let activity = |app_name: &str, browser_url: Option<&str>| Activity {
            id: None,
            timestamp: 1,
            app_name: app_name.to_string(),
            window_title: String::new(),
            screenshot_path: String::new(),
            ocr_text: None,
            category: "development".to_string(),
            duration: 60,
            browser_url: browser_url.map(str::to_string),
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };

        let filtered = filter_activities_by_privacy(
            vec![
                activity("SecretApp", None),
                activity("Google Chrome", Some("https://secret.example.com/a")),
                activity("Code", None),
            ],
            &config,
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].app_name, "Code");
    }

    #[test]
    fn sanitize_value_应移除screenshot_path() {
        let mut v = json!({
            "id": 1,
            "screenshot_path": "/Users/secret/screenshots/abc.png",
            "app_name": "Chrome"
        });
        sanitize_value(&mut v);
        assert!(v.get("screenshot_path").is_none());
        assert_eq!(v["app_name"], json!("Chrome"));
        assert_eq!(v["id"], json!(1));
    }

    #[test]
    fn sanitize_value_应移除ocr_text() {
        let mut v = json!({
            "id": 1,
            "ocr_text": "password: hunter2",
            "app_name": "Chrome"
        });
        sanitize_value(&mut v);
        assert!(v.get("ocr_text").is_none());
        assert_eq!(v["app_name"], json!("Chrome"));
    }

    #[test]
    fn sanitize_value_应截断过长的window_title() {
        let long_title = "a".repeat(80);
        let mut v = json!({ "window_title": long_title });
        sanitize_value(&mut v);
        let truncated = v["window_title"].as_str().unwrap();
        assert!(truncated.ends_with('…'));
        // 40 字 + 省略号
        assert_eq!(truncated.chars().count(), 41);
    }

    #[test]
    fn sanitize_value_短window_title不应被截断或加省略号() {
        let mut v = json!({ "window_title": "短标题.txt" });
        sanitize_value(&mut v);
        assert_eq!(v["window_title"], json!("短标题.txt"));
    }

    #[test]
    fn sanitize_value_应递归处理嵌套结构() {
        let mut v = json!({
            "results": [
                { "ocr_text": "secret1", "title": "ok" },
                { "ocr_text": "secret2", "title": "ok" }
            ],
            "meta": { "screenshot_path": "/path/a.png" }
        });
        sanitize_value(&mut v);
        for item in v["results"].as_array().unwrap() {
            assert!(item.get("ocr_text").is_none());
        }
        assert!(v["meta"].get("screenshot_path").is_none());
    }

    #[test]
    fn sanitize_result_应解析text字段里的json再脱敏() {
        // 模拟 MCP tool 返回结构：content 数组里每个 item.text 是 JSON 字符串
        let inner = json!([
            { "id": 1, "screenshot_path": "/sensitive.png", "ocr_text": "leak" }
        ]);
        let mut result = json!({
            "content": [
                { "type": "text", "text": serde_json::to_string_pretty(&inner).unwrap() }
            ]
        });
        sanitize_result(&mut result);
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert!(parsed[0].get("screenshot_path").is_none());
        assert!(parsed[0].get("ocr_text").is_none());
        assert_eq!(parsed[0]["id"], json!(1));
    }

    #[test]
    fn sanitize_result_对非json的text字段应保持不变() {
        let original = "纯文本日报内容\n\n今日完成 3 项工作";
        let mut result = json!({
            "content": [
                { "type": "text", "text": original }
            ]
        });
        sanitize_result(&mut result);
        assert_eq!(result["content"][0]["text"].as_str().unwrap(), original);
    }
}
