import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('设置页应提供节点 Beta 标签并在设置工作台内渲染节点组件', async () => {
  const source = await readFile(new URL('./Settings.svelte', import.meta.url), 'utf8');

  assert.match(source, /import SettingsNodeGateway from '\.\/components\/SettingsNodeGateway\.svelte'/);
  assert.match(source, /id:\s*'node'/);
  assert.match(source, /labelKey:\s*'settings\.tabs\.node'/);
  assert.match(source, /beta:\s*true/);
  assert.match(source, /\bBeta\b/);
  assert.match(source, /activeTab === 'node'/);
  assert.match(source, /<SettingsNodeGateway bind:config/);

  const storageTabIndex = source.indexOf("id: 'storage'");
  const nodeTabIndex = source.indexOf("id: 'node'");
  assert.notEqual(storageTabIndex, -1);
  assert.notEqual(nodeTabIndex, -1);
  assert.ok(nodeTabIndex > storageTabIndex, '节点标签应位于存储标签之后');
});

test('节点设置组件应复用设置页配置对象并读取节点与本地 API 状态', async () => {
  const source = await readFile(
    new URL('./components/SettingsNodeGateway.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /export let config/);
  assert.match(source, /invoke(?:<[^>]+>)?\('get_node_gateway_status'\)/);
  assert.match(source, /invoke(?:<[^>]+>)?\('get_localhost_api_status'\)/);
  assert.match(source, /invoke(?:<[^>]+>)?\('get_telegram_bot_status'\)/);
  assert.match(source, /import \{ updateConfigQueued \} from '\$lib\/utils\/configSaveQueue\.ts'/);
  assert.match(source, /const configSnapshot = structuredClone\(config\)/);
  assert.match(source, /updateConfigQueued<NodeGatewayConfig>/);
  assert.match(source, /applyConfigFields\(latestConfig, configSnapshot, NODE_GATEWAY_CONFIG_FIELDS\)/);
  assert.doesNotMatch(source, /saveConfigQueued\(/);
  assert.match(source, /nodeGatewayPage\.title/);
});

test('节点设置组件应提供本地 API 开关和 token 管理', async () => {
  // 拆分后 token 管理逻辑在 LocalApiPanel 子组件
  const source = await readFile(
    new URL('./components/nodeGateway/LocalApiPanel.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /localhost_api_enabled/);
  assert.match(source, /invoke(?:<[^>]+>)?\('reveal_localhost_api_token'\)/);
  assert.match(source, /invoke(?:<[^>]+>)?\('rotate_localhost_api_token'\)/);
  assert.match(source, /nodeGatewayPage\.localApi/);
  assert.match(source, /aria-label=\{t\('nodeGatewayPage\.apiHostLabel'\)\}/);
  assert.match(source, /aria-label=\{t\('nodeGatewayPage\.apiPortLabel'\)\}/);
});

test('节点子面板的二元开关应提供本地化 switch 语义', async () => {
  const sources = await Promise.all([
    'LocalApiPanel.svelte',
    'McpServerPanel.svelte',
    'TelegramBotPanel.svelte',
    'BotCredentialsPanel.svelte',
  ].map((file) => readFile(new URL(`./components/nodeGateway/${file}`, import.meta.url), 'utf8')));

  for (const source of sources) {
    assert.match(source, /role="switch"/);
    assert.match(source, /aria-label=/);
    assert.match(source, /aria-checked=\{/);
  }
  assert.match(sources[2], /nodeGatewayPage\.(?:showSecret|hideSecret)/);
  assert.match(sources[3], /nodeGatewayPage\.(?:showSecret|hideSecret)/);
});

test('Telegram Bot 状态应在页面加载后轮询并在销毁时清理定时器', async () => {
  const source = await readFile(
    new URL('./components/SettingsNodeGateway.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /function startTelegramStatusPolling\(\)/);
  assert.match(source, /setInterval\(async \(\) =>/);
  assert.match(source, /if \(config\.telegram_bot_enabled\) \{\s*startTelegramStatusPolling\(\);/);
  assert.match(source, /onDestroy\(\(\) => \{\s*stopTelegramStatusPolling\(\);/);
});

test('拆分后应包含三个分组的 CollapsibleSection', async () => {
  const source = await readFile(
    new URL('./components/SettingsNodeGateway.svelte', import.meta.url),
    'utf8'
  );
  assert.match(source, /groupAiTools/);
  assert.match(source, /groupNotifications/);
  assert.match(source, /groupAdvanced/);
  assert.match(source, /McpServerPanel/);
  assert.match(source, /LocalApiPanel/);
  assert.match(source, /TelegramBotPanel/);
  assert.match(source, /BotCredentialsPanel/);
});


test('节点网关保存只应合并节点网关、本地 API 与机器人字段', async () => {
  const source = await readFile(
    new URL('./components/SettingsNodeGateway.svelte', import.meta.url),
    'utf8'
  );
  const match = source.match(/const NODE_GATEWAY_CONFIG_FIELDS = \[([\s\S]*?)\] as const/);
  assert.ok(match, '缺少 NODE_GATEWAY_CONFIG_FIELDS 字段白名单');
  const fields = [...match[1].matchAll(/'([^']+)'/g)].map((item) => item[1]);

  assert.deepEqual(fields, [
    'node_gateway',
    'node_devices',
    'mcp_server_enabled',
    'localhost_api_enabled',
    'localhost_api_host',
    'localhost_api_port',
    'telegram_bot_enabled',
    'telegram_bot_token',
    'telegram_bot_proxy',
    'telegram_bot_allowed_chat_ids',
    'feishu_bot_enabled',
    'feishu_app_id',
    'feishu_app_secret',
    'feishu_verification_token',
    'wecom_bot_enabled',
    'wecom_corp_id',
    'wecom_token',
    'wecom_encoding_aes_key',
    'dingtalk_bot_enabled',
    'dingtalk_app_secret',
  ]);
});
