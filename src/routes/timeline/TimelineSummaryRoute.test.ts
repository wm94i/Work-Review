import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('应用应暂时保留两个旧摘要路由以兼容历史深链接', async () => {
  const appSource = await readFile(new URL('../../App.svelte', import.meta.url), 'utf8');

  assert.match(appSource, /'\/timeline\/summary\/:date'/);
  assert.match(appSource, /'\/timeline\/summary'/);
});

test('带日期和不带日期的旧摘要地址都应 replace 到时间线抽屉参数', async () => {
  const source = await readFile(new URL('./Summary.svelte', import.meta.url), 'utf8');

  assert.match(source, /const routeParams = router\.params/);
  assert.match(source, /const routeDate = match\?\.\[1\] \?\? paramDate/);
  assert.match(source, /const date = isValidLocalDateString\(routeDate\) \? routeDate : getLocalDateString\(\)/);
  assert.match(source, /const target = `\/timeline\?date=\$\{date\}&summary=1`/);
  assert.match(source, /replace\(target\)/);
});

test('时间线消费旧摘要打开参数后应立即清除 summary 标记，避免刷新再次打开', async () => {
  const source = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');

  assert.match(source, /function consumeRequestedSummaryOpen\(\)/);
  assert.match(source, /params\.delete\('summary'\)/);
  assert.match(source, /window\.history\.replaceState/);
  assert.match(source, /if \(readRequestedSummaryOpen\(\)\) \{[\s\S]*showSummaryDrawer = true;[\s\S]*consumeRequestedSummaryOpen\(\)/);
});

test('旧摘要路由与时间线参数应共用严格的真实日历日期校验', async () => {
  const summarySource = await readFile(new URL('./Summary.svelte', import.meta.url), 'utf8');
  const timelineSource = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');

  assert.match(summarySource, /import \{ isValidLocalDateString \} from '\$lib\/utils\/dateValidation\.ts'/);
  assert.match(summarySource, /const date = isValidLocalDateString\(routeDate\) \? routeDate : getLocalDateString\(\)/);
  assert.match(timelineSource, /import \{ isValidLocalDateString \} from '\$lib\/utils\/dateValidation\.ts'/);
  assert.match(timelineSource, /nextDate && isValidLocalDateString\(nextDate\) \? nextDate : null/);
  assert.match(timelineSource, /isValidLocalDateString\(payload\.date\)/);
});
