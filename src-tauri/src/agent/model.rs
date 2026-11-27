//! Agent 模型适配层
//!
//! 职责：把统一的消息格式翻译成各家 API 的请求格式，
//!       把各家 API 的响应翻译回统一格式。

use super::deadline::Deadline;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;
use work_review_core::config::{AiProvider, ModelConfig};
use work_review_core::error::AppError;
use work_review_core::generation_params;

/// 非流式共享 HTTP 客户端（连接 10s；完整请求受调用方绝对 deadline 限制）。
/// 进程内复用连接池，避免每次模型调用都重建客户端与 TLS 连接。
static CHAT_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
/// 流式共享 HTTP 客户端：连接 10s；请求 deadline + 逐块 idle 双护栏。
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

fn local_model_target(endpoint: &str) -> Option<(String, u16)> {
    let parsed = reqwest::Url::parse(endpoint.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let host = parsed.host_str()?;
    let clean = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.');
    let is_loopback = clean == "localhost"
        || clean.ends_with(".localhost")
        || clean
            .parse::<IpAddr>()
            .is_ok_and(|ip| work_review_agent_harness::mcp_client::normalize_ip(ip).is_loopback());
    let port = parsed.port_or_known_default()?;
    is_loopback.then(|| (host.to_string(), port))
}

pub(crate) fn is_local_only_model_endpoint(endpoint: &str) -> bool {
    local_model_target(endpoint).is_some()
}

fn local_model_client(endpoint: &str) -> Result<Option<reqwest::Client>, AppError> {
    let Some((host, port)) = local_model_target(endpoint) else {
        return Ok(None);
    };
    // localhost（含子域）固定到回环，禁止 DNS、代理和重定向绕过本地模型授权。
    let addrs = [
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
        SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
    ];
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addrs)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map(Some)
        .map_err(|e| AppError::Unknown(e.to_string()))
}

async fn send_until(
    request: reqwest::RequestBuilder,
    deadline: Deadline,
) -> Result<reqwest::Response, AppError> {
    if deadline.is_expired() {
        return Err(AppError::Analysis("助手请求已超时".to_string()));
    }
    // reqwest 请求级 timeout 从连接持续到响应体读完，不止响应头。
    // 每次发送（含重试/回退）只用同一绝对 deadline 的剩余时间，覆盖 json/text/error body。
    let request = request.timeout(deadline.remaining());
    tokio::time::timeout(deadline.remaining(), request.send())
        .await
        .map_err(|_| AppError::Analysis("等待模型响应头超时".to_string()))?
        .map_err(|e| AppError::Unknown(e.to_string()))
}

/// 非流式请求发送：命中 429/5xx 时等待 2 秒重试一次（流式路径不重试）。
/// 重试等待与第二次发送都消费同一 deadline。
async fn send_with_retry(
    request: reqwest::RequestBuilder,
    deadline: Deadline,
) -> Result<reqwest::Response, AppError> {
    let retry = request.try_clone();
    let response = send_until(request, deadline).await?;
    let status = response.status();
    if status.as_u16() == 429 || status.is_server_error() {
        if let Some(retry_request) = retry {
            let wait = deadline.cap(Duration::from_secs(2));
            if wait.is_zero() {
                return Ok(response);
            }
            log::warn!("模型请求返回 {status}，{wait:?} 后重试一次");
            tokio::time::sleep(wait).await;
            return send_until(retry_request, deadline).await;
        }
    }
    Ok(response)
}

// ══════════════════════════════════════════════════════════
// 第一部分：统一的消息格式
// ══════════════════════════════════════════════════════════

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
    /// Claude 原始内容块：保留思考签名和块顺序，供工具结果回传。
    pub claude_content_blocks: Option<Vec<Value>>,
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
    /// 仅 Claude 请求使用，不序列化到其他提供商或持久化聊天记录。
    #[serde(skip)]
    pub claude_content_blocks: Option<Vec<Value>>,
}

impl Message {
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            claude_content_blocks: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            claude_content_blocks: None,
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
            claude_content_blocks: None,
        }
    }

    pub fn assistant_tool_response(response: &LlmResponse) -> Self {
        let mut message =
            Self::assistant_with_tool_calls(response.tool_calls.as_deref().unwrap_or_default());
        message.claude_content_blocks = response.claude_content_blocks.clone();
        message
    }

    fn to_provider_value(&self, provider: AiProvider) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        if provider == AiProvider::Claude {
            if let Some(blocks) = &self.claude_content_blocks {
                value["claude_content_blocks"] = json!(blocks);
            }
        }
        value
    }

    pub fn tool_result_named(tool_call_id: &str, content: &str, name: Option<&str>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: name.map(|n| n.to_string()),
            claude_content_blocks: None,
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
pub async fn chat_with_tools(
    model_config: &ModelConfig,
    system_prompt: &str,
    messages: &[Message],
    tools: &[Value],
    deadline: Deadline,
) -> Result<LlmResponse, AppError> {
    let local_client = local_model_client(&model_config.endpoint)?;
    let client = match local_client.as_ref() {
        Some(client) => client,
        None => chat_client()?,
    };

    // 构造完整的 messages 数组：system + 用户对话历史
    let mut full_messages = vec![json!({
        "role": "system",
        "content": system_prompt
    })];
    for msg in messages {
        full_messages.push(msg.to_provider_value(model_config.provider));
    }

    // 根据提供商分发
    match model_config.provider {
        AiProvider::Ollama => {
            chat_ollama(client, model_config, &full_messages, tools, deadline).await
        }
        AiProvider::Claude => {
            chat_claude(client, model_config, &full_messages, tools, deadline).await
        }
        AiProvider::Gemini => {
            chat_gemini(client, model_config, &full_messages, tools, deadline).await
        }
        _ => chat_openai_compatible(client, model_config, &full_messages, tools, deadline).await,
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
    deadline: Deadline,
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
        "temperature": 0.2
    });
    generation_params::apply_openai_compatible(&mut body, model_config, false);

    // 只有提供了工具定义时才加 tools 参数
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }

    let mut request = client.post(&url).json(&body);
    if let Some(api_key) = &model_config.api_key {
        if !api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }
    }

    let response = send_with_retry(request, deadline).await?;
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
    deadline: Deadline,
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
    generation_params::apply_ollama(&mut body, model_config);

    let response = send_with_retry(client.post(&url).json(&body), deadline).await?;
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
                // 原样回传 Claude 内容块，不能重建或丢弃已签名的思考块。
                Some("assistant") if m["claude_content_blocks"].is_array() => {
                    json!({"role": "assistant", "content": m["claude_content_blocks"]})
                }
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
    deadline: Deadline,
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
        "system": system_content,
        "messages": claude_messages,
    });
    if !claude_tools.is_empty() {
        body["tools"] = json!(claude_tools);
    }
    generation_params::apply_claude(&mut body, model_config, !claude_tools.is_empty());

    let response = send_with_retry(
        client
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("x-api-key", api_key)
            .json(&body),
        deadline,
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
    deadline: Deadline,
) -> Result<LlmResponse, AppError> {
    let endpoint = model_config.endpoint.trim().trim_end_matches('/');
    let api_key = model_config
        .api_key
        .as_deref()
        .ok_or_else(|| AppError::Analysis("Gemini 需要 API Key，请在设置中配置".to_string()))?;
    let url = format!("{endpoint}/models/{}:generateContent", model_config.model);

    let mut body = build_gemini_request_body(messages, tools);
    let mut generation_config = body
        .get("generationConfig")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    generation_params::apply_gemini(&mut generation_config, model_config);
    body["generationConfig"] = Value::Object(generation_config);

    let response = send_with_retry(
        client
            .post(&url)
            .header("x-goog-api-key", api_key)
            .json(&body),
        deadline,
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

    // 思考型模型的思维链在 reasoning_content；思考耗尽 max_tokens 时 content 为空，
    // 兜底取思维链，保证助手始终有内容可展示。
    let content = msg["content"]
        .as_str()
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| msg["reasoning_content"].as_str().map(|s| s.to_string()));

    Ok(LlmResponse {
        content,
        tool_calls,
        stop_reason,
        claude_content_blocks: None,
    })
}

/// 解析 Claude 格式的响应
pub(crate) fn parse_claude_response(result: &Value) -> Result<LlmResponse, AppError> {
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
        claude_content_blocks: content_blocks.cloned(),
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
        claude_content_blocks: None,
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
// - 超时策略：请求全过程共用调用方传入的绝对 deadline；流式读取额外限制
//   逐块空闲最多 30s，不因重试或回退重置总预算。

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
    on_text: OnTextDelta<'_>,
    deadline: Deadline,
) -> Result<LlmResponse, AppError> {
    // 流式客户端复用连接池；发送时设置剩余 deadline，读取另有逐块 idle 超时。
    let local_client = local_model_client(&model_config.endpoint)?;
    let client = match local_client.as_ref() {
        Some(client) => client,
        None => stream_client()?,
    };

    let mut full_messages = vec![json!({
        "role": "system",
        "content": system_prompt
    })];
    for msg in messages {
        full_messages.push(msg.to_provider_value(model_config.provider));
    }

    let streamed = match model_config.provider {
        AiProvider::Ollama => {
            chat_ollama_streaming(
                client,
                model_config,
                &full_messages,
                tools,
                on_text,
                deadline,
            )
            .await
        }
        AiProvider::Claude => {
            chat_claude_streaming(
                client,
                model_config,
                &full_messages,
                tools,
                on_text,
                deadline,
            )
            .await
        }
        AiProvider::Gemini => {
            chat_gemini_streaming(
                client,
                model_config,
                &full_messages,
                tools,
                on_text,
                deadline,
            )
            .await
        }
        _ => {
            chat_openai_compatible_streaming(
                client,
                model_config,
                &full_messages,
                tools,
                on_text,
                deadline,
            )
            .await
        }
    };

    match streamed {
        Ok(response) => Ok(response),
        Err(e) => {
            log::warn!("流式调用失败，回退非流式: {e}");
            chat_with_tools(model_config, system_prompt, messages, tools, deadline).await
        }
    }
}

/// 行缓冲：把任意切分的字节块拼成完整行，残行留在缓冲区等下一个 chunk。
///
/// 按**字节**缓冲、遇到 `\n` 才解码：网络分块可能把一个多字节 UTF-8 字符（中文 3 字节）
/// 切在两个 chunk 之间，逐块 `from_utf8_lossy` 会把它变成 "�"，还可能破坏 JSON 行。
/// 流式单行缓冲上限（4 MiB），防止异常服务端持续输出不含换行符的数据导致内存无限增长。
const MAX_STREAM_LINE_BYTES: usize = 4 * 1024 * 1024;

struct LineBuffer {
    buf: Vec<u8>,
    seen_first_line: bool,
}

impl LineBuffer {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            seen_first_line: false,
        }
    }

    fn take_decoded_line(&mut self) -> String {
        let raw = std::mem::take(&mut self.buf);
        let text = String::from_utf8_lossy(&raw);
        let mut line = text.trim_end_matches(['\n', '\r']);
        if !self.seen_first_line {
            self.seen_first_line = true;
            line = line.strip_prefix('\u{FEFF}').unwrap_or(line);
        }
        line.to_string()
    }

    /// 喂入一个 chunk，返回其中所有完整行（去掉行尾 \r\n）。
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, AppError> {
        let mut lines = Vec::new();
        let mut start = 0usize;
        while let Some(rel) = chunk[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + rel;
            if self.buf.len().saturating_add(end - start) > MAX_STREAM_LINE_BYTES {
                self.buf.clear();
                return Err(AppError::Analysis("流式响应单行超过大小限制".to_string()));
            }
            self.buf.extend_from_slice(&chunk[start..end]);
            lines.push(self.take_decoded_line());
            start = end + 1;
        }
        let remainder = &chunk[start..];
        if self.buf.len().saturating_add(remainder.len()) > MAX_STREAM_LINE_BYTES {
            self.buf.clear();
            return Err(AppError::Analysis("流式响应单行超过大小限制".to_string()));
        }
        self.buf.extend_from_slice(remainder);
        Ok(lines)
    }

    /// 流正常 EOF 时冲刷最后一条未以 `\n` 结尾的残行。
    fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        Some(self.take_decoded_line())
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
    deadline: Deadline,
    mut on_line: impl FnMut(&str) -> bool,
) -> Result<(), AppError> {
    let mut response = response;
    let mut line_buf = LineBuffer::new();

    loop {
        if deadline.is_expired() {
            return Err(AppError::Analysis("流式响应总时长超限".to_string()));
        }
        let idle = deadline.cap(Duration::from_secs(30));
        if idle.is_zero() {
            return Err(AppError::Analysis("流式响应总时长超限".to_string()));
        }
        let chunk = tokio::time::timeout(idle, response.chunk())
            .await
            .map_err(|_| AppError::Analysis("流式响应空闲超时".to_string()))?
            .map_err(|e| AppError::Analysis(format!("流式读取失败: {e}")))?;

        let Some(bytes) = chunk else {
            if let Some(line) = line_buf.finish() {
                if !line.is_empty() {
                    let _ = on_line(&line);
                }
            }
            return Ok(()); // 流正常结束
        };
        for line in line_buf.push(&bytes)? {
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
                let idx = tc["index"]
                    .as_u64()
                    .unwrap_or(self.partial_calls.len() as u64) as usize;
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
        // 思考型模型（DeepSeek V4/R1、Qwen3 等）的思维链增量走 reasoning_content，
        // 与 content 同级；正文帧缺失/为空时输出思考增量，避免助手全程无输出。
        let text = delta["content"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| delta["reasoning_content"].as_str())?;
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
            claude_content_blocks: None,
        }
    }
}

async fn chat_openai_compatible_streaming(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    on_text: OnTextDelta<'_>,
    deadline: Deadline,
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
        "temperature": 0.2,
        "stream": true
    });
    generation_params::apply_openai_compatible(&mut body, model_config, true);
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }

    let mut request = client.post(&url).json(&body);
    if let Some(api_key) = &model_config.api_key {
        if !api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }
    }

    let response = ensure_stream_status(send_until(request, deadline).await?, "LLM").await?;

    let mut assembler = OpenAiStreamAssembler::default();
    drive_stream(response, deadline, |line| {
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
            claude_content_blocks: None,
        }
    }
}

async fn chat_ollama_streaming(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    on_text: OnTextDelta<'_>,
    deadline: Deadline,
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
    generation_params::apply_ollama(&mut body, model_config);

    let response = ensure_stream_status(
        send_until(client.post(&url).json(&body), deadline).await?,
        "Ollama",
    )
    .await?;

    let mut assembler = OllamaStreamAssembler::default();
    drive_stream(response, deadline, |line| {
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
    /// 保留全部内容块和原始顺序，思考文本与签名分片独立累积。
    content_blocks: std::collections::BTreeMap<u64, Value>,
}

impl ClaudeStreamAssembler {
    fn ingest(&mut self, payload: &Value) -> Option<String> {
        match payload["type"].as_str() {
            Some("content_block_start") => {
                let block = &payload["content_block"];
                let index = payload["index"].as_u64().unwrap_or(0);
                self.content_blocks.insert(index, block.clone());
                if block["type"].as_str() == Some("tool_use") {
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
                let index = payload["index"].as_u64().unwrap_or(0);
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        let text = delta["text"].as_str()?;
                        if text.is_empty() {
                            return None;
                        }
                        self.text.push_str(text);
                        if let Some(block) = self.content_blocks.get_mut(&index) {
                            if let Some(Value::String(full_text)) = block.get_mut("text") {
                                full_text.push_str(text);
                            } else {
                                block["text"] = json!(text);
                            }
                        }
                        Some(text.to_string())
                    }
                    Some("thinking_delta") | Some("signature_delta") => {
                        let field = if delta["type"] == "thinking_delta" {
                            "thinking"
                        } else {
                            "signature"
                        };
                        if let Some(block) = self.content_blocks.get_mut(&index) {
                            let text = delta[field].as_str().unwrap_or("");
                            if let Some(Value::String(value)) = block.get_mut(field) {
                                value.push_str(text);
                            } else {
                                block[field] = json!(text);
                            }
                        }
                        None
                    }
                    Some("input_json_delta") => {
                        if let Some(slot) = self.tools.get_mut(&index) {
                            slot.2
                                .push_str(delta["partial_json"].as_str().unwrap_or(""));
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

    fn finish(mut self) -> LlmResponse {
        // 工具参数以 JSON 分片传送；解析完成后补回原始工具块。
        for (index, (_, _, args)) in &self.tools {
            if !args.trim().is_empty() {
                if let Some(block) = self.content_blocks.get_mut(index) {
                    block["input"] = serde_json::from_str(args).unwrap_or(json!({}));
                }
            }
        }
        let tool_calls: Vec<ToolCall> = self
            .tools
            .into_iter()
            .filter(|(_, (_, name, _))| !name.is_empty())
            .map(|(index, (id, name, args))| ToolCall {
                id,
                name,
                arguments: if args.trim().is_empty() {
                    self.content_blocks
                        .get(&index)
                        .and_then(|block| block.get("input"))
                        .cloned()
                        .unwrap_or(json!({}))
                } else {
                    serde_json::from_str(&args).unwrap_or(json!({}))
                },
            })
            .collect();
        let claude_content_blocks = Some(self.content_blocks.into_values().collect());
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
            claude_content_blocks,
        }
    }
}

async fn chat_claude_streaming(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    on_text: OnTextDelta<'_>,
    deadline: Deadline,
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
        "system": system_content,
        "messages": claude_messages,
        "stream": true
    });
    if !claude_tools.is_empty() {
        body["tools"] = json!(claude_tools);
    }
    generation_params::apply_claude(&mut body, model_config, !claude_tools.is_empty());

    let response = ensure_stream_status(
        send_until(
            client
                .post(&url)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .header("x-api-key", api_key)
                .json(&body),
            deadline,
        )
        .await?,
        "Claude",
    )
    .await?;

    let mut assembler = ClaudeStreamAssembler::default();
    drive_stream(response, deadline, |line| {
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
            tool_calls: if has_tools { Some(tool_calls) } else { None },
            stop_reason,
            claude_content_blocks: None,
        }
    }
}

async fn chat_gemini_streaming(
    client: &reqwest::Client,
    model_config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
    on_text: OnTextDelta<'_>,
    deadline: Deadline,
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

    let mut body = build_gemini_request_body(messages, tools);
    let mut generation_config = body
        .get("generationConfig")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    generation_params::apply_gemini(&mut generation_config, model_config);
    body["generationConfig"] = Value::Object(generation_config);

    let response = ensure_stream_status(
        send_until(
            client
                .post(&url)
                .header("x-goog-api-key", api_key)
                .json(&body),
            deadline,
        )
        .await?,
        "Gemini",
    )
    .await?;

    let mut assembler = GeminiStreamAssembler::default();
    drive_stream(response, deadline, |line| {
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_test_headers(socket: &mut tokio::net::TcpStream) {
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(socket.read_u8().await.unwrap());
            assert!(headers.len() <= 8192, "本地测试请求头过大");
        }
        let content_len = String::from_utf8(headers)
            .unwrap()
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
            .unwrap_or(0);
        assert!(content_len < 64 * 1024);
        socket.read_exact(&mut vec![0; content_len]).await.unwrap();
    }

    /// 只绑定回环地址；响应头及部分 body 发完后保持连接，不结束响应体。
    async fn stalled_body_server(
        status: &'static str,
        retry_first: bool,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            if retry_first {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_test_headers(&mut socket).await;
                socket
                    .write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
            }
            let (mut socket, _) = listener.accept().await.unwrap();
            read_test_headers(&mut socket).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{"
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            std::future::pending::<()>().await;
            drop(socket);
        });
        (endpoint, server)
    }

    #[tokio::test]
    async fn 响应头之后的json和错误body读取不得超过截止() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for case in ["json", "text", "stream_error"] {
            let status = if case == "json" {
                "200 OK"
            } else {
                "400 Bad Request"
            };
            let (endpoint, server) = stalled_body_server(status, false).await;
            let deadline = Deadline::after(Duration::from_millis(150));
            let response = send_until(client.get(&endpoint), deadline)
                .await
                .expect("本地服务应立即返回响应头");
            let result = tokio::time::timeout(Duration::from_secs(1), async {
                match case {
                    "json" => {
                        let error = response.json::<Value>().await.unwrap_err();
                        assert!(error.is_timeout(), "JSON 应受请求 deadline 限制: {error}");
                    }
                    "text" => {
                        let error = response.text().await.unwrap_err();
                        assert!(
                            error.is_timeout(),
                            "错误文本应受请求 deadline 限制: {error}"
                        );
                    }
                    _ => assert!(ensure_stream_status(response, "local").await.is_err()),
                }
            })
            .await;
            server.abort();
            result.expect("响应头后的 body 卡住不能绕过 deadline");
            assert!(deadline.is_expired(), "不应在 body 未完成时提前返回");
        }
    }

    #[tokio::test]
    async fn 重试后的响应体仍应消费原截止预算() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let (endpoint, server) = stalled_body_server("200 OK", true).await;
        // 2 秒重试等待后只剩 400ms；第二次请求不得重新获得完整预算。
        let deadline = Deadline::after(Duration::from_millis(2400));
        let result = tokio::time::timeout(Duration::from_millis(3000), async {
            let response = send_with_retry(client.get(&endpoint), deadline)
                .await
                .unwrap();
            assert!(response.status().is_success(), "应已进入第二次请求");
            let error = response.json::<Value>().await.unwrap_err();
            assert!(error.is_timeout(), "重试后的 body 应按原截止超时: {error}");
        })
        .await;
        server.abort();
        result.expect("重试后的响应体不能重新获得完整预算");
        assert!(deadline.is_expired());
    }

    #[tokio::test]
    async fn 本地模型流式与非流式请求都不得跟随重定向() {
        let destination = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", gateway.local_addr().unwrap());
        let location = format!("http://{}/collect", destination.local_addr().unwrap());
        let server = tokio::spawn(async move {
            // 非流式一次，流式一次及其非流式回退一次。
            for _ in 0..3 {
                let (mut socket, _) = gateway.accept().await.unwrap();
                read_test_headers(&mut socket).await;
                let response = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let model = ModelConfig {
            provider: AiProvider::OpenAI,
            endpoint,
            model: "local-test".into(),
            ..ModelConfig::default_text()
        };
        assert!(is_local_only_model_endpoint(&model.endpoint));
        let messages = [Message::user("private-local-record")];
        let result = chat_with_tools(
            &model,
            "local only",
            &messages,
            &[],
            Deadline::after(Duration::from_secs(2)),
        )
        .await;
        assert!(result.is_err());
        let result = chat_with_tools_streaming(
            &model,
            "local only",
            &messages,
            &[],
            &mut |_| {},
            Deadline::after(Duration::from_secs(2)),
        )
        .await;
        assert!(result.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), destination.accept())
                .await
                .is_err(),
            "不得向重定向目的地发送任何请求"
        );
        server.abort();
    }

    #[test]
    fn 本地模型请求应固定localhost解析并绕过环境代理() {
        const CHILD: &str = "WORK_REVIEW_LOCAL_MODEL_PROXY_TEST";
        if std::env::var_os(CHILD).is_none() {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "agent::model::tests::本地模型请求应固定localhost解析并绕过环境代理",
                    "--nocapture",
                ])
                .env(CHILD, "1");
            for key in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                command.env(key, "http://127.0.0.1:9");
            }
            command.env("NO_PROXY", "").env("no_proxy", "");
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_test_headers(&mut socket).await;
                let body = r#"{"choices":[{"message":{"content":"local-ok"},"finish_reason":"stop"}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let model = ModelConfig {
                provider: AiProvider::OpenAI,
                endpoint: format!("http://private-gateway.localhost.:{port}"),
                model: "local-test".into(),
                ..ModelConfig::default_text()
            };
            let response = chat_with_tools(
                &model, "local only", &[Message::user("private-local-record")], &[],
                Deadline::after(Duration::from_secs(2)),
            ).await.unwrap();
            assert_eq!(response.content.as_deref(), Some("local-ok"));
            server.await.unwrap();
        });
    }

    #[test]
    fn test_message_user() {
        let msg = Message::user("你好");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.as_deref(), Some("你好"));
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
    }

    #[test]
    fn 非流式解析应在正文为空时兜底思维链() {
        // 思考型模型耗尽额度：content 为空、思维链在 reasoning_content
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning_content": "用户在询问工作情况，我需要查询记录…"
                },
                "finish_reason": "length"
            }]
        });

        let parsed = parse_openai_response(&response).unwrap();
        assert_eq!(
            parsed.content.as_deref(),
            Some("用户在询问工作情况，我需要查询记录…")
        );
        assert_eq!(parsed.stop_reason, StopReason::MaxTokens);

        // 正文非空时不受 reasoning_content 影响
        let normal = json!({
            "choices": [{
                "message": { "role": "assistant", "content": "正文回答", "reasoning_content": "思考" },
                "finish_reason": "stop"
            }]
        });
        let parsed = parse_openai_response(&normal).unwrap();
        assert_eq!(parsed.content.as_deref(), Some("正文回答"));
    }

    #[test]
    fn 流式装配器应输出思维链增量() {
        let mut assembler = OpenAiStreamAssembler::default();

        // 思考阶段：delta 只有 reasoning_content
        let thinking_frame = json!({
            "choices": [{ "delta": { "reasoning_content": "先查数据库…" }, "finish_reason": null }]
        });
        assert_eq!(
            assembler.ingest(&thinking_frame),
            Some("先查数据库…".to_string())
        );

        // 正文阶段：delta 有 content
        let answer_frame = json!({
            "choices": [{ "delta": { "content": "今日工作总结如下" }, "finish_reason": null }]
        });
        assert_eq!(
            assembler.ingest(&answer_frame),
            Some("今日工作总结如下".to_string())
        );

        // 全程只有思维链（额度耗尽）时，finish 结果仍应包含思考文本
        let finished = assembler.finish();
        assert!(finished
            .content
            .as_deref()
            .is_some_and(|s| s.contains("先查数据库…")));
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
        assert!(buf.push(b"data: {\"a\"").unwrap().is_empty());
        let lines = buf.push(b":1}\ndata: [DONE]\n").unwrap();
        assert_eq!(lines, vec!["data: {\"a\":1}", "data: [DONE]"]);
        // \r\n 行尾应被剥掉
        let lines = buf.push(b"hello\r\n").unwrap();
        assert_eq!(lines, vec!["hello"]);
    }

    #[test]
    fn 行缓冲不应破坏跨chunk切分的多字节字符() {
        let mut buf = LineBuffer::new();
        let line = "data: {\"content\":\"你好世界\"}\n".as_bytes();
        // 在"你"的 3 个字节中间切开
        let split = line.iter().position(|byte| *byte == 0xE4).unwrap() + 1;
        assert!(buf.push(&line[..split]).unwrap().is_empty());
        let lines = buf.push(&line[split..]).unwrap();
        assert_eq!(lines, vec!["data: {\"content\":\"你好世界\"}"]);
        assert!(!lines[0].contains('\u{FFFD}'));
    }

    #[test]
    fn 行缓冲应剥离首行bom并冲刷无换行尾行且拦截超长行() {
        let mut buf = LineBuffer::new();
        let lines = buf
            .push("\u{FEFF}data: {\"content\":\"首行\"}\ndata: {\"content\":\"尾行\"}".as_bytes())
            .unwrap();
        assert_eq!(lines, vec!["data: {\"content\":\"首行\"}"]);
        assert_eq!(
            buf.finish().as_deref(),
            Some("data: {\"content\":\"尾行\"}")
        );
        assert!(buf.finish().is_none());

        let mut limited = LineBuffer::new();
        let oversized = vec![b'a'; MAX_STREAM_LINE_BYTES + 1];
        assert!(limited.push(&oversized).is_err());
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
    fn claude工具往返应原样保留思考签名和全部内容块() {
        let blocks = json!([
            {"type":"thinking","thinking":"","signature":"原始签名"},
            {"type":"text","text":"先查询"},
            {"type":"redacted_thinking","data":"加密数据"},
            {"type":"text","text":"记录"},
            {"type":"tool_use","id":"toolu_1","name":"aggregate_stats","input":{"date_from":"2026-09-01"}}
        ]);
        let response =
            parse_claude_response(&json!({"content": blocks, "stop_reason":"tool_use"})).unwrap();
        assert_eq!(response.content.as_deref(), Some("先查询记录"));
        let message = Message::assistant_tool_response(&response);
        let tool_result = Message::tool_result_named("toolu_1", "已查询", None);
        let (messages, _, _) = build_claude_request_parts(
            &[
                message.to_provider_value(AiProvider::Claude),
                tool_result.to_provider_value(AiProvider::Claude),
            ],
            &[],
        );
        assert_eq!(messages[0]["content"], blocks);
        assert_eq!(messages[1]["content"][0]["tool_use_id"], "toolu_1");
        for provider in [AiProvider::OpenAI, AiProvider::Ollama, AiProvider::Gemini] {
            let value = message.to_provider_value(provider);
            assert!(value.get("claude_content_blocks").is_none());
            assert_eq!(value["tool_calls"][0]["id"], "toolu_1");
        }
        assert!(serde_json::to_value(&message)
            .unwrap()
            .get("claude_content_blocks")
            .is_none());
    }

    #[test]
    fn claude流式工具往返应拼接思考和签名分片并保留块顺序() {
        let blocks = json!([
            {"type":"thinking","thinking":"推理内容","signature":"签名完整"},
            {"type":"text","text":"先查记录"},
            {"type":"tool_use","id":"toolu_1","name":"aggregate_stats","input":{"date_from":"2026-09-01"}},
            {"type":"text","text":"再回答"}
        ]);
        let mut asm = ClaudeStreamAssembler::default();
        for (index, block) in blocks.as_array().unwrap().iter().enumerate() {
            let mut start = block.clone();
            match block["type"].as_str().unwrap() {
                "thinking" => {
                    start["thinking"] = json!("");
                    start["signature"] = json!("");
                }
                "text" => start["text"] = json!(""),
                "tool_use" => start["input"] = json!({}),
                _ => {}
            }
            asm.ingest(&json!({"type":"content_block_start","index":index,"content_block":start}));
        }
        for (kind, field, parts) in [
            ("thinking_delta", "thinking", ["推理", "内容"]),
            ("signature_delta", "signature", ["签名", "完整"]),
        ] {
            for part in parts {
                let mut delta = json!({"type":kind});
                delta[field] = json!(part);
                assert!(asm
                    .ingest(&json!({"type":"content_block_delta","index":0,"delta":delta}))
                    .is_none());
            }
        }
        for (index, text) in [(1, "先查"), (1, "记录"), (3, "再回答")] {
            assert_eq!(asm.ingest(&json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}})).as_deref(), Some(text));
        }
        for part in ["{\"date_from\":", "\"2026-09-01\"}"] {
            asm.ingest(&json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":part}}));
        }
        asm.ingest(&json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}));
        let response = asm.finish();
        assert_eq!(response.content.as_deref(), Some("先查记录再回答"));
        assert_eq!(
            response.claude_content_blocks.as_ref().unwrap(),
            blocks.as_array().unwrap()
        );
        assert_eq!(
            response.tool_calls.as_ref().unwrap()[0].arguments,
            blocks[2]["input"]
        );
        let message = Message::assistant_tool_response(&response);
        let (messages, _, _) =
            build_claude_request_parts(&[message.to_provider_value(AiProvider::Claude)], &[]);
        assert_eq!(messages[0]["content"], blocks);
    }

    #[test]
    fn claude流式起始块已有工具参数时不应丢失() {
        let mut asm = ClaudeStreamAssembler::default();
        asm.ingest(&json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"toolu_1","name":"aggregate_stats","input":{"date_from":"2026-09-01"}}}));
        let response = asm.finish();
        assert_eq!(
            response.tool_calls.as_ref().unwrap()[0].arguments["date_from"],
            "2026-09-01"
        );
        assert_eq!(
            response.claude_content_blocks.unwrap()[0]["input"]["date_from"],
            "2026-09-01"
        );
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
