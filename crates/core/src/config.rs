use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

fn default_true() -> bool {
    true
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
}

impl AiProvider {
    /// 获取提供商的显示名称
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
        }
    }

    /// 获取默认模型名称
    pub fn default_model(&self) -> &'static str {
        match self {
            AiProvider::Ollama => "qwen3",
            AiProvider::OpenAI => "gpt-5.4",
            AiProvider::Gemini => "gemini-3-flash",
            AiProvider::Claude => "claude-sonnet-4-6",
            AiProvider::SiliconFlow => "Qwen/Qwen3-8B",
            AiProvider::DeepSeek => "deepseek-chat",
            AiProvider::Qwen => "qwen-flash",
            AiProvider::Zhipu => "glm-5-turbo",
            AiProvider::Moonshot => "moonshot-v1-8k",
            AiProvider::Doubao => "doubao-lite-4k",
            AiProvider::MiniMax => "MiniMax-M2.5",
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
        }
    }

    /// 创建默认的视觉模型配置
    pub fn default_vision() -> Self {
        Self {
            provider: AiProvider::Ollama,
            endpoint: AiProvider::Ollama.default_endpoint().to_string(),
            api_key: None,
            model: "llava".to_string(),
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
            model: AiProvider::Ollama.default_model().to_string(),
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
            filter_sensitive: true, // 已弃用，保留兼容
            excluded_apps: vec![],
        }
    }
}

impl PrivacyConfig {
    /// 获取应用的隐私级别
    pub fn get_app_privacy_level(&self, app_name: &str) -> PrivacyLevel {
        let normalized = crate::categorize::normalize_display_app_name(app_name).to_lowercase();
        // 先检查新的规则（规范化后精确匹配 + 包含匹配）
        for rule in &self.app_rules {
            let rule_normalized = crate::categorize::normalize_display_app_name(&rule.app_name).to_lowercase();
            // 精确匹配或包含匹配（应用名包含规则名，处理 "Google Chrome" 包含 "Chrome" 的情况）
            if normalized == rule_normalized || normalized.contains(&rule_normalized) {
                log::debug!(
                    "应用 {} 匹配规则 {}, 级别: {:?}",
                    app_name,
                    rule.app_name,
                    rule.level
                );
                return rule.level;
            }
        }
        // 兼容旧版 excluded_apps（视为 Ignored）
        for excluded in &self.excluded_apps {
            let excluded_normalized = crate::categorize::normalize_display_app_name(excluded).to_lowercase();
            if normalized.contains(&excluded_normalized) {
                return PrivacyLevel::Ignored;
            }
        }
        PrivacyLevel::Full
    }

    /// 检查窗口标题是否触发隐私保护
    pub fn should_anonymize_by_keyword(&self, window_title: &str) -> bool {
        let title_lower = window_title.to_lowercase();
        self.excluded_keywords
            .iter()
            .any(|k| title_lower.contains(&k.to_lowercase()))
    }

    /// 从 URL 中提取域名
    pub fn extract_domain(url: &str) -> String {
        let without_protocol = url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        without_protocol
            .split('/')
            .next()
            .unwrap_or("")
            .to_lowercase()
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

    /// 收集所有应忽略的应用名（小写），包含 app_rules 和遗留 excluded_apps
    pub fn collect_ignored_app_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .app_rules
            .iter()
            .filter(|rule| rule.level == PrivacyLevel::Ignored)
            .map(|rule| rule.app_name.to_lowercase())
            .collect();
        for app in &self.excluded_apps {
            let lower = app.to_lowercase();
            if !names.contains(&lower) {
                names.push(lower);
            }
        }
        names
    }

    /// 收集所有域名黑名单（已提取域名、已去空）
    pub fn collect_excluded_domains(&self) -> Vec<String> {
        self.excluded_domains
            .iter()
            .map(|d| Self::extract_domain(d))
            .filter(|d| !d.is_empty())
            .collect()
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
    /// 元数据保留天数（超过后删除数据库记录）
    pub metadata_retention_days: u32,
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
            metadata_retention_days: 30,  // 默认保留30天元数据
            storage_limit_mb: 2048,       // 默认2GB上限
            jpeg_quality: 85,             // 85%质量，更清晰
            max_image_width: 1280,        // 最大宽度1280px
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

/// 应用配置
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    /// 截屏间隔（秒）
    pub screenshot_interval: u64,
    /// 空闲检测阈值（分钟）：超过此时间无键鼠输入则进入疑似空闲
    #[serde(default = "default_idle_threshold_minutes")]
    pub idle_threshold_minutes: u32,
    /// AI分析模式
    pub ai_mode: AiMode,
    /// 文本模型配置
    #[serde(default = "ModelConfig::default_text")]
    pub text_model: ModelConfig,
    /// 可保存的文本模型档案
    #[serde(default)]
    pub text_model_profiles: Vec<TextModelProfile>,
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
    /// 存储配置
    #[serde(default)]
    pub storage: StorageConfig,
    /// 远程存储配置（S3/MinIO 或 WebDAV）
    #[serde(default)]
    pub remote_storage: RemoteStorageConfig,
    /// 日报附加提示词
    #[serde(default)]
    pub daily_report_custom_prompt: String,
    /// 日报提示词预设模板列表
    #[serde(default)]
    pub daily_report_prompt_presets: Vec<PromptPreset>,
    /// 日报 Markdown 导出目录
    #[serde(default)]
    pub daily_report_export_dir: Option<String>,
    /// 日报自动生成后是否自动导出 Markdown
    #[serde(default)]
    pub daily_report_auto_export: bool,
    /// 日报自动生成时间 (HH:MM)，为空时使用工作结束时间
    #[serde(default)]
    pub daily_report_auto_generate_time: Option<String>,
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
    /// 是否启用 MCP Server
    #[serde(default)]
    pub mcp_server_enabled: bool,
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
    /// 是否启用桌面化身窗口
    #[serde(default)]
    pub avatar_enabled: bool,
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
fn default_avatar_opacity() -> f64 {
    0.82
}
fn default_avatar_preset() -> String {
    "original-standard".to_string()
}
fn default_avatar_persona() -> String {
    "assistant".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            screenshot_interval: 30,
            idle_threshold_minutes: default_idle_threshold_minutes(),
            ai_mode: AiMode::Local,
            text_model: ModelConfig::default_text(),
            text_model_profiles: Vec::new(),
            vision_model: ModelConfig::default_vision(),
            privacy: PrivacyConfig::default(),
            app_category_rules: Vec::new(),
            custom_categories: Vec::new(),
            website_semantic_rules: Vec::new(),
            custom_semantic_categories: Vec::new(),
            deleted_default_categories: Vec::new(),
            deleted_default_semantic_categories: Vec::new(),
            storage: StorageConfig::default(),
            remote_storage: RemoteStorageConfig::default(),
            daily_report_custom_prompt: String::new(),
            daily_report_prompt_presets: Vec::new(),
            daily_report_export_dir: None,
            daily_report_auto_export: false,
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
            mcp_server_enabled: false,
            node_devices: Vec::new(),
            node_gateway: NodeGatewayConfig::default(),
            auto_start: false,
            auto_start_silent: false,
            macos_screen_capture_permission_prompted: false,
            last_app_version: None,
            theme: "system".to_string(),
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
            // 旧版兼容字段
            ai_provider: AiProviderConfig::default(),
            ollama_host: "http://localhost:11434".to_string(),
            ollama_model: "llava".to_string(),
            openai_api_key: None,
            openai_model: "gpt-5.4".to_string(),
            hide_dock_icon: false,
            lightweight_mode: false,
            break_reminder_enabled: false,
            break_reminder_interval_minutes: default_break_reminder_interval_minutes(),
            avatar_enabled: false,
            avatar_scale: default_avatar_scale(),
            avatar_opacity: default_avatar_opacity(),
            avatar_preset: default_avatar_preset(),
            avatar_persona: default_avatar_persona(),
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

impl AppConfig {
    /// 规范化配置，兼容旧字段并补齐助手可用的文本模型档案
    pub fn normalize(&mut self) {
        self.migrate_legacy_config();
        normalize_custom_categories(&mut self.custom_categories);
        normalize_app_category_rules(&mut self.app_category_rules, &self.custom_categories);
        // Seed defaults BEFORE rule validation so built-in categories are present
        seed_default_categories(&mut self.custom_categories, &self.deleted_default_categories);
        seed_default_semantic_categories(&mut self.custom_semantic_categories, &self.deleted_default_semantic_categories);
        normalize_custom_semantic_categories(&mut self.custom_semantic_categories);
        normalize_website_semantic_rules(
            &mut self.website_semantic_rules,
            &self.custom_semantic_categories,
        );
        self.screenshot_interval = normalize_screenshot_interval(self.screenshot_interval);
        self.idle_threshold_minutes = normalize_idle_threshold_minutes(self.idle_threshold_minutes);
        self.avatar_scale = normalize_avatar_scale(self.avatar_scale);
        self.avatar_opacity = normalize_avatar_opacity(self.avatar_opacity);
        self.avatar_preset = normalize_avatar_preset(&self.avatar_preset);
        self.avatar_persona = normalize_avatar_persona(&self.avatar_persona);
        normalize_avatar_followups(&mut self.avatar_followups);
        self.break_reminder_interval_minutes =
            normalize_break_reminder_interval_minutes(self.break_reminder_interval_minutes);
        self.daily_report_custom_prompt = self.daily_report_custom_prompt.trim().to_string();
        normalize_prompt_presets(&mut self.daily_report_prompt_presets);
        self.daily_report_export_dir =
            normalize_optional_string(self.daily_report_export_dir.take());
        self.localhost_api_port = normalize_localhost_api_port(self.localhost_api_port);
        self.localhost_api_host = normalize_optional_string(self.localhost_api_host.take());
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
            let content = std::fs::read_to_string(path)?;
            let mut config: AppConfig = serde_json::from_str(&content)?;

            config.normalize();

            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// 保存配置到文件
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }
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
            };
        }

        if self.vision_model.model.is_empty() {
            if let Some(ref vision_model) = self.ai_provider.vision_model {
                self.vision_model = ModelConfig {
                    provider: self.ai_provider.provider,
                    endpoint: self.ai_provider.endpoint.clone(),
                    api_key: self.ai_provider.api_key.clone(),
                    model: vision_model.clone(),
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

fn same_model_config(left: &ModelConfig, right: &ModelConfig) -> bool {
    left.provider == right.provider
        && left.endpoint.trim() == right.endpoint.trim()
        && left.model.trim() == right.model.trim()
        && left.api_key.as_deref().unwrap_or("").trim()
            == right.api_key.as_deref().unwrap_or("").trim()
}

fn default_profile_name(model_config: &ModelConfig) -> String {
    format!(
        "{} · {}",
        model_config.provider.display_name(),
        model_config.model.trim()
    )
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

fn seed_default_semantic_categories(categories: &mut Vec<CustomSemanticCategory>, deleted: &[String]) {
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

    value.clamp(0.7, 1.3)
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
    use super::{
        default_avatar_opacity, default_avatar_persona, default_avatar_preset,
        default_avatar_scale, normalize_app_category_rules, normalize_avatar_followups,
        normalize_avatar_opacity, normalize_avatar_persona, normalize_avatar_preset,
        normalize_avatar_scale, AiProvider, AppCategoryRule, AppConfig, AvatarFollowupItem,
        ScreenshotDisplayMode, WebsiteSemanticRule, DEFAULT_LOCALHOST_API_PORT,
    };

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
        assert_eq!(AiProvider::MiniMax.default_model(), "MiniMax-M2.5");
    }

    #[test]
    fn 桌宠缩放应被钳制在允许范围内() {
        assert_eq!(normalize_avatar_scale(0.3), 0.7);
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
        config.remote_storage.webdav.public_url_base = Some(" https://cdn.example.com/base/ ".to_string());

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
}
