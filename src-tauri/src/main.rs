// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// macOS: objc 宏（msg_send!, class! 等）需要 macro_use 全局导入
#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

mod activity_classifier;
mod agent;
mod analysis;
mod autostart;
mod avatar_engine;
mod avatar_followup;
#[allow(dead_code)]
mod avatar_input;
mod avatar_proactive;
mod bot_common;
mod commands;
mod config;
mod database;
mod dingtalk_bot;
mod error;
mod feishu_bot;
mod idle_detector;
mod linux_session;
mod localhost_api;
mod monitor;
mod node_gateway;
mod ocr;
mod privacy;
mod remote_upload;
mod screen_lock;
mod screenshot;
mod storage;
mod telegram_bot;
mod wecom_bot;
mod work_intelligence;

use config::{config_backup_path, AppConfig, AvatarFollowupItem, ConfigLoadStatus};
use database::Database;
use once_cell::sync::OnceCell;
use privacy::PrivacyFilter;
use screenshot::ScreenshotService;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage::StorageManager;
use tauri::menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItem, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Position};

// 全局 AppHandle，用于在 macOS Dock 点击时恢复窗口
static APP_HANDLE: OnceCell<AppHandle> = OnceCell::new();
const MAIN_WINDOW_LABEL: &str = "main";
const AUTOSTART_LAUNCH_ARG: &str = "--autostart";
const TRAY_MENU_SHOW_ID: &str = "show";
const TRAY_MENU_RECORDING_TOGGLE_ID: &str = "recording-toggle";
const TRAY_MENU_LIGHTWEIGHT_MODE_ID: &str = "lightweight-mode";
const TRAY_MENU_AVATAR_TOGGLE_ID: &str = "avatar-toggle";
const TRAY_MENU_QUIT_ID: &str = "quit";
pub(crate) const RECORDING_STATE_CHANGED_EVENT: &str = "recording-state-changed";
pub(crate) const CONFIG_CHANGED_EVENT: &str = "config-changed";

type AppMenuItem = MenuItem<tauri::Wry>;
type AppCheckMenuItem = CheckMenuItem<tauri::Wry>;

pub(crate) struct TrayMenuState {
    show: AppMenuItem,
    recording_toggle: AppMenuItem,
    lightweight_mode: AppCheckMenuItem,
    avatar_toggle: AppCheckMenuItem,
    quit: AppMenuItem,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordingStatePayload {
    pub is_recording: bool,
    pub is_paused: bool,
}

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
enum MainWindowCloseBehavior {
    HideToTray,
    CloseWindow,
}

fn main_window_close_behavior(lightweight_mode: bool) -> MainWindowCloseBehavior {
    if lightweight_mode {
        MainWindowCloseBehavior::CloseWindow
    } else {
        MainWindowCloseBehavior::HideToTray
    }
}

fn effective_dock_visibility(
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
fn configure_main_window_collection_behavior(window: &tauri::WebviewWindow) {
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

fn align_window_to_reference_monitor(
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

pub(crate) fn ensure_main_window(app: &AppHandle) -> Result<tauri::WebviewWindow, error::AppError> {
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
        .ok_or_else(|| error::AppError::Unknown("未找到主窗口配置".to_string()))?;

    let window = tauri::WebviewWindowBuilder::from_config(app, window_config)
        .map_err(|e| error::AppError::Unknown(format!("创建主窗口构建器失败: {e}")))?
        .build()
        .map_err(|e| error::AppError::Unknown(format!("重建主窗口失败: {e}")))?;

    configure_main_window(&window);
    Ok(window)
}

pub(crate) fn reveal_main_window(
    app: &AppHandle,
    source_window_label: Option<&str>,
) -> Result<(), error::AppError> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingToggleAction {
    Start,
    Pause,
    Resume,
}

fn tray_recording_toggle_action(is_recording: bool, is_paused: bool) -> RecordingToggleAction {
    if !is_recording {
        RecordingToggleAction::Start
    } else if is_paused {
        RecordingToggleAction::Resume
    } else {
        RecordingToggleAction::Pause
    }
}

/// 托盘菜单文案的三语映射（后端独立于前端 i18n，因为托盘在 Rust 端构造）。
fn tray_label(key: &str, locale: &str) -> &'static str {
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

fn tray_recording_toggle_label(is_recording: bool, is_paused: bool, locale: &str) -> &'static str {
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
async fn set_app_locale(
    locale: String,
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), crate::error::AppError> {
    let normalized = match locale.as_str() {
        v if v.starts_with("en") => "en",
        "zh-TW" | "zh-HK" => "zh-TW",
        _ => "zh-CN",
    };
    let config = {
        let mut s = state
            .lock()
            .map_err(|e| crate::error::AppError::Unknown(e.to_string()))?;
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

fn build_tray_icon(app: &tauri::App) -> tauri::image::Image<'static> {
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

/// 应用状态
pub struct AppState {
    pub config: AppConfig,
    pub config_load_status: ConfigLoadStatus,
    pub database: Database,
    pub privacy_filter: PrivacyFilter,
    pub screenshot_service: ScreenshotService,
    pub storage_manager: StorageManager,
    pub data_dir: PathBuf,
    pub config_path: PathBuf,
    pub is_recording: bool,
    pub is_paused: bool,
    pub avatar_state: avatar_engine::AvatarStatePayload,
    pub avatar_generating_report: bool,
    pub generating_report: bool,
    pub localhost_api_runtime: localhost_api::LocalhostApiRuntime,
    pub telegram_bot_runtime: telegram_bot::TelegramBotRuntime,
    /// avatar 循环缓存的活动窗口（时间戳 + 窗口信息），供 screenshot 循环复用
    pub cached_active_window: Option<(std::time::Instant, monitor::ActiveWindow)>,
}

#[derive(Default)]
pub(crate) struct AppLifecycleState {
    suppress_next_exit: bool,
    explicit_quit_requested: bool,
}

#[derive(Serialize, Deserialize)]
struct DataDirPreference {
    data_dir: String,
}

fn should_prevent_exit(suppress_next_exit: bool, explicit_quit_requested: bool) -> bool {
    suppress_next_exit && !explicit_quit_requested
}

fn launch_args_contain_autostart(args: &[String]) -> bool {
    args.iter().any(|arg| arg == AUTOSTART_LAUNCH_ARG)
}

fn args_include_explicit_hidden_flag(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "--hidden" | "--minimized"))
}

#[cfg(not(windows))]
fn launch_args_request_hidden_window(args: &[String]) -> bool {
    launch_args_contain_autostart(args) || args_include_explicit_hidden_flag(args)
}

fn duplicate_instance_should_stay_silent(args: &[String]) -> bool {
    launch_args_contain_autostart(args) || args_include_explicit_hidden_flag(args)
}

fn should_hide_main_window_on_setup(_config: &AppConfig, launch_args: &[String]) -> bool {
    #[cfg(windows)]
    {
        // Windows 下注册表参数由 silent 选择动态写入：silent 模式带 `--hidden`，
        // show 模式只写 `--autostart`。显隐决策直接看 launch args，
        // 不再依赖 config.json，彻底消除前端忘保存带来的失同步。
        args_include_explicit_hidden_flag(launch_args)
    }

    #[cfg(not(windows))]
    {
        // Non-Windows autostart state can be stale during early setup. Once
        // launch args prove this came from autostart/hidden launch, use the
        // user's silent-mode preference as the visibility source of truth.
        _config.auto_start_silent && launch_args_request_hidden_window(launch_args)
    }
}

fn should_request_screen_capture_permission(
    has_screen_capture_permission: bool,
    already_prompted: bool,
) -> bool {
    !has_screen_capture_permission && !already_prompted
}

fn should_initialize_startup_permissions(status: ConfigLoadStatus) -> bool {
    !status.requires_fail_safe()
}

fn should_run_startup_cleanup(status: ConfigLoadStatus) -> bool {
    !status.requires_fail_safe()
}

fn should_initialize_avatar_input(status: ConfigLoadStatus) -> bool {
    !status.requires_fail_safe()
}

/// 清理历史遗留的钥匙串占位符。
/// 早期未发布版本曾把敏感字段真实值迁入系统钥匙串,磁盘配置写成 "__keychain__" 占位符;
/// 该机制已整体移除。这里把残留占位符清空,避免被当作真实密钥使用（需在设置中重新填写）。
fn clear_legacy_keychain_placeholders(config: &mut crate::config::AppConfig) {
    const PLACEHOLDER: &str = "__keychain__";
    let mut cleared = 0u32;
    {
        let mut clear_opt = |field: &mut Option<String>| {
            if field.as_deref() == Some(PLACEHOLDER) {
                *field = None;
                cleared += 1;
            }
        };
        clear_opt(&mut config.text_model.api_key);
        clear_opt(&mut config.vision_model.api_key);
        clear_opt(&mut config.ai_provider.api_key);
        clear_opt(&mut config.openai_api_key);
        clear_opt(&mut config.assistant_search_api_key);
        clear_opt(&mut config.embedding_api_key);
        clear_opt(&mut config.telegram_bot_token);
        clear_opt(&mut config.feishu_app_secret);
        clear_opt(&mut config.feishu_verification_token);
        clear_opt(&mut config.feishu_encrypt_key);
        clear_opt(&mut config.wecom_token);
        clear_opt(&mut config.wecom_encoding_aes_key);
        clear_opt(&mut config.dingtalk_app_secret);
        for profile in config.text_model_profiles.iter_mut() {
            clear_opt(&mut profile.model_config.api_key);
        }
    }
    {
        let mut clear_str = |field: &mut String| {
            if field.as_str() == PLACEHOLDER {
                field.clear();
                cleared += 1;
            }
        };
        clear_str(&mut config.remote_storage.s3.access_key);
        clear_str(&mut config.remote_storage.s3.secret_key);
        clear_str(&mut config.remote_storage.webdav.password);
    }
    if cleared > 0 {
        log::warn!(
            "检测到 {cleared} 个历史钥匙串占位符（机制已移除），已清空对应字段，请在设置中重新填写密钥"
        );
    }
}

fn describe_config_file_issue(path: &Path, error: Option<&str>) -> String {
    match error {
        Some(error) => format!("{} 读取或解析失败: {error}", path.display()),
        None => format!("{} 不存在", path.display()),
    }
}

fn initial_recording_state(status: ConfigLoadStatus) -> (bool, bool) {
    if status.requires_fail_safe() {
        (false, true)
    } else {
        (true, false)
    }
}

struct WindowsSystemDialogRule {
    executable_names: &'static [&'static str],
    exact_window_texts: &'static [&'static str],
}

const WINDOWS_SYSTEM_DIALOG_RULES: &[WindowsSystemDialogRule] = &[
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

fn normalized_windows_system_window_text(value: &str) -> String {
    value.trim().to_lowercase()
}

fn windows_path_file_stem(path: &str) -> Option<String> {
    let file_name = path
        .rsplit(['\\', '/'])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let stem = file_name
        .strip_suffix(".exe")
        .or_else(|| file_name.strip_suffix(".EXE"))
        .unwrap_or(file_name)
        .trim();

    if stem.is_empty() {
        None
    } else {
        Some(stem.to_lowercase())
    }
}

fn windows_executable_name(active_window: &monitor::ActiveWindow) -> Option<String> {
    active_window
        .executable_path
        .as_deref()
        .and_then(windows_path_file_stem)
        .or_else(|| {
            let normalized_name = active_window
                .app_name
                .trim()
                .trim_end_matches(".exe")
                .trim_end_matches(".EXE")
                .trim();

            if normalized_name.is_empty() {
                None
            } else {
                Some(normalized_name.to_lowercase())
            }
        })
}

fn matches_windows_system_dialog_rule(
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

fn is_windows_system_dialog(active_window: &monitor::ActiveWindow) -> bool {
    WINDOWS_SYSTEM_DIALOG_RULES
        .iter()
        .any(|rule| matches_windows_system_dialog_rule(active_window, rule))
}

const BREAK_REMINDER_BUFFER_MINUTES: u64 = 5;
const BREAK_REMINDER_MESSAGE: &str = "该休息一下了，起来活动活动吧。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakReminderPhase {
    Counting,
    Cooldown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BreakReminderRuntime {
    phase: BreakReminderPhase,
    elapsed_ms: u64,
    bubble_visible: bool,
}

impl BreakReminderRuntime {
    fn new() -> Self {
        Self {
            phase: BreakReminderPhase::Counting,
            elapsed_ms: 0,
            bubble_visible: false,
        }
    }

    fn reset(&mut self) {
        self.phase = BreakReminderPhase::Counting;
        self.elapsed_ms = 0;
        self.bubble_visible = false;
    }

    fn reset_active_cycle(&mut self) {
        if self.phase == BreakReminderPhase::Counting {
            self.elapsed_ms = 0;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakReminderSignal {
    TickMillis(u64),
    #[allow(dead_code)]
    TickMinutes(u64),
    #[allow(dead_code)]
    Dismiss,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct BreakReminderAdvanceResult {
    should_emit: bool,
    should_clear: bool,
    payload: Option<avatar_engine::AvatarBubblePayload>,
}

fn advance_break_reminder(
    state: &mut BreakReminderRuntime,
    enabled: bool,
    interval_minutes: u64,
    signal: BreakReminderSignal,
) -> BreakReminderAdvanceResult {
    let mut result = BreakReminderAdvanceResult::default();

    if !enabled {
        if state.bubble_visible {
            result.should_clear = true;
            result.payload = Some(avatar_engine::AvatarBubblePayload::clear());
        }
        state.reset();
        return result;
    }

    match signal {
        BreakReminderSignal::Dismiss => {
            if state.bubble_visible {
                state.bubble_visible = false;
                result.should_clear = true;
                result.payload = Some(avatar_engine::AvatarBubblePayload::clear());
            }
            return result;
        }
        BreakReminderSignal::TickMillis(0) | BreakReminderSignal::TickMinutes(0) => return result,
        _ => {}
    }

    let delta_ms = match signal {
        BreakReminderSignal::TickMillis(value) => value,
        BreakReminderSignal::TickMinutes(value) => value.saturating_mul(60_000),
        BreakReminderSignal::Dismiss => 0,
    };

    match state.phase {
        BreakReminderPhase::Counting => {
            state.elapsed_ms = state.elapsed_ms.saturating_add(delta_ms);
            if state.elapsed_ms >= interval_minutes.saturating_mul(60_000) {
                state.phase = BreakReminderPhase::Cooldown;
                state.elapsed_ms = 0;
                state.bubble_visible = true;
                result.should_emit = true;
                result.payload = Some(avatar_engine::AvatarBubblePayload::persistent_info(
                    BREAK_REMINDER_MESSAGE,
                ));
            }
        }
        BreakReminderPhase::Cooldown => {
            state.elapsed_ms = state.elapsed_ms.saturating_add(delta_ms);
            if state.elapsed_ms >= BREAK_REMINDER_BUFFER_MINUTES.saturating_mul(60_000) {
                state.phase = BreakReminderPhase::Counting;
                state.elapsed_ms = 0;
            }
        }
    }

    result
}

const AVATAR_SWITCH_NUDGE_WINDOW_MS: u64 = 3 * 60 * 1000;
const AVATAR_SWITCH_NUDGE_THRESHOLD: usize = 8;
const AVATAR_SWITCH_NUDGE_COOLDOWN_MS: u64 = 20 * 60 * 1000;
const AVATAR_BACKLOG_NUDGE_COOLDOWN_MS: u64 = 90 * 60 * 1000;
const AVATAR_BACKLOG_NUDGE_MIN_AGE_SECS: i64 = 30 * 60;
const AVATAR_NUDGE_SWITCH_COMPANION: &str = "__avatar_nudge_switch_companion__";
const AVATAR_NUDGE_SWITCH_ASSISTANT: &str = "__avatar_nudge_switch_assistant__";
const AVATAR_NUDGE_SWITCH_COACH: &str = "__avatar_nudge_switch_coach__";

#[derive(Default)]
struct AvatarNudgeRuntime {
    recent_switches_ms: VecDeque<u64>,
    last_switch_nudge_at_ms: u64,
    last_backlog_nudge_at_ms: u64,
}

fn avatar_switch_nudge_message_key(persona: &str) -> &'static str {
    match persona.trim() {
        "companion" => AVATAR_NUDGE_SWITCH_COMPANION,
        "coach" => AVATAR_NUDGE_SWITCH_COACH,
        _ => AVATAR_NUDGE_SWITCH_ASSISTANT,
    }
}

fn avatar_backlog_nudge_message_key(persona: &str, count: usize) -> String {
    format!("__avatar_backlog_nudge__:{}:{}", persona.trim(), count)
}

fn record_avatar_window_switch(runtime: &mut AvatarNudgeRuntime, now_ms: u64) -> bool {
    runtime.recent_switches_ms.push_back(now_ms);

    while runtime
        .recent_switches_ms
        .front()
        .is_some_and(|timestamp| now_ms.saturating_sub(*timestamp) > AVATAR_SWITCH_NUDGE_WINDOW_MS)
    {
        runtime.recent_switches_ms.pop_front();
    }

    if runtime.recent_switches_ms.len() < AVATAR_SWITCH_NUDGE_THRESHOLD {
        return false;
    }

    if runtime.last_switch_nudge_at_ms != 0
        && now_ms.saturating_sub(runtime.last_switch_nudge_at_ms) < AVATAR_SWITCH_NUDGE_COOLDOWN_MS
    {
        return false;
    }

    runtime.last_switch_nudge_at_ms = now_ms;
    true
}

fn count_open_avatar_followups_for_nudge(items: &[AvatarFollowupItem], now_ts: i64) -> usize {
    items
        .iter()
        .filter(|item| item.status == "open")
        .filter(|item| now_ts.saturating_sub(item.created_at) >= AVATAR_BACKLOG_NUDGE_MIN_AGE_SECS)
        .count()
}

fn should_emit_avatar_backlog_nudge(
    runtime: &mut AvatarNudgeRuntime,
    items: &[AvatarFollowupItem],
    now_ts: i64,
    now_ms: u64,
) -> Option<usize> {
    let count = count_open_avatar_followups_for_nudge(items, now_ts);
    if count == 0 {
        return None;
    }

    if runtime.last_backlog_nudge_at_ms != 0
        && now_ms.saturating_sub(runtime.last_backlog_nudge_at_ms)
            < AVATAR_BACKLOG_NUDGE_COOLDOWN_MS
    {
        return None;
    }

    runtime.last_backlog_nudge_at_ms = now_ms;
    Some(count)
}

pub(crate) fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("work-review"))
        .unwrap_or_else(|| PathBuf::from("./data"))
}

fn data_dir_preference_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("work-review").join("data-location.json"))
        .unwrap_or_else(|| PathBuf::from("./work-review-data-location.json"))
}

fn load_data_dir_preference() -> Option<PathBuf> {
    let path = data_dir_preference_path();
    let content = std::fs::read_to_string(path).ok()?;
    let preference: DataDirPreference = serde_json::from_str(&content).ok()?;
    let data_dir = preference.data_dir.trim();
    if data_dir.is_empty() {
        None
    } else {
        Some(PathBuf::from(data_dir))
    }
}

pub(crate) fn save_data_dir_preference(data_dir: &Path) -> std::io::Result<()> {
    let default_dir = default_data_dir();
    let preference_path = data_dir_preference_path();

    if data_dir == default_dir {
        if preference_path.exists() {
            std::fs::remove_file(preference_path)?;
        }
        return Ok(());
    }

    if let Some(parent) = preference_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(&DataDirPreference {
        data_dir: data_dir.to_string_lossy().to_string(),
    })
    .map_err(std::io::Error::other)?;

    std::fs::write(preference_path, content)?;
    Ok(())
}

fn ensure_data_dir(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(path)?;
    Ok(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
}

/// 获取数据目录
fn resolve_data_dir() -> PathBuf {
    let default_dir = default_data_dir();
    let preferred_dir = load_data_dir_preference().unwrap_or_else(|| default_dir.clone());

    match ensure_data_dir(&preferred_dir) {
        Ok(dir) => {
            migrate_legacy_data_dir(&dir);
            dir
        }
        Err(error) => {
            log::warn!("创建数据目录失败，回退默认目录: {error}");

            if preferred_dir != default_dir {
                if let Ok(dir) = ensure_data_dir(&default_dir) {
                    migrate_legacy_data_dir(&dir);
                    let _ = save_data_dir_preference(&dir);
                    return dir;
                }
            }

            let fallback_dir = PathBuf::from("./data");
            if let Err(fallback_error) = std::fs::create_dir_all(&fallback_dir) {
                log::warn!("创建兜底数据目录失败: {fallback_error}");
            }
            migrate_legacy_data_dir(&fallback_dir);
            fallback_dir
        }
    }
}

fn migrate_legacy_data_dir(target_dir: &PathBuf) {
    let legacy_dir = match std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("data")))
    {
        Some(path) => path,
        None => return,
    };

    if legacy_dir == *target_dir || !legacy_dir.exists() {
        return;
    }

    let target_has_data = target_dir.join("config.json").exists()
        || target_dir.join("workreview.db").exists()
        || target_dir.join("screenshots").exists();
    if target_has_data {
        return;
    }

    if let Err(error) = copy_dir_contents(&legacy_dir, target_dir, false) {
        log::warn!("迁移旧版数据目录失败: {error}");
    } else {
        log::info!("已将旧版数据目录迁移到稳定目录: {target_dir:?}");
    }
}

pub(crate) fn copy_dir_contents(
    from: &Path,
    to: &Path,
    overwrite_existing: bool,
) -> Result<u64, std::io::Error> {
    std::fs::create_dir_all(to)?;
    let mut copied_files = 0;

    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = to.join(entry.file_name());

        if source_path.is_dir() {
            copied_files += copy_dir_contents(&source_path, &target_path, overwrite_existing)?;
            continue;
        }

        if overwrite_existing || !target_path.exists() {
            std::fs::copy(&source_path, &target_path)?;
            copied_files += 1;
        }
    }

    Ok(copied_files)
}

/// 浏览器 URL 采集偶发失败时，尝试从最近同窗口标题的活动里恢复 URL。
/// 这是近似统计兜底：优先减少同一页面被切碎成多段或掉成 0 站点 0 页面。
fn recover_recent_browser_url(
    database: &Database,
    app_name: &str,
    window_title: &str,
    now_ts: i64,
    max_age_secs: i64,
) -> Option<String> {
    if !monitor::is_browser_app(app_name) || window_title.is_empty() {
        return None;
    }

    database
        .get_latest_activity_by_app_title(app_name, window_title)
        .ok()
        .flatten()
        .and_then(|activity| {
            let age = now_ts - activity.timestamp;
            if age <= max_age_secs {
                activity.browser_url.filter(|url| !url.is_empty())
            } else {
                None
            }
        })
}

pub(crate) fn resolve_activity_classification(
    config: &AppConfig,
    app_name: &str,
    window_title: &str,
    browser_url: Option<&str>,
) -> activity_classifier::ActivityClassification {
    let mut base_category = monitor::categorize_app_with_rules(
        &config.app_category_rules,
        app_name,
        window_title,
        &config.custom_categories,
    );
    // 分类被删除时回退到 "other"
    if base_category != "other"
        && !config
            .custom_categories
            .iter()
            .any(|c| c.key == base_category)
    {
        base_category = "other".to_string();
    }

    // ── 内置域名知识库:浏览器活动按站点内容细分基础分类 ──
    // 此前浏览器活动一律归 "browser",B 站和 GitHub 在统计里没有区别。
    // 用户显式规则仍优先:命中 app_category_rules 时 base 已不是 "browser",不会走到这里。
    let domain_knowledge =
        browser_url.and_then(work_review_core::knowledge::builtin_domain_category);
    if base_category == "browser" {
        if let Some((kb_base, _)) = domain_knowledge {
            if kb_base != "browser" && config.custom_categories.iter().any(|c| c.key == kb_base) {
                base_category = kb_base.to_string();
            }
        }
    }

    // ── AI 实体分类缓存:后台任务学习到的未知应用/域名归类(见 entity_classify_task) ──
    let mut cached_semantic: Option<String> = None;
    let cache_lookup = match base_category.as_str() {
        "other" => Some(format!("app:{}", app_name.trim().to_lowercase())),
        "browser" if domain_knowledge.is_none() => browser_url
            .map(work_review_core::config::PrivacyConfig::extract_domain)
            .map(|d| d.split(':').next().unwrap_or("").to_string())
            .filter(|d| !d.is_empty())
            .map(|d| format!("domain:{d}")),
        _ => None,
    };
    if let Some(cache_key) = cache_lookup {
        let cached = entity_category_cache()
            .read()
            .ok()
            .and_then(|m| m.get(&cache_key).cloned());
        if let Some((cached_base, semantic)) = cached {
            if config
                .custom_categories
                .iter()
                .any(|c| c.key == cached_base)
            {
                base_category = cached_base;
                cached_semantic = Some(semantic);
            }
        }
    }

    let mut classification = activity_classifier::classify_activity_with_base_category(
        app_name,
        window_title,
        browser_url,
        &base_category,
    );

    // 语义分类被删除时回退到 "未知活动"
    if classification.semantic_category != "未知活动"
        && !config
            .custom_semantic_categories
            .iter()
            .any(|c| c.key == classification.semantic_category)
    {
        classification.semantic_category = "未知活动".to_string();
    }

    // 知识库/AI 缓存的语义提示:仅在打分器置信度不高时兜底;
    // 用户 website_semantic_rules 在下方应用,仍拥有最终覆盖权
    let semantic_hint =
        cached_semantic.or_else(|| domain_knowledge.map(|(_, semantic)| semantic.to_string()));
    if let Some(hint) = semantic_hint {
        if classification.confidence < 70
            && config
                .custom_semantic_categories
                .iter()
                .any(|c| c.key == hint)
        {
            classification.semantic_category = hint;
            classification.confidence = 70;
        }
    }

    if let Some(semantic_category) =
        monitor::find_website_semantic_override(&config.website_semantic_rules, browser_url)
    {
        classification.base_category = monitor::semantic_category_to_base_category(
            &semantic_category,
            &classification.base_category,
        );
        classification.semantic_category = semantic_category.clone();
        classification.confidence = classification.confidence.max(100);
        classification
            .evidence
            .push(format!("命中网站语义规则: {semantic_category}"));
    }

    classification
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordingLoopDecision {
    should_continue: bool,
    screenshot_interval: u64,
    reset_capture_clock: bool,
}

const ACTIVITY_INPUT_IDLE_HARD_STOP_MINUTES: u64 = 10;
const ACTIVITY_INPUT_IDLE_HARD_STOP_SECS: u64 = ACTIVITY_INPUT_IDLE_HARD_STOP_MINUTES * 60;

fn should_confirm_idle(
    input_idle: bool,
    input_idle_seconds: u64,
    screenshots_enabled: bool,
    _screenshot_confirmed: bool,
) -> bool {
    if !input_idle {
        return false;
    }

    // 长时间无输入时直接切断时长，避免后台程序或动态页面无限续时。
    if input_idle_seconds >= ACTIVITY_INPUT_IDLE_HARD_STOP_SECS {
        return true;
    }

    // 键鼠超时但未到硬超时：不再因"画面无变化"判空闲。
    // 创意类应用（PS/C4D/Blender）、AI 网页阅读、代码思考等场景下，画面可能长时间
    // 几乎不变但用户确实在工作。哈希相似度对这类场景是反信号，故废弃其"确认空闲"作用。
    // 关闭截图时无法看画面，回退到"键鼠超时即空闲"（保留原行为）。
    !screenshots_enabled
}

fn previous_app_backfill_duration(
    app_changed: bool,
    duration_to_record: i64,
    was_input_idle: bool,
    is_confirmed_idle: bool,
) -> i64 {
    if !app_changed || duration_to_record <= 0 || was_input_idle || is_confirmed_idle {
        0
    } else {
        duration_to_record
    }
}

fn should_persist_merge_update(effective_duration: i64) -> bool {
    effective_duration > 0
}

const ACTIVITY_MERGE_GAP_SECS: i64 = 600;

fn should_merge_contiguous_activity(
    app_changed: bool,
    app_name: &str,
    current_timestamp: i64,
    latest_timestamp: i64,
) -> bool {
    !app_changed
        && app_name != "Unknown"
        && current_timestamp >= latest_timestamp
        && current_timestamp - latest_timestamp <= ACTIVITY_MERGE_GAP_SECS
}

fn resolve_previous_activity_to_backfill(
    state: &Arc<Mutex<AppState>>,
    previous_app_name: Option<&str>,
    previous_browser_url: Option<&str>,
    previous_window_title: Option<&str>,
) -> Option<database::Activity> {
    let previous_app_name = previous_app_name?;

    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(previous_url) = previous_browser_url.filter(|url| !url.is_empty()) {
        state_guard
            .database
            .get_latest_activity_by_url(previous_url)
            .ok()
            .flatten()
    } else if monitor::is_browser_app(previous_app_name) {
        previous_window_title
            .filter(|title| !title.is_empty())
            .and_then(|title| {
                state_guard
                    .database
                    .get_latest_activity_by_app_title(previous_app_name, title)
                    .ok()
                    .flatten()
            })
            .or_else(|| {
                state_guard
                    .database
                    .get_latest_activity_by_app(previous_app_name)
                    .ok()
                    .flatten()
            })
    } else {
        state_guard
            .database
            .get_latest_activity_by_app(previous_app_name)
            .ok()
            .flatten()
    }
}

#[allow(clippy::too_many_arguments)]
fn persist_previous_activity_backfill(
    database: &Database,
    config: &AppConfig,
    privacy_filter: &PrivacyFilter,
    previous_activity: Option<&database::Activity>,
    previous_app_name: Option<&str>,
    previous_window_title: Option<&str>,
    previous_browser_url: Option<&str>,
    duration_delta: i64,
    current_timestamp: i64,
    current_app_name: &str,
) -> Option<i64> {
    if duration_delta <= 0 {
        return None;
    }

    if let Some(previous_activity) = previous_activity {
        let previous_id = previous_activity.id?;

        let _ = database.merge_activity(
            previous_id,
            duration_delta,
            None,
            &previous_activity.screenshot_path,
            current_timestamp,
            None,
        );
        log::debug!(
            "⏱️ 时长回补: {} +{}s (切换到 {})",
            previous_activity.app_name,
            duration_delta,
            current_app_name
        );
        return Some(previous_id);
    }

    let previous_app_name = previous_app_name?.trim();
    if previous_app_name.is_empty() {
        return None;
    }

    let previous_window_title = previous_window_title.unwrap_or("");
    let previous_browser_url = previous_browser_url.filter(|url| !url.is_empty());
    let privacy_action = privacy_filter.check_privacy_full(
        previous_app_name,
        previous_window_title,
        previous_browser_url,
    );
    if privacy_action == privacy::PrivacyAction::Skip {
        log::debug!("上一应用回补跳过(隐私): {previous_app_name}");
        return None;
    }

    let classification = crate::resolve_activity_classification(
        config,
        previous_app_name,
        previous_window_title,
        previous_browser_url,
    );
    let (window_title, browser_url) = if privacy_action == privacy::PrivacyAction::Anonymize {
        ("[内容已脱敏]".to_string(), None)
    } else {
        (
            previous_window_title.to_string(),
            previous_browser_url.map(ToString::to_string),
        )
    };

    let activity = database::Activity {
        id: None,
        timestamp: current_timestamp,
        app_name: previous_app_name.to_string(),
        window_title,
        screenshot_path: String::new(),
        ocr_text: None,
        category: classification.base_category,
        duration: duration_delta,
        browser_url,
        executable_path: None,
        semantic_category: Some(classification.semantic_category),
        semantic_confidence: Some(i32::from(classification.confidence)),
        screenshot_url: None,
    };

    match database.insert_activity(&activity) {
        Ok(activity_id) => {
            log::debug!(
                "⏱️ 新建上一应用回补: {previous_app_name} +{duration_delta}s (id={activity_id}, 切换到 {current_app_name})"
            );
            Some(activity_id)
        }
        Err(e) => {
            log::error!("上一应用回补记录失败: {e}");
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn backfill_previous_activity_if_needed(
    state: &Arc<Mutex<AppState>>,
    previous_activity: Option<&database::Activity>,
    previous_app_name: Option<&str>,
    previous_window_title: Option<&str>,
    previous_browser_url: Option<&str>,
    duration_delta: i64,
    current_timestamp: i64,
    current_app_name: &str,
) {
    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
    let _ = persist_previous_activity_backfill(
        &state_guard.database,
        &state_guard.config,
        &state_guard.privacy_filter,
        previous_activity,
        previous_app_name,
        previous_window_title,
        previous_browser_url,
        duration_delta,
        current_timestamp,
        current_app_name,
    );
}

fn recording_loop_decision(
    is_recording: bool,
    is_paused: bool,
    screenshot_interval: u64,
) -> RecordingLoopDecision {
    if !is_recording || is_paused {
        RecordingLoopDecision {
            should_continue: false,
            screenshot_interval: 1,
            reset_capture_clock: true,
        }
    } else {
        RecordingLoopDecision {
            should_continue: true,
            screenshot_interval,
            reset_capture_clock: false,
        }
    }
}

fn monitoring_poll_interval_ms_for_platform(is_macos: bool) -> u64 {
    if is_macos {
        1500
    } else {
        500
    }
}

fn monitoring_poll_interval_ms() -> u64 {
    monitoring_poll_interval_ms_for_platform(cfg!(target_os = "macos"))
}

const ACTIVE_WINDOW_CACHE_MAX_AGE_MS: u64 = 1250;
const MIN_CAPTURE_INTERVAL_MS: u128 = 3000;
const MIN_BROWSER_CHANGE_CAPTURE_INTERVAL_MS: u128 = 1200;

fn reusable_cached_active_window(
    cached: Option<&(std::time::Instant, monitor::ActiveWindow)>,
    now: std::time::Instant,
) -> Option<monitor::ActiveWindow> {
    let (sampled_at, active_window) = cached?;
    let age = now.checked_duration_since(*sampled_at)?;

    if age > Duration::from_millis(ACTIVE_WINDOW_CACHE_MAX_AGE_MS) {
        return None;
    }

    Some(active_window.clone())
}

fn should_probe_browser_url_before_change_detection(
    app_name: &str,
    window_title: &str,
    last_app_name: Option<&str>,
    last_window_title: Option<&str>,
    current_browser_url: Option<&str>,
) -> bool {
    if !monitor::is_browser_app(app_name) || window_title.is_empty() {
        return false;
    }
    // 首次遇到浏览器窗口时（last 为 None），也需要探测 URL
    if last_app_name.is_none() || last_window_title.is_none() {
        return current_browser_url.is_none();
    }
    // 同窗口持续使用时，探测 URL 变化
    last_app_name == Some(app_name) && last_window_title == Some(window_title)
}

fn claim_browser_url_probe(browser_url_probe_attempted: &mut bool, eligible: bool) -> bool {
    if *browser_url_probe_attempted || !eligible {
        return false;
    }

    // 查询开始即消耗本轮预算。即使 URL 获取失败，也不能立即重复扫描 UIA。
    *browser_url_probe_attempted = true;
    true
}

fn resolve_browser_url_once<F>(
    browser_url_probe_attempted: &mut bool,
    eligible: bool,
    resolve: F,
) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    claim_browser_url_probe(browser_url_probe_attempted, eligible).then(resolve)?
}

fn browser_url_probe_attempted_after_full_lookup(
    used_full_window_lookup: bool,
    is_browser: bool,
) -> bool {
    used_full_window_lookup && is_browser
}

fn browser_change_capture_min_interval_ms(
    app_name: &str,
    title_changed: bool,
    url_changed: bool,
) -> u128 {
    if monitor::is_browser_app(app_name) && (title_changed || url_changed) {
        MIN_BROWSER_CHANGE_CAPTURE_INTERVAL_MS
    } else {
        MIN_CAPTURE_INTERVAL_MS
    }
}

fn should_refresh_browser_url_before_record(app_name: &str, window_title: &str) -> bool {
    monitor::is_browser_app(app_name) && !window_title.is_empty()
}

fn avatar_monitor_poll_interval_ms_for_platform(is_macos: bool, active: bool) -> u64 {
    if is_macos {
        if active {
            750
        } else {
            2000
        }
    } else if active {
        180
    } else {
        750
    }
}

#[allow(dead_code)]
fn avatar_monitor_poll_interval_ms() -> u64 {
    avatar_monitor_poll_interval_ms_for_platform(cfg!(target_os = "macos"), true)
}

fn screen_lock_check_interval_ms_for_platform(is_macos: bool) -> u64 {
    if is_macos {
        5000
    } else {
        1000
    }
}

fn screen_lock_check_interval_ms() -> u64 {
    screen_lock_check_interval_ms_for_platform(cfg!(target_os = "macos"))
}

#[derive(Debug, Clone, PartialEq)]
struct AvatarActivityDecision {
    should_continue: bool,
    reset_state: Option<avatar_engine::AvatarStatePayload>,
}

fn avatar_activity_decision(
    avatar_enabled: bool,
    is_recording: bool,
    is_paused: bool,
    avatar_opacity: f64,
    avatar_preset: &str,
    avatar_persona: &str,
    avatar_body_hidden: bool,
) -> AvatarActivityDecision {
    if !avatar_enabled {
        return AvatarActivityDecision {
            should_continue: false,
            reset_state: Some(avatar_engine::apply_avatar_visual_settings(
                avatar_engine::default_avatar_state(),
                avatar_opacity,
                avatar_preset,
                avatar_persona,
                avatar_body_hidden,
            )),
        };
    }

    if !is_recording || is_paused {
        return AvatarActivityDecision {
            should_continue: false,
            reset_state: Some(avatar_engine::apply_avatar_visual_settings(
                avatar_engine::default_avatar_state(),
                avatar_opacity,
                avatar_preset,
                avatar_persona,
                avatar_body_hidden,
            )),
        };
    }

    AvatarActivityDecision {
        should_continue: true,
        reset_state: None,
    }
}

fn avatar_proactive_ai_should_run(
    avatar_enabled: bool,
    avatar_proactive_ai_enabled: bool,
    is_paused: bool,
    text_model: &work_review_core::config::ModelConfig,
    now_ms: u64,
    next_check_ms: u64,
) -> bool {
    avatar_enabled
        && avatar_proactive_ai_enabled
        && !is_paused
        && !text_model.endpoint.trim().is_empty()
        && !text_model.model.trim().is_empty()
        && now_ms >= next_check_ms
}

#[derive(Debug, Clone, PartialEq)]
struct AvatarTransitionDecision {
    emit_state: Option<avatar_engine::AvatarStatePayload>,
    pending_state: Option<avatar_engine::AvatarStatePayload>,
    pending_hits: u8,
}

fn avatar_transition_decision(
    current: Option<&avatar_engine::AvatarStatePayload>,
    pending: Option<&avatar_engine::AvatarStatePayload>,
    pending_hits: u8,
    candidate: &avatar_engine::AvatarStatePayload,
) -> AvatarTransitionDecision {
    const AVATAR_MODE_STABILITY_THRESHOLD: u8 = 2;

    match current {
        None => AvatarTransitionDecision {
            emit_state: Some(candidate.clone()),
            pending_state: None,
            pending_hits: 0,
        },
        Some(current_state) if current_state == candidate => AvatarTransitionDecision {
            emit_state: None,
            pending_state: None,
            pending_hits: 0,
        },
        Some(current_state) if current_state.mode == candidate.mode => AvatarTransitionDecision {
            emit_state: Some(candidate.clone()),
            pending_state: None,
            pending_hits: 0,
        },
        Some(_) => {
            let next_hits = if pending == Some(candidate) {
                pending_hits.saturating_add(1)
            } else {
                1
            };

            if next_hits >= AVATAR_MODE_STABILITY_THRESHOLD {
                AvatarTransitionDecision {
                    emit_state: Some(candidate.clone()),
                    pending_state: None,
                    pending_hits: 0,
                }
            } else {
                AvatarTransitionDecision {
                    emit_state: None,
                    pending_state: Some(candidate.clone()),
                    pending_hits: next_hits,
                }
            }
        }
    }
}

fn should_skip_transient_window(active_window: &monitor::ActiveWindow) -> bool {
    let app_lower = active_window.app_name.to_lowercase();
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

fn should_skip_system_window(active_window: &monitor::ActiveWindow) -> bool {
    let is_sys = monitor::is_system_process(&active_window.app_name);
    let is_minimized_window = active_window.is_minimized;
    let is_explorer_shell = {
        let name_lower = active_window.app_name.to_lowercase();
        let name_trimmed = name_lower.trim_end_matches(".exe");
        (name_trimmed == "explorer" || name_trimmed == "file explorer")
            && active_window.window_title.is_empty()
    };
    // Windows 在 UAC / 任务管理器异常时，进程名可能退化成标题或受保护进程名，
    // 需要结合标题与可执行路径一起兜底过滤。
    let is_windows_system_dialog = is_windows_system_dialog(active_window);

    is_sys || is_minimized_window || is_explorer_shell || is_windows_system_dialog
}

async fn background_avatar_task(state: Arc<Mutex<AppState>>, app: AppHandle) {
    let mut last_avatar_state: Option<avatar_engine::AvatarStatePayload> = None;
    let mut pending_avatar_state: Option<avatar_engine::AvatarStatePayload> = None;
    let mut pending_avatar_hits: u8 = 0;
    let mut last_window_signature: Option<String> = None;
    let mut break_reminder_runtime = BreakReminderRuntime::new();
    let mut avatar_nudge_runtime = AvatarNudgeRuntime::default();
    // 频繁切换检测运行时
    let mut last_switch_signature: Option<String> = None;
    let mut recent_switches_ms: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
    let mut last_focus_nudge_ms: u64 = 0;
    // 工作目标庆祝：防止同一天重复庆祝
    let mut goal_celebrated_date: String = String::new();
    let mut goal_check_counter: u32 = 0;
    let mut cached_rules: Vec<work_review_core::config::AppCategoryRule> = Vec::new();
    let mut cached_custom_categories: Vec<work_review_core::config::CustomCategory> = Vec::new();
    let mut cached_rules_signature: u64 = 0;
    const IDLE_TIMEOUT_MINUTES: u64 = 3;
    let idle_detector = idle_detector::IdleDetector::new(IDLE_TIMEOUT_MINUTES);
    // 桌宠模型生成提醒运行时状态
    let task_start_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
    let mut next_proactive_check_ms: u64 = task_start_ms + 10 * 60_000; // 启动后 10 分钟首次
    let mut proactive_mood: Option<(String, u64)> = None; // (mode, expires_ms)
    let mut active_app_since_ms: u64 = task_start_ms;
    let mut last_proactive_app: Option<String> = None;

    loop {
        let (
            avatar_enabled,
            avatar_proactive_ai_enabled,
            avatar_generating_report,
            avatar_opacity,
            avatar_preset,
            avatar_persona,
            avatar_body_hidden,
            is_recording,
            is_paused,
            break_reminder_enabled,
            break_reminder_interval_minutes,
            assistant_timeout_secs,
        ) = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state_guard.config.avatar_enabled,
                state_guard.config.avatar_proactive_ai_enabled,
                state_guard.avatar_generating_report,
                state_guard.config.avatar_opacity,
                state_guard.config.avatar_preset.clone(),
                state_guard.config.avatar_persona.clone(),
                state_guard.config.avatar_body_hidden,
                state_guard.is_recording,
                state_guard.is_paused,
                state_guard.config.break_reminder_enabled,
                state_guard.config.break_reminder_interval_minutes,
                state_guard.config.assistant_timeout_secs,
            )
        };

        let text_model = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            state_guard.config.text_model.clone()
        };

        let activity_decision = avatar_activity_decision(
            avatar_enabled,
            is_recording,
            is_paused,
            avatar_opacity,
            &avatar_preset,
            &avatar_persona,
            avatar_body_hidden,
        );
        let poll_interval_ms = avatar_monitor_poll_interval_ms_for_platform(
            cfg!(target_os = "macos"),
            activity_decision.should_continue,
        );
        tokio::time::sleep(Duration::from_millis(poll_interval_ms)).await;

        if !activity_decision.should_continue {
            let reminder_result = advance_break_reminder(
                &mut break_reminder_runtime,
                false,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(0),
            );
            if let Some(payload) = reminder_result.payload.as_ref() {
                avatar_engine::emit_avatar_bubble(&app, payload);
            }

            pending_avatar_state = None;
            pending_avatar_hits = 0;
            last_window_signature = None;
            avatar_nudge_runtime.recent_switches_ms.clear();

            if let Some(reset_state) = activity_decision.reset_state {
                let should_emit_reset = last_avatar_state.as_ref() != Some(&reset_state);
                {
                    let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    state_guard.avatar_state = reset_state.clone();
                }

                if avatar_enabled && should_emit_reset {
                    avatar_engine::emit_avatar_state(&app, &reset_state);
                }

                last_avatar_state = Some(reset_state);
            } else {
                last_avatar_state = None;
            }
            continue;
        }

        let sampled_at = std::time::Instant::now();
        let active_window = match monitor::get_active_window_fast() {
            Ok(window) => window,
            Err(_) => continue,
        };

        {
            let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            state_guard.cached_active_window = Some((sampled_at, active_window.clone()));
            let rules = &state_guard.config.app_category_rules;
            let cats = &state_guard.config.custom_categories;
            let sig = rules.len() as u64
                | rules.last().map_or(0u64, |r| {
                    let mut h: u64 = 0;
                    for b in r.app_name.as_bytes() {
                        h = h.wrapping_add(*b as u64);
                    }
                    for b in r.category.as_bytes() {
                        h = h.wrapping_mul(31).wrapping_add(*b as u64);
                    }
                    h
                });
            if sig != cached_rules_signature {
                cached_rules = rules.clone();
                cached_custom_categories = cats.clone();
                cached_rules_signature = sig;
            }
        };
        let app_category_rules = &cached_rules;
        let app_custom_categories = &cached_custom_categories;

        if should_skip_transient_window(&active_window) || should_skip_system_window(&active_window)
        {
            continue;
        }

        let input_idle = idle_detector.is_input_idle();
        // 工作目标庆祝：每 ~60 轮检查一次（约 1-2 分钟）
        goal_check_counter = goal_check_counter.wrapping_add(1);
        if goal_check_counter.is_multiple_of(60) && avatar_enabled {
            let (goal_minutes, goal_notify) = {
                let s = state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    s.config.daily_work_goal_minutes,
                    s.config.goal_notifications,
                )
            };
            if goal_notify {
                if let Some(goal) = goal_minutes {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    if today != goal_celebrated_date {
                        let work_secs = {
                            let s = state.lock().unwrap_or_else(|e| e.into_inner());
                            let segments = s.config.effective_work_segments();
                            s.database
                                .get_daily_stats_with_segments(&today, &segments)
                                .map(|st| st.work_time_duration)
                                .unwrap_or(0)
                        };
                        if work_secs >= (goal as i64 * 60) {
                            avatar_engine::emit_avatar_bubble(
                                &app,
                                &avatar_engine::AvatarBubblePayload::success(
                                    "🎉 今日工作目标达成！继续保持！",
                                ),
                            );
                            goal_celebrated_date = today;
                        }
                    }
                }
            }
        }

        let reminder_result = if !(avatar_enabled && break_reminder_enabled) {
            advance_break_reminder(
                &mut break_reminder_runtime,
                false,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(0),
            )
        } else if break_reminder_runtime.phase == BreakReminderPhase::Cooldown {
            advance_break_reminder(
                &mut break_reminder_runtime,
                true,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(poll_interval_ms),
            )
        } else if input_idle {
            break_reminder_runtime.reset_active_cycle();
            BreakReminderAdvanceResult::default()
        } else {
            advance_break_reminder(
                &mut break_reminder_runtime,
                true,
                break_reminder_interval_minutes,
                BreakReminderSignal::TickMillis(poll_interval_ms),
            )
        };
        if let Some(payload) = reminder_result.payload.as_ref() {
            avatar_engine::emit_avatar_bubble(&app, payload);
        }

        let avatar_state = avatar_engine::apply_avatar_visual_settings(
            avatar_engine::derive_avatar_state_with_rules(
                app_category_rules,
                app_custom_categories,
                &active_window.app_name,
                &active_window.window_title,
                active_window.browser_url.as_deref(),
                input_idle,
                avatar_generating_report,
            ),
            avatar_opacity,
            &avatar_preset,
            &avatar_persona,
            avatar_body_hidden,
        );

        let window_signature = format!(
            "{}|{}|{}",
            active_window.app_name,
            active_window.window_title,
            active_window.browser_url.as_deref().unwrap_or_default()
        );
        let window_changed = last_window_signature.as_deref() != Some(window_signature.as_str());
        if last_proactive_app.as_deref() != Some(active_window.app_name.as_str()) {
            active_app_since_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
            last_proactive_app = Some(active_window.app_name.clone());
        }

        // 方向 2：频繁切换检测 —— 5 分钟内切换 ≥6 次 → 桌宠提醒专注（30 分钟冷却）
        let actually_switched = last_switch_signature.as_deref() != Some(window_signature.as_str());
        last_switch_signature = Some(window_signature.clone());
        if actually_switched && is_recording && !is_paused {
            let switch_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
            recent_switches_ms.push_back(switch_ms);
            while let Some(&old) = recent_switches_ms.front() {
                if switch_ms.saturating_sub(old) > 5 * 60 * 1000 {
                    recent_switches_ms.pop_front();
                } else {
                    break;
                }
            }
            if recent_switches_ms.len() >= 6
                && switch_ms.saturating_sub(last_focus_nudge_ms) > 30 * 60 * 1000
            {
                avatar_engine::emit_avatar_bubble(
                    &app,
                    &avatar_engine::AvatarBubblePayload::info(
                        "检测到频繁切换应用，要试试专注一段时间吗？",
                    ),
                );
                last_focus_nudge_ms = switch_ms;
                recent_switches_ms.clear();
            }
        }

        let transition_decision = avatar_transition_decision(
            last_avatar_state.as_ref(),
            pending_avatar_state.as_ref(),
            pending_avatar_hits,
            &avatar_state,
        );

        pending_avatar_state = transition_decision.pending_state;
        pending_avatar_hits = transition_decision.pending_hits;

        if let Some(mut next_avatar_state) = transition_decision.emit_state {
            // 桌宠模型生成提醒的情绪覆盖：AI 给的表情未过期则覆盖 mode
            let mood_now_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
            match &proactive_mood {
                Some((mood_mode, expires_ms)) if mood_now_ms < *expires_ms => {
                    next_avatar_state.mode = mood_mode.clone();
                }
                Some(_) => {
                    proactive_mood = None; // 过期清空
                }
                None => {}
            }
            let collect_cost_ms = sampled_at.elapsed().as_millis();
            let previous_mode = last_avatar_state
                .as_ref()
                .map(|state| state.mode.as_str())
                .unwrap_or("none");

            {
                let mut state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                state_guard.avatar_state = next_avatar_state.clone();
            }

            avatar_engine::emit_avatar_state(&app, &next_avatar_state);

            let entered_idle = match &last_avatar_state {
                Some(previous) => !previous.is_idle && next_avatar_state.is_idle,
                None => next_avatar_state.is_idle,
            };

            if entered_idle {
                avatar_engine::emit_avatar_bubble(
                    &app,
                    &avatar_engine::AvatarBubblePayload::info("先放松一下，待会再继续推进。"),
                );
            }

            log::info!(
                "🐾 桌宠状态切换: {} -> {} | 窗口={} | 采集耗时={}ms",
                previous_mode,
                next_avatar_state.mode,
                window_signature,
                collect_cost_ms
            );

            last_avatar_state = Some(next_avatar_state);
            last_window_signature = Some(window_signature);
        } else if window_changed {
            log::debug!(
                "🐾 桌宠检测到前台切换，但状态未变: {} | 采集耗时={}ms",
                window_signature,
                sampled_at.elapsed().as_millis()
            );
            last_window_signature = Some(window_signature);
        }

        if avatar_enabled && window_changed {
            let now = chrono::Local::now();
            let now_ts = now.timestamp();
            let now_ms = now.timestamp_millis().max(0) as u64;
            let switch_nudge_ready = record_avatar_window_switch(&mut avatar_nudge_runtime, now_ms);
            let date_from = (now - chrono::Duration::days(3))
                .format("%Y-%m-%d")
                .to_string();
            let followup_result = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                let activities = crate::commands::load_filtered_activities_in_range(
                    &state_guard,
                    Some(date_from.as_str()),
                    None,
                    480,
                );
                (
                    activities,
                    state_guard.config.avatar_persona.clone(),
                    state_guard.config.avatar_followups.clone(),
                )
            };

            if let (Ok(activities), persona, manual_followups) = followup_result {
                let mut emitted_followup = false;
                if let Some(payload) = crate::avatar_followup::find_followup_suggestion(
                    &activities,
                    &active_window,
                    &persona,
                    &manual_followups,
                    now_ts,
                    now_ms,
                ) {
                    if crate::avatar_followup::should_emit_followup(&payload.project_key, now_ms) {
                        crate::avatar_followup::emit_followup_suggestion(&app, &payload);
                        crate::avatar_followup::note_followup_emitted(&payload.project_key, now_ms);
                        emitted_followup = true;
                    }
                }

                if !emitted_followup && switch_nudge_ready {
                    avatar_engine::emit_avatar_bubble(
                        &app,
                        &avatar_engine::AvatarBubblePayload::info(avatar_switch_nudge_message_key(
                            &persona,
                        )),
                    );
                } else if !emitted_followup {
                    if let Some(count) = should_emit_avatar_backlog_nudge(
                        &mut avatar_nudge_runtime,
                        &manual_followups,
                        now_ts,
                        now_ms,
                    ) {
                        avatar_engine::emit_avatar_bubble(
                            &app,
                            &avatar_engine::AvatarBubblePayload::info(
                                avatar_backlog_nudge_message_key(&persona, count),
                            ),
                        );
                    }
                }
            }
        }

        // 桌宠模型生成提醒：到点才调一次文本模型，模型自主决定是否提示和提示内容
        let proactive_now_ms = chrono::Local::now().timestamp_millis().max(0) as u64;
        if avatar_proactive_ai_should_run(
            avatar_enabled,
            avatar_proactive_ai_enabled,
            is_paused,
            &text_model,
            proactive_now_ms,
            next_proactive_check_ms,
        ) {
            let active_minutes = proactive_now_ms.saturating_sub(active_app_since_ms) / 60_000;
            let recent_switches = recent_switches_ms.len() as u32;
            let (work_seconds_today, hour, minute) = {
                let now = chrono::Local::now();
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                let today = now.format("%Y-%m-%d").to_string();
                let segments = state_guard.config.effective_work_segments();
                let work_secs = state_guard
                    .database
                    .get_daily_stats_with_segments(&today, &segments)
                    .map(|st| st.work_time_duration.max(0) as u64)
                    .unwrap_or(0);
                let h = now.format("%H").to_string().parse::<u32>().unwrap_or(0);
                let m = now.format("%M").to_string().parse::<u32>().unwrap_or(0);
                (work_secs, h, m)
            };
            let context = avatar_proactive::ProactiveContext {
                app_name: active_window.app_name.clone(),
                active_minutes,
                work_seconds_today,
                recent_switches,
                is_idle: input_idle,
                hour,
                minute,
            };
            let outcome = avatar_proactive::decide_and_speak(
                &app,
                &text_model,
                &avatar_persona,
                "zh-CN",
                &context,
                assistant_timeout_secs,
            )
            .await;
            if let Some((mode, expires)) = outcome.mood {
                proactive_mood = Some((mode, expires));
            }
            next_proactive_check_ms = outcome.next_check_ms;
        }
    }
}

// 系统托盘在 setup 钩子中使用 TrayIconBuilder 创建 (Tauri v2)

/// 后台截屏任务
/// 使用 Arc<Mutex<AppState>> 而非 tauri::State，因为 State 无法在 async move 块中手动构造
async fn background_screenshot_task(state: Arc<Mutex<AppState>>, app: AppHandle) {
    // ===== 状态变量 =====
    let mut last_app_name: Option<String> = None;
    let mut last_app_window_title: Option<String> = None;
    let mut last_browser_url: Option<String> = None;

    let mut last_capture_time = std::time::Instant::now();

    // ===== 空闲检测器 =====
    // 先以输入空闲进入”疑似空闲”，再结合前台变化做短时保留。
    // 使用用户配置的空闲阈值，默认 5 分钟
    let idle_threshold_minutes = {
        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.idle_threshold_minutes as u64
    };
    let idle_detector = idle_detector::IdleDetector::new(idle_threshold_minutes.max(1));
    let mut last_idle_log_time = std::time::Instant::now();
    let mut is_currently_idle = false; // 当前是否处于空闲状态

    let poll_interval_ms = monitoring_poll_interval_ms(); // 桌宠状态和窗口切换检测优先更快反馈

    // OCR 并发限制：最多 2 个 OCR 任务同时运行，防止任务堆积消耗内存
    let ocr_semaphore = Arc::new(tokio::sync::Semaphore::new(2));

    // 合并路径的截图哈希去重：用 Arc 共享给异步任务，避免 static 跨活动污染
    let merge_screenshot_hash = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // 锁屏检测器（无内部状态，复用同一实例避免重复分配）
    let screen_lock_monitor = screen_lock::ScreenLockMonitor::new();
    let mut last_screen_lock_check = std::time::Instant::now()
        .checked_sub(Duration::from_millis(screen_lock_check_interval_ms()))
        .unwrap_or_else(std::time::Instant::now);
    let mut cached_screen_locked = false;

    loop {
        // 首先检查录制状态并获取配置
        let decision = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            recording_loop_decision(
                state_guard.is_recording,
                state_guard.is_paused,
                state_guard.config.screenshot_interval,
            )
        };

        if decision.reset_capture_clock {
            last_capture_time = std::time::Instant::now();
        }

        if !decision.should_continue {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }

        if last_screen_lock_check.elapsed()
            >= Duration::from_millis(screen_lock_check_interval_ms())
        {
            cached_screen_locked = screen_lock_monitor.is_locked();
            last_screen_lock_check = std::time::Instant::now();
        }

        // 检测屏幕锁定状态，锁屏时不统计时长
        if cached_screen_locked {
            log::info!("🔒 屏幕已锁定，暂停活动统计");
            last_app_name = None; // 重置应用状态，解锁后视为新开始
            last_capture_time = std::time::Instant::now(); // 重置截图计时，避免解锁后累加锁屏时长
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        let screenshot_interval = decision.screenshot_interval;

        // 轮询检测活动窗口（1秒间隔），让桌宠状态切换更及时
        tokio::time::sleep(Duration::from_millis(poll_interval_ms)).await;

        // 获取当前活动窗口
        // 失败原因：Windows 睡眠/待机/UAC 时无前台窗口、macOS 权限不足等
        // 此时重置计时器，避免累积的时长被错误归属到下一个真实应用
        let active_window_now = std::time::Instant::now();
        let cached_active_window = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            reusable_cached_active_window(
                state_guard.cached_active_window.as_ref(),
                active_window_now,
            )
        };
        let (mut active_window, used_full_window_lookup) =
            if let Some(window) = cached_active_window {
                // 头像循环缓存不含浏览器 URL；浏览器窗口需要走完整采集路径。
                if monitor::is_browser_app(&window.app_name) && window.browser_url.is_none() {
                    match monitor::get_active_window() {
                        Ok(window) => (window, true),
                        Err(_) => (window, false),
                    }
                } else {
                    (window, false)
                }
            } else {
                (
                    match monitor::get_active_window() {
                        Ok(w) => w,
                        Err(_) => {
                            last_capture_time = std::time::Instant::now();
                            continue;
                        }
                    },
                    true,
                )
            };
        let mut browser_url_probe_attempted = browser_url_probe_attempted_after_full_lookup(
            used_full_window_lookup,
            monitor::is_browser_app(&active_window.app_name),
        );

        // 再次检查状态
        let should_capture = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            state_guard.is_recording && !state_guard.is_paused
        };

        if !should_capture {
            continue;
        }

        // macOS 系统进程在用户切换应用、点击 Dock 时会短暂成为前台应用
        // 跳过这些进程避免它们偷走其他应用的使用时长
        // 不更新 last_app_name，时长会在下一个正常轮询中通过 elapsed_secs 自然回收
        {
            if should_skip_transient_window(&active_window) {
                log::debug!("跳过系统瞬态进程: {}", active_window.app_name);
                last_app_name = None;
                last_app_window_title = None;
                last_browser_url = None;
                last_capture_time = std::time::Instant::now();
                continue;
            }
        }

        // 跳过系统 shell / 锁屏 / 桌面进程，避免睡眠/唤醒时累积虚假时长
        // 注意 explorer 特殊处理：有窗口标题时是文件管理器，应该记录
        {
            if should_skip_system_window(&active_window) {
                log::debug!(
                    "跳过系统/桌面窗口: {} (title={}, minimized={})",
                    active_window.app_name,
                    active_window.window_title,
                    active_window.is_minimized
                );
                last_app_name = None;
                last_app_window_title = None;
                last_browser_url = None;
                last_capture_time = std::time::Instant::now();
                continue;
            }
        }

        let should_probe_browser_url = should_probe_browser_url_before_change_detection(
            &active_window.app_name,
            &active_window.window_title,
            last_app_name.as_deref(),
            last_app_window_title.as_deref(),
            active_window.browser_url.as_deref(),
        );
        if let Some(resolved_url) = resolve_browser_url_once(
            &mut browser_url_probe_attempted,
            should_probe_browser_url,
            || {
                monitor::resolve_browser_url_for_window(
                    &active_window.app_name,
                    &active_window.window_title,
                )
            },
        ) {
            if last_browser_url.as_deref() != Some(resolved_url.as_str()) {
                log::debug!(
                    "浏览器 URL 预探测命中: {} | {} -> {}",
                    active_window.app_name,
                    active_window.window_title,
                    resolved_url
                );
            }
            active_window.browser_url = Some(resolved_url);
        }

        // 浏览器 URL 存在瞬时采集失败时，尽量复用同窗口最近一次成功值，减少统计断裂。
        const BROWSER_URL_STICKY_GAP_SECS: i64 = 120;
        if active_window.browser_url.is_none()
            && monitor::is_browser_app(&active_window.app_name)
            && !active_window.window_title.is_empty()
        {
            let now_ts = chrono::Local::now().timestamp();

            let recovered_url = if last_app_name.as_deref() == Some(active_window.app_name.as_str())
                && last_app_window_title.as_deref() == Some(active_window.window_title.as_str())
            {
                last_browser_url.clone()
            } else {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                recover_recent_browser_url(
                    &state_guard.database,
                    &active_window.app_name,
                    &active_window.window_title,
                    now_ts,
                    BROWSER_URL_STICKY_GAP_SECS,
                )
            };

            if let Some(recovered_url) = recovered_url {
                log::debug!(
                    "恢复浏览器 URL: {} | {} -> {}",
                    active_window.app_name,
                    active_window.window_title,
                    recovered_url
                );
                active_window.browser_url = Some(recovered_url);
            }
        }

        // ===== 检测应用切换 =====
        let previous_window_title = last_app_window_title.clone();
        let previous_browser_url = last_browser_url.clone();

        let mut url_changed = match (&last_browser_url, &active_window.browser_url) {
            (Some(l), Some(r)) => l != r,
            (None, None) => false,
            _ => true,
        };

        // 只有当两个标题不同时才算切换
        let title_changed = match (&last_app_window_title, &active_window.window_title) {
            (Some(last_title), active_title) => last_title != active_title,
            (None, _) => true,
        };

        let mut app_changed = match &last_app_name {
            Some(last) => last != &active_window.app_name || url_changed || title_changed,
            None => true,
        };
        let capture_min_interval_ms = browser_change_capture_min_interval_ms(
            &active_window.app_name,
            title_changed,
            url_changed,
        );

        // 计算距离上次截图的时间
        let elapsed_since_capture = last_capture_time.elapsed();
        let elapsed_secs = elapsed_since_capture.as_secs();

        // ===== 应用切换日志 =====
        if app_changed && last_app_name.is_some() {
            log::info!(
                "📊 应用切换: {} [{}] → {} [{}]",
                last_app_name.as_deref().unwrap_or("无"),
                previous_window_title.as_deref().unwrap_or(""),
                active_window.app_name,
                active_window.window_title,
            );
        }

        // ===== 空闲检测第一阶段：键鼠活动检查 =====
        let input_idle_seconds = idle_detector.get_idle_seconds();
        let input_idle = input_idle_seconds >= idle_threshold_minutes * 60;

        let was_input_idle = is_currently_idle;
        // 每 30 秒打印一次空闲状态日志（避免刷屏）；状态切换本身每轮都要处理
        let should_log_idle = last_idle_log_time.elapsed() >= Duration::from_secs(30);
        if input_idle != is_currently_idle {
            if input_idle {
                if should_log_idle {
                    log::info!("⏸️  键鼠超时，等待截图确认空闲状态...");
                }
            } else {
                if should_log_idle {
                    log::info!("▶️  检测到用户活动，恢复正常记录");
                }
                idle_detector.reset();
            }
        }
        if should_log_idle {
            last_idle_log_time = std::time::Instant::now();
        }
        is_currently_idle = input_idle;

        // ===== 判断是否截图 =====
        // 1. 定时触发：到达配置的间隔时间
        // 2. 应用切换触发：满足最小间隔
        let should_take_screenshot = if elapsed_secs >= screenshot_interval {
            log::debug!("定时截图触发");
            true
        } else if app_changed && elapsed_since_capture.as_millis() >= capture_min_interval_ms {
            if capture_min_interval_ms < MIN_CAPTURE_INTERVAL_MS {
                log::debug!("浏览器导航截图触发");
            } else {
                log::debug!("应用切换截图触发");
            }
            true
        } else {
            false
        };

        // 保存 app_name 副本供浮动窗口检测使用（在 move 之前）
        let frontmost_app_name = active_window.app_name.clone();

        if !should_take_screenshot {
            // 如果是因为冷却时间未到而没有截图，但应用/标签页实际上已经变化了
            // 那么我们不要更新 last_* 变量，这样下一个轮询周期 app_changed 仍然为 true
            if !app_changed {
                last_app_name = Some(active_window.app_name.clone());
                last_app_window_title = Some(active_window.window_title.clone());
                last_browser_url = active_window.browser_url.clone();
            }
            continue;
        }

        let should_refresh_browser_url = should_refresh_browser_url_before_record(
            &active_window.app_name,
            &active_window.window_title,
        );
        if let Some(resolved_url) = resolve_browser_url_once(
            &mut browser_url_probe_attempted,
            should_refresh_browser_url,
            || {
                monitor::resolve_browser_url_for_window(
                    &active_window.app_name,
                    &active_window.window_title,
                )
            },
        ) {
            if active_window.browser_url.as_deref() != Some(resolved_url.as_str()) {
                log::debug!(
                    "浏览器 URL 落库前刷新: {} | {} -> {}",
                    active_window.app_name,
                    active_window.window_title,
                    resolved_url
                );
            }
            active_window.browser_url = Some(resolved_url);
            url_changed = match (&last_browser_url, &active_window.browser_url) {
                (Some(l), Some(r)) => l != r,
                (None, None) => false,
                _ => true,
            };
            app_changed = match &last_app_name {
                Some(last) => last != &active_window.app_name || url_changed || title_changed,
                None => true,
            };
        }

        // 保存切换前的应用名，用于时长归属修正
        let previous_app_name = if app_changed {
            last_app_name.clone()
        } else {
            None
        };

        // 取决定截图后，才更新上一个应用的信息
        last_app_name = Some(active_window.app_name.clone());
        last_app_window_title = Some(active_window.window_title.clone());
        last_browser_url = active_window.browser_url.clone();

        // 更新截图时间
        last_capture_time = std::time::Instant::now();

        // 使用距离上次截图的实际经过时间作为本次记录的时长
        // 而非固定的轮询间隔，避免截图间隔大于轮询间隔时丢失时长
        let (privacy_action, duration_to_record) = {
            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
            let action = state_guard.privacy_filter.check_privacy_full(
                &active_window.app_name,
                &active_window.window_title,
                active_window.browser_url.as_deref(),
            );
            // 窗口标题与 OCR 文本同等过滤后再向下游传递(入库/事件/分类)：
            // 邮件主题、IM 会话名里常出现邮箱/手机号等敏感信息,此前只过滤 OCR 文本。
            // 隐私判定(上方)与逐 tick 变化检测(last_* 变量)仍使用原始标题。
            active_window.window_title = state_guard
                .privacy_filter
                .filter_text(&active_window.window_title);
            // elapsed_secs 是距离上次截图的真实秒数，确保时长不丢失
            let duration = elapsed_secs.max(1) as i64;
            (action, duration)
        };
        // 锁已释放

        let current_timestamp = chrono::Local::now().timestamp();
        let previous_activity_to_backfill = if app_changed {
            resolve_previous_activity_to_backfill(
                &state,
                previous_app_name.as_deref(),
                previous_browser_url.as_deref(),
                previous_window_title.as_deref(),
            )
        } else {
            None
        };
        let adjusted_duration = if app_changed {
            0i64
        } else {
            duration_to_record
        };

        use privacy::PrivacyAction;
        let result: Option<database::Activity> = match privacy_action {
            PrivacyAction::Skip => {
                log::debug!(
                    "完全跳过: {} - {}",
                    active_window.app_name,
                    active_window.window_title
                );
                // 切到忽略应用不应吞掉上一应用的最后一段经过时间：
                // 与 Anonymize/Record 路径一致,先回补给上一活动再跳过本条
                let skip_is_confirmed_idle =
                    should_confirm_idle(input_idle, input_idle_seconds, false, false);
                let previous_effective_duration = previous_app_backfill_duration(
                    app_changed,
                    duration_to_record,
                    was_input_idle,
                    skip_is_confirmed_idle,
                );
                backfill_previous_activity_if_needed(
                    &state,
                    previous_activity_to_backfill.as_ref(),
                    previous_app_name.as_deref(),
                    previous_window_title.as_deref(),
                    previous_browser_url.as_deref(),
                    previous_effective_duration,
                    current_timestamp,
                    &active_window.app_name,
                );
                None
            }
            PrivacyAction::Anonymize => {
                log::debug!(
                    "内容脱敏: {} - {}",
                    active_window.app_name,
                    active_window.window_title
                );
                let classification = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    crate::resolve_activity_classification(
                        &state_guard.config,
                        &active_window.app_name,
                        &active_window.window_title,
                        active_window.browser_url.as_deref(),
                    )
                };
                let anonymized_is_confirmed_idle =
                    should_confirm_idle(input_idle, input_idle_seconds, false, false);
                let previous_effective_duration = previous_app_backfill_duration(
                    app_changed,
                    duration_to_record,
                    was_input_idle,
                    anonymized_is_confirmed_idle,
                );
                backfill_previous_activity_if_needed(
                    &state,
                    previous_activity_to_backfill.as_ref(),
                    previous_app_name.as_deref(),
                    previous_window_title.as_deref(),
                    previous_browser_url.as_deref(),
                    previous_effective_duration,
                    current_timestamp,
                    &active_window.app_name,
                );
                let effective_duration = if anonymized_is_confirmed_idle {
                    log::debug!("空闲确认: 脱敏活动跳过时长记录");
                    0
                } else {
                    adjusted_duration
                };

                if effective_duration <= 0 && !app_changed {
                    None
                } else {
                    let activity = database::Activity {
                        id: None,
                        timestamp: current_timestamp,
                        app_name: active_window.app_name,
                        window_title: "[内容已脱敏]".to_string(),
                        screenshot_path: String::new(),
                        ocr_text: None,
                        category: classification.base_category,
                        duration: effective_duration,
                        browser_url: None,
                        executable_path: active_window.executable_path,
                        semantic_category: Some(classification.semantic_category),
                        semantic_confidence: Some(i32::from(classification.confidence)),
                        screenshot_url: None,
                    };

                    // 短暂获取锁写入数据库
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    match state_guard.database.insert_activity(&activity) {
                        Ok(_) => Some(activity),
                        Err(e) => {
                            log::error!("保存活动记录失败: {e}");
                            None
                        }
                    }
                }
            }
            PrivacyAction::Record => {
                let (classification, screenshots_enabled) = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        crate::resolve_activity_classification(
                            &state_guard.config,
                            &active_window.app_name,
                            &active_window.window_title,
                            active_window.browser_url.as_deref(),
                        ),
                        state_guard.config.storage.screenshots_enabled,
                    )
                };
                let category = classification.base_category.clone();

                // 先检查是否有可合并的记录（在截屏之前判断，避免不必要的截图保存）
                let latest_activity = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(url) = active_window
                        .browser_url
                        .as_deref()
                        .filter(|url| !url.is_empty())
                    {
                        state_guard
                            .database
                            .get_latest_activity_by_url(url)
                            .ok()
                            .flatten()
                    } else if monitor::is_browser_app(&active_window.app_name)
                        && !active_window.window_title.is_empty()
                    {
                        state_guard
                            .database
                            .get_latest_activity_by_app_title(
                                &active_window.app_name,
                                &active_window.window_title,
                            )
                            .ok()
                            .flatten()
                    } else {
                        state_guard
                            .database
                            .get_latest_activity_by_app(&active_window.app_name)
                            .ok()
                            .flatten()
                    }
                };

                // 只合并当前连续会话。切回此前使用过的应用/页面必须新建记录，
                // 否则累计 duration 配合新 timestamp 会把中间会话覆盖成重叠区间。
                let is_merge = if let Some(ref latest) = latest_activity {
                    let mut merge = should_merge_contiguous_activity(
                        app_changed,
                        &active_window.app_name,
                        current_timestamp,
                        latest.timestamp,
                    );

                    // 如果由于某种原因 browser_url 获取失败，但它确实是一个浏览器
                    // 我们必须更严格地判断合并条件，否则不同标签页的切换会被错误合并。
                    if merge
                        && active_window.browser_url.is_none()
                        && monitor::is_browser_app(&active_window.app_name)
                        && (latest.window_title != active_window.window_title
                            || latest.browser_url.is_some())
                    {
                        merge = false;
                    }

                    merge
                } else {
                    false
                };

                if is_merge {
                    // === 合并路径：不保存截图，只做 OCR ===
                    let latest = latest_activity.unwrap();
                    let latest_id = match latest.id {
                        Some(id) => id,
                        None => {
                            log::error!("合并活动记录缺少 id，跳过");
                            continue;
                        }
                    };
                    let previous_screenshot_path = latest.screenshot_path.clone();

                    // 截屏到内存，保存为临时文件供 OCR 使用
                    let screenshot_result = if screenshots_enabled {
                        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                        state_guard
                            .screenshot_service
                            .capture_for_window(Some(&active_window))
                            .ok()
                    } else {
                        None
                    };

                    // ===== 空闲检测第二阶段：截图哈希确认 =====
                    // 只有键鼠超时时才检查屏幕变化，避免正常使用时的额外计算
                    let screenshot_idle = if input_idle {
                        if let Some(ref screenshot) = screenshot_result {
                            let hash = screenshot::ScreenshotService::calculate_image_hash(
                                &screenshot.path,
                            )
                            .unwrap_or(0);
                            idle_detector.confirm_idle_with_hash(hash)
                        } else {
                            false
                        }
                    } else {
                        // 有键鼠活动，重置空闲检测器
                        idle_detector.reset();
                        false
                    };
                    // 画面静止但键鼠超时：仅日志诊断，不再据此判空闲（创意类应用场景）
                    if input_idle && screenshot_idle && screenshots_enabled {
                        log::debug!(
                            "键鼠超时且画面静止，但未到硬超时，保留时长等待键鼠恢复 (idle_secs={input_idle_seconds})"
                        );
                    }
                    let is_confirmed_idle = should_confirm_idle(
                        input_idle,
                        input_idle_seconds,
                        screenshots_enabled,
                        screenshot_idle,
                    );
                    let previous_effective_duration = previous_app_backfill_duration(
                        app_changed,
                        duration_to_record,
                        was_input_idle,
                        is_confirmed_idle,
                    );
                    backfill_previous_activity_if_needed(
                        &state,
                        previous_activity_to_backfill.as_ref(),
                        previous_app_name.as_deref(),
                        previous_window_title.as_deref(),
                        previous_browser_url.as_deref(),
                        previous_effective_duration,
                        current_timestamp,
                        &active_window.app_name,
                    );

                    // 如果确认空闲，跳过时长记录
                    let effective_duration = if is_confirmed_idle {
                        log::debug!("空闲确认: 跳过本次时长记录");
                        0
                    } else {
                        adjusted_duration
                    };

                    // 截图仍用于 OCR；duration 为 0 时不能推进 timestamp，
                    // 否则已有区间会整体向后漂移。OCR 在下方独立更新。
                    let (latest_archive_path, ocr_input_path, temporary_ocr_source_path) =
                        if let Some(ref screenshot) = screenshot_result {
                            (
                                Some(screenshot.path.clone()),
                                screenshot
                                    .ocr_source_path
                                    .clone()
                                    .unwrap_or_else(|| screenshot.path.clone()),
                                screenshot
                                    .ocr_source_path
                                    .clone()
                                    .filter(|path| path != &screenshot.path),
                            )
                        } else {
                            (None, PathBuf::new(), None)
                        };

                    let persisted_screenshot_path = previous_screenshot_path.clone();
                    let mut persisted_duration = latest.duration;

                    if should_persist_merge_update(effective_duration) {
                        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                        match state_guard.database.merge_activity(
                            latest_id,
                            effective_duration,
                            None,
                            &previous_screenshot_path,
                            current_timestamp,
                            active_window.browser_url.as_deref(),
                        ) {
                            Ok(_) => {
                                persisted_duration += effective_duration;
                                log::info!(
                                    "✅ 合并成功: {} (id={}, 新时长={}s)",
                                    active_window.app_name,
                                    latest_id,
                                    latest.duration + effective_duration
                                );
                            }
                            Err(e) => {
                                log::error!("合并活动记录失败: {e}");
                            }
                        }
                    }

                    // 对截图执行 OCR；若已成功合并，则保留最新截图并清理旧截图
                    if let Some(screenshot) = screenshot_result {
                        let latest_capture_path =
                            latest_archive_path.unwrap_or_else(|| screenshot.path.clone());
                        let state_clone = state.clone();
                        let data_dir_clone = {
                            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            state_guard.data_dir.clone()
                        };

                        let ocr_sem = ocr_semaphore.clone();
                        let merge_hash = merge_screenshot_hash.clone();

                        tokio::spawn(async move {
                            use std::sync::atomic::Ordering;

                            // 非阻塞获取 permit，满载时跳过 OCR 避免任务堆积
                            let _permit = match ocr_sem.try_acquire_owned() {
                                Ok(p) => p,
                                Err(_) => {
                                    log::debug!("OCR 并发已满，跳过合并路径 OCR");
                                    if let Some(temp_path) = temporary_ocr_source_path.clone() {
                                        let _ = std::fs::remove_file(&temp_path);
                                    }
                                    let _ = std::fs::remove_file(&latest_capture_path);
                                    return;
                                }
                            };

                            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                            // 计算哈希做去重判断
                            let current_hash = screenshot::ScreenshotService::calculate_image_hash(
                                &latest_capture_path,
                            )
                            .unwrap_or(0);
                            let last_hash = merge_hash.swap(current_hash, Ordering::Relaxed);

                            let should_ocr = if last_hash != 0 {
                                let similarity = screenshot::ScreenshotService::hash_similarity(
                                    last_hash,
                                    current_hash,
                                );
                                if similarity > 90 {
                                    log::debug!("合并截图相似度 {similarity}%，跳过 OCR");
                                    false
                                } else {
                                    log::debug!("合并截图相似度 {similarity}%，执行 OCR");
                                    true
                                }
                            } else {
                                true
                            };

                            if should_ocr {
                                // OCR 是阻塞调用，放入 spawn_blocking 避免阻塞异步运行时
                                let data_dir_for_ocr = data_dir_clone.clone();
                                let ocr_source_path = ocr_input_path.clone();
                                let ocr_outcome = tokio::task::spawn_blocking(move || {
                                    let ocr_service = ocr::OcrService::new(&data_dir_for_ocr);
                                    ocr_service.extract_text(&ocr_source_path)
                                })
                                .await;
                                match ocr_outcome {
                                    Ok(Ok(Some(ocr_result))) => {
                                        if !ocr_result.text.is_empty() {
                                            let filtered_text =
                                                ocr::filter_sensitive_text(&ocr_result.text);
                                            if let Ok(state_guard) = state_clone.lock() {
                                                let _ = state_guard.database.update_activity_ocr(
                                                    latest_id,
                                                    Some(filtered_text),
                                                );
                                                log::info!(
                                                    "OCR 完成(合并): 活动 {} 识别到 {} 个字符",
                                                    latest_id,
                                                    ocr_result.text.len()
                                                );
                                            }
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        log::warn!("OCR 后台任务执行失败(合并): {e}");
                                    }
                                }
                            }

                            if let Some(temp_path) = temporary_ocr_source_path {
                                let _ = std::fs::remove_file(&temp_path);
                            }

                            let _ = std::fs::remove_file(&latest_capture_path);
                            log::debug!("已删除仅用于合并 OCR 的临时截图: {latest_capture_path:?}");
                        });
                    }

                    Some(database::Activity {
                        id: Some(latest_id),
                        timestamp: current_timestamp,
                        app_name: active_window.app_name.clone(),
                        window_title: active_window.window_title,
                        screenshot_path: persisted_screenshot_path,
                        ocr_text: None,
                        category,
                        duration: persisted_duration,
                        browser_url: active_window.browser_url,
                        executable_path: active_window.executable_path,
                        semantic_category: Some(classification.semantic_category.clone()),
                        semantic_confidence: Some(i32::from(classification.confidence)),
                        screenshot_url: None,
                    })
                } else {
                    // === 新建路径：正常截屏并保存 ===
                    if screenshots_enabled {
                        let screenshot_result = {
                            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            state_guard
                                .screenshot_service
                                .capture_for_window(Some(&active_window))
                        };

                        match screenshot_result {
                            Ok(screenshot_result) => {
                                // ===== 空闲检测第二阶段：截图哈希确认 =====
                                let screenshot_idle = if input_idle {
                                    let hash = screenshot::ScreenshotService::calculate_image_hash(
                                        &screenshot_result.path,
                                    )
                                    .unwrap_or(0);
                                    idle_detector.confirm_idle_with_hash(hash)
                                } else {
                                    idle_detector.reset();
                                    false
                                };
                                let is_confirmed_idle = should_confirm_idle(
                                    input_idle,
                                    input_idle_seconds,
                                    screenshots_enabled,
                                    screenshot_idle,
                                );
                                let previous_effective_duration = previous_app_backfill_duration(
                                    app_changed,
                                    duration_to_record,
                                    was_input_idle,
                                    is_confirmed_idle,
                                );
                                backfill_previous_activity_if_needed(
                                    &state,
                                    previous_activity_to_backfill.as_ref(),
                                    previous_app_name.as_deref(),
                                    previous_window_title.as_deref(),
                                    previous_browser_url.as_deref(),
                                    previous_effective_duration,
                                    current_timestamp,
                                    &active_window.app_name,
                                );

                                // 如果确认空闲，跳过时长记录（但仍创建活动记录以保持截图）
                                let effective_duration = if is_confirmed_idle {
                                    log::debug!("空闲确认: 新活动时长设为 0");
                                    0
                                } else {
                                    adjusted_duration
                                };

                                let (
                                    relative_path,
                                    archive_path,
                                    ocr_input_path,
                                    temporary_ocr_source_path,
                                    data_dir_clone,
                                ) = {
                                    let state_guard =
                                        state.lock().unwrap_or_else(|e| e.into_inner());
                                    (
                                        state_guard
                                            .screenshot_service
                                            .get_relative_path(&screenshot_result.path),
                                        screenshot_result.path.clone(),
                                        screenshot_result
                                            .ocr_source_path
                                            .clone()
                                            .unwrap_or_else(|| screenshot_result.path.clone()),
                                        screenshot_result
                                            .ocr_source_path
                                            .clone()
                                            .filter(|path| path != &screenshot_result.path),
                                        state_guard.data_dir.clone(),
                                    )
                                };

                                let activity = database::Activity {
                                    id: None,
                                    timestamp: screenshot_result.timestamp,
                                    app_name: active_window.app_name.clone(),
                                    window_title: active_window.window_title,
                                    screenshot_path: relative_path.clone(),
                                    ocr_text: None,
                                    category,
                                    duration: effective_duration,
                                    browser_url: active_window.browser_url,
                                    executable_path: active_window.executable_path,
                                    semantic_category: Some(
                                        classification.semantic_category.clone(),
                                    ),
                                    semantic_confidence: Some(i32::from(classification.confidence)),
                                    screenshot_url: None,
                                };

                                let inserted = {
                                    let state_guard =
                                        state.lock().unwrap_or_else(|e| e.into_inner());
                                    state_guard.database.insert_activity(&activity)
                                };

                                match inserted {
                                    Ok(activity_id) => {
                                        log::info!(
                                            "📝 新建活动: {} (id={})",
                                            active_window.app_name,
                                            activity_id
                                        );

                                        // 异步 OCR（新建活动的截图已保存，不删除）
                                        let state_clone = state.clone();
                                        let ocr_sem = ocr_semaphore.clone();
                                        tokio::spawn(async move {
                                            // 非阻塞获取 permit，满载时跳过 OCR
                                            let _permit = match ocr_sem.try_acquire_owned() {
                                                Ok(p) => p,
                                                Err(_) => {
                                                    log::debug!("OCR 并发已满，跳过新建路径 OCR");
                                                    if let Some(temp_path) =
                                                        temporary_ocr_source_path.clone()
                                                    {
                                                        let _ = std::fs::remove_file(&temp_path);
                                                    }
                                                    return;
                                                }
                                            };

                                            tokio::time::sleep(tokio::time::Duration::from_secs(1))
                                                .await;

                                            // OCR 是阻塞调用，放入 spawn_blocking 避免阻塞异步运行时
                                            let data_dir_for_ocr = data_dir_clone.clone();
                                            let ocr_source_path = ocr_input_path.clone();
                                            let ocr_outcome =
                                                tokio::task::spawn_blocking(move || {
                                                    let ocr_service =
                                                        ocr::OcrService::new(&data_dir_for_ocr);
                                                    ocr_service.extract_text(&ocr_source_path)
                                                })
                                                .await;

                                            match ocr_outcome {
                                                Ok(Ok(Some(ocr_result))) => {
                                                    if !ocr_result.text.is_empty() {
                                                        let filtered_text =
                                                            ocr::filter_sensitive_text(
                                                                &ocr_result.text,
                                                            );
                                                        if let Ok(state_guard) = state_clone.lock()
                                                        {
                                                            let _ = state_guard
                                                                .database
                                                                .update_activity_ocr(
                                                                    activity_id,
                                                                    Some(filtered_text),
                                                                );
                                                            log::info!(
                                                        "OCR 完成(新建): 活动 {} 识别到 {} 个字符",
                                                        activity_id,
                                                        ocr_result.text.len()
                                                    );
                                                        }
                                                    }
                                                }
                                                Ok(_) => {}
                                                Err(e) => {
                                                    log::warn!("OCR 后台任务执行失败(新建): {e}");
                                                }
                                            }

                                            if let Some(temp_path) = temporary_ocr_source_path {
                                                let _ = std::fs::remove_file(&temp_path);
                                            }
                                        });

                                        // 异步远程上传截图
                                        {
                                            let remote_cfg = {
                                                let g =
                                                    state.lock().unwrap_or_else(|e| e.into_inner());
                                                g.config.remote_storage.clone()
                                            };
                                            if remote_cfg.provider != work_review_core::config::RemoteStorageProvider::None {
                                                let st = state.clone();
                                                let ap = archive_path.clone();
                                                let rp = relative_path.clone();
                                                tokio::spawn(async move {
                                                    match remote_upload::upload_screenshot(&remote_cfg, &ap, &rp).await {
                                                        Ok(url) => {
                                                            log::info!("远程上传成功: {url}");
                                                            if let Ok(g) = st.lock() {
                                                                let _ = g.database.update_activity_screenshot_url(activity_id, &url);
                                                            }
                                                        }
                                                        Err(e) => log::warn!("远程上传失败: {e}"),
                                                    }
                                                });
                                            }
                                        }

                                        Some(database::Activity {
                                            id: Some(activity_id),
                                            ..activity
                                        })
                                    }
                                    Err(e) => {
                                        log::error!("保存活动记录失败: {e}");
                                        let _ = std::fs::remove_file(&archive_path);
                                        if let Some(temp_path) = temporary_ocr_source_path {
                                            let _ = std::fs::remove_file(&temp_path);
                                        }
                                        None
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("截屏失败: {e}");
                                None
                            }
                        }
                    } else {
                        let is_confirmed_idle = should_confirm_idle(
                            input_idle,
                            input_idle_seconds,
                            screenshots_enabled,
                            false,
                        );
                        let previous_effective_duration = previous_app_backfill_duration(
                            app_changed,
                            duration_to_record,
                            was_input_idle,
                            is_confirmed_idle,
                        );
                        backfill_previous_activity_if_needed(
                            &state,
                            previous_activity_to_backfill.as_ref(),
                            previous_app_name.as_deref(),
                            previous_window_title.as_deref(),
                            previous_browser_url.as_deref(),
                            previous_effective_duration,
                            current_timestamp,
                            &active_window.app_name,
                        );
                        let effective_duration = if is_confirmed_idle {
                            log::debug!("关闭截图后按输入空闲判定，新活动时长设为 0");
                            0
                        } else {
                            adjusted_duration
                        };

                        let activity = database::Activity {
                            id: None,
                            timestamp: current_timestamp,
                            app_name: active_window.app_name.clone(),
                            window_title: active_window.window_title,
                            screenshot_path: String::new(),
                            ocr_text: None,
                            category,
                            duration: effective_duration,
                            browser_url: active_window.browser_url,
                            executable_path: active_window.executable_path,
                            semantic_category: Some(classification.semantic_category.clone()),
                            semantic_confidence: Some(i32::from(classification.confidence)),
                            screenshot_url: None,
                        };

                        let inserted = {
                            let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            state_guard.database.insert_activity(&activity)
                        };

                        match inserted {
                            Ok(activity_id) => {
                                log::info!(
                                    "📝 新建无截图活动: {} (id={})",
                                    active_window.app_name,
                                    activity_id
                                );
                                Some(database::Activity {
                                    id: Some(activity_id),
                                    ..activity
                                })
                            }
                            Err(e) => {
                                log::error!("保存无截图活动记录失败: {e}");
                                None
                            }
                        }
                    }
                }
            }
        };

        // 发送事件到前端
        if let Some(activity) = result {
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                let _ = window.emit("screenshot-taken", &activity);
            }
        }

        // ===== 浮动窗口（PiP 画中画）检测 =====
        // 检测 layer > 0 的浮动窗口（如视频小窗），为它们记录使用时长
        // 浮动窗口不截图（截图已由主活动管理），仅记录时长
        let overlay_windows = monitor::get_overlay_windows(&frontmost_app_name);
        for ow in &overlay_windows {
            // 隐私检查
            let ow_privacy = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                state_guard
                    .privacy_filter
                    .check_privacy(&ow.app_name, &ow.window_title)
            };

            if ow_privacy == privacy::PrivacyAction::Skip {
                log::debug!("浮动窗口跳过(隐私): {}", ow.app_name);
                continue;
            }

            let overlay_is_confirmed_idle =
                should_confirm_idle(input_idle, input_idle_seconds, false, false);
            if overlay_is_confirmed_idle {
                log::debug!("浮动窗口空闲确认，跳过时长记录: {}", ow.app_name);
                continue;
            }

            let classification = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                crate::resolve_activity_classification(
                    &state_guard.config,
                    &ow.app_name,
                    &ow.window_title,
                    ow.browser_url.as_deref(),
                )
            };
            let ow_category = classification.base_category.clone();
            let current_ts = chrono::Local::now().timestamp();
            let ow_duration = poll_interval_ms.div_ceil(1000) as i64;

            // 查找该应用的最近活动记录，尝试合并
            let latest = {
                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                state_guard
                    .database
                    .get_latest_activity_by_app(&ow.app_name)
                    .ok()
                    .flatten()
            };

            const OW_MERGE_GAP_SECS: i64 = 600;
            let can_merge = if let Some(ref act) = latest {
                ow.app_name != "Unknown" && (current_ts - act.timestamp) <= OW_MERGE_GAP_SECS
            } else {
                false
            };

            if can_merge {
                let act = latest.unwrap();
                if let Some(act_id) = act.id {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    match state_guard.database.merge_activity(
                        act_id,
                        ow_duration,
                        None,
                        &act.screenshot_path,
                        current_ts,
                        None,
                    ) {
                        Ok(_) => {
                            log::info!(
                                "🪟 浮动窗口合并: {} (id={}, +{}s, 总{}s)",
                                ow.app_name,
                                act_id,
                                ow_duration,
                                act.duration + ow_duration
                            );
                        }
                        Err(e) => log::error!("浮动窗口合并失败: {e}"),
                    }
                }
            } else {
                // 新建活动记录（无截图）
                let ow_title = if ow_privacy == privacy::PrivacyAction::Anonymize {
                    "[内容已脱敏]".to_string()
                } else {
                    ow.window_title.clone()
                };

                let activity = database::Activity {
                    id: None,
                    timestamp: current_ts,
                    app_name: ow.app_name.clone(),
                    window_title: ow_title,
                    screenshot_path: String::new(),
                    ocr_text: None,
                    category: ow_category,
                    duration: ow_duration,
                    browser_url: None,
                    executable_path: ow.executable_path.clone(),
                    semantic_category: Some(classification.semantic_category),
                    semantic_confidence: Some(i32::from(classification.confidence)),
                    screenshot_url: None,
                };

                let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                match state_guard.database.insert_activity(&activity) {
                    Ok(id) => {
                        log::info!(
                            "🪟 浮动窗口新建: {} (id={}, {}s)",
                            ow.app_name,
                            id,
                            ow_duration
                        );
                    }
                    Err(e) => log::error!("浮动窗口记录失败: {e}"),
                }
            }
        }
    }
}

/// 小时摘要生成任务
/// 每小时检查一次，为上一个完整小时生成摘要
/// 为指定日期和小时生成并保存摘要
/// 计算某小时的摘要（不落库）。该小时无活动或查询失败时返回 None。
pub(crate) fn build_hourly_summary(
    state: &Arc<Mutex<AppState>>,
    date: &str,
    hour: i32,
) -> Option<database::HourlySummary> {
    use analysis::hourly::{generate_fallback_summary, HourlyStats};

    let activities = {
        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        state_guard.database.get_hourly_activities(date, hour)
    };

    match activities {
        Ok(acts) if !acts.is_empty() => {
            let stats = HourlyStats::from_activities(date, hour, acts);
            let summary = generate_fallback_summary(&stats);

            Some(database::HourlySummary {
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
        Ok(_) => {
            log::debug!("该小时无活动数据: {date} {hour}:00");
            None
        }
        Err(e) => {
            log::error!("获取小时活动数据失败: {e}");
            None
        }
    }
}

pub(crate) fn generate_and_save_summary(state: &Arc<Mutex<AppState>>, date: &str, hour: i32) {
    let Some(hourly_summary) = build_hourly_summary(state, date, hour) else {
        return;
    };

    let save_result = {
        let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
        state_guard.database.save_hourly_summary(&hourly_summary)
    };

    match save_result {
        Ok(_) => log::info!("小时摘要保存成功: {date} {hour}:00"),
        Err(e) => log::error!("保存小时摘要失败: {e}"),
    }
}

async fn hourly_summary_task(state: Arc<Mutex<AppState>>) {
    use chrono::{Local, Timelike};

    // 等待30秒后开始（给应用启动留时间，但不用等太久）
    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    // 启动时回填今天所有已过时段的摘要（覆盖旧格式数据）
    {
        let now = Local::now();
        let date = now.format("%Y-%m-%d").to_string();
        let current_hour = now.hour() as i32;

        log::info!("回填今天 0:00 ~ {current_hour}:00 的小时摘要");
        for hour in 0..current_hour {
            generate_and_save_summary(&state, &date, hour);
        }
    }

    loop {
        let now = Local::now();
        let current_hour = now.hour() as i32;
        let date = now.format("%Y-%m-%d").to_string();

        // 为上一个小时生成摘要（如果还没有）
        let target_hour = if current_hour > 0 {
            current_hour - 1
        } else {
            23
        };
        let target_date = if current_hour > 0 {
            date.clone()
        } else {
            (now - chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string()
        };

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

        // 休眠到下一个小时的第5分钟
        let next_check = (now + chrono::Duration::hours(1))
            .with_minute(5)
            .unwrap()
            .with_second(0)
            .unwrap();
        let sleep_duration = (next_check - now).num_seconds().max(60) as u64;
        tokio::time::sleep(tokio::time::Duration::from_secs(sleep_duration)).await;
    }
}

#[tauri::command]
fn get_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    return "macos";
    #[cfg(target_os = "windows")]
    return "windows";
    #[cfg(target_os = "linux")]
    return "linux";
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    return "unknown";
}

/// 安装崩溃捕获：panic 时把版本、时间、panic 信息与完整调用栈写入
/// `<数据目录>/crashes/crash-<时间>.log`，方便用户回传后定位闪退根因。
fn install_crash_handler() {
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("<无法获取 panic 信息>");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<未知位置>".to_string());
        let now = chrono::Local::now();
        let report = format!(
            "Work Review crash report\n版本: {}\n时间: {}\nPanic: {}\n位置: {}\n\n调用栈:\n{}\n",
            env!("CARGO_PKG_VERSION"),
            now.format("%Y-%m-%d %H:%M:%S"),
            message,
            location,
            backtrace
        );
        let crash_dir = default_data_dir().join("crashes");
        match std::fs::create_dir_all(&crash_dir).and_then(|_| {
            std::fs::write(
                crash_dir.join(format!("crash-{}.log", now.format("%Y%m%d_%H%M%S"))),
                &report,
            )
        }) {
            Ok(()) => eprintln!("崩溃日志已写入 {}/", crash_dir.display()),
            Err(e) => eprintln!("写入崩溃日志失败: {e}"),
        }
        eprintln!("{report}");
    }));
}

/// 远程存储补传任务：定期扫描「本地有截图但远程 URL 缺失」的近期活动并补传。
/// 实时上传失败只有一次轻量重试（见 remote_upload），断网/服务不可用超过该窗口的
/// 截图会永久缺失——本任务作为兜底，每 10 分钟补传一批。
/// 窗口限定近 72 小时：更早的缺口多半是远程存储当时未启用或本地文件已被保留策略清理，
/// 反复重试没有意义。
async fn remote_upload_backfill_task(state: Arc<Mutex<AppState>>) {
    const INITIAL_DELAY_SECS: u64 = 60;
    const SWEEP_INTERVAL_SECS: u64 = 10 * 60;
    const BACKFILL_WINDOW_SECS: i64 = 72 * 60 * 60;
    const BATCH_LIMIT: u32 = 20;

    tokio::time::sleep(tokio::time::Duration::from_secs(INITIAL_DELAY_SECS)).await;

    loop {
        let (remote_cfg, data_dir, pending) = {
            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
            let remote_cfg = guard.config.remote_storage.clone();
            if remote_cfg.provider == work_review_core::config::RemoteStorageProvider::None {
                (remote_cfg, std::path::PathBuf::new(), Vec::new())
            } else {
                let since = chrono::Local::now().timestamp() - BACKFILL_WINDOW_SECS;
                let pending = guard
                    .database
                    .get_activities_missing_screenshot_url(since, BATCH_LIMIT)
                    .unwrap_or_else(|e| {
                        log::warn!("查询待补传截图失败: {e}");
                        Vec::new()
                    });
                (remote_cfg, guard.data_dir.clone(), pending)
            }
        };

        let mut uploaded = 0u32;
        for (activity_id, relative_path) in pending {
            // 历史数据可能存过绝对路径，跳过以免拼出畸形的远程对象键
            if std::path::Path::new(&relative_path).is_absolute() {
                continue;
            }
            let local_path = data_dir.join(&relative_path);
            if !local_path.exists() {
                continue; // 本地文件已被保留策略清理，无从补传
            }
            match remote_upload::upload_screenshot(&remote_cfg, &local_path, &relative_path).await {
                Ok(url) => {
                    let updated = {
                        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                        guard
                            .database
                            .update_activity_screenshot_url(activity_id, &url)
                    };
                    match updated {
                        Ok(()) => uploaded += 1,
                        Err(e) => {
                            log::warn!("补传后写回 screenshot_url 失败（活动 {activity_id}）: {e}")
                        }
                    }
                }
                Err(e) => log::warn!("补传截图失败（活动 {activity_id}）: {e}"),
            }
            // 温和限速，避免补传批次挤占正常上传与前台网络
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
        if uploaded > 0 {
            log::info!("截图补传完成：本轮补传 {uploaded} 张");
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
    }
}

/// AI 实体分类缓存的进程内查询表（entity_key → (基础分类, 语义分类)）。
/// 启动时从数据库载入,entity_classify_task 学习到新实体后同步更新,
/// 采集循环里的 resolve_activity_classification 只读查询。
pub(crate) fn entity_category_cache(
) -> &'static std::sync::RwLock<std::collections::HashMap<String, (String, String)>> {
    static CACHE: OnceCell<std::sync::RwLock<std::collections::HashMap<String, (String, String)>>> =
        OnceCell::new();
    CACHE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// 调用文本模型对一批实体做归类,返回 (entity_key, 基础分类, 语义分类)。
async fn classify_entities_with_model(
    model: &work_review_core::config::ModelConfig,
    entities: &[String],
    category_options: &str,
    semantic_options: &str,
    timeout_secs: u64,
) -> Option<Vec<(String, String, String)>> {
    let list = entities
        .iter()
        .map(|e| format!("- {e}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "你是活动分类助手。以下实体来自个人时间追踪软件,格式为 kind:名称(app 表示桌面应用,domain 表示网站域名)。\n\
         请为每个实体选择最贴切的基础分类 key 与语义分类名,只能从给定选项中选。\n\
         判定必须保守:只有名称有明确证据(明显是视频/游戏/音乐/购物等)才可归入娱乐或消遣类分类;\n\
         看起来像开发工具、软件项目名、专有名词或含义不明的实体,宁可跳过也不要猜——\n\
         对没有把握的实体,base 填 \"skip\",本轮不学习该实体。\n\n\
         基础分类(key: 名称)：{category_options}\n语义分类：{semantic_options}\n\n实体：\n{list}\n\n\
         只输出 JSON 数组,不要任何其他文字。元素形如 {{\"key\":\"app:xxx\",\"base\":\"development\",\"semantic\":\"编码开发\"}}"
    );

    let endpoint = model.endpoint.trim().trim_end_matches('/');
    let url = if endpoint.ends_with("/chat/completions") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/chat/completions")
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let mut request = client.post(&url).json(&serde_json::json!({
        "model": model.model,
        "messages": [{ "role": "user", "content": prompt }],
        "temperature": 0,
        "stream": false,
    }));
    if let Some(key) = model.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        request = request.header("Authorization", format!("Bearer {key}"));
    }

    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        log::warn!("实体分类模型调用失败: HTTP {}", response.status());
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    let content = value["choices"][0]["message"]["content"].as_str()?;
    let content = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Vec<serde_json::Value> = serde_json::from_str(content).ok()?;
    Some(
        parsed
            .into_iter()
            .filter_map(|item| {
                Some((
                    item["key"].as_str()?.to_string(),
                    item["base"].as_str()?.to_string(),
                    item["semantic"].as_str()?.to_string(),
                ))
            })
            .collect(),
    )
}

/// AI 实体分类学习任务：定期把"仍未识别"的应用与域名交给文本模型归类一次,
/// 写入缓存供采集即时查询,并回溯修正近 14 天的历史记录——分类随使用越来越准,
/// 不再依赖用户手动加规则。未配置文本模型时静默空转;内置知识库与用户显式规则始终优先
/// （候选收集阶段已排除两者覆盖的实体）。
async fn entity_classify_task(state: Arc<Mutex<AppState>>) {
    const INITIAL_DELAY_SECS: u64 = 180;
    const SWEEP_INTERVAL_SECS: u64 = 30 * 60;
    const LOOKBACK_SECS: i64 = 3 * 24 * 60 * 60;
    const RETRO_SECS: i64 = 14 * 24 * 60 * 60;
    const MIN_TOTAL_DURATION_SECS: i64 = 600;
    const BATCH_LIMIT: usize = 12;

    tokio::time::sleep(tokio::time::Duration::from_secs(INITIAL_DELAY_SECS)).await;

    loop {
        let now = chrono::Local::now().timestamp();
        let (
            model,
            category_options,
            semantic_options,
            valid_bases,
            valid_semantics,
            app_rule_names,
            website_rule_domains,
            app_candidates,
            url_durations,
            assistant_timeout_secs,
        ) = {
            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
            let model = guard.config.text_model.clone();
            let category_options = guard
                .config
                .custom_categories
                .iter()
                .filter(|c| c.key != "browser")
                .map(|c| format!("{}: {}", c.key, c.name))
                .collect::<Vec<_>>()
                .join("、");
            let semantic_options = guard
                .config
                .custom_semantic_categories
                .iter()
                .map(|c| c.key.clone())
                .collect::<Vec<_>>()
                .join("、");
            let valid_bases: std::collections::HashSet<String> = guard
                .config
                .custom_categories
                .iter()
                .map(|c| c.key.clone())
                .collect();
            let valid_semantics: std::collections::HashSet<String> = guard
                .config
                .custom_semantic_categories
                .iter()
                .map(|c| c.key.clone())
                .collect();
            let app_rule_names: Vec<String> = guard
                .config
                .app_category_rules
                .iter()
                .map(|r| r.app_name.to_lowercase())
                .filter(|r| !r.is_empty())
                .collect();
            let website_rule_domains: Vec<String> = guard
                .config
                .website_semantic_rules
                .iter()
                .map(|r| r.domain.to_lowercase())
                .filter(|r| !r.is_empty())
                .collect();
            let app_candidates = guard
                .database
                .get_unclassified_app_candidates(now - LOOKBACK_SECS, MIN_TOTAL_DURATION_SECS, 20)
                .unwrap_or_default();
            let url_durations = guard
                .database
                .get_browser_url_durations(now - LOOKBACK_SECS)
                .unwrap_or_default();
            (
                model,
                category_options,
                semantic_options,
                valid_bases,
                valid_semantics,
                app_rule_names,
                website_rule_domains,
                app_candidates,
                url_durations,
                guard.config.assistant_timeout_secs,
            )
        };

        if model.endpoint.trim().is_empty() || model.model.trim().is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
            continue;
        }

        let cached_keys: std::collections::HashSet<String> = entity_category_cache()
            .read()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();

        // 应用候选:排除已学过与用户规则已覆盖的
        let mut entities: Vec<String> = Vec::new();
        for app in app_candidates {
            let app_lower = app.to_lowercase();
            let key = format!("app:{app_lower}");
            if cached_keys.contains(&key)
                || app_rule_names
                    .iter()
                    .any(|r| app_lower.contains(r.as_str()))
            {
                continue;
            }
            entities.push(key);
            if entities.len() >= BATCH_LIMIT {
                break;
            }
        }

        // 域名候选:聚合累计时长,排除知识库/用户规则/已学过的
        let mut domain_totals: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for (url, duration) in url_durations {
            let domain = work_review_core::config::PrivacyConfig::extract_domain(&url);
            let domain = domain.split(':').next().unwrap_or("").to_string();
            if !domain.is_empty() {
                *domain_totals.entry(domain).or_insert(0) += duration;
            }
        }
        let mut domain_candidates: Vec<(String, i64)> = domain_totals
            .into_iter()
            .filter(|(domain, total)| {
                *total >= MIN_TOTAL_DURATION_SECS
                    && work_review_core::knowledge::builtin_domain_category(domain).is_none()
                    && !cached_keys.contains(&format!("domain:{domain}"))
                    && !website_rule_domains.iter().any(|rule| {
                        work_review_core::config::PrivacyConfig::domain_matches(domain, rule)
                    })
            })
            .collect();
        domain_candidates.sort_by_key(|item| std::cmp::Reverse(item.1));
        for (domain, _) in domain_candidates {
            if entities.len() >= BATCH_LIMIT {
                break;
            }
            entities.push(format!("domain:{domain}"));
        }

        if entities.is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
            continue;
        }

        let Some(results) = classify_entities_with_model(
            &model,
            &entities,
            &category_options,
            &semantic_options,
            assistant_timeout_secs,
        )
        .await
        else {
            log::warn!("实体自动归类本轮失败,下轮重试");
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
            continue;
        };

        let mut learned = 0usize;
        for (key, base, semantic) in results {
            // 模型对没把握的实体返回 "skip":不学习、不缓存,留待将来有更多上下文再判
            if base == "skip" {
                continue;
            }
            // 只接受本批实体、且分类必须在现有集合内("browser" 不作为学习目标)
            if !entities.contains(&key)
                || base == "browser"
                || !valid_bases.contains(&base)
                || !valid_semantics.contains(&semantic)
            {
                continue;
            }

            let persisted = {
                let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .database
                    .upsert_entity_category(&key, &base, &semantic)
            };
            if let Err(e) = persisted {
                log::warn!("写入实体分类缓存失败({key}): {e}");
                continue;
            }
            if let Ok(mut map) = entity_category_cache().write() {
                map.insert(key.clone(), (base.clone(), semantic.clone()));
            }

            // 回溯修正近 14 天历史记录,让统计"自己变准"
            let updated = if let Some(app) = key.strip_prefix("app:") {
                let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .database
                    .reclassify_app_activities(app, &base, &semantic, now - RETRO_SECS)
                    .unwrap_or(0)
            } else if let Some(domain) = key.strip_prefix("domain:") {
                let rows = {
                    let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    guard
                        .database
                        .get_browser_activity_urls(now - RETRO_SECS)
                        .unwrap_or_default()
                };
                let ids: Vec<i64> = rows
                    .into_iter()
                    .filter(|(_, url)| {
                        let row_domain =
                            work_review_core::config::PrivacyConfig::extract_domain(url);
                        row_domain.split(':').next().unwrap_or("") == domain
                    })
                    .map(|(id, _)| id)
                    .collect();
                let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .database
                    .reclassify_activities_by_ids(&ids, &base, &semantic)
                    .unwrap_or(0)
            } else {
                0
            };

            learned += 1;
            log::info!("实体自动归类: {key} → {base}/{semantic}（回溯修正 {updated} 条）");
        }
        if learned > 0 {
            log::info!("实体分类自动学习完成: 本轮学习 {learned} 个实体");
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
    }
}

/// 存储清理周期任务：保留天数与容量上限此前只在启动时执行一次,
/// 应用常驻数周就完全不生效——这里每 6 小时补跑一次。
/// 配置损坏(fail-safe)时与启动路径一致,跳过清理以保护历史数据。
async fn storage_cleanup_task(state: Arc<Mutex<AppState>>) {
    const SWEEP_INTERVAL_SECS: u64 = 6 * 60 * 60;

    loop {
        // 启动清理已在 main 里执行过,首轮直接等一个周期
        tokio::time::sleep(tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;

        let (data_dir, storage_config, allowed) = {
            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                guard.data_dir.clone(),
                guard.config.storage.clone(),
                should_run_startup_cleanup(guard.config_load_status),
            )
        };
        if !allowed {
            continue;
        }

        // 用独立实例在阻塞线程执行,避免清理期间占用 AppState 锁阻塞采集循环
        let manager = StorageManager::new(&data_dir, storage_config);
        match tokio::task::spawn_blocking(move || manager.cleanup()).await {
            Ok(Ok(_)) => log::debug!("周期存储清理完成"),
            Ok(Err(e)) => log::warn!("周期存储清理失败: {e}"),
            Err(e) => log::warn!("周期存储清理任务异常: {e}"),
        }
    }
}

/// Linux: 注入 WebKit/Wayland 兼容性环境变量，规避 webkit2gtk 在 Wayland（尤其
/// KDE Plasma / NVIDIA）下启动即崩的问题（"Error 71 (Protocol error) dispatching
/// to Wayland display"，上游 Tauri #10702 / GTK WebKitGTK）。必须在 GTK/WebKit
/// 初始化前调用；仅当用户未显式设置时注入，便于自行覆盖或换用 GDK_BACKEND=x11。
#[cfg(target_os = "linux")]
fn apply_linux_webkit_wayland_workarounds() {
    // webkit2gtk 的 DMA-BUF 渲染器在部分 Wayland compositor 上会触发协议错误，
    // 关闭后回退到传统渲染路径，是 Tauri 社区对 #10702 最普遍有效的 workaround。
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

#[tokio::main]
async fn main() {
    // 安装崩溃捕获（最早执行，确保后续任何 panic 都能记录调用栈）
    install_crash_handler();

    // 初始化日志
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Linux: 修复 sudo 下丢失的 Wayland/DBus 环境变量
    #[cfg(target_os = "linux")]
    linux_session::fix_wayland_env_if_sudo();

    // Linux: 注入 WebKit/Wayland 兼容性 workaround，规避 KDE Plasma / NVIDIA 等环境下
    // webkit2gtk 启动即崩（"Error 71 (Protocol error) dispatching to Wayland display"）。
    // 上游追踪：https://github.com/tauri-apps/tauri/issues/10702 。仅当用户未显式设置时注入。
    #[cfg(target_os = "linux")]
    apply_linux_webkit_wayland_workarounds();

    log::info!("work回顾助手启动中...");

    // 获取数据目录
    let data_dir = resolve_data_dir();
    log::info!("数据目录: {data_dir:?}");

    // 加载配置
    let config_path = data_dir.join("config.json");
    let load_result = AppConfig::load_with_recovery(&config_path);
    let config_load_status = load_result.status;
    let backup_path = config_backup_path(&config_path);
    match config_load_status {
        ConfigLoadStatus::Loaded => {}
        ConfigLoadStatus::Missing => {
            log::info!(
                "主配置与备份均不存在，按首次启动使用默认配置。主配置: {}; 备份配置: {}",
                config_path.display(),
                backup_path.display()
            );
        }
        ConfigLoadStatus::RecoveredFromBackup => {
            log::warn!(
                "主配置不可用，已从备份 {} 恢复到内存；为保留现场，不会自动覆盖主配置。主配置: {}",
                backup_path.display(),
                describe_config_file_issue(&config_path, load_result.primary_error.as_deref())
            );
        }
        ConfigLoadStatus::Corrupted => {
            log::error!(
                "主配置与备份均不可用，已使用关闭采集、截图和网络服务的故障安全配置。主配置: {}; 备份配置: {}",
                describe_config_file_issue(&config_path, load_result.primary_error.as_deref()),
                describe_config_file_issue(&backup_path, load_result.backup_error.as_deref())
            );
        }
    }
    #[allow(unused_mut)]
    let mut config = load_result.config;

    // 清理历史遗留的钥匙串占位符（该机制已移除）
    clear_legacy_keychain_placeholders(&mut config);

    // 迁移旧版 excluded_apps → app_rules
    if config.privacy.migrate_legacy_excluded_apps() {
        log::info!("已迁移旧版 excluded_apps 到 app_rules");
        if config_load_status.allows_automatic_save() {
            if let Err(e) = config.save(&config_path) {
                log::warn!("保存迁移后的配置失败: {e}");
            }
        }
    }

    // 初始化数据库
    let db_path = data_dir.join("workreview.db");
    let database = Database::new(&db_path).expect("初始化数据库失败");

    // 版本变化检测（首次启动同样视为变化）：FTS 重建门控与下方录屏权限引导复用
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let is_version_changed = config.last_app_version.as_deref() != Some(&current_version);

    // 首次启动或升级后重建 FTS 索引，确保历史数据可被全文检索。
    // 平时启动跳过：全量重建成本随库体积线性增长,而 FTS 触发器已维持增量同步
    if is_version_changed {
        if let Err(e) = database.rebuild_fts_index() {
            log::warn!("FTS 索引重建失败（不影响核心功能）: {e}");
        }
    }

    // 载入 AI 实体分类缓存到内存查询表（采集循环即时查询,零额外 IO）
    match database.load_entity_category_cache() {
        Ok(entries) => {
            if !entries.is_empty() {
                log::info!("载入实体分类缓存 {} 条", entries.len());
            }
            if let Ok(mut map) = entity_category_cache().write() {
                for (key, base, semantic) in entries {
                    map.insert(key, (base, semantic));
                }
            }
        }
        Err(e) => log::warn!("载入实体分类缓存失败: {e}"),
    }

    // 初始化隐私过滤器
    let privacy_filter = PrivacyFilter::from_config(&config.privacy);

    // 初始化截屏服务
    let screenshot_service = ScreenshotService::new(&data_dir, &config.storage);

    // 版本更新后重置 macOS 录屏权限引导标记，确保更新后能重新弹窗
    if is_version_changed {
        #[cfg(target_os = "macos")]
        if config.last_app_version.is_some() {
            log::info!(
                "检测到版本更新 ({} → {})，重置录屏权限引导标记",
                config.last_app_version.as_deref().unwrap_or("-"),
                current_version
            );
            config.macos_screen_capture_permission_prompted = false;
        }
        config.last_app_version = Some(current_version);
        if config_load_status.allows_automatic_save() {
            if let Err(e) = config.save(&config_path) {
                log::warn!("保存版本信息失败: {e}");
            }
        }
    }

    // macOS: 启动时检查并请求必要的系统权限
    #[cfg(target_os = "macos")]
    {
        if should_initialize_startup_permissions(config_load_status) {
            // 1. 屏幕录制权限（截图功能必需）
            let has_screen_capture_permission = screenshot::has_screen_capture_permission();
            let already_prompted = config.macos_screen_capture_permission_prompted;
            if should_request_screen_capture_permission(
                has_screen_capture_permission,
                already_prompted,
            ) {
                log::warn!("⚠️  屏幕录制权限未授权，正在请求...");
                log::warn!(
                    "   请在「系统设置 → 隐私与安全性 → 屏幕录制」中授权 Work Review，然后重启应用"
                );
                screenshot::request_screen_capture_permission();
            } else if !has_screen_capture_permission {
                log::warn!("⚠️  屏幕录制权限仍未授权，跳过重复请求，请在系统设置中确认后重启应用");
            } else {
                log::info!("✅ 屏幕录制权限已授权");
            }
            config.macos_screen_capture_permission_prompted = !has_screen_capture_permission;
            if config.macos_screen_capture_permission_prompted != already_prompted
                && config_load_status.allows_automatic_save()
            {
                if let Err(e) = config.save(&config_path) {
                    log::warn!("保存 macOS 录屏权限提示状态失败: {e}");
                }
            }

            // 2. 辅助功能权限（读取窗口标题、浏览器 URL 必需）
            if !screenshot::has_accessibility_permission(false) {
                log::warn!("⚠️  辅助功能权限未授权，正在请求...");
                log::warn!("   请在「系统设置 → 隐私与安全性 → 辅助功能」中授权 Work Review");
                // prompt=true 会弹出系统引导对话框
                screenshot::has_accessibility_permission(true);
            } else {
                log::info!("✅ 辅助功能权限已授权");
            }

            // 3. 输入监控权限（桌宠键鼠联动必需）
            if config.avatar_enabled && !screenshot::has_input_monitoring_permission() {
                log::warn!("⚠️  输入监控权限未授权，正在请求...");
                log::warn!("   请在「系统设置 → 隐私与安全性 → 输入监控」中授权 Work Review");
                screenshot::request_input_monitoring_permission();
            } else if config.avatar_enabled {
                log::info!("✅ 输入监控权限已授权");
            }
        }
    }

    // 初始化存储管理器
    let storage_manager = StorageManager::new(&data_dir, config.storage.clone());
    let initial_avatar_opacity = config.avatar_opacity;
    let initial_avatar_preset = config.avatar_preset.clone();
    let initial_avatar_persona = config.avatar_persona.clone();
    let initial_avatar_body_hidden = config.avatar_body_hidden;

    // 配置损坏时使用默认存储策略会误删历史数据，因此故障安全启动必须跳过清理。
    if should_run_startup_cleanup(config_load_status) {
        if let Err(e) = storage_manager.cleanup() {
            log::warn!("启动时清理存储失败: {e}");
        }
    } else {
        log::warn!("配置损坏，已跳过启动存储清理以保护历史数据");
    }

    let (is_recording, is_paused) = initial_recording_state(config_load_status);

    // 创建应用状态，使用 Arc 包装以便在多个地方共享
    let app_state = Arc::new(Mutex::new(AppState {
        config,
        config_load_status,
        database,
        privacy_filter,
        screenshot_service,
        storage_manager,
        data_dir,
        config_path,
        is_recording,
        is_paused,
        avatar_state: avatar_engine::apply_avatar_visual_settings(
            avatar_engine::default_avatar_state(),
            initial_avatar_opacity,
            &initial_avatar_preset,
            &initial_avatar_persona,
            initial_avatar_body_hidden,
        ),
        avatar_generating_report: false,
        generating_report: false,
        localhost_api_runtime: localhost_api::LocalhostApiRuntime::default(),
        telegram_bot_runtime: telegram_bot::TelegramBotRuntime::default(),
        cached_active_window: None,
    }));
    let app_lifecycle_state = Arc::new(Mutex::new(AppLifecycleState::default()));

    // 构建 Tauri 应用
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(not(windows))]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        Some(vec![AUTOSTART_LAUNCH_ARG]),
    ));
    builder
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            // 如果第二个实例是自启动触发的，保持静默不弹窗
            if duplicate_instance_should_stay_silent(&argv) {
                log::info!("检测到重复自启动，保持静默 | 参数: {argv:?}");
                return;
            }
            // 当用户尝试打开第二个实例时，将焦点给到现有窗口
            if let Err(e) = reveal_main_window(&app.clone(), None) {
                log::warn!("恢复主窗口失败: {e}");
            }
            log::info!("检测到重复打开，参数: {argv:?}, 工作目录: {cwd}");
        }))
        .manage(app_state.clone())
        .manage(app_lifecycle_state.clone())
        // 系统托盘在 setup 中创建 (Tauri v2)
        .on_window_event(|window, event| {
            if window.label() != MAIN_WINDOW_LABEL {
                return;
            }

            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let lightweight_mode = window
                    .try_state::<Arc<Mutex<AppState>>>()
                    .and_then(|state| state.lock().ok().map(|guard| guard.config.lightweight_mode))
                    .unwrap_or(false);

                if main_window_close_behavior(lightweight_mode)
                    == MainWindowCloseBehavior::HideToTray
                {
                    let _ = window.hide();
                    let _ = window.app_handle().emit("main-window-visibility", false);
                    api.prevent_close();
                } else if let Some(lifecycle_state) =
                    window.try_state::<Arc<Mutex<AppLifecycleState>>>()
                {
                    let mut lifecycle_state =
                        lifecycle_state.lock().unwrap_or_else(|e| e.into_inner());
                    lifecycle_state.suppress_next_exit = true;
                }
            } else if let tauri::WindowEvent::Destroyed = event {
                sync_effective_dock_visibility(window.app_handle());
            }
        })
        .setup(|app| {
            if let Err(e) = autostart::init_autostart(app.handle()) {
                log::warn!("初始化开机自启功能失败: {e}");
            }

            let window = app
                .get_webview_window("main")
                .expect("main window should exist at setup");
            configure_main_window(&window);
            let launch_args = std::env::args().collect::<Vec<_>>();
            // 获取 Arc<Mutex<AppState>> 并克隆以便在异步任务中使用
            let state = app.state::<Arc<Mutex<AppState>>>();
            let should_hide_main_window = {
                let state_guard = state.inner().lock().unwrap_or_else(|e| e.into_inner());
                if state_guard.config.auto_start {
                    if let Err(e) = autostart::enable_autostart(
                        app.handle().clone(),
                        state_guard.config.auto_start_silent,
                    ) {
                        log::warn!("同步修复开机自启注册项失败: {e}");
                    }
                }
                let result = should_hide_main_window_on_setup(&state_guard.config, &launch_args);
                log::info!(
                    "启动窗口决策: show={} | auto_start={} auto_start_silent={} args={:?}",
                    !result,
                    state_guard.config.auto_start,
                    state_guard.config.auto_start_silent,
                    launch_args,
                );
                result
            };

            if should_hide_main_window {
                let _ = window.hide();
                let _ = app.emit("main-window-visibility", false);
            } else {
                let _ = window.show();
                let _ = app.emit("main-window-visibility", true);
            }

            let state_clone = state.inner().clone();
            let state_clone2 = state.inner().clone();
            let state_clone3 = state.inner().clone();
            let state_clone4 = state.inner().clone();
            let state_clone5 = state.inner().clone();
            let state_clone6 = state.inner().clone();
            let state_for_tray = state.inner().clone();
            let app_handle = app.handle().clone();
            let screenshot_app_handle = app.handle().clone();

            let (
                avatar_enabled,
                avatar_scale,
                avatar_body_hidden,
                avatar_position,
                avatar_state,
                config_load_status,
            ) = {
                let state_guard = state.inner().lock().unwrap_or_else(|e| e.into_inner());
                (
                    state_guard.config.avatar_enabled,
                    state_guard.config.avatar_scale,
                    state_guard.config.avatar_body_hidden,
                    state_guard.config.avatar_x.zip(state_guard.config.avatar_y),
                    state_guard.avatar_state.clone(),
                    state_guard.config_load_status,
                )
            };

            if let Err(e) = avatar_engine::sync_avatar_window(
                app.handle(),
                avatar_enabled,
                avatar_scale,
                avatar_position,
                false,
                avatar_body_hidden,
            ) {
                log::warn!("初始化桌宠窗口失败: {e}");
            } else if avatar_enabled {
                avatar_engine::emit_avatar_state(app.handle(), &avatar_state);
            }

            // 初始化智能穿透运行时 flag（从启动 config，供 input bridge 轮询无锁读）。
            // 配置损坏时不得注册全局键鼠监听，以免故障安全启动仍采集输入事件。
            if should_initialize_avatar_input(config_load_status) {
                if let Ok(s) = state.inner().lock() {
                    avatar_input::set_avatar_enabled_flag(s.config.avatar_enabled);
                    avatar_input::set_avatar_click_through_flag(s.config.avatar_click_through);
                }
                avatar_input::start_avatar_input_monitor(app.handle());
                avatar_input::spawn_avatar_input_monitor_retry(app.handle().clone());
                avatar_input::spawn_avatar_input_bridge(app.handle().clone());
            } else {
                avatar_input::set_avatar_enabled_flag(false);
                avatar_input::set_avatar_click_through_flag(false);
                log::warn!("配置损坏，已跳过全局键鼠监听初始化");
            }

            if let Err(e) = localhost_api::sync_localhost_api_runtime(app.handle(), state.inner()) {
                log::warn!("初始化本地 API 失败: {e}");
            }
            if let Err(e) = telegram_bot::sync_telegram_bot_runtime(state.inner()) {
                log::warn!("初始化 Telegram Bot 失败: {e}");
            }

            // 创建 Tauri v2 系统托盘
            let tray_locale = state
                .inner()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .config
                .locale
                .clone();
            let show =
                MenuItemBuilder::with_id(TRAY_MENU_SHOW_ID, tray_label("show", &tray_locale))
                    .build(app)?;
            let recording_toggle = MenuItemBuilder::with_id(
                TRAY_MENU_RECORDING_TOGGLE_ID,
                tray_recording_toggle_label(true, false, &tray_locale),
            )
            .build(app)?;
            let lightweight_mode = CheckMenuItemBuilder::with_id(
                TRAY_MENU_LIGHTWEIGHT_MODE_ID,
                tray_label("lightweight", &tray_locale),
            )
            .checked(false)
            .build(app)?;
            let avatar_toggle = CheckMenuItemBuilder::with_id(
                TRAY_MENU_AVATAR_TOGGLE_ID,
                tray_label("avatar", &tray_locale),
            )
            .checked(avatar_enabled)
            .build(app)?;
            let quit =
                MenuItemBuilder::with_id(TRAY_MENU_QUIT_ID, tray_label("quit", &tray_locale))
                    .build(app)?;

            let menu = MenuBuilder::new(app)
                .item(&show)
                .separator()
                .item(&recording_toggle)
                .item(&lightweight_mode)
                .item(&avatar_toggle)
                .separator()
                .item(&quit)
                .build()?;

            app.manage(TrayMenuState {
                show: show.clone(),
                recording_toggle: recording_toggle.clone(),
                lightweight_mode: lightweight_mode.clone(),
                avatar_toggle: avatar_toggle.clone(),
                quit: quit.clone(),
            });
            refresh_tray_menu(app.handle());

            let tray_icon = build_tray_icon(app);
            let tray_builder = TrayIconBuilder::new().icon(tray_icon).menu(&menu);

            #[cfg(target_os = "macos")]
            let tray_builder = tray_builder.icon_as_template(true);

            let _tray = tray_builder
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    TRAY_MENU_QUIT_ID => {
                        if let Some(lifecycle_state) =
                            app.try_state::<Arc<Mutex<AppLifecycleState>>>()
                        {
                            let mut lifecycle_state =
                                lifecycle_state.lock().unwrap_or_else(|e| e.into_inner());
                            lifecycle_state.explicit_quit_requested = true;
                        }
                        app.exit(0);
                    }
                    TRAY_MENU_SHOW_ID => {
                        if let Err(e) = reveal_main_window(&app.clone(), None) {
                            log::warn!("从托盘恢复主窗口失败: {e}");
                        }
                    }
                    TRAY_MENU_RECORDING_TOGGLE_ID => {
                        {
                            let mut state =
                                state_for_tray.lock().unwrap_or_else(|e| e.into_inner());
                            let action =
                                tray_recording_toggle_action(state.is_recording, state.is_paused);
                            match action {
                                RecordingToggleAction::Start => {
                                    state.is_recording = true;
                                    state.is_paused = false;
                                    log::info!("托盘操作：开始录制");
                                }
                                RecordingToggleAction::Pause => {
                                    state.is_paused = true;
                                    log::info!("托盘操作：暂停录制");
                                }
                                RecordingToggleAction::Resume => {
                                    state.is_paused = false;
                                    log::info!("托盘操作：恢复录制");
                                }
                            }
                        }
                        emit_recording_state_changed(app);
                    }
                    TRAY_MENU_LIGHTWEIGHT_MODE_ID => {
                        let next_config = {
                            let state = state_for_tray.lock().unwrap_or_else(|e| e.into_inner());
                            let mut config = state.config.clone();
                            config.lightweight_mode = !config.lightweight_mode;
                            config
                        };

                        if let Err(e) =
                            commands::persist_app_config(next_config, app.clone(), &state_for_tray)
                        {
                            log::warn!("从托盘切换轻量模式失败: {e}");
                            refresh_tray_menu(app);
                        }
                    }
                    TRAY_MENU_AVATAR_TOGGLE_ID => {
                        let next_config = {
                            let state = state_for_tray.lock().unwrap_or_else(|e| e.into_inner());
                            let mut config = state.config.clone();
                            config.avatar_enabled = !config.avatar_enabled;
                            config
                        };

                        if let Err(e) =
                            commands::persist_app_config(next_config, app.clone(), &state_for_tray)
                        {
                            log::warn!("从托盘切换桌宠失败: {e}");
                            refresh_tray_menu(app);
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(move |_tray, event| {
                    // 处理托盘图标点击
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app_handle = _tray.app_handle();
                        if let Err(e) = reveal_main_window(app_handle, None) {
                            log::warn!("点击托盘恢复主窗口失败: {e}");
                        }
                    }
                })
                .build(app)?;

            // 启动后台截屏任务
            tauri::async_runtime::spawn(async move {
                background_screenshot_task(state_clone, screenshot_app_handle).await;
            });

            tauri::async_runtime::spawn(async move {
                background_avatar_task(state_clone3, app_handle).await;
            });

            // 启动小时摘要生成任务（每小时检查一次）
            tauri::async_runtime::spawn(async move {
                hourly_summary_task(state_clone2).await;
            });

            // 启动远程截图补传任务（每 10 分钟扫描一次，兜底断网期间漏传的截图）
            tauri::async_runtime::spawn(async move {
                remote_upload_backfill_task(state_clone4).await;
            });

            // 启动存储清理周期任务（每 6 小时，保留策略不再只在启动时生效一次）
            tauri::async_runtime::spawn(async move {
                storage_cleanup_task(state_clone5).await;
            });

            // 启动实体分类学习任务（每 30 分钟，自动归类未知应用/域名并回溯修正）
            tauri::async_runtime::spawn(async move {
                entity_classify_task(state_clone6).await;
            });

            sync_effective_dock_visibility(app.handle());

            // 保存 AppHandle 到全局变量，用于从 macOS Dock 点击恢复窗口
            let _ = APP_HANDLE.set(app.handle().clone());

            // 注: macOS Dock 点击恢复窗口通过系统托盘 LeftClick 事件处理
            // 用户需要点击状态栏的系统托盘图标来恢复窗口

            log::info!("应用初始化完成");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            autostart::enable_autostart,
            autostart::disable_autostart,
            autostart::is_autostart_enabled,
            commands::get_today_stats,
            commands::get_overview_stats,
            commands::get_overview_domains,
            commands::get_overview_domain_detail,
            commands::get_range_daily_totals,
            commands::get_daily_stats,
            commands::get_timeline,
            commands::get_hourly_app_breakdown,
            commands::generate_report,
            commands::get_saved_report,
            commands::update_report_content,
            commands::set_report_block_preference,
            commands::export_report_markdown,
            commands::export_timeline_json,
            commands::export_reports_range,
            commands::get_localhost_api_status,
            commands::get_node_gateway_status,
            commands::get_telegram_bot_status,
            commands::generate_telegram_bot_bind_code,
            commands::generate_text_with_model,
            commands::reveal_localhost_api_token,
            commands::rotate_localhost_api_token,
            commands::get_config,
            commands::save_config,
            commands::get_update_settings,
            commands::save_update_settings,
            commands::should_check_updates,
            commands::update_last_check_time,
            commands::pause_recording,
            commands::resume_recording,
            commands::get_recording_state,
            commands::get_avatar_state,
            commands::save_avatar_position,
            commands::persist_avatar_position,
            commands::set_avatar_window_expanded,
            commands::set_avatar_interactive_regions,
            commands::get_data_dir,
            commands::get_default_data_dir,
            commands::get_runtime_platform,
            commands::get_linux_session_support,
            commands::install_gnome_avatar_extension,
            commands::change_data_dir,
            commands::cleanup_old_data_dir,
            commands::check_github_update,
            commands::download_and_install_github_update,
            commands::quit_app_for_update,
            commands::open_data_dir,
            commands::get_screenshot_thumbnail,
            commands::get_screenshot_full,
            commands::test_remote_storage,
            commands::test_model,
            commands::get_ai_providers,
            commands::fetch_models,
            commands::get_running_apps,
            commands::get_recent_apps,
            commands::set_app_category_rule,
            commands::set_domain_semantic_rule,
            commands::get_categories,
            commands::save_custom_category,
            commands::delete_custom_category,
            commands::get_semantic_categories,
            commands::save_custom_semantic_category,
            commands::delete_custom_semantic_category,
            commands::get_storage_stats,
            commands::get_hourly_summaries,
            commands::get_activity,
            commands::chat_work_assistant,
            commands::cancel_assistant_request,
            commands::confirm_assistant_action,
            commands::list_assistant_conversations,
            commands::create_assistant_conversation,
            commands::get_assistant_messages,
            commands::append_assistant_message,
            commands::delete_assistant_conversation,
            commands::list_user_memories,
            commands::create_user_memory,
            commands::update_user_memory,
            commands::delete_user_memory,
            commands::clear_user_memories,
            commands::index_semantic_memory,
            commands::semantic_memory_status,
            commands::test_embedding_model,
            commands::test_assistant_search,
            commands::synthesize_insights,
            commands::clear_old_activities,
            set_app_locale,
            commands::delete_activity,
            commands::delete_activities_by_date,
            commands::delete_activities_by_range,
            commands::delete_activities_by_app,
            commands::check_permissions,
            commands::open_permission_settings,
            commands::set_dock_visibility,
            commands::get_app_icon,
            commands::save_background_image,
            commands::get_background_image,
            commands::clear_background_image,
            commands::show_main_window,
            commands::handle_avatar_followup_action,
            get_platform,
        ])
        .build(tauri::generate_context!())
        .expect("构建 Tauri 应用时出错")
        .run(|_app_handle, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if let Some(lifecycle_state) =
                    _app_handle.try_state::<Arc<Mutex<AppLifecycleState>>>()
                {
                    let mut lifecycle_state =
                        lifecycle_state.lock().unwrap_or_else(|e| e.into_inner());
                    let should_prevent = should_prevent_exit(
                        lifecycle_state.suppress_next_exit,
                        lifecycle_state.explicit_quit_requested,
                    );
                    lifecycle_state.suppress_next_exit = false;

                    if should_prevent {
                        log::info!("拦截最后一个主窗口关闭导致的退出，保留后台与托盘");
                        api.prevent_exit();
                    }
                }
            }
            // 处理 macOS Dock 点击：显示隐藏的窗口（仅 macOS）
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } if !has_visible_windows => {
                if let Err(e) = reveal_main_window(&_app_handle.clone(), None) {
                    log::warn!("Dock 恢复主窗口失败: {e}");
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::{
        advance_break_reminder, avatar_activity_decision, avatar_monitor_poll_interval_ms,
        avatar_monitor_poll_interval_ms_for_platform, avatar_proactive_ai_should_run,
        avatar_transition_decision, browser_change_capture_min_interval_ms,
        browser_url_probe_attempted_after_full_lookup, describe_config_file_issue,
        duplicate_instance_should_stay_silent, effective_dock_visibility, initial_recording_state,
        launch_args_contain_autostart, main_window_close_behavior, monitoring_poll_interval_ms,
        monitoring_poll_interval_ms_for_platform, persist_previous_activity_backfill,
        previous_app_backfill_duration, record_avatar_window_switch, recording_loop_decision,
        resolve_activity_classification, resolve_browser_url_once, reusable_cached_active_window,
        screen_lock_check_interval_ms_for_platform, should_confirm_idle,
        should_emit_avatar_backlog_nudge, should_hide_main_window_on_setup,
        should_initialize_avatar_input, should_initialize_startup_permissions,
        should_merge_contiguous_activity, should_persist_merge_update, should_prevent_exit,
        should_probe_browser_url_before_change_detection, should_request_screen_capture_permission,
        should_run_startup_cleanup, should_skip_system_window, tray_recording_toggle_action,
        tray_recording_toggle_label, AvatarNudgeRuntime, BreakReminderRuntime, BreakReminderSignal,
        MainWindowCloseBehavior, RecordingToggleAction,
    };
    use crate::avatar_engine::{
        apply_avatar_visual_settings, default_avatar_state, derive_avatar_state,
    };
    use crate::config::{
        AiProvider, AppConfig, AvatarFollowupItem, ConfigLoadStatus, ModelConfig,
        WebsiteSemanticRule,
    };
    use crate::database::Database;
    use crate::monitor::ActiveWindow;
    use crate::privacy::PrivacyFilter;
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn temp_db_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("work-review-tauri-{name}-{unique}.db"))
    }

    #[test]
    fn 配置损坏时初始录制必须关闭并暂停() {
        assert_eq!(
            initial_recording_state(ConfigLoadStatus::Corrupted),
            (false, true)
        );
    }

    #[test]
    fn 正常首次启动和备份恢复的录制状态应按恢复策略区分() {
        assert_eq!(
            initial_recording_state(ConfigLoadStatus::Loaded),
            (true, false)
        );
        assert_eq!(
            initial_recording_state(ConfigLoadStatus::Missing),
            (true, false)
        );
        assert_eq!(
            initial_recording_state(ConfigLoadStatus::RecoveredFromBackup),
            (true, false)
        );
        assert!(!ConfigLoadStatus::RecoveredFromBackup.allows_automatic_save());
        assert!(!ConfigLoadStatus::Corrupted.allows_automatic_save());
    }

    #[test]
    fn 暂停录制时应重置截图计时器() {
        let decision = recording_loop_decision(true, true, 30);
        assert!(!decision.should_continue);
        assert!(decision.reset_capture_clock);
        assert_eq!(decision.screenshot_interval, 1);
    }

    #[test]
    fn 停止录制时应重置截图计时器() {
        let decision = recording_loop_decision(false, false, 30);
        assert!(!decision.should_continue);
        assert!(decision.reset_capture_clock);
        assert_eq!(decision.screenshot_interval, 1);
    }

    #[test]
    fn 正常录制时应保留截图间隔() {
        let decision = recording_loop_decision(true, false, 30);
        assert!(decision.should_continue);
        assert!(!decision.reset_capture_clock);
        assert_eq!(decision.screenshot_interval, 30);
    }

    #[test]
    fn 键鼠活跃时无论截图与否都不应判空闲() {
        assert!(!should_confirm_idle(false, 0, true, false));
        assert!(!should_confirm_idle(false, 0, false, true));
    }

    #[test]
    fn 关闭截图后应直接按输入空闲判断为空闲() {
        assert!(should_confirm_idle(true, 5 * 60, false, false));
        assert!(should_confirm_idle(true, 5 * 60, false, true));
    }

    #[test]
    fn 开启截图时键鼠超时不再因画面静止判空闲() {
        // 反转后的核心行为：创意类应用（PS/C4D）画面长时间不变，但键鼠超时
        // 不应直接判空闲 —— 用户可能在调色、思考、阅读 AI 回复。
        // 画面静止（true）或画面有变化（false）都不影响：未到硬超时一律保留时长。
        assert!(!should_confirm_idle(true, 5 * 60, true, true));
        assert!(!should_confirm_idle(true, 5 * 60, true, false));
        assert!(!should_confirm_idle(true, 9 * 60, true, true));
    }

    #[test]
    fn 达到十分钟硬超时后应强制切断空闲() {
        assert!(should_confirm_idle(true, 10 * 60, true, false));
        assert!(should_confirm_idle(true, 10 * 60, false, false));
        assert!(should_confirm_idle(true, 15 * 60, true, true));
        // 边界：差一秒未到硬超时，仍不判空闲
        assert!(!should_confirm_idle(true, 10 * 60 - 1, true, true));
    }

    #[test]
    fn 已进入输入空闲后切换应用不应回补上一应用时长() {
        assert_eq!(previous_app_backfill_duration(true, 3600, true, false), 0);
        assert_eq!(previous_app_backfill_duration(true, 3600, false, true), 0);
        assert_eq!(
            previous_app_backfill_duration(true, 3600, false, false),
            3600
        );
        assert_eq!(previous_app_backfill_duration(false, 3600, false, false), 0);
    }

    #[test]
    fn 零增量不应推进合并记录终点() {
        assert!(should_persist_merge_update(120));
        assert!(!should_persist_merge_update(0));
    }

    #[test]
    fn 只有未切换的连续活动可以合并() {
        assert!(should_merge_contiguous_activity(
            false, "Code", 1_500, 1_000
        ));
        assert!(!should_merge_contiguous_activity(
            true, "Code", 1_500, 1_000
        ));
        assert!(!should_merge_contiguous_activity(
            false, "Unknown", 1_500, 1_000
        ));
        assert!(!should_merge_contiguous_activity(
            false, "Code", 1_601, 1_000
        ));
        assert!(!should_merge_contiguous_activity(false, "Code", 999, 1_000));
    }

    #[test]
    fn 切换到新应用时上一应用即使没有历史记录也应回补轻量活动() {
        let db_path = temp_db_path("previous-backfill-new");
        let database = Database::new(&db_path).expect("创建测试数据库失败");
        let config = AppConfig::default();
        let privacy_filter = PrivacyFilter::from_config(&config.privacy);
        let current_timestamp = chrono::Local::now().timestamp();

        let inserted_id = persist_previous_activity_backfill(
            &database,
            &config,
            &privacy_filter,
            None,
            Some("cmux"),
            Some("npm run tauri dev"),
            None,
            8,
            current_timestamp,
            "Google Chrome",
        )
        .expect("没有历史记录的上一应用也应创建轻量记录");

        let activity = database
            .get_activity_by_id(inserted_id)
            .expect("查询回补活动失败")
            .expect("回补活动应已写入数据库");

        assert_eq!(activity.app_name, "cmux");
        assert_eq!(activity.window_title, "npm run tauri dev");
        assert_eq!(activity.duration, 8);
        assert_eq!(activity.timestamp, current_timestamp);
        assert!(activity.screenshot_path.is_empty());
    }

    #[test]
    fn 当前平台主监控轮询间隔应匹配平台策略() {
        assert_eq!(
            monitoring_poll_interval_ms(),
            monitoring_poll_interval_ms_for_platform(cfg!(target_os = "macos"))
        );
    }

    #[test]
    fn 当前平台桌宠独立轮询间隔应匹配平台策略() {
        assert_eq!(
            avatar_monitor_poll_interval_ms(),
            avatar_monitor_poll_interval_ms_for_platform(cfg!(target_os = "macos"), true)
        );
    }

    #[test]
    fn 域名语义规则应覆盖浏览器活动默认分类() {
        let mut config = AppConfig::default();
        config.website_semantic_rules = vec![WebsiteSemanticRule {
            domain: "github.com".to_string(),
            semantic_category: "任务规划".to_string(),
        }];
        config.normalize();

        let classification = resolve_activity_classification(
            &config,
            "Google Chrome",
            "Issue #28",
            Some("https://github.com/issues/28"),
        );

        assert_eq!(classification.base_category, "browser");
        assert_eq!(classification.semantic_category, "任务规划");
    }

    #[test]
    fn 非mac主监控轮询间隔应保持半秒() {
        assert_eq!(monitoring_poll_interval_ms_for_platform(false), 500);
    }

    #[test]
    fn 非mac桌宠活跃轮询间隔应压到一百八十毫秒() {
        assert_eq!(
            avatar_monitor_poll_interval_ms_for_platform(false, true),
            180
        );
    }

    #[test]
    fn mac主监控轮询间隔应降频() {
        assert_eq!(monitoring_poll_interval_ms_for_platform(true), 1500);
    }

    #[test]
    fn 同标题浏览器页应在切换判定前主动探测真实网址() {
        assert!(should_probe_browser_url_before_change_detection(
            "Google Chrome",
            "项目文档",
            Some("Google Chrome"),
            Some("项目文档"),
            None,
        ));
        assert!(!should_probe_browser_url_before_change_detection(
            "Google Chrome",
            "项目文档",
            Some("Google Chrome"),
            Some("另一个标签页"),
            None,
        ));
        assert!(!should_probe_browser_url_before_change_detection(
            "Cursor",
            "main.rs",
            Some("Cursor"),
            Some("main.rs"),
            None,
        ));
    }

    #[test]
    fn 首次遇到浏览器窗口时应探测url() {
        assert!(should_probe_browser_url_before_change_detection(
            "Google Chrome",
            "项目文档",
            None,
            None,
            None,
        ));
        assert!(!should_probe_browser_url_before_change_detection(
            "Google Chrome",
            "项目文档",
            None,
            None,
            Some("https://example.com"),
        ));
    }

    #[test]
    fn 完整窗口查询后本轮后续阶段都不应再次查询url() {
        let mut calls = 1;
        let mut attempted = browser_url_probe_attempted_after_full_lookup(true, true);

        let first = resolve_browser_url_once(&mut attempted, true, || {
            calls += 1;
            None
        });
        let second = resolve_browser_url_once(&mut attempted, true, || {
            calls += 1;
            None
        });

        assert_eq!(calls, 1);
        assert!(first.is_none());
        assert!(second.is_none());
    }

    #[test]
    fn 变化检测领取查询后落库刷新不能再次领取() {
        let mut attempted = false;

        let mut calls = 0;
        resolve_browser_url_once(&mut attempted, true, || {
            calls += 1;
            None
        });
        resolve_browser_url_once(&mut attempted, true, || {
            calls += 1;
            None
        });

        assert_eq!(calls, 1);
    }

    #[test]
    fn 浏览器url查询失败也应消耗本轮预算() {
        let mut attempted = false;
        let mut calls = 0;

        let resolved_url = resolve_browser_url_once(&mut attempted, true, || {
            calls += 1;
            None
        });
        let retried_url = resolve_browser_url_once(&mut attempted, true, || {
            calls += 1;
            Some("https://example.com".to_string())
        });

        assert_eq!(calls, 1);
        assert!(resolved_url.is_none());
        assert!(retried_url.is_none());
    }

    #[test]
    fn 变化检测无需查询时落库前仍可使用唯一预算() {
        let mut attempted = false;

        let mut calls = 0;
        resolve_browser_url_once(&mut attempted, false, || {
            calls += 1;
            None
        });
        resolve_browser_url_once(&mut attempted, true, || {
            calls += 1;
            None
        });
        resolve_browser_url_once(&mut attempted, true, || {
            calls += 1;
            None
        });

        assert_eq!(calls, 1);
    }

    #[test]
    fn 完整窗口查询是否探测url不应依赖查询结果() {
        assert!(browser_url_probe_attempted_after_full_lookup(true, true));
        assert!(!browser_url_probe_attempted_after_full_lookup(false, true));
        assert!(!browser_url_probe_attempted_after_full_lookup(true, false));
    }

    #[test]
    fn 浏览器导航变化应使用更短的截图冷却() {
        assert_eq!(
            browser_change_capture_min_interval_ms("Google Chrome", true, false),
            1200
        );
        assert_eq!(
            browser_change_capture_min_interval_ms("Google Chrome", false, true),
            1200
        );
        assert_eq!(
            browser_change_capture_min_interval_ms("Google Chrome", false, false),
            3000
        );
        assert_eq!(
            browser_change_capture_min_interval_ms("Cursor", true, false),
            3000
        );
    }

    #[test]
    fn mac桌宠活跃轮询间隔应降频() {
        assert_eq!(
            avatar_monitor_poll_interval_ms_for_platform(true, true),
            750
        );
    }

    #[test]
    fn mac桌宠空闲轮询间隔应进一步降频() {
        assert_eq!(
            avatar_monitor_poll_interval_ms_for_platform(true, false),
            2000
        );
    }

    #[test]
    fn mac锁屏检测轮询间隔应显著降频() {
        assert_eq!(screen_lock_check_interval_ms_for_platform(true), 5000);
    }

    #[test]
    fn 新鲜的活动窗口缓存应被截图循环复用() {
        let now = Instant::now();
        let cached_window = ActiveWindow {
            app_name: "Cursor".to_string(),
            window_title: "main.rs".to_string(),
            browser_url: None,
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
        };

        let reused = reusable_cached_active_window(Some(&(now, cached_window.clone())), now);

        assert!(reused.is_some());
        let reused = reused.expect("fresh cache should be reused");
        assert_eq!(reused.app_name, cached_window.app_name);
        assert_eq!(reused.window_title, cached_window.window_title);
    }

    #[test]
    fn 过期的活动窗口缓存不应被截图循环复用() {
        let now = Instant::now();
        let cached_window = ActiveWindow {
            app_name: "Cursor".to_string(),
            window_title: "main.rs".to_string(),
            browser_url: None,
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
        };
        let stale_at = now
            .checked_sub(Duration::from_millis(1500))
            .expect("stale timestamp should be valid");

        let reused = reusable_cached_active_window(Some(&(stale_at, cached_window)), now);

        assert!(reused.is_none());
    }

    #[test]
    fn 暂停录制时桌宠应回到待命状态() {
        let decision =
            avatar_activity_decision(true, true, true, 0.82, "keyboard-focus", "assistant", false);

        assert!(!decision.should_continue);
        assert_eq!(
            decision.reset_state,
            Some(apply_avatar_visual_settings(
                default_avatar_state(),
                0.82,
                "keyboard-focus",
                "assistant",
                false,
            ))
        );
    }

    #[test]
    fn 停止录制时桌宠应回到待命状态() {
        let decision = avatar_activity_decision(
            true,
            false,
            false,
            0.82,
            "minimal-office",
            "assistant",
            false,
        );

        assert!(!decision.should_continue);
        assert_eq!(
            decision.reset_state,
            Some(apply_avatar_visual_settings(
                default_avatar_state(),
                0.82,
                "minimal-office",
                "assistant",
                false,
            ))
        );
    }

    #[test]
    fn 桌宠模型生成提醒必须显式开启才会调用文本模型() {
        let model = ModelConfig {
            provider: AiProvider::Ollama,
            endpoint: "http://localhost:11434".to_string(),
            api_key: None,
            model: "qwen3".to_string(),
        };

        assert!(avatar_proactive_ai_should_run(
            true, true, false, &model, 1_000, 1_000
        ));
        assert!(!avatar_proactive_ai_should_run(
            true, false, false, &model, 1_000, 1_000
        ));
        assert!(!avatar_proactive_ai_should_run(
            false, true, false, &model, 1_000, 1_000
        ));
        assert!(!avatar_proactive_ai_should_run(
            true, true, true, &model, 1_000, 1_000
        ));
        assert!(!avatar_proactive_ai_should_run(
            true, true, false, &model, 999, 1_000
        ));

        let empty_model = ModelConfig {
            model: String::new(),
            ..model
        };

        assert!(!avatar_proactive_ai_should_run(
            true,
            true,
            false,
            &empty_model,
            1_000,
            1_000
        ));
    }

    #[test]
    fn 模式首次波动时不应立刻切换桌宠状态() {
        let current = derive_avatar_state("Cursor", "main.rs", None, false, false);
        let candidate = derive_avatar_state("Google Chrome", "产品文档 - docs", None, false, false);

        let decision = avatar_transition_decision(Some(&current), None, 0, &candidate);

        assert_eq!(decision.emit_state, None);
        assert_eq!(decision.pending_state, Some(candidate));
        assert_eq!(decision.pending_hits, 1);
    }

    #[test]
    fn 模式连续两次命中后才应切换桌宠状态() {
        let current = derive_avatar_state("Cursor", "main.rs", None, false, false);
        let candidate = derive_avatar_state("Google Chrome", "产品文档 - docs", None, false, false);

        let decision = avatar_transition_decision(Some(&current), Some(&candidate), 1, &candidate);

        assert_eq!(decision.emit_state, Some(candidate));
        assert_eq!(decision.pending_state, None);
        assert_eq!(decision.pending_hits, 0);
    }

    #[test]
    fn 轻量模式关闭时主窗口关闭按钮应改为隐藏() {
        assert_eq!(
            main_window_close_behavior(false),
            MainWindowCloseBehavior::HideToTray
        );
    }

    #[test]
    fn 轻量模式开启时主窗口关闭按钮应允许真正关闭() {
        assert_eq!(
            main_window_close_behavior(true),
            MainWindowCloseBehavior::CloseWindow
        );
    }

    #[test]
    fn dock可见性应同时考虑用户偏好轻量模式与主窗口是否存在() {
        assert!(!effective_dock_visibility(true, false, true));
        assert!(effective_dock_visibility(false, false, true));
        assert!(effective_dock_visibility(false, true, true));
        assert!(!effective_dock_visibility(false, true, false));
    }

    #[test]
    fn 托盘录制按钮应根据当前状态切换动作() {
        assert_eq!(
            tray_recording_toggle_action(false, false),
            RecordingToggleAction::Start
        );
        assert_eq!(
            tray_recording_toggle_action(true, false),
            RecordingToggleAction::Pause
        );
        assert_eq!(
            tray_recording_toggle_action(true, true),
            RecordingToggleAction::Resume
        );
    }

    #[test]
    fn 托盘录制按钮文案应与状态一致() {
        assert_eq!(
            tray_recording_toggle_label(false, false, "zh-CN"),
            "开始录制"
        );
        assert_eq!(
            tray_recording_toggle_label(true, false, "zh-CN"),
            "暂停录制"
        );
        assert_eq!(tray_recording_toggle_label(true, true, "zh-CN"), "恢复录制");
    }

    #[test]
    fn 仅应拦截主窗口关闭导致的被动退出() {
        assert!(should_prevent_exit(true, false));
        assert!(!should_prevent_exit(false, false));
        assert!(!should_prevent_exit(true, true));
    }

    #[test]
    #[cfg(not(windows))]
    fn 非windows上应优先使用_autostart_或_hidden_参数并结合配置判定隐藏() {
        let mut config = AppConfig::default();
        config.auto_start = true;
        config.auto_start_silent = true;

        assert!(should_hide_main_window_on_setup(
            &config,
            &["work-review".to_string(), "--autostart".to_string()]
        ));
        assert!(!should_hide_main_window_on_setup(
            &config,
            &["work-review".to_string()]
        ));
        assert!(should_hide_main_window_on_setup(
            &config,
            &["work-review".to_string(), "--hidden".to_string()]
        ));
        assert!(should_hide_main_window_on_setup(
            &config,
            &["work-review".to_string(), "--minimized".to_string()]
        ));

        let mut stale_enabled_config = AppConfig::default();
        stale_enabled_config.auto_start = false;
        stale_enabled_config.auto_start_silent = true;
        assert!(should_hide_main_window_on_setup(
            &stale_enabled_config,
            &["work-review".to_string(), "--autostart".to_string()]
        ));

        let mut visible_config = AppConfig::default();
        visible_config.auto_start = false;
        visible_config.auto_start_silent = false;
        assert!(!should_hide_main_window_on_setup(
            &visible_config,
            &["work-review".to_string(), "--hidden".to_string()]
        ));
    }

    #[test]
    #[cfg(windows)]
    fn windows上应仅凭_launch_args_里显式的_hidden_决定是否隐藏() {
        // 注册表参数由 silent 选择动态写入，显隐决策不再依赖 config，消除失同步翻车。
        let mut silent_config = AppConfig::default();
        silent_config.auto_start = true;
        silent_config.auto_start_silent = true;

        // silent 模式注册表 → --autostart --hidden → 隐藏
        assert!(should_hide_main_window_on_setup(
            &silent_config,
            &[
                "work-review".to_string(),
                "--autostart".to_string(),
                "--hidden".to_string(),
            ]
        ));
        // show 模式注册表 → 只有 --autostart → 显示
        assert!(!should_hide_main_window_on_setup(
            &silent_config,
            &["work-review".to_string(), "--autostart".to_string()]
        ));
        // 普通手动打开 → 显示
        assert!(!should_hide_main_window_on_setup(
            &silent_config,
            &["work-review".to_string()]
        ));

        // 即使 config 还没保存用户选择（失同步），注册表里的 --hidden 依然能强制隐藏
        let mut stale_config = AppConfig::default();
        stale_config.auto_start = false;
        stale_config.auto_start_silent = false;
        assert!(should_hide_main_window_on_setup(
            &stale_config,
            &["work-review".to_string(), "--hidden".to_string()]
        ));
        assert!(should_hide_main_window_on_setup(
            &stale_config,
            &["work-review".to_string(), "--minimized".to_string()]
        ));
    }

    #[test]
    fn 自启动参数判定应精确匹配_autostart() {
        assert!(launch_args_contain_autostart(&[
            "work-review".to_string(),
            "--autostart".to_string()
        ]));
        assert!(!launch_args_contain_autostart(&[
            "work-review".to_string(),
            "--autostarted".to_string()
        ]));
    }

    #[test]
    fn 重复实例遇到自启或隐藏参数时应保持静默() {
        assert!(duplicate_instance_should_stay_silent(&[
            "work-review".to_string(),
            "--autostart".to_string()
        ]));
        assert!(duplicate_instance_should_stay_silent(&[
            "work-review".to_string(),
            "--hidden".to_string()
        ]));
        assert!(duplicate_instance_should_stay_silent(&[
            "work-review".to_string(),
            "--minimized".to_string()
        ]));
        assert!(!duplicate_instance_should_stay_silent(&[
            "work-review".to_string(),
            "--autostarted".to_string()
        ]));
    }

    #[test]
    fn 任务管理器未响应时仍应识别为系统窗口() {
        let active_window = ActiveWindow {
            app_name: "任务管理器 (未响应)".to_string(),
            window_title: "任务管理器 (未响应)".to_string(),
            browser_url: None,
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
        };

        assert!(should_skip_system_window(&active_window));
    }

    #[test]
    fn uac提示窗口应识别为系统窗口() {
        let active_window = ActiveWindow {
            app_name: "consent.exe".to_string(),
            window_title: "用户账户控制".to_string(),
            browser_url: None,
            executable_path: Some(r"C:\Windows\System32\consent.exe".to_string()),
            window_bounds: None,
            is_minimized: false,
        };

        assert!(should_skip_system_window(&active_window));
    }

    #[test]
    fn windows_security提示窗口应识别为系统窗口() {
        let active_window = ActiveWindow {
            app_name: "Windows Security".to_string(),
            window_title: "Windows Security".to_string(),
            browser_url: None,
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
        };

        assert!(should_skip_system_window(&active_window));
    }

    #[test]
    fn windows最小化前台窗口应视为非工作窗口() {
        let active_window = ActiveWindow {
            app_name: "WeChat".to_string(),
            window_title: "微信".to_string(),
            browser_url: None,
            executable_path: None,
            window_bounds: None,
            is_minimized: true,
        };

        assert!(should_skip_system_window(&active_window));
    }

    #[test]
    fn work_review自身窗口不应被当成系统窗口跳过() {
        let active_window = ActiveWindow {
            app_name: "Work Review".to_string(),
            window_title: "时间线".to_string(),
            browser_url: None,
            executable_path: Some(
                "/Applications/Work Review.app/Contents/MacOS/work-review".to_string(),
            ),
            window_bounds: None,
            is_minimized: false,
        };

        assert!(!should_skip_system_window(&active_window));
    }

    #[test]
    fn 普通应用标题提到任务管理器时不应被误判为系统窗口() {
        let active_window = ActiveWindow {
            app_name: "任务管理器实现说明".to_string(),
            window_title: "任务管理器实现说明".to_string(),
            browser_url: None,
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
        };

        assert!(!should_skip_system_window(&active_window));
    }

    #[test]
    fn macos录屏权限已提示过时不应重复请求() {
        assert!(should_request_screen_capture_permission(false, false));
        assert!(!should_request_screen_capture_permission(false, true));
        assert!(!should_request_screen_capture_permission(true, false));
    }

    #[test]
    fn 配置损坏时应跳过启动权限初始化() {
        assert!(!should_initialize_startup_permissions(
            ConfigLoadStatus::Corrupted
        ));
        assert!(should_initialize_startup_permissions(
            ConfigLoadStatus::Loaded
        ));
        assert!(should_initialize_startup_permissions(
            ConfigLoadStatus::Missing
        ));
        assert!(should_initialize_startup_permissions(
            ConfigLoadStatus::RecoveredFromBackup
        ));
    }

    #[test]
    fn 配置损坏时应跳过启动存储清理() {
        assert!(!should_run_startup_cleanup(ConfigLoadStatus::Corrupted));
        assert!(should_run_startup_cleanup(ConfigLoadStatus::Loaded));
        assert!(should_run_startup_cleanup(ConfigLoadStatus::Missing));
        assert!(should_run_startup_cleanup(
            ConfigLoadStatus::RecoveredFromBackup
        ));
    }

    #[test]
    fn 配置损坏时应跳过键鼠采集() {
        assert!(!should_initialize_avatar_input(ConfigLoadStatus::Corrupted));

        for status in [
            ConfigLoadStatus::Loaded,
            ConfigLoadStatus::Missing,
            ConfigLoadStatus::RecoveredFromBackup,
        ] {
            assert!(should_initialize_avatar_input(status));
        }
    }

    #[test]
    fn 配置文件问题描述应区分不存在与读取解析失败() {
        let path = std::path::Path::new("/tmp/work-review-config.json");

        assert_eq!(
            describe_config_file_issue(path, None),
            "/tmp/work-review-config.json 不存在"
        );
        assert_eq!(
            describe_config_file_issue(path, Some("JSON 语法错误")),
            "/tmp/work-review-config.json 读取或解析失败: JSON 语法错误"
        );
    }

    #[test]
    fn 休息提醒首次达到阈值时应触发一次() {
        let mut state = BreakReminderRuntime::new();

        let first =
            advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(49));
        let second =
            advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(1));

        assert!(!first.should_emit);
        assert!(second.should_emit);
        assert!(second
            .payload
            .as_ref()
            .is_some_and(|payload| payload.persistent));
    }

    #[test]
    fn 休息提醒应在五分钟缓冲后重新开始下一轮计时() {
        let mut state = BreakReminderRuntime::new();

        let first =
            advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(50));
        let cooldown =
            advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(5));
        let next_round =
            advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(50));

        assert!(first.should_emit);
        assert!(!cooldown.should_emit);
        assert!(next_round.should_emit);
    }

    #[test]
    fn 手动关闭提醒不应打断下一轮计时() {
        let mut state = BreakReminderRuntime::new();

        let _ = advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(50));
        let dismiss = advance_break_reminder(&mut state, true, 50, BreakReminderSignal::Dismiss);
        let _ = advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(5));
        let next_round =
            advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(50));

        assert!(dismiss.should_clear);
        assert!(next_round.should_emit);
    }

    #[test]
    fn 关闭休息提醒时应立即清除当前气泡并停止计时() {
        let mut state = BreakReminderRuntime::new();
        let _ = advance_break_reminder(&mut state, true, 50, BreakReminderSignal::TickMinutes(50));

        let disabled =
            advance_break_reminder(&mut state, false, 50, BreakReminderSignal::TickMinutes(1));

        assert!(disabled.should_clear);
        assert!(!disabled.should_emit);
    }

    #[test]
    fn 短时间频繁切换窗口时应触发主动提醒() {
        let mut runtime = AvatarNudgeRuntime::default();

        // 需要 8 次切换才触发（阈值从 4 调整为 8）
        assert!(!record_avatar_window_switch(&mut runtime, 1_000));
        assert!(!record_avatar_window_switch(&mut runtime, 20_000));
        assert!(!record_avatar_window_switch(&mut runtime, 40_000));
        assert!(!record_avatar_window_switch(&mut runtime, 60_000));
        assert!(!record_avatar_window_switch(&mut runtime, 80_000));
        assert!(!record_avatar_window_switch(&mut runtime, 100_000));
        assert!(!record_avatar_window_switch(&mut runtime, 120_000));
        assert!(record_avatar_window_switch(&mut runtime, 140_000));
        // 冷却期内不再触发
        assert!(!record_avatar_window_switch(&mut runtime, 145_000));
    }

    #[test]
    fn 待跟进堆积一段时间后应触发主动提醒() {
        let mut runtime = AvatarNudgeRuntime::default();
        let followups = vec![AvatarFollowupItem {
            id: "1".to_string(),
            title: "支付回调".to_string(),
            date: "2024-03-09".to_string(),
            source_app: "Cursor".to_string(),
            source_title: "payments.ts".to_string(),
            project_key: "cursor::payments".to_string(),
            created_at: 1_710_000_000,
            status: "open".to_string(),
        }];

        assert_eq!(
            should_emit_avatar_backlog_nudge(&mut runtime, &followups, 1_710_003_000, 30_000),
            Some(1)
        );
        assert_eq!(
            should_emit_avatar_backlog_nudge(&mut runtime, &followups, 1_710_003_100, 31_000),
            None
        );
    }
}
