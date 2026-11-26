import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('应用壳层应使用统一 i18n 资源并支持三种界面语言', async () => {
  const [appSource, i18nSource] = await Promise.all([
    readFile(new URL('./App.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./lib/i18n/index.ts', import.meta.url), 'utf8'),
  ]);

  assert.match(i18nSource, /'zh-CN'/);
  assert.match(i18nSource, /'en'/);
  assert.match(i18nSource, /'zh-TW'/);
  assert.match(i18nSource, /'ar'/);
  assert.match(i18nSource, /export function cycleLocale\(\)/);
  assert.match(appSource, /applyLocaleToDocument/);
  assert.match(appSource, /initializeLocale/);
  assert.match(appSource, /\{#key currentLocale\}/);
  assert.match(appSource, /<Router \{routes\} \/>/);
});

test('侧边栏底部应提供语言切换菜单，并按 ZH、EN、TW 顺序展示缩写标识', async () => {
  const source = await readFile(new URL('./lib/components/Sidebar.svelte', import.meta.url), 'utf8');

  assert.match(source, /locale-switch/);
  assert.match(source, /aria-haspopup="menu"/);
  assert.match(source, /toggleLocaleMenu/);
  assert.match(source, /selectLocale\(option\.value\)/);
  assert.match(source, /value: 'zh-CN', label: 'ZH'/);
  assert.match(source, /value: 'en', label: 'EN'/);
  assert.match(source, /value: 'zh-TW', label: 'TW'/);
  assert.match(source, /value: 'ar', label: 'AR'/);
  assert.ok(
    source.indexOf("value: 'zh-CN', label: 'ZH'")
      < source.indexOf("value: 'en', label: 'EN'")
      && source.indexOf("value: 'en', label: 'EN'")
      < source.indexOf("value: 'zh-TW', label: 'TW'")
      && source.indexOf("value: 'zh-TW', label: 'TW'")
      < source.indexOf("value: 'ar', label: 'AR'"),
    '语言顺序应为 ZH、EN、TW、AR'
  );
  assert.match(source, /emitTo\('avatar', 'locale-changed', normalizedLocale\)/);
  assert.doesNotMatch(source, /sidebar-footer-version/);
});

test('后端语言切换命令应保持 crate 内可见以避免 Tauri command 宏重导出冲突', async () => {
  const [traySource, handlerSource] = await Promise.all([
    readFile(new URL('../src-tauri/src/tray.rs', import.meta.url), 'utf8'),
    readFile(new URL('../src-tauri/src/commands/mod.rs', import.meta.url), 'utf8'),
  ]);

  assert.match(traySource, /#\[tauri::command\]\s*pub\(crate\) async fn set_app_locale\b/);
  assert.doesNotMatch(traySource, /#\[tauri::command\]\s*pub async fn set_app_locale\b/);
  assert.match(handlerSource, /crate::tray::set_app_locale/);
});

test('语言菜单应左对齐展开，并按“英文缩写 + 语言名称”展示避免裁切', async () => {
  const source = await readFile(new URL('./lib/components/Sidebar.svelte', import.meta.url), 'utf8');

  assert.match(source, /class="absolute bottom-full start-0 mb-2/);
  assert.match(source, /min-w-\[148px\]/);
  assert.match(source, /whitespace-nowrap/);
  assert.match(source, /fullLabelKey: 'sidebar\.localeNames\.zhCN'/);
  assert.match(source, /fullLabel: translate\(option\.fullLabelKey\)/);
  assert.match(
    source,
    /<span class="font-semibold tracking-\[0\.08em\] text-slate-500 dark:text-\[#7d8590\]">\{option\.label\}<\/span>\s*<span class="text-slate-700 dark:text-\[#c9d1d9\]">\{option\.fullLabel\}<\/span>/s
  );
});

test('桌宠窗口应初始化 locale 并监听语言切换事件', async () => {
  const source = await readFile(new URL('./routes/avatar/AvatarWindow.svelte', import.meta.url), 'utf8');

  assert.match(source, /initializeLocale\(\)/);
  assert.match(source, /applyLocaleToDocument/);
  assert.match(source, /listen\('locale-changed'/);
  assert.match(
    source,
    /getAvatarStateBubble\(\s*nextState\.mode,\s*currentLocale,\s*nextState\.contextLabel,\s*nextState\.avatarPersona,?\s*\)/s
  );
  assert.match(
    source,
    /nextState\.mode !== state\.mode\s*\|\|\s*nextState\.contextLabel !== state\.contextLabel/
  );
});
