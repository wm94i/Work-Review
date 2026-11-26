//! 本地 Git 仓库提交采集与格式化：把当天本机代码提交关联到工作日报与 AI 上下文。
//!
//! 设计原则：
//! - 只读本地 `git` 命令，不联网、不写入仓库、不经过 shell（直接传 argv 防命令注入）；
//! - 严格校验日期格式（`YYYY-MM-DD`），拒绝 `--` 开头的作者过滤词防止参数注入；
//! - 提交标题在进入日报/AI 上下文前统一经 `PrivacyFilter::filter_text` 脱敏。

use crate::analysis::AppLocale;
use crate::privacy::PrivacyFilter;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

const MAX_COMMITS_PER_REPO: usize = 50;
const MAX_REPOS: usize = 32;

/// 单条 Git 提交记录
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitEntry {
    pub hash: String,
    pub timestamp: i64,
    pub time_label: String,
    pub subject: String,
}

/// 单个本地仓库在指定日期的提交汇总
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoSummary {
    pub repo_name: String,
    pub repo_path: String,
    pub branch: Option<String>,
    pub commits: Vec<GitCommitEntry>,
}

fn is_valid_iso_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
}

fn build_git_command(repo_dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo_dir);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: 后台执行 git 时不闪烁命令行窗口
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn detect_repo_branch(repo_dir: &Path) -> Option<String> {
    let output = build_git_command(repo_dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        None
    } else {
        Some(branch)
    }
}

fn detect_default_author(repo_dir: &Path) -> Option<String> {
    for key in ["user.email", "user.name"] {
        let Ok(output) = build_git_command(repo_dir)
            .args(["config", "--get", key])
            .output()
        else {
            continue;
        };
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !value.is_empty() && !value.starts_with('-') {
                return Some(value);
            }
        }
    }
    None
}

fn format_local_hhmm(timestamp: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string())
}

fn parse_git_log_lines(stdout: &str) -> Vec<GitCommitEntry> {
    let mut commits = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(3, '\x1f');
        let Some(hash) = parts.next().map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        let Some(ts) = parts.next().and_then(|v| v.trim().parse::<i64>().ok()) else {
            continue;
        };
        let subject = parts
            .next()
            .map(|s| s.replace(['\r', '\n'], " ").trim().to_string())
            .unwrap_or_default();
        if subject.is_empty() {
            continue;
        }
        commits.push(GitCommitEntry {
            hash: hash.to_string(),
            timestamp: ts,
            time_label: format_local_hhmm(ts),
            subject,
        });
    }
    commits
}

/// 采集指定日期的本地 Git 提交（仅返回有提交记录的仓库）。
pub fn collect_git_commits_for_date(
    repo_paths: &[String],
    date: &str,
    author_filter: Option<&str>,
) -> Vec<GitRepoSummary> {
    let date = date.trim();
    if !is_valid_iso_date(date) || repo_paths.is_empty() {
        return Vec::new();
    }

    let explicit_author = author_filter
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.starts_with('-'))
        .map(str::to_string);

    let since = format!("{date} 00:00:00");
    let until = format!("{date} 23:59:59");
    let mut results = Vec::new();

    for raw_path in repo_paths.iter().take(MAX_REPOS) {
        let trimmed = raw_path.trim();
        if trimmed.is_empty() {
            continue;
        }
        let repo_dir = Path::new(trimmed);
        if !repo_dir.is_dir() || !repo_dir.join(".git").exists() {
            continue;
        }

        let effective_author = explicit_author
            .clone()
            .or_else(|| detect_default_author(repo_dir));

        let mut cmd = build_git_command(repo_dir);
        cmd.args([
            "log",
            "--no-merges",
            "--date=unix",
            "--pretty=format:%h%x1f%ct%x1f%s",
        ])
        .arg(format!("--since={since}"))
        .arg(format!("--until={until}"))
        .arg(format!("--max-count={MAX_COMMITS_PER_REPO}"));
        if let Some(ref author) = effective_author {
            cmd.arg(format!("--author={author}"));
        }

        let Ok(output) = cmd.output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let commits = parse_git_log_lines(&stdout);
        if commits.is_empty() {
            continue;
        }

        let repo_name = repo_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| trimmed.to_string());
        let branch = detect_repo_branch(repo_dir);

        results.push(GitRepoSummary {
            repo_name,
            repo_path: trimmed.to_string(),
            branch,
            commits,
        });
    }

    results
}

/// 对提交主题执行隐私与密钥脱敏（避免提交信息里的临时 token 或密码进入日报与 AI 上下文）。
pub fn sanitize_git_commits(
    summaries: Vec<GitRepoSummary>,
    privacy_filter: &PrivacyFilter,
) -> Vec<GitRepoSummary> {
    summaries
        .into_iter()
        .map(|mut repo| {
            for commit in &mut repo.commits {
                commit.subject = privacy_filter.filter_text(&commit.subject);
            }
            repo
        })
        .collect()
}

/// 统计总提交数
pub fn total_commit_count(summaries: &[GitRepoSummary]) -> usize {
    summaries.iter().map(|repo| repo.commits.len()).sum()
}

/// 将 Git 提交汇总渲染为日报 Markdown 段落（无提交时返回空串）。
pub fn format_git_commits_markdown(summaries: &[GitRepoSummary], locale: AppLocale) -> String {
    let total = total_commit_count(summaries);
    if total == 0 {
        return String::new();
    }

    let heading = match locale {
        AppLocale::ZhCn => "## 代码提交产出",
        AppLocale::ZhTw => "## 程式碼提交產出",
        AppLocale::En => "## Git Commits",
        AppLocale::Ar => "## التزامات Git",
    };
    let summary_line = match locale {
        AppLocale::ZhCn => format!(
            "> 共关联 **{}** 个仓库，**{}** 条提交记录。",
            summaries.len(),
            total
        ),
        AppLocale::ZhTw => format!(
            "> 共關聯 **{}** 個儲存庫，**{}** 筆提交記錄。",
            summaries.len(),
            total
        ),
        AppLocale::En => format!(
            "> Linked **{}** repository(ies) with **{}** commit(s).",
            summaries.len(),
            total
        ),
        AppLocale::Ar => format!(
            "> تم ربط **{}** مستودع(ات) بإجمالي **{}** التزام(ات).",
            summaries.len(),
            total
        ),
    };

    let mut out = format!("{heading}\n\n{summary_line}\n");
    for repo in summaries {
        let branch_badge = repo
            .branch
            .as_deref()
            .map(|b| format!(" (`{b}`)"))
            .unwrap_or_default();
        out.push_str(&format!("\n### {}{branch_badge}\n\n", repo.repo_name));
        for commit in &repo.commits {
            out.push_str(&format!(
                "- `{}` {} — {}\n",
                commit.hash, commit.time_label, commit.subject
            ));
        }
    }
    out
}

/// 将 Git 提交汇总压缩为供 AI 日报提示词参考的精简上下文（最多展示前 20 条）。
pub fn format_git_commits_prompt_context(
    summaries: &[GitRepoSummary],
    locale: AppLocale,
) -> String {
    let total = total_commit_count(summaries);
    if total == 0 {
        return String::new();
    }
    let header = match locale {
        AppLocale::ZhCn => "今日关联的本地 Git 代码提交（可作为实际工作产出的直接依据）：",
        AppLocale::ZhTw => "今日關聯的本地 Git 程式碼提交（可作為實際工作產出的直接依據）：",
        AppLocale::En => {
            "Linked local Git commits for today (use as direct evidence of engineering output):"
        }
        AppLocale::Ar => {
            "التزامات Git المحلية المرتبطة باليوم (استخدمها كدليل مباشر على مخرجات العمل):"
        }
    };
    let mut lines = vec![header.to_string()];
    let mut shown = 0usize;
    'outer: for repo in summaries {
        let branch = repo
            .branch
            .as_deref()
            .map(|b| format!("@{b}"))
            .unwrap_or_default();
        for commit in &repo.commits {
            if shown >= 20 {
                break 'outer;
            }
            lines.push(format!(
                "- [{}{}] {} ({}) {}",
                repo.repo_name, branch, commit.hash, commit.time_label, commit.subject
            ));
            shown += 1;
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PrivacyConfig;

    #[test]
    fn 非法日期或空仓库列表应安全返回空() {
        assert!(collect_git_commits_for_date(&[], "2026-11-23", None).is_empty());
        assert!(collect_git_commits_for_date(&[".".to_string()], "2026-99-99", None).is_empty());
        assert!(collect_git_commits_for_date(&[".".to_string()], "--exec=evil", None).is_empty());
    }

    #[test]
    fn 应解析本地临时仓库当日提交并格式化四语markdown与脱敏() {
        let temp_dir =
            std::env::temp_dir().join(format!("wr-git-commits-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let run_git = |args: &[&str]| {
            let status = build_git_command(&temp_dir)
                .args(args)
                .status()
                .expect("git 命令应可用");
            assert!(status.success(), "git {:?} 应执行成功", args);
        };

        run_git(&["init"]);
        run_git(&["config", "user.name", "Work Review Tester"]);
        run_git(&["config", "user.email", "tester@example.com"]);

        std::fs::write(temp_dir.join("README.md"), "hello\n").unwrap();
        run_git(&["add", "README.md"]);
        run_git(&[
            "commit",
            "-m",
            "feat: implement git link with sk-proj-abcdefghijklmnopqrstuvwx1234",
        ]);

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let raw = collect_git_commits_for_date(
            &[temp_dir.to_string_lossy().to_string()],
            &today,
            Some("tester@example.com"),
        );
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].commits.len(), 1);
        assert!(raw[0].commits[0].subject.contains("implement git link"));

        let filter = PrivacyFilter::from_config(&PrivacyConfig::default());
        let sanitized = sanitize_git_commits(raw, &filter);
        assert!(
            !sanitized[0].commits[0]
                .subject
                .contains("sk-proj-abcdefghijklmnopqrstuvwx1234"),
            "提交标题中的密钥必须被脱敏"
        );
        assert!(sanitized[0].commits[0].subject.contains("[已过滤]"));

        for locale in [
            AppLocale::ZhCn,
            AppLocale::ZhTw,
            AppLocale::En,
            AppLocale::Ar,
        ] {
            let md = format_git_commits_markdown(&sanitized, locale);
            assert!(md.contains(&sanitized[0].commits[0].hash));
            let prompt_ctx = format_git_commits_prompt_context(&sanitized, locale);
            assert!(prompt_ctx.contains(&sanitized[0].repo_name));
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn git日志解析应容忍空行和非法行() {
        let sample = "\nabc1234\x1f1700000000\x1ffix: handle edge case\nbadline\n\ndef5678\x1fnot-a-ts\x1fskip\n";
        let commits = parse_git_log_lines(sample);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, "abc1234");
        assert_eq!(commits[0].subject, "fix: handle edge case");
    }
}
