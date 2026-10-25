import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { REPORT_EDITOR_FONTS, reportEditorFontFamily } from './reportEditorFont.ts';

test('日报编辑默认继承界面字体，命名字体缺失时保留界面字体回退', () => {
  for (const value of [undefined, null, '', 'missing-font', 'toString', '"; color: red']) {
    assert.equal(reportEditorFontFamily(value), 'inherit');
  }
  for (const font of REPORT_EDITOR_FONTS.filter(font => font !== 'monospace')) {
    assert.equal(reportEditorFontFamily(font), `"${font}", var(--font-ui)`);
  }
  const mono = reportEditorFontFamily('monospace');
  assert.match(mono, /ui-monospace.*Consolas/);
  assert.ok(mono.indexOf('"Microsoft YaHei"') < mono.lastIndexOf('monospace'));
});

test('编辑器与外观设置共用字体配置，默认 CSS 不再强制中文走等宽回退', () => {
  const css = readFileSync(new URL('../../app.css', import.meta.url), 'utf8');
  const report = readFileSync(new URL('../../routes/report/Report.svelte', import.meta.url), 'utf8');
  const settings = readFileSync(new URL('../../routes/settings/components/SettingsAppearance.svelte', import.meta.url), 'utf8');
  assert.match(css, /\.report-edit-textarea\s*\{[^}]*font-family:\s*inherit;/);
  assert.match(css, /--font-ui:[^;]*"Microsoft YaHei", sans-serif;/);
  assert.match(report, /style:font-family=\{reportEditorFontFamily\(config\?\.report_editor_font\)\}/);
  assert.match(settings, /config\.report_editor_font = event\.currentTarget\.value;\s*dispatch\('change', config\)/);
  assert.match(settings, /label for="report-editor-font"/);
  assert.match(settings, /#each REPORT_EDITOR_FONTS as font/);
});
