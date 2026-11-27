//! Personal Agent Harness 可观测性、个人技能、自主定时任务与外部 MCP Client 管理命令。
//!
//! 所有磁盘与数据库 I/O 均在释放主状态锁之后通过 `spawn_blocking` 执行；
//! 外部 MCP Client 的网络探测使用异步 HTTP 客户端并严格执行 SSRF 防护。

use crate::agent::mcp_bridge::{test_mcp_server_connection, McpServerTestResult};
use crate::agent::routine_runner::{execute_routine_by_id, RoutineRunOutcome};
use crate::AppState;
use std::sync::{Arc, Mutex};
use tauri::State;
use work_review_agent_harness::ledger::{AgentRunSummary, AgentRunTrace};
use work_review_agent_harness::mcp_client::{
    delete_mcp_server_in_dir, load_mcp_servers_from_dir, save_mcp_server_in_dir,
    McpClientServerView, SaveMcpClientServerInput,
};
use work_review_agent_harness::routine::{
    delete_routine_in_dir, load_routines_from_dir, save_routine_in_dir, AgentRoutine,
    SaveRoutineInput,
};
use work_review_agent_harness::skills::{
    delete_skill_from_dir, read_skill_detail, save_skill_to_dir, SkillCatalog, SkillDetail,
    SkillMetadata,
};
use work_review_core::error::AppError;

/// 列出最近的 Agent Run 概要（可选按会话 ID 过滤）。
#[tauri::command]
pub async fn list_agent_runs(
    conversation_id: Option<i64>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<AgentRunSummary>, AppError> {
    let database = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.database.clone()
    };
    let capped_limit = limit.unwrap_or(50).clamp(1, 200) as usize;
    tokio::task::spawn_blocking(move || database.list_agent_runs(conversation_id, capped_limit))
        .await
        .map_err(|e| AppError::Unknown(format!("读取 Agent 运行记录任务失败: {e}")))?
}

/// 读取单次 Agent Run 的完整 Trace（概要 + 有序事件流 + 工具权限审计）。
#[tauri::command]
pub async fn get_agent_run_trace(
    run_id: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<AgentRunTrace>, AppError> {
    let trimmed = run_id.trim().to_string();
    if trimmed.is_empty() {
        return Err(AppError::Unknown("run_id 不能为空".to_string()));
    }
    let database = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.database.clone()
    };
    tokio::task::spawn_blocking(move || database.get_agent_run_trace(&trimmed))
        .await
        .map_err(|e| AppError::Unknown(format!("读取 Agent Trace 任务失败: {e}")))?
}

/// 列出本机 `<data_dir>/skills/*/SKILL.md` 已安装的个人技能元数据。
#[tauri::command]
pub async fn list_personal_skills(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<SkillMetadata>, AppError> {
    let skills_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.join("skills")
    };
    tokio::task::spawn_blocking(move || SkillCatalog::load_from_dir(&skills_dir).list_metadata())
        .await
        .map_err(|e| AppError::Unknown(format!("扫描个人技能目录失败: {e}")))
}

/// 读取单个个人技能的完整详情（含正文）供前端查看或编辑。
#[tauri::command]
pub async fn get_personal_skill(
    name: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<SkillDetail, AppError> {
    let skills_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.join("skills")
    };
    tokio::task::spawn_blocking(move || read_skill_detail(&skills_dir, &name))
        .await
        .map_err(|e| AppError::Unknown(format!("读取个人技能任务失败: {e}")))?
        .map_err(AppError::Unknown)
}

/// 创建或更新一个个人技能，并返回刷新后的全部技能列表。
#[tauri::command]
pub async fn save_personal_skill(
    name: String,
    description: String,
    allowed_tools: Vec<String>,
    body: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<SkillMetadata>, AppError> {
    let skills_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.join("skills")
    };
    tokio::task::spawn_blocking(move || {
        save_skill_to_dir(&skills_dir, &name, &description, &allowed_tools, &body)?;
        Ok::<Vec<SkillMetadata>, String>(SkillCatalog::load_from_dir(&skills_dir).list_metadata())
    })
    .await
    .map_err(|e| AppError::Unknown(format!("保存个人技能任务失败: {e}")))?
    .map_err(AppError::Unknown)
}

/// 删除指定名称的个人技能，并返回刷新后的全部技能列表。
#[tauri::command]
pub async fn delete_personal_skill(
    name: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<SkillMetadata>, AppError> {
    let skills_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.join("skills")
    };
    tokio::task::spawn_blocking(move || {
        delete_skill_from_dir(&skills_dir, &name)?;
        Ok::<Vec<SkillMetadata>, String>(SkillCatalog::load_from_dir(&skills_dir).list_metadata())
    })
    .await
    .map_err(|e| AppError::Unknown(format!("删除个人技能任务失败: {e}")))?
    .map_err(AppError::Unknown)
}

/// 在系统文件管理器中打开 `<data_dir>/skills` 目录（不存在时先安全创建，拒绝符号链接）。
#[tauri::command]
pub async fn open_personal_skills_dir(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let skills_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.join("skills")
    };

    tokio::task::spawn_blocking(move || -> Result<String, AppError> {
        match std::fs::symlink_metadata(&skills_dir) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(AppError::Unknown(
                        "skills 目录为符号链接或非法路径，拒绝打开".to_string(),
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&skills_dir)
                    .map_err(|e| AppError::Unknown(format!("创建个人技能目录失败: {e}")))?;
            }
            Err(err) => {
                return Err(AppError::Unknown(format!("检查个人技能目录失败: {err}")));
            }
        }

        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(&skills_dir)
                .spawn()
                .map_err(|e| AppError::Unknown(format!("打开个人技能目录失败: {e}")))?;
        }

        #[cfg(target_os = "windows")]
        {
            // explorer.exe 成功打开目录后也可能返回非零退出码，因此仅使用 spawn() 不检查退出状态。
            std::process::Command::new("explorer")
                .arg(&skills_dir)
                .spawn()
                .map_err(|e| AppError::Unknown(format!("打开个人技能目录失败: {e}")))?;
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            std::process::Command::new("xdg-open")
                .arg(&skills_dir)
                .spawn()
                .map_err(|e| {
                    AppError::Unknown(format!(
                        "打开个人技能目录失败 ({}): {e}",
                        skills_dir.display()
                    ))
                })?;
        }

        Ok(skills_dir.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| AppError::Unknown(format!("打开个人技能目录任务失败: {e}")))?
}

/// 列出所有已配置的自主定时任务。
#[tauri::command]
pub async fn list_agent_routines(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<AgentRoutine>, AppError> {
    let data_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.clone()
    };
    tokio::task::spawn_blocking(move || load_routines_from_dir(&data_dir))
        .await
        .map_err(|e| AppError::Unknown(format!("读取自主任务列表失败: {e}")))
}

/// 创建或更新一条自主定时任务，返回最新列表。
#[tauri::command]
pub async fn save_agent_routine(
    input: SaveRoutineInput,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<AgentRoutine>, AppError> {
    let data_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.clone()
    };
    tokio::task::spawn_blocking(move || save_routine_in_dir(&data_dir, input))
        .await
        .map_err(|e| AppError::Unknown(format!("保存自主任务失败: {e}")))?
        .map_err(AppError::Unknown)
}

/// 删除一条自主定时任务，返回最新列表。
#[tauri::command]
pub async fn delete_agent_routine(
    id: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<AgentRoutine>, AppError> {
    let data_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.clone()
    };
    tokio::task::spawn_blocking(move || delete_routine_in_dir(&data_dir, &id))
        .await
        .map_err(|e| AppError::Unknown(format!("删除自主任务失败: {e}")))?
        .map_err(AppError::Unknown)
}

/// 立即在只读沙箱内执行指定自主任务一次，返回摘要与 Run ID。
#[tauri::command]
pub async fn run_agent_routine_now(
    id: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<RoutineRunOutcome, AppError> {
    execute_routine_by_id(state.inner(), &id).await
}

/// 列出已配置的外部 HTTP MCP Server（永远屏蔽明文 Token）。
#[tauri::command]
pub async fn list_mcp_client_servers(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<McpClientServerView>, AppError> {
    let data_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.clone()
    };
    tokio::task::spawn_blocking(move || {
        load_mcp_servers_from_dir(&data_dir)
            .iter()
            .map(McpClientServerView::from)
            .collect()
    })
    .await
    .map_err(|e| AppError::Unknown(format!("读取外部 MCP 服务列表失败: {e}")))
}

/// 创建或更新一个外部 HTTP MCP Server（保存前校验目标地址合规性），返回脱敏视图列表。
#[tauri::command]
pub async fn save_mcp_client_server(
    input: SaveMcpClientServerInput,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<McpClientServerView>, AppError> {
    let (data_dir, self_port) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (guard.data_dir.clone(), guard.config.localhost_api_port)
    };
    let _ = crate::agent::mcp_bridge::validate_mcp_endpoint_target(
        &input.endpoint,
        input.allow_localhost,
        Some(self_port),
    )
    .map_err(AppError::Unknown)?;

    tokio::task::spawn_blocking(move || save_mcp_server_in_dir(&data_dir, input))
        .await
        .map_err(|e| AppError::Unknown(format!("保存外部 MCP 服务失败: {e}")))?
        .map_err(AppError::Unknown)
}

/// 删除一个外部 HTTP MCP Server，返回脱敏视图列表。
#[tauri::command]
pub async fn delete_mcp_client_server(
    id: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<McpClientServerView>, AppError> {
    let data_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.clone()
    };
    tokio::task::spawn_blocking(move || delete_mcp_server_in_dir(&data_dir, &id))
        .await
        .map_err(|e| AppError::Unknown(format!("删除外部 MCP 服务失败: {e}")))?
        .map_err(AppError::Unknown)
}

/// 测试指定已配置外部 MCP Server 的连通性与可用工具清单。
#[tauri::command]
pub async fn test_mcp_client_server(
    id: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<McpServerTestResult, AppError> {
    let (data_dir, self_port) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (guard.data_dir.clone(), guard.config.localhost_api_port)
    };
    let trimmed_id = id.trim().to_string();
    let server = tokio::task::spawn_blocking(move || {
        load_mcp_servers_from_dir(&data_dir)
            .into_iter()
            .find(|s| s.id == trimmed_id)
    })
    .await
    .map_err(|e| AppError::Unknown(format!("读取外部 MCP 服务失败: {e}")))?
    .ok_or_else(|| AppError::Unknown("未找到指定的外部 MCP 服务".to_string()))?;

    Ok(test_mcp_server_connection(&server, Some(self_port)).await)
}
