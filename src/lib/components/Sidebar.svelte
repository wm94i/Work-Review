<script lang="ts">
  import { link, location } from 'svelte-spa-router';
  import { invoke } from '@tauri-apps/api/core';
  import { emitTo } from '@tauri-apps/api/event';
  import { createEventDispatcher } from 'svelte';
  import {
    getLocaleLabel,
    locale,
    setLocale,
    t,
    type InterpolationParams,
    type Locale,
  } from '$lib/i18n/index.ts';

  type Theme = 'system' | 'light' | 'dark';

  interface NavItem {
    path: string;
    labelKey: string;
    icon: 'home' | 'timeline' | 'report' | 'ask' | 'settings' | 'info';
  }

  interface LocaleOption {
    value: Locale;
    label: string;
    fullLabelKey: string;
  }

  export let isRecording = true;
  export let isPaused = false;
  export let theme: Theme = 'system';
  
  const dispatch = createEventDispatcher<{ themeChange: Theme }>();
  let localeMenuOpen = false;
  let localeMenuContainer: HTMLElement | null = null;

  const navItems: NavItem[] = [
    { path: '/', labelKey: 'sidebar.nav.overview', icon: 'home' },
    { path: '/timeline', labelKey: 'sidebar.nav.timeline', icon: 'timeline' },
    { path: '/report', labelKey: 'sidebar.nav.report', icon: 'report' },
    { path: '/ask', labelKey: 'sidebar.nav.ask', icon: 'ask' },
    { path: '/settings', labelKey: 'sidebar.nav.settings', icon: 'settings' },
    { path: '/about', labelKey: 'sidebar.nav.about', icon: 'info' },
  ];

  $: currentLocale = $locale;
  $: translate = (key: string, params: InterpolationParams = {}) => {
    currentLocale;
    return t(key, params);
  };
  const localeOptionsBase: LocaleOption[] = [
    { value: 'zh-CN', label: 'ZH', fullLabelKey: 'sidebar.localeNames.zhCN' },
    { value: 'en', label: 'EN', fullLabelKey: 'sidebar.localeNames.en' },
    { value: 'zh-TW', label: 'TW', fullLabelKey: 'sidebar.localeNames.zhTW' },
    { value: 'ar', label: 'AR', fullLabelKey: 'sidebar.localeNames.ar' },
  ];
  $: localeOptions = localeOptionsBase.map((option) => ({
    ...option,
    fullLabel: translate(option.fullLabelKey),
  }));
  $: currentLocaleLabel = getLocaleLabel(currentLocale);

  function cycleTheme(): void {
    const themes: Theme[] = ['system', 'light', 'dark'];
    const currentIndex = themes.indexOf(theme);
    const nextTheme = themes[(currentIndex + 1) % themes.length] ?? 'system';
    dispatch('themeChange', nextTheme);
  }

  function toggleLocaleMenu(): void {
    localeMenuOpen = !localeMenuOpen;
  }

  function selectLocale(nextLocale: Locale): void {
    const normalizedLocale = setLocale(nextLocale);
    localeMenuOpen = false;
    emitTo('avatar', 'locale-changed', normalizedLocale).catch((error) => {
      console.warn('同步桌宠语言失败:', error);
    });
    invoke('set_app_locale', { locale: normalizedLocale }).catch((error) => {
      console.warn('同步后端语言失败:', error);
    });
  }

  function handleWindowClick(event: MouseEvent): void {
    if (
      !localeMenuOpen
      || (event.target instanceof Node && localeMenuContainer?.contains(event.target))
    ) {
      return;
    }

    localeMenuOpen = false;
  }

  function handleWindowKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      localeMenuOpen = false;
    }
  }

  async function toggleRecording() {
    try {
      if (isPaused) {
        await invoke('resume_recording');
      } else {
        await invoke('pause_recording');
      }
    } catch (e) {
      console.error('切换录制状态失败:', e);
    }
  }

  $: activeStates = navItems.reduce<Record<string, boolean>>((acc, item) => {
    const loc = $location || '/';
    if (item.path === '/') {
      acc[item.path] = loc === '/';
    } else {
      acc[item.path] = loc === item.path || loc.startsWith(item.path + '/');
    }
    return acc;
  }, {});
</script>

<svelte:window on:click={handleWindowClick} on:keydown={handleWindowKeydown} />

<div class="sidebar-editorial-shell h-full flex flex-col overflow-hidden">
  <div class="sidebar-top">
    <!-- Logo 区域 -->
    <div class="sidebar-brand sidebar-brand-panel">
      <div class="sidebar-brand-row flex items-center gap-3 min-w-0">
        <div class="flex items-center gap-3 min-w-0">
          <div class="w-10 h-10 rounded-xl overflow-hidden shadow-md dark:shadow-[0_4px_12px_rgba(0,0,0,0.4)] shrink-0 ring-1 ring-slate-200/50 dark:ring-[#30363d]/50">
            <img src="/icons/256x256.png" alt="Work Review" class="w-full h-full object-cover" />
          </div>
          <div class="min-w-0">
            <h1 class="sidebar-brand-title">Work Review</h1>
          </div>
        </div>
      </div>
    </div>

    <!-- 录制状态 -->
    <div class="sidebar-status sidebar-status-panel">
      <div class="flex items-center justify-between gap-3">
        <div class="sidebar-recording-copy flex items-center gap-2 min-w-0">
          <span class="relative flex h-2.5 w-2.5">
            {#if isRecording && !isPaused}
              <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75"></span>
              <span class="relative inline-flex rounded-full h-2.5 w-2.5 bg-emerald-500"></span>
            {:else}
              <span class="relative inline-flex rounded-full h-2.5 w-2.5 bg-slate-300 dark:bg-[#484f58]"></span>
            {/if}
          </span>
          <span class="text-xs font-semibold tracking-[0.08em] text-slate-500 dark:text-[#7d8590]">
            {translate('sidebar.recordingStatus')}
          </span>
        </div>
        <button
          on:click={toggleRecording}
          class="sidebar-recording-toggle mt-0.5 shrink-0 px-3 py-1.5 text-[11px] font-semibold rounded-lg transition-all
            {isPaused 
              ? 'bg-emerald-100 text-emerald-700 hover:bg-emerald-200 dark:bg-emerald-900/40 dark:text-emerald-300' 
              : 'bg-slate-100 text-slate-700 hover:bg-slate-200 dark:bg-[#30363d] dark:text-[#adbac7]'}"
          aria-label={isPaused ? translate('sidebar.resume') : translate('sidebar.pause')}
          title={isPaused ? translate('sidebar.resume') : translate('sidebar.pause')}
        >
          <span class="sidebar-recording-toggle-label">
            {#if isPaused}{translate('sidebar.resume')}{:else}{translate('sidebar.pause')}{/if}
          </span>
          {#if isPaused}
            <svg class="sidebar-recording-toggle-icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M8 5v14l11-7z" />
            </svg>
          {:else}
            <svg class="sidebar-recording-toggle-icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M7 5h4v14H7zM13 5h4v14h-4z" />
            </svg>
          {/if}
        </button>
      </div>
    </div>
  </div>

  <div class="sidebar-main">
    <!-- 导航菜单 -->
    <nav class="sidebar-nav sidebar-nav-section">
      <ul class="sidebar-nav-list">
        {#each navItems as item}
          <li>
            <a href={item.path} use:link
              aria-label={translate(item.labelKey)}
              aria-current={activeStates[item.path] ? 'page' : undefined}
              class="group sidebar-nav-item
                {activeStates[item.path]
                  ? 'sidebar-nav-item-active'
                  : 'sidebar-nav-item-idle'}">

              <div class="sidebar-nav-main">
                <!-- SVG 图标 -->
                <div class="sidebar-nav-icon {activeStates[item.path] ? 'text-indigo-600 dark:text-indigo-400' : 'text-slate-400 group-hover:text-slate-500 dark:group-hover:text-slate-400'}">
                  {#if item.icon === 'home'}
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6" />
                    </svg>
                  {:else if item.icon === 'timeline'}
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                  {:else if item.icon === 'report'}
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 17v-2m3 2v-4m3 4v-6m2 10H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                    </svg>
                  {:else if item.icon === 'ask'}
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M8 10h8M8 14h4m-6 6h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" />
                    </svg>
                  {:else if item.icon === 'settings'}
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                    </svg>
                  {:else if item.icon === 'info'}
                    <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                  {/if}
                </div>

                <span class="sidebar-nav-label {activeStates[item.path] ? 'sidebar-nav-label-active' : ''}">{translate(item.labelKey)}</span>
              </div>
            </a>
          </li>
        {/each}
      </ul>
    </nav>

    <!-- 底部工具栏 -->
    <div class="sidebar-bottom sidebar-toolbelt">
      <div class="sidebar-footer w-full justify-between gap-y-2">

        <div class="relative" bind:this={localeMenuContainer}>
          <button
            type="button"
            class="sidebar-locale-switch locale-switch inline-flex h-8 min-w-[72px] items-center justify-center gap-1.5 rounded-lg border border-slate-200/80 bg-white/90 px-3 text-[11px] font-semibold tracking-normal text-slate-700 shadow-[inset_0_1px_0_rgba(255,255,255,0.6)] outline-none dark:shadow-none transition hover:border-slate-300 hover:text-slate-900 focus:ring-2 focus:ring-slate-300 dark:border-[#30363d]/80 dark:bg-[#161b22]/80 dark:text-[#c9d1d9] dark:hover:border-[#484f58] dark:hover:text-[#e6edf3] dark:focus:ring-primary-600"
            aria-label={translate('sidebar.localeButtonTitle')}
            aria-haspopup="menu"
            aria-expanded={localeMenuOpen}
            title={translate('sidebar.localeButtonTitle')}
            on:click={toggleLocaleMenu}
          >
            <svg class="sidebar-locale-compact-icon" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="12" cy="12" r="9" stroke-width="1.8" />
              <path stroke-linecap="round" stroke-width="1.8" d="M3 12h18M12 3a15 15 0 010 18M12 3a15 15 0 000 18" />
            </svg>
            <span class="sidebar-locale-label leading-none">{currentLocaleLabel}</span>
            <svg class="sidebar-locale-chevron h-3 w-3 shrink-0 text-slate-400 transition-transform {localeMenuOpen ? 'rotate-180' : ''}" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 9l6 6 6-6" />
            </svg>
          </button>

          {#if localeMenuOpen}
            <div
              class="absolute bottom-full start-0 mb-2 min-w-[148px] rounded-[var(--radius-md)] border border-slate-200/80 bg-white/96 p-1.5 shadow-xl dark:shadow-[0_12px_32px_rgba(0,0,0,0.5)] backdrop-blur dark:border-[#30363d]/80 dark:bg-[#161b22]/96"
              role="menu"
            >
              {#each localeOptions as option}
                <button
                  type="button"
                  class="flex w-full items-center gap-2 whitespace-nowrap rounded-xl px-3 py-2 text-start text-xs font-medium transition-colors {currentLocale === option.value ? 'bg-slate-200/80 text-slate-900 dark:bg-[#30363d] dark:text-[#e6edf3]' : 'text-slate-500 hover:bg-slate-50 hover:text-slate-900 dark:text-[#adbac7] dark:hover:bg-[#21262d]/80 dark:hover:text-[#e6edf3]'}"
                  role="menuitemradio"
                  aria-checked={currentLocale === option.value}
                  on:click={() => selectLocale(option.value)}
                >
                  <span class="font-semibold tracking-[0.08em] text-slate-500 dark:text-[#7d8590]">{option.label}</span>
                  <span class="text-slate-700 dark:text-[#c9d1d9]">{option.fullLabel}</span>
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <button on:click={cycleTheme}
          class="sidebar-footer-action"
          title="{theme === 'system' ? translate('sidebar.themeTitle.system') : theme === 'light' ? translate('sidebar.themeTitle.light') : translate('sidebar.themeTitle.dark')}">
          {#if theme === 'system'}
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" /></svg>
          {:else if theme === 'light'}
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" /></svg>
          {:else}
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" /></svg>
          {/if}
        </button>
      </div>
    </div>
  </div>
</div>
