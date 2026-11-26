import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  getIsoWeekKey,
  shiftIsoWeekKey,
  getMonthKey,
  shiftMonthKey,
} from './reportPeriodNavigation.ts';

test('getIsoWeekKey 应正确处理 53 周跨年与元旦边界', () => {
  assert.equal(getIsoWeekKey(new Date(2020, 11, 31)), '2020-W53');
  assert.equal(getIsoWeekKey(new Date(2021, 0, 1)), '2020-W53');
  assert.equal(getIsoWeekKey(new Date(2021, 0, 4)), '2021-W01');
  assert.equal(getIsoWeekKey(new Date(2025, 11, 29)), '2026-W01');
  assert.equal(getIsoWeekKey(new Date(2026, 0, 1)), '2026-W01');
});

test('shiftIsoWeekKey 应安全跨越年末 52/53 周与夏令时切换日', () => {
  assert.equal(shiftIsoWeekKey('2026-W01', -1), '2025-W52');
  assert.equal(shiftIsoWeekKey('2025-W52', 1), '2026-W01');
  assert.equal(shiftIsoWeekKey('2020-W53', 1), '2021-W01');
  assert.equal(shiftIsoWeekKey('2021-W01', -1), '2020-W53');
  // 春季与秋季常见夏令时切换周前后步进不漂移
  assert.equal(shiftIsoWeekKey('2026-W10', 1), '2026-W11');
  assert.equal(shiftIsoWeekKey('2026-W44', 1), '2026-W45');
});

test('getMonthKey 与 shiftMonthKey 应正确处理跨年回绕与闰年二月', () => {
  assert.equal(getMonthKey(new Date(2026, 0, 31)), '2026-01');
  assert.equal(getMonthKey(new Date(2024, 1, 29)), '2024-02');
  assert.equal(shiftMonthKey('2026-01', -1), '2025-12');
  assert.equal(shiftMonthKey('2025-12', 1), '2026-01');
  assert.equal(shiftMonthKey('2024-03', -1), '2024-02');
  assert.equal(shiftMonthKey('2026-05', -12), '2025-05');
});

test('Report 周期加载与生成切换竞态应立即释放旧状态并同步 hash', () => {
  const reportSrc = readFileSync(new URL('./Report.svelte', import.meta.url), 'utf8');
  const loadBlock = reportSrc.slice(
    reportSrc.indexOf('async function loadPeriodReview'),
    reportSrc.indexOf('async function generatePeriodReview'),
  );
  const genBlock = reportSrc.slice(
    reportSrc.indexOf('async function generatePeriodReview'),
    reportSrc.indexOf('async function copyPeriodReviewContent'),
  );
  assert.match(loadBlock, /periodGenerating\s*=\s*false/);
  assert.match(genBlock, /periodLoading\s*=\s*false/);
  assert.match(reportSrc, /window\.history\.replaceState\(null, '', targetHash\)/);
});

test('Overview currentLocalDate 仅在日期变化时赋值以防止时钟空转触发统计 IPC', () => {
  const overviewSrc = readFileSync(new URL('../Overview.svelte', import.meta.url), 'utf8');
  assert.match(overviewSrc, /if \(nextLocalDate !== currentLocalDate\)/);
  assert.match(overviewSrc, /activeOverviewRangeKey/);
});
