import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const script = fileURLToPath(new URL('./prepare-release-version.mjs', import.meta.url));

function fixture(t: TestContext, version = '1.1.4', crlf = false) {
  const root = mkdtempSync(join(tmpdir(), 'work-review-release-version-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'src-tauri'));
  const json = (value: unknown) => `${JSON.stringify(value, null, 2)}\n`;
  const files: Record<string, string> = {
    'package.json': json({ name: 'work-review', version }),
    'package-lock.json': json({ version, packages: { '': { version }, dependency: { version: '9.0.0' } } }),
    'src-tauri/tauri.conf.json': json({ version, plugins: {} }),
    'src-tauri/Cargo.toml': `[package]\nname = "work-review"\nversion = "${version}"\n`,
    'Cargo.lock': `[[package]]\nname = "work-review-core"\nversion = "0.1.0"\n\n[[package]]\nname = "work-review"\nversion = "${version}"\n`,
  };
  for (const [path, content] of Object.entries(files)) {
    files[path] = crlf ? content.replaceAll('\n', '\r\n') : content;
    writeFileSync(join(root, path), files[path], 'utf8');
  }
  return { root, files };
}

function run(root: string, tag: string) {
  const result = spawnSync(process.execPath, [script, tag], { cwd: root, encoding: 'utf8', timeout: 10_000 });
  assert.ifError(result.error);
  return result;
}

function assertUnchanged(root: string, files: Record<string, string>) {
  for (const [path, content] of Object.entries(files)) {
    assert.equal(readFileSync(join(root, path), 'utf8'), content, path);
  }
}

test('RC 构建应将五个版本源和 package-lock 根包同步为完整 RC 版本', (t) => {
  for (const crlf of [false, true]) {
    const { root } = fixture(t, '1.1.4', crlf);
    const result = run(root, 'v1.1.4-rc.10');
    assert.equal(result.status, 0, result.stderr);
    for (const path of ['package.json', 'src-tauri/tauri.conf.json']) {
      assert.equal(JSON.parse(readFileSync(join(root, path), 'utf8')).version, '1.1.4-rc.10');
    }
    const lock = JSON.parse(readFileSync(join(root, 'package-lock.json'), 'utf8'));
    assert.equal(lock.version, '1.1.4-rc.10');
    assert.equal(lock.packages[''].version, '1.1.4-rc.10');
    assert.equal(lock.packages.dependency.version, '9.0.0');
    assert.ok(readFileSync(join(root, 'src-tauri/Cargo.toml'), 'utf8').includes('version = "1.1.4-rc.10"'));
    const cargoLock = readFileSync(join(root, 'Cargo.lock'), 'utf8');
    assert.match(cargoLock, /name = "work-review"\r?\nversion = "1\.1\.4-rc\.10"/);
    assert.match(cargoLock, /name = "work-review-core"\r?\nversion = "0\.1\.0"/);
  }
});

test('正式版和已经完整同步的 RC 应通过且不重写源文件', (t) => {
  for (const version of ['0.0.0', '1.1.1', '12.34.56', '1.1.4-rc.1', '1.1.4-rc.10']) {
    const { root, files } = fixture(t, version);
    const result = run(root, `v${version}`);
    assert.equal(result.status, 0, result.stderr);
    assertUnchanged(root, files);
  }
});

test('版本不符和非法 tag 应在写入前失败，不允许直接把 RC 改称正式版', (t) => {
  for (const tag of [
    'v01.2.3', 'v1.02.3', 'v1.2.03', 'v1.2', '1.1.4',
    'v1.1.4-rc.01', 'v1.1.4-rc.', 'v1.1.4-beta.1',
    'v1.1.4-rc.1.extra', 'v1.1.4-rc.1+meta', 'v1.1.5-rc.1',
  ]) {
    const { root, files } = fixture(t);
    assert.equal(run(root, tag).status, 1, tag);
    assertUnchanged(root, files);
  }
  const { root, files } = fixture(t, '1.1.4-rc.1');
  assert.equal(run(root, 'v1.1.4').status, 1);
  assertUnchanged(root, files);
});

test('任一版本源漂移都必须先失败，而不是部分写入后才发现不一致', (t) => {
  const { root, files } = fixture(t);
  files['src-tauri/Cargo.toml'] = files['src-tauri/Cargo.toml'].replace('1.1.4', '1.1.3');
  writeFileSync(join(root, 'src-tauri/Cargo.toml'), files['src-tauri/Cargo.toml'], 'utf8');
  assert.equal(run(root, 'v1.1.4-rc.1').status, 1);
  assertUnchanged(root, files);
});
