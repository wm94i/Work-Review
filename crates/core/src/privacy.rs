use crate::config::{AppConfig, PrivacyConfig, PrivacyLevel, UnknownUrlFallback};
use crate::database::{Activity, DailyStats, MemorySearchItem};
use once_cell::sync::Lazy;
use regex::Regex;

/// 隐私检查结果
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrivacyAction {
    /// 正常记录（截图 + 统计）
    Record,
    /// 内容脱敏（只统计时长，不保存截图）
    Anonymize,
    /// 完全跳过（不记录任何信息）
    Skip,
}

/// 预编译的通用敏感凭据/令牌正则（避免每次 `PrivacyFilter::from_config` 重复编译）。
/// 先于数字类规则执行，避免令牌中的数字段被手机号/卡号拆碎残留。
static DIRECT_SECRET_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        // 多行 PEM 私钥块（含正文与尾标）
        r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
        // HTTP Bearer 令牌
        r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{16,}",
        // OpenAI / Anthropic 等 sk- 前缀密钥
        r"\bsk-[A-Za-z0-9_-]{16,}",
        // GitHub / GitLab 令牌
        r"\bgh[pousr]_[A-Za-z0-9]{20,}",
        r"\bgithub_pat_[A-Za-z0-9_]{20,}",
        r"\bglpat-[A-Za-z0-9_-]{16,}",
        // AWS Access Key ID
        r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        // Slack 令牌
        r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
        r"\bxapp-\d-[A-Za-z0-9-]{16,}",
        // Google API / OAuth 令牌
        r"\bAIza[0-9A-Za-z_-]{35}\b",
        r"\bya29\.[0-9A-Za-z_-]{20,}",
        // JWT
        r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("内置密钥正则必须可编译"))
    .collect()
});

/// 银行卡/信用卡（16-19 位连续或 4 位分组；手动校验非数字边界以兼容中文紧邻场景）
static CREDIT_CARD_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{0,3}")
        .expect("银行卡正则必须可编译")
});
/// 身份证号（18 位；先于银行卡与手机号执行，避免末位 X 被截成 17 位银行卡+X 或被手机号截断）
static ID_CARD_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[1-9]\d{5}(?:18|19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dXx]")
        .expect("身份证正则必须可编译")
});
/// 手机号（11 位；手动校验非数字边界以兼容 `手机13812345678`）
static PHONE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"1[3-9]\d{9}").expect("手机号正则必须可编译"));
/// 邮箱（修复 `[A-Z|a-z]` 笔误；手动校验前导非邮箱字符以兼容 `邮箱a@b.com`）
static EMAIL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").expect("邮箱正则必须可编译")
});
/// 密码 / API Key / Secret / Token 键值（兼容普通分隔符与 JSON 引号 `{"password": "..."}`）
static KEY_VALUE_SECRET_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r#"(?i)(?:password|密码|pwd|passwd)["']?[\s:：=]+["']?[^\s"',;}\]]+"#,
        r#"(?i)(?:[A-Z0-9_]*(?:api[_-]?key|secret(?:[_-]?(?:access[_-]?)?key)?|access[_-]?token|auth[_-]?token)|token)["']?[\s:：=]+["']?[^\s"',;}\]]+"#,
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("键值敏感正则必须可编译"))
    .collect()
});

fn replace_with_non_digit_boundary(text: &str, re: &Regex, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_end = 0;
    for m in re.find_iter(text) {
        let prev_is_digit = text[..m.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit());
        let next_is_digit = text[m.end()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        if prev_is_digit || next_is_digit {
            continue;
        }
        out.push_str(&text[last_end..m.start()]);
        out.push_str(replacement);
        last_end = m.end();
    }
    out.push_str(&text[last_end..]);
    out
}

fn replace_emails_cjk_safe(text: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_end = 0;
    for m in EMAIL_PATTERN.find_iter(text) {
        let prev_is_email_char = text[..m.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'));
        let next_is_domain_char = text[m.end()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-');
        if prev_is_email_char || next_is_domain_char {
            continue;
        }
        out.push_str(&text[last_end..m.start()]);
        out.push_str(replacement);
        last_end = m.end();
    }
    out.push_str(&text[last_end..]);
    out
}

/// 根据浏览器原始窗口标题检测无痕 / 隐私浏览窗口（多语言样本，必须在标题清洗前调用）。
///
/// 为避免普通网页标题碰巧包含 "incognito" 等词造成误判，仅在浏览器应用上检测
/// 典型括号标记、尾段标记或浏览器专有无痕前缀/后缀。
pub fn is_private_browser_window_title(app_name: &str, raw_window_title: &str) -> bool {
    if !crate::categorize::is_browser_app(app_name) {
        return false;
    }
    let trimmed = raw_window_title.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();

    // 整标题或分隔符尾段匹配（兼容 " - ", " — ", " – " 分隔的 Firefox / Edge / Chrome / Safari）
    let segments: Vec<&str> = lower
        .split(" - ")
        .flat_map(|s| s.split(" — "))
        .flat_map(|s| s.split(" – "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    // 明确的括号/方括号无痕后缀（Chrome / Edge / Safari / Brave / Vivaldi / Opera / Firefox 多语言）
    // 仅匹配浏览器标题尾段或独立标记段，避免正文页面标题包含 "(Incognito)" 时误判为无痕窗口。
    const BRACKETED_MARKERS: &[&str] = &[
        "(incognito)",
        "[incognito]",
        "(inprivate)",
        "[inprivate]",
        "(private browsing)",
        "(private window)",
        "(private)",
        "(无痕)",
        "（无痕）",
        "(无痕模式)",
        "（无痕模式）",
        "(無痕)",
        "（無痕）",
        "(無痕模式)",
        "（無痕模式）",
        "(隐私浏览)",
        "（隐私浏览）",
        "(隱私瀏覽)",
        "（隱私瀏覽）",
        "(私密浏览)",
        "（私密浏览）",
        "(私密瀏覽)",
        "（私密瀏覽）",
        "(隐私窗口)",
        "（隐私窗口）",
        "(隱私視窗)",
        "（隱私視窗）",
        "(私密窗口)",
        "（私密窗口）",
        "(私密視窗)",
        "（私密視窗）",
        "(navigation privée)",
        "(privates fenster)",
        "(navegación privada)",
        "(シークレット)",
        "（シークレット）",
        "(プライベート)",
        "（プライベート）",
    ];
    if let Some(last) = segments.last() {
        if BRACKETED_MARKERS.iter().any(|m| last.contains(m)) {
            return true;
        }
    }
    if segments.len() >= 2 {
        let prev = segments[segments.len() - 2];
        if BRACKETED_MARKERS.contains(&prev)
            || (segments.len() >= 3 && BRACKETED_MARKERS.iter().any(|m| prev.ends_with(m)))
        {
            return true;
        }
    }

    const TRAILING_SEGMENT_MARKERS: &[&str] = &[
        "incognito",
        "inprivate",
        "private browsing",
        "private window",
        "private window with tor",
        "无痕模式",
        "無痕模式",
        "隐私浏览",
        "隱私瀏覽",
        "私密浏览",
        "私密瀏覽",
        "隐私窗口",
        "隱私視窗",
        "私密窗口",
        "私密視窗",
        "navigation privée",
        "privates fenster",
        "navegación privada",
        "シークレット",
        "プライベート ブラウジング",
        "プライベートブラウジング",
        "تصفح متخف",
        "نافذة خاصة",
        "التصفح المتخفي",
    ];

    if let Some(last) = segments.last() {
        if TRAILING_SEGMENT_MARKERS.iter().any(|marker| {
            *last == *marker
                || last.ends_with(marker)
                || last.starts_with(marker)
                || last.contains(&format!(" {marker}"))
                || last.contains(&format!("{marker} "))
        }) {
            return true;
        }
    }
    if segments.len() == 1 {
        let only = segments[0];
        if only == "inprivate"
            || only.starts_with("inprivate ")
            || only.contains("private browsing")
            || only.contains("无痕")
            || only.contains("無痕")
            || only.contains("隐私浏览")
            || only.contains("隱私瀏覽")
            || only.contains("私密浏览")
            || only.contains("私密瀏覽")
            || only.contains("私密窗口")
            || only.contains("隐私窗口")
            || only.contains("تصفح متخف")
            || only.contains("نافذة خاصة")
        {
            return true;
        }
    }
    // 浏览器倒数第二段偶尔在 profile 名之前（如 "页面 - Google Chrome (无痕) - Profile 1" 或 "页面 - InPrivate - Microsoft Edge"）
    if segments.len() >= 2 {
        let prev = segments[segments.len() - 2];
        if TRAILING_SEGMENT_MARKERS.contains(&prev) {
            return true;
        }
    }

    false
}

/// 隐私过滤器
pub struct PrivacyFilter {
    config: PrivacyConfig,
}

impl PrivacyFilter {
    /// 从配置创建隐私过滤器
    pub fn from_config(config: &PrivacyConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    /// 检查应用和窗口的隐私操作
    /// 返回应该采取的隐私行动
    pub fn check_privacy(&self, app_name: &str, window_title: &str) -> PrivacyAction {
        // 1. 先检查应用级别的隐私规则
        let app_level = self.config.get_app_privacy_level(app_name);

        match app_level {
            PrivacyLevel::Ignored => {
                log::debug!("应用 {app_name} 设置为完全忽略");
                return PrivacyAction::Skip;
            }
            PrivacyLevel::Anonymized => {
                log::debug!("应用 {app_name} 设置为内容脱敏");
                return PrivacyAction::Anonymize;
            }
            PrivacyLevel::Full => {
                // 继续检查窗口标题
            }
        }

        // 2. 检查窗口标题关键词（匹配时使用脱敏模式）
        if self.config.should_anonymize_by_keyword(window_title) {
            log::debug!("窗口标题 {window_title} 匹配敏感关键词，使用脱敏模式");
            return PrivacyAction::Anonymize;
        }

        PrivacyAction::Record
    }

    /// 检查 URL 域名是否在黑名单中
    /// 如果匹配，返回 Skip；否则返回 Record。
    /// 复用统计侧的 `matches_excluded_domain`（含合并域名兼容匹配）——
    /// 此前采集侧只做精确/后缀匹配,比统计侧窄:统计里被隐藏的域名
    /// 实际仍在被截图与 OCR。两侧必须是同一套语义。
    pub fn check_url_privacy(&self, url: Option<&str>) -> PrivacyAction {
        if let Some(url) = url {
            if !url.is_empty() && matches_excluded_domain(url, &self.config.excluded_domains) {
                log::debug!("URL 命中排除域名名单，跳过记录");
                return PrivacyAction::Skip;
            }
        }
        PrivacyAction::Record
    }

    /// 综合检查（含无痕窗口标记）：无痕窗口跳过 > 应用/标题规则 > 排除域名 > 未知 URL 降级。
    pub fn check_privacy_with_private(
        &self,
        app_name: &str,
        window_title: &str,
        browser_url: Option<&str>,
        is_private: bool,
    ) -> PrivacyAction {
        // 0. 无痕 / 隐私浏览器窗口默认跳过
        if self.config.skip_private_windows
            && (is_private || is_private_browser_window_title(app_name, window_title))
        {
            log::debug!("无痕浏览器窗口 {app_name} 触发自动跳过");
            return PrivacyAction::Skip;
        }

        // 1. 先检查应用和窗口标题
        let app_action = self.check_privacy(app_name, window_title);
        if app_action == PrivacyAction::Skip {
            return PrivacyAction::Skip;
        }

        // 2. 检查 URL 域名黑名单
        let url_action = self.check_url_privacy(browser_url);
        if url_action == PrivacyAction::Skip {
            return PrivacyAction::Skip;
        }

        // 3. 浏览器 URL 无法识别时的降级策略（配置排除域名时默认 Anonymize，防止绕过域名过滤）
        if app_action == PrivacyAction::Record && crate::categorize::is_browser_app(app_name) {
            let domain_known = browser_url
                .map(PrivacyConfig::extract_domain)
                .is_some_and(|d| !d.is_empty());
            if !domain_known
                && self.config.effective_unknown_url_fallback() == UnknownUrlFallback::Anonymize
            {
                log::debug!("浏览器 {app_name} 未识别到有效 URL，按 unknown_url_fallback 降级脱敏");
                return PrivacyAction::Anonymize;
            }
        }

        // 4. 返回应用级别的结果（可能是 Record 或 Anonymize）
        app_action
    }

    /// 综合检查：应用 + 窗口标题 + URL
    pub fn check_privacy_full(
        &self,
        app_name: &str,
        window_title: &str,
        browser_url: Option<&str>,
    ) -> PrivacyAction {
        self.check_privacy_with_private(app_name, window_title, browser_url, false)
    }

    /// 兼容旧接口：检查是否应该跳过
    pub fn should_skip(&self, app_name: &str, window_title: &str) -> bool {
        self.check_privacy(app_name, window_title) == PrivacyAction::Skip
    }

    /// 过滤文本中的敏感信息（密钥/令牌、银行卡、身份证、手机号、邮箱、密码/凭据键值）
    pub fn filter_text(&self, text: &str) -> String {
        let mut filtered = text.to_string();

        for pattern in DIRECT_SECRET_PATTERNS.iter() {
            filtered = pattern.replace_all(&filtered, "[已过滤]").to_string();
        }
        // 长数字优先：先 18 位身份证（含末位 X），再 16-19 位银行卡，最后 11 位手机号
        filtered = replace_with_non_digit_boundary(&filtered, &ID_CARD_PATTERN, "[已过滤]");
        filtered = replace_with_non_digit_boundary(&filtered, &CREDIT_CARD_PATTERN, "[已过滤]");
        filtered = replace_with_non_digit_boundary(&filtered, &PHONE_PATTERN, "[已过滤]");
        filtered = replace_emails_cjk_safe(&filtered, "[已过滤]");
        for pattern in KEY_VALUE_SECRET_PATTERNS.iter() {
            filtered = pattern.replace_all(&filtered, "[已过滤]").to_string();
        }

        filtered
    }

    /// 更新配置
    pub fn update_config(&mut self, config: &PrivacyConfig) {
        self.config = config.clone();
    }
}

/// 判断 app 是否命中忽略名单。匹配前先经过 `normalize_display_app_name` 归一化，
/// 再做大小写不敏感的单向子串匹配（应用名包含忽略条目才算命中；不做反向包含，
/// 避免忽略 "Google Chrome Canary" 时误伤 "Google Chrome"）。
pub fn matches_ignored_app(app_name: &str, ignored_apps: &[String]) -> bool {
    let app_lower = crate::categorize::normalize_display_app_name(app_name)
        .to_lowercase()
        .trim()
        .to_string();
    if app_lower.is_empty() {
        return false;
    }

    ignored_apps.iter().any(|ignored| {
        let ignored_norm = crate::categorize::normalize_display_app_name(ignored)
            .to_lowercase()
            .trim()
            .to_string();
        !ignored_norm.is_empty() && app_lower.contains(&ignored_norm)
    })
}

/// 判断目标 URL/域名是否命中排除域名名单。先提取域名，再做精确后缀匹配或
/// 合并域名（merged domain）的兼容匹配。
pub fn matches_excluded_domain(target: &str, excluded_domains: &[String]) -> bool {
    let domain = PrivacyConfig::extract_domain(target);
    if domain.is_empty() {
        return false;
    }

    excluded_domains.iter().any(|excluded| {
        let excluded_domain = PrivacyConfig::extract_domain(excluded);
        !excluded_domain.is_empty()
            && (PrivacyConfig::domain_matches(&domain, &excluded_domain)
                || merged_domain_matches_excluded(&domain, &excluded_domain))
    })
}

/// 合并域名（merged domain）兼容匹配：当目标域名本身是一个「合并入口」
/// （如 `github.com` 被合并成 `github.io` 之类的二级入口）时，允许与排除名单中
/// 的同类入口做前缀匹配。
pub fn merged_domain_matches_excluded(domain: &str, excluded_domain: &str) -> bool {
    if !crate::categorize::is_merged_domain(domain) {
        return false;
    }

    let domain = domain.trim_end_matches('.').to_lowercase();
    let excluded_domain = excluded_domain.trim_end_matches('.').to_lowercase();
    let domain_labels: Vec<&str> = domain.split('.').collect();
    let excluded_labels: Vec<&str> = excluded_domain.split('.').collect();

    domain_labels.len() == 2
        && excluded_labels.len() == 2
        && domain_labels[0] == excluded_labels[0]
        && domain_labels[1].starts_with(excluded_labels[1])
        && domain_labels[1].len() > excluded_labels[1].len()
}

/// 对外输出时替换脱敏活动窗口标题所用的占位文本。
pub const ANONYMIZED_TITLE_PLACEHOLDER: &str = "[内容已脱敏]";

/// 对外输出 URL 清洗：只保留 `http` / `https` 协议，移除 `userinfo@`、查询参数（`?...`）与片段（`#...`），
/// 防止对外导出 / MCP / 助手结果泄露令牌、会话 ID 或签名参数。
pub fn sanitize_url_for_external(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let (scheme, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http", rest)
    } else if lower.starts_with("https://") {
        ("https", &trimmed["https://".len()..])
    } else if lower.starts_with("http://") {
        ("http", &trimmed["http://".len()..])
    } else {
        return None;
    };

    let without_fragment = rest.split('#').next().unwrap_or("");
    let without_query = without_fragment.split('?').next().unwrap_or("");
    let (authority, path) = match without_query.find('/') {
        Some(idx) => (&without_query[..idx], &without_query[idx..]),
        None => (without_query, ""),
    };
    let host_port = authority.rsplit('@').next().unwrap_or("").trim();
    if host_port.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host_port}{path}"))
}

fn screenshot_url_has_sensitive_parts(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return true;
    }
    let rest = if lower.starts_with("https://") {
        &trimmed["https://".len()..]
    } else {
        &trimmed["http://".len()..]
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.contains('@') || rest.contains('?') || rest.contains('#')
}

/// 对外输出（MCP / 本地 API / Agent 工具结果 / AI 日报上下文）前按**当前**隐私规则处理单条活动。
///
/// - 命中"脱敏"（应用级别或窗口标题关键词）的活动：清空标题、URL、OCR、可执行文件路径、远程截图 URL。
///   这样在采集之后才新增的脱敏规则也会作用于历史记录的外发。
/// - 普通记录也会对 `browser_url` 剥离用户信息、查询参数与片段，并移除含签名参数/凭据的 `screenshot_url`。
/// - `include_ocr == false` 时一律清空 OCR 原文。
///
/// "忽略应用 / 排除域名"由调用方先行剔除整条记录，这里不处理。
pub fn sanitize_activity_for_external(
    filter: &PrivacyFilter,
    activity: &mut Activity,
    include_ocr: bool,
) {
    let action = filter.check_privacy_full(
        &activity.app_name,
        &activity.window_title,
        activity.browser_url.as_deref(),
    );
    if action != PrivacyAction::Record {
        activity.window_title = ANONYMIZED_TITLE_PLACEHOLDER.to_string();
        activity.browser_url = None;
        activity.executable_path = None;
        activity.screenshot_path.clear();
        activity.screenshot_url = None;
        activity.ocr_text = None;
    } else {
        activity.window_title = filter.filter_text(&activity.window_title);
        activity.browser_url = activity
            .browser_url
            .as_deref()
            .and_then(sanitize_url_for_external);
        if activity
            .screenshot_url
            .as_deref()
            .is_some_and(screenshot_url_has_sensitive_parts)
        {
            activity.screenshot_url = None;
        }
        if include_ocr {
            activity.ocr_text = activity.ocr_text.as_deref().map(|t| filter.filter_text(t));
        }
    }
    if !include_ocr {
        activity.ocr_text = None;
    }
}

/// 对外输出记忆检索结果前按当前脱敏规则处理：脱敏记录清空标题、网址与摘要（摘要可能来自 OCR），
/// 普通记录对标题与摘要执行敏感信息过滤，并对 `browser_url` 剥离用户信息、查询参数与片段。
pub fn sanitize_memory_item_for_external(filter: &PrivacyFilter, item: &mut MemorySearchItem) {
    let action = filter.check_privacy_full(
        item.app_name.as_deref().unwrap_or(""),
        &item.title,
        item.browser_url.as_deref(),
    );
    if action != PrivacyAction::Record {
        item.title = ANONYMIZED_TITLE_PLACEHOLDER.to_string();
        item.browser_url = None;
        item.excerpt = String::new();
    } else {
        item.title = filter.filter_text(&item.title);
        item.browser_url = item
            .browser_url
            .as_deref()
            .and_then(sanitize_url_for_external);
        if !item.excerpt.is_empty() {
            item.excerpt = filter.filter_text(&item.excerpt);
        }
    }
}

/// 从 `AppConfig` 收集隐私过滤所需的两组名单：
/// 返回 `(ignored_apps, excluded_domains)`。
pub fn collect_privacy_filters(config: &AppConfig) -> (Vec<String>, Vec<String>) {
    (
        config.privacy.collect_ignored_app_names(),
        config.privacy.collect_excluded_domains(),
    )
}

/// 把忽略应用名单应用到 `DailyStats`：移除命中名单的应用/浏览器，并重算合计时长。
pub fn apply_ignored_apps_to_stats(mut stats: DailyStats, ignored_apps: &[String]) -> DailyStats {
    if ignored_apps.is_empty() {
        return stats;
    }

    let filtered_app_usage: Vec<_> = stats
        .app_usage
        .into_iter()
        .filter(|app| !matches_ignored_app(&app.app_name, ignored_apps))
        .collect();

    stats.total_duration = filtered_app_usage.iter().map(|app| app.duration).sum();
    stats.app_usage = filtered_app_usage;
    stats
        .browser_usage
        .retain(|browser| !matches_ignored_app(&browser.browser_name, ignored_apps));
    stats.browser_duration = stats
        .browser_usage
        .iter()
        .map(|browser| browser.duration)
        .sum();

    if stats.work_time_duration > stats.total_duration {
        stats.work_time_duration = stats.total_duration;
    }

    stats
}

/// 把排除域名名单应用到 `DailyStats`：移除命中名单的 URL/域名/浏览器子域名，
/// 重算浏览器时长并丢弃时长归零的浏览器。
pub fn apply_excluded_domains_to_stats(
    mut stats: DailyStats,
    excluded_domains: &[String],
) -> DailyStats {
    if excluded_domains.is_empty() {
        return stats;
    }

    stats
        .url_usage
        .retain(|url| !matches_excluded_domain(&url.domain, excluded_domains));
    stats
        .domain_usage
        .retain(|domain| !matches_excluded_domain(&domain.domain, excluded_domains));

    for browser in &mut stats.browser_usage {
        browser
            .domains
            .retain(|domain| !matches_excluded_domain(&domain.domain, excluded_domains));
        browser.duration = browser.domains.iter().map(|domain| domain.duration).sum();
    }
    stats.browser_usage.retain(|browser| browser.duration > 0);
    stats.browser_usage.sort_by(|left, right| {
        right
            .duration
            .cmp(&left.duration)
            .then_with(|| left.browser_name.cmp(&right.browser_name))
    });
    stats.browser_duration = stats
        .browser_usage
        .iter()
        .map(|browser| browser.duration)
        .sum();

    stats
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    use crate::config::AppPrivacyRule;

    fn external_activity(app_name: &str, title: &str) -> Activity {
        Activity {
            id: Some(1),
            timestamp: 1_700_000_000,
            app_name: app_name.to_string(),
            window_title: title.to_string(),
            screenshot_path: "screenshots/2026-11-22/120000_000.jpg".to_string(),
            ocr_text: Some("屏幕上的内容".to_string()),
            category: "development".to_string(),
            duration: 30,
            browser_url: Some("https://example.com/page".to_string()),
            executable_path: Some("C:\\Apps\\app.exe".to_string()),
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: Some("https://cdn.example/shot.jpg".to_string()),
        }
    }

    #[test]
    fn 对外输出时脱敏应用应清空标题网址与ocr() {
        let mut config = PrivacyConfig::default();
        config.app_rules.push(AppPrivacyRule {
            app_name: "Slack".to_string(),
            level: PrivacyLevel::Anonymized,
        });
        let filter = PrivacyFilter::from_config(&config);
        let mut activity = external_activity("Slack", "#secret-channel");
        sanitize_activity_for_external(&filter, &mut activity, true);
        assert_eq!(activity.window_title, ANONYMIZED_TITLE_PLACEHOLDER);
        assert!(activity.browser_url.is_none());
        assert!(activity.ocr_text.is_none());
        assert!(activity.executable_path.is_none());
        assert!(activity.screenshot_path.is_empty());
        assert!(activity.screenshot_url.is_none());
    }

    #[test]
    fn 对外输出时普通应用按开关决定是否保留ocr() {
        let filter = PrivacyFilter::from_config(&PrivacyConfig::default());
        let mut with_ocr = external_activity("Code", "main.rs");
        sanitize_activity_for_external(&filter, &mut with_ocr, true);
        assert_eq!(with_ocr.window_title, "main.rs");
        assert!(with_ocr.ocr_text.is_some());

        let mut without_ocr = external_activity("Code", "main.rs");
        sanitize_activity_for_external(&filter, &mut without_ocr, false);
        assert_eq!(without_ocr.window_title, "main.rs");
        assert!(without_ocr.ocr_text.is_none());
        assert!(without_ocr.browser_url.is_some());

        // 对外 URL 清洗必须剥离 userinfo、查询参数、片段，并移除含签名参数的 screenshot_url
        let mut with_sensitive_urls = external_activity("Code", "main.rs");
        with_sensitive_urls.browser_url =
            Some("https://user:pass@docs.example.com/guide?token=secret123#sec".to_string());
        with_sensitive_urls.screenshot_url =
            Some("https://s3.example.com/shot.jpg?X-Amz-Signature=deadbeef".to_string());
        sanitize_activity_for_external(&filter, &mut with_sensitive_urls, false);
        assert_eq!(
            with_sensitive_urls.browser_url.as_deref(),
            Some("https://docs.example.com/guide")
        );
        assert!(with_sensitive_urls.screenshot_url.is_none());
    }

    #[test]
    fn 对外输出时标题命中敏感关键词也应脱敏() {
        let mut config = PrivacyConfig::default();
        config.excluded_keywords.push("工资单".to_string());
        let filter = PrivacyFilter::from_config(&config);
        let mut activity = external_activity("Excel", "2026 工资单.xlsx");
        sanitize_activity_for_external(&filter, &mut activity, true);
        assert_eq!(activity.window_title, ANONYMIZED_TITLE_PLACEHOLDER);
        assert!(activity.ocr_text.is_none());
    }

    #[test]
    fn test_privacy_levels() {
        let mut config = PrivacyConfig::default();
        config.app_rules.push(AppPrivacyRule {
            app_name: "WeChat".to_string(),
            level: PrivacyLevel::Anonymized,
        });

        let filter = PrivacyFilter::from_config(&config);

        // 完全忽略
        assert_eq!(
            filter.check_privacy("1Password", "Main"),
            PrivacyAction::Skip
        );

        // 内容脱敏（通过规则）
        assert_eq!(
            filter.check_privacy("WeChat", "Chat"),
            PrivacyAction::Anonymize
        );

        // 正常记录
        assert_eq!(
            filter.check_privacy("VS Code", "main.rs"),
            PrivacyAction::Record
        );
    }

    #[test]
    fn test_keyword_anonymize() {
        let config = PrivacyConfig::default();
        let filter = PrivacyFilter::from_config(&config);

        // 关键词触发脱敏
        assert_eq!(
            filter.check_privacy("Chrome", "Bank Login"),
            PrivacyAction::Anonymize
        );
        assert_eq!(
            filter.check_privacy("Firefox", "Enter password"),
            PrivacyAction::Anonymize
        );

        // 正常记录
        assert_eq!(
            filter.check_privacy("Chrome", "GitHub"),
            PrivacyAction::Record
        );
    }

    #[test]
    fn 忽略名单应只做单向小写子串匹配() {
        let ignored = vec!["google chrome".to_string()];
        assert!(matches_ignored_app("Google Chrome", &ignored));
        // 裸可执行名先归一化成显示名后命中
        assert!(matches_ignored_app("chrome", &ignored));
        // 不再做反向包含：忽略 Canary 不应误伤 Google Chrome
        let canary_only = vec!["google chrome canary".to_string()];
        assert!(!matches_ignored_app("Google Chrome", &canary_only));
        // 空条目不应导致忽略一切
        assert!(!matches_ignored_app("VS Code", &[String::new()]));
    }

    #[test]
    fn 采集侧域名排除应与统计侧同一套匹配() {
        let mut config = PrivacyConfig::default();
        config.excluded_domains = vec!["example.com".to_string()];
        let filter = PrivacyFilter::from_config(&config);

        let collected = config.collect_excluded_domains();
        for url in [
            "https://example.com/page",
            "https://example.com:8443/path",
            "https://sub.example.com:8443/path",
            " HTTPS://WWW.Example.COM:8443?query=1#fragment ",
            "example.com",
            "Sub.Example.COM:8443/path",
            "example.com?query=1",
            "example.com#fragment",
            "example.com?next=https://other.com&email=a@b.com",
        ] {
            assert!(
                matches_excluded_domain(url, &config.excluded_domains),
                "{url}"
            );
            assert!(matches_excluded_domain(url, &collected), "{url}");
            assert_eq!(
                filter.check_privacy_full("Chrome", "Page", Some(url)),
                PrivacyAction::Skip,
                "{url}"
            );
        }
        // URL 排除的 Skip 仍优先于窗口标题触发的 Anonymize。
        assert_eq!(
            filter.check_privacy_full("Chrome", "Login", Some("https://example.com:8443")),
            PrivacyAction::Skip
        );
        // 配置排除域名且未显式覆盖 unknown_url_fallback 时，无有效 URL 的浏览器窗口默认降级脱敏
        for unknown_url in [
            None,
            Some(""),
            Some("   "),
            Some("https://"),
            Some("not a host"),
        ] {
            assert_eq!(
                filter.check_privacy_full("Chrome", "Page", unknown_url),
                PrivacyAction::Anonymize,
                "unknown url should degrade to Anonymize by default: {unknown_url:?}"
            );
        }
        // 已解析出非排除域名时正常记录
        for url in [
            Some("https://other.com"),
            Some("https://example.com.evil:8443/path"),
            Some("https://notexample.com:8443/path"),
            Some("https://example.com@evil.com:8443/path"),
            Some("https://evil.com/?next=example.com#example.com"),
        ] {
            assert_eq!(
                filter.check_privacy_full("Chrome", "Page", url),
                PrivacyAction::Record,
                "{url:?}"
            );
        }
        // 用户显式设置 unknown_url_fallback = Record 时，未知 URL 仍允许记录
        config.unknown_url_fallback = Some(UnknownUrlFallback::Record);
        let record_filter = PrivacyFilter::from_config(&config);
        assert_eq!(
            record_filter.check_privacy_full("Chrome", "Page", None),
            PrivacyAction::Record
        );
    }

    #[test]
    fn private_browser_windows_detected_across_languages_and_respect_toggle() {
        for (app, title) in [
            ("Google Chrome", "New Tab - Google Chrome (Incognito)"),
            ("Google Chrome", "新标签页 - Google Chrome (无痕)"),
            ("Google Chrome", "新分頁 - Google Chrome (無痕模式)"),
            ("Microsoft Edge", "InPrivate - Microsoft Edge"),
            ("Microsoft Edge", "GitHub - [InPrivate] - Microsoft Edge"),
            ("Firefox", "Mozilla Firefox Private Browsing"),
            ("Firefox", "Rust Docs — Mozilla Firefox (隐私浏览)"),
            ("Safari", "Apple — Private Browsing"),
            ("Safari", "私密浏览 - Safari"),
            ("Brave", "DuckDuckGo - Brave (Private Window with Tor)"),
            ("Google Chrome", "Docs - Google Chrome (Navigation privée)"),
            ("Google Chrome", "Docs - Google Chrome (Privates Fenster)"),
            ("Google Chrome", "Docs - Google Chrome (シークレット)"),
            ("Firefox", "تصفح متخف - Firefox"),
            (
                "Google Chrome",
                "Article - Google Chrome (无痕) - Profile 1",
            ),
        ] {
            assert!(
                is_private_browser_window_title(app, title),
                "expected private window match for {app}: {title}"
            );
        }

        // 普通文章标题与非浏览器应用不应误判为无痕窗口
        assert!(!is_private_browser_window_title(
            "Google Chrome",
            "How Chrome Incognito works - Google Chrome"
        ));
        assert!(!is_private_browser_window_title(
            "Google Chrome",
            "How Chrome (Incognito) Works - Google Chrome"
        ));
        assert!(!is_private_browser_window_title(
            "Google Chrome",
            "How Chrome (Incognito) Works - Medium - Google Chrome"
        ));
        assert!(!is_private_browser_window_title(
            "VS Code",
            "privacy.rs (Incognito) - Work_Review"
        ));

        let mut config = PrivacyConfig::default();
        assert!(config.skip_private_windows);
        let skip_filter = PrivacyFilter::from_config(&config);
        assert_eq!(
            skip_filter.check_privacy_with_private("Google Chrome", "Clean Title", None, true),
            PrivacyAction::Skip
        );

        config.skip_private_windows = false;
        let allow_filter = PrivacyFilter::from_config(&config);
        assert_eq!(
            allow_filter.check_privacy_with_private(
                "Google Chrome",
                "New Tab - Google Chrome (Incognito)",
                Some("https://example.org"),
                true
            ),
            PrivacyAction::Record
        );
    }

    #[test]
    fn privacy_domain_rules_use_the_same_host_normalization() {
        let config = PrivacyConfig {
            excluded_domains: vec![
                " HTTPS://Example.COM:8443/path?query=1#fragment ".to_string(),
                " ".to_string(),
                "https://".to_string(),
            ],
            ..PrivacyConfig::default()
        };
        let collected = config.collect_excluded_domains();
        assert_eq!(collected, vec!["example.com"]);
        let filter = PrivacyFilter::from_config(&config);
        assert_eq!(
            filter.check_privacy_full("Chrome", "Page", Some("sub.example.com:9443")),
            PrivacyAction::Skip
        );
        assert!(matches_excluded_domain("sub.example.com:9443", &collected));
        // 保留子域方向与 www 标签，不把特定子域规则扩大到父域。
        assert!(!matches_excluded_domain(
            "example.com",
            &["www.example.com".to_string()]
        ));
        assert!(!matches_excluded_domain(
            "example.com",
            &[String::new(), "https://".to_string()]
        ));
        // OCR 合并域名兼容语义保持不变。
        assert!(matches_excluded_domain(
            "linux.dolatest",
            &["linux.do".to_string()]
        ));
    }

    #[test]
    fn test_filter_text() {
        let config = PrivacyConfig::default();
        let filter = PrivacyFilter::from_config(&config);

        let text = "My phone is 13812345678 and card is 1234-5678-9012-3456";
        let filtered = filter.filter_text(text);

        assert!(filtered.contains("[已过滤]"));
        assert!(!filtered.contains("13812345678"));

        // 中文紧邻手机号 / 身份证 / 银行卡 / 邮箱与 JSON 引号键值 / PEM 多行私钥均应被过滤
        let cjk = filter.filter_text(
            "手机13812345678 身份证110105199003071234 邮箱alice@example.com {\"password\": \"hunter2\", \"api_key\": \"sk-1234567890abcdefghij\"}",
        );
        assert!(!cjk.contains("13812345678"));
        assert!(!cjk.contains("110105199003071234"));
        assert!(!cjk.contains("110105"));
        assert!(!cjk.contains("alice@example.com"));
        assert!(!cjk.contains("hunter2"));
        assert!(!cjk.contains("sk-1234567890abcdefghij"));

        let pem = filter.filter_text(
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAsecretbody\n-----END RSA PRIVATE KEY-----",
        );
        assert!(!pem.contains("MIIEowIBAAKCAQEAsecretbody"));
    }
}
