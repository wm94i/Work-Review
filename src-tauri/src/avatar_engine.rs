use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, Monitor, PhysicalPosition, Position, Size,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

pub const AVATAR_WINDOW_LABEL: &str = "avatar";
pub const AVATAR_STATE_EVENT: &str = "avatar-state-changed";
pub const AVATAR_BUBBLE_EVENT: &str = "avatar-bubble";
pub const AVATAR_INPUT_EVENT: &str = "avatar-input-changed";
pub const AVATAR_PRESET_DEFAULT: &str = "original-standard";

const AVATAR_SCALE_MIN: f64 = 0.4;
const AVATAR_SCALE_MAX: f64 = 1.3;
const AVATAR_SCALE_DEFAULT: f64 = 0.9;
const AVATAR_OPACITY_DEFAULT: f64 = 0.82;
const AVATAR_WINDOW_BASE_WIDTH: f64 = 276.0;
const AVATAR_WINDOW_BASE_HEIGHT: f64 = 248.0;
const AVATAR_WINDOW_EXPANDED_BASE_WIDTH: f64 = 380.0;
const AVATAR_WINDOW_EXPANDED_BASE_HEIGHT: f64 = 440.0;
#[cfg(test)]
const AVATAR_WINDOW_WIDTH: f64 = AVATAR_WINDOW_BASE_WIDTH * AVATAR_SCALE_DEFAULT;
#[cfg(test)]
const AVATAR_WINDOW_HEIGHT: f64 = AVATAR_WINDOW_BASE_HEIGHT * AVATAR_SCALE_DEFAULT;
const AVATAR_WINDOW_MARGIN: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AvatarStatePayload {
    pub mode: String,
    pub app_name: String,
    pub context_label: String,
    pub hint: String,
    pub is_idle: bool,
    pub is_generating_report: bool,
    pub avatar_opacity: f64,
    pub avatar_preset: String,
    pub avatar_persona: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AvatarInputPayload {
    pub keyboard_active: bool,
    pub mouse_active: bool,
    pub keyboard_group: String,
    #[serde(default)]
    pub keyboard_groups: Vec<String>,
    pub keyboard_visual_key: String,
    #[serde(default)]
    pub keyboard_visual_keys: Vec<String>,
    pub mouse_group: String,
    pub cursor_ratio_x: f64,
    pub cursor_ratio_y: f64,
    pub last_keyboard_input_at_ms: u64,
    pub last_mouse_input_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvatarBubblePayload {
    pub message: String,
    pub tone: String,
    #[serde(default)]
    pub persistent: bool,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub clear: bool,
}

impl AvatarBubblePayload {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            tone: "info".to_string(),
            persistent: false,
            duration_ms: Some(4200),
            clear: false,
        }
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            tone: "success".to_string(),
            persistent: false,
            duration_ms: Some(4200),
            clear: false,
        }
    }

    pub fn persistent_info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            tone: "info".to_string(),
            persistent: true,
            duration_ms: None,
            clear: false,
        }
    }

    pub fn clear() -> Self {
        Self {
            message: String::new(),
            tone: "info".to_string(),
            persistent: false,
            duration_ms: None,
            clear: true,
        }
    }
}

pub fn default_avatar_state() -> AvatarStatePayload {
    AvatarStatePayload {
        mode: "idle".to_string(),
        app_name: "Work Review".to_string(),
        context_label: "待命中".to_string(),
        hint: "准备陪你开始工作".to_string(),
        is_idle: true,
        is_generating_report: false,
        avatar_opacity: AVATAR_OPACITY_DEFAULT,
        avatar_preset: AVATAR_PRESET_DEFAULT.to_string(),
        avatar_persona: "assistant".to_string(),
    }
}

pub fn derive_avatar_state(
    app_name: &str,
    window_title: &str,
    browser_url: Option<&str>,
    is_idle: bool,
    is_generating_report: bool,
) -> AvatarStatePayload {
    derive_avatar_state_with_rules(
        &[],
        &[],
        app_name,
        window_title,
        browser_url,
        is_idle,
        is_generating_report,
    )
}

pub fn derive_avatar_state_with_rules(
    rules: &[crate::config::AppCategoryRule],
    custom_categories: &[crate::config::CustomCategory],
    app_name: &str,
    window_title: &str,
    browser_url: Option<&str>,
    is_idle: bool,
    is_generating_report: bool,
) -> AvatarStatePayload {
    let app_name = normalize_app_name(app_name);
    let title_lower = window_title.trim().to_lowercase();
    let url_lower = browser_url.unwrap_or_default().trim().to_lowercase();
    let manual_base_category =
        crate::monitor::find_category_override(rules, app_name.as_str(), custom_categories);
    let base_category = crate::monitor::categorize_app_with_rules(
        rules,
        app_name.as_str(),
        window_title,
        custom_categories,
    );
    let classification = crate::activity_classifier::classify_activity_with_base_category(
        app_name.as_str(),
        window_title,
        browser_url,
        &base_category,
    );

    if is_generating_report {
        return AvatarStatePayload {
            mode: "generating".to_string(),
            app_name,
            context_label: "生成中".to_string(),
            hint: "正在帮你整理今天的内容".to_string(),
            is_idle,
            is_generating_report: true,
            avatar_opacity: AVATAR_OPACITY_DEFAULT,
            avatar_preset: AVATAR_PRESET_DEFAULT.to_string(),
            avatar_persona: "assistant".to_string(),
        };
    }

    if is_idle {
        return AvatarStatePayload {
            mode: "idle".to_string(),
            app_name,
            context_label: "待机中".to_string(),
            hint: "先歇一会，恢复状态再继续".to_string(),
            is_idle: true,
            is_generating_report: false,
            avatar_opacity: AVATAR_OPACITY_DEFAULT,
            avatar_preset: AVATAR_PRESET_DEFAULT.to_string(),
            avatar_persona: "assistant".to_string(),
        };
    }

    if let Some(base_category) = manual_base_category.as_deref() {
        return match base_category {
            "development" => avatar_state(
                "working",
                app_name,
                "编码中",
                "先把这一段逻辑收住，再看下一处。",
            ),
            "browser" => {
                let (context_label, hint) = if contains_any(
                    &title_lower,
                    &["文档", "readme", "docs", "guide", "manual", "wiki"],
                ) || contains_any(
                    &url_lower,
                    &["/docs", "readme", "wiki", "developer.", "docs."],
                ) {
                    ("文档中", "先把文档结构看清，再动手修改。")
                } else {
                    ("阅读中", "正在吸收内容，先别打断节奏")
                };

                avatar_state("reading", app_name, context_label, hint)
            }
            "communication" => avatar_state(
                "working",
                app_name,
                "沟通中",
                "先把来回沟通收住，再继续推进。",
            ),
            "office" => avatar_state(
                "working",
                app_name,
                "办公中",
                "先把这一段事务处理完，再切换上下文。",
            ),
            "design" => avatar_state(
                "working",
                app_name,
                "创作中",
                "先把这一版想法落下来，再比较取舍。",
            ),
            "entertainment" => avatar_state(
                "slacking",
                app_name,
                "休息中",
                "放松可以，但别把节奏彻底丢掉。",
            ),
            _ => avatar_state("working", app_name, "办公中", "保持推进，先把这一段收住"),
        };
    }

    match classification.semantic_category.as_str() {
        "会议沟通" => {
            let (context_label, hint) = if contains_any(
                &title_lower,
                &["共享屏幕", "演示", "demo", "汇报", "review"],
            ) {
                ("演示中", "先把演示节奏收稳，再继续推进。")
            } else if contains_any(&title_lower, &["语音通话", "call", "huddle"]) {
                ("通话中", "先把关键结论对齐，再回到执行。")
            } else {
                ("开会中", "先把沟通结论收拢，再继续推进。")
            };

            avatar_state("meeting", app_name, context_label, hint)
        }
        "音乐音频" => {
            let (context_label, hint) = if contains_any(&title_lower, &["播客", "podcast"]) {
                ("播客中", "先把这段观点听完，再决定要不要切走。")
            } else {
                ("听歌中", "保持节奏，但别让注意力被带跑。")
            };

            avatar_state("music", app_name, context_label, hint)
        }
        "视频内容" => {
            let (context_label, hint) = if contains_any(
                &title_lower,
                &["课程", "教程", "lesson", "training", "回放"],
            ) {
                ("学习中", "先吸收这一段，再整理下一步动作。")
            } else if contains_any(&title_lower, &["直播", "live"]) {
                ("直播中", "先看清实时信息，再判断是否介入。")
            } else {
                ("视频中", "先看完这一段，再决定下一步。")
            };

            avatar_state("video", app_name, context_label, hint)
        }
        "资料阅读" => {
            let (context_label, hint) = if contains_any(
                &title_lower,
                &["文档", "readme", "docs", "guide", "manual", "wiki"],
            ) || contains_any(
                &url_lower,
                &["/docs", "readme", "wiki", "developer.", "docs."],
            ) {
                ("文档中", "先把文档结构看清，再动手修改。")
            } else {
                ("阅读中", "正在吸收内容，先别打断节奏")
            };

            avatar_state("reading", app_name, context_label, hint)
        }
        "资料调研" => avatar_state(
            "reading",
            app_name,
            "调研中",
            "先收拢信息，再决定怎么推进。",
        ),
        "休息娱乐" => avatar_state(
            "slacking",
            app_name,
            "休息中",
            "放松可以，但别把节奏彻底丢掉。",
        ),
        "即时聊天" => avatar_state(
            "working",
            app_name,
            "沟通中",
            "先把来回沟通收住，再继续推进。",
        ),
        "内容撰写" => {
            let (context_label, hint) =
                if contains_any(&title_lower, &["日报", "周报", "复盘", "总结"]) {
                    ("总结中", "先把结论写完整，再回头补细节。")
                } else if contains_any(&title_lower, &["方案", "需求", "prd"]) {
                    ("方案中", "先把核心方案收拢，再继续展开。")
                } else {
                    ("写作中", "先把这段内容写完整，再回头修。")
                };

            avatar_state("working", app_name, context_label, hint)
        }
        "任务规划" => {
            let (context_label, hint) =
                if contains_any(
                    &title_lower,
                    &["排期", "里程碑", "backlog", "sprint", "roadmap"],
                ) || contains_any(&url_lower, &["linear.app", "jira", "atlassian.net"])
                {
                    ("排期中", "先把节奏和先后顺序排清楚。")
                } else if contains_any(&title_lower, &["待办", "任务", "看板", "board"]) {
                    ("拆解中", "先把任务拆细，再进入执行。")
                } else {
                    ("规划中", "先把优先级排清楚，再进入执行。")
                };

            avatar_state("working", app_name, context_label, hint)
        }
        "编码开发" => avatar_state(
            "working",
            app_name,
            "编码中",
            "先把这一段逻辑收住，再看下一处。",
        ),
        "设计创作" => avatar_state(
            "working",
            app_name,
            "创作中",
            "先把这一版想法落下来，再比较取舍。",
        ),
        "未知活动" if classification.confidence < 60 => avatar_state(
            "working",
            app_name,
            "判断中",
            "信息还不够明确，先继续观察一下。",
        ),
        _ => avatar_state("working", app_name, "办公中", "保持推进，先把这一段收住"),
    }
}

pub fn apply_avatar_visual_settings(
    mut payload: AvatarStatePayload,
    opacity: f64,
    preset: &str,
    persona: &str,
) -> AvatarStatePayload {
    payload.avatar_opacity = opacity;
    payload.avatar_preset = if preset.trim().is_empty() {
        AVATAR_PRESET_DEFAULT.to_string()
    } else {
        preset.trim().to_string()
    };
    payload.avatar_persona = match persona.trim() {
        "companion" | "coach" => persona.trim().to_string(),
        _ => "assistant".to_string(),
    };
    payload
}

pub fn sync_avatar_window(
    app: &AppHandle,
    enabled: bool,
    scale: f64,
    saved_position: Option<(i32, i32)>,
    expanded: bool,
) -> tauri::Result<()> {
    if enabled {
        let had_existing_window = app.get_webview_window(AVATAR_WINDOW_LABEL).is_some();
        ensure_avatar_window(app, scale)?;
        if let Some(window) = app.get_webview_window(AVATAR_WINDOW_LABEL) {
            let normalized_scale = normalize_avatar_scale(scale);
            let current_position = if had_existing_window {
                window
                    .outer_position()
                    .ok()
                    .map(|position| (position.x, position.y))
            } else {
                None
            };
            let effective_position =
                remembered_avatar_position(had_existing_window, current_position, saved_position);
            let (x, y) = default_avatar_position(app, normalized_scale, effective_position);
            resize_avatar_window(&window, normalized_scale, expanded);
            let _ = window.set_always_on_top(true);
            let _ = window.set_visible_on_all_workspaces(true);
            let _ = window.set_skip_taskbar(true);
            // 仅当目标位置与当前物理位置不同时才 set_position。
            // 避免每次配置变更都触发无意义的窗口移动，进而引发 onMoved → save →
            // sync 的循环，以及 Wayland/某些 WM 下频繁移动导致的窗口闪烁（#120 飘动）。
            let needs_move = current_position != Some((x, y));
            if needs_move {
                let _ = window.set_position(Position::Physical(PhysicalPosition::new(x, y)));
            }
            let _ = window.unminimize();
            let _ = window.show();
        }
    } else if let Some(window) = app.get_webview_window(AVATAR_WINDOW_LABEL) {
        let _ = window.hide();
    }

    Ok(())
}

/// 设置桌宠窗口的鼠标穿透状态。
/// 当 click_through=true 时，鼠标点击会穿透桌宠窗口到下层应用。
pub fn set_avatar_click_through(app: &AppHandle, click_through: bool) {
    if let Some(window) = app.get_webview_window(AVATAR_WINDOW_LABEL) {
        if let Err(e) = window.set_ignore_cursor_events(click_through) {
            log::warn!("设置桌宠鼠标穿透失败: {e}");
        } else {
            log::info!(
                "桌宠鼠标穿透已{}",
                if click_through { "开启" } else { "关闭" }
            );
        }
    }
}

/// 矩形命中测试（纯函数，便于单测）。左上闭、右下开。
fn point_in_rect(cx: i64, cy: i64, x: i64, y: i64, w: i64, h: i64) -> bool {
    cx >= x && cy >= y && cx < x + w && cy < y + h
}

/// 全局鼠标是否落在桌宠窗口矩形内（物理像素）。用于智能穿透命中测试。
/// 注意：用 `outer_size()`（物理），不用 `avatar_window_size`（逻辑），避免 DPR≠1 偏移。
pub fn avatar_window_hit_by_cursor(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window(AVATAR_WINDOW_LABEL) else {
        return false;
    };
    let Ok(pos) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };
    let Some((cx, cy)) = crate::avatar_input::global_cursor_position(app) else {
        return false;
    };
    point_in_rect(
        cx,
        cy,
        pos.x as i64,
        pos.y as i64,
        size.width as i64,
        size.height as i64,
    )
}

pub fn apply_avatar_window_expansion(
    app: &AppHandle,
    scale: f64,
    expanded: bool,
) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(AVATAR_WINDOW_LABEL) {
        let normalized_scale = normalize_avatar_scale(scale);
        resize_avatar_window(&window, normalized_scale, expanded);
        clamp_window_within_current_monitor(&window, normalized_scale, expanded);
    }
    Ok(())
}

fn clamp_window_within_current_monitor(window: &WebviewWindow, scale: f64, expanded: bool) {
    let monitor = match window.current_monitor() {
        Ok(Some(monitor)) => monitor,
        _ => return,
    };
    let current = match window.outer_position() {
        Ok(position) => position,
        Err(_) => return,
    };
    let work_area = monitor.work_area();
    let bounds = Rect {
        x: work_area.position.x,
        y: work_area.position.y,
        width: work_area.size.width as i32,
        height: work_area.size.height as i32,
    };
    let (window_width, window_height) = avatar_window_size(scale, expanded);
    let (clamped_x, clamped_y) =
        clamp_avatar_position_with_size(bounds, current.x, current.y, window_width, window_height);
    if (clamped_x, clamped_y) != (current.x, current.y) {
        let _ = window.set_position(Position::Physical(PhysicalPosition::new(
            clamped_x, clamped_y,
        )));
    }
}

fn remembered_avatar_position(
    had_existing_window: bool,
    current_position: Option<(i32, i32)>,
    saved_position: Option<(i32, i32)>,
) -> Option<(i32, i32)> {
    if had_existing_window {
        current_position.or(saved_position)
    } else {
        saved_position
    }
}

pub fn emit_avatar_state(app: &AppHandle, payload: &AvatarStatePayload) {
    let _ = app.emit_to(AVATAR_WINDOW_LABEL, AVATAR_STATE_EVENT, payload);
}

pub fn emit_avatar_bubble(app: &AppHandle, payload: &AvatarBubblePayload) {
    let _ = app.emit_to(AVATAR_WINDOW_LABEL, AVATAR_BUBBLE_EVENT, payload);
}

pub fn emit_avatar_input(app: &AppHandle, payload: &AvatarInputPayload) {
    let _ = app.emit_to(AVATAR_WINDOW_LABEL, AVATAR_INPUT_EVENT, payload);
}

fn ensure_avatar_window(app: &AppHandle, scale: f64) -> tauri::Result<()> {
    if app.get_webview_window(AVATAR_WINDOW_LABEL).is_some() {
        return Ok(());
    }

    let (window_width, window_height) = avatar_window_size(scale, false);

    let window = WebviewWindowBuilder::new(app, AVATAR_WINDOW_LABEL, WebviewUrl::default())
        .title("Work Review Avatar")
        .inner_size(window_width, window_height)
        .min_inner_size(window_width, window_height)
        .max_inner_size(window_width, window_height)
        .position(40.0, 40.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .decorations(false)
        .transparent(true)
        .visible(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .shadow(false)
        .focused(false)
        .build()?;

    let _ = window.set_always_on_top(true);
    let _ = window.set_visible_on_all_workspaces(true);
    let _ = window.set_skip_taskbar(true);

    Ok(())
}

fn resize_avatar_window(window: &WebviewWindow, scale: f64, expanded: bool) {
    let (window_width, window_height) = avatar_window_size(scale, expanded);
    let _ = window.set_size(Size::Logical(LogicalSize::new(window_width, window_height)));
    let _ = window.set_min_size(Some(Size::Logical(LogicalSize::new(
        window_width,
        window_height,
    ))));
    let _ = window.set_max_size(Some(Size::Logical(LogicalSize::new(
        window_width,
        window_height,
    ))));
}

fn default_avatar_position(
    app: &AppHandle,
    scale: f64,
    saved_position: Option<(i32, i32)>,
) -> (i32, i32) {
    // 多屏适配：已保存位置时，优先用该位置所在的显示器来 clamp，
    // 避免副屏坐标被主屏的工作区边界强行拉回（issue #120）。
    if let Some((saved_x, saved_y)) = saved_position {
        if let Some(bounds) = monitor_bounds_for_point(app, saved_x, saved_y) {
            return resolve_avatar_position(bounds, None, saved_position, scale);
        }
    }

    if let Some(main_window) = app.get_webview_window("main") {
        if let Ok(Some(monitor)) = main_window.current_monitor() {
            let anchor = match (main_window.outer_position(), main_window.outer_size()) {
                (Ok(position), Ok(size)) => Some(Rect {
                    x: position.x,
                    y: position.y,
                    width: size.width as i32,
                    height: size.height as i32,
                }),
                _ => None,
            };

            return compute_avatar_position(monitor, anchor, saved_position, scale);
        }
    }

    if let Ok(Some(monitor)) = app.primary_monitor() {
        return compute_avatar_position(monitor, None, saved_position, scale);
    }

    saved_position.unwrap_or((40, 40))
}

/// 在所有可用显示器中，找到包含 (x, y) 的那个，返回其工作区物理像素 bounds。
/// 用于多屏场景下根据桌宠保存位置定位正确的显示器。
fn monitor_bounds_for_point(app: &AppHandle, x: i32, y: i32) -> Option<Rect> {
    let monitors = app.available_monitors().ok()?;
    for monitor in monitors {
        let position = monitor.position();
        let size = monitor.size();
        let bounds = Rect {
            x: position.x,
            y: position.y,
            width: size.width as i32,
            height: size.height as i32,
        };
        if point_in_rect(x as i64, y as i64, bounds.x as i64, bounds.y as i64, bounds.width as i64, bounds.height as i64) {
            let work_area = monitor.work_area();
            return Some(Rect {
                x: work_area.position.x,
                y: work_area.position.y,
                width: work_area.size.width as i32,
                height: work_area.size.height as i32,
            });
        }
    }
    None
}

fn compute_avatar_position(
    monitor: Monitor,
    anchor: Option<Rect>,
    saved_position: Option<(i32, i32)>,
    scale: f64,
) -> (i32, i32) {
    let work_area = monitor.work_area();
    let bounds = Rect {
        x: work_area.position.x,
        y: work_area.position.y,
        width: work_area.size.width as i32,
        height: work_area.size.height as i32,
    };

    resolve_avatar_position(bounds, anchor, saved_position, scale)
}

fn resolve_avatar_position(
    bounds: Rect,
    anchor: Option<Rect>,
    saved_position: Option<(i32, i32)>,
    scale: f64,
) -> (i32, i32) {
    let (window_width, window_height) = avatar_window_size(scale, false);

    if let Some((saved_x, saved_y)) = saved_position {
        return clamp_avatar_position_with_size(
            bounds,
            saved_x,
            saved_y,
            window_width,
            window_height,
        );
    }

    let preferred_x = anchor
        .map(|rect| rect.x + rect.width - window_width as i32 - AVATAR_WINDOW_MARGIN as i32)
        .unwrap_or(bounds.x + bounds.width - window_width as i32 - AVATAR_WINDOW_MARGIN as i32);
    let preferred_y = anchor
        .map(|rect| rect.y + rect.height - window_height as i32 - AVATAR_WINDOW_MARGIN as i32)
        .unwrap_or(bounds.y + bounds.height - window_height as i32 - AVATAR_WINDOW_MARGIN as i32);

    clamp_avatar_position_with_size(
        bounds,
        preferred_x,
        preferred_y,
        window_width,
        window_height,
    )
}

#[cfg(test)]
fn clamp_avatar_position(bounds: Rect, preferred_x: i32, preferred_y: i32) -> (i32, i32) {
    clamp_avatar_position_with_size(
        bounds,
        preferred_x,
        preferred_y,
        AVATAR_WINDOW_WIDTH,
        AVATAR_WINDOW_HEIGHT,
    )
}

fn clamp_avatar_position_with_size(
    bounds: Rect,
    preferred_x: i32,
    preferred_y: i32,
    window_width: f64,
    window_height: f64,
) -> (i32, i32) {
    let max_x = (bounds.x + bounds.width - window_width as i32).max(bounds.x);
    let max_y = (bounds.y + bounds.height - window_height as i32).max(bounds.y);

    (
        preferred_x.clamp(bounds.x, max_x),
        preferred_y.clamp(bounds.y, max_y),
    )
}

fn avatar_window_size(scale: f64, expanded: bool) -> (f64, f64) {
    let normalized_scale = normalize_avatar_scale(scale);
    let (base_width, base_height) = if expanded {
        (
            AVATAR_WINDOW_EXPANDED_BASE_WIDTH,
            AVATAR_WINDOW_EXPANDED_BASE_HEIGHT,
        )
    } else {
        (AVATAR_WINDOW_BASE_WIDTH, AVATAR_WINDOW_BASE_HEIGHT)
    };
    (
        ((base_width * normalized_scale) * 10.0).round() / 10.0,
        ((base_height * normalized_scale) * 10.0).round() / 10.0,
    )
}

fn normalize_avatar_scale(scale: f64) -> f64 {
    if !scale.is_finite() {
        return AVATAR_SCALE_DEFAULT;
    }

    scale.clamp(AVATAR_SCALE_MIN, AVATAR_SCALE_MAX)
}

fn normalize_app_name(app_name: &str) -> String {
    let trimmed = app_name.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        "当前任务".to_string()
    } else {
        trimmed.to_string()
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn avatar_state(
    mode: &str,
    app_name: String,
    context_label: &str,
    hint: &str,
) -> AvatarStatePayload {
    AvatarStatePayload {
        mode: mode.to_string(),
        app_name,
        context_label: context_label.to_string(),
        hint: hint.to_string(),
        is_idle: false,
        is_generating_report: false,
        avatar_opacity: AVATAR_OPACITY_DEFAULT,
        avatar_preset: AVATAR_PRESET_DEFAULT.to_string(),
        avatar_persona: "assistant".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        avatar_window_size, clamp_avatar_position, clamp_avatar_position_with_size,
        default_avatar_state, derive_avatar_state, derive_avatar_state_with_rules, point_in_rect,
        remembered_avatar_position, resolve_avatar_position, Rect, AVATAR_WINDOW_HEIGHT,
        AVATAR_WINDOW_WIDTH,
    };
    use crate::config::AppCategoryRule;

    #[test]
    fn point_in_rect_边界判定() {
        // 左上闭、右下开
        assert!(point_in_rect(5, 5, 0, 0, 10, 10), "内部应命中");
        assert!(point_in_rect(0, 0, 0, 0, 10, 10), "左上角应命中（闭）");
        assert!(!point_in_rect(10, 10, 0, 0, 10, 10), "右下角不应命中（开）");
        assert!(!point_in_rect(-1, 5, 0, 0, 10, 10), "左侧外不应命中");
        assert!(!point_in_rect(5, 10, 0, 0, 10, 10), "下边外不应命中");
        assert!(point_in_rect(-5, -5, -10, -10, 8, 8), "负坐标内部应命中");
        assert!(!point_in_rect(0, 0, 0, 0, 0, 0), "零尺寸矩形不应命中");
    }

    #[test]
    fn 空闲状态应优先进入待机模式() {
        let state = derive_avatar_state("VS Code", "main.rs", None, true, false);

        assert_eq!(state.mode, "idle");
        assert!(state.is_idle);
        assert_eq!(state.context_label, "待机中");
    }

    #[test]
    fn 日报生成应优先进入生成模式() {
        let state = derive_avatar_state("Google Chrome", "日报", None, false, true);

        assert_eq!(state.mode, "generating");
        assert!(state.is_generating_report);
        assert_eq!(state.context_label, "生成中");
    }

    #[test]
    fn 浏览器上下文应识别为阅读模式() {
        let state = derive_avatar_state("Google Chrome", "产品文档 - docs", None, false, false);

        assert_eq!(state.mode, "reading");
        assert_eq!(state.context_label, "文档中");
    }

    #[test]
    fn 会议应用应识别为开会状态() {
        let state = derive_avatar_state("Zoom", "项目例会", None, false, false);

        assert_eq!(state.mode, "meeting");
        assert_eq!(state.context_label, "开会中");
    }

    #[test]
    fn 普通沟通工具不应直接识别为开会状态() {
        let state = derive_avatar_state("Slack", "设计评审结论整理", None, false, false);

        assert_ne!(state.mode, "meeting");
        assert_eq!(state.mode, "working");
        assert_eq!(state.context_label, "沟通中");
    }

    #[test]
    fn 音乐应用应识别为音乐状态() {
        let state = derive_avatar_state("Spotify", "Daily Mix", None, false, false);

        assert_eq!(state.mode, "music");
        assert_eq!(state.context_label, "听歌中");
    }

    #[test]
    fn 视频应用应识别为视频状态() {
        let state = derive_avatar_state("Bilibili", "RustConf 回放", None, false, false);

        assert_eq!(state.mode, "video");
        assert_eq!(state.context_label, "学习中");
    }

    #[test]
    fn 娱乐场景应识别为摸鱼状态() {
        let state = derive_avatar_state("Steam", "Balatro", None, false, false);

        assert_eq!(state.mode, "slacking");
        assert_eq!(state.context_label, "休息中");
    }

    #[test]
    fn 非阅读上下文应识别为工作模式() {
        let state = derive_avatar_state("Cursor", "commands.rs", None, false, false);

        assert_eq!(state.mode, "working");
        assert_eq!(state.context_label, "编码中");
    }

    #[test]
    fn github_拉取请求页面应识别为编码中() {
        let state = derive_avatar_state(
            "Google Chrome",
            "Fix updater retry · Pull Request #12 · wm94i/Work_Review",
            Some("https://github.com/wm94i/Work_Review/pull/12"),
            false,
            false,
        );

        assert_eq!(state.mode, "working");
        assert_eq!(state.context_label, "编码中");
    }

    #[test]
    fn 浏览器任务看板应识别为排期中() {
        let state = derive_avatar_state(
            "Google Chrome",
            "Sprint 15 Board - Linear",
            Some("https://linear.app/work-review/board"),
            false,
            false,
        );

        assert_eq!(state.mode, "working");
        assert_eq!(state.context_label, "排期中");
    }

    #[test]
    fn 共享屏幕会议应识别为演示中() {
        let state = derive_avatar_state("Zoom", "项目评审 - 共享屏幕", None, false, false);

        assert_eq!(state.mode, "meeting");
        assert_eq!(state.context_label, "演示中");
    }

    #[test]
    fn 教程视频应识别为学习中() {
        let state = derive_avatar_state("Bilibili", "Tauri 教程 第三课", None, false, false);

        assert_eq!(state.mode, "video");
        assert_eq!(state.context_label, "学习中");
    }

    #[test]
    fn 播客内容应识别为播客中() {
        let state = derive_avatar_state("Spotify", "AI Podcast 第 42 期", None, false, false);

        assert_eq!(state.mode, "music");
        assert_eq!(state.context_label, "播客中");
    }

    #[test]
    fn 低证据活动应在桌宠层回退为判断中() {
        let state = derive_avatar_state("UnknownApp", "首页", None, false, false);

        assert_eq!(state.mode, "working");
        assert_eq!(state.context_label, "判断中");
    }

    #[test]
    fn 手动分类规则应影响桌宠动作判断() {
        let state = derive_avatar_state_with_rules(
            &[AppCategoryRule {
                app_name: "Steam".to_string(),
                category: "design".to_string(),
            }],
            &[],
            "Steam",
            "首页",
            None,
            false,
            false,
        );

        assert_eq!(state.mode, "working");
        assert_eq!(state.context_label, "创作中");
    }

    #[test]
    fn 默认状态应可直接用于初始化() {
        let state = default_avatar_state();

        assert_eq!(state.mode, "idle");
        assert_eq!(state.app_name, "Work Review");
        assert!(!state.is_generating_report);
    }

    #[test]
    fn 桌宠窗口尺寸应随缩放变化() {
        let (small_w, small_h) = avatar_window_size(0.7, false);
        let (default_w, default_h) = avatar_window_size(0.9, false);
        let (large_w, large_h) = avatar_window_size(1.3, false);

        assert!(small_w < default_w);
        assert!(small_h < default_h);
        assert!(large_w > default_w);
        assert!(large_h > default_h);
        assert_eq!((default_w, default_h), (248.4, 223.2));
    }

    #[test]
    fn 桌宠窗口在展开模式下应比紧凑模式更宽更高() {
        let (compact_w, compact_h) = avatar_window_size(0.9, false);
        let (expanded_w, expanded_h) = avatar_window_size(0.9, true);

        assert!(expanded_w > compact_w);
        assert!(expanded_h > compact_h);
        assert_eq!((expanded_w, expanded_h), (342.0, 396.0));
    }

    #[test]
    fn 桌宠位置应优先吸附到锚点右下侧() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 1440,
            height: 900,
        };

        let (x, y) = clamp_avatar_position(bounds, 716, 356);

        assert_eq!((x, y), (716, 356));
    }

    #[test]
    fn 桌宠位置超出屏幕时应被钳制回可视区域() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        };

        let (x, y) = clamp_avatar_position(bounds, 1200, 600);

        assert_eq!(
            (x, y),
            (
                1280 - AVATAR_WINDOW_WIDTH as i32,
                720 - AVATAR_WINDOW_HEIGHT as i32
            )
        );
    }

    #[test]
    fn 已保存桌宠位置应优先于默认吸附位置() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 1440,
            height: 900,
        };
        let anchor = Rect {
            x: 100,
            y: 100,
            width: 900,
            height: 600,
        };

        let (x, y) = resolve_avatar_position(bounds, Some(anchor), Some((120, 240)), 0.9);

        assert_eq!((x, y), (120, 240));
    }

    #[test]
    fn 已保存桌宠位置超出范围时应回到可视区域() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        };

        let (x, y) = resolve_avatar_position(bounds, None, Some((1600, 900)), 0.9);

        assert_eq!(
            (x, y),
            (
                1280 - AVATAR_WINDOW_WIDTH as i32,
                720 - AVATAR_WINDOW_HEIGHT as i32
            )
        );
    }

    #[test]
    fn 首次创建桌宠窗口时不应误用占位坐标() {
        let position = remembered_avatar_position(false, Some((40, 40)), None);

        assert_eq!(position, None);
    }

    #[test]
    fn 已存在桌宠窗口时应优先保持当前位置() {
        let position = remembered_avatar_position(true, Some((640, 520)), Some((120, 240)));

        assert_eq!(position, Some((640, 520)));
    }

    #[test]
    fn 桌宠展开时应按新尺寸将贴边位置钳制回可视区域() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        };
        let (compact_w, compact_h) = avatar_window_size(0.9, false);
        let compact_x = 1280 - compact_w as i32;
        let compact_y = 720 - compact_h as i32;

        let (expanded_w, expanded_h) = avatar_window_size(0.9, true);
        let (x, y) =
            clamp_avatar_position_with_size(bounds, compact_x, compact_y, expanded_w, expanded_h);

        assert_eq!((x, y), (1280 - expanded_w as i32, 720 - expanded_h as i32));
        assert!(x < compact_x);
        assert!(y < compact_y);
    }

    #[test]
    fn 桌宠收起时不应移动仍在可视区域内的位置() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        };
        let (compact_w, compact_h) = avatar_window_size(0.9, false);

        let (x, y) = clamp_avatar_position_with_size(bounds, 120, 200, compact_w, compact_h);

        assert_eq!((x, y), (120, 200));
    }

    /// 多屏场景：主屏 1440x900 在原点，副屏在 (1440, 0)，尺寸 1920x1080。
    /// 验证保存的副屏坐标在用副屏 bounds clamp 时，仍落在副屏内（不被拉回主屏）。
    #[test]
    fn 多屏下副屏坐标应保留在副屏工作区内() {
        let secondary_bounds = Rect {
            x: 1440,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let saved = (2200, 800);
        let (compact_w, compact_h) = avatar_window_size(0.9, false);

        let (x, y) =
            clamp_avatar_position_with_size(secondary_bounds, saved.0, saved.1, compact_w, compact_h);

        // 仍在副屏内（x >= 1440），且未被错误地拉回主屏原点附近
        assert!(
            x >= secondary_bounds.x,
            "副屏坐标 x={x} 不应被钳制到主屏（x<{}）",
            secondary_bounds.x
        );
        assert_eq!((x, y), saved);
    }

    /// 多屏场景：副屏坐标用主屏 bounds clamp 时会越界 —— 这正是 #120 的 bug 行为，
    /// 用于回归验证修复后我们改用副屏 bounds 的必要性。
    #[test]
    fn 多屏下副屏坐标若误用主屏bounds会被错误钳制() {
        let primary_bounds = Rect {
            x: 0,
            y: 0,
            width: 1440,
            height: 900,
        };
        let (compact_w, compact_h) = avatar_window_size(0.9, false);
        let saved = (2200, 800);

        let (x, y) =
            clamp_avatar_position_with_size(primary_bounds, saved.0, saved.1, compact_w, compact_h);

        // 用主屏 bounds 时副屏 x=2200 会被钳到主屏右边缘 —— 这是要避免的行为
        assert_eq!(x, primary_bounds.x + primary_bounds.width - compact_w as i32);
        assert_ne!((x, y), saved);
    }

    /// 多屏场景：保存坐标的副屏被拔掉（不在任何 monitor 上）时，
    /// resolve_avatar_position 应回退到传入 monitor 的 bounds，仍给出合法可视位置。
    #[test]
    fn 已保存坐标超出所有屏幕时应回退到可用显示器可视区域() {
        let fallback_bounds = Rect {
            x: 0,
            y: 0,
            width: 1440,
            height: 900,
        };
        let orphan_saved = (5000, 5000);

        let (x, y) = resolve_avatar_position(fallback_bounds, None, Some(orphan_saved), 0.9);

        let (w, h) = avatar_window_size(0.9, false);
        assert_eq!((x, y), (fallback_bounds.x + fallback_bounds.width - w as i32, fallback_bounds.y + fallback_bounds.height - h as i32));
    }
}
