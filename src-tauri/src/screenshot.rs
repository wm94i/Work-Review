#[cfg(target_os = "linux")]
use crate::linux_session::{current_linux_desktop_environment, current_linux_desktop_session};
#[cfg(any(target_os = "linux", test))]
use crate::linux_session::{LinuxDesktopEnvironment, LinuxDesktopSession};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
use image::DynamicImage;
#[cfg(target_os = "macos")]
use image::RgbaImage;
use image::{imageops::FilterType, ColorType};
#[cfg(any(target_os = "linux", test))]
use regex::Regex;
use std::io::Cursor;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Duration;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};
use work_review_core::config::{
    PrivacyConfig, ScreenshotDisplayMode, ScreenshotWidthMode, StorageConfig,
};
use work_review_core::error::{AppError, Result};

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxScreenshotSupport {
    pub provider: &'static str,
    pub supported: bool,
}

#[cfg(target_os = "linux")]
impl LinuxScreenshotSupport {
    const fn supported(provider: &'static str) -> Self {
        Self {
            provider,
            supported: true,
        }
    }

    const fn unsupported() -> Self {
        Self {
            provider: "none",
            supported: false,
        }
    }
}

/// 检查 macOS 屏幕录制权限（不触发授权弹窗）
/// 使用 CGPreflightScreenCaptureAccess (macOS 10.15+)
/// 返回 true 表示已授权，false 表示未授权
#[cfg(target_os = "macos")]
pub fn has_screen_capture_permission() -> bool {
    // CGPreflightScreenCaptureAccess: 仅检查，不弹窗
    // macOS 10.15+ 提供，10.14 及以下默认返回 true（不需要权限）
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 请求 macOS 屏幕录制权限（触发系统弹窗引导用户到设置）
/// 使用 CGRequestScreenCaptureAccess (macOS 10.15+)
/// 注意：此函数仅触发系统提示，用户需要手动在系统设置中授权后重启应用
#[cfg(target_os = "macos")]
pub fn request_screen_capture_permission() {
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe {
        CGRequestScreenCaptureAccess();
    }
}

/// 检查 macOS 辅助功能（Accessibility）权限
/// AppleScript 读取窗口标题、浏览器 URL 均需要此权限
/// prompt=true 时弹出系统授权引导
#[cfg(target_os = "macos")]
pub fn has_accessibility_permission(prompt: bool) -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    extern "C" {
        fn AXIsProcessTrustedWithOptions(
            options: core_foundation::dictionary::CFDictionaryRef,
        ) -> bool;
    }

    if prompt {
        let key = CFString::new("AXTrustedCheckOptionPrompt");
        let value = CFBoolean::true_value();
        let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
        unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
    } else {
        let key = CFString::new("AXTrustedCheckOptionPrompt");
        let value = CFBoolean::false_value();
        let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
        unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
    }
}

/// 检查 macOS 输入监控（Input Monitoring）权限
/// 桌宠全局键盘鼠标联动需要此权限
#[cfg(target_os = "macos")]
pub fn has_input_monitoring_permission() -> bool {
    extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
    }

    unsafe { CGPreflightListenEventAccess() }
}

/// 请求 macOS 输入监控（Input Monitoring）权限
/// 会触发系统引导，用户需要在系统设置中手动授权
#[cfg(target_os = "macos")]
pub fn request_input_monitoring_permission() {
    extern "C" {
        fn CGRequestListenEventAccess() -> bool;
    }

    unsafe {
        CGRequestListenEventAccess();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn has_screen_capture_permission() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn has_accessibility_permission(_prompt: bool) -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn has_input_monitoring_permission() -> bool {
    true
}

/// 截屏结果
#[derive(Debug, Clone)]
pub struct ScreenshotResult {
    /// 归档截图路径（长期保留）
    pub path: PathBuf,
    /// OCR 临时源图路径（识别后可删除）
    pub ocr_source_path: Option<PathBuf>,
    pub timestamp: i64,
    pub width: u32,
    pub height: u32,
    /// 采集时在内存中计算好的 64-bit dHash，供空闲判定与去重复用（避免二次磁盘解码）
    pub image_hash: Option<u64>,
}

/// 合并路径轻量化截屏结果（不落盘归档 JPEG，仅在需要 OCR 且画面变化时写出临时 OCR PNG）
#[derive(Debug, Clone)]
pub struct MergeCaptureResult {
    pub image_hash: u64,
    pub ocr_source_path: Option<PathBuf>,
    #[allow(dead_code)]
    pub timestamp: i64,
}

/// 截屏服务配置
#[derive(Clone)]
pub struct ScreenshotConfig {
    /// 最大宽度（超过此宽度会按比例缩放）
    pub max_width: u32,
    /// JPEG 质量 (1-100)
    pub jpeg_quality: u8,
    /// 截图范围模式
    pub display_mode: ScreenshotDisplayMode,
    /// 截图宽度模式
    pub width_mode: ScreenshotWidthMode,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            max_width: 1440,
            jpeg_quality: 70,
            display_mode: ScreenshotDisplayMode::ActiveWindow,
            width_mode: ScreenshotWidthMode::Auto,
        }
    }
}

impl From<&StorageConfig> for ScreenshotConfig {
    fn from(value: &StorageConfig) -> Self {
        Self {
            max_width: value.max_image_width.max(320),
            jpeg_quality: value.jpeg_quality.clamp(30, 95),
            display_mode: value.screenshot_display_mode,
            width_mode: value.screenshot_width_mode,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
const SCREENSHOT_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_screenshot_command_with_timeout(command: &mut Command, context: &str) -> Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| AppError::Screenshot(format!("{context} 启动失败: {error}")))?;

    // 在独立线程持续排空 stdout/stderr，既保留 xrandr / gdbus 等查询输出，又避免管道写满阻塞子进程。
    let stdout_handle = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_handle = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });

    let started_at = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started_at.elapsed() < SCREENSHOT_COMMAND_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::Screenshot(format!(
                    "{context} 执行超时（>{}s）",
                    SCREENSHOT_COMMAND_TIMEOUT.as_secs()
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::Screenshot(format!("{context} 等待失败: {error}")));
            }
        }
    };

    let stdout = stdout_handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let stderr = stderr_handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// 截屏服务
#[derive(Clone)]
pub struct ScreenshotService {
    data_dir: PathBuf,
    config: ScreenshotConfig,
    capture_lock: Arc<Mutex<()>>,
    /// 截图遮罩所用的隐私规则（随配置保存同步更新）。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    privacy: PrivacyConfig,
}

impl ScreenshotService {
    pub fn new(data_dir: &Path, storage_config: &StorageConfig) -> Self {
        let _ = cleanup_stale_ocr_temp_dir(data_dir);
        Self {
            data_dir: data_dir.to_path_buf(),
            config: ScreenshotConfig::from(storage_config),
            capture_lock: Arc::new(Mutex::new(())),
            privacy: PrivacyConfig::default(),
        }
    }

    pub fn update_config(&mut self, storage_config: &StorageConfig) {
        self.config = ScreenshotConfig::from(storage_config);
    }

    /// 更新截图遮罩所用的隐私规则。
    pub fn update_privacy(&mut self, privacy: &PrivacyConfig) {
        self.privacy = privacy.clone();
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    pub fn capture_for_window(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
    ) -> Result<ScreenshotResult> {
        self.capture_for_window_with_options(active_window, true)
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    pub fn capture_for_window_with_options(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        let _capture_guard = self
            .capture_lock
            .lock()
            .map_err(|e| AppError::Screenshot(format!("截图任务锁异常: {e}")))?;
        self.capture_impl(active_window, need_ocr_source)
    }

    /// 合并路径专用采集：仅在内存计算 dHash 做空闲判定与画面去重，不编码/删除归档 JPEG，
    /// 只有 `need_ocr == true` 且画面相似度 <= 90% 时才落盘临时 OCR PNG。
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    pub fn capture_for_merge_ocr(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        last_hash: u64,
        need_ocr: bool,
    ) -> Result<MergeCaptureResult> {
        let _capture_guard = self
            .capture_lock
            .lock()
            .map_err(|e| AppError::Screenshot(format!("截图任务锁异常: {e}")))?;
        self.capture_merge_impl(active_window, last_hash, need_ocr)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    pub fn capture_for_window(
        &self,
        _active_window: Option<&crate::monitor::ActiveWindow>,
    ) -> Result<ScreenshotResult> {
        let _capture_guard = self
            .capture_lock
            .lock()
            .map_err(|e| AppError::Screenshot(format!("截图任务锁异常: {e}")))?;
        self.capture_impl()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    pub fn capture_for_window_with_options(
        &self,
        _active_window: Option<&crate::monitor::ActiveWindow>,
        _need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        self.capture_for_window(None)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    pub fn capture_for_merge_ocr(
        &self,
        _active_window: Option<&crate::monitor::ActiveWindow>,
        _last_hash: u64,
        _need_ocr: bool,
    ) -> Result<MergeCaptureResult> {
        Err(AppError::Screenshot("当前平台不支持截屏".to_string()))
    }

    #[allow(dead_code)]
    pub fn capture(&self) -> Result<ScreenshotResult> {
        self.capture_for_window(None)
    }

    /// 执行截屏（Windows）
    /// 优先使用 GDI BitBlt（完全静默，无隐私边框闪烁）
    /// 失败时降级到 Windows Graphics Capture API（Win11 会显示黄色边框）
    #[cfg(target_os = "windows")]
    fn capture_impl(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        // 生成文件路径
        let now = chrono::Local::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let time_str = now.format("%H%M%S_%3f").to_string();

        let screenshots_dir = self.data_dir.join("screenshots").join(&date_str);
        std::fs::create_dir_all(&screenshots_dir)?;

        // 截图前先记录一次敏感区域：截图期间窗口移动 / 置顶也不会漏遮（截图后再取一次并集）
        let target_window = if should_capture_all_displays(&self.config) {
            None
        } else {
            active_window
        };
        let pre_capture_regions = self.sensitive_regions(target_window)?;

        // 优先使用 GDI BitBlt（静默，无边框闪烁）
        match self.capture_with_gdi(target_window) {
            Ok((mut pixels, width, height, origin_x, origin_y)) => {
                self.mask_sensitive_windows(
                    &mut pixels,
                    width,
                    height,
                    origin_x,
                    origin_y,
                    &pre_capture_regions,
                )?;
                return self.persist_rgba_capture(
                    pixels,
                    width,
                    height,
                    &screenshots_dir,
                    &time_str,
                    now.timestamp(),
                    need_ocr_source,
                );
            }
            Err(e) if target_window.is_none() && should_capture_all_displays(&self.config) => {
                return Err(AppError::Screenshot(format!("全屏幕截图失败: {e}")));
            }
            Err(e) => {
                log::warn!("GDI BitBlt 失败: {e}，降级到 Windows Graphics Capture");
            }
        }

        // WGC 降级路径产出的是文件，不经过像素遮罩：截图前后任一时刻截图范围内有敏感窗口，
        // 都放弃本次截图。
        const WGC_SKIP_MESSAGE: &str =
            "截图范围内存在隐私规则要求遮挡的窗口，降级截图路径无法遮罩，已跳过本次截图";
        if !pre_capture_regions.is_empty() {
            return Err(AppError::Screenshot(WGC_SKIP_MESSAGE.to_string()));
        }

        // 降级使用 Windows Graphics Capture API（可能显示黄色边框）
        match self.capture_with_wgc(&screenshots_dir, &time_str, active_window) {
            Ok(result) => {
                let _temp_guard = TemporaryCaptureFile::new(&result.0);
                if !self.sensitive_regions(active_window)?.is_empty() {
                    return Err(AppError::Screenshot(WGC_SKIP_MESSAGE.to_string()));
                }
                self.persist_existing_png_capture(
                    &result.0,
                    &screenshots_dir,
                    &time_str,
                    now.timestamp(),
                    need_ocr_source,
                )
            }
            Err(e) => {
                log::warn!("Windows Graphics Capture 也失败: {e}");
                Err(AppError::Screenshot(format!(
                    "截图失败（GDI 和 WGC 均不可用）: {e}"
                )))
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn capture_merge_impl(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        last_hash: u64,
        need_ocr: bool,
    ) -> Result<MergeCaptureResult> {
        let now = chrono::Local::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let time_str = now.format("%H%M%S_%3f").to_string();
        let screenshots_dir = self.data_dir.join("screenshots").join(&date_str);

        let target_window = if should_capture_all_displays(&self.config) {
            None
        } else {
            active_window
        };
        let pre_capture_regions = self.sensitive_regions(target_window)?;

        if let Ok((mut pixels, width, height, origin_x, origin_y)) =
            self.capture_with_gdi(target_window)
        {
            self.mask_sensitive_windows(
                &mut pixels,
                width,
                height,
                origin_x,
                origin_y,
                &pre_capture_regions,
            )?;
            let img = image::RgbaImage::from_raw(width, height, pixels)
                .ok_or_else(|| AppError::Screenshot("创建图像失败".to_string()))?;
            let dynamic_image = DynamicImage::ImageRgba8(img);
            return self.finalize_merge_dynamic_image(
                dynamic_image,
                &screenshots_dir,
                &time_str,
                now.timestamp(),
                last_hash,
                need_ocr,
            );
        }

        // GDI 失败时回退常规采集并在去重后删除临时归档图
        let fallback = self.capture_impl(active_window, need_ocr)?;
        let image_hash = fallback.image_hash.unwrap_or(0);
        let keep_ocr =
            need_ocr && (last_hash == 0 || Self::hash_similarity(last_hash, image_hash) <= 90);
        let ocr_source_path = if keep_ocr {
            fallback.ocr_source_path.clone()
        } else {
            if let Some(ref p) = fallback.ocr_source_path {
                let _ = std::fs::remove_file(p);
            }
            None
        };
        let _ = std::fs::remove_file(&fallback.path);
        let thumb = Self::thumbnail_cache_path(&fallback.path);
        if thumb != fallback.path {
            let _ = std::fs::remove_file(&thumb);
        }
        Ok(MergeCaptureResult {
            image_hash,
            ocr_source_path,
            timestamp: fallback.timestamp,
        })
    }

    /// 使用 Windows Graphics Capture API 截屏
    #[cfg(target_os = "windows")]
    fn capture_with_wgc(
        &self,
        screenshots_dir: &Path,
        time_str: &str,
        active_window: Option<&crate::monitor::ActiveWindow>,
    ) -> Result<(PathBuf, u32, u32)> {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };
        use windows_capture::{
            capture::GraphicsCaptureApiHandler,
            frame::Frame,
            graphics_capture_api::InternalCaptureControl,
            monitor::Monitor,
            settings::{
                ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
                MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
            },
        };

        let temp_png = screenshots_dir.join(format!("{time_str}_temp.png"));
        let mut temp_guard = TemporaryCaptureFile::new(&temp_png);

        struct CaptureResult {
            success: bool,
            error: Option<String>,
            width: u32,
            height: u32,
        }

        let result = Arc::new(Mutex::new(CaptureResult {
            success: false,
            error: None,
            width: 0,
            height: 0,
        }));
        let captured = Arc::new(AtomicBool::new(false));

        struct SingleFrameCapture {
            result: Arc<Mutex<CaptureResult>>,
            captured: Arc<AtomicBool>,
            output_path: PathBuf,
        }

        impl GraphicsCaptureApiHandler for SingleFrameCapture {
            type Flags = (Arc<Mutex<CaptureResult>>, Arc<AtomicBool>, PathBuf);
            type Error = Box<dyn std::error::Error + Send + Sync>;

            fn new(
                ctx: windows_capture::capture::Context<Self::Flags>,
            ) -> std::result::Result<Self, Self::Error> {
                Ok(Self {
                    result: ctx.flags.0,
                    captured: ctx.flags.1,
                    output_path: ctx.flags.2,
                })
            }

            fn on_frame_arrived(
                &mut self,
                frame: &mut Frame,
                capture_control: InternalCaptureControl,
            ) -> std::result::Result<(), Self::Error> {
                if self.captured.load(Ordering::SeqCst) {
                    capture_control.stop();
                    return Ok(());
                }

                self.captured.store(true, Ordering::SeqCst);

                let width = frame.width();
                let height = frame.height();

                use windows_capture::frame::ImageFormat;
                match frame.save_as_image(&self.output_path, ImageFormat::Png) {
                    Ok(()) => {
                        if let Ok(mut r) = self.result.lock() {
                            r.success = true;
                            r.width = width;
                            r.height = height;
                        }
                    }
                    Err(e) => {
                        if let Ok(mut r) = self.result.lock() {
                            r.error = Some(format!("{}", e));
                        }
                    }
                }

                capture_control.stop();
                Ok(())
            }

            fn on_closed(&mut self) -> std::result::Result<(), Self::Error> {
                Ok(())
            }
        }

        let target_monitor = capture_target_monitor(active_window)
            .or_else(|| Monitor::primary().ok())
            .ok_or_else(|| AppError::Screenshot("获取目标显示器失败".to_string()))?;

        // 尝试 WithoutBorder
        let flags = (result.clone(), captured.clone(), temp_png.clone());
        let settings = Settings::new(
            target_monitor,
            CursorCaptureSettings::WithCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            flags,
        );

        let capture_handle = std::thread::spawn(move || SingleFrameCapture::start(settings));

        let first_attempt = match capture_handle.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(format!("{e}")),
            Err(_) => Err("捕获线程异常".to_string()),
        };

        // 首次失败时降级到 WithBorder
        if let Err(ref first_err) = first_attempt {
            log::debug!("WithoutBorder 失败: {first_err}，尝试 WithBorder");

            {
                let mut r = result
                    .lock()
                    .map_err(|_| AppError::Screenshot("锁错误".to_string()))?;
                r.success = false;
                r.error = None;
            }
            captured.store(false, Ordering::SeqCst);

            let flags2 = (result.clone(), captured.clone(), temp_png.clone());
            let settings2 = Settings::new(
                target_monitor,
                CursorCaptureSettings::WithCursor,
                DrawBorderSettings::WithBorder,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                flags2,
            );

            let capture_handle2 = std::thread::spawn(move || SingleFrameCapture::start(settings2));

            match capture_handle2.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(AppError::Screenshot(format!("WithBorder 也失败: {e}"))),
                Err(_) => return Err(AppError::Screenshot("捕获线程异常".to_string())),
            }
        }

        let (success, error_msg, width, height) = {
            let r = result
                .lock()
                .map_err(|_| AppError::Screenshot("锁错误".to_string()))?;
            (r.success, r.error.clone(), r.width, r.height)
        };

        if !success {
            let msg = error_msg.unwrap_or_else(|| "未知错误".to_string());
            return Err(AppError::Screenshot(msg));
        }

        temp_guard.disarm();
        Ok((temp_png, width, height))
    }

    /// 使用 GDI BitBlt 截屏（Windows 10 兼容方案）
    #[cfg(target_os = "windows")]
    fn capture_with_gdi(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
    ) -> Result<(Vec<u8>, u32, u32, i32, i32)> {
        use std::ptr::null_mut;
        use winapi::um::wingdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits,
            SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
        };
        use winapi::um::winuser::{GetDC, ReleaseDC};

        // 与遮罩计算共用同一套截图范围，保证像素坐标与窗口坐标对齐
        let (source_x, source_y, width, height) = self.capture_rect(active_window);

        unsafe {
            if width == 0 || height == 0 {
                return Err(AppError::Screenshot("获取屏幕尺寸失败".to_string()));
            }
            let pixel_bytes = (width as usize)
                .checked_mul(height as usize)
                .and_then(|px| px.checked_mul(4))
                .ok_or_else(|| AppError::Screenshot("截图尺寸溢出".to_string()))?;

            // 获取屏幕 DC
            let screen_dc = GetDC(null_mut());
            if screen_dc.is_null() {
                return Err(AppError::Screenshot("获取屏幕 DC 失败".to_string()));
            }

            // 创建兼容 DC
            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_null() {
                ReleaseDC(null_mut(), screen_dc);
                return Err(AppError::Screenshot("创建兼容 DC 失败".to_string()));
            }

            // 创建兼容位图
            let bitmap = CreateCompatibleBitmap(screen_dc, width as i32, height as i32);
            if bitmap.is_null() {
                DeleteDC(mem_dc);
                ReleaseDC(null_mut(), screen_dc);
                return Err(AppError::Screenshot("创建位图失败".to_string()));
            }

            // 选择位图到内存 DC
            let old_bitmap = SelectObject(mem_dc, bitmap as *mut _);

            // 复制屏幕内容
            let blt_result = BitBlt(
                mem_dc,
                0,
                0,
                width as i32,
                height as i32,
                screen_dc,
                source_x,
                source_y,
                SRCCOPY,
            );

            if blt_result == 0 {
                SelectObject(mem_dc, old_bitmap);
                DeleteObject(bitmap as *mut _);
                DeleteDC(mem_dc);
                ReleaseDC(null_mut(), screen_dc);
                return Err(AppError::Screenshot("BitBlt 失败".to_string()));
            }

            // 准备获取像素数据
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32), // 负值表示自上而下
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [std::mem::zeroed(); 1],
            };

            let mut pixels: Vec<u8> = vec![0; pixel_bytes];

            let lines = GetDIBits(
                mem_dc,
                bitmap,
                0,
                height,
                pixels.as_mut_ptr() as *mut _,
                &mut bmi,
                DIB_RGB_COLORS,
            );

            // 清理资源
            SelectObject(mem_dc, old_bitmap);
            DeleteObject(bitmap as *mut _);
            DeleteDC(mem_dc);
            ReleaseDC(null_mut(), screen_dc);

            if lines == 0 {
                return Err(AppError::Screenshot("GetDIBits 失败".to_string()));
            }

            // BGRA -> RGBA
            for chunk in pixels.as_chunks_mut::<4>().0 {
                chunk.swap(0, 2); // B <-> R
            }

            log::info!(
                "GDI 截图成功: {}x{} @ ({}, {})",
                width,
                height,
                source_x,
                source_y
            );
            Ok((pixels, width, height, source_x, source_y))
        }
    }

    /// 计算当前截图范围（虚拟屏幕坐标）：全部显示器模式为整个虚拟屏幕，否则为目标显示器。
    #[cfg(target_os = "windows")]
    fn capture_rect(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
    ) -> (i32, i32, u32, u32) {
        use winapi::um::winuser::{
            GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
            SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        };
        unsafe {
            if should_capture_all_displays(&self.config) {
                (
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    GetSystemMetrics(SM_CXVIRTUALSCREEN).max(0) as u32,
                    GetSystemMetrics(SM_CYVIRTUALSCREEN).max(0) as u32,
                )
            } else {
                capture_target_monitor_rect(active_window).unwrap_or_else(|| {
                    (
                        0,
                        0,
                        GetSystemMetrics(SM_CXSCREEN).max(0) as u32,
                        GetSystemMetrics(SM_CYSCREEN).max(0) as u32,
                    )
                })
            }
        }
    }

    /// 把截图范围内可见的敏感窗口（隐私级别非"完整记录"或标题命中敏感关键词）涂黑。
    /// 遮罩区域 = 截图前记录的区域 ∪ 截图后重新计算的区域，覆盖截图期间窗口移动 / 置顶。
    #[cfg(target_os = "windows")]
    fn mask_sensitive_windows(
        &self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        origin_x: i32,
        origin_y: i32,
        pre_capture_regions: &[crate::screenshot_mask::MaskRect],
    ) -> Result<()> {
        let capture = crate::screenshot_mask::MaskRect::new(
            origin_x,
            origin_y,
            origin_x.saturating_add(width as i32),
            origin_y.saturating_add(height as i32),
        );
        let mut regions = crate::screenshot_mask::sensitive_regions_in(&self.privacy, &capture)
            .ok_or_else(window_enumeration_truncated_error)?;
        regions.extend_from_slice(pre_capture_regions);
        if regions.is_empty() {
            return Ok(());
        }
        let painted = crate::screenshot_mask::apply_mask_rgba(
            pixels, width, height, origin_x, origin_y, &regions,
        );
        log::info!(
            "截图隐私遮罩: 遮挡 {} 个区域，共 {} 像素",
            regions.len(),
            painted
        );
        Ok(())
    }

    /// 截图范围内需要遮罩的区域（窗口枚举被截断时返回错误，放弃本次截图）。
    #[cfg(target_os = "windows")]
    fn sensitive_regions(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
    ) -> Result<Vec<crate::screenshot_mask::MaskRect>> {
        let (x, y, width, height) = self.capture_rect(active_window);
        let capture = crate::screenshot_mask::MaskRect::new(
            x,
            y,
            x.saturating_add(width as i32),
            y.saturating_add(height as i32),
        );
        crate::screenshot_mask::sensitive_regions_in(&self.privacy, &capture)
            .ok_or_else(window_enumeration_truncated_error)
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn persist_existing_png_capture(
        &self,
        temp_png: &Path,
        screenshots_dir: &Path,
        time_str: &str,
        timestamp: i64,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        let (archive_path, ocr_source_path) =
            capture_output_paths(&self.data_dir, screenshots_dir, time_str);
        if need_ocr_source {
            ensure_parent_dir(&ocr_source_path)?;
        }
        let persist_result = (|| -> Result<(u32, u32, u64, Option<PathBuf>)> {
            let source_for_decode = if need_ocr_source {
                if temp_png != ocr_source_path {
                    move_or_copy_file(temp_png, &ocr_source_path)?;
                }
                ocr_source_path.as_path()
            } else {
                temp_png
            };

            let image = image::open(source_for_decode)
                .map_err(|e| AppError::Screenshot(format!("读取截图失败: {e}")))?;
            let image_hash = Self::compute_dhash_from_dynamic_image(&image);
            let archive_image = prepare_archive_image_with_config(image, &self.config);
            let width = archive_image.width();
            let height = archive_image.height();

            save_archive_jpeg_with_quality(
                &archive_image,
                &archive_path,
                self.config.jpeg_quality,
            )?;
            save_thumbnail_cache_from_image(&archive_image, &archive_path);
            Ok((
                width,
                height,
                image_hash,
                need_ocr_source.then_some(ocr_source_path.clone()),
            ))
        })();
        let (width, height, image_hash, saved_ocr_path) = match persist_result {
            Ok(dimensions) => dimensions,
            Err(error) => {
                cleanup_failed_capture_outputs(&archive_path, &ocr_source_path);
                return Err(error);
            }
        };

        let file_size = std::fs::metadata(&archive_path)
            .map(|m| m.len())
            .unwrap_or(0);
        log::info!(
            "截图归档到: {:?} ({}x{}, {} KB)",
            archive_path,
            width,
            height,
            file_size / 1024
        );

        Ok(ScreenshotResult {
            path: archive_path,
            ocr_source_path: saved_ocr_path,
            timestamp,
            width,
            height,
            image_hash: Some(image_hash),
        })
    }

    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    fn persist_dynamic_image_capture(
        &self,
        dynamic_image: DynamicImage,
        screenshots_dir: &Path,
        time_str: &str,
        timestamp: i64,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        let (archive_path, ocr_source_path) =
            capture_output_paths(&self.data_dir, screenshots_dir, time_str);
        if need_ocr_source {
            ensure_parent_dir(&ocr_source_path)?;
        }
        let persist_result = (|| -> Result<(u32, u32, u64, Option<PathBuf>)> {
            let image_hash = Self::compute_dhash_from_dynamic_image(&dynamic_image);
            let saved_ocr_path = if need_ocr_source {
                dynamic_image
                    .save_with_format(&ocr_source_path, image::ImageFormat::Png)
                    .map_err(|e| AppError::Screenshot(format!("保存 OCR 临时图失败: {e}")))?;
                Some(ocr_source_path.clone())
            } else {
                None
            };

            let archive_image = prepare_archive_image_with_config(dynamic_image, &self.config);
            let width = archive_image.width();
            let height = archive_image.height();
            save_archive_jpeg_with_quality(
                &archive_image,
                &archive_path,
                self.config.jpeg_quality,
            )?;
            save_thumbnail_cache_from_image(&archive_image, &archive_path);
            Ok((width, height, image_hash, saved_ocr_path))
        })();
        let (width, height, image_hash, saved_ocr_path) = match persist_result {
            Ok(dimensions) => dimensions,
            Err(error) => {
                cleanup_failed_capture_outputs(&archive_path, &ocr_source_path);
                return Err(error);
            }
        };

        let file_size = std::fs::metadata(&archive_path)
            .map(|m| m.len())
            .unwrap_or(0);
        log::info!(
            "截图归档到: {:?} ({}x{}, {} KB)",
            archive_path,
            width,
            height,
            file_size / 1024
        );

        Ok(ScreenshotResult {
            path: archive_path,
            ocr_source_path: saved_ocr_path,
            timestamp,
            width,
            height,
            image_hash: Some(image_hash),
        })
    }

    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    fn finalize_merge_dynamic_image(
        &self,
        dynamic_image: DynamicImage,
        screenshots_dir: &Path,
        time_str: &str,
        timestamp: i64,
        last_hash: u64,
        need_ocr: bool,
    ) -> Result<MergeCaptureResult> {
        let image_hash = Self::compute_dhash_from_dynamic_image(&dynamic_image);
        let should_save_ocr = if !need_ocr {
            false
        } else if last_hash != 0 {
            let similarity = Self::hash_similarity(last_hash, image_hash);
            if similarity > 90 {
                log::debug!("合并截图内存 dHash 相似度 {similarity}%，跳过编码与 OCR");
                false
            } else {
                log::debug!("合并截图内存 dHash 相似度 {similarity}%，写出 OCR 源图");
                true
            }
        } else {
            true
        };

        let ocr_source_path = if should_save_ocr {
            let (_, ocr_path) = capture_output_paths(&self.data_dir, screenshots_dir, time_str);
            ensure_parent_dir(&ocr_path)?;
            dynamic_image
                .save_with_format(&ocr_path, image::ImageFormat::Png)
                .map_err(|e| AppError::Screenshot(format!("保存合并 OCR 临时图失败: {e}")))?;
            Some(ocr_path)
        } else {
            None
        };

        Ok(MergeCaptureResult {
            image_hash,
            ocr_source_path,
            timestamp,
        })
    }

    /// 将 RGBA 像素数据转为归档截图（按所有权接管缓冲区，避免额外 `.to_vec()` 复制）
    #[cfg(target_os = "windows")]
    #[allow(clippy::too_many_arguments)]
    fn persist_rgba_capture(
        &self,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        screenshots_dir: &Path,
        time_str: &str,
        timestamp: i64,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        let img = image::RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| AppError::Screenshot("创建图像失败".to_string()))?;
        self.persist_dynamic_image_capture(
            DynamicImage::ImageRgba8(img),
            screenshots_dir,
            time_str,
            timestamp,
            need_ocr_source,
        )
    }

    /// 执行截屏（macOS）
    #[cfg(target_os = "macos")]
    fn capture_impl(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        let now = chrono::Local::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let time_str = now.format("%H%M%S_%3f").to_string();

        let screenshots_dir = self.data_dir.join("screenshots").join(&date_str);
        std::fs::create_dir_all(&screenshots_dir)?;

        let dynamic_image =
            self.capture_dynamic_image_macos(active_window, &screenshots_dir, &time_str)?;

        self.persist_dynamic_image_capture(
            dynamic_image,
            &screenshots_dir,
            &time_str,
            now.timestamp(),
            need_ocr_source,
        )
    }

    #[cfg(target_os = "macos")]
    fn capture_merge_impl(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        last_hash: u64,
        need_ocr: bool,
    ) -> Result<MergeCaptureResult> {
        let now = chrono::Local::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let time_str = now.format("%H%M%S_%3f").to_string();
        let screenshots_dir = self.data_dir.join("screenshots").join(&date_str);
        std::fs::create_dir_all(&screenshots_dir)?;

        let dynamic_image =
            self.capture_dynamic_image_macos(active_window, &screenshots_dir, &time_str)?;
        self.finalize_merge_dynamic_image(
            dynamic_image,
            &screenshots_dir,
            &time_str,
            now.timestamp(),
            last_hash,
            need_ocr,
        )
    }

    #[cfg(target_os = "macos")]
    fn capture_dynamic_image_macos(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        screenshots_dir: &Path,
        time_str: &str,
    ) -> Result<DynamicImage> {
        use screenshots::Screen;

        if should_capture_all_displays(&self.config) {
            self.capture_all_displays_macos(screenshots_dir, time_str)
        } else {
            let screen = if let Some((x, y)) = capture_target_point(active_window) {
                match Screen::from_point(x, y) {
                    Ok(screen) => screen,
                    Err(e) => {
                        log::warn!("按窗口坐标选屏失败，将回退到默认屏幕: {e}");
                        let screens = Screen::all().map_err(|err| {
                            AppError::Screenshot(format!("获取屏幕列表失败: {err}"))
                        })?;
                        screens
                            .first()
                            .copied()
                            .ok_or_else(|| AppError::Screenshot("没有找到屏幕".to_string()))?
                    }
                }
            } else {
                let screens = Screen::all()
                    .map_err(|e| AppError::Screenshot(format!("获取屏幕列表失败: {e}")))?;
                screens
                    .first()
                    .copied()
                    .ok_or_else(|| AppError::Screenshot("没有找到屏幕".to_string()))?
            };

            Ok(DynamicImage::ImageRgba8(
                self.capture_single_display_macos(
                    &screen,
                    screenshots_dir,
                    &format!("{time_str}_display"),
                )?,
            ))
        }
    }

    #[cfg(target_os = "macos")]
    fn capture_single_display_macos(
        &self,
        screen: &screenshots::Screen,
        screenshots_dir: &Path,
        temp_name: &str,
    ) -> Result<RgbaImage> {
        // 优先使用 screenshots crate（CGDisplay::screenshot），不会触发系统隐私边框闪烁
        // screencapture CLI 在 macOS Sonoma/Sequoia 上会显示黄色边框隐私指示器
        match screen.capture() {
            Ok(fallback) => {
                RgbaImage::from_raw(fallback.width(), fallback.height(), fallback.into_raw())
                    .ok_or_else(|| AppError::Screenshot("图像转换失败".to_string()))
            }
            Err(error) => {
                log::warn!("screenshots crate 失败，回退 macOS 原生 screencapture: {error}");
                self.capture_display_with_screencapture_macos(screen, screenshots_dir, temp_name)
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn capture_display_with_screencapture_macos(
        &self,
        screen: &screenshots::Screen,
        screenshots_dir: &Path,
        temp_name: &str,
    ) -> Result<RgbaImage> {
        let temp_png = screenshots_dir.join(format!("{temp_name}.png"));
        let _temp_guard = TemporaryCaptureFile::new(&temp_png);
        let rect = macos_capture_rect(
            screen.display_info.x,
            screen.display_info.y,
            screen.display_info.width,
            screen.display_info.height,
        );

        let output = run_screenshot_command_with_timeout(
            Command::new("screencapture").args([
                "-x",
                "-t",
                "png",
                "-R",
                &rect,
                &temp_png.to_string_lossy(),
            ]),
            "screencapture",
        )?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(AppError::Screenshot(format!(
                "screencapture 截图失败: {}",
                if stderr.is_empty() {
                    "未知错误"
                } else {
                    &stderr
                }
            )));
        }

        let image = image::open(&temp_png)
            .map_err(|e| AppError::Screenshot(format!("读取 screencapture 结果失败: {e}")))?
            .to_rgba8();
        Ok(image)
    }

    #[cfg(target_os = "macos")]
    fn capture_all_displays_macos(
        &self,
        screenshots_dir: &Path,
        time_str: &str,
    ) -> Result<DynamicImage> {
        use screenshots::Screen;

        let screens =
            Screen::all().map_err(|e| AppError::Screenshot(format!("获取屏幕列表失败: {e}")))?;
        if screens.is_empty() {
            return Err(AppError::Screenshot("没有找到屏幕".to_string()));
        }

        let mut captured_images = Vec::new();
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;

        for (index, screen) in screens.into_iter().enumerate() {
            let image = self.capture_single_display_macos(
                &screen,
                screenshots_dir,
                &format!("{time_str}_display_{index}"),
            )?;
            let offset_x =
                display_pixel_offset(screen.display_info.x, screen.display_info.scale_factor);
            let offset_y =
                display_pixel_offset(screen.display_info.y, screen.display_info.scale_factor);
            let width = image.width() as i32;
            let height = image.height() as i32;

            min_x = min_x.min(offset_x);
            min_y = min_y.min(offset_y);
            max_x = max_x.max(offset_x + width);
            max_y = max_y.max(offset_y + height);

            captured_images.push((
                offset_x,
                offset_y,
                width as u32,
                height as u32,
                image.into_raw(),
            ));
        }

        let canvas_width = (max_x - min_x).max(1) as u32;
        let canvas_height = (max_y - min_y).max(1) as u32;
        let mut canvas = RgbaImage::new(canvas_width, canvas_height);

        for (offset_x, offset_y, width, height, raw_pixels) in captured_images {
            let start_x = (offset_x - min_x) as u32;
            let start_y = (offset_y - min_y) as u32;

            for y in 0..height {
                for x in 0..width {
                    let pixel_index = ((y * width + x) * 4) as usize;
                    let pixel = image::Rgba([
                        raw_pixels[pixel_index],
                        raw_pixels[pixel_index + 1],
                        raw_pixels[pixel_index + 2],
                        raw_pixels[pixel_index + 3],
                    ]);
                    canvas.put_pixel(start_x + x, start_y + y, pixel);
                }
            }
        }

        Ok(DynamicImage::ImageRgba8(canvas))
    }

    /// 执行截屏（Linux）
    #[cfg(target_os = "linux")]
    fn capture_impl(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        let now = chrono::Local::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let time_str = now.format("%H%M%S_%3f").to_string();

        let screenshots_dir = self.data_dir.join("screenshots").join(&date_str);
        std::fs::create_dir_all(&screenshots_dir)?;

        let session = current_linux_desktop_session();
        let desktop_environment = current_linux_desktop_environment();

        match session {
            LinuxDesktopSession::Wayland => self.capture_linux_wayland(
                &screenshots_dir,
                &time_str,
                now.timestamp(),
                active_window,
                desktop_environment,
                need_ocr_source,
            ),
            LinuxDesktopSession::X11 | LinuxDesktopSession::Unknown => self.capture_linux_x11(
                &screenshots_dir,
                &time_str,
                now.timestamp(),
                active_window,
                desktop_environment,
                need_ocr_source,
            ),
        }
    }

    #[cfg(target_os = "linux")]
    fn capture_merge_impl(
        &self,
        active_window: Option<&crate::monitor::ActiveWindow>,
        last_hash: u64,
        need_ocr: bool,
    ) -> Result<MergeCaptureResult> {
        let fallback = self.capture_impl(active_window, need_ocr)?;
        let image_hash = fallback.image_hash.unwrap_or(0);
        let keep_ocr =
            need_ocr && (last_hash == 0 || Self::hash_similarity(last_hash, image_hash) <= 90);
        let ocr_source_path = if keep_ocr {
            fallback.ocr_source_path.clone()
        } else {
            if let Some(ref p) = fallback.ocr_source_path {
                let _ = std::fs::remove_file(p);
            }
            None
        };
        let _ = std::fs::remove_file(&fallback.path);
        let thumb = Self::thumbnail_cache_path(&fallback.path);
        if thumb != fallback.path {
            let _ = std::fs::remove_file(&thumb);
        }
        Ok(MergeCaptureResult {
            image_hash,
            ocr_source_path,
            timestamp: fallback.timestamp,
        })
    }

    #[cfg(target_os = "linux")]
    fn maybe_crop_linux_capture(
        &self,
        temp_png: &Path,
        active_window: Option<&crate::monitor::ActiveWindow>,
        session: LinuxDesktopSession,
        desktop_environment: LinuxDesktopEnvironment,
    ) {
        if should_capture_all_displays(&self.config) {
            return;
        }

        if let Err(error) =
            crop_linux_capture_to_rect(temp_png, active_window, session, desktop_environment)
        {
            log::warn!("Linux 按窗口边界裁剪截图失败，回退整桌面截图: {error}");
        }
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_arguments)]
    fn capture_linux_x11(
        &self,
        screenshots_dir: &Path,
        time_str: &str,
        timestamp: i64,
        active_window: Option<&crate::monitor::ActiveWindow>,
        desktop_environment: LinuxDesktopEnvironment,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        let temp_png = screenshots_dir.join(format!("{time_str}_temp.png"));
        let _temp_guard = TemporaryCaptureFile::new(&temp_png);
        let scrot_result = run_screenshot_command_with_timeout(
            Command::new("scrot").args(["-o", &temp_png.to_string_lossy()]),
            "scrot",
        );

        let captured = match scrot_result {
            Ok(output) if output.status.success() && temp_png.exists() => true,
            _ => {
                let import_result = run_screenshot_command_with_timeout(
                    Command::new("import").args([
                        "-window",
                        "root",
                        temp_png.to_string_lossy().as_ref(),
                    ]),
                    "import",
                );

                match import_result {
                    Ok(output) if output.status.success() && temp_png.exists() => true,
                    _ => {
                        let maim_result = run_screenshot_command_with_timeout(
                            Command::new("maim").arg(temp_png.to_string_lossy().to_string()),
                            "maim",
                        );

                        matches!(
                            maim_result,
                            Ok(output) if output.status.success() && temp_png.exists()
                        )
                    }
                }
            }
        };

        if !captured {
            return Err(AppError::Screenshot(
                "截屏失败：请安装 scrot、maim 或 ImageMagick (import)".to_string(),
            ));
        }

        self.maybe_crop_linux_capture(
            &temp_png,
            active_window,
            LinuxDesktopSession::X11,
            desktop_environment,
        );

        self.persist_existing_png_capture(
            &temp_png,
            screenshots_dir,
            time_str,
            timestamp,
            need_ocr_source,
        )
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_arguments)]
    fn capture_linux_wayland(
        &self,
        screenshots_dir: &Path,
        time_str: &str,
        timestamp: i64,
        active_window: Option<&crate::monitor::ActiveWindow>,
        desktop_environment: LinuxDesktopEnvironment,
        need_ocr_source: bool,
    ) -> Result<ScreenshotResult> {
        let temp_png = screenshots_dir.join(format!("{time_str}_temp.png"));
        let _temp_guard = TemporaryCaptureFile::new(&temp_png);
        let output_path = temp_png.to_string_lossy().to_string();

        let candidates = [
            ("grim", vec![output_path.clone()]),
            (
                "gnome-screenshot",
                vec!["-f".to_string(), output_path.clone()],
            ),
            (
                "spectacle",
                vec![
                    "-b".to_string(),
                    "-n".to_string(),
                    "-o".to_string(),
                    output_path.clone(),
                ],
            ),
        ];

        let mut captured = false;

        for (command_name, args) in candidates {
            let output = run_screenshot_command_with_timeout(
                Command::new(command_name).args(&args),
                command_name,
            );
            match output {
                Ok(result) if result.status.success() && temp_png.exists() => {
                    captured = true;
                    break;
                }
                Ok(_) | Err(_) => {}
            }
        }

        if !captured {
            return Err(AppError::Screenshot(
                "Wayland 截图失败：请安装 grim、gnome-screenshot 或 spectacle".to_string(),
            ));
        }

        self.maybe_crop_linux_capture(
            &temp_png,
            active_window,
            LinuxDesktopSession::Wayland,
            desktop_environment,
        );

        self.persist_existing_png_capture(
            &temp_png,
            screenshots_dir,
            time_str,
            timestamp,
            need_ocr_source,
        )
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    fn capture_impl(&self) -> Result<ScreenshotResult> {
        Err(AppError::Screenshot("当前平台不支持截屏".to_string()))
    }

    pub fn get_relative_path(&self, full_path: &Path) -> String {
        full_path
            .strip_prefix(&self.data_dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| full_path.to_string_lossy().to_string())
    }

    /// 计算指定截图路径对应的 400px 磁盘缩略图缓存路径（同目录 `.thumb.jpg`）
    pub fn thumbnail_cache_path(path: &Path) -> PathBuf {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("screenshot");
        path.with_file_name(format!("{stem}.thumb.jpg"))
    }

    pub fn generate_thumbnail_base64(&self, path: &Path, max_size: u32) -> Result<String> {
        let cache_path = (max_size == 400).then(|| Self::thumbnail_cache_path(path));
        if let Some(ref thumb_path) = cache_path {
            if let Ok(cached_bytes) = std::fs::read(thumb_path) {
                if !cached_bytes.is_empty() {
                    return Ok(BASE64_STANDARD.encode(&cached_bytes));
                }
            }
        }

        let img = image::open(path)?;
        let (src_w, src_h) = (img.width(), img.height());
        let thumbnail = if src_w > max_size || src_h > max_size {
            img.resize(max_size, max_size, FilterType::Triangle)
        } else {
            img
        };

        let rgb_thumbnail = thumbnail.to_rgb8();
        let mut buffer = Vec::new();
        let mut cursor = Cursor::new(&mut buffer);
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 60);
        encoder.encode(
            rgb_thumbnail.as_raw(),
            thumbnail.width(),
            thumbnail.height(),
            ColorType::Rgb8.into(),
        )?;

        if let Some(ref thumb_path) = cache_path {
            let _ = std::fs::write(thumb_path, &buffer);
        }

        Ok(BASE64_STANDARD.encode(&buffer))
    }

    pub fn generate_full_image_base64(&self, path: &Path) -> Result<String> {
        let bytes = std::fs::read(path)?;
        Ok(BASE64_STANDARD.encode(bytes))
    }

    /// 在内存中计算 64-bit 水平梯度 dHash（9×8 面积/Triangle 缩小灰度图），避免重解码落盘文件。
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
    pub fn compute_dhash_from_dynamic_image(img: &DynamicImage) -> u64 {
        let small = img.resize_exact(9, 8, FilterType::Triangle).to_luma8();
        let mut hash: u64 = 0;
        let mut bit = 0u32;
        for y in 0..8u32 {
            for x in 0..8u32 {
                let left = small.get_pixel(x, y).0[0];
                let right = small.get_pixel(x + 1, y).0[0];
                if left > right {
                    hash |= 1u64 << bit;
                }
                bit += 1;
            }
        }
        hash
    }

    #[allow(dead_code)]
    pub fn calculate_image_hash(path: &Path) -> Result<u64> {
        let img = image::open(path)?;
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
        {
            Ok(Self::compute_dhash_from_dynamic_image(&img))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", test)))]
        {
            let small = img.resize_exact(8, 8, FilterType::Triangle).to_luma8();
            let sum: u32 = small.pixels().map(|p| p.0[0] as u32).sum();
            let avg = sum / 64;
            let mut hash: u64 = 0;
            for (i, pixel) in small.pixels().enumerate() {
                if pixel.0[0] as u32 > avg {
                    hash |= 1u64 << i;
                }
            }
            Ok(hash)
        }
    }

    pub fn hash_similarity(hash1: u64, hash2: u64) -> u8 {
        let xor = hash1 ^ hash2;
        let diff = xor.count_ones();
        let similarity = (64 - diff) * 100 / 64;
        similarity as u8
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn capture_target_point(
    active_window: Option<&crate::monitor::ActiveWindow>,
) -> Option<(i32, i32)> {
    let bounds = active_window?.window_bounds?;
    if bounds.width == 0 || bounds.height == 0 {
        return None;
    }

    let half_width = i32::try_from(bounds.width / 2).ok()?;
    let half_height = i32::try_from(bounds.height / 2).ok()?;
    Some((
        bounds.x.saturating_add(half_width),
        bounds.y.saturating_add(half_height),
    ))
}

#[cfg(any(target_os = "linux", test))]
type LinuxCaptureRect = (i32, i32, u32, u32);

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_window_rect(
    active_window: Option<&crate::monitor::ActiveWindow>,
) -> Option<LinuxCaptureRect> {
    let bounds = active_window?.window_bounds?;
    if bounds.width == 0 || bounds.height == 0 {
        return None;
    }

    Some((bounds.x, bounds.y, bounds.width, bounds.height))
}

#[cfg(any(target_os = "linux", test))]
fn parse_xrandr_active_monitor_rects(output: &str) -> Vec<LinuxCaptureRect> {
    let regex =
        Regex::new(r"(\d+)/\d+x(\d+)/\d+([+-]\d+)([+-]\d+)").expect("xrandr regex 应可编译");

    regex
        .captures_iter(output)
        .filter_map(|capture| {
            let width = capture.get(1)?.as_str().parse::<u32>().ok()?;
            let height = capture.get(2)?.as_str().parse::<u32>().ok()?;
            let x = capture.get(3)?.as_str().parse::<i32>().ok()?;
            let y = capture.get(4)?.as_str().parse::<i32>().ok()?;
            Some((x, y, width, height))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn run_xrandr_active_monitors() -> Option<Vec<LinuxCaptureRect>> {
    let output = run_screenshot_command_with_timeout(
        Command::new("xrandr").arg("--listactivemonitors"),
        "xrandr",
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }

    let parsed = parse_xrandr_active_monitor_rects(&String::from_utf8_lossy(&output.stdout));
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_gnome_display_config_positions(output: &str) -> Vec<(i32, i32)> {
    let regex =
        Regex::new(r"\((-?\d+), (-?\d+), [0-9.]+, uint32").expect("display config regex 应可编译");

    regex
        .captures_iter(output)
        .filter_map(|capture| {
            Some((
                capture.get(1)?.as_str().parse::<i32>().ok()?,
                capture.get(2)?.as_str().parse::<i32>().ok()?,
            ))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn run_gnome_display_config_positions() -> Option<Vec<(i32, i32)>> {
    let output = run_screenshot_command_with_timeout(
        Command::new("gdbus").args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Mutter.DisplayConfig",
            "--object-path",
            "/org/gnome/Mutter/DisplayConfig",
            "--method",
            "org.gnome.Mutter.DisplayConfig.GetCurrentState",
        ]),
        "gdbus DisplayConfig.GetCurrentState",
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }

    let positions = parse_gnome_display_config_positions(&String::from_utf8_lossy(&output.stdout));
    if positions.is_empty() {
        None
    } else {
        Some(positions)
    }
}

#[cfg(any(target_os = "linux", test))]
#[allow(clippy::too_many_arguments)]
fn linux_screenshot_support_for_session(
    session: LinuxDesktopSession,
    desktop_environment: LinuxDesktopEnvironment,
    scrot_available: bool,
    import_available: bool,
    maim_available: bool,
    grim_available: bool,
    gnome_screenshot_available: bool,
    spectacle_available: bool,
) -> (&'static str, bool) {
    match session {
        LinuxDesktopSession::Wayland => match desktop_environment {
            LinuxDesktopEnvironment::Gnome => {
                if gnome_screenshot_available {
                    ("gnome-screenshot", true)
                } else if grim_available {
                    ("grim", true)
                } else if spectacle_available {
                    ("spectacle", true)
                } else {
                    ("none", false)
                }
            }
            LinuxDesktopEnvironment::Kde => {
                if spectacle_available {
                    ("spectacle", true)
                } else if grim_available {
                    ("grim", true)
                } else if gnome_screenshot_available {
                    ("gnome-screenshot", true)
                } else {
                    ("none", false)
                }
            }
            LinuxDesktopEnvironment::Hyprland
            | LinuxDesktopEnvironment::Sway
            | LinuxDesktopEnvironment::Unknown => {
                if grim_available {
                    ("grim", true)
                } else if gnome_screenshot_available {
                    ("gnome-screenshot", true)
                } else if spectacle_available {
                    ("spectacle", true)
                } else {
                    ("none", false)
                }
            }
        },
        LinuxDesktopSession::X11 | LinuxDesktopSession::Unknown => {
            if scrot_available {
                ("scrot", true)
            } else if import_available {
                ("import", true)
            } else if maim_available {
                ("maim", true)
            } else {
                ("none", false)
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn is_linux_command_available(command: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|unparsed: std::ffi::OsString| {
            std::env::split_paths(&unparsed).collect::<Vec<_>>()
        })
        .map(|dir| dir.join(command))
        .any(|path| {
            std::fs::metadata(&path)
                .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

#[cfg(target_os = "linux")]
pub fn current_linux_screenshot_support() -> LinuxScreenshotSupport {
    let session = current_linux_desktop_session();
    let desktop_environment = current_linux_desktop_environment();
    let (provider, supported) = linux_screenshot_support_for_session(
        session,
        desktop_environment,
        is_linux_command_available("scrot"),
        is_linux_command_available("import"),
        is_linux_command_available("maim"),
        is_linux_command_available("grim"),
        is_linux_command_available("gnome-screenshot"),
        is_linux_command_available("spectacle"),
    );

    if supported {
        LinuxScreenshotSupport::supported(provider)
    } else {
        LinuxScreenshotSupport::unsupported()
    }
}

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rect_contains_point(rect: LinuxCaptureRect, point: (i32, i32)) -> bool {
    let (x, y, width, height) = rect;
    let max_x = x.saturating_add(i32::try_from(width).unwrap_or(i32::MAX));
    let max_y = y.saturating_add(i32::try_from(height).unwrap_or(i32::MAX));
    point.0 >= x && point.0 < max_x && point.1 >= y && point.1 < max_y
}

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn virtual_origin_from_rects(rects: &[LinuxCaptureRect]) -> Option<(i32, i32)> {
    let min_x = rects.iter().map(|(x, _, _, _)| *x).min()?;
    let min_y = rects.iter().map(|(_, y, _, _)| *y).min()?;
    Some((min_x, min_y))
}

#[cfg(target_os = "linux")]
fn linux_virtual_origin(
    session: LinuxDesktopSession,
    desktop_environment: LinuxDesktopEnvironment,
) -> Option<(i32, i32)> {
    match session {
        LinuxDesktopSession::X11 => run_xrandr_active_monitors()
            .as_ref()
            .and_then(|rects| virtual_origin_from_rects(rects)),
        LinuxDesktopSession::Wayland if desktop_environment == LinuxDesktopEnvironment::Gnome => {
            run_gnome_display_config_positions()
                .as_ref()
                .and_then(|positions| {
                    let min_x = positions.iter().map(|(x, _)| *x).min()?;
                    let min_y = positions.iter().map(|(_, y)| *y).min()?;
                    Some((min_x, min_y))
                })
        }
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn linux_capture_target_rect(
    active_window: Option<&crate::monitor::ActiveWindow>,
    session: LinuxDesktopSession,
) -> Option<LinuxCaptureRect> {
    let window_rect = linux_window_rect(active_window)?;

    if session == LinuxDesktopSession::X11 {
        let center = capture_target_point(active_window)?;
        if let Some(monitor_rect) = run_xrandr_active_monitors().and_then(|rects| {
            rects
                .into_iter()
                .find(|rect| rect_contains_point(*rect, center))
        }) {
            return Some(monitor_rect);
        }
    }

    Some(window_rect)
}

#[cfg(any(target_os = "linux", test))]
fn normalize_linux_crop_rect(
    virtual_origin: (i32, i32),
    rect: LinuxCaptureRect,
    image_width: u32,
    image_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let left = rect.0.checked_sub(virtual_origin.0)?;
    let top = rect.1.checked_sub(virtual_origin.1)?;
    let right = left.checked_add(i32::try_from(rect.2).ok()?)?;
    let bottom = top.checked_add(i32::try_from(rect.3).ok()?)?;

    let crop_left = left.max(0).min(i32::try_from(image_width).ok()?);
    let crop_top = top.max(0).min(i32::try_from(image_height).ok()?);
    let crop_right = right.max(0).min(i32::try_from(image_width).ok()?);
    let crop_bottom = bottom.max(0).min(i32::try_from(image_height).ok()?);

    if crop_right <= crop_left || crop_bottom <= crop_top {
        return None;
    }

    Some((
        u32::try_from(crop_left).ok()?,
        u32::try_from(crop_top).ok()?,
        u32::try_from(crop_right - crop_left).ok()?,
        u32::try_from(crop_bottom - crop_top).ok()?,
    ))
}

#[cfg(target_os = "linux")]
fn crop_linux_capture_to_rect(
    temp_png: &Path,
    active_window: Option<&crate::monitor::ActiveWindow>,
    session: LinuxDesktopSession,
    desktop_environment: LinuxDesktopEnvironment,
) -> Result<()> {
    let Some(target_rect) = linux_capture_target_rect(active_window, session) else {
        return Ok(());
    };

    let image = image::open(temp_png)
        .map_err(|e| AppError::Screenshot(format!("读取 Linux 截图失败: {e}")))?;
    let virtual_origin = linux_virtual_origin(session, desktop_environment).unwrap_or((0, 0));
    let Some((crop_x, crop_y, crop_width, crop_height)) =
        normalize_linux_crop_rect(virtual_origin, target_rect, image.width(), image.height())
    else {
        return Ok(());
    };

    let cropped = image.crop_imm(crop_x, crop_y, crop_width, crop_height);
    cropped
        .save_with_format(temp_png, image::ImageFormat::Png)
        .map_err(|e| AppError::Screenshot(format!("保存 Linux 裁剪截图失败: {e}")))?;
    Ok(())
}

fn should_capture_all_displays(config: &ScreenshotConfig) -> bool {
    config.display_mode == ScreenshotDisplayMode::All
}

/// 检测主屏幕宽度（像素）
/// 返回主屏幕的逻辑宽度，用于自适应截图分辨率
#[cfg(target_os = "macos")]
fn detect_primary_screen_width() -> Option<u32> {
    use screenshots::Screen;
    Screen::all().ok()?.first().map(|s| s.display_info.width)
}

#[cfg(target_os = "linux")]
fn detect_primary_screen_width() -> Option<u32> {
    use x11::xlib;
    unsafe {
        let display = xlib::XOpenDisplay(std::ptr::null());
        if display.is_null() {
            return None;
        }
        let screen = xlib::XDefaultScreen(display);
        let width = xlib::XDisplayWidth(display, screen).max(1) as u32;
        xlib::XCloseDisplay(display);
        Some(width)
    }
}

#[cfg(target_os = "windows")]
fn detect_primary_screen_width() -> Option<u32> {
    // Windows 上回退到固定值，后续可集成 EnumDisplayMonitors
    None
}

impl ScreenshotConfig {
    /// 根据模式计算实际使用的最大宽度
    /// auto 模式：取屏幕宽度的 70%，保证 OCR 清晰度同时控制存储
    /// fixed 模式：使用配置的 max_width
    fn compute_max_width(&self) -> u32 {
        match self.width_mode {
            ScreenshotWidthMode::Auto => {
                if let Some(screen_width) = detect_primary_screen_width() {
                    let adaptive = (screen_width as f32 * 0.7) as u32;
                    // 至少 1280，不超过原始 max_width 的 2 倍
                    adaptive.clamp(1280, self.max_width.max(1280) * 2)
                } else {
                    self.max_width
                }
            }
            ScreenshotWidthMode::Fixed => self.max_width,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn prepare_archive_image_with_config(
    dynamic_image: DynamicImage,
    config: &ScreenshotConfig,
) -> DynamicImage {
    let width = dynamic_image.width();
    let height = dynamic_image.height();
    let max_width = config.compute_max_width();

    if width > max_width {
        let scale = max_width as f32 / width as f32;
        let new_height = (height as f32 * scale).max(1.0) as u32;
        dynamic_image.resize(max_width, new_height, FilterType::Triangle)
    } else {
        dynamic_image
    }
}

/// 归档截图落盘时同步写出 400px `.thumb.jpg` 磁盘缓存，供时间线首屏直接读取免解码。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn save_thumbnail_cache_from_image(archive_image: &DynamicImage, archive_path: &Path) {
    let thumb_path = ScreenshotService::thumbnail_cache_path(archive_path);
    if thumb_path == archive_path {
        return;
    }
    let thumbnail = if archive_image.width() > 400 || archive_image.height() > 400 {
        archive_image.resize(400, 400, FilterType::Triangle)
    } else {
        archive_image.clone()
    };
    let rgb = thumbnail.to_rgb8();
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 60);
    if encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ColorType::Rgb8.into(),
        )
        .is_ok()
    {
        let _ = std::fs::write(&thumb_path, &buffer);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn save_archive_jpeg_with_quality(
    dynamic_image: &DynamicImage,
    archive_path: &Path,
    jpeg_quality: u8,
) -> Result<()> {
    let rgb_image = dynamic_image.to_rgb8();
    let mut output_file = std::fs::File::create(archive_path)?;
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output_file, jpeg_quality);
    encoder.encode(
        rgb_image.as_raw(),
        rgb_image.width(),
        rgb_image.height(),
        ColorType::Rgb8.into(),
    )?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn capture_output_paths(
    data_dir: &Path,
    screenshots_dir: &Path,
    time_str: &str,
) -> (PathBuf, PathBuf) {
    let date_str = screenshots_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-date");
    (
        screenshots_dir.join(format!("{time_str}.jpg")),
        ocr_temp_root(data_dir)
            .join(date_str)
            .join(format!("{time_str}_ocr.png")),
    )
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
struct TemporaryCaptureFile {
    path: PathBuf,
    armed: bool,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
impl TemporaryCaptureFile {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            armed: true,
        }
    }

    #[cfg(any(target_os = "windows", test))]
    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
impl Drop for TemporaryCaptureFile {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "清理系统截图临时文件 {} 时出错: {error}",
                    self.path.display()
                );
            }
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn move_or_copy_file(from: &Path, to: &Path) -> Result<()> {
    if from == to {
        return Ok(());
    }

    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            let _ = std::fs::remove_file(from);
            Ok(())
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn ocr_temp_root(data_dir: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    data_dir.to_string_lossy().hash(&mut hasher);
    std::env::temp_dir()
        .join("work-review")
        .join(format!("ocr-{:016x}", hasher.finish()))
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn cleanup_failed_capture_outputs(archive_path: &Path, ocr_source_path: &Path) {
    for path in [archive_path, ocr_source_path] {
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!("清理失败截图文件 {} 时出错: {error}", path.display());
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn cleanup_stale_ocr_temp_dir(data_dir: &Path) -> Result<()> {
    let temp_root = ocr_temp_root(data_dir);
    if temp_root.exists() {
        std::fs::remove_dir_all(&temp_root)?;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn macos_capture_rect(x: i32, y: i32, width: u32, height: u32) -> String {
    format!("{x},{y},{width},{height}")
}

#[cfg(target_os = "macos")]
fn display_pixel_offset(value: i32, scale_factor: f32) -> i32 {
    ((value as f32) * scale_factor.max(1.0)).round() as i32
}

#[cfg(target_os = "windows")]
fn capture_target_monitor(
    active_window: Option<&crate::monitor::ActiveWindow>,
) -> Option<windows_capture::monitor::Monitor> {
    let monitor = capture_target_hmonitor(active_window)?;
    Some(windows_capture::monitor::Monitor::from_raw_hmonitor(
        monitor as *mut std::ffi::c_void,
    ))
}

/// 窗口数量超过枚举上限、无法保证遮罩完整时的错误（放弃本次截图）。
#[cfg(target_os = "windows")]
fn window_enumeration_truncated_error() -> AppError {
    AppError::Screenshot("可见窗口数量过多，无法完成截图隐私遮罩，已跳过本次截图".to_string())
}

#[cfg(target_os = "windows")]
fn capture_target_monitor_rect(
    active_window: Option<&crate::monitor::ActiveWindow>,
) -> Option<(i32, i32, u32, u32)> {
    use winapi::um::winuser::{GetMonitorInfoW, MONITORINFO};

    let monitor = capture_target_hmonitor(active_window)?;
    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: unsafe { std::mem::zeroed() },
        rcWork: unsafe { std::mem::zeroed() },
        dwFlags: 0,
    };

    let ok = unsafe { GetMonitorInfoW(monitor, &mut monitor_info as *mut MONITORINFO) };
    if ok == 0 {
        return None;
    }

    let width = monitor_info
        .rcMonitor
        .right
        .checked_sub(monitor_info.rcMonitor.left)?;
    let height = monitor_info
        .rcMonitor
        .bottom
        .checked_sub(monitor_info.rcMonitor.top)?;

    if width <= 0 || height <= 0 {
        return None;
    }

    Some((
        monitor_info.rcMonitor.left,
        monitor_info.rcMonitor.top,
        width as u32,
        height as u32,
    ))
}

#[cfg(target_os = "windows")]
fn capture_target_hmonitor(
    active_window: Option<&crate::monitor::ActiveWindow>,
) -> Option<winapi::shared::windef::HMONITOR> {
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};

    let (x, y) = capture_target_point(active_window)?;
    let point = POINT { x, y };
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        None
    } else {
        Some(monitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        capture_target_point, linux_screenshot_support_for_session, normalize_linux_crop_rect,
        parse_xrandr_active_monitor_rects, should_capture_all_displays, ScreenshotService,
    };
    use crate::linux_session::{LinuxDesktopEnvironment, LinuxDesktopSession};
    use crate::monitor::{ActiveWindow, WindowBounds};
    use base64::Engine as _;
    use image::{DynamicImage, Rgba, RgbaImage};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};
    use work_review_core::config::{ScreenshotDisplayMode, ScreenshotWidthMode, StorageConfig};

    #[test]
    fn 应按窗口中心点选择目标屏幕() {
        let active_window = ActiveWindow {
            app_name: "Work Review".to_string(),
            window_title: "测试窗口".to_string(),
            browser_url: None,
            executable_path: None,
            window_bounds: Some(WindowBounds {
                x: 1440,
                y: 120,
                width: 1280,
                height: 800,
            }),
            is_minimized: false,
            is_private: false,
        };

        assert_eq!(
            capture_target_point(Some(&active_window)),
            Some((2080, 520))
        );
    }

    #[test]
    fn 缺少窗口边界时不应生成选屏坐标() {
        let active_window = ActiveWindow {
            app_name: "Work Review".to_string(),
            window_title: "测试窗口".to_string(),
            browser_url: None,
            executable_path: None,
            window_bounds: None,
            is_minimized: false,
            is_private: false,
        };

        assert_eq!(capture_target_point(Some(&active_window)), None);
        assert_eq!(capture_target_point(None), None);
    }

    #[test]
    fn 截图服务应继承存储配置中的显示模式与压缩参数() {
        let storage = StorageConfig {
            jpeg_quality: 92,
            max_image_width: 2048,
            screenshot_display_mode: ScreenshotDisplayMode::All,
            ..StorageConfig::default()
        };

        let service = ScreenshotService::new(Path::new("."), &storage);

        assert_eq!(service.config.jpeg_quality, 92);
        assert_eq!(service.config.max_width, 2048);
        assert!(should_capture_all_displays(&service.config));
    }

    #[test]
    fn 更新截图配置后应切换显示模式() {
        let mut service = ScreenshotService::new(Path::new("."), &StorageConfig::default());
        let updated_storage = StorageConfig {
            screenshot_display_mode: ScreenshotDisplayMode::All,
            ..StorageConfig::default()
        };

        service.update_config(&updated_storage);

        assert_eq!(service.config.display_mode, ScreenshotDisplayMode::All);
    }

    #[test]
    fn macos截图矩形应保留多屏负坐标() {
        assert_eq!(
            super::macos_capture_rect(-1512, -982, 1512, 982),
            "-1512,-982,1512,982"
        );
    }

    #[test]
    fn xrandr活动显示器输出应解析为矩形列表() {
        let output = r#"
Monitors: 2
 0: +*eDP-1 1920/340x1080/190+0+0  eDP-1
 1: +HDMI-1 2560/600x1440/340+1920+0  HDMI-1
"#;

        assert_eq!(
            parse_xrandr_active_monitor_rects(output),
            vec![(0, 0, 1920, 1080), (1920, 0, 2560, 1440)]
        );
    }

    #[test]
    fn linux裁剪矩形应按虚拟桌面原点归一化() {
        assert_eq!(
            normalize_linux_crop_rect((-1920, 0), (-1600, 120, 1280, 800), 4480, 1440),
            Some((320, 120, 1280, 800))
        );
    }

    #[test]
    fn linux截图provider应按会话与桌面环境选择() {
        assert_eq!(
            linux_screenshot_support_for_session(
                LinuxDesktopSession::Wayland,
                LinuxDesktopEnvironment::Gnome,
                false,
                false,
                false,
                true,
                false,
                true
            ),
            ("grim", true)
        );
        assert_eq!(
            linux_screenshot_support_for_session(
                LinuxDesktopSession::Wayland,
                LinuxDesktopEnvironment::Kde,
                false,
                false,
                false,
                true,
                true,
                true
            ),
            ("spectacle", true)
        );
        assert_eq!(
            linux_screenshot_support_for_session(
                LinuxDesktopSession::X11,
                LinuxDesktopEnvironment::Unknown,
                false,
                true,
                true,
                false,
                false,
                false
            ),
            ("import", true)
        );
        assert_eq!(
            linux_screenshot_support_for_session(
                LinuxDesktopSession::Wayland,
                LinuxDesktopEnvironment::Unknown,
                false,
                false,
                false,
                false,
                false,
                false
            ),
            ("none", false)
        );
    }

    #[test]
    fn 截图归档应缩放为jpg并将ocr临时图移出截图目录() {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!("work-review-shot-test-{unique_suffix}"));
        let screenshots_dir = data_dir.join("screenshots").join("2026-04-02");
        fs::create_dir_all(&screenshots_dir).unwrap();

        let storage = StorageConfig {
            jpeg_quality: 85,
            max_image_width: 1440,
            screenshot_width_mode: ScreenshotWidthMode::Fixed,
            ..StorageConfig::default()
        };
        let service = ScreenshotService::new(&data_dir, &storage);
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(3024, 1964, Rgba([24, 48, 96, 255])));

        let result = service
            .persist_dynamic_image_capture(image, &screenshots_dir, "101530_123", 0, true)
            .unwrap();
        let archive = image::open(&result.path).unwrap();

        assert_eq!(result.path, screenshots_dir.join("101530_123.jpg"));
        assert_eq!(archive.width(), 1440);
        assert_eq!(archive.height(), 935);
        assert!(result.image_hash.is_some(), "应随采集计算内存 dHash");
        assert!(
            ScreenshotService::thumbnail_cache_path(&result.path).exists(),
            "归档落盘应同步生成 400px .thumb.jpg 缓存"
        );

        let ocr_temp_path = result.ocr_source_path.unwrap();
        assert!(ocr_temp_path.ends_with("101530_123_ocr.png"));
        assert!(!ocr_temp_path.starts_with(&screenshots_dir));

        let no_ocr_image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(640, 480, Rgba([10, 20, 30, 255])));
        let no_ocr = service
            .persist_dynamic_image_capture(no_ocr_image, &screenshots_dir, "101535_000", 0, false)
            .unwrap();
        assert!(
            no_ocr.ocr_source_path.is_none(),
            "不需要 OCR 时不应编码写出 PNG 临时图"
        );

        let dup_img = DynamicImage::ImageRgba8(RgbaImage::from_fn(320, 200, |x, _| {
            if x < 160 {
                Rgba([240, 240, 240, 255])
            } else {
                Rgba([20, 20, 20, 255])
            }
        }));
        let h1 = ScreenshotService::compute_dhash_from_dynamic_image(&dup_img);
        let merge_dup = service
            .finalize_merge_dynamic_image(dup_img, &screenshots_dir, "101540_000", 0, h1, true)
            .unwrap();
        assert_eq!(merge_dup.image_hash, h1);
        assert!(
            merge_dup.ocr_source_path.is_none(),
            "合并路径高相似帧应跳过 PNG 编码与 OCR"
        );

        let _ = fs::remove_dir_all(&data_dir);
        let _ = fs::remove_dir_all(super::ocr_temp_root(&data_dir));
    }

    #[test]
    fn 截图归档失败时应清理ocr临时图和不完整归档() {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("work-review-shot-cleanup-test-{unique_suffix}"));
        fs::create_dir_all(&data_dir).unwrap();

        let invalid_screenshots_dir = data_dir.join("screenshots-file");
        fs::write(&invalid_screenshots_dir, b"not-a-directory").unwrap();
        let (archive_path, ocr_temp_path) =
            super::capture_output_paths(&data_dir, &invalid_screenshots_dir, "101530_456");
        let service = ScreenshotService::new(&data_dir, &StorageConfig::default());
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, Rgba([24, 48, 96, 255])));

        assert!(service
            .persist_dynamic_image_capture(image, &invalid_screenshots_dir, "101530_456", 0, true)
            .is_err());
        assert!(!ocr_temp_path.exists(), "失败后不得遗留 OCR 临时图");
        assert!(!archive_path.exists(), "失败后不得遗留不完整归档");

        let _ = fs::remove_dir_all(&data_dir);
        let _ = fs::remove_dir_all(super::ocr_temp_root(&data_dir));
    }

    #[test]
    fn 截图服务克隆应共享同一个采集锁() {
        let service = ScreenshotService::new(Path::new("."), &StorageConfig::default());
        let cloned = service.clone();

        assert!(std::sync::Arc::ptr_eq(
            &service.capture_lock,
            &cloned.capture_lock,
        ));
    }

    #[test]
    fn 临时截图守卫应在失败退出时清理文件且允许成功移交() {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir =
            std::env::temp_dir().join(format!("work-review-temp-guard-test-{unique_suffix}"));
        fs::create_dir_all(&temp_dir).unwrap();

        let failed_path = temp_dir.join("failed.png");
        fs::write(&failed_path, b"partial").unwrap();
        drop(super::TemporaryCaptureFile::new(&failed_path));
        assert!(!failed_path.exists(), "失败退出时必须清理临时截图");

        let handed_off_path = temp_dir.join("handed-off.png");
        fs::write(&handed_off_path, b"complete").unwrap();
        let mut guard = super::TemporaryCaptureFile::new(&handed_off_path);
        guard.disarm();
        drop(guard);
        assert!(handed_off_path.exists(), "成功移交后不得提前删除临时截图");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn 详情图base64应返回归档jpg原始内容() {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir =
            std::env::temp_dir().join(format!("work-review-full-shot-test-{unique_suffix}"));
        let screenshots_dir = data_dir.join("screenshots").join("2026-04-17");
        fs::create_dir_all(&screenshots_dir).unwrap();

        let storage = StorageConfig {
            jpeg_quality: 85,
            max_image_width: 1440,
            screenshot_width_mode: ScreenshotWidthMode::Fixed,
            ..StorageConfig::default()
        };
        let service = ScreenshotService::new(&data_dir, &storage);
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(1920, 1080, Rgba([12, 34, 56, 255])));

        let result = service
            .persist_dynamic_image_capture(image, &screenshots_dir, "153859_896", 0, true)
            .unwrap();

        let expected = fs::read(&result.path).unwrap();
        let encoded = service.generate_full_image_base64(&result.path).unwrap();
        let actual = super::BASE64_STANDARD.decode(encoded).unwrap();

        assert_eq!(actual, expected);

        let _ = fs::remove_dir_all(&data_dir);
        let _ = fs::remove_dir_all(super::ocr_temp_root(&data_dir));
    }
}
