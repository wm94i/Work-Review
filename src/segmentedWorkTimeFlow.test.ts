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

test('统计与工作时段判断应使用有效分段时间配置', async () => {
  const commandsSource = await readCommandsSource();

  assert.match(commandsSource, /state\.config\.effective_work_segments\(\)/);
  assert.match(commandsSource, /get_daily_stats_with_segments/);
});

test('自动日报时间应由 Rust 后台任务依据分段工作时间配置调度并通知前端刷新缓存', async () => {
  const [appSource, storageTaskSource] = await Promise.all([
    readFile(new URL('./App.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../src-tauri/src/tasks/storage.rs', import.meta.url), 'utf8'),
  ]);

  assert.match(storageTaskSource, /fn resolve_auto_report_work_end_minutes/);
  assert.match(storageTaskSource, /config\.effective_work_segments\(\)/);
  assert.match(storageTaskSource, /auto_report_and_insight_task/);
  assert.match(appSource, /auto-report-generated/);
});
