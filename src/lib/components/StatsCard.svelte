<script lang="ts">
  export let title = '';
  export let value = '';
  export let icon = '';
  type StatsCardColor = 'indigo' | 'emerald' | 'amber' | 'rose' | 'blue' | 'violet';

  export let color: StatsCardColor = 'indigo';
  export let href: string | null = null;
  export let embedded = false;
  export let subtitle: string | null = null;
  /** 标记为概览 KPI 卡：供 ui-style-c 紧凑样式精准命中，避免误伤 subtitle */
  export let dataKpiCard = false;

  const iconColors: Record<StatsCardColor, string> = {
    indigo: 'text-indigo-500 dark:text-indigo-400',
    emerald: 'text-emerald-500 dark:text-emerald-400',
    amber: 'text-amber-500 dark:text-amber-400',
    rose: 'text-rose-500 dark:text-rose-400',
    blue: 'text-blue-500 dark:text-blue-400',
    violet: 'text-violet-500 dark:text-violet-400',
  };

  const iconBgs: Record<StatsCardColor, string> = {
    indigo: 'bg-indigo-50 dark:bg-indigo-950/40',
    emerald: 'bg-emerald-50 dark:bg-emerald-950/40',
    amber: 'bg-amber-50 dark:bg-amber-950/40',
    rose: 'bg-rose-50 dark:bg-rose-950/40',
    blue: 'bg-blue-50 dark:bg-blue-950/40',
    violet: 'bg-violet-50 dark:bg-violet-950/40',
  };

  $: cardClass = embedded
    ? 'block min-h-[108px] rounded-lg bg-slate-50/88 px-4 py-4 dark:bg-[#161b22]/30'
    : 'block min-h-[116px] rounded-lg border border-slate-200/80 bg-white p-5 dark:border-[#30363d]/60 dark:bg-[#21262d]/80';
  $: iconClass = embedded
    ? `flex h-10 w-10 items-center justify-center rounded-lg ${iconBgs[color]} ${iconColors[color]} shrink-0`
    : `flex h-11 w-11 items-center justify-center rounded-lg border border-white/80 shadow-sm dark:shadow-none dark:border-[#30363d]/80 ${iconBgs[color]} ${iconColors[color]} shrink-0`;
  $: valueClass = embedded
    ? 'mt-4 whitespace-nowrap text-[1.9rem] font-semibold leading-none tabular-nums text-slate-900 dark:text-[#e6edf3]'
    : 'mt-6 whitespace-nowrap text-[2.15rem] font-semibold leading-none tabular-nums text-slate-900 dark:text-[#e6edf3]';
  $: interactiveClass = embedded
    ? 'transition-all duration-200 hover:bg-slate-100/90 hover:-translate-y-0.5 dark:hover:bg-slate-900/50'
    : 'transition-all duration-200 hover:shadow-md dark:shadow-[0_4px_12px_rgba(0,0,0,0.4)] hover:-translate-y-0.5 cursor-pointer group';
</script>

{#if href}
  <a
    {href}
    class="{cardClass} {interactiveClass}"
    data-kpi-card={dataKpiCard ? '' : undefined}
  >
    <div class="flex h-full items-center justify-between gap-4">
      <div class="min-w-0 flex-1">
        <span class="text-[13px] font-medium text-slate-500 dark:text-[#9c9ca6] leading-none">{title}</span>
        <p class={valueClass}>{value}</p>
        {#if subtitle}
          <p class="mt-1.5 text-xs font-medium leading-none tabular-nums text-slate-500 dark:text-[#9c9ca6]">{subtitle}</p>
        {/if}
      </div>
      <div class={iconClass}>
        {#if icon === 'duration'}
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M12 7v5l3 2m6-2a9 9 0 11-18 0 9 9 0 0118 0z" />
          </svg>
        {:else if icon === 'focus'}
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M4 10V7a3 3 0 013-3h3m4 0h3a3 3 0 013 3v3m0 4v3a3 3 0 01-3 3h-3m-4 0H7a3 3 0 01-3-3v-3" />
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M9 12h6" />
          </svg>
        {:else if icon === 'browser'}
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M3 12h18M12 3a15.3 15.3 0 014 9 15.3 15.3 0 01-4 9 15.3 15.3 0 01-4-9 15.3 15.3 0 014-9Z" />
            <circle cx="12" cy="12" r="9" stroke-width="1.8" />
          </svg>
        {:else if icon === 'apps'}
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <rect x="4" y="4" width="7" height="7" rx="2" stroke-width="1.8" />
            <rect x="13" y="4" width="7" height="7" rx="2" stroke-width="1.8" />
            <rect x="4" y="13" width="7" height="7" rx="2" stroke-width="1.8" />
            <rect x="13" y="13" width="7" height="7" rx="2" stroke-width="1.8" />
          </svg>
        {/if}
      </div>
    </div>
  </a>
{:else}
  <div class={cardClass} data-kpi-card={dataKpiCard ? '' : undefined}>
    <div class="flex h-full items-center justify-between gap-4">
      <div class="min-w-0 flex-1">
        <span class="text-[13px] font-medium text-slate-500 dark:text-[#9c9ca6] leading-none">{title}</span>
        <p class={valueClass}>{value}</p>
        {#if subtitle}
          <p class="mt-1.5 text-xs font-medium leading-none tabular-nums text-slate-500 dark:text-[#9c9ca6]">{subtitle}</p>
        {/if}
      </div>
      <div class={iconClass}>
        {#if icon === 'duration'}
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M12 7v5l3 2m6-2a9 9 0 11-18 0 9 9 0 0118 0z" />
          </svg>
        {:else if icon === 'focus'}
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M4 10V7a3 3 0 013-3h3m4 0h3a3 3 0 013 3v3m0 4v3a3 3 0 01-3 3h-3m-4 0H7a3 3 0 01-3-3v-3" />
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M9 12h6" />
          </svg>
        {:else if icon === 'browser'}
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M3 12h18M12 3a15.3 15.3 0 014 9 15.3 15.3 0 01-4 9 15.3 15.3 0 01-4-9 15.3 15.3 0 014-9Z" />
            <circle cx="12" cy="12" r="9" stroke-width="1.8" />
          </svg>
        {:else if icon === 'apps'}
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <rect x="4" y="4" width="7" height="7" rx="2" stroke-width="1.8" />
            <rect x="13" y="4" width="7" height="7" rx="2" stroke-width="1.8" />
            <rect x="4" y="13" width="7" height="7" rx="2" stroke-width="1.8" />
            <rect x="13" y="13" width="7" height="7" rx="2" stroke-width="1.8" />
          </svg>
        {/if}
      </div>
    </div>
  </div>
{/if}
