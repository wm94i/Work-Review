//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::analysis::AppLocale;
use crate::config::{AiProvider, ModelConfig};
use crate::database::MemorySearchItem;
use crate::error::AppError;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

use super::shared::collect_privacy_filters;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantAnswer {
    pub answer: String,
    pub references: Vec<MemorySearchItem>,
    pub used_ai: bool,
    pub model_name: Option<String>,
    pub tool_labels: Vec<String>,
    pub cards: Vec<AssistantCard>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantCard {
    pub kind: String,
    pub title: String,
    pub content: serde_json::Value,
}

pub(crate) fn format_browser_url_for_display(raw_url: &str) -> String {
    let mut output = String::with_capacity(raw_url.len());
    let bytes = raw_url.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            output.push(bytes[index] as char);
            index += 1;
            continue;
        }

        let start = index;
        let mut decoded_bytes = Vec::new();

        while index + 2 < bytes.len() && bytes[index] == b'%' {
            let hex = &raw_url[index + 1..index + 3];
            let Ok(value) = u8::from_str_radix(hex, 16) else {
                break;
            };
            decoded_bytes.push(value);
            index += 3;
        }

        if decoded_bytes.is_empty() {
            output.push('%');
            index = start + 1;
            continue;
        }

        let raw_segment = &raw_url[start..index];
        if !decoded_bytes.iter().any(|byte| *byte >= 0x80) {
            output.push_str(raw_segment);
            continue;
        }

        match String::from_utf8(decoded_bytes) {
            Ok(decoded) => output.push_str(&decoded),
            Err(_) => output.push_str(raw_segment),
        }
    }

    output
}

fn assistant_empty_question_message(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::ZhCn => "请输入你想问的问题。",
        AppLocale::ZhTw => "請輸入你想問的問題。",
        AppLocale::En => "Please enter your question.",
    }
}

fn build_assistant_system_prompt(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::ZhCn => {
            "你是 Work Review 的工作助手。你可以回答任何问题。对于工作相关问题，你拥有工具可以查询用户的真实工作记录（活动时间线、统计、工作会话等），请优先使用工具获取准确数据后回答。对于非工作问题，直接用你的知识回答即可。请用与用户提问相同的语言回答，无论工作记录是什么语言（英文提问用英文，中文提问用中文）。先给结论再给依据，不要编造不存在的事实。"
        }
        AppLocale::ZhTw => {
            "你是 Work Review 的工作助手。你可以回答任何問題。對於工作相關問題，你擁有工具可以查詢使用者的真實工作記錄（活動時間線、統計、工作會話等），請優先使用工具獲取準確資料後回答。對於非工作問題，直接用你的知識回答即可。請用與使用者提問相同的語言回答，無論工作記錄是什麼語言（英文提問用英文，中文提問用中文）。先給結論再給依據，不要編造不存在的事實。"
        }
        AppLocale::En => {
            "You are the Work Review assistant. You can answer any question. For work-related questions, you have tools to query the user's actual work records (activity timeline, statistics, work sessions, etc.) — use them for accuracy. For non-work questions, answer directly from your knowledge. Respond in the same language as the user's question, regardless of the language of the work records (English question -> English answer, Chinese question -> Chinese answer). Lead with the conclusion, then support with evidence. Do not invent facts."
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum AssistantQuestionKind {
    StageSummary,
    OutcomeRecap,
    ProcessRecap,
    EvidenceQuery,
    TimeStat,
    Comparison,
    Listing,
    Freeform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantReasoningMode {
    Basic,
    AiEnhanced,
}

fn build_history_context(history: &[AssistantChatMessage]) -> String {
    history
        .iter()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|message| format!("{}: {}", message.role, message.content.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_short_follow_up_question(question: &str) -> bool {
    let trimmed = question.trim();
    let normalized = trimmed.to_lowercase();

    trimmed.chars().count() <= 18
        && [
            "继续",
            "展开",
            "细说",
            "详细",
            "具体",
            "接着",
            "那",
            "这个",
            "这里",
            "这个结论",
            "说说",
            "依据",
        ]
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

fn build_question_analysis_context(question: &str, history: &[AssistantChatMessage]) -> String {
    let trimmed = question.trim();
    if history.is_empty() {
        return trimmed.to_lowercase();
    }

    let should_expand = trimmed.chars().count() <= 18
        || [
            "这个",
            "这个结论",
            "这里",
            "这些",
            "它",
            "上面",
            "刚才",
            "继续",
            "展开",
            "依据",
        ]
        .iter()
        .any(|pattern| trimmed.contains(pattern));

    if !should_expand {
        return trimmed.to_lowercase();
    }

    let mut context = build_history_context(history);
    if !context.is_empty() {
        context.push('\n');
    }
    context.push_str(trimmed);
    context.to_lowercase()
}

fn detect_question_kind_from_text(text: &str) -> AssistantQuestionKind {
    let context = text.trim().to_lowercase();

    if context.is_empty() {
        return AssistantQuestionKind::StageSummary;
    }

    let time_stat_patterns = ["花了多少时间", "多少时间", "总时长", "时间分布", "时间占比"];
    if time_stat_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::TimeStat;
    }

    let comparison_patterns = ["对比", "比较", "和上周", "相比", "比上周", "变化", "差异"];
    if comparison_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::Comparison;
    }

    let listing_patterns = ["列出", "列举", "所有", "全部", "哪些", "清单"];
    if listing_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::Listing;
    }

    let evidence_patterns = [
        "依据",
        "证据",
        "怎么得出",
        "怎么判断",
        "为什么这么说",
        "哪些记录",
        "哪条记录",
        "从哪里看",
        "原文",
    ];
    if evidence_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::EvidenceQuery;
    }

    let process_patterns = [
        "过程",
        "怎么推进",
        "时间花在哪",
        "花在哪",
        "节奏",
        "session",
        "工作段",
        "时段",
        "时间线",
        "切换",
        "过程复盘",
    ];
    if process_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::ProcessRecap;
    }

    let outcome_patterns = [
        "结果",
        "产出",
        "完成了什么",
        "推进到哪",
        "进展",
        "交付",
        "没收口",
        "待办",
        "下一步",
        "后续",
        "风险",
        "阻塞",
    ];
    if outcome_patterns
        .iter()
        .any(|pattern| context.contains(pattern))
    {
        return AssistantQuestionKind::OutcomeRecap;
    }

    AssistantQuestionKind::StageSummary
}

fn last_user_question_kind(history: &[AssistantChatMessage]) -> Option<AssistantQuestionKind> {
    history
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .map(|message| detect_question_kind_from_text(&message.content))
}

fn infer_question_kind_from_assistant_reply(
    history: &[AssistantChatMessage],
) -> Option<AssistantQuestionKind> {
    let content = history
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && !message.content.trim().is_empty())
        .map(|message| message.content.trim().to_lowercase())?;

    let mut best_kind = AssistantQuestionKind::StageSummary;
    let mut best_score = 0i32;

    let candidates: Vec<(AssistantQuestionKind, &[&str])> = vec![
        (
            AssistantQuestionKind::EvidenceQuery,
            &[
                "## 依据补充",
                "依据",
                "记录",
                "原始记录",
                "证据",
                "哪条记录",
            ],
        ),
        (
            AssistantQuestionKind::ProcessRecap,
            &[
                "## 过程分析",
                "session",
                "工作段",
                "时间花在",
                "推进片段",
                "切换",
            ],
        ),
        (
            AssistantQuestionKind::OutcomeRecap,
            &["待办", "风险", "交付", "结果概览", "收口", "下一步"],
        ),
        (
            AssistantQuestionKind::StageSummary,
            &["结论", "主线", "阶段", "主要做了什么", "工作重心"],
        ),
        (
            AssistantQuestionKind::TimeStat,
            &["## 时间统计", "时长", "时间分布", "占比", "花了多少时间"],
        ),
        (
            AssistantQuestionKind::Comparison,
            &["## 对比分析", "对比", "比较", "变化", "差异", "相比"],
        ),
        (
            AssistantQuestionKind::Listing,
            &["## 清单", "列举", "列出", "所有", "全部", "清单"],
        ),
    ];

    for (kind, patterns) in candidates {
        let score = patterns
            .iter()
            .map(|pattern| {
                if content.contains(pattern) {
                    if pattern.starts_with("## ") {
                        3
                    } else {
                        1
                    }
                } else {
                    0
                }
            })
            .sum::<i32>();

        if score > best_score {
            best_score = score;
            best_kind = kind;
        }
    }

    if best_score > 0 {
        Some(best_kind)
    } else {
        None
    }
}

fn detect_assistant_question_kind_with_mode(
    question: &str,
    history: &[AssistantChatMessage],
    mode: AssistantReasoningMode,
) -> AssistantQuestionKind {
    let trimmed = question.trim();
    let current_kind = detect_question_kind_from_text(trimmed);

    if current_kind == AssistantQuestionKind::EvidenceQuery
        || current_kind == AssistantQuestionKind::TimeStat
        || current_kind == AssistantQuestionKind::Comparison
        || current_kind == AssistantQuestionKind::Listing
    {
        return current_kind;
    }

    if is_short_follow_up_question(trimmed) {
        if mode == AssistantReasoningMode::AiEnhanced {
            if let Some(assistant_kind) = infer_question_kind_from_assistant_reply(history) {
                if assistant_kind != AssistantQuestionKind::StageSummary {
                    return assistant_kind;
                }
            }
        }

        if let Some(previous_kind) = last_user_question_kind(history) {
            return previous_kind;
        }
    }

    let context = build_question_analysis_context(question, history);
    let contextual_kind = detect_question_kind_from_text(&context);
    if contextual_kind != AssistantQuestionKind::StageSummary {
        return contextual_kind;
    }

    current_kind
}

#[allow(dead_code)]
fn detect_assistant_question_kind(
    question: &str,
    history: &[AssistantChatMessage],
) -> AssistantQuestionKind {
    detect_assistant_question_kind_with_mode(question, history, AssistantReasoningMode::Basic)
}

#[allow(dead_code)]
fn push_markdown_section(answer: &mut String, title: &str, lines: Vec<String>, empty_text: &str) {
    if lines.is_empty() && empty_text.is_empty() {
        return;
    }

    answer.push_str(title);
    answer.push_str("\n\n");

    if lines.is_empty() {
        answer.push_str(empty_text);
        answer.push_str("\n\n");
        return;
    }

    for line in lines {
        if line.starts_with("- ") || line.starts_with("> ") {
            answer.push_str(&line);
        } else {
            answer.push_str("- ");
            answer.push_str(&line);
        }
        answer.push('\n');
    }
    answer.push('\n');
}

pub(crate) async fn generate_text_answer_with_model(
    model_config: &ModelConfig,
    system_prompt: &str,
    prompt: &str,
) -> Result<String, AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Unknown(e.to_string()))?;

    match model_config.provider {
        AiProvider::Ollama => {
            let ollama_base = model_config.endpoint.trim().trim_end_matches('/');
            let ollama_url = if ollama_base.ends_with("/api/chat") {
                ollama_base.to_string()
            } else {
                format!("{ollama_base}/api/chat")
            };
            let response = client
                .post(&ollama_url)
                .json(&serde_json::json!({
                    "model": model_config.model,
                    "messages": [
                        {
                            "role": "system",
                            "content": system_prompt
                        },
                        {
                            "role": "user",
                            "content": prompt
                        }
                    ],
                    "stream": false
                }))
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(AppError::Analysis(format!(
                    "Ollama 记忆问答失败: {}",
                    response.status()
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let answer = result["message"]["content"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                return Err(AppError::Analysis("Ollama 返回空内容".to_string()));
            }
            Ok(answer)
        }
        AiProvider::Claude => {
            let api_key = model_config.api_key.as_deref().unwrap_or("");
            if api_key.is_empty() {
                return Err(AppError::Analysis("Claude API Key 未配置".to_string()));
            }

            let claude_base = model_config.endpoint.trim().trim_end_matches('/');
            let claude_url = if claude_base.ends_with("/messages") {
                claude_base.to_string()
            } else {
                format!("{claude_base}/messages")
            };
            let response = client
                .post(&claude_url)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&serde_json::json!({
                    "model": model_config.model,
                    "max_tokens": 1600,
                    "system": system_prompt,
                    "messages": [
                        {
                            "role": "user",
                            "content": prompt
                        }
                    ]
                }))
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await.unwrap_or_default();
                return Err(AppError::Analysis(format!(
                    "Claude 记忆问答失败: {error_text}"
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let answer = result["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                return Err(AppError::Analysis("Claude 返回空内容".to_string()));
            }
            Ok(answer)
        }
        AiProvider::Gemini => {
            let api_key = model_config.api_key.as_deref().unwrap_or("");
            if api_key.is_empty() {
                return Err(AppError::Analysis("Gemini API Key 未配置".to_string()));
            }

            let gemini_base = model_config.endpoint.trim().trim_end_matches('/');
            let gemini_url = format!(
                "{}/models/{}:generateContent?key={}",
                gemini_base, model_config.model, api_key
            );
            let response = client
                .post(&gemini_url)
                .json(&serde_json::json!({
                    "contents": [{
                        "parts": [{
                            "text": format!("{}\n\n{}", system_prompt, prompt)
                        }]
                    }],
                    "generationConfig": {
                        "temperature": 0.2,
                        "maxOutputTokens": 1600
                    }
                }))
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await.unwrap_or_default();
                return Err(AppError::Analysis(format!(
                    "Gemini 记忆问答失败: {error_text}"
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let answer = result["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                return Err(AppError::Analysis("Gemini 返回空内容".to_string()));
            }
            Ok(answer)
        }
        _ => {
            let endpoint = model_config.endpoint.trim().trim_end_matches('/');
            let url = if endpoint.ends_with("/chat/completions") {
                endpoint.to_string()
            } else {
                format!("{endpoint}/chat/completions")
            };
            let mut request = client.post(&url).json(&serde_json::json!({
                "model": model_config.model,
                "messages": [
                    {
                        "role": "system",
                        "content": system_prompt
                    },
                    {
                        "role": "user",
                        "content": prompt
                    }
                ],
                "max_tokens": 1600,
                "temperature": 0.2
            }));

            if let Some(api_key) = &model_config.api_key {
                if !api_key.is_empty() {
                    request = request.header("Authorization", format!("Bearer {api_key}"));
                }
            }

            let response = request.send().await?;

            if !response.status().is_success() {
                let error_text = response.text().await.unwrap_or_default();
                return Err(AppError::Analysis(format!(
                    "OpenAI 兼容记忆问答失败: {error_text}"
                )));
            }

            let result: serde_json::Value = response.json().await?;
            let answer = result["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                return Err(AppError::Analysis("模型返回空内容".to_string()));
            }
            Ok(answer)
        }
    }
}

/// 统一工作助手（Stage 6: 已接入 Agent Orchestrator）
///
/// 接口签名保持不变，内部实现替换为 Agentic 架构：
/// - 简单查询 → FastPath（规则 + 模板）
/// - 复杂查询 → AgentPath（LLM 自主决策 + 多轮工具调用）
/// - 无模型   → FallbackPath（纯模板回答）
#[tauri::command]
#[allow(unused_variables)] // date_from/date_to 为接口预留，Agent 当前从问题自行推断时间范围
pub async fn chat_work_assistant(
    question: String,
    history: Option<Vec<AssistantChatMessage>>,
    model_config: Option<ModelConfig>,
    locale: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    on_event: tauri::ipc::Channel<crate::agent::StreamEvent>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<AssistantAnswer, AppError> {
    let trimmed_question = question.trim().to_string();
    let history = history.unwrap_or_default();
    let assistant_locale = AppLocale::from_option(locale.as_deref());

    if trimmed_question.is_empty() {
        let answer = assistant_empty_question_message(assistant_locale).to_string();
        let tool_labels = vec!["记忆检索".to_string()];
        // 空问题也推一个 Done，保持事件流完整（前端可统一收尾）。
        let _ = on_event.send(crate::agent::StreamEvent::Done {
            answer: answer.clone(),
            references: vec![],
            tool_labels: tool_labels.clone(),
        });
        return Ok(AssistantAnswer {
            answer,
            references: Vec::new(),
            used_ai: false,
            model_name: None,
            tool_labels,
            cards: Vec::new(),
        });
    }

    // 将前端历史转为 Agent 内部的 Message 格式（保留 role）
    let agent_history: Vec<crate::agent::Message> = history
        .iter()
        .map(|m| {
            if m.role == "assistant" {
                crate::agent::Message::assistant(&m.content)
            } else {
                crate::agent::Message::user(&m.content)
            }
        })
        .collect();

    // 从 AppState 中 clone Database + 收集隐私过滤器（Arc 引用计数 +1，可跨 await）
    let (database, ignored_apps, excluded_domains) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        let (ignored_apps, excluded_domains) = collect_privacy_filters(&s);
        (s.database.clone(), ignored_apps, excluded_domains)
    };

    // 流式桥接：agent 用 mpsc 推事件，这里转发到 Tauri ipc::Channel（前端 onmessage 收）。
    // Channel::send 失败即前端已销毁，停止转发。
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::agent::StreamEvent>(64);
    let on_event_clone = on_event.clone();
    let bridge = tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if on_event_clone.send(ev).is_err() {
                break;
            }
        }
    });

    // Stage 6: 完整 Orchestrator 集成
    // 使用 locale 感知的系统提示词，确保繁体/英文用户得到对应语言的回答
    let system_prompt = build_assistant_system_prompt(assistant_locale);
    let result = crate::agent::Orchestrator::handle(
        &trimmed_question,
        model_config.as_ref(),
        &database,
        &agent_history,
        Some(system_prompt),
        &ignored_apps,
        &excluded_domains,
        Some(tx),
    )
    .await;

    // 等桥接任务把剩余事件发完（tx 在 handle 内 drop 后 rx.recv() 返回 None）。
    let _ = bridge.await;

    let result = match result {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            let _ = on_event.send(crate::agent::StreamEvent::Error(msg));
            return Err(e);
        }
    };

    Ok(AssistantAnswer {
        answer: result.answer,
        references: result.references,
        used_ai: result.used_ai,
        model_name: model_config.map(|c| c.model.clone()),
        tool_labels: result.tool_labels,
        cards: Vec::new(),
    })
}

/// 用指定模型生成一段文本（单轮，非 agent 循环）。用于 starter prompt 动态生成等轻量场景。
#[tauri::command]
pub async fn generate_text_with_model(
    model_config: ModelConfig,
    system_prompt: String,
    prompt: String,
) -> Result<String, AppError> {
    generate_text_answer_with_model(&model_config, &system_prompt, &prompt).await
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 应将命令输出中的_url_格式化为可读文本() {
        assert_eq!(
            format_browser_url_for_display(
                "https://www.google.com.hk/search?q=%E5%A4%A7%E6%B8%A1%E5%8F%A3&client=firefox-b-d"
            ),
            "https://www.google.com.hk/search?q=大渡口&client=firefox-b-d"
        );
        assert_eq!(
            format_browser_url_for_display(
                "https://example.com/search?q=a%26b&name=%E5%BC%A0%E4%B8%89"
            ),
            "https://example.com/search?q=a%26b&name=张三"
        );
    }

    fn sample_process_follow_up_history() -> Vec<AssistantChatMessage> {
        vec![
            AssistantChatMessage {
                role: "user".to_string(),
                content: "最近时间主要花在哪？".to_string(),
            },
            AssistantChatMessage {
                role: "assistant".to_string(),
                content: "## 结论\n\n- 这段时间更像是围绕少数主题持续推进。\n\n## 过程分析\n\n- 主要是编码开发相关 session。\n".to_string(),
            },
        ]
    }

    fn sample_stage_follow_up_history() -> Vec<AssistantChatMessage> {
        vec![
            AssistantChatMessage {
                role: "user".to_string(),
                content: "这周主要做了什么？".to_string(),
            },
            AssistantChatMessage {
                role: "assistant".to_string(),
                content: "## 结论\n\n- 这周主线是助手回答链路改造。\n".to_string(),
            },
        ]
    }

    #[test]
    fn 助手问题分类应识别阶段总结与过程复盘和证据追问() {
        assert_eq!(
            detect_assistant_question_kind("这周主要做了什么？", &[]),
            AssistantQuestionKind::StageSummary
        );
        assert_eq!(
            detect_assistant_question_kind("最近时间主要花在哪？", &[]),
            AssistantQuestionKind::ProcessRecap
        );
        assert_eq!(
            detect_assistant_question_kind("这个结论的依据是什么？", &[]),
            AssistantQuestionKind::EvidenceQuery
        );
    }

    #[test]
    fn 助手问题分类应继承上一轮过程复盘语境() {
        let history = sample_process_follow_up_history();

        assert_eq!(
            detect_assistant_question_kind("继续", &history),
            AssistantQuestionKind::ProcessRecap
        );
        assert_eq!(
            detect_assistant_question_kind("展开说说这个", &history),
            AssistantQuestionKind::ProcessRecap
        );
    }

    #[test]
    fn 助手问题分类应将依据追问优先识别为证据问题() {
        let history = sample_stage_follow_up_history();

        assert_eq!(
            detect_assistant_question_kind("那依据呢", &history),
            AssistantQuestionKind::EvidenceQuery
        );
        assert_eq!(
            detect_assistant_question_kind("这个结论怎么得出的", &history),
            AssistantQuestionKind::EvidenceQuery
        );
    }

    #[test]
    fn ai增强识别器应比基础模板更强承接助手上下文() {
        let history = vec![
            AssistantChatMessage {
                role: "user".to_string(),
                content: "这周主要做了什么？".to_string(),
            },
            AssistantChatMessage {
                role: "assistant".to_string(),
                content: "## 结论\n\n- 这周主线是助手回答链路改造。\n\n## 过程分析\n\n- 主要是编码开发相关 session。\n".to_string(),
            },
        ];

        assert_eq!(
            detect_assistant_question_kind_with_mode(
                "展开说说这个",
                &history,
                AssistantReasoningMode::Basic
            ),
            AssistantQuestionKind::StageSummary
        );
        assert_eq!(
            detect_assistant_question_kind_with_mode(
                "展开说说这个",
                &history,
                AssistantReasoningMode::AiEnhanced
            ),
            AssistantQuestionKind::ProcessRecap
        );
    }

}
