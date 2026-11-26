import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';

async function readCommandsSource() {
  // commands.rs 已按领域拆分为 commands/*.rs，这里拼接所有子模块以保持断言语义不变。
  const dir = new URL('./src/commands/', import.meta.url);
  const files = (await readdir(dir)).filter((f) => f.endsWith('.rs'));
  const parts = await Promise.all(files.map((f) => readFile(new URL(f, dir), 'utf8')));
  return parts.join('\n');
}

test('主窗口显示逻辑应尽量跟随当前活跃空间而不是停留在应用原空间', async () => {
  const windowSource = await readFile(new URL('./src/window.rs', import.meta.url), 'utf8');
  const commandSource = await readCommandsSource();
  const avatarSource = await readFile(new URL('../src/routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');

  assert.match(windowSource, /MoveToActiveSpace|setCollectionBehavior_/);
  assert.match(windowSource, /source_window_label/);
  assert.match(commandSource, /show_main_window/);
  assert.match(avatarSource, /invoke\('show_main_window', \{ sourceWindowLabel: appWindow\.label \}\)/);
});
