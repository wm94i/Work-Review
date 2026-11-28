//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::screenshot::ScreenshotService;
use crate::storage::StorageManager;
use crate::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use work_review_core::config::AppConfig;
use work_review_core::database::Database;
use work_review_core::error::AppError;
use work_review_core::privacy::PrivacyFilter;

use super::shared::update_app_config;

static DATA_MAINTENANCE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// 进程级数据目录维护互斥守卫（覆盖 `change_data_dir` 与 `restore_backup`），
/// 防止迁移与恢复并发争用数据目录和 SQLite 句柄。
pub(crate) struct DataMaintenanceGuard {
    armed: bool,
}

impl DataMaintenanceGuard {
    /// 进程即将重启时保留锁态直到进程退出，防止重启前 600ms 窗口内并发修改数据目录。
    pub(crate) fn keep_locked_until_exit(mut self) {
        self.armed = false;
    }
}

impl Drop for DataMaintenanceGuard {
    fn drop(&mut self) {
        if self.armed {
            DATA_MAINTENANCE_IN_PROGRESS.store(false, Ordering::Release);
        }
    }
}

pub(crate) fn try_acquire_data_maintenance_lock() -> Result<DataMaintenanceGuard, AppError> {
    DATA_MAINTENANCE_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| DataMaintenanceGuard { armed: true })
        .map_err(|_| AppError::Config("当前正在执行数据目录迁移或备份恢复，请稍候再试".to_string()))
}

/// 受管本机数据目录条目（用于 `change_data_dir` 目录迁移与旧目录清理）。
///
/// 注：可移植备份包（`commands/backup.rs`）使用独立白名单（仅 `workreview.db` 与脱敏后的
/// `config.json`），不会把含明文 Bearer Token 的 `mcp_clients.json` 导出到备份归档中。
const MANAGED_DATA_ENTRIES: &[&str] = &[
    "config.json",
    "config.json.bak",
    "workreview.db",
    "screenshots",
    "ocr_logs",
    "background.jpg",
    "update_settings.json",
    "skills",
    "agent_routines.json",
    "mcp_clients.json",
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
    validate_config_endpoints(&config)?;
    update_app_config(app, state.inner(), move |current| {
        Ok(preserve_server_owned_fields(config, current))
    })
    .map(|_| ())
}

/// 由后端维护、前端草稿不应覆盖的字段：设置页持有的是打开时的配置快照，
/// 期间在聊天中完成的 Bot 发送者绑定 / Telegram 绑定码消费、桌宠实时拖拽坐标及待跟进清单
/// 若被整份写回就会丢失（绑定码也会"复活"）。
fn preserve_server_owned_fields(mut incoming: AppConfig, current: &AppConfig) -> AppConfig {
    incoming.bot_sender_bindings = current.bot_sender_bindings.clone();
    incoming.telegram_bot_bind_code = current.telegram_bot_bind_code.clone();
    incoming.telegram_bot_bind_code_expires_at = current.telegram_bot_bind_code_expires_at;
    incoming.avatar_x = current.avatar_x;
    incoming.avatar_y = current.avatar_y;
    incoming.avatar_followups = current.avatar_followups.clone();
    incoming
}

/// 模型端点安全校验：远程端点必须使用 https，明文 http 仅允许本机/内网（Ollama 等）
fn validate_config_endpoints(config: &AppConfig) -> Result<(), AppError> {
    super::ai::validate_model_endpoint(&config.text_model.endpoint)?;
    super::ai::validate_model_endpoint(&config.vision_model.endpoint)?;
    super::ai::validate_model_endpoint(&config.ai_provider.endpoint)?;
    for profile in &config.text_model_profiles {
        super::ai::validate_model_endpoint(&profile.model_config.endpoint)?;
    }
    for cached in config.text_model_provider_cache.values() {
        super::ai::validate_model_endpoint(&cached.endpoint)?;
    }
    // 嵌入服务会收到活动文本（含 OCR）与 embedding_api_key，同样禁止公网明文 http
    super::ai::validate_model_endpoint(&config.embedding_endpoint)?;
    Ok(())
}

/// 动态 Map 类字段（如 `HashMap<String, ModelConfig>`）在局部补丁时整体替换，
/// 避免对象深度递归合并导致无法删除键或清空缓存。
const MAP_REPLACE_KEYS: &[&str] = &["text_model_provider_cache"];

/// 深度合并：对象逐键递归，动态 Map 与其它值（含数组与 null）整体替换。
fn merge_json_value(base: &mut serde_json::Value, patch: &serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                if MAP_REPLACE_KEYS.contains(&key.as_str()) {
                    base_map.insert(key.clone(), value.clone());
                    continue;
                }
                match base_map.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge_json_value(existing, value)
                    }
                    _ => {
                        base_map.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// 把局部配置合并到当前配置上（纯函数）。未知的顶层字段直接报错，避免拼写错误被静默忽略。
pub(crate) fn apply_config_patch(
    current: &AppConfig,
    patch: &serde_json::Value,
) -> Result<AppConfig, AppError> {
    let patch_map = patch
        .as_object()
        .ok_or_else(|| AppError::Config("配置补丁必须是对象".to_string()))?;
    let mut merged = serde_json::to_value(current)
        .map_err(|e| AppError::Unknown(format!("序列化当前配置失败: {e}")))?;
    if let Some(base_map) = merged.as_object() {
        if let Some(unknown) = patch_map.keys().find(|key| !base_map.contains_key(*key)) {
            return Err(AppError::Config(format!("未知的配置字段: {unknown}")));
        }
    }
    merge_json_value(&mut merged, patch);
    serde_json::from_value(merged).map_err(|e| AppError::Config(format!("配置补丁无效: {e}")))
}

/// 局部更新配置：只合并调用方提交的字段，服务端读-改-写，
/// 避免页面持有的旧配置整份写回时覆盖其它位置（托盘、Bot、同步等）刚做的修改。
#[tauri::command]
pub async fn patch_config(
    patch: serde_json::Value,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<AppConfig, AppError> {
    if patch.get("bot_sender_bindings").is_some()
        || patch.get("telegram_bot_bind_code").is_some()
        || patch.get("telegram_bot_bind_code_expires_at").is_some()
        || patch.get("config_synced_at").is_some()
    {
        return Err(AppError::Config(
            "Bot 绑定与同步元数据字段只能通过专用后端命令维护".to_string(),
        ));
    }
    update_app_config(app, state.inner(), |current| {
        let next = apply_config_patch(current, &patch)?;
        validate_config_endpoints(&next)?;
        Ok(next)
    })
}

/// 设置单个应用的隐私级别（`full` 表示移除规则）。服务端读-改-写，不依赖前端持有的配置。
pub(crate) fn apply_app_privacy_level(
    config: &mut AppConfig,
    app_name: &str,
    level: &str,
) -> Result<(), AppError> {
    use work_review_core::config::{AppPrivacyRule, PrivacyLevel};
    let app_name = app_name.trim();
    if app_name.is_empty() {
        return Err(AppError::Config("应用名不能为空".to_string()));
    }
    let target_key = work_review_core::categorize::normalize_display_app_name(app_name)
        .trim()
        .to_lowercase();
    if target_key.is_empty() {
        return Err(AppError::Config("应用名无效".to_string()));
    }
    let level = match level {
        "full" => None,
        "anonymized" => Some(PrivacyLevel::Anonymized),
        "ignored" => Some(PrivacyLevel::Ignored),
        other => return Err(AppError::Config(format!("未知的隐私级别: {other}"))),
    };
    let matches_target = |raw: &str| {
        work_review_core::categorize::normalize_display_app_name(raw.trim())
            .trim()
            .to_lowercase()
            == target_key
    };
    // 按归一化应用名对齐并清理所有别名/大小写重复项以及遗留 excluded_apps
    config
        .privacy
        .excluded_apps
        .retain(|app| !matches_target(app));
    let rules = &mut config.privacy.app_rules;
    match level {
        None => rules.retain(|rule| !matches_target(&rule.app_name)),
        Some(level) => {
            let mut updated = false;
            rules.retain_mut(|rule| {
                if matches_target(&rule.app_name) {
                    if !updated {
                        rule.level = level;
                        updated = true;
                        true
                    } else {
                        false
                    }
                } else {
                    true
                }
            });
            if !updated {
                rules.push(AppPrivacyRule {
                    app_name: app_name.to_string(),
                    level,
                });
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn set_app_privacy_level(
    app_name: String,
    level: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    update_app_config(app, state.inner(), |current| {
        let mut config = current.clone();
        apply_app_privacy_level(&mut config, &app_name, &level)?;
        Ok(config)
    })
    .map(|_| ())
}

#[cfg(test)]
mod patch_tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::{apply_app_privacy_level, apply_config_patch, preserve_server_owned_fields};
    use work_review_core::config::{AppConfig, BotSenderBinding, PrivacyLevel};

    #[test]
    fn 整份保存配置不得覆盖后端维护的bot绑定() {
        let mut current = AppConfig::default();
        current.bot_sender_bindings.insert(
            "feishu".to_string(),
            BotSenderBinding {
                allowed_senders: vec!["ou_new".to_string()],
                ..BotSenderBinding::default()
            },
        );
        // 设置页草稿是打开页面时的快照：绑定码尚未使用、没有授权成员
        let mut stale_draft = AppConfig::default();
        stale_draft.theme = "dark".to_string();
        stale_draft.bot_sender_bindings.insert(
            "feishu".to_string(),
            BotSenderBinding {
                bind_code: Some("WR-OLD".to_string()),
                bind_code_expires_at: Some(i64::MAX),
                ..BotSenderBinding::default()
            },
        );
        let saved = preserve_server_owned_fields(stale_draft, &current);
        assert_eq!(saved.theme, "dark");
        let binding = &saved.bot_sender_bindings["feishu"];
        assert_eq!(binding.allowed_senders, vec!["ou_new".to_string()]);
        assert!(binding.bind_code.is_none());
    }

    #[test]
    fn 局部更新只改提交的字段() {
        let mut current = AppConfig::default();
        current.theme = "light".to_string();
        current.telegram_bot_allowed_chat_ids = vec![42];
        let next = apply_config_patch(
            &current,
            &serde_json::json!({ "daily_report_custom_prompt": "新提示词" }),
        )
        .unwrap();
        assert_eq!(next.daily_report_custom_prompt, "新提示词");
        assert_eq!(next.theme, "light");
        assert_eq!(next.telegram_bot_allowed_chat_ids, vec![42]);
    }

    #[test]
    fn 局部更新嵌套对象应逐键合并() {
        let mut current = AppConfig::default();
        current.storage.jpeg_quality = 70;
        let next = apply_config_patch(
            &current,
            &serde_json::json!({ "storage": { "screenshot_retention_days": 3 } }),
        )
        .unwrap();
        assert_eq!(next.storage.screenshot_retention_days, 3);
        assert_eq!(next.storage.jpeg_quality, 70);
    }

    #[test]
    fn 局部更新应拒绝未知字段与类型错误() {
        let current = AppConfig::default();
        assert!(apply_config_patch(&current, &serde_json::json!({ "no_such_field": 1 })).is_err());
        assert!(apply_config_patch(&current, &serde_json::json!({ "theme": 123 })).is_err());
        assert!(apply_config_patch(&current, &serde_json::json!([1, 2])).is_err());
    }

    #[test]
    fn 局部更新可把可选字段置空() {
        let mut current = AppConfig::default();
        current.daily_report_system_prompt_override = Some("override".to_string());
        let next = apply_config_patch(
            &current,
            &serde_json::json!({ "daily_report_system_prompt_override": null }),
        )
        .unwrap();
        assert!(next.daily_report_system_prompt_override.is_none());
    }

    #[test]
    fn 设置应用隐私级别应新增更新与移除规则() {
        let mut config = AppConfig::default();
        apply_app_privacy_level(&mut config, "WeChat", "ignored").unwrap();
        let level = |config: &AppConfig| {
            config
                .privacy
                .app_rules
                .iter()
                .find(|rule| rule.app_name == "WeChat")
                .map(|rule| rule.level)
        };
        assert_eq!(level(&config), Some(PrivacyLevel::Ignored));
        apply_app_privacy_level(&mut config, "WeChat", "anonymized").unwrap();
        assert_eq!(level(&config), Some(PrivacyLevel::Anonymized));
        apply_app_privacy_level(&mut config, "WeChat", "full").unwrap();
        assert_eq!(level(&config), None);
        assert!(apply_app_privacy_level(&mut config, "WeChat", "bogus").is_err());
        assert!(apply_app_privacy_level(&mut config, "  ", "ignored").is_err());

        // 大小写不同视为同一应用：不新增重复规则，也能正确移除
        apply_app_privacy_level(&mut config, "WeChat", "ignored").unwrap();
        apply_app_privacy_level(&mut config, "wechat", "anonymized").unwrap();
        let count = config
            .privacy
            .app_rules
            .iter()
            .filter(|rule| rule.app_name.eq_ignore_ascii_case("wechat"))
            .count();
        assert_eq!(count, 1);
        apply_app_privacy_level(&mut config, "WECHAT", "full").unwrap();
        assert_eq!(level(&config), None);
    }
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

pub(crate) fn path_for_display(path: &Path) -> String {
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

pub(crate) fn remove_migration_path(path: &Path) -> Result<(), AppError> {
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

pub(crate) fn migration_sibling_path(target_dir: &Path, label: &str) -> Result<PathBuf, AppError> {
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
) -> Result<(), AppError> {
    let mut restored = true;
    for entry_name in installed_entries.iter().rev() {
        if let Err(error) = remove_migration_path(&target_dir.join(entry_name)) {
            log::error!("回滚迁移新条目 {} 失败: {error}", entry_name);
            restored = false;
        }
    }
    for entry_name in moved_entries.iter().rev() {
        let backup_path = backup_dir.join(entry_name);
        if !backup_path.exists() {
            continue;
        }
        if let Err(error) = std::fs::rename(&backup_path, target_dir.join(entry_name)) {
            log::error!("恢复迁移旧条目 {} 失败: {error}", entry_name);
            restored = false;
        }
    }
    // 只允许删除空备份目录，回滚不完整时绝不能递归清理剩余的旧数据。
    if !restored || std::fs::remove_dir(backup_dir).is_err() {
        return Err(AppError::Config(format!(
            "未能完整回滚，原目标数据的备份保留在 {}，请勿删除该目录",
            backup_dir.display()
        )));
    }
    Ok(())
}

pub(crate) fn commit_migration_stage(
    stage_dir: &Path,
    target_dir: &Path,
    preserved_entries: &[&str],
) -> Result<(), AppError> {
    let entry_names: Vec<_> = migration_entry_names()
        .into_iter()
        .filter(|name| !preserved_entries.contains(name))
        .collect();
    let backup_dir = migration_sibling_path(target_dir, "migration-backup")?;
    std::fs::create_dir_all(&backup_dir)?;
    let mut moved_entries = Vec::new();
    let mut installed_entries = Vec::new();

    let committed: Result<(), AppError> = (|| {
        for &entry_name in &entry_names {
            let source = target_dir.join(entry_name);
            if source.exists() {
                std::fs::rename(&source, backup_dir.join(entry_name))?;
                moved_entries.push(entry_name.to_string());
            }
        }
        for &entry_name in &entry_names {
            let source = stage_dir.join(entry_name);
            if source.exists() {
                std::fs::rename(&source, target_dir.join(entry_name))?;
                installed_entries.push(entry_name.to_string());
            }
        }
        Ok(())
    })();
    if let Err(error) = committed {
        let restored =
            restore_migration_entries(target_dir, &backup_dir, &moved_entries, &installed_entries);
        let _ = remove_migration_path(stage_dir);
        if let Err(rollback_error) = restored {
            return Err(AppError::Config(format!(
                "迁移失败：{error}；{rollback_error}"
            )));
        }
        return Err(error);
    }

    if let Err(error) = remove_migration_path(&backup_dir) {
        log::warn!("清理迁移备份目录失败（数据已切换成功）: {error}");
    }
    if let Err(error) = remove_migration_path(stage_dir) {
        log::warn!("清理迁移临时目录失败（数据已切换成功）: {error}");
    }
    Ok(())
}

pub(crate) fn copy_managed_data_without_live_db(
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
    let _maintenance_guard = try_acquire_data_maintenance_lock()?;
    let requested_dir = target_dir.trim();
    if requested_dir.is_empty() {
        return Err(AppError::Config("目标目录不能为空".to_string()));
    }

    let requested_path = to_absolute_path(Path::new(requested_dir))?;
    crate::data_dir::validate_data_dir_target(&requested_path)?;
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

    // 迁移窗口期暂停录制，防止后台采集线程向即将替换的旧库继续写入活动导致迁移丢数。
    struct RecordingPauseGuard {
        state: Arc<Mutex<AppState>>,
        previous_paused: bool,
    }
    impl Drop for RecordingPauseGuard {
        fn drop(&mut self) {
            if let Ok(mut guard) = self.state.lock() {
                guard.is_paused = self.previous_paused;
            }
        }
    }
    let previous_paused = {
        let mut guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let prev = guard.is_paused;
        guard.is_paused = true;
        prev
    };
    let pause_guard = RecordingPauseGuard {
        state: state.inner().clone(),
        previous_paused,
    };

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

    commit_migration_stage(&staging_dir, &target_dir, &[])?;
    let config_path = target_dir.join("config.json");
    let preference_warning = match crate::save_data_dir_preference(&target_dir) {
        Ok(()) => None,
        Err(error) => {
            log::error!("保存数据目录偏好失败，当前运行仍已切换: {error}");
            Some(format!("；但保存数据目录偏好失败：{error}"))
        }
    };

    // 重新获取锁，仅做轻量状态更新
    let mut state_guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state_guard.database = Database::new(&target_dir.join("workreview.db"))?;
    if let Err(e) = state_guard.database.rebuild_fts_index() {
        log::warn!("迁移后 FTS 索引重建失败: {e}");
    }
    state_guard.privacy_filter = PrivacyFilter::from_config(&config.privacy);
    state_guard.screenshot_service = ScreenshotService::new(&target_dir, &config.storage);
    state_guard
        .screenshot_service
        .update_privacy(&config.privacy);
    state_guard.storage_manager = StorageManager::new(&target_dir, config.storage.clone());
    state_guard.data_dir = target_dir.clone();
    state_guard.config_path = config_path;

    log::info!("数据目录已切换到: {target_dir:?}");
    drop(state_guard);
    // 显式释放暂停守卫，恢复原录制暂停状态并同步通知前端
    drop(pause_guard);
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
        resolve_migration_target, restore_migration_entries,
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
    fn 回滚受阻时必须保留备份并报告恢复路径() {
        let root =
            std::env::temp_dir().join(format!("migration-rollback-{}", uuid::Uuid::new_v4()));
        let target = root.join("target");
        let backup = root.join("backup");
        std::fs::create_dir_all(target.join("config.json")).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("config.json"), b"irreplaceable-old-config").unwrap();
        std::fs::write(target.join("config.json/blocker"), b"user-file").unwrap();
        let error =
            restore_migration_entries(&target, &backup, &["config.json".into()], &[]).unwrap_err();
        assert!(error
            .to_string()
            .contains(&backup.to_string_lossy().to_string()));
        assert_eq!(
            std::fs::read(backup.join("config.json")).unwrap(),
            b"irreplaceable-old-config"
        );
        assert_eq!(
            std::fs::read(target.join("config.json/blocker")).unwrap(),
            b"user-file"
        );

        std::fs::remove_dir_all(target.join("config.json")).unwrap();
        restore_migration_entries(&target, &backup, &["config.json".into()], &[]).unwrap();
        assert!(!backup.exists());
        assert_eq!(
            std::fs::read(target.join("config.json")).unwrap(),
            b"irreplaceable-old-config"
        );
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
        commit_migration_stage(&stage, &target, &[]).unwrap();

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

    #[test]
    fn 数据目录迁移应包含个人技能自主任务与mcp配置() {
        let root = std::env::temp_dir().join(format!(
            "work-review-harness-migration-{}",
            uuid::Uuid::new_v4()
        ));
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join("skills").join("weekly-brief")).unwrap();
        std::fs::write(
            src.join("skills").join("weekly-brief").join("SKILL.md"),
            "# weekly",
        )
        .unwrap();
        std::fs::write(src.join("agent_routines.json"), "[]").unwrap();
        std::fs::write(src.join("mcp_clients.json"), "[]").unwrap();

        super::copy_managed_data_without_live_db(&src, &dst).unwrap();
        assert!(dst
            .join("skills")
            .join("weekly-brief")
            .join("SKILL.md")
            .is_file());
        assert!(dst.join("agent_routines.json").is_file());
        assert!(dst.join("mcp_clients.json").is_file());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 保存旧配置快照不得覆盖桌宠实时坐标与待跟进记录() {
        let current = work_review_core::config::AppConfig {
            avatar_x: Some(1280),
            avatar_y: Some(640),
            avatar_followups: vec![work_review_core::config::AvatarFollowupItem {
                id: "followup-1".to_string(),
                title: "继续收尾".to_string(),
                date: "2026-11-27".to_string(),
                source_app: "Code".to_string(),
                source_title: "avatar.rs".to_string(),
                project_key: "work_review".to_string(),
                created_at: 1_700_000_000,
                status: "open".to_string(),
            }],
            ..Default::default()
        };

        let incoming = work_review_core::config::AppConfig {
            avatar_x: Some(100),
            avatar_y: Some(100),
            avatar_opacity: 0.65,
            ..Default::default()
        };

        let merged = super::preserve_server_owned_fields(incoming, &current);

        assert_eq!(merged.avatar_x, Some(1280));
        assert_eq!(merged.avatar_y, Some(640));
        assert_eq!(merged.avatar_followups.len(), 1);
        assert_eq!(merged.avatar_followups[0].id, "followup-1");
        assert!((merged.avatar_opacity - 0.65).abs() < f64::EPSILON);
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
