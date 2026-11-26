use crate::categorize::{
    browser_page_domain_label, normalize_display_app_name, resolve_browser_page_hint,
};
use crate::config::{AppConfig, PrivacyConfig};
use crate::database::Activity;
use crate::error::{AppError, Result};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

pub const UNASSIGNED_PROJECT_KEY: &str = "__unassigned__";
pub const UNASSIGNED_PROJECT_NAME: &str = "未归属";
pub const UNASSIGNED_PROJECT_COLOR: &str = "#94A3B8";
pub const MAX_PROJECT_COUNT: usize = 128;
pub const MAX_RULES_PER_PROJECT: usize = 64;
const PROJECT_REGEX_SIZE_LIMIT: usize = 256 * 1024;
const PROJECT_REGEX_DFA_SIZE_LIMIT: usize = 256 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRuleField {
    App,
    Title,
    Domain,
    UrlPath,
    ExecutablePath,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRuleOp {
    Equals,
    Contains,
    Regex,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProjectRule {
    pub field: ProjectRuleField,
    pub op: ProjectRuleOp,
    pub value: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProjectDefinition {
    pub key: String,
    pub name: String,
    #[serde(default = "default_project_color")]
    pub color: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub rules: Vec<ProjectRule>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProjectUsage {
    pub key: String,
    pub name: String,
    pub color: String,
    pub duration: i64,
    #[serde(default)]
    pub activity_count: i64,
}

fn default_project_color() -> String {
    "#3B82F6".to_string()
}

pub fn compile_project_regex(pattern: &str) -> Result<Regex> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config("项目规则正则不能为空".to_string()));
    }
    if trimmed.chars().count() > 256 {
        return Err(AppError::Config(
            "项目规则正则过长（最多 256 个字符）".to_string(),
        ));
    }
    RegexBuilder::new(trimmed)
        .case_insensitive(true)
        .size_limit(PROJECT_REGEX_SIZE_LIMIT)
        .dfa_size_limit(PROJECT_REGEX_DFA_SIZE_LIMIT)
        .build()
        .map_err(|err| AppError::Config(format!("项目规则正则无效: {err}")))
}

pub fn validate_project_rule(rule: &ProjectRule) -> Result<()> {
    let trimmed = rule.value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config("项目规则匹配值不能为空".to_string()));
    }
    if trimmed.chars().count() > 512 {
        return Err(AppError::Config(
            "项目规则匹配值过长（最多 512 个字符）".to_string(),
        ));
    }
    if rule.op == ProjectRuleOp::Regex {
        compile_project_regex(trimmed)?;
    }
    Ok(())
}

pub fn slugify_project_key(raw: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if matches!(ch, '-' | '_' | ' ' | '/' | '.') && !slug.is_empty() && !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() || slug == UNASSIGNED_PROJECT_KEY {
        format!(
            "project-{}",
            uuid::Uuid::new_v4().simple().to_string()[..8].to_lowercase()
        )
    } else {
        slug.chars().take(48).collect()
    }
}

pub fn normalize_project_color(color: &str) -> String {
    let trimmed = color.trim();
    let is_valid_hex = (trimmed.len() == 4 || trimmed.len() == 7)
        && trimmed.starts_with('#')
        && trimmed[1..].chars().all(|c| c.is_ascii_hexdigit());
    if is_valid_hex {
        trimmed.to_uppercase()
    } else {
        default_project_color()
    }
}

fn sanitize_raw_project_key(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(48)
        .collect::<String>()
        .to_lowercase()
}

pub fn validate_and_normalize_project(mut project: ProjectDefinition) -> Result<ProjectDefinition> {
    project.name = project.name.trim().to_string();
    if project.name.is_empty() {
        return Err(AppError::Config("项目名称不能为空".to_string()));
    }
    if project.name.chars().count() > 64 {
        return Err(AppError::Config(
            "项目名称过长（最多 64 个字符）".to_string(),
        ));
    }
    let raw_key = project.key.trim();
    project.key = if raw_key.is_empty() {
        slugify_project_key(&project.name)
    } else {
        let cleaned = sanitize_raw_project_key(raw_key);
        if cleaned == UNASSIGNED_PROJECT_KEY {
            return Err(AppError::Config("项目标识不能使用保留名称".to_string()));
        }
        if cleaned.is_empty() {
            slugify_project_key(&project.name)
        } else {
            cleaned
        }
    };
    project.color = normalize_project_color(&project.color);
    if project.rules.len() > MAX_RULES_PER_PROJECT {
        return Err(AppError::Config(format!(
            "单个项目最多支持 {MAX_RULES_PER_PROJECT} 条规则"
        )));
    }
    for rule in &mut project.rules {
        rule.value = rule.value.trim().to_string();
        validate_project_rule(rule)?;
    }
    Ok(project)
}

pub fn normalize_project_definitions(projects: &mut Vec<ProjectDefinition>) {
    let mut seen_keys = HashSet::new();
    let mut cleaned = Vec::with_capacity(projects.len().min(MAX_PROJECT_COUNT));
    for project in projects.drain(..) {
        if cleaned.len() >= MAX_PROJECT_COUNT {
            break;
        }
        let name = project.name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let sanitized = sanitize_raw_project_key(&project.key);
        let mut key = if sanitized.is_empty() || sanitized == UNASSIGNED_PROJECT_KEY {
            slugify_project_key(&name)
        } else {
            sanitized
        };
        let mut attempt = cleaned.len() + 1;
        while key.is_empty() || key == UNASSIGNED_PROJECT_KEY || !seen_keys.insert(key.clone()) {
            let candidate = slugify_project_key(&format!("{name}-{attempt}"));
            key = if candidate == key || candidate == UNASSIGNED_PROJECT_KEY {
                format!(
                    "project-{}",
                    uuid::Uuid::new_v4().simple().to_string()[..8].to_lowercase()
                )
            } else {
                candidate
            };
            attempt += 1;
        }
        let rules: Vec<ProjectRule> = project
            .rules
            .into_iter()
            .take(MAX_RULES_PER_PROJECT)
            .filter_map(|mut rule| {
                rule.value = rule.value.trim().to_string();
                validate_project_rule(&rule).ok().map(|()| rule)
            })
            .collect();
        cleaned.push(ProjectDefinition {
            key,
            name: name.chars().take(64).collect(),
            color: normalize_project_color(&project.color),
            archived: project.archived,
            rules,
        });
    }
    *projects = cleaned;
}

enum CompiledOp {
    Equals(String),
    Contains(String),
    Regex(Regex),
}

struct CompiledRule {
    field: ProjectRuleField,
    op: CompiledOp,
}

struct CompiledProject {
    definition: ProjectDefinition,
    rules: Vec<CompiledRule>,
}

/// 查询时项目归集解析器：预编译所有活跃项目的匹配规则。
#[derive(Default)]
pub struct ProjectResolver {
    projects: Vec<CompiledProject>,
}

impl ProjectResolver {
    pub fn from_config(config: &AppConfig) -> Self {
        Self::from_projects(&config.projects)
    }

    pub fn from_projects(projects: &[ProjectDefinition]) -> Self {
        let mut compiled = Vec::new();
        for project in projects {
            if project.archived || project.rules.is_empty() {
                continue;
            }
            let mut rules = Vec::with_capacity(project.rules.len());
            for rule in &project.rules {
                let value = rule.value.trim();
                if value.is_empty() {
                    continue;
                }
                let op = match rule.op {
                    ProjectRuleOp::Equals => CompiledOp::Equals(value.to_lowercase()),
                    ProjectRuleOp::Contains => CompiledOp::Contains(value.to_lowercase()),
                    ProjectRuleOp::Regex => match compile_project_regex(value) {
                        Ok(re) => CompiledOp::Regex(re),
                        Err(err) => {
                            log::warn!("跳过无效的项目正则规则 {}: {err}", project.key);
                            continue;
                        }
                    },
                };
                rules.push(CompiledRule {
                    field: rule.field,
                    op,
                });
            }
            if !rules.is_empty() {
                compiled.push(CompiledProject {
                    definition: project.clone(),
                    rules,
                });
            }
        }
        Self { projects: compiled }
    }

    pub fn is_empty(&self) -> bool {
        self.projects.is_empty()
    }

    pub fn resolve(&self, activity: &Activity) -> Option<&ProjectDefinition> {
        self.resolve_fields(
            &activity.app_name,
            &activity.window_title,
            activity.browser_url.as_deref(),
            activity.executable_path.as_deref(),
        )
    }

    pub fn resolve_fields(
        &self,
        app_name: &str,
        window_title: &str,
        browser_url: Option<&str>,
        executable_path: Option<&str>,
    ) -> Option<&ProjectDefinition> {
        if self.projects.is_empty() {
            return None;
        }
        let normalized_app = normalize_display_app_name(app_name);
        let raw_app = app_name.trim();
        let domain = extract_activity_domain(browser_url, window_title);
        let url_value = browser_url.map(str::trim).unwrap_or("");
        let exe_raw = executable_path.map(str::trim).unwrap_or("");
        let exe_posix = exe_raw.replace('\\', "/");

        for project in &self.projects {
            if project.rules.iter().any(|rule| {
                rule_matches(
                    rule,
                    raw_app,
                    &normalized_app,
                    window_title,
                    domain.as_deref(),
                    url_value,
                    exe_raw,
                    &exe_posix,
                )
            }) {
                return Some(&project.definition);
            }
        }
        None
    }

    /// 预览单条规则对给定活动列表的命中情况（调用前应已完成隐私过滤）。
    pub fn matches_single_rule(rule: &ProjectRule, activity: &Activity) -> Result<bool> {
        validate_project_rule(rule)?;
        let temp = ProjectDefinition {
            key: "__preview__".to_string(),
            name: "Preview".to_string(),
            color: default_project_color(),
            archived: false,
            rules: vec![rule.clone()],
        };
        let resolver = Self::from_projects(&[temp]);
        Ok(resolver.resolve(activity).is_some())
    }

    /// 对已按隐私规则过滤的活动列表按项目汇总时长。
    /// 若没有配置任何活跃项目，返回空列表。
    pub fn aggregate_activities(
        &self,
        activities: &[Activity],
        include_unassigned: bool,
    ) -> Vec<ProjectUsage> {
        if self.projects.is_empty() {
            return Vec::new();
        }
        let mut buckets: HashMap<String, ProjectUsage> = HashMap::new();
        for activity in activities {
            if activity.duration <= 0 {
                continue;
            }
            if let Some(project) = self.resolve(activity) {
                let entry = buckets
                    .entry(project.key.clone())
                    .or_insert_with(|| ProjectUsage {
                        key: project.key.clone(),
                        name: project.name.clone(),
                        color: project.color.clone(),
                        duration: 0,
                        activity_count: 0,
                    });
                entry.duration += activity.duration;
                entry.activity_count += 1;
            } else if include_unassigned {
                let entry = buckets
                    .entry(UNASSIGNED_PROJECT_KEY.to_string())
                    .or_insert_with(|| ProjectUsage {
                        key: UNASSIGNED_PROJECT_KEY.to_string(),
                        name: UNASSIGNED_PROJECT_NAME.to_string(),
                        color: UNASSIGNED_PROJECT_COLOR.to_string(),
                        duration: 0,
                        activity_count: 0,
                    });
                entry.duration += activity.duration;
                entry.activity_count += 1;
            }
        }

        let mut list: Vec<ProjectUsage> = buckets.into_values().collect();
        sort_project_usage(&mut list);
        list
    }
}

pub fn sort_project_usage(usage: &mut [ProjectUsage]) {
    usage.sort_by_key(|item| {
        (
            item.key == UNASSIGNED_PROJECT_KEY,
            Reverse(item.duration),
            item.name.clone(),
        )
    });
}

fn extract_activity_domain(browser_url: Option<&str>, window_title: &str) -> Option<String> {
    if let Some(url) = browser_url {
        let extracted = PrivacyConfig::extract_domain(url);
        if !extracted.is_empty() {
            return Some(extracted);
        }
    }
    resolve_browser_page_hint(browser_url, window_title, None).map(|hint| {
        let label = browser_page_domain_label(&hint);
        let extracted = PrivacyConfig::extract_domain(&label);
        if extracted.is_empty() {
            label.to_lowercase()
        } else {
            extracted
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn rule_matches(
    rule: &CompiledRule,
    raw_app: &str,
    normalized_app: &str,
    window_title: &str,
    domain: Option<&str>,
    url_value: &str,
    exe_raw: &str,
    exe_posix: &str,
) -> bool {
    match rule.field {
        ProjectRuleField::App => {
            op_matches_text(&rule.op, normalized_app) || op_matches_text(&rule.op, raw_app)
        }
        ProjectRuleField::Title => op_matches_text(&rule.op, window_title),
        ProjectRuleField::Domain => {
            let Some(domain) = domain else {
                return false;
            };
            match &rule.op {
                CompiledOp::Equals(target) => {
                    let target_domain = PrivacyConfig::extract_domain(target);
                    let pattern = if target_domain.is_empty() {
                        target.as_str()
                    } else {
                        target_domain.as_str()
                    };
                    !pattern.is_empty() && PrivacyConfig::domain_matches(domain, pattern)
                }
                CompiledOp::Contains(target) => domain.to_lowercase().contains(target),
                CompiledOp::Regex(re) => re.is_match(domain),
            }
        }
        ProjectRuleField::UrlPath => !url_value.is_empty() && op_matches_text(&rule.op, url_value),
        ProjectRuleField::ExecutablePath => {
            if exe_raw.is_empty() {
                return false;
            }
            match &rule.op {
                CompiledOp::Equals(target) => {
                    let norm_target = target.replace('\\', "/");
                    exe_raw.eq_ignore_ascii_case(target)
                        || exe_posix.eq_ignore_ascii_case(&norm_target)
                }
                CompiledOp::Contains(target) => {
                    let norm_target = target.replace('\\', "/");
                    exe_raw.to_lowercase().contains(target)
                        || exe_posix.to_lowercase().contains(&norm_target)
                }
                CompiledOp::Regex(re) => re.is_match(exe_raw) || re.is_match(exe_posix),
            }
        }
    }
}

fn op_matches_text(op: &CompiledOp, text: &str) -> bool {
    match op {
        CompiledOp::Equals(target) => text.trim().to_lowercase() == *target,
        CompiledOp::Contains(target) => text.to_lowercase().contains(target),
        CompiledOp::Regex(re) => re.is_match(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_activity(
        app: &str,
        title: &str,
        url: Option<&str>,
        exe: Option<&str>,
        duration: i64,
    ) -> Activity {
        Activity {
            id: Some(1),
            timestamp: 1_700_000_000,
            app_name: app.to_string(),
            window_title: title.to_string(),
            screenshot_path: String::new(),
            ocr_text: None,
            category: "development".to_string(),
            duration,
            browser_url: url.map(str::to_string),
            executable_path: exe.map(str::to_string),
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        }
    }

    #[test]
    fn 规则匹配矩阵应覆盖全部字段与操作符() {
        let projects = vec![
            ProjectDefinition {
                key: "wr".to_string(),
                name: "Work Review".to_string(),
                color: "#2563EB".to_string(),
                archived: false,
                rules: vec![
                    ProjectRule {
                        field: ProjectRuleField::Title,
                        op: ProjectRuleOp::Contains,
                        value: "Work_Review".to_string(),
                    },
                    ProjectRule {
                        field: ProjectRuleField::UrlPath,
                        op: ProjectRuleOp::Regex,
                        value: r"github\.com/wm94i/Work-Review".to_string(),
                    },
                ],
            },
            ProjectDefinition {
                key: "docs".to_string(),
                name: "文档沉淀".to_string(),
                color: "#10B981".to_string(),
                archived: false,
                rules: vec![
                    ProjectRule {
                        field: ProjectRuleField::App,
                        op: ProjectRuleOp::Equals,
                        value: "Obsidian".to_string(),
                    },
                    ProjectRule {
                        field: ProjectRuleField::Domain,
                        op: ProjectRuleOp::Equals,
                        value: "notion.so".to_string(),
                    },
                ],
            },
            ProjectDefinition {
                key: "infra".to_string(),
                name: "基础设施".to_string(),
                color: "#F59E0B".to_string(),
                archived: false,
                rules: vec![ProjectRule {
                    field: ProjectRuleField::ExecutablePath,
                    op: ProjectRuleOp::Contains,
                    value: "tools/kubectl".to_string(),
                }],
            },
        ];
        let resolver = ProjectResolver::from_projects(&projects);

        assert_eq!(
            resolver
                .resolve(&sample_activity(
                    "Code",
                    "main.rs - Work_Review",
                    None,
                    None,
                    120
                ))
                .map(|p| p.key.as_str()),
            Some("wr")
        );
        assert_eq!(
            resolver
                .resolve(&sample_activity(
                    "Google Chrome",
                    "PR #42",
                    Some("https://github.com/wm94i/Work-Review/pull/42"),
                    None,
                    120
                ))
                .map(|p| p.key.as_str()),
            Some("wr")
        );
        assert_eq!(
            resolver
                .resolve(&sample_activity("Obsidian", "Daily Note", None, None, 120))
                .map(|p| p.key.as_str()),
            Some("docs")
        );
        assert_eq!(
            resolver
                .resolve(&sample_activity(
                    "Chrome",
                    "Roadmap",
                    Some("https://www.notion.so/team/roadmap"),
                    None,
                    120
                ))
                .map(|p| p.key.as_str()),
            Some("docs")
        );
        assert_eq!(
            resolver
                .resolve(&sample_activity(
                    "kubectl",
                    "cluster",
                    None,
                    Some(r"C:\tools\kubectl\kubectl.exe"),
                    120
                ))
                .map(|p| p.key.as_str()),
            Some("infra")
        );
    }

    #[test]
    fn 项目按定义顺序首个命中生效且忽略已归档项目() {
        let projects = vec![
            ProjectDefinition {
                key: "archived".to_string(),
                name: "已归档".to_string(),
                color: "#64748B".to_string(),
                archived: true,
                rules: vec![ProjectRule {
                    field: ProjectRuleField::App,
                    op: ProjectRuleOp::Contains,
                    value: "Code".to_string(),
                }],
            },
            ProjectDefinition {
                key: "first".to_string(),
                name: "第一优先".to_string(),
                color: "#3B82F6".to_string(),
                archived: false,
                rules: vec![ProjectRule {
                    field: ProjectRuleField::App,
                    op: ProjectRuleOp::Contains,
                    value: "Code".to_string(),
                }],
            },
            ProjectDefinition {
                key: "second".to_string(),
                name: "第二优先".to_string(),
                color: "#EF4444".to_string(),
                archived: false,
                rules: vec![ProjectRule {
                    field: ProjectRuleField::Title,
                    op: ProjectRuleOp::Contains,
                    value: "main.rs".to_string(),
                }],
            },
        ];
        let resolver = ProjectResolver::from_projects(&projects);
        let hit = resolver.resolve(&sample_activity("VS Code", "main.rs", None, None, 60));
        assert_eq!(hit.map(|p| p.key.as_str()), Some("first"));
    }

    #[test]
    fn 非法正则和空字段在保存校验阶段必须拒绝() {
        let bad_regex = ProjectDefinition {
            key: "bad".to_string(),
            name: "非法正则".to_string(),
            color: "#123456".to_string(),
            archived: false,
            rules: vec![ProjectRule {
                field: ProjectRuleField::Title,
                op: ProjectRuleOp::Regex,
                value: "[unclosed".to_string(),
            }],
        };
        assert!(validate_and_normalize_project(bad_regex).is_err());

        let huge_regex = ProjectDefinition {
            key: "huge".to_string(),
            name: "超大正则".to_string(),
            color: "#123456".to_string(),
            archived: false,
            rules: vec![ProjectRule {
                field: ProjectRuleField::Title,
                op: ProjectRuleOp::Regex,
                value: "a{100000}".to_string(),
            }],
        };
        assert!(validate_and_normalize_project(huge_regex).is_err());
    }

    #[test]
    fn 聚合统计应把未命中活动归入未归属并置底排序() {
        let projects = vec![ProjectDefinition {
            key: "alpha".to_string(),
            name: "Alpha".to_string(),
            color: "#3B82F6".to_string(),
            archived: false,
            rules: vec![ProjectRule {
                field: ProjectRuleField::Title,
                op: ProjectRuleOp::Contains,
                value: "alpha".to_string(),
            }],
        }];
        let resolver = ProjectResolver::from_projects(&projects);
        let acts = vec![
            sample_activity("Code", "alpha core", None, None, 300),
            sample_activity("Slack", "random chat", None, None, 900),
        ];
        let usage = resolver.aggregate_activities(&acts, true);
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[0].key, "alpha");
        assert_eq!(usage[0].duration, 300);
        assert_eq!(usage[1].key, UNASSIGNED_PROJECT_KEY);
        assert_eq!(usage[1].duration, 900);
    }
}
