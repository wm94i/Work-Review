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

test('网站语义规则命中后应同步基础分类，保障工休统计可用', async () => {
  const commandsSource = await readCommandsSource();
  const recordingSource = await readFile(new URL('../src-tauri/src/recording/mod.rs', import.meta.url), 'utf8');
  const coreSource = await readFile(
    new URL('../crates/core/src/categorize.rs', import.meta.url),
    'utf8',
  );
  const monitorSource = await readFile(new URL('../src-tauri/src/monitor.rs', import.meta.url), 'utf8');

  assert.match(
    commandsSource,
    /work_review_core::categorize::semantic_category_to_base_category/,
  );
  assert.match(commandsSource, /update_activity_classification\([\s\S]*next_base_category/);
  assert.match(
    recordingSource,
    /classification\.base_category\s*=\s*work_review_core::categorize::semantic_category_to_base_category/,
  );
  assert.match(coreSource, /pub fn semantic_category_to_base_category/);
  assert.doesNotMatch(monitorSource, /fn semantic_category_to_base_category/);
});

test('桌面采集侧迁移到核心分类模块后应继续使用兼容口径', async () => {
  const recordingSource = await readFile(new URL('../src-tauri/src/recording/mod.rs', import.meta.url), 'utf8');
  const avatarSource = await readFile(
    new URL('../src-tauri/src/avatar_engine.rs', import.meta.url),
    'utf8',
  );
  const categorySource = await readFile(
    new URL('../src-tauri/src/commands/category.rs', import.meta.url),
    'utf8',
  );

  assert.match(
    recordingSource,
    /work_review_core::categorize::categorize_collected_app_with_rules/,
  );
  assert.match(
    avatarSource,
    /work_review_core::categorize::find_collected_category_override/,
  );
  assert.match(
    avatarSource,
    /work_review_core::categorize::categorize_collected_app_with_rules/,
  );
  assert.match(
    categorySource,
    /work_review_core::categorize::find_collected_category_override/,
  );
});
