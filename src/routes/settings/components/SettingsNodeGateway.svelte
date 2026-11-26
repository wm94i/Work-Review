<script lang="ts">
  import { createEventDispatcher, onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { locale, t } from '$lib/i18n/index.ts';
  import CollapsibleSection from '../../../lib/components/CollapsibleSection.svelte';

  import McpServerPanel from './nodeGateway/McpServerPanel.svelte';
  import LocalApiPanel from './nodeGateway/LocalApiPanel.svelte';
  import TelegramBotPanel from './nodeGateway/TelegramBotPanel.svelte';
  import WecomBotPanel from './nodeGateway/WecomBotPanel.svelte';
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
    feishu_encrypt_key: string;
    wecom_bot_enabled: boolean;
    wecom_bot_id: string;
    wecom_bot_secret: string;
    wecom_corp_id: string;
    wecom_token: string;
    wecom_encoding_aes_key: string;
    dingtalk_bot_enabled: boolean;
    dingtalk_app_secret: string;
    bot_natural_language_enabled?: boolean;
  }

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

  interface WecomBotStatus {
    starting: boolean;
    running: boolean;
    lastError: string | null;
    longConnectionConfigured?: boolean;
    callbackConfigured?: boolean;
  }

  interface ToastDetail {
    message: string;
    type: 'success' | 'error';
  }

  export let config: NodeGatewayConfig;
  export let dataDir: string;

  const dispatch = createEventDispatcher<{
    change: NodeGatewayConfig | { autosaved: true };
    toast: ToastDetail;
  }>();
  $: currentLocale = $locale;

  let nodeStatus: NodeGatewayStatus = { deviceId: '', protocolVersion: '', deviceName: '' };
  let localStatus: LocalApiStatus = { enabled: false, baseUrl: '', tokenPreview: '', lastError: null };
  let tgBotStatus: TelegramBotStatus | null = null;
  let wecomBotStatus: WecomBotStatus | null = null;
  let loading = true;
  let saving = false;
  let tgStatusPollId: ReturnType<typeof setInterval> | null = null;
  let wecomStatusPollId: ReturnType<typeof setInterval> | null = null;

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
    if (typeof config.bot_natural_language_enabled !== 'boolean') {
      config.bot_natural_language_enabled = false;
    }
  }

  function stopTelegramStatusPolling() {
    if (tgStatusPollId) {
      clearInterval(tgStatusPollId);
      tgStatusPollId = null;
    }
  }

  let tgStatusRefreshing = false;
  let lastPolledTelegramChatIds = '';

  async function refreshTelegramBotStatus() {
    if (tgStatusRefreshing) return;
    tgStatusRefreshing = true;
    try {
      tgBotStatus = await invoke<TelegramBotStatus>('get_telegram_bot_status');
      if (tgBotStatus?.allowedChatIds) {
        const nextSnapshot = JSON.stringify(tgBotStatus.allowedChatIds);
        // 仅当后端实际变化时同步，避免轮询覆盖表单中尚未保存的编辑内容
        if (nextSnapshot !== lastPolledTelegramChatIds) {
          lastPolledTelegramChatIds = nextSnapshot;
          config.telegram_bot_allowed_chat_ids = tgBotStatus.allowedChatIds;
        }
      }
    } catch (e) {
      /* ignore */
    } finally {
      tgStatusRefreshing = false;
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

  function stopWecomStatusPolling() {
    if (wecomStatusPollId) {
      clearInterval(wecomStatusPollId);
      wecomStatusPollId = null;
    }
  }

  async function refreshWecomBotStatus() {
    try {
      wecomBotStatus = await invoke<WecomBotStatus>('get_wecom_bot_status');
    } catch (e) {
      /* ignore */
    }
  }

  function startWecomStatusPolling() {
    stopWecomStatusPolling();
    refreshWecomBotStatus();
    let attempts = 0;
    wecomStatusPollId = setInterval(async () => {
      attempts++;
      await refreshWecomBotStatus();
      if (attempts >= 8 || (wecomBotStatus && !wecomBotStatus.starting)) {
        stopWecomStatusPolling();
      }
    }, 3000);
  }

  async function loadStatus() {
    loading = true;
    try {
      const [node, local, tg, wecom] = await Promise.all([
        invoke<NodeGatewayStatus>('get_node_gateway_status'),
        invoke<LocalApiStatus>('get_localhost_api_status'),
        invoke<TelegramBotStatus>('get_telegram_bot_status'),
        invoke<WecomBotStatus>('get_wecom_bot_status'),
      ]);
      nodeStatus = node;
      localStatus = local;
      tgBotStatus = tg;
      wecomBotStatus = wecom;
      if (tgBotStatus?.allowedChatIds) {
        config.telegram_bot_allowed_chat_ids = tgBotStatus.allowedChatIds;
      }
      if (config.telegram_bot_enabled) {
        startTelegramStatusPolling();
      }
      if (config.wecom_bot_enabled) {
        startWecomStatusPolling();
      }
    } catch (e) {
      console.error('加载接入管理状态失败:', e);
    }
    loading = false;
  }

  async function persistConfig() {
    saving = true;
    normalizeConfig();
    try {
      await invoke<void>('save_config', { config });
      dispatch('change', { autosaved: true });
      // Reload status after save
      const [node, local, tg, wecom] = await Promise.all([
        invoke<NodeGatewayStatus>('get_node_gateway_status'),
        invoke<LocalApiStatus>('get_localhost_api_status'),
        invoke<TelegramBotStatus>('get_telegram_bot_status'),
        invoke<WecomBotStatus>('get_wecom_bot_status'),
      ]);
      nodeStatus = node;
      localStatus = local;
      tgBotStatus = tg;
      wecomBotStatus = wecom;
      if (config.wecom_bot_enabled) {
        startWecomStatusPolling();
      }
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
  function handleStartWecomPolling() {
    startWecomStatusPolling();
  }

  onMount(() => {
    normalizeConfig();
    loadStatus();
  });

  onDestroy(() => {
    stopTelegramStatusPolling();
    stopWecomStatusPolling();
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
          <!-- Bot 自然语言只读问答开关（默认关闭，跨全平台生效） -->
          <div class="rounded-lg border border-slate-200 px-3.5 py-3 dark:border-[#30363d]">
            <label class="flex items-start justify-between gap-3 cursor-pointer">
              <div class="min-w-0">
                <span class="settings-text text-sm font-medium inline-flex items-center gap-2">
                  {t('nodeGatewayPage.botNaturalLanguageTitle')}
                  <span class="rounded-full px-1.5 py-0.5 text-[10px] font-medium {config.bot_natural_language_enabled ? 'bg-emerald-50 text-emerald-600 dark:bg-emerald-950/40 dark:text-emerald-400' : 'bg-slate-100 text-slate-400 dark:bg-[#21262d] dark:text-[#636c76]'}">
                    {config.bot_natural_language_enabled ? t('settingsAI.statusEnabled') : t('settingsAI.statusDisabled')}
                  </span>
                </span>
                <p class="settings-muted mt-1">{t('nodeGatewayPage.botNaturalLanguageHint')}</p>
                <p class="settings-muted mt-0.5">{t('nodeGatewayPage.botNaturalLanguageWecomHint')}</p>
              </div>
              <button
                type="button"
                class="switch-track mt-0.5 shrink-0 {config.bot_natural_language_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'} disabled:opacity-50"
                role="switch"
                aria-label={t('nodeGatewayPage.botNaturalLanguageTitle')}
                aria-checked={Boolean(config.bot_natural_language_enabled)}
                disabled={saving}
                on:click={() => {
                  config.bot_natural_language_enabled = !config.bot_natural_language_enabled;
                  handleSave();
                }}
              >
                <span class="switch-thumb {config.bot_natural_language_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
              </button>
            </label>
          </div>
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
            bindingPlatform="feishu"
            titleKey="nodeGatewayPage.feishuBot"
            enabledLabelKey="nodeGatewayPage.feishuEnabled"
            hintKey="nodeGatewayPage.feishuBotHint"
            iconColor="#3370FF"
            iconPath="M20.487 17.14c.88-1.668 1.388-3.566 1.388-5.576C21.875 5.197 17.263.583 11.896.583 6.53.583 1.917 5.197 1.917 10.564c0 5.367 4.613 9.98 9.98 9.98 1.99 0 3.846-.583 5.417-1.585l3.428 1.485a.77.77 0 00.97-1.034l-1.225-2.27z"
            fields={[
              { key: 'feishu_app_id', labelKey: 'nodeGatewayPage.feishuAppId', placeholder: 'cli_xxx' },
              { key: 'feishu_app_secret', labelKey: 'nodeGatewayPage.feishuAppSecret', placeholder: 'Secret', secret: true },
              { key: 'feishu_verification_token', labelKey: 'nodeGatewayPage.feishuVerificationToken', placeholder: 'Verification Token' },
              { key: 'feishu_encrypt_key', labelKey: 'nodeGatewayPage.feishuEncryptKey', placeholder: 'Encrypt Key', secret: true },
            ]}
            on:save={handleSave}
          />
          <WecomBotPanel
            {config}
            {wecomBotStatus}
            {saving}
            on:save={handleSave}
            on:startWecomPolling={handleStartWecomPolling}
          />
          <BotCredentialsPanel
            {config}
            {saving}
            enabledKey="dingtalk_bot_enabled"
            bindingPlatform="dingtalk"
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
