<script lang="ts">
  import { createEventDispatcher, tick } from 'svelte';
  import { formatDurationLocalized, t } from '$lib/i18n/index.ts';
  import { trapFocus } from '$lib/utils/focusTrap.ts';
  import { portalToBody } from '$lib/actions/portal.ts';
  import {
    formatHourRange,
    getFullSummary,
    getMainApps,
    getPrimarySummary,
    getSecondarySummary,
    getSummaryDisplayParts,
    getSummaryRhythmTone,
    orderHourlySummariesForDisplay,
    type HourlySummaryRecord,
  } from './summaryPresentation.ts';

  export let open = false;
  export let date = '';
  export let summaries: HourlySummaryRecord[] = [];
  export let loading = false;
  export let refreshing = false;
  export let error = null as string | null;

  const dispatch = createEventDispatcher<{ close: void }>();
  let closeButton: HTMLButtonElement | null = null;
  let expandedHours = new Set<number>();
  let previousOpen = false;
  let previousDate = date;
  let previousSummarySignature = '';

  $: summarySignature = JSON.stringify(
    summaries.map((summary) => [
      summary.hour,
      summary.total_duration,
      summary.activity_count,
      summary.summary,
      summary.main_apps,
    ])
  );

  $: displaySummaries = orderHourlySummariesForDisplay(summaries);

  $: peakDuration = summaries.reduce(
    (max, summary) => Math.max(max, summary.total_duration || 0),
    0
  );

  $: if (summarySignature !== previousSummarySignature) {
    previousSummarySignature = summarySignature;
    expandedHours = new Set();
  }

  $: if (date !== previousDate) {
    previousDate = date;
    expandedHours = new Set();
  }

  $: if (open && !previousOpen) {
    previousOpen = true;
    focusCloseButton();
  } else if (!open && previousOpen) {
    previousOpen = false;
  }

  async function focusCloseButton() {
    await tick();
    closeButton?.focus();
  }

  function requestClose() {
    dispatch('close');
  }

  function handleOverlayKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      requestClose();
    }
  }

  function toggleExpand(hour: number) {
    const next = new Set(expandedHours);
    if (next.has(hour)) {
      next.delete(hour);
    } else {
      next.add(hour);
    }
    expandedHours = next;
  }

  function needsExpand(summary: HourlySummaryRecord) {
    const full = getFullSummary(summary.summary);
    const primary = getPrimarySummary(summary.summary);
    const secondary = getSecondarySummary(summary.summary);
    const displayed = [primary, secondary].filter(Boolean).join('');
    return full.length > displayed.length + 2;
  }

  function isPeakSummary(summary: HourlySummaryRecord) {
    return summaries.length > 1
      && peakDuration > 0
      && summary.total_duration === peakDuration;
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="hourly-summary-overlay"
    use:portalToBody
    role="presentation"
    on:click|self={requestClose}
    on:keydown={handleOverlayKeydown}
  >
    <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
    <aside
      class="hourly-summary-drawer"
      use:trapFocus
      role="dialog"
      aria-modal="true"
      aria-labelledby="hourly-summary-title"
    >
      <header class="hourly-summary-header">
        <div class="hourly-summary-heading">
          <span class="hourly-summary-kicker">{date}</span>
          <h2 id="hourly-summary-title">{t('timelineSummary.title')}</h2>
          <p>{t('timelineSummary.summaryCount', { count: summaries.length })}</p>
        </div>
        <button
          bind:this={closeButton}
          type="button"
          class="hourly-summary-close"
          aria-label={t('window.close')}
          on:click={requestClose}
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" aria-hidden="true">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </header>

      <div class="hourly-summary-body">
        {#if refreshing && summaries.length > 0}
          <div class="hourly-summary-refreshing" aria-live="polite">
            <span class="hourly-summary-spinner" aria-hidden="true"></span>
            <span>{t('common.loading')}</span>
          </div>
        {/if}

        {#if error}
          <p class="hourly-summary-refresh-error" role="status">{error}</p>
        {/if}

        {#if (loading || refreshing) && summaries.length === 0}
          <div class="hourly-summary-state" aria-live="polite">
            <span class="hourly-summary-spinner" aria-hidden="true"></span>
            <p>{t('common.loading')}</p>
          </div>
        {:else if summaries.length === 0}
          <div class="hourly-summary-state">
            <span class="hourly-summary-empty-icon" aria-hidden="true">◷</span>
            <p>{t('timelineSummary.noData')}</p>
          </div>
        {:else}
          <div class="hourly-summary-list">
            {#each displaySummaries as summary (summary.hour)}
              {@const apps = getMainApps(summary.main_apps)}
              {@const expanded = expandedHours.has(summary.hour)}
              {@const display = getSummaryDisplayParts(summary.summary, expanded)}
              {@const rhythmTone = getSummaryRhythmTone(summary.total_duration)}
              {@const peak = isPeakSummary(summary)}
              <article class:hourly-summary-item-peak={peak} class="hourly-summary-item">
                <div class="hourly-summary-item-header">
                  <div>
                    <h3>{formatHourRange(summary.hour)}</h3>
                    <p>
                      <span>
                        {t('timelineSummary.activeDuration', {
                          duration: formatDurationLocalized(summary.total_duration || 0, { compact: true }),
                        })}
                      </span>
                      <span aria-hidden="true"> · </span>
                      <span>{t('timelineSummary.activityCount', { count: summary.activity_count || 0 })}</span>
                    </p>
                  </div>
                  <div class="hourly-summary-badges">
                    {#if peak}
                      <span class="hourly-summary-peak">{t('timelineSummary.peakBadge')}</span>
                    {/if}
                    <span class={`hourly-summary-rhythm hourly-summary-rhythm-${rhythmTone}`}>
                      {t(`timelineSummary.rhythm.${rhythmTone}`)}
                    </span>
                  </div>
                </div>

                {#if display.primary}
                  <p class="hourly-summary-primary">{display.primary}</p>
                {/if}

                {#if display.secondary}
                  <p class="hourly-summary-secondary">{display.secondary}</p>
                {/if}

                {#if needsExpand(summary)}
                  <button
                    type="button"
                    class="hourly-summary-expand"
                    aria-expanded={expanded}
                    on:click={() => toggleExpand(summary.hour)}
                  >
                    {expanded ? t('timelineSummary.collapse') : t('timelineSummary.expandFull')}
                    <svg class:hourly-summary-expand-open={expanded} viewBox="0 0 24 24" fill="none" stroke="currentColor" aria-hidden="true">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
                    </svg>
                  </button>
                {/if}

                {#if apps.length > 0}
                  <div class="hourly-summary-apps" aria-label={t('timelineSummary.appsCount', { count: apps.length })}>
                    {#each apps as app}
                      <span>{app}</span>
                    {/each}
                  </div>
                {/if}
              </article>
            {/each}
          </div>
        {/if}
      </div>
    </aside>
  </div>
{/if}

<style>
  .hourly-summary-overlay {
    position: fixed;
    inset: 0;
    z-index: 145;
    display: flex;
    justify-content: flex-end;
    padding: 1rem;
    background: rgba(15, 23, 42, 0.48);
  }

  .hourly-summary-drawer {
    width: min(42rem, 100%);
    height: calc(100vh - 2rem);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border: 1px solid rgba(148, 163, 184, 0.24);
    border-radius: var(--radius-lg);
    background: #fff;
    box-shadow: -18px 0 48px rgba(15, 23, 42, 0.18);
  }

  .hourly-summary-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    padding: 1.35rem 1.4rem 1.2rem;
    border-bottom: 1px solid rgba(148, 163, 184, 0.18);
  }

  .hourly-summary-heading h2,
  .hourly-summary-heading p,
  .hourly-summary-item h3,
  .hourly-summary-item p {
    margin: 0;
  }

  .hourly-summary-kicker {
    display: block;
    margin-bottom: 0.28rem;
    color: #78716c;
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.08em;
  }

  .hourly-summary-heading h2 {
    color: #1c1917;
    font-size: 1.18rem;
    line-height: 1.35;
  }

  .hourly-summary-heading p {
    margin-top: 0.35rem;
    color: #78716c;
    font-size: 0.82rem;
  }

  .hourly-summary-close {
    width: 2.35rem;
    height: 2.35rem;
    display: inline-flex;
    flex: 0 0 auto;
    align-items: center;
    justify-content: center;
    border: 1px solid rgba(120, 113, 108, 0.16);
    border-radius: var(--radius-lg);
    color: #57534e;
    background: rgba(250, 250, 249, 0.92);
    cursor: pointer;
  }

  .hourly-summary-close:hover {
    color: #1c1917;
    background: #f5f5f4;
  }

  .hourly-summary-close:focus-visible,
  .hourly-summary-expand:focus-visible {
    outline: 2px solid rgba(87, 83, 78, 0.4);
    outline-offset: 2px;
  }

  .hourly-summary-close svg {
    width: 1.1rem;
    height: 1.1rem;
  }

  .hourly-summary-body {
    min-height: 0;
    flex: 1;
    overflow-y: auto;
    padding: 1.1rem 1.25rem 1.4rem;
    background: #fafaf9;
  }

  .hourly-summary-refreshing,
  .hourly-summary-refresh-error {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    margin: 0 0 0.8rem;
    color: #78716c;
    font-size: 0.76rem;
  }

  .hourly-summary-refresh-error {
    padding: 0.6rem 0.75rem;
    border: 1px solid rgba(248, 113, 113, 0.2);
    border-radius: var(--radius-lg);
    color: #b91c1c;
    background: rgba(254, 242, 242, 0.78);
  }

  .hourly-summary-spinner {
    width: 0.9rem;
    height: 0.9rem;
    border: 2px solid rgba(120, 113, 108, 0.24);
    border-top-color: #57534e;
    border-radius: var(--radius-full);
    animation: hourly-summary-spin 700ms linear infinite;
  }

  .hourly-summary-state {
    min-height: 17rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.8rem;
    color: #78716c;
    text-align: center;
  }

  .hourly-summary-empty-icon {
    font-size: 2rem;
    color: #a8a29e;
  }

  .hourly-summary-list {
    display: grid;
    gap: 0.8rem;
  }

  .hourly-summary-item {
    padding: 1rem 1.05rem;
    border: 1px solid rgba(120, 113, 108, 0.14);
    border-radius: var(--radius-md);
    background: rgba(255, 255, 255, 0.92);
    box-shadow: 0 10px 24px rgba(15, 23, 42, 0.04);
  }

  .hourly-summary-item-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
  }

  .hourly-summary-item h3 {
    color: #292524;
    font-size: 0.95rem;
    font-weight: 720;
  }

  .hourly-summary-item-header p {
    margin-top: 0.25rem;
    color: #a8a29e;
    font-size: 0.74rem;
  }

  .hourly-summary-badges {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.38rem;
  }

  .hourly-summary-peak,
  .hourly-summary-rhythm {
    display: inline-flex;
    align-items: center;
    min-height: 1.55rem;
    padding: 0.18rem 0.56rem;
    border-radius: var(--radius-full);
    font-size: 0.68rem;
    font-weight: 700;
    white-space: nowrap;
  }

  /* 徽章走中性体系：高峰用深实底突出，节奏档位统一灰胶囊（靠文字区分） */
  .hourly-summary-peak {
    color: #fafaf9;
    background: #292524;
  }

  .hourly-summary-rhythm {
    border: 1px solid rgba(148, 163, 184, 0.32);
    color: #57534e;
    background: #fff;
  }

  .hourly-summary-primary {
    margin-top: 0.82rem !important;
    color: #1c1917;
    font-size: 0.9rem;
    font-weight: 600;
    line-height: 1.65;
  }

  .hourly-summary-secondary {
    margin-top: 0.48rem !important;
    color: #57534e;
    font-size: 0.82rem;
    line-height: 1.72;
  }

  .hourly-summary-expand {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    margin-top: 0.56rem;
    padding: 0;
    border: 0;
    color: #78716c;
    background: transparent;
    font-size: 0.74rem;
    cursor: pointer;
  }

  .hourly-summary-expand svg {
    width: 0.8rem;
    height: 0.8rem;
    transition: transform 160ms ease;
  }

  .hourly-summary-expand-open {
    transform: rotate(180deg);
  }

  .hourly-summary-apps {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-top: 0.78rem;
  }

  .hourly-summary-apps span {
    padding: 0.27rem 0.58rem;
    border: 1px solid rgba(120, 113, 108, 0.12);
    border-radius: var(--radius-full);
    color: #57534e;
    background: #fafaf9;
    font-size: 0.72rem;
  }

  :global(.dark) .hourly-summary-drawer {
    border-color: var(--surface-border-default);
    background: #161b22;
    box-shadow: -18px 0 48px rgba(0, 0, 0, 0.28);
  }

  :global(.dark) .hourly-summary-header {
    border-bottom-color: var(--surface-border-subtle);
    background: #161b22;
  }

  :global(.dark) .hourly-summary-heading h2,
  :global(.dark) .hourly-summary-item h3,
  :global(.dark) .hourly-summary-primary {
    color: #e6edf3;
  }

  :global(.dark) .hourly-summary-heading p,
  :global(.dark) .hourly-summary-item-header p,
  :global(.dark) .hourly-summary-secondary,
  :global(.dark) .hourly-summary-refreshing,
  :global(.dark) .hourly-summary-state,
  :global(.dark) .hourly-summary-expand {
    color: #8b949e;
  }

  :global(.dark) .hourly-summary-kicker {
    color: #8b949e;
  }

  :global(.dark) .hourly-summary-close {
    color: #adbac7;
    border-color: var(--surface-border-default);
    background: #21262d;
  }

  :global(.dark) .hourly-summary-close:hover {
    color: #e6edf3;
    background: #30363d;
  }

  :global(.dark) .hourly-summary-body {
    background: #0d1117;
  }

  :global(.dark) .hourly-summary-refresh-error {
    color: #ff7b72;
    border-color: rgba(248, 81, 73, 0.22);
    background: rgba(63, 20, 20, 0.28);
  }

  :global(.dark) .hourly-summary-item {
    border-color: var(--surface-border-subtle);
    background: #161b22;
    box-shadow: none;
  }

  :global(.dark) .hourly-summary-peak {
    color: #161b22;
    background: #8b949e;
  }

  :global(.dark) .hourly-summary-rhythm {
    border-color: rgba(48, 54, 61, 0.9);
    color: #adbac7;
    background: #21262d;
  }

  :global(.dark) .hourly-summary-apps span {
    color: #adbac7;
    border-color: var(--surface-border-subtle);
    background: #21262d;
  }

  @keyframes hourly-summary-spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (max-width: 640px) {
    .hourly-summary-overlay {
      padding: 0;
    }

    .hourly-summary-drawer {
      width: 100%;
      height: 100vh;
      border-inline-end: 0;
      border-radius: 0;
    }

    .hourly-summary-header,
    .hourly-summary-body {
      padding-inline: 1rem;
    }

    .hourly-summary-item-header {
      flex-direction: column;
      gap: 0.55rem;
    }

    .hourly-summary-badges {
      justify-content: flex-start;
    }
  }
</style>
