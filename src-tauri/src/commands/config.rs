//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::config::AppConfig;
use crate::database::Database;
use crate::error::AppError;
use crate::privacy::PrivacyFilter;
use crate::screenshot::ScreenshotService;
use crate::storage::StorageManager;
use crate::AppState;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

use super::shared::persist_app_config;

const MANAGED_DATA_ENTRIES: &[&str] = &[
    "config.json",
    "workreview.db",
    "screenshots",
    "ocr_logs",
    "background.jpg",
    "update_settings.json",
];

const LIVE_DATABASE_FILES: &[&str] = &["workreview.db", "workreview.db-shm", "workreview.db-wal"];

/// 获取配置
#[tauri::command]
pub async fn get_config(state: State<'_, Arc<Mutex<AppState>>>) -> Result<AppConfig, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(state.config.clone())
}

/// 保存配置
#[tauri::command]
pub async fn save_config(
    config: AppConfig,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    persist_app_config(config, app, state.inner())
}

/// 获取数据目录
#[tauri::command]
pub async fn get_data_dir(state: State<'_, Arc<Mutex<AppState>>>) -> Result<String, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(path_for_display(&state.data_dir))
}

/// 获取默认数据目录
#[tauri::command]
pub async fn get_default_data_dir() -> Result<String, AppError> {
    Ok(path_for_display(&crate::default_data_dir()))
}

fn path_for_display(path: &Path) -> String {
    let raw = path.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    {
        raw.strip_prefix(r"\\?\")
            .or_else(|| raw.strip_prefix(r"\??\"))
            .unwrap_or(&raw)
            .to_string()
    }

    #[cfg(not(target_os = "windows"))]
    {
        raw
    }
}

fn is_ignorable_dir_entry(name: &str) -> bool {
    name.starts_with('.') || name == "Thumbs.db"
}

fn is_managed_dir_entry(name: &str) -> bool {
    MANAGED_DATA_ENTRIES.contains(&name)
}

fn is_cleanup_managed_dir_entry(name: &str) -> bool {
    MANAGED_DATA_ENTRIES.contains(&name) || LIVE_DATABASE_FILES.contains(&name)
}

fn to_absolute_path(path: &Path) -> Result<PathBuf, AppError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn ensure_target_dir_ready(target_dir: &Path) -> Result<bool, AppError> {
    std::fs::create_dir_all(target_dir)?;

    let mut has_existing_app_data = false;

    for entry in std::fs::read_dir(target_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if is_ignorable_dir_entry(&name) {
            continue;
        }

        if !is_managed_dir_entry(&name) {
            return Err(AppError::Config(format!(
                "目标目录包含非 Work Review 数据（{name}），为避免误覆盖，请选择空目录或旧的数据目录"
            )));
        }

        has_existing_app_data = true;
    }

    if !has_existing_app_data {
        return Ok(false);
    }

    // 目标目录若已存在旧版应用数据，先清空受管条目，再完整覆盖为当前数据。
    for entry_name in MANAGED_DATA_ENTRIES {
        let path = target_dir.join(entry_name);
        if !path.exists() {
            continue;
        }

        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }

    Ok(true)
}

fn copy_managed_data_without_live_db(
    source_dir: &Path,
    target_dir: &Path,
) -> Result<u64, AppError> {
    let mut copied_files = 0u64;

    for entry_name in MANAGED_DATA_ENTRIES {
        if LIVE_DATABASE_FILES.contains(entry_name) {
            continue;
        }

        let source_path = source_dir.join(entry_name);
        if !source_path.exists() {
            continue;
        }

        let target_path = target_dir.join(entry_name);
        if source_path.is_dir() {
            copied_files += crate::copy_dir_contents(&source_path, &target_path, true)?;
        } else {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source_path, &target_path)?;
            copied_files += 1;
        }
    }

    Ok(copied_files)
}

fn remove_app_managed_entries(target_dir: &Path) -> Result<(u64, Vec<String>), AppError> {
    let mut removed_entries = 0u64;
    let mut preserved_entries = Vec::new();

    if !target_dir.exists() {
        return Ok((0, preserved_entries));
    }

    for entry in std::fs::read_dir(target_dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if is_ignorable_dir_entry(&name) {
            continue;
        }

        if is_cleanup_managed_dir_entry(&name) {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
            removed_entries += 1;
            continue;
        }

        preserved_entries.push(name);
    }

    if preserved_entries.is_empty() {
        let mut remaining_entries = std::fs::read_dir(target_dir)?;
        if remaining_entries.next().is_none() {
            let _ = std::fs::remove_dir(target_dir);
        }
    }

    Ok((removed_entries, preserved_entries))
}

/// 切换数据目录，并迁移当前数据
#[tauri::command]
pub async fn change_data_dir(
    target_dir: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let requested_dir = target_dir.trim();
    if requested_dir.is_empty() {
        return Err(AppError::Config("目标目录不能为空".to_string()));
    }

    let requested_path = to_absolute_path(Path::new(requested_dir))?;
    let current_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state
            .data_dir
            .canonicalize()
            .unwrap_or_else(|_| state.data_dir.clone())
    };

    if requested_path == current_dir {
        return Ok(serde_json::json!({
            "dataDir": current_dir.to_string_lossy().to_string(),
            "copiedFiles": 0,
            "message": "数据目录未变化",
        }));
    }

    if requested_path.starts_with(&current_dir) || current_dir.starts_with(&requested_path) {
        return Err(AppError::Config(
            "新旧数据目录不能互为父子目录，请选择独立目录".to_string(),
        ));
    }

    let target_dir = {
        std::fs::create_dir_all(&requested_path)?;
        requested_path
            .canonicalize()
            .unwrap_or_else(|_| requested_path.clone())
    };

    // 先清空目标目录中已有的受管条目（必须在 backup_to 之前，否则会删掉刚备份的数据库）
    let replaced_existing_data = ensure_target_dir_ready(&target_dir)?;

    // 复制截图等文件（在锁外执行，不阻塞截图循环）
    let copied_files = copy_managed_data_without_live_db(&current_dir, &target_dir)?;

    // 短暂获取锁，做安全 SQLite 备份，然后立即释放
    let config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        // SQLite 备份必须在持锁状态下执行（backup_to 内部做 WAL checkpoint + VACUUM INTO）
        state
            .database
            .backup_to(&target_dir.join("workreview.db"))?;
        state.config.clone()
    };

    let config_path = target_dir.join("config.json");
    config.save(&config_path)?;
    crate::save_data_dir_preference(&target_dir)?;

    // 重新获取锁，仅做轻量状态更新
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database = Database::new(&target_dir.join("workreview.db"))?;
    if let Err(e) = state.database.rebuild_fts_index() {
        log::warn!("迁移后 FTS 索引重建失败: {e}");
    }
    state.privacy_filter = PrivacyFilter::from_config(&config.privacy);
    state.screenshot_service = ScreenshotService::new(&target_dir, &config.storage);
    state.storage_manager = StorageManager::new(&target_dir, config.storage.clone());
    state.data_dir = target_dir.clone();
    state.config_path = config_path;

    log::info!("数据目录已切换到: {target_dir:?}");
    drop(state);
    crate::emit_recording_state_changed(&app);

    Ok(serde_json::json!({
        "dataDir": target_dir.to_string_lossy().to_string(),
        "oldDataDir": current_dir.to_string_lossy().to_string(),
        "copiedFiles": copied_files,
        "replacedExistingData": replaced_existing_data,
        "message": format!(
            "数据目录已更新，已迁移 {} 个文件{}",
            copied_files,
            if replaced_existing_data { "，并覆盖旧目录中的 Work Review 数据" } else { "" }
        ),
    }))
}

#[tauri::command]
pub async fn cleanup_old_data_dir(
    target_dir: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let requested_dir = target_dir.trim();
    if requested_dir.is_empty() {
        return Err(AppError::Config("旧目录不能为空".to_string()));
    }

    let requested_path = to_absolute_path(Path::new(requested_dir))?;
    if !requested_path.exists() {
        return Ok(serde_json::json!({
            "removedEntries": 0,
            "preservedEntries": [],
            "message": "旧目录不存在，无需清理",
        }));
    }

    let current_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state
            .data_dir
            .canonicalize()
            .unwrap_or_else(|_| state.data_dir.clone())
    };

    let cleanup_dir = requested_path
        .canonicalize()
        .unwrap_or_else(|_| requested_path.clone());

    if cleanup_dir == current_dir {
        return Err(AppError::Config(
            "不能清理当前正在使用的数据目录".to_string(),
        ));
    }

    if cleanup_dir.starts_with(&current_dir) || current_dir.starts_with(&cleanup_dir) {
        return Err(AppError::Config(
            "为避免误删，当前数据目录与待清理目录不能互为父子目录".to_string(),
        ));
    }

    let (removed_entries, preserved_entries) = remove_app_managed_entries(&cleanup_dir)?;
    let message = if preserved_entries.is_empty() {
        if cleanup_dir.exists() {
            format!("已清理旧目录中的 {removed_entries} 项 Work Review 数据")
        } else {
            format!("已清理旧目录中的 {removed_entries} 项 Work Review 数据，并移除空目录")
        }
    } else {
        format!(
            "已清理旧目录中的 {} 项 Work Review 数据，保留其他文件：{}",
            removed_entries,
            preserved_entries.join("、")
        )
    };

    Ok(serde_json::json!({
        "removedEntries": removed_entries,
        "preservedEntries": preserved_entries,
        "message": message,
    }))
}

/// 在系统文件管理器中打开数据目录
/// plugin-shell 的 open 对本地路径在部分平台不可靠，改用系统命令直接打开
#[tauri::command]
pub async fn open_data_dir(state: State<'_, Arc<Mutex<AppState>>>) -> Result<(), AppError> {
    let data_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.data_dir.clone()
    };

    // 目录不存在时先创建，避免打开失败
    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| AppError::Unknown(format!("创建数据目录失败: {e}")))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&data_dir)
            .spawn()
            .map_err(|e| AppError::Unknown(format!("打开数据目录失败: {e}")))?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&data_dir)
            .spawn()
            .map_err(|e| AppError::Unknown(format!("打开数据目录失败: {e}")))?;
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&data_dir)
            .spawn()
            .map_err(|e| AppError::Unknown(format!("打开数据目录失败: {e}")))?;
    }

    Ok(())
}

