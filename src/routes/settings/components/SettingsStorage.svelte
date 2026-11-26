<script lang="ts">
  import { createEventDispatcher, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { ask, open as openDialog } from '@tauri-apps/plugin-dialog';
  import { cache } from '../../../lib/stores/cache.ts';
  import { locale, t } from '$lib/i18n/index.ts';
  import { showToast } from '$lib/stores/toast.ts';
  import { formatUserError } from '$lib/utils/errorDisplay.ts';
  import CollapsibleSection from '../../../lib/components/CollapsibleSection.svelte';
  import LocalizedDatePicker from '../../../lib/components/LocalizedDatePicker.svelte';

  interface ScreenshotStorageSettings {
    screenshots_enabled: boolean;
    screenshot_retention_days: number;
    metadata_cleanup_enabled: boolean;
    metadata_retention_days: number;
    screenshot_display_mode: string;
    screenshot_width_mode: string;
    max_image_width: number;
    storage_limit_mb: number;
  }

  interface S3StorageSettings {
    endpoint?: string;
    bucket?: string;
    access_key?: string;
    secret_key?: string;
    region?: string;
    path_prefix?: string;
    public_url_base?: string;
  }

  interface WebDavStorageSettings {
    url?: string;
    username?: string;
    password?: string;
    path_prefix?: string;
    public_url_base?: string;
  }

  interface RemoteStorageSettings {
    provider: string;
    s3: S3StorageSettings;
    webdav: WebDavStorageSettings;
    sync_app_config?: boolean;
  }

  interface StorageConfig {
    screenshot_interval: number;
    storage: ScreenshotStorageSettings;
    daily_report_export_dir: string | null;
    daily_report_auto_export: boolean;
    daily_report_export_frontmatter?: boolean;
    daily_report_export_section_heading?: string | null;
    git_report_enabled?: boolean;
    git_repo_paths?: string[];
    git_author_filter?: string | null;
    remote_storage?: RemoteStorageSettings;
  }

  interface GitCommitEntry {
    hash: string;
    timestamp: number;
    timeLabel: string;
    subject: string;
  }

  interface GitRepoSummary {
    repoName: string;
    repoPath: string;
    branch?: string | null;
    commits: GitCommitEntry[];
  }

  interface StorageStats {
    total_size_mb: number;
    storage_limit_mb: number;
    retention_days: number;
    total_files: number;
  }

  interface ScreenshotMode {
    value: string;
    labelKey: string;
    descriptionKey: string;
  }

  interface LocalizedScreenshotMode extends ScreenshotMode {
    label: string;
    description: string;
  }

  interface ChangeDataDirResult {
    dataDir: string;
    oldDataDir?: string;
    copiedFiles: number;
    replacedExistingData?: boolean;
    message: string;
  }

  interface ClearOldActivitiesResult {
    deleted_activities: number;
    deleted_screenshots: number;
    failed_screenshot_paths: string[];
    kept_dates: string[];
    message: string;
  }

  interface CleanupOldDataDirResult {
    removedEntries: number;
    preservedEntries: string[];
    message: string;
  }

  type DataExportFormat = 'csv' | 'json';

  interface ExportActivitiesResult {
    path: string;
    count: number;
    format: DataExportFormat;
  }

  interface ExportDailyStatsResult {
    path: string;
    days: number;
    format: DataExportFormat;
  }

  interface CreateBackupResult {
    backupDir: string;
    fileCount: number;
    totalBytes: number;
    includesScreenshots: boolean;
    includesSecrets: boolean;
  }

  interface RestoreBackupResult {
    restoredFrom: string;
    safetyBackupDir: string;
    restoredFiles: number;
    schemaVersion: number;
  }
  
  export let config: StorageConfig;
  export let storageStats: StorageStats | null = null;
  export let dataDir = '';
  export let defaultDataDir = '';
  export let hasUnsavedChanges = false;
  export let restoreLocked = false;
  export let waitForPendingSave: () => Promise<void> = async () => {};
  
  const dispatch = createEventDispatcher<{
    change: StorageConfig | { autosaved: true };
    clearCache: void;
    dataDirChanged: ChangeDataDirResult;
    restoreStateChange: { restoring: boolean };
    backupRestored: RestoreBackupResult;
  }>();
  $: currentLocale = $locale;
  let isClearing = false;
  let isConfirmingMetadataCleanup = false;
  let isMigrating = false;
  let isCleaningPreviousDir = false;
  let cleanupCandidateDir = '';
  let localizedScreenshotModes: LocalizedScreenshotMode[] = [];
  let screenshotIntervalLabel = '';
  let retentionDaysLabel = '';
  let storageRetentionLabel = '';
  let keepForever = false;
  let isTestingRemote = false;
  let s3SecretKeyVisible = false;
  let s3AccessKeyVisible = false;
  let webdavPasswordVisible = false;
  let isPreviewingGit = false;
  let gitPreviewLoaded = false;
  let gitPreviewResults: GitRepoSummary[] = [];
  let gitPreviewError = '';
  let gitPreviewRequestId = 0;

  function resetGitPreview() {
    gitPreviewRequestId += 1;
    isPreviewingGit = false;
    gitPreviewLoaded = false;
    gitPreviewResults = [];
    gitPreviewError = '';
  }

  function getOffsetLocalIsoDate(offsetDays: number): string {
    const now = new Date();
    now.setDate(now.getDate() + offsetDays);
    const year = now.getFullYear();
    const month = String(now.getMonth() + 1).padStart(2, '0');
    const day = String(now.getDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
  }

  const dataExportFormats: DataExportFormat[] = ['csv', 'json'];
  let dataExportFrom = getOffsetLocalIsoDate(-6);
  let dataExportTo = getOffsetLocalIsoDate(0);
  let dataExportFormat: DataExportFormat = 'csv';
  let dataExportIncludeOcr = false;
  let isExportingData = false;

  let backupIncludeScreenshots = false;
  let backupIncludeSecrets = false;
  let isCreatingBackup = false;
  let isRestoringBackup = false;
  let restoreRestartPending = false;

  async function runExportActivities() {
    if (isExportingData) return;
    isExportingData = true;
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        defaultPath: config.daily_report_export_dir || dataDir || undefined,
      });
      if (!selected || Array.isArray(selected)) return;

      const result = await invoke<ExportActivitiesResult>('export_activities', {
        dateFrom: dataExportFrom,
        dateTo: dataExportTo,
        format: dataExportFormat,
        includeOcr: dataExportIncludeOcr,
        targetDir: selected,
      });
      showToast(
        t('settingsStorage.dataExportActivitiesSuccess', {
          count: result.count,
          path: result.path,
        }),
        'success',
      );
    } catch (e) {
      showToast(
        t('settingsStorage.dataExportFailed', {
          error: formatUserError(e, t('common.loadFailedRetry')),
        }),
        'error',
      );
    } finally {
      isExportingData = false;
    }
  }

  async function runExportDailyStats() {
    if (isExportingData) return;
    isExportingData = true;
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        defaultPath: config.daily_report_export_dir || dataDir || undefined,
      });
      if (!selected || Array.isArray(selected)) return;

      const result = await invoke<ExportDailyStatsResult>('export_daily_stats', {
        dateFrom: dataExportFrom,
        dateTo: dataExportTo,
        format: dataExportFormat,
        targetDir: selected,
      });
      showToast(
        t('settingsStorage.dataExportStatsSuccess', {
          days: result.days,
          path: result.path,
        }),
        'success',
      );
    } catch (e) {
      showToast(
        t('settingsStorage.dataExportFailed', {
          error: formatUserError(e, t('common.loadFailedRetry')),
        }),
        'error',
      );
    } finally {
      isExportingData = false;
    }
  }

  async function runCreateBackup() {
    if (isCreatingBackup || isRestoringBackup || restoreRestartPending || restoreLocked) return;
    isCreatingBackup = true;
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
      });
      if (!selected || Array.isArray(selected)) return;

      const result = await invoke<CreateBackupResult>('create_backup', {
        targetDir: selected,
        includeScreenshots: backupIncludeScreenshots,
        includeSecrets: backupIncludeSecrets,
      });
      showToast(
        t('settingsStorage.backupCreatedToast', {
          files: result.fileCount,
          path: result.backupDir,
        }),
        'success',
      );
    } catch (e) {
      showToast(
        t('settingsStorage.backupFailed', {
          error: formatUserError(e, t('common.loadFailedRetry')),
        }),
        'error',
      );
    } finally {
      isCreatingBackup = false;
    }
  }

  async function runRestoreBackup() {
    if (isCreatingBackup || isRestoringBackup || restoreRestartPending || restoreLocked) return;
    isRestoringBackup = true;
    cancelPendingPollingInterval();
    dispatch('restoreStateChange', { restoring: true });
    let restoredOk = false;
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
      });
      if (!selected || Array.isArray(selected)) return;

      const baseMsg = t('settingsStorage.restoreConfirmMessage', { dir: selected });
      const confirmMsg = hasUnsavedChanges
        ? `${baseMsg}\n\n${t('settingsStorage.restoreUnsavedWarning')}`
        : baseMsg;
      const confirmed = await ask(confirmMsg, {
        title: t('settingsStorage.restoreConfirmTitle'),
        kind: 'warning',
      });
      if (!confirmed) return;

      // 在触发后端恢复前，先取消排队并等待进行中的整份保存 / 轮询间隔补丁落盘完成，防止竞态覆盖。
      cancelPendingPollingInterval();
      await Promise.all([
        waitForPendingSave(),
        pollingFlushPromise ?? Promise.resolve(),
      ]);

      const result = await invoke<RestoreBackupResult>('restore_backup', {
        backupDir: selected,
      });
      restoredOk = true;
      restoreRestartPending = true;
      cache.clear();
      dispatch('backupRestored', result);
      showToast(
        t('settingsStorage.restoreSuccessToast', {
          files: result.restoredFiles,
        }),
        'success',
      );
    } catch (e) {
      showToast(
        t('settingsStorage.restoreFailed', {
          error: formatUserError(e, t('common.loadFailedRetry')),
        }),
        'error',
      );
    } finally {
      isRestoringBackup = false;
      if (!restoredOk) {
        dispatch('restoreStateChange', { restoring: false });
      }
    }
  }
  const screenshotModes: ScreenshotMode[] = [
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

  async function toggleMetadataCleanup() {
    if (isConfirmingMetadataCleanup) return;
    if (config.storage.metadata_cleanup_enabled) {
      config.storage.metadata_cleanup_enabled = false;
      handleChange();
      return;
    }
    isConfirmingMetadataCleanup = true;
    try {
      const confirmed = await ask(t('settingsStorage.metadataCleanupConfirm'), {
        title: t('settingsStorage.metadataCleanup'),
        kind: 'warning',
      });
      if (!confirmed) return;
      config.storage.metadata_cleanup_enabled = true;
      handleChange();
    } finally {
      isConfirmingMetadataCleanup = false;
    }
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
      await invoke<ClearOldActivitiesResult>('clear_old_activities');
      showToast(t('settingsStorage.clearDone'), 'success');
      cache.clear();
      dispatch('clearCache');
    } catch (e) {
      showToast(t('settingsStorage.clearFailed', { error: e }), 'error');
    } finally {
      isClearing = false;
    }
  }

  async function migrateToDataDir(targetDir: string) {
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
      const result = await invoke<ChangeDataDirResult>('change_data_dir', { targetDir: nextDir });
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
      await invoke<void>('open_data_dir');
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
      await invoke<CleanupOldDataDirResult>('cleanup_old_data_dir', { targetDir });
      cleanupCandidateDir = '';
      showToast(t('settingsStorage.oldDirCleaned'), 'success');
    } catch (e) {
      showToast(t('settingsStorage.cleanupOldFailed', { error: e }), 'error');
    } finally {
      isCleaningPreviousDir = false;
    }
  }

  let pollingSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let debouncedPollingInterval: number | null = null;
  let pendingPollingInterval: number | null = null;
  let inFlightPollingInterval: number | null = null;
  let pollingFlushInFlight = false;
  let pollingFlushPromise: Promise<void> | null = null;
  const POLLING_SAVE_FLASH_MS = 1200;
  let pollingInputFocused = false;
  let pollingInputValue = String(normalizePollingInterval(config?.screenshot_interval));
  let pollingSaveFlashPhase: 0 | 1 | 2 = 0;
  let pollingFlashTimer: ReturnType<typeof setTimeout> | null = null;
  let destroyed = false;

  function clearPollingFlashTimer() {
    if (pollingFlashTimer) {
      clearTimeout(pollingFlashTimer);
      pollingFlashTimer = null;
    }
  }

  function triggerPollingSaveFlash() {
    if (destroyed) return;
    clearPollingFlashTimer();
    pollingSaveFlashPhase = pollingSaveFlashPhase === 1 ? 2 : 1;
    pollingFlashTimer = setTimeout(() => {
      pollingFlashTimer = null;
      pollingSaveFlashPhase = 0;
    }, POLLING_SAVE_FLASH_MS);
  }

  function clearPollingSaveTimer() {
    if (pollingSaveTimer) {
      clearTimeout(pollingSaveTimer);
      pollingSaveTimer = null;
    }
    debouncedPollingInterval = null;
  }

  function cancelPendingPollingInterval() {
    clearPollingSaveTimer();
    pendingPollingInterval = null;
  }

  $: if (restoreLocked || restoreRestartPending || isRestoringBackup) {
    cancelPendingPollingInterval();
  }

  function normalizePollingInterval(raw: unknown, fallback = 30): number {
    const numeric = Number(raw);
    if (!Number.isFinite(numeric)) return fallback;
    return Math.max(5, Math.min(600, Math.round(numeric)));
  }

  $: pollingSliderValue =
    pendingPollingInterval
    ?? debouncedPollingInterval
    ?? inFlightPollingInterval
    ?? normalizePollingInterval(config?.screenshot_interval);

  $: if (
    !pollingInputFocused
    && pollingSaveTimer === null
    && debouncedPollingInterval === null
    && pendingPollingInterval === null
    && !pollingFlushInFlight
    && config
  ) {
    const synced = String(normalizePollingInterval(config.screenshot_interval));
    if (pollingInputValue !== synced) {
      pollingInputValue = synced;
    }
  }

  async function flushPollingIntervalQueue() {
    if (pollingFlushInFlight) return;
    if (restoreLocked || restoreRestartPending || isRestoringBackup) {
      pendingPollingInterval = null;
      return;
    }
    pollingFlushInFlight = true;
    const run = (async () => {
      try {
        while (pendingPollingInterval !== null) {
          if (restoreLocked || restoreRestartPending || isRestoringBackup) {
            pendingPollingInterval = null;
            return;
          }
          const nextValue = pendingPollingInterval;
          pendingPollingInterval = null;
          inFlightPollingInterval = nextValue;
          try {
            await invoke('patch_config', { patch: { screenshot_interval: nextValue } });
            if (
              !destroyed
              && config
              && pendingPollingInterval === null
              && debouncedPollingInterval === null
            ) {
              config.screenshot_interval = nextValue;
            }
            if (
              !destroyed
              && pendingPollingInterval === null
              && debouncedPollingInterval === null
              && !restoreLocked
              && !restoreRestartPending
              && !isRestoringBackup
            ) {
              triggerPollingSaveFlash();
              dispatch('change', { autosaved: true });
            }
          } catch (e) {
            if (
              pendingPollingInterval === null
              && !restoreLocked
              && !restoreRestartPending
              && !isRestoringBackup
            ) {
              if (!destroyed) {
                handleChange();
              }
              showToast(String(e), 'error');
            }
          } finally {
            inFlightPollingInterval = null;
          }
        }
      } finally {
        pollingFlushInFlight = false;
      }
    })();
    pollingFlushPromise = run;
    try {
      await run;
    } finally {
      if (pollingFlushPromise === run) {
        pollingFlushPromise = null;
      }
    }
  }

  function persistPollingInterval(nextValue: number) {
    if (restoreLocked || restoreRestartPending || isRestoringBackup) return;
    pendingPollingInterval = nextValue;
    void flushPollingIntervalQueue();
  }

  function scheduleDebouncedPollingSave(normalized: number) {
    if (restoreLocked || restoreRestartPending || isRestoringBackup) {
      cancelPendingPollingInterval();
      return;
    }
    clearPollingSaveTimer();
    debouncedPollingInterval = normalized;
    pollingSaveTimer = setTimeout(() => {
      pollingSaveTimer = null;
      const toSave = debouncedPollingInterval;
      debouncedPollingInterval = null;
      if (toSave !== null) {
        void persistPollingInterval(toSave);
      }
    }, 600);
  }

  function handlePollingNumberInput(event: Event) {
    const target = event.currentTarget as HTMLInputElement | null;
    const rawText = target ? target.value : pollingInputValue;
    pollingInputValue = rawText;

    if (restoreLocked || restoreRestartPending || isRestoringBackup) {
      cancelPendingPollingInterval();
      return;
    }

    if (rawText.trim() === '') {
      clearPollingSaveTimer();
      return;
    }

    const parsed = Number(rawText);
    if (!Number.isFinite(parsed)) {
      clearPollingSaveTimer();
      return;
    }

    const normalized = normalizePollingInterval(parsed);
    config.screenshot_interval = normalized;
    scheduleDebouncedPollingSave(normalized);
  }

  function handlePollingNumberCommit() {
    if (restoreLocked || restoreRestartPending || isRestoringBackup) {
      cancelPendingPollingInterval();
      return;
    }
    clearPollingSaveTimer();

    const fallback = normalizePollingInterval(config?.screenshot_interval);
    if (pollingInputValue.trim() === '') {
      pollingInputValue = String(fallback);
      return;
    }

    const normalized = normalizePollingInterval(pollingInputValue, fallback);
    pollingInputValue = String(normalized);
    config.screenshot_interval = normalized;
    void persistPollingInterval(normalized);
  }

  function handlePollingNumberBlur() {
    pollingInputFocused = false;
    if (restoreLocked || restoreRestartPending || isRestoringBackup) {
      cancelPendingPollingInterval();
      return;
    }
    const fallback = normalizePollingInterval(config?.screenshot_interval);
    if (pollingInputValue.trim() === '') {
      clearPollingSaveTimer();
      pollingInputValue = String(fallback);
      return;
    }
    const normalized = normalizePollingInterval(pollingInputValue, fallback);
    const hasPendingDebounce = pollingSaveTimer !== null || debouncedPollingInterval !== null;
    pollingInputValue = String(normalized);
    if (hasPendingDebounce || normalized !== config.screenshot_interval) {
      clearPollingSaveTimer();
      config.screenshot_interval = normalized;
      void persistPollingInterval(normalized);
    }
  }

  function handlePollingRangeInput(event: Event) {
    const target = event.currentTarget as HTMLInputElement | null;
    if (!target) return;
    if (restoreLocked || restoreRestartPending || isRestoringBackup) {
      cancelPendingPollingInterval();
      return;
    }
    const normalized = normalizePollingInterval(target.value, normalizePollingInterval(config?.screenshot_interval));
    pollingInputValue = String(normalized);
    config.screenshot_interval = normalized;
    scheduleDebouncedPollingSave(normalized);
  }

  function handlePollingRangeChange(event: Event) {
    const target = event.currentTarget as HTMLInputElement | null;
    if (!target) return;
    if (restoreLocked || restoreRestartPending || isRestoringBackup) {
      cancelPendingPollingInterval();
      return;
    }
    clearPollingSaveTimer();
    const normalized = normalizePollingInterval(target.value, normalizePollingInterval(config?.screenshot_interval));
    pollingInputValue = String(normalized);
    config.screenshot_interval = normalized;
    void persistPollingInterval(normalized);
  }

  onDestroy(() => {
    destroyed = true;
    clearPollingFlashTimer();
    const toFlush = debouncedPollingInterval;
    clearPollingSaveTimer();
    if (toFlush !== null && !restoreLocked && !restoreRestartPending && !isRestoringBackup) {
      persistPollingInterval(toFlush);
    }
  });

  function handleChange(_event?: Event) {
    if (restoreLocked || restoreRestartPending || isRestoringBackup) return;
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

  async function addGitRepoDir() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
    });
    if (!selected || Array.isArray(selected)) {
      return;
    }
    const existing = Array.isArray(config.git_repo_paths) ? config.git_repo_paths : [];
    // 保留目录选择器返回的原始路径，不做 trim() 以免破坏带尾随空格的合法目录名。
    if (!existing.some((item) => item === selected)) {
      config.git_repo_paths = [...existing, selected];
      resetGitPreview();
      handleChange();
    }
  }

  function removeGitRepoDir(pathToRemove: string) {
    config.git_repo_paths = (config.git_repo_paths || []).filter((item) => item !== pathToRemove);
    resetGitPreview();
    handleChange();
  }

  function getLocalIsoDate(): string {
    const now = new Date();
    const year = now.getFullYear();
    const month = String(now.getMonth() + 1).padStart(2, '0');
    const day = String(now.getDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
  }

  async function previewTodayGitCommits() {
    const repos = config.git_repo_paths || [];
    if (repos.length === 0) return;
    const reqId = ++gitPreviewRequestId;
    isPreviewingGit = true;
    gitPreviewError = '';
    try {
      const results = await invoke<GitRepoSummary[]>('get_git_commits_for_date', {
        date: getLocalIsoDate(),
        repoPaths: repos,
        authorFilter: config.git_author_filter?.trim() || null,
      });
      if (reqId !== gitPreviewRequestId) return;
      gitPreviewResults = results;
      gitPreviewLoaded = true;
    } catch (e) {
      if (reqId !== gitPreviewRequestId) return;
      gitPreviewError = String(e);
      gitPreviewResults = [];
      gitPreviewLoaded = false;
    } finally {
      if (reqId === gitPreviewRequestId) {
        isPreviewingGit = false;
      }
    }
  }

  $: gitPreviewCommitCount = gitPreviewResults.reduce((sum, repo) => sum + repo.commits.length, 0);

  async function testRemoteStorage() {
    isTestingRemote = true;
    try {
      // 传入表单当前值直接测试：设置需点「保存」才落盘，若测已保存配置，
      // 用户填完表单未保存就点测试会误报「未配置远程存储」
      const result = await invoke<string>('test_remote_storage', {
        remoteStorage: config.remote_storage,
      });
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
    if (!config.remote_storage) config.remote_storage = { provider: 'none', s3: {}, webdav: {}, sync_app_config: false };
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
        class="switch-track {config.storage.screenshots_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
        role="switch"
        aria-label={t('settingsStorage.screenshotsEnabled')}
        aria-checked={config.storage.screenshots_enabled}
      >
        <span class="switch-thumb {config.storage.screenshots_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
      </button>
    </div>

    <!-- 轮询间隔 -->
    <div class="settings-block {pollingSaveFlashPhase === 1 ? 'ui-save-flash-a' : pollingSaveFlashPhase === 2 ? 'ui-save-flash-b' : ''}">
      <div class="flex items-center justify-between">
        <label for="screenshot-interval" class="settings-text">{t('settingsStorage.pollingInterval')}</label>
        <div class="flex items-center gap-2">
          <input
            type="number"
            aria-label={t('settingsStorage.pollingInterval')}
            min="5"
            max="600"
            step="1"
            value={pollingInputValue}
            on:focus={() => { pollingInputFocused = true; }}
            on:input={handlePollingNumberInput}
            on:change={handlePollingNumberCommit}
            on:blur={handlePollingNumberBlur}
            class="w-16 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-[#484f58] dark:bg-[#21262d]"
          />
          <span class="text-xs settings-subtle">{t('settingsStorage.secondsUnit')}</span>
        </div>
      </div>
      <input
        id="screenshot-interval"
        type="range"
        value={pollingSliderValue}
        on:input={handlePollingRangeInput}
        on:change={handlePollingRangeChange}
        min="5"
        max="600"
        step="1"
        class="range-input"
      />
      <div class="flex justify-between text-xs settings-subtle">
        <span>{t('settingsStorage.precise')}</span>
        <span>{t('settingsStorage.powerSave')}</span>
      </div>
      <p class="settings-muted mt-1">{t('settingsStorage.pollingHint')}</p>
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
              class="rounded border-slate-300 dark:border-[#484f58]"
            />
            {t('settingsStorage.keepForever')}
          </label>
          {#if !keepForever}
            <input
              type="number"
              aria-label={t('settingsStorage.retentionDays')}
              min="1"
              max="9999"
              step="1"
              bind:value={config.storage.screenshot_retention_days}
              on:change={() => {
                config.storage.screenshot_retention_days = Math.max(1, Number(config.storage.screenshot_retention_days) || 7);
                handleChange();
              }}
              class="w-16 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-[#484f58] dark:bg-[#21262d]"
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
      <div class="settings-row">
        <div>
          <span class="settings-text">{t('settingsStorage.metadataCleanup')}</span>
          <p class="settings-muted mt-0.5">{t('settingsStorage.metadataCleanupHint')}</p>
        </div>
        <button
          type="button"
          role="switch"
          aria-label={t('settingsStorage.metadataCleanup')}
          aria-checked={Boolean(config.storage.metadata_cleanup_enabled)}
          disabled={isConfirmingMetadataCleanup}
          on:click={toggleMetadataCleanup}
          class="switch-track {config.storage.metadata_cleanup_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
        >
          <span class="switch-thumb {config.storage.metadata_cleanup_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
        </button>
      </div>
      {#if config.storage.metadata_cleanup_enabled}
        <div class="mt-3 flex items-center justify-between gap-3">
          <label for="metadata-retention-days" class="settings-text">{t('settingsStorage.metadataRetentionDays')}</label>
          <input
            id="metadata-retention-days"
            type="number"
            min="0"
            max="9999"
            step="1"
            bind:value={config.storage.metadata_retention_days}
            on:change={() => {
              config.storage.metadata_retention_days = Math.max(0, Math.min(9999, Math.trunc(Number(config.storage.metadata_retention_days) || 0)));
              handleChange();
            }}
            class="w-20 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-[#484f58] dark:bg-[#21262d]"
          />
        </div>
        <p class="settings-muted mt-1">{t('settingsStorage.metadataRetentionHint')}</p>
      {/if}
    </div>

    <div class="settings-block">
      <p id="settings-storage-screenshot-mode-label" class="settings-text mb-2">{t('settingsStorage.screenshotMode')}</p>
      <div class="ui-choice-grid" role="group" aria-labelledby="settings-storage-screenshot-mode-label">
        {#each localizedScreenshotModes as mode}
          {@const isSelected = config.storage.screenshot_display_mode === mode.value}
          <button
            type="button"
            aria-pressed={isSelected}
            on:click={() => {
              config.storage.screenshot_display_mode = mode.value;
              handleChange();
            }}
            class="ui-choice-card {isSelected ? 'ui-choice-card-active' : ''}"
          >
            <div class="ui-choice-head">
              <div class="ui-choice-title-group">
                <span class="ui-choice-icon" aria-hidden="true">
                  {#if mode.value === 'active_window'}
                    <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M4 6a2 2 0 012-2h12a2 2 0 012 2v12a2 2 0 01-2 2H6a2 2 0 01-2-2V6zm0 3h16" />
                    </svg>
                  {:else}
                    <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" />
                    </svg>
                  {/if}
                </span>
                <span class="ui-choice-title">{mode.label}</span>
              </div>
              <span class="ui-choice-indicator" aria-hidden="true"></span>
            </div>
            <p class="ui-choice-desc">{mode.description}</p>
          </button>
        {/each}
      </div>
    </div>
  </div>
</div>

<!-- 截图分辨率 -->
<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 id="settings-storage-width-mode-label" class="settings-card-title">{t('settingsStorage.widthMode')}</h3>

  <div class="settings-section">
    <div class="settings-block">
      <div class="ui-choice-grid" role="group" aria-labelledby="settings-storage-width-mode-label">
        <button
          type="button"
          aria-pressed={config.storage.screenshot_width_mode === 'auto'}
          on:click={() => {
            config.storage.screenshot_width_mode = 'auto';
            handleChange();
          }}
          class="ui-choice-card {config.storage.screenshot_width_mode === 'auto' ? 'ui-choice-card-active' : ''}"
        >
          <div class="ui-choice-head">
            <div class="ui-choice-title-group">
              <span class="ui-choice-icon" aria-hidden="true">
                <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M4 8V4m0 0h4M4 4l5 5m11-1V4m0 0h-4m4 0l-5 5M4 16v4m0 0h4m-4 0l5-5m11 5l-5-5m5 5v-4m0 4h-4" />
                </svg>
              </span>
              <span class="ui-choice-title">{t('settingsStorage.widthModeAuto')}</span>
            </div>
            <span class="ui-choice-indicator" aria-hidden="true"></span>
          </div>
          <p class="ui-choice-desc">{t('settingsStorage.widthModeAutoDesc')}</p>
        </button>
        <button
          type="button"
          aria-pressed={config.storage.screenshot_width_mode === 'fixed'}
          on:click={() => {
            config.storage.screenshot_width_mode = 'fixed';
            handleChange();
          }}
          class="ui-choice-card {config.storage.screenshot_width_mode === 'fixed' ? 'ui-choice-card-active' : ''}"
        >
          <div class="ui-choice-head">
            <div class="ui-choice-title-group">
              <span class="ui-choice-icon" aria-hidden="true">
                <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M6 12h12M6 12l3-3m-3 3l3 3m9-3l-3-3m3 3l-3 3M4 6v12m16-12v12" />
                </svg>
              </span>
              <span class="ui-choice-title">{t('settingsStorage.widthModeFixed')}</span>
            </div>
            <span class="ui-choice-indicator" aria-hidden="true"></span>
          </div>
          <p class="ui-choice-desc">{t('settingsStorage.widthModeFixedDesc')}</p>
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
    <div class="settings-subsection">
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
          class="switch-track {config.daily_report_auto_export ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'} {!config.daily_report_export_dir ? 'opacity-60 cursor-not-allowed' : ''}"
          on:click={() => {
            if (config.daily_report_export_dir) {
              config.daily_report_auto_export = !config.daily_report_auto_export;
              handleChange();
            }
          }}
          disabled={!config.daily_report_export_dir}
          role="switch"
          aria-label={t('settingsStorage.autoExport')}
          aria-checked={config.daily_report_auto_export}
        >
          <span class="switch-thumb {config.daily_report_auto_export ? 'translate-x-5' : 'translate-x-0'}"></span>
        </button>
      </div>

      <div class="mt-4 border-t border-slate-200/70 pt-4 dark:border-[#30363d]/80 space-y-3">
        <label class="flex items-start justify-between gap-3 cursor-pointer">
          <div>
            <span class="settings-text">{t('settingsStorage.exportFrontmatter')}</span>
            <p class="settings-muted mt-0.5">{t('settingsStorage.exportFrontmatterHint')}</p>
          </div>
          <input
            type="checkbox"
            checked={Boolean(config.daily_report_export_frontmatter)}
            on:change={(e) => {
              config.daily_report_export_frontmatter = e.currentTarget.checked;
              handleChange();
            }}
            class="mt-1 h-4 w-4 rounded border-slate-300 dark:border-[#484f58]"
          />
        </label>

        <label class="block">
          <span class="settings-text">{t('settingsStorage.exportSectionHeading')}</span>
          <p class="settings-muted mt-0.5">{t('settingsStorage.exportSectionHeadingHint')}</p>
          <input
            type="text"
            value={config.daily_report_export_section_heading ?? ''}
            on:input={(e) => {
              const raw = e.currentTarget.value;
              config.daily_report_export_section_heading = raw.trim() ? raw : null;
              handleChange();
            }}
            placeholder={t('settingsStorage.exportSectionHeadingPlaceholder')}
            class="mt-1.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
          />
        </label>
      </div>
    </div>
  </div>
</div>

<!-- Git 提交产出关联 -->
<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsStorage.gitTitle')}</h3>
  <p class="settings-muted mb-3">{t('settingsStorage.gitHint')}</p>

  <div class="settings-section space-y-4">
    <div class="settings-row">
      <div>
        <span class="settings-text">{t('settingsStorage.gitEnabled')}</span>
        <p class="settings-muted mt-0.5">{t('settingsStorage.gitEnabledHint')}</p>
      </div>
      <button
        type="button"
        on:click={() => {
          config.git_report_enabled = !config.git_report_enabled;
          handleChange();
        }}
        class="switch-track {config.git_report_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
        role="switch"
        aria-label={t('settingsStorage.gitEnabled')}
        aria-checked={Boolean(config.git_report_enabled)}
      >
        <span class="switch-thumb {config.git_report_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
      </button>
    </div>

    {#if config.git_report_enabled}
      <div class="settings-subsection space-y-3">
        <div class="flex items-center justify-between gap-2">
          <span class="settings-text">{t('settingsStorage.gitRepos')}</span>
          <button
            type="button"
            on:click={addGitRepoDir}
            class="settings-action-secondary"
          >
            {t('settingsStorage.gitAddRepo')}
          </button>
        </div>

        {#if (config.git_repo_paths || []).length === 0}
          <p class="settings-muted">{t('settingsStorage.gitEmptyRepos')}</p>
        {:else}
          <ul class="space-y-1.5">
            {#each config.git_repo_paths || [] as repoPath (repoPath)}
              <li class="flex items-center justify-between gap-2 rounded-lg bg-slate-50/90 px-3 py-1.5 text-xs dark:bg-[#161b22]/60">
                <span class="min-w-0 truncate font-mono text-slate-700 dark:text-[#c9d1d9]" title={repoPath}>{repoPath}</span>
                <button
                  type="button"
                  on:click={() => removeGitRepoDir(repoPath)}
                  class="shrink-0 text-xs text-rose-500 hover:text-rose-600 dark:text-rose-400"
                >
                  {t('settingsStorage.gitRemoveRepo')}
                </button>
              </li>
            {/each}
          </ul>
        {/if}

        <label class="block">
          <span class="settings-text">{t('settingsStorage.gitAuthorFilter')}</span>
          <p class="settings-muted mt-0.5">{t('settingsStorage.gitAuthorFilterHint')}</p>
          <input
            type="text"
            value={config.git_author_filter ?? ''}
            on:input={(e) => {
              const raw = e.currentTarget.value;
              config.git_author_filter = raw.trim() ? raw : null;
              resetGitPreview();
              handleChange();
            }}
            placeholder={t('settingsStorage.gitAuthorFilterPlaceholder')}
            class="mt-1.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
          />
        </label>

        {#if (config.git_repo_paths || []).length > 0}
          <div class="pt-1">
            <button
              type="button"
              on:click={previewTodayGitCommits}
              disabled={isPreviewingGit}
              class="settings-action-secondary disabled:opacity-50"
            >
              {isPreviewingGit ? t('settingsStorage.gitPreviewing') : t('settingsStorage.gitPreviewToday')}
            </button>

            {#if gitPreviewError}
              <p class="mt-2 text-xs text-rose-500 dark:text-rose-400">{gitPreviewError}</p>
            {:else if gitPreviewLoaded}
              {#if gitPreviewCommitCount === 0}
                <p class="settings-muted mt-2">{t('settingsStorage.gitPreviewEmpty')}</p>
              {:else}
                <div class="mt-2.5 rounded-lg border border-slate-200/80 bg-slate-50/70 p-2.5 text-xs dark:border-[#30363d] dark:bg-[#161b22]/50">
                  <p class="font-medium text-slate-700 dark:text-[#c9d1d9]">
                    {t('settingsStorage.gitPreviewSummary', {
                      repos: gitPreviewResults.length,
                      count: gitPreviewCommitCount,
                    })}
                  </p>
                  <div class="mt-2 space-y-2">
                    {#each gitPreviewResults as repo (repo.repoPath)}
                      <div>
                        <p class="font-semibold text-slate-600 dark:text-[#adbac7]">{repo.repoName} ({repo.commits.length})</p>
                        <ul class="mt-1 space-y-0.5 text-slate-500 dark:text-[#7d8590]">
                          {#each repo.commits.slice(0, 5) as commit (commit.hash)}
                            <li class="truncate">
                              <span class="font-mono text-[11px] text-slate-600 dark:text-[#adbac7]">{commit.hash}</span>
                              {commit.subject}
                              {#if commit.timeLabel}
                                <span class="ms-1 opacity-75">({commit.timeLabel})</span>
                              {/if}
                            </li>
                          {/each}
                        </ul>
                      </div>
                    {/each}
                  </div>
                </div>
              {/if}
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<!-- 远程存储（折叠，默认收起） -->
<CollapsibleSection
  title={t('settingsStorage.remoteStorageTitle')}
  subtitle={t('settingsStorage.remoteStorageDesc')}
  storageKey="settings.storage.remoteBackup"
>
  <div class="settings-section space-y-4">
    <div class="settings-subsection">
      <div class="flex items-center gap-2 mb-3">
        <div class="flex h-6 w-6 items-center justify-center rounded-md bg-primary-100 dark:bg-primary-900/30">
          <svg class="w-3.5 h-3.5 text-primary-600 dark:text-primary-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" />
          </svg>
        </div>
        <span class="text-sm font-medium text-slate-700 dark:text-[#c9d1d9]">{t('settingsStorage.remoteProvider')}</span>
      </div>
      <div class="ui-seg-group" role="group" aria-label={t('settingsStorage.remoteProvider')}>
        {#each [
          { value: 'none', label: t('settingsStorage.remoteProviderNone') },
          { value: 's3', label: t('settingsStorage.remoteProviderS3') },
          { value: 'webdav', label: t('settingsStorage.remoteProviderWebDav') },
        ] as opt}
          {@const selected = (config.remote_storage?.provider || 'none') === opt.value}
          <button
            type="button"
            aria-pressed={selected}
            on:click={() => {
              if (!config.remote_storage) config.remote_storage = { provider: 'none', s3: {}, webdav: {}, sync_app_config: false };
              config.remote_storage = { ...config.remote_storage, provider: opt.value };
              handleChange();
            }}
            class="ui-seg-item {selected ? 'ui-seg-item-active' : ''}"
          >
            {opt.label}
          </button>
        {/each}
      </div>
    </div>

    {#if config.remote_storage?.provider === 's3'}
      <div class="settings-subsection space-y-3">
        <div class="flex items-center gap-2">
          <div class="flex h-6 w-6 items-center justify-center rounded-md bg-amber-100 dark:bg-amber-900/30">
            <svg class="w-3.5 h-3.5 text-amber-600 dark:text-amber-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4" />
            </svg>
          </div>
          <span class="text-sm font-medium text-slate-700 dark:text-[#c9d1d9]">S3 / MinIO</span>
          <span class="settings-chip-success">{t('settingsStorage.remoteProviderS3')}</span>
        </div>

        <div class="grid grid-cols-1 gap-2 md:grid-cols-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.s3Endpoint')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.s3.endpoint}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
              placeholder={t('settingsStorage.s3EndpointHint')}
            />
          </label>
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.s3Bucket')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.s3.bucket}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
              placeholder="my-bucket"
            />
          </label>
        </div>

        <div class="grid grid-cols-1 gap-2 md:grid-cols-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.s3AccessKey')}</span>
            <div class="mt-0.5 relative">
              {#if s3AccessKeyVisible}
                <input
                  type="text"
                  bind:value={config.remote_storage.s3.access_key}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Access Key"
                  autocomplete="off"
                />
              {:else}
                <input
                  type="password"
                  bind:value={config.remote_storage.s3.access_key}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Access Key"
                  autocomplete="off"
                />
              {/if}
              <button
                type="button"
                class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-700 dark:hover:text-[#adbac7]"
                aria-label={`${t(s3AccessKeyVisible ? 'settingsStorage.hideSecret' : 'settingsStorage.showSecret')}: ${t('settingsStorage.s3AccessKey')}`}
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
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.s3SecretKey')}</span>
            <div class="mt-0.5 relative">
              {#if s3SecretKeyVisible}
                <input
                  type="text"
                  bind:value={config.remote_storage.s3.secret_key}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Secret Key"
                  autocomplete="off"
                />
              {:else}
                <input
                  type="password"
                  bind:value={config.remote_storage.s3.secret_key}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="Secret Key"
                  autocomplete="off"
                />
              {/if}
              <button
                type="button"
                class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-700 dark:hover:text-[#adbac7]"
                aria-label={`${t(s3SecretKeyVisible ? 'settingsStorage.hideSecret' : 'settingsStorage.showSecret')}: ${t('settingsStorage.s3SecretKey')}`}
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

        <div class="grid grid-cols-1 gap-2 md:grid-cols-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.s3Region')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.s3.region}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
              placeholder="us-east-1"
            />
          </label>
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.s3PathPrefix')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.s3.path_prefix}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
              placeholder={t('settingsStorage.s3PathPrefixHint')}
            />
          </label>
        </div>

        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.s3PublicUrlBase')}</span>
          <input
            type="text"
            bind:value={config.remote_storage.s3.public_url_base}
            on:blur={handleChange}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
            placeholder={t('settingsStorage.s3PublicUrlBaseHint')}
          />
        </label>

        <div class="flex items-center justify-between pt-1">
          <p class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('settingsStorage.publicUrlBaseEffectHint')}</p>
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
      <div class="settings-subsection space-y-3">
        <div class="flex items-center gap-2">
          <div class="flex h-6 w-6 items-center justify-center rounded-md bg-blue-100 dark:bg-blue-900/30">
            <svg class="w-3.5 h-3.5 text-blue-600 dark:text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2" />
            </svg>
          </div>
          <span class="text-sm font-medium text-slate-700 dark:text-[#c9d1d9]">WebDAV</span>
          <span class="settings-chip-success">{t('settingsStorage.remoteProviderWebDav')}</span>
        </div>

        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.webdavUrl')}</span>
          <input
            type="text"
            bind:value={config.remote_storage.webdav.url}
            on:blur={handleChange}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
            placeholder={t('settingsStorage.webdavUrlHint')}
          />
        </label>

        <div class="grid grid-cols-1 gap-2 md:grid-cols-2">
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.webdavUsername')}</span>
            <input
              type="text"
              bind:value={config.remote_storage.webdav.username}
              on:blur={handleChange}
              class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
              placeholder="username"
            />
          </label>
          <label class="block">
            <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.webdavPassword')}</span>
            <div class="mt-0.5 relative">
              {#if webdavPasswordVisible}
                <input
                  type="text"
                  bind:value={config.remote_storage.webdav.password}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="password"
                  autocomplete="off"
                />
              {:else}
                <input
                  type="password"
                  bind:value={config.remote_storage.webdav.password}
                  on:blur={handleChange}
                  class="w-full rounded-md bg-white/80 px-3 py-1.5 pr-8 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
                  placeholder="password"
                  autocomplete="off"
                />
              {/if}
              <button
                type="button"
                class="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-slate-400 hover:text-slate-700 dark:hover:text-[#adbac7]"
                aria-label={`${t(webdavPasswordVisible ? 'settingsStorage.hideSecret' : 'settingsStorage.showSecret')}: ${t('settingsStorage.webdavPassword')}`}
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
          <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.webdavPathPrefix')}</span>
          <input
            type="text"
            bind:value={config.remote_storage.webdav.path_prefix}
            on:blur={handleChange}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
            placeholder={t('settingsStorage.webdavPathPrefixHint')}
          />
        </label>

        <label class="block">
          <span class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.webdavPublicUrlBase')}</span>
          <input
            type="text"
            bind:value={config.remote_storage.webdav.public_url_base}
            on:blur={handleChange}
            class="mt-0.5 w-full rounded-md bg-white/80 px-3 py-1.5 text-sm font-mono text-slate-900 ring-1 ring-slate-200 focus:ring-primary-300 dark:bg-[#30363d]/50 dark:text-[#e6edf3] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
            placeholder={t('settingsStorage.webdavPublicUrlBaseHint')}
          />
        </label>
        <div class="flex items-center justify-between gap-3">
          <div class="min-w-0">
            <span class="settings-text text-sm">{t('settingsStorage.syncAppConfig')}</span>
            <p class="settings-note mt-1">{t('settingsStorage.syncAppConfigHint')}</p>
          </div>
          <button
            type="button"
            role="switch"
            aria-label={t('settingsStorage.syncAppConfig')}
            aria-checked={Boolean(config.remote_storage?.sync_app_config)}
            class="switch-track {config.remote_storage?.sync_app_config ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
            on:click={() => {
              if (!config.remote_storage) return;
              config.remote_storage.sync_app_config = !config.remote_storage.sync_app_config;
              handleChange();
            }}
          >
            <span class="switch-thumb {config.remote_storage?.sync_app_config ? 'translate-x-5' : 'translate-x-0'}"></span>
          </button>
        </div>

        <div class="flex items-center justify-between pt-1">
          <p class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('settingsStorage.publicUrlBaseEffectHint')}</p>
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
</CollapsibleSection>

<!-- 数据导出与一键备份 -->
<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsStorage.dataPortabilityTitle')}</h3>
  <p class="settings-muted mb-4">{t('settingsStorage.dataPortabilityDesc')}</p>

  <div class="settings-section space-y-5">
    <div class="settings-subsection space-y-3">
      <p class="settings-text font-medium">{t('settingsStorage.dataExportTitle')}</p>
      <p class="settings-muted">{t('settingsStorage.dataExportHint')}</p>

      <div class="flex flex-wrap items-center gap-3">
        <LocalizedDatePicker
          mode="range"
          bind:startDate={dataExportFrom}
          bind:endDate={dataExportTo}
          localeCode={currentLocale}
          max={getLocalIsoDate()}
          triggerClass="page-control-input w-auto"
        />

        <fieldset
          class="ui-seg-group"
          aria-label={t('settingsStorage.dataExportFormat')}
        >
          {#each dataExportFormats as fmt}
            <label
              class="ui-seg-item select-none uppercase focus-within:ring-2 focus-within:ring-inset focus-within:ring-primary-400 {dataExportFormat === fmt ? 'ui-seg-item-active' : ''}"
            >
              <input
                type="radio"
                name="data-export-format"
                value={fmt}
                bind:group={dataExportFormat}
                class="sr-only"
              />
              {fmt}
            </label>
          {/each}
        </fieldset>
      </div>

      <label class="inline-flex items-center gap-2 text-xs settings-muted cursor-pointer select-none">
        <input
          type="checkbox"
          bind:checked={dataExportIncludeOcr}
          class="h-4 w-4 rounded border-slate-300 dark:border-[#484f58]"
        />
        <span>{t('settingsStorage.dataExportIncludeOcr')}</span>
      </label>

      <div class="flex flex-wrap gap-2.5">
        <button
          type="button"
          on:click={runExportActivities}
          disabled={isExportingData}
          class="settings-action-secondary disabled:opacity-50"
        >
          {isExportingData ? t('settingsStorage.dataExporting') : t('settingsStorage.exportActivitiesBtn')}
        </button>
        <button
          type="button"
          on:click={runExportDailyStats}
          disabled={isExportingData}
          class="settings-action-secondary disabled:opacity-50"
        >
          {isExportingData ? t('settingsStorage.dataExporting') : t('settingsStorage.exportDailyStatsBtn')}
        </button>
      </div>
    </div>

    <div class="settings-subsection space-y-3">
      <p class="settings-text font-medium">{t('settingsStorage.backupSectionTitle')}</p>
      <p class="settings-muted">{t('settingsStorage.backupSectionHint')}</p>

      <div class="space-y-2">
        <label class="flex items-start gap-2 text-xs settings-muted cursor-pointer select-none">
          <input
            type="checkbox"
            bind:checked={backupIncludeScreenshots}
            class="mt-0.5 h-4 w-4 rounded border-slate-300 dark:border-[#484f58]"
          />
          <span>{t('settingsStorage.backupIncludeScreenshots')}</span>
        </label>

        <label class="flex items-start gap-2 text-xs settings-muted cursor-pointer select-none">
          <input
            type="checkbox"
            bind:checked={backupIncludeSecrets}
            class="mt-0.5 h-4 w-4 rounded border-slate-300 dark:border-[#484f58]"
          />
          <span>{t('settingsStorage.backupIncludeSecrets')}</span>
        </label>
      </div>

      <div class="flex flex-wrap gap-2.5 pt-1">
        <button
          type="button"
          on:click={runCreateBackup}
          disabled={isCreatingBackup || isRestoringBackup || restoreRestartPending || restoreLocked}
          class="settings-action-primary disabled:opacity-50"
        >
          {isCreatingBackup ? t('settingsStorage.creatingBackup') : t('settingsStorage.createBackupBtn')}
        </button>
        <button
          type="button"
          on:click={runRestoreBackup}
          disabled={isCreatingBackup || isRestoringBackup || restoreRestartPending || restoreLocked}
          class="settings-action-secondary disabled:opacity-50"
        >
          {isRestoringBackup || restoreRestartPending || restoreLocked ? t('settingsStorage.restoringBackup') : t('settingsStorage.restoreBackupBtn')}
        </button>
      </div>
    </div>
  </div>
</div>

<div class="settings-card mb-5" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsStorage.dataDirTitle')}</h3>

  <div class="settings-section">
    <div class="settings-block">
      <div class="settings-subsection">
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
        <div class="settings-subsection">
          <div class="mb-5">
            <div class="mb-2 flex items-end justify-between">
              <div>
                <span class="text-2xl font-bold text-slate-900 dark:text-[#e6edf3]">{storageStats.total_size_mb}</span>
                <span class="settings-muted"> / {config.storage.storage_limit_mb} MB</span>
              </div>
              <span class="text-sm font-medium {usagePercent > 80 ? 'settings-text-danger' : 'settings-muted'}">{usagePercent}%</span>
            </div>
            <div class="h-2.5 w-full overflow-hidden rounded-full bg-slate-100 dark:bg-[#30363d]">
              <div
                class="h-full rounded-full transition-all duration-500 {usageColor}"
                style="width: {usagePercent}%"
              ></div>
            </div>
          </div>
          <div class="mb-4 flex items-center justify-between gap-3">
            <span class="text-xs text-slate-500 dark:text-[#7d8590]">{t('settingsStorage.storageLimitLabel')}</span>
            <div class="flex items-center gap-1.5">
              <input
                type="number"
                aria-label={t('settingsStorage.storageLimitLabel')}
                min="256"
                max="102400"
                step="256"
                bind:value={config.storage.storage_limit_mb}
                on:change={() => {
                  config.storage.storage_limit_mb = Math.max(256, Number(config.storage.storage_limit_mb) || 2048);
                  handleChange();
                }}
                class="w-24 rounded-lg border border-slate-200 bg-white px-2.5 py-1 text-right text-sm font-mono text-slate-700 ring-1 ring-slate-200 focus:ring-primary-300 dark:border-[#484f58] dark:bg-[#21262d] dark:text-[#c9d1d9] dark:ring-[#484f58] dark:focus:ring-primary-600 focus:outline-none"
              />
              <span class="text-xs text-slate-400 dark:text-[#636c76]">MB</span>
            </div>
          </div>

          <div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
                <div class="px-3 py-2 text-center">
              <p class="text-xl font-bold text-slate-900 dark:text-[#e6edf3]">{storageStats.total_files}</p>
              <p class="settings-muted mt-0.5">{t('settingsStorage.screenshotsCount')}</p>
            </div>
                <div class="border-x border-slate-200/70 px-3 py-2 text-center dark:border-[#30363d]/70">
              <p class="text-xl font-bold text-slate-900 dark:text-[#e6edf3]">{storageStats.total_size_mb} MB</p>
              <p class="settings-muted mt-0.5">{t('settingsStorage.usedSpace')}</p>
            </div>
                <div class="px-3 py-2 text-center">
              <p class="text-xl font-bold text-slate-900 dark:text-[#e6edf3]">{storageRetentionLabel}</p>
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
