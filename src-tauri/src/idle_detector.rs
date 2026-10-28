// 空闲检测模块
// 检测用户是否处于空闲状态（无键鼠操作且屏幕内容无变化）
// 用于解决"应用挂着不用但时长继续累加"的问题

#[cfg(target_os = "linux")]
use crate::linux_session::{current_linux_desktop_session, LinuxDesktopSession};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 上一次截图的哈希值
static LAST_SCREENSHOT_HASH: AtomicU64 = AtomicU64::new(0);
/// 连续屏幕无变化的次数
static NO_CHANGE_COUNT: AtomicU64 = AtomicU64::new(0);
/// 两个后台循环共享键鼠空闲查询结果，避免重复启动平台命令。
static LAST_IDLE_SAMPLE_AT_MS: AtomicU64 = AtomicU64::new(0);
static CACHED_IDLE_SECONDS: AtomicU64 = AtomicU64::new(0);
const IDLE_SAMPLE_CACHE_TTL_MS: u64 = 500;

/// 空闲检测器
pub struct IdleDetector {
    /// 空闲超时时间（秒）
    idle_timeout_secs: u64,
}

impl IdleDetector {
    /// 创建空闲检测器
    pub fn new(idle_timeout_minutes: u64) -> Self {
        Self {
            idle_timeout_secs: idle_timeout_minutes * 60,
        }
    }

    /// 第一阶段：检查键鼠是否超时
    /// 返回 true 表示键鼠超时（可能空闲），需要进一步用截图哈希确认
    pub fn is_input_idle(&self) -> bool {
        let idle_secs = get_idle_seconds();
        idle_secs >= self.idle_timeout_secs
    }

    /// 第二阶段：用截图哈希判断屏幕是否长时间无变化。
    ///
    /// **注意**：返回值不再驱动空闲判定。早期版本曾把"画面连续无变化"当作"确认空闲"
    /// 的依据，但对创意类应用（PS/C4D/Blender 等）、AI 网页阅读、代码思考等场景，
    /// 用户确实在工作而画面几乎不变 —— 哈希相似度对这类场景是反信号。
    /// 现在空闲判定只在 `should_confirm_idle` 里做（键鼠 + 硬超时兜底）。
    ///
    /// 此函数保留用于：累计 `NO_CHANGE_COUNT`、输出 trace 日志，便于诊断与未来扩展
    /// （如区分"工作但画面静止"vs"非工作"）。
    pub fn confirm_idle_with_hash(&self, current_hash: u64) -> bool {
        let last_hash = LAST_SCREENSHOT_HASH.swap(current_hash, Ordering::Relaxed);

        // 首次无历史哈希，不判定为空闲
        if last_hash == 0 {
            NO_CHANGE_COUNT.store(0, Ordering::Relaxed);
            return false;
        }

        // 计算相似度
        let similarity = hash_similarity(last_hash, current_hash);

        if similarity < 95 {
            // 屏幕有变化，不是空闲（终端输出、视频播放等）
            NO_CHANGE_COUNT.store(0, Ordering::Relaxed);
            log::trace!("屏幕有变化 (相似度 {similarity}%)，视为活跃");
            return false;
        }

        // 屏幕无变化，累计计数
        let count = NO_CHANGE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;

        // 需要连续多次无变化才判定为空闲
        // 按 30 秒截图间隔计算，3 次约 1.5 分钟
        if count >= 3 {
            log::debug!("确认空闲: 屏幕连续 {count} 次无变化");
            return true;
        }

        log::trace!("屏幕无变化 ({count}/3)，继续观察");
        false
    }

    /// 重置空闲状态（用户有活动时调用）
    pub fn reset(&self) {
        NO_CHANGE_COUNT.store(0, Ordering::Relaxed);
        // 同步清除哈希基线：否则用户离开→空闲确认→回来活动（触发 reset）后，
        // 若屏幕恰好与离开时相似（同一文档/网页），会快速重新累计无变化计数，
        // 把刚恢复活动的用户再次误判为空闲。存入 0 会在 confirm_idle_with_hash
        // 中被当作"首次无历史"跳过，安全。
        LAST_SCREENSHOT_HASH.store(0, Ordering::Relaxed);
    }

    /// 获取当前空闲秒数（供外部使用）
    #[allow(dead_code)]
    pub fn get_idle_seconds(&self) -> u64 {
        get_idle_seconds()
    }
}

impl Default for IdleDetector {
    fn default() -> Self {
        Self::new(3)
    }
}

/// 计算两个哈希的相似度（0-100）
fn hash_similarity(hash1: u64, hash2: u64) -> u32 {
    let xor = hash1 ^ hash2;
    let diff_bits = xor.count_ones();
    100 - (diff_bits * 100 / 64)
}

// ============== Windows 实现 ==============

#[cfg(target_os = "windows")]
fn query_idle_seconds() -> u64 {
    use std::mem::size_of;
    use winapi::um::sysinfoapi::GetTickCount;
    use winapi::um::winuser::{GetLastInputInfo, LASTINPUTINFO};

    unsafe {
        let mut lii = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };

        if GetLastInputInfo(&mut lii) != 0 {
            let current_tick = GetTickCount();
            let idle_ms = if current_tick >= lii.dwTime {
                current_tick - lii.dwTime
            } else {
                (u32::MAX - lii.dwTime) + current_tick + 1
            };
            (idle_ms / 1000) as u64
        } else {
            0
        }
    }
}

// ============== macOS 实现 ==============

#[cfg(target_os = "macos")]
fn query_idle_seconds() -> u64 {
    // 使用 FFI 直接调用 CGEventSourceSecondsSinceLastEventType
    // 因为 core-graphics crate 不直接暴露这个函数
    use core_graphics::event_source::CGEventSourceStateID;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(
            state_id: CGEventSourceStateID,
            event_type: u32,
        ) -> f64;
    }

    // kCGAnyInputEventType = ~0 (所有输入事件类型)
    const K_CG_ANY_INPUT_EVENT_TYPE: u32 = !0u32;

    let idle_time = unsafe {
        CGEventSourceSecondsSinceLastEventType(
            CGEventSourceStateID::HIDSystemState,
            K_CG_ANY_INPUT_EVENT_TYPE,
        )
    };

    if idle_time >= 0.0 {
        idle_time as u64
    } else {
        0
    }
}

// ============== Linux 实现 ==============

#[cfg(target_os = "linux")]
fn query_idle_seconds() -> u64 {
    use std::process::Command;

    if matches!(
        current_linux_desktop_session(),
        LinuxDesktopSession::Wayland
    ) {
        // GNOME (Mutter) 不实现 org.freedesktop.ScreenSaver.GetSessionIdleTime
        // (返回 NotSupported),xprintidle 也只能看到 XWayland 输入,
        // 因此优先使用 Mutter 自带的 IdleMonitor,无需安装扩展。
        // GNOME (Mutter) answers GetSessionIdleTime with NotSupported and
        // xprintidle only sees XWayland input, so query Mutter's IdleMonitor first.
        let candidates: [&[&str]; 2] = [
            &[
                "--session",
                "--print-reply",
                "--dest=org.gnome.Mutter.IdleMonitor",
                "/org/gnome/Mutter/IdleMonitor/Core",
                "org.gnome.Mutter.IdleMonitor.GetIdletime",
            ],
            &[
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.ScreenSaver",
                "/org/freedesktop/ScreenSaver",
                "org.freedesktop.ScreenSaver.GetSessionIdleTime",
            ],
        ];

        for args in candidates {
            if let Some(result) = run_linux_idle_command(Command::new("dbus-send").args(args)) {
                if result.status.success() {
                    let stdout = String::from_utf8_lossy(&result.stdout);
                    if let Some(idle_ms) = parse_dbus_idle_ms(&stdout) {
                        return idle_ms / 1000;
                    }
                }
            }
        }
    }

    // X11 以及 Wayland 回退到 xprintidle
    let output = run_linux_idle_command(&mut Command::new("xprintidle"));
    match output {
        Some(result) if result.status.success() => {
            let stdout = String::from_utf8_lossy(&result.stdout);
            let idle_ms: u64 = stdout.trim().parse().unwrap_or(0);
            idle_ms / 1000
        }
        _ => 0,
    }
}

#[cfg(target_os = "linux")]
fn run_linux_idle_command(command: &mut std::process::Command) -> Option<std::process::Output> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(2);

    command.stdout(Stdio::piped()).stderr(Stdio::null());
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

/// 解析 `dbus-send --print-reply` 输出中的空闲毫秒数。
/// ScreenSaver 返回 `uint32`,Mutter IdleMonitor 返回 `uint64`。
#[cfg(any(target_os = "linux", test))]
fn parse_dbus_idle_ms(stdout: &str) -> Option<u64> {
    stdout
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|parts| match parts {
            ["uint32" | "uint64", value] => value.parse::<u64>().ok(),
            _ => None,
        })
}

// ============== 其他平台 ==============

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn query_idle_seconds() -> u64 {
    0
}

fn idle_sample_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn idle_cache_is_fresh(now_ms: u64, sampled_at_ms: u64) -> bool {
    sampled_at_ms != 0 && now_ms.saturating_sub(sampled_at_ms) < IDLE_SAMPLE_CACHE_TTL_MS
}

fn get_idle_seconds() -> u64 {
    let now_ms = idle_sample_now_ms();
    let sampled_at_ms = LAST_IDLE_SAMPLE_AT_MS.load(Ordering::Acquire);
    if idle_cache_is_fresh(now_ms, sampled_at_ms) {
        return CACHED_IDLE_SECONDS.load(Ordering::Relaxed);
    }

    // 只有一个循环负责更新；竞争者直接读取上一份结果，不再并发启动外部命令。
    if sampled_at_ms == 0
        || LAST_IDLE_SAMPLE_AT_MS
            .compare_exchange(sampled_at_ms, now_ms, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        let idle_seconds = query_idle_seconds();
        CACHED_IDLE_SECONDS.store(idle_seconds, Ordering::Release);
        return idle_seconds;
    }

    CACHED_IDLE_SECONDS.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_similarity() {
        assert_eq!(hash_similarity(0x1234567890ABCDEF, 0x1234567890ABCDEF), 100);
        assert_eq!(hash_similarity(0x0000000000000000, 0xFFFFFFFFFFFFFFFF), 0);
    }

    #[test]
    fn 空闲查询缓存只在半秒内复用() {
        assert!(idle_cache_is_fresh(1_500, 1_200));
        assert!(!idle_cache_is_fresh(1_700, 1_200));
        assert!(!idle_cache_is_fresh(1_500, 0));
    }

    #[test]
    fn test_parse_dbus_idle_ms() {
        let mutter = "method return time=1792623559.822007 sender=:1.10 -> destination=:1.5 serial=1 reply_serial=2\n   uint64 17103\n";
        assert_eq!(parse_dbus_idle_ms(mutter), Some(17103));
        let screensaver = "method return time=1 sender=:1.10 -> destination=:1.5 serial=1 reply_serial=2\n   uint32 4200\n";
        assert_eq!(parse_dbus_idle_ms(screensaver), Some(4200));
        assert_eq!(parse_dbus_idle_ms(""), None);
        assert_eq!(parse_dbus_idle_ms("   string \"oops\"\n"), None);
    }
}
