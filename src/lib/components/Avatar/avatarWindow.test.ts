import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import {
  normalizeAvatarInputActivity,
  parseAvatarBubblePayload,
  parseAvatarFollowupPayload,
  parseAvatarState,
} from '../../../routes/avatar/avatarPayload.ts';

function readCommandsSource() {
  // commands.rs 已按领域拆分为 commands/*.rs，这里拼接所有子模块以保持断言语义不变。
  const dir = new URL('../../../../src-tauri/src/commands/', import.meta.url);
  const files = readdirSync(dir).filter((f) => f.endsWith('.rs'));
  return files.map((f) => readFileSync(new URL(f, dir), 'utf8')).join('\n');
}

test('桌宠状态载荷应校验必需字段并兼容旧版可选外观字段', () => {
  const legacyState = {
    mode: 'working',
    appName: 'Code',
    contextLabel: '编码中',
    hint: '继续当前任务',
    isIdle: false,
    isGeneratingReport: false,
    avatarOpacity: 0.82,
  };

  assert.deepEqual(parseAvatarState(legacyState), {
    ...legacyState,
    avatarPreset: 'original-standard',
    avatarPersona: 'assistant',
    avatarBodyHidden: false,
  });
  assert.equal(parseAvatarState(null), null);
  assert.equal(parseAvatarState({ ...legacyState, mode: 'unsupported' }), null);
});

test('桌宠输入载荷应在未知或字段漂移时回落到安全默认值', () => {
  assert.deepEqual(normalizeAvatarInputActivity(null), {
    keyboardActive: false,
    mouseActive: false,
    keyboardGroup: 'idle',
    keyboardVisualKey: '',
    mouseGroup: 'idle',
    cursorRatioX: 0.5,
    cursorRatioY: 0.5,
    lastKeyboardInputAtMs: 0,
    lastMouseInputAtMs: 0,
  });

  assert.deepEqual(normalizeAvatarInputActivity({
    keyboardActive: true,
    cursorRatioX: 0.25,
    cursorRatioY: '0.75',
  }), {
    keyboardActive: true,
    mouseActive: false,
    keyboardGroup: 'idle',
    keyboardVisualKey: '',
    mouseGroup: 'idle',
    cursorRatioX: 0.25,
    cursorRatioY: 0.5,
    lastKeyboardInputAtMs: 0,
    lastMouseInputAtMs: 0,
  });
});

test('桌宠气泡和跟进建议应拒绝无法安全渲染的载荷', () => {
  assert.deepEqual(parseAvatarBubblePayload({ message: '继续推进' }), {
    message: '继续推进',
    tone: undefined,
    persistent: undefined,
    durationMs: undefined,
    clear: undefined,
  });
  assert.equal(parseAvatarBubblePayload({ message: 1 }), null);

  const followup = {
    projectKey: 'work-review',
    date: '2026-08-10',
    title: '继续迁移',
    sourceApp: 'Code',
    sourceTitle: 'avatarPayload.ts',
    intentLabel: '继续任务',
    confidence: 92,
    sessionAgeHours: 2,
  };
  assert.deepEqual(parseAvatarFollowupPayload(followup), {
    ...followup,
    persona: 'assistant',
  });
  assert.equal(parseAvatarFollowupPayload({ ...followup, confidence: '92' }), null);
});

test('桌宠窗口应强制网页根节点透明，避免轮廓外出现白底', () => {
  const source = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');

  assert.match(source, /:global\(:root\)/);
  assert.match(source, /:global\(html\)/);
  assert.match(source, /:global\(body\)/);
  assert.match(source, /background:\s*transparent !important/);
});

test('设置页应在顶部单独展示权限区，并将背景外观放入独立外观页', () => {
  const settingsSource = readFileSync(new URL('../../../routes/settings/Settings.svelte', import.meta.url), 'utf8');
  const avatarSource = readFileSync(new URL('../../../routes/settings/components/SettingsAvatar.svelte', import.meta.url), 'utf8');
  const generalSource = readFileSync(new URL('../../../routes/settings/components/SettingsGeneral.svelte', import.meta.url), 'utf8');
  const systemSource = readFileSync(new URL('../../../routes/settings/components/SettingsSystem.svelte', import.meta.url), 'utf8');
  const appearanceSource = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');

  assert.match(settingsSource, /settings\.tabs\.avatar/);
  assert.match(settingsSource, /settings\.tabs\.appearance/);
  assert.match(settingsSource, /import SettingsAvatar/);
  assert.match(settingsSource, /import SettingsAppearance/);
  assert.match(settingsSource, /import SettingsSystem/);
  assert.match(settingsSource, /invoke(?:<[^>]+>)?\('get_runtime_platform'\)/);
  assert.match(settingsSource, /activeTab === 'avatar'/);
  assert.match(settingsSource, /activeTab === 'appearance'/);
  assert.match(settingsSource, /<SettingsAvatar bind:config on:change=\{\(\) => dirty = true\} \/>/);
  assert.match(settingsSource, /<SettingsAppearance bind:config mode="background-only" on:change=\{handleSettingsChange\} \/>/);
  assert.match(settingsSource, /\{#if settingsRuntimePlatform === 'macos'\}/);
  assert.match(settingsSource, /<SettingsSystem \/>/);
  assert.match(settingsSource, /settings-top-status-zone/);
  assert.doesNotMatch(generalSource, /import SettingsAppearance/);
  assert.doesNotMatch(generalSource, /<SettingsAppearance/);
  assert.doesNotMatch(generalSource, /import SettingsSystem/);
  assert.doesNotMatch(generalSource, /<SettingsSystem \/>/);
  assert.match(avatarSource, /mode="avatar-only"/);
  assert.match(systemSource, /settingsGeneral\.permissionsTitle/);
  assert.match(appearanceSource, /export let mode(?:\s*:\s*AppearanceMode)? = 'full'/);
  assert.match(appearanceSource, /showAvatarControls = mode === 'full' \|\| mode === 'avatar-only'/);
  assert.match(appearanceSource, /showBackgroundSettings = mode === 'full' \|\| mode === 'background-only'/);
  assert.doesNotMatch(appearanceSource, /showSystemStatus/);
});

test('设置页应提供桌宠连续缩放滑杆', () => {
  const source = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');

  assert.match(source, /avatar_scale/);
  assert.match(source, /type="range"/);
  assert.match(source, /min="0\.4"/);
  assert.match(source, /max="1\.3"/);
  assert.match(source, /step="0\.05"/);
});

test('桌面化身 Beta 应展示在外层标签栏，设置内容卡内不再渲染硬编码 Beta 徽标', () => {
  const settingsSource = readFileSync(new URL('../../../routes/settings/Settings.svelte', import.meta.url), 'utf8');
  const appearanceSource = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');

  assert.match(settingsSource, /id:\s*'avatar'[\s\S]*beta:\s*true/);
  assert.match(appearanceSource, /settingsAppearance\.avatar/);
  assert.match(appearanceSource, /settingsAppearance\.avatarBetaHint/);
  // 桌面化身区用 avatarBetaHint 文案、不应硬编码 Beta 徽标；
  // 界面风格区的 Beta 徽标是独立功能，不受此约束。
  const avatarBlock =
    appearanceSource.match(/\{#if showAvatarControls\}[\s\S]*?\{\/if\}/)?.[0] ?? appearanceSource;
  assert.doesNotMatch(avatarBlock, />\s*Beta\s*</);
});

test('桌宠控制项应迁移到外观页独立区域，并提供猫体透明度滑杆', () => {
  const appearanceSource = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');
  const generalSource = readFileSync(new URL('../../../routes/settings/components/SettingsGeneral.svelte', import.meta.url), 'utf8');

  assert.match(appearanceSource, /settingsAppearance\.avatarOpacity/);
  assert.match(appearanceSource, /settingsAppearance\.avatarOpacityHint/);
  assert.match(appearanceSource, /avatar_opacity/);
  assert.match(appearanceSource, /min="0\.45"/);
  assert.match(appearanceSource, /max="1"/);
  assert.match(appearanceSource, /settingsAppearance\.avatarOpacityAria/);
  assert.doesNotMatch(generalSource, /settingsAppearance\.avatarOpacity/);
  assert.doesNotMatch(generalSource, /settingsAppearance\.avatar/);
});

test('常规设置页应提供关闭主界面后释放 Webview 的轻量模式开关', () => {
  const source = readFileSync(new URL('../../../routes/settings/components/SettingsGeneral.svelte', import.meta.url), 'utf8');

  assert.match(source, /settingsGeneral\.lightweightMode/);
  assert.match(source, /settingsGeneral\.lightweightModeDescription/);
  assert.match(source, /config\.lightweight_mode/);
});

test('开启开机自启动后应出现主界面启动模式二级选项', () => {
  const source = readFileSync(new URL('../../../routes/settings/components/SettingsGeneral.svelte', import.meta.url), 'utf8');
  const i18nSource = readFileSync(new URL('../../../lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8');

  assert.match(source, /\{#if autoStartEnabled\}/);
  assert.match(source, /config\.auto_start_silent/);
  assert.match(source, /settingsGeneral\.autoStartLaunchMode/);
  assert.match(source, /settingsGeneral\.autoStartLaunchShow/);
  assert.match(source, /settingsGeneral\.autoStartLaunchSilent/);
  assert.match(i18nSource, /autoStartLaunchMode:\s*'启动后显示'/);
  assert.match(i18nSource, /autoStartLaunchSilent:\s*'启动时静默驻留'/);
});

test('休息提醒应放在桌宠外观设置中，并依赖桌面化身开关', () => {
  const source = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');
  const generalSource = readFileSync(new URL('../../../routes/settings/components/SettingsGeneral.svelte', import.meta.url), 'utf8');
  const i18nSource = readFileSync(new URL('../../../lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8');

  assert.match(source, /config\.break_reminder_enabled/);
  assert.match(source, /config\.break_reminder_interval_minutes/);
  assert.match(source, /settingsAppearance\.breakReminder/);
  assert.match(source, /settingsAppearance\.breakReminderDescription/);
  assert.match(source, /settingsAppearance\.breakReminderInterval/);
  assert.match(source, /disabled=\{!config\.avatar_enabled\}/);
  assert.match(source, /settingsAppearance\.breakReminderRequiresAvatar/);
  assert.match(source, /\{#if config\.break_reminder_enabled\}/);
  assert.doesNotMatch(generalSource, /break_reminder_enabled/);
  assert.match(i18nSource, /settingsAppearance:\s*\{/);
  assert.match(i18nSource, /breakReminderInterval:\s*'提醒间隔'/);
});

test('桌宠窗口重新同步时应优先保持当前窗口位置，避免尺寸调整后跳回默认点位', () => {
  const source = readFileSync(new URL('../../../../src-tauri/src/avatar_engine.rs', import.meta.url), 'utf8');

  assert.match(source, /had_existing_window/);
  assert.match(source, /remembered_avatar_position/);
  assert.match(source, /if had_existing_window/);
  assert.match(source, /else \{\s*saved_position/s);
});

test('桌宠窗口在不可见时应暂停动作节拍，重新可见后再恢复', () => {
  const source = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');

  assert.match(source, /document\.addEventListener\('visibilitychange'/);
  assert.match(
    source,
    /if\s*\(document\.hidden\)[\s\S]*clearTimeout\(motionTimer\)[\s\S]*motionTimer\s*=\s*null/
  );
  assert.match(
    source,
    /else\s*\{[\s\S]*scheduleNextMotionStep\(\)/
  );
  assert.match(source, /document\.removeEventListener\('visibilitychange'/);
});

test('桌宠窗口应监听原生输入事件，并将键鼠活跃状态喂给桌宠渲染层', () => {
  const windowSource = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');
  const engineSource = readFileSync(new URL('../../../../src-tauri/src/avatar_engine.rs', import.meta.url), 'utf8');
  const mainSource = readFileSync(new URL('../../../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');

  assert.match(windowSource, /avatar-input-changed/);
  assert.match(windowSource, /inputActivity/);
  assert.match(windowSource, /keyboardActive/);
  assert.match(windowSource, /mouseActive/);
  assert.match(windowSource, /keyboardGroup/);
  assert.match(windowSource, /cursorRatioX/);
  assert.match(windowSource, /cursorRatioY/);
  assert.match(windowSource, /lastKeyboardInputAtMs/);
  assert.match(windowSource, /lastMouseInputAtMs/);
  assert.match(windowSource, /<AvatarCanvas[\s\S]*\{inputActivity\}/);
  assert.match(engineSource, /AVATAR_INPUT_EVENT/);
  assert.match(engineSource, /AvatarInputPayload/);
  assert.match(engineSource, /keyboard_group/);
  assert.match(engineSource, /keyboard_groups/);
  assert.match(engineSource, /keyboard_visual_keys/);
  assert.match(engineSource, /emit_avatar_input/);
  assert.match(mainSource, /mod avatar_input/);
  assert.match(mainSource, /start_avatar_input_monitor/);
  assert.match(mainSource, /spawn_avatar_input_bridge/);
  assert.match(inputSource, /cfg\(target_os = "windows"\)/);
  assert.match(inputSource, /SetWindowsHookExW/);
  assert.match(inputSource, /WH_KEYBOARD_LL/);
  assert.match(inputSource, /WH_MOUSE_LL/);
  assert.match(inputSource, /GetCursorPos/);
});

test('macOS 权限在启动后恢复时应自动重试桌宠输入监听', () => {
  const mainSource = readFileSync(new URL('../../../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');

  assert.match(mainSource, /spawn_avatar_input_monitor_retry/);
  assert.match(inputSource, /pub fn spawn_avatar_input_monitor_retry/);
  assert.match(inputSource, /has_accessibility_permission\(false\)/);
  assert.match(inputSource, /has_input_monitoring_permission\(\)/);
  assert.match(inputSource, /start_avatar_input_monitor\(&app\)/);
});

test('所有平台均启用智能穿透命中轮询，修复 Windows 穿透模式下通知按钮无法点击（#137）', () => {
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');

  assert.match(inputSource, /smart_click_through_polling_enabled_for_platform/);
  // 不再有 !is_windows 的平台特判；统一启用轮询
  assert.doesNotMatch(inputSource, /fn smart_click_through_polling_enabled_for_platform\(is_windows: bool\) -> bool \{\s*!is_windows\s*\}/);
  assert.match(inputSource, /fn smart_click_through_polling_enabled_for_platform\(\) -> bool \{\s*true\s*\}/);
});

test('桌宠键盘模式应按原版 BongoCat 键区分组选择不同高亮层', () => {
  const source = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');

  assert.match(registrySource, /standard-keyboard-7\.png/);
  assert.match(registrySource, /standard-keyboard-8\.png/);
  assert.match(registrySource, /standard-keyboard-9\.png/);
  assert.match(registrySource, /standard-keyboard-10\.png/);
  assert.match(registrySource, /standard-keyboard-11\.png/);
  assert.match(registrySource, /standard-keyboard-12\.png/);
  assert.match(registrySource, /standard-keyboard-13\.png/);
  assert.match(registrySource, /standard-keyboard-14\.png/);
  assert.match(registrySource, /standard-keyboard-1\.png/);
  assert.match(registrySource, /standard-keyboard-2\.png/);
  assert.match(registrySource, /standard-keyboard-3\.png/);
  assert.match(registrySource, /standard-keyboard-4\.png/);
  assert.match(registrySource, /standard-keyboard-5\.png/);
  assert.match(registrySource, /standard-keyboard-6\.png/);
  assert.match(registrySource, /STANDARD_KEYBOARD_FRAME_BY_GROUP/);
  assert.match(source, /inputActivity\.keyboardGroup/);
  assert.match(source, /keyOverlaySrc = keyboardActive/);
  assert.match(source, /preset\.keyboardFrames\[keyboardGroup\]/);
  assert.match(inputSource, /standard_keyboard_group_from_key_code/);
  assert.match(inputSource, /digit-1/);
  assert.match(inputSource, /digit-7/);
  assert.match(inputSource, /key-q/);
  assert.match(inputSource, /key-e/);
  assert.match(inputSource, /key-r/);
  assert.match(inputSource, /space/);
  assert.match(inputSource, /key-a/);
  assert.match(inputSource, /key-d/);
  assert.match(inputSource, /key-s/);
  assert.match(inputSource, /key-w/);
});

test('桌宠键盘模式应导入原版左右手多帧资源并按键区切换手部帧', () => {
  const source = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(registrySource, /standard-hand-4\.png/);
  assert.match(registrySource, /standard-hand-5\.png/);
  assert.match(registrySource, /standard-hand-6\.png/);
  assert.match(registrySource, /standard-hand-7\.png/);
  assert.match(registrySource, /standard-hand-8\.png/);
  assert.match(registrySource, /standard-hand-9\.png/);
  assert.match(registrySource, /standard-hand-10\.png/);
  assert.match(registrySource, /standard-hand-11\.png/);
  assert.match(registrySource, /standard-hand-12\.png/);
  assert.match(registrySource, /standard-hand-13\.png/);
  assert.match(registrySource, /standard-hand-14\.png/);
  assert.match(registrySource, /standard-hand-0\.png/);
  assert.match(registrySource, /standard-hand-1\.png/);
  assert.match(registrySource, /standard-hand-2\.png/);
  assert.match(registrySource, /standard-hand-3\.png/);
  assert.match(registrySource, /standard-up\.png/);
  assert.match(registrySource, /STANDARD_HAND_FRAME_BY_GROUP/);
  assert.match(source, /standardHandSrc = keyboardActive/);
  assert.match(source, /standard-hand-layer/);
});

test('桌宠鼠标模式应按上游标准模式生成动态左手，并叠加鼠标设备状态层', () => {
  const source = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');

  assert.match(registrySource, /mouse-bg\.png/);
  assert.match(registrySource, /mouse\.png/);
  assert.match(registrySource, /mouse_left\.png/);
  assert.match(registrySource, /mouse_right\.png/);
  assert.match(registrySource, /mouse_side\.png/);
  assert.match(source, /computeStandardMouseGeometry/);
  assert.match(source, /mouseArmPoints/);
  assert.match(source, /mouseDeviceX/);
  assert.match(source, /mouseDeviceY/);
  assert.match(source, /mouse-arm-fill/);
  assert.match(source, /mouse-arm-shadow/);
  assert.match(source, /mouse-arm-stroke/);
  assert.match(registrySource, /MOUSE_DEVICE_OVERLAY_BY_GROUP/);
  assert.match(source, /mouseGroup/);
  assert.match(source, /cursorRatioX/);
  assert.match(source, /cursorRatioY/);
  assert.match(inputSource, /mouse_group_from_event_type/);
  assert.match(inputSource, /record_cursor_ratio/);
  assert.match(inputSource, /current_cursor_ratio/);
  assert.match(inputSource, /mouse-left/);
  assert.match(inputSource, /mouse-right/);
  assert.match(inputSource, /mouse-side/);
  assert.match(inputSource, /mouse-move/);
});

test('Linux 外观设置页应展示桌宠联动能力，并区分完整联动与仅鼠标联动', () => {
  const systemSource = readFileSync(new URL('../../../routes/settings/components/SettingsSystem.svelte', import.meta.url), 'utf8');
  const i18nSource = readFileSync(new URL('../../../lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8');
  const commandsSource = readCommandsSource();
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');

  assert.match(systemSource, /get_linux_session_support/);
  assert.match(systemSource, /avatarInputSupportTitle/);
  assert.match(systemSource, /avatarInputMouseOnly/);
  assert.match(systemSource, /avatarInputUnavailable/);
  assert.match(systemSource, /permissionsLinuxInputHint/);
  assert.match(systemSource, /permissionsGnomeExtensionHint/);
  assert.match(i18nSource, /avatarLinuxSupportTitle/);
  assert.match(i18nSource, /avatarInputMouseOnly/);
  assert.match(i18nSource, /avatarInputUnavailable/);
  assert.match(commandsSource, /avatar_input_support_level/);
  assert.match(commandsSource, /avatar_mouse_supported/);
  assert.match(commandsSource, /avatar_keyboard_supported/);
  assert.match(commandsSource, /gnome_avatar_extension_installed/);
  assert.match(commandsSource, /gnome_avatar_extension_enabled/);
  assert.match(commandsSource, /gnome_avatar_extension_needs_relogin/);
  assert.match(commandsSource, /install_gnome_avatar_extension/);
  assert.match(commandsSource, /requires_relogin/);
  assert.match(inputSource, /gnome-shell-dbus/);
  assert.match(inputSource, /kdotool-mouselocation/);
  assert.match(inputSource, /hyprctl-cursorpos/);
  assert.match(inputSource, /WorkReviewAvatarInput\.GetInput/);
  assert.match(systemSource, /installGnomeAvatarExtension/);
  assert.match(systemSource, /avatarGnomeExtensionInstall/);
  assert.match(systemSource, /avatarGnomeExtensionInstalling/);
  assert.match(systemSource, /avatarGnomeExtensionReady/);
  assert.match(systemSource, /avatarGnomeExtensionRelogin/);
  assert.match(i18nSource, /avatarGnomeExtensionInstall/);
  assert.match(i18nSource, /avatarGnomeExtensionInstalling/);
  assert.match(i18nSource, /avatarGnomeExtensionReady/);
  assert.match(i18nSource, /avatarGnomeExtensionRelogin/);
});

test('设置页顶部权限区仅在 macOS 显示，并提供 mac 权限跳转', () => {
  const settingsSource = readFileSync(new URL('../../../routes/settings/Settings.svelte', import.meta.url), 'utf8');
  const systemSource = readFileSync(new URL('../../../routes/settings/components/SettingsSystem.svelte', import.meta.url), 'utf8');
  const i18nSource = readFileSync(new URL('../../../lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8');
  const commandsSource = readCommandsSource();
  const screenshotSource = readFileSync(new URL('../../../../src-tauri/src/screenshot.rs', import.meta.url), 'utf8');
  const mainSource = readFileSync(new URL('../../../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  const handlerSource = readFileSync(new URL('../../../../src-tauri/src/commands/mod.rs', import.meta.url), 'utf8');

  assert.match(settingsSource, /\{#if settingsRuntimePlatform === 'macos'\}/);
  assert.match(settingsSource, /<SettingsSystem \/>/);
  assert.match(settingsSource, /settings-top-status-zone/);
  assert.match(systemSource, /invoke(?:<[^>]+>)?\('check_permissions'\)/);
  assert.match(systemSource, /invoke\('open_permission_settings'/);
  assert.match(systemSource, /runtimePlatform === 'macos'/);
  assert.match(systemSource, /avatarScreenCapturePermission/);
  assert.match(systemSource, /avatarAccessibilityPermission/);
  assert.match(systemSource, /avatarInputMonitoringPermission/);
  assert.match(systemSource, /permission-summary-strip/);
  assert.match(systemSource, /permission-summary-toggle/);
  assert.match(systemSource, /showPermissionDetails/);
  assert.match(systemSource, /permissionDetailsExpanded/);
  assert.match(systemSource, /settingsGeneral\.permissionsOpen/);
  assert.match(systemSource, /settingsGeneral\.permissionsOpenNow/);
  assert.match(systemSource, /settingsGeneral\.permissionsLater/);
  assert.match(systemSource, /permissionsScreenCaptureGuide/);
  assert.match(systemSource, /permission-summary-title/);
  assert.match(systemSource, /permission-item-card/);
  assert.match(systemSource, /permission-status-pill/);
  assert.match(systemSource, /permission-setup-dialog/);
  assert.match(systemSource, /settingsGeneral\.permissionGranted/);
  assert.match(systemSource, /settingsGeneral\.permissionMissing/);
  assert.match(commandsSource, /"input_monitoring"/);
  assert.match(commandsSource, /open_permission_settings/);
  assert.match(commandsSource, /Privacy_ScreenCapture/);
  assert.match(commandsSource, /Privacy_Accessibility/);
  assert.match(commandsSource, /Privacy_ListenEvent/);
  assert.match(mainSource, /\.invoke_handler\(commands::handler\(\)\)/);
  assert.match(handlerSource, /\bopen_permission_settings\b/);
  assert.match(screenshotSource, /CGPreflightListenEventAccess/);
  assert.match(screenshotSource, /CGRequestListenEventAccess/);
  assert.match(i18nSource, /permissionsTitle:\s*'权限设置'/);
  assert.match(i18nSource, /permissionsRefresh:\s*'重新检查'/);
  assert.match(i18nSource, /permissionsOpen:\s*'开启'/);
  assert.match(i18nSource, /permissionsOpenNow:\s*'去设置'/);
  assert.match(i18nSource, /permissionsLater:\s*'稍后开启'/);
  assert.match(i18nSource, /permissionsInputMonitoringGuide:\s*'如果权限已开启/);
  assert.match(i18nSource, /permissionGranted:\s*'可用'/);
  assert.match(i18nSource, /permissionMissing:\s*'受限'/);
});

test('关闭桌面助手前应主动持久化当前位置，避免重新显示时回到左上角', () => {
  const appearanceSource = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');
  const commandsSource = readCommandsSource();
  const mainSource = readFileSync(new URL('../../../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  const handlerSource = readFileSync(new URL('../../../../src-tauri/src/commands/mod.rs', import.meta.url), 'utf8');

  assert.match(appearanceSource, /invoke(?:<[^>]+>)?\('persist_avatar_position'\)/);
  assert.match(commandsSource, /pub async fn persist_avatar_position/);
  assert.match(commandsSource, /AVATAR_WINDOW_LABEL/);
  assert.match(mainSource, /\.invoke_handler\(commands::handler\(\)\)/);
  assert.match(handlerSource, /\bpersist_avatar_position\b/);
});

test('外观设置页应提供三种桌宠互动风格切换，并持久化 avatar_persona', () => {
  const appearanceSource = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');
  const configSource = readFileSync(new URL('../../../../crates/core/src/config.rs', import.meta.url), 'utf8');
  const i18nSource = readFileSync(new URL('../../../lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8');

  assert.match(appearanceSource, /AVATAR_PERSONA_OPTIONS/);
  assert.match(appearanceSource, /config\.avatar_persona/);
  assert.match(appearanceSource, /selectAvatarPersona/);
  assert.match(appearanceSource, /settingsAppearance\.avatarPersona/);
  assert.match(appearanceSource, /avatarPersonaCompanionTitle/);
  assert.match(appearanceSource, /avatarPersonaAssistantTitle/);
  assert.match(appearanceSource, /avatarPersonaCoachTitle/);
  assert.match(appearanceSource, /avatarPersonaCurrent/);
  assert.match(configSource, /pub avatar_persona: String/);
  assert.match(configSource, /default_avatar_persona/);
  assert.match(i18nSource, /avatarPersonaCompanionTitle/);
  assert.match(i18nSource, /avatarPersonaAssistantTitle/);
  assert.match(i18nSource, /avatarPersonaCoachTitle/);
});

test('桌宠窗口应监听接上次继续事件，并提供继续任务、开始专注、记一下与稍后动作', () => {
  const windowSource = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');
  const cardSource = readFileSync(new URL('./AvatarFollowupCard.svelte', import.meta.url), 'utf8');
  const commandSource = readCommandsSource();
  const mainSource = readFileSync(new URL('../../../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  const handlerSource = readFileSync(new URL('../../../../src-tauri/src/commands/mod.rs', import.meta.url), 'utf8');
  const avatarTaskSource = readFileSync(new URL('../../../../src-tauri/src/tasks/avatar.rs', import.meta.url), 'utf8');
  const followupSource = readFileSync(new URL('../../../../src-tauri/src/avatar_followup.rs', import.meta.url), 'utf8');
  const i18nSource = readFileSync(new URL('../../../lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8');
  const appSource = readFileSync(new URL('../../../App.svelte', import.meta.url), 'utf8');
  const timelineSource = readFileSync(new URL('../../../routes/timeline/Timeline.svelte', import.meta.url), 'utf8');

  assert.match(windowSource, /AvatarFollowupCard/);
  assert.match(windowSource, /avatar-followup-suggestion/);
  assert.match(windowSource, /handle_avatar_followup_action/);
  assert.match(windowSource, /emitTo\('main', 'avatar-open-timeline'/);
  assert.match(windowSource, /startFollowupFocus/);
  assert.match(windowSource, /focusSession/);
  assert.match(windowSource, /focusTimer/);
  assert.match(windowSource, /avatarPersona/);
  assert.match(windowSource, /avatarFollowupFocusStarted/);
  assert.match(windowSource, /avatarFollowupFocusFinished/);
  assert.match(windowSource, /__avatar_nudge_switch_companion__/);
  assert.match(windowSource, /__avatar_backlog_nudge__:/);
  assert.match(i18nSource, /avatarNudgeBacklogCoach/);
  assert.match(windowSource, /rememberFollowup/);
  assert.match(windowSource, /snoozeFollowup/);
  assert.match(windowSource, /FOLLOWUP_PERSONA_THEME/);
  assert.match(windowSource, /avatarFollowupCompanionStrategy/);
  assert.match(windowSource, /avatarFollowupCoachStrategy/);
  assert.match(cardSource, /openTimeline/);
  assert.match(cardSource, /focus/);
  assert.match(cardSource, /remember/);
  assert.match(cardSource, /snooze/);
  // strategy 段已移除（信息精简），不再断言 copy.strategy
  assert.match(cardSource, /copy\.badgeClass/);
  assert.match(cardSource, /copy\.primaryClass/);
  assert.match(cardSource, /dismissLabel/);
  assert.match(commandSource, /pub struct AvatarFollowupActionInput/);
  assert.match(commandSource, /handle_avatar_followup_action/);
  assert.match(commandSource, /"focus"\s*=>\s*crate::avatar_followup::AvatarFollowupAction::Focus/);
  assert.match(commandSource, /avatar_followups\.push/);
  assert.match(followupSource, /AvatarFollowupAction::Focus/);
  assert.match(followupSource, /FOCUS_COOLDOWN_MS/);
  assert.match(mainSource, /mod avatar_followup/);
  assert.match(mainSource, /\.invoke_handler\(commands::handler\(\)\)/);
  assert.match(handlerSource, /\bhandle_avatar_followup_action\b/);
  assert.match(avatarTaskSource, /find_followup_suggestion/);
  assert.match(appSource, /avatar-open-timeline/);
  assert.match(appSource, /push\('\/timeline'\)/);
  assert.match(appSource, /timeline-focus-date/);
  assert.match(timelineSource, /timeline-focus-date/);
  assert.match(timelineSource, /window\.location\.hash/);
});

test('桌宠输入链应维护按住键集合，并支持多键同时显示与释放', () => {
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');

  assert.match(inputSource, /KEYBOARD_STATE/);
  assert.match(inputSource, /pressed_keys/);
  assert.match(inputSource, /normalize_keyboard_groups/);
  assert.match(inputSource, /normalize_keyboard_visual_keys/);
  assert.match(inputSource, /release_keyboard_input/);
  assert.match(inputSource, /WM_KEYUP/);
  assert.match(inputSource, /XI_RawKeyRelease/);
  assert.doesNotMatch(canvasSource, /keyboard-combo-indicator/);
  assert.doesNotMatch(canvasSource, /keyboardVisualLayerSources/);
});

test('桌宠输入桥应以更高刷新率推送输入状态，减少鼠标联动卡顿', () => {
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');

  assert.match(inputSource, /Duration::from_millis\(16\)/);
  assert.match(canvasSource, /tweened\(0\.5/);
  assert.match(canvasSource, /cursorTweenDuration = mouseActive \? 44 : 110/);
  assert.match(canvasSource, /cursorRatioXTween\.set/);
});

test('桌宠窗口应拦截右键菜单和打印快捷键，避免弹出无法消除的原生界面', () => {
  const source = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');

  assert.match(source, /document\.addEventListener\('contextmenu'/);
  assert.match(source, /event\.preventDefault\(\)/);
  assert.match(source, /document\.addEventListener\('keydown'/);
  assert.match(source, /event\.key\s*===\s*'p'/);
  assert.match(source, /event\.metaKey\s*\|\|\s*event\.ctrlKey/);
  assert.match(source, /document\.removeEventListener\('contextmenu'/);
  assert.match(source, /document\.removeEventListener\('keydown'/);
});

test('桌宠窗口应只保留 SVG 桌宠实现，不再引用 Live2D 分支', () => {
  const source = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');

  assert.doesNotMatch(source, /AvatarLive2D/);
  assert.doesNotMatch(source, /live2dLoadFailed/);
  assert.doesNotMatch(source, /avatarType/);
});

test('外观设置页不应再提供桌宠风格切换', () => {
  const source = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');

  assert.doesNotMatch(source, /handleAvatarTypeChange/);
  assert.doesNotMatch(source, /settingsAppearance\.avatarType/);
  assert.doesNotMatch(source, />\s*SVG Cat\s*</);
  assert.doesNotMatch(source, />\s*Live2D\s*</);
});

test('桌宠配置应支持官方预设字段，并在设置页仅展示当前可用的原版标准卡片', () => {
  const configSource = readFileSync(new URL('../../../../crates/core/src/config.rs', import.meta.url), 'utf8');
  const commandsSource = readCommandsSource();
  const appearanceSource = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');
  const i18nSource = readFileSync(new URL('../../../lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8');

  assert.match(configSource, /pub avatar_preset: String/);
  assert.match(configSource, /default_avatar_preset/);
  assert.match(commandsSource, /previous_avatar_preset/);
  assert.match(commandsSource, /config\.avatar_preset/);
  assert.match(appearanceSource, /config\.avatar_preset/);
  assert.match(appearanceSource, /AVATAR_PRESET_OPTIONS/);
  assert.match(registrySource, /original-standard/);
  assert.match(registrySource, /AVAILABLE_AVATAR_PRESET_IDS/);
  assert.match(registrySource, /new Set/);
  assert.match(i18nSource, /avatarPreset:/);
  assert.match(i18nSource, /avatarPresetOriginalTitle/);
});

test('桌宠渲染层应通过官方预设注册表解析资源，而不是继续硬编码单套资源', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const windowSource = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(canvasSource, /getAvatarPresetDefinition/);
  assert.match(canvasSource, /normalizeAvatarPresetId/);
  assert.match(canvasSource, /\$:\s*preset = getAvatarPresetDefinition/);
  assert.match(canvasSource, /preset\.sceneSrc/);
  assert.match(canvasSource, /preset\.showKeyboardOverlay/);
  assert.match(canvasSource, /preset\.showMouseDevice/);
  assert.match(canvasSource, /preset\.showMouseArm/);
  assert.match(windowSource, /avatarPreset/);
  assert.match(registrySource, /showKeyboardOverlay/);
  assert.match(registrySource, /showMouseDevice/);
  assert.match(registrySource, /showMouseArm/);
});

test('官方预设卡片应使用真实桌宠预览，并把已下线预设回退到原版标准', () => {
  const appearanceSource = readFileSync(new URL('../../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');
  const previewSource = readFileSync(new URL('./AvatarPresetPreview.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(appearanceSource, /AvatarPresetPreview/);
  assert.doesNotMatch(appearanceSource, /<img[\s\S]*preset\.preview/);
  assert.match(previewSource, /AvatarCanvas/);
  assert.match(previewSource, /pointer-events-none/);
  assert.match(previewSource, /getAvatarPresetOption/);
  assert.match(registrySource, /original-standard[\s\S]*keyboardActive:\s*true[\s\S]*mouseActive:\s*true/);
  assert.match(registrySource, /AVAILABLE_AVATAR_PRESET_IDS\.has\(presetId\)/);
  assert.match(registrySource, /AVATAR_PRESET_DEFAULT/);
});

test('键盘专注与极简办公应保留各自场景图，并移除调试式热点覆盖', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(canvasSource, /contentTransform/);
  assert.match(canvasSource, /staticCoverSrc/);
  assert.match(canvasSource, /keyboardVisualClip/);
  assert.match(canvasSource, /mouseVisualClip/);
  assert.match(registrySource, /model-standard\/background\.png/);
  assert.match(registrySource, /model-standard\/cover\.png/);
  assert.match(registrySource, /keyboard-focus[\s\S]*showKeyboardOverlay:\s*false/);
  assert.match(registrySource, /keyboard-focus[\s\S]*showMouseDevice:\s*false/);
  assert.match(registrySource, /keyboard-focus[\s\S]*showMouseArm:\s*true/);
  assert.doesNotMatch(registrySource, /keyboard-focus[\s\S]*interactionLayout:/);
  assert.match(registrySource, /model-gamepad\/background\.png/);
  assert.match(registrySource, /model-gamepad\/cover\.png/);
  assert.match(registrySource, /minimal-office[\s\S]*showKeyboardOverlay:\s*false/);
  assert.match(registrySource, /minimal-office[\s\S]*showMouseDevice:\s*false/);
  assert.match(registrySource, /minimal-office[\s\S]*showMouseArm:\s*false/);
  assert.doesNotMatch(registrySource, /minimal-office[\s\S]*interactionLayout:/);
});

test('三套官方预设应保持明显不同的视觉场景，而不是都退回标准底图', () => {
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(registrySource, /original-standard[\s\S]*sceneSrc:\s*mouseBg/);
  assert.match(registrySource, /keyboard-focus[\s\S]*sceneSrc:\s*modelStandardBackground/);
  assert.match(registrySource, /minimal-office[\s\S]*sceneSrc:\s*modelGamepadBackground/);
  assert.match(registrySource, /keyboard-focus[\s\S]*staticCoverSrc:\s*modelStandardCover/);
  assert.match(registrySource, /minimal-office[\s\S]*staticCoverSrc:\s*modelGamepadCover/);
});

test('键盘专注与极简办公应在各自场景上消费静态图层，但不再渲染热点映射', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(canvasSource, /sceneInteractionLayout/);
  assert.match(canvasSource, /keyboardHotspots/);
  assert.match(canvasSource, /mouseHotspots/);
  assert.match(canvasSource, /keyboardVisualSrc/);
  assert.match(registrySource, /keyboard-focus[\s\S]*keyboardVisualLayers:\s*STANDARD_MODEL_LEFT_KEYS/);
  assert.doesNotMatch(registrySource, /keyboard-focus[\s\S]*interactionLayout:\s*STANDARD_MODEL_INTERACTION/);
  assert.match(registrySource, /minimal-office[\s\S]*keyboardVisualLayers:\s*GAMEPAD_KEYBOARD_LAYER_BY_KEY/);
  assert.match(registrySource, /minimal-office[\s\S]*mouseVisualLayers:\s*GAMEPAD_MOUSE_LAYER_BY_GROUP/);
  assert.doesNotMatch(registrySource, /minimal-office[\s\S]*interactionLayout:\s*GAMEPAD_MODEL_INTERACTION/);
});

test('静态场景预设的鼠标联动不应再用 multiply 叠加产生阴影块，鼠标移动高亮应保持可见', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(canvasSource, /scene-interaction-layer/);
  assert.doesNotMatch(canvasSource, /mix-blend-mode:\s*multiply/);
  assert.match(registrySource, /trackpad:\s*polygonHotspot\('44,126 214,101 242,204 73,229',\s*HOTSPOT_CYAN_FILL,\s*HOTSPOT_CYAN_STROKE\)/);
  assert.match(registrySource, /STANDARD_MODEL_INTERACTION[\s\S]*'mouse-move':\s*\[polygonHotspot\('44,126 214,101 242,204 73,229',\s*HOTSPOT_CYAN_FILL,\s*HOTSPOT_CYAN_STROKE\)\]/);
  assert.match(registrySource, /STANDARD_MODEL_INTERACTION[\s\S]*'mouse-left':\s*\[polygonHotspot\('44,126 124,114 157,217 73,229',\s*HOTSPOT_CYAN_FILL,\s*HOTSPOT_CYAN_STROKE\)\]/);
  assert.match(registrySource, /STANDARD_MODEL_INTERACTION[\s\S]*'mouse-right':\s*\[polygonHotspot\('124,114 214,101 242,204 157,217',\s*HOTSPOT_CYAN_FILL,\s*HOTSPOT_CYAN_STROKE\)\]/);
  assert.match(registrySource, /GAMEPAD_MODEL_INTERACTION[\s\S]*'mouse-move':\s*\[polygonHotspot\('333,189 359,202 332,216 306,203',\s*HOTSPOT_CYAN_FILL,\s*HOTSPOT_CYAN_STROKE\)\]/);
});

test('静态封面预设不应再整体抖动，只保留静态封面和精确图层反馈', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');

  assert.doesNotMatch(canvasSource, /getStaticCoverTransform/);
  assert.doesNotMatch(canvasSource, /staticCoverTransform/);
  assert.match(canvasSource, /static-cover-layer/);
  assert.doesNotMatch(canvasSource, /translate\(-4\.2 2\.8\) rotate\(-1\.5 300 162\)/);
  assert.doesNotMatch(canvasSource, /translate\(1\.2 2\.1\) rotate\(0\.8 304 160\)/);
});

test('深色桌面上的提醒气泡应使用固定浅色面板，避免文字和底色一起发黑', () => {
  const popoverSource = readFileSync(new URL('./AvatarPopover.svelte', import.meta.url), 'utf8');

  assert.match(popoverSource, /background:\s*rgba\(255,\s*255,\s*255,\s*0\.96\)/);
  assert.match(popoverSource, /color:\s*rgb\(15,\s*23,\s*42\)/);
  assert.match(popoverSource, /border-color:\s*rgba\(226,\s*232,\s*240,\s*0\.96\)/);
  assert.match(popoverSource, /backdrop-filter:\s*blur\(12px\)\s*saturate\(1\.04\)/);
});

test('静态场景预设应继续通过精确键名驱动源图层映射，保证各自视觉里的键盘鼠标联动', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const windowSource = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');
  const inputSource = readFileSync(new URL('../../../../src-tauri/src/avatar_input.rs', import.meta.url), 'utf8');
  const engineSource = readFileSync(new URL('../../../../src-tauri/src/avatar_engine.rs', import.meta.url), 'utf8');

  assert.match(engineSource, /keyboard_visual_key/);
  assert.match(windowSource, /keyboardVisualKey/);
  assert.match(canvasSource, /keyboardVisualKey/);
  assert.match(canvasSource, /preset\.keyboardVisualLayers\?\.\[keyboardVisualKey\]/);
  assert.match(canvasSource, /preset\.mouseVisualLayers\?\.\[mouseGroup\]/);
  assert.match(
    registrySource,
    /import\.meta\.glob<string>\('\.\/assets\/bongocat\/model-standard\/left-keys\/\*\.png'/
  );
  assert.match(
    registrySource,
    /import\.meta\.glob<string>\('\.\/assets\/bongocat\/model-gamepad\/left-keys\/\*\.png'/
  );
  assert.match(
    registrySource,
    /import\.meta\.glob<string>\('\.\/assets\/bongocat\/model-gamepad\/right-keys\/\*\.png'/
  );
  assert.match(inputSource, /fn keyboard_visual_key_from_key_code/);
  assert.match(inputSource, /"KeyA"/);
  assert.match(inputSource, /"KeyN"/);
  assert.match(inputSource, /"KeyO"/);
  assert.match(inputSource, /"KeyP"/);
  assert.match(inputSource, /"Num9"/);
  assert.match(inputSource, /"Space"/);
  assert.match(inputSource, /"Return"/);
  assert.match(inputSource, /"ShiftLeft"/);
  assert.match(inputSource, /"ControlRight"/);
});

test('静态场景预设应默认先绘制热点层，再绘制 cover，最后再叠加精确键位图层；仅键盘专注允许触控板热点提升到 cover 上方', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');

  assert.match(
    canvasSource,
    /\{#if preCoverKeyboardHotspots.length \|\| preCoverMouseHotspots.length\}[\s\S]*scene-interaction-layer[\s\S]*\{\/if\}[\s\S]*\{#if staticCoverSrc\}[\s\S]*class="static-cover-layer"/
  );
  assert.match(
    canvasSource,
    /\{#if staticCoverSrc\}[\s\S]*class="static-cover-layer"[\s\S]*\{\/if\}[\s\S]*\{#if showKeyboardLayers && keyboardVisualSrc\}[\s\S]*keyboard-visual-layer/
  );
  assert.match(
    canvasSource,
    /\{#if staticCoverSrc\}[\s\S]*class="static-cover-layer"[\s\S]*\{\/if\}[\s\S]*\{#if showMouseLayers && mouseVisualSrc\}[\s\S]*mouse-visual-layer/
  );
  assert.match(
    canvasSource,
    /\{#if staticCoverSrc\}[\s\S]*class="static-cover-layer"[\s\S]*\{\/if\}[\s\S]*\{#if postCoverMouseHotspots.length\}[\s\S]*post-cover-mouse-hotspot-layer/
  );
});

test('静态场景预设应把精确联动图层裁剪到设备区域，且不再启用 cover 上方热点层', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(canvasSource, /keyboardVisualClip/);
  assert.match(canvasSource, /mouseVisualClip/);
  assert.match(canvasSource, /mouseHotspotsAboveCover/);
  assert.match(canvasSource, /clipPath/);
  assert.match(canvasSource, /clip-path=\{keyboardVisualClipUrl \|\| undefined\}/);
  assert.match(canvasSource, /clip-path=\{mouseVisualClipUrl \|\| undefined\}/);
  assert.match(canvasSource, /post-cover-mouse-hotspot-layer/);
  assert.match(registrySource, /keyboard-focus[\s\S]*keyboardVisualClip:/);
  assert.doesNotMatch(registrySource, /keyboard-focus[\s\S]*keyboardHotspotsAboveCover:\s*true/);
  assert.match(registrySource, /minimal-office[\s\S]*keyboardVisualClip:/);
  assert.match(registrySource, /minimal-office[\s\S]*mouseVisualClip:/);
});

test('键盘专注与极简办公不再渲染独立鼠标臂指示器', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(canvasSource, /computeStaticSceneMouseGeometry/);
  assert.match(canvasSource, /staticSceneMouseGeometry =/);
  assert.match(canvasSource, /preset\.mouseMotionModel/);
  assert.match(canvasSource, /class="scene-mouse-paw"/);
  assert.match(canvasSource, /cursorRatioX/);
  assert.match(canvasSource, /cursorRatioY/);
  assert.doesNotMatch(registrySource, /keyboard-focus[\s\S]*mouseMotionModel:/);
  assert.doesNotMatch(registrySource, /minimal-office[\s\S]*mouseMotionModel:/);
});

test('静态预设应移除整块鼠标移动蓝色蒙层，并通过精确图层反馈输入', () => {
  const canvasSource = readFileSync(new URL('./AvatarCanvas.svelte', import.meta.url), 'utf8');
  const registrySource = readFileSync(new URL('./avatarPresetRegistry.ts', import.meta.url), 'utf8');

  assert.match(canvasSource, /keyboardHotspotsAboveCover/);
  assert.match(canvasSource, /postCoverKeyboardHotspots/);
  assert.match(canvasSource, /post-cover-keyboard-hotspot-layer/);
  assert.match(canvasSource, /!mouseVisualSrc && !preset\.mouseMotionModel/);
  assert.doesNotMatch(registrySource, /keyboard-focus[\s\S]*keyboardHotspotsAboveCover:\s*true/);
  assert.doesNotMatch(registrySource, /minimal-office[\s\S]*keyboardHotspotsAboveCover:\s*true/);
  assert.match(registrySource, /keyboard-focus[\s\S]*keyboardVisualLayers:\s*STANDARD_MODEL_LEFT_KEYS/);
  assert.match(registrySource, /minimal-office[\s\S]*mouseVisualLayers:\s*GAMEPAD_MOUSE_LAYER_BY_GROUP/);
});

test('桌宠继续提醒卡片应纵向排列三个次要动作，并为按钮提供完整文案的 title/aria-label', () => {
  const cardSource = readFileSync(new URL('./AvatarFollowupCard.svelte', import.meta.url), 'utf8');

  assert.doesNotMatch(cardSource, /grid-cols-3/);
  assert.match(cardSource, /flex flex-col gap-2/);
  assert.match(cardSource, /title=\{copy\.focusFull \|\| copy\.focus\}/);
  assert.match(cardSource, /aria-label=\{copy\.focusFull \|\| copy\.focus\}/);
  assert.match(cardSource, /title=\{copy\.rememberFull \|\| copy\.remember\}/);
  assert.match(cardSource, /aria-label=\{copy\.rememberFull \|\| copy\.remember\}/);
  assert.match(cardSource, /title=\{copy\.snoozeFull \|\| copy\.snooze\}/);
  assert.match(cardSource, /aria-label=\{copy\.snoozeFull \|\| copy\.snooze\}/);
  assert.match(cardSource, /w-\[min\(92vw,348px\)\]/);
});

test('桌宠窗口应在继续提醒卡片出现/消失时动态扩展原生窗口尺寸', () => {
  const windowSource = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');
  const commandsSource = readCommandsSource();
  const mainSource = readFileSync(new URL('../../../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  const handlerSource = readFileSync(new URL('../../../../src-tauri/src/commands/mod.rs', import.meta.url), 'utf8');
  const engineSource = readFileSync(new URL('../../../../src-tauri/src/avatar_engine.rs', import.meta.url), 'utf8');

  assert.match(windowSource, /set_avatar_window_expanded/);
  assert.match(windowSource, /syncAvatarExpansion\(followup != null\)/);
  assert.match(windowSource, /avatarExpanded === expanded/);
  assert.match(windowSource, /let avatarExpanded = null/);
  assert.match(windowSource, /avatarExpanded = previous/);
  assert.match(commandsSource, /pub async fn set_avatar_window_expanded/);
  assert.match(commandsSource, /apply_avatar_window_expansion/);
  assert.match(mainSource, /\.invoke_handler\(commands::handler\(\)\)/);
  assert.match(handlerSource, /\bset_avatar_window_expanded\b/);
  assert.match(engineSource, /pub fn apply_avatar_window_expansion/);
  assert.match(engineSource, /clamp_window_within_current_monitor/);
  assert.match(engineSource, /AVATAR_WINDOW_EXPANDED_BASE_WIDTH/);
  assert.match(engineSource, /AVATAR_WINDOW_EXPANDED_BASE_HEIGHT/);
  assert.match(engineSource, /fn avatar_window_size\(scale: f64, expanded: bool, body_hidden: bool\)/);
});

test('桌宠窗口在系统级 resize 后应回正尺寸，避免拖拽时持续变小', () => {
  const windowSource = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');

  assert.match(windowSource, /scheduleAvatarSizeCorrection/);
  assert.match(windowSource, /nativeWindow\.onResized/);
  assert.match(windowSource, /suppressNextResizeCorrection/);
  assert.match(windowSource, /syncAvatarExpansion\(expanded, \{ force: true \}\)/);
});

test('继续提醒卡片应为每种人格提供紧凑按钮文案与完整 tooltip 文案', () => {
  const windowSource = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');
  const i18nSource = readFileSync(new URL('../../../lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8');

  assert.match(windowSource, /focusFullKey:\s*'settingsAppearance\.avatarFollowupFocusFull'/);
  assert.match(windowSource, /focusFullKey:\s*'settingsAppearance\.avatarFollowupFocusFullCompanion'/);
  assert.match(windowSource, /focusFullKey:\s*'settingsAppearance\.avatarFollowupFocusFullCoach'/);
  assert.match(windowSource, /rememberFullKey:\s*'settingsAppearance\.avatarFollowupRememberFull'/);
  assert.match(windowSource, /snoozeFullKey:\s*'settingsAppearance\.avatarFollowupSnoozeFull'/);
  assert.match(windowSource, /focusFull:\s*t\(theme\.focusFullKey\)/);
  assert.match(windowSource, /rememberFull:\s*t\(theme\.rememberFullKey\)/);
  assert.match(windowSource, /snoozeFull:\s*t\(theme\.snoozeFullKey\)/);
  assert.match(i18nSource, /avatarFollowupFocusFull:/);
  assert.match(i18nSource, /avatarFollowupFocusFullCompanion:/);
  assert.match(i18nSource, /avatarFollowupFocusFullCoach:/);
  assert.match(i18nSource, /avatarFollowupRememberFull:/);
  assert.match(i18nSource, /avatarFollowupSnoozeFull:/);
});

test('隐藏桌宠时仅实际通知区域应阻止鼠标穿透，透明区域保持可操作', () => {
  const windowSource = readFileSync(new URL('../../../routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');
  const popoverSource = readFileSync(new URL('./AvatarPopover.svelte', import.meta.url), 'utf8');
  const followupSource = readFileSync(new URL('./AvatarFollowupCard.svelte', import.meta.url), 'utf8');
  const commandSource = readFileSync(new URL('../../../../src-tauri/src/commands/avatar.rs', import.meta.url), 'utf8');
  const mainSource = readFileSync(new URL('../../../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  const handlerSource = readFileSync(new URL('../../../../src-tauri/src/commands/mod.rs', import.meta.url), 'utf8');

  assert.match(popoverSource, /data-avatar-hit-region=\{bubble\?\.persistent \? 'bubble' : undefined\}/);
  assert.match(popoverSource, /class:pointer-events-auto=\{bubble\?\.persistent\}/);
  assert.doesNotMatch(popoverSource, /class="pointer-events-auto relative rounded-\[16px\]/);
  assert.match(followupSource, /data-avatar-hit-region="followup"/);
  assert.match(windowSource, /import \{ onMount, tick \} from 'svelte'/);
  assert.match(windowSource, /document\.querySelectorAll\('\[data-avatar-hit-region\]'\)/);
  assert.match(windowSource, /invoke\('set_avatar_interactive_regions'/);
  assert.match(windowSource, /getBoundingClientRect\(\)/);
  assert.match(windowSource, /precise:\s*!!state\.avatarBodyHidden/);
  assert.match(windowSource, /new ResizeObserver/);
  assert.match(windowSource, /cancelAnimationFrame/);
  assert.match(windowSource, /interactiveRegionObserver\?\.disconnect\(\)/);
  assert.match(windowSource, /class:pointer-events-none=\{state\.avatarBodyHidden\}/);
  assert.match(windowSource, /state\.avatarBodyHidden[\s\S]*startAvatarDrag\(e\)/);
  assert.match(windowSource, /class="h-full w-\[82%\] pointer-events-auto"/);
  assert.match(commandSource, /pub async fn set_avatar_interactive_regions/);
  assert.match(mainSource, /\.invoke_handler\(commands::handler\(\)\)/);
  assert.match(handlerSource, /\bset_avatar_interactive_regions\b/);
});

test('桌宠气泡面板在非紧凑文案下应允许更长的英文文字换行，避免整段被裁断', () => {
  const popoverSource = readFileSync(new URL('./AvatarPopover.svelte', import.meta.url), 'utf8');

  assert.match(popoverSource, /width: min\(82vw, 336px\); min-width: min\(82vw, 160px\); max-width: min\(82vw, 336px\);/);
  assert.match(popoverSource, /max-height: 140px/);
  assert.match(popoverSource, /word-break: normal/);
  assert.match(popoverSource, /overflow-wrap: anywhere/);
  assert.doesNotMatch(popoverSource, /line-break: strict/);
});
