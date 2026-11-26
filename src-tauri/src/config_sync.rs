//! 常用应用配置的 WebDAV 同步（最后写入获胜）。
//!
//! 只在远程存储提供商为 WebDAV 且用户打开 `sync_app_config` 时生效。
//! 只上传白名单偏好与隐私规则（见 `SYNCED_PREFERENCE_FIELDS`），密钥、端点、
//! Bot、远程存储、存储策略与本机字段一律不上传、不被远端覆盖；隐私规则只增不减。

use crate::commands::{update_app_config_with_source, ConfigWriteSource};
use crate::remote_upload::{pull_app_config_bytes, push_app_config_bytes, should_sync_app_config};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
use work_review_core::config::AppConfig;
use work_review_core::error::AppError;

/// 串行化 WebDAV 同步推送，避免连续保存并发 PUT 乱序覆盖新快照。
fn push_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// v1：整份 `AppConfig`（含密钥，已废弃，仅兼容读取）。
/// v2：仅白名单偏好 + 隐私规则。旧客户端会拒绝 v2 信封，不会被其覆盖。
const ENVELOPE_FORMAT: u32 = 2;
const LEGACY_ENVELOPE_FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SyncedConfigEnvelope {
    format: u32,
    synced_at: u64,
    config: serde_json::Value,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

pub fn stamp_local_sync(config: &mut AppConfig) {
    if should_sync_app_config(&config.remote_storage) {
        config.config_synced_at = now_ms()
            .max(config.config_synced_at.saturating_add(1))
            .max(1);
    }
}

pub fn spawn_push(config: AppConfig) {
    if !should_sync_app_config(&config.remote_storage) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let _guard = push_lock().lock().await;
        if let Err(error) = push_now(&config).await {
            log::warn!("WebDAV 配置同步上传失败: {error}");
        }
    });
}

fn build_envelope(config: &AppConfig) -> SyncedConfigEnvelope {
    SyncedConfigEnvelope {
        format: ENVELOPE_FORMAT,
        synced_at: config.config_synced_at.max(1),
        config: config.synced_preferences_value(),
    }
}

async fn push_now(config: &AppConfig) -> Result<(), AppError> {
    let bytes = serde_json::to_vec_pretty(&build_envelope(config))
        .map_err(|error| AppError::Unknown(format!("序列化同步配置失败: {error}")))?;
    push_app_config_bytes(&config.remote_storage, bytes).await?;
    log::info!("常用配置已上传到 WebDAV");
    Ok(())
}

pub fn spawn_pull_on_startup(app: AppHandle, state: Arc<Mutex<AppState>>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = pull_and_apply(app, state).await {
            log::warn!("WebDAV 配置同步下载失败: {error}");
        }
    });
}

async fn pull_and_apply(app: AppHandle, state: Arc<Mutex<AppState>>) -> Result<(), AppError> {
    let remote_storage = {
        let guard = state
            .lock()
            .map_err(|error| AppError::Unknown(error.to_string()))?;
        if !should_sync_app_config(&guard.config.remote_storage) {
            return Ok(());
        }
        guard.config.remote_storage.clone()
    };
    let Some(bytes) = pull_app_config_bytes(&remote_storage).await? else {
        return Ok(());
    };
    let envelope: SyncedConfigEnvelope = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::Unknown(format!("解析远端配置失败: {error}")))?;
    if !(LEGACY_ENVELOPE_FORMAT..=ENVELOPE_FORMAT).contains(&envelope.format) {
        return Err(AppError::Unknown(format!(
            "不支持的配置同步格式: {}",
            envelope.format
        )));
    }
    // 旧版 v1 信封含明文密钥：无论是否采纳，都用本机脱敏快照覆盖远端文件。
    let scrub_legacy_remote = envelope.format < ENVELOPE_FORMAT;

    let local_config = {
        let guard = state
            .lock()
            .map_err(|error| AppError::Unknown(error.to_string()))?;
        if !should_sync_app_config(&guard.config.remote_storage) {
            return Ok(());
        }
        guard.config.clone()
    };
    if envelope.synced_at <= local_config.config_synced_at {
        if scrub_legacy_remote {
            log::info!("远端为旧版含密钥的同步文件，改写为脱敏快照");
            let _guard = push_lock().lock().await;
            push_now(&local_config).await?;
        }
        return Ok(());
    }

    // 在同一把状态锁内复查同步开关与时间戳：若期间已关闭同步或本机写入更新版本，直接放弃不落盘。
    // 同步写入不重打时间戳、不回推，避免多设备之间来回弹。
    let remote_preferences = envelope.config;
    let remote_synced_at = envelope.synced_at;
    let mut applied = false;
    let merge_result =
        update_app_config_with_source(app, &state, ConfigWriteSource::Sync, |current| {
            if !should_sync_app_config(&current.remote_storage)
                || remote_synced_at <= current.config_synced_at
            {
                return Err(AppError::Config("SKIP_SYNC_APPLY".to_string()));
            }
            let mut merged = current.clone();
            merged.apply_synced_preferences(&remote_preferences);
            merged.config_synced_at = remote_synced_at;
            applied = true;
            Ok(merged)
        });
    let merged = match merge_result {
        Ok(merged) => merged,
        Err(AppError::Config(ref msg)) if msg == "SKIP_SYNC_APPLY" => return Ok(()),
        Err(err) => return Err(err),
    };
    if applied {
        log::info!("已从 WebDAV 应用更新的常用配置");
    }
    if scrub_legacy_remote && should_sync_app_config(&merged.remote_storage) {
        let _guard = push_lock().lock().await;
        push_now(&merged).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    use crate::remote_upload::should_sync_app_config;
    use work_review_core::config::{RemoteStorageProvider, WebDavConfig};

    #[test]
    fn 同步信封应能往返() {
        let envelope = build_envelope(&AppConfig::default());
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let parsed: SyncedConfigEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.format, ENVELOPE_FORMAT);
        assert_eq!(parsed.synced_at, 1);
        assert!(parsed.config.get("theme").is_some());
    }

    #[test]
    fn 上传信封不得包含密钥与远程存储配置() {
        let mut config = AppConfig::default();
        config.remote_storage.webdav.password = "dav-password".to_string();
        config.telegram_bot_token = Some("telegram-token".to_string());
        config.text_model.api_key = Some("model-key".to_string());
        let text = serde_json::to_string(&build_envelope(&config)).unwrap();
        assert!(!text.contains("dav-password"));
        assert!(!text.contains("telegram-token"));
        assert!(!text.contains("model-key"));
        assert!(!text.contains("remote_storage"));
    }

    #[test]
    fn 旧版整份配置信封仍可解析() {
        let mut legacy_config = AppConfig::default();
        legacy_config.theme = "dark".to_string();
        legacy_config.telegram_bot_token = Some("legacy-remote-token".to_string());
        let legacy = serde_json::json!({
            "format": LEGACY_ENVELOPE_FORMAT,
            "synced_at": 7,
            "config": serde_json::to_value(legacy_config).unwrap(),
        });
        let parsed: SyncedConfigEnvelope = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.format, LEGACY_ENVELOPE_FORMAT);
        let mut local = AppConfig::default();
        local.telegram_bot_token = Some("local-token".to_string());
        local.apply_synced_preferences(&parsed.config);
        // 白名单偏好被采纳，旧信封里的密钥不得覆盖本机
        assert_eq!(local.theme, "dark");
        assert_eq!(local.telegram_bot_token.as_deref(), Some("local-token"));
    }

    #[test]
    fn 仅webdav且打开开关才同步() {
        let mut config = AppConfig::default().remote_storage;
        assert!(!should_sync_app_config(&config));
        config.provider = RemoteStorageProvider::WebDav;
        config.webdav = WebDavConfig {
            url: "https://dav.example.com/dav".to_string(),
            ..WebDavConfig::default()
        };
        config.sync_app_config = true;
        assert!(should_sync_app_config(&config));
    }
}
