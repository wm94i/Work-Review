import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('全局样式应提供 Magpie 风格统一强调色、分段轨道、保存轻反馈与折叠动效令牌', async () => {
  const [css, tailwind, overview, collapsible, storage] = await Promise.all([
    readFile(new URL('./app.css', import.meta.url), 'utf8'),
    readFile(new URL('../tailwind.config.ts', import.meta.url), 'utf8'),
    readFile(new URL('./routes/Overview.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./lib/components/CollapsibleSection.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/settings/components/SettingsStorage.svelte', import.meta.url), 'utf8'),
  ]);

  // 1. 统一令牌与暗色 WCAG AA 对比度
  assert.match(css, /--ui-accent:\s*#4f46e5;/);
  assert.match(css, /--ui-seg-track:/);
  assert.match(css, /--ui-seg-thumb:/);
  assert.match(css, /--ui-seg-edge:/);
  assert.match(css, /\.dark\s*\{[^}]*--ui-muted:\s*#9c9ca6;/);

  // 2. Windows 清晰等宽字体栈（避免回退宋体）
  assert.match(tailwind, /Cascadia Mono[^\]]*Microsoft YaHei UI/);
  assert.match(css, /--font-mono:[^;]*Cascadia Mono[^;]*Microsoft YaHei UI/);

  // 3. 概览分段轨道与平滑折叠区（首次展开前懒挂载，收起态不可聚焦）
  assert.match(overview, /class="ui-segmented-track"/);
  assert.match(css, /\.ui-segmented-track\s*\{/);
  assert.match(collapsible, /class="ui-fold \{open \? 'ui-fold-open' : ''\}"/);
  assert.match(collapsible, /let everOpened = open;/);
  assert.match(collapsible, /inert=\{!open \? true : undefined\}/);
  assert.match(css, /\.ui-fold\s*\{[^}]*grid-template-rows:\s*0fr;/);

  // 4. 轮询间隔自动保存行内轻反馈（双轨重播 + 卸载清理定时器）
  assert.match(storage, /POLLING_SAVE_FLASH_MS = 1200/);
  assert.match(storage, /pollingSaveFlashPhase === 1 \? 'ui-save-flash-a' : pollingSaveFlashPhase === 2 \? 'ui-save-flash-b' : ''/);
  assert.match(storage, /onDestroy\(\(\) => \{[^}]*clearPollingFlashTimer\(\)/);
  assert.match(css, /@keyframes ui-save-flash-a/);
  assert.match(css, /@keyframes ui-save-flash-b/);

  // 5. 助手与暗色弹窗统一走共享令牌，不再使用割裂色系
  assert.match(css, /\.ask-workbench-shell\s*\{[^}]*--ask-accent:\s*var\(--ui-accent\);/);
  assert.match(css, /\.dark \.modal-panel\s*\{[^}]*background:\s*var\(--editorial-surface-featured\);/);
});

test('设置页大块单选与分段条应改为结构化选项卡与凹槽轨道，且全应用统一全宽自适应并支持设置页连续滚动跟随', async () => {
  const [css, settings, ai, storage, general, appearance, privacy, overview, timeline, about] = await Promise.all([
    readFile(new URL('./app.css', import.meta.url), 'utf8'),
    readFile(new URL('./routes/settings/Settings.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/settings/components/SettingsAI.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/settings/components/SettingsStorage.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/settings/components/SettingsGeneral.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/settings/components/SettingsPrivacy.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/Overview.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/timeline/Timeline.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./routes/about/About.svelte', import.meta.url), 'utf8'),
  ]);

  // 1. 基础模板/AI 增强、截图范围、分辨率模式改用结构化单选卡
  assert.match(css, /\.ui-choice-grid\s*\{[^}]*grid-template-columns:\s*repeat\(2,\s*minmax\(0,\s*1fr\)\);[^}]*max-width:\s*54rem;/);
  assert.match(css, /\.ui-choice-card-active\s*\{[^}]*border-color:\s*var\(--ui-accent\);/);
  assert.match(ai, /class="ui-choice-grid"/);
  assert.match(ai, /ui-choice-card-active/);
  assert.equal((storage.match(/class="ui-choice-grid"/g) || []).length, 2);

  // 2. AI 三入口与其余单行分段条改用凹槽轨道，且 AI 服务商网格适配超宽全屏并保留 aria-pressed
  assert.match(ai, /settings-ai-section-tabs/);
  assert.match(ai, /lg:grid-cols-4\s+xl:grid-cols-5\s+2xl:grid-cols-6/);
  assert.match(ai, /aria-pressed=\{active\}/);
  assert.match(general, /ui-seg-group/);
  assert.match(appearance, /ui-seg-group/);
  assert.match(storage, /ui-seg-group/);
  assert.match(privacy, /class="settings-privacy-strategy-segments"/);

  // 3. 全应用统一全宽自适应（无窄页级居中限宽条），由内部响应式网格吸收宽屏空间
  assert.match(css, /\.overview-page-shell\s*\{[^}]*width:\s*100%;/);
  assert.doesNotMatch(css, /\.overview-page-shell\s*\{[^}]*max-width:/);
  assert.match(overview, /class="page-shell overview-page-shell"/);
  assert.doesNotMatch(css, /\.report-editorial-shell\s*\{[^}]*max-width:/);
  assert.match(css, /@media \(min-width:\s*1280px\)\s*\{\s*\.report-reading-layout\s*\{[^}]*13rem;/);
  assert.doesNotMatch(css, /\.settings-editorial-board\s*\{[^}]*max-width:/);
  assert.doesNotMatch(css, /\.ask-workbench-shell > \.page-header\s*\{[^}]*max-width:/);
  assert.doesNotMatch(timeline, /\.page-shell\s*\{[^}]*max-width:\s*82rem;/);
  assert.match(timeline, /@media \(min-width:\s*1280px\)\s*\{[^@]*\.timeline-entry-card-compact-grid\s*\{[^}]*grid-template-areas:\s*'app title meta';/);
  assert.match(css, /\.about-minimal-shell\s*\{[^}]*max-width:\s*50rem;[^}]*justify-content:\s*center;/);
  assert.match(css, /\.about-minimal-shell > \.page-card\s*\{[^}]*border:\s*0;[^}]*background:\s*transparent;/);
  assert.match(about, /class="about-brand-icon"/);
  assert.match(about, /about-update-check-btn/);

  // 4. 设置页连续滚动分区 + 粘性导航 Scroll-Spy + 点击平滑定位
  assert.match(css, /\.settings-tab-rail\s*\{[^}]*position:\s*sticky;[^}]*max-height:\s*calc\(100vh - 5\.5rem\);/);
  assert.match(css, /\.settings-scroll-section\s*\{[^}]*scroll-margin-top:/);
  assert.match(settings, /import \{ resolveActiveSettingsSection \} from '\.\/settingsScrollSpy\.ts';/);
  assert.match(settings, /use:attachScrollSpy/);
  assert.match(settings, /on:click=\{\(\) => scrollToSection\(tab\.id\)\}/);
  assert.equal((settings.match(/data-settings-section="[a-z]+"/g) || []).length, 8);

  // 5. range 滑块竖直居中
  assert.match(css, /\.range-input::-webkit-slider-runnable-track\s*\{[^}]*height:\s*0\.375rem;/);
  assert.match(css, /\.range-input::-webkit-slider-thumb\s*\{[^}]*margin-top:\s*-0\.3125rem;/);
});
