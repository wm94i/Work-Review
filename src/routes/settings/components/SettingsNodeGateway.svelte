<script>
  import { createEventDispatcher, onDestroy, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { locale, t } from '$lib/i18n/index.js';
  import { showToast } from '$lib/stores/toast.js';

  export let config;
  export let dataDir = '';

  const dispatch = createEventDispatcher();

  let nodeStatus = null;
  let localStatus = null;
  let tgBotStatus = null;
  let loading = true;
  let saving = false;
  let tokenVisible = false;
  let tokenValue = '';
  let tgTokenVisible = false;
  let feishuSecretVisible = false;
  let examplesExpanded = false;
  let activeApiCategory = 'all';
  let tgStatusPollId = null;
  $: currentLocale = $locale;

  $: mcpDbPath = dataDir ? `${dataDir}/work_review.db` : '';
  $: mcpConfigPath = dataDir ? `${dataDir}/config.json` : '';
  $: mcpConfigJson = JSON.stringify({
    mcpServers: {
      'work-review': {
        command: '/path/to/work-review-mcp-server',
        args: [],
        env: {
          WORK_REVIEW_DB_PATH: mcpDbPath || '/path/to/work_review.db',
          WORK_REVIEW_CONFIG_PATH: mcpConfigPath || '/path/to/config.json',
        },
      },
    },
  }, null, 2);

  function normalizeConfig() {
    if (!config.node_gateway || typeof config.node_gateway !== 'object') {
      config.node_gateway = { device_name: null };
    }
    if (
      typeof config.node_gateway.device_name !== 'string' ||
      !config.node_gateway.device_name.trim()
    ) {
      config.node_gateway.device_name = null;
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
      tgBotStatus = await invoke('get_telegram_bot_status');
      if (Array.isArray(tgBotStatus?.allowedChatIds)) {
        config.telegram_bot_allowed_chat_ids = tgBotStatus.allowedChatIds;
      }
    } catch (error) {
      console.warn('刷新 Telegram Bot 状态失败:', error);
    }
  }

  function startTelegramStatusPolling() {
    stopTelegramStatusPolling();
    if (!config.telegram_bot_enabled) {
      return;
    }

    let polls = 0;
    tgStatusPollId = setInterval(async () => {
      polls += 1;
      await refreshTelegramBotStatus();
      if (polls >= 6 || (tgBotStatus && !tgBotStatus.starting)) {
        stopTelegramStatusPolling();
      }
    }, 3000);
  }

  async function loadStatus() {
    if (!nodeStatus || !localStatus) {
      loading = true;
    }
    try {
      const [nextNodeStatus, nextLocalStatus, nextTgBotStatus] = await Promise.all([
        invoke('get_node_gateway_status'),
        invoke('get_localhost_api_status'),
        invoke('get_telegram_bot_status'),
      ]);
      nodeStatus = nextNodeStatus;
      localStatus = nextLocalStatus;
      tgBotStatus = nextTgBotStatus;
      if (Array.isArray(tgBotStatus?.allowedChatIds)) {
        config.telegram_bot_allowed_chat_ids = tgBotStatus.allowedChatIds;
      }
      if (config.telegram_bot_enabled) {
        startTelegramStatusPolling();
      } else {
        stopTelegramStatusPolling();
      }
    } catch (error) {
      console.error('读取集成页面数据失败:', error);
      showToast(t('nodeGatewayPage.loadFailed', { error }), 'error');
    } finally {
      loading = false;
    }
  }

  async function persistConfig(successMessage = null) {
    saving = true;
    try {
      normalizeConfig();
      await invoke('save_config', { config });
      dispatch('change', config);
      await loadStatus();
      if (successMessage) {
        showToast(successMessage, 'success');
      }
      return true;
    } catch (error) {
      console.error('保存集成配置失败:', error);
      showToast(t('nodeGatewayPage.saveFailed', { error }), 'error');
      return false;
    } finally {
      saving = false;
    }
  }

  async function revealToken() {
    try {
      tokenValue = await invoke('reveal_localhost_api_token');
      tokenVisible = true;
    } catch (error) {
      console.error('读取本地 API token 失败:', error);
      showToast(t('nodeGatewayPage.tokenRevealFailed', { error }), 'error');
    }
  }

  async function rotateToken() {
    try {
      tokenValue = await invoke('rotate_localhost_api_token');
      tokenVisible = true;
      localStatus = await invoke('get_localhost_api_status');
      showToast(t('nodeGatewayPage.tokenRotated'), 'success');
    } catch (error) {
      console.error('轮换本地 API token 失败:', error);
      showToast(t('nodeGatewayPage.tokenRotateFailed', { error }), 'error');
    }
  }

  async function copyToken() {
    if (!tokenVisible || !tokenValue) {
      await revealToken();
    }
    if (!tokenValue) return;

    try {
      await navigator.clipboard.writeText(tokenValue);
      showToast(t('nodeGatewayPage.tokenCopied'), 'success');
    } catch (error) {
      console.error('复制本地 API token 失败:', error);
      showToast(t('nodeGatewayPage.tokenCopyFailed', { error }), 'error');
    }
  }

  async function generateTelegramBindCode() {
    try {
      const result = await invoke('generate_telegram_bot_bind_code');
      tgBotStatus = await invoke('get_telegram_bot_status');
      const code = result?.code || tgBotStatus?.bindCode;
      if (code) {
        await navigator.clipboard.writeText(`/bind ${code}`);
      }
      showToast(t('nodeGatewayPage.telegramBindCodeGenerated'), 'success');
    } catch (error) {
      console.error('生成 Telegram 绑定码失败:', error);
      showToast(t('nodeGatewayPage.telegramBindCodeFailed', { error }), 'error');
    }
  }

  $: curlToken = tokenVisible && tokenValue ? tokenValue : '<your-token>';
  $: curlBase = localStatus ? localStatus.baseUrl : 'http://127.0.0.1:47831';

  function curlCommand(method, path, body = null) {
    const sep = path.includes('?') ? '&' : '?';
    const auth = path === '/health' ? '' : `${sep}token=${curlToken}`;
    const methodFlag = method === 'GET' ? '' : ` -X ${method}`;
    const contentType = body ? ` -H "Content-Type: application/json" -d '${JSON.stringify(body)}'` : '';
    return `curl${methodFlag}${contentType} ${curlBase}${path}${auth}`;
  }

  async function copyCurl(cmd) {
    try {
      await navigator.clipboard.writeText(cmd);
      showToast(t('nodeGatewayPage.curlCopied'), 'success');
    } catch (error) {
      showToast(t('nodeGatewayPage.tokenCopyFailed', { error }), 'error');
    }
  }

  async function copyMcpConfig() {
    try {
      await navigator.clipboard.writeText(mcpConfigJson);
      showToast(t('nodeGatewayPage.mcpServerConfigCopied'), 'success');
    } catch (error) {
      showToast(t('nodeGatewayPage.tokenCopyFailed', { error }), 'error');
    }
  }

  onMount(async () => {
    normalizeConfig();
    await loadStatus();
  });

  onDestroy(() => {
    stopTelegramStatusPolling();
  });
</script>

<div class="settings-card node-gateway-settings-shell" data-locale={currentLocale}>
  <div class="flex items-center justify-between">
    <div>
      <h3 class="settings-card-title mb-0">{t('nodeGatewayPage.title')}</h3>
      <p class="settings-card-desc mb-0">{t('nodeGatewayPage.subtitle')}</p>
    </div>
    {#if !loading}
    <button
      type="button"
      class="settings-action-secondary"
      on:click={loadStatus}
      disabled={saving}
    >
      {t('nodeGatewayPage.refresh')}
    </button>
    {/if}
  </div>

  {#if loading}
    <div class="flex justify-center py-10">
      <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary-500"></div>
    </div>
  {:else if nodeStatus && localStatus}
    <div class="settings-section space-y-4">

      <!-- Device Identity -->
      <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 dark:border-slate-700/80 dark:bg-slate-800/40">
        <div class="flex items-center gap-2 mb-3">
          <div class="flex h-6 w-6 items-center justify-center rounded-md bg-primary-100 dark:bg-primary-900/30">
            <svg class="w-3.5 h-3.5 text-primary-600 dark:text-primary-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M9 3v2m6-2v2M9 19v2m6-2v2M5 9H3m2 6H3m18-6h-2m2 6h-2M7 19h10a2 2 0 002-2V7a2 2 0 00-2-2H7a2 2 0 00-2 2v10a2 2 0 002 2z" />
            </svg>
          </div>
          <span class="text-sm font-medium text-slate-700 dark:text-slate-200">{t('nodeGatewayPage.deviceIdentity')}</span>
          <span class="settings-chip-neutral">{nodeStatus.protocolVersion}</span>
        </div>
        <div class="space-y-2">
          <div class="flex items-center justify-between gap-2 rounded-lg bg-white/70 px-3 py-1.5 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
            <span class="text-xs text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.deviceId')}</span>
            <span class="font-mono text-xs text-slate-700 dark:text-slate-300" title={nodeStatus.deviceId}>{nodeStatus.deviceId}</span>
          </div>
          <label class="block">
            <span class="text-xs text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.deviceName')}</span>
            <input
              type="text"
              bind:value={config.node_gateway.device_name}
              class="mt-0.5 w-full rounded-lg bg-white/70 px-3 py-1.5 text-sm text-slate-800 ring-1 ring-slate-200/70 focus:ring-primary-300 dark:bg-slate-900/20 dark:text-white dark:ring-slate-700/70 dark:focus:ring-primary-600 focus:outline-none"
              placeholder={nodeStatus.deviceName}
            />
          </label>
          <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.deviceNameHint')}</p>
        </div>
      </div>

      <!-- ========== Bot 集成 大卡片 ========== -->
      <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 space-y-4 dark:border-slate-700/80 dark:bg-slate-800/40">
        <!-- Bot 集成 标题 -->
        <div class="flex items-center gap-2">
          <div class="flex h-7 w-7 items-center justify-center rounded-lg bg-indigo-100 dark:bg-indigo-900/30">
            <svg class="w-4 h-4 text-indigo-600 dark:text-indigo-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M8.288 15.038a5.25 5.25 0 017.424 0M5.106 11.856c3.807-3.808 9.98-3.808 13.788 0M1.924 8.674c5.565-5.565 14.587-5.565 20.152 0" />
            </svg>
          </div>
          <div>
            <span class="text-sm font-semibold text-slate-700 dark:text-slate-200">{t('nodeGatewayPage.botIntegration')}</span>
          </div>
        </div>

        <!-- Local API -->
        <div class="rounded-xl bg-white/70 px-3.5 py-3 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
          <div class="flex items-center justify-between gap-3 mb-2">
            <div class="flex items-center gap-2">
              <div class="flex h-5 w-5 items-center justify-center rounded-md bg-primary-100 dark:bg-primary-900/30">
                <svg class="w-3 h-3 text-primary-600 dark:text-primary-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2" />
                </svg>
              </div>
              <span class="text-sm font-medium text-slate-700 dark:text-slate-200">{t('nodeGatewayPage.localApi')}</span>
              {#if localStatus.enabled}
                <span class="settings-chip-success">{t('nodeGatewayPage.localhostEnabled')}</span>
              {:else}
                <span class="settings-chip-neutral">{t('nodeGatewayPage.localhostDisabled')}</span>
              {/if}
            </div>
            <button
              type="button"
              on:click={() => {
                config.localhost_api_enabled = !config.localhost_api_enabled;
                persistConfig();
              }}
              disabled={saving}
              class="switch-track {config.localhost_api_enabled ? 'bg-primary-500' : 'bg-slate-300 dark:bg-slate-600'} {saving ? 'opacity-60 cursor-not-allowed' : ''}"
            >
              <span class="switch-thumb {config.localhost_api_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
            </button>
          </div>

          {#if config.localhost_api_enabled}
          <div class="space-y-2">
            <div class="grid gap-2 grid-cols-2">
              <div class="rounded-lg bg-white/70 px-3 py-1.5 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
                <div class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.apiHostLabel')}</div>
                <input
                  type="text"
                  bind:value={config.localhost_api_host}
                  class="w-full bg-transparent text-sm font-mono text-slate-800 dark:text-white focus:outline-none"
                  placeholder="127.0.0.1"
                />
              </div>
              <div class="rounded-lg bg-white/70 px-3 py-1.5 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
                <div class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.apiPortLabel')}</div>
                <input
                  type="number"
                  bind:value={config.localhost_api_port}
                  on:blur={() => {
                    if (!Number.isInteger(config.localhost_api_port) || config.localhost_api_port <= 0 || config.localhost_api_port > 65535) {
                      config.localhost_api_port = 47831;
                    }
                  }}
                  class="w-full bg-transparent text-sm font-mono text-slate-800 dark:text-white focus:outline-none"
                  min="1"
                  max="65535"
                  placeholder="47831"
                />
              </div>
            </div>
            <div class="flex items-center justify-between rounded-lg bg-white/70 px-3 py-1.5 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
              <span class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.localhostAddress')}</span>
              <span class="text-sm font-mono text-slate-700 dark:text-slate-300">{localStatus.baseUrl}</span>
            </div>
            <div class="rounded-lg bg-white/70 px-3 py-2 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
              <div class="flex items-center justify-between gap-2 mb-1">
                <span class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.localhostToken')}</span>
                <div class="flex gap-1">
                  <button type="button" class="settings-chip-button" on:click={revealToken}>{t('nodeGatewayPage.revealToken')}</button>
                  <button type="button" class="settings-chip-button" on:click={copyToken}>{t('nodeGatewayPage.copyToken')}</button>
                  <button type="button" class="settings-chip-button settings-chip-button-active" on:click={rotateToken}>{t('nodeGatewayPage.rotateToken')}</button>
                </div>
              </div>
              <div class="font-mono text-[11px] text-slate-500 dark:text-slate-400 break-all">
                {tokenVisible ? tokenValue || t('nodeGatewayPage.empty') : localStatus.tokenPreview || t('nodeGatewayPage.empty')}
              </div>
            </div>
            <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.apiHostHint')}</p>
          </div>
          {/if}
        </div>

        {#if config.localhost_api_enabled}
        <!-- Telegram Bot -->
        <div class="rounded-xl bg-white/70 px-3.5 py-3 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
          <div class="flex items-center justify-between gap-3">
            <div class="flex items-center gap-2">
              <svg class="w-4 h-4 text-[#229ED9]" viewBox="0 0 24 24" fill="currentColor">
                <path d="M11.944 0A12 12 0 0 0 0 12a12 12 0 0 0 12 12 12 12 0 0 0 12-12A12 12 0 0 0 12 0a12 12 0 0 0-.056 0zm4.962 7.224c.1-.002.321.023.465.14a.506.506 0 0 1 .171.325c.016.093.036.306.02.472-.18 1.898-.962 6.502-1.36 8.627-.168.9-.499 1.201-.82 1.23-.696.065-1.225-.46-1.9-.902-1.056-.693-1.653-1.124-2.678-1.8-1.185-.78-.417-1.21.258-1.91.177-.184 3.247-2.977 3.307-3.23.007-.032.014-.15-.056-.212s-.174-.041-.249-.024c-.106.024-1.793 1.14-5.061 3.345-.479.33-.913.49-1.302.48-.428-.008-1.252-.241-1.865-.44-.752-.245-1.349-.374-1.297-.789.027-.216.325-.437.893-.663 3.498-1.524 5.83-2.529 6.998-3.014 3.332-1.386 4.025-1.627 4.476-1.635z"/>
              </svg>
              <span class="text-sm text-slate-700 dark:text-slate-200">Telegram</span>
              {#if config.telegram_bot_enabled}
                <span class="settings-chip-success">{t('nodeGatewayPage.telegramEnabled')}</span>
              {/if}
            </div>
            <button
              type="button"
              on:click={() => {
                config.telegram_bot_enabled = !config.telegram_bot_enabled;
                persistConfig();
              }}
              disabled={saving}
              class="switch-track {config.telegram_bot_enabled ? 'bg-primary-500' : 'bg-slate-300 dark:bg-slate-600'} {saving ? 'opacity-60 cursor-not-allowed' : ''}"
            >
              <span class="switch-thumb {config.telegram_bot_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
            </button>
          </div>
          {#if config.telegram_bot_enabled}
          <div class="mt-2 space-y-2">
            <label class="block">
              <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.telegramBotToken')}</span>
              <div class="mt-0.5 relative">
                {#if tgTokenVisible}
                  <input
                    type="text"
                    bind:value={config.telegram_bot_token}
                    on:blur={() => persistConfig()}
                    class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                    placeholder="123456:ABC-DEF..."
                  />
                {:else}
                  <input
                    type="password"
                    bind:value={config.telegram_bot_token}
                    on:blur={() => persistConfig()}
                    class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                    placeholder="123456:ABC-DEF..."
                  />
                {/if}
                <button
                  type="button"
                  class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-600 dark:hover:text-slate-300"
                  on:click={() => (tgTokenVisible = !tgTokenVisible)}
                >
                  {#if tgTokenVisible}
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
                  {:else}
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                  {/if}
                </button>
              </div>
            </label>
            {#if tgBotStatus}
              {#if tgBotStatus.starting}
                <div class="flex items-center gap-1.5 text-[11px] text-blue-500 dark:text-blue-400">
                  <div class="animate-spin h-3 w-3 border-[1.5px] border-blue-400 border-t-transparent rounded-full"></div>
                  ...
                </div>
              {:else if tgBotStatus.running}
                <div class="flex items-center gap-1.5 text-[11px] text-emerald-600 dark:text-emerald-400">
                  <span class="inline-block h-1.5 w-1.5 rounded-full bg-emerald-500"></span>
                  Bot running
                </div>
              {:else if tgBotStatus.lastError}
                <div class="flex items-center gap-1.5 text-[11px] text-amber-600 dark:text-amber-400">
                  <span class="inline-block h-1.5 w-1.5 rounded-full bg-amber-500"></span>
                  {tgBotStatus.lastError}
                </div>
              {/if}
            {/if}
            <label class="block">
              <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.telegramBotProxy')}</span>
              <input
                type="text"
                bind:value={config.telegram_bot_proxy}
                on:blur={() => persistConfig()}
                class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                placeholder="http://127.0.0.1:7890"
              />
            </label>
            <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.telegramBotProxyHint')}</p>
            <div class="rounded-lg bg-blue-50/70 px-3 py-2 ring-1 ring-blue-100 dark:bg-blue-950/20 dark:ring-blue-900/50">
              <div class="flex items-center justify-between gap-3">
                <div class="min-w-0">
                  <div class="text-[11px] font-medium text-blue-700 dark:text-blue-300">{t('nodeGatewayPage.telegramBindCodeTitle')}</div>
                  <div class="mt-0.5 text-[11px] text-blue-600/80 dark:text-blue-300/80">{t('nodeGatewayPage.telegramBindCodeHint')}</div>
                  {#if tgBotStatus?.bindCode && !tgBotStatus?.bindCodeExpired}
                    <div class="mt-1 font-mono text-sm font-semibold text-blue-800 dark:text-blue-200">/bind {tgBotStatus.bindCode}</div>
                  {:else if tgBotStatus?.bindCodeExpired}
                    <div class="mt-1 text-[11px] text-amber-600 dark:text-amber-300">{t('nodeGatewayPage.telegramBindCodeExpired')}</div>
                  {/if}
                </div>
                <button
                  type="button"
                  class="shrink-0 rounded-md bg-blue-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-blue-700 disabled:opacity-60"
                  on:click={generateTelegramBindCode}
                  disabled={saving}
                >
                  {t('nodeGatewayPage.telegramBindCodeGenerate')}
                </button>
              </div>
            </div>
            <label class="block mt-2">
              <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.telegramAllowedChatIds')}</span>
              <input
                type="text"
                value={(config.telegram_bot_allowed_chat_ids || []).join(', ')}
                on:blur={(e) => {
                  const ids = e.target.value.split(',').map(s => parseInt(s.trim(), 10)).filter(n => !isNaN(n));
                  config.telegram_bot_allowed_chat_ids = ids;
                  persistConfig();
                }}
                class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                placeholder="123456789, 987654321"
              />
            </label>
            <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.telegramAllowedChatIdsHint')}</p>
            <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.telegramBotHint')}</p>
          </div>
          {/if}
        </div>

        <!-- Feishu Bot -->
        <div class="rounded-xl bg-white/70 px-3.5 py-3 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
          <div class="flex items-center justify-between gap-3">
            <div class="flex items-center gap-2">
              <svg class="w-4 h-4 text-[#3370FF]" viewBox="0 0 24 24" fill="currentColor">
                <path d="M20.487 17.14c.88-1.668 1.388-3.566 1.388-5.576C21.875 5.197 17.263.583 11.896.583 6.53.583 1.917 5.197 1.917 10.564c0 5.367 4.613 9.98 9.98 9.98 1.99 0 3.846-.583 5.417-1.585l3.428 1.485a.77.77 0 00.97-1.034l-1.225-2.27z"/>
              </svg>
              <span class="text-sm text-slate-700 dark:text-slate-200">{t('nodeGatewayPage.feishuBot')}</span>
              {#if config.feishu_bot_enabled}
                <span class="settings-chip-success">{t('nodeGatewayPage.feishuEnabled')}</span>
              {/if}
            </div>
            <button
              type="button"
              on:click={() => {
                config.feishu_bot_enabled = !config.feishu_bot_enabled;
                persistConfig();
              }}
              disabled={saving}
              class="switch-track {config.feishu_bot_enabled ? 'bg-primary-500' : 'bg-slate-300 dark:bg-slate-600'} {saving ? 'opacity-60 cursor-not-allowed' : ''}"
            >
              <span class="switch-thumb {config.feishu_bot_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
            </button>
          </div>
          {#if config.feishu_bot_enabled}
          <div class="mt-2 space-y-2">
            <div class="grid gap-2 grid-cols-2">
              <label class="block">
                <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.feishuAppId')}</span>
                <input
                  type="text"
                  bind:value={config.feishu_app_id}
                  on:blur={() => persistConfig()}
                  class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="cli_xxx"
                />
              </label>
              <label class="block">
                <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.feishuAppSecret')}</span>
                <div class="mt-0.5 relative">
                  {#if feishuSecretVisible}
                    <input
                      type="text"
                      bind:value={config.feishu_app_secret}
                      on:blur={() => persistConfig()}
                      class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                      placeholder="Secret"
                    />
                  {:else}
                    <input
                      type="password"
                      bind:value={config.feishu_app_secret}
                      on:blur={() => persistConfig()}
                      class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                      placeholder="Secret"
                    />
                  {/if}
                  <button
                    type="button"
                    class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-600 dark:hover:text-slate-300"
                    on:click={() => (feishuSecretVisible = !feishuSecretVisible)}
                  >
                    {#if feishuSecretVisible}
                      <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
                    {:else}
                      <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                    {/if}
                  </button>
                </div>
              </label>
            </div>
            <label class="block">
              <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.feishuVerificationToken')}</span>
              <input
                type="text"
                bind:value={config.feishu_verification_token}
                on:blur={() => persistConfig()}
                class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                placeholder="Verification Token"
              />
            </label>
            <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.feishuBotHint')}</p>
          </div>
          {/if}
        </div>

        <!-- Device Registry -->
        {#if config.telegram_bot_enabled || config.feishu_bot_enabled}
        <div class="space-y-2">
          <div class="flex items-center justify-between">
            <span class="text-xs font-medium text-slate-600 dark:text-slate-300">{t('nodeGatewayPage.deviceRegistry')}</span>
            <button
              type="button"
              class="settings-chip-button settings-chip-button-active text-[11px]"
              on:click={() => {
                if (!config.node_devices) config.node_devices = [];
                config.node_devices = [...config.node_devices, { name: '', url: '', token: '' }];
              }}
            >
              {t('nodeGatewayPage.addDevice')}
            </button>
          </div>
          <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.deviceRegistryHint')}</p>
          <div class="flex items-center gap-2 rounded-lg bg-primary-50/60 px-3 py-1.5 ring-1 ring-primary-200/60 dark:bg-primary-900/20 dark:ring-primary-800/40">
            <span class="text-xs font-medium text-primary-700 dark:text-primary-300">{t('nodeGatewayPage.localDevice')}</span>
            <span class="text-xs font-mono text-slate-500 dark:text-slate-400">{localStatus?.baseUrl || '-'}</span>
          </div>
          {#each config.node_devices || [] as device, i}
          <div class="flex items-start gap-2 rounded-lg bg-white/70 px-3 py-2 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
            <div class="flex-1 grid gap-x-2 gap-y-1 grid-cols-[1fr_2fr]">
              <label class="block">
                <span class="text-[10px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.deviceNameCol')}</span>
                <input
                  type="text"
                  bind:value={device.name}
                  class="mt-0.5 w-full rounded-md bg-white/80 px-2 py-1 text-sm text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Office PC"
                />
              </label>
              <label class="block">
                <span class="text-[10px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.deviceUrlCol')}</span>
                <input
                  type="text"
                  bind:value={device.url}
                  class="mt-0.5 w-full rounded-md bg-white/80 px-2 py-1 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="http://192.168.1.100:47831"
                />
              </label>
              <label class="col-span-2 block">
                <span class="text-[10px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.deviceTokenCol')}</span>
                <input
                  type="password"
                  bind:value={device.token}
                  class="mt-0.5 w-full rounded-md bg-white/80 px-2 py-1 text-xs font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="wr-local-..."
                />
              </label>
            </div>
            <button
              type="button"
              class="text-xs text-red-400 hover:text-red-500 mt-3 shrink-0"
              on:click={() => {
                config.node_devices = config.node_devices.filter((_, j) => j !== i);
                persistConfig();
              }}
            >
              {t('nodeGatewayPage.removeDevice')}
            </button>
          </div>
          {/each}
        </div>
        {/if}

        <!-- API Examples -->
        <div>
          <button
            type="button"
            class="flex w-full items-center justify-between gap-2 rounded-xl bg-white/70 px-3.5 py-2.5 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70"
            on:click={() => (examplesExpanded = !examplesExpanded)}
          >
            <div class="flex items-center gap-2">
              <svg class="w-3.5 h-3.5 text-slate-500 dark:text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4" />
              </svg>
              <span class="text-sm font-medium text-slate-700 dark:text-slate-200">{t('nodeGatewayPage.apiExamples')}</span>
              <span class="text-[10px] text-slate-400">19 endpoints</span>
            </div>
            <svg class="w-4 h-4 text-slate-400 transition-transform {examplesExpanded ? 'rotate-180' : ''}" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
            </svg>
          </button>

          {#if examplesExpanded}
          <div class="mt-2 space-y-2">
            <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.apiExamplesHint')}</p>
            <!-- Category filter buttons -->
            <div class="flex flex-wrap gap-1.5">
              {#each [
                { key: 'all', label: t('nodeGatewayPage.apiCategoryAll') },
                { key: 'report', label: t('nodeGatewayPage.apiCategoryReport') },
                { key: 'timeline', label: t('nodeGatewayPage.apiCategoryTimeline') },
                { key: 'stats', label: t('nodeGatewayPage.apiCategoryStats') },
                { key: 'system', label: t('nodeGatewayPage.apiCategorySystem') },
              ] as cat}
                <button
                  type="button"
                  on:click={() => activeApiCategory = cat.key}
                  class="px-2.5 py-1 rounded-lg text-[11px] font-medium transition-colors
                         {activeApiCategory === cat.key
                           ? 'bg-slate-700 text-white dark:bg-slate-200 dark:text-slate-800'
                           : 'bg-slate-100 text-slate-500 hover:bg-slate-200 dark:bg-slate-800 dark:text-slate-400 dark:hover:bg-slate-700'}"
                >{cat.label}</button>
              {/each}
            </div>

            {#each [
              { cat: 'system', method: 'GET', path: '/health', desc: t('nodeGatewayPage.exampleHealthDesc'), cmd: curlCommand('GET', '/health') },
              { cat: 'system', method: 'GET', path: '/v1/device', desc: t('nodeGatewayPage.exampleDeviceDesc'), cmd: curlCommand('GET', '/v1/device') },
              { cat: 'system', method: 'GET', path: '/v1/storage/stats', desc: t('nodeGatewayPage.exampleStorageStatsDesc'), cmd: curlCommand('GET', '/v1/storage/stats') },
              { cat: 'report', method: 'GET', path: '/v1/reports', desc: t('nodeGatewayPage.exampleReportsDesc'), cmd: curlCommand('GET', '/v1/reports') },
              { cat: 'report', method: 'GET', path: '/v1/reports/:date', desc: t('nodeGatewayPage.exampleReportByDateDesc'), cmd: curlCommand('GET', '/v1/reports/2025-01-15') },
              { cat: 'report', method: 'GET', path: '/v1/reports/generate', desc: t('nodeGatewayPage.exampleGenerateDesc'), cmd: curlCommand('GET', '/v1/reports/generate?date=2025-01-15') },
              { cat: 'report', method: 'POST', path: '/v1/reports/export-markdown', desc: t('nodeGatewayPage.exampleExportDesc'), cmd: curlCommand('POST', '/v1/reports/export-markdown', { date: '2025-01-15', content: '# Report', export_dir: '/path/to/dir' }) },
              { cat: 'report', method: 'GET', path: '/v1/weekly-review', desc: t('nodeGatewayPage.exampleWeeklyReviewDesc'), cmd: curlCommand('GET', '/v1/weekly-review?date_from=2025-01-13&date_to=2025-01-19') },
              { cat: 'timeline', method: 'GET', path: '/v1/timeline/:date', desc: t('nodeGatewayPage.exampleTimelineDesc'), cmd: curlCommand('GET', '/v1/timeline/2025-01-15') },
              { cat: 'timeline', method: 'GET', path: '/v1/activities/:date', desc: t('nodeGatewayPage.exampleActivitiesDesc'), cmd: curlCommand('GET', '/v1/activities/2025-01-15') },
              { cat: 'timeline', method: 'GET', path: '/v1/hourly-summaries/:date', desc: t('nodeGatewayPage.exampleHourlySummariesDesc'), cmd: curlCommand('GET', '/v1/hourly-summaries/2025-01-15') },
              { cat: 'stats', method: 'GET', path: '/v1/stats/today', desc: t('nodeGatewayPage.exampleStatsTodayDesc'), cmd: curlCommand('GET', '/v1/stats/today') },
              { cat: 'stats', method: 'GET', path: '/v1/stats/daily/:date', desc: t('nodeGatewayPage.exampleStatsDailyDesc'), cmd: curlCommand('GET', '/v1/stats/daily/2025-01-15') },
              { cat: 'stats', method: 'GET', path: '/v1/stats/overview', desc: t('nodeGatewayPage.exampleStatsOverviewDesc'), cmd: curlCommand('GET', '/v1/stats/overview?mode=week&date=2025-01-17') },
              { cat: 'stats', method: 'GET', path: '/v1/hourly-app-breakdown/:date', desc: t('nodeGatewayPage.exampleHourlyBreakdownDesc'), cmd: curlCommand('GET', '/v1/hourly-app-breakdown/2025-01-15') },
              { cat: 'stats', method: 'GET', path: '/v1/apps/recent', desc: t('nodeGatewayPage.exampleAppsRecentDesc'), cmd: curlCommand('GET', '/v1/apps/recent') },
              { cat: 'stats', method: 'GET', path: '/v1/apps/category-overview', desc: t('nodeGatewayPage.exampleAppsCategoryDesc'), cmd: curlCommand('GET', '/v1/apps/category-overview') },
              { cat: 'stats', method: 'GET', path: '/v1/categories', desc: t('nodeGatewayPage.exampleCategoriesDesc'), cmd: curlCommand('GET', '/v1/categories') },
              { cat: 'stats', method: 'GET', path: '/v1/categories/semantic', desc: t('nodeGatewayPage.exampleSemanticCategoriesDesc'), cmd: curlCommand('GET', '/v1/categories/semantic') },
            ] as example}
              {#if activeApiCategory === 'all' || activeApiCategory === example.cat}
            <div class="rounded-lg ring-1 ring-slate-200/70 dark:ring-slate-700/70 overflow-hidden">
              <!-- Header: method badge + path + copy button -->
              <div class="flex items-center gap-2 bg-white/90 px-3 py-1.5 dark:bg-slate-800/60">
                <span class="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-bold leading-none
                  {example.method === 'POST'
                    ? 'bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-400'
                    : 'bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-400'}"
                >{example.method}</span>
                <span class="text-xs font-mono font-medium text-slate-700 dark:text-slate-200 truncate">{example.path}</span>
                <span class="ml-auto shrink-0">
                  <button type="button" class="rounded-md px-2 py-0.5 text-[10px] font-medium text-slate-500 hover:bg-slate-100 hover:text-slate-700 dark:text-slate-400 dark:hover:bg-slate-700 dark:hover:text-slate-200 transition-colors" on:click={() => copyCurl(example.cmd)}>
                    {t('nodeGatewayPage.copyCurl')}
                  </button>
                </span>
              </div>
              <!-- Body: description + curl -->
              <div class="border-t border-slate-100 dark:border-slate-700/50">
                <div class="px-3 py-1 text-[11px] text-slate-500 dark:text-slate-400">{example.desc}</div>
                <pre class="mx-2 mb-1.5 rounded-md bg-slate-800/90 px-2.5 py-1.5 text-[10.5px] font-mono leading-relaxed text-emerald-300/90 dark:bg-slate-900/90 overflow-x-auto" on:click={() => copyCurl(example.cmd)}>{example.cmd}</pre>
              </div>
            </div>
              {/if}
            {/each}
          </div>
          {/if}
        </div>
        {/if}
      </div>

      <!-- ========== MCP Server 大卡片 ========== -->
      <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 space-y-4 dark:border-slate-700/80 dark:bg-slate-800/40">
        <!-- MCP Server 标题 + 开关 -->
        <div class="flex items-center justify-between gap-3">
          <div class="flex items-center gap-2">
            <div class="flex h-7 w-7 items-center justify-center rounded-lg bg-emerald-100 dark:bg-emerald-900/30">
              <svg class="w-4 h-4 text-emerald-600 dark:text-emerald-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" />
              </svg>
            </div>
            <div>
              <span class="text-sm font-semibold text-slate-700 dark:text-slate-200">MCP Server</span>
              {#if config.mcp_server_enabled}
                <span class="settings-chip-success ml-1.5">{t('nodeGatewayPage.mcpServerEnabled')}</span>
              {:else}
                <span class="settings-chip-neutral ml-1.5">{t('nodeGatewayPage.mcpServerDisabled')}</span>
              {/if}
            </div>
          </div>
          <button
            type="button"
            on:click={() => {
              config.mcp_server_enabled = !config.mcp_server_enabled;
              persistConfig();
            }}
            disabled={saving}
            class="switch-track {config.mcp_server_enabled ? 'bg-primary-500' : 'bg-slate-300 dark:bg-slate-600'} {saving ? 'opacity-60 cursor-not-allowed' : ''}"
          >
            <span class="switch-thumb {config.mcp_server_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
          </button>
        </div>

        <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.mcpServerDescription')}</p>

        {#if config.mcp_server_enabled}
        <!-- MCP 配置内容 -->
        <div class="space-y-3">
          <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('nodeGatewayPage.mcpServerBinaryHint')}</p>

          <div class="space-y-1.5">
            {#if mcpDbPath}
            <div class="flex items-center justify-between gap-2 rounded-lg bg-white/70 px-3 py-1.5 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
              <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.mcpServerDbPath')}</span>
              <span class="font-mono text-[11px] text-slate-700 dark:text-slate-300 max-w-[60%] truncate" title={mcpDbPath}>{mcpDbPath}</span>
            </div>
            {/if}
            {#if mcpConfigPath}
            <div class="flex items-center justify-between gap-2 rounded-lg bg-white/70 px-3 py-1.5 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
              <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.mcpServerConfigPath')}</span>
              <span class="font-mono text-[11px] text-slate-700 dark:text-slate-300 max-w-[60%] truncate" title={mcpConfigPath}>{mcpConfigPath}</span>
            </div>
            {/if}
            <div class="flex items-center justify-between gap-2 rounded-lg bg-white/70 px-3 py-1.5 ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
              <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('nodeGatewayPage.mcpServerBinaryPath')}</span>
              <span class="font-mono text-[11px] text-slate-700 dark:text-slate-300 max-w-[60%] truncate">work-review-mcp-server</span>
            </div>
          </div>

          <div>
            <div class="flex items-center justify-between mb-1">
              <span class="text-xs font-medium text-slate-600 dark:text-slate-300">{t('nodeGatewayPage.mcpServerConfigTitle')}</span>
              <button type="button" class="settings-chip-button settings-chip-button-active text-[11px]" on:click={copyMcpConfig}>
                {t('nodeGatewayPage.mcpServerCopyConfig')}
              </button>
            </div>
            <p class="text-[11px] text-slate-400 dark:text-slate-500 mb-1.5">{t('nodeGatewayPage.mcpServerConfigHint')}</p>
            <pre class="rounded-lg bg-slate-800 p-3 text-[11px] font-mono text-slate-300 leading-relaxed overflow-x-auto dark:bg-slate-900/80">{mcpConfigJson}</pre>
          </div>
        </div>
        {/if}
      </div>

      <!-- Error -->
      {#if localStatus.lastError}
      <div class="settings-panel-danger">
        <div class="settings-text text-red-700 dark:text-red-400">{t('nodeGatewayPage.lastError')}</div>
        <div class="settings-muted mt-1 break-all">{localStatus.lastError}</div>
      </div>
      {/if}

    </div>
  {/if}
</div>

<style>
</style>
