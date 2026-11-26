import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';

test('存储设置应提供独立的启用截图开关，而不只是截图间隔', async () => {
  const source = await readFile(
    new URL('./components/SettingsStorage.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /settingsStorage\.screenshotsEnabled/);
  assert.match(source, /config\.storage\.screenshots_enabled/);
  assert.match(source, /settingsStorage\.screenshotsEnabledHint/);
  assert.match(source, /role="switch"/);
  assert.match(source, /aria-label=\{t\('settingsStorage\.screenshotsEnabled'\)\}/);
  assert.match(source, /aria-checked=\{config\.storage\.screenshots_enabled\}/);
});

test('存储设置数值输入和自动导出开关应提供本地化可访问名称', async () => {
  const source = await readFile(
    new URL('./components/SettingsStorage.svelte', import.meta.url),
    'utf8'
  );

  for (const key of ['pollingInterval', 'retentionDays', 'storageLimitLabel', 'autoExport']) {
    assert.match(source, new RegExp(`aria-label=\\{t\\('settingsStorage\\.${key}'\\)\\}`));
  }
  assert.match(source, /aria-checked=\{config\.daily_report_auto_export\}/);
  assert.equal((source.match(/settingsStorage\.(?:showSecret|hideSecret)/g) || []).length, 6);
  assert.match(source, /aria-label=\{`\$\{t\(s3AccessKeyVisible/);
  assert.match(source, /aria-label=\{`\$\{t\(webdavPasswordVisible/);
});

test('元数据清理应默认关闭，启用须确认，关闭不需要确认', async () => {
  const source = await readFile(new URL('./components/SettingsStorage.svelte', import.meta.url), 'utf8');
  const settings = await readFile(new URL('./Settings.svelte', import.meta.url), 'utf8');
  const toggle = source.match(/  async function toggleMetadataCleanup\(\) \{[\s\S]*?\n  \}/)?.[0];
  assert.ok(toggle, '执行组件的实际确认逻辑');
  assert.match(settings, /metadata_cleanup_enabled: false/);
  for (const [enabled, approved, expected, expectedAsks] of [
    [false, false, false, 1],
    [false, true, true, 1],
    [true, false, false, 0],
  ] as const) {
    let asks = 0;
    let changes = 0;
    const config = { storage: { metadata_cleanup_enabled: enabled, metadata_retention_days: 30 } };
    const context = {
      config,
      isConfirmingMetadataCleanup: false,
      t: (key: string) => key,
      ask: async () => { asks += 1; return approved; },
      handleChange: () => { changes += 1; },
    };
    await runInNewContext(`${toggle}\ntoggleMetadataCleanup();`, context);
    assert.equal(config.storage.metadata_cleanup_enabled, expected);
    assert.equal(asks, expectedAsks);
    assert.equal(changes, enabled || approved ? 1 : 0);
    assert.equal(context.isConfirmingMetadataCleanup, false);
  }
  assert.match(source, /aria-label=\{t\('settingsStorage\.metadataCleanup'\)\}/);
  assert.match(source, /for="metadata-retention-days"/);
  assert.match(source, /id="metadata-retention-days"/);
});

test('元数据保留提示应提供四种语言并与截图期限分开', async () => {
  for (const language of ['zh-CN', 'zh-TW', 'en', 'ar']) {
    const source = await readFile(new URL(`../../lib/i18n/locales/${language}.ts`, import.meta.url), 'utf8');
    for (const key of ['metadataCleanup', 'metadataCleanupHint', 'metadataRetentionDays', 'metadataRetentionHint', 'metadataCleanupConfirm']) {
      assert.match(source, new RegExp(`\\b${key}:`), `${language} 应翻译 ${key}`);
    }
  }
});

test('WebDAV 应提供常用配置同步开关', async () => {
  const source = await readFile(
    new URL('./components/SettingsStorage.svelte', import.meta.url),
    'utf8'
  );
  assert.match(source, /settingsStorage\.syncAppConfig/);
  assert.match(source, /sync_app_config/);
});

test('轮询间隔滑块应与 5~600 秒范围一致并在修改后局部自动保存', async () => {
  const source = await readFile(
    new URL('./components/SettingsStorage.svelte', import.meta.url),
    'utf8'
  );
  assert.match(source, /id="screenshot-interval"[\s\S]*?min="5"[\s\S]*?max="600"[\s\S]*?step="1"/);
  assert.match(source, /Math\.round\(numeric\)/);
  assert.match(source, /invoke\('patch_config',\s*\{\s*patch:\s*\{\s*screenshot_interval:\s*nextValue\s*\}\s*\}\)/);
});
