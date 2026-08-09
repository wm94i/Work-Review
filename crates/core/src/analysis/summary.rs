use crate::analysis::report_blocks::{
    apply_preferences, assemble_with_section_count, default_summary_order, format_section_heading,
    merge_ai_order, wrap_block, BLOCK_AI_ANALYSIS,
};
use crate::analysis::{
    append_custom_prompt_for_locale, format_duration_for_locale, generate_activity_timeline,
    generate_hourly_activity_summary_for_locale, translate_semantic_category_name, Analyzer,
    AppLocale, GeneratedReport,
};
use crate::config::AiProvider;
use crate::database::{Activity, DailyStats};
use crate::error::{AppError, Result};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// 日报命令的外层截止时间由配置决定；请求预算在其基础上预留 60 秒用于回退模板和收尾
/// （见 AppConfig::report_ai_request_timeout_secs），此处只负责把秒数转成 Duration。
fn summary_request_timeout(timeout_secs: u64) -> Duration {
    Duration::from_secs(timeout_secs)
}

fn format_domain_label(
    domain: &crate::database::DomainUsage,
    locale: AppLocale,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    match domain.semantic_category.as_deref().map(str::trim) {
        Some(semantic_category) if !semantic_category.is_empty() => {
            let semantic_category =
                translate_semantic_category_name(semantic_category, locale, semantic_overrides);
            match locale {
                AppLocale::En => format!("{} ({})", domain.domain, semantic_category),
                AppLocale::Ar => format!("{} ({})", domain.domain, semantic_category),
                _ => format!("{}（{}）", domain.domain, semantic_category),
            }
        }
        _ => domain.domain.clone(),
    }
}

fn empty_value(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::ZhCn => "无",
        AppLocale::ZhTw => "無",
        AppLocale::En => "None",
        AppLocale::Ar => "لا يوجد",
    }
}

fn join_list(locale: AppLocale, items: Vec<String>) -> String {
    items.join(if locale == AppLocale::En { ", " } else if locale == AppLocale::Ar { "، " } else { "、" })
}

fn ai_system_prompt(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::ZhCn => {
            "你是一个专业的工作效率分析助手，帮助用户分析和总结每日工作。请使用简体中文回答。"
        }
        AppLocale::ZhTw => {
            "你是一位專業的工作效率分析助手，負責協助使用者分析與總結每日工作。請使用繁體中文回答。"
        }
        AppLocale::En => {
            "You are a professional work-efficiency analysis assistant. Summarize and analyze the user's workday in English."
        }
        AppLocale::Ar => {
            "أنت مساعد محترف في تحليل كفاءة العمل. قم بتلخيص وتحليل يوم عمل المستخدم باللغة العربية."
        }
    }
}

fn empty_ai_fallback_reason(locale: AppLocale) -> String {
    match locale {
        AppLocale::ZhCn => "返回空内容，已回退到基础模板".to_string(),
        AppLocale::ZhTw => "回傳空內容，已回退到基礎模板".to_string(),
        AppLocale::En => {
            "the model returned empty content, so the report fell back to the base template"
                .to_string()
        }
        AppLocale::Ar => {
            "عاد النموذج بمحتوى فارغ، لذا رجع التقرير إلى القالب الأساسي"
                .to_string()
        }
    }
}

#[derive(Clone, Copy)]
enum AiFallbackCategory {
    ConfigOrAuth,
    Timeout,
    Network,
    Service,
    MalformedResponse,
    Unknown,
}

fn classify_ai_request_error(error_text: &str) -> AiFallbackCategory {
    let normalized = error_text.to_lowercase();

    if [
        "未配置",
        "not configured",
        "api key",
        "invalidendpoint",
        "invalid endpoint",
        "endpoint not found",
        "endpoint does not exist",
        "unauthorized",
        "forbidden",
        "authentication",
        "invalid key",
        "incorrect api key",
        "http 401",
        "http 403",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        AiFallbackCategory::ConfigOrAuth
    } else if ["timed out", "timeout", "deadline has elapsed", "超时"]
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        AiFallbackCategory::Timeout
    } else if [
        "error decoding",
        "decode response",
        "invalid json",
        "json error",
        "malformed response",
        "serialization",
        "序列化错误",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        AiFallbackCategory::MalformedResponse
    } else if [
        "error sending request",
        "connection refused",
        "connection reset",
        "connect error",
        "dns error",
        "network error",
        "tcp error",
        "tls error",
        "certificate error",
        "请求失败",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        AiFallbackCategory::Network
    } else if [
        "api 错误",
        "http status",
        "status code",
        "rate limit",
        "too many requests",
        "service unavailable",
        "bad gateway",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        AiFallbackCategory::Service
    } else {
        AiFallbackCategory::Unknown
    }
}

fn request_ai_fallback_reason(locale: AppLocale, error_text: &str) -> String {
    let category = classify_ai_request_error(error_text);
    let reason = match (locale, category) {
        (AppLocale::ZhCn, AiFallbackCategory::ConfigOrAuth) => {
            "认证或配置不可用，已回退到基础模板"
        }
        (AppLocale::ZhCn, AiFallbackCategory::Timeout) => "请求超时，已回退到基础模板",
        (AppLocale::ZhCn, AiFallbackCategory::Network) => "网络连接失败，已回退到基础模板",
        (AppLocale::ZhCn, AiFallbackCategory::Service) => "服务返回错误，已回退到基础模板",
        (AppLocale::ZhCn, AiFallbackCategory::MalformedResponse) => {
            "响应格式异常，已回退到基础模板"
        }
        (AppLocale::ZhCn, AiFallbackCategory::Unknown) => "请求失败，已回退到基础模板",
        (AppLocale::ZhTw, AiFallbackCategory::ConfigOrAuth) => {
            "驗證或設定不可用，已回退到基礎模板"
        }
        (AppLocale::ZhTw, AiFallbackCategory::Timeout) => "請求逾時，已回退到基礎模板",
        (AppLocale::ZhTw, AiFallbackCategory::Network) => "網路連線失敗，已回退到基礎模板",
        (AppLocale::ZhTw, AiFallbackCategory::Service) => "服務回傳錯誤，已回退到基礎模板",
        (AppLocale::ZhTw, AiFallbackCategory::MalformedResponse) => {
            "回應格式異常，已回退到基礎模板"
        }
        (AppLocale::ZhTw, AiFallbackCategory::Unknown) => "請求失敗，已回退到基礎模板",
        (AppLocale::En, AiFallbackCategory::ConfigOrAuth) => {
            "authentication or AI configuration was unavailable, so the report fell back to the base template"
        }
        (AppLocale::En, AiFallbackCategory::Timeout) => {
            "the AI request timed out, so the report fell back to the base template"
        }
        (AppLocale::En, AiFallbackCategory::Network) => {
            "the network connection failed, so the report fell back to the base template"
        }
        (AppLocale::En, AiFallbackCategory::Service) => {
            "the AI service returned an error, so the report fell back to the base template"
        }
        (AppLocale::En, AiFallbackCategory::MalformedResponse) => {
            "the AI service returned an invalid response, so the report fell back to the base template"
        }
        (AppLocale::En, AiFallbackCategory::Unknown) => {
            "the AI request failed, so the report fell back to the base template"
        }
        (AppLocale::Ar, AiFallbackCategory::ConfigOrAuth) => {
            "تعذرت المصادقة أو إعداد الذكاء الاصطناعي، لذا رجع التقرير إلى القالب الأساسي"
        }
        (AppLocale::Ar, AiFallbackCategory::Timeout) => {
            "انتهت مهلة طلب الذكاء الاصطناعي، لذا رجع التقرير إلى القالب الأساسي"
        }
        (AppLocale::Ar, AiFallbackCategory::Network) => {
            "فشل الاتصال بالشبكة، لذا رجع التقرير إلى القالب الأساسي"
        }
        (AppLocale::Ar, AiFallbackCategory::Service) => {
            "أعادت خدمة الذكاء الاصطناعي خطأً، لذا رجع التقرير إلى القالب الأساسي"
        }
        (AppLocale::Ar, AiFallbackCategory::MalformedResponse) => {
            "أعادت خدمة الذكاء الاصطناعي استجابة غير صالحة، لذا رجع التقرير إلى القالب الأساسي"
        }
        (AppLocale::Ar, AiFallbackCategory::Unknown) => {
            "فشل طلب الذكاء الاصطناعي، لذا رجع التقرير إلى القالب الأساسي"
        }
    };

    reason.to_string()
}

/// 空正文响应的脱敏元数据（供日志）：只记录候选数量、finish reason、
/// 字段存在性等结构事实，绝不序列化原始内容——响应里可能包含
/// reasoning_content / thinking / thought 等隐藏推理原文。
fn openai_response_metadata(response: &serde_json::Value) -> String {
    let choices = response["choices"].as_array().map(|a| a.len()).unwrap_or(0);
    let message = &response["choices"][0]["message"];
    format!(
        "choices={choices} finish_reason={:?} has_content={} content_is_string={} has_reasoning_content={} has_tool_calls={}",
        response["choices"][0]["finish_reason"].as_str(),
        message.get("content").is_some(),
        message["content"].is_string(),
        message.get("reasoning_content").is_some(),
        message.get("tool_calls").is_some(),
    )
}

fn ollama_response_metadata(response: &serde_json::Value) -> String {
    format!(
        "has_response={} response_is_string={} has_thinking={} done_reason={:?}",
        response.get("response").is_some(),
        response["response"].is_string(),
        response.get("thinking").is_some(),
        response["done_reason"].as_str(),
    )
}

fn claude_response_metadata(response: &serde_json::Value) -> String {
    let block_types: Vec<&str> = response["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["type"].as_str())
                .collect()
        })
        .unwrap_or_default();
    format!(
        "block_types={block_types:?} stop_reason={:?}",
        response["stop_reason"].as_str(),
    )
}

fn gemini_response_metadata(response: &serde_json::Value) -> String {
    let candidates = response["candidates"].as_array().map(|a| a.len()).unwrap_or(0);
    let parts = response["candidates"][0]["content"]["parts"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let thought_parts = response["candidates"][0]["content"]["parts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|p| p["thought"].as_bool() == Some(true))
                .count()
        })
        .unwrap_or(0);
    format!(
        "candidates={candidates} parts={parts} thought_parts={thought_parts} finish_reason={:?}",
        response["candidates"][0]["finishReason"].as_str(),
    )
}

/// 从 OpenAI 兼容响应中提取正文。
/// 对 content 为 null / 缺失 / 类型异常保持宽容（推理模型、截断输出常见），
/// 一律返回空串交给上层"空内容"友好回退，不再误报"响应格式异常"。
/// 思考内容（reasoning_content）不是最终回答，不作为正文来源。
fn extract_openai_text(response: &serde_json::Value) -> String {
    let text = response["choices"]
        .as_array()
        .and_then(|choices| {
            choices.iter().find_map(|choice| {
                choice["message"]["content"]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
            })
        })
        .unwrap_or("")
        .to_string();
    if text.is_empty() {
        log::warn!(
            "OpenAI 兼容响应未提取到可用正文: {}",
            openai_response_metadata(response)
        );
    }
    text
}

/// 从 Ollama /api/generate 响应中提取正文；字段缺失按空内容回退处理。
fn extract_ollama_text(response: &serde_json::Value) -> String {
    let text = response["response"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        log::warn!(
            "Ollama 响应未提取到可用正文: {}",
            ollama_response_metadata(response)
        );
    }
    text
}

/// 从 Claude 响应中提取正文：拼接所有 text 块，跳过 thinking 块；无 text 块返回空串。
fn extract_claude_text(response: &serde_json::Value) -> String {
    let mut text = String::new();
    if let Some(blocks) = response["content"].as_array() {
        for block in blocks {
            if block["type"].as_str() == Some("text") {
                if let Some(part) = block["text"].as_str() {
                    text.push_str(part);
                }
            }
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        log::warn!(
            "Claude 响应未提取到可用正文: {}",
            claude_response_metadata(response)
        );
    }
    text
}

/// 从 Gemini 响应中提取正文：拼接所有非思考 part 的文本。
/// 思考模型的 thought part 与安全拦截（candidates 为空）都不会误报格式异常。
fn extract_gemini_text(response: &serde_json::Value) -> String {
    let mut text = String::new();
    if let Some(parts) = response["candidates"][0]["content"]["parts"].as_array() {
        for part in parts {
            if part["thought"].as_bool() == Some(true) {
                continue;
            }
            if let Some(part_text) = part["text"].as_str() {
                text.push_str(part_text);
            }
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        log::warn!(
            "Gemini 响应未提取到可用正文: {}",
            gemini_response_metadata(response)
        );
    }
    text
}

fn api_response_error(provider: &str, status: StatusCode, error_text: &str) -> AppError {
    let detail = error_text.trim();
    let message = if detail.is_empty() {
        format!("{provider} API 错误 (HTTP {})", status.as_u16())
    } else {
        format!("{provider} API 错误 (HTTP {}): {detail}", status.as_u16())
    };
    AppError::Analysis(message)
}

/// 检测是否因 max_tokens 设置过大导致 400（模型输出上限低于请求值）
fn is_max_tokens_too_large(error_text: &str) -> bool {
    let lower = error_text.to_lowercase();
    lower.contains("max_tokens")
        || lower.contains("maxtokens")
        || lower.contains("max_output_tokens")
        || lower.contains("maxoutputtokens")
        || lower.contains("too many tokens")
}

/// 检测是否因缺少必填的 max_tokens 参数导致 400
fn is_max_tokens_required(error_text: &str) -> bool {
    let lower = error_text.to_lowercase();
    (lower.contains("max_tokens") || lower.contains("maxtokens"))
        && (lower.contains("required") || lower.contains("must be") || lower.contains("missing"))
}

fn openai_compatible_chat_completion_urls(endpoint: &str) -> Vec<String> {
    let base = endpoint.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }

    if base.ends_with("/chat/completions") {
        return vec![base.to_string()];
    }

    let mut urls = vec![format!("{base}/chat/completions")];
    if !base.ends_with("/v1") {
        urls.push(format!("{base}/v1/chat/completions"));
    }
    urls.dedup();
    urls
}

/// 摘要上传分析器
/// 只上传统计摘要，不上传原始截图
pub struct SummaryAnalyzer {
    provider: AiProvider,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    custom_prompt: String,
    system_prompt_override: Option<String>,
    locale: AppLocale,
    pinned_blocks: Vec<String>,
    cached_ai_order: Option<Vec<String>>,
    /// AI 请求预算（来自用户配置的日报生成超时）
    ai_request_timeout: Duration,
    /// 是否启用思考模式（None = 沿用服务端默认）
    enable_thinking: Option<bool>,
    /// 思考 token 预算上限（None = 不限制）
    thinking_budget: Option<u32>,
    /// 单次生成最大 token 数（None = 沿用服务端默认）
    max_output_tokens: Option<u32>,
    client: Client,
}

impl SummaryAnalyzer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model_config: &crate::config::ModelConfig,
        custom_prompt: &str,
        system_prompt_override: Option<&str>,
        locale: AppLocale,
        pinned_blocks: Vec<String>,
        cached_ai_order: Option<Vec<String>>,
        ai_request_timeout_secs: u64,
    ) -> Self {
        let client = Client::builder()
            .timeout(summary_request_timeout(ai_request_timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            provider: model_config.provider,
            endpoint: model_config.endpoint.clone(),
            model: model_config.model.clone(),
            api_key: model_config.api_key.clone(),
            custom_prompt: custom_prompt.to_string(),
            system_prompt_override: system_prompt_override.map(|s| s.to_string()),
            locale,
            pinned_blocks,
            cached_ai_order,
            ai_request_timeout: Duration::from_secs(ai_request_timeout_secs),
            enable_thinking: model_config.enable_thinking,
            thinking_budget: model_config.thinking_budget,
            max_output_tokens: model_config.max_output_tokens,
            client,
        }
    }

    /// 完整生成参数（含 max_output_tokens），用于 Ollama/Claude/Gemini 路径。
    fn generation_params(&self) -> crate::generation_params::GenerationParams {
        crate::generation_params::GenerationParams {
            enable_thinking: self.enable_thinking,
            thinking_budget: self.thinking_budget,
            max_output_tokens: self.max_output_tokens,
        }
    }

    /// 仅思考字段（不含 max_output_tokens）：OpenAI 兼容路径的 max_tokens
    /// 由降档阶梯管理，思考字段才走能力映射。
    fn thinking_params(&self) -> crate::generation_params::GenerationParams {
        crate::generation_params::GenerationParams {
            enable_thinking: self.enable_thinking,
            thinking_budget: self.thinking_budget,
            max_output_tokens: None,
        }
    }

    async fn generate_with_ollama(&self, prompt: &str) -> Result<String> {
        let mut body = json!({
            "model": self.model,
            "prompt": format!("{}\n\n{}", ai_system_prompt(self.locale), prompt),
            "stream": false,
        });
        // 思考开关（think）与输出上限（options.num_predict）按 Ollama 协议映射
        self.generation_params().apply_to_ollama(&mut body);
        let response = self
            .client
            .post(format!("{}/api/generate", self.endpoint))
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::Analysis(format!(
                "Ollama API 错误: {}",
                response.status()
            )));
        }

        let result: serde_json::Value = response.json().await?;
        Ok(extract_ollama_text(&result))
    }
    async fn generate_with_openai_compatible(&self, prompt: &str) -> Result<String> {
        // 用户显式配置了最大 token 数：直接使用，不再自动降档
        if let Some(configured) = self.max_output_tokens {
            return self.try_openai_compatible_request(prompt, Some(configured)).await;
        }
        // 第一轮：不设 max_tokens，让模型用自身默认值
        match self.try_openai_compatible_request(prompt, None).await {
            Ok(content) => return Ok(content),
            Err(AppError::Analysis(ref msg)) if is_max_tokens_too_large(msg) => {
                log::info!("max_tokens 过大，降到 2048 重试");
            }
            Err(AppError::Analysis(ref msg)) if is_max_tokens_required(msg) => {
                log::info!("max_tokens 必填，补上 4096 重试");
                return self.try_openai_compatible_request(prompt, Some(4096)).await;
            }
            Err(e) => return Err(e),
        }
        // 第二轮：降到 2048
        match self.try_openai_compatible_request(prompt, Some(2048)).await {
            Ok(content) => return Ok(content),
            Err(AppError::Analysis(ref msg)) if is_max_tokens_too_large(msg) => {
                log::info!("max_tokens=2048 仍超限，降到 1024 重试");
            }
            Err(e) => return Err(e),
        }
        // 第三轮：降到 1024
        self.try_openai_compatible_request(prompt, Some(1024)).await
    }

    async fn try_openai_compatible_request(
        &self,
        prompt: &str,
        max_tokens: Option<u32>,
    ) -> Result<String> {
        let mut payload = json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": ai_system_prompt(self.locale)
                },
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "temperature": 0.2,
            "stream": false,
        });
        if let Some(tokens) = max_tokens {
            payload["max_tokens"] = json!(tokens);
        }
        // 思考模式参数按提供商协议映射（Qwen 顶层字段 / SiliconFlow
        // chat_template_kwargs / 其余未确认支持的提供商不发送）。
        // max_tokens 由上方降档阶梯管理，这里只应用思考字段。
        self.thinking_params()
            .apply_to_openai_compatible(self.provider, &mut payload, false);

        let mut last_error: Option<String> = None;

        for url in openai_compatible_chat_completion_urls(&self.endpoint) {
            let mut request = self.client.post(&url).json(&payload);

            if let Some(api_key) = &self.api_key {
                if !api_key.is_empty() {
                    request = request.header("Authorization", format!("Bearer {api_key}"));
                }
            }

            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(format!("{url} 请求失败: {error}"));
                    continue;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let error_text = response.text().await.unwrap_or_default();
                last_error = Some(format!("{url} API 错误 ({status}): {error_text}"));
                if status.is_client_error() {
                    break;
                }
                continue;
            }

            let result: serde_json::Value = response.json().await?;
            return Ok(extract_openai_text(&result));
        }

        Err(AppError::Analysis(last_error.unwrap_or_else(|| {
            "API 请求失败：未生成可用请求地址".to_string()
        })))
    }

    async fn generate_with_claude(&self, prompt: &str) -> Result<String> {
        let api_key = self.api_key.as_deref().unwrap_or("");
        if api_key.is_empty() {
            return Err(AppError::Analysis("Claude API Key 未配置".to_string()));
        }

        // Claude API 强制要求 max_tokens。用户显式配置了上限时直接使用，
        // 否则走 4096 → 2048 → 1024 降档阶梯。
        let max_tokens_steps: Vec<u32> = if let Some(configured) = self.max_output_tokens {
            vec![configured]
        } else {
            vec![4096, 2048, 1024]
        };
        let thinking = self.thinking_params();

        for &base_max_tokens in &max_tokens_steps {
            let mut body = json!({
                "model": self.model,
                "messages": [
                    {
                        "role": "user",
                        "content": prompt
                    }
                ],
                "system": ai_system_prompt(self.locale)
            });
            // 思考模式（thinking 块）与 max_tokens 按 Anthropic 协议映射；
            // 自动保证 max_tokens > budget_tokens（Anthropic 硬性要求）
            let effective_max_tokens = thinking.apply_to_claude(&mut body, base_max_tokens);
            let response = self
                .client
                .post(format!("{}/messages", self.endpoint))
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await?;

            if response.status().is_success() {
                let result: serde_json::Value = response.json().await?;
                return Ok(extract_claude_text(&result));
            }

            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            if is_max_tokens_too_large(&error_text) {
                log::info!("Claude max_tokens={effective_max_tokens} 超限，降档重试");
                continue;
            }
            return Err(api_response_error("Claude", status, &error_text));
        }

        Err(AppError::Analysis(
            "Claude 模型不支持足够的输出长度，请更换模型".to_string(),
        ))
    }

    async fn generate_with_gemini(&self, prompt: &str) -> Result<String> {
        let api_key = self.api_key.as_deref().unwrap_or("");
        if api_key.is_empty() {
            return Err(AppError::Analysis("Gemini API Key 未配置".to_string()));
        }

        // API Key 走 x-goog-api-key 请求头（见 try_gemini_request），
        // 不再拼进 URL query，避免泄漏到日志/代理记录
        let url = format!("{}/models/{}:generateContent", self.endpoint, self.model);

        let text = format!("{}\n\n{}", ai_system_prompt(self.locale), prompt);

        // 用户显式配置了最大 token 数：直接使用，不再自动降档
        if let Some(configured) = self.max_output_tokens {
            return self.try_gemini_request(&url, &text, Some(configured)).await;
        }
        // 第一轮：不设 maxOutputTokens
        match self.try_gemini_request(&url, &text, None).await {
            Ok(content) => return Ok(content),
            Err(AppError::Analysis(ref msg)) if is_max_tokens_too_large(msg) => {
                log::info!("Gemini max_tokens 过大，降到 2048 重试");
            }
            Err(AppError::Analysis(ref msg)) if is_max_tokens_required(msg) => {
                log::info!("Gemini max_tokens 必填，补上 4096 重试");
                return self.try_gemini_request(&url, &text, Some(4096)).await;
            }
            Err(e) => return Err(e),
        }
        // 第二轮：2048
        match self.try_gemini_request(&url, &text, Some(2048)).await {
            Ok(content) => return Ok(content),
            Err(AppError::Analysis(ref msg)) if is_max_tokens_too_large(msg) => {
                log::info!("Gemini max_tokens=2048 仍超限，降到 1024 重试");
            }
            Err(e) => return Err(e),
        }
        // 第三轮：1024
        self.try_gemini_request(&url, &text, Some(1024)).await
    }

    async fn try_gemini_request(
        &self,
        url: &str,
        text: &str,
        max_output_tokens: Option<u32>,
    ) -> Result<String> {
        let mut config = json!({ "temperature": 0.2 });
        if let Some(tokens) = max_output_tokens {
            config["maxOutputTokens"] = json!(tokens);
        }
        // 思考模式按 Gemini 协议映射（generationConfig.thinkingConfig）
        self.thinking_params()
            .apply_to_gemini_generation_config(&mut config);

        let response = self
            .client
            .post(url)
            .header("x-goog-api-key", self.api_key.as_deref().unwrap_or(""))
            .json(&json!({
                "contents": [{ "parts": [{ "text": text }] }],
                "generationConfig": config
            }))
            .send()
            .await?;

        if response.status().is_success() {
            let result: serde_json::Value = response.json().await?;
            return Ok(extract_gemini_text(&result));
        }

        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        Err(api_response_error("Gemini", status, &error_text))
    }

    async fn generate_ai_content(&self, prompt: &str) -> Result<String> {
        log::info!(
            "generate_ai_content: provider={:?}, endpoint={}, model={}, prompt_len={}",
            self.provider,
            self.endpoint,
            self.model,
            prompt.len()
        );
        match self.provider {
            AiProvider::Ollama => self.generate_with_ollama(prompt).await,
            AiProvider::Claude => self.generate_with_claude(prompt).await,
            AiProvider::Gemini => self.generate_with_gemini(prompt).await,
            _ => self.generate_with_openai_compatible(prompt).await,
        }
    }

    fn extract_keywords(&self, activities: &[Activity]) -> Vec<String> {
        let mut keywords = Vec::new();

        for activity in activities {
            let title_words = activity
                .window_title
                .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                .filter(|word| {
                    let len = word.chars().count();
                    (2..=30).contains(&len)
                })
                .take(3)
                .collect::<Vec<_>>();

            for word in title_words {
                let item = word.to_string();
                if !keywords.contains(&item) {
                    keywords.push(item);
                }
            }

            if let Some(ocr_text) = &activity.ocr_text {
                let ocr_words = ocr_text
                    .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                    .filter(|word| {
                        let len = word.chars().count();
                        (2..=20).contains(&len)
                            && word.chars().all(|c| c.is_alphabetic() || c >= '\u{4e00}')
                    })
                    .take(5)
                    .collect::<Vec<_>>();

                for word in ocr_words {
                    let item = word.to_string();
                    if !keywords.contains(&item) && keywords.len() < 30 {
                        keywords.push(item);
                    }
                }
            }
        }

        keywords.truncate(30);
        keywords
    }

    fn build_ai_prompt(&self, date: &str, stats: &DailyStats, activities: &[Activity]) -> String {
        let apps_list = join_list(
            self.locale,
            stats
                .app_usage
                .iter()
                .take(8)
                .map(|app| {
                    format!(
                        "{} ({})",
                        app.app_name,
                        format_duration_for_locale(app.duration, self.locale)
                    )
                })
                .collect(),
        );

        let urls_list = join_list(
            self.locale,
            stats
                .domain_usage
                .iter()
                .take(5)
                .map(|domain| format_domain_label(domain, self.locale, &HashMap::new()))
                .collect(),
        );

        let keywords = self.extract_keywords(activities);
        let top_keywords = join_list(self.locale, keywords.into_iter().take(8).collect());
        let hourly_summary = generate_hourly_activity_summary_for_locale(stats, self.locale)
            .unwrap_or_else(|| match self.locale {
                AppLocale::ZhCn => "暂无按小时活跃度数据".to_string(),
                AppLocale::ZhTw => "暫無按小時活躍度資料".to_string(),
                AppLocale::En => "No hourly activity data available".to_string(),
                AppLocale::Ar => "لا تتوفر بيانات نشاط بالساعة".to_string(),
            });

        let timeline = generate_activity_timeline(activities, self.locale);

        // 为 override 分支保留引用（base_prompt 构建后会 move 这些变量）
        let apps_list_for_override = apps_list.clone();
        let urls_list_for_override = urls_list.clone();
        let keywords_for_override = top_keywords.clone();

        let base_prompt = match self.locale {
            AppLocale::ZhCn => format!(
                r#"请基于以下数据，生成一份面向用户的工作日报 AI 分析补充。重点是提炼信息和给出洞察，不要逐条复述原始数据。

【日期】
{date}

【今日原始数据】
工作时长：{}
主要应用：{}
访问网站：{}
按小时活跃度：{}
屏幕内容关键词：{}

【活动时间线】
{timeline}

【核心要求】
1. 结合应用、网站和关键词，推断今天的工作重心。
2. 结合时长分布判断专注状态和节奏。
3. 给出 1 到 2 条具体建议。
4. 如果某项数据缺失，请明确说明未获取到，不要编造。

【输出格式】
请严格按以下四个三级标题输出，并使用简体中文：

### 观察
用 1 段总结今天最明显的工作模式。

### 证据
列出 2 到 4 条来自应用、网站、关键词或时段分布的具体依据。

### 建议
给出 1 到 2 条可执行建议。

### 小结
用 1 句话收束今天的工作状态。"#,
                format_duration_for_locale(stats.total_duration, self.locale),
                if apps_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    apps_list
                },
                if urls_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    urls_list
                },
                hourly_summary,
                if top_keywords.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    top_keywords
                },
                timeline = timeline,
            ),
            AppLocale::ZhTw => format!(
                r#"請根據以下資料，生成一份面向使用者的工作日報 AI 分析補充。重點是提煉資訊與給出洞察，不要逐條重述原始資料。

【日期】
{date}

【今日原始資料】
工作時長：{}
主要應用：{}
造訪網站：{}
按小時活躍度：{}
畫面內容關鍵詞：{}

【活動時間線】
{timeline}

【核心要求】
1. 結合應用、網站與關鍵詞，推斷今天的工作重心。
2. 結合時長分布判斷專注狀態與節奏。
3. 給出 1 到 2 條具體建議。
4. 如果某項資料缺失，請明確說明未取得，不要編造。

【輸出格式】
請嚴格按以下四個三級標題輸出，並使用繁體中文：

### 觀察
用 1 段總結今天最明顯的工作模式。

### 證據
列出 2 到 4 條來自應用、網站、關鍵詞或時段分布的具體依據。

### 建議
給出 1 到 2 條可執行建議。

### 小結
用 1 句話收束今天的工作狀態。"#,
                format_duration_for_locale(stats.total_duration, self.locale),
                if apps_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    apps_list
                },
                if urls_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    urls_list
                },
                hourly_summary,
                if top_keywords.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    top_keywords
                },
                timeline = timeline,
            ),
            AppLocale::En => format!(
                r#"Use the data below to write the AI analysis section of a daily work report. Focus on insight and synthesis rather than repeating raw numbers line by line.

[Date]
{date}

[Raw data]
Work duration: {}
Main apps: {}
Visited websites: {}
Hourly activity: {}
Screen-content keywords: {}

[Activity Timeline]
{timeline}

[Requirements]
1. Infer the user's main work focus from apps, websites, and keywords.
2. Assess focus and rhythm from the time distribution.
3. Give 1 to 2 concrete suggestions.
4. If a data point is missing, say so clearly instead of making it up.

[Output format]
Write in English and use exactly these four level-3 Markdown headings:

### Observation
Summarize the clearest work pattern in one paragraph.

### Evidence
List 2 to 4 concrete signals from apps, websites, keywords, or hourly distribution.

### Suggestions
Give 1 to 2 actionable suggestions.

### Wrap-up
Close with one sentence about the day's work state."#,
                format_duration_for_locale(stats.total_duration, self.locale),
                if apps_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    apps_list
                },
                if urls_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    urls_list
                },
                hourly_summary,
                if top_keywords.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    top_keywords
                },
                timeline = timeline,
            ),
            AppLocale::Ar => format!(
                r#"استخدم البيانات أدناه لكتابة قسم تحليل الذكاء الاصطناعي لتقرير العمل اليومي. ركز على الرؤى والتلخيص بدلاً من تكرار الأرقام الخام سطراً بسطر.

[التاريخ]
{date}

[البيانات الأولية]
مدة العمل: {}
أهم التطبيقات: {}
المواقع المزارة: {}
النشاط بالساعة: {}
الكلمات المفتاحية للشاشة: {}

[سجل الأنشطة الزمني]
{timeline}

[المتطلبات]
1. استنتج تركيز العمل الرئيسي للمستخدم من التطبيقات والمواقع والكلمات المفتاحية.
2. قم بتقييم التركيز والإيقاع من توزيع الوقت.
3. قدم اقتراحاً واحداً أو اقتراحين ملموسين.
4. إذا كانت إحدى البيانات مفقودة، اذكر ذلك بوضوح بدلاً من اختلاقها.

[تنسيق الإخراج]
اكتب باللغة العربية واستخدم العناوين الأربعة من المستوى الثالث التالية بالضبط:

### الملاحظة
لخص نمط العمل الأكثر وضوحاً في فقرة واحدة.

### الأدلة
اذكر من 2 إلى 4 إشارات ملموسة من التطبيقات أو المواقع أو الكلمات المفتاحية أو التوزيع بالساعة.

### الاقتراحات
قدم اقتراحاً واحداً أو اقتراحين قابلين للتنفيذ.

### الخلاصة
اختتم بجملة واحدة حول حالة العمل في هذا اليوم."#,
                format_duration_for_locale(stats.total_duration, self.locale),
                if apps_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    apps_list
                },
                if urls_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    urls_list
                },
                hourly_summary,
                if top_keywords.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    top_keywords
                },
                timeline = timeline,
            ),
        };

        // 如果用户覆盖了系统提示词模板，则用它替代硬编码的 base_prompt
        let final_prompt = if let Some(ref override_prompt) = self.system_prompt_override {
            let trimmed = override_prompt.trim();
            if trimmed.is_empty() {
                base_prompt
            } else {
                // 将日期和统计数据注入到用户模板中
                let injected = match self.locale {
                    AppLocale::ZhCn => format!(
                        "【日期】\n{}\n\n【今日原始数据】\n工作时长：{}\n主要应用：{}\n访问网站：{}\n按小时活跃度：{}\n屏幕内容关键词：{}\n\n【活动时间线】\n{}",
                        date,
                        format_duration_for_locale(stats.total_duration, self.locale),
                        if apps_list_for_override.is_empty() { empty_value(self.locale).to_string() } else { apps_list_for_override.clone() },
                        if urls_list_for_override.is_empty() { empty_value(self.locale).to_string() } else { urls_list_for_override.clone() },
                        hourly_summary,
                        if keywords_for_override.is_empty() { empty_value(self.locale).to_string() } else { keywords_for_override.clone() },
                        timeline,
                    ),
                    AppLocale::ZhTw => format!(
                        "【日期】\n{}\n\n【今日原始資料】\n工作時長：{}\n主要應用：{}\n造訪網站：{}\n按小時活躍度：{}\n畫面內容關鍵詞：{}\n\n【活動時間線】\n{}",
                        date,
                        format_duration_for_locale(stats.total_duration, self.locale),
                        if apps_list_for_override.is_empty() { empty_value(self.locale).to_string() } else { apps_list_for_override.clone() },
                        if urls_list_for_override.is_empty() { empty_value(self.locale).to_string() } else { urls_list_for_override.clone() },
                        hourly_summary,
                        if keywords_for_override.is_empty() { empty_value(self.locale).to_string() } else { keywords_for_override.clone() },
                        timeline,
                    ),
                    AppLocale::En => format!(
                        "[Date]\n{}\n\n[Raw Data]\nWork duration: {}\nTop apps: {}\nWebsites: {}\nHourly activity: {}\nScreen keywords: {}\n\n[Activity Timeline]\n{}",
                        date,
                        format_duration_for_locale(stats.total_duration, self.locale),
                        if apps_list_for_override.is_empty() { empty_value(self.locale).to_string() } else { apps_list_for_override.clone() },
                        if urls_list_for_override.is_empty() { empty_value(self.locale).to_string() } else { urls_list_for_override.clone() },
                        hourly_summary,
                        if keywords_for_override.is_empty() { empty_value(self.locale).to_string() } else { keywords_for_override.clone() },
                        timeline,
                    ),
                    AppLocale::Ar => format!(
                        "[التاريخ]\n{}\n\n[البيانات الأولية]\nمدة العمل: {}\nأهم التطبيقات: {}\nالمواقع: {}\nالنشاط بالساعة: {}\nالكلمات المفتاحية للشاشة: {}\n\n[سجل الأنشطة الزمني]\n{}",
                        date,
                        format_duration_for_locale(stats.total_duration, self.locale),
                        if apps_list_for_override.is_empty() { empty_value(self.locale).to_string() } else { apps_list_for_override.clone() },
                        if urls_list_for_override.is_empty() { empty_value(self.locale).to_string() } else { urls_list_for_override.clone() },
                        hourly_summary,
                        if keywords_for_override.is_empty() { empty_value(self.locale).to_string() } else { keywords_for_override.clone() },
                        timeline,
                    ),
                };
                format!("{trimmed}\n\n{injected}")
            }
        } else {
            base_prompt
        };

        append_custom_prompt_for_locale(final_prompt, &self.custom_prompt, self.locale)
    }

    fn generate_fallback_ai_content(&self, apps_list: &str) -> String {
        match self.locale {
            AppLocale::ZhCn => format!(
                "### 观察\n\n今天主要围绕 {} 等工具推进工作，整体工作主线比较清晰。\n\n### 证据\n\n- 记录显示主要应用集中在当前工作工具上。\n- 当天存在连续活动记录，可作为工作节奏判断依据。\n\n### 建议\n\n建议为最重要的任务预留更完整的连续时间段，减少中途切换。\n\n### 小结\n\n今天保持了稳定推进，已经积累了不错的工作产出。\n\n---\n*注：由基础模板生成。配置 AI 模型后可获得更深入的智能分析。*",
                if apps_list.is_empty() { "多个应用".to_string() } else { apps_list.to_string() }
            ),
            AppLocale::ZhTw => format!(
                "### 觀察\n\n今天主要圍繞 {} 等工具推進工作，整體工作主線相對清晰。\n\n### 證據\n\n- 記錄顯示主要應用集中在目前工作工具上。\n- 當天存在連續活動記錄，可作為工作節奏判斷依據。\n\n### 建議\n\n建議為最重要的任務預留更完整的連續時間段，減少中途切換。\n\n### 小結\n\n今天維持了穩定推進，已經累積了不錯的工作產出。\n\n---\n*註：目前由基礎模板生成。配置 AI 模型後可獲得更深入的智慧分析。*",
                if apps_list.is_empty() { "多個應用".to_string() } else { apps_list.to_string() }
            ),
            AppLocale::En => format!(
                "### Observation\n\nToday's work mainly revolved around tools such as {}, and the overall direction stayed fairly clear.\n\n### Evidence\n\n- The recorded activity is concentrated around the main work tools.\n- The day includes continuous activity records that can support a rhythm assessment.\n\n### Suggestions\n\nReserve a longer uninterrupted block for the most important task to reduce context switching.\n\n### Wrap-up\n\nThe day moved forward at a stable pace and produced solid progress.\n\n---\n*Note: This section was generated from the base template because AI analysis was unavailable.*",
                if apps_list.is_empty() { "several apps".to_string() } else { apps_list.to_string() }
            ),
            AppLocale::Ar => format!(
                "### الملاحظة\n\nتمحور عمل اليوم بشكل رئيسي حول أدوات مثل {}، وبقي الاتجاه العام واضحاً إلى حد كبير.\n\n### الأدلة\n\n- يتركز النشاط المسجل حول أدوات العمل الرئيسية.\n- يتضمن اليوم سجلات نشاط مستمرة يمكن أن تدعم تقييم إيقاع العمل.\n\n### الاقتراحات\n\nتخصيص فترة أطول دون انقطاع للمهمة الأكثر أهمية لتقليل التبديل بين السياقات.\n\n### الخلاصة\n\nسار اليوم بوتيرة مستقرة وحقق تقدماً ملموساً.\n\n---\n*ملاحظة: تم إنشاء هذا القسم من القالب الأساسي لأن تحليل الذكاء الاصطناعي لم يكن متوفراً.*",
                if apps_list.is_empty() { "عدة تطبيقات".to_string() } else { apps_list.to_string() }
            ),
        }
    }

    /// 让 AI 基于当天工作模式决定统计区块的最优排列顺序。
    /// 失败时返回 None，调用方回退到默认顺序。
    async fn decide_block_order(&self, stats: &DailyStats) -> Option<Vec<String>> {
        let work_minutes = stats.total_duration / 60;
        let top_category = stats
            .category_usage
            .first()
            .map(|c| c.category.as_str())
            .unwrap_or("");
        let top_app = stats
            .app_usage
            .first()
            .map(|a| a.app_name.as_str())
            .unwrap_or("");
        let domain_count = stats.domain_usage.len();

        let prompt = format!(
            "You are a daily report layout editor. Based on today's work pattern, decide the best order for these report sections (most important first).\n\n\
Available sections: [\"CATEGORY_TABLE\", \"APP_USAGE_TABLE\", \"HOURLY_SUMMARY\", \"DOMAIN_USAGE_TABLE\"]\n\n\
Today's pattern: worked {work_minutes} min; top category: {top_category}; top app: {top_app}; visited {domain_count} websites.\n\n\
Rules:\n\
- Put the most relevant section first\n\
- If website usage is 0, omit DOMAIN_USAGE_TABLE\n\
- Return ONLY a JSON array of section names, e.g. [\"APP_USAGE_TABLE\",\"CATEGORY_TABLE\",\"HOURLY_SUMMARY\"]\n\
- Do not include sections not listed above"
        );

        let content = self.generate_ai_content(&prompt).await.ok()?;
        let trimmed = content
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim()
            .trim_end_matches("```")
            .trim();
        let parsed: Vec<String> = serde_json::from_str(trimmed).ok()?;
        if parsed.is_empty() {
            return None;
        }
        Some(parsed)
    }
}

#[async_trait]
impl Analyzer for SummaryAnalyzer {
    async fn generate_report(
        &self,
        date: &str,
        stats: &DailyStats,
        activities: &[Activity],
        _screenshots_dir: &Path,
        locale: AppLocale,
        category_name_overrides: HashMap<String, String>,
        semantic_name_overrides: HashMap<String, String>,
    ) -> Result<GeneratedReport> {
        log::info!("生成混合模式日报：固定模板 + AI 扩展");

        let mut report = String::new();

        match locale {
            AppLocale::ZhCn => {
                report.push_str(&format!("# 工作日报\n\n**日期：{date}**\n\n"));
            }
            AppLocale::ZhTw => {
                report.push_str(&format!("# 工作日報\n\n**日期：{date}**\n\n"));
            }
            AppLocale::En => {
                report.push_str(&format!("# Daily Report\n\n**Date:** {date}\n\n"));
            }
            AppLocale::Ar => {
                report.push_str(&format!("# تقرير اليوم\n\n**التاريخ:** {date}\n\n"));
            }
        }

        // 区块编排和正文分析共享同一个 AI 预算，确保在外层截止时间（用户配置）前仍能生成回退报告。
        let ai_deadline = tokio::time::Instant::now() + self.ai_request_timeout;

        // 统计区块：AI 编排顺序 + 用户偏好
        // 有缓存顺序时直接用（记忆性），否则调 LLM 排序。
        let default_order = default_summary_order();
        let (ai_order, order_to_cache) = if let Some(ref cached) = self.cached_ai_order {
            if !cached.is_empty() {
                (Some(cached.clone()), None) // 用缓存，不需要存
            } else {
                let decided = tokio::time::timeout_at(ai_deadline, self.decide_block_order(stats))
                    .await
                    .unwrap_or(None);
                (decided.clone(), decided) // 新排序，需要存
            }
        } else {
            let decided = tokio::time::timeout_at(ai_deadline, self.decide_block_order(stats))
                .await
                .unwrap_or(None);
            (decided.clone(), decided)
        };
        let ordered = match &ai_order {
            Some(order) => merge_ai_order(order, &default_order),
            None => default_order,
        };
        let blocks = apply_preferences(ordered, &self.pinned_blocks);
        let (stats_sections, section_count) = assemble_with_section_count(
            &blocks,
            stats,
            locale,
            &category_name_overrides,
            &semantic_name_overrides,
        );
        report.push_str(&stats_sections);

        let ai_analysis_title = match locale {
            AppLocale::ZhCn => "AI 分析",
            AppLocale::ZhTw => "AI 分析",
            AppLocale::En => "AI Analysis",
            AppLocale::Ar => "تحليل الذكاء الاصطناعي",
        };
        let apps_list = join_list(
            locale,
            stats
                .app_usage
                .iter()
                .take(8)
                .map(|app| {
                    format!(
                        "{} ({})",
                        app.app_name,
                        format_duration_for_locale(app.duration, locale)
                    )
                })
                .collect(),
        );

        let ai_prompt = self.build_ai_prompt(date, stats, activities);
        let ai_content = match tokio::time::timeout_at(
            ai_deadline,
            self.generate_ai_content(&ai_prompt),
        )
        .await
        {
            Ok(Ok(content)) if !content.is_empty() => (content, true, None),
            Ok(Ok(_)) => (
                self.generate_fallback_ai_content(&apps_list),
                false,
                Some(empty_ai_fallback_reason(locale)),
            ),
            Ok(Err(error)) => (
                self.generate_fallback_ai_content(&apps_list),
                false,
                Some(request_ai_fallback_reason(locale, &error.to_string())),
            ),
            Err(_) => (
                self.generate_fallback_ai_content(&apps_list),
                false,
                Some(request_ai_fallback_reason(locale, "request timeout")),
            ),
        };

        let mut ai_section = format_section_heading(locale, section_count + 1, ai_analysis_title);
        ai_section.push_str(&ai_content.0);
        report.push_str(&wrap_block(BLOCK_AI_ANALYSIS, &ai_section));

        // 活动时间线放在最底部
        let timeline = generate_activity_timeline(activities, locale);
        if !timeline.is_empty() {
            report.push_str("\n\n");
            report.push_str(&timeline);
        }

        Ok(GeneratedReport {
            content: report,
            used_ai: ai_content.1,
            fallback_reason: ai_content.2,
            ai_order: order_to_cache,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        api_response_error, claude_response_metadata, empty_ai_fallback_reason,
        extract_claude_text, extract_gemini_text, extract_ollama_text, extract_openai_text,
        gemini_response_metadata, ollama_response_metadata, openai_compatible_chat_completion_urls,
        openai_response_metadata, request_ai_fallback_reason, summary_request_timeout,
        SummaryAnalyzer,
    };
    use crate::analysis::Analyzer;
    use crate::analysis::AppLocale;
    use crate::config::AiProvider;
    use crate::database::{AppUsage, CategoryUsage, DailyStats, HourlyActivityBucket};
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Duration;

    fn sample_stats_for_ai_structure() -> DailyStats {
        DailyStats {
            total_duration: 5400,
            screenshot_count: 5,
            app_usage: vec![AppUsage {
                app_name: "VS Code".to_string(),
                duration: 3600,
                count: 2,
                executable_path: None,
                screenshot_url: None,
            }],
            category_usage: vec![CategoryUsage {
                category: "development".to_string(),
                duration: 5400,
            }],
            browser_duration: 0,
            url_usage: vec![],
            domain_usage: vec![],
            domain_total_count: 0,
            browser_usage: vec![],
            work_time_duration: 5400,
            overtime_duration: 0,
            hourly_activity_distribution: vec![HourlyActivityBucket {
                hour: 10,
                duration: 3600,
            }],
        }
    }

    fn test_analyzer(locale: AppLocale) -> SummaryAnalyzer {
        SummaryAnalyzer {
            provider: AiProvider::OpenAI,
            endpoint: "https://example.com/v1".to_string(),
            model: "test-model".to_string(),
            api_key: None,
            custom_prompt: String::new(),
            system_prompt_override: None,
            locale,
            pinned_blocks: Vec::new(),
            cached_ai_order: None,
            ai_request_timeout: Duration::from_secs(240),
            enable_thinking: None,
            thinking_budget: None,
            max_output_tokens: None,
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("测试 client 应可创建"),
        }
    }

    #[test]
    fn 日报_ai请求预算应来自配置且低于外层截止时间() {
        let config = crate::config::AppConfig::default();
        let budget = config.report_ai_request_timeout_secs();

        // 默认外层 300s → 预算 240s，留出回退模板与收尾余量
        assert_eq!(budget, 240);
        assert_eq!(
            summary_request_timeout(budget),
            Duration::from_secs(240)
        );
        assert!(budget < config.report_generation_timeout_secs);

        // 外层配置调大/调小时预算同步变化，且始终保留最低请求预算
        let mut big = crate::config::AppConfig {
            report_generation_timeout_secs: 1800,
            ..crate::config::AppConfig::default()
        };
        big.normalize();
        assert_eq!(big.report_ai_request_timeout_secs(), 1740);

        let mut small = crate::config::AppConfig {
            report_generation_timeout_secs: 60,
            ..crate::config::AppConfig::default()
        };
        small.normalize();
        assert_eq!(small.report_ai_request_timeout_secs(), 30);
    }

    #[test]
    fn ai回退原因应按安全类别返回面向前端的友好中文文案() {
        assert_eq!(
            empty_ai_fallback_reason(AppLocale::ZhCn),
            "返回空内容，已回退到基础模板"
        );
        assert_eq!(
            request_ai_fallback_reason(AppLocale::ZhCn, "Gemini API Key 未配置"),
            "认证或配置不可用，已回退到基础模板"
        );
        assert_eq!(
            request_ai_fallback_reason(AppLocale::ZhCn, "HTTP 401 Unauthorized"),
            "认证或配置不可用，已回退到基础模板"
        );
        assert_eq!(
            request_ai_fallback_reason(AppLocale::ZhCn, "operation timed out"),
            "请求超时，已回退到基础模板"
        );
        assert_eq!(
            request_ai_fallback_reason(
                AppLocale::ZhCn,
                "error sending request: connection refused"
            ),
            "网络连接失败，已回退到基础模板"
        );
        assert_eq!(
            request_ai_fallback_reason(AppLocale::ZhCn, "API 错误 (503 Service Unavailable)"),
            "服务返回错误，已回退到基础模板"
        );
        assert_eq!(
            request_ai_fallback_reason(AppLocale::ZhCn, "error decoding response body"),
            "响应格式异常，已回退到基础模板"
        );

        let safe_reason = request_ai_fallback_reason(
            AppLocale::ZhCn,
            "Authorization: Bearer secret-token; provider response: internal details",
        );
        assert_eq!(safe_reason, "请求失败，已回退到基础模板");
        assert!(!safe_reason.contains("secret-token"));
        assert!(!safe_reason.contains("internal details"));
    }

    #[test]
    fn ai响应无可用正文应返回空串交给空内容友好回退而非误报格式异常() {
        // 推理模型 content=null、choices 为空、content 类型异常、纯空白，
        // 都应宽容返回空串，由上层走"返回空内容"友好回退，而不是误报"响应格式异常"。
        for response in [
            serde_json::json!({ "choices": [] }),
            serde_json::json!({ "choices": [{ "message": { "content": null } }] }),
            serde_json::json!({ "choices": [{ "message": { "content": 42 } }] }),
            serde_json::json!({ "choices": [{ "message": { "content": "  " } }] }),
            serde_json::json!({}),
        ] {
            assert_eq!(
                extract_openai_text(&response),
                "",
                "应宽容返回空串: {response}"
            );
        }
        assert_eq!(extract_ollama_text(&serde_json::json!({})), "");
        assert_eq!(extract_claude_text(&serde_json::json!({})), "");
        assert_eq!(extract_gemini_text(&serde_json::json!({ "candidates": [] })), "");
    }

    #[test]
    fn ai响应正文提取应兼容推理模型与思考块() {
        // OpenAI 兼容：正常 content 应取出
        assert_eq!(
            extract_openai_text(&serde_json::json!({
                "choices": [{ "message": { "content": "  今日分析  " } }]
            })),
            "今日分析"
        );
        // Claude：跳过 thinking 块，拼接 text 块
        assert_eq!(
            extract_claude_text(&serde_json::json!({
                "content": [
                    { "type": "thinking", "thinking": "内部推理" },
                    { "type": "text", "text": "结论A" },
                    { "type": "text", "text": "结论B" }
                ]
            })),
            "结论A结论B"
        );
        // Gemini：跳过 thought part，拼接正文 part
        assert_eq!(
            extract_gemini_text(&serde_json::json!({
                "candidates": [{ "content": { "parts": [
                    { "text": "思考过程", "thought": true },
                    { "text": "最终回答" }
                ] } }]
            })),
            "最终回答"
        );
    }

    #[test]
    fn claude与gemini错误应保留http状态用于安全分类() {
        let auth_error = api_response_error(
            "Claude",
            reqwest::StatusCode::UNAUTHORIZED,
            "provider-specific body",
        );
        assert_eq!(
            request_ai_fallback_reason(AppLocale::ZhCn, &auth_error.to_string()),
            "认证或配置不可用，已回退到基础模板"
        );

        let service_error = api_response_error(
            "Gemini",
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "provider-specific body",
        );
        assert_eq!(
            request_ai_fallback_reason(AppLocale::ZhCn, &service_error.to_string()),
            "服务返回错误，已回退到基础模板"
        );
    }

    #[test]
    fn openai兼容端点应补齐_chat_completions_路径并兼容_v1_回退() {
        assert_eq!(
            openai_compatible_chat_completion_urls("https://api.deepseek.com"),
            vec![
                "https://api.deepseek.com/chat/completions".to_string(),
                "https://api.deepseek.com/v1/chat/completions".to_string()
            ]
        );
        assert_eq!(
            openai_compatible_chat_completion_urls("https://api.openai.com/v1"),
            vec!["https://api.openai.com/v1/chat/completions".to_string()]
        );
        assert_eq!(
            openai_compatible_chat_completion_urls("https://api.siliconflow.cn/v1"),
            vec!["https://api.siliconflow.cn/v1/chat/completions".to_string()]
        );
    }

    #[test]
    fn 已包含_chat_completions_的端点不应重复拼接() {
        assert_eq!(
            openai_compatible_chat_completion_urls(
                "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
            ),
            vec!["https://ark.cn-beijing.volces.com/api/v3/chat/completions".to_string()]
        );
        assert_eq!(
            openai_compatible_chat_completion_urls("https://example.com/v1/chat/completions/"),
            vec!["https://example.com/v1/chat/completions".to_string()]
        );
    }

    #[test]
    fn ai提示词应要求固定观察证据建议小结结构() {
        let analyzer = test_analyzer(AppLocale::ZhCn);
        let stats = sample_stats_for_ai_structure();
        let prompt = analyzer.build_ai_prompt("2026-06-22", &stats, &[]);

        assert!(prompt.contains("### 观察"));
        assert!(prompt.contains("### 证据"));
        assert!(prompt.contains("### 建议"));
        assert!(prompt.contains("### 小结"));
        assert!(!prompt.contains("**工作内容概述**"));
        assert!(!prompt.contains("**效率评估**"));
    }

    #[test]
    fn ai回退内容应使用与提示词一致的固定结构() {
        let analyzer = test_analyzer(AppLocale::ZhCn);
        let fallback = analyzer.generate_fallback_ai_content("VS Code（1小时）");

        assert!(fallback.contains("### 观察"));
        assert!(fallback.contains("### 证据"));
        assert!(fallback.contains("### 建议"));
        assert!(fallback.contains("### 小结"));
        assert!(!fallback.contains("### 工作内容概述"));
        assert!(!fallback.contains("### 效率评估"));
    }

    #[test]
    fn 英文ai提示词和回退内容应使用固定英文结构() {
        let analyzer = test_analyzer(AppLocale::En);
        let stats = sample_stats_for_ai_structure();
        let prompt = analyzer.build_ai_prompt("2026-06-22", &stats, &[]);
        let fallback = analyzer.generate_fallback_ai_content("VS Code (1h)");

        for content in [prompt, fallback] {
            assert!(content.contains("### Observation"));
            assert!(content.contains("### Evidence"));
            assert!(content.contains("### Suggestions"));
            assert!(content.contains("### Wrap-up"));
            assert!(!content.contains("**Work Summary**"));
            assert!(!content.contains("### Work Summary"));
        }
    }

    #[tokio::test]
    async fn summary生成的ai分析应带可管理段落标记() {
        let mut analyzer = test_analyzer(AppLocale::ZhCn);
        analyzer.endpoint = "http://127.0.0.1:1/v1".to_string();
        analyzer.cached_ai_order = Some(vec![
            "CATEGORY_TABLE".to_string(),
            "APP_USAGE_TABLE".to_string(),
            "HOURLY_SUMMARY".to_string(),
        ]);
        let stats = sample_stats_for_ai_structure();

        let report = analyzer
            .generate_report(
                "2026-06-22",
                &stats,
                &[],
                Path::new(""),
                AppLocale::ZhCn,
                HashMap::new(),
                HashMap::new(),
            )
            .await
            .expect("网络失败时也应生成 fallback 日报");

        assert!(report
            .content
            .contains("<!-- WR_BLOCK_START:AI_ANALYSIS -->"));
        assert!(report.content.contains("## 四、AI 分析"));
        assert!(report.content.contains("<!-- WR_BLOCK_END:AI_ANALYSIS -->"));
    }

    #[test]
    fn 空正文日志元数据不得包含原始推理内容() {
        // OpenAI 兼容：仅含 reasoning_content 的响应
        let openai = serde_json::json!({
            "choices": [{
                "message": { "content": null, "reasoning_content": "隐藏的推理原文-OPENAI" },
                "finish_reason": "stop"
            }]
        });
        let meta = openai_response_metadata(&openai);
        assert!(!meta.contains("隐藏的推理原文"), "日志元数据泄漏了推理内容: {meta}");
        assert!(meta.contains("has_reasoning_content=true"), "应记录字段存在性: {meta}");

        // Ollama：仅含 thinking 的响应
        let ollama = serde_json::json!({
            "response": "",
            "thinking": "隐藏的推理原文-OLLAMA"
        });
        let meta = ollama_response_metadata(&ollama);
        assert!(!meta.contains("隐藏的推理原文"), "日志元数据泄漏了推理内容: {meta}");
        assert!(meta.contains("has_thinking=true"), "应记录字段存在性: {meta}");

        // Claude：仅含 thinking 块的响应
        let claude = serde_json::json!({
            "content": [{ "type": "thinking", "thinking": "隐藏的推理原文-CLAUDE" }],
            "stop_reason": "end_turn"
        });
        let meta = claude_response_metadata(&claude);
        assert!(!meta.contains("隐藏的推理原文"), "日志元数据泄漏了推理内容: {meta}");
        assert!(meta.contains("thinking"), "应记录块类型: {meta}");

        // Gemini：仅含 thought part 的响应
        let gemini = serde_json::json!({
            "candidates": [{
                "content": { "parts": [{ "text": "隐藏的推理原文-GEMINI", "thought": true }] },
                "finishReason": "STOP"
            }]
        });
        let meta = gemini_response_metadata(&gemini);
        assert!(!meta.contains("隐藏的推理原文"), "日志元数据泄漏了推理内容: {meta}");
        assert!(meta.contains("thought_parts=1"), "应记录 thought part 数量: {meta}");
    }

    #[test]
    fn 仅含推理内容的响应应提取空正文走友好回退() {
        // OpenAI 兼容：content=null 且仅含 reasoning_content → 空串
        assert_eq!(
            extract_openai_text(&serde_json::json!({
                "choices": [{ "message": { "content": null, "reasoning_content": "推理" } }]
            })),
            ""
        );
        // Claude：仅 thinking 块 → 空串
        assert_eq!(
            extract_claude_text(&serde_json::json!({
                "content": [{ "type": "thinking", "thinking": "推理" }]
            })),
            ""
        );
        // Gemini：仅 thought part → 空串
        assert_eq!(
            extract_gemini_text(&serde_json::json!({
                "candidates": [{ "content": { "parts": [{ "text": "推理", "thought": true }] } }]
            })),
            ""
        );
        // Ollama：仅 thinking → 空串
        assert_eq!(
            extract_ollama_text(&serde_json::json!({ "response": "", "thinking": "推理" })),
            ""
        );
    }
}
