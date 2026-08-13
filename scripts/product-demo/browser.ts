import { spawn, type ChildProcess } from 'node:child_process';
import { access } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';

import {
  chromium,
  type Browser,
  type BrowserContext,
  type BrowserContextOptions,
} from 'playwright';

import type { DemoAspect } from './types.ts';

export const DEMO_FIXED_TIME = '2026-08-12T18:00:00+08:00';
export const DEMO_BASE_URL = new URL('http://127.0.0.1:5173/');

export const DEMO_CHROMIUM_ARGS = [
  '--disable-gpu',
  '--disable-lcd-text',
  '--disable-skia-runtime-opts',
  '--font-render-hinting=none',
  '--force-color-profile=srgb',
] as const;

const MACOS_BROWSER_FALLBACKS = [
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
] as const;

export interface DemoRecordingConfig {
  viewport: { width: number; height: number };
  videoSize: { width: number; height: number };
  deviceScaleFactor: number;
  locale: 'zh-CN';
  timezoneId: 'Asia/Shanghai';
  reducedMotion: 'reduce';
}

export interface DemoServer {
  baseUrl: URL;
  process: ChildProcess;
  logs: string[];
}

export interface WaitForDemoServerOptions {
  timeoutMs?: number;
  pollIntervalMs?: number;
  fetchImpl?: (
    url: string,
    options?: { signal?: AbortSignal },
  ) => Promise<{ ok: boolean }>;
}

export interface StopDemoServerOptions {
  gracefulTimeoutMs?: number;
  forceTimeoutMs?: number;
}

export interface DemoNetworkDiagnostics {
  blockedWebSockets: string[];
}

export const DEMO_RECORDING_CONFIG: Record<DemoAspect, DemoRecordingConfig> = {
  '16x9': {
    viewport: { width: 1920, height: 1080 },
    videoSize: { width: 1920, height: 1080 },
    deviceScaleFactor: 1,
    locale: 'zh-CN',
    timezoneId: 'Asia/Shanghai',
    reducedMotion: 'reduce',
  },
  '9x16': {
    viewport: { width: 1080, height: 1920 },
    videoSize: { width: 1080, height: 1920 },
    deviceScaleFactor: 1,
    locale: 'zh-CN',
    timezoneId: 'Asia/Shanghai',
    reducedMotion: 'reduce',
  },
};

export function normalizeDemoBaseUrl(value: string): URL {
  const url = new URL(value);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error('产品演示地址必须是 HTTP(S) URL。');
  }
  if (!url.pathname.endsWith('/')) url.pathname += '/';
  return url;
}

export function isAllowedDemoRequest(value: string, baseUrl = DEMO_BASE_URL): boolean {
  try {
    const requestUrl = new URL(value);
    return requestUrl.origin === baseUrl.origin
      || requestUrl.protocol === 'data:'
      || requestUrl.protocol === 'blob:';
  } catch {
    return false;
  }
}

export function isAllowedDemoWebSocket(
  value: string,
  baseUrl = DEMO_BASE_URL,
): boolean {
  try {
    const websocketUrl = new URL(value);
    if (websocketUrl.protocol !== 'ws:' && websocketUrl.protocol !== 'wss:') {
      return false;
    }
    websocketUrl.protocol = websocketUrl.protocol === 'ws:' ? 'http:' : 'https:';
    return websocketUrl.origin === baseUrl.origin;
  } catch {
    return false;
  }
}

export function createDemoContextOptions(
  aspect: DemoAspect,
  videoDir?: string,
): BrowserContextOptions {
  const config = DEMO_RECORDING_CONFIG[aspect];
  return {
    viewport: { ...config.viewport },
    deviceScaleFactor: config.deviceScaleFactor,
    locale: config.locale,
    timezoneId: config.timezoneId,
    reducedMotion: config.reducedMotion,
    colorScheme: 'light',
    ...(videoDir
      ? {
          recordVideo: {
            dir: videoDir,
            size: { ...config.videoSize },
          },
        }
      : {}),
  };
}

export function buildDeterministicBrowserInitScript(fixedTime = DEMO_FIXED_TIME): string {
  const serializedTime = JSON.stringify(fixedTime);
  return `(() => {
    const NativeDate = window.Date;
    const fixedTimestamp = new NativeDate(${serializedTime}).getTime();
    function DemoDate(...args) {
      if (!new.target) return new NativeDate(fixedTimestamp).toString();
      return Reflect.construct(
        NativeDate,
        args.length === 0 ? [fixedTimestamp] : args,
        new.target,
      );
    }
    Object.setPrototypeOf(DemoDate, NativeDate);
    DemoDate.prototype = NativeDate.prototype;
    Object.defineProperty(DemoDate, 'now', {
      configurable: true,
      value: () => fixedTimestamp,
    });
    window.Date = DemoDate;
    Math.random = () => 0.424242;
    window.localStorage.setItem('work-review.locale', 'zh-CN');
    window.localStorage.setItem('theme', 'light');
    const style = document.createElement('style');
    style.dataset.workReviewDemo = 'deterministic';
    style.textContent = '*,:before,:after{scroll-behavior:auto!important;}';
    const attachStyle = () => document.head?.appendChild(style);
    if (document.head) attachStyle();
    else document.addEventListener('DOMContentLoaded', attachStyle, { once: true });
  })();`;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function fetchBeforeDeadline(
  url: string,
  remainingMs: number,
  fetchImpl: NonNullable<WaitForDemoServerOptions['fetchImpl']>,
): Promise<{ ok: boolean }> {
  const controller = new AbortController();
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      fetchImpl(url, { signal: controller.signal }),
      new Promise<never>((_, reject) => {
        timeout = setTimeout(() => {
          controller.abort();
          reject(new Error('产品演示服务单次请求超时。'));
        }, Math.max(0, remainingMs));
      }),
    ]);
  } finally {
    if (timeout) clearTimeout(timeout);
  }
}

export async function waitForDemoServer(
  baseUrl: URL,
  options: WaitForDemoServerOptions = {},
): Promise<void> {
  const timeoutMs = options.timeoutMs ?? 30_000;
  const pollIntervalMs = options.pollIntervalMs ?? 150;
  const fetchImpl = options.fetchImpl ?? (
    (url: string, requestOptions?: { signal?: AbortSignal }) => fetch(url, requestOptions)
  );
  const deadline = Date.now() + timeoutMs;

  while (Date.now() <= deadline) {
    const remainingMs = deadline - Date.now();
    if (remainingMs < 0) break;
    try {
      const response = await fetchBeforeDeadline(baseUrl.href, remainingMs, fetchImpl);
      if (response.ok) return;
    } catch {
      // Vite 启动期间连接失败或单次请求超时属于预期，由总截止时间统一判定。
    }
    const delayMs = Math.min(pollIntervalMs, Math.max(0, deadline - Date.now()));
    if (delayMs > 0) await delay(delayMs);
  }

  throw new Error(`等待产品演示 Vite 服务超时（${timeoutMs}ms）：${baseUrl.href}`);
}

export interface DemoServerCommand {
  binary: string;
  args: string[];
  cwd: string;
}

/** 直接启动本地 Vite CLI，避免 npm 包装进程遗留孙进程。 */
export function buildDemoServerCommand(cwd: string, baseUrl: URL): DemoServerCommand {
  const port = baseUrl.port || (baseUrl.protocol === 'https:' ? '443' : '80');
  return {
    binary: process.execPath,
    args: [
      path.join(cwd, 'node_modules', 'vite', 'bin', 'vite.js'),
      '--host',
      baseUrl.hostname,
      '--port',
      port,
      '--strictPort',
    ],
    cwd,
  };
}

export async function startDemoServer(options: {
  cwd?: string;
  baseUrl?: URL;
  timeoutMs?: number;
} = {}): Promise<DemoServer> {
  const baseUrl = options.baseUrl ?? DEMO_BASE_URL;
  const command = buildDemoServerCommand(options.cwd ?? process.cwd(), baseUrl);
  const child = spawn(
    command.binary,
    command.args,
    {
      cwd: command.cwd,
      env: { ...process.env, NO_COLOR: '1' },
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  );
  const logs: string[] = [];
  child.stdout.on('data', (chunk: Buffer) => logs.push(chunk.toString()));
  child.stderr.on('data', (chunk: Buffer) => logs.push(chunk.toString()));

  try {
    await Promise.race([
      waitForDemoServer(baseUrl, { timeoutMs: options.timeoutMs }),
      new Promise<never>((_, reject) => {
        child.once('exit', (code, signal) => {
          reject(new Error(
            `产品演示 Vite 服务提前退出（code=${String(code)}, signal=${String(signal)}）：\n${logs.join('')}`,
          ));
        });
        child.once('error', reject);
      }),
    ]);
  } catch (error) {
    await stopDemoServer({ baseUrl, process: child, logs });
    throw error;
  }

  return { baseUrl, process: child, logs };
}

function hasProcessExited(child: ChildProcess): boolean {
  return child.exitCode !== null || child.signalCode !== null;
}

function waitForProcessExit(child: ChildProcess, timeoutMs: number): Promise<boolean> {
  if (hasProcessExited(child)) return Promise.resolve(true);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (exited: boolean) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      child.off('exit', onExit);
      resolve(exited);
    };
    const onExit = () => finish(true);
    child.once('exit', onExit);
    const timeout = setTimeout(() => finish(false), Math.max(0, timeoutMs));
    if (hasProcessExited(child)) finish(true);
  });
}

export async function stopDemoServer(
  server: DemoServer,
  options: StopDemoServerOptions = {},
): Promise<void> {
  const gracefulTimeoutMs = options.gracefulTimeoutMs ?? 2_000;
  const forceTimeoutMs = options.forceTimeoutMs ?? 2_000;
  if (hasProcessExited(server.process)) return;

  const gracefulExit = waitForProcessExit(server.process, gracefulTimeoutMs);
  server.process.kill('SIGTERM');
  if (await gracefulExit || hasProcessExited(server.process)) return;

  const forcedExit = waitForProcessExit(server.process, forceTimeoutMs);
  server.process.kill('SIGKILL');
  if (await forcedExit || hasProcessExited(server.process)) return;

  throw new Error(`产品演示 Vite 服务 SIGKILL 后退出超时（${forceTimeoutMs}ms）。`);
}

async function firstAccessible(paths: readonly string[]): Promise<string | undefined> {
  for (const candidate of paths) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // 继续尝试下一个候选浏览器。
    }
  }
  return undefined;
}

export async function resolveDemoBrowserExecutable(): Promise<string | undefined> {
  const bundled = chromium.executablePath();
  const candidates = process.platform === 'darwin'
    ? [bundled, ...MACOS_BROWSER_FALLBACKS]
    : [bundled];
  return firstAccessible(candidates);
}

export async function launchDemoBrowser(): Promise<Browser> {
  const executablePath = await resolveDemoBrowserExecutable();
  if (!executablePath) {
    throw new Error(
      '未找到可用浏览器。请执行 `npx playwright install chromium`，或在 macOS 安装 Google Chrome / Microsoft Edge。',
    );
  }
  return chromium.launch({
    headless: true,
    executablePath,
    args: [...DEMO_CHROMIUM_ARGS],
  });
}

export async function installDeterministicBrowserState(
  context: BrowserContext,
  fixedTime = DEMO_FIXED_TIME,
): Promise<void> {
  await context.addInitScript({ content: buildDeterministicBrowserInitScript(fixedTime) });
}

export async function createDemoContext(
  browser: Browser,
  aspect: DemoAspect,
  videoDir?: string,
  baseUrl = DEMO_BASE_URL,
  diagnostics?: DemoNetworkDiagnostics,
): Promise<BrowserContext> {
  const context = await browser.newContext(createDemoContextOptions(aspect, videoDir));
  await context.route('**/*', async (route) => {
    if (isAllowedDemoRequest(route.request().url(), baseUrl)) {
      await route.continue();
      return;
    }
    await route.abort('blockedbyclient');
  });
  await context.routeWebSocket('**/*', async (websocket) => {
    const url = websocket.url();
    if (isAllowedDemoWebSocket(url, baseUrl)) {
      websocket.connectToServer();
      return;
    }
    diagnostics?.blockedWebSockets.push(url);
    await websocket.close({
      code: 1008,
      reason: '产品演示禁止外部 WebSocket',
    });
  });
  await installDeterministicBrowserState(context);
  return context;
}
