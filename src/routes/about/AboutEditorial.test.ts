import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

async function readAboutSource() {
  return readFile(new URL('./About.svelte', import.meta.url), 'utf8');
}

test('关于页应保留左对齐标准页头，并将品牌主体居中组织', async () => {
  const source = await readAboutSource();

  assert.match(source, /class="page-header page-axis-operation"/);
  assert.match(source, /class="page-title-group"/);
  assert.match(source, /class="page-title-badge"/);
  assert.match(source, /class="page-title-copy"/);
  assert.match(source, /t\('about\.pageTitle'\)/);
  assert.match(source, /t\('about\.pageSubtitle'\)/);
  assert.match(source, /about-editorial-shell/);
  assert.match(source, /about-minimal-shell/);
  assert.match(source, /about-brand-card/);
  assert.match(source, /about-brand-identity/);
  assert.match(source, /about-brand-title-row/);
  assert.match(source, /about-version-badge/);
  assert.match(source, /about-brand-title-row[\s\S]*?v\{appVersion\}[\s\S]*?<\/div>/);
  assert.match(source, /about-action-row/);
});

test('品牌卡底部应按当前版本、自动检查更新和更新状态分为三列', async () => {
  const source = await readAboutSource();
  const brandStart = source.indexOf('class="page-card about-brand-card"');
  const brandEnd = source.indexOf('</section>', brandStart);
  const brandCard = source.slice(brandStart, brandEnd);

  assert.match(brandCard, /about-update-panel/);
  assert.match(brandCard, /about-update-info/);
  assert.match(brandCard, /about-update-details/);
  assert.match(brandCard, /aria-controls="about-update-details"/);
  assert.match(brandCard, /about-update-grid/);
  assert.equal(
    (brandCard.match(/class="about-update-unit(?:\s|\")/g) || []).length,
    3,
    '更新区域应准确包含三列'
  );
  assert.match(brandCard, /v\{appVersion\}/);
  assert.match(brandCard, /role="switch"/);
  assert.match(brandCard, /t\('about\.checkUpdates'\)/);
  assert.match(brandCard, /\{#if updateStatus\}[\s\S]*about-update-feedback/);
  assert.ok(
    brandCard.indexOf('about-action-row') < brandCard.indexOf('about-update-grid'),
    '三个轻量操作应位于更新状态区之前'
  );
  assert.doesNotMatch(source.slice(brandEnd), /about-update-banner/);
});

test('产品原则应使用统一外层卡片并保持三项居中内容', async () => {
  const source = await readAboutSource();

  assert.match(source, /class="page-card about-principles-card"/);
  assert.match(source, /about-principles-title/);
  assert.match(source, /t\('about\.productPrinciplesTitle'\)/);
  assert.match(source, /about-trust-grid/);
  assert.equal(
    (source.match(/class="about-trust-card"/g) || []).length,
    3,
    '产品原则应保留三项'
  );
  assert.doesNotMatch(source, /class="page-card about-trust-card"/);
});

test('技术栈应使用低对比居中行，不再使用胶囊结构', async () => {
  const source = await readAboutSource();

  assert.match(source, /class="page-card about-tech-stack"/);
  assert.equal(
    (source.match(/class="about-tech-item"/g) || []).length,
    4,
    '技术栈应展示四个轻量条目'
  );
  assert.doesNotMatch(source, /about-tech-pill/);
  assert.doesNotMatch(source, /about-action-strip/);
  assert.doesNotMatch(source, /about-system-note/);
  assert.doesNotMatch(source, /about-system-grid/);
  assert.doesNotMatch(source, /about-linux-pills/);
});
