import { mkdir, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';

import type { Browser, BrowserContext, Page, Video } from 'playwright';

import {
  DEMO_BASE_URL,
  createDemoContext,
  isAllowedDemoRequest,
  launchDemoBrowser,
  startDemoServer,
  stopDemoServer,
  type DemoServer,
} from './browser.ts';
import { probeMedia } from './ffmpeg.ts';
import { createDemoFixtures } from './fixtures.ts';
import { STORYBOARD } from './storyboard.ts';
import { SHOT_RUNNERS } from './shots.ts';
import { installDemoTauriMock } from './tauriMock.ts';
import type {
  DemoAspect,
  DemoFixtures,
  DemoMockState,
  StoryboardScene,
  StoryboardSceneId,
} from './types.ts';

export interface CaptureDiagnostics {
  pageErrors: string[];
  consoleErrors: string[];
  blockedRequests: string[];
  unhandledCommands: string[];
}

export interface SceneCapturePaths {
  aspectDir: string;
  videoPath: string;
  startFramePath: string;
  actionFramePath: string;
  endFramePath: string;
  metadataPath: string;
}

export interface ExportedMarkdownCapture {
  fileName: string;
  content: string;
}

export interface SceneCaptureResult {
  scene: StoryboardSceneId;
  aspect: DemoAspect;
  paths: SceneCapturePaths;
  contentStartOffsetSeconds: number;
  durationSeconds: number;
  diagnostics: CaptureDiagnostics;
  invokeCommands: string[];
  /** 仅在进程内传递，不得写入分镜 metadata。 */
  exportedMarkdown?: ExportedMarkdownCapture;
}

interface RecordSceneDependencies {
  browser: Browser;
  baseUrl?: URL;
  fixtures?: DemoFixtures;
  contextFactory?: typeof createDemoContext;
  mockInstaller?: typeof installDemoTauriMock;
  shotRunners?: typeof SHOT_RUNNERS;
  now?: () => number;
  probeMediaFile?: (mediaPath: string) => Promise<{ durationSeconds: number | null }>;
  replaceFile?: (sourcePath: string, outputPath: string) => Promise<void>;
}

export interface CaptureAllScenesOptions {
  outputDir: string;
  aspects?: readonly DemoAspect[];
  sceneIds?: readonly StoryboardSceneId[];
  cwd?: string;
  baseUrl?: URL;
}

export function buildDemoRouteUrl(baseUrl: URL, route: string): string {
  const normalizedRoute = route.startsWith('/') ? route : `/${route}`;
  const url = new URL(baseUrl.href);
  url.hash = `#${normalizedRoute}`;
  return url.href;
}

export function buildSceneCapturePaths(
  outputDir: string,
  aspect: DemoAspect,
  sceneId: StoryboardSceneId,
): SceneCapturePaths {
  const aspectDir = path.join(outputDir, aspect);
  return {
    aspectDir,
    videoPath: path.join(aspectDir, `${sceneId}.webm`),
    startFramePath: path.join(aspectDir, `${sceneId}-start.png`),
    actionFramePath: path.join(aspectDir, `${sceneId}-action.png`),
    endFramePath: path.join(aspectDir, `${sceneId}-end.png`),
    metadataPath: path.join(aspectDir, `${sceneId}.json`),
  };
}

export function createCaptureDiagnostics(): CaptureDiagnostics {
  return {
    pageErrors: [],
    consoleErrors: [],
    blockedRequests: [],
    unhandledCommands: [],
  };
}

export function assertCleanCaptureDiagnostics(
  diagnostics: CaptureDiagnostics,
  sceneId: StoryboardSceneId,
): void {
  const details = [
    ...diagnostics.pageErrors.map((value) => `页面错误：${value}`),
    ...diagnostics.consoleErrors.map((value) => `控制台错误：${value}`),
    ...diagnostics.blockedRequests.map((value) => `外部请求：${value}`),
    ...diagnostics.unhandledCommands.map((value) => `未处理命令：${value}`),
  ];
  if (details.length > 0) {
    throw new Error(`分镜 ${sceneId} 录制诊断失败：\n${details.join('\n')}`);
  }
}

function attachPageDiagnostics(
  page: Page,
  diagnostics: CaptureDiagnostics,
  baseUrl: URL,
): void {
  page.on('pageerror', (error) => diagnostics.pageErrors.push(error.message));
  page.on('console', (message) => {
    if (message.type() !== 'error') return;
    const text = message.text();
    diagnostics.consoleErrors.push(text);
    const match = /产品演示 Tauri Mock 未实现命令：([^\s]+)/u.exec(text);
    if (match?.[1]) diagnostics.unhandledCommands.push(match[1]);
  });
  page.on('request', (request) => {
    const url = request.url();
    if (!isAllowedDemoRequest(url, baseUrl)) diagnostics.blockedRequests.push(url);
  });
}

async function captureFrame(page: Page, outputPath: string): Promise<void> {
  await page.screenshot({
    path: outputPath,
    fullPage: false,
    animations: 'disabled',
    caret: 'hide',
  });
}

async function replaceExistingFile(sourcePath: string, outputPath: string): Promise<void> {
  // Windows 不允许 rename 直接覆盖已存在目标；先显式移除，保证重复录制语义一致。
  await rm(outputPath, { force: true });
  await rename(sourcePath, outputPath);
}

async function persistVideo(
  video: Video | null,
  outputPath: string,
  replaceFile: (sourcePath: string, destinationPath: string) => Promise<void>,
): Promise<void> {
  if (!video) throw new Error(`分镜没有产生 Playwright 录屏：${outputPath}`);
  const temporaryPath = await video.path();
  await replaceFile(temporaryPath, outputPath);
}

function collectStructuredMockDiagnostics(
  state: DemoMockState | undefined,
  diagnostics: CaptureDiagnostics,
): void {
  if (!state) return;

  for (const command of state.unhandledCommands) {
    if (typeof command !== 'string' || diagnostics.unhandledCommands.includes(command)) continue;
    diagnostics.unhandledCommands.push(command);
  }
}

function requiredMediaDuration(
  durationSeconds: number | null,
  mediaPath: string,
): number {
  if (durationSeconds === null || !Number.isFinite(durationSeconds) || durationSeconds <= 0) {
    throw new Error(`无法读取分镜 WebM 实际时长：${mediaPath}`);
  }
  return durationSeconds;
}

function combineCaptureAndCloseErrors(
  sceneId: StoryboardSceneId,
  captureError: unknown,
  closeError: unknown,
): AggregateError {
  const captureMessage = captureError instanceof Error ? captureError.message : String(captureError);
  const closeMessage = closeError instanceof Error ? closeError.message : String(closeError);
  return new AggregateError(
    [captureError, closeError],
    `分镜 ${sceneId} 录制失败：${captureMessage}；关闭浏览器上下文也失败：${closeMessage}`,
  );
}

function findScene(sceneId: StoryboardSceneId): StoryboardScene {
  const scene = STORYBOARD.find((item) => item.id === sceneId);
  if (!scene) throw new Error(`未知产品演示分镜：${sceneId}`);
  return scene;
}

interface BrowserExportedFile {
  path: string;
  content: string;
  kind: 'markdown';
}

function requireExportedMarkdown(
  value: unknown,
  expectedPath: string,
): ExportedMarkdownCapture {
  if (!Array.isArray(value)) {
    throw new Error('export 镜头无法读取 UI 实际导出文件列表');
  }
  const matching = value.filter((entry): entry is BrowserExportedFile => (
    Boolean(entry)
    && typeof entry === 'object'
    && (entry as Record<string, unknown>).path === expectedPath
    && (entry as Record<string, unknown>).kind === 'markdown'
  ));
  if (matching.length !== 1) {
    throw new Error(`export 镜头缺少唯一的 ${path.basename(expectedPath)} UI 实际导出`);
  }
  const content = matching[0]!.content;
  if (typeof content !== 'string' || !content.trim()) {
    throw new Error(`export 镜头的 UI 实际导出 ${path.basename(expectedPath)} 内容为空`);
  }
  return { fileName: path.basename(expectedPath), content };
}

async function readExportedMarkdown(
  page: Page,
  expectedPath: string,
): Promise<ExportedMarkdownCapture> {
  const exportedFiles = await page.evaluate(async () => {
    const readFiles = (window as typeof window & {
      __WORK_REVIEW_DEMO_EXPORTED_FILES__?: () => Promise<unknown>;
    }).__WORK_REVIEW_DEMO_EXPORTED_FILES__;
    if (typeof readFiles !== 'function') {
      throw new Error('页面未暴露 UI 实际导出读取接口');
    }
    return readFiles();
  });
  return requireExportedMarkdown(exportedFiles, expectedPath);
}

export async function recordScene(
  scene: StoryboardScene,
  aspect: DemoAspect,
  outputDir: string,
  dependencies: RecordSceneDependencies,
): Promise<SceneCaptureResult> {
  const baseUrl = dependencies.baseUrl ?? DEMO_BASE_URL;
  const fixtures = dependencies.fixtures ?? createDemoFixtures();
  const contextFactory = dependencies.contextFactory ?? createDemoContext;
  const mockInstaller = dependencies.mockInstaller ?? installDemoTauriMock;
  const shotRunners = dependencies.shotRunners ?? SHOT_RUNNERS;
  const now = dependencies.now ?? Date.now;
  const probeMediaFile = dependencies.probeMediaFile ?? ((mediaPath: string) => probeMedia(mediaPath));
  const replaceFile = dependencies.replaceFile ?? replaceExistingFile;
  const paths = buildSceneCapturePaths(outputDir, aspect, scene.id);
  const diagnostics = createCaptureDiagnostics();
  let context: BrowserContext | undefined;
  let video: Video | null = null;
  let state: DemoMockState | undefined;
  let captureError: unknown;
  let recordingOriginAt = 0;
  let contentStartAt: number | undefined;
  let actionFrameCaptured = false;
  let exportedMarkdown: ExportedMarkdownCapture | undefined;

  await mkdir(paths.aspectDir, { recursive: true });

  try {
    context = await contextFactory(
      dependencies.browser,
      aspect,
      paths.aspectDir,
      baseUrl,
      { blockedWebSockets: diagnostics.blockedRequests },
    );
    state = await mockInstaller(context, fixtures);
    const page = await context.newPage();
    video = page.video();
    recordingOriginAt = now();
    attachPageDiagnostics(page, diagnostics, baseUrl);

    await page.goto(buildDemoRouteUrl(baseUrl, scene.route), {
      waitUntil: 'networkidle',
      timeout: 30_000,
    });
    await page.waitForFunction(() => document.fonts?.status === 'loaded', undefined, {
      timeout: 10_000,
    });
    await captureFrame(page, paths.startFramePath);
    const recordingStartedAt = now();

    const runner = shotRunners[scene.id];
    await runner({
      page,
      aspect,
      scene,
      fixtures,
      markContentStart: () => {
        if (contentStartAt !== undefined) {
          throw new Error(`分镜 ${scene.id} 重复标记正式内容起点`);
        }
        contentStartAt = now();
      },
      captureActionFrame: async () => {
        if (actionFrameCaptured) {
          throw new Error(`分镜 ${scene.id} 重复捕获动作关键帧`);
        }
        await captureFrame(page, paths.actionFramePath);
        actionFrameCaptured = true;
      },
    });
    if (contentStartAt === undefined) contentStartAt = recordingStartedAt;
    if (!actionFrameCaptured) {
      await captureFrame(page, paths.actionFramePath);
      actionFrameCaptured = true;
    }

    const targetMilliseconds = (scene.end - scene.start) * 1_000 + 500;
    const remainingMilliseconds = Math.max(250, targetMilliseconds - (now() - recordingStartedAt));
    await page.waitForTimeout(remainingMilliseconds);
    await captureFrame(page, paths.endFramePath);
    if (scene.id === 'export') {
      exportedMarkdown = await readExportedMarkdown(page, fixtures.exportPath);
    }

    collectStructuredMockDiagnostics(state, diagnostics);
    assertCleanCaptureDiagnostics(diagnostics, scene.id);
  } catch (error) {
    captureError = error;
  }

  try {
    await context?.close();
  } catch (closeError) {
    if (captureError !== undefined) {
      throw combineCaptureAndCloseErrors(scene.id, captureError, closeError);
    }
    throw closeError;
  }
  if (captureError !== undefined) throw captureError;

  await persistVideo(video, paths.videoPath, replaceFile);
  const mediaProbe = await probeMediaFile(paths.videoPath);
  const durationSeconds = requiredMediaDuration(mediaProbe.durationSeconds, paths.videoPath);
  const contentStartOffsetSeconds = Math.max(
    0,
    ((contentStartAt ?? recordingOriginAt) - recordingOriginAt) / 1_000,
  );
  const result: SceneCaptureResult = {
    scene: scene.id,
    aspect,
    paths,
    contentStartOffsetSeconds,
    durationSeconds,
    diagnostics,
    invokeCommands: state?.invokeLog.map((entry) => entry.command) ?? [],
    ...(exportedMarkdown ? { exportedMarkdown } : {}),
  };
  const metadata = {
    scene: result.scene,
    aspect: result.aspect,
    contentStartOffsetSeconds: result.contentStartOffsetSeconds,
    durationSeconds: result.durationSeconds,
    diagnostics: result.diagnostics,
    invokeCommands: result.invokeCommands,
  };
  await writeFile(paths.metadataPath, `${JSON.stringify(metadata, null, 2)}\n`, 'utf8');
  return result;
}

/** 无论浏览器关闭是否失败，都保证继续停止 Vite 服务，并保留首个错误。 */
export async function cleanupCaptureResources(
  browser: Browser | undefined,
  server: DemoServer | undefined,
  stopServer: typeof stopDemoServer = stopDemoServer,
): Promise<void> {
  let firstError: unknown;
  try {
    await browser?.close();
  } catch (error) {
    firstError = error;
  }

  try {
    if (server) await stopServer(server);
  } catch (error) {
    if (firstError === undefined) firstError = error;
  }

  if (firstError !== undefined) throw firstError;
}

export async function captureAllScenes(
  options: CaptureAllScenesOptions,
): Promise<SceneCaptureResult[]> {
  const aspects = options.aspects ?? ['16x9', '9x16'];
  const sceneIds = options.sceneIds ?? STORYBOARD.map((scene) => scene.id);
  const baseUrl = options.baseUrl ?? DEMO_BASE_URL;
  let server: DemoServer | undefined;
  let browser: Browser | undefined;

  try {
    server = await startDemoServer({ cwd: options.cwd, baseUrl });
    browser = await launchDemoBrowser();
    const results: SceneCaptureResult[] = [];
    for (const aspect of aspects) {
      for (const sceneId of sceneIds) {
        const scene = findScene(sceneId);
        results.push(await recordScene(scene, aspect, options.outputDir, {
          browser,
          baseUrl,
        }));
      }
    }
    return results;
  } finally {
    await cleanupCaptureResources(browser, server);
  }
}
