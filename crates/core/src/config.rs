use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

fn default_true() -> bool {
    true
}

fn default_locale() -> String {
    "zh-CN".to_string()
}

/// AI 提供商类型
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AiProvider {
    /// 本地 Ollama
    #[default]
    Ollama,
    /// OpenAI / OpenAI Compatible
    OpenAI,
    /// Google Gemini
    Gemini,
    /// Anthropic Claude
    Claude,
    /// 硅基流动 SiliconFlow
    #[serde(rename = "siliconflow")]
    SiliconFlow,
    /// DeepSeek
    #[serde(rename = "deepseek")]
    DeepSeek,
    /// 通义千问 Qwen
    #[serde(rename = "qwen")]
    Qwen,
    /// 智谱 ChatGLM
    #[serde(rename = "zhipu")]
    Zhipu,
    /// 月之暗面 Moonshot
    #[serde(rename = "moonshot")]
    Moonshot,
    /// 火山引擎 豆包
    #[serde(rename = "doubao")]
    Doubao,
    /// 稀宇科技 MiniMax
    #[serde(rename = "minimax")]
    MiniMax,
    /// OpenRouter（多模型聚合）
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// Groq（高速推理）
    #[serde(rename = "groq")]
    Groq,
    /// xAI Grok
    #[serde(rename = "xai")]
    XAI,
    /// Mistral AI
    #[serde(rename = "mistral")]
    Mistral,
    /// LM Studio（本地）
    #[serde(rename = "lmstudio")]
    LmStudio,
    /// 自定义 OpenAI 兼容接口
    #[serde(rename = "custom")]
    Custom,
}

impl AiProvider {
    /// 获取提供商的显示名称（仅用于新建 profile 时的默认命名，见 `default_profile_name`）。
    ///
    /// 运行时显示由前端 `providerDisplayNames`（Ask.svelte）按 locale 提供，
    /// 这里只有中文一种，已不再作为运行时显示来源。
    #[deprecated(
        note = "运行时显示请用前端 providerDisplayNames；此处仅保留用于 default_profile_name 的初始命名"
    )]
    pub fn display_name(&self) -> &'static str {
        match self {
            AiProvider::Ollama => "Ollama (本地)",
            AiProvider::OpenAI => "OpenAI / 兼容API",
            AiProvider::Gemini => "Google Gemini",
            AiProvider::Claude => "Anthropic Claude",
            AiProvider::SiliconFlow => "硅基流动 SiliconFlow",
            AiProvider::DeepSeek => "DeepSeek",
            AiProvider::Qwen => "通义千问 Qwen",
            AiProvider::Zhipu => "智谱 ChatGLM",
            AiProvider::Moonshot => "月之暗面 Kimi",
            AiProvider::Doubao => "火山引擎 豆包",
            AiProvider::MiniMax => "稀宇科技 MiniMax",
            AiProvider::OpenRouter => "OpenRouter",
            AiProvider::Groq => "Groq",
            AiProvider::XAI => "xAI Grok",
            AiProvider::Mistral => "Mistral",
            AiProvider::LmStudio => "LM Studio (本地)",
            AiProvider::Custom => "自定义接口",
        }
    }

    /// 获取默认的 API 地址
    pub fn default_endpoint(&self) -> &'static str {
        match self {
            AiProvider::Ollama => "http://localhost:11434",
            AiProvider::OpenAI => "https://api.openai.com/v1",
            AiProvider::Gemini => "https://generativelanguage.googleapis.com/v1",
            AiProvider::Claude => "https://api.anthropic.com/v1",
            AiProvider::SiliconFlow => "https://api.siliconflow.cn/v1",
            AiProvider::DeepSeek => "https://api.deepseek.com",
            AiProvider::Qwen => "https://dashscope.aliyuncs.com/compatible-mode/v1",
            AiProvider::Zhipu => "https://open.bigmodel.cn/api/paas/v4",
            AiProvider::Moonshot => "https://api.moonshot.cn/v1",
            AiProvider::Doubao => "https://ark.cn-beijing.volces.com/api/v3",
            AiProvider::MiniMax => "https://api.minimaxi.com/v1",
            AiProvider::OpenRouter => "https://openrouter.ai/api/v1",
            AiProvider::Groq => "https://api.groq.com/openai/v1",
            AiProvider::XAI => "https://api.x.ai/v1",
            AiProvider::Mistral => "https://api.mistral.ai/v1",
            AiProvider::LmStudio => "http://localhost:1234/v1",
            AiProvider::Custom => "",
        }
    }

    /// 获取默认模型名称
    pub fn default_model(&self) -> &'static str {
        match self {
            AiProvider::Ollama => "qwen3",
            AiProvider::OpenAI => "gpt-6.2-sol",
            AiProvider::Gemini => "gemini-4-hydrogen",
            AiProvider::Claude => "claude-sonnet-5-5",
            AiProvider::SiliconFlow => "Qwen/Qwen3-8B",
            AiProvider::DeepSeek => "deepseek-flash",
            AiProvider::Qwen => "qwen3.8-flash",
            AiProvider::Zhipu => "glm-5.3-flash",
            AiProvider::Moonshot => "kimi-k3",
            AiProvider::Doubao => "doubao-seed-evolving",
            AiProvider::MiniMax => "MiniMax-M3.1-Flash",
            AiProvider::OpenRouter => "openrouter/auto",
            AiProvider::Groq => "openai/gpt-oss-120b",
            AiProvider::XAI => "grok-4.7",
            AiProvider::Mistral => "mistral-small-latest",
            AiProvider::LmStudio => "local-model",
            AiProvider::Custom => "",
        }
    }

    /// 是否使用 OpenAI 兼容格式
    pub fn is_openai_compatible(&self) -> bool {
        matches!(
            self,
            AiProvider::OpenAI
                | AiProvider::SiliconFlow
                | AiProvider::DeepSeek
                | AiProvider::Qwen
                | AiProvider::Zhipu
                | AiProvider::Moonshot
                | AiProvider::Doubao
                | AiProvider::MiniMax
                | AiProvider::OpenRouter
                | AiProvider::Groq
                | AiProvider::XAI
                | AiProvider::Mistral
                | AiProvider::LmStudio
                | AiProvider::Custom
        )
    }

    /// 返回与 JSON 序列化一致的小写服务商标识
    pub fn as_str(&self) -> &'static str {
        match self {
            AiProvider::Ollama => "ollama",
            AiProvider::OpenAI => "openai",
            AiProvider::Gemini => "gemini",
            AiProvider::Claude => "claude",
            AiProvider::SiliconFlow => "siliconflow",
            AiProvider::DeepSeek => "deepseek",
            AiProvider::Qwen => "qwen",
            AiProvider::Zhipu => "zhipu",
            AiProvider::Moonshot => "moonshot",
            AiProvider::Doubao => "doubao",
            AiProvider::MiniMax => "minimax",
            AiProvider::OpenRouter => "openrouter",
            AiProvider::Groq => "groq",
            AiProvider::XAI => "xai",
            AiProvider::Mistral => "mistral",
            AiProvider::LmStudio => "lmstudio",
            AiProvider::Custom => "custom",
        }
    }

    /// 是否通常需要配置 API Key（本地 Ollama / LM Studio 与自定义接口允许为空）
    pub fn requires_api_key(&self) -> bool {
        !matches!(
            self,
            AiProvider::Ollama | AiProvider::LmStudio | AiProvider::Custom
        )
    }
}

/// AI分析模式
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AiMode {
    /// 本地多模态模型（分析截图）
    #[default]
    Local,
    /// 摘要模式（只上传统计摘要）
    Summary,
    /// 云端视觉模型（上传截图到云端分析）
    Cloud,
}

/// 单个模型配置（独立的提供商、地址、密钥、模型名）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModelConfig {
    /// 提供商类型
    pub provider: AiProvider,
    /// API 地址
    pub endpoint: String,
    /// API Key
    pub api_key: Option<String>,
    /// 模型名称
    pub model: String,
    /// 思考模式：None=跟随服务端，Some(true/false)=显式开关
    #[serde(default)]
    pub enable_thinking: Option<bool>,
    /// 思考 token 预算；0 或空表示不发送
    #[serde(default)]
    pub thinking_budget: Option<u32>,
    /// 最大输出 token；0 或空表示使用链路默认值
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

impl ModelConfig {
    /// 创建默认的文本模型配置
    /// 注意：model 默认为空，强制用户手动配置，避免 UI 误显示"已配置"
    pub fn default_text() -> Self {
        Self {
            provider: AiProvider::Ollama,
            endpoint: AiProvider::Ollama.default_endpoint().to_string(),
            api_key: None,
            model: String::new(), // 默认为空，用户需手动填写
            enable_thinking: None,
            thinking_budget: None,
            max_output_tokens: None,
        }
    }

    /// 创建默认的视觉模型配置
    pub fn default_vision() -> Self {
        Self {
            provider: AiProvider::Ollama,
            endpoint: AiProvider::Ollama.default_endpoint().to_string(),
            api_key: None,
            model: "llava".to_string(),
            enable_thinking: None,
            thinking_budget: None,
            max_output_tokens: None,
        }
    }
}

/// 可保存的文本模型档案
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TextModelProfile {
    /// 档案唯一标识
    pub id: String,
    /// 档案显示名称
    pub name: String,
    /// 对应模型配置
    pub model_config: ModelConfig,
    /// 最近一次测试状态：untested / success / error
    #[serde(default = "default_connection_status")]
    pub test_status: String,
    /// 最近一次测试时间（毫秒时间戳）
    #[serde(default)]
    pub last_tested_at: Option<u64>,
    /// 最近一次测试结果描述
    #[serde(default)]
    pub last_test_message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvatarFollowupItem {
    pub id: String,
    pub title: String,
    pub date: String,
    pub source_app: String,
    pub source_title: String,
    pub project_key: String,
    pub created_at: i64,
    #[serde(default = "default_avatar_followup_status")]
    pub status: String,
}

fn default_avatar_followup_status() -> String {
    "open".to_string()
}

/// AI 提供商配置（旧版，保留用于向后兼容）
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AiProviderConfig {
    /// 提供商类型
    pub provider: AiProvider,
    /// API 地址（可自定义，支持代理）
    pub endpoint: String,
    /// API Key
    pub api_key: Option<String>,
    /// 模型名称
    pub model: String,
    /// 视觉模型名称（用于分析截图）
    pub vision_model: Option<String>,
}

impl Default for AiProviderConfig {
    fn default() -> Self {
        Self {
            provider: AiProvider::Ollama,
            endpoint: AiProvider::Ollama.default_endpoint().to_string(),
            api_key: None,
            // 旧配置的缺省值不能在迁移时冒充用户配置的模型。
            model: String::new(),
            vision_model: Some("llava".to_string()),
        }
    }
}

/// 应用隐私级别
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PrivacyLevel {
    /// 完全记录（截图 + 统计）
    #[default]
    Full,
    /// 内容脱敏（只统计时长，不保存截图）
    Anonymized,
    /// 完全忽略（不记录任何信息）
    Ignored,
}

/// 应用隐私规则
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppPrivacyRule {
    /// 应用名称
    pub app_name: String,
    /// 隐私级别（默认为 Full，兼容旧配置）
    #[serde(default)]
    pub level: PrivacyLevel,
}

/// 应用分类规则
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppCategoryRule {
    /// 应用名称
    pub app_name: String,
    /// 目标分类
    pub category: String,
}

/// 用户自定义分类
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CustomCategory {
    /// 唯一标识（slug 格式，如 "project-mgmt"）
    pub key: String,
    /// 显示名称（用户输入，如 "项目管理"）
    pub name: String,
    /// 颜色（hex 格式，如 "#8B5CF6"）
    pub color: String,
    /// 图标（emoji，如 "📋"）
    pub icon: String,
}

/// 用户自定义语义分类
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CustomSemanticCategory {
    /// 唯一标识（slug 格式，如 "project-mgmt"）
    pub key: String,
    /// 显示名称（用户输入，如 "项目管理"）
    pub name: String,
}

/// 日报提示词预设模板
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PromptPreset {
    /// 预设名称
    pub name: String,
    /// 提示词内容
    pub prompt: String,
}

/// 网站语义分类规则
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WebsiteSemanticRule {
    /// 域名
    pub domain: String,
    /// 目标语义分类
    pub semantic_category: String,
}

/// 浏览器 URL 无法识别时的默认隐私降级策略
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnknownUrlFallback {
    Record,
    Anonymize,
}

/// 隐私配置
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PrivacyConfig {
    /// 应用隐私规则列表
    #[serde(default)]
    pub app_rules: Vec<AppPrivacyRule>,
    /// 排除的窗口标题关键词（触发时使用 Anonymized 级别）
    #[serde(default)]
    pub excluded_keywords: Vec<String>,
    /// URL 域名黑名单（匹配时完全忽略，不记录）
    #[serde(default)]
    pub excluded_domains: Vec<String>,
    /// 已弃用：敏感词过滤始终启用，此字段仅保留反序列化兼容
    #[serde(default = "default_true")]
    pub filter_sensitive: bool,
    /// 自动跳过浏览器无痕 / 隐私窗口（默认开启）
    #[serde(default = "default_true")]
    pub skip_private_windows: bool,
    /// 浏览器 URL 未知时的降级策略：None 表示配置了排除域名时自动 Anonymize，否则 Record
    #[serde(default)]
    pub unknown_url_fallback: Option<UnknownUrlFallback>,

    // 兼容旧版
    #[serde(default)]
    pub excluded_apps: Vec<String>,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            app_rules: vec![
                AppPrivacyRule {
                    app_name: "1Password".to_string(),
                    level: PrivacyLevel::Ignored,
                },
                AppPrivacyRule {
                    app_name: "Bitwarden".to_string(),
                    level: PrivacyLevel::Ignored,
                },
                AppPrivacyRule {
                    app_name: "Keychain".to_string(),
                    level: PrivacyLevel::Ignored,
                },
            ],
            excluded_keywords: vec![
                "bank".to_string(),
                "login".to_string(),
                "password".to_string(),
                "密码".to_string(),
                "银行".to_string(),
                "支付".to_string(),
            ],
            excluded_domains: vec![], // 默认无域名黑名单
            filter_sensitive: true,   // 已弃用，保留兼容
            skip_private_windows: true,
            unknown_url_fallback: None,
            excluded_apps: vec![],
        }
    }
}

fn normalized_app_rule_key(app_name: &str) -> String {
    crate::categorize::normalize_display_app_name(app_name.trim())
        .trim()
        .to_lowercase()
}

impl PrivacyConfig {
    /// 获取应用的隐私级别（命中多条规则或遗留 excluded_apps 时取最严级别：忽略 > 脱敏 > 完整记录）
    pub fn get_app_privacy_level(&self, app_name: &str) -> PrivacyLevel {
        let normalized = normalized_app_rule_key(app_name);
        if normalized.is_empty() {
            return PrivacyLevel::Full;
        }
        let mut strictest = PrivacyLevel::Full;
        // 检查新规则（规范化后精确匹配 + 单向包含匹配），取最严级别避免先匹配遮蔽后置收紧规则
        for rule in &self.app_rules {
            let rule_normalized = normalized_app_rule_key(&rule.app_name);
            if rule_normalized.is_empty() {
                continue;
            }
            if normalized == rule_normalized || normalized.contains(&rule_normalized) {
                log::debug!(
                    "应用 {} 匹配规则 {}, 级别: {:?}",
                    app_name,
                    rule.app_name,
                    rule.level
                );
                if privacy_level_rank(rule.level) > privacy_level_rank(strictest) {
                    strictest = rule.level;
                }
            }
        }
        // 兼容旧版 excluded_apps（视为 Ignored，不可被 app_rules 中的宽松规则降级）
        for excluded in &self.excluded_apps {
            let excluded_normalized = normalized_app_rule_key(excluded);
            if !excluded_normalized.is_empty()
                && (normalized == excluded_normalized || normalized.contains(&excluded_normalized))
            {
                return PrivacyLevel::Ignored;
            }
        }
        strictest
    }

    /// 检查窗口标题是否触发隐私保护
    pub fn should_anonymize_by_keyword(&self, window_title: &str) -> bool {
        let title_lower = window_title.to_lowercase();
        self.excluded_keywords.iter().any(|k| {
            let k = k.trim();
            !k.is_empty() && title_lower.contains(&k.to_lowercase())
        })
    }

    /// 从完整 URL 或裸域名（可带端口）提取小写主机名，保留 www 和子域。
    /// 无法解析或没有主机时返回空字符串，供调用者跳过匹配或分类。
    pub fn extract_domain(url: &str) -> String {
        let url = url.trim();
        let has_scheme = url
            .split_once("://")
            .is_some_and(|(scheme, _)| !scheme.contains(['/', '?', '#']));
        let parsed = if has_scheme {
            reqwest::Url::parse(url)
        } else {
            // 裸域名不接受相对路径或用户信息，避免被补全为另一个主机。
            let authority = url.split(['/', '?', '#']).next().unwrap_or("");
            if url.starts_with(['/', '\\']) || authority.contains('@') {
                return String::new();
            }
            reqwest::Url::parse(&format!("https://{url}"))
        };
        parsed
            .ok()
            .and_then(|url| {
                url.host_str()
                    .map(|host| host.trim_end_matches('.').to_lowercase())
            })
            .unwrap_or_default()
    }

    /// 后缀匹配域名：domain == pattern 或 domain 以 .pattern 结尾
    pub fn domain_matches(domain: &str, pattern: &str) -> bool {
        if domain == pattern {
            return true;
        }
        domain.ends_with(&format!(".{pattern}"))
    }

    /// 迁移旧版 excluded_apps 到 app_rules
    pub fn migrate_legacy_excluded_apps(&mut self) -> bool {
        if self.excluded_apps.is_empty() {
            return false;
        }
        let existing_names: std::collections::HashSet<String> = self
            .app_rules
            .iter()
            .map(|r| r.app_name.to_lowercase())
            .collect();
        let mut migrated = false;
        for app in self.excluded_apps.drain(..) {
            if !existing_names.contains(&app.to_lowercase()) {
                self.app_rules.push(AppPrivacyRule {
                    app_name: app,
                    level: PrivacyLevel::Ignored,
                });
                migrated = true;
            }
        }
        migrated
    }

    /// 收集所有应忽略的应用名（与匹配侧一致做显示名归一化 + 小写去空），包含 app_rules 和遗留 excluded_apps
    pub fn collect_ignored_app_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for rule in self
            .app_rules
            .iter()
            .filter(|rule| rule.level == PrivacyLevel::Ignored)
        {
            let normalized = normalized_app_rule_key(&rule.app_name);
            if !normalized.is_empty() && !names.contains(&normalized) {
                names.push(normalized);
            }
        }
        for app in &self.excluded_apps {
            let normalized = normalized_app_rule_key(app);
            if !normalized.is_empty() && !names.contains(&normalized) {
                names.push(normalized);
            }
        }
        names
    }

    /// 清理无效的空白 / 归一化后为空的隐私条目（如 `.exe`），避免通配误伤全部应用。
    pub fn normalize(&mut self) {
        self.app_rules.retain_mut(|rule| {
            let trimmed = rule.app_name.trim().to_string();
            rule.app_name = trimmed;
            !normalized_app_rule_key(&rule.app_name).is_empty()
        });
        self.excluded_apps.retain_mut(|app| {
            *app = app.trim().to_string();
            !normalized_app_rule_key(app).is_empty()
        });
        self.excluded_keywords.retain_mut(|kw| {
            *kw = kw.trim().to_string();
            !kw.is_empty()
        });
        self.excluded_domains.retain_mut(|domain| {
            *domain = domain.trim().to_string();
            !domain.is_empty()
        });
    }

    /// 收集所有域名黑名单（已提取域名、已去空）
    pub fn collect_excluded_domains(&self) -> Vec<String> {
        self.excluded_domains
            .iter()
            .map(|d| Self::extract_domain(d))
            .filter(|d| !d.is_empty())
            .collect()
    }

    /// 浏览器 URL 无法识别时的实际降级策略：
    /// 显式配置了值时尊重用户选择；未显式配置时，若存在排除域名则默认 `Anonymize`，否则 `Record`。
    pub fn effective_unknown_url_fallback(&self) -> UnknownUrlFallback {
        self.unknown_url_fallback.unwrap_or_else(|| {
            if self.collect_excluded_domains().is_empty() {
                UnknownUrlFallback::Record
            } else {
                UnknownUrlFallback::Anonymize
            }
        })
    }
}

/// 存储配置
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ScreenshotDisplayMode {
    /// 仅截活动窗口所在屏幕
    #[default]
    ActiveWindow,
    /// 截取所有屏幕
    All,
}

/// 截图宽度模式
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ScreenshotWidthMode {
    /// 自适应：根据屏幕分辨率动态计算（取屏幕宽度的 70%）
    #[default]
    Auto,
    /// 固定值：使用 max_image_width 设定的值
    Fixed,
}

/// 存储配置
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StorageConfig {
    /// 截图保留天数（超过后删除截图文件）
    pub screenshot_retention_days: u32,
    /// 元数据保留天数，0 表示永久保留；仅在显式开启自动清理后生效。
    pub metadata_retention_days: u32,
    /// 旧版本未执行元数据清理，升级后默认关闭，避免按旧默认天数误删历史。
    #[serde(default)]
    pub metadata_cleanup_enabled: bool,
    /// 存储空间上限（MB），超过后自动清理最旧的数据
    pub storage_limit_mb: u32,
    /// JPEG 质量 (1-100)
    pub jpeg_quality: u8,
    /// 最大图片宽度（超过会缩放）
    pub max_image_width: u32,
    /// 是否启用截图与 OCR
    #[serde(default = "default_screenshots_enabled")]
    pub screenshots_enabled: bool,
    /// 截图屏幕范围
    #[serde(default)]
    pub screenshot_display_mode: ScreenshotDisplayMode,
    /// 截图宽度模式：auto 自适应屏幕，fixed 使用固定值
    #[serde(default)]
    pub screenshot_width_mode: ScreenshotWidthMode,
}

fn default_screenshots_enabled() -> bool {
    true
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            screenshot_retention_days: 7, // 默认保留7天截图
            metadata_retention_days: 30,  // 启用自动清理后的默认保留天数
            metadata_cleanup_enabled: false,
            storage_limit_mb: 2048, // 默认2GB上限
            jpeg_quality: 85,       // 85%质量，更清晰
            max_image_width: 1280,  // 最大宽度1280px
            screenshots_enabled: true,
            screenshot_display_mode: ScreenshotDisplayMode::ActiveWindow,
            screenshot_width_mode: ScreenshotWidthMode::Auto,
        }
    }
}

/// 远程存储提供商
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RemoteStorageProvider {
    #[default]
    None,
    S3,
    #[serde(rename = "webdav")]
    WebDav,
}

/// S3 兼容存储（MinIO）配置
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct S3Config {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key: String,
    #[serde(default)]
    pub secret_key: String,
    #[serde(default = "default_s3_region")]
    pub region: String,
    #[serde(default)]
    pub path_prefix: String,
    #[serde(default)]
    pub public_url_base: Option<String>,
}

fn default_s3_region() -> String {
    "us-east-1".to_string()
}

/// WebDAV 存储配置
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct WebDavConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub path_prefix: String,
    #[serde(default)]
    pub public_url_base: Option<String>,
}

/// 远程存储配置
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct RemoteStorageConfig {
    #[serde(default)]
    pub provider: RemoteStorageProvider,
    #[serde(default)]
    pub s3: S3Config,
    #[serde(default)]
    pub webdav: WebDavConfig,
    /// 将常用应用配置同步到 WebDAV（最后写入获胜）。本机端口、窗口位置等不覆盖。
    #[serde(default)]
    pub sync_app_config: bool,
}

pub const DEFAULT_LOCALHOST_API_PORT: u16 = 47_831;

fn default_localhost_api_port() -> u16 {
    DEFAULT_LOCALHOST_API_PORT
}

fn normalize_localhost_api_port(port: u16) -> u16 {
    if port == 0 {
        DEFAULT_LOCALHOST_API_PORT
    } else {
        port
    }
}

/// 配置加载状态，用于区分首次启动、正常加载和故障恢复。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigLoadStatus {
    Loaded,
    Missing,
    RecoveredFromBackup,
    Corrupted,
}

impl ConfigLoadStatus {
    /// 当前加载状态是否允许启动流程自动写回配置。
    pub fn allows_automatic_save(self) -> bool {
        matches!(self, Self::Loaded | Self::Missing)
    }

    /// 当前加载状态是否必须以故障安全模式启动。
    pub fn requires_fail_safe(self) -> bool {
        matches!(self, Self::Corrupted)
    }
}

/// 返回在完整配置文件名后追加 `.bak` 的备份路径。
pub fn config_backup_path(path: &Path) -> PathBuf {
    let mut backup_name = path.as_os_str().to_os_string();
    backup_name.push(".bak");
    PathBuf::from(backup_name)
}

fn config_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn unique_config_temp_path(target: &Path) -> io::Result<PathBuf> {
    let file_name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "配置路径必须包含文件名"))?;
    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(".tmp-");
    temp_name.push(uuid::Uuid::new_v4().simple().to_string());
    Ok(config_parent(target).join(temp_name))
}

fn open_secure_temp_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

struct PendingConfigFile {
    path: PathBuf,
    pending: bool,
}

impl PendingConfigFile {
    fn create(target: &Path) -> io::Result<(Self, File)> {
        let path = unique_config_temp_path(target)?;
        let file = open_secure_temp_file(&path)?;
        Ok((
            Self {
                path,
                pending: true,
            },
            file,
        ))
    }

    fn replace(&mut self, target: &Path) -> io::Result<()> {
        atomic_replace(&self.path, target)?;
        self.pending = false;
        Ok(())
    }
}

impl Drop for PendingConfigFile {
    fn drop(&mut self) {
        if self.pending {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn update_config_backup_with_sync<F>(primary_path: &Path, sync_parent: F) -> io::Result<()>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    let mut primary_file = match File::open(primary_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let backup_path = config_backup_path(primary_path);
    let (mut pending_backup, mut backup_file) = PendingConfigFile::create(&backup_path)?;
    io::copy(&mut primary_file, &mut backup_file)?;
    backup_file.flush()?;
    backup_file.sync_all()?;
    drop(backup_file);
    commit_pending_config(
        &mut pending_backup,
        &backup_path,
        config_parent(primary_path),
        sync_parent,
    )
}

fn update_config_backup(primary_path: &Path) -> io::Result<()> {
    update_config_backup_with_sync(primary_path, sync_parent_directory)
}

#[cfg(unix)]
pub fn atomic_replace(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(windows)]
pub fn atomic_replace(source: &Path, target: &Path) -> io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let target_wide = target
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            target_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
pub fn atomic_replace(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(unix)]
pub fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
pub fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    // 非 Unix 平台无通用目录句柄 fsync 接口（Windows 同卷重命名依赖 NTFS 日志）。
    Ok(())
}

fn commit_pending_config<F>(
    pending_config: &mut PendingConfigFile,
    path: &Path,
    parent: &Path,
    sync_parent: F,
) -> io::Result<()>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    pending_config.replace(path)?;
    if let Err(error) = sync_parent(parent) {
        // 原子替换是提交点；此后只能报告持久化告警，不能伪装成“保存未发生”。
        log::warn!(
            "配置已提交，但同步父目录 {} 失败: {error}",
            parent.display()
        );
    }
    Ok(())
}

/// 飞书 / 钉钉 / 企业微信 Bot 的发送者绑定：只有绑定过的发送者才能查询本机数据。
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct BotSenderBinding {
    /// 已授权的发送者 ID（飞书 open_id、钉钉 staffId、企业微信 userid）
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    /// 一次性绑定码（在设置页生成，发送 `/bind 绑定码` 完成绑定）
    #[serde(default)]
    pub bind_code: Option<String>,
    /// 绑定码过期时间（Unix 秒）
    #[serde(default)]
    pub bind_code_expires_at: Option<i64>,
    /// 当前绑定码的错误尝试次数，达到上限后作废
    #[serde(default)]
    pub failed_attempts: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NodeGatewayConfig {
    #[serde(default)]
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDevice {
    pub name: String,
    pub url: String,
    pub token: String,
}

/// 工作时间段
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WorkTimeSegment {
    pub start_hour: u8,
    #[serde(default)]
    pub start_minute: u8,
    pub end_hour: u8,
    #[serde(default)]
    pub end_minute: u8,
}

/// AI 按场景分流的任务类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTaskKind {
    /// 每日工作日报生成
    Report,
    /// 工作助手对话问答
    Assistant,
    /// 应用与网站自动语义分类
    Classification,
    /// 桌宠主动 AI 提醒
    Proactive,
}

/// 按任务场景指定使用的模型服务商/档案（留空表示跟随默认主模型 `text_model`）
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskModelRouting {
    #[serde(default)]
    pub report_provider: Option<String>,
    #[serde(default)]
    pub assistant_provider: Option<String>,
    #[serde(default)]
    pub classification_provider: Option<String>,
    #[serde(default)]
    pub proactive_provider: Option<String>,
}

/// 应用配置
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    /// 截屏间隔（秒）
    pub screenshot_interval: u64,
    /// 界面语言（zh-CN / en / zh-TW），用于后端托盘菜单等本地化
    #[serde(default = "default_locale")]
    pub locale: String,
    /// 空闲检测阈值（分钟）：超过此时间无键鼠输入则进入疑似空闲
    #[serde(default = "default_idle_threshold_minutes")]
    pub idle_threshold_minutes: u32,
    /// AI分析模式
    pub ai_mode: AiMode,
    /// 文本模型配置
    #[serde(default = "ModelConfig::default_text")]
    pub text_model: ModelConfig,
    /// 助手整次请求绝对超时（秒）
    #[serde(default = "default_assistant_timeout_secs")]
    pub assistant_timeout_secs: u64,
    /// 日报生成外层超时（秒）
    #[serde(default = "default_report_generation_timeout_secs")]
    pub report_generation_timeout_secs: u64,
    /// 可保存的文本模型档案
    #[serde(default)]
    pub text_model_profiles: Vec<TextModelProfile>,
    /// 各服务商的模型配置快照（key 为 provider id；切换服务商时保留各家的地址/模型/Key，随配置一起保存）
    #[serde(default)]
    pub text_model_provider_cache: HashMap<String, ModelConfig>,
    /// 按任务场景路由模型（日报 / 助手 / 自动分类 / 桌宠提醒，均默认跟随主模型）
    #[serde(default)]
    pub task_model_routing: TaskModelRouting,
    /// 视觉模型配置
    #[serde(default = "ModelConfig::default_vision")]
    pub vision_model: ModelConfig,
    /// 隐私配置
    #[serde(default)]
    pub privacy: PrivacyConfig,
    /// 应用分类覆盖规则
    #[serde(default)]
    pub app_category_rules: Vec<AppCategoryRule>,
    /// 用户自定义分类
    #[serde(default)]
    pub custom_categories: Vec<CustomCategory>,
    /// 网站语义分类覆盖规则
    #[serde(default)]
    pub website_semantic_rules: Vec<WebsiteSemanticRule>,
    /// 用户自定义语义分类
    #[serde(default)]
    pub custom_semantic_categories: Vec<CustomSemanticCategory>,
    /// 已删除的内置应用分类 key（防止 seed 复活）
    #[serde(default)]
    pub deleted_default_categories: Vec<String>,
    /// 已删除的内置语义分类 key（防止 seed 复活）
    #[serde(default)]
    pub deleted_default_semantic_categories: Vec<String>,
    /// 项目 / 标签工时归集定义（查询时按顺序匹配，默认不同步以防暴露本地路径）
    #[serde(default)]
    pub projects: Vec<crate::project::ProjectDefinition>,
    /// 存储配置
    #[serde(default)]
    pub storage: StorageConfig,
    /// 远程存储配置（S3/MinIO 或 WebDAV）
    #[serde(default)]
    pub remote_storage: RemoteStorageConfig,
    /// 最近一次成功参与 WebDAV 配置同步的时间戳（毫秒）。0 表示从未同步。
    #[serde(default)]
    pub config_synced_at: u64,
    /// 日报附加提示词
    #[serde(default)]
    pub daily_report_custom_prompt: String,
    /// 日报提示词预设模板列表
    #[serde(default)]
    pub daily_report_prompt_presets: Vec<PromptPreset>,
    /// 日报系统提示词覆盖（有值时替代硬编码的系统提示词）
    #[serde(default)]
    pub daily_report_system_prompt_override: Option<String>,
    /// 日报 Markdown 导出目录
    #[serde(default)]
    pub daily_report_export_dir: Option<String>,
    /// 日报自动生成后是否自动导出 Markdown
    #[serde(default)]
    pub daily_report_auto_export: bool,
    /// 日报导出是否附带 YAML Frontmatter（适配 Obsidian / Logseq）
    #[serde(default)]
    pub daily_report_export_frontmatter: bool,
    /// 日报导出写入的目标标题区块（如 `## Work Review`；为空时覆盖整份文件）
    #[serde(default)]
    pub daily_report_export_section_heading: Option<String>,
    /// 是否在日报中关联本地 Git 仓库当日提交
    #[serde(default)]
    pub git_report_enabled: bool,
    /// 参与日报关联的本地 Git 仓库绝对路径列表（仅本机使用，不同步）
    #[serde(default)]
    pub git_repo_paths: Vec<String>,
    /// 可选 Git 提交作者过滤（姓名或邮箱；留空时自动取仓库 `user.email`/`user.name`）
    #[serde(default)]
    pub git_author_filter: Option<String>,
    /// 日报自动生成时间 (HH:MM)，为空时使用工作结束时间
    #[serde(default)]
    pub daily_report_auto_generate_time: Option<String>,
    /// 日报钉选段落（永远排在最前），值为 StatsBlock 的 block_name
    #[serde(default)]
    pub daily_report_pinned_blocks: Vec<String>,
    /// 日报隐藏段落（不显示），值为 StatsBlock 的 block_name
    #[serde(default)]
    pub daily_report_hidden_blocks: Vec<String>,
    /// 上次 AI 编排的段落顺序（缓存，避免每次生成都调 LLM 排序）
    #[serde(default)]
    pub daily_report_last_ai_order: Vec<String>,
    /// 是否启用本地 localhost API
    #[serde(default)]
    pub localhost_api_enabled: bool,
    /// 本地 localhost API 监听地址
    #[serde(default)]
    pub localhost_api_host: Option<String>,
    /// 本地 localhost API 端口
    #[serde(default = "default_localhost_api_port")]
    pub localhost_api_port: u16,
    /// 是否启用 Telegram Bot
    #[serde(default)]
    pub telegram_bot_enabled: bool,
    /// Telegram Bot Token
    #[serde(default)]
    pub telegram_bot_token: Option<String>,
    /// Telegram Bot 代理地址 (e.g. http://127.0.0.1:7890, socks5://127.0.0.1:1080)
    #[serde(default)]
    pub telegram_bot_proxy: Option<String>,
    /// Telegram Bot 允许的 chat_id 白名单；为空时仅允许 /start 和 /bind
    #[serde(default)]
    pub telegram_bot_allowed_chat_ids: Vec<i64>,
    /// Telegram Bot 一次性绑定码；通过 /bind <code> 授权当前 chat_id 后失效
    #[serde(default)]
    pub telegram_bot_bind_code: Option<String>,
    /// Telegram Bot 一次性绑定码过期时间（Unix 秒）
    #[serde(default)]
    pub telegram_bot_bind_code_expires_at: Option<i64>,
    /// 是否启用飞书 Bot
    #[serde(default)]
    pub feishu_bot_enabled: bool,
    /// 飞书 App ID
    #[serde(default)]
    pub feishu_app_id: Option<String>,
    /// 飞书 App Secret
    #[serde(default)]
    pub feishu_app_secret: Option<String>,
    /// 飞书 Verification Token
    #[serde(default)]
    pub feishu_verification_token: Option<String>,
    /// 飞书 Encrypt Key
    #[serde(default)]
    pub feishu_encrypt_key: Option<String>,
    /// 是否启用企业微信 Bot
    #[serde(default)]
    pub wecom_bot_enabled: bool,
    /// 企业微信 Corp ID
    #[serde(default)]
    pub wecom_corp_id: Option<String>,
    /// 企业微信回调 Token
    #[serde(default)]
    pub wecom_token: Option<String>,
    /// 企业微信 EncodingAESKey
    #[serde(default)]
    pub wecom_encoding_aes_key: Option<String>,
    /// 企业微信智能机器人 Bot ID（长连接）
    #[serde(default)]
    pub wecom_bot_id: Option<String>,
    /// 企业微信智能机器人 Secret（长连接）
    #[serde(default)]
    pub wecom_bot_secret: Option<String>,
    /// 是否启用钉钉 Bot
    #[serde(default)]
    pub dingtalk_bot_enabled: bool,
    /// 钉钉应用 App Secret
    #[serde(default)]
    pub dingtalk_app_secret: Option<String>,
    /// 飞书 / 钉钉 / 企业微信 Bot 发送者绑定（key：feishu / dingtalk / wecom）
    #[serde(default)]
    pub bot_sender_bindings: HashMap<String, BotSenderBinding>,
    /// 是否允许已授权 Bot 发送者直接发送自然语言提问（默认关闭；仅只读工作数据、不执行写工具或联网）
    #[serde(default)]
    pub bot_natural_language_enabled: bool,
    /// 是否启用 MCP Server
    #[serde(default)]
    pub mcp_server_enabled: bool,
    /// MCP 客户端是否可读取 OCR 原文、截图路径与可执行文件路径（默认 false：脱敏输出）
    #[serde(default)]
    pub mcp_include_ocr: bool,
    /// 本地 API 是否允许按 `?include_ocr=true` 返回 OCR 原文（默认 false：一律不返回）
    #[serde(default)]
    pub localhost_api_include_ocr: bool,
    /// 已注册的远程设备列表
    #[serde(default)]
    pub node_devices: Vec<NodeDevice>,
    /// 设备节点 / 控制面配置
    #[serde(default)]
    pub node_gateway: NodeGatewayConfig,
    /// 是否开机自启
    pub auto_start: bool,
    /// 开机自启动时是否静默驻留而不显示主界面
    #[serde(default)]
    pub auto_start_silent: bool,
    /// macOS 录屏权限是否已经主动弹过引导，避免每次启动都重复请求
    #[serde(default)]
    pub macos_screen_capture_permission_prompted: bool,
    /// 上次运行的应用版本，用于检测版本更新后重置权限引导
    #[serde(default)]
    pub last_app_version: Option<String>,
    /// 主题模式: system, light, dark
    pub theme: String,
    /// 界面风格: a=Quiet Pro, b=当前柔和层次, c=Compact Data
    #[serde(default = "default_ui_visual_style")]
    pub ui_visual_style: String,
    /// 日报编辑字体；空字符串使用界面字体，未安装的字体由前端回退。
    #[serde(default)]
    pub report_editor_font: String,
    /// 上班开始时间（0-23）
    #[serde(default = "default_work_start")]
    pub work_start_hour: u8,
    /// 上班结束时间（0-23）
    #[serde(default = "default_work_end")]
    pub work_end_hour: u8,
    /// 上班开始分钟（0-59）
    #[serde(default)]
    pub work_start_minute: u8,
    /// 上班结束分钟（0-59）
    #[serde(default)]
    pub work_end_minute: u8,
    /// 分段工作时间（优先级高于上面的单段时间）
    /// 为空时使用 work_start_hour/work_end_hour 作为一段
    /// 例：[{start_hour:9,start_minute:0,end_hour:12,end_minute:0},{start_hour:13,start_minute:0,end_hour:18,end_minute:0}]
    #[serde(default)]
    pub work_time_segments: Vec<WorkTimeSegment>,
    /// 是否启用工作时间过滤。关闭后所有活动都算"工作时间"，不再区分
    #[serde(default = "default_true")]
    pub work_time_enabled: bool,
    /// 每日标准工时（小时），用于计算加班时长。默认 8.0
    #[serde(default = "default_standard_work_hours")]
    pub standard_work_hours: f64,

    // 兼容旧版配置
    #[serde(default)]
    pub ai_provider: AiProviderConfig,
    #[serde(default)]
    pub ollama_host: String,
    #[serde(default)]
    pub ollama_model: String,
    #[serde(default)]
    pub openai_api_key: Option<String>,
    #[serde(default)]
    pub openai_model: String,
    /// 隐藏 Dock 图标（仅保留菜单栏）
    #[serde(default)]
    pub hide_dock_icon: bool,
    /// 轻量模式：关闭主界面时释放主 Webview，仅保留后台录制与托盘
    #[serde(default)]
    pub lightweight_mode: bool,
    /// 是否启用休息提醒
    #[serde(default)]
    pub break_reminder_enabled: bool,
    /// 连续活跃多久后提醒（分钟）
    #[serde(default = "default_break_reminder_interval_minutes")]
    pub break_reminder_interval_minutes: u64,
    /// 每日工作时长目标（分钟），None = 不设目标
    #[serde(default)]
    pub daily_work_goal_minutes: Option<u32>,
    /// 达到目标时桌宠庆祝提醒
    #[serde(default)]
    pub goal_notifications: bool,
    /// AI 工作记忆：自动合成洞察
    #[serde(default)]
    pub memory_enabled: bool,
    /// 上次记忆合成日期（防同天重复）
    #[serde(default)]
    pub memory_last_synthesis_date: Option<String>,
    /// 助手显式长期记忆开关。默认关闭，关闭不会删除已保存的记忆。
    #[serde(default)]
    pub assistant_memory_enabled: bool,
    /// 助手联网能力总开关（隐私默认关：开启后问题可能发给搜索服务商/目标网站）
    #[serde(default)]
    pub assistant_web_access_enabled: bool,
    /// 搜索服务商："tavily" / "bocha"
    #[serde(default = "default_assistant_search_provider")]
    pub assistant_search_provider: String,
    /// 搜索服务 API Key（为空时 web_search 工具不注册，网页读取/天气不受影响）
    #[serde(default)]
    pub assistant_search_api_key: Option<String>,
    /// 语义记忆总开关（隐私默认关：开启后 OCR/标题会送往嵌入模型；
    /// 本地 Ollama 时数据不出机，云端 API 时明示出网）
    #[serde(default)]
    pub memory_semantic_enabled: bool,
    /// 是否开启后台定时增量语义索引（默认关闭；仅在开启语义记忆后生效，开启后会定期将新增/更新活动的文本送往已配置嵌入服务）
    #[serde(default)]
    pub semantic_auto_index: bool,
    /// 嵌入服务商："ollama"（本地默认）/ "openai"（OpenAI 兼容 /v1/embeddings）
    #[serde(default = "default_embedding_provider")]
    pub embedding_provider: String,
    /// 嵌入服务地址（Ollama 默认 http://localhost:11434）
    #[serde(default = "default_embedding_endpoint")]
    pub embedding_endpoint: String,
    /// 嵌入模型名（Ollama 默认 nomic-embed-text；OpenAI 兼容常用 BAAI/bge-m3 等）
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// 嵌入服务 API Key（Ollama 不需要）
    #[serde(default)]
    pub embedding_api_key: Option<String>,
    /// 是否启用桌面化身窗口
    #[serde(default)]
    pub avatar_enabled: bool,
    /// 隐藏桌宠本体（仅保留通知气泡与卡片），窗口收缩到仅容纳通知区（#137 诉求二）
    #[serde(default)]
    pub avatar_body_hidden: bool,
    /// 桌宠缩放比例（0.7 - 1.3）
    #[serde(default = "default_avatar_scale")]
    pub avatar_scale: f64,
    /// 桌宠猫体透明度（0.45 - 1.0）
    #[serde(default = "default_avatar_opacity")]
    pub avatar_opacity: f64,
    /// 桌宠官方预设
    #[serde(default = "default_avatar_preset")]
    pub avatar_preset: String,
    /// 桌宠互动风格
    #[serde(default = "default_avatar_persona")]
    pub avatar_persona: String,
    /// 桌宠是否允许调用文本模型生成不定时提醒
    #[serde(default)]
    pub avatar_proactive_ai_enabled: bool,
    /// 桌宠鼠标穿透（点击事件穿透到下层窗口）
    #[serde(default)]
    pub avatar_click_through: bool,
    /// 通过桌宠手动记下的待跟进项
    #[serde(default)]
    pub avatar_followups: Vec<AvatarFollowupItem>,
    /// 桌宠窗口横向位置
    #[serde(default)]
    pub avatar_x: Option<i32>,
    /// 桌宠窗口纵向位置
    #[serde(default)]
    pub avatar_y: Option<i32>,
    /// 隐藏系统标题栏装饰
    #[serde(default)]
    pub hide_decorations: bool,
    /// 背景图片文件名（存储在 data 目录下）
    #[serde(default)]
    pub background_image: Option<String>,
    /// 背景图片不透明度 (0.01 - 0.60)
    #[serde(default = "default_bg_opacity")]
    pub background_opacity: f32,
    /// 背景图片模糊程度 (0 = 清晰, 1 = 轻微, 2 = 中等)
    #[serde(default = "default_bg_blur")]
    pub background_blur: u8,
}

/// 配置加载结果，保留恢复状态和主备文件错误信息。
#[derive(Debug)]
pub struct ConfigLoadResult {
    pub config: AppConfig,
    pub status: ConfigLoadStatus,
    pub primary_error: Option<String>,
    pub backup_error: Option<String>,
}

#[allow(clippy::large_enum_variant)]
enum ConfigFileLoad {
    Loaded(AppConfig),
    Missing,
    Invalid(String),
}

fn default_work_start() -> u8 {
    9
}
fn default_work_end() -> u8 {
    18
}
fn default_bg_opacity() -> f32 {
    0.25
}
fn default_bg_blur() -> u8 {
    1
}
fn default_break_reminder_interval_minutes() -> u64 {
    50
}
fn default_avatar_scale() -> f64 {
    0.9
}
fn default_assistant_search_provider() -> String {
    "tavily".to_string()
}
fn default_embedding_provider() -> String {
    "ollama".to_string()
}
fn default_embedding_endpoint() -> String {
    "http://localhost:11434".to_string()
}
fn default_embedding_model() -> String {
    "nomic-embed-text".to_string()
}
fn default_standard_work_hours() -> f64 {
    8.0
}
fn default_avatar_opacity() -> f64 {
    0.82
}
fn default_avatar_preset() -> String {
    "original-standard".to_string()
}
fn default_avatar_persona() -> String {
    "assistant".to_string()
}
fn default_ui_visual_style() -> String {
    "c".to_string()
}
pub fn default_assistant_timeout_secs() -> u64 {
    120
}
pub fn default_report_generation_timeout_secs() -> u64 {
    300
}
fn normalize_assistant_timeout_secs(value: u64) -> u64 {
    value.clamp(30, 900)
}
fn normalize_report_generation_timeout_secs(value: u64) -> u64 {
    value.clamp(60, 1800)
}
fn normalize_positive_token(value: Option<u32>) -> Option<u32> {
    value.filter(|n| *n > 0)
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            screenshot_interval: 30,
            locale: default_locale(),
            idle_threshold_minutes: default_idle_threshold_minutes(),
            ai_mode: AiMode::Local,
            text_model: ModelConfig::default_text(),
            assistant_timeout_secs: default_assistant_timeout_secs(),
            report_generation_timeout_secs: default_report_generation_timeout_secs(),
            text_model_profiles: Vec::new(),
            text_model_provider_cache: HashMap::new(),
            task_model_routing: TaskModelRouting::default(),
            vision_model: ModelConfig::default_vision(),
            privacy: PrivacyConfig::default(),
            app_category_rules: Vec::new(),
            custom_categories: Vec::new(),
            website_semantic_rules: Vec::new(),
            custom_semantic_categories: Vec::new(),
            deleted_default_categories: Vec::new(),
            deleted_default_semantic_categories: Vec::new(),
            projects: Vec::new(),
            storage: StorageConfig::default(),
            remote_storage: RemoteStorageConfig::default(),
            config_synced_at: 0,
            daily_report_custom_prompt: String::new(),
            daily_report_prompt_presets: Vec::new(),
            daily_report_system_prompt_override: None,
            daily_report_export_dir: None,
            daily_report_auto_export: false,
            daily_report_export_frontmatter: false,
            daily_report_export_section_heading: None,
            git_report_enabled: false,
            git_repo_paths: Vec::new(),
            git_author_filter: None,
            daily_report_pinned_blocks: Vec::new(),
            daily_report_hidden_blocks: Vec::new(),
            daily_report_last_ai_order: Vec::new(),
            daily_report_auto_generate_time: None,
            localhost_api_enabled: false,
            localhost_api_host: None,
            localhost_api_port: DEFAULT_LOCALHOST_API_PORT,
            telegram_bot_enabled: false,
            telegram_bot_token: None,
            telegram_bot_proxy: None,
            telegram_bot_allowed_chat_ids: Vec::new(),
            telegram_bot_bind_code: None,
            telegram_bot_bind_code_expires_at: None,
            feishu_bot_enabled: false,
            feishu_app_id: None,
            feishu_app_secret: None,
            feishu_verification_token: None,
            feishu_encrypt_key: None,
            wecom_bot_enabled: false,
            wecom_corp_id: None,
            wecom_token: None,
            wecom_encoding_aes_key: None,
            wecom_bot_id: None,
            wecom_bot_secret: None,
            dingtalk_bot_enabled: false,
            dingtalk_app_secret: None,
            bot_sender_bindings: HashMap::new(),
            bot_natural_language_enabled: false,
            mcp_server_enabled: false,
            mcp_include_ocr: false,
            localhost_api_include_ocr: false,
            node_devices: Vec::new(),
            node_gateway: NodeGatewayConfig::default(),
            auto_start: false,
            auto_start_silent: false,
            macos_screen_capture_permission_prompted: false,
            last_app_version: None,
            theme: "system".to_string(),
            ui_visual_style: default_ui_visual_style(),
            report_editor_font: String::new(),
            work_start_hour: 9,
            work_end_hour: 18,
            work_start_minute: 0,
            work_end_minute: 0,
            work_time_segments: vec![
                WorkTimeSegment {
                    start_hour: 9,
                    start_minute: 0,
                    end_hour: 12,
                    end_minute: 0,
                },
                WorkTimeSegment {
                    start_hour: 13,
                    start_minute: 0,
                    end_hour: 18,
                    end_minute: 0,
                },
            ],
            work_time_enabled: true,
            standard_work_hours: default_standard_work_hours(),
            // 旧版兼容字段
            ai_provider: AiProviderConfig::default(),
            ollama_host: "http://localhost:11434".to_string(),
            ollama_model: "llava".to_string(),
            openai_api_key: None,
            openai_model: AiProvider::OpenAI.default_model().to_string(),
            hide_dock_icon: false,
            lightweight_mode: false,
            break_reminder_enabled: false,
            break_reminder_interval_minutes: default_break_reminder_interval_minutes(),
            daily_work_goal_minutes: None,
            goal_notifications: false,
            memory_enabled: false,
            memory_last_synthesis_date: None,
            assistant_memory_enabled: false,
            assistant_web_access_enabled: false,
            assistant_search_provider: default_assistant_search_provider(),
            assistant_search_api_key: None,
            memory_semantic_enabled: false,
            semantic_auto_index: false,
            embedding_provider: default_embedding_provider(),
            embedding_endpoint: default_embedding_endpoint(),
            embedding_model: default_embedding_model(),
            embedding_api_key: None,
            avatar_enabled: false,
            avatar_body_hidden: false,
            avatar_scale: default_avatar_scale(),
            avatar_opacity: default_avatar_opacity(),
            avatar_preset: default_avatar_preset(),
            avatar_persona: default_avatar_persona(),
            avatar_proactive_ai_enabled: false,
            avatar_click_through: false,
            avatar_followups: Vec::new(),
            avatar_x: None,
            avatar_y: None,
            hide_decorations: false,
            background_image: None,
            background_opacity: 0.25,
            background_blur: 1,
        }
    }
}

/// 可经 WebDAV 同步的偏好字段白名单（`AppConfig` 的 JSON 键名）。
///
/// 采用白名单而非黑名单：新增字段默认只留在本机。以下几类字段**不得**加入：
/// 密钥/Token、模型与嵌入端点、会把本地数据发往外部服务的开关（AI 模式、联网、
/// 语义索引、记忆合成、主动 AI）、Bot / MCP / 本地 API / 节点配置、远程存储、
/// 存储与清理策略、本机路径与窗口坐标；含窗口标题等工作内容的数据（如桌宠待办）；
/// 会改写模型指令的系统提示词覆盖；让记录器更不易被察觉的开关（隐藏 Dock 图标、轻量模式）。
/// 隐私规则单独走 [`merge_privacy_additive`]。
pub const SYNCED_PREFERENCE_FIELDS: &[&str] = &[
    "screenshot_interval",
    "locale",
    "idle_threshold_minutes",
    "assistant_timeout_secs",
    "report_generation_timeout_secs",
    "app_category_rules",
    "custom_categories",
    "website_semantic_rules",
    "custom_semantic_categories",
    "deleted_default_categories",
    "deleted_default_semantic_categories",
    "daily_report_custom_prompt",
    "daily_report_prompt_presets",
    "daily_report_auto_generate_time",
    "daily_report_pinned_blocks",
    "daily_report_hidden_blocks",
    "daily_report_last_ai_order",
    "theme",
    "ui_visual_style",
    "report_editor_font",
    "work_start_hour",
    "work_end_hour",
    "work_start_minute",
    "work_end_minute",
    "work_time_segments",
    "work_time_enabled",
    "standard_work_hours",
    "break_reminder_enabled",
    "break_reminder_interval_minutes",
    "daily_work_goal_minutes",
    "goal_notifications",
    "avatar_enabled",
    "avatar_body_hidden",
    "avatar_scale",
    "avatar_opacity",
    "avatar_preset",
    "avatar_persona",
    "avatar_click_through",
    "hide_decorations",
    "background_opacity",
    "background_blur",
];

/// 同步隐私规则所用的 JSON 键名（只增不减合并，不走白名单直接覆盖）。
pub const SYNCED_PRIVACY_FIELD: &str = "privacy";

fn privacy_level_rank(level: PrivacyLevel) -> u8 {
    match level {
        PrivacyLevel::Full => 0,
        PrivacyLevel::Anonymized => 1,
        PrivacyLevel::Ignored => 2,
    }
}

fn push_unique_case_insensitive(target: &mut Vec<String>, values: &[String]) {
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !target
            .iter()
            .any(|existing| existing.trim().eq_ignore_ascii_case(trimmed))
        {
            target.push(trimmed.to_string());
        }
    }
}

/// 隐私规则"只增不减"合并：远端只能新增或收紧规则，不能删除或降级本机规则。
///
/// - 同名应用（按归一化名称对齐，含别名与 `.exe`）取更严格的级别（忽略 > 脱敏 > 完整记录）；
///   远端的"完整记录"规则或归一化后为空（如 `.exe`）的规则不新增，也不得降级遗留 `excluded_apps`。
/// - 排除关键词、排除域名、旧版忽略应用取并集；`filter_sensitive` 取或。
pub fn merge_privacy_additive(local: &PrivacyConfig, remote: &PrivacyConfig) -> PrivacyConfig {
    const MAX_SYNCED_PRIVACY_ENTRIES: usize = 1024;

    let mut merged = local.clone();
    let local_excluded_keys: std::collections::HashSet<String> = merged
        .excluded_apps
        .iter()
        .map(|app| normalized_app_rule_key(app))
        .filter(|key| !key.is_empty())
        .collect();

    for remote_rule in remote.app_rules.iter().take(MAX_SYNCED_PRIVACY_ENTRIES) {
        let name = remote_rule.app_name.trim();
        let remote_key = normalized_app_rule_key(name);
        if name.is_empty() || remote_key.is_empty() {
            continue;
        }
        // 本机遗留 excluded_apps 已视为 Ignored，禁止远端通过 app_rules 写入更宽松级别降级
        if local_excluded_keys.contains(&remote_key) && remote_rule.level != PrivacyLevel::Ignored {
            continue;
        }
        let mut matched_any = false;
        for existing in merged.app_rules.iter_mut() {
            if normalized_app_rule_key(&existing.app_name) == remote_key {
                matched_any = true;
                if privacy_level_rank(remote_rule.level) > privacy_level_rank(existing.level) {
                    existing.level = remote_rule.level;
                }
            }
        }
        if !matched_any
            && remote_rule.level != PrivacyLevel::Full
            && merged.app_rules.len() < MAX_SYNCED_PRIVACY_ENTRIES
        {
            merged.app_rules.push(AppPrivacyRule {
                app_name: name.to_string(),
                level: remote_rule.level,
            });
        }
    }
    push_unique_case_insensitive(
        &mut merged.excluded_keywords,
        &remote.excluded_keywords[..remote
            .excluded_keywords
            .len()
            .min(MAX_SYNCED_PRIVACY_ENTRIES)],
    );
    push_unique_case_insensitive(
        &mut merged.excluded_domains,
        &remote.excluded_domains[..remote
            .excluded_domains
            .len()
            .min(MAX_SYNCED_PRIVACY_ENTRIES)],
    );
    let valid_remote_excluded_apps: Vec<String> = remote
        .excluded_apps
        .iter()
        .take(MAX_SYNCED_PRIVACY_ENTRIES)
        .filter(|app| !normalized_app_rule_key(app).is_empty())
        .cloned()
        .collect();
    push_unique_case_insensitive(&mut merged.excluded_apps, &valid_remote_excluded_apps);
    merged.filter_sensitive = local.filter_sensitive || remote.filter_sensitive;
    merged.skip_private_windows = local.skip_private_windows || remote.skip_private_windows;
    merged.unknown_url_fallback = if local.effective_unknown_url_fallback()
        == UnknownUrlFallback::Anonymize
        || remote.effective_unknown_url_fallback() == UnknownUrlFallback::Anonymize
    {
        Some(UnknownUrlFallback::Anonymize)
    } else {
        local.unknown_url_fallback.or(remote.unknown_url_fallback)
    };
    merged
}

impl AppConfig {
    /// 生成用于上传的同步快照：仅包含白名单偏好与隐私规则，绝不含密钥、端点与本机字段。
    pub fn synced_preferences_value(&self) -> serde_json::Value {
        let full = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        let mut snapshot = serde_json::Map::new();
        if let Some(object) = full.as_object() {
            for key in SYNCED_PREFERENCE_FIELDS
                .iter()
                .copied()
                .chain(std::iter::once(SYNCED_PRIVACY_FIELD))
            {
                if let Some(value) = object.get(key) {
                    snapshot.insert(key.to_string(), value.clone());
                }
            }
        }
        serde_json::Value::Object(snapshot)
    }

    /// 应用远端同步快照（兼容旧版整份配置格式）。
    ///
    /// 只采纳白名单字段；隐私规则只增不减；其余字段（密钥、端点、Bot、远程存储、
    /// 存储策略、本机路径等）一律保留本机值。单个字段类型不兼容时跳过该字段。
    pub fn apply_synced_preferences(&mut self, remote: &serde_json::Value) {
        let Some(remote_object) = remote.as_object() else {
            return;
        };
        let Ok(serde_json::Value::Object(base)) = serde_json::to_value(&*self) else {
            return;
        };

        let mut candidate = base.clone();
        for key in SYNCED_PREFERENCE_FIELDS {
            if let Some(value) = remote_object.get(*key) {
                candidate.insert((*key).to_string(), value.clone());
            }
        }

        let mut next: AppConfig = match serde_json::from_value(serde_json::Value::Object(candidate))
        {
            Ok(next) => next,
            Err(_) => {
                // 逐字段尝试，跳过类型不兼容的远端字段（如旧版本写入的历史格式）。
                let mut accepted = base;
                for key in SYNCED_PREFERENCE_FIELDS {
                    let Some(value) = remote_object.get(*key) else {
                        continue;
                    };
                    let mut trial = accepted.clone();
                    trial.insert((*key).to_string(), value.clone());
                    if serde_json::from_value::<AppConfig>(serde_json::Value::Object(trial.clone()))
                        .is_ok()
                    {
                        accepted = trial;
                    } else {
                        log::warn!("跳过类型不兼容的同步字段: {key}");
                    }
                }
                match serde_json::from_value(serde_json::Value::Object(accepted)) {
                    Ok(next) => next,
                    Err(_) => return,
                }
            }
        };

        if let Some(raw_privacy) = remote_object.get(SYNCED_PRIVACY_FIELD) {
            match serde_json::from_value::<PrivacyConfig>(raw_privacy.clone()) {
                Ok(remote_privacy) => {
                    next.privacy = merge_privacy_additive(&self.privacy, &remote_privacy);
                }
                Err(error) => {
                    log::warn!("远端同步隐私配置解析失败，保留本机隐私规则: {error}");
                }
            }
        }
        next.work_time_segments.truncate(32);
        next.daily_report_prompt_presets.truncate(64);

        *self = next;
    }

    /// 兼容入口：以整份 `AppConfig` 作为远端快照应用（内部同样只采纳白名单字段）。
    pub fn apply_synced_remote(&mut self, remote: AppConfig) {
        let remote_value = serde_json::to_value(remote).unwrap_or(serde_json::Value::Null);
        self.apply_synced_preferences(&remote_value);
    }

    /// 生成不含密钥与凭据的备份配置副本（默认备份策略，见 M3-2）。
    pub fn redacted_for_backup(&self) -> Self {
        fn redact_url_userinfo(raw: &str) -> String {
            let trimmed = raw.trim();
            if let Ok(mut url) = reqwest::Url::parse(trimmed) {
                if !url.username().is_empty() || url.password().is_some() {
                    let _ = url.set_username("");
                    let _ = url.set_password(None);
                    return url.to_string();
                }
            }
            raw.to_string()
        }

        let mut copy = self.clone();
        copy.text_model.endpoint = redact_url_userinfo(&copy.text_model.endpoint);
        copy.text_model.api_key = None;
        copy.vision_model.endpoint = redact_url_userinfo(&copy.vision_model.endpoint);
        copy.vision_model.api_key = None;
        copy.ai_provider.endpoint = redact_url_userinfo(&copy.ai_provider.endpoint);
        copy.ai_provider.api_key = None;
        copy.openai_api_key = None;
        for profile in &mut copy.text_model_profiles {
            profile.model_config.endpoint = redact_url_userinfo(&profile.model_config.endpoint);
            profile.model_config.api_key = None;
        }
        for model in copy.text_model_provider_cache.values_mut() {
            model.endpoint = redact_url_userinfo(&model.endpoint);
            model.api_key = None;
        }
        copy.remote_storage.s3.endpoint = redact_url_userinfo(&copy.remote_storage.s3.endpoint);
        copy.remote_storage.s3.access_key.clear();
        copy.remote_storage.s3.secret_key.clear();
        copy.remote_storage.webdav.url = redact_url_userinfo(&copy.remote_storage.webdav.url);
        copy.remote_storage.webdav.password.clear();
        copy.telegram_bot_token = None;
        copy.telegram_bot_proxy = copy.telegram_bot_proxy.as_deref().map(redact_url_userinfo);
        copy.telegram_bot_bind_code = None;
        copy.telegram_bot_bind_code_expires_at = None;
        copy.feishu_app_secret = None;
        copy.feishu_verification_token = None;
        copy.feishu_encrypt_key = None;
        copy.wecom_token = None;
        copy.wecom_encoding_aes_key = None;
        copy.wecom_bot_secret = None;
        copy.dingtalk_app_secret = None;
        copy.assistant_search_api_key = None;
        copy.embedding_endpoint = redact_url_userinfo(&copy.embedding_endpoint);
        copy.embedding_api_key = None;
        for device in &mut copy.node_devices {
            device.url = redact_url_userinfo(&device.url);
            device.token.clear();
        }
        for binding in copy.bot_sender_bindings.values_mut() {
            binding.bind_code = None;
            binding.bind_code_expires_at = None;
            binding.failed_attempts = 0;
        }
        copy
    }

    /// 恢复不含密钥的备份时，仅在服务目标地址与账户身份一致时回填本机已有密钥，
    /// 避免把本机凭据误填到备份指向的新端点或不同账户上。
    pub fn preserve_missing_secrets_from(&mut self, current: &AppConfig) {
        let same_endpoint = |a: &str, b: &str| -> bool {
            let ta = a.trim();
            let tb = b.trim();
            if ta.is_empty() || tb.is_empty() {
                return false;
            }
            match (reqwest::Url::parse(ta), reqwest::Url::parse(tb)) {
                (Ok(ua), Ok(ub)) => {
                    // 仅对 scheme 与 host 做不区分大小写规范化（reqwest::Url 已自动规范化）；
                    // 路径、查询串、用户名及显式端口一律保持大小写敏感的严格比对，
                    // 防止大小写敏感的多租户路由（如 /TenantA vs /tenanta）被误判为同一端点。
                    fn norm_path(u: &reqwest::Url) -> &str {
                        let p = u.path();
                        if p == "/" {
                            ""
                        } else {
                            p.trim_end_matches('/')
                        }
                    }
                    ua.scheme() == ub.scheme()
                        && ua.host_str() == ub.host_str()
                        && ua.port_or_known_default() == ub.port_or_known_default()
                        && ua.username() == ub.username()
                        && norm_path(&ua) == norm_path(&ub)
                        && ua.query() == ub.query()
                }
                _ => ta.trim_end_matches('/') == tb.trim_end_matches('/'),
            }
        };
        let same_non_empty_opt = |a: &Option<String>, b: &Option<String>| -> bool {
            match (
                a.as_deref().map(str::trim).filter(|s| !s.is_empty()),
                b.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            ) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            }
        };

        if self.text_model.api_key.is_none()
            && self.text_model.provider == current.text_model.provider
            && same_endpoint(&self.text_model.endpoint, &current.text_model.endpoint)
        {
            self.text_model.api_key = current.text_model.api_key.clone();
        }
        if self.vision_model.api_key.is_none()
            && self.vision_model.provider == current.vision_model.provider
            && same_endpoint(&self.vision_model.endpoint, &current.vision_model.endpoint)
        {
            self.vision_model.api_key = current.vision_model.api_key.clone();
        }
        if self.ai_provider.api_key.is_none()
            && self.ai_provider.provider == current.ai_provider.provider
            && same_endpoint(&self.ai_provider.endpoint, &current.ai_provider.endpoint)
        {
            self.ai_provider.api_key = current.ai_provider.api_key.clone();
        }
        if self.openai_api_key.is_none()
            && self.text_model.provider == current.text_model.provider
            && same_endpoint(&self.text_model.endpoint, &current.text_model.endpoint)
        {
            self.openai_api_key = current.openai_api_key.clone();
        }
        for profile in &mut self.text_model_profiles {
            if profile.model_config.api_key.is_none() {
                if let Some(cur) = current.text_model_profiles.iter().find(|p| {
                    p.id == profile.id
                        && p.model_config.provider == profile.model_config.provider
                        && same_endpoint(&p.model_config.endpoint, &profile.model_config.endpoint)
                }) {
                    profile.model_config.api_key = cur.model_config.api_key.clone();
                }
            }
        }
        for (provider_key, model) in &mut self.text_model_provider_cache {
            if model.api_key.is_none() {
                if let Some(cur) = current.text_model_provider_cache.get(provider_key) {
                    if cur.provider == model.provider
                        && same_endpoint(&cur.endpoint, &model.endpoint)
                    {
                        model.api_key = cur.api_key.clone();
                    }
                }
            }
        }
        let s3_same_target = same_endpoint(
            &self.remote_storage.s3.endpoint,
            &current.remote_storage.s3.endpoint,
        ) && !self.remote_storage.s3.bucket.trim().is_empty()
            && self.remote_storage.s3.bucket.trim() == current.remote_storage.s3.bucket.trim();
        let restored_s3_access = self.remote_storage.s3.access_key.trim();
        let live_s3_access = current.remote_storage.s3.access_key.trim();
        if s3_same_target {
            if restored_s3_access.is_empty() && self.remote_storage.s3.secret_key.trim().is_empty()
            {
                // 脱敏备份同时清空了 access_key 与 secret_key：按成对凭据一起回填
                self.remote_storage.s3.access_key = current.remote_storage.s3.access_key.clone();
                self.remote_storage.s3.secret_key = current.remote_storage.s3.secret_key.clone();
            } else if !restored_s3_access.is_empty()
                && restored_s3_access == live_s3_access
                && self.remote_storage.s3.secret_key.trim().is_empty()
            {
                // 仅当 access_key 严格一致时才补齐缺失的 secret_key，避免把旧密钥填给新账户
                self.remote_storage.s3.secret_key = current.remote_storage.s3.secret_key.clone();
            }
        }
        let webdav_same_target = same_endpoint(
            &self.remote_storage.webdav.url,
            &current.remote_storage.webdav.url,
        ) && !self.remote_storage.webdav.username.trim().is_empty()
            && self.remote_storage.webdav.username.trim()
                == current.remote_storage.webdav.username.trim();
        if webdav_same_target && self.remote_storage.webdav.password.is_empty() {
            self.remote_storage.webdav.password = current.remote_storage.webdav.password.clone();
        }
        if self.telegram_bot_token.is_none()
            && self.telegram_bot_enabled
            && current.telegram_bot_enabled
            && self.telegram_bot_allowed_chat_ids == current.telegram_bot_allowed_chat_ids
        {
            self.telegram_bot_token = current.telegram_bot_token.clone();
        }
        if same_non_empty_opt(&self.feishu_app_id, &current.feishu_app_id) {
            if self.feishu_app_secret.is_none() {
                self.feishu_app_secret = current.feishu_app_secret.clone();
            }
            if self.feishu_verification_token.is_none() {
                self.feishu_verification_token = current.feishu_verification_token.clone();
            }
            if self.feishu_encrypt_key.is_none() {
                self.feishu_encrypt_key = current.feishu_encrypt_key.clone();
            }
        }
        if same_non_empty_opt(&self.wecom_corp_id, &current.wecom_corp_id) {
            if self.wecom_token.is_none() {
                self.wecom_token = current.wecom_token.clone();
            }
            if self.wecom_encoding_aes_key.is_none() {
                self.wecom_encoding_aes_key = current.wecom_encoding_aes_key.clone();
            }
        }
        if same_non_empty_opt(&self.wecom_bot_id, &current.wecom_bot_id)
            && self.wecom_bot_secret.is_none()
        {
            self.wecom_bot_secret = current.wecom_bot_secret.clone();
        }
        if self.dingtalk_app_secret.is_none()
            && self.dingtalk_bot_enabled
            && current.dingtalk_bot_enabled
            && self
                .bot_sender_bindings
                .get("dingtalk")
                .map(|b| &b.allowed_senders)
                == current
                    .bot_sender_bindings
                    .get("dingtalk")
                    .map(|b| &b.allowed_senders)
        {
            self.dingtalk_app_secret = current.dingtalk_app_secret.clone();
        }
        if self.assistant_search_api_key.is_none()
            && self
                .assistant_search_provider
                .eq_ignore_ascii_case(&current.assistant_search_provider)
        {
            self.assistant_search_api_key = current.assistant_search_api_key.clone();
        }
        if self.embedding_api_key.is_none()
            && self
                .embedding_provider
                .eq_ignore_ascii_case(&current.embedding_provider)
            && same_endpoint(&self.embedding_endpoint, &current.embedding_endpoint)
        {
            self.embedding_api_key = current.embedding_api_key.clone();
        }
        for device in &mut self.node_devices {
            if device.token.is_empty() {
                if let Some(cur) = current
                    .node_devices
                    .iter()
                    .find(|d| same_endpoint(&d.url, &device.url) && d.name == device.name)
                {
                    device.token = cur.token.clone();
                }
            }
        }
    }

    /// 规范化配置，兼容旧字段并补齐助手可用的文本模型档案
    pub fn normalize(&mut self) {
        self.migrate_legacy_config();
        self.privacy.normalize();
        crate::project::normalize_project_definitions(&mut self.projects);
        normalize_custom_categories(&mut self.custom_categories);
        normalize_app_category_rules(&mut self.app_category_rules, &self.custom_categories);
        // Seed defaults BEFORE rule validation so built-in categories are present
        seed_default_categories(
            &mut self.custom_categories,
            &self.deleted_default_categories,
        );
        seed_default_semantic_categories(
            &mut self.custom_semantic_categories,
            &self.deleted_default_semantic_categories,
        );
        normalize_custom_semantic_categories(&mut self.custom_semantic_categories);
        normalize_website_semantic_rules(
            &mut self.website_semantic_rules,
            &self.custom_semantic_categories,
        );
        self.screenshot_interval = normalize_screenshot_interval(self.screenshot_interval);
        self.idle_threshold_minutes = normalize_idle_threshold_minutes(self.idle_threshold_minutes);
        self.ui_visual_style = normalize_ui_visual_style(&self.ui_visual_style);
        self.avatar_scale = normalize_avatar_scale(self.avatar_scale);
        self.avatar_opacity = normalize_avatar_opacity(self.avatar_opacity);
        self.avatar_preset = normalize_avatar_preset(&self.avatar_preset);
        self.avatar_persona = normalize_avatar_persona(&self.avatar_persona);
        normalize_avatar_followups(&mut self.avatar_followups);
        self.break_reminder_interval_minutes =
            normalize_break_reminder_interval_minutes(self.break_reminder_interval_minutes);
        self.standard_work_hours = self.standard_work_hours.clamp(1.0, 24.0);
        self.assistant_timeout_secs = normalize_assistant_timeout_secs(self.assistant_timeout_secs);
        self.report_generation_timeout_secs =
            normalize_report_generation_timeout_secs(self.report_generation_timeout_secs);
        self.text_model.thinking_budget = normalize_positive_token(self.text_model.thinking_budget);
        self.text_model.max_output_tokens =
            normalize_positive_token(self.text_model.max_output_tokens);
        if self.text_model.enable_thinking == Some(true)
            && self.text_model.thinking_budget == Some(0)
        {
            self.text_model.thinking_budget = None;
        }
        self.daily_report_custom_prompt = self.daily_report_custom_prompt.trim().to_string();
        normalize_prompt_presets(&mut self.daily_report_prompt_presets);
        self.daily_report_export_dir =
            normalize_optional_string(self.daily_report_export_dir.take());
        self.daily_report_export_section_heading =
            normalize_optional_string(self.daily_report_export_section_heading.take());
        self.git_author_filter = normalize_optional_string(self.git_author_filter.take());
        normalize_git_repo_paths(&mut self.git_repo_paths);
        self.task_model_routing.report_provider =
            normalize_optional_string(self.task_model_routing.report_provider.take());
        self.task_model_routing.assistant_provider =
            normalize_optional_string(self.task_model_routing.assistant_provider.take());
        self.task_model_routing.classification_provider =
            normalize_optional_string(self.task_model_routing.classification_provider.take());
        self.task_model_routing.proactive_provider =
            normalize_optional_string(self.task_model_routing.proactive_provider.take());
        self.localhost_api_port = normalize_localhost_api_port(self.localhost_api_port);
        self.localhost_api_host = normalize_optional_string(self.localhost_api_host.take());
        self.wecom_bot_id = normalize_optional_string(self.wecom_bot_id.take());
        self.wecom_bot_secret = normalize_optional_string(self.wecom_bot_secret.take());
        self.telegram_bot_bind_code = normalize_optional_string(self.telegram_bot_bind_code.take())
            .map(|code| code.to_ascii_uppercase());
        if self.telegram_bot_bind_code.is_none() {
            self.telegram_bot_bind_code_expires_at = None;
        }
        self.remote_storage.s3.public_url_base =
            normalize_optional_string(self.remote_storage.s3.public_url_base.take());
        self.remote_storage.webdav.public_url_base =
            normalize_optional_string(self.remote_storage.webdav.public_url_base.take());
        self.node_gateway.device_name =
            normalize_optional_string(self.node_gateway.device_name.take());
        self.sync_text_model_profiles();
    }

    /// 从文件加载配置
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            Self::load_existing(path)
        } else {
            Ok(Self::default())
        }
    }

    /// 加载配置并返回可供启动流程判定的状态。
    pub fn load_with_recovery(path: &Path) -> ConfigLoadResult {
        let backup_path = config_backup_path(path);
        match Self::load_file(path) {
            ConfigFileLoad::Loaded(config) => {
                let backup_error = match Self::load_file(&backup_path) {
                    ConfigFileLoad::Invalid(error) => Some(error),
                    ConfigFileLoad::Loaded(_) | ConfigFileLoad::Missing => None,
                };
                ConfigLoadResult {
                    config,
                    status: ConfigLoadStatus::Loaded,
                    primary_error: None,
                    backup_error,
                }
            }
            ConfigFileLoad::Missing => match Self::load_file(&backup_path) {
                ConfigFileLoad::Loaded(config) => ConfigLoadResult {
                    config,
                    status: ConfigLoadStatus::RecoveredFromBackup,
                    primary_error: None,
                    backup_error: None,
                },
                ConfigFileLoad::Missing => ConfigLoadResult {
                    config: Self::default(),
                    status: ConfigLoadStatus::Missing,
                    primary_error: None,
                    backup_error: None,
                },
                ConfigFileLoad::Invalid(backup_error) => ConfigLoadResult {
                    config: Self::fail_safe(),
                    status: ConfigLoadStatus::Corrupted,
                    primary_error: None,
                    backup_error: Some(backup_error),
                },
            },
            ConfigFileLoad::Invalid(primary_error) => match Self::load_file(&backup_path) {
                ConfigFileLoad::Loaded(config) => ConfigLoadResult {
                    config,
                    status: ConfigLoadStatus::RecoveredFromBackup,
                    primary_error: Some(primary_error),
                    backup_error: None,
                },
                ConfigFileLoad::Missing => ConfigLoadResult {
                    config: Self::fail_safe(),
                    status: ConfigLoadStatus::Corrupted,
                    primary_error: Some(primary_error),
                    backup_error: None,
                },
                ConfigFileLoad::Invalid(backup_error) => ConfigLoadResult {
                    config: Self::fail_safe(),
                    status: ConfigLoadStatus::Corrupted,
                    primary_error: Some(primary_error),
                    backup_error: Some(backup_error),
                },
            },
        }
    }

    fn load_file(path: &Path) -> ConfigFileLoad {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return ConfigFileLoad::Missing;
            }
            Err(error) => {
                return ConfigFileLoad::Invalid(format!(
                    "读取配置文件 {} 失败: {error}",
                    path.display()
                ));
            }
        };

        match serde_json::from_str::<AppConfig>(&content) {
            Ok(mut config) => {
                config.normalize();
                ConfigFileLoad::Loaded(config)
            }
            Err(error) => {
                ConfigFileLoad::Invalid(format!("解析配置文件 {} 失败: {error}", path.display()))
            }
        }
    }

    fn load_existing(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: AppConfig = serde_json::from_str(&content)?;
        config.normalize();
        Ok(config)
    }

    fn fail_safe() -> Self {
        let mut config = Self::default();
        config.storage.screenshots_enabled = false;
        config.localhost_api_enabled = false;
        config.telegram_bot_enabled = false;
        config.feishu_bot_enabled = false;
        config.wecom_bot_enabled = false;
        config.dingtalk_bot_enabled = false;
        config.mcp_server_enabled = false;
        config.remote_storage.provider = RemoteStorageProvider::None;
        config
    }

    /// 保存配置到文件
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_vec_pretty(self)?;
        let parent = config_parent(path);
        std::fs::create_dir_all(parent)?;

        let (mut pending_config, mut config_file) = PendingConfigFile::create(path)?;
        config_file.write_all(&content)?;
        config_file.flush()?;
        config_file.sync_all()?;
        drop(config_file);

        update_config_backup(path)?;

        commit_pending_config(&mut pending_config, path, parent, sync_parent_directory)?;
        Ok(())
    }

    /// 迁移旧版配置到新结构
    fn migrate_legacy_config(&mut self) {
        // 迁移旧版单段工作时间到 work_time_segments
        if self.work_time_segments.is_empty() {
            self.work_time_segments = vec![WorkTimeSegment {
                start_hour: self.work_start_hour,
                start_minute: self.work_start_minute,
                end_hour: self.work_end_hour,
                end_minute: self.work_end_minute,
            }];
        }
        // 如果新配置为空但旧配置有值，则迁移
        if self.text_model.model.is_empty() && !self.ai_provider.model.is_empty() {
            self.text_model = ModelConfig {
                provider: self.ai_provider.provider,
                endpoint: self.ai_provider.endpoint.clone(),
                api_key: self.ai_provider.api_key.clone(),
                model: self.ai_provider.model.clone(),
                enable_thinking: None,
                thinking_budget: None,
                max_output_tokens: None,
            };
        }

        if self.vision_model.model.is_empty() {
            if let Some(ref vision_model) = self.ai_provider.vision_model {
                self.vision_model = ModelConfig {
                    provider: self.ai_provider.provider,
                    endpoint: self.ai_provider.endpoint.clone(),
                    api_key: self.ai_provider.api_key.clone(),
                    model: vision_model.clone(),
                    enable_thinking: None,
                    thinking_budget: None,
                    max_output_tokens: None,
                };
            }
        }

        // 如果旧版 ollama 配置存在
        if !self.ollama_host.is_empty() && self.text_model.endpoint.is_empty() {
            self.text_model.endpoint = self.ollama_host.clone();
            self.vision_model.endpoint = self.ollama_host.clone();
        }

        // 迁移旧版背景不透明度（0.05 太低，用户看不到背景）
        if self.background_opacity <= 0.05 && self.background_image.is_some() {
            self.background_opacity = 0.25;
        }

        self.sync_text_model_profiles();
    }

    /// 获取文本模型端点
    pub fn get_text_endpoint(&self) -> &str {
        &self.text_model.endpoint
    }

    /// 按任务场景解析实际使用的文本模型配置：
    /// 优先查找对应场景指定的模型档案或已缓存服务商，未配置或不完整时回退到主模型 `text_model`。
    pub fn resolve_model_for_task(&self, task: AiTaskKind) -> ModelConfig {
        let route_key = match task {
            AiTaskKind::Report => self.task_model_routing.report_provider.as_deref(),
            AiTaskKind::Assistant => self.task_model_routing.assistant_provider.as_deref(),
            AiTaskKind::Classification => {
                self.task_model_routing.classification_provider.as_deref()
            }
            AiTaskKind::Proactive => self.task_model_routing.proactive_provider.as_deref(),
        }
        .map(str::trim)
        .filter(|value| !value.is_empty());

        let Some(key) = route_key else {
            return self.text_model.clone();
        };

        let candidate = self
            .text_model_profiles
            .iter()
            .find(|profile| profile.id == key)
            .map(|profile| profile.model_config.clone())
            .or_else(|| {
                if self.text_model.provider.as_str().eq_ignore_ascii_case(key) {
                    Some(self.text_model.clone())
                } else {
                    None
                }
            })
            .or_else(|| {
                self.text_model_provider_cache
                    .get(key)
                    .cloned()
                    .or_else(|| {
                        self.text_model_provider_cache
                            .iter()
                            .find(|(provider_key, _)| provider_key.eq_ignore_ascii_case(key))
                            .map(|(_, cached)| cached.clone())
                    })
            });

        match candidate {
            Some(model) if is_routable_model_configured(&model) => model,
            _ => self.text_model.clone(),
        }
    }

    /// 获取视觉模型端点
    pub fn get_vision_endpoint(&self) -> &str {
        &self.vision_model.endpoint
    }

    /// 获取有效的工作时间段列表
    /// - work_time_enabled=false: 返回全天段 (0:00-23:59)，所有活动都算工作时间
    /// - work_time_enabled=true + segments 非空: 使用自定义时间段
    /// - work_time_enabled=true + segments 为空: 从旧字段构造一段
    pub fn effective_work_segments(&self) -> Vec<WorkTimeSegment> {
        if !self.work_time_enabled {
            return vec![WorkTimeSegment {
                start_hour: 0,
                start_minute: 0,
                end_hour: 23,
                end_minute: 59,
            }];
        }
        if !self.work_time_segments.is_empty() {
            return self.work_time_segments.clone();
        }
        vec![WorkTimeSegment {
            start_hour: self.work_start_hour,
            start_minute: self.work_start_minute,
            end_hour: self.work_end_hour,
            end_minute: self.work_end_minute,
        }]
    }

    fn sync_text_model_profiles(&mut self) {
        if !is_model_configured(&self.text_model) {
            return;
        }

        if let Some(profile) = self
            .text_model_profiles
            .iter_mut()
            .find(|profile| profile.id == "default-text-model")
        {
            profile.name = default_profile_name(&self.text_model);
            profile.model_config = self.text_model.clone();
            if profile.test_status.trim().is_empty() {
                profile.test_status = default_connection_status();
            }
            return;
        }

        if self
            .text_model_profiles
            .iter()
            .any(|profile| same_model_config(&profile.model_config, &self.text_model))
        {
            return;
        }

        self.text_model_profiles.insert(
            0,
            TextModelProfile {
                id: "default-text-model".to_string(),
                name: default_profile_name(&self.text_model),
                model_config: self.text_model.clone(),
                test_status: default_connection_status(),
                last_tested_at: None,
                last_test_message: None,
            },
        );
    }
}

fn is_model_configured(model_config: &ModelConfig) -> bool {
    !model_config.endpoint.trim().is_empty() && !model_config.model.trim().is_empty()
}

fn is_routable_model_configured(model_config: &ModelConfig) -> bool {
    if !is_model_configured(model_config) {
        return false;
    }
    if !model_config.provider.requires_api_key() {
        return true;
    }
    model_config
        .api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty())
}

fn normalize_git_repo_paths(paths: &mut Vec<String>) {
    const MAX_GIT_REPOS: usize = 32;
    let mut normalized = Vec::new();
    for raw in paths.iter() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
        {
            continue;
        }
        normalized.push(trimmed.to_string());
        if normalized.len() >= MAX_GIT_REPOS {
            break;
        }
    }
    *paths = normalized;
}

fn same_model_config(left: &ModelConfig, right: &ModelConfig) -> bool {
    left.provider == right.provider
        && left.endpoint.trim() == right.endpoint.trim()
        && left.model.trim() == right.model.trim()
        && left.api_key.as_deref().unwrap_or("").trim()
            == right.api_key.as_deref().unwrap_or("").trim()
        && left.enable_thinking == right.enable_thinking
        && left.thinking_budget == right.thinking_budget
        && left.max_output_tokens == right.max_output_tokens
}

fn default_profile_name(model_config: &ModelConfig) -> String {
    // 仅新建 profile 时的一次性默认命名；运行时显示走前端。
    #[allow(deprecated)]
    let provider_name = model_config.provider.display_name();
    format!("{} · {}", provider_name, model_config.model.trim())
}

fn default_connection_status() -> String {
    "untested".to_string()
}

pub fn normalize_category_key_private(value: &str, custom_keys: &[String]) -> String {
    let trimmed = value.trim().to_lowercase();
    match trimmed.as_str() {
        "development" | "browser" | "communication" | "office" | "design" | "entertainment"
        | "other" => trimmed,
        _ if custom_keys.iter().any(|k| k == &trimmed) => trimmed,
        _ => "other".to_string(),
    }
}

fn normalize_custom_categories(categories: &mut Vec<CustomCategory>) {
    use std::collections::HashSet;

    let mut seen = HashSet::new();
    categories.retain(|c| {
        let key = c.key.trim().to_lowercase();
        // key 只允许小写字母、数字、连字符
        let valid_key = !key.is_empty()
            && key
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
        // name 非空，color 以 # 开头且 7 字符
        let valid_color = c.color.starts_with('#') && c.color.len() == 7;
        if !valid_key || c.name.trim().is_empty() || !valid_color {
            return false;
        }
        seen.insert(key)
    });
}

fn normalize_app_category_rules(
    rules: &mut Vec<AppCategoryRule>,
    custom_categories: &[CustomCategory],
) {
    use std::collections::HashSet;

    let mut normalized_rules = Vec::new();
    let mut seen = HashSet::new();

    for rule in rules.iter().rev() {
        let app_name = rule.app_name.trim();
        if app_name.is_empty() {
            continue;
        }

        let normalized_app_name = crate::categorize::normalize_display_app_name(app_name);
        let dedupe_key = normalized_app_name.to_lowercase();
        if !seen.insert(dedupe_key) {
            continue;
        }

        let custom_keys: Vec<String> = custom_categories.iter().map(|c| c.key.clone()).collect();
        normalized_rules.push(AppCategoryRule {
            app_name: normalized_app_name,
            category: normalize_category_key_private(&rule.category, &custom_keys),
        });
    }

    normalized_rules.reverse();
    *rules = normalized_rules;
}

fn normalize_website_semantic_rules(
    rules: &mut Vec<WebsiteSemanticRule>,
    custom_semantic_categories: &[CustomSemanticCategory],
) {
    use std::collections::HashSet;

    let custom_keys: Vec<String> = custom_semantic_categories
        .iter()
        .map(|c| c.key.clone())
        .collect();
    let mut normalized_rules = Vec::new();
    let mut seen = HashSet::new();

    for rule in rules.iter().rev() {
        let Some(domain) = crate::categorize::normalize_domain_rule(&rule.domain) else {
            continue;
        };

        let semantic_category = rule.semantic_category.trim();
        if semantic_category.is_empty() {
            continue;
        }

        // 验证语义分类：必须在自定义分类列表中
        let is_custom = custom_keys.iter().any(|k| k == semantic_category);
        if !is_custom {
            continue;
        }

        if !seen.insert(domain.clone()) {
            continue;
        }

        normalized_rules.push(WebsiteSemanticRule {
            domain,
            semantic_category: semantic_category.to_string(),
        });
    }

    normalized_rules.reverse();
    *rules = normalized_rules;
}

pub fn is_valid_builtin_semantic_category(category: &str) -> bool {
    matches!(
        category,
        "编码开发"
            | "内容撰写"
            | "资料阅读"
            | "资料调研"
            | "任务规划"
            | "设计创作"
            | "AI 协作"
            | "即时聊天"
            | "会议沟通"
            | "视频内容"
            | "音乐音频"
            | "休息娱乐"
            | "未知活动"
    )
}

fn normalize_custom_semantic_categories(categories: &mut Vec<CustomSemanticCategory>) {
    use std::collections::HashSet;

    let mut seen = HashSet::new();
    categories.retain(|c| {
        let key = c.key.trim();
        let name = c.name.trim();
        if key.is_empty() || name.is_empty() {
            return false;
        }
        seen.insert(key.to_string())
    });
}

/// 内置应用分类 key 列表（统一维护点）
pub const DEFAULT_CATEGORY_KEYS: &[&str] = &[
    "development",
    "browser",
    "communication",
    "office",
    "design",
    "entertainment",
    "other",
];

/// 内置语义分类 key 列表（统一维护点）
pub const DEFAULT_SEMANTIC_CATEGORY_KEYS: &[&str] = &[
    "编码开发",
    "内容撰写",
    "资料阅读",
    "资料调研",
    "任务规划",
    "设计创作",
    "AI 协作",
    "即时聊天",
    "会议沟通",
    "视频内容",
    "音乐音频",
    "休息娱乐",
    "未知活动",
];

fn seed_default_categories(categories: &mut Vec<CustomCategory>, deleted: &[String]) {
    let defaults: Vec<(&str, &str, &str, &str)> = vec![
        ("development", "开发工具", "#3B82F6", "⚡"),
        ("browser", "浏览器", "#22C55E", "🌐"),
        ("communication", "通讯协作", "#EAB308", "💬"),
        ("office", "办公软件", "#8B5CF6", "📝"),
        ("design", "设计工具", "#EC4899", "🎨"),
        ("entertainment", "娱乐摸鱼", "#EF4444", "🎮"),
        ("other", "其他", "#6B7280", "📁"),
    ];
    let existing_keys: Vec<String> = categories.iter().map(|c| c.key.clone()).collect();
    for (key, name, color, icon) in defaults {
        if deleted.iter().any(|d| d == key) {
            continue;
        }
        if !existing_keys.iter().any(|k| k == key) {
            categories.push(CustomCategory {
                key: key.to_string(),
                name: name.to_string(),
                color: color.to_string(),
                icon: icon.to_string(),
            });
        }
    }
}

fn seed_default_semantic_categories(
    categories: &mut Vec<CustomSemanticCategory>,
    deleted: &[String],
) {
    let defaults: Vec<(&str, &str)> = vec![
        ("编码开发", "编码开发"),
        ("内容撰写", "内容撰写"),
        ("资料阅读", "资料阅读"),
        ("资料调研", "资料调研"),
        ("任务规划", "任务规划"),
        ("设计创作", "设计创作"),
        ("AI 协作", "AI 协作"),
        ("即时聊天", "即时聊天"),
        ("会议沟通", "会议沟通"),
        ("视频内容", "视频内容"),
        ("音乐音频", "音乐音频"),
        ("休息娱乐", "休息娱乐"),
        ("未知活动", "未知活动"),
    ];
    let existing_keys: Vec<String> = categories.iter().map(|c| c.key.clone()).collect();
    for (key, name) in defaults {
        if deleted.iter().any(|d| d == key) {
            continue;
        }
        if !existing_keys.iter().any(|k| k == key) {
            categories.push(CustomSemanticCategory {
                key: key.to_string(),
                name: name.to_string(),
            });
        }
    }
}

fn normalize_prompt_presets(presets: &mut Vec<PromptPreset>) {
    for p in presets.iter_mut() {
        p.name = p.name.trim().to_string();
        p.prompt = p.prompt.trim().to_string();
    }
    presets.retain(|p| !p.name.is_empty() && !p.prompt.is_empty());
}

fn normalize_avatar_scale(value: f64) -> f64 {
    if !value.is_finite() {
        return default_avatar_scale();
    }

    value.clamp(0.4, 1.3)
}

fn normalize_avatar_opacity(value: f64) -> f64 {
    if !value.is_finite() {
        return default_avatar_opacity();
    }

    value.clamp(0.45, 1.0)
}

fn normalize_avatar_preset(value: &str) -> String {
    match value.trim() {
        "original-standard" | "keyboard-focus" | "minimal-office" => value.trim().to_string(),
        _ => default_avatar_preset(),
    }
}

fn normalize_avatar_persona(value: &str) -> String {
    match value.trim() {
        "companion" | "assistant" | "coach" => value.trim().to_string(),
        _ => default_avatar_persona(),
    }
}

fn normalize_ui_visual_style(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "a" | "b" | "c" => value.trim().to_ascii_lowercase(),
        _ => default_ui_visual_style(),
    }
}

fn normalize_avatar_followups(items: &mut Vec<AvatarFollowupItem>) {
    let mut seen = std::collections::HashSet::new();
    items.retain_mut(|item| {
        item.id = item.id.trim().to_string();
        item.title = item.title.trim().to_string();
        item.date = item.date.trim().to_string();
        item.source_app = item.source_app.trim().to_string();
        item.source_title = item.source_title.trim().to_string();
        item.project_key = item.project_key.trim().to_string();
        item.status = match item.status.trim() {
            "done" => "done".to_string(),
            _ => default_avatar_followup_status(),
        };

        if item.title.is_empty() || item.project_key.is_empty() || item.date.is_empty() {
            return false;
        }

        let dedupe_key = format!(
            "{}::{}::{}",
            item.project_key.to_lowercase(),
            item.title.to_lowercase(),
            item.status
        );
        seen.insert(dedupe_key)
    });

    items.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.title.cmp(&b.title))
    });
    items.truncate(200);
}

fn normalize_break_reminder_interval_minutes(value: u64) -> u64 {
    match value {
        30 | 45 | 50 | 60 | 90 | 120 => value,
        _ => default_break_reminder_interval_minutes(),
    }
}

/// 截屏间隔最低 5 秒，防止配置值过小导致 CPU/磁盘占用过高
fn normalize_screenshot_interval(value: u64) -> u64 {
    value.clamp(5, 600)
}

fn default_idle_threshold_minutes() -> u32 {
    5
}

fn normalize_idle_threshold_minutes(value: u32) -> u32 {
    value.clamp(1, 60)
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::merge_privacy_additive;
    use super::{
        commit_pending_config, config_backup_path, default_avatar_opacity, default_avatar_persona,
        default_avatar_preset, default_avatar_scale, default_ui_visual_style,
        normalize_app_category_rules, normalize_avatar_followups, normalize_avatar_opacity,
        normalize_avatar_persona, normalize_avatar_preset, normalize_avatar_scale,
        normalize_ui_visual_style, update_config_backup_with_sync, AiProvider, AiTaskKind,
        AppCategoryRule, AppConfig, AppPrivacyRule, AvatarFollowupItem, ConfigLoadStatus,
        ModelConfig, PendingConfigFile, PrivacyConfig, PrivacyLevel, RemoteStorageProvider,
        ScreenshotDisplayMode, TaskModelRouting, WebsiteSemanticRule, DEFAULT_LOCALHOST_API_PORT,
        SYNCED_PREFERENCE_FIELDS, SYNCED_PRIVACY_FIELD,
    };
    use std::io::{self, Write};
    use std::path::PathBuf;

    #[test]
    fn privacy_extract_domain_normalizes_hosts() {
        for (input, expected) in [
            ("https://example.com/path", "example.com"),
            ("https://example.com:8443/path", "example.com"),
            (
                " HTTPS://Sub.Example.COM:8443?query=1#fragment ",
                "sub.example.com",
            ),
            ("example.com", "example.com"),
            ("Example.COM:8443/path?query=1#fragment", "example.com"),
            ("example.com?query=1", "example.com"),
            ("example.com#fragment", "example.com"),
            (
                "example.com?next=https://other.com&email=a@b.com",
                "example.com",
            ),
            ("https://WWW.Example.com:8443", "www.example.com"),
            ("https://Sub.Example.com.:8443/path", "sub.example.com"),
            ("http://localhost:5173/app", "localhost"),
            ("127.0.0.1:8080", "127.0.0.1"),
            ("https://[::1]:8443/path", "[::1]"),
            ("", ""),
            ("   ", ""),
            ("not a host", ""),
            ("https://", ""),
            ("https://example.com:invalid/path", ""),
            ("example.com:99999", ""),
            ("/example.com/path", ""),
            ("mailto:user@example.com", ""),
            ("about:blank", ""),
            ("file:///tmp/example.com", ""),
        ] {
            assert_eq!(
                super::PrivacyConfig::extract_domain(input),
                expected,
                "{input:?}"
            );
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "work-review-config-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn 元数据自动清理须兼容旧配置且不得经远端同步开启() {
        let mut legacy = serde_json::to_value(AppConfig::default()).unwrap();
        legacy["storage"]
            .as_object_mut()
            .unwrap()
            .remove("metadata_cleanup_enabled");
        let mut local: AppConfig = serde_json::from_value(legacy).unwrap();
        assert!(!local.storage.metadata_cleanup_enabled);
        assert_eq!(local.storage.metadata_retention_days, 30);
        let mut remote = AppConfig::default();
        remote.storage.metadata_cleanup_enabled = true;
        remote.storage.metadata_retention_days = 1;
        local.apply_synced_remote(remote);
        assert!(!local.storage.metadata_cleanup_enabled);
        assert_eq!(local.storage.metadata_retention_days, 30);
        local.storage.metadata_cleanup_enabled = true;
        local.storage.metadata_retention_days = 90;
        let restored: AppConfig =
            serde_json::from_value(serde_json::to_value(&local).unwrap()).unwrap();
        assert!(restored.storage.metadata_cleanup_enabled);
        assert_eq!(restored.storage.metadata_retention_days, 90);
        local.apply_synced_remote(AppConfig::default());
        assert!(local.storage.metadata_cleanup_enabled);
        assert_eq!(local.storage.metadata_retention_days, 90);
    }

    #[test]
    fn 未配置模型时规范化与重载不应生成默认模型档案() {
        let mut config = AppConfig::default();
        config.normalize();
        assert!(config.text_model.model.is_empty());
        assert!(config.text_model_profiles.is_empty());

        let mut value = serde_json::to_value(&config).unwrap();
        value.as_object_mut().unwrap().remove("ai_provider");
        let mut reloaded: AppConfig = serde_json::from_value(value).unwrap();
        reloaded.normalize();
        reloaded.normalize();
        assert!(reloaded.text_model.model.is_empty());
        assert!(reloaded.text_model_profiles.is_empty());
        assert!(reloaded.text_model_provider_cache.is_empty());
    }

    #[test]
    fn 旧版显式保存的本地模型仍应迁移到主模型() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("text_model");
        value["ai_provider"]["model"] = serde_json::json!("qwen3");
        let mut config: AppConfig = serde_json::from_value(value).unwrap();
        config.normalize();
        assert_eq!(config.text_model.provider, AiProvider::Ollama);
        assert_eq!(config.text_model.model, "qwen3");
        assert_eq!(config.text_model_profiles.len(), 1);
        assert_eq!(config.text_model_profiles[0].model_config.model, "qwen3");
    }

    #[test]
    fn 配置文件不存在时应返回首次启动状态() {
        let dir = unique_temp_dir("config-missing");

        let result = AppConfig::load_with_recovery(&dir.join("config.json"));

        assert_eq!(result.status, ConfigLoadStatus::Missing);
        assert!(!result.status.requires_fail_safe());
        assert!(result.status.allows_automatic_save());
    }

    #[test]
    fn 服务商缓存字段应兼容旧配置文件并完整往返() {
        // 模拟旧版配置文件：序列化默认配置后移除新字段再反序列化
        let mut legacy = serde_json::to_value(AppConfig::default()).unwrap();
        assert!(legacy
            .as_object_mut()
            .unwrap()
            .remove("text_model_provider_cache")
            .is_some());
        let old: AppConfig = serde_json::from_value(legacy).expect("旧配置缺少缓存字段时应可解析");
        assert!(old.text_model_provider_cache.is_empty());

        // 各家服务商的配置（含 API Key）应无损往返
        let mut config = AppConfig::default();
        config.text_model_provider_cache.insert(
            "deepseek".to_string(),
            super::ModelConfig {
                provider: AiProvider::DeepSeek,
                endpoint: "https://api.deepseek.com".to_string(),
                api_key: Some("sk-test".to_string()),
                model: "deepseek-chat".to_string(),
                ..super::ModelConfig::default_text()
            },
        );
        let round: AppConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert_eq!(
            round
                .text_model_provider_cache
                .get("deepseek")
                .and_then(|m| m.api_key.as_deref()),
            Some("sk-test")
        );
    }

    #[test]
    fn 同步白名单字段均应存在于配置序列化结果中() {
        let value = serde_json::to_value(AppConfig::default()).unwrap();
        let object = value.as_object().unwrap();
        for key in SYNCED_PREFERENCE_FIELDS
            .iter()
            .chain(std::iter::once(&SYNCED_PRIVACY_FIELD))
        {
            assert!(object.contains_key(*key), "白名单字段不存在: {key}");
        }
    }

    #[test]
    fn 同步白名单不得包含密钥端点与外发开关() {
        const FORBIDDEN: &[&str] = &[
            "text_model",
            "vision_model",
            "text_model_profiles",
            "text_model_provider_cache",
            "ai_provider",
            "ai_mode",
            "openai_api_key",
            "ollama_host",
            "assistant_search_api_key",
            "assistant_search_provider",
            "assistant_web_access_enabled",
            "embedding_api_key",
            "embedding_endpoint",
            "memory_semantic_enabled",
            "semantic_auto_index",
            "memory_enabled",
            "avatar_proactive_ai_enabled",
            "remote_storage",
            "storage",
            "localhost_api_enabled",
            "localhost_api_host",
            "localhost_api_port",
            "mcp_server_enabled",
            "node_devices",
            "node_gateway",
            "telegram_bot_enabled",
            "telegram_bot_token",
            "telegram_bot_allowed_chat_ids",
            "feishu_bot_enabled",
            "feishu_app_secret",
            "wecom_bot_enabled",
            "wecom_bot_secret",
            "dingtalk_bot_enabled",
            "dingtalk_app_secret",
            "bot_natural_language_enabled",
            "daily_report_export_dir",
            "daily_report_auto_export",
            "daily_report_export_frontmatter",
            "daily_report_export_section_heading",
            "task_model_routing",
            "git_report_enabled",
            "git_repo_paths",
            "git_author_filter",
            "auto_start",
            "avatar_x",
            "avatar_y",
            "background_image",
            "bot_sender_bindings",
            "mcp_include_ocr",
            "localhost_api_include_ocr",
            "avatar_followups",
            "daily_report_system_prompt_override",
            "hide_dock_icon",
            "lightweight_mode",
        ];
        for key in FORBIDDEN {
            assert!(
                !SYNCED_PREFERENCE_FIELDS.contains(key),
                "敏感或本机字段不得进入同步白名单: {key}"
            );
        }
        let defaults = AppConfig::default();
        assert!(!defaults.semantic_auto_index);
        assert!(!defaults.bot_natural_language_enabled);
    }

    #[test]
    fn 同步快照不应包含任何密钥值() {
        let mut config = AppConfig::default();
        config.text_model.api_key = Some("sk-secret-text".to_string());
        config.embedding_api_key = Some("sk-secret-embed".to_string());
        config.telegram_bot_token = Some("tg-secret".to_string());
        config.wecom_bot_secret = Some("wecom-secret".to_string());
        config.remote_storage.webdav.password = "dav-secret".to_string();
        config.daily_report_custom_prompt = "保留的提示词".to_string();

        let snapshot = serde_json::to_string(&config.synced_preferences_value()).unwrap();
        for secret in [
            "sk-secret-text",
            "sk-secret-embed",
            "tg-secret",
            "wecom-secret",
            "dav-secret",
        ] {
            assert!(!snapshot.contains(secret), "同步快照泄露了密钥: {secret}");
        }
        assert!(snapshot.contains("保留的提示词"));
    }

    #[test]
    fn 远端同步不得覆盖本机密钥端点与机器人配置() {
        let mut local = AppConfig::default();
        local.text_model.endpoint = "https://api.local-choice.example".to_string();
        local.text_model.api_key = Some("local-key".to_string());
        local.telegram_bot_allowed_chat_ids = vec![1];
        local.storage.screenshot_retention_days = 30;

        let mut remote = AppConfig::default();
        remote.text_model.endpoint = "https://attacker.example".to_string();
        remote.text_model.api_key = None;
        remote.telegram_bot_allowed_chat_ids = vec![1, 666];
        remote.assistant_web_access_enabled = true;
        remote.storage.screenshot_retention_days = 1;
        remote.theme = "dark".to_string();

        local.apply_synced_remote(remote);
        assert_eq!(
            local.text_model.endpoint,
            "https://api.local-choice.example"
        );
        assert_eq!(local.text_model.api_key.as_deref(), Some("local-key"));
        assert_eq!(local.telegram_bot_allowed_chat_ids, vec![1]);
        assert!(!local.assistant_web_access_enabled);
        assert_eq!(local.storage.screenshot_retention_days, 30);
        assert_eq!(local.theme, "dark");
    }

    #[test]
    fn 远端同步的隐私规则只增不减() {
        let mut local = AppConfig::default();
        local.privacy.app_rules = vec![
            AppPrivacyRule {
                app_name: "WeChat".to_string(),
                level: PrivacyLevel::Ignored,
            },
            AppPrivacyRule {
                app_name: "Slack".to_string(),
                level: PrivacyLevel::Anonymized,
            },
        ];
        local.privacy.excluded_domains = vec!["bank.example".to_string()];

        let mut remote = AppConfig::default();
        remote.privacy.app_rules = vec![
            // 试图降级本机规则
            AppPrivacyRule {
                app_name: "wechat".to_string(),
                level: PrivacyLevel::Full,
            },
            // 收紧本机规则
            AppPrivacyRule {
                app_name: "Slack".to_string(),
                level: PrivacyLevel::Ignored,
            },
            // 新增规则
            AppPrivacyRule {
                app_name: "1Password".to_string(),
                level: PrivacyLevel::Ignored,
            },
            // 新增"完整记录"规则无意义，不应写入
            AppPrivacyRule {
                app_name: "Notes".to_string(),
                level: PrivacyLevel::Full,
            },
        ];
        // 远端删除了本机排除域名，并新增一个
        remote.privacy.excluded_domains = vec!["mail.example".to_string()];

        local.apply_synced_remote(remote);
        let level_of = |name: &str| {
            local
                .privacy
                .app_rules
                .iter()
                .find(|rule| rule.app_name.eq_ignore_ascii_case(name))
                .map(|rule| rule.level)
        };
        assert_eq!(level_of("WeChat"), Some(PrivacyLevel::Ignored));
        assert_eq!(level_of("Slack"), Some(PrivacyLevel::Ignored));
        assert_eq!(level_of("1Password"), Some(PrivacyLevel::Ignored));
        assert_eq!(level_of("Notes"), None);
        assert!(local
            .privacy
            .excluded_domains
            .contains(&"bank.example".to_string()));
        assert!(local
            .privacy
            .excluded_domains
            .contains(&"mail.example".to_string()));
    }

    #[test]
    fn 忽略应用收集应归一化可执行名并与匹配侧对称() {
        let mut privacy = PrivacyConfig::default();
        privacy.app_rules.push(AppPrivacyRule {
            app_name: "msedge.exe".to_string(),
            level: PrivacyLevel::Ignored,
        });
        let ignored = privacy.collect_ignored_app_names();
        assert!(ignored.contains(&"microsoft edge".to_string()));
        assert!(crate::privacy::matches_ignored_app("msedge.exe", &ignored));
        assert!(crate::privacy::matches_ignored_app(
            "Microsoft Edge",
            &ignored
        ));
    }

    #[test]
    fn 远端隐私同步不得用空归一化通配或降级遗留规则或被别名遮蔽() {
        let mut local = PrivacyConfig::default();
        local.excluded_apps = vec!["1Password".to_string()];
        local.app_rules = vec![AppPrivacyRule {
            app_name: "WeChat".to_string(),
            level: PrivacyLevel::Full,
        }];
        let mut remote = PrivacyConfig::default();
        remote.app_rules = vec![
            // .exe 归一化后为空，不得接收成为通配规则
            AppPrivacyRule {
                app_name: ".exe".to_string(),
                level: PrivacyLevel::Ignored,
            },
            // 不得把本机遗留 excluded_apps 降级为 Anonymized
            AppPrivacyRule {
                app_name: "1Password".to_string(),
                level: PrivacyLevel::Anonymized,
            },
            // 中文/可执行别名收紧
            AppPrivacyRule {
                app_name: "WeChat.exe".to_string(),
                level: PrivacyLevel::Ignored,
            },
        ];
        remote.excluded_apps = vec![".exe".to_string()];
        let merged = merge_privacy_additive(&local, &remote);
        assert_eq!(merged.get_app_privacy_level("VS Code"), PrivacyLevel::Full);
        assert_eq!(
            merged.get_app_privacy_level("1Password"),
            PrivacyLevel::Ignored
        );
        assert_eq!(
            merged.get_app_privacy_level("WeChat"),
            PrivacyLevel::Ignored
        );
        // 空关键词不应匹配任意窗口标题
        let mut empty_kw = PrivacyConfig::default();
        empty_kw.excluded_keywords = vec!["".to_string(), "   ".to_string()];
        assert!(!empty_kw.should_anonymize_by_keyword("Normal Window"));
    }

    #[test]
    fn 远端单个字段类型不兼容时应跳过该字段而保留其余字段() {
        let mut local = AppConfig::default();
        local.theme = "light".to_string();
        let remote = serde_json::json!({
            "theme": "dark",
            "work_start_hour": "not-a-number",
        });
        local.apply_synced_preferences(&remote);
        assert_eq!(local.theme, "dark");
        assert_eq!(local.work_start_hour, AppConfig::default().work_start_hour);
    }

    #[test]
    fn 应用远端同步快照时应保留本机端口与窗口位置() {
        let mut local = AppConfig::default();
        local.localhost_api_port = 12345;
        local.avatar_x = Some(40);
        local.locale = "zh-CN".to_string();
        let mut remote = AppConfig::default();
        remote.localhost_api_port = 47831;
        remote.avatar_x = Some(9);
        remote.locale = "en".to_string();
        remote.assistant_timeout_secs = 180;
        local.apply_synced_remote(remote);
        assert_eq!(local.localhost_api_port, 12345);
        assert_eq!(local.avatar_x, Some(40));
        assert_eq!(local.locale, "en");
        assert_eq!(local.assistant_timeout_secs, 180);
    }

    #[test]
    fn 旧配置缺少超时与思考字段时应使用默认值() {
        let mut legacy = serde_json::to_value(AppConfig::default()).unwrap();
        let object = legacy.as_object_mut().unwrap();
        assert!(object.remove("assistant_timeout_secs").is_some());
        assert!(object.remove("report_generation_timeout_secs").is_some());
        let text_model = object
            .get_mut("text_model")
            .unwrap()
            .as_object_mut()
            .unwrap();
        text_model.remove("enable_thinking");
        text_model.remove("thinking_budget");
        text_model.remove("max_output_tokens");
        let old: AppConfig =
            serde_json::from_value(legacy).expect("旧配置缺少超时与思考字段时应可解析");
        assert_eq!(old.assistant_timeout_secs, 120);
        assert_eq!(old.report_generation_timeout_secs, 300);
        assert_eq!(old.text_model.enable_thinking, None);
        assert_eq!(old.text_model.thinking_budget, None);
        assert_eq!(old.text_model.max_output_tokens, None);
    }

    #[test]
    fn 超时与思考预算应规范化到合法范围() {
        let mut config = AppConfig::default();
        config.assistant_timeout_secs = 1;
        config.report_generation_timeout_secs = 10_000;
        config.text_model.thinking_budget = Some(0);
        config.text_model.max_output_tokens = Some(0);
        config.normalize();
        assert_eq!(config.assistant_timeout_secs, 30);
        assert_eq!(config.report_generation_timeout_secs, 1800);
        assert_eq!(config.text_model.thinking_budget, None);
        assert_eq!(config.text_model.max_output_tokens, None);
    }

    #[test]
    fn 旧配置缺少企微长连接字段时应可解析() {
        let mut legacy = serde_json::to_value(AppConfig::default()).unwrap();
        let object = legacy.as_object_mut().unwrap();
        assert!(object.remove("wecom_bot_id").is_some());
        assert!(object.remove("wecom_bot_secret").is_some());
        let old: AppConfig =
            serde_json::from_value(legacy).expect("旧配置缺少企微长连接字段时应可解析");
        assert_eq!(old.wecom_bot_id, None);
        assert_eq!(old.wecom_bot_secret, None);
    }

    #[test]
    fn 配置规范化应清理企微长连接空白凭证() {
        let mut config = AppConfig {
            wecom_bot_id: Some("  bot-id  ".to_string()),
            wecom_bot_secret: Some("   ".to_string()),
            ..AppConfig::default()
        };
        config.normalize();
        assert_eq!(config.wecom_bot_id.as_deref(), Some("bot-id"));
        assert_eq!(config.wecom_bot_secret, None);
    }

    #[test]
    fn 合法主配置应返回正常加载状态() {
        let dir = unique_temp_dir("config-loaded");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&AppConfig::default()).expect("配置应可序列化"),
        )
        .expect("应写入合法主配置");

        let result = AppConfig::load_with_recovery(&path);

        assert_eq!(result.status, ConfigLoadStatus::Loaded);
        assert!(result.primary_error.is_none());
        assert!(result.backup_error.is_none());
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 主配置有效但备份损坏时应保留正常加载状态并报告备份错误() {
        let dir = unique_temp_dir("config-valid-primary-broken-backup");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        let mut primary = AppConfig::default();
        primary.locale = "en".to_string();
        std::fs::write(
            &path,
            serde_json::to_vec(&primary).expect("主配置应可序列化"),
        )
        .expect("应写入合法主配置");
        std::fs::write(&backup_path, b"broken-backup").expect("应写入损坏备份");

        let result = AppConfig::load_with_recovery(&path);

        assert_eq!(result.status, ConfigLoadStatus::Loaded);
        assert_eq!(result.config.locale, "en");
        assert!(result.primary_error.is_none());
        assert!(result.backup_error.is_some());
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 主配置缺失但备份有效时应从备份恢复() {
        let dir = unique_temp_dir("config-missing-primary-valid-backup");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        let mut backup = AppConfig::default();
        backup.locale = "en".to_string();
        std::fs::write(
            &backup_path,
            serde_json::to_vec(&backup).expect("备份配置应可序列化"),
        )
        .expect("应写入合法备份");

        let result = AppConfig::load_with_recovery(&path);

        assert_eq!(result.status, ConfigLoadStatus::RecoveredFromBackup);
        assert_eq!(result.config.locale, "en");
        assert!(result.primary_error.is_none());
        assert!(result.backup_error.is_none());
        assert!(!result.status.allows_automatic_save());
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 主配置缺失且备份损坏时应进入故障安全状态() {
        let dir = unique_temp_dir("config-missing-primary-broken-backup");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        std::fs::write(&backup_path, b"broken-backup").expect("应写入损坏备份");

        let result = AppConfig::load_with_recovery(&path);

        assert_eq!(result.status, ConfigLoadStatus::Corrupted);
        assert!(result.status.requires_fail_safe());
        assert!(result.primary_error.is_none());
        assert!(result.backup_error.is_some());
        assert!(!result.config.storage.screenshots_enabled);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(&backup_path).expect("应保留损坏备份"),
            "broken-backup"
        );
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 主配置损坏且备份缺失时应进入故障安全状态() {
        let dir = unique_temp_dir("config-broken-primary-missing-backup");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        std::fs::write(&path, b"broken-primary").expect("应写入损坏主配置");

        let result = AppConfig::load_with_recovery(&path);

        assert_eq!(result.status, ConfigLoadStatus::Corrupted);
        assert!(result.status.requires_fail_safe());
        assert!(result.primary_error.is_some());
        assert!(result.backup_error.is_none());
        assert!(!result.config.storage.screenshots_enabled);
        assert_eq!(
            std::fs::read_to_string(&path).expect("应保留损坏主配置"),
            "broken-primary"
        );
        assert!(!backup_path.exists());
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 备份路径应在完整文件名后追加bak() {
        let path = PathBuf::from("/tmp/work-review/config.json");

        assert_eq!(
            config_backup_path(&path),
            PathBuf::from("/tmp/work-review/config.json.bak")
        );
    }

    #[test]
    fn 主配置损坏时应从备份恢复且不覆盖主文件() {
        let dir = unique_temp_dir("config-backup-recovery");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        std::fs::write(&path, b"{ broken json").expect("应写入损坏主配置");
        let mut backup = AppConfig::default();
        backup.locale = "en".to_string();
        std::fs::write(
            &backup_path,
            serde_json::to_vec(&backup).expect("备份配置应可序列化"),
        )
        .expect("应写入合法备份");

        let result = AppConfig::load_with_recovery(&path);

        assert_eq!(result.status, ConfigLoadStatus::RecoveredFromBackup);
        assert_eq!(result.config.locale, "en");
        assert!(result.primary_error.is_some());
        assert!(result.backup_error.is_none());
        assert_eq!(
            std::fs::read_to_string(&path).expect("应读取原主配置"),
            "{ broken json"
        );
        assert!(!result.status.allows_automatic_save());
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 主配置与备份都损坏时应返回关闭采集和网络服务的安全配置() {
        let dir = unique_temp_dir("config-fail-safe");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        std::fs::write(&path, b"broken-primary").expect("应写入损坏主配置");
        std::fs::write(&backup_path, b"broken-backup").expect("应写入损坏备份");

        let result = AppConfig::load_with_recovery(&path);

        assert_eq!(result.status, ConfigLoadStatus::Corrupted);
        assert!(result.status.requires_fail_safe());
        assert!(!result.status.allows_automatic_save());
        assert!(result.primary_error.is_some());
        assert!(result.backup_error.is_some());
        assert!(!result.config.storage.screenshots_enabled);
        assert!(!result.config.localhost_api_enabled);
        assert!(!result.config.telegram_bot_enabled);
        assert!(!result.config.feishu_bot_enabled);
        assert!(!result.config.wecom_bot_enabled);
        assert!(!result.config.dingtalk_bot_enabled);
        assert!(!result.config.mcp_server_enabled);
        assert_eq!(
            result.config.remote_storage.provider,
            RemoteStorageProvider::None
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("应读取原主配置"),
            "broken-primary"
        );
        assert_eq!(
            std::fs::read_to_string(&backup_path).expect("应读取原备份"),
            "broken-backup"
        );
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 覆盖保存应生成可解析备份并写入新配置() {
        let dir = unique_temp_dir("config-atomic-save");
        let path = dir.join("config.json");
        let mut old = AppConfig::default();
        old.locale = "en".to_string();
        old.save(&path).expect("首次保存应成功");
        let mut new = old.clone();
        new.locale = "zh-CN".to_string();

        new.save(&path).expect("覆盖保存应成功");

        assert_eq!(
            AppConfig::load(&path).expect("主配置应可加载").locale,
            "zh-CN"
        );
        assert_eq!(
            AppConfig::load(&config_backup_path(&path))
                .expect("备份应可加载")
                .locale,
            "en"
        );
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 主文件提交后父目录同步失败不应返回保存失败() {
        let dir = unique_temp_dir("config-directory-sync-failure");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let (mut pending_config, mut config_file) =
            PendingConfigFile::create(&path).expect("应创建配置临时文件");
        config_file
            .write_all(b"new-config")
            .expect("应写入临时文件");
        config_file.flush().expect("应刷新临时文件");
        config_file.sync_all().expect("应同步临时文件");
        drop(config_file);

        let result = commit_pending_config(&mut pending_config, &path, &dir, |_| {
            Err(io::Error::other("模拟父目录同步失败"))
        });

        assert!(result.is_ok());
        assert_eq!(
            std::fs::read(&path).expect("提交后应读取主文件"),
            b"new-config"
        );
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 备份提交后父目录同步失败不应返回保存失败() {
        let dir = unique_temp_dir("config-backup-directory-sync-failure");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        let original_bytes = b"old-config";
        std::fs::write(&path, original_bytes).expect("应写入原主配置");

        let result =
            update_config_backup_with_sync(&path, |_| Err(io::Error::other("模拟父目录同步失败")));

        assert!(result.is_ok());
        assert_eq!(
            std::fs::read(&backup_path).expect("提交后应读取备份"),
            original_bytes
        );
        assert_eq!(
            std::fs::read(&path).expect("备份更新不应修改主配置"),
            original_bytes
        );
        let names = std::fs::read_dir(&dir)
            .expect("应列出临时目录")
            .map(|entry| {
                entry
                    .expect("目录项应有效")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert!(!names.iter().any(|name| name.contains(".tmp-")));
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 主配置不存在时首次保存应成功且不生成备份() {
        let dir = unique_temp_dir("config-first-save-without-backup");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        let config = AppConfig::default();

        config.save(&path).expect("首次保存应成功");

        assert!(path.is_file());
        assert!(!backup_path.exists());
        let names = std::fs::read_dir(&dir)
            .expect("应列出临时目录")
            .map(|entry| {
                entry
                    .expect("目录项应有效")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert!(!names.iter().any(|name| name.contains(".tmp-")));
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[cfg(unix)]
    #[test]
    fn 主配置无法打开时应禁止覆盖并清理临时文件() {
        use std::os::unix::fs::symlink;

        let dir = unique_temp_dir("config-primary-open-failure");
        std::fs::create_dir_all(&dir).expect("应创建临时目录");
        let path = dir.join("config.json");
        let backup_path = config_backup_path(&path);
        symlink(&path, &path).expect("应创建自引用符号链接");

        let result = AppConfig::default().save(&path);

        assert!(result.is_err());
        assert!(std::fs::symlink_metadata(&path)
            .expect("原符号链接应保留")
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&path).expect("应读取原符号链接"), path);
        assert!(!backup_path.exists());
        let names = std::fs::read_dir(&dir)
            .expect("应列出临时目录")
            .map(|entry| {
                entry
                    .expect("目录项应有效")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert!(!names.iter().any(|name| name.contains(".tmp-")));
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[cfg(unix)]
    #[test]
    fn 配置与备份权限应为0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = unique_temp_dir("config-permissions");
        let path = dir.join("config.json");
        let config = AppConfig::default();
        config.save(&path).expect("首次保存应成功");
        config.save(&path).expect("覆盖保存应成功");

        let primary_mode = std::fs::metadata(&path)
            .expect("应读取主配置元数据")
            .permissions()
            .mode()
            & 0o777;
        let backup_mode = std::fs::metadata(config_backup_path(&path))
            .expect("应读取备份元数据")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(primary_mode, 0o600);
        assert_eq!(backup_mode, 0o600);
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 备份更新失败时应保留原配置并清理临时文件() {
        let dir = unique_temp_dir("config-save-failure");
        let path = dir.join("config.json");
        let original = AppConfig::default();
        original.save(&path).expect("首次保存应成功");
        let backup_path = config_backup_path(&path);
        std::fs::create_dir_all(&backup_path).expect("应创建阻塞备份目录");
        std::fs::write(backup_path.join("blocker"), b"x").expect("应写入阻塞文件");
        let original_bytes = std::fs::read(&path).expect("应读取原配置");

        let mut changed = original.clone();
        changed.locale = "en".to_string();
        assert!(changed.save(&path).is_err());

        assert_eq!(
            std::fs::read(&path).expect("失败后应读取原配置"),
            original_bytes
        );
        let names = std::fs::read_dir(&dir)
            .expect("应列出临时目录")
            .map(|entry| {
                entry
                    .expect("目录项应有效")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert!(!names.iter().any(|name| name.contains(".tmp-")));
        std::fs::remove_dir_all(dir).expect("应清理临时目录");
    }

    #[test]
    fn 桌宠缩放默认值应为百分之九十() {
        let config = AppConfig::default();

        assert_eq!(config.avatar_scale, default_avatar_scale());
        assert_eq!(config.avatar_scale, 0.9);
    }

    #[test]
    fn 桌宠透明度默认值应为百分之八十二() {
        let config = AppConfig::default();

        assert_eq!(config.avatar_opacity, default_avatar_opacity());
        assert_eq!(config.avatar_opacity, 0.82);
    }

    #[test]
    fn 桌宠官方预设默认应为原版标准模式() {
        let config = AppConfig::default();

        assert_eq!(config.avatar_preset, default_avatar_preset());
        assert_eq!(config.avatar_preset, "original-standard");
    }

    #[test]
    fn 桌宠互动风格默认应为助手型() {
        let config = AppConfig::default();

        assert_eq!(config.avatar_persona, default_avatar_persona());
        assert_eq!(config.avatar_persona, "assistant");
    }

    #[test]
    fn 桌宠模型生成提醒默认应关闭() {
        let config = AppConfig::default();

        assert!(!config.avatar_proactive_ai_enabled);
    }

    #[test]
    fn 界面风格默认应保持当前柔和层次且只允许三套方案() {
        let config = AppConfig::default();

        assert_eq!(config.ui_visual_style, default_ui_visual_style());
        assert_eq!(config.ui_visual_style, "c");
        assert_eq!(normalize_ui_visual_style(" a "), "a");
        assert_eq!(normalize_ui_visual_style("B"), "b");
        assert_eq!(normalize_ui_visual_style("c"), "c");
        assert_eq!(normalize_ui_visual_style("unknown"), "c");
    }

    #[test]
    fn 日报编辑字体兼容旧配置且保存后保留选择() {
        let mut json = serde_json::to_value(AppConfig::default()).unwrap();
        assert_eq!(json["report_editor_font"], "");
        json.as_object_mut().unwrap().remove("report_editor_font");
        let mut config: AppConfig = serde_json::from_value(json).unwrap();
        assert!(config.report_editor_font.is_empty());
        config.report_editor_font = "Microsoft YaHei".into();
        let saved = serde_json::to_string(&config).unwrap();
        let loaded: AppConfig = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded.report_editor_font, "Microsoft YaHei");
    }

    #[test]
    fn 桌宠默认位置应为空以便首次按锚点吸附() {
        let config = AppConfig::default();

        assert_eq!(config.avatar_x, None);
        assert_eq!(config.avatar_y, None);
    }

    #[test]
    fn 轻量模式默认应关闭() {
        let config = AppConfig::default();

        assert!(!config.lightweight_mode);
    }

    #[test]
    fn 助手长期记忆默认应关闭() {
        let config = AppConfig::default();

        assert!(!config.assistant_memory_enabled);
    }

    #[test]
    fn 日报附加提示词默认应为空且不导出() {
        let config = AppConfig::default();

        assert!(config.daily_report_custom_prompt.is_empty());
        assert_eq!(config.daily_report_export_dir, None);
    }

    #[test]
    fn 截图模式默认应为活动窗口所在屏幕() {
        let config = AppConfig::default();

        assert_eq!(
            config.storage.screenshot_display_mode,
            ScreenshotDisplayMode::ActiveWindow
        );
    }

    #[test]
    fn 截图开关默认应开启() {
        let config = AppConfig::default();

        assert!(config.storage.screenshots_enabled);
    }

    #[test]
    fn 休息提醒默认应关闭且间隔为五十分钟() {
        let config = AppConfig::default();

        assert!(!config.break_reminder_enabled);
        assert_eq!(config.break_reminder_interval_minutes, 50);
    }

    #[test]
    fn 开机自启动默认应显示主界面() {
        let config = AppConfig::default();

        assert!(!config.auto_start_silent);
    }

    #[test]
    fn macos录屏权限提示默认不应标记为已弹出() {
        let config = AppConfig::default();

        assert!(!config.macos_screen_capture_permission_prompted);
    }

    #[test]
    fn minimax应使用_openai_兼容配置() {
        assert!(AiProvider::MiniMax.is_openai_compatible());
        assert_eq!(
            AiProvider::MiniMax.default_endpoint(),
            "https://api.minimaxi.com/v1"
        );
        assert_eq!(AiProvider::MiniMax.default_model(), "MiniMax-M3.1-Flash");
    }

    #[test]
    fn 默认模型清单应对齐当前在役官方模型() {
        assert_eq!(AiProvider::OpenAI.default_model(), "gpt-6.2-sol");
        assert_eq!(AiProvider::Gemini.default_model(), "gemini-4-hydrogen");
        assert_eq!(AiProvider::DeepSeek.default_model(), "deepseek-flash");
        assert_eq!(AiProvider::Qwen.default_model(), "qwen3.8-flash");
        assert_eq!(AiProvider::Moonshot.default_model(), "kimi-k3");
        assert_eq!(AiProvider::MiniMax.default_model(), "MiniMax-M3.1-Flash");
        assert_eq!(AiProvider::Zhipu.default_model(), "glm-5.3-flash");
        assert_eq!(AiProvider::Claude.default_model(), "claude-sonnet-5-5");
        assert_eq!(AppConfig::default().openai_model, "gpt-6.2-sol");
    }

    #[test]
    fn 桌宠缩放应被钳制在允许范围内() {
        assert_eq!(normalize_avatar_scale(0.2), 0.4);
        assert_eq!(normalize_avatar_scale(2.0), 1.3);
        assert_eq!(normalize_avatar_scale(f64::NAN), 0.9);
    }

    #[test]
    fn 桌宠透明度应被钳制在允许范围内() {
        assert_eq!(normalize_avatar_opacity(0.1), 0.45);
        assert_eq!(normalize_avatar_opacity(1.5), 1.0);
        assert_eq!(normalize_avatar_opacity(f64::NAN), 0.82);
    }

    #[test]
    fn 桌宠预设应被规范到官方预设集合内() {
        assert_eq!(normalize_avatar_preset("keyboard-focus"), "keyboard-focus");
        assert_eq!(
            normalize_avatar_preset(" minimal-office "),
            "minimal-office"
        );
        assert_eq!(normalize_avatar_preset("wild-theme"), "original-standard");
        assert_eq!(normalize_avatar_preset(""), "original-standard");
    }

    #[test]
    fn 桌宠互动风格应被规范到允许集合内() {
        assert_eq!(normalize_avatar_persona("companion"), "companion");
        assert_eq!(normalize_avatar_persona(" coach "), "coach");
        assert_eq!(normalize_avatar_persona("other"), "assistant");
    }

    #[test]
    fn 桌宠手动待跟进应去重并清理非法项() {
        let mut items = vec![
            AvatarFollowupItem {
                id: " 1 ".to_string(),
                title: " 修复支付页回调 ".to_string(),
                date: "2026-04-18".to_string(),
                source_app: "Cursor".to_string(),
                source_title: "payments.ts".to_string(),
                project_key: "payments".to_string(),
                created_at: 20,
                status: "open".to_string(),
            },
            AvatarFollowupItem {
                id: "2".to_string(),
                title: "修复支付页回调".to_string(),
                date: "2026-04-18".to_string(),
                source_app: "Cursor".to_string(),
                source_title: "payments.ts".to_string(),
                project_key: "payments".to_string(),
                created_at: 10,
                status: "open".to_string(),
            },
            AvatarFollowupItem {
                id: "3".to_string(),
                title: "   ".to_string(),
                date: "2026-04-18".to_string(),
                source_app: "Cursor".to_string(),
                source_title: "payments.ts".to_string(),
                project_key: "payments".to_string(),
                created_at: 30,
                status: "open".to_string(),
            },
        ];

        normalize_avatar_followups(&mut items);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "1");
        assert_eq!(items[0].title, "修复支付页回调");
    }

    #[test]
    fn 应用分类规则应去重并规范化分类值() {
        let mut rules = vec![
            AppCategoryRule {
                app_name: " firefox ".to_string(),
                category: "browser".to_string(),
            },
            AppCategoryRule {
                app_name: "Firefox".to_string(),
                category: "office".to_string(),
            },
            AppCategoryRule {
                app_name: "  ".to_string(),
                category: "design".to_string(),
            },
            AppCategoryRule {
                app_name: "MuMu".to_string(),
                category: "unknown".to_string(),
            },
        ];

        normalize_app_category_rules(&mut rules, &[]);

        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].app_name, "Firefox");
        assert_eq!(rules[0].category, "office");
        assert_eq!(rules[1].category, "other");
    }

    #[test]
    fn 网站语义规则应按域名去重并规范化输入() {
        let mut config = AppConfig::default();
        config.website_semantic_rules = vec![
            WebsiteSemanticRule {
                domain: " https://github.com/issues ".to_string(),
                semantic_category: " 任务规划 ".to_string(),
            },
            WebsiteSemanticRule {
                domain: "GitHub.com".to_string(),
                semantic_category: " 资料调研 ".to_string(),
            },
            WebsiteSemanticRule {
                domain: "   ".to_string(),
                semantic_category: "无效".to_string(),
            },
        ];

        config.normalize();

        assert_eq!(config.website_semantic_rules.len(), 1);
        assert_eq!(config.website_semantic_rules[0].domain, "github.com");
        assert_eq!(
            config.website_semantic_rules[0].semantic_category,
            "资料调研"
        );
    }

    #[test]
    fn 配置规范化应清理空白日报附加设置() {
        let mut config = AppConfig {
            daily_report_custom_prompt: "  输出需要更偏周报风格  ".to_string(),
            daily_report_export_dir: Some("   ".to_string()),
            ..AppConfig::default()
        };

        config.normalize();

        assert_eq!(config.daily_report_custom_prompt, "输出需要更偏周报风格");
        assert_eq!(config.daily_report_export_dir, None);
    }

    #[test]
    fn 配置规范化应清理空白远程截图公开地址() {
        let mut config = AppConfig::default();
        config.remote_storage.s3.public_url_base = Some("   ".to_string());
        config.remote_storage.webdav.public_url_base =
            Some(" https://cdn.example.com/base/ ".to_string());

        config.normalize();

        assert_eq!(config.remote_storage.s3.public_url_base, None);
        assert_eq!(
            config.remote_storage.webdav.public_url_base.as_deref(),
            Some("https://cdn.example.com/base/")
        );
    }

    #[test]
    fn 本地api默认应关闭并使用固定默认端口() {
        let config = AppConfig::default();

        assert!(!config.localhost_api_enabled);
        assert_eq!(config.localhost_api_port, DEFAULT_LOCALHOST_API_PORT);
    }

    #[test]
    fn 本地api端口应在规范化时回退到安全默认值() {
        let mut config = AppConfig {
            localhost_api_port: 0,
            ..AppConfig::default()
        };

        config.normalize();

        assert_eq!(config.localhost_api_port, DEFAULT_LOCALHOST_API_PORT);
    }

    #[test]
    fn 节点网关默认不应预填设备名() {
        let config = AppConfig::default();

        assert_eq!(config.node_gateway.device_name, None);
        assert_eq!(config.node_gateway.device_name, None);
    }

    #[test]
    fn 任务模型分流应按场景解析并在配置不完整时回退主模型() {
        let mut config = AppConfig::default();
        config.text_model = ModelConfig {
            provider: AiProvider::OpenAI,
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: Some("sk-main".to_string()),
            model: "gpt-main".to_string(),
            enable_thinking: None,
            thinking_budget: None,
            max_output_tokens: None,
        };
        config.text_model_provider_cache.insert(
            "ollama".to_string(),
            ModelConfig {
                provider: AiProvider::Ollama,
                endpoint: "http://localhost:11434".to_string(),
                api_key: None,
                model: "qwen3:8b".to_string(),
                enable_thinking: None,
                thinking_budget: None,
                max_output_tokens: None,
            },
        );
        // 缺少 API Key 的云端缓存不应被选中
        config.text_model_provider_cache.insert(
            "claude".to_string(),
            ModelConfig {
                provider: AiProvider::Claude,
                endpoint: "https://api.anthropic.com/v1".to_string(),
                api_key: None,
                model: "claude-sonnet".to_string(),
                enable_thinking: None,
                thinking_budget: None,
                max_output_tokens: None,
            },
        );
        config.task_model_routing = TaskModelRouting {
            report_provider: Some("claude".to_string()),
            assistant_provider: None,
            classification_provider: Some("ollama".to_string()),
            proactive_provider: Some("ollama".to_string()),
        };

        assert_eq!(
            config
                .resolve_model_for_task(AiTaskKind::Classification)
                .model,
            "qwen3:8b"
        );
        assert_eq!(
            config
                .resolve_model_for_task(AiTaskKind::Proactive)
                .provider,
            AiProvider::Ollama
        );
        // 未指定或缺少 Key 均回退主模型
        assert_eq!(
            config.resolve_model_for_task(AiTaskKind::Assistant).model,
            "gpt-main"
        );
        assert_eq!(
            config.resolve_model_for_task(AiTaskKind::Report).model,
            "gpt-main"
        );
    }

    #[test]
    fn 配置规范化应清理空白分流标题与重复git仓库路径() {
        let mut config = AppConfig::default();
        config.daily_report_export_section_heading = Some("  ## Work Review  ".to_string());
        config.git_author_filter = Some("   ".to_string());
        config.git_repo_paths = vec![
            "  D:\\repo-a  ".to_string(),
            "d:\\repo-a".to_string(),
            "   ".to_string(),
            "D:\\repo-b".to_string(),
        ];
        config.task_model_routing.report_provider = Some("  ".to_string());
        config.task_model_routing.classification_provider = Some(" ollama ".to_string());

        config.normalize();

        assert_eq!(
            config.daily_report_export_section_heading.as_deref(),
            Some("## Work Review")
        );
        assert_eq!(config.git_author_filter, None);
        assert_eq!(config.git_repo_paths, vec!["D:\\repo-a", "D:\\repo-b"]);
        assert_eq!(config.task_model_routing.report_provider, None);
        assert_eq!(
            config.task_model_routing.classification_provider.as_deref(),
            Some("ollama")
        );
    }
}
