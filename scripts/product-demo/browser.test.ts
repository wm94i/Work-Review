import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import test from 'node:test';
import vm from 'node:vm';

import type { ChildProcess } from 'node:child_process';
import type { Browser, BrowserContext, WebSocketRoute } from 'playwright';

import {
  DEMO_FIXED_TIME,
  DEMO_CHROMIUM_ARGS,
  buildDemoServerCommand,
  buildDeterministicBrowserInitScript,
  createDemoContext,
  createDemoContextOptions,
  isAllowedDemoRequest,
  normalizeDemoBaseUrl,
  stopDemoServer,
  waitForDemoServer,
} from './browser.ts';

test('固定演示时间晚于当天最后活动和日报生成时间', () => {
  assert.equal(DEMO_FIXED_TIME, '2026-08-12T18:00:00+08:00');
});

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function createFakeChildProcess(
  onKill: (signal: NodeJS.Signals, process: EventEmitter) => void,
): ChildProcess {
  const child = new EventEmitter() as EventEmitter & {
    exitCode: number | null;
    signalCode: NodeJS.Signals | null;
    kill: (signal?: NodeJS.Signals | number) => boolean;
  };
  child.exitCode = null;
  child.signalCode = null;
  child.kill = (signal = 'SIGTERM') => {
    onKill(signal as NodeJS.Signals, child);
    return true;
  };
  return child as unknown as ChildProcess;
}

test('演示地址只接受 HTTP(S) 并规范为目录 URL', () => {
  assert.equal(normalizeDemoBaseUrl('http://127.0.0.1:5173').href, 'http://127.0.0.1:5173/');
  assert.equal(normalizeDemoBaseUrl('https://localhost:5173/demo').href, 'https://localhost:5173/demo/');
  assert.throws(() => normalizeDemoBaseUrl('file:///tmp/demo'), /HTTP\(S\)/);
});

test('网络白名单只放行演示同源、data 和 blob 请求', () => {
  const baseUrl = new URL('http://127.0.0.1:5173/');
  assert.equal(isAllowedDemoRequest('http://127.0.0.1:5173/src/main.ts', baseUrl), true);
  assert.equal(isAllowedDemoRequest('data:image/png;base64,AA==', baseUrl), true);
  assert.equal(isAllowedDemoRequest('blob:http://127.0.0.1:5173/demo', baseUrl), true);
  assert.equal(isAllowedDemoRequest('https://docs.example.test/demo', baseUrl), false);
  assert.equal(isAllowedDemoRequest('http://localhost:5173/', baseUrl), false);
  assert.equal(isAllowedDemoRequest('not-a-url', baseUrl), false);
});

test('固定浏览器状态脚本不依赖 tsx 辅助函数并冻结演示时间', () => {
  const script = buildDeterministicBrowserInitScript('2026-08-12T10:30:00+08:00');
  assert.doesNotMatch(script, /__name/);
  assert.match(script, /2026-08-12T10:30:00\+08:00/);
  assert.match(script, /Date/);
  assert.match(script, /Math\.random/);
});

test('固定 Date 保留函数、构造器、静态方法和 instanceof 原生语义', () => {
  const fixedTime = '2026-08-12T10:30:00+08:00';
  const storage = new Map<string, string>();
  const browserWindow = {
    Date,
    localStorage: {
      setItem(key: string, value: string) {
        storage.set(key, value);
      },
    },
  };
  const document = {
    head: { appendChild() {} },
    createElement() {
      return { dataset: {}, textContent: '' };
    },
    addEventListener() {},
  };
  const sandbox = {
    window: browserWindow,
    document,
    Math: Object.create(Math) as Math,
  };

  vm.runInNewContext(buildDeterministicBrowserInitScript(fixedTime), sandbox);

  const DemoDate = browserWindow.Date;
  const fixedTimestamp = new Date(fixedTime).getTime();
  const fixedDate = new DemoDate();
  const explicitDate = new DemoDate('2020-01-02T03:04:05.000Z');

  assert.equal(DemoDate(), new Date(fixedTimestamp).toString());
  assert.equal(fixedDate.getTime(), fixedTimestamp);
  assert.equal(explicitDate.toISOString(), '2020-01-02T03:04:05.000Z');
  assert.equal(DemoDate.now(), fixedTimestamp);
  assert.equal(DemoDate.parse('2020-01-02T03:04:05.000Z'), Date.parse('2020-01-02T03:04:05.000Z'));
  assert.equal(DemoDate.UTC(2020, 0, 2, 3, 4, 5), Date.UTC(2020, 0, 2, 3, 4, 5));
  assert.equal(fixedDate instanceof DemoDate, true);
  assert.equal(fixedDate instanceof Date, true);
});

test('浏览器录制参数包含稳定字体和 sRGB 开关', () => {
  assert.deepEqual(DEMO_CHROMIUM_ARGS, [
    '--disable-gpu',
    '--disable-lcd-text',
    '--disable-skia-runtime-opts',
    '--font-render-hinting=none',
    '--force-color-profile=srgb',
  ]);
  assert.deepEqual(createDemoContextOptions('9x16').viewport, { width: 1080, height: 1920 });
});

test('服务轮询在返回成功状态后结束', async () => {
  let calls = 0;
  await waitForDemoServer(new URL('http://127.0.0.1:5173/'), {
    timeoutMs: 100,
    pollIntervalMs: 0,
    fetchImpl: async () => {
      calls += 1;
      return { ok: calls >= 3 };
    },
  });
  assert.equal(calls, 3);
});

test('服务轮询超时会提供明确错误', async () => {
  await assert.rejects(
    waitForDemoServer(new URL('http://127.0.0.1:5173/'), {
      timeoutMs: 5,
      pollIntervalMs: 0,
      fetchImpl: async () => ({ ok: false }),
    }),
    /Vite.*超时/,
  );
});

test('服务单次 fetch 永不结束时仍在总超时内失败', async () => {
  const startedAt = Date.now();
  const outcome = await Promise.race([
    waitForDemoServer(new URL('http://127.0.0.1:5173/'), {
      timeoutMs: 20,
      pollIntervalMs: 0,
      fetchImpl: async () => new Promise<{ ok: boolean }>(() => {}),
    }).then(
      () => ({ status: 'resolved' as const }),
      (error: unknown) => ({ status: 'rejected' as const, error }),
    ),
    sleep(150).then(() => ({ status: 'hung' as const })),
  ]);

  assert.equal(outcome.status, 'rejected');
  assert.match(String('error' in outcome ? outcome.error : ''), /Vite.*超时/);
  assert.ok(Date.now() - startedAt < 140, '服务轮询必须受总超时约束');
});

test('停止服务会先注册 exit 等待再发送 SIGKILL', async () => {
  const signals: NodeJS.Signals[] = [];
  let listenersAtSigkill = 0;
  const child = createFakeChildProcess((signal, process) => {
    signals.push(signal);
    if (signal === 'SIGKILL') {
      listenersAtSigkill = process.listenerCount('exit');
      process.emit('exit', null, 'SIGKILL');
    }
  });

  const outcome = await Promise.race([
    stopDemoServer(
      { baseUrl: new URL('http://127.0.0.1:5173/'), process: child, logs: [] },
      { gracefulTimeoutMs: 5, forceTimeoutMs: 20 },
    ).then(() => 'completed' as const),
    sleep(150).then(() => 'hung' as const),
  ]);

  assert.equal(outcome, 'completed');
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL']);
  assert.ok(listenersAtSigkill > 0, 'SIGKILL 前必须先监听 exit');
});

test('停止服务在 SIGKILL 后仍无 exit 时有限超时并报错', async () => {
  const child = createFakeChildProcess(() => {});
  const outcome = await Promise.race([
    stopDemoServer(
      { baseUrl: new URL('http://127.0.0.1:5173/'), process: child, logs: [] },
      { gracefulTimeoutMs: 5, forceTimeoutMs: 10 },
    ).then(
      () => ({ status: 'resolved' as const }),
      (error: unknown) => ({ status: 'rejected' as const, error }),
    ),
    sleep(150).then(() => ({ status: 'hung' as const })),
  ]);

  assert.equal(outcome.status, 'rejected');
  assert.match(String('error' in outcome ? outcome.error : ''), /SIGKILL.*超时/);
});

test('浏览器上下文放行同源 WebSocket 并阻断、记录外部 WebSocket', async () => {
  let websocketHandler: ((route: WebSocketRoute) => Promise<unknown> | unknown) | undefined;
  const fakeContext = {
    async route() {},
    async routeWebSocket(
      _url: string | RegExp | ((url: URL) => boolean),
      handler: (route: WebSocketRoute) => Promise<unknown> | unknown,
    ) {
      websocketHandler = handler;
    },
    async addInitScript() {},
  };
  const fakeBrowser = {
    async newContext() {
      return fakeContext as unknown as BrowserContext;
    },
  };
  const diagnostics = { blockedWebSockets: [] as string[] };
  const baseUrl = new URL('http://127.0.0.1:5173/');

  await createDemoContext(
    fakeBrowser as unknown as Browser,
    '16x9',
    undefined,
    baseUrl,
    diagnostics,
  );
  assert.ok(websocketHandler, '必须注册 WebSocket 路由');

  let sameOriginConnected = false;
  let sameOriginClosed = false;
  await websocketHandler({
    url: () => 'ws://127.0.0.1:5173/?token=demo',
    connectToServer() {
      sameOriginConnected = true;
      return {} as WebSocketRoute;
    },
    async close() {
      sameOriginClosed = true;
    },
  } as unknown as WebSocketRoute);

  let externalConnected = false;
  let externalCloseOptions: { code?: number; reason?: string } | undefined;
  await websocketHandler({
    url: () => 'wss://telemetry.example.test/socket',
    connectToServer() {
      externalConnected = true;
      return {} as WebSocketRoute;
    },
    async close(options?: { code?: number; reason?: string }) {
      externalCloseOptions = options;
    },
  } as unknown as WebSocketRoute);

  assert.equal(sameOriginConnected, true);
  assert.equal(sameOriginClosed, false);
  assert.equal(externalConnected, false);
  assert.deepEqual(externalCloseOptions, {
    code: 1008,
    reason: '产品演示禁止外部 WebSocket',
  });
  assert.deepEqual(diagnostics.blockedWebSockets, [
    'wss://telemetry.example.test/socket',
  ]);
});


test('演示服务直接启动本地 Vite 入口，避免 npm 孙进程残留', () => {
  const command = buildDemoServerCommand(
    '/workspace/work-review',
    new URL('http://127.0.0.1:5173/'),
  );

  assert.equal(command.binary, process.execPath);
  assert.deepEqual(command.args, [
    '/workspace/work-review/node_modules/vite/bin/vite.js',
    '--host',
    '127.0.0.1',
    '--port',
    '5173',
    '--strictPort',
  ]);
  assert.equal(command.cwd, '/workspace/work-review');
  assert.doesNotMatch(command.args.join(' '), /npm run dev/u);
});
