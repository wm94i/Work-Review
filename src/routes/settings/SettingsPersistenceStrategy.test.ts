import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { applyConfigFields } from '../../lib/utils/appearancePersistence.ts';

function extractFieldList(source: string, constantName: string): string[] {
  const match = source.match(new RegExp(
    `const ${constantName} = \\[([\\s\\S]*?)\\] as const`,
  ));
  assert.ok(match, `缺少 ${constantName} 字段白名单`);
  return [...match[1].matchAll(/'([^']+)'/g)].map((item) => item[1]);
}

test('设置页保存应在共享队列执行时只合并设置表单负责的字段', async () => {
  const source = await readFile(new URL('./Settings.svelte', import.meta.url), 'utf8');

  const expectedFields = [
    'auto_start',
    'auto_start_silent',
    'hide_dock_icon',
    'lightweight_mode',
    'work_time_enabled',
    'work_time_segments',
    'work_start_hour',
    'work_start_minute',
    'work_end_hour',
    'work_end_minute',
    'standard_work_hours',
    'idle_threshold_minutes',
    'daily_work_goal_minutes',
    'goal_notifications',
    'memory_enabled',
    'daily_report_auto_generate_time',
    'avatar_enabled',
    'avatar_scale',
    'avatar_opacity',
    'avatar_persona',
    'avatar_preset',
    'avatar_click_through',
    'avatar_body_hidden',
    'avatar_proactive_ai_enabled',
    'break_reminder_enabled',
    'break_reminder_interval_minutes',
    'ai_mode',
    'text_model',
    'assistant_web_access_enabled',
    'assistant_search_provider',
    'assistant_search_api_key',
    'memory_semantic_enabled',
    'embedding_provider',
    'embedding_endpoint',
    'embedding_model',
    'embedding_api_key',
    'assistant_memory_enabled',
    'node_gateway',
    'node_devices',
    'mcp_server_enabled',
    'localhost_api_enabled',
    'localhost_api_host',
    'localhost_api_port',
    'telegram_bot_enabled',
    'telegram_bot_token',
    'telegram_bot_proxy',
    'telegram_bot_allowed_chat_ids',
    'feishu_bot_enabled',
    'feishu_app_id',
    'feishu_app_secret',
    'feishu_verification_token',
    'wecom_bot_enabled',
    'wecom_corp_id',
    'wecom_token',
    'wecom_encoding_aes_key',
    'dingtalk_bot_enabled',
    'dingtalk_app_secret',
    'privacy',
    'screenshot_interval',
    'storage',
    'daily_report_export_dir',
    'daily_report_auto_export',
    'remote_storage',
  ];

  assert.deepEqual(extractFieldList(source, 'SETTINGS_FORM_CONFIG_FIELDS'), expectedFields);
  assert.match(source, /import \{ updateConfigQueued \} from '\$lib\/utils\/configSaveQueue\.ts';/);
  assert.match(source, /const configSnapshot = structuredClone\(config\);/);
  assert.match(source, /updateConfigQueued<SettingsConfig>/);
  assert.match(source, /applyConfigFields\(latestConfig, configSnapshot, SETTINGS_FORM_CONFIG_FIELDS\)/);
  assert.match(source, /cache\.setConfig\(persistedConfig\);/);
  assert.doesNotMatch(source, /saveConfigQueued\(/);
});

test('全局保存不得击穿 Appearance 独立自动保存字段的乐观回滚', async () => {
  const source = await readFile(new URL('./Settings.svelte', import.meta.url), 'utf8');
  const fields = extractFieldList(source, 'SETTINGS_FORM_CONFIG_FIELDS');
  interface SaveIsolationFixture {
    auto_start: boolean;
    ui_template: string;
    ui_visual_style: string;
    background_image: string | null;
    background_opacity: number;
    background_blur: number;
    unknown_config_field: string;
    [field: string]: unknown;
  }

  const latestConfig: SaveIsolationFixture = {
    auto_start: false,
    ui_template: 'classic',
    ui_visual_style: 'c',
    background_image: null,
    background_opacity: 0.25,
    background_blur: 1,
    unknown_config_field: '由最新持久化配置保留',
  };
  const optimisticSnapshot: SaveIsolationFixture = {
    auto_start: true,
    ui_template: 'evidence-star-map',
    ui_visual_style: 'a',
    background_image: 'optimistic.jpg',
    background_opacity: 0.8,
    background_blur: 2,
    unknown_config_field: '旧快照不得覆盖',
  };

  const saved = applyConfigFields(latestConfig, optimisticSnapshot, fields);

  assert.equal(saved.auto_start, true);
  assert.equal(saved.ui_template, 'classic');
  assert.equal(saved.ui_visual_style, 'c');
  assert.equal(saved.background_image, null);
  assert.equal(saved.background_opacity, 0.25);
  assert.equal(saved.background_blur, 1);
  assert.equal(saved.unknown_config_field, '由最新持久化配置保留');
});

test('全局保存应继续负责 avatar 与 break reminder 字段', async () => {
  const source = await readFile(new URL('./Settings.svelte', import.meta.url), 'utf8');
  const fields = extractFieldList(source, 'SETTINGS_FORM_CONFIG_FIELDS');

  assert.deepEqual(
    fields.filter((field) => field.startsWith('avatar_') || field.startsWith('break_reminder_')),
    [
      'avatar_enabled',
      'avatar_scale',
      'avatar_opacity',
      'avatar_persona',
      'avatar_preset',
      'avatar_click_through',
      'avatar_body_hidden',
      'avatar_proactive_ai_enabled',
      'break_reminder_enabled',
      'break_reminder_interval_minutes',
    ],
  );
});
