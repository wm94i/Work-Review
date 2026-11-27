use crate::screenshot::ScreenshotService;
use crate::storage::StorageManager;
use crate::AppState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use work_review_core::config::{atomic_replace, sync_parent_directory, AppConfig, PrivacyConfig};
use work_review_core::database::{
    inspect_sqlite_backup_schema_version, Activity, Database, CURRENT_SCHEMA_VERSION,
};
use work_review_core::error::AppError;
use work_review_core::privacy::{sanitize_activity_for_external, PrivacyAction, PrivacyFilter};

use super::config::{
    commit_migration_stage, path_for_display, remove_migration_path,
    try_acquire_data_maintenance_lock,
};
use super::report::validate_export_dir;
use super::shared::{collect_privacy_filters, filter_activities_by_privacy, is_iso_date};

const BACKUP_FORMAT_VERSION: u32 = 1;
const MAX_EXPORT_RANGE_DAYS: i64 = 3650;
const EXPORT_ACTIVITY_PAGE_SIZE: usize = 5_000;
// 恢复只替换备份支持的数据，本机技能、任务和凭证在切换及回滚时都留在原位。
const RESTORE_PRESERVED_ENTRIES: &[&str] = &[
    "localhost_api_token.txt",
    "update_settings.json",
    "skills",
    "agent_routines.json",
    "mcp_clients.json",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DataExportFormat {
    Csv,
    Json,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportActivitiesResult {
    pub path: String,
    pub count: usize,
    pub format: DataExportFormat,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDailyStatsResult {
    pub path: String,
    pub days: usize,
    pub format: DataExportFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupFileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub format_version: u32,
    pub app_version: String,
    pub schema_version: i32,
    pub created_at: i64,
    pub includes_screenshots: bool,
    pub includes_secrets: bool,
    pub files: Vec<BackupFileEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBackupResult {
    pub backup_dir: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub includes_screenshots: bool,
    pub includes_secrets: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreBackupResult {
    pub restored_from: String,
    pub safety_backup_dir: String,
    pub restored_files: usize,
    pub schema_version: i32,
}

#[derive(Debug, Clone, Serialize)]
struct ExportedDailyStatRow {
    date: String,
    total_duration: i64,
    work_time_duration: i64,
    overtime_duration: i64,
    browser_duration: i64,
    screenshot_count: i64,
    top_app: String,
    top_category: String,
}

/// RFC 4180 CSV 单元格转义 + 公式注入防护：
/// 以 `=`、`+`、`-`、`@`、`\t`、`\r`、`\n` 或控制字符开头（含前导任意空白/控制字符后的 `=`/`+`/`-`/`@`）
/// 一律加前缀 `'`，防止 Excel/Sheets 执行公式。
pub fn escape_csv_cell(raw: &str) -> String {
    let starts_with_danger = raw
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r' | '\n') || c.is_control());
    let trimmed_start = raw.trim_start_matches(|c: char| c.is_whitespace() || c.is_control());
    let trimmed_starts_with_formula = trimmed_start
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@'));
    let needs_formula_guard = starts_with_danger || trimmed_starts_with_formula;
    let guarded = if needs_formula_guard {
        format!("'{raw}")
    } else {
        raw.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

fn write_activities_csv_header<W: Write>(writer: &mut W, include_ocr: bool) -> std::io::Result<()> {
    if include_ocr {
        writer.write_all(b"id,timestamp,datetime,app_name,window_title,category,semantic_category,duration_secs,browser_url,ocr_text\r\n")
    } else {
        writer.write_all(b"id,timestamp,datetime,app_name,window_title,category,semantic_category,duration_secs,browser_url\r\n")
    }
}

fn write_activity_csv_row<W: Write>(
    writer: &mut W,
    act: &Activity,
    include_ocr: bool,
) -> std::io::Result<()> {
    let dt = chrono::DateTime::from_timestamp(act.timestamp, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default();
    let mut cells = vec![
        act.id.map(|id| id.to_string()).unwrap_or_default(),
        act.timestamp.to_string(),
        escape_csv_cell(&dt),
        escape_csv_cell(&act.app_name),
        escape_csv_cell(&act.window_title),
        escape_csv_cell(&act.category),
        escape_csv_cell(act.semantic_category.as_deref().unwrap_or("")),
        act.duration.to_string(),
        escape_csv_cell(act.browser_url.as_deref().unwrap_or("")),
    ];
    if include_ocr {
        cells.push(escape_csv_cell(act.ocr_text.as_deref().unwrap_or("")));
    }
    writer.write_all(cells.join(",").as_bytes())?;
    writer.write_all(b"\r\n")
}

fn format_daily_stats_csv(rows: &[ExportedDailyStatRow]) -> String {
    let mut out = String::from(
        "date,total_duration_secs,work_time_duration_secs,overtime_duration_secs,browser_duration_secs,screenshot_count,top_app,top_category\r\n",
    );
    for row in rows {
        let line = [
            escape_csv_cell(&row.date),
            row.total_duration.to_string(),
            row.work_time_duration.to_string(),
            row.overtime_duration.to_string(),
            row.browser_duration.to_string(),
            row.screenshot_count.to_string(),
            escape_csv_cell(&row.top_app),
            escape_csv_cell(&row.top_category),
        ]
        .join(",");
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out
}

fn reject_symlink_file(path: &Path) -> Result<(), AppError> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(AppError::Config(
                "目标文件为符号链接，已拒绝写入".to_string(),
            ));
        }
    }
    Ok(())
}

/// 在已校验的导出目录内通过独占临时文件（`create_new(true)`）流式写入，落盘 `sync_all` 后
/// 使用跨平台原子替换（Unix `rename` / Windows `MoveFileExW`）覆盖目标文件，
/// 既消除符号链接 TOCTOU，又保证替换失败时原有目标文件完整无损。
pub(crate) fn write_export_file_atomic_with<F, T>(
    export_dir: &Path,
    file_name: &str,
    write_fn: F,
) -> Result<(PathBuf, T), AppError>
where
    F: FnOnce(&mut BufWriter<std::fs::File>) -> Result<T, AppError>,
{
    let output_path = export_dir.join(file_name);
    reject_symlink_file(&output_path)?;

    let mut tmp_path = None;
    let mut tmp_file = None;
    for _ in 0..16 {
        let candidate = export_dir.join(format!(
            ".work-review-export-{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                tmp_path = Some(candidate);
                tmp_file = Some(file);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let tmp_path =
        tmp_path.ok_or_else(|| AppError::Unknown("无法在导出目录创建独占临时文件".to_string()))?;
    let tmp_file = tmp_file.expect("tmp_file present when tmp_path is Some");

    let write_res: Result<T, AppError> = (|| {
        let mut writer = BufWriter::new(tmp_file);
        let value = write_fn(&mut writer)?;
        writer.flush()?;
        let file = writer
            .into_inner()
            .map_err(|e| AppError::Unknown(format!("刷新导出文件缓冲区失败: {e}")))?;
        file.sync_all()?;
        drop(file);

        reject_symlink_file(&output_path)?;
        atomic_replace(&tmp_path, &output_path)?;
        if let Err(err) = sync_parent_directory(export_dir) {
            log::warn!(
                "导出文件已提交，但同步父目录 {} 失败: {err}",
                export_dir.display()
            );
        }
        Ok(value)
    })();

    if write_res.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    let value = write_res?;
    Ok((output_path, value))
}

pub(crate) fn write_export_file_atomic(
    export_dir: &Path,
    file_name: &str,
    content: &[u8],
) -> Result<PathBuf, AppError> {
    let (path, ()) = write_export_file_atomic_with(export_dir, file_name, |writer| {
        writer.write_all(content)?;
        Ok(())
    })?;
    Ok(path)
}

fn parse_validated_date_range(
    date_from: &str,
    date_to: &str,
) -> Result<(chrono::NaiveDate, chrono::NaiveDate), AppError> {
    let from = date_from.trim();
    let to = date_to.trim();
    if !is_iso_date(from) {
        return Err(AppError::Config(format!("开始日期格式无效: {from}")));
    }
    if !is_iso_date(to) {
        return Err(AppError::Config(format!("结束日期格式无效: {to}")));
    }
    let start = chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .map_err(|e| AppError::Config(format!("开始日期无效: {e}")))?;
    let end = chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d")
        .map_err(|e| AppError::Config(format!("结束日期无效: {e}")))?;
    if start > end {
        return Err(AppError::Config("开始日期不能晚于结束日期".to_string()));
    }
    if (end - start).num_days() > MAX_EXPORT_RANGE_DAYS {
        return Err(AppError::Config(format!(
            "导出范围不能超过 {MAX_EXPORT_RANGE_DAYS} 天"
        )));
    }
    Ok((start, end))
}

fn sha256_bytes(bytes: &[u8]) -> (u64, String) {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    (bytes.len() as u64, hex::encode(hasher.finalize()))
}

fn sha256_file(path: &Path) -> Result<(u64, String), AppError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((total, hex::encode(hasher.finalize())))
}

fn create_unique_dir_exclusive(parent: &Path, base_name: &str) -> Result<PathBuf, AppError> {
    let first = parent.join(base_name);
    match std::fs::create_dir(&first) {
        Ok(()) => return Ok(first),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    for _ in 0..16 {
        let candidate = parent.join(format!(
            "{base_name}-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(AppError::Unknown(format!(
        "无法创建唯一目录（前缀 {base_name}）"
    )))
}

fn publish_staged_dir_unique(
    stage_dir: &Path,
    parent: &Path,
    base_name: &str,
) -> Result<PathBuf, AppError> {
    let first = parent.join(base_name);
    if !first.exists() && std::fs::rename(stage_dir, &first).is_ok() {
        return Ok(first);
    }
    for _ in 0..16 {
        let candidate = parent.join(format!(
            "{base_name}-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        if candidate.exists() {
            continue;
        }
        match std::fs::rename(stage_dir, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(AppError::Unknown(format!(
        "无法发布备份目录（前缀 {base_name}）"
    )))
}

fn is_safe_backup_entry_name(name: &std::ffi::OsStr) -> bool {
    let s = name.to_string_lossy();
    !s.is_empty() && !s.starts_with('.') && !s.contains([':', '/', '\\'])
}

fn copy_regular_dir_files_recursive(
    src_root: &Path,
    dst_root: &Path,
) -> Result<Vec<PathBuf>, AppError> {
    let mut copied = Vec::new();
    let root_meta = match std::fs::symlink_metadata(src_root) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(copied),
        Err(e) => return Err(e.into()),
    };
    if root_meta.file_type().is_symlink() || !root_meta.file_type().is_dir() {
        return Ok(copied);
    }
    let mut stack = vec![(src_root.to_path_buf(), dst_root.to_path_buf())];
    while let Some((src_dir, dst_dir)) = stack.pop() {
        for entry in std::fs::read_dir(&src_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            // 跳过隐藏文件与非法路径段（如 macOS `.DS_Store`），确保备份产物始终可通过 `verify_backup_dir` 恢复。
            if !is_safe_backup_entry_name(&name) {
                continue;
            }
            let meta = std::fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            let dst_path = dst_dir.join(&name);
            if meta.file_type().is_dir() {
                stack.push((entry.path(), dst_path));
            } else if meta.file_type().is_file() {
                if let Some(parent) = dst_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), &dst_path)?;
                copied.push(dst_path);
            }
        }
    }
    Ok(copied)
}

fn validate_backup_relative_path(raw: &str) -> Result<PathBuf, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.ends_with('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
    {
        return Err(AppError::Config(format!("备份清单路径非法: {raw}")));
    }
    let rel = PathBuf::from(trimmed);
    let mut comp_names: Vec<String> = Vec::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(name) => {
                let s = name.to_string_lossy().to_string();
                if s.is_empty() || s.starts_with('.') || s.contains(':') {
                    return Err(AppError::Config(format!("备份清单包含非法路径段: {raw}")));
                }
                comp_names.push(s);
            }
            _ => return Err(AppError::Config(format!("备份清单包含非法路径组件: {raw}"))),
        }
    }
    let Some(first) = comp_names.first().map(String::as_str) else {
        return Err(AppError::Config(format!("备份清单路径为空: {raw}")));
    };
    match first {
        "workreview.db" | "config.json" => {
            if comp_names.len() != 1 {
                return Err(AppError::Config(format!(
                    "备份清单核心文件路径层级非法: {raw}"
                )));
            }
        }
        "screenshots" => {
            if comp_names.len() < 2 {
                return Err(AppError::Config(format!(
                    "备份清单截图路径必须指向文件: {raw}"
                )));
            }
        }
        _ => {
            return Err(AppError::Config(format!("备份清单包含未授权条目: {raw}")));
        }
    }
    Ok(rel)
}

/// 逐级校验 `base` 下的每个路径段，禁止任何中间目录或目标文件为符号链接。
fn ensure_no_symlink_components(base: &Path, rel: &Path) -> Result<PathBuf, AppError> {
    let mut current = base.to_path_buf();
    for comp in rel.components() {
        current.push(comp);
        let meta = std::fs::symlink_metadata(&current).map_err(|_| {
            AppError::Config(format!("备份路径不存在或无法访问: {}", rel.display()))
        })?;
        if meta.file_type().is_symlink() {
            return Err(AppError::Config(format!(
                "备份路径包含符号链接，已拒绝访问: {}",
                rel.display()
            )));
        }
    }
    Ok(current)
}

struct ExportActivitiesRequest<'a> {
    from_str: &'a str,
    to_str: &'a str,
    format: DataExportFormat,
    include_ocr: bool,
    ignored_apps: &'a [String],
    excluded_domains: &'a [String],
    privacy_config: &'a PrivacyConfig,
    export_dir: &'a Path,
    page_size: usize,
}

fn export_activities_to_dir(
    database: &Database,
    req: &ExportActivitiesRequest<'_>,
) -> Result<ExportActivitiesResult, AppError> {
    let ext = match req.format {
        DataExportFormat::Csv => "csv",
        DataExportFormat::Json => "json",
    };
    let file_name = format!(
        "work-review-activities-{}-to-{}.{ext}",
        req.from_str, req.to_str
    );
    let effective_page_size = req.page_size.max(1);
    let filter = PrivacyFilter::from_config(req.privacy_config);

    let (output_path, exported_count) =
        write_export_file_atomic_with(req.export_dir, &file_name, |writer| {
            match req.format {
                DataExportFormat::Csv => write_activities_csv_header(writer, req.include_ocr)?,
                DataExportFormat::Json => writer.write_all(b"[")?,
            }

            let mut exported_count = 0usize;
            let mut after_id: i64 = 0;

            loop {
                let page = database.get_activities_in_date_range_after_id(
                    req.from_str,
                    req.to_str,
                    after_id,
                    effective_page_size,
                )?;
                if page.is_empty() {
                    break;
                }
                let page_len = page.len();
                let last_id = page.last().and_then(|a| a.id);
                let filtered =
                    filter_activities_by_privacy(page, req.ignored_apps, req.excluded_domains);
                for mut act in filtered {
                    // 隐私优先：先按完整隐私规则过滤 `Skip`（含关键词/无痕窗口等），再脱敏写出
                    if filter.check_privacy_full(
                        &act.app_name,
                        &act.window_title,
                        act.browser_url.as_deref(),
                    ) == PrivacyAction::Skip
                    {
                        continue;
                    }
                    sanitize_activity_for_external(&filter, &mut act, req.include_ocr);
                    // 导出的外部文件不暴露本机绝对截图路径、可执行路径或远端签名截图链接
                    act.screenshot_path.clear();
                    act.executable_path = None;
                    act.screenshot_url = None;

                    match req.format {
                        DataExportFormat::Csv => {
                            write_activity_csv_row(writer, &act, req.include_ocr)?;
                        }
                        DataExportFormat::Json => {
                            if exported_count == 0 {
                                writer.write_all(b"\n  ")?;
                            } else {
                                writer.write_all(b",\n  ")?;
                            }
                            serde_json::to_writer(&mut *writer, &act).map_err(|e| {
                                AppError::Unknown(format!("流式序列化活动 JSON 失败: {e}"))
                            })?;
                        }
                    }
                    exported_count += 1;
                }
                if page_len < effective_page_size {
                    break;
                }
                match last_id {
                    Some(id) if id > after_id => after_id = id,
                    _ => break,
                }
            }

            if req.format == DataExportFormat::Json {
                if exported_count > 0 {
                    writer.write_all(b"\n]\n")?;
                } else {
                    writer.write_all(b"]\n")?;
                }
            }

            Ok(exported_count)
        })?;

    Ok(ExportActivitiesResult {
        path: path_for_display(&output_path),
        count: exported_count,
        format: req.format,
    })
}

/// 导出活动记录为 CSV 或 JSON（经 `validate_export_dir` 校验、游标分页流式写出与完整隐私脱敏）
#[tauri::command]
pub async fn export_activities(
    date_from: String,
    date_to: String,
    format: DataExportFormat,
    include_ocr: bool,
    target_dir: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<ExportActivitiesResult, AppError> {
    let (start_date, end_date) = parse_validated_date_range(&date_from, &date_to)?;
    let export_dir = validate_export_dir(Path::new(target_dir.trim()))?;
    let from_str = start_date.format("%Y-%m-%d").to_string();
    let to_str = end_date.format("%Y-%m-%d").to_string();

    let (database, ignored_apps, excluded_domains, privacy_config) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&guard);
        (
            guard.database.clone(),
            ignored_apps,
            excluded_domains,
            guard.config.privacy.clone(),
        )
    };

    tokio::task::spawn_blocking(move || {
        export_activities_to_dir(
            &database,
            &ExportActivitiesRequest {
                from_str: &from_str,
                to_str: &to_str,
                format,
                include_ocr,
                ignored_apps: &ignored_apps,
                excluded_domains: &excluded_domains,
                privacy_config: &privacy_config,
                export_dir: &export_dir,
                page_size: EXPORT_ACTIVITY_PAGE_SIZE,
            },
        )
    })
    .await
    .map_err(|e| AppError::Unknown(format!("导出活动任务异常: {e}")))?
}

/// 按天汇总导出每日统计（CSV / JSON）
#[tauri::command]
pub async fn export_daily_stats(
    date_from: String,
    date_to: String,
    format: DataExportFormat,
    target_dir: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<ExportDailyStatsResult, AppError> {
    let (start_date, end_date) = parse_validated_date_range(&date_from, &date_to)?;
    let export_dir = validate_export_dir(Path::new(target_dir.trim()))?;
    let from_str = start_date.format("%Y-%m-%d").to_string();
    let to_str = end_date.format("%Y-%m-%d").to_string();

    let (
        database,
        segments,
        ignored_apps,
        excluded_domains,
        work_time_enabled,
        standard_work_hours,
    ) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&guard);
        (
            guard.database.clone(),
            guard.config.effective_work_segments(),
            ignored_apps,
            excluded_domains,
            guard.config.work_time_enabled,
            guard.config.standard_work_hours,
        )
    };

    tokio::task::spawn_blocking(move || {
        let per_day = database.get_daily_stats_range_with_segments_filtered(
            start_date,
            end_date,
            &segments,
            &ignored_apps,
            &excluded_domains,
        )?;
        let rows: Vec<ExportedDailyStatRow> = per_day
            .into_iter()
            .map(|(date, mut stats)| {
                work_review_core::database::apply_flex_overtime_correction(
                    &mut stats,
                    work_time_enabled,
                    standard_work_hours,
                );
                ExportedDailyStatRow {
                    date,
                    total_duration: stats.total_duration,
                    work_time_duration: stats.work_time_duration,
                    overtime_duration: stats.overtime_duration,
                    browser_duration: stats.browser_duration,
                    screenshot_count: stats.screenshot_count,
                    top_app: stats
                        .app_usage
                        .first()
                        .map(|a| a.app_name.clone())
                        .unwrap_or_default(),
                    top_category: stats
                        .category_usage
                        .first()
                        .map(|c| c.category.clone())
                        .unwrap_or_default(),
                }
            })
            .collect();

        let ext = match format {
            DataExportFormat::Csv => "csv",
            DataExportFormat::Json => "json",
        };
        let file_name = format!("work-review-daily-stats-{from_str}-to-{to_str}.{ext}");

        let content = match format {
            DataExportFormat::Csv => format_daily_stats_csv(&rows),
            DataExportFormat::Json => serde_json::to_string_pretty(&rows)
                .map_err(|e| AppError::Unknown(format!("序列化日统计 JSON 失败: {e}")))?,
        };
        let output_path = write_export_file_atomic(&export_dir, &file_name, content.as_bytes())?;

        Ok(ExportDailyStatsResult {
            path: path_for_display(&output_path),
            days: rows.len(),
            format,
        })
    })
    .await
    .map_err(|e| AppError::Unknown(format!("导出日统计任务异常: {e}")))?
}

/// 将数据库快照、配置、可选截图及完整 SHA-256 `manifest.json` 写入已独占创建的暂存目录。
/// 供 `create_backup_from_parts` 与恢复前自动安全备份复用，确保安全备份同样可被 `restore_backup` 完整恢复。
fn write_verified_backup_bundle(
    database: &Database,
    config: &AppConfig,
    data_dir: &Path,
    dest_dir: &Path,
    include_screenshots: bool,
    include_secrets: bool,
) -> Result<(usize, u64), AppError> {
    let db_target = dest_dir.join("workreview.db");
    database.backup_to(&db_target)?;

    let backup_config = if include_secrets {
        config.clone()
    } else {
        config.redacted_for_backup()
    };
    let config_target = dest_dir.join("config.json");
    backup_config.save(&config_target)?;

    let mut files_to_hash = vec![
        ("config.json".to_string(), config_target),
        ("workreview.db".to_string(), db_target),
    ];

    if include_screenshots {
        let src_shots = data_dir.join("screenshots");
        let dst_shots = dest_dir.join("screenshots");
        let copied = copy_regular_dir_files_recursive(&src_shots, &dst_shots)?;
        for file_path in copied {
            let rel = file_path
                .strip_prefix(dest_dir)
                .map_err(|e| AppError::Unknown(format!("计算备份相对路径失败: {e}")))?
                .to_string_lossy()
                .replace('\\', "/");
            if validate_backup_relative_path(&rel).is_err() {
                let _ = std::fs::remove_file(&file_path);
                continue;
            }
            files_to_hash.push((rel, file_path));
        }
    }

    files_to_hash.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest_files = Vec::with_capacity(files_to_hash.len());
    let mut total_bytes = 0u64;
    for (rel, full_path) in files_to_hash {
        let (size, sha256) = sha256_file(&full_path)?;
        total_bytes += size;
        manifest_files.push(BackupFileEntry {
            path: rel,
            size,
            sha256,
        });
    }

    let file_count = manifest_files.len();
    let manifest = BackupManifest {
        format_version: BACKUP_FORMAT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: CURRENT_SCHEMA_VERSION,
        created_at: chrono::Utc::now().timestamp(),
        includes_screenshots: include_screenshots,
        includes_secrets: include_secrets,
        files: manifest_files,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Unknown(format!("序列化备份清单失败: {e}")))?;
    std::fs::write(dest_dir.join("manifest.json"), manifest_json)?;

    // 发布前对暂存包执行一次完整的 `verify_backup_dir` 自检，确保每份备份与安全备份必然可恢复。
    verify_backup_dir(dest_dir)?;

    Ok((file_count, total_bytes))
}

/// 纯目录级备份实现（不依赖 AppHandle，便于单测往返验证）。
pub(crate) fn create_backup_from_parts(
    database: &Database,
    config: &AppConfig,
    data_dir: &Path,
    target_dir: &Path,
    include_screenshots: bool,
    include_secrets: bool,
) -> Result<CreateBackupResult, AppError> {
    let validated_parent = validate_export_dir(target_dir)?;
    let live_canonical = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    // 禁止将备份目录放在当前数据目录内（含 screenshots/ 子目录），防止递归复制与污染数据目录。
    if validated_parent == live_canonical || validated_parent.starts_with(&live_canonical) {
        return Err(AppError::Config(
            "备份目标目录不能位于当前数据目录内部，请选择独立目录".to_string(),
        ));
    }

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let stage_dir = create_unique_dir_exclusive(
        &validated_parent,
        &format!(
            ".work-review-backup-stage-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ),
    )?;

    let write_res: Result<CreateBackupResult, AppError> = (|| {
        let (file_count, total_bytes) = write_verified_backup_bundle(
            database,
            config,
            &live_canonical,
            &stage_dir,
            include_screenshots,
            include_secrets,
        )?;
        let final_dir = publish_staged_dir_unique(
            &stage_dir,
            &validated_parent,
            &format!("work-review-backup-{stamp}"),
        )?;

        Ok(CreateBackupResult {
            backup_dir: path_for_display(&final_dir),
            file_count,
            total_bytes,
            includes_screenshots: include_screenshots,
            includes_secrets: include_secrets,
        })
    })();

    if write_res.is_err() {
        let _ = remove_migration_path(&stage_dir);
    }
    write_res
}

/// 一键备份当前数据到指定目录（默认不含截图与密钥）；与恢复/迁移共用进程维护互斥锁避免竞态。
#[tauri::command]
pub async fn create_backup(
    target_dir: String,
    include_screenshots: bool,
    include_secrets: bool,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<CreateBackupResult, AppError> {
    let maintenance_guard = try_acquire_data_maintenance_lock()?;
    let target_path = PathBuf::from(target_dir.trim());
    let (database, config, data_dir) = {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (
            guard.database.clone(),
            guard.config.clone(),
            guard.data_dir.clone(),
        )
    };
    tokio::task::spawn_blocking(move || {
        let _maintenance_guard = maintenance_guard;
        create_backup_from_parts(
            &database,
            &config,
            &data_dir,
            &target_path,
            include_screenshots,
            include_secrets,
        )
    })
    .await
    .map_err(|e| AppError::Unknown(format!("备份任务异常: {e}")))?
}

/// 校验备份目录的 manifest、路径安全性、重复条目、全路径符号链接、SHA-256 与 SQLite schema 版本（只读）。
pub(crate) fn verify_backup_dir(backup_dir: &Path) -> Result<(PathBuf, BackupManifest), AppError> {
    let canonical = backup_dir
        .canonicalize()
        .map_err(|_| AppError::Config("备份目录不存在或无法访问".to_string()))?;
    if !canonical.is_dir() {
        return Err(AppError::Config("请选择有效的备份目录".to_string()));
    }
    let manifest_path = canonical.join("manifest.json");
    let manifest_meta = std::fs::symlink_metadata(&manifest_path)
        .map_err(|_| AppError::Config("备份目录缺少 manifest.json".to_string()))?;
    if manifest_meta.file_type().is_symlink() || !manifest_meta.file_type().is_file() {
        return Err(AppError::Config(
            "备份目录中的 manifest.json 不是普通文件".to_string(),
        ));
    }
    let manifest_raw = std::fs::read_to_string(&manifest_path)?;
    let manifest: BackupManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| AppError::Config(format!("备份清单格式损坏: {e}")))?;

    if manifest.format_version != BACKUP_FORMAT_VERSION {
        return Err(AppError::Config(format!(
            "不支持的备份格式版本: v{}（当前支持 v{}）",
            manifest.format_version, BACKUP_FORMAT_VERSION
        )));
    }
    if manifest.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(AppError::Config(format!(
            "备份数据库版本（v{}）高于当前程序支持版本（v{}），请先升级程序后再恢复",
            manifest.schema_version, CURRENT_SCHEMA_VERSION
        )));
    }

    let mut seen_paths: HashSet<String> = HashSet::with_capacity(manifest.files.len());
    let mut has_db = false;
    let mut has_config = false;
    for entry in &manifest.files {
        let rel = validate_backup_relative_path(&entry.path)?;
        let dedup_key = entry.path.trim().to_lowercase();
        if !seen_paths.insert(dedup_key) {
            return Err(AppError::Config(format!(
                "备份清单包含重复条目: {}",
                entry.path
            )));
        }
        if entry.path == "workreview.db" {
            has_db = true;
        } else if entry.path == "config.json" {
            has_config = true;
        } else if !manifest.includes_screenshots {
            return Err(AppError::Config(format!(
                "备份清单声明不含截图，却包含截图条目: {}",
                entry.path
            )));
        }
        let full = ensure_no_symlink_components(&canonical, &rel)?;
        let meta = std::fs::symlink_metadata(&full)
            .map_err(|_| AppError::Config(format!("备份文件缺失: {}", entry.path)))?;
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            return Err(AppError::Config(format!(
                "备份条目不是普通文件: {}",
                entry.path
            )));
        }
        let (actual_size, actual_hash) = sha256_file(&full)?;
        if actual_size != entry.size || !actual_hash.eq_ignore_ascii_case(&entry.sha256) {
            return Err(AppError::Config(format!(
                "备份文件哈希校验失败（文件可能已损坏或被篡改）: {}",
                entry.path
            )));
        }
    }
    if !has_db || !has_config {
        return Err(AppError::Config(
            "备份清单不完整：必须包含 workreview.db 与 config.json".to_string(),
        ));
    }

    let db_path = canonical.join("workreview.db");
    let actual_schema = inspect_sqlite_backup_schema_version(&db_path)?;
    if actual_schema > CURRENT_SCHEMA_VERSION {
        return Err(AppError::Config(format!(
            "备份数据库实际 schema 版本（v{}）高于当前程序支持版本（v{}），已拒绝恢复",
            actual_schema, CURRENT_SCHEMA_VERSION
        )));
    }

    Ok((canonical, manifest))
}

struct PreparedRestoreStage {
    canonical_backup: PathBuf,
    live_canonical: PathBuf,
    safety_dir: PathBuf,
    stage_dir: PathBuf,
    restored_config: AppConfig,
    restored_files: usize,
    schema_version: i32,
}

fn prepare_restore_backup_stage(
    backup_dir: &Path,
    live_data_dir: &Path,
    live_database: &Database,
    live_config: &AppConfig,
) -> Result<PreparedRestoreStage, AppError> {
    let (canonical_backup, manifest) = verify_backup_dir(backup_dir)?;
    let live_canonical = live_data_dir
        .canonicalize()
        .unwrap_or_else(|_| live_data_dir.to_path_buf());
    if canonical_backup == live_canonical
        || canonical_backup.starts_with(&live_canonical)
        || live_canonical.starts_with(&canonical_backup)
    {
        return Err(AppError::Config(
            "备份目录不能与当前数据目录重叠".to_string(),
        ));
    }
    let live_parent = live_canonical
        .parent()
        .ok_or_else(|| AppError::Config("无法解析数据目录父路径".to_string()))?;

    // 1. 校验 config.json 原始字节的 SHA-256 后再解析，消除 verify 与 read 之间的 TOCTOU。
    let config_entry = manifest
        .files
        .iter()
        .find(|e| e.path == "config.json")
        .ok_or_else(|| AppError::Config("备份清单缺少 config.json".to_string()))?;
    let config_full = ensure_no_symlink_components(&canonical_backup, Path::new("config.json"))?;
    let config_bytes = std::fs::read(&config_full)?;
    let (cfg_size, cfg_hash) = sha256_bytes(&config_bytes);
    if cfg_size != config_entry.size || !cfg_hash.eq_ignore_ascii_case(&config_entry.sha256) {
        return Err(AppError::Config(
            "备份配置文件在恢复期间发生变化，已中止恢复".to_string(),
        ));
    }
    let config_text = String::from_utf8(config_bytes)
        .map_err(|e| AppError::Config(format!("备份配置编码无效: {e}")))?;
    // 先 normalize 完成旧版字段迁移（避免 migrate_legacy_config 覆盖回填后的 text_model.api_key），
    // 再按需回填本机密钥（仅在端点/身份一致时回填），最后再次 normalize 同步 model profiles。
    let mut restored_config: AppConfig = serde_json::from_str(&config_text)
        .map_err(|e| AppError::Config(format!("备份配置解析失败: {e}")))?;
    restored_config.normalize();
    if !manifest.includes_secrets {
        restored_config.preserve_missing_secrets_from(live_config);
        restored_config.normalize();
    }

    // 2. 自动创建一份完整可恢复的恢复前安全备份（含 manifest.json、SHA-256 与按需截图），
    //    用户后续可直接使用同一个 restore_backup 命令回退到该安全备份。
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let safety_stage = create_unique_dir_exclusive(
        live_parent,
        &format!(
            ".work-review-pre-restore-stage-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ),
    )?;
    let safety_res: Result<PathBuf, AppError> = (|| {
        write_verified_backup_bundle(
            live_database,
            live_config,
            &live_canonical,
            &safety_stage,
            manifest.includes_screenshots,
            true,
        )?;
        publish_staged_dir_unique(
            &safety_stage,
            live_parent,
            &format!("work-review-pre-restore-backup-{stamp}"),
        )
    })();
    let safety_dir = match safety_res {
        Ok(dir) => dir,
        Err(err) => {
            let _ = remove_migration_path(&safety_stage);
            return Err(AppError::Config(format!(
                "恢复前自动备份当前数据失败，已中止恢复: {err}"
            )));
        }
    };

    // 3. 在暂存目录仅按 manifest 白名单逐个暂存并复验 SHA-256（绝不递归复制未校验文件）。
    let stage_dir = create_unique_dir_exclusive(
        live_parent,
        &format!(
            ".work-review-restore-stage-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ),
    )?;
    let stage_res: Result<(), AppError> = (|| {
        for entry in &manifest.files {
            if entry.path == "config.json" {
                continue;
            }
            let rel = validate_backup_relative_path(&entry.path)?;
            let src = ensure_no_symlink_components(&canonical_backup, &rel)?;
            let dst = stage_dir.join(&rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dst)?;
            let (staged_size, staged_hash) = sha256_file(&dst)?;
            if staged_size != entry.size || !staged_hash.eq_ignore_ascii_case(&entry.sha256) {
                return Err(AppError::Config(format!(
                    "暂存文件二次哈希校验失败（源文件在恢复期间发生变更）: {}",
                    entry.path
                )));
            }
        }

        restored_config.save(&stage_dir.join("config.json"))?;

        // 若备份不含截图，则保留当前截图目录中的常规文件
        if !manifest.includes_screenshots {
            let current_shots = live_canonical.join("screenshots");
            if current_shots.exists() {
                copy_regular_dir_files_recursive(&current_shots, &stage_dir.join("screenshots"))?;
            }
        }

        // 切换前先在暂存目录实际打开并初始化目标 SQLite 数据库，确保可正常打开与迁移后再动线上数据
        let staged_db_path = stage_dir.join("workreview.db");
        {
            let probe_db = Database::new(&staged_db_path).map_err(|e| {
                AppError::Config(format!("暂存数据库无法打开或初始化，已中止恢复: {e}"))
            })?;
            probe_db.release_file_handles()?;
        }
        for sidecar in ["workreview.db-wal", "workreview.db-shm"] {
            let _ = std::fs::remove_file(stage_dir.join(sidecar));
        }

        Ok(())
    })();
    if let Err(err) = stage_res {
        let _ = remove_migration_path(&stage_dir);
        return Err(err);
    }

    Ok(PreparedRestoreStage {
        canonical_backup,
        live_canonical,
        safety_dir,
        stage_dir,
        restored_config,
        restored_files: manifest.files.len(),
        schema_version: manifest.schema_version,
    })
}

/// 当切库后重新打开新数据库失败时，从 `safety_dir` 尝试自动回滚原库与配置。
fn rollback_live_data_dir_from_safety(
    safety_dir: &Path,
    live_canonical: &Path,
) -> Result<(), AppError> {
    let live_parent = live_canonical
        .parent()
        .ok_or_else(|| AppError::Config("无法解析数据目录父路径以回滚".to_string()))?;
    let rollback_stage = create_unique_dir_exclusive(
        live_parent,
        &format!(
            ".work-review-rollback-stage-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ),
    )?;
    let prep: Result<(), AppError> = (|| {
        std::fs::copy(
            safety_dir.join("workreview.db"),
            rollback_stage.join("workreview.db"),
        )?;
        std::fs::copy(
            safety_dir.join("config.json"),
            rollback_stage.join("config.json"),
        )?;
        let safety_shots = safety_dir.join("screenshots");
        if safety_shots.is_dir() {
            copy_regular_dir_files_recursive(&safety_shots, &rollback_stage.join("screenshots"))?;
        } else {
            let live_shots = live_canonical.join("screenshots");
            if live_shots.is_dir() {
                copy_regular_dir_files_recursive(&live_shots, &rollback_stage.join("screenshots"))?;
            }
        }
        Ok(())
    })();
    if let Err(e) = prep {
        let _ = remove_migration_path(&rollback_stage);
        return Err(e);
    }
    commit_migration_stage(&rollback_stage, live_canonical, RESTORE_PRESERVED_ENTRIES)
}

/// 纯文件系统与状态级恢复流程（便于单元/集成测试，不触发重启）。
#[cfg(test)]
pub(crate) fn restore_backup_into_data_dir(
    backup_dir: &Path,
    live_data_dir: &Path,
    live_database: &Database,
    live_config: &AppConfig,
) -> Result<(RestoreBackupResult, AppConfig), AppError> {
    let prepared =
        prepare_restore_backup_stage(backup_dir, live_data_dir, live_database, live_config)?;

    live_database.release_file_handles()?;
    commit_migration_stage(
        &prepared.stage_dir,
        &prepared.live_canonical,
        RESTORE_PRESERVED_ENTRIES,
    )?;

    Ok((
        RestoreBackupResult {
            restored_from: path_for_display(&prepared.canonical_backup),
            safety_backup_dir: path_for_display(&prepared.safety_dir),
            restored_files: prepared.restored_files,
            schema_version: prepared.schema_version,
        },
        prepared.restored_config,
    ))
}

/// 从备份目录恢复数据：加进程维护互斥锁 → 校验 manifest 与 SHA-256 → 完整安全备份 → 暂存复验与试开库 →
/// 持有 `AppState` 锁执行原子切换与重开库（失败自动从安全备份回滚）→ 重启应用。
#[tauri::command]
pub async fn restore_backup(
    backup_dir: String,
    app: AppHandle,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<RestoreBackupResult, AppError> {
    let trimmed = backup_dir.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config("备份目录不能为空".to_string()));
    }

    let maintenance_guard = try_acquire_data_maintenance_lock()?;

    struct RecordingPauseGuard {
        state: Arc<Mutex<AppState>>,
        previous_paused: bool,
        armed: bool,
    }
    impl RecordingPauseGuard {
        fn disarm(&mut self) {
            self.armed = false;
        }
    }
    impl Drop for RecordingPauseGuard {
        fn drop(&mut self) {
            if self.armed {
                if let Ok(mut guard) = self.state.lock() {
                    guard.is_paused = self.previous_paused;
                }
            }
        }
    }

    let (previous_paused, live_data_dir, live_database, live_config) = {
        let mut guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let prev = guard.is_paused;
        guard.is_paused = true;
        (
            prev,
            guard.data_dir.clone(),
            guard.database.clone(),
            guard.config.clone(),
        )
    };
    let mut pause_guard = RecordingPauseGuard {
        state: state.inner().clone(),
        previous_paused,
        armed: true,
    };

    let backup_path = PathBuf::from(trimmed);
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_restore_backup_stage(&backup_path, &live_data_dir, &live_database, &live_config)
    })
    .await
    .map_err(|e| AppError::Unknown(format!("恢复备份准备任务异常: {e}")))??;

    // 最终切换窗口全程持有 AppState 锁，避免释放句柄后至重开库前有并发请求读写数据库或配置。
    let result = {
        let mut guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        guard.database.release_file_handles()?;

        if let Err(err) = commit_migration_stage(
            &prepared.stage_dir,
            &prepared.live_canonical,
            RESTORE_PRESERVED_ENTRIES,
        ) {
            if let Ok(reopened) = Database::new(&prepared.live_canonical.join("workreview.db")) {
                guard.database = reopened;
            }
            return Err(err);
        }

        match Database::new(&prepared.live_canonical.join("workreview.db")) {
            Ok(reopened_db) => {
                let data_dir = prepared.live_canonical.clone();
                guard.database = reopened_db;
                guard.privacy_filter =
                    PrivacyFilter::from_config(&prepared.restored_config.privacy);
                guard.screenshot_service =
                    ScreenshotService::new(&data_dir, &prepared.restored_config.storage);
                guard
                    .screenshot_service
                    .update_privacy(&prepared.restored_config.privacy);
                guard.storage_manager =
                    StorageManager::new(&data_dir, prepared.restored_config.storage.clone());
                guard.config = prepared.restored_config;
                RestoreBackupResult {
                    restored_from: path_for_display(&prepared.canonical_backup),
                    safety_backup_dir: path_for_display(&prepared.safety_dir),
                    restored_files: prepared.restored_files,
                    schema_version: prepared.schema_version,
                }
            }
            Err(open_err) => {
                // 新库打开失败时自动从安全备份回滚并恢复旧库句柄
                let rollback_note = match rollback_live_data_dir_from_safety(
                    &prepared.safety_dir,
                    &prepared.live_canonical,
                ) {
                    Ok(()) => match Database::new(&prepared.live_canonical.join("workreview.db")) {
                        Ok(old_db) => {
                            guard.database = old_db;
                            "已自动回滚至恢复前数据".to_string()
                        }
                        Err(reopen_old_err) => format!(
                            "已回滚文件但重开旧库失败（{reopen_old_err}），安全备份位于 {}",
                            path_for_display(&prepared.safety_dir)
                        ),
                    },
                    Err(rb_err) => format!(
                        "自动回滚失败（{rb_err}），请从安全备份手动恢复：{}",
                        path_for_display(&prepared.safety_dir)
                    ),
                };
                return Err(AppError::Config(format!(
                    "切换后打开恢复数据库失败（{open_err}）；{rollback_note}"
                )));
            }
        }
    };

    // 恢复成功后保持录制暂停与维护互斥锁直到进程重启，避免 600ms 窗口内写入脏数据。
    pause_guard.disarm();
    maintenance_guard.keep_locked_until_exit();

    let app_for_restart = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        app_for_restart.restart();
    });

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "work-review-backup-test-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn csv转义应符合rfc4180并防御公式注入() {
        assert_eq!(escape_csv_cell("normal"), "normal");
        assert_eq!(escape_csv_cell("a,b"), "\"a,b\"");
        assert_eq!(escape_csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(escape_csv_cell("=CMD()"), "'=CMD()");
        assert_eq!(escape_csv_cell("+1+1"), "'+1+1");
        assert_eq!(escape_csv_cell("-2+3"), "'-2+3");
        assert_eq!(escape_csv_cell("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(
            escape_csv_cell(" =HYPERLINK(\"x\")"),
            "\"' =HYPERLINK(\"\"x\"\")\""
        );
        // 制表符、换行、全角空白/混合空白后的公式字符同样必须加前导撇号防护
        assert!(escape_csv_cell("\t=CMD()").starts_with("'\t"));
        assert!(escape_csv_cell("\n=CMD()").starts_with("\"'\n"));
        assert!(escape_csv_cell("\u{3000}=CMD()").starts_with("'\u{3000}"));
        assert!(escape_csv_cell("\x0b+1").starts_with("'\x0b"));
    }

    #[test]
    fn 备份与恢复往返应保持数据一致并拦截篡改重复条目与不兼容schema() {
        let root = temp_test_dir("roundtrip");
        let live_dir = root.join("live");
        let export_parent = root.join("backups");
        std::fs::create_dir_all(live_dir.join("screenshots")).unwrap();
        std::fs::create_dir_all(&export_parent).unwrap();
        std::fs::write(
            live_dir.join("screenshots/2026-11-20.jpg"),
            b"fake-jpeg-bytes",
        )
        .unwrap();
        // 模拟 macOS/系统隐藏文件，备份必须跳过点文件且自检通过
        std::fs::write(live_dir.join("screenshots/.DS_Store"), b"finder-meta").unwrap();

        let db = Database::new(&live_dir.join("workreview.db")).unwrap();
        let act = Activity {
            id: None,
            timestamp: 1_760_000_000,
            app_name: "VS Code".to_string(),
            window_title: "backup.rs".to_string(),
            screenshot_path: String::new(),
            ocr_text: None,
            category: "development".to_string(),
            duration: 180,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };
        db.insert_activity(&act).unwrap();

        let mut cfg = AppConfig::default();
        cfg.text_model.api_key = Some("sk-secret-local-only-key".to_string());
        cfg.theme = "dark".to_string();
        cfg.save(&live_dir.join("config.json")).unwrap();

        // 禁止把备份建在数据目录内部或 screenshots/ 内
        assert!(create_backup_from_parts(&db, &cfg, &live_dir, &live_dir, false, false).is_err());
        assert!(create_backup_from_parts(
            &db,
            &cfg,
            &live_dir,
            &live_dir.join("screenshots"),
            true,
            false
        )
        .is_err());

        let created =
            create_backup_from_parts(&db, &cfg, &live_dir, &export_parent, true, false).unwrap();
        let backup_path = PathBuf::from(&created.backup_dir);
        assert!(backup_path.join("manifest.json").is_file());
        // 默认备份不应包含密钥
        let backed_up_cfg_text = std::fs::read_to_string(backup_path.join("config.json")).unwrap();
        assert!(!backed_up_cfg_text.contains("sk-secret-local-only-key"));

        // 在备份的 screenshots 目录注入未登记于 manifest 的陌生文件，恢复时绝不能被带入线上目录
        std::fs::write(
            backup_path.join("screenshots/rogue-unverified.txt"),
            b"should-not-be-restored",
        )
        .unwrap();

        // 构造一个不同的目标目录进行恢复
        let restore_target = root.join("target-live");
        std::fs::create_dir_all(restore_target.join("screenshots")).unwrap();
        std::fs::write(
            restore_target.join("screenshots/original.jpg"),
            b"original-shot",
        )
        .unwrap();
        let target_db = Database::new(&restore_target.join("workreview.db")).unwrap();
        let mut target_cfg = AppConfig::default();
        target_cfg.text_model.api_key = Some("sk-keep-my-target-key".to_string());
        target_cfg
            .save(&restore_target.join("config.json"))
            .unwrap();

        let (restored, restored_cfg) =
            restore_backup_into_data_dir(&backup_path, &restore_target, &target_db, &target_cfg)
                .unwrap();
        assert_eq!(restored.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(restored_cfg.theme, "dark");
        // 相同 provider/endpoint 且备份不含密钥时保留目标设备原有密钥
        assert_eq!(
            restored_cfg.text_model.api_key.as_deref(),
            Some("sk-keep-my-target-key")
        );
        // 仅 manifest 登记的截图被恢复，未登记的陌生文件不会被带入
        assert!(restore_target.join("screenshots/2026-11-20.jpg").is_file());
        assert!(!restore_target
            .join("screenshots/rogue-unverified.txt")
            .exists());

        // 恢复前安全备份必须是完整可校验的备份包（含 manifest.json 与原截图）
        let safety_path = PathBuf::from(&restored.safety_backup_dir);
        let (_, safety_manifest) = verify_backup_dir(&safety_path).unwrap();
        assert!(safety_manifest.includes_screenshots);
        assert!(safety_path.join("screenshots/original.jpg").is_file());

        drop(target_db);
        let reopened = Database::new(&restore_target.join("workreview.db")).unwrap();
        let loaded = reopened.get_activities_in_range(None, None, 10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].window_title, "backup.rs");

        // 备份清单包含重复条目时必须拒绝恢复
        let orig_manifest: BackupManifest = serde_json::from_str(
            &std::fs::read_to_string(backup_path.join("manifest.json")).unwrap(),
        )
        .unwrap();
        let mut dup_manifest = orig_manifest.clone();
        dup_manifest.files.push(orig_manifest.files[0].clone());
        std::fs::write(
            backup_path.join("manifest.json"),
            serde_json::to_string_pretty(&dup_manifest).unwrap(),
        )
        .unwrap();
        let dup_err = verify_backup_dir(&backup_path).unwrap_err().to_string();
        assert!(dup_err.contains("重复条目"));

        // 恢复正常 manifest 后再测篡改与更高 schema 版本
        std::fs::write(
            backup_path.join("manifest.json"),
            serde_json::to_string_pretty(&orig_manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(backup_path.join("config.json"), b"{}").unwrap();
        let tamper_err = verify_backup_dir(&backup_path).unwrap_err().to_string();
        assert!(tamper_err.contains("哈希校验失败"));

        let mut bad_manifest = orig_manifest;
        bad_manifest.schema_version = CURRENT_SCHEMA_VERSION + 10;
        std::fs::write(
            backup_path.join("manifest.json"),
            serde_json::to_string_pretty(&bad_manifest).unwrap(),
        )
        .unwrap();
        let version_err = verify_backup_dir(&backup_path).unwrap_err().to_string();
        assert!(version_err.contains("高于当前程序支持版本"));

        drop(db);
        drop(reopened);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 恢复与回滚应原位保留个人技能自主任务及mcp凭证() {
        let root = temp_test_dir("preserve-harness");
        let live = root.join("live");
        let source = root.join("source");
        let backups = root.join("backups");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::create_dir_all(live.join("skills/weekly/assets/empty")).unwrap();
        let files = [
            ("skills/weekly/SKILL.md", "# 私人技能"),
            ("skills/weekly/assets/.private", "hidden skill asset"),
            ("agent_routines.json", r#"[{"id":"local-routine"}]"#),
            ("mcp_clients.json", r#"[{"authToken":"local-only-token"}]"#),
            ("localhost_api_token.txt", "local-api-token"),
            ("update_settings.json", r#"{"channel":"rc"}"#),
        ];
        for (name, content) in files {
            std::fs::write(live.join(name), content).unwrap();
        }
        let source_db = Database::new(&source.join("workreview.db")).unwrap();
        let source_cfg = AppConfig {
            theme: "dark".into(),
            ..Default::default()
        };
        let backup =
            create_backup_from_parts(&source_db, &source_cfg, &source, &backups, false, true)
                .unwrap();
        let live_db = Database::new(&live.join("workreview.db")).unwrap();
        let live_cfg = AppConfig {
            theme: "light".into(),
            ..Default::default()
        };
        live_cfg.save(&live.join("config.json")).unwrap();
        let (restored, cfg) =
            restore_backup_into_data_dir(Path::new(&backup.backup_dir), &live, &live_db, &live_cfg)
                .unwrap();
        assert_eq!(cfg.theme, "dark");
        for (name, content) in files {
            assert_eq!(std::fs::read_to_string(live.join(name)).unwrap(), content);
            assert!(!Path::new(&backup.backup_dir).join(name).exists());
        }
        assert!(live.join("skills/weekly/assets/empty").is_dir());

        rollback_live_data_dir_from_safety(Path::new(&restored.safety_backup_dir), &live).unwrap();
        let rolled_back: AppConfig =
            serde_json::from_slice(&std::fs::read(live.join("config.json")).unwrap()).unwrap();
        assert_eq!(rolled_back.theme, "light");
        for (name, content) in files {
            assert_eq!(std::fs::read_to_string(live.join(name)).unwrap(), content);
        }
        assert!(live.join("skills/weekly/assets/empty").is_dir());
        drop(source_db);
        drop(live_db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn 端点变更时回填密钥不得泄漏本机旧密钥到新端点() {
        use work_review_core::config::AiProvider;

        let mut live = AppConfig::default();
        live.text_model.provider = AiProvider::OpenAI;
        live.text_model.endpoint = "https://api.example.com/TenantA".to_string();
        live.text_model.api_key = Some("sk-openai-live-secret".to_string());
        live.remote_storage.s3.endpoint = "https://s3.a.example.com".to_string();
        live.remote_storage.s3.bucket = "bucket-a".to_string();
        live.remote_storage.s3.access_key = "AKIA-LIVE-A".to_string();
        live.remote_storage.s3.secret_key = "s3-secret-a".to_string();

        // 1. 路径大小写不同（/TenantA vs /tenanta）视为不同多租户端点，不得回填密钥
        let mut restored_other_case_path = AppConfig::default();
        restored_other_case_path.text_model.provider = AiProvider::OpenAI;
        restored_other_case_path.text_model.endpoint =
            "https://api.example.com/tenanta".to_string();
        // 2. S3 同桶同端点但 access_key 变更，不得将本机 secret_key 回填给不同账户
        restored_other_case_path.remote_storage.s3.endpoint =
            "https://s3.a.example.com".to_string();
        restored_other_case_path.remote_storage.s3.bucket = "bucket-a".to_string();
        restored_other_case_path.remote_storage.s3.access_key = "AKIA-OTHER-B".to_string();

        restored_other_case_path.preserve_missing_secrets_from(&live);
        assert!(restored_other_case_path.text_model.api_key.is_none());
        assert!(restored_other_case_path
            .remote_storage
            .s3
            .secret_key
            .is_empty());

        // 3. scheme/host 大小写不同但路径严格一致，且脱敏备份清空了 S3 成对凭据时应成对回填
        let mut restored_same = AppConfig::default();
        restored_same.text_model.provider = AiProvider::OpenAI;
        restored_same.text_model.endpoint = "HTTPS://API.EXAMPLE.COM/TenantA/".to_string();
        restored_same.remote_storage.s3.endpoint = "HTTPS://S3.A.EXAMPLE.COM/".to_string();
        restored_same.remote_storage.s3.bucket = "bucket-a".to_string();

        restored_same.preserve_missing_secrets_from(&live);
        assert_eq!(
            restored_same.text_model.api_key.as_deref(),
            Some("sk-openai-live-secret")
        );
        assert_eq!(restored_same.remote_storage.s3.access_key, "AKIA-LIVE-A");
        assert_eq!(restored_same.remote_storage.s3.secret_key, "s3-secret-a");
    }

    #[test]
    fn 活动流式导出应过滤隐私跳过记录并支持跨页完整输出() {
        let root = temp_test_dir("stream-export");
        let db = Database::new(&root.join("workreview.db")).unwrap();
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let base_ts = chrono::TimeZone::from_local_datetime(
            &chrono::Local,
            &day.and_hms_opt(10, 0, 0).unwrap(),
        )
        .earliest()
        .unwrap()
        .timestamp();

        let make_act = |offset: i64, app: &str, title: &str| Activity {
            id: None,
            timestamp: base_ts + offset,
            app_name: app.to_string(),
            window_title: title.to_string(),
            screenshot_path: "/secret/local/path.jpg".to_string(),
            ocr_text: Some("ocr-secret".to_string()),
            category: "development".to_string(),
            duration: 60,
            browser_url: Some("https://example.com/work".to_string()),
            executable_path: Some("/usr/bin/code".to_string()),
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: Some("https://signed.example.com/s.jpg".to_string()),
        };

        // 5 条正常记录 + 1 条关键词脱敏（保留时长但剥离标题）+ 2 条隐私 Skip（无痕窗口 + 忽略应用）
        db.insert_activity(&make_act(1, "VS Code", "task-1"))
            .unwrap();
        db.insert_activity(&make_act(2, "Chrome", "Payroll Secret Sheet"))
            .unwrap();
        db.insert_activity(&make_act(3, "VS Code", "task-2"))
            .unwrap();
        db.insert_activity(&make_act(4, "Chrome", "GitHub - InPrivate"))
            .unwrap();
        db.insert_activity(&make_act(5, "1Password", "Vault Master"))
            .unwrap();
        db.insert_activity(&make_act(6, "VS Code", "task-3"))
            .unwrap();
        db.insert_activity(&make_act(7, "VS Code", "task-4"))
            .unwrap();
        db.insert_activity(&make_act(8, "VS Code", "task-5"))
            .unwrap();

        let privacy = PrivacyConfig {
            app_rules: vec![work_review_core::config::AppPrivacyRule {
                app_name: "1Password".to_string(),
                level: work_review_core::config::PrivacyLevel::Ignored,
            }],
            excluded_keywords: vec!["Payroll Secret".to_string()],
            skip_private_windows: true,
            ..PrivacyConfig::default()
        };

        // page_size = 2 验证跨多页流式写出；2 条 Skip 被彻底排除，共导出 6 条
        let csv_res = export_activities_to_dir(
            &db,
            &ExportActivitiesRequest {
                from_str: "2026-11-20",
                to_str: "2026-11-20",
                format: DataExportFormat::Csv,
                include_ocr: false,
                ignored_apps: &[],
                excluded_domains: &[],
                privacy_config: &privacy,
                export_dir: &root,
                page_size: 2,
            },
        )
        .unwrap();
        assert_eq!(csv_res.count, 6);
        let csv_text = std::fs::read_to_string(&csv_res.path).unwrap();
        assert!(!csv_text.contains("Payroll Secret"));
        assert!(!csv_text.contains("InPrivate"));
        assert!(!csv_text.contains("1Password"));
        assert!(!csv_text.contains("ocr-secret"));
        assert!(csv_text.contains("task-1") && csv_text.contains("task-5"));

        let json_res = export_activities_to_dir(
            &db,
            &ExportActivitiesRequest {
                from_str: "2026-11-20",
                to_str: "2026-11-20",
                format: DataExportFormat::Json,
                include_ocr: false,
                ignored_apps: &[],
                excluded_domains: &[],
                privacy_config: &privacy,
                export_dir: &root,
                page_size: 2,
            },
        )
        .unwrap();
        assert_eq!(json_res.count, 6);
        let json_text = std::fs::read_to_string(&json_res.path).unwrap();
        let parsed: Vec<Activity> = serde_json::from_str(&json_text).unwrap();
        assert_eq!(parsed.len(), 6);
        assert!(parsed.iter().all(|a| a.screenshot_path.is_empty()
            && a.executable_path.is_none()
            && a.screenshot_url.is_none()
            && a.ocr_text.is_none()));
        assert!(!json_text.contains("Payroll Secret"));
        assert!(!json_text.contains("InPrivate"));
        assert!(!json_text.contains("1Password"));

        drop(db);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 原子替换导出文件在覆盖成功与失败时均不丢失数据且不残留临时文件() {
        let root = temp_test_dir("atomic-export");
        let path = write_export_file_atomic(&root, "report.csv", b"v1").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"v1");

        // 覆盖已有文件成功
        write_export_file_atomic(&root, "report.csv", b"v2").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"v2");

        // 同名目录不可被常规文件替换：必须返回错误、保留原目录并清理临时文件
        let blocked_dir = root.join("blocked-target");
        std::fs::create_dir(&blocked_dir).unwrap();
        std::fs::write(blocked_dir.join("keep.txt"), b"keep").unwrap();
        assert!(write_export_file_atomic(&root, "blocked-target", b"v3").is_err());
        assert!(blocked_dir.join("keep.txt").is_file());
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 数据维护互斥锁应拦截并发进入并在释放后恢复可用() {
        let g1 = try_acquire_data_maintenance_lock().expect("first lock");
        assert!(try_acquire_data_maintenance_lock().is_err());
        drop(g1);
        let g2 = try_acquire_data_maintenance_lock().expect("lock after drop");
        drop(g2);
    }
}
