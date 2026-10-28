import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('侧边栏应提供编辑部导航框架', async () => {
  const [source, appCssSource] = await Promise.all([
    readFile(new URL('./Sidebar.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../../app.css', import.meta.url), 'utf8'),
  ]);

  assert.match(source, /sidebar-editorial-shell/);
  assert.match(source, /sidebar-nav-section/);
  assert.match(source, /sidebar-brand-panel/);
  assert.match(source, /sidebar-status-panel/);
  assert.match(source, /sidebar-recording-copy/);
  assert.match(source, /sidebar-recording-toggle/);
  assert.match(source, /sidebar-recording-toggle-label/);
  assert.match(source, /sidebar-recording-toggle-icon/);
  assert.match(source, /sidebar-locale-switch/);
  assert.match(source, /sidebar-locale-compact-icon/);
  assert.match(source, /aria-label=\{isPaused \? translate\('sidebar\.resume'\) : translate\('sidebar\.pause'\)\}/);
  assert.match(source, /aria-label=\{translate\(item\.labelKey\)\}/);
  assert.match(source, /aria-current=\{activeStates\[item\.path\] \? 'page' : undefined\}/);
  assert.match(source, /sidebar-toolbelt/);
  assert.doesNotMatch(source, /sidebar-brand-chip/);
  assert.doesNotMatch(source, /sidebar-nav-index/);
  assert.match(appCssSource, /\.sidebar-editorial-shell\s*\{[\s\S]*background:\s*transparent;/);
  assert.match(appCssSource, /\.sidebar-brand-panel\s*\{[\s\S]*background:\s*transparent;/);
  assert.match(appCssSource, /\.sidebar-brand-panel\s*\{[\s\S]*border:\s*none;/);
  assert.match(appCssSource, /\.sidebar-status-panel\s*\{[\s\S]*background:\s*transparent;/);
  assert.match(appCssSource, /\.sidebar-status-panel\s*\{[\s\S]*box-shadow:\s*none;/);
  assert.match(appCssSource, /\.sidebar-recording-toggle-icon,\s*\.sidebar-locale-compact-icon\s*\{[\s\S]*display:\s*none/);
  assert.match(appCssSource, /\.sidebar-nav-section\s*\{[\s\S]*background:\s*transparent;/);
  assert.match(appCssSource, /\.sidebar-nav-section\s*\{[\s\S]*border:\s*none;/);
  assert.match(appCssSource, /\.sidebar-toolbelt\s*\{[\s\S]*background:\s*transparent;/);
  assert.match(appCssSource, /\.sidebar-toolbelt\s*\{[\s\S]*border:\s*none;/);
  assert.match(appCssSource, /@media \(max-width: 700px\)[\s\S]*\.app-shell \.sidebar-recording-copy\s*\{[\s\S]*display:\s*none/);
  assert.match(appCssSource, /@media \(max-width: 700px\)[\s\S]*\.app-shell \.sidebar-recording-toggle\s*\{[\s\S]*width:\s*100%/);
  assert.match(appCssSource, /@media \(max-width: 700px\)[\s\S]*\.app-shell \.sidebar-recording-toggle-label\s*\{[\s\S]*display:\s*none/);
  assert.match(appCssSource, /@media \(max-width: 700px\)[\s\S]*\.app-shell \.sidebar-recording-toggle-icon\s*\{[\s\S]*display:\s*block/);
  assert.match(appCssSource, /@media \(max-width: 700px\)[\s\S]*\.app-shell \.sidebar-locale-label,[\s\S]*display:\s*none/);
  assert.match(appCssSource, /@media \(max-width: 700px\)[\s\S]*\.app-shell \.sidebar-locale-compact-icon\s*\{[\s\S]*display:\s*block/);
  assert.match(appCssSource, /@media \(max-width: 700px\)[\s\S]*\.app-shell \.sidebar-footer\s*\{[\s\S]*flex-direction:\s*column/);
});

test('侧边栏品牌区不再渲染副标题装饰文字', async () => {
  // 副标题"记录 · 分析 · 证明"已删除以精简界面，此测试守卫不被无意加回
  const source = await readFile(new URL('./Sidebar.svelte', import.meta.url), 'utf8');

  assert.doesNotMatch(source, /sidebar-brand-line/);
  assert.doesNotMatch(source, /sidebar-brand-segment/);
  assert.doesNotMatch(source, /sidebar\.tagline/);
});

test('侧边栏激活态仅使用背景高亮，不应渲染额外竖条', async () => {
  const [source, appCssSource] = await Promise.all([
    readFile(new URL('./Sidebar.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../../app.css', import.meta.url), 'utf8'),
  ]);

  assert.doesNotMatch(source, /sidebar-nav-rail/);
  assert.doesNotMatch(appCssSource, /\.sidebar-nav-rail\s*\{/);
});

test('侧边栏不应继续提供独立的设备节点入口，节点能力应收回设置页 Beta 标签', async () => {
  const source = await readFile(new URL('./Sidebar.svelte', import.meta.url), 'utf8');

  assert.doesNotMatch(source, /path:\s*'\/node'/);
  assert.doesNotMatch(source, /labelKey:\s*'sidebar\.nav\.node'/);
  assert.doesNotMatch(source, /item\.icon === 'node'/);
});
