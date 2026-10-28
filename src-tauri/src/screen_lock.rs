// 锁屏检测模块 (Windows / macOS)
// 监听系统锁屏/解锁事件，用于控制录制状态

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 屏幕锁定状态
pub struct ScreenLockMonitor {
    /// 是否锁定
    is_locked: Arc<AtomicBool>,
}

impl ScreenLockMonitor {
    /// 创建锁屏监控器
    pub fn new() -> Self {
        Self {
            is_locked: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 检查屏幕是否锁定 (Windows)
    /// 使用 OpenInputDesktop 方式判断：锁屏时系统桌面切换到 Winlogon 桌面，
    /// 此时当前线程无法打开输入桌面，可靠性远高于 GetForegroundWindow/quser
    #[cfg(target_os = "windows")]
    pub fn is_locked(&self) -> bool {
        use winapi::um::winnt::GENERIC_ALL;
        use winapi::um::winuser::{CloseDesktop, OpenInputDesktop, SwitchDesktop};

        unsafe {
            // 尝试打开当前输入桌面
            // 锁屏时系统会切换到 Winlogon 桌面，当前进程无权限打开，返回 null
            let desktop = OpenInputDesktop(0, 0, GENERIC_ALL);
            if desktop.is_null() {
                // 无法打开输入桌面，说明已经锁屏
                log::debug!("锁屏检测: OpenInputDesktop 返回 null，判断为锁屏");
                return true;
            }

            // 尝试切换到该桌面（如果切换失败，说明是受限的 Winlogon 桌面）
            let switched = SwitchDesktop(desktop);
            CloseDesktop(desktop);

            if switched == 0 {
                // SwitchDesktop 失败，说明是锁屏桌面
                log::debug!("锁屏检测: SwitchDesktop 失败，判断为锁屏");
                return true;
            }
        }

        false
    }

    /// 检查屏幕是否锁定 (macOS)
    /// 仅使用 CGSessionCopyCurrentDictionary (FFI)，不再 spawn 外部进程
    /// CGSession 覆盖锁屏、睡眠、Power Nap 唤醒等全部场景，是最可靠的检测方式
    #[cfg(target_os = "macos")]
    pub fn is_locked(&self) -> bool {
        // CGSessionCopyCurrentDictionary: 纯 FFI 调用，无进程 spawn 开销
        // 返回当前登录会话字典，包含 CGSSessionScreenIsLocked 键
        let locked = Self::is_session_locked();
        if locked {
            log::debug!("锁屏检测: CGSession 报告屏幕已锁定");
        }
        locked
    }

    /// macOS: 通过 CGSessionCopyCurrentDictionary 检测锁屏
    /// 这是最可靠的方式，在 Power Nap 唤醒、合盖睡眠等场景均可准确检测
    #[cfg(target_os = "macos")]
    fn is_session_locked() -> bool {
        use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
        use core_foundation::boolean::CFBoolean;
        use core_foundation::dictionary::CFDictionaryRef;
        use core_foundation::string::CFString;

        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
        }

        unsafe {
            let dict = CGSessionCopyCurrentDictionary();
            if dict.is_null() {
                // 无法获取会话信息（可能在睡眠或无用户登录），视为锁定
                return true;
            }

            let key = CFString::new("CGSSessionScreenIsLocked");
            let mut value_ref: CFTypeRef = std::ptr::null();
            let found = core_foundation::dictionary::CFDictionaryGetValueIfPresent(
                dict,
                key.as_CFTypeRef() as *const _,
                &mut value_ref,
            );

            let locked = if found != 0 && !value_ref.is_null() {
                // 值是 CFBoolean，检查是否为 true
                let cf_bool = CFBoolean::wrap_under_get_rule(value_ref as _);
                cf_bool == CFBoolean::true_value()
            } else {
                false
            };

            CFRelease(dict as _);
            locked
        }
    }

    /// 检查屏幕是否锁定 (Linux)
    /// 优先使用 loginctl；只有该命令不可用时才走一次合并回退，避免每轮启动多个进程。
    #[cfg(target_os = "linux")]
    pub fn is_locked(&self) -> bool {
        use std::process::Command;

        if let Some(output) = run_linux_lock_command(Command::new("loginctl").args([
            "show-session",
            "auto",
            "--property=LockedHint",
            "--value",
            "--no-legend",
        ])) {
            if output.status.success() {
                let locked = loginctl_reports_locked(&String::from_utf8_lossy(&output.stdout));
                if locked {
                    log::debug!("锁屏检测: loginctl 报告 session 已锁定");
                }
                return locked;
            }
        }

        // 无 systemd 的桌面环境：一次 pgrep 覆盖常见锁屏进程，再尝试 Cinnamon D-Bus。
        if let Some(output) = run_linux_lock_command(Command::new("pgrep").args([
            "-f",
            "^(cinnamon-screensaver|gnome-screensaver|xscreensaver|i3lock|swaylock)( .*)?$",
        ])) {
            if output.status.success() {
                log::debug!("锁屏检测: 常见锁屏进程正在运行");
                return true;
            }
        }

        run_linux_lock_command(Command::new("dbus-send").args([
            "--session",
            "--dest=org.cinnamon.ScreenSaver",
            "--type=method_call",
            "--print-reply",
            "/org/cinnamon/ScreenSaver",
            "org.cinnamon.ScreenSaver.GetActive",
        ]))
        .is_some_and(|output| {
            let locked = output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("boolean true");
            if locked {
                log::debug!("锁屏检测: Cinnamon ScreenSaver 报告已激活");
            }
            locked
        })
    }

    /// 检查屏幕是否锁定 (其他平台)
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub fn is_locked(&self) -> bool {
        false
    }

    /// 设置锁定状态（用于手动更新）
    pub fn set_locked(&self, locked: bool) {
        self.is_locked.store(locked, Ordering::SeqCst);
    }
}

#[cfg(any(target_os = "linux", test))]
fn loginctl_reports_locked(stdout: &str) -> bool {
    matches!(stdout.trim(), "yes" | "LockedHint=yes")
}

#[cfg(target_os = "linux")]
fn run_linux_lock_command(command: &mut std::process::Command) -> Option<std::process::Output> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(2);

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if started_at.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

impl Default for ScreenLockMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::loginctl_reports_locked;

    #[test]
    fn loginctl锁屏结果只接受明确的yes() {
        assert!(loginctl_reports_locked("yes\n"));
        assert!(loginctl_reports_locked("LockedHint=yes\n"));
        assert!(!loginctl_reports_locked("no\n"));
        assert!(!loginctl_reports_locked(""));
    }
}
