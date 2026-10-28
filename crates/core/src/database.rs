#![allow(clippy::type_complexity)] // SQL 投影元组与查询列一一对应。

use crate::error::{AppError, Result};
use chrono::{Local, MappedLocalTime, NaiveDateTime, TimeZone};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// 安全地将 NaiveDateTime 转换为本地时间戳
/// 在 DST 跳变时不会 panic：
/// - Ambiguous（秋季回拨）：取较早的时间
/// - None（春季前跳）：向前偏移1小时后重试
fn safe_local_timestamp(ndt: NaiveDateTime) -> i64 {
    match Local.from_local_datetime(&ndt) {
        MappedLocalTime::Single(dt) => dt.timestamp(),
        MappedLocalTime::Ambiguous(dt, _) => dt.timestamp(),
        MappedLocalTime::None => {
            // DST 跳变导致该本地时间不存在，向前偏移1小时
            let shifted = ndt + chrono::Duration::hours(1);
            Local
                .from_local_datetime(&shifted)
                .earliest()
                .map(|dt| dt.timestamp())
                .unwrap_or_else(|| ndt.and_utc().timestamp())
        }
    }
}

fn local_day_bounds(date: chrono::NaiveDate) -> Result<(i64, i64)> {
    let next_date = date
        .succ_opt()
        .ok_or_else(|| AppError::Config("日期超出可统计范围".to_string()))?;
    let start = safe_local_timestamp(date.and_hms_opt(0, 0, 0).unwrap());
    let end = safe_local_timestamp(next_date.and_hms_opt(0, 0, 0).unwrap());
    Ok((start, end))
}

fn local_hour_bounds(date: chrono::NaiveDate, hour: u32) -> Result<(i64, i64)> {
    let hour = hour.min(23);
    let start = safe_local_timestamp(date.and_hms_opt(hour, 0, 0).unwrap());
    let end_naive = if hour == 23 {
        date.succ_opt().and_then(|next| next.and_hms_opt(0, 0, 0))
    } else {
        date.and_hms_opt(hour + 1, 0, 0)
    }
    .ok_or_else(|| AppError::Config("小时超出可统计范围".to_string()))?;
    Ok((start, safe_local_timestamp(end_naive)))
}

pub fn local_hour_end_timestamp(date: &str, hour: i32) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    local_hour_bounds(date, hour.clamp(0, 23) as u32)
        .ok()
        .map(|(_, end)| end)
}

fn category_counts_toward_work_time(category: &str) -> bool {
    crate::categorize::normalize_category_key(category) != "entertainment"
}

fn matches_ignored_app_for_stats(app_name: &str, ignored_apps: &[String]) -> bool {
    let app_lower = crate::categorize::normalize_display_app_name(app_name)
        .to_lowercase()
        .trim()
        .to_string();
    if app_lower.is_empty() {
        return false;
    }

    // 单向小写子串匹配，与 privacy::matches_ignored_app 保持一致：
    // 不做反向包含，避免忽略 "Google Chrome Canary" 时误伤 "Google Chrome"。
    ignored_apps
        .iter()
        .any(|ignored| !ignored.is_empty() && app_lower.contains(ignored))
}

fn merged_domain_matches_excluded(domain: &str, excluded_domain: &str) -> bool {
    if !crate::categorize::is_merged_domain(domain) {
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

fn matches_excluded_domain_for_stats(target: &str, excluded_domains: &[String]) -> bool {
    let domain = crate::config::PrivacyConfig::extract_domain(target);
    if domain.is_empty() {
        return false;
    }

    excluded_domains.iter().any(|excluded| {
        let excluded_domain = crate::config::PrivacyConfig::extract_domain(excluded);
        !excluded_domain.is_empty()
            && (crate::config::PrivacyConfig::domain_matches(&domain, &excluded_domain)
                || merged_domain_matches_excluded(&domain, &excluded_domain))
    })
}

const UNRESOLVED_BROWSER_DOMAIN_LABEL: &str = "未识别页面";
const UNRESOLVED_BROWSER_URL_LABEL: &str = "未识别 URL";

const ACTIVITY_SELECT_COLUMNS: &str = "id, timestamp, app_name, window_title, screenshot_path, ocr_text, category, duration, browser_url, executable_path, semantic_category, semantic_confidence, screenshot_url";

fn activity_from_row(row: &Row<'_>) -> rusqlite::Result<Activity> {
    Ok(Activity {
        id: Some(row.get(0)?),
        timestamp: row.get(1)?,
        app_name: row.get(2)?,
        window_title: row.get(3)?,
        screenshot_path: row.get(4)?,
        ocr_text: row.get(5)?,
        category: row.get(6)?,
        duration: row.get(7)?,
        browser_url: row.get(8)?,
        executable_path: row.get(9)?,
        semantic_category: row.get(10)?,
        semantic_confidence: row.get(11)?,
        screenshot_url: row.get(12)?,
    })
}

/// 活动记录
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Activity {
    pub id: Option<i64>,
    pub timestamp: i64,
    pub app_name: String,
    pub window_title: String,
    pub screenshot_path: String,
    pub ocr_text: Option<String>,
    pub category: String,
    pub duration: i64,
    /// 浏览器 URL（如果当前是浏览器应用）
    #[serde(default)]
    pub browser_url: Option<String>,
    /// 可执行文件路径（主要用于 Windows 图标读取）
    #[serde(default)]
    pub executable_path: Option<String>,
    /// 中文语义分类
    #[serde(default)]
    pub semantic_category: Option<String>,
    /// 语义分类置信度（0-100）
    #[serde(default)]
    pub semantic_confidence: Option<i32>,
    /// 远程截图 URL（上传成功后填充）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_url: Option<String>,
}

/// 每日报告
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DailyReport {
    pub date: String,
    #[serde(default = "default_report_locale")]
    pub locale: String,
    pub content: String,
    pub ai_mode: String,
    pub model_name: Option<String>,
    #[serde(default)]
    pub fallback_reason: Option<String>,
    pub created_at: i64,
}

fn default_report_locale() -> String {
    "zh-CN".to_string()
}

/// 应用使用统计
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppUsage {
    pub app_name: String,
    pub duration: i64,
    pub count: i64,
    #[serde(default)]
    pub executable_path: Option<String>,
    /// 该应用最近一次远程截图 URL（上传成功后填充）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppCategorySnapshot {
    pub app_name: String,
    pub category: String,
    pub total_duration: i64,
    pub count: i64,
    pub executable_path: Option<String>,
    pub latest_timestamp: i64,
    pub screenshot_url: Option<String>,
}

/// 分类使用统计
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CategoryUsage {
    pub category: String,
    pub duration: i64,
}

/// 按小时活跃度统计
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct HourlyActivityBucket {
    pub hour: i32,
    pub duration: i64,
}

/// 每小时×应用的时长分布
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HourlyAppBucket {
    pub hour: i32,
    pub total_duration: i64,
    pub apps: Vec<AppDuration>,
}

/// 单个应用的时长
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppDuration {
    pub app_name: String,
    pub duration: i64,
    /// 应用分类 key（development/browser/communication...），用于按分类着色
    #[serde(default)]
    pub category: String,
    /// 该小时内该应用最近一次远程截图 URL（上传成功后填充）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_url: Option<String>,
}

/// 小时摘要
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HourlySummary {
    pub id: Option<i64>,
    /// 日期 YYYY-MM-DD
    pub date: String,
    /// 小时 (0-23)
    pub hour: i32,
    /// AI 生成的摘要内容
    pub summary: String,
    /// 该小时的主要应用
    pub main_apps: String,
    /// 该小时的活动数量
    pub activity_count: i32,
    /// 该小时的总时长（秒）
    pub total_duration: i64,
    /// 代表性截图路径列表（JSON数组）
    pub representative_screenshots: Option<String>,
    /// 创建时间
    pub created_at: i64,
}

/// 每日统计
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DailyStats {
    pub total_duration: i64,
    pub screenshot_count: i64,
    pub app_usage: Vec<AppUsage>,
    pub category_usage: Vec<CategoryUsage>,
    pub browser_duration: i64,
    pub url_usage: Vec<UrlUsage>,
    pub domain_usage: Vec<DomainUsage>,
    /// 隐私过滤后的真实唯一域名总数；首页可仅展示其中一部分。
    #[serde(default)]
    pub domain_total_count: usize,
    /// 按浏览器分组的使用统计
    pub browser_usage: Vec<BrowserUsage>,
    /// 工作时间内的活动时长（新增）
    #[serde(default)]
    pub work_time_duration: i64,
    /// 加班时长（秒）：超出标准工时的工作时长
    #[serde(default)]
    pub overtime_duration: i64,
    /// 24 小时活跃度分布
    #[serde(default)]
    pub hourly_activity_distribution: Vec<HourlyActivityBucket>,
}

pub fn apply_flex_overtime_correction(
    stats: &mut DailyStats,
    work_time_enabled: bool,
    standard_work_hours: f64,
) {
    if !work_time_enabled {
        stats.work_time_duration = stats
            .category_usage
            .iter()
            .filter(|item| category_counts_toward_work_time(&item.category))
            .map(|item| item.duration)
            .sum();
        let standard_seconds = (standard_work_hours * 3600.0).round() as i64;
        stats.overtime_duration = (stats.work_time_duration - standard_seconds).max(0);
    }
}

/// 域名使用统计（按域名分组）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DomainUsage {
    pub domain: String,
    pub duration: i64,
    #[serde(default)]
    pub semantic_category: Option<String>,
    pub urls: Vec<UrlDetail>,
}

/// URL 详情
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UrlDetail {
    pub url: String,
    pub duration: i64,
}

/// 浏览器使用统计（按浏览器应用分组）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BrowserUsage {
    /// 浏览器名称（如 Chrome, Safari, Arc 等）
    pub browser_name: String,
    /// 总使用时长
    pub duration: i64,
    #[serde(default)]
    pub executable_path: Option<String>,
    /// 该浏览器下访问的域名列表
    pub domains: Vec<DomainUsage>,
}

/// URL 使用统计（保留兼容）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UrlUsage {
    pub url: String,
    pub domain: String,
    pub duration: i64,
}

/// AI 工作记忆洞察（自进化）
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WorkInsight {
    pub id: i64,
    pub insight_type: String,
    pub content: String,
    pub confidence: f64,
    pub source_date: String,
    pub created_at: i64,
    pub confirmed_count: i32,
    pub denied_count: i32,
    pub archived: bool,
}

/// 语义记忆检索命中项（屏幕级数字记忆）
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SemanticMemoryHit {
    pub chunk_id: i64,
    pub date: String,
    pub app_name: String,
    pub title: String,
    pub browser_url: Option<String>,
    pub excerpt: String,
    /// 归一化余弦相似度（0~1，越大越相关）
    pub score: f64,
}

/// 语义记忆索引状态
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SemanticMemoryStats {
    pub total_chunks: usize,
    pub embedded_chunks: usize,
    /// 已消费到的活动记录 id 游标（增量索引起点）
    pub last_indexed_activity_id: i64,
}

/// 当前重建代际中等待发送到嵌入服务的语义块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSemanticMemoryChunk {
    pub id: i64,
    pub content: String,
    pub app_name: String,
    pub title: String,
    pub browser_url: Option<String>,
    pub last_activity_id: i64,
}

/// 语义记忆索引单例状态。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SemanticMemoryIndexState {
    /// 当前重建代际；索引失效时清空，同一指纹的并发任务也必须不同。
    #[serde(default)]
    pub build_id: String,
    pub embedding_fingerprint: String,
    pub privacy_fingerprint: String,
    pub rebuild_required: bool,
    pub status: String,
    pub indexed_activities: usize,
    pub total_activities: usize,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

/// 用户确认后持久化的显式长期记忆。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssistantUserMemory {
    pub id: i64,
    pub memory_type: String,
    pub memory_key: String,
    pub value_text: String,
    pub recall_policy: String,
    pub sensitivity: String,
    pub source_kind: String,
    pub source_conversation_id: Option<i64>,
    pub source_request_id: Option<String>,
    pub revision: i64,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 把归一化 f32 向量编码为 LE 字节（存 BLOB）。
pub fn encode_embedding(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for v in vector {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// BLOB → f32 向量（长度非 4 的倍数时丢弃尾部残字节）。
pub fn decode_embedding(blob: &[u8]) -> Vec<f32> {
    blob.as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// 向量 L2 归一化（零向量原样返回，检索时点积为 0 自然沉底）。
pub fn normalize_embedding(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for v in &mut vector {
            *v /= norm;
        }
    }
    vector
}

/// 助手会话（持久化于 SQLite，替代 localStorage 40 条上限）
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantConversation {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: usize,
}

/// 助手会话消息
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantStoredMessage {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    /// 该轮工具执行摘要（JSON 字符串，前端存档；追问时随历史回传给模型）
    pub tool_digest: Option<String>,
    pub model_name: Option<String>,
    pub created_at: i64,
}

/// 工作记忆搜索结果
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchItem {
    pub source_type: String,
    pub source_id: Option<i64>,
    pub date: String,
    pub timestamp: i64,
    pub title: String,
    pub excerpt: String,
    pub app_name: Option<String>,
    pub browser_url: Option<String>,
    pub duration: Option<i64>,
    pub score: i64,
}

const ASSISTANT_USER_MEMORY_COLUMNS: &str = "id, memory_type, memory_key, value_text, recall_policy, sensitivity, source_kind, source_conversation_id, source_request_id, revision, expires_at, created_at, updated_at";
/// 仅用于持久化语义索引活动游标，不属于可检索、可嵌入的真实记忆块。
pub const SEMANTIC_MEMORY_CURSOR_CHUNK_KEY: &str = "__cursor__";

fn assistant_user_memory_from_row(row: &Row<'_>) -> rusqlite::Result<AssistantUserMemory> {
    Ok(AssistantUserMemory {
        id: row.get(0)?,
        memory_type: row.get(1)?,
        memory_key: row.get(2)?,
        value_text: row.get(3)?,
        recall_policy: row.get(4)?,
        sensitivity: row.get(5)?,
        source_kind: row.get(6)?,
        source_conversation_id: row.get(7)?,
        source_request_id: row.get(8)?,
        revision: row.get(9)?,
        expires_at: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// 规范化 URL（用于合并判断）
/// 移除末尾斜杠、规范化空白字符
pub fn normalize_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn parse_date_bounds(date_from: Option<&str>, date_to: Option<&str>) -> (Option<i64>, Option<i64>) {
    let start_ts = date_from.and_then(|date| {
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(safe_local_timestamp)
    });

    let end_ts = date_to.and_then(|date| {
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.succ_opt())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(safe_local_timestamp)
    });

    (start_ts, end_ts)
}

fn calculate_overlap_duration(
    interval_end: i64,
    duration: i64,
    range_start: i64,
    range_end: i64,
) -> i64 {
    if duration <= 0 || range_end <= range_start {
        return 0;
    }

    let interval_start = interval_end.saturating_sub(duration);
    let overlap_start = interval_start.max(range_start);
    let overlap_end = interval_end.min(range_end);
    (overlap_end - overlap_start).max(0)
}

/// 多段工作时间重叠计算。同一时间即使落入多个配置段，也只能计一次。
fn calculate_work_time_segments_overlap(
    interval_end: i64,
    duration: i64,
    day_start: i64,
    day_end: i64,
    segments: &[(i64, i64)],
) -> i64 {
    if duration <= 0 || day_end <= day_start {
        return 0;
    }

    let activity_start = interval_end.saturating_sub(duration).max(day_start);
    let activity_end = interval_end.min(day_end);
    if activity_end <= activity_start {
        return 0;
    }

    let mut overlaps = Vec::new();
    let mut collect_overlap = |range_start: i64, range_end: i64| {
        let start = activity_start.max(range_start);
        let end = activity_end.min(range_end);
        if end > start {
            overlaps.push((start, end));
        }
    };

    for &(work_start, work_end) in segments {
        if work_start == work_end {
            continue;
        }
        if work_end > work_start {
            collect_overlap(work_start, work_end);
        } else {
            collect_overlap(day_start, work_end);
            collect_overlap(work_start, day_end);
        }
    }

    calculate_covered_duration(overlaps)
}

/// 下一个「本地时区整点」边界。
/// 之前用 `(t / 3600 + 1) * 3600` 按 UTC 整点切桶，在 UTC+5:30/+5:45/+9:30 等
/// 非整小时时区，本地小时在 UTC 半点翻转，跨本地整点的区间会整段归入起点小时。
/// 整小时时区（如 UTC+8）两种算法结果一致。
fn next_local_hour_boundary(t: i64) -> i64 {
    use chrono::Timelike;
    match chrono::DateTime::from_timestamp(t, 0) {
        Some(dt) => {
            let local = dt.with_timezone(&chrono::Local);
            let secs_into_hour = i64::from(local.minute()) * 60 + i64::from(local.second());
            t + (3600 - secs_into_hour)
        }
        None => (t / 3600 + 1) * 3600,
    }
}

fn calculate_covered_duration(mut ranges: Vec<(i64, i64)>) -> i64 {
    if ranges.is_empty() {
        return 0;
    }

    ranges.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

    let mut total = 0;
    let mut current_start = ranges[0].0;
    let mut current_end = ranges[0].1;

    for (start, end) in ranges.into_iter().skip(1) {
        if start <= current_end {
            current_end = current_end.max(end);
        } else {
            total += current_end - current_start;
            current_start = start;
            current_end = end;
        }
    }

    total + (current_end - current_start)
}

fn tokenize_memory_query(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|token| token.trim().to_lowercase())
        .filter(|token| token.len() >= 2)
        .collect()
}

fn score_memory_match(query: &str, fields: &[&str]) -> i64 {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.is_empty() {
        return 0;
    }

    let normalized_fields: Vec<String> = fields
        .iter()
        .map(|field| field.trim().to_lowercase())
        .filter(|field| !field.is_empty())
        .collect();

    if normalized_fields.is_empty() {
        return 0;
    }

    let mut score = 0;

    for field in &normalized_fields {
        if field == &normalized_query {
            score += 180;
        } else if field.contains(&normalized_query) {
            score += 120;
        }
    }

    for token in tokenize_memory_query(query) {
        for field in &normalized_fields {
            if field == &token {
                score += 45;
            } else if field.contains(&token) {
                score += 20;
            }
        }
    }

    score
}

fn truncate_excerpt(value: &str, max_chars: usize) -> String {
    let text = value.trim().replace(['\n', '\r'], " ");
    let mut iter = text.chars();
    let excerpt: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

fn pick_excerpt(candidates: &[String]) -> String {
    candidates
        .iter()
        .find(|candidate| !candidate.trim().is_empty())
        .map(|candidate| truncate_excerpt(candidate, 180))
        .unwrap_or_default()
}

fn execute_add_column_if_missing(conn: &Connection, sql: &str) -> Result<()> {
    match conn.execute(sql, []) {
        Ok(_) => Ok(()),
        Err(error)
            if error
                .to_string()
                .to_ascii_lowercase()
                .contains("duplicate column name") =>
        {
            Ok(())
        }
        Err(error) => Err(AppError::Database(error)),
    }
}

/// 数据库管理器
///
/// 内部用 `Arc<Mutex<Connection>>` 实现廉价 Clone，
/// 使得 Database 可以跨 async 边界共享（Agent 架构 Stage 6）。
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Self {
            conn: Arc::clone(&self.conn),
        }
    }
}

impl Database {
    /// 创建新的数据库连接
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;
        // WAL 模式提升并发读写能力（主应用与 MCP Server 可能同时访问同一库）；
        // synchronous=NORMAL 在 WAL 下已足够安全；busy_timeout 避免锁竞争时立刻报错。
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;",
        )?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_tables()?;
        Ok(db)
    }

    /// 备份数据库到目标路径
    pub fn backup_to(&self, db_path: &Path) -> Result<()> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let target = db_path.to_string_lossy().replace('\'', "''");
        conn.execute_batch("PRAGMA wal_checkpoint(FULL);")?;
        conn.execute_batch(&format!("VACUUM INTO '{target}';"))?;
        Ok(())
    }

    /// 初始化数据库表
    fn init_tables(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS activities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                app_name TEXT NOT NULL,
                window_title TEXT NOT NULL,
                screenshot_path TEXT NOT NULL,
                ocr_text TEXT,
                category TEXT NOT NULL,
                duration INTEGER NOT NULL,
                browser_url TEXT,
                executable_path TEXT,
                semantic_category TEXT,
                semantic_confidence INTEGER
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_activities_timestamp_app ON activities (timestamp, app_name)",
            [],
        )?;

        // 反向复合索引：get_last_activity_by_app 等「按 app_name + 时间范围」的热路径
        // （监控循环每次采样都会调用）无法使用上面 (timestamp, app_name) 的前导列,
        // 会退化为范围扫描;get_recent_app_usage / get_app_category_overview 的
        // 相关子查询同样依赖此索引。
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_activities_app_timestamp ON activities (app_name, timestamp)",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS daily_reports (
                date TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                ai_mode TEXT NOT NULL,
                model_name TEXT,
                fallback_reason TEXT,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS daily_reports_localized (
                date TEXT NOT NULL,
                locale TEXT NOT NULL,
                content TEXT NOT NULL,
                ai_mode TEXT NOT NULL,
                model_name TEXT,
                fallback_reason TEXT,
                created_at INTEGER NOT NULL,
                UNIQUE(date, locale)
            )",
            [],
        )?;

        execute_add_column_if_missing(
            &conn,
            "ALTER TABLE daily_reports ADD COLUMN fallback_reason TEXT",
        )?;
        execute_add_column_if_missing(
            &conn,
            "ALTER TABLE daily_reports_localized ADD COLUMN fallback_reason TEXT",
        )?;

        conn.execute(
            "INSERT OR IGNORE INTO daily_reports_localized (date, locale, content, ai_mode, model_name, fallback_reason, created_at)
             SELECT date, 'zh-CN', content, ai_mode, model_name, fallback_reason, created_at
             FROM daily_reports",
            [],
        )?;

        // 小时摘要表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS hourly_summaries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                date TEXT NOT NULL,
                hour INTEGER NOT NULL,
                summary TEXT NOT NULL,
                main_apps TEXT NOT NULL,
                activity_count INTEGER NOT NULL,
                total_duration INTEGER NOT NULL,
                representative_screenshots TEXT,
                created_at INTEGER NOT NULL,
                UNIQUE(date, hour)
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_hourly_summaries_date ON hourly_summaries (date)",
            [],
        )?;

        // 迁移：添加活动扩展列（已经存在时仅忽略明确的 duplicate column 错误）
        execute_add_column_if_missing(&conn, "ALTER TABLE activities ADD COLUMN browser_url TEXT")?;
        execute_add_column_if_missing(
            &conn,
            "ALTER TABLE activities ADD COLUMN executable_path TEXT",
        )?;
        execute_add_column_if_missing(
            &conn,
            "ALTER TABLE activities ADD COLUMN semantic_category TEXT",
        )?;
        execute_add_column_if_missing(
            &conn,
            "ALTER TABLE activities ADD COLUMN semantic_confidence INTEGER",
        )?;
        execute_add_column_if_missing(
            &conn,
            "ALTER TABLE activities ADD COLUMN screenshot_url TEXT",
        )?;

        // URL 合并查询按 RTRIM 规范化；表达式索引避免每次浏览器采样扫描当天历史。
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_activities_browser_url_normalized
             ON activities (RTRIM(browser_url, '/'), timestamp, id)",
            [],
        )?;

        // AI 实体分类缓存:后台任务对未知应用/域名做一次性归类,此后采集直接查表,
        // 无需用户手动加规则。entity_key 形如 "app:xxx" / "domain:yyy"。
        conn.execute(
            "CREATE TABLE IF NOT EXISTS entity_category_cache (
                entity_key TEXT PRIMARY KEY,
                base_category TEXT NOT NULL,
                semantic_category TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'ai',
                created_at INTEGER NOT NULL
            )",
            [],
        )?;

        // === AI 工作记忆（自进化洞察） ===
        conn.execute(
            "CREATE TABLE IF NOT EXISTS insights (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                insight_type TEXT NOT NULL,
                content TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.5,
                source_date TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                confirmed_count INTEGER NOT NULL DEFAULT 0,
                denied_count INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;

        // === 助手会话持久化 ===
        conn.execute(
            "CREATE TABLE IF NOT EXISTS assistant_conversations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS assistant_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                tool_digest TEXT,
                model_name TEXT,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_assistant_messages_conversation
             ON assistant_messages (conversation_id, id)",
            [],
        )?;

        // === 语义记忆块（屏幕级数字记忆） ===
        // 按 (日期, 应用, 标题/域名) 聚合的记忆单元；embedding 为归一化 f32 LE 字节，
        // NULL 表示尚未嵌入（索引任务增量补齐）。
        conn.execute(
            "CREATE TABLE IF NOT EXISTS memory_chunks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chunk_key TEXT NOT NULL UNIQUE,
                date TEXT NOT NULL,
                app_name TEXT NOT NULL,
                title TEXT NOT NULL,
                browser_url TEXT,
                content TEXT NOT NULL,
                embedding BLOB,
                last_activity_id INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_memory_chunks_date ON memory_chunks (date)",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS semantic_memory_state (
                singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
                embedding_fingerprint TEXT NOT NULL DEFAULT '',
                privacy_fingerprint TEXT NOT NULL DEFAULT '',
                rebuild_required INTEGER NOT NULL DEFAULT 1,
                status TEXT NOT NULL DEFAULT 'idle',
                indexed_activities INTEGER NOT NULL DEFAULT 0,
                total_activities INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        // 兼容旧数据库：首次升级时补充重建代际列；重复执行时仅忽略 duplicate column。
        execute_add_column_if_missing(
            &conn,
            "ALTER TABLE semantic_memory_state ADD COLUMN build_id TEXT NOT NULL DEFAULT ''",
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO semantic_memory_state (singleton_id, updated_at) VALUES (1, ?1)",
            params![chrono::Utc::now().timestamp()],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS assistant_user_memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                memory_type TEXT NOT NULL,
                memory_key TEXT NOT NULL,
                value_text TEXT NOT NULL,
                recall_policy TEXT NOT NULL DEFAULT 'relevant',
                sensitivity TEXT NOT NULL DEFAULT 'normal',
                source_kind TEXT NOT NULL DEFAULT 'explicit_chat',
                source_conversation_id INTEGER,
                source_request_id TEXT,
                revision INTEGER NOT NULL DEFAULT 1,
                expires_at INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(memory_type, memory_key)
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_assistant_user_memories_policy_updated
             ON assistant_user_memories (recall_policy, updated_at DESC)",
            [],
        )?;

        // === FTS5 全文检索索引 ===
        // activities FTS: 索引窗口标题、OCR 文本、应用名、浏览器 URL
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS activities_fts USING fts5(
                app_name, window_title, ocr_text, browser_url,
                content='activities',
                content_rowid='id',
                tokenize='unicode61 remove_diacritics 2'
            );

            -- 同步触发器：INSERT
            CREATE TRIGGER IF NOT EXISTS activities_ai AFTER INSERT ON activities BEGIN
                INSERT INTO activities_fts(rowid, app_name, window_title, ocr_text, browser_url)
                VALUES (new.id, new.app_name, new.window_title, new.ocr_text, new.browser_url);
            END;

            -- 同步触发器：DELETE
            CREATE TRIGGER IF NOT EXISTS activities_ad AFTER DELETE ON activities BEGIN
                INSERT INTO activities_fts(activities_fts, rowid, app_name, window_title, ocr_text, browser_url)
                VALUES ('delete', old.id, old.app_name, old.window_title, old.ocr_text, old.browser_url);
            END;

            -- 同步触发器：UPDATE
            CREATE TRIGGER IF NOT EXISTS activities_au AFTER UPDATE ON activities BEGIN
                INSERT INTO activities_fts(activities_fts, rowid, app_name, window_title, ocr_text, browser_url)
                VALUES ('delete', old.id, old.app_name, old.window_title, old.ocr_text, old.browser_url);
                INSERT INTO activities_fts(rowid, app_name, window_title, ocr_text, browser_url)
                VALUES (new.id, new.app_name, new.window_title, new.ocr_text, new.browser_url);
            END;"
        )?;

        // hourly_summaries FTS
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS hourly_summaries_fts USING fts5(
                date, summary, main_apps,
                content='hourly_summaries',
                content_rowid='id',
                tokenize='unicode61 remove_diacritics 2'
            );

            CREATE TRIGGER IF NOT EXISTS hourly_summaries_ai AFTER INSERT ON hourly_summaries BEGIN
                INSERT INTO hourly_summaries_fts(rowid, date, summary, main_apps)
                VALUES (new.id, new.date, new.summary, new.main_apps);
            END;

            CREATE TRIGGER IF NOT EXISTS hourly_summaries_ad AFTER DELETE ON hourly_summaries BEGIN
                INSERT INTO hourly_summaries_fts(hourly_summaries_fts, rowid, date, summary, main_apps)
                VALUES ('delete', old.id, old.date, old.summary, old.main_apps);
            END;

            CREATE TRIGGER IF NOT EXISTS hourly_summaries_au AFTER UPDATE ON hourly_summaries BEGIN
                INSERT INTO hourly_summaries_fts(hourly_summaries_fts, rowid, date, summary, main_apps)
                VALUES ('delete', old.id, old.date, old.summary, old.main_apps);
                INSERT INTO hourly_summaries_fts(rowid, date, summary, main_apps)
                VALUES (new.id, new.date, new.summary, new.main_apps);
            END;"
        )?;

        // daily_reports_localized FTS
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS daily_reports_fts USING fts5(
                date, content, ai_mode,
                content='daily_reports_localized',
                content_rowid='rowid',
                tokenize='unicode61 remove_diacritics 2'
            );

            CREATE TRIGGER IF NOT EXISTS daily_reports_ai AFTER INSERT ON daily_reports_localized BEGIN
                INSERT INTO daily_reports_fts(rowid, date, content, ai_mode)
                VALUES (new.rowid, new.date, new.content, new.ai_mode);
            END;

            CREATE TRIGGER IF NOT EXISTS daily_reports_ad AFTER DELETE ON daily_reports_localized BEGIN
                INSERT INTO daily_reports_fts(daily_reports_fts, rowid, date, content, ai_mode)
                VALUES ('delete', old.rowid, old.date, old.content, old.ai_mode);
            END;

            CREATE TRIGGER IF NOT EXISTS daily_reports_au AFTER UPDATE ON daily_reports_localized BEGIN
                INSERT INTO daily_reports_fts(daily_reports_fts, rowid, date, content, ai_mode)
                VALUES ('delete', old.rowid, old.date, old.content, old.ai_mode);
                INSERT INTO daily_reports_fts(rowid, date, content, ai_mode)
                VALUES (new.rowid, new.date, new.content, new.ai_mode);
            END;"
        )?;

        Ok(())
    }

    /// 重建 FTS 索引（用于首次迁移或修复）
    pub fn rebuild_fts_index(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.execute_batch(
            "INSERT INTO activities_fts(activities_fts) VALUES('rebuild');
             INSERT INTO hourly_summaries_fts(hourly_summaries_fts) VALUES('rebuild');
             INSERT INTO daily_reports_fts(daily_reports_fts) VALUES('rebuild');",
        )?;
        Ok(())
    }

    /// 插入活动记录。若语义索引已就绪，新活动会在同一事务中立即将其标记为陈旧。
    pub fn insert_activity(&self, activity: &Activity) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let normalized_browser_url = activity
            .browser_url
            .as_deref()
            .map(normalize_url)
            .filter(|url| !url.is_empty());
        let tx = conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO activities (timestamp, app_name, window_title, screenshot_path, ocr_text, category, duration, browser_url, executable_path, semantic_category, semantic_confidence, screenshot_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                activity.timestamp,
                activity.app_name,
                activity.window_title,
                activity.screenshot_path,
                activity.ocr_text,
                activity.category,
                activity.duration,
                normalized_browser_url,
                activity.executable_path,
                activity.semantic_category,
                activity.semantic_confidence,
                activity.screenshot_url,
            ],
        )?;
        let activity_id = tx.last_insert_rowid();

        tx.execute(
            "UPDATE semantic_memory_state
             SET rebuild_required = CASE WHEN status = 'ready' THEN 1 ELSE rebuild_required END,
                 status = CASE WHEN status = 'ready' THEN 'idle' ELSE status END,
                 build_id = CASE WHEN status = 'ready' THEN '' ELSE build_id END,
                 total_activities = (SELECT COUNT(*) FROM activities),
                 updated_at = ?1
             WHERE singleton_id = 1",
            params![chrono::Utc::now().timestamp()],
        )?;
        tx.commit()?;

        Ok(activity_id)
    }

    /// 获取指定应用最近24小时内的最新一条活动记录
    pub fn get_last_activity_by_app(&self, app_name: &str) -> Result<Option<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        // 回溯24小时
        let start_ts = chrono::Local::now().timestamp() - 86400;

        let sql = format!(
            "SELECT {ACTIVITY_SELECT_COLUMNS}
             FROM activities
             WHERE app_name = ?1 AND timestamp >= ?2
             ORDER BY id DESC
             LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![app_name, start_ts])?;
        if let Some(row) = rows.next()? {
            Ok(Some(activity_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// 获取指定应用今天的最近一条活动记录（用于合并判断）
    pub fn get_latest_activity_by_app(&self, app_name: &str) -> Result<Option<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        // 获取今天的开始时间戳（当天 00:00:00）
        let today_start = {
            let now = chrono::Local::now();
            let ndt = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
            safe_local_timestamp(ndt)
        };

        let sql = format!(
            "SELECT {ACTIVITY_SELECT_COLUMNS}
             FROM activities
             WHERE app_name = ?1 AND timestamp >= ?2
             ORDER BY id DESC
             LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![app_name, today_start])?;
        if let Some(row) = rows.next()? {
            Ok(Some(activity_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// 获取指定应用 + 窗口标题今天的最近一条活动记录
    /// 当浏览器 URL 暂时不可用时，用于避免不同标签页互相串时长
    pub fn get_latest_activity_by_app_title(
        &self,
        app_name: &str,
        window_title: &str,
    ) -> Result<Option<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let today_start = {
            let now = chrono::Local::now();
            let ndt = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
            safe_local_timestamp(ndt)
        };

        let sql = format!(
            "SELECT {ACTIVITY_SELECT_COLUMNS}
             FROM activities
             WHERE app_name = ?1 AND window_title = ?2 AND timestamp >= ?3
             ORDER BY id DESC
             LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![app_name, window_title, today_start])?;
        if let Some(row) = rows.next()? {
            Ok(Some(activity_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// 按 URL 获取今天的活动记录（用于浏览器 URL 合并）
    /// 使用规范化 URL 进行匹配，解决末尾斜杠差异问题
    pub fn get_latest_activity_by_url(&self, browser_url: &str) -> Result<Option<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let today_start = {
            let now = chrono::Local::now();
            let ndt = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
            safe_local_timestamp(ndt)
        };

        // 规范化输入 URL
        let normalized_url = normalize_url(browser_url);
        log::debug!("URL 合并查询: 原始='{browser_url}', 规范化='{normalized_url}'");

        // 与表达式索引保持一致，兼容历史记录中带末尾斜杠的 URL。
        let sql = format!(
            "SELECT {ACTIVITY_SELECT_COLUMNS}
             FROM activities
             WHERE RTRIM(browser_url, '/') = ?1 AND timestamp >= ?2
             ORDER BY id DESC
             LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![normalized_url, today_start])?;
        if let Some(row) = rows.next()? {
            Ok(Some(activity_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// 根据 ID 获取单个活动
    pub fn get_activity_by_id(&self, id: i64) -> Result<Option<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let sql = format!("SELECT {ACTIVITY_SELECT_COLUMNS} FROM activities WHERE id = ?1");
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(activity_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// 合并活动：累加时长、追加OCR、更新截图路径、更新 browser_url
    pub fn merge_activity(
        &self,
        id: i64,
        duration_delta: i64,
        new_ocr: Option<&str>,
        _new_screenshot_path: &str,
        new_timestamp: i64,
        new_browser_url: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        // 获取现有的 OCR 内容
        let existing_ocr: Option<String> = conn
            .query_row(
                "SELECT ocr_text FROM activities WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();

        // 合并 OCR：追加新内容
        let merged_ocr = match (existing_ocr, new_ocr) {
            (Some(existing), Some(new)) if !new.is_empty() => {
                // 追加新内容，用分隔符隔开
                Some(format!("{existing}\n---\n{new}"))
            }
            (Some(existing), _) => Some(existing),
            (None, Some(new)) => Some(new.to_string()),
            (None, None) => None,
        };

        // 如果有新的 browser_url，一并更新（解决 SPA 页面导航后 URL 不刷新的问题）
        if let Some(url) = new_browser_url
            .map(normalize_url)
            .filter(|url| !url.is_empty())
        {
            conn.execute(
                "UPDATE activities
                 SET duration = duration + ?1,
                     ocr_text = ?2,
                     timestamp = ?3,
                     browser_url = ?5
                 WHERE id = ?4",
                params![duration_delta, merged_ocr, new_timestamp, id, url],
            )?;
        } else {
            conn.execute(
                "UPDATE activities
                 SET duration = duration + ?1,
                     ocr_text = ?2,
                     timestamp = ?3
                 WHERE id = ?4",
                params![duration_delta, merged_ocr, new_timestamp, id],
            )?;
        }

        Ok(())
    }

    /// 更新活动的 OCR 文本，并让异步识别出的浏览器页面应用最新网站分类规则。
    pub fn update_activity_ocr(
        &self,
        id: i64,
        ocr_text: Option<String>,
        website_rules: &[crate::config::WebsiteSemanticRule],
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let tx = conn.unchecked_transaction()?;

        tx.execute(
            "UPDATE activities SET ocr_text = ?1 WHERE id = ?2",
            params![ocr_text, id],
        )?;

        if !website_rules.is_empty() {
            let activity: Option<(String, String, Option<String>, String)> = tx
                .query_row(
                    "SELECT app_name, window_title, browser_url, category
                     FROM activities WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;

            if let Some((app_name, window_title, browser_url, category)) = activity {
                if crate::categorize::is_browser_app(&app_name) {
                    let page_hint = crate::categorize::resolve_browser_page_hint(
                        browser_url.as_deref(),
                        &window_title,
                        ocr_text.as_deref(),
                    );
                    if let Some(semantic_category) =
                        crate::categorize::find_website_semantic_override(
                            website_rules,
                            page_hint.as_deref(),
                        )
                    {
                        let base_category = crate::categorize::semantic_category_to_base_category(
                            &semantic_category,
                            &category,
                        );
                        tx.execute(
                            "UPDATE activities
                             SET category = ?1, semantic_category = ?2, semantic_confidence = 100
                             WHERE id = ?3",
                            params![base_category, semantic_category, id],
                        )?;
                    }
                }
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// 更新活动的远程截图 URL（远程上传成功后调用）
    pub fn update_activity_screenshot_url(&self, id: i64, url: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        conn.execute(
            "UPDATE activities SET screenshot_url = ?1 WHERE id = ?2",
            params![url, id],
        )?;

        Ok(())
    }

    /// 清空指定日期目录下已删除的本地截图路径，保留可能仍有效的远程 URL。
    pub fn clear_screenshot_paths_for_date(&self, date: &str) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let forward_prefix = format!("screenshots/{date}/%");
        let windows_prefix = format!(r"screenshots\{date}\%");
        Ok(conn.execute(
            "UPDATE activities
             SET screenshot_path = ''
             WHERE screenshot_path LIKE ?1 OR screenshot_path LIKE ?2",
            params![forward_prefix, windows_prefix],
        )?)
    }

    /// 查找「本地有截图但远程 URL 缺失」的近期活动（远程存储补传用），
    /// 返回 (id, screenshot_path)，按时间倒序。
    /// 仅看 since_timestamp 之后的记录：更早的缺口多半是远程存储当时未启用、
    /// 或本地文件已被保留策略清理，反复捞出来重试没有意义。
    pub fn get_activities_missing_screenshot_url(
        &self,
        since_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, screenshot_path FROM activities
             WHERE timestamp >= ?1
               AND screenshot_path != ''
               AND (screenshot_url IS NULL OR screenshot_url = '')
             ORDER BY timestamp DESC
             LIMIT ?2",
        )?;
        let rows: Vec<(i64, String)> = stmt
            .query_map(params![since_timestamp, limit], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// 读取全部 AI 实体分类缓存（启动时载入内存查询表）。
    pub fn load_entity_category_cache(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT entity_key, base_category, semantic_category FROM entity_category_cache",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// 写入/覆盖一条实体分类缓存。
    pub fn upsert_entity_category(
        &self,
        entity_key: &str,
        base_category: &str,
        semantic_category: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.execute(
            "INSERT OR REPLACE INTO entity_category_cache
             (entity_key, base_category, semantic_category, source, created_at)
             VALUES (?1, ?2, ?3, 'ai', ?4)",
            params![
                entity_key,
                base_category,
                semantic_category,
                chrono::Local::now().timestamp()
            ],
        )?;
        Ok(())
    }

    /// 收集近期仍归为 "other" 的应用候选（按累计时长降序,过滤琐碎实体）。
    pub fn get_unclassified_app_candidates(
        &self,
        since_ts: i64,
        min_total_duration: i64,
        limit: u32,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT app_name, SUM(duration) AS total FROM activities
             WHERE timestamp >= ?1 AND category = 'other' AND app_name != ''
             GROUP BY app_name HAVING total >= ?2
             ORDER BY total DESC LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![since_ts, min_total_duration, limit], |row| {
                row.get::<_, String>(0)
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// 近期浏览器活动的 (browser_url, 累计时长)，供上层聚合到域名后挑选归类候选。
    pub fn get_browser_url_durations(&self, since_ts: i64) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT browser_url, SUM(duration) FROM activities
             WHERE timestamp >= ?1 AND category = 'browser'
               AND browser_url IS NOT NULL AND browser_url != ''
             GROUP BY browser_url",
        )?;
        let rows = stmt
            .query_map(params![since_ts], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// 近期浏览器活动的 (id, browser_url)，供 AI 归类后按域名回溯修正。
    pub fn get_browser_activity_urls(&self, since_ts: i64) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT id, browser_url FROM activities
             WHERE timestamp >= ?1 AND category = 'browser'
               AND browser_url IS NOT NULL AND browser_url != ''",
        )?;
        let rows = stmt
            .query_map(params![since_ts], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// AI 归类后回溯修正：按应用名更新近期仍为 "other" 的记录,返回更新行数。
    pub fn reclassify_app_activities(
        &self,
        app_name: &str,
        base_category: &str,
        semantic_category: &str,
        since_ts: i64,
    ) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        // 学习键为小写,应用名原始大小写不定,按 LOWER 匹配
        let count = conn.execute(
            "UPDATE activities SET category = ?1, semantic_category = ?2
             WHERE LOWER(app_name) = ?3 AND timestamp >= ?4 AND category = 'other'",
            params![
                base_category,
                semantic_category,
                app_name.to_lowercase(),
                since_ts
            ],
        )?;
        Ok(count)
    }

    /// AI 归类后回溯修正：按 id 列表更新（域名场景,上层已按域名筛出 id）。
    pub fn reclassify_activities_by_ids(
        &self,
        ids: &[i64],
        base_category: &str,
        semantic_category: &str,
    ) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        // id 为内部 i64,直接拼接数字占位安全
        let id_list = ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let count = conn.execute(
            &format!(
                "UPDATE activities SET category = ?1, semantic_category = ?2 WHERE id IN ({id_list})"
            ),
            params![base_category, semantic_category],
        )?;
        Ok(count)
    }

    /// 将指定分类下的历史活动记录重新归类到目标分类，返回更新的行数
    pub fn reassign_activity_category(&self, from: &str, to: &str) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let count = conn.execute(
            "UPDATE activities SET category = ?1 WHERE category = ?2",
            params![to, from],
        )?;

        Ok(count)
    }

    /// 删除指定日期之前的所有活动记录（使用时间戳范围查询以利用索引）。
    /// 活动、摘要缓存与语义索引失效在同一事务中提交。
    pub fn delete_activities_before_date(&self, before_date: &str) -> Result<usize> {
        let date_parsed = chrono::NaiveDate::parse_from_str(before_date, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;
        let upper_ts = safe_local_timestamp(date_parsed.and_hms_opt(0, 0, 0).unwrap());
        let conn = self
            .conn
            .lock()
            .map_err(|e| AppError::Unknown(format!("数据库锁获取失败: {e}")))?;
        let tx = conn.unchecked_transaction()?;
        let dates = {
            let mut stmt = tx.prepare("SELECT timestamp FROM activities WHERE timestamp < ?1")?;
            let dates = stmt
                .query_map(params![upper_ts], |row| row.get::<_, i64>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .map(Self::ts_to_local_date)
                .filter(|date| !date.is_empty())
                .collect::<std::collections::HashSet<_>>();
            dates
        };
        let deleted = tx.execute(
            "DELETE FROM activities WHERE timestamp < ?1",
            params![upper_ts],
        )?;
        if deleted > 0 {
            Self::invalidate_daily_cache(&tx, &dates)?;
            Self::clear_memory_chunks_and_mark_rebuild_required_on(&tx)?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// Unix 时间戳 → 本地时区日期字符串（YYYY-MM-DD）
    fn ts_to_local_date(ts: i64) -> String {
        chrono::Local
            .timestamp_opt(ts, 0)
            .single()
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default()
    }

    /// 失效指定日期集合的日报与小时摘要缓存。
    fn invalidate_daily_cache(
        conn: &Connection,
        dates: &std::collections::HashSet<String>,
    ) -> Result<()> {
        for date in dates {
            conn.execute("DELETE FROM daily_reports WHERE date = ?1", params![date])?;
            conn.execute(
                "DELETE FROM daily_reports_localized WHERE date = ?1",
                params![date],
            )?;
            conn.execute(
                "DELETE FROM hourly_summaries WHERE date = ?1",
                params![date],
            )?;
        }
        Ok(())
    }

    /// 删除单条活动记录，返回删除数量和截图路径（供上层删截图文件）
    pub fn delete_activity_by_id(&self, id: i64) -> Result<(usize, Vec<String>)> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let tx = conn.unchecked_transaction()?;
        let row = tx
            .query_row(
                "SELECT screenshot_path, timestamp FROM activities WHERE id = ?1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let paths = row
            .as_ref()
            .map(|(path, _)| path)
            .filter(|path| !path.is_empty())
            .cloned()
            .into_iter()
            .collect();
        let dates = row
            .map(|(_, timestamp)| Self::ts_to_local_date(timestamp))
            .filter(|date| !date.is_empty())
            .into_iter()
            .collect::<std::collections::HashSet<_>>();

        let deleted = tx.execute("DELETE FROM activities WHERE id = ?1", params![id])?;
        Self::invalidate_daily_cache(&tx, &dates)?;
        Self::clear_memory_chunks_and_mark_rebuild_required_on(&tx)?;
        tx.commit()?;
        Ok((deleted, paths))
    }

    /// 删除指定日期全天（本地时区）的活动记录，返回删除数量和截图路径
    pub fn delete_activities_by_date(&self, date: &str) -> Result<(usize, Vec<String>)> {
        let date_parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;
        let (start_ts, end_ts) = local_day_bounds(date_parsed)?;
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let tx = conn.unchecked_transaction()?;
        let paths = {
            let mut stmt = tx.prepare(
                "SELECT screenshot_path FROM activities WHERE timestamp >= ?1 AND timestamp < ?2",
            )?;
            let paths = stmt
                .query_map(params![start_ts, end_ts], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|path| !path.is_empty())
                .collect::<Vec<_>>();
            paths
        };
        let deleted = tx.execute(
            "DELETE FROM activities WHERE timestamp >= ?1 AND timestamp < ?2",
            params![start_ts, end_ts],
        )?;
        let dates = std::iter::once(date.to_string()).collect();
        Self::invalidate_daily_cache(&tx, &dates)?;
        Self::clear_memory_chunks_and_mark_rebuild_required_on(&tx)?;
        tx.commit()?;
        Ok((deleted, paths))
    }

    /// 删除指定时间段 [start_ts, end_ts)（Unix 秒）内的活动记录，返回删除数量和截图路径
    pub fn delete_activities_by_range(
        &self,
        start_ts: i64,
        end_ts: i64,
    ) -> Result<(usize, Vec<String>)> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let tx = conn.unchecked_transaction()?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT screenshot_path, timestamp FROM activities
                 WHERE timestamp >= ?1 AND timestamp < ?2",
            )?;
            let rows = stmt
                .query_map(params![start_ts, end_ts], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let paths = rows
            .iter()
            .filter(|(path, _)| !path.is_empty())
            .map(|(path, _)| path.clone())
            .collect();
        let dates = rows
            .iter()
            .map(|(_, timestamp)| Self::ts_to_local_date(*timestamp))
            .filter(|date| !date.is_empty())
            .collect::<std::collections::HashSet<_>>();
        let deleted = tx.execute(
            "DELETE FROM activities WHERE timestamp >= ?1 AND timestamp < ?2",
            params![start_ts, end_ts],
        )?;
        Self::invalidate_daily_cache(&tx, &dates)?;
        Self::clear_memory_chunks_and_mark_rebuild_required_on(&tx)?;
        tx.commit()?;
        Ok((deleted, paths))
    }

    /// 删除指定应用的所有活动记录；date_range 为 Some 时限定时间段，返回删除数量和截图路径
    pub fn delete_activities_by_app(
        &self,
        app_name: &str,
        date_range: Option<(i64, i64)>,
    ) -> Result<(usize, Vec<String>)> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let tx = conn.unchecked_transaction()?;
        let rows = match date_range {
            Some((start_ts, end_ts)) => {
                let mut stmt = tx.prepare(
                    "SELECT screenshot_path, timestamp FROM activities
                     WHERE app_name = ?1 AND timestamp >= ?2 AND timestamp < ?3",
                )?;
                let rows = stmt
                    .query_map(params![app_name, start_ts, end_ts], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            }
            None => {
                let mut stmt = tx.prepare(
                    "SELECT screenshot_path, timestamp FROM activities WHERE app_name = ?1",
                )?;
                let rows = stmt
                    .query_map(params![app_name], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            }
        };
        let paths = rows
            .iter()
            .filter(|(path, _)| !path.is_empty())
            .map(|(path, _)| path.clone())
            .collect();
        let dates = rows
            .iter()
            .map(|(_, timestamp)| Self::ts_to_local_date(*timestamp))
            .filter(|date| !date.is_empty())
            .collect::<std::collections::HashSet<_>>();
        let deleted = match date_range {
            Some((start_ts, end_ts)) => tx.execute(
                "DELETE FROM activities
                 WHERE app_name = ?1 AND timestamp >= ?2 AND timestamp < ?3",
                params![app_name, start_ts, end_ts],
            )?,
            None => tx.execute(
                "DELETE FROM activities WHERE app_name = ?1",
                params![app_name],
            )?,
        };
        Self::invalidate_daily_cache(&tx, &dates)?;
        Self::clear_memory_chunks_and_mark_rebuild_required_on(&tx)?;
        tx.commit()?;
        Ok((deleted, paths))
    }

    /// 获取指定日期的统计数据
    /// work_start_hour: 工作开始时间（0-23），默认 9
    /// work_end_hour: 工作结束时间（0-23），默认 18
    /// 按分段工作时间获取每日统计
    pub fn get_daily_stats_with_segments(
        &self,
        date: &str,
        segments: &[crate::config::WorkTimeSegment],
    ) -> Result<DailyStats> {
        self.get_daily_stats_with_segments_filtered(date, segments, &[], &[])
    }

    /// 按分段工作时间获取每日统计，并在聚合前应用隐私过滤。
    ///
    /// 过滤前移到聚合入口，保证总时长、应用、分类、小时分布、网站统计同口径。
    pub fn get_daily_stats_with_segments_filtered(
        &self,
        date: &str,
        segments: &[crate::config::WorkTimeSegment],
        ignored_apps: &[String],
        excluded_domains: &[String],
    ) -> Result<DailyStats> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let date_parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;

        let (start_ts, end_ts) = local_day_bounds(date_parsed)?;

        // 计算所有工作段时间戳
        let segment_ts: Vec<(i64, i64)> = segments
            .iter()
            .map(|s| {
                let ws = (s.start_hour as u32).min(23);
                let we = (s.end_hour as u32).min(23);
                let wsm = (s.start_minute as u32).min(59);
                let wem = (s.end_minute as u32).min(59);
                let work_start = safe_local_timestamp(date_parsed.and_hms_opt(ws, wsm, 0).unwrap());
                let work_end = safe_local_timestamp(date_parsed.and_hms_opt(we, wem, 0).unwrap());
                (work_start, work_end)
            })
            .collect();

        let mut screenshot_count: i64 = 0;

        let mut stmt = conn.prepare(
            "SELECT timestamp,
                    app_name,
                    window_title,
                    ocr_text,
                    category,
                    duration,
                    browser_url,
                    executable_path,
                    semantic_category,
                    screenshot_path,
                    screenshot_url
             FROM activities
             WHERE timestamp > ?1 AND (timestamp - duration) < ?2
             ORDER BY timestamp ASC",
        )?;

        let activity_rows = stmt.query_map(params![start_ts, end_ts], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })?;

        let mut total_duration: i64 = 0;
        let mut work_time_duration: i64 = 0;
        let mut after_hours_duration: i64 = 0;
        // 最后一个工作时段的结束时间戳，用于计算下班后加班时长。
        // 跨午夜段（如 22:00→次日06:00）的结束时间属于次日；对当前自然日而言，
        // 该工作段一直覆盖到日界线，因此以 end_ts 参与比较，避免班内活动被误判为加班。
        let last_segment_end_ts = segment_ts
            .iter()
            .map(|&(ws, we)| if we < ws { end_ts } else { we })
            .max()
            .unwrap_or(end_ts);
        let mut app_usage_map: std::collections::HashMap<
            String,
            (i64, i64, Option<String>, Option<String>, i64),
        > = std::collections::HashMap::new();
        let mut category_usage_map: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        let mut browser_duration: i64 = 0;
        let mut hourly_ranges: [Vec<(i64, i64)>; 24] = std::array::from_fn(|_| Vec::new());

        let mut browser_map: std::collections::HashMap<
            String,
            std::collections::HashMap<String, std::collections::HashMap<String, i64>>,
        > = std::collections::HashMap::new();
        let mut browser_duration_map: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        let mut browser_path_map: std::collections::HashMap<String, (Option<String>, i64)> =
            std::collections::HashMap::new();
        let mut url_duration_map: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        let mut domain_semantic_map: std::collections::HashMap<
            String,
            std::collections::HashMap<String, i64>,
        > = std::collections::HashMap::new();

        let activity_rows: Vec<_> = activity_rows.collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        for (
            timestamp,
            app_name,
            window_title,
            ocr_text,
            category,
            duration,
            browser_url,
            executable_path,
            semantic_category,
            screenshot_path,
            screenshot_url,
        ) in activity_rows
        {
            if matches_ignored_app_for_stats(&app_name, ignored_apps) {
                continue;
            }

            let browser_page = if crate::categorize::is_browser_app(&app_name) {
                let normalized_browser_url = browser_url
                    .as_deref()
                    .map(normalize_url)
                    .filter(|url| !url.is_empty());

                let page_hint = crate::categorize::resolve_browser_page_hint(
                    browser_url.as_deref(),
                    &window_title,
                    ocr_text.as_deref(),
                );

                let (domain, page_hint) = match page_hint {
                    Some(page_hint) => (
                        crate::categorize::browser_page_domain_label(&page_hint),
                        page_hint,
                    ),
                    None => (
                        UNRESOLVED_BROWSER_DOMAIN_LABEL.to_string(),
                        normalized_browser_url
                            .unwrap_or_else(|| UNRESOLVED_BROWSER_URL_LABEL.to_string()),
                    ),
                };

                if matches_excluded_domain_for_stats(&domain, excluded_domains)
                    || matches_excluded_domain_for_stats(&page_hint, excluded_domains)
                {
                    continue;
                }

                Some((
                    crate::categorize::normalize_display_app_name(&app_name),
                    domain,
                    page_hint,
                ))
            } else {
                None
            };

            let day_duration = calculate_overlap_duration(timestamp, duration, start_ts, end_ts);
            if day_duration <= 0 {
                continue;
            }

            if timestamp >= start_ts
                && timestamp < end_ts
                && (!screenshot_path.trim().is_empty()
                    || screenshot_url
                        .as_deref()
                        .is_some_and(|url| !url.trim().is_empty()))
            {
                screenshot_count += 1;
            }

            total_duration += day_duration;

            if category_counts_toward_work_time(&category) {
                work_time_duration += calculate_work_time_segments_overlap(
                    timestamp,
                    duration,
                    start_ts,
                    end_ts,
                    &segment_ts,
                );

                // 加班也是工作时长，娱乐活动不能进入“含加班”口径。
                if timestamp > last_segment_end_ts {
                    after_hours_duration += calculate_overlap_duration(
                        timestamp,
                        duration,
                        last_segment_end_ts,
                        end_ts,
                    );
                }
            }

            let interval_start = timestamp.saturating_sub(duration);
            let overlap_start = interval_start.max(start_ts);
            let overlap_end = timestamp.min(end_ts);
            if overlap_end > overlap_start {
                let mut t = overlap_start;
                while t < overlap_end {
                    let hour = chrono::DateTime::from_timestamp(t, 0)
                        .map(|dt| dt.with_timezone(&chrono::Local).format("%H").to_string())
                        .unwrap_or_default();
                    let bucket_end = next_local_hour_boundary(t);
                    let range_end = overlap_end.min(bucket_end);
                    if let Ok(h) = hour.parse::<usize>() {
                        if h < 24 {
                            hourly_ranges[h].push((t, range_end));
                        }
                    }
                    t = bucket_end;
                }
            }

            let display_name = crate::categorize::normalize_display_app_name(&app_name);
            let entry = app_usage_map.entry(display_name.clone()).or_insert((
                0,
                0,
                executable_path.clone(),
                None,
                i64::MIN,
            ));
            entry.0 += day_duration;
            entry.1 += 1;
            if entry.2.is_none() && executable_path.is_some() {
                entry.2 = executable_path.clone();
            }
            if screenshot_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
                && timestamp >= entry.4
            {
                entry.3 = screenshot_url.clone();
                entry.4 = timestamp;
            }

            // 保留自定义类别 key（trim/lowercase 归一化即可，不强制收敛到基础分类）。
            // 之前用 normalize_category_key 会把自定义类别一律归到 "other"，导致日报时间分配看不到自定义类别。
            let norm_cat = category.trim().to_lowercase();
            *category_usage_map.entry(norm_cat).or_insert(0) += day_duration;

            if let Some((normalized_browser_name, domain, page_hint)) = browser_page {
                browser_duration += day_duration;
                *browser_duration_map
                    .entry(normalized_browser_name.clone())
                    .or_insert(0) += day_duration;
                {
                    let browser_entry = browser_path_map
                        .entry(normalized_browser_name.clone())
                        .or_insert((None, 0));
                    if executable_path
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .is_some()
                        && timestamp >= browser_entry.1
                    {
                        browser_entry.0 = executable_path.clone();
                        browser_entry.1 = timestamp;
                    }
                }

                let domain_map = browser_map.entry(normalized_browser_name).or_default();
                let page_map = domain_map.entry(domain.clone()).or_default();
                *page_map.entry(page_hint.clone()).or_insert(0) += day_duration;
                *url_duration_map.entry(page_hint).or_insert(0) += day_duration;

                if let Some(semantic_cat) = semantic_category
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    *domain_semantic_map
                        .entry(domain.clone())
                        .or_default()
                        .entry(semantic_cat.to_string())
                        .or_insert(0) += day_duration;
                }
            }
        }

        drop(conn);

        let mut app_usage: Vec<AppUsage> = app_usage_map
            .into_iter()
            .map(|(name, (dur, count, exe, screenshot_url, _))| AppUsage {
                app_name: name,
                duration: dur,
                count,
                executable_path: exe,
                screenshot_url,
            })
            .collect();
        app_usage.sort_by_key(|b| Reverse(b.duration));

        let mut category_usage: Vec<CategoryUsage> = category_usage_map
            .into_iter()
            .map(|(category, duration)| CategoryUsage { category, duration })
            .collect();
        category_usage.sort_by_key(|b| Reverse(b.duration));

        let hourly_activity_distribution: Vec<HourlyActivityBucket> = hourly_ranges
            .iter()
            .enumerate()
            .map(|(hour, ranges)| HourlyActivityBucket {
                hour: hour as i32,
                duration: calculate_covered_duration(ranges.clone()),
            })
            .collect();

        let pick_semantic = |semantic_map: &std::collections::HashMap<
            String,
            std::collections::HashMap<String, i64>,
        >,
                             domain: &str|
         -> Option<String> {
            semantic_map.get(domain).and_then(|candidates| {
                candidates
                    .iter()
                    .max_by(|(left_name, left_duration), (right_name, right_duration)| {
                        left_duration
                            .cmp(right_duration)
                            .then_with(|| right_name.cmp(left_name))
                    })
                    .map(|(semantic_category, _)| semantic_category.clone())
            })
        };

        let browser_durations: Vec<(String, i64)> = browser_duration_map.into_iter().collect();
        let mut browser_usage: Vec<BrowserUsage> = browser_durations
            .iter()
            .map(|(browser_name, total_duration)| {
                let domain_map = browser_map.get(browser_name);
                let mut domains: Vec<DomainUsage> = match domain_map {
                    Some(dm) => dm
                        .iter()
                        .map(|(domain, urls)| {
                            let mut url_details: Vec<UrlDetail> = urls
                                .iter()
                                .map(|(url, duration)| UrlDetail {
                                    url: url.clone(),
                                    duration: *duration,
                                })
                                .collect();
                            url_details.sort_by(|a, b| {
                                b.duration.cmp(&a.duration).then_with(|| a.url.cmp(&b.url))
                            });
                            let domain_duration: i64 = url_details.iter().map(|u| u.duration).sum();
                            DomainUsage {
                                domain: domain.clone(),
                                duration: domain_duration,
                                semantic_category: pick_semantic(&domain_semantic_map, domain),
                                urls: url_details,
                            }
                        })
                        .collect(),
                    None => Vec::new(),
                };
                domains.sort_by_key(|b| Reverse(b.duration));

                BrowserUsage {
                    browser_name: browser_name.clone(),
                    duration: *total_duration,
                    executable_path: browser_path_map
                        .get(browser_name)
                        .and_then(|(path, _)| path.clone()),
                    domains,
                }
            })
            .collect();
        browser_usage.sort_by_key(|b| Reverse(b.duration));

        let mut url_usage_rows: Vec<(String, i64)> = url_duration_map.into_iter().collect();
        url_usage_rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        // 先从全部 URL 聚合域名总时长，再截断 URL 列表
        let mut domain_total_map: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for (url, duration) in &url_usage_rows {
            let domain = crate::categorize::browser_page_domain_label(url);
            *domain_total_map.entry(domain).or_insert(0) += duration;
        }

        // 域名 URL 明细：必须用「截断前」的完整列表构建,否则每个域名的"页数"
        // (domain.urls.len)会被全局 top-10 截断压制——所有域名页数之和 ≤10,
        // 概览"常驻网站"的页数因此严重偏小(这是页数显示不对的根因)。
        // 时长聚合(domain_total_map)本就在截断前完成,不受影响。
        let mut domain_url_detail_map: std::collections::HashMap<String, Vec<UrlDetail>> =
            std::collections::HashMap::new();
        for (url, duration) in &url_usage_rows {
            domain_url_detail_map
                .entry(crate::categorize::browser_page_domain_label(url))
                .or_default()
                .push(UrlDetail {
                    url: url.clone(),
                    duration: *duration,
                });
        }

        // url_usage 仍取全局 top-10(用于"最常访问 URL"展示),与页数明细解耦
        url_usage_rows.truncate(10);
        let url_usage: Vec<UrlUsage> = url_usage_rows
            .into_iter()
            .map(|(url, duration)| UrlUsage {
                domain: crate::categorize::browser_page_domain_label(&url),
                url,
                duration,
            })
            .collect();
        let mut domain_usage: Vec<DomainUsage> = domain_total_map
            .into_iter()
            .map(|(domain, duration)| {
                let semantic_category = pick_semantic(&domain_semantic_map, &domain);
                DomainUsage {
                    domain: domain.clone(),
                    duration,
                    semantic_category,
                    urls: domain_url_detail_map.remove(&domain).unwrap_or_default(),
                }
            })
            .collect();
        domain_usage.sort_by_key(|b| Reverse(b.duration));
        let domain_total_count = domain_usage.len();

        Ok(DailyStats {
            total_duration,
            screenshot_count,
            app_usage,
            category_usage,
            browser_duration,
            url_usage,
            domain_usage,
            domain_total_count,
            browser_usage,
            work_time_duration,
            overtime_duration: after_hours_duration,
            hourly_activity_distribution,
        })
    }

    pub fn get_daily_stats_with_work_time(
        &self,
        date: &str,
        work_start_hour: u8,
        work_end_hour: u8,
        work_start_minute: u8,
        work_end_minute: u8,
    ) -> Result<DailyStats> {
        let segments = vec![crate::config::WorkTimeSegment {
            start_hour: work_start_hour,
            start_minute: work_start_minute,
            end_hour: work_end_hour,
            end_minute: work_end_minute,
        }];
        self.get_daily_stats_with_segments(date, &segments)
    }

    /// 获取指定日期的统计数据（使用默认工作时间 9:00-18:00）
    pub fn get_daily_stats(&self, date: &str) -> Result<DailyStats> {
        self.get_daily_stats_with_work_time(date, 9, 18, 0, 0)
    }

    /// 获取指定日期的时间线 (支持分页)
    /// 使用 GROUP BY 聚合，确保同一应用（同 URL）只返回一条记录
    pub fn get_timeline(
        &self,
        date: &str,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let date_parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;
        let (start_ts, end_ts) = local_day_bounds(date_parsed)?;

        let limit_val = limit.unwrap_or(1000);
        let offset_val = offset.unwrap_or(0);

        let mut stmt = conn.prepare(
            "WITH ranked AS (
                SELECT
                    id,
                    timestamp,
                    app_name,
                    window_title,
                    screenshot_path,
                    ocr_text,
                    category,
                    duration,
                    COALESCE(RTRIM(browser_url, '/'), '') as browser_url,
                    executable_path,
                    semantic_category,
                    semantic_confidence,
                    screenshot_url,
                    ROW_NUMBER() OVER (
                        PARTITION BY
                            app_name,
                            CASE
                                WHEN browser_url IS NOT NULL AND browser_url != '' THEN RTRIM(browser_url, '/')
                                ELSE window_title
                            END
                        ORDER BY timestamp DESC, id DESC
                    ) as rn,
                    SUM(MAX(0, MIN(timestamp, ?2) - MAX(timestamp - duration, ?1))) OVER (
                        PARTITION BY
                            app_name,
                            CASE
                                WHEN browser_url IS NOT NULL AND browser_url != '' THEN RTRIM(browser_url, '/')
                                ELSE window_title
                            END
                    ) as total_duration
                FROM activities
                WHERE timestamp > ?1 AND (timestamp - duration) < ?2
             )
             SELECT
                id,
                timestamp,
                app_name,
                window_title,
                screenshot_path,
                ocr_text,
                category,
                total_duration,
                browser_url,
                executable_path,
                semantic_category,
                semantic_confidence,
                screenshot_url
             FROM ranked
             WHERE rn = 1
             ORDER BY timestamp DESC, id DESC
             LIMIT ?3 OFFSET ?4",
        )?;

        let activities: Vec<Activity> = stmt
            .query_map(params![start_ts, end_ts, limit_val, offset_val], |row| {
                let browser_url: String = row.get(8)?;
                Ok(Activity {
                    id: Some(row.get(0)?),
                    timestamp: row.get(1)?,
                    app_name: row.get(2)?,
                    window_title: row.get(3)?,
                    screenshot_path: row.get(4)?,
                    ocr_text: row.get(5)?,
                    category: row.get(6)?,
                    duration: row.get(7)?,
                    browser_url: if browser_url.is_empty() {
                        None
                    } else {
                        Some(browser_url)
                    },
                    executable_path: row.get(9)?,
                    semantic_category: row.get(10)?,
                    semantic_confidence: row.get(11)?,
                    screenshot_url: row.get(12)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(activities)
    }

    /// 获取单日每小时×应用的时长分布。
    /// 返回 24 个小时桶，每桶内含各应用的累计时长。
    pub fn get_hourly_app_breakdown(&self, date: &str) -> Result<Vec<HourlyAppBucket>> {
        self.get_hourly_app_breakdown_range(date, date)
    }

    /// 获取日期范围内每小时×应用的时长分布。
    /// 多日范围按“小时-of-day”合并，例如所有日期的 10:00 都汇总到 hour=10。
    pub fn get_hourly_app_breakdown_range(
        &self,
        date_from: &str,
        date_to: &str,
    ) -> Result<Vec<HourlyAppBucket>> {
        self.get_hourly_app_breakdown_range_filtered(date_from, date_to, &[], &[])
    }

    /// 获取日期范围内每小时×应用的时长分布，并在活动行进入小时聚合前应用隐私过滤。
    pub fn get_hourly_app_breakdown_range_filtered(
        &self,
        date_from: &str,
        date_to: &str,
        ignored_apps: &[String],
        excluded_domains: &[String],
    ) -> Result<Vec<HourlyAppBucket>> {
        let mut start_date = chrono::NaiveDate::parse_from_str(date_from, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;
        let mut end_date = chrono::NaiveDate::parse_from_str(date_to, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;
        if start_date > end_date {
            std::mem::swap(&mut start_date, &mut end_date);
        }
        let start_ts = local_day_bounds(start_date)?.0;
        let end_ts = local_day_bounds(end_date)?.1;
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut stmt = conn.prepare(
            "SELECT
                id,
                timestamp,
                app_name,
                window_title,
                ocr_text,
                category,
                duration,
                screenshot_url,
                browser_url
             FROM activities
             WHERE timestamp > ?1 AND (timestamp - duration) < ?2 AND duration > 0
             ORDER BY timestamp ASC",
        )?;

        let raw: Vec<(
            i64,
            i64,
            String,
            String,
            Option<String>,
            String,
            i64,
            Option<String>,
            Option<String>,
        )> = stmt
            .query_map(params![start_ts, end_ts], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        #[derive(Clone)]
        struct HourlySegment {
            hour: i32,
            start: i64,
            end: i64,
            timestamp: i64,
            row_id: i64,
            app_name: String,
            category: String,
            screenshot_url: Option<String>,
        }

        let mut segments = Vec::new();
        let mut app_map: std::collections::HashMap<
            (i32, String),
            (
                i64,
                std::collections::HashMap<String, i64>,
                Option<String>,
                i64,
                i64,
            ),
        > = std::collections::HashMap::new();

        for (
            row_id,
            timestamp,
            app_name,
            window_title,
            ocr_text,
            category,
            duration,
            screenshot_url,
            browser_url,
        ) in raw
        {
            if matches_ignored_app_for_stats(&app_name, ignored_apps) {
                continue;
            }

            if crate::categorize::is_browser_app(&app_name) {
                let normalized_browser_url = browser_url
                    .as_deref()
                    .map(normalize_url)
                    .filter(|url| !url.is_empty());

                let page_hint = crate::categorize::resolve_browser_page_hint(
                    browser_url.as_deref(),
                    &window_title,
                    ocr_text.as_deref(),
                );

                let (domain, page_hint) = match page_hint {
                    Some(page_hint) => (
                        crate::categorize::browser_page_domain_label(&page_hint),
                        page_hint,
                    ),
                    None => (
                        UNRESOLVED_BROWSER_DOMAIN_LABEL.to_string(),
                        normalized_browser_url
                            .unwrap_or_else(|| UNRESOLVED_BROWSER_URL_LABEL.to_string()),
                    ),
                };

                if matches_excluded_domain_for_stats(&domain, excluded_domains)
                    || matches_excluded_domain_for_stats(&page_hint, excluded_domains)
                {
                    continue;
                }
            }

            let interval_start = timestamp.saturating_sub(duration);
            let overlap_start = interval_start.max(start_ts);
            let overlap_end = timestamp.min(end_ts);
            if overlap_end <= overlap_start {
                continue;
            }

            let display_name = crate::categorize::normalize_display_app_name(&app_name);
            let mut t = overlap_start;
            while t < overlap_end {
                let hour = chrono::DateTime::from_timestamp(t, 0)
                    .map(|dt| dt.with_timezone(&chrono::Local).format("%H").to_string())
                    .unwrap_or_default();
                let bucket_end = next_local_hour_boundary(t);
                let range_end = overlap_end.min(bucket_end);

                if let Ok(hour) = hour.parse::<i32>() {
                    if (0..24).contains(&hour) {
                        segments.push(HourlySegment {
                            hour,
                            start: t,
                            end: range_end,
                            timestamp,
                            row_id,
                            app_name: display_name.clone(),
                            category: category.clone(),
                            screenshot_url: screenshot_url.clone(),
                        });
                    }
                }
                t = bucket_end;
            }
        }

        for hour in 0..24 {
            let hour_segments: Vec<&HourlySegment> = segments
                .iter()
                .filter(|segment| segment.hour == hour)
                .collect();
            if hour_segments.is_empty() {
                continue;
            }

            let mut points = Vec::with_capacity(hour_segments.len() * 2);
            for segment in &hour_segments {
                points.push(segment.start);
                points.push(segment.end);
            }
            points.sort_unstable();
            points.dedup();

            // 和主小时柱一致：重叠活动只统计一次。每个原子时间段归给最新观测记录。
            for window in points.windows(2) {
                let [segment_start, segment_end] = [window[0], window[1]];
                if segment_end <= segment_start {
                    continue;
                }

                let Some(chosen) = hour_segments
                    .iter()
                    .filter(|segment| segment.start < segment_end && segment.end > segment_start)
                    .max_by(|left, right| {
                        left.timestamp
                            .cmp(&right.timestamp)
                            .then_with(|| left.row_id.cmp(&right.row_id))
                    })
                else {
                    continue;
                };

                let segment_duration = segment_end - segment_start;
                let entry = app_map.entry((hour, chosen.app_name.clone())).or_insert((
                    0,
                    std::collections::HashMap::new(),
                    None,
                    i64::MIN,
                    i64::MIN,
                ));
                entry.0 += segment_duration;
                *entry.1.entry(chosen.category.clone()).or_insert(0) += segment_duration;
                if chosen
                    .screenshot_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some()
                    && (chosen.timestamp, chosen.row_id) >= (entry.3, entry.4)
                {
                    entry.2 = chosen.screenshot_url.clone();
                    entry.3 = chosen.timestamp;
                    entry.4 = chosen.row_id;
                }
            }
        }

        let mut buckets: Vec<HourlyAppBucket> = (0..24)
            .map(|h| HourlyAppBucket {
                hour: h,
                total_duration: 0,
                apps: Vec::new(),
            })
            .collect();

        for ((hour, app_name), (duration, category_votes, screenshot_url, _, _)) in app_map {
            let category = category_votes
                .into_iter()
                .max_by(
                    |(left_category, left_duration), (right_category, right_duration)| {
                        left_duration
                            .cmp(right_duration)
                            .then_with(|| right_category.cmp(left_category))
                    },
                )
                .map(|(category, _)| category)
                .unwrap_or_else(|| "other".to_string());

            if let Some(bucket) = buckets.get_mut(hour as usize) {
                bucket.total_duration += duration;
                bucket.apps.push(AppDuration {
                    app_name,
                    category,
                    duration,
                    screenshot_url,
                });
            }
        }

        for bucket in &mut buckets {
            bucket.apps.sort_by(|left, right| {
                right
                    .duration
                    .cmp(&left.duration)
                    .then_with(|| left.app_name.cmp(&right.app_name))
            });
        }

        Ok(buckets)
    }

    /// 获取日期范围内的原始活动记录
    /// 返回按时间升序排列的明细，用于 session 聚合、意图识别和待办提取
    pub fn get_activities_in_range(
        &self,
        date_from: Option<&str>,
        date_to: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let limit = limit.clamp(1, 10_000) as i64;
        let (start_ts, end_ts) = parse_date_bounds(date_from, date_to);

        let mut stmt = conn.prepare(
            "SELECT id, timestamp, app_name, window_title, screenshot_path, ocr_text, category, duration, browser_url, executable_path, semantic_category, semantic_confidence, screenshot_url
             FROM activities
             WHERE (?1 IS NULL OR timestamp >= ?1)
               AND (?2 IS NULL OR timestamp < ?2)
             ORDER BY timestamp ASC, id ASC
             LIMIT ?3",
        )?;

        let activities = stmt
            .query_map(params![start_ts, end_ts, limit], |row| {
                Ok(Activity {
                    id: Some(row.get(0)?),
                    timestamp: row.get(1)?,
                    app_name: row.get(2)?,
                    window_title: row.get(3)?,
                    screenshot_path: row.get(4)?,
                    ocr_text: row.get(5)?,
                    category: row.get(6)?,
                    duration: row.get(7)?,
                    browser_url: row.get(8)?,
                    executable_path: row.get(9)?,
                    semantic_category: row.get(10)?,
                    semantic_confidence: row.get(11)?,
                    screenshot_url: row.get(12)?,
                })
            })?
            .filter_map(|row| row.ok())
            .collect();

        Ok(activities)
    }

    /// 保存每日报告
    pub fn save_report(&self, report: &DailyReport) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        conn.execute(
            "INSERT OR REPLACE INTO daily_reports_localized (date, locale, content, ai_mode, model_name, fallback_reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                report.date,
                report.locale,
                report.content,
                report.ai_mode,
                report.model_name,
                report.fallback_reason,
                report.created_at,
            ],
        )?;

        Ok(())
    }

    /// 获取每日报告
    pub fn get_report(&self, date: &str, locale: Option<&str>) -> Result<Option<DailyReport>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let locale = locale.unwrap_or("zh-CN");

        let result = conn.query_row(
            "SELECT date, locale, content, ai_mode, model_name, fallback_reason, created_at
             FROM daily_reports_localized
             WHERE date = ?1 AND locale = ?2",
            params![date, locale],
            |row| {
                Ok(DailyReport {
                    date: row.get(0)?,
                    locale: row.get(1)?,
                    content: row.get(2)?,
                    ai_mode: row.get(3)?,
                    model_name: row.get(4)?,
                    fallback_reason: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        );

        match result {
            Ok(report) => Ok(Some(report)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e)),
        }
    }

    /// 列出所有可用日报日期
    pub fn list_report_dates(&self, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT date FROM daily_reports_localized ORDER BY date DESC LIMIT ?1",
        )?;
        let dates: Vec<String> = stmt
            .query_map(params![limit], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(dates)
    }

    /// 按日期范围查询日报（升序）
    ///
    /// `start_date` / `end_date` 用 ISO `YYYY-MM-DD` 字符串比较即可（lexicographic 与日期顺序一致）。
    /// 用于批量导出场景，无匹配返回空 Vec（由调用方决定是否报错）。
    pub fn get_reports_in_range(
        &self,
        start_date: &str,
        end_date: &str,
        locale: Option<&str>,
    ) -> Result<Vec<DailyReport>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let locale = locale.unwrap_or("zh-CN");

        let mut stmt = conn.prepare(
            "SELECT date, locale, content, ai_mode, model_name, fallback_reason, created_at
             FROM daily_reports_localized
             WHERE date >= ?1 AND date <= ?2 AND locale = ?3
             ORDER BY date ASC",
        )?;

        let reports: Vec<DailyReport> = stmt
            .query_map(params![start_date, end_date, locale], |row| {
                Ok(DailyReport {
                    date: row.get(0)?,
                    locale: row.get(1)?,
                    content: row.get(2)?,
                    ai_mode: row.get(3)?,
                    model_name: row.get(4)?,
                    fallback_reason: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(reports)
    }

    /// 保存小时摘要
    pub fn save_hourly_summary(&self, summary: &HourlySummary) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        conn.execute(
            "INSERT OR REPLACE INTO hourly_summaries 
             (date, hour, summary, main_apps, activity_count, total_duration, representative_screenshots, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                summary.date,
                summary.hour,
                summary.summary,
                summary.main_apps,
                summary.activity_count,
                summary.total_duration,
                summary.representative_screenshots,
                summary.created_at,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// 获取指定日期的所有小时摘要
    pub fn get_hourly_summaries(&self, date: &str) -> Result<Vec<HourlySummary>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, date, hour, summary, main_apps, activity_count, total_duration, representative_screenshots, created_at 
             FROM hourly_summaries 
             WHERE date = ?1 
             ORDER BY hour ASC"
        )?;

        let summaries: Vec<HourlySummary> = stmt
            .query_map(params![date], |row| {
                Ok(HourlySummary {
                    id: Some(row.get(0)?),
                    date: row.get(1)?,
                    hour: row.get(2)?,
                    summary: row.get(3)?,
                    main_apps: row.get(4)?,
                    activity_count: row.get(5)?,
                    total_duration: row.get(6)?,
                    representative_screenshots: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(summaries)
    }

    /// 获取指定小时的活动数据（用于生成小时摘要）
    pub fn get_hourly_activities(&self, date: &str, hour: i32) -> Result<Vec<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let date_parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;
        let h = (hour as u32).min(23);
        let (start_ts, end_ts) = local_hour_bounds(date_parsed, h)?;

        let sql = format!(
            "SELECT {ACTIVITY_SELECT_COLUMNS}
             FROM activities
             WHERE timestamp > ?1 AND timestamp - duration < ?2
             ORDER BY timestamp ASC"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut activities: Vec<(i64, Activity)> = stmt
            .query_map(params![start_ts, end_ts], activity_from_row)?
            .filter_map(|r| r.ok())
            .filter_map(|mut activity| {
                let interval_start = activity.timestamp.saturating_sub(activity.duration);
                let overlap_start = interval_start.max(start_ts);
                let overlap_end = activity.timestamp.min(end_ts);

                if overlap_end <= overlap_start {
                    return None;
                }

                activity.duration = calculate_overlap_duration(
                    activity.timestamp,
                    activity.duration,
                    start_ts,
                    end_ts,
                );
                activity.timestamp = overlap_end;

                Some((overlap_start, activity))
            })
            .collect();

        activities.sort_by(
            |(left_start, left_activity), (right_start, right_activity)| {
                left_start
                    .cmp(right_start)
                    .then_with(|| left_activity.timestamp.cmp(&right_activity.timestamp))
                    .then_with(|| left_activity.id.cmp(&right_activity.id))
            },
        );

        Ok(activities
            .into_iter()
            .map(|(_, activity)| activity)
            .collect())
    }

    /// 检查指定小时是否已有摘要
    pub fn has_hourly_summary(&self, date: &str, hour: i32) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM hourly_summaries WHERE date = ?1 AND hour = ?2",
            params![date, hour],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }

    /// 获取历史应用列表（按使用时长排序）
    /// 返回去重后的应用名列表
    pub fn get_recent_apps(&self, limit: u32) -> Result<Vec<String>> {
        Ok(self
            .get_recent_app_usage(limit)?
            .into_iter()
            .map(|item| item.app_name)
            .collect())
    }

    /// 获取历史应用详情（按使用时长排序），包含最近可用的远程截图 URL
    pub fn get_recent_app_usage(&self, limit: u32) -> Result<Vec<AppUsage>> {
        use std::collections::HashMap;

        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        // 聚合下推到 SQL：按原始 app_name 分组求和，避免把全史活动逐行搬到 Rust 侧；
        // executable_path / screenshot_url 用相关子查询取时间最近的非空值，
        // 与旧实现"按 timestamp DESC 取第一个可用值"的语义一致。
        let mut stmt = conn.prepare(
            "SELECT a.app_name,
                    SUM(a.duration),
                    COUNT(*),
                    MAX(a.timestamp),
                    (SELECT e.executable_path FROM activities e
                      WHERE e.app_name = a.app_name AND e.executable_path IS NOT NULL
                      ORDER BY e.timestamp DESC, e.id DESC LIMIT 1),
                    (SELECT s.screenshot_url FROM activities s
                      WHERE s.app_name = a.app_name AND s.screenshot_url IS NOT NULL
                        AND TRIM(s.screenshot_url) <> ''
                      ORDER BY s.timestamp DESC, s.id DESC LIMIT 1)
             FROM activities a
             GROUP BY a.app_name",
        )?;

        let rows: Vec<(String, i64, i64, i64, Option<String>, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Rust 侧只对归一化后同名的应用做二次合并（分组数远小于活动行数）
        let mut merged: HashMap<String, (i64, i64, Option<String>, Option<String>, i64)> =
            HashMap::new();
        for (raw_name, duration, count, latest_ts, executable_path, screenshot_url) in rows {
            let normalized = crate::categorize::normalize_display_app_name(&raw_name);
            let entry = merged
                .entry(normalized)
                .or_insert((0, 0, None, None, i64::MIN));
            entry.0 += duration;
            entry.1 += count;
            if latest_ts >= entry.4 {
                // 更近活跃的分组优先提供 executable_path / screenshot_url
                if executable_path.is_some() {
                    entry.2 = executable_path;
                }
                if screenshot_url.is_some() {
                    entry.3 = screenshot_url;
                }
                entry.4 = latest_ts;
            } else {
                if entry.2.is_none() {
                    entry.2 = executable_path;
                }
                if entry.3.is_none() {
                    entry.3 = screenshot_url;
                }
            }
        }

        let mut apps: Vec<AppUsage> = merged
            .into_iter()
            .map(
                |(app_name, (duration, count, executable_path, screenshot_url, _))| AppUsage {
                    app_name,
                    duration,
                    count,
                    executable_path,
                    screenshot_url,
                },
            )
            .collect();
        apps.sort_by(|a, b| {
            b.duration
                .cmp(&a.duration)
                .then_with(|| a.app_name.cmp(&b.app_name))
        });
        apps.truncate(limit as usize);
        Ok(apps)
    }

    pub fn get_app_category_overview(&self) -> Result<Vec<AppCategorySnapshot>> {
        use std::collections::HashMap;

        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        // 聚合下推到 SQL：category 取每组最新一行的值（SQLite 裸列 + MAX 语义），
        // executable_path / screenshot_url 用相关子查询取时间最近的非空值，与旧实现一致。
        let mut stmt = conn.prepare(
            "SELECT a.app_name,
                    a.category,
                    SUM(a.duration),
                    COUNT(*),
                    MAX(a.timestamp),
                    (SELECT e.executable_path FROM activities e
                      WHERE e.app_name = a.app_name AND e.executable_path IS NOT NULL
                      ORDER BY e.timestamp DESC, e.id DESC LIMIT 1),
                    (SELECT s.screenshot_url FROM activities s
                      WHERE s.app_name = a.app_name AND s.screenshot_url IS NOT NULL
                        AND TRIM(s.screenshot_url) <> ''
                      ORDER BY s.timestamp DESC, s.id DESC LIMIT 1)
             FROM activities a
             GROUP BY a.app_name",
        )?;

        let rows: Vec<(
            String,
            String,
            i64,
            i64,
            i64,
            Option<String>,
            Option<String>,
        )> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .collect();

        // Rust 侧只对归一化后同名（忽略大小写）的应用做二次合并
        let mut merged: HashMap<String, AppCategorySnapshot> = HashMap::new();
        for (
            raw_name,
            category,
            total_duration,
            count,
            latest_timestamp,
            executable_path,
            screenshot_url,
        ) in rows
        {
            let normalized_name = crate::categorize::normalize_display_app_name(&raw_name);
            let key = normalized_name.to_lowercase();

            if let Some(entry) = merged.get_mut(&key) {
                entry.total_duration += total_duration;
                entry.count += count;
                if latest_timestamp >= entry.latest_timestamp {
                    entry.latest_timestamp = latest_timestamp;
                    entry.app_name = normalized_name;
                    entry.category = category;
                    if executable_path.is_some() {
                        entry.executable_path = executable_path;
                    }
                    if screenshot_url.is_some() {
                        entry.screenshot_url = screenshot_url;
                    }
                } else {
                    if entry.executable_path.is_none() {
                        entry.executable_path = executable_path;
                    }
                    if entry.screenshot_url.is_none() {
                        entry.screenshot_url = screenshot_url;
                    }
                }
            } else {
                merged.insert(
                    key,
                    AppCategorySnapshot {
                        app_name: normalized_name,
                        category,
                        total_duration,
                        count,
                        executable_path,
                        latest_timestamp,
                        screenshot_url,
                    },
                );
            }
        }

        let mut overview: Vec<AppCategorySnapshot> = merged.into_values().collect();
        overview.sort_by(|a, b| {
            b.total_duration
                .cmp(&a.total_duration)
                .then_with(|| b.latest_timestamp.cmp(&a.latest_timestamp))
                .then_with(|| a.app_name.cmp(&b.app_name))
        });

        Ok(overview)
    }

    pub fn get_activities_by_normalized_app_name(&self, app_name: &str) -> Result<Vec<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let target = crate::categorize::normalize_display_app_name(app_name).to_lowercase();
        // 用 LIKE 做初步过滤，减少加载量；精确匹配仍在 Rust 侧完成
        let like_pattern = format!("%{app_name}%");
        let sql = format!(
            "SELECT {ACTIVITY_SELECT_COLUMNS}
             FROM activities
             WHERE app_name LIKE ?1
             ORDER BY timestamp ASC, id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;

        let activities = stmt
            .query_map([&like_pattern], activity_from_row)?
            .filter_map(|row| row.ok())
            .filter(|activity| {
                crate::categorize::normalize_display_app_name(&activity.app_name).to_lowercase()
                    == target
            })
            .collect();

        Ok(activities)
    }

    pub fn get_activities_by_domain(&self, domain: &str) -> Result<Vec<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let Some(target) = crate::categorize::normalize_domain_rule(domain) else {
            return Ok(Vec::new());
        };

        // 先流式读取轻量字段，仅在 URL 和标题均无法解析时读取单条 OCR。
        // 不使用域名 LIKE 预筛选，以免标题推断或可疑 URL 兜底的表示形式被遗漏。
        let mut candidates = conn.prepare(
            "SELECT id, app_name, window_title, browser_url,
                    ocr_text IS NOT NULL AND ocr_text != ''
             FROM activities ORDER BY timestamp ASC, id ASC",
        )?;
        let mut ocr_query = conn.prepare("SELECT ocr_text FROM activities WHERE id = ?1")?;
        let mut activity_query = conn.prepare(&format!(
            "SELECT {ACTIVITY_SELECT_COLUMNS} FROM activities WHERE id = ?1"
        ))?;
        let mut rows = candidates.query([])?;
        let mut activities = Vec::new();

        while let Some(row) = rows.next()? {
            let app_name: String = row.get(1)?;
            if !crate::categorize::is_browser_app(&app_name) {
                continue;
            }

            let id: i64 = row.get(0)?;
            let window_title: String = row.get(2)?;
            let browser_url: Option<String> = row.get(3)?;
            let page_hint = match crate::categorize::resolve_browser_page_hint(
                browser_url.as_deref(),
                &window_title,
                None,
            ) {
                Some(page_hint) => Some(page_hint),
                None if row.get::<_, bool>(4)? => {
                    let ocr_text: Option<String> = ocr_query.query_row([id], |row| row.get(0))?;
                    crate::categorize::resolve_browser_page_hint(
                        browser_url.as_deref(),
                        &window_title,
                        ocr_text.as_deref(),
                    )
                }
                None => None,
            };

            if page_hint
                .as_deref()
                .and_then(crate::categorize::normalize_domain_rule)
                .as_deref()
                == Some(target.as_str())
            {
                activities.push(activity_query.query_row([id], activity_from_row)?);
            }
        }

        Ok(activities)
    }

    pub fn update_activity_classification(
        &self,
        id: i64,
        category: &str,
        semantic_category: Option<&str>,
        semantic_confidence: Option<i32>,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        conn.execute(
            "UPDATE activities
             SET category = ?1, semantic_category = ?2, semantic_confidence = ?3
             WHERE id = ?4",
            params![category, semantic_category, semantic_confidence, id],
        )?;

        Ok(())
    }

    /// FTS5 全文检索：将用户查询转为 FTS5 兼容的 OR 查询
    /// 处理中文分词：按空格/标点拆分，每个 token 用 OR 连接
    fn build_fts_query(query: &str) -> String {
        let is_punct = |c: char| -> bool {
            c.is_ascii_punctuation()
                || "，。、？！：；（）【】《》".contains(c)
                || c == '\u{201C}' || c == '\u{201D}' // ""
                || c == '\u{2018}' || c == '\u{2019}' // ''
        };

        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|t| t.trim().trim_matches(is_punct))
            .filter(|t| !t.is_empty() && !t.is_empty())
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect();

        if tokens.is_empty() {
            return format!("\"{}\"", query.trim().replace('"', "\"\""));
        }

        tokens.join(" OR ")
    }

    /// FTS5 搜索活动记录
    fn search_activities_fts(
        &self,
        fts_query: &str,
        start_ts: Option<i64>,
        end_ts: Option<i64>,
        limit: i64,
    ) -> Result<
        Vec<(
            i64,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            i64,
            i64,
        )>,
    > {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut stmt = conn.prepare(
            "SELECT a.id, a.timestamp, a.app_name, a.window_title, a.ocr_text, a.browser_url, a.duration, fts.rank
             FROM activities_fts fts
             JOIN activities a ON a.id = fts.rowid
             WHERE activities_fts MATCH ?1
               AND (?2 IS NULL OR a.timestamp >= ?2)
               AND (?3 IS NULL OR a.timestamp < ?3)
             ORDER BY fts.rank
             LIMIT ?4",
        )?;

        let rows = stmt
            .query_map(params![fts_query, start_ts, end_ts, limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .collect();

        Ok(rows)
    }

    /// FTS5 搜索小时摘要
    fn search_hourly_fts(
        &self,
        fts_query: &str,
        date_from: Option<&str>,
        date_to: Option<&str>,
        limit: i64,
    ) -> Result<Vec<(i64, String, i32, String, String, i64, i64, i64)>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut stmt = conn.prepare(
            "SELECT h.id, h.date, h.hour, h.summary, h.main_apps, h.total_duration, h.created_at, fts.rank
             FROM hourly_summaries_fts fts
             JOIN hourly_summaries h ON h.id = fts.rowid
             WHERE hourly_summaries_fts MATCH ?1
               AND (?2 IS NULL OR h.date >= ?2)
               AND (?3 IS NULL OR h.date <= ?3)
             ORDER BY fts.rank
             LIMIT ?4",
        )?;

        let rows = stmt
            .query_map(params![fts_query, date_from, date_to, limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .collect();

        Ok(rows)
    }

    /// FTS5 搜索日报
    fn search_reports_fts(
        &self,
        fts_query: &str,
        date_from: Option<&str>,
        date_to: Option<&str>,
        limit: i64,
    ) -> Result<Vec<(String, String, String, Option<String>, i64, i64)>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut stmt = conn.prepare(
            "SELECT d.date, d.content, d.ai_mode, d.model_name, d.created_at, fts.rank
             FROM daily_reports_fts fts
             JOIN daily_reports_localized d ON d.rowid = fts.rowid
             WHERE daily_reports_fts MATCH ?1
               AND (?2 IS NULL OR d.date >= ?2)
               AND (?3 IS NULL OR d.date <= ?3)
             GROUP BY d.date
             ORDER BY fts.rank
             LIMIT ?4",
        )?;

        let rows = stmt
            .query_map(params![fts_query, date_from, date_to, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .collect();

        Ok(rows)
    }

    /// 搜索工作记忆
    /// 使用 FTS5 全文检索，支持中英文混合查询。
    /// 回退到关键词匹配当 FTS 无结果时。
    /// 搜索工作记忆
    /// 使用 FTS5 全文检索，支持中英文混合查询。
    /// FTS 无结果时回退到关键词匹配。
    pub fn search_memory(
        &self,
        query: &str,
        date_from: Option<&str>,
        date_to: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemorySearchItem>> {
        let trimmed_query = query.trim();
        if trimmed_query.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.clamp(1, 50);
        let fetch_limit = (limit as i64) * 12;
        let (start_ts, end_ts) = parse_date_bounds(date_from, date_to);
        let report_date_from = date_from.map(|s| s.to_string());
        let report_date_to = date_to.map(|s| s.to_string());
        let fts_query = Self::build_fts_query(trimmed_query);

        let mut items = Vec::new();

        // === FTS5 检索活动记录 ===
        if let Ok(fts_rows) = self.search_activities_fts(&fts_query, start_ts, end_ts, fetch_limit)
        {
            for (id, timestamp, app_name, window_title, ocr_text, browser_url, duration, _rank) in
                fts_rows
            {
                // FTS rank 是负数，越小越好；转换为正数分数
                let score = 200i64; // FTS 匹配即高分

                let date = Local
                    .timestamp_opt(timestamp, 0)
                    .earliest()
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();

                let excerpt = pick_excerpt(&[
                    ocr_text.clone().unwrap_or_default(),
                    browser_url.clone().unwrap_or_default(),
                    window_title.clone(),
                ]);

                items.push(MemorySearchItem {
                    source_type: "activity".to_string(),
                    source_id: Some(id),
                    date,
                    timestamp,
                    title: if window_title.trim().is_empty() {
                        app_name.clone()
                    } else {
                        window_title.clone()
                    },
                    excerpt,
                    app_name: Some(crate::categorize::normalize_display_app_name(&app_name)),
                    browser_url,
                    duration: Some(duration),
                    score,
                });
            }
        }

        // === FTS5 检索小时摘要 ===
        if let Ok(fts_rows) = self.search_hourly_fts(
            &fts_query,
            report_date_from.as_deref(),
            report_date_to.as_deref(),
            fetch_limit,
        ) {
            for (id, date, hour, summary, main_apps, total_duration, created_at, _rank) in fts_rows
            {
                items.push(MemorySearchItem {
                    source_type: "hourly_summary".to_string(),
                    source_id: Some(id),
                    date: date.clone(),
                    timestamp: created_at,
                    title: format!("{date} {hour:02}:00 小时摘要"),
                    excerpt: pick_excerpt(&[summary.clone(), main_apps.clone()]),
                    app_name: None,
                    browser_url: None,
                    duration: Some(total_duration),
                    score: 180,
                });
            }
        }

        // === FTS5 检索日报 ===
        if let Ok(fts_rows) = self.search_reports_fts(
            &fts_query,
            report_date_from.as_deref(),
            report_date_to.as_deref(),
            fetch_limit,
        ) {
            for (date, content, _ai_mode, _model_name, created_at, _rank) in fts_rows {
                items.push(MemorySearchItem {
                    source_type: "daily_report".to_string(),
                    source_id: None,
                    date: date.clone(),
                    timestamp: created_at,
                    title: format!("{date} 日报"),
                    excerpt: pick_excerpt(&[content]),
                    app_name: None,
                    browser_url: None,
                    duration: None,
                    score: 160,
                });
            }
        }

        // === FTS 无结果时回退到关键词匹配 ===
        if items.is_empty() {
            return self.search_memory_fallback(
                trimmed_query,
                start_ts,
                end_ts,
                &report_date_from,
                &report_date_to,
                limit,
            );
        }

        items.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.timestamp.cmp(&a.timestamp))
                .then_with(|| a.title.cmp(&b.title))
        });
        items.truncate(limit);

        Ok(items)
    }

    // ============== AI 工作记忆（自进化洞察）==============

    /// 创建新洞察。返回 id。
    pub fn create_insight(
        &self,
        insight_type: &str,
        content: &str,
        source_date: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.execute(
            "INSERT INTO insights (insight_type, content, confidence, source_date, created_at) VALUES (?1, ?2, 0.6, ?3, ?4)",
            rusqlite::params![insight_type, content, source_date, chrono::Utc::now().timestamp()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    // ══════════════════════════════════════════════════════════
    // 助手会话持久化
    // ══════════════════════════════════════════════════════════

    /// 新建助手会话，返回会话 id。同时清理最旧的超额会话（保留最近 100 个）。
    pub fn create_assistant_conversation(&self, title: &str) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO assistant_conversations (title, created_at, updated_at) VALUES (?1, ?2, ?2)",
            params![title, now],
        )?;
        let id = conn.last_insert_rowid();
        // 超额清理：删除最旧的会话及其消息
        conn.execute(
            "DELETE FROM assistant_messages WHERE conversation_id IN (
                SELECT id FROM assistant_conversations
                ORDER BY updated_at DESC LIMIT -1 OFFSET 100
            )",
            [],
        )?;
        conn.execute(
            "DELETE FROM assistant_conversations WHERE id IN (
                SELECT id FROM assistant_conversations
                ORDER BY updated_at DESC LIMIT -1 OFFSET 100
            )",
            [],
        )?;
        Ok(id)
    }

    /// 会话列表（按最近更新排序），附带消息数。
    pub fn list_assistant_conversations(&self, limit: usize) -> Result<Vec<AssistantConversation>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.title, c.created_at, c.updated_at,
                    (SELECT COUNT(*) FROM assistant_messages m WHERE m.conversation_id = c.id)
             FROM assistant_conversations c
             ORDER BY c.updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(AssistantConversation {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                message_count: row.get::<_, i64>(4)? as usize,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppError::Database)
    }

    /// 读取会话消息（按插入顺序）。
    pub fn get_assistant_messages(
        &self,
        conversation_id: i64,
    ) -> Result<Vec<AssistantStoredMessage>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, tool_digest, model_name, created_at
             FROM assistant_messages WHERE conversation_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            Ok(AssistantStoredMessage {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                tool_digest: row.get(4)?,
                model_name: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppError::Database)
    }

    /// 追加消息并刷新会话 updated_at。返回消息 id。
    pub fn append_assistant_message(
        &self,
        conversation_id: i64,
        role: &str,
        content: &str,
        tool_digest: Option<&str>,
        model_name: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO assistant_messages (conversation_id, role, content, tool_digest, model_name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![conversation_id, role, content, tool_digest, model_name, now],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE assistant_conversations SET updated_at = ?1 WHERE id = ?2",
            params![now, conversation_id],
        )?;
        Ok(id)
    }

    /// 删除会话及其全部消息。
    pub fn delete_assistant_conversation(&self, conversation_id: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.execute(
            "DELETE FROM assistant_messages WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        conn.execute(
            "DELETE FROM assistant_conversations WHERE id = ?1",
            params![conversation_id],
        )?;
        Ok(())
    }

    // ══════════════════════════════════════════════════════════
    // 语义记忆（屏幕级数字记忆）
    // ══════════════════════════════════════════════════════════

    // ══════════════════════════════════════════════════════════
    // 用户显式长期记忆
    // ══════════════════════════════════════════════════════════

    fn validate_user_memory_fields(
        memory_type: &str,
        memory_key: &str,
        value_text: &str,
        recall_policy: &str,
        sensitivity: &str,
    ) -> Result<()> {
        const MEMORY_TYPES: &[&str] = &[
            "preference",
            "workflow",
            "profile",
            "goal",
            "project",
            "constraint",
        ];
        const RECALL_POLICIES: &[&str] = &["always", "relevant", "manual"];
        const SENSITIVITIES: &[&str] = &["normal", "caution"];

        if !MEMORY_TYPES.contains(&memory_type) {
            return Err(AppError::Config(format!(
                "不支持的长期记忆类型: {memory_type}"
            )));
        }
        let memory_key = memory_key.trim();
        if memory_key.is_empty() || value_text.trim().is_empty() {
            return Err(AppError::Config(
                "长期记忆的 key 和内容不能为空".to_string(),
            ));
        }
        if memory_key.chars().count() > 64 {
            return Err(AppError::Config(
                "长期记忆 key 不能超过 64 个字符".to_string(),
            ));
        }
        if !memory_key
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-' | '.'))
        {
            return Err(AppError::Config(
                "长期记忆 key 只能包含字母、数字、下划线、连字符和点".to_string(),
            ));
        }
        if !RECALL_POLICIES.contains(&recall_policy) {
            return Err(AppError::Config(format!(
                "不支持的长期记忆召回策略: {recall_policy}"
            )));
        }
        if !SENSITIVITIES.contains(&sensitivity) {
            return Err(AppError::Config(format!(
                "不支持的长期记忆敏感级别: {sensitivity}"
            )));
        }
        if sensitivity == "caution" && recall_policy == "always" {
            return Err(AppError::Config(
                "谨慎级长期记忆不能设置为 always 召回".to_string(),
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_user_memory(
        &self,
        memory_type: &str,
        memory_key: &str,
        value_text: &str,
        recall_policy: &str,
        sensitivity: &str,
        source_kind: &str,
        source_conversation_id: Option<i64>,
        source_request_id: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<AssistantUserMemory> {
        Self::validate_user_memory_fields(
            memory_type,
            memory_key,
            value_text,
            recall_policy,
            sensitivity,
        )?;
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO assistant_user_memories
                 (memory_type, memory_key, value_text, recall_policy, sensitivity, source_kind,
                  source_conversation_id, source_request_id, revision, expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?10)",
            params![
                memory_type,
                memory_key.trim(),
                value_text.trim(),
                recall_policy,
                sensitivity,
                if source_kind.trim().is_empty() {
                    "explicit_chat"
                } else {
                    source_kind
                },
                source_conversation_id,
                source_request_id,
                expires_at,
                now,
            ],
        )?;
        let id = conn.last_insert_rowid();
        conn.query_row(
            &format!(
                "SELECT {ASSISTANT_USER_MEMORY_COLUMNS} FROM assistant_user_memories WHERE id = ?1"
            ),
            params![id],
            assistant_user_memory_from_row,
        )
        .map_err(AppError::Database)
    }

    pub fn get_user_memory(&self, id: i64) -> Result<Option<AssistantUserMemory>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.query_row(
            &format!(
                "SELECT {ASSISTANT_USER_MEMORY_COLUMNS} FROM assistant_user_memories WHERE id = ?1"
            ),
            params![id],
            assistant_user_memory_from_row,
        )
        .optional()
        .map_err(AppError::Database)
    }

    pub fn list_user_memories(
        &self,
        memory_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AssistantUserMemory>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        let limit = limit.clamp(1, 500) as i64;
        let rows = if let Some(memory_type) = memory_type.filter(|value| !value.trim().is_empty()) {
            let mut stmt = conn.prepare(&format!(
                "SELECT {ASSISTANT_USER_MEMORY_COLUMNS}
                 FROM assistant_user_memories
                 WHERE memory_type = ?1 AND (expires_at IS NULL OR expires_at > ?2)
                 ORDER BY updated_at DESC, id DESC LIMIT ?3"
            ))?;
            let rows = stmt
                .query_map(
                    params![memory_type, now, limit],
                    assistant_user_memory_from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {ASSISTANT_USER_MEMORY_COLUMNS}
                 FROM assistant_user_memories
                 WHERE expires_at IS NULL OR expires_at > ?1
                 ORDER BY updated_at DESC, id DESC LIMIT ?2"
            ))?;
            let rows = stmt
                .query_map(params![now, limit], assistant_user_memory_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        Ok(rows)
    }

    pub fn search_user_memories(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<AssistantUserMemory>> {
        let query = query.trim();
        if query.is_empty() {
            return self.list_user_memories(None, limit);
        }
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        let pattern = format!("%{}%", query.to_lowercase());
        let mut stmt = conn.prepare(&format!(
            "SELECT {ASSISTANT_USER_MEMORY_COLUMNS}
             FROM assistant_user_memories
             WHERE (expires_at IS NULL OR expires_at > ?1)
               AND LOWER(memory_type || ' ' || memory_key || ' ' || value_text) LIKE ?2
             ORDER BY updated_at DESC, id DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(
            params![now, pattern, limit.clamp(1, 100) as i64],
            assistant_user_memory_from_row,
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppError::Database)
    }

    pub fn recall_user_memories(&self, query: &str) -> Result<Vec<AssistantUserMemory>> {
        const MAX_TOTAL: usize = 12;
        const MAX_ALWAYS: i64 = 8;
        const MAX_RELEVANT: i64 = 6;
        const MAX_CHARACTERS: usize = 2400;

        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        let mut candidates = Vec::new();
        {
            let mut stmt = conn.prepare(&format!(
                "SELECT {ASSISTANT_USER_MEMORY_COLUMNS}
                 FROM assistant_user_memories
                 WHERE recall_policy = 'always' AND sensitivity != 'caution'
                   AND (expires_at IS NULL OR expires_at > ?1)
                 ORDER BY updated_at DESC, id DESC LIMIT ?2"
            ))?;
            candidates.extend(
                stmt.query_map(params![now, MAX_ALWAYS], assistant_user_memory_from_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        }

        let query = query.trim();
        if !query.is_empty() {
            let pattern = format!("%{}%", query.to_lowercase());
            let mut stmt = conn.prepare(&format!(
                "SELECT {ASSISTANT_USER_MEMORY_COLUMNS}
                 FROM assistant_user_memories
                 WHERE recall_policy = 'relevant'
                   AND (expires_at IS NULL OR expires_at > ?1)
                   AND LOWER(memory_type || ' ' || memory_key || ' ' || value_text) LIKE ?2
                 ORDER BY updated_at DESC, id DESC LIMIT ?3"
            ))?;
            candidates.extend(
                stmt.query_map(
                    params![now, pattern, MAX_RELEVANT],
                    assistant_user_memory_from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        }

        let mut recalled = Vec::new();
        let mut characters = 0usize;
        for memory in candidates {
            if recalled.len() >= MAX_TOTAL {
                break;
            }
            let memory_characters = memory.memory_key.chars().count()
                + memory.value_text.chars().count()
                + memory.memory_type.chars().count();
            if characters.saturating_add(memory_characters) > MAX_CHARACTERS {
                continue;
            }
            characters += memory_characters;
            recalled.push(memory);
        }
        Ok(recalled)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_user_memory(
        &self,
        id: i64,
        expected_revision: i64,
        memory_type: &str,
        memory_key: &str,
        value_text: &str,
        recall_policy: &str,
        sensitivity: &str,
        expires_at: Option<i64>,
    ) -> Result<AssistantUserMemory> {
        Self::validate_user_memory_fields(
            memory_type,
            memory_key,
            value_text,
            recall_policy,
            sensitivity,
        )?;
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        let updated = conn.execute(
            "UPDATE assistant_user_memories
             SET memory_type = ?1, memory_key = ?2, value_text = ?3, recall_policy = ?4,
                 sensitivity = ?5, expires_at = ?6, revision = revision + 1, updated_at = ?7
             WHERE id = ?8 AND revision = ?9",
            params![
                memory_type,
                memory_key.trim(),
                value_text.trim(),
                recall_policy,
                sensitivity,
                expires_at,
                now,
                id,
                expected_revision,
            ],
        )?;
        if updated == 0 {
            return Err(AppError::Config(
                "长期记忆不存在或 revision 已变化".to_string(),
            ));
        }
        conn.query_row(
            &format!(
                "SELECT {ASSISTANT_USER_MEMORY_COLUMNS} FROM assistant_user_memories WHERE id = ?1"
            ),
            params![id],
            assistant_user_memory_from_row,
        )
        .map_err(AppError::Database)
    }

    pub fn forget_user_memory(&self, id: i64, expected_revision: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let deleted = conn.execute(
            "DELETE FROM assistant_user_memories WHERE id = ?1 AND revision = ?2",
            params![id, expected_revision],
        )?;
        if deleted == 0 {
            return Err(AppError::Config(
                "长期记忆不存在或 revision 已变化".to_string(),
            ));
        }
        Ok(())
    }

    pub fn clear_user_memories(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.execute("DELETE FROM assistant_user_memories", [])
            .map_err(AppError::Database)
    }

    // ══════════════════════════════════════════════════════════
    // 语义记忆（屏幕级数字记忆）
    // ══════════════════════════════════════════════════════════

    fn semantic_memory_state_from_connection(
        conn: &Connection,
    ) -> Result<SemanticMemoryIndexState> {
        conn.query_row(
            "SELECT build_id, embedding_fingerprint, privacy_fingerprint, rebuild_required, status,
                    indexed_activities, total_activities, last_error, updated_at
             FROM semantic_memory_state WHERE singleton_id = 1",
            [],
            |row| {
                Ok(SemanticMemoryIndexState {
                    build_id: row.get(0)?,
                    embedding_fingerprint: row.get(1)?,
                    privacy_fingerprint: row.get(2)?,
                    rebuild_required: row.get::<_, i64>(3)? != 0,
                    status: row.get(4)?,
                    indexed_activities: row.get::<_, i64>(5)?.max(0) as usize,
                    total_activities: row.get::<_, i64>(6)?.max(0) as usize,
                    last_error: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .map_err(AppError::Database)
    }

    pub fn get_semantic_memory_state(&self) -> Result<SemanticMemoryIndexState> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        Self::semantic_memory_state_from_connection(&conn)
    }

    /// 声明一个新的语义重建代际。此阶段用于模型探测，不清空旧块；
    /// 探测失败时可按 build_id 精确记录失败，且不会误伤后来任务。
    pub fn begin_semantic_memory_build(
        &self,
        build_id: &str,
        embedding_fingerprint: &str,
        privacy_fingerprint: &str,
    ) -> Result<SemanticMemoryIndexState> {
        if build_id.trim().is_empty() {
            return Err(AppError::Config("语义重建 build_id 不能为空".to_string()));
        }
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.execute(
            "UPDATE semantic_memory_state
             SET build_id = ?1,
                 embedding_fingerprint = ?2,
                 privacy_fingerprint = ?3,
                 rebuild_required = 1,
                 status = 'building',
                 indexed_activities = 0,
                 total_activities = (SELECT COUNT(*) FROM activities),
                 last_error = NULL,
                 updated_at = ?4
             WHERE singleton_id = 1",
            params![
                build_id,
                embedding_fingerprint,
                privacy_fingerprint,
                chrono::Utc::now().timestamp(),
            ],
        )?;
        Self::semantic_memory_state_from_connection(&conn)
    }

    /// 模型探测成功后激活指定代际，并在同一事务中清空旧语义块。
    pub fn activate_semantic_memory_build(
        &self,
        build_id: &str,
        expected_embedding_fingerprint: &str,
        final_embedding_fingerprint: &str,
        privacy_fingerprint: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let tx = conn.unchecked_transaction()?;
        let updated = tx.execute(
            "UPDATE semantic_memory_state
             SET embedding_fingerprint = ?1,
                 rebuild_required = 1,
                 status = 'building',
                 indexed_activities = 0,
                 total_activities = (SELECT COUNT(*) FROM activities),
                 last_error = NULL,
                 updated_at = ?2
             WHERE singleton_id = 1
               AND build_id = ?3
               AND status = 'building'
               AND embedding_fingerprint = ?4
               AND privacy_fingerprint = ?5",
            params![
                final_embedding_fingerprint,
                chrono::Utc::now().timestamp(),
                build_id,
                expected_embedding_fingerprint,
                privacy_fingerprint,
            ],
        )?;
        if updated == 1 {
            tx.execute("DELETE FROM memory_chunks", [])?;
        }
        tx.commit()?;
        Ok(updated == 1)
    }

    /// 判断数据库是否仍由指定代际、指纹的任务占有。
    pub fn is_semantic_memory_build_current(
        &self,
        build_id: &str,
        embedding_fingerprint: &str,
        privacy_fingerprint: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let active = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM semantic_memory_state
                 WHERE singleton_id = 1
                   AND build_id = ?1
                   AND status = 'building'
                   AND embedding_fingerprint = ?2
                   AND privacy_fingerprint = ?3
             )",
            params![build_id, embedding_fingerprint, privacy_fingerprint],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(active != 0)
    }

    /// 仅将指定代际标记为失败；同指纹的新任务不会被旧任务覆盖。
    pub fn fail_semantic_memory_build(
        &self,
        build_id: &str,
        embedding_fingerprint: &str,
        privacy_fingerprint: &str,
        error: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let updated = conn.execute(
            "UPDATE semantic_memory_state
             SET rebuild_required = 1,
                 status = 'failed',
                 last_error = ?1,
                 updated_at = ?2
             WHERE singleton_id = 1
               AND build_id = ?3
               AND status = 'building'
               AND embedding_fingerprint = ?4
               AND privacy_fingerprint = ?5",
            params![
                error.chars().take(500).collect::<String>(),
                chrono::Utc::now().timestamp(),
                build_id,
                embedding_fingerprint,
                privacy_fingerprint,
            ],
        )?;
        Ok(updated == 1)
    }

    pub fn update_semantic_memory_state(&self, state: &SemanticMemoryIndexState) -> Result<()> {
        if !matches!(
            state.status.as_str(),
            "idle" | "building" | "ready" | "failed"
        ) {
            return Err(AppError::Config(format!(
                "不支持的语义记忆状态: {}",
                state.status
            )));
        }
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.execute(
            "INSERT INTO semantic_memory_state
                 (singleton_id, build_id, embedding_fingerprint, privacy_fingerprint, rebuild_required,
                  status, indexed_activities, total_activities, last_error, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(singleton_id) DO UPDATE SET
                 build_id = excluded.build_id,
                 embedding_fingerprint = excluded.embedding_fingerprint,
                 privacy_fingerprint = excluded.privacy_fingerprint,
                 rebuild_required = excluded.rebuild_required,
                 status = excluded.status,
                 indexed_activities = excluded.indexed_activities,
                 total_activities = excluded.total_activities,
                 last_error = excluded.last_error,
                 updated_at = excluded.updated_at",
            params![
                state.build_id,
                state.embedding_fingerprint,
                state.privacy_fingerprint,
                state.rebuild_required as i64,
                state.status,
                state.indexed_activities as i64,
                state.total_activities as i64,
                state.last_error,
                chrono::Utc::now().timestamp(),
            ],
        )?;
        Ok(())
    }

    /// 仅当数据库仍处于指定的重建任务时更新状态，防止删除或配置变化后的旧任务
    /// 把 `idle/rebuild_required` 覆盖回 `ready` 或 `failed`。
    pub fn update_semantic_memory_state_if_current_build(
        &self,
        expected_build_id: &str,
        expected_embedding_fingerprint: &str,
        expected_privacy_fingerprint: &str,
        state: &SemanticMemoryIndexState,
    ) -> Result<bool> {
        if !matches!(
            state.status.as_str(),
            "idle" | "building" | "ready" | "failed"
        ) {
            return Err(AppError::Config(format!(
                "不支持的语义记忆状态: {}",
                state.status
            )));
        }
        if state.build_id != expected_build_id
            || state.embedding_fingerprint != expected_embedding_fingerprint
            || state.privacy_fingerprint != expected_privacy_fingerprint
        {
            return Err(AppError::Config(
                "语义记忆条件更新的 build_id 或指纹不一致".to_string(),
            ));
        }

        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let updated = conn.execute(
            "UPDATE semantic_memory_state
             SET build_id = ?1,
                 embedding_fingerprint = ?2,
                 privacy_fingerprint = ?3,
                 rebuild_required = ?4,
                 status = ?5,
                 indexed_activities = ?6,
                 total_activities = (SELECT COUNT(*) FROM activities),
                 last_error = ?7,
                 updated_at = ?8
             WHERE singleton_id = 1
               AND build_id = ?9
               AND status = 'building'
               AND embedding_fingerprint = ?10
               AND privacy_fingerprint = ?11
               AND (
                   ?5 <> 'ready'
                   OR (
                       NOT EXISTS (
                           SELECT 1 FROM activities
                           WHERE id > (
                               SELECT COALESCE(MAX(last_activity_id), 0)
                               FROM memory_chunks
                           )
                       )
                       AND NOT EXISTS (
                           SELECT 1 FROM memory_chunks
                           WHERE chunk_key != ?12 AND embedding IS NULL
                       )
                   )
               )",
            params![
                state.build_id,
                state.embedding_fingerprint,
                state.privacy_fingerprint,
                state.rebuild_required as i64,
                state.status,
                state.indexed_activities as i64,
                state.last_error,
                chrono::Utc::now().timestamp(),
                expected_build_id,
                expected_embedding_fingerprint,
                expected_privacy_fingerprint,
                SEMANTIC_MEMORY_CURSOR_CHUNK_KEY,
            ],
        )?;
        Ok(updated == 1)
    }

    fn clear_memory_chunks_and_mark_rebuild_required_on(conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM memory_chunks", [])?;
        conn.execute(
            "INSERT INTO semantic_memory_state
                 (singleton_id, rebuild_required, status, indexed_activities,
                  total_activities, last_error, updated_at)
             VALUES (1, 1, 'idle', 0, (SELECT COUNT(*) FROM activities), NULL, ?1)
             ON CONFLICT(singleton_id) DO UPDATE SET
                 build_id = '',
                 rebuild_required = 1,
                 status = 'idle',
                 indexed_activities = 0,
                 total_activities = (SELECT COUNT(*) FROM activities),
                 last_error = NULL,
                 updated_at = excluded.updated_at",
            params![chrono::Utc::now().timestamp()],
        )?;
        Ok(())
    }

    pub fn clear_memory_chunks_and_mark_rebuild_required(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let tx = conn.unchecked_transaction()?;
        Self::clear_memory_chunks_and_mark_rebuild_required_on(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// 按 id 游标增量读取活动记录（含 OCR 文本），供语义索引消费。
    pub fn get_activities_after_id(&self, after_id: i64, limit: usize) -> Result<Vec<Activity>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, app_name, window_title, screenshot_path, ocr_text,
                    category, duration, browser_url, executable_path,
                    semantic_category, semantic_confidence, screenshot_url
             FROM activities WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after_id, limit as i64], |row| {
            Ok(Activity {
                id: Some(row.get(0)?),
                timestamp: row.get(1)?,
                app_name: row.get(2)?,
                window_title: row.get(3)?,
                screenshot_path: row.get(4)?,
                ocr_text: row.get(5)?,
                category: row.get(6)?,
                duration: row.get(7)?,
                browser_url: row.get(8)?,
                executable_path: row.get(9)?,
                semantic_category: row.get(10)?,
                semantic_confidence: row.get(11)?,
                screenshot_url: row.get(12)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppError::Database)
    }

    /// 判断游标之后是否仍有尚未消费的活动。
    pub fn has_activities_after_id(&self, after_id: i64) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM activities WHERE id > ?1)",
            params![after_id],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(exists != 0)
    }

    /// 写入/更新记忆块。内容变化时清空 embedding（待重嵌入）。
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_memory_chunk(
        &self,
        chunk_key: &str,
        date: &str,
        app_name: &str,
        title: &str,
        browser_url: Option<&str>,
        content: &str,
        last_activity_id: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO memory_chunks
                 (chunk_key, date, app_name, title, browser_url, content, embedding, last_activity_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8)
             ON CONFLICT(chunk_key) DO UPDATE SET
                 last_activity_id = excluded.last_activity_id,
                 updated_at = excluded.updated_at,
                 content = CASE
                     WHEN LENGTH(excluded.content) > LENGTH(memory_chunks.content)
                     THEN excluded.content ELSE memory_chunks.content END,
                 embedding = CASE
                     WHEN LENGTH(excluded.content) > LENGTH(memory_chunks.content)
                     THEN NULL ELSE memory_chunks.embedding END",
            params![chunk_key, date, app_name, title, browser_url, content, last_activity_id, now],
        )?;
        Ok(())
    }

    /// 仅在指定重建代际仍有效、且来源活动仍存在时写入/更新记忆块。
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_memory_chunk_if_current_build(
        &self,
        build_id: &str,
        embedding_fingerprint: &str,
        privacy_fingerprint: &str,
        chunk_key: &str,
        date: &str,
        app_name: &str,
        title: &str,
        browser_url: Option<&str>,
        content: &str,
        last_activity_id: i64,
    ) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let now = chrono::Utc::now().timestamp();
        let updated = conn.execute(
            "INSERT INTO memory_chunks
                 (chunk_key, date, app_name, title, browser_url, content, embedding, last_activity_id, updated_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8
             WHERE EXISTS (
                 SELECT 1 FROM semantic_memory_state
                 WHERE singleton_id = 1
                   AND build_id = ?9
                   AND status = 'building'
                   AND embedding_fingerprint = ?10
                   AND privacy_fingerprint = ?11
             )
               AND (?1 = ?12 OR EXISTS (SELECT 1 FROM activities WHERE id = ?7))
             ON CONFLICT(chunk_key) DO UPDATE SET
                 last_activity_id = excluded.last_activity_id,
                 updated_at = excluded.updated_at,
                 content = CASE
                     WHEN LENGTH(excluded.content) > LENGTH(memory_chunks.content)
                     THEN excluded.content ELSE memory_chunks.content END,
                 embedding = CASE
                     WHEN LENGTH(excluded.content) > LENGTH(memory_chunks.content)
                     THEN NULL ELSE memory_chunks.embedding END",
            params![
                chunk_key,
                date,
                app_name,
                title,
                browser_url,
                content,
                last_activity_id,
                now,
                build_id,
                embedding_fingerprint,
                privacy_fingerprint,
                SEMANTIC_MEMORY_CURSOR_CHUNK_KEY,
            ],
        )?;
        Ok(updated == 1)
    }

    /// 仅返回指定重建代际的待嵌入块；来源活动必须仍然存在。
    pub fn get_unembedded_memory_chunks_if_current_build(
        &self,
        build_id: &str,
        embedding_fingerprint: &str,
        privacy_fingerprint: &str,
        limit: usize,
    ) -> Result<Vec<PendingSemanticMemoryChunk>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT mc.id, mc.content, mc.app_name, mc.title, mc.browser_url, mc.last_activity_id
             FROM memory_chunks mc
             WHERE mc.embedding IS NULL
               AND mc.chunk_key != ?1
               AND EXISTS (SELECT 1 FROM activities a WHERE a.id = mc.last_activity_id)
               AND EXISTS (
                   SELECT 1 FROM semantic_memory_state state
                   WHERE state.singleton_id = 1
                     AND state.build_id = ?2
                     AND state.status = 'building'
                     AND state.embedding_fingerprint = ?3
                     AND state.privacy_fingerprint = ?4
               )
             ORDER BY mc.id ASC LIMIT ?5",
        )?;
        let rows = stmt.query_map(
            params![
                SEMANTIC_MEMORY_CURSOR_CHUNK_KEY,
                build_id,
                embedding_fingerprint,
                privacy_fingerprint,
                limit as i64,
            ],
            |row| {
                Ok(PendingSemanticMemoryChunk {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    app_name: row.get(2)?,
                    title: row.get(3)?,
                    browser_url: row.get(4)?,
                    last_activity_id: row.get(5)?,
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppError::Database)
    }

    /// 取待嵌入的记忆块（id, content）。
    pub fn get_unembedded_memory_chunks(&self, limit: usize) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT id, content FROM memory_chunks
             WHERE embedding IS NULL AND chunk_key != ?1
             ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            params![SEMANTIC_MEMORY_CURSOR_CHUNK_KEY, limit as i64],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppError::Database)
    }

    /// 写入记忆块 embedding（传入前请先 L2 归一化）。
    pub fn set_memory_chunk_embedding(&self, chunk_id: i64, embedding: &[u8]) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        conn.execute(
            "UPDATE memory_chunks SET embedding = ?1 WHERE id = ?2",
            params![embedding, chunk_id],
        )?;
        Ok(())
    }

    /// 仅在语义块内容和当前重建指纹仍匹配时写入嵌入。
    /// 异步嵌入期间索引可能被清空并复用 SQLite 行 ID；同时校验内容可避免旧向量
    /// 落到新一轮重建的不同语义块上。
    pub fn set_memory_chunk_embedding_if_current_build(
        &self,
        expected_build_id: &str,
        chunk_id: i64,
        expected_content: &str,
        embedding: &[u8],
        expected_embedding_fingerprint: &str,
        expected_privacy_fingerprint: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let updated = conn.execute(
            "UPDATE memory_chunks
             SET embedding = ?1
             WHERE id = ?2
               AND content = ?3
               AND embedding IS NULL
               AND chunk_key != ?4
               AND EXISTS (
                   SELECT 1 FROM semantic_memory_state
                   WHERE singleton_id = 1
                     AND build_id = ?5
                     AND status = 'building'
                     AND embedding_fingerprint = ?6
                     AND privacy_fingerprint = ?7
               )",
            params![
                embedding,
                chunk_id,
                expected_content,
                SEMANTIC_MEMORY_CURSOR_CHUNK_KEY,
                expected_build_id,
                expected_embedding_fingerprint,
                expected_privacy_fingerprint,
            ],
        )?;
        Ok(updated == 1)
    }

    /// 索引状态：总块数 / 已嵌入块数 / 活动 id 游标。
    pub fn semantic_memory_stats(&self) -> Result<SemanticMemoryStats> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let (total, embedded, cursor): (i64, i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(CASE WHEN chunk_key != ?1 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN chunk_key != ?1 AND embedding IS NOT NULL THEN 1 ELSE 0 END), 0),
                    COALESCE(MAX(last_activity_id), 0)
             FROM memory_chunks",
            params![SEMANTIC_MEMORY_CURSOR_CHUNK_KEY],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok(SemanticMemoryStats {
            total_chunks: total as usize,
            embedded_chunks: embedded as usize,
            last_indexed_activity_id: cursor,
        })
    }

    /// 语义检索：对全部已嵌入块做流式点积（向量已归一化，点积=余弦），
    /// 边扫边维护 Top-K，不把全部向量载入内存。个人量级（万级块）毫秒-百毫秒级。
    pub fn search_memory_chunks_semantic(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<SemanticMemoryHit>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT id, date, app_name, title, browser_url, content, embedding
             FROM memory_chunks
             WHERE embedding IS NOT NULL AND chunk_key != ?1",
        )?;
        let mut rows = stmt.query(params![SEMANTIC_MEMORY_CURSOR_CHUNK_KEY])?;

        // 小顶堆语义：保留分数最高的 limit 条（用 Vec + 阈值淘汰，K 很小）
        let mut top: Vec<SemanticMemoryHit> = Vec::with_capacity(limit + 1);
        while let Some(row) = rows.next()? {
            let blob: Vec<u8> = row.get(6)?;
            let vector = decode_embedding(&blob);
            if vector.len() != query_embedding.len() {
                continue; // 维度不匹配（换过嵌入模型的旧块），跳过
            }
            let score: f32 = vector
                .iter()
                .zip(query_embedding.iter())
                .map(|(a, b)| a * b)
                .sum();
            let score = f64::from(score);
            if top.len() >= limit && top.last().map(|h| h.score).unwrap_or(f64::MIN) >= score {
                continue;
            }
            let content: String = row.get(5)?;
            let hit = SemanticMemoryHit {
                chunk_id: row.get(0)?,
                date: row.get(1)?,
                app_name: row.get(2)?,
                title: row.get(3)?,
                browser_url: row.get(4)?,
                excerpt: truncate_excerpt(&content, 240),
                score,
            };
            let pos = top
                .binary_search_by(|h| {
                    score
                        .partial_cmp(&h.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or_else(|p| p);
            top.insert(pos, hit);
            if top.len() > limit {
                top.pop();
            }
        }
        Ok(top)
    }

    /// 获取活跃洞察（未归档，confidence > 0.3），按置信度降序。
    pub fn get_active_insights(&self, limit: usize) -> Result<Vec<WorkInsight>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let mut stmt = conn.prepare(
            "SELECT id, insight_type, content, confidence, source_date, created_at, confirmed_count, denied_count, archived
             FROM insights WHERE archived = 0 AND confidence > 0.3
             ORDER BY confidence DESC, created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
            Ok(WorkInsight {
                id: row.get(0)?,
                insight_type: row.get(1)?,
                content: row.get(2)?,
                confidence: row.get(3)?,
                source_date: row.get(4)?,
                created_at: row.get(5)?,
                confirmed_count: row.get(6)?,
                denied_count: row.get(7)?,
                archived: row.get::<_, i32>(8)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppError::Database)
    }

    /// 检查是否存在相似洞察（同类型 + 内容包含关键词），用于去重。
    pub fn has_similar_insight(&self, insight_type: &str, keyword: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM insights WHERE archived = 0 AND insight_type = ?1 AND content LIKE ?2",
            rusqlite::params![insight_type, format!("%{}%", keyword)],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// FTS 无结果时的回退搜索：使用原始关键词匹配
    fn search_memory_fallback(
        &self,
        query: &str,
        start_ts: Option<i64>,
        end_ts: Option<i64>,
        date_from: &Option<String>,
        date_to: &Option<String>,
        limit: usize,
    ) -> Result<Vec<MemorySearchItem>> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let fetch_limit = (limit as i64) * 12;
        let mut items = Vec::new();

        // 回退：activities
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, app_name, window_title, ocr_text, browser_url, duration
             FROM activities
             WHERE (?1 IS NULL OR timestamp >= ?1)
               AND (?2 IS NULL OR timestamp < ?2)
             ORDER BY timestamp DESC
             LIMIT ?3",
        )?;

        let rows: Vec<(
            i64,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            i64,
        )> = stmt
            .query_map(params![start_ts, end_ts, fetch_limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .collect();

        for (id, timestamp, app_name, window_title, ocr_text, browser_url, duration) in rows {
            let score = score_memory_match(
                query,
                &[
                    &app_name,
                    &window_title,
                    ocr_text.as_deref().unwrap_or(""),
                    browser_url.as_deref().unwrap_or(""),
                ],
            );
            if score <= 0 {
                continue;
            }

            let date = Local
                .timestamp_opt(timestamp, 0)
                .earliest()
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_default();

            let excerpt = pick_excerpt(&[
                ocr_text.clone().unwrap_or_default(),
                browser_url.clone().unwrap_or_default(),
                window_title.clone(),
            ]);

            items.push(MemorySearchItem {
                source_type: "activity".to_string(),
                source_id: Some(id),
                date,
                timestamp,
                title: if window_title.trim().is_empty() {
                    app_name.clone()
                } else {
                    window_title.clone()
                },
                excerpt,
                app_name: Some(crate::categorize::normalize_display_app_name(&app_name)),
                browser_url,
                duration: Some(duration),
                score,
            });
        }

        // 回退：hourly_summaries
        let mut hourly_stmt = conn.prepare(
            "SELECT id, date, hour, summary, main_apps, total_duration, created_at
             FROM hourly_summaries
             WHERE (?1 IS NULL OR date >= ?1)
               AND (?2 IS NULL OR date <= ?2)
             ORDER BY created_at DESC
             LIMIT ?3",
        )?;

        let hourly_rows: Vec<(i64, String, i32, String, String, i64, i64)> = hourly_stmt
            .query_map(
                params![date_from.as_deref(), date_to.as_deref(), fetch_limit],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i32>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )?
            .filter_map(|row| row.ok())
            .collect();

        for (id, date, hour, summary, main_apps, total_duration, created_at) in hourly_rows {
            let score =
                score_memory_match(query, &[&summary, &main_apps, &date, &hour.to_string()]);
            if score <= 0 {
                continue;
            }

            items.push(MemorySearchItem {
                source_type: "hourly_summary".to_string(),
                source_id: Some(id),
                date: date.clone(),
                timestamp: created_at,
                title: format!("{date} {hour:02}:00 小时摘要"),
                excerpt: pick_excerpt(&[summary.clone(), main_apps.clone()]),
                app_name: None,
                browser_url: None,
                duration: Some(total_duration),
                score,
            });
        }

        // 回退：daily_reports_localized
        let mut report_stmt = conn.prepare(
            "SELECT date, content, ai_mode, model_name, created_at
             FROM daily_reports_localized
             WHERE (?1 IS NULL OR date >= ?1)
               AND (?2 IS NULL OR date <= ?2)
             ORDER BY created_at DESC
             LIMIT ?3",
        )?;

        let report_rows: Vec<(String, String, String, Option<String>, i64)> = report_stmt
            .query_map(
                params![date_from.as_deref(), date_to.as_deref(), fetch_limit],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )?
            .filter_map(|row| row.ok())
            .collect();

        for (date, content, _ai_mode, _model_name, created_at) in report_rows {
            let score = score_memory_match(query, &[&date, &content]);
            if score <= 0 {
                continue;
            }

            items.push(MemorySearchItem {
                source_type: "daily_report".to_string(),
                source_id: None,
                date: date.clone(),
                timestamp: created_at,
                title: format!("{date} 日报"),
                excerpt: pick_excerpt(&[content]),
                app_name: None,
                browser_url: None,
                duration: None,
                score,
            });
        }

        items.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.timestamp.cmp(&a.timestamp))
                .then_with(|| a.title.cmp(&b.title))
        });
        items.truncate(limit);

        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_flex_overtime_correction, execute_add_column_if_missing, local_day_bounds,
        local_hour_bounds, local_hour_end_timestamp, safe_local_timestamp, Activity, Database,
        SemanticMemoryIndexState,
    };
    use chrono::{TimeZone, Timelike};
    use rusqlite::params;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("work-review-{name}-{unique}.db"))
    }

    fn local_ts(date: &str, hour: u32, minute: u32) -> i64 {
        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").expect("解析日期失败");
        let ndt = date.and_hms_opt(hour, minute, 0).expect("构造本地时间失败");
        safe_local_timestamp(ndt)
    }

    fn in_memory_db() -> Database {
        let db = Database {
            conn: std::sync::Arc::new(std::sync::Mutex::new(
                rusqlite::Connection::open_in_memory().expect("创建内存数据库失败"),
            )),
        };
        db.init_tables().expect("初始化内存数据库失败");
        db
    }

    #[test]
    fn 数据库迁移只忽略重复列而传播其他错误() {
        let conn = rusqlite::Connection::open_in_memory().expect("创建内存数据库失败");
        conn.execute("CREATE TABLE items (id INTEGER)", [])
            .expect("创建测试表失败");

        execute_add_column_if_missing(&conn, "ALTER TABLE items ADD COLUMN label TEXT")
            .expect("首次添加列应成功");
        execute_add_column_if_missing(&conn, "ALTER TABLE items ADD COLUMN label TEXT")
            .expect("重复添加同一列应被安全忽略");
        assert!(execute_add_column_if_missing(
            &conn,
            "ALTER TABLE missing_items ADD COLUMN label TEXT"
        )
        .is_err());
    }

    #[test]
    fn 补传查询应只返回近期缺远程url且有本地截图的记录() {
        let db_path = temp_db_path("backfill");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();

        let mut base = Activity {
            id: None,
            timestamp: now - 10,
            app_name: "Code".to_string(),
            window_title: "缺URL".to_string(),
            screenshot_path: "screenshots/a.jpg".to_string(),
            ocr_text: None,
            category: "development".to_string(),
            duration: 10,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };
        db.insert_activity(&base).expect("插入缺URL记录失败");

        base.window_title = "已有URL".to_string();
        base.screenshot_path = "screenshots/b.jpg".to_string();
        base.screenshot_url = Some("https://cdn.example.com/b.jpg".to_string());
        db.insert_activity(&base).expect("插入已有URL记录失败");

        base.window_title = "无截图".to_string();
        base.screenshot_path = String::new();
        base.screenshot_url = None;
        db.insert_activity(&base).expect("插入无截图记录失败");

        base.window_title = "过旧".to_string();
        base.screenshot_path = "screenshots/c.jpg".to_string();
        base.timestamp = now - 100_000;
        db.insert_activity(&base).expect("插入过旧记录失败");

        let pending = db
            .get_activities_missing_screenshot_url(now - 3600, 10)
            .expect("查询补传记录失败");

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, "screenshots/a.jpg");

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 时间线应使用最新记录详情并累计分组时长() {
        let db_path = temp_db_path("timeline");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();

        let records = vec![
            Activity {
                id: None,
                timestamp: now - 30,
                app_name: "Code".to_string(),
                window_title: "文件A".to_string(),
                screenshot_path: "shot-a.jpg".to_string(),
                ocr_text: Some("old".to_string()),
                category: "development".to_string(),
                duration: 10,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: now - 10,
                app_name: "Code".to_string(),
                window_title: "文件A".to_string(),
                screenshot_path: "shot-b.jpg".to_string(),
                ocr_text: Some("new".to_string()),
                category: "development".to_string(),
                duration: 25,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: Some("https://cdn.example.com/workreview/shot-b.jpg".to_string()),
            },
            Activity {
                id: None,
                timestamp: now - 5,
                app_name: "Code".to_string(),
                window_title: "文件B".to_string(),
                screenshot_path: "shot-c.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 15,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let timeline = db.get_timeline(&date, None, None).expect("读取时间线失败");
        let file_a = timeline
            .iter()
            .find(|activity| activity.window_title == "文件A")
            .expect("未找到文件A记录");
        let file_b = timeline
            .iter()
            .find(|activity| activity.window_title == "文件B")
            .expect("未找到文件B记录");

        assert_eq!(timeline.len(), 2);
        assert_eq!(file_a.duration, 35);
        assert_eq!(file_a.screenshot_path, "shot-b.jpg");
        assert_eq!(
            file_a.screenshot_url.as_deref(),
            Some("https://cdn.example.com/workreview/shot-b.jpg")
        );
        assert_eq!(file_a.ocr_text.as_deref(), Some("new"));
        assert_eq!(file_b.duration, 15);

        let raw_activities = db
            .get_activities_in_range(Some(&date), Some(&date), 100)
            .expect("读取原始活动失败");
        let raw_file_a = raw_activities
            .iter()
            .find(|activity| activity.screenshot_path == "shot-b.jpg")
            .expect("未找到原始文件A记录");
        assert_eq!(
            raw_file_a.screenshot_url.as_deref(),
            Some("https://cdn.example.com/workreview/shot-b.jpg")
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 合并活动应保留原始截图路径() {
        let db_path = temp_db_path("merge-keep-original-screenshot");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();

        let activity = Activity {
            id: None,
            timestamp: now - 30,
            app_name: "Google Chrome".to_string(),
            window_title: "GitHub".to_string(),
            screenshot_path: "shot-a.jpg".to_string(),
            ocr_text: Some("old".to_string()),
            category: "browser".to_string(),
            duration: 10,
            browser_url: Some("https://github.com".to_string()),
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };

        let inserted_id = db.insert_activity(&activity).expect("插入测试数据失败");
        db.merge_activity(inserted_id, 20, Some("new"), "shot-b.jpg", now, None)
            .expect("合并活动失败");

        let merged = db
            .get_activity_by_id(inserted_id)
            .expect("读取活动失败")
            .expect("未读取到活动");

        assert_eq!(merged.screenshot_path, "shot-a.jpg");
        assert_eq!(merged.duration, 30);
        assert_eq!(merged.timestamp, now);
        assert_eq!(merged.ocr_text.as_deref(), Some("old\n---\nnew"));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 最近应用详情应返回最近可用远程截图地址() {
        let db_path = temp_db_path("recent-app-usage-screenshot-url");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();

        let records = vec![
            Activity {
                id: None,
                timestamp: now - 90,
                app_name: "Code".to_string(),
                window_title: "旧窗口".to_string(),
                screenshot_path: "old.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 30,
                browser_url: None,
                executable_path: Some("C:/Code/code.exe".to_string()),
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: Some("https://cdn.example.com/old.jpg".to_string()),
            },
            Activity {
                id: None,
                timestamp: now - 30,
                app_name: "Code".to_string(),
                window_title: "新窗口".to_string(),
                screenshot_path: "new.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 45,
                browser_url: None,
                executable_path: Some("C:/Code/code.exe".to_string()),
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: Some("https://cdn.example.com/new.jpg".to_string()),
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let apps = db.get_recent_app_usage(10).expect("读取最近应用详情失败");
        let code = apps
            .iter()
            .find(|item| item.app_name == "VS Code")
            .expect("未找到 VS Code 应用详情");

        assert_eq!(code.duration, 75);
        assert_eq!(code.count, 2);
        assert_eq!(
            code.screenshot_url.as_deref(),
            Some("https://cdn.example.com/new.jpg")
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 今日统计应合并应用别名避免重复显示() {
        let db_path = temp_db_path("daily-stats-merge");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();

        let records = vec![
            Activity {
                id: None,
                timestamp: now - 60,
                app_name: "work-review".to_string(),
                window_title: "主窗口".to_string(),
                screenshot_path: "wr-a.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 540,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: now - 30,
                app_name: "Work Review".to_string(),
                window_title: "设置".to_string(),
                screenshot_path: "wr-b.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 540,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: now - 10,
                app_name: "Code".to_string(),
                window_title: "main.rs".to_string(),
                screenshot_path: "code.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 300,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(&date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        let work_review = stats
            .app_usage
            .iter()
            .find(|item| item.app_name == "Work Review")
            .expect("未找到 Work Review 聚合结果");

        assert_eq!(work_review.duration, 1080);
        assert_eq!(work_review.count, 2);
        assert_eq!(
            stats
                .app_usage
                .iter()
                .filter(|item| item.app_name == "work-review")
                .count(),
            0
        );
        assert_eq!(stats.app_usage.len(), 2);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 今日统计应输出按小时活跃度分布() {
        let db_path = temp_db_path("daily-stats-hourly-distribution");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let records = vec![
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 30),
                app_name: "Code".to_string(),
                window_title: "main.rs".to_string(),
                screenshot_path: "code-a.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 30 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 11, 10),
                app_name: "Chrome".to_string(),
                window_title: "docs".to_string(),
                screenshot_path: "chrome-a.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 20 * 60,
                browser_url: Some("https://example.com/docs".to_string()),
                executable_path: None,
                semantic_category: Some("资料阅读".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        assert_eq!(stats.hourly_activity_distribution.len(), 24);
        assert_eq!(stats.hourly_activity_distribution[10].hour, 10);
        assert_eq!(stats.hourly_activity_distribution[10].duration, 40 * 60);
        assert_eq!(stats.hourly_activity_distribution[11].hour, 11);
        assert_eq!(stats.hourly_activity_distribution[11].duration, 10 * 60);
        assert!(stats
            .hourly_activity_distribution
            .iter()
            .enumerate()
            .all(|(hour, bucket)| {
                if hour == 10 || hour == 11 {
                    true
                } else {
                    bucket.duration == 0
                }
            }));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 浏览器网站统计应将无法识别页面归入未识别分组() {
        let db_path = temp_db_path("daily-stats-browser-identified-pages-only");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let records = vec![
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 30),
                app_name: "Chrome".to_string(),
                window_title: "Linux Do".to_string(),
                screenshot_path: "chrome-a.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 30 * 60,
                browser_url: Some("linux.dolatest".to_string()),
                executable_path: None,
                semantic_category: Some("资料阅读".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 40),
                app_name: "Chrome".to_string(),
                window_title: "Docs".to_string(),
                screenshot_path: "chrome-b.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 5 * 60,
                browser_url: Some("https://example.com/docs".to_string()),
                executable_path: None,
                semantic_category: Some("资料阅读".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        // 活动总时长仍统计全部浏览器活动。
        assert_eq!(stats.total_duration, 35 * 60);

        // 网站统计应包含未识别页面，保证总时长与页面明细可对齐。
        assert_eq!(stats.browser_duration, 35 * 60);
        assert_eq!(stats.browser_usage.len(), 1);
        assert_eq!(stats.browser_usage[0].browser_name, "Google Chrome");
        assert_eq!(stats.browser_usage[0].duration, 35 * 60);
        assert_eq!(stats.browser_usage[0].domains.len(), 2);
        assert_eq!(stats.browser_usage[0].domains[0].domain, "未识别页面");
        assert_eq!(stats.browser_usage[0].domains[0].duration, 30 * 60);
        assert_eq!(stats.browser_usage[0].domains[1].domain, "example.com");
        assert_eq!(stats.browser_usage[0].domains[1].duration, 5 * 60);
        assert_eq!(
            stats.browser_usage[0].domains[0].urls[0].url,
            "linux.dolatest"
        );
        assert!(stats
            .browser_usage
            .iter()
            .flat_map(|browser| browser.domains.iter())
            .all(|domain| domain.domain != "linux.dolatest"));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 今日统计聚合同名应用时应保留最近可用的执行路径() {
        let db_path = temp_db_path("daily-stats-keep-latest-non-empty-path");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let records = vec![
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 0),
                app_name: "mail".to_string(),
                window_title: "Inbox".to_string(),
                screenshot_path: "mail-a.jpg".to_string(),
                ocr_text: None,
                category: "office".to_string(),
                duration: 10 * 60,
                browser_url: None,
                executable_path: Some("/Applications/Mail.app/Contents/MacOS/Mail".to_string()),
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 15),
                app_name: "邮件".to_string(),
                window_title: "Draft".to_string(),
                screenshot_path: "mail-b.jpg".to_string(),
                ocr_text: None,
                category: "office".to_string(),
                duration: 5 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        let mail = stats
            .app_usage
            .iter()
            .find(|item| item.app_name == "Mail")
            .expect("未找到 Mail 聚合结果");

        assert_eq!(
            mail.executable_path.as_deref(),
            Some("/Applications/Mail.app/Contents/MacOS/Mail")
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 今日统计的单小时活跃度不应因重叠活动而超过真实覆盖时长() {
        let db_path = temp_db_path("daily-stats-hourly-overlap");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let records = vec![
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 40),
                app_name: "Chrome".to_string(),
                window_title: "docs".to_string(),
                screenshot_path: "chrome-a.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 40 * 60,
                browser_url: Some("https://example.com/docs".to_string()),
                executable_path: None,
                semantic_category: Some("资料阅读".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 55),
                app_name: "WeChat".to_string(),
                window_title: "team".to_string(),
                screenshot_path: "wechat-a.jpg".to_string(),
                ocr_text: None,
                category: "communication".to_string(),
                duration: 35 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: Some("即时聊天".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        assert_eq!(stats.hourly_activity_distribution[10].duration, 55 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 单日按小时活跃度每个小时桶不应超过一小时() {
        let db_path = temp_db_path("daily-stats-hourly-bucket-cap");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        for activity in [
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 50),
                app_name: "Chrome".to_string(),
                window_title: "docs".to_string(),
                screenshot_path: "chrome-a.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 50 * 60,
                browser_url: Some("https://example.com/docs".to_string()),
                executable_path: None,
                semantic_category: Some("资料阅读".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 55),
                app_name: "WeChat".to_string(),
                window_title: "team".to_string(),
                screenshot_path: "wechat-a.jpg".to_string(),
                ocr_text: None,
                category: "communication".to_string(),
                duration: 55 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 11, 5),
                app_name: "Code".to_string(),
                window_title: "main.rs".to_string(),
                screenshot_path: "code-a.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 65 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
        ] {
            db.insert_activity(&activity).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");
        let buckets = db
            .get_hourly_app_breakdown(date)
            .expect("读取每小时应用明细失败");

        assert_eq!(stats.hourly_activity_distribution[10].duration, 60 * 60);
        assert_eq!(buckets[10].total_duration, 60 * 60);
        assert!(stats
            .hourly_activity_distribution
            .iter()
            .all(|bucket| bucket.duration <= 60 * 60));
        assert!(buckets
            .iter()
            .all(|bucket| bucket.total_duration <= 60 * 60));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 隐私过滤后的今日统计应保持分类和小时分布同口径() {
        let db_path = temp_db_path("daily-stats-privacy-filtered-consistency");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        for activity in [
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 10),
                app_name: "SecretApp".to_string(),
                window_title: "private".to_string(),
                screenshot_path: "secret.jpg".to_string(),
                ocr_text: None,
                category: "communication".to_string(),
                duration: 10 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 30),
                app_name: "Code".to_string(),
                window_title: "main.rs".to_string(),
                screenshot_path: "code.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 20 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 50),
                app_name: "Chrome".to_string(),
                window_title: "Secret".to_string(),
                screenshot_path: "browser.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 15 * 60,
                browser_url: Some("https://secret.example.com/doc".to_string()),
                executable_path: None,
                semantic_category: Some("资料阅读".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
        ] {
            db.insert_activity(&activity).expect("插入测试数据失败");
        }

        let segments = vec![crate::config::WorkTimeSegment {
            start_hour: 9,
            start_minute: 0,
            end_hour: 18,
            end_minute: 0,
        }];
        let stats = db
            .get_daily_stats_with_segments_filtered(
                date,
                &segments,
                &["secretapp".to_string()],
                &["secret.example.com".to_string()],
            )
            .expect("读取过滤后统计失败");

        assert_eq!(stats.total_duration, 20 * 60);
        assert_eq!(stats.screenshot_count, 1);
        assert_eq!(stats.app_usage.len(), 1);
        assert_eq!(stats.app_usage[0].app_name, "VS Code");
        assert_eq!(stats.category_usage.len(), 1);
        assert_eq!(stats.category_usage[0].category, "development");
        assert_eq!(stats.category_usage[0].duration, 20 * 60);
        assert_eq!(stats.hourly_activity_distribution[10].duration, 20 * 60);
        assert_eq!(stats.browser_duration, 0);
        assert!(stats.domain_usage.is_empty());

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 每日统计应保留完整域名并记录真实域名总数() {
        let db_path = temp_db_path("daily-stats-full-domains");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        for index in 0..12 {
            db.insert_activity(&Activity {
                id: None,
                timestamp: local_ts(date, 10, index + 1),
                app_name: "Chrome".to_string(),
                window_title: format!("site-{index}"),
                screenshot_path: format!("site-{index}.jpg"),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 60,
                browser_url: Some(format!("https://site-{index}.example/page")),
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            })
            .expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        assert_eq!(stats.domain_usage.len(), 12);
        assert_eq!(stats.domain_total_count, 12);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 自定义分类不应在时间分配里被归并到其他() {
        let db_path = temp_db_path("custom-category-not-other");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        db.insert_activity(&Activity {
            id: None,
            timestamp: local_ts(date, 10, 10),
            app_name: "MyApp".to_string(),
            window_title: "work".to_string(),
            screenshot_path: "a.jpg".to_string(),
            ocr_text: None,
            category: "design_custom".to_string(),
            duration: 20 * 60,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        })
        .expect("插入测试数据失败");

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取统计失败");

        // 自定义分类 key 应原样保留，不应被归到 "other"（回归 #109）
        assert!(
            stats
                .category_usage
                .iter()
                .any(|c| c.category == "design_custom"),
            "自定义分类应出现在时间分配里，而不是被吞掉"
        );
        assert!(
            !stats
                .category_usage
                .iter()
                .any(|c| c.category == "other" && c.duration > 0),
            "自定义分类的时长不应被算到 other"
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 每小时应用明细应按小时重叠时长拆分跨小时活动() {
        let db_path = temp_db_path("hourly-app-breakdown-overlap");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        db.insert_activity(&Activity {
            id: None,
            timestamp: local_ts(date, 11, 10),
            app_name: "Code".to_string(),
            window_title: "main.rs".to_string(),
            screenshot_path: "code-a.jpg".to_string(),
            ocr_text: None,
            category: "development".to_string(),
            duration: 40 * 60,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: Some("https://cdn.example.com/code-1110.jpg".to_string()),
        })
        .expect("插入测试数据失败");

        let buckets = db
            .get_hourly_app_breakdown(date)
            .expect("读取每小时应用明细失败");

        assert_eq!(buckets[10].total_duration, 30 * 60);
        assert_eq!(buckets[10].apps[0].app_name, "VS Code");
        assert_eq!(buckets[10].apps[0].duration, 30 * 60);
        assert_eq!(buckets[11].total_duration, 10 * 60);
        assert_eq!(buckets[11].apps[0].app_name, "VS Code");
        assert_eq!(buckets[11].apps[0].duration, 10 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 每小时应用明细范围查询应按小时合并多日数据() {
        let db_path = temp_db_path("hourly-app-breakdown-range");
        let db = Database::new(&db_path).expect("创建测试数据库失败");

        for (date, minutes) in [("2026-03-27", 20), ("2026-03-28", 25)] {
            db.insert_activity(&Activity {
                id: None,
                timestamp: local_ts(date, 10, minutes),
                app_name: "Cursor".to_string(),
                window_title: "main.rs".to_string(),
                screenshot_path: format!("cursor-{date}.jpg"),
                ocr_text: None,
                category: "development".to_string(),
                duration: 10 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            })
            .expect("插入测试数据失败");
        }

        let buckets = db
            .get_hourly_app_breakdown_range("2026-03-27", "2026-03-28")
            .expect("读取范围每小时应用明细失败");

        assert_eq!(buckets[10].total_duration, 20 * 60);
        assert_eq!(buckets[10].apps.len(), 1);
        assert_eq!(buckets[10].apps[0].app_name, "Cursor");
        assert_eq!(buckets[10].apps[0].duration, 20 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 每小时应用明细应在聚合前应用应用与域名隐私过滤() {
        let db_path = temp_db_path("hourly-app-breakdown-privacy");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        for activity in [
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 10),
                app_name: "Chrome".to_string(),
                window_title: "allowed".to_string(),
                screenshot_path: "allowed.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 10 * 60,
                browser_url: Some("https://allowed.example/doc".to_string()),
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 20),
                app_name: "Chrome".to_string(),
                window_title: "private".to_string(),
                screenshot_path: "private.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 10 * 60,
                browser_url: Some("https://private.example/secret".to_string()),
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 30),
                app_name: "SecretApp".to_string(),
                window_title: "ignored".to_string(),
                screenshot_path: "ignored.jpg".to_string(),
                ocr_text: None,
                category: "communication".to_string(),
                duration: 10 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 40),
                app_name: "Code".to_string(),
                window_title: "main.rs".to_string(),
                screenshot_path: "code.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 10 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
        ] {
            db.insert_activity(&activity).expect("插入测试数据失败");
        }

        let buckets = db
            .get_hourly_app_breakdown_range_filtered(
                date,
                date,
                &["secretapp".to_string()],
                &["private.example".to_string()],
            )
            .expect("读取隐私过滤后的每小时应用明细失败");

        assert_eq!(buckets[10].total_duration, 20 * 60);
        assert_eq!(buckets[10].apps.len(), 2);
        assert_eq!(
            buckets[10]
                .apps
                .iter()
                .find(|app| app.app_name == "Google Chrome")
                .map(|app| app.duration),
            Some(10 * 60)
        );
        assert_eq!(
            buckets[10]
                .apps
                .iter()
                .find(|app| app.app_name == "VS Code")
                .map(|app| app.duration),
            Some(10 * 60)
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 每小时应用明细应复用主统计的浏览器页面提示隐私过滤() {
        let db_path = temp_db_path("hourly-app-breakdown-browser-page-hint-privacy");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        for activity in [
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 10),
                app_name: "Chrome".to_string(),
                window_title: "https://allowed.example/doc - Chrome".to_string(),
                screenshot_path: "allowed.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 10 * 60,
                browser_url: Some("https://allowed.example/doc".to_string()),
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 20),
                app_name: "Chrome".to_string(),
                window_title: "https://private.example/from-title - Chrome".to_string(),
                screenshot_path: "private-title.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 10 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 30),
                app_name: "Chrome".to_string(),
                window_title: "页面加载中".to_string(),
                screenshot_path: "private-ocr.jpg".to_string(),
                ocr_text: Some("当前页面 https://private.example/from-ocr".to_string()),
                category: "browser".to_string(),
                duration: 10 * 60,
                browser_url: Some("linux.dolatest".to_string()),
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
        ] {
            db.insert_activity(&activity).expect("插入测试数据失败");
        }

        let excluded_domains = ["private.example".to_string()];
        let stats = db
            .get_daily_stats_with_segments_filtered(date, &[], &[], &excluded_domains)
            .expect("读取隐私过滤后的主统计失败");
        let buckets = db
            .get_hourly_app_breakdown_range_filtered(date, date, &[], &excluded_domains)
            .expect("读取隐私过滤后的每小时应用明细失败");

        assert_eq!(stats.total_duration, 10 * 60);
        assert_eq!(buckets[10].total_duration, stats.total_duration);
        assert_eq!(buckets[10].apps.len(), 1);
        assert_eq!(buckets[10].apps[0].app_name, "Google Chrome");
        assert_eq!(buckets[10].apps[0].duration, 10 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 每小时应用明细不应因重叠活动超过主小时覆盖时长() {
        let db_path = temp_db_path("hourly-app-breakdown-dedup-overlap");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        for activity in [
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 40),
                app_name: "Chrome".to_string(),
                window_title: "docs".to_string(),
                screenshot_path: "chrome-a.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 40 * 60,
                browser_url: Some("https://example.com/docs".to_string()),
                executable_path: None,
                semantic_category: Some("资料阅读".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 55),
                app_name: "WeChat".to_string(),
                window_title: "team".to_string(),
                screenshot_path: "wechat-a.jpg".to_string(),
                ocr_text: None,
                category: "communication".to_string(),
                duration: 35 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: Some("即时聊天".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
        ] {
            db.insert_activity(&activity).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");
        let buckets = db
            .get_hourly_app_breakdown(date)
            .expect("读取每小时应用明细失败");

        assert_eq!(stats.hourly_activity_distribution[10].duration, 55 * 60);
        assert_eq!(buckets[10].total_duration, 55 * 60);
        assert_eq!(
            buckets[10].apps.iter().map(|app| app.duration).sum::<i64>(),
            stats.hourly_activity_distribution[10].duration
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 小时摘要活动应只保留目标小时内的重叠时长() {
        let db_path = temp_db_path("hourly-summary-overlap");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let activity = Activity {
            id: None,
            timestamp: local_ts(date, 11, 10),
            app_name: "Chrome".to_string(),
            window_title: "docs".to_string(),
            screenshot_path: "chrome-a.jpg".to_string(),
            ocr_text: None,
            category: "browser".to_string(),
            duration: 20 * 60,
            browser_url: Some("https://example.com/docs".to_string()),
            executable_path: None,
            semantic_category: Some("资料阅读".to_string()),
            semantic_confidence: Some(80),
            screenshot_url: None,
        };

        db.insert_activity(&activity).expect("插入测试数据失败");

        let hour10 = db
            .get_hourly_activities(date, 10)
            .expect("读取 10 点小时活动失败");
        let hour11 = db
            .get_hourly_activities(date, 11)
            .expect("读取 11 点小时活动失败");

        assert_eq!(hour10.len(), 1);
        assert_eq!(hour10[0].duration, 10 * 60);
        assert_eq!(hour11.len(), 1);
        assert_eq!(hour11[0].duration, 10 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 今日统计应仅累计落在当天窗口内的跨天时长() {
        let db_path = temp_db_path("daily-stats-cross-day-start");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let activity = Activity {
            id: None,
            timestamp: local_ts(date, 0, 10),
            app_name: "Code".to_string(),
            window_title: "night.ts".to_string(),
            screenshot_path: "night.jpg".to_string(),
            ocr_text: None,
            category: "development".to_string(),
            duration: 20 * 60,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };

        db.insert_activity(&activity).expect("插入测试数据失败");

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        assert_eq!(stats.total_duration, 10 * 60);
        assert_eq!(stats.app_usage[0].duration, 10 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 今日统计应纳入跨到次日的重叠时长() {
        let db_path = temp_db_path("daily-stats-cross-day-end");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let activity = Activity {
            id: None,
            timestamp: local_ts("2026-03-28", 0, 10),
            app_name: "Code".to_string(),
            window_title: "late.ts".to_string(),
            screenshot_path: "late.jpg".to_string(),
            ocr_text: None,
            category: "development".to_string(),
            duration: 20 * 60,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };

        db.insert_activity(&activity).expect("插入测试数据失败");

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        assert_eq!(stats.total_duration, 10 * 60);
        assert_eq!(stats.app_usage[0].duration, 10 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 办公时长应仅累计办公时间窗口内的交集() {
        let db_path = temp_db_path("daily-stats-work-window");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let activity = Activity {
            id: None,
            timestamp: local_ts(date, 9, 10),
            app_name: "Code".to_string(),
            window_title: "standup.md".to_string(),
            screenshot_path: "standup.jpg".to_string(),
            ocr_text: None,
            category: "development".to_string(),
            duration: 20 * 60,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };

        db.insert_activity(&activity).expect("插入测试数据失败");

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        assert_eq!(stats.total_duration, 20 * 60);
        assert_eq!(stats.work_time_duration, 10 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 重叠工作段截图计数与加班分类应保持同一口径() {
        let db_path = temp_db_path("daily-stats-overlapping-work-segments");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        for activity in [
            Activity {
                id: None,
                timestamp: local_ts(date, 11, 0),
                app_name: "Code".to_string(),
                window_title: "main.rs".to_string(),
                screenshot_path: "code.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 60 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 18, 30),
                app_name: "Video".to_string(),
                window_title: "休息".to_string(),
                screenshot_path: String::new(),
                ocr_text: None,
                category: "entertainment".to_string(),
                duration: 30 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 19, 0),
                app_name: "Code".to_string(),
                window_title: "review.rs".to_string(),
                screenshot_path: String::new(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 30 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: Some("https://cdn.example.com/review.jpg".to_string()),
            },
        ] {
            db.insert_activity(&activity).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_segments(
                date,
                &[
                    crate::config::WorkTimeSegment {
                        start_hour: 9,
                        start_minute: 0,
                        end_hour: 12,
                        end_minute: 0,
                    },
                    crate::config::WorkTimeSegment {
                        start_hour: 10,
                        start_minute: 0,
                        end_hour: 13,
                        end_minute: 0,
                    },
                ],
            )
            .expect("读取今日统计失败");

        assert_eq!(stats.total_duration, 2 * 60 * 60);
        assert_eq!(stats.work_time_duration, 60 * 60);
        assert_eq!(stats.overtime_duration, 30 * 60);
        assert_eq!(stats.screenshot_count, 2);

        let mut flex_stats = stats.clone();
        apply_flex_overtime_correction(&mut flex_stats, false, 0.5);
        assert_eq!(flex_stats.work_time_duration, 90 * 60);
        assert_eq!(flex_stats.overtime_duration, 60 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 本地自然日边界应落在相邻两天的零点() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let next_date = date.succ_opt().unwrap();
        let (start, end) = local_day_bounds(date).expect("计算自然日边界失败");
        let start_local = chrono::Local.timestamp_opt(start, 0).single().unwrap();
        let end_local = chrono::Local.timestamp_opt(end, 0).single().unwrap();

        assert_eq!(start_local.date_naive(), date);
        assert_eq!(end_local.date_naive(), next_date);
        assert_eq!(start_local.time().num_seconds_from_midnight(), 0);
        assert_eq!(end_local.time().num_seconds_from_midnight(), 0);
    }

    #[test]
    fn 本地小时边界应结束于下一个当地整点() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let (start, end) = local_hour_bounds(date, 1).expect("计算小时边界失败");
        let start_local = chrono::Local.timestamp_opt(start, 0).single().unwrap();
        let end_local = chrono::Local.timestamp_opt(end, 0).single().unwrap();

        assert_eq!(start_local.date_naive(), date);
        assert_eq!(start_local.hour(), 1);
        assert_eq!(end_local.date_naive(), date);
        assert_eq!(end_local.hour(), 2);
        assert!(end > start);
        assert_eq!(local_hour_end_timestamp("2026-11-01", 1), Some(end));
    }

    #[test]
    fn 跨零点办公时长应累计当天两段工作窗口的交集() {
        let db_path = temp_db_path("daily-stats-cross-midnight-work-window");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let records = vec![
            Activity {
                id: None,
                timestamp: local_ts(date, 5, 30),
                app_name: "Code".to_string(),
                window_title: "night-ops.md".to_string(),
                screenshot_path: "night-ops-early.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 120 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 23, 30),
                app_name: "Code".to_string(),
                window_title: "night-ops.md".to_string(),
                screenshot_path: "night-ops-late.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 120 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
        ];

        for record in records {
            db.insert_activity(&record).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 22, 6, 0, 0)
            .expect("读取今日统计失败");

        assert_eq!(stats.total_duration, 240 * 60);
        assert_eq!(stats.work_time_duration, 210 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 娱乐分类不应计入办公时长() {
        let db_path = temp_db_path("daily-stats-ignore-entertainment-work-time");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let date = "2026-03-27";

        let records = vec![
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 0),
                app_name: "Douyin".to_string(),
                window_title: "推荐".to_string(),
                screenshot_path: "douyin.jpg".to_string(),
                ocr_text: None,
                category: "entertainment".to_string(),
                duration: 30 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: Some("休息娱乐".to_string()),
                semantic_confidence: Some(100),
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: local_ts(date, 10, 45),
                app_name: "Code".to_string(),
                window_title: "main.rs".to_string(),
                screenshot_path: "code.jpg".to_string(),
                ocr_text: None,
                category: "development".to_string(),
                duration: 15 * 60,
                browser_url: None,
                executable_path: None,
                semantic_category: Some("编码开发".to_string()),
                semantic_confidence: Some(100),
                screenshot_url: None,
            },
        ];

        for record in &records {
            db.insert_activity(record).expect("插入测试数据失败");
        }

        let stats = db
            .get_daily_stats_with_work_time(date, 9, 18, 0, 0)
            .expect("读取今日统计失败");

        assert_eq!(stats.total_duration, 45 * 60);
        assert_eq!(stats.work_time_duration, 15 * 60);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 时间线应返回最新记录的可执行路径() {
        let db_path = temp_db_path("timeline-executable-path");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();

        let records = vec![
            Activity {
                id: None,
                timestamp: now - 30,
                app_name: "Code".to_string(),
                window_title: "文件A".to_string(),
                screenshot_path: "shot-a.jpg".to_string(),
                ocr_text: Some("old".to_string()),
                category: "development".to_string(),
                duration: 10,
                browser_url: None,
                executable_path: Some(
                    r"C:\Users\wmy\AppData\Local\Programs\Microsoft VS Code\Code.exe".to_string(),
                ),
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: now - 10,
                app_name: "Code".to_string(),
                window_title: "文件A".to_string(),
                screenshot_path: "shot-b.jpg".to_string(),
                ocr_text: Some("new".to_string()),
                category: "development".to_string(),
                duration: 25,
                browser_url: None,
                executable_path: Some(r"D:\Portable\Code\Code.exe".to_string()),
                semantic_category: None,
                semantic_confidence: None,
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let timeline = db.get_timeline(&date, None, None).expect("读取时间线失败");
        let file_a = timeline
            .iter()
            .find(|activity| activity.window_title == "文件A")
            .expect("未找到文件A记录");

        assert_eq!(
            file_a.executable_path.as_deref(),
            Some(r"D:\Portable\Code\Code.exe")
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 活动记录应保留语义分类结果() {
        let db_path = temp_db_path("semantic-category-roundtrip");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();

        let activity = Activity {
            id: None,
            timestamp: now,
            app_name: "Google Chrome".to_string(),
            window_title: "Tauri Guide".to_string(),
            screenshot_path: "guide.jpg".to_string(),
            ocr_text: None,
            category: "browser".to_string(),
            duration: 120,
            browser_url: Some("https://tauri.app/zh-cn/develop/calling-rust/".to_string()),
            semantic_category: Some("资料阅读".to_string()),
            semantic_confidence: Some(86),
            executable_path: None,
            screenshot_url: None,
        };

        db.insert_activity(&activity).expect("插入测试数据失败");

        let latest = db
            .get_last_activity_by_app("Google Chrome")
            .expect("读取活动失败")
            .expect("未读取到活动");

        assert_eq!(latest.semantic_category.as_deref(), Some("资料阅读"));
        assert_eq!(latest.semantic_confidence, Some(86));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 应支持按归一化应用名批量读取并更新历史分类() {
        let db_path = temp_db_path("reclassify-by-normalized-app");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();

        let records = vec![
            Activity {
                id: None,
                timestamp: now - 30,
                app_name: "MuMu".to_string(),
                window_title: "首页".to_string(),
                screenshot_path: "mumu-a.jpg".to_string(),
                ocr_text: None,
                category: "design".to_string(),
                duration: 20,
                browser_url: None,
                executable_path: None,
                semantic_category: Some("设计创作".to_string()),
                semantic_confidence: Some(75),
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: now - 10,
                app_name: "mumu".to_string(),
                window_title: "游戏中心".to_string(),
                screenshot_path: "mumu-b.jpg".to_string(),
                ocr_text: None,
                category: "design".to_string(),
                duration: 30,
                browser_url: None,
                executable_path: None,
                semantic_category: Some("设计创作".to_string()),
                semantic_confidence: Some(70),
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let matched = db
            .get_activities_by_normalized_app_name("MuMu")
            .expect("按应用读取历史活动失败");
        assert_eq!(matched.len(), 2);

        for activity in &matched {
            db.update_activity_classification(
                activity.id.expect("活动应已持久化"),
                "entertainment",
                Some("休息娱乐"),
                Some(88),
            )
            .expect("更新活动分类失败");
        }

        let refreshed = db
            .get_activities_by_normalized_app_name("MuMu")
            .expect("重新读取历史活动失败");

        assert!(refreshed
            .iter()
            .all(|activity| activity.category == "entertainment"));
        assert!(refreshed
            .iter()
            .all(|activity| activity.semantic_category.as_deref() == Some("休息娱乐")));
        assert!(refreshed
            .iter()
            .all(|activity| activity.semantic_confidence == Some(88)));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 应支持按域名批量读取浏览器历史活动() {
        let db_path = temp_db_path("reclassify-by-domain");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let now = chrono::Local::now().timestamp();

        let records = vec![
            Activity {
                id: None,
                timestamp: now - 30,
                app_name: "Google Chrome".to_string(),
                window_title: "GitHub Issues".to_string(),
                screenshot_path: "chrome-a.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 20,
                browser_url: Some("https://github.com/issues/28".to_string()),
                executable_path: None,
                semantic_category: Some("编码开发".to_string()),
                semantic_confidence: Some(82),
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: now - 10,
                app_name: "Arc".to_string(),
                window_title: "Pull Requests".to_string(),
                screenshot_path: "arc-a.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 30,
                browser_url: Some("https://github.com/pulls".to_string()),
                executable_path: None,
                semantic_category: Some("编码开发".to_string()),
                semantic_confidence: Some(80),
                screenshot_url: None,
            },
            Activity {
                id: None,
                timestamp: now - 5,
                app_name: "Google Chrome".to_string(),
                window_title: "Docs".to_string(),
                screenshot_path: "chrome-b.jpg".to_string(),
                ocr_text: None,
                category: "browser".to_string(),
                duration: 15,
                browser_url: Some("https://docs.github.com/en".to_string()),
                executable_path: None,
                semantic_category: Some("资料阅读".to_string()),
                semantic_confidence: Some(76),
                screenshot_url: None,
            },
        ];

        for activity in &records {
            db.insert_activity(activity).expect("插入测试数据失败");
        }

        let matched = db
            .get_activities_by_domain("github.com")
            .expect("按域名读取历史活动失败");
        assert_eq!(matched.len(), 2);
        assert!(matched.iter().all(|activity| {
            activity
                .browser_url
                .as_deref()
                .unwrap_or_default()
                .contains("github.com/")
        }));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 网站历史回填应覆盖推断页面并与展示及工休统计一致() {
        let db = in_memory_db();
        let date = "2026-03-27";
        let cases = [
            (
                "Chrome",
                Some(" https://github.com:443/from-url/ "),
                "other.example - Chrome",
                Some("https://other.example"),
            ),
            (
                "Arc",
                None,
                "github.com - Arc",
                Some("https://other.example"),
            ),
            (
                "Chrome",
                Some(" \t\n "),
                "github.com/from-title - Chrome",
                None,
            ),
            (
                "Safari",
                Some(""),
                "页面加载中",
                Some("https://github.com/from-empty-url"),
            ),
            (
                "Chrome",
                None,
                "页面加载中",
                Some("当前页面 https://github.com/from-ocr"),
            ),
            (
                "Chrome",
                Some("https://linux.dolatest/"),
                "github.com - Chrome",
                None,
            ),
            (
                "Chrome",
                Some("linux.dolatest"),
                "页面加载中",
                Some("https://github.com/from-merged-url"),
            ),
            (
                "Chrome",
                Some("https://other.example/github.com"),
                "github.com - Chrome",
                Some("https://github.com"),
            ),
            (
                "Chrome",
                Some("https://docs.github.com/en"),
                "github.com - Chrome",
                Some("https://github.com"),
            ),
            (
                "Code",
                Some("https://github.com"),
                "github.com",
                Some("https://github.com"),
            ),
            ("Chrome", None, "页面加载中", None),
            (
                "Chrome",
                None,
                "docs.github.com - Chrome",
                Some("https://github.com"),
            ),
        ];
        let mut ids = Vec::new();

        for (index, (app_name, browser_url, title, ocr_text)) in cases.iter().enumerate() {
            let id = db
                .insert_activity(&Activity {
                    id: None,
                    timestamp: local_ts(date, 10, index as u32 + 1),
                    app_name: (*app_name).to_string(),
                    window_title: (*title).to_string(),
                    screenshot_path: format!("domain-{index}.jpg"),
                    ocr_text: ocr_text.map(str::to_string),
                    category: "browser".to_string(),
                    duration: 60,
                    browser_url: browser_url.map(str::to_string),
                    executable_path: None,
                    semantic_category: Some("资料阅读".to_string()),
                    semantic_confidence: Some(70),
                    screenshot_url: None,
                })
                .expect("插入浏览器分类测试数据失败");
            // 保留旧数据库可能存在的空白和尾斜杠，避免被插入接口提前规范化。
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE activities SET browser_url = ?1 WHERE id = ?2",
                    params![browser_url, id],
                )
                .expect("还原历史网址格式失败");
            ids.push(id);
        }

        let matched = db
            .get_activities_by_domain(" HTTPS://GITHUB.COM:443/settings ")
            .expect("读取网站历史失败");
        assert_eq!(
            matched
                .iter()
                .map(|activity| activity.id.unwrap())
                .collect::<Vec<_>>(),
            ids[..7]
        );
        assert!(db.get_activities_by_domain(" ").unwrap().is_empty());

        let before = db.get_daily_stats(date).expect("读取回填前统计失败");
        let domain = before
            .domain_usage
            .iter()
            .find(|item| item.domain == "github.com")
            .expect("推断页面应出现在网站统计中");
        assert_eq!(
            domain.duration,
            matched
                .iter()
                .map(|activity| activity.duration)
                .sum::<i64>()
        );
        assert_eq!(before.work_time_duration, 12 * 60);

        for activity in matched {
            let base_category = crate::categorize::semantic_category_to_base_category(
                "休息娱乐",
                &activity.category,
            );
            db.update_activity_classification(
                activity.id.unwrap(),
                &base_category,
                Some("休息娱乐"),
                Some(100),
            )
            .expect("回填网站分类失败");
        }

        let after = db.get_daily_stats(date).expect("读取回填后统计失败");
        assert_eq!(after.total_duration, before.total_duration);
        assert_eq!(after.work_time_duration, 5 * 60);
        assert_eq!(
            after
                .category_usage
                .iter()
                .find(|item| item.category == "entertainment")
                .map(|item| item.duration),
            Some(7 * 60)
        );
        assert_eq!(
            after
                .domain_usage
                .iter()
                .find(|item| item.domain == "github.com")
                .and_then(|item| item.semantic_category.as_deref()),
            Some("休息娱乐")
        );
        assert!(after
            .browser_usage
            .iter()
            .flat_map(|browser| &browser.domains)
            .filter(|item| item.domain == "github.com")
            .all(|item| item.semantic_category.as_deref() == Some("休息娱乐")));

        let excluded = ["github.com".to_string()];
        let filtered = db
            .get_daily_stats_with_segments_filtered(date, &[], &[], &excluded)
            .expect("读取过滤后的每日统计失败");
        let hours = db
            .get_hourly_app_breakdown_range_filtered(date, date, &[], &excluded)
            .expect("读取过滤后的小时统计失败");
        // 隐私过滤包含子域，但普通应用中的网址仍不参与浏览器网站过滤。
        assert_eq!(filtered.total_duration, 3 * 60);
        assert_eq!(hours[10].total_duration, filtered.total_duration);
    }

    #[test]
    fn 网站历史读取错误不应被当作零条匹配吞掉() {
        let db = in_memory_db();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO activities (timestamp, app_name, window_title, screenshot_path,
             category, duration, browser_url)
             VALUES (1, 'Chrome', 'GitHub', '', X'FF', 60, 'https://github.com')",
                [],
            )
            .expect("插入损坏分类测试数据失败");
        assert!(db.get_activities_by_domain("github.com").is_err());

        let db = in_memory_db();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO activities (timestamp, app_name, window_title, screenshot_path,
             category, duration, ocr_text)
             VALUES (1, 'Chrome', '页面加载中', '', 'browser', 60, X'FF')",
                [],
            )
            .expect("插入损坏 OCR 测试数据失败");
        assert!(db.get_activities_by_domain("github.com").is_err());
    }

    #[test]
    fn 异步ocr应补充网站规则分类并尊重网址标题和应用边界() {
        let db = in_memory_db();
        let rules = [crate::config::WebsiteSemanticRule {
            domain: "github.com".to_string(),
            semantic_category: "休息娱乐".to_string(),
        }];
        let date = "2026-03-27";
        let cases = [
            (
                "Chrome",
                None,
                "页面加载中",
                "https://github.com/page",
                true,
            ),
            (
                "Chrome",
                Some("https://other.example"),
                "github.com - Chrome",
                "https://github.com/page",
                false,
            ),
            (
                "Chrome",
                Some("https://github.com"),
                "other.example - Chrome",
                "https://other.example/page",
                true,
            ),
            (
                "Chrome",
                None,
                "other.example - Chrome",
                "https://github.com/page",
                false,
            ),
            ("Code", None, "页面加载中", "https://github.com/page", false),
            (
                "Chrome",
                Some("linux.dolatest"),
                "页面加载中",
                "https://github.com/page",
                true,
            ),
            (
                "Chrome",
                None,
                "页面加载中",
                "https://other.example/page",
                false,
            ),
        ];

        for (index, (app_name, browser_url, title, ocr_text, should_override)) in
            cases.iter().enumerate()
        {
            let id = db
                .insert_activity(&Activity {
                    id: None,
                    timestamp: local_ts(date, 10, index as u32 + 1),
                    app_name: (*app_name).to_string(),
                    window_title: (*title).to_string(),
                    screenshot_path: format!("ocr-{index}.jpg"),
                    ocr_text: None,
                    category: "browser".to_string(),
                    duration: 60,
                    browser_url: browser_url.map(str::to_string),
                    executable_path: None,
                    semantic_category: Some("资料阅读".to_string()),
                    semantic_confidence: Some(70),
                    screenshot_url: None,
                })
                .expect("插入异步 OCR 测试数据失败");

            db.update_activity_ocr(id, Some((*ocr_text).to_string()), &rules)
                .expect("更新 OCR 和网站分类失败");
            let activity = db
                .conn
                .lock()
                .unwrap()
                .query_row(
                    &format!(
                        "SELECT {} FROM activities WHERE id = ?1",
                        super::ACTIVITY_SELECT_COLUMNS
                    ),
                    [id],
                    super::activity_from_row,
                )
                .expect("读取 OCR 更新后的活动失败");
            assert_eq!(activity.ocr_text.as_deref(), Some(*ocr_text));
            assert_eq!(activity.browser_url.as_deref(), *browser_url);
            assert_eq!(
                activity.category,
                if *should_override {
                    "entertainment"
                } else {
                    "browser"
                }
            );
            assert_eq!(
                activity.semantic_category.as_deref(),
                Some(if *should_override {
                    "休息娱乐"
                } else {
                    "资料阅读"
                })
            );
            assert_eq!(
                activity.semantic_confidence,
                Some(if *should_override { 100 } else { 70 })
            );
        }

        let stats = db
            .get_daily_stats(date)
            .expect("读取 OCR 补分类后的统计失败");
        assert_eq!(stats.total_duration, 7 * 60);
        assert_eq!(stats.work_time_duration, 4 * 60);
    }

    #[test]
    fn 异步ocr分类写入失败应回滚ocr文本() {
        let db = in_memory_db();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO activities (id, timestamp, app_name, window_title, screenshot_path,
             category, duration, ocr_text)
             VALUES (1, 1, 'Chrome', '页面加载中', '', 'browser', 60, '旧文本');
             CREATE TRIGGER reject_classification BEFORE UPDATE OF category ON activities
             BEGIN SELECT RAISE(ABORT, '分类更新失败'); END;",
            )
            .expect("初始化回滚测试数据失败");
        let rules = [crate::config::WebsiteSemanticRule {
            domain: "github.com".to_string(),
            semantic_category: "休息娱乐".to_string(),
        }];

        assert!(db
            .update_activity_ocr(1, Some("https://github.com".to_string()), &rules)
            .is_err());
        let ocr_text: String = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT ocr_text FROM activities WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("读取回滚后的 OCR 失败");
        assert_eq!(ocr_text, "旧文本");
    }

    #[test]
    fn 同一天日报应支持按语言分别保存和读取() {
        let db_path = temp_db_path("report-locale");
        let db = Database::new(&db_path).expect("创建数据库失败");
        let now = chrono::Local::now().timestamp();
        let date = "2026-03-30".to_string();

        db.save_report(&super::DailyReport {
            date: date.clone(),
            locale: "zh-CN".to_string(),
            content: "# 工作日报\n\n中文内容".to_string(),
            ai_mode: "summary".to_string(),
            model_name: Some("gemma3:270m".to_string()),
            fallback_reason: Some("请求失败，已回退到基础模板".to_string()),
            created_at: now,
        })
        .expect("保存中文日报失败");

        db.save_report(&super::DailyReport {
            date: date.clone(),
            locale: "en".to_string(),
            content: "# Daily Report\n\nEnglish content".to_string(),
            ai_mode: "summary".to_string(),
            model_name: Some("gemma3:270m".to_string()),
            fallback_reason: None,
            created_at: now + 1,
        })
        .expect("保存英文日报失败");

        let zh_report = db
            .get_report(&date, Some("zh-CN"))
            .expect("读取中文日报失败")
            .expect("未找到中文日报");
        let en_report = db
            .get_report(&date, Some("en"))
            .expect("读取英文日报失败")
            .expect("未找到英文日报");

        assert_eq!(zh_report.content, "# 工作日报\n\n中文内容");
        assert_eq!(en_report.content, "# Daily Report\n\nEnglish content");
        assert_eq!(
            zh_report.fallback_reason.as_deref(),
            Some("请求失败，已回退到基础模板")
        );
        assert_eq!(en_report.fallback_reason, None);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 数据库备份应保留活动与日报数据() {
        let source_path = temp_db_path("backup-source");
        let backup_path = temp_db_path("backup-target");
        let db = Database::new(&source_path).expect("创建源数据库失败");
        let now = chrono::Local::now().timestamp();
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();

        db.insert_activity(&Activity {
            id: None,
            timestamp: now,
            app_name: "Windows Terminal".to_string(),
            window_title: "npm run tauri dev".to_string(),
            screenshot_path: "term.jpg".to_string(),
            ocr_text: Some("cargo check".to_string()),
            category: "development".to_string(),
            duration: 120,
            browser_url: None,
            executable_path: Some(
                r"C:\Users\wmy\AppData\Local\Microsoft\WindowsApps\wt.exe".to_string(),
            ),
            semantic_category: Some("编码开发".to_string()),
            semantic_confidence: Some(88),
            screenshot_url: None,
        })
        .expect("插入活动失败");

        db.save_report(&super::DailyReport {
            date: date.clone(),
            locale: "zh-CN".to_string(),
            content: "# 工作日报\n\n今天主要在修 bug。".to_string(),
            ai_mode: "summary".to_string(),
            model_name: Some("gpt-4.1".to_string()),
            fallback_reason: Some("返回空内容，已回退到基础模板".to_string()),
            created_at: now,
        })
        .expect("保存日报失败");

        db.backup_to(&backup_path).expect("执行数据库备份失败");

        let restored = Database::new(&backup_path).expect("打开备份数据库失败");
        let activity = restored
            .get_last_activity_by_app("Windows Terminal")
            .expect("读取备份活动失败")
            .expect("备份后未找到活动");
        let report = restored
            .get_report(&date, Some("zh-CN"))
            .expect("读取备份日报失败")
            .expect("备份后未找到日报");

        assert_eq!(activity.window_title, "npm run tauri dev");
        assert_eq!(activity.screenshot_path, "term.jpg");
        assert_eq!(report.content, "# 工作日报\n\n今天主要在修 bug。");
        assert_eq!(
            report.fallback_reason.as_deref(),
            Some("返回空内容，已回退到基础模板")
        );

        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(backup_path);
    }

    fn sample_activity(timestamp: i64, app_name: &str) -> Activity {
        Activity {
            id: None,
            timestamp,
            app_name: app_name.to_string(),
            window_title: "测试活动".to_string(),
            screenshot_path: "test.png".to_string(),
            ocr_text: Some("测试 OCR".to_string()),
            category: "development".to_string(),
            duration: 60,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        }
    }

    fn seed_activity_deletion_state(name: &str) -> (PathBuf, Database, i64, i64, String) {
        let db_path = temp_db_path(name);
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let timestamp = local_ts("2026-08-04", 10, 0);
        let activity_id = db
            .insert_activity(&sample_activity(timestamp, "Code"))
            .expect("插入活动失败");
        db.upsert_memory_chunk(
            "2026-08-04|Code|测试活动",
            "2026-08-04",
            "Code",
            "测试活动",
            None,
            "测试语义内容",
            activity_id,
        )
        .expect("插入语义块失败");
        {
            let conn = db.conn.lock().expect("获取数据库锁失败");
            conn.execute(
                "INSERT INTO daily_reports (date, content, ai_mode, created_at) VALUES (?1, '日报', 'test', ?2)",
                params!["2026-08-04", timestamp],
            )
            .expect("插入日报失败");
            conn.execute(
                "INSERT INTO daily_reports_localized (date, locale, content, ai_mode, created_at) VALUES (?1, 'zh-CN', '日报', 'test', ?2)",
                params!["2026-08-04", timestamp],
            )
            .expect("插入本地化日报失败");
            conn.execute(
                "INSERT INTO hourly_summaries (date, hour, summary, main_apps, activity_count, total_duration, created_at) VALUES (?1, 10, '摘要', 'Code', 1, 60, ?2)",
                params!["2026-08-04", timestamp],
            )
            .expect("插入小时摘要失败");
        }
        (
            db_path,
            db,
            activity_id,
            timestamp,
            "2026-08-04".to_string(),
        )
    }

    fn assert_activity_deletion_invalidated(db: &Database, date: &str) {
        let conn = db.conn.lock().expect("获取数据库锁失败");
        let activity_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM activities", [], |row| row.get(0))
            .expect("查询活动数量失败");
        let chunk_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| row.get(0))
            .expect("查询语义块数量失败");
        let report_count: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM daily_reports WHERE date = ?1) +
                        (SELECT COUNT(*) FROM daily_reports_localized WHERE date = ?1) +
                        (SELECT COUNT(*) FROM hourly_summaries WHERE date = ?1)",
                params![date],
                |row| row.get(0),
            )
            .expect("查询摘要数量失败");
        drop(conn);
        let state = db
            .get_semantic_memory_state()
            .expect("读取语义记忆状态失败");
        assert_eq!(activity_count, 0);
        assert_eq!(chunk_count, 0);
        assert_eq!(report_count, 0);
        assert!(state.rebuild_required);
        assert_eq!(state.status, "idle");
        assert_eq!(state.indexed_activities, 0);
    }

    #[test]
    fn 语义状态与长期记忆核心crud应可用() {
        let db_path = temp_db_path("semantic-user-memory-crud");
        let db = Database::new(&db_path).expect("创建测试数据库失败");

        let initial = db
            .get_semantic_memory_state()
            .expect("读取初始语义状态失败");
        assert!(initial.rebuild_required);
        assert_eq!(initial.status, "idle");

        db.update_semantic_memory_state(&SemanticMemoryIndexState {
            build_id: "ready-build".to_string(),
            embedding_fingerprint: "embedding-v1".to_string(),
            privacy_fingerprint: "privacy-v1".to_string(),
            rebuild_required: false,
            status: "ready".to_string(),
            indexed_activities: 3,
            total_activities: 3,
            last_error: None,
            updated_at: 0,
        })
        .expect("更新语义状态失败");
        let ready = db
            .get_semantic_memory_state()
            .expect("读取更新后的语义状态失败");
        assert_eq!(ready.embedding_fingerprint, "embedding-v1");
        assert!(!ready.rebuild_required);
        assert_eq!(ready.status, "ready");

        let now = chrono::Utc::now().timestamp();
        let created = db
            .create_user_memory(
                "preference",
                "editor_theme",
                "深色主题",
                "always",
                "normal",
                "explicit_chat",
                Some(7),
                Some("request-1"),
                None,
            )
            .expect("创建长期记忆失败");
        assert_eq!(created.revision, 1);
        assert!(db
            .create_user_memory(
                "preference",
                "editor_theme",
                "浅色主题",
                "always",
                "normal",
                "explicit_chat",
                None,
                None,
                None,
            )
            .is_err());

        let updated = db
            .update_user_memory(
                created.id,
                created.revision,
                "preference",
                "editor_theme",
                "深色主题并使用大字号",
                "always",
                "normal",
                None,
            )
            .expect("更新长期记忆失败");
        assert_eq!(updated.revision, 2);
        assert!(db
            .update_user_memory(
                created.id,
                created.revision,
                "preference",
                "editor_theme",
                "旧版本覆盖",
                "always",
                "normal",
                None,
            )
            .is_err());

        db.create_user_memory(
            "project",
            "rust_project",
            "当前项目使用 Rust 和 SQLite",
            "relevant",
            "normal",
            "explicit_chat",
            None,
            None,
            None,
        )
        .expect("创建相关记忆失败");
        db.create_user_memory(
            "goal",
            "expired_goal",
            "已经过期的目标",
            "relevant",
            "normal",
            "explicit_chat",
            None,
            None,
            Some(now - 1),
        )
        .expect("创建过期记忆失败");
        db.create_user_memory(
            "constraint",
            "manual_only",
            "仅手动查询",
            "manual",
            "normal",
            "explicit_chat",
            None,
            None,
            None,
        )
        .expect("创建手动记忆失败");

        let search = db
            .search_user_memories("Rust", 10)
            .expect("搜索长期记忆失败");
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].memory_key, "rust_project");
        let recalled = db.recall_user_memories("Rust").expect("召回长期记忆失败");
        assert!(recalled
            .iter()
            .any(|item| item.memory_key == "editor_theme"));
        assert!(recalled
            .iter()
            .any(|item| item.memory_key == "rust_project"));
        assert!(!recalled
            .iter()
            .any(|item| item.memory_key == "expired_goal"));
        assert!(!recalled.iter().any(|item| item.memory_key == "manual_only"));

        db.forget_user_memory(updated.id, updated.revision)
            .expect("硬删除长期记忆失败");
        assert!(db
            .get_user_memory(updated.id)
            .expect("读取已删除记忆失败")
            .is_none());
        assert_eq!(db.clear_user_memories().expect("清空长期记忆失败"), 3);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 长期记忆键应限制长度和安全字符() {
        let db_path = temp_db_path("user-memory-key-validation");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let invalid_keys = [
            "answer\nstyle",
            "answer\u{0007}style",
            "[/用户确认的长期记忆]",
            "answer style",
            "answer/style",
        ];

        for memory_key in invalid_keys {
            assert!(
                db.create_user_memory(
                    "preference",
                    memory_key,
                    "先给结论",
                    "relevant",
                    "normal",
                    "manual",
                    None,
                    None,
                    None,
                )
                .is_err(),
                "非法长期记忆 key 应被拒绝: {memory_key:?}"
            );
        }

        let oversized_key = "a".repeat(65);
        assert!(db
            .create_user_memory(
                "preference",
                &oversized_key,
                "先给结论",
                "relevant",
                "normal",
                "manual",
                None,
                None,
                None,
            )
            .is_err());

        let valid = db
            .create_user_memory(
                "preference",
                "回答风格.v2",
                "先给结论",
                "relevant",
                "normal",
                "manual",
                None,
                None,
                None,
            )
            .expect("Unicode 字母数字及安全分隔符应可用");
        assert_eq!(valid.memory_key, "回答风格.v2");
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 无过期活动时定期清理不得清空语义索引() {
        let (path, db, _, _, date) = seed_activity_deletion_state("retention-noop");
        assert_eq!(db.delete_activities_before_date(&date).unwrap(), 0);
        let count: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "未删除活动时应保留已建好的语义块");
        assert!(db.get_report(&date, Some("zh-CN")).unwrap().is_some());
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 所有活动删除入口应同时失效摘要与语义索引() {
        let (path, db, _, _, date) = seed_activity_deletion_state("delete-before-date");
        assert_eq!(db.delete_activities_before_date("2026-08-05").unwrap(), 1);
        assert_activity_deletion_invalidated(&db, &date);
        let _ = std::fs::remove_file(path);

        let (path, db, id, _, date) = seed_activity_deletion_state("delete-by-id");
        assert_eq!(db.delete_activity_by_id(id).unwrap().0, 1);
        assert_activity_deletion_invalidated(&db, &date);
        let _ = std::fs::remove_file(path);

        let (path, db, _, _, date) = seed_activity_deletion_state("delete-by-date");
        assert_eq!(db.delete_activities_by_date(&date).unwrap().0, 1);
        assert_activity_deletion_invalidated(&db, &date);
        let _ = std::fs::remove_file(path);

        let (path, db, _, timestamp, date) = seed_activity_deletion_state("delete-by-range");
        assert_eq!(
            db.delete_activities_by_range(timestamp, timestamp + 1)
                .unwrap()
                .0,
            1
        );
        assert_activity_deletion_invalidated(&db, &date);
        let _ = std::fs::remove_file(path);

        let (path, db, _, _, date) = seed_activity_deletion_state("delete-by-app");
        assert_eq!(db.delete_activities_by_app("Code", None).unwrap().0, 1);
        assert_activity_deletion_invalidated(&db, &date);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 删除活动时语义状态更新失败应回滚整笔事务() {
        let (db_path, db, activity_id, _, date) = seed_activity_deletion_state("delete-rollback");
        {
            let conn = db.conn.lock().expect("获取数据库锁失败");
            conn.execute_batch(
                "CREATE TRIGGER block_semantic_state_update
                 BEFORE UPDATE ON semantic_memory_state
                 BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
            )
            .expect("创建失败触发器失败");
        }

        assert!(db.delete_activity_by_id(activity_id).is_err());
        let conn = db.conn.lock().expect("获取数据库锁失败");
        let activity_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM activities", [], |row| row.get(0))
            .unwrap();
        let chunk_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| row.get(0))
            .unwrap();
        let summary_count: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM daily_reports WHERE date = ?1) +
                        (SELECT COUNT(*) FROM daily_reports_localized WHERE date = ?1) +
                        (SELECT COUNT(*) FROM hourly_summaries WHERE date = ?1)",
                params![date],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(activity_count, 1);
        assert_eq!(chunk_count, 1);
        assert_eq!(summary_count, 3);
        drop(conn);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 语义游标占位块不应参与嵌入与块统计() {
        let db_path = temp_db_path("semantic-cursor-sentinel");
        let db = Database::new(&db_path).expect("创建测试数据库失败");

        db.upsert_memory_chunk(
            "normal-chunk",
            "2026-08-05",
            "Code",
            "正常内容",
            None,
            "正常语义内容",
            10,
        )
        .expect("写入正常语义块失败");
        db.upsert_memory_chunk(
            "__cursor__",
            "1970-01-01",
            "__cursor__",
            "__cursor__",
            None,
            "__cursor__",
            20,
        )
        .expect("写入语义游标失败");

        let pending = db
            .get_unembedded_memory_chunks(10)
            .expect("读取待嵌入语义块失败");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, "正常语义内容");

        let stats = db.semantic_memory_stats().expect("读取语义统计失败");
        assert_eq!(stats.total_chunks, 1);
        assert_eq!(stats.embedded_chunks, 0);
        assert_eq!(stats.last_indexed_activity_id, 20);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 旧重建任务不得把嵌入写入复用id的新语义块() {
        let db_path = temp_db_path("semantic-stale-embedding");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let building = SemanticMemoryIndexState {
            build_id: "build-a".to_string(),
            embedding_fingerprint: "embedding-a:3".to_string(),
            privacy_fingerprint: "privacy-a".to_string(),
            rebuild_required: true,
            status: "building".to_string(),
            indexed_activities: 0,
            total_activities: 1,
            last_error: None,
            updated_at: 0,
        };
        db.update_semantic_memory_state(&building)
            .expect("进入语义重建状态失败");
        db.upsert_memory_chunk("old", "2026-08-05", "Code", "旧内容", None, "旧内容", 1)
            .expect("写入旧语义块失败");
        let old_pending = db
            .get_unembedded_memory_chunks(1)
            .expect("读取旧语义块失败");
        let old_id = old_pending[0].0;

        db.clear_memory_chunks_and_mark_rebuild_required()
            .expect("失效旧语义索引失败");
        let new_building = SemanticMemoryIndexState {
            build_id: "build-b".to_string(),
            ..building.clone()
        };
        db.update_semantic_memory_state(&new_building)
            .expect("启动新一轮语义重建失败");
        {
            let conn = db.conn.lock().expect("锁定测试数据库失败");
            conn.execute(
                "INSERT INTO memory_chunks
                     (id, chunk_key, date, app_name, title, browser_url, content,
                      embedding, last_activity_id, updated_at)
                 VALUES (?1, 'new', '2026-08-05', 'Code', '新内容', NULL,
                         '新内容', NULL, 2, 0)",
                params![old_id],
            )
            .expect("以旧行 ID 写入新语义块失败");
        }
        let new_pending = db
            .get_unembedded_memory_chunks(1)
            .expect("读取新语义块失败");
        assert_eq!(
            new_pending[0].0, old_id,
            "测试需要覆盖 SQLite 行 ID 复用场景"
        );

        let written = db
            .set_memory_chunk_embedding_if_current_build(
                "build-a",
                old_id,
                "旧内容",
                &[1, 2, 3],
                "embedding-a:3",
                "privacy-a",
            )
            .expect("条件写入旧嵌入失败");
        assert!(!written);
        assert_eq!(
            db.get_unembedded_memory_chunks(1)
                .expect("复核新语义块失败")[0]
                .1,
            "新内容"
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 新增活动应使就绪语义索引立即失效() {
        let db_path = temp_db_path("semantic-ready-stale-on-insert");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        db.update_semantic_memory_state(&SemanticMemoryIndexState {
            build_id: "ready-build".to_string(),
            embedding_fingerprint: "embedding-a:3".to_string(),
            privacy_fingerprint: "privacy-a".to_string(),
            rebuild_required: false,
            status: "ready".to_string(),
            indexed_activities: 0,
            total_activities: 0,
            last_error: None,
            updated_at: 0,
        })
        .expect("写入就绪状态失败");

        db.insert_activity(&sample_activity(local_ts("2026-08-05", 12, 0), "Code"))
            .expect("插入新活动失败");

        let state = db
            .get_semantic_memory_state()
            .expect("读取新增活动后的语义状态失败");
        assert_ne!(state.status, "ready");
        assert!(state.rebuild_required);
        assert_eq!(state.total_activities, 1);
        assert!(state.build_id.is_empty(), "失效后必须撤销旧构建代际");
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 同指纹新重建必须阻止旧build更新状态和写块() {
        let db_path = temp_db_path("semantic-build-generation");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let activity_id = db
            .insert_activity(&sample_activity(local_ts("2026-08-05", 13, 0), "Code"))
            .expect("插入活动失败");
        db.begin_semantic_memory_build("build-a", "embedding-a:3", "privacy-a")
            .expect("启动旧重建失败");
        db.begin_semantic_memory_build("build-b", "embedding-a:3", "privacy-a")
            .expect("启动新重建失败");

        let stale_upsert = db
            .upsert_memory_chunk_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                "old",
                "2026-08-05",
                "Code",
                "旧内容",
                None,
                "不得写入",
                activity_id,
            )
            .expect("执行旧任务条件写块失败");
        assert!(!stale_upsert);

        let mut stale_ready = db.get_semantic_memory_state().expect("读取新重建状态失败");
        stale_ready.build_id = "build-a".to_string();
        stale_ready.status = "ready".to_string();
        stale_ready.rebuild_required = false;
        let stale_update = db
            .update_semantic_memory_state_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                &stale_ready,
            )
            .expect("执行旧任务条件状态更新失败");
        assert!(!stale_update);

        let current = db
            .get_semantic_memory_state()
            .expect("读取最终语义状态失败");
        assert_eq!(current.build_id, "build-b");
        assert_eq!(current.status, "building");
        let chunk_count: i64 = db
            .conn
            .lock()
            .expect("锁定数据库失败")
            .query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| row.get(0))
            .expect("查询语义块失败");
        assert_eq!(chunk_count, 0);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 旧build失败不得误伤同指纹的新任务() {
        let db_path = temp_db_path("semantic-stale-failure");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        db.begin_semantic_memory_build("build-a", "embedding-a:3", "privacy-a")
            .expect("启动旧重建失败");
        db.begin_semantic_memory_build("build-b", "embedding-a:3", "privacy-a")
            .expect("启动新重建失败");

        let failed = db
            .fail_semantic_memory_build("build-a", "embedding-a:3", "privacy-a", "旧任务失败")
            .expect("记录旧任务失败状态失败");
        assert!(!failed);
        let current = db.get_semantic_memory_state().expect("读取新任务状态失败");
        assert_eq!(current.build_id, "build-b");
        assert_eq!(current.status, "building");
        assert_eq!(current.last_error, None);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 已失效重建不得被旧任务覆盖为就绪() {
        let db_path = temp_db_path("semantic-stale-state");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let building = SemanticMemoryIndexState {
            build_id: "build-a".to_string(),
            embedding_fingerprint: "embedding-a:3".to_string(),
            privacy_fingerprint: "privacy-a".to_string(),
            rebuild_required: true,
            status: "building".to_string(),
            indexed_activities: 0,
            total_activities: 1,
            last_error: None,
            updated_at: 0,
        };
        db.update_semantic_memory_state(&building)
            .expect("进入语义重建状态失败");
        db.clear_memory_chunks_and_mark_rebuild_required()
            .expect("失效语义索引失败");

        let ready = SemanticMemoryIndexState {
            rebuild_required: false,
            status: "ready".to_string(),
            indexed_activities: 1,
            ..building
        };
        let updated = db
            .update_semantic_memory_state_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                &ready,
            )
            .expect("条件更新语义状态失败");
        assert!(!updated);
        let current = db
            .get_semantic_memory_state()
            .expect("读取失效后的语义状态失败");
        assert_eq!(current.status, "idle");
        assert!(current.rebuild_required);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn building期间新增活动不得被旧进度原子标记为ready() {
        let db_path = temp_db_path("semantic-ready-race-with-new-activity");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let mut stale_ready = db
            .begin_semantic_memory_build("build-a", "embedding-a:3", "privacy-a")
            .expect("启动语义重建失败");

        db.insert_activity(&sample_activity(local_ts("2026-08-05", 14, 0), "Code"))
            .expect("重建期间插入新活动失败");
        stale_ready.status = "ready".to_string();
        stale_ready.rebuild_required = false;
        stale_ready.total_activities = 0;

        let updated = db
            .update_semantic_memory_state_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                &stale_ready,
            )
            .expect("尝试提交旧完成进度失败");
        assert!(!updated, "游标之后仍有活动时不得进入 ready");
        let current = db
            .get_semantic_memory_state()
            .expect("读取并发插入后的语义状态失败");
        assert_eq!(current.status, "building");
        assert!(current.rebuild_required);
        assert_eq!(current.total_activities, 1);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 未嵌入块存在时不得原子标记为ready() {
        let db_path = temp_db_path("semantic-ready-with-pending-embedding");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let activity_id = db
            .insert_activity(&sample_activity(local_ts("2026-08-05", 14, 30), "Code"))
            .expect("插入活动失败");
        let mut ready = db
            .begin_semantic_memory_build("build-a", "embedding-a:3", "privacy-a")
            .expect("启动语义重建失败");
        assert!(db
            .upsert_memory_chunk_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                "chunk-a",
                "2026-08-05",
                "Code",
                "待嵌入内容",
                None,
                "待嵌入内容",
                activity_id,
            )
            .expect("写入待嵌入块失败"));
        ready.status = "ready".to_string();
        ready.rebuild_required = false;
        ready.indexed_activities = 1;

        let updated = db
            .update_semantic_memory_state_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                &ready,
            )
            .expect("尝试在待嵌入状态完成重建失败");
        assert!(!updated, "存在未嵌入真实块时不得进入 ready");
        assert_eq!(
            db.get_semantic_memory_state()
                .expect("读取待嵌入状态失败")
                .status,
            "building"
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn ready状态总活动数必须使用数据库实时计数() {
        let db_path = temp_db_path("semantic-ready-live-total");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let mut ready = db
            .begin_semantic_memory_build("build-a", "embedding-a:3", "privacy-a")
            .expect("启动语义重建失败");
        ready.status = "ready".to_string();
        ready.rebuild_required = false;
        ready.total_activities = 999;

        assert!(db
            .update_semantic_memory_state_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                &ready,
            )
            .expect("完成空索引重建失败"));
        let current = db
            .get_semantic_memory_state()
            .expect("读取完成后的语义状态失败");
        assert_eq!(current.status, "ready");
        assert_eq!(current.total_activities, 0);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 来源活动删除后当前build也不得写入语义块() {
        let db_path = temp_db_path("semantic-source-deleted-before-upsert");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let activity_id = db
            .insert_activity(&sample_activity(local_ts("2026-08-05", 15, 0), "Code"))
            .expect("插入来源活动失败");
        db.begin_semantic_memory_build("build-a", "embedding-a:3", "privacy-a")
            .expect("启动语义重建失败");
        db.conn
            .lock()
            .expect("锁定测试数据库失败")
            .execute("DELETE FROM activities WHERE id = ?1", params![activity_id])
            .expect("删除来源活动失败");

        let written = db
            .upsert_memory_chunk_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                "chunk-a",
                "2026-08-05",
                "Code",
                "已删除来源",
                None,
                "不得落库",
                activity_id,
            )
            .expect("执行来源校验写块失败");
        assert!(!written);
        assert_eq!(
            db.conn
                .lock()
                .expect("锁定测试数据库失败")
                .query_row("SELECT COUNT(*) FROM memory_chunks", [], |row| row
                    .get::<_, i64>(0))
                .expect("查询语义块数量失败"),
            0
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 旧build不得读取新任务占用期间的待嵌入块() {
        let db_path = temp_db_path("semantic-stale-pending-read");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let activity_id = db
            .insert_activity(&sample_activity(local_ts("2026-08-05", 15, 30), "Code"))
            .expect("插入来源活动失败");
        db.begin_semantic_memory_build("build-a", "embedding-a:3", "privacy-a")
            .expect("启动旧语义重建失败");
        assert!(db
            .upsert_memory_chunk_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                "chunk-a",
                "2026-08-05",
                "Code",
                "待嵌入",
                None,
                "待嵌入",
                activity_id,
            )
            .expect("写入待嵌入块失败"));
        db.begin_semantic_memory_build("build-b", "embedding-a:3", "privacy-a")
            .expect("启动新语义重建失败");

        let stale_pending = db
            .get_unembedded_memory_chunks_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                10,
            )
            .expect("旧任务读取待嵌入块失败");
        assert!(stale_pending.is_empty());
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 当前build失败应可靠记录错误() {
        let db_path = temp_db_path("semantic-current-build-failure");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        db.begin_semantic_memory_build("build-a", "embedding-probe", "privacy-a")
            .expect("启动语义探测失败");

        assert!(db
            .fail_semantic_memory_build("build-a", "embedding-probe", "privacy-a", "模型探测失败",)
            .expect("记录当前任务失败状态失败"));
        let current = db.get_semantic_memory_state().expect("读取失败状态失败");
        assert_eq!(current.build_id, "build-a");
        assert_eq!(current.status, "failed");
        assert!(current.rebuild_required);
        assert_eq!(current.last_error.as_deref(), Some("模型探测失败"));
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn 活动追平且全部嵌入后当前build应可标记为ready() {
        let db_path = temp_db_path("semantic-ready-after-embedding");
        let db = Database::new(&db_path).expect("创建测试数据库失败");
        let activity_id = db
            .insert_activity(&sample_activity(local_ts("2026-08-05", 16, 0), "Code"))
            .expect("插入来源活动失败");
        let mut ready = db
            .begin_semantic_memory_build("build-a", "embedding-a:3", "privacy-a")
            .expect("启动语义重建失败");
        assert!(db
            .upsert_memory_chunk_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                "chunk-a",
                "2026-08-05",
                "Code",
                "已嵌入",
                None,
                "已嵌入",
                activity_id,
            )
            .expect("写入语义块失败"));
        let pending = db
            .get_unembedded_memory_chunks_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                1,
            )
            .expect("读取待嵌入块失败");
        assert_eq!(pending.len(), 1);
        assert!(db
            .set_memory_chunk_embedding_if_current_build(
                "build-a",
                pending[0].id,
                &pending[0].content,
                &[1, 2, 3],
                "embedding-a:3",
                "privacy-a",
            )
            .expect("写入嵌入失败"));
        ready.status = "ready".to_string();
        ready.rebuild_required = false;
        ready.indexed_activities = 1;

        assert!(db
            .update_semantic_memory_state_if_current_build(
                "build-a",
                "embedding-a:3",
                "privacy-a",
                &ready,
            )
            .expect("完成语义重建失败"));
        let current = db.get_semantic_memory_state().expect("读取就绪状态失败");
        assert_eq!(current.status, "ready");
        assert!(!current.rebuild_required);
        assert_eq!(current.total_activities, 1);
        let _ = std::fs::remove_file(db_path);
    }
}
