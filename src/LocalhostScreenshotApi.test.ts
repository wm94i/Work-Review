import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const [apiSource, examplesSource, readmeEn, readmeZh, readmeTw] = await Promise.all([
  readFile(new URL('../src-tauri/src/localhost_api.rs', import.meta.url), 'utf8'),
  readFile(
    new URL('./routes/settings/components/nodeGateway/ApiExamplesPanel.svelte', import.meta.url),
    'utf8',
  ),
  readFile(new URL('../README.md', import.meta.url), 'utf8'),
  readFile(new URL('../README.zh.md', import.meta.url), 'utf8'),
  readFile(new URL('../README.tw.md', import.meta.url), 'utf8'),
]);

test('Localhost API 应提供受保护的即时截图端点', () => {
  assert.match(apiSource, /\("POST", "\/v1\/screenshots\/capture"\)/);
  assert.match(apiSource, /handle_capture_screenshot\(state\)\.await/);
  assert.match(apiSource, /ensure_api_capture_enabled/);
  assert.match(apiSource, /spawn_blocking/);
  assert.match(apiSource, /capture_for_window_with_options\(active_window\.as_ref\(\), false\)/);
  assert.match(apiSource, /generate_full_image_base64/);
});

test('设置页和三语 README 应公开即时截图调用方式', () => {
  assert.match(examplesSource, /\/v1\/screenshots\/capture/);
  assert.match(examplesSource, /exampleCaptureScreenshotDesc/);
  assert.match(examplesSource, />20 endpoints</);

  for (const readme of [readmeEn, readmeZh, readmeTw]) {
    assert.match(readme, /POST[^\n]*`\/v1\/screenshots\/capture`/);
  }
});

test('设置页 curl 示例不得把掩码 token 当作真实鉴权值', () => {
  assert.doesNotMatch(examplesSource, /curlToken\s*=\s*localStatus\.tokenPreview/);
  assert.match(examplesSource, /AUTH_TOKEN_PLACEHOLDER\s*=\s*'<token>'/);
  assert.match(examplesSource, /if \(path !== '\/health'\)/);
});
