// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// macOS: objc 宏（msg_send!, class! 等）需要 macro_use 全局导入
#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

mod activity_clock;
mod agent;
mod autostart;
mod avatar_engine;
mod avatar_followup;
#[allow(dead_code)]
mod avatar_input;
mod avatar_proactive;
#[cfg(target_os = "windows")]
mod avatar_transparency;
mod bootstrap;
mod bot_common;
mod break_reminder;
mod commands;
mod config_sync;
mod crash;
mod data_dir;
mod dingtalk_bot;
mod feishu_bot;
mod idle_detector;
mod linux_session;
mod localhost_api;
mod monitor;
mod navigation_guard;
mod node_gateway;
mod ocr;
mod recording;
mod remote_upload;
mod screen_lock;
mod screenshot;
mod screenshot_mask;
mod storage;
mod tasks;
mod telegram_bot;
mod tray;
mod wecom_aibot;
mod wecom_bot;
mod window;

pub(crate) use bootstrap::*;
#[cfg(test)]
use break_reminder::*;
pub(crate) use crash::*;
pub(crate) use data_dir::{default_data_dir, save_data_dir_preference};
pub(crate) use recording::*;
pub(crate) use tasks::*;
pub(crate) use tray::*;
pub(crate) use window::*;

use once_cell::sync::OnceCell;
use screenshot::{ScreenshotResult, ScreenshotService};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use storage::StorageManager;
use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use work_review_core::config::{config_backup_path, AppConfig, ConfigLoadStatus};
use work_review_core::database::Database;
use work_review_core::privacy::PrivacyFilter;

// 全局 AppHandle，供原生系统回调恢复窗口。
pub(crate) static APP_HANDLE: OnceCell<AppHandle> = OnceCell::new();
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";
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
    pub wecom_bot_runtime: wecom_aibot::WecomBotRuntime,
    /// avatar 循环缓存的活动窗口（时间戳 + 窗口信息），供 screenshot 循环复用
    pub cached_active_window: Option<(std::time::Instant, monitor::ActiveWindow)>,
}

#[tokio::main]
async fn main() {
    // 安装崩溃捕获（最早执行，确保后续任何 panic 都能记录调用栈）
    install_crash_handler();

    // 初始化日志
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // 必须在 GTK/Tauri 或采集线程首次使用 Xlib 前初始化其全局锁。
    #[cfg(target_os = "linux")]
    if unsafe { x11::xlib::XInitThreads() } == 0 {
        log::error!("Xlib 多线程初始化失败，无法安全启动桌面采集");
        std::process::exit(1);
    }

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
    let data_dir = match data_dir::resolve_data_dir() {
        Ok(directory) => directory,
        Err(error) => {
            data_dir::report_startup_error(&error);
            std::process::exit(1);
        }
    };
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

    // 初始化数据库（失败时展示友好提示框而非直接 panic 闪退）
    let db_path = data_dir.join("workreview.db");
    let database = match Database::new(&db_path) {
        Ok(db) => db,
        Err(error) => {
            let startup_err = std::io::Error::other(format!(
                "初始化数据库失败（{}）：{error}。请检查磁盘空间、文件权限或是否有其他进程占用数据库文件后重试。",
                db_path.display()
            ));
            data_dir::report_startup_error(&startup_err);
            std::process::exit(1);
        }
    };

    // 版本变化检测（首次启动同样视为变化）：FTS 重建移到窗口启动后的后台任务执行，不阻塞首屏。
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let is_version_changed = config.last_app_version.as_deref() != Some(&current_version);
    let should_rebuild_fts_in_background = is_version_changed;

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
    let mut screenshot_service = ScreenshotService::new(&data_dir, &config.storage);
    screenshot_service.update_privacy(&config.privacy);

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

    // 初始化存储管理器（启动清理已移入窗口出现后的 storage_cleanup_task 后台首轮执行，不阻塞首屏）
    let storage_manager = StorageManager::new(&data_dir, config.storage.clone());
    let initial_avatar_opacity = config.avatar_opacity;
    let initial_avatar_preset = config.avatar_preset.clone();
    let initial_avatar_persona = config.avatar_persona.clone();
    let initial_avatar_body_hidden = config.avatar_body_hidden;

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
        wecom_bot_runtime: wecom_aibot::WecomBotRuntime::default(),
        cached_active_window: None,
    }));
    let app_lifecycle_state = Arc::new(Mutex::new(AppLifecycleState::default()));

    // 构建 Tauri 应用：单实例插件优先注册，重复启动时尽早退出
    let builder = tauri::Builder::default()
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
        .plugin(navigation_guard::plugin())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(not(windows))]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        Some(vec![AUTOSTART_LAUNCH_ARG]),
    ));
    builder
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
        .setup(move |app| {
            if let Err(e) = autostart::init_autostart(app.handle()) {
                log::warn!("初始化开机自启功能失败: {e}");
            }

            let window =
                ensure_main_window(app.handle()).expect("main window should exist at setup");
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
            let state_clone7 = state.inner().clone();
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
                let avatar_enabled = if let Ok(s) = state.inner().lock() {
                    avatar_input::set_avatar_enabled_flag(s.config.avatar_enabled);
                    avatar_input::set_avatar_click_through_flag(s.config.avatar_click_through);
                    avatar_input::set_avatar_body_hidden_flag(s.config.avatar_body_hidden);
                    s.config.avatar_enabled
                } else {
                    false
                };
                if avatar_enabled {
                    avatar_input::start_avatar_input_monitor(app.handle());
                    avatar_input::spawn_avatar_input_monitor_retry(app.handle().clone());
                    avatar_input::spawn_avatar_input_bridge(app.handle().clone());
                }
            } else {
                avatar_input::set_avatar_enabled_flag(false);
                avatar_input::set_avatar_click_through_flag(false);
                avatar_input::set_avatar_body_hidden_flag(false);
                log::warn!("配置损坏，已跳过全局键鼠监听初始化");
            }

            if let Err(e) = localhost_api::sync_localhost_api_runtime(app.handle(), state.inner()) {
                log::warn!("初始化本地 API 失败: {e}");
            }
            if let Err(e) = telegram_bot::sync_telegram_bot_runtime(state.inner()) {
                log::warn!("初始化 Telegram Bot 失败: {e}");
            }
            if let Err(e) = wecom_aibot::sync_wecom_bot_runtime(state.inner()) {
                log::warn!("初始化企业微信 Bot 失败: {e}");
            }
            // 仅在配置正常加载允许自动保存时才启动 WebDAV 拉取，避免从备份恢复时覆盖唯一干净的 config.json.bak。
            if config_load_status.allows_automatic_save() {
                config_sync::spawn_pull_on_startup(app.handle().clone(), state.inner().clone());
            } else {
                log::warn!("当前配置处于备份恢复/故障安全状态，已跳过启动期 WebDAV 配置同步");
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
                        if let Err(e) =
                            commands::update_app_config(app.clone(), &state_for_tray, |current| {
                                let mut next = current.clone();
                                next.lightweight_mode = !next.lightweight_mode;
                                Ok(next)
                            })
                        {
                            log::warn!("从托盘切换轻量模式失败: {e}");
                            refresh_tray_menu(app);
                        }
                    }
                    TRAY_MENU_AVATAR_TOGGLE_ID => {
                        if let Err(e) =
                            commands::update_app_config(app.clone(), &state_for_tray, |current| {
                                let mut next = current.clone();
                                next.avatar_enabled = !next.avatar_enabled;
                                Ok(next)
                            })
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

            // 先发布句柄，再注册后台电源通知，避免恢复事件找不到应用。
            let _ = APP_HANDLE.set(app.handle().clone());

            // 首次启动或升级后的 FTS 重建放入后台执行，不阻塞首屏窗口显示
            if should_rebuild_fts_in_background {
                let db_for_fts = state
                    .inner()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .database
                    .clone();
                tauri::async_runtime::spawn(async move {
                    match tokio::task::spawn_blocking(move || db_for_fts.rebuild_fts_index()).await
                    {
                        Ok(Ok(())) => log::info!("升级后后台 FTS 索引重建完成"),
                        Ok(Err(e)) => log::warn!("后台 FTS 索引重建失败（不影响核心功能）: {e}"),
                        Err(e) => log::warn!("后台 FTS 索引重建任务异常: {e}"),
                    }
                });
            }

            // 所有常驻后台循环统一加监督重启：panic 后记录日志并按指数退避重启，托盘同步反映状态
            let crash_app_handle = screenshot_app_handle.clone();
            let crash_state = state_clone.clone();
            let recording_crash_hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                flush_active_recording_session_on_exit(&crash_state);
                emit_recording_state_changed(&crash_app_handle);
            });
            spawn_supervised_task(
                "background_screenshot_task",
                Some(recording_crash_hook),
                move || {
                    let s = state_clone.clone();
                    let a = screenshot_app_handle.clone();
                    async move {
                        background_screenshot_task(s, a).await;
                    }
                },
            );

            let report_app_handle = app.handle().clone();
            let state_for_auto_report = state.inner().clone();
            spawn_supervised_task("auto_report_and_insight_task", None, move || {
                let s = state_for_auto_report.clone();
                let a = report_app_handle.clone();
                async move {
                    auto_report_and_insight_task(s, a).await;
                }
            });

            spawn_supervised_task("background_avatar_task", None, move || {
                let s = state_clone3.clone();
                let a = app_handle.clone();
                async move {
                    background_avatar_task(s, a).await;
                }
            });

            spawn_supervised_task("hourly_summary_task", None, move || {
                let s = state_clone2.clone();
                async move {
                    hourly_summary_task(s).await;
                }
            });

            spawn_supervised_task("remote_upload_backfill_task", None, move || {
                let s = state_clone4.clone();
                async move {
                    remote_upload_backfill_task(s).await;
                }
            });

            spawn_supervised_task("storage_cleanup_task", None, move || {
                let s = state_clone5.clone();
                async move {
                    storage_cleanup_task(s).await;
                }
            });

            spawn_supervised_task("entity_classify_task", None, move || {
                let s = state_clone6.clone();
                async move {
                    entity_classify_task(s).await;
                }
            });

            spawn_supervised_task("semantic_auto_index_task", None, move || {
                let s = state_clone7.clone();
                async move {
                    semantic_auto_index_task(s).await;
                }
            });

            let state_for_routines = state.inner().clone();
            spawn_supervised_task("agent_routine_scheduler_task", None, move || {
                let s = state_for_routines.clone();
                async move {
                    agent::routine_runner::agent_routine_scheduler_task(s).await;
                }
            });

            sync_effective_dock_visibility(app.handle());

            // 注: macOS Dock 点击恢复窗口通过系统托盘 LeftClick 事件处理
            // 用户需要点击状态栏的系统托盘图标来恢复窗口

            log::info!("应用初始化完成");
            Ok(())
        })
        .invoke_handler(commands::handler())
        .build(tauri::generate_context!())
        .expect("构建 Tauri 应用时出错")
        .run(|_app_handle, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                let mut prevented = false;
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
                        prevented = true;
                    }
                }
                if !prevented {
                    if let Some(state) = _app_handle.try_state::<Arc<Mutex<AppState>>>() {
                        flush_active_recording_session_on_exit(state.inner());
                    }
                }
            }
            tauri::RunEvent::Exit => {
                avatar_input::set_avatar_enabled_flag(false);
                avatar_input::stop_avatar_input_monitor(_app_handle);
                if let Some(state) = _app_handle.try_state::<Arc<Mutex<AppState>>>() {
                    flush_active_recording_session_on_exit(state.inner());
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
mod hourly_schedule_tests {
    use super::seconds_until_next_hourly_check;

    #[test]
    fn 小时摘要应在下一个整点第5分钟检查() {
        // 10:02:00 → 11:05:00
        assert_eq!(seconds_until_next_hourly_check(2, 0), 63 * 60);
        // 10:59:59 → 11:05:00
        assert_eq!(seconds_until_next_hourly_check(59, 59), 5 * 60 + 1);
        // 10:05:00 → 11:05:00
        assert_eq!(seconds_until_next_hourly_check(5, 0), 60 * 60);
        // 闰秒等异常输入不应溢出
        assert!(seconds_until_next_hourly_check(59, 60) >= 60);
    }
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
        monitoring_poll_interval_ms_for_platform, overlay_session_increment,
        persist_previous_activity_backfill, previous_app_backfill_duration,
        record_avatar_window_switch, recording_loop_decision, resolve_activity_classification,
        resolve_browser_url_once, reusable_cached_active_window,
        screen_lock_check_interval_ms_for_platform, should_confirm_idle,
        should_emit_avatar_backlog_nudge, should_hide_main_window_on_setup,
        should_initialize_avatar_input, should_initialize_startup_permissions,
        should_merge_contiguous_activity, should_persist_merge_update, should_prevent_exit,
        should_probe_browser_url_before_change_detection, should_request_screen_capture_permission,
        should_run_ocr, should_run_startup_cleanup, should_skip_system_window,
        tray_recording_toggle_action, tray_recording_toggle_label, AvatarNudgeRuntime,
        BreakReminderRuntime, BreakReminderSignal, MainWindowCloseBehavior, RecordingToggleAction,
    };
    use crate::avatar_engine::{
        apply_avatar_visual_settings, default_avatar_state, derive_avatar_state,
    };
    use crate::monitor::ActiveWindow;
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use work_review_core::config::{
        AiProvider, AppConfig, AvatarFollowupItem, ConfigLoadStatus, ModelConfig,
        WebsiteSemanticRule,
    };
    use work_review_core::database::Database;
    use work_review_core::privacy::PrivacyFilter;

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
    fn 浮窗应按实际连续观察间隔计时且不得跨会话合并() {
        let start = Instant::now();
        let mut activity = work_review_core::database::Activity {
            id: Some(7),
            timestamp: 1000,
            app_name: "Player".into(),
            window_title: "PiP".into(),
            screenshot_path: String::new(),
            ocr_text: None,
            category: "entertainment".into(),
            duration: 0,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };
        let sample = start + Duration::from_millis(30_500);
        assert_eq!(
            overlay_session_increment(None, &activity, sample, 1030),
            None
        );
        assert_eq!(
            overlay_session_increment(Some((7, start)), &activity, sample, 1030),
            Some((30, start + Duration::from_secs(30)))
        );
        // 30.5 秒后的半秒余量不得被每轮舍弃：再过 1.5 秒应追加 2 秒。
        activity.timestamp = 1030;
        assert_eq!(
            overlay_session_increment(
                Some((7, start + Duration::from_secs(30))),
                &activity,
                start + Duration::from_secs(32),
                1032
            ),
            Some((2, start + Duration::from_secs(32)))
        );
        // 前台记录取代了上一次浮窗记录、长时间断开、系统时钟倒退都须新建会话。
        activity.id = Some(8);
        assert_eq!(
            overlay_session_increment(Some((7, start)), &activity, sample, 1030),
            None
        );
        activity.id = Some(7);
        assert_eq!(
            overlay_session_increment(Some((7, start)), &activity, sample, 1631),
            None
        );
        assert_eq!(
            overlay_session_increment(Some((7, start)), &activity, sample, 1029),
            None
        );
        activity.app_name = "Unknown".into();
        assert_eq!(
            overlay_session_increment(Some((7, start)), &activity, sample, 1030),
            None
        );
    }

    #[test]
    fn ocr应遵循全局间隔且首次立即执行() {
        assert!(should_run_ocr(None, 30));
        assert!(!should_run_ocr(Some(Duration::from_secs(29)), 30));
        assert!(should_run_ocr(Some(Duration::from_secs(30)), 30));
        assert!(!should_run_ocr(Some(Duration::from_secs(5)), 30));
        assert!(!should_run_ocr(Some(Duration::ZERO), 30));

        // 连续 80 分钟每秒切换窗口，30 秒间隔最多启动 160 次 OCR。
        let mut last_ocr_at = None;
        let mut ocr_count = 0;
        for second in 0..80 * 60 {
            let now = Duration::from_secs(second);
            let elapsed = last_ocr_at.map(|last| now - last);
            if should_run_ocr(elapsed, 30) {
                last_ocr_at = Some(now);
                ocr_count += 1;
            }
        }
        assert_eq!(ocr_count, 160);
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
    fn 网站规则应在浏览器网址缺失或不可用时匹配标题() {
        let mut config = AppConfig::default();
        config.website_semantic_rules = vec![WebsiteSemanticRule {
            domain: "github.com".to_string(),
            semantic_category: "休息娱乐".to_string(),
        }];
        config.normalize();

        for browser_url in [None, Some("  "), Some("https://linux.dolatest")] {
            let classification = resolve_activity_classification(
                &config,
                "Google Chrome",
                "github.com - Google Chrome",
                browser_url,
            );
            assert_eq!(classification.base_category, "entertainment");
            assert_eq!(classification.semantic_category, "休息娱乐");
            assert_eq!(classification.confidence, 100);
        }
    }

    #[test]
    fn 网站规则不应因标题提到域名而覆盖其它网站或普通应用() {
        let mut config = AppConfig::default();
        config.website_semantic_rules = vec![WebsiteSemanticRule {
            domain: "github.com".to_string(),
            semantic_category: "休息娱乐".to_string(),
        }];
        config.normalize();

        for (app_name, browser_url) in [
            ("Google Chrome", Some("https://docs.github.com/guide")),
            ("Visual Studio Code", None),
        ] {
            let classification = resolve_activity_classification(
                &config,
                app_name,
                "github.com - 项目文档",
                browser_url,
            );
            assert!(!classification
                .evidence
                .iter()
                .any(|evidence| evidence.starts_with("命中网站语义规则:")));
        }
    }

    #[test]
    fn 非mac主监控轮询间隔应按平台避免高频窗口查询() {
        assert_eq!(
            monitoring_poll_interval_ms_for_platform(false),
            if cfg!(target_os = "linux") { 1000 } else { 500 }
        );
    }

    #[test]
    fn 非mac桌宠活跃轮询间隔应按平台避免高频窗口查询() {
        assert_eq!(
            avatar_monitor_poll_interval_ms_for_platform(false, true),
            if cfg!(target_os = "linux") { 1000 } else { 180 }
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
            is_private: false,
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
            is_private: false,
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
            ..ModelConfig::default_text()
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
            is_private: false,
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
            is_private: false,
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
            is_private: false,
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
            is_private: false,
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
            is_private: false,
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
            is_private: false,
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

    #[test]
    fn 自动日报触发时刻应优先自定义时间其次取最晚工作段结束() {
        let mut config = AppConfig::default();
        config.work_end_hour = 18;
        config.work_end_minute = 30;
        config.work_time_segments = vec![
            work_review_core::config::WorkTimeSegment {
                start_hour: 9,
                start_minute: 0,
                end_hour: 12,
                end_minute: 0,
            },
            work_review_core::config::WorkTimeSegment {
                start_hour: 13,
                start_minute: 30,
                end_hour: 19,
                end_minute: 15,
            },
        ];
        assert_eq!(
            super::resolve_auto_report_work_end_minutes(&config),
            19 * 60 + 15
        );

        config.daily_report_auto_generate_time = Some("20:45".to_string());
        assert_eq!(
            super::resolve_auto_report_work_end_minutes(&config),
            20 * 60 + 45
        );

        config.daily_report_auto_generate_time = Some("invalid".to_string());
        assert_eq!(
            super::resolve_auto_report_work_end_minutes(&config),
            19 * 60 + 15
        );
    }

    #[test]
    fn 空闲期间截图周期应自动拉长且活跃时保持配置值() {
        assert_eq!(super::effective_screenshot_interval_secs(30, false), 30);
        assert_eq!(super::effective_screenshot_interval_secs(15, true), 90);
        assert_eq!(super::effective_screenshot_interval_secs(60, true), 180);
    }
}
