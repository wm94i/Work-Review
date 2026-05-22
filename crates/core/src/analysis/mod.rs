pub mod cloud;
pub mod hourly;
pub mod local;
pub mod report_blocks;
pub mod summary;

use crate::config::{AiMode, AiProvider};
use crate::database::{Activity, DailyStats};
use crate::error::Result;
use async_trait::async_trait;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppLocale {
    ZhCn,
    ZhTw,
    En,
}

impl AppLocale {
    pub fn from_code(value: &str) -> Self {
        let normalized = value.trim().to_lowercase();
        if normalized.starts_with("zh-tw") || normalized.starts_with("zh-hk") {
            Self::ZhTw
        } else if normalized.starts_with("en") {
            Self::En
        } else {
            Self::ZhCn
        }
    }

    pub fn from_option(value: Option<&str>) -> Self {
        value.map(Self::from_code).unwrap_or(Self::ZhCn)
    }

    pub fn as_code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::ZhTw => "zh-TW",
            Self::En => "en",
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedReport {
    pub content: String,
    pub used_ai: bool,
    pub fallback_reason: Option<String>,
}

/// AI分析器 trait
/// 使用 async_trait 宏使 trait 支持 dyn 兼容
#[async_trait]
pub trait Analyzer: Send + Sync {
    /// 生成日报
    async fn generate_report(
        &self,
        date: &str,
        stats: &DailyStats,
        activities: &[Activity],
        screenshots_dir: &Path,
        locale: AppLocale,
        category_name_overrides: HashMap<String, String>,
        semantic_name_overrides: HashMap<String, String>,
    ) -> Result<GeneratedReport>;
}

pub fn normalize_custom_prompt(custom_prompt: &str) -> Option<String> {
    let trimmed = custom_prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn append_custom_prompt_for_locale(
    base_prompt: String,
    custom_prompt: &str,
    locale: AppLocale,
) -> String {
    if let Some(custom_prompt) = normalize_custom_prompt(custom_prompt) {
        match locale {
            AppLocale::ZhCn => format!(
                "{base_prompt}\n\n## 额外要求\n以下是用户补充的日报偏好，请在不违背前述结构和约束的前提下尽量满足：\n{custom_prompt}"
            ),
            AppLocale::ZhTw => format!(
                "{base_prompt}\n\n## 額外要求\n以下是使用者補充的日報偏好，請在不違背前述結構與約束的前提下盡量滿足：\n{custom_prompt}"
            ),
            AppLocale::En => format!(
                "{base_prompt}\n\n## Additional Preferences\nPlease follow the user's extra report preferences below as much as possible without breaking the structure and constraints above:\n{custom_prompt}"
            ),
        }
    } else {
        base_prompt
    }
}

/// 创建分析器
pub fn create_analyzer(
    mode: AiMode,
    provider: AiProvider,
    endpoint: &str,
    model: &str,
    api_key: Option<&str>,
    custom_prompt: &str,
    locale: AppLocale,
) -> Box<dyn Analyzer + Send + Sync> {
    log::info!(
        "create_analyzer: mode={:?}, provider={:?}, endpoint={}, model={}, has_api_key={}",
        mode,
        provider,
        endpoint,
        model,
        api_key.is_some()
    );
    match mode {
        AiMode::Local => Box::new(local::LocalAnalyzer::new(
            endpoint,
            model,
            custom_prompt,
            locale,
        )),
        AiMode::Summary => Box::new(summary::SummaryAnalyzer::new(
            provider,
            endpoint,
            model,
            api_key,
            custom_prompt,
            locale,
        )),
        AiMode::Cloud => Box::new(cloud::CloudAnalyzer::new(
            endpoint,
            api_key.unwrap_or(""),
            model,
            custom_prompt,
            locale,
        )),
    }
}

/// 格式化时长（秒 -> 可读字符串，精确到秒）
pub fn format_duration(seconds: i64) -> String {
    format_duration_for_locale(seconds, AppLocale::ZhCn)
}

pub fn format_duration_for_locale(seconds: i64, locale: AppLocale) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    match locale {
        AppLocale::En => {
            if hours > 0 {
                format!("{hours}h {minutes}m {secs}s")
            } else if minutes > 0 {
                format!("{minutes}m {secs}s")
            } else {
                format!("{secs}s")
            }
        }
        AppLocale::ZhTw => {
            if hours > 0 {
                format!("{hours}小時{minutes}分{secs}秒")
            } else if minutes > 0 {
                format!("{minutes}分{secs}秒")
            } else {
                format!("{secs}秒")
            }
        }
        AppLocale::ZhCn => {
            if hours > 0 {
                format!("{hours}小时{minutes}分{secs}秒")
            } else if minutes > 0 {
                format!("{minutes}分{secs}秒")
            } else {
                format!("{secs}秒")
            }
        }
    }
}

use std::collections::HashMap;

pub fn translate_category_name(
    category_key: &str,
    locale: AppLocale,
    category_overrides: &HashMap<String, String>,
) -> String {
    if let Some(name) = category_overrides.get(category_key) {
        return name.clone();
    }
    match locale {
        AppLocale::ZhCn => match category_key {
            "development" => "开发工具".to_string(),
            "browser" => "浏览器".to_string(),
            "communication" => "通讯协作".to_string(),
            "office" => "办公软件".to_string(),
            "design" => "设计工具".to_string(),
            "entertainment" => "娱乐摸鱼".to_string(),
            "other" => "其他".to_string(),
            _ => crate::categorize::get_category_name(category_key).to_string(),
        },
        AppLocale::ZhTw => match category_key {
            "development" => "開發工具".to_string(),
            "browser" => "瀏覽器".to_string(),
            "communication" => "通訊協作".to_string(),
            "office" => "辦公軟體".to_string(),
            "design" => "設計工具".to_string(),
            "entertainment" => "娛樂".to_string(),
            "other" => "其他".to_string(),
            _ => crate::categorize::get_category_name(category_key).to_string(),
        },
        AppLocale::En => match category_key {
            "development" => "Development".to_string(),
            "browser" => "Browser".to_string(),
            "communication" => "Communication".to_string(),
            "office" => "Office".to_string(),
            "design" => "Design".to_string(),
            "entertainment" => "Entertainment".to_string(),
            "other" => "Other".to_string(),
            _ => category_key.to_string(),
        },
    }
}

pub fn translate_semantic_category_name(
    category_label: &str,
    locale: AppLocale,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    if let Some(name) = semantic_overrides.get(category_label) {
        return name.clone();
    }
    match locale {
        AppLocale::ZhCn => match category_label {
            "编码开发" => "编码开发".to_string(),
            "内容撰写" => "内容撰写".to_string(),
            "资料阅读" => "资料阅读".to_string(),
            "资料调研" => "资料调研".to_string(),
            "任务规划" => "任务规划".to_string(),
            "设计创作" => "设计创作".to_string(),
            "AI 协作" => "AI 协作".to_string(),
            "即时聊天" => "即时聊天".to_string(),
            "会议沟通" => "会议沟通".to_string(),
            "视频内容" => "视频内容".to_string(),
            "音乐音频" => "音乐音频".to_string(),
            "休息娱乐" => "休息娱乐".to_string(),
            "未知活动" => "未知活动".to_string(),
            "代码评审" => "代码评审".to_string(),
            "工作跟进" => "工作跟进".to_string(),
            _ => category_label.to_string(),
        },
        AppLocale::ZhTw => match category_label {
            "编码开发" => "編碼開發".to_string(),
            "内容撰写" => "內容撰寫".to_string(),
            "资料阅读" => "資料閱讀".to_string(),
            "资料调研" => "資料調研".to_string(),
            "任务规划" => "任務規劃".to_string(),
            "设计创作" => "設計創作".to_string(),
            "AI 协作" => "AI 協作".to_string(),
            "即时聊天" => "即時聊天".to_string(),
            "会议沟通" => "會議溝通".to_string(),
            "视频内容" => "影片內容".to_string(),
            "音乐音频" => "音樂音訊".to_string(),
            "休息娱乐" => "休息娛樂".to_string(),
            "未知活动" => "未知活動".to_string(),
            "代码评审" => "程式碼審查".to_string(),
            "工作跟进" => "工作跟進".to_string(),
            _ => category_label.to_string(),
        },
        AppLocale::En => match category_label {
            "编码开发" => "Development".to_string(),
            "内容撰写" => "Writing".to_string(),
            "资料阅读" => "Reading".to_string(),
            "资料调研" => "Research".to_string(),
            "任务规划" => "Planning".to_string(),
            "设计创作" => "Design".to_string(),
            "AI 协作" => "AI Collaboration".to_string(),
            "即时聊天" => "Chat".to_string(),
            "会议沟通" => "Meetings".to_string(),
            "视频内容" => "Video".to_string(),
            "音乐音频" => "Audio".to_string(),
            "休息娱乐" => "Leisure".to_string(),
            "未知活动" => "Unknown".to_string(),
            "代码评审" => "Code Review".to_string(),
            "工作跟进" => "Follow-up".to_string(),
            _ => category_label.to_string(),
        },
    }
}

fn format_hour_range(hour: i32) -> String {
    let normalized_hour = hour.rem_euclid(24);
    format!(
        "{:02}:00-{:02}:00",
        normalized_hour,
        (normalized_hour + 1).rem_euclid(24)
    )
}

fn wrap_with_range_label(range: String, duration: String, locale: AppLocale) -> String {
    match locale {
        AppLocale::En => format!("{range} ({duration})"),
        _ => format!("{range}（{duration}）"),
    }
}

pub fn generate_hourly_activity_summary_for_locale(
    stats: &DailyStats,
    locale: AppLocale,
) -> Option<String> {
    let mut active_buckets = stats
        .hourly_activity_distribution
        .iter()
        .filter(|bucket| bucket.duration > 0)
        .map(|bucket| (bucket.hour, bucket.duration))
        .collect::<Vec<_>>();

    if active_buckets.is_empty() {
        return None;
    }

    active_buckets.sort_by(|(left_hour, left_duration), (right_hour, right_duration)| {
        right_duration
            .cmp(left_duration)
            .then_with(|| left_hour.cmp(right_hour))
    });

    let (peak_hour, peak_duration) = active_buckets[0];
    let separator = if locale == AppLocale::En { ", " } else { "、" };
    let top_ranges = active_buckets
        .iter()
        .take(3)
        .map(|(hour, duration)| {
            wrap_with_range_label(
                format_hour_range(*hour),
                format_duration_for_locale(*duration, locale),
                locale,
            )
        })
        .collect::<Vec<_>>()
        .join(separator);

    Some(match locale {
        AppLocale::ZhCn => format!(
            "- 高峰时段: {}（{}）\n- 活跃小时数: {} 个\n- 主要活跃区间: {}\n",
            format_hour_range(peak_hour),
            format_duration_for_locale(peak_duration, locale),
            active_buckets.len(),
            top_ranges
        ),
        AppLocale::ZhTw => format!(
            "- 高峰時段: {}（{}）\n- 活躍小時數: {} 個\n- 主要活躍區間: {}\n",
            format_hour_range(peak_hour),
            format_duration_for_locale(peak_duration, locale),
            active_buckets.len(),
            top_ranges
        ),
        AppLocale::En => format!(
            "- Peak hour: {} ({})\n- Active hours: {}\n- Main active ranges: {}\n",
            format_hour_range(peak_hour),
            format_duration_for_locale(peak_duration, locale),
            active_buckets.len(),
            top_ranges
        ),
    })
}

/// 生成统计摘要
pub fn generate_stats_summary_for_locale(
    stats: &DailyStats,
    locale: AppLocale,
    category_overrides: &HashMap<String, String>,
) -> String {
    let mut summary = String::new();

    match locale {
        AppLocale::ZhCn => {
            summary.push_str("## 今日工作统计\n\n");
            summary.push_str(&format!(
                "- 总工作时长: {}\n",
                format_duration_for_locale(stats.total_duration, locale)
            ));
            summary.push_str(&format!("- 截图数量: {}\n\n", stats.screenshot_count));
            summary.push_str("### 应用使用时长\n\n");
        }
        AppLocale::ZhTw => {
            summary.push_str("## 今日工作統計\n\n");
            summary.push_str(&format!(
                "- 總工作時長: {}\n",
                format_duration_for_locale(stats.total_duration, locale)
            ));
            summary.push_str(&format!("- 截圖數量: {}\n\n", stats.screenshot_count));
            summary.push_str("### 應用使用時長\n\n");
        }
        AppLocale::En => {
            summary.push_str("## Daily Work Stats\n\n");
            summary.push_str(&format!(
                "- Total work duration: {}\n",
                format_duration_for_locale(stats.total_duration, locale)
            ));
            summary.push_str(&format!(
                "- Screenshot count: {}\n\n",
                stats.screenshot_count
            ));
            summary.push_str("### App usage\n\n");
        }
    }

    for app in &stats.app_usage {
        summary.push_str(&format!(
            "- {}: {}\n",
            app.app_name,
            format_duration_for_locale(app.duration, locale)
        ));
    }

    summary.push_str(match locale {
        AppLocale::ZhCn => "\n### 分类时间分布\n\n",
        AppLocale::ZhTw => "\n### 分類時間分布\n\n",
        AppLocale::En => "\n### Category breakdown\n\n",
    });
    for cat in &stats.category_usage {
        let percentage = if stats.total_duration > 0 {
            (cat.duration as f64 / stats.total_duration as f64 * 100.0) as i32
        } else {
            0
        };
        summary.push_str(&format!(
            "- {}: {} ({}%)\n",
            translate_category_name(&cat.category, locale, category_overrides),
            format_duration_for_locale(cat.duration, locale),
            percentage
        ));
    }

    if let Some(hourly_summary) = generate_hourly_activity_summary_for_locale(stats, locale) {
        summary.push_str(match locale {
            AppLocale::ZhCn => "\n### 按小时活跃度\n\n",
            AppLocale::ZhTw => "\n### 按小時活躍度\n\n",
            AppLocale::En => "\n### Hourly activity\n\n",
        });
        summary.push_str(&hourly_summary);
    }

    summary
}

/// 生成活动时间线文本（用于日报正文，默认折叠）
/// 显示按时间排序的应用使用路径
pub fn generate_activity_timeline(
    activities: &[crate::database::Activity],
    locale: AppLocale,
) -> String {
    if activities.is_empty() {
        return String::new();
    }

    let summary_label = match locale {
        AppLocale::ZhCn => "活动时间线",
        AppLocale::ZhTw => "活動時間線",
        AppLocale::En => "Activity Timeline",
    };

    let count = activities.len();

    let mut lines = Vec::new();

    for act in activities {
        let dt = chrono::DateTime::from_timestamp(act.timestamp, 0)
            .map(|dt| dt.with_timezone(&chrono::Local))
            .map(|dt| dt.format("%H:%M").to_string())
            .unwrap_or_else(|| "??".to_string());

        let end_ts = act.timestamp;
        let start_ts = end_ts - act.duration;
        let start_dt = chrono::DateTime::from_timestamp(start_ts, 0)
            .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
            .unwrap_or_else(|| "??".to_string());

        let duration_str = format_duration_for_locale(act.duration, locale);
        let app_name = crate::categorize::normalize_display_app_name(&act.app_name);

        let title = if act.window_title.chars().count() > 60 {
            let truncated: String = act.window_title.chars().take(57).collect();
            format!("{truncated}…")
        } else {
            act.window_title.clone()
        };

        lines.push(format!(
            "| {}-{} | {} | {} | {} |",
            start_dt, dt, duration_str, app_name, title
        ));
    }

    let col_header = match locale {
        AppLocale::ZhCn => "| 时间段 | 时长 | 应用 | 窗口 |\n|--------|------|------|------|\n",
        AppLocale::ZhTw => "| 時間段 | 時長 | 應用 | 視窗 |\n|--------|------|------|------|\n",
        AppLocale::En => {
            "| Period | Duration | App | Window |\n|--------|----------|-----|--------|\n"
        }
    };

    format!(
        "<details>\n<summary>{}（{}条记录）</summary>\n\n{}{}</details>",
        summary_label,
        count,
        col_header,
        lines.join("\n")
    )
}

/// 将活动按时间连续性聚合成工作段，生成紧凑的文本摘要供 AI prompt 使用。
/// 每段输出一行：`09:00-09:45 (45分) 编码开发 | VS Code, Chrome | api.rs, github.com`
/// 0 信息丢失，条目数通常 5-20 段，prompt 大小 2-5KB。
pub fn generate_session_timeline(
    activities: &[crate::database::Activity],
    locale: AppLocale,
) -> String {
    if activities.is_empty() {
        return String::new();
    }

    let mut sessions = crate::work_intelligence::build_work_sessions(activities);
    if sessions.is_empty() {
        return String::new();
    }

    // 按时间正序排列（build_work_sessions 默认倒序）
    sessions.sort_by_key(|s| s.start_timestamp);

    let header_label = match locale {
        AppLocale::ZhCn => format!("### 工作段（{}段）", sessions.len()),
        AppLocale::ZhTw => format!("### 工作段（{}段）", sessions.len()),
        AppLocale::En => format!("### Work Sessions ({})", sessions.len()),
    };

    let mut lines = vec![header_label];

    for session in &sessions {
        let start = format_hhmm(session.start_timestamp);
        let end = format_hhmm(session.end_timestamp);
        let dur = format_duration_for_locale(session.duration, locale);

        let apps: String = session
            .top_apps
            .iter()
            .take(3)
            .map(|a| crate::categorize::normalize_display_app_name(&a.name))
            .collect::<Vec<_>>()
            .join(", ");

        let mut context_parts: Vec<String> = Vec::new();
        if !session.title.is_empty() {
            let truncated: String = session.title.chars().take(50).collect();
            context_parts.push(truncated);
        }
        for domain in session.browser_domains.iter().take(3) {
            context_parts.push(domain.clone());
        }
        let context = if context_parts.is_empty() {
            "-".to_string()
        } else {
            context_parts.join(", ")
        };

        lines.push(format!(
            "{}-{} ({}) {} | {} | {}",
            start, end, dur, session.intent_label, apps, context
        ));
    }

    lines.join("\n")
}

fn format_hhmm(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "??".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        append_custom_prompt_for_locale, generate_stats_summary_for_locale,
        normalize_custom_prompt, translate_semantic_category_name, AppLocale,
    };
    use crate::database::{DailyStats, HourlyActivityBucket};
    use std::collections::HashMap;

    #[test]
    fn 空白附加提示词应被忽略() {
        assert_eq!(normalize_custom_prompt("   "), None);
    }

    #[test]
    fn 应将附加提示词追加到基础提示词末尾() {
        let prompt = append_custom_prompt_for_locale("基础提示".to_string(), "输出偏正式一些", AppLocale::ZhCn);

        assert!(prompt.contains("基础提示"));
        assert!(prompt.contains("额外要求"));
        assert!(prompt.contains("输出偏正式一些"));
    }

    #[test]
    fn 英文附加提示词应使用英文标题() {
        let prompt = append_custom_prompt_for_locale(
            "Base prompt".to_string(),
            "Keep it concise",
            AppLocale::En,
        );

        assert!(prompt.contains("Additional Preferences"));
        assert!(prompt.contains("Keep it concise"));
    }

    #[test]
    fn 统计摘要应包含按小时活跃度信息() {
        let stats = DailyStats {
            total_duration: 5400,
            screenshot_count: 3,
            hourly_activity_distribution: vec![
                HourlyActivityBucket {
                    hour: 10,
                    duration: 3600,
                },
                HourlyActivityBucket {
                    hour: 14,
                    duration: 1800,
                },
            ],
            ..Default::default()
        };

        let empty = HashMap::new();
        let summary = generate_stats_summary_for_locale(&stats, AppLocale::ZhCn, &empty);
        let english_summary = generate_stats_summary_for_locale(&stats, AppLocale::En, &empty);

        assert!(summary.contains("按小时活跃度"));
        assert!(summary.contains("高峰时段"));
        assert!(summary.contains("10:00-11:00"));
        assert!(english_summary.contains("Hourly activity"));
        assert!(english_summary.contains("Peak hour"));
    }

    #[test]
    fn 英文语义分类应翻译为英文标签() {
        let empty = HashMap::new();
        assert_eq!(
            translate_semantic_category_name("编码开发", AppLocale::En, &empty),
            "Development"
        );
        assert_eq!(
            translate_semantic_category_name("未知活动", AppLocale::En, &empty),
            "Unknown"
        );
    }
}
