//! Auto-extracted from the historical `commands.rs`. Behavior unchanged.

use crate::config::{AiProvider, AiProviderConfig, ModelConfig};
use crate::error::AppError;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

/// 模型测试结果
#[derive(Serialize, Deserialize, Debug)]
pub struct ModelTestResult {
    pub success: bool,
    pub message: String,
    pub response_time_ms: u64,
    pub model_info: Option<String>,
}

pub(crate) fn is_text_model_available(model_config: &ModelConfig) -> bool {
    !model_config.endpoint.trim().is_empty() && !model_config.model.trim().is_empty()
}

/// 判断主机名是否为本机/内网地址（Ollama 等本地部署允许走 http）。
fn is_private_or_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            std::net::IpAddr::V6(v6) => v6.is_loopback(),
        };
    }
    false
}

/// 校验模型端点：远程端点必须使用 https，明文 http 仅允许本机/内网地址
/// （10.x / 192.168.x / 172.16-31.x / 127.0.0.1 / [::1] / localhost），
/// 防止 API Key 与工作数据经明文链路发往远程服务。
/// 空端点不校验（视为未配置）；非 http/https 前缀交由请求层报错。
pub(crate) fn validate_model_endpoint(endpoint: &str) -> Result<(), AppError> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    let Some(rest) = lower.strip_prefix("http://") else {
        return Ok(());
    };

    // 提取主机名：截到 path/query/fragment 之前，剥离端口；IPv6 形如 [::1]:11434
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(inner) = host_port.strip_prefix('[') {
        inner.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };

    if is_private_or_local_host(host) {
        return Ok(());
    }
    Err(AppError::Config(
        "远程模型端点必须使用 https（本机/内网地址除外）".to_string(),
    ))
}


/// 测试模型连接（新版，使用 ModelConfig）
#[tauri::command]
pub async fn test_model(model_config: ModelConfig) -> Result<ModelTestResult, AppError> {
    validate_model_endpoint(&model_config.endpoint)?;
    let start = std::time::Instant::now();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Unknown(e.to_string()))?;

    // 将 ModelConfig 转换为 AiProviderConfig 以复用现有测试逻辑
    let provider_config = AiProviderConfig {
        provider: model_config.provider,
        endpoint: model_config.endpoint,
        api_key: model_config.api_key,
        model: model_config.model,
        vision_model: None,
    };

    let result = match provider_config.provider {
        AiProvider::Ollama => test_ollama(&client, &provider_config).await,
        AiProvider::Gemini => test_gemini(&client, &provider_config).await,
        AiProvider::Claude => test_claude(&client, &provider_config).await,
        // OpenAI 及兼容格式的供应商（硅基流动、DeepSeek、通义千问、智谱、月之暗面、豆包）
        _ if provider_config.provider.is_openai_compatible() => {
            test_openai(&client, &provider_config).await
        }
        // 兜底：默认使用 OpenAI 格式
        _ => test_openai(&client, &provider_config).await,
    };

    let elapsed = start.elapsed().as_millis() as u64;

    match result {
        Ok(info) => Ok(ModelTestResult {
            success: true,
            message: "连接成功！模型可用。".to_string(),
            response_time_ms: elapsed,
            model_info: Some(info),
        }),
        Err(e) => Ok(ModelTestResult {
            success: false,
            message: format!("连接失败: {e}"),
            response_time_ms: elapsed,
            model_info: None,
        }),
    }
}

/// 测试 Ollama 连接
async fn test_ollama(
    client: &reqwest::Client,
    config: &AiProviderConfig,
) -> Result<String, String> {
    // 1. 先测试服务是否可用
    let tags_url = format!("{}/api/tags", config.endpoint);
    let response = client
        .get(&tags_url)
        .send()
        .await
        .map_err(|e| format!("无法连接到 Ollama 服务: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Ollama 服务返回错误: {}", response.status()));
    }

    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("解析响应失败: {e}"))?;

    // 2. 基于模型列表和能力信息判断是否为可用的文本生成模型
    let models = data["models"].as_array().ok_or("无法获取模型列表")?;
    let installed_model_exists = models.iter().any(|model| {
        model["name"]
            .as_str()
            .is_some_and(|name| ollama_model_names_match(&config.model, name))
    });
    let available_models = resolve_ollama_text_model_names(client, &config.endpoint, &data)
        .await
        .map_err(|e| format!("过滤 Ollama 模型列表失败: {e}"))?;

    let text_model_exists = available_models
        .iter()
        .any(|name| ollama_model_names_match(&config.model, name));

    if !text_model_exists {
        if installed_model_exists {
            return Err(format!(
                "模型 {} 已安装，但不是可用于对话/生成的文本模型",
                config.model
            ));
        }

        let available: Vec<String> = available_models.into_iter().take(5).collect();
        let available_hint = if available.is_empty() {
            "当前未发现可用文本模型".to_string()
        } else {
            format!("可用模型: {}", available.join(", "))
        };
        return Err(format!("模型 {} 未安装。{}", config.model, available_hint));
    }

    // 3. 实际调用模型生成测试（关键验证步骤）
    let generate_url = format!("{}/api/generate", config.endpoint);
    let test_response = client
        .post(&generate_url)
        .json(&serde_json::json!({
            "model": config.model,
            "prompt": "Hi",
            "stream": false,
            "options": {
                "num_predict": 5  // 只生成5个token，快速测试
            }
        }))
        .send()
        .await
        .map_err(|e| format!("调用模型失败: {e}"))?;

    if !test_response.status().is_success() {
        let error_text = test_response.text().await.unwrap_or_default();
        return Err(format!("模型响应失败: {error_text}"));
    }

    let result: serde_json::Value = test_response
        .json()
        .await
        .map_err(|e| format!("解析模型响应失败: {e}"))?;

    // 检查是否有实际响应
    if result["response"].as_str().is_some() {
        Ok(format!("模型 {} 测试通过，响应正常", config.model))
    } else {
        Err("模型返回空响应".to_string())
    }
}

/// 测试 OpenAI 连接
fn openai_connection_test_max_tokens() -> u32 {
    16
}

/// 判断端点路径是否已含 OpenAI 风格版本号段（/v1、/v2、/v3 … /v9）。
///
/// 火山引擎用 `/api/v3`、智谱用 `/api/paas/v4`——这些已经是完整版本路径，
/// 不应再追加 `/v1`（否则拼出 `/api/v3/v1/chat/completions` 这种不存在的端点）。
fn endpoint_has_version_segment(base: &str) -> bool {
    base.rsplit('/').next().is_some_and(|last| {
        last.len() >= 2
            && last.starts_with('v')
            && last[1..].chars().all(|c| c.is_ascii_digit())
            && !last[1..].is_empty()
    })
}

fn openai_compatible_chat_completion_urls(endpoint: &str) -> Vec<String> {
    let base = endpoint.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }

    if base.ends_with("/chat/completions") {
        return vec![base.to_string()];
    }

    // base 已含版本号段（/v1、/v3、/v4 等）：直接拼 /chat/completions，不再回退 /v1。
    // 例：火山引擎 api/v3 → v3/chat/completions（正确），不应再试 v3/v1/chat/completions。
    let mut urls = vec![format!("{base}/chat/completions")];
    if !endpoint_has_version_segment(&base) {
        // base 不含版本号（如 DeepSeek api.deepseek.com）：补一个 /v1 回退候选。
        urls.push(format!("{base}/v1/chat/completions"));
    }
    urls.dedup();
    urls
}

async fn test_openai(
    client: &reqwest::Client,
    config: &AiProviderConfig,
) -> Result<String, String> {
    let api_key = config.api_key.as_ref().ok_or("未配置 API Key")?;

    let payload = serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": openai_connection_test_max_tokens(),
    });

    let mut last_error = None;

    for url in openai_compatible_chat_completion_urls(&config.endpoint) {
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&payload)
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(format!("{url} 请求失败: {error}"));
                continue;
            }
        };

        if response.status().is_success() {
            let data: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("解析响应失败: {e}"))?;
            let model_used = data["model"].as_str().unwrap_or(&config.model);
            return Ok(format!("模型 {model_used} 响应正常"));
        }

        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        last_error = Some(format!("{url} API 错误 ({status}): {error_text}"));
    }

    Err(last_error.unwrap_or_else(|| "API 请求失败：未生成可用请求地址".to_string()))
}

/// 测试 Google Gemini 连接
async fn test_gemini(
    client: &reqwest::Client,
    config: &AiProviderConfig,
) -> Result<String, String> {
    let api_key = config.api_key.as_ref().ok_or("未配置 API Key")?;

    // API Key 走请求头而非 URL query，避免泄漏到日志/代理记录
    let url = format!(
        "{}/models/{}:generateContent",
        config.endpoint, config.model
    );

    let response = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(&serde_json::json!({
            "contents": [{"parts": [{"text": "Hello"}]}],
            "generationConfig": {"maxOutputTokens": 10}
        }))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    if response.status().is_success() {
        Ok(format!("Gemini 模型 {} 响应正常", config.model))
    } else {
        let error_text = response.text().await.unwrap_or_default();
        Err(format!("API 错误: {error_text}"))
    }
}

/// 测试 Anthropic Claude 连接
async fn test_claude(
    client: &reqwest::Client,
    config: &AiProviderConfig,
) -> Result<String, String> {
    let api_key = config.api_key.as_ref().ok_or("未配置 API Key")?;

    let claude_base = config.endpoint.trim().trim_end_matches('/');
    let claude_url = if claude_base.ends_with("/messages") {
        claude_base.to_string()
    } else {
        format!("{claude_base}/messages")
    };
    let response = client
        .post(&claude_url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({
            "model": config.model,
            "max_tokens": 10,
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    if response.status().is_success() {
        Ok(format!("Claude 模型 {} 响应正常", config.model))
    } else {
        let error_text = response.text().await.unwrap_or_default();
        Err(format!("API 错误: {error_text}"))
    }
}

fn normalize_ollama_model_name(name: &str) -> Option<(String, String)> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    match normalized.rsplit_once(':') {
        Some((base, tag)) if !base.is_empty() && !tag.is_empty() => {
            Some((base.to_string(), tag.to_string()))
        }
        _ => Some((normalized, "latest".to_string())),
    }
}

fn ollama_model_names_match(configured: &str, installed: &str) -> bool {
    normalize_ollama_model_name(configured)
        .zip(normalize_ollama_model_name(installed))
        .is_some_and(
            |((configured_base, configured_tag), (installed_base, installed_tag))| {
                configured_base == installed_base && configured_tag == installed_tag
            },
        )
}

fn is_ollama_embedding_model(model: &serde_json::Value) -> bool {
    let has_embedding_marker = |value: &str| {
        let normalized = value.trim().to_ascii_lowercase();
        normalized.contains("embed")
            || normalized.contains("embedding")
            || normalized.contains("text-embedding")
            || normalized == "bert"
    };

    if model["name"].as_str().is_some_and(has_embedding_marker) {
        return true;
    }

    let details = &model["details"];
    if details["family"].as_str().is_some_and(has_embedding_marker) {
        return true;
    }

    details["families"].as_array().is_some_and(|families| {
        families
            .iter()
            .filter_map(|family| family.as_str())
            .any(has_embedding_marker)
    })
}

fn ollama_show_response_supports_completion(data: &serde_json::Value) -> Option<bool> {
    data["capabilities"].as_array().map(|capabilities| {
        capabilities
            .iter()
            .filter_map(|capability| capability.as_str())
            .any(|capability| capability.eq_ignore_ascii_case("completion"))
    })
}

fn ollama_model_should_be_listed(
    model: &serde_json::Value,
    show_response: Option<&serde_json::Value>,
) -> bool {
    match show_response.and_then(ollama_show_response_supports_completion) {
        Some(supports_completion) => supports_completion,
        None => !is_ollama_embedding_model(model),
    }
}

#[allow(dead_code)]
fn parse_ollama_model_names(data: &serde_json::Value) -> Result<Vec<String>, AppError> {
    let models = data["models"]
        .as_array()
        .ok_or_else(|| AppError::Unknown("无法获取 Ollama 模型列表".to_string()))?;

    let mut names = models
        .iter()
        .filter(|model| !is_ollama_embedding_model(model))
        .filter_map(|model| model["name"].as_str().map(|name| name.trim().to_string()))
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();

    names.sort();
    names.dedup();

    Ok(names)
}

async fn fetch_ollama_show_response(
    client: &reqwest::Client,
    endpoint: &str,
    model_name: &str,
) -> Result<serde_json::Value, AppError> {
    let response = client
        .post(format!("{endpoint}/api/show"))
        .json(&serde_json::json!({
            "model": model_name,
            "verbose": false
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(AppError::Analysis(format!(
            "Ollama 模型详情返回错误: {}",
            response.status()
        )));
    }

    Ok(response.json().await?)
}

async fn resolve_ollama_text_model_names(
    client: &reqwest::Client,
    endpoint: &str,
    data: &serde_json::Value,
) -> Result<Vec<String>, AppError> {
    let models = data["models"]
        .as_array()
        .ok_or_else(|| AppError::Unknown("无法获取 Ollama 模型列表".to_string()))?;

    let mut join_set = tokio::task::JoinSet::new();
    for model in models {
        let Some(model_name) = model["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };

        let client = client.clone();
        let endpoint = endpoint.to_string();
        let model_name = model_name.to_string();
        let model_snapshot = model.clone();
        join_set.spawn(async move {
            let show_response = fetch_ollama_show_response(&client, &endpoint, &model_name).await;
            (model_snapshot, model_name, show_response)
        });
    }

    let mut filtered_names = Vec::new();
    while let Some(result) = join_set.join_next().await {
        let (model, model_name, show_response) = result
            .map_err(|error| AppError::Unknown(format!("查询 Ollama 模型详情失败: {error}")))?;

        match show_response {
            Ok(show_response) => {
                if ollama_model_should_be_listed(&model, Some(&show_response)) {
                    filtered_names.push(model_name);
                }
            }
            Err(error) => {
                if ollama_model_should_be_listed(&model, None) {
                    log::debug!(
                        "获取 Ollama 模型详情失败，回退名称规则后保留模型: model={model_name}, error={error}"
                    );
                    filtered_names.push(model_name);
                } else {
                    log::debug!(
                        "获取 Ollama 模型详情失败，回退名称规则后排除模型: model={model_name}, error={error}"
                    );
                }
            }
        }
    }

    filtered_names.sort();
    filtered_names.dedup();
    Ok(filtered_names)
}


/// 从 OpenAI 兼容提供商获取模型列表
async fn fetch_openai_compatible_models(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<Vec<String>, AppError> {
    let base = endpoint.trim().trim_end_matches('/');
    let url = format!("{base}/models");
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await;

    let response = match response {
        Ok(r) if r.status().is_success() => r,
        Ok(_) => {
            // 端点可能不含版本号前缀，重试 {endpoint}/v1/models。
            // 但如果已含版本段（火山引擎 /api/v3、智谱 /api/paas/v4），
            // 不再追加 /v1（会拼出 /api/v3/v1/models 这种不存在的路径）。
            if endpoint_has_version_segment(base) {
                return Err(AppError::Analysis(format!(
                    "无法获取模型列表，请确认端点地址正确：{base}/models 返回错误"
                )));
            }
            let retry_url = format!("{base}/v1/models");
            let retry = client
                .get(&retry_url)
                .header("Authorization", format!("Bearer {api_key}"))
                .send()
                .await
                .map_err(|e| AppError::Analysis(format!("无法获取模型列表: {e}")))?;
            if !retry.status().is_success() {
                return Err(AppError::Analysis(format!(
                    "API 返回错误: {}",
                    retry.status()
                )));
            }
            retry
        }
        Err(e) => return Err(AppError::Analysis(format!("请求失败: {e}"))),
    };

    let data: serde_json::Value = response.json().await?;
    let models = data["data"]
        .as_array()
        .ok_or_else(|| AppError::Analysis("无法解析模型列表（缺少 data 字段）".to_string()))?;

    let mut names: Vec<String> = models
        .iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .collect();

    names.sort();
    names.dedup();
    Ok(names)
}

/// 从 Google Gemini 获取模型列表
async fn fetch_gemini_models(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<Vec<String>, AppError> {
    // API Key 走请求头而非 URL query，避免泄漏到日志/代理记录
    let url = format!("{endpoint}/models");
    let response = client
        .get(&url)
        .header("x-goog-api-key", api_key)
        .send()
        .await
        .map_err(|e| AppError::Analysis(format!("无法连接到 Gemini 服务: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(AppError::Analysis(format!(
            "Gemini API 错误 ({status}): {error_text}"
        )));
    }

    let data: serde_json::Value = response.json().await?;
    let models = data["models"]
        .as_array()
        .ok_or_else(|| AppError::Analysis("无法解析 Gemini 模型列表".to_string()))?;

    let mut names: Vec<String> = models
        .iter()
        // 仅保留支持 generateContent 的模型（排除 embedding 等专用模型）
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .map(|methods| {
                    methods
                        .iter()
                        .any(|m| m.as_str() == Some("generateContent"))
                })
                .unwrap_or(true)
        })
        .filter_map(|m| {
            m["name"]
                .as_str()
                .map(|name| name.strip_prefix("models/").unwrap_or(name).to_string())
        })
        .filter(|s| !s.is_empty())
        .collect();

    names.sort();
    names.dedup();
    Ok(names)
}

/// 从 Anthropic Claude 获取模型列表
async fn fetch_claude_models(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<Vec<String>, AppError> {
    let url = format!("{endpoint}/models");
    let response = client
        .get(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|e| AppError::Analysis(format!("无法连接到 Claude 服务: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(AppError::Analysis(format!(
            "Claude API 错误 ({status}): {error_text}"
        )));
    }

    let data: serde_json::Value = response.json().await?;
    let models = data["data"]
        .as_array()
        .ok_or_else(|| AppError::Analysis("无法解析 Claude 模型列表".to_string()))?;

    let mut names: Vec<String> = models
        .iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .collect();

    names.sort();
    names.dedup();
    Ok(names)
}

/// 通用获取模型列表（支持所有提供商）
#[tauri::command]
pub async fn fetch_models(
    provider: String,
    endpoint: String,
    api_key: Option<String>,
) -> Result<Vec<String>, AppError> {
    let endpoint = endpoint.trim().trim_end_matches('/').to_string();
    if endpoint.is_empty() {
        return Err(AppError::Config("API 地址不能为空".to_string()));
    }

    let provider: crate::config::AiProvider =
        serde_json::from_value(serde_json::Value::String(provider))
            .map_err(|_| AppError::Config("未知的 AI 提供商类型".to_string()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    match provider {
        crate::config::AiProvider::Ollama => {
            let response = client
                .get(format!("{endpoint}/api/tags"))
                .send()
                .await
                .map_err(|e| AppError::Analysis(format!("无法连接到 Ollama 服务: {e}")))?;
            if !response.status().is_success() {
                return Err(AppError::Analysis(format!(
                    "Ollama 服务返回错误: {}",
                    response.status()
                )));
            }
            let data: serde_json::Value = response.json().await?;
            resolve_ollama_text_model_names(&client, &endpoint, &data).await
        }
        crate::config::AiProvider::Gemini => {
            let api_key = api_key
                .filter(|k| !k.is_empty())
                .ok_or(AppError::Config("Gemini 需要 API Key".to_string()))?;
            fetch_gemini_models(&client, &endpoint, &api_key).await
        }
        crate::config::AiProvider::Claude => {
            let api_key = api_key
                .filter(|k| !k.is_empty())
                .ok_or(AppError::Config("Claude 需要 API Key".to_string()))?;
            fetch_claude_models(&client, &endpoint, &api_key).await
        }
        _ if provider.is_openai_compatible() => {
            let api_key = api_key
                .filter(|k| !k.is_empty())
                .ok_or(AppError::Config("需要 API Key".to_string()))?;
            fetch_openai_compatible_models(&client, &endpoint, &api_key).await
        }
        _ => Err(AppError::Config("不支持的提供商类型".to_string())),
    }
}

/// 获取支持的 AI 提供商列表
/// 测试助手联网搜索配置：按当前服务商发一次最小搜索请求。
/// 不要求先启用总开关（用户通常想先测通再开启）。
#[tauri::command]
pub async fn test_assistant_search(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, AppError> {
    let (provider, api_key) = {
        let s = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
        (
            s.config.assistant_search_provider.clone(),
            s.config
                .assistant_search_api_key
                .clone()
                .unwrap_or_default(),
        )
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| AppError::Unknown(format!("创建 HTTP 客户端失败: {e}")))?;

    let started = std::time::Instant::now();
    let count: usize = match provider.as_str() {
        "duckduckgo" => {
            // 免费方案实际请求 Bing：拿到结果页且包含结果标题即视为可用
            let resp = client
                .get("https://www.bing.com/search")
                .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36")
                .query(&[("q", "connection test"), ("count", "3")])
                .send()
                .await
                .map_err(|e| AppError::Analysis(format!("搜索请求失败: {e}")))?;
            if !resp.status().is_success() {
                return Err(AppError::Analysis(format!(
                    "搜索服务返回 HTTP {}",
                    resp.status()
                )));
            }
            let html = resp
                .text()
                .await
                .map_err(|e| AppError::Analysis(format!("搜索响应读取失败: {e}")))?;
            if html.contains("<h2") { 1 } else { 0 }
        }
        "bocha" => {
            if api_key.trim().is_empty() {
                return Err(AppError::Config("请先填写博查 API Key".to_string()));
            }
            let resp: serde_json::Value = client
                .post("https://api.bochaai.com/v1/web-search")
                .header("Authorization", format!("Bearer {}", api_key.trim()))
                .json(&serde_json::json!({ "query": "connection test", "count": 1 }))
                .send()
                .await
                .map_err(|e| AppError::Analysis(format!("搜索请求失败: {e}")))?
                .json()
                .await
                .map_err(|e| AppError::Analysis(format!("搜索结果解析失败: {e}")))?;
            resp["data"]["webPages"]["value"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0)
        }
        _ => {
            if api_key.trim().is_empty() {
                return Err(AppError::Config("请先填写 Tavily API Key".to_string()));
            }
            let resp: serde_json::Value = client
                .post("https://api.tavily.com/search")
                .json(&serde_json::json!({
                    "api_key": api_key.trim(),
                    "query": "connection test",
                    "max_results": 1
                }))
                .send()
                .await
                .map_err(|e| AppError::Analysis(format!("搜索请求失败: {e}")))?
                .json()
                .await
                .map_err(|e| AppError::Analysis(format!("搜索结果解析失败: {e}")))?;
            if let Some(err) = resp["error"].as_str() {
                return Err(AppError::Analysis(format!("搜索服务返回错误: {err}")));
            }
            resp["results"].as_array().map(|a| a.len()).unwrap_or(0)
        }
    };

    if count == 0 {
        return Err(AppError::Analysis(
            "搜索请求成功但未返回结果，请检查配置".to_string(),
        ));
    }
    Ok(serde_json::json!({
        "resultCount": count,
        "latencyMs": started.elapsed().as_millis() as u64,
    }))
}

#[tauri::command]
pub async fn get_ai_providers() -> Result<Vec<serde_json::Value>, AppError> {
    Ok(vec![
        serde_json::json!({
            "id": "ollama",
            "name": "Ollama (本地)",
            "description": "在本机运行的开源大模型，数据不出本机",
            "default_endpoint": "http://localhost:11434",
            "default_model": "qwen3",
            "requires_api_key": false,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "openai",
            "name": "OpenAI / 兼容API",
            "description": "支持 OpenAI 官方及兼容 API（Azure、Cloudflare 等）",
            "default_endpoint": "https://api.openai.com/v1",
            "default_model": "gpt-5.1",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "atlascloud",
            "name": "Atlas Cloud",
            "description": "OpenAI 兼容的多模型文本推理 API",
            "default_endpoint": "https://api.atlascloud.ai/v1",
            "default_model": "deepseek-ai/deepseek-v4-pro",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "siliconflow",
            "name": "硅基流动 SiliconFlow",
            "description": "国内高性价比 API，兼容 OpenAI 格式",
            "default_endpoint": "https://api.siliconflow.cn/v1",
            "default_model": "Qwen/Qwen3-8B",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "deepseek",
            "name": "DeepSeek",
            "description": "国产开源模型，性能强劲，兼容 OpenAI 格式",
            "default_endpoint": "https://api.deepseek.com",
            "default_model": "deepseek-v4-flash",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "qwen",
            "name": "通义千问 Qwen",
            "description": "阿里云通义大模型，兼容 OpenAI 格式",
            "default_endpoint": "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "default_model": "qwen-flash",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "zhipu",
            "name": "智谱 ChatGLM",
            "description": "智谱 AI 大模型",
            "default_endpoint": "https://open.bigmodel.cn/api/paas/v4",
            "default_model": "glm-4.6",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "moonshot",
            "name": "月之暗面 Kimi",
            "description": "Moonshot AI，擅长长文本",
            "default_endpoint": "https://api.moonshot.cn/v1",
            "default_model": "moonshot-v1-8k",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "doubao",
            "name": "火山引擎 豆包",
            "description": "字节跳动大模型",
            "default_endpoint": "https://ark.cn-beijing.volces.com/api/v3",
            "default_model": "doubao-lite-4k",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "minimax",
            "name": "稀宇科技 MiniMax",
            "description": "MiniMax 文本模型，兼容 OpenAI 格式",
            "default_endpoint": "https://api.minimaxi.com/v1",
            "default_model": "MiniMax-M2.5",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "openrouter",
            "name": "OpenRouter",
            "description": "多模型聚合网关，一个 Key 调用上百个模型",
            "default_endpoint": "https://openrouter.ai/api/v1",
            "default_model": "openrouter/auto",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "groq",
            "name": "Groq",
            "description": "超高速推理，兼容 OpenAI 格式",
            "default_endpoint": "https://api.groq.com/openai/v1",
            "default_model": "llama-3.3-70b-versatile",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "xai",
            "name": "xAI Grok",
            "description": "xAI 的 Grok 系列模型，兼容 OpenAI 格式",
            "default_endpoint": "https://api.x.ai/v1",
            "default_model": "grok-2-latest",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "mistral",
            "name": "Mistral",
            "description": "Mistral AI 系列模型，兼容 OpenAI 格式",
            "default_endpoint": "https://api.mistral.ai/v1",
            "default_model": "mistral-small-latest",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "lmstudio",
            "name": "LM Studio (本地)",
            "description": "本机运行的 LM Studio 服务，数据不出本机",
            "default_endpoint": "http://localhost:1234/v1",
            "default_model": "local-model",
            "requires_api_key": false,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "custom",
            "name": "自定义接口",
            "description": "任何 OpenAI 兼容接口，自行填写地址与模型",
            "default_endpoint": "",
            "default_model": "",
            "requires_api_key": false,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "gemini",
            "name": "Google Gemini",
            "description": "Google 的 Gemini 系列模型",
            "default_endpoint": "https://generativelanguage.googleapis.com/v1",
            "default_model": "gemini-2.5-flash",
            "requires_api_key": true,
            "supports_vision": false,
        }),
        serde_json::json!({
            "id": "claude",
            "name": "Anthropic Claude",
            "description": "Anthropic 的 Claude 系列模型",
            "default_endpoint": "https://api.anthropic.com/v1",
            "default_model": "claude-3-7-sonnet-latest",
            "requires_api_key": true,
            "supports_vision": false,
        }),
    ])
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai兼容探测请求的输出上限不应低于十六() {
        assert_eq!(openai_connection_test_max_tokens(), 16);
    }

    #[test]
    fn 模型端点应强制远程https仅放行本机内网http() {
        assert!(validate_model_endpoint("https://api.openai.com/v1").is_ok());
        assert!(validate_model_endpoint("").is_ok());
        assert!(validate_model_endpoint("http://localhost:11434").is_ok());
        assert!(validate_model_endpoint("http://127.0.0.1:11434").is_ok());
        assert!(validate_model_endpoint("http://[::1]:11434").is_ok());
        assert!(validate_model_endpoint("http://192.168.1.5:11434").is_ok());
        assert!(validate_model_endpoint("http://10.1.2.3:8080/v1").is_ok());
        assert!(validate_model_endpoint("http://172.31.0.1/v1").is_ok());

        assert!(validate_model_endpoint("http://api.example.com/v1").is_err());
        assert!(validate_model_endpoint("http://8.8.8.8/v1").is_err());
        // 172.32.x 不属于私网段
        assert!(validate_model_endpoint("http://172.32.0.1/v1").is_err());
    }

    #[test]
    fn openai兼容端点应自动补齐_chat_completions_并支持_v1_回退() {
        // 无版本号的端点（DeepSeek）：尝试两个候选
        assert_eq!(
            openai_compatible_chat_completion_urls("https://api.deepseek.com"),
            vec![
                "https://api.deepseek.com/chat/completions".to_string(),
                "https://api.deepseek.com/v1/chat/completions".to_string()
            ]
        );
        // 已含 /v1：直接拼，不再回退
        assert_eq!(
            openai_compatible_chat_completion_urls("https://api.openai.com/v1"),
            vec!["https://api.openai.com/v1/chat/completions".to_string()]
        );
    }

    #[test]
    fn 火山引擎端点不应回退_v1_路径() {
        // 火山引擎 api/v3 已是完整版本路径，不应再追加 /v1（会拼出
        // /api/v3/v1/chat/completions 这种不存在的端点——issue 用户报错根因）。
        assert_eq!(
            openai_compatible_chat_completion_urls("https://ark.cn-beijing.volces.com/api/v3"),
            vec![
                "https://ark.cn-beijing.volces.com/api/v3/chat/completions".to_string()
            ]
        );
    }

    #[test]
    fn 智谱端点不应回退_v1_路径() {
        // 智谱 /api/paas/v4 同理，已含版本段
        assert_eq!(
            openai_compatible_chat_completion_urls("https://open.bigmodel.cn/api/paas/v4"),
            vec![
                "https://open.bigmodel.cn/api/paas/v4/chat/completions".to_string()
            ]
        );
    }

    #[test]
    fn 已含_chat_completions的端点不再拼接() {
        // 用户直接填了完整端点：原样使用
        assert_eq!(
            openai_compatible_chat_completion_urls("https://example.com/v1/chat/completions"),
            vec!["https://example.com/v1/chat/completions".to_string()]
        );
    }

    #[test]
    fn 应能解析_ollama_模型列表响应() {
        let payload = serde_json::json!({
            "models": [
                { "name": "qwen2.5:latest" },
                { "name": "llama3.1:8b" },
                { "name": "qwen2.5:latest" }
            ]
        });

        let names = parse_ollama_model_names(&payload).expect("应能解析模型列表");

        assert_eq!(
            names,
            vec!["llama3.1:8b".to_string(), "qwen2.5:latest".to_string()]
        );
    }

    #[test]
    fn 解析_ollama_模型列表时应过滤嵌入模型() {
        let payload = serde_json::json!({
            "models": [
                { "name": "qwen3.5:4b" },
                { "name": "nomic-embed-text:latest" },
                { "name": "llama3.2:latest" }
            ]
        });

        let names = parse_ollama_model_names(&payload).expect("应能解析模型列表");

        assert_eq!(
            names,
            vec!["llama3.2:latest".to_string(), "qwen3.5:4b".to_string()]
        );
    }

    #[test]
    fn ollama_show_响应应根据能力判断是否支持文本生成() {
        let embedding_only = serde_json::json!({
            "capabilities": ["embedding"]
        });
        let completion_and_vision = serde_json::json!({
            "capabilities": ["completion", "vision"]
        });
        let missing_capabilities = serde_json::json!({});

        assert_eq!(
            ollama_show_response_supports_completion(&embedding_only),
            Some(false)
        );
        assert_eq!(
            ollama_show_response_supports_completion(&completion_and_vision),
            Some(true)
        );
        assert_eq!(
            ollama_show_response_supports_completion(&missing_capabilities),
            None
        );
    }

    #[test]
    fn ollama_模型名匹配应兼容_latest_缩写且避免宽松子串误判() {
        assert!(ollama_model_names_match("qwen2.5", "qwen2.5:latest"));
        assert!(ollama_model_names_match(
            "hf.co/Qwen/Qwen3-8B-GGUF:Q5_K_M",
            "hf.co/Qwen/Qwen3-8B-GGUF:Q5_K_M"
        ));
        assert!(!ollama_model_names_match("qwen2.5", "qwen2.5-coder:latest"));
        assert!(!ollama_model_names_match("qwen2.5", "deepseek-r1:1.5b"));
    }

    #[test]
    fn ollama_模型展示应优先相信能力信息再回退名称规则() {
        let suspicious_but_completion = serde_json::json!({
            "name": "embed-chat-preview:latest"
        });
        let completion_show = serde_json::json!({
            "capabilities": ["completion"]
        });

        let embedding_only = serde_json::json!({
            "name": "all-minilm:latest"
        });
        let embedding_show = serde_json::json!({
            "capabilities": ["embedding"]
        });

        let heuristic_only = serde_json::json!({
            "name": "nomic-embed-text:latest"
        });

        assert!(ollama_model_should_be_listed(
            &suspicious_but_completion,
            Some(&completion_show)
        ));
        assert!(!ollama_model_should_be_listed(
            &embedding_only,
            Some(&embedding_show)
        ));
        assert!(!ollama_model_should_be_listed(&heuristic_only, None));
    }

}
