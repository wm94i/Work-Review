<script lang="ts">
  import { untrack } from 'svelte';
  import { t } from '$lib/i18n/index.ts';
  import { invoke } from '@tauri-apps/api/core';

  type BotSenderPlatform = 'feishu' | 'dingtalk' | 'wecom';

  interface BotSenderBindingStatus {
    allowedCount: number;
    allowedSenders: string[];
    bindCode: string | null;
    bindCodeExpiresAt: number | null;
    bindCodeExpired: boolean;
  }

  interface Props {
    platform: BotSenderPlatform;
    disabled?: boolean;
  }

  /** 与后端 bot_common::BOT_BIND_CODE_TTL_SECONDS / BOT_BIND_MAX_FAILED_ATTEMPTS 保持一致 */
  const BIND_CODE_TTL_MINUTES = 10;
  const BIND_MAX_FAILED_ATTEMPTS = 5;
  /** 绑定码待使用期间轮询状态，让在聊天中完成的绑定及时反映到设置页 */
  const PENDING_REFRESH_MS = 5000;

  let { platform, disabled = false }: Props = $props();

  let status = $state<BotSenderBindingStatus | null>(null);
  let busy = $state(false);
  let confirmingClear = $state(false);
  let message = $state('');
  let messageType = $state<'success' | 'error'>('success');
  let confirmTimer: ReturnType<typeof setTimeout> | null = null;
  let destroyed = false;
  let loadSeq = 0;

  const pendingCode = $derived(Boolean(status?.bindCode && !status.bindCodeExpired));
  const expiresAtLabel = $derived(
    status?.bindCodeExpiresAt
      ? new Date(status.bindCodeExpiresAt * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      : '',
  );

  function resetConfirmClear(): void {
    confirmingClear = false;
    if (confirmTimer) {
      clearTimeout(confirmTimer);
      confirmTimer = null;
    }
  }

  async function loadStatus(targetPlatform: BotSenderPlatform = platform): Promise<void> {
    const seq = ++loadSeq;
    const prevAllowed = status?.allowedCount ?? 0;
    const prevPending = Boolean(status?.bindCode && !status.bindCodeExpired);
    try {
      const next = await invoke<BotSenderBindingStatus>('get_bot_sender_binding_status', {
        platform: targetPlatform,
      });
      if (destroyed || seq !== loadSeq) return;
      status = next;
      const nextPending = Boolean(next.bindCode && !next.bindCodeExpired);
      if (messageType === 'error' || (prevPending && !nextPending && next.allowedCount > prevAllowed)) {
        message = '';
      }
      if (next.allowedCount === 0) {
        resetConfirmClear();
      }
    } catch (e) {
      if (destroyed || seq !== loadSeq) return;
      messageType = 'error';
      message = t('nodeGatewayPage.senderBindingFailed', { error: String(e) });
    }
  }

  async function generateCode(): Promise<void> {
    resetConfirmClear();
    busy = true;
    try {
      await invoke('generate_bot_sender_bind_code', { platform });
      await loadStatus(platform);
      if (destroyed) return;
      messageType = 'success';
      message = t('nodeGatewayPage.senderBindingGenerated');
    } catch (e) {
      if (destroyed) return;
      messageType = 'error';
      message = t('nodeGatewayPage.senderBindingFailed', { error: String(e) });
    } finally {
      busy = false;
    }
  }

  async function clearBindings(): Promise<void> {
    if (!confirmingClear) {
      confirmingClear = true;
      if (confirmTimer) clearTimeout(confirmTimer);
      confirmTimer = setTimeout(() => {
        confirmingClear = false;
        confirmTimer = null;
      }, 5000);
      return;
    }
    resetConfirmClear();
    busy = true;
    try {
      await invoke('clear_bot_sender_bindings', { platform });
      await loadStatus(platform);
      if (destroyed) return;
      messageType = 'success';
      message = t('nodeGatewayPage.senderBindingCleared');
    } catch (e) {
      if (destroyed) return;
      messageType = 'error';
      message = t('nodeGatewayPage.senderBindingFailed', { error: String(e) });
    } finally {
      busy = false;
      resetConfirmClear();
    }
  }

  $effect(() => {
    const currentPlatform = platform;
    destroyed = false;
    untrack(() => {
      void loadStatus(currentPlatform);
    });
    return () => {
      destroyed = true;
      resetConfirmClear();
    };
  });

  $effect(() => {
    if (!pendingCode) return;
    const currentPlatform = platform;
    const timer = setInterval(() => {
      void loadStatus(currentPlatform);
    }, PENDING_REFRESH_MS);
    return () => {
      clearInterval(timer);
    };
  });
</script>

<div class="rounded-lg bg-blue-50/70 px-3 py-2 ring-1 ring-blue-100 dark:bg-blue-950/20 dark:ring-blue-900/50">
  <div class="flex items-start justify-between gap-3">
    <div class="min-w-0">
      <div class="text-[11px] font-medium text-blue-700 dark:text-blue-300">{t('nodeGatewayPage.senderBindingTitle')}</div>
      <div class="mt-0.5 text-[11px] text-blue-600/80 dark:text-blue-300/80">
        {t('nodeGatewayPage.senderBindingHint', { minutes: BIND_CODE_TTL_MINUTES, attempts: BIND_MAX_FAILED_ATTEMPTS })}
      </div>
      {#if status}
        <div class="mt-1 text-[11px] {status.allowedCount > 0 ? 'text-slate-600 dark:text-[#adbac7]' : 'text-amber-600 dark:text-amber-300'}">
          {status.allowedCount > 0
            ? t('nodeGatewayPage.senderBindingCount', { count: status.allowedCount })
            : t('nodeGatewayPage.senderBindingNone')}
        </div>
        {#if status.allowedSenders.length > 0}
          <ul class="mt-0.5 flex flex-wrap gap-1">
            {#each status.allowedSenders as sender (sender)}
              <li class="settings-chip-neutral font-mono text-[10px]">{sender}</li>
            {/each}
          </ul>
        {/if}
        {#if pendingCode}
          <div class="mt-1 font-mono text-sm font-semibold text-blue-800 dark:text-blue-200">/bind {status.bindCode}</div>
          {#if expiresAtLabel}
            <div class="text-[11px] text-blue-600/80 dark:text-blue-300/80">
              {t('nodeGatewayPage.senderBindingExpiresAt', { time: expiresAtLabel })}
            </div>
          {/if}
        {:else if status.bindCodeExpired}
          <div class="mt-1 text-[11px] text-amber-600 dark:text-amber-300">{t('nodeGatewayPage.senderBindingExpired')}</div>
        {/if}
      {/if}
      {#if message}
        <div class="mt-1 text-[11px] {messageType === 'success' ? 'text-emerald-600 dark:text-emerald-400' : 'text-red-600 dark:text-red-400'}" role="status">
          {message}
        </div>
      {/if}
    </div>
    <div class="flex shrink-0 flex-col items-end gap-1.5">
      <button
        type="button"
        class="rounded-md bg-blue-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-blue-700 disabled:opacity-60"
        onclick={generateCode}
        disabled={disabled || busy}
      >
        {t('nodeGatewayPage.senderBindingGenerate')}
      </button>
      {#if status && status.allowedCount > 0}
        <button
          type="button"
          class="rounded-md px-2 py-1 text-[11px] text-red-600 ring-1 ring-red-200 hover:bg-red-50 disabled:opacity-60 dark:text-red-400 dark:ring-red-900/60 dark:hover:bg-red-950/30"
          onclick={clearBindings}
          disabled={disabled || busy}
        >
          {confirmingClear ? t('nodeGatewayPage.senderBindingClearConfirm') : t('nodeGatewayPage.senderBindingClear')}
        </button>
      {/if}
    </div>
  </div>
</div>
