//! Provider/协议能力映射与生成参数（max_tokens / 思考模式）的统一应用点。
//!
//! 背景：设置页对每个模型档案暴露 enable_thinking / thinking_budget /
//! max_output_tokens 三项配置，但各家 API 的字段名与支持程度完全不同。
//! 本模块是唯一的"能力 → 协议字段"映射来源：
//! - 只有确认支持的提供商/协议才会写入扩展字段（严格校验请求体的端点
//!   不会因为未知字段返回 400）；
//! - 不支持的提供商由前端依据 [`GenerationCapabilities`] 禁用对应设置项，
//!   不允许"保存成功但实际忽略"。
//!
//! 各协议字段依据（2026-08 核对）：
//! - Ollama 原生 API：顶层 `think: bool`；token 上限走 `options.num_predict`。
//!   `chat_template_kwargs.enable_thinking` 在 Ollama 上不可靠，不使用。
//! - DashScope（Qwen）OpenAI 兼容模式：顶层 `enable_thinking` / `thinking_budget`；
//!   且非流式调用不允许 enable_thinking=true（服务端会 400）。
//! - vLLM/SGLang 类 OpenAI 兼容端点（SiliconFlow）：`chat_template_kwargs`。
//! - Anthropic Messages API：`thinking: {type, budget_tokens}`，
//!   且要求 budget_tokens < max_tokens；budget_tokens 最小 1024。
//! - Gemini generateContent：`generationConfig.thinkingConfig.thinkingBudget`
//!   （0 = 关闭，-1 = 动态）与 `generationConfig.maxOutputTokens`。

use crate::config::{AiProvider, ModelConfig};
use serde::Serialize;
use serde_json::{json, Value};

/// 提供商对生成参数的支持情况（序列化给前端，用于禁用不支持的设置项）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct GenerationCapabilities {
    /// 支持开/关思考模式（enable_thinking 语义）
    pub thinking_toggle: bool,
    /// 支持限制思考 token 预算（thinking_budget 语义）
    pub thinking_budget: bool,
    /// 支持限制单次生成 token 上限（max_output_tokens 语义）
    pub max_output_tokens: bool,
    /// 思考参数仅在流式调用中生效（DashScope 非流式禁止 enable_thinking=true）。
    /// 前端据此提示"日报等非流式路径沿用服务端默认"，避免误以为全场景生效。
    pub thinking_streaming_only: bool,
}

impl AiProvider {
    /// 该提供商的生成参数能力映射（唯一来源，前端与请求构造共用）。
    pub fn generation_capabilities(&self) -> GenerationCapabilities {
        match self {
            // Ollama：think 开关可用；没有思考预算参数
            AiProvider::Ollama => GenerationCapabilities {
                thinking_toggle: true,
                thinking_budget: false,
                max_output_tokens: true,
                thinking_streaming_only: false,
            },
            // DashScope 兼容模式：顶层 enable_thinking / thinking_budget
            AiProvider::Qwen => GenerationCapabilities {
                thinking_toggle: true,
                thinking_budget: true,
                max_output_tokens: true,
                thinking_streaming_only: true,
            },
            // vLLM/SGLang 托管：chat_template_kwargs
            AiProvider::SiliconFlow => GenerationCapabilities {
                thinking_toggle: true,
                thinking_budget: true,
                max_output_tokens: true,
                thinking_streaming_only: false,
            },
            // Anthropic：thinking.{type,budget_tokens}
            AiProvider::Claude => GenerationCapabilities {
                thinking_toggle: true,
                thinking_budget: true,
                max_output_tokens: true,
                thinking_streaming_only: false,
            },
            // Gemini：generationConfig.thinkingConfig
            AiProvider::Gemini => GenerationCapabilities {
                thinking_toggle: true,
                thinking_budget: true,
                max_output_tokens: true,
                thinking_streaming_only: false,
            },
            // 其余 OpenAI 兼容提供商未确认支持扩展字段：一律不发送，
            // 前端禁用设置项，避免严格端点 400 或静默失效。
            _ => GenerationCapabilities {
                thinking_toggle: false,
                thinking_budget: false,
                max_output_tokens: true,
                thinking_streaming_only: false,
            },
        }
    }
}

/// 用户配置的生成参数（来自模型档案；全部可选，None = 不干预服务端默认）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GenerationParams {
    pub enable_thinking: Option<bool>,
    pub thinking_budget: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// Anthropic 要求 budget_tokens >= 1024。
const CLAUDE_MIN_THINKING_BUDGET: u32 = 1024;
/// 启用思考时默认给思考阶段的预算（用户未配置 thinking_budget 时）。
const CLAUDE_DEFAULT_THINKING_BUDGET: u32 = 4096;

impl GenerationParams {
    pub fn from_model_config(config: &ModelConfig) -> Self {
        Self {
            enable_thinking: config.enable_thinking,
            thinking_budget: config.thinking_budget,
            max_output_tokens: config.max_output_tokens,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.enable_thinking.is_none() && self.thinking_budget.is_none() && self.max_output_tokens.is_none()
    }

    /// OpenAI 兼容请求体：按提供商协议写扩展字段。
    /// `streaming` 影响 Qwen：DashScope 非流式调用禁止 enable_thinking=true。
    pub fn apply_to_openai_compatible(
        &self,
        provider: AiProvider,
        body: &mut Value,
        streaming: bool,
    ) {
        if let Some(max_tokens) = self.max_output_tokens {
            body["max_tokens"] = json!(max_tokens);
        }

        match provider {
            AiProvider::Qwen => {
                // DashScope 文档参数：顶层 enable_thinking / thinking_budget。
                // 非流式调用只允许 enable_thinking=false（true 会 400），
                // 因此非流式时仅在用户显式关闭时发送。
                match self.enable_thinking {
                    Some(false) => body["enable_thinking"] = json!(false),
                    Some(true) if streaming => body["enable_thinking"] = json!(true),
                    _ => {}
                }
                if streaming {
                    if let Some(budget) = self.thinking_budget {
                        body["thinking_budget"] = json!(budget);
                    }
                }
            }
            AiProvider::SiliconFlow => {
                // vLLM/SGLang 约定：chat_template_kwargs 透传给聊天模板
                let mut kwargs = serde_json::Map::new();
                if let Some(enable) = self.enable_thinking {
                    kwargs.insert("enable_thinking".to_string(), json!(enable));
                }
                if let Some(budget) = self.thinking_budget {
                    kwargs.insert("thinking_budget".to_string(), json!(budget));
                }
                if !kwargs.is_empty() {
                    body["chat_template_kwargs"] = json!(kwargs);
                }
            }
            // 其余提供商：未确认支持，不发送任何扩展字段
            _ => {}
        }
    }

    /// Ollama 原生 API（/api/chat、/api/generate）请求体。
    pub fn apply_to_ollama(&self, body: &mut Value) {
        if let Some(enable) = self.enable_thinking {
            body["think"] = json!(enable);
        }
        if let Some(max_tokens) = self.max_output_tokens {
            body["options"]["num_predict"] = json!(max_tokens);
        }
    }

    /// Claude Messages API 请求体。返回最终使用的 max_tokens
    /// （Claude 强制要求 max_tokens，且启用思考时必须 > budget_tokens）。
    pub fn apply_to_claude(&self, body: &mut Value, default_max_tokens: u32) -> u32 {
        let mut max_tokens = self.max_output_tokens.unwrap_or(default_max_tokens);

        match self.enable_thinking {
            Some(true) => {
                let budget = self
                    .thinking_budget
                    .unwrap_or(CLAUDE_DEFAULT_THINKING_BUDGET)
                    .max(CLAUDE_MIN_THINKING_BUDGET);
                body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
                // Anthropic 要求 budget_tokens < max_tokens，否则 400
                if max_tokens <= budget {
                    max_tokens = budget + 1;
                }
            }
            Some(false) => {
                body["thinking"] = json!({ "type": "disabled" });
            }
            None => {
                // 仅配置了预算而未显式开关思考：按启用处理，否则预算无意义
                if let Some(budget) = self.thinking_budget {
                    let budget = budget.max(CLAUDE_MIN_THINKING_BUDGET);
                    body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
                    if max_tokens <= budget {
                        max_tokens = budget + 1;
                    }
                }
            }
        }

        body["max_tokens"] = json!(max_tokens);
        max_tokens
    }

    /// Gemini `generationConfig` 对象。
    pub fn apply_to_gemini_generation_config(&self, config: &mut Value) {
        if let Some(max_tokens) = self.max_output_tokens {
            config["maxOutputTokens"] = json!(max_tokens);
        }
        match (self.enable_thinking, self.thinking_budget) {
            (Some(false), _) => {
                // thinkingBudget = 0 是 Gemini 官方的关闭方式
                config["thinkingConfig"] = json!({ "thinkingBudget": 0 });
            }
            (Some(true), budget) | (None, budget @ Some(_)) => {
                // -1 = 动态预算（交由模型决定）
                config["thinkingConfig"] =
                    json!({ "thinkingBudget": budget.map(i64::from).unwrap_or(-1) });
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(
        enable_thinking: Option<bool>,
        thinking_budget: Option<u32>,
        max_output_tokens: Option<u32>,
    ) -> GenerationParams {
        GenerationParams {
            enable_thinking,
            thinking_budget,
            max_output_tokens,
        }
    }

    #[test]
    fn 能力映射应只标记已确认支持的提供商() {
        // 确认支持思考开关的提供商
        for provider in [
            AiProvider::Ollama,
            AiProvider::Qwen,
            AiProvider::SiliconFlow,
            AiProvider::Claude,
            AiProvider::Gemini,
        ] {
            assert!(
                provider.generation_capabilities().thinking_toggle,
                "{provider:?} 应支持思考开关"
            );
        }
        // Ollama 没有思考预算参数
        assert!(!AiProvider::Ollama.generation_capabilities().thinking_budget);
        // 只有 DashScope（Qwen）声明"思考仅流式生效"：非流式 enable_thinking=true 会 400
        assert!(AiProvider::Qwen.generation_capabilities().thinking_streaming_only);
        for provider in [
            AiProvider::Ollama,
            AiProvider::SiliconFlow,
            AiProvider::Claude,
            AiProvider::Gemini,
        ] {
            assert!(
                !provider.generation_capabilities().thinking_streaming_only,
                "{provider:?} 思考参数不应标记为仅流式"
            );
        }
        // 未确认支持的提供商一律不发送扩展字段
        for provider in [
            AiProvider::OpenAI,
            AiProvider::DeepSeek,
            AiProvider::Zhipu,
            AiProvider::Moonshot,
            AiProvider::Doubao,
            AiProvider::MiniMax,
            AiProvider::OpenRouter,
            AiProvider::Groq,
            AiProvider::XAI,
            AiProvider::Mistral,
            AiProvider::LmStudio,
            AiProvider::Custom,
        ] {
            let caps = provider.generation_capabilities();
            assert!(!caps.thinking_toggle, "{provider:?} 不应声明支持思考开关");
            assert!(!caps.thinking_budget, "{provider:?} 不应声明支持思考预算");
            assert!(caps.max_output_tokens, "{provider:?} 应支持 max_tokens");
            assert!(
                !caps.thinking_streaming_only,
                "{provider:?} 不支持思考时不应声明仅流式"
            );
        }
    }

    #[test]
    fn openai兼容路径应按提供商协议写思考字段() {
        // Qwen：顶层 enable_thinking / thinking_budget（流式）
        let mut body = json!({});
        params(Some(true), Some(2048), None).apply_to_openai_compatible(
            AiProvider::Qwen,
            &mut body,
            true,
        );
        assert_eq!(body["enable_thinking"], json!(true));
        assert_eq!(body["thinking_budget"], json!(2048));
        assert!(body.get("chat_template_kwargs").is_none());

        // Qwen 非流式：enable_thinking=true 不允许发送（DashScope 会 400）
        let mut body = json!({});
        params(Some(true), Some(2048), None).apply_to_openai_compatible(
            AiProvider::Qwen,
            &mut body,
            false,
        );
        assert!(body.get("enable_thinking").is_none());
        assert!(body.get("thinking_budget").is_none());

        // Qwen 非流式显式关闭：允许发送 false
        let mut body = json!({});
        params(Some(false), None, None).apply_to_openai_compatible(
            AiProvider::Qwen,
            &mut body,
            false,
        );
        assert_eq!(body["enable_thinking"], json!(false));

        // SiliconFlow：chat_template_kwargs
        let mut body = json!({});
        params(Some(false), None, None).apply_to_openai_compatible(
            AiProvider::SiliconFlow,
            &mut body,
            true,
        );
        assert_eq!(
            body["chat_template_kwargs"],
            json!({ "enable_thinking": false })
        );
        assert!(body.get("enable_thinking").is_none());

        // 未确认支持的提供商：不写任何思考字段（max_tokens 仍通用）
        let mut body = json!({});
        params(Some(true), Some(2048), Some(4096)).apply_to_openai_compatible(
            AiProvider::OpenAI,
            &mut body,
            true,
        );
        assert!(body.get("enable_thinking").is_none());
        assert!(body.get("chat_template_kwargs").is_none());
        assert!(body.get("thinking_budget").is_none());
        assert_eq!(body["max_tokens"], json!(4096));
    }

    #[test]
    fn ollama路径应使用think与num_predict() {
        let mut body = json!({});
        params(Some(false), Some(1024), Some(3000)).apply_to_ollama(&mut body);
        assert_eq!(body["think"], json!(false));
        assert_eq!(body["options"]["num_predict"], json!(3000));
        // Ollama 不支持思考预算：不发送
        assert!(body.get("thinking_budget").is_none());
        assert!(body.get("chat_template_kwargs").is_none());

        // 未配置时不写任何字段
        let mut body = json!({});
        params(None, None, None).apply_to_ollama(&mut body);
        assert!(body.get("think").is_none());
        assert!(body.get("options").is_none());
    }

    #[test]
    fn claude路径应写thinking块并保证max_tokens大于预算() {
        // 启用思考 + 预算：max_tokens 必须 > budget_tokens
        let mut body = json!({});
        let max_tokens = params(Some(true), Some(2048), Some(1600)).apply_to_claude(&mut body, 1600);
        assert_eq!(body["thinking"], json!({ "type": "enabled", "budget_tokens": 2048 }));
        assert_eq!(max_tokens, 2049);
        assert_eq!(body["max_tokens"], json!(2049));

        // 启用思考未配预算：默认预算，且预算不低于 Anthropic 最小值 1024
        let mut body = json!({});
        let max_tokens = params(Some(true), None, None).apply_to_claude(&mut body, 1600);
        assert_eq!(
            body["thinking"],
            json!({ "type": "enabled", "budget_tokens": 4096 })
        );
        assert_eq!(max_tokens, 4097);

        // 关闭思考
        let mut body = json!({});
        let max_tokens = params(Some(false), None, Some(800)).apply_to_claude(&mut body, 1600);
        assert_eq!(body["thinking"], json!({ "type": "disabled" }));
        assert_eq!(max_tokens, 800);
        assert_eq!(body["max_tokens"], json!(800));

        // 未配置：不写 thinking 块，max_tokens 用默认
        let mut body = json!({});
        let max_tokens = params(None, None, None).apply_to_claude(&mut body, 1600);
        assert!(body.get("thinking").is_none());
        assert_eq!(max_tokens, 1600);
    }

    #[test]
    fn gemini路径应写思考配置与输出上限() {
        // 关闭思考 → thinkingBudget = 0
        let mut config = json!({});
        params(Some(false), None, None).apply_to_gemini_generation_config(&mut config);
        assert_eq!(config["thinkingConfig"], json!({ "thinkingBudget": 0 }));

        // 开启思考 + 预算
        let mut config = json!({});
        params(Some(true), Some(1024), Some(8000)).apply_to_gemini_generation_config(&mut config);
        assert_eq!(config["thinkingConfig"], json!({ "thinkingBudget": 1024 }));
        assert_eq!(config["maxOutputTokens"], json!(8000));

        // 开启思考未配预算 → 动态（-1）
        let mut config = json!({});
        params(Some(true), None, None).apply_to_gemini_generation_config(&mut config);
        assert_eq!(config["thinkingConfig"], json!({ "thinkingBudget": -1 }));

        // 未配置 → 不写 thinkingConfig
        let mut config = json!({});
        params(None, None, None).apply_to_gemini_generation_config(&mut config);
        assert!(config.get("thinkingConfig").is_none());
    }
}
