//! 每小时工作摘要计算、持久化与整点后调度任务。

use crate::{commands, AppState};
use std::sync::{Arc, Mutex};
/// 小时摘要生成任务
/// 每小时检查一次，为上一个完整小时生成摘要
/// 为指定日期和小时生成并保存摘要
/// 计算某小时的摘要（不落库）。该小时无活动或查询失败时返回 None。
pub(crate) fn build_hourly_summary_from_activities(
    date: &str,
    hour: i32,
    raw_activities: Vec<work_review_core::database::Activity>,
    ignored_apps: &[String],
    excluded_domains: &[String],
    privacy_filter: &work_review_core::privacy::PrivacyFilter,
) -> Option<work_review_core::database::HourlySummary> {
    use work_review_core::analysis::hourly::{generate_fallback_summary, HourlyStats};

    let filtered =
        commands::filter_activities_by_privacy(raw_activities, ignored_apps, excluded_domains);
    // 小时摘要落库并被本地 API / MCP / 记忆搜索共享：过滤忽略应用/排除域名、脱敏标题并剥离 OCR
    let acts = commands::sanitize_activities_for_external(filtered, privacy_filter, false);
    if acts.is_empty() {
        log::debug!("该小时无活动数据: {date} {hour}:00");
        return None;
    }

    let stats = HourlyStats::from_activities(acts);
    let summary = generate_fallback_summary(&stats);

    Some(work_review_core::database::HourlySummary {
        id: None,
        date: date.to_string(),
        hour,
        summary,
        main_apps: stats.get_main_apps().join(", "),
        activity_count: stats.activity_count,
        total_duration: stats.total_duration,
        representative_screenshots: Some(
            serde_json::to_string(&stats.representative_screenshots).unwrap_or_default(),
        ),
        created_at: chrono::Local::now().timestamp(),
    })
}

pub(crate) fn build_hourly_summary(
    state: &Arc<Mutex<AppState>>,
    date: &str,
    hour: i32,
) -> Option<work_review_core::database::HourlySummary> {
    let (database, ignored_apps, excluded_domains, privacy_config) = {
        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        let (ignored_apps, excluded_domains) = commands::collect_privacy_filters(&state_guard);
        (
            state_guard.database.clone(),
            ignored_apps,
            excluded_domains,
            state_guard.config.privacy.clone(),
        )
    };

    match database.get_hourly_activities(date, hour) {
        Ok(raw) => {
            let privacy_filter =
                work_review_core::privacy::PrivacyFilter::from_config(&privacy_config);
            build_hourly_summary_from_activities(
                date,
                hour,
                raw,
                &ignored_apps,
                &excluded_domains,
                &privacy_filter,
            )
        }
        Err(e) => {
            log::error!("获取小时活动数据失败: {e}");
            None
        }
    }
}

pub(crate) fn save_hourly_summary_from_activities(
    state: &Arc<Mutex<AppState>>,
    database: &work_review_core::database::Database,
    date: &str,
    hour: i32,
    raw_activities: Vec<work_review_core::database::Activity>,
) {
    let (ignored_apps, excluded_domains, privacy_config) = {
        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        let (ignored_apps, excluded_domains) = commands::collect_privacy_filters(&state_guard);
        (
            ignored_apps,
            excluded_domains,
            state_guard.config.privacy.clone(),
        )
    };
    let privacy_filter = work_review_core::privacy::PrivacyFilter::from_config(&privacy_config);
    if let Some(hourly_summary) = build_hourly_summary_from_activities(
        date,
        hour,
        raw_activities,
        &ignored_apps,
        &excluded_domains,
        &privacy_filter,
    ) {
        match database.save_hourly_summary(&hourly_summary) {
            Ok(_) => log::info!("小时摘要保存成功: {date} {hour}:00"),
            Err(e) => log::error!("保存小时摘要失败: {e}"),
        }
    }
}

pub(crate) fn generate_and_save_summary(state: &Arc<Mutex<AppState>>, date: &str, hour: i32) {
    let Some(hourly_summary) = build_hourly_summary(state, date, hour) else {
        return;
    };

    let database = {
        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        state_guard.database.clone()
    };

    match database.save_hourly_summary(&hourly_summary) {
        Ok(_) => log::info!("小时摘要保存成功: {date} {hour}:00"),
        Err(e) => log::error!("保存小时摘要失败: {e}"),
    }
}

pub(crate) async fn hourly_summary_task(state: Arc<Mutex<AppState>>) {
    use chrono::{Local, Timelike};

    // 等待30秒后开始（给应用启动留时间，但不用等太久）
    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    // 启动时一次范围查询回填今天所有已过时段的摘要（替代逐小时 24 次独立 SQL 查询）
    {
        let now = Local::now();
        let date = now.format("%Y-%m-%d").to_string();
        let current_hour = now.hour() as usize;

        if current_hour > 0 {
            log::info!("回填今天 0:00 ~ {current_hour}:00 的小时摘要");
            let state_clone = state.clone();
            let date_clone = date.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let (database, ignored_apps, excluded_domains, privacy_config) = {
                    let state_guard = state_clone.lock().unwrap_or_else(|e| e.into_inner());
                    let (ignored_apps, excluded_domains) =
                        commands::collect_privacy_filters(&state_guard);
                    (
                        state_guard.database.clone(),
                        ignored_apps,
                        excluded_domains,
                        state_guard.config.privacy.clone(),
                    )
                };
                match database.get_all_hourly_activities_for_date(&date_clone) {
                    Ok(hourly_buckets) => {
                        let privacy_filter =
                            work_review_core::privacy::PrivacyFilter::from_config(&privacy_config);
                        for (hour, bucket) in hourly_buckets
                            .into_iter()
                            .enumerate()
                            .take(current_hour.min(24))
                        {
                            if let Some(summary) = build_hourly_summary_from_activities(
                                &date_clone,
                                hour as i32,
                                bucket,
                                &ignored_apps,
                                &excluded_domains,
                                &privacy_filter,
                            ) {
                                if let Err(e) = database.save_hourly_summary(&summary) {
                                    log::error!("保存小时摘要失败: {e}");
                                }
                            }
                        }
                    }
                    Err(e) => log::error!("批量读取当日小时活动失败: {e}"),
                }
            })
            .await;
        }
    }

    loop {
        let now = Local::now();

        // 为上一个真实小时生成摘要：
        // - 春季夏令时向前跳变（02:00→03:00）时，`now - 1h` 落回 01:xx，避免漏掉 01:00 且不查空洞的 02:00；
        // - 次日 00:05 时用 `date_naive().pred_opt()` 取昨天而非 `now - Duration::days(1)`，
        //   避免 23 小时夏令时日后减去 86400 实秒跳回前天（date - 2）。
        let (target_date, target_hour) = previous_hourly_summary_target(now);

        // 检查是否已有摘要
        let should_generate = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            match state_guard
                .database
                .has_hourly_summary(&target_date, target_hour)
            {
                Ok(has) => !has,
                Err(e) => {
                    log::error!("检查小时摘要失败: {e}");
                    false
                }
            }
        };

        if should_generate {
            log::info!("开始生成 {target_date} {target_hour}:00 的小时摘要");
            generate_and_save_summary(&state, &target_date, target_hour);
        }

        // 休眠到下一个小时的第 5 分钟。按"本小时已过秒数"计算，而不是
        // `(now + 1h).with_minute(5).unwrap()`：夏令时回拨/跳变时本地时间可能有歧义或不存在，
        // `with_minute` 返回 None 会让整个小时摘要任务 panic 退出。
        let sleep_duration = seconds_until_next_hourly_check(now.minute(), now.second());
        tokio::time::sleep(tokio::time::Duration::from_secs(sleep_duration)).await;
    }
}

/// 计算上一个应生成摘要的小时 `(YYYY-MM-DD, hour)`，兼容夏令时跳变与回拨。
pub(crate) fn previous_hourly_summary_target(
    now: chrono::DateTime<chrono::Local>,
) -> (String, i32) {
    use chrono::Timelike;
    if now.hour() == 0 {
        let prev_date = now
            .date_naive()
            .pred_opt()
            .unwrap_or_else(|| now.date_naive());
        return (prev_date.format("%Y-%m-%d").to_string(), 23);
    }
    let prev = now - chrono::Duration::hours(1);
    (prev.format("%Y-%m-%d").to_string(), prev.hour() as i32)
}

/// 距下一个整点第 5 分钟的秒数（至少 60 秒）。
pub(crate) fn seconds_until_next_hourly_check(minute: u32, second: u32) -> u64 {
    let elapsed_in_hour = u64::from(minute.min(59)) * 60 + u64::from(second.min(59));
    (3600 - elapsed_in_hour + 5 * 60).max(60)
}
