import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('桌宠模型生成提醒应有独立开关且不影响本地规则提醒', async () => {
  const [settingsSource, configSource, mainSource, zhCNSource, enSource, zhTWSource] = await Promise.all([
    readFile(new URL('./routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../crates/core/src/config.rs', import.meta.url), 'utf8'),
    readFile(new URL('../src-tauri/src/main.rs', import.meta.url), 'utf8'),
    readFile(new URL('./lib/i18n/locales/zh-CN.ts', import.meta.url), 'utf8'),
    readFile(new URL('./lib/i18n/locales/en.ts', import.meta.url), 'utf8'),
    readFile(new URL('./lib/i18n/locales/zh-TW.ts', import.meta.url), 'utf8'),
  ]);

  assert.match(configSource, /pub avatar_proactive_ai_enabled: bool/);
  assert.match(configSource, /avatar_proactive_ai_enabled: false/);
  assert.match(configSource, /桌宠模型生成提醒默认应关闭/);

  assert.match(mainSource, /avatar_proactive_ai_enabled/);
  assert.match(mainSource, /avatar_proactive_ai_should_run\(/);
  assert.match(mainSource, /if avatar_proactive_ai_should_run\(/);
  assert.match(mainSource, /avatar_proactive_ai_should_run\([\s\S]{0,260}avatar_proactive_ai_enabled[\s\S]{0,260}&text_model/);

  assert.match(settingsSource, /toggleAvatarProactiveAi/);
  assert.match(settingsSource, /config\.avatar_proactive_ai_enabled/);
  assert.match(settingsSource, /settingsAppearance\.avatarProactiveAi/);
  assert.match(settingsSource, /settingsAppearance\.avatarProactiveAiDescription/);
  assert.match(settingsSource, /settingsAppearance\.avatarLocalReminderNote/);
  assert.match(settingsSource, /settingsAppearance\.avatarProactiveAiDataNotice/);
  assert.match(settingsSource, /settingsAppearance\.avatarProactiveAiRequiresAvatar/);
  assert.match(settingsSource, /disabled=\{!config\.avatar_enabled\}/);
  assert.match(settingsSource, /config\.avatar_proactive_ai_enabled && config\.avatar_enabled/);
  assert.match(settingsSource, /saveConfigQuietly\(\['avatar_proactive_ai_enabled'\]\)/);

  for (const source of [zhCNSource, enSource, zhTWSource]) {
    assert.match(source, /avatarProactiveAi:/);
    assert.match(source, /avatarProactiveAiDescription:/);
    assert.match(source, /avatarLocalReminderNote:/);
    assert.match(source, /avatarProactiveAiDataNotice:/);
    assert.match(source, /avatarProactiveAiRequiresAvatar:/);
  }
});
