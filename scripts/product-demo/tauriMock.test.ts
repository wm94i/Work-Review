import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import test from 'node:test';

import { Channel } from '@tauri-apps/api/core';

import { buildHistoryPayload } from '../../src/routes/ask/historyPayload.ts';
import { createDemoFixtures } from './fixtures.ts';
import {
  createDemoMockState,
  handleDemoInvoke,
  installDemoTauriMock,
  takePendingChannelEvents,
} from './tauriMock.ts';

const createState = () => createDemoMockState(createDemoFixtures());

function readPngSize(base64: string): { width: number; height: number; bytes: Buffer } {
  const bytes = Buffer.from(base64, 'base64');
  assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
  return {
    width: bytes.readUInt32BE(16),
    height: bytes.readUInt32BE(20),
    bytes,
  };
}

function readPngInternationalText(bytes: Buffer): string[] {
  const texts: string[] = [];
  let offset = 8;
  while (offset + 12 <= bytes.length) {
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString('ascii', offset + 4, offset + 8);
    const dataStart = offset + 8;
    const dataEnd = dataStart + length;
    if (dataEnd + 4 > bytes.length) break;
    if (type === 'iTXt') texts.push(bytes.toString('utf8', dataStart, dataEnd));
    offset = dataEnd + 4;
  }
  return texts;
}

async function installBrowserHarness() {
  let source = '';
  const exposed = new Map<string, (...args: any[]) => Promise<any>>();
  const context = {
    exposeFunction: async (name: string, callback: (...args: any[]) => Promise<any>) => {
      exposed.set(name, callback);
    },
    addInitScript: async (script: unknown) => {
      source = typeof script === 'function'
        ? String(script)
        : String((script as { content?: unknown }).content ?? '');
    },
  };
  const state = await installDemoTauriMock(
    context as unknown as Parameters<typeof installDemoTauriMock>[0],
    createDemoFixtures(),
  );
  const timerDelays: number[] = [];
  let timerTurn = 0;
  const localStorageValues = new Map<string, string>();
  const windowObject: Record<string, any> = {
    localStorage: {
      setItem: (key: string, value: string) => localStorageValues.set(key, value),
    },
    __WORK_REVIEW_DEMO_INVOKE__: exposed.get('__WORK_REVIEW_DEMO_INVOKE__'),
    __WORK_REVIEW_DEMO_SHOT_INVOKES__: exposed.get('__WORK_REVIEW_DEMO_SHOT_INVOKES__'),
    __WORK_REVIEW_DEMO_EXPORTED_FILES__: exposed.get('__WORK_REVIEW_DEMO_EXPORTED_FILES__'),
  };
  const fakeSetTimeout = (callback: () => void, milliseconds = 0) => {
    timerDelays.push(Number(milliseconds));
    timerTurn += 1;
    queueMicrotask(callback);
    return 0;
  };
  new Function('window', 'setTimeout', source)(windowObject, fakeSetTimeout);
  return {
    state,
    windowObject,
    timerDelays,
    getTimerTurn: () => timerTurn,
  };
}


test('浏览器初始化脚本序列化后不依赖 tsx 的 __name 辅助函数并暴露只读调用日志', async () => {
  let source = '';
  const exposedFunctions: string[] = [];
  const context = {
    exposeFunction: async (name: string) => { exposedFunctions.push(name); },
    addInitScript: async (script: unknown) => {
      source = typeof script === 'function'
        ? String(script)
        : String((script as { content?: unknown }).content ?? '');
    },
  };

  await installDemoTauriMock(
    context as unknown as Parameters<typeof installDemoTauriMock>[0],
    createDemoFixtures(),
  );

  assert.notEqual(source, '');
  assert.deepEqual(exposedFunctions, [
    '__WORK_REVIEW_DEMO_INVOKE__',
    '__WORK_REVIEW_DEMO_SHOT_INVOKES__',
    '__WORK_REVIEW_DEMO_EXPORTED_FILES__',
  ]);
  assert.doesNotMatch(source, /\b__name\b/);
  assert.doesNotThrow(() => new Function(source));
});

test('读取配置、统计、时间线、单活动详情和截图缩略图', async () => {
  const state = createState();
  const config = await handleDemoInvoke(state, 'get_config', {});
  const stats = await handleDemoInvoke(state, 'get_today_stats', {});
  const timeline = await handleDemoInvoke(state, 'get_timeline', {
    date: state.fixtures.date,
    limit: 100,
    offset: 0,
  });
  const activity = await handleDemoInvoke(state, 'get_activity', { id: 106 });
  const thumbnail = await handleDemoInvoke(state, 'get_screenshot_thumbnail', {
    path: state.fixtures.activities[0].screenshotPath,
  });

  assert.deepEqual((config as Record<string, unknown>).text_model_profiles, [
    state.fixtures.assistant.modelProfile,
  ]);
  assert.equal((stats as { total_duration: number }).total_duration, 19_800);
  assert.equal((timeline as Array<{ app_name: string }>).length, 6);
  assert.equal((activity as { window_title: string }).window_title, 'npm run verify:frontend');
  assert.match(String(thumbnail), /^[A-Za-z0-9+/]+=*$/);
});


test('截图 Mock 返回按活动区分的确定性脱敏工作卡片，不包含真实路径或真实应用内容', async () => {
  const state = createState();
  const fictionalApps = [
    'Nebula IDE',
    'Atlas Browser',
    'Paperwork Docs',
    'Orbit Meet',
    'Canvas Lab',
    'Nova Console',
  ];
  const previews: string[] = [];

  for (const [index, activity] of state.fixtures.activities.entries()) {
    const first = String(await handleDemoInvoke(state, 'get_screenshot_thumbnail', {
      path: activity.screenshotPath,
    }));
    const second = String(await handleDemoInvoke(state, 'get_screenshot_thumbnail', {
      path: activity.screenshotPath,
    }));
    const full = String(await handleDemoInvoke(state, 'get_screenshot_full', {
      path: activity.screenshotPath,
    }));
    assert.equal(first, second);
    assert.equal(first, full);

    const { width, height, bytes } = readPngSize(first);
    assert.ok(width >= 640 && height >= 360, `${activity.id} 的预览尺寸必须明显大于 1×1`);
    const metadata = readPngInternationalText(bytes).join('\n');
    assert.match(metadata, /已脱敏/);
    assert.match(metadata, /Aurora Board/);
    assert.match(metadata, new RegExp(fictionalApps[index]));
    assert.doesNotMatch(metadata, /Cursor|Terminal|Figma|docs\.example\.test|npm run|\/Users\/|\/tmp\/|Work_Review/);
    previews.push(first);
  }

  assert.equal(new Set(previews).size, state.fixtures.activities.length);
});

test('应用图标命令返回空值以触发安全的内置回退图标', async () => {
  const state = createState();

  assert.equal(
    await handleDemoInvoke(state, 'get_app_icon', {
      appName: 'Cursor',
      executablePath: null,
    }),
    '',
  );
  assert.deepEqual(state.unhandledCommands, []);
});

test('系统权限检查返回固定且全部可用的 macOS 演示状态', async () => {
  const state = createState();

  assert.deepEqual(await handleDemoInvoke(state, 'check_permissions', {}), {
    screen_capture: true,
    accessibility: true,
    input_monitoring: true,
    screenshot_supported: true,
    avatar_input_supported: true,
    all_granted: true,
    platform: 'macos',
  });
  assert.deepEqual(state.unhandledCommands, []);
});

test('About 页面可读取并保存隔离的更新设置', async () => {
  const state = createState();

  const settings = await handleDemoInvoke(state, 'get_update_settings', {});
  assert.deepEqual(settings, {
    autoCheck: true,
    lastCheckTime: 0,
    checkIntervalHours: 24,
  });

  await handleDemoInvoke(state, 'save_update_settings', {
    settings: {
      autoCheck: false,
      lastCheckTime: 0,
      checkIntervalHours: 24,
    },
  });

  assert.equal(await handleDemoInvoke(state, 'should_check_updates', {}), false);
});

test('日报生成前为空，生成后可读取并按段落保存修改', async () => {
  const state = createState();

  assert.equal(
    await handleDemoInvoke(state, 'get_saved_report', {
      date: state.fixtures.date,
      locale: 'zh-CN',
    }),
    null,
  );

  const generationStartedAt = performance.now();
  const generation = handleDemoInvoke(state, 'generate_report', {
    date: state.fixtures.date,
    force: true,
    locale: 'zh-CN',
  });
  await sleep(100);
  assert.equal(state.reportGenerated, false, '生成短延迟期间应保持骨架状态');
  await generation;
  const generationElapsed = performance.now() - generationStartedAt;
  assert.ok(generationElapsed >= 600, `生成延迟过短：${generationElapsed.toFixed(0)}ms`);
  assert.ok(generationElapsed < 2_000, `生成延迟过长：${generationElapsed.toFixed(0)}ms`);
  const generated = await handleDemoInvoke(state, 'get_saved_report', {
    date: state.fixtures.date,
    locale: 'zh-CN',
  });
  assert.equal(state.reportGenerated, true);
  assert.equal((generated as { content: string }).content, state.fixtures.report.content);

  const edited = `${state.fixtures.report.content}\n\n已显式保存段落。`;
  await handleDemoInvoke(state, 'update_report_content', {
    date: state.fixtures.date,
    locale: 'zh-CN',
    content: edited,
  });
  assert.equal(state.reportContent, edited);
});

test('保存隐私三档和截图设置，只记录安全目录打开与虚拟 Markdown 导出', async () => {
  const state = createState();
  const config = await handleDemoInvoke(state, 'get_config', {}) as Record<string, any>;
  config.privacy.app_rules = [
    { app_name: 'Cursor', level: 'full' },
    { app_name: '浏览器', level: 'anonymized' },
    { app_name: '会议', level: 'ignored' },
  ];
  config.storage.screenshots_enabled = false;

  await handleDemoInvoke(state, 'save_config', { config });
  await handleDemoInvoke(state, 'open_data_dir', {});
  const exportPath = await handleDemoInvoke(state, 'export_report_markdown', {
    date: state.fixtures.date,
    content: state.fixtures.report.content,
    exportDir: `${state.fixtures.safeRoot}exports/`,
  });

  assert.deepEqual(state.privacyRules, config.privacy.app_rules);
  assert.equal(state.screenshotsEnabled, false);
  assert.deepEqual(state.openedDirectories, [state.fixtures.dataDir]);
  assert.equal(exportPath, state.fixtures.exportPath);
  assert.equal(state.exportedFiles[state.fixtures.exportPath], state.fixtures.report.content);
});

test('capture 层可读取 UI 实际导出的结构化 Markdown 快照且不能污染 Mock state', async () => {
  const { state, windowObject } = await installBrowserHarness();
  const exportedContent = '# Aurora Board 日报\n\n- 已脱敏的虚构工作成果。';

  const exportPath = await windowObject.__TAURI_INTERNALS__.invoke('export_report_markdown', {
    date: state.fixtures.date,
    content: exportedContent,
    exportDir: `${state.fixtures.safeRoot}exports/`,
  });
  const firstSnapshot = await windowObject.__WORK_REVIEW_DEMO_EXPORTED_FILES__();

  assert.equal(exportPath, state.fixtures.exportPath);
  assert.deepEqual(firstSnapshot, [
    {
      path: state.fixtures.exportPath,
      content: exportedContent,
      kind: 'markdown',
    },
  ]);
  assert.doesNotMatch(JSON.stringify(firstSnapshot), /\/Users\/|\.worktrees\/|product-demo-impl/);

  firstSnapshot[0].content = '不应写回 Mock state';
  assert.deepEqual(await windowObject.__WORK_REVIEW_DEMO_EXPORTED_FILES__(), [
    {
      path: state.fixtures.exportPath,
      content: exportedContent,
      kind: 'markdown',
    },
  ]);
});

test('助手基础模板和演示 AI 复用同一会话，并保留第一轮完整历史', async () => {
  const state = createState();
  const conversationId = await handleDemoInvoke(state, 'create_assistant_conversation', {
    title: state.fixtures.assistant.basicQuestion,
  });

  const basic = await handleDemoInvoke(state, 'chat_work_assistant', {
    question: state.fixtures.assistant.basicQuestion,
    history: [],
    modelConfig: null,
    requestId: 'request-basic',
    onEvent: { id: 41 },
  }) as { answer: string; usedAi: boolean; modelName: string | null };
  await persistRound(state, Number(conversationId), state.fixtures.assistant.basicQuestion, basic);

  const history = [
    { role: 'user', content: state.fixtures.assistant.basicQuestion },
    { role: 'assistant', content: state.fixtures.assistant.basicAnswer },
  ];
  const ai = await handleDemoInvoke(state, 'chat_work_assistant', {
    question: state.fixtures.assistant.aiQuestion,
    history,
    modelConfig: state.fixtures.assistant.modelProfile.model_config,
    requestId: 'request-ai',
    onEvent: '__CHANNEL__:42',
  }) as { answer: string; usedAi: boolean; modelName: string | null };
  await persistRound(state, Number(conversationId), state.fixtures.assistant.aiQuestion, ai);

  assert.equal(state.conversationCreateCalls, 1);
  assert.equal(state.chatCalls, 2);
  assert.equal(state.appendCalls, 4);
  assert.equal(state.assistantMessages.every((message) => message.conversationId === conversationId), true);
  assert.deepEqual(state.chatRequests[1].history, history);
  assert.equal(basic.answer, state.fixtures.assistant.basicAnswer);
  assert.equal(basic.usedAi, false);
  assert.equal(ai.answer, state.fixtures.assistant.aiAnswer);
  assert.equal(ai.usedAi, true);
  assert.equal(ai.modelName, state.fixtures.assistant.modelProfile.name);
});

test('演示 AI 接受真实 buildHistoryPayload 追加的工具摘要', async () => {
  const state = createState();
  const history = buildHistoryPayload([
    { role: 'user', content: state.fixtures.assistant.basicQuestion },
    {
      role: 'assistant',
      content: state.fixtures.assistant.basicAnswer,
      streaming: false,
      steps: [
        {
          tool: 'get_today_stats',
          label: '汇总今日统计',
          status: 'done',
          ok: true,
          hits: 0,
          digest: '已汇总今天的工作统计',
        },
      ],
    },
  ]);

  assert.match(history[1].content, /\[工具：get_today_stats✓\]/);
  await assert.doesNotReject(handleDemoInvoke(state, 'chat_work_assistant', {
    question: state.fixtures.assistant.aiQuestion,
    history,
    modelConfig: state.fixtures.assistant.modelProfile.model_config,
    requestId: 'request-ai-real-history',
    onEvent: { id: 76 },
  }));
});

test('演示 AI Channel 返回与 Aurora Board 实现、文档和前端验证一致的非零引用', async () => {
  const state = createState();

  const answer = await handleDemoInvoke(state, 'chat_work_assistant', {
    question: state.fixtures.assistant.aiQuestion,
    history: [
      { role: 'user', content: state.fixtures.assistant.basicQuestion },
      { role: 'assistant', content: state.fixtures.assistant.basicAnswer },
    ],
    modelConfig: state.fixtures.assistant.modelProfile.model_config,
    requestId: 'request-ai',
    onEvent: { id: 77 },
  }) as { references: Array<Record<string, unknown>> };

  const events = takePendingChannelEvents(state, 77);
  assert.deepEqual(events.slice(0, 2).map((event) => event.message?.type), [
    'stepStart',
    'stepResult',
  ]);
  const stepResult = events[1].message ?? {};
  const references = stepResult.references as Array<Record<string, unknown>>;
  assert.equal(stepResult.tool, 'query_activities');
  assert.equal(stepResult.ok, true);
  assert.equal(stepResult.hits, 3);
  assert.deepEqual(references.map((reference) => reference.sourceId), [101, 103, 106]);
  assert.deepEqual(references.map((reference) => reference.timestamp), [
    state.fixtures.activities[0].timestamp,
    state.fixtures.activities[2].timestamp,
    state.fixtures.activities[5].timestamp,
  ]);
  assert.match(String(references[0].title), /Aurora Board.*导出流程/);
  assert.match(String(references[1].title), /Aurora Board.*发布检查清单/);
  assert.match(String(references[2].title), /16:40.*前端验证/);
  assert.match(String(stepResult.digest), /3 条.*实现.*文档.*16:40.*前端验证/);
  assert.deepEqual(answer.references, references);
  assert.deepEqual(events.at(-2)?.message?.references, references);

  const streamedAnswer = events
    .filter((event) => event.message?.type === 'token')
    .map((event) => String(event.message?.token ?? ''))
    .join('');
  assert.equal(streamedAnswer, state.fixtures.assistant.aiAnswer);
  assert.equal(events.at(-2)?.message?.type, 'done');
  assert.equal(events.at(-1)?.end, true);
  assert.deepEqual(events.map((event) => event.index), events.map((_, index) => index));
});

test('浏览器 Channel 通过确定性有限延迟分批派发，现有 Tauri Channel 可逐批消费', async () => {
  const harness = await installBrowserHarness();
  const previousWindow = Reflect.get(globalThis, 'window');
  Reflect.set(globalThis, 'window', harness.windowObject);
  try {
    const received: Array<{ event: Record<string, unknown>; timerTurn: number }> = [];
    const channel = new Channel<Record<string, unknown>>();
    channel.onmessage = (event) => received.push({
      event,
      timerTurn: harness.getTimerTurn(),
    });

    await harness.windowObject.__TAURI_INTERNALS__.invoke('chat_work_assistant', {
      question: harness.state.fixtures.assistant.aiQuestion,
      history: [
        { role: 'user', content: harness.state.fixtures.assistant.basicQuestion },
        { role: 'assistant', content: harness.state.fixtures.assistant.basicAnswer },
      ],
      modelConfig: harness.state.fixtures.assistant.modelProfile.model_config,
      requestId: 'request-ai-stream',
      onEvent: channel,
    });

    const tokens = received.filter(({ event }) => event.type === 'token');
    assert.ok(tokens.length >= 4);
    assert.ok(new Set(tokens.map(({ timerTurn }) => timerTurn)).size >= 4);
    assert.ok(harness.timerDelays.length >= tokens.length);
    assert.ok(harness.timerDelays.every((delay) => delay > 0 && delay <= 250));
    const totalScheduledDelay = harness.timerDelays.reduce((total, delay) => total + delay, 0);
    assert.ok(totalScheduledDelay >= 600 && totalScheduledDelay <= 1_500);
    assert.equal(received.at(-1)?.event.type, 'done');
  } finally {
    if (previousWindow === undefined) Reflect.deleteProperty(globalThis, 'window');
    else Reflect.set(globalThis, 'window', previousWindow);
  }
});

test('关于页读取固定演示版本号', async () => {
  const state = createState();
  assert.equal(await handleDemoInvoke(state, 'plugin:app|version', {}), '1.1.1-demo');
});

test('动态问题生成固定 JSON，未知命令明确抛错', async () => {
  const state = createState();
  assert.equal(
    await handleDemoInvoke(state, 'generate_text_with_model', {
      modelConfig: state.fixtures.assistant.modelProfile.model_config,
      prompt: '生成问题',
    }),
    '["今天最值得总结的成果是什么？"]',
  );

  await assert.rejects(
    handleDemoInvoke(state, 'unknown_demo_command', {}),
    /unknown_demo_command/,
  );
  assert.deepEqual(state.unhandledCommands, ['unknown_demo_command']);
});

async function persistRound(
  state: ReturnType<typeof createState>,
  conversationId: number,
  question: string,
  answer: { answer: string; modelName: string | null },
): Promise<void> {
  await handleDemoInvoke(state, 'append_assistant_message', {
    conversationId,
    role: 'user',
    content: question,
    toolDigest: null,
    modelName: null,
  });
  await handleDemoInvoke(state, 'append_assistant_message', {
    conversationId,
    role: 'assistant',
    content: answer.answer,
    toolDigest: null,
    modelName: answer.modelName,
  });
}
