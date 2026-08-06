import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('云端分析器应使用配置的兼容端点，而不是写死官方 OpenAI 地址', async () => {
  const [cloudSource, analysisSource] = await Promise.all([
    readFile(new URL('../crates/core/src/analysis/cloud.rs', import.meta.url), 'utf8'),
    readFile(new URL('../crates/core/src/analysis/mod.rs', import.meta.url), 'utf8'),
  ]);

  assert.doesNotMatch(cloudSource, /https:\/\/api\.openai\.com\/v1\/chat\/completions/);
  assert.match(cloudSource, /build_chat_url/);
  assert.match(analysisSource, /CloudAnalyzer::new\(\s*&model_config\.endpoint,/);
});
