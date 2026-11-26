//! 主窗口管理、生命周期守卫与系统/瞬态窗口过滤。

use crate::{commands, monitor, AppState, MAIN_WINDOW_LABEL};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Position};
#[cfg(target_os = "windows")]
pub(crate) fn build_windows_window_icon() -> Option<tauri::image::Image<'static>> {
    match image::load_from_memory_with_format(
        include_bytes!("../icons/windows-icon.png"),
        image::ImageFormat::Png,
    ) {
        Ok(decoded) => {
            let decoded = if decoded.width() > 256 || decoded.height() > 256 {
                decoded.resize_exact(256, 256, image::imageops::FilterType::Lanczos3)
            } else {
                decoded
            };

            let rgba = decoded.to_rgba8();
            let (width, height) = rgba.dimensions();
            Some(tauri::image::Image::new_owned(
                rgba.into_raw(),
                width,
                height,
            ))
        }
        Err(e) => {
            log::warn!("加载 Windows 专用窗口图标失败，回退默认图标: {e}");
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainWindowCloseBehavior {
    HideToTray,
    CloseWindow,
}

pub(crate) fn main_window_close_behavior(lightweight_mode: bool) -> MainWindowCloseBehavior {
    if lightweight_mode {
        MainWindowCloseBehavior::CloseWindow
    } else {
        MainWindowCloseBehavior::HideToTray
    }
}

pub(crate) fn effective_dock_visibility(
    hide_dock_icon: bool,
    lightweight_mode: bool,
    has_main_window: bool,
) -> bool {
    !hide_dock_icon && (!lightweight_mode || has_main_window)
}

pub(crate) fn sync_effective_dock_visibility(app: &AppHandle) {
    let Some(state) = app.try_state::<Arc<Mutex<AppState>>>() else {
        return;
    };

    let (hide_dock_icon, lightweight_mode) = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        (state.config.hide_dock_icon, state.config.lightweight_mode)
    };
    let has_main_window = app.get_webview_window(MAIN_WINDOW_LABEL).is_some();
    let visible = effective_dock_visibility(hide_dock_icon, lightweight_mode, has_main_window);
    commands::apply_dock_visibility(visible, false);
}

pub(crate) fn configure_main_window(_window: &tauri::WebviewWindow) {
    #[cfg(target_os = "windows")]
    if let Some(icon) = build_windows_window_icon() {
        if let Err(e) = _window.set_icon(icon) {
            log::warn!("设置 Windows 主窗口图标失败，继续使用默认图标: {e}");
        }
    }

    #[cfg(target_os = "macos")]
    {
        use tauri::TitleBarStyle;

        let _ = _window.set_decorations(true);
        let _ = _window.set_title_bar_style(TitleBarStyle::Transparent);
        configure_main_window_collection_behavior(_window);
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn configure_main_window_collection_behavior(window: &tauri::WebviewWindow) {
    use cocoa::appkit::{NSWindow, NSWindowCollectionBehavior};
    use cocoa::base::id;

    if let Ok(ns_window) = window.ns_window() {
        unsafe {
            let ns_window = ns_window as id;
            let mut behavior = ns_window.collectionBehavior();
            behavior |= NSWindowCollectionBehavior::NSWindowCollectionBehaviorMoveToActiveSpace;
            ns_window.setCollectionBehavior_(behavior);
        }
    }
}

pub(crate) fn align_window_to_reference_monitor(
    window: &tauri::WebviewWindow,
    reference_window: Option<&tauri::WebviewWindow>,
) {
    let Some(reference_window) = reference_window else {
        return;
    };

    let Ok(Some(reference_monitor)) = reference_window.current_monitor() else {
        return;
    };
    let Ok(window_size) = window.outer_size() else {
        return;
    };

    let work_area = reference_monitor.work_area();
    let monitor_width = work_area.size.width as i32;
    let monitor_height = work_area.size.height as i32;
    let window_width = window_size.width as i32;
    let window_height = window_size.height as i32;

    let target_x = work_area.position.x + ((monitor_width - window_width).max(0) / 2);
    let target_y = work_area.position.y + ((monitor_height - window_height).max(0) / 2);

    let _ = window.set_position(Position::Physical(PhysicalPosition::new(
        target_x, target_y,
    )));
}

pub(crate) fn ensure_main_window(
    app: &AppHandle,
) -> Result<tauri::WebviewWindow, work_review_core::error::AppError> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        return Ok(window);
    }

    let window_config = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == MAIN_WINDOW_LABEL)
        .or_else(|| app.config().app.windows.first())
        .ok_or_else(|| {
            work_review_core::error::AppError::Unknown("未找到主窗口配置".to_string())
        })?;

    let window = tauri::WebviewWindowBuilder::from_config(app, window_config)
        .map_err(|e| {
            work_review_core::error::AppError::Unknown(format!("创建主窗口构建器失败: {e}"))
        })?
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .build()
        .map_err(|e| work_review_core::error::AppError::Unknown(format!("重建主窗口失败: {e}")))?;

    configure_main_window(&window);
    Ok(window)
}

pub(crate) fn reveal_main_window(
    app: &AppHandle,
    source_window_label: Option<&str>,
) -> Result<(), work_review_core::error::AppError> {
    let window = ensure_main_window(app)?;
    let reference_window = source_window_label.and_then(|label| app.get_webview_window(label));
    align_window_to_reference_monitor(&window, reference_window.as_ref());
    let _ = window.unminimize();
    let _ = window.show();
    let _ = app.emit("main-window-visibility", true);
    let _ = window.set_focus();
    sync_effective_dock_visibility(app);
    Ok(())
}

#[derive(Default)]
pub(crate) struct AppLifecycleState {
    pub(crate) suppress_next_exit: bool,
    pub(crate) explicit_quit_requested: bool,
}

pub(crate) fn should_prevent_exit(suppress_next_exit: bool, explicit_quit_requested: bool) -> bool {
    suppress_next_exit && !explicit_quit_requested
}

pub(crate) struct WindowsSystemDialogRule {
    executable_names: &'static [&'static str],
    exact_window_texts: &'static [&'static str],
}

pub(crate) const WINDOWS_SYSTEM_DIALOG_RULES: &[WindowsSystemDialogRule] = &[
    WindowsSystemDialogRule {
        executable_names: &["taskmgr"],
        exact_window_texts: &[
            "task manager",
            "task manager (not responding)",
            "任务管理器",
            "任务管理器 (未响应)",
            "任务管理器（未响应）",
        ],
    },
    WindowsSystemDialogRule {
        executable_names: &["consent", "credentialuibroker"],
        exact_window_texts: &[
            "user account control",
            "windows security",
            "用户账户控制",
            "用户帐户控制",
            "windows 安全",
            "windows 安全中心",
        ],
    },
];

pub(crate) fn normalized_windows_system_window_text(value: &str) -> String {
    value.trim().to_lowercase()
}

pub(crate) fn windows_path_file_stem(path: &str) -> Option<String> {
    let file_name = path
        .rsplit(['\\', '/'])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let lowered = file_name.to_lowercase();
    let stem = lowered.strip_suffix(".exe").unwrap_or(&lowered).trim();

    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

pub(crate) fn windows_executable_name(active_window: &monitor::ActiveWindow) -> Option<String> {
    active_window
        .executable_path
        .as_deref()
        .and_then(windows_path_file_stem)
        .or_else(|| {
            let lowered = active_window.app_name.trim().to_lowercase();
            let normalized_name = lowered.strip_suffix(".exe").unwrap_or(&lowered).trim();

            if normalized_name.is_empty() {
                None
            } else {
                Some(normalized_name.to_string())
            }
        })
}

pub(crate) fn matches_windows_system_dialog_rule(
    active_window: &monitor::ActiveWindow,
    rule: &WindowsSystemDialogRule,
) -> bool {
    let executable_name = windows_executable_name(active_window);
    if executable_name.as_deref().is_some_and(|name| {
        rule.executable_names
            .iter()
            .any(|candidate| candidate == &name)
    }) {
        return true;
    }

    let app_name = normalized_windows_system_window_text(&active_window.app_name);
    let window_title = normalized_windows_system_window_text(&active_window.window_title);
    let allow_exact_text_fallback =
        !app_name.is_empty() && (window_title.is_empty() || app_name == window_title);

    allow_exact_text_fallback
        && rule
            .exact_window_texts
            .iter()
            .any(|candidate| candidate == &app_name)
}

pub(crate) fn is_windows_system_dialog(active_window: &monitor::ActiveWindow) -> bool {
    WINDOWS_SYSTEM_DIALOG_RULES
        .iter()
        .any(|rule| matches_windows_system_dialog_rule(active_window, rule))
}

pub(crate) fn should_skip_transient_window(active_window: &monitor::ActiveWindow) -> bool {
    let app_lower = active_window.app_name.trim().to_lowercase();
    matches!(
        app_lower.as_str(),
        "dock"
            | "systemuiserver"
            | "control center"
            | "spotlight"
            | "notificationcenter"
            | "loginwindow"
            | "screencaptureui"
            | "universalaccessauthwarn"
            | "windowmanager"
            | "wallpaper"
    )
}

pub(crate) fn should_skip_system_window(active_window: &monitor::ActiveWindow) -> bool {
    let is_sys = work_review_core::categorize::is_system_process(&active_window.app_name);
    let is_minimized_window = active_window.is_minimized;
    let is_explorer_shell = {
        let name_lower = active_window.app_name.trim().to_lowercase();
        let name_trimmed = name_lower.strip_suffix(".exe").unwrap_or(&name_lower);
        (name_trimmed == "explorer" || name_trimmed == "file explorer")
            && active_window.window_title.is_empty()
    };
    // Windows 在 UAC / 任务管理器异常时，进程名可能退化成标题或受保护进程名，
    // 需要结合标题与可执行路径一起兜底过滤。
    let is_windows_system_dialog = is_windows_system_dialog(active_window);

    is_sys || is_minimized_window || is_explorer_shell || is_windows_system_dialog
}
