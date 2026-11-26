//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::AppState;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tauri::State;
use work_review_core::config::WebsiteSemanticRule;
use work_review_core::database::{
    apply_flex_overtime_correction, local_hour_end_timestamp, AppUsage, BrowserUsage,
    CategoryUsage, DailyStats, DomainUsage, HourlyActivityBucket, HourlyAppBucket, UrlDetail,
    UrlUsage,
};
use work_review_core::error::AppError;
use work_review_core::privacy::{apply_excluded_domains_to_stats, apply_ignored_apps_to_stats};

use super::shared::collect_privacy_filters;

/// 读取侧兜底：网站语义规则是用户的显式选择，展示时始终覆盖数据库内按时长投票的结果，
/// 避免历史回填缺失（如规则先于历史数据从其他设备同步）时概览仍显示旧分类。
fn apply_website_semantic_rules_to_stats(
    mut stats: DailyStats,
    rules: &[WebsiteSemanticRule],
) -> DailyStats {
    if rules.is_empty() {
        return stats;
    }
    for domain in &mut stats.domain_usage {
        if let Some(semantic_category) =
            work_review_core::categorize::find_website_semantic_override(
                rules,
                Some(&domain.domain),
            )
        {
            domain.semantic_category = Some(semantic_category);
        }
    }
    for browser in &mut stats.browser_usage {
        for domain in &mut browser.domains {
            if let Some(semantic_category) =
                work_review_core::categorize::find_website_semantic_override(
                    rules,
                    Some(&domain.domain),
                )
            {
                domain.semantic_category = Some(semantic_category);
            }
        }
    }
    stats
}

/// 从 `AppState` 短暂加锁提取的统计查询上下文，允许在释放全局锁后并发执行 SQLite 只读查询与聚合。
#[derive(Clone)]
struct OverviewQueryContext {
    database: work_review_core::database::Database,
    segments: Vec<work_review_core::config::WorkTimeSegment>,
    ignored_apps: Vec<String>,
    excluded_domains: Vec<String>,
    privacy_config: work_review_core::config::PrivacyConfig,
    work_time_enabled: bool,
    standard_work_hours: f64,
    website_semantic_rules: Vec<WebsiteSemanticRule>,
    projects: Vec<work_review_core::project::ProjectDefinition>,
}

impl OverviewQueryContext {
    fn from_state(state: &AppState) -> Self {
        let (ignored_apps, excluded_domains) = collect_privacy_filters(state);
        Self {
            database: state.database.clone(),
            segments: state.config.effective_work_segments(),
            ignored_apps,
            excluded_domains,
            privacy_config: state.config.privacy.clone(),
            work_time_enabled: state.config.work_time_enabled,
            standard_work_hours: state.config.standard_work_hours,
            website_semantic_rules: state.config.website_semantic_rules.clone(),
            projects: state.config.projects.clone(),
        }
    }

    fn snapshot(state: &Arc<Mutex<AppState>>) -> Result<Self, AppError> {
        let guard = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        Ok(Self::from_state(&guard))
    }

    fn finalize_day_stats(&self, mut stats: DailyStats) -> DailyStats {
        apply_flex_overtime_correction(
            &mut stats,
            self.work_time_enabled,
            self.standard_work_hours,
        );
        apply_website_semantic_rules_to_stats(stats, &self.website_semantic_rules)
    }

    fn load_single_day(&self, date: &str) -> Result<DailyStats, AppError> {
        let stats = self.database.get_daily_stats_with_projects_filtered(
            date,
            &self.segments,
            &self.ignored_apps,
            &self.excluded_domains,
            &self.projects,
            Some(&self.privacy_config),
        )?;
        Ok(self.finalize_day_stats(stats))
    }

    fn load_date_range(
        &self,
        start: chrono::NaiveDate,
        end: chrono::NaiveDate,
    ) -> Result<Vec<(String, DailyStats)>, AppError> {
        let per_day = self.database.get_daily_stats_range_with_projects_filtered(
            start,
            end,
            &self.segments,
            &self.ignored_apps,
            &self.excluded_domains,
            &self.projects,
            Some(&self.privacy_config),
        )?;
        Ok(per_day
            .into_iter()
            .map(|(date, stats)| (date, self.finalize_day_stats(stats)))
            .collect())
    }
}

pub(crate) fn load_daily_stats_for_overview(
    state: &AppState,
    date: &str,
) -> Result<DailyStats, AppError> {
    OverviewQueryContext::from_state(state).load_single_day(date)
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

/// 域名列表中的浏览器来源摘要，不携带 URL 明细。
#[derive(serde::Serialize, Debug, Clone)]
pub struct OverviewDomainBrowserSourceSummary {
    pub browser_name: String,
    pub duration: i64,
    pub percentage: f64,
}

/// 完整域名列表中的单项摘要。
#[derive(serde::Serialize, Debug, Clone)]
pub struct OverviewDomainSummary {
    pub domain: String,
    pub duration: i64,
    pub semantic_category: Option<String>,
    pub page_count: usize,
    pub browser_sources: Vec<OverviewDomainBrowserSourceSummary>,
}

/// 当前概览范围内的完整域名摘要集合。
#[derive(serde::Serialize, Debug, Clone)]
pub struct OverviewDomainCollection {
    pub total_count: usize,
    pub domains: Vec<OverviewDomainSummary>,
}

/// 域名详情中的单个浏览器来源，包含该来源下的完整 URL。
#[derive(serde::Serialize, Debug, Clone)]
pub struct OverviewDomainBrowserSource {
    pub browser_name: String,
    pub duration: i64,
    pub percentage: f64,
    pub urls: Vec<UrlDetail>,
}

/// 单个域名的完整详情。
#[derive(serde::Serialize, Debug, Clone)]
pub struct OverviewDomainDetail {
    pub domain: String,
    pub duration: i64,
    pub semantic_category: Option<String>,
    pub page_count: usize,
    pub urls: Vec<UrlDetail>,
    pub browser_sources: Vec<OverviewDomainBrowserSource>,
}

#[derive(Default)]
struct OverviewBrowserSourceAggregate {
    duration: i64,
    urls: HashMap<String, i64>,
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

fn source_percentage(duration: i64, domain_duration: i64) -> f64 {
    if duration <= 0 || domain_duration <= 0 {
        0.0
    } else {
        duration as f64 * 100.0 / domain_duration as f64
    }
}

fn collect_overview_domain_sources(
    stats: &DailyStats,
    domain: &DomainUsage,
) -> Vec<OverviewDomainBrowserSource> {
    let mut source_map: HashMap<String, OverviewBrowserSourceAggregate> = HashMap::new();

    for browser in &stats.browser_usage {
        for browser_domain in &browser.domains {
            if !browser_domain.domain.eq_ignore_ascii_case(&domain.domain) {
                continue;
            }

            let source = source_map.entry(browser.browser_name.clone()).or_default();
            source.duration += browser_domain.duration;
            for url in &browser_domain.urls {
                *source.urls.entry(url.url.clone()).or_insert(0) += url.duration;
            }
        }
    }

    let attributed_duration = source_map
        .values()
        .map(|source| source.duration)
        .sum::<i64>();
    let remaining_duration = domain.duration.saturating_sub(attributed_duration);

    if remaining_duration > 0 {
        let mut residual_urls = domain
            .urls
            .iter()
            .map(|url| (url.url.clone(), url.duration))
            .collect::<HashMap<_, _>>();
        for source in source_map.values() {
            for (url, duration) in &source.urls {
                let remaining = residual_urls.entry(url.clone()).or_insert(0);
                *remaining = remaining.saturating_sub(*duration);
            }
        }
        residual_urls.retain(|_, duration| *duration > 0);
        source_map.insert(
            "other".to_string(),
            OverviewBrowserSourceAggregate {
                duration: remaining_duration,
                urls: residual_urls,
            },
        );
    }

    let mut sources = source_map
        .into_iter()
        .filter(|(_, source)| source.duration > 0)
        .map(|(browser_name, source)| {
            let mut urls = source
                .urls
                .into_iter()
                .map(|(url, duration)| UrlDetail { url, duration })
                .collect::<Vec<_>>();
            sort_url_details(&mut urls);
            OverviewDomainBrowserSource {
                browser_name,
                duration: source.duration,
                percentage: source_percentage(source.duration, domain.duration),
                urls,
            }
        })
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.browser_name.cmp(&right.browser_name))
    });
    sources
}

fn build_overview_domain_collection(stats: &DailyStats) -> OverviewDomainCollection {
    let domains = stats
        .domain_usage
        .iter()
        .map(|domain| OverviewDomainSummary {
            domain: domain.domain.clone(),
            duration: domain.duration,
            semantic_category: domain.semantic_category.clone(),
            page_count: domain.urls.len(),
            browser_sources: collect_overview_domain_sources(stats, domain)
                .into_iter()
                .map(|source| OverviewDomainBrowserSourceSummary {
                    browser_name: source.browser_name,
                    duration: source.duration,
                    percentage: source.percentage,
                })
                .collect(),
        })
        .collect::<Vec<_>>();

    OverviewDomainCollection {
        total_count: domains.len(),
        domains,
    }
}

fn build_overview_domain_detail(stats: &DailyStats, domain: &str) -> Option<OverviewDomainDetail> {
    let target = domain.trim().trim_end_matches('.');
    let domain = stats.domain_usage.iter().find(|item| {
        item.domain
            .trim_end_matches('.')
            .eq_ignore_ascii_case(target)
    })?;
    let mut urls = domain.urls.clone();
    sort_url_details(&mut urls);

    Some(OverviewDomainDetail {
        domain: domain.domain.clone(),
        duration: domain.duration,
        semantic_category: domain.semantic_category.clone(),
        page_count: urls.len(),
        urls,
        browser_sources: collect_overview_domain_sources(stats, domain),
    })
}

fn project_overview_homepage(mut stats: DailyStats) -> DailyStats {
    sort_domain_usage(&mut stats.domain_usage);
    stats.domain_total_count = stats.domain_usage.len();
    stats.domain_usage.truncate(6);

    let visible_domains = stats
        .domain_usage
        .iter()
        .map(|domain| domain.domain.to_lowercase())
        .collect::<HashSet<_>>();
    for browser in &mut stats.browser_usage {
        browser
            .domains
            .retain(|domain| visible_domains.contains(&domain.domain.to_lowercase()));
    }

    stats
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
    let mut overtime_duration = 0;

    let mut app_usage_map: HashMap<String, AppUsage> = HashMap::new();
    let mut category_usage_map: HashMap<String, i64> = HashMap::new();
    let mut url_usage_map: HashMap<String, UrlUsage> = HashMap::new();
    let mut domain_usage_map: HashMap<String, DomainAggregate> = HashMap::new();
    let mut browser_usage_map: HashMap<String, BrowserAggregate> = HashMap::new();
    let mut project_usage_map: HashMap<String, work_review_core::project::ProjectUsage> =
        HashMap::new();
    let mut hourly_activity_distribution: Vec<HourlyActivityBucket> = (0..24)
        .map(|hour| HourlyActivityBucket { hour, duration: 0 })
        .collect();

    for day in days {
        total_duration += day.total_duration;
        screenshot_count += day.screenshot_count;
        browser_duration += day.browser_duration;
        work_time_duration += day.work_time_duration;
        overtime_duration += day.overtime_duration;

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

        for project in day.project_usage {
            let entry = project_usage_map
                .entry(project.key.clone())
                .or_insert_with(|| work_review_core::project::ProjectUsage {
                    key: project.key.clone(),
                    name: project.name.clone(),
                    color: project.color.clone(),
                    duration: 0,
                    activity_count: 0,
                });
            entry.duration += project.duration;
            entry.activity_count += project.activity_count;
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
    let domain_total_count = domain_usage.len();

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

    let mut project_usage: Vec<work_review_core::project::ProjectUsage> =
        project_usage_map.into_values().collect();
    work_review_core::project::sort_project_usage(&mut project_usage);

    DailyStats {
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
        overtime_duration,
        hourly_activity_distribution,
        project_usage,
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

/// 获取今日统计 —— 内部复用版（供 Tauri 命令与 localhost API 共用，释放全局锁后查询）
pub(crate) fn get_today_stats_inner(state: &Arc<Mutex<AppState>>) -> Result<DailyStats, AppError> {
    let ctx = OverviewQueryContext::snapshot(state)?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let stats = ctx.load_single_day(&today)?;
    let stats = apply_ignored_apps_to_stats(stats, &ctx.ignored_apps);
    let mut stats = apply_excluded_domains_to_stats(stats, &ctx.excluded_domains);
    stats.domain_total_count = stats.domain_usage.len();
    Ok(stats)
}

/// 获取今日统计
#[tauri::command]
pub async fn get_today_stats(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<DailyStats, AppError> {
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || get_today_stats_inner(&state_arc))
        .await
        .map_err(|e| AppError::Unknown(format!("统计查询任务异常: {e}")))?
}

/// 加载未做首页裁剪的完整概览统计，供首页、完整域名列表与单域名详情复用。
/// 多日与周模式使用一次范围查询在 Rust 中按日切分，消除逐天循环 SQL 开销。
fn load_full_overview_stats_with_context(
    mode: &str,
    date: Option<&str>,
    date_from: Option<&str>,
    date_to: Option<&str>,
    ctx: &OverviewQueryContext,
) -> Result<DailyStats, AppError> {
    let normalized_mode = mode.trim().to_lowercase();

    let stats = match normalized_mode.as_str() {
        "date" => {
            let (start, end) = resolve_overview_date_span(date, date_from, date_to)?;

            if start == end {
                let date_value = start.format("%Y-%m-%d").to_string();
                ctx.load_single_day(&date_value)?
            } else {
                let daily_stats = ctx
                    .load_date_range(start, end)?
                    .into_iter()
                    .map(|(_, stats)| stats)
                    .collect();
                sum_daily_stats(daily_stats)
            }
        }
        "week" => {
            let anchor = resolve_overview_anchor_date(date)?;
            let (date_from, date_to) = overview_week_bounds_for_date(anchor);
            let start = chrono::NaiveDate::parse_from_str(&date_from, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析周概览开始日期失败: {e}")))?;
            let end = chrono::NaiveDate::parse_from_str(&date_to, "%Y-%m-%d")
                .map_err(|e| AppError::Config(format!("解析周概览结束日期失败: {e}")))?;

            let daily_stats = ctx
                .load_date_range(start, end)?
                .into_iter()
                .map(|(_, stats)| stats)
                .collect();
            sum_daily_stats(daily_stats)
        }
        _ => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            ctx.load_single_day(&today)?
        }
    };

    let stats = apply_ignored_apps_to_stats(stats, &ctx.ignored_apps);
    let mut stats = apply_excluded_domains_to_stats(stats, &ctx.excluded_domains);
    stats.domain_total_count = stats.domain_usage.len();
    Ok(stats)
}

/// 获取概览统计 —— 内部复用版
pub(crate) fn get_overview_stats_inner(
    mode: String,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<DailyStats, AppError> {
    let ctx = OverviewQueryContext::snapshot(state)?;
    let stats = load_full_overview_stats_with_context(
        &mode,
        date.as_deref(),
        date_from.as_deref(),
        date_to.as_deref(),
        &ctx,
    )?;
    Ok(project_overview_homepage(stats))
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
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        get_overview_stats_inner(mode, date, date_from, date_to, &state_arc)
    })
    .await
    .map_err(|e| AppError::Unknown(format!("概览统计任务异常: {e}")))?
}

/// 获取当前概览范围内的完整域名摘要，不包含 URL 明细。
#[tauri::command]
pub async fn get_overview_domains(
    mode: String,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<OverviewDomainCollection, AppError> {
    let ctx = OverviewQueryContext::snapshot(state.inner())?;
    tokio::task::spawn_blocking(move || {
        let stats = load_full_overview_stats_with_context(
            &mode,
            date.as_deref(),
            date_from.as_deref(),
            date_to.as_deref(),
            &ctx,
        )?;
        Ok(build_overview_domain_collection(&stats))
    })
    .await
    .map_err(|e| AppError::Unknown(format!("域名摘要任务异常: {e}")))?
}

/// 获取当前概览范围内单个域名的完整 URL 与跨浏览器来源。
#[tauri::command]
pub async fn get_overview_domain_detail(
    domain: String,
    mode: String,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Option<OverviewDomainDetail>, AppError> {
    let ctx = OverviewQueryContext::snapshot(state.inner())?;
    tokio::task::spawn_blocking(move || {
        let stats = load_full_overview_stats_with_context(
            &mode,
            date.as_deref(),
            date_from.as_deref(),
            date_to.as_deref(),
            &ctx,
        )?;
        Ok(build_overview_domain_detail(&stats, &domain))
    })
    .await
    .map_err(|e| AppError::Unknown(format!("域名详情任务异常: {e}")))?
}

/// 获取指定日期的统计 —— 内部复用版
pub(crate) fn get_daily_stats_inner(
    date: &str,
    state: &Arc<Mutex<AppState>>,
) -> Result<DailyStats, AppError> {
    let ctx = OverviewQueryContext::snapshot(state)?;
    ctx.load_single_day(date)
}

/// 获取指定日期的统计
#[tauri::command]
pub async fn get_daily_stats(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<DailyStats, AppError> {
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || get_daily_stats_inner(&date, &state_arc))
        .await
        .map_err(|e| AppError::Unknown(format!("日统计任务异常: {e}")))?
}

fn resolve_overview_date_bounds(
    mode: &str,
    date: Option<&str>,
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Result<(String, String), AppError> {
    match mode.trim().to_lowercase().as_str() {
        "week" => {
            let anchor = resolve_overview_anchor_date(date)?;
            Ok(overview_week_bounds_for_date(anchor))
        }
        "today" => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            Ok((today.clone(), today))
        }
        _ => {
            let (start, end) = resolve_overview_date_span(date, date_from, date_to)?;
            Ok((
                start.format("%Y-%m-%d").to_string(),
                end.format("%Y-%m-%d").to_string(),
            ))
        }
    }
}

pub(crate) fn get_hourly_app_breakdown_inner(
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    mode: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<HourlyAppBucket>, AppError> {
    let ctx = OverviewQueryContext::snapshot(state)?;
    let normalized_mode = mode.unwrap_or_else(|| "date".to_string());
    let (date_from, date_to) = resolve_overview_date_bounds(
        &normalized_mode,
        date.as_deref(),
        date_from.as_deref(),
        date_to.as_deref(),
    )?;

    ctx.database.get_hourly_app_breakdown_range_filtered(
        &date_from,
        &date_to,
        &ctx.ignored_apps,
        &ctx.excluded_domains,
    )
}

/// 获取专注度与上下文切换指标 —— 内部复用版（供 Tauri 命令与 localhost API 共用）
pub(crate) fn get_focus_metrics_inner(
    mode: Option<String>,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: &Arc<Mutex<AppState>>,
) -> Result<work_review_core::work_intelligence::FocusMetrics, AppError> {
    let ctx = OverviewQueryContext::snapshot(state)?;
    let normalized_mode = mode.unwrap_or_else(|| "today".to_string());
    let (start_date, end_date) = resolve_overview_date_bounds(
        &normalized_mode,
        date.as_deref(),
        date_from.as_deref(),
        date_to.as_deref(),
    )?;
    let raw_activities =
        ctx.database
            .get_activities_in_range(Some(&start_date), Some(&end_date), 10_000)?;
    let activities = super::shared::filter_activities_by_privacy(
        raw_activities,
        &ctx.ignored_apps,
        &ctx.excluded_domains,
    );
    Ok(work_review_core::work_intelligence::compute_focus_metrics(
        &activities,
    ))
}

/// 获取专注度与上下文切换/打断指标
#[tauri::command]
pub async fn get_focus_metrics(
    mode: Option<String>,
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<work_review_core::work_intelligence::FocusMetrics, AppError> {
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        get_focus_metrics_inner(mode, date, date_from, date_to, &state_arc)
    })
    .await
    .map_err(|e| AppError::Unknown(format!("专注度指标任务异常: {e}")))?
}

/// 获取每小时×应用的时长分布
#[tauri::command]
pub async fn get_hourly_app_breakdown(
    date: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    mode: Option<String>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<HourlyAppBucket>, AppError> {
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        get_hourly_app_breakdown_inner(date, date_from, date_to, mode, &state_arc)
    })
    .await
    .map_err(|e| AppError::Unknown(format!("小时明细任务异常: {e}")))?
}

/// 获取历史应用列表 —— 内部复用版
pub(crate) fn get_recent_apps_inner(state: &Arc<Mutex<AppState>>) -> Result<Vec<String>, AppError> {
    let database = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.database.clone()
    };
    database.get_recent_apps(50)
}

/// 历史应用详情 —— 内部复用版，供 localhost API 返回（过滤忽略应用并脱敏敏感应用路径/截图 URL）
pub(crate) fn get_recent_app_usage_inner(
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<AppUsage>, AppError> {
    let (database, ignored_apps, privacy_config) = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, _) = collect_privacy_filters(&state);
        (
            state.database.clone(),
            ignored_apps,
            state.config.privacy.clone(),
        )
    };
    let items = database.get_recent_app_usage(50)?;
    Ok(items
        .into_iter()
        .filter(|item| {
            !work_review_core::privacy::matches_ignored_app(&item.app_name, &ignored_apps)
        })
        .map(|mut item| {
            if privacy_config.get_app_privacy_level(&item.app_name)
                != work_review_core::config::PrivacyLevel::Full
            {
                item.executable_path = None;
                item.screenshot_url = None;
            }
            item
        })
        .collect())
}

/// 获取历史应用列表（从数据库）
#[tauri::command]
pub async fn get_recent_apps(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<String>, AppError> {
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || get_recent_apps_inner(&state_arc))
        .await
        .map_err(|e| AppError::Unknown(format!("历史应用查询任务异常: {e}")))?
}

/// 获取当前运行的应用列表
#[tauri::command]
pub async fn get_running_apps() -> Result<Vec<String>, AppError> {
    tokio::task::spawn_blocking(get_running_apps_impl)
        .await
        .map_err(|e| AppError::Unknown(format!("运行中应用查询任务异常: {e}")))?
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
            .map(work_review_core::categorize::normalize_display_app_name)
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
                    let display_name =
                        work_review_core::categorize::normalize_display_app_name(&name);
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
pub(crate) fn get_storage_stats_inner(
    state: &Arc<Mutex<AppState>>,
) -> Result<serde_json::Value, AppError> {
    let (data_dir, storage_config) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (s.data_dir.clone(), s.config.storage.clone())
    };
    let manager = crate::storage::StorageManager::new(&data_dir, storage_config);
    let stats = manager
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
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || get_storage_stats_inner(&state_arc))
        .await
        .map_err(|e| AppError::Unknown(format!("存储统计任务异常: {e}")))?
}

/// 获取指定日期的小时摘要 —— 内部复用版。
/// 只重算「缺失或可能不完整」的小时：已有摘要且生成于该小时结束之后的直接复用
/// （此前每次查看都对 0..24 全量重算并 REPLACE 写库,进一次时间线 = 24 次聚合 + 24 次写）。
/// 今天的当前小时改为内存计算、不落库,避免半截摘要进库。
/// 删除/合并记录的路径都会失效当日摘要缓存（invalidate_daily_cache）,
/// 因此「已有且完整即跳过」不会留下脏数据。
pub(crate) fn get_hourly_summaries_inner(
    date: &str,
    state: &Arc<Mutex<AppState>>,
) -> Result<Vec<serde_json::Value>, AppError> {
    use chrono::Timelike;

    let app_state = state.clone();
    let database = {
        let s = app_state
            .lock()
            .map_err(|e| AppError::Unknown(e.to_string()))?;
        s.database.clone()
    };

    let saved_created_at: HashMap<i32, i64> = database
        .get_hourly_summaries(date)?
        .iter()
        .map(|summary| (summary.hour, summary.created_at))
        .collect();

    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let current_hour = now.hour() as i32;
    // 只生成"已结束"的小时:今天补到当前小时之前,未来日期无可生成
    let end_hour = if date == today {
        current_hour
    } else if date > today.as_str() {
        0
    } else {
        24
    };

    let stale_hours: Vec<i32> = (0..end_hour)
        .filter(|hour| {
            !matches!(
                (saved_created_at.get(hour), local_hour_end_timestamp(date, *hour)),
                (Some(&created_at), Some(end_ts)) if created_at >= end_ts
            )
        })
        .collect();

    if stale_hours.len() > 1 {
        // 需要补齐多个小时时用 1 次整日查询替代多次独立 SQL
        if let Ok(mut all_buckets) = database.get_all_hourly_activities_for_date(date) {
            for hour in stale_hours {
                if let Some(slot) = all_buckets.get_mut(hour as usize) {
                    let activities = std::mem::take(slot);
                    crate::save_hourly_summary_from_activities(
                        &app_state, &database, date, hour, activities,
                    );
                }
            }
        }
    } else if let Some(&hour) = stale_hours.first() {
        crate::generate_and_save_summary(&app_state, date, hour);
    }

    let mut summaries = database.get_hourly_summaries(date)?;

    // 今天的当前小时：内存计算,不写库(旧版本可能残留过半截摘要,一并以内存结果覆盖展示)
    if date == today {
        if let Some(current) = crate::build_hourly_summary(&app_state, date, current_hour) {
            summaries.retain(|summary| summary.hour != current_hour);
            summaries.push(current);
            summaries.sort_by_key(|summary| summary.hour);
        }
    }

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
    let state_arc = state.inner().clone();
    tokio::task::spawn_blocking(move || get_hourly_summaries_inner(&date, &state_arc))
        .await
        .map_err(|e| AppError::Unknown(format!("小时摘要查询任务异常: {e}")))?
}

#[derive(Debug, Default)]
struct OldActivitiesCleanupResult {
    deleted_activities: usize,
    deleted_screenshots: usize,
    failed_screenshot_paths: Vec<String>,
}

fn relative_screenshot_path(data_dir: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(data_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn record_screenshot_cleanup_failure(
    data_dir: &std::path::Path,
    path: &std::path::Path,
    error: &std::io::Error,
    result: &mut OldActivitiesCleanupResult,
) {
    log::warn!("删除旧截图失败 {}: {error}", path.display());
    result
        .failed_screenshot_paths
        .push(relative_screenshot_path(data_dir, path));
}

/// 递归删除截图目录，并只统计实际成功删除的文件。
///
/// 文件删除失败时继续处理其他条目，失败路径会返回给调用方用于提示和后续重试。
fn remove_screenshot_directory(
    data_dir: &std::path::Path,
    directory: &std::path::Path,
    result: &mut OldActivitiesCleanupResult,
) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            record_screenshot_cleanup_failure(data_dir, directory, &error, result);
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                record_screenshot_cleanup_failure(data_dir, directory, &error, result);
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                record_screenshot_cleanup_failure(data_dir, &path, &error, result);
                continue;
            }
        };

        if file_type.is_dir() {
            remove_screenshot_directory(data_dir, &path, result);
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => result.deleted_screenshots += 1,
                Err(error) => record_screenshot_cleanup_failure(data_dir, &path, &error, result),
            }
        }
    }

    if let Err(error) = std::fs::remove_dir(directory) {
        record_screenshot_cleanup_failure(data_dir, directory, &error, result);
    }
}

/// 数据库事务必须先成功，之后才允许删除对应日期的截图目录。
fn clear_old_activities_storage<F>(
    data_dir: &std::path::Path,
    delete_before: chrono::NaiveDate,
    delete_activities: F,
) -> Result<OldActivitiesCleanupResult, AppError>
where
    F: FnOnce() -> Result<usize, AppError>,
{
    let deleted_activities = delete_activities()?;
    let mut result = OldActivitiesCleanupResult {
        deleted_activities,
        ..Default::default()
    };
    let screenshots_dir = data_dir.join("screenshots");
    let entries = match std::fs::read_dir(&screenshots_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(error) => {
            record_screenshot_cleanup_failure(data_dir, &screenshots_dir, &error, &mut result);
            return Ok(result);
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                record_screenshot_cleanup_failure(data_dir, &screenshots_dir, &error, &mut result);
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                record_screenshot_cleanup_failure(data_dir, &path, &error, &mut result);
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }

        let directory_name = entry.file_name();
        let Some(directory_name) = directory_name.to_str() else {
            continue;
        };
        let Ok(directory_date) = chrono::NaiveDate::parse_from_str(directory_name, "%Y-%m-%d")
        else {
            continue;
        };
        if directory_date < delete_before {
            remove_screenshot_directory(data_dir, &path, &mut result);
        }
    }

    result.failed_screenshot_paths.sort();
    result.failed_screenshot_paths.dedup();
    Ok(result)
}

/// 清理昨天之前的所有活动记录
#[tauri::command]
pub async fn clear_old_activities(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let data_dir = {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.data_dir.clone()
    };

    // 获取要保留的日期（今天和昨天）
    let today_date = chrono::Local::now().date_naive();
    let delete_before = today_date - chrono::Duration::days(1);
    let today = today_date.format("%Y-%m-%d").to_string();
    let yesterday = delete_before.format("%Y-%m-%d").to_string();
    let cleanup = clear_old_activities_storage(&data_dir, delete_before, || {
        let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        state.database.delete_activities_before_date(&yesterday)
    })?;
    let failed_count = cleanup.failed_screenshot_paths.len();
    let message = if failed_count == 0 {
        format!(
            "已清理 {} 条旧活动记录和 {} 张旧截图，保留今天和昨天的数据",
            cleanup.deleted_activities, cleanup.deleted_screenshots
        )
    } else {
        format!(
            "数据库已清理 {} 条旧活动记录；成功删除 {} 张旧截图，{} 个路径删除失败，可稍后重试",
            cleanup.deleted_activities, cleanup.deleted_screenshots, failed_count
        )
    };

    Ok(serde_json::json!({
        "deleted_activities": cleanup.deleted_activities,
        "deleted_screenshots": cleanup.deleted_screenshots,
        "failed_screenshot_paths": cleanup.failed_screenshot_paths,
        "kept_dates": [today, yesterday],
        "message": message
    }))
}

/// 按相对 data_dir 的路径删除截图文件（连带同目录 `.thumb.jpg` 缓存），返回成功删除的原图文件数
fn remove_screenshot_files(data_dir: &std::path::Path, paths: Vec<String>) -> usize {
    let mut removed = 0usize;
    for p in paths {
        if p.is_empty() {
            continue;
        }
        let path = data_dir.join(&p);
        if path.exists() {
            match std::fs::remove_file(&path) {
                Ok(_) => removed += 1,
                Err(e) => log::warn!("删除截图文件失败 {}: {e}", path.display()),
            }
        }
        let thumb_path = crate::screenshot::ScreenshotService::thumbnail_cache_path(&path);
        if thumb_path != path && thumb_path.exists() {
            let _ = std::fs::remove_file(&thumb_path);
        }
    }
    removed
}

/// 删除单条活动记录（连带删除对应截图文件，OCR 文本随行删除）
#[tauri::command]
pub async fn delete_activity(
    id: i64,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let (data_dir, paths, deleted) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (deleted, paths) = s.database.delete_activity_by_id(id)?;
        (s.data_dir.clone(), paths, deleted)
    };
    let removed = remove_screenshot_files(&data_dir, paths);
    log::info!("删除单条活动 id={id}: {deleted} 条记录, {removed} 张截图");
    Ok(serde_json::json!({ "deleted": deleted, "removed_screenshots": removed }))
}

/// 删除指定日期（本地时区全天）的全部活动记录（连带截图）
#[tauri::command]
pub async fn delete_activities_by_date(
    date: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let (data_dir, paths, deleted) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (deleted, paths) = s.database.delete_activities_by_date(&date)?;
        (s.data_dir.clone(), paths, deleted)
    };
    let removed = remove_screenshot_files(&data_dir, paths);
    log::info!("按日期删除 {date}: {deleted} 条记录, {removed} 张截图");
    Ok(serde_json::json!({ "deleted": deleted, "removed_screenshots": removed }))
}

/// 删除指定时间段 [start_ts, end_ts)（Unix 秒）内的全部活动记录（连带截图）
#[tauri::command]
pub async fn delete_activities_by_range(
    start_ts: i64,
    end_ts: i64,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let (data_dir, paths, deleted) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (deleted, paths) = s.database.delete_activities_by_range(start_ts, end_ts)?;
        (s.data_dir.clone(), paths, deleted)
    };
    let removed = remove_screenshot_files(&data_dir, paths);
    log::info!("按时间段删除 [{start_ts}, {end_ts}): {deleted} 条记录, {removed} 张截图");
    Ok(serde_json::json!({ "deleted": deleted, "removed_screenshots": removed }))
}

/// 删除指定应用的活动记录；start_ts/end_ts 同时给定时只删该时间段内的，否则删该应用全部（连带截图）
#[tauri::command]
pub async fn delete_activities_by_app(
    app_name: String,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let date_range = match (start_ts, end_ts) {
        (Some(s), Some(e)) => Some((s, e)),
        _ => None,
    };
    let (data_dir, paths, deleted) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (deleted, paths) = s.database.delete_activities_by_app(&app_name, date_range)?;
        (s.data_dir.clone(), paths, deleted)
    };
    let removed = remove_screenshot_files(&data_dir, paths);
    log::info!("按应用删除 {app_name}: {deleted} 条记录, {removed} 张截图");
    Ok(serde_json::json!({ "deleted": deleted, "removed_screenshots": removed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::cell::Cell;
    use work_review_core::database::HourlyActivityBucket;

    fn clear_old_activities_test_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "work-review-clear-old-activities-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn 概览统计应读取侧应用网站语义规则覆盖历史投票() {
        let rules = vec![WebsiteSemanticRule {
            domain: "github.com".to_string(),
            semantic_category: "休息娱乐".to_string(),
        }];
        let stats = DailyStats {
            domain_usage: vec![
                DomainUsage {
                    domain: "github.com".to_string(),
                    duration: 120,
                    semantic_category: Some("工作开发".to_string()),
                    urls: Vec::new(),
                },
                DomainUsage {
                    domain: "docs.example.com".to_string(),
                    duration: 60,
                    semantic_category: Some("资料阅读".to_string()),
                    urls: Vec::new(),
                },
                DomainUsage {
                    domain: "unlabeled.example.com".to_string(),
                    duration: 30,
                    semantic_category: None,
                    urls: Vec::new(),
                },
            ],
            browser_usage: vec![BrowserUsage {
                browser_name: "Chrome".to_string(),
                duration: 120,
                executable_path: None,
                domains: vec![DomainUsage {
                    domain: "github.com".to_string(),
                    duration: 120,
                    semantic_category: None,
                    urls: Vec::new(),
                }],
            }],
            ..DailyStats::default()
        };

        let applied = apply_website_semantic_rules_to_stats(stats, &rules);
        let category_of = |domain: &str, list: &[DomainUsage]| {
            list.iter()
                .find(|item| item.domain == domain)
                .expect("测试数据应包含目标域名")
                .semantic_category
                .clone()
        };
        assert_eq!(
            category_of("github.com", &applied.domain_usage).as_deref(),
            Some("休息娱乐")
        );
        assert_eq!(
            category_of("docs.example.com", &applied.domain_usage).as_deref(),
            Some("资料阅读"),
            "未命中规则的域名不应被改动"
        );
        assert_eq!(
            category_of("unlabeled.example.com", &applied.domain_usage),
            None,
            "无规则的域名不应被凭空赋予分类"
        );
        assert_eq!(
            category_of("github.com", &applied.browser_usage[0].domains).as_deref(),
            Some("休息娱乐"),
            "浏览器分组内的域名也应应用规则"
        );

        let untouched = apply_website_semantic_rules_to_stats(applied, &[]);
        assert_eq!(
            category_of("github.com", &untouched.domain_usage).as_deref(),
            Some("休息娱乐"),
            "空规则列表应原样返回"
        );
    }

    #[test]
    fn 数据库清理失败时不应预先删除旧截图() {
        let data_dir = clear_old_activities_test_dir("database-failure");
        let old_dir = data_dir.join("screenshots/2026-08-01");
        std::fs::create_dir_all(&old_dir).expect("创建旧截图目录失败");
        let screenshot = old_dir.join("old.png");
        std::fs::write(&screenshot, b"old screenshot").expect("写入旧截图失败");
        let database_called = Cell::new(false);

        let result = clear_old_activities_storage(
            &data_dir,
            NaiveDate::from_ymd_opt(2026, 8, 4).expect("有效截止日期"),
            || {
                database_called.set(true);
                Err(AppError::Unknown("模拟数据库事务失败".to_string()))
            },
        );

        assert!(result.is_err(), "数据库错误应向调用方返回");
        assert!(database_called.get(), "应尝试执行数据库事务");
        assert!(
            screenshot.exists(),
            "数据库事务失败时，旧截图必须保持原样以免记录引用断裂"
        );
        std::fs::remove_dir_all(&data_dir).expect("清理测试目录失败");
    }

    #[test]
    fn 数据库成功后只删除截止日期之前的截图并返回实际计数() {
        let data_dir = clear_old_activities_test_dir("success");
        let screenshots_dir = data_dir.join("screenshots");
        let old_dir = screenshots_dir.join("2026-08-01");
        let nested_dir = old_dir.join("nested");
        let yesterday_dir = screenshots_dir.join("2026-08-04");
        let today_dir = screenshots_dir.join("2026-08-05");
        let future_dir = screenshots_dir.join("2026-08-06");
        let unknown_dir = screenshots_dir.join("manual-backup");
        for directory in [
            &nested_dir,
            &yesterday_dir,
            &today_dir,
            &future_dir,
            &unknown_dir,
        ] {
            std::fs::create_dir_all(directory).expect("创建截图目录失败");
        }
        let old_screenshot = old_dir.join("one.png");
        std::fs::write(&old_screenshot, b"one").expect("写入旧截图失败");
        std::fs::write(nested_dir.join("two.png"), b"two").expect("写入嵌套旧截图失败");
        std::fs::write(yesterday_dir.join("keep.png"), b"keep").expect("写入昨日截图失败");
        std::fs::write(today_dir.join("keep.png"), b"keep").expect("写入今日截图失败");
        std::fs::write(future_dir.join("keep.png"), b"keep").expect("写入未来截图失败");
        std::fs::write(unknown_dir.join("keep.png"), b"keep").expect("写入未知目录截图失败");

        let result = clear_old_activities_storage(
            &data_dir,
            NaiveDate::from_ymd_opt(2026, 8, 4).expect("有效截止日期"),
            || {
                assert!(
                    old_screenshot.exists(),
                    "执行数据库事务时，旧截图尚未被预先删除"
                );
                Ok(7)
            },
        )
        .expect("清理旧活动应成功");

        assert_eq!(result.deleted_activities, 7);
        assert_eq!(result.deleted_screenshots, 2);
        assert!(result.failed_screenshot_paths.is_empty());
        assert!(!old_dir.exists(), "旧截图目录应在数据库成功后删除");
        assert!(yesterday_dir.exists(), "截止日期当天的截图应保留");
        assert!(today_dir.exists(), "今天的截图应保留");
        assert!(future_dir.exists(), "未来日期目录不属于数据库清理范围");
        assert!(unknown_dir.exists(), "无法识别日期的目录不应被误删");
        std::fs::remove_dir_all(&data_dir).expect("清理测试目录失败");
    }

    #[cfg(unix)]
    #[test]
    fn 截图删除失败时不应虚增计数且应返回失败路径() {
        use std::os::unix::fs::PermissionsExt;

        let data_dir = clear_old_activities_test_dir("filesystem-failure");
        let old_dir = data_dir.join("screenshots/2026-08-01");
        std::fs::create_dir_all(&old_dir).expect("创建旧截图目录失败");
        let screenshot = old_dir.join("locked.png");
        std::fs::write(&screenshot, b"locked").expect("写入旧截图失败");
        std::fs::set_permissions(&old_dir, std::fs::Permissions::from_mode(0o500))
            .expect("设置只读目录权限失败");

        let result = clear_old_activities_storage(
            &data_dir,
            NaiveDate::from_ymd_opt(2026, 8, 4).expect("有效截止日期"),
            || Ok(1),
        )
        .expect("数据库成功后，文件清理失败应通过结果显式报告");

        assert_eq!(result.deleted_activities, 1);
        assert_eq!(
            result.deleted_screenshots, 0,
            "删除失败的截图不应计入成功数量"
        );
        assert!(screenshot.exists(), "删除失败的截图应仍然存在");
        assert!(
            result
                .failed_screenshot_paths
                .iter()
                .any(|path| path.ends_with("screenshots/2026-08-01/locked.png")),
            "结果中应包含可重试的相对失败路径"
        );

        std::fs::set_permissions(&old_dir, std::fs::Permissions::from_mode(0o700))
            .expect("恢复测试目录权限失败");
        std::fs::remove_dir_all(&data_dir).expect("清理测试目录失败");
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
            domain_total_count: 1,
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
            overtime_duration: 0,
            hourly_activity_distribution: vec![hourly(9, 60), hourly(10, 60)],
            project_usage: vec![],
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
            domain_total_count: 2,
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
            overtime_duration: 0,
            hourly_activity_distribution: vec![hourly(9, 30), hourly(10, 90), hourly(11, 60)],
            project_usage: vec![],
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
        assert_eq!(merged.domain_total_count, 2);
    }

    fn overview_domain(domain: &str, duration: i64, urls: Vec<(&str, i64)>) -> DomainUsage {
        DomainUsage {
            domain: domain.to_string(),
            duration,
            semantic_category: Some("资料阅读".to_string()),
            urls: urls
                .into_iter()
                .map(|(url, duration)| UrlDetail {
                    url: url.to_string(),
                    duration,
                })
                .collect(),
        }
    }

    #[test]
    fn 首页概览应只投影前六域名并保留真实总数() {
        let domains = (0..8)
            .map(|index| {
                overview_domain(&format!("site-{index}.example"), (8 - index) * 10, vec![])
            })
            .collect::<Vec<_>>();
        let stats = DailyStats {
            domain_usage: domains.clone(),
            browser_usage: vec![BrowserUsage {
                browser_name: "Google Chrome".to_string(),
                duration: 360,
                executable_path: None,
                domains,
            }],
            ..Default::default()
        };

        let full_collection = build_overview_domain_collection(&stats);
        let projected = project_overview_homepage(stats);

        assert_eq!(full_collection.total_count, 8);
        assert_eq!(full_collection.domains.len(), 8);
        assert_eq!(projected.domain_total_count, 8);
        assert_eq!(projected.domain_usage.len(), 6);
        assert_eq!(projected.browser_usage[0].duration, 360);
        assert_eq!(projected.browser_usage[0].domains.len(), 6);
        assert!(projected.browser_usage[0]
            .domains
            .iter()
            .all(|domain| projected
                .domain_usage
                .iter()
                .any(|visible| visible.domain == domain.domain)));
    }

    #[test]
    fn 域名集合应返回完整摘要且不携带网址明细() {
        let stats = overview_sources_fixture();

        let collection = build_overview_domain_collection(&stats);

        assert_eq!(collection.total_count, 1);
        assert_eq!(collection.domains.len(), 1);
        let summary = &collection.domains[0];
        assert_eq!(summary.domain, "docs.example.com");
        assert_eq!(summary.duration, 100);
        assert_eq!(summary.page_count, 2);
        assert_eq!(summary.browser_sources.len(), 3);
        assert_eq!(summary.browser_sources[0].browser_name, "Google Chrome");
        assert_eq!(summary.browser_sources[1].browser_name, "Safari");
        assert_eq!(summary.browser_sources[2].browser_name, "other");
        assert!((summary.browser_sources[2].percentage - 20.0).abs() < f64::EPSILON);
        let serialized = serde_json::to_value(&collection).expect("域名集合应可序列化");
        assert!(serialized["domains"][0].get("urls").is_none());
        assert!(serialized["domains"][0]["browser_sources"][0]
            .get("urls")
            .is_none());
    }

    #[test]
    fn 域名详情应只返回目标域名并补齐无法归属的来源() {
        let stats = overview_sources_fixture();

        let detail =
            build_overview_domain_detail(&stats, "docs.example.com").expect("应返回目标域名详情");

        assert_eq!(detail.domain, "docs.example.com");
        assert_eq!(detail.urls.len(), 2);
        assert_eq!(detail.browser_sources.len(), 3);
        let other = detail
            .browser_sources
            .iter()
            .find(|source| source.browser_name == "other")
            .expect("应补齐 other 来源");
        assert_eq!(other.duration, 20);
        assert!((other.percentage - 20.0).abs() < f64::EPSILON);
        assert_eq!(other.urls.len(), 2);
        assert_eq!(other.urls.iter().map(|url| url.duration).sum::<i64>(), 20);
        assert!(build_overview_domain_detail(&stats, "missing.example.com").is_none());
    }

    fn overview_sources_fixture() -> DailyStats {
        DailyStats {
            domain_usage: vec![overview_domain(
                "docs.example.com",
                100,
                vec![
                    ("https://docs.example.com/a", 60),
                    ("https://docs.example.com/b", 40),
                ],
            )],
            browser_usage: vec![
                BrowserUsage {
                    browser_name: "Google Chrome".to_string(),
                    duration: 50,
                    executable_path: None,
                    domains: vec![overview_domain(
                        "docs.example.com",
                        50,
                        vec![("https://docs.example.com/a", 50)],
                    )],
                },
                BrowserUsage {
                    browser_name: "Safari".to_string(),
                    duration: 30,
                    executable_path: None,
                    domains: vec![overview_domain(
                        "docs.example.com",
                        30,
                        vec![("https://docs.example.com/b", 30)],
                    )],
                },
            ],
            ..Default::default()
        }
    }
}

/// 获取区间内逐日投入合计（概览「按天投入」卡）。
/// 区间按自然日展开，最多 31 天，超出部分从起点截断；
/// 使用一次范围查询在 Rust 中按日聚合（与概览统计同口径，含工作时段与隐私过滤），
/// 返回 [{ date, total_duration, work_time_duration }]。
#[tauri::command]
pub async fn get_range_daily_totals(
    date_from: String,
    date_to: String,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<serde_json::Value>, AppError> {
    let ctx = OverviewQueryContext::snapshot(state.inner())?;
    tokio::task::spawn_blocking(move || {
        let (start, end) =
            resolve_overview_date_span(None, Some(date_from.as_str()), Some(date_to.as_str()))?;
        let max_end = start + chrono::Duration::days(30);
        let bounded_end = end.min(max_end);
        let per_day = ctx.load_date_range(start, bounded_end)?;
        Ok(per_day
            .into_iter()
            .map(|(date, stats)| {
                serde_json::json!({
                    "date": date,
                    "total_duration": stats.total_duration,
                    "work_time_duration": stats.work_time_duration,
                })
            })
            .collect())
    })
    .await
    .map_err(|e| AppError::Unknown(format!("按天投入查询任务异常: {e}")))?
}
