//! Obsidian / Logseq / 本地 Markdown Vault 深度导出工具。
//!
//! 能力：
//! 1. 可选生成标准 YAML Frontmatter（日期、总工时、工作时长、专注度评分、主要应用、Git 提交数、标签），
//!    若已有 Daily Note 已包含 Frontmatter，仅安全更新/追加 Work Review 字段，不覆盖用户原有属性；
//! 2. 可选按「指定标题区块」增量写入：仅替换 `<!-- work-review:start -->`...`<!-- work-review:end -->`
//!    或目标标题下的区块内容，保留用户在 Daily Note 中手写的其它全部段落。

use serde::{Deserialize, Serialize};

pub const OBSIDIAN_BLOCK_START: &str = "<!-- work-review:start -->";
pub const OBSIDIAN_BLOCK_END: &str = "<!-- work-review:end -->";

/// 日报导出时的结构化元数据（用于 YAML Frontmatter）
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportFrontmatterMeta {
    pub date: String,
    pub total_minutes: i64,
    pub work_minutes: i64,
    pub focus_score: Option<u8>,
    pub top_apps: Vec<String>,
    pub git_commit_count: usize,
}

fn yaml_quote(raw: &str) -> String {
    let cleaned = raw.replace(['\r', '\n'], " ");
    let escaped = cleaned.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// 将用户输入的标题规范化为 Markdown 标题行（如 `"Work Review"` → `"## Work Review"`）。
pub fn normalize_section_heading(raw: &str) -> Option<String> {
    let trimmed = raw.replace(['\r', '\n'], " ").trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') {
        let hashes = trimmed.chars().take_while(|c| *c == '#').count().min(6);
        let title = trimmed[hashes..].trim();
        if title.is_empty() {
            return None;
        }
        Some(format!("{} {title}", "#".repeat(hashes)))
    } else {
        Some(format!("## {trimmed}"))
    }
}

fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    match trimmed[hashes..].chars().next() {
        Some(' ' | '\t') => Some(hashes),
        _ => None,
    }
}

fn build_frontmatter_entries(meta: &ExportFrontmatterMeta) -> Vec<(String, String)> {
    let mut entries = vec![
        ("date".to_string(), meta.date.clone()),
        ("source".to_string(), "work-review".to_string()),
        (
            "work_review_total_minutes".to_string(),
            meta.total_minutes.max(0).to_string(),
        ),
        (
            "work_review_work_minutes".to_string(),
            meta.work_minutes.max(0).to_string(),
        ),
    ];
    if let Some(score) = meta.focus_score {
        entries.push(("work_review_focus_score".to_string(), score.to_string()));
    }
    if meta.git_commit_count > 0 {
        entries.push((
            "work_review_git_commits".to_string(),
            meta.git_commit_count.to_string(),
        ));
    }
    if !meta.top_apps.is_empty() {
        let list = meta
            .top_apps
            .iter()
            .take(5)
            .map(|app| yaml_quote(app))
            .collect::<Vec<_>>()
            .join(", ");
        entries.push(("work_review_top_apps".to_string(), format!("[{list}]")));
    }
    entries
}

fn build_new_frontmatter(meta: &ExportFrontmatterMeta) -> String {
    let mut lines = vec!["---".to_string()];
    for (key, value) in build_frontmatter_entries(meta) {
        lines.push(format!("{key}: {value}"));
    }
    lines.push("tags: [work-review, daily-report]".to_string());
    lines.push("---".to_string());
    lines.join("\n")
}

/// 若文本以 `---` YAML Frontmatter 开头，返回 `(frontmatter_body, rest_after_closing_delimiter)`。
fn split_existing_frontmatter(content: &str) -> Option<(&str, &str)> {
    let after_open = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))?;
    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            let body = &after_open[..offset];
            let rest = &after_open[offset + line.len()..];
            return Some((body, rest));
        }
        offset += line.len();
    }
    None
}

fn merge_frontmatter(content: &str, meta: &ExportFrontmatterMeta) -> String {
    let Some((existing_body, rest)) = split_existing_frontmatter(content) else {
        let fm = build_new_frontmatter(meta);
        if content.trim().is_empty() {
            return format!("{fm}\n");
        }
        return format!("{fm}\n\n{}", content.trim_start_matches(['\r', '\n']));
    };

    // 仅更新/追加 work_review_* 字段，保留用户原有的 date/tags/aliases 等全部字段
    let managed_entries: Vec<(String, String)> = build_frontmatter_entries(meta)
        .into_iter()
        .filter(|(k, _)| k.starts_with("work_review_"))
        .collect();
    let managed_keys: std::collections::HashSet<&str> =
        managed_entries.iter().map(|(k, _)| k.as_str()).collect();

    let mut preserved: Vec<String> = existing_body
        .lines()
        .filter(|line| {
            let key = line.split(':').next().map(str::trim).unwrap_or("");
            !managed_keys.contains(key)
        })
        .map(str::to_string)
        .collect();
    for (key, value) in managed_entries {
        preserved.push(format!("{key}: {value}"));
    }

    format!("---\n{}\n---\n{}", preserved.join("\n"), rest)
}

/// 将日报正文中的一级标题降级为三级标题，以便安全嵌套在用户的 `## Work Review` 区块内。
fn demote_top_heading_for_section(report_markdown: &str, target_level: usize) -> String {
    let shift = target_level.min(4);
    let mut out = Vec::new();
    let mut in_code_fence = false;
    for line in report_markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            out.push(line.to_string());
            continue;
        }
        if !in_code_fence {
            if let Some(level) = heading_level(line) {
                let new_level = (level + shift).min(6);
                let title = trimmed[level..].trim();
                out.push(format!("{} {title}", "#".repeat(new_level)));
                continue;
            }
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

/// 替换已有文本中 `<!-- work-review:start -->` 到 `<!-- work-review:end -->` 的区域。
fn replace_managed_markers(existing: &str, wrapped_block: &str) -> Option<String> {
    let start = existing.find(OBSIDIAN_BLOCK_START)?;
    let search_from = start + OBSIDIAN_BLOCK_START.len();
    let rel_end = existing[search_from..].find(OBSIDIAN_BLOCK_END)?;
    let end = search_from + rel_end + OBSIDIAN_BLOCK_END.len();
    let mut out = String::with_capacity(existing.len() + wrapped_block.len());
    out.push_str(&existing[..start]);
    out.push_str(wrapped_block);
    out.push_str(&existing[end..]);
    Some(out)
}

/// 在已有文本中定位目标标题行，并将该标题下到下一个同级/更高级标题之间的内容替换为 `wrapped_block`。
fn replace_under_heading(existing: &str, heading: &str, wrapped_block: &str) -> Option<String> {
    let target_level = heading_level(heading).unwrap_or(2);
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let mut target_idx = None;
    let mut in_code_fence = false;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if !in_code_fence && trimmed == heading {
            target_idx = Some(idx);
            break;
        }
    }
    let idx = target_idx?;

    let mut end_idx = lines.len();
    in_code_fence = false;
    for (next_idx, line) in lines.iter().enumerate().skip(idx + 1) {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if !in_code_fence {
            if let Some(level) = heading_level(trimmed) {
                if level <= target_level {
                    end_idx = next_idx;
                    break;
                }
            }
        }
    }

    let before: String = lines[..=idx].concat();
    let after: String = lines[end_idx..].concat();
    let mut out = String::new();
    out.push_str(before.trim_end_matches(['\r', '\n']));
    out.push_str("\n\n");
    out.push_str(wrapped_block);
    if !after.is_empty() {
        out.push_str("\n\n");
        out.push_str(after.trim_start_matches(['\r', '\n']));
    } else {
        out.push('\n');
    }
    Some(out)
}

/// 生成最终写入磁盘的 Markdown 内容（兼容既有全量覆盖与 Obsidian Frontmatter / 区块增量写入）。
pub fn render_obsidian_export(
    existing_file: Option<&str>,
    report_markdown: &str,
    meta: &ExportFrontmatterMeta,
    include_frontmatter: bool,
    section_heading: Option<&str>,
) -> String {
    let normalized_heading = section_heading.and_then(normalize_section_heading);

    // 1. 全量覆盖模式（未指定目标标题区块）
    let Some(heading) = normalized_heading else {
        if !include_frontmatter {
            return report_markdown.to_string();
        }
        return merge_frontmatter(report_markdown, meta);
    };

    // 2. Obsidian / Daily Note 指定标题区块增量模式
    let target_level = heading_level(&heading).unwrap_or(2);
    let nested_body = demote_top_heading_for_section(report_markdown.trim(), target_level);
    let wrapped_block = format!("{OBSIDIAN_BLOCK_START}\n{nested_body}\n{OBSIDIAN_BLOCK_END}");

    let merged_body = match existing_file.filter(|text| !text.trim().is_empty()) {
        Some(existing) => {
            if let Some(replaced) = replace_managed_markers(existing, &wrapped_block) {
                replaced
            } else if let Some(replaced) = replace_under_heading(existing, &heading, &wrapped_block)
            {
                replaced
            } else {
                format!(
                    "{}\n\n{heading}\n\n{wrapped_block}\n",
                    existing.trim_end_matches(['\r', '\n'])
                )
            }
        }
        None => format!("{heading}\n\n{wrapped_block}\n"),
    };

    if include_frontmatter {
        merge_frontmatter(&merged_body, meta)
    } else {
        merged_body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> ExportFrontmatterMeta {
        ExportFrontmatterMeta {
            date: "2026-11-23".to_string(),
            total_minutes: 420,
            work_minutes: 360,
            focus_score: Some(84),
            top_apps: vec!["VS Code".to_string(), "Chrome".to_string()],
            git_commit_count: 5,
        }
    }

    #[test]
    fn 默认关闭扩展选项时应原样透传日报内容() {
        let report = "# 工作日报\n\n正文内容";
        let out = render_obsidian_export(None, report, &sample_meta(), false, None);
        assert_eq!(out, report);
    }

    #[test]
    fn 全量模式开启frontmatter应输出标准yaml头() {
        let report = "# 工作日报\n\n正文内容";
        let out = render_obsidian_export(None, report, &sample_meta(), true, None);
        assert!(out.starts_with("---\ndate: 2026-11-23\nsource: work-review\n"));
        assert!(out.contains("work_review_total_minutes: 420"));
        assert!(out.contains("work_review_focus_score: 84"));
        assert!(out.contains("work_review_git_commits: 5"));
        assert!(out.contains("tags: [work-review, daily-report]"));
        assert!(out.ends_with("# 工作日报\n\n正文内容"));
    }

    #[test]
    fn 区块模式应保留用户手写日记并支持幂等重复更新() {
        let user_note = "---\ndate: 2026-11-23\nmood: happy\ntags: [journal]\n---\n\n# Daily Note\n\n## Morning Plan\n- Finish M1\n\n## Work Review\n旧占位内容\n\n## Evening Reflection\nKeep this section!\n";
        let first = render_obsidian_export(
            Some(user_note),
            "# 工作日报\n\n## 一、概览\n第一次内容",
            &sample_meta(),
            true,
            Some("Work Review"),
        );
        // 用户原有 frontmatter 字段与手写段落必须完好保留
        assert!(first.contains("mood: happy"));
        assert!(first.contains("tags: [journal]"));
        assert!(first.contains("work_review_total_minutes: 420"));
        assert!(first.contains("## Morning Plan\n- Finish M1"));
        assert!(first.contains("## Evening Reflection\nKeep this section!"));
        assert!(!first.contains("旧占位内容"));
        // 日报 H1/H2 应正确降级避免破坏 Daily Note 层级
        assert!(first.contains("### 工作日报"));
        assert!(first.contains("#### 一、概览"));

        // 二次写入应幂等替换标记区块，不重复追加标题或字段
        let mut meta2 = sample_meta();
        meta2.total_minutes = 480;
        let second = render_obsidian_export(
            Some(&first),
            "# 工作日报\n\n## 一、概览\n第二次更新",
            &meta2,
            true,
            Some("## Work Review"),
        );
        assert_eq!(second.matches("## Work Review").count(), 1);
        assert_eq!(second.matches(OBSIDIAN_BLOCK_START).count(), 1);
        assert_eq!(second.matches("work_review_total_minutes:").count(), 1);
        assert!(second.contains("work_review_total_minutes: 480"));
        assert!(second.contains("第二次更新"));
        assert!(!second.contains("第一次内容"));
        assert!(second.contains("## Evening Reflection\nKeep this section!"));
    }

    #[test]
    fn 区块模式目标标题不存在时应追加到文档末尾() {
        let user_note = "# 2026-11-23\n\n手写随笔\n";
        let out = render_obsidian_export(
            Some(user_note),
            "# 工作日报\n\n内容",
            &sample_meta(),
            false,
            Some("## Work Log"),
        );
        assert!(out
            .starts_with("# 2026-11-23\n\n手写随笔\n\n## Work Log\n\n<!-- work-review:start -->"));
    }
}
