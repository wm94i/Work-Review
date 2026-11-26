import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

// 「删除记录」功能完整性断言（按单条/日期/时间段/应用，连带删活动+截图）。
// 纯源码扫描，沿用 I18nLayout.test.ts 的 readFile + assert.match 模式。

test('database.rs 应提供 4 个删除活动的方法', async () => {
  const src = await readFile(new URL('../crates/core/src/database.rs', import.meta.url), 'utf8');
  for (const fn of [
    'delete_activity_by_id',
    'delete_activities_by_date',
    'delete_activities_by_range',
    'delete_activities_by_app',
  ]) {
    assert.match(src, new RegExp(`fn ${fn}\\b`), `缺少数据库方法 ${fn}`);
  }
});

test('stats.rs 应注册 4 个删除命令并经 remove_screenshot_files 连带删截图', async () => {
  const src = await readFile(
    new URL('../src-tauri/src/commands/stats.rs', import.meta.url),
    'utf8',
  );
  for (const cmd of [
    'delete_activity',
    'delete_activities_by_date',
    'delete_activities_by_range',
    'delete_activities_by_app',
  ]) {
    assert.match(src, new RegExp(`pub async fn ${cmd}\\b`), `缺少命令 ${cmd}`);
  }
  assert.match(src, /remove_screenshot_files/);
});

test('commands::handler 应在 invoke_handler 注册 4 个删除命令', async () => {
  const [mainSrc, handlerSrc] = await Promise.all([
    readFile(new URL('../src-tauri/src/main.rs', import.meta.url), 'utf8'),
    readFile(new URL('../src-tauri/src/commands/mod.rs', import.meta.url), 'utf8'),
  ]);
  assert.match(mainSrc, /\.invoke_handler\(commands::handler\(\)\)/);
  for (const cmd of [
    'delete_activity',
    'delete_activities_by_date',
    'delete_activities_by_range',
    'delete_activities_by_app',
  ]) {
    assert.match(handlerSrc, new RegExp(`\\b${cmd}\\b`), `未注册 ${cmd}`);
  }
});

test('Timeline 应提供 4 个删除入口与对应 invoke', async () => {
  const src = await readFile(
    new URL('./routes/timeline/Timeline.svelte', import.meta.url),
    'utf8',
  );
  for (const command of [
    'delete_activity',
    'delete_activities_by_date',
    'delete_activities_by_range',
    'delete_activities_by_app',
  ]) {
    assert.match(
      src,
      new RegExp(`invoke(?:<[^>]+>)?\\('${command}'`),
      `缺少调用 invoke('${command}'`,
    );
  }
  assert.match(src, /async function deleteActivity/, '缺少单条删除入口');
  assert.match(src, /showCleanupPanel/, '缺少批量清理面板');
});
