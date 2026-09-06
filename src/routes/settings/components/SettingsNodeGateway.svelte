<script lang="ts">
  import { createEventDispatcher, onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { locale, t } from '$lib/i18n/index.ts';
  import { updateConfigQueued } from '$lib/utils/configSaveQueue.ts';
  import { applyConfigFields } from '$lib/utils/appearancePersistence.ts';
  import CollapsibleSection from '../../../lib/components/CollapsibleSection.svelte';

  import McpServerPanel from './nodeGateway/McpServerPanel.svelte';
  import LocalApiPanel from './nodeGateway/LocalApiPanel.svelte';
  import TelegramBotPanel from './nodeGateway/TelegramBotPanel.svelte';
  import BotCredentialsPanel from './nodeGateway/BotCredentialsPanel.svelte';
  import DeviceIdentityPanel from './nodeGateway/DeviceIdentityPanel.svelte';
  import DeviceRegistryPanel from './nodeGateway/DeviceRegistryPanel.svelte';
  import ApiExamplesPanel from './nodeGateway/ApiExamplesPanel.svelte';

  interface NodeGatewaySettings {
    device_name: string | null;
    endpoint: string | null;
  }

  interface NodeDevice {
    name: string;
    url: string;
    token: string;
  }

  interface NodeGatewayConfig {
    node_gateway: NodeGatewaySettings;
    node_devices?: NodeDevice[];
    mcp_server_enabled: boolean;
    localhost_api_enabled: boolean;
    localhost_api_host: string | null;
    localhost_api_port: number;
    telegram_bot_enabled: boolean;
    telegram_bot_token: string;
    telegram_bot_proxy: string;
    telegram_bot_allowed_chat_ids: number[];
    feishu_bot_enabled: boolean;
    feishu_app_id: string;
    feishu_app_secret: string;
    feishu_verification_token: string;
    wecom_bot_enabled: boolean;
    wecom_corp_id: string;
    wecom_token: string;
    wecom_encoding_aes_key: string;
    dingtalk_bot_enabled: boolean;
    dingtalk_app_secret: string;
  }


  const NODE_GATEWAY_CONFIG_FIELDS = [
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
  ] as const satisfies readonly (keyof NodeGatewayConfig)[];

  interface NodeGatewayStatus {
    deviceId: string;
    protocolVersion: string;
    deviceName: string;
  }

  interface LocalApiStatus {
    enabled: boolean;
    baseUrl: string;
    tokenPreview: string;
    lastError: string | null;
  }

  interface TelegramBotStatus {
    starting: boolean;
    running: boolean;
    lastError: string | null;
    bindCode?: string | null;
    bindCodeExpired?: boolean;
    allowedChatIds?: number[];
  }

  interface ToastDetail {
    message: string;
    type: 'success' | 'error';
  }

  export let config: NodeGatewayConfig;
  export let dataDir: string;

  const dispatch = createEventDispatcher<{
    change: NodeGatewayConfig;
    toast: ToastDetail;
  }>();
  $: currentLocale = $locale;

  let nodeStatus: NodeGatewayStatus = { deviceId: '', protocolVersion: '', deviceName: '' };
  let localStatus: LocalApiStatus = { enabled: false, baseUrl: '', tokenPreview: '', lastError: null };
  let tgBotStatus: TelegramBotStatus | null = null;
  let loading = true;
  let saving = false;
  let tgStatusPollId: ReturnType<typeof setInterval> | null = null;

  $: mcpDbPath = dataDir ? `${dataDir}/work_review.db` : '';
  $: mcpConfigPath = dataDir ? `${dataDir}/config.json` : '';
  $: mcpConfigJson = JSON.stringify({
    mcpServers: {
      'work-review': {
        command: 'work-review-mcp-server',
        env: {
          WORK_REVIEW_DB_PATH: mcpDbPath,
          WORK_REVIEW_CONFIG_PATH: mcpConfigPath,
        },
      },
    },
  }, null, 2);

  function normalizeConfig() {
    if (!config.node_gateway) {
      config.node_gateway = { device_name: null, endpoint: null };
    }
    if (typeof config.node_gateway.device_name === 'string') {
      config.node_gateway.device_name = config.node_gateway.device_name.trim() || null;
    }
    if (
      typeof config.localhost_api_host !== 'string' ||
      !config.localhost_api_host.trim()
    ) {
      config.localhost_api_host = null;
    }
  }

  function stopTelegramStatusPolling() {
    if (tgStatusPollId) {
      clearInterval(tgStatusPollId);
      tgStatusPollId = null;
    }
  }

  async function refreshTelegramBotStatus() {
    try {
      tgBotStatus = await invoke<TelegramBotStatus>('get_telegram_bot_status');
      if (tgBotStatus?.allowedChatIds) {
        config.telegram_bot_allowed_chat_ids = tgBotStatus.allowedChatIds;
      }
    } catch (e) {
      /* ignore */
    }
  }

  function startTelegramStatusPolling() {
    stopTelegramStatusPolling();
    refreshTelegramBotStatus();
    let attempts = 0;
    tgStatusPollId = setInterval(async () => {
      attempts++;
      await refreshTelegramBotStatus();
      if (attempts >= 6 || (tgBotStatus && !tgBotStatus.starting)) {
        stopTelegramStatusPolling();
      }
    }, 3000);
  }

  async function loadStatus() {
    loading = true;
    try {
      const [node, local, tg] = await Promise.all([
        invoke<NodeGatewayStatus>('get_node_gateway_status'),
        invoke<LocalApiStatus>('get_localhost_api_status'),
        invoke<TelegramBotStatus>('get_telegram_bot_status'),
      ]);
      nodeStatus = node;
      localStatus = local;
      tgBotStatus = tg;
      if (tgBotStatus?.allowedChatIds) {
        config.telegram_bot_allowed_chat_ids = tgBotStatus.allowedChatIds;
      }
      if (config.telegram_bot_enabled) {
        startTelegramStatusPolling();
      }
    } catch (e) {
      console.error('加载接入管理状态失败:', e);
    }
    loading = false;
  }

  async function persistConfig() {
    saving = true;
    normalizeConfig();
    const configSnapshot = structuredClone(config);
    try {
      await updateConfigQueued<NodeGatewayConfig>((latestConfig) =>
        applyConfigFields(latestConfig, configSnapshot, NODE_GATEWAY_CONFIG_FIELDS)
      );
      dispatch('change', config);
      // Reload status after save
      const [node, local, tg] = await Promise.all([
        invoke<NodeGatewayStatus>('get_node_gateway_status'),
        invoke<LocalApiStatus>('get_localhost_api_status'),
        invoke<TelegramBotStatus>('get_telegram_bot_status'),
      ]);
      nodeStatus = node;
      localStatus = local;
      tgBotStatus = tg;
    } catch (e) {
      dispatch('toast', { message: t('nodeGatewayPage.saveFailed', { error: e }), type: 'error' });
    }
    saving = false;
  }

  function handleToast(e: CustomEvent<ToastDetail>) {
    dispatch('toast', e.detail);
  }
  function handleSave() {
    persistConfig();
  }
  function handleReloadStatus() {
    loadStatus();
  }
  function handleStartTgPolling() {
    startTelegramStatusPolling();
  }

  onMount(() => {
    normalizeConfig();
    loadStatus();
  });

  onDestroy(() => {
    stopTelegramStatusPolling();
  });
</script>

<div class="settings-card" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('nodeGatewayPage.title')}</h3>
  <p class="settings-card-desc">{t('nodeGatewayPage.subtitle')}</p>

  {#if loading}
    <div class="flex items-center justify-center py-8">
      <div class="animate-spin h-5 w-5 border-2 border-primary-500 border-t-transparent rounded-full"></div>
    </div>
  {:else}
    <div class="space-y-4">
      <!-- 分组 1：AI 工具接入（默认展开） -->
      <CollapsibleSection title={t('nodeGatewayPage.groupAiTools')} storageKey="settings.node.aiTools" defaultOpen={true}>
        <div class="space-y-3">
          <McpServerPanel
            {config}
            {saving}
            {mcpDbPath}
            {mcpConfigPath}
            {mcpConfigJson}
            on:save={handleSave}
            on:toast={handleToast}
          />
          <LocalApiPanel
            {config}
            {localStatus}
            {saving}
            on:save={handleSave}
            on:reloadStatus={handleReloadStatus}
            on:toast={handleToast}
          />
        </div>
      </CollapsibleSection>

      <!-- 分组 2：消息通知（默认收起） -->
      <CollapsibleSection title={t('nodeGatewayPage.groupNotifications')} storageKey="settings.node.bots">
        <div class="space-y-3">
          <TelegramBotPanel
            {config}
            {tgBotStatus}
            {saving}
            on:save={handleSave}
            on:startTgPolling={handleStartTgPolling}
            on:reloadStatus={handleReloadStatus}
            on:toast={handleToast}
          />
          <BotCredentialsPanel
            {config}
            {saving}
            enabledKey="feishu_bot_enabled"
            titleKey="nodeGatewayPage.feishuBot"
            enabledLabelKey="nodeGatewayPage.feishuEnabled"
            hintKey="nodeGatewayPage.feishuBotHint"
            iconColor="#3370FF"
            iconPath="M20.487 17.14c.88-1.668 1.388-3.566 1.388-5.576C21.875 5.197 17.263.583 11.896.583 6.53.583 1.917 5.197 1.917 10.564c0 5.367 4.613 9.98 9.98 9.98 1.99 0 3.846-.583 5.417-1.585l3.428 1.485a.77.77 0 00.97-1.034l-1.225-2.27z"
            fields={[
              { key: 'feishu_app_id', labelKey: 'nodeGatewayPage.feishuAppId', placeholder: 'cli_xxx' },
              { key: 'feishu_app_secret', labelKey: 'nodeGatewayPage.feishuAppSecret', placeholder: 'Secret', secret: true },
              { key: 'feishu_verification_token', labelKey: 'nodeGatewayPage.feishuVerificationToken', placeholder: 'Verification Token' },
            ]}
            on:save={handleSave}
          />
          <BotCredentialsPanel
            {config}
            {saving}
            enabledKey="wecom_bot_enabled"
            titleKey="nodeGatewayPage.wecomBot"
            enabledLabelKey="nodeGatewayPage.wecomEnabled"
            hintKey="nodeGatewayPage.wecomBotHint"
            iconColor="#07C160"
            iconPath="M9.5 4C5.36 4 2 6.91 2 10.5c0 2.08 1.13 3.93 2.88 5.13-.14.8-.5 2-.5 2l2-.5c.5.24 1.04.42 1.6.54-.13-.5-.2-1.02-.2-1.55C8.78 13.69 12 12 16 12c.3 0 .6.01.89.04C16.96 7.56 13.64 4 9.5 4z"
            fields={[
              { key: 'wecom_corp_id', labelKey: 'nodeGatewayPage.wecomCorpId', placeholder: 'Corp ID' },
              { key: 'wecom_token', labelKey: 'nodeGatewayPage.wecomToken', placeholder: 'Token' },
              { key: 'wecom_encoding_aes_key', labelKey: 'nodeGatewayPage.wecomEncodingAesKey', placeholder: 'EncodingAESKey' },
            ]}
            on:save={handleSave}
          />
          <BotCredentialsPanel
            {config}
            {saving}
            enabledKey="dingtalk_bot_enabled"
            titleKey="nodeGatewayPage.dingtalkBot"
            enabledLabelKey="nodeGatewayPage.dingtalkEnabled"
            hintKey="nodeGatewayPage.dingtalkBotHint"
            iconColor="#1677FF"
            iconPath="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2z"
            fields={[
              { key: 'dingtalk_app_secret', labelKey: 'nodeGatewayPage.dingtalkAppSecret', placeholder: 'App Secret', secret: true },
            ]}
            on:save={handleSave}
          />
        </div>
      </CollapsibleSection>

      <!-- 分组 3：高级（默认收起） -->
      <CollapsibleSection title={t('nodeGatewayPage.groupAdvanced')} storageKey="settings.node.advanced">
        <div class="space-y-4">
          <DeviceIdentityPanel {config} {nodeStatus} />
          {#if config.telegram_bot_enabled || config.feishu_bot_enabled || config.wecom_bot_enabled || config.dingtalk_bot_enabled}
            <DeviceRegistryPanel {config} {localStatus} on:save={handleSave} />
          {/if}
          {#if config.localhost_api_enabled}
            <ApiExamplesPanel {localStatus} on:toast={handleToast} />
          {/if}
        </div>
      </CollapsibleSection>

      <!-- Error -->
      {#if localStatus.lastError}
        <div class="rounded-lg bg-red-50 px-3 py-2 ring-1 ring-red-200 dark:bg-red-950/20 dark:ring-red-900/50">
          <p class="text-[11px] text-red-600 dark:text-red-400">{localStatus.lastError}</p>
        </div>
      {/if}
    </div>
  {/if}
</div>
