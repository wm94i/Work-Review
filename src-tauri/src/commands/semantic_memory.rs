//! 语义记忆（屏幕级数字记忆）。
//!
//! 把已采集的 OCR 文本、窗口标题和 URL 聚合为记忆块，经嵌入模型向量化后支持
//! 语义检索。索引与召回均执行当前完整隐私规则；任何状态或指纹不一致都会关闭
//! 向量召回并降级为关键词检索，避免使用过期或不符合当前隐私配置的向量。

use crate::AppState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;
use work_review_core::config::{AppConfig, PrivacyConfig};
use work_review_core::database::{
    encode_embedding, normalize_embedding, Activity, Database, MemorySearchItem, SemanticMemoryHit,
    SemanticMemoryIndexState, SemanticMemoryStats, SEMANTIC_MEMORY_CURSOR_CHUNK_KEY,
};
use work_review_core::error::AppError;
use work_review_core::privacy::PrivacyAction;

/// 单轮索引最多消费的活动行数（前端循环调用直至完成）。
const INDEX_ACTIVITY_BATCH: usize = 1500;
/// 单轮索引最多嵌入的块数（控制单次调用时长）。
const EMBED_BATCH: usize = 32;
/// 单轮增量索引最多按日重算的脏日期数量。
const REINDEX_DATES_BATCH: usize = 2;
/// 记忆块正文上限（字符）。
const CHUNK_CONTENT_MAX_CHARS: usize = 1200;
/// 分块规则变化时必须重建索引。
pub(crate) const CHUNK_RULE_VERSION: &str = "semantic-chunk-v2";
/// 向量归一化规则变化时必须重建索引。
pub(crate) const NORMALIZATION_VERSION: &str = "l2-v1";

/// 单飞行互斥锁：避免手动索引与后台自动索引并发抢占同一 `build_id` 或重复嵌入。
static SEMANTIC_INDEX_SINGLE_FLIGHT: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SemanticIndexProgress {
    /// 本轮消费的活动行数。
    pub processed_activities: usize,
    /// 本轮新写入或更新的记忆块数。
    pub upserted_chunks: usize,
    /// 本轮完成嵌入的块数。
    pub embedded_chunks: usize,
    /// 仍待嵌入的块数。
    pub pending_embeddings: usize,
    /// 活动游标是否已追平。
    pub activities_done: bool,
    /// 兼容既有调用方的块统计。
    pub stats: SemanticMemoryStats,
    /// 真实持久化索引状态。
    pub state: SemanticMemoryIndexState,
}

#[derive(Clone, Debug)]
struct EmbeddingConfig {
    provider: String,
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

#[derive(Clone, Debug)]
struct SemanticBuildToken {
    build_id: String,
    embedding_fingerprint: String,
    privacy_fingerprint: String,
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// 规范化嵌入地址。只删除无语义的首尾空白、fragment 和末尾斜杠，
/// 不强行修改路径大小写，避免把实际不同的服务地址视为同一配置。
pub(crate) fn normalize_embedding_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim();
    if let Ok(mut url) = reqwest::Url::parse(trimmed) {
        url.set_fragment(None);
        return url.to_string().trim_end_matches('/').to_string();
    }
    trimmed.trim_end_matches('/').to_string()
}

fn embedding_config_fingerprint_from_values(provider: &str, endpoint: &str, model: &str) -> String {
    let canonical = format!(
        "provider={}\nendpoint={}\nmodel={}\nchunk={}\nnormalization={}",
        provider.trim().to_ascii_lowercase(),
        normalize_embedding_endpoint(endpoint),
        model.trim(),
        CHUNK_RULE_VERSION,
        NORMALIZATION_VERSION,
    );
    sha256_hex(&canonical)
}

/// 配置级指纹不包含 API Key，也不包含运行时探测到的向量维度。
/// 用于配置保存时判断是否需要立即使旧索引失效。
pub(crate) fn embedding_config_fingerprint(config: &AppConfig) -> String {
    embedding_config_fingerprint_from_values(
        &config.embedding_provider,
        &config.embedding_endpoint,
        &config.embedding_model,
    )
}

fn embedding_config_fingerprint_for_runtime(config: &EmbeddingConfig) -> String {
    embedding_config_fingerprint_from_values(&config.provider, &config.endpoint, &config.model)
}

/// 最终索引指纹包含真实向量维度。格式保留配置指纹前缀，便于在不调用服务时
/// 先检查 Provider、Endpoint、模型和规则版本是否一致。
fn embedding_fingerprint(config: &EmbeddingConfig, dimension: usize) -> String {
    format!(
        "{}:{dimension}",
        embedding_config_fingerprint_for_runtime(config)
    )
}

fn normalize_unordered_fingerprint_values(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut normalized: Vec<String> = values.into_iter().collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

/// 隐私指纹必须与真实判定语义一致：应用规则采用“首个匹配项”语义，因此保留顺序；
/// 其余集合规则排序去重。结构化 JSON 编码避免分隔符造成规范化碰撞。
pub(crate) fn privacy_fingerprint(config: &PrivacyConfig) -> String {
    let app_rules: Vec<serde_json::Value> = config
        .app_rules
        .iter()
        .map(|rule| {
            serde_json::json!({
                "appName": work_review_core::categorize::normalize_display_app_name(&rule.app_name)
                    .to_lowercase(),
                "level": rule.level,
            })
        })
        .collect();
    let excluded_apps =
        normalize_unordered_fingerprint_values(config.excluded_apps.iter().map(|app| {
            work_review_core::categorize::normalize_display_app_name(app).to_lowercase()
        }));
    let excluded_keywords = normalize_unordered_fingerprint_values(
        config
            .excluded_keywords
            .iter()
            .map(|keyword| keyword.to_lowercase()),
    );
    let excluded_domains = normalize_unordered_fingerprint_values(
        config
            .excluded_domains
            .iter()
            .map(|domain| PrivacyConfig::extract_domain(domain))
            .filter(|domain| !domain.is_empty()),
    );
    let canonical = serde_json::json!({
        "appRules": app_rules,
        "excludedApps": excluded_apps,
        "excludedKeywords": excluded_keywords,
        "excludedDomains": excluded_domains,
        "skipPrivateWindows": config.skip_private_windows,
        "unknownUrlFallback": config.effective_unknown_url_fallback(),
    });
    sha256_hex(&serde_json::to_string(&canonical).expect("隐私指纹 JSON 序列化不应失败"))
}

pub(crate) fn invalidate_semantic_memory_index(database: &Database) -> Result<(), AppError> {
    database.clear_memory_chunks_and_mark_rebuild_required()
}

fn load_embedding_config(state: &Arc<Mutex<AppState>>) -> Result<EmbeddingConfig, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    if !state.config.memory_semantic_enabled {
        return Err(AppError::Config(
            "语义记忆未启用，请到「设置 → AI 模型」开启".to_string(),
        ));
    }
    Ok(EmbeddingConfig {
        provider: state.config.embedding_provider.trim().to_ascii_lowercase(),
        endpoint: normalize_embedding_endpoint(&state.config.embedding_endpoint),
        model: state.config.embedding_model.trim().to_string(),
        api_key: state.config.embedding_api_key.clone(),
    })
}

/// 嵌入成功响应体上限（16 MiB，足以容纳单批 32 块 × 高维浮点 JSON，同时防止恶意服务端 OOM）。
const MAX_EMBEDDING_JSON_BYTES: usize = 16 * 1024 * 1024;
/// 嵌入错误响应体读取上限（8 KiB，先限流读取再截断，避免先全量读入内存）。
const MAX_EMBEDDING_ERROR_BYTES: usize = 8 * 1024;

/// 生产环境复用带连接池的 HTTP 客户端，减少分批嵌入时重复建连开销；
/// 显式禁用自动重定向（防止 HTTPS→HTTP 降级、SSRF 与 Bearer Token 泄露）；
/// 本机回环地址绕过系统代理，避免全局代理工具拦截本机 Ollama / LM Studio / llama.cpp 请求。
#[cfg(not(test))]
static EMBEDDING_REMOTE_HTTP_CLIENT: once_cell::sync::Lazy<Result<reqwest::Client, String>> =
    once_cell::sync::Lazy::new(|| build_embedding_http_client(false));

#[cfg(not(test))]
static EMBEDDING_LOCAL_HTTP_CLIENT: once_cell::sync::Lazy<Result<reqwest::Client, String>> =
    once_cell::sync::Lazy::new(|| build_embedding_http_client(true));

fn build_embedding_http_client(bypass_proxy: bool) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none());
    if bypass_proxy {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|e| format!("创建嵌入 HTTP 客户端失败: {e}"))
}

fn is_loopback_embedding_endpoint(endpoint: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(endpoint.trim()) else {
        return false;
    };
    let Some(host) = parsed
        .host_str()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']'))
    else {
        return false;
    };
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return true;
    }
    lower
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

async fn read_response_bytes_bounded(
    mut response: reqwest::Response,
    max_bytes: usize,
    truncate_on_limit: bool,
) -> Result<Vec<u8>, AppError> {
    if !truncate_on_limit
        && response
            .content_length()
            .is_some_and(|len| len > max_bytes as u64)
    {
        return Err(AppError::Analysis(format!(
            "嵌入响应体积超过上限（>{max_bytes} 字节）"
        )));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| AppError::Analysis(format!("读取嵌入响应失败: {e}")))?
    {
        let remaining = max_bytes.saturating_sub(buf.len());
        if chunk.len() > remaining {
            if truncate_on_limit {
                buf.extend_from_slice(&chunk[..remaining]);
                break;
            }
            return Err(AppError::Analysis(format!(
                "嵌入响应体积超过上限（>{max_bytes} 字节）"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn read_error_body_snippet(response: reqwest::Response) -> String {
    match read_response_bytes_bounded(response, MAX_EMBEDDING_ERROR_BYTES, true).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).chars().take(300).collect(),
        Err(_) => String::new(),
    }
}

async fn read_json_bounded(
    response: reqwest::Response,
    context: &str,
) -> Result<serde_json::Value, AppError> {
    let bytes = read_response_bytes_bounded(response, MAX_EMBEDDING_JSON_BYTES, false).await?;
    serde_json::from_slice(&bytes).map_err(|e| AppError::Analysis(format!("{context}: {e}")))
}

fn embedding_client(endpoint: &str) -> Result<reqwest::Client, AppError> {
    let loopback = is_loopback_embedding_endpoint(endpoint);
    #[cfg(not(test))]
    {
        let lazy = if loopback {
            &EMBEDDING_LOCAL_HTTP_CLIENT
        } else {
            &EMBEDDING_REMOTE_HTTP_CLIENT
        };
        lazy.as_ref()
            .cloned()
            .map_err(|e| AppError::Unknown(e.clone()))
    }
    #[cfg(test)]
    {
        // 单测每个 #[tokio::test] 使用独立 runtime，避免跨 runtime 共享连接池。
        build_embedding_http_client(loopback).map_err(AppError::Unknown)
    }
}

fn is_api_version_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 2
        && bytes[0] == b'v'
        && bytes[1].is_ascii_digit()
        && bytes[1..].iter().all(u8::is_ascii_alphanumeric)
}

/// 构造 OpenAI 兼容 `/embeddings` 请求地址：
/// - 已以 `/embeddings` 结尾时直接复用（保留 query 参数）；
/// - 路径已含版本段（如 `/v1`、`/v3`、`/v4`、`/v1beta`）时直接追加 `/embeddings`；
/// - 其余默认追加 `/v1/embeddings`（兼容 LM Studio、llama.cpp、SiliconFlow、OpenAI 等）。
pub(crate) fn resolve_openai_embeddings_url(endpoint: &str) -> String {
    let normalized = normalize_embedding_endpoint(endpoint);
    if let Ok(mut url) = reqwest::Url::parse(&normalized) {
        let trimmed_path = url.path().trim_end_matches('/').to_string();
        if trimmed_path.ends_with("/embeddings") {
            url.set_path(&trimmed_path);
            return url.to_string();
        }
        let has_version = trimmed_path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .any(is_api_version_segment);
        let next_path = if has_version {
            format!("{trimmed_path}/embeddings")
        } else {
            format!("{trimmed_path}/v1/embeddings")
        };
        url.set_path(&next_path);
        return url.to_string();
    }

    let trimmed = normalized.trim_end_matches('/');
    if trimmed.ends_with("/embeddings") {
        trimmed.to_string()
    } else if trimmed
        .split('/')
        .filter(|segment| !segment.is_empty())
        .any(is_api_version_segment)
    {
        format!("{trimmed}/embeddings")
    } else {
        format!("{trimmed}/v1/embeddings")
    }
}

fn resolve_ollama_base_url(endpoint: &str) -> String {
    let normalized = normalize_embedding_endpoint(endpoint);
    if let Ok(mut url) = reqwest::Url::parse(&normalized) {
        let stripped = {
            let trimmed_path = url.path().trim_end_matches('/');
            trimmed_path
                .strip_suffix("/api/embeddings")
                .or_else(|| trimmed_path.strip_suffix("/api/embed"))
                .unwrap_or(trimmed_path)
                .trim_end_matches('/')
                .to_string()
        };
        url.set_path(&stripped);
        return url.to_string().trim_end_matches('/').to_string();
    }

    let trimmed = normalized.trim_end_matches('/');
    trimmed
        .strip_suffix("/api/embeddings")
        .or_else(|| trimmed.strip_suffix("/api/embed"))
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_string()
}

fn parse_and_normalize_embedding(
    value: &serde_json::Value,
    context: &str,
) -> Result<Vec<f32>, AppError> {
    let array = value
        .as_array()
        .ok_or_else(|| AppError::Analysis(format!("{context}缺少向量数组")))?;
    if array.is_empty() {
        return Err(AppError::Analysis(format!("{context}返回了空向量")));
    }
    let mut vector = Vec::with_capacity(array.len());
    let mut raw_norm_sq = 0.0_f64;
    for item in array {
        let number = item
            .as_f64()
            .ok_or_else(|| AppError::Analysis(format!("{context}包含非数值向量元素")))?;
        let element = number as f32;
        if !element.is_finite() {
            return Err(AppError::Analysis(format!("{context}包含非法浮点向量元素")));
        }
        raw_norm_sq += f64::from(element) * f64::from(element);
        vector.push(element);
    }
    if !raw_norm_sq.is_finite() || raw_norm_sq <= f64::from(f32::EPSILON) {
        return Err(AppError::Analysis(format!("{context}返回了零向量")));
    }
    let normalized = normalize_embedding(vector);
    let norm_sq: f64 = normalized
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum();
    if !normalized.iter().all(|v| v.is_finite())
        || !norm_sq.is_finite()
        || norm_sq <= f64::from(f32::EPSILON)
    {
        return Err(AppError::Analysis(format!(
            "{context}归一化后得到无效或零向量"
        )));
    }
    Ok(normalized)
}

fn parse_openai_embeddings_response(
    payload: &serde_json::Value,
    expected_len: usize,
) -> Result<Vec<Vec<f32>>, AppError> {
    let data = payload["data"]
        .as_array()
        .ok_or_else(|| AppError::Analysis("嵌入响应缺少 data 字段".to_string()))?;
    if data.len() != expected_len {
        return Err(AppError::Analysis(format!(
            "嵌入结果数量不匹配: 期望 {} 实际 {}",
            expected_len,
            data.len()
        )));
    }

    let any_index = data.iter().any(|item| !item["index"].is_null());
    if any_index {
        let mut ordered: Vec<Option<Vec<f32>>> = vec![None; expected_len];
        for item in data {
            let idx = item["index"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| AppError::Analysis("嵌入响应 index 字段无效".to_string()))?;
            if idx >= expected_len || ordered[idx].is_some() {
                return Err(AppError::Analysis(format!(
                    "嵌入响应 index 越界或重复: {idx}"
                )));
            }
            ordered[idx] = Some(parse_and_normalize_embedding(
                &item["embedding"],
                "嵌入响应",
            )?);
        }
        return ordered
            .into_iter()
            .map(|slot| slot.ok_or_else(|| AppError::Analysis("嵌入响应缺少部分向量".to_string())))
            .collect();
    }

    let mut vectors = Vec::with_capacity(expected_len);
    for item in data {
        vectors.push(parse_and_normalize_embedding(
            &item["embedding"],
            "嵌入响应",
        )?);
    }
    Ok(vectors)
}

fn is_ollama_model_not_found(status: reqwest::StatusCode, body: &str) -> bool {
    if status.as_u16() != 404 {
        return false;
    }
    // Ollama 模型缺失统一返回 JSON `{"error":"model \"...\" not found..."}`；
    // 仅解析 JSON error 字段，避免反向代理返回的 HTML 404 页面误判并跳过旧版回退。
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let Some(error_msg) = json["error"].as_str() else {
        return false;
    };
    let lower = error_msg.to_ascii_lowercase();
    lower.contains("model") && lower.contains("not found")
}

/// 旧版 Ollama（< 0.2.x）不支持批量 `/api/embed`，返回 404 page not found 或 405；
/// 需排除“模型未安装”的 404，避免无效回退并直接提示 `ollama pull`。
fn should_fallback_ollama_legacy_endpoint(status: reqwest::StatusCode, body: &str) -> bool {
    matches!(status.as_u16(), 404 | 405) && !is_ollama_model_not_found(status, body)
}

fn ollama_model_not_found_error(model: &str, status: reqwest::StatusCode, body: &str) -> AppError {
    AppError::Analysis(format!(
        "嵌入模型 {model} 未安装：请在终端执行 ollama pull {model} 后重试（原始错误：HTTP {status}: {body}）"
    ))
}

async fn embed_texts_ollama_legacy(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, AppError> {
    let url = format!("{base_url}/api/embeddings");
    let mut vectors = Vec::with_capacity(texts.len());
    for text in texts {
        let response = client
            .post(&url)
            .json(&serde_json::json!({ "model": model, "prompt": text }))
            .send()
            .await
            .map_err(|e| {
                AppError::Analysis(format!(
                    "Ollama 嵌入请求失败（请确认 Ollama 已启动并已拉取模型 {model}）: {e}"
                ))
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = read_error_body_snippet(response).await;
            if is_ollama_model_not_found(status, &body) {
                return Err(ollama_model_not_found_error(model, status, &body));
            }
            return Err(AppError::Analysis(format!(
                "Ollama 嵌入返回 HTTP {status}: {body}"
            )));
        }
        let payload = read_json_bounded(response, "Ollama 嵌入响应解析失败").await?;
        vectors.push(parse_and_normalize_embedding(
            &payload["embedding"],
            "Ollama 嵌入响应",
        )?);
    }
    Ok(vectors)
}

async fn embed_texts_ollama(
    client: &reqwest::Client,
    config: &EmbeddingConfig,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, AppError> {
    let base_url = resolve_ollama_base_url(&config.endpoint);
    let batch_url = format!("{base_url}/api/embed");
    let response = client
        .post(&batch_url)
        .json(&serde_json::json!({ "model": config.model, "input": texts }))
        .send()
        .await
        .map_err(|e| {
            AppError::Analysis(format!(
                "Ollama 嵌入请求失败（请确认 Ollama 已启动并已拉取模型 {}）: {e}",
                config.model
            ))
        })?;

    if response.status().is_success() {
        let payload = read_json_bounded(response, "Ollama 批量嵌入响应解析失败").await?;
        let embeddings = payload["embeddings"].as_array().ok_or_else(|| {
            AppError::Analysis("Ollama 批量嵌入响应缺少 embeddings 字段".to_string())
        })?;
        if embeddings.len() != texts.len() {
            return Err(AppError::Analysis(format!(
                "Ollama 批量嵌入结果数量不匹配: 期望 {} 实际 {}",
                texts.len(),
                embeddings.len()
            )));
        }
        let mut vectors = Vec::with_capacity(texts.len());
        for item in embeddings {
            vectors.push(parse_and_normalize_embedding(item, "Ollama 批量嵌入响应")?);
        }
        return Ok(vectors);
    }

    let status = response.status();
    let body = read_error_body_snippet(response).await;
    if is_ollama_model_not_found(status, &body) {
        return Err(ollama_model_not_found_error(&config.model, status, &body));
    }
    if should_fallback_ollama_legacy_endpoint(status, &body) {
        log::info!(
            "Ollama 端点不支持批量 /api/embed（HTTP {status}），自动回退旧版 /api/embeddings"
        );
        return embed_texts_ollama_legacy(client, &base_url, &config.model, texts).await;
    }

    Err(AppError::Analysis(format!(
        "Ollama 嵌入返回 HTTP {status}: {body}"
    )))
}

/// 调用嵌入服务，返回与输入等长的归一化向量列表。
async fn embed_texts(
    config: &EmbeddingConfig,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, AppError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    super::ai::validate_model_endpoint(&config.endpoint)?;
    let client = embedding_client(&config.endpoint)?;

    if config.provider == "openai" {
        let url = resolve_openai_embeddings_url(&config.endpoint);
        let mut request = client
            .post(&url)
            .json(&serde_json::json!({ "model": config.model, "input": texts }));
        if let Some(key) = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
        {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let response = request
            .send()
            .await
            .map_err(|e| AppError::Analysis(format!("嵌入请求失败: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = read_error_body_snippet(response).await;
            return Err(AppError::Analysis(format!(
                "嵌入服务返回 HTTP {status}: {body}"
            )));
        }
        let payload = read_json_bounded(response, "嵌入响应解析失败").await?;
        parse_openai_embeddings_response(&payload, texts.len())
    } else {
        embed_texts_ollama(&client, config, texts).await
    }
}

/// 记忆块草稿（分块结果，纯数据）。
#[derive(Debug, Clone)]
pub(crate) struct ChunkDraft {
    pub chunk_key: String,
    pub date: String,
    pub app_name: String,
    pub title: String,
    pub browser_url: Option<String>,
    pub content: String,
    pub last_activity_id: i64,
}

fn local_date_of_timestamp(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn domain_of(url: &str) -> String {
    let trimmed = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    trimmed
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// 把活动记录聚合成记忆块草稿。
pub(crate) fn build_chunk_drafts(activities: &[Activity]) -> Vec<ChunkDraft> {
    struct Group {
        date: String,
        app_name: String,
        title: String,
        browser_url: Option<String>,
        best_ocr: String,
        titles: Vec<String>,
        last_activity_id: i64,
    }

    let mut groups: HashMap<String, Group> = HashMap::new();
    for activity in activities {
        let Some(id) = activity.id else { continue };
        let title = activity.window_title.trim();
        if title.is_empty() || title == "[内容已脱敏]" {
            continue;
        }
        let ocr = activity.ocr_text.as_deref().unwrap_or("").trim();
        if ocr.is_empty() && activity.browser_url.is_none() && title.chars().count() < 4 {
            continue;
        }

        let date = local_date_of_timestamp(activity.timestamp);
        let group_hint = activity
            .browser_url
            .as_deref()
            .map(domain_of)
            .filter(|domain| !domain.is_empty())
            .unwrap_or_else(|| title.chars().take(80).collect());
        let chunk_key = format!("{date}|{}|{group_hint}", activity.app_name);

        let entry = groups.entry(chunk_key).or_insert_with(|| Group {
            date,
            app_name: activity.app_name.clone(),
            title: title.to_string(),
            browser_url: activity.browser_url.clone(),
            best_ocr: String::new(),
            titles: Vec::new(),
            last_activity_id: id,
        });
        entry.last_activity_id = entry.last_activity_id.max(id);
        if ocr.chars().count() > entry.best_ocr.chars().count() {
            entry.best_ocr = ocr.to_string();
        }
        if entry.browser_url.is_none() {
            entry.browser_url = activity.browser_url.clone();
        }
        let title_owned = title.to_string();
        if !entry.titles.contains(&title_owned) && entry.titles.len() < 5 {
            entry.titles.push(title_owned);
        }
    }

    let mut drafts: Vec<ChunkDraft> = groups
        .into_iter()
        .map(|(chunk_key, group)| {
            let mut content = format!("{} | {}", group.app_name, group.titles.join(" / "));
            if let Some(url) = &group.browser_url {
                content.push_str(&format!("\n{url}"));
            }
            if !group.best_ocr.is_empty() {
                content.push('\n');
                content.push_str(&group.best_ocr);
            }
            let content: String = content.chars().take(CHUNK_CONTENT_MAX_CHARS).collect();
            ChunkDraft {
                chunk_key,
                date: group.date,
                app_name: group.app_name,
                title: group.title,
                browser_url: group.browser_url,
                content,
                last_activity_id: group.last_activity_id,
            }
        })
        .collect();
    drafts.sort_by_key(|draft| draft.last_activity_id);
    drafts
}

fn fail_build_if_active(
    database: &Database,
    token: &SemanticBuildToken,
    error: &AppError,
    incremental: bool,
) {
    let res = if incremental {
        database.fail_semantic_incremental_update(
            &token.build_id,
            &token.embedding_fingerprint,
            &token.privacy_fingerprint,
            &error.to_string(),
        )
    } else {
        database.fail_semantic_memory_build(
            &token.build_id,
            &token.embedding_fingerprint,
            &token.privacy_fingerprint,
            &error.to_string(),
        )
    };
    match res {
        Ok(true) | Ok(false) => {}
        Err(update_error) => log::error!("记录语义索引失败状态时出错: {update_error}"),
    }
}

async fn start_rebuild(
    database: &Database,
    embedding: &EmbeddingConfig,
    current_privacy_fingerprint: &str,
) -> Result<SemanticMemoryIndexState, AppError> {
    let build_id = uuid::Uuid::new_v4().simple().to_string();
    let provisional_embedding_fingerprint = embedding_config_fingerprint_for_runtime(embedding);
    database.begin_semantic_memory_build(
        &build_id,
        &provisional_embedding_fingerprint,
        current_privacy_fingerprint,
    )?;
    let provisional_token = SemanticBuildToken {
        build_id: build_id.clone(),
        embedding_fingerprint: provisional_embedding_fingerprint.clone(),
        privacy_fingerprint: current_privacy_fingerprint.to_string(),
    };

    // 先声明唯一代际再探测模型：失败可精确落到本任务，同指纹的新任务不受影响。
    let probe = match embed_texts(embedding, &["semantic memory index probe".to_string()]).await {
        Ok(probe) => probe,
        Err(error) => {
            fail_build_if_active(database, &provisional_token, &error, false);
            return Err(error);
        }
    };
    let dimension = probe.first().map(Vec::len).unwrap_or(0);
    if dimension == 0 {
        let error = AppError::Analysis("嵌入服务返回了空向量".to_string());
        fail_build_if_active(database, &provisional_token, &error, false);
        return Err(error);
    }
    let final_embedding_fingerprint = embedding_fingerprint(embedding, dimension);

    if !database.activate_semantic_memory_build(
        &build_id,
        &provisional_embedding_fingerprint,
        &final_embedding_fingerprint,
        current_privacy_fingerprint,
    )? {
        return Err(AppError::Config(
            "语义索引重建已被更新的任务替代".to_string(),
        ));
    }
    database.get_semantic_memory_state()
}

fn build_matches_current_config(
    index_state: &SemanticMemoryIndexState,
    embedding: &EmbeddingConfig,
    current_privacy_fingerprint: &str,
) -> bool {
    let expected_prefix = format!("{}:", embedding_config_fingerprint_for_runtime(embedding));
    !index_state.build_id.is_empty()
        && index_state.status == "building"
        && index_state
            .embedding_fingerprint
            .starts_with(&expected_prefix)
        && index_state.privacy_fingerprint == current_privacy_fingerprint
}

/// 判断已有语义索引是否可直接走增量模式（不清空历史块，仅按日重算脏日期 + 追加游标后新活动）。
fn can_resume_incremental_index(
    index_state: &SemanticMemoryIndexState,
    embedding: &EmbeddingConfig,
    current_privacy_fingerprint: &str,
) -> bool {
    let expected_prefix = format!("{}:", embedding_config_fingerprint_for_runtime(embedding));
    !index_state.rebuild_required
        && !index_state.build_id.is_empty()
        && index_state
            .embedding_fingerprint
            .starts_with(&expected_prefix)
        && index_state.privacy_fingerprint == current_privacy_fingerprint
        && matches!(index_state.status.as_str(), "ready" | "building" | "failed")
}

/// 向量检索放行条件：只要指纹一致、未要求全量重建且已有已嵌入块，
/// 即使存在少量尚未追平的新活动或正在后台增量更新，依然允许语义检索（M3-5）。
pub(crate) fn is_vector_query_allowed(
    index_state: &SemanticMemoryIndexState,
    config_prefix: &str,
    current_privacy_fingerprint: &str,
    embedded_chunks: usize,
) -> bool {
    !index_state.rebuild_required
        && !index_state.build_id.is_empty()
        && embedded_chunks > 0
        && matches!(index_state.status.as_str(), "ready" | "building" | "failed")
        && index_state.embedding_fingerprint.starts_with(config_prefix)
        && index_state.privacy_fingerprint == current_privacy_fingerprint
}

fn build_token(index_state: &SemanticMemoryIndexState) -> SemanticBuildToken {
    SemanticBuildToken {
        build_id: index_state.build_id.clone(),
        embedding_fingerprint: index_state.embedding_fingerprint.clone(),
        privacy_fingerprint: index_state.privacy_fingerprint.clone(),
    }
}

fn ensure_current_build_context(
    state: &AppState,
    embedding: &EmbeddingConfig,
    token: &SemanticBuildToken,
) -> Result<(), AppError> {
    let current_embedding_config = embedding_config_fingerprint(&state.config);
    let expected_embedding_config = embedding_config_fingerprint_for_runtime(embedding);
    let current_privacy = privacy_fingerprint(&state.config.privacy);
    if !state.config.memory_semantic_enabled
        || current_embedding_config != expected_embedding_config
        || current_privacy != token.privacy_fingerprint
    {
        return Err(AppError::Config(
            "语义索引配置或隐私规则已变化，请重新建立".to_string(),
        ));
    }
    Ok(())
}

async fn index_semantic_memory_unlocked(
    state_arc: &Arc<Mutex<AppState>>,
    force_rebuild: bool,
    allow_full_rebuild: bool,
) -> Result<SemanticIndexProgress, AppError> {
    let embedding = load_embedding_config(state_arc)?;
    let (database, current_privacy_fingerprint) = {
        let state = state_arc
            .lock()
            .map_err(|e| AppError::Unknown(e.to_string()))?;
        (
            state.database.clone(),
            privacy_fingerprint(&state.config.privacy),
        )
    };

    let mut index_state = database.get_semantic_memory_state()?;
    let is_incremental = if !force_rebuild
        && can_resume_incremental_index(&index_state, &embedding, &current_privacy_fingerprint)
    {
        if index_state.status != "building" {
            let transitioned = database.begin_semantic_incremental_update(
                &index_state.build_id,
                &index_state.embedding_fingerprint,
                &index_state.privacy_fingerprint,
            )?;
            if !transitioned {
                if !allow_full_rebuild {
                    let stats = database.semantic_memory_stats()?;
                    let pending_embeddings =
                        stats.total_chunks.saturating_sub(stats.embedded_chunks);
                    return Ok(SemanticIndexProgress {
                        processed_activities: 0,
                        upserted_chunks: 0,
                        embedded_chunks: 0,
                        pending_embeddings,
                        activities_done: true,
                        stats,
                        state: database.get_semantic_memory_state()?,
                    });
                }
                index_state =
                    start_rebuild(&database, &embedding, &current_privacy_fingerprint).await?;
                false
            } else {
                index_state = database.get_semantic_memory_state()?;
                true
            }
        } else {
            true
        }
    } else if !force_rebuild
        && build_matches_current_config(&index_state, &embedding, &current_privacy_fingerprint)
    {
        // 用户已手动发起的全量重建进行中，继续推进当前匹配代际
        false
    } else if allow_full_rebuild {
        index_state = start_rebuild(&database, &embedding, &current_privacy_fingerprint).await?;
        false
    } else {
        // 后台自动增量索引禁止静默发起全量历史重建，需由用户在设置页显式触发
        let stats = database.semantic_memory_stats()?;
        let pending_embeddings = stats.total_chunks.saturating_sub(stats.embedded_chunks);
        return Ok(SemanticIndexProgress {
            processed_activities: 0,
            upserted_chunks: 0,
            embedded_chunks: 0,
            pending_embeddings,
            activities_done: true,
            stats,
            state: index_state,
        });
    };
    let token = build_token(&index_state);

    let result = async {
        if !database.is_semantic_memory_build_current(
            &token.build_id,
            &token.embedding_fingerprint,
            &token.privacy_fingerprint,
        )? {
            return Err(AppError::Config(
                "语义索引重建已被更新的任务替代".to_string(),
            ));
        }

        // 步骤 1：优先处理按日增量重算队列（局部删改 / 异步 OCR 补齐），每轮最多处理 REINDEX_DATES_BATCH 天。
        let dirty_dates = database.list_memory_reindex_dates(REINDEX_DATES_BATCH)?;
        let mut reindex_upserted = 0usize;
        for (dirty_date, snapshot_updated_at) in dirty_dates {
            let state = state_arc
                .lock()
                .map_err(|e| AppError::Unknown(e.to_string()))?;
            ensure_current_build_context(&state, &embedding, &token)?;
            if !database.delete_memory_chunks_for_date_if_current_build(
                &token.build_id,
                &token.embedding_fingerprint,
                &token.privacy_fingerprint,
                &dirty_date,
            )? {
                return Err(AppError::Config("语义索引在按日重算前已失效".to_string()));
            }
            let day_activities = state
                .database
                .get_activities_for_date_for_semantic_index(&dirty_date)?;
            let allowed: Vec<Activity> = day_activities
                .into_iter()
                .filter(|activity| {
                    state.privacy_filter.check_privacy_full(
                        &activity.app_name,
                        &activity.window_title,
                        activity.browser_url.as_deref(),
                    ) == PrivacyAction::Record
                })
                .collect();
            let drafts = build_chunk_drafts(&allowed);
            for draft in &drafts {
                let written = database.upsert_memory_chunk_if_current_build(
                    &token.build_id,
                    &token.embedding_fingerprint,
                    &token.privacy_fingerprint,
                    &draft.chunk_key,
                    &draft.date,
                    &draft.app_name,
                    &draft.title,
                    draft.browser_url.as_deref(),
                    &draft.content,
                    draft.last_activity_id,
                )?;
                if !written {
                    return Err(AppError::Config(
                        "语义索引在按日重算入库前已失效或来源活动已删除".to_string(),
                    ));
                }
                reindex_upserted += 1;
            }
            if !database.clear_memory_reindex_date_if_current_build(
                &token.build_id,
                &token.embedding_fingerprint,
                &token.privacy_fingerprint,
                &dirty_date,
                snapshot_updated_at,
            )? {
                return Err(AppError::Config(
                    "语义索引在清除重算队列前已失效".to_string(),
                ));
            }
        }

        // 步骤 2：沿 id 游标增量追加新活动（不覆盖已有日期的已嵌入向量）。
        let cursor = database.semantic_memory_stats()?.last_indexed_activity_id;
        let initial_activities = database.get_activities_after_id(cursor, INDEX_ACTIVITY_BATCH)?;
        let max_id = initial_activities
            .iter()
            .filter_map(|activity| activity.id)
            .max()
            .unwrap_or(cursor);

        // 入库前重新读取活动并应用当前隐私过滤；持有 AppState 锁期间，配置保存和
        // 删除活动无法穿过该校验，数据库条件写入再校验 build_id、指纹和来源活动。
        let (processed, append_upserted) = {
            let state = state_arc
                .lock()
                .map_err(|e| AppError::Unknown(e.to_string()))?;
            ensure_current_build_context(&state, &embedding, &token)?;
            if !database.is_semantic_memory_build_current(
                &token.build_id,
                &token.embedding_fingerprint,
                &token.privacy_fingerprint,
            )? {
                return Err(AppError::Config("语义索引在块入库前已失效".to_string()));
            }

            let activities: Vec<Activity> = state
                .database
                .get_activities_after_id(cursor, INDEX_ACTIVITY_BATCH)?
                .into_iter()
                .filter(|activity| activity.id.unwrap_or_default() <= max_id)
                .collect();
            let processed = activities.len();
            let allowed: Vec<Activity> = activities
                .into_iter()
                .filter(|activity| {
                    state.privacy_filter.check_privacy_full(
                        &activity.app_name,
                        &activity.window_title,
                        activity.browser_url.as_deref(),
                    ) == PrivacyAction::Record
                })
                .collect();
            let mut drafts = build_chunk_drafts(&allowed);

            // 即使本批全部被隐私规则过滤，也要推进持久化游标。
            if let Some(last) = drafts.last_mut() {
                last.last_activity_id = last.last_activity_id.max(max_id);
            } else if max_id > cursor {
                drafts.push(ChunkDraft {
                    chunk_key: SEMANTIC_MEMORY_CURSOR_CHUNK_KEY.to_string(),
                    date: "1970-01-01".to_string(),
                    app_name: "__cursor__".to_string(),
                    title: "__cursor__".to_string(),
                    browser_url: None,
                    content: "__cursor__".to_string(),
                    last_activity_id: max_id,
                });
            }

            let mut upserted = 0usize;
            for draft in &drafts {
                let written = database.upsert_memory_chunk_if_current_build(
                    &token.build_id,
                    &token.embedding_fingerprint,
                    &token.privacy_fingerprint,
                    &draft.chunk_key,
                    &draft.date,
                    &draft.app_name,
                    &draft.title,
                    draft.browser_url.as_deref(),
                    &draft.content,
                    draft.last_activity_id,
                )?;
                if !written {
                    return Err(AppError::Config(
                        "语义索引在块入库前已失效或来源活动已删除".to_string(),
                    ));
                }
                if draft.chunk_key != SEMANTIC_MEMORY_CURSOR_CHUNK_KEY {
                    upserted += 1;
                }
            }
            (processed, upserted)
        };
        let upserted = reindex_upserted + append_upserted;

        let pending = database.get_unembedded_memory_chunks_if_current_build(
            &token.build_id,
            &token.embedding_fingerprint,
            &token.privacy_fingerprint,
            EMBED_BATCH,
        )?;
        let mut embedded = 0usize;
        if !pending.is_empty() {
            let texts = {
                let state = state_arc
                    .lock()
                    .map_err(|e| AppError::Unknown(e.to_string()))?;
                ensure_current_build_context(&state, &embedding, &token)?;
                if !database.is_semantic_memory_build_current(
                    &token.build_id,
                    &token.embedding_fingerprint,
                    &token.privacy_fingerprint,
                )? {
                    return Err(AppError::Config(
                        "语义索引在发送嵌入请求前已失效".to_string(),
                    ));
                }
                for chunk in &pending {
                    let activity = database
                        .get_activity_by_id(chunk.last_activity_id)?
                        .ok_or_else(|| {
                            AppError::Config("语义块来源活动已删除，已取消嵌入请求".to_string())
                        })?;
                    let activity_allowed = state.privacy_filter.check_privacy_full(
                        &activity.app_name,
                        &activity.window_title,
                        activity.browser_url.as_deref(),
                    ) == PrivacyAction::Record;
                    let chunk_allowed = state.privacy_filter.check_privacy_full(
                        &chunk.app_name,
                        &chunk.title,
                        chunk.browser_url.as_deref(),
                    ) == PrivacyAction::Record;
                    if !activity_allowed || !chunk_allowed {
                        return Err(AppError::Config(
                            "语义块不再满足当前隐私规则，已取消嵌入请求".to_string(),
                        ));
                    }
                }
                pending
                    .iter()
                    .map(|chunk| chunk.content.clone())
                    .collect::<Vec<_>>()
            };

            // 紧邻外发再校验一次代际；失效任务不得把已删除或新禁用内容发送出去。
            if !database.is_semantic_memory_build_current(
                &token.build_id,
                &token.embedding_fingerprint,
                &token.privacy_fingerprint,
            )? {
                return Err(AppError::Config(
                    "语义索引在发送嵌入请求前已失效".to_string(),
                ));
            }
            let vectors = embed_texts(&embedding, &texts).await?;
            let expected_dimension = token
                .embedding_fingerprint
                .rsplit_once(':')
                .and_then(|(_, dimension)| dimension.parse::<usize>().ok())
                .unwrap_or(0);
            if vectors
                .iter()
                .any(|vector| vector.len() != expected_dimension)
            {
                return Err(AppError::Analysis(
                    "嵌入向量维度在重建过程中发生变化，请重试".to_string(),
                ));
            }
            for (chunk, vector) in pending.iter().zip(vectors.iter()) {
                if database.set_memory_chunk_embedding_if_current_build(
                    &token.build_id,
                    chunk.id,
                    &chunk.content,
                    &encode_embedding(vector),
                    &token.embedding_fingerprint,
                    &token.privacy_fingerprint,
                )? {
                    embedded += 1;
                }
            }
        }

        if !database.is_semantic_memory_build_current(
            &token.build_id,
            &token.embedding_fingerprint,
            &token.privacy_fingerprint,
        )? {
            return Err(AppError::Config(
                "语义索引在建立过程中已失效，请重新建立".to_string(),
            ));
        }
        index_state = database.get_semantic_memory_state()?;
        let stats = database.semantic_memory_stats()?;
        let pending_embeddings = stats.total_chunks.saturating_sub(stats.embedded_chunks);
        let mut activities_done = !database
            .has_activities_after_id(stats.last_indexed_activity_id)?
            && index_state.pending_reindex_dates == 0;
        index_state.indexed_activities = index_state
            .indexed_activities
            .saturating_add(processed)
            .min(index_state.total_activities);
        if activities_done && pending_embeddings == 0 {
            index_state.status = "ready".to_string();
            index_state.rebuild_required = false;
            index_state.indexed_activities = index_state.total_activities;
            index_state.last_error = None;
        }
        let updated = database.update_semantic_memory_state_if_current_build(
            &token.build_id,
            &token.embedding_fingerprint,
            &token.privacy_fingerprint,
            &index_state,
        )?;
        if !updated {
            let current = database.get_semantic_memory_state()?;
            if database.is_semantic_memory_build_current(
                &token.build_id,
                &token.embedding_fingerprint,
                &token.privacy_fingerprint,
            )? {
                // 新活动或脏日期可能在完成检查后写入；保持 building，下一轮继续消费。
                activities_done = false;
                return Ok(SemanticIndexProgress {
                    processed_activities: processed,
                    upserted_chunks: upserted,
                    embedded_chunks: embedded,
                    pending_embeddings,
                    activities_done,
                    stats,
                    state: current,
                });
            }
            return Err(AppError::Config(
                "语义索引在建立过程中已失效，请重新建立".to_string(),
            ));
        }
        let index_state = database.get_semantic_memory_state()?;

        Ok(SemanticIndexProgress {
            processed_activities: processed,
            upserted_chunks: upserted,
            embedded_chunks: embedded,
            pending_embeddings,
            activities_done,
            stats,
            state: index_state,
        })
    }
    .await;

    if let Err(error) = &result {
        fail_build_if_active(&database, &token, error, is_incremental);
    }
    result
}

async fn index_semantic_memory_inner(
    state_arc: &Arc<Mutex<AppState>>,
    force_rebuild: bool,
) -> Result<SemanticIndexProgress, AppError> {
    let _guard = SEMANTIC_INDEX_SINGLE_FLIGHT.lock().await;
    index_semantic_memory_unlocked(state_arc, force_rebuild, true).await
}

/// 判断后台自动增量任务是否允许且确实有增量/续跑工作（绝不静默触发全量重建）。
fn background_auto_index_has_work(
    index_state: &SemanticMemoryIndexState,
    embedding: &EmbeddingConfig,
    current_privacy_fingerprint: &str,
    pending_embeddings: usize,
    has_new_activities: bool,
) -> bool {
    let resumable_incremental =
        can_resume_incremental_index(index_state, embedding, current_privacy_fingerprint);
    let resumable_manual_build =
        build_matches_current_config(index_state, embedding, current_privacy_fingerprint);
    if !resumable_incremental && !resumable_manual_build {
        return false;
    }
    resumable_manual_build
        || index_state.pending_reindex_dates > 0
        || pending_embeddings > 0
        || has_new_activities
}

/// 后台定时增量索引入口（M3-5）：默认关闭，仅当 `memory_semantic_enabled && semantic_auto_index`
/// 且当前未暂停录制时运行；每次最多执行 1 批（32 块上限），且绝不静默发起全量历史重建。
pub(crate) async fn run_background_semantic_auto_index(state_arc: &Arc<Mutex<AppState>>) {
    let Ok(_guard) = SEMANTIC_INDEX_SINGLE_FLIGHT.try_lock() else {
        return;
    };
    let (should_run, database, current_privacy_fingerprint) = match state_arc.lock() {
        Ok(s) => (
            s.config.memory_semantic_enabled && s.config.semantic_auto_index && !s.is_paused,
            s.database.clone(),
            privacy_fingerprint(&s.config.privacy),
        ),
        Err(_) => return,
    };
    if !should_run {
        return;
    }
    let Ok(embedding) = load_embedding_config(state_arc) else {
        return;
    };
    let has_work = (|| -> Result<bool, AppError> {
        let index_state = database.get_semantic_memory_state()?;
        let stats = database.semantic_memory_stats()?;
        let pending_embeddings = stats.total_chunks.saturating_sub(stats.embedded_chunks);
        let has_new = database.has_activities_after_id(stats.last_indexed_activity_id)?;
        Ok(background_auto_index_has_work(
            &index_state,
            &embedding,
            &current_privacy_fingerprint,
            pending_embeddings,
            has_new,
        ))
    })()
    .unwrap_or(false);
    if !has_work {
        return;
    }

    match index_semantic_memory_unlocked(state_arc, false, false).await {
        Ok(progress) => {
            if progress.processed_activities > 0 || progress.embedded_chunks > 0 {
                log::info!(
                    "后台增量语义索引完成一轮: processed={}, upserted={}, embedded={}, pending={}",
                    progress.processed_activities,
                    progress.upserted_chunks,
                    progress.embedded_chunks,
                    progress.pending_embeddings
                );
            }
        }
        Err(err) => {
            log::warn!("后台增量语义索引本轮失败（不影响后续手动或下次自动重试）: {err}");
        }
    }
}

/// 分批推进一次增量更新或全量重建。
/// 默认 `force_rebuild = false`（仅在无有效索引、指纹变化或 `rebuild_required` 时才全量清空重建）；
/// 传入 `force_rebuild = true` 时强制清空旧块从游标 0 重建。
#[tauri::command]
pub async fn index_semantic_memory(
    force_rebuild: Option<bool>,
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<SemanticIndexProgress, AppError> {
    let state_arc = state.inner().clone();
    index_semantic_memory_inner(&state_arc, force_rebuild.unwrap_or(false)).await
}

/// 测试嵌入模型连通性：发一条探针文本，返回向量维度与耗时。
#[tauri::command]
pub async fn test_embedding_model(
    provider: String,
    endpoint: String,
    model: String,
    api_key: Option<String>,
) -> Result<serde_json::Value, AppError> {
    let endpoint = normalize_embedding_endpoint(&endpoint);
    if endpoint.is_empty() || model.trim().is_empty() {
        return Err(AppError::Config("请先填写嵌入服务地址与模型名".to_string()));
    }
    let embedding = EmbeddingConfig {
        provider: provider.trim().to_ascii_lowercase(),
        endpoint,
        model: model.trim().to_string(),
        api_key,
    };

    let started = std::time::Instant::now();
    let vectors = embed_texts(&embedding, &["connection test".to_string()]).await?;
    let dimension = vectors.first().map(Vec::len).unwrap_or(0);
    if dimension == 0 {
        return Err(AppError::Analysis("嵌入服务返回了空向量".to_string()));
    }
    Ok(serde_json::json!({
        "dimension": dimension,
        "latencyMs": started.elapsed().as_millis() as u64,
    }))
}

/// 返回真实持久化索引状态，供设置页显示重建需求、进度和最后错误。
#[tauri::command]
pub async fn semantic_memory_status(
    state: State<'_, Arc<Mutex<AppState>>>,
) -> Result<SemanticMemoryIndexState, AppError> {
    let state = state.lock().map_err(|e| AppError::Unknown(e.to_string()))?;
    state.database.get_semantic_memory_state()
}

fn fts_activity_hits(items: Vec<MemorySearchItem>) -> Vec<SemanticMemoryHit> {
    items
        .into_iter()
        // 摘要和日报没有可追溯的应用/域名来源，无法按当前规则证明安全。
        .filter(|item| item.source_type == "activity")
        .map(|item| SemanticMemoryHit {
            chunk_id: item.source_id.unwrap_or_default(),
            date: item.date,
            app_name: item.app_name.unwrap_or_default(),
            title: item.title,
            browser_url: item.browser_url,
            excerpt: item.excerpt,
            score: 0.0,
        })
        .collect()
}

fn filter_hits_by_current_privacy(
    state: &AppState,
    hits: Vec<SemanticMemoryHit>,
) -> Vec<SemanticMemoryHit> {
    hits.into_iter()
        .filter(|hit| hit.app_name != "__cursor__")
        .filter(|hit| {
            state.privacy_filter.check_privacy_full(
                &hit.app_name,
                &hit.title,
                hit.browser_url.as_deref(),
            ) == PrivacyAction::Record
        })
        .collect()
}

/// 内部实现：也供助手 semantic_search 工具桥接调用。
pub(crate) async fn search_semantic_memory_inner(
    state_arc: &Arc<Mutex<AppState>>,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticMemoryHit>, AppError> {
    let embedding = load_embedding_config(state_arc)?;
    let (database, current_privacy_fingerprint) = {
        let state = state_arc
            .lock()
            .map_err(|e| AppError::Unknown(e.to_string()))?;
        (
            state.database.clone(),
            privacy_fingerprint(&state.config.privacy),
        )
    };
    let index_state = database.get_semantic_memory_state()?;
    let stats = database.semantic_memory_stats()?;
    let config_prefix = format!("{}:", embedding_config_fingerprint_for_runtime(&embedding));

    // 指纹一致、未要求全量重建且已有向量块时允许语义检索（即使存在少量未追平的新活动或正在增量更新）；
    // 配置/隐私指纹变化或待重建时严格关闭向量查询并降级为 FTS。
    let vector_query_allowed = is_vector_query_allowed(
        &index_state,
        &config_prefix,
        &current_privacy_fingerprint,
        stats.embedded_chunks,
    );

    let semantic_hits = if vector_query_allowed {
        match embed_texts(&embedding, &[query.to_string()]).await {
            Ok(vectors) => match vectors.into_iter().next() {
                Some(query_vector) => {
                    let current_embedding_fingerprint =
                        embedding_fingerprint(&embedding, query_vector.len());
                    if index_state.embedding_fingerprint != current_embedding_fingerprint {
                        log::warn!("嵌入维度或配置指纹变化，语义检索降级并标记待重建");
                        invalidate_semantic_memory_index(&database)?;
                        Vec::new()
                    } else {
                        let hits =
                            database.search_memory_chunks_semantic(&query_vector, limit.max(20))?;
                        let state = state_arc
                            .lock()
                            .map_err(|e| AppError::Unknown(e.to_string()))?;
                        filter_hits_by_current_privacy(&state, hits)
                    }
                }
                None => Vec::new(),
            },
            Err(error) => {
                log::warn!("嵌入服务不可用，语义检索降级为关键词检索: {error}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let fts_hits = fts_activity_hits(
        database
            .search_memory(query, None, None, limit.max(20))
            .unwrap_or_default(),
    );
    let fts_hits = {
        let state = state_arc
            .lock()
            .map_err(|e| AppError::Unknown(e.to_string()))?;
        filter_hits_by_current_privacy(&state, fts_hits)
    };

    // RRF 融合向量召回与 FTS 召回。
    const RRF_K: f64 = 60.0;
    let mut fused: HashMap<String, SemanticMemoryHit> = HashMap::new();
    for (rank, hit) in semantic_hits.into_iter().enumerate() {
        let key = format!("{}|{}|{}", hit.date, hit.app_name, hit.title);
        let entry = fused
            .entry(key)
            .or_insert_with(|| SemanticMemoryHit { score: 0.0, ..hit });
        entry.score += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    for (rank, hit) in fts_hits.into_iter().enumerate() {
        let key = format!("{}|{}|{}", hit.date, hit.app_name, hit.title);
        let entry = fused.entry(key).or_insert(hit);
        entry.score += 1.0 / (RRF_K + rank as f64 + 1.0);
    }

    let mut results: Vec<SemanticMemoryHit> = fused.into_values().collect();
    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use work_review_core::config::{AppPrivacyRule, PrivacyLevel};

    fn activity(
        id: i64,
        timestamp: i64,
        app: &str,
        title: &str,
        url: Option<&str>,
        ocr: Option<&str>,
    ) -> Activity {
        Activity {
            id: Some(id),
            timestamp,
            app_name: app.to_string(),
            window_title: title.to_string(),
            screenshot_path: String::new(),
            ocr_text: ocr.map(ToString::to_string),
            category: "other".to_string(),
            duration: 30,
            browser_url: url.map(ToString::to_string),
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        }
    }

    #[test]
    fn 分块应按日期应用域名聚合并保留最长ocr() {
        let timestamp = 1_753_500_000;
        let activities = vec![
            activity(
                1,
                timestamp,
                "Chrome",
                "Rust async 详解 - 博客",
                Some("https://blog.example.com/rust-async"),
                Some("短 OCR"),
            ),
            activity(
                2,
                timestamp + 60,
                "Chrome",
                "Rust async 详解 - 博客",
                Some("https://blog.example.com/rust-async?p=2"),
                Some("这是一段更长的 OCR 文本内容示例"),
            ),
            activity(
                3,
                timestamp + 120,
                "Xcode",
                "main.rs — work-review",
                None,
                Some("fn main() {}"),
            ),
        ];
        let drafts = build_chunk_drafts(&activities);
        assert_eq!(drafts.len(), 2, "同域名网页应合并，Xcode 单独一块");
        let chrome = drafts
            .iter()
            .find(|draft| draft.app_name == "Chrome")
            .unwrap();
        assert!(chrome.content.contains("更长的 OCR"), "应保留组内最长 OCR");
        assert!(chrome.content.contains("blog.example.com"));
        assert_eq!(chrome.last_activity_id, 2);
    }

    #[test]
    fn 分块应跳过脱敏与无价值记录() {
        let timestamp = 1_753_500_000;
        let activities = vec![
            activity(1, timestamp, "App", "[内容已脱敏]", None, Some("不应出现")),
            activity(2, timestamp, "App", "ab", None, None),
        ];
        assert!(build_chunk_drafts(&activities).is_empty());
    }

    #[test]
    fn 域名提取应容忍协议与路径() {
        assert_eq!(domain_of("https://a.b.com/x?y=1"), "a.b.com");
        assert_eq!(domain_of("http://a.b.com"), "a.b.com");
        assert_eq!(domain_of("a.b.com/path"), "a.b.com");
    }

    #[test]
    fn embedding配置指纹应忽略api_key与末尾斜杠() {
        let first = AppConfig {
            embedding_provider: "OPENAI".to_string(),
            embedding_endpoint: "https://example.com/v1/".to_string(),
            embedding_model: "bge-m3".to_string(),
            embedding_api_key: Some("secret-a".to_string()),
            ..AppConfig::default()
        };
        let mut second = first.clone();
        second.embedding_endpoint = "https://example.com/v1".to_string();
        second.embedding_api_key = Some("secret-b".to_string());

        assert_eq!(
            embedding_config_fingerprint(&first),
            embedding_config_fingerprint(&second)
        );
        second.embedding_model = "other-model".to_string();
        assert_ne!(
            embedding_config_fingerprint(&first),
            embedding_config_fingerprint(&second)
        );
    }

    #[test]
    fn 重叠应用规则应按最严级别生效且规则变更触发指纹变化() {
        let first = PrivacyConfig {
            app_rules: vec![
                AppPrivacyRule {
                    app_name: "Acme".to_string(),
                    level: PrivacyLevel::Full,
                },
                AppPrivacyRule {
                    app_name: "Acme Vault".to_string(),
                    level: PrivacyLevel::Ignored,
                },
            ],
            ..PrivacyConfig::default()
        };
        let mut second = first.clone();
        second.app_rules.reverse();

        // 重叠规则统一取最严级别（Ignored > Anonymized > Full），防止宽松规则或别名遮蔽严格规则
        assert_eq!(
            first.get_app_privacy_level("Acme Vault"),
            PrivacyLevel::Ignored
        );
        assert_eq!(
            second.get_app_privacy_level("Acme Vault"),
            PrivacyLevel::Ignored
        );

        let mut relaxed = first.clone();
        relaxed.app_rules[1].level = PrivacyLevel::Anonymized;
        assert_ne!(privacy_fingerprint(&first), privacy_fingerprint(&relaxed));
    }

    #[test]
    fn 隐私指纹应忽略无序集合顺序但识别语义变化() {
        let mut first = PrivacyConfig {
            excluded_keywords: vec!["登录".to_string(), "Password".to_string()],
            excluded_domains: vec![
                "https://Secret.Example.com/path".to_string(),
                "private.example.org".to_string(),
            ],
            excluded_apps: vec!["Legacy".to_string(), "Vault".to_string()],
            ..PrivacyConfig::default()
        };
        let mut second = first.clone();
        second.excluded_keywords.reverse();
        second.excluded_domains.reverse();
        second.excluded_apps.reverse();

        assert_eq!(privacy_fingerprint(&first), privacy_fingerprint(&second));
        first.excluded_keywords.push("token".to_string());
        assert_ne!(privacy_fingerprint(&first), privacy_fingerprint(&second));
    }

    #[test]
    fn 隐私指纹结构化编码不得发生分隔符碰撞() {
        let first = PrivacyConfig {
            excluded_keywords: vec!["a|b".to_string(), "c".to_string()],
            ..PrivacyConfig::default()
        };
        let second = PrivacyConfig {
            excluded_keywords: vec!["a".to_string(), "b|c".to_string()],
            ..PrivacyConfig::default()
        };

        assert_ne!(privacy_fingerprint(&first), privacy_fingerprint(&second));
    }

    #[test]
    fn fts降级只允许可按当前隐私规则复核的活动来源() {
        let items = vec![
            work_review_core::database::MemorySearchItem {
                source_type: "activity".to_string(),
                source_id: Some(1),
                date: "2026-08-05".to_string(),
                timestamp: 1,
                title: "活动".to_string(),
                excerpt: "允许返回".to_string(),
                app_name: Some("Code".to_string()),
                browser_url: None,
                duration: Some(60),
                score: 200,
            },
            work_review_core::database::MemorySearchItem {
                source_type: "hourly_summary".to_string(),
                source_id: Some(2),
                date: "2026-08-05".to_string(),
                timestamp: 2,
                title: "小时摘要".to_string(),
                excerpt: "旧隐私内容".to_string(),
                app_name: None,
                browser_url: None,
                duration: Some(60),
                score: 180,
            },
            work_review_core::database::MemorySearchItem {
                source_type: "daily_report".to_string(),
                source_id: None,
                date: "2026-08-05".to_string(),
                timestamp: 3,
                title: "日报".to_string(),
                excerpt: "旧隐私内容".to_string(),
                app_name: None,
                browser_url: None,
                duration: None,
                score: 160,
            },
        ];

        let hits = fts_activity_hits(items);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].excerpt, "允许返回");
    }

    #[test]
    fn 增量滞后状态下应继续允许向量检索且指纹不匹配时严格拒绝() {
        let cfg = EmbeddingConfig {
            provider: "ollama".to_string(),
            endpoint: "http://localhost:11434".to_string(),
            model: "nomic-embed-text".to_string(),
            api_key: None,
        };
        let prefix = format!("{}:", embedding_config_fingerprint_for_runtime(&cfg));
        let state = SemanticMemoryIndexState {
            build_id: "build-1".to_string(),
            embedding_fingerprint: format!("{prefix}768"),
            privacy_fingerprint: "priv-1".to_string(),
            rebuild_required: false,
            status: "ready".to_string(),
            indexed_activities: 100,
            total_activities: 108, // 新增 8 条未索引增量
            pending_reindex_dates: 1,
            last_error: None,
            updated_at: 0,
        };
        assert!(can_resume_incremental_index(&state, &cfg, "priv-1"));
        assert!(is_vector_query_allowed(&state, &prefix, "priv-1", 42));
        // 尚无已嵌入向量块时不触发无意义嵌入请求
        assert!(!is_vector_query_allowed(&state, &prefix, "priv-1", 0));
        // 隐私指纹或嵌入模型变化时必须拒绝向量查询并触发全量重建
        assert!(!can_resume_incremental_index(&state, &cfg, "priv-2"));
        assert!(!is_vector_query_allowed(&state, &prefix, "priv-2", 42));

        let mut rebuild_needed = state.clone();
        rebuild_needed.rebuild_required = true;
        assert!(!can_resume_incremental_index(
            &rebuild_needed,
            &cfg,
            "priv-1"
        ));
        assert!(!is_vector_query_allowed(
            &rebuild_needed,
            &prefix,
            "priv-1",
            42
        ));
        // 后台自动增量索引绝不得在 rebuild_required / 未建索引 / 指纹失配时静默发起全量重建
        assert!(!background_auto_index_has_work(
            &rebuild_needed,
            &cfg,
            "priv-1",
            0,
            true
        ));
        let unbuilt = SemanticMemoryIndexState {
            build_id: String::new(),
            embedding_fingerprint: String::new(),
            privacy_fingerprint: String::new(),
            rebuild_required: true,
            status: "idle".to_string(),
            indexed_activities: 0,
            total_activities: 50,
            pending_reindex_dates: 0,
            last_error: None,
            updated_at: 0,
        };
        assert!(!background_auto_index_has_work(
            &unbuilt, &cfg, "priv-1", 0, true
        ));
        // 仅在已有匹配代际且有增量或手动重建进行中时才允许后台推进
        assert!(background_auto_index_has_work(
            &state, &cfg, "priv-1", 0, true
        ));
        assert!(!background_auto_index_has_work(
            &state, &cfg, "priv-2", 5, true
        ));
        let mut clean_ready = state.clone();
        clean_ready.pending_reindex_dates = 0;
        assert!(!background_auto_index_has_work(
            &clean_ready,
            &cfg,
            "priv-1",
            0,
            false
        ));
        let mut manual_rebuilding = rebuild_needed.clone();
        manual_rebuilding.status = "building".to_string();
        assert!(background_auto_index_has_work(
            &manual_rebuilding,
            &cfg,
            "priv-1",
            0,
            false
        ));
    }

    #[tokio::test]
    async fn 后台自动索引在关闭暂停或待全量重建时不发起嵌入请求() {
        let dir = std::env::temp_dir().join(format!(
            "work-review-auto-index-guard-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = work_review_core::database::Database::new(&dir.join("test.db")).unwrap();
        // 配置指向不可达端口，若错误发起嵌入或全量重建将改写 semantic_memory_state
        let config = work_review_core::config::AppConfig {
            embedding_provider: "ollama".to_string(),
            embedding_endpoint: "http://127.0.0.1:9".to_string(),
            embedding_model: "nomic-embed-text".to_string(),
            ..Default::default()
        };
        let state = Arc::new(Mutex::new(crate::AppState {
            config: config.clone(),
            config_load_status: work_review_core::config::ConfigLoadStatus::Loaded,
            database: db.clone(),
            privacy_filter: work_review_core::privacy::PrivacyFilter::from_config(&config.privacy),
            screenshot_service: crate::screenshot::ScreenshotService::new(&dir, &config.storage),
            storage_manager: crate::storage::StorageManager::new(&dir, config.storage.clone()),
            data_dir: dir.clone(),
            config_path: dir.join("config.json"),
            is_recording: false,
            is_paused: false,
            avatar_state: crate::avatar_engine::default_avatar_state(),
            avatar_generating_report: false,
            generating_report: false,
            localhost_api_runtime: crate::localhost_api::LocalhostApiRuntime::default(),
            telegram_bot_runtime: crate::telegram_bot::TelegramBotRuntime::default(),
            wecom_bot_runtime: crate::wecom_aibot::WecomBotRuntime::default(),
            cached_active_window: None,
        }));

        // 1. 默认 semantic_auto_index = false -> 直接跳过
        run_background_semantic_auto_index(&state).await;
        assert_eq!(db.get_semantic_memory_state().unwrap().status, "idle");

        // 2. 开启 semantic_auto_index 但 memory_semantic_enabled = false -> 直接跳过
        state.lock().unwrap().config.semantic_auto_index = true;
        run_background_semantic_auto_index(&state).await;
        assert_eq!(db.get_semantic_memory_state().unwrap().status, "idle");

        // 3. 均开启但 is_paused = true -> 直接跳过
        {
            let mut g = state.lock().unwrap();
            g.config.memory_semantic_enabled = true;
            g.is_paused = true;
        }
        run_background_semantic_auto_index(&state).await;
        assert_eq!(db.get_semantic_memory_state().unwrap().status, "idle");

        // 4. 解除暂停但索引从未建立（rebuild_required = true）-> 禁止静默全量重建
        state.lock().unwrap().is_paused = false;
        run_background_semantic_auto_index(&state).await;
        let final_state = db.get_semantic_memory_state().unwrap();
        assert_eq!(final_state.status, "idle");
        assert!(final_state.build_id.is_empty());
        assert!(final_state.last_error.is_none());

        drop(state);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn openai兼容嵌入地址解析应支持本地服务与多版本路径() {
        assert_eq!(
            resolve_openai_embeddings_url("http://127.0.0.1:1234"),
            "http://127.0.0.1:1234/v1/embeddings"
        );
        assert_eq!(
            resolve_openai_embeddings_url("http://127.0.0.1:1234/v1/"),
            "http://127.0.0.1:1234/v1/embeddings"
        );
        assert_eq!(
            resolve_openai_embeddings_url("http://127.0.0.1:8080/v1/embeddings/"),
            "http://127.0.0.1:8080/v1/embeddings"
        );
        assert_eq!(
            resolve_openai_embeddings_url("https://open.bigmodel.cn/api/paas/v4"),
            "https://open.bigmodel.cn/api/paas/v4/embeddings"
        );
        assert_eq!(
            resolve_openai_embeddings_url("https://ark.cn-beijing.volces.com/api/v3/"),
            "https://ark.cn-beijing.volces.com/api/v3/embeddings"
        );
        assert_eq!(
            resolve_openai_embeddings_url(
                "https://generativelanguage.googleapis.com/v1beta/openai"
            ),
            "https://generativelanguage.googleapis.com/v1beta/openai/embeddings"
        );
        assert_eq!(
            resolve_openai_embeddings_url(
                "https://my.openai.azure.com/openai/deployments/emb/embeddings/?api-version=2024-02-01"
            ),
            "https://my.openai.azure.com/openai/deployments/emb/embeddings?api-version=2024-02-01"
        );
    }

    #[test]
    fn ollama基础地址与旧版回退判定应区分路由缺失与模型未安装() {
        assert_eq!(
            resolve_ollama_base_url("http://localhost:11434/"),
            "http://localhost:11434"
        );
        assert_eq!(
            resolve_ollama_base_url("http://localhost:11434/api/embed"),
            "http://localhost:11434"
        );
        assert_eq!(
            resolve_ollama_base_url("http://localhost:11434/api/embeddings"),
            "http://localhost:11434"
        );

        assert!(is_loopback_embedding_endpoint("http://localhost:11434"));
        assert!(is_loopback_embedding_endpoint("http://localhost.:11434"));
        assert!(is_loopback_embedding_endpoint("http://127.0.0.1:1234/v1"));
        assert!(is_loopback_embedding_endpoint("http://[::1]:8080/v1"));
        assert!(!is_loopback_embedding_endpoint(
            "https://api.siliconflow.cn"
        ));
        assert!(!is_loopback_embedding_endpoint(
            "http://127.0.0.1.evil.com/v1"
        ));

        assert!(should_fallback_ollama_legacy_endpoint(
            reqwest::StatusCode::NOT_FOUND,
            "404 page not found"
        ));
        // 反向代理返回的 HTML 404 即使提到 model 和 not found，也应回退旧接口而非误判模型缺失
        assert!(should_fallback_ollama_legacy_endpoint(
            reqwest::StatusCode::NOT_FOUND,
            "<html><body>Requested model page was not found</body></html>"
        ));
        assert!(should_fallback_ollama_legacy_endpoint(
            reqwest::StatusCode::METHOD_NOT_ALLOWED,
            "Method Not Allowed"
        ));
        assert!(is_ollama_model_not_found(
            reqwest::StatusCode::NOT_FOUND,
            r#"{"error":"model \"bge-m3\" not found, try pulling it first"}"#
        ));
        assert!(!should_fallback_ollama_legacy_endpoint(
            reqwest::StatusCode::NOT_FOUND,
            r#"{"error":"model \"bge-m3\" not found, try pulling it first"}"#
        ));
        assert!(!should_fallback_ollama_legacy_endpoint(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "internal error"
        ));
    }

    #[test]
    fn openai响应应按index重排并拒绝空向量零向量或非法index() {
        let payload = serde_json::json!({
            "data": [
                { "index": 1, "embedding": [0.0, 4.0] },
                { "index": 0, "embedding": [3.0, 0.0] }
            ]
        });
        let vectors = parse_openai_embeddings_response(&payload, 2).unwrap();
        assert_eq!(vectors.len(), 2);
        assert!((vectors[0][0] - 1.0).abs() < 1e-6);
        assert!((vectors[1][1] - 1.0).abs() < 1e-6);

        let dup_idx = serde_json::json!({
            "data": [
                { "index": 0, "embedding": [1.0] },
                { "index": 0, "embedding": [1.0] }
            ]
        });
        assert!(parse_openai_embeddings_response(&dup_idx, 2).is_err());

        let empty_vec = serde_json::json!({
            "data": [{ "index": 0, "embedding": [] }]
        });
        assert!(parse_openai_embeddings_response(&empty_vec, 1).is_err());

        let zero_vec = serde_json::json!({
            "data": [{ "index": 0, "embedding": [0.0, 0.0] }]
        });
        assert!(parse_openai_embeddings_response(&zero_vec, 1).is_err());

        // f32 平方溢出导致归一化退化为零向量时也必须拒绝
        let overflow_vec = serde_json::json!({
            "data": [{ "index": 0, "embedding": [1.0e25, 1.0e25] }]
        });
        assert!(parse_openai_embeddings_response(&overflow_vec, 1).is_err());
    }

    async fn spawn_mock_http_server(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (
        String,
        tokio::task::JoinHandle<Vec<(String, String, String)>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut recorded = Vec::new();
            for (status_line, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let mut header_end = None;
                let mut content_length = 0usize;

                loop {
                    let n = stream.read(&mut tmp).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let headers_text = String::from_utf8_lossy(&buf[..pos]);
                            for line in headers_text.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.eq_ignore_ascii_case("content-length") {
                                        content_length = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(end) = header_end {
                        if buf.len() >= end + content_length {
                            break;
                        }
                    }
                }

                let end = header_end.unwrap_or(buf.len());
                let headers_text = String::from_utf8_lossy(&buf[..end]).to_string();
                let first_line = headers_text.lines().next().unwrap_or("").to_string();
                let path = first_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                let req_body = String::from_utf8_lossy(&buf[end..]).to_string();
                recorded.push((path, headers_text, req_body));

                let reply = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
                let _ = stream.shutdown().await;
            }
            recorded
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn ollama优先使用批量embed接口单次完成多条向量化() {
        let (endpoint, handle) =
            spawn_mock_http_server(vec![("200 OK", r#"{"embeddings":[[3.0,4.0],[0.0,2.0]]}"#)])
                .await;
        let cfg = EmbeddingConfig {
            provider: "ollama".to_string(),
            endpoint,
            model: "shaw/dmeta-embedding-zh".to_string(),
            api_key: None,
        };
        let vectors = embed_texts(&cfg, &["第一条".to_string(), "第二条".to_string()])
            .await
            .unwrap();
        let recorded = handle.await.unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "/api/embed");
        assert!(recorded[0].2.contains("\"input\":[\"第一条\",\"第二条\"]"));
        assert_eq!(vectors.len(), 2);
        assert!((vectors[0][0] - 0.6).abs() < 1e-5);
        assert!((vectors[0][1] - 0.8).abs() < 1e-5);
        assert!((vectors[1][1] - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn ollama旧版404时自动回退到embeddings逐条接口() {
        let (endpoint, handle) = spawn_mock_http_server(vec![
            ("404 Not Found", "404 page not found"),
            ("200 OK", r#"{"embedding":[1.0,0.0]}"#),
            ("200 OK", r#"{"embedding":[0.0,5.0]}"#),
        ])
        .await;
        let cfg = EmbeddingConfig {
            provider: "ollama".to_string(),
            endpoint,
            model: "nomic-embed-text".to_string(),
            api_key: None,
        };
        let vectors = embed_texts(&cfg, &["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        let recorded = handle.await.unwrap();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].0, "/api/embed");
        assert_eq!(recorded[1].0, "/api/embeddings");
        assert_eq!(recorded[2].0, "/api/embeddings");
        assert_eq!(vectors.len(), 2);
        assert!((vectors[0][0] - 1.0).abs() < 1e-5);
        assert!((vectors[1][1] - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn ollama模型未安装404应直接提示拉取命令而不回退旧接口() {
        let (endpoint, handle) = spawn_mock_http_server(vec![(
            "404 Not Found",
            r#"{"error":"model \"bge-m3\" not found, try pulling it first"}"#,
        )])
        .await;
        let cfg = EmbeddingConfig {
            provider: "ollama".to_string(),
            endpoint,
            model: "bge-m3".to_string(),
            api_key: None,
        };
        let err = embed_texts(&cfg, &["hello".to_string()]).await.unwrap_err();
        let recorded = handle.await.unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "/api/embed");
        assert!(
            err.to_string().contains("ollama pull bge-m3"),
            "应给出拉取命令提示，实际: {err}"
        );
    }

    #[tokio::test]
    async fn openai兼容本地小模型无需api_key且支持乱序index() {
        let (endpoint, handle) = spawn_mock_http_server(vec![(
            "200 OK",
            r#"{"data":[{"index":1,"embedding":[0.0,2.0]},{"index":0,"embedding":[4.0,0.0]}]}"#,
        )])
        .await;
        let cfg = EmbeddingConfig {
            provider: "openai".to_string(),
            endpoint: format!("{endpoint}/v1"),
            model: "text-embedding-bge-small-zh-v1.5".to_string(),
            api_key: Some("   ".to_string()),
        };
        let vectors = embed_texts(&cfg, &["first".to_string(), "second".to_string()])
            .await
            .unwrap();
        let recorded = handle.await.unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "/v1/embeddings");
        assert!(
            !recorded[0]
                .1
                .to_ascii_lowercase()
                .contains("authorization:"),
            "空白 API Key 不应发送 Authorization 头"
        );
        assert!((vectors[0][0] - 1.0).abs() < 1e-5);
        assert!((vectors[1][1] - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn 远程明文http嵌入端点应被安全校验拒绝() {
        let cfg = EmbeddingConfig {
            provider: "openai".to_string(),
            endpoint: "http://api.example.com/v1".to_string(),
            model: "bge-m3".to_string(),
            api_key: Some("sk-test".to_string()),
        };
        let err = embed_texts(&cfg, &["hello".to_string()]).await.unwrap_err();
        assert!(err.to_string().contains("https"));
    }

    #[tokio::test]
    async fn 嵌入请求不得跟随重定向避免降级与ssrf() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut tmp = [0u8; 2048];
            let _ = stream.read(&mut tmp).await;
            let reply = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 8\r\nConnection: close\r\n\r\nredirect";
            stream.write_all(reply.as_bytes()).await.unwrap();
            let _ = stream.shutdown().await;
            1usize
        });

        let cfg = EmbeddingConfig {
            provider: "openai".to_string(),
            endpoint: format!("http://{addr}/v1"),
            model: "bge-m3".to_string(),
            api_key: Some("sk-secret".to_string()),
        };
        let err = embed_texts(&cfg, &["hello".to_string()]).await.unwrap_err();
        let hits = handle.await.unwrap();
        assert_eq!(hits, 1);
        assert!(
            err.to_string().contains("302"),
            "重定向应直接失败而非跟随，实际: {err}"
        );
    }
}
