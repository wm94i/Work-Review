import test from 'node:test';
import assert from 'node:assert/strict';
import { access, readFile } from 'node:fs/promises';

import {
  createUpdateFlow,
  type GithubUpdateInfo,
  type GithubUpdateInstallResult,
  type GithubUpdateStatusPayload,
  type UpdateFlowDependencies,
} from './updater.ts';

const updaterUrl = new URL('./updater.ts', import.meta.url);

function releaseInfo(overrides: Partial<GithubUpdateInfo> = {}): GithubUpdateInfo {
  return {
    currentVersion: '1.0.0',
    latestVersion: '1.1.0',
    available: true,
    autoUpdateReady: true,
    prerelease: false,
    releaseUrl: 'https://example.com/releases/1.1.0',
    body: null,
    source: 'test-source',
    ...overrides,
  };
}

function installResult(release: GithubUpdateInfo): GithubUpdateInstallResult {
  return {
    updated: true,
    available: true,
    version: release.latestVersion,
    source: 'test-source',
    message: '在线更新已完成',
    attemptedSources: ['test-source'],
  };
}

function updateStatusPayload(
  overrides: Partial<GithubUpdateStatusPayload> = {},
): GithubUpdateStatusPayload {
  return {
    stage: 'completed',
    message: '在线更新已完成',
    source: 'test-source',
    version: '1.1.0',
    downloadedBytes: null,
    totalBytes: null,
    percent: null,
    ...overrides,
  };
}

interface CommandCall {
  command: string;
  args: Record<string, unknown> | undefined;
}

interface ToastCall {
  message: unknown;
  type: Parameters<UpdateFlowDependencies['showToast']>[1];
  duration: number | undefined;
}

interface WarningCall {
  message: string;
  error: unknown;
}

type ConfirmationOptions = Parameters<UpdateFlowDependencies['confirm']>[0];
type UpdateStatusPayload = GithubUpdateStatusPayload | null | undefined;

interface HarnessOptions {
  checkedRelease?: GithubUpdateInfo;
  platform?: string;
  confirmResults?: boolean[];
  commandErrors?: Record<string, unknown>;
  checkPromise?: Promise<GithubUpdateInfo> | null;
  statusPayload?: UpdateStatusPayload;
}

interface UpdateHarness {
  commands: CommandCall[];
  statuses: string[];
  toasts: ToastCall[];
  confirmations: ConfirmationOptions[];
  opened: string[];
  warnings: WarningCall[];
  dependencies: UpdateFlowDependencies;
  readonly unlistenCount: number;
  readonly relaunchCount: number;
}

function createHarness(options: HarnessOptions = {}): UpdateHarness {
  const {
    checkedRelease = releaseInfo(),
    platform = 'linux',
    confirmResults = [],
    commandErrors = {},
    checkPromise = null,
    statusPayload = updateStatusPayload(),
  } = options;
  const commands: CommandCall[] = [];
  const statuses: string[] = [];
  const toasts: ToastCall[] = [];
  const confirmations: ConfirmationOptions[] = [];
  const opened: string[] = [];
  const warnings: WarningCall[] = [];
  let dispatchStatus: ((payload: UpdateStatusPayload) => void) | null = null;
  let unlistenCount = 0;
  let relaunchCount = 0;

  const invokeUnknown = async (
    command: string,
    args?: Record<string, unknown>,
  ): Promise<unknown> => {
    commands.push({ command, args });
    const configuredError = commandErrors[command];
    if (Array.isArray(configuredError)) {
      const nextError: unknown = configuredError.shift();
      if (nextError) throw nextError;
    } else if (configuredError) {
      throw configuredError;
    }

    switch (command) {
      case 'check_github_update':
        return checkPromise ? checkPromise : checkedRelease;
      case 'update_last_check_time':
      case 'quit_app_for_update':
        return undefined;
      case 'download_and_install_github_update':
        dispatchStatus?.(statusPayload);
        return installResult(checkedRelease);
      case 'get_runtime_platform':
        return platform;
      default:
        throw new Error(`未处理的测试命令: ${command}`);
    }
  };

  const dependencies: UpdateFlowDependencies = {
    invoke: async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
      const value = await invokeUnknown(command, args);
      return value as T;
    },
    listen: async <T>(event: string, handler: (event: { payload: T }) => void) => {
      assert.equal(event, 'update-status');
      dispatchStatus = (payload: UpdateStatusPayload) => {
        handler({ payload: payload as T });
      };
      return () => {
        unlistenCount += 1;
      };
    },
    relaunch: async () => {
      relaunchCount += 1;
    },
    open: async (url: string) => {
      opened.push(url);
    },
    confirm: async (confirmOptions?: ConfirmationOptions) => {
      confirmations.push(confirmOptions);
      return confirmResults.shift() ?? true;
    },
    showToast: (message: unknown, type, duration?: number) => {
      toasts.push({ message, type, duration });
    },
    translate: (key: string, params?: Record<string, unknown>) => (
      params ? `${key}:${JSON.stringify(params)}` : key
    ),
    warn: (message: string, error: unknown) => {
      warnings.push({ message, error });
    },
    error: (_message: string, _error: unknown) => {},
  };

  return {
    commands,
    statuses,
    toasts,
    confirmations,
    opened,
    warnings,
    dependencies,
    get unlistenCount() {
      return unlistenCount;
    },
    get relaunchCount() {
      return relaunchCount;
    },
  };
}

test('更新流程应迁移到 TypeScript 并保留模块级单例实例', async () => {
  await access(updaterUrl);

  const source = await readFile(updaterUrl, 'utf8');
  assert.match(
    source,
    /export const runUpdateFlow: RunUpdateFlow = createUpdateFlow\(\{/,
  );
});

test('发现新版本但当前发布未准备好在线更新时不应继续安装', async () => {
  const source = await readFile(updaterUrl, 'utf8');

  assert.match(source, /if \(!releaseInfo\.autoUpdateReady\)/);
  assert.match(source, /translate\('updater\.availableManual'\)/);
  assert.match(source, /open\(releaseInfo\.releaseUrl\)/);

  const harness = createHarness({
    checkedRelease: releaseInfo({ autoUpdateReady: false }),
    confirmResults: [true],
  });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const result = await runUpdateFlow({
    confirmBeforeDownload: true,
    onStatusChange: (status) => harness.statuses.push(status),
  });

  assert.deepEqual(result, {
    updated: false,
    available: true,
    autoUpdateReady: false,
    releaseUrl: 'https://example.com/releases/1.1.0',
  });
  assert.deepEqual(harness.opened, ['https://example.com/releases/1.1.0']);
  assert.equal(
    harness.commands.some(({ command }) => command === 'download_and_install_github_update'),
    false
  );
});

test('Windows 便携版只打开发布页并提示保留Data，测试版仍需风险确认', async () => {
  for (const prerelease of [false, true]) {
    for (const confirmed of [false, true]) {
      const release = releaseInfo({
        autoUpdateReady: false,
        source: 'portable-release-api',
        prerelease,
      });
      const harness = createHarness({
        checkedRelease: release,
        platform: 'windows',
        confirmResults: [confirmed],
      });
      await createUpdateFlow(harness.dependencies)();
      assert.match(String(harness.confirmations[0]?.message), /updater\.portableUpdateHint/);
      if (prerelease) {
        assert.match(String(harness.confirmations[0]?.message), /updater\.prereleaseManualMessage/);
        assert.equal(harness.confirmations[0]?.tone, 'warning');
      }
      assert.deepEqual(harness.opened, confirmed ? [release.releaseUrl] : []);
      assert.equal(harness.relaunchCount, 0);
      assert.ok(harness.commands.every(({ command }) =>
        command !== 'download_and_install_github_update' && command !== 'quit_app_for_update'));
    }
  }
});

test('无更新时应在成功检查后记录时间并支持静默状态', async () => {
  const harness = createHarness({
    checkedRelease: releaseInfo({ available: false, autoUpdateReady: false }),
  });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const result = await runUpdateFlow({
    silentWhenUpToDate: true,
    onStatusChange: (status) => harness.statuses.push(status),
  });

  assert.deepEqual(result, { updated: false, available: false });
  assert.deepEqual(
    harness.commands.map(({ command }) => command),
    ['check_github_update', 'update_last_check_time']
  );
  assert.deepEqual(harness.statuses, ['updater.checking', '']);
});

test('记录检查时间失败时应告警但不打断更新检查结果', async () => {
  const writeError = new Error('write failed');
  const harness = createHarness({
    checkedRelease: releaseInfo({ available: false, autoUpdateReady: false }),
    commandErrors: { update_last_check_time: writeError },
  });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const result = await runUpdateFlow();

  assert.deepEqual(result, { updated: false, available: false });
  assert.deepEqual(harness.warnings, [
    { message: '记录更新检查时间失败:', error: writeError },
  ]);
});

test('并发调用应跳过第二次更新且不重复检查', async () => {
  let resolveCheck: ((value: GithubUpdateInfo | PromiseLike<GithubUpdateInfo>) => void) | undefined;
  const pendingCheck = new Promise<GithubUpdateInfo>((resolve) => {
    resolveCheck = resolve;
  });
  const harness = createHarness({ checkPromise: pendingCheck });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const firstRun = runUpdateFlow();
  const secondResult = await runUpdateFlow();

  assert.deepEqual(secondResult, { skipped: true, reason: 'in-flight' });
  assert.equal(
    harness.commands.filter(({ command }) => command === 'check_github_update').length,
    1
  );

  assert.ok(resolveCheck);
  resolveCheck(releaseInfo({ available: false, autoUpdateReady: false }));
  await firstRun;
});

test('在线更新应本地化后端状态、释放监听并在非 Windows 重启', async () => {
  const harness = createHarness();
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const result = await runUpdateFlow({
    onStatusChange: (status) => harness.statuses.push(status),
  });

  assert.deepEqual(result, { updated: true });
  assert.equal(harness.unlistenCount, 1);
  assert.equal(harness.relaunchCount, 1);
  assert.ok(harness.statuses.includes('updater.completed'));
  assert.equal(harness.statuses.at(-1), 'updater.restarting');
  assert.deepEqual(
    harness.commands.find(({ command }) => command === 'download_and_install_github_update'),
    {
      command: 'download_and_install_github_update',
      args: { expectedVersion: '1.1.0' },
    }
  );
});

test('更新状态事件载荷为空时仍应完成更新并释放监听', async () => {
  const harness = createHarness({ statusPayload: null });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const result = await runUpdateFlow();

  assert.deepEqual(result, { updated: true });
  assert.equal(harness.unlistenCount, 1);
  assert.equal(harness.relaunchCount, 1);
});

test('Windows 更新完成后应交给安装器退出而不是重启 WebView', async () => {
  const harness = createHarness({ platform: 'windows' });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const result = await runUpdateFlow();

  assert.deepEqual(result, { updated: true, handoffToInstaller: true });
  assert.equal(harness.relaunchCount, 0);
  assert.ok(harness.commands.some(({ command }) => command === 'quit_app_for_update'));
});

test('下载安装失败仍应释放监听、提示错误并解除并发锁', async () => {
  const harness = createHarness({
    commandErrors: {
      download_and_install_github_update: new Error('Download request failed: timeout'),
    },
  });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const firstResult = await runUpdateFlow();

  assert.ok('error' in firstResult);
  assert.equal(firstResult.updated, false);
  assert.match(firstResult.error, /Download request failed: timeout/);
  assert.equal(harness.unlistenCount, 1);
  assert.deepEqual(harness.toasts.at(-1), {
    message: 'updater.timeout',
    type: 'error',
    duration: undefined,
  });

  const secondResult = await runUpdateFlow();
  assert.deepEqual(secondResult, {
    updated: false,
    error: 'Error: Download request failed: timeout',
  });
  assert.equal(
    harness.commands.filter(({ command }) => command === 'check_github_update').length,
    2
  );
});

test('首次状态回调抛错后仍应释放并发锁', async () => {
  const harness = createHarness({
    checkedRelease: releaseInfo({ available: false, autoUpdateReady: false }),
  });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  await assert.rejects(
    runUpdateFlow({
      onStatusChange: () => {
        throw new Error('status callback failed');
      },
    }),
    /status callback failed/
  );

  const secondResult = await runUpdateFlow();
  assert.deepEqual(secondResult, { updated: false, available: false });
  assert.equal(
    harness.commands.filter(({ command }) => command === 'check_github_update').length,
    1
  );
});

test('平台探测失败后应清空缓存并允许下次更新重试', async () => {
  const platformError = new Error('platform unavailable');
  const harness = createHarness({
    commandErrors: { get_runtime_platform: [platformError] },
  });
  const runUpdateFlow = createUpdateFlow(harness.dependencies);

  const firstResult = await runUpdateFlow();
  const secondResult = await runUpdateFlow();

  assert.deepEqual(firstResult, {
    updated: false,
    error: 'Error: platform unavailable',
  });
  assert.deepEqual(secondResult, { updated: true });
  assert.equal(
    harness.commands.filter(({ command }) => command === 'get_runtime_platform').length,
    2
  );
});

test('默认检查必须先确认版本，取消后不得下载或退出应用', async () => {
  const harness = createHarness({ confirmResults: [false] });
  const result = await createUpdateFlow(harness.dependencies)();
  assert.deepEqual(result, { updated: false, cancelled: true });
  assert.equal(harness.confirmations.length, 1);
  assert.equal(harness.confirmations[0]?.title, 'updater.newVersionTitle');
  assert.match(harness.confirmations[0]?.message ?? '', /1\.1\.0/);
  assert.equal(harness.commands.some(c => c.command === 'download_and_install_github_update'), false);
  assert.equal(harness.relaunchCount, 0);
});

test('RC 即使关闭普通确认也必须提示测试风险并允许取消', async () => {
  const harness = createHarness({
    checkedRelease: releaseInfo({ latestVersion: '1.1.4-rc.3', prerelease: true }),
    confirmResults: [false],
  });
  const result = await createUpdateFlow(harness.dependencies)({ confirmBeforeDownload: false });
  assert.deepEqual(result, { updated: false, cancelled: true });
  assert.equal(harness.confirmations[0]?.title, 'updater.prereleaseTitle');
  assert.equal(harness.confirmations[0]?.tone, 'warning');
  assert.match(harness.confirmations[0]?.message ?? '', /updater\.prereleaseMessage.*1\.1\.4-rc\.3/);
  assert.equal(harness.commands.some(c => c.command === 'download_and_install_github_update'), false);
});

test('确认 RC 后应只请求安装已展示的完整版本号', async () => {
  const harness = createHarness({
    checkedRelease: releaseInfo({ latestVersion: '1.1.4-rc.10', prerelease: true }),
  });
  const result = await createUpdateFlow(harness.dependencies)();
  assert.deepEqual(result, { updated: true });
  assert.equal(harness.confirmations.length, 1);
  assert.deepEqual(
    harness.commands.find(c => c.command === 'download_and_install_github_update')?.args,
    { expectedVersion: '1.1.4-rc.10' },
  );
});

test('平台没有 RC 在线包时应警告并打开对应发布页，不尝试安装', async () => {
  const releaseUrl = 'https://github.com/wm94i/Work-Review/releases/tag/v1.1.4-rc.1';
  const harness = createHarness({
    checkedRelease: releaseInfo({
      latestVersion: '1.1.4-rc.1', prerelease: true, autoUpdateReady: false, releaseUrl,
    }),
  });
  const result = await createUpdateFlow(harness.dependencies)();
  assert.ok('autoUpdateReady' in result && !result.autoUpdateReady);
  assert.equal(harness.confirmations[0]?.tone, 'warning');
  assert.match(harness.confirmations[0]?.message ?? '', /updater\.prereleaseManualMessage.*1\.1\.4-rc\.1/);
  assert.deepEqual(harness.opened, [releaseUrl]);
  assert.equal(harness.commands.some(c => c.command === 'download_and_install_github_update'), false);
});