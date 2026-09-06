import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const read = (path: string) => readFile(new URL(path, import.meta.url), 'utf8');

test('App Shell 窗口控制按钮应统一使用 Lucide 图标', async () => {
  const app = await read('./App.svelte');
  const controlsStart = app.indexOf('<div class="app-shell-window-controls');
  const controlsEnd = app.indexOf('</div>', controlsStart);

  assert.notEqual(controlsStart, -1, '应存在窗口控制按钮区域');
  assert.notEqual(controlsEnd, -1, '窗口控制按钮区域应闭合');

  const controls = app.slice(controlsStart, controlsEnd);
  assert.match(app, /import Minus from 'lucide-svelte\/icons\/minus';/);
  assert.match(app, /import Square from 'lucide-svelte\/icons\/square';/);
  assert.match(app, /import X from 'lucide-svelte\/icons\/x';/);
  assert.match(controls, /<Minus\b/);
  assert.match(controls, /<Square\b/);
  assert.match(controls, /<X\b/);
  assert.doesNotMatch(controls, /<svg\b/);
});
