<script lang="ts">
  import { t } from '$lib/i18n/index.ts';
  import { createEventDispatcher } from 'svelte';
  import BotSenderBindingPanel from './BotSenderBindingPanel.svelte';

  interface WecomConfig {
    wecom_bot_enabled: boolean;
    wecom_bot_id: string;
    wecom_bot_secret: string;
    wecom_corp_id: string;
    wecom_token: string;
    wecom_encoding_aes_key: string;
  }

  interface WecomBotStatus {
    starting: boolean;
    running: boolean;
    lastError: string | null;
    longConnectionConfigured?: boolean;
    callbackConfigured?: boolean;
  }

  export let config: WecomConfig;
  export let wecomBotStatus: WecomBotStatus | null = null;
  export let saving = false;

  const dispatch = createEventDispatcher<{
    save: null;
    startWecomPolling: null;
  }>();

  let secretVisible = false;
  let tokenVisible = false;
  let aesVisible = false;
  let showCallback = false;

  function toggle() {
    config.wecom_bot_enabled = !config.wecom_bot_enabled;
    dispatch('save');
    if (config.wecom_bot_enabled) {
      dispatch('startWecomPolling');
    }
  }

  $: callbackOpen = showCallback
    || Boolean(config.wecom_corp_id)
    || Boolean(config.wecom_token)
    || Boolean(config.wecom_encoding_aes_key);
</script>

<div class="settings-subsection">
  <div class="flex items-center justify-between gap-3">
    <div class="flex items-center gap-2">
      <svg class="w-4 h-4" style="color: #07C160" viewBox="0 0 24 24" fill="currentColor">
        <path d="M9.5 4C5.36 4 2 6.91 2 10.5c0 2.08 1.13 3.93 2.88 5.13-.14.8-.5 2-.5 2l2-.5c.5.24 1.04.42 1.6.54-.13-.5-.2-1.02-.2-1.55C8.78 13.69 12 12 16 12c.3 0 .6.01.89.04C16.96 7.56 13.64 4 9.5 4z" />
      </svg>
      <span class="text-sm text-slate-700 dark:text-[#c9d1d9]">{t('nodeGatewayPage.wecomBot')}</span>
      {#if config.wecom_bot_enabled}
        <span class="settings-chip-success">{t('nodeGatewayPage.wecomEnabled')}</span>
      {/if}
    </div>
    <button
      type="button"
      on:click={toggle}
      disabled={saving}
      class="switch-track {config.wecom_bot_enabled ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'} {saving ? 'opacity-60 cursor-not-allowed' : ''}"
      role="switch"
      aria-label={t('nodeGatewayPage.wecomBot')}
      aria-checked={config.wecom_bot_enabled}
    >
      <span class="switch-thumb {config.wecom_bot_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
    </button>
  </div>
  {#if config.wecom_bot_enabled}
    <div class="mt-2 space-y-2">
      <div class="settings-responsive-field-grid grid gap-2">
        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('nodeGatewayPage.wecomBotId')}</span>
          <input
            type="text"
            bind:value={config.wecom_bot_id}
            on:blur={() => dispatch('save')}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
            placeholder="AIBOTID"
          />
        </label>
        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('nodeGatewayPage.wecomBotSecret')}</span>
          <div class="mt-0.5 relative">
            {#if secretVisible}
              <input
                type="text"
                bind:value={config.wecom_bot_secret}
                on:blur={() => dispatch('save')}
                class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                placeholder="Secret"
              />
            {:else}
              <input
                type="password"
                bind:value={config.wecom_bot_secret}
                on:blur={() => dispatch('save')}
                class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                placeholder="Secret"
              />
            {/if}
            <button
              type="button"
              class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-700 dark:hover:text-[#adbac7]"
              aria-label={`${t(secretVisible ? 'nodeGatewayPage.hideSecret' : 'nodeGatewayPage.showSecret')}: ${t('nodeGatewayPage.wecomBotSecret')}`}
              on:click={() => (secretVisible = !secretVisible)}
            >
              {#if secretVisible}
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
              {:else}
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
              {/if}
            </button>
          </div>
        </label>
      </div>
      {#if wecomBotStatus}
        {#if wecomBotStatus.starting}
          <div class="flex items-center gap-1.5 text-[11px] text-blue-500 dark:text-blue-400">
            <div class="animate-spin h-3 w-3 border-[1.5px] border-blue-400 border-t-transparent rounded-full"></div>
            {t('nodeGatewayPage.wecomConnecting')}
          </div>
        {:else if wecomBotStatus.running}
          <div class="flex items-center gap-1.5 text-[11px] text-emerald-600 dark:text-emerald-400">
            <span class="inline-block h-1.5 w-1.5 rounded-full bg-emerald-500"></span>
            {t('nodeGatewayPage.wecomConnected')}
          </div>
        {:else if wecomBotStatus.lastError}
          <div class="flex items-center gap-1.5 text-[11px] text-amber-600 dark:text-amber-400">
            <span class="inline-block h-1.5 w-1.5 rounded-full bg-amber-500"></span>
            {wecomBotStatus.lastError}
          </div>
        {:else if wecomBotStatus.callbackConfigured && !wecomBotStatus.longConnectionConfigured}
          <div class="flex items-center gap-1.5 text-[11px] text-slate-500 dark:text-[#7d8590]">
            {t('nodeGatewayPage.wecomCallbackOnly')}
          </div>
        {/if}
      {/if}
      <p class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('nodeGatewayPage.wecomBotHint')}</p>
      <BotSenderBindingPanel platform="wecom" disabled={saving} />
      <button
        type="button"
        class="text-[11px] text-slate-500 underline-offset-2 hover:underline dark:text-[#7d8590]"
        on:click={() => (showCallback = !showCallback)}
      >
        {t('nodeGatewayPage.wecomCallbackSection')}
      </button>
      {#if callbackOpen}
        <div class="settings-responsive-field-grid grid gap-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('nodeGatewayPage.wecomCorpId')}</span>
            <input
              type="text"
              bind:value={config.wecom_corp_id}
              on:blur={() => dispatch('save')}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
              placeholder="Corp ID"
            />
          </label>
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('nodeGatewayPage.wecomToken')}</span>
            <div class="mt-0.5 relative">
              {#if tokenVisible}
                <input
                  type="text"
                  bind:value={config.wecom_token}
                  on:blur={() => dispatch('save')}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Token"
                />
              {:else}
                <input
                  type="password"
                  bind:value={config.wecom_token}
                  on:blur={() => dispatch('save')}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Token"
                />
              {/if}
              <button
                type="button"
                class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-700 dark:hover:text-[#adbac7]"
                aria-label={`${t(tokenVisible ? 'nodeGatewayPage.hideSecret' : 'nodeGatewayPage.showSecret')}: ${t('nodeGatewayPage.wecomToken')}`}
                on:click={() => (tokenVisible = !tokenVisible)}
              >
                {#if tokenVisible}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
                {:else}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                {/if}
              </button>
            </div>
          </label>
          <label class="block md:col-span-2">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('nodeGatewayPage.wecomEncodingAesKey')}</span>
            <div class="mt-0.5 relative">
              {#if aesVisible}
                <input
                  type="text"
                  bind:value={config.wecom_encoding_aes_key}
                  on:blur={() => dispatch('save')}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="EncodingAESKey"
                />
              {:else}
                <input
                  type="password"
                  bind:value={config.wecom_encoding_aes_key}
                  on:blur={() => dispatch('save')}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="EncodingAESKey"
                />
              {/if}
              <button
                type="button"
                class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-700 dark:hover:text-[#adbac7]"
                aria-label={`${t(aesVisible ? 'nodeGatewayPage.hideSecret' : 'nodeGatewayPage.showSecret')}: ${t('nodeGatewayPage.wecomEncodingAesKey')}`}
                on:click={() => (aesVisible = !aesVisible)}
              >
                {#if aesVisible}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
                {:else}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                {/if}
              </button>
            </div>
          </label>
        </div>
        <p class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('nodeGatewayPage.wecomCallbackHint')}</p>
      {/if}
    </div>
  {/if}
</div>
