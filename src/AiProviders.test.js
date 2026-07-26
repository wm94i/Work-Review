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

test('应提供 MiniMax 作为新的 AI 提供商并同步到文档', async () => {
  const [configSource, commandSource, readmeSource, readmeEnSource] = await Promise.all([
    readFile(new URL('../crates/core/src/config.rs', import.meta.url), 'utf8'),
    readCommandsSource(),
    readFile(new URL('../README.zh.md', import.meta.url), 'utf8'),
    readFile(new URL('../README.md', import.meta.url), 'utf8'),
  ]);

  assert.match(configSource, /MiniMax/);
  assert.match(configSource, /https:\/\/api\.minimaxi\.com\/v1/);
  assert.match(commandSource, /稀宇科技 MiniMax/);
  assert.match(commandSource, /MiniMax-M2\.5/);
  assert.match(readmeSource, /MiniMax/);
  assert.match(readmeEnSource, /MiniMax/);
});

test('应提供 OpenRouter/Groq/xAI/Mistral/LM Studio/自定义 六个新提供商并同步文档', async () => {
  const [configSource, commandSource, readmeSource, readmeEnSource, settingsSource] =
    await Promise.all([
      readFile(new URL('../crates/core/src/config.rs', import.meta.url), 'utf8'),
      readCommandsSource(),
      readFile(new URL('../README.zh.md', import.meta.url), 'utf8'),
      readFile(new URL('../README.md', import.meta.url), 'utf8'),
      readFile(
        new URL('./routes/settings/components/SettingsAI.svelte', import.meta.url),
        'utf8'
      ),
    ]);

  for (const id of ['openrouter', 'groq', 'xai', 'mistral', 'lmstudio', 'custom']) {
    assert.match(configSource, new RegExp(`"${id}"`), `config.rs 缺少 ${id} serde rename`);
    assert.match(commandSource, new RegExp(`"id": "${id}"`), `get_ai_providers 缺少 ${id}`);
    assert.match(settingsSource, new RegExp(`${id}:`), `SettingsAI providerLabels 缺少 ${id}`);
  }
  // 全部走 OpenAI 兼容派发
  assert.match(configSource, /AiProvider::OpenRouter[\s\S]*?AiProvider::Custom[\s\S]*?\)\s*\}/);
  assert.match(readmeSource, /OpenRouter/);
  assert.match(readmeEnSource, /OpenRouter/);
  // 品牌图标缺失时必须有字母块回退
  assert.match(settingsSource, /providerIconFailed/);
  assert.match(settingsSource, /icons\/providers/);
});

test('应提供 Atlas Cloud OpenAI 兼容提供商并同步技术清单', async () => {
  const [configSource, commandSource, readmeSource, readmeEnSource, settingsSource, askSource] =
    await Promise.all([
      readFile(new URL('../crates/core/src/config.rs', import.meta.url), 'utf8'),
      readCommandsSource(),
      readFile(new URL('../README.zh.md', import.meta.url), 'utf8'),
      readFile(new URL('../README.md', import.meta.url), 'utf8'),
      readFile(
        new URL('./routes/settings/components/SettingsAI.svelte', import.meta.url),
        'utf8'
      ),
      readFile(new URL('./routes/ask/Ask.svelte', import.meta.url), 'utf8'),
    ]);

  assert.match(configSource, /AtlasCloud/);
  assert.match(configSource, /https:\/\/api\.atlascloud\.ai\/v1/);
  assert.match(configSource, /deepseek-ai\/deepseek-v4-pro/);
  assert.match(configSource, /AiProvider::AtlasCloud[\s\S]*?AiProvider::Custom[\s\S]*?\)\s*\}/);
  assert.match(commandSource, /"id": "atlascloud"/);
  assert.match(settingsSource, /atlascloud:/);
  assert.match(askSource, /atlascloud:/);
  assert.match(readmeSource, /Atlas Cloud/);
  assert.match(readmeEnSource, /Atlas Cloud/);
});
