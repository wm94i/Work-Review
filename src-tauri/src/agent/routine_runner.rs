//! 个人 Agent 自主定时任务执行器与后台调度循环（Heartbeat / Routines）。
//!
//! 安全不变量：
//! - 始终以只读沙箱运行（`actions = None`、`confirm = None`、`mcp_bridge = None`、`web_tools = None`），
//!   绝不未经用户确认修改本地状态或调用外部 MCP。
//! - **云模型外发门控**：默认（`allow_cloud_model == false`）仅允许本机回环模型（如 Ollama / LM Studio）
//!   或纯本地统计模板，绝不静默将本机工作记录发送给远程云模型；只有用户针对该任务显式开启 `allow_cloud_model`
//!   后，才允许使用已配置的云端助手模型。
//! - **后台调度二次校验**：定时入口在获取执行信号量并重新加载磁盘配置后，再次确认 `enabled && is_routine_due`，
//!   防止用户在上一任务运行期间关闭任务后仍误触发一次。
//! - 每次运行以 `source = "routine:<id>"` 记入 SQLite `agent_runs` / `agent_run_events` / `agent_tool_audit`。

use crate::agent::deadline::Deadline;
use crate::agent::model::is_local_only_model_endpoint;
use crate::agent::{AssistantRequestClassification, AssistantRuntime, Orchestrator};
use crate::AppState;
use chrono::Local;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use work_review_agent_harness::context::ArtifactStore;
use work_review_agent_harness::ledger::{redact_secrets, scrub_preview};
use work_review_agent_harness::routine::{
    is_routine_due, load_routines_from_dir, record_routine_execution, AgentRoutine,
};
use work_review_agent_harness::skills::SkillCatalog;
use work_review_core::config::AiTaskKind;
use work_review_core::error::AppError;
use work_review_core::privacy::{PrivacyAction, PrivacyFilter, ANONYMIZED_TITLE_PLACEHOLDER};

const ROUTINE_POLL_INTERVAL_SECS: u64 = 60;
const ROUTINE_TIMEOUT_SECS: u64 = 90;
const ROUTINE_SYSTEM_PROMPT: &str = "你是 Work Review 的后台定时工作复盘代理（只读模式）。\
请优先调用本机工作记录工具核实真实活动、项目工时、待办或日报，不要编造事实，也不要尝试执行写操作。\
若任务指定了个人技能，请先调用 load_skill 读取技能步骤再按要求输出简洁、结构化的复盘结果。";

static ROUTINE_EXECUTION_SEMAPHORE: Lazy<Semaphore> = Lazy::new(|| Semaphore::new(1));

/// 单次自主任务执行的返回结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineRunOutcome {
    pub routine_id: String,
    pub status: String,
    pub run_id: Option<String>,
    pub summary: String,
}

fn should_skip_scheduled_run(
    routine: &AgentRoutine,
    from_scheduler: bool,
    now: chrono::DateTime<Local>,
) -> bool {
    from_scheduler && (!routine.enabled || !is_routine_due(routine, now))
}

fn sanitize_routine_summary(privacy_filter: &PrivacyFilter, raw: &str) -> String {
    if privacy_filter.check_privacy("", raw) != PrivacyAction::Record {
        return ANONYMIZED_TITLE_PLACEHOLDER.to_string();
    }
    scrub_preview(&privacy_filter.filter_text(&redact_secrets(raw)), 240)
}

fn build_routine_prompt(routine: &AgentRoutine) -> String {
    if let Some(skill) = routine
        .skill_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        format!(
            "请先调用 load_skill(name=\"{skill}\") 读取个人技能说明，再执行以下定时任务：\n\n{}",
            routine.prompt
        )
    } else {
        routine.prompt.clone()
    }
}

/// 立即执行指定 ID 的自主任务（供前端「立即运行」按钮调用，允许手动测试已暂停的任务）。
pub async fn execute_routine_by_id(
    state: &Arc<Mutex<AppState>>,
    routine_id: &str,
) -> Result<RoutineRunOutcome, AppError> {
    execute_routine_internal(state, routine_id, false).await
}

async fn execute_routine_internal(
    state: &Arc<Mutex<AppState>>,
    routine_id: &str,
    from_scheduler: bool,
) -> Result<RoutineRunOutcome, AppError> {
    let trimmed_id = routine_id.trim().to_string();
    let _permit = ROUTINE_EXECUTION_SEMAPHORE
        .try_acquire()
        .map_err(|_| AppError::Unknown("已有自主任务正在执行，请稍后再试".to_string()))?;

    let (
        database,
        data_dir,
        ignored_apps,
        excluded_domains,
        privacy_config,
        projects,
        avatar_followups,
        candidate_model,
    ) = {
        let guard = state
            .lock()
            .map_err(|e| AppError::Unknown(format!("读取应用状态失败: {e}")))?;
        let (ignored_apps, excluded_domains) = crate::commands::collect_privacy_filters(&guard);
        let candidate = guard.config.resolve_model_for_task(AiTaskKind::Assistant);
        let candidate_model =
            if !candidate.endpoint.trim().is_empty() && !candidate.model.trim().is_empty() {
                Some(candidate)
            } else {
                None
            };
        (
            guard.database.clone(),
            guard.data_dir.clone(),
            ignored_apps,
            excluded_domains,
            guard.config.privacy.clone(),
            guard.config.projects.clone(),
            guard.config.avatar_followups.clone(),
            candidate_model,
        )
    };

    let data_dir_for_load = data_dir.clone();
    let id_for_load = trimmed_id.clone();
    let (routine, skill_catalog) = tokio::task::spawn_blocking(move || {
        let routine = load_routines_from_dir(&data_dir_for_load)
            .into_iter()
            .find(|r| r.id == id_for_load);
        let skills_dir = data_dir_for_load.join("skills");
        let catalog = SkillCatalog::load_from_dir(&skills_dir);
        let catalog = (!catalog.is_empty()).then(|| Arc::new(catalog));
        (routine, catalog)
    })
    .await
    .map_err(|e| AppError::Unknown(format!("加载自主任务配置失败: {e}")))?;

    let Some(routine) = routine else {
        return Err(AppError::Unknown(format!("未找到自主任务: {trimmed_id}")));
    };

    // 后台调度入口在取得执行许可并重新加载最新配置后，二次校验启用状态与到期条件，
    // 防止用户在前一个任务运行期间禁用该任务后仍被误执行一次。
    if should_skip_scheduled_run(&routine, from_scheduler, Local::now()) {
        return Ok(RoutineRunOutcome {
            routine_id: routine.id,
            status: "skipped".to_string(),
            run_id: routine.last_run_id,
            summary: "任务已禁用或当前未到期，已跳过本次调度".to_string(),
        });
    }

    let privacy_filter = Arc::new(PrivacyFilter::from_config(&privacy_config));

    // 过滤未填全 endpoint/model 的半配置模型（与 Orchestrator::handle 的 has_model 判定保持一致），
    // 确保 effective_model.is_some() 时 Orchestrator 必然进入 QueryPath::Agent，不会误走 FastPath。
    let configured_model =
        candidate_model.filter(|m| !m.endpoint.trim().is_empty() && !m.model.trim().is_empty());

    // 默认禁止定时任务把本机工作记录发往云模型；仅本机回环模型或用户显式勾选 allow_cloud_model 时启用 AI 模型。
    let cloud_model_blocked = configured_model
        .as_ref()
        .is_some_and(|m| !routine.allow_cloud_model && !is_local_only_model_endpoint(&m.endpoint));
    let effective_model = configured_model
        .filter(|m| routine.allow_cloud_model || is_local_only_model_endpoint(&m.endpoint));

    // 绑定了个人技能的自主任务无法仅凭规则 FastPath 执行，缺少可用 AI 模型时明确失败，避免静默误报成功。
    if effective_model.is_none() && routine.skill_name.is_some() {
        let ran_at = chrono::Utc::now().timestamp();
        let reason = if cloud_model_blocked {
            "该自主任务绑定了个人技能，但仅允许本机模型且当前助手模型为云端模型；请在任务设置中允许云端模型或切换为本机回环模型。"
        } else {
            "该自主任务绑定了个人技能，但尚未配置可用的 AI 模型，无法执行技能步骤。"
        };
        let status = "failed".to_string();
        let summary = sanitize_routine_summary(&privacy_filter, reason);
        let data_dir_for_save = data_dir.clone();
        let id_for_save = routine.id.clone();
        let status_for_save = status.clone();
        let summary_for_save = summary.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Err(err) = record_routine_execution(
                &data_dir_for_save,
                &id_for_save,
                None,
                &status_for_save,
                Some(&summary_for_save),
                ran_at,
            ) {
                log::warn!("记录自主任务执行快照失败 ({id_for_save}): {err}");
            }
        })
        .await;
        return Ok(RoutineRunOutcome {
            routine_id: routine.id,
            status,
            run_id: None,
            summary,
        });
    }

    let source_tag = format!("routine:{}", routine.id);
    let runtime = AssistantRuntime {
        avatar_followups,
        projects,
        actions: None,
        confirm: None,
        current_context: None,
        semantic_search: None,
        cancel: None,
        privacy_filter: Some(privacy_filter.clone()),
        client_request_id: None,
        conversation_id: None,
        run_source: Some(source_tag.clone()),
        has_recalled_user_memory: false,
        artifacts: Some(ArtifactStore::new()),
        skill_catalog,
        mcp_bridge: None,
        context_budget: None,
    };

    let prompt = build_routine_prompt(&routine);
    let run_started_at = chrono::Utc::now().timestamp();
    let deadline = Deadline::after(Duration::from_secs(ROUTINE_TIMEOUT_SECS));

    let outcome = crate::agent::tools::with_user_memory_tool_capabilities(
        crate::agent::tools::UserMemoryToolCapabilities::default(),
        Orchestrator::handle(
            &prompt,
            AssistantRequestClassification::work_review(false),
            effective_model.as_ref(),
            &database,
            &[],
            Some(ROUTINE_SYSTEM_PROMPT),
            &ignored_apps,
            &excluded_domains,
            None,
            runtime,
            None,
            deadline,
        ),
    )
    .await;

    let ran_at = chrono::Utc::now().timestamp();

    // 读取本次执行落盘的最新对应 source 的账本记录（纯本地 FastPath 无 LLM 轮次时返回 None）
    let db_for_lookup = database.clone();
    let source_for_lookup = source_tag.clone();
    let last_run = tokio::task::spawn_blocking(move || {
        db_for_lookup
            .list_agent_runs(None, 50)
            .ok()
            .and_then(|runs| {
                runs.into_iter()
                    .find(|r| r.source == source_for_lookup && r.started_at >= run_started_at)
            })
    })
    .await
    .ok()
    .flatten();
    let last_run_id = last_run.as_ref().map(|r| r.id.clone());
    let ledger_status = last_run.as_ref().map(|r| r.status.as_str());

    let (status, summary) = resolve_routine_outcome(
        outcome,
        effective_model.is_some(),
        cloud_model_blocked,
        ledger_status,
        &privacy_filter,
    );

    let data_dir_for_save = data_dir.clone();
    let id_for_save = routine.id.clone();
    let run_id_for_save = last_run_id.clone();
    let status_for_save = status.clone();
    let summary_for_save = summary.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Err(err) = record_routine_execution(
            &data_dir_for_save,
            &id_for_save,
            run_id_for_save.as_deref(),
            &status_for_save,
            Some(&summary_for_save),
            ran_at,
        ) {
            log::warn!("记录自主任务执行快照失败 ({id_for_save}): {err}");
        }
    })
    .await;

    Ok(RoutineRunOutcome {
        routine_id: routine.id,
        status,
        run_id: last_run_id,
        summary,
    })
}

/// 后台定时调度任务：每 60 秒扫描一次到期自主任务并按序在只读沙箱内执行。
pub async fn agent_routine_scheduler_task(state: Arc<Mutex<AppState>>) {
    // 启动后延迟 30 秒再首次扫描，避免与应用启动期 I/O 争抢资源。
    tokio::time::sleep(Duration::from_secs(30)).await;
    let mut ticker = tokio::time::interval(Duration::from_secs(ROUTINE_POLL_INTERVAL_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        let data_dir = match state.lock() {
            Ok(guard) => guard.data_dir.clone(),
            Err(_) => continue,
        };
        let due_ids: Vec<String> = tokio::task::spawn_blocking(move || {
            let now = Local::now();
            load_routines_from_dir(&data_dir)
                .into_iter()
                .filter(|r| is_routine_due(r, now))
                .map(|r| r.id)
                .collect()
        })
        .await
        .unwrap_or_default();

        for routine_id in due_ids {
            match execute_routine_internal(&state, &routine_id, true).await {
                Ok(outcome) => {
                    log::info!(
                        "自主任务 {} 调度结果: status={} run_id={:?}",
                        outcome.routine_id,
                        outcome.status,
                        outcome.run_id
                    );
                }
                Err(err) => {
                    log::warn!("自主任务 {} 调度跳过或失败: {err}", routine_id);
                }
            }
        }
    }
}

/// 综合 Orchestrator 返回值、是否期望使用 AI 及账本终态，判定自主任务最终状态与摘要。
fn resolve_routine_outcome(
    outcome: Result<crate::agent::orchestrator::OrchestratorResult, AppError>,
    expected_ai: bool,
    cloud_model_blocked: bool,
    ledger_status: Option<&str>,
    privacy_filter: &PrivacyFilter,
) -> (String, String) {
    match outcome {
        Err(err) => (
            "failed".to_string(),
            sanitize_routine_summary(privacy_filter, &format!("自主任务执行失败: {err}")),
        ),
        Ok(res) => {
            if expected_ai && !res.used_ai {
                // Orchestrator 在 AI 执行失败时会降级为本地统计并返回 Ok(used_ai=false)；
                // 对自主任务而言这属于执行失败，不得误报 completed。
                return (
                    "failed".to_string(),
                    sanitize_routine_summary(privacy_filter, &res.answer),
                );
            }
            let status = match ledger_status {
                Some("timed_out") => "timed_out",
                Some("failed") => "failed",
                Some("cancelled") => "cancelled",
                _ => "completed",
            };
            let raw_summary = if !expected_ai && cloud_model_blocked {
                format!("[仅本机统计：未授权云端模型] {}", res.answer)
            } else {
                res.answer
            };
            (
                status.to_string(),
                sanitize_routine_summary(privacy_filter, &raw_summary),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 仅本机回环模型端点应被视为本地不外发() {
        assert!(is_local_only_model_endpoint("http://localhost:11434"));
        assert!(is_local_only_model_endpoint("http://127.0.0.1:1234/v1"));
        assert!(is_local_only_model_endpoint("http://[::1]:11434/v1"));
        assert!(is_local_only_model_endpoint(
            "http://[::ffff:127.0.0.1]:11434"
        ));
        assert!(!is_local_only_model_endpoint("https://api.openai.com/v1"));
        assert!(!is_local_only_model_endpoint("http://192.168.1.50:11434"));
        assert!(!is_local_only_model_endpoint(
            "http://localhost@evil.example/v1"
        ));
        assert!(!is_local_only_model_endpoint(""));
    }

    #[test]
    fn 后台调度二次校验应跳过已禁用任务而手动测试放行() {
        let mut routine = AgentRoutine {
            id: "r1".to_string(),
            name: "测试复盘".to_string(),
            prompt: "汇总今日记录".to_string(),
            skill_name: None,
            schedule: work_review_agent_harness::routine::RoutineSchedule::IntervalMinutes {
                every_minutes: 60,
            },
            enabled: false,
            allow_cloud_model: false,
            last_run_at: None,
            last_run_id: None,
            last_status: None,
            last_summary: None,
        };
        let now = Local::now();
        assert!(should_skip_scheduled_run(&routine, true, now));
        assert!(!should_skip_scheduled_run(&routine, false, now));

        routine.enabled = true;
        assert!(!should_skip_scheduled_run(&routine, true, now));
    }

    #[test]
    fn 自主任务摘要落盘应遵循敏感词与凭据脱敏() {
        let mut privacy = work_review_core::config::PrivacyConfig::default();
        privacy.excluded_keywords.push("SecretAlpha".to_string());
        let filter = PrivacyFilter::from_config(&privacy);

        assert_eq!(
            sanitize_routine_summary(&filter, "今日完成 SecretAlpha 部署"),
            ANONYMIZED_TITLE_PLACEHOLDER
        );
        let safe = sanitize_routine_summary(&filter, "完成复盘 Bearer tok_1234567890abcdef");
        assert!(safe.contains("完成复盘"));
        assert!(!safe.contains("tok_1234567890abcdef"));
        let safe = sanitize_routine_summary(&filter, r#"{"token":"alpha beta gamma"}"#);
        for part in ["alpha", "beta", "gamma"] {
            assert!(!safe.contains(part), "定时任务摘要残留凭证: {safe}");
        }
    }

    #[test]
    fn 自主任务结果判定应识别ai降级失败超时与未授权云模型提示() {
        use crate::agent::orchestrator::OrchestratorResult;

        let filter =
            PrivacyFilter::from_config(&work_review_core::config::PrivacyConfig::default());

        // 1. 期望 AI 执行但 Orchestrator 降级返回 used_ai=false → 必须记为 failed
        let (st, summary) = resolve_routine_outcome(
            Ok(OrchestratorResult {
                answer: "⚠️ AI 模型调用失败，已降级为本地统计".to_string(),
                used_ai: false,
                tool_labels: vec!["降级查询".to_string()],
                references: vec![],
            }),
            true,
            false,
            Some("failed"),
            &filter,
        );
        assert_eq!(st, "failed");
        assert!(summary.contains("AI 模型调用失败"));

        // 1b. 即使账本未命中（如账本写盘降级），只要 expected_ai=true（已保证必走 AgentPath）且 used_ai=false 仍应判定为 failed
        let (st, _) = resolve_routine_outcome(
            Ok(OrchestratorResult {
                answer: "⚠️ AI 模型调用失败，已降级为本地统计".to_string(),
                used_ai: false,
                tool_labels: vec!["降级查询".to_string()],
                references: vec![],
            }),
            true,
            false,
            None,
            &filter,
        );
        assert_eq!(st, "failed");

        // 2. AI 正常返回但账本标记 timed_out → 透传 timed_out
        let (st, _) = resolve_routine_outcome(
            Ok(OrchestratorResult {
                answer: "处理超时，请尝试更具体的问题。".to_string(),
                used_ai: true,
                tool_labels: vec![],
                references: vec![],
            }),
            true,
            false,
            Some("timed_out"),
            &filter,
        );
        assert_eq!(st, "timed_out");

        // 3. 本地统计模式且因未授权云模型而跳过云模型 → completed 且摘要显式提示
        let (st, summary) = resolve_routine_outcome(
            Ok(OrchestratorResult {
                answer: "今日工作 4 小时".to_string(),
                used_ai: false,
                tool_labels: vec!["rule-based".to_string()],
                references: vec![],
            }),
            false,
            true,
            None,
            &filter,
        );
        assert_eq!(st, "completed");
        assert!(summary.starts_with("[仅本机统计：未授权云端模型]"));

        // 3b. 纯本地模式（未配置任何云模型）→ completed 且不附加未授权云模型提示
        let (st, summary) = resolve_routine_outcome(
            Ok(OrchestratorResult {
                answer: "今日工作 4 小时".to_string(),
                used_ai: false,
                tool_labels: vec!["rule-based".to_string()],
                references: vec![],
            }),
            false,
            false,
            None,
            &filter,
        );
        assert_eq!(st, "completed");
        assert_eq!(summary, "今日工作 4 小时");

        // 4. 正常 AI 完成 → completed
        let (st, summary) = resolve_routine_outcome(
            Ok(OrchestratorResult {
                answer: "本周复盘完成".to_string(),
                used_ai: true,
                tool_labels: vec!["weekly_review".to_string()],
                references: vec![],
            }),
            true,
            false,
            Some("completed"),
            &filter,
        );
        assert_eq!(st, "completed");
        assert_eq!(summary, "本周复盘完成");
    }
}
