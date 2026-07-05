//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::config::ModelConfig;
use crate::database::MemorySearchItem;
use crate::error::AppError;
use crate::work_intelligence::{analyze_intents, build_work_sessions, extract_todos, generate_weekly_review as build_weekly_review, IntentAnalysisResult, TodoExtractionResult, WeeklyReviewResult, WorkSession};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{State};

use super::shared::{load_filtered_activities_in_range, merge_manual_followups_into_todos};
use super::ask::{format_browser_url_for_display, generate_text_answer_with_model};
use super::ai::is_text_model_available;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAnswer {
    pub answer: String,
    pub references: Vec<MemorySearchItem>,
    pub used_ai: bool,
    pub model_name: Option<String>,
}

fn format_memory_references(references: &[MemorySearchItem]) -> String {
    references
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let source_label = match item.source_type.as_str() {
                "activity" => "活动记录",
                "hourly_summary" => "小时摘要",
                "daily_report" => "日报",
                _ => "记忆",
            };

            let mut parts = vec![format!("{}. [{}] {}", index + 1, source_label, item.title)];
            parts.push(format!("日期: {}", item.date));

            if let Some(app_name) = &item.app_name {
                if !app_name.is_empty() {
                    parts.push(format!("应用: {app_name}"));
                }
            }

            if let Some(browser_url) = &item.browser_url {
                if !browser_url.is_empty() {
                    parts.push(format!(
                        "URL: {}",
                        format_browser_url_for_display(browser_url)
                    ));
                }
            }

            if let Some(duration) = item.duration {
                if duration > 0 {
                    parts.push(format!("时长: {duration}秒"));
                }
            }

            if !item.excerpt.is_empty() {
                parts.push(format!("内容: {}", item.excerpt));
            }

            parts.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn build_memory_answer_prompt(question: &str, references: &[MemorySearchItem]) -> String {
    format!(
        "你是一个个人工作记忆助手。请严格基于给定记录回答，不要编造未出现的事实。\n\
如果证据不足，要明确说“不确定”或“记录里没有显示”。\n\
优先回答时间、应用、网站、工作主题和依据。\n\
回答请用中文，结构简洁，可使用短段落或要点。\n\n\
用户问题：{question}\n\n\
相关记录：\n{refs}",
        refs = format_memory_references(references)
    )
}

fn build_fallback_memory_answer(question: &str, references: &[MemorySearchItem]) -> String {
    if references.is_empty() {
        return format!(
            "未找到和“{question}”相关的历史记录。\n\n可尝试换一个关键词，或缩小日期范围后再搜索。"
        );
    }

    let mut answer = String::new();
    answer.push_str("以下是检索到的相关记录。\n\n");

    for item in references.iter().take(5) {
        answer.push_str(&format!("- {}（{}）", item.title, item.date));
        if let Some(app_name) = &item.app_name {
            if !app_name.is_empty() {
                answer.push_str(&format!("，应用：{app_name}"));
            }
        }
        if let Some(browser_url) = &item.browser_url {
            if !browser_url.is_empty() {
                answer.push_str(&format!(
                    "，URL：{}",
                    format_browser_url_for_display(browser_url)
                ));
            }
        }
        if let Some(duration) = item.duration {
            if duration > 0 {
                answer.push_str(&format!("，时长约 {duration} 秒"));
            }
        }
        if !item.excerpt.is_empty() {
            answer.push_str(&format!("。摘要：{}", item.excerpt));
        }
        answer.push('\n');
    }

    answer.push_str("\n当前为基础回答模式，仅基于检索结果做整理，未启用大模型归纳。");
    answer
}

async fn generate_memory_answer_with_model(
    model_config: &ModelConfig,
    question: &str,
    references: &[MemorySearchItem],
) -> Result<String, AppError> {
    generate_text_answer_with_model(
        model_config,
        "你是一个严谨的个人工作记忆助手，只能基于提供的记录作答，请用中文回答。",
        &build_memory_answer_prompt(question, references),
    )
    .await
}

/// 搜索工作记忆
#[tauri::command]
pub async fn search_memory(
    query: String,
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<MemorySearchItem>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.search_memory(
        &query,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(20) as usize,
    )
}

/// 基于工作记忆回答问题
#[tauri::command]
pub async fn ask_memory(
    question: String,
    date_from: Option<String>,
    date_to: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<MemoryAnswer, AppError> {
    let (model_config, references) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let references =
            state
                .database
                .search_memory(&question, date_from.as_deref(), date_to.as_deref(), 8)?;
        (state.config.text_model.clone(), references)
    };

    if references.is_empty() {
        return Ok(MemoryAnswer {
            answer: build_fallback_memory_answer(&question, &references),
            references,
            used_ai: false,
            model_name: None,
        });
    }

    if is_text_model_available(&model_config) {
        match generate_memory_answer_with_model(&model_config, &question, &references).await {
            Ok(answer) => {
                return Ok(MemoryAnswer {
                    answer,
                    references,
                    used_ai: true,
                    model_name: Some(model_config.model),
                });
            }
            Err(error) => {
                log::warn!("记忆问答 AI 生成失败，回退基础模式: {error}");
            }
        }
    }

    Ok(MemoryAnswer {
        answer: build_fallback_memory_answer(&question, &references),
        references,
        used_ai: false,
        model_name: None,
    })
}

/// 获取连续工作 session 聚合结果
#[tauri::command]
pub async fn get_work_sessions(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<WorkSession>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000) as usize,
    )?;

    Ok(build_work_sessions(&activities))
}

/// 基于 session 识别主要工作意图
#[tauri::command]
pub async fn recognize_work_intents(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<IntentAnalysisResult, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000) as usize,
    )?;

    Ok(analyze_intents(&activities))
}

/// 获取活跃洞察列表
#[tauri::command]
pub async fn get_insights(
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<work_review_core::database::WorkInsight>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(state
        .database
        .get_active_insights(limit.unwrap_or(20) as usize)?)
}

/// 用户对洞察的反馈（确认/否认）
#[tauri::command]
pub async fn feedback_insight(
    id: i64,
    positive: bool,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.feedback_insight(id, positive)?;
    Ok(())
}

/// 合成今日工作洞察（规则版，MVP 不依赖 AI）
pub(crate) fn synthesize_insights_inner(
    date: &str,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<work_review_core::database::WorkInsight>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let segments = state.config.effective_work_segments();
    let stats = state
        .database
        .get_daily_stats_with_segments(date, &segments)?;

    let mut created_ids: Vec<i64> = Vec::new();

    // 洞察 1：高峰时段
    if let Some(peak) = stats
        .hourly_activity_distribution
        .iter()
        .max_by_key(|h| h.duration)
    {
        if peak.duration > 0 && peak.duration >= 1800 {
            let hours = peak.duration / 3600;
            let mins = (peak.duration % 3600) / 60;
            let content = format!(
                "今日高峰时段 {:02}:00，累计 {}{}",
                peak.hour,
                if hours > 0 {
                    format!("{}小时", hours)
                } else {
                    String::new()
                },
                if mins > 0 {
                    format!("{}分钟", mins)
                } else {
                    String::new()
                },
            );
            let keyword = format!("{:02}:00", peak.hour);
            if !state.database.has_similar_insight("peak_hours", &keyword)? {
                let id = state
                    .database
                    .create_insight("peak_hours", &content, date)?;
                created_ids.push(id);
            }
        }
    }

    // 洞察 2：分类分布（娱乐/通讯偏高时提醒）
    let total: i64 = stats.category_usage.iter().map(|c| c.duration).sum();
    if total > 0 {
        for cat in &stats.category_usage {
            let pct = cat.duration * 100 / total;
            let cat_key = cat.category.as_str();
            if (cat_key == "entertainment" || cat_key == "communication") && pct > 25 {
                let content = format!(
                    "{}类活动占比 {}%（{}分钟），建议控制",
                    match cat_key {
                        "entertainment" => "娱乐".to_string(),
                        "communication" => "通讯".to_string(),
                        _ => cat.category.clone(),
                    },
                    pct,
                    cat.duration / 60
                );
                if !state.database.has_similar_insight("distraction", cat_key)? {
                    let id = state
                        .database
                        .create_insight("distraction", &content, date)?;
                    created_ids.push(id);
                }
            }
        }
    }

    // 洞察 3：工作时长总结
    if stats.work_time_duration > 0 {
        let hours = stats.work_time_duration / 3600;
        let content = format!("今日办公时长 {} 小时", hours);
        if !state
            .database
            .has_similar_insight("work_volume", &format!("{}小时", hours))?
        {
            let id = state
                .database
                .create_insight("work_volume", &content, date)?;
            created_ids.push(id);
        }
    }

    // 返回新创建的活跃洞察
    let all = state.database.get_active_insights(20)?;
    let new_insights = all
        .into_iter()
        .filter(|i| created_ids.contains(&i.id))
        .collect();
    Ok(new_insights)
}

/// 合成今日洞察（Tauri 命令）
#[tauri::command]
pub async fn synthesize_insights(
    date: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<work_review_core::database::WorkInsight>, AppError> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let date = date.unwrap_or(today);
    synthesize_insights_inner(&date, state.inner())
}

/// 生成周报 / 阶段复盘 —— 内部复用版（供 Tauri 命令与 localhost API 共用）
pub(crate) fn generate_weekly_review_inner(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: &Arc<Mutex<AppState>>,
) -> Result<WeeklyReviewResult, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000) as usize,
    )?;

    Ok(build_weekly_review(
        &activities,
        date_from.as_deref(),
        date_to.as_deref(),
    ))
}

/// 生成周报 / 阶段复盘
#[tauri::command]
pub async fn generate_weekly_review(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<WeeklyReviewResult, AppError> {
    generate_weekly_review_inner(date_from, date_to, limit, state.inner())
}

/// 提取待跟进事项
#[tauri::command]
pub async fn extract_todo_items(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<TodoExtractionResult, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000) as usize,
    )?;

    Ok(merge_manual_followups_into_todos(
        extract_todos(&activities),
        &state.config.avatar_followups,
        date_from.as_deref(),
        date_to.as_deref(),
    ))
}

