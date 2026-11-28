//! 自主定时任务（Routines / Heartbeat）配置、到期判定与原子持久化。
//!
//! 设计约束：
//! - 持久化于 `<data_dir>/agent_routines.json`，最多 16 条，采用同目录临时文件 + 原子重命名。
//! - 后台触发始终在只读沙箱通道运行（无写操作桥、无外部 MCP），仅生成复盘摘要并写入 SQLite Run 账本。

use crate::ledger::scrub_preview;
use crate::skills::is_valid_skill_name;
use chrono::{DateTime, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static ROUTINES_IO_LOCK: Mutex<()> = Mutex::new(());

pub const ROUTINES_FILE_NAME: &str = "agent_routines.json";
const MAX_ROUTINES: usize = 16;
const MAX_ROUTINES_FILE_BYTES: u64 = 128 * 1024;
const MIN_INTERVAL_MINUTES: u32 = 15;
const MAX_INTERVAL_MINUTES: u32 = 1440;

/// 自主任务的触发策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoutineSchedule {
    /// 每天在本地指定时刻（小时 0-23，分钟 0-59）执行一次。
    DailyAt { hour: u8, minute: u8 },
    /// 按固定间隔（15-1440 分钟）周期执行。
    IntervalMinutes { every_minutes: u32 },
    /// 仅手动触发，不参与后台定时轮询。
    ManualOnly,
}

/// 单条个人 Agent 自主任务定义与最近执行快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRoutine {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub enabled: bool,
    /// 是否允许后台定时任务调用远程云模型（默认 false：仅走本地模型或本地统计引擎，严禁静默外发工作记录）。
    #[serde(default)]
    pub allow_cloud_model: bool,
    pub schedule: RoutineSchedule,
    pub skill_name: Option<String>,
    pub last_run_at: Option<i64>,
    pub last_run_id: Option<String>,
    pub last_status: Option<String>,
    pub last_summary: Option<String>,
}

/// 创建/更新自主任务的输入结构。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRoutineInput {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub enabled: bool,
    #[serde(default)]
    pub allow_cloud_model: bool,
    pub schedule: RoutineSchedule,
    pub skill_name: Option<String>,
}

fn routines_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ROUTINES_FILE_NAME)
}

fn sanitize_single_line(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

fn normalize_schedule(schedule: RoutineSchedule) -> Result<RoutineSchedule, String> {
    match schedule {
        RoutineSchedule::DailyAt { hour, minute } => {
            if hour > 23 || minute > 59 {
                return Err("每日执行时间无效（小时需在 0-23，分钟需在 0-59）".to_string());
            }
            Ok(RoutineSchedule::DailyAt { hour, minute })
        }
        RoutineSchedule::IntervalMinutes { every_minutes } => {
            if !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(&every_minutes) {
                return Err(format!(
                    "执行间隔需在 {MIN_INTERVAL_MINUTES} 到 {MAX_INTERVAL_MINUTES} 分钟之间"
                ));
            }
            Ok(RoutineSchedule::IntervalMinutes { every_minutes })
        }
        RoutineSchedule::ManualOnly => Ok(RoutineSchedule::ManualOnly),
    }
}

/// 判断单条自主任务在给定本地时间点是否到期。
pub fn is_routine_due(routine: &AgentRoutine, now: DateTime<Local>) -> bool {
    if !routine.enabled {
        return false;
    }
    match routine.schedule {
        RoutineSchedule::ManualOnly => false,
        RoutineSchedule::IntervalMinutes { every_minutes } => {
            let interval_secs = (every_minutes.clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES)
                as i64)
                .saturating_mul(60);
            match routine.last_run_at {
                None => true,
                Some(last_ts) => now.timestamp().saturating_sub(last_ts) >= interval_secs,
            }
        }
        RoutineSchedule::DailyAt { hour, minute } => {
            let current_minutes = now.hour() * 60 + now.minute();
            let target_minutes = (hour as u32) * 60 + (minute as u32);
            if current_minutes < target_minutes {
                return false;
            }
            match routine.last_run_at {
                None => true,
                Some(last_ts) => match Local.timestamp_opt(last_ts, 0).earliest() {
                    Some(last_dt) => last_dt.date_naive() < now.date_naive(),
                    None => true,
                },
            }
        }
    }
}

/// 从 `<data_dir>/agent_routines.json` 安全加载全部自主任务（不存在时返回空列表）。
pub fn load_routines_from_dir(data_dir: &Path) -> Vec<AgentRoutine> {
    let path = routines_path(data_dir);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return Vec::new();
    };
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_ROUTINES_FILE_BYTES {
        return Vec::new();
    }
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(mut items) = serde_json::from_str::<Vec<AgentRoutine>>(&raw) else {
        return Vec::new();
    };
    items.retain(|item| is_valid_skill_name(&item.id) && !item.prompt.trim().is_empty());
    items.truncate(MAX_ROUTINES);
    items
}

fn write_routines_atomic(data_dir: &Path, routines: &[AgentRoutine]) -> Result<(), String> {
    if let Ok(dir_meta) = fs::symlink_metadata(data_dir) {
        if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
            return Err("数据目录为符号链接或非法路径，拒绝写入自主任务配置".to_string());
        }
    } else {
        fs::create_dir_all(data_dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }

    let path = routines_path(data_dir);
    if let Ok(file_meta) = fs::symlink_metadata(&path) {
        if file_meta.file_type().is_symlink() || !file_meta.is_file() {
            return Err("自主任务配置文件为符号链接或非普通文件，拒绝覆盖".to_string());
        }
    }

    let payload =
        serde_json::to_string_pretty(routines).map_err(|e| format!("序列化自主任务失败: {e}"))?;
    if payload.len() as u64 > MAX_ROUTINES_FILE_BYTES {
        return Err("自主任务配置文件体积超出上限".to_string());
    }

    let tmp = data_dir.join(format!(
        ".{ROUTINES_FILE_NAME}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let write_res = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(payload.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp);
        return Err(format!("写入自主任务临时文件失败: {err}"));
    }
    if let Err(err) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("提交自主任务配置失败: {err}"));
    }
    Ok(())
}

/// 创建或更新单条自主任务（保留已有执行记录字段），返回最新列表。
pub fn save_routine_in_dir(
    data_dir: &Path,
    input: SaveRoutineInput,
) -> Result<Vec<AgentRoutine>, String> {
    let _io_guard = ROUTINES_IO_LOCK
        .lock()
        .map_err(|_| "自主任务配置锁不可用".to_string())?;
    let id = input.id.trim().to_string();
    if !is_valid_skill_name(&id) {
        return Err("任务标识仅支持 1-64 位字母、数字、连字符或下划线".to_string());
    }
    let name = sanitize_single_line(&input.name, 80);
    if name.is_empty() {
        return Err("任务名称不能为空".to_string());
    }
    let prompt: String = input
        .prompt
        .replace("\r\n", "\n")
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .collect();
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() || prompt.chars().count() > 2_000 {
        return Err("任务提示词不能为空且不能超过 2000 字符".to_string());
    }
    let schedule = normalize_schedule(input.schedule)?;
    let skill_name = match input.skill_name.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(skill) if is_valid_skill_name(skill) => Some(skill.to_string()),
        Some(_) => return Err("关联个人技能名称非法".to_string()),
    };

    let mut routines = load_routines_from_dir(data_dir);
    if let Some(existing) = routines.iter_mut().find(|r| r.id == id) {
        existing.name = name;
        existing.prompt = prompt;
        existing.enabled = input.enabled;
        existing.allow_cloud_model = input.allow_cloud_model;
        existing.schedule = schedule;
        existing.skill_name = skill_name;
    } else {
        if routines.len() >= MAX_ROUTINES {
            return Err(format!("最多支持配置 {MAX_ROUTINES} 条自主任务"));
        }
        routines.push(AgentRoutine {
            id,
            name,
            prompt,
            enabled: input.enabled,
            allow_cloud_model: input.allow_cloud_model,
            schedule,
            skill_name,
            last_run_at: None,
            last_run_id: None,
            last_status: None,
            last_summary: None,
        });
    }

    write_routines_atomic(data_dir, &routines)?;
    Ok(routines)
}

/// 删除指定自主任务，返回剩余列表。
pub fn delete_routine_in_dir(data_dir: &Path, id: &str) -> Result<Vec<AgentRoutine>, String> {
    let _io_guard = ROUTINES_IO_LOCK
        .lock()
        .map_err(|_| "自主任务配置锁不可用".to_string())?;
    let trimmed = id.trim();
    if !is_valid_skill_name(trimmed) {
        return Err("任务标识非法".to_string());
    }
    let mut routines = load_routines_from_dir(data_dir);
    let before = routines.len();
    routines.retain(|r| r.id != trimmed);
    if routines.len() == before {
        return Err(format!("未找到自主任务: {trimmed}"));
    }
    write_routines_atomic(data_dir, &routines)?;
    Ok(routines)
}

/// 记录某条自主任务的最新执行结果（落盘脱敏摘要）。
pub fn record_routine_execution(
    data_dir: &Path,
    id: &str,
    run_id: Option<&str>,
    status: &str,
    summary: Option<&str>,
    ran_at: i64,
) -> Result<(), String> {
    let _io_guard = ROUTINES_IO_LOCK
        .lock()
        .map_err(|_| "自主任务配置锁不可用".to_string())?;
    let mut routines = load_routines_from_dir(data_dir);
    let Some(routine) = routines.iter_mut().find(|r| r.id == id) else {
        return Ok(());
    };
    routine.last_run_at = Some(ran_at);
    routine.last_run_id = run_id
        .map(|s| sanitize_single_line(s, 64))
        .filter(|s| !s.is_empty());
    routine.last_status = Some(sanitize_single_line(status, 32));
    routine.last_summary = summary.map(|s| scrub_preview(s, 240));
    write_routines_atomic(data_dir, &routines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn local_dt(year: i32, month: u32, day: u32, hour: u32, min: u32) -> DateTime<Local> {
        let naive = NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, min, 0)
            .unwrap();
        Local.from_local_datetime(&naive).earliest().unwrap()
    }

    #[test]
    fn 每日定时与固定间隔到期判定应准确且不重复触发() {
        let mut daily = AgentRoutine {
            id: "evening-review".into(),
            name: "每日复盘".into(),
            prompt: "汇总今日工作".into(),
            enabled: true,
            allow_cloud_model: false,
            schedule: RoutineSchedule::DailyAt {
                hour: 18,
                minute: 30,
            },
            skill_name: None,
            last_run_at: None,
            last_run_id: None,
            last_status: None,
            last_summary: None,
        };

        assert!(!is_routine_due(&daily, local_dt(2026, 11, 27, 18, 29)));
        assert!(is_routine_due(&daily, local_dt(2026, 11, 27, 18, 30)));

        // 当天已跑过后不再重复触发，次日到点再次触发
        daily.last_run_at = Some(local_dt(2026, 11, 27, 18, 31).timestamp());
        assert!(!is_routine_due(&daily, local_dt(2026, 11, 27, 20, 0)));
        assert!(is_routine_due(&daily, local_dt(2026, 11, 28, 18, 30)));

        let mut interval = daily.clone();
        interval.schedule = RoutineSchedule::IntervalMinutes { every_minutes: 60 };
        interval.last_run_at = Some(local_dt(2026, 11, 27, 10, 0).timestamp());
        assert!(!is_routine_due(&interval, local_dt(2026, 11, 27, 10, 59)));
        assert!(is_routine_due(&interval, local_dt(2026, 11, 27, 11, 0)));
    }

    #[test]
    fn 自主任务增删改查与执行记录应原子持久化并校验参数() {
        let root = std::env::temp_dir().join(format!("wr-routines-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();

        assert!(save_routine_in_dir(
            &root,
            SaveRoutineInput {
                id: "../bad".into(),
                name: "x".into(),
                prompt: "p".into(),
                enabled: true,
                allow_cloud_model: false,
                schedule: RoutineSchedule::ManualOnly,
                skill_name: None,
            }
        )
        .is_err());

        let saved = save_routine_in_dir(
            &root,
            SaveRoutineInput {
                id: "daily-wrapup".into(),
                name: "下班前复盘".into(),
                prompt: "汇总今日完成事项和未完待办".into(),
                enabled: true,
                allow_cloud_model: false,
                schedule: RoutineSchedule::DailyAt {
                    hour: 18,
                    minute: 30,
                },
                skill_name: Some("weekly-obsidian".into()),
            },
        )
        .expect("创建自主任务应成功");
        assert_eq!(saved.len(), 1);
        assert!(!saved[0].allow_cloud_model);
        assert_eq!(saved[0].skill_name.as_deref(), Some("weekly-obsidian"));

        record_routine_execution(
            &root,
            "daily-wrapup",
            Some("run_123"),
            "completed",
            Some("今日主要完成 Personal Agent 升级"),
            1_700_000_000,
        )
        .expect("记录执行结果应成功");
        let loaded = load_routines_from_dir(&root);
        assert_eq!(loaded[0].last_run_id.as_deref(), Some("run_123"));
        assert_eq!(loaded[0].last_status.as_deref(), Some("completed"));

        let remaining = delete_routine_in_dir(&root, "daily-wrapup").expect("删除应成功");
        assert!(remaining.is_empty());

        let _ = fs::remove_dir_all(&root);
    }
}
