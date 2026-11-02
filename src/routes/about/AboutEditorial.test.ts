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

test('品牌卡底部只保留自动检查与一个更新按钮，不重复版本或扩展独立面板', async () => {
  const source = await readAboutSource();
  const brandStart = source.indexOf('class="page-card about-brand-card"');
  const brandEnd = source.indexOf('</section>', brandStart);
  const brandCard = source.slice(brandStart, brandEnd);
  const controls = brandCard.slice(brandCard.indexOf('class="about-update-controls"'));

  assert.match(brandCard, /about-update-controls/);
  assert.equal((brandCard.match(/v\{appVersion\}/g) || []).length, 1, '版本只在品牌标题旁显示一次');
  assert.equal((controls.match(/<button\b/g) || []).length, 1, '更新行只保留一个按钮');
  assert.equal((controls.match(/type="checkbox"/g) || []).length, 1);
  assert.match(controls, /t\('about\.checkUpdates'\)/);
  assert.match(controls, /disabled=\{isCheckingUpdate\}/);
  assert.match(controls, /aria-busy=\{isCheckingUpdate\}/);
  assert.match(controls, /\{#if updateStatus\}[\s\S]*role="status" aria-live="polite"/);
  assert.doesNotMatch(source, /about-update-(?:panel|grid|details|info)|about-channel-|isUpdateDetailsOpen/);
  assert.ok(
    brandCard.indexOf('about-action-row') < brandCard.indexOf('about-update-controls'),
    '原有轻量操作之后只增加紧凑更新行'
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
