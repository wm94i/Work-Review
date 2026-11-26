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

test('前端应向日报生成与工作助手透传当前 locale，并让日期输入跟随语言切换', async () => {
  const [appSource, storageTaskSource, reportSource, askSource, timelineSource, summarySource, summaryDrawerSource] = await Promise.all([
    readFile(new URL('./App.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../src-tauri/src/tasks/storage.rs', import.meta.url), 'utf8'),
    readFile(new URL('./routes/report/Report.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/ask/Ask.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/timeline/Timeline.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/timeline/Summary.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/timeline/HourlySummaryDrawer.svelte', import.meta.url), 'utf8'),
  ]);

  assert.match(appSource, /safeListen\(\s*'auto-report-generated'/);
  assert.match(
    storageTaskSource,
    /async fn auto_report_and_insight_task[\s\S]*commands::generate_report_inner\(\s*today\.clone\(\),\s*Some\(false\),\s*Some\(locale\.clone\(\)\),/,
  );
  assert.match(reportSource, /invoke\('generate_report', \{ date: targetDate, force, locale: targetLocale \}\)/);
  assert.match(reportSource, /invoke<DailyReport \| null>\('get_saved_report', \{ date: targetDate, locale: targetLocale \}\)/);
  assert.match(
    reportSource,
    /if \(\s*!savedReport\s*&&\s*previousReport\?\.date === targetDate\s*&&\s*previousReport\?\.content\s*&&\s*reportGenerationOwnership\.claim\(requestId, cacheData\?\.reportGenerating \?\? false\)\s*\)[\s\S]*?invoke\('generate_report', \{ date: targetDate, force: false, locale: targetLocale \}\)/,
  );
  assert.match(askSource, /invoke<AssistantAnswer>\('chat_work_assistant', \{[\s\S]*locale: currentLocale,[\s\S]*\}\)/);

  assert.match(reportSource, /LocalizedDatePicker/);
  assert.match(timelineSource, /LocalizedDatePicker/);
  assert.doesNotMatch(summarySource, /LocalizedDatePicker/);
  assert.match(summaryDrawerSource, /formatDurationLocalized/);
  assert.match(summaryDrawerSource, /t\('timelineSummary\./);
  assert.match(reportSource, /localeCode=\{currentLocale\}/);
  assert.match(timelineSource, /localeCode=\{currentLocale\}/);
  assert.doesNotMatch(summarySource, /localeCode=\{currentLocale\}/);
});

test('助手页展示层不应继续依赖写死中文的工作智能工具函数', async () => {
  const [askSource, i18nSource] = await Promise.all([
    readFile(new URL('./routes/ask/Ask.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./lib/i18n/index.ts', import.meta.url), 'utf8'),
  ]);

  assert.doesNotMatch(askSource, /from '\.\.\/\.\.\/lib\/utils\/workIntelligence\.js'/);
  assert.match(askSource, /formatDurationLocalized/);
  assert.match(i18nSource, /export function formatDurationLocalized/);
  assert.doesNotMatch(askSource, /toLocaleString\('zh-CN'/);
});

test('后端日报模板与助手提示词应支持按 locale 输出', async () => {
  const [commandsSource, summarySource, localSource] = await Promise.all([
    readCommandsSource(),
    readFile(new URL('../crates/core/src/analysis/summary.rs', import.meta.url), 'utf8'),
    readFile(new URL('../crates/core/src/analysis/local.rs', import.meta.url), 'utf8'),
  ]);

  assert.match(commandsSource, /pub async fn chat_work_assistant\([\s\S]*locale: Option<String>/);
  assert.match(commandsSource, /pub async fn generate_report\([\s\S]*locale: Option<String>/);
  assert.match(commandsSource, /build_assistant_system_prompt/);
  assert.match(summarySource, /locale:/);
  assert.match(summarySource, /AppLocale|ReportLocale|report_locale/);
  assert.match(localSource, /locale:/);
});
