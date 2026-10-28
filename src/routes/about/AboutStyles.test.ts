import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const cssUrl = new URL('../../app.css', import.meta.url);

test('关于页页头应使用操作轴，主体继续使用阅读内容轴', async () => {
  const source = await readFile(new URL('./About.svelte', import.meta.url), 'utf8');

  assert.match(source, /class="page-header page-axis-operation"/);
  assert.match(source, /class="w-full about-minimal-shell page-axis-reading"/);
  assert.doesNotMatch(source, /about-minimal-shell[^\"]*\bmx-auto\b/);
  assert.doesNotMatch(source, /about-minimal-shell[^\"]*\bmax-w-4xl\b/);
});

test('关于页主体应在窗口拉高时自适应垂直居中而非堆积底部空白', async () => {
  const css = await readFile(cssUrl, 'utf8');

  assert.match(
    css,
    /\.about-editorial-shell\s*\{[^}]*min-height:\s*100%;[^}]*display:\s*flex;[^}]*flex-direction:\s*column;/,
    '关于页外壳应撑满可视高度并纵向布局'
  );
  assert.match(
    css,
    /\.about-minimal-shell\s*\{[^}]*flex:\s*1 1 auto;[^}]*justify-content:\s*center;/,
    '主体应占据页头下方剩余空间并垂直居中'
  );
});

test('关于页新结构应保持居中、三列更新与留白分组', async () => {
  const css = await readFile(cssUrl, 'utf8');
  const source = await readFile(new URL('./About.svelte', import.meta.url), 'utf8');

  assert.match(css, /\.about-brand-identity\s*\{[\s\S]*?align-items:\s*center;/);
  assert.match(css, /\.about-brand-title-row\s*\{[\s\S]*?justify-content:\s*center;/);
  assert.match(css, /^\.about-version-badge\s*\{[\s\S]*?border-radius:\s*var\(--radius-full\);/m);
  assert.match(css, /\.about-update-panel\s*\{[\s\S]*?border-radius:\s*var\(--radius-lg\);/);
  assert.match(css, /\.about-update-grid\s*\{[\s\S]*?grid-template-columns:\s*minmax\(7rem,\s*0\.72fr\)\s+minmax\(0,\s*1\.65fr\)\s+auto;/);
  assert.match(css, /\.about-update-info\s*\{[\s\S]*?border-radius:\s*var\(--radius-full\);/);
  assert.match(css, /\.about-trust-grid\s*\{[\s\S]*?gap:\s*1rem;/);
  assert.doesNotMatch(css, /\.about-update-unit\s*\+\s*\.about-update-unit\s*\{/);
  assert.doesNotMatch(css, /\.about-trust-card\s*\+\s*\.about-trust-card\s*\{/);
  assert.match(css, /\.about-principles-card\b/);
  assert.match(css, /\.about-tech-item\b/);
  assert.match(css, /@media \(max-width:\s*640px\)[\s\S]*?\.about-update-grid\s*\{[\s\S]*?grid-template-columns:\s*1fr;/);
  assert.match(css, /@media \(max-width:\s*640px\)[\s\S]*?\.about-tech-stack\s*\{[\s\S]*?justify-content:\s*center;/);
  assert.match(source, /about-update-panel/);
  assert.match(source, /about-update-details/);
  assert.match(source, /aria-expanded=\{isUpdateDetailsOpen\}/);
});

test('关于页新卡片的深色边界应保持低对比', async () => {
  const [css, source] = await Promise.all([
    readFile(cssUrl, 'utf8'),
    readFile(new URL('./About.svelte', import.meta.url), 'utf8'),
  ]);

  assert.match(css, /\.about-principles-card\s*\{[^}]*border-color:\s*var\(--surface-border-default\)/);
  assert.match(css, /\.about-tech-stack\s*\{[^}]*border-color:\s*var\(--surface-border-default\)/);
  assert.match(css, /\.dark \.about-principles-card\s*\{[^}]*border-color:\s*var\(--surface-border-default\)[^}]*box-shadow:\s*none/);
  assert.match(css, /\.dark \.about-tech-stack\s*\{[^}]*border-color:\s*var\(--surface-border-default\)[^}]*box-shadow:\s*none/);
  assert.doesNotMatch(source, /style="border-color:\s*var\(--surface-border-default\);"/);
});

test('关于页产品原则在深色模式下应保持标题与说明可读', async () => {
  const css = await readFile(cssUrl, 'utf8');

  assert.match(css, /\.dark \.about-trust-title\s*\{[\s\S]*?color:\s*#e6edf3;/);
  assert.match(css, /\.dark \.about-trust-copy\s*\{[\s\S]*?color:\s*#8b949e;/);
});

test('关于页不应残留旧操作条、技术胶囊和更新横幅样式', async () => {
  const css = await readFile(cssUrl, 'utf8');

  assert.doesNotMatch(css, /^\.about-action-strip\b/m);
  assert.doesNotMatch(css, /^\.about-brand-head\b/m);
  assert.doesNotMatch(css, /^\.about-tech-pill(?:\b|-)/m);
  assert.doesNotMatch(css, /^\.about-update-banner\b/m);
  assert.doesNotMatch(css, /^\.dark \.about-tech-pill(?:\b|-)/m);
  assert.doesNotMatch(css, /^\.dark \.about-update-banner\b/m);
});
