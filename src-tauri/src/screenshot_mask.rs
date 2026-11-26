//! 截图隐私遮罩：把截图区域内可见的"非完整记录"应用窗口涂黑后再落盘 / OCR。
//!
//! 背景：非"全部显示器"模式下截取的是前台窗口所在的整个显示器，而隐私判断只针对前台应用。
//! 被设为"忽略 / 脱敏"或标题命中敏感关键词的其它应用窗口只要可见，就会进入截图、OCR 与远程上传。
//!
//! 做法：按 Z 序（自上而下）枚举可见顶层窗口，计算每个窗口未被上层窗口遮挡的可见区域，
//! 对敏感窗口的可见区域填黑。半透明 / 鼠标穿透窗口不计为遮挡物（宁可多遮，不可漏遮）。
//!
//! 几何计算为纯函数，便于单测；窗口枚举目前仅实现 Windows（macOS / Linux 见模块末尾说明）。
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// 屏幕坐标矩形（左上闭、右下开），单位为物理像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaskRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl MaskRect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn intersect(&self, other: &MaskRect) -> Option<MaskRect> {
        let rect = MaskRect::new(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        );
        (!rect.is_empty()).then_some(rect)
    }

    /// `self - other`：返回最多 4 个互不重叠的矩形。
    pub fn subtract(&self, other: &MaskRect) -> Vec<MaskRect> {
        let Some(overlap) = self.intersect(other) else {
            return vec![*self];
        };
        let mut pieces = Vec::with_capacity(4);
        // 上
        if overlap.top > self.top {
            pieces.push(MaskRect::new(self.left, self.top, self.right, overlap.top));
        }
        // 下
        if overlap.bottom < self.bottom {
            pieces.push(MaskRect::new(
                self.left,
                overlap.bottom,
                self.right,
                self.bottom,
            ));
        }
        // 左
        if overlap.left > self.left {
            pieces.push(MaskRect::new(
                self.left,
                overlap.top,
                overlap.left,
                overlap.bottom,
            ));
        }
        // 右
        if overlap.right < self.right {
            pieces.push(MaskRect::new(
                overlap.right,
                overlap.top,
                self.right,
                overlap.bottom,
            ));
        }
        pieces
    }
}

/// 一个顶层窗口在遮罩计算中的属性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowLayer {
    pub rect: MaskRect,
    /// 需要遮罩（应用隐私级别不是"完整记录"，或标题命中敏感关键词）。
    pub sensitive: bool,
    /// 是否遮挡下层窗口（半透明 / 鼠标穿透窗口为 false）。
    pub occludes: bool,
}

/// 窗口数量上限：防御异常环境下的超大枚举结果。
const MAX_LAYERS: usize = 512;
/// 单个窗口可见区域碎片上限：超过时退化为直接遮罩整个窗口（宁可多遮）。
const MAX_FRAGMENTS: usize = 256;

/// 计算需要涂黑的区域。`layers` 必须按 Z 序自上而下排列。
pub fn compute_mask_regions(layers: &[WindowLayer], capture: &MaskRect) -> Vec<MaskRect> {
    let mut covered: Vec<MaskRect> = Vec::new();
    let mut regions = Vec::new();

    for layer in layers.iter().take(MAX_LAYERS) {
        let Some(clipped) = layer.rect.intersect(capture) else {
            continue;
        };

        if layer.sensitive {
            let mut visible = vec![clipped];
            for occluder in &covered {
                visible = visible
                    .iter()
                    .flat_map(|piece| piece.subtract(occluder))
                    .collect();
                if visible.is_empty() {
                    break;
                }
                if visible.len() > MAX_FRAGMENTS {
                    visible = vec![clipped];
                    break;
                }
            }
            regions.extend(visible);
        }

        if layer.occludes {
            covered.push(clipped);
        }
    }

    regions
}

/// 把屏幕坐标的遮罩区域涂黑到 RGBA 缓冲区（缓冲区左上角对应屏幕坐标 `origin`）。
pub fn apply_mask_rgba(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    regions: &[MaskRect],
) -> usize {
    let width_i = width as i64;
    let height_i = height as i64;
    let Some(required_bytes) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
    else {
        return 0;
    };
    if pixels.len() < required_bytes {
        return 0;
    }
    let mut painted = 0usize;
    for region in regions {
        let left = (region.left as i64 - origin_x as i64).clamp(0, width_i) as usize;
        let right = (region.right as i64 - origin_x as i64).clamp(0, width_i) as usize;
        let top = (region.top as i64 - origin_y as i64).clamp(0, height_i) as usize;
        let bottom = (region.bottom as i64 - origin_y as i64).clamp(0, height_i) as usize;
        if right <= left || bottom <= top {
            continue;
        }
        for row in top..bottom {
            let start = (row * width as usize + left) * 4;
            let end = (row * width as usize + right) * 4;
            for pixel in pixels[start..end].as_chunks_mut::<4>().0 {
                *pixel = [0, 0, 0, 255];
            }
        }
        painted += (right - left) * (bottom - top);
    }
    painted
}

/// 判断窗口是否需要遮罩：标题命中敏感关键词，或应用隐私级别不是"完整记录"。
/// 读不到进程名（空字符串，如受保护/提权进程）时，除检查标题关键词外，还按标题分段匹配应用隐私规则，
/// 避免仅因无法打开进程句柄而漏遮。
pub fn is_sensitive_window(
    privacy: &work_review_core::config::PrivacyConfig,
    app_name: &str,
    window_title: &str,
) -> bool {
    use work_review_core::config::PrivacyLevel;
    let trimmed_title = window_title.trim();
    let keyword_hit =
        !trimmed_title.is_empty() && privacy.should_anonymize_by_keyword(trimmed_title);
    if keyword_hit {
        return true;
    }
    let trimmed_app = app_name.trim();
    if privacy.skip_private_windows
        && !trimmed_app.is_empty()
        && work_review_core::privacy::is_private_browser_window_title(trimmed_app, trimmed_title)
    {
        return true;
    }
    if !trimmed_app.is_empty() {
        return privacy.get_app_privacy_level(trimmed_app) != PrivacyLevel::Full;
    }
    if trimmed_title.is_empty() {
        return false;
    }
    // 进程名不可读时，尝试把标题整串或常见分隔段（" - " / " — " / " | "）当作应用名匹配规则
    if privacy.get_app_privacy_level(trimmed_title) != PrivacyLevel::Full {
        return true;
    }
    for separator in [" - ", " — ", " – ", " | "] {
        if trimmed_title.contains(separator)
            && trimmed_title
                .split(separator)
                .map(str::trim)
                .filter(|seg| !seg.is_empty())
                .any(|seg| privacy.get_app_privacy_level(seg) != PrivacyLevel::Full)
        {
            return true;
        }
    }
    false
}

/// 枚举当前桌面上可见的顶层窗口（Z 序自上而下）。
///
/// 返回 `None` 表示窗口枚举失败或超过上限被截断：此时无法保证遮罩完整，调用方应放弃本次截图。
#[cfg(target_os = "windows")]
pub fn enumerate_window_layers(
    privacy: &work_review_core::config::PrivacyConfig,
) -> Option<Vec<WindowLayer>> {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::shared::minwindef::{BOOL, FALSE, LPARAM, TRUE};
    use winapi::shared::windef::{HWND, RECT};
    use winapi::um::dwmapi::DwmGetWindowAttribute;
    use winapi::um::processthreadsapi::GetCurrentProcessId;
    use winapi::um::winuser::{
        EnumChildWindows, EnumWindows, GetLayeredWindowAttributes, GetWindowDisplayAffinity,
        GetWindowLongW, GetWindowRect, GetWindowRgnBox, GetWindowTextW, GetWindowThreadProcessId,
        IsIconic, IsWindowVisible, GWL_EXSTYLE, LWA_ALPHA, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };

    const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
    const DWMWA_CLOAKED: u32 = 14;
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
    const NULLREGION: i32 = 1;
    const SIMPLEREGION: i32 = 2;
    const COMPLEXREGION: i32 = 3;
    // 注：WS_EX_NOREDIRECTIONBITMAP（DirectComposition）不视为透明——实测 Chrome、Windows Terminal、
    // 系统设置、UWP 宿主等常规不透明窗口都带此样式，若按透明处理会把前台正常内容大面积涂黑。
    // 残余风险：真正透明的 DComp 浮层下方的敏感窗口不会被遮罩（本应用透明桌宠窗口已显式排除）。
    /// EnumWindows 会返回所有顶层窗口（含不可见），上限需远大于可见窗口上限
    const MAX_HANDLES: usize = MAX_LAYERS * 16;

    struct Collected {
        handles: Vec<HWND>,
        truncated: bool,
    }

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let collected = &mut *(lparam as *mut Collected);
            if collected.handles.len() < MAX_HANDLES {
                collected.handles.push(hwnd);
                TRUE
            } else {
                collected.truncated = true;
                FALSE
            }
        }))
        .unwrap_or(FALSE)
    }

    struct UwpChildLookup {
        host_pid: u32,
        child_pid: Option<u32>,
    }

    unsafe extern "system" fn find_uwp_child_pid(hwnd: HWND, lparam: LPARAM) -> BOOL {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let lookup = &mut *(lparam as *mut UwpChildLookup);
            let mut child_pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut child_pid);
            if child_pid != 0 && child_pid != lookup.host_pid {
                lookup.child_pid = Some(child_pid);
                FALSE
            } else {
                TRUE
            }
        }))
        .unwrap_or(FALSE)
    }

    let mut collected = Collected {
        handles: Vec::new(),
        truncated: false,
    };
    let enum_ok = unsafe { EnumWindows(Some(collect), &mut collected as *mut Collected as LPARAM) };
    if collected.truncated || enum_ok == 0 {
        return None;
    }

    let current_pid = unsafe { GetCurrentProcessId() };
    let avatar_hwnd = crate::avatar_engine::avatar_hwnd_raw();
    let mut app_name_by_pid: HashMap<u32, String> = HashMap::new();
    let mut layers = Vec::new();

    for hwnd in collected.handles {
        if layers.len() >= MAX_LAYERS {
            return None;
        }
        unsafe {
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                continue;
            }

            // 被 DWM 隐藏（挂起的 UWP、其它虚拟桌面）的窗口不可见
            let mut cloaked: u32 = 0;
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut u32 as *mut _,
                std::mem::size_of::<u32>() as u32,
            ) == 0
                && cloaked != 0
            {
                continue;
            }

            let mut rect: RECT = std::mem::zeroed();
            let frame_ok = DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut RECT as *mut _,
                std::mem::size_of::<RECT>() as u32,
            ) == 0;
            if !frame_ok && GetWindowRect(hwnd, &mut rect) == 0 {
                continue;
            }
            let mask_rect = MaskRect::new(rect.left, rect.top, rect.right, rect.bottom);
            if mask_rect.is_empty() {
                continue;
            }

            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            // 分层窗口只有在显式 LWA_ALPHA 且 alpha=255 且无色键（flags == LWA_ALPHA）时才算遮挡物；
            // alpha=0 的全透明分层窗口直接跳过；逐像素 alpha（UpdateLayeredWindow）按透明处理。
            let (layered_opaque, layered_invisible) = if ex_style & WS_EX_LAYERED != 0 {
                let mut color_key = 0u32;
                let mut alpha = 0u8;
                let mut flags = 0u32;
                let ok =
                    GetLayeredWindowAttributes(hwnd, &mut color_key, &mut alpha, &mut flags) != 0;
                let opaque = ok && flags == LWA_ALPHA && alpha == 255;
                let invisible = ok && (flags & LWA_ALPHA != 0) && alpha == 0;
                (opaque, invisible)
            } else {
                (false, false)
            };
            if layered_invisible {
                continue;
            }

            let is_avatar = avatar_hwnd != 0 && (hwnd as usize) == avatar_hwnd;
            // 被系统从截屏中排除的窗口在截图中不可见，不能计为上层遮挡物
            let mut affinity: u32 = 0;
            let excluded_from_capture = GetWindowDisplayAffinity(hwnd, &mut affinity) != 0
                && affinity == WDA_EXCLUDEFROMCAPTURE;
            // 带自定义非矩形裁剪区（空区域 / 复杂区域 / 缩小矩形）的窗口不计为完全遮挡物
            let mut rgn_rect: RECT = std::mem::zeroed();
            let rgn_kind = GetWindowRgnBox(hwnd, &mut rgn_rect);
            let clipped_region = matches!(rgn_kind, NULLREGION | COMPLEXREGION)
                || (rgn_kind == SIMPLEREGION
                    && ((rgn_rect.right - rgn_rect.left) < (rect.right - rect.left)
                        || (rgn_rect.bottom - rgn_rect.top) < (rect.bottom - rect.top)));

            let occludes = frame_ok
                && !is_avatar
                && !excluded_from_capture
                && !clipped_region
                && ex_style & WS_EX_TRANSPARENT == 0
                && (ex_style & WS_EX_LAYERED == 0 || layered_opaque);

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let mut effective_pid = pid;
            let mut app_name = app_name_by_pid
                .entry(effective_pid)
                .or_insert_with(|| crate::monitor::display_app_name_for_pid(effective_pid))
                .clone();

            // UWP 应用外层托管在 ApplicationFrameHost.exe，解析实际子窗口 PID 获取真实应用名
            if app_name.eq_ignore_ascii_case("ApplicationFrameHost") && pid != 0 {
                let mut lookup = UwpChildLookup {
                    host_pid: pid,
                    child_pid: None,
                };
                EnumChildWindows(
                    hwnd,
                    Some(find_uwp_child_pid),
                    &mut lookup as *mut UwpChildLookup as LPARAM,
                );
                if let Some(child_pid) = lookup.child_pid {
                    effective_pid = child_pid;
                    let child_name = app_name_by_pid
                        .entry(effective_pid)
                        .or_insert_with(|| crate::monitor::display_app_name_for_pid(effective_pid))
                        .clone();
                    if !child_name.is_empty() {
                        app_name = child_name;
                    }
                }
            }

            // 跳过本进程窗口的 GetWindowTextW，避免后台线程向本进程 UI 线程发 WM_GETTEXT 造成阻塞
            let title = if pid == current_pid {
                String::new()
            } else {
                let mut title_buf: [u16; 512] = [0; 512];
                let title_len =
                    GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
                if title_len > 0 {
                    OsString::from_wide(&title_buf[..title_len as usize])
                        .to_string_lossy()
                        .into_owned()
                } else {
                    String::new()
                }
            };

            layers.push(WindowLayer {
                rect: mask_rect,
                sensitive: !is_avatar && is_sensitive_window(privacy, &app_name, &title),
                occludes,
            });
        }
    }

    Some(layers)
}

/// 当前截图范围内需要遮罩的区域；枚举被截断时返回 `None`（调用方应放弃截图）。
#[cfg(target_os = "windows")]
pub fn sensitive_regions_in(
    privacy: &work_review_core::config::PrivacyConfig,
    capture: &MaskRect,
) -> Option<Vec<MaskRect>> {
    enumerate_window_layers(privacy).map(|layers| compute_mask_regions(&layers, capture))
}

// macOS / Linux：暂未实现窗口枚举。
// - macOS 可用 CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly) 获取 owner 与 bounds
//   （单位为 point，需按显示器缩放换算），待具备真机验证条件后补齐。
// - Linux（X11 / 各 Wayland 合成器）无统一可靠的窗口枚举手段，保持现状并在隐私设置中说明限制。

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(rect: MaskRect, sensitive: bool) -> WindowLayer {
        WindowLayer {
            rect,
            sensitive,
            occludes: true,
        }
    }

    fn area(regions: &[MaskRect]) -> i64 {
        regions
            .iter()
            .map(|r| (r.right - r.left) as i64 * (r.bottom - r.top) as i64)
            .sum()
    }

    #[test]
    fn 矩形相减应得到互不重叠的剩余部分() {
        let a = MaskRect::new(0, 0, 100, 100);
        let b = MaskRect::new(25, 25, 75, 75);
        let pieces = a.subtract(&b);
        assert_eq!(area(&pieces), 100 * 100 - 50 * 50);
        for piece in &pieces {
            assert!(piece.intersect(&b).is_none());
        }
        assert_eq!(a.subtract(&MaskRect::new(200, 200, 300, 300)), vec![a]);
        assert!(a.subtract(&MaskRect::new(-10, -10, 200, 200)).is_empty());
    }

    #[test]
    fn 被前台窗口完全遮挡的敏感窗口不需要遮罩() {
        let capture = MaskRect::new(0, 0, 1920, 1080);
        let layers = vec![
            layer(MaskRect::new(0, 0, 1920, 1080), false),
            layer(MaskRect::new(100, 100, 800, 600), true),
        ];
        assert!(compute_mask_regions(&layers, &capture).is_empty());
    }

    #[test]
    fn 部分露出的敏感窗口只遮罩露出部分() {
        let capture = MaskRect::new(0, 0, 1920, 1080);
        let layers = vec![
            // 前台窗口占左半屏
            layer(MaskRect::new(0, 0, 960, 1080), false),
            // 敏感窗口横跨中线
            layer(MaskRect::new(800, 100, 1200, 500), true),
        ];
        let regions = compute_mask_regions(&layers, &capture);
        assert_eq!(regions, vec![MaskRect::new(960, 100, 1200, 500)]);
    }

    #[test]
    fn 半透明窗口不计为遮挡物() {
        let capture = MaskRect::new(0, 0, 1920, 1080);
        let layers = vec![
            WindowLayer {
                rect: MaskRect::new(0, 0, 1920, 1080),
                sensitive: false,
                occludes: false,
            },
            layer(MaskRect::new(100, 100, 300, 300), true),
        ];
        assert_eq!(
            compute_mask_regions(&layers, &capture),
            vec![MaskRect::new(100, 100, 300, 300)]
        );
    }

    #[test]
    fn 遮罩区域应裁剪到截图范围并支持负坐标显示器() {
        // 截图为左侧副屏（虚拟坐标为负）
        let capture = MaskRect::new(-1920, 0, 0, 1080);
        let layers = vec![layer(MaskRect::new(-200, 50, 400, 300), true)];
        let regions = compute_mask_regions(&layers, &capture);
        assert_eq!(regions, vec![MaskRect::new(-200, 50, 0, 300)]);

        let mut pixels = vec![255u8; 1920 * 1080 * 4];
        let painted = apply_mask_rgba(&mut pixels, 1920, 1080, -1920, 0, &regions);
        assert_eq!(painted, 200 * 250);
        // 区域内像素被涂黑，区域外保持原样
        let inside = ((60 * 1920) + 1800) * 4;
        assert_eq!(&pixels[inside..inside + 4], &[0, 0, 0, 255]);
        let outside = ((60 * 1920) + 100) * 4;
        assert_eq!(&pixels[outside..outside + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn 上层敏感窗口不受下层窗口影响() {
        let capture = MaskRect::new(0, 0, 1000, 1000);
        let layers = vec![
            layer(MaskRect::new(0, 0, 500, 500), true),
            layer(MaskRect::new(0, 0, 1000, 1000), false),
        ];
        assert_eq!(
            compute_mask_regions(&layers, &capture),
            vec![MaskRect::new(0, 0, 500, 500)]
        );
    }

    #[test]
    fn 缓冲区尺寸不符时不写入() {
        let mut pixels = vec![255u8; 10];
        let painted = apply_mask_rgba(&mut pixels, 100, 100, 0, 0, &[MaskRect::new(0, 0, 10, 10)]);
        assert_eq!(painted, 0);
    }

    #[test]
    fn 敏感判断应同时考虑应用级别与标题关键词() {
        let privacy = work_review_core::config::PrivacyConfig::default();
        // 默认规则：1Password 忽略；关键词包含 bank
        assert!(is_sensitive_window(&privacy, "1Password", "Vault"));
        assert!(is_sensitive_window(
            &privacy,
            "Google Chrome",
            "My Bank - Statement"
        ));
        assert!(!is_sensitive_window(&privacy, "Code", "main.rs - project"));
        assert!(!is_sensitive_window(&privacy, "Code", ""));
        // 读不到进程名时仍按标题关键词与标题分段应用规则遮罩（不因信息缺失而放行）
        assert!(is_sensitive_window(&privacy, "", "Online Bank - Login"));
        assert!(is_sensitive_window(
            &privacy,
            "",
            "1Password - Personal Vault"
        ));
        assert!(!is_sensitive_window(&privacy, "", "main.rs"));
    }

    #[test]
    fn 缓冲区尺寸溢出时应安全返回0() {
        let mut pixels = vec![255u8; 16];
        let painted = apply_mask_rgba(
            &mut pixels,
            u32::MAX,
            u32::MAX,
            0,
            0,
            &[MaskRect::new(0, 0, 10, 10)],
        );
        assert_eq!(painted, 0);
    }
}

/// 真机冒烟：在当前桌面上枚举窗口，验证 Win32 / DWM 调用可用（需交互式桌面，CI 不运行）。
/// 运行：`cargo test -p work-review --bin work-review -- screenshot_mask --ignored --nocapture`
#[cfg(all(test, target_os = "windows"))]
mod desktop_smoke {
    use super::*;

    #[test]
    #[ignore]
    fn 真机枚举可见窗口() {
        let privacy = work_review_core::config::PrivacyConfig::default();
        let layers = enumerate_window_layers(&privacy).expect("正常桌面的窗口数量不应超过枚举上限");
        let sensitive = layers.iter().filter(|layer| layer.sensitive).count();
        let occluding = layers.iter().filter(|layer| layer.occludes).count();
        println!(
            "可见顶层窗口 {} 个，遮挡物 {} 个，敏感窗口 {} 个",
            layers.len(),
            occluding,
            sensitive
        );
        assert!(!layers.is_empty(), "交互式桌面上至少应有一个可见窗口");
        for layer in &layers {
            assert!(!layer.rect.is_empty());
        }
    }
}
