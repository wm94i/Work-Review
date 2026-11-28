//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

#[cfg(target_os = "linux")]
use crate::linux_session::{current_linux_desktop_environment, current_linux_desktop_session};
use crate::AppState;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};
use work_review_core::config::AvatarFollowupItem;
use work_review_core::error::AppError;

use super::shared::update_app_config;

#[cfg(target_os = "linux")]
const GNOME_AVATAR_EXTENSION_UUID: &str = "work-review-avatar-input@workreview.app";

#[cfg(target_os = "linux")]
const GNOME_AVATAR_EXTENSION_METADATA: &str = include_str!(
    "../../../scripts/gnome-shell/work-review-avatar-input@workreview.app/metadata.json"
);

#[cfg(target_os = "linux")]
const GNOME_AVATAR_EXTENSION_SOURCE: &str = include_str!(
    "../../../scripts/gnome-shell/work-review-avatar-input@workreview.app/extension.js"
);

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GnomeAvatarExtensionInstallResult {
    pub installed: bool,
    pub enabled: bool,
    pub requires_relogin: bool,
    pub extension_dir: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AvatarFollowupActionInput {
    pub action: String,
    pub project_key: String,
    pub title: String,
    pub date: String,
    pub source_app: String,
    pub source_title: String,
    pub persona: String,
}

#[cfg(target_os = "linux")]
fn gnome_avatar_extension_install_dir() -> Result<PathBuf, AppError> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))
        .ok_or_else(|| AppError::Unknown("无法定位当前用户的数据目录".to_string()))?;

    Ok(data_home
        .join("gnome-shell")
        .join("extensions")
        .join(GNOME_AVATAR_EXTENSION_UUID))
}

#[cfg(target_os = "linux")]
pub(crate) fn is_gnome_avatar_extension_installed() -> bool {
    gnome_avatar_extension_install_dir()
        .ok()
        .map(|dir| dir.join("metadata.json").exists() && dir.join("extension.js").exists())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub(crate) fn is_gnome_avatar_extension_enabled() -> bool {
    std::process::Command::new("gnome-extensions")
        .args(["list", "--enabled"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim() == GNOME_AVATAR_EXTENSION_UUID)
        })
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub(crate) fn gnome_avatar_extension_needs_relogin(
    desktop_environment: crate::linux_session::LinuxDesktopEnvironment,
    installed: bool,
    enabled: bool,
    avatar_input_support: &crate::avatar_input::LinuxAvatarInputSupport,
) -> bool {
    desktop_environment == crate::linux_session::LinuxDesktopEnvironment::Gnome
        && installed
        && enabled
        && avatar_input_support.provider != "gnome-shell-dbus"
}

/// 获取当前桌宠状态
#[tauri::command]
pub async fn get_avatar_state(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<crate::avatar_engine::AvatarStatePayload, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(state.avatar_state.clone())
}

/// 保存桌宠窗口位置
#[tauri::command]
pub async fn save_avatar_position(
    x: i32,
    y: i32,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    crate::avatar_engine::clear_pre_expansion_anchor_if_moved(x, y);
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let config_path = state.config_path.clone();

    state.config.avatar_x = Some(x);
    state.config.avatar_y = Some(y);
    state.config.save(&config_path)?;

    Ok(())
}

/// 根据气泡/卡片展开状态调整桌宠窗口尺寸
#[tauri::command]
pub async fn set_avatar_window_expanded(
    expanded: bool,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let (scale, body_hidden) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (state.config.avatar_scale, state.config.avatar_body_hidden)
    };
    crate::avatar_engine::apply_avatar_window_expansion(&app, scale, expanded, body_hidden)
        .map_err(|e| AppError::Unknown(format!("调整桌宠窗口尺寸失败: {e}")))
}

/// 更新隐藏桌宠时可交互的气泡/卡片区域。
#[tauri::command]
pub async fn set_avatar_interactive_regions(
    precise: bool,
    regions: Vec<crate::avatar_engine::AvatarInteractiveRegion>,
) -> Result<(), AppError> {
    crate::avatar_engine::update_avatar_interactive_regions(precise, regions);
    crate::avatar_input::force_resync_click_through();
    Ok(())
}

/// 从桌面助手窗口读取当前位置并持久化
#[tauri::command]
pub async fn persist_avatar_position(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<bool, AppError> {
    let Some(window) = app.get_webview_window(crate::avatar_engine::AVATAR_WINDOW_LABEL) else {
        return Ok(false);
    };

    let position = window
        .outer_position()
        .map_err(|e| AppError::Unknown(format!("读取桌面助手位置失败: {e}")))?;

    crate::avatar_engine::clear_pre_expansion_anchor_if_moved(position.x, position.y);
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let config_path = state.config_path.clone();

    state.config.avatar_x = Some(position.x);
    state.config.avatar_y = Some(position.y);
    state.config.save(&config_path)?;

    Ok(true)
}

/// 显示主窗口
#[tauri::command]
pub async fn show_main_window(
    app: AppHandle,
    source_window_label: Option<String>,
) -> Result<(), AppError> {
    crate::reveal_main_window(&app, source_window_label.as_deref())
}

#[tauri::command]
pub async fn handle_avatar_followup_action(
    input: AvatarFollowupActionInput,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let project_key = input.project_key.trim().to_string();
    if project_key.is_empty() {
        return Err(AppError::Unknown(
            "缺少项目标识，无法处理桌宠待跟进动作".to_string(),
        ));
    }

    let action = match input.action.trim() {
        "timeline" => crate::avatar_followup::AvatarFollowupAction::Timeline,
        "focus" => crate::avatar_followup::AvatarFollowupAction::Focus,
        "remember" => crate::avatar_followup::AvatarFollowupAction::Remember,
        "snooze" => crate::avatar_followup::AvatarFollowupAction::Snooze,
        _ => crate::avatar_followup::AvatarFollowupAction::Dismiss,
    };

    if matches!(
        action,
        crate::avatar_followup::AvatarFollowupAction::Remember
    ) {
        let normalized_title = input.title.trim().to_string();
        let already_exists = {
            let state_guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
            state_guard.config.avatar_followups.iter().any(|item| {
                item.status == "open"
                    && item.project_key == project_key
                    && item.title.trim() == normalized_title
            })
        };

        if !already_exists {
            let project_key_for_save = project_key.clone();
            let date = input.date.trim().to_string();
            let source_app = input.source_app.trim().to_string();
            let source_title = input.source_title.trim().to_string();
            let created_at = chrono::Local::now().timestamp();

            update_app_config(app.clone(), state.inner(), move |current| {
                let mut next = current.clone();
                let exists = next.avatar_followups.iter().any(|item| {
                    item.status == "open"
                        && item.project_key == project_key_for_save
                        && item.title.trim() == normalized_title
                });
                if !exists {
                    next.avatar_followups.push(AvatarFollowupItem {
                        id: uuid::Uuid::new_v4().to_string(),
                        title: normalized_title,
                        date,
                        source_app,
                        source_title,
                        project_key: project_key_for_save,
                        created_at,
                        status: "open".to_string(),
                    });
                }
                Ok(next)
            })?;
        }
    }

    crate::avatar_followup::apply_followup_action(&project_key, action, &input.persona);

    Ok(serde_json::json!({
        "ok": true,
        "action": input.action,
        "projectKey": project_key,
    }))
}

#[tauri::command]
pub async fn install_gnome_avatar_extension() -> Result<GnomeAvatarExtensionInstallResult, AppError>
{
    #[cfg(target_os = "linux")]
    {
        let session = current_linux_desktop_session();
        let desktop_environment = current_linux_desktop_environment();

        if desktop_environment != crate::linux_session::LinuxDesktopEnvironment::Gnome {
            return Err(AppError::Unknown(
                "当前不是 GNOME 会话，无法自动安装 GNOME 桌宠扩展".to_string(),
            ));
        }

        let install_dir = gnome_avatar_extension_install_dir()?;
        std::fs::create_dir_all(&install_dir)
            .map_err(|e| AppError::Unknown(format!("创建 GNOME 扩展目录失败: {e}")))?;
        std::fs::write(
            install_dir.join("metadata.json"),
            GNOME_AVATAR_EXTENSION_METADATA,
        )
        .map_err(|e| AppError::Unknown(format!("写入 GNOME 扩展 metadata 失败: {e}")))?;
        std::fs::write(
            install_dir.join("extension.js"),
            GNOME_AVATAR_EXTENSION_SOURCE,
        )
        .map_err(|e| AppError::Unknown(format!("写入 GNOME 扩展脚本失败: {e}")))?;

        let enabled = std::process::Command::new("gnome-extensions")
            .args(["enable", GNOME_AVATAR_EXTENSION_UUID])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some()
            || is_gnome_avatar_extension_enabled();
        let avatar_input_support = crate::avatar_input::current_linux_avatar_input_support();
        let requires_relogin = gnome_avatar_extension_needs_relogin(
            desktop_environment,
            true,
            enabled,
            &avatar_input_support,
        );

        let message = if requires_relogin {
            "GNOME 桌宠扩展已启用，但当前 GNOME Shell 还未加载最新扩展。请重新登录后再试。"
                .to_string()
        } else if enabled {
            format!(
                "GNOME 桌宠扩展已安装并启用（{} / {}）",
                session.as_str(),
                desktop_environment.as_str()
            )
        } else {
            "GNOME 桌宠扩展文件已写入，请确认系统已安装 gnome-extensions 并手动启用扩展".to_string()
        };

        Ok(GnomeAvatarExtensionInstallResult {
            installed: true,
            enabled,
            requires_relogin,
            extension_dir: install_dir.to_string_lossy().to_string(),
            message,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(AppError::Unknown(
            "只有 Linux GNOME 会话支持自动安装桌宠扩展".to_string(),
        ))
    }
}
