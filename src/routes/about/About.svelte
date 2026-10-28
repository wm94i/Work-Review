<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { open } from '@tauri-apps/plugin-shell';
  import { getVersion } from '@tauri-apps/api/app';
  import { locale, t } from '$lib/i18n/index.ts';
  import { runUpdateFlow } from '$lib/utils/updater.ts';

  interface UpdateSettings {
    autoCheck: boolean;
    lastCheckTime: number;
    checkIntervalHours: number;
  }

  const wechatSponsorshipQr = new URL('../../../docs/sponsorship/vx.png', import.meta.url).href;
  const alipaySponsorshipQr = new URL('../../../docs/sponsorship/zfb.png', import.meta.url).href;
  const bmcQr = new URL('../../../docs/sponsorship/buycoffee.png', import.meta.url).href;

  let appVersion = '';
  let isCheckingUpdate = false;
  let autoCheckUpdate = true;
  let isSponsorshipOpen = false;
  let isUpdateDetailsOpen = false;
  let zoomedQr: string | null = null;
  let updateStatus = '';
  let updateStatusTimer: ReturnType<typeof setTimeout> | null = null;
  $: currentLocale = $locale;

  onMount(async () => {
    try {
      const version = await getVersion();
      appVersion = typeof version === 'string' && version.trim() ? version : '--';
    } catch (e) {
      console.error('获取版本失败:', e);
      appVersion = '--';
    }

    try {
      const settings = await invoke<UpdateSettings>('get_update_settings');
      autoCheckUpdate = settings?.autoCheck ?? true;
    } catch (e) {
      console.error('读取更新设置失败:', e);
    }
  });

  async function toggleAutoCheck() {
    autoCheckUpdate = !autoCheckUpdate;
    try {
      const settings = await invoke<UpdateSettings>('get_update_settings');
      settings.autoCheck = autoCheckUpdate;
      await invoke('save_update_settings', { settings });
    } catch (e) {
      console.error('保存更新设置失败:', e);
      autoCheckUpdate = !autoCheckUpdate;
    }
  }

  async function openGitHub() {
    await open('https://github.com/wm94i/Work-Review');
  }

  async function openDataDir() {
    try {
      await invoke('open_data_dir');
    } catch (e) {
      console.error('打开目录失败:', e);
    }
  }

  function openSponsorshipModal() {
    isSponsorshipOpen = true;
  }

  function closeSponsorshipModal() {
    isSponsorshipOpen = false;
  }

  async function checkForUpdates() {
    if (isCheckingUpdate) return;

    isCheckingUpdate = true;
    updateStatus = t('about.checkingUpdates');

    await runUpdateFlow({
      onStatusChange: (status) => {
        updateStatus = status;
      },
    });

    isCheckingUpdate = false;
    if (updateStatus) {
      if (updateStatusTimer !== null) clearTimeout(updateStatusTimer);
      updateStatusTimer = setTimeout(() => {
        updateStatus = '';
        updateStatusTimer = null;
      }, 3000);
    }
  }

  onDestroy(() => {
    if (updateStatusTimer !== null) clearTimeout(updateStatusTimer);
  });

  function handleWindowKeydown(event: KeyboardEvent): void {
    if (event.key !== 'Escape') return;
    if (isSponsorshipOpen) {
      closeSponsorshipModal();
    } else if (isUpdateDetailsOpen) {
      isUpdateDetailsOpen = false;
    }
  }
</script>

<svelte:window on:keydown={handleWindowKeydown} />

<div class="page-shell about-editorial-shell" data-locale={currentLocale}>
  <div class="page-header page-axis-operation">
    <div class="page-title-group">
      <div class="page-title-badge">
        <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <circle cx="12" cy="12" r="8.5" stroke-width="1.8" />
          <path stroke-linecap="round" stroke-width="1.8" d="M12 10.5v5" />
          <circle cx="12" cy="7.5" r=".8" fill="currentColor" stroke="none" />
        </svg>
      </div>
      <div class="page-title-copy">
        <h2>{t('about.pageTitle')}</h2>
        <p>{t('about.pageSubtitle')}</p>
      </div>
    </div>
  </div>

  <div class="w-full about-minimal-shell page-axis-reading">
    <section class="page-card about-brand-card">
      <div class="about-brand-identity flex flex-col items-center">
        <div class="about-brand-mark">
          <img src="/icons/256x256.png" alt="Work Review" class="h-16 w-16 rounded-lg object-cover" />
        </div>

        <div class="about-brand-copy">
          <div class="about-brand-title-row flex items-center justify-center">
            <h1 class="about-brand-title">Work Review</h1>
            <span class="about-version-badge">v{appVersion}</span>
          </div>
          <p class="about-brand-description">{t('about.description')}</p>
        </div>
      </div>

      <div class="about-action-row mt-6">
        <button type="button" on:click={openGitHub} class="page-action-secondary min-h-10 px-4 py-2">
          <svg class="w-4 h-4 shrink-0" fill="currentColor" viewBox="0 0 24 24"><path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/></svg>
          <span class="leading-none">GitHub</span>
        </button>
        <button type="button" on:click={openDataDir} class="page-action-secondary min-h-10 px-4 py-2">
          <svg class="w-4 h-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"/></svg>
          <span class="leading-none">{t('about.openDataDir')}</span>
        </button>
        <button
          type="button"
          on:click={openSponsorshipModal}
          class="about-support-link"
        >
          <svg class="w-4 h-4 shrink-0 text-rose-500 dark:text-rose-400" fill="currentColor" viewBox="0 0 24 24">
            <path d="M11.996 21.357c-.34 0-.673-.092-.966-.267C8.304 19.466 2.25 15.48 2.25 9.806c0-3.034 2.395-5.556 5.47-5.556 1.708 0 3.31.78 4.276 2.074.966-1.293 2.567-2.074 4.275-2.074 3.074 0 5.48 2.522 5.48 5.556 0 5.674-6.054 9.66-8.78 11.284a1.88 1.88 0 0 1-.975.267Z" />
          </svg>
          <span class="leading-none">{t('about.sponsorship')}</span>
        </button>
      </div>

      <div class="about-update-panel mt-7">
        <div class="about-update-heading">
          <div class="about-update-heading-main">
            <div class="about-update-icon" aria-hidden="true">
              <svg fill="none" viewBox="0 0 24 24">
                <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.7" d="M4 7.5A2.5 2.5 0 0 1 6.5 5h11A2.5 2.5 0 0 1 20 7.5v9a2.5 2.5 0 0 1-2.5 2.5h-11A2.5 2.5 0 0 1 4 16.5v-9Z" />
                <path stroke="currentColor" stroke-linecap="round" stroke-width="1.7" d="M8 9.5h8M8 13h5" />
              </svg>
            </div>
            <div class="min-w-0 text-left">
              <div class="about-update-title">{t('about.updateSectionTitle')}</div>
              <p class="about-update-description">{t('about.updateSectionDescription')}</p>
            </div>
          </div>
          <div class="about-update-heading-actions">
            <span class="about-channel-badge">
              <span class="about-channel-dot" aria-hidden="true"></span>
              {t('about.stableChannel')}
            </span>
            <button
              type="button"
              class="about-update-info"
              aria-label={t('about.updateDetailsButton')}
              aria-expanded={isUpdateDetailsOpen}
              aria-controls="about-update-details"
              on:click={() => isUpdateDetailsOpen = !isUpdateDetailsOpen}
            >
              <svg fill="none" viewBox="0 0 24 24" aria-hidden="true">
                <circle cx="12" cy="12" r="8.5" stroke="currentColor" stroke-width="1.7" />
                <path stroke="currentColor" stroke-linecap="round" stroke-width="1.7" d="M12 10.5v5" />
                <circle cx="12" cy="7.5" r=".8" fill="currentColor" stroke="none" />
              </svg>
            </button>
          </div>
        </div>

        {#if isUpdateDetailsOpen}
          <div id="about-update-details" class="about-update-details" role="region" aria-label={t('about.updateDetailsButton')}>
            <p>{t('about.updateDetails')}</p>
          </div>
        {/if}

        <div class="about-update-grid">
          <div class="about-update-unit about-update-version-unit">
            <span class="about-update-label">{t('about.currentVersionTag')}</span>
            <strong class="about-update-value">v{appVersion}</strong>
          </div>

          <div class="about-update-unit about-update-option-unit">
            <div class="min-w-0 text-left">
              <span class="about-update-label">{t('about.autoCheckUpdate')}</span>
              <p class="about-update-hint">{t('about.autoCheckUpdateHint')}</p>
            </div>
            <div class="about-update-option-control">
              <button
                type="button"
                role="switch"
                aria-checked={autoCheckUpdate}
                aria-label={t('about.autoCheckUpdate')}
                on:click={toggleAutoCheck}
                class="about-switch {autoCheckUpdate ? 'about-switch-on' : ''}"
              >
                <span class="about-switch-thumb"></span>
              </button>
              <span class="about-switch-status">{autoCheckUpdate ? t('settings.summary.stateEnabled') : t('settings.summary.stateDisabled')}</span>
            </div>
          </div>

          <div class="about-update-unit about-update-action-unit">
            <span class="about-update-label">{t('about.updateStatus')}</span>
          <button
            type="button"
            on:click={checkForUpdates}
            disabled={isCheckingUpdate}
            class="page-control-btn min-h-9 px-3 disabled:opacity-50"
          >
            {#if isCheckingUpdate}
              <svg class="h-3.5 w-3.5 shrink-0 animate-spin" fill="none" viewBox="0 0 24 24"><circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle><path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path></svg>
              <span class="leading-none">{t('about.checkingUpdates')}</span>
            {:else}
              <svg class="h-3.5 w-3.5 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
              <span class="leading-none">{t('about.checkUpdates')}</span>
            {/if}
          </button>
        </div>
      </div>

        {#if updateStatus}
          <div class="about-update-feedback" role="status" aria-live="polite">
            <p>{updateStatus}</p>
          </div>
        {/if}
      </div>
    </section>

    <section class="page-card about-principles-card">
      <h2 class="about-principles-title page-section-title text-center">{t('about.productPrinciplesTitle')}</h2>
      <div class="about-trust-grid">
        <article class="about-trust-card">
          <span class="about-trust-kicker">01</span>
          <h3 class="about-trust-title">{t('about.localFirstTitle')}</h3>
          <p class="about-trust-copy">{t('about.localFirstCopy')}</p>
        </article>
        <article class="about-trust-card">
          <span class="about-trust-kicker">02</span>
          <h3 class="about-trust-title">{t('about.timelineTrustTitle')}</h3>
          <p class="about-trust-copy">{t('about.timelineTrustCopy')}</p>
        </article>
        <article class="about-trust-card">
          <span class="about-trust-kicker">03</span>
          <h3 class="about-trust-title">{t('about.reportTrustTitle')}</h3>
          <p class="about-trust-copy">{t('about.reportTrustCopy')}</p>
        </article>
      </div>
    </section>

    <section class="page-card about-tech-stack">
      <span class="about-tech-item">Tauri 2</span>
      <span class="about-tech-item">Svelte</span>
      <span class="about-tech-item">Rust</span>
      <span class="about-tech-item">SQLite</span>
    </section>
  </div>
</div>

{#if isSponsorshipOpen}
  <div
    class="fixed inset-0 z-[135] flex items-center justify-center bg-slate-950/52 px-4 py-6 backdrop-blur-md animate-fadeIn"
  >
    <button
      type="button"
      class="absolute inset-0 cursor-default"
      on:click={closeSponsorshipModal}
      aria-label={t('about.closeSupportDialog')}
    ></button>

    <div
      class="relative z-10 w-full max-w-3xl rounded-xl border border-slate-200/80 bg-white p-6 text-left shadow-2xl shadow-slate-950/20 dark:border-[#30363d]/70 dark:bg-[#161b22] sm:p-7"
      role="dialog"
      aria-modal="true"
      aria-labelledby="sponsorship-dialog-title"
    >
      <div class="relative text-center">
        <div class="inline-flex items-center gap-2 rounded-full border border-amber-200 bg-amber-50 px-3 py-1 text-[11px] font-semibold tracking-[0.14em] text-amber-700 dark:border-amber-900/60 dark:bg-amber-950/40 dark:text-amber-300">
          {t('about.supportBadge')}
        </div>
        <h3 id="sponsorship-dialog-title" class="mt-3 text-2xl font-semibold text-slate-900 dark:text-[#e6edf3]">
          {t('about.supportTitle')}
        </h3>
        <p class="mt-3 text-sm leading-7 text-slate-700 dark:text-[#adbac7]">
          {t('about.supportCopy')}
        </p>
        <p class="text-sm leading-7 text-slate-500 dark:text-[#7d8590]">
          {t('about.supportCopy2')}
        </p>

        <button
          type="button"
          on:click={closeSponsorshipModal}
          class="absolute right-0 top-0 inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-lg border border-slate-200 bg-white text-slate-500 transition hover:bg-slate-50 hover:text-slate-700 dark:border-[#30363d] dark:bg-[#21262d] dark:text-[#7d8590] dark:hover:bg-[#30363d] dark:hover:text-[#c9d1d9]"
          aria-label={t('about.closeSupportDialog')}
        >
          <svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="m6 6 12 12M18 6 6 18" />
          </svg>
        </button>
      </div>

      <div class="mt-6 grid gap-4 grid-cols-3">
        <div class="rounded-lg border border-slate-200/80 bg-slate-50/80 p-4 dark:border-[#30363d]/80 dark:bg-[#21262d]/50">
          <div class="flex items-center justify-center gap-2">
            <div class="flex h-10 w-10 items-center justify-center rounded-lg bg-emerald-100 text-emerald-700 dark:bg-emerald-950/50 dark:text-emerald-300">
              <svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M7.5 7.5h9v9h-9z" />
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M4.5 12h3m9 0h3M12 4.5v3m0 9v3" />
              </svg>
            </div>
            <h4 class="text-base font-semibold text-slate-900 dark:text-[#e6edf3]">{t('about.wechat')}</h4>
          </div>
          <div class="mt-4 flex items-center justify-center rounded-lg bg-white p-4 shadow-[inset_0_1px_0_rgba(255,255,255,0.8)] dark:bg-[#0d1117] dark:shadow-none">
            <button type="button" class="aspect-square w-40 cursor-zoom-in rounded-lg transition-transform hover:scale-[1.02]" on:click={() => zoomedQr = wechatSponsorshipQr}>
              <img src={wechatSponsorshipQr} alt={t('about.wechatQrAlt')} class="h-full w-full rounded-lg object-cover" />
            </button>
          </div>
        </div>

        <div class="rounded-lg border border-slate-200/80 bg-slate-50/80 p-4 dark:border-[#30363d]/80 dark:bg-[#21262d]/50">
          <div class="flex items-center justify-center gap-2">
            <div class="flex h-10 w-10 items-center justify-center rounded-lg bg-sky-100 text-sky-700 dark:bg-sky-950/50 dark:text-sky-300">
              <svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M4.5 8.25h15M6.75 4.5h10.5A2.25 2.25 0 0 1 19.5 6.75v10.5a2.25 2.25 0 0 1-2.25 2.25H6.75A2.25 2.25 0 0 1 4.5 17.25V6.75A2.25 2.25 0 0 1 6.75 4.5Z" />
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M8.25 13.5h7.5" />
              </svg>
            </div>
            <h4 class="text-base font-semibold text-slate-900 dark:text-[#e6edf3]">{t('about.alipay')}</h4>
          </div>
          <div class="mt-4 flex items-center justify-center rounded-lg bg-white p-4 shadow-[inset_0_1px_0_rgba(255,255,255,0.8)] dark:bg-[#0d1117] dark:shadow-none">
            <button type="button" class="aspect-square w-40 cursor-zoom-in rounded-lg transition-transform hover:scale-[1.02]" on:click={() => zoomedQr = alipaySponsorshipQr}>
              <img src={alipaySponsorshipQr} alt={t('about.alipayQrAlt')} class="h-full w-full rounded-lg object-cover" />
            </button>
          </div>
        </div>

        <div class="rounded-lg border border-slate-200/80 bg-slate-50/80 p-4 dark:border-[#30363d]/80 dark:bg-[#21262d]/50">
          <div class="flex items-center justify-center gap-2">
            <div class="flex h-10 w-10 items-center justify-center rounded-lg bg-amber-100 text-amber-700 dark:bg-amber-950/50 dark:text-amber-300">
              <svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M15.75 6.75a3.75 3.75 0 1 1-7.5 0 3.75 3.75 0 0 1 7.5 0Z" />
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M12 10.5v6.75" />
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M9.75 13.5h4.5" />
              </svg>
            </div>
            <h4 class="text-base font-semibold text-slate-900 dark:text-[#e6edf3]">Buy Me a Coffee</h4>
          </div>
          <div class="mt-4 flex items-center justify-center rounded-lg bg-white p-4 shadow-[inset_0_1px_0_rgba(255,255,255,0.8)] dark:bg-[#0d1117] dark:shadow-none">
            <button type="button" class="aspect-square w-40 cursor-zoom-in rounded-lg transition-transform hover:scale-[1.02]" on:click={() => zoomedQr = bmcQr}>
              <img src={bmcQr} alt="Buy Me a Coffee QR code" class="h-full w-full rounded-lg object-cover" />
            </button>
          </div>
        </div>
      </div>
    </div>

    {#if zoomedQr}
      <div
        class="fixed inset-0 z-[140] flex items-center justify-center bg-slate-950/70 backdrop-blur-sm animate-fadeIn"
      >
        <button
          type="button"
          class="absolute inset-0 cursor-zoom-out"
          aria-label={t('about.closeSupportDialog')}
          on:click={() => zoomedQr = null}
        ></button>
        <img src={zoomedQr} alt="" class="relative z-10 max-h-[75vh] max-w-[75vw] rounded-xl shadow-2xl" />
      </div>
    {/if}
  </div>
{/if}
