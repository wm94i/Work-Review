import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

import type { Browser, BrowserContext, Page, Video } from 'playwright';

import type { DemoNetworkDiagnostics } from './browser.ts';

import {
  assertCleanCaptureDiagnostics,
  buildDemoRouteUrl,
  buildSceneCapturePaths,
  cleanupCaptureResources,
  createCaptureDiagnostics,
  recordScene,
} from './capture.ts';
import { createDemoFixtures } from './fixtures.ts';
import { STORYBOARD } from './storyboard.ts';
import { createDemoMockState } from './tauriMock.ts';
import type { DemoMockState, ShotRunner } from './types.ts';

test('分镜路由使用同源 Hash 路由并保留查询参数', () => {
  assert.equal(
    buildDemoRouteUrl(new URL('http://127.0.0.1:5173/'), '/timeline?date=2026-08-12'),
    'http://127.0.0.1:5173/#/timeline?date=2026-08-12',
  );
});

test('录制路径按横竖屏和分镜隔离并包含三张关键帧与元数据', () => {
  assert.deepEqual(
    buildSceneCapturePaths('/tmp/work-review-demo/intermediate', '9x16', 'assistant'),
    {
      aspectDir: '/tmp/work-review-demo/intermediate/9x16',
      videoPath: '/tmp/work-review-demo/intermediate/9x16/assistant.webm',
      startFramePath: '/tmp/work-review-demo/intermediate/9x16/assistant-start.png',
      actionFramePath: '/tmp/work-review-demo/intermediate/9x16/assistant-action.png',
      endFramePath: '/tmp/work-review-demo/intermediate/9x16/assistant-end.png',
      metadataPath: '/tmp/work-review-demo/intermediate/9x16/assistant.json',
    },
  );
});

test('页面错误、控制台 error、外部请求和未处理命令都会阻止交付', () => {
  const diagnostics = createCaptureDiagnostics();
  diagnostics.pageErrors.push('渲染失败');
  diagnostics.consoleErrors.push('状态错误');
  diagnostics.blockedRequests.push('https://example.com/pixel');
  diagnostics.unhandledCommands.push('missing_command');

  assert.throws(
    () => assertCleanCaptureDiagnostics(diagnostics, 'timeline'),
    /timeline[\s\S]*渲染失败[\s\S]*example\.com[\s\S]*missing_command/,
  );
});

test('无错误诊断可以继续录制', () => {
  assert.doesNotThrow(() => assertCleanCaptureDiagnostics(createCaptureDiagnostics(), 'hook'));
  assert.equal(STORYBOARD.length, 7);
});

interface RecordHarnessOptions {
  sceneId?: 'hook' | 'export';
  exportedFilesSnapshot?: unknown;
  onExportRead?: () => void;
  outputDir?: string;
  rawContent?: string;
  existingVideoContent?: string;
  now?: () => number;
  onContextFactory?: (diagnostics?: DemoNetworkDiagnostics) => void;
  onGoto?: () => void;
  onScreenshot?: (outputPath: string) => void;
  runner?: ShotRunner;
  close?: () => Promise<void>;
  state?: DemoMockState & { unhandledCommands?: string[] };
  probeMediaFile?: (mediaPath: string) => Promise<{ durationSeconds: number | null }>;
  replaceFile?: (sourcePath: string, outputPath: string) => Promise<void>;
}

async function runSceneCapture(options: RecordHarnessOptions = {}) {
  const rootDir = options.outputDir
    ? path.dirname(options.outputDir)
    : await mkdtemp(path.join(tmpdir(), 'work-review-capture-test-'));
  const outputDir = options.outputDir ?? path.join(rootDir, 'intermediate');
  const rawVideoPath = path.join(rootDir, 'raw.webm');
  await writeFile(rawVideoPath, options.rawContent ?? 'raw-video', 'utf8');

  const fixtures = createDemoFixtures();
  const scene = STORYBOARD.find((entry) => entry.id === (options.sceneId ?? 'hook'))!;
  if (options.existingVideoContent !== undefined) {
    const existingPaths = buildSceneCapturePaths(outputDir, '16x9', scene.id);
    await mkdir(existingPaths.aspectDir, { recursive: true });
    await writeFile(existingPaths.videoPath, options.existingVideoContent, 'utf8');
  }
  const state = options.state ?? createDemoMockState(fixtures);
  const waits: number[] = [];
  const screenshotPaths: string[] = [];
  const video = {
    path: async () => rawVideoPath,
  } as Video;
  const page = {
    on: () => undefined,
    video: () => video,
    goto: async () => {
      options.onGoto?.();
    },
    waitForFunction: async () => undefined,
    screenshot: async ({ path: outputPath }: { path: string }) => {
      await writeFile(outputPath, 'frame', 'utf8');
      screenshotPaths.push(outputPath);
      options.onScreenshot?.(outputPath);
    },
    waitForTimeout: async (milliseconds: number) => {
      waits.push(milliseconds);
    },
    evaluate: async () => {
      options.onExportRead?.();
      return options.exportedFilesSnapshot ?? [];
    },
  } as unknown as Page;
  const context = {
    newPage: async () => page,
    close: options.close ?? (async () => undefined),
  } as unknown as BrowserContext;
  const runner = options.runner ?? (async () => undefined);

  const result = await recordScene(scene, '16x9', outputDir, {
    browser: {} as Browser,
    fixtures,
    contextFactory: async (_browser, _aspect, _videoDir, _baseUrl, diagnostics) => {
      options.onContextFactory?.(diagnostics);
      return context;
    },
    mockInstaller: async () => state,
    shotRunners: { [scene.id]: runner } as never,
    now: options.now,
    probeMediaFile: options.probeMediaFile ?? (async () => ({ durationSeconds: 7.5 })),
    replaceFile: options.replaceFile,
  });

  return {
    outputDir,
    rawVideoPath,
    result,
    screenshotPaths,
    waits,
  };
}

async function runHookCapture(options: RecordHarnessOptions = {}) {
  return runSceneCapture({ ...options, sceneId: 'hook' });
}

test('export 镜头只读取页面暴露接口产生的实际 Markdown，且 metadata 不包含内容或路径', async () => {
  const fixtures = createDemoFixtures();
  let exportReads = 0;
  const content = '# 2026-08-12 日报\n\nUI 实际导出内容';
  const capture = await runSceneCapture({
    sceneId: 'export',
    exportedFilesSnapshot: [{
      path: fixtures.exportPath,
      content,
      kind: 'markdown',
    }],
    onExportRead: () => { exportReads += 1; },
  });

  assert.equal(exportReads, 1);
  assert.deepEqual(capture.result.exportedMarkdown, {
    fileName: '2026-08-12.md',
    content,
  });
  const metadata = await readFile(capture.result.paths.metadataPath, 'utf8');
  assert.doesNotMatch(metadata, /UI 实际导出内容|exportedMarkdown|work-review-demo/u);
});

test('export 镜头缺少 UI 实际导出的目标 Markdown 时立即失败', async () => {
  await assert.rejects(
    runSceneCapture({ sceneId: 'export', exportedFilesSnapshot: [] }),
    /export.*2026-08-12\.md.*实际导出/u,
  );
});

test('浏览器上下文阻断的外部 WebSocket 会进入录制诊断', async () => {
  await assert.rejects(
    runHookCapture({
      onContextFactory: (diagnostics) => {
        diagnostics?.blockedWebSockets.push('wss://example.com/socket');
      },
    }),
    /example\.com\/socket/,
  );
});

test('目标录制时长从页面准备且起始帧完成后开始计算', async () => {
  let currentTime = 0;
  const capture = await runHookCapture({
    now: () => currentTime,
    onContextFactory: () => {
      currentTime = 2_000;
    },
    onGoto: () => {
      currentTime = 4_000;
    },
    runner: async ({ markContentStart, captureActionFrame }) => {
      currentTime = 5_250;
      markContentStart?.();
      currentTime = 5_500;
      await captureActionFrame?.();
    },
    onScreenshot: (outputPath) => {
      if (outputPath.endsWith('-start.png')) currentTime = 5_000;
    },
  });

  assert.deepEqual(capture.waits, [7_000]);
  assert.equal(capture.result.contentStartOffsetSeconds, 3.25);
});

test('动作关键帧由分镜在关键瞬态主动捕获且 metadata 不写入宿主机绝对路径', async () => {
  const capture = await runHookCapture({
    runner: async ({ captureActionFrame }) => {
      await captureActionFrame?.();
    },
  });

  assert.equal(
    capture.screenshotPaths.filter((value) => value.endsWith('-action.png')).length,
    1,
  );
  const metadata = await readFile(capture.result.paths.metadataPath, 'utf8');
  assert.doesNotMatch(metadata, /\/Users\/|[A-Z]:\\/u);
  assert.doesNotMatch(metadata, /"paths"/u);
  assert.match(metadata, /"contentStartOffsetSeconds"/u);
});

test('metadata 使用可注入媒体探测器返回的实际 WebM 时长', async () => {
  const probedPaths: string[] = [];
  const capture = await runHookCapture({
    probeMediaFile: async (mediaPath) => {
      probedPaths.push(mediaPath);
      return { durationSeconds: 7.234 };
    },
  });

  assert.deepEqual(probedPaths, [capture.result.paths.videoPath]);
  assert.equal(capture.result.durationSeconds, 7.234);
  const metadata = JSON.parse(
    await readFile(capture.result.paths.metadataPath, 'utf8'),
  ) as { durationSeconds: number };
  assert.equal(metadata.durationSeconds, 7.234);
});

test('runner 与 context.close 同时失败时首要保留 runner 原始错误', async () => {
  await assert.rejects(
    runHookCapture({
      runner: async () => {
        throw new Error('runner failed');
      },
      close: async () => {
        throw new Error('context close failed');
      },
    }),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.match(error.message, /runner failed/);
      if (error instanceof AggregateError) {
        assert.match(String(error.errors[0]), /runner failed/);
        assert.match(String(error.errors[1]), /context close failed/);
      }
      return true;
    },
  );
});

test('目标视频已存在时通过显式替换策略覆盖旧文件', async () => {
  let sawExistingDestination = false;
  const capture = await runHookCapture({
    rawContent: 'new-video',
    existingVideoContent: 'old-video',
    replaceFile: async (sourcePath, outputPath) => {
      sawExistingDestination = (await readFile(outputPath, 'utf8')) === 'old-video';
      await writeFile(outputPath, await readFile(sourcePath));
    },
  });

  assert.equal(sawExistingDestination, true);
  assert.equal(await readFile(capture.result.paths.videoPath, 'utf8'), 'new-video');
});

test('结构化 Mock state 中的未实现命令会阻止录制', async () => {
  const fixtures = createDemoFixtures();
  const state = Object.assign(createDemoMockState(fixtures), {
    unhandledCommands: ['quiet_missing_command'],
  });

  await assert.rejects(
    runHookCapture({ state }),
    /quiet_missing_command/,
  );
});


test('浏览器关闭失败时仍会继续停止 Vite 服务', async () => {
  const order: string[] = [];
  const closeError = new Error('browser close failed');

  await assert.rejects(
    cleanupCaptureResources(
      { close: async () => { order.push('browser'); throw closeError; } } as unknown as Browser,
      { baseUrl: new URL('http://127.0.0.1:5173/'), process: {} as never, logs: [] },
      async () => { order.push('server'); },
    ),
    (error: unknown) => {
      assert.equal(error, closeError);
      assert.deepEqual(order, ['browser', 'server']);
      return true;
    },
  );
});
