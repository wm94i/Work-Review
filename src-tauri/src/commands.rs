use crate::analysis::AppLocale;
use crate::config::{
    AiProvider, AiProviderConfig, AppCategoryRule, AppConfig, AvatarFollowupItem,
    CustomSemanticCategory, ModelConfig, PrivacyConfig, WebsiteSemanticRule,
};
use crate::database::Database;
use crate::database::{
    Activity, AppUsage, BrowserUsage, CategoryUsage, DailyReport, DailyStats, DomainUsage,
    HourlyActivityBucket, HourlyAppBucket, MemorySearchItem, UrlDetail, UrlUsage,
};
use crate::error::AppError;
#[cfg(target_os = "linux")]
use crate::linux_session::{
    current_linux_desktop_environment, current_linux_desktop_session, LinuxDesktopSession,
};
use crate::privacy::PrivacyFilter;
use crate::screenshot::ScreenshotService;
use crate::storage::StorageManager;
use crate::work_intelligence::{
    analyze_intents, build_work_sessions, extract_todos,
    generate_weekly_review as build_weekly_review, IntentAnalysisResult, TodoExtractionResult,
    WeeklyReviewResult, WorkSession,
};
use crate::AppState;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;
use work_review_core::categorize::is_merged_domain;

const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/wm94i/Work_Review/releases/latest";
const GITHUB_LATEST_RELEASE_PAGE: &str = "https://github.com/wm94i/Work_Review/releases/latest";
const UPDATE_STATUS_EVENT: &str = "update-status";
const UPDATER_JSON_ENDPOINTS: &[&str] = &[
    "https://github.com/wm94i/Work_Review/releases/latest/download/updater.json",
    "https://ghproxy.cn/https://github.com/wm94i/Work_Review/releases/latest/download/updater-ghproxy.json",
    "https://ghp.ci/https://github.com/wm94i/Work_Review/releases/latest/download/updater-ghp.json",
];
const DEFAULT_UPDATE_CHECK_INTERVAL_HOURS: u64 = 24;
const UPDATE_REQUEST_TIMEOUT_SECS: u64 = 35;
const UPDATE_CONNECT_TIMEOUT_SECS: u64 = 12;
const MANAGED_DATA_ENTRIES: &[&str] = &[
    "config.json",
    "workreview.db",
    "screenshots",
    "ocr_logs",
    "background.jpg",
    "update_settings.json",
];
const LIVE_DATABASE_FILES: &[&str] = &["workreview.db", "workreview.db-shm", "workreview.db-wal"];
#[cfg(target_os = "linux")]
const GNOME_AVATAR_EXTENSION_UUID: &str = "work-review-avatar-input@workreview.app";
#[cfg(target_os = "linux")]
const GNOME_AVATAR_EXTENSION_METADATA: &str =
    include_str!("../../scripts/gnome-shell/work-review-avatar-input@workreview.app/metadata.json");
#[cfg(target_os = "linux")]
const GNOME_AVATAR_EXTENSION_SOURCE: &str =
    include_str!("../../scripts/gnome-shell/work-review-avatar-input@workreview.app/extension.js");

/// 模型测试结果
#[derive(Serialize, Deserialize, Debug)]
pub struct ModelTestResult {
    pub success: bool,
    pub message: String,
    pub response_time_ms: u64,
    pub model_info: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppCategoryOverviewItem {
    pub app_name: String,
    pub category: String,
    pub total_duration: i64,
    pub is_overridden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAnswer {
    pub answer: String,
    pub references: Vec<MemorySearchItem>,
    pub used_ai: bool,
    pub model_name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantAnswer {
    pub answer: String,
    pub references: Vec<MemorySearchItem>,
    pub used_ai: bool,
    pub model_name: Option<String>,
    pub tool_labels: Vec<String>,
    pub cards: Vec<AssistantCard>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantCard {
    pub kind: String,
    pub title: String,
    pub content: serde_json::Value,
}

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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LinuxSessionSupportInfo {
    pub platform: String,
    pub session_type: String,
    pub desktop_environment: String,
    pub active_window_provider: String,
    pub active_window_supported: bool,
    pub screenshot_supported: bool,
    pub browser_url_support_level: String,
    pub avatar_input_provider: String,
    pub avatar_input_support_level: String,
    pub avatar_keyboard_supported: bool,
    pub avatar_mouse_supported: bool,
    pub gnome_avatar_extension_installed: bool,
    pub gnome_avatar_extension_enabled: bool,
    pub gnome_avatar_extension_needs_relogin: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GnomeAvatarExtensionInstallResult {
    pub installed: bool,
    pub enabled: bool,
    pub requires_relogin: bool,
    pub extension_dir: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AvatarFollowupActionInput {
    pub action: String,
    pub project_key: String,
    pub title: String,
    pub date: String,
    pub source_app: String,
    pub source_title: String,
    pub persona: String,
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

fn resolve_saved_report_metadata(
    configured_mode: &crate::config::AiMode,
    configured_model_name: &str,
    used_ai: bool,
) -> (String, Option<String>) {
    let configured_mode = format!("{configured_mode:?}").to_lowercase();

    match (configured_mode.as_str(), used_ai) {
        ("summary", false) => ("local".to_string(), None),
        ("cloud", false) => ("local".to_string(), None),
        (_, false) => (configured_mode, None),
        _ => {
            let model_name = configured_model_name.trim();
            (
                configured_mode,
                if model_name.is_empty() {
                    None
                } else {
                    Some(model_name.to_string())
                },
            )
        }
    }
}

#[allow(dead_code)]
fn normalize_saved_report_ai_mode(value: &str) -> String {
    value.trim().to_lowercase()
}

fn build_daily_report_export_path(export_dir: &Path, date: &str) -> PathBuf {
    let safe_date = date.replace('/', "-").replace('\\', "-");
    export_dir.join(format!("{safe_date}.md"))
}

fn export_daily_report_markdown(
    export_dir: &Path,
    date: &str,
    content: &str,
) -> Result<(), AppError> {
    std::fs::create_dir_all(export_dir)?;
    let output_path = build_daily_report_export_path(export_dir, date);
    std::fs::write(output_path, content)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn gnome_avatar_extension_install_dir() -> Result<PathBuf, AppError> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))
        .ok_or_else(|| AppError::Unknown("无法定位当前用户的数据目录".to_string()))?;

    Ok(data_home
        .join("gnome-shell")
        .join("extensions")
        .join(GNOME_AVATAR_EXTENSION_UUID))
}

#[cfg(target_os = "linux")]
fn is_gnome_avatar_extension_installed() -> bool {
    gnome_avatar_extension_install_dir()
        .ok()
        .map(|dir| dir.join("metadata.json").exists() && dir.join("extension.js").exists())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn is_gnome_avatar_extension_enabled() -> bool {
    std::process::Command::new("gnome-extensions")
        .args(["list", "--enabled"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim() == GNOME_AVATAR_EXTENSION_UUID)
        })
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn gnome_avatar_extension_needs_relogin(
    desktop_environment: crate::linux_session::LinuxDesktopEnvironment,
    installed: bool,
    enabled: bool,
    avatar_input_support: &crate::avatar_input::LinuxAvatarInputSupport,
) -> bool {
    desktop_environment == crate::linux_session::LinuxDesktopEnvironment::Gnome
        && installed
        && enabled
        && avatar_input_support.provider != "gnome-shell-dbus"
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

fn is_text_model_available(model_config: &ModelConfig) -> bool {
    !model_config.endpoint.trim().is_empty() && !model_config.model.trim().is_empty()
}

/// 从问题中提取时间范围关键词，返回 (date_from, date_to)
fn parse_temporal_range(question: &str) -> (Option<String>, Option<String>) {
    use chrono::{Datelike, Local};

    let normalized = question.trim().to_lowercase();
    let today = Local::now().date_naive();
    let fmt = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();

    // 今天/今日
    if normalized.contains("今天") || normalized.contains("今日") {
        let d = fmt(today);
        return (Some(d.clone()), Some(d));
    }

    // 昨天/昨日
    if normalized.contains("昨天") || normalized.contains("昨日") {
        let d = fmt(today - chrono::Duration::days(1));
        return (Some(d.clone()), Some(d));
    }

    // 前天
    if normalized.contains("前天") {
        let d = fmt(today - chrono::Duration::days(2));
        return (Some(d.clone()), Some(d));
    }

    // 最近N天/近N天/过去N天 — 用 regex 提取数字
    if let Ok(re) = regex::Regex::new(r"(?:最近|近|过去)\s*(\d+)\s*天") {
        if let Some(caps) = re.captures(&normalized) {
            if let Ok(n) = caps[1].parse::<i64>() {
                return (
                    Some(fmt(today - chrono::Duration::days(n))),
                    Some(fmt(today)),
                );
            }
        }
    }

    // 含"最近"但无数字 → 默认 7 天
    if normalized.contains("最近") {
        return (
            Some(fmt(today - chrono::Duration::days(7))),
            Some(fmt(today)),
        );
    }

    // 本周/这周
    if normalized.contains("本周") || normalized.contains("这周") {
        let wd = today.weekday().num_days_from_monday() as i64;
        let monday = today - chrono::Duration::days(wd);
        return (Some(fmt(monday)), Some(fmt(today)));
    }

    // 上周/上一周
    if normalized.contains("上周") || normalized.contains("上一周") {
        let wd = today.weekday().num_days_from_monday() as i64;
        let this_monday = today - chrono::Duration::days(wd);
        let last_monday = this_monday - chrono::Duration::days(7);
        let last_sunday = this_monday - chrono::Duration::days(1);
        return (Some(fmt(last_monday)), Some(fmt(last_sunday)));
    }

    // 本月/这个月
    if normalized.contains("本月") || normalized.contains("这个月") {
        let first = today.with_day(1).unwrap_or(today);
        return (Some(fmt(first)), Some(fmt(today)));
    }

    // 上月/上个月
    if normalized.contains("上月") || normalized.contains("上个月") {
        let first_this = today.with_day(1).unwrap_or(today);
        let last_day_prev = first_this - chrono::Duration::days(1);
        let first_prev = last_day_prev.with_day(1).unwrap_or(last_day_prev);
        return (Some(fmt(first_prev)), Some(fmt(last_day_prev)));
    }

    (None, None)
}

/// Parse a single date from user input for bot commands (e.g. `/report 昨天`).
/// Returns the resolved date string in YYYY-MM-DD format, or the input unchanged.
pub fn resolve_single_date(input: Option<&str>) -> String {
    use chrono::Datelike;

    let s = input.unwrap_or("today").to_lowercase();
    let today = chrono::Local::now().date_naive();
    let fmt = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();

    match s.as_str() {
        "today" | "今天" | "今日" => fmt(today),
        "yesterday" | "昨天" | "昨日" => fmt(today - chrono::Duration::days(1)),
        "前天" => fmt(today - chrono::Duration::days(2)),
        "本周" | "这周" => {
            let wd = today.weekday().num_days_from_monday() as i64;
            fmt(today - chrono::Duration::days(wd))
        }
        "上周" => {
            let wd = today.weekday().num_days_from_monday() as i64;
            let this_monday = today - chrono::Duration::days(wd);
            fmt(this_monday - chrono::Duration::days(7))
        }
        _ => s,
    }
}

fn format_memory_references(references: &[MemorySearchItem]) -> String {
    references
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let source_label = match item.source_type.as_str() {
                "activity" => "活动记录",
                "hourly_summary" => "小时摘要",
                "daily_report" => "日报",
                _ => "记忆",
            };

            let mut parts = vec![format!("{}. [{}] {}", index + 1, source_label, item.title)];
            parts.push(format!("日期: {}", item.date));

            if let Some(app_name) = &item.app_name {
                if !app_name.is_empty() {
                    parts.push(format!("应用: {app_name}"));
                }
            }

            if let Some(browser_url) = &item.browser_url {
                if !browser_url.is_empty() {
                    parts.push(format!(
                        "URL: {}",
                        format_browser_url_for_display(browser_url)
                    ));
                }
            }

            if let Some(duration) = item.duration {
                if duration > 0 {
                    parts.push(format!("时长: {}秒", duration));
                }
            }

            if !item.excerpt.is_empty() {
                parts.push(format!("内容: {}", item.excerpt));
            }

            parts.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_browser_url_for_display(raw_url: &str) -> String {
    let mut output = String::with_capacity(raw_url.len());
    let bytes = raw_url.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            output.push(bytes[index] as char);
            index += 1;
            continue;
        }

        let start = index;
        let mut decoded_bytes = Vec::new();

        while index + 2 < bytes.len() && bytes[index] == b'%' {
            let hex = &raw_url[index + 1..index + 3];
            let Ok(value) = u8::from_str_radix(hex, 16) else {
                break;
            };
            decoded_bytes.push(value);
            index += 3;
        }

        if decoded_bytes.is_empty() {
            output.push('%');
            index = start + 1;
            continue;
        }

        let raw_segment = &raw_url[start..index];
        if !decoded_bytes.iter().any(|byte| *byte >= 0x80) {
            output.push_str(raw_segment);
            continue;
        }

        match String::from_utf8(decoded_bytes) {
            Ok(decoded) => output.push_str(&decoded),
            Err(_) => output.push_str(raw_segment),
        }
    }

    output
}

fn is_low_signal_reference(item: &MemorySearchItem) -> bool {
    let title = item.title.trim().to_lowercase();
    let excerpt = item.excerpt.trim().to_lowercase();

    let menu_terms = [
        "文件",
        "编辑",
        "显示",
        "窗口",
        "帮助",
        "历史记录",
        "书签",
        "标签页",
        "个人资料",
        "记录状态",
        "时间线",
        "日报",
        "window",
        "help",
        "file",
        "edit",
        "view",
    ];

    let menu_hits = menu_terms
        .iter()
        .filter(|term| excerpt.contains(**term))
        .count();
    let path_like_title = title.contains('/') || title.contains('\\');
    let generic_title = title.starts_with("无标题")
        || title.starts_with("untitled")
        || title == "work review"
        || title.starts_with("work review -");
    let browser_shell_title =
        title.contains("google chrome") && item.browser_url.as_deref().unwrap_or("").is_empty();

    menu_hits >= 5 || ((path_like_title || generic_title || browser_shell_title) && menu_hits >= 3)
}

fn filter_reference_items<'a>(
    references: &'a [MemorySearchItem],
    limit: usize,
) -> Vec<&'a MemorySearchItem> {
    references
        .iter()
        .filter(|item| !is_low_signal_reference(item))
        .take(limit)
        .collect()
}

fn build_memory_answer_prompt(question: &str, references: &[MemorySearchItem]) -> String {
    format!(
        "你是一个个人工作记忆助手。请严格基于给定记录回答，不要编造未出现的事实。\n\
如果证据不足，要明确说“不确定”或“记录里没有显示”。\n\
优先回答时间、应用、网站、工作主题和依据。\n\
回答请用中文，结构简洁，可使用短段落或要点。\n\n\
用户问题：{question}\n\n\
相关记录：\n{refs}",
        refs = format_memory_references(references)
    )
}

fn build_fallback_memory_answer(question: &str, references: &[MemorySearchItem]) -> String {
    if references.is_empty() {
        return format!(
            "未找到和“{question}”相关的历史记录。\n\n可尝试换一个关键词，或缩小日期范围后再搜索。"
        );
    }

    let mut answer = String::new();
    answer.push_str("以下是检索到的相关记录。\n\n");

    for item in references.iter().take(5) {
        answer.push_str(&format!("- {}（{}）", item.title, item.date));
        if let Some(app_name) = &item.app_name {
            if !app_name.is_empty() {
                answer.push_str(&format!("，应用：{app_name}"));
            }
        }
        if let Some(browser_url) = &item.browser_url {
            if !browser_url.is_empty() {
                answer.push_str(&format!(
                    "，URL：{}",
                    format_browser_url_for_display(browser_url)
                ));
            }
        }
        if let Some(duration) = item.duration {
            if duration > 0 {
                answer.push_str(&format!("，时长约 {} 秒", duration));
            }
        }
        if !item.excerpt.is_empty() {
            answer.push_str(&format!("。摘要：{}", item.excerpt));
        }
        answer.push('\n');
    }

    answer.push_str("\n当前为基础回答模式，仅基于检索结果做整理，未启用大模型归纳。");
    answer
}

fn assistant_empty_question_message(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::ZhCn => "请输入你想问的问题。",
        AppLocale::ZhTw => "請輸入你想問的問題。",
        AppLocale::En => "Please enter your question.",
    }
}

fn build_assistant_system_prompt(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::ZhCn => {
            "你是 Work Review 的工作助手。你只能基于给定记录回答。请使用简体中文回答，直接回应用户问题，先给结论再给依据。不要提及内部分析步骤，不要编造不存在的事实。"
        }
        AppLocale::ZhTw => {
            "你是 Work Review 的工作助手。你只能基於給定記錄回答。請使用繁體中文回答，直接回應使用者問題，先給結論再給依據。不要提及內部分析步驟，也不要編造不存在的事實。"
        }
        AppLocale::En => {
            "You are the Work Review assistant. Answer only from the provided records. Reply in English, lead with the conclusion, then support it with evidence. Do not mention internal analysis steps and do not invent facts."
        }
    }
}

fn assistant_output_language_requirement(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::ZhCn => "8. 最终回答必须使用简体中文，不要混入英文标题或繁体写法。\n",
        AppLocale::ZhTw => "8. 最終回答必須使用繁體中文，不要混入簡體標題或英文標題。\n",
        AppLocale::En => {
            "8. The final answer must be written in English, including headings and bullets.\n"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantTool {
    Memory,
    Sessions,
    Intents,
    Review,
    Todos,
}

impl AssistantTool {
    fn label(&self) -> &'static str {
        match self {
            AssistantTool::Memory => "记忆检索",
            AssistantTool::Sessions => "Session 聚合",
            AssistantTool::Intents => "意图识别",
            AssistantTool::Review => "周报复盘",
            AssistantTool::Todos => "待办提取",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantQuestionKind {
    StageSummary,
    OutcomeRecap,
    ProcessRecap,
    EvidenceQuery,
    TimeStat,
    Comparison,
    Listing,
    Freeform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantReasoningMode {
    Basic,
    AiEnhanced,
}

impl AssistantQuestionKind {
    fn label(&self, locale: AppLocale) -> &'static str {
        match (locale, self) {
            (AppLocale::En, AssistantQuestionKind::StageSummary) => "stage summary",
            (AppLocale::En, AssistantQuestionKind::OutcomeRecap) => "outcome recap",
            (AppLocale::En, AssistantQuestionKind::ProcessRecap) => "process recap",
            (AppLocale::En, AssistantQuestionKind::EvidenceQuery) => "evidence query",
            (AppLocale::En, AssistantQuestionKind::TimeStat) => "time statistics",
            (AppLocale::En, AssistantQuestionKind::Comparison) => "comparison",
            (AppLocale::En, AssistantQuestionKind::Listing) => "listing",
            (AppLocale::En, AssistantQuestionKind::Freeform) => "freeform",
            (AppLocale::ZhTw, AssistantQuestionKind::StageSummary) => "階段總結",
            (AppLocale::ZhTw, AssistantQuestionKind::OutcomeRecap) => "結果復盤",
            (AppLocale::ZhTw, AssistantQuestionKind::ProcessRecap) => "過程復盤",
            (AppLocale::ZhTw, AssistantQuestionKind::EvidenceQuery) => "依據追問",
            (AppLocale::ZhTw, AssistantQuestionKind::TimeStat) => "時間統計",
            (AppLocale::ZhTw, AssistantQuestionKind::Comparison) => "對比分析",
            (AppLocale::ZhTw, AssistantQuestionKind::Listing) => "清單列舉",
            (AppLocale::ZhTw, AssistantQuestionKind::Freeform) => "自由提問",
            (_, AssistantQuestionKind::StageSummary) => "阶段总结",
            (_, AssistantQuestionKind::OutcomeRecap) => "结果复盘",
            (_, AssistantQuestionKind::ProcessRecap) => "过程复盘",
            (_, AssistantQuestionKind::EvidenceQuery) => "依据追问",
            (_, AssistantQuestionKind::TimeStat) => "时间统计",
            (_, AssistantQuestionKind::Comparison) => "对比分析",
            (_, AssistantQuestionKind::Listing) => "清单列举",
            (_, AssistantQuestionKind::Freeform) => "自由提问",
        }
    }
}

fn build_history_context(history: &[AssistantChatMessage]) -> String {
    history
        .iter()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|message| format!("{}: {}", message.role, message.content.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract contextual time keywords from recent user messages to improve
/// continuity across turns (e.g. "今天", "昨天", "本周", "最近").
fn extract_contextual_entities(history: &[AssistantChatMessage], locale: AppLocale) -> String {
    let zh_keywords = ["今天", "昨天", "本周", "上周", "本月", "上月", "最近"];
    let en_keywords = [
        "today",
        "yesterday",
        "this week",
        "last week",
        "this month",
        "last month",
        "recently",
    ];
    let keywords: &[&str] = match locale {
        AppLocale::En => &en_keywords,
        _ => &zh_keywords,
    };
    let mut found = Vec::new();

    for msg in history.iter().rev().take(6) {
        if msg.role != "user" {
            continue;
        }
        for keyword in keywords {
            if msg.content.contains(keyword) && !found.contains(&(*keyword).to_string()) {
                found.push(keyword.to_string());
            }
        }
    }

    if found.is_empty() {
        String::new()
    } else {
        match locale {
            AppLocale::En => format!("Contextual time keywords mentioned: {}", found.join(", ")),
            AppLocale::ZhTw => format!("上下文提及的時間關鍵詞：{}", found.join("、")),
            AppLocale::ZhCn => format!("上下文提及的时间关键词：{}", found.join("、")),
        }
    }
}

fn is_short_follow_up_question(question: &str) -> bool {
    let trimmed = question.trim();
    let normalized = trimmed.to_lowercase();

    trimmed.chars().count() <= 18
        && [
            "继续",
            "展开",
            "细说",
            "详细",
            "具体",
            "接着",
            "那",
            "这个",
            "这里",
            "这个结论",
            "说说",
            "依据",
        ]
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

fn assistant_reasoning_mode(model_config: Option<&ModelConfig>) -> AssistantReasoningMode {
    if model_config.is_some_and(is_text_model_available) {
        AssistantReasoningMode::AiEnhanced
    } else {
        AssistantReasoningMode::Basic
    }
}

fn build_contextual_query(question: &str, history: &[AssistantChatMessage]) -> String {
    let trimmed = question.trim();
    if trimmed.chars().count() >= 8 && !is_short_follow_up_question(trimmed) {
        return trimmed.to_string();
    }

    let previous_user = history
        .iter()
        .rev()
        .find(|message| message.role == "user" && message.content.trim() != trimmed)
        .map(|message| message.content.trim().to_string());

    if let Some(previous_user) = previous_user {
        format!("{previous_user} {trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn build_question_analysis_context(question: &str, history: &[AssistantChatMessage]) -> String {
    let trimmed = question.trim();
    if history.is_empty() {
        return trimmed.to_lowercase();
    }

    let should_expand = trimmed.chars().count() <= 18
        || [
            "这个",
            "这个结论",
            "这里",
            "这些",
            "它",
            "上面",
            "刚才",
            "继续",
            "展开",
            "依据",
        ]
        .iter()
        .any(|pattern| trimmed.contains(pattern));

    if !should_expand {
        return trimmed.to_lowercase();
    }

    let mut context = build_history_context(history);
    if !context.is_empty() {
        context.push('\n');
    }
    context.push_str(trimmed);
    context.to_lowercase()
}

fn detect_question_kind_from_text(text: &str) -> AssistantQuestionKind {
    let context = text.trim().to_lowercase();

    if context.is_empty() {
        return AssistantQuestionKind::StageSummary;
    }

    let time_stat_patterns = ["花了多少时间", "多少时间", "总时长", "时间分布", "时间占比"];
    if time_stat_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::TimeStat;
    }

    let comparison_patterns = ["对比", "比较", "和上周", "相比", "比上周", "变化", "差异"];
    if comparison_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::Comparison;
    }

    let listing_patterns = ["列出", "列举", "所有", "全部", "哪些", "清单"];
    if listing_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::Listing;
    }

    let evidence_patterns = [
        "依据",
        "证据",
        "怎么得出",
        "怎么判断",
        "为什么这么说",
        "哪些记录",
        "哪条记录",
        "从哪里看",
        "原文",
    ];
    if evidence_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::EvidenceQuery;
    }

    let process_patterns = [
        "过程",
        "怎么推进",
        "时间花在哪",
        "花在哪",
        "节奏",
        "session",
        "工作段",
        "时段",
        "时间线",
        "切换",
        "过程复盘",
    ];
    if process_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::ProcessRecap;
    }

    let outcome_patterns = [
        "结果",
        "产出",
        "完成了什么",
        "推进到哪",
        "进展",
        "交付",
        "没收口",
        "待办",
        "下一步",
        "后续",
        "风险",
        "阻塞",
    ];
    if outcome_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::OutcomeRecap;
    }

    AssistantQuestionKind::StageSummary
}

fn last_user_question_kind(history: &[AssistantChatMessage]) -> Option<AssistantQuestionKind> {
    history
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .map(|message| detect_question_kind_from_text(&message.content))
}

fn infer_question_kind_from_assistant_reply(
    history: &[AssistantChatMessage],
) -> Option<AssistantQuestionKind> {
    let content = history
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && !message.content.trim().is_empty())
        .map(|message| message.content.trim().to_lowercase())?;

    let mut best_kind = AssistantQuestionKind::StageSummary;
    let mut best_score = 0i32;

    let candidates: Vec<(AssistantQuestionKind, &[&str])> = vec![
        (
            AssistantQuestionKind::EvidenceQuery,
            &[
                "## 依据补充",
                "依据",
                "记录",
                "原始记录",
                "证据",
                "哪条记录",
            ],
        ),
        (
            AssistantQuestionKind::ProcessRecap,
            &[
                "## 过程分析",
                "session",
                "工作段",
                "时间花在",
                "推进片段",
                "切换",
            ],
        ),
        (
            AssistantQuestionKind::OutcomeRecap,
            &["待办", "风险", "交付", "结果概览", "收口", "下一步"],
        ),
        (
            AssistantQuestionKind::StageSummary,
            &["结论", "主线", "阶段", "主要做了什么", "工作重心"],
        ),
        (
            AssistantQuestionKind::TimeStat,
            &["## 时间统计", "时长", "时间分布", "占比", "花了多少时间"],
        ),
        (
            AssistantQuestionKind::Comparison,
            &["## 对比分析", "对比", "比较", "变化", "差异", "相比"],
        ),
        (
            AssistantQuestionKind::Listing,
            &["## 清单", "列举", "列出", "所有", "全部", "清单"],
        ),
    ];

    for (kind, patterns) in candidates {
        let score = patterns
            .iter()
            .map(|pattern| {
                if content.contains(pattern) {
                    if pattern.starts_with("## ") {
                        3
                    } else {
                        1
                    }
                } else {
                    0
                }
            })
            .sum::<i32>();

        if score > best_score {
            best_score = score;
            best_kind = kind;
        }
    }

    if best_score > 0 {
        Some(best_kind)
    } else {
        None
    }
}

fn detect_assistant_question_kind_with_mode(
    question: &str,
    history: &[AssistantChatMessage],
    mode: AssistantReasoningMode,
) -> AssistantQuestionKind {
    let trimmed = question.trim();
    let current_kind = detect_question_kind_from_text(trimmed);

    if current_kind == AssistantQuestionKind::EvidenceQuery
        || current_kind == AssistantQuestionKind::TimeStat
        || current_kind == AssistantQuestionKind::Comparison
        || current_kind == AssistantQuestionKind::Listing
    {
        return current_kind;
    }

    if is_short_follow_up_question(trimmed) {
        if mode == AssistantReasoningMode::AiEnhanced {
            if let Some(assistant_kind) = infer_question_kind_from_assistant_reply(history) {
                if assistant_kind != AssistantQuestionKind::StageSummary {
                    return assistant_kind;
                }
            }
        }

        if let Some(previous_kind) = last_user_question_kind(history) {
            return previous_kind;
        }
    }

    let context = build_question_analysis_context(question, history);
    let contextual_kind = detect_question_kind_from_text(&context);
    if contextual_kind != AssistantQuestionKind::StageSummary {
        return contextual_kind;
    }

    current_kind
}

#[allow(dead_code)]
fn detect_assistant_question_kind(
    question: &str,
    history: &[AssistantChatMessage],
) -> AssistantQuestionKind {
    detect_assistant_question_kind_with_mode(question, history, AssistantReasoningMode::Basic)
}

fn unique_assistant_tools(tools: Vec<AssistantTool>) -> Vec<AssistantTool> {
    let mut unique = Vec::new();
    for tool in tools {
        if !unique.contains(&tool) {
            unique.push(tool);
        }
    }
    unique
}

fn map_question_kind_to_tools(kind: AssistantQuestionKind) -> Vec<AssistantTool> {
    match kind {
        AssistantQuestionKind::StageSummary => {
            vec![
                AssistantTool::Review,
                AssistantTool::Intents,
                AssistantTool::Memory,
            ]
        }
        AssistantQuestionKind::OutcomeRecap => vec![
            AssistantTool::Review,
            AssistantTool::Intents,
            AssistantTool::Todos,
            AssistantTool::Memory,
        ],
        AssistantQuestionKind::ProcessRecap => {
            vec![
                AssistantTool::Sessions,
                AssistantTool::Intents,
                AssistantTool::Memory,
            ]
        }
        AssistantQuestionKind::EvidenceQuery => vec![
            AssistantTool::Memory,
            AssistantTool::Sessions,
            AssistantTool::Intents,
        ],
        AssistantQuestionKind::TimeStat => vec![
            AssistantTool::Intents,
            AssistantTool::Sessions,
            AssistantTool::Memory,
        ],
        AssistantQuestionKind::Comparison => vec![
            AssistantTool::Review,
            AssistantTool::Intents,
            AssistantTool::Sessions,
            AssistantTool::Memory,
        ],
        AssistantQuestionKind::Listing => vec![
            AssistantTool::Sessions,
            AssistantTool::Intents,
            AssistantTool::Memory,
        ],
        AssistantQuestionKind::Freeform => vec![
            AssistantTool::Memory,
            AssistantTool::Sessions,
            AssistantTool::Intents,
            AssistantTool::Review,
            AssistantTool::Todos,
        ],
    }
}

fn detect_assistant_tools_with_history(
    question: &str,
    history: &[AssistantChatMessage],
    mode: AssistantReasoningMode,
) -> Vec<AssistantTool> {
    let kind = detect_assistant_question_kind_with_mode(question, history, mode);
    let context = build_question_analysis_context(question, history);
    let mut tools = map_question_kind_to_tools(kind);

    if ["session", "工作段", "时段", "切换"]
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        tools.push(AssistantTool::Sessions);
    }

    if ["待办", "todo", "后续", "下一步", "没收口"]
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        tools.push(AssistantTool::Todos);
    }

    if ["复盘", "总结", "回顾", "这周", "本周", "上周"]
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        tools.push(AssistantTool::Review);
    }

    if ["重心", "方向", "主要工作", "主要做了什么"]
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        tools.push(AssistantTool::Intents);
    }

    tools.push(AssistantTool::Memory);
    unique_assistant_tools(tools)
}

fn summarize_sessions_for_prompt(sessions: &[WorkSession]) -> String {
    if sessions.is_empty() {
        return "无 session 数据".to_string();
    }

    sessions
        .iter()
        .take(6)
        .enumerate()
        .map(|(index, session)| {
            format!(
                "{}. {} {}-{}，{}，主应用：{}，意图：{}，标题：{}",
                index + 1,
                session.date,
                session.start_timestamp,
                session.end_timestamp,
                crate::analysis::format_duration(session.duration),
                session.dominant_app,
                session.intent_label,
                session.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_intents_for_prompt(result: &IntentAnalysisResult) -> String {
    if result.summary.is_empty() {
        return "无意图识别结果".to_string();
    }

    result
        .summary
        .iter()
        .take(6)
        .map(|item| {
            format!(
                "- {}：{}，{} 段",
                item.label,
                crate::analysis::format_duration(item.duration),
                item.session_count
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_todos_for_prompt(result: &TodoExtractionResult) -> String {
    if result.items.is_empty() {
        return "无待办提取结果".to_string();
    }

    result
        .items
        .iter()
        .take(8)
        .map(|item| {
            format!(
                "- {}（{}，{}，置信度 {}，{}）",
                item.title, item.date, item.source_app, item.confidence, item.reason
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_assistant_prompt(
    question: &str,
    question_kind: AssistantQuestionKind,
    history: &[AssistantChatMessage],
    date_from: Option<&str>,
    date_to: Option<&str>,
    references: &[MemorySearchItem],
    sessions: Option<&[WorkSession]>,
    intents: Option<&IntentAnalysisResult>,
    review: Option<&WeeklyReviewResult>,
    todos: Option<&TodoExtractionResult>,
    locale: AppLocale,
) -> String {
    let range = match (date_from, date_to) {
        (Some(start), Some(end)) if start == end => match locale {
            AppLocale::En => format!("{start} (that day)"),
            AppLocale::ZhTw => format!("{start} \u{7576}\u{5929}"),
            AppLocale::ZhCn => format!("{start} \u{5f53}\u{5929}"),
        },
        (Some(start), Some(end)) => match locale {
            AppLocale::En => format!("{start} to {end}"),
            AppLocale::ZhTw => format!("{start} \u{5230} {end}"),
            AppLocale::ZhCn => format!("{start} \u{5230} {end}"),
        },
        (Some(start), None) => match locale {
            AppLocale::En => format!("after {start}"),
            AppLocale::ZhTw => format!("{start} \u{4e4b}\u{5f8c}"),
            AppLocale::ZhCn => format!("{start} \u{4e4b}\u{540e}"),
        },
        (None, Some(end)) => match locale {
            AppLocale::En => format!("before {end}"),
            AppLocale::ZhTw => format!("{end} \u{4e4b}\u{524d}"),
            AppLocale::ZhCn => format!("{end} \u{4e4b}\u{524d}"),
        },
        (None, None) => match locale {
            AppLocale::En => "all available records".to_string(),
            AppLocale::ZhTw => "\u{5168}\u{90e8}\u{53ef}\u{7528}\u{8a18}\u{9304}".to_string(),
            AppLocale::ZhCn => "\u{5168}\u{90e8}\u{53ef}\u{7528}\u{8bb0}\u{5f55}".to_string(),
        },
    };

    let mut prompt = match locale {
        AppLocale::En => format!(
            "User question: {question}\nQuestion type: {}\nData range: {range} (all data below is within this range; information outside the range is unavailable)\n\nAnswer in an analytical-review style. Strictly use the data below; do not fabricate facts. State clearly when evidence is insufficient.\n",
            question_kind.label(locale)
        ),
        AppLocale::ZhTw => format!(
            "\u{4f7f}\u{7528}\u{8005}\u{554f}\u{984c}\u{ff1a}{question}\n\u{554f}\u{984c}\u{985e}\u{578b}\u{ff1a}{}\n\u{8cc7}\u{6599}\u{6642}\u{9593}\u{7bc4}\u{570d}\u{ff1a}{range}\u{ff08}\u{4ee5}\u{4e0b}\u{6240}\u{6709}\u{8cc7}\u{6599}\u{5747}\u{5728}\u{6b64}\u{7bc4}\u{570d}\u{5167}\u{ff0c}\u{8d85}\u{51fa}\u{7bc4}\u{570d}\u{7684}\u{8cc7}\u{8a0a}\u{4e0d}\u{53ef}\u{7528}\u{ff09}\n\n\u{8acb}\u{7528}\u{300c}\u{5206}\u{6790}\u{5fa9}\u{76e4}\u{578b}\u{300d}\u{98a8}\u{683c}\u{76f4}\u{63a5}\u{56de}\u{7b54}\u{3002}\u{56b4}\u{683c}\u{57fa}\u{65bc}\u{4ee5}\u{4e0b}\u{8cc7}\u{6599}\u{ff0c}\u{4e0d}\u{8981}\u{7de8}\u{9020}\u{672a}\u{51fa}\u{73fe}\u{7684}\u{4e8b}\u{5be6}\u{ff1b}\u{8b49}\u{64da}\u{4e0d}\u{8db3}\u{6642}\u{660e}\u{78ba}\u{8aaa}\u{660e}\u{3002}\n",
            question_kind.label(locale)
        ),
        AppLocale::ZhCn => format!(
            "\u{7528}\u{6237}\u{95ee}\u{9898}\u{ff1a}{question}\n\u{95ee}\u{9898}\u{7c7b}\u{578b}\u{ff1a}{}\n\u{6570}\u{636e}\u{65f6}\u{95f4}\u{8303}\u{56f4}\u{ff1a}{range}\u{ff08}\u{4ee5}\u{4e0b}\u{6240}\u{6709}\u{6570}\u{636e}\u{5747}\u{5728}\u{6b64}\u{8303}\u{56f4}\u{5185}\u{ff0c}\u{8d85}\u{51fa}\u{8303}\u{56f4}\u{7684}\u{4fe1}\u{606f}\u{4e0d}\u{53ef}\u{7528}\u{ff09}\n\n\u{8bf7}\u{7528}\u{201c}\u{5206}\u{6790}\u{590d}\u{76d8}\u{578b}\u{201d}\u{98ce}\u{683c}\u{76f4}\u{63a5}\u{56de}\u{7b54}\u{3002}\u{4e25}\u{683c}\u{57fa}\u{4e8e}\u{4ee5}\u{4e0b}\u{6570}\u{636e}\u{ff0c}\u{4e0d}\u{8981}\u{7f16}\u{9020}\u{672a}\u{51fa}\u{73b0}\u{7684}\u{4e8b}\u{5b9e}\u{ff1b}\u{8bc1}\u{636e}\u{4e0d}\u{8db3}\u{65f6}\u{660e}\u{786e}\u{8bf4}\u{660e}\u{3002}\n",
            question_kind.label(locale)
        ),
    };

    let recent_history: Vec<_> = history.iter().rev().take(8).collect::<Vec<_>>();
    if !recent_history.is_empty() {
        prompt.push_str(match locale {
            AppLocale::En => "\n[Conversation context]\n",
            AppLocale::ZhTw => "\n\u{3010}\u{5c0d}\u{8a71}\u{4e0a}\u{4e0b}\u{6587}\u{3011}\n",
            AppLocale::ZhCn => "\n\u{3010}\u{5bf9}\u{8bdd}\u{4e0a}\u{4e0b}\u{6587}\u{3011}\n",
        });
        for msg in recent_history.into_iter().rev() {
            let role_label = match locale {
                AppLocale::En => {
                    if msg.role == "user" {
                        "User"
                    } else {
                        "Assistant"
                    }
                }
                AppLocale::ZhTw => {
                    if msg.role == "user" {
                        "\u{4f7f}\u{7528}\u{8005}"
                    } else {
                        "\u{52a9}\u{624b}"
                    }
                }
                AppLocale::ZhCn => {
                    if msg.role == "user" {
                        "\u{7528}\u{6237}"
                    } else {
                        "\u{52a9}\u{624b}"
                    }
                }
            };
            let content = msg.content.trim();
            let short = if content.chars().count() > 200 {
                format!("{}\u{2026}", content.chars().take(200).collect::<String>())
            } else {
                content.to_string()
            };
            match locale {
                AppLocale::En => prompt.push_str(&format!("{role_label}: {short}\n")),
                _ => prompt.push_str(&format!("{role_label}\u{ff1a}{short}\n")),
            }
        }
    }

    let entity_ctx = extract_contextual_entities(history, locale);
    if !entity_ctx.is_empty() {
        prompt.push_str(&format!("\n{entity_ctx}\n"));
    }

    if !references.is_empty() {
        let filtered_references = filter_reference_items(references, 5);
        prompt.push_str(match locale {
            AppLocale::En => "\n[Related memories]\n",
            AppLocale::ZhTw => "\n【相關記憶】\n",
            AppLocale::ZhCn => "\n【相关记忆】\n",
        });
        if filtered_references.is_empty() {
            prompt.push_str(match locale {
                AppLocale::En => "Direct hits have low signal quality (mostly window titles or menu noise). Prioritize review summaries, intent distributions, and sessions.\n",
                AppLocale::ZhTw => "直接命中的原始記錄區分度不高，多數是視窗標題或選單雜訊，請優先參考階段復盤、意圖分布和工作段。\n",
                AppLocale::ZhCn => "直接命中的原始记录区分度不高，多数是窗口标题或菜单噪声，请优先参考阶段复盘、意图分布和工作段。\n",
            });
        } else {
            let filtered = filtered_references.into_iter().cloned().collect::<Vec<_>>();
            prompt.push_str(&format_memory_references(&filtered));
        }
        prompt.push('\n');
    }

    if let Some(sessions) = sessions {
        prompt.push_str(match locale {
            AppLocale::En => "\n[Work sessions]\n",
            AppLocale::ZhTw => "\n【工作段】\n",
            AppLocale::ZhCn => "\n【工作段】\n",
        });
        prompt.push_str(&summarize_sessions_for_prompt(sessions));
        prompt.push('\n');
    }

    if let Some(intents) = intents {
        prompt.push_str(match locale {
            AppLocale::En => "\n[Intent distribution]\n",
            AppLocale::ZhTw => "\n【意圖分布】\n",
            AppLocale::ZhCn => "\n【意图分布】\n",
        });
        prompt.push_str(&summarize_intents_for_prompt(intents));
        prompt.push('\n');
    }

    if let Some(review) = review {
        prompt.push_str(match locale {
            AppLocale::En => "\n[Phase review]\n",
            AppLocale::ZhTw => "\n【階段復盤】\n",
            AppLocale::ZhCn => "\n【阶段复盘】\n",
        });
        prompt.push_str(&review.markdown);
        prompt.push('\n');
    }

    if let Some(todos) = todos {
        prompt.push_str(match locale {
            AppLocale::En => "\n[To-do items]\n",
            AppLocale::ZhTw => "\n【待辦事項】\n",
            AppLocale::ZhCn => "\n【待办事项】\n",
        });
        prompt.push_str(&summarize_todos_for_prompt(todos));
        prompt.push('\n');
    }

    // 主动洞察：检测异常模式
    let insights = generate_active_insights(intents, review, sessions, locale);
    if !insights.is_empty() {
        prompt.push_str(match locale {
            AppLocale::En => "\n[Active insights]\nThe following anomalies were detected — mention them naturally in your answer:\n",
            AppLocale::ZhTw => "\n【主動洞察】\n以下是從資料中偵測到的異常模式，請在回答中自然提及：\n",
            AppLocale::ZhCn => "\n【主动洞察】\n以下是从数据中检测到的异常模式，请在回答中自然提及：\n",
        });
        for insight in &insights {
            prompt.push_str(&format!("- {insight}\n"));
        }
    }

    let output_requirements = match (locale, question_kind) {
        (AppLocale::ZhCn, AssistantQuestionKind::TimeStat) => {
            "\n输出要求：\n1. 用中文回答，直接回应用户关于时间的问题。\n2. 使用清晰的 Markdown 排版。\n3. 输出结构：`## 结论`（含时长数据摘要）、`## 时间分布`（按意图或 session 拆分）。\n4. 数据不足时明确说明，不要编造。\n5. 不要提及内部分析工具名称。\n"
        }
        (AppLocale::ZhCn, AssistantQuestionKind::Comparison) => {
            "\n输出要求：\n1. 用中文回答，聚焦对比分析。\n2. 使用清晰的 Markdown 排版。\n3. 输出结构：`## 结论`（对比核心差异）、`## 对比分析`（逐维度对比）。\n4. 每个维度说明变化方向和幅度。\n5. 不要提及内部分析工具名称。\n"
        }
        (AppLocale::ZhCn, AssistantQuestionKind::Listing) => {
            "\n输出要求：\n1. 用中文回答，列出所有相关条目。\n2. 使用无序列表，一行一条，保持简洁。\n3. 输出结构：`## 清单`（完整列举）、`## 补充说明`（可选）。\n4. 不要遗漏，但也不要虚构不存在的条目。\n5. 不要提及内部分析工具名称。\n"
        }
        (AppLocale::ZhCn, AssistantQuestionKind::Freeform) => {
            "\n输出要求：\n1. 用中文回答，灵活回应用户问题。\n2. 使用清晰的 Markdown 排版。\n3. 根据问题性质自由组织内容，可包含数据、分析、建议等。\n4. 先给出核心回答，再展开细节。\n5. 不要提及内部分析工具名称。\n6. 不要虚构日期、任务或结果。\n"
        }
        (AppLocale::ZhCn, _) => {
            "\n输出要求：\n1. 用中文回答，直接回应用户的问题，不要泛泛而谈。\n2. 使用清晰的 Markdown 排版（标题、列表、加粗等）。\n3. 必须按以下固定结构输出：`## 结论`、`## 结果概览`、`## 过程分析`、`## 依据补充`、`## 复盘总结`。\n4. 整体风格是\u{201c}先结果，再过程\u{201d}，每个结论自然带上依据，不要写成审计报告。\n5. 列举时使用无序列表，一行一条。\n6. 不要提及内部分析工具名称。\n7. 不要虚构日期、任务或结果。\n"
        }
        (AppLocale::ZhTw, AssistantQuestionKind::TimeStat) => {
            "\n輸出要求：\n1. 請用繁體中文回答，聚焦時間統計。\n2. 使用清楚的 Markdown 排版。\n3. 輸出結構：`## 結論`（含時長數據摘要）、`## 時間分布`（按意圖或 session 拆分）。\n4. 數據不足時明確說明，不要編造。\n5. 不要提及內部分析工具名稱。\n"
        }
        (AppLocale::ZhTw, AssistantQuestionKind::Comparison) => {
            "\n輸出要求：\n1. 請用繁體中文回答，聚焦對比分析。\n2. 使用清楚的 Markdown 排版。\n3. 輸出結構：`## 結論`（對比核心差異）、`## 對比分析`（逐維度對比）。\n4. 每個維度說明變化方向和幅度。\n5. 不要提及內部分析工具名稱。\n"
        }
        (AppLocale::ZhTw, AssistantQuestionKind::Listing) => {
            "\n輸出要求：\n1. 請用繁體中文回答，列出所有相關條目。\n2. 使用無序列表，一行一條，保持簡潔。\n3. 輸出結構：`## 清單`（完整列舉）、`## 補充說明`（可選）。\n4. 不要遺漏，但也不要虛構不存在的條目。\n5. 不要提及內部分析工具名稱。\n"
        }
        (AppLocale::ZhTw, AssistantQuestionKind::Freeform) => {
            "\n輸出要求：\n1. 請用繁體中文回答，靈活回應使用者問題。\n2. 使用清楚的 Markdown 排版。\n3. 根據問題性質自由組織內容，可包含數據、分析、建議等。\n4. 先給出核心回答，再展開細節。\n5. 不要提及內部分析工具名稱。\n6. 不要虛構日期、任務或結果。\n"
        }
        (AppLocale::ZhTw, _) => {
            "\n輸出要求：\n1. 請用繁體中文回答，直接回應使用者問題，不要空泛。\n2. 使用清楚的 Markdown 排版（標題、列表、粗體等）。\n3. 必須按以下固定結構輸出：`## 結論`、`## 結果概覽`、`## 過程分析`、`## 依據補充`、`## 復盤總結`。\n4. 整體風格是先結果、再過程，每個結論都要自然帶出依據。\n5. 列舉時使用無序列表，一行一條。\n6. 不要提及內部分析工具名稱。\n7. 不要虛構日期、任務或結果。\n"
        }
        (AppLocale::En, AssistantQuestionKind::TimeStat) => {
            "\nOutput requirements:\n1. Answer in English, focus on time statistics.\n2. Use clear Markdown formatting.\n3. Use this structure: `## Conclusion` (with duration summary), `## Time Distribution` (by intent or session).\n4. Clearly state when data is insufficient; do not fabricate.\n5. Do not mention internal tool names.\n"
        }
        (AppLocale::En, AssistantQuestionKind::Comparison) => {
            "\nOutput requirements:\n1. Answer in English, focus on comparative analysis.\n2. Use clear Markdown formatting.\n3. Use this structure: `## Conclusion` (core differences), `## Comparison` (dimension-by-dimension).\n4. For each dimension, state direction and magnitude of change.\n5. Do not mention internal tool names.\n"
        }
        (AppLocale::En, AssistantQuestionKind::Listing) => {
            "\nOutput requirements:\n1. Answer in English, list all relevant items.\n2. Use unordered bullet lists, one item per line, keep it concise.\n3. Use this structure: `## Listing` (complete list), `## Notes` (optional).\n4. Do not omit real items or fabricate non-existent ones.\n5. Do not mention internal tool names.\n"
        }
        (AppLocale::En, AssistantQuestionKind::Freeform) => {
            "\nOutput requirements:\n1. Answer in English, respond flexibly to the user's question.\n2. Use clear Markdown formatting.\n3. Organize content freely based on the question nature; may include data, analysis, or suggestions.\n4. Lead with the core answer, then expand with details.\n5. Do not mention internal tool names.\n6. Do not invent dates, tasks, or outcomes.\n"
        }
        (AppLocale::En, _) => {
            "\nOutput requirements:\n1. Answer in English and respond to the user's question directly.\n2. Use clear Markdown formatting with headings, lists, and bold text where helpful.\n3. Use this exact structure: `## Conclusion`, `## Overview`, `## Process Analysis`, `## Evidence`, `## Recap`.\n4. Lead with results first, then explain the process and evidence.\n5. When listing points, use unordered bullets with one point per line.\n6. Do not mention internal tool names.\n7. Do not invent dates, tasks, or outcomes.\n"
        }
    };
    prompt.push_str(output_requirements);

    let few_shot = match (locale, question_kind) {
        (AppLocale::En, AssistantQuestionKind::TimeStat) => {
            "\n[Example]\nUser: How much time did I spend coding?\nAssistant:\n## Conclusion\nCoding-related work accounted for ~45% of total time, mainly in code reviews and feature development.\n## Time Distribution\n- **Code review**: 2h 15m, 28%\n- **Feature dev**: 1h 30m, 19%\n- **Documentation**: 45m, 10%\n\n"
        }
        (AppLocale::En, AssistantQuestionKind::Comparison) => {
            "\n[Example]\nUser: How does this week compare to last week?\nAssistant:\n## Conclusion\nTotal hours increased ~20%, but deep-work sessions decreased.\n## Comparison\n- **Total hours**: 25h this week vs 21h last week\n- **Deep work**: 6 blocks this week vs 9 last week\n- **Context switching**: More frequent this week, may hurt efficiency\n\n"
        }
        (AppLocale::En, AssistantQuestionKind::Listing) => {
            "\n[Example]\nUser: List all completed tasks\nAssistant:\n## Listing\n- Fix login page styling (Apr 25)\n- Refactor user permissions module (Apr 26)\n- Write API documentation (Apr 27)\n- Deploy and verify test environment (Apr 28)\n\n"
        }
        (AppLocale::ZhTw, AssistantQuestionKind::TimeStat) => {
            "\n【示例】\n使用者：花了多少時間在編碼上？\n助手：\n## 結論\n這段時間編碼相關工作約佔總時長的 45%，主要集中在程式碼審查和功能開發。\n## 時間分布\n- **程式碼審查**：2 小時 15 分鐘，佔 28%\n- **功能開發**：1 小時 30 分鐘，佔 19%\n- **文件撰寫**：45 分鐘，佔 10%\n\n"
        }
        (AppLocale::ZhTw, AssistantQuestionKind::Comparison) => {
            "\n【示例】\n使用者：和上週相比有什麼變化？\n助手：\n## 結論\n本週總投入時間比上週增加約 20%，但深度工作時段減少了。\n## 對比分析\n- **總時長**：本週 25 小時 vs 上週 21 小時\n- **深度工作**：本週 6 段 vs 上週 9 段\n- **任務切換**：本週更頻繁，可能影響效率\n\n"
        }
        (AppLocale::ZhTw, AssistantQuestionKind::Listing) => {
            "\n【示例】\n使用者：列出所有完成的任務\n助手：\n## 清單\n- 修復登入頁面的樣式問題（4月25日）\n- 完成使用者權限模組的重構（4月26日）\n- 撰寫 API 介面文件（4月27日）\n- 部署測試環境並驗證（4月28日）\n\n"
        }
        (_, AssistantQuestionKind::TimeStat) => {
            "\n【示例】\n用户：花了多少时间在编码上？\n助手：\n## 结论\n这段时间编码相关工作约占总时长的 45%，主要集中在代码审查和功能开发。\n## 时间分布\n- **代码审查**：2 小时 15 分钟，占 28%\n- **功能开发**：1 小时 30 分钟，占 19%\n- **文档编写**：45 分钟，占 10%\n\n"
        }
        (_, AssistantQuestionKind::Comparison) => {
            "\n【示例】\n用户：和上周相比有什么变化？\n助手：\n## 结论\n本周总投入时间比上周增加约 20%，但深度工作时段减少了。\n## 对比分析\n- **总时长**：本周 25 小时 vs 上周 21 小时\n- **深度工作**：本周 6 段 vs 上周 9 段\n- **任务切换**：本周更频繁，可能影响效率\n\n"
        }
        (_, AssistantQuestionKind::Listing) => {
            "\n【示例】\n用户：列出所有完成的任务\n助手：\n## 清单\n- 修复登录页面的样式问题（4月25日）\n- 完成用户权限模块的重构（4月26日）\n- 编写 API 接口文档（4月27日）\n- 部署测试环境并验证（4月28日）\n\n"
        }
        _ => "",
    };
    prompt.push_str(few_shot);

    prompt.push_str(assistant_output_language_requirement(locale));

    prompt
}

fn build_reference_line(item: &MemorySearchItem) -> String {
    let mut line = format!("- **{}**（{}）", item.title, item.date);
    if let Some(app) = &item.app_name {
        if !app.is_empty() {
            line.push_str(&format!("，{app}"));
        }
    }
    if !item.excerpt.is_empty() {
        let short_excerpt: String = item.excerpt.chars().take(80).collect();
        line.push_str(&format!("：{short_excerpt}"));
        if item.excerpt.chars().count() > 80 {
            line.push('…');
        }
    }
    line
}

#[allow(dead_code)]
fn push_markdown_section(answer: &mut String, title: &str, lines: Vec<String>, empty_text: &str) {
    if lines.is_empty() && empty_text.is_empty() {
        return;
    }

    answer.push_str(title);
    answer.push_str("\n\n");

    if lines.is_empty() {
        answer.push_str(empty_text);
        answer.push_str("\n\n");
        return;
    }

    for line in lines {
        if line.starts_with("- ") || line.starts_with("> ") {
            answer.push_str(&line);
        } else {
            answer.push_str("- ");
            answer.push_str(&line);
        }
        answer.push('\n');
    }
    answer.push('\n');
}

fn collect_reference_lines(references: &[MemorySearchItem], limit: usize) -> Vec<String> {
    filter_reference_items(references, limit)
        .into_iter()
        .map(build_reference_line)
        .collect()
}

fn collect_session_lines(
    sessions: Option<&[WorkSession]>,
    limit: usize,
    locale: AppLocale,
) -> Vec<String> {
    sessions
        .unwrap_or(&[])
        .iter()
        .take(limit)
        .map(|session| match locale {
            AppLocale::En => format!(
                "**{}**: {}, mainly {}, intent: {}",
                session.title,
                crate::analysis::format_duration(session.duration),
                session.dominant_app,
                session.intent_label
            ),
            AppLocale::ZhTw => format!(
                "**{}**：{}，主要使用 {}，意圖為 {}",
                session.title,
                crate::analysis::format_duration(session.duration),
                session.dominant_app,
                session.intent_label
            ),
            AppLocale::ZhCn => format!(
                "**{}**：{}，主要使用 {}，意图为 {}",
                session.title,
                crate::analysis::format_duration(session.duration),
                session.dominant_app,
                session.intent_label
            ),
        })
        .collect()
}

fn collect_intent_lines(
    intents: Option<&IntentAnalysisResult>,
    limit: usize,
    locale: AppLocale,
) -> Vec<String> {
    intents
        .map(|result| {
            let total_duration: i64 = result.summary.iter().map(|item| item.duration).sum();
            result
                .summary
                .iter()
                .take(limit)
                .map(|item| {
                    let pct = if total_duration > 0 {
                        item.duration as f64 / total_duration as f64 * 100.0
                    } else {
                        0.0
                    };
                    match locale {
                        AppLocale::En => format!(
                            "**{}**: {}, {} sessions ({:.0}%)",
                            item.label,
                            crate::analysis::format_duration(item.duration),
                            item.session_count,
                            pct
                        ),
                        AppLocale::ZhTw => format!(
                            "**{}**：{}，{} 段（{:.0}%）",
                            item.label,
                            crate::analysis::format_duration(item.duration),
                            item.session_count,
                            pct
                        ),
                        AppLocale::ZhCn => format!(
                            "**{}**：{}，{} 段（{:.0}%）",
                            item.label,
                            crate::analysis::format_duration(item.duration),
                            item.session_count,
                            pct
                        ),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn collect_todo_lines(todos: Option<&TodoExtractionResult>, limit: usize) -> Vec<String> {
    todos
        .map(|result| {
            result
                .items
                .iter()
                .take(limit)
                .map(|item| format!("**{}**（{}，{}）", item.title, item.date, item.reason))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// 一行摘要：⏱ Xh | 🎯 N 个意图 | 📝 M 条记录
fn build_summary_header(
    review: Option<&WeeklyReviewResult>,
    intents: Option<&IntentAnalysisResult>,
    sessions: Option<&[WorkSession]>,
    locale: AppLocale,
) -> String {
    let mut parts = Vec::new();

    if let Some(review) = review {
        let total_hours = review.total_duration as f64 / 3600.0;
        parts.push(format!("⏱ {:.1}h", total_hours));
        parts.push(match locale {
            AppLocale::En => format!("active {} days", review.active_days),
            AppLocale::ZhTw => format!("活躍 {} 天", review.active_days),
            AppLocale::ZhCn => format!("活跃 {} 天", review.active_days),
        });
    }

    if let Some(intents) = intents {
        let count = intents.summary.len();
        if count > 0 {
            parts.push(match locale {
                AppLocale::En => format!("🎯 {} intents", count),
                AppLocale::ZhTw => format!("🎯 {} 個意圖", count),
                AppLocale::ZhCn => format!("🎯 {} 个意图", count),
            });
        }
    }

    if let Some(sessions) = sessions {
        if !sessions.is_empty() {
            parts.push(match locale {
                AppLocale::En => format!("📝 {} sessions", sessions.len()),
                AppLocale::ZhTw => format!("📝 {} 個 session", sessions.len()),
                AppLocale::ZhCn => format!("📝 {} 个 session", sessions.len()),
            });
        }
    }

    if parts.is_empty() {
        return String::new();
    }

    parts.join(" | ")
}

/// 根据意图类型生成一句话结论
fn build_conclusion_line(
    _question: &str,
    kind: AssistantQuestionKind,
    intent_lines: &[String],
    session_lines: &[String],
    todo_lines: &[String],
    reference_lines: &[String],
    review: Option<&WeeklyReviewResult>,
    locale: AppLocale,
) -> String {
    match kind {
        AssistantQuestionKind::StageSummary => {
            if !intent_lines.is_empty() {
                let top_intents: Vec<&str> = intent_lines
                    .iter()
                    .take(2)
                    .map(|l| {
                        l.split('：')
                            .next()
                            .unwrap_or(l)
                            .trim_start_matches("**")
                            .trim_end_matches("**")
                    })
                    .collect();
                match locale {
                    AppLocale::En => {
                        format!("Main focus areas: **{}**.", top_intents.join("**, **"))
                    }
                    AppLocale::ZhTw => {
                        format!("主線工作集中在 **{}**。", top_intents.join("**、**"))
                    }
                    AppLocale::ZhCn => {
                        format!("主线工作集中在 **{}**。", top_intents.join("**、**"))
                    }
                }
            } else if let Some(r) = review {
                match locale {
                    AppLocale::En => format!(
                        "Total investment: {:.1}h across {} active days.",
                        r.total_duration as f64 / 3600.0,
                        r.active_days
                    ),
                    AppLocale::ZhTw => format!(
                        "總投入 {:.1}h，活躍 {} 天。",
                        r.total_duration as f64 / 3600.0,
                        r.active_days
                    ),
                    AppLocale::ZhCn => format!(
                        "总投入 {:.1}h，活跃 {} 天。",
                        r.total_duration as f64 / 3600.0,
                        r.active_days
                    ),
                }
            } else {
                match locale {
                    AppLocale::En => "Insufficient records to draw a clear conclusion.".to_string(),
                    AppLocale::ZhTw => "當前記錄不足以給出明確結論。".to_string(),
                    AppLocale::ZhCn => "当前记录不足以给出明确结论。".to_string(),
                }
            }
        }
        AssistantQuestionKind::OutcomeRecap => {
            if !todo_lines.is_empty() {
                match locale {
                    AppLocale::En => format!(
                        "Phase progress exists, but {} item(s) still need closure.",
                        todo_lines.len()
                    ),
                    AppLocale::ZhTw => {
                        format!("已有階段性推進，但仍有 {} 項待收口。", todo_lines.len())
                    }
                    AppLocale::ZhCn => {
                        format!("已有阶段性推进，但仍有 {} 项待收口。", todo_lines.len())
                    }
                }
            } else if let Some(r) = review {
                match locale {
                    AppLocale::En => format!(
                        "Completed {} sessions with {} deep-work blocks.",
                        r.session_count, r.deep_work_sessions
                    ),
                    AppLocale::ZhTw => format!(
                        "完成 {} 個 session，深度工作 {} 段。",
                        r.session_count, r.deep_work_sessions
                    ),
                    AppLocale::ZhCn => format!(
                        "完成 {} 个 session，深度工作 {} 段。",
                        r.session_count, r.deep_work_sessions
                    ),
                }
            } else {
                match locale {
                    AppLocale::En => {
                        "Some scattered progress is visible in current records.".to_string()
                    }
                    AppLocale::ZhTw => "當前記錄能看到零散進展。".to_string(),
                    AppLocale::ZhCn => "当前记录能看到零散进展。".to_string(),
                }
            }
        }
        AssistantQuestionKind::ProcessRecap => {
            if !session_lines.is_empty() {
                match locale {
                    AppLocale::En => format!(
                        "{} work sessions found, timeline traceable.",
                        session_lines.len()
                    ),
                    AppLocale::ZhTw => {
                        format!("共 {} 個工作段，時間投入可追溯。", session_lines.len())
                    }
                    AppLocale::ZhCn => {
                        format!("共 {} 个工作段，时间投入可追溯。", session_lines.len())
                    }
                }
            } else {
                match locale {
                    AppLocale::En => {
                        "Not enough continuous sessions to reconstruct the process.".to_string()
                    }
                    AppLocale::ZhTw => "缺少足夠的連續工作段來還原過程。".to_string(),
                    AppLocale::ZhCn => "缺少足够的连续工作段来还原过程。".to_string(),
                }
            }
        }
        AssistantQuestionKind::EvidenceQuery => {
            if !reference_lines.is_empty() {
                match locale {
                    AppLocale::En => format!(
                        "Found {} traceable records as evidence.",
                        reference_lines.len()
                    ),
                    AppLocale::ZhTw => {
                        format!("找到 {} 條可引用的記錄依據。", reference_lines.len())
                    }
                    AppLocale::ZhCn => {
                        format!("找到 {} 条可引用的记录依据。", reference_lines.len())
                    }
                }
            } else {
                match locale {
                    AppLocale::En => "No directly citable records found.".to_string(),
                    AppLocale::ZhTw => "當前沒有檢索到可直接引用的記錄。".to_string(),
                    AppLocale::ZhCn => "当前没有检索到可直接引用的记录。".to_string(),
                }
            }
        }
        AssistantQuestionKind::TimeStat => {
            if !intent_lines.is_empty() {
                let top_names: Vec<&str> = intent_lines
                    .iter()
                    .take(2)
                    .map(|l| {
                        l.split('：')
                            .next()
                            .unwrap_or(l)
                            .trim_start_matches("**")
                            .trim_end_matches("**")
                    })
                    .collect();
                match locale {
                    AppLocale::En => {
                        format!("Time mainly distributed across {}.", top_names.join(", "))
                    }
                    AppLocale::ZhTw => format!("時間主要分布在 {}。", top_names.join("、")),
                    AppLocale::ZhCn => format!("时间主要分布在 {}。", top_names.join("、")),
                }
            } else {
                match locale {
                    AppLocale::En => {
                        "Insufficient records for time distribution statistics.".to_string()
                    }
                    AppLocale::ZhTw => "當前記錄不足以統計時間分布。".to_string(),
                    AppLocale::ZhCn => "当前记录不足以统计时间分布。".to_string(),
                }
            }
        }
        AssistantQuestionKind::Comparison => match locale {
            AppLocale::En => "Comparative analysis based on available data.".to_string(),
            AppLocale::ZhTw => "基於已有數據進行對比分析。".to_string(),
            AppLocale::ZhCn => "基于已有数据进行对比分析。".to_string(),
        },
        AssistantQuestionKind::Listing => {
            let count = session_lines.len() + intent_lines.len();
            if count > 0 {
                match locale {
                    AppLocale::En => format!("Found {count} relevant records."),
                    AppLocale::ZhTw => format!("共找到 {count} 條相關記錄。"),
                    AppLocale::ZhCn => format!("共找到 {count} 条相关记录。"),
                }
            } else {
                match locale {
                    AppLocale::En => "No matching items found in current records.".to_string(),
                    AppLocale::ZhTw => "當前記錄中沒有找到匹配的條目。".to_string(),
                    AppLocale::ZhCn => "当前记录中没有找到匹配的条目。".to_string(),
                }
            }
        }
        AssistantQuestionKind::Freeform => {
            if !intent_lines.is_empty() || !session_lines.is_empty() {
                match locale {
                    AppLocale::En => "Answer based on available records below.".to_string(),
                    AppLocale::ZhTw => "基於當前可用記錄的回答如下。".to_string(),
                    AppLocale::ZhCn => "基于当前可用记录的回答如下。".to_string(),
                }
            } else {
                match locale {
                    AppLocale::En => "Current records are limited.".to_string(),
                    AppLocale::ZhTw => "當前記錄較為有限。".to_string(),
                    AppLocale::ZhCn => "当前记录较为有限。".to_string(),
                }
            }
        }
    }
}

/// 生成带进度条的时间分布
fn build_time_distribution(intents: &IntentAnalysisResult, locale: AppLocale) -> String {
    let total: i64 = intents.summary.iter().map(|i| i.duration).sum();
    if total <= 0 {
        return String::new();
    }

    let mut lines = Vec::new();
    let bar_width = 10;

    for item in intents.summary.iter().take(6) {
        let pct = item.duration as f64 / total as f64;
        let filled = (pct * bar_width as f64).round() as usize;
        let empty = bar_width - filled;
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
        let hours = crate::analysis::format_duration(item.duration);
        let pct_str = format!("{:.0}%", pct * 100.0);
        lines.push(match locale {
            AppLocale::En => format!("{}  {}  {} ({})", item.label, bar, hours, pct_str),
            _ => format!("{}  {}  {}（{}）", item.label, bar, hours, pct_str),
        });
    }

    lines.join("\n")
}

/// 根据当前回答生成追问引导
fn build_followup_hints(
    kind: AssistantQuestionKind,
    intent_lines: &[String],
    session_lines: &[String],
    todo_lines: &[String],
    locale: AppLocale,
) -> String {
    let mut hints = Vec::new();

    match kind {
        AssistantQuestionKind::StageSummary | AssistantQuestionKind::Freeform => {
            if session_lines.len() > 2 {
                hints.push(match locale {
                    AppLocale::En => {
                        "Want details on a specific session? Just tell me the time range."
                            .to_string()
                    }
                    AppLocale::ZhTw => {
                        "想看某個 session 的詳細情況？告訴我時間段即可。".to_string()
                    }
                    AppLocale::ZhCn => {
                        "想看某个 session 的详细情况？告诉我时间段即可。".to_string()
                    }
                });
            }
            if !todo_lines.is_empty() {
                hints.push(match locale {
                    AppLocale::En => "Need me to prioritize your to-do items?".to_string(),
                    AppLocale::ZhTw => "需要我列出待辦事項的優先級嗎？".to_string(),
                    AppLocale::ZhCn => "需要我列出待办事项的优先级吗？".to_string(),
                });
            }
        }
        AssistantQuestionKind::TimeStat => {
            hints.push(match locale {
                AppLocale::En => {
                    "Want time breakdown for a specific app? Tell me the name.".to_string()
                }
                AppLocale::ZhTw => "想看具體某個應用的時間佔比？告訴我應用名。".to_string(),
                AppLocale::ZhCn => "想看具体某个应用的时间占比？告诉我应用名。".to_string(),
            });
            hints.push(match locale {
                AppLocale::En => {
                    "Need to compare time distribution across different periods?".to_string()
                }
                AppLocale::ZhTw => "需要對比不同時間段的時間分布嗎？".to_string(),
                AppLocale::ZhCn => "需要对比不同时间段的时间分布吗？".to_string(),
            });
        }
        AssistantQuestionKind::ProcessRecap => {
            if intent_lines.len() > 1 {
                hints.push(match locale {
                    AppLocale::En => {
                        "Want to drill into specific sessions under an intent?".to_string()
                    }
                    AppLocale::ZhTw => "想深入看某個意圖下的具體 session？".to_string(),
                    AppLocale::ZhCn => "想深入看某个意图下的具体 session？".to_string(),
                });
            }
        }
        AssistantQuestionKind::OutcomeRecap => {
            if !todo_lines.is_empty() {
                hints.push(match locale {
                    AppLocale::En => "Need me to sort to-dos by priority?".to_string(),
                    AppLocale::ZhTw => "需要我把待辦按優先級排序嗎？".to_string(),
                    AppLocale::ZhCn => "需要我把待办按优先级排序吗？".to_string(),
                });
            }
        }
        _ => {}
    }

    if hints.is_empty() {
        hints.push(match locale {
            AppLocale::En => {
                "Ask about a date, time range, app, or specific session for more detail."
                    .to_string()
            }
            AppLocale::ZhTw => "也可以指定日期、時間範圍、應用或具體工作段繼續追問。".to_string(),
            AppLocale::ZhCn => "也可以指定日期、时间范围、应用或具体工作段继续追问。".to_string(),
        });
    }

    let header = match locale {
        AppLocale::En => "**You can follow up:**\n",
        AppLocale::ZhTw => "**可以繼續追問：**\n",
        AppLocale::ZhCn => "**可以继续追问：**\n",
    };
    let mut result = String::from(header);
    for hint in hints.iter().take(2) {
        result.push_str(&format!("- {hint}\n"));
    }
    result
}

fn build_fallback_assistant_answer(
    question: &str,
    question_kind: AssistantQuestionKind,
    references: &[MemorySearchItem],
    sessions: Option<&[WorkSession]>,
    intents: Option<&IntentAnalysisResult>,
    review: Option<&WeeklyReviewResult>,
    todos: Option<&TodoExtractionResult>,
    _tool_labels: &[String],
    locale: AppLocale,
) -> String {
    let mut answer = String::new();
    let intent_lines = collect_intent_lines(intents, 6, locale);
    let session_lines = collect_session_lines(sessions, 8, locale);
    let todo_lines = collect_todo_lines(todos, 5);
    let reference_lines = collect_reference_lines(references, 5);
    let insights = generate_active_insights(intents, review, sessions, locale);

    // === 1. 一行摘要：关键指标 ===
    let summary_line = build_summary_header(review, intents, sessions, locale);
    if !summary_line.is_empty() {
        answer.push_str(&summary_line);
        answer.push_str("\n\n");
    }

    // === 2. 结论：按意图类型给出核心回答 ===
    let conclusion = build_conclusion_line(
        question,
        question_kind,
        &intent_lines,
        &session_lines,
        &todo_lines,
        &reference_lines,
        review,
        locale,
    );
    if !conclusion.is_empty() {
        answer.push_str(match locale {
            AppLocale::En => "## Conclusion\n\n",
            AppLocale::ZhTw => "## 結論\n\n",
            AppLocale::ZhCn => "## 结论\n\n",
        });
        answer.push_str(&conclusion);
        answer.push_str("\n\n");
    }

    // === 3. 时间分布：可视化进度条 ===
    if let Some(intents) = intents {
        if !intents.summary.is_empty() {
            answer.push_str(match locale {
                AppLocale::En => "## Time Distribution\n\n",
                AppLocale::ZhTw => "## 時間分布\n\n",
                AppLocale::ZhCn => "## 时间分布\n\n",
            });
            answer.push_str(&build_time_distribution(intents, locale));
            answer.push_str("\n");
        }
    }

    // === 4. 工作段：按时间线列出 ===
    if !session_lines.is_empty() {
        answer.push_str(match locale {
            AppLocale::En => "## Sessions\n\n",
            AppLocale::ZhTw => "## 工作段\n\n",
            AppLocale::ZhCn => "## 工作段\n\n",
        });
        for line in session_lines.iter().take(6) {
            answer.push_str("- ");
            answer.push_str(line);
            answer.push('\n');
        }
        answer.push('\n');
    }

    // === 5. 待办/风险 ===
    if !todo_lines.is_empty() {
        answer.push_str(match locale {
            AppLocale::En => "## Follow-ups\n\n",
            AppLocale::ZhTw => "## 待跟進\n\n",
            AppLocale::ZhCn => "## 待跟进\n\n",
        });
        for line in todo_lines.iter().take(5) {
            answer.push_str("- ");
            answer.push_str(line);
            answer.push('\n');
        }
        answer.push('\n');
    }

    // === 6. 主动洞察 ===
    if !insights.is_empty() {
        answer.push_str(match locale {
            AppLocale::En => "## Insights\n\n",
            AppLocale::ZhTw => "## 主動洞察\n\n",
            AppLocale::ZhCn => "## 主动洞察\n\n",
        });
        for insight in insights.iter().take(3) {
            answer.push_str("- ");
            answer.push_str(insight);
            answer.push('\n');
        }
        answer.push('\n');
    }

    // === 7. 追问引导 ===
    let hints = build_followup_hints(
        question_kind,
        &intent_lines,
        &session_lines,
        &todo_lines,
        locale,
    );
    if !hints.is_empty() {
        answer.push_str("---\n\n");
        answer.push_str(&hints);
    }

    answer
}

/// 生成主动洞察——检测异常模式并生成提示
fn generate_active_insights(
    intents: Option<&IntentAnalysisResult>,
    review: Option<&WeeklyReviewResult>,
    sessions: Option<&[WorkSession]>,
    locale: AppLocale,
) -> Vec<String> {
    let mut insights = Vec::new();
    if let Some(review) = review {
        if review.session_count > 0 {
            let deep_ratio = review.deep_work_sessions as f64 / review.session_count as f64;
            if deep_ratio < 0.2 && review.session_count >= 4 {
                insights.push(match locale {
                    AppLocale::En => format!(
                        "Deep-work ratio is low ({:.0}%). Consider blocking longer focus periods to reduce fragmentation.",
                        deep_ratio * 100.0
                    ),
                    AppLocale::ZhTw => format!(
                        "深度工作佔比偏低（{:.0}%），建議集中時間段處理複雜任務，減少碎片化切換。",
                        deep_ratio * 100.0
                    ),
                    AppLocale::ZhCn => format!(
                        "深度工作占比偏低（{:.0}%），建议集中时间段处理复杂任务，减少碎片化切换。",
                        deep_ratio * 100.0
                    ),
                });
            }
        }
        if review.active_days <= 2 && review.total_duration > 7200 {
            insights.push(match locale {
                AppLocale::En => {
                    "Work is concentrated in very few days. Watch out for burnout.".to_string()
                }
                AppLocale::ZhTw => "工作集中在較少天數，注意避免過度集中導致疲勞。".to_string(),
                AppLocale::ZhCn => "工作集中在较少天数，注意避免过度集中导致疲劳。".to_string(),
            });
        }
    }
    if let Some(intents) = intents {
        let total: i64 = intents.summary.iter().map(|i| i.duration).sum();
        if let Some(top) = intents.summary.first() {
            if total > 0 {
                let top_ratio = top.duration as f64 / total as f64;
                if top_ratio > 0.7 {
                    insights.push(match locale {
                        AppLocale::En => format!(
                            "\"{}\" accounts for {:.0}% of time. Check if other priorities are being neglected.",
                            top.label, top_ratio * 100.0
                        ),
                        AppLocale::ZhTw => format!(
                            "「{}」佔比 {:.0}%，時間投入高度集中，建議檢查是否忽略了其他重要事項。",
                            top.label, top_ratio * 100.0
                        ),
                        AppLocale::ZhCn => format!(
                            "「{}」占比 {:.0}%，时间投入高度集中，建议检查是否忽略了其他重要事项。",
                            top.label, top_ratio * 100.0
                        ),
                    });
                }
            }
        }
    }
    if let Some(sessions) = sessions {
        let short = sessions.iter().filter(|s| s.duration < 300).count();
        if short > sessions.len() / 2 && sessions.len() >= 4 {
            insights.push(match locale {
                AppLocale::En => format!(
                    "Over half of sessions are under 5 minutes ({}/{}). Consider protecting focus blocks.",
                    short, sessions.len()
                ),
                AppLocale::ZhTw => format!(
                    "超過一半的工作段不足 5 分鐘（{}/{}），建議保護專注時間段。",
                    short, sessions.len()
                ),
                AppLocale::ZhCn => format!(
                    "超过一半的工作段不足 5 分钟（{}/{}），建议保护专注时间段。",
                    short, sessions.len()
                ),
            });
        }
    }
    insights
}

fn build_assistant_cards(
    sessions: Option<&[WorkSession]>,
    intents: Option<&IntentAnalysisResult>,
    review: Option<&WeeklyReviewResult>,
    todos: Option<&TodoExtractionResult>,
) -> Vec<AssistantCard> {
    let mut cards = Vec::new();

    if let Some(review) = review {
        cards.push(AssistantCard {
            kind: "review".to_string(),
            title: "阶段复盘".to_string(),
            content: serde_json::json!({
                "totalDuration": review.total_duration,
                "activeDays": review.active_days,
                "sessionCount": review.session_count,
                "deepWorkSessions": review.deep_work_sessions,
                "highlights": review.highlights,
                "risks": review.risks,
            }),
        });
    }

    if let Some(intents) = intents {
        if !intents.summary.is_empty() {
            cards.push(AssistantCard {
                kind: "intents".to_string(),
                title: "意图分布".to_string(),
                content: serde_json::json!({
                    "items": intents.summary.iter().take(6).map(|item| serde_json::json!({
                        "label": item.label,
                        "duration": item.duration,
                        "sessionCount": item.session_count,
                    })).collect::<Vec<_>>(),
                }),
            });
        }
    }

    if let Some(todos) = todos {
        if !todos.items.is_empty() {
            cards.push(AssistantCard {
                kind: "todos".to_string(),
                title: "待办候选".to_string(),
                content: serde_json::json!({
                    "items": todos.items.iter().take(8).map(|item| serde_json::json!({
                        "title": item.title,
                        "date": item.date,
                        "sourceApp": item.source_app,
                        "confidence": item.confidence,
                        "reason": item.reason,
                    })).collect::<Vec<_>>(),
                }),
            });
        }
    }

    if let Some(sessions) = sessions {
        if !sessions.is_empty() {
            cards.push(AssistantCard {
                kind: "sessions".to_string(),
                title: "代表性 Session".to_string(),
                content: serde_json::json!({
                    "items": sessions.iter().take(5).map(|session| serde_json::json!({
                        "title": session.title,
                        "date": session.date,
                        "duration": session.duration,
                        "dominantApp": session.dominant_app,
                        "intentLabel": session.intent_label,
                    })).collect::<Vec<_>>(),
                }),
            });
        }
    }

    cards
}

async fn generate_text_answer_with_model(
    model_config: &ModelConfig,
    system_prompt: &str,
    prompt: &str,
) -> Result<String, AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Unknown(e.to_string()))?;

    match model_config.provider {
        AiProvider::Ollama => {
            let ollama_base = model_config.endpoint.trim().trim_end_matches('/');
            let ollama_url = if ollama_base.ends_with("/api/chat") { ollama_base.to_string() } else { format!("{ollama_base}/api/chat") };
            let response = client
                .post(&ollama_url)
                .json(&serde_json::json!({
                    "model": model_config.model,
                    "messages": [
                        {
                            "role": "system",
                            "content": system_prompt
                        },
                        {
                            "role": "user",
                            "content": prompt
                        }
                    ],
                    "stream": false
                }))
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(AppError::Analysis(format!(
                    "Ollama 记忆问答失败: {}",
                    response.status()
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let answer = result["message"]["content"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                return Err(AppError::Analysis("Ollama 返回空内容".to_string()));
            }
            Ok(answer)
        }
        AiProvider::Claude => {
            let api_key = model_config.api_key.as_deref().unwrap_or("");
            if api_key.is_empty() {
                return Err(AppError::Analysis("Claude API Key 未配置".to_string()));
            }

            let claude_base = model_config.endpoint.trim().trim_end_matches('/');
            let claude_url = if claude_base.ends_with("/messages") { claude_base.to_string() } else { format!("{claude_base}/messages") };
            let response = client
                .post(&claude_url)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&serde_json::json!({
                    "model": model_config.model,
                    "max_tokens": 1600,
                    "system": system_prompt,
                    "messages": [
                        {
                            "role": "user",
                            "content": prompt
                        }
                    ]
                }))
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await.unwrap_or_default();
                return Err(AppError::Analysis(format!(
                    "Claude 记忆问答失败: {error_text}"
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let answer = result["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                return Err(AppError::Analysis("Claude 返回空内容".to_string()));
            }
            Ok(answer)
        }
        AiProvider::Gemini => {
            let api_key = model_config.api_key.as_deref().unwrap_or("");
            if api_key.is_empty() {
                return Err(AppError::Analysis("Gemini API Key 未配置".to_string()));
            }

            let gemini_base = model_config.endpoint.trim().trim_end_matches('/');
            let gemini_url = format!("{}/models/{}:generateContent?key={}", gemini_base, model_config.model, api_key);
            let response = client
                .post(&gemini_url)
                .json(&serde_json::json!({
                    "contents": [{
                        "parts": [{
                            "text": format!("{}\n\n{}", system_prompt, prompt)
                        }]
                    }],
                    "generationConfig": {
                        "temperature": 0.2,
                        "maxOutputTokens": 1600
                    }
                }))
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await.unwrap_or_default();
                return Err(AppError::Analysis(format!(
                    "Gemini 记忆问答失败: {error_text}"
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let answer = result["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                return Err(AppError::Analysis("Gemini 返回空内容".to_string()));
            }
            Ok(answer)
        }
        _ => {
            let endpoint = model_config.endpoint.trim().trim_end_matches('/');
            let url = if endpoint.ends_with("/chat/completions") {
                endpoint.to_string()
            } else {
                format!("{endpoint}/chat/completions")
            };
            let mut request = client
                .post(&url)
                .json(&serde_json::json!({
                    "model": model_config.model,
                    "messages": [
                        {
                            "role": "system",
                            "content": system_prompt
                        },
                        {
                            "role": "user",
                            "content": prompt
                        }
                    ],
                    "max_tokens": 1600,
                    "temperature": 0.2
                }));

            if let Some(api_key) = &model_config.api_key {
                if !api_key.is_empty() {
                    request = request.header("Authorization", format!("Bearer {api_key}"));
                }
            }

            let response = request.send().await?;

            if !response.status().is_success() {
                let error_text = response.text().await.unwrap_or_default();
                return Err(AppError::Analysis(format!(
                    "OpenAI 兼容记忆问答失败: {error_text}"
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let answer = result["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                return Err(AppError::Analysis("模型返回空内容".to_string()));
            }
            Ok(answer)
        }
    }
}

async fn generate_memory_answer_with_model(
    model_config: &ModelConfig,
    question: &str,
    references: &[MemorySearchItem],
) -> Result<String, AppError> {
    generate_text_answer_with_model(
        model_config,
        "你是一个严谨的个人工作记忆助手，只能基于提供的记录作答，请用中文回答。",
        &build_memory_answer_prompt(question, references),
    )
    .await
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

fn matches_ignored_app(app_name: &str, ignored_apps: &[String]) -> bool {
    let app_lower = app_name.to_lowercase();
    ignored_apps
        .iter()
        .any(|ignored| app_lower.contains(ignored) || ignored.contains(&app_lower))
}

fn apply_ignored_apps_to_stats(mut stats: DailyStats, ignored_apps: &[String]) -> DailyStats {
    if ignored_apps.is_empty() {
        return stats;
    }

    let filtered_app_usage: Vec<_> = stats
        .app_usage
        .into_iter()
        .filter(|app| !matches_ignored_app(&app.app_name, ignored_apps))
        .collect();

    stats.total_duration = filtered_app_usage.iter().map(|app| app.duration).sum();
    stats.app_usage = filtered_app_usage;

    stats
        .browser_usage
        .retain(|browser| !matches_ignored_app(&browser.browser_name, ignored_apps));
    stats.browser_duration = stats
        .browser_usage
        .iter()
        .map(|browser| browser.duration)
        .sum();

    if stats.work_time_duration > stats.total_duration {
        stats.work_time_duration = stats.total_duration;
    }

    stats
}

fn matches_excluded_domain(target: &str, excluded_domains: &[String]) -> bool {
    let domain = PrivacyConfig::extract_domain(target);
    if domain.is_empty() {
        return false;
    }

    excluded_domains.iter().any(|excluded| {
        let excluded_domain = PrivacyConfig::extract_domain(excluded);
        !excluded_domain.is_empty()
            && (PrivacyConfig::domain_matches(&domain, &excluded_domain)
                || merged_domain_matches_excluded(&domain, &excluded_domain))
    })
}

fn merged_domain_matches_excluded(domain: &str, excluded_domain: &str) -> bool {
    if !is_merged_domain(domain) {
        return false;
    }

    let domain = domain.trim_end_matches('.').to_lowercase();
    let excluded_domain = excluded_domain.trim_end_matches('.').to_lowercase();
    let domain_labels: Vec<&str> = domain.split('.').collect();
    let excluded_labels: Vec<&str> = excluded_domain.split('.').collect();

    domain_labels.len() == 2
        && excluded_labels.len() == 2
        && domain_labels[0] == excluded_labels[0]
        && domain_labels[1].starts_with(excluded_labels[1])
        && domain_labels[1].len() > excluded_labels[1].len()
}

fn apply_excluded_domains_to_stats(
    mut stats: DailyStats,
    excluded_domains: &[String],
) -> DailyStats {
    if excluded_domains.is_empty() {
        return stats;
    }

    stats
        .url_usage
        .retain(|url| !matches_excluded_domain(&url.domain, excluded_domains));
    stats
        .domain_usage
        .retain(|domain| !matches_excluded_domain(&domain.domain, excluded_domains));

    for browser in &mut stats.browser_usage {
        browser
            .domains
            .retain(|domain| !matches_excluded_domain(&domain.domain, excluded_domains));
        browser.duration = browser.domains.iter().map(|domain| domain.duration).sum();
    }
    stats.browser_usage.retain(|browser| browser.duration > 0);
    stats.browser_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.browser_name.cmp(&right.browser_name))
    });
    stats.browser_duration = stats
        .browser_usage
        .iter()
        .map(|browser| browser.duration)
        .sum();

    stats
}

fn load_daily_stats_for_overview(state: &AppState, date: &str) -> Result<DailyStats, AppError> {
    let segments = state.config.effective_work_segments();
    state
        .database
        .get_daily_stats_with_segments(date, &segments)
}

fn overview_week_bounds_for_date(anchor: chrono::NaiveDate) -> (String, String) {
    use chrono::Datelike;

    let monday = anchor - chrono::Duration::days(anchor.weekday().num_days_from_monday() as i64);
    (
        monday.format("%Y-%m-%d").to_string(),
        anchor.format("%Y-%m-%d").to_string(),
    )
}

#[derive(Default)]
struct DomainAggregate {
    duration: i64,
    semantic_votes: HashMap<String, i64>,
    urls: HashMap<String, i64>,
}

#[derive(Default)]
struct BrowserAggregate {
    duration: i64,
    executable_path: Option<String>,
    domains: HashMap<String, DomainAggregate>,
}

fn update_preferred_path(target: &mut Option<String>, candidate: Option<String>) {
    if target.is_none() {
        *target = candidate.filter(|value| !value.trim().is_empty());
    }
}

fn record_semantic_vote(
    votes: &mut HashMap<String, i64>,
    semantic_category: Option<String>,
    duration: i64,
) {
    if duration <= 0 {
        return;
    }

    if let Some(category) = semantic_category
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        *votes.entry(category).or_insert(0) += duration;
    }
}

fn resolve_primary_semantic(votes: HashMap<String, i64>) -> Option<String> {
    votes
        .into_iter()
        .max_by(
            |(left_label, left_duration), (right_label, right_duration)| {
                left_duration
                    .cmp(right_duration)
                    .then_with(|| right_label.cmp(left_label))
            },
        )
        .map(|(label, _)| label)
}

fn sort_url_details(items: &mut [UrlDetail]) {
    items.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| right.url.cmp(&left.url))
    });
}

fn sort_domain_usage(items: &mut [DomainUsage]) {
    items.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.domain.cmp(&right.domain))
    });

    for item in items {
        sort_url_details(&mut item.urls);
    }
}

fn build_domain_usage_from_aggregate(domain: String, aggregate: DomainAggregate) -> DomainUsage {
    let mut urls = aggregate
        .urls
        .into_iter()
        .map(|(url, duration)| UrlDetail { url, duration })
        .collect::<Vec<_>>();
    sort_url_details(&mut urls);

    DomainUsage {
        domain,
        duration: aggregate.duration,
        semantic_category: resolve_primary_semantic(aggregate.semantic_votes),
        urls,
    }
}

fn merge_domain_usage_maps(
    target: &mut HashMap<String, DomainAggregate>,
    domains: Vec<DomainUsage>,
) {
    for domain in domains {
        let domain_key = domain.domain.clone();
        let entry = target.entry(domain_key).or_default();
        entry.duration += domain.duration;
        record_semantic_vote(
            &mut entry.semantic_votes,
            domain.semantic_category.clone(),
            domain.duration,
        );

        for url in domain.urls {
            *entry.urls.entry(url.url).or_insert(0) += url.duration;
        }
    }
}

fn sum_daily_stats(days: Vec<DailyStats>) -> DailyStats {
    let mut total_duration = 0;
    let mut screenshot_count = 0;
    let mut browser_duration = 0;
    let mut work_time_duration = 0;

    let mut app_usage_map: HashMap<String, AppUsage> = HashMap::new();
    let mut category_usage_map: HashMap<String, i64> = HashMap::new();
    let mut url_usage_map: HashMap<String, UrlUsage> = HashMap::new();
    let mut domain_usage_map: HashMap<String, DomainAggregate> = HashMap::new();
    let mut browser_usage_map: HashMap<String, BrowserAggregate> = HashMap::new();
    let mut hourly_activity_distribution: Vec<HourlyActivityBucket> = (0..24)
        .map(|hour| HourlyActivityBucket { hour, duration: 0 })
        .collect();

    for day in days {
        total_duration += day.total_duration;
        screenshot_count += day.screenshot_count;
        browser_duration += day.browser_duration;
        work_time_duration += day.work_time_duration;

        for app in day.app_usage {
            let entry = app_usage_map
                .entry(app.app_name.clone())
                .or_insert(AppUsage {
                    app_name: app.app_name.clone(),
                    duration: 0,
                    count: 0,
                    executable_path: None,
                    screenshot_url: None,
                });
            entry.duration += app.duration;
            entry.count += app.count;
            if entry.screenshot_url.is_none() && app.screenshot_url.is_some() {
                entry.screenshot_url = app.screenshot_url;
            }
            update_preferred_path(&mut entry.executable_path, app.executable_path);
        }

        for category in day.category_usage {
            *category_usage_map.entry(category.category).or_insert(0) += category.duration;
        }

        for url in day.url_usage {
            let entry = url_usage_map.entry(url.url.clone()).or_insert(UrlUsage {
                url: url.url.clone(),
                domain: url.domain.clone(),
                duration: 0,
            });
            entry.duration += url.duration;
            if entry.domain.trim().is_empty() {
                entry.domain = url.domain;
            }
        }

        merge_domain_usage_maps(&mut domain_usage_map, day.domain_usage);

        for browser in day.browser_usage {
            let entry = browser_usage_map
                .entry(browser.browser_name.clone())
                .or_default();
            entry.duration += browser.duration;
            update_preferred_path(&mut entry.executable_path, browser.executable_path);
            merge_domain_usage_maps(&mut entry.domains, browser.domains);
        }

        for bucket in day.hourly_activity_distribution {
            if (0..24).contains(&bucket.hour) {
                hourly_activity_distribution[bucket.hour as usize].duration += bucket.duration;
            }
        }
    }

    let mut app_usage = app_usage_map.into_values().collect::<Vec<_>>();
    app_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.app_name.cmp(&right.app_name))
    });

    let mut category_usage = category_usage_map
        .into_iter()
        .map(|(category, duration)| CategoryUsage { category, duration })
        .collect::<Vec<_>>();
    category_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.category.cmp(&right.category))
    });

    let mut url_usage = url_usage_map.into_values().collect::<Vec<_>>();
    url_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| right.url.cmp(&left.url))
    });

    let mut domain_usage = domain_usage_map
        .into_iter()
        .map(|(domain, aggregate)| build_domain_usage_from_aggregate(domain, aggregate))
        .collect::<Vec<_>>();
    sort_domain_usage(&mut domain_usage);

    let mut browser_usage = browser_usage_map
        .into_iter()
        .map(|(browser_name, aggregate)| {
            let mut domains = aggregate
                .domains
                .into_iter()
                .map(|(domain, domain_aggregate)| {
                    build_domain_usage_from_aggregate(domain, domain_aggregate)
                })
                .collect::<Vec<_>>();
            sort_domain_usage(&mut domains);

            BrowserUsage {
                browser_name,
                duration: aggregate.duration,
                executable_path: aggregate.executable_path,
                domains,
            }
        })
        .collect::<Vec<_>>();
    browser_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.browser_name.cmp(&right.browser_name))
    });

    DailyStats {
        total_duration,
        screenshot_count,
        app_usage,
        category_usage,
        browser_duration,
        url_usage,
        domain_usage,
        browser_usage,
        work_time_duration,
        hourly_activity_distribution,
    }
}

fn resolve_overview_anchor_date(date: Option<&str>) -> Result<chrono::NaiveDate, AppError> {
    match date.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|e| AppError::Config(format!("解析概览日期失败: {e}"))),
        None => Ok(chrono::Local::now().date_naive()),
    }
}

fn resolve_overview_date_span(
    date: Option<&str>,
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Result<(chrono::NaiveDate, chrono::NaiveDate), AppError> {
    let fallback = resolve_overview_anchor_date(date)?;
    let start = date_from
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析概览开始日期失败: {e}")))
        })
        .transpose()?
        .unwrap_or(fallback);
    let end = date_to
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析概览结束日期失败: {e}")))
        })
        .transpose()?
        .unwrap_or(start);

    Ok(if start <= end {
        (start, end)
    } else {
        (end, start)
    })
}

/// 获取今日统计 —— 内部复用版（供 Tauri 命令与 localhost API 共用）
pub(crate) fn get_today_stats_inner(state: &Arc<Mutex<AppState>>) -> Result<DailyStats, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let stats = load_daily_stats_for_overview(&state, &today)?;
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
    let stats = apply_ignored_apps_to_stats(stats, &ignored_apps);
    Ok(apply_excluded_domains_to_stats(stats, &excluded_domains))
}

/// 获取今日统计
#[tauri::command]
pub async fn get_today_stats(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<DailyStats, AppError> {
    get_today_stats_inner(state.inner())
}

/// 获取概览统计 —— 内部复用版
pub(crate) fn get_overview_stats_inner(
    mode: String,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<DailyStats, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let normalized_mode = mode.trim().to_lowercase();
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);

    let stats = match normalized_mode.as_str() {
        "date" => {
            let (start, end) = resolve_overview_date_span(
                date.as_deref(),
                date_from.as_deref(),
                date_to.as_deref(),
            )?;

            if start == end {
                let date_value = start.format("%Y-%m-%d").to_string();
                load_daily_stats_for_overview(&state, &date_value)?
            } else {
                let mut daily_stats = Vec::new();
                let mut current = start;
                while current <= end {
                    let current_date = current.format("%Y-%m-%d").to_string();
                    daily_stats.push(load_daily_stats_for_overview(&state, &current_date)?);
                    current = current
                        .succ_opt()
                        .ok_or_else(|| AppError::Config("计算概览日期范围失败".to_string()))?;
                }
                sum_daily_stats(daily_stats)
            }
        }
        "week" => {
            let anchor = resolve_overview_anchor_date(date.as_deref())?;
            let (date_from, date_to) = overview_week_bounds_for_date(anchor);
            let start = chrono::NaiveDate::parse_from_str(&date_from, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析周概览开始日期失败: {e}")))?;
            let end = chrono::NaiveDate::parse_from_str(&date_to, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析周概览结束日期失败: {e}")))?;

            let mut daily_stats = Vec::new();
            let mut current = start;
            while current <= end {
                let current_date = current.format("%Y-%m-%d").to_string();
                daily_stats.push(load_daily_stats_for_overview(&state, &current_date)?);
                current = current
                    .succ_opt()
                    .ok_or_else(|| AppError::Config("计算周概览日期范围失败".to_string()))?;
            }
            sum_daily_stats(daily_stats)
        }
        _ => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            load_daily_stats_for_overview(&state, &today)?
        }
    };

    let stats = apply_ignored_apps_to_stats(stats, &ignored_apps);
    Ok(apply_excluded_domains_to_stats(stats, &excluded_domains))
}

/// 获取概览统计（支持今日 / 指定日期 / 本周）
#[tauri::command]
pub async fn get_overview_stats(
    mode: String,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<DailyStats, AppError> {
    get_overview_stats_inner(mode, date, date_from, date_to, state.inner())
}

/// 获取指定日期的统计 —— 内部复用版
pub(crate) fn get_daily_stats_inner(
    date: &str,
    state: &Arc<Mutex<AppState>>,
) -> Result<DailyStats, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let segments = s.config.effective_work_segments();
    let raw_stats = s.database.get_daily_stats_with_segments(date, &segments)?;
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&s);
    Ok(apply_excluded_domains_to_stats(
        apply_ignored_apps_to_stats(raw_stats, &ignored_apps),
        &excluded_domains,
    ))
}

/// 获取指定日期的统计
#[tauri::command]
pub async fn get_daily_stats(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<DailyStats, AppError> {
    get_daily_stats_inner(&date, state.inner())
}

/// 获取指定日期的时间线 —— 内部复用版（供 Tauri 命令与 localhost API 共用）
pub(crate) fn get_timeline_inner(
    date: String,
    limit: Option<u32>,
    offset: Option<u32>,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<Activity>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = state.database.get_timeline(&date, limit, offset)?;
    let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
    let filtered = filter_activities_by_privacy(activities, &ignored_apps, &excluded_domains);

    if !ignored_apps.is_empty() || !excluded_domains.is_empty() {
        log::info!(
            "隐私过滤: 需过滤应用 {:?}, 域名 {:?}，结果 {} 条",
            ignored_apps,
            excluded_domains,
            filtered.len()
        );
    }

    Ok(filtered)
}

/// 获取指定日期的时间线
#[tauri::command]
pub async fn get_timeline(
    date: String,
    limit: Option<u32>,
    offset: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<Activity>, AppError> {
    get_timeline_inner(date, limit, offset, state.inner())
}

/// 获取每小时×应用的时长分布
#[tauri::command]
pub async fn get_hourly_app_breakdown(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<HourlyAppBucket>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.get_hourly_app_breakdown(&date)
}

fn collect_privacy_filters(state: &AppState) -> (Vec<String>, Vec<String>) {
    let ignored_apps = state.config.privacy.collect_ignored_app_names();
    let excluded_domains = state.config.privacy.collect_excluded_domains();
    (ignored_apps, excluded_domains)
}

fn filter_activities_by_privacy(
    activities: Vec<Activity>,
    ignored_apps: &[String],
    excluded_domains: &[String],
) -> Vec<Activity> {
    let no_app_filter = ignored_apps.is_empty();
    let no_domain_filter = excluded_domains.is_empty();

    if no_app_filter && no_domain_filter {
        return activities;
    }

    activities
        .into_iter()
        .filter(|activity| {
            let app_lower = activity.app_name.to_lowercase();
            if !no_app_filter
                && ignored_apps
                    .iter()
                    .any(|ignored| app_lower.contains(ignored) || ignored.contains(&app_lower))
            {
                return false;
            }

            if !no_domain_filter {
                if let Some(url) = &activity.browser_url {
                    let domain = PrivacyConfig::extract_domain(url);
                    if excluded_domains
                        .iter()
                        .any(|excluded| PrivacyConfig::domain_matches(&domain, excluded))
                    {
                        return false;
                    }
                }
            }

            true
        })
        .collect()
}

pub(crate) fn load_filtered_activities_in_range(
    state: &AppState,
    date_from: Option<&str>,
    date_to: Option<&str>,
    limit: usize,
) -> Result<Vec<Activity>, AppError> {
    let activities = state
        .database
        .get_activities_in_range(date_from, date_to, limit)?;
    let (ignored_apps, excluded_domains) = collect_privacy_filters(state);
    Ok(filter_activities_by_privacy(
        activities,
        &ignored_apps,
        &excluded_domains,
    ))
}

fn manual_followups_in_range(
    items: &[AvatarFollowupItem],
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Vec<AvatarFollowupItem> {
    items
        .iter()
        .filter(|item| item.status == "open")
        .filter(|item| {
            date_from
                .map(|start| item.date.as_str() >= start)
                .unwrap_or(true)
                && date_to.map(|end| item.date.as_str() <= end).unwrap_or(true)
        })
        .cloned()
        .collect()
}

fn merge_manual_followups_into_todos(
    mut extracted: TodoExtractionResult,
    manual_items: &[AvatarFollowupItem],
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> TodoExtractionResult {
    let manual_items = manual_followups_in_range(manual_items, date_from, date_to);
    if manual_items.is_empty() {
        return extracted;
    }

    let mut seen = std::collections::HashSet::new();
    for item in &extracted.items {
        seen.insert(item.title.trim().to_lowercase());
    }

    for item in manual_items {
        let normalized = item.title.trim().to_lowercase();
        if normalized.is_empty() || !seen.insert(normalized) {
            continue;
        }

        extracted.items.push(crate::work_intelligence::TodoItem {
            title: item.title.clone(),
            date: item.date.clone(),
            source_title: item.source_title.clone(),
            source_app: item.source_app.clone(),
            confidence: 96,
            reason: "桌宠手动加入待跟进".to_string(),
        });
    }

    extracted.items.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| b.date.cmp(&a.date))
            .then_with(|| a.title.cmp(&b.title))
    });
    extracted.items.truncate(20);
    extracted.summary = format!(
        "共整理出 {} 条待跟进项（含桌宠手动加入）。",
        extracted.items.len()
    );
    extracted
}

/// 获取单个活动（用于刷新详情页，获取最新 OCR 结果）
#[tauri::command]
pub async fn get_activity(
    id: i64,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<Activity>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.get_activity_by_id(id)
}

/// 搜索工作记忆
#[tauri::command]
pub async fn search_memory(
    query: String,
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<MemorySearchItem>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.search_memory(
        &query,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(20) as usize,
    )
}

/// 基于工作记忆回答问题
#[tauri::command]
pub async fn ask_memory(
    question: String,
    date_from: Option<String>,
    date_to: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<MemoryAnswer, AppError> {
    let (model_config, references) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let references =
            state
                .database
                .search_memory(&question, date_from.as_deref(), date_to.as_deref(), 8)?;
        (state.config.text_model.clone(), references)
    };

    if references.is_empty() {
        return Ok(MemoryAnswer {
            answer: build_fallback_memory_answer(&question, &references),
            references,
            used_ai: false,
            model_name: None,
        });
    }

    if is_text_model_available(&model_config) {
        match generate_memory_answer_with_model(&model_config, &question, &references).await {
            Ok(answer) => {
                return Ok(MemoryAnswer {
                    answer,
                    references,
                    used_ai: true,
                    model_name: Some(model_config.model),
                });
            }
            Err(error) => {
                log::warn!("记忆问答 AI 生成失败，回退基础模式: {error}");
            }
        }
    }

    Ok(MemoryAnswer {
        answer: build_fallback_memory_answer(&question, &references),
        references,
        used_ai: false,
        model_name: None,
    })
}

/// 统一工作助手
#[tauri::command]
pub async fn chat_work_assistant(
    question: String,
    history: Option<Vec<AssistantChatMessage>>,
    model_config: Option<ModelConfig>,
    locale: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<AssistantAnswer, AppError> {
    let trimmed_question = question.trim().to_string();
    let history = history.unwrap_or_default();
    let assistant_locale = AppLocale::from_option(locale.as_deref());
    if trimmed_question.is_empty() {
        return Ok(AssistantAnswer {
            answer: assistant_empty_question_message(assistant_locale).to_string(),
            references: Vec::new(),
            used_ai: false,
            model_name: None,
            tool_labels: vec!["记忆检索".to_string()],
            cards: Vec::new(),
        });
    }

    // 时间范围：前端传入优先，否则从问题中自动提取
    let (date_from, date_to) = if date_from.is_some() || date_to.is_some() {
        (date_from, date_to)
    } else {
        let (auto_from, auto_to) = parse_temporal_range(&trimmed_question);
        (auto_from, auto_to)
    };

    let reasoning_mode = assistant_reasoning_mode(model_config.as_ref());
    let question_kind =
        detect_assistant_question_kind_with_mode(&trimmed_question, &history, reasoning_mode);
    let tools = detect_assistant_tools_with_history(&trimmed_question, &history, reasoning_mode);
    let search_query = build_contextual_query(&trimmed_question, &history);
    let (references, sessions, intents, review, todos) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let references = state.database.search_memory(
            &search_query,
            date_from.as_deref(),
            date_to.as_deref(),
            8,
        )?;

        let needs_activity_data = tools.iter().any(|tool| {
            matches!(
                tool,
                AssistantTool::Sessions
                    | AssistantTool::Intents
                    | AssistantTool::Review
                    | AssistantTool::Todos
            )
        });

        let activities = if needs_activity_data {
            Some(load_filtered_activities_in_range(
                &state,
                date_from.as_deref(),
                date_to.as_deref(),
                5000,
            )?)
        } else {
            None
        };

        let sessions = if tools.contains(&AssistantTool::Sessions) {
            activities.as_ref().map(|items| build_work_sessions(items))
        } else {
            None
        };

        let intents = if tools.contains(&AssistantTool::Intents) {
            activities.as_ref().map(|items| analyze_intents(items))
        } else {
            None
        };

        let review = if tools.contains(&AssistantTool::Review) {
            activities
                .as_ref()
                .map(|items| build_weekly_review(items, date_from.as_deref(), date_to.as_deref()))
        } else {
            None
        };

        let todos = if tools.contains(&AssistantTool::Todos) {
            Some(merge_manual_followups_into_todos(
                activities
                    .as_ref()
                    .map(|items| extract_todos(items))
                    .unwrap_or_else(|| TodoExtractionResult {
                        items: Vec::new(),
                        summary: "当前时间范围内没有提取到明确的待办信号。".to_string(),
                    }),
                &state.config.avatar_followups,
                date_from.as_deref(),
                date_to.as_deref(),
            ))
        } else {
            None
        };

        (references, sessions, intents, review, todos)
    };

    let tool_labels = tools
        .iter()
        .map(|tool| tool.label().to_string())
        .collect::<Vec<_>>();
    let cards = build_assistant_cards(
        sessions.as_deref(),
        intents.as_ref(),
        review.as_ref(),
        todos.as_ref(),
    );

    // model_config: None = basic template (no AI), Some = AI enhanced
    if let Some(ref ai_model) = model_config {
        if is_text_model_available(ai_model) {
            let prompt = build_assistant_prompt(
                &trimmed_question,
                question_kind,
                &history,
                date_from.as_deref(),
                date_to.as_deref(),
                &references,
                sessions.as_deref(),
                intents.as_ref(),
                review.as_ref(),
                todos.as_ref(),
                assistant_locale,
            );

            let sys = build_assistant_system_prompt(assistant_locale);

            match generate_text_answer_with_model(ai_model, sys, &prompt).await {
                Ok(answer) => {
                    return Ok(AssistantAnswer {
                        answer,
                        references,
                        used_ai: true,
                        model_name: Some(ai_model.model.clone()),
                        tool_labels,
                        cards,
                    });
                }
                Err(error) => {
                    log::warn!("AI generation failed, falling back: {error}");
                }
            }
        }
    }

    Ok(AssistantAnswer {
        answer: build_fallback_assistant_answer(
            &trimmed_question,
            question_kind,
            &references,
            sessions.as_deref(),
            intents.as_ref(),
            review.as_ref(),
            todos.as_ref(),
            &tool_labels,
            assistant_locale,
        ),
        references,
        used_ai: false,
        model_name: None,
        tool_labels,
        cards,
    })
}

/// 获取连续工作 session 聚合结果
#[tauri::command]
pub async fn get_work_sessions(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<WorkSession>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000) as usize,
    )?;

    Ok(build_work_sessions(&activities))
}

/// 基于 session 识别主要工作意图
#[tauri::command]
pub async fn recognize_work_intents(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<IntentAnalysisResult, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000) as usize,
    )?;

    Ok(analyze_intents(&activities))
}

/// 生成周报 / 阶段复盘 —— 内部复用版（供 Tauri 命令与 localhost API 共用）
pub(crate) fn generate_weekly_review_inner(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: &Arc<Mutex<AppState>>,
) -> Result<WeeklyReviewResult, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000) as usize,
    )?;

    Ok(build_weekly_review(
        &activities,
        date_from.as_deref(),
        date_to.as_deref(),
    ))
}

/// 生成周报 / 阶段复盘
#[tauri::command]
pub async fn generate_weekly_review(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<WeeklyReviewResult, AppError> {
    generate_weekly_review_inner(date_from, date_to, limit, state.inner())
}

/// 提取待跟进事项
#[tauri::command]
pub async fn extract_todo_items(
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<TodoExtractionResult, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let activities = load_filtered_activities_in_range(
        &state,
        date_from.as_deref(),
        date_to.as_deref(),
        limit.unwrap_or(5000) as usize,
    )?;

    Ok(merge_manual_followups_into_todos(
        extract_todos(&activities),
        &state.config.avatar_followups,
        date_from.as_deref(),
        date_to.as_deref(),
    ))
}

/// 生成日报
pub(crate) async fn generate_report_inner(
    date: String,
    force: Option<bool>,
    locale: Option<String>,
    app: &AppHandle,
    state: &Arc<Mutex<AppState>>,
) -> Result<String, AppError> {
    let report_locale = AppLocale::from_option(locale.as_deref());
    let report_locale_code = report_locale.as_code();
    // 如果不是强制重新生成，先检查缓存
    if !force.unwrap_or(false) {
        let state_guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        if let Ok(Some(cached)) = state_guard
            .database
            .get_report(&date, Some(report_locale_code))
        {
            log::info!("使用缓存日报: {date}");
            return Ok(cached.content);
        }
    }

    let (config, stats, activities, data_dir) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let segments = state.config.effective_work_segments();
        let raw_stats = state
            .database
            .get_daily_stats_with_segments(&date, &segments)?;
        // 生成日报时获取最多 2000 条记录
        let raw_activities = state.database.get_timeline(&date, Some(2000), None)?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
        let stats = apply_excluded_domains_to_stats(
            apply_ignored_apps_to_stats(raw_stats, &ignored_apps),
            &excluded_domains,
        );
        let activities =
            filter_activities_by_privacy(raw_activities, &ignored_apps, &excluded_domains);
        (
            state.config.clone(),
            stats,
            activities,
            state.data_dir.clone(),
        )
    };

    let avatar_start_state = {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.avatar_generating_report = true;
        let avatar_state = crate::avatar_engine::apply_avatar_visual_settings(
            crate::avatar_engine::derive_avatar_state(
                &state.avatar_state.app_name,
                "",
                None,
                state.avatar_state.is_idle,
                true,
            ),
            state.config.avatar_opacity,
            &state.config.avatar_preset,
            &state.config.avatar_persona,
        );
        state.avatar_state = avatar_state.clone();
        if state.config.avatar_enabled {
            Some(avatar_state)
        } else {
            None
        }
    };

    if let Some(avatar_state) = avatar_start_state.as_ref() {
        crate::avatar_engine::emit_avatar_state(&app, avatar_state);
        crate::avatar_engine::emit_avatar_bubble(
            &app,
            &crate::avatar_engine::AvatarBubblePayload::info(match report_locale {
                AppLocale::ZhCn => "开始整理日报，稍等我一下。",
                AppLocale::ZhTw => "開始整理日報，稍等我一下。",
                AppLocale::En => "I'm preparing your daily report. Give me a moment.",
            }),
        );
    }

    // 创建分析器（使用 text_model 配置）
    let analyzer = crate::analysis::create_analyzer(
        config.ai_mode,
        config.text_model.provider,
        &config.text_model.endpoint,
        &config.text_model.model,
        config.text_model.api_key.as_deref(),
        &config.daily_report_custom_prompt,
        report_locale,
    );

    // 生成报告（spawn 隔离 panic，防止内部错误杀死整个 tokio 线程）
    // 外层加 300 秒总超时，防止 AI 调用卡死后前端永远等待
    let screenshots_dir = data_dir.clone();
    let date_gen = date.clone();
    let category_name_overrides: std::collections::HashMap<String, String> = config
        .custom_categories
        .iter()
        .map(|c| (c.key.clone(), c.name.clone()))
        .collect();
    let semantic_name_overrides: std::collections::HashMap<String, String> = config
        .custom_semantic_categories
        .iter()
        .map(|c| (c.key.clone(), c.name.clone()))
        .collect();
    let spawn_result = tokio::spawn(async move {
        analyzer
            .generate_report(
                &date_gen,
                &stats,
                &activities,
                &screenshots_dir,
                report_locale,
                category_name_overrides,
                semantic_name_overrides,
            )
            .await
    });

    let report_result = match tokio::time::timeout(
        std::time::Duration::from_secs(300),
        spawn_result,
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(work_review_core::error::AppError::Analysis(
            match report_locale {
                AppLocale::ZhCn => "日报生成过程中发生内部错误，请重试".to_string(),
                AppLocale::ZhTw => "日報生成過程中發生內部錯誤，請重試".to_string(),
                AppLocale::En => {
                    "Internal error during report generation, please retry".to_string()
                }
            },
        )),
        Err(_) => Err(work_review_core::error::AppError::Analysis(
            match report_locale {
                AppLocale::ZhCn => "日报生成超时，请稍后重试".to_string(),
                AppLocale::ZhTw => "日報生成逾時，請稍後重試".to_string(),
                AppLocale::En => "Report generation timed out, please try again later".to_string(),
            },
        )),
    };

    let avatar_finish_state = {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.avatar_generating_report = false;
        let avatar_state = crate::avatar_engine::apply_avatar_visual_settings(
            crate::avatar_engine::derive_avatar_state(
                &state.avatar_state.app_name,
                "",
                None,
                state.avatar_state.is_idle,
                false,
            ),
            state.config.avatar_opacity,
            &state.config.avatar_preset,
            &state.config.avatar_persona,
        );
        state.avatar_state = avatar_state.clone();
        if state.config.avatar_enabled {
            Some(avatar_state)
        } else {
            None
        }
    };

    if let Some(avatar_state) = avatar_finish_state.as_ref() {
        crate::avatar_engine::emit_avatar_state(&app, avatar_state);
        let bubble = if report_result.is_ok() {
            crate::avatar_engine::AvatarBubblePayload::success(match report_locale {
                AppLocale::ZhCn => "日报整理好了，可以回来看看。",
                AppLocale::ZhTw => "日報整理好了，可以回來看看。",
                AppLocale::En => "Your daily report is ready. You can check it now.",
            })
        } else {
            crate::avatar_engine::AvatarBubblePayload::info(match report_locale {
                AppLocale::ZhCn => "这次日报整理失败了，稍后可以再试。",
                AppLocale::ZhTw => "這次日報整理失敗了，稍後可以再試。",
                AppLocale::En => "This report run failed. Please try again later.",
            })
        };
        crate::avatar_engine::emit_avatar_bubble(&app, &bubble);
    }

    let generated_report = report_result?;
    let report = generated_report.content.clone();
    let (saved_ai_mode, saved_model_name) = resolve_saved_report_metadata(
        &config.ai_mode,
        &config.text_model.model,
        generated_report.used_ai,
    );

    // 保存报告
    {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let daily_report = DailyReport {
            date: date.clone(),
            locale: report_locale_code.to_string(),
            content: report.clone(),
            ai_mode: saved_ai_mode,
            model_name: saved_model_name,
            fallback_reason: generated_report.fallback_reason.clone(),
            created_at: chrono::Utc::now().timestamp(),
        };
        state.database.save_report(&daily_report)?;
    }

    if config.daily_report_auto_export {
        if let Some(export_dir) = config.daily_report_export_dir.as_deref() {
            export_daily_report_markdown(Path::new(export_dir), &date, &report)?;
        }
    }

    Ok(report)
}

struct ReportGenerationGuard {
    state: Arc<Mutex<AppState>>,
}

impl Drop for ReportGenerationGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.generating_report = false;
        }
    }
}

#[tauri::command]
pub async fn generate_report(
    date: String,
    force: Option<bool>,
    locale: Option<String>,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    {
        let mut s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        if s.generating_report {
            return Err(AppError::Unknown("日报正在生成中，请稍候".to_string()));
        }
        s.generating_report = true;
    }
    let _guard = ReportGenerationGuard { state: state.inner().clone() };
    generate_report_inner(date, force, locale, &app, state.inner()).await
}

/// 获取已保存的日报
pub(crate) fn get_saved_report_inner(
    date: String,
    locale: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<Option<DailyReport>, AppError> {
    let report_locale = AppLocale::from_option(locale.as_deref());
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let saved = state
        .database
        .get_report(&date, Some(report_locale.as_code()))?;
    let Some(mut report) = saved else {
        return Ok(None);
    };

    // 用最新的 stats 重新渲染统计区块，解决 issue #80：保存的 markdown 里固化的时长
    // 数字会随着工作日继续推进而变得陈旧。老报告若没有占位符标记则原样返回。
    let segments = state.config.effective_work_segments();
    if let Ok(raw_stats) = state.database.get_daily_stats_with_segments(&date, &segments) {
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&state);
        let live_stats = apply_excluded_domains_to_stats(
            apply_ignored_apps_to_stats(raw_stats, &ignored_apps),
            &excluded_domains,
        );
        let category_name_overrides: std::collections::HashMap<String, String> = state
            .config
            .custom_categories
            .iter()
            .map(|c| (c.key.clone(), c.name.clone()))
            .collect();
        let semantic_name_overrides: std::collections::HashMap<String, String> = state
            .config
            .custom_semantic_categories
            .iter()
            .map(|c| (c.key.clone(), c.name.clone()))
            .collect();
        report.content = crate::analysis::report_blocks::render_report_with_live_stats(
            &report.content,
            &live_stats,
            report_locale,
            &category_name_overrides,
            &semantic_name_overrides,
        );
    }

    Ok(Some(report))
}

#[tauri::command]
pub async fn get_saved_report(
    date: String,
    locale: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<DailyReport>, AppError> {
    get_saved_report_inner(date, locale, state.inner())
}

/// 更新已保存日报的内容（用于结构化编辑）
#[tauri::command]
pub async fn update_report_content(
    date: String,
    locale: Option<String>,
    content: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let report_locale = AppLocale::from_option(locale.as_deref());
    let locale_code = report_locale.as_code();
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let existing = state
        .database
        .get_report(&date, Some(locale_code))?
        .ok_or_else(|| AppError::Database(rusqlite::Error::InvalidParameterName("报告不存在".to_string())))?;
    let updated = DailyReport {
        content,
        ..existing
    };
    state.database.save_report(&updated)?;
    Ok(())
}

pub(crate) fn export_report_markdown_inner(
    date: String,
    content: Option<String>,
    export_dir: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<String, AppError> {
    let (export_dir, saved_content) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let requested_export_dir = export_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(|dir| dir.to_string());
        let configured_export_dir = state
            .config
            .daily_report_export_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(|dir| dir.to_string());
        let export_dir = requested_export_dir
            .or(configured_export_dir)
            .ok_or_else(|| {
                AppError::Config(
                    "请先选择导出目录，或在设置中配置日报 Markdown 导出目录".to_string(),
                )
            })?;
        let saved_content = if let Some(content) = content {
            content
        } else {
            state
                .database
                .get_report(&date, Some("zh-CN"))?
                .ok_or_else(|| AppError::Config("未找到可导出的日报".to_string()))?
                .content
        };
        (export_dir, saved_content)
    };

    let export_dir_path = Path::new(&export_dir);
    export_daily_report_markdown(export_dir_path, &date, &saved_content)?;
    Ok(build_daily_report_export_path(export_dir_path, &date)
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub async fn export_report_markdown(
    date: String,
    content: Option<String>,
    export_dir: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    export_report_markdown_inner(date, content, export_dir, state.inner())
}

/// 获取配置
#[tauri::command]
pub async fn get_config(state: State<'_, Arc<Mutex<AppState>>>) -> Result<AppConfig, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(state.config.clone())
}

#[tauri::command]
pub async fn get_localhost_api_status(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<crate::localhost_api::LocalhostApiStatusPayload, AppError> {
    crate::localhost_api::get_localhost_api_status(state.inner())
}

#[tauri::command]
pub async fn get_node_gateway_status(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<crate::node_gateway::NodeGatewayStatusPayload, AppError> {
    crate::node_gateway::get_node_gateway_status(state.inner())
}

#[tauri::command]
pub async fn get_telegram_bot_status(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let now_ts = chrono::Local::now().timestamp();
    Ok(serde_json::json!({
        "running": s.telegram_bot_runtime.is_running(),
        "starting": s.telegram_bot_runtime.is_starting(),
        "lastError": s.telegram_bot_runtime.last_error(),
        "allowedChatIds": s.config.telegram_bot_allowed_chat_ids.clone(),
        "bindCode": s.config.telegram_bot_bind_code.clone(),
        "bindCodeExpiresAt": s.config.telegram_bot_bind_code_expires_at,
        "bindCodeExpired": s.config.telegram_bot_bind_code_expires_at.map(|expires_at| expires_at < now_ts).unwrap_or(false),
    }))
}

#[tauri::command]
pub async fn generate_telegram_bot_bind_code(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    const TELEGRAM_BIND_CODE_TTL_SECONDS: i64 = 10 * 60;

    let raw = uuid::Uuid::new_v4().simple().to_string();
    let code = format!("WR-{}", raw[..6].to_ascii_uppercase());
    let expires_at = chrono::Local::now().timestamp() + TELEGRAM_BIND_CODE_TTL_SECONDS;
    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut config = state.config.clone();
        config.telegram_bot_bind_code = Some(code.clone());
        config.telegram_bot_bind_code_expires_at = Some(expires_at);
        config
    };

    persist_app_config(next_config, app, state.inner())?;

    Ok(serde_json::json!({
        "code": code,
        "expiresAt": expires_at,
    }))
}

#[tauri::command]
pub async fn reveal_localhost_api_token(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    crate::localhost_api::reveal_localhost_api_token(state.inner())
}

#[tauri::command]
pub async fn rotate_localhost_api_token(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let token = crate::localhost_api::rotate_localhost_api_token(state.inner())?;
    crate::localhost_api::sync_localhost_api_runtime(&app, state.inner())?;
    Ok(token)
}

pub(crate) fn persist_app_config(
    mut config: AppConfig,
    app: AppHandle,
    state: &Arc<Mutex<AppState>>,
) -> Result<(), AppError> {
    config.normalize();
    let (
        previous_avatar_enabled,
        previous_avatar_scale,
        previous_avatar_opacity,
        previous_avatar_preset,
        previous_avatar_x,
        previous_avatar_y,
        previous_hide_dock_icon,
        previous_lightweight_mode,
        avatar_state,
    ) = {
        let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let previous_config = state.config.clone();

        // 更新配置
        state.config = config.clone();
        state.storage_manager.update_config(config.storage.clone());
        state.screenshot_service.update_config(&config.storage);

        // 保存到文件
        let config_path = state.config_path.clone();
        config.save(&config_path)?;

        // 更新隐私过滤器
        state.privacy_filter.update_config(&config.privacy);
        state.avatar_state = crate::avatar_engine::apply_avatar_visual_settings(
            state.avatar_state.clone(),
            config.avatar_opacity,
            &config.avatar_preset,
            &config.avatar_persona,
        );
        (
            previous_config.avatar_enabled,
            previous_config.avatar_scale,
            previous_config.avatar_opacity,
            previous_config.avatar_preset,
            previous_config.avatar_x,
            previous_config.avatar_y,
            previous_config.hide_dock_icon,
            previous_config.lightweight_mode,
            state.avatar_state.clone(),
        )
    };

    let avatar_window_changed = previous_avatar_enabled != config.avatar_enabled
        || previous_avatar_scale != config.avatar_scale
        || previous_avatar_x != config.avatar_x
        || previous_avatar_y != config.avatar_y;
    let avatar_visual_changed = previous_avatar_opacity != config.avatar_opacity
        || previous_avatar_preset != config.avatar_preset;
    let dock_visibility_changed = previous_hide_dock_icon != config.hide_dock_icon
        || previous_lightweight_mode != config.lightweight_mode;

    if avatar_window_changed {
        crate::avatar_engine::sync_avatar_window(
            &app,
            config.avatar_enabled,
            config.avatar_scale,
            config.avatar_x.zip(config.avatar_y),
            false,
        )
        .map_err(|e| AppError::Unknown(format!("同步桌宠窗口失败: {e}")))?;
    }

    if config.avatar_enabled
        && (avatar_window_changed || avatar_visual_changed)
        && !refresh_avatar_state_for_current_window(&app, state)
    {
        crate::avatar_engine::emit_avatar_state(&app, &avatar_state);
    }

    if dock_visibility_changed {
        crate::sync_effective_dock_visibility(&app);
    }

    crate::localhost_api::sync_localhost_api_runtime(&app, state)?;
    crate::telegram_bot::sync_telegram_bot_runtime(state)?;
    crate::emit_config_changed(&app, &config);

    log::info!("配置已保存");
    Ok(())
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

fn refresh_avatar_state_for_current_window(app: &AppHandle, state: &Arc<Mutex<AppState>>) -> bool {
    let active_window = match crate::monitor::get_active_window_fast() {
        Ok(window) => window,
        Err(_) => return false,
    };

    let next_avatar_state = {
        let mut state_guard = match state.lock() {
            Ok(guard) => guard,
            Err(e) => {
                log::warn!("刷新桌宠状态时获取状态锁失败: {e}");
                return false;
            }
        };

        if !state_guard.config.avatar_enabled {
            return false;
        }

        let next_state = crate::avatar_engine::apply_avatar_visual_settings(
            crate::avatar_engine::derive_avatar_state_with_rules(
                &state_guard.config.app_category_rules,
                &state_guard.config.custom_categories,
                &active_window.app_name,
                &active_window.window_title,
                active_window.browser_url.as_deref(),
                state_guard.avatar_state.is_idle,
                state_guard.avatar_generating_report,
            ),
            state_guard.config.avatar_opacity,
            &state_guard.config.avatar_preset,
            &state_guard.config.avatar_persona,
        );
        state_guard.avatar_state = next_state.clone();
        next_state
    };

    crate::avatar_engine::emit_avatar_state(app, &next_avatar_state);
    true
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

/// 测试 AI 模型连接
#[tauri::command]
pub async fn test_ai_model(provider_config: AiProviderConfig) -> Result<ModelTestResult, AppError> {
    let start = std::time::Instant::now();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Unknown(e.to_string()))?;

    let result = match provider_config.provider {
        AiProvider::Ollama => test_ollama(&client, &provider_config).await,
        AiProvider::Gemini => test_gemini(&client, &provider_config).await,
        AiProvider::Claude => test_claude(&client, &provider_config).await,
        // OpenAI 及兼容格式的供应商
        _ => test_openai(&client, &provider_config).await,
    };

    let elapsed = start.elapsed().as_millis() as u64;

    match result {
        Ok(info) => Ok(ModelTestResult {
            success: true,
            message: "连接成功！模型可用。".to_string(),
            response_time_ms: elapsed,
            model_info: Some(info),
        }),
        Err(e) => Ok(ModelTestResult {
            success: false,
            message: format!("连接失败: {e}"),
            response_time_ms: elapsed,
            model_info: None,
        }),
    }
}

/// 测试模型连接（新版，使用 ModelConfig）
#[tauri::command]
pub async fn test_model(model_config: ModelConfig) -> Result<ModelTestResult, AppError> {
    let start = std::time::Instant::now();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Unknown(e.to_string()))?;

    // 将 ModelConfig 转换为 AiProviderConfig 以复用现有测试逻辑
    let provider_config = AiProviderConfig {
        provider: model_config.provider,
        endpoint: model_config.endpoint,
        api_key: model_config.api_key,
        model: model_config.model,
        vision_model: None,
    };

    let result = match provider_config.provider {
        AiProvider::Ollama => test_ollama(&client, &provider_config).await,
        AiProvider::Gemini => test_gemini(&client, &provider_config).await,
        AiProvider::Claude => test_claude(&client, &provider_config).await,
        // OpenAI 及兼容格式的供应商（硅基流动、DeepSeek、通义千问、智谱、月之暗面、豆包）
        _ if provider_config.provider.is_openai_compatible() => {
            test_openai(&client, &provider_config).await
        }
        // 兜底：默认使用 OpenAI 格式
        _ => test_openai(&client, &provider_config).await,
    };

    let elapsed = start.elapsed().as_millis() as u64;

    match result {
        Ok(info) => Ok(ModelTestResult {
            success: true,
            message: "连接成功！模型可用。".to_string(),
            response_time_ms: elapsed,
            model_info: Some(info),
        }),
        Err(e) => Ok(ModelTestResult {
            success: false,
            message: format!("连接失败: {e}"),
            response_time_ms: elapsed,
            model_info: None,
        }),
    }
}

/// 测试 Ollama 连接
async fn test_ollama(
    client: &reqwest::Client,
    config: &AiProviderConfig,
) -> Result<String, String> {
    // 1. 先测试服务是否可用
    let tags_url = format!("{}/api/tags", config.endpoint);
    let response = client
        .get(&tags_url)
        .send()
        .await
        .map_err(|e| format!("无法连接到 Ollama 服务: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Ollama 服务返回错误: {}", response.status()));
    }

    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("解析响应失败: {e}"))?;

    // 2. 基于模型列表和能力信息判断是否为可用的文本生成模型
    let models = data["models"].as_array().ok_or("无法获取模型列表")?;
    let installed_model_exists = models.iter().any(|model| {
        model["name"]
            .as_str()
            .is_some_and(|name| ollama_model_names_match(&config.model, name))
    });
    let available_models = resolve_ollama_text_model_names(client, &config.endpoint, &data)
        .await
        .map_err(|e| format!("过滤 Ollama 模型列表失败: {e}"))?;

    let text_model_exists = available_models
        .iter()
        .any(|name| ollama_model_names_match(&config.model, name));

    if !text_model_exists {
        if installed_model_exists {
            return Err(format!(
                "模型 {} 已安装，但不是可用于对话/生成的文本模型",
                config.model
            ));
        }

        let available: Vec<String> = available_models.into_iter().take(5).collect();
        let available_hint = if available.is_empty() {
            "当前未发现可用文本模型".to_string()
        } else {
            format!("可用模型: {}", available.join(", "))
        };
        return Err(format!("模型 {} 未安装。{}", config.model, available_hint));
    }

    // 3. 实际调用模型生成测试（关键验证步骤）
    let generate_url = format!("{}/api/generate", config.endpoint);
    let test_response = client
        .post(&generate_url)
        .json(&serde_json::json!({
            "model": config.model,
            "prompt": "Hi",
            "stream": false,
            "options": {
                "num_predict": 5  // 只生成5个token，快速测试
            }
        }))
        .send()
        .await
        .map_err(|e| format!("调用模型失败: {e}"))?;

    if !test_response.status().is_success() {
        let error_text = test_response.text().await.unwrap_or_default();
        return Err(format!("模型响应失败: {error_text}"));
    }

    let result: serde_json::Value = test_response
        .json()
        .await
        .map_err(|e| format!("解析模型响应失败: {e}"))?;

    // 检查是否有实际响应
    if result["response"].as_str().is_some() {
        Ok(format!("模型 {} 测试通过，响应正常", config.model))
    } else {
        Err("模型返回空响应".to_string())
    }
}

/// 测试 OpenAI 连接
fn openai_connection_test_max_tokens() -> u32 {
    16
}

fn openai_compatible_chat_completion_urls(endpoint: &str) -> Vec<String> {
    let base = endpoint.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }

    if base.ends_with("/chat/completions") {
        return vec![base.to_string()];
    }

    let mut urls = vec![format!("{base}/chat/completions")];
    if !base.ends_with("/v1") {
        urls.push(format!("{base}/v1/chat/completions"));
    }
    urls.dedup();
    urls
}

async fn test_openai(
    client: &reqwest::Client,
    config: &AiProviderConfig,
) -> Result<String, String> {
    let api_key = config.api_key.as_ref().ok_or("未配置 API Key")?;

    let payload = serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": openai_connection_test_max_tokens(),
    });

    let mut last_error = None;

    for url in openai_compatible_chat_completion_urls(&config.endpoint) {
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&payload)
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(format!("{url} 请求失败: {error}"));
                continue;
            }
        };

        if response.status().is_success() {
            let data: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("解析响应失败: {e}"))?;
            let model_used = data["model"].as_str().unwrap_or(&config.model);
            return Ok(format!("模型 {model_used} 响应正常"));
        }

        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        last_error = Some(format!("{url} API 错误 ({status}): {error_text}"));
    }

    Err(last_error.unwrap_or_else(|| "API 请求失败：未生成可用请求地址".to_string()))
}

/// 测试 Google Gemini 连接
async fn test_gemini(
    client: &reqwest::Client,
    config: &AiProviderConfig,
) -> Result<String, String> {
    let api_key = config.api_key.as_ref().ok_or("未配置 API Key")?;

    let url = format!(
        "{}/models/{}:generateContent?key={}",
        config.endpoint, config.model, api_key
    );

    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "contents": [{"parts": [{"text": "Hello"}]}],
            "generationConfig": {"maxOutputTokens": 10}
        }))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    if response.status().is_success() {
        Ok(format!("Gemini 模型 {} 响应正常", config.model))
    } else {
        let error_text = response.text().await.unwrap_or_default();
        Err(format!("API 错误: {error_text}"))
    }
}

/// 测试 Anthropic Claude 连接
async fn test_claude(
    client: &reqwest::Client,
    config: &AiProviderConfig,
) -> Result<String, String> {
    let api_key = config.api_key.as_ref().ok_or("未配置 API Key")?;

    let claude_base = config.endpoint.trim().trim_end_matches('/');
    let claude_url = if claude_base.ends_with("/messages") { claude_base.to_string() } else { format!("{claude_base}/messages") };
    let response = client
        .post(&claude_url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({
            "model": config.model,
            "max_tokens": 10,
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    if response.status().is_success() {
        Ok(format!("Claude 模型 {} 响应正常", config.model))
    } else {
        let error_text = response.text().await.unwrap_or_default();
        Err(format!("API 错误: {error_text}"))
    }
}

fn normalize_ollama_model_name(name: &str) -> Option<(String, String)> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    match normalized.rsplit_once(':') {
        Some((base, tag)) if !base.is_empty() && !tag.is_empty() => {
            Some((base.to_string(), tag.to_string()))
        }
        _ => Some((normalized, "latest".to_string())),
    }
}

fn ollama_model_names_match(configured: &str, installed: &str) -> bool {
    normalize_ollama_model_name(configured)
        .zip(normalize_ollama_model_name(installed))
        .is_some_and(
            |((configured_base, configured_tag), (installed_base, installed_tag))| {
                configured_base == installed_base && configured_tag == installed_tag
            },
        )
}

fn is_ollama_embedding_model(model: &serde_json::Value) -> bool {
    let has_embedding_marker = |value: &str| {
        let normalized = value.trim().to_ascii_lowercase();
        normalized.contains("embed")
            || normalized.contains("embedding")
            || normalized.contains("text-embedding")
            || normalized == "bert"
    };

    if model["name"].as_str().is_some_and(has_embedding_marker) {
        return true;
    }

    let details = &model["details"];
    if details["family"].as_str().is_some_and(has_embedding_marker) {
        return true;
    }

    details["families"].as_array().is_some_and(|families| {
        families
            .iter()
            .filter_map(|family| family.as_str())
            .any(has_embedding_marker)
    })
}

fn ollama_show_response_supports_completion(data: &serde_json::Value) -> Option<bool> {
    data["capabilities"].as_array().map(|capabilities| {
        capabilities
            .iter()
            .filter_map(|capability| capability.as_str())
            .any(|capability| capability.eq_ignore_ascii_case("completion"))
    })
}

fn ollama_model_should_be_listed(
    model: &serde_json::Value,
    show_response: Option<&serde_json::Value>,
) -> bool {
    match show_response.and_then(ollama_show_response_supports_completion) {
        Some(supports_completion) => supports_completion,
        None => !is_ollama_embedding_model(model),
    }
}

#[allow(dead_code)]
fn parse_ollama_model_names(data: &serde_json::Value) -> Result<Vec<String>, AppError> {
    let models = data["models"]
        .as_array()
        .ok_or_else(|| AppError::Unknown("无法获取 Ollama 模型列表".to_string()))?;

    let mut names = models
        .iter()
        .filter(|model| !is_ollama_embedding_model(model))
        .filter_map(|model| model["name"].as_str().map(|name| name.trim().to_string()))
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();

    names.sort();
    names.dedup();

    Ok(names)
}

async fn fetch_ollama_show_response(
    client: &reqwest::Client,
    endpoint: &str,
    model_name: &str,
) -> Result<serde_json::Value, AppError> {
    let response = client
        .post(format!("{endpoint}/api/show"))
        .json(&serde_json::json!({
            "model": model_name,
            "verbose": false
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(AppError::Analysis(format!(
            "Ollama 模型详情返回错误: {}",
            response.status()
        )));
    }

    Ok(response.json().await?)
}

async fn resolve_ollama_text_model_names(
    client: &reqwest::Client,
    endpoint: &str,
    data: &serde_json::Value,
) -> Result<Vec<String>, AppError> {
    let models = data["models"]
        .as_array()
        .ok_or_else(|| AppError::Unknown("无法获取 Ollama 模型列表".to_string()))?;

    let mut join_set = tokio::task::JoinSet::new();
    for model in models {
        let Some(model_name) = model["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };

        let client = client.clone();
        let endpoint = endpoint.to_string();
        let model_name = model_name.to_string();
        let model_snapshot = model.clone();
        join_set.spawn(async move {
            let show_response = fetch_ollama_show_response(&client, &endpoint, &model_name).await;
            (model_snapshot, model_name, show_response)
        });
    }

    let mut filtered_names = Vec::new();
    while let Some(result) = join_set.join_next().await {
        let (model, model_name, show_response) = result
            .map_err(|error| AppError::Unknown(format!("查询 Ollama 模型详情失败: {error}")))?;

        match show_response {
            Ok(show_response) => {
                if ollama_model_should_be_listed(&model, Some(&show_response)) {
                    filtered_names.push(model_name);
                }
            }
            Err(error) => {
                if ollama_model_should_be_listed(&model, None) {
                    log::debug!(
                        "获取 Ollama 模型详情失败，回退名称规则后保留模型: model={}, error={}",
                        model_name,
                        error
                    );
                    filtered_names.push(model_name);
                } else {
                    log::debug!(
                        "获取 Ollama 模型详情失败，回退名称规则后排除模型: model={}, error={}",
                        model_name,
                        error
                    );
                }
            }
        }
    }

    filtered_names.sort();
    filtered_names.dedup();
    Ok(filtered_names)
}

#[tauri::command]
pub async fn get_ollama_models(endpoint: String) -> Result<Vec<String>, AppError> {
    let endpoint = endpoint.trim().trim_end_matches('/').to_string();
    if endpoint.is_empty() {
        return Err(AppError::Config("Ollama 地址不能为空".to_string()));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let response = client
        .get(format!("{endpoint}/api/tags"))
        .send()
        .await
        .map_err(|error| AppError::Analysis(format!("无法连接到 Ollama 服务: {error}")))?;

    if !response.status().is_success() {
        return Err(AppError::Analysis(format!(
            "Ollama 服务返回错误: {}",
            response.status()
        )));
    }

    let data: serde_json::Value = response.json().await?;
    resolve_ollama_text_model_names(&client, &endpoint, &data).await
}

/// 从 OpenAI 兼容提供商获取模型列表
async fn fetch_openai_compatible_models(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<Vec<String>, AppError> {
    let url = format!("{endpoint}/models");
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await;

    let response = match response {
        Ok(r) if r.status().is_success() => r,
        Ok(_) => {
            // 端点可能不含 /v1 前缀，重试 {endpoint}/v1/models
            let retry_url = format!("{endpoint}/v1/models");
            let retry = client
                .get(&retry_url)
                .header("Authorization", format!("Bearer {api_key}"))
                .send()
                .await
                .map_err(|e| AppError::Analysis(format!("无法获取模型列表: {e}")))?;
            if !retry.status().is_success() {
                return Err(AppError::Analysis(format!(
                    "API 返回错误: {}",
                    retry.status()
                )));
            }
            retry
        }
        Err(e) => return Err(AppError::Analysis(format!("请求失败: {e}"))),
    };

    let data: serde_json::Value = response.json().await?;
    let models = data["data"]
        .as_array()
        .ok_or_else(|| AppError::Analysis("无法解析模型列表（缺少 data 字段）".to_string()))?;

    let mut names: Vec<String> = models
        .iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .collect();

    names.sort();
    names.dedup();
    Ok(names)
}

/// 从 Google Gemini 获取模型列表
async fn fetch_gemini_models(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<Vec<String>, AppError> {
    let url = format!("{endpoint}/models?key={api_key}");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Analysis(format!("无法连接到 Gemini 服务: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(AppError::Analysis(format!(
            "Gemini API 错误 ({status}): {error_text}"
        )));
    }

    let data: serde_json::Value = response.json().await?;
    let models = data["models"]
        .as_array()
        .ok_or_else(|| AppError::Analysis("无法解析 Gemini 模型列表".to_string()))?;

    let mut names: Vec<String> = models
        .iter()
        // 仅保留支持 generateContent 的模型（排除 embedding 等专用模型）
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .map(|methods| {
                    methods
                        .iter()
                        .any(|m| m.as_str() == Some("generateContent"))
                })
                .unwrap_or(true)
        })
        .filter_map(|m| {
            m["name"]
                .as_str()
                .map(|name| name.strip_prefix("models/").unwrap_or(name).to_string())
        })
        .filter(|s| !s.is_empty())
        .collect();

    names.sort();
    names.dedup();
    Ok(names)
}

/// 从 Anthropic Claude 获取模型列表
async fn fetch_claude_models(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<Vec<String>, AppError> {
    let url = format!("{endpoint}/models");
    let response = client
        .get(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|e| AppError::Analysis(format!("无法连接到 Claude 服务: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(AppError::Analysis(format!(
            "Claude API 错误 ({status}): {error_text}"
        )));
    }

    let data: serde_json::Value = response.json().await?;
    let models = data["data"]
        .as_array()
        .ok_or_else(|| AppError::Analysis("无法解析 Claude 模型列表".to_string()))?;

    let mut names: Vec<String> = models
        .iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .collect();

    names.sort();
    names.dedup();
    Ok(names)
}

/// 通用获取模型列表（支持所有提供商）
#[tauri::command]
pub async fn fetch_models(
    provider: String,
    endpoint: String,
    api_key: Option<String>,
) -> Result<Vec<String>, AppError> {
    let endpoint = endpoint.trim().trim_end_matches('/').to_string();
    if endpoint.is_empty() {
        return Err(AppError::Config("API 地址不能为空".to_string()));
    }

    let provider: crate::config::AiProvider =
        serde_json::from_value(serde_json::Value::String(provider))
            .map_err(|_| AppError::Config("未知的 AI 提供商类型".to_string()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    match provider {
        crate::config::AiProvider::Ollama => {
            let response = client
                .get(format!("{endpoint}/api/tags"))
                .send()
                .await
                .map_err(|e| AppError::Analysis(format!("无法连接到 Ollama 服务: {e}")))?;
            if !response.status().is_success() {
                return Err(AppError::Analysis(format!(
                    "Ollama 服务返回错误: {}",
                    response.status()
                )));
            }
            let data: serde_json::Value = response.json().await?;
            resolve_ollama_text_model_names(&client, &endpoint, &data).await
        }
        crate::config::AiProvider::Gemini => {
            let api_key = api_key
                .filter(|k| !k.is_empty())
                .ok_or(AppError::Config("Gemini 需要 API Key".to_string()))?;
            fetch_gemini_models(&client, &endpoint, &api_key).await
        }
        crate::config::AiProvider::Claude => {
            let api_key = api_key
                .filter(|k| !k.is_empty())
                .ok_or(AppError::Config("Claude 需要 API Key".to_string()))?;
            fetch_claude_models(&client, &endpoint, &api_key).await
        }
        _ if provider.is_openai_compatible() => {
            let api_key = api_key
                .filter(|k| !k.is_empty())
                .ok_or(AppError::Config("需要 API Key".to_string()))?;
            fetch_openai_compatible_models(&client, &endpoint, &api_key).await
        }
        _ => Err(AppError::Config("不支持的提供商类型".to_string())),
    }
}

/// 获取支持的 AI 提供商列表
#[tauri::command]
pub async fn get_ai_providers() -> Result<Vec<serde_json::Value>, AppError> {
    Ok(vec![
        serde_json::json!({
            "id": "ollama",
            "name": "Ollama (本地)",
            "description": "在本机运行的开源大模型，数据不出本机",
            "default_endpoint": "http://localhost:11434",
            "default_model": "qwen3",
            "requires_api_key": false,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "openai",
            "name": "OpenAI / 兼容API",
            "description": "支持 OpenAI 官方及兼容 API（Azure、Cloudflare 等）",
            "default_endpoint": "https://api.openai.com/v1",
            "default_model": "gpt-5.1",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "siliconflow",
            "name": "硅基流动 SiliconFlow",
            "description": "国内高性价比 API，兼容 OpenAI 格式",
            "default_endpoint": "https://api.siliconflow.cn/v1",
            "default_model": "Qwen/Qwen3-8B",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "deepseek",
            "name": "DeepSeek",
            "description": "国产开源模型，性能强劲，兼容 OpenAI 格式",
            "default_endpoint": "https://api.deepseek.com",
            "default_model": "deepseek-v4-flash",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "qwen",
            "name": "通义千问 Qwen",
            "description": "阿里云通义大模型，兼容 OpenAI 格式",
            "default_endpoint": "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "default_model": "qwen-flash",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "zhipu",
            "name": "智谱 ChatGLM",
            "description": "智谱 AI 大模型",
            "default_endpoint": "https://open.bigmodel.cn/api/paas/v4",
            "default_model": "glm-4.6",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "moonshot",
            "name": "月之暗面 Kimi",
            "description": "Moonshot AI，擅长长文本",
            "default_endpoint": "https://api.moonshot.cn/v1",
            "default_model": "moonshot-v1-8k",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "doubao",
            "name": "火山引擎 豆包",
            "description": "字节跳动大模型",
            "default_endpoint": "https://ark.cn-beijing.volces.com/api/v3",
            "default_model": "doubao-lite-4k",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "minimax",
            "name": "稀宇科技 MiniMax",
            "description": "MiniMax 文本模型，兼容 OpenAI 格式",
            "default_endpoint": "https://api.minimaxi.com/v1",
            "default_model": "MiniMax-M2.5",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "gemini",
            "name": "Google Gemini",
            "description": "Google 的 Gemini 系列模型",
            "default_endpoint": "https://generativelanguage.googleapis.com/v1",
            "default_model": "gemini-2.5-flash",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "claude",
            "name": "Anthropic Claude",
            "description": "Anthropic 的 Claude 系列模型",
            "default_endpoint": "https://api.anthropic.com/v1",
            "default_model": "claude-3-7-sonnet-latest",
            "requires_api_key": true,
            "supports_vision": false,
        }),
    ])
}

/// 开始录制
#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.is_recording = true;
    state.is_paused = false;
    log::info!("开始录制");
    drop(state);
    crate::emit_recording_state_changed(&app);
    Ok(())
}

/// 停止录制
#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.is_recording = false;
    state.is_paused = false;
    log::info!("停止录制");
    drop(state);
    crate::emit_recording_state_changed(&app);
    Ok(())
}

/// 暂停录制
#[tauri::command]
pub async fn pause_recording(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.is_paused = true;
    log::info!("暂停录制");
    drop(state);
    crate::emit_recording_state_changed(&app);
    Ok(())
}

/// 恢复录制
#[tauri::command]
pub async fn resume_recording(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.is_recording = true;
    state.is_paused = false;
    log::info!("恢复录制");
    drop(state);
    crate::emit_recording_state_changed(&app);
    Ok(())
}

/// 获取录制状态
#[tauri::command]
pub async fn get_recording_state(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(bool, bool), AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok((state.is_recording, state.is_paused))
}

/// 获取当前桌宠状态
#[tauri::command]
pub async fn get_avatar_state(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<crate::avatar_engine::AvatarStatePayload, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(state.avatar_state.clone())
}

/// 保存桌宠窗口位置
#[tauri::command]
pub async fn save_avatar_position(
    x: i32,
    y: i32,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let config_path = state.config_path.clone();

    state.config.avatar_x = Some(x);
    state.config.avatar_y = Some(y);
    state.config.save(&config_path)?;

    Ok(())
}

/// 根据气泡/卡片展开状态调整桌宠窗口尺寸
#[tauri::command]
pub async fn set_avatar_window_expanded(
    expanded: bool,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let scale = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.config.avatar_scale
    };
    crate::avatar_engine::apply_avatar_window_expansion(&app, scale, expanded)
        .map_err(|e| AppError::Unknown(format!("调整桌宠窗口尺寸失败: {e}")))
}

/// 从桌面助手窗口读取当前位置并持久化
#[tauri::command]
pub async fn persist_avatar_position(
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<bool, AppError> {
    let Some(window) = app.get_webview_window(crate::avatar_engine::AVATAR_WINDOW_LABEL) else {
        return Ok(false);
    };

    let position = window
        .outer_position()
        .map_err(|e| AppError::Unknown(format!("读取桌面助手位置失败: {e}")))?;

    let mut state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let config_path = state.config_path.clone();

    state.config.avatar_x = Some(position.x);
    state.config.avatar_y = Some(position.y);
    state.config.save(&config_path)?;

    Ok(true)
}

/// 显示主窗口
#[tauri::command]
pub async fn show_main_window(
    app: AppHandle,
    source_window_label: Option<String>,
) -> Result<(), AppError> {
    crate::reveal_main_window(&app, source_window_label.as_deref())
}

#[tauri::command]
pub async fn handle_avatar_followup_action(
    input: AvatarFollowupActionInput,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let project_key = input.project_key.trim().to_string();
    if project_key.is_empty() {
        return Err(AppError::Unknown(
            "缺少项目标识，无法处理桌宠待跟进动作".to_string(),
        ));
    }

    let action = match input.action.trim() {
        "timeline" => crate::avatar_followup::AvatarFollowupAction::Timeline,
        "focus" => crate::avatar_followup::AvatarFollowupAction::Focus,
        "remember" => crate::avatar_followup::AvatarFollowupAction::Remember,
        "snooze" => crate::avatar_followup::AvatarFollowupAction::Snooze,
        _ => crate::avatar_followup::AvatarFollowupAction::Dismiss,
    };

    if matches!(
        action,
        crate::avatar_followup::AvatarFollowupAction::Remember
    ) {
        let mut config = {
            let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
            state.config.clone()
        };

        let normalized_title = input.title.trim().to_string();
        let exists = config.avatar_followups.iter().any(|item| {
            item.status == "open"
                && item.project_key == project_key
                && item.title.trim() == normalized_title
        });

        if !exists {
            config.avatar_followups.push(AvatarFollowupItem {
                id: uuid::Uuid::new_v4().to_string(),
                title: normalized_title,
                date: input.date.trim().to_string(),
                source_app: input.source_app.trim().to_string(),
                source_title: input.source_title.trim().to_string(),
                project_key: project_key.clone(),
                created_at: chrono::Local::now().timestamp(),
                status: "open".to_string(),
            });

            persist_app_config(config, app.clone(), state.inner())?;
        }
    }

    crate::avatar_followup::apply_followup_action(&project_key, action, &input.persona);

    Ok(serde_json::json!({
        "ok": true,
        "action": input.action,
        "projectKey": project_key,
    }))
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

#[tauri::command]
pub async fn get_runtime_platform() -> Result<String, AppError> {
    Ok(std::env::consts::OS.to_string())
}

#[tauri::command]
pub async fn get_linux_session_support() -> Result<LinuxSessionSupportInfo, AppError> {
    #[cfg(target_os = "linux")]
    {
        let session = current_linux_desktop_session();
        let desktop_environment = current_linux_desktop_environment();
        let active_window_provider =
            crate::monitor::current_linux_active_window_provider(session, desktop_environment);
        let avatar_input_support = crate::avatar_input::current_linux_avatar_input_support();
        let screenshot_support = crate::screenshot::current_linux_screenshot_support();
        let gnome_avatar_extension_installed = desktop_environment
            == crate::linux_session::LinuxDesktopEnvironment::Gnome
            && is_gnome_avatar_extension_installed();
        let gnome_avatar_extension_enabled = desktop_environment
            == crate::linux_session::LinuxDesktopEnvironment::Gnome
            && is_gnome_avatar_extension_enabled();
        let gnome_avatar_extension_needs_relogin = gnome_avatar_extension_needs_relogin(
            desktop_environment,
            gnome_avatar_extension_installed,
            gnome_avatar_extension_enabled,
            &avatar_input_support,
        );
        let active_window_supported = active_window_provider.is_some();
        let browser_url_support_level = if active_window_supported {
            "mixed"
        } else {
            "limited"
        };

        return Ok(LinuxSessionSupportInfo {
            platform: "linux".to_string(),
            session_type: session.as_str().to_string(),
            desktop_environment: desktop_environment.as_str().to_string(),
            active_window_provider: active_window_provider.unwrap_or("none").to_string(),
            active_window_supported,
            screenshot_supported: screenshot_support.supported,
            browser_url_support_level: browser_url_support_level.to_string(),
            avatar_input_provider: avatar_input_support.provider.to_string(),
            avatar_input_support_level: avatar_input_support.support_level.to_string(),
            avatar_keyboard_supported: avatar_input_support.keyboard_supported,
            avatar_mouse_supported: avatar_input_support.mouse_supported,
            gnome_avatar_extension_installed,
            gnome_avatar_extension_enabled,
            gnome_avatar_extension_needs_relogin,
        });
    }

    #[cfg(not(target_os = "linux"))]
    {
        Ok(LinuxSessionSupportInfo {
            platform: std::env::consts::OS.to_string(),
            session_type: "not_applicable".to_string(),
            desktop_environment: "not_applicable".to_string(),
            active_window_provider: "not_applicable".to_string(),
            active_window_supported: false,
            screenshot_supported: false,
            browser_url_support_level: "not_applicable".to_string(),
            avatar_input_provider: "not_applicable".to_string(),
            avatar_input_support_level: "not_applicable".to_string(),
            avatar_keyboard_supported: false,
            avatar_mouse_supported: false,
            gnome_avatar_extension_installed: false,
            gnome_avatar_extension_enabled: false,
            gnome_avatar_extension_needs_relogin: false,
        })
    }
}

#[tauri::command]
pub async fn install_gnome_avatar_extension() -> Result<GnomeAvatarExtensionInstallResult, AppError>
{
    #[cfg(target_os = "linux")]
    {
        let session = current_linux_desktop_session();
        let desktop_environment = current_linux_desktop_environment();

        if desktop_environment != crate::linux_session::LinuxDesktopEnvironment::Gnome {
            return Err(AppError::Unknown(
                "当前不是 GNOME 会话，无法自动安装 GNOME 桌宠扩展".to_string(),
            ));
        }

        let install_dir = gnome_avatar_extension_install_dir()?;
        std::fs::create_dir_all(&install_dir)
            .map_err(|e| AppError::Unknown(format!("创建 GNOME 扩展目录失败: {e}")))?;
        std::fs::write(
            install_dir.join("metadata.json"),
            GNOME_AVATAR_EXTENSION_METADATA,
        )
        .map_err(|e| AppError::Unknown(format!("写入 GNOME 扩展 metadata 失败: {e}")))?;
        std::fs::write(
            install_dir.join("extension.js"),
            GNOME_AVATAR_EXTENSION_SOURCE,
        )
        .map_err(|e| AppError::Unknown(format!("写入 GNOME 扩展脚本失败: {e}")))?;

        let enabled = std::process::Command::new("gnome-extensions")
            .args(["enable", GNOME_AVATAR_EXTENSION_UUID])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some()
            || is_gnome_avatar_extension_enabled();
        let avatar_input_support = crate::avatar_input::current_linux_avatar_input_support();
        let requires_relogin = gnome_avatar_extension_needs_relogin(
            desktop_environment,
            true,
            enabled,
            &avatar_input_support,
        );

        let message = if requires_relogin {
            "GNOME 桌宠扩展已启用，但当前 GNOME Shell 还未加载最新扩展。请重新登录后再试。"
                .to_string()
        } else if enabled {
            format!(
                "GNOME 桌宠扩展已安装并启用（{} / {}）",
                session.as_str(),
                desktop_environment.as_str()
            )
        } else {
            "GNOME 桌宠扩展文件已写入，请确认系统已安装 gnome-extensions 并手动启用扩展".to_string()
        };

        return Ok(GnomeAvatarExtensionInstallResult {
            installed: true,
            enabled,
            requires_relogin,
            extension_dir: install_dir.to_string_lossy().to_string(),
            message,
        });
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(AppError::Unknown(
            "只有 Linux GNOME 会话支持自动安装桌宠扩展".to_string(),
        ))
    }
}

#[tauri::command]
pub async fn quit_app_for_update(app: AppHandle) -> Result<(), AppError> {
    app.exit(0);
    Ok(())
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
                "目标目录包含非 Work Review 数据（{}），为避免误覆盖，请选择空目录或旧的数据目录",
                name
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

    log::info!("数据目录已切换到: {:?}", target_dir);
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
            format!("已清理旧目录中的 {} 项 Work Review 数据", removed_entries)
        } else {
            format!(
                "已清理旧目录中的 {} 项 Work Review 数据，并移除空目录",
                removed_entries
            )
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
                        format!("正在下载更新 {:.1} MB（{}）", mb, progress_source)
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
                        format!("下载完成，正在安装（{}）...", finish_source),
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

/// 获取截图缩略图
#[tauri::command]
pub async fn get_screenshot_thumbnail(
    path: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let full_path = state.data_dir.join(&path);
    state
        .screenshot_service
        .generate_thumbnail_base64(&full_path, 400)
}

/// 获取高分辨率截图（用于详情弹窗，1200px）
#[tauri::command]
pub async fn get_screenshot_full(
    path: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let full_path = state.data_dir.join(&path);
    state
        .screenshot_service
        .generate_full_image_base64(&full_path)
}

/// 测试远程存储连接
#[tauri::command]
pub async fn test_remote_storage(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let (remote_config, data_dir) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (guard.config.remote_storage.clone(), guard.data_dir.clone())
    };

    if remote_config.provider == work_review_core::config::RemoteStorageProvider::None {
        return Err(AppError::Config("未配置远程存储".into()));
    }

    let test_path = data_dir.join("remote_test.jpg");
    let test_bytes: Vec<u8> = {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([128, 128, 128]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg)
            .map_err(|e| AppError::Screenshot(format!("创建测试图片失败: {e}")))?;
        buf.into_inner()
    };
    tokio::fs::write(&test_path, &test_bytes)
        .await
        .map_err(|e| AppError::Screenshot(format!("写入测试文件失败: {e}")))?;

    let client = reqwest::Client::new();
    let result =
        crate::remote_upload::upload_screenshot(&client, &remote_config, &test_path, "test/connection-test.jpg")
            .await;

    let _ = tokio::fs::remove_file(&test_path).await;

    match result {
        Ok(url) => Ok(format!("连接成功: {url}")),
        Err(e) => Err(e),
    }
}

/// 手动执行一次截屏
#[tauri::command]
pub async fn take_screenshot(state: State<'_, Arc<Mutex<AppState>>>) -> Result<Activity, AppError> {
    let (
        screenshot_result,
        app_name,
        window_title,
        browser_url,
        category,
        semantic_category,
        semantic_confidence,
        relative_path,
        executable_path,
    ) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;

        // 获取当前活动窗口
        let active_window = crate::monitor::get_active_window().ok();

        #[cfg(target_os = "linux")]
        let active_window = if active_window.is_none()
            && !matches!(
                current_linux_desktop_session(),
                LinuxDesktopSession::Wayland
            ) {
            return Err(AppError::Unknown("获取当前活动窗口失败".to_string()));
        } else {
            active_window
        };

        #[cfg(not(target_os = "linux"))]
        let active_window = match active_window {
            Some(active_window) => Some(active_window),
            None => return Err(AppError::Unknown("获取当前活动窗口失败".to_string())),
        };

        // 检查隐私过滤
        if let Some(active_window) = active_window.as_ref() {
            if state.privacy_filter.check_privacy_full(
                &active_window.app_name,
                &active_window.window_title,
                active_window.browser_url.as_deref(),
            ) == crate::privacy::PrivacyAction::Skip
            {
                return Err(AppError::Privacy("当前窗口被隐私规则过滤".to_string()));
            }
        }

        // 执行截屏
        let result = state
            .screenshot_service
            .capture_for_window(active_window.as_ref())?;
        let relative_path = state.screenshot_service.get_relative_path(&result.path);
        let app_name = active_window
            .as_ref()
            .map(|window| window.app_name.clone())
            .unwrap_or_else(|| "Wayland Session".to_string());
        let window_title = active_window
            .as_ref()
            .map(|window| window.window_title.clone())
            .unwrap_or_else(|| "Wayland screenshot".to_string());
        let browser_url = active_window
            .as_ref()
            .and_then(|window| window.browser_url.clone());
        let executable_path = active_window
            .as_ref()
            .and_then(|window| window.executable_path.clone());
        let classification = crate::resolve_activity_classification(
            &state.config,
            &app_name,
            &window_title,
            browser_url.as_deref(),
        );

        (
            result,
            app_name,
            window_title,
            browser_url,
            classification.base_category,
            classification.semantic_category,
            classification.confidence,
            relative_path,
            executable_path,
        )
    };

    // 创建活动记录
    let activity = Activity {
        id: None,
        timestamp: screenshot_result.timestamp,
        app_name,
        window_title,
        screenshot_path: relative_path,
        ocr_text: None,
        category,
        duration: 30,
        browser_url,
        executable_path,
        semantic_category: Some(semantic_category),
        semantic_confidence: Some(i32::from(semantic_confidence)),
        screenshot_url: None,
    };

    // 保存到数据库
    let insert_result = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.database.insert_activity(&activity)
    };

    if let Err(error) = insert_result {
        let _ = std::fs::remove_file(&screenshot_result.path);
        if let Some(temp_path) = screenshot_result
            .ocr_source_path
            .as_ref()
            .filter(|path| *path != &screenshot_result.path)
        {
            let _ = std::fs::remove_file(temp_path);
        }
        return Err(error);
    }

    if let Some(temp_path) = screenshot_result
        .ocr_source_path
        .as_ref()
        .filter(|path| *path != &screenshot_result.path)
    {
        let _ = std::fs::remove_file(temp_path);
    }

    Ok(activity)
}

/// 获取历史应用列表 —— 内部复用版
pub(crate) fn get_recent_apps_inner(state: &Arc<Mutex<AppState>>) -> Result<Vec<String>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.get_recent_apps(50)
}

/// 历史应用详情 —— 内部复用版，供 localhost API 返回截图 URL
pub(crate) fn get_recent_app_usage_inner(
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<AppUsage>, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.get_recent_app_usage(50)
}

/// 获取历史应用列表（从数据库）
#[tauri::command]
pub async fn get_recent_apps(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<String>, AppError> {
    get_recent_apps_inner(state.inner())
}

/// 应用分类概览 —— 内部复用版
pub(crate) fn get_app_category_overview_inner(state: &Arc<Mutex<AppState>>) -> Result<Vec<AppCategoryOverviewItem>, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let overview = s.database.get_app_category_overview()?;

    Ok(overview
        .into_iter()
        .map(|item| {
            let override_category = crate::monitor::find_category_override(
                &s.config.app_category_rules,
                &item.app_name,
                &s.config.custom_categories,
            );
            let is_overridden = override_category.is_some();
            AppCategoryOverviewItem {
                app_name: item.app_name,
                category: override_category.unwrap_or(item.category),
                total_duration: item.total_duration,
                is_overridden,
                screenshot_url: item.screenshot_url,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn get_app_category_overview(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<AppCategoryOverviewItem>, AppError> {
    get_app_category_overview_inner(state.inner())
}

fn upsert_app_category_rule(config: &mut AppConfig, app_name: &str, category: &str) {
    let normalized_app_name = crate::monitor::normalize_display_app_name(app_name);
    let custom_keys: Vec<String> = config
        .custom_categories
        .iter()
        .map(|c| c.key.clone())
        .collect();
    let normalized_category = crate::config::normalize_category_key_private(category, &custom_keys);
    let match_key = normalized_app_name.to_lowercase();

    if let Some(rule) = config.app_category_rules.iter_mut().find(|rule| {
        crate::monitor::normalize_display_app_name(&rule.app_name).to_lowercase() == match_key
    }) {
        rule.app_name = normalized_app_name;
        rule.category = normalized_category;
        return;
    }

    config.app_category_rules.push(AppCategoryRule {
        app_name: normalized_app_name,
        category: normalized_category,
    });
}

fn reclassify_app_history_in_state(
    state: &AppState,
    app_name: &str,
    category: &str,
) -> Result<usize, AppError> {
    let custom_keys: Vec<String> = state
        .config
        .custom_categories
        .iter()
        .map(|c| c.key.clone())
        .collect();
    let target_category = crate::config::normalize_category_key_private(category, &custom_keys);
    let activities = state
        .database
        .get_activities_by_normalized_app_name(app_name)?;

    for activity in &activities {
        let classification = crate::activity_classifier::classify_activity_with_base_category(
            &activity.app_name,
            &activity.window_title,
            activity.browser_url.as_deref(),
            &target_category,
        );
        state.database.update_activity_classification(
            activity.id.expect("活动记录应包含主键"),
            &classification.base_category,
            Some(&classification.semantic_category),
            Some(i32::from(classification.confidence)),
        )?;
    }

    Ok(activities.len())
}

fn upsert_domain_semantic_rule(config: &mut AppConfig, domain: &str, semantic_category: &str) {
    let Some(normalized_domain) = crate::monitor::normalize_domain_rule(domain) else {
        return;
    };
    let normalized_semantic_category = semantic_category.trim().to_string();

    if let Some(rule) = config.website_semantic_rules.iter_mut().find(|rule| {
        crate::monitor::normalize_domain_rule(&rule.domain).as_deref()
            == Some(normalized_domain.as_str())
    }) {
        rule.domain = normalized_domain;
        rule.semantic_category = normalized_semantic_category;
        return;
    }

    config.website_semantic_rules.push(WebsiteSemanticRule {
        domain: normalized_domain,
        semantic_category: normalized_semantic_category,
    });
}

fn reclassify_domain_history_in_state(
    state: &AppState,
    domain: &str,
    semantic_category: &str,
) -> Result<usize, AppError> {
    let activities = state.database.get_activities_by_domain(domain)?;
    let semantic_category = semantic_category.trim();

    for activity in &activities {
        let next_base_category = crate::monitor::semantic_category_to_base_category(
            semantic_category,
            &activity.category,
        );
        state.database.update_activity_classification(
            activity.id.expect("活动记录应包含主键"),
            &next_base_category,
            Some(semantic_category),
            Some(100),
        )?;
    }

    Ok(activities.len())
}

#[tauri::command]
pub async fn set_app_category_rule(
    app_name: String,
    category: String,
    sync_history: bool,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let trimmed_app_name = app_name.trim();
    if trimmed_app_name.is_empty() {
        return Err(AppError::Unknown("应用名称不能为空".to_string()));
    }

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();
        upsert_app_category_rule(&mut next_config, trimmed_app_name, &category);
        next_config
    };

    persist_app_config(next_config, app, state.inner())?;

    if !sync_history {
        return Ok(0);
    }

    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    reclassify_app_history_in_state(&state, trimmed_app_name, &category)
}

#[tauri::command]
pub async fn reclassify_app_history(
    app_name: String,
    category: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    reclassify_app_history_in_state(&state, &app_name, &category)
}

/// 分类信息（前端展示用）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CategoryInfo {
    pub key: String,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub is_system: bool,
}

/// 分类信息 —— 内部复用版
pub(crate) fn get_categories_inner(state: &Arc<Mutex<AppState>>) -> Result<Vec<CategoryInfo>, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let mut result = Vec::new();
    for c in &s.config.custom_categories {
        result.push(CategoryInfo {
            key: c.key.clone(),
            name: c.name.clone(),
            color: c.color.clone(),
            icon: c.icon.clone(),
            is_system: c.key == "other",
        });
    }
    Ok(result)
}

#[tauri::command]
pub async fn get_categories(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<CategoryInfo>, AppError> {
    get_categories_inner(state.inner())
}

#[tauri::command]
pub async fn save_custom_category(
    key: String,
    name: String,
    color: String,
    icon: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let key = key.trim().to_lowercase();
    let name = name.trim().to_string();
    let color = color.trim().to_string();
    let icon = icon.trim().to_string();

    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(AppError::Unknown(
            "分类标识只能包含小写字母、数字和连字符".to_string(),
        ));
    }
    if name.is_empty() {
        return Err(AppError::Unknown("分类名称不能为空".to_string()));
    }
    if !color.starts_with('#') || color.len() != 7 {
        return Err(AppError::Unknown("颜色格式无效，需为 #RRGGBB".to_string()));
    }

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();

        let custom = crate::config::CustomCategory {
            key: key.clone(),
            name: name.clone(),
            color: color.clone(),
            icon: icon.clone(),
        };

        if let Some(existing) = next_config
            .custom_categories
            .iter_mut()
            .find(|c| c.key == key)
        {
            *existing = custom;
        } else {
            next_config.custom_categories.push(custom);
        }

        next_config
    };

    persist_app_config(next_config, app, state.inner())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_custom_category(
    key: String,
    reassign_to: Option<String>,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let key = key.trim().to_lowercase();
    let fallback = reassign_to.unwrap_or_else(|| "other".to_string());

    if key == "other" {
        return Err(AppError::Unknown("不能删除默认分类「其他」".to_string()));
    }

    let affected = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        // 统计引用该分类的规则数
        state
            .config
            .app_category_rules
            .iter()
            .filter(|r| r.category == key)
            .count()
    };

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();

        // 删除分类
        next_config.custom_categories.retain(|c| c.key != key);

        // 记录已删除的内置分类 key，防止 seed 复活
        if crate::config::DEFAULT_CATEGORY_KEYS.contains(&key.as_str())
            && !next_config.deleted_default_categories.contains(&key)
        {
            next_config.deleted_default_categories.push(key.clone());
        }

        // 重定向引用该分类的规则
        for rule in &mut next_config.app_category_rules {
            if rule.category == key {
                rule.category = fallback.clone();
            }
        }

        next_config
    };

    persist_app_config(next_config, app, state.inner())?;
    Ok(affected)
}

/// 语义分类信息（前端展示用）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SemanticCategoryInfo {
    pub key: String,
    pub name: String,
    pub is_system: bool,
}

/// 语义分类信息 —— 内部复用版
pub(crate) fn get_semantic_categories_inner(state: &Arc<Mutex<AppState>>) -> Result<Vec<SemanticCategoryInfo>, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let mut result = Vec::new();
    for c in &s.config.custom_semantic_categories {
        result.push(SemanticCategoryInfo {
            key: c.key.clone(),
            name: c.name.clone(),
            is_system: c.key == "未知活动",
        });
    }
    Ok(result)
}

#[tauri::command]
pub async fn get_semantic_categories(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<SemanticCategoryInfo>, AppError> {
    get_semantic_categories_inner(state.inner())
}

#[tauri::command]
pub async fn save_custom_semantic_category(
    key: String,
    name: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let key = key.trim().to_string();
    let name = name.trim().to_string();

    if key.is_empty() {
        return Err(AppError::Unknown("分类标识不能为空".to_string()));
    }
    if name.is_empty() {
        return Err(AppError::Unknown("分类名称不能为空".to_string()));
    }

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();

        let custom = CustomSemanticCategory {
            key: key.clone(),
            name: name.clone(),
        };

        if let Some(existing) = next_config
            .custom_semantic_categories
            .iter_mut()
            .find(|c| c.key == key)
        {
            *existing = custom;
        } else {
            next_config.custom_semantic_categories.push(custom);
        }

        next_config
    };

    persist_app_config(next_config, app, state.inner())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_custom_semantic_category(
    key: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let key = key.trim().to_string();

    if key == "未知活动" {
        return Err(AppError::Unknown("不能删除默认分类「未知活动」".to_string()));
    }

    let affected = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        // 统计引用该分类的规则数
        state
            .config
            .website_semantic_rules
            .iter()
            .filter(|r| r.semantic_category == key)
            .count()
    };

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();

        // 删除自定义语义分类
        next_config
            .custom_semantic_categories
            .retain(|c| c.key != key);

        // 记录已删除的内置语义分类 key，防止 seed 复活
        if crate::config::DEFAULT_SEMANTIC_CATEGORY_KEYS.contains(&key.as_str())
            && !next_config.deleted_default_semantic_categories.contains(&key)
        {
            next_config.deleted_default_semantic_categories.push(key.clone());
        }

        // 重定向引用该分类的规则到"未知活动"
        for rule in &mut next_config.website_semantic_rules {
            if rule.semantic_category == key {
                rule.semantic_category = "未知活动".to_string();
            }
        }

        next_config
    };

    persist_app_config(next_config, app, state.inner())?;
    Ok(affected)
}

#[tauri::command]
pub async fn set_domain_semantic_rule(
    domain: String,
    semantic_category: String,
    sync_history: bool,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<usize, AppError> {
    let normalized_domain = crate::monitor::normalize_domain_rule(&domain)
        .ok_or_else(|| AppError::Unknown("域名不能为空".to_string()))?;
    let trimmed_semantic_category = semantic_category.trim();
    if trimmed_semantic_category.is_empty() {
        return Err(AppError::Unknown("语义分类不能为空".to_string()));
    }

    let next_config = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let mut next_config = state.config.clone();
        upsert_domain_semantic_rule(
            &mut next_config,
            &normalized_domain,
            trimmed_semantic_category,
        );
        next_config
    };

    persist_app_config(next_config, app, state.inner())?;

    if !sync_history {
        return Ok(0);
    }

    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    reclassify_domain_history_in_state(&state, &normalized_domain, trimmed_semantic_category)
}

/// 获取当前运行的应用列表
#[tauri::command]
pub async fn get_running_apps() -> Result<Vec<String>, AppError> {
    get_running_apps_impl()
}

/// macOS 实现
#[cfg(target_os = "macos")]
fn get_running_apps_impl() -> Result<Vec<String>, AppError> {
    use std::process::Command;

    // 使用 AppleScript 获取运行中的应用
    let output = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to get name of every process whose background only is false"#
        ])
        .output()
        .map_err(|e| AppError::Unknown(format!("执行 AppleScript 失败: {e}")))?;

    if output.status.success() {
        let apps_str = String::from_utf8_lossy(&output.stdout);
        let mut apps: Vec<String> = apps_str
            .split(", ")
            .map(|s| crate::monitor::normalize_display_app_name(s))
            .filter(|s| !s.is_empty())
            .collect();
        apps.sort();
        apps.dedup();
        Ok(apps)
    } else {
        Err(AppError::Unknown("获取应用列表失败".to_string()))
    }
}

/// Windows 实现
#[cfg(target_os = "windows")]
fn get_running_apps_impl() -> Result<Vec<String>, AppError> {
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut apps = HashSet::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() {
            return Ok(vec![]);
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                // 获取进程名
                let name_len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = OsString::from_wide(&entry.szExeFile[..name_len])
                    .to_string_lossy()
                    .to_string();

                // 排除系统进程
                let name_lower = name.to_lowercase();
                if !name_lower.ends_with(".exe") {
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                    continue;
                }

                // 排除常见系统进程
                let excluded = [
                    "svchost.exe",
                    "csrss.exe",
                    "wininit.exe",
                    "services.exe",
                    "lsass.exe",
                    "smss.exe",
                    "winlogon.exe",
                    "dwm.exe",
                    "fontdrvhost.exe",
                    "sihost.exe",
                    "taskhostw.exe",
                    "runtimebroker.exe",
                    "searchhost.exe",
                    "startmenuexperiencehost.exe",
                    "textinputhost.exe",
                    "ctfmon.exe",
                    "conhost.exe",
                ];

                if !excluded.contains(&name_lower.as_str()) {
                    // 移除 .exe 后缀
                    let display_name = crate::monitor::normalize_display_app_name(&name);
                    apps.insert(display_name);
                }

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
    }

    let mut result: Vec<String> = apps.into_iter().collect();
    result.sort();
    Ok(result)
}

/// 其他平台
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn get_running_apps_impl() -> Result<Vec<String>, AppError> {
    Ok(vec![])
}

/// 获取存储统计信息 —— 内部复用版
pub(crate) fn get_storage_stats_inner(state: &Arc<Mutex<AppState>>) -> Result<serde_json::Value, AppError> {
    let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let stats = s
        .storage_manager
        .get_stats()
        .map_err(|e| AppError::Unknown(e.to_string()))?;

    Ok(serde_json::json!({
        "total_files": stats.total_files,
        "total_size_mb": format!("{:.1}", stats.total_size_mb),
        "storage_limit_mb": stats.storage_limit_mb,
        "retention_days": stats.retention_days,
    }))
}

/// 获取存储统计信息
#[tauri::command]
pub async fn get_storage_stats(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    get_storage_stats_inner(state.inner())
}

/// 获取指定日期的小时摘要 —— 内部复用版
pub(crate) fn get_hourly_summaries_inner(
    date: &str,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<serde_json::Value>, AppError> {
    let app_state = state.clone();

    for hour in 0..24 {
        crate::generate_and_save_summary(&app_state, date, hour);
    }

    let s = app_state
        .lock()
        .map_err(|e| AppError::Unknown(e.to_string()))?;
    let summaries = s.database.get_hourly_summaries(date)?;

    Ok(summaries
        .iter()
        .map(|s| {
            serde_json::json!({
                "hour": s.hour,
                "summary": s.summary,
                "main_apps": s.main_apps,
                "activity_count": s.activity_count,
                "total_duration": s.total_duration,
            })
        })
        .collect())
}

/// 获取指定日期的小时摘要
#[tauri::command]
pub async fn get_hourly_summaries(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<serde_json::Value>, AppError> {
    get_hourly_summaries_inner(&date, state.inner())
}

/// 清理今天之前的所有活动记录
#[tauri::command]
pub async fn clear_old_activities(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let data_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.data_dir.clone()
    };

    // 获取要保留的日期（今天和昨天）
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let yesterday = (now - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    let mut deleted_screenshots = 0;

    // 删除旧截图目录（保留今天和昨天）
    let screenshots_dir = data_dir.join("screenshots");
    if screenshots_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&screenshots_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    // 保留今天和昨天的目录
                    if name != today && name != yesterday && entry.path().is_dir() {
                        if let Ok(dir_entries) = std::fs::read_dir(entry.path()) {
                            for file_entry in dir_entries.flatten() {
                                if file_entry.path().is_file() {
                                    deleted_screenshots += 1;
                                }
                            }
                        }
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
    }

    // 同步清理数据库中对应的旧记录
    {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        if let Err(e) = state.database.delete_activities_before_date(&today) {
            log::warn!("清理旧活动记录失败: {e}");
        }
    }

    Ok(serde_json::json!({
        "deleted_screenshots": deleted_screenshots,
        "kept_dates": [today, yesterday],
        "message": format!("已清理 {} 张旧截图和对应活动记录，保留今天和昨天的数据", deleted_screenshots)
    }))
}

/// 获取指定日期的 OCR 日志
#[tauri::command]
pub async fn get_ocr_log(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<String, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let ocr_logger = crate::ocr_logger::OcrLogger::new(&state.data_dir);
    ocr_logger.read_log(&date)
}

/// 检查屏幕锁定状态
#[tauri::command]
pub async fn is_screen_locked() -> Result<bool, AppError> {
    let monitor = crate::screen_lock::ScreenLockMonitor::new();
    Ok(monitor.is_locked())
}

/// 检查 macOS 系统权限状态（屏幕录制 + 辅助功能）
/// Windows 上始终返回全部已授权
#[tauri::command]
pub async fn check_permissions() -> Result<serde_json::Value, AppError> {
    let screen_capture = crate::screenshot::has_screen_capture_permission();
    let accessibility = crate::screenshot::has_accessibility_permission(false);
    let input_monitoring = crate::screenshot::has_input_monitoring_permission();

    #[cfg(target_os = "linux")]
    let screenshot_supported = crate::screenshot::current_linux_screenshot_support().supported;
    #[cfg(not(target_os = "linux"))]
    let screenshot_supported = screen_capture;

    #[cfg(target_os = "linux")]
    let avatar_input_supported =
        crate::avatar_input::current_linux_avatar_input_support().support_level != "none";
    #[cfg(target_os = "macos")]
    let avatar_input_supported = accessibility && input_monitoring;
    #[cfg(target_os = "windows")]
    let avatar_input_supported = true;
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let avatar_input_supported = false;

    let all_granted = if cfg!(target_os = "macos") {
        screen_capture && accessibility && input_monitoring
    } else {
        screenshot_supported && avatar_input_supported
    };

    Ok(serde_json::json!({
        "screen_capture": screen_capture,
        "accessibility": accessibility,
        "input_monitoring": input_monitoring,
        "screenshot_supported": screenshot_supported,
        "avatar_input_supported": avatar_input_supported,
        "all_granted": all_granted,
        "platform": std::env::consts::OS,
    }))
}

#[cfg(target_os = "macos")]
fn macos_permission_settings_url(permission: &str) -> Option<&'static str> {
    match permission {
        "screen_capture" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        }
        "accessibility" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        }
        "input_monitoring" => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        }
        _ => None,
    }
}

/// 打开系统权限设置页
#[tauri::command]
pub async fn open_permission_settings(permission: String) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        match permission.as_str() {
            "screen_capture" => crate::screenshot::request_screen_capture_permission(),
            "accessibility" => {
                crate::screenshot::has_accessibility_permission(true);
            }
            "input_monitoring" => crate::screenshot::request_input_monitoring_permission(),
            _ => {}
        }

        let target = macos_permission_settings_url(&permission)
            .ok_or_else(|| AppError::Unknown(format!("不支持的权限类型: {}", permission)))?;

        std::process::Command::new("open")
            .arg(target)
            .spawn()
            .map_err(|e| AppError::Unknown(format!("打开系统权限设置失败: {e}")))?;

        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = permission;
        Err(AppError::Unknown(
            "当前平台暂不支持直接跳转系统权限设置".to_string(),
        ))
    }
}

/// 检查是否在工作时间内
#[tauri::command]
pub async fn is_work_time(state: State<'_, Arc<Mutex<AppState>>>) -> Result<bool, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    let segments = state.config.effective_work_segments();
    Ok(crate::screen_lock::ScreenLockMonitor::is_work_time_in_segments(&segments))
}

/// 检查 PaddleOCR 是否可用
#[tauri::command]
pub async fn check_ocr_available() -> Result<serde_json::Value, AppError> {
    let paddle_available = crate::ocr::OcrService::check_paddle_available();

    Ok(serde_json::json!({
        "paddle_ocr_available": paddle_available,
        "install_command": crate::ocr::OcrService::get_paddle_install_command(),
        "platform": std::env::consts::OS,
    }))
}

/// 执行 OCR 识别
#[tauri::command]
pub async fn run_ocr(
    screenshot_path: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let (data_dir, full_path) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let full_path = state.data_dir.join(&screenshot_path);
        (state.data_dir.clone(), full_path)
    };

    if !full_path.exists() {
        return Err(AppError::Unknown(format!("截图文件不存在: {full_path:?}")));
    }

    let ocr_service = crate::ocr::OcrService::new(&data_dir);

    match ocr_service.extract_text(&full_path) {
        Ok(Some(result)) => {
            // 过滤敏感信息
            let filtered_text = crate::ocr::filter_sensitive_text(&result.text);

            Ok(serde_json::json!({
                "success": true,
                "text": filtered_text,
                "raw_text": result.text,
                "confidence": result.confidence,
                "box_count": result.boxes.len(),
            }))
        }
        Ok(None) => Ok(serde_json::json!({
            "success": true,
            "text": "",
            "message": "未检测到文字",
        })),
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "error": e.to_string(),
        })),
    }
}

/// 获取 OCR 安装指南
#[tauri::command]
pub async fn get_ocr_install_guide() -> Result<serde_json::Value, AppError> {
    let platform = std::env::consts::OS;

    let guide = match platform {
        "windows" => serde_json::json!({
            "platform": "Windows",
            "steps": [
                "1. 确保已安装 Python 3.8+",
                "2. 打开命令提示符或 PowerShell",
                "3. 运行以下命令安装 PaddleOCR：",
                "   pip install paddlepaddle paddleocr -i https://mirror.baidu.com/pypi/simple",
                "4. 等待安装完成（首次运行会自动下载模型）",
                "",
                "备选方案：使用 Windows 内置 OCR（无需安装，但识别效果较弱）"
            ],
            "install_command": "pip install paddlepaddle paddleocr -i https://mirror.baidu.com/pypi/simple",
            "has_builtin_fallback": true,
        }),
        "macos" => serde_json::json!({
            "platform": "macOS",
            "steps": [
                "macOS 使用系统内置的 Vision 框架进行 OCR，无需额外安装。",
                "",
                "如需使用 PaddleOCR（效果更好）：",
                "1. 确保已安装 Python 3.8+",
                "2. 运行以下命令：",
                "   pip install paddlepaddle paddleocr",
            ],
            "install_command": "pip install paddlepaddle paddleocr",
            "has_builtin_fallback": true,
        }),
        _ => serde_json::json!({
            "platform": platform,
            "steps": [
                "1. 确保已安装 Python 3.8+",
                "2. 运行以下命令安装 PaddleOCR：",
                "   pip install paddlepaddle paddleocr",
            ],
            "install_command": "pip install paddlepaddle paddleocr",
            "has_builtin_fallback": false,
        }),
    };

    Ok(guide)
}

/// 设置 Dock 图标可见性 (仅 macOS)
#[tauri::command]
#[allow(unused_variables)]
#[allow(unexpected_cfgs)]
pub fn set_dock_visibility(visible: bool) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        apply_dock_visibility(visible, true);
        log::info!("Dock 图标可见性已设置为: {visible}");
    }

    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("set_dock_visibility 仅支持 macOS");
    }

    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn refresh_dock_icon(activate: bool) {
    use cocoa::appkit::{NSApp, NSImage};
    use cocoa::base::nil;
    use cocoa::foundation::NSString;
    use objc::runtime::Object;

    unsafe {
        let app: *mut Object = NSApp();

        // 使用 NSBundle.mainBundle 获取图标路径
        let bundle: *mut Object = objc::msg_send![objc::class!(NSBundle), mainBundle];
        let resource: *mut Object = objc::msg_send![
            bundle,
            pathForResource: NSString::alloc(nil).init_str("icon")
            ofType: NSString::alloc(nil).init_str("icns")
        ];

        // 如果 bundle 中找不到，尝试硬编码路径
        let path_to_use = if resource != nil {
            resource
        } else {
            NSString::alloc(nil)
                .init_str("/Applications/Work Review.app/Contents/Resources/icon.icns")
        };

        let image: *mut Object = NSImage::alloc(nil).initByReferencingFile_(path_to_use);
        if image != nil {
            let _: () = objc::msg_send![app, setApplicationIconImage: image];
            log::info!("已重新设置 Dock 图标");
        }

        if activate {
            let _: () = objc::msg_send![app, activateIgnoringOtherApps: true];
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn apply_dock_visibility(visible: bool, activate: bool) {
    use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
    use objc::runtime::Object;

    unsafe {
        let app: *mut Object = NSApp();

        if visible {
            // 显示 Dock 图标: 切换回 Regular 策略
            app.setActivationPolicy_(
                NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular,
            );

            // 切换 ActivationPolicy 后主动重载图标，避免启动后 Dock 残留旧图标缓存
            refresh_dock_icon(activate);
        } else {
            // 隐藏 Dock 图标: 切换到 Accessory 策略
            app.setActivationPolicy_(
                NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn apply_dock_visibility(_visible: bool, _activate: bool) {}

/// 获取应用图标（Base64 PNG）
/// 返回应用的图标，如果获取失败返回空字符串
#[tauri::command]
pub async fn get_app_icon(
    app_name: String,
    executable_path: Option<String>,
) -> Result<String, AppError> {
    get_app_icon_impl(&app_name, executable_path.as_deref()).await
}

#[cfg(any(target_os = "macos", test))]
fn normalize_macos_app_lookup_name(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches(".app");
    let mut normalized = String::new();
    let mut last_was_space = false;

    for ch in trimmed.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_alphanumeric() {
            normalized.push(ch);
            last_was_space = false;
        } else if !last_was_space {
            normalized.push(' ');
            last_was_space = true;
        }
    }

    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(any(target_os = "macos", test))]
fn push_normalized_macos_lookup_name(target: &mut Vec<String>, value: &str) {
    let normalized = normalize_macos_app_lookup_name(value);
    if normalized.is_empty() || target.iter().any(|existing| existing == &normalized) {
        return;
    }
    target.push(normalized);
}

#[cfg(any(target_os = "macos", test))]
fn macos_lookup_name_variants(value: &str) -> Vec<String> {
    const ALIAS_GROUPS: &[&[&str]] = &[&["腾讯视频", "QQLive", "Tencent Video"]];

    let normalized = normalize_macos_app_lookup_name(value);
    if normalized.is_empty() {
        return Vec::new();
    }

    let mut variants = Vec::new();
    push_normalized_macos_lookup_name(&mut variants, value);

    for group in ALIAS_GROUPS {
        if !group
            .iter()
            .any(|alias| normalize_macos_app_lookup_name(alias) == normalized)
        {
            continue;
        }

        for alias in *group {
            push_normalized_macos_lookup_name(&mut variants, alias);
        }
    }

    variants
}

#[cfg(any(target_os = "macos", test))]
fn macos_significant_name_tokens(value: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &["app", "browser", "desktop", "helper", "tools"];

    let mut tokens = Vec::new();
    for token in normalize_macos_app_lookup_name(value).split_whitespace() {
        if token.len() < 2 || STOPWORDS.contains(&token) {
            continue;
        }
        if !tokens.iter().any(|existing| existing == token) {
            tokens.push(token.to_string());
        }
    }
    tokens
}

#[cfg(target_os = "macos")]
fn macos_bundle_path_from_executable(executable_path: &str) -> Option<PathBuf> {
    let path = Path::new(executable_path);
    for ancestor in path.ancestors() {
        let is_app_bundle = ancestor
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("app"))
            .unwrap_or(false);
        if is_app_bundle {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

#[cfg(any(target_os = "macos", test))]
fn score_normalized_macos_app_bundle_name(normalized_app: &str, normalized_bundle: &str) -> i32 {
    if normalized_app.is_empty() || normalized_bundle.is_empty() {
        return 0;
    }

    let mut score = 0;
    if normalized_app == normalized_bundle {
        score += 1000;
    } else if normalized_app.contains(&normalized_bundle)
        || normalized_bundle.contains(&normalized_app)
    {
        score += 500;
    }

    let app_tokens = macos_significant_name_tokens(&normalized_app);
    let bundle_tokens = macos_significant_name_tokens(&normalized_bundle);
    let overlap_count = bundle_tokens
        .iter()
        .filter(|token| app_tokens.iter().any(|candidate| candidate == *token))
        .count() as i32;
    score += overlap_count * 160;

    if let Some(first_token) = app_tokens.first() {
        if normalized_bundle.starts_with(first_token) {
            score += 80;
        }
    }

    score
}

#[cfg(any(target_os = "macos", test))]
fn macos_score_app_bundle_name(app_name: &str, bundle_name: &str) -> i32 {
    let app_variants = macos_lookup_name_variants(app_name);
    let bundle_variants = macos_lookup_name_variants(bundle_name);

    let mut best_score = 0;
    for normalized_app in &app_variants {
        for normalized_bundle in &bundle_variants {
            best_score = best_score.max(score_normalized_macos_app_bundle_name(
                normalized_app,
                normalized_bundle,
            ));
        }
    }

    best_score
}

#[cfg(target_os = "macos")]
fn collect_macos_app_bundles(root: &Path, depth: usize, bundles: &mut Vec<PathBuf>) {
    if depth == 0 || !root.exists() {
        return;
    }

    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let is_app_bundle = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("app"))
            .unwrap_or(false);
        if is_app_bundle {
            bundles.push(path);
            continue;
        }

        collect_macos_app_bundles(&path, depth.saturating_sub(1), bundles);
    }
}

#[cfg(target_os = "macos")]
fn macos_icon_app_path_candidates(app_name: &str, executable_path: Option<&str>) -> Vec<String> {
    let mut candidates: Vec<(i32, String)> = Vec::new();

    if let Some(path) = executable_path.and_then(macos_bundle_path_from_executable) {
        candidates.push((i32::MAX, path.to_string_lossy().to_string()));
    }

    let mut search_roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home_dir) = dirs::home_dir() {
        search_roots.push(home_dir.join("Applications"));
    }

    let mut bundles = Vec::new();
    for root in search_roots {
        collect_macos_app_bundles(&root, 3, &mut bundles);
    }

    for bundle in bundles {
        let Some(bundle_name) = bundle.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let score = macos_score_app_bundle_name(app_name, bundle_name);
        if score <= 0 {
            continue;
        }
        candidates.push((score, bundle.to_string_lossy().to_string()));
    }

    candidates.sort_by(|(score_a, path_a), (score_b, path_b)| {
        score_b
            .cmp(score_a)
            .then_with(|| path_a.len().cmp(&path_b.len()))
            .then_with(|| path_a.cmp(path_b))
    });

    let mut deduped = Vec::new();
    for (_, path) in candidates {
        if deduped.iter().any(|existing| existing == &path) {
            continue;
        }
        deduped.push(path);
    }
    deduped
}

/// macOS 实现：使用 mdfind 获取应用图标（带磁盘缓存）
#[cfg(target_os = "macos")]
async fn get_app_icon_impl(
    app_name: &str,
    executable_path: Option<&str>,
) -> Result<String, AppError> {
    use std::path::Path;
    use std::process::Command;

    // 缓存目录：/tmp/work_review_icons/
    let cache_dir = Path::new("/tmp/work_review_icons");
    if !cache_dir.exists() {
        let _ = std::fs::create_dir_all(cache_dir);
    }

    // 安全文件名：将空格和特殊字符替换为下划线
    let safe_name: String = app_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let cache_file = cache_dir.join(format!("{safe_name}.b64"));

    // 检查缓存：如果缓存存在且非空，直接返回
    if cache_file.exists() {
        if let Ok(cached) = std::fs::read_to_string(&cache_file) {
            if !cached.is_empty() {
                log::debug!("从缓存读取图标: {app_name}");
                return Ok(cached);
            }
        }
    }

    let app_path = macos_icon_app_path_candidates(app_name, executable_path)
        .into_iter()
        .find(|candidate| Path::new(candidate).exists())
        .unwrap_or_default();

    if app_path.is_empty() {
        log::debug!("未找到应用路径: {app_name}");
        return Ok(String::new());
    }

    log::debug!("找到应用路径: {app_name} -> {app_path}");

    // 获取 Info.plist 中的图标文件名
    let info_plist = format!("{app_path}/Contents/Info.plist");
    let icon_name = if Path::new(&info_plist).exists() {
        // 使用 defaults read 读取 CFBundleIconFile
        let defaults_output = Command::new("defaults")
            .args(["read", &info_plist, "CFBundleIconFile"])
            .output();

        if let Ok(output) = defaults_output {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                // 确保有 .icns 扩展名
                if name.ends_with(".icns") {
                    name
                } else {
                    format!("{name}.icns")
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // 构造图标文件路径
    let icns_path = if !icon_name.is_empty() {
        format!("{app_path}/Contents/Resources/{icon_name}")
    } else {
        // 尝试查找任何 .icns 文件
        let find_output = Command::new("find")
            .args([
                &format!("{app_path}/Contents/Resources"),
                "-name",
                "*.icns",
                "-maxdepth",
                "1",
            ])
            .output()
            .map_err(|e| AppError::Unknown(format!("查找图标失败: {e}")))?;

        String::from_utf8_lossy(&find_output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    };

    if icns_path.is_empty() || !Path::new(&icns_path).exists() {
        log::debug!("未找到图标文件: {app_name}");
        return Ok(String::new());
    }

    log::debug!("找到图标文件: {icns_path}");

    // 使用 sips 转换为 PNG
    let temp_png = format!(
        "/tmp/app_icon_{}_{}.png",
        app_name.replace(' ', "_"),
        std::process::id()
    );

    let sips_output = Command::new("sips")
        .args([
            "-s", "format", "png", "-Z", "128", &icns_path, "--out", &temp_png,
        ])
        .output();

    if let Ok(result) = sips_output {
        if result.status.success() {
            if let Ok(png_data) = std::fs::read(&temp_png) {
                let _ = std::fs::remove_file(&temp_png);
                let base64_str =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_data);
                // 保存到缓存
                let _ = std::fs::write(&cache_file, &base64_str);
                log::debug!("图标已缓存: {} ({} bytes)", app_name, base64_str.len());
                return Ok(base64_str);
            }
        } else {
            log::debug!("sips 转换失败: {}", String::from_utf8_lossy(&result.stderr));
        }
    }

    let _ = std::fs::remove_file(&temp_png);
    Ok(String::new())
}

/// Windows 实现：使用 Shell API 获取高清应用图标
/// 优先提取 256x256 (JUMBO) 图标，降级到 48x48 (EXTRALARGE)，最后回退到 32x32
/// 带磁盘缓存，避免重复启动 PowerShell
#[cfg(any(target_os = "windows", test))]
fn sanitize_icon_cache_name(value: &str) -> String {
    let safe_name: String = value
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();

    if safe_name.is_empty() {
        "icon".to_string()
    } else {
        safe_name
    }
}

#[cfg(any(target_os = "windows", test))]
fn build_windows_icon_cache_key(app_name: &str, executable_path: Option<&str>) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let safe_name = sanitize_icon_cache_name(app_name);
    let Some(path) = executable_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return safe_name;
    };

    let mut hasher = DefaultHasher::new();
    path.to_lowercase().hash(&mut hasher);
    format!("{safe_name}_{:016x}", hasher.finish())
}

#[cfg(any(target_os = "windows", test))]
fn merge_windows_icon_lookup_candidates(
    executable_path: Option<&str>,
    known_icon_paths: Vec<String>,
) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut push_candidate = |value: &str| {
        let candidate = value.trim().trim_matches('"').replace('/', "\\");
        if !candidate.is_empty() && !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    };

    if let Some(path) = executable_path {
        push_candidate(path);
    }

    for path in known_icon_paths {
        push_candidate(&path);
    }

    candidates
}

#[cfg(target_os = "windows")]
fn windows_known_icon_paths(app_name: &str) -> Vec<String> {
    let trimmed = app_name
        .trim()
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .trim();
    let normalized = trimmed.to_lowercase();
    let compact = normalized
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>();

    let program_files = std::env::var("ProgramFiles").unwrap_or_default();
    let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let app_data = std::env::var("APPDATA").unwrap_or_default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());

    let mut paths = Vec::new();
    let mut push_path = |path: String| {
        if !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
    };

    match compact.as_str() {
        "explorer" | "fileexplorer" => {
            push_path(format!(r"{}\explorer.exe", windir));
        }
        "msedge" | "edge" | "microsoftedge" => {
            push_path(format!(
                r"{}\Microsoft\Edge\Application\msedge.exe",
                program_files_x86
            ));
            push_path(format!(
                r"{}\Microsoft\Edge\Application\msedge.exe",
                program_files
            ));
        }
        "chrome" | "googlechrome" => {
            push_path(format!(
                r"{}\Google\Chrome\Application\chrome.exe",
                program_files
            ));
            push_path(format!(
                r"{}\Google\Chrome\Application\chrome.exe",
                program_files_x86
            ));
        }
        "wechat" | "weixin" => {
            push_path(format!(r"{}\Tencent\WeChat\WeChat.exe", program_files_x86));
            push_path(format!(r"{}\Tencent\WeChat\WeChat.exe", program_files));
        }
        "wecom" | "wxwork" => {
            push_path(format!(r"{}\Tencent\WeCom\WXWork.exe", program_files_x86));
            push_path(format!(r"{}\Tencent\WeCom\WXWork.exe", program_files));
        }
        "obsidian" => {
            push_path(format!(
                r"{}\Programs\Obsidian\Obsidian.exe",
                local_app_data
            ));
        }
        "pixpin" => {
            push_path(format!(r"{}\PixPin\PixPin.exe", local_app_data));
        }
        "xshell" => {
            push_path(format!(
                r"{}\NetSarang Computer\7\Xshell.exe",
                program_files_x86
            ));
            push_path(format!(
                r"{}\NetSarang Computer\7\Xshell.exe",
                program_files
            ));
            push_path(format!(
                r"{}\NetSarang Computer\8\Xshell.exe",
                program_files_x86
            ));
            push_path(format!(
                r"{}\NetSarang Computer\8\Xshell.exe",
                program_files
            ));
        }
        "wechatappex" => {
            push_path(format!(
                r"{}\Tencent\WeChat\XPlugin\Plugins\WeChatAppEx\WeChatAppEx.exe",
                app_data
            ));
        }
        _ => {}
    }

    paths
}

#[cfg(target_os = "windows")]
fn encode_windows_icon_path(value: &str) -> Vec<u16> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
unsafe fn get_windows_icon_from_shell_image_list(
    path: &str,
    list_type: i32,
) -> Option<winapi::shared::windef::HICON> {
    use std::mem::zeroed;
    use std::ptr::null_mut;
    use winapi::ctypes::c_void;
    use winapi::um::commoncontrols::IImageList;
    use winapi::um::shellapi::{SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_SYSICONINDEX};
    use winapi::Interface;

    let wide_path = encode_windows_icon_path(path);
    let mut file_info: SHFILEINFOW = zeroed();
    let lookup_result = SHGetFileInfoW(
        wide_path.as_ptr(),
        0,
        &mut file_info,
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_SYSICONINDEX,
    );
    if lookup_result == 0 {
        return None;
    }

    let mut image_list: *mut IImageList = null_mut();
    let hr = SHGetImageList(
        list_type,
        &IImageList::uuidof(),
        &mut image_list as *mut _ as *mut *mut c_void,
    );
    if hr < 0 || image_list.is_null() {
        return None;
    }

    let mut icon = null_mut();
    let hr = (*image_list).GetIcon(file_info.iIcon, 0, &mut icon);
    (*image_list).Release();

    if hr < 0 || icon.is_null() {
        None
    } else {
        Some(icon)
    }
}

#[cfg(target_os = "windows")]
unsafe fn get_windows_associated_icon(path: &str) -> Option<winapi::shared::windef::HICON> {
    use std::ptr::null_mut;
    use winapi::shared::minwindef::WORD;
    use winapi::um::shellapi::ExtractAssociatedIconW;

    let mut wide_path = encode_windows_icon_path(path);
    if wide_path.len() < 260 {
        wide_path.resize(260, 0);
    }

    let mut icon_index: WORD = 0;
    let icon = ExtractAssociatedIconW(null_mut(), wide_path.as_mut_ptr(), &mut icon_index);
    if icon.is_null() {
        None
    } else {
        Some(icon)
    }
}

#[cfg(target_os = "windows")]
unsafe fn render_windows_icon_pixels(
    icon: winapi::shared::windef::HICON,
) -> Option<(Vec<u8>, u32, u32)> {
    const DI_NORMAL: u32 = 0x0003;

    use std::mem::zeroed;
    use std::ptr::{copy_nonoverlapping, null_mut, write_bytes};
    use winapi::shared::minwindef::UINT;
    use winapi::shared::windef::HGDIOBJ;
    use winapi::um::wingdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject,
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use winapi::um::winuser::{DrawIconEx, GetDC, GetIconInfo, ReleaseDC, ICONINFO};

    let mut icon_info: ICONINFO = zeroed();
    if GetIconInfo(icon, &mut icon_info) == 0 {
        return None;
    }

    let rendered = (|| {
        let source_bitmap = if !icon_info.hbmColor.is_null() {
            icon_info.hbmColor
        } else {
            icon_info.hbmMask
        };
        if source_bitmap.is_null() {
            return None;
        }

        let mut bitmap: BITMAP = zeroed();
        let get_object_result = GetObjectW(
            source_bitmap as *mut _,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bitmap as *mut _ as *mut _,
        );
        if get_object_result == 0 {
            return None;
        }

        let width = bitmap.bmWidth.abs();
        let mut height = bitmap.bmHeight.abs();
        if icon_info.hbmColor.is_null() {
            height /= 2;
        }
        if width <= 0 || height <= 0 {
            return None;
        }

        let screen_dc = GetDC(null_mut());
        if screen_dc.is_null() {
            return None;
        }

        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_null() {
            ReleaseDC(null_mut(), screen_dc);
            return None;
        }

        let mut bitmap_info: BITMAPINFO = zeroed();
        bitmap_info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bitmap_info.bmiHeader.biWidth = width;
        bitmap_info.bmiHeader.biHeight = -height;
        bitmap_info.bmiHeader.biPlanes = 1;
        bitmap_info.bmiHeader.biBitCount = 32;
        bitmap_info.bmiHeader.biCompression = BI_RGB;

        let mut dib_bits = null_mut();
        let dib = CreateDIBSection(
            screen_dc,
            &bitmap_info,
            DIB_RGB_COLORS as UINT,
            &mut dib_bits,
            null_mut(),
            0,
        );
        if dib.is_null() || dib_bits.is_null() {
            DeleteDC(mem_dc);
            ReleaseDC(null_mut(), screen_dc);
            return None;
        }

        let old_object = SelectObject(mem_dc, dib as HGDIOBJ);
        if old_object.is_null() {
            DeleteObject(dib as HGDIOBJ);
            DeleteDC(mem_dc);
            ReleaseDC(null_mut(), screen_dc);
            return None;
        }

        let pixel_len = width as usize * height as usize * 4;
        write_bytes(dib_bits as *mut u8, 0, pixel_len);

        let draw_result = DrawIconEx(mem_dc, 0, 0, icon, width, height, 0, null_mut(), DI_NORMAL);
        let mut pixels = None;
        if draw_result != 0 {
            let mut buffer = vec![0; pixel_len];
            copy_nonoverlapping(dib_bits as *const u8, buffer.as_mut_ptr(), pixel_len);
            pixels = Some((buffer, width as u32, height as u32));
        }

        SelectObject(mem_dc, old_object);
        DeleteObject(dib as HGDIOBJ);
        DeleteDC(mem_dc);
        ReleaseDC(null_mut(), screen_dc);
        pixels
    })();

    if !icon_info.hbmColor.is_null() {
        DeleteObject(icon_info.hbmColor as HGDIOBJ);
    }
    if !icon_info.hbmMask.is_null() {
        DeleteObject(icon_info.hbmMask as HGDIOBJ);
    }

    rendered
}

#[cfg(target_os = "windows")]
fn encode_windows_icon_base64(mut pixels: Vec<u8>, width: u32, height: u32) -> Option<String> {
    if width == 0 || height == 0 {
        return None;
    }

    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }

    let image = image::RgbaImage::from_raw(width, height, pixels)?;
    let mut dynamic_image = image::DynamicImage::ImageRgba8(image);
    if width > 128 || height > 128 {
        dynamic_image = dynamic_image.resize_exact(128, 128, image::imageops::FilterType::Lanczos3);
    }

    let mut cursor = std::io::Cursor::new(Vec::new());
    dynamic_image
        .write_to(&mut cursor, image::ImageFormat::Png)
        .ok()?;

    Some(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        cursor.into_inner(),
    ))
}

#[cfg(target_os = "windows")]
fn convert_windows_icon_to_base64(icon: winapi::shared::windef::HICON) -> Option<(String, u32)> {
    use winapi::um::winuser::DestroyIcon;

    let rendered = unsafe { render_windows_icon_pixels(icon) };
    unsafe {
        DestroyIcon(icon);
    }

    let (pixels, width, height) = rendered?;
    let encoded = encode_windows_icon_base64(pixels, width, height)?;
    Some((encoded, width.max(height)))
}

#[cfg(target_os = "windows")]
fn extract_windows_icon_base64(path: &str) -> Option<String> {
    use winapi::um::shellapi::{SHIL_EXTRALARGE, SHIL_JUMBO};

    let mut jumbo_fallback = None;

    if let Some(icon) = unsafe { get_windows_icon_from_shell_image_list(path, SHIL_JUMBO as i32) } {
        if let Some((encoded, size)) = convert_windows_icon_to_base64(icon) {
            if size >= 48 {
                return Some(encoded);
            }
            jumbo_fallback = Some(encoded);
        }
    }

    let mut extra_large_fallback = None;
    if let Some(icon) =
        unsafe { get_windows_icon_from_shell_image_list(path, SHIL_EXTRALARGE as i32) }
    {
        if let Some((encoded, size)) = convert_windows_icon_to_base64(icon) {
            if size >= 32 {
                return Some(encoded);
            }
            extra_large_fallback = Some(encoded);
        }
    }

    if let Some(icon) = unsafe { get_windows_associated_icon(path) } {
        if let Some((encoded, _)) = convert_windows_icon_to_base64(icon) {
            return Some(encoded);
        }
    }

    extra_large_fallback.or(jumbo_fallback)
}

#[cfg(target_os = "windows")]
async fn get_app_icon_impl(
    app_name: &str,
    executable_path: Option<&str>,
) -> Result<String, AppError> {
    const WINDOWS_ICON_CACHE_VERSION: &str = "v5";

    // 磁盘缓存：检查是否已有缓存
    let cache_dir = std::env::temp_dir().join("work_review_icons");
    let _ = std::fs::create_dir_all(&cache_dir);
    let cache_key = build_windows_icon_cache_key(
        &crate::monitor::normalize_display_app_name(app_name),
        executable_path,
    );
    let cache_file = cache_dir.join(format!("{cache_key}_{WINDOWS_ICON_CACHE_VERSION}.b64"));

    if cache_file.exists() {
        if let Ok(metadata) = std::fs::metadata(&cache_file) {
            // 缓存有效期 24 小时
            if let Ok(modified) = metadata.modified() {
                if modified.elapsed().unwrap_or_default().as_secs() < 86400 {
                    if let Ok(cached) = std::fs::read_to_string(&cache_file) {
                        if cached.len() > 100 {
                            return Ok(cached);
                        }
                    }
                }
            }
        }
    }

    let icon_lookup_candidates = merge_windows_icon_lookup_candidates(
        executable_path,
        windows_known_icon_paths(app_name)
            .into_iter()
            .filter(|path| std::path::Path::new(path).exists())
            .collect::<Vec<_>>(),
    );
    if icon_lookup_candidates.is_empty() {
        return Ok(String::new());
    }

    // 仅对明确的可执行路径提取图标，不扫描注册表、开始菜单快捷方式或全部运行进程。
    for candidate_path in icon_lookup_candidates {
        if !Path::new(&candidate_path).exists() {
            continue;
        }

        if let Some(base64_str) = extract_windows_icon_base64(&candidate_path) {
            if base64_str.len() > 100 {
                let _ = std::fs::write(&cache_file, &base64_str);
                return Ok(base64_str);
            }
        }
    }

    Ok(String::new())
}

/// 其他平台：返回空字符串
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn get_app_icon_impl(
    _app_name: &str,
    _executable_path: Option<&str>,
) -> Result<String, AppError> {
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_excluded_domains_to_stats, build_daily_report_export_path,
        build_fallback_assistant_answer, build_updater_manifest_candidates,
        build_windows_icon_cache_key, detect_assistant_question_kind,
        detect_assistant_question_kind_with_mode, export_daily_report_markdown,
        format_browser_url_for_display, macos_score_app_bundle_name,
        merge_windows_icon_lookup_candidates, normalize_macos_app_lookup_name,
        normalize_saved_report_ai_mode, ollama_model_names_match, ollama_model_should_be_listed,
        ollama_show_response_supports_completion, openai_compatible_chat_completion_urls,
        openai_connection_test_max_tokens, overview_week_bounds_for_date, parse_ollama_model_names,
        resolve_saved_report_metadata, sum_daily_stats, AppLocale, AssistantChatMessage,
        AssistantQuestionKind, AssistantReasoningMode, UPDATER_JSON_ENDPOINTS,
        UPDATE_CONNECT_TIMEOUT_SECS, UPDATE_REQUEST_TIMEOUT_SECS,
    };
    use crate::config::AiMode;
    use crate::database::{
        AppUsage, BrowserUsage, CategoryUsage, DailyStats, DomainUsage, HourlyActivityBucket,
        MemorySearchItem, UrlDetail, UrlUsage,
    };
    use crate::work_intelligence::{
        IntentAnalysisResult, IntentSummary, NamedDuration, WeeklyReviewResult, WorkSession,
    };
    use chrono::NaiveDate;
    use std::path::{Path, PathBuf};

    fn sample_review() -> WeeklyReviewResult {
        WeeklyReviewResult {
            title: "阶段复盘".to_string(),
            markdown: "本周主要推进了助手回答链路改造。".to_string(),
            total_duration: 6 * 3600,
            active_days: 3,
            session_count: 5,
            deep_work_sessions: 2,
            top_intents: vec![IntentSummary {
                label: "编码开发".to_string(),
                duration: 4 * 3600,
                session_count: 3,
            }],
            top_apps: vec![NamedDuration {
                name: "VS Code".to_string(),
                duration: 4 * 3600,
            }],
            highlights: vec!["完成助手回答链路重构设计".to_string()],
            risks: vec!["追问场景仍依赖关键词".to_string()],
        }
    }

    fn sample_intents() -> IntentAnalysisResult {
        IntentAnalysisResult {
            sessions: vec![],
            summary: vec![IntentSummary {
                label: "编码开发".to_string(),
                duration: 4 * 3600,
                session_count: 3,
            }],
        }
    }

    fn sample_sessions() -> Vec<WorkSession> {
        vec![WorkSession {
            session_id: "2026-03-27-1".to_string(),
            date: "2026-03-27".to_string(),
            start_timestamp: 1_711_500_000,
            end_timestamp: 1_711_503_600,
            duration: 3600,
            activity_count: 4,
            app_count: 2,
            dominant_app: "VS Code".to_string(),
            dominant_category: "development".to_string(),
            title: "重构助手回答链路".to_string(),
            browser_domains: vec!["github.com".to_string()],
            top_apps: vec![NamedDuration {
                name: "VS Code".to_string(),
                duration: 3000,
            }],
            top_keywords: vec!["assistant".to_string()],
            intent_label: "编码开发".to_string(),
            intent_confidence: 88,
            intent_evidence: vec!["修改 commands.rs".to_string()],
        }]
    }

    fn sample_references() -> Vec<MemorySearchItem> {
        vec![MemorySearchItem {
            source_type: "activity".to_string(),
            source_id: Some(1),
            date: "2026-03-27".to_string(),
            timestamp: 1_711_503_600,
            title: "重构助手回答链路".to_string(),
            excerpt: "完成问题分类和回答骨架调整".to_string(),
            app_name: Some("VS Code".to_string()),
            browser_url: None,
            duration: Some(3600),
            score: 120,
        }]
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_permission_settings_url_should_match_expected_panels() {
        assert_eq!(
            super::macos_permission_settings_url("screen_capture"),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        );
        assert_eq!(
            super::macos_permission_settings_url("accessibility"),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        );
        assert_eq!(
            super::macos_permission_settings_url("input_monitoring"),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        );
        assert_eq!(super::macos_permission_settings_url("unknown"), None);
    }

    #[test]
    fn 应将命令输出中的_url_格式化为可读文本() {
        assert_eq!(
            format_browser_url_for_display(
                "https://www.google.com.hk/search?q=%E5%A4%A7%E6%B8%A1%E5%8F%A3&client=firefox-b-d"
            ),
            "https://www.google.com.hk/search?q=大渡口&client=firefox-b-d"
        );
        assert_eq!(
            format_browser_url_for_display(
                "https://example.com/search?q=a%26b&name=%E5%BC%A0%E4%B8%89"
            ),
            "https://example.com/search?q=a%26b&name=张三"
        );
    }

    #[test]
    fn openai兼容探测请求的输出上限不应低于十六() {
        assert_eq!(openai_connection_test_max_tokens(), 16);
    }

    #[test]
    fn openai兼容端点应自动补齐_chat_completions_并支持_v1_回退() {
        assert_eq!(
            openai_compatible_chat_completion_urls("https://api.deepseek.com"),
            vec![
                "https://api.deepseek.com/chat/completions".to_string(),
                "https://api.deepseek.com/v1/chat/completions".to_string()
            ]
        );
        assert_eq!(
            openai_compatible_chat_completion_urls("https://api.openai.com/v1"),
            vec!["https://api.openai.com/v1/chat/completions".to_string()]
        );
    }

    #[test]
    fn 概览本周范围应从周一开始到锚点日期结束() {
        let anchor = NaiveDate::from_ymd_opt(2026, 4, 1).expect("valid anchor date");

        let (date_from, date_to) = overview_week_bounds_for_date(anchor);

        assert_eq!(date_from, "2026-03-30");
        assert_eq!(date_to, "2026-04-01");
    }

    #[test]
    fn 周概览统计应合并重复应用浏览器与小时分布() {
        let hourly = |hour, duration| HourlyActivityBucket { hour, duration };
        let day_one = DailyStats {
            total_duration: 120,
            screenshot_count: 2,
            app_usage: vec![AppUsage {
                app_name: "Cursor".to_string(),
                duration: 120,
                count: 2,
                executable_path: Some("/Applications/Cursor.app".to_string()),
                screenshot_url: Some("https://cdn.example.com/cursor-day-one.jpg".to_string()),
            }],
            category_usage: vec![CategoryUsage {
                category: "development".to_string(),
                duration: 120,
            }],
            browser_duration: 60,
            url_usage: vec![UrlUsage {
                url: "https://docs.example.com/a".to_string(),
                domain: "docs.example.com".to_string(),
                duration: 60,
            }],
            domain_usage: vec![DomainUsage {
                domain: "docs.example.com".to_string(),
                duration: 60,
                semantic_category: Some("资料阅读".to_string()),
                urls: vec![UrlDetail {
                    url: "https://docs.example.com/a".to_string(),
                    duration: 60,
                }],
            }],
            browser_usage: vec![BrowserUsage {
                browser_name: "Google Chrome".to_string(),
                duration: 60,
                executable_path: Some("/Applications/Google Chrome.app".to_string()),
                domains: vec![DomainUsage {
                    domain: "docs.example.com".to_string(),
                    duration: 60,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://docs.example.com/a".to_string(),
                        duration: 60,
                    }],
                }],
            }],
            work_time_duration: 100,
            hourly_activity_distribution: vec![hourly(9, 60), hourly(10, 60)],
        };
        let day_two = DailyStats {
            total_duration: 180,
            screenshot_count: 3,
            app_usage: vec![
                AppUsage {
                    app_name: "Cursor".to_string(),
                    duration: 120,
                    count: 1,
                    executable_path: Some("/Applications/Cursor.app".to_string()),
                    screenshot_url: None,
                },
                AppUsage {
                    app_name: "Google Chrome".to_string(),
                    duration: 60,
                    count: 1,
                    executable_path: Some("/Applications/Google Chrome.app".to_string()),
                    screenshot_url: Some("https://cdn.example.com/chrome-day-two.jpg".to_string()),
                },
            ],
            category_usage: vec![
                CategoryUsage {
                    category: "development".to_string(),
                    duration: 120,
                },
                CategoryUsage {
                    category: "browser".to_string(),
                    duration: 60,
                },
            ],
            browser_duration: 120,
            url_usage: vec![
                UrlUsage {
                    url: "https://docs.example.com/a".to_string(),
                    domain: "docs.example.com".to_string(),
                    duration: 30,
                },
                UrlUsage {
                    url: "https://news.example.com/b".to_string(),
                    domain: "news.example.com".to_string(),
                    duration: 90,
                },
            ],
            domain_usage: vec![
                DomainUsage {
                    domain: "docs.example.com".to_string(),
                    duration: 30,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://docs.example.com/a".to_string(),
                        duration: 30,
                    }],
                },
                DomainUsage {
                    domain: "news.example.com".to_string(),
                    duration: 90,
                    semantic_category: Some("资料调研".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://news.example.com/b".to_string(),
                        duration: 90,
                    }],
                },
            ],
            browser_usage: vec![BrowserUsage {
                browser_name: "Google Chrome".to_string(),
                duration: 120,
                executable_path: Some("/Applications/Google Chrome.app".to_string()),
                domains: vec![
                    DomainUsage {
                        domain: "docs.example.com".to_string(),
                        duration: 30,
                        semantic_category: Some("资料阅读".to_string()),
                        urls: vec![UrlDetail {
                            url: "https://docs.example.com/a".to_string(),
                            duration: 30,
                        }],
                    },
                    DomainUsage {
                        domain: "news.example.com".to_string(),
                        duration: 90,
                        semantic_category: Some("资料调研".to_string()),
                        urls: vec![UrlDetail {
                            url: "https://news.example.com/b".to_string(),
                            duration: 90,
                        }],
                    },
                ],
            }],
            work_time_duration: 160,
            hourly_activity_distribution: vec![hourly(9, 30), hourly(10, 90), hourly(11, 60)],
        };

        let merged = sum_daily_stats(vec![day_one, day_two]);

        assert_eq!(merged.total_duration, 300);
        assert_eq!(merged.screenshot_count, 5);
        assert_eq!(merged.work_time_duration, 260);
        assert_eq!(merged.browser_duration, 180);
        assert_eq!(merged.app_usage.len(), 2);
        assert_eq!(merged.app_usage[0].app_name, "Cursor");
        assert_eq!(merged.app_usage[0].duration, 240);
        assert_eq!(merged.app_usage[0].count, 3);
        assert_eq!(merged.hourly_activity_distribution[9].duration, 90);
        assert_eq!(merged.hourly_activity_distribution[10].duration, 150);
        assert_eq!(merged.hourly_activity_distribution[11].duration, 60);
        assert_eq!(merged.browser_usage.len(), 1);
        assert_eq!(merged.browser_usage[0].duration, 180);
        assert_eq!(merged.browser_usage[0].domains.len(), 2);
        assert_eq!(merged.domain_usage[0].domain, "docs.example.com");
        assert_eq!(merged.domain_usage[0].duration, 90);
        assert_eq!(merged.url_usage[0].url, "https://news.example.com/b");
        assert_eq!(merged.url_usage[0].duration, 90);
    }

    #[test]
    fn 概览统计应过滤排除域名并重算浏览器时长() {
        let stats = DailyStats {
            total_duration: 360,
            screenshot_count: 6,
            app_usage: vec![],
            category_usage: vec![],
            browser_duration: 210,
            url_usage: vec![
                UrlUsage {
                    url: "https://linux.do/latest".to_string(),
                    domain: "linux.do".to_string(),
                    duration: 120,
                },
                UrlUsage {
                    url: "linux.dolatest".to_string(),
                    domain: "linux.dolatest".to_string(),
                    duration: 30,
                },
                UrlUsage {
                    url: "https://github.com/issues".to_string(),
                    domain: "github.com".to_string(),
                    duration: 60,
                },
            ],
            domain_usage: vec![
                DomainUsage {
                    domain: "linux.do".to_string(),
                    duration: 120,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://linux.do/latest".to_string(),
                        duration: 120,
                    }],
                },
                DomainUsage {
                    domain: "linux.dolatest".to_string(),
                    duration: 30,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: vec![UrlDetail {
                        url: "linux.dolatest".to_string(),
                        duration: 30,
                    }],
                },
                DomainUsage {
                    domain: "github.com".to_string(),
                    duration: 60,
                    semantic_category: Some("编码开发".to_string()),
                    urls: vec![UrlDetail {
                        url: "https://github.com/issues".to_string(),
                        duration: 60,
                    }],
                },
            ],
            browser_usage: vec![BrowserUsage {
                browser_name: "Google Chrome".to_string(),
                duration: 210,
                executable_path: Some("/Applications/Google Chrome.app".to_string()),
                domains: vec![
                    DomainUsage {
                        domain: "linux.do".to_string(),
                        duration: 120,
                        semantic_category: Some("资料阅读".to_string()),
                        urls: vec![UrlDetail {
                            url: "https://linux.do/latest".to_string(),
                            duration: 120,
                        }],
                    },
                    DomainUsage {
                        domain: "linux.dolatest".to_string(),
                        duration: 30,
                        semantic_category: Some("资料阅读".to_string()),
                        urls: vec![UrlDetail {
                            url: "linux.dolatest".to_string(),
                            duration: 30,
                        }],
                    },
                    DomainUsage {
                        domain: "github.com".to_string(),
                        duration: 60,
                        semantic_category: Some("编码开发".to_string()),
                        urls: vec![UrlDetail {
                            url: "https://github.com/issues".to_string(),
                            duration: 60,
                        }],
                    },
                ],
            }],
            work_time_duration: 0,
            hourly_activity_distribution: vec![],
        };

        let filtered = apply_excluded_domains_to_stats(stats, &["linux.do".to_string()]);

        assert_eq!(filtered.browser_duration, 60);
        assert_eq!(filtered.browser_usage.len(), 1);
        assert_eq!(filtered.browser_usage[0].domains.len(), 1);
        assert_eq!(filtered.browser_usage[0].domains[0].domain, "github.com");
        assert_eq!(filtered.url_usage.len(), 1);
        assert_eq!(filtered.url_usage[0].domain, "github.com");
        assert_eq!(filtered.domain_usage.len(), 1);
        assert_eq!(filtered.domain_usage[0].domain, "github.com");
    }

    fn sample_noisy_references() -> Vec<MemorySearchItem> {
        vec![
            MemorySearchItem {
                source_type: "activity".to_string(),
                source_id: Some(2),
                date: "2026-03-27".to_string(),
                timestamp: 1_711_504_200,
                title: "../Pycharm_Project/Work_Review/src-tauri".to_string(),
                excerpt: "编辑 显示 通知 窗口 帮助 Work Review 记录 分析 证明 记录状态 暂停 时间线 日报".to_string(),
                app_name: Some("cmux".to_string()),
                browser_url: None,
                duration: Some(1800),
                score: 100,
            },
            MemorySearchItem {
                source_type: "activity".to_string(),
                source_id: Some(3),
                date: "2026-03-27".to_string(),
                timestamp: 1_711_504_500,
                title: "无标题 - Google Chrome - momoi".to_string(),
                excerpt: "Chrome 文件 编辑 显示 历史记录 书签 个人资料 标签页 窗口 帮助 Work Review 记录 分析".to_string(),
                app_name: Some("Google Chrome".to_string()),
                browser_url: None,
                duration: Some(900),
                score: 96,
            },
        ]
    }

    fn sample_process_follow_up_history() -> Vec<AssistantChatMessage> {
        vec![
            AssistantChatMessage {
                role: "user".to_string(),
                content: "最近时间主要花在哪？".to_string(),
            },
            AssistantChatMessage {
                role: "assistant".to_string(),
                content: "## 结论\n\n- 这段时间更像是围绕少数主题持续推进。\n\n## 过程分析\n\n- 主要是编码开发相关 session。\n".to_string(),
            },
        ]
    }

    fn sample_stage_follow_up_history() -> Vec<AssistantChatMessage> {
        vec![
            AssistantChatMessage {
                role: "user".to_string(),
                content: "这周主要做了什么？".to_string(),
            },
            AssistantChatMessage {
                role: "assistant".to_string(),
                content: "## 结论\n\n- 这周主线是助手回答链路改造。\n".to_string(),
            },
        ]
    }

    #[test]
    fn windows图标候选应优先真实路径并去重() {
        let candidates = merge_windows_icon_lookup_candidates(
            Some(r"D:\Portable\Code\Code.exe"),
            vec![
                r"C:\Program Files\Microsoft VS Code\Code.exe".to_string(),
                r"D:\Portable\Code\Code.exe".to_string(),
                r"C:\Program Files\Microsoft VS Code\Code.exe".to_string(),
            ],
        );

        assert_eq!(
            candidates,
            vec![
                r"D:\Portable\Code\Code.exe".to_string(),
                r"C:\Program Files\Microsoft VS Code\Code.exe".to_string(),
            ]
        );
    }

    #[test]
    fn windows图标缓存key应包含真实路径特征() {
        let portable_key =
            build_windows_icon_cache_key("VS Code", Some(r"D:\Portable\Code\Code.exe"));
        let installed_key = build_windows_icon_cache_key(
            "VS Code",
            Some(r"C:\Program Files\Microsoft VS Code\Code.exe"),
        );

        assert_ne!(portable_key, installed_key);
        assert!(portable_key.starts_with("VS_Code_"));
        assert!(installed_key.starts_with("VS_Code_"));
    }

    #[test]
    fn macos图标名称归一化应兼容分隔符与后缀() {
        assert_eq!(
            normalize_macos_app_lookup_name("Zen Browser"),
            "zen browser"
        );
        assert_eq!(
            normalize_macos_app_lookup_name("antigravity_tools.app"),
            "antigravity tools"
        );
        assert_eq!(
            normalize_macos_app_lookup_name("Antigravity-Tools"),
            "antigravity tools"
        );
    }

    #[test]
    fn macos应用包名评分应兼容缩写与分隔符差异() {
        assert!(
            macos_score_app_bundle_name("Foo Browser", "Foo")
                > macos_score_app_bundle_name("Foo Browser", "Bar")
        );
        assert!(
            macos_score_app_bundle_name("antigravity_tools", "Antigravity")
                > macos_score_app_bundle_name("antigravity_tools", "Calculator")
        );
        assert!(
            macos_score_app_bundle_name("antigravity_tools", "Antigravity Tools")
                >= macos_score_app_bundle_name("antigravity_tools", "Antigravity")
        );
    }

    #[test]
    fn macos应用包名评分应兼容中文显示名与英文包名别名() {
        assert!(macos_score_app_bundle_name("腾讯视频", "QQLive") > 0);
        assert!(
            macos_score_app_bundle_name("腾讯视频", "QQLive")
                > macos_score_app_bundle_name("腾讯视频", "QQ")
        );
    }

    #[test]
    fn 更新清单候选应优先显式版本地址再回退latest地址() {
        let candidates = build_updater_manifest_candidates(
            "https://github.com/wm94i/Work_Review/releases/latest/download/updater.json",
            Some("1.0.24"),
        );

        assert_eq!(
            candidates,
            vec![
                "https://github.com/wm94i/Work_Review/releases/download/v1.0.24/updater.json"
                    .to_string(),
                "https://github.com/wm94i/Work_Review/releases/latest/download/updater.json"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn summary回退到基础模板时不应保留_ai_模型标签() {
        let (ai_mode, model_name) =
            resolve_saved_report_metadata(&AiMode::Summary, "gpt-5.4", false);

        assert_eq!(ai_mode, "local");
        assert_eq!(model_name, None);
    }

    #[test]
    fn ai成功生成时应保留实际配置的模式与模型() {
        let (ai_mode, model_name) =
            resolve_saved_report_metadata(&AiMode::Summary, "gpt-5.4", true);

        assert_eq!(ai_mode, "summary");
        assert_eq!(model_name, Some("gpt-5.4".to_string()));
    }

    #[test]
    fn 保存的日报模式应统一转为小写() {
        assert_eq!(normalize_saved_report_ai_mode("Summary"), "summary");
        assert_eq!(normalize_saved_report_ai_mode(" local "), "local");
    }

    #[test]
    fn 日报导出路径应按日期生成_markdown_文件名() {
        let export_path = build_daily_report_export_path(Path::new("/tmp/reports"), "2026-03-29");

        assert_eq!(
            export_path,
            PathBuf::from("/tmp/reports").join("2026-03-29.md")
        );
    }

    #[test]
    fn 日报导出应写入_markdown_文件() {
        let temp_dir =
            std::env::temp_dir().join(format!("work-review-export-{}", uuid::Uuid::new_v4()));
        export_daily_report_markdown(&temp_dir, "2026-03-29", "# 工作日报\n\n测试内容")
            .expect("应能导出 Markdown");

        let output_path = temp_dir.join("2026-03-29.md");
        let content = std::fs::read_to_string(&output_path).expect("应能读取导出内容");
        assert_eq!(content, "# 工作日报\n\n测试内容");

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn 应能解析_ollama_模型列表响应() {
        let payload = serde_json::json!({
            "models": [
                { "name": "qwen2.5:latest" },
                { "name": "llama3.1:8b" },
                { "name": "qwen2.5:latest" }
            ]
        });

        let names = parse_ollama_model_names(&payload).expect("应能解析模型列表");

        assert_eq!(
            names,
            vec!["llama3.1:8b".to_string(), "qwen2.5:latest".to_string()]
        );
    }

    #[test]
    fn 解析_ollama_模型列表时应过滤嵌入模型() {
        let payload = serde_json::json!({
            "models": [
                { "name": "qwen3.5:4b" },
                { "name": "nomic-embed-text:latest" },
                { "name": "llama3.2:latest" }
            ]
        });

        let names = parse_ollama_model_names(&payload).expect("应能解析模型列表");

        assert_eq!(
            names,
            vec!["llama3.2:latest".to_string(), "qwen3.5:4b".to_string()]
        );
    }

    #[test]
    fn ollama_show_响应应根据能力判断是否支持文本生成() {
        let embedding_only = serde_json::json!({
            "capabilities": ["embedding"]
        });
        let completion_and_vision = serde_json::json!({
            "capabilities": ["completion", "vision"]
        });
        let missing_capabilities = serde_json::json!({});

        assert_eq!(
            ollama_show_response_supports_completion(&embedding_only),
            Some(false)
        );
        assert_eq!(
            ollama_show_response_supports_completion(&completion_and_vision),
            Some(true)
        );
        assert_eq!(
            ollama_show_response_supports_completion(&missing_capabilities),
            None
        );
    }

    #[test]
    fn ollama_模型名匹配应兼容_latest_缩写且避免宽松子串误判() {
        assert!(ollama_model_names_match("qwen2.5", "qwen2.5:latest"));
        assert!(ollama_model_names_match(
            "hf.co/Qwen/Qwen3-8B-GGUF:Q5_K_M",
            "hf.co/Qwen/Qwen3-8B-GGUF:Q5_K_M"
        ));
        assert!(!ollama_model_names_match("qwen2.5", "qwen2.5-coder:latest"));
        assert!(!ollama_model_names_match("qwen2.5", "deepseek-r1:1.5b"));
    }

    #[test]
    fn ollama_模型展示应优先相信能力信息再回退名称规则() {
        let suspicious_but_completion = serde_json::json!({
            "name": "embed-chat-preview:latest"
        });
        let completion_show = serde_json::json!({
            "capabilities": ["completion"]
        });

        let embedding_only = serde_json::json!({
            "name": "all-minilm:latest"
        });
        let embedding_show = serde_json::json!({
            "capabilities": ["embedding"]
        });

        let heuristic_only = serde_json::json!({
            "name": "nomic-embed-text:latest"
        });

        assert!(ollama_model_should_be_listed(
            &suspicious_but_completion,
            Some(&completion_show)
        ));
        assert!(!ollama_model_should_be_listed(
            &embedding_only,
            Some(&embedding_show)
        ));
        assert!(!ollama_model_should_be_listed(&heuristic_only, None));
    }
    #[test]
    fn 助手问题分类应识别阶段总结与过程复盘和证据追问() {
        assert_eq!(
            detect_assistant_question_kind("这周主要做了什么？", &[]),
            AssistantQuestionKind::StageSummary
        );
        assert_eq!(
            detect_assistant_question_kind("最近时间主要花在哪？", &[]),
            AssistantQuestionKind::ProcessRecap
        );
        assert_eq!(
            detect_assistant_question_kind("这个结论的依据是什么？", &[]),
            AssistantQuestionKind::EvidenceQuery
        );
    }

    #[test]
    fn 更新清单候选应保留代理前缀并规范化版本号() {
        let candidates = build_updater_manifest_candidates(
            "https://ghproxy.cn/https://github.com/wm94i/Work_Review/releases/latest/download/updater-ghproxy.json",
            Some("v1.0.24"),
        );

        assert_eq!(
            candidates,
            vec![
                "https://ghproxy.cn/https://github.com/wm94i/Work_Review/releases/download/v1.0.24/updater-ghproxy.json"
                    .to_string(),
                "https://ghproxy.cn/https://github.com/wm94i/Work_Review/releases/latest/download/updater-ghproxy.json"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn 更新源应优先官方_github_并放宽超时() {
        assert_eq!(
            UPDATER_JSON_ENDPOINTS.first().copied(),
            Some("https://github.com/wm94i/Work_Review/releases/latest/download/updater.json")
        );
        assert!(UPDATE_REQUEST_TIMEOUT_SECS >= 30);
        assert!(UPDATE_CONNECT_TIMEOUT_SECS >= 10);
    }

    #[test]
    fn 助手问题分类应继承上一轮过程复盘语境() {
        let history = sample_process_follow_up_history();

        assert_eq!(
            detect_assistant_question_kind("继续", &history),
            AssistantQuestionKind::ProcessRecap
        );
        assert_eq!(
            detect_assistant_question_kind("展开说说这个", &history),
            AssistantQuestionKind::ProcessRecap
        );
    }

    #[test]
    fn 助手问题分类应将依据追问优先识别为证据问题() {
        let history = sample_stage_follow_up_history();

        assert_eq!(
            detect_assistant_question_kind("那依据呢", &history),
            AssistantQuestionKind::EvidenceQuery
        );
        assert_eq!(
            detect_assistant_question_kind("这个结论怎么得出的", &history),
            AssistantQuestionKind::EvidenceQuery
        );
    }

    #[test]
    fn ai增强识别器应比基础模板更强承接助手上下文() {
        let history = vec![
            AssistantChatMessage {
                role: "user".to_string(),
                content: "这周主要做了什么？".to_string(),
            },
            AssistantChatMessage {
                role: "assistant".to_string(),
                content: "## 结论\n\n- 这周主线是助手回答链路改造。\n\n## 过程分析\n\n- 主要是编码开发相关 session。\n".to_string(),
            },
        ];

        assert_eq!(
            detect_assistant_question_kind_with_mode(
                "展开说说这个",
                &history,
                AssistantReasoningMode::Basic
            ),
            AssistantQuestionKind::StageSummary
        );
        assert_eq!(
            detect_assistant_question_kind_with_mode(
                "展开说说这个",
                &history,
                AssistantReasoningMode::AiEnhanced
            ),
            AssistantQuestionKind::ProcessRecap
        );
    }

    #[test]
    fn 复盘型基础回答应输出统一章节骨架() {
        let answer = build_fallback_assistant_answer(
            "这周主要做了什么？",
            AssistantQuestionKind::StageSummary,
            &sample_references(),
            Some(&sample_sessions()),
            Some(&sample_intents()),
            Some(&sample_review()),
            None,
            &["周报复盘".to_string()],
            AppLocale::ZhCn,
        );

        assert!(answer.contains("## 结论"));
        assert!(answer.contains("## 时间分布"));
        assert!(answer.contains("## 工作段"));
        assert!(answer.contains("可以继续追问"));
    }

    #[test]
    fn 低信噪比原始记录不应直接出现在依据补充中() {
        let answer = build_fallback_assistant_answer(
            "这周主要做了什么？",
            AssistantQuestionKind::StageSummary,
            &sample_noisy_references(),
            Some(&sample_sessions()),
            Some(&sample_intents()),
            Some(&sample_review()),
            None,
            &["周报复盘".to_string()],
            AppLocale::ZhCn,
        );

        assert!(!answer.contains("../Pycharm_Project/Work_Review/src-tauri"));
        assert!(!answer.contains("无标题 - Google Chrome - momoi"));
    }
}

/// 保存背景图片（接收 base64 编码的图片数据）
#[tauri::command]
pub async fn save_background_image(
    data: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let (data_dir, config_path) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (s.data_dir.clone(), s.config_path.clone())
    };

    // 解码 base64
    let image_bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data)
        .map_err(|e| AppError::Unknown(format!("base64 解码失败: {e}")))?;

    // 保存为 JPEG（压缩体积）
    let img = image::load_from_memory(&image_bytes)
        .map_err(|e| AppError::Unknown(format!("图片解析失败: {e}")))?;

    // 限制最大尺寸为 1920px 宽
    let img = if img.width() > 1920 {
        img.resize(1920, 1920, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let bg_path = data_dir.join("background.jpg");
    img.save_with_format(&bg_path, image::ImageFormat::Jpeg)
        .map_err(|e| AppError::Unknown(format!("保存背景图失败: {e}")))?;

    // 更新配置
    let mut s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    s.config.background_image = Some("background.jpg".to_string());
    s.config.save(&config_path)?;

    Ok(())
}

/// 获取背景图片（返回 base64）
#[tauri::command]
pub async fn get_background_image(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<String>, AppError> {
    let (data_dir, bg_filename) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (s.data_dir.clone(), s.config.background_image.clone())
    };

    let filename = match bg_filename {
        Some(f) if !f.is_empty() => f,
        _ => return Ok(None),
    };

    let bg_path = data_dir.join(&filename);
    if !bg_path.exists() {
        return Ok(None);
    }

    let bytes =
        std::fs::read(&bg_path).map_err(|e| AppError::Unknown(format!("读取背景图失败: {e}")))?;
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    Ok(Some(b64))
}

/// 清除背景图片
#[tauri::command]
pub async fn clear_background_image(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), AppError> {
    let (data_dir, config_path) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (s.data_dir.clone(), s.config_path.clone())
    };

    // 删除文件
    let bg_path = data_dir.join("background.jpg");
    if bg_path.exists() {
        let _ = std::fs::remove_file(&bg_path);
    }

    // 更新配置
    let mut s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    s.config.background_image = None;
    s.config.save(&config_path)?;

    Ok(())
}
