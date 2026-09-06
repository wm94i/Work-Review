import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const read = (path: string) => readFile(new URL(path, import.meta.url), 'utf8');

test('应用应保留经典模板并可切换证据星图 Shell', async () => {
  const [app, css] = await Promise.all([read('./App.svelte'), read('./app.css')]);
  assert.match(app, /uiTemplate/);
  assert.match(app, /applyUiTemplate/);
  assert.match(app, /EvidenceShell/);
  assert.match(app, /ui-template-changed/);
  assert.match(app, /template-\{\$uiTemplate\}/);
  assert.match(css, /\.app-shell\.template-evidence-star-map/);
  assert.match(css, /\[data-ui-template=['"]evidence-star-map['"]\]/);
  assert.match(css, /prefers-reduced-motion:\s*reduce/);
});

test('外观设置应提供经典与证据星图模板并支持自动保存回滚', async () => {
  const [settings, appearance, zhCN, en] = await Promise.all([
    read('./routes/settings/Settings.svelte'),
    read('./routes/settings/components/SettingsAppearance.svelte'),
    read('./lib/i18n/locales/zh-CN.ts'),
    read('./lib/i18n/locales/en.ts'),
  ]);
  assert.match(settings, /ui_template/);
  assert.match(settings, /loadedConfig\.ui_template = 'classic'/);
  assert.match(appearance, /UI_TEMPLATE_OPTIONS/);
  assert.match(appearance, /'evidence-star-map'/);
  assert.match(appearance, /selectUiTemplate/);
  assert.match(appearance, /ui-template-changed/);
  assert.match(appearance, /previousTemplate/);
  assert.match(appearance, /autosaved:\s*true/);
  for (const source of [zhCN, en]) {
    assert.match(source, /uiTemplate/);
    assert.match(source, /evidenceStarMap/);
  }
});

test('概览与时间线应接入证据星图专用头部而不复制数据请求', async () => {
  const [overview, timeline] = await Promise.all([
    read('./routes/Overview.svelte'),
    read('./routes/timeline/Timeline.svelte'),
  ]);
  assert.match(overview, /EvidenceOverviewHeader/);
  assert.match(overview, /\$uiTemplate === 'evidence-star-map'/);
  assert.match(timeline, /EvidenceTimelineHeader/);
  assert.match(timeline, /\$uiTemplate === 'evidence-star-map'/);
  assert.match(timeline, /evidenceCount=\{hasMore \? `\$\{activities\.length\}\+` : activities\.length\}/);
});

test('证据星图录制切换应防止非录制态和请求期间重复触发', async () => {
  const [app, navigation] = await Promise.all([
    read('./App.svelte'),
    read('./lib/components/evidence/EvidenceNavigation.svelte'),
  ]);

  assert.match(app, /let recordingTransitionPending = false;/);
  assert.match(
    app,
    /async function toggleEvidenceRecording\(\): Promise<void> \{[\s\S]*?if \(!isRecording \|\| recordingTransitionPending\) return;[\s\S]*?recordingTransitionPending = true;[\s\S]*?finally \{[\s\S]*?recordingTransitionPending = false;[\s\S]*?\}/,
  );
  assert.match(app, /\{recordingTransitionPending\}/);
  assert.match(navigation, /export let recordingTransitionPending = false;/);
  assert.match(navigation, /disabled=\{!isRecording \|\| recordingTransitionPending\}/);
});

test('应用内完整配置保存应统一进入串行队列', async () => {
  const app = await read('./App.svelte');

  assert.match(app, /import \{ updateConfigQueued \} from ['"]\.\/lib\/utils\/configSaveQueue\.ts['"];?/);
  assert.equal([...app.matchAll(/await updateConfigQueued</g)].length, 2);
  assert.doesNotMatch(app, /invoke(?:<[^>]+>)?\(\s*['"]save_config['"]/);
});
