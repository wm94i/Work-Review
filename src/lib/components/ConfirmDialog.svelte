<script lang="ts">
  import {
    confirmDialog,
    resolveConfirm,
    type ConfirmTone,
  } from '$lib/stores/confirm.ts';
  import { trapFocus } from '$lib/utils/focusTrap.ts';

  const toneMap: Record<ConfirmTone, { iconBg: string; button: string }> = {
    info: {
      iconBg: 'bg-blue-50 text-blue-600 dark:bg-blue-950/50 dark:text-blue-300',
      button: 'bg-indigo-500 hover:bg-indigo-600 text-white',
    },
    warning: {
      iconBg: 'bg-amber-50 text-amber-600 dark:bg-amber-950/50 dark:text-amber-300',
      button: 'bg-amber-500 hover:bg-amber-600 text-white',
    },
    error: {
      iconBg: 'bg-red-50 text-red-600 dark:bg-red-950/50 dark:text-red-300',
      button: 'bg-red-500 hover:bg-red-600 text-white',
    },
  };

  const dialogState = $derived($confirmDialog);
  const tone = $derived(toneMap[dialogState?.tone ?? 'info']);

  function handleKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape' && dialogState) {
      resolveConfirm(false);
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if dialogState}
  <div class="fixed inset-0 z-[200] flex items-center justify-center px-4 py-6 bg-slate-950/48 backdrop-blur-md animate-fadeIn">
    <div
      use:trapFocus
      class="w-full max-w-md rounded-[var(--radius-lg)] border border-slate-200 dark:border-[#30363d] bg-white dark:bg-[#161b22] shadow-2xl shadow-slate-950/24 dark:shadow-black/50 p-6"
      role="dialog"
      aria-modal="true"
      aria-labelledby="confirm-dialog-title"
    >
      <div class="flex items-start gap-4">
        <div class={`flex h-12 w-12 shrink-0 items-center justify-center rounded-[var(--radius-md)] ${tone.iconBg}`}>
          <svg class="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v3.75m0 3.75h.008v.008H12v-.008ZM10.29 3.86 1.82 18a2 2 0 0 0 1.72 3h16.92a2 2 0 0 0 1.72-3L13.71 3.86a2 2 0 0 0-3.42 0Z" />
          </svg>
        </div>
        <div class="min-w-0 flex-1">
          <h3 id="confirm-dialog-title" class="text-lg font-semibold text-slate-900 dark:text-[#e6edf3]">
            {dialogState.title}
          </h3>
          <p class="mt-2 text-sm leading-6 text-slate-500 dark:text-[#7d8590] whitespace-pre-line">
            {dialogState.message}
          </p>
        </div>
      </div>

      <div class="mt-6 flex items-center justify-end gap-3">
        <button
          type="button"
          onclick={() => resolveConfirm(false)}
          class="inline-flex min-h-11 items-center justify-center rounded-[var(--radius-md)] border border-slate-200 dark:border-[#30363d] bg-white dark:bg-[#21262d] px-5 text-sm font-medium text-slate-700 dark:text-[#adbac7] transition-colors hover:bg-slate-50 dark:hover:bg-[#30363d]"
        >
          {dialogState.cancelText}
        </button>
        <button
          type="button"
          onclick={() => resolveConfirm(true)}
          class={`inline-flex min-h-11 items-center justify-center rounded-[var(--radius-md)] px-5 text-sm font-medium transition-colors ${tone.button}`}
        >
          {dialogState.confirmText}
        </button>
      </div>
    </div>
  </div>
{/if}
