import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import {
  applyConfigFields,
  createPersistedConfigSnapshot,
} from './lib/utils/appearancePersistence.ts';

const appearanceUrl = new URL('./routes/settings/components/SettingsAppearance.svelte', import.meta.url);

async function readAppearance(): Promise<string> {
  return readFile(appearanceUrl, 'utf8');
}

test('外观设置必须按字段在统一串行队列执行时合并最新配置', async () => {
  const source = await readAppearance();

  assert.match(source, /import\s+\{\s*updateConfigQueued\s*\}\s+from\s+['"]\$lib\/utils\/configSaveQueue\.ts['"]/);
  assert.doesNotMatch(source, /invoke(?:<[^>]+>)?\s*\(\s*['"]save_config['"]/);

  const wrapperStart = source.indexOf('async function saveAppearanceConfig');
  const wrapperEnd = source.indexOf('$: if (config !== observedConfig', wrapperStart);
  assert.notEqual(wrapperStart, -1);
  assert.notEqual(wrapperEnd, -1);
  assert.match(source.slice(wrapperStart, wrapperEnd), /await\s+updateConfigQueued<AppearanceConfig>/);
  assert.match(source.slice(wrapperStart, wrapperEnd), /applyConfigFields\(latestConfig,\s*configSnapshot,\s*fields\)/);
  assert.doesNotMatch(
    source.slice(0, wrapperStart) + source.slice(wrapperEnd),
    /updateConfigQueued</,
  );
});

test('模板保存失败应恢复最后成功快照并保持设置页待保存状态', async () => {
  const source = await readAppearance();
  const start = source.indexOf('async function selectUiTemplate');
  const end = source.indexOf('async function selectUiVisualStyle', start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const templateHandler = source.slice(start, end);
  const failureStart = templateHandler.lastIndexOf('catch (e)');
  const failureBranch = templateHandler.slice(failureStart);

  assert.match(source, /const persistedConfigSnapshot\s*=\s*createPersistedConfigSnapshot\(config\)/);
  assert.match(templateHandler, /await\s+saveAppearanceConfig\(configSnapshot,\s*\['ui_template'\]\)/);
  assert.match(failureBranch, /cache\.setConfig\(persistedConfigSnapshot\.read\(\)\)/);
  assert.match(failureBranch, /dispatch\(['"]change['"],\s*config\)/);
  assert.doesNotMatch(failureBranch, /autosaved:\s*true/);
});

test('外部替换配置对象时应刷新模板持久化基线', async () => {
  const source = await readAppearance();

  assert.match(source, /let observedConfig\s*=\s*config/);
  assert.match(source, /config\s*!==\s*observedConfig/);
  assert.match(source, /persistedUiTemplate\s*=\s*config\.ui_template\s*\|\|\s*['"]classic['"]/);
  assert.match(source, /persistedConfigSnapshot\.recordSuccessfulSave\(config\)/);
});

test('模板保存成功时只有完整配置仍与发送快照一致才可标记为已自动保存', async () => {
  const source = await readAppearance();
  const start = source.indexOf('async function selectUiTemplate');
  const end = source.indexOf('async function selectUiVisualStyle', start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const templateHandler = source.slice(start, end);

  assert.match(
    templateHandler,
    /if\s*\(pendingUiTemplate\s*===\s*null\s*&&\s*configsMatch\(config,\s*configSnapshot\)\)/,
  );
  assert.match(
    templateHandler,
    /if\s*\(pendingUiTemplate\s*===\s*null\s*&&\s*configsMatch\(config,\s*configSnapshot\)\)\s*\{[\s\S]*?cache\.setConfig\(structuredClone\(savedConfig\)\)[\s\S]*?autosaved:\s*true[\s\S]*?\}\s*else\s*\{\s*dispatch\(['"]change['"],\s*config\)/,
  );
});

test('模板卡片文案应使用逻辑方向对齐以支持 RTL', async () => {
  const source = await readAppearance();
  const start = source.indexOf('<div class="settings-card evidence-template-settings">');
  const end = source.indexOf('<div class="settings-card" data-locale={currentLocale}>', start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const templateCards = source.slice(start, end);

  assert.doesNotMatch(templateCards, /class=["'][^"']*\btext-left\b[^"']*["']/);
  assert.match(templateCards, /style=["'][^"']*text-align:\s*start;?[^"']*["']/);
});

test('其他外观设置成功后模板保存失败必须回滚到最新持久化快照', () => {
  const config = {
    ui_template: 'classic',
    ui_visual_style: 'c',
    background_blur: 1,
  };
  const persistedSnapshot = createPersistedConfigSnapshot(config);

  // 先成功保存另一个设置；组件随后仍会原地修改同一个 config 对象。
  config.ui_visual_style = 'a';
  persistedSnapshot.recordSuccessfulSave(config);

  // 模板切换进入乐观状态，但本次保存失败，不能污染已成功保存的基线。
  config.ui_template = 'evidence-star-map';
  config.background_blur = 8;

  assert.deepEqual(persistedSnapshot.read(), {
    ui_template: 'classic',
    ui_visual_style: 'a',
    background_blur: 1,
  });
});

test('其他外观字段保存不得携带尚未成功的模板乐观值', () => {
  const persisted = {
    ui_template: 'classic',
    ui_visual_style: 'c',
    background_blur: 1,
  };
  const optimistic = {
    ui_template: 'evidence-star-map',
    ui_visual_style: 'a',
    background_blur: 1,
  };

  const saved = applyConfigFields(persisted, optimistic, ['ui_visual_style']);

  assert.deepEqual(saved, {
    ui_template: 'classic',
    ui_visual_style: 'a',
    background_blur: 1,
  });
});
