import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('tauri dev 与 vite dev server 应固定使用同一个 127.0.0.1 地址', async () => {
  const [tauriConfigSource, viteConfigSource] = await Promise.all([
    readFile(new URL('./src-tauri/tauri.conf.json', import.meta.url), 'utf8'),
    readFile(new URL('./vite.config.ts', import.meta.url), 'utf8'),
  ]);

  assert.match(tauriConfigSource, /"devUrl":\s*"http:\/\/127\.0\.0\.1:5173"/);
  assert.match(viteConfigSource, /host:\s*'127\.0\.0\.1'/);
  assert.match(viteConfigSource, /cors:\s*\{\s*origin:\s*\/\^https\?/);
  assert.match(viteConfigSource, /127\\\.0\\\.0\\\.1\|localhost/);
  assert.match(viteConfigSource, /hmr:\s*\{\s*host:\s*'127\.0\.0\.1'/);
  assert.match(viteConfigSource, /optimizeDeps:\s*\{\s*entries:\s*\['index\.html',\s*'avatar\.html'\]/);
});
