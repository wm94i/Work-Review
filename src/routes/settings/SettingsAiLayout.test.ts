import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('AI 设置中的 API 密钥输入应支持显示与隐藏切换', async () => {
  const source = await readFile(
    new URL('./components/SettingsAI.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /let showApiKey = false;/);
  assert.match(source, /\{#if showApiKey\}/);
  assert.match(source, /type="text"/);
  assert.match(source, /type="password"/);
  assert.match(source, /settingsAI\.hideApiKey/);
  assert.match(source, /settingsAI\.showApiKey/);
});

test('联网、语义记忆、后台自动索引与长期记忆开关应暴露本地化 switch 语义', async () => {
  const source = await readFile(
    new URL('./components/SettingsAI.svelte', import.meta.url),
    'utf8'
  );

  assert.equal((source.match(/role="switch"/g) || []).length, 4);
  assert.match(source, /aria-label=\{t\('settingsAI\.webAccess\.title'\)\}/);
  assert.match(source, /aria-label=\{t\('settingsAI\.semanticMemory\.title'\)\}/);
  assert.match(source, /aria-label=\{t\('settingsAI\.semanticMemory\.autoIndexTitle'\)\}/);
  assert.match(source, /aria-label=\{t\('settingsAI\.assistantMemory\.enabled'\)\}/);
  assert.equal((source.match(/aria-checked=\{/g) || []).length, 4);
});

test('日报导出目录应从 AI 设置移到存储设置', async () => {
  const aiSource = await readFile(
    new URL('./components/SettingsAI.svelte', import.meta.url),
    'utf8'
  );
  const storageSource = await readFile(
    new URL('./components/SettingsStorage.svelte', import.meta.url),
    'utf8'
  );

  assert.doesNotMatch(aiSource, /日报 Markdown 导出目录/);
  assert.match(storageSource, /settingsStorage\.exportDir/);
  assert.match(storageSource, /pickDailyReportExportDir/);
});

test('模型选择使用 select 下拉并支持手动输入切换', async () => {
  const source = await readFile(
    new URL('./components/SettingsAI.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /invoke<string\[]>\('fetch_models'/);
  assert.match(source, /refreshModels/);
  assert.match(source, /fetchedModels/);
  assert.match(source, /<select/);
  assert.match(source, /settingsAI\.manualModel/);
  assert.match(source, /settingsAI\.refreshModels/);
  assert.match(source, /let showManualInput = false;/);
});

test('刷新模型列表后应给出反馈，且仅在模型为空时才默认回填首项', async () => {
  const source = await readFile(
    new URL('./components/SettingsAI.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /!config\.text_model\.model\?\.trim\(\)/);
  assert.match(source, /settingsAI\.loadedModels/);
});

test('AI 设置应提供超时与思考模式控件', async () => {
  const source = await readFile(
    new URL('./components/SettingsAI.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /settingsAI\.assistantTimeout/);
  assert.match(source, /settingsAI\.reportTimeout/);
  assert.match(source, /settingsAI\.thinkingMode/);
  assert.match(source, /generation_capabilities/);
  assert.match(source, /enable_thinking/);
});

test('已获取模型数量通过 modelsLoaded 变量追踪', async () => {
  const source = await readFile(
    new URL('./components/SettingsAI.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /let modelsLoaded = 0;/);
  assert.match(source, /modelsLoaded = fetchedModels\.length/);
  assert.match(source, /modelsLoaded > 0/);
});

test('select 列表应渲染所有已获取模型并提供手动输入选项', async () => {
  const source = await readFile(
    new URL('./components/SettingsAI.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /#each fetchedModels as model \(model\)/);
  assert.match(source, /__manual__/);
  assert.doesNotMatch(source, /MANUAL_MODEL_VALUE/);
});
