import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';

async function readCommandsSource() {
  // commands.rs 已按领域拆分为 commands/*.rs，这里拼接所有子模块以保持断言语义不变。
  const dir = new URL('../src-tauri/src/commands/', import.meta.url);
  const files = (await readdir(dir)).filter((f) => f.endsWith('.rs'));
  const parts = await Promise.all(files.map((f) => readFile(new URL(f, dir), 'utf8')));
  return parts.join('\n');
}

test('应用壳层应监听录制状态变更事件并同步侧边栏状态', async () => {
  const source = await readFile(new URL('./App.svelte', import.meta.url), 'utf8');

  assert.match(
    source,
    /safeListen\(\s*'recording-state-changed',\s*isRecordingStateChangedPayload/
  );
  assert.match(source, /isRecording\s*=\s*payload\.isRecording/);
  assert.match(source, /isPaused\s*=\s*payload\.isPaused/);
});

test('托盘和设置的配置变更应回推到前端缓存与设置页', async () => {
  const appSource = await readFile(new URL('./App.svelte', import.meta.url), 'utf8');
  const settingsSource = await readFile(
    new URL('./routes/settings/Settings.svelte', import.meta.url),
    'utf8'
  );
  const rustSource = (
    await Promise.all([
      readCommandsSource(),
      readFile(new URL('../src-tauri/src/tray.rs', import.meta.url), 'utf8'),
      readFile(new URL('../src-tauri/src/main.rs', import.meta.url), 'utf8'),
    ])
  ).join('\n');

  assert.match(appSource, /safeListen\(\s*'config-changed',\s*isRuntimeConfig/);
  assert.match(appSource, /cache\.setConfig\(payload\)/);
  assert.match(settingsSource, /cache\.subscribe\(\(state\)\s*=>/);
  assert.match(settingsSource, /config\s*=\s*state\.config/);
  assert.match(rustSource, /config-changed/);
});

test('轻量模式关闭主界面时应触发窗口关闭而非仅隐藏', async () => {
  const source = await readFile(new URL('./App.svelte', import.meta.url), 'utf8');

  assert.match(source, /runtimeConfig\?\.lightweight_mode/);
  assert.match(source, /await appWindow\.close\(\)/);
  assert.match(source, /await appWindow\.hide\(\)/);
});

test('自动检查更新不应在网络请求前提前写入最后检查时间', async () => {
  const source = await readFile(new URL('./App.svelte', import.meta.url), 'utf8');

  assert.match(source, /await runUpdateFlow\(/);
  assert.doesNotMatch(source, /invoke\('update_last_check_time'\)/);
});

test('应用壳层应屏蔽 WebView 原生右键菜单', async () => {
  const source = await readFile(new URL('./App.svelte', import.meta.url), 'utf8');

  assert.match(source, /function preventNativeContextMenu\(e: MouseEvent\): void/);
  assert.match(source, /preventNativeContextMenu\(e: MouseEvent\): void \{\s*e\.preventDefault\(\);/);
  assert.match(source, /window\.addEventListener\('contextmenu', preventNativeContextMenu\)/);
  assert.match(source, /window\.removeEventListener\('contextmenu', preventNativeContextMenu\)/);
});
