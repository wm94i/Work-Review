//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::database::Activity;
use crate::error::AppError;
use crate::AppState;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

use super::shared::{collect_privacy_filters, filter_activities_by_privacy};

/// 获取指定日期的时间线 —— 内部复用版（供 Tauri 命令与 localhost API 共用）
pub(crate) fn get_timeline_inner(
    date: String,
    limit: Option<u32>,
    offset: Option<u32>,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<Activity>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = state.database.get_timeline(&date, limit, offset)?;
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
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
    get_timeline_inner(date, limit, offset, state.inner())
}

/// 获取单个活动（用于刷新详情页，获取最新 OCR 结果）
#[tauri::command]
pub async fn get_activity(
    id: i64,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<Activity>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.get_activity_by_id(id)
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
    let target_path_buf = PathBuf::from(target);

    // 走完整时间线（不分页），并应用隐私过滤
    let mut activities = get_timeline_inner(date.clone(), None, None, state)?;
    if !include_ocr {
        for a in activities.iter_mut() {
            a.ocr_text = None;
        }
    }

    // 父目录可能不存在（用户选了新位置），尝试创建
    if let Some(parent) = target_path_buf.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

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
    Ok(target_path_buf.to_string_lossy().to_string())
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

/// 获取截图缩略图
#[tauri::command]
pub async fn get_screenshot_thumbnail(
    path: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let full_path = state.data_dir.join(&path);
    state
        .screenshot_service
        .generate_thumbnail_base64(&full_path, 400)
}

/// 获取高分辨率截图（用于详情弹窗，1200px）
#[tauri::command]
pub async fn get_screenshot_full(
    path: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let full_path = state.data_dir.join(&path);
    state
        .screenshot_service
        .generate_full_image_base64(&full_path)
}

/// 获取指定日期的 OCR 日志
#[tauri::command]
pub async fn get_ocr_log(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let ocr_logger = crate::ocr_logger::OcrLogger::new(&state.data_dir);
    ocr_logger.read_log(&date)
}

