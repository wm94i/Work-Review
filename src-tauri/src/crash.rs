//! 崩溃栈捕获、平台探测与 Linux WebKit/Wayland 兼容性环境处理。

use crate::default_data_dir;
#[tauri::command]
pub(crate) fn get_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    return "macos";
    #[cfg(target_os = "windows")]
    return "windows";
    #[cfg(target_os = "linux")]
    return "linux";
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    return "unknown";
}

/// 安装崩溃捕获：panic 时把版本、时间、panic 信息与完整调用栈写入
/// `<数据目录>/crashes/crash-<时间>.log`，方便用户回传后定位闪退根因。
pub(crate) fn install_crash_handler() {
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("<无法获取 panic 信息>");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<未知位置>".to_string());
        let now = chrono::Local::now();
        let report = format!(
            "Work Review crash report\n版本: {}\n时间: {}\nPanic: {}\n位置: {}\n\n调用栈:\n{}\n",
            env!("CARGO_PKG_VERSION"),
            now.format("%Y-%m-%d %H:%M:%S"),
            message,
            location,
            backtrace
        );
        let crash_dir = default_data_dir().join("crashes");
        match std::fs::create_dir_all(&crash_dir).and_then(|_| {
            std::fs::write(
                crash_dir.join(format!("crash-{}.log", now.format("%Y%m%d_%H%M%S"))),
                &report,
            )
        }) {
            Ok(()) => eprintln!("崩溃日志已写入 {}/", crash_dir.display()),
            Err(e) => eprintln!("写入崩溃日志失败: {e}"),
        }
        eprintln!("{report}");
    }));
}

/// Linux: 注入 WebKit/Wayland 兼容性环境变量，规避 webkit2gtk 在 Wayland（尤其
/// KDE Plasma / NVIDIA）下启动即崩的问题（"Error 71 (Protocol error) dispatching
/// to Wayland display"，上游 Tauri #10702 / GTK WebKitGTK）。必须在 GTK/WebKit
/// 初始化前调用；仅当用户未显式设置时注入，便于自行覆盖或换用 GDK_BACKEND=x11。
#[cfg(target_os = "linux")]
pub(crate) fn apply_linux_webkit_wayland_workarounds() {
    // webkit2gtk 的 DMA-BUF 渲染器在部分 Wayland compositor 上会触发协议错误，
    // 关闭后回退到传统渲染路径，是 Tauri 社区对 #10702 最普遍有效的 workaround。
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}
