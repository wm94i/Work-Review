//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::screenshot::ScreenshotService;
use crate::storage::StorageManager;
use crate::AppState;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use work_review_core::config::AppConfig;
use work_review_core::database::Database;
use work_review_core::error::AppError;
use work_review_core::privacy::PrivacyFilter;

use super::shared::persist_app_config;

const MANAGED_DATA_ENTRIES: &[&str] = &[
    "config.json",
    "config.json.bak",
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
    // 模型端点安全校验：远程端点必须使用 https，明文 http 仅允许本机/内网（Ollama 等）
    super::ai::validate_model_endpoint(&config.text_model.endpoint)?;
    super::ai::validate_model_endpoint(&config.vision_model.endpoint)?;
    super::ai::validate_model_endpoint(&config.ai_provider.endpoint)?;
    for profile in &config.text_model_profiles {
        super::ai::validate_model_endpoint(&profile.model_config.endpoint)?;
    }
    for cached in config.text_model_provider_cache.values() {
        super::ai::validate_model_endpoint(&cached.endpoint)?;
    }
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
    MANAGED_DATA_ENTRIES.contains(&name) || LIVE_DATABASE_FILES.contains(&name)
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

fn resolve_migration_target(
    current_dir: &Path,
    requested_path: &Path,
) -> Result<PathBuf, AppError> {
    std::fs::create_dir_all(requested_path)?;
    let target_dir = requested_path.canonicalize()?;
    if target_dir != current_dir
        && (target_dir.starts_with(current_dir) || current_dir.starts_with(&target_dir))
    {
        return Err(AppError::Config(
            "新旧数据目录不能互为父子目录，请选择独立目录".to_string(),
        ));
    }
    Ok(target_dir)
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

    // 这里只校验目录，绝不提前删除旧数据；迁移副本验证完成后才会替换受管条目。
    Ok(has_existing_app_data)
}

fn migration_entry_names() -> Vec<&'static str> {
    let mut names = Vec::with_capacity(MANAGED_DATA_ENTRIES.len() + LIVE_DATABASE_FILES.len());
    for name in MANAGED_DATA_ENTRIES
        .iter()
        .chain(LIVE_DATABASE_FILES.iter())
    {
        if !names.contains(name) {
            names.push(name);
        }
    }
    names
}

fn remove_migration_path(path: &Path) -> Result<(), AppError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn migration_sibling_path(target_dir: &Path, label: &str) -> Result<PathBuf, AppError> {
    let parent = target_dir
        .parent()
        .ok_or_else(|| AppError::Config("无法为数据迁移创建临时目录".to_string()))?;
    Ok(parent.join(format!(
        ".work-review-{label}-{}",
        uuid::Uuid::new_v4().simple()
    )))
}

fn restore_migration_entries(
    target_dir: &Path,
    backup_dir: &Path,
    moved_entries: &[String],
    installed_entries: &[String],
) {
    for entry_name in installed_entries.iter().rev() {
        if let Err(error) = remove_migration_path(&target_dir.join(entry_name)) {
            log::error!("回滚迁移新条目 {} 失败: {error}", entry_name);
        }
    }
    for entry_name in moved_entries.iter().rev() {
        let backup_path = backup_dir.join(entry_name);
        if !backup_path.exists() {
            continue;
        }
        if let Err(error) = std::fs::rename(&backup_path, target_dir.join(entry_name)) {
            log::error!("恢复迁移旧条目 {} 失败: {error}", entry_name);
        }
    }
}

fn commit_migration_stage(stage_dir: &Path, target_dir: &Path) -> Result<(), AppError> {
    let backup_dir = migration_sibling_path(target_dir, "migration-backup")?;
    std::fs::create_dir_all(&backup_dir)?;
    let mut moved_entries = Vec::new();

    for entry_name in migration_entry_names() {
        let source = target_dir.join(entry_name);
        if !source.exists() {
            continue;
        }
        if let Err(error) = std::fs::rename(&source, backup_dir.join(entry_name)) {
            restore_migration_entries(target_dir, &backup_dir, &moved_entries, &[]);
            let _ = remove_migration_path(&backup_dir);
            let _ = remove_migration_path(stage_dir);
            return Err(error.into());
        }
        moved_entries.push(entry_name.to_string());
    }

    let mut installed_entries = Vec::new();
    for entry_name in migration_entry_names() {
        let source = stage_dir.join(entry_name);
        if !source.exists() {
            continue;
        }
        if let Err(error) = std::fs::rename(&source, target_dir.join(entry_name)) {
            restore_migration_entries(target_dir, &backup_dir, &moved_entries, &installed_entries);
            let _ = remove_migration_path(&backup_dir);
            let _ = remove_migration_path(stage_dir);
            return Err(error.into());
        }
        installed_entries.push(entry_name.to_string());
    }

    if let Err(error) = remove_migration_path(&backup_dir) {
        log::warn!("清理迁移备份目录失败（数据已切换成功）: {error}");
    }
    if let Err(error) = remove_migration_path(stage_dir) {
        log::warn!("清理迁移临时目录失败（数据已切换成功）: {error}");
    }
    Ok(())
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
        state.data_dir.canonicalize()?
    };

    let target_dir = resolve_migration_target(&current_dir, &requested_path)?;
    if target_dir == current_dir {
        return Ok(serde_json::json!({
            "dataDir": current_dir.to_string_lossy().to_string(),
            "copiedFiles": 0,
            "message": "数据目录未变化",
        }));
    }

    // 只校验目标目录，不先删除其中的旧数据。
    let replaced_existing_data = ensure_target_dir_ready(&target_dir)?;
    let staging_dir = migration_sibling_path(&target_dir, "migration-stage")?;
    std::fs::create_dir_all(&staging_dir)?;

    // 先在目标目录旁构建完整副本；复制或 SQLite 备份失败时，目标旧数据完全不变。
    let staged_result: Result<(u64, AppConfig), AppError> = (|| {
        let copied_files = copy_managed_data_without_live_db(&current_dir, &staging_dir)?;
        let config = {
            let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
            // SQLite 备份必须在持锁状态下执行（backup_to 内部做 WAL checkpoint + VACUUM INTO）
            state
                .database
                .backup_to(&staging_dir.join("workreview.db"))?;
            state.config.clone()
        };
        config.save(&staging_dir.join("config.json"))?;
        Ok((copied_files, config))
    })();
    let (copied_files, config) = match staged_result {
        Ok(result) => result,
        Err(error) => {
            if let Err(cleanup_error) = remove_migration_path(&staging_dir) {
                log::warn!("清理失败的迁移临时目录失败: {cleanup_error}");
            }
            return Err(error);
        }
    };

    commit_migration_stage(&staging_dir, &target_dir)?;
    let config_path = target_dir.join("config.json");
    let preference_warning = match crate::save_data_dir_preference(&target_dir) {
        Ok(()) => None,
        Err(error) => {
            log::error!("保存数据目录偏好失败，当前运行仍已切换: {error}");
            Some(format!("；但保存数据目录偏好失败：{error}"))
        }
    };

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
        ) + preference_warning.as_deref().unwrap_or(""),
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

#[cfg(test)]
mod tests {
    use super::{
        commit_migration_stage, ensure_target_dir_ready, migration_sibling_path,
        resolve_migration_target,
    };

    #[test]
    fn 迁移应按真实路径拒绝父子目录并识别当前目录别名() {
        let root = std::env::temp_dir().join(format!("migration-path-{}", uuid::Uuid::new_v4()));
        let current = root.join("current");
        std::fs::create_dir_all(current.join("screenshots")).unwrap();
        let current = current.canonicalize().unwrap();
        let root = root.canonicalize().unwrap();

        assert_eq!(
            resolve_migration_target(&current, &current).unwrap(),
            current
        );
        assert!(resolve_migration_target(&current, &root).is_err());
        assert!(resolve_migration_target(&current, &current.join("screenshots/new")).is_err());
        assert_eq!(
            resolve_migration_target(&current, &root.join("sibling")).unwrap(),
            root.join("sibling"),
        );
        assert!(resolve_migration_target(
            &current,
            &root.join("sibling/../current/screenshots/new"),
        )
        .is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&current, root.join("same-alias")).unwrap();
            symlink(current.join("screenshots"), root.join("child-alias")).unwrap();
            symlink(&root, root.join("parent-alias")).unwrap();
            assert_eq!(
                resolve_migration_target(&current, &root.join("same-alias")).unwrap(),
                current,
            );
            assert!(resolve_migration_target(&current, &root.join("child-alias/new")).is_err());
            assert!(resolve_migration_target(&current, &root.join("parent-alias")).is_err());
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn 迁移构建阶段不得删除目标旧数据并能替换受管条目() {
        let root = std::env::temp_dir().join(format!("migration-stage-{}", uuid::Uuid::new_v4()));
        let target = root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("config.json"), b"old-config").unwrap();
        std::fs::write(target.join(".keep.txt"), b"user-file").unwrap();

        assert!(ensure_target_dir_ready(&target).unwrap());
        assert_eq!(
            std::fs::read(target.join("config.json")).unwrap(),
            b"old-config"
        );

        let stage = migration_sibling_path(&target, "test-stage").unwrap();
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("config.json"), b"new-config").unwrap();
        commit_migration_stage(&stage, &target).unwrap();

        assert_eq!(
            std::fs::read(target.join("config.json")).unwrap(),
            b"new-config"
        );
        assert_eq!(
            std::fs::read(target.join(".keep.txt")).unwrap(),
            b"user-file"
        );
        assert!(!stage.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
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
