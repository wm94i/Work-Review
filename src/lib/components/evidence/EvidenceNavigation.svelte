<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import LayoutDashboard from 'lucide-svelte/icons/layout-dashboard';
  import Waypoints from 'lucide-svelte/icons/waypoints';
  import FileText from 'lucide-svelte/icons/file-text';
  import Bot from 'lucide-svelte/icons/bot';
  import Settings from 'lucide-svelte/icons/settings';
  import Info from 'lucide-svelte/icons/info';
  import Pause from 'lucide-svelte/icons/pause';
  import Play from 'lucide-svelte/icons/play';
  import Monitor from 'lucide-svelte/icons/monitor';
  import Sun from 'lucide-svelte/icons/sun';
  import Moon from 'lucide-svelte/icons/moon';
  import { locale, t } from '$lib/i18n/index.ts';

  type Theme = 'system' | 'light' | 'dark';

  export let activeRoute = '/';
  export let isRecording = false;
  export let isPaused = false;
  export let recordingTransitionPending = false;
  export let theme: Theme = 'system';

  const dispatch = createEventDispatcher<{
    'toggle-recording': void;
    themeChange: Theme;
  }>();

  const items = [
    { path: '/', icon: LayoutDashboard, labelKey: 'sidebar.nav.overview' },
    { path: '/timeline', icon: Waypoints, labelKey: 'sidebar.nav.timeline' },
    { path: '/report', icon: FileText, labelKey: 'sidebar.nav.report' },
    { path: '/ask', icon: Bot, labelKey: 'sidebar.nav.ask' },
    { path: '/settings', icon: Settings, labelKey: 'sidebar.nav.settings' },
    { path: '/about', icon: Info, labelKey: 'sidebar.nav.about' },
  ] as const;

  const themeOptions = [
    { value: 'system', icon: Monitor, labelKey: 'sidebar.themeTitle.system' },
    { value: 'light', icon: Sun, labelKey: 'sidebar.themeTitle.light' },
    { value: 'dark', icon: Moon, labelKey: 'sidebar.themeTitle.dark' },
  ] as const;

  $: activeRecording = isRecording && !isPaused;
  $: currentLocale = $locale;
  $: localizedItems = items.map((item) => ({
    ...item,
    label: t(item.labelKey),
    locale: currentLocale,
  }));
  $: localizedThemeOptions = themeOptions.map((option) => ({
    ...option,
    label: t(option.labelKey),
    locale: currentLocale,
  }));
  $: recordingLabel = currentLocale && (
    !isRecording
      ? t('evidenceTemplate.recordingStopped')
      : isPaused
        ? t('evidenceTemplate.recordingPaused')
        : t('evidenceTemplate.recordingActive')
  );
  $: recordingActionLabel = currentLocale && (
    isPaused
      ? t('evidenceTemplate.resumeRecording')
      : t('evidenceTemplate.pauseRecording')
  );

  function isRouteActive(path: string): boolean {
    return path === '/' ? activeRoute === '/' : activeRoute.startsWith(path);
  }
</script>

<nav class="evidence-nav" data-locale={currentLocale} aria-label={t('evidenceTemplate.mainNavigation')}>
  <a class="evidence-nav__brand" href="#/" aria-label={t('evidenceTemplate.home')}>
    <span>WR</span>
  </a>

  <div class="evidence-nav__routes">
    {#each localizedItems as item}
      <a
        class:active={isRouteActive(item.path)}
        href="#{item.path}"
        aria-label={item.label}
        aria-current={isRouteActive(item.path) ? 'page' : undefined}
      >
        <svelte:component this={item.icon} size={18} strokeWidth={1.7} aria-hidden="true" />
        <span class="evidence-nav__tooltip">{item.label}</span>
      </a>
    {/each}
  </div>

  <div class="evidence-nav__recording">
    <div
      class="evidence-nav__themes"
      role="group"
      aria-label={t('evidenceTemplate.themePicker')}
    >
      {#each localizedThemeOptions as option}
        <button
          type="button"
          class:active={theme === option.value}
          aria-label={option.label}
          aria-pressed={theme === option.value}
          title={option.label}
          on:click={() => dispatch('themeChange', option.value)}
        >
          <svelte:component this={option.icon} size={15} strokeWidth={1.7} aria-hidden="true" />
        </button>
      {/each}
    </div>

    <button
      type="button"
      disabled={!isRecording || recordingTransitionPending}
      aria-label={recordingActionLabel}
      title={recordingActionLabel}
      on:click={() => dispatch('toggle-recording')}
    >
      {#if isPaused}
        <Play size={15} strokeWidth={1.8} aria-hidden="true" />
      {:else}
        <Pause size={15} strokeWidth={1.8} aria-hidden="true" />
      {/if}
    </button>

    <div
      class="evidence-nav__status"
      class:recording={activeRecording}
      role="status"
      aria-live="polite"
    >
      <span class="evidence-nav__status-dot" aria-hidden="true"></span>
      <span>{recordingLabel}</span>
    </div>
  </div>
</nav>

<style>
  .evidence-nav {
    display: flex;
    height: 100%;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding: 0.25rem 0.45rem 0.4rem;
  }

  .evidence-nav__brand,
  .evidence-nav__routes a {
    position: relative;
    display: grid;
    width: 2.75rem;
    height: 2.75rem;
    place-items: center;
    border: 1px solid transparent;
    border-radius: var(--radius-lg);
    color: var(--evidence-nav-text);
    transition: color 160ms ease, border-color 160ms ease, background 160ms ease, transform 160ms ease;
  }

  .evidence-nav__brand {
    margin-bottom: 0.35rem;
    color: var(--evidence-acid-ink);
    background: var(--evidence-acid);
    box-shadow: var(--evidence-brand-shadow);
    font: 800 0.68rem/1 ui-monospace, SFMono-Regular, Menlo, monospace;
    letter-spacing: -0.04em;
  }

  .evidence-nav__routes {
    display: flex;
    flex-direction: column;
    gap: 0.42rem;
  }

  .evidence-nav__routes a:hover,
  .evidence-nav__routes a:focus-visible {
    color: var(--evidence-nav-hover-text);
    border-color: var(--evidence-nav-hover-border);
    background: var(--evidence-nav-hover-background);
    transform: translateX(2px);
  }

  .evidence-nav__routes a:focus-visible {
    outline: 2px solid var(--evidence-focus-ring);
    outline-offset: 2px;
  }

  .evidence-nav__routes a.active {
    color: var(--evidence-acid);
    border-color: var(--evidence-nav-active-border);
    background: var(--evidence-nav-active-background);
  }

  .evidence-nav__routes a.active::after {
    position: absolute;
    inset-inline-start: -0.72rem;
    width: 2px;
    height: 1.25rem;
    border-radius: var(--radius-full);
    background: var(--evidence-acid);
    box-shadow: 0 0 12px var(--evidence-nav-acid-glow);
    content: '';
  }

  .evidence-nav__tooltip {
    position: absolute;
    inset-inline-start: 3.25rem;
    z-index: 5;
    padding: 0.38rem 0.55rem;
    border: 1px solid var(--evidence-line);
    border-radius: var(--radius-md);
    color: var(--evidence-tooltip-text);
    background: var(--evidence-tooltip-background);
    box-shadow: var(--evidence-tooltip-shadow);
    font-size: 0.68rem;
    opacity: 0;
    pointer-events: none;
    transform: translateX(-4px);
    white-space: nowrap;
    transition: opacity 140ms ease, transform 140ms ease;
  }

  .evidence-nav__routes a:hover .evidence-nav__tooltip,
  .evidence-nav__routes a:focus-visible .evidence-nav__tooltip {
    opacity: 1;
    transform: translateX(0);
  }

  .evidence-nav__recording {
    display: grid;
    margin-top: auto;
    justify-items: center;
    gap: 0.55rem;
  }

  .evidence-nav__themes {
    display: grid;
    gap: 0.3rem;
    padding: 0.28rem;
    border: 1px solid var(--evidence-line);
    border-radius: var(--radius-lg);
    background: var(--evidence-control-background);
  }

  .evidence-nav__themes button {
    display: grid;
    width: 1.9rem;
    height: 1.9rem;
    place-items: center;
    border: 1px solid transparent;
    border-radius: var(--radius-md);
    color: var(--evidence-nav-control-text);
    background: transparent;
    transition: color 160ms ease, border-color 160ms ease, background 160ms ease, transform 160ms ease;
  }

  .evidence-nav__themes button:hover,
  .evidence-nav__themes button:focus-visible {
    color: var(--evidence-nav-hover-text);
    border-color: var(--evidence-nav-hover-border);
    background: var(--evidence-nav-hover-background);
    transform: translateX(1px);
  }

  .evidence-nav__themes button:focus-visible {
    outline: 2px solid var(--evidence-focus-ring);
    outline-offset: 2px;
  }

  .evidence-nav__themes button.active {
    color: var(--evidence-acid);
    border-color: var(--evidence-nav-active-border);
    background: var(--evidence-nav-active-background);
  }

  .evidence-nav__recording > button {
    display: grid;
    width: 2.2rem;
    height: 2.2rem;
    place-items: center;
    border: 1px solid var(--evidence-line);
    border-radius: var(--radius-lg);
    color: var(--evidence-nav-control-text);
    background: var(--evidence-control-background);
  }

  .evidence-nav__recording > button:hover:not(:disabled),
  .evidence-nav__recording > button:focus-visible {
    color: var(--evidence-acid);
    border-color: var(--evidence-nav-control-hover-border);
    background: var(--evidence-nav-control-hover-background);
  }

  .evidence-nav__recording > button:focus-visible {
    outline: 2px solid var(--evidence-focus-ring);
    outline-offset: 2px;
  }

  .evidence-nav__recording > button:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }

  .evidence-nav__status {
    display: grid;
    justify-items: center;
    gap: 0.35rem;
    color: var(--evidence-status-text);
    font: 500 0.62rem/1 ui-monospace, SFMono-Regular, Menlo, monospace;
    text-align: center;
  }

  .evidence-nav__status-dot {
    width: 0.45rem;
    height: 0.45rem;
    border-radius: var(--radius-full);
    background: var(--evidence-status-dot);
  }

  .evidence-nav__status.recording {
    color: var(--evidence-status-live-text);
  }

  .evidence-nav__status.recording .evidence-nav__status-dot {
    background: var(--evidence-acid);
    box-shadow: 0 0 12px var(--evidence-status-active-shadow);
  }

  @media (max-width: 900px) {
    .evidence-nav {
      padding-inline: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .evidence-nav__brand,
    .evidence-nav__routes a,
    .evidence-nav__tooltip,
    .evidence-nav__themes button {
      transition: none;
    }

    .evidence-nav__themes button:hover,
    .evidence-nav__themes button:focus-visible {
      transform: none;
    }
  }
</style>
