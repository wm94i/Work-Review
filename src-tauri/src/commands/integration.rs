//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::AppState;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use work_review_core::error::AppError;

use super::shared::update_app_config;

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
    let bind_code_expired = s.config.telegram_bot_bind_code.is_some()
        && s.config
            .telegram_bot_bind_code_expires_at
            .map(|expires_at| expires_at < now_ts)
            .unwrap_or(true);
    Ok(serde_json::json!({
        "running": s.telegram_bot_runtime.is_running(),
        "starting": s.telegram_bot_runtime.is_starting(),
        "lastError": s.telegram_bot_runtime.last_error(),
        "allowedChatIds": s.config.telegram_bot_allowed_chat_ids.clone(),
        "bindCode": if bind_code_expired { None } else { s.config.telegram_bot_bind_code.clone() },
        "bindCodeExpiresAt": s.config.telegram_bot_bind_code_expires_at,
        "bindCodeExpired": bind_code_expired,
    }))
}

#[tauri::command]
pub async fn get_wecom_bot_status(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(serde_json::json!({
        "running": s.wecom_bot_runtime.is_running(),
        "starting": s.wecom_bot_runtime.is_starting(),
        "lastError": s.wecom_bot_runtime.last_error(),
        "longConnectionConfigured": crate::wecom_aibot::long_connection_configured(&s.config),
        "callbackConfigured": crate::wecom_aibot::callback_configured(&s.config),
    }))
}

#[tauri::command]
pub async fn generate_telegram_bot_bind_code(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    const TELEGRAM_BIND_CODE_TTL_SECONDS: i64 = 10 * 60;

    let raw = uuid::Uuid::new_v4().simple().to_string();
    let code = format!("WR-{}", raw[..8].to_ascii_uppercase());
    let expires_at = chrono::Local::now().timestamp() + TELEGRAM_BIND_CODE_TTL_SECONDS;
    update_app_config(app, state.inner(), |current| {
        let mut config = current.clone();
        config.telegram_bot_bind_code = Some(code.clone());
        config.telegram_bot_bind_code_expires_at = Some(expires_at);
        Ok(config)
    })?;
    crate::telegram_bot::reset_telegram_bind_failed_attempts();

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

/// 测试远程存储连接。
/// 接收设置页表单的当前值直接测试（而非后端已保存配置）：设置更改需点「保存」
/// 才落盘，此前命令读已保存配置，填完表单未保存就点「测试连接」会误报
/// 「未配置远程存储」。所测即所见；实际录制上传仍以保存后的配置为准。
#[tauri::command]
pub async fn test_remote_storage(
    remote_storage: work_review_core::config::RemoteStorageConfig,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let data_dir = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.data_dir.clone()
    };

    if remote_storage.provider == work_review_core::config::RemoteStorageProvider::None {
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

    let result = crate::remote_upload::upload_screenshot(
        &remote_storage,
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

fn ensure_bot_sender_platform(platform: &str) -> Result<(), AppError> {
    if crate::bot_common::BOT_SENDER_PLATFORMS.contains(&platform) {
        Ok(())
    } else {
        Err(AppError::Config(format!("不支持的 Bot 平台: {platform}")))
    }
}

/// 发送者 ID 只展示首尾，避免设置页截图泄露完整 ID。
fn mask_sender_id(sender_id: &str) -> String {
    let chars: Vec<char> = sender_id.chars().collect();
    if chars.len() <= 6 {
        return "*".repeat(chars.len().max(1));
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 3..].iter().collect();
    format!("{head}…{tail}")
}

#[tauri::command]
pub async fn get_bot_sender_binding_status(
    platform: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    ensure_bot_sender_platform(&platform)?;
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let now_ts = chrono::Local::now().timestamp();
    let binding = s
        .config
        .bot_sender_bindings
        .get(&platform)
        .cloned()
        .unwrap_or_default();
    // 与授权判断（evaluate_bot_sender）一致：有绑定码但没有过期时间也视为已过期
    let expired = binding.bind_code.is_some()
        && binding
            .bind_code_expires_at
            .map(|expires_at| expires_at < now_ts)
            .unwrap_or(true);
    Ok(serde_json::json!({
        "allowedCount": binding.allowed_senders.len(),
        "allowedSenders": binding
            .allowed_senders
            .iter()
            .map(|id| mask_sender_id(id))
            .collect::<Vec<_>>(),
        "bindCode": if expired { None } else { binding.bind_code.clone() },
        "bindCodeExpiresAt": binding.bind_code_expires_at,
        "bindCodeExpired": expired,
    }))
}

#[tauri::command]
pub async fn generate_bot_sender_bind_code(
    platform: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    ensure_bot_sender_platform(&platform)?;
    let raw = uuid::Uuid::new_v4().simple().to_string();
    let code = format!("WR-{}", raw[..8].to_ascii_uppercase());
    let expires_at =
        chrono::Local::now().timestamp() + crate::bot_common::BOT_BIND_CODE_TTL_SECONDS;
    update_app_config(app, state.inner(), |current| {
        let mut config = current.clone();
        let binding = config.bot_sender_bindings.entry(platform).or_default();
        binding.bind_code = Some(code.clone());
        binding.bind_code_expires_at = Some(expires_at);
        binding.failed_attempts = 0;
        Ok(config)
    })?;
    Ok(serde_json::json!({
        "code": code,
        "expiresAt": expires_at,
    }))
}

#[tauri::command]
pub async fn clear_bot_sender_bindings(
    platform: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    ensure_bot_sender_platform(&platform)?;
    update_app_config(app, state.inner(), |current| {
        let mut config = current.clone();
        config.bot_sender_bindings.remove(&platform);
        Ok(config)
    })
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{ensure_bot_sender_platform, mask_sender_id};

    #[test]
    fn 只接受支持的bot平台() {
        assert!(ensure_bot_sender_platform("feishu").is_ok());
        assert!(ensure_bot_sender_platform("dingtalk").is_ok());
        assert!(ensure_bot_sender_platform("wecom").is_ok());
        assert!(ensure_bot_sender_platform("telegram").is_err());
        assert!(ensure_bot_sender_platform("../x").is_err());
    }

    #[test]
    fn 发送者id应脱敏展示() {
        assert_eq!(mask_sender_id("ou_1234567890"), "ou_…890");
        assert_eq!(mask_sender_id("abc"), "***");
        assert_eq!(mask_sender_id(""), "*");
    }
}
