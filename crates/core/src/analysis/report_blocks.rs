//! 日报"动态统计区块"占位符与读时替换 + 段落积木化
//!
//! 解决 issue #80 / #79.cmt3：保存的日报 markdown 里固化的时长数字会变陈旧（自动生成
//! 触发后用户继续工作的活动不会被回填）。读取/展示时使用最新的 DailyStats 重新渲染
//! 这些区块，保证每次看到的数字都是当前的。
//!
//! 段落积木化（方向 A）：每个统计区块是一个 `StatsBlock` 枚举变体，`assemble()`
//! 按调用方提供的顺序依次渲染 + wrap_block，输出带标记的 markdown 字符串。
//! 调用方（summary.rs / local.rs）不再逐个 push_str，而是构造 block 数组。

use crate::analysis::{
    format_duration_for_locale, generate_hourly_activity_summary_for_locale,
    translate_category_name, translate_semantic_category_name, AppLocale,
};
use crate::database::{DailyStats, DomainUsage};
use std::collections::HashMap;

pub const BLOCK_CATEGORY_TABLE: &str = "CATEGORY_TABLE";
pub const BLOCK_APP_USAGE_TABLE: &str = "APP_USAGE_TABLE";
pub const BLOCK_HOURLY_SUMMARY: &str = "HOURLY_SUMMARY";
pub const BLOCK_DOMAIN_USAGE_TABLE: &str = "DOMAIN_USAGE_TABLE";
pub const BLOCK_AI_ANALYSIS: &str = "AI_ANALYSIS";
pub const BLOCK_LOCAL_OVERVIEW: &str = "LOCAL_OVERVIEW";
pub const BLOCK_LOCAL_CATEGORY: &str = "LOCAL_CATEGORY";
pub const BLOCK_LOCAL_APP_USAGE: &str = "LOCAL_APP_USAGE";
pub const BLOCK_LOCAL_DOMAIN_USAGE: &str = "LOCAL_DOMAIN_USAGE";
pub const BLOCK_PROJECT_USAGE: &str = "PROJECT_USAGE";

/// 段落积木枚举——每个变体对应一个可独立渲染的统计区块。
/// 调用方（summary.rs / local.rs）构造 `Vec<StatsBlock>` 传入 `assemble()`，
/// 由 assemble 按顺序渲染 + wrap_block，输出带标记的 markdown。
///
/// AI 分析段和活动时间线不纳入此枚举——它们不是 stats 驱动的纯函数，
/// 由调用方在 assemble 前后直接插入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatsBlock {
    /// summary 模式：时间分配表格
    CategoryTable,
    /// summary 模式：应用使用明细表格
    AppUsageTable,
    /// summary + local：按小时活跃度
    HourlySummary,
    /// summary 模式：网站访问明细表格
    DomainUsageTable,
    /// local 模式：今日概览
    LocalOverview,
    /// local 模式：时间分配列表
    LocalCategory,
    /// local 模式：应用使用列表
    LocalAppUsage,
    /// local 模式：网站访问列表
    LocalDomainUsage,
    /// summary + local：项目投入分布（未配置项目或无命中时自动跳过）
    ProjectUsage,
}

impl StatsBlock {
    /// 对应的 BLOCK_* 常量名（用于 wrap_block 标记）
    pub fn block_name(&self) -> &'static str {
        match self {
            Self::CategoryTable => BLOCK_CATEGORY_TABLE,
            Self::AppUsageTable => BLOCK_APP_USAGE_TABLE,
            Self::HourlySummary => BLOCK_HOURLY_SUMMARY,
            Self::DomainUsageTable => BLOCK_DOMAIN_USAGE_TABLE,
            Self::LocalOverview => BLOCK_LOCAL_OVERVIEW,
            Self::LocalCategory => BLOCK_LOCAL_CATEGORY,
            Self::LocalAppUsage => BLOCK_LOCAL_APP_USAGE,
            Self::LocalDomainUsage => BLOCK_LOCAL_DOMAIN_USAGE,
            Self::ProjectUsage => BLOCK_PROJECT_USAGE,
        }
    }

    /// 用 stats 渲染该区块的 markdown 内容（不含 wrap_block 标记）
    pub fn render(
        &self,
        stats: &DailyStats,
        locale: AppLocale,
        category_overrides: &HashMap<String, String>,
        semantic_overrides: &HashMap<String, String>,
    ) -> String {
        match self {
            Self::CategoryTable => render_category_table(stats, locale, category_overrides),
            Self::AppUsageTable => render_app_usage_table(stats, locale),
            Self::HourlySummary => render_hourly_summary(stats, locale),
            Self::DomainUsageTable => render_domain_usage_table(stats, locale, semantic_overrides),
            Self::LocalOverview => render_local_overview(stats, locale),
            Self::LocalCategory => render_local_category_list(stats, locale, category_overrides),
            Self::LocalAppUsage => render_local_app_usage_list(stats, locale),
            Self::LocalDomainUsage => {
                render_local_domain_usage_list(stats, locale, semantic_overrides)
            }
            Self::ProjectUsage => render_project_usage_table(stats, locale),
        }
    }
}

fn section_number_prefix(locale: AppLocale, index: usize) -> String {
    let cjk = ["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"];

    match locale {
        AppLocale::En => format!("{index}. "),
        AppLocale::Ar => format!("{index}. "),
        _ => {
            let number = cjk
                .get(index.saturating_sub(1))
                .map(|value| (*value).to_string())
                .unwrap_or_else(|| index.to_string());
            format!("{number}、")
        }
    }
}

/// 生成带动态序号的二级标题。
pub fn format_section_heading(locale: AppLocale, index: usize, title: &str) -> String {
    format!("## {}{title}\n\n", section_number_prefix(locale, index))
}

/// 给 markdown 字符串中的 `## ` 标题加上序号（根据 locale 用中文或阿拉伯数字）。
fn number_sections_from(markdown: &str, locale: AppLocale, first_index: usize) -> (String, usize) {
    let mut counter = 0;
    let mut out = String::with_capacity(markdown.len());

    for line in markdown.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("## ") {
            counter += 1;
            out.push_str("## ");
            out.push_str(&section_number_prefix(locale, first_index + counter - 1));
            out.push_str(rest);
        } else {
            out.push_str(line);
        }
    }

    (out, counter)
}

/// 按给定顺序组装统计区块，返回带 WR_BLOCK 标记的 markdown 字符串。
/// 标题序号由 assemble 动态分配（render 函数里的标题不含序号）。
/// 空内容的块自动跳过（wrap_block 返回空串）。
pub fn assemble(
    blocks: &[StatsBlock],
    stats: &DailyStats,
    locale: AppLocale,
    category_overrides: &HashMap<String, String>,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    assemble_with_section_count(
        blocks,
        stats,
        locale,
        category_overrides,
        semantic_overrides,
    )
    .0
}

/// 按给定顺序组装统计区块，并返回实际渲染出的二级段落数量。
pub fn assemble_with_section_count(
    blocks: &[StatsBlock],
    stats: &DailyStats,
    locale: AppLocale,
    category_overrides: &HashMap<String, String>,
    semantic_overrides: &HashMap<String, String>,
) -> (String, usize) {
    let mut out = String::new();
    for block in blocks {
        let rendered = block.render(stats, locale, category_overrides, semantic_overrides);
        let wrapped = wrap_block(block.block_name(), &rendered);
        if !wrapped.is_empty() {
            out.push_str(&wrapped);
        }
    }
    // 动态编号：把 render 函数输出的无编号标题（## 时间分配）按实际顺序编号
    number_sections_from(&out, locale, 1)
}

/// 从字符串名解析回 StatsBlock（AI 编排用）
pub fn parse_block_name(name: &str) -> Option<StatsBlock> {
    match name.trim().to_uppercase().as_str() {
        "CATEGORY_TABLE" => Some(StatsBlock::CategoryTable),
        "APP_USAGE_TABLE" => Some(StatsBlock::AppUsageTable),
        "HOURLY_SUMMARY" => Some(StatsBlock::HourlySummary),
        "DOMAIN_USAGE_TABLE" => Some(StatsBlock::DomainUsageTable),
        "LOCAL_OVERVIEW" => Some(StatsBlock::LocalOverview),
        "LOCAL_CATEGORY" => Some(StatsBlock::LocalCategory),
        "LOCAL_APP_USAGE" => Some(StatsBlock::LocalAppUsage),
        "LOCAL_DOMAIN_USAGE" => Some(StatsBlock::LocalDomainUsage),
        "PROJECT_USAGE" => Some(StatsBlock::ProjectUsage),
        _ => None,
    }
}

/// 根据用户偏好（pinned）调整段落顺序：pinned 中的块排到最前（按 pinned 列表顺序）。
///
/// 注意：hidden 的过滤已移到前端显示层（`getVisibleReportSections`），生成时保留
/// 全部区块。这样用户在「管理段落」恢复隐藏区块后无需重新生成即可见——之前的
/// 实现在此过滤 hidden，导致 hidden 区块被永久写出生成内容、restore 后无法显示。
pub fn apply_preferences(blocks: Vec<StatsBlock>, pinned: &[String]) -> Vec<StatsBlock> {
    let is_pinned = |b: &StatsBlock| pinned.iter().any(|p| p == b.block_name());

    // 分成 pinned（按 pinned 列表顺序）+ 其余（保持原顺序）
    let mut pinned_blocks = Vec::new();
    for pin_name in pinned {
        if let Some(b) = blocks.iter().find(|b| b.block_name() == pin_name) {
            pinned_blocks.push(*b);
        }
    }
    let rest: Vec<StatsBlock> = blocks.into_iter().filter(|b| !is_pinned(b)).collect();

    pinned_blocks.into_iter().chain(rest).collect()
}

/// 将 AI 返回的排序结果（block name 数组）映射为 StatsBlock 数组。
/// 只保留在 available 中的 block（避免 AI 返回不存在的块），未覆盖的块按原顺序追加。
pub fn merge_ai_order(ai_order: &[String], available: &[StatsBlock]) -> Vec<StatsBlock> {
    let mut result = Vec::new();
    let mut used = std::collections::HashSet::new();

    // 按 AI 顺序，把能匹配上的 block 放进来
    for name in ai_order {
        if let Some(block) = parse_block_name(name) {
            if available.contains(&block) && !used.contains(&block) {
                result.push(block);
                used.insert(block);
            }
        }
    }

    // AI 没覆盖到的 block，保持原顺序追加
    for block in available {
        if !used.contains(block) {
            result.push(*block);
        }
    }

    result
}
pub fn default_summary_order() -> Vec<StatsBlock> {
    vec![
        StatsBlock::CategoryTable,
        StatsBlock::ProjectUsage,
        StatsBlock::AppUsageTable,
        StatsBlock::HourlySummary,
        StatsBlock::DomainUsageTable,
    ]
}

/// local 模式的默认段落顺序
pub fn default_local_order() -> Vec<StatsBlock> {
    vec![
        StatsBlock::LocalOverview,
        StatsBlock::LocalCategory,
        StatsBlock::ProjectUsage,
        StatsBlock::LocalAppUsage,
        StatsBlock::HourlySummary,
        StatsBlock::LocalDomainUsage,
    ]
}

const BLOCK_PREFIX_START: &str = "<!-- WR_BLOCK_START:";
const BLOCK_PREFIX_END: &str = "<!-- WR_BLOCK_END:";
const MARKER_SUFFIX: &str = " -->";

/// 把内容包入占位符标记中。如果传入 content 是空串则返回空串（不插入空块）。
pub fn wrap_block(name: &str, content: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    format!(
        "{prefix_start}{name}{suffix}\n{content}{newline}{prefix_end}{name}{suffix}\n",
        prefix_start = BLOCK_PREFIX_START,
        prefix_end = BLOCK_PREFIX_END,
        suffix = MARKER_SUFFIX,
        newline = if content.ends_with('\n') { "" } else { "\n" }
    )
}

/// 读时用最新 stats 重新渲染所有已知统计区块。未识别 / 未出现的块原样保留。
pub fn render_report_with_live_stats(
    content: &str,
    stats: &DailyStats,
    locale: AppLocale,
    category_overrides: &HashMap<String, String>,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    let mut output = content.to_string();
    output = replace_block(
        &output,
        BLOCK_CATEGORY_TABLE,
        &render_category_table(stats, locale, category_overrides),
    );
    output = replace_block(
        &output,
        BLOCK_APP_USAGE_TABLE,
        &render_app_usage_table(stats, locale),
    );
    output = replace_block(
        &output,
        BLOCK_HOURLY_SUMMARY,
        &render_hourly_summary(stats, locale),
    );
    output = replace_block(
        &output,
        BLOCK_DOMAIN_USAGE_TABLE,
        &render_domain_usage_table(stats, locale, semantic_overrides),
    );
    output = replace_block(
        &output,
        BLOCK_LOCAL_OVERVIEW,
        &render_local_overview(stats, locale),
    );
    output = replace_block(
        &output,
        BLOCK_LOCAL_CATEGORY,
        &render_local_category_list(stats, locale, category_overrides),
    );
    output = replace_block(
        &output,
        BLOCK_LOCAL_APP_USAGE,
        &render_local_app_usage_list(stats, locale),
    );
    output = replace_block(
        &output,
        BLOCK_LOCAL_DOMAIN_USAGE,
        &render_local_domain_usage_list(stats, locale, semantic_overrides),
    );
    output = replace_block(
        &output,
        BLOCK_PROJECT_USAGE,
        &render_project_usage_table(stats, locale),
    );
    output
}

fn replace_block(content: &str, name: &str, fresh: &str) -> String {
    let start = format!("{BLOCK_PREFIX_START}{name}{MARKER_SUFFIX}");
    let end = format!("{BLOCK_PREFIX_END}{name}{MARKER_SUFFIX}");

    let mut result = String::with_capacity(content.len());
    let mut cursor = 0usize;

    loop {
        let Some(rel_start) = content[cursor..].find(&start) else {
            result.push_str(&content[cursor..]);
            break;
        };
        let abs_start = cursor + rel_start;
        result.push_str(&content[cursor..abs_start]);

        let Some(rel_end) = content[abs_start..].find(&end) else {
            // 未配对，原样保留
            result.push_str(&content[abs_start..]);
            break;
        };
        let abs_end = abs_start + rel_end + end.len();

        result.push_str(&start);
        result.push('\n');
        let trimmed = fresh.trim_matches('\n');
        if !trimmed.is_empty() {
            result.push_str(trimmed);
            result.push('\n');
        }
        result.push_str(&end);
        cursor = abs_end;
    }

    result
}

// ─────────── summary mode 的块渲染器 ───────────

fn usage_percentage(duration: i64, total_duration: i64) -> i32 {
    if total_duration > 0 {
        (duration as f64 / total_duration as f64 * 100.0).round() as i32
    } else {
        0
    }
}

fn format_hour_range_for_note(hour: i32) -> String {
    let start = hour.clamp(0, 23);
    let end = (start + 1) % 24;
    format!("{start:02}:00-{end:02}:00")
}

fn category_table_takeaway(
    stats: &DailyStats,
    locale: AppLocale,
    category_overrides: &HashMap<String, String>,
) -> String {
    let Some(top_category) = stats.category_usage.first() else {
        return String::new();
    };
    let name = translate_category_name(&top_category.category, locale, category_overrides);
    let duration = format_duration_for_locale(top_category.duration, locale);
    let percentage = usage_percentage(top_category.duration, stats.total_duration);

    match locale {
        AppLocale::ZhCn => format!(
            "> **短结论：** 主要时间集中在{name}，累计{duration}，占总记录时长 {percentage}%。\n\n"
        ),
        AppLocale::ZhTw => format!(
            "> **短結論：** 主要時間集中在{name}，累計{duration}，佔總記錄時長 {percentage}%。\n\n"
        ),
        AppLocale::En => format!(
            "> **Takeaway:** Most tracked time went to {name}, totaling {duration} ({percentage}% of the recorded day).\n\n"
        ),
        AppLocale::Ar => format!(
            "> **الخلاصة:** ذهب معظم الوقت المتتبع إلى {name}، بإجمالي {duration} ({percentage}% من اليوم المسجل).\n\n"
        ),
    }
}

fn app_usage_table_takeaway(stats: &DailyStats, locale: AppLocale) -> String {
    let Some(top_app) = stats.app_usage.first() else {
        return String::new();
    };
    let duration = format_duration_for_locale(top_app.duration, locale);
    let app_count = stats.app_usage.len();

    match locale {
        AppLocale::ZhCn => format!(
            "> **短结论：** 使用最久的应用是 {}，累计{}；今天共记录 {} 个应用。\n\n",
            top_app.app_name, duration, app_count
        ),
        AppLocale::ZhTw => format!(
            "> **短結論：** 使用最久的應用是 {}，累計{}；今天共記錄 {} 個應用。\n\n",
            top_app.app_name, duration, app_count
        ),
        AppLocale::En => format!(
            "> **Takeaway:** The most-used app was {}, totaling {}; {} apps were recorded today.\n\n",
            top_app.app_name, duration, app_count
        ),
        AppLocale::Ar => format!(
            "> **الخلاصة:** التطبيق الأكثر استخداماً كان {}، بإجمالي {}؛ تم تسجيل {} تطبيقات اليوم.\n\n",
            top_app.app_name, duration, app_count
        ),
    }
}

fn hourly_summary_takeaway(stats: &DailyStats, locale: AppLocale) -> String {
    let active_buckets = stats
        .hourly_activity_distribution
        .iter()
        .filter(|bucket| bucket.duration > 0)
        .collect::<Vec<_>>();
    let Some(peak_bucket) = active_buckets
        .iter()
        .max_by(|left, right| {
            left.duration
                .cmp(&right.duration)
                .then_with(|| right.hour.cmp(&left.hour))
        })
        .copied()
    else {
        return String::new();
    };
    let peak_range = format_hour_range_for_note(peak_bucket.hour);
    let duration = format_duration_for_locale(peak_bucket.duration, locale);
    let active_hours = active_buckets.len();

    match locale {
        AppLocale::ZhCn => format!(
            "> **短结论：** 活跃高峰出现在{peak_range}，该小时记录{duration}；全天共有 {active_hours} 个活跃小时。\n\n"
        ),
        AppLocale::ZhTw => format!(
            "> **短結論：** 活躍高峰出現在{peak_range}，該小時記錄{duration}；全天共有 {active_hours} 個活躍小時。\n\n"
        ),
        AppLocale::En => format!(
            "> **Takeaway:** Activity peaked at {peak_range}, with {duration} recorded in that hour across {active_hours} active hours.\n\n"
        ),
        AppLocale::Ar => format!(
            "> **الخلاصة:** بلغ النشاط ذروته في {peak_range}، حيث تم تسجيل {duration} في تلك الساعة عبر {active_hours} ساعات نشطة.\n\n"
        ),
    }
}

fn domain_usage_table_takeaway(
    stats: &DailyStats,
    locale: AppLocale,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    let Some(top_domain) = stats.domain_usage.first() else {
        return String::new();
    };
    let domain = format_domain_label_local(top_domain, locale, semantic_overrides);
    let duration = format_duration_for_locale(top_domain.duration, locale);
    let domain_count = stats.domain_usage.len();

    match locale {
        AppLocale::ZhCn => format!(
            "> **短结论：** 访问时间最长的网站是 {domain}，累计{duration}；今天共记录 {domain_count} 个网站。\n\n"
        ),
        AppLocale::ZhTw => format!(
            "> **短結論：** 造訪時間最長的網站是 {domain}，累計{duration}；今天共記錄 {domain_count} 個網站。\n\n"
        ),
        AppLocale::En => format!(
            "> **Takeaway:** The longest website visit was {domain}, totaling {duration}; {domain_count} websites were recorded today.\n\n"
        ),
        AppLocale::Ar => format!(
            "> **الخلاصة:** أطول زيارة لموقع ويب كانت {domain}، بإجمالي {duration}؛ تم تسجيل {domain_count} مواقع اليوم.\n\n"
        ),
    }
}

pub fn render_category_table(
    stats: &DailyStats,
    locale: AppLocale,
    category_overrides: &HashMap<String, String>,
) -> String {
    if stats.category_usage.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 时间分配\n\n",
        AppLocale::ZhTw => "## 時間分配\n\n",
        AppLocale::En => "## Time Allocation\n\n",
        AppLocale::Ar => "## تخصيص الوقت\n\n",
    });
    out.push_str(&category_table_takeaway(stats, locale, category_overrides));
    out.push_str(match locale {
        AppLocale::ZhCn => "| 类别 | 时长 | 占比 |\n|:--|--:|--:|\n",
        AppLocale::ZhTw => "| 類別 | 時長 | 佔比 |\n|:--|--:|--:|\n",
        AppLocale::En => "| Category | Duration | Share |\n|:--|--:|--:|\n",
        AppLocale::Ar => "| الفئة | المدة | النسبة |\n|:--|--:|--:|\n",
    });
    for cat in &stats.category_usage {
        let percentage = usage_percentage(cat.duration, stats.total_duration);
        out.push_str(&format!(
            "| {} | {} | {}% |\n",
            translate_category_name(&cat.category, locale, category_overrides),
            format_duration_for_locale(cat.duration, locale),
            percentage
        ));
    }
    out.push('\n');
    out
}

pub fn render_app_usage_table(stats: &DailyStats, locale: AppLocale) -> String {
    if stats.app_usage.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 应用使用明细\n\n",
        AppLocale::ZhTw => "## 應用使用明細\n\n",
        AppLocale::En => "## App Details\n\n",
        AppLocale::Ar => "## تفاصيل التطبيقات\n\n",
    });
    out.push_str(&app_usage_table_takeaway(stats, locale));
    out.push_str(match locale {
        AppLocale::ZhCn => "| 序号 | 应用名称 | 使用时长 |\n|--:|:--|--:|\n",
        AppLocale::ZhTw => "| 序號 | 應用名稱 | 使用時長 |\n|--:|:--|--:|\n",
        AppLocale::En => "| # | App | Duration |\n|--:|:--|--:|\n",
        AppLocale::Ar => "| # | التطبيق | المدة |\n|--:|:--|--:|\n",
    });
    for (index, app) in stats.app_usage.iter().enumerate() {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            index + 1,
            app.app_name,
            format_duration_for_locale(app.duration, locale)
        ));
    }
    out.push('\n');
    out
}

pub fn render_hourly_summary(stats: &DailyStats, locale: AppLocale) -> String {
    let Some(hourly) = generate_hourly_activity_summary_for_locale(stats, locale) else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 按小时活跃度\n\n",
        AppLocale::ZhTw => "## 按小時活躍度\n\n",
        AppLocale::En => "## Hourly Activity\n\n",
        AppLocale::Ar => "## النشاط بالساعة\n\n",
    });
    out.push_str(&hourly_summary_takeaway(stats, locale));
    out.push_str(&hourly);
    out.push('\n');
    out
}

pub fn render_domain_usage_table(
    stats: &DailyStats,
    locale: AppLocale,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    if stats.domain_usage.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 网站访问明细\n\n",
        AppLocale::ZhTw => "## 網站造訪明細\n\n",
        AppLocale::En => "## Website Details\n\n",
        AppLocale::Ar => "## تفاصيل المواقع\n\n",
    });
    out.push_str(&domain_usage_table_takeaway(
        stats,
        locale,
        semantic_overrides,
    ));
    out.push_str(match locale {
        AppLocale::ZhCn => "| 序号 | 网站域名 | 访问时长 |\n|--:|:--|--:|\n",
        AppLocale::ZhTw => "| 序號 | 網站網域 | 造訪時長 |\n|--:|:--|--:|\n",
        AppLocale::En => "| # | Domain | Duration |\n|--:|:--|--:|\n",
        AppLocale::Ar => "| # | الموقع | المدة |\n|--:|:--|--:|\n",
    });
    for (index, domain) in stats.domain_usage.iter().enumerate() {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            index + 1,
            format_domain_label_local(domain, locale, semantic_overrides),
            format_duration_for_locale(domain.duration, locale)
        ));
    }
    out.push('\n');
    out
}

pub fn render_project_usage_table(stats: &DailyStats, locale: AppLocale) -> String {
    let has_named_project = stats
        .project_usage
        .iter()
        .any(|p| p.key != crate::project::UNASSIGNED_PROJECT_KEY && p.duration > 0);
    if !has_named_project {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 项目投入分布\n\n",
        AppLocale::ZhTw => "## 專案投入分布\n\n",
        AppLocale::En => "## Project Breakdown\n\n",
        AppLocale::Ar => "## توزيع المشاريع\n\n",
    });
    out.push_str(match locale {
        AppLocale::ZhCn => "| 项目 | 时长 | 占比 |\n|:--|--:|--:|\n",
        AppLocale::ZhTw => "| 專案 | 時長 | 佔比 |\n|:--|--:|--:|\n",
        AppLocale::En => "| Project | Duration | Share |\n|:--|--:|--:|\n",
        AppLocale::Ar => "| المشروع | المدة | النسبة |\n|:--|--:|--:|\n",
    });
    for project in &stats.project_usage {
        if project.duration <= 0 {
            continue;
        }
        let label = if project.key == crate::project::UNASSIGNED_PROJECT_KEY {
            match locale {
                AppLocale::ZhCn => "未归属",
                AppLocale::ZhTw => "未歸屬",
                AppLocale::En => "Unassigned",
                AppLocale::Ar => "غير مصنّف",
            }
        } else {
            project.name.as_str()
        };
        let percentage = usage_percentage(project.duration, stats.total_duration);
        out.push_str(&format!(
            "| {} | {} | {}% |\n",
            label,
            format_duration_for_locale(project.duration, locale),
            percentage
        ));
    }
    out.push('\n');
    out
}

// ─────────── local mode 的块渲染器（与 summary mode 表格风格不同，是列表风格）───────────

pub fn render_local_overview(stats: &DailyStats, locale: AppLocale) -> String {
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 今日概览\n\n",
        AppLocale::ZhTw => "## 今日概覽\n\n",
        AppLocale::En => "## Overview\n\n",
        AppLocale::Ar => "## نظرة عامة\n\n",
    });
    let line = match locale {
        AppLocale::ZhCn => format!(
            "- **总工作时长**: {}\n- **截图数量**: {} 张\n- **使用应用**: {} 个\n",
            format_duration_for_locale(stats.total_duration, locale),
            stats.screenshot_count,
            stats.app_usage.len()
        ),
        AppLocale::ZhTw => format!(
            "- **總工作時長**: {}\n- **截圖數量**: {} 張\n- **使用應用**: {} 個\n",
            format_duration_for_locale(stats.total_duration, locale),
            stats.screenshot_count,
            stats.app_usage.len()
        ),
        AppLocale::En => format!(
            "- **Total work duration**: {}\n- **Screenshots**: {}\n- **Apps used**: {}\n",
            format_duration_for_locale(stats.total_duration, locale),
            stats.screenshot_count,
            stats.app_usage.len()
        ),
        AppLocale::Ar => format!(
            "- **إجمالي مدة العمل**: {}\n- **لقطات الشاشة**: {}\n- **التطبيقات المستخدمة**: {}\n",
            format_duration_for_locale(stats.total_duration, locale),
            stats.screenshot_count,
            stats.app_usage.len()
        ),
    };
    out.push_str(&line);
    out
}

pub fn render_local_category_list(
    stats: &DailyStats,
    locale: AppLocale,
    category_overrides: &HashMap<String, String>,
) -> String {
    if stats.category_usage.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 时间分配\n\n",
        AppLocale::ZhTw => "## 時間分配\n\n",
        AppLocale::En => "## Time allocation\n\n",
        AppLocale::Ar => "## تخصيص الوقت\n\n",
    });
    for cat in &stats.category_usage {
        let percentage = if stats.total_duration > 0 {
            (cat.duration as f64 / stats.total_duration as f64 * 100.0) as i32
        } else {
            0
        };
        out.push_str(&format!(
            "- **{}**: {} ({}%)\n",
            translate_category_name(&cat.category, locale, category_overrides),
            format_duration_for_locale(cat.duration, locale),
            percentage
        ));
    }
    out
}

pub fn render_local_app_usage_list(stats: &DailyStats, locale: AppLocale) -> String {
    if stats.app_usage.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 应用使用情况\n\n",
        AppLocale::ZhTw => "## 應用使用情況\n\n",
        AppLocale::En => "## App usage\n\n",
        AppLocale::Ar => "## استخدام التطبيقات\n\n",
    });
    for (index, app) in stats.app_usage.iter().take(5).enumerate() {
        out.push_str(&format!(
            "{}. **{}**: {}\n",
            index + 1,
            app.app_name,
            format_duration_for_locale(app.duration, locale)
        ));
    }
    out
}

pub fn render_local_domain_usage_list(
    stats: &DailyStats,
    locale: AppLocale,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    if stats.domain_usage.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(match locale {
        AppLocale::ZhCn => "## 网站访问\n\n",
        AppLocale::ZhTw => "## 網站造訪\n\n",
        AppLocale::En => "## Website visits\n\n",
        AppLocale::Ar => "## زيارات المواقع\n\n",
    });
    for domain in stats.domain_usage.iter().take(5) {
        out.push_str(&format!(
            "- **{}**: {}\n",
            format_domain_label_local(domain, locale, semantic_overrides),
            format_duration_for_locale(domain.duration, locale)
        ));
    }
    out
}

fn format_domain_label_local(
    domain: &DomainUsage,
    locale: AppLocale,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    match domain.semantic_category.as_deref().map(str::trim) {
        Some(semantic_category) if !semantic_category.is_empty() => {
            let semantic_category =
                translate_semantic_category_name(semantic_category, locale, semantic_overrides);
            match locale {
                AppLocale::En => format!("{} ({})", domain.domain, semantic_category),
                AppLocale::Ar => format!("{} ({})", domain.domain, semantic_category),
                _ => format!("{}（{}）", domain.domain, semantic_category),
            }
        }
        _ => domain.domain.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{AppUsage, CategoryUsage, DomainUsage, HourlyActivityBucket, UrlDetail};

    fn sample_stats_for_numbering() -> DailyStats {
        DailyStats {
            total_duration: 3600,
            screenshot_count: 3,
            app_usage: vec![AppUsage {
                app_name: "VS Code".to_string(),
                duration: 2400,
                count: 1,
                executable_path: None,
                screenshot_url: None,
            }],
            category_usage: vec![CategoryUsage {
                category: "development".to_string(),
                duration: 3600,
            }],
            browser_duration: 0,
            url_usage: vec![],
            domain_usage: vec![],
            domain_total_count: 0,
            browser_usage: vec![],
            work_time_duration: 3600,
            overtime_duration: 0,
            hourly_activity_distribution: vec![],
            project_usage: vec![],
        }
    }

    fn sample_stats_for_editorial_notes() -> DailyStats {
        DailyStats {
            total_duration: 7200,
            screenshot_count: 8,
            app_usage: vec![
                AppUsage {
                    app_name: "VS Code".to_string(),
                    duration: 4200,
                    count: 3,
                    executable_path: None,
                    screenshot_url: None,
                },
                AppUsage {
                    app_name: "Chrome".to_string(),
                    duration: 1800,
                    count: 2,
                    executable_path: None,
                    screenshot_url: None,
                },
            ],
            category_usage: vec![
                CategoryUsage {
                    category: "development".to_string(),
                    duration: 5400,
                },
                CategoryUsage {
                    category: "browser".to_string(),
                    duration: 1800,
                },
            ],
            browser_duration: 1800,
            url_usage: vec![],
            domain_total_count: 1,
            domain_usage: vec![DomainUsage {
                domain: "github.com".to_string(),
                duration: 1500,
                semantic_category: Some("开发协作".to_string()),
                urls: vec![UrlDetail {
                    url: "https://github.com/example/project".to_string(),
                    duration: 1500,
                }],
            }],
            browser_usage: vec![],
            work_time_duration: 7200,
            overtime_duration: 0,
            hourly_activity_distribution: vec![
                HourlyActivityBucket {
                    hour: 9,
                    duration: 1200,
                },
                HourlyActivityBucket {
                    hour: 10,
                    duration: 3000,
                },
            ],
            project_usage: vec![],
        }
    }

    #[test]
    fn wrap_block_skips_empty() {
        assert!(wrap_block(BLOCK_CATEGORY_TABLE, "").is_empty());
    }

    #[test]
    fn wrap_block_adds_markers_and_trailing_newline() {
        let wrapped = wrap_block(BLOCK_CATEGORY_TABLE, "hello\n");
        assert!(wrapped.starts_with("<!-- WR_BLOCK_START:CATEGORY_TABLE -->\n"));
        assert!(wrapped.contains("hello\n"));
        assert!(wrapped
            .trim_end()
            .ends_with("<!-- WR_BLOCK_END:CATEGORY_TABLE -->"));
    }

    #[test]
    fn replace_block_overwrites_content_between_markers() {
        let original = "before\n<!-- WR_BLOCK_START:APP_USAGE_TABLE -->\nstale rows\n<!-- WR_BLOCK_END:APP_USAGE_TABLE -->\nafter\n";
        let updated = replace_block(original, BLOCK_APP_USAGE_TABLE, "fresh rows\n");
        assert!(updated.contains("fresh rows"));
        assert!(!updated.contains("stale rows"));
        assert!(updated.starts_with("before\n"));
        assert!(updated.ends_with("after\n"));
    }

    #[test]
    fn replace_block_returns_input_when_no_markers() {
        let original = "no markers here\n";
        let updated = replace_block(original, BLOCK_CATEGORY_TABLE, "ignored");
        assert_eq!(updated, original);
    }

    #[test]
    fn replace_block_handles_unmatched_start() {
        let original = "<!-- WR_BLOCK_START:CATEGORY_TABLE -->\nnever closed\n";
        let updated = replace_block(original, BLOCK_CATEGORY_TABLE, "ignored");
        assert_eq!(updated, original);
    }

    #[test]
    fn render_report_with_live_stats_passes_through_legacy_content() {
        let legacy = "# 工作日报\n\n直接写死的旧版本，没有任何标记\n";
        let stats = DailyStats {
            total_duration: 0,
            screenshot_count: 0,
            app_usage: vec![],
            category_usage: vec![],
            browser_duration: 0,
            url_usage: vec![],
            domain_usage: vec![],
            domain_total_count: 0,
            browser_usage: vec![],
            work_time_duration: 0,
            overtime_duration: 0,
            hourly_activity_distribution: vec![],
            project_usage: vec![],
        };
        assert_eq!(
            render_report_with_live_stats(
                legacy,
                &stats,
                AppLocale::ZhCn,
                &HashMap::new(),
                &HashMap::new()
            ),
            legacy
        );
    }

    #[test]
    fn assemble应按实际区块顺序动态编号并返回段落数() {
        let stats = sample_stats_for_numbering();
        let (markdown, section_count) = assemble_with_section_count(
            &[StatsBlock::AppUsageTable, StatsBlock::CategoryTable],
            &stats,
            AppLocale::ZhCn,
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(section_count, 2);
        assert!(markdown.contains("## 一、应用使用明细"));
        assert!(markdown.contains("## 二、时间分配"));
        assert!(!markdown.contains("## 三、应用使用明细"));
    }

    #[test]
    fn 英文段落标题应按实际顺序使用阿拉伯数字编号() {
        let stats = sample_stats_for_numbering();
        let (markdown, section_count) = assemble_with_section_count(
            &[StatsBlock::CategoryTable, StatsBlock::AppUsageTable],
            &stats,
            AppLocale::En,
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(section_count, 2);
        assert!(markdown.contains("## 1. Time Allocation"));
        assert!(markdown.contains("## 2. App Details"));
    }

    #[test]
    fn summary四个数据区块应保留并在标题后给出短结论() {
        let stats = sample_stats_for_editorial_notes();
        let markdown = assemble(
            &default_summary_order(),
            &stats,
            AppLocale::ZhCn,
            &HashMap::new(),
            &HashMap::new(),
        );

        assert!(markdown.contains("## 一、时间分配\n\n> **短结论：**"));
        assert!(markdown.contains("主要时间集中在"));
        assert!(markdown.contains("| 类别 | 时长 | 占比 |"));

        assert!(markdown.contains("## 二、应用使用明细\n\n> **短结论：**"));
        assert!(markdown.contains("使用最久的应用是 VS Code"));
        assert!(markdown.contains("| 序号 | 应用名称 | 使用时长 |"));

        assert!(markdown.contains("## 三、按小时活跃度\n\n> **短结论：**"));
        assert!(markdown.contains("活跃高峰出现在"));
        assert!(markdown.contains("- 高峰时段:"));

        assert!(markdown.contains("## 四、网站访问明细\n\n> **短结论：**"));
        assert!(markdown.contains("访问时间最长的网站是 github.com"));
        assert!(markdown.contains("| 序号 | 网站域名 | 访问时长 |"));
    }

    #[test]
    fn 英文summary数据区块应使用takeaway短结论() {
        let stats = sample_stats_for_editorial_notes();
        let markdown = assemble(
            &[
                StatsBlock::CategoryTable,
                StatsBlock::AppUsageTable,
                StatsBlock::HourlySummary,
                StatsBlock::DomainUsageTable,
            ],
            &stats,
            AppLocale::En,
            &HashMap::new(),
            &HashMap::new(),
        );

        assert!(markdown.contains("## 1. Time Allocation\n\n> **Takeaway:**"));
        assert!(markdown.contains("Most tracked time went to"));
        assert!(markdown.contains("## 2. App Details\n\n> **Takeaway:**"));
        assert!(markdown.contains("The most-used app was VS Code"));
        assert!(markdown.contains("## 3. Hourly Activity\n\n> **Takeaway:**"));
        assert!(markdown.contains("Activity peaked at"));
        assert!(markdown.contains("## 4. Website Details\n\n> **Takeaway:**"));
        assert!(markdown.contains("The longest website visit was github.com"));
    }
}
