import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const read = (path: string) => readFile(new URL(path, import.meta.url), 'utf8');

test('Evidence 导航应直接提供 system、light、dark 三种主题选择并复用 App 保存逻辑', async () => {
  const [app, navigation] = await Promise.all([
    read('./App.svelte'),
    read('./lib/components/evidence/EvidenceNavigation.svelte'),
  ]);

  assert.match(navigation, /export let theme(?:: Theme)? = 'system';/);
  assert.match(navigation, /themeChange:\s*Theme/);
  assert.match(navigation, /const themeOptions[\s\S]*'system'[\s\S]*'light'[\s\S]*'dark'/);
  assert.match(navigation, /role="group"/);
  assert.match(navigation, /aria-label=\{t\('evidenceTemplate\.themePicker'\)\}/);
  assert.match(navigation, /aria-pressed=\{theme === option\.value\}/);
  assert.match(navigation, /dispatch\('themeChange', option\.value\)/);
  assert.match(navigation, /Monitor/);
  assert.match(navigation, /Sun/);
  assert.match(navigation, /Moon/);
  assert.match(navigation, /@media \(prefers-reduced-motion: reduce\)/);

  assert.match(
    app,
    /<EvidenceNavigation[\s\S]*\{theme\}[\s\S]*on:themeChange=\{handleThemeChange\}/,
  );
});

test('Evidence 主题选择器文案应覆盖四种语言', async () => {
  const localeSources = await Promise.all([
    read('./lib/i18n/locales/zh-CN.ts'),
    read('./lib/i18n/locales/zh-TW.ts'),
    read('./lib/i18n/locales/en.ts'),
    read('./lib/i18n/locales/ar.ts'),
  ]);

  for (const source of localeSources) {
    assert.match(source, /evidenceTemplate:\s*\{[\s\S]*themePicker:/);
    assert.match(source, /themeTitle:\s*\{[\s\S]*system:[\s\S]*light:[\s\S]*dark:/);
  }
});
