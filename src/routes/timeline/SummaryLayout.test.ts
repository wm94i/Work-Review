import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('旧时段摘要页面应收敛为兼容跳转，不再维护独立摘要布局', async () => {
  const source = await readFile(new URL('./Summary.svelte', import.meta.url), 'utf8');

  assert.match(source, /import \{ replace, router \} from 'svelte-spa-router'/);
  assert.match(source, /onMount/);
  assert.match(source, /summary=1/);
  assert.match(source, /replace\(target\)/);
  assert.doesNotMatch(source, /summary-editorial-shell/);
  assert.doesNotMatch(source, /LocalizedDatePicker/);
  assert.doesNotMatch(source, /get_hourly_summaries/);
});

test('旧时段摘要页面跳转期间应提供轻量加载状态', async () => {
  const source = await readFile(new URL('./Summary.svelte', import.meta.url), 'utf8');

  assert.match(source, /summary-route-redirect/);
  assert.match(source, /aria-live="polite"/);
  assert.match(source, /timelineSummary\.title/);
});
