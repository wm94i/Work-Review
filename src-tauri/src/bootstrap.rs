//! 启动参数判定、配置迁移兜底、目录复制与常驻后台任务监督重启。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use work_review_core::config::{AppConfig, ConfigLoadStatus};
pub(crate) const AUTOSTART_LAUNCH_ARG: &str = "--autostart";

pub(crate) fn launch_args_contain_autostart(args: &[String]) -> bool {
    args.iter().any(|arg| arg == AUTOSTART_LAUNCH_ARG)
}

pub(crate) fn args_include_explicit_hidden_flag(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "--hidden" | "--minimized"))
}

#[cfg(not(windows))]
pub(crate) fn launch_args_request_hidden_window(args: &[String]) -> bool {
    launch_args_contain_autostart(args) || args_include_explicit_hidden_flag(args)
}

pub(crate) fn duplicate_instance_should_stay_silent(args: &[String]) -> bool {
    launch_args_contain_autostart(args) || args_include_explicit_hidden_flag(args)
}

pub(crate) fn should_hide_main_window_on_setup(
    _config: &AppConfig,
    launch_args: &[String],
) -> bool {
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

#[cfg(any(target_os = "macos", test))]
pub(crate) fn should_request_screen_capture_permission(
    has_screen_capture_permission: bool,
    already_prompted: bool,
) -> bool {
    !has_screen_capture_permission && !already_prompted
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn should_initialize_startup_permissions(status: ConfigLoadStatus) -> bool {
    !status.requires_fail_safe()
}

pub(crate) fn should_run_startup_cleanup(status: ConfigLoadStatus) -> bool {
    !status.requires_fail_safe()
}

pub(crate) fn should_initialize_avatar_input(status: ConfigLoadStatus) -> bool {
    !status.requires_fail_safe()
}

/// 清理历史遗留的钥匙串占位符。
/// 早期未发布版本曾把敏感字段真实值迁入系统钥匙串,磁盘配置写成 "__keychain__" 占位符;
/// 该机制已整体移除。这里把残留占位符清空,避免被当作真实密钥使用（需在设置中重新填写）。
pub(crate) fn clear_legacy_keychain_placeholders(config: &mut work_review_core::config::AppConfig) {
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
        clear_opt(&mut config.wecom_bot_secret);
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

pub(crate) fn describe_config_file_issue(path: &Path, error: Option<&str>) -> String {
    match error {
        Some(error) => format!("{} 读取或解析失败: {error}", path.display()),
        None => format!("{} 不存在", path.display()),
    }
}

pub(crate) fn initial_recording_state(status: ConfigLoadStatus) -> (bool, bool) {
    if status.requires_fail_safe() {
        (false, true)
    } else {
        (true, false)
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

/// 为常驻后台循环增加监督重启：捕获 panic 或异常退出，记录日志并按指数退避（2s → 60s）自动重启。
pub(crate) fn spawn_supervised_task<F, Fut>(
    name: &'static str,
    on_crash: Option<Arc<dyn Fn() + Send + Sync>>,
    mut make_future: F,
) where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        let mut backoff_secs = 2u64;
        loop {
            let started = std::time::Instant::now();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut make_future)) {
                Ok(fut) => match tauri::async_runtime::spawn(fut).await {
                    Ok(()) => {
                        log::warn!("后台任务 [{name}] 意外正常退出，将在 {backoff_secs}s 后重启");
                    }
                    Err(error) => {
                        log::error!(
                            "后台任务 [{name}] 发生异常/panic: {error}，将在 {backoff_secs}s 后重启"
                        );
                    }
                },
                Err(_) => {
                    log::error!(
                        "后台任务 [{name}] 构造 future 时发生 panic，将在 {backoff_secs}s 后重启"
                    );
                }
            }
            if let Some(ref hook) = on_crash {
                hook();
            }
            if started.elapsed() >= Duration::from_secs(300) {
                backoff_secs = 2;
            }
            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs.saturating_mul(2)).min(60);
        }
    });
}
