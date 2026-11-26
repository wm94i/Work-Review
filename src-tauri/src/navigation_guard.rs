//! WebView 导航守卫：只允许主界面 / 桌宠 WebView 停留在应用自身地址。
//!
//! 前端 Markdown（助手回复、日报）中的链接已在点击时拦截并交给系统浏览器；
//! 这里作为第二道防线，阻止任何把 WebView 导航到外部页面的行为
//! （外部页面一旦载入主 WebView，就能直接调用已授权的 IPC 命令）。

/// 是否为应用自身地址。发布版只认当前平台的 Tauri 内置协议；调试版额外允许 Vite 开发服务器端口。
/// `data:` / `blob:` / `asset:` / 非 `about:blank` 及带自定义端口的 `tauri.localhost` 一律拒绝。
pub fn is_allowed_app_navigation(url: &tauri::Url) -> bool {
    is_allowed_app_navigation_for(url, cfg!(windows), cfg!(debug_assertions))
}

fn is_allowed_app_navigation_for(
    url: &tauri::Url,
    is_windows: bool,
    allow_dev_server: bool,
) -> bool {
    match url.scheme() {
        "about" => url.as_str() == "about:blank",
        "tauri" => !is_windows && url.host_str() == Some("localhost") && url.port().is_none(),
        "http" | "https" => {
            (is_windows && url.host_str() == Some("tauri.localhost") && url.port().is_none())
                || (allow_dev_server
                    && url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
                    && url.port() == Some(5173))
        }
        _ => false,
    }
}

pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("navigation-guard")
        .on_navigation(|_webview, url| {
            let allowed = is_allowed_app_navigation(url);
            if !allowed {
                // 只记录协议与主机，避免把 URL 中可能夹带的数据写进日志
                log::warn!(
                    "已阻止 WebView 导航到外部地址: {}://{}",
                    url.scheme(),
                    url.host_str().unwrap_or("")
                );
            }
            allowed
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::{is_allowed_app_navigation, is_allowed_app_navigation_for};

    fn allowed(raw: &str) -> bool {
        is_allowed_app_navigation(&tauri::Url::parse(raw).unwrap())
    }

    fn allowed_for(raw: &str, is_windows: bool, allow_dev: bool) -> bool {
        is_allowed_app_navigation_for(&tauri::Url::parse(raw).unwrap(), is_windows, allow_dev)
    }

    #[test]
    fn 应用自身地址按平台允许导航() {
        // Windows：仅 http(s)://tauri.localhost（无自定义端口）与 about:blank
        assert!(allowed_for("http://tauri.localhost/#/report", true, false));
        assert!(allowed_for("https://tauri.localhost/", true, false));
        assert!(allowed_for("about:blank", true, false));
        assert!(!allowed_for("tauri://localhost/index.html", true, false));
        assert!(!allowed_for("https://tauri.localhost:8443/", true, false));

        // macOS / Linux：仅 tauri://localhost（无自定义端口）与 about:blank
        assert!(allowed_for(
            "tauri://localhost/index.html#/timeline",
            false,
            false
        ));
        assert!(allowed_for("about:blank", false, false));
        assert!(!allowed_for("https://tauri.localhost/", false, false));
        assert!(!allowed_for("tauri://localhost:1234/", false, false));
    }

    #[test]
    fn 外部地址与非应用内部协议不允许导航() {
        assert!(!allowed("https://example.com/"));
        assert!(!allowed("http://evil.tauri.localhost.example.com/"));
        assert!(!allowed("file:///C:/Windows/System32/"));
        assert!(!allowed("ms-settings:privacy"));
        assert!(!allowed("javascript:alert(1)"));
        assert!(!allowed("data:text/html,<script>alert(1)</script>"));
        assert!(!allowed("about:srcdoc"));
        assert!(!allowed("asset://localhost/image.png"));
        assert!(!allowed("https://asset.localhost/image.png"));
        assert!(!allowed("https://ipc.localhost/"));
    }

    #[test]
    fn 开发服务器地址仅调试构建且仅限5173端口允许() {
        assert_eq!(allowed("http://127.0.0.1:5173/"), cfg!(debug_assertions));
        assert_eq!(allowed("http://localhost:5173/"), cfg!(debug_assertions));
        assert!(!allowed_for("http://127.0.0.1:5173/", true, false));
        // 即使在调试构建也不允许跳往本机其他端口（如 localhost API 47831）
        assert!(!allowed_for(
            "http://127.0.0.1:47831/v1/context",
            true,
            true
        ));
        assert!(!allowed_for("https://127.0.0.1:5173/", true, true));
    }
}
