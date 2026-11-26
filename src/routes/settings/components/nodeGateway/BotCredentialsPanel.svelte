<script lang="ts">
  import { t } from '$lib/i18n/index.ts';
  import { createEventDispatcher } from 'svelte';
  import BotSenderBindingPanel from './BotSenderBindingPanel.svelte';

  type EnabledKey =
    | 'feishu_bot_enabled'
    | 'wecom_bot_enabled'
    | 'dingtalk_bot_enabled';

  type CredentialFieldKey =
    | 'feishu_app_id'
    | 'feishu_app_secret'
    | 'feishu_verification_token'
    | 'feishu_encrypt_key'
    | 'wecom_corp_id'
    | 'wecom_token'
    | 'wecom_encoding_aes_key'
    | 'dingtalk_app_secret';

  interface BotCredentialsConfig {
    feishu_bot_enabled: boolean;
    wecom_bot_enabled: boolean;
    dingtalk_bot_enabled: boolean;
    feishu_app_id: string;
    feishu_app_secret: string;
    feishu_verification_token: string;
    feishu_encrypt_key: string;
    wecom_corp_id: string;
    wecom_token: string;
    wecom_encoding_aes_key: string;
    dingtalk_app_secret: string;
  }

  interface CredentialField {
    key: CredentialFieldKey;
    labelKey: string;
    placeholder?: string;
    secret?: boolean;
    cols?: number;
  }

  export let config: BotCredentialsConfig;
  export let enabledKey: EnabledKey;
  export let titleKey: string;
  export let enabledLabelKey: string;
  export let hintKey: string | null = null;
  export let iconColor = '#6366f1';
  export let iconPath = '';
  export let fields: CredentialField[] = [];
  export let saving = false;
  /** 需要发送者授权的平台；为 null 时不显示授权区块 */
  export let bindingPlatform: 'feishu' | 'dingtalk' | null = null;

  const dispatch = createEventDispatcher<{ save: null }>();
  let secretVisible: Partial<Record<CredentialFieldKey, boolean>> = {};

  function toggle() {
    config[enabledKey] = !config[enabledKey];
    dispatch('save');
  }
</script>

<div class="settings-subsection">
  <div class="flex items-center justify-between gap-3">
    <div class="flex items-center gap-2">
      {#if iconPath}
        <svg class="w-4 h-4" style="color: {iconColor}" viewBox="0 0 24 24" fill="currentColor">
          <path d={iconPath} />
        </svg>
      {/if}
      <span class="text-sm text-slate-700 dark:text-[#c9d1d9]">{t(titleKey)}</span>
      {#if config[enabledKey]}
        <span class="settings-chip-success">{t(enabledLabelKey)}</span>
      {/if}
    </div>
    <button
      type="button"
      on:click={toggle}
      disabled={saving}
      class="switch-track {config[enabledKey] ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'} {saving ? 'opacity-60 cursor-not-allowed' : ''}"
      role="switch"
      aria-label={t(titleKey)}
      aria-checked={config[enabledKey]}
    >
      <span class="switch-thumb {config[enabledKey] ? 'translate-x-5' : 'translate-x-0'}"></span>
    </button>
  </div>
  {#if config[enabledKey]}
    <div class="mt-2 space-y-2">
      <div class="settings-responsive-field-grid grid gap-2">
        {#each fields as field}
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t(field.labelKey)}</span>
            {#if field.secret}
              <div class="mt-0.5 relative">
                {#if secretVisible[field.key]}
                  <input
                    type="text"
                    bind:value={config[field.key]}
                    on:blur={() => dispatch('save')}
                    class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                    placeholder={field.placeholder || ''}
                  />
                {:else}
                  <input
                    type="password"
                    bind:value={config[field.key]}
                    on:blur={() => dispatch('save')}
                    class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                    placeholder={field.placeholder || ''}
                  />
                {/if}
                <button
                  type="button"
                  class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-700 dark:hover:text-[#adbac7]"
                  aria-label={`${t(secretVisible[field.key] ? 'nodeGatewayPage.hideSecret' : 'nodeGatewayPage.showSecret')}: ${t(field.labelKey)}`}
                  on:click={() => (secretVisible[field.key] = !secretVisible[field.key])}
                >
                  {#if secretVisible[field.key]}
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
                  {:else}
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                  {/if}
                </button>
              </div>
            {:else}
              <input
                type="text"
                bind:value={config[field.key]}
                on:blur={() => dispatch('save')}
                class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                placeholder={field.placeholder || ''}
              />
            {/if}
          </label>
        {/each}
      </div>
      {#if hintKey}
        <p class="text-[11px] text-slate-400 dark:text-[#636c76]">{t(hintKey)}</p>
      {/if}
      {#if bindingPlatform}
        <BotSenderBindingPanel platform={bindingPlatform} disabled={saving} />
      {/if}
    </div>
  {/if}
</div>
