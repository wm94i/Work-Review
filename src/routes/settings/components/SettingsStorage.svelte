<script>
  import { createEventDispatcher } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { ask, open as openDialog } from '@tauri-apps/plugin-dialog';
  import { cache } from '../../../lib/stores/cache.js';
  import { locale, t } from '$lib/i18n/index.js';
  import { showToast } from '$lib/stores/toast.js';
  
  export let config;
  export let storageStats = null;
  export let dataDir = '';
  export let defaultDataDir = '';
  
  const dispatch = createEventDispatcher();
  $: currentLocale = $locale;
  let isClearing = false;
  let isMigrating = false;
  let isCleaningPreviousDir = false;
  let cleanupCandidateDir = '';
  let localizedScreenshotModes = [];
  let screenshotIntervalLabel = '';
  let retentionDaysLabel = '';
  let storageRetentionLabel = '';
  let keepForever = false;
  let isTestingRemote = false;
  let s3SecretKeyVisible = false;
  let s3AccessKeyVisible = false;
  let webdavPasswordVisible = false;
  const screenshotModes = [
    {
      value: 'active_window',
      labelKey: 'settingsStorage.modeActiveWindow',
      descriptionKey: 'settingsStorage.modeActiveWindowDesc',
    },
    {
      value: 'all',
      labelKey: 'settingsStorage.modeAll',
      descriptionKey: 'settingsStorage.modeAllDesc',
    },
  ];
  $: {
    currentLocale;
    localizedScreenshotModes = screenshotModes.map((mode) => ({
      ...mode,
      label: t(mode.labelKey),
      description: t(mode.descriptionKey),
    }));
  }

  function clearCache() {
    cache.clear();
    showToast(t('settingsStorage.clearCacheAction'));
    dispatch('clearCache');
  }

  async function clearOldData() {
    const confirmed = await ask(t('settingsStorage.clearHistoryConfirmMessage'), {
      title: t('settingsStorage.clearHistoryConfirmTitle'),
      kind: 'warning',
    });

    if (!confirmed) {
      return;
    }
    
    isClearing = true;
    try {
      await invoke('clear_old_activities');
      showToast(t('settingsStorage.clearDone'), 'success');
      cache.clear();
      dispatch('clearCache');
    } catch (e) {
      showToast(t('settingsStorage.clearFailed', { error: e }), 'error');
    } finally {
      isClearing = false;
    }
  }

  async function migrateToDataDir(targetDir) {
    const nextDir = targetDir?.trim();
    if (!nextDir) {
      return;
    }

    if (nextDir === dataDir) {
      showToast(t('settingsStorage.alreadyCurrentDir'));
      return;
    }

    const confirmed = await ask(
      t('settingsStorage.migrateConfirmMessage', { dir: nextDir }),
      {
        title: t('settingsStorage.migrateConfirmTitle'),
        kind: 'warning',
      },
    );

    if (!confirmed) {
      return;
    }

    isMigrating = true;
    try {
      const result = await invoke('change_data_dir', { targetDir: nextDir });
      cleanupCandidateDir = result?.oldDataDir || dataDir;
      showToast(t('settingsStorage.migrated'), 'success');
      dispatch('dataDirChanged', result);
    } catch (e) {
      showToast(t('settingsStorage.migrateFailed', { error: e }), 'error');
    } finally {
      isMigrating = false;
    }
  }

  async function pickDataDir() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      defaultPath: dataDir || defaultDataDir || undefined,
    });

    if (!selected || Array.isArray(selected)) {
      return;
    }

    await migrateToDataDir(selected);
  }

  async function restoreDefaultDataDir() {
    await migrateToDataDir(defaultDataDir);
  }

  async function openCurrentDataDir() {
    try {
      await invoke('open_data_dir');
    } catch (e) {
      showToast(t('settingsStorage.openDirFailed', { error: e }), 'error');
    }
  }

  async function cleanupPreviousDataDir() {
    const targetDir = cleanupCandidateDir?.trim();
    if (!targetDir || isCleaningPreviousDir) {
      return;
    }

    const confirmed = await ask(
      t('settingsStorage.cleanupOldConfirmMessage', { dir: targetDir }),
      {
        title: t('settingsStorage.cleanupOldConfirmTitle'),
        kind: 'warning',
      },
    );

    if (!confirmed) {
      return;
    }

    isCleaningPreviousDir = true;
    try {
      await invoke('cleanup_old_data_dir', { targetDir });
      cleanupCandidateDir = '';
      showToast(t('settingsStorage.oldDirCleaned'), 'success');
    } catch (e) {
      showToast(t('settingsStorage.cleanupOldFailed', { error: e }), 'error');
    } finally {
      isCleaningPreviousDir = false;
    }
  }

  function handleChange() {
    dispatch('change', config);
  }

  async function pickDailyReportExportDir() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      defaultPath: config.daily_report_export_dir || dataDir || defaultDataDir || undefined,
    });

    if (!selected || Array.isArray(selected)) {
      return;
    }

    config.daily_report_export_dir = selected;
    handleChange();
  }

  function clearDailyReportExportDir() {
    config.daily_report_export_dir = null;
    handleChange();
  }

  async function testRemoteStorage() {
    isTestingRemote = true;
    try {
      const result = await invoke('test_remote_storage');
      showToast(result, 'success');
    } catch (e) {
      showToast(t('settingsStorage.testConnectionFailed', { error: e }), 'error');
    } finally {
      isTestingRemote = false;
    }
  }

  // 计算存储使用百分比
  $: usagePercent = storageStats 
    ? Math.min(Math.round((storageStats.total_size_mb / storageStats.storage_limit_mb) * 100), 100) 
    : 0;

  // 使用量颜色
  $: usageColor = usagePercent > 80 ? 'bg-red-500' : usagePercent > 50 ? 'bg-amber-500' : 'bg-emerald-500';
  $: usingDefaultDataDir = dataDir && defaultDataDir && dataDir === defaultDataDir;
  $: {
    currentLocale;
    screenshotIntervalLabel = t('settingsStorage.secondsValue', { count: config?.screenshot_interval ?? 0 });
    retentionDaysLabel = t('settingsStorage.daysValue', { count: config?.storage?.screenshot_retention_days ?? 0 });
    keepForever = config?.storage?.screenshot_retention_days === 0;
    storageRetentionLabel = t('settingsStorage.daysValue', { count: storageStats?.retention_days ?? 0 });
  }
  $: if (cleanupCandidateDir && cleanupCandidateDir === dataDir) {
    cleanupCandidateDir = '';
  }
  $: {
    if (!config.remote_storage) config.remote_storage = { provider: 'none', s3: {}, webdav: {} };
    if (!config.remote_storage.s3) config.remote_storage.s3 = {};
    if (!config.remote_storage.webdav) config.remote_storage.webdav = {};
  }
</script>

<!-- 截图与保留 -->
<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsStorage.screenshotCardTitle')}</h3>
  
  <div class="settings-section">
    <div class="settings-row">
      <div>
        <span class="settings-text">{t('settingsStorage.screenshotsEnabled')}</span>
        <p class="settings-muted mt-0.5">{t('settingsStorage.screenshotsEnabledHint')}</p>
      </div>
      <button
        type="button"
        on:click={() => {
          config.storage.screenshots_enabled = !config.storage.screenshots_enabled;
          handleChange();
        }}
        class="switch-track {config.storage.screenshots_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-slate-600'}"
        aria-pressed={config.storage.screenshots_enabled}
      >
        <span class="switch-thumb {config.storage.screenshots_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
      </button>
    </div>

    <!-- 轮询间隔 -->
    <div class="settings-block">
      <div class="flex items-center justify-between">
        <label for="screenshot-interval" class="settings-text">{t('settingsStorage.pollingInterval')}</label>
        <div class="flex items-center gap-2">
          <input
            type="number"
            min="5"
            max="600"
            step="5"
            bind:value={config.screenshot_interval}
            on:change={() => {
              config.screenshot_interval = Math.max(5, Math.min(600, Number(config.screenshot_interval) || 30));
              handleChange();
            }}
            class="w-16 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-slate-600 dark:bg-slate-800"
          />
          <span class="text-xs settings-subtle">{t('settingsStorage.secondsUnit')}</span>
        </div>
      </div>
      <input
        id="screenshot-interval"
        type="range"
        bind:value={config.screenshot_interval}
        on:change={() => {
          config.screenshot_interval = Math.max(5, Math.min(600, Number(config.screenshot_interval) || 30));
          handleChange();
        }}
        min="10"
        max="120"
        step="5"
        class="range-input"
      />
      <div class="flex justify-between text-xs settings-subtle">
        <span>{t('settingsStorage.precise')}</span>
        <span>{t('settingsStorage.powerSave')}</span>
      </div>
    </div>

    <!-- 数据保留 -->
    <div class="settings-block">
      <div class="flex items-center justify-between">
        <label for="retention-days" class="settings-text">{t('settingsStorage.retentionDays')}</label>
        <div class="flex items-center gap-2">
          <label class="flex items-center gap-1.5 text-xs settings-subtle cursor-pointer select-none">
            <input
              type="checkbox"
              checked={keepForever}
              on:click={() => {
                if (!keepForever) {
                  config.storage.screenshot_retention_days = 0;
                } else {
                  config.storage.screenshot_retention_days = 7;
                }
                handleChange();
              }}
              class="rounded border-slate-300 dark:border-slate-600"
            />
            {t('settingsStorage.keepForever')}
          </label>
          {#if !keepForever}
            <input
              type="number"
              min="1"
              max="9999"
              step="1"
              bind:value={config.storage.screenshot_retention_days}
              on:change={() => {
                config.storage.screenshot_retention_days = Math.max(1, Number(config.storage.screenshot_retention_days) || 7);
                handleChange();
              }}
              class="w-16 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-slate-600 dark:bg-slate-800"
            />
          {:else}
            <span class="settings-value">∞</span>
          {/if}
        </div>
      </div>
      {#if !keepForever}
        <input
          id="retention-days"
          type="range"
          bind:value={config.storage.screenshot_retention_days}
          on:change={() => {
            config.storage.screenshot_retention_days = Math.max(1, Number(config.storage.screenshot_retention_days) || 7);
            handleChange();
          }}
          min="1"
          max="90"
          step="1"
          class="range-input"
        />
        <div class="flex justify-between text-xs settings-subtle">
          <span>{t('settingsStorage.retentionMin')}</span>
          <span>{t('settingsStorage.retentionMax')}</span>
        </div>
      {/if}
    </div>

    <div class="settings-block">
      <p class="settings-text mb-2">{t('settingsStorage.screenshotMode')}</p>
      <div class="flex gap-2">
        {#each localizedScreenshotModes as mode}
          <button
            type="button"
            on:click={() => {
              config.storage.screenshot_display_mode = mode.value;
              handleChange();
            }}
            class="flex-1 min-h-16 px-3 py-2.5 rounded-lg text-sm font-medium leading-none transition-all duration-150
                   {config.storage.screenshot_display_mode === mode.value
                     ? 'settings-segment-active'
                     : 'settings-segment-base'}"
          >
            <div class="flex h-full flex-col items-center justify-center gap-1 text-center">
              <div class="leading-none">{mode.label}</div>
              <div class="text-[10px] leading-snug {config.storage.screenshot_display_mode === mode.value ? 'text-white/70' : 'settings-subtle'}">
                {mode.description}
              </div>
            </div>
          </button>
        {/each}
      </div>
    </div>
  </div>
</div>

<!-- 截图分辨率 -->
<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsStorage.widthMode')}</h3>

  <div class="settings-section">
    <div class="settings-block">
      <div class="flex gap-2">
        <button
          type="button"
          on:click={() => {
            config.storage.screenshot_width_mode = 'auto';
            handleChange();
          }}
          class="flex-1 min-h-16 px-3 py-2.5 rounded-lg text-sm font-medium leading-none transition-all duration-150
                 {config.storage.screenshot_width_mode === 'auto'
                   ? 'settings-segment-active'
                   : 'settings-segment-base'}"
        >
          <div class="flex h-full flex-col items-center justify-center gap-1 text-center">
            <div class="leading-none">{t('settingsStorage.widthModeAuto')}</div>
            <div class="text-[10px] leading-snug {config.storage.screenshot_width_mode === 'auto' ? 'text-white/70' : 'settings-subtle'}">
              {t('settingsStorage.widthModeAutoDesc')}
            </div>
          </div>
        </button>
        <button
          type="button"
          on:click={() => {
            config.storage.screenshot_width_mode = 'fixed';
            handleChange();
          }}
          class="flex-1 min-h-16 px-3 py-2.5 rounded-lg text-sm font-medium leading-none transition-all duration-150
                 {config.storage.screenshot_width_mode === 'fixed'
                   ? 'settings-segment-active'
                   : 'settings-segment-base'}"
        >
          <div class="flex h-full flex-col items-center justify-center gap-1 text-center">
            <div class="leading-none">{t('settingsStorage.widthModeFixed')}</div>
            <div class="text-[10px] leading-snug {config.storage.screenshot_width_mode === 'fixed' ? 'text-white/70' : 'settings-subtle'}">
              {t('settingsStorage.widthModeFixedDesc')}
            </div>
          </div>
        </button>
      </div>
    </div>

    {#if config.storage.screenshot_width_mode === 'fixed'}
      <div class="settings-block">
        <div class="flex items-center justify-between">
          <label for="max-image-width" class="settings-text">{t('settingsStorage.maxWidth')}</label>
          <span class="settings-value">{config.storage.max_image_width}px</span>
        </div>
        <input
          id="max-image-width"
          type="range"
          bind:value={config.storage.max_image_width}
          on:change={handleChange}
          min="640"
          max="3840"
          step="64"
          class="range-input"
        />
        <div class="flex justify-between text-xs settings-subtle">
          <span>640px</span>
          <span>3840px</span>
        </div>
      </div>
    {/if}
  </div>
</div>

<!-- 日报导出 -->
<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsStorage.exportTitle')}</h3>

  <div class="settings-block">
    <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 dark:border-slate-700/80 dark:bg-slate-800/40">
      <p class="settings-text">{t('settingsStorage.exportDir')}</p>
      <p class="settings-muted mt-1 break-all">
        {config.daily_report_export_dir || t('settingsStorage.notSet')}
      </p>
      <div class="mt-4 flex flex-wrap gap-3">
        <button
          type="button"
          on:click={pickDailyReportExportDir}
          class="settings-action-secondary"
        >
          {t('settingsStorage.chooseDir')}
        </button>
        {#if config.daily_report_export_dir}
          <button
            type="button"
            on:click={clearDailyReportExportDir}
            class="settings-action-secondary"
          >
            {t('settingsStorage.clearDir')}
          </button>
        {/if}
      </div>

      <div class="mt-4 flex items-center justify-between">
        <div>
          <p class="settings-text">{t('settingsStorage.autoExport')}</p>
          <p class="settings-muted mt-0.5">{t('settingsStorage.autoExportHint')}</p>
        </div>
        <button
          type="button"
          class="switch-track {config.daily_report_auto_export ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-slate-600'} {!config.daily_report_export_dir ? 'opacity-60 cursor-not-allowed' : ''}"
          on:click={() => { if (config.daily_report_export_dir) config.daily_report_auto_export = !config.daily_report_auto_export; }}
          disabled={!config.daily_report_export_dir}
        >
          <span class="switch-thumb {config.daily_report_auto_export ? 'translate-x-5' : 'translate-x-0'}"></span>
        </button>
      </div>
    </div>
  </div>
</div>

<!-- 远程存储 -->
<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsStorage.remoteStorageTitle')}</h3>
  <p class="settings-card-desc mb-0">{t('settingsStorage.remoteStorageDesc')}</p>

  <div class="settings-section space-y-4">
    <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 dark:border-slate-700/80 dark:bg-slate-800/40">
      <div class="flex items-center gap-2 mb-3">
        <div class="flex h-6 w-6 items-center justify-center rounded-md bg-primary-100 dark:bg-primary-900/30">
          <svg class="w-3.5 h-3.5 text-primary-600 dark:text-primary-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" />
          </svg>
        </div>
        <span class="text-sm font-medium text-slate-700 dark:text-slate-200">{t('settingsStorage.remoteProvider')}</span>
      </div>
      <div class="flex gap-2">
        {#each [
          { value: 'none', label: t('settingsStorage.remoteProviderNone') },
          { value: 's3', label: t('settingsStorage.remoteProviderS3') },
          { value: 'webdav', label: t('settingsStorage.remoteProviderWebDav') },
        ] as opt}
          <button
            type="button"
            on:click={() => {
              if (!config.remote_storage) config.remote_storage = { provider: 'none', s3: {}, webdav: {} };
              config.remote_storage = { ...config.remote_storage, provider: opt.value };
              handleChange();
            }}
            class="flex-1 min-h-9 px-3 py-2 text-xs font-medium rounded-lg leading-none transition-all
                   {(config.remote_storage?.provider || 'none') === opt.value
                     ? 'settings-segment-active'
                     : 'settings-segment-base'}"
          >
            {opt.label}
          </button>
        {/each}
      </div>
    </div>

    {#if config.remote_storage?.provider === 's3'}
      <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 space-y-3 dark:border-slate-700/80 dark:bg-slate-800/40">
        <div class="flex items-center gap-2">
          <div class="flex h-6 w-6 items-center justify-center rounded-md bg-amber-100 dark:bg-amber-900/30">
            <svg class="w-3.5 h-3.5 text-amber-600 dark:text-amber-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4" />
            </svg>
          </div>
          <span class="text-sm font-medium text-slate-700 dark:text-slate-200">S3 / MinIO</span>
          <span class="settings-chip-success">{t('settingsStorage.remoteProviderS3')}</span>
        </div>

        <div class="grid gap-2 grid-cols-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.s3Endpoint')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.s3.endpoint}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
              placeholder={t('settingsStorage.s3EndpointHint')}
            />
          </label>
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.s3Bucket')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.s3.bucket}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
              placeholder="my-bucket"
            />
          </label>
        </div>

        <div class="grid gap-2 grid-cols-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.s3AccessKey')}</span>
            <div class="mt-0.5 relative">
              {#if s3AccessKeyVisible}
                <input
                  type="text"
                  bind:value={config.remote_storage.s3.access_key}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Access Key"
                  autocomplete="off"
                />
              {:else}
                <input
                  type="password"
                  bind:value={config.remote_storage.s3.access_key}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Access Key"
                  autocomplete="off"
                />
              {/if}
              <button
                type="button"
                class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-600 dark:hover:text-slate-300"
                on:click={() => (s3AccessKeyVisible = !s3AccessKeyVisible)}
              >
                {#if s3AccessKeyVisible}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
                {:else}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                {/if}
              </button>
            </div>
          </label>
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.s3SecretKey')}</span>
            <div class="mt-0.5 relative">
              {#if s3SecretKeyVisible}
                <input
                  type="text"
                  bind:value={config.remote_storage.s3.secret_key}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Secret Key"
                  autocomplete="off"
                />
              {:else}
                <input
                  type="password"
                  bind:value={config.remote_storage.s3.secret_key}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Secret Key"
                  autocomplete="off"
                />
              {/if}
              <button
                type="button"
                class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-600 dark:hover:text-slate-300"
                on:click={() => (s3SecretKeyVisible = !s3SecretKeyVisible)}
              >
                {#if s3SecretKeyVisible}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
                {:else}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                {/if}
              </button>
            </div>
          </label>
        </div>

        <div class="grid gap-2 grid-cols-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.s3Region')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.s3.region}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
              placeholder="us-east-1"
            />
          </label>
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.s3PathPrefix')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.s3.path_prefix}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
              placeholder={t('settingsStorage.s3PathPrefixHint')}
            />
          </label>
        </div>

        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.s3PublicUrlBase')}</span>
          <input
            type="text"
            bind:value={config.remote_storage.s3.public_url_base}
            on:blur={handleChange}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
            placeholder={t('settingsStorage.s3PublicUrlBaseHint')}
          />
        </label>

        <div class="flex items-center justify-between pt-1">
          <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('settingsStorage.publicUrlBaseEffectHint')}</p>
          <button
            type="button"
            class="settings-action-secondary"
            disabled={isTestingRemote}
            on:click={testRemoteStorage}
          >
            {isTestingRemote ? t('settingsStorage.testConnectionTesting') : t('settingsStorage.testConnection')}
          </button>
        </div>
      </div>
    {/if}

    {#if config.remote_storage?.provider === 'webdav'}
      <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 space-y-3 dark:border-slate-700/80 dark:bg-slate-800/40">
        <div class="flex items-center gap-2">
          <div class="flex h-6 w-6 items-center justify-center rounded-md bg-blue-100 dark:bg-blue-900/30">
            <svg class="w-3.5 h-3.5 text-blue-600 dark:text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2" />
            </svg>
          </div>
          <span class="text-sm font-medium text-slate-700 dark:text-slate-200">WebDAV</span>
          <span class="settings-chip-success">{t('settingsStorage.remoteProviderWebDav')}</span>
        </div>

        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.webdavUrl')}</span>
          <input
            type="text"
            bind:value={config.remote_storage.webdav.url}
            on:blur={handleChange}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
            placeholder={t('settingsStorage.webdavUrlHint')}
          />
        </label>

        <div class="grid gap-2 grid-cols-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.webdavUsername')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.webdav.username}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
              placeholder="username"
            />
          </label>
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.webdavPassword')}</span>
            <div class="mt-0.5 relative">
              {#if webdavPasswordVisible}
                <input
                  type="text"
                  bind:value={config.remote_storage.webdav.password}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="password"
                  autocomplete="off"
                />
              {:else}
                <input
                  type="password"
                  bind:value={config.remote_storage.webdav.password}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="password"
                  autocomplete="off"
                />
              {/if}
              <button
                type="button"
                class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-600 dark:hover:text-slate-300"
                on:click={() => (webdavPasswordVisible = !webdavPasswordVisible)}
              >
                {#if webdavPasswordVisible}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.878 9.878L6.59 6.59m7.532 7.532l3.29 3.29M3 3l18 18" /></svg>
                {:else}
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                {/if}
              </button>
            </div>
          </label>
        </div>

        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.webdavPathPrefix')}</span>
          <input
            type="text"
            bind:value={config.remote_storage.webdav.path_prefix}
            on:blur={handleChange}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
            placeholder={t('settingsStorage.webdavPathPrefixHint')}
          />
        </label>

        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-slate-400">{t('settingsStorage.webdavPublicUrlBase')}</span>
          <input
            type="text"
            bind:value={config.remote_storage.webdav.public_url_base}
            on:blur={handleChange}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-800 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-slate-700/50 dark:text-white dark:ring-slate-600 dark:focus:ring-primary-600 focus:outline-none"
            placeholder={t('settingsStorage.webdavPublicUrlBaseHint')}
          />
        </label>

        <div class="flex items-center justify-between pt-1">
          <p class="text-[11px] text-slate-400 dark:text-slate-500">{t('settingsStorage.publicUrlBaseEffectHint')}</p>
          <button
            type="button"
            class="settings-action-secondary"
            disabled={isTestingRemote}
            on:click={testRemoteStorage}
          >
            {isTestingRemote ? t('settingsStorage.testConnectionTesting') : t('settingsStorage.testConnection')}
          </button>
        </div>
      </div>
    {/if}
  </div>
</div>

<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsStorage.dataDirTitle')}</h3>

  <div class="settings-section">
    <div class="settings-block">
      <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 dark:border-slate-700/80 dark:bg-slate-800/40">
        <div class="grid gap-4 md:grid-cols-2">
          <div>
            <p class="settings-text">{t('settingsStorage.currentDir')}</p>
            <p class="settings-muted mt-1 break-all">{dataDir || t('common.loading')}</p>
          </div>
          <div>
            <p class="settings-text">{t('settingsStorage.defaultDir')}</p>
            <p class="settings-muted mt-1 break-all">{defaultDataDir || t('common.loading')}</p>
          </div>
        </div>

        <div class="mt-4 flex flex-wrap gap-3">
          <button
            on:click={pickDataDir}
            disabled={isMigrating}
            class="settings-action-secondary"
          >
            {#if isMigrating}
              {t('settingsStorage.migrating')}
            {:else}
              {t('settingsStorage.changeLocation')}
            {/if}
          </button>

          <button
            on:click={openCurrentDataDir}
            disabled={isMigrating}
            class="settings-action-secondary"
          >
            {t('settingsStorage.openCurrentDir')}
          </button>

          {#if !usingDefaultDataDir && defaultDataDir}
            <button
              on:click={restoreDefaultDataDir}
              disabled={isMigrating}
              class="settings-action-secondary"
            >
              {t('settingsStorage.restoreDefaultDir')}
            </button>
          {/if}
        </div>

        {#if cleanupCandidateDir}
          <div class="mt-4 rounded-xl border border-amber-200/70 bg-amber-50/90 p-3 dark:border-amber-500/30 dark:bg-amber-950/20">
            <p class="settings-text">{t('settingsStorage.oldDirPending')}</p>
            <p class="settings-muted mt-1 break-all">{cleanupCandidateDir}</p>
            <div class="mt-3 flex flex-wrap gap-3">
              <button
                on:click={cleanupPreviousDataDir}
                disabled={isCleaningPreviousDir || isMigrating}
                class="settings-action-secondary"
              >
                {#if isCleaningPreviousDir}
                  {t('settingsStorage.cleaning')}
                {:else}
                  {t('settingsStorage.cleanOldDir')}
                {/if}
              </button>
              <button
                on:click={() => cleanupCandidateDir = ''}
                disabled={isCleaningPreviousDir}
                class="settings-action-secondary"
              >
                {t('settingsStorage.later')}
              </button>
            </div>
          </div>
        {/if}
      </div>
    </div>

    {#if storageStats}
      <div class="settings-block">
        <div class="rounded-2xl border border-slate-200/80 bg-slate-50/90 p-4 dark:border-slate-700/80 dark:bg-slate-800/40">
          <div class="mb-5">
            <div class="mb-2 flex items-end justify-between">
              <div>
                <span class="text-2xl font-bold text-slate-800 dark:text-white">{storageStats.total_size_mb}</span>
                <span class="settings-muted"> / {storageStats.storage_limit_mb} MB</span>
              </div>
              <span class="text-sm font-medium {usagePercent > 80 ? 'settings-text-danger' : 'settings-muted'}">{usagePercent}%</span>
            </div>
            <div class="h-2.5 w-full overflow-hidden rounded-full bg-slate-100 dark:bg-slate-700">
              <div
                class="h-full rounded-full transition-all duration-500 {usageColor}"
                style="width: {usagePercent}%"
              ></div>
            </div>
          </div>

          <div class="grid grid-cols-3 gap-3">
            <div class="rounded-xl bg-white/70 p-3 text-center ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
              <p class="text-xl font-bold text-slate-800 dark:text-white">{storageStats.total_files}</p>
              <p class="settings-muted mt-0.5">{t('settingsStorage.screenshotsCount')}</p>
            </div>
            <div class="rounded-xl bg-white/70 p-3 text-center ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
              <p class="text-xl font-bold text-slate-800 dark:text-white">{storageStats.total_size_mb} MB</p>
              <p class="settings-muted mt-0.5">{t('settingsStorage.usedSpace')}</p>
            </div>
            <div class="rounded-xl bg-white/70 p-3 text-center ring-1 ring-slate-200/70 dark:bg-slate-900/20 dark:ring-slate-700/70">
              <p class="text-xl font-bold text-slate-800 dark:text-white">{storageRetentionLabel}</p>
              <p class="settings-muted mt-0.5">{t('settingsStorage.retentionPeriod')}</p>
            </div>
          </div>
        </div>
      </div>
    {/if}

    <div class="space-y-2.5">
      <div class="settings-row">
        <span class="settings-text">{t('settingsStorage.clearCache')}</span>
        <button
          on:click={clearCache}
          class="settings-action-secondary"
        >
          {t('settingsStorage.clearCacheAction')}
        </button>
      </div>

      <div class="settings-panel-danger flex items-center justify-between">
        <span class="settings-text-danger text-sm font-medium">{t('settingsStorage.clearHistory')}</span>
        <button
          on:click={clearOldData}
          disabled={isClearing}
          class="settings-action-danger"
        >
          {#if isClearing}
            {t('settingsStorage.cleaning')}
          {:else}
            {t('settingsStorage.clearHistoryAction')}
          {/if}
        </button>
      </div>
    </div>
  </div>
</div>
