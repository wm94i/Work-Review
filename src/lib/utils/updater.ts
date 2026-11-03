import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen as tauriListen } from '@tauri-apps/api/event';
import { relaunch as relaunchApp } from '@tauri-apps/plugin-process';
import { open as openExternal } from '@tauri-apps/plugin-shell';
import { confirm as openConfirm, type ConfirmOptions } from '$lib/stores/confirm.ts';
import { showToast as displayToast, type ToastType } from '$lib/stores/toast.ts';
import { t } from '$lib/i18n/index.ts';

const UPDATE_STATUS_EVENT = 'update-status';

export interface GithubUpdateInfo {
  currentVersion: string;
  latestVersion: string;
  available: boolean;
  autoUpdateReady: boolean;
  prerelease: boolean;
  releaseUrl: string;
  body: string | null;
  source: string | null;
}

export interface GithubUpdateInstallResult {
  updated: boolean;
  available: boolean;
  version: string | null;
  source: string | null;
  message: string;
  attemptedSources: string[];
}

export interface GithubUpdateStatusPayload {
  stage: string;
  message: string;
  source: string | null;
  version: string | null;
  downloadedBytes: number | null;
  totalBytes: number | null;
  percent: number | null;
}

export interface RunUpdateFlowOptions {
  silentWhenUpToDate?: boolean;
  confirmBeforeDownload?: boolean;
  onStatusChange?: (status: string) => void;
}

export type RunUpdateFlowResult =
  | { skipped: true; reason: 'in-flight' }
  | { updated: false; available: false }
  | {
      updated: false;
      available: true;
      autoUpdateReady: false;
      releaseUrl: string;
    }
  | { updated: false; cancelled: true }
  | { updated: true; handoffToInstaller: true }
  | { updated: true }
  | { updated: false; error: string };

export type RunUpdateFlow = (
  options?: RunUpdateFlowOptions,
) => Promise<RunUpdateFlowResult>;

export interface UpdateFlowDependencies {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  listen: <T>(
    event: string,
    handler: (event: { payload: T }) => void,
  ) => Promise<() => void>;
  relaunch: () => Promise<void>;
  open: (url: string) => Promise<void>;
  confirm: (options?: ConfirmOptions) => Promise<boolean>;
  showToast: (message: unknown, type?: ToastType, duration?: number) => void;
  translate: (key: string, params?: Record<string, unknown>) => string;
  warn: (message: string, error: unknown) => void;
  error: (message: string, error: unknown) => void;
}

function localizeRuntimeStatusMessage(
  message: unknown,
  translate: UpdateFlowDependencies['translate'],
): string {
  const text = String(message || '').trim();
  if (!text) {
    return '';
  }

  let matched = text.match(/^正在检查更新源\s+(.+)\.\.\.$/);
  if (matched) {
    return translate('updater.checkingSource', { sourceLabel: matched[1] });
  }

  matched = text.match(/^发现新版本\s+(.+?)，准备从\s+(.+)\s+下载\.\.\.$/);
  if (matched) {
    return translate('updater.preparingDownload', {
      version: matched[1],
      sourceLabel: matched[2],
    });
  }

  matched = text.match(/^更新安装完成，来源\s+(.+)$/);
  if (matched) {
    return translate('updater.installedFromSource', { sourceLabel: matched[1] });
  }

  matched = text.match(/^未找到可用于版本\s+(.+)\s+的在线更新源$/);
  if (matched) {
    return translate('updater.noSourceForVersion', { version: matched[1] });
  }

  matched = text.match(/^在线更新失败，已尝试全部更新源：(.+)$/);
  if (matched) {
    return translate('updater.failedWithDetails', { details: matched[1] });
  }

  if (text === '在线更新已完成') {
    return translate('updater.completed');
  }

  if (text === '当前未发现可安装的在线更新') {
    return translate('updater.noInstallAvailable');
  }

  matched = text.match(/^正在下载更新\s+(\d+)%（(.+)）$/);
  if (matched) {
    return translate('updater.downloadingPercent', {
      percent: matched[1],
      sourceLabel: matched[2],
    });
  }

  matched = text.match(/^正在下载更新\s+([\d.]+)\s*MB（(.+)）$/);
  if (matched) {
    return translate('updater.downloadingSize', {
      size: matched[1],
      sourceLabel: matched[2],
    });
  }

  matched = text.match(/^下载完成，正在安装（(.+)）\.\.\.$/);
  if (matched) {
    return translate('updater.installingFromSource', { sourceLabel: matched[1] });
  }

  matched = text.match(/^源\s+(.+)\s+更新失败，准备尝试下一个源\.\.\.$/);
  if (matched) {
    return translate('updater.sourceFailedRetrying', { sourceLabel: matched[1] });
  }

  // 未识别的后端状态不能直接透出，避免在非中文界面泄漏中文消息。
  return translate('updater.inProgress');
}

export function createUpdateFlow(dependencies: UpdateFlowDependencies): RunUpdateFlow {
  const {
    invoke,
    listen,
    relaunch,
    open,
    confirm,
    showToast,
    translate,
    warn,
    error: logError,
  } = dependencies;

  let updateInFlight = false;
  let runtimePlatformPromise: Promise<string> | null = null;

  async function getRuntimePlatform(): Promise<string> {
    if (!runtimePlatformPromise) {
      runtimePlatformPromise = invoke<string>('get_runtime_platform').catch((error) => {
        runtimePlatformPromise = null;
        throw error;
      });
    }

    return runtimePlatformPromise;
  }

  return async function runUpdateFlow(options = {}): Promise<RunUpdateFlowResult> {
    const {
      silentWhenUpToDate = false,
      confirmBeforeDownload = true,
      onStatusChange = () => {},
    } = options;

    if (updateInFlight) {
      return { skipped: true, reason: 'in-flight' };
    }

    updateInFlight = true;

    try {
      onStatusChange(translate('updater.checking'));

      const releaseInfo = await invoke<GithubUpdateInfo>('check_github_update');
      await invoke<void>('update_last_check_time').catch((error) => {
        warn('记录更新检查时间失败:', error);
      });

      if (!releaseInfo.available) {
        onStatusChange(silentWhenUpToDate ? '' : translate('updater.upToDate'));
        return { updated: false, available: false };
      }

      if (!releaseInfo.autoUpdateReady) {
        onStatusChange(translate('updater.availableManual'));
        showToast(translate('updater.availableManual'), 'info', 4500);

        if ((confirmBeforeDownload || releaseInfo.prerelease) && releaseInfo.releaseUrl) {
          const shouldOpenRelease = await confirm({
            title: translate(releaseInfo.prerelease ? 'updater.prereleaseTitle' : 'updater.newVersionTitle'),
            message: translate(releaseInfo.prerelease ? 'updater.prereleaseManualMessage' : 'updater.openReleaseMessage', {
              version: releaseInfo.latestVersion,
            }) + (releaseInfo.source === 'portable-release-api' ? `\n\n${translate('updater.portableUpdateHint')}` : ''),
            confirmText: translate('updater.openRelease'),
            cancelText: translate('updater.later'),
            tone: releaseInfo.prerelease ? 'warning' : 'info',
          });

          if (shouldOpenRelease) {
            await open(releaseInfo.releaseUrl);
          }
        }

        return {
          updated: false,
          available: true,
          autoUpdateReady: false,
          releaseUrl: releaseInfo.releaseUrl,
        };
      }

      if (confirmBeforeDownload || releaseInfo.prerelease) {
        const shouldStart = await confirm({
          title: translate(releaseInfo.prerelease ? 'updater.prereleaseTitle' : 'updater.newVersionTitle'),
          message: translate(releaseInfo.prerelease ? 'updater.prereleaseMessage' : 'updater.startUpdateMessage', {
            version: releaseInfo.latestVersion,
          }),
          confirmText: translate('updater.startUpdate'),
          cancelText: translate('updater.later'),
          tone: releaseInfo.prerelease ? 'warning' : 'info',
        });

        if (!shouldStart) {
          onStatusChange('');
          return { updated: false, cancelled: true };
        }
      }

      const unlistenUpdateStatus = await listen<
        GithubUpdateStatusPayload | null | undefined
      >(
        UPDATE_STATUS_EVENT,
        (event) => {
          const statusMessage = event.payload?.message;
          if (statusMessage) {
            onStatusChange(
              localizeRuntimeStatusMessage(statusMessage, translate),
            );
          }
        },
      );

      try {
        await invoke<GithubUpdateInstallResult>('download_and_install_github_update', {
          expectedVersion: releaseInfo.latestVersion,
        });
      } finally {
        await unlistenUpdateStatus();
      }

      const runtimePlatform = await getRuntimePlatform();
      if (runtimePlatform === 'windows') {
        onStatusChange(translate('updater.installerStartedStatus'));
        showToast(translate('updater.installerStartedToast'), 'success');
        await invoke<void>('quit_app_for_update');
        return { updated: true, handoffToInstaller: true };
      }

      onStatusChange(translate('updater.restarting'));
      await relaunch();
      return { updated: true };
    } catch (error) {
      const errMsg = String(error);
      logError('检查更新失败:', error);

      if (errMsg.includes('timeout') || errMsg.includes('timed out')) {
        onStatusChange(translate('updater.failed'));
        showToast(translate('updater.timeout'), 'error');
      } else if (
        errMsg.includes('Download request failed')
        || errMsg.includes('failed to download')
        || errMsg.includes('Network')
      ) {
        onStatusChange(translate('updater.failed'));
        showToast(translate('updater.failedAllSources'), 'error');
      } else {
        onStatusChange(translate('updater.failed'));
        showToast(translate('updater.failed'), 'error');
      }

      await confirm({
        title: translate('updater.errorTitle'),
        message: translate('updater.errorMessage', { error: errMsg }),
        confirmText: translate('updater.acknowledge'),
        cancelText: translate('updater.retryLater'),
        tone: 'error',
      });

      return { updated: false, error: errMsg };
    } finally {
      updateInFlight = false;
    }
  };
}

export const runUpdateFlow: RunUpdateFlow = createUpdateFlow({
  invoke: <T>(command: string, args?: Record<string, unknown>) =>
    tauriInvoke<T>(command, args),
  listen: <T>(event: string, handler: (event: { payload: T }) => void) =>
    tauriListen<T>(event, handler),
  relaunch: relaunchApp,
  open: (url: string) => openExternal(url),
  confirm: openConfirm,
  showToast: displayToast,
  translate: t,
  warn: (message: string, error: unknown) => console.warn(message, error),
  error: (message: string, error: unknown) => console.error(message, error),
});
