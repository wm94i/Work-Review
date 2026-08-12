import assert from 'node:assert/strict';
import test from 'node:test';

import { buildHistoryPayload } from '../../src/routes/ask/historyPayload.ts';
import { createDemoFixtures } from './fixtures.ts';
import {
  createDemoMockState,
  handleDemoInvoke,
  installDemoTauriMock,
  takePendingChannelEvents,
} from './tauriMock.ts';

const createState = () => createDemoMockState(createDemoFixtures());


test('浏览器初始化脚本序列化后不依赖 tsx 的 __name 辅助函数', async () => {
  let source = '';
  const context = {
    exposeFunction: async () => {},
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

test('日报生成前为空，生成后可读取并按段落保存修改', async () => {
  const state = createState();

  assert.equal(
    await handleDemoInvoke(state, 'get_saved_report', {
      date: state.fixtures.date,
      locale: 'zh-CN',
    }),
    null,
  );

  await handleDemoInvoke(state, 'generate_report', {
    date: state.fixtures.date,
    force: true,
    locale: 'zh-CN',
  });
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

test('助手 Channel 按步骤、令牌、完成、结束顺序发送，活动查询不虚构引用', async () => {
  const state = createState();

  await handleDemoInvoke(state, 'chat_work_assistant', {
    question: state.fixtures.assistant.aiQuestion,
    history: [
      { role: 'user', content: state.fixtures.assistant.basicQuestion },
      { role: 'assistant', content: state.fixtures.assistant.basicAnswer },
    ],
    modelConfig: state.fixtures.assistant.modelProfile.model_config,
    requestId: 'request-ai',
    onEvent: { id: 77 },
  });

  const events = takePendingChannelEvents(state, 77);
  assert.deepEqual(events.slice(0, 2).map((event) => event.message?.type), [
    'stepStart',
    'stepResult',
  ]);
  assert.deepEqual(events[1].message, {
    type: 'stepResult',
    requestId: 'request-ai',
    tool: 'query_activities',
    ok: true,
    hits: 0,
    references: [],
    digest: '已检查今天的活动记录',
  });
  const streamedAnswer = events
    .filter((event) => event.message?.type === 'token')
    .map((event) => String(event.message?.token ?? ''))
    .join('');
  assert.equal(streamedAnswer, state.fixtures.assistant.aiAnswer);
  assert.equal(events.at(-2)?.message?.type, 'done');
  assert.equal(events.at(-1)?.end, true);
  assert.deepEqual(events.map((event) => event.index), events.map((_, index) => index));
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
