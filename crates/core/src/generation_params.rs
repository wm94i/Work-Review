//! 模型生成参数：思考模式 / token 预算按 Provider 协议映射。
//!
//! 只有确认支持的协议才写入请求体；未支持的提供商不发送扩展字段，
//! 避免“设置已保存但实际被忽略”或严格端点 400。

use crate::config::{AiProvider, ModelConfig};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingProtocol {
    None,
    OllamaThink,
    DashScopeTopLevel,
    SiliconFlowKwargs,
    ClaudeThinking,
    GeminiBudget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationCapabilities {
    pub thinking: ThinkingProtocol,
    pub thinking_streaming_only: bool,
    pub thinking_budget: bool,
    pub max_output_tokens: bool,
}

impl GenerationCapabilities {
    pub fn to_json(self) -> Value {
        json!({
            "thinking": self.thinking != ThinkingProtocol::None,
            "thinkingStreamingOnly": self.thinking_streaming_only,
            "thinkingBudget": self.thinking_budget,
            "maxOutputTokens": self.max_output_tokens,
        })
    }
}

pub fn capabilities_for(provider: AiProvider) -> GenerationCapabilities {
    match provider {
        AiProvider::Ollama | AiProvider::LmStudio => GenerationCapabilities {
            thinking: ThinkingProtocol::OllamaThink,
            thinking_streaming_only: false,
            thinking_budget: false,
            max_output_tokens: true,
        },
        AiProvider::Qwen => GenerationCapabilities {
            thinking: ThinkingProtocol::DashScopeTopLevel,
            thinking_streaming_only: true,
            thinking_budget: true,
            max_output_tokens: true,
        },
        AiProvider::SiliconFlow => GenerationCapabilities {
            thinking: ThinkingProtocol::SiliconFlowKwargs,
            thinking_streaming_only: false,
            thinking_budget: false,
            max_output_tokens: true,
        },
        AiProvider::Claude => GenerationCapabilities {
            thinking: ThinkingProtocol::ClaudeThinking,
            thinking_streaming_only: false,
            thinking_budget: true,
            max_output_tokens: true,
        },
        AiProvider::Gemini => GenerationCapabilities {
            thinking: ThinkingProtocol::GeminiBudget,
            thinking_streaming_only: false,
            thinking_budget: true,
            max_output_tokens: true,
        },
        _ => GenerationCapabilities {
            thinking: ThinkingProtocol::None,
            thinking_streaming_only: false,
            thinking_budget: false,
            max_output_tokens: true,
        },
    }
}

pub fn resolved_max_tokens(config: &ModelConfig, default: u32) -> u32 {
    config
        .max_output_tokens
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn matches_model_family(bare: &str, family: &str) -> bool {
    match bare.strip_prefix(family) {
        Some("") => true,
        Some(rest) => matches!(rest.as_bytes()[0], b'-' | b'.'),
        None => false,
    }
}

/// 判断 OpenAI Chat Completions 模型是否需要改用 `max_completion_tokens` 且省略非默认采样参数
/// （OpenAI GPT-5 / GPT-6 / o 系列在推理模式下拒绝旧版 `max_tokens` 与非默认 `temperature`）。
/// 仅对 `AiProvider::OpenAI` 生效，避免向不支持该字段的第三方 OpenAI 兼容端点（含 `Custom`）发送不兼容参数；
/// 注：若 Azure / 代理将 GPT-6 映射为完全自定义的部署名，此启发式无法仅凭名称识别。
pub fn uses_openai_reasoning_chat_params(provider: AiProvider, model: &str) -> bool {
    if provider != AiProvider::OpenAI {
        return false;
    }
    let normalized = model.trim().to_ascii_lowercase();
    let bare = normalized
        .rsplit_once('/')
        .map(|(_, tail)| tail)
        .unwrap_or(&normalized);
    matches_model_family(bare, "gpt-5")
        || matches_model_family(bare, "gpt-6")
        || matches_model_family(bare, "o1")
        || matches_model_family(bare, "o3")
        || matches_model_family(bare, "o4")
}

pub fn openai_max_tokens_field(provider: AiProvider, model: &str) -> &'static str {
    if uses_openai_reasoning_chat_params(provider, model) {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

pub fn apply_openai_compatible(body: &mut Value, config: &ModelConfig, streaming: bool) {
    let cap = capabilities_for(config.provider);
    let max_tokens = resolved_max_tokens(config, 8192);
    if uses_openai_reasoning_chat_params(config.provider, &config.model) {
        if let Some(object) = body.as_object_mut() {
            object.remove("max_tokens");
            // OpenAI 新一代推理系模型在默认推理状态下拒绝非默认采样参数，交由服务端使用默认值。
            for field in ["temperature", "top_p"] {
                object.remove(field);
            }
        }
        body["max_completion_tokens"] = json!(max_tokens);
    } else {
        body["max_tokens"] = json!(max_tokens);
    }

    match cap.thinking {
        ThinkingProtocol::DashScopeTopLevel => {
            if let Some(enabled) = config.enable_thinking {
                if streaming || !enabled {
                    body["enable_thinking"] = json!(enabled);
                    if enabled {
                        if let Some(budget) = config.thinking_budget.filter(|value| *value > 0) {
                            body["thinking_budget"] = json!(budget);
                        }
                    }
                }
            }
        }
        ThinkingProtocol::SiliconFlowKwargs => {
            if let Some(enabled) = config.enable_thinking {
                body["chat_template_kwargs"] = json!({ "enable_thinking": enabled });
            }
        }
        _ => {}
    }
}

pub fn apply_ollama(body: &mut Value, config: &ModelConfig) {
    if let Some(enabled) = config.enable_thinking {
        body["think"] = json!(enabled);
    }
    let options = body
        .as_object_mut()
        .expect("ollama body is object")
        .entry("options")
        .or_insert_with(|| json!({}));
    options["num_predict"] = json!(resolved_max_tokens(config, 8192));
}

pub fn apply_claude(body: &mut Value, config: &ModelConfig, tools_present: bool) {
    let mut max_tokens = resolved_max_tokens(config, 1600);
    let model = config.model.trim().to_ascii_lowercase();
    let sonnet_5_5 = model == "claude-sonnet-5-5" || model.starts_with("claude-sonnet-5-5-");
    if sonnet_5_5 {
        // Sonnet 5.5 默认自适应思考，不接受旧版 enabled / disabled 或 token 预算。
        // 关闭开关只关闭前置思考；工具间的思考块仍须原样回传。
        match config.enable_thinking {
            Some(true) => body["thinking"] = json!({ "type": "adaptive" }),
            Some(false) => body["thinking"] = json!({ "type": "between_tools" }),
            None => {}
        }
        // 新模型拒绝非默认采样值，交由服务端使用模型默认值。
        if let Some(object) = body.as_object_mut() {
            for field in ["temperature", "top_p", "top_k"] {
                object.remove(field);
            }
        }
    } else if !tools_present {
        // 保持旧模型的工具请求行为，单轮请求仍使用原有显式思考预算。
        match config.enable_thinking {
            Some(true) => {
                let budget = config
                    .thinking_budget
                    .filter(|value| *value >= 1024)
                    .unwrap_or(1024);
                if max_tokens <= budget {
                    max_tokens = budget.saturating_add(1);
                }
                body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
            }
            Some(false) => {
                body["thinking"] = json!({ "type": "disabled" });
            }
            None => {}
        }
    }
    body["max_tokens"] = json!(max_tokens);
}

pub fn apply_gemini(generation_config: &mut Map<String, Value>, config: &ModelConfig) {
    generation_config.insert(
        "maxOutputTokens".to_string(),
        json!(resolved_max_tokens(config, 8192)),
    );
    match config.enable_thinking {
        Some(true) => {
            let budget = config
                .thinking_budget
                .filter(|value| *value > 0)
                .map(i64::from)
                .unwrap_or(-1);
            generation_config.insert(
                "thinkingConfig".to_string(),
                json!({ "thinkingBudget": budget }),
            );
        }
        Some(false) => {
            generation_config.insert("thinkingConfig".to_string(), json!({ "thinkingBudget": 0 }));
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiProvider, ModelConfig};

    fn model(provider: AiProvider) -> ModelConfig {
        ModelConfig {
            provider,
            endpoint: "https://example.com".to_string(),
            api_key: None,
            model: "demo".to_string(),
            enable_thinking: Some(true),
            thinking_budget: Some(2048),
            max_output_tokens: Some(4096),
        }
    }

    #[test]
    fn openai兼容默认只发max_tokens不广播chat_template_kwargs() {
        let mut body = json!({ "model": "gpt" });
        apply_openai_compatible(&mut body, &model(AiProvider::OpenAI), true);
        assert_eq!(body["max_tokens"], 4096);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.get("chat_template_kwargs").is_none());
        assert!(body.get("enable_thinking").is_none());
    }

    #[test]
    fn openai新一代推理模型应改用max_completion_tokens并移除temperature() {
        for model_name in ["gpt-6.2-sol", "gpt-6-luna", "gpt-5.4", "o3", "o4-mini"] {
            let mut cfg = model(AiProvider::OpenAI);
            cfg.model = model_name.to_string();
            let mut body = json!({ "model": model_name, "temperature": 0.2, "top_p": 0.9 });
            apply_openai_compatible(&mut body, &cfg, true);
            assert_eq!(
                body["max_completion_tokens"], 4096,
                "{model_name} 应写入 max_completion_tokens"
            );
            assert!(
                body.get("max_tokens").is_none(),
                "{model_name} 不应保留旧版 max_tokens"
            );
            assert!(
                body.get("temperature").is_none(),
                "{model_name} 不应发送非默认 temperature"
            );
            assert!(
                body.get("top_p").is_none(),
                "{model_name} 不应发送非默认 top_p"
            );
        }

        // 第三方 OpenAI 兼容服务（如 OpenRouter / DeepSeek / Qwen / Custom）保持 max_tokens 与 temperature
        let mut third_party = model(AiProvider::OpenRouter);
        third_party.model = "openai/gpt-6.2-sol".to_string();
        let mut body = json!({ "model": third_party.model, "temperature": 0.2 });
        apply_openai_compatible(&mut body, &third_party, true);
        assert_eq!(body["max_tokens"], 4096);
        assert!(body.get("max_completion_tokens").is_none());
        assert_eq!(body["temperature"], 0.2);
    }

    #[test]
    fn openai推理参数匹配应覆盖边界前缀并拒绝非目标模型() {
        for positive in [
            "gpt-5",
            "gpt-5.4",
            "gpt-5-mini",
            " gpt-6.2-sol ",
            "openai/gpt-5",
            "o1",
            "o1-mini",
            "o1-2024-12-17",
            "o3-mini-high",
            "o4-mini",
        ] {
            assert!(
                uses_openai_reasoning_chat_params(AiProvider::OpenAI, positive),
                "{positive} 应识别为 OpenAI 推理系参数模式"
            );
        }

        for negative in ["gpt-4o", "gpt-4.1", "gpt-50", "gpt-600", "o2", "o10", ""] {
            assert!(
                !uses_openai_reasoning_chat_params(AiProvider::OpenAI, negative),
                "{negative} 不应误判为 OpenAI 推理系参数模式"
            );
        }

        assert!(!uses_openai_reasoning_chat_params(
            AiProvider::Custom,
            "gpt-5"
        ));
        assert!(!uses_openai_reasoning_chat_params(
            AiProvider::OpenRouter,
            "openai/gpt-6.2-sol"
        ));
    }

    #[test]
    fn siliconflow应写入chat_template_kwargs() {
        let mut body = json!({});
        apply_openai_compatible(&mut body, &model(AiProvider::SiliconFlow), true);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
    }

    #[test]
    fn qwen非流式开启思考时不应发送enable_thinking() {
        let mut body = json!({});
        apply_openai_compatible(&mut body, &model(AiProvider::Qwen), false);
        assert!(body.get("enable_thinking").is_none());
        apply_openai_compatible(&mut body, &model(AiProvider::Qwen), true);
        assert_eq!(body["enable_thinking"], true);
        assert_eq!(body["thinking_budget"], 2048);
    }

    #[test]
    fn ollama应使用think与num_predict() {
        let mut body = json!({});
        apply_ollama(&mut body, &model(AiProvider::Ollama));
        assert_eq!(body["think"], true);
        assert_eq!(body["options"]["num_predict"], 4096);
    }

    #[test]
    fn claude开启思考时max_tokens必须大于budget() {
        let mut config = model(AiProvider::Claude);
        config.max_output_tokens = Some(1000);
        config.thinking_budget = Some(1024);
        let mut body = json!({});
        apply_claude(&mut body, &config, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body["max_tokens"].as_u64().unwrap() > 1024);
        let mut with_tools = json!({});
        apply_claude(&mut with_tools, &config, true);
        assert!(with_tools.get("thinking").is_none());
    }

    #[test]
    fn claude_sonnet55应按开关映射自适应与工具间思考() {
        let mut config = model(AiProvider::Claude);
        config.model = "claude-sonnet-5-5".to_string();
        config.max_output_tokens = Some(1000);
        for tools_present in [false, true] {
            for (enabled, expected) in [(true, "adaptive"), (false, "between_tools")] {
                config.enable_thinking = Some(enabled);
                let mut body = json!({"temperature": 0.2, "top_p": 0.9, "top_k": 20});
                apply_claude(&mut body, &config, tools_present);
                assert_eq!(body["thinking"]["type"], expected);
                assert!(body["thinking"].get("budget_tokens").is_none());
                assert_eq!(body["max_tokens"], 1000);
                for field in ["temperature", "top_p", "top_k"] {
                    assert!(body.get(field).is_none());
                }
            }
        }
        config.model = "claude-sonnet-5-5-20260928".to_string();
        config.enable_thinking = None;
        let mut body = json!({});
        apply_claude(&mut body, &config, true);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn claude旧模型仍使用原有思考格式() {
        let mut config = model(AiProvider::Claude);
        config.model = "claude-sonnet-4-5".to_string();
        config.enable_thinking = Some(false);
        let mut body = json!({"temperature": 0.2});
        apply_claude(&mut body, &config, false);
        assert_eq!(body["thinking"]["type"], "disabled");
        assert_eq!(body["temperature"], 0.2);
    }

    #[test]
    fn gemini显式开启且无预算时应使用动态负一() {
        let mut config = model(AiProvider::Gemini);
        config.thinking_budget = None;
        let mut gen = Map::new();
        apply_gemini(&mut gen, &config);
        assert_eq!(gen["thinkingConfig"]["thinkingBudget"], -1);
        config.enable_thinking = Some(false);
        let mut off = Map::new();
        apply_gemini(&mut off, &config);
        assert_eq!(off["thinkingConfig"]["thinkingBudget"], 0);
    }

    #[test]
    fn 能力映射应对不支持思考的提供商关闭控件() {
        let openai = capabilities_for(AiProvider::OpenAI);
        assert_eq!(openai.thinking, ThinkingProtocol::None);
        assert!(openai.max_output_tokens);
        assert!(capabilities_for(AiProvider::Qwen).thinking_streaming_only);
    }
}
