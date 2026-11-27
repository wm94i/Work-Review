//! 上下文工程：Token 预算估算、超长工具输出 Artifact 分页与确定性对话压缩。
//!
//! 设计约束：
//! - 不依赖外部 tokenizer，采用中英文混合感知估算（CJK 与 ASCII 分别折算），毫秒级完成。
//! - 超长工具结果自动截断为「头尾摘录 + artifact_id 提示」，完整内容保存在本次 Run 的内存
//!   [`ArtifactStore`] 中（带条数与体积上限），模型可按需调用 `read_run_artifact` 分页读取。
//! - 对话压缩（Compaction）绝不拆散 assistant `tool_calls` 与后续 `tool` 结果的配对，避免触发
//!   OpenAI / Claude / Gemini 协议校验报错。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 单次 Run 的上下文预算配置。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBudget {
    /// 上下文窗口目标 token 上限（默认 16,000，兼顾本地小模型与云端成本）。
    pub max_context_tokens: usize,
    /// 超过该比例时触发自动压缩（默认 0.80）。
    pub compaction_trigger_ratio: f32,
    /// 单次工具输出直接放入上下文的最大字符数，超出则存入 Artifact。
    pub max_tool_result_chars: usize,
    /// 压缩时至少保留的尾部消息条数。
    pub preserve_recent_messages: usize,
    /// 压缩摘要文本的最大字符数。
    pub max_compacted_summary_chars: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_context_tokens: 16_000,
            compaction_trigger_ratio: 0.80,
            max_tool_result_chars: 6_000,
            preserve_recent_messages: 6,
            max_compacted_summary_chars: 3_200,
        }
    }
}

impl ContextBudget {
    pub fn compaction_threshold_tokens(&self) -> usize {
        let ratio = self.compaction_trigger_ratio.clamp(0.4, 0.98);
        ((self.max_context_tokens as f64) * (ratio as f64)).round() as usize
    }
}

/// 轻量级中英文混合 Token 估算器。
///
/// 经验值：ASCII 约 4 字符 ≈ 1 token；非 ASCII（汉字/日韩/符号）约 1.4 字符 ≈ 1 token。
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut ascii_chars: usize = 0;
    let mut non_ascii_chars: usize = 0;
    for ch in text.chars() {
        if ch.is_ascii() {
            ascii_chars += 1;
        } else {
            non_ascii_chars += 1;
        }
    }
    let ascii_tokens = ascii_chars.div_ceil(4);
    let non_ascii_tokens = (non_ascii_chars * 10).div_ceil(14);
    (ascii_tokens + non_ascii_tokens).max(1)
}

/// 按字符数安全切片（绝不在多字节 UTF-8 中间截断）。
pub fn slice_chars(
    text: &str,
    start_char: usize,
    max_chars: usize,
) -> (String, usize, Option<usize>) {
    let total_chars = text.chars().count();
    if start_char >= total_chars || max_chars == 0 {
        return (String::new(), total_chars, None);
    }
    let end_char = start_char.saturating_add(max_chars).min(total_chars);
    let slice: String = text
        .chars()
        .skip(start_char)
        .take(end_char - start_char)
        .collect();
    let next_offset = (end_char < total_chars).then_some(end_char);
    (slice, total_chars, next_offset)
}

/// 单条内存 Artifact 条目。
#[derive(Debug, Clone)]
pub struct RunArtifact {
    pub id: String,
    pub tool_name: String,
    pub content: String,
    pub total_chars: usize,
    pub truncated: bool,
}

/// 读取 Artifact 分页切片结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactSlice {
    pub artifact_id: String,
    pub tool_name: String,
    pub offset: usize,
    pub returned_chars: usize,
    pub total_chars: usize,
    pub truncated: bool,
    pub next_offset: Option<usize>,
    pub content: String,
}

const MAX_ARTIFACTS_PER_RUN: usize = 64;
const MAX_ARTIFACT_BYTES: usize = 512 * 1024;
const ARTIFACT_BYTE_CAP_NOTICE: &str = "\n\n...[产物已达 512 KiB 内存上限，后续内容已截断]...";

fn clip_artifact_content(full_content: &str) -> (String, bool) {
    if full_content.len() > MAX_ARTIFACT_BYTES {
        let mut bounded: String = full_content
            .char_indices()
            .take_while(|(idx, _)| *idx < MAX_ARTIFACT_BYTES)
            .map(|(_, ch)| ch)
            .collect();
        bounded.push_str(ARTIFACT_BYTE_CAP_NOTICE);
        (bounded, true)
    } else {
        (full_content.to_string(), false)
    }
}

#[derive(Debug, Default)]
struct ArtifactStoreInner {
    next_seq: usize,
    entries: HashMap<String, RunArtifact>,
    insertion_order: Vec<String>,
}

/// 绑定到单次 Run 生命周期的内存 Artifact 仓库（绝不触碰任意磁盘路径）。
#[derive(Debug, Clone, Default)]
pub struct ArtifactStore {
    inner: Arc<Mutex<ArtifactStoreInner>>,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 存入完整工具输出，返回 `art_<n>` ID。超出单条体积或总数上限时安全裁剪最旧条目。
    pub fn store(&self, tool_name: &str, full_content: &str) -> Option<String> {
        let (bounded, truncated) = clip_artifact_content(full_content);
        self.store_prepared(tool_name, bounded, truncated)
    }

    fn store_prepared(&self, tool_name: &str, bounded: String, truncated: bool) -> Option<String> {
        let mut guard = self.inner.lock().ok()?;
        guard.next_seq += 1;
        let id = format!("art_{}", guard.next_seq);
        let total_chars = bounded.chars().count();

        if guard.insertion_order.len() >= MAX_ARTIFACTS_PER_RUN {
            if let Some(oldest_id) = guard.insertion_order.first().cloned() {
                guard.insertion_order.remove(0);
                guard.entries.remove(&oldest_id);
            }
        }
        guard.insertion_order.push(id.clone());
        guard.entries.insert(
            id.clone(),
            RunArtifact {
                id: id.clone(),
                tool_name: tool_name.to_string(),
                content: bounded,
                total_chars,
                truncated,
            },
        );
        Some(id)
    }

    /// 分页读取指定 Artifact 的内容。
    pub fn read_slice(
        &self,
        artifact_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<ArtifactSlice, String> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| "Artifact 存储锁不可用".to_string())?;
        let artifact = guard
            .entries
            .get(artifact_id.trim())
            .ok_or_else(|| format!("未找到运行期产物: {}", artifact_id.trim()))?;
        let clamped_limit = limit.clamp(200, 8_000);
        let (content, total_chars, next_offset) =
            slice_chars(&artifact.content, offset, clamped_limit);
        let returned_chars = content.chars().count();
        Ok(ArtifactSlice {
            artifact_id: artifact.id.clone(),
            tool_name: artifact.tool_name.clone(),
            offset,
            returned_chars,
            total_chars,
            truncated: artifact.truncated,
            next_offset,
            content,
        })
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.entries.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 将超长工具输出裁剪为头尾摘录，并把完整内容存入本轮 ArtifactStore。
    ///
    /// 返回 `(送入上下文的文本, Option<artifact_id>)`。
    pub fn truncate_tool_output(
        &self,
        tool_name: &str,
        content: &str,
        max_chars: usize,
    ) -> (String, Option<String>) {
        let (stored_content, truncated) = clip_artifact_content(content);
        let total_chars = stored_content.chars().count();
        if max_chars < 400 || total_chars <= max_chars {
            return (stored_content, None);
        }

        let head_budget = (max_chars * 70) / 100;
        let tail_budget = (max_chars * 20) / 100;
        let head: String = stored_content.chars().take(head_budget).collect();
        let tail: String = stored_content
            .chars()
            .skip(total_chars.saturating_sub(tail_budget))
            .collect();
        let omitted = total_chars.saturating_sub(head_budget + tail_budget);

        let Some(artifact_id) = self.store_prepared(tool_name, stored_content, truncated) else {
            let (clipped, _, _) = slice_chars(content, 0, max_chars);
            return (clipped, None);
        };

        let cap_note = if truncated {
            "（已按 512 KiB 上限截断）"
        } else {
            ""
        };
        let excerpt = format!(
            "{head}\n\n...[已截断 {omitted} 字；已存产物共 {total_chars} 字{cap_note}，产物 ID 为 {artifact_id}，如需查看更多细节请调用 read_run_artifact(artifact_id=\"{artifact_id}\", offset={head_budget}, limit=3000)]...\n\n{tail}"
        );
        (excerpt, Some(artifact_id))
    }
}

/// 供压缩规划使用的消息抽象（与具体模型 Message 结构解耦）。
#[derive(Debug, Clone)]
pub struct CompactionItem {
    pub role: String,
    pub text: String,
    pub is_tool_call: bool,
    pub is_tool_result: bool,
}

/// 上下文压缩方案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPlan {
    /// 需要从消息列表头部合并替换掉的消息条数。
    pub drop_prefix_count: usize,
    /// 合并生成的压缩摘要文本。
    pub summary_text: String,
    pub estimated_tokens_before: usize,
    pub estimated_tokens_after: usize,
}

/// 规划上下文确定性压缩。
///
/// 触发条件：估算总 token 数超过 `budget.compaction_threshold_tokens()` 且头部有足够可压缩消息。
/// 安全不变量：绝不把一条 `assistant (tool_call)` 与其对应的 `tool` 结果拆开。
pub fn plan_compaction(
    items: &[CompactionItem],
    system_prompt: &str,
    budget: &ContextBudget,
) -> Option<CompactionPlan> {
    let sys_tokens = estimate_tokens(system_prompt);
    let msg_tokens: usize = items.iter().map(|m| estimate_tokens(&m.text) + 6).sum();
    let total_tokens = sys_tokens + msg_tokens;
    let threshold = budget.compaction_threshold_tokens();

    if total_tokens <= threshold || items.len() <= budget.preserve_recent_messages + 1 {
        return None;
    }

    let mut split_idx = items.len().saturating_sub(budget.preserve_recent_messages);

    // 防止在 tool_call / tool_result 中间切开：若保留区首条是 tool_result，向前回退到对应的 tool_call 之前。
    while split_idx > 0 && items[split_idx].is_tool_result {
        split_idx -= 1;
        if split_idx > 0 && items[split_idx].is_tool_call {
            // 把成对的 assistant tool_call 也保留到尾部，避免尾部出现悬空 tool_result。
            break;
        }
    }
    // 若切点正好落在 tool_call 后一条且下一条不是 tool_result，正常；若切点本身是 tool_call，
    // 需确保它不会孤立在压缩区里而把结果留在保留区（上面已处理），同时若压缩区尾部是 tool_call
    // 但保留区首条不是 tool_result 也安全。

    if split_idx < 2 {
        return None;
    }

    let summary_text =
        build_deterministic_summary(&items[..split_idx], budget.max_compacted_summary_chars);
    let summary_tokens = estimate_tokens(&summary_text) + 6;
    let kept_tokens: usize = items[split_idx..]
        .iter()
        .map(|m| estimate_tokens(&m.text) + 6)
        .sum();
    let estimated_tokens_after = sys_tokens + summary_tokens + kept_tokens;

    if estimated_tokens_after >= total_tokens {
        return None;
    }

    Some(CompactionPlan {
        drop_prefix_count: split_idx,
        summary_text,
        estimated_tokens_before: total_tokens,
        estimated_tokens_after,
    })
}

fn build_deterministic_summary(prefix: &[CompactionItem], max_chars: usize) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "[早期上下文压缩摘要 — 已压缩前 {} 条消息以控制上下文预算]",
        prefix.len()
    ));

    let per_item_cap = (max_chars / prefix.len().max(1)).clamp(80, 260);
    let mut used_chars = lines[0].chars().count();

    for (idx, item) in prefix.iter().enumerate() {
        let role_tag = if item.is_tool_call {
            "助手调用工具"
        } else if item.is_tool_result {
            "工具结果"
        } else if item.role == "user" {
            "用户"
        } else {
            "助手"
        };
        let compact_text: String = item.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let (snippet, total, _) = slice_chars(&compact_text, 0, per_item_cap);
        let ellipsis = if total > per_item_cap { "…" } else { "" };
        let line = format!("{}. [{}] {}{}", idx + 1, role_tag, snippet, ellipsis);
        let line_len = line.chars().count() + 1;
        if used_chars + line_len > max_chars {
            lines.push(format!(
                "...（其余 {} 条早期记录已省略）",
                prefix.len() - idx
            ));
            break;
        }
        used_chars += line_len;
        lines.push(line);
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 超长工具输出应写入artifact并支持utf8安全分页读取() {
        let store = ArtifactStore::new();
        let big = "中文工作记录片段。".repeat(400); // 3600 chars
        let (excerpt, artifact_id) = store.truncate_tool_output("search_memory", &big, 800);
        let art_id = artifact_id.expect("超出阈值应生成 artifact_id");
        assert!(excerpt.contains(&art_id));
        assert!(excerpt.contains("read_run_artifact"));
        assert!(excerpt.chars().count() < big.chars().count());

        let page1 = store.read_slice(&art_id, 0, 500).expect("分页读取应成功");
        assert_eq!(page1.returned_chars, 500);
        assert_eq!(page1.next_offset, Some(500));
        assert_eq!(page1.total_chars, big.chars().count());
        assert!(!page1.truncated);
    }

    #[test]
    fn 超512kib输出写入artifact时应标记truncated且摘录尾部与分页一致() {
        let store = ArtifactStore::new();
        let huge = format!(
            "{}END_MARKER_BEYOND_CAP",
            "a".repeat(MAX_ARTIFACT_BYTES + 2048)
        );
        let (excerpt, artifact_id) = store.truncate_tool_output("search_memory", &huge, 800);
        let art_id = artifact_id.expect("应生成 artifact_id");
        assert!(excerpt.contains("512 KiB"));
        assert!(!excerpt.contains("END_MARKER_BEYOND_CAP"));

        let first = store.read_slice(&art_id, 0, 200).expect("首页读取应成功");
        assert!(first.truncated);
        assert!(excerpt.contains(&format!("共 {} 字", first.total_chars)));

        let last_offset = first.total_chars.saturating_sub(200);
        let tail_page = store
            .read_slice(&art_id, last_offset, 200)
            .expect("尾页读取应成功");
        assert!(tail_page.content.ends_with(ARTIFACT_BYTE_CAP_NOTICE));
        assert!(excerpt.ends_with(ARTIFACT_BYTE_CAP_NOTICE));
    }

    #[test]
    fn 压缩规划绝不拆散tool_call与tool_result配对() {
        let long_text = "这是一段用于触发上下文预算超限的长文本记录。".repeat(30);
        let items = vec![
            CompactionItem {
                role: "user".into(),
                text: long_text.clone(),
                is_tool_call: false,
                is_tool_result: false,
            },
            CompactionItem {
                role: "assistant".into(),
                text: long_text.clone(),
                is_tool_call: false,
                is_tool_result: false,
            },
            CompactionItem {
                role: "user".into(),
                text: long_text.clone(),
                is_tool_call: false,
                is_tool_result: false,
            },
            CompactionItem {
                role: "assistant".into(),
                text: "calling query_activities".into(),
                is_tool_call: true,
                is_tool_result: false,
            },
            CompactionItem {
                role: "tool".into(),
                text: long_text.clone(),
                is_tool_call: false,
                is_tool_result: true,
            },
            CompactionItem {
                role: "user".into(),
                text: "继续总结".into(),
                is_tool_call: false,
                is_tool_result: false,
            },
        ];

        let budget = ContextBudget {
            max_context_tokens: 600,
            compaction_trigger_ratio: 0.5,
            max_tool_result_chars: 1000,
            preserve_recent_messages: 2, // 默认切点为 4（正好是 tool_result），必须回退到 3（保留 tool_call）
            max_compacted_summary_chars: 600,
        };

        let plan = plan_compaction(&items, "system", &budget).expect("应触发压缩");
        assert_eq!(
            plan.drop_prefix_count, 3,
            "必须把成对的 tool_call 与 tool_result 一起留在保留区"
        );
        assert!(plan.estimated_tokens_after < plan.estimated_tokens_before);
        assert!(plan.summary_text.contains("早期上下文压缩摘要"));
    }
}
