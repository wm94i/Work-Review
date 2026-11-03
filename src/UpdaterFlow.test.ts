import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';

async function readCommandsSource() {
  // commands.rs 已按领域拆分为 commands/*.rs，这里拼接所有子模块以保持断言语义不变。
  const dir = new URL('../src-tauri/src/commands/', import.meta.url);
  const files = (await readdir(dir)).filter((f) => f.endsWith('.rs'));
  const parts = await Promise.all(files.map((f) => readFile(new URL(f, dir), 'utf8')));
  return parts.join('\n');
}

test('后端检查更新应优先验证当前平台存在可安装更新包', async () => {
  const source = await readCommandsSource();

  assert.match(source, /check_installable_update/);
  assert.match(source, /\.updater_builder\(\)/);
  assert.match(source, /match updater\.check\(\)\.await/);
  assert.match(source, /auto_update_ready: true/);
});

test('安装前必须先验证更新签名及独立的官方资产摘要，不能直接下载安装', async () => {
  const source = await readFile(new URL('../src-tauri/src/commands/updater.rs', import.meta.url), 'utf8');
  const install = source.slice(source.indexOf('pub async fn download_and_install_github_update('), source.indexOf('#[cfg(test)]'));
  assert.match(install, /https:\/\/api\.github\.com\/repos\/wm94i\/Work-Review\/releases\/tags\/v/);
  assert.ok(install.indexOf('let trusted_release') < install.indexOf('for endpoint in UPDATER_JSON_ENDPOINTS'));
  assert.ok(install.indexOf('trusted_asset_digest(') < install.indexOf('.download('));
  assert.ok(install.indexOf('.download(') < install.indexOf('verify_asset_digest('));
  const installPosition = install.search(/update\s*\.install\(bytes\)/);
  assert.ok(installPosition > install.indexOf('verify_asset_digest('));
  assert.doesNotMatch(install, /\.download_and_install\(/);
});
