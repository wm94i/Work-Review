//! 远程截图补传、自动日报/洞察调度、后台语义增量索引与周期存储清理任务。

use crate::bootstrap::should_run_startup_cleanup;
use crate::{commands, remote_upload, AppState, StorageManager};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use work_review_core::config::AppConfig;
/// 根据配置计算每日自动日报与工作洞察触发时刻（自当天 00:00 起的分钟数）。
/// 优先使用 `daily_report_auto_generate_time`，其次取 `effective_work_segments()` 中最晚结束时间。
pub(crate) fn resolve_auto_report_work_end_minutes(config: &AppConfig) -> u32 {
    if let Some(custom) = config.daily_report_auto_generate_time.as_deref() {
        let mut parts = custom.trim().split(':');
        if let (Some(h_str), Some(m_str)) = (parts.next(), parts.next()) {
            if let (Ok(h), Ok(m)) = (h_str.trim().parse::<u32>(), m_str.trim().parse::<u32>()) {
                return h.min(23) * 60 + m.min(59);
            }
        }
    }

    let segments = config.effective_work_segments();
    segments
        .iter()
        .map(|seg| u32::from(seg.end_hour.min(23)) * 60 + u32::from(seg.end_minute.min(59)))
        .max()
        .unwrap_or_else(|| {
            u32::from(config.work_end_hour.min(23)) * 60 + u32::from(config.work_end_minute.min(59))
        })
}

/// 远程存储补传任务：定期扫描「本地有截图但远程 URL 缺失」的近期活动并补传。
/// 实时上传失败只有一次轻量重试（见 remote_upload），断网/服务不可用超过该窗口的
/// 截图会永久缺失——本任务作为兜底，每 10 分钟补传一批。
/// 窗口限定近 72 小时：更早的缺口多半是远程存储当时未启用或本地文件已被保留策略清理，
/// 反复重试没有意义。
pub(crate) async fn remote_upload_backfill_task(state: Arc<Mutex<AppState>>) {
    const INITIAL_DELAY_SECS: u64 = 60;
    const SWEEP_INTERVAL_SECS: u64 = 10 * 60;
    const BACKFILL_WINDOW_SECS: i64 = 72 * 60 * 60;
    const BATCH_LIMIT: u32 = 20;

    tokio::time::sleep(tokio::time::Duration::from_secs(INITIAL_DELAY_SECS)).await;

    loop {
        let (remote_cfg, data_dir, database) = {
            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                guard.config.remote_storage.clone(),
                guard.data_dir.clone(),
                guard.database.clone(),
            )
        };

        let pending =
            if remote_cfg.provider == work_review_core::config::RemoteStorageProvider::None {
                Vec::new()
            } else {
                let since = chrono::Local::now().timestamp() - BACKFILL_WINDOW_SECS;
                let db_for_query = database.clone();
                tokio::task::spawn_blocking(move || {
                    db_for_query.get_activities_missing_screenshot_url(since, BATCH_LIMIT)
                })
                .await
                .ok()
                .and_then(|res| match res {
                    Ok(rows) => Some(rows),
                    Err(e) => {
                        log::warn!("查询待补传截图失败: {e}");
                        None
                    }
                })
                .unwrap_or_default()
            };

        let mut uploaded = 0u32;
        let mut pruned_missing = 0u32;
        for (activity_id, relative_path) in pending {
            // 历史数据可能存过绝对路径，或者本地截图文件已被清理：清空本地路径标记，避免反复堵塞 20 条补传队列。
            let unusable_or_missing = std::path::Path::new(&relative_path).is_absolute()
                || !data_dir.join(&relative_path).exists();
            if unusable_or_missing {
                let db_for_clear = database.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    db_for_clear.clear_activity_screenshot_path(activity_id)
                })
                .await;
                pruned_missing += 1;
                continue;
            }
            let local_path = data_dir.join(&relative_path);
            match remote_upload::upload_screenshot(&remote_cfg, &local_path, &relative_path).await {
                Ok(url) => {
                    let db_for_update = database.clone();
                    let updated = tokio::task::spawn_blocking(move || {
                        db_for_update.update_activity_screenshot_url(activity_id, &url)
                    })
                    .await;
                    match updated {
                        Ok(Ok(())) => uploaded += 1,
                        Ok(Err(e)) => {
                            log::warn!("补传后写回 screenshot_url 失败（活动 {activity_id}）: {e}")
                        }
                        Err(e) => {
                            log::warn!("补传写回任务异常（活动 {activity_id}）: {e}")
                        }
                    }
                }
                Err(e) => log::warn!("补传截图失败（活动 {activity_id}）: {e}"),
            }
            // 温和限速，避免补传批次挤占正常上传与前台网络
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
        if uploaded > 0 || pruned_missing > 0 {
            log::info!(
                "截图补传完成：本轮补传 {uploaded} 张，清理失效本地路径标记 {pruned_missing} 条"
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
    }
}

/// 自动日报与每日工作记忆洞察调度任务（从前端迁入 Rust，轻量模式无主窗口时同样执行）。
/// 失败按指数退避（2m → 4m → ... → 60m），避免离线或模型异常时每分钟高频重试。
pub(crate) async fn auto_report_and_insight_task(state: Arc<Mutex<AppState>>, app: AppHandle) {
    use chrono::Timelike;

    const INITIAL_DELAY_SECS: u64 = 45;
    const CHECK_INTERVAL_SECS: u64 = 60;
    const BASE_BACKOFF_SECS: u64 = 120;
    const MAX_BACKOFF_SECS: u64 = 3600;

    tokio::time::sleep(Duration::from_secs(INITIAL_DELAY_SECS)).await;

    let mut last_report_done_date: Option<String> = None;
    let mut report_backoff_secs = BASE_BACKOFF_SECS;
    let mut report_next_retry_at: Option<std::time::Instant> = None;
    let mut insight_backoff_secs = BASE_BACKOFF_SECS;
    let mut insight_next_retry_at: Option<std::time::Instant> = None;
    let mut last_observed_date = String::new();

    loop {
        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        if today != last_observed_date {
            last_observed_date = today.clone();
            report_backoff_secs = BASE_BACKOFF_SECS;
            report_next_retry_at = None;
            insight_backoff_secs = BASE_BACKOFF_SECS;
            insight_next_retry_at = None;
        }

        let current_minutes = now.hour() * 60 + now.minute();
        let (
            database,
            target_minutes,
            locale,
            memory_enabled,
            memory_last_synthesis_date,
            config_allows_save,
        ) = {
            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                guard.database.clone(),
                resolve_auto_report_work_end_minutes(&guard.config),
                guard.config.locale.clone(),
                guard.config.memory_enabled,
                guard.config.memory_last_synthesis_date.clone(),
                guard.config_load_status.allows_automatic_save(),
            )
        };

        if current_minutes >= target_minutes {
            // 1. 自动生成今日日报
            let report_ready = last_report_done_date.as_deref() != Some(today.as_str())
                && report_next_retry_at
                    .map(|at| std::time::Instant::now() >= at)
                    .unwrap_or(true);
            if report_ready {
                let db_check = database.clone();
                let date_check = today.clone();
                let locale_check = locale.clone();
                let existing = tokio::task::spawn_blocking(move || {
                    db_check.get_report(&date_check, Some(&locale_check))
                })
                .await;

                match existing {
                    Ok(Ok(Some(_))) => {
                        last_report_done_date = Some(today.clone());
                        report_backoff_secs = BASE_BACKOFF_SECS;
                        report_next_retry_at = None;
                    }
                    Ok(Ok(None)) => {
                        log::info!("工作结束时间到达，后端开始自动生成 {today} 日报...");
                        match commands::generate_report_inner(
                            today.clone(),
                            Some(false),
                            Some(locale.clone()),
                            &app,
                            &state,
                        )
                        .await
                        {
                            Ok(_) => {
                                log::info!("后端自动日报生成完成: {today}");
                                last_report_done_date = Some(today.clone());
                                report_backoff_secs = BASE_BACKOFF_SECS;
                                report_next_retry_at = None;
                                let _ = app.emit(
                                    "auto-report-generated",
                                    serde_json::json!({ "date": today, "locale": locale }),
                                );
                            }
                            Err(error) => {
                                log::warn!(
                                    "后端自动日报生成失败（{report_backoff_secs}s 后重试）: {error}"
                                );
                                report_next_retry_at = Some(
                                    std::time::Instant::now()
                                        + Duration::from_secs(report_backoff_secs),
                                );
                                report_backoff_secs =
                                    (report_backoff_secs.saturating_mul(2)).min(MAX_BACKOFF_SECS);
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        log::warn!("检查今日日报是否存在失败: {error}");
                    }
                    Err(error) => {
                        log::warn!("检查今日日报任务异常: {error}");
                    }
                }
            }

            // 2. 自动合成今日工作记忆洞察
            let insight_ready = memory_enabled
                && memory_last_synthesis_date.as_deref() != Some(today.as_str())
                && insight_next_retry_at
                    .map(|at| std::time::Instant::now() >= at)
                    .unwrap_or(true);
            if insight_ready {
                let state_for_synth = state.clone();
                let date_for_synth = today.clone();
                let synth_res = tokio::task::spawn_blocking(move || {
                    commands::synthesize_insights_inner(&date_for_synth, &state_for_synth)
                })
                .await;
                match synth_res {
                    Ok(Ok(_)) => {
                        log::info!("后端工作记忆每日洞察合成完成: {today}");
                        insight_backoff_secs = BASE_BACKOFF_SECS;
                        insight_next_retry_at = None;
                        if config_allows_save {
                            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            guard.config.memory_last_synthesis_date = Some(today.clone());
                            if let Err(e) = guard.config.save(&guard.config_path) {
                                log::warn!("保存工作记忆合成日期失败: {e}");
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        log::warn!(
                            "后端工作记忆合成失败（{insight_backoff_secs}s 后重试）: {error}"
                        );
                        insight_next_retry_at = Some(
                            std::time::Instant::now() + Duration::from_secs(insight_backoff_secs),
                        );
                        insight_backoff_secs =
                            (insight_backoff_secs.saturating_mul(2)).min(MAX_BACKOFF_SECS);
                    }
                    Err(error) => {
                        log::warn!("后端工作记忆合成任务异常: {error}");
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(CHECK_INTERVAL_SECS)).await;
    }
}

/// 可选的语义记忆后台增量索引任务（默认关闭，`semantic_auto_index = false`）。
/// 开启后每 30 分钟按游标追平新增活动与脏日期，单次调用受单飞行互斥保护。
pub(crate) async fn semantic_auto_index_task(state: Arc<Mutex<AppState>>) {
    const INITIAL_DELAY_SECS: u64 = 120;
    const SWEEP_INTERVAL_SECS: u64 = 30 * 60;

    tokio::time::sleep(Duration::from_secs(INITIAL_DELAY_SECS)).await;

    loop {
        commands::run_background_semantic_auto_index(&state).await;
        tokio::time::sleep(Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
    }
}

/// 存储清理周期任务：窗口出现后首轮立即延迟 10 秒执行（不再阻塞 main() 启动路径），
/// 之后每 6 小时周期执行一次。配置损坏(fail-safe)时跳过清理以保护历史数据。
pub(crate) async fn storage_cleanup_task(state: Arc<Mutex<AppState>>) {
    const INITIAL_DELAY_SECS: u64 = 10;
    const SWEEP_INTERVAL_SECS: u64 = 6 * 60 * 60;

    tokio::time::sleep(tokio::time::Duration::from_secs(INITIAL_DELAY_SECS)).await;

    loop {
        let (data_dir, database, storage_config, allowed) = {
            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                guard.data_dir.clone(),
                guard.database.clone(),
                guard.config.storage.clone(),
                should_run_startup_cleanup(guard.config_load_status),
            )
        };
        if allowed {
            // 用独立实例在阻塞线程执行,避免清理期间占用 AppState 锁阻塞采集循环
            let manager = StorageManager::new(&data_dir, storage_config);
            match tokio::task::spawn_blocking(move || manager.cleanup(&database)).await {
                Ok(Ok(_)) => log::debug!("周期存储清理完成"),
                Ok(Err(e)) => log::warn!("周期存储清理失败: {e}"),
                Err(e) => log::warn!("周期存储清理任务异常: {e}"),
            }
        } else {
            log::warn!("配置损坏，已跳过存储清理以保护历史数据");
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
    }
}
