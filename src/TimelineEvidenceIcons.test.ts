import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const timelineUrl = new URL('./routes/timeline/Timeline.svelte', import.meta.url);

async function readTimeline(): Promise<string> {
  return readFile(timelineUrl, 'utf8');
}

function sliceBetween(source: string, startMarker: string, endMarker: string): string {
  const start = source.indexOf(startMarker);
  const end = source.indexOf(endMarker, start);

  assert.notEqual(start, -1, `未找到起始标记：${startMarker}`);
  assert.notEqual(end, -1, `未找到结束标记：${endMarker}`);
  return source.slice(start, end);
}

test('证据星图时间线的功能图标必须使用 lucide-svelte 直接子路径导入', async () => {
  const source = await readTimeline();

  for (const [component, path] of [
    ['Sparkles', 'sparkles'],
    ['LoaderCircle', 'loader-circle'],
    ['ChevronDown', 'chevron-down'],
    ['ChevronRight', 'chevron-right'],
    ['RefreshCw', 'refresh-cw'],
    ['Trash2', 'trash-2'],
    ['FolderDown', 'folder-down'],
    ['X', 'x'],
    ['ListFilter', 'list-filter'],
  ]) {
    assert.match(
      source,
      new RegExp(`import\\s+${component}\\s+from\\s+['"]lucide-svelte/icons/${path}['"]`),
      `${component} 必须通过 lucide-svelte/icons/${path} 直接导入`,
    );
  }
});

test('时间段摘要按钮必须使用语义 Lucide 图标且不包含手写 SVG', async () => {
  const source = await readTimeline();
  const summaryButton = sliceBetween(
    source,
    'bind:this={summaryTrigger}',
    '</button>',
  );

  assert.match(summaryButton, /<Sparkles\b/);
  assert.match(summaryButton, /<ChevronRight\b/);
  assert.doesNotMatch(summaryButton, /<svg\b/i);
});

test('加载更多按钮必须使用 Lucide 加载与展开图标且不包含手写 SVG', async () => {
  const source = await readTimeline();
  const loadMoreButton = sliceBetween(
    source,
    'on:click={loadMore}',
    '</button>',
  );

  assert.match(loadMoreButton, /<LoaderCircle\b/);
  assert.match(loadMoreButton, /<ChevronDown\b/);
  assert.doesNotMatch(loadMoreButton, /<svg\b/i);
});

test('证据模式可见的时间线源码不应残留手写 SVG', async () => {
  const source = await readTimeline();

  assert.doesNotMatch(source, /<svg\b/i);
});

test('分类下拉 Lucide 图标必须使用显式类承接原有尺寸样式', async () => {
  const source = await readTimeline();

  assert.match(source, /<ChevronDown\s+class="timeline-category-trigger-icon"\s+aria-hidden="true"\s*\/>/);
  assert.match(source, /\.timeline-category-trigger-icon\s*\{/);
  assert.doesNotMatch(source, /\.timeline-category-trigger\s+svg\s*\{/);
});
