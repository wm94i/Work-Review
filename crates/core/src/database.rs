use crate::error::{AppError, Result};
use chrono::{Local, MappedLocalTime, NaiveDateTime, TimeZone};
use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

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

fn category_counts_toward_work_time(category: &str) -> bool {
    crate::categorize::normalize_category_key(category) != "entertainment"
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
    /// 按浏览器分组的使用统计
    pub browser_usage: Vec<BrowserUsage>,
    /// 工作时间内的活动时长（新增）
    #[serde(default)]
    pub work_time_duration: i64,
    /// 24 小时活跃度分布
    #[serde(default)]
    pub hourly_activity_distribution: Vec<HourlyActivityBucket>,
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

fn calculate_work_time_overlap_duration(
    interval_end: i64,
    duration: i64,
    day_start: i64,
    day_end: i64,
    work_start: i64,
    work_end: i64,
) -> i64 {
    if work_start == work_end {
        0
    } else if work_end > work_start {
        calculate_overlap_duration(interval_end, duration, work_start, work_end)
    } else {
        calculate_overlap_duration(interval_end, duration, day_start, work_end)
            + calculate_overlap_duration(interval_end, duration, work_start, day_end)
    }
}

/// 多段工作时间重叠计算
fn calculate_work_time_segments_overlap(
    interval_end: i64,
    duration: i64,
    day_start: i64,
    day_end: i64,
    segments: &[(i64, i64)],
) -> i64 {
    segments
        .iter()
        .map(|&(ws, we)| {
            calculate_work_time_overlap_duration(interval_end, duration, day_start, day_end, ws, we)
        })
        .sum()
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
    let text = value.trim().replace('\n', " ").replace('\r', " ");
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

/// 数据库管理器
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// 创建新的数据库连接
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;
        let db = Self {
            conn: Mutex::new(conn),
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

        let _ = conn.execute(
            "ALTER TABLE daily_reports ADD COLUMN fallback_reason TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE daily_reports_localized ADD COLUMN fallback_reason TEXT",
            [],
        );

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

        // 迁移：添加 browser_url 列（如果不存在）
        let _ = conn.execute("ALTER TABLE activities ADD COLUMN browser_url TEXT", []);
        // 迁移：添加 executable_path 列（如果不存在）
        let _ = conn.execute("ALTER TABLE activities ADD COLUMN executable_path TEXT", []);
        // 迁移：添加 semantic_category 列（如果不存在）
        let _ = conn.execute(
            "ALTER TABLE activities ADD COLUMN semantic_category TEXT",
            [],
        );
        // 迁移：添加 semantic_confidence 列（如果不存在）
        let _ = conn.execute(
            "ALTER TABLE activities ADD COLUMN semantic_confidence INTEGER",
            [],
        );
        // 迁移：添加 screenshot_url 列（远程存储 URL）
        let _ = conn.execute(
            "ALTER TABLE activities ADD COLUMN screenshot_url TEXT",
            [],
        );

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

    /// 插入活动记录
    pub fn insert_activity(&self, activity: &Activity) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let normalized_browser_url = activity
            .browser_url
            .as_deref()
            .map(normalize_url)
            .filter(|url| !url.is_empty());

        conn.execute(
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

        Ok(conn.last_insert_rowid())
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

        // 使用 RTRIM 规范化数据库中的 URL 进行比较
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

    /// 合并活动：累加时长、追加OCR、更新截图路径
    pub fn merge_activity(
        &self,
        id: i64,
        duration_delta: i64,
        new_ocr: Option<&str>,
        _new_screenshot_path: &str,
        new_timestamp: i64,
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

        conn.execute(
            "UPDATE activities 
             SET duration = duration + ?1, 
                 ocr_text = ?2, 
                 timestamp = ?3
             WHERE id = ?4",
            params![duration_delta, merged_ocr, new_timestamp, id],
        )?;

        Ok(())
    }

    /// 精确增加活动时长（用于事件驱动时长计算）
    /// 当检测到应用切换时，将上一个应用的实际使用时长累加到其记录
    pub fn add_duration(&self, id: i64, duration_delta: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        conn.execute(
            "UPDATE activities SET duration = duration + ?1 WHERE id = ?2",
            params![duration_delta, id],
        )?;

        log::debug!("精确时长累加: id={id}, +{duration_delta}秒");
        Ok(())
    }

    /// 更新活动的 OCR 文本
    pub fn update_activity_ocr(&self, id: i64, ocr_text: Option<String>) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        conn.execute(
            "UPDATE activities SET ocr_text = ?1 WHERE id = ?2",
            params![ocr_text, id],
        )?;

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

    /// 删除指定应用在指定时间之后的旧记录（保留 keep_id），返回删除数量和截图路径
    pub fn delete_old_activities_by_app(
        &self,
        app_name: &str,
        keep_id: i64,
        since_timestamp: i64,
    ) -> Result<(usize, Vec<String>)> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        // 先获取要删除的记录的截图路径
        let mut stmt = conn.prepare(
            "SELECT screenshot_path FROM activities 
             WHERE app_name = ?1 AND id != ?2 AND timestamp >= ?3",
        )?;

        let paths: Vec<String> = stmt
            .query_map(params![app_name, keep_id, since_timestamp], |row| {
                row.get::<_, String>(0)
            })?
            .filter_map(|r| r.ok())
            .filter(|p| !p.is_empty())
            .collect();

        // 删除旧记录
        let deleted = conn.execute(
            "DELETE FROM activities 
             WHERE app_name = ?1 AND id != ?2 AND timestamp >= ?3",
            params![app_name, keep_id, since_timestamp],
        )?;
        Ok((deleted, paths))
    }

    /// 删除指定日期之前的所有活动记录（使用时间戳范围查询以利用索引）
    pub fn delete_activities_before_date(&self, before_date: &str) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| {
            crate::error::AppError::Unknown(format!("数据库锁获取失败: {e}"))
        })?;
        let date_parsed = chrono::NaiveDate::parse_from_str(before_date, "%Y-%m-%d")
            .map_err(|e| crate::error::AppError::Config(e.to_string()))?;
        let upper_ts = safe_local_timestamp(date_parsed.and_hms_opt(0, 0, 0).unwrap());
        let count = conn.execute(
            "DELETE FROM activities WHERE timestamp < ?1",
            rusqlite::params![upper_ts],
        )?;
        Ok(count)
    }

    /// 对于浏览器，按 URL 合并记录
    /// 将重复记录的 duration 累加到保留记录后再删除重复项
    /// 返回删除的记录数和截图路径
    pub fn cleanup_duplicate_activities(&self, date: &str) -> Result<(usize, Vec<String>)> {
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let tx = conn.unchecked_transaction()?;

        // 获取当天的时间戳范围
        let date_parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;

        let start_ts = safe_local_timestamp(date_parsed.and_hms_opt(0, 0, 0).unwrap());
        let end_ts = start_ts + 86400;

        // 获取当天所有活动
        let mut stmt = conn.prepare(
            "SELECT id, app_name, browser_url, window_title, duration FROM activities
             WHERE timestamp >= ?1 AND timestamp < ?2",
        )?;

        let activities: Vec<(i64, String, Option<String>, Option<String>, i64)> = stmt
            .query_map(params![start_ts, end_ts], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Group by same key as get_timeline: (app_name, browser_url OR window_title)
        use std::collections::HashMap;
        // key -> Vec<(id, duration)>
        let mut groups: HashMap<String, Vec<(i64, i64)>> = HashMap::new();

        for (id, app_name, browser_url, window_title, duration) in activities {
            let key = if let Some(ref url) = browser_url {
                if !url.is_empty() {
                    format!("url:{app_name}|{}", url.trim_end_matches('/'))
                } else {
                    let title = window_title.as_deref().unwrap_or("");
                    format!("app:{app_name}|{title}")
                }
            } else {
                let title = window_title.as_deref().unwrap_or("");
                format!("app:{app_name}|{title}")
            };

            groups.entry(key).or_default().push((id, duration));
        }

        let mut total_deleted = 0usize;
        let mut all_paths = Vec::new();

        for (_key, mut entries) in groups {
            // 只有一条记录的组无需清理
            if entries.len() <= 1 {
                continue;
            }

            // 按 duration 降序排列，保留最长的那条
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            let keep_id = entries[0].0;

            // 计算需要累加的 duration（其余记录的总时长）
            let extra_duration: i64 = entries[1..].iter().map(|(_, d)| *d).sum();
            let ids_to_delete: Vec<i64> = entries[1..].iter().map(|(id, _)| *id).collect();

            // 先将额外的 duration 累加到保留记录
            if extra_duration > 0 {
                conn.execute(
                    "UPDATE activities SET duration = duration + ?1 WHERE id = ?2",
                    params![extra_duration, keep_id],
                )?;
            }

            // 获取要删除的记录的截图路径
            for del_id in &ids_to_delete {
                let path: String = conn
                    .query_row(
                        "SELECT screenshot_path FROM activities WHERE id = ?1",
                        params![del_id],
                        |row| row.get(0),
                    )
                    .unwrap_or_default();
                if !path.is_empty() {
                    all_paths.push(path);
                }
            }

            // 删除重复记录
            for del_id in &ids_to_delete {
                conn.execute("DELETE FROM activities WHERE id = ?1", params![del_id])?;
                total_deleted += 1;
            }
        }

        log::info!("清理重复记录: 删除 {total_deleted} 条，时长已合并到保留记录");

        tx.commit()?;

        Ok((total_deleted, all_paths))
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
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let date_parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;

        let start_ts = safe_local_timestamp(date_parsed.and_hms_opt(0, 0, 0).unwrap());
        let end_ts = start_ts + 86400;

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

        let screenshot_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM activities WHERE timestamp >= ?1 AND timestamp < ?2",
            params![start_ts, end_ts],
            |row| row.get(0),
        )?;

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
                row.get::<_, Option<String>>(9)?,
            ))
        })?;

        let mut total_duration: i64 = 0;
        let mut work_time_duration: i64 = 0;
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
            screenshot_url,
        ) in activity_rows
        {
            let day_duration = calculate_overlap_duration(timestamp, duration, start_ts, end_ts);
            if day_duration <= 0 {
                continue;
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
                    let bucket_end = (t / 3600 + 1) * 3600;
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

            let norm_cat = crate::categorize::normalize_category_key(&category);
            *category_usage_map.entry(norm_cat).or_insert(0) += day_duration;

            if crate::categorize::is_browser_app(&app_name) {
                browser_duration += day_duration;
                let normalized_browser_name =
                    crate::categorize::normalize_display_app_name(&app_name);
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

                let normalized_browser_url = browser_url
                    .as_deref()
                    .map(normalize_url)
                    .filter(|url| !url.is_empty());

                let page_hint = normalized_browser_url
                    .as_deref()
                    .filter(|url| !crate::categorize::is_merged_domain(url))
                    .map(|url| url.to_string())
                    .or_else(|| crate::categorize::infer_browser_page_hint(&window_title))
                    .or_else(|| {
                        ocr_text
                            .as_deref()
                            .and_then(crate::categorize::infer_browser_page_hint_from_text)
                    });

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
        app_usage.sort_by(|a, b| b.duration.cmp(&a.duration));

        let mut category_usage: Vec<CategoryUsage> = category_usage_map
            .into_iter()
            .map(|(category, duration)| CategoryUsage { category, duration })
            .collect();
        category_usage.sort_by(|a, b| b.duration.cmp(&a.duration));

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
                domains.sort_by(|a, b| b.duration.cmp(&a.duration));

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
        browser_usage.sort_by(|a, b| b.duration.cmp(&a.duration));

        let mut url_usage_rows: Vec<(String, i64)> = url_duration_map.into_iter().collect();
        url_usage_rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        // 先从全部 URL 聚合域名总时长，再截断 URL 列表
        let mut domain_total_map: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for (url, duration) in &url_usage_rows {
            let domain = crate::categorize::browser_page_domain_label(url);
            *domain_total_map.entry(domain).or_insert(0) += duration;
        }

        url_usage_rows.truncate(10);
        let url_usage: Vec<UrlUsage> = url_usage_rows
            .into_iter()
            .map(|(url, duration)| UrlUsage {
                domain: crate::categorize::browser_page_domain_label(&url),
                url,
                duration,
            })
            .collect();

        // 从截断后的 URL 列表构建 URL 明细
        let mut domain_url_detail_map: std::collections::HashMap<String, Vec<UrlDetail>> =
            std::collections::HashMap::new();
        for u in &url_usage {
            domain_url_detail_map
                .entry(u.domain.clone())
                .or_default()
                .push(UrlDetail {
                    url: u.url.clone(),
                    duration: u.duration,
                });
        }
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
        domain_usage.sort_by(|a, b| b.duration.cmp(&a.duration));
        domain_usage.truncate(10);

        Ok(DailyStats {
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
        let start_ts = safe_local_timestamp(date_parsed.and_hms_opt(0, 0, 0).unwrap());
        let end_ts = start_ts + 86400;

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
                    SUM(duration) OVER (
                        PARTITION BY
                            app_name,
                            CASE
                                WHEN browser_url IS NOT NULL AND browser_url != '' THEN RTRIM(browser_url, '/')
                                ELSE window_title
                            END
                    ) as total_duration
                FROM activities
                WHERE timestamp >= ?1 AND timestamp < ?2
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

    /// 获取每小时×应用的时长分布
    /// 返回 24 个小时桶，每桶内含各应用的累计时长
    pub fn get_hourly_app_breakdown(&self, date: &str) -> Result<Vec<HourlyAppBucket>> {
        let date_parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| AppError::Config(e.to_string()))?;
        let start_ts = safe_local_timestamp(date_parsed.and_hms_opt(0, 0, 0).unwrap());
        let end_ts = start_ts + 86400;
        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut stmt = conn.prepare(
            "SELECT
                CAST((timestamp - ?1) / 3600 AS INTEGER) as hour,
                app_name,
                SUM(duration) as total_duration,
                (
                    SELECT latest.screenshot_url
                    FROM activities latest
                    WHERE latest.timestamp >= ?1
                      AND latest.timestamp < ?2
                      AND CAST((latest.timestamp - ?1) / 3600 AS INTEGER) = CAST((activities.timestamp - ?1) / 3600 AS INTEGER)
                      AND latest.app_name = activities.app_name
                      AND latest.screenshot_url IS NOT NULL
                      AND TRIM(latest.screenshot_url) != ''
                    ORDER BY latest.timestamp DESC, latest.id DESC
                    LIMIT 1
                ) as screenshot_url
             FROM activities
             WHERE timestamp >= ?1 AND timestamp < ?2 AND duration > 0
             GROUP BY hour, app_name
             ORDER BY hour, total_duration DESC",
        )?;

        let raw: Vec<(i32, String, i64, Option<String>)> = stmt
            .query_map(params![start_ts, end_ts], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        // 按 hour 分桶
        let mut buckets: Vec<HourlyAppBucket> = (0..24)
            .map(|h| HourlyAppBucket {
                hour: h,
                total_duration: 0,
                apps: Vec::new(),
            })
            .collect();

        for (hour, app_name, duration, screenshot_url) in raw {
            if let Some(bucket) = buckets.get_mut(hour as usize) {
                bucket.total_duration += duration;
                bucket.apps.push(AppDuration {
                    app_name,
                    duration,
                    screenshot_url,
                });
            }
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
        let start_ts = safe_local_timestamp(date_parsed.and_hms_opt(h, 0, 0).unwrap());
        let end_ts = start_ts + 3600; // 1小时

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

        let mut stmt = conn.prepare(
            "SELECT app_name, duration, executable_path, screenshot_url, timestamp
             FROM activities
             ORDER BY timestamp DESC, id DESC",
        )?;

        let rows: Vec<(String, i64, Option<String>, Option<String>, i64)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut merged: HashMap<String, (i64, i64, Option<String>, Option<String>, i64)> =
            HashMap::new();
        for (raw_name, duration, executable_path, screenshot_url, timestamp) in rows {
            let normalized = crate::categorize::normalize_display_app_name(&raw_name);
            let entry = merged
                .entry(normalized.clone())
                .or_insert((0, 0, None, None, i64::MIN));
            entry.0 += duration;
            entry.1 += 1;
            if entry.2.is_none() && executable_path.is_some() {
                entry.2 = executable_path;
            }
            if screenshot_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
                && timestamp >= entry.4
            {
                entry.3 = screenshot_url;
                entry.4 = timestamp;
            }
        }

        let mut apps: Vec<AppUsage> = merged
            .into_iter()
            .map(|(app_name, (duration, count, executable_path, screenshot_url, _))| AppUsage {
                app_name,
                duration,
                count,
                executable_path,
                screenshot_url,
            })
            .collect();
        apps.sort_by(|a, b| b.duration.cmp(&a.duration).then_with(|| a.app_name.cmp(&b.app_name)));
        apps.truncate(limit as usize);
        Ok(apps)
    }

    pub fn get_app_category_overview(&self) -> Result<Vec<AppCategorySnapshot>> {
        use std::collections::HashMap;

        let conn = self.conn.lock().map_err(|e| {
            AppError::Database(rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut stmt = conn.prepare(
            "SELECT app_name, category, duration, timestamp, executable_path, screenshot_url
             FROM activities
             ORDER BY timestamp DESC, id DESC",
        )?;

        let rows: Vec<(String, String, i64, i64, Option<String>, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .collect();

        let mut merged: HashMap<String, AppCategorySnapshot> = HashMap::new();
        for (raw_name, category, duration, timestamp, executable_path, screenshot_url) in rows {
            let normalized_name = crate::categorize::normalize_display_app_name(&raw_name);
            let key = normalized_name.to_lowercase();

            let entry = merged.entry(key).or_insert_with(|| AppCategorySnapshot {
                app_name: normalized_name.clone(),
                category: category.clone(),
                total_duration: 0,
                count: 0,
                executable_path: executable_path.clone(),
                latest_timestamp: timestamp,
                screenshot_url: None,
            });

            entry.total_duration += duration;
            entry.count += 1;
            if entry.executable_path.is_none() && executable_path.is_some() {
                entry.executable_path = executable_path.clone();
            }
            if timestamp >= entry.latest_timestamp {
                entry.latest_timestamp = timestamp;
                entry.app_name = normalized_name;
                entry.category = category;
            }
            if entry.screenshot_url.is_none()
                && screenshot_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
            {
                entry.screenshot_url = screenshot_url;
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
        let like_pattern = format!("%{}%", app_name);
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

        let sql = format!(
            "SELECT {ACTIVITY_SELECT_COLUMNS}
             FROM activities
             WHERE browser_url IS NOT NULL AND browser_url != '' AND browser_url LIKE ?1
             ORDER BY timestamp ASC, id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;

        let like_pattern = format!("%{}%", &target);
        let activities = stmt
            .query_map([&like_pattern], activity_from_row)?
            .filter_map(|row| row.ok())
            .filter(|activity| {
                activity
                    .browser_url
                    .as_deref()
                    .and_then(crate::categorize::normalize_domain_rule)
                    .as_deref()
                    == Some(target.as_str())
            })
            .collect();

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
            .filter(|t| t.len() >= 1 && !t.is_empty())
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
                    title: format!("{date} {:02}:00 小时摘要", hour),
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
                title: format!("{date} {:02}:00 小时摘要", hour),
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
    use super::{safe_local_timestamp, Activity, Database};
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
        db.merge_activity(inserted_id, 20, Some("new"), "shot-b.jpg", now)
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
        assert_eq!(stats.browser_usage[0].domains[0].urls[0].url, "linux.dolatest");
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
}
