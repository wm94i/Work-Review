<script lang="ts">
  import { toast, clearToast, type ToastType } from '$lib/stores/toast.ts';

  const iconMap: Record<ToastType, string> = {
    success: 'M5 13l4 4L19 7',
    error: 'M6 18L18 6M6 6l12 12',
    warning: 'M12 9v4m0 4h.01M10.29 3.86l-7.5 13A1 1 0 003.65 18h16.7a1 1 0 00.86-1.5l-7.5-13a1 1 0 00-1.72 0z',
    info: 'M13 16h-1v-4h-1m1-4h.01M12 22a10 10 0 110-20 10 10 0 010 20z',
  };

  // Alger Music 式语义 tint 玻璃：半透明主题色底 + 亮暗各自前景 + 毛玻璃
  const colorMap: Record<ToastType, string> = {
    success: 'bg-[rgba(34,197,94,0.15)] dark:bg-[rgba(34,197,94,0.18)] text-[#16a34a] dark:text-[#4ade80]',
    error: 'bg-[rgba(239,68,68,0.12)] dark:bg-[rgba(239,68,68,0.18)] text-[#dc2626] dark:text-[#f87171]',
    warning: 'bg-[rgba(245,158,11,0.12)] dark:bg-[rgba(245,158,11,0.18)] text-[#d97706] dark:text-[#fbbf24]',
    info: 'bg-[rgba(59,130,246,0.12)] dark:bg-[rgba(59,130,246,0.18)] text-[#2563eb] dark:text-[#60a5fa]',
  };

  const toastState = $derived($toast);
  const iconPath = $derived(iconMap[toastState?.type ?? 'info']);
  const toastClass = $derived(colorMap[toastState?.type ?? 'info']);
</script>

{#if toastState}
  <div
    class="fixed inset-x-0 bottom-6 z-[210] flex justify-center px-4 animate-fadeIn pointer-events-none"
    role="status"
    aria-live="polite"
  >
    <button
      type="button"
      onclick={() => clearToast()}
      class={`toast-glass max-w-xl min-h-11 px-[18px] py-2.5 text-sm font-medium flex items-center gap-2 pointer-events-auto ${toastClass}`}
    >
      <svg class="w-4 h-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d={iconPath} />
      </svg>
      <span class="leading-snug text-center break-words min-w-0">{toastState.message}</span>
    </button>
  </div>
{/if}
