//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::AppState;
use reqwest::Url;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;
use work_review_core::error::AppError;

const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/wm94i/Work-Review/releases/latest";

const GITHUB_LATEST_RELEASE_PAGE: &str = "https://github.com/wm94i/Work-Review/releases/latest";
const GITHUB_RELEASES_API: &str =
    "https://api.github.com/repos/wm94i/Work-Review/releases?per_page=100";

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
    pub prerelease: bool,
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
    #[serde(default)]
    pub include_prerelease: bool,
    pub last_check_time: u64,
    #[serde(default = "default_update_check_interval")]
    pub check_interval_hours: u64,
}

#[derive(Deserialize, Debug)]
struct GithubReleaseResponse {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Deserialize, Debug)]
struct GithubReleaseAsset {
    browser_download_url: String,
    digest: Option<String>,
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
    Version::parse(normalized_version).ok()?;

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
    // 确认过的版本只读对应 tag，不能在下载时悄悄回退到另一个 Latest。
    match expected_version {
        Some(version) => build_versioned_updater_endpoint(endpoint, version)
            .into_iter()
            .collect(),
        None => vec![endpoint.to_string()],
    }
}

fn update_asset_matches_manifest(asset: &Url, manifest: &str, version: &str) -> bool {
    let manifest =
        build_versioned_updater_endpoint(manifest, version).unwrap_or_else(|| manifest.to_string());
    let Some((directory, _)) = manifest.rsplit_once('/') else {
        return false;
    };
    asset
        .as_str()
        .strip_prefix(&format!("{directory}/"))
        .is_some_and(|filename| {
            !filename.is_empty()
                && !filename.contains('/')
                && !filename.to_ascii_lowercase().contains("%2f")
                && !filename.to_ascii_lowercase().contains("%5c")
        })
}

fn trusted_asset_digest<'a>(
    release: &'a GithubReleaseResponse,
    asset_url: &Url,
) -> Option<&'a str> {
    let mut official_url = asset_url.as_str();
    for prefix in ["https://gh-proxy.cn/", "https://gh-proxy.com/"] {
        if let Some(url) = official_url.strip_prefix(prefix) {
            official_url = url;
            break;
        }
    }
    release
        .assets
        .iter()
        .find(|asset| asset.browser_download_url == official_url)
        .and_then(|asset| asset.digest.as_deref())
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn verify_asset_digest(bytes: &[u8], expected: &str) -> Result<(), AppError> {
    if !hex::encode(Sha256::digest(bytes)).eq_ignore_ascii_case(expected) {
        return Err(AppError::Unknown(
            "安装包与 GitHub 官方版本摘要不符，已拒绝安装".to_string(),
        ));
    }
    Ok(())
}

fn update_http_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .user_agent("WorkReview-Updater")
        .timeout(Duration::from_secs(UPDATE_REQUEST_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(UPDATE_CONNECT_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::Unknown(format!("创建更新检查客户端失败: {e}")))
}

#[allow(clippy::too_many_arguments)]
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

async fn check_installable_update(
    app: &AppHandle,
    expected_version: Option<&str>,
    include_prerelease: bool,
) -> Option<GithubUpdateInfo> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let mut last_failure = None;
    let mut no_update_source = None;

    for endpoint in UPDATER_JSON_ENDPOINTS {
        let source_label = update_source_label(endpoint);
        for manifest_endpoint in build_updater_manifest_candidates(endpoint, expected_version) {
            let endpoint_url = match Url::parse(&manifest_endpoint) {
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
                    if !is_update_allowed(
                        &current_version,
                        &update.version,
                        include_prerelease,
                        expected_version,
                    ) || !update_asset_matches_manifest(
                        &update.download_url,
                        &manifest_endpoint,
                        &update.version,
                    ) {
                        last_failure = Some(format!(
                            "{source_label}: 更新版本或包地址不符合所选渠道或目标"
                        ));
                        continue;
                    }
                    return Some(GithubUpdateInfo {
                        current_version: update.current_version,
                        prerelease: parse_release_version(&update.version)
                            .is_some_and(|version| !version.pre.is_empty()),
                        release_url: format!(
                            "https://github.com/wm94i/Work-Review/releases/tag/v{}",
                            update.version
                        ),
                        latest_version: update.version,
                        available: true,
                        auto_update_ready: true,
                        body: update.body,
                        source: Some(source_label),
                    });
                }
                Ok(None) if expected_version.is_none() => {
                    no_update_source = Some(source_label.clone());
                }
                Ok(None) => {
                    last_failure = Some(format!("{source_label}: 目标版本没有可安装更新包"));
                }
                Err(error) => {
                    last_failure = Some(format!("{source_label}: 检查可安装更新失败: {error}"));
                }
            }
        }
    }

    if let Some(source) = no_update_source {
        return Some(GithubUpdateInfo {
            prerelease: parse_release_version(&current_version)
                .is_some_and(|version| !version.pre.is_empty()),
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
        log::warn!("安装型更新检查未找到目标更新: {failure}");
    }

    None
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            include_prerelease: false,
            last_check_time: 0,
            check_interval_hours: DEFAULT_UPDATE_CHECK_INTERVAL_HOURS,
        }
    }
}

fn normalize_version(version: &str) -> &str {
    version.trim().trim_start_matches(['v', 'V'])
}

fn parse_release_version(version: &str) -> Option<Version> {
    Version::parse(normalize_version(version)).ok()
}

fn is_update_allowed(
    current: &str,
    candidate: &str,
    include_prerelease: bool,
    expected: Option<&str>,
) -> bool {
    let (Some(current), Some(candidate)) = (
        parse_release_version(current),
        parse_release_version(candidate),
    ) else {
        return false;
    };
    (include_prerelease || candidate.pre.is_empty())
        && candidate.cmp_precedence(&current) == Ordering::Greater
        && expected.is_none_or(|expected| {
            parse_release_version(expected).is_some_and(|version| version == candidate)
        })
}

fn select_release(
    releases: &[GithubReleaseResponse],
    include_prerelease: bool,
) -> Option<&GithubReleaseResponse> {
    releases
        .iter()
        .filter_map(|release| {
            let version = parse_release_version(&release.tag_name)?;
            if release.draft
                || (!include_prerelease && (release.prerelease || !version.pre.is_empty()))
            {
                return None;
            }
            Some((release, version))
        })
        .max_by(|(_, left), (_, right)| left.cmp_precedence(right))
        .map(|(release, _)| release)
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

fn ensure_installer_update_allowed(portable_mode: bool) -> Result<(), AppError> {
    if portable_mode {
        return Err(AppError::Config(
            "便携模式不运行安装版更新器，请从发布页下载新版便携 ZIP，完全退出后替换 EXE 并保留 Data 目录".to_string(),
        ));
    }
    Ok(())
}

/// 正式版优先检查安装清单；测试版需本机启用，并按 SemVer 选择已发布的候选。
#[tauri::command]
pub async fn check_github_update(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<GithubUpdateInfo, AppError> {
    let data_dir = state
        .lock()
        .map_err(|e| AppError::Unknown(e.to_string()))?
        .data_dir
        .clone();
    let settings = load_update_settings_from_dir(&data_dir)?;
    let portable_mode = crate::data_dir::is_portable_mode()?;
    let installer_allowed = ensure_installer_update_allowed(portable_mode).is_ok();
    if installer_allowed && !settings.include_prerelease {
        if let Some(update_info) = check_installable_update(&app, None, false).await {
            return Ok(update_info);
        }
    }

    let client = update_http_client()?;
    let endpoint = if settings.include_prerelease {
        GITHUB_RELEASES_API
    } else {
        GITHUB_LATEST_RELEASE_API
    };
    let response = client
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?;
    let releases = if settings.include_prerelease {
        // shortcut: 查询最近 100 个发行版；超过此范围仍需发现旧发布时间的高版本时再增加分页。
        response.json::<Vec<GithubReleaseResponse>>().await?
    } else {
        vec![response.json::<GithubReleaseResponse>().await?]
    };
    let release = select_release(&releases, settings.include_prerelease)
        .ok_or_else(|| AppError::Unknown("未找到符合所选更新渠道的已发布版本".to_string()))?;
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let latest_version = normalize_version(&release.tag_name).to_string();
    let available = is_update_allowed(
        &current_version,
        &latest_version,
        settings.include_prerelease,
        None,
    );
    if available && installer_allowed {
        if let Some(mut update_info) =
            check_installable_update(&app, Some(&latest_version), settings.include_prerelease).await
        {
            update_info.prerelease |= release.prerelease;
            return Ok(update_info);
        }
    }

    Ok(GithubUpdateInfo {
        current_version,
        prerelease: release.prerelease
            || parse_release_version(&latest_version)
                .is_some_and(|version| !version.pre.is_empty()),
        latest_version,
        available,
        auto_update_ready: false,
        release_url: release.html_url.clone(),
        body: release.body.clone(),
        source: Some(
            if portable_mode {
                "portable-release-api"
            } else {
                "github-release-api"
            }
            .to_string(),
        ),
    })
}

/// 逐个尝试更新源进行在线更新，避免某个代理只返回 updater.json 但下载失败时直接中断。
#[tauri::command]
pub async fn download_and_install_github_update(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
    expected_version: Option<String>,
) -> Result<GithubUpdateInstallResult, AppError> {
    ensure_installer_update_allowed(crate::data_dir::is_portable_mode()?)?;
    let data_dir = state
        .lock()
        .map_err(|e| AppError::Unknown(e.to_string()))?
        .data_dir
        .clone();
    let settings = load_update_settings_from_dir(&data_dir)?;
    let expected = expected_version
        .as_deref()
        .ok_or_else(|| AppError::Unknown("必须先检查并确认目标更新版本".to_string()))?;
    if !is_update_allowed(
        env!("CARGO_PKG_VERSION"),
        expected,
        settings.include_prerelease,
        None,
    ) {
        return Err(AppError::Unknown(
            "目标版本不是新版本，或未启用接收测试版".to_string(),
        ));
    }
    // 镜像提供的有效旧签名不能证明目标版本；摘要只信任 GitHub 官方 HTTPS API。
    let trusted_release = update_http_client()?
        .get(format!(
            "https://api.github.com/repos/wm94i/Work-Review/releases/tags/v{}",
            normalize_version(expected)
        ))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json::<GithubReleaseResponse>()
        .await?;
    if trusted_release.draft
        || (!settings.include_prerelease && trusted_release.prerelease)
        || parse_release_version(&trusted_release.tag_name) != parse_release_version(expected)
    {
        return Err(AppError::Unknown(
            "官方发行版与确认版本或渠道不符".to_string(),
        ));
    }
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
                    if !is_update_allowed(
                        env!("CARGO_PKG_VERSION"),
                        &found_update.version,
                        settings.include_prerelease,
                        Some(expected),
                    ) || !update_asset_matches_manifest(
                        &found_update.download_url,
                        &manifest_endpoint,
                        expected,
                    ) {
                        last_check_error = Some(format!(
                            "{source_label}: 返回版本 {} 与确认版本 {} 或所选渠道不符",
                            found_update.version, expected
                        ));
                        continue;
                    }
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

        let Some(expected_digest) = trusted_asset_digest(&trusted_release, &update.download_url)
        else {
            failures.push(format!(
                "{source_label}: 缺少 GitHub 官方 SHA-256 校验信息，已拒绝安装"
            ));
            continue;
        };

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

        let downloaded = update
            .download(
                move |chunk_length, total_bytes| {
                    downloaded_bytes += chunk_length as u64;
                    let percent = total_bytes.and_then(|total| {
                        downloaded_bytes
                            .saturating_mul(100)
                            .checked_div(total)
                            .map(|percent| percent.min(100))
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
                || {},
            )
            .await;
        let installed = match downloaded {
            Ok(bytes) => verify_asset_digest(&bytes, expected_digest).and_then(|()| {
                emit_update_status(
                    &app,
                    "installing",
                    format!("下载及校验完成，正在安装（{source_label}）..."),
                    Some(source_label.clone()),
                    Some(update.version.clone()),
                    None,
                    None,
                    Some(100),
                );
                update
                    .install(bytes)
                    .map_err(|error| AppError::Unknown(error.to_string()))
            }),
            Err(error) => Err(AppError::Unknown(error.to_string())),
        };
        match installed {
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
    fn 便携模式不得运行安装版更新器而普通安装不受影响() {
        let error = ensure_installer_update_allowed(true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("ZIP"));
        assert!(error.contains("Data"));
        assert!(ensure_installer_update_allowed(false).is_ok());
    }

    #[test]
    fn 确认版本后更新清单必须固定到对应tag而非latest() {
        let candidates = build_updater_manifest_candidates(
            "https://github.com/wm94i/Work-Review/releases/latest/download/updater.json",
            Some("1.0.24"),
        );

        assert_eq!(
            candidates,
            vec![
                "https://github.com/wm94i/Work-Review/releases/download/v1.0.24/updater.json"
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
            ]
        );
    }

    #[test]
    fn 更新设置缺省不得开启测试版且应持久化选择() {
        let legacy: UpdateSettings = serde_json::from_str(
            r#"{"autoCheck":true,"lastCheckTime":123,"checkIntervalHours":24}"#,
        )
        .unwrap();
        assert!(!legacy.include_prerelease);
        assert!(!UpdateSettings::default().include_prerelease);
        let enabled = UpdateSettings {
            include_prerelease: true,
            ..legacy
        };
        let restored: UpdateSettings =
            serde_json::from_str(&serde_json::to_string(&enabled).unwrap()).unwrap();
        assert!(restored.include_prerelease);
        assert_eq!(restored.last_check_time, 123);
    }

    #[test]
    fn 版本比较必须区分rc序号正式版和构建元数据() {
        for (current, candidate, allowed) in [
            ("1.1.3", "1.1.3-rc.3", false),
            ("1.1.3", "1.1.2", false),
            ("1.1.3", "1.1.4-rc.1", true),
            ("1.1.3-rc.3", "1.1.3-rc.10", true),
            ("1.1.3-rc.10", "1.1.3-rc.3", false),
            ("1.1.3-rc.3", "1.1.3", true),
            ("1.1.3+old", "1.1.3+new", false),
            ("1.1.3", "1.1.4-rc.01", false),
            ("1.1.3", "nightly", false),
            ("unknown", "1.1.4", false),
        ] {
            assert_eq!(
                is_update_allowed(current, candidate, true, None),
                allowed,
                "{current} -> {candidate}"
            );
        }
    }

    #[test]
    fn 未开启测试版时检查和安装都应拒绝预发布() {
        assert!(!is_update_allowed("1.1.3", "1.1.4-rc.1", false, None));
        assert!(!is_update_allowed(
            "1.1.3",
            "1.1.4-rc.1",
            false,
            Some("1.1.4-rc.1")
        ));
        assert!(is_update_allowed("1.1.3-rc.3", "1.1.3", false, None));
    }

    #[test]
    fn 下载安装必须匹配确认版本且不可静默换成更高版本() {
        assert!(is_update_allowed("1.1.3", "1.1.4", false, Some("v1.1.4")));
        for candidate in ["1.1.3", "1.1.4-rc.3", "1.1.5"] {
            assert!(!is_update_allowed("1.1.3", candidate, true, Some("1.1.4")));
        }
    }

    #[test]
    fn 已确认rc应使用独立版本清单且拒绝无效版本路径() {
        let endpoint = UPDATER_JSON_ENDPOINTS[0];
        assert_eq!(
            build_updater_manifest_candidates(endpoint, Some("1.1.4-rc.3")),
            vec!["https://github.com/wm94i/Work-Review/releases/download/v1.1.4-rc.3/updater.json"]
        );
        assert!(build_updater_manifest_candidates(endpoint, Some("../bad")).is_empty());
        assert_eq!(
            build_updater_manifest_candidates(endpoint, None),
            vec![endpoint]
        );
    }

    fn release(tag: &str, prerelease: bool, draft: bool) -> GithubReleaseResponse {
        GithubReleaseResponse {
            tag_name: tag.to_string(),
            html_url: String::new(),
            body: None,
            prerelease,
            draft,
            assets: Vec::new(),
        }
    }

    #[test]
    fn 测试版选择应按语义版本而非发布时间或rc字符串排序() {
        let releases = [
            release("v1.1.3-rc.10", true, false),
            release("v9.0.0", false, true),
            release("nightly", false, false),
            release("v1.1.3-rc.3", true, false),
            release("v1.1.2", false, false),
        ];
        assert_eq!(
            select_release(&releases, true).unwrap().tag_name,
            "v1.1.3-rc.10"
        );
        assert_eq!(select_release(&releases, false).unwrap().tag_name, "v1.1.2");
    }

    #[test]
    fn 同版本正式发布应优先于rc且不能伪造预发布标记绕过渠道() {
        let releases = [
            release("v1.1.3-rc.3", true, false),
            release("v1.1.3", false, false),
            release("v1.1.4-rc.1", false, false),
        ];
        assert_eq!(select_release(&releases, false).unwrap().tag_name, "v1.1.3");
        assert_eq!(
            select_release(&releases[..2], true).unwrap().tag_name,
            "v1.1.3"
        );
        assert_eq!(
            select_release(&releases, true).unwrap().tag_name,
            "v1.1.4-rc.1"
        );
        assert!(select_release(&[release("v1.1.4", true, false)], false).is_none());
    }

    #[test]
    fn 清单版本正确但包地址指向旧rc或其他来源时也必须拒绝() {
        let manifest = UPDATER_JSON_ENDPOINTS[0];
        for (url, allowed) in [
            ("https://github.com/wm94i/Work-Review/releases/download/v1.1.4-rc.10/setup.exe", true),
            ("https://github.com/wm94i/Work-Review/releases/download/v1.1.4-rc.3/setup.exe", false),
            ("https://github.com/other/repo/releases/download/v1.1.4-rc.10/setup.exe", false),
            ("http://github.com/wm94i/Work-Review/releases/download/v1.1.4-rc.10/setup.exe", false),
            ("https://example.com/wm94i/Work-Review/releases/download/v1.1.4-rc.10/setup.exe", false),
            ("https://github.com/wm94i/Work-Review/releases/download/v1.1.4-rc.10/sub/setup.exe", false),
            ("https://github.com/wm94i/Work-Review/releases/download/v1.1.4-rc.10/bad%2fname.exe", false),
        ] {
            assert_eq!(
                update_asset_matches_manifest(&Url::parse(url).unwrap(), manifest, "1.1.4-rc.10"),
                allowed,
                "{url}"
            );
        }
        let pinned =
            build_updater_manifest_candidates(UPDATER_JSON_ENDPOINTS[1], Some("1.1.4-rc.10"));
        assert!(update_asset_matches_manifest(
            &Url::parse("https://gh-proxy.cn/https://github.com/wm94i/Work-Review/releases/download/v1.1.4-rc.10/setup.exe").unwrap(),
            &pinned[0],
            "1.1.4-rc.10",
        ));
    }

    #[test]
    fn 摘要只允许官方对应资产且代理不可另选旧包() {
        let bytes = b"new release package";
        let digest = hex::encode(Sha256::digest(bytes));
        let mut trusted = release("v1.1.4-rc.1", true, false);
        trusted.assets.push(GithubReleaseAsset {
            browser_download_url:
                "https://github.com/wm94i/Work-Review/releases/download/v1.1.4-rc.1/setup.exe"
                    .to_string(),
            digest: Some(format!("sha256:{digest}")),
        });
        for prefix in ["", "https://gh-proxy.cn/", "https://gh-proxy.com/"] {
            let url = Url::parse(&format!(
                "{prefix}{}",
                trusted.assets[0].browser_download_url
            ))
            .unwrap();
            assert_eq!(trusted_asset_digest(&trusted, &url), Some(digest.as_str()));
        }
        let other =
            Url::parse("https://github.com/wm94i/Work-Review/releases/download/v1.1.3/setup.exe")
                .unwrap();
        assert!(trusted_asset_digest(&trusted, &other).is_none());
    }

    #[test]
    fn 缺失或伪造格式的官方摘要不能跳过校验() {
        let url =
            Url::parse("https://github.com/wm94i/Work-Review/releases/download/v1.1.4/setup.exe")
                .unwrap();
        for digest in [
            None,
            Some("sha256:bad"),
            Some("sha1:1234"),
            Some("sha256:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"),
        ] {
            let mut trusted = release("v1.1.4", false, false);
            trusted.assets.push(GithubReleaseAsset {
                browser_download_url: url.to_string(),
                digest: digest.map(str::to_string),
            });
            assert!(trusted_asset_digest(&trusted, &url).is_none());
        }
    }

    #[test]
    fn 正确版本路径及旧有效签名也不能替代官方目标包摘要() {
        let new_package = b"authentic new package";
        let old_package = b"authentic old package with an otherwise valid signature";
        let digest = hex::encode(Sha256::digest(new_package));
        assert!(verify_asset_digest(new_package, &digest).is_ok());
        assert!(verify_asset_digest(new_package, &digest.to_ascii_uppercase()).is_ok());
        assert!(verify_asset_digest(old_package, &digest).is_err());
        assert!(verify_asset_digest(b"", &digest).is_err());
    }

    #[test]
    fn 更新源应优先官方_github() {
        assert_eq!(
            UPDATER_JSON_ENDPOINTS.first().copied(),
            Some("https://github.com/wm94i/Work-Review/releases/latest/download/updater.json")
        );
    }
}
