<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';
  import CalendarDays from 'lucide-svelte/icons/calendar-days';
  import { locale, t } from '$lib/i18n/index.ts';

  export let dateLabel = '';
  export let totalDuration = '0 分钟';
  export let evidenceCount = 0;
  export let isRecording = false;
  export let canGoNext = true;

  const dispatch = createEventDispatcher<{
    previous: void;
    today: void;
    next: void;
  }>();
</script>

<header
  class="evidence-page-head"
  data-locale={$locale}
  aria-labelledby="evidence-overview-title"
>
  <div class="evidence-page-head__copy">
    <p class="evidence-page-head__eyebrow">{t('evidenceTemplate.overviewEyebrow')}</p>
    <h1 id="evidence-overview-title">
      <span>{totalDuration}</span>
      <small>{t('evidenceTemplate.focused')}</small>
    </h1>
    <p class="evidence-page-head__meta">
      <span>{t('evidenceTemplate.evidenceFragments', { count: evidenceCount })}</span>
      <span class:live={isRecording}>
        {isRecording
          ? t('evidenceTemplate.traceActive')
          : t('evidenceTemplate.localEvidenceField')}
      </span>
    </p>
  </div>

  <div class="evidence-page-head__controls">
    <div class="evidence-date" role="group" aria-label={t('evidenceTemplate.overviewDateNavigation')}>
      <button type="button" aria-label={t('evidenceTemplate.previousDay')} on:click={() => dispatch('previous')}>
        <ChevronLeft size={17} strokeWidth={1.7} aria-hidden="true" />
      </button>
      <button class="evidence-date__today" type="button" aria-label={t('evidenceTemplate.today')} on:click={() => dispatch('today')}>
        <CalendarDays size={16} strokeWidth={1.7} aria-hidden="true" />
        <span>{dateLabel}</span>
      </button>
      <button type="button" aria-label={t('evidenceTemplate.nextDay')} disabled={!canGoNext} on:click={() => dispatch('next')}>
        <ChevronRight size={17} strokeWidth={1.7} aria-hidden="true" />
      </button>
    </div>
    <slot name="actions" />
  </div>
</header>

<style>
  .evidence-page-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 2rem;
    padding: 2rem 2rem 1.25rem;
    border-bottom: 1px solid var(--evidence-header-divider);
  }

  .evidence-page-head__copy {
    min-width: 0;
  }

  .evidence-page-head__eyebrow,
  .evidence-page-head__meta {
    margin: 0;
    font: 600 0.62rem/1.4 ui-monospace, SFMono-Regular, Menlo, monospace;
    letter-spacing: 0.17em;
    text-transform: uppercase;
  }

  .evidence-page-head__eyebrow {
    color: var(--evidence-text-subtle);
  }

  h1 {
    display: flex;
    align-items: flex-end;
    gap: 0.9rem;
    margin: 0.72rem 0 0.72rem;
    color: var(--evidence-text-strong);
    font-size: clamp(3rem, 7vw, 5.4rem);
    line-height: 0.82;
    letter-spacing: -0.085em;
  }

  h1 small {
    padding-bottom: 0.36rem;
    color: var(--evidence-acid);
    font: 600 0.62rem/1 ui-monospace, SFMono-Regular, Menlo, monospace;
    letter-spacing: 0.12em;
  }

  .evidence-page-head__meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
    color: var(--evidence-header-meta);
    letter-spacing: 0.08em;
  }

  .evidence-page-head__meta span + span::before {
    margin-inline-end: 0.75rem;
    color: var(--evidence-header-separator);
    content: '/';
  }

  .evidence-page-head__meta .live {
    color: var(--evidence-header-live);
  }

  .evidence-page-head__controls {
    display: flex;
    max-width: 34rem;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
    gap: 0.55rem;
  }

  .evidence-date {
    display: flex;
    align-items: center;
    gap: 0.3rem;
  }

  .evidence-date button {
    display: inline-flex;
    min-width: 2.3rem;
    min-height: 2.3rem;
    align-items: center;
    justify-content: center;
    gap: 0.42rem;
    padding: 0.55rem 0.65rem;
    border: 1px solid var(--evidence-line-control);
    border-radius: var(--radius-md);
    color: var(--evidence-header-control-text);
    background: var(--evidence-control-background);
    font-size: 0.7rem;
    transition: color 160ms ease, border-color 160ms ease, background 160ms ease;
  }

  .evidence-date button:hover:not(:disabled),
  .evidence-date button:focus-visible {
    color: var(--evidence-header-control-hover-text);
    border-color: var(--evidence-control-hover-border);
    background: var(--evidence-control-hover-background);
  }

  .evidence-date button:focus-visible {
    outline: 2px solid var(--evidence-focus-ring);
    outline-offset: 2px;
  }

  .evidence-date button:disabled {
    opacity: 0.34;
    cursor: not-allowed;
  }

  .evidence-date__today {
    max-width: min(22rem, 44vw);
  }

  .evidence-date__today span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @media (max-width: 900px) {
    .evidence-page-head {
      align-items: flex-start;
      flex-direction: column;
      gap: 1.25rem;
      padding-inline: 1.25rem;
    }

    .evidence-page-head__controls {
      max-width: none;
      justify-content: flex-start;
    }
  }

  @media (max-width: 640px) {
    .evidence-page-head {
      padding: 1.35rem 1rem 1rem;
    }

    h1 {
      font-size: clamp(2.4rem, 14vw, 4rem);
    }

    .evidence-date__today {
      max-width: 12rem;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .evidence-date button {
      transition: none;
    }
  }
</style>
