import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const cssUrl = new URL('../../app.css', import.meta.url);

test('关于页页头应使用操作轴，主体继续使用阅读内容轴语义类', async () => {
  const source = await readFile(new URL('./About.svelte', import.meta.url), 'utf8');

  assert.match(source, /class="page-header page-axis-operation"/);
  assert.match(source, /class="w-full about-minimal-shell page-axis-reading"/);
  assert.doesNotMatch(source, /about-minimal-shell[^\"]*\bmx-auto\b/);
  assert.doesNotMatch(source, /about-minimal-shell[^\"]*\bmax-w-4xl\b/);
});

test('关于页应采用极简原生居中画布，在最大化窗口自然垂直居中且去掉外框堆叠', async () => {
  const css = await readFile(cssUrl, 'utf8');

  assert.match(
    css,
    /\.about-editorial-shell\s*\{[^}]*min-height:\s*100%;[^}]*display:\s*flex;[^}]*flex-direction:\s*column;/,
    '关于页外壳应撑满可视高度并纵向布局'
  );
  assert.match(
    css,
    /\.about-minimal-shell\s*\{[^}]*max-width:\s*50rem;[^}]*margin-inline:\s*auto;[^}]*justify-content:\s*center;/,
    '关于页主体应自然水平与垂直居中，保持桌面关于页的克制尺度'
  );
  assert.match(
    css,
    /\.about-minimal-shell > \.page-card\s*\{[^}]*border:\s*0;[^}]*background:\s*transparent;[^}]*box-shadow:\s*none;/,
    '关于页应去除多重卡片外框与阴影，呈现原生无框画布'
  );
});

test('关于页品牌与更新行应保持居中紧凑排版、无套娃卡片、允许窄窗换行且保留键盘焦点', async () => {
  const [css, source] = await Promise.all([
    readFile(cssUrl, 'utf8'),
    readFile(new URL('./About.svelte', import.meta.url), 'utf8'),
  ]);

  assert.match(css, /\.about-brand-identity\s*\{[^}]*flex-direction:\s*column;[^}]*align-items:\s*center;/);
  assert.match(css, /\.about-brand-title-row\s*\{[^}]*justify-content:\s*center;/);
  assert.match(css, /^\.about-version-badge\s*\{[^}]*border-radius:\s*var\(--radius-full\);/m);
  assert.match(css, /\.about-action-row\s*\{[^}]*justify-content:\s*center;/);
  assert.match(
    css,
    /\.about-update-controls\s*\{[^}]*display:\s*flex;[^}]*flex-wrap:\s*wrap;[^}]*justify-content:\s*center;/,
  );
  assert.match(css, /\.about-auto-check input:focus-visible\s*\{[^}]*outline:/);
  assert.match(css, /\.dark \.about-auto-check,\s*\.dark \.about-update-feedback\s*\{[^}]*color:/);
  assert.match(css, /\.about-update-feedback\s*\{[^}]*overflow-wrap:\s*anywhere;/);
  assert.match(css, /\.about-trust-grid\s*\{[^}]*grid-template-columns:\s*repeat\(3,\s*minmax\(0,\s*1fr\)\);/);
  assert.match(css, /\.about-trust-card\s*\{[^}]*border:\s*0;[^}]*background:\s*transparent;/);
  assert.doesNotMatch(css, /\.about-trust-icon\b/, '原则项不应堆砌彩色图标块');
  assert.doesNotMatch(css, /\.about-trust-card\s*\+\s*\.about-trust-card\s*\{/);
  assert.match(css, /\.about-principles-card\b/);
  assert.match(css, /\.about-tech-item\b/);
  assert.match(css, /@media \(max-width:\s*760px\)\s*\{[^@]*\.about-trust-grid\s*\{[^}]*grid-template-columns:\s*1fr;/);
  assert.match(css, /@media \(max-width:\s*640px\)\s*\{[^@]*\.about-tech-stack\s*\{[^}]*justify-content:\s*center;/);
  assert.doesNotMatch(source, /style="border-color:\s*var\(--surface-border-default\);"/);
  for (const selector of ['about-update-panel', 'about-update-grid', 'about-update-unit', 'about-update-heading', 'about-update-info', 'about-update-details', 'about-channel-badge', 'about-switch']) {
    assert.doesNotMatch(css, new RegExp(`\\.${selector}\\b`), `应删除已废弃样式 ${selector}`);
  }
});

test('关于页原则分隔线与深色文字应保持低对比且清晰可读', async () => {
  const css = await readFile(cssUrl, 'utf8');

  assert.match(
    css,
    /\.about-minimal-shell > \.page-card\.about-principles-card\s*\{[^}]*border-top:\s*1px solid var\(--surface-border-subtle\);/,
  );
  assert.match(css, /\.dark \.about-trust-title\s*\{[^}]*color:\s*#e6edf3;/);
  assert.match(css, /\.dark \.about-trust-copy\s*\{[^}]*color:\s*#8b949e;/);
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
