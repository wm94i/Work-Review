<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import Orbit from 'lucide-svelte/icons/orbit';
  import Rows3 from 'lucide-svelte/icons/rows-3';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';
  import { locale, t } from '$lib/i18n/index.ts';

  export let dateLabel = '';
  export let evidenceCount: number | string = 0;
  export let viewMode: 'orbit' | 'stream' = 'stream';
  export let canGoNext = true;

  const dispatch = createEventDispatcher<{
    view: { mode: 'orbit' | 'stream' };
    previous: void;
    next: void;
  }>();
</script>

<header
  class="evidence-timeline-head"
  data-locale={$locale}
  aria-labelledby="evidence-timeline-title"
>
  <div class="evidence-timeline-head__copy">
    <p>{t('evidenceTemplate.dayTrace', { count: evidenceCount })}</p>
    <h1 id="evidence-timeline-title">{t('evidenceTemplate.timelineTitle')}</h1>
    <span>{dateLabel}</span>
  </div>

  <div class="evidence-timeline-head__controls">
    <div class="evidence-view-switch" role="group" aria-label={t('evidenceTemplate.timelineView')}>
      <button
        type="button"
        aria-pressed={viewMode === 'orbit'}
        on:click={() => dispatch('view', { mode: 'orbit' })}
      >
        <Orbit size={16} strokeWidth={1.7} aria-hidden="true" />
        {t('evidenceTemplate.orbit')}
      </button>
      <button
        type="button"
        aria-pressed={viewMode === 'stream'}
        on:click={() => dispatch('view', { mode: 'stream' })}
      >
        <Rows3 size={16} strokeWidth={1.7} aria-hidden="true" />
        {t('evidenceTemplate.stream')}
      </button>
    </div>

    <div class="evidence-day-switch" role="group" aria-label={t('evidenceTemplate.timelineDateNavigation')}>
      <button type="button" aria-label={t('evidenceTemplate.previousDay')} on:click={() => dispatch('previous')}>
        <ChevronLeft size={17} strokeWidth={1.7} aria-hidden="true" />
      </button>
      <button type="button" aria-label={t('evidenceTemplate.nextDay')} disabled={!canGoNext} on:click={() => dispatch('next')}>
        <ChevronRight size={17} strokeWidth={1.7} aria-hidden="true" />
      </button>
    </div>
  </div>
</header>

<style>
  .evidence-timeline-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 2rem;
    padding: 1.8rem 2rem 1.2rem;
    border-bottom: 1px solid var(--evidence-header-divider);
  }

  .evidence-timeline-head__copy p,
  .evidence-timeline-head__copy span {
    margin: 0;
    color: var(--evidence-timeline-secondary);
    font: 600 0.62rem/1.4 ui-monospace, SFMono-Regular, Menlo, monospace;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  h1 {
    margin: 0.55rem 0 0.6rem;
    color: var(--evidence-timeline-heading);
    font-size: clamp(2.6rem, 6vw, 4.8rem);
    line-height: 0.9;
    letter-spacing: -0.07em;
  }

  .evidence-timeline-head__controls,
  .evidence-view-switch,
  .evidence-day-switch {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .evidence-timeline-head__controls {
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .evidence-view-switch,
  .evidence-day-switch {
    padding: 0.2rem;
    border: 1px solid var(--evidence-line);
    border-radius: var(--radius-lg);
    background: var(--evidence-summary-background);
  }

  button {
    display: inline-flex;
    min-height: 2.25rem;
    align-items: center;
    justify-content: center;
    gap: 0.38rem;
    padding: 0.5rem 0.65rem;
    border: 1px solid transparent;
    border-radius: var(--radius-md);
    color: var(--evidence-switch-text);
    background: transparent;
    font-size: 0.7rem;
    transition: color 160ms ease, border-color 160ms ease, background 160ms ease;
  }

  button:hover:not(:disabled),
  button:focus-visible {
    color: var(--evidence-switch-hover-text);
    background: var(--evidence-switch-hover-background);
  }

  button:focus-visible {
    outline: 2px solid var(--evidence-focus-ring);
    outline-offset: 2px;
  }

  button[aria-pressed='true'] {
    color: var(--evidence-acid);
    border-color: var(--evidence-switch-active-border);
    background: var(--evidence-switch-active-background);
  }

  button:disabled {
    opacity: 0.34;
    cursor: not-allowed;
  }

  @media (max-width: 900px) {
    .evidence-timeline-head {
      align-items: flex-start;
      flex-direction: column;
      gap: 1.2rem;
      padding-inline: 1.25rem;
    }

    .evidence-timeline-head__controls {
      justify-content: flex-start;
    }
  }

  @media (max-width: 720px) {
    .evidence-timeline-head {
      padding: 1.35rem 1rem 1rem;
    }

    h1 {
      font-size: clamp(2.3rem, 13vw, 3.8rem);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    button {
      transition: none;
    }
  }
</style>
