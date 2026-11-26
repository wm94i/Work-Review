//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::AppState;
use std::sync::{Arc, Mutex};
use tauri::State;
use work_review_core::error::AppError;
use work_review_core::work_intelligence::{
    generate_weekly_review as build_weekly_review, WeeklyReviewResult,
};

use super::shared::{collect_privacy_filters, load_filtered_activities_in_range};

/// 合成今日工作洞察（规则版，MVP 不依赖 AI）
pub(crate) fn synthesize_insights_inner(
    date: &str,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<work_review_core::database::WorkInsight>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let segments = state.config.effective_work_segments();
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
    let stats = state.database.get_daily_stats_with_segments_filtered(
        date,
        &segments,
        &ignored_apps,
        &excluded_domains,
    )?;

    let mut created_ids: Vec<i64> = Vec::new();

    // 洞察 1：高峰时段
    if let Some(peak) = stats
        .hourly_activity_distribution
        .iter()
        .max_by_key(|h| h.duration)
    {
        if peak.duration >= 1800 {
            let hours = peak.duration / 3600;
            let mins = (peak.duration % 3600) / 60;
            let content = format!(
                "今日高峰时段 {:02}:00，累计 {}{}",
                peak.hour,
                if hours > 0 {
                    format!("{hours}小时")
                } else {
                    String::new()
                },
                if mins > 0 {
                    format!("{mins}分钟")
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
        let content = format!("今日办公时长 {hours} 小时");
        if !state
            .database
            .has_similar_insight("work_volume", &format!("{hours}小时"))?
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

/// 生成周报 / 阶段复盘 —— 供 localhost API 使用（对外输出）
pub(crate) fn generate_weekly_review_inner(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: &Arc<Mutex<AppState>>,
) -> Result<WeeklyReviewResult, AppError> {
    const MAX_WEEKLY_REVIEW_ACTIVITIES: u32 = 20_000;
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000).clamp(1, MAX_WEEKLY_REVIEW_ACTIVITIES) as usize,
    )?;
    // 对外输出：按当前脱敏规则处理，且不使用 OCR 原文（复盘里的待办 / 亮点会引用标题与 OCR）
    let activities =
        super::shared::sanitize_activities_for_external(activities, &state.privacy_filter, false);

    Ok(build_weekly_review(
        &activities,
        date_from.as_deref(),
        date_to.as_deref(),
    ))
}
