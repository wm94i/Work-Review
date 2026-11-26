import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('语义记忆不再有独立页面与侧栏入口（查询统一走助手）', () => {
  const app = read('./App.svelte');
  const sidebar = read('./lib/components/Sidebar.svelte');

  assert.doesNotMatch(app, /'\/memory'/, 'App.svelte 不应再注册 /memory 路由');
  assert.doesNotMatch(sidebar, /sidebar\.nav\.memory/, '侧栏不应再有记忆入口');
  assert.ok(
    !existsSync(fileURLToPath(new URL('./routes/memory/Memory.svelte', import.meta.url))),
    '独立记忆页应已删除'
  );
});

test('索引管理收进设置页语义记忆区块', () => {
  const settings = read('./routes/settings/components/SettingsAI.svelte');

  assert.match(settings, /invoke<SemanticIndexProgress>\('index_semantic_memory'/);
  assert.match(settings, /invoke<SemanticMemoryState>\('semantic_memory_status'\)/);
  assert.match(settings, /memory_semantic_enabled/);
  assert.match(settings, /semantic_auto_index/);
  assert.match(settings, /forceRebuild/);
  // 组件销毁后索引循环必须停止
  assert.match(settings, /semanticDestroyed/);
  // 引导用户"直接问助手"
  assert.match(settings, /semanticMemory\.askHint/);
});

test('语义索引使用稳定指纹、持久化重建状态和 fail-closed 查询', () => {
  const semantic = readFileSync(
    fileURLToPath(new URL('../src-tauri/src/commands/semantic_memory.rs', import.meta.url)),
    'utf8'
  );

  assert.match(semantic, /normalize_embedding_endpoint/);
  assert.match(semantic, /embedding_fingerprint/);
  assert.match(semantic, /privacy_fingerprint/);
  assert.match(semantic, /CHUNK_RULE_VERSION/);
  assert.match(semantic, /NORMALIZATION_VERSION/);
  assert.match(semantic, /SemanticMemoryIndexState/);
  assert.match(semantic, /get_semantic_memory_state/);
  assert.match(semantic, /rebuild_required/);
  const vectorHelper = semantic.slice(
    semantic.indexOf('pub(crate) fn is_vector_query_allowed'),
    semantic.indexOf('fn build_token')
  );
  assert.match(vectorHelper, /!index_state\.rebuild_required/);
  assert.match(vectorHelper, /embedded_chunks\s*>\s*0/);
  assert.match(vectorHelper, /embedding_fingerprint\s*\.starts_with\(config_prefix\)/);
  assert.match(vectorHelper, /privacy_fingerprint\s*==\s*current_privacy_fingerprint/);
  const vectorGate = semantic.slice(
    semantic.indexOf('let vector_query_allowed'),
    semantic.indexOf('let semantic_hits')
  );
  assert.match(vectorGate, /is_vector_query_allowed\(/);
});

test('语义索引和召回都只允许 PrivacyAction::Record', () => {
  const semantic = readFileSync(
    fileURLToPath(new URL('../src-tauri/src/commands/semantic_memory.rs', import.meta.url)),
    'utf8'
  );

  const checks = semantic.match(/check_privacy_full/g) ?? [];
  const records = semantic.match(/PrivacyAction::Record/g) ?? [];
  assert.ok(checks.length >= 2, '索引和召回应各执行一次完整隐私检查');
  assert.ok(records.length >= 2, '索引和召回应各自只保留 Record');
});

test('配置保存会在 Embedding 或隐私指纹变化时先失效语义索引', () => {
  const shared = readFileSync(
    fileURLToPath(new URL('../src-tauri/src/commands/shared.rs', import.meta.url)),
    'utf8'
  );

  assert.match(shared, /embedding_config_fingerprint/);
  assert.match(shared, /privacy_fingerprint/);
  assert.match(shared, /invalidate_semantic_memory_index/);
});

test('清理旧活动的数据库失败必须返回错误', () => {
  const stats = readFileSync(
    fileURLToPath(new URL('../src-tauri/src/commands/stats.rs', import.meta.url)),
    'utf8'
  );

  assert.match(stats, /let deleted_activities = delete_activities\(\)\?;/);
  assert.match(
    stats,
    /state\.database\.delete_activities_before_date\(&yesterday\)\s*\n\s*\}\)\?;/
  );
  assert.doesNotMatch(stats, /if let Err\(e\) = state\.database\.delete_activities_before_date/);
});

test('设置页读取真实语义状态且不再用固定 500 轮驱动一致性', () => {
  const settings = read('./routes/settings/components/SettingsAI.svelte');

  for (const field of ['status', 'rebuildRequired', 'indexedActivities', 'totalActivities', 'lastError']) {
    assert.match(settings, new RegExp(field));
  }
  for (const key of ['statusLabel', 'progress', 'lastError', 'buildIndex', 'rebuildIndex', 'retryIndex', 'ftsFallback']) {
    assert.match(settings, new RegExp(`settingsAI\\.semanticMemory\\.${key}`));
  }
  assert.doesNotMatch(settings, /round\s*<\s*500/);
});

test('助手侧的语义检索接线完整', () => {
  const tools = readFileSync(
    fileURLToPath(new URL('../src-tauri/src/agent/tools.rs', import.meta.url)),
    'utf8'
  );
  const executor = readFileSync(
    fileURLToPath(new URL('../src-tauri/src/agent/executor.rs', import.meta.url)),
    'utf8'
  );
  const ask = readFileSync(
    fileURLToPath(new URL('../src-tauri/src/commands/ask.rs', import.meta.url)),
    'utf8'
  );

  assert.match(tools, /"semantic_search"/, '工具注册中心应有 semantic_search');
  assert.match(executor, /\[记忆能力\]/, '系统提示应声明记忆能力');
  assert.match(ask, /search_semantic_memory_inner/, 'ask.rs 应桥接语义检索');
});

test('语义记忆 i18n 键在四语言中完整', () => {
  for (const localeFile of ['zh-CN', 'zh-TW', 'en', 'ar']) {
    const source = read(`./lib/i18n/locales/${localeFile}.ts`);
    for (const key of ['semanticMemory:', 'askHint:', 'indexStatus:', 'startIndex:', 'providerOllama:']) {
      assert.ok(source.includes(key), `${localeFile}.ts 缺少 ${key}`);
    }
  }
});

test('语义记忆默认关闭且嵌入默认走本地 Ollama（隐私立场守护）', () => {
  const config = readFileSync(
    fileURLToPath(new URL('../crates/core/src/config.rs', import.meta.url)),
    'utf8'
  );
  assert.match(config, /memory_semantic_enabled: bool/);
  assert.match(config, /memory_semantic_enabled: false/);
  assert.match(config, /fn default_embedding_provider\(\) -> String \{\s*"ollama"\.to_string\(\)/);
});
