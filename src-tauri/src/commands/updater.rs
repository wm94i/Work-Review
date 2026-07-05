//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::error::AppError;
use crate::AppState;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;

const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/wm94i/Work-Review/releases/latest";

const GITHUB_LATEST_RELEASE_PAGE: &str = "https://github.com/wm94i/Work-Review/releases/latest";

const UPDATE_STATUS_EVENT: &str = "update-status";

const UPDATER_JSON_ENDPOINTS: &[&str] = &[
    "https://github.com/wm94i/Work-Review/releases/latest/download/updater.json",
    "https://gh-proxy.cn/https://github.com/wm94i/Work-Review/releases/latest/download/updater-ghproxy.json",
    "https://gh-proxy.com/https://github.com/wm94i/Work-Review/releases/latest/download/updater-ghp.json",
];

const DEFAULT_UPDATE_CHECK_INTERVAL_HOURS: u64 = 24;

const UPDATE_REQUEST_TIMEOUT_SECS: u64 = 35;

const UPDATE_CONNECT_TIMEOUT_SECS: u64 = 12;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GithubUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub available: bool,
    pub auto_update_ready: bool,
    pub release_url: String,
    pub body: Option<String>,
    pub source: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GithubUpdateInstallResult {
    pub updated: bool,
    pub available: bool,
    pub version: Option<String>,
    pub source: Option<String>,
    pub message: String,
    pub attempted_sources: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct GithubUpdateStatusPayload {
    stage: String,
    message: String,
    source: Option<String>,
    version: Option<String>,
    downloaded_bytes: Option<u64>,
    total_bytes: Option<u64>,
    percent: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettings {
    pub auto_check: bool,
    pub last_check_time: u64,
    #[serde(default = "default_update_check_interval")]
    pub check_interval_hours: u64,
}

#[derive(Deserialize, Debug)]
struct GithubReleaseResponse {
    tag_name: String,
    html_url: String,
    body: Option<String>,
}

fn default_update_check_interval() -> u64 {
    DEFAULT_UPDATE_CHECK_INTERVAL_HOURS
}

fn update_source_label(endpoint: &str) -> String {
    Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_string()))
        .unwrap_or_else(|| endpoint.to_string())
}

fn build_versioned_updater_endpoint(endpoint: &str, version: &str) -> Option<String> {
    let normalized_version = normalize_version(version);
    if normalized_version.is_empty() {
        return None;
    }

    endpoint.contains("releases/latest/download/").then(|| {
        endpoint.replacen(
            "releases/latest/download/",
            &format!("releases/download/v{normalized_version}/"),
            1,
        )
    })
}

fn build_updater_manifest_candidates(
    endpoint: &str,
    expected_version: Option<&str>,
) -> Vec<String> {
    let mut candidates = Vec::new();

    if let Some(expected_version) = expected_version {
        if let Some(versioned_endpoint) =
            build_versioned_updater_endpoint(endpoint, expected_version)
        {
            candidates.push(versioned_endpoint);
        }
    }

    candidates.push(endpoint.to_string());
    candidates.dedup();
    candidates
}

fn emit_update_status(
    app: &AppHandle,
    stage: &str,
    message: impl Into<String>,
    source: Option<String>,
    version: Option<String>,
    downloaded_bytes: Option<u64>,
    total_bytes: Option<u64>,
    percent: Option<u64>,
) {
    let _ = app.emit(
        UPDATE_STATUS_EVENT,
        GithubUpdateStatusPayload {
            stage: stage.to_string(),
            message: message.into(),
            source,
            version,
            downloaded_bytes,
            total_bytes,
            percent,
        },
    );
}

async fn check_installable_update(app: &AppHandle) -> Option<GithubUpdateInfo> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let mut last_failure = None;
    let mut no_update_source = None;

    for endpoint in UPDATER_JSON_ENDPOINTS {
        let source_label = update_source_label(endpoint);
        let endpoint_url = match Url::parse(endpoint) {
            Ok(url) => url,
            Err(error) => {
                last_failure = Some(format!("{source_label}: 解析更新源失败: {error}"));
                continue;
            }
        };

        let updater = match app
            .updater_builder()
            .endpoints(vec![endpoint_url])
            .map(|builder| {
                builder
                    .timeout(Duration::from_secs(UPDATE_REQUEST_TIMEOUT_SECS))
                    .configure_client(|client| {
                        client
                            .connect_timeout(Duration::from_secs(UPDATE_CONNECT_TIMEOUT_SECS))
                            .user_agent("WorkReview-Updater")
                    })
            })
            .and_then(|builder| builder.build())
        {
            Ok(updater) => updater,
            Err(error) => {
                last_failure = Some(format!("{source_label}: 构建更新器失败: {error}"));
                continue;
            }
        };

        match updater.check().await {
            Ok(Some(update)) => {
                return Some(GithubUpdateInfo {
                    current_version: update.current_version,
                    latest_version: update.version,
                    available: true,
                    auto_update_ready: true,
                    release_url: GITHUB_LATEST_RELEASE_PAGE.to_string(),
                    body: update.body,
                    source: Some(source_label),
                });
            }
            Ok(None) => {
                no_update_source = Some(source_label);
                continue;
            }
            Err(error) => {
                last_failure = Some(format!("{source_label}: 检查可安装更新失败: {error}"));
            }
        }
    }

    if let Some(source) = no_update_source {
        return Some(GithubUpdateInfo {
            current_version: current_version.clone(),
            latest_version: current_version,
            available: false,
            auto_update_ready: true,
            release_url: GITHUB_LATEST_RELEASE_PAGE.to_string(),
            body: None,
            source: Some(source),
        });
    }

    if let Some(failure) = last_failure {
        log::warn!("安装型更新检查失败，回退到 GitHub Release API: {failure}");
    }

    None
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            last_check_time: 0,
            check_interval_hours: DEFAULT_UPDATE_CHECK_INTERVAL_HOURS,
        }
    }
}

fn normalize_version(version: &str) -> &str {
    version.trim().trim_start_matches(['v', 'V'])
}

fn parse_version_parts(version: &str) -> Vec<u64> {
    normalize_version(version)
        .split('.')
        .map(|segment| {
            segment
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
        })
        .collect()
}

fn compare_versions(current: &str, latest: &str) -> Ordering {
    let current_parts = parse_version_parts(current);
    let latest_parts = parse_version_parts(latest);
    let max_len = current_parts.len().max(latest_parts.len());

    for index in 0..max_len {
        let current_value = *current_parts.get(index).unwrap_or(&0);
        let latest_value = *latest_parts.get(index).unwrap_or(&0);
        match current_value.cmp(&latest_value) {
            Ordering::Equal => continue,
            other => return other,
        }
    }

    Ordering::Equal
}

fn update_settings_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("update_settings.json")
}

fn load_update_settings_from_dir(data_dir: &Path) -> Result<UpdateSettings, AppError> {
    let settings_path = update_settings_path(data_dir);

    if !settings_path.exists() {
        return Ok(UpdateSettings::default());
    }

    let content = std::fs::read_to_string(&settings_path)
        .map_err(|e| AppError::Unknown(format!("读取更新设置失败: {e}")))?;

    serde_json::from_str(&content).map_err(|e| AppError::Unknown(format!("解析更新设置失败: {e}")))
}

fn save_update_settings_to_dir(data_dir: &Path, settings: &UpdateSettings) -> Result<(), AppError> {
    let settings_path = update_settings_path(data_dir);
    let content = serde_json::to_string_pretty(settings)
        .map_err(|e| AppError::Unknown(format!("序列化更新设置失败: {e}")))?;

    std::fs::write(&settings_path, content)
        .map_err(|e| AppError::Unknown(format!("保存更新设置失败: {e}")))?;

    Ok(())
}

fn should_check_for_updates(settings: &UpdateSettings) -> bool {
    if !settings.auto_check {
        return false;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let interval_hours = if settings.check_interval_hours > 0 {
        settings.check_interval_hours
    } else {
        DEFAULT_UPDATE_CHECK_INTERVAL_HOURS
    };
    let elapsed_hours = now.saturating_sub(settings.last_check_time) / 3600;

    elapsed_hours >= interval_hours
}

/// 获取更新检查设置
#[tauri::command]
pub async fn get_update_settings(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<UpdateSettings, AppError> {
    let data_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.data_dir.clone()
    };

    load_update_settings_from_dir(&data_dir)
}

/// 保存更新检查设置
#[tauri::command]
pub async fn save_update_settings(
    settings: UpdateSettings,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let data_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.data_dir.clone()
    };

    save_update_settings_to_dir(&data_dir, &settings)
}

/// 判断当前是否应自动检查更新
#[tauri::command]
pub async fn should_check_updates(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<bool, AppError> {
    let data_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.data_dir.clone()
    };
    let settings = load_update_settings_from_dir(&data_dir)?;

    Ok(should_check_for_updates(&settings))
}

/// 更新时间检查时间戳
#[tauri::command]
pub async fn update_last_check_time(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let data_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.data_dir.clone()
    };
    let mut settings = load_update_settings_from_dir(&data_dir)?;
    settings.last_check_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    save_update_settings_to_dir(&data_dir, &settings)
}

#[tauri::command]
pub async fn quit_app_for_update(app: AppHandle) -> Result<(), AppError> {
    app.exit(0);
    Ok(())
}

/// 基于 updater.json 优先检查更新；若自动更新元数据暂未就绪，则回退到 GitHub Release API。
#[tauri::command]
pub async fn check_github_update(app: AppHandle) -> Result<GithubUpdateInfo, AppError> {
    let client = reqwest::Client::builder()
        .user_agent("WorkReview-Updater")
        .timeout(Duration::from_secs(UPDATE_REQUEST_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(UPDATE_CONNECT_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::Unknown(format!("创建更新检查客户端失败: {e}")))?;

    if let Some(update_info) = check_installable_update(&app).await {
        return Ok(update_info);
    }

    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let release = client
        .get(GITHUB_LATEST_RELEASE_API)
        .header(reqwest::header::USER_AGENT, "WorkReview-Updater")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json::<GithubReleaseResponse>()
        .await?;

    let latest_version = normalize_version(&release.tag_name).to_string();
    let has_update = compare_versions(&current_version, &latest_version) == Ordering::Less;

    if !has_update {
        return Ok(GithubUpdateInfo {
            current_version,
            latest_version,
            available: false,
            auto_update_ready: false,
            release_url: release.html_url,
            body: release.body,
            source: Some("github-release-api".to_string()),
        });
    }

    Ok(GithubUpdateInfo {
        current_version,
        latest_version,
        available: true,
        auto_update_ready: false,
        release_url: release.html_url,
        body: release.body,
        source: Some("github-release-api".to_string()),
    })
}

/// 逐个尝试更新源进行在线更新，避免某个代理只返回 updater.json 但下载失败时直接中断。
#[tauri::command]
pub async fn download_and_install_github_update(
    app: AppHandle,
    expected_version: Option<String>,
) -> Result<GithubUpdateInstallResult, AppError> {
    let mut attempted_sources = Vec::new();
    let mut failures = Vec::new();

    for endpoint in UPDATER_JSON_ENDPOINTS {
        let source_label = update_source_label(endpoint);
        attempted_sources.push(source_label.clone());

        emit_update_status(
            &app,
            "checking",
            format!("正在检查更新源 {source_label}..."),
            Some(source_label.clone()),
            expected_version.clone(),
            None,
            None,
            None,
        );

        let manifest_candidates =
            build_updater_manifest_candidates(endpoint, expected_version.as_deref());

        let mut update = None;
        let mut last_check_error = None;

        for manifest_endpoint in manifest_candidates {
            let endpoint_url = Url::parse(&manifest_endpoint).map_err(|e| {
                AppError::Unknown(format!("解析更新源失败 ({manifest_endpoint}): {e}"))
            })?;

            let updater = match app
                .updater_builder()
                .endpoints(vec![endpoint_url])
                .map_err(|e| AppError::Unknown(format!("配置更新源失败 ({source_label}): {e}")))?
                .timeout(Duration::from_secs(UPDATE_REQUEST_TIMEOUT_SECS))
                .configure_client(|client| {
                    client
                        .connect_timeout(Duration::from_secs(UPDATE_CONNECT_TIMEOUT_SECS))
                        .user_agent("WorkReview-Updater")
                })
                .build()
            {
                Ok(updater) => updater,
                Err(error) => {
                    last_check_error = Some(format!("{source_label}: 构建更新器失败: {error}"));
                    continue;
                }
            };

            match updater.check().await {
                Ok(Some(found_update)) => {
                    update = Some(found_update);
                    last_check_error = None;
                    break;
                }
                Ok(None) => {
                    last_check_error = Some(format!("{source_label}: 未返回可安装的更新包"));
                }
                Err(error) => {
                    last_check_error = Some(format!("{source_label}: 检查更新失败: {error}"));
                }
            }
        }

        let Some(update) = update else {
            if let Some(error) = last_check_error {
                failures.push(error);
            } else {
                failures.push(format!("{source_label}: 未返回可安装的更新包"));
            }
            continue;
        };

        if let Some(expected) = expected_version.as_deref() {
            if compare_versions(&update.version, expected) == Ordering::Less {
                failures.push(format!(
                    "{source_label}: 返回版本 {}，低于目标版本 {}",
                    update.version, expected
                ));
                continue;
            }
        }

        emit_update_status(
            &app,
            "found",
            format!(
                "发现新版本 {}，准备从 {source_label} 下载...",
                update.version
            ),
            Some(source_label.clone()),
            Some(update.version.clone()),
            None,
            None,
            None,
        );

        let progress_app = app.clone();
        let progress_source = source_label.clone();
        let progress_version = update.version.clone();
        let mut downloaded_bytes = 0_u64;

        let finish_app = app.clone();
        let finish_source = source_label.clone();
        let finish_version = update.version.clone();

        match update
            .download_and_install(
                move |chunk_length, total_bytes| {
                    downloaded_bytes += chunk_length as u64;
                    let percent = total_bytes.and_then(|total| {
                        if total == 0 {
                            None
                        } else {
                            Some(((downloaded_bytes * 100) / total).min(100))
                        }
                    });

                    let message = if let Some(percent) = percent {
                        format!("正在下载更新 {percent}%（{progress_source}）")
                    } else {
                        let mb = ((downloaded_bytes as f64) / 1024.0 / 1024.0).max(0.1);
                        format!("正在下载更新 {mb:.1} MB（{progress_source}）")
                    };

                    emit_update_status(
                        &progress_app,
                        "downloading",
                        message,
                        Some(progress_source.clone()),
                        Some(progress_version.clone()),
                        Some(downloaded_bytes),
                        total_bytes,
                        percent,
                    );
                },
                move || {
                    emit_update_status(
                        &finish_app,
                        "installing",
                        format!("下载完成，正在安装（{finish_source}）..."),
                        Some(finish_source.clone()),
                        Some(finish_version.clone()),
                        None,
                        None,
                        Some(100),
                    );
                },
            )
            .await
        {
            Ok(()) => {
                emit_update_status(
                    &app,
                    "completed",
                    format!("更新安装完成，来源 {source_label}"),
                    Some(source_label.clone()),
                    Some(update.version.clone()),
                    None,
                    None,
                    Some(100),
                );

                return Ok(GithubUpdateInstallResult {
                    updated: true,
                    available: true,
                    version: Some(update.version),
                    source: Some(source_label),
                    message: "在线更新已完成".to_string(),
                    attempted_sources,
                });
            }
            Err(error) => {
                failures.push(format!("{source_label}: 下载或安装失败: {error}"));
                emit_update_status(
                    &app,
                    "retrying",
                    format!("源 {source_label} 更新失败，准备尝试下一个源..."),
                    Some(source_label),
                    Some(update.version.clone()),
                    None,
                    None,
                    None,
                );
            }
        }
    }

    let message = if failures.is_empty() {
        if let Some(expected) = expected_version.as_deref() {
            format!("未找到可用于版本 {expected} 的在线更新源")
        } else {
            "当前未发现可安装的在线更新".to_string()
        }
    } else {
        format!("在线更新失败，已尝试全部更新源：{}", failures.join("；"))
    };

    emit_update_status(
        &app,
        "failed",
        message.clone(),
        None,
        expected_version.clone(),
        None,
        None,
        None,
    );

    Err(AppError::Unknown(message))
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 更新清单候选应优先显式版本地址再回退latest地址() {
        let candidates = build_updater_manifest_candidates(
            "https://github.com/wm94i/Work-Review/releases/latest/download/updater.json",
            Some("1.0.24"),
        );

        assert_eq!(
            candidates,
            vec![
                "https://github.com/wm94i/Work-Review/releases/download/v1.0.24/updater.json"
                    .to_string(),
                "https://github.com/wm94i/Work-Review/releases/latest/download/updater.json"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn 更新清单候选应保留代理前缀并规范化版本号() {
        let candidates = build_updater_manifest_candidates(
            "https://gh-proxy.cn/https://github.com/wm94i/Work-Review/releases/latest/download/updater-ghproxy.json",
            Some("v1.0.24"),
        );

        assert_eq!(
            candidates,
            vec![
                "https://gh-proxy.cn/https://github.com/wm94i/Work-Review/releases/download/v1.0.24/updater-ghproxy.json"
                    .to_string(),
                "https://gh-proxy.cn/https://github.com/wm94i/Work-Review/releases/latest/download/updater-ghproxy.json"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn 更新源应优先官方_github_并放宽超时() {
        assert_eq!(
            UPDATER_JSON_ENDPOINTS.first().copied(),
            Some("https://github.com/wm94i/Work-Review/releases/latest/download/updater.json")
        );
        assert!(UPDATE_REQUEST_TIMEOUT_SECS >= 30);
        assert!(UPDATE_CONNECT_TIMEOUT_SECS >= 10);
    }

}
