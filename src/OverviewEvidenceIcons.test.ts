import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const overviewUrl = new URL('./routes/Overview.svelte', import.meta.url);

async function readOverview(): Promise<string> {
  return readFile(overviewUrl, 'utf8');
}

test('概览功能图标必须使用 lucide-svelte 直接子路径导入', async () => {
  const source = await readOverview();

  for (const [component, path] of [
    ['LayoutDashboard', 'layout-dashboard'],
    ['ChevronLeft', 'chevron-left'],
    ['ChevronRight', 'chevron-right'],
    ['Sparkles', 'sparkles'],
    ['ChartNoAxesGantt', 'chart-no-axes-gantt'],
    ['ChartNoAxesColumn', 'chart-no-axes-column'],
    ['X', 'x'],
    ['Check', 'check'],
    ['ChevronUp', 'chevron-up'],
    ['ChevronDown', 'chevron-down'],
  ]) {
    assert.match(
      source,
      new RegExp(`import\\s+${component}\\s+from\\s+['"]lucide-svelte/icons/${path}['"]`),
      `${component} 必须通过 lucide-svelte/icons/${path} 直接导入`,
    );
  }

  assert.doesNotMatch(source, /from\s+['"]lucide-svelte['"]/);
});

test('概览源码不得残留内联 SVG', async () => {
  const source = await readOverview();

  assert.doesNotMatch(source, /<svg\b/i);
});

test('图表视图切换必须保留双模式语义图标', async () => {
  const source = await readOverview();
  const toggleStart = source.indexOf("appUsageViewMode = appUsageViewMode === 'row' ? 'column' : 'row';");
  const toggleEnd = source.indexOf('</button>', toggleStart);

  assert.notEqual(toggleStart, -1, '未找到应用使用图表视图切换逻辑');
  assert.notEqual(toggleEnd, -1, '未找到应用使用图表视图切换按钮结尾');

  const toggleButton = source.slice(toggleStart, toggleEnd);
  assert.match(toggleButton, /appUsageViewMode === 'row'[\s\S]*<ChartNoAxesGantt\b/);
  assert.match(toggleButton, /{:else}[\s\S]*<ChartNoAxesColumn\b/);
});
