use once_cell::sync::Lazy;
use regex::Regex;

static URL_LIKE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)(https?://[^\s<>"']+|(?:localhost|(?:[a-z0-9-]+\.)+[a-z]{2,}|(?:\d{1,3}\.){3}\d{1,3})(?::\d{2,5})?(?:/[^\s<>"']*)?)"#,
    )
    .expect("URL regex should compile")
});

/// 判断进程名是否属于系统/桌面 shell 进程（不应记录使用时长）
/// 这些进程在锁屏/睡眠/唤醒/桌面切换时短暂成为前台，不代表真正的用户活动
pub fn is_system_process(app_name: &str) -> bool {
    let name_lower = app_name.to_lowercase();
    let name_lower = name_lower.trim_end_matches(".exe");

    matches!(
        name_lower,
        // Windows 桌面 / 锁屏 / 搜索
        "desktop"
            | "lockapp"
            | "logonui"
            | "searchapp"
            | "searchhost"
            | "shellexperiencehost"
            | "startmenuexperiencehost"
            | "textinputhost"
            | "dwm"
            | "csrss"
            | "taskmgr"
            // macOS 桌面 / 锁屏
            | "loginwindow"
            | "screensaverengine"
            | "screensaver"
            // Linux 桌面 / 锁屏 / 系统进程
            | "cinnamon-session"
            | "cinnamon-screensaver"
            | "gnome-shell"
            | "gnome-screensaver"
            | "plasmashell"
            | "kscreenlocker"
            | "xscreensaver"
            | "i3lock"
            | "swaylock"
            | "xfce4-session"
    )
}

/// 判断进程名是否属于浏览器
pub fn is_browser_app(app_name: &str) -> bool {
    let app_lower = app_name.to_lowercase();
    // 子串匹配：这些片段足够独特，不会被其它常见应用名误命中
    let substring_match = app_lower.contains("chrome")
        || app_lower.contains("msedge")
        || app_lower.contains("microsoft edge")
        || app_lower.contains("brave")
        || app_lower.contains("opera")
        || app_lower.contains("vivaldi")
        || app_lower.contains("firefox")
        || app_lower.contains("safari")
        || app_lower.contains("orion")
        || app_lower.contains("zen browser")
        || app_lower.contains("browser")
        || app_lower.contains("qq browser")
        || app_lower.contains("360 browser")
        || app_lower.contains("sogou browser")
        || app_lower.contains("360se")
        || app_lower.contains("360chrome")
        || app_lower.contains("qqbrowser")
        || app_lower.contains("sogouexplorer")
        || app_lower.contains("2345explorer")
        || app_lower.contains("liebao")
        || app_lower.contains("maxthon")
        || app_lower.contains("theworld")
        || app_lower.contains("iexplore")
        || app_lower.contains("tabbit");
    if substring_match {
        return true;
    }
    // 精确匹配：避免短关键字误中其它应用
    //   - "cent" 之前用 contains() 会把 "Tencent Lemon" / "Tencent Meeting" 等误判为浏览器
    //   - "arc" 同理（"Arch Linux" 等 .exe 都会误中）
    matches!(
        app_lower.as_str(),
        "cent" | "cent browser" | "centbrowser" | "arc"
    )
}

/// 统一应用显示名称，避免不同来源（进程名、数据库历史、运行中列表）出现重复项
pub fn normalize_display_app_name(app_name: &str) -> String {
    let trimmed = app_name
        .trim()
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .trim();

    let normalized = trimmed.to_lowercase();
    let compact = normalized
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();

    if (normalized.contains("work_review")
        || normalized.contains("work-review")
        || normalized.contains("work review")
        || compact.contains("workreview"))
        && (normalized.contains("setup")
            || normalized.contains("installer")
            || compact.contains("setup")
            || compact.contains("installer"))
    {
        return "Work Review Setup".to_string();
    }

    if normalized.starts_with("com.apple.safari")
        || normalized.starts_with("com.apple.webkit")
        || normalized.contains("safarisupport")
        || normalized.contains("webkit.networking")
    {
        return "Safari".to_string();
    }

    match normalized.as_str() {
        // ── 本应用 ──
        "work-review" | "work_review" | "workreview" | "work review" => "Work Review".to_string(),
        // ── 浏览器 ──
        "chrome" | "google chrome" => "Google Chrome".to_string(),
        "msedge" | "edge" | "microsoft edge" => "Microsoft Edge".to_string(),
        "brave" | "brave browser" => "Brave Browser".to_string(),
        "firefox" => "Firefox".to_string(),
        "safari" => "Safari".to_string(),
        "opera" => "Opera".to_string(),
        "vivaldi" => "Vivaldi".to_string(),
        "chromium" => "Chromium".to_string(),
        "arc" => "Arc".to_string(),
        "zen browser" | "zen" => "Zen Browser".to_string(),
        "cent" | "cent browser" | "centbrowser" => "Cent Browser".to_string(),
        "qqbrowser" | "qq browser" | "qq浏览器" => "QQ Browser".to_string(),
        "360se" | "360chrome" | "360 browser" | "360浏览器" => "360 Browser".to_string(),
        "sogouexplorer" | "sogou browser" | "搜狗浏览器" => "Sogou Browser".to_string(),
        "maxthon" => "Maxthon".to_string(),
        "yandex" | "yandex browser" => "Yandex Browser".to_string(),
        "tor" | "tor browser" => "Tor Browser".to_string(),
        "waterfox" => "Waterfox".to_string(),
        "librewolf" => "LibreWolf".to_string(),
        "floorp" => "Floorp".to_string(),
        "iceweasel" => "Iceweasel".to_string(),
        // ── IDE / 编辑器 ──
        "code" | "vscode" | "visual studio code" | "vs code" => "VS Code".to_string(),
        "cursor" => "Cursor".to_string(),
        "windsurf" | "antigravity" => "Windsurf".to_string(),
        "idea" | "idea64" | "intellij idea" => "IntelliJ IDEA".to_string(),
        "pycharm" | "pycharm64" => "PyCharm".to_string(),
        "webstorm" | "webstorm64" => "WebStorm".to_string(),
        "goland" | "goland64" => "GoLand".to_string(),
        "clion" | "clion64" => "CLion".to_string(),
        "rider" | "rider64" => "Rider".to_string(),
        "phpstorm" | "phpstorm64" => "PhpStorm".to_string(),
        "rubymine" | "rubymine64" => "RubyMine".to_string(),
        "datagrip" | "datagrip64" => "DataGrip".to_string(),
        "fleet" => "Fleet".to_string(),
        "android studio" | "studio64" => "Android Studio".to_string(),
        "devenv" | "visual studio" => "Visual Studio".to_string(),
        "xcode" => "Xcode".to_string(),
        "sublime_text" | "sublime text" => "Sublime Text".to_string(),
        "atom" => "Atom".to_string(),
        "zed" | "zed-editor" => "Zed".to_string(),
        "nova" => "Nova".to_string(),
        "textmate" => "TextMate".to_string(),
        "vim" | "gvim" | "mvim" => "Vim".to_string(),
        "nvim" => "Neovim".to_string(),
        "emacs" => "Emacs".to_string(),
        "codeblocks" => "Code::Blocks".to_string(),
        // ── 通讯 / 社交 ──
        "wechat" | "weixin" | "微信" => "WeChat".to_string(),
        "wecom" | "企业微信" | "wxwork" => "WeCom".to_string(),
        "qq" => "QQ".to_string(),
        "telegram" | "telegram desktop" => "Telegram".to_string(),
        "slack" => "Slack".to_string(),
        "discord" => "Discord".to_string(),
        "teams" | "msteams" | "ms-teams" | "microsoft teams" => "Microsoft Teams".to_string(),
        "dingtalk" | "钉钉" => "DingTalk".to_string(),
        "feishu" | "飞书" | "lark" => "Feishu".to_string(),
        "zoom" | "zoom.us" => "Zoom".to_string(),
        "skype" | "skypeapp" => "Skype".to_string(),
        "line" => "LINE".to_string(),
        "whatsapp" => "WhatsApp".to_string(),
        "signal" => "Signal".to_string(),
        "steam" | "steamwebhelper" => "Steam".to_string(),
        // ── 办公 / 笔记 ──
        "notion" | "notion-enhanced" | "notion-enhanced-app" => "Notion".to_string(),
        "obsidian" => "Obsidian".to_string(),
        "typora" => "Typora".to_string(),
        "marktext" | "mark text" => "Mark Text".to_string(),
        "onenote" | "microsoft onenote" => "OneNote".to_string(),
        "evernote" => "Evernote".to_string(),
        "youdaonote" | "有道云笔记" => "Youdao Note".to_string(),
        "yuque" | "语雀" => "Yuque".to_string(),
        // ── Microsoft Office ──
        "winword" | "word" => "Microsoft Word".to_string(),
        "excel" => "Microsoft Excel".to_string(),
        "powerpnt" | "powerpoint" => "Microsoft PowerPoint".to_string(),
        "outlook" => "Microsoft Outlook".to_string(),
        "msaccess" | "access" => "Microsoft Access".to_string(),
        "mspub" | "publisher" => "Microsoft Publisher".to_string(),
        "et" | "wps" => "WPS Office".to_string(),
        "wpp" => "WPS Presentation".to_string(),
        "wpspdf" => "WPS PDF".to_string(),
        // ── 终端 ──
        "windowsterminal" | "windows terminal" | "windowsterminal.exe" => {
            "Windows Terminal".to_string()
        }
        "powershell" | "pwsh" => "PowerShell".to_string(),
        "cmd" => "Command Prompt".to_string(),
        "iterm2" | "iterm" => "iTerm2".to_string(),
        "terminal" | "terminal.app" => "Terminal".to_string(),
        "warp" => "Warp".to_string(),
        "alacritty" => "Alacritty".to_string(),
        "kitty" => "Kitty".to_string(),
        "wezterm" | "wezterm-gui" => "WezTerm".to_string(),
        "hyper" => "Hyper".to_string(),
        "tabby" => "Tabby".to_string(),
        "terminus" => "Terminus".to_string(),
        "mobaxterm" | "mobaxterm1" => "MobaXterm".to_string(),
        "putty" => "PuTTY".to_string(),
        // Linux 终端
        "gnome-terminal" | "gnome-terminal-server" => "GNOME Terminal".to_string(),
        "xfce4-terminal" => "Xfce Terminal".to_string(),
        "konsole" => "Konsole".to_string(),
        "tilix" => "Tilix".to_string(),
        "terminator" => "Terminator".to_string(),
        // ── 文件管理器 ──
        "explorer" => "File Explorer".to_string(),
        "finder" => "Finder".to_string(),
        "nemo" => "Nemo".to_string(),
        "nautilus" | "org.gnome.nautilus" => "Files".to_string(),
        "thunar" => "Thunar".to_string(),
        "dolphin" => "Dolphin".to_string(),
        // ── 设计 / 绘图 ──
        "figma" => "Figma".to_string(),
        "xd" | "adobe xd" => "Adobe XD".to_string(),
        "photoshop" | "adobe photoshop" => "Photoshop".to_string(),
        "illustrator" | "adobe illustrator" => "Illustrator".to_string(),
        "sketch" => "Sketch".to_string(),
        "inkscape" => "Inkscape".to_string(),
        "gimp" => "GIMP".to_string(),
        "blender" => "Blender".to_string(),
        "canva" => "Canva".to_string(),
        // ── 音乐 / 视频 ──
        "spotify" => "Spotify".to_string(),
        "netease_cloudmusic" | "cloudmusic" | "网易云音乐" => {
            "NetEase Cloud Music".to_string()
        }
        "qqmusic" | "qqmusicuniversal" | "qq音乐" => "QQ Music".to_string(),
        "kugou" | "酷狗音乐" => "KuGou Music".to_string(),
        "kuwo" | "酷我音乐" => "KuWo Music".to_string(),
        "vlc" => "VLC".to_string(),
        "potplayer" | "potplayermini64" => "PotPlayer".to_string(),
        "mpv" => "mpv".to_string(),
        "iina" => "IINA".to_string(),
        "apple music" | "music" => "Music".to_string(),
        // ── 开发工具 ──
        "docker desktop" => "Docker Desktop".to_string(),
        "postman" => "Postman".to_string(),
        "insomnia" => "Insomnia".to_string(),
        "fork" | "fork-git-client" => "Fork".to_string(),
        "sourcetree" => "SourceTree".to_string(),
        "githubdesktop" | "github desktop" => "GitHub Desktop".to_string(),
        "gitkraken" => "GitKraken".to_string(),
        "tableplus" => "TablePlus".to_string(),
        "navicat" | "navicatpremium" => "Navicat".to_string(),
        "robomongo" | "robo3t" | "studio 3t" => "MongoDB Compass".to_string(),
        "redis-desktop-manager" | "rdm" => "RedisInsight".to_string(),
        // ── 远程桌面 / SSH ──
        "mstsc" => "Remote Desktop".to_string(),
        "teamviewer" => "TeamViewer".to_string(),
        "anydesk" => "AnyDesk".to_string(),
        "tovnavive" | "to desk" => "ToDesk".to_string(),
        "sunloginclient" | "sunlogin" => "Sunlogin".to_string(),
        // ── 其他 ──
        "mail" | "apple mail" | "邮件" => "Mail".to_string(),
        "discover" | "org.kde.discover" => "Discover".to_string(),
        "coreautha" | "coreauthuiagent" | "coreauthenticationuiagent" => {
            "System Authentication".to_string()
        }
        "xfltd" => "XFLTD".to_string(),
        "thunderbird" | "thunderbird-bin" => "Thunderbird".to_string(),
        "libreoffice" => "LibreOffice".to_string(),
        "evince" | "org.gnome.evince" => "Evince".to_string(),
        "eog" | "org.gnome.eog" => "Eye of GNOME".to_string(),
        "gedit" | "org.gnome.gedit" => "gedit".to_string(),
        "calibre" | "calibre-gui" => "Calibre".to_string(),
        // ── 系统工具 ──
        "lemon" | "tencent lemon" => "Tencent Lemon".to_string(),
        "cleanmymac" | "clean my mac" => "CleanMyMac".to_string(),
        "alfred" => "Alfred".to_string(),
        "raycast" => "Raycast".to_string(),
        "bartender" => "Bartender".to_string(),
        "istat menus" | "istat" => "iStat Menus".to_string(),
        "appcleaner" | "app cleaner" => "AppCleaner".to_string(),
        "the unarchiver" | "unarchiver" => "The Unarchiver".to_string(),
        "keka" => "Keka".to_string(),
        "daisydisk" => "DaisyDisk".to_string(),
        "onyx" => "OnyX".to_string(),
        "macpaw" => "MacPaw".to_string(),
        "sensei" => "Sensei".to_string(),
        "peak" => "Peak".to_string(),
        "ninjaclean" | "ninja clean" => "Ninja Clean".to_string(),
        "applink" => "AppLink".to_string(),
        "eqmac" => "eqMac".to_string(),
        "rectangle" => "Rectangle".to_string(),
        "magnet" => "Magnet".to_string(),
        "spectacle" => "Spectacle".to_string(),
        "amethyst" => "Amethyst".to_string(),
        "yabai" => "yabai".to_string(),
        "stats" => "Stats".to_string(),
        "monitor" => "Monitor".to_string(),
        _ => trimmed.to_string(),
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_probable_domain(value: &str) -> bool {
    let candidate = value.trim().trim_matches('/').to_lowercase();
    if candidate.is_empty()
        || candidate.contains(' ')
        || candidate.starts_with('.')
        || candidate.ends_with('.')
        || !candidate.contains('.')
    {
        return false;
    }

    let labels: Vec<&str> = candidate.split('.').collect();
    if labels.len() < 2 {
        return false;
    }

    let tld = labels.last().copied().unwrap_or_default();
    // TLD 最少 2 字符、最多 12 字符，且必须全是 ASCII 字母
    // 上限防止 OCR 丢失斜杠后把域名和路径拼为超长假 TLD（如 github.comwm94i）
    if tld.len() < 2 || tld.len() > 12 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }

    labels.iter().all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn trim_url_candidate(value: &str) -> &str {
    value.trim().trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';'
        )
    })
}

fn split_host_and_rest(value: &str) -> (&str, &str) {
    if let Some(index) = value.find(|c| ['/', '?', '#'].contains(&c)) {
        (&value[..index], &value[index..])
    } else {
        (value, "")
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn split_host_port(value: &str) -> (&str, Option<&str>) {
    if let Some(index) = value.rfind(':') {
        let host = &value[..index];
        let port = &value[index + 1..];
        if !host.is_empty() && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            return (host, Some(port));
        }
    }

    (value, None)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_probable_ipv4(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 4 {
        return false;
    }

    parts.iter().all(|part| {
        !part.is_empty()
            && part.len() <= 3
            && part.chars().all(|c| c.is_ascii_digit())
            && part.parse::<u8>().is_ok()
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_probable_host(value: &str) -> bool {
    let host = value.trim().trim_end_matches('.');
    if host.is_empty() {
        return false;
    }

    let (host_without_port, _) = split_host_port(host);
    let host_lower = host_without_port.to_lowercase();

    host_lower == "localhost"
        || is_probable_domain(host_without_port)
        || is_probable_ipv4(host_without_port)
}

/// 将浏览器地址栏或窗口标题中的候选值规范化为 URL。
pub fn normalize_browser_url_candidate(value: &str) -> Option<String> {
    let candidate = trim_url_candidate(value)
        .trim_matches(|c: char| c.is_control() || c == '\u{200b}' || c == '\u{feff}')
        .trim_end_matches('.');

    if candidate.is_empty() {
        return None;
    }

    if candidate.contains(' ') {
        return None;
    }

    let candidate_lower = candidate.to_lowercase();
    if candidate_lower.starts_with("http://") || candidate_lower.starts_with("https://") {
        return Some(candidate.to_string());
    }

    if candidate.contains("://")
        || candidate_lower.starts_with("about:")
        || candidate_lower.starts_with("chrome:")
        || candidate_lower.starts_with("edge:")
        || candidate_lower.starts_with("file:")
    {
        return Some(candidate.to_string());
    }

    let (host, _) = split_host_and_rest(candidate);
    if is_probable_host(host) {
        let result = format!(
            "{}{}",
            if split_host_port(host).0.to_lowercase() == "localhost"
                || is_probable_ipv4(split_host_port(host).0)
            {
                "http://"
            } else {
                "https://"
            },
            candidate.trim_end_matches('/')
        );
        if is_merged_domain(&result) {
            return None;
        }
        return Some(result);
    }

    if is_probable_domain(candidate) {
        let result = format!("https://{}", candidate.trim_end_matches('/'));
        if is_merged_domain(&result) {
            return None;
        }
        return Some(result);
    }

    None
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn extract_url_from_text(text: &str) -> Option<String> {
    URL_LIKE_RE
        .find_iter(text)
        .filter_map(|m| normalize_browser_url_candidate(m.as_str()))
        .next()
}

/// 检测是否为可疑的 host-only 域名（可能由 OCR 丢失斜杠导致域名+路径合并）
/// 例如 `linux.do/latest` → OCR 丢失 `/` → `linux.dolatest`
pub fn is_merged_domain(url: &str) -> bool {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);

    let (host, rest) = split_host_and_rest(without_scheme);
    if !rest.is_empty() {
        return false;
    }

    let host = split_host_port(host).0.trim_end_matches('.');
    if host.is_empty() || host == "localhost" {
        return false;
    }

    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() != 2 {
        return false;
    }

    let tld = labels[1].to_lowercase();
    // 只检查较长（>6字符）且以已知 ccTLD 前缀开头的假 TLD
    if tld.len() <= 6 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }

    let prefix = &tld[..2];
    matches!(
        prefix,
        "ai" | "cc"
            | "cn"
            | "de"
            | "do"
            | "fr"
            | "hk"
            | "id"
            | "in"
            | "io"
            | "jp"
            | "kr"
            | "me"
            | "ru"
            | "sg"
            | "tv"
            | "uk"
            | "us"
    )
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn infer_browser_page_hint(window_title: &str) -> Option<String> {
    extract_url_from_title(window_title).filter(|url| !is_merged_domain(url))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn infer_browser_page_hint_from_text(text: &str) -> Option<String> {
    extract_url_from_text(text).filter(|url| !is_merged_domain(url))
}

/// 按采集网址、窗口标题、OCR 文本的优先级解析浏览器页面。
/// 保留页面路径供展示使用；可疑的合并域名不能阻止标题和 OCR 兜底。
pub fn resolve_browser_page_hint(
    browser_url: Option<&str>,
    window_title: &str,
    ocr_text: Option<&str>,
) -> Option<String> {
    browser_url
        .map(|url| url.trim().trim_end_matches('/'))
        .filter(|url| !url.is_empty() && !is_merged_domain(url))
        .map(str::to_string)
        .or_else(|| infer_browser_page_hint(window_title))
        .or_else(|| ocr_text.and_then(infer_browser_page_hint_from_text))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn browser_page_domain_label(page_hint: &str) -> String {
    if let Some(url) = normalize_browser_url_candidate(page_hint) {
        let without_scheme = url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(url.as_str());
        let (host, _) = split_host_and_rest(without_scheme);
        return split_host_port(host).0.to_string();
    }

    page_hint.trim().to_string()
}

pub fn normalize_domain_rule(value: &str) -> Option<String> {
    let domain = browser_page_domain_label(value).trim().to_lowercase();
    if domain.is_empty() {
        None
    } else {
        Some(domain)
    }
}

pub fn find_website_semantic_override(
    rules: &[crate::config::WebsiteSemanticRule],
    browser_url: Option<&str>,
) -> Option<String> {
    let target_domain = browser_url.and_then(normalize_domain_rule)?;

    rules.iter().find_map(|rule| {
        let rule_domain = normalize_domain_rule(&rule.domain)?;
        if rule_domain == target_domain {
            Some(rule.semantic_category.trim().to_string())
        } else {
            None
        }
    })
}

/// 归一化基础分类 key，但保留自定义分类 key（不强制收敛到 other）。
fn normalize_category_keeping_custom(fallback_category: &str) -> String {
    let fallback = fallback_category.trim().to_lowercase();
    let normalized = normalize_category_key(&fallback);
    if normalized == "other" && !fallback.is_empty() && fallback != "other" {
        fallback
    } else {
        normalized
    }
}

/// 将网站语义分类映射到基础分类，便于工作/休息时长统计直接生效。
/// 对无法识别的语义分类，保留原有基础分类。
pub fn semantic_category_to_base_category(
    semantic_category: &str,
    fallback_category: &str,
) -> String {
    let semantic = semantic_category.trim();
    if semantic.is_empty() {
        return normalize_category_keeping_custom(fallback_category);
    }

    let mapped = match semantic {
        "休息娱乐" | "视频内容" | "音乐音频" => Some("entertainment"),
        "即时聊天" | "会议沟通" => Some("communication"),
        "设计创作" => Some("design"),
        "编码开发" => Some("development"),
        "内容撰写" => Some("office"),
        "资料阅读" | "资料调研" | "任务规划" | "AI 协作" | "未知活动" => {
            Some("browser")
        }
        _ => None,
    };

    mapped
        .map(str::to_string)
        .unwrap_or_else(|| normalize_category_keeping_custom(fallback_category))
}

fn extract_url_from_title(window_title: &str) -> Option<String> {
    let title = window_title.trim();
    if title.is_empty() {
        return None;
    }

    if let Some(url) = title
        .split_whitespace()
        .next()
        .and_then(normalize_browser_url_candidate)
    {
        return Some(url);
    }

    for part in title.rsplit(" - ") {
        if let Some(url) = normalize_browser_url_candidate(part) {
            return Some(url);
        }
    }

    extract_url_from_text(title)
}
fn categorize_app_with_policy(
    app_name: &str,
    window_title: &str,
    preserve_collector_behavior: bool,
) -> String {
    let app_lower = app_name.to_lowercase();

    // 开发工具（IDE、编辑器、终端、数据库工具、API 工具、容器、版本控制）
    if app_lower.contains("code")
        || app_lower.contains("visual studio")
        || app_lower.contains("cursor")
        || app_lower.contains("idea")
        || app_lower.contains("pycharm")
        || app_lower.contains("webstorm")
        || app_lower.contains("goland")
        || app_lower.contains("clion")
        || app_lower.contains("rustrover")
        || app_lower.contains("rider")
        || app_lower.contains("phpstorm")
        || app_lower.contains("datagrip")
        || app_lower.contains("fleet")
        || app_lower.contains("xcode")
        || app_lower.contains("android studio")
        || app_lower.contains("hbuilder")
        || app_lower.contains("sublime")
        || app_lower.contains("atom")
        || app_lower.contains("vim")
        || app_lower.contains("neovim")
        || app_lower.contains("emacs")
        || app_lower.contains("nova")
        || app_lower.contains("bbedit")
        || app_lower.contains("coteditor")
        || app_lower.contains("textmate")
        || app_lower.contains("terminal")
        || app_lower.contains("iterm")
        || app_lower.contains("warp")
        || app_lower.contains("alacritty")
        || app_lower.contains("kitty")
        || app_lower.contains("wezterm")
        || app_lower.contains("hyper")
        || app_lower.contains("windowsterminal")
        || app_lower.contains("cmd")
        || app_lower.contains("powershell")
        || app_lower.contains("git")
        || app_lower.contains("sourcetree")
        || app_lower.contains("gitkraken")
        || app_lower.contains("docker")
        || app_lower.contains("postman")
        || app_lower.contains("insomnia")
        || app_lower.contains("dbeaver")
        || app_lower.contains("navicat")
        || app_lower.contains("tableplus")
        || app_lower.contains("sequel")
        || app_lower.contains("charles")
        || app_lower.contains("fiddler")
    {
        return "development".to_string();
    }

    // 浏览器（支持市面上所有主流浏览器，包含 Windows 进程名）
    // 注意：短名称用精确匹配或 starts_with，避免误匹配系统进程
    if app_lower.contains("chrome")
        || app_lower.contains("firefox")
        || app_lower.contains("safari")
        || app_lower.contains("msedge")
        || app_lower.contains("microsoft edge")
        || app_lower.contains("opera")
        || app_lower.contains("brave")
        || app_lower.starts_with("arc")
        || app_lower.contains("vivaldi")
        || app_lower.contains("chromium")
        || app_lower.contains("orion")
        || app_lower.starts_with("zen")
        || app_lower.contains("sidekick")
        || app_lower.contains("wavebox")
        || app_lower.contains("maxthon")
        || app_lower.contains("waterfox")
        || app_lower.contains("librewolf")
        || app_lower.contains("tor browser")
        || app_lower.contains("duckduckgo")
        || app_lower.contains("yandex")
        || app_lower.starts_with("whale")
        || app_lower.contains("naver")
        || app_lower.contains("uc browser")
        || app_lower.contains("qq browser")
        || app_lower.contains("360 browser")
        || app_lower.contains("sogou browser")
        || app_lower.contains("qqbrowser")
        || app_lower.contains("360se")
        || app_lower.contains("360chrome")
        || app_lower.contains("sogouexplorer")
        || app_lower.contains("2345explorer")
        || app_lower.contains("liebao")
        || app_lower.contains("theworld")
        || app_lower.contains("centbrowser")
        || app_lower.contains("iexplore")
        || app_lower.contains("qq浏览器")
        || app_lower.contains("360浏览器")
        || app_lower.contains("搜狗浏览器")
        || app_lower.contains("tabbit")
    {
        return "browser".to_string();
    }

    // 通讯工具（注意：qq 的匹配要排除已被浏览器捕获的 qqbrowser）
    if app_lower.contains("slack")
        || app_lower.contains("teams")
        || app_lower.contains("zoom")
        || app_lower.contains("discord")
        || app_lower.contains("wechat")
        // 核心分类器避免把微信读书误归通讯；采集侧兼容模式保留迁移前的历史口径。
        || (app_lower.contains("微信")
            && (preserve_collector_behavior || !app_lower.contains("微信读书")))
        || app_lower.contains("wecom")
        || app_lower.contains("企业微信")
        || (app_lower.contains("qq") && !app_lower.contains("qqbrowser"))
        || app_lower.contains("telegram")
        || app_lower.contains("skype")
        || app_lower.contains("dingtalk")
        || app_lower.contains("钉钉")
        || app_lower.contains("飞书")
        || app_lower.contains("lark")
    {
        return "communication".to_string();
    }

    // 办公软件
    if app_lower.contains("word")
        || app_lower.contains("excel")
        || app_lower.contains("powerpoint")
        || app_lower.contains("pages")
        || app_lower.contains("numbers")
        || app_lower.contains("keynote")
        || app_lower.contains("notion")
        || app_lower.contains("obsidian")
        || app_lower.contains("logseq")
        || app_lower.contains("evernote")
        || app_lower.contains("onenote")
        || app_lower.contains("wps")
        || app_lower.contains("typora")
        || app_lower.contains("bear")
        || app_lower.contains("ulysses")
        || app_lower.contains("xmind")
        || app_lower.contains("mindnode")
    {
        return "office".to_string();
    }

    // 设计工具
    if app_lower.contains("figma")
        || app_lower.contains("sketch")
        || app_lower.contains("photoshop")
        || app_lower.contains("illustrator")
        || app_lower.contains("xd")
        || app_lower.contains("canva")
        || app_lower.contains("pixelmator")
        || app_lower.contains("affinity")
        || app_lower.contains("lightroom")
        || app_lower.contains("indesign")
    {
        return "design".to_string();
    }

    // 娱乐
    if app_lower.contains("spotify")
        || app_lower.contains("music")
        || app_lower.contains("youtube")
        || app_lower.contains("netflix")
        || app_lower.contains("bilibili")
        || app_lower.contains("game")
        || app_lower.contains("steam")
        || app_lower.contains("网易云")
        || app_lower.contains("qqmusic")
        || app_lower.contains("爱奇艺")
    {
        return "entertainment".to_string();
    }

    // 内置应用知识库兜底：覆盖上方硬编码链之外的常见应用（中文生态/创作/学术等），
    // 词边界匹配防误伤；仍未命中才继续窗口标题兜底与 "other"
    if !preserve_collector_behavior {
        if let Some(category) = crate::knowledge::builtin_app_category(&app_lower) {
            return category.to_string();
        }
    }

    // 窗口标题兜底：app_name 无法识别时，用窗口标题中的 IDE/工具关键词做最后一轮匹配
    // 典型场景：Windows 上 JetBrains IDE 进程名可能是 java.exe / idea64.exe 截断后不匹配
    if !window_title.is_empty() {
        let title_lower = window_title.to_lowercase();
        if title_lower.contains("intellij")
            || title_lower.contains("pycharm")
            || title_lower.contains("webstorm")
            || title_lower.contains("goland")
            || title_lower.contains("clion")
            || title_lower.contains("datagrip")
            || title_lower.contains("rustrover")
            || title_lower.contains("visual studio")
            || title_lower.contains("vs code")
            || title_lower.contains("cursor")
        {
            return "development".to_string();
        }
    }

    "other".to_string()
}

/// 使用核心知识库的统一应用分类。
pub fn categorize_app(app_name: &str, window_title: &str) -> String {
    categorize_app_with_policy(app_name, window_title, false)
}

/// 保留桌面采集器迁移前的分类口径，避免已有统计在纯结构重构后发生变化。
pub fn categorize_collected_app(app_name: &str, window_title: &str) -> String {
    categorize_app_with_policy(app_name, window_title, true)
}

pub fn normalize_category_key(category: &str) -> String {
    match category.trim().to_lowercase().as_str() {
        "development" | "browser" | "communication" | "office" | "design" | "entertainment"
        | "other" => category.trim().to_lowercase(),
        _ => "other".to_string(),
    }
}

/// 检查分类 key 是否有效（预设 + 自定义）
pub fn is_valid_category_key(
    category: &str,
    custom_categories: &[crate::config::CustomCategory],
) -> bool {
    let lowered = category.trim().to_lowercase();
    custom_categories.iter().any(|c| c.key == lowered)
}

fn normalized_app_rule_key(app_name: &str) -> String {
    normalize_display_app_name(app_name).to_lowercase()
}

fn find_category_override_with_policy(
    rules: &[crate::config::AppCategoryRule],
    app_name: &str,
    custom_categories: &[crate::config::CustomCategory],
    allow_short_fuzzy_match: bool,
) -> Option<String> {
    let normalized_app_name = normalized_app_rule_key(app_name);
    let custom_keys: Vec<String> = custom_categories.iter().map(|c| c.key.clone()).collect();

    rules.iter().find_map(|rule| {
        let normalized_rule = normalized_app_rule_key(&rule.app_name);
        let exact = normalized_app_name == normalized_rule;
        let app_contains_rule = (allow_short_fuzzy_match || normalized_rule.len() >= 3)
            && normalized_app_name.contains(&normalized_rule);
        let rule_contains_app = (allow_short_fuzzy_match || normalized_app_name.len() >= 3)
            && normalized_rule.contains(&normalized_app_name);
        if exact || app_contains_rule || rule_contains_app {
            Some(crate::config::normalize_category_key_private(
                &rule.category,
                &custom_keys,
            ))
        } else {
            None
        }
    })
}

pub fn find_category_override(
    rules: &[crate::config::AppCategoryRule],
    app_name: &str,
    custom_categories: &[crate::config::CustomCategory],
) -> Option<String> {
    find_category_override_with_policy(rules, app_name, custom_categories, false)
}

/// 保留桌面采集器迁移前对短应用名称的模糊匹配行为。
pub fn find_collected_category_override(
    rules: &[crate::config::AppCategoryRule],
    app_name: &str,
    custom_categories: &[crate::config::CustomCategory],
) -> Option<String> {
    find_category_override_with_policy(rules, app_name, custom_categories, true)
}

pub fn categorize_app_with_rules(
    rules: &[crate::config::AppCategoryRule],
    app_name: &str,
    window_title: &str,
    custom_categories: &[crate::config::CustomCategory],
) -> String {
    find_category_override(rules, app_name, custom_categories)
        .unwrap_or_else(|| categorize_app(app_name, window_title))
}

/// 使用迁移前桌面采集器的分类与手动规则匹配口径。
pub fn categorize_collected_app_with_rules(
    rules: &[crate::config::AppCategoryRule],
    app_name: &str,
    window_title: &str,
    custom_categories: &[crate::config::CustomCategory],
) -> String {
    find_collected_category_override(rules, app_name, custom_categories)
        .unwrap_or_else(|| categorize_collected_app(app_name, window_title))
}

/// 获取分类的中文名称
pub fn get_category_name(category: &str) -> &str {
    match category {
        "development" => "开发工具",
        "browser" => "浏览器",
        "communication" => "通讯协作",
        "office" => "办公软件",
        "design" => "设计工具",
        "entertainment" => "娱乐",
        _ => "其他",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_browser_app_recognises_common_browsers() {
        assert!(is_browser_app("Google Chrome"));
        assert!(is_browser_app("Microsoft Edge"));
        assert!(is_browser_app("Firefox"));
        assert!(is_browser_app("Safari"));
        assert!(is_browser_app("QQ Browser"));
    }

    #[test]
    fn is_browser_app_rejects_apps_with_substrings_that_used_to_false_match() {
        // 回归测试：之前 contains("cent") 让 "Tencent Lemon" 等被识别为浏览器，
        // 进而被收进日报的"网站访问明细"。同理 contains("arc") 误中 "Arch Linux"。
        assert!(!is_browser_app("Tencent Lemon"));
        assert!(!is_browser_app("Tencent Meeting"));
        assert!(!is_browser_app("WeCom"));
        assert!(!is_browser_app("Arch Linux"));
        assert!(!is_browser_app("Spotlight Search"));
        // 真正的 Cent / Arc 浏览器仍然能识别
        assert!(is_browser_app("Cent"));
        assert!(is_browser_app("Cent Browser"));
        assert!(is_browser_app("Arc"));
    }

    #[test]
    fn normalize_display_app_name_returns_canonical_for_tencent_lemon() {
        // 显示名归一化已集中在核心模块，确保腾讯柠檬走到统一规则。
        assert_eq!(normalize_display_app_name("Lemon"), "Tencent Lemon");
        assert_eq!(normalize_display_app_name("Tencent Lemon"), "Tencent Lemon");
        assert_eq!(normalize_display_app_name("LEMON.exe"), "Tencent Lemon");
    }

    #[test]
    fn normalize_display_app_name_returns_canonical_for_cent_browser() {
        assert_eq!(normalize_display_app_name("cent"), "Cent Browser");
        assert_eq!(normalize_display_app_name("cent browser"), "Cent Browser");
        assert_eq!(normalize_display_app_name("centbrowser"), "Cent Browser");
        assert_eq!(
            normalize_display_app_name("centbrowser.exe"),
            "Cent Browser"
        );
    }

    #[test]
    fn 归一化后的浏览器显示名仍能归类为浏览器() {
        assert_eq!(categorize_app("Microsoft Edge", "example.com"), "browser");
        assert_eq!(categorize_app("QQ Browser", "example.com"), "browser");
        assert_eq!(categorize_app("360 Browser", "example.com"), "browser");
        assert_eq!(categorize_app("Sogou Browser", "example.com"), "browser");
    }

    #[test]
    fn 手动分类规则应优先于内置分类() {
        let rules = vec![crate::config::AppCategoryRule {
            app_name: "MuMu".to_string(),
            category: "entertainment".to_string(),
        }];

        assert_eq!(
            categorize_app_with_rules(&rules, "MuMu模拟器", "项目设计稿", &[]),
            "entertainment"
        );
        assert_eq!(categorize_app("MuMu模拟器", "项目设计稿"), "other");
    }

    #[test]
    fn 手动分类规则匹配应兼容应用名归一化() {
        let rules = vec![crate::config::AppCategoryRule {
            app_name: "Firefox".to_string(),
            category: "office".to_string(),
        }];

        assert_eq!(
            categorize_app_with_rules(&rules, "firefox", "搜索页", &[]),
            "office"
        );
    }

    #[test]
    fn 采集侧分类迁移应保留微信读书的原有分类() {
        assert_eq!(categorize_collected_app("微信读书", ""), "communication");
        assert_eq!(categorize_app("微信读书", ""), "entertainment");
    }

    #[test]
    fn 采集侧手动规则迁移应保留短名称模糊匹配() {
        let rules = vec![crate::config::AppCategoryRule {
            app_name: "QQ".to_string(),
            category: "entertainment".to_string(),
        }];

        assert_eq!(
            find_collected_category_override(&rules, "QQ Music", &[]),
            Some("entertainment".to_string())
        );
        assert_eq!(find_category_override(&rules, "QQ Music", &[]), None);
    }

    #[test]
    fn 常见系统与桌面应用名应归一化为稳定显示名() {
        assert_eq!(normalize_display_app_name("discover"), "Discover");
        assert_eq!(normalize_display_app_name("mail"), "Mail");
        assert_eq!(normalize_display_app_name("邮件"), "Mail");
        assert_eq!(
            normalize_display_app_name("coreautha"),
            "System Authentication"
        );
        assert_eq!(
            normalize_display_app_name("Work_Review.v1.0.35_x64-setup"),
            "Work Review Setup"
        );
        assert_eq!(normalize_display_app_name("xfltd"), "XFLTD");
    }

    #[test]
    fn 网站语义分类应映射为可统计的基础分类并保留自定义兜底() {
        assert_eq!(
            semantic_category_to_base_category("休息娱乐", "browser"),
            "entertainment"
        );
        assert_eq!(
            semantic_category_to_base_category("编码开发", "browser"),
            "development"
        );
        assert_eq!(
            semantic_category_to_base_category("资料阅读", "browser"),
            "browser"
        );
        assert_eq!(
            semantic_category_to_base_category("未知自定义语义", "custom-focus"),
            "custom-focus"
        );
    }

    #[test]
    fn 规范化地址栏候选值并过滤合并域名() {
        assert_eq!(
            normalize_browser_url_candidate("https://example.com/path"),
            Some("https://example.com/path".to_string())
        );
        assert_eq!(
            normalize_browser_url_candidate("example.com"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            normalize_browser_url_candidate("bing.com/search?q=test"),
            Some("https://bing.com/search?q=test".to_string())
        );
        assert_eq!(
            normalize_browser_url_candidate("localhost:3000/dashboard"),
            Some("http://localhost:3000/dashboard".to_string())
        );
        assert_eq!(
            normalize_browser_url_candidate("chrome://settings"),
            Some("chrome://settings".to_string())
        );
        assert_eq!(normalize_browser_url_candidate("搜索内容"), None);
        assert_eq!(normalize_browser_url_candidate("1.2.3"), None);
        assert_eq!(normalize_browser_url_candidate("linux.dolatest"), None);
    }

    #[test]
    fn 从标题提取域名时避免误判() {
        assert_eq!(
            extract_url_from_title("项目文档 - docs.example.com - Google Chrome"),
            Some("https://docs.example.com".to_string())
        );
        assert_eq!(
            extract_url_from_title("bing.com/search?q=test - Google Chrome"),
            Some("https://bing.com/search?q=test".to_string())
        );
        assert_eq!(extract_url_from_title("版本 1.2.3 - Google Chrome"), None);
        assert!(is_probable_domain("sub.example.com"));
        assert!(!is_probable_domain("1.2.3"));
        assert_eq!(infer_browser_page_hint("https://linux.dolatest"), None);
    }

    #[test]
    fn 浏览器页面解析应统一网址标题和ocr的优先级() {
        let cases = [
            (
                Some(" https://example.com/from-url/ "),
                "https://title.example/page - Chrome",
                Some("https://ocr.example/page"),
                Some("https://example.com/from-url"),
            ),
            (
                None,
                "title.example - Chrome",
                Some("https://ocr.example/page"),
                Some("https://title.example"),
            ),
            (
                Some(" \t\n "),
                "title.example - Chrome",
                None,
                Some("https://title.example"),
            ),
            (
                Some(""),
                "页面加载中",
                Some("当前页面 https://ocr.example/page"),
                Some("https://ocr.example/page"),
            ),
            (
                Some("https://linux.dolatest/"),
                "title.example - Chrome",
                Some("https://ocr.example/page"),
                Some("https://title.example"),
            ),
            (
                Some("linux.dolatest"),
                "页面加载中",
                Some("当前页面 https://ocr.example/page"),
                Some("https://ocr.example/page"),
            ),
            (None, "页面加载中", None, None),
        ];

        for (browser_url, title, ocr_text, expected) in cases {
            assert_eq!(
                resolve_browser_page_hint(browser_url, title, ocr_text).as_deref(),
                expected,
                "网址={browser_url:?}，标题={title}，OCR={ocr_text:?}"
            );
        }
    }

    #[test]
    fn 网站域名规则应规范化后精确匹配() {
        assert_eq!(
            normalize_domain_rule(" HTTPS://Docs.Example.com:443/path?q=1 "),
            Some("docs.example.com".to_string())
        );
        assert_eq!(normalize_domain_rule("   "), None);

        let rules = vec![
            crate::config::WebsiteSemanticRule {
                domain: "docs.example.com".to_string(),
                semantic_category: " 资料阅读 ".to_string(),
            },
            crate::config::WebsiteSemanticRule {
                domain: "example.com".to_string(),
                semantic_category: "资料调研".to_string(),
            },
        ];

        assert_eq!(
            find_website_semantic_override(&rules, Some("https://docs.example.com/guide")),
            Some("资料阅读".to_string())
        );
        assert_eq!(
            find_website_semantic_override(&rules, Some("https://api.example.com")),
            None
        );
    }

    #[test]
    fn normalize_display_app_name_covers_collector_specific_aliases() {
        assert_eq!(
            normalize_display_app_name("com.apple.SafariPlatformSupport.Helper"),
            "Safari"
        );
        assert_eq!(
            normalize_display_app_name("com.apple.WebKit.Networking"),
            "Safari"
        );
        assert_eq!(normalize_display_app_name("datagrip64"), "DataGrip");
        assert_eq!(normalize_display_app_name("ninjaclean"), "Ninja Clean");
        assert_eq!(normalize_display_app_name("eqmac"), "eqMac");
        assert_eq!(normalize_display_app_name("yabai"), "yabai");
        assert_eq!(normalize_display_app_name("Safari"), "Safari");
    }
}
