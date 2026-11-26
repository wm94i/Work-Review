use crate::analysis::{format_duration, AppLocale};
use crate::database::Activity;
use crate::project::{ProjectDefinition, ProjectResolver, ProjectUsage, UNASSIGNED_PROJECT_KEY};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

const SESSION_GAP_SECONDS: i64 = 15 * 60;
const DEEP_WORK_THRESHOLD_SECONDS: i64 = 45 * 60;
const TODO_TEXT_LIMIT: usize = 80;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NamedDuration {
    pub name: String,
    pub duration: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IntentSummary {
    pub label: String,
    pub duration: i64,
    pub session_count: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WorkSession {
    pub session_id: String,
    pub date: String,
    pub start_timestamp: i64,
    pub end_timestamp: i64,
    pub duration: i64,
    pub activity_count: usize,
    pub app_count: usize,
    pub dominant_app: String,
    pub dominant_category: String,
    pub title: String,
    pub browser_domains: Vec<String>,
    pub top_apps: Vec<NamedDuration>,
    pub top_keywords: Vec<String>,
    pub intent_label: String,
    pub intent_confidence: i32,
    pub intent_evidence: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IntentAnalysisResult {
    pub sessions: Vec<WorkSession>,
    pub summary: Vec<IntentSummary>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WeeklyReviewResult {
    pub title: String,
    pub markdown: String,
    pub total_duration: i64,
    pub active_days: usize,
    pub session_count: usize,
    pub deep_work_sessions: usize,
    pub top_intents: Vec<IntentSummary>,
    pub top_apps: Vec<NamedDuration>,
    pub highlights: Vec<String>,
    pub risks: Vec<String>,
}

/// 周期复盘粒度：周报或月报（M3-6）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReviewGranularity {
    Week,
    Month,
}

/// 月报中的按周投入趋势桶。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WeeklyEffortBucket {
    pub week_key: String,
    pub date_from: String,
    pub date_to: String,
    pub duration: i64,
    pub active_days: usize,
}

/// 与上一周期（上周 / 上月）的环比指标。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeriodComparisonDelta {
    pub prev_total_duration: i64,
    pub duration_delta: i64,
    pub duration_delta_pct: Option<f64>,
    pub prev_active_days: usize,
    pub prev_deep_work_sessions: usize,
}

/// 周期复盘结构化结果（周报 / 月报共用，支持四语本地化 Markdown）。
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PeriodReviewResult {
    pub granularity: ReviewGranularity,
    pub period_key: String,
    pub date_from: String,
    pub date_to: String,
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub total_duration: i64,
    pub active_days: usize,
    pub session_count: usize,
    pub deep_work_sessions: usize,
    pub top_intents: Vec<IntentSummary>,
    pub top_apps: Vec<NamedDuration>,
    pub project_usage: Vec<ProjectUsage>,
    pub weekly_buckets: Vec<WeeklyEffortBucket>,
    pub comparison: Option<PeriodComparisonDelta>,
    pub highlights: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub title: String,
    pub date: String,
    pub source_title: String,
    pub source_app: String,
    pub confidence: i32,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TodoExtractionResult {
    pub items: Vec<TodoItem>,
    pub summary: String,
}

/// 高频短时打断源统计（如频繁弹出切入的聊天/邮件窗口）
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InterrupterStat {
    pub app_name: String,
    pub count: usize,
    pub total_duration: i64,
}

/// 专注度与上下文切换指标
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FocusMetrics {
    /// 纳入计算的活跃总时长（秒）
    pub total_duration: i64,
    /// 最长连续专注段时长（秒）
    pub longest_focus_seconds: i64,
    /// 平均专注段时长（秒）
    pub average_focus_seconds: i64,
    /// 深度专注段次数（≥ 25 分钟）
    pub deep_focus_sessions: usize,
    /// 上下文切换次数（同段工作内跨应用切换）
    pub context_switches: usize,
    /// 每小时切换频次（四舍五入）
    pub switches_per_hour: u32,
    /// 综合专注度评分（0–100）
    pub focus_score: u8,
    /// 高频短时打断源（最多前 3 个）
    pub top_interrupters: Vec<InterrupterStat>,
}

const FOCUS_STREAK_GAP_SECONDS: i64 = 180;
const FOCUS_SWITCH_GAP_SECONDS: i64 = 30 * 60;
const DEEP_FOCUS_MIN_SECONDS: i64 = 25 * 60;
const SHORT_INTERRUPTION_MAX_SECONDS: i64 = 180;

fn is_focus_work_category(category: &str) -> bool {
    matches!(
        category.trim().to_ascii_lowercase().as_str(),
        "development" | "office" | "design" | "browser" | "other" | ""
    )
}

/// 基于隐私过滤后的活动流计算专注度、上下文切换与高频打断源指标。
pub fn compute_focus_metrics(activities: &[Activity]) -> FocusMetrics {
    if activities.is_empty() {
        return FocusMetrics::default();
    }

    struct Slice {
        app_display: String,
        app_key: String,
        category: String,
        start: i64,
        end: i64,
        duration: i64,
    }

    let mut slices: Vec<Slice> = activities
        .iter()
        .filter(|a| a.duration > 0)
        .map(|a| {
            let dur = a.duration.max(1);
            let display = crate::categorize::normalize_display_app_name(&a.app_name);
            let display = if display.trim().is_empty() {
                a.app_name.trim().to_string()
            } else {
                display
            };
            Slice {
                app_key: display.to_lowercase(),
                app_display: display,
                category: a.category.clone(),
                start: a.timestamp - dur,
                end: a.timestamp,
                duration: dur,
            }
        })
        .collect();

    if slices.is_empty() {
        return FocusMetrics::default();
    }
    slices.sort_by_key(|s| (s.start, s.end));

    // 1. 合并相邻同应用连续切片（gap <= 180s），得到每个“停留片段”
    struct Run {
        app_display: String,
        app_key: String,
        category: String,
        start: i64,
        end: i64,
        duration: i64,
    }

    let mut runs: Vec<Run> = Vec::with_capacity(slices.len());
    for slice in slices {
        if let Some(last) = runs.last_mut() {
            let gap = slice.start - last.end;
            if last.app_key == slice.app_key && gap <= FOCUS_STREAK_GAP_SECONDS {
                last.end = last.end.max(slice.end);
                last.duration += slice.duration;
                continue;
            }
        }
        runs.push(Run {
            app_display: slice.app_display,
            app_key: slice.app_key,
            category: slice.category,
            start: slice.start,
            end: slice.end,
            duration: slice.duration,
        });
    }

    let total_duration: i64 = runs.iter().map(|r| r.duration).sum();

    // 2. 统计上下文切换与短时打断源（非主导工作应用短暂停留 <= 180s 且紧邻前后活动）
    let mut context_switches: usize = 0;
    let mut interrupter_map: HashMap<String, (String, usize, i64)> = HashMap::new();
    for idx in 1..runs.len() {
        let prev = &runs[idx - 1];
        let curr = &runs[idx];
        let gap = curr.start - prev.end;
        if curr.app_key != prev.app_key && gap <= FOCUS_SWITCH_GAP_SECONDS {
            context_switches += 1;
            if curr.duration <= SHORT_INTERRUPTION_MAX_SECONDS {
                let entry = interrupter_map
                    .entry(curr.app_key.clone())
                    .or_insert_with(|| (curr.app_display.clone(), 0, 0));
                entry.1 += 1;
                entry.2 += curr.duration;
            }
        }
    }

    let mut top_interrupters: Vec<InterrupterStat> = interrupter_map
        .into_values()
        .filter(|(_, count, _)| *count >= 2)
        .map(|(app_name, count, total_duration)| InterrupterStat {
            app_name,
            count,
            total_duration,
        })
        .collect();
    top_interrupters.sort_by_key(|item| (Reverse(item.count), Reverse(item.total_duration)));
    top_interrupters.truncate(3);

    // 3. 计算连续专注段：非娱乐/非短时聊天打断的工作停留（允许同类工作工具短间隔衔接）
    let mut focus_streaks: Vec<i64> = Vec::new();
    let mut current_streak: i64 = 0;
    let mut current_streak_end: Option<i64> = None;

    for run in &runs {
        let is_work = is_focus_work_category(&run.category);
        if !is_work {
            if current_streak > 0 {
                focus_streaks.push(current_streak);
                current_streak = 0;
                current_streak_end = None;
            }
            continue;
        }
        match current_streak_end {
            Some(prev_end) if run.start - prev_end <= FOCUS_STREAK_GAP_SECONDS => {
                current_streak += run.duration;
                current_streak_end = Some(prev_end.max(run.end));
            }
            _ => {
                if current_streak > 0 {
                    focus_streaks.push(current_streak);
                }
                current_streak = run.duration;
                current_streak_end = Some(run.end);
            }
        }
    }
    if current_streak > 0 {
        focus_streaks.push(current_streak);
    }

    let longest_focus_seconds = focus_streaks.iter().copied().max().unwrap_or(0);
    let average_focus_seconds = if focus_streaks.is_empty() {
        0
    } else {
        focus_streaks.iter().sum::<i64>() / focus_streaks.len() as i64
    };
    let deep_focus_sessions = focus_streaks
        .iter()
        .filter(|&&dur| dur >= DEEP_FOCUS_MIN_SECONDS)
        .count();
    let deep_focus_duration: i64 = focus_streaks
        .iter()
        .filter(|&&dur| dur >= DEEP_FOCUS_MIN_SECONDS)
        .sum();

    // 每小时切换频次（按至少 15 分钟计算避免几分钟样本出现极端值）
    let effective_hours = (total_duration as f64 / 3600.0).max(0.25);
    let switches_per_hour = (context_switches as f64 / effective_hours).round() as u32;

    // 专注度评分（0–100）：深度专注占比（0-50）+ 最长连续专注分（0-25）+ 切换平稳分（0-25）
    let deep_ratio = (deep_focus_duration as f64 / total_duration.max(1) as f64).clamp(0.0, 1.0);
    let deep_component = deep_ratio * 50.0;
    let longest_component = ((longest_focus_seconds as f64 / (60.0 * 60.0)).clamp(0.0, 1.0)) * 25.0;
    let switch_penalty = ((switches_per_hour as f64 - 4.0).max(0.0) / 20.0).clamp(0.0, 1.0);
    let switch_component = (1.0 - switch_penalty) * 25.0;
    let focus_score = (deep_component + longest_component + switch_component)
        .round()
        .clamp(0.0, 100.0) as u8;

    FocusMetrics {
        total_duration,
        longest_focus_seconds,
        average_focus_seconds,
        deep_focus_sessions,
        context_switches,
        switches_per_hour,
        focus_score,
        top_interrupters,
    }
}

/// 将专注度指标格式化为一行紧凑上下文，供 AI 日报提示词参考。
pub fn format_focus_metrics_for_prompt(
    metrics: &FocusMetrics,
    locale: crate::analysis::AppLocale,
) -> String {
    use crate::analysis::{format_duration_for_locale, AppLocale};
    if metrics.total_duration <= 0 {
        return String::new();
    }
    let longest = format_duration_for_locale(metrics.longest_focus_seconds, locale);
    let interrupters = metrics
        .top_interrupters
        .iter()
        .map(|item| format!("{} ({}x)", item.app_name, item.count))
        .collect::<Vec<_>>()
        .join(", ");

    match locale {
        AppLocale::ZhCn => {
            if interrupters.is_empty() {
                format!(
                    "专注度评分 {}/100，最长连续专注 {}，深度专注段 {} 次，应用切换 {} 次（约 {}/小时）",
                    metrics.focus_score,
                    longest,
                    metrics.deep_focus_sessions,
                    metrics.context_switches,
                    metrics.switches_per_hour
                )
            } else {
                format!(
                    "专注度评分 {}/100，最长连续专注 {}，深度专注段 {} 次，应用切换 {} 次（约 {}/小时），高频短时打断源：{}",
                    metrics.focus_score,
                    longest,
                    metrics.deep_focus_sessions,
                    metrics.context_switches,
                    metrics.switches_per_hour,
                    interrupters
                )
            }
        }
        AppLocale::ZhTw => {
            if interrupters.is_empty() {
                format!(
                    "專注度評分 {}/100，最長連續專注 {}，深度專注段 {} 次，應用切換 {} 次（約 {}/小時）",
                    metrics.focus_score,
                    longest,
                    metrics.deep_focus_sessions,
                    metrics.context_switches,
                    metrics.switches_per_hour
                )
            } else {
                format!(
                    "專注度評分 {}/100，最長連續專注 {}，深度專注段 {} 次，應用切換 {} 次（約 {}/小時），高頻短時打斷源：{}",
                    metrics.focus_score,
                    longest,
                    metrics.deep_focus_sessions,
                    metrics.context_switches,
                    metrics.switches_per_hour,
                    interrupters
                )
            }
        }
        AppLocale::En => {
            if interrupters.is_empty() {
                format!(
                    "Focus score {}/100, longest focus streak {}, {} deep-focus block(s), {} context switches (~{}/hr)",
                    metrics.focus_score,
                    longest,
                    metrics.deep_focus_sessions,
                    metrics.context_switches,
                    metrics.switches_per_hour
                )
            } else {
                format!(
                    "Focus score {}/100, longest focus streak {}, {} deep-focus block(s), {} context switches (~{}/hr), top brief interrupters: {}",
                    metrics.focus_score,
                    longest,
                    metrics.deep_focus_sessions,
                    metrics.context_switches,
                    metrics.switches_per_hour,
                    interrupters
                )
            }
        }
        AppLocale::Ar => {
            if interrupters.is_empty() {
                format!(
                    "درجة التركيز {}/100، أطول جلسة تركيز {}، {} جلسات تركيز عميق، {} تبديلات سياق (~{}/ساعة)",
                    metrics.focus_score,
                    longest,
                    metrics.deep_focus_sessions,
                    metrics.context_switches,
                    metrics.switches_per_hour
                )
            } else {
                format!(
                    "درجة التركيز {}/100، أطول جلسة تركيز {}، {} جلسات تركيز عميق، {} تبديلات سياق (~{}/ساعة)، أبرز مصادر المقاطعة: {}",
                    metrics.focus_score,
                    longest,
                    metrics.deep_focus_sessions,
                    metrics.context_switches,
                    metrics.switches_per_hour,
                    interrupters
                )
            }
        }
    }
}

#[derive(Debug, Clone)]
struct IntentMatch {
    label: String,
    score: i32,
    evidence: Vec<String>,
}

#[derive(Default)]
struct SessionAccumulator {
    activities: Vec<Activity>,
    start_timestamp: i64,
    end_timestamp: i64,
    total_duration: i64,
}

impl SessionAccumulator {
    // 全仓库统一语义：activity.timestamp 是记录区间的**终点**，真实区间为
    // [timestamp - duration, timestamp]（与统计裁剪、时间线一致）。
    // 此前按 [timestamp, timestamp+duration] 计算，所有会话整体后移一个
    // duration，起止时间显示错位，跨午夜时 date 归属也会错日。
    fn from_activity(activity: Activity) -> Self {
        let end_timestamp = activity.timestamp;
        let start_timestamp = activity.timestamp - activity.duration.max(1);
        Self {
            activities: vec![activity],
            start_timestamp,
            end_timestamp,
            total_duration: end_timestamp - start_timestamp,
        }
    }

    fn can_merge(&self, next: &Activity) -> bool {
        // next 的区间起点与当前会话终点的间隔;区间重叠时为负值,同样合并
        next.timestamp - next.duration.max(1) - self.end_timestamp <= SESSION_GAP_SECONDS
    }

    fn push(&mut self, activity: Activity) {
        self.end_timestamp = self.end_timestamp.max(activity.timestamp);
        self.total_duration += activity.duration.max(1);
        self.activities.push(activity);
    }

    fn finalize(self, index: usize) -> WorkSession {
        let mut app_durations: HashMap<String, i64> = HashMap::new();
        let mut category_durations: HashMap<String, i64> = HashMap::new();
        let mut domain_durations: HashMap<String, i64> = HashMap::new();
        let mut keyword_counts: HashMap<String, i32> = HashMap::new();
        let mut title_counts: HashMap<String, i32> = HashMap::new();
        let mut combined_lines = Vec::new();

        for activity in &self.activities {
            *app_durations.entry(activity.app_name.clone()).or_insert(0) +=
                activity.duration.max(1);
            *category_durations
                .entry(activity.category.clone())
                .or_insert(0) += activity.duration.max(1);

            if !activity.window_title.trim().is_empty() {
                *title_counts
                    .entry(activity.window_title.trim().to_string())
                    .or_insert(0) += 1;
            }

            if let Some(url) = &activity.browser_url {
                let domain = extract_domain(url);
                if !domain.is_empty() {
                    *domain_durations.entry(domain).or_insert(0) += activity.duration.max(1);
                }
            }

            let mut text_parts = vec![activity.app_name.as_str(), activity.window_title.as_str()];
            if let Some(ocr_text) = &activity.ocr_text {
                text_parts.push(ocr_text.as_str());
            }
            if let Some(url) = &activity.browser_url {
                text_parts.push(url.as_str());
            }

            let joined = text_parts.join(" ");
            combined_lines.push(joined.clone());
            for token in extract_keywords(&joined) {
                *keyword_counts.entry(token).or_insert(0) += 1;
            }
        }

        let dominant_app =
            top_name_by_duration(&app_durations).unwrap_or_else(|| "未知应用".to_string());
        let dominant_category =
            top_name_by_duration(&category_durations).unwrap_or_else(|| "unknown".to_string());
        let title = top_name_by_count(&title_counts).unwrap_or_else(|| dominant_app.clone());
        let browser_domains = top_named_durations(&domain_durations, 4)
            .into_iter()
            .map(|item| item.name)
            .collect::<Vec<_>>();
        let top_apps = top_named_durations(&app_durations, 4);
        let top_keywords = top_keywords(&keyword_counts, 6);
        let intent = classify_session(
            &dominant_category,
            &dominant_app,
            &title,
            &browser_domains,
            &top_keywords,
            &combined_lines,
        );

        WorkSession {
            session_id: format!(
                "{}-{}-{}",
                date_from_timestamp(self.start_timestamp),
                self.start_timestamp,
                index + 1
            ),
            date: date_from_timestamp(self.start_timestamp),
            start_timestamp: self.start_timestamp,
            end_timestamp: self.end_timestamp,
            duration: self.total_duration.max(1),
            activity_count: self.activities.len(),
            app_count: app_durations.len(),
            dominant_app,
            dominant_category,
            title,
            browser_domains,
            top_apps,
            top_keywords,
            intent_label: intent.label,
            intent_confidence: intent.score.clamp(35, 95),
            intent_evidence: intent.evidence,
        }
    }
}

pub fn build_work_sessions(activities: &[Activity]) -> Vec<WorkSession> {
    let mut sorted = activities.to_vec();
    sorted.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));

    let mut sessions = Vec::new();
    let mut current: Option<SessionAccumulator> = None;

    for activity in sorted {
        match current.as_mut() {
            Some(acc) if acc.can_merge(&activity) => acc.push(activity),
            Some(acc) => {
                let finished = std::mem::take(acc);
                sessions.push(finished.finalize(sessions.len()));
                *acc = SessionAccumulator::from_activity(activity);
            }
            None => current = Some(SessionAccumulator::from_activity(activity)),
        }
    }

    if let Some(acc) = current {
        sessions.push(acc.finalize(sessions.len()));
    }

    sessions.sort_by_key(|b| Reverse(b.start_timestamp));
    sessions
}

pub fn analyze_intents(activities: &[Activity]) -> IntentAnalysisResult {
    let sessions = build_work_sessions(activities);
    let mut summary_map: HashMap<String, (i64, usize)> = HashMap::new();

    for session in &sessions {
        let entry = summary_map
            .entry(session.intent_label.clone())
            .or_insert((0, 0));
        entry.0 += session.duration;
        entry.1 += 1;
    }

    let mut summary = summary_map
        .into_iter()
        .map(|(label, (duration, session_count))| IntentSummary {
            label,
            duration,
            session_count,
        })
        .collect::<Vec<_>>();

    summary.sort_by(|a, b| {
        b.duration
            .cmp(&a.duration)
            .then_with(|| b.session_count.cmp(&a.session_count))
            .then_with(|| a.label.cmp(&b.label))
    });

    IntentAnalysisResult { sessions, summary }
}

pub fn generate_weekly_review(
    activities: &[Activity],
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> WeeklyReviewResult {
    let analysis = analyze_intents(activities);
    let total_duration = analysis
        .sessions
        .iter()
        .map(|session| session.duration)
        .sum::<i64>();
    let session_count = analysis.sessions.len();
    let deep_work_sessions = analysis
        .sessions
        .iter()
        .filter(|session| session.duration >= DEEP_WORK_THRESHOLD_SECONDS)
        .count();

    let mut day_durations: HashMap<String, i64> = HashMap::new();
    let mut app_durations: HashMap<String, i64> = HashMap::new();
    let mut context_switch_sessions = 0usize;

    for session in &analysis.sessions {
        *day_durations.entry(session.date.clone()).or_insert(0) += session.duration;
        for item in &session.top_apps {
            *app_durations.entry(item.name.clone()).or_insert(0) += item.duration;
        }
        if session.app_count >= 4 {
            context_switch_sessions += 1;
        }
    }

    let active_days = day_durations.len();
    let top_apps = top_named_durations(&app_durations, 5);
    let top_intents = analysis.summary.iter().take(5).cloned().collect::<Vec<_>>();
    let busiest_day = day_durations
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)))
        .map(|(day, duration)| (day.clone(), *duration));

    let average_session_duration = if session_count > 0 {
        total_duration / session_count as i64
    } else {
        0
    };

    let mut highlights = Vec::new();
    if let Some(primary_intent) = top_intents.first() {
        highlights.push(format!(
            "本阶段投入最多的是“{}”，累计 {}。",
            primary_intent.label,
            format_duration(primary_intent.duration)
        ));
    }
    if let Some((day, duration)) = busiest_day.as_ref() {
        highlights.push(format!(
            "{} 是投入最高的一天，累计 {}。",
            day,
            format_duration(*duration)
        ));
    }
    if deep_work_sessions > 0 {
        highlights.push(format!(
            "出现 {deep_work_sessions} 段超过 45 分钟的连续专注时段。"
        ));
    }
    if highlights.is_empty() {
        highlights.push("当前时间范围内记录较少，暂时无法形成稳定模式。".to_string());
    }

    let mut risks = Vec::new();
    if average_session_duration > 0 && average_session_duration < 20 * 60 {
        risks.push("平均 session 偏短，任务切换可能较频繁。".to_string());
    }
    if context_switch_sessions > 0
        && session_count > 0
        && context_switch_sessions * 2 >= session_count
    {
        risks.push("多应用混合 session 偏多，建议留意上下文切换成本。".to_string());
    }
    if let Some(meeting) = top_intents.iter().find(|item| item.label == "会议沟通") {
        if meeting.duration * 4 >= total_duration.max(1) {
            risks.push("会议沟通占比较高，注意给深度工作预留整块时间。".to_string());
        }
    }
    if risks.is_empty() {
        risks.push("整体节奏较稳定，没有明显异常信号。".to_string());
    }

    let title = format!(
        "工作复盘（{} ~ {}）",
        date_from.unwrap_or("最早记录"),
        date_to.unwrap_or("最近记录")
    );

    let mut markdown = String::new();
    markdown.push_str(&format!("# {title}\n\n"));
    markdown.push_str("## 本期概览\n\n");
    markdown.push_str(&format!(
        "- 总投入时长：{}\n",
        format_duration(total_duration)
    ));
    markdown.push_str(&format!("- 活跃天数：{active_days} 天\n"));
    markdown.push_str(&format!("- session 数量：{session_count} 段\n"));
    markdown.push_str(&format!("- 深度工作段：{deep_work_sessions} 段\n\n"));

    markdown.push_str("## 重点工作\n\n");
    if top_intents.is_empty() {
        markdown.push_str("- 暂无足够数据识别重点工作。\n");
    } else {
        for item in &top_intents {
            markdown.push_str(&format!(
                "- {}：{}，共 {} 段\n",
                item.label,
                format_duration(item.duration),
                item.session_count
            ));
        }
    }
    markdown.push('\n');

    markdown.push_str("## 核心观察\n\n");
    for line in &highlights {
        markdown.push_str(&format!("- {line}\n"));
    }
    markdown.push('\n');

    markdown.push_str("## 风险与提醒\n\n");
    for line in &risks {
        markdown.push_str(&format!("- {line}\n"));
    }
    markdown.push('\n');

    markdown.push_str("## 下阶段建议\n\n");
    if average_session_duration > 0 && average_session_duration < 20 * 60 {
        markdown.push_str("- 给复杂任务预留 45 到 90 分钟的完整时间块，减少碎片切换。\n");
    } else {
        markdown.push_str("- 延续当前节奏，把高价值任务继续放到连续时间段里推进。\n");
    }
    if let Some(item) = top_apps.first() {
        markdown.push_str(&format!(
            "- 继续围绕 {} 这个主阵地沉淀产出，避免工具切换带来的分心。\n",
            item.name
        ));
    }

    WeeklyReviewResult {
        title,
        markdown,
        total_duration,
        active_days,
        session_count,
        deep_work_sessions,
        top_intents,
        top_apps,
        highlights,
        risks,
    }
}

fn format_duration_localized_period(seconds: i64, locale: AppLocale) -> String {
    let abs = seconds.unsigned_abs();
    let hours = abs / 3600;
    let mins = (abs % 3600) / 60;
    match locale {
        AppLocale::ZhCn | AppLocale::ZhTw => {
            if hours > 0 && mins > 0 {
                format!("{hours}小时{mins}分")
            } else if hours > 0 {
                format!("{hours}小时")
            } else if mins > 0 {
                format!("{mins}分钟")
            } else {
                format!("{abs}秒")
            }
        }
        AppLocale::En => {
            if hours > 0 && mins > 0 {
                format!("{hours}h {mins}m")
            } else if hours > 0 {
                format!("{hours}h")
            } else if mins > 0 {
                format!("{mins}m")
            } else {
                format!("{abs}s")
            }
        }
        AppLocale::Ar => {
            if hours > 0 && mins > 0 {
                format!("{hours} س {mins} د")
            } else if hours > 0 {
                format!("{hours} س")
            } else if mins > 0 {
                format!("{mins} د")
            } else {
                format!("{abs} ث")
            }
        }
    }
}

fn localize_intent_label(label: &str, locale: AppLocale) -> String {
    match locale {
        AppLocale::ZhCn => label.to_string(),
        AppLocale::ZhTw => match label {
            "编码开发" => "編碼開發",
            "代码评审" => "程式碼評審",
            "需求文档" => "需求文件",
            "会议沟通" => "會議溝通",
            "问题排查" => "問題排查",
            "测试验证" => "測試驗證",
            "学习调研" => "學習調研",
            "AI 协作" => "AI 協作",
            "项目管理" => "專案管理",
            "通用工作" => "通用工作",
            other => other,
        }
        .to_string(),
        AppLocale::En => match label {
            "编码开发" => "Coding & Development",
            "代码评审" => "Code Review",
            "需求文档" => "Specs & Docs",
            "会议沟通" => "Meetings & Comms",
            "问题排查" => "Debugging",
            "测试验证" => "Testing & QA",
            "学习调研" => "Research & Learning",
            "AI 协作" => "AI Collaboration",
            "项目管理" => "Project Management",
            "通用工作" => "General Work",
            other => other,
        }
        .to_string(),
        AppLocale::Ar => match label {
            "编码开发" => "البرمجة والتطوير",
            "代码评审" => "مراجعة الكود",
            "需求文档" => "المستندات والمواصفات",
            "会议沟通" => "الاجتماعات والتواصل",
            "问题排查" => "استكشاف الأخطاء",
            "测试验证" => "الاختبار والتحقق",
            "学习调研" => "البحث والتعلم",
            "AI 协作" => "التعاون مع الذكاء الاصطناعي",
            "项目管理" => "إدارة المشاريع",
            "通用工作" => "عمل عام",
            other => other,
        }
        .to_string(),
    }
}

fn format_signed_delta_period(delta_secs: i64, pct: Option<f64>, locale: AppLocale) -> String {
    let sign = if delta_secs >= 0 { "+" } else { "-" };
    let dur = format_duration_localized_period(delta_secs.abs(), locale);
    match pct {
        Some(p) => format!("{sign}{dur} ({:+.1}%)", p),
        None => format!("{sign}{dur}"),
    }
}

fn aggregate_weekly_buckets(
    activities: &[Activity],
    period_start: &str,
    period_end: &str,
) -> Vec<WeeklyEffortBucket> {
    let clamp_start = NaiveDate::parse_from_str(period_start, "%Y-%m-%d").ok();
    let clamp_end = NaiveDate::parse_from_str(period_end, "%Y-%m-%d").ok();

    struct BucketAcc {
        week_monday: NaiveDate,
        duration: i64,
        days: HashSet<String>,
    }

    let mut map: BTreeMap<String, BucketAcc> = BTreeMap::new();
    for act in activities {
        let day_str = date_from_timestamp(act.timestamp);
        let Ok(day) = NaiveDate::parse_from_str(&day_str, "%Y-%m-%d") else {
            continue;
        };
        let iso = day.iso_week();
        let week_key = format!("{:04}-W{:02}", iso.year(), iso.week());
        let monday =
            NaiveDate::from_isoywd_opt(iso.year(), iso.week(), chrono::Weekday::Mon).unwrap_or(day);
        let entry = map.entry(week_key).or_insert_with(|| BucketAcc {
            week_monday: monday,
            duration: 0,
            days: HashSet::new(),
        });
        entry.duration += act.duration.max(0);
        entry.days.insert(day_str);
    }

    map.into_iter()
        .map(|(week_key, acc)| {
            let raw_end = acc.week_monday + chrono::Duration::days(6);
            let from = clamp_start
                .map(|s| acc.week_monday.max(s))
                .unwrap_or(acc.week_monday);
            let to = clamp_end.map(|e| raw_end.min(e)).unwrap_or(raw_end);
            WeeklyEffortBucket {
                week_key,
                date_from: from.format("%Y-%m-%d").to_string(),
                date_to: to.format("%Y-%m-%d").to_string(),
                duration: acc.duration,
                active_days: acc.days.len(),
            }
        })
        .collect()
}

fn compute_project_usage_for_period(
    activities: &[Activity],
    projects: &[ProjectDefinition],
    locale: AppLocale,
) -> Vec<ProjectUsage> {
    if projects.is_empty() || activities.is_empty() {
        return Vec::new();
    }
    let resolver = ProjectResolver::from_projects(projects);
    let mut usage = resolver.aggregate_activities(activities, true);
    let has_matched = usage
        .iter()
        .any(|u| u.key != UNASSIGNED_PROJECT_KEY && u.duration > 0);
    if !has_matched {
        return Vec::new();
    }
    let unassigned_name = match locale {
        AppLocale::ZhCn => "未归属",
        AppLocale::ZhTw => "未歸屬",
        AppLocale::En => "Unassigned",
        AppLocale::Ar => "غير مصنف",
    };
    for item in &mut usage {
        if item.key == UNASSIGNED_PROJECT_KEY {
            item.name = unassigned_name.to_string();
        }
    }
    usage
}

/// 生成周报 / 月报周期复盘（M3-6）：聚合活动意图、项目投入、按周趋势、环比变化与四语本地化 Markdown。
#[allow(clippy::too_many_arguments)]
pub fn generate_period_review(
    activities: &[Activity],
    prev_activities: Option<&[Activity]>,
    date_from: &str,
    date_to: &str,
    granularity: ReviewGranularity,
    period_key: &str,
    locale: AppLocale,
    projects: &[ProjectDefinition],
) -> PeriodReviewResult {
    let intent_analysis = analyze_intents(activities);
    let sessions = intent_analysis.sessions;
    let total_duration: i64 = activities.iter().map(|a| a.duration.max(0)).sum();

    let mut days_duration: HashMap<String, i64> = HashMap::new();
    let mut apps_duration: HashMap<String, i64> = HashMap::new();
    for activity in activities {
        let dur = activity.duration.max(0);
        if dur > 0 {
            let day = date_from_timestamp(activity.timestamp);
            *days_duration.entry(day).or_insert(0) += dur;
            *apps_duration.entry(activity.app_name.clone()).or_insert(0) += dur;
        }
    }

    let active_days = days_duration.len();
    let session_count = sessions.len();
    let deep_work_sessions = sessions
        .iter()
        .filter(|s| s.duration >= DEEP_WORK_THRESHOLD_SECONDS)
        .count();
    let context_switch_sessions = sessions.iter().filter(|s| s.app_count >= 3).count();
    let busiest_day = days_duration
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(day, dur)| (day.clone(), *dur));

    let mut top_apps: Vec<NamedDuration> = apps_duration
        .into_iter()
        .map(|(name, duration)| NamedDuration { name, duration })
        .collect();
    top_apps.sort_by(|a, b| {
        b.duration
            .cmp(&a.duration)
            .then_with(|| a.name.cmp(&b.name))
    });
    top_apps.truncate(6);

    let top_intents: Vec<IntentSummary> = intent_analysis
        .summary
        .into_iter()
        .take(5)
        .map(|item| IntentSummary {
            label: localize_intent_label(&item.label, locale),
            duration: item.duration,
            session_count: item.session_count,
        })
        .collect();

    let project_usage = compute_project_usage_for_period(activities, projects, locale);
    let weekly_buckets = aggregate_weekly_buckets(activities, date_from, date_to);

    let comparison = prev_activities.map(|prev| {
        let prev_total: i64 = prev.iter().map(|a| a.duration.max(0)).sum();
        let prev_days: HashSet<String> = prev
            .iter()
            .filter(|a| a.duration > 0)
            .map(|a| date_from_timestamp(a.timestamp))
            .collect();
        let prev_deep = analyze_intents(prev)
            .sessions
            .into_iter()
            .filter(|s| s.duration >= DEEP_WORK_THRESHOLD_SECONDS)
            .count();
        let delta = total_duration - prev_total;
        let pct = if prev_total > 0 {
            Some((delta as f64 / prev_total as f64) * 100.0)
        } else {
            None
        };
        PeriodComparisonDelta {
            prev_total_duration: prev_total,
            duration_delta: delta,
            duration_delta_pct: pct,
            prev_active_days: prev_days.len(),
            prev_deep_work_sessions: prev_deep,
        }
    });

    let avg_session_duration = if session_count > 0 {
        total_duration / session_count as i64
    } else {
        0
    };

    let mut highlights = Vec::new();
    if let Some(primary) = top_intents.first() {
        let dur_str = format_duration_localized_period(primary.duration, locale);
        highlights.push(match locale {
            AppLocale::ZhCn => format!(
                "本期投入最多的方向是「{}」，累计 {}。",
                primary.label, dur_str
            ),
            AppLocale::ZhTw => format!(
                "本期投入最多的方向是「{}」，累計 {}。",
                primary.label, dur_str
            ),
            AppLocale::En => format!(
                "Top focus area was \"{}\", totaling {}.",
                primary.label, dur_str
            ),
            AppLocale::Ar => format!(
                "أكثر مجال تركيزًا كان «{}» بإجمالي {}.",
                primary.label, dur_str
            ),
        });
    }
    if let Some((day, dur)) = busiest_day.as_ref() {
        let dur_str = format_duration_localized_period(*dur, locale);
        highlights.push(match locale {
            AppLocale::ZhCn => format!("{day} 是投入最高的一天，累计 {dur_str}。"),
            AppLocale::ZhTw => format!("{day} 是投入最高的一天，累計 {dur_str}。"),
            AppLocale::En => format!("{day} was the most active day with {dur_str} logged."),
            AppLocale::Ar => format!("كان يوم {day} الأكثر نشاطًا بإجمالي {dur_str}."),
        });
    }
    if deep_work_sessions > 0 {
        highlights.push(match locale {
            AppLocale::ZhCn => {
                format!("共完成 {deep_work_sessions} 段超过 45 分钟的连续深度专注。")
            }
            AppLocale::ZhTw => {
                format!("共完成 {deep_work_sessions} 段超過 45 分鐘的連續深度專注。")
            }
            AppLocale::En => {
                format!("Completed {deep_work_sessions} deep-work sessions lasting 45+ minutes.")
            }
            AppLocale::Ar => {
                format!("تم إتمام {deep_work_sessions} جلسة عمل عميق تجاوزت 45 دقيقة.")
            }
        });
    }
    if highlights.is_empty() {
        highlights.push(match locale {
            AppLocale::ZhCn => "当前周期内记录较少，暂未形成稳定的节奏模式。".to_string(),
            AppLocale::ZhTw => "目前週期內記錄較少，暫未形成穩定的節奏模式。".to_string(),
            AppLocale::En => {
                "Not enough activity logged in this period to identify clear patterns yet."
                    .to_string()
            }
            AppLocale::Ar => {
                "لا توجد سجلات كافية في هذه الفترة لتحديد أنماط عمل واضحة بعد.".to_string()
            }
        });
    }

    let mut risks = Vec::new();
    if avg_session_duration > 0 && avg_session_duration < 20 * 60 {
        risks.push(match locale {
            AppLocale::ZhCn => "平均工作段偏短，任务切换可能较频繁。".to_string(),
            AppLocale::ZhTw => "平均工作段偏短，任務切換可能較頻繁。".to_string(),
            AppLocale::En => {
                "Average session length is short, suggesting frequent context switching."
                    .to_string()
            }
            AppLocale::Ar => "متوسط مدة الجلسة قصير، مما يشير إلى كثرة تبديل السياق.".to_string(),
        });
    }
    if context_switch_sessions > 0
        && session_count > 0
        && context_switch_sessions * 2 >= session_count
    {
        risks.push(match locale {
            AppLocale::ZhCn => "多应用混合工作段占比较高，建议留意切换成本。".to_string(),
            AppLocale::ZhTw => "多應用混合工作段佔比較高，建議留意切換成本。".to_string(),
            AppLocale::En => {
                "A high share of sessions mixed 3+ apps; watch for context-switch overhead."
                    .to_string()
            }
            AppLocale::Ar => {
                "نسبة كبيرة من الجلسات تشمل 3 تطبيقات أو أكثر؛ انتبه لتكلفة تبديل المهام."
                    .to_string()
            }
        });
    }
    let meeting_label = localize_intent_label("会议沟通", locale);
    if let Some(meeting) = top_intents.iter().find(|i| i.label == meeting_label) {
        if meeting.duration * 4 >= total_duration.max(1) {
            risks.push(match locale {
                AppLocale::ZhCn => "会议沟通占比较高，建议为深度工作预留连续时间块。".to_string(),
                AppLocale::ZhTw => "會議溝通佔比較高，建議為深度工作預留連續時間塊。".to_string(),
                AppLocale::En => "Meetings & comms took a large share; consider protecting unbroken deep-work blocks.".to_string(),
                AppLocale::Ar => "تستحوذ الاجتماعات على نسبة مرتفعة؛ يُنصح بحجز كتل زمنية متصلة للعمل العميق.".to_string(),
            });
        }
    }
    if risks.is_empty() {
        risks.push(match locale {
            AppLocale::ZhCn => "整体工作节奏较稳定，未检测到明显异常波动。".to_string(),
            AppLocale::ZhTw => "整體工作節奏較穩定，未偵測到明顯異常波動。".to_string(),
            AppLocale::En => "Work cadence looks steady with no major risk signals.".to_string(),
            AppLocale::Ar => "وتيرة العمل مستقرة بشكل عام دون إشارات مخاطرة واضحة.".to_string(),
        });
    }

    let period_label = match (granularity, locale) {
        (ReviewGranularity::Week, AppLocale::ZhCn) => "周度复盘",
        (ReviewGranularity::Week, AppLocale::ZhTw) => "週度復盤",
        (ReviewGranularity::Week, AppLocale::En) => "Weekly Review",
        (ReviewGranularity::Week, AppLocale::Ar) => "المراجعة الأسبوعية",
        (ReviewGranularity::Month, AppLocale::ZhCn) => "月度复盘",
        (ReviewGranularity::Month, AppLocale::ZhTw) => "月度復盤",
        (ReviewGranularity::Month, AppLocale::En) => "Monthly Review",
        (ReviewGranularity::Month, AppLocale::Ar) => "المراجعة الشهرية",
    };
    let title = format!("{period_label} · {period_key} ({date_from} ~ {date_to})");

    let mut md = String::new();
    md.push_str(&format!("# {title}\n\n"));

    // 1. 概览
    let (h_overview, l_total, l_days, l_sessions, l_deep, l_vs_prev) = match locale {
        AppLocale::ZhCn => (
            "## 本期概览",
            "总投入时长",
            "活跃天数",
            "工作段数",
            "深度专注段（≥45m）",
            if granularity == ReviewGranularity::Month {
                "环比上月"
            } else {
                "环比上周"
            },
        ),
        AppLocale::ZhTw => (
            "## 本期概覽",
            "總投入時長",
            "活躍天數",
            "工作段數",
            "深度專注段（≥45m）",
            if granularity == ReviewGranularity::Month {
                "環比上月"
            } else {
                "環比上週"
            },
        ),
        AppLocale::En => (
            "## Period Overview",
            "Total Time",
            "Active Days",
            "Work Sessions",
            "Deep Work Sessions (45m+)",
            if granularity == ReviewGranularity::Month {
                "vs. Previous Month"
            } else {
                "vs. Previous Week"
            },
        ),
        AppLocale::Ar => (
            "## نظرة عامة على الفترة",
            "إجمالي الوقت",
            "الأيام النشطة",
            "جلسات العمل",
            "جلسات العمل العميق (45د+)",
            if granularity == ReviewGranularity::Month {
                "مقارنة بالشهر السابق"
            } else {
                "مقارنة بالأسبوع السابق"
            },
        ),
    };
    md.push_str(&format!("{h_overview}\n\n"));
    md.push_str(&format!(
        "- {l_total}: {}\n",
        format_duration_localized_period(total_duration, locale)
    ));
    if let Some(cmp) = &comparison {
        if cmp.prev_total_duration > 0 || total_duration > 0 {
            md.push_str(&format!(
                "- {l_vs_prev}: {}\n",
                format_signed_delta_period(cmp.duration_delta, cmp.duration_delta_pct, locale)
            ));
        }
    }
    md.push_str(&format!("- {l_days}: {active_days}\n"));
    md.push_str(&format!("- {l_sessions}: {session_count}\n"));
    md.push_str(&format!("- {l_deep}: {deep_work_sessions}\n\n"));

    // 2. 月报按周趋势表
    if granularity == ReviewGranularity::Month && !weekly_buckets.is_empty() {
        let (h_trend, c_week, c_range, c_dur, c_days) = match locale {
            AppLocale::ZhCn => (
                "## 按周投入趋势",
                "周期",
                "日期范围",
                "投入时长",
                "活跃天数",
            ),
            AppLocale::ZhTw => (
                "## 按週投入趨勢",
                "週期",
                "日期範圍",
                "投入時長",
                "活躍天數",
            ),
            AppLocale::En => (
                "## Weekly Trend",
                "Week",
                "Date Range",
                "Duration",
                "Active Days",
            ),
            AppLocale::Ar => (
                "## الاتجاه الأسبوعي",
                "الأسبوع",
                "النطاق",
                "المدة",
                "الأيام النشطة",
            ),
        };
        md.push_str(&format!("{h_trend}\n\n"));
        md.push_str(&format!("| {c_week} | {c_range} | {c_dur} | {c_days} |\n"));
        md.push_str("| --- | --- | --- | --- |\n");
        for b in &weekly_buckets {
            md.push_str(&format!(
                "| {} | {} ~ {} | {} | {} |\n",
                b.week_key,
                b.date_from,
                b.date_to,
                format_duration_localized_period(b.duration, locale),
                b.active_days
            ));
        }
        md.push('\n');
    }

    // 3. 项目工时分布
    if !project_usage.is_empty() {
        let h_projects = match locale {
            AppLocale::ZhCn => "## 项目工时分布",
            AppLocale::ZhTw => "## 專案工時分佈",
            AppLocale::En => "## Project Breakdown",
            AppLocale::Ar => "## توزيع وقت المشاريع",
        };
        md.push_str(&format!("{h_projects}\n\n"));
        let denom = total_duration.max(1) as f64;
        for proj in project_usage.iter().take(10) {
            let pct = (proj.duration as f64 / denom * 100.0).round() as i64;
            md.push_str(&format!(
                "- **{}**: {} ({pct}%)\n",
                proj.name,
                format_duration_localized_period(proj.duration, locale)
            ));
        }
        md.push('\n');
    }

    // 4. 重点方向 & 常用应用
    let (h_intents, empty_intents, h_apps) = match locale {
        AppLocale::ZhCn => (
            "## 重点工作方向",
            "暂无足够数据识别重点工作。",
            "## 主要应用",
        ),
        AppLocale::ZhTw => (
            "## 重點工作方向",
            "暫無足夠資料識別重點工作。",
            "## 主要應用",
        ),
        AppLocale::En => (
            "## Focus Areas",
            "Not enough data to classify focus areas.",
            "## Top Applications",
        ),
        AppLocale::Ar => (
            "## مجالات التركيز الرئيسية",
            "لا توجد بيانات كافية لتصنيف مجالات العمل.",
            "## أبرز التطبيقات",
        ),
    };
    md.push_str(&format!("{h_intents}\n\n"));
    if top_intents.is_empty() {
        md.push_str(&format!("- {empty_intents}\n"));
    } else {
        for item in &top_intents {
            md.push_str(&format!(
                "- {}: {} ({})\n",
                item.label,
                format_duration_localized_period(item.duration, locale),
                item.session_count
            ));
        }
    }
    md.push('\n');

    if !top_apps.is_empty() {
        md.push_str(&format!("{h_apps}\n\n"));
        for app in &top_apps {
            md.push_str(&format!(
                "- {}: {}\n",
                app.name,
                format_duration_localized_period(app.duration, locale)
            ));
        }
        md.push('\n');
    }

    // 5. 核心观察 / 风险与建议
    let (h_highlights, h_risks, h_next) = match locale {
        AppLocale::ZhCn => ("## 核心观察", "## 风险与提醒", "## 下阶段建议"),
        AppLocale::ZhTw => ("## 核心觀察", "## 風險與提醒", "## 下階段建議"),
        AppLocale::En => ("## Key Highlights", "## Risks & Watchouts", "## Next Steps"),
        AppLocale::Ar => (
            "## أبرز الملاحظات",
            "## المخاطر والتنبيهات",
            "## توصيات المرحلة القادمة",
        ),
    };
    md.push_str(&format!("{h_highlights}\n\n"));
    for line in &highlights {
        md.push_str(&format!("- {line}\n"));
    }
    md.push('\n');

    md.push_str(&format!("{h_risks}\n\n"));
    for line in &risks {
        md.push_str(&format!("- {line}\n"));
    }
    md.push('\n');

    md.push_str(&format!("{h_next}\n\n"));
    if avg_session_duration > 0 && avg_session_duration < 20 * 60 {
        md.push_str(match locale {
            AppLocale::ZhCn => "- 为高价值复杂任务预留 45~90 分钟完整时间块，减少碎片化切换。\n",
            AppLocale::ZhTw => "- 為高價值複雜任務預留 45~90 分鐘完整時間塊，減少碎片化切換。\n",
            AppLocale::En => {
                "- Block 45–90 minute uninterrupted focus windows for complex high-value tasks.\n"
            }
            AppLocale::Ar => {
                "- خصص فترات تركيز متصلة من 45 إلى 90 دقيقة للمهام المعقدة وعالية القيمة.\n"
            }
        });
    } else {
        md.push_str(match locale {
            AppLocale::ZhCn => "- 延续当前节奏，继续把关键里程碑安排在连续深度专注时段推进。\n",
            AppLocale::ZhTw => "- 延續目前節奏，繼續把關鍵里程碑安排在連續深度專注時段推進。\n",
            AppLocale::En => "- Keep the steady cadence and schedule key deliverables inside continuous focus blocks.\n",
            AppLocale::Ar => "- حافظ على هذا الإيقاع المستقر وجدول المهام المحورية ضمن جلسات تركيز متواصلة.\n",
        });
    }
    if let Some(app) = top_apps.first() {
        md.push_str(&match locale {
            AppLocale::ZhCn => format!("- 围绕 {} 这一主阵地持续沉淀产出，减少无关窗口干扰。\n", app.name),
            AppLocale::ZhTw => format!("- 圍繞 {} 這一主陣地持續沉澱產出，減少無關視窗干擾。\n", app.name),
            AppLocale::En => format!("- Keep consolidating core output in {} while trimming unnecessary window switches.\n", app.name),
            AppLocale::Ar => format!("- واصل تركيز إنتاجك الأساسي في {} مع تقليل التشتت بين النوافذ.\n", app.name),
        });
    }

    PeriodReviewResult {
        granularity,
        period_key: period_key.to_string(),
        date_from: date_from.to_string(),
        date_to: date_to.to_string(),
        locale: locale.as_code().to_string(),
        title,
        markdown: md,
        total_duration,
        active_days,
        session_count,
        deep_work_sessions,
        top_intents,
        top_apps,
        project_usage,
        weekly_buckets,
        comparison,
        highlights,
        risks,
    }
}

pub fn extract_todos(activities: &[Activity]) -> TodoExtractionResult {
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    for activity in activities {
        let sources = build_todo_sources(activity);
        for (candidate, confidence, reason) in sources {
            let normalized = normalize_candidate(&candidate);
            if normalized.len() < 4 || !seen.insert(normalized) {
                continue;
            }

            items.push(TodoItem {
                title: candidate,
                date: date_from_timestamp(activity.timestamp),
                source_title: truncate_text(&activity.window_title, TODO_TEXT_LIMIT),
                source_app: activity.app_name.clone(),
                confidence,
                reason,
            });
        }
    }

    items.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| b.date.cmp(&a.date))
            .then_with(|| a.title.cmp(&b.title))
    });
    items.truncate(20);

    let summary = if items.is_empty() {
        "当前时间范围内没有提取到明确的待办信号。".to_string()
    } else {
        format!("共提取到 {} 条候选待办，已按置信度排序。", items.len())
    };

    TodoExtractionResult { items, summary }
}

fn classify_session(
    dominant_category: &str,
    dominant_app: &str,
    title: &str,
    browser_domains: &[String],
    top_keywords: &[String],
    combined_lines: &[String],
) -> IntentMatch {
    let lower_app = dominant_app.to_lowercase();
    let lower_title = title.to_lowercase();
    let domain_text = browser_domains.join(" ").to_lowercase();
    let keyword_text = top_keywords.join(" ").to_lowercase();
    let corpus = combined_lines.join(" ").to_lowercase();

    let mut matches = vec![
        score_intent(
            "编码开发",
            &corpus,
            &[
                "cursor",
                "vscode",
                "visual studio code",
                "code",
                "pycharm",
                "intellij",
                "xcode",
                "webstorm",
                "terminal",
                "iterm",
                "warp",
                "git",
                "commit",
                "branch",
                "merge",
                "cargo",
                "npm",
                "pnpm",
                "debug",
                "feature",
                "代码",
                "开发",
            ],
            12,
        ),
        score_intent(
            "代码评审",
            &corpus,
            &[
                "pull request",
                "merge request",
                "code review",
                "review",
                "diff",
                "approval",
                "comment",
                "pr ",
                " mr ",
                "评审",
                "审查",
            ],
            14,
        ),
        score_intent(
            "需求文档",
            &corpus,
            &[
                "prd",
                "spec",
                "doc",
                "docs",
                "document",
                "notion",
                "语雀",
                "飞书文档",
                "confluence",
                "需求",
                "文档",
                "方案",
                "设计稿",
            ],
            10,
        ),
        score_intent(
            "会议沟通",
            &corpus,
            &[
                "zoom",
                "meeting",
                "meet",
                "teams",
                "slack",
                "discord",
                "飞书",
                "企业微信",
                "会议",
                "沟通",
                "同步",
                "站会",
                "腾讯会议",
            ],
            12,
        ),
        score_intent(
            "问题排查",
            &corpus,
            &[
                "bug",
                "issue",
                "error",
                "exception",
                "traceback",
                "stack trace",
                "failed",
                "failure",
                "日志",
                "报错",
                "修复",
                "排查",
                "异常",
            ],
            12,
        ),
        score_intent(
            "测试验证",
            &corpus,
            &[
                "test",
                "pytest",
                "vitest",
                "playwright",
                "cypress",
                "junit",
                "assert",
                "验证",
                "回归",
                "测试",
                "单测",
            ],
            10,
        ),
        score_intent(
            "学习调研",
            &corpus,
            &[
                "google",
                "stackoverflow",
                "docs",
                "documentation",
                "guide",
                "tutorial",
                "research",
                "readme",
                "调研",
                "教程",
                "文档",
                "资料",
            ],
            8,
        ),
        score_intent(
            "AI 协作",
            &corpus,
            &[
                "chatgpt",
                "openai",
                "claude",
                "deepseek",
                "kimi",
                "gemini",
                "copilot",
                "cursor",
                "llm",
                "prompt",
                "大模型",
            ],
            9,
        ),
        score_intent(
            "项目管理",
            &corpus,
            &[
                "jira",
                "linear",
                "asana",
                "trello",
                "roadmap",
                "milestone",
                "todoist",
                "sprint",
                "任务",
                "排期",
                "看板",
                "项目",
            ],
            10,
        ),
    ];

    if dominant_category == "development" {
        add_score(&mut matches, "编码开发", 10, "主类别为 development");
    }
    if lower_app.contains("browser") || lower_app.contains("chrome") || lower_app.contains("safari")
    {
        add_score(&mut matches, "学习调研", 4, "会话主要发生在浏览器");
    }
    if lower_title.contains("pr") || lower_title.contains("review") {
        add_score(&mut matches, "代码评审", 8, format!("标题包含 {title}"));
    }
    if domain_text.contains("github.com") {
        add_score(&mut matches, "编码开发", 6, "包含 github.com");
        add_score(&mut matches, "代码评审", 4, "包含 github.com");
    }
    if domain_text.contains("openai.com")
        || domain_text.contains("claude.ai")
        || domain_text.contains("chat.deepseek.com")
    {
        add_score(&mut matches, "AI 协作", 10, "包含 AI 工具站点");
    }
    if keyword_text.contains("bug") || keyword_text.contains("报错") {
        add_score(&mut matches, "问题排查", 8, "关键词显示异常处理");
    }

    matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.cmp(&b.label)));
    let best = matches
        .into_iter()
        .find(|item| item.score > 0)
        .unwrap_or_else(|| IntentMatch {
            label: "通用工作".to_string(),
            score: 0,
            evidence: vec!["未命中明确意图信号，按通用工作处理".to_string()],
        });

    IntentMatch {
        label: best.label,
        score: if best.score <= 0 { 40 } else { 40 + best.score },
        evidence: best.evidence.into_iter().take(4).collect(),
    }
}

fn score_intent(label: &str, corpus: &str, patterns: &[&str], score_per_hit: i32) -> IntentMatch {
    let mut evidence = Vec::new();
    let mut score = 0;

    for pattern in patterns {
        if corpus.contains(pattern) {
            score += score_per_hit;
            evidence.push(format!("命中关键词 {pattern}"));
        }
    }

    IntentMatch {
        label: label.to_string(),
        score,
        evidence,
    }
}

fn add_score<S: Into<String>>(matches: &mut [IntentMatch], label: &str, delta: i32, evidence: S) {
    if let Some(item) = matches.iter_mut().find(|item| item.label == label) {
        item.score += delta;
        item.evidence.push(evidence.into());
    }
}

fn build_todo_sources(activity: &Activity) -> Vec<(String, i32, String)> {
    let mut candidates = Vec::new();
    let mut texts = vec![activity.window_title.clone()];
    if let Some(ocr_text) = &activity.ocr_text {
        texts.push(ocr_text.clone());
    }
    if let Some(url) = &activity.browser_url {
        texts.push(url.clone());
    }

    let checkbox_regex = checkbox_regex();
    let explicit_regex = explicit_todo_regex();
    let action_regex = action_todo_regex();

    for text in texts {
        for capture in checkbox_regex.captures_iter(&text) {
            if let Some(matched) = capture.get(1) {
                candidates.push((
                    truncate_text(matched.as_str(), TODO_TEXT_LIMIT),
                    90,
                    "命中未完成清单项".to_string(),
                ));
            }
        }

        for capture in explicit_regex.captures_iter(&text) {
            // capture.get(2) 取实际待办文本，跳过 "TODO:" / "待办：" 等前缀
            if let Some(matched) = capture.get(2).or_else(|| capture.get(0)) {
                candidates.push((
                    truncate_text(&clean_todo_text(matched.as_str()), TODO_TEXT_LIMIT),
                    82,
                    "命中显式待办关键词".to_string(),
                ));
            }
        }

        for capture in action_regex.captures_iter(&text) {
            if let Some(matched) = capture.get(0) {
                candidates.push((
                    truncate_text(&clean_todo_text(matched.as_str()), TODO_TEXT_LIMIT),
                    68,
                    "命中动作型任务短语".to_string(),
                ));
            }
        }
    }

    let title_lower = activity.window_title.to_lowercase();
    if title_lower.contains("issue")
        || title_lower.contains("bug")
        || title_lower.contains("pr #")
        || title_lower.contains("任务")
    {
        candidates.push((
            truncate_text(&activity.window_title, TODO_TEXT_LIMIT),
            60,
            "窗口标题像是待跟进事项".to_string(),
        ));
    }

    candidates
}

fn checkbox_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?m)\[[ xX]?\]\s*([^\n]{2,80})").expect("checkbox regex"))
}

fn explicit_todo_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)(todo|待办|follow[- ]?up|next step|后续动作|需要跟进)\s*[:：-]?\s*([^\n。；]{2,80})",
        )
        .expect("explicit todo regex")
    })
}

fn action_todo_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)(修复|排查|优化|补充|整理|确认|同步|review|fix|investigate|refactor|verify)\s*[:：-]?\s*([^\n。；]{0,60})",
        )
        .expect("action todo regex")
    })
}

fn extract_keywords(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && !is_cjk(c))
        .map(str::trim)
        .filter(|token| token.chars().count() >= 2)
        .map(|token| token.to_lowercase())
        .filter(|token| !is_noise_token(token))
        .collect()
}

fn is_noise_token(token: &str) -> bool {
    matches!(
        token,
        "https"
            | "http"
            | "www"
            | "com"
            | "the"
            | "and"
            | "with"
            | "for"
            | "from"
            | "into"
            | "main"
            | "work"
            | "review"
            | "页面"
            | "窗口"
            | "文件"
            | "今天"
            | "进行"
            | "正在"
    )
}

fn is_cjk(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
}

fn top_name_by_duration(values: &HashMap<String, i64>) -> Option<String> {
    values
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(name, _)| name.clone())
}

fn top_name_by_count(values: &HashMap<String, i32>) -> Option<String> {
    values
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(name, _)| name.clone())
}

fn top_named_durations(values: &HashMap<String, i64>, limit: usize) -> Vec<NamedDuration> {
    let mut items = values
        .iter()
        .map(|(name, duration)| NamedDuration {
            name: name.clone(),
            duration: *duration,
        })
        .collect::<Vec<_>>();

    items.sort_by_key(|item| (Reverse(item.duration), item.name.clone()));
    items.truncate(limit);
    items
}

fn top_keywords(keyword_counts: &HashMap<String, i32>, limit: usize) -> Vec<String> {
    let mut items = keyword_counts.iter().collect::<Vec<_>>();
    items.sort_by(|a, b| {
        b.1.cmp(a.1)
            .then_with(|| a.0.len().cmp(&b.0.len()))
            .then_with(|| a.0.cmp(b.0))
    });
    items
        .into_iter()
        .take(limit)
        .map(|(keyword, _)| keyword.clone())
        .collect()
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let mut iter = value.trim().chars();
    let excerpt = iter.by_ref().take(max_chars).collect::<String>();
    if iter.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

fn clean_todo_text(value: &str) -> String {
    value
        .replace(['\n', '\r'], " ")
        .replace("[]", "")
        .replace("[ ]", "")
        .replace("[x]", "")
        .trim_matches(|c: char| c == ':' || c == '：' || c == '-' || c == ' ')
        .trim()
        .to_string()
}

fn normalize_candidate(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric() || is_cjk(*ch))
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn extract_domain(url: &str) -> String {
    url.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

fn date_from_timestamp(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .earliest()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        analyze_intents, build_work_sessions, compute_focus_metrics, extract_todos,
        format_focus_metrics_for_prompt,
    };
    use crate::database::Activity;

    fn activity(
        timestamp: i64,
        app_name: &str,
        title: &str,
        duration: i64,
        browser_url: Option<&str>,
        ocr_text: Option<&str>,
    ) -> Activity {
        Activity {
            id: None,
            timestamp,
            app_name: app_name.to_string(),
            window_title: title.to_string(),
            screenshot_path: String::new(),
            ocr_text: ocr_text.map(|value| value.to_string()),
            category: "development".to_string(),
            duration,
            browser_url: browser_url.map(|value| value.to_string()),
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        }
    }

    #[test]
    fn 会话区间应以活动时间戳为终点回推() {
        // timestamp 是区间终点:600 秒的活动应回推出 [ts-600, ts]
        let activities = vec![activity(1_700_000_600, "Code", "编辑文件", 600, None, None)];

        let sessions = build_work_sessions(&activities);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].start_timestamp, 1_700_000_000);
        assert_eq!(sessions[0].end_timestamp, 1_700_000_600);
        assert_eq!(sessions[0].duration, 600);
    }

    #[test]
    fn 应按空闲间隔聚合连续_session() {
        let activities = vec![
            activity(1_700_000_000, "Code", "实现 session 聚合", 300, None, None),
            activity(1_700_000_360, "iTerm", "cargo test", 240, None, None),
            activity(
                1_700_002_000,
                "Chrome",
                "GitHub Pull Request",
                180,
                Some("https://github.com/org/repo/pull/1"),
                None,
            ),
        ];

        let sessions = build_work_sessions(&activities);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].activity_count, 2);
        assert_eq!(sessions[1].dominant_app, "Code");
        assert_eq!(sessions[0].browser_domains, vec!["github.com"]);
    }

    #[test]
    fn 应基于会话识别主要意图() {
        let activities = vec![
            activity(
                1_700_000_000,
                "Code",
                "fix login bug",
                300,
                None,
                Some("traceback investigate error"),
            ),
            activity(
                1_700_000_200,
                "Chrome",
                "GitHub Pull Request review",
                240,
                Some("https://github.com/org/repo/pull/8"),
                None,
            ),
        ];

        let result = analyze_intents(&activities);
        assert_eq!(result.sessions.len(), 1);
        assert!(
            result.sessions[0].intent_label == "问题排查"
                || result.sessions[0].intent_label == "代码评审"
                || result.sessions[0].intent_label == "编码开发"
        );
        assert!(!result.summary.is_empty());
    }

    #[test]
    fn 应提取显式待办项() {
        let activities = vec![
            activity(
                1_700_000_000,
                "Notion",
                "TODO: 补充周报结论",
                120,
                None,
                Some("- [ ] 修复 session 聚合边界"),
            ),
            activity(
                1_700_000_200,
                "Chrome",
                "Issue #42 fix login redirect",
                120,
                Some("https://github.com/org/repo/issues/42"),
                None,
            ),
        ];

        let todos = extract_todos(&activities);
        assert!(!todos.items.is_empty());
        assert!(todos.items.iter().any(|item| item.title.contains("修复")));
    }

    #[test]
    fn 专注度指标应统计最长专注段与高频短时打断源() {
        let base = 1_700_000_000;
        let activities = vec![
            // 30 分钟深度编程
            Activity {
                category: "development".to_string(),
                ..activity(base + 1800, "VS Code", "main.rs", 1800, None, None)
            },
            // 30 秒切去微信（打断 1）
            Activity {
                category: "communication".to_string(),
                ..activity(base + 1835, "WeChat", "群聊", 30, None, None)
            },
            // 10 分钟回到 VS Code
            Activity {
                category: "development".to_string(),
                ..activity(base + 2440, "VS Code", "main.rs", 600, None, None)
            },
            // 40 秒再次切去微信（打断 2）
            Activity {
                category: "communication".to_string(),
                ..activity(base + 2485, "WeChat", "群聊", 40, None, None)
            },
            // 20 分钟继续编程
            Activity {
                category: "development".to_string(),
                ..activity(base + 3690, "VS Code", "lib.rs", 1200, None, None)
            },
        ];

        let metrics = compute_focus_metrics(&activities);
        assert_eq!(metrics.longest_focus_seconds, 1800);
        assert_eq!(metrics.deep_focus_sessions, 1);
        assert_eq!(metrics.context_switches, 4);
        assert!(metrics.focus_score > 40);
        assert_eq!(metrics.top_interrupters.len(), 1);
        assert_eq!(metrics.top_interrupters[0].app_name, "WeChat");
        assert_eq!(metrics.top_interrupters[0].count, 2);

        let prompt = format_focus_metrics_for_prompt(&metrics, crate::analysis::AppLocale::ZhCn);
        assert!(prompt.contains("WeChat"));
    }

    #[test]
    fn 周期复盘应支持周报与月报环比项目分布与四语本地化() {
        use super::{generate_period_review, ReviewGranularity};
        use crate::analysis::AppLocale;
        use crate::project::{ProjectDefinition, ProjectRule, ProjectRuleField, ProjectRuleOp};
        use chrono::{Local, NaiveDate, TimeZone};

        let make_ts = |y: i32, m: u32, d: u32, h: u32| {
            let ndt = NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap();
            Local
                .from_local_datetime(&ndt)
                .earliest()
                .unwrap()
                .timestamp()
        };

        let current_activities = vec![
            activity(
                make_ts(2026, 11, 2, 10),
                "VS Code",
                "Work_Review - main.rs",
                3600,
                None,
                None,
            ),
            activity(
                make_ts(2026, 11, 10, 14),
                "VS Code",
                "Work_Review - report.rs",
                3600,
                None,
                None,
            ),
            activity(
                make_ts(2026, 11, 18, 11),
                "Chrome",
                "Docs",
                1800,
                Some("https://docs.rs"),
                None,
            ),
        ];
        let prev_activities = vec![activity(
            make_ts(2026, 10, 12, 10),
            "VS Code",
            "Work_Review - old.rs",
            4500,
            None,
            None,
        )];
        let projects = vec![ProjectDefinition {
            key: "wr".to_string(),
            name: "Work Review".to_string(),
            color: "#3B82F6".to_string(),
            archived: false,
            rules: vec![ProjectRule {
                field: ProjectRuleField::Title,
                op: ProjectRuleOp::Contains,
                value: "Work_Review".to_string(),
            }],
        }];

        let zh = generate_period_review(
            &current_activities,
            Some(&prev_activities),
            "2026-11-01",
            "2026-11-30",
            ReviewGranularity::Month,
            "2026-11",
            AppLocale::ZhCn,
            &projects,
        );
        assert_eq!(zh.total_duration, 9000);
        assert_eq!(zh.active_days, 3);
        assert_eq!(zh.weekly_buckets.len(), 3);
        assert_eq!(zh.project_usage.first().map(|p| p.key.as_str()), Some("wr"));
        let cmp = zh.comparison.as_ref().expect("comparison");
        assert_eq!(cmp.prev_total_duration, 4500);
        assert_eq!(cmp.duration_delta, 4500);
        assert!((cmp.duration_delta_pct.unwrap() - 100.0).abs() < 0.01);
        assert!(zh.markdown.contains("月度复盘"));
        assert!(zh.markdown.contains("按周投入趋势"));
        assert!(zh.markdown.contains("Work Review"));

        for (loc, expected) in [
            (AppLocale::ZhTw, "月度復盤"),
            (AppLocale::En, "Monthly Review"),
            (AppLocale::Ar, "المراجعة الشهرية"),
        ] {
            let rev = generate_period_review(
                &current_activities,
                Some(&prev_activities),
                "2026-11-01",
                "2026-11-30",
                ReviewGranularity::Month,
                "2026-11",
                loc,
                &projects,
            );
            assert!(rev.markdown.contains(expected));
        }

        // 空数据不得 panic 且应给出兜底提示
        let empty = generate_period_review(
            &[],
            Some(&[]),
            "2026-02-01",
            "2026-02-28",
            ReviewGranularity::Week,
            "2026-W06",
            AppLocale::En,
            &[],
        );
        assert_eq!(empty.total_duration, 0);
        assert!(empty.markdown.contains("Weekly Review"));
    }
}
