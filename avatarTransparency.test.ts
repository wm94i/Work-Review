import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const source = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('Windows 透明恢复应复用电源回调，不能依赖录制循环或新轮询', () => {
  const clock = source('./src-tauri/src/activity_clock.rs');
  const recovery = source('./src-tauri/src/avatar_transparency.rs');
  const main = source('./src-tauri/src/main.rs');
  const callback = clock.slice(clock.indexOf('unsafe extern "system" fn notification('), clock.indexOf('pub(crate) struct PowerMonitor {'));
  assert.match(callback, /needs_display_recovery\(previous, next_power_state\(previous, event\)\)/);
  assert.match(callback, /crate::avatar_transparency::schedule\(app\)/);
  const appHandleSet = main.indexOf('APP_HANDLE.set(');
  const startRecordingTask = main.indexOf('"background_screenshot_task"');
  assert.ok(appHandleSet > 0 && startRecordingTask > appHandleSet);
  assert.doesNotMatch(recovery, /is_recording|is_paused|loop\s*\{|interval\(/);
  assert.match(main, /#\[cfg\(target_os = "windows"\)\]\s*mod avatar_transparency;/);
});

test('原生透明恢复应合并通知、在 GUI 线程执行并再次检查显示和窗口可见状态', () => {
  const recovery = source('./src-tauri/src/avatar_transparency.rs');
  assert.match(recovery, /RECOVERY_PENDING\.swap\(true, Ordering::AcqRel\)/);
  assert.match(recovery, /sleep\(Duration::from_millis\(250\)\)/);
  assert.match(recovery, /run_on_main_thread\(move \|\|/);
  assert.match(recovery, /if !crate::activity_clock::display_is_available\(\)/);
  assert.match(recovery, /if window\.is_visible\(\)\?/);
  assert.match(recovery, /restore_window\(window\.hwnd\(\)\?\.0 as HWND\)\?/);
  assert.equal((recovery.match(/RECOVERY_PENDING\.store\(false, Ordering::Release\)/g) ?? []).length, 2);
  assert.doesNotMatch(recovery, /set_focus|set_position|set_size|set_ignore_cursor_events|\.show\(|\.hide\(|WebviewWindowBuilder|disable-gpu/);
});

test('重新显示已有桌宠应恢复透明，但隐藏桌宠不能触发显示或重建', () => {
  const engine = source('./src-tauri/src/avatar_engine.rs');
  const sync = engine.slice(engine.indexOf('pub fn sync_avatar_window('), engine.indexOf('pub fn set_avatar_click_through('));
  assert.match(sync, /window\.show\(\);\s*#\[cfg\(target_os = "windows"\)\]\s*crate::avatar_transparency::schedule\(app\)/);
  const hiding = sync.slice(sync.indexOf('} else if let Some(window)'));
  assert.match(hiding, /window\.hide\(\)/);
  assert.doesNotMatch(hiding, /avatar_transparency::schedule|window\.show\(/);
});

test('DWM 恢复必须保留返回值校验和临时 GDI 区域清理', () => {
  const recovery = source('./src-tauri/src/avatar_transparency.rs');
  assert.match(recovery, /hwnd\.is_null\(\) \|\| IsWindow\(hwnd\) == 0/);
  assert.match(recovery, /CreateRectRgn\(0, 0, -1, -1\)/);
  assert.match(recovery, /dwFlags: DWM_BB_ENABLE \| DWM_BB_BLURREGION/);
  assert.match(recovery, /let result = DwmEnableBlurBehindWindow\(hwnd, &settings\)/);
  assert.match(recovery, /DeleteObject\(region as _\)/);
  assert.match(recovery, /if result < 0/);
  assert.match(recovery, /HRESULT=0x/);
});
