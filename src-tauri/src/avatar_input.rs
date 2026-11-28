// avatar_input 中的 Linux/Wayland 函数仅在对应平台编译路径中使用
// dead_code allow 在 main.rs 的 mod 声明处设置
use crate::avatar_engine::AvatarInputPayload;
use once_cell::sync::Lazy;
use std::sync::atomic::{
    AtomicBool, AtomicI8, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering,
};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

const KEYBOARD_ACTIVE_WINDOW_MS: u64 = 180;
const MOUSE_ACTIVE_WINDOW_MS: u64 = 220;

static LAST_KEYBOARD_INPUT_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_MOUSE_INPUT_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_KEYBOARD_GROUP_CODE: AtomicU8 = AtomicU8::new(0);
static LAST_KEYBOARD_KEY_CODE: AtomicU16 = AtomicU16::new(0);
static LAST_MOUSE_GROUP_CODE: AtomicU8 = AtomicU8::new(0);
static CURSOR_RATIO_X_PERMILLE: AtomicU32 = AtomicU32::new(500);
static CURSOR_RATIO_Y_PERMILLE: AtomicU32 = AtomicU32::new(500);
static CURSOR_RATIO_INITIALIZED: AtomicBool = AtomicBool::new(false);
static KEYBOARD_HAS_PRESSED_KEYS: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static WINDOWS_KEYBOARD_INPUTS: Lazy<KeyboardInputBuffer> = Lazy::new(KeyboardInputBuffer::new);
#[cfg(target_os = "windows")]
static WINDOWS_HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
#[cfg(target_os = "windows")]
static WINDOWS_RAW_CURSOR_POS: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "windows")]
static WINDOWS_RAW_CURSOR_DIRTY: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
type VirtualScreenCache = Option<(std::time::Instant, (i32, i32, f64, f64))>;
#[cfg(target_os = "windows")]
static WINDOWS_VIRTUAL_SCREEN_CACHE: Lazy<Mutex<VirtualScreenCache>> =
    Lazy::new(|| Mutex::new(None));
#[cfg(target_os = "linux")]
static WAYLAND_CURSOR_POSITION: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "linux")]
static WAYLAND_CURSOR_UPDATED_AT_MS: AtomicU64 = AtomicU64::new(0);
static INPUT_BRIDGE_GEN: AtomicU64 = AtomicU64::new(0);
static INPUT_MONITOR_STARTED: AtomicBool = AtomicBool::new(false);
static INPUT_MONITOR_GEN: AtomicU64 = AtomicU64::new(0);
static INPUT_MONITOR_LIFECYCLE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
#[cfg(target_os = "macos")]
static INPUT_MONITOR_RETRY_GEN: AtomicU64 = AtomicU64::new(0);
// 智能穿透与隐藏本体运行时状态（无锁读，避免轮询每轮锁 Mutex）
static AVATAR_CLICK_THROUGH_ENABLED: AtomicBool = AtomicBool::new(false);
static AVATAR_ENABLED_FLAG: AtomicBool = AtomicBool::new(false);
static AVATAR_BODY_HIDDEN_FLAG: AtomicBool = AtomicBool::new(false);
// set_ignore_cursor_events 缓存：-1=未知，0=false，1=true
static AVATAR_CLICK_THROUGH_IGNORE_CACHE: AtomicI8 = AtomicI8::new(-1);

pub fn set_avatar_click_through_flag(on: bool) {
    AVATAR_CLICK_THROUGH_ENABLED.store(on, Ordering::Relaxed);
}
pub fn set_avatar_body_hidden_flag(hidden: bool) {
    AVATAR_BODY_HIDDEN_FLAG.store(hidden, Ordering::Relaxed);
    if hidden {
        #[cfg(target_os = "windows")]
        WINDOWS_KEYBOARD_INPUTS.invalidate();
        KEYBOARD_HAS_PRESSED_KEYS.store(false, Ordering::Relaxed);
        KEYBOARD_STATE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pressed_keys
            .clear();
    }
    force_resync_click_through();
}
pub fn set_avatar_enabled_flag(on: bool) {
    AVATAR_ENABLED_FLAG.store(on, Ordering::Relaxed);
    if !on {
        clear_avatar_input_state();
        force_resync_click_through();
    }
}
/// 重置 ignore 缓存，强制下一轮命中测试重新同步（穿透配置/窗口变更时调）。
pub fn force_resync_click_through() {
    AVATAR_CLICK_THROUGH_IGNORE_CACHE.store(-1, Ordering::Relaxed);
}
static KEYBOARD_STATE: Lazy<Mutex<KeyboardInputState>> =
    Lazy::new(|| Mutex::new(KeyboardInputState::default()));

fn clear_avatar_input_state() {
    #[cfg(target_os = "windows")]
    WINDOWS_KEYBOARD_INPUTS.invalidate();
    LAST_KEYBOARD_INPUT_AT_MS.store(0, Ordering::Relaxed);
    LAST_MOUSE_INPUT_AT_MS.store(0, Ordering::Relaxed);
    LAST_KEYBOARD_GROUP_CODE.store(0, Ordering::Relaxed);
    LAST_KEYBOARD_KEY_CODE.store(0, Ordering::Relaxed);
    LAST_MOUSE_GROUP_CODE.store(0, Ordering::Relaxed);
    CURSOR_RATIO_INITIALIZED.store(false, Ordering::Relaxed);
    KEYBOARD_HAS_PRESSED_KEYS.store(false, Ordering::Relaxed);
    KEYBOARD_STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .pressed_keys
        .clear();
    #[cfg(target_os = "windows")]
    WINDOWS_RAW_CURSOR_DIRTY.store(false, Ordering::Relaxed);
    #[cfg(target_os = "linux")]
    WAYLAND_CURSOR_UPDATED_AT_MS.store(0, Ordering::Relaxed);
}

const KEYBOARD_GROUP_DIGIT_1: u8 = 1;
const KEYBOARD_GROUP_DIGIT_2: u8 = 2;
const KEYBOARD_GROUP_DIGIT_3: u8 = 3;
const KEYBOARD_GROUP_DIGIT_4: u8 = 4;
const KEYBOARD_GROUP_DIGIT_5: u8 = 5;
const KEYBOARD_GROUP_DIGIT_6: u8 = 6;
const KEYBOARD_GROUP_DIGIT_7: u8 = 7;
const KEYBOARD_GROUP_KEY_Q: u8 = 8;
const KEYBOARD_GROUP_KEY_E: u8 = 9;
const KEYBOARD_GROUP_KEY_R: u8 = 10;
const KEYBOARD_GROUP_SPACE: u8 = 11;
const KEYBOARD_GROUP_KEY_A: u8 = 12;
const KEYBOARD_GROUP_KEY_D: u8 = 13;
const KEYBOARD_GROUP_KEY_S: u8 = 14;
const KEYBOARD_GROUP_KEY_W: u8 = 15;

const MOUSE_GROUP_MOVE: u8 = 1;
const MOUSE_GROUP_LEFT: u8 = 2;
const MOUSE_GROUP_RIGHT: u8 = 3;
const MOUSE_GROUP_SIDE: u8 = 4;
const INPUT_BRIDGE_POLL_INTERVAL: Duration = Duration::from_millis(16);
const INPUT_BRIDGE_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(64);
#[cfg(target_os = "macos")]
const INPUT_MONITOR_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const PRESSED_KEY_STALE_MS: u64 = KEYBOARD_ACTIVE_WINDOW_MS;
#[cfg(target_os = "linux")]
const WAYLAND_MOUSE_POLL_INTERVAL: Duration = Duration::from_millis(120);
const WAYLAND_CURSOR_STALE_MS: u64 = 500;

#[derive(Default)]
struct KeyboardInputState {
    pressed_keys: Vec<PressedKeyState>,
}

#[derive(Clone, Copy)]
struct PressedKeyState {
    key_code: u16,
    pressed_at_ms: u64,
}

#[cfg(any(target_os = "windows", test))]
const KEYBOARD_INPUT_CAPACITY: usize = 256;

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BufferedKeyboardInput {
    epoch: u64,
    pressed_at_ms: u64,
    key_code: u16,
    group_code: u8,
    pressed: bool,
}

#[cfg(any(target_os = "windows", test))]
struct KeyboardInputBuffer {
    sender: std::sync::mpsc::SyncSender<BufferedKeyboardInput>,
    receiver: Mutex<std::sync::mpsc::Receiver<BufferedKeyboardInput>>,
    epoch: AtomicU64,
    reset_pending: AtomicBool,
}

#[cfg(any(target_os = "windows", test))]
impl KeyboardInputBuffer {
    fn new() -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel(KEYBOARD_INPUT_CAPACITY);
        Self {
            sender,
            receiver: Mutex::new(receiver),
            epoch: AtomicU64::new(0),
            reset_pending: AtomicBool::new(false),
        }
    }

    fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.reset_pending.store(true, Ordering::Release);
    }

    fn push(&self, input: BufferedKeyboardInput) -> bool {
        if input.epoch != self.epoch.load(Ordering::Acquire) {
            return false;
        }
        if self.sender.try_send(input).is_err() {
            // shortcut: 超过 256 个待采样键事件时重置可视按键；需完整展示高频宏时再扩容。
            self.invalidate();
            return false;
        }
        true
    }

    fn drain(&self) -> (bool, Vec<BufferedKeyboardInput>) {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let reset = self.reset_pending.swap(false, Ordering::AcqRel);
        let epoch = self.epoch.load(Ordering::Acquire);
        let events = receiver
            .try_iter()
            .take(KEYBOARD_INPUT_CAPACITY)
            .filter(|input| input.epoch == epoch)
            .collect();
        (reset, events)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn is_input_still_active(last_input_at_ms: u64, now_ms: u64, active_window_ms: u64) -> bool {
    last_input_at_ms > 0 && now_ms.saturating_sub(last_input_at_ms) <= active_window_ms
}

fn normalize_keyboard_visual_keys(key_codes: &[u16]) -> Vec<String> {
    let mut result = Vec::new();

    for key_code in key_codes.iter().rev() {
        let visual_key = keyboard_visual_key_from_key_code(*key_code);
        if visual_key.is_empty() || result.iter().any(|item| item == visual_key) {
            continue;
        }
        result.push(visual_key.to_string());
    }

    result
}

fn normalize_keyboard_groups(key_codes: &[u16]) -> Vec<String> {
    let mut result = Vec::new();

    for key_code in key_codes.iter().rev() {
        let group_code = standard_keyboard_group_from_key_code(*key_code);
        let group_label = keyboard_group_label(group_code);
        if group_label == "idle" || result.iter().any(|item| item == group_label) {
            continue;
        }
        result.push(group_label.to_string());
    }

    result
}

fn current_keyboard_key_codes(now_ms: u64) -> Vec<u16> {
    let mut state = KEYBOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state
        .pressed_keys
        .retain(|item| now_ms.saturating_sub(item.pressed_at_ms) <= PRESSED_KEY_STALE_MS);
    KEYBOARD_HAS_PRESSED_KEYS.store(!state.pressed_keys.is_empty(), Ordering::Relaxed);

    state
        .pressed_keys
        .iter()
        .map(|item| item.key_code)
        .collect()
}

fn keyboard_group_label(code: u8) -> &'static str {
    match code {
        KEYBOARD_GROUP_DIGIT_1 => "digit-1",
        KEYBOARD_GROUP_DIGIT_2 => "digit-2",
        KEYBOARD_GROUP_DIGIT_3 => "digit-3",
        KEYBOARD_GROUP_DIGIT_4 => "digit-4",
        KEYBOARD_GROUP_DIGIT_5 => "digit-5",
        KEYBOARD_GROUP_DIGIT_6 => "digit-6",
        KEYBOARD_GROUP_DIGIT_7 => "digit-7",
        KEYBOARD_GROUP_KEY_Q => "key-q",
        KEYBOARD_GROUP_KEY_E => "key-e",
        KEYBOARD_GROUP_KEY_R => "key-r",
        KEYBOARD_GROUP_SPACE => "space",
        KEYBOARD_GROUP_KEY_A => "key-a",
        KEYBOARD_GROUP_KEY_D => "key-d",
        KEYBOARD_GROUP_KEY_S => "key-s",
        KEYBOARD_GROUP_KEY_W => "key-w",
        _ => "idle",
    }
}

fn standard_keyboard_group_from_key_code(key_code: u16) -> u8 {
    match key_code {
        18 => KEYBOARD_GROUP_DIGIT_1,
        19 => KEYBOARD_GROUP_DIGIT_2,
        20 => KEYBOARD_GROUP_DIGIT_3,
        21 => KEYBOARD_GROUP_DIGIT_4,
        23 => KEYBOARD_GROUP_DIGIT_5,
        22 => KEYBOARD_GROUP_DIGIT_6,
        26 => KEYBOARD_GROUP_DIGIT_7,
        12 => KEYBOARD_GROUP_KEY_Q,
        14 => KEYBOARD_GROUP_KEY_E,
        15 => KEYBOARD_GROUP_KEY_R,
        49 => KEYBOARD_GROUP_SPACE,
        0 => KEYBOARD_GROUP_KEY_A,
        2 => KEYBOARD_GROUP_KEY_D,
        1 => KEYBOARD_GROUP_KEY_S,
        13 => KEYBOARD_GROUP_KEY_W,
        _ => 0,
    }
}

fn keyboard_visual_key_from_key_code(key_code: u16) -> &'static str {
    match key_code {
        0 => "KeyA",
        1 => "KeyS",
        2 => "KeyD",
        3 => "KeyF",
        4 => "KeyH",
        5 => "KeyG",
        6 => "KeyZ",
        7 => "KeyX",
        8 => "KeyC",
        9 => "KeyV",
        11 => "KeyB",
        12 => "KeyQ",
        13 => "KeyW",
        14 => "KeyE",
        15 => "KeyR",
        16 => "KeyY",
        17 => "KeyT",
        18 => "Num1",
        19 => "Num2",
        20 => "Num3",
        21 => "Num4",
        22 => "Num6",
        23 => "Num5",
        25 => "Num9",
        26 => "Num7",
        28 => "Num8",
        29 => "Num0",
        31 => "KeyO",
        32 => "KeyU",
        34 => "KeyI",
        35 => "KeyP",
        36 => "Return",
        37 => "KeyL",
        38 => "KeyJ",
        40 => "KeyK",
        43 => "Comma",
        44 => "Slash",
        45 => "KeyN",
        46 => "KeyM",
        47 => "Period",
        48 => "Tab",
        49 => "Space",
        50 => "BackQuote",
        51 => "Backspace",
        53 => "Escape",
        55 => "Meta",
        56 => "ShiftLeft",
        57 => "CapsLock",
        58 => "Alt",
        59 => "ControlLeft",
        60 => "ShiftRight",
        61 => "AltGr",
        62 => "ControlRight",
        63 => "Fn",
        117 => "Delete",
        123 => "LeftArrow",
        124 => "RightArrow",
        125 => "DownArrow",
        126 => "UpArrow",
        _ => "",
    }
}

fn mouse_group_label(code: u8) -> &'static str {
    match code {
        MOUSE_GROUP_LEFT => "mouse-left",
        MOUSE_GROUP_RIGHT => "mouse-right",
        MOUSE_GROUP_SIDE => "mouse-side",
        MOUSE_GROUP_MOVE => "mouse-move",
        _ => "idle",
    }
}

fn linux_session_supports_avatar_input(session: crate::linux_session::LinuxDesktopSession) -> bool {
    matches!(session, crate::linux_session::LinuxDesktopSession::X11)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxAvatarInputSupport {
    pub provider: &'static str,
    pub support_level: &'static str,
    pub keyboard_supported: bool,
    pub mouse_supported: bool,
}

impl LinuxAvatarInputSupport {
    const fn full(provider: &'static str) -> Self {
        Self {
            provider,
            support_level: "full",
            keyboard_supported: true,
            mouse_supported: true,
        }
    }

    const fn mouse_only(provider: &'static str) -> Self {
        Self {
            provider,
            support_level: "mouse-only",
            keyboard_supported: false,
            mouse_supported: true,
        }
    }

    const fn none() -> Self {
        Self {
            provider: "none",
            support_level: "none",
            keyboard_supported: false,
            mouse_supported: false,
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxWaylandMouseProvider {
    GnomeShellDbus,
    KdeKdotool,
    HyprlandHyprctl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LinuxWaylandMouseSnapshot {
    x: i32,
    y: i32,
    group_code: u8,
    keyboard_key_code: Option<u16>,
    keyboard_timestamp_ms: Option<u64>,
}

fn linux_avatar_input_support_for_session(
    session: crate::linux_session::LinuxDesktopSession,
    desktop_environment: crate::linux_session::LinuxDesktopEnvironment,
    gnome_mouse_provider_available: bool,
    kde_mouse_provider_available: bool,
    hyprland_mouse_provider_available: bool,
) -> LinuxAvatarInputSupport {
    use crate::linux_session::{LinuxDesktopEnvironment, LinuxDesktopSession};

    match session {
        LinuxDesktopSession::X11 => LinuxAvatarInputSupport::full("xinput2"),
        LinuxDesktopSession::Wayland => match desktop_environment {
            LinuxDesktopEnvironment::Gnome if gnome_mouse_provider_available => {
                LinuxAvatarInputSupport::full("gnome-shell-dbus")
            }
            LinuxDesktopEnvironment::Kde if kde_mouse_provider_available => {
                LinuxAvatarInputSupport::mouse_only("kdotool-mouselocation")
            }
            LinuxDesktopEnvironment::Hyprland if hyprland_mouse_provider_available => {
                LinuxAvatarInputSupport::mouse_only("hyprctl-cursorpos")
            }
            LinuxDesktopEnvironment::Unknown if gnome_mouse_provider_available => {
                LinuxAvatarInputSupport::full("gnome-shell-dbus")
            }
            LinuxDesktopEnvironment::Unknown if hyprland_mouse_provider_available => {
                LinuxAvatarInputSupport::mouse_only("hyprctl-cursorpos")
            }
            LinuxDesktopEnvironment::Unknown if kde_mouse_provider_available => {
                LinuxAvatarInputSupport::mouse_only("kdotool-mouselocation")
            }
            _ => LinuxAvatarInputSupport::none(),
        },
        LinuxDesktopSession::Unknown => LinuxAvatarInputSupport::none(),
    }
}

fn linux_keysym_to_avatar_key_code(keysym: u64) -> Option<u16> {
    match keysym {
        0x0030 => Some(29),
        0x0031 => Some(18),
        0x0032 => Some(19),
        0x0033 => Some(20),
        0x0034 => Some(21),
        0x0035 => Some(23),
        0x0036 => Some(22),
        0x0037 => Some(26),
        0x0038 => Some(28),
        0x0039 => Some(25),
        0x0041 | 0x0061 => Some(0),
        0x0042 | 0x0062 => Some(11),
        0x0043 | 0x0063 => Some(8),
        0x0044 | 0x0064 => Some(2),
        0x0045 | 0x0065 => Some(14),
        0x0046 | 0x0066 => Some(3),
        0x0047 | 0x0067 => Some(5),
        0x0048 | 0x0068 => Some(4),
        0x0049 | 0x0069 => Some(34),
        0x004A | 0x006A => Some(38),
        0x004B | 0x006B => Some(40),
        0x004C | 0x006C => Some(37),
        0x004D | 0x006D => Some(46),
        0x004E | 0x006E => Some(45),
        0x004F | 0x006F => Some(31),
        0x0050 | 0x0070 => Some(35),
        0x0051 | 0x0071 => Some(12),
        0x0052 | 0x0072 => Some(15),
        0x0053 | 0x0073 => Some(1),
        0x0054 | 0x0074 => Some(17),
        0x0055 | 0x0075 => Some(32),
        0x0056 | 0x0076 => Some(9),
        0x0057 | 0x0077 => Some(13),
        0x0058 | 0x0078 => Some(7),
        0x0059 | 0x0079 => Some(16),
        0x005A | 0x007A => Some(6),
        0x0020 => Some(49),
        0x002C => Some(43),
        0x002E => Some(47),
        0x002F => Some(44),
        0x0060 => Some(50),
        0xFF08 => Some(51),
        0xFF09 => Some(48),
        0xFF0D => Some(36),
        0xFF1B => Some(53),
        0xFF51 => Some(123),
        0xFF52 => Some(126),
        0xFF53 => Some(124),
        0xFF54 => Some(125),
        0xFFFF => Some(117),
        0xFFE1 => Some(56),
        0xFFE2 => Some(60),
        0xFFE3 => Some(59),
        0xFFE4 => Some(62),
        0xFFE5 => Some(57),
        0xFFE7 | 0xFFE8 | 0xFFEB | 0xFFEC => Some(55),
        0xFFE9 => Some(58),
        0xFFEA => Some(61),
        _ => None,
    }
}

fn linux_mouse_group_from_button_detail(detail: i32) -> u8 {
    match detail {
        1 => MOUSE_GROUP_LEFT,
        3 => MOUSE_GROUP_RIGHT,
        2 | 8 | 9 => MOUSE_GROUP_SIDE,
        _ => MOUSE_GROUP_MOVE,
    }
}

fn mouse_group_code_from_label(value: &str) -> u8 {
    match value.trim() {
        "mouse-left" => MOUSE_GROUP_LEFT,
        "mouse-right" => MOUSE_GROUP_RIGHT,
        "mouse-side" => MOUSE_GROUP_SIDE,
        "mouse-move" => MOUSE_GROUP_MOVE,
        _ => MOUSE_GROUP_MOVE,
    }
}

fn parse_gnome_avatar_input_dbus_output(output: &str) -> Option<LinuxWaylandMouseSnapshot> {
    let json_start = output.find('{')?;
    let json_end = output.rfind('}')?;
    let payload = &output[json_start..=json_end];
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;

    let x = value.get("x")?.as_i64()? as i32;
    let y = value.get("y")?.as_i64()? as i32;
    let group_code = value
        .get("mouseGroup")
        .and_then(|entry| entry.as_str())
        .map(mouse_group_code_from_label)
        .unwrap_or(MOUSE_GROUP_MOVE);
    let keyboard_key_code = value
        .get("keyval")
        .and_then(|entry| entry.as_u64())
        .and_then(linux_keysym_to_avatar_key_code);
    let keyboard_timestamp_ms = value
        .get("keyboardTimestampMs")
        .and_then(|entry| entry.as_u64());

    Some(LinuxWaylandMouseSnapshot {
        x,
        y,
        group_code,
        keyboard_key_code,
        keyboard_timestamp_ms,
    })
}

fn parse_kdotool_mouse_location_output(output: &str) -> Option<(i32, i32)> {
    let mut x = None;
    let mut y = None;

    for segment in output.split_whitespace() {
        if let Some(value) = segment.strip_prefix("x:") {
            x = value.parse::<i32>().ok();
        } else if let Some(value) = segment.strip_prefix("y:") {
            y = value.parse::<i32>().ok();
        }
    }

    Some((x?, y?))
}

fn parse_hyprctl_cursorpos_output(output: &str) -> Option<(i32, i32)> {
    let normalized = output.trim().replace(',', " ");
    let mut parts = normalized.split_whitespace();
    let x = parts.next()?.parse::<i32>().ok()?;
    let y = parts.next()?.parse::<i32>().ok()?;
    Some((x, y))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cursor_ratio_from_virtual_desktop_bounds(
    point_x: i32,
    point_y: i32,
    min_x: i32,
    min_y: i32,
    width: u32,
    height: u32,
) -> (f64, f64) {
    let width = width.max(1) as f64;
    let height = height.max(1) as f64;
    (
        ((point_x - min_x) as f64 / width).clamp(0.0, 1.0),
        ((point_y - min_y) as f64 / height).clamp(0.0, 1.0),
    )
}

#[cfg(target_os = "linux")]
fn run_linux_avatar_command_with_timeout(
    command: &mut std::process::Command,
    context: &str,
) -> Option<std::process::Output> {
    use std::process::Stdio;
    use std::thread;
    use std::time::Instant;

    let timeout = Duration::from_millis(1000);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn().ok()?;
    let started_at = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if started_at.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                log::debug!("桌宠输入联动命令超时：{context}");
                return None;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                log::debug!("桌宠输入联动命令执行失败（{context}）：{error}");
                return None;
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn is_kde_wayland_mouse_provider_available() -> bool {
    run_linux_avatar_command_with_timeout(
        std::process::Command::new("kdotool").arg("getmouselocation"),
        "kdotool getmouselocation",
    )
    .and_then(|output| {
        parse_kdotool_mouse_location_output(&String::from_utf8_lossy(&output.stdout))
    })
    .is_some()
}

#[cfg(target_os = "linux")]
fn is_gnome_wayland_mouse_provider_available() -> bool {
    run_linux_avatar_command_with_timeout(
        std::process::Command::new("gdbus").args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/shell/extensions/WorkReviewAvatarInput",
            "--method",
            "org.gnome.shell.extensions.WorkReviewAvatarInput.GetInput",
        ]),
        "gdbus WorkReviewAvatarInput.GetInput",
    )
    .filter(|output| output.status.success())
    .and_then(|output| {
        parse_gnome_avatar_input_dbus_output(&String::from_utf8_lossy(&output.stdout))
    })
    .is_some()
}

#[cfg(target_os = "linux")]
fn is_hyprland_wayland_mouse_provider_available() -> bool {
    run_linux_avatar_command_with_timeout(
        std::process::Command::new("hyprctl").arg("cursorpos"),
        "hyprctl cursorpos",
    )
    .and_then(|output| parse_hyprctl_cursorpos_output(&String::from_utf8_lossy(&output.stdout)))
    .is_some()
}

#[cfg(target_os = "linux")]
fn linux_wayland_mouse_provider_for_environment(
    desktop_environment: crate::linux_session::LinuxDesktopEnvironment,
    gnome_mouse_provider_available: bool,
    kde_mouse_provider_available: bool,
    hyprland_mouse_provider_available: bool,
) -> Option<LinuxWaylandMouseProvider> {
    use crate::linux_session::LinuxDesktopEnvironment;

    match desktop_environment {
        LinuxDesktopEnvironment::Gnome if gnome_mouse_provider_available => {
            Some(LinuxWaylandMouseProvider::GnomeShellDbus)
        }
        LinuxDesktopEnvironment::Kde if kde_mouse_provider_available => {
            Some(LinuxWaylandMouseProvider::KdeKdotool)
        }
        LinuxDesktopEnvironment::Hyprland if hyprland_mouse_provider_available => {
            Some(LinuxWaylandMouseProvider::HyprlandHyprctl)
        }
        LinuxDesktopEnvironment::Unknown if gnome_mouse_provider_available => {
            Some(LinuxWaylandMouseProvider::GnomeShellDbus)
        }
        LinuxDesktopEnvironment::Unknown if hyprland_mouse_provider_available => {
            Some(LinuxWaylandMouseProvider::HyprlandHyprctl)
        }
        LinuxDesktopEnvironment::Unknown if kde_mouse_provider_available => {
            Some(LinuxWaylandMouseProvider::KdeKdotool)
        }
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn query_wayland_mouse_cursor_point(
    provider: LinuxWaylandMouseProvider,
) -> Option<LinuxWaylandMouseSnapshot> {
    match provider {
        LinuxWaylandMouseProvider::GnomeShellDbus => run_linux_avatar_command_with_timeout(
            std::process::Command::new("gdbus").args([
                "call",
                "--session",
                "--dest",
                "org.gnome.Shell",
                "--object-path",
                "/org/gnome/shell/extensions/WorkReviewAvatarInput",
                "--method",
                "org.gnome.shell.extensions.WorkReviewAvatarInput.GetInput",
            ]),
            "gdbus WorkReviewAvatarInput.GetInput",
        )
        .and_then(|output| {
            parse_gnome_avatar_input_dbus_output(&String::from_utf8_lossy(&output.stdout))
        }),
        LinuxWaylandMouseProvider::KdeKdotool => run_linux_avatar_command_with_timeout(
            std::process::Command::new("kdotool").arg("getmouselocation"),
            "kdotool getmouselocation",
        )
        .and_then(|output| {
            parse_kdotool_mouse_location_output(&String::from_utf8_lossy(&output.stdout)).map(
                |(x, y)| LinuxWaylandMouseSnapshot {
                    x,
                    y,
                    group_code: MOUSE_GROUP_MOVE,
                    keyboard_key_code: None,
                    keyboard_timestamp_ms: None,
                },
            )
        }),
        LinuxWaylandMouseProvider::HyprlandHyprctl => run_linux_avatar_command_with_timeout(
            std::process::Command::new("hyprctl").arg("cursorpos"),
            "hyprctl cursorpos",
        )
        .and_then(|output| {
            parse_hyprctl_cursorpos_output(&String::from_utf8_lossy(&output.stdout)).map(
                |(x, y)| LinuxWaylandMouseSnapshot {
                    x,
                    y,
                    group_code: MOUSE_GROUP_MOVE,
                    keyboard_key_code: None,
                    keyboard_timestamp_ms: None,
                },
            )
        }),
    }
}

fn logical_monitor_rect_from_physical(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
) -> Option<(i32, i32, u32, u32)> {
    if !scale_factor.is_finite() || scale_factor <= 0.0 || width == 0 || height == 0 {
        return None;
    }

    let logical_x = (x as f64 / scale_factor).round();
    let logical_y = (y as f64 / scale_factor).round();
    let logical_width = (width as f64 / scale_factor).round();
    let logical_height = (height as f64 / scale_factor).round();

    if !logical_x.is_finite()
        || logical_x < i32::MIN as f64
        || logical_x > i32::MAX as f64
        || !logical_y.is_finite()
        || logical_y < i32::MIN as f64
        || logical_y > i32::MAX as f64
        || !logical_width.is_finite()
        || logical_width < 1.0
        || logical_width > u32::MAX as f64
        || !logical_height.is_finite()
        || logical_height < 1.0
        || logical_height > u32::MAX as f64
    {
        return None;
    }

    Some((
        logical_x as i32,
        logical_y as i32,
        logical_width as u32,
        logical_height as u32,
    ))
}

#[cfg(target_os = "linux")]
fn virtual_desktop_logical_bounds_from_monitors(app: &AppHandle) -> Option<(i32, i32, u32, u32)> {
    let monitors = app.available_monitors().ok()?;
    let mut bounds: Option<(i64, i64, i64, i64)> = None;

    for monitor in monitors {
        let position = monitor.position();
        let size = monitor.size();
        let (x, y, width, height) = logical_monitor_rect_from_physical(
            position.x,
            position.y,
            size.width,
            size.height,
            monitor.scale_factor(),
        )?;
        let left = i64::from(x);
        let top = i64::from(y);
        let right = left.checked_add(i64::from(width))?;
        let bottom = top.checked_add(i64::from(height))?;

        bounds = Some(match bounds {
            Some((min_x, min_y, max_x, max_y)) => (
                min_x.min(left),
                min_y.min(top),
                max_x.max(right),
                max_y.max(bottom),
            ),
            None => (left, top, right, bottom),
        });
    }

    let (min_x, min_y, max_x, max_y) = bounds?;
    let width = u32::try_from(max_x.checked_sub(min_x)?).ok()?;
    let height = u32::try_from(max_y.checked_sub(min_y)?).ok()?;
    if width == 0 || height == 0 {
        return None;
    }

    Some((
        i32::try_from(min_x).ok()?,
        i32::try_from(min_y).ok()?,
        width,
        height,
    ))
}

#[cfg(target_os = "linux")]
fn probe_linux_wayland_mouse_providers(
    session: crate::linux_session::LinuxDesktopSession,
    desktop_environment: crate::linux_session::LinuxDesktopEnvironment,
) -> (bool, bool, bool) {
    use crate::linux_session::{LinuxDesktopEnvironment, LinuxDesktopSession};

    if session != LinuxDesktopSession::Wayland {
        return (false, false, false);
    }

    match desktop_environment {
        LinuxDesktopEnvironment::Gnome => {
            (is_gnome_wayland_mouse_provider_available(), false, false)
        }
        LinuxDesktopEnvironment::Kde => (false, is_kde_wayland_mouse_provider_available(), false),
        LinuxDesktopEnvironment::Hyprland => {
            (false, false, is_hyprland_wayland_mouse_provider_available())
        }
        LinuxDesktopEnvironment::Sway => (false, false, false),
        LinuxDesktopEnvironment::Unknown => (
            is_gnome_wayland_mouse_provider_available(),
            is_kde_wayland_mouse_provider_available(),
            is_hyprland_wayland_mouse_provider_available(),
        ),
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[cfg(target_os = "linux")]
pub fn current_linux_avatar_input_support() -> LinuxAvatarInputSupport {
    let session = crate::linux_session::current_linux_desktop_session();
    let desktop_environment = crate::linux_session::current_linux_desktop_environment();
    let (gnome_available, kde_available, hyprland_available) =
        probe_linux_wayland_mouse_providers(session, desktop_environment);
    linux_avatar_input_support_for_session(
        session,
        desktop_environment,
        gnome_available,
        kde_available,
        hyprland_available,
    )
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[cfg(not(target_os = "linux"))]
pub fn current_linux_avatar_input_support() -> LinuxAvatarInputSupport {
    LinuxAvatarInputSupport::none()
}

#[cfg(target_os = "macos")]
fn mouse_group_from_event_type_raw(event_type_raw: usize) -> u8 {
    use cocoa::appkit::NSEventType;

    if event_type_raw == NSEventType::NSLeftMouseDown as usize {
        MOUSE_GROUP_LEFT
    } else if event_type_raw == NSEventType::NSRightMouseDown as usize {
        MOUSE_GROUP_RIGHT
    } else if event_type_raw == NSEventType::NSOtherMouseDown as usize {
        MOUSE_GROUP_SIDE
    } else {
        MOUSE_GROUP_MOVE
    }
}

#[cfg(target_os = "windows")]
fn windows_virtual_key_to_avatar_key_code(virtual_key: u32) -> Option<u16> {
    match virtual_key {
        0x30 => Some(29),
        0x31 => Some(18),
        0x32 => Some(19),
        0x33 => Some(20),
        0x34 => Some(21),
        0x35 => Some(23),
        0x36 => Some(22),
        0x37 => Some(26),
        0x38 => Some(28),
        0x39 => Some(25),
        0x41 => Some(0),
        0x42 => Some(11),
        0x43 => Some(8),
        0x44 => Some(2),
        0x45 => Some(14),
        0x46 => Some(3),
        0x47 => Some(5),
        0x48 => Some(4),
        0x49 => Some(34),
        0x4A => Some(38),
        0x4B => Some(40),
        0x4C => Some(37),
        0x4D => Some(46),
        0x4E => Some(45),
        0x4F => Some(31),
        0x50 => Some(35),
        0x51 => Some(12),
        0x52 => Some(15),
        0x53 => Some(1),
        0x54 => Some(17),
        0x55 => Some(32),
        0x56 => Some(9),
        0x57 => Some(13),
        0x58 => Some(7),
        0x59 => Some(16),
        0x5A => Some(6),
        0x08 => Some(51),
        0x09 => Some(48),
        0x0D => Some(36),
        0x14 => Some(57),
        0x1B => Some(53),
        0x20 => Some(49),
        0x25 => Some(123),
        0x26 => Some(126),
        0x27 => Some(124),
        0x28 => Some(125),
        0xBC => Some(43),
        0xBE => Some(47),
        0xBF => Some(44),
        0x2E => Some(117),
        0xA0 => Some(56),
        0xA1 => Some(60),
        0xA2 => Some(59),
        0xA3 => Some(62),
        0xA4 => Some(58),
        0xA5 => Some(61),
        0x5B | 0x5C => Some(55),
        0xC0 => Some(50),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn windows_mouse_group_from_message(message: u32) -> u8 {
    use winapi::um::winuser::{
        WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
        WM_RBUTTONDOWN, WM_XBUTTONDOWN,
    };

    match message {
        WM_LBUTTONDOWN => MOUSE_GROUP_LEFT,
        WM_RBUTTONDOWN => MOUSE_GROUP_RIGHT,
        WM_MBUTTONDOWN | WM_XBUTTONDOWN => MOUSE_GROUP_SIDE,
        WM_MOUSEMOVE | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => MOUSE_GROUP_MOVE,
        _ => MOUSE_GROUP_MOVE,
    }
}

#[cfg(target_os = "windows")]
fn cached_windows_virtual_screen_metrics() -> (i32, i32, f64, f64) {
    use winapi::um::winuser::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    const CACHE_TTL: Duration = Duration::from_secs(2);
    let now = std::time::Instant::now();
    {
        let cache = WINDOWS_VIRTUAL_SCREEN_CACHE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((captured_at, metrics)) = *cache {
            if now.duration_since(captured_at) < CACHE_TTL {
                return metrics;
            }
        }
    }

    let min_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let min_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1) as f64;
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1) as f64;
    let metrics = (min_x, min_y, width, height);

    let mut cache = WINDOWS_VIRTUAL_SCREEN_CACHE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *cache = Some((now, metrics));
    metrics
}

#[cfg(target_os = "windows")]
fn cursor_ratio_from_virtual_screen(point_x: i32, point_y: i32) -> (f64, f64) {
    let (min_x, min_y, width, height) = cached_windows_virtual_screen_metrics();
    let x_ratio = ((point_x - min_x) as f64 / width).clamp(0.0, 1.0);
    let y_ratio = ((point_y - min_y) as f64 / height).clamp(0.0, 1.0);

    (x_ratio, y_ratio)
}

#[cfg(target_os = "windows")]
fn buffer_windows_keyboard_input(input: BufferedKeyboardInput) {
    if WINDOWS_KEYBOARD_INPUTS.push(input) && input.pressed {
        LAST_KEYBOARD_INPUT_AT_MS.store(input.pressed_at_ms, Ordering::Relaxed);
        LAST_KEYBOARD_GROUP_CODE.store(input.group_code, Ordering::Relaxed);
        LAST_KEYBOARD_KEY_CODE.store(input.key_code, Ordering::Relaxed);
    }
}

#[cfg(target_os = "windows")]
fn sync_windows_keyboard_input() {
    let (reset, events) = WINDOWS_KEYBOARD_INPUTS.drain();
    if !reset && events.is_empty() {
        return;
    }
    let mut state = KEYBOARD_STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if reset {
        state.pressed_keys.clear();
    }
    for input in events {
        if input.epoch != WINDOWS_KEYBOARD_INPUTS.epoch.load(Ordering::Acquire) {
            continue;
        }
        if input.pressed
            && (!AVATAR_ENABLED_FLAG.load(Ordering::Relaxed)
                || AVATAR_BODY_HIDDEN_FLAG.load(Ordering::Relaxed))
        {
            continue;
        }
        state
            .pressed_keys
            .retain(|key| key.key_code != input.key_code);
        if input.pressed {
            state.pressed_keys.push(PressedKeyState {
                key_code: input.key_code,
                pressed_at_ms: input.pressed_at_ms,
            });
        }
    }
    KEYBOARD_HAS_PRESSED_KEYS.store(!state.pressed_keys.is_empty(), Ordering::Relaxed);
}

#[cfg(target_os = "windows")]
fn sync_windows_cursor_ratio() {
    if !WINDOWS_RAW_CURSOR_DIRTY.swap(false, Ordering::AcqRel) {
        return;
    }

    let (x, y) = unpack_wayland_cursor_position(WINDOWS_RAW_CURSOR_POS.load(Ordering::Relaxed));
    let (cursor_ratio_x, cursor_ratio_y) = cursor_ratio_from_virtual_screen(x, y);
    record_cursor_ratio(cursor_ratio_x, cursor_ratio_y);
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_keyboard_hook_proc(
    code: i32,
    w_param: usize,
    l_param: isize,
) -> isize {
    use std::ptr;
    use winapi::um::winuser::{
        CallNextHookEx, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    let epoch = WINDOWS_KEYBOARD_INPUTS.epoch.load(Ordering::Acquire);
    if code >= 0 && AVATAR_ENABLED_FLAG.load(Ordering::Relaxed) {
        // SAFETY: 非负 Hook code 下，Windows 提供当前回调期间有效的键盘事件结构。
        let keyboard_info = unsafe { &*(l_param as *const KBDLLHOOKSTRUCT) };
        if let Some(key_code) = windows_virtual_key_to_avatar_key_code(keyboard_info.vkCode) {
            match w_param as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN if !AVATAR_BODY_HIDDEN_FLAG.load(Ordering::Relaxed) => {
                    buffer_windows_keyboard_input(BufferedKeyboardInput {
                        epoch,
                        pressed_at_ms: now_ms(),
                        key_code,
                        group_code: standard_keyboard_group_from_key_code(key_code),
                        pressed: true,
                    });
                }
                // 隐藏期间仍入队松键事件；可视状态锁仅由普通采样线程获取。
                WM_KEYUP | WM_SYSKEYUP => {
                    buffer_windows_keyboard_input(BufferedKeyboardInput {
                        epoch,
                        pressed_at_ms: 0,
                        key_code,
                        group_code: 0,
                        pressed: false,
                    });
                }
                _ => {}
            }
        }
    }

    // SAFETY: 将 Windows 提供的原始回调参数原样传递到 Hook 链。
    unsafe { CallNextHookEx(ptr::null_mut(), code, w_param, l_param) }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_mouse_hook_proc(
    code: i32,
    w_param: usize,
    l_param: isize,
) -> isize {
    use std::ptr;
    use winapi::um::winuser::{
        CallNextHookEx, MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL,
        WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
    };

    if code >= 0 && AVATAR_ENABLED_FLAG.load(Ordering::Relaxed) {
        let message = w_param as u32;
        let tracked_message = matches!(
            message,
            WM_MOUSEMOVE
                | WM_MOUSEWHEEL
                | WM_MOUSEHWHEEL
                | WM_LBUTTONDOWN
                | WM_RBUTTONDOWN
                | WM_MBUTTONDOWN
                | WM_XBUTTONDOWN
        );

        if tracked_message {
            // 低级鼠标钩子内仅做无锁原子写入，不在 1000Hz 回调里调用 GetCursorPos/GetSystemMetrics。
            // SAFETY: 这是 Windows 为当前低级鼠标事件提供的有效结构指针。
            let mouse_info = unsafe { &*(l_param as *const MSLLHOOKSTRUCT) };
            record_mouse_input(windows_mouse_group_from_message(message));
            WINDOWS_RAW_CURSOR_POS.store(
                pack_wayland_cursor_position((mouse_info.pt.x, mouse_info.pt.y)),
                Ordering::Relaxed,
            );
            WINDOWS_RAW_CURSOR_DIRTY.store(true, Ordering::Release);
        }
    }

    // SAFETY: 将 Windows 提供的原始回调参数原样传递到 Hook 链。
    unsafe { CallNextHookEx(ptr::null_mut(), code, w_param, l_param) }
}

#[cfg(target_os = "linux")]
unsafe fn linux_query_cursor_ratio(
    display: *mut x11::xlib::Display,
    root_window: x11::xlib::Window,
    screen: i32,
) -> Option<(f64, f64)> {
    use x11::xlib;

    let mut root_return: x11::xlib::Window = 0;
    let mut child_return: x11::xlib::Window = 0;
    let mut root_x = 0;
    let mut root_y = 0;
    let mut win_x = 0;
    let mut win_y = 0;
    let mut mask_return: u32 = 0;

    // SAFETY: 调用方在本线程持有打开的 Display，所有输出参数均为有效局部变量。
    let found = unsafe {
        xlib::XQueryPointer(
            display,
            root_window,
            &mut root_return,
            &mut child_return,
            &mut root_x,
            &mut root_y,
            &mut win_x,
            &mut win_y,
            &mut mask_return,
        )
    };
    if found == xlib::False {
        return None;
    }

    // SAFETY: Display 与 screen 来自同一次有效的 X11 连接。
    let (width, height) = unsafe {
        (
            xlib::XDisplayWidth(display, screen).max(1) as f64,
            xlib::XDisplayHeight(display, screen).max(1) as f64,
        )
    };

    Some((
        (root_x as f64 / width).clamp(0.0, 1.0),
        (root_y as f64 / height).clamp(0.0, 1.0),
    ))
}

pub(crate) fn build_avatar_input_payload(now_ms: u64) -> AvatarInputPayload {
    let last_keyboard_input_at_ms = LAST_KEYBOARD_INPUT_AT_MS.load(Ordering::Relaxed);
    let last_mouse_input_at_ms = LAST_MOUSE_INPUT_AT_MS.load(Ordering::Relaxed);
    let keyboard_group_code = LAST_KEYBOARD_GROUP_CODE.load(Ordering::Relaxed);
    let keyboard_key_code = LAST_KEYBOARD_KEY_CODE.load(Ordering::Relaxed);
    let mouse_group_code = LAST_MOUSE_GROUP_CODE.load(Ordering::Relaxed);
    let keyboard_key_codes = current_keyboard_key_codes(now_ms);
    let keyboard_groups = normalize_keyboard_groups(&keyboard_key_codes);
    let keyboard_visual_keys = normalize_keyboard_visual_keys(&keyboard_key_codes);
    let keyboard_is_pressed = !keyboard_key_codes.is_empty();
    let keyboard_active = keyboard_is_pressed
        || is_input_still_active(last_keyboard_input_at_ms, now_ms, KEYBOARD_ACTIVE_WINDOW_MS);
    let primary_keyboard_group = keyboard_groups
        .first()
        .cloned()
        .unwrap_or_else(|| keyboard_group_label(keyboard_group_code).to_string());
    let primary_keyboard_visual_key = keyboard_visual_keys
        .first()
        .cloned()
        .unwrap_or_else(|| keyboard_visual_key_from_key_code(keyboard_key_code).to_string());

    AvatarInputPayload {
        keyboard_active,
        mouse_active: is_input_still_active(last_mouse_input_at_ms, now_ms, MOUSE_ACTIVE_WINDOW_MS),
        keyboard_group: primary_keyboard_group,
        keyboard_groups,
        keyboard_visual_key: primary_keyboard_visual_key,
        keyboard_visual_keys,
        mouse_group: mouse_group_label(mouse_group_code).to_string(),
        cursor_ratio_x: CURSOR_RATIO_X_PERMILLE.load(Ordering::Relaxed) as f64 / 1000.0,
        cursor_ratio_y: CURSOR_RATIO_Y_PERMILLE.load(Ordering::Relaxed) as f64 / 1000.0,
        last_keyboard_input_at_ms,
        last_mouse_input_at_ms,
    }
}

pub(crate) fn record_keyboard_input(group_code: u8, key_code: u16) {
    if !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed) {
        return;
    }
    let pressed_at_ms = now_ms();
    {
        let mut state = KEYBOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state
            .pressed_keys
            .retain(|existing| existing.key_code != key_code);
        state.pressed_keys.push(PressedKeyState {
            key_code,
            pressed_at_ms,
        });
        KEYBOARD_HAS_PRESSED_KEYS.store(true, Ordering::Relaxed);
    }
    LAST_KEYBOARD_INPUT_AT_MS.store(pressed_at_ms, Ordering::Relaxed);
    LAST_KEYBOARD_GROUP_CODE.store(group_code, Ordering::Relaxed);
    LAST_KEYBOARD_KEY_CODE.store(key_code, Ordering::Relaxed);
}

pub(crate) fn release_keyboard_input(key_code: u16) {
    if !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed) {
        return;
    }
    let next_primary = {
        let mut state = KEYBOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state
            .pressed_keys
            .retain(|existing| existing.key_code != key_code);
        KEYBOARD_HAS_PRESSED_KEYS.store(!state.pressed_keys.is_empty(), Ordering::Relaxed);
        state.pressed_keys.last().copied()
    };

    LAST_KEYBOARD_INPUT_AT_MS.store(now_ms(), Ordering::Relaxed);

    if let Some(primary_key_state) = next_primary {
        LAST_KEYBOARD_GROUP_CODE.store(
            standard_keyboard_group_from_key_code(primary_key_state.key_code),
            Ordering::Relaxed,
        );
        LAST_KEYBOARD_KEY_CODE.store(primary_key_state.key_code, Ordering::Relaxed);
    }
}

pub(crate) fn record_mouse_input(group_code: u8) {
    if !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed) {
        return;
    }
    LAST_MOUSE_INPUT_AT_MS.store(now_ms(), Ordering::Relaxed);
    LAST_MOUSE_GROUP_CODE.store(group_code, Ordering::Relaxed);
}

pub(crate) fn record_cursor_ratio(x_ratio: f64, y_ratio: f64) {
    if !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed) {
        return;
    }
    let to_permille = |value: f64| -> u32 { (value.clamp(0.0, 1.0) * 1000.0).round() as u32 };

    CURSOR_RATIO_X_PERMILLE.store(to_permille(x_ratio), Ordering::Relaxed);
    CURSOR_RATIO_Y_PERMILLE.store(to_permille(y_ratio), Ordering::Relaxed);
    CURSOR_RATIO_INITIALIZED.store(true, Ordering::Release);
}

fn can_skip_idle_input_payload(last: &AvatarInputPayload) -> bool {
    if last.keyboard_active || last.mouse_active {
        return false;
    }
    if KEYBOARD_HAS_PRESSED_KEYS.load(Ordering::Relaxed) {
        return false;
    }
    if LAST_KEYBOARD_INPUT_AT_MS.load(Ordering::Relaxed) != last.last_keyboard_input_at_ms
        || LAST_MOUSE_INPUT_AT_MS.load(Ordering::Relaxed) != last.last_mouse_input_at_ms
    {
        return false;
    }
    let ratio_x = CURSOR_RATIO_X_PERMILLE.load(Ordering::Relaxed) as f64 / 1000.0;
    let ratio_y = CURSOR_RATIO_Y_PERMILLE.load(Ordering::Relaxed) as f64 / 1000.0;
    last.cursor_ratio_x == ratio_x && last.cursor_ratio_y == ratio_y
}

pub fn spawn_avatar_input_bridge(app: AppHandle) {
    let bridge_gen = INPUT_BRIDGE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let smart_click_through_polling_enabled = smart_click_through_polling_enabled_for_platform();

    tauri::async_runtime::spawn(async move {
        let mut last_payload: Option<AvatarInputPayload> = None;
        let mut active_tick: u32 = 0;
        let mut idle_ticks: u32 = 0;

        loop {
            if INPUT_BRIDGE_GEN.load(Ordering::Acquire) != bridge_gen
                || !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed)
            {
                return;
            }

            #[cfg(target_os = "windows")]
            sync_windows_keyboard_input();

            if AVATAR_BODY_HIDDEN_FLAG.load(Ordering::Relaxed) {
                last_payload = None;
                idle_ticks = 0;
                if smart_click_through_polling_enabled {
                    apply_smart_click_through(&app);
                }
                tokio::time::sleep(INPUT_BRIDGE_IDLE_POLL_INTERVAL).await;
                continue;
            }

            #[cfg(target_os = "windows")]
            sync_windows_cursor_ratio();

            let mut changed = false;
            let skip_idle_build = last_payload
                .as_ref()
                .is_some_and(can_skip_idle_input_payload);
            if !skip_idle_build {
                let next_payload = build_avatar_input_payload(now_ms());
                if last_payload.as_ref() != Some(&next_payload) {
                    crate::avatar_engine::emit_avatar_input(&app, &next_payload);
                    last_payload = Some(next_payload);
                    changed = true;
                }
            }

            let is_active = changed
                || last_payload
                    .as_ref()
                    .is_some_and(|payload| payload.keyboard_active || payload.mouse_active);

            if is_active {
                idle_ticks = 0;
                active_tick = active_tick.wrapping_add(1);
                if smart_click_through_polling_enabled && active_tick.is_multiple_of(4) {
                    apply_smart_click_through(&app);
                }
                tokio::time::sleep(INPUT_BRIDGE_POLL_INTERVAL).await;
            } else {
                idle_ticks = idle_ticks.saturating_add(1);
                if idle_ticks >= 4 {
                    if smart_click_through_polling_enabled {
                        apply_smart_click_through(&app);
                    }
                    tokio::time::sleep(INPUT_BRIDGE_IDLE_POLL_INTERVAL).await;
                } else {
                    active_tick = active_tick.wrapping_add(1);
                    if smart_click_through_polling_enabled && active_tick.is_multiple_of(4) {
                        apply_smart_click_through(&app);
                    }
                    tokio::time::sleep(INPUT_BRIDGE_POLL_INTERVAL).await;
                }
            }
        }
    });
}

/// 所有平台均启用智能穿透轮询。
///
/// 此前 Windows 被显式禁用（`!is_windows`），导致开启穿透后整个窗口永久穿透，
/// 通知卡片按钮完全无法点击（issue #137 诉求三）。现有 `apply_smart_click_through`
/// 已有缓存去抖（`AVATAR_CLICK_THROUGH_IGNORE_CACHE`，仅在状态变化时才调 setter），
/// 轮询间隔 ~64ms 且状态不变时不触发任何窗口 API，无高频闪烁风险。
fn smart_click_through_polling_enabled_for_platform() -> bool {
    true
}

fn should_ignore_avatar_cursor(
    precise: bool,
    click_through_on: bool,
    cursor_hit: Option<bool>,
) -> bool {
    if precise {
        // 精确模式拿不到坐标时优先保证通知可点击，避免整个窗口永久穿透。
        cursor_hit.is_some_and(|hit| !hit)
    } else {
        // 非精确模式保持旧语义：穿透开启且无法确认命中时继续穿透。
        click_through_on && !cursor_hit.unwrap_or(false)
    }
}

/// 智能穿透：精确模式仅在前端交互区域内关闭穿透；非精确模式保留用户开关语义。
/// 仅当目标状态与缓存不同时（或 force_resync 重置后）才经主线程调 setter，避免高频闪烁。
fn apply_smart_click_through(app: &AppHandle) {
    if !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed) {
        return; // 桌宠禁用，不触碰穿透
    }
    let precise = crate::avatar_engine::avatar_precise_hit_testing_enabled();
    let click_through_on = AVATAR_CLICK_THROUGH_ENABLED.load(Ordering::Relaxed);
    let cursor_hit = if precise || click_through_on {
        crate::avatar_engine::avatar_window_hit_by_cursor(app)
    } else {
        None
    };
    let desired_ignore = should_ignore_avatar_cursor(precise, click_through_on, cursor_hit);
    let desired_cache: i8 = if desired_ignore { 1 } else { 0 };
    if AVATAR_CLICK_THROUGH_IGNORE_CACHE.swap(desired_cache, Ordering::Relaxed) != desired_cache {
        let app_clone = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            crate::avatar_engine::set_avatar_click_through(&app_clone, desired_ignore);
        }) {
            force_resync_click_through();
            log::debug!("桌宠穿透更新未送达主线程，将重试: {error}");
        }
    }
}

fn select_cursor_position(
    prefer_provider_position: bool,
    native_position: Option<(i64, i64)>,
    provider_position: Option<(i64, i64)>,
) -> Option<(i64, i64)> {
    if prefer_provider_position {
        provider_position
    } else {
        native_position.or(provider_position)
    }
}

fn logical_cursor_position_to_target(
    position: (i32, i32),
    target_scale_factor: f64,
) -> Option<(i64, i64)> {
    if !target_scale_factor.is_finite() || target_scale_factor <= 0.0 {
        return None;
    }

    let target_x = (position.0 as f64 * target_scale_factor).round();
    let target_y = (position.1 as f64 * target_scale_factor).round();
    if !target_x.is_finite()
        || target_x < i64::MIN as f64
        || target_x > i64::MAX as f64
        || !target_y.is_finite()
        || target_y < i64::MIN as f64
        || target_y > i64::MAX as f64
    {
        return None;
    }

    Some((target_x as i64, target_y as i64))
}

#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn pack_wayland_cursor_position(position: (i32, i32)) -> u64 {
    (u64::from(position.0 as u32) << 32) | u64::from(position.1 as u32)
}

#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn unpack_wayland_cursor_position(position: u64) -> (i32, i32) {
    ((position >> 32) as u32 as i32, position as u32 as i32)
}

fn fresh_wayland_cursor_position(
    position: (i32, i32),
    updated_at_ms: u64,
    current_time_ms: u64,
    target_scale_factor: f64,
) -> Option<(i64, i64)> {
    if updated_at_ms == 0 {
        return None;
    }

    let age_ms = current_time_ms.checked_sub(updated_at_ms)?;
    if age_ms > WAYLAND_CURSOR_STALE_MS {
        return None;
    }

    logical_cursor_position_to_target(position, target_scale_factor)
}

#[cfg(target_os = "linux")]
fn record_wayland_cursor_position(position: (i32, i32)) {
    WAYLAND_CURSOR_POSITION.store(pack_wayland_cursor_position(position), Ordering::Relaxed);
    // 时间戳使用 Release 发布，读取端 Acquire 后可见对应的完整 packed 坐标。
    WAYLAND_CURSOR_UPDATED_AT_MS.store(now_ms(), Ordering::Release);
}

#[cfg(target_os = "linux")]
fn recorded_wayland_cursor_position(target_scale_factor: f64) -> Option<(i64, i64)> {
    let updated_at_ms = WAYLAND_CURSOR_UPDATED_AT_MS.load(Ordering::Acquire);
    let position = WAYLAND_CURSOR_POSITION.load(Ordering::Relaxed);
    fresh_wayland_cursor_position(
        unpack_wayland_cursor_position(position),
        updated_at_ms,
        now_ms(),
        target_scale_factor,
    )
}

fn scale_cursor_position_to_target(
    position: (f64, f64),
    source_scale_factor: f64,
    target_scale_factor: f64,
) -> Option<(i64, i64)> {
    if !position.0.is_finite()
        || !position.1.is_finite()
        || !source_scale_factor.is_finite()
        || source_scale_factor <= 0.0
        || !target_scale_factor.is_finite()
        || target_scale_factor <= 0.0
    {
        return None;
    }

    Some((
        (position.0 / source_scale_factor * target_scale_factor).round() as i64,
        (position.1 / source_scale_factor * target_scale_factor).round() as i64,
    ))
}

fn native_cursor_position(app: &AppHandle, target_scale_factor: f64) -> Option<(i64, i64)> {
    let position = app.cursor_position().ok()?;

    #[cfg(target_os = "macos")]
    {
        // Tao 使用主显示器 DPR 把 NSEvent 的全局逻辑坐标转成物理坐标，
        // 而窗口位置使用窗口所在显示器 DPR；先还原逻辑坐标，再转到目标窗口 DPR。
        let source_scale_factor = app.primary_monitor().ok().flatten()?.scale_factor();
        scale_cursor_position_to_target(
            (position.x, position.y),
            source_scale_factor,
            target_scale_factor,
        )
    }

    #[cfg(not(target_os = "macos"))]
    {
        scale_cursor_position_to_target(
            (position.x, position.y),
            target_scale_factor,
            target_scale_factor,
        )
    }
}

fn cursor_position_from_recorded_ratio(app: &AppHandle) -> Option<(i64, i64)> {
    if !CURSOR_RATIO_INITIALIZED.load(Ordering::Acquire) {
        return None;
    }

    let ratio_x = CURSOR_RATIO_X_PERMILLE.load(Ordering::Relaxed) as f64 / 1000.0;
    let ratio_y = CURSOR_RATIO_Y_PERMILLE.load(Ordering::Relaxed) as f64 / 1000.0;
    let monitors = app.available_monitors().ok()?;
    if monitors.is_empty() {
        return None;
    }
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for monitor in &monitors {
        let position = monitor.position();
        let size = monitor.size();
        min_x = min_x.min(position.x);
        min_y = min_y.min(position.y);
        max_x = max_x.max(position.x.saturating_add(size.width as i32));
        max_y = max_y.max(position.y.saturating_add(size.height as i32));
    }
    let width = max_x.checked_sub(min_x)?;
    let height = max_y.checked_sub(min_y)?;
    if width <= 0 || height <= 0 {
        return None;
    }
    let abs_x = min_x as f64 + ratio_x * width as f64;
    let abs_y = min_y as f64 + ratio_y * height as f64;
    Some((abs_x.round() as i64, abs_y.round() as i64))
}

/// 全局鼠标绝对坐标（目标窗口物理像素，y 向下）。
/// macOS、Windows 和 X11 优先使用 Tauri 原生绝对坐标，避免依赖输入监听权限；
/// Tao 在 Wayland 下会返回占位坐标，因此直接使用 provider 的合成器逻辑坐标。
pub fn global_cursor_position(app: &AppHandle, target_scale_factor: f64) -> Option<(i64, i64)> {
    #[cfg(target_os = "linux")]
    let prefer_provider_position = matches!(
        crate::linux_session::current_linux_desktop_session(),
        crate::linux_session::LinuxDesktopSession::Wayland
    );
    #[cfg(not(target_os = "linux"))]
    let prefer_provider_position = false;

    let native_position = if prefer_provider_position {
        None
    } else {
        native_cursor_position(app, target_scale_factor)
    };
    #[cfg(target_os = "linux")]
    let provider_position = if prefer_provider_position {
        recorded_wayland_cursor_position(target_scale_factor)
    } else {
        cursor_position_from_recorded_ratio(app)
    };
    #[cfg(not(target_os = "linux"))]
    let provider_position = cursor_position_from_recorded_ratio(app);

    select_cursor_position(prefer_provider_position, native_position, provider_position)
}

fn should_retry_avatar_input_monitor(
    monitor_started: bool,
    has_accessibility_permission: bool,
    has_input_monitoring_permission: bool,
) -> bool {
    !monitor_started && has_accessibility_permission && has_input_monitoring_permission
}

#[cfg(target_os = "macos")]
pub fn spawn_avatar_input_monitor_retry(app: AppHandle) {
    let retry_gen = INPUT_MONITOR_RETRY_GEN.fetch_add(1, Ordering::SeqCst) + 1;

    tauri::async_runtime::spawn(async move {
        loop {
            if INPUT_MONITOR_RETRY_GEN.load(Ordering::Acquire) != retry_gen
                || !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed)
            {
                return;
            }
            let monitor_started = INPUT_MONITOR_STARTED.load(Ordering::SeqCst);
            if !monitor_started {
                let has_accessibility_permission =
                    crate::screenshot::has_accessibility_permission(false);
                let has_input_monitoring_permission =
                    crate::screenshot::has_input_monitoring_permission();

                if should_retry_avatar_input_monitor(
                    monitor_started,
                    has_accessibility_permission,
                    has_input_monitoring_permission,
                ) {
                    log::info!("桌宠输入联动权限已就绪，重新注册全局键鼠监听");
                    start_avatar_input_monitor(&app);
                }
            }

            tokio::time::sleep(INPUT_MONITOR_RETRY_INTERVAL).await;
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn spawn_avatar_input_monitor_retry(_app: AppHandle) {}

#[cfg(target_os = "macos")]
type MacosScreenBoundsCache = Option<(std::time::Instant, (f64, f64, f64, f64))>;

#[cfg(target_os = "macos")]
thread_local! {
    static MACOS_INPUT_MONITOR: std::cell::RefCell<Option<MacosInputMonitor>> =
        const { std::cell::RefCell::new(None) };
    static MACOS_SCREEN_BOUNDS_CACHE: std::cell::RefCell<MacosScreenBoundsCache> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
struct MacosInputMonitor {
    keyboard_monitor: cocoa::base::id,
    mouse_monitor: cocoa::base::id,
    keyboard_handler: block::RcBlock<(cocoa::base::id,), ()>,
    mouse_handler: block::RcBlock<(cocoa::base::id,), ()>,
}

#[cfg(target_os = "macos")]
unsafe fn remove_macos_input_monitors() {
    use cocoa::base::nil;

    MACOS_INPUT_MONITOR.with(|slot| {
        if let Some(monitor) = slot.borrow_mut().take() {
            let event_class = class!(NSEvent);
            // SAFETY: 主线程取出由 NSEvent 注册的监听句柄，每个句柄仅移除一次。
            if monitor.keyboard_monitor != nil {
                let _: () =
                    unsafe { msg_send![event_class, removeMonitor: monitor.keyboard_monitor] };
            }
            if monitor.mouse_monitor != nil {
                let _: () = unsafe { msg_send![event_class, removeMonitor: monitor.mouse_monitor] };
            }
        }
    });
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn try_claim_input_monitor_generation() -> Option<u64> {
    let _guard = INPUT_MONITOR_LIFECYCLE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !AVATAR_ENABLED_FLAG.load(Ordering::SeqCst) || INPUT_MONITOR_STARTED.load(Ordering::SeqCst) {
        return None;
    }
    let my_gen = INPUT_MONITOR_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    INPUT_MONITOR_STARTED.store(true, Ordering::SeqCst);
    Some(my_gen)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn release_input_monitor_generation(my_gen: u64) {
    let _guard = INPUT_MONITOR_LIFECYCLE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if INPUT_MONITOR_GEN.load(Ordering::SeqCst) == my_gen {
        INPUT_MONITOR_STARTED.store(false, Ordering::SeqCst);
    }
}

#[cfg(target_os = "macos")]
pub fn start_avatar_input_monitor(app: &AppHandle) {
    use block::ConcreteBlock;
    use cocoa::appkit::{NSEventMask, NSEventType};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSPoint, NSRect};

    unsafe fn current_cursor_ratio() -> (f64, f64) {
        const SCREEN_BOUNDS_TTL: Duration = Duration::from_secs(2);
        let now = std::time::Instant::now();
        let bounds = MACOS_SCREEN_BOUNDS_CACHE.with(|cache| {
            if let Some((captured_at, cached)) = *cache.borrow() {
                if now.duration_since(captured_at) < SCREEN_BOUNDS_TTL {
                    return Some(cached);
                }
            }

            // SAFETY: 回调位于 AppKit 主线程，屏幕数组在当前自动释放池内有效。
            let (screens, count): (id, usize) = unsafe {
                let screens: id = msg_send![class!(NSScreen), screens];
                let count: usize = msg_send![screens, count];
                (screens, count)
            };
            if count == 0 {
                return None;
            }

            let mut min_x = 0.0;
            let mut min_y = 0.0;
            let mut max_x = 0.0;
            let mut max_y = 0.0;

            for index in 0..count {
                // SAFETY: index 已受数组 count 限制，返回的 NSScreen 在本次遍历中有效。
                let frame: NSRect = unsafe {
                    let screen: id = msg_send![screens, objectAtIndex: index];
                    msg_send![screen, frame]
                };
                let left = frame.origin.x;
                let bottom = frame.origin.y;
                let right = frame.origin.x + frame.size.width;
                let top = frame.origin.y + frame.size.height;

                if index == 0 {
                    min_x = left;
                    min_y = bottom;
                    max_x = right;
                    max_y = top;
                } else {
                    min_x = min_x.min(left);
                    min_y = min_y.min(bottom);
                    max_x = max_x.max(right);
                    max_y = max_y.max(top);
                }
            }

            let computed = (min_x, min_y, max_x, max_y);
            *cache.borrow_mut() = Some((now, computed));
            Some(computed)
        });

        let Some((min_x, min_y, max_x, max_y)) = bounds else {
            return (0.5, 0.5);
        };

        // SAFETY: 在已初始化 AppKit 的主线程读取全局鼠标位置。
        let point: NSPoint = unsafe { msg_send![class!(NSEvent), mouseLocation] };
        let width = (max_x - min_x).max(1.0);
        let height = (max_y - min_y).max(1.0);
        let x_ratio = ((point.x - min_x) / width).clamp(0.0, 1.0);
        let y_ratio = (1.0 - ((point.y - min_y) / height)).clamp(0.0, 1.0);

        (x_ratio, y_ratio)
    }

    unsafe fn is_macos_modifier_key_pressed(event: id, key_code: u16) -> Option<bool> {
        // SAFETY: event 是当前 NSEvent 回调提供的有效对象。
        let modifier_flags: u64 = unsafe { msg_send![event, modifierFlags] };
        let active = match key_code {
            55 | 54 => modifier_flags & (1 << 20) != 0,
            56 | 60 => modifier_flags & (1 << 17) != 0,
            58 | 61 => modifier_flags & (1 << 19) != 0,
            59 | 62 => modifier_flags & (1 << 18) != 0,
            57 => modifier_flags & (1 << 16) != 0,
            63 => modifier_flags & (1 << 23) != 0,
            _ => return None,
        };

        Some(active)
    }

    if INPUT_MONITOR_STARTED.load(Ordering::SeqCst) {
        return;
    }

    if !crate::screenshot::has_accessibility_permission(false) {
        log::warn!("桌宠输入联动未启动：缺少辅助功能权限");
        return;
    }

    if !crate::screenshot::has_input_monitoring_permission() {
        log::warn!("桌宠输入联动未启动：缺少输入监控权限");
        return;
    }

    let Some(my_gen) = try_claim_input_monitor_generation() else {
        return;
    };

    let run_result = app.run_on_main_thread(move || unsafe {
        if INPUT_MONITOR_GEN.load(Ordering::Acquire) != my_gen {
            return;
        }
        MACOS_INPUT_MONITOR.with(|slot| {
            let event_class = class!(NSEvent);
            if let Some(old) = slot.borrow_mut().take() {
                if old.keyboard_monitor != nil {
                    let _: () = msg_send![event_class, removeMonitor: old.keyboard_monitor];
                }
                if old.mouse_monitor != nil {
                    let _: () = msg_send![event_class, removeMonitor: old.mouse_monitor];
                }
            }

            let keyboard_handler = ConcreteBlock::new(|event: id| {
                if !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed) {
                    return;
                }
                let body_hidden = AVATAR_BODY_HIDDEN_FLAG.load(Ordering::Relaxed);
                let event_type_raw: usize = msg_send![event, type];
                let key_code: u16 = msg_send![event, keyCode];
                if event_type_raw == NSEventType::NSKeyUp as usize {
                    release_keyboard_input(key_code);
                } else if event_type_raw == NSEventType::NSFlagsChanged as usize {
                    if is_macos_modifier_key_pressed(event, key_code).unwrap_or(true) {
                        if !body_hidden {
                            record_keyboard_input(
                                standard_keyboard_group_from_key_code(key_code),
                                key_code,
                            );
                        }
                    } else {
                        release_keyboard_input(key_code);
                    }
                } else if !body_hidden {
                    record_keyboard_input(
                        standard_keyboard_group_from_key_code(key_code),
                        key_code,
                    );
                }
            })
            .copy();
            let mouse_handler = ConcreteBlock::new(|event: id| {
                if !AVATAR_ENABLED_FLAG.load(Ordering::Relaxed)
                    || AVATAR_BODY_HIDDEN_FLAG.load(Ordering::Relaxed)
                {
                    return;
                }
                let event_type_raw: usize = msg_send![event, type];
                record_mouse_input(mouse_group_from_event_type_raw(event_type_raw));
                let (cursor_ratio_x, cursor_ratio_y) = current_cursor_ratio();
                record_cursor_ratio(cursor_ratio_x, cursor_ratio_y);
            })
            .copy();

            let keyboard_mask = (NSEventMask::NSKeyDownMask
                | NSEventMask::NSKeyUpMask
                | NSEventMask::NSFlagsChangedMask)
                .bits();
            let mouse_mask = (NSEventMask::NSLeftMouseDownMask
                | NSEventMask::NSRightMouseDownMask
                | NSEventMask::NSOtherMouseDownMask
                | NSEventMask::NSMouseMovedMask
                | NSEventMask::NSScrollWheelMask)
                .bits();

            let keyboard_monitor: id = msg_send![
                event_class,
                addGlobalMonitorForEventsMatchingMask: keyboard_mask
                handler: &*keyboard_handler
            ];
            let mouse_monitor: id = msg_send![
                event_class,
                addGlobalMonitorForEventsMatchingMask: mouse_mask
                handler: &*mouse_handler
            ];

            if keyboard_monitor == nil || mouse_monitor == nil {
                if keyboard_monitor != nil {
                    let _: () = msg_send![event_class, removeMonitor: keyboard_monitor];
                }
                if mouse_monitor != nil {
                    let _: () = msg_send![event_class, removeMonitor: mouse_monitor];
                }
                release_input_monitor_generation(my_gen);
                log::warn!("桌宠输入联动注册失败：系统未返回有效的全局监听句柄");
                return;
            }

            if INPUT_MONITOR_GEN.load(Ordering::Acquire) != my_gen {
                let _: () = msg_send![event_class, removeMonitor: keyboard_monitor];
                let _: () = msg_send![event_class, removeMonitor: mouse_monitor];
                return;
            }

            slot.replace(Some(MacosInputMonitor {
                keyboard_monitor,
                mouse_monitor,
                keyboard_handler,
                mouse_handler,
            }));
        });
    });

    if let Err(e) = run_result {
        release_input_monitor_generation(my_gen);
        log::warn!("桌宠输入联动注册失败: {e}");
    }
}

#[cfg(target_os = "windows")]
pub fn start_avatar_input_monitor(_app: &AppHandle) {
    use std::{mem, ptr, thread};
    use winapi::um::libloaderapi::GetModuleHandleW;
    use winapi::um::processthreadsapi::GetCurrentThreadId;
    use winapi::um::winuser::{
        DispatchMessageW, GetMessageW, PeekMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL,
    };

    let Some(my_gen) = try_claim_input_monitor_generation() else {
        return;
    };

    thread::spawn(move || unsafe {
        if INPUT_MONITOR_GEN.load(Ordering::Acquire) != my_gen {
            release_input_monitor_generation(my_gen);
            return;
        }

        // 在注册 Hook 前完成有界队列分配，回调里只使用非阻塞入队和原子更新。
        Lazy::force(&WINDOWS_KEYBOARD_INPUTS);
        let module_handle = GetModuleHandleW(ptr::null());
        let keyboard_hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(windows_keyboard_hook_proc),
            module_handle,
            0,
        );
        let mouse_hook =
            SetWindowsHookExW(WH_MOUSE_LL, Some(windows_mouse_hook_proc), module_handle, 0);

        if keyboard_hook.is_null() || mouse_hook.is_null() {
            if !keyboard_hook.is_null() {
                UnhookWindowsHookEx(keyboard_hook);
            }
            if !mouse_hook.is_null() {
                UnhookWindowsHookEx(mouse_hook);
            }
            release_input_monitor_generation(my_gen);
            log::warn!("桌宠输入联动注册失败：Windows 低级键鼠 Hook 初始化失败");
            return;
        }

        // 先通过 PeekMessageW 建立线程消息队列，再在生命周期锁内校验代次并发布线程 ID，
        // 确保与 stop_avatar_input_monitor 的 WM_QUIT 投递绝不交错丢消息或覆盖新代次。
        let mut message: MSG = mem::zeroed();
        PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_NOREMOVE);
        let thread_id = GetCurrentThreadId();
        let should_run = {
            let _guard = INPUT_MONITOR_LIFECYCLE_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if INPUT_MONITOR_GEN.load(Ordering::SeqCst) == my_gen {
                WINDOWS_HOOK_THREAD_ID.store(thread_id, Ordering::Release);
                true
            } else {
                false
            }
        };

        if should_run {
            loop {
                let result = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
                if result == -1 {
                    log::warn!("桌宠输入联动消息循环异常退出：Windows 消息泵返回错误");
                    break;
                }
                if result == 0 || INPUT_MONITOR_GEN.load(Ordering::Acquire) != my_gen {
                    break;
                }

                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }

        {
            let _guard = INPUT_MONITOR_LIFECYCLE_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let _ = WINDOWS_HOOK_THREAD_ID.compare_exchange(
                thread_id,
                0,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
        UnhookWindowsHookEx(keyboard_hook);
        UnhookWindowsHookEx(mouse_hook);
        release_input_monitor_generation(my_gen);
    });
}

#[cfg(target_os = "linux")]
pub fn start_avatar_input_monitor(app: &AppHandle) {
    use crate::linux_session::{
        current_linux_desktop_environment, current_linux_desktop_session, LinuxDesktopSession,
    };
    use std::{ffi::CString, mem, ptr, thread, time::Instant};
    use x11::{xinput2, xlib};

    let session = current_linux_desktop_session();
    let desktop_environment = current_linux_desktop_environment();
    let (
        gnome_mouse_provider_available,
        kde_mouse_provider_available,
        hyprland_mouse_provider_available,
    ) = probe_linux_wayland_mouse_providers(session, desktop_environment);
    let support = linux_avatar_input_support_for_session(
        session,
        desktop_environment,
        gnome_mouse_provider_available,
        kde_mouse_provider_available,
        hyprland_mouse_provider_available,
    );

    if support.support_level == "none" {
        log::warn!(
            "桌宠输入联动未启动：Linux {} / {} 会话暂不支持可用的桌宠输入 provider",
            session.as_str(),
            desktop_environment.as_str()
        );
        return;
    }

    let Some(my_gen) = try_claim_input_monitor_generation() else {
        return;
    };

    match session {
        LinuxDesktopSession::X11 => {
            thread::spawn(move || unsafe {
                let display = xlib::XOpenDisplay(ptr::null());
                if display.is_null() {
                    release_input_monitor_generation(my_gen);
                    log::warn!("桌宠输入联动注册失败：无法连接到 X11 Display");
                    return;
                }

                let screen = xlib::XDefaultScreen(display);
                let root_window = xlib::XRootWindow(display, screen);

                let mut opcode = 0;
                let mut first_event = 0;
                let mut first_error = 0;
                let xinput_extension = CString::new("XInputExtension").expect("固定字符串不应失败");
                if xlib::XQueryExtension(
                    display,
                    xinput_extension.as_ptr(),
                    &mut opcode,
                    &mut first_event,
                    &mut first_error,
                ) == xlib::False
                {
                    xlib::XCloseDisplay(display);
                    release_input_monitor_generation(my_gen);
                    log::warn!("桌宠输入联动注册失败：XInputExtension 不可用");
                    return;
                }

                let mut major = xinput2::XI_2_Major;
                let mut minor = xinput2::XI_2_Minor;
                if xinput2::XIQueryVersion(display, &mut major, &mut minor) != xlib::Success as i32
                {
                    xlib::XCloseDisplay(display);
                    release_input_monitor_generation(my_gen);
                    log::warn!("桌宠输入联动注册失败：XInput2 不可用");
                    return;
                }

                let mut mask = [0_u8; 4];
                xinput2::XISetMask(&mut mask, xinput2::XI_RawKeyPress);
                xinput2::XISetMask(&mut mask, xinput2::XI_RawKeyRelease);
                xinput2::XISetMask(&mut mask, xinput2::XI_RawButtonPress);
                xinput2::XISetMask(&mut mask, xinput2::XI_RawMotion);

                let mut event_mask = xinput2::XIEventMask {
                    deviceid: xinput2::XIAllMasterDevices,
                    mask_len: mask.len() as i32,
                    mask: mask.as_mut_ptr(),
                };

                if xinput2::XISelectEvents(display, root_window, &mut event_mask, 1)
                    != xlib::Success as i32
                {
                    xlib::XCloseDisplay(display);
                    release_input_monitor_generation(my_gen);
                    log::warn!("桌宠输入联动注册失败：XInput2 事件订阅失败");
                    return;
                }

                xlib::XFlush(display);

                let mut last_motion_query: Option<Instant> = None;
                let mut last_event_at = Instant::now();

                loop {
                    if INPUT_MONITOR_GEN.load(Ordering::Acquire) != my_gen {
                        break;
                    }

                    if xlib::XPending(display) == 0 {
                        let sleep_ms = if last_event_at.elapsed() > Duration::from_secs(1) {
                            32
                        } else {
                            8
                        };
                        thread::sleep(Duration::from_millis(sleep_ms));
                        continue;
                    }

                    let mut event: xlib::XEvent = mem::zeroed();
                    xlib::XNextEvent(display, &mut event);
                    last_event_at = Instant::now();

                    if event.get_type() != xlib::GenericEvent {
                        continue;
                    }

                    let cookie = &mut event.generic_event_cookie;
                    if cookie.extension != opcode
                        || xlib::XGetEventData(display, cookie) != xlib::True
                    {
                        continue;
                    }

                    let body_hidden = AVATAR_BODY_HIDDEN_FLAG.load(Ordering::Relaxed);
                    match cookie.evtype {
                        xinput2::XI_RawKeyPress if !body_hidden => {
                            let raw_event = &*(cookie.data as *const xinput2::XIRawEvent);
                            let keysym =
                                xlib::XkbKeycodeToKeysym(display, raw_event.detail as u8, 0, 0);
                            if let Some(key_code) = linux_keysym_to_avatar_key_code(keysym as u64) {
                                record_keyboard_input(
                                    standard_keyboard_group_from_key_code(key_code),
                                    key_code,
                                );
                            }
                        }
                        xinput2::XI_RawKeyRelease => {
                            let raw_event = &*(cookie.data as *const xinput2::XIRawEvent);
                            let keysym =
                                xlib::XkbKeycodeToKeysym(display, raw_event.detail as u8, 0, 0);
                            if let Some(key_code) = linux_keysym_to_avatar_key_code(keysym as u64) {
                                release_keyboard_input(key_code);
                            }
                        }
                        xinput2::XI_RawButtonPress => {
                            let raw_event = &*(cookie.data as *const xinput2::XIRawEvent);
                            record_mouse_input(linux_mouse_group_from_button_detail(
                                raw_event.detail,
                            ));
                            if let Some((cursor_ratio_x, cursor_ratio_y)) =
                                linux_query_cursor_ratio(display, root_window, screen)
                            {
                                record_cursor_ratio(cursor_ratio_x, cursor_ratio_y);
                                last_motion_query = Some(Instant::now());
                            }
                        }
                        xinput2::XI_RawMotion => {
                            record_mouse_input(MOUSE_GROUP_MOVE);
                            let should_query = last_motion_query
                                .is_none_or(|prev| prev.elapsed() >= INPUT_BRIDGE_POLL_INTERVAL);
                            if should_query {
                                if let Some((cursor_ratio_x, cursor_ratio_y)) =
                                    linux_query_cursor_ratio(display, root_window, screen)
                                {
                                    record_cursor_ratio(cursor_ratio_x, cursor_ratio_y);
                                }
                                last_motion_query = Some(Instant::now());
                            }
                        }
                        _ => {}
                    }

                    xlib::XFreeEventData(display, cookie);
                }

                xlib::XCloseDisplay(display);
                release_input_monitor_generation(my_gen);
            });
        }
        LinuxDesktopSession::Wayland => {
            let Some(provider) = linux_wayland_mouse_provider_for_environment(
                desktop_environment,
                gnome_mouse_provider_available,
                kde_mouse_provider_available,
                hyprland_mouse_provider_available,
            ) else {
                release_input_monitor_generation(my_gen);
                log::warn!("桌宠输入联动注册失败：未找到可用的 Wayland 鼠标 provider");
                return;
            };

            let app = app.clone();
            thread::spawn(move || {
                const BOUNDS_TTL: Duration = Duration::from_secs(2);
                let mut last_mouse_state: Option<(i32, i32, u8)> = None;
                let mut last_cursor_point: Option<(i32, i32)> = None;
                let mut last_keyboard_timestamp_ms: Option<u64> = None;
                let mut cached_bounds: Option<(Instant, (i32, i32, u32, u32))> = None;

                loop {
                    if INPUT_MONITOR_GEN.load(Ordering::Acquire) != my_gen {
                        break;
                    }

                    if let Some(snapshot) = query_wayland_mouse_cursor_point(provider) {
                        record_wayland_cursor_position((snapshot.x, snapshot.y));
                        let next_mouse_state = (snapshot.x, snapshot.y, snapshot.group_code);
                        if last_mouse_state != Some(next_mouse_state) {
                            record_mouse_input(snapshot.group_code);
                            last_mouse_state = Some(next_mouse_state);
                        }

                        if !AVATAR_BODY_HIDDEN_FLAG.load(Ordering::Relaxed)
                            && snapshot.keyboard_timestamp_ms != last_keyboard_timestamp_ms
                        {
                            if let Some(timestamp_ms) = snapshot.keyboard_timestamp_ms {
                                if let Some(key_code) = snapshot.keyboard_key_code {
                                    record_keyboard_input(
                                        standard_keyboard_group_from_key_code(key_code),
                                        key_code,
                                    );
                                }
                                last_keyboard_timestamp_ms = Some(timestamp_ms);
                            }
                        }

                        let point = (snapshot.x, snapshot.y);
                        if last_cursor_point != Some(point) {
                            last_cursor_point = Some(point);
                            let bounds = match cached_bounds {
                                Some((captured_at, b)) if captured_at.elapsed() < BOUNDS_TTL => {
                                    Some(b)
                                }
                                _ => {
                                    let refreshed =
                                        virtual_desktop_logical_bounds_from_monitors(&app);
                                    if let Some(b) = refreshed {
                                        cached_bounds = Some((Instant::now(), b));
                                    }
                                    refreshed
                                }
                            };
                            if let Some((min_x, min_y, width, height)) = bounds {
                                let (cursor_ratio_x, cursor_ratio_y) =
                                    cursor_ratio_from_virtual_desktop_bounds(
                                        snapshot.x, snapshot.y, min_x, min_y, width, height,
                                    );
                                record_cursor_ratio(cursor_ratio_x, cursor_ratio_y);
                            }
                        }
                    }

                    thread::sleep(WAYLAND_MOUSE_POLL_INTERVAL);
                }

                release_input_monitor_generation(my_gen);
            });
        }
        LinuxDesktopSession::Unknown => {
            release_input_monitor_generation(my_gen);
            log::warn!("桌宠输入联动注册失败：Linux 会话类型未知");
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn start_avatar_input_monitor(_app: &AppHandle) {}

/// 停止全局键鼠输入监听与桥接任务，释放平台 Hook / NSEvent Monitor / X11 Display。
pub fn stop_avatar_input_monitor(app: &AppHandle) {
    #[cfg(target_os = "windows")]
    let thread_id = {
        let _guard = INPUT_MONITOR_LIFECYCLE_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        INPUT_MONITOR_GEN.fetch_add(1, Ordering::SeqCst);
        INPUT_BRIDGE_GEN.fetch_add(1, Ordering::SeqCst);
        INPUT_MONITOR_STARTED.store(false, Ordering::SeqCst);
        WINDOWS_HOOK_THREAD_ID.swap(0, Ordering::AcqRel)
    };
    #[cfg(not(target_os = "windows"))]
    {
        let _guard = INPUT_MONITOR_LIFECYCLE_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        INPUT_MONITOR_GEN.fetch_add(1, Ordering::SeqCst);
        INPUT_BRIDGE_GEN.fetch_add(1, Ordering::SeqCst);
        INPUT_MONITOR_STARTED.store(false, Ordering::SeqCst);
    }

    clear_avatar_input_state();

    #[cfg(target_os = "windows")]
    {
        let _ = app;
        use winapi::um::winuser::{PostThreadMessageW, WM_QUIT};
        if thread_id != 0 {
            unsafe {
                PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        INPUT_MONITOR_RETRY_GEN.fetch_add(1, Ordering::SeqCst);
        let _ = app.run_on_main_thread(|| unsafe {
            remove_macos_input_monitors();
        });
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = app;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_session::{LinuxDesktopEnvironment, LinuxDesktopSession};
    use std::sync::{Mutex, MutexGuard};

    static AVATAR_INPUT_TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn reset_avatar_input_test_state() -> MutexGuard<'static, ()> {
        let guard = AVATAR_INPUT_TEST_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        LAST_KEYBOARD_INPUT_AT_MS.store(0, Ordering::Relaxed);
        LAST_MOUSE_INPUT_AT_MS.store(0, Ordering::Relaxed);
        LAST_KEYBOARD_GROUP_CODE.store(0, Ordering::Relaxed);
        LAST_KEYBOARD_KEY_CODE.store(0, Ordering::Relaxed);
        LAST_MOUSE_GROUP_CODE.store(0, Ordering::Relaxed);
        CURSOR_RATIO_X_PERMILLE.store(500, Ordering::Relaxed);
        CURSOR_RATIO_Y_PERMILLE.store(500, Ordering::Relaxed);
        CURSOR_RATIO_INITIALIZED.store(false, Ordering::Relaxed);
        KEYBOARD_HAS_PRESSED_KEYS.store(false, Ordering::Relaxed);
        #[cfg(target_os = "windows")]
        {
            WINDOWS_KEYBOARD_INPUTS.invalidate();
            let _ = WINDOWS_KEYBOARD_INPUTS.drain();
        }
        KEYBOARD_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pressed_keys
            .clear();
        guard
    }

    #[test]
    fn 键盘缓冲应按顺序保留按下与松开事件() {
        let buffer = KeyboardInputBuffer::new();
        for (key_code, pressed) in [(59, true), (0, true), (0, false)] {
            assert!(buffer.push(BufferedKeyboardInput {
                epoch: 0,
                pressed_at_ms: 1000,
                key_code,
                group_code: 0,
                pressed,
            }));
        }
        let (reset, events) = buffer.drain();
        assert!(!reset);
        assert_eq!(
            events
                .iter()
                .map(|event| (event.key_code, event.pressed))
                .collect::<Vec<_>>(),
            vec![(59, true), (0, true), (0, false)]
        );
    }

    #[test]
    fn 键盘状态重置后旧事件不应覆盖新输入() {
        let buffer = KeyboardInputBuffer::new();
        let old = BufferedKeyboardInput {
            epoch: 0,
            pressed_at_ms: 1000,
            key_code: 0,
            group_code: KEYBOARD_GROUP_KEY_A,
            pressed: true,
        };
        assert!(buffer.push(old));
        buffer.invalidate();
        assert!(!buffer.push(BufferedKeyboardInput {
            pressed: false,
            ..old
        }));
        let current = BufferedKeyboardInput {
            epoch: buffer.epoch.load(Ordering::Acquire),
            ..old
        };
        assert!(buffer.push(current));
        let (reset, events) = buffer.drain();
        assert!(reset);
        assert_eq!(events, vec![current]);
    }

    #[test]
    fn 键盘缓冲溢出应丢弃旧代次并要求重置可视状态() {
        let buffer = KeyboardInputBuffer::new();
        let input = BufferedKeyboardInput {
            epoch: 0,
            pressed_at_ms: 1000,
            key_code: 12,
            group_code: KEYBOARD_GROUP_KEY_Q,
            pressed: true,
        };
        for _ in 0..KEYBOARD_INPUT_CAPACITY {
            assert!(buffer.push(input));
        }
        assert!(!buffer.push(input));
        let (reset, events) = buffer.drain();
        assert!(reset);
        assert!(events.is_empty());
        assert!(buffer.push(BufferedKeyboardInput {
            epoch: buffer.epoch.load(Ordering::Acquire),
            ..input
        }));
        let (reset, events) = buffer.drain();
        assert!(!reset);
        assert_eq!(events.len(), 1);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows键盘采集不应等待可视状态锁且采样后应正确松键() {
        let _guard = reset_avatar_input_test_state();
        let was_enabled = AVATAR_ENABLED_FLAG.swap(true, Ordering::Relaxed);
        let was_hidden = AVATAR_BODY_HIDDEN_FLAG.swap(false, Ordering::Relaxed);
        let epoch = WINDOWS_KEYBOARD_INPUTS.epoch.load(Ordering::Acquire);
        let state_guard = KEYBOARD_STATE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let receiver_guard = WINDOWS_KEYBOARD_INPUTS
            .receiver
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            buffer_windows_keyboard_input(BufferedKeyboardInput {
                epoch,
                pressed_at_ms: 1000,
                key_code: 12,
                group_code: KEYBOARD_GROUP_KEY_Q,
                pressed: true,
            });
            sender.send(()).unwrap();
        });
        let did_not_block = receiver.recv_timeout(Duration::from_secs(1)).is_ok();
        drop(receiver_guard);
        drop(state_guard);
        worker.join().unwrap();
        sync_windows_keyboard_input();
        let pressed = build_avatar_input_payload(1000);
        buffer_windows_keyboard_input(BufferedKeyboardInput {
            epoch,
            pressed_at_ms: 0,
            key_code: 12,
            group_code: 0,
            pressed: false,
        });
        sync_windows_keyboard_input();
        let released = build_avatar_input_payload(1200);
        AVATAR_ENABLED_FLAG.store(was_enabled, Ordering::Relaxed);
        AVATAR_BODY_HIDDEN_FLAG.store(was_hidden, Ordering::Relaxed);
        assert!(
            did_not_block,
            "输入回调不能等待普通采样线程持有的按键状态锁"
        );
        assert_eq!(pressed.keyboard_visual_keys, vec!["KeyQ".to_string()]);
        assert!(!released.keyboard_active);
        assert!(released.keyboard_visual_keys.is_empty());
    }

    #[test]
    fn macos权限恢复且监听尚未启动时应触发重试() {
        assert!(!should_retry_avatar_input_monitor(true, true, true));
        assert!(!should_retry_avatar_input_monitor(false, false, true));
        assert!(!should_retry_avatar_input_monitor(false, true, false));
        assert!(should_retry_avatar_input_monitor(false, true, true));
    }

    #[test]
    fn 输入活跃窗口应只在短时间内保持有效() {
        let _guard = reset_avatar_input_test_state();
        assert!(!is_input_still_active(0, 1000, 180));
        assert!(is_input_still_active(900, 1000, 180));
        assert!(!is_input_still_active(700, 1000, 180));
    }

    #[test]
    fn 所有平台均启用智能穿透命中轮询() {
        // issue #137 诉求三：Windows 此前被禁用，导致穿透模式下通知按钮无法点击。
        // 现统一启用，鼠标进入窗口矩形（含通知按钮区域）时临时关闭穿透，按钮可点。
        assert!(smart_click_through_polling_enabled_for_platform());
    }

    #[test]
    fn 精确模式穿透只由交互区域命中决定() {
        assert!(should_ignore_avatar_cursor(true, false, Some(false)));
        assert!(!should_ignore_avatar_cursor(true, false, Some(true)));
        assert!(should_ignore_avatar_cursor(true, true, Some(false)));
        assert!(!should_ignore_avatar_cursor(true, true, Some(true)));
    }

    #[test]
    fn 精确模式坐标不可用时应保持通知可交互() {
        assert!(!should_ignore_avatar_cursor(true, false, None));
        assert!(!should_ignore_avatar_cursor(true, true, None));
    }

    #[test]
    fn 非精确模式应保持用户穿透开关语义() {
        assert!(!should_ignore_avatar_cursor(false, false, Some(false)));
        assert!(!should_ignore_avatar_cursor(false, false, Some(true)));
        assert!(should_ignore_avatar_cursor(false, true, Some(false)));
        assert!(!should_ignore_avatar_cursor(false, true, Some(true)));
        assert!(should_ignore_avatar_cursor(false, true, None));
    }

    #[test]
    fn 原生鼠标坐标应转换到目标窗口的物理缩放空间() {
        assert_eq!(
            scale_cursor_position_to_target((1800.0, -300.0), 2.0, 1.0),
            Some((900, -150))
        );
        assert_eq!(
            scale_cursor_position_to_target((900.0, 450.0), 1.0, 2.0),
            Some((1800, 900))
        );
        assert_eq!(
            scale_cursor_position_to_target((900.0, 450.0), 0.0, 2.0),
            None
        );
    }

    #[test]
    fn 非_wayland_应优先原生鼠标坐标并仅在失败时回退() {
        let native = Some((120, -80));
        let fallback = Some((500, 400));
        assert_eq!(select_cursor_position(false, native, fallback), native);
        assert_eq!(select_cursor_position(false, None, fallback), fallback);
    }

    #[test]
    fn wayland_provider逻辑坐标应直接转换到目标窗口缩放空间() {
        assert_eq!(
            logical_cursor_position_to_target((-320, 240), 1.5),
            Some((-480, 360))
        );
        assert_eq!(logical_cursor_position_to_target((20, 40), 0.0), None);
        assert_eq!(
            unpack_wayland_cursor_position(pack_wayland_cursor_position((-320, 240))),
            (-320, 240)
        );
    }

    #[test]
    fn wayland_provider过期坐标不应继续参与命中测试() {
        assert_eq!(
            fresh_wayland_cursor_position((-320, 240), 1_000, 1_500, 1.5),
            Some((-480, 360))
        );
        assert_eq!(
            fresh_wayland_cursor_position((-320, 240), 1_000, 1_501, 1.5),
            None
        );
        assert_eq!(
            fresh_wayland_cursor_position((-320, 240), 0, 1_000, 1.5),
            None
        );
        assert_eq!(
            fresh_wayland_cursor_position((-320, 240), 1_000, 999, 1.5),
            None
        );
    }

    #[test]
    fn wayland混合缩放显示器应先还原各自逻辑边界() {
        assert_eq!(
            logical_monitor_rect_from_physical(-1920, 0, 1920, 1080, 1.0),
            Some((-1920, 0, 1920, 1080))
        );
        assert_eq!(
            logical_monitor_rect_from_physical(0, 0, 3840, 2160, 2.0),
            Some((0, 0, 1920, 1080))
        );
    }

    #[test]
    fn wayland_应使用已有_provider_坐标而不是_tao_占位值() {
        let tao_placeholder = Some((0, 0));
        let provider = Some((-320, 240));
        assert_eq!(
            select_cursor_position(true, tao_placeholder, provider),
            provider
        );
    }

    #[test]
    fn 输入载荷应根据最近输入时间生成键鼠活跃状态() {
        let _guard = reset_avatar_input_test_state();
        KEYBOARD_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pressed_keys = vec![PressedKeyState {
            key_code: 12,
            pressed_at_ms: 1000,
        }];
        LAST_KEYBOARD_INPUT_AT_MS.store(1000, Ordering::Relaxed);
        LAST_MOUSE_INPUT_AT_MS.store(850, Ordering::Relaxed);
        LAST_KEYBOARD_GROUP_CODE.store(KEYBOARD_GROUP_KEY_Q, Ordering::Relaxed);
        LAST_KEYBOARD_KEY_CODE.store(12, Ordering::Relaxed);
        LAST_MOUSE_GROUP_CODE.store(MOUSE_GROUP_RIGHT, Ordering::Relaxed);
        CURSOR_RATIO_X_PERMILLE.store(250, Ordering::Relaxed);
        CURSOR_RATIO_Y_PERMILLE.store(750, Ordering::Relaxed);

        let payload = build_avatar_input_payload(1030);
        assert!(payload.keyboard_active);
        assert!(payload.mouse_active);
        assert_eq!(payload.keyboard_group, "key-q");
        assert_eq!(payload.keyboard_groups, vec!["key-q".to_string()]);
        assert_eq!(payload.keyboard_visual_key, "KeyQ");
        assert_eq!(payload.keyboard_visual_keys, vec!["KeyQ".to_string()]);
        assert_eq!(payload.mouse_group, "mouse-right");
        assert_eq!(payload.cursor_ratio_x, 0.25);
        assert_eq!(payload.cursor_ratio_y, 0.75);

        KEYBOARD_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pressed_keys
            .clear();
        let payload = build_avatar_input_payload(1200);
        assert!(!payload.keyboard_active);
        assert!(!payload.mouse_active);
    }

    #[test]
    fn 多键按住时应返回完整键集合而不是只保留最后一个键() {
        let _guard = reset_avatar_input_test_state();
        {
            let mut state = KEYBOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.pressed_keys = vec![
                PressedKeyState {
                    key_code: 13,
                    pressed_at_ms: 1950,
                },
                PressedKeyState {
                    key_code: 2,
                    pressed_at_ms: 2000,
                },
            ];
        }
        LAST_KEYBOARD_INPUT_AT_MS.store(2000, Ordering::Relaxed);
        LAST_KEYBOARD_GROUP_CODE.store(KEYBOARD_GROUP_KEY_D, Ordering::Relaxed);
        LAST_KEYBOARD_KEY_CODE.store(2, Ordering::Relaxed);

        let payload = build_avatar_input_payload(2010);
        assert!(payload.keyboard_active);
        assert_eq!(payload.keyboard_group, "key-d");
        assert_eq!(
            payload.keyboard_groups,
            vec!["key-d".to_string(), "key-w".to_string()]
        );
        assert_eq!(
            payload.keyboard_visual_keys,
            vec!["KeyD".to_string(), "KeyW".to_string()]
        );
    }

    #[test]
    fn 过旧的残留按键不应继续触发组合键显示() {
        let _guard = reset_avatar_input_test_state();
        {
            let mut state = KEYBOARD_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.pressed_keys = vec![
                PressedKeyState {
                    key_code: 59,
                    pressed_at_ms: 1000,
                },
                PressedKeyState {
                    key_code: 48,
                    pressed_at_ms: 2400,
                },
            ];
        }
        LAST_KEYBOARD_INPUT_AT_MS.store(2400, Ordering::Relaxed);
        LAST_KEYBOARD_GROUP_CODE.store(KEYBOARD_GROUP_KEY_Q, Ordering::Relaxed);
        LAST_KEYBOARD_KEY_CODE.store(48, Ordering::Relaxed);

        let payload = build_avatar_input_payload(2420);
        assert!(payload.keyboard_groups.is_empty());
        assert_eq!(payload.keyboard_group, "key-q");
        assert_eq!(payload.keyboard_visual_keys, vec!["Tab".to_string()]);
    }

    #[test]
    fn 键盘键码应映射到原版键区分组() {
        let _guard = reset_avatar_input_test_state();
        assert_eq!(
            standard_keyboard_group_from_key_code(18),
            KEYBOARD_GROUP_DIGIT_1
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(26),
            KEYBOARD_GROUP_DIGIT_7
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(12),
            KEYBOARD_GROUP_KEY_Q
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(14),
            KEYBOARD_GROUP_KEY_E
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(15),
            KEYBOARD_GROUP_KEY_R
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(49),
            KEYBOARD_GROUP_SPACE
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(0),
            KEYBOARD_GROUP_KEY_A
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(2),
            KEYBOARD_GROUP_KEY_D
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(1),
            KEYBOARD_GROUP_KEY_S
        );
        assert_eq!(
            standard_keyboard_group_from_key_code(13),
            KEYBOARD_GROUP_KEY_W
        );
        assert_eq!(standard_keyboard_group_from_key_code(123), 0);
    }

    #[test]
    fn 键盘键码应映射到源资源图层名称() {
        let _guard = reset_avatar_input_test_state();
        assert_eq!(keyboard_visual_key_from_key_code(0), "KeyA");
        assert_eq!(keyboard_visual_key_from_key_code(45), "KeyN");
        assert_eq!(keyboard_visual_key_from_key_code(31), "KeyO");
        assert_eq!(keyboard_visual_key_from_key_code(35), "KeyP");
        assert_eq!(keyboard_visual_key_from_key_code(25), "Num9");
        assert_eq!(keyboard_visual_key_from_key_code(49), "Space");
        assert_eq!(keyboard_visual_key_from_key_code(36), "Return");
        assert_eq!(keyboard_visual_key_from_key_code(56), "ShiftLeft");
        assert_eq!(keyboard_visual_key_from_key_code(62), "ControlRight");
        assert_eq!(keyboard_visual_key_from_key_code(43), "Comma");
        assert_eq!(keyboard_visual_key_from_key_code(47), "Period");
        assert_eq!(keyboard_visual_key_from_key_code(123), "LeftArrow");
        assert_eq!(keyboard_visual_key_from_key_code(124), "RightArrow");
        assert_eq!(keyboard_visual_key_from_key_code(125), "DownArrow");
        assert_eq!(keyboard_visual_key_from_key_code(126), "UpArrow");
        assert_eq!(keyboard_visual_key_from_key_code(127), "");
    }

    #[test]
    fn 鼠标分组标签应映射到原版鼠标模式名称() {
        let _guard = reset_avatar_input_test_state();
        assert_eq!(mouse_group_label(MOUSE_GROUP_MOVE), "mouse-move");
        assert_eq!(mouse_group_label(MOUSE_GROUP_LEFT), "mouse-left");
        assert_eq!(mouse_group_label(MOUSE_GROUP_RIGHT), "mouse-right");
        assert_eq!(mouse_group_label(MOUSE_GROUP_SIDE), "mouse-side");
    }

    #[test]
    fn linux_x11会话应支持桌宠输入联动() {
        let _guard = reset_avatar_input_test_state();
        use crate::linux_session::LinuxDesktopSession;

        assert!(linux_session_supports_avatar_input(
            LinuxDesktopSession::X11
        ));
        assert!(!linux_session_supports_avatar_input(
            LinuxDesktopSession::Wayland
        ));
        assert!(!linux_session_supports_avatar_input(
            LinuxDesktopSession::Unknown
        ));
    }

    #[test]
    fn linux_keysym应映射到桌宠可消费的统一键码() {
        let _guard = reset_avatar_input_test_state();
        assert_eq!(linux_keysym_to_avatar_key_code(0x0061), Some(0));
        assert_eq!(linux_keysym_to_avatar_key_code(0x0077), Some(13));
        assert_eq!(linux_keysym_to_avatar_key_code(0x0031), Some(18));
        assert_eq!(linux_keysym_to_avatar_key_code(0xFF08), Some(51));
        assert_eq!(linux_keysym_to_avatar_key_code(0xFF53), Some(124));
        assert_eq!(linux_keysym_to_avatar_key_code(0xFFEB), Some(55));
        assert_eq!(linux_keysym_to_avatar_key_code(0xFFBE), None);
    }

    #[test]
    fn linux鼠标按钮应映射到桌宠鼠标分组() {
        let _guard = reset_avatar_input_test_state();
        assert_eq!(linux_mouse_group_from_button_detail(1), MOUSE_GROUP_LEFT);
        assert_eq!(linux_mouse_group_from_button_detail(3), MOUSE_GROUP_RIGHT);
        assert_eq!(linux_mouse_group_from_button_detail(8), MOUSE_GROUP_SIDE);
        assert_eq!(linux_mouse_group_from_button_detail(99), MOUSE_GROUP_MOVE);
    }

    #[test]
    fn kde_wayland应识别为仅鼠标联动() {
        let _guard = reset_avatar_input_test_state();
        let support = linux_avatar_input_support_for_session(
            LinuxDesktopSession::Wayland,
            LinuxDesktopEnvironment::Kde,
            false,
            true,
            false,
        );

        assert_eq!(support.support_level, "mouse-only");
        assert_eq!(support.provider, "kdotool-mouselocation");
        assert!(!support.keyboard_supported);
        assert!(support.mouse_supported);
    }

    #[test]
    fn hyprland_wayland应识别为仅鼠标联动() {
        let _guard = reset_avatar_input_test_state();
        let support = linux_avatar_input_support_for_session(
            LinuxDesktopSession::Wayland,
            LinuxDesktopEnvironment::Hyprland,
            false,
            false,
            true,
        );

        assert_eq!(support.support_level, "mouse-only");
        assert_eq!(support.provider, "hyprctl-cursorpos");
        assert!(!support.keyboard_supported);
        assert!(support.mouse_supported);
    }

    #[test]
    fn gnome_wayland在缺少provider时应视为不可用() {
        let _guard = reset_avatar_input_test_state();
        let support = linux_avatar_input_support_for_session(
            LinuxDesktopSession::Wayland,
            LinuxDesktopEnvironment::Gnome,
            false,
            false,
            false,
        );

        assert_eq!(support.support_level, "none");
        assert_eq!(support.provider, "none");
        assert!(!support.keyboard_supported);
        assert!(!support.mouse_supported);
    }

    #[test]
    fn kdotool鼠标位置输出应解析为坐标() {
        let _guard = reset_avatar_input_test_state();
        assert_eq!(
            parse_kdotool_mouse_location_output("x:1489 y:812 screen:0 window:50331653"),
            Some((1489, 812))
        );
        assert_eq!(parse_kdotool_mouse_location_output("x:abc y:812"), None);
    }

    #[test]
    fn gnome_wayland在扩展可用时应识别为完整联动() {
        let _guard = reset_avatar_input_test_state();
        let support = linux_avatar_input_support_for_session(
            LinuxDesktopSession::Wayland,
            LinuxDesktopEnvironment::Gnome,
            true,
            false,
            false,
        );

        assert_eq!(support.support_level, "full");
        assert_eq!(support.provider, "gnome-shell-dbus");
        assert!(support.keyboard_supported);
        assert!(support.mouse_supported);
    }

    #[test]
    fn gnome_avatar_input_dbus输出应解析坐标鼠标分组与最近按键() {
        let _guard = reset_avatar_input_test_state();
        let output = "('{\"x\":1489,\"y\":812,\"mouseGroup\":\"mouse-right\",\"keyval\":113,\"keycode\":24,\"keyboardTimestampMs\":123456}','')";
        assert_eq!(
            parse_gnome_avatar_input_dbus_output(output),
            Some(LinuxWaylandMouseSnapshot {
                x: 1489,
                y: 812,
                group_code: MOUSE_GROUP_RIGHT,
                keyboard_key_code: Some(12),
                keyboard_timestamp_ms: Some(123456),
            })
        );
    }

    #[test]
    fn hyprctl_cursorpos输出应解析为坐标() {
        let _guard = reset_avatar_input_test_state();
        assert_eq!(
            parse_hyprctl_cursorpos_output("1489, 812"),
            Some((1489, 812))
        );
        assert_eq!(
            parse_hyprctl_cursorpos_output("1489 812"),
            Some((1489, 812))
        );
        assert_eq!(parse_hyprctl_cursorpos_output("oops"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows虚拟键应映射到桌宠可消费的统一键码() {
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x41), Some(0));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x53), Some(1));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x44), Some(2));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x57), Some(13));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x20), Some(49));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x25), Some(123));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0xBC), Some(43));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0xBE), Some(47));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0xBF), Some(44));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x2E), Some(117));
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x2C), None);
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x2D), None);
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x2F), None);
        assert_eq!(windows_virtual_key_to_avatar_key_code(0x70), None);
    }

    #[test]
    fn 空闲无变化时应允许跳过重复输入载荷构建() {
        let _guard = reset_avatar_input_test_state();
        KEYBOARD_HAS_PRESSED_KEYS.store(false, Ordering::Relaxed);
        let idle = build_avatar_input_payload(2000);
        assert!(can_skip_idle_input_payload(&idle));

        LAST_MOUSE_INPUT_AT_MS.store(2010, Ordering::Relaxed);
        assert!(!can_skip_idle_input_payload(&idle));
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn 输入监听代次切换时旧代次退出不得清除新代次状态() {
        let _guard = reset_avatar_input_test_state();
        INPUT_MONITOR_STARTED.store(false, Ordering::SeqCst);
        AVATAR_ENABLED_FLAG.store(true, Ordering::SeqCst);

        let gen1 = try_claim_input_monitor_generation().expect("首次申请应成功");
        assert!(
            try_claim_input_monitor_generation().is_none(),
            "重复启动应被去重"
        );

        // 模拟 stop 后立即重启进入新代次 gen2
        {
            let _lock = INPUT_MONITOR_LIFECYCLE_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            INPUT_MONITOR_GEN.fetch_add(1, Ordering::SeqCst);
            INPUT_MONITOR_STARTED.store(false, Ordering::SeqCst);
        }
        let gen2 = try_claim_input_monitor_generation().expect("停止后重启应获得新代次");
        assert!(gen2 > gen1);

        // 旧代次 gen1 迟到退出不得把 gen2 的 STARTED 复位为 false
        release_input_monitor_generation(gen1);
        assert!(INPUT_MONITOR_STARTED.load(Ordering::SeqCst));

        // 新代次 gen2 正常退出才复位
        release_input_monitor_generation(gen2);
        assert!(!INPUT_MONITOR_STARTED.load(Ordering::SeqCst));
        AVATAR_ENABLED_FLAG.store(false, Ordering::SeqCst);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows鼠标消息应映射到桌宠鼠标分组() {
        use winapi::um::winuser::{
            WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
        };

        assert_eq!(
            windows_mouse_group_from_message(WM_MOUSEMOVE),
            MOUSE_GROUP_MOVE
        );
        assert_eq!(
            windows_mouse_group_from_message(WM_LBUTTONDOWN),
            MOUSE_GROUP_LEFT
        );
        assert_eq!(
            windows_mouse_group_from_message(WM_RBUTTONDOWN),
            MOUSE_GROUP_RIGHT
        );
        assert_eq!(
            windows_mouse_group_from_message(WM_MBUTTONDOWN),
            MOUSE_GROUP_SIDE
        );
        assert_eq!(
            windows_mouse_group_from_message(WM_XBUTTONDOWN),
            MOUSE_GROUP_SIDE
        );
    }
}
