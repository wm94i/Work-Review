//! 采样连续性独立于截图周期和当前输入空闲，避免唤醒时把整个采样空档当作工作时间。
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_SAMPLE_GAP: Duration = Duration::from_secs(60);
const MAX_CLOCK_DRIFT: Duration = Duration::from_secs(5);
const SUSPENDED: u64 = 1;
const DISPLAY_OFF: u64 = 2;
const POWER_FLAGS: u64 = SUSPENDED | DISPLAY_OFF;

#[derive(Clone, Copy)]
pub(crate) struct Sample {
    pub instant: Instant,
    wall: SystemTime,
    power: u64,
}

impl Sample {
    pub fn now() -> Self {
        Self {
            instant: Instant::now(),
            wall: SystemTime::now(),
            power: power_snapshot(),
        }
    }

    pub fn timestamp(self) -> i64 {
        self.wall
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    pub fn follows(self, previous: Self) -> bool {
        if self.power != previous.power || self.power & POWER_FLAGS != 0 {
            return false;
        }
        match (
            self.instant.checked_duration_since(previous.instant),
            self.wall.duration_since(previous.wall),
        ) {
            (Some(monotonic), Ok(wall)) => {
                monotonic <= MAX_SAMPLE_GAP
                    && wall <= MAX_SAMPLE_GAP
                    && monotonic.abs_diff(wall) <= MAX_CLOCK_DRIFT
            }
            _ => false,
        }
    }
}

pub(crate) struct ActivityClock {
    previous: Sample,
}

impl ActivityClock {
    pub fn new() -> Self {
        Self {
            previous: Sample::now(),
        }
    }

    pub fn sample(&mut self) -> Option<Sample> {
        self.observe(Sample::now())
    }

    pub fn previous_sample(&self) -> Sample {
        self.previous
    }

    fn observe(&mut self, now: Sample) -> Option<Sample> {
        let previous = std::mem::replace(&mut self.previous, now);
        now.follows(previous).then_some(now)
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy)]
enum PowerEvent {
    Suspend,
    Resume,
    DisplayOff(bool),
}

#[cfg(any(target_os = "windows", test))]
fn next_power_state(previous: u64, event: PowerEvent) -> u64 {
    let flags = previous & POWER_FLAGS;
    let next_flags = match event {
        PowerEvent::Suspend => flags | SUSPENDED,
        PowerEvent::Resume => flags & !SUSPENDED,
        PowerEvent::DisplayOff(true) => flags | DISPLAY_OFF,
        PowerEvent::DisplayOff(false) => flags & !DISPLAY_OFF,
    };
    if matches!(event, PowerEvent::DisplayOff(_)) && flags == next_flags {
        return previous;
    }
    // 状态和代次放在同一原子值中；即使两次采样间已完成熄屏→亮屏，仍能识别边界。
    (previous.wrapping_add(POWER_FLAGS + 1) & !POWER_FLAGS) | next_flags
}

#[cfg(target_os = "windows")]
fn power_snapshot() -> u64 {
    windows::POWER_STATE.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(not(target_os = "windows"))]
fn power_snapshot() -> u64 {
    0
}

#[cfg(not(target_os = "windows"))]
pub(crate) struct PowerMonitor;

#[cfg(not(target_os = "windows"))]
impl PowerMonitor {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "windows")]
pub(crate) use windows::PowerMonitor;

#[cfg(target_os = "windows")]
mod windows {
    use super::{next_power_state, PowerEvent};
    use std::sync::atomic::{AtomicU64, Ordering};
    use winapi::ctypes::c_void;
    use winapi::shared::guiddef::IsEqualGUID;
    use winapi::um::powerbase::{
        PowerRegisterSuspendResumeNotification, PowerUnregisterSuspendResumeNotification,
    };
    use winapi::um::powersetting::{
        PowerSettingRegisterNotification, PowerSettingUnregisterNotification,
    };
    use winapi::um::winnt::GUID_SESSION_DISPLAY_STATUS;
    use winapi::um::winuser::{
        PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND, PBT_POWERSETTINGCHANGE,
        POWERBROADCAST_SETTING,
    };

    pub(super) static POWER_STATE: AtomicU64 = AtomicU64::new(0);
    const DEVICE_NOTIFY_CALLBACK: u32 = 2;

    // winapi 0.3 的 Callback 字段多了一层指针；这里按 SDK 的函数指针 + Context 布局声明。
    #[repr(C)]
    struct NotificationRecipient {
        callback: unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> u32,
        context: usize,
    }

    // 通知订阅描述及回调状态都为静态生命周期，不依赖窗口句柄或临时栈地址。
    static RECIPIENT: NotificationRecipient = NotificationRecipient {
        callback: notification,
        context: 0,
    };

    unsafe extern "system" fn notification(
        _context: *mut c_void,
        event_type: u32,
        setting: *mut c_void,
    ) -> u32 {
        let event = match event_type as usize {
            PBT_APMSUSPEND => Some(PowerEvent::Suspend),
            PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => Some(PowerEvent::Resume),
            PBT_POWERSETTINGCHANGE if !setting.is_null() => {
                let setting = &*(setting as *const POWERBROADCAST_SETTING);
                if IsEqualGUID(&setting.PowerSetting, &GUID_SESSION_DISPLAY_STATUS)
                    && setting.DataLength == std::mem::size_of::<u32>() as u32
                {
                    match std::ptr::read_unaligned(setting.Data.as_ptr().cast::<u32>()) {
                        0 => Some(PowerEvent::DisplayOff(true)),
                        1 | 2 => Some(PowerEvent::DisplayOff(false)),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(event) = event {
            let _ = POWER_STATE.fetch_update(Ordering::AcqRel, Ordering::Acquire, |previous| {
                Some(next_power_state(previous, event))
            });
        }
        0
    }

    pub(crate) struct PowerMonitor {
        suspend_registration: usize,
        display_registration: usize,
    }

    impl PowerMonitor {
        pub fn new() -> Self {
            let mut suspend_registration = std::ptr::null_mut();
            let mut display_registration = std::ptr::null_mut();
            unsafe {
                let recipient = &RECIPIENT as *const NotificationRecipient as *mut c_void;
                let error = PowerRegisterSuspendResumeNotification(
                    DEVICE_NOTIFY_CALLBACK,
                    recipient,
                    &mut suspend_registration,
                );
                if error != 0 {
                    log::warn!("注册 Windows 睡眠/恢复通知失败: {error}，保留采样中断保护");
                }
                let error = PowerSettingRegisterNotification(
                    &GUID_SESSION_DISPLAY_STATUS,
                    DEVICE_NOTIFY_CALLBACK,
                    recipient,
                    &mut display_registration,
                );
                if error != 0 {
                    log::warn!("注册 Windows 会话显示状态通知失败: {error}，保留采样中断保护");
                }
            }
            Self {
                suspend_registration: suspend_registration as usize,
                display_registration: display_registration as usize,
            }
        }
    }

    impl Drop for PowerMonitor {
        fn drop(&mut self) {
            unsafe {
                if self.suspend_registration != 0 {
                    PowerUnregisterSuspendResumeNotification(self.suspend_registration as _);
                }
                if self.display_registration != 0 {
                    PowerSettingUnregisterNotification(self.display_registration as _);
                }
            }
        }
    }

    #[cfg(test)]
    #[test]
    #[ignore = "需要交互式 Windows 会话，不能要求无桌面的 CI 注册会话显示通知"]
    fn windows电源通知应能无窗口注册并注销() {
        let mut monitor = PowerMonitor::new();
        assert_ne!(monitor.suspend_registration, 0);
        assert_ne!(monitor.display_registration, 0);
        unsafe {
            assert_eq!(
                PowerUnregisterSuspendResumeNotification(monitor.suspend_registration as _),
                0
            );
            monitor.suspend_registration = 0;
            assert_eq!(
                PowerSettingUnregisterNotification(monitor.display_registration as _),
                0
            );
            monitor.display_registration = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(base: Sample, seconds: u64, power: u64) -> Sample {
        Sample {
            instant: base.instant + Duration::from_secs(seconds),
            wall: base.wall + Duration::from_secs(seconds),
            power,
        }
    }

    fn base() -> Sample {
        Sample {
            instant: Instant::now(),
            wall: UNIX_EPOCH + Duration::from_secs(1_000_000),
            power: 0,
        }
    }

    #[test]
    fn 长采样中断必须切段但下一次正常采样可恢复() {
        let base = base();
        let mut clock = ActivityClock { previous: base };
        let resumed = sample(base, 29_705, 0);
        assert!(clock.observe(resumed).is_none());
        assert!(clock.observe(sample(resumed, 1, 0)).is_some());
    }

    #[test]
    fn 单调时钟不计睡眠的平台也必须识别墙钟长空档() {
        let base = base();
        let mut resumed = sample(base, 1, 0);
        resumed.wall += Duration::from_secs(8 * 3600);
        assert!(!resumed.follows(base));
    }

    #[test]
    fn 时钟倒退应切段而非继续移动原活动区间() {
        let base = base();
        let mut now = sample(base, 1, 0);
        now.wall = base.wall - Duration::from_secs(1);
        assert!(!now.follows(base));
        now = sample(base, 1, 0);
        now.instant = base.instant - Duration::from_secs(1);
        assert!(!now.follows(base));
    }

    #[test]
    fn 墙钟前跳或短睡眠造成时钟偏差时应切段() {
        let base = base();
        let mut now = sample(base, 1, 0);
        now.wall += Duration::from_secs(30);
        assert!(!now.follows(base));
        assert!(sample(base, 60, 0).follows(base));
        assert!(!sample(base, 61, 0).follows(base));
    }

    #[test]
    fn 十分钟截图周期不影响持续采样() {
        let base = base();
        let mut clock = ActivityClock { previous: base };
        for second in 1..=600 {
            assert!(clock.observe(sample(base, second, 0)).is_some());
        }
    }

    #[test]
    fn 短暂熄屏亮屏即使未采到熄屏也必须切段() {
        let base = base();
        let off = next_power_state(0, PowerEvent::DisplayOff(true));
        let on = next_power_state(off, PowerEvent::DisplayOff(false));
        assert_eq!(on & POWER_FLAGS, 0);
        assert!(!sample(base, 2, on).follows(base));
        assert!(!sample(base, 1, off).follows(sample(base, 0, off)));
    }

    #[test]
    fn 后台唤醒但会话屏幕仍关闭时不可记录() {
        let off = next_power_state(0, PowerEvent::DisplayOff(true));
        let suspended = next_power_state(off, PowerEvent::Suspend);
        let resumed = next_power_state(suspended, PowerEvent::Resume);
        assert_eq!(resumed & POWER_FLAGS, DISPLAY_OFF);
        let base = base();
        assert!(!sample(base, 1, resumed).follows(sample(base, 0, resumed)));
    }

    #[test]
    fn 屏幕变暗或重复亮屏通知不应打断正常活动() {
        assert_eq!(next_power_state(0, PowerEvent::DisplayOff(false)), 0);
        let base = base();
        assert!(sample(base, 1, 0).follows(base));
    }

    #[test]
    fn 仅收到恢复通知也应切开原活动() {
        let resumed = next_power_state(0, PowerEvent::Resume);
        assert_eq!(resumed & POWER_FLAGS, 0);
        let base = base();
        assert!(!sample(base, 1, resumed).follows(base));
    }

    #[test]
    fn 跨午夜中断必须切段() {
        let mut base = base();
        base.wall = UNIX_EPOCH + Duration::from_secs(86_390);
        assert!(!sample(base, 120, 0).follows(base));
        assert!(sample(base, 20, 0).follows(base));
    }

    #[test]
    fn 待机尾段不得推进此前空闲前的旧记录() {
        assert!(crate::should_merge_observed_tail(980, 1_000, 20));
        assert!(crate::should_merge_observed_tail(981, 1_000, 20));
        assert!(!crate::should_merge_observed_tail(900, 1_000, 20));
        assert!(!crate::should_merge_observed_tail(1_001, 1_000, 20));
        assert!(!crate::should_merge_observed_tail(1_000, 1_000, 0));
    }

    #[test]
    fn 唤醒后同应用或换应用且截图开关均不得填满睡眠小时() {
        use chrono::{Local, TimeZone};
        use work_review_core::config::AppConfig;
        use work_review_core::database::{Activity, Database};
        use work_review_core::privacy::PrivacyFilter;

        let date = "2026-11-02";
        let before_sleep = Local
            .with_ymd_and_hms(2026, 11, 2, 1, 14, 45)
            .single()
            .expect("本地时间应唯一")
            .timestamp();
        let mut base = base();
        base.wall = UNIX_EPOCH + Duration::from_secs((before_sleep - 20) as u64);
        for next_app in ["WorkBuddy", "Code"] {
            for screenshots_enabled in [false, true] {
                let path = std::env::temp_dir()
                    .join(format!("work-review-suspend-{}.db", uuid::Uuid::new_v4()));
                let db = Database::new(&path).unwrap();
                let config = AppConfig::default();
                let privacy = PrivacyFilter::from_config(&config.privacy);
                let old = Activity {
                    id: None,
                    timestamp: base.timestamp(),
                    app_name: "WorkBuddy".to_string(),
                    window_title: "project".to_string(),
                    screenshot_path: if screenshots_enabled {
                        "old.png".to_string()
                    } else {
                        String::new()
                    },
                    ocr_text: None,
                    category: "development".to_string(),
                    duration: 60,
                    browser_url: None,
                    executable_path: None,
                    semantic_category: None,
                    semantic_confidence: None,
                    screenshot_url: None,
                };
                let old_id = db.insert_activity(&old).unwrap();
                let mut last_app = Some(old.app_name.clone());
                let mut last_title = Some(old.window_title.clone());
                let mut last_url = Some("https://example.com".to_string());
                let mut last_capture = base.instant;
                let mut overlays = std::collections::HashMap::from([(
                    "Player".to_string(),
                    (old_id, base.instant),
                )]);
                let mut clock = ActivityClock {
                    previous: sample(base, 20, 0),
                };
                let observed = clock.previous_sample();
                let resumed = sample(base, 29_725, 0);
                assert!(clock.observe(resumed).is_none());
                // 保留最后一次正常采样前的 20 秒，但不是整个睡眠空档。
                let tail_duration = observed.instant.duration_since(last_capture).as_secs() as i64;
                assert_eq!(tail_duration, 20);
                let previous = db
                    .get_activity_by_id(old_id)
                    .unwrap()
                    .filter(|activity| {
                        crate::should_merge_observed_tail(
                            activity.timestamp,
                            observed.timestamp(),
                            tail_duration,
                        )
                    })
                    .unwrap();
                assert_eq!(
                    crate::persist_previous_activity_backfill(
                        &db,
                        &config,
                        &privacy,
                        Some(&previous),
                        last_app.as_deref(),
                        last_title.as_deref(),
                        last_url.as_deref(),
                        tail_duration,
                        observed.timestamp(),
                        "采样中断",
                    ),
                    Some(old_id)
                );
                crate::reset_recording_session(
                    &mut last_app,
                    &mut last_title,
                    &mut last_url,
                    &mut last_capture,
                    &mut overlays,
                    resumed.instant,
                );
                assert!(last_app.is_none() && last_title.is_none() && last_url.is_none());
                assert!(overlays.is_empty());

                let current = clock.observe(sample(resumed, 10, 0)).unwrap();
                let duration = current.instant.duration_since(last_capture).as_secs() as i64;
                let changed = last_app.as_deref() != Some(next_app);
                assert!(!crate::should_confirm_idle(
                    false,
                    0,
                    screenshots_enabled,
                    false
                ));
                assert!(!crate::should_merge_contiguous_activity(
                    changed,
                    next_app,
                    current.timestamp(),
                    old.timestamp,
                ));
                let delta = crate::previous_app_backfill_duration(changed, duration, false, false);
                assert!(crate::persist_previous_activity_backfill(
                    &db,
                    &config,
                    &privacy,
                    None,
                    last_app.as_deref(),
                    last_title.as_deref(),
                    last_url.as_deref(),
                    delta,
                    current.timestamp(),
                    next_app,
                )
                .is_none());
                assert_eq!(duration, 10);
                let new_id = db
                    .insert_activity(&Activity {
                        timestamp: current.timestamp(),
                        app_name: next_app.to_string(),
                        duration,
                        screenshot_path: if screenshots_enabled {
                            "resumed.png".to_string()
                        } else {
                            String::new()
                        },
                        ..old.clone()
                    })
                    .unwrap();
                assert_ne!(old_id, new_id);
                let preserved = db.get_activity_by_id(old_id).unwrap().unwrap();
                assert_eq!(preserved.timestamp, before_sleep);
                assert_eq!(preserved.duration, 80);
                let buckets = db.get_hourly_app_breakdown(date).unwrap();
                assert!(buckets[2..9]
                    .iter()
                    .all(|bucket| bucket.total_duration == 0));
                assert_eq!(
                    buckets
                        .iter()
                        .map(|bucket| bucket.total_duration)
                        .sum::<i64>(),
                    90
                );
                drop(db);
                std::fs::remove_file(&path).unwrap();
            }
        }
    }
}
