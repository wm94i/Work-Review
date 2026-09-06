import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const productionConfigWriters = [
  './App.svelte',
  './routes/report/Report.svelte',
  './routes/settings/Settings.svelte',
  './routes/settings/components/SettingsAppearance.svelte',
  './routes/settings/components/SettingsGeneral.svelte',
  './routes/settings/components/SettingsNodeGateway.svelte',
  './routes/timeline/Timeline.svelte',
] as const;

test('局部配置写入不得再排队保存页面持有的完整旧快照', async () => {
  const sources = await Promise.all(
    productionConfigWriters.map(async (path) => ({
      path,
      source: await readFile(new URL(path, import.meta.url), 'utf8'),
    })),
  );

  for (const { path, source } of sources) {
    assert.doesNotMatch(
      source,
      /\bsaveConfigQueued\s*\(/,
      `${path} 仍在保存调用时捕获的完整配置快照`,
    );
    assert.doesNotMatch(
      source,
      /import\s+\{[^}]*\bsaveConfigQueued\b[^}]*\}\s+from\s+['"][^'"]*configSaveQueue\.ts['"]/,
      `${path} 不应继续导入完整快照保存函数`,
    );
  }
});
