import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('概览页应加载并展示专注节奏与打断源指标', async () => {
  const source = await readFile(new URL('../Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /invoke<FocusMetrics>\('get_focus_metrics'/);
  assert.match(source, /focusMetrics\.totalDuration > 0/);
  assert.match(source, /overview-focus-strip/);
  assert.match(source, /overview\.focusScore/);
  assert.match(source, /overview\.focusLongestStreak/);
  assert.match(source, /overview\.focusDeepSessions/);
  assert.match(source, /overview\.focusSwitchRate/);
  assert.match(source, /overview\.focusInterrupters/);
});

test('存储设置应提供 Obsidian Frontmatter、增量标题区块与 Git 提交关联控件', async () => {
  const source = await readFile(
    new URL('./components/SettingsStorage.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /daily_report_export_frontmatter/);
  assert.match(source, /daily_report_export_section_heading/);
  assert.match(source, /settingsStorage\.exportFrontmatter/);
  assert.match(source, /settingsStorage\.exportSectionHeading/);

  assert.match(source, /git_report_enabled/);
  assert.match(source, /git_repo_paths/);
  assert.match(source, /git_author_filter/);
  assert.match(source, /invoke<GitRepoSummary\[]>\('get_git_commits_for_date'/);
  assert.match(source, /commit\.hash/);
  assert.match(source, /settingsStorage\.gitAddRepo/);
  assert.match(source, /settingsStorage\.gitPreviewToday/);
});

test('AI 设置应挂载独立的按场景模型分流面板且不破坏开关计数契约', async () => {
  const [aiSource, routingSource, askSource] = await Promise.all([
    readFile(new URL('./components/SettingsAI.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./components/ModelRoutingPanel.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../ask/Ask.svelte', import.meta.url), 'utf8'),
  ]);

  assert.match(aiSource, /import ModelRoutingPanel from '\.\/ModelRoutingPanel\.svelte'/);
  assert.match(aiSource, /<ModelRoutingPanel/);
  assert.equal((aiSource.match(/role="switch"/g) || []).length, 4);

  assert.match(routingSource, /report_provider/);
  assert.match(routingSource, /assistant_provider/);
  assert.match(routingSource, /classification_provider/);
  assert.match(routingSource, /proactive_provider/);
  assert.match(routingSource, /settingsAI\.routingFollowDefault/);

  assert.match(askSource, /task_model_routing\?\.assistant_provider/);
  assert.match(askSource, /routed-assistant:/);
});

test('四项扩展功能的文案应同步覆盖四种语言且无缺失键', async () => {
  const requiredKeys = [
    'focusScore',
    'focusLongestStreak',
    'focusDeepSessions',
    'focusSwitchRate',
    'focusInterrupters',
    'routingTitle',
    'routingReport',
    'routingAssistant',
    'routingClassification',
    'routingProactive',
    'exportFrontmatter',
    'exportSectionHeading',
    'gitTitle',
    'gitEnabled',
    'gitPreviewToday',
  ];

  for (const language of ['zh-CN', 'zh-TW', 'en', 'ar']) {
    const source = await readFile(
      new URL(`../../lib/i18n/locales/${language}.ts`, import.meta.url),
      'utf8'
    );
    for (const key of requiredKeys) {
      assert.match(source, new RegExp(`\\b${key}:`), `${language} 应包含 ${key}`);
    }
  }
});
