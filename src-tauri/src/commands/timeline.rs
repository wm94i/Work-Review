//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::AppState;
use std::sync::{Arc, Mutex};
use tauri::State;
use work_review_core::database::Activity;
use work_review_core::error::AppError;

use super::shared::{collect_privacy_filters, filter_activities_by_privacy};

/// 获取指定日期的时间线 —— 内部复用版（供 Tauri 命令与 localhost API 共用，释放全局锁后查询）
pub(crate) fn get_timeline_inner(
    date: String,
    limit: Option<u32>,
    offset: Option<u32>,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<Activity>, AppError> {
    let (database, ignored_apps, excluded_domains) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&guard);
        (guard.database.clone(), ignored_apps, excluded_domains)
    };
    let activities = database.get_timeline(&date, limit, offset)?;
    let filtered = filter_activities_by_privacy(activities, &ignored_apps, &excluded_domains);

    if !ignored_apps.is_empty() || !excluded_domains.is_empty() {
        log::info!(
            "隐私过滤: 需过滤应用 {:?}, 域名 {:?}，结果 {} 条",
            ignored_apps,
            excluded_domains,
            filtered.len()
        );
    }

    Ok(filtered)
}

/// 获取指定日期的时间线
#[tauri::command]
pub async fn get_timeline(
    date: String,
    limit: Option<u32>,
    offset: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<Activity>, AppError> {
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || get_timeline_inner(date, limit, offset, &state_arc))
        .await
        .map_err(|e| AppError::Unknown(format!("时间线查询任务异常: {e}")))?
}

/// 获取单个活动（用于刷新详情页，获取最新 OCR 结果）
#[tauri::command]
pub async fn get_activity(
    id: i64,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<Activity>, AppError> {
    let (database, ignored_apps, excluded_domains) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&guard);
        (guard.database.clone(), ignored_apps, excluded_domains)
    };
    tokio::task::spawn_blocking(move || {
        let activity = database.get_activity_by_id(id)?;
        Ok(filter_activities_by_privacy(
            activity.into_iter().collect(),
            &ignored_apps,
            &excluded_domains,
        )
        .into_iter()
        .next())
    })
    .await
    .map_err(|e| AppError::Unknown(format!("活动详情查询任务异常: {e}")))?
}

/// 导出指定日期的时间线为 JSON 文件
///
/// - 复用 `get_timeline_inner` 的隐私过滤（忽略应用 / 排除域名）
/// - `include_ocr = false` 时，OCR 文本会被清空，避免泄露屏幕内容
/// - 写入路径为用户选择的 `target_path`，必须以 `.json` 结尾
pub(crate) fn export_timeline_json_inner(
    date: String,
    target_path: String,
    include_ocr: bool,
    state: &Arc<Mutex<AppState>>,
) -> Result<String, AppError> {
    let target = target_path.trim();
    if target.is_empty() {
        return Err(AppError::Config("请先选择导出文件路径".to_string()));
    }
    let date = date.trim().to_string();
    if !super::shared::is_iso_date(&date) {
        return Err(AppError::Config("日期格式应为 YYYY-MM-DD".to_string()));
    }
    // 校验扩展名、拒绝隐藏 / 敏感目录，父目录不存在时创建
    let target_path_buf =
        super::report::prepare_export_file_target(std::path::Path::new(target), &["json"])?;

    // 走完整时间线（不按默认 1000 条截断），并按当前隐私规则过滤与脱敏
    let activities = get_timeline_inner(date.clone(), Some(50_000), None, state)?;
    let activities = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        super::shared::sanitize_activities_for_external(
            activities,
            &guard.privacy_filter,
            include_ocr,
        )
    };

    let payload = serde_json::json!({
        "version": 1,
        "date": date,
        "exported_at": chrono::Local::now().to_rfc3339(),
        "include_ocr": include_ocr,
        "count": activities.len(),
        "activities": activities,
    });
    let serialized = serde_json::to_string_pretty(&payload)
        .map_err(|e| AppError::Unknown(format!("序列化时间线失败: {e}")))?;
    std::fs::write(&target_path_buf, serialized)?;
    Ok(super::config::path_for_display(&target_path_buf))
}

#[tauri::command]
pub async fn export_timeline_json(
    date: String,
    target_path: String,
    include_ocr: Option<bool>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    export_timeline_json_inner(
        date,
        target_path,
        include_ocr.unwrap_or(false),
        state.inner(),
    )
}

/// 获取截图缩略图（释放全局锁并在阻塞线程池执行，命中磁盘 400px 缩略图缓存时免重解码）
#[tauri::command]
pub async fn get_screenshot_thumbnail(
    path: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    super::shared::validate_relative_path(&path)?;
    let (full_path, screenshot_service) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (guard.data_dir.join(&path), guard.screenshot_service.clone())
    };
    tokio::task::spawn_blocking(move || {
        screenshot_service.generate_thumbnail_base64(&full_path, 400)
    })
    .await
    .map_err(|e| AppError::Unknown(format!("缩略图加载任务异常: {e}")))?
}

/// 获取高分辨率截图（用于详情弹窗，1200px；释放全局锁并在阻塞线程池读取）
#[tauri::command]
pub async fn get_screenshot_full(
    path: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    super::shared::validate_relative_path(&path)?;
    let (full_path, screenshot_service) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (guard.data_dir.join(&path), guard.screenshot_service.clone())
    };
    tokio::task::spawn_blocking(move || screenshot_service.generate_full_image_base64(&full_path))
        .await
        .map_err(|e| AppError::Unknown(format!("原图加载任务异常: {e}")))?
}
