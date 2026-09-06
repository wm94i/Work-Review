import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const reportUrl = new URL('./routes/report/Report.svelte', import.meta.url);

async function readReport(): Promise<string> {
  return readFile(reportUrl, 'utf8');
}

function sliceFunction(source: string, startMarker: string, endMarker: string): string {
  const start = source.indexOf(startMarker);
  const end = source.indexOf(endMarker, start);
  assert.notEqual(start, -1, `未找到函数：${startMarker}`);
  assert.notEqual(end, -1, `未找到函数结束边界：${endMarker}`);
  return source.slice(start, end);
}

test('日报提示词保存必须在共享队列执行时读取最新配置且只更新自定义提示词', async () => {
  const source = await readReport();
  const handler = sliceFunction(
    source,
    'async function persistReportPrompt()',
    '/** 预设数量上限',
  );

  assert.match(
    source,
    /import\s+\{\s*updateConfigQueued\s*\}\s+from\s+['"]\$lib\/utils\/configSaveQueue\.ts['"]/,
  );
  assert.doesNotMatch(source, /\bsaveConfigQueued\b/);
  assert.match(
    handler,
    /const\s+customPrompt\s*=\s*\(config\.daily_report_custom_prompt\s*\|\|\s*['"]['"]\)\.trim\(\)/,
  );
  assert.match(handler, /config\.daily_report_custom_prompt\s*=\s*customPrompt/);
  assert.match(handler, /await\s+updateConfigQueued<ReportConfig>\s*\(/);
  assert.match(handler, /latestConfig\.daily_report_custom_prompt\s*=\s*promptToSave/);
  assert.doesNotMatch(handler, /daily_report_prompt_presets/);
  assert.doesNotMatch(handler, /invoke(?:<[^>]+>)?\s*\(\s*['"]save_config['"]/);
});

test('日报提示词保存进行中再次触发时必须合并并串行保存最终最新值', async () => {
  const source = await readReport();
  const handler = sliceFunction(
    source,
    'async function persistReportPrompt()',
    '/** 预设数量上限',
  );

  assert.doesNotMatch(
    handler,
    /config\.ai_mode\s*!==\s*['"]summary['"]\s*\|\|\s*promptSaving/,
  );
  assert.match(source, /let\s+pendingReportPrompt\s*:\s*string\s*\|\s*null\s*=\s*null/);
  assert.match(source, /let\s+promptSaveTask\s*:\s*Promise<void>\s*\|\s*null\s*=\s*null/);
  assert.match(handler, /pendingReportPrompt\s*=\s*customPrompt/);
  assert.match(handler, /if\s*\(promptSaveTask\)\s*\{\s*return\s+promptSaveTask/);
  assert.match(handler, /while\s*\(pendingReportPrompt\s*!==\s*null\)/);
  assert.match(handler, /const\s+promptToSave\s*=\s*pendingReportPrompt/);
  assert.match(handler, /latestConfig\.daily_report_custom_prompt\s*=\s*promptToSave/);
});

test('日报提示词预设保存必须只把本地预设快照合并进执行时最新配置', async () => {
  const source = await readReport();
  const handler = sliceFunction(
    source,
    'async function persistPromptPresets()',
    '// 把节点移到 document.body',
  );

  assert.match(
    handler,
    /const\s+promptPresets\s*=\s*structuredClone\(config\.daily_report_prompt_presets\)/,
  );
  assert.match(handler, /await\s+updateConfigQueued<ReportConfig>\s*\(/);
  assert.match(handler, /latestConfig\.daily_report_prompt_presets\s*=\s*promptPresets/);
  assert.doesNotMatch(handler, /latestConfig\.daily_report_custom_prompt\s*=/);
  assert.doesNotMatch(handler, /invoke(?:<[^>]+>)?\s*\(\s*['"]save_config['"]/);
  assert.doesNotMatch(source, /\bsavePresets\b/);
});
