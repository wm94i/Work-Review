//! 渐进式本机 `SKILL.md` 发现与按需加载器。
//!
//! 目录约定：`<data_dir>/skills/<skill-name>/SKILL.md`
//! 安全约束：
//! - 拒绝符号链接目录与符号链接 `SKILL.md`（防路径逃逸）；
//! - 技能名仅允许 `[a-zA-Z0-9_-]`，拒绝 `..` 与路径分隔符；
//! - 单文件上限 64 KiB、最多加载 32 个技能；
//! - 启动/对话仅向 system prompt 注入精简目录（名称 + 一句话描述），命中后通过
//!   `load_skill` 工具按需读取正文，并用数据边界包裹防注入。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_SKILLS: usize = 32;
const MAX_SKILL_FILE_BYTES: u64 = 64 * 1024;
const MAX_SKILL_BODY_CHARS: usize = 16_000;

/// 暴露给提示词目录与前端列表的技能元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: String,
    pub allowed_tools: Vec<String>,
}

/// 单个个人技能的完整详情（用于前端查看与编辑）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub name: String,
    pub description: String,
    pub path: String,
    pub allowed_tools: Vec<String>,
    pub body: String,
}

#[derive(Debug, Clone)]
struct LoadedSkill {
    metadata: SkillMetadata,
    body: String,
}

/// 已加载的本机技能目录快照。
#[derive(Debug, Clone, Default)]
pub struct SkillCatalog {
    skills: BTreeMap<String, LoadedSkill>,
}

impl SkillCatalog {
    /// 从指定 `skills/` 目录扫描合法的 `SKILL.md`。目录不存在时静默返回空目录。
    pub fn load_from_dir(skills_root: &Path) -> Self {
        let Ok(root_meta) = fs::symlink_metadata(skills_root) else {
            return Self::default();
        };
        if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
            return Self::default();
        }
        let Ok(entries) = fs::read_dir(skills_root) else {
            return Self::default();
        };

        let mut dirs: Vec<(String, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|entry| {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if !is_valid_skill_name(&file_name) {
                    return None;
                }
                let path = entry.path();
                let meta = fs::symlink_metadata(&path).ok()?;
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return None;
                }
                Some((file_name, path))
            })
            .collect();
        dirs.sort_by(|a, b| a.0.cmp(&b.0));

        let mut skills = BTreeMap::new();
        for (dir_name, dir_path) in dirs.into_iter().take(MAX_SKILLS) {
            let skill_file = dir_path.join("SKILL.md");
            // 注：std::fs 跨平台不提供 O_NOFOLLOW，这里采用 symlink_metadata + read_to_string 的
            // 常规本机防误挂检查，对单人桌面工作空间已满足防护边界。
            let Ok(file_meta) = fs::symlink_metadata(&skill_file) else {
                continue;
            };
            if file_meta.file_type().is_symlink()
                || !file_meta.is_file()
                || file_meta.len() > MAX_SKILL_FILE_BYTES
            {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&skill_file) else {
                continue;
            };
            let (meta, body) = parse_skill_markdown(&dir_name, &skill_file, &raw);
            if body.trim().is_empty() || skills.contains_key(&meta.name) {
                continue;
            }
            skills.insert(
                meta.name.clone(),
                LoadedSkill {
                    metadata: meta,
                    body,
                },
            );
        }

        Self { skills }
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn list_metadata(&self) -> Vec<SkillMetadata> {
        self.skills.values().map(|s| s.metadata.clone()).collect()
    }

    /// 构造注入 system prompt 的渐进式技能目录（空目录返回 None，零 token 开销）。
    pub fn build_catalog_prompt(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let mut lines = Vec::with_capacity(self.skills.len() + 2);
        lines.push("\n\n[已安装的个人技能（按需加载）] 当用户请求匹配以下技能场景时，先调用 load_skill(name) 读取完整流程说明，再按步骤执行：".to_string());
        for skill in self.skills.values() {
            lines.push(format!(
                "- {}: {}",
                skill.metadata.name, skill.metadata.description
            ));
        }
        Some(lines.join("\n"))
    }

    /// 按名称按需加载完整技能说明正文（用边界标记包裹）。
    pub fn load_skill_body(&self, name: &str) -> Result<String, String> {
        let key = name.trim();
        let skill = self
            .skills
            .get(key)
            .ok_or_else(|| format!("未找到名为「{key}」的个人技能"))?;
        let clipped: String = skill.body.chars().take(MAX_SKILL_BODY_CHARS).collect();
        Ok(format!(
            "<<<技能说明开始 name=\"{}\" >>>\n{}\n<<<技能说明结束>>>",
            skill.metadata.name, clipped
        ))
    }
}

/// 校验个人技能名是否合法：仅允许 1..=64 位 `[a-zA-Z0-9_-]`，拒绝隐藏名、路径分隔符与 Windows 保留设备名。
pub fn is_valid_skill_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    !matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn sanitize_frontmatter_scalar(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

fn escape_double_quoted(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 读取指定名称的个人技能完整详情（拒绝符号链接与非法名称）。
pub fn read_skill_detail(skills_root: &Path, name: &str) -> Result<SkillDetail, String> {
    let trimmed = name.trim();
    if !is_valid_skill_name(trimmed) {
        return Err("技能名称仅支持字母、数字、连字符和下划线（1-64 字符）".to_string());
    }
    let catalog = SkillCatalog::load_from_dir(skills_root);
    let skill = catalog
        .skills
        .get(trimmed)
        .ok_or_else(|| format!("未找到名为「{trimmed}」的个人技能"))?;
    Ok(SkillDetail {
        name: skill.metadata.name.clone(),
        description: skill.metadata.description.clone(),
        path: skill.metadata.path.clone(),
        allowed_tools: skill.metadata.allowed_tools.clone(),
        body: skill.body.clone(),
    })
}

/// 创建或更新本机 `<skills_root>/<name>/SKILL.md` 个人技能（严格校验名称、体积、符号链接与上限）。
pub fn save_skill_to_dir(
    skills_root: &Path,
    name: &str,
    description: &str,
    allowed_tools: &[String],
    body: &str,
) -> Result<SkillMetadata, String> {
    let trimmed_name = name.trim();
    if !is_valid_skill_name(trimmed_name) {
        return Err("技能名称仅支持字母、数字、连字符和下划线（1-64 字符）".to_string());
    }
    let trimmed_body = body.replace("\r\n", "\n");
    let trimmed_body = trimmed_body.trim();
    if trimmed_body.is_empty() {
        return Err("技能流程说明正文不能为空".to_string());
    }
    if trimmed_body.chars().count() > MAX_SKILL_BODY_CHARS {
        return Err(format!(
            "技能流程说明正文超过 {MAX_SKILL_BODY_CHARS} 字符上限，请精简后保存"
        ));
    }

    if let Ok(root_meta) = fs::symlink_metadata(skills_root) {
        if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
            return Err("skills 目录为非法路径或符号链接，拒绝写入".to_string());
        }
    } else {
        fs::create_dir_all(skills_root).map_err(|e| format!("创建 skills 目录失败: {e}"))?;
        let root_meta =
            fs::symlink_metadata(skills_root).map_err(|e| format!("检查 skills 目录失败: {e}"))?;
        if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
            return Err("skills 目录为非法路径或符号链接，拒绝写入".to_string());
        }
    }

    let skill_dir = skills_root.join(trimmed_name);
    let skill_file = skill_dir.join("SKILL.md");
    let is_existing = skill_file.exists();
    if !is_existing {
        let existing_catalog = SkillCatalog::load_from_dir(skills_root);
        if existing_catalog.len() >= MAX_SKILLS {
            return Err(format!("最多支持安装 {MAX_SKILLS} 个个人技能"));
        }
    }

    if let Ok(dir_meta) = fs::symlink_metadata(&skill_dir) {
        if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
            return Err("目标技能目录为符号链接或非目录，拒绝写入".to_string());
        }
    } else {
        fs::create_dir(&skill_dir).map_err(|e| format!("创建技能目录失败: {e}"))?;
    }

    if let Ok(file_meta) = fs::symlink_metadata(&skill_file) {
        if file_meta.file_type().is_symlink() || !file_meta.is_file() {
            return Err("目标 SKILL.md 为符号链接或非普通文件，拒绝覆盖".to_string());
        }
    }

    let clean_desc = sanitize_frontmatter_scalar(description, 160);
    let desc_value = if clean_desc.is_empty() {
        "用户自定义工作流技能".to_string()
    } else {
        clean_desc
    };
    let clean_tools: Vec<String> = allowed_tools
        .iter()
        .map(|s| s.trim())
        .filter(|s| is_valid_skill_name(s))
        .take(32)
        .map(str::to_string)
        .collect();

    let mut markdown = String::new();
    markdown.push_str("---\n");
    markdown.push_str(&format!("name: {trimmed_name}\n"));
    markdown.push_str(&format!(
        "description: \"{}\"\n",
        escape_double_quoted(&desc_value)
    ));
    if !clean_tools.is_empty() {
        markdown.push_str(&format!("allowed_tools: [{}]\n", clean_tools.join(", ")));
    }
    markdown.push_str("---\n\n");
    markdown.push_str(trimmed_body);
    markdown.push('\n');

    if markdown.len() as u64 > MAX_SKILL_FILE_BYTES {
        return Err("SKILL.md 内容超过 64 KiB 上限，请精简后重试".to_string());
    }

    // 先写同目录临时文件再原子重命名，避免并发保存或中途异常产生半截 SKILL.md。
    let tmp_file = skill_dir.join(format!(".SKILL.md.{}.tmp", uuid::Uuid::new_v4()));
    if let Err(err) = fs::write(&tmp_file, markdown.as_bytes()) {
        let _ = fs::remove_file(&tmp_file);
        return Err(format!("写入 SKILL.md 临时文件失败: {err}"));
    }
    if let Err(err) = fs::rename(&tmp_file, &skill_file) {
        let _ = fs::remove_file(&tmp_file);
        return Err(format!("提交 SKILL.md 失败: {err}"));
    }
    let (meta, _) = parse_skill_markdown(trimmed_name, &skill_file, &markdown);
    Ok(meta)
}

/// 删除指定名称的个人技能（仅删除 `SKILL.md` 及空目录，拒绝符号链接，绝不递归误删其他文件）。
pub fn delete_skill_from_dir(skills_root: &Path, name: &str) -> Result<(), String> {
    let trimmed_name = name.trim();
    if !is_valid_skill_name(trimmed_name) {
        return Err("技能名称非法，拒绝删除".to_string());
    }
    let root_meta =
        fs::symlink_metadata(skills_root).map_err(|_| "skills 目录不存在".to_string())?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err("skills 目录为符号链接或非法路径，拒绝删除".to_string());
    }

    let skill_dir = skills_root.join(trimmed_name);
    let dir_meta = fs::symlink_metadata(&skill_dir)
        .map_err(|_| format!("未找到个人技能「{trimmed_name}」"))?;
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return Err("目标技能目录为符号链接，拒绝删除".to_string());
    }

    let skill_file = skill_dir.join("SKILL.md");
    let file_meta = fs::symlink_metadata(&skill_file)
        .map_err(|_| format!("技能目录中缺少 SKILL.md: {trimmed_name}"))?;
    if file_meta.file_type().is_symlink() || !file_meta.is_file() {
        return Err("目标 SKILL.md 为符号链接，拒绝删除".to_string());
    }

    fs::remove_file(&skill_file).map_err(|e| format!("删除 SKILL.md 失败: {e}"))?;
    // 仅当目录已空时清理空目录，保留用户可能额外放置的其他本地文件。
    let _ = fs::remove_dir(&skill_dir);
    Ok(())
}

fn parse_skill_markdown(fallback_name: &str, path: &Path, raw: &str) -> (SkillMetadata, String) {
    let normalized = raw.replace("\r\n", "\n");
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut allowed_tools = Vec::new();
    let mut body = normalized.as_str();

    if let Some(rest) = normalized.strip_prefix("---\n") {
        if let Some(end_idx) = rest.find("\n---\n") {
            let frontmatter = &rest[..end_idx];
            body = &rest[end_idx + 5..];
            for line in frontmatter.lines() {
                let trimmed = line.trim();
                if let Some(val) = trimmed.strip_prefix("name:") {
                    let candidate = unquote(val);
                    if is_valid_skill_name(&candidate) {
                        name = candidate;
                    }
                } else if let Some(val) = trimmed.strip_prefix("description:") {
                    description = unquote(val);
                } else if let Some(val) = trimmed.strip_prefix("allowed_tools:") {
                    let cleaned = val.trim().trim_start_matches('[').trim_end_matches(']');
                    allowed_tools = cleaned
                        .split(',')
                        .map(unquote)
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
        }
    }

    if description.is_empty() {
        description = body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| l.trim_start_matches('#').trim().to_string())
            .unwrap_or_else(|| "用户自定义工作流技能".to_string());
    }

    let desc_clipped: String = description.chars().take(160).collect();
    (
        SkillMetadata {
            name,
            description: desc_clipped,
            path: path.to_string_lossy().to_string(),
            allowed_tools,
        },
        body.trim().to_string(),
    )
}

fn unquote(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(ch);
            }
        }
        out.trim().to_string()
    } else if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 应正确解析frontmatter并生成渐进式目录与按需正文() {
        let root = std::env::temp_dir().join(format!("wr-skills-test-{}", uuid::Uuid::new_v4()));
        let skill_dir = root.join("weekly-obsidian");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: weekly-obsidian\ndescription: 汇总本周工时与 Git 提交\nallowed_tools: [project_stats, weekly_review]\n---\n# 步骤\n1. 查询项目工时\n2. 生成周复盘\n",
        )
        .unwrap();

        let catalog = SkillCatalog::load_from_dir(&root);
        assert_eq!(catalog.len(), 1);
        let list = catalog.list_metadata();
        assert_eq!(list[0].name, "weekly-obsidian");
        assert_eq!(list[0].description, "汇总本周工时与 Git 提交");
        assert_eq!(
            list[0].allowed_tools,
            vec!["project_stats", "weekly_review"]
        );

        let prompt = catalog
            .build_catalog_prompt()
            .expect("应生成技能目录提示词");
        assert!(prompt.contains("weekly-obsidian"));
        assert!(!prompt.contains("# 步骤"), "目录提示词不得预加载完整正文");

        let body = catalog
            .load_skill_body("weekly-obsidian")
            .expect("应可按需加载正文");
        assert!(body.contains("<<<技能说明开始"));
        assert!(body.contains("1. 查询项目工时"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn 隐藏目录与非法技能名应被忽略() {
        let root = std::env::temp_dir().join(format!("wr-skills-bad-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join(".hidden").join("SKILL.md"), "# hidden").unwrap();

        let catalog = SkillCatalog::load_from_dir(&root);
        assert!(catalog.is_empty());
        assert!(catalog.build_catalog_prompt().is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn 同名frontmatter技能冲突时保留先扫描项且不静默覆盖() {
        let root = std::env::temp_dir().join(format!("wr-skills-dup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("a-first")).unwrap();
        fs::create_dir_all(root.join("b-second")).unwrap();
        fs::write(
            root.join("a-first").join("SKILL.md"),
            "---\nname: shared-name\ndescription: first\n---\nfirst body",
        )
        .unwrap();
        fs::write(
            root.join("b-second").join("SKILL.md"),
            "---\nname: shared-name\ndescription: second\n---\nsecond body",
        )
        .unwrap();

        let catalog = SkillCatalog::load_from_dir(&root);
        assert_eq!(catalog.len(), 1);
        let body = catalog.load_skill_body("shared-name").unwrap();
        assert!(body.contains("first body"));
        assert!(!body.contains("second body"));

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn 符号链接技能目录与文件应被拒绝() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("wr-skills-sym-{}", uuid::Uuid::new_v4()));
        let outside = std::env::temp_dir().join(format!("wr-skills-out-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("SKILL.md");
        fs::write(&outside_file, "# outside secret").unwrap();

        symlink(&outside, root.join("linked-dir")).unwrap();
        fs::create_dir_all(root.join("linked-file-dir")).unwrap();
        symlink(&outside_file, root.join("linked-file-dir").join("SKILL.md")).unwrap();

        let catalog = SkillCatalog::load_from_dir(&root);
        assert!(catalog.is_empty());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn 技能创建读取更新删除应通过校验且防frontmatter注入() {
        let root = std::env::temp_dir().join(format!("wr-skills-crud-{}", uuid::Uuid::new_v4()));

        // 非法名称与 Windows 保留设备名必须拒绝
        assert!(save_skill_to_dir(&root, "../escape", "bad", &[], "body").is_err());
        assert!(save_skill_to_dir(&root, ".hidden", "bad", &[], "body").is_err());
        assert!(save_skill_to_dir(&root, "CON", "bad", &[], "body").is_err());
        assert!(save_skill_to_dir(&root, "nul", "bad", &[], "body").is_err());
        assert!(save_skill_to_dir(&root, "valid-skill", "desc", &[], "   ").is_err());

        let meta = save_skill_to_dir(
            &root,
            "weekly-standup",
            "生成\"周会\"纪要\\模板\nname: hijacked",
            &[
                "weekly_review".into(),
                "../bad".into(),
                "project_stats".into(),
            ],
            "1. 调用 weekly_review\n2. 汇总项目工时",
        )
        .expect("创建技能应成功");
        assert_eq!(meta.name, "weekly-standup");
        assert_eq!(meta.description, "生成\"周会\"纪要\\模板name: hijacked");
        assert_eq!(meta.allowed_tools, vec!["weekly_review", "project_stats"]);
        assert!(!meta.description.contains('\n'));

        let detail = read_skill_detail(&root, "weekly-standup").expect("读取技能详情应成功");
        assert_eq!(detail.description, "生成\"周会\"纪要\\模板name: hijacked");
        assert!(detail.body.contains("汇总项目工时"));

        // 同名更新（覆盖写）应原子生效
        save_skill_to_dir(
            &root,
            "weekly-standup",
            "更新后的周复盘",
            &["weekly_review".into()],
            "1. 更新后正文",
        )
        .expect("覆盖更新技能应成功");
        let updated = read_skill_detail(&root, "weekly-standup").expect("读取更新后详情应成功");
        assert_eq!(updated.description, "更新后的周复盘");
        assert_eq!(updated.body, "1. 更新后正文");

        // 超 16,000 字符上限拒绝（与 load_skill_body 对齐）
        let huge_body = "x".repeat(MAX_SKILL_BODY_CHARS + 1);
        assert!(save_skill_to_dir(&root, "too-big", "big", &[], &huge_body).is_err());

        delete_skill_from_dir(&root, "weekly-standup").expect("删除技能应成功");
        assert!(SkillCatalog::load_from_dir(&root).is_empty());

        let _ = fs::remove_dir_all(&root);
    }
}
