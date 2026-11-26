//! 系统托盘菜单、托盘图标与录制/配置事件广播。

use crate::{commands, AppState};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::menu::{CheckMenuItem, MenuItem};
use tauri::{AppHandle, Emitter, Manager};
use work_review_core::config::AppConfig;
pub(crate) const TRAY_MENU_SHOW_ID: &str = "show";
pub(crate) const TRAY_MENU_RECORDING_TOGGLE_ID: &str = "recording-toggle";
pub(crate) const TRAY_MENU_LIGHTWEIGHT_MODE_ID: &str = "lightweight-mode";
pub(crate) const TRAY_MENU_AVATAR_TOGGLE_ID: &str = "avatar-toggle";
pub(crate) const TRAY_MENU_QUIT_ID: &str = "quit";
pub(crate) const RECORDING_STATE_CHANGED_EVENT: &str = "recording-state-changed";
pub(crate) const CONFIG_CHANGED_EVENT: &str = "config-changed";

type AppMenuItem = MenuItem<tauri::Wry>;
type AppCheckMenuItem = CheckMenuItem<tauri::Wry>;

pub(crate) struct TrayMenuState {
    pub(crate) show: AppMenuItem,
    pub(crate) recording_toggle: AppMenuItem,
    pub(crate) lightweight_mode: AppCheckMenuItem,
    pub(crate) avatar_toggle: AppCheckMenuItem,
    pub(crate) quit: AppMenuItem,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordingStatePayload {
    pub is_recording: bool,
    pub is_paused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingToggleAction {
    Start,
    Pause,
    Resume,
}

pub(crate) fn tray_recording_toggle_action(
    is_recording: bool,
    is_paused: bool,
) -> RecordingToggleAction {
    if !is_recording {
        RecordingToggleAction::Start
    } else if is_paused {
        RecordingToggleAction::Resume
    } else {
        RecordingToggleAction::Pause
    }
}

/// 托盘菜单文案的三语映射（后端独立于前端 i18n，因为托盘在 Rust 端构造）。
pub(crate) fn tray_label(key: &str, locale: &str) -> &'static str {
    match (key, locale) {
        ("show", "en") => "Show Window",
        ("show", "zh-TW") => "顯示視窗",
        ("show", _) => "显示窗口",
        ("quit", "en") => "Quit",
        ("quit", "zh-TW") => "結束",
        ("quit", _) => "退出",
        ("lightweight", "en") => "Lightweight Mode",
        ("lightweight", "zh-TW") => "輕量模式",
        ("lightweight", _) => "轻量模式",
        ("avatar", "en") => "Desktop Pet",
        ("avatar", "zh-TW") => "桌寵",
        ("avatar", _) => "桌宠",
        ("recording_start", "en") => "Start Recording",
        ("recording_start", "zh-TW") => "開始錄製",
        ("recording_start", _) => "开始录制",
        ("recording_pause", "en") => "Pause",
        ("recording_pause", "zh-TW") => "暫停錄製",
        ("recording_pause", _) => "暂停录制",
        ("recording_resume", "en") => "Resume",
        ("recording_resume", "zh-TW") => "恢復錄製",
        ("recording_resume", _) => "恢复录制",
        _ => "",
    }
}

pub(crate) fn tray_recording_toggle_label(
    is_recording: bool,
    is_paused: bool,
    locale: &str,
) -> &'static str {
    match tray_recording_toggle_action(is_recording, is_paused) {
        RecordingToggleAction::Start => tray_label("recording_start", locale),
        RecordingToggleAction::Pause => tray_label("recording_pause", locale),
        RecordingToggleAction::Resume => tray_label("recording_resume", locale),
    }
}

pub(crate) fn refresh_tray_menu(app: &AppHandle) {
    let Some(tray_menu) = app.try_state::<TrayMenuState>() else {
        return;
    };
    let Some(state) = app.try_state::<Arc<Mutex<AppState>>>() else {
        return;
    };

    let (is_recording, is_paused, lightweight_mode, avatar_enabled, locale) = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        (
            state.is_recording,
            state.is_paused,
            state.config.lightweight_mode,
            state.config.avatar_enabled,
            state.config.locale.clone(),
        )
    };

    let _ = tray_menu.show.set_text(tray_label("show", &locale));
    let _ = tray_menu
        .recording_toggle
        .set_text(tray_recording_toggle_label(
            is_recording,
            is_paused,
            &locale,
        ));
    let _ = tray_menu
        .lightweight_mode
        .set_text(tray_label("lightweight", &locale));
    let _ = tray_menu.lightweight_mode.set_checked(lightweight_mode);
    let _ = tray_menu
        .avatar_toggle
        .set_text(tray_label("avatar", &locale));
    let _ = tray_menu.avatar_toggle.set_checked(avatar_enabled);
    let _ = tray_menu.quit.set_text(tray_label("quit", &locale));
}

/// 前端切换语言时同步到后端 config，并刷新托盘菜单文案
#[tauri::command]
pub(crate) async fn set_app_locale(
    locale: String,
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), work_review_core::error::AppError> {
    let normalized = match locale.as_str() {
        v if v.starts_with("en") => "en",
        "zh-TW" | "zh-HK" => "zh-TW",
        _ => "zh-CN",
    };
    let config = {
        let mut s = state
            .lock()
            .map_err(|e| work_review_core::error::AppError::Unknown(e.to_string()))?;
        s.config.locale = normalized.to_string();
        s.config.clone()
    };
    commands::persist_app_config(config, app.clone(), state.inner())?;
    refresh_tray_menu(&app);
    Ok(())
}

pub(crate) fn emit_recording_state_changed(app: &AppHandle) {
    let Some(state) = app.try_state::<Arc<Mutex<AppState>>>() else {
        return;
    };

    let payload = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        RecordingStatePayload {
            is_recording: state.is_recording,
            is_paused: state.is_paused,
        }
    };

    let _ = app.emit(RECORDING_STATE_CHANGED_EVENT, payload);
    refresh_tray_menu(app);
}

pub(crate) fn emit_config_changed(app: &AppHandle, config: &AppConfig) {
    let _ = app.emit(CONFIG_CHANGED_EVENT, config);
    refresh_tray_menu(app);
}

pub(crate) fn build_tray_icon(app: &tauri::App) -> tauri::image::Image<'static> {
    #[cfg(target_os = "macos")]
    {
        match image::load_from_memory_with_format(
            include_bytes!("../icons/tray-template.png"),
            image::ImageFormat::Png,
        ) {
            Ok(decoded) => {
                let rgba = decoded.to_rgba8();
                let (width, height) = rgba.dimensions();
                tauri::image::Image::new_owned(rgba.into_raw(), width, height)
            }
            Err(e) => {
                log::warn!("加载 macOS 状态栏专用图标失败，回退默认图标: {e}");
                app.default_window_icon()
                    .expect("应用默认图标缺失")
                    .clone()
                    .to_owned()
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        app.default_window_icon()
            .expect("应用默认图标缺失")
            .clone()
            .to_owned()
    }
}
