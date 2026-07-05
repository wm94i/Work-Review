//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::error::AppError;
use crate::AppState;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

use super::shared::persist_app_config;

#[tauri::command]
pub async fn get_localhost_api_status(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<crate::localhost_api::LocalhostApiStatusPayload, AppError> {
    crate::localhost_api::get_localhost_api_status(state.inner())
}

#[tauri::command]
pub async fn get_node_gateway_status(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<crate::node_gateway::NodeGatewayStatusPayload, AppError> {
    crate::node_gateway::get_node_gateway_status(state.inner())
}

#[tauri::command]
pub async fn get_telegram_bot_status(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let now_ts = chrono::Local::now().timestamp();
    Ok(serde_json::json!({
        "running": s.telegram_bot_runtime.is_running(),
        "starting": s.telegram_bot_runtime.is_starting(),
        "lastError": s.telegram_bot_runtime.last_error(),
        "allowedChatIds": s.config.telegram_bot_allowed_chat_ids.clone(),
        "bindCode": s.config.telegram_bot_bind_code.clone(),
        "bindCodeExpiresAt": s.config.telegram_bot_bind_code_expires_at,
        "bindCodeExpired": s.config.telegram_bot_bind_code_expires_at.map(|expires_at| expires_at < now_ts).unwrap_or(false),
    }))
}

#[tauri::command]
pub async fn generate_telegram_bot_bind_code(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    const TELEGRAM_BIND_CODE_TTL_SECONDS: i64 = 10 * 60;

    let raw = uuid::Uuid::new_v4().simple().to_string();
    let code = format!("WR-{}", raw[..6].to_ascii_uppercase());
    let expires_at = chrono::Local::now().timestamp() + TELEGRAM_BIND_CODE_TTL_SECONDS;
    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut config = state.config.clone();
        config.telegram_bot_bind_code = Some(code.clone());
        config.telegram_bot_bind_code_expires_at = Some(expires_at);
        config
    };

    persist_app_config(next_config, app, state.inner())?;

    Ok(serde_json::json!({
        "code": code,
        "expiresAt": expires_at,
    }))
}

#[tauri::command]
pub async fn reveal_localhost_api_token(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    crate::localhost_api::reveal_localhost_api_token(state.inner())
}

#[tauri::command]
pub async fn rotate_localhost_api_token(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let token = crate::localhost_api::rotate_localhost_api_token(state.inner())?;
    crate::localhost_api::sync_localhost_api_runtime(&app, state.inner())?;
    Ok(token)
}

/// 测试远程存储连接
#[tauri::command]
pub async fn test_remote_storage(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let (remote_config, data_dir) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (guard.config.remote_storage.clone(), guard.data_dir.clone())
    };

    if remote_config.provider == work_review_core::config::RemoteStorageProvider::None {
        return Err(AppError::Config("未配置远程存储".into()));
    }

    let test_path = data_dir.join("remote_test.jpg");
    let test_bytes: Vec<u8> = {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([128, 128, 128]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg)
            .map_err(|e| AppError::Screenshot(format!("创建测试图片失败: {e}")))?;
        buf.into_inner()
    };
    tokio::fs::write(&test_path, &test_bytes)
        .await
        .map_err(|e| AppError::Screenshot(format!("写入测试文件失败: {e}")))?;

    let client = reqwest::Client::new();
    let result = crate::remote_upload::upload_screenshot(
        &client,
        &remote_config,
        &test_path,
        "test/connection-test.jpg",
    )
    .await;

    let _ = tokio::fs::remove_file(&test_path).await;

    match result {
        Ok(url) => Ok(format!("连接成功: {url}")),
        Err(e) => Err(e),
    }
}

