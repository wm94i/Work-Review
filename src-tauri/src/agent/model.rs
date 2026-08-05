//! Stage 2: Model 层 — Agent 的"嘴巴"
//!
//! 职责：把统一的消息格式翻译成各家 API 的请求格式，
//!       把各家 API 的响应翻译回统一格式。
//!
//! 对应 Python: 02_model.py 里的 Message/ToolCall/LlmResponse/Provider

use crate::config::{AiProvider, ModelConfig};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 非流式共享 HTTP 客户端（连接 10s 超时；整体超时按调用方配置的预算逐请求设置）。
/// 进程内复用连接池，避免每次模型调用都重建客户端与 TLS 连接。
static CHAT_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
/// 流式共享 HTTP 客户端：只限连接超时，读取由逐块 idle + 总时长双护栏控制。
static STREAM_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn chat_client() -> Result<&'static reqwest::Client, AppError> {
    if let Some(client) = CHAT_CLIENT.get() {
        return Ok(client);
    }
    let built = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(CHAT_CLIENT.get_or_init(|| built))
}

fn stream_client() -> Result<&'static reqwest::Client, AppError> {
    if let Some(client) = STREAM_CLIENT.get() {
        return Ok(client);
    }
    let built = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Unknown(e.to_string()))?;
    Ok(STREAM_CLIENT.get_or_init(|| built))
}


/// 模型调用超时预算（由用户配置的"助手回答超时"派生）。
/// 单次模型调用不允许超过用户的整体耐心；多轮总预算由 executor 墙钟控制。
#[derive(Debug, Clone, Copy)]
pub struct ModelTimeouts {
    /// 单次非流式请求整体超时
    pub request_total: Duration,
    /// 单次流式响应总时长上限
    pub stream_total: Duration,
    /// 流式逐块空闲超时（连接死了要快速失败，与总预算无关）
    pub stream_idle: Duration,
    /// Agent 循环总墙钟预算（超预算走收束路径而非硬超时）
    pub loop_wall_clock: Duration,
}

impl Default for ModelTimeouts {
    fn default() -> Self {
        Self {
            request_total: Duration::from_secs(60),
            stream_total: Duration::from_secs(120),
            stream_idle: Duration::from_secs(30),
            loop_wall_clock: Duration::from_secs(120),
        }
    }
}

impl ModelTimeouts {
    /// 从用户配置的助手回答超时（秒）派生：非流式与流式总时长都用整体预算，
    /// 空闲超时固定 30s，Agent 循环墙钟与整体预算一致。
    pub fn from_assistant_timeout_secs(secs: u64) -> Self {
        Self {
            request_total: Duration::from_secs(secs),
            stream_total: Duration::from_secs(secs),
            stream_idle: Duration::from_secs(30),
            loop_wall_clock: Duration::from_secs(secs),
        }
    }
}
/// 非流式请求发送：命中 429/5xx 时等待 2 秒重试一次（流式路径不重试）。
async fn send_with_retry(request: reqwest::RequestBuilder) -> Result<reqwest::Response, AppError> {
    let retry = request.try_clone();
    let response = request.send().await?;
    let status = response.status();
    if status.as_u16() == 429 || status.is_server_error() {
        if let Some(retry_request) = retry {
            log::warn!("模型请求返回 {status}，2 秒后重试一次");
            tokio::time::sleep(Duration::from_secs(2)).await;
            return Ok(retry_request.send().await?);
        }
    }
    Ok(response)
}

// ══════════════════════════════════════════════════════════
// 第一部分：统一的消息格式
// ══════════════════════════════════════════════════════════
// 对应 Python: class Message / ToolCall / LlmResponse

/// LLM 想调用的工具
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// 停止原因
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    Stop,
    ToolCall,
    MaxTokens,
}

/// LLM 的统一响应 — 不管底层是什么提供商
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub stop_reason: StopReason,
}

/// 统一的消息格式 — Agent 内部只用这个
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 工具名称（仅 tool role 消息使用，Gemini 需要）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tool_calls(calls: &[ToolCall]) -> Self {
        let tool_calls_json: Vec<Value> = calls
            .iter()
            .map(|tc| {
                json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments.to_string()
                    }
                })
            })
            .collect();
        Self {
            role: "assistant".into(),
            content: None,
            tool_calls: Some(Value::Array(tool_calls_json)),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result_named(tool_call_id: &str, content: &str, name: Option<&str>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: name.map(|n| n.to_string()),
        }
    }
}

// ══════════════════════════════════════════════════════════
// 第二部分：chat 函数 — 统一的 LLM 调用入口
// ══════════════════════════════════════════════════════════

/// 统一的 LLM 调用函数（支持 tool-calling）
///
/// 这是你现有的 `generate_text_answer_with_model` 的升级版：
/// - 旧版：只能发 system + user，收纯文字
/// - 新版：支持发 messages + tools，收文字或 tool_calls
///
/// 对应 Python: provider.chat(messages, tools) -> LlmResponse
pub async fn chat_with_tools(
    model_config: &ModelConfig,
    system_prompt: &str,
    messages: &[Message],
    tools: &[Value],
    timeouts: ModelTimeouts,
) -> Result<LlmResponse, AppError> {
    let client = chat_client()?;

    // 构造完整的 messages 数组：system + 用户对话历史
    let mut full_messages = vec![json!({
        "role": "system",
        "content": system_prompt
    })];
    for msg in messages {
        full_messages.push(serde_json::to_value(msg).unwrap_or_default());
    }

    // 根据提供商分发
    match model_config.provider {
        AiProvider::Ollama => {
            chat_ollama(client, model_config, &full_messages, tools, timeouts).await
        }
        AiProvider::Claude => {
            chat_claude(client, model_config, &full_messages, tools, timeouts).await
        }
        AiProvider::Gemini => {
            chat_gemini(client, model_config, &full_messages, tools, timeouts).await
        }
        _ => chat_openai_compatible(client, model_config, &full_messages, tools, timeouts).await,
    }
}
// ══════════════════════════════════════════════════════════
// 第三部分：各家 Provider 的实现 — 格式翻译
// ══════════════════════════════════════════════════════════

/// OpenAI 兼容格式（覆盖 8 个提供商：OpenAI/SiliconFlow/DeepSeek/Qwen/Zhipu/Moonshot/Doubao/MiniMax）
///
/// 面试要点：这些提供商都用相同的 API 格式，所以一个实现覆盖全部。
async fn chat_openai_compatible(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    timeouts: ModelTimeouts,
) -> Result<LlmResponse, AppError> {
    let endpoint = model_config.endpoint.trim().trim_end_matches('/');
    let url = if endpoint.ends_with("/chat/completions") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/chat/completions")
    };

    let mut body = json!({
        "model": model_config.model,
        "messages": messages,
        "max_tokens": 1600,
        "temperature": 0.2
    });

    // 只有提供了工具定义时才加 tools 参数
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }

    let mut request = client.post(&url).json(&body).timeout(timeouts.request_total);
    if let Some(api_key) = &model_config.api_key {
        if !api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }
    }

    let response = send_with_retry(request).await?;
    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(AppError::Analysis(format!("LLM 调用失败: {error_text}")));
    }

    let result: Value = response.json().await?;
    parse_openai_response(&result)
}

/// Ollama 格式
async fn chat_ollama(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    timeouts: ModelTimeouts,
) -> Result<LlmResponse, AppError> {
    let ollama_base = model_config.endpoint.trim().trim_end_matches('/');
    let url = if ollama_base.ends_with("/api/chat") {
        ollama_base.to_string()
    } else {
        format!("{ollama_base}/api/chat")
    };

    let mut body = json!({
        "model": model_config.model,
        "messages": messages,
        "stream": false
    });

    // Ollama 也支持 tools 参数（模型支持的话）
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }

    let response =
        send_with_retry(client.post(&url).json(&body).timeout(timeouts.request_total)).await?;
    if !response.status().is_success() {
        return Err(AppError::Analysis(format!(
            "Ollama 调用失败: {}",
            response.status()
        )));
    }

    let result: Value = response.json().await?;
    // Ollama 的响应格式和 OpenAI 类似
    parse_openai_response(&result)
}

/// Claude 请求格式转换：统一 messages → (claude_messages, system, claude_tools)。
/// 流式与非流式共用，保证两条路径的格式永远一致。
fn build_claude_request_parts(
    messages: &[Value],
    tools: &[Value],
) -> (Vec<Value>, String, Vec<Value>) {
    // Claude 的消息格式：去掉 system（放在顶层），转换 tool 消息格式
    let claude_messages: Vec<Value> = messages
        .iter()
        .filter(|m| m["role"].as_str() != Some("system"))
        .map(|m| {
            match m["role"].as_str() {
                // assistant + tool_calls → Claude content blocks with tool_use
                Some("assistant") if m["tool_calls"].is_array() => {
                    let mut content_blocks: Vec<Value> = vec![];
                    // 如果有文字内容，先加文字 block
                    if let Some(text) = m["content"].as_str() {
                        if !text.is_empty() {
                            content_blocks.push(json!({"type": "text", "text": text}));
                        }
                    }
                    // 加 tool_use blocks
                    if let Some(calls) = m["tool_calls"].as_array() {
                        for call in calls {
                            content_blocks.push(json!({
                                "type": "tool_use",
                                "id": call["id"],
                                "name": call["function"]["name"],
                                "input": serde_json::from_str::<Value>(
                                    call["function"]["arguments"].as_str().unwrap_or("{}")
                                ).unwrap_or(json!({}))
                            }));
                        }
                    }
                    json!({"role": "assistant", "content": content_blocks})
                }
                // tool result → Claude user message with tool_result content block
                Some("tool") => {
                    json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": m["tool_call_id"],
                            "content": m["content"]
                        }]
                    })
                }
                // user / plain assistant → 直接传
                _ => m.clone(),
            }
        })
        .collect();

    let system_content = messages
        .iter()
        .find(|m| m["role"].as_str() == Some("system"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or("")
        .to_string();

    // Claude 的工具定义格式不同：用 input_schema 而不是 parameters
    let claude_tools: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t["function"]["name"],
                "description": t["function"]["description"],
                "input_schema": t["function"]["parameters"]
            })
        })
        .collect();

    (claude_messages, system_content, claude_tools)
}

/// Claude (Anthropic) 格式
async fn chat_claude(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    timeouts: ModelTimeouts,
) -> Result<LlmResponse, AppError> {
    let api_key = model_config
        .api_key
        .as_deref()
        .ok_or_else(|| AppError::Analysis("Claude 需要 API Key，请在设置中配置".to_string()))?;

    let endpoint = model_config.endpoint.trim().trim_end_matches('/');
    let url = if endpoint.ends_with("/messages") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/messages")
    };

    let (claude_messages, system_content, claude_tools) =
        build_claude_request_parts(messages, tools);

    let mut body = json!({
        "model": model_config.model,
        "max_tokens": 1600,
        "system": system_content,
        "messages": claude_messages,
    });
    if !claude_tools.is_empty() {
        body["tools"] = json!(claude_tools);
    }

    let response = send_with_retry(
        client
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("x-api-key", api_key)
            .json(&body)
            .timeout(timeouts.request_total),
    )
    .await?;

    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(AppError::Analysis(format!("Claude 调用失败: {error_text}")));
    }

    let result: Value = response.json().await?;
    parse_claude_response(&result)
}

/// Gemini 请求体构造：统一 messages → contents + systemInstruction + tools。
/// 流式与非流式共用，保证两条路径的格式永远一致。
fn build_gemini_request_body(messages: &[Value], tools: &[Value]) -> Value {
    // Gemini 格式：contents + systemInstruction + tools
    let mut contents = vec![];
    let mut system_instruction = None;

    for msg in messages {
        match msg["role"].as_str() {
            Some("system") => {
                system_instruction = Some(json!({"parts": [{"text": msg["content"]}] }));
            }
            Some("user") => {
                contents.push(json!({
                    "role": "user",
                    "parts": [{"text": msg["content"]}]
                }));
            }
            Some("assistant") if msg["tool_calls"].is_array() => {
                // assistant + tool_calls → Gemini functionCall parts
                let mut parts: Vec<Value> = vec![];
                if let Some(text) = msg["content"].as_str() {
                    if !text.is_empty() {
                        parts.push(json!({"text": text}));
                    }
                }
                if let Some(calls) = msg["tool_calls"].as_array() {
                    for call in calls {
                        let args: Value = serde_json::from_str(
                            call["function"]["arguments"].as_str().unwrap_or("{}"),
                        )
                        .unwrap_or(json!({}));
                        parts.push(json!({
                            "functionCall": {
                                "name": call["function"]["name"],
                                "args": args
                            }
                        }));
                    }
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }
            Some("assistant") => {
                contents.push(json!({
                    "role": "model",
                    "parts": [{"text": msg["content"].as_str().unwrap_or("")}]
                }));
            }
            Some("tool") => {
                // tool result → Gemini functionResponse
                let fn_name = msg
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                contents.push(json!({
                    "role": "function",
                    "parts": [{
                        "functionResponse": {
                            "name": fn_name,
                            "response": {
                                "result": msg["content"]
                            }
                        }
                    }]
                }));
            }
            _ => {}
        }
    }

    // Gemini 的工具定义格式：functionDeclarations
    let gemini_tools: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t["function"]["name"],
                "description": t["function"]["description"],
                "parameters": t["function"]["parameters"]
            })
        })
        .collect();

    let mut body = json!({
        "contents": contents,
    });
    if let Some(sys) = system_instruction {
        body["systemInstruction"] = sys;
    }
    if !gemini_tools.is_empty() {
        body["tools"] = json!([{"function_declarations": gemini_tools}]);
    }
    body
}

/// Gemini 格式
async fn chat_gemini(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    timeouts: ModelTimeouts,
) -> Result<LlmResponse, AppError> {
    let endpoint = model_config.endpoint.trim().trim_end_matches('/');
    let api_key = model_config
        .api_key
        .as_deref()
        .ok_or_else(|| AppError::Analysis("Gemini 需要 API Key，请在设置中配置".to_string()))?;
    let url = format!("{endpoint}/models/{}:generateContent", model_config.model);

    let body = build_gemini_request_body(messages, tools);

    let response = send_with_retry(
        client
            .post(&url)
            .header("x-goog-api-key", api_key)
            .json(&body)
            .timeout(timeouts.request_total),
    )
    .await?;
    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(AppError::Analysis(format!(
            "Gemini 调用失败: {}",
            error_text.chars().take(300).collect::<String>()
        )));
    }

    let result: Value = response.json().await?;
    parse_gemini_response(&result)
}

// ══════════════════════════════════════════════════════════
// 第四部分：响应解析 — 各家格式 → 统一格式
// ══════════════════════════════════════════════════════════

/// 解析 OpenAI 格式的响应（Ollama 也用这个）
fn parse_openai_response(result: &Value) -> Result<LlmResponse, AppError> {
    let choice = &result["choices"][0];
    let msg = &choice["message"];

    // 解析 tool_calls
    let tool_calls = if let Some(tcs) = msg["tool_calls"].as_array() {
        let parsed: Vec<ToolCall> = tcs
            .iter()
            .filter_map(|tc| {
                let id = tc["id"].as_str()?.to_string();
                let name = tc["function"]["name"].as_str()?.to_string();
                let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                let arguments = serde_json::from_str(args_str).unwrap_or(json!({}));
                Some(ToolCall {
                    id,
                    name,
                    arguments,
                })
            })
            .collect();
        if parsed.is_empty() {
            None
        } else {
            Some(parsed)
        }
    } else {
        None
    };

    // 判断 stop_reason
    let stop_reason = match choice["finish_reason"].as_str() {
        Some("tool_calls") => StopReason::ToolCall,
        Some("length") => StopReason::MaxTokens,
        _ => {
            if tool_calls.is_some() {
                StopReason::ToolCall
            } else {
                StopReason::Stop
            }
        }
    };

    let content = msg["content"].as_str().map(|s| s.to_string());

    Ok(LlmResponse {
        content,
        tool_calls,
        stop_reason,
    })
}

/// 解析 Claude 格式的响应
fn parse_claude_response(result: &Value) -> Result<LlmResponse, AppError> {
    let content_blocks = result["content"].as_array();

    let mut text_content = String::new();
    let mut tool_calls = Vec::new();

    if let Some(blocks) = content_blocks {
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(t) = block["text"].as_str() {
                        text_content.push_str(t);
                    }
                }
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        arguments: block["input"].clone(),
                        //            ↑ Claude 的参数已经是 object，不需要 JSON.parse
                    });
                }
                _ => {}
            }
        }
    }

    let stop_reason = match result["stop_reason"].as_str() {
        Some("tool_use") => StopReason::ToolCall,
        Some("max_tokens") => StopReason::MaxTokens,
        _ => {
            if !tool_calls.is_empty() {
                StopReason::ToolCall
            } else {
                StopReason::Stop
            }
        }
    };

    Ok(LlmResponse {
        content: if text_content.is_empty() {
            None
        } else {
            Some(text_content)
        },
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        stop_reason,
    })
}

/// 解析 Gemini 格式的响应
fn parse_gemini_response(result: &Value) -> Result<LlmResponse, AppError> {
    let parts = result["candidates"][0]["content"]["parts"].as_array();

    let mut text_content = String::new();
    let mut tool_calls = Vec::new();

    if let Some(parts_arr) = parts {
        for part in parts_arr {
            if let Some(text) = part["text"].as_str() {
                text_content.push_str(text);
            }
            if let Some(fc) = part.get("functionCall") {
                tool_calls.push(ToolCall {
                    id: format!("gemini_{}", tool_calls.len()),
                    name: fc["name"].as_str().unwrap_or("").to_string(),
                    arguments: fc["args"].clone(),
                });
            }
        }
    }

    let stop_reason = if !tool_calls.is_empty() {
        StopReason::ToolCall
    } else {
        StopReason::Stop
    };

    Ok(LlmResponse {
        content: if text_content.is_empty() {
            None
        } else {
            Some(text_content)
        },
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        stop_reason,
    })
}

// ══════════════════════════════════════════════════════════
// 第四部分：Token 流式 — chat_with_tools 的流式版本
// ══════════════════════════════════════════════════════════
// 设计：
// - 每个 provider 一个"纯装配器"（逐条喂 SSE/NDJSON payload，可单测无 HTTP）
//   + 一个 HTTP 驱动（chunk 循环 + 行缓冲 + 喂装配器 + 文本增量回调）。
// - 入口 chat_with_tools_streaming 与 chat_with_tools 同参，多一个 on_text 回调；
//   流式路径任何失败都回退到既有非流式实现，保证不比旧行为差。
// - 超时策略：流式不用整体请求超时（长答案会被掐断），改为逐块空闲
//   + 总时长双护栏，两者均来自用户配置的助手回答超时（ModelTimeouts）。

/// 文本增量回调。executor 层把增量批量合并成 StreamEvent::Token 推给前端。
pub type OnTextDelta<'a> = &'a mut (dyn FnMut(&str) + Send);

/// 统一的流式 LLM 调用入口。语义与 `chat_with_tools` 完全一致（返回完整
/// LlmResponse，含 tool_calls / stop_reason），额外通过 `on_text` 实时吐出
/// 文本增量。流式路径失败时自动回退非流式（此时不再产生增量，答案随
/// 返回值一次性给出，前端由 Done 事件兜底）。
pub async fn chat_with_tools_streaming(
    model_config: &ModelConfig,
    system_prompt: &str,
    messages: &[Message],
    tools: &[Value],
    timeouts: ModelTimeouts,
    on_text: OnTextDelta<'_>,
) -> Result<LlmResponse, AppError> {
    // 流式客户端：只限制连接超时，读取超时由逐块 idle 超时控制。
    let client = stream_client()?;

    let mut full_messages = vec![json!({
        "role": "system",
        "content": system_prompt
    })];
    for msg in messages {
        full_messages.push(serde_json::to_value(msg).unwrap_or_default());
    }

    let streamed = match model_config.provider {
        AiProvider::Ollama => {
            chat_ollama_streaming(client, model_config, &full_messages, tools, timeouts, on_text)
                .await
        }
        AiProvider::Claude => {
            chat_claude_streaming(client, model_config, &full_messages, tools, timeouts, on_text)
                .await
        }
        AiProvider::Gemini => {
            chat_gemini_streaming(client, model_config, &full_messages, tools, timeouts, on_text)
                .await
        }
        _ => {
            chat_openai_compatible_streaming(
                client,
                model_config,
                &full_messages,
                tools,
                timeouts,
                on_text,
            )
            .await
        }
    };

    match streamed {
        Ok(response) => Ok(response),
        Err(e) => {
            log::warn!("流式调用失败，回退非流式: {e}");
            chat_with_tools(model_config, system_prompt, messages, tools, timeouts).await
        }
    }
}

/// 行缓冲：把任意切分的字节块拼成完整行，残行留在缓冲区等下一个 chunk。
struct LineBuffer {
    buf: String,
}

impl LineBuffer {
    fn new() -> Self {
        Self { buf: String::new() }
    }

    /// 喂入一个 chunk，返回其中所有完整行（去掉行尾 \r\n）。
    fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=pos).collect();
            lines.push(line.trim_end_matches(['\n', '\r']).to_string());
        }
        lines
    }
}

/// 从 SSE 行提取 data payload：`data: {...}` → `{...}`；非 data 行返回 None。
fn sse_data_payload(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(rest.trim_start())
}

/// 通用流式驱动：chunk 循环 + 行缓冲 + 逐行回调。
/// `on_line` 返回 true 表示流已到终态（如 OpenAI 的 [DONE]），提前结束。
async fn drive_stream(
    response: reqwest::Response,
    timeouts: ModelTimeouts,
    mut on_line: impl FnMut(&str) -> bool,
) -> Result<(), AppError> {
    let mut response = response;
    let mut line_buf = LineBuffer::new();
    let started = Instant::now();

    loop {
        if started.elapsed() > timeouts.stream_total {
            return Err(AppError::Analysis("流式响应总时长超限".to_string()));
        }
        let chunk = tokio::time::timeout(timeouts.stream_idle, response.chunk())
            .await
            .map_err(|_| AppError::Analysis("流式响应空闲超时".to_string()))?
            .map_err(|e| AppError::Analysis(format!("流式读取失败: {e}")))?;

        let Some(bytes) = chunk else {
            return Ok(()); // 流正常结束
        };
        let text = String::from_utf8_lossy(&bytes);
        for line in line_buf.push(&text) {
            if line.is_empty() {
                continue;
            }
            if on_line(&line) {
                return Ok(());
            }
        }
    }
}

/// 校验流式响应状态码，非 2xx 时读取 body 报错（触发上层回退）。
async fn ensure_stream_status(
    response: reqwest::Response,
    provider_label: &str,
) -> Result<reqwest::Response, AppError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(AppError::Analysis(format!(
        "{provider_label} 流式调用失败 ({status}): {}",
        body.chars().take(300).collect::<String>()
    )))
}

// ── OpenAI 兼容流式 ──────────────────────────────────────────

/// OpenAI SSE delta 装配器。
/// 增量结构：choices[0].delta.content（文本）/ delta.tool_calls[]（按 index 分片）。
#[derive(Default)]
struct OpenAiStreamAssembler {
    content: String,
    finish_reason: Option<String>,
    /// index 对齐的 (id, name, arguments_json 分片累积)
    partial_calls: Vec<(String, String, String)>,
}

impl OpenAiStreamAssembler {
    /// 喂入一条 data payload；返回本条携带的文本增量。
    fn ingest(&mut self, payload: &Value) -> Option<String> {
        let choice = &payload["choices"][0];
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        let delta = &choice["delta"];
        if let Some(tcs) = delta["tool_calls"].as_array() {
            for tc in tcs {
                let idx = tc["index"].as_u64().unwrap_or(self.partial_calls.len() as u64) as usize;
                while self.partial_calls.len() <= idx {
                    self.partial_calls
                        .push((String::new(), String::new(), String::new()));
                }
                let slot = &mut self.partial_calls[idx];
                if let Some(id) = tc["id"].as_str() {
                    slot.0.push_str(id);
                }
                if let Some(name) = tc["function"]["name"].as_str() {
                    slot.1.push_str(name);
                }
                if let Some(args) = tc["function"]["arguments"].as_str() {
                    slot.2.push_str(args);
                }
            }
        }
        let text = delta["content"].as_str()?;
        if text.is_empty() {
            return None;
        }
        self.content.push_str(text);
        Some(text.to_string())
    }

    fn finish(self) -> LlmResponse {
        let tool_calls: Vec<ToolCall> = self
            .partial_calls
            .into_iter()
            .enumerate()
            .filter(|(_, (_, name, _))| !name.is_empty())
            .map(|(i, (id, name, args))| ToolCall {
                id: if id.is_empty() {
                    format!("stream_call_{i}")
                } else {
                    id
                },
                name,
                arguments: serde_json::from_str(&args).unwrap_or(json!({})),
            })
            .collect();
        let tool_calls = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };

        let stop_reason = match self.finish_reason.as_deref() {
            Some("tool_calls") => StopReason::ToolCall,
            Some("length") => StopReason::MaxTokens,
            _ => {
                if tool_calls.is_some() {
                    StopReason::ToolCall
                } else {
                    StopReason::Stop
                }
            }
        };

        LlmResponse {
            content: if self.content.is_empty() {
                None
            } else {
                Some(self.content)
            },
            tool_calls,
            stop_reason,
        }
    }
}

async fn chat_openai_compatible_streaming(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    timeouts: ModelTimeouts,
    on_text: OnTextDelta<'_>,
) -> Result<LlmResponse, AppError> {
    let endpoint = model_config.endpoint.trim().trim_end_matches('/');
    let url = if endpoint.ends_with("/chat/completions") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/chat/completions")
    };

    let mut body = json!({
        "model": model_config.model,
        "messages": messages,
        "max_tokens": 1600,
        "temperature": 0.2,
        "stream": true
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }

    let mut request = client.post(&url).json(&body);
    if let Some(api_key) = &model_config.api_key {
        if !api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }
    }

    let response = ensure_stream_status(request.send().await?, "LLM").await?;

    let mut assembler = OpenAiStreamAssembler::default();
    drive_stream(response, timeouts, |line| {
        let Some(payload) = sse_data_payload(line) else {
            return false;
        };
        if payload == "[DONE]" {
            return true;
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            if let Some(delta) = assembler.ingest(&value) {
                on_text(&delta);
            }
        }
        false
    })
    .await?;

    Ok(assembler.finish())
}

// ── Ollama 流式（NDJSON） ────────────────────────────────────

/// Ollama /api/chat 流式装配器。每行一个 JSON：
/// message.content 为文本增量；message.tool_calls 整体到达；done=true 结束。
#[derive(Default)]
struct OllamaStreamAssembler {
    content: String,
    tool_calls: Vec<ToolCall>,
    done_reason: Option<String>,
}

impl OllamaStreamAssembler {
    fn ingest(&mut self, payload: &Value) -> Option<String> {
        if let Some(reason) = payload["done_reason"].as_str() {
            self.done_reason = Some(reason.to_string());
        }
        if let Some(tcs) = payload["message"]["tool_calls"].as_array() {
            for tc in tcs {
                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }
                // Ollama 的 arguments 是 object；容错处理字符串形式
                let arguments = if tc["function"]["arguments"].is_object() {
                    tc["function"]["arguments"].clone()
                } else {
                    tc["function"]["arguments"]
                        .as_str()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(json!({}))
                };
                self.tool_calls.push(ToolCall {
                    id: tc["id"]
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("ollama_call_{}", self.tool_calls.len())),
                    name,
                    arguments,
                });
            }
        }
        let text = payload["message"]["content"].as_str()?;
        if text.is_empty() {
            return None;
        }
        self.content.push_str(text);
        Some(text.to_string())
    }

    fn finish(self) -> LlmResponse {
        let has_tools = !self.tool_calls.is_empty();
        let stop_reason = if has_tools {
            StopReason::ToolCall
        } else if self.done_reason.as_deref() == Some("length") {
            StopReason::MaxTokens
        } else {
            StopReason::Stop
        };
        LlmResponse {
            content: if self.content.is_empty() {
                None
            } else {
                Some(self.content)
            },
            tool_calls: if has_tools {
                Some(self.tool_calls)
            } else {
                None
            },
            stop_reason,
        }
    }
}

async fn chat_ollama_streaming(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    timeouts: ModelTimeouts,
    on_text: OnTextDelta<'_>,
) -> Result<LlmResponse, AppError> {
    let ollama_base = model_config.endpoint.trim().trim_end_matches('/');
    let url = if ollama_base.ends_with("/api/chat") {
        ollama_base.to_string()
    } else {
        format!("{ollama_base}/api/chat")
    };

    let mut body = json!({
        "model": model_config.model,
        "messages": messages,
        "stream": true
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }

    let response = ensure_stream_status(client.post(&url).json(&body).send().await?, "Ollama").await?;

    let mut assembler = OllamaStreamAssembler::default();
    drive_stream(response, timeouts, |line| {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            if let Some(delta) = assembler.ingest(&value) {
                on_text(&delta);
            }
            return value["done"].as_bool() == Some(true);
        }
        false
    })
    .await?;

    Ok(assembler.finish())
}

// ── Claude 流式（SSE 事件） ──────────────────────────────────

/// Claude SSE 装配器。按事件 type 分发：
/// content_block_start(tool_use) 开工具块 → content_block_delta 累积
/// text_delta / input_json_delta → message_delta 带 stop_reason。
#[derive(Default)]
struct ClaudeStreamAssembler {
    text: String,
    stop_reason: Option<String>,
    /// content block index → (tool_use id, name, partial_json 累积)
    tools: std::collections::BTreeMap<u64, (String, String, String)>,
}

impl ClaudeStreamAssembler {
    fn ingest(&mut self, payload: &Value) -> Option<String> {
        match payload["type"].as_str() {
            Some("content_block_start") => {
                let block = &payload["content_block"];
                if block["type"].as_str() == Some("tool_use") {
                    let index = payload["index"].as_u64().unwrap_or(0);
                    self.tools.insert(
                        index,
                        (
                            block["id"].as_str().unwrap_or("").to_string(),
                            block["name"].as_str().unwrap_or("").to_string(),
                            String::new(),
                        ),
                    );
                }
                None
            }
            Some("content_block_delta") => {
                let delta = &payload["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        let text = delta["text"].as_str()?;
                        if text.is_empty() {
                            return None;
                        }
                        self.text.push_str(text);
                        Some(text.to_string())
                    }
                    Some("input_json_delta") => {
                        let index = payload["index"].as_u64().unwrap_or(0);
                        if let Some(slot) = self.tools.get_mut(&index) {
                            slot.2.push_str(delta["partial_json"].as_str().unwrap_or(""));
                        }
                        None
                    }
                    _ => None,
                }
            }
            Some("message_delta") => {
                if let Some(reason) = payload["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_string());
                }
                None
            }
            _ => None,
        }
    }

    fn finish(self) -> LlmResponse {
        let tool_calls: Vec<ToolCall> = self
            .tools
            .into_values()
            .filter(|(_, name, _)| !name.is_empty())
            .map(|(id, name, args)| ToolCall {
                id,
                name,
                arguments: if args.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&args).unwrap_or(json!({}))
                },
            })
            .collect();
        let tool_calls = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };

        let stop_reason = match self.stop_reason.as_deref() {
            Some("tool_use") => StopReason::ToolCall,
            Some("max_tokens") => StopReason::MaxTokens,
            _ => {
                if tool_calls.is_some() {
                    StopReason::ToolCall
                } else {
                    StopReason::Stop
                }
            }
        };

        LlmResponse {
            content: if self.text.is_empty() {
                None
            } else {
                Some(self.text)
            },
            tool_calls,
            stop_reason,
        }
    }
}

async fn chat_claude_streaming(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    timeouts: ModelTimeouts,
    on_text: OnTextDelta<'_>,
) -> Result<LlmResponse, AppError> {
    let api_key = model_config
        .api_key
        .as_deref()
        .ok_or_else(|| AppError::Analysis("Claude 需要 API Key，请在设置中配置".to_string()))?;

    let endpoint = model_config.endpoint.trim().trim_end_matches('/');
    let url = if endpoint.ends_with("/messages") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/messages")
    };

    let (claude_messages, system_content, claude_tools) = build_claude_request_parts(messages, tools);

    let mut body = json!({
        "model": model_config.model,
        "max_tokens": 1600,
        "system": system_content,
        "messages": claude_messages,
        "stream": true
    });
    if !claude_tools.is_empty() {
        body["tools"] = json!(claude_tools);
    }

    let response = ensure_stream_status(
        client
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("x-api-key", api_key)
            .json(&body)
            .send()
            .await?,
        "Claude",
    )
    .await?;

    let mut assembler = ClaudeStreamAssembler::default();
    drive_stream(response, timeouts, |line| {
        let Some(payload) = sse_data_payload(line) else {
            return false;
        };
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            if let Some(delta) = assembler.ingest(&value) {
                on_text(&delta);
            }
            return value["type"].as_str() == Some("message_stop");
        }
        false
    })
    .await?;

    Ok(assembler.finish())
}

// ── Gemini 流式（alt=sse） ───────────────────────────────────

/// Gemini streamGenerateContent 装配器。每条 data 是一个 GenerateContentResponse
/// 分片：candidates[0].content.parts[] 里 text 为增量、functionCall 整体到达。
#[derive(Default)]
struct GeminiStreamAssembler {
    text: String,
    finish_reason: Option<String>,
    calls: Vec<(String, Value)>,
}

impl GeminiStreamAssembler {
    fn ingest(&mut self, payload: &Value) -> Option<String> {
        let candidate = &payload["candidates"][0];
        if let Some(reason) = candidate["finishReason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        let mut delta = String::new();
        if let Some(parts) = candidate["content"]["parts"].as_array() {
            for part in parts {
                if let Some(text) = part["text"].as_str() {
                    delta.push_str(text);
                }
                if let Some(fc) = part.get("functionCall") {
                    self.calls.push((
                        fc["name"].as_str().unwrap_or("").to_string(),
                        fc["args"].clone(),
                    ));
                }
            }
        }
        if delta.is_empty() {
            return None;
        }
        self.text.push_str(&delta);
        Some(delta)
    }

    fn finish(self) -> LlmResponse {
        let tool_calls: Vec<ToolCall> = self
            .calls
            .into_iter()
            .filter(|(name, _)| !name.is_empty())
            .enumerate()
            .map(|(i, (name, args))| ToolCall {
                id: format!("gemini_{i}"),
                name,
                arguments: args,
            })
            .collect();
        let has_tools = !tool_calls.is_empty();

        let stop_reason = if has_tools {
            StopReason::ToolCall
        } else if self.finish_reason.as_deref() == Some("MAX_TOKENS") {
            StopReason::MaxTokens
        } else {
            StopReason::Stop
        };

        LlmResponse {
            content: if self.text.is_empty() {
                None
            } else {
                Some(self.text)
            },
            tool_calls: if has_tools {
                Some(tool_calls)
            } else {
                None
            },
            stop_reason,
        }
    }
}

async fn chat_gemini_streaming(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    timeouts: ModelTimeouts,
    on_text: OnTextDelta<'_>,
) -> Result<LlmResponse, AppError> {
    let endpoint = model_config.endpoint.trim().trim_end_matches('/');
    let api_key = model_config
        .api_key
        .as_deref()
        .ok_or_else(|| AppError::Analysis("Gemini 需要 API Key，请在设置中配置".to_string()))?;
    let url = format!(
        "{endpoint}/models/{}:streamGenerateContent?alt=sse",
        model_config.model
    );

    let body = build_gemini_request_body(messages, tools);

    let response = ensure_stream_status(
        client
            .post(&url)
            .header("x-goog-api-key", api_key)
            .json(&body)
            .send()
            .await?,
        "Gemini",
    )
    .await?;

    let mut assembler = GeminiStreamAssembler::default();
    drive_stream(response, timeouts, |line| {
        let Some(payload) = sse_data_payload(line) else {
            return false;
        };
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            if let Some(delta) = assembler.ingest(&value) {
                on_text(&delta);
            }
        }
        false
    })
    .await?;

    Ok(assembler.finish())
}

// ══════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_user() {
        let msg = Message::user("你好");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.as_deref(), Some("你好"));
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
    }

    #[test]
    fn test_message_tool_result() {
        let msg = Message::tool_result_named("call_123", "结果数据", None);
        assert_eq!(msg.role, "tool");
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_123"));
    }

    #[test]
    fn test_parse_openai_response_with_tool_call() {
        // 模拟 OpenAI 返回一个 tool_call
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "analyze_intents",
                            "arguments": "{\"date_from\":\"2026-06-01\",\"date_to\":\"2026-06-09\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let parsed = parse_openai_response(&response).unwrap();

        assert_eq!(parsed.stop_reason, StopReason::ToolCall);
        assert!(parsed.content.is_none());

        let tc = &parsed.tool_calls.unwrap()[0];
        assert_eq!(tc.id, "call_abc123");
        assert_eq!(tc.name, "analyze_intents");
        assert_eq!(tc.arguments["date_from"], "2026-06-01");
    }

    #[test]
    fn test_parse_openai_response_text_only() {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "你好！我是你的工作助手。"
                },
                "finish_reason": "stop"
            }]
        });

        let parsed = parse_openai_response(&response).unwrap();
        assert_eq!(parsed.stop_reason, StopReason::Stop);
        assert_eq!(parsed.content.as_deref(), Some("你好！我是你的工作助手。"));
        assert!(parsed.tool_calls.is_none());
    }

    #[test]
    fn test_parse_claude_response_with_tool_use() {
        // 模拟 Claude 返回一个 tool_use
        let response = json!({
            "content": [
                {"type": "text", "text": "让我查一下..."},
                {"type": "tool_use", "id": "toolu_xyz", "name": "search_memory", "input": {"query": "debug"}}
            ],
            "stop_reason": "tool_use"
        });

        let parsed = parse_claude_response(&response).unwrap();

        assert_eq!(parsed.stop_reason, StopReason::ToolCall);
        assert_eq!(parsed.content.as_deref(), Some("让我查一下..."));

        let tc = &parsed.tool_calls.unwrap()[0];
        assert_eq!(tc.id, "toolu_xyz");
        assert_eq!(tc.name, "search_memory");
        // Claude 的 input 已经是 object，不需要 JSON.parse
        assert_eq!(tc.arguments["query"], "debug");
    }

    #[test]
    fn test_parse_gemini_response_with_function_call() {
        // 模拟 Gemini 返回一个 functionCall
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "analyze_intents",
                            "args": {"date_from": "2026-05-01", "date_to": "2026-05-31"}
                        }
                    }]
                }
            }]
        });

        let parsed = parse_gemini_response(&response).unwrap();

        assert_eq!(parsed.stop_reason, StopReason::ToolCall);
        let tc = &parsed.tool_calls.unwrap()[0];
        assert_eq!(tc.name, "analyze_intents");
        assert_eq!(tc.arguments["date_from"], "2026-05-01");
    }

    #[test]
    fn test_openai_arguments_are_string_but_parsed_to_object() {
        // OpenAI 的 arguments 是字符串，parse_openai_response 要 JSON.parse 它
        let response = json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call_test",
                        "type": "function",
                        "function": {
                            "name": "search_memory",
                            "arguments": "{\"query\":\"编码\",\"date_from\":\"2026-06-01\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let parsed = parse_openai_response(&response).unwrap();
        let tc = &parsed.tool_calls.unwrap()[0];
        // arguments 应该被解析成 object，不是字符串
        assert!(tc.arguments.is_object());
        assert_eq!(tc.arguments["query"], "编码");
    }

    // ── 流式装配器测试 ──────────────────────────────────────

    #[test]
    fn 行缓冲应正确处理跨chunk切分的行() {
        let mut buf = LineBuffer::new();
        // 一行被切成两个 chunk
        assert!(buf.push("data: {\"a\"").is_empty());
        let lines = buf.push(":1}\ndata: [DONE]\n");
        assert_eq!(lines, vec!["data: {\"a\":1}", "data: [DONE]"]);
        // \r\n 行尾应被剥掉
        let lines = buf.push("hello\r\n");
        assert_eq!(lines, vec!["hello"]);
    }

    #[test]
    fn sse行解析应提取data载荷并忽略其他行() {
        assert_eq!(sse_data_payload("data: {\"x\":1}"), Some("{\"x\":1}"));
        assert_eq!(sse_data_payload("data:[DONE]"), Some("[DONE]"));
        assert_eq!(sse_data_payload("event: message_stop"), None);
        assert_eq!(sse_data_payload(": keep-alive"), None);
    }

    #[test]
    fn openai流式装配应累积文本并拼装分片工具调用() {
        let mut asm = OpenAiStreamAssembler::default();

        // 文本增量逐条返回
        let d1 = asm.ingest(&json!({"choices":[{"delta":{"content":"今天"}}]}));
        assert_eq!(d1.as_deref(), Some("今天"));
        let d2 = asm.ingest(&json!({"choices":[{"delta":{"content":"写了代码"}}]}));
        assert_eq!(d2.as_deref(), Some("写了代码"));

        // 工具调用分片：第一片带 id+name，后续片只带 arguments 增量
        asm.ingest(&json!({"choices":[{"delta":{"tool_calls":[
            {"index":0,"id":"call_1","function":{"name":"search_memory","arguments":"{\"que"}}
        ]}}]}));
        asm.ingest(&json!({"choices":[{"delta":{"tool_calls":[
            {"index":0,"function":{"arguments":"ry\":\"编码\"}"}}
        ]}}]}));
        asm.ingest(&json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}));

        let resp = asm.finish();
        assert_eq!(resp.stop_reason, StopReason::ToolCall);
        assert_eq!(resp.content.as_deref(), Some("今天写了代码"));
        let calls = resp.tool_calls.expect("应拼出工具调用");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "search_memory");
        assert_eq!(calls[0].arguments["query"], "编码");
    }

    #[test]
    fn openai流式纯文本应以stop结束() {
        let mut asm = OpenAiStreamAssembler::default();
        asm.ingest(&json!({"choices":[{"delta":{"content":"答案"}}]}));
        asm.ingest(&json!({"choices":[{"delta":{},"finish_reason":"stop"}]}));
        let resp = asm.finish();
        assert_eq!(resp.stop_reason, StopReason::Stop);
        assert_eq!(resp.content.as_deref(), Some("答案"));
        assert!(resp.tool_calls.is_none());
    }

    #[test]
    fn claude流式装配应拼装input_json_delta工具参数() {
        let mut asm = ClaudeStreamAssembler::default();
        asm.ingest(&json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"toolu_1","name":"aggregate_stats"}}));
        asm.ingest(&json!({"type":"content_block_delta","index":0,
            "delta":{"type":"input_json_delta","partial_json":"{\"date_fr"}}));
        asm.ingest(&json!({"type":"content_block_delta","index":0,
            "delta":{"type":"input_json_delta","partial_json":"om\":\"2026-07-01\"}"}}));
        asm.ingest(&json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}));

        let resp = asm.finish();
        assert_eq!(resp.stop_reason, StopReason::ToolCall);
        let calls = resp.tool_calls.expect("应拼出工具调用");
        assert_eq!(calls[0].id, "toolu_1");
        assert_eq!(calls[0].arguments["date_from"], "2026-07-01");
    }

    #[test]
    fn claude流式文本增量应实时返回() {
        let mut asm = ClaudeStreamAssembler::default();
        let d = asm.ingest(&json!({"type":"content_block_delta","index":0,
            "delta":{"type":"text_delta","text":"结论："}}));
        assert_eq!(d.as_deref(), Some("结论："));
        asm.ingest(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}));
        let resp = asm.finish();
        assert_eq!(resp.stop_reason, StopReason::Stop);
        assert_eq!(resp.content.as_deref(), Some("结论："));
    }

    #[test]
    fn gemini流式装配应收集文本与函数调用() {
        let mut asm = GeminiStreamAssembler::default();
        let d = asm.ingest(&json!({"candidates":[{"content":{"parts":[{"text":"本周"}]}}]}));
        assert_eq!(d.as_deref(), Some("本周"));
        asm.ingest(&json!({"candidates":[{"content":{"parts":[
            {"functionCall":{"name":"trend_comparison","args":{"metric":"duration"}}}
        ]}}]}));
        let resp = asm.finish();
        assert_eq!(resp.stop_reason, StopReason::ToolCall);
        let calls = resp.tool_calls.expect("应拼出工具调用");
        assert_eq!(calls[0].id, "gemini_0");
        assert_eq!(calls[0].arguments["metric"], "duration");
    }

    #[test]
    fn ollama流式装配应处理对象参数与长度截断() {
        let mut asm = OllamaStreamAssembler::default();
        let d = asm.ingest(&json!({"message":{"content":"分析"},"done":false}));
        assert_eq!(d.as_deref(), Some("分析"));
        asm.ingest(&json!({"message":{"content":"","tool_calls":[
            {"function":{"name":"category_search","arguments":{"category":"development"}}}
        ]},"done":false}));
        let resp = asm.finish();
        assert_eq!(resp.stop_reason, StopReason::ToolCall);
        let calls = resp.tool_calls.expect("应拼出工具调用");
        assert_eq!(calls[0].name, "category_search");
        assert_eq!(calls[0].arguments["category"], "development");

        // 无工具 + done_reason=length → MaxTokens
        let mut asm2 = OllamaStreamAssembler::default();
        asm2.ingest(&json!({"message":{"content":"太长"},"done":true,"done_reason":"length"}));
        assert_eq!(asm2.finish().stop_reason, StopReason::MaxTokens);
    }
}
