//! Windows 亮屏后恢复已有桌宠 HWND 的透明合成，不重建 WebView 或改变窗口交互。
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use winapi::shared::windef::HWND;
use winapi::um::dwmapi::{
    DwmEnableBlurBehindWindow, DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND,
};
use winapi::um::wingdi::{CreateRectRgn, DeleteObject};
use winapi::um::winuser::IsWindow;

static RECOVERY_PENDING: AtomicBool = AtomicBool::new(false);

pub(crate) fn schedule(app: &AppHandle) {
    if RECOVERY_PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // 合并相邻恢复/亮屏通知：首拍 250ms 尽快恢复，次拍 750ms 后兜底应对 DWM/GPU 慢恢复；平时不轮询。
        tokio::time::sleep(Duration::from_millis(250)).await;
        let first_handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            if let Err(error) = restore_visible_avatar(&first_handle) {
                log::warn!("恢复桌宠原生透明失败(首拍): {error}");
            }
        }) {
            log::warn!("调度桌宠首拍透明恢复失败: {error}");
        }
        tokio::time::sleep(Duration::from_millis(750)).await;
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            RECOVERY_PENDING.store(false, Ordering::Release);
            if let Err(error) = restore_visible_avatar(&handle) {
                log::warn!("恢复桌宠原生透明失败: {error}");
            }
        }) {
            RECOVERY_PENDING.store(false, Ordering::Release);
            log::warn!("调度桌宠透明恢复失败: {error}");
        }
    });
}

fn restore_visible_avatar(app: &AppHandle) -> tauri::Result<()> {
    if !crate::activity_clock::display_is_available() {
        return Ok(());
    }
    let Some(window) = app.get_webview_window(crate::avatar_engine::AVATAR_WINDOW_LABEL) else {
        return Ok(());
    };
    // 只修复现有可见窗口，不能重新显示用户已关闭的桌宠。
    if window.is_visible()? {
        restore_window(window.hwnd()?.0 as HWND)?;
        log::debug!("已恢复桌宠原生透明合成");
    }
    Ok(())
}

pub(crate) fn restore_window(hwnd: HWND) -> io::Result<()> {
    unsafe {
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "桌宠窗口句柄已失效",
            ));
        }
        // 与 Tao 创建透明窗口时一致：空模糊区域只恢复 alpha 合成，不增加背景模糊。
        let region = CreateRectRgn(0, 0, -1, -1);
        if region.is_null() {
            return Err(io::Error::last_os_error());
        }
        let settings = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
            fEnable: 1,
            hRgnBlur: region,
            fTransitionOnMaximized: 0,
        };
        let result = DwmEnableBlurBehindWindow(hwnd, &settings);
        let released = DeleteObject(region as _);
        if result < 0 {
            return Err(io::Error::other(format!(
                "DwmEnableBlurBehindWindow HRESULT=0x{:08X}",
                result as u32
            )));
        }
        if released == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 透明恢复拒绝空句柄和失效句柄() {
        for hwnd in [std::ptr::null_mut(), 1usize as HWND] {
            assert_eq!(
                restore_window(hwnd).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
