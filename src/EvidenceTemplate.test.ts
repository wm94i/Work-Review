import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const componentUrls = {
  shell: new URL('./lib/components/evidence/EvidenceShell.svelte', import.meta.url),
  navigation: new URL('./lib/components/evidence/EvidenceNavigation.svelte', import.meta.url),
  overviewHeader: new URL('./lib/components/evidence/EvidenceOverviewHeader.svelte', import.meta.url),
  timelineHeader: new URL('./lib/components/evidence/EvidenceTimelineHeader.svelte', import.meta.url),
};

async function readComponent(url: URL): Promise<string> {
  try {
    return await readFile(url, 'utf8');
  } catch {
    return '';
  }
}

async function readEvidenceSources(): Promise<Record<keyof typeof componentUrls, string>> {
  const entries = await Promise.all(
    Object.entries(componentUrls).map(async ([name, url]) => [name, await readComponent(url)] as const),
  );

  return Object.fromEntries(entries) as Record<keyof typeof componentUrls, string>;
}

const emojiPattern = /[\u{1F000}-\u{1FAFF}\u{2600}-\u{27BF}]/u;
const characterIconPattern = />\s*[×✕✖✓✔✎←→‹›]\s*</u;
const lucideImportPattern = /from ['"]lucide-svelte(?:\/icons\/[^'"]+)?['"]/;

test('证据星图组件必须使用 Lucide，并禁止手写 SVG、Emoji 与字符图标', async () => {
  const sources = await readEvidenceSources();

  for (const [name, source] of Object.entries(sources)) {
    assert.notEqual(source, '', `${name} 组件尚未创建`);
    assert.doesNotMatch(source, /<svg\b/i, `${name} 不得包含手写 SVG`);
    assert.doesNotMatch(source, emojiPattern, `${name} 不得包含 Emoji`);
    assert.doesNotMatch(source, characterIconPattern, `${name} 不得使用字符充当图标`);
  }

  assert.match(sources.shell, lucideImportPattern);
  assert.match(sources.navigation, lucideImportPattern);
  assert.match(sources.overviewHeader, lucideImportPattern);
  assert.match(sources.timelineHeader, lucideImportPattern);
});

test('EvidenceShell 应提供语义区域、可插槽布局和独立的星图视觉背景', async () => {
  const source = await readComponent(componentUrls.shell);

  assert.match(source, /<main\b[^>]*class="evidence-shell/);
  assert.match(source, /aria-label=\{t\(['"]evidenceTemplate\.workspace['"]\)\}/);
  assert.match(source, /<slot\s+name="navigation"\s*\/?>/);
  assert.match(source, /<slot\s*\/?>/);
  assert.match(source, /<slot\s+name="inspector"\s*\/?>/);
  assert.match(source, /<Orbit\b/);
  assert.match(source, /evidence-shell__field/);
  assert.match(source, /@media\s*\(max-width:\s*900px\)/);
  assert.match(source, /@media\s*\(prefers-reduced-motion:\s*reduce\)/);
});

test('EvidenceNavigation 应提供当前路由、录制状态和可访问导航语义', async () => {
  const source = await readComponent(componentUrls.navigation);

  assert.match(source, /export let activeRoute/);
  assert.match(source, /export let isRecording/);
  assert.match(source, /export let isPaused/);
  assert.match(source, /createEventDispatcher/);
  assert.match(source, /isRecording && !isPaused/);
  assert.match(source, /dispatch\('toggle-recording'\)/);
  assert.match(source, /<nav\b[^>]*aria-label=\{t\(['"]evidenceTemplate\.mainNavigation['"]\)\}/);
  assert.match(source, /aria-current=\{[^}]+\? ['"]page['"] : undefined\}/);
  assert.match(source, /role="status"/);
  assert.match(source, /aria-live="polite"/);
  assert.match(source, /import LayoutDashboard from/);
  assert.match(source, /import Waypoints from/);
  assert.match(source, /import FileText from/);
  assert.match(source, /import Bot from/);
  assert.match(source, /import Settings from/);
  assert.match(source, /<svelte:component\b/);
  assert.match(source, /@media\s*\(max-width:\s*900px\)/);
});

test('概览标题应提供日期导航、今天动作与可插入的页面动作', async () => {
  const source = await readComponent(componentUrls.overviewHeader);

  assert.match(source, /createEventDispatcher/);
  assert.match(source, /export let dateLabel/);
  assert.match(source, /aria-labelledby="evidence-overview-title"/);
  assert.match(source, /aria-label=\{t\(['"]evidenceTemplate\.previousDay['"]\)\}/);
  assert.match(source, /aria-label=\{t\(['"]evidenceTemplate\.nextDay['"]\)\}/);
  assert.match(source, /aria-label=\{t\(['"]evidenceTemplate\.today['"]\)\}/);
  assert.match(source, /<ChevronLeft\b/);
  assert.match(source, /<ChevronRight\b/);
  assert.match(source, /<CalendarDays\b/);
  assert.match(source, /<slot\s+name="actions"\s*\/?>/);
  assert.match(source, /@media\s*\(prefers-reduced-motion:\s*reduce\)/);
});

test('时间线标题应公开证据数量、视图切换与可访问的时间控制', async () => {
  const source = await readComponent(componentUrls.timelineHeader);

  assert.match(source, /createEventDispatcher/);
  assert.match(source, /export let evidenceCount/);
  assert.match(source, /export let viewMode/);
  assert.match(source, /aria-labelledby="evidence-timeline-title"/);
  assert.match(source, /role="group"\s+aria-label=\{t\(['"]evidenceTemplate\.timelineView['"]\)\}/);
  assert.match(source, /aria-pressed=\{viewMode === ['"]orbit['"]\}/);
  assert.match(source, /aria-pressed=\{viewMode === ['"]stream['"]\}/);
  assert.match(source, /aria-label=\{t\(['"]evidenceTemplate\.previousDay['"]\)\}/);
  assert.match(source, /aria-label=\{t\(['"]evidenceTemplate\.nextDay['"]\)\}/);
  assert.match(source, /<Orbit\b/);
  assert.match(source, /<Rows3\b/);
  assert.match(source, /<ChevronLeft\b/);
  assert.match(source, /<ChevronRight\b/);
  assert.match(source, /@media\s*\(max-width:\s*720px\)/);
});
