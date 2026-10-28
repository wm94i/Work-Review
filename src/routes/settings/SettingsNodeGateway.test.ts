import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('设置页应提供节点 Beta 标签并在设置工作台内渲染节点组件', async () => {
  const source = await readFile(new URL('./Settings.svelte', import.meta.url), 'utf8');

  assert.match(source, /import SettingsNodeGateway from '\.\/components\/SettingsNodeGateway\.svelte'/);
  assert.match(source, /id:\s*'node'/);
  assert.match(source, /labelKey:\s*'settings\.tabs\.node'/);
  assert.match(source, /beta:\s*true/);
  assert.match(source, /t\('settings\.tabs\.beta'\)/);
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
  assert.match(source, /invoke(?:<[^>]+>)?\('get_wecom_bot_status'\)/);
  assert.match(source, /invoke(?:<[^>]+>)?\('save_config', \{ config \}\)/);
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
    'WecomBotPanel.svelte',
  ].map((file) => readFile(new URL(`./components/nodeGateway/${file}`, import.meta.url), 'utf8')));

  for (const source of sources) {
    assert.match(source, /role="switch"/);
    assert.match(source, /aria-label=/);
    assert.match(source, /aria-checked=\{/);
  }
  assert.match(sources[2], /nodeGatewayPage\.(?:showSecret|hideSecret)/);
  assert.match(sources[3], /nodeGatewayPage\.(?:showSecret|hideSecret)/);
  assert.match(sources[4], /nodeGatewayPage\.(?:showSecret|hideSecret)/);
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
  assert.match(source, /WecomBotPanel/);
  assert.match(source, /BotCredentialsPanel/);
});

test('企业微信 Bot 应走智能机器人长连接并轮询状态', async () => {
  const [gatewaySource, wecomSource, configSource, mainSource] = await Promise.all([
    readFile(new URL('./components/SettingsNodeGateway.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./components/nodeGateway/WecomBotPanel.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../../../crates/core/src/config.rs', import.meta.url), 'utf8'),
    readFile(new URL('../../../src-tauri/src/main.rs', import.meta.url), 'utf8'),
  ]);

  assert.match(gatewaySource, /function startWecomStatusPolling\(\)/);
  assert.match(gatewaySource, /if \(config\.wecom_bot_enabled\) \{\s*startWecomStatusPolling\(\);/);
  assert.match(gatewaySource, /onDestroy\(\(\) => \{\s*stopTelegramStatusPolling\(\);\s*stopWecomStatusPolling\(\);/);
  assert.match(wecomSource, /wecom_bot_id/);
  assert.match(wecomSource, /wecom_bot_secret/);
  assert.match(wecomSource, /nodeGatewayPage\.wecomConnecting/);
  assert.match(wecomSource, /nodeGatewayPage\.wecomConnected/);
  assert.match(configSource, /pub wecom_bot_id: Option<String>/);
  assert.match(configSource, /pub wecom_bot_secret: Option<String>/);
  assert.match(mainSource, /commands::get_wecom_bot_status/);
});
