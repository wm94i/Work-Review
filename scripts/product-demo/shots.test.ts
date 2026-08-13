import assert from 'node:assert/strict';
import test from 'node:test';

import type { Locator, Page } from 'playwright';

import { createDemoFixtures } from './fixtures.ts';
import {
  ASSISTANT_STORAGE_KEY,
  SHOT_RUNNERS,
  assertAssistantInvokeContract,
  buildAssistantStorageState,
  buildShotRouteUrl,
  type AssistantInvokeObservation,
} from './shots.ts';
import { STORYBOARD } from './storyboard.ts';
import type { StoryboardSceneId } from './types.ts';

interface Interaction {
  action: string;
  target: string;
  value?: unknown;
}

class FakeLocator {
  constructor(
    private readonly owner: FakePage,
    private readonly target: string,
  ) {}

  filter(options: { hasText?: string | RegExp }): FakeLocator {
    return new FakeLocator(this.owner, `${this.target} hasText=${String(options.hasText ?? '')}`);
  }

  locator(selector: string): FakeLocator {
    return new FakeLocator(this.owner, `${this.target} >> ${selector}`);
  }

  getByRole(role: string, options: { name?: string | RegExp; exact?: boolean } = {}): FakeLocator {
    return new FakeLocator(this.owner, `${this.target} >> role=${role} name=${String(options.name ?? '')}`);
  }

  getByText(text: string | RegExp, options: { exact?: boolean } = {}): FakeLocator {
    return new FakeLocator(this.owner, `${this.target} >> text=${String(text)} exact=${String(options.exact ?? false)}`);
  }

  getByTitle(title: string | RegExp): FakeLocator {
    return new FakeLocator(this.owner, `${this.target} >> title=${String(title)}`);
  }

  async waitFor(options: { state?: string } = {}): Promise<void> {
    this.owner.interactions.push({ action: `wait:${options.state ?? 'visible'}`, target: this.target });
    if (this.target.includes('.settings-privacy-rule-row')) {
      const expectedApp = this.target.includes('hasText=Aurora Notes');
      const expectedLevel = this.target.includes('hasText=仅统计时长');
      const matches = this.owner.submittedPrivacyRules.some((rule) => (
        (!expectedApp || rule.appName === 'Aurora Notes')
        && (!expectedLevel || rule.level === 'anonymized')
      ));
      if (!matches) throw new Error('未找到已提交的隐私规则');
    }
  }

  async isVisible(): Promise<boolean> {
    this.owner.interactions.push({ action: 'isVisible', target: this.target });
    return true;
  }

  async click(): Promise<void> {
    this.owner.interactions.push({ action: 'click', target: this.target });
    if (this.target.includes('启用截图')) this.owner.screenshotsEnabled = !this.owner.screenshotsEnabled;
    if (this.target.includes('role=radio name=完全记录')) this.owner.selectedPrivacyLevel = 'full';
    if (this.target.includes('role=radio name=仅统计时长')) this.owner.selectedPrivacyLevel = 'anonymized';
    if (this.target.includes('role=radio name=完全忽略')) this.owner.selectedPrivacyLevel = 'ignored';
    if (this.target === 'role=button name=添加规则' && this.owner.privacyAppName) {
      this.owner.submittedPrivacyRules.push({
        appName: this.owner.privacyAppName,
        level: this.owner.selectedPrivacyLevel,
      });
    }
  }

  async hover(): Promise<void> {
    this.owner.interactions.push({ action: 'hover', target: this.target });
  }

  async fill(value: string): Promise<void> {
    this.owner.interactions.push({ action: 'fill', target: this.target, value });
    if (this.target.includes('#app-name-input')) this.owner.privacyAppName = value;
  }

  async focus(): Promise<void> {
    this.owner.interactions.push({ action: 'focus', target: this.target });
  }

  async press(key: string): Promise<void> {
    this.owner.interactions.push({ action: 'press', target: this.target, value: key });
    if (!this.target.includes('选择助手模型')) return;
    if (key === 'Home') this.owner.selectedAssistantModel = '__basic__';
    if (key === 'ArrowDown') this.owner.selectedAssistantModel = 'demo-ai';
  }

  async selectOption(value: string): Promise<string[]> {
    this.owner.interactions.push({ action: 'selectOption', target: this.target, value });
    if (this.target.includes('选择助手模型') && (value === '__basic__' || value === 'demo-ai')) {
      this.owner.selectedAssistantModel = value;
    }
    return [value];
  }

  async inputValue(): Promise<string> {
    this.owner.interactions.push({ action: 'inputValue', target: this.target });
    if (this.target.includes('选择助手模型')) return this.owner.selectedAssistantModel;
    return '';
  }

  async getAttribute(name: string): Promise<string | null> {
    this.owner.interactions.push({ action: `getAttribute:${name}`, target: this.target });
    if (name === 'aria-checked' && this.target.includes('启用截图')) {
      return String(this.owner.screenshotsEnabled);
    }
    if (name === 'aria-checked' && this.target.includes('完全记录')) {
      return String(this.owner.selectedPrivacyLevel === 'full');
    }
    if (name === 'aria-checked' && this.target.includes('仅统计时长')) {
      return String(this.owner.selectedPrivacyLevel === 'anonymized');
    }
    if (name === 'aria-checked' && this.target.includes('完全忽略')) {
      return String(this.owner.selectedPrivacyLevel === 'ignored');
    }
    return null;
  }

  async count(): Promise<number> {
    this.owner.interactions.push({ action: 'count', target: this.target });
    return /role=switch name=.*(OCR|AI)/.test(this.target)
      ? this.owner.forbiddenPrivacySwitchCount
      : 1;
  }
}

class FakePage {
  readonly interactions: Interaction[] = [];
  readonly serializedBrowserFunctions: string[] = [];
  screenshotsEnabled = true;
  selectedAssistantModel = '__basic__';
  selectedPrivacyLevel: 'full' | 'anonymized' | 'ignored' = 'ignored';
  privacyAppName = '';
  submittedPrivacyRules: Array<{
    appName: string;
    level: 'full' | 'anonymized' | 'ignored';
  }> = [];
  currentUrl = 'http://127.0.0.1:5173/#/overview';

  constructor(
    readonly assistantInvokeLog: AssistantInvokeObservation[],
    readonly forbiddenPrivacySwitchCount = 0,
  ) {}

  url(): string {
    return this.currentUrl;
  }

  async goto(url: string): Promise<null> {
    this.currentUrl = url;
    this.interactions.push({ action: 'goto', target: url });
    return null;
  }

  getByRole(role: string, options: { name?: string | RegExp; exact?: boolean } = {}): FakeLocator {
    return new FakeLocator(this, `role=${role} name=${String(options.name ?? '')}`);
  }

  getByText(text: string | RegExp, options: { exact?: boolean } = {}): FakeLocator {
    return new FakeLocator(this, `text=${String(text)} exact=${String(options.exact ?? false)}`);
  }

  getByTitle(title: string | RegExp): FakeLocator {
    return new FakeLocator(this, `title=${String(title)}`);
  }

  locator(selector: string): FakeLocator {
    return new FakeLocator(this, `selector=${selector}`);
  }

  async evaluate(pageFunction: unknown, arg?: unknown): Promise<unknown> {
    this.serializedBrowserFunctions.push(String(pageFunction));
    this.interactions.push({ action: 'evaluate', target: 'page', value: arg });
    if (arg === '__WORK_REVIEW_DEMO_SHOT_INVOKES__') return structuredClone(this.assistantInvokeLog);
    return null;
  }

  async waitForFunction(pageFunction: unknown): Promise<void> {
    this.serializedBrowserFunctions.push(String(pageFunction));
    this.interactions.push({ action: 'waitForFunction', target: 'assistant invoke log' });
  }

  async waitForTimeout(milliseconds: number): Promise<void> {
    this.interactions.push({ action: 'waitForTimeout', target: String(milliseconds) });
  }
}

function validAssistantLog(): AssistantInvokeObservation[] {
  const fixtures = createDemoFixtures();
  const conversationId = fixtures.assistant.conversationId;
  const firstHistory = [
    { role: 'user', content: fixtures.assistant.basicQuestion },
    { role: 'assistant', content: fixtures.assistant.basicAnswer },
  ];
  return [
    { command: 'create_assistant_conversation', args: { title: fixtures.assistant.basicQuestion } },
    {
      command: 'chat_work_assistant',
      args: { question: fixtures.assistant.basicQuestion, history: [], modelConfig: null },
    },
    {
      command: 'append_assistant_message',
      args: { conversationId, role: 'user', content: fixtures.assistant.basicQuestion },
    },
    {
      command: 'append_assistant_message',
      args: { conversationId, role: 'assistant', content: fixtures.assistant.basicAnswer },
    },
    {
      command: 'chat_work_assistant',
      args: {
        question: fixtures.assistant.aiQuestion,
        history: firstHistory,
        modelConfig: fixtures.assistant.modelProfile.model_config,
      },
    },
    {
      command: 'append_assistant_message',
      args: { conversationId, role: 'user', content: fixtures.assistant.aiQuestion },
    },
    {
      command: 'append_assistant_message',
      args: { conversationId, role: 'assistant', content: fixtures.assistant.aiAnswer },
    },
  ];
}

function scene(id: StoryboardSceneId) {
  const found = STORYBOARD.find((item) => item.id === id);
  assert.ok(found, `缺少 ${id} 分镜`);
  return found;
}

function asPage(page: FakePage): Page {
  return page as unknown as Page;
}

function joinedInteractions(page: FakePage): string {
  return page.interactions
    .map((item) => {
      const value = item.value && typeof item.value === 'object'
        ? JSON.stringify(item.value)
        : String(item.value ?? '');
      return `${item.action} ${item.target} ${value}`;
    })
    .join('\n');
}

function interactionIndex(
  page: FakePage,
  predicate: (interaction: Interaction) => boolean,
): number {
  return page.interactions.findIndex(predicate);
}

function assertInteractionOrder(
  page: FakePage,
  before: (interaction: Interaction) => boolean,
  after: (interaction: Interaction) => boolean,
  message: string,
): void {
  const beforeIndex = interactionIndex(page, before);
  const afterIndex = interactionIndex(page, after);
  assert.ok(beforeIndex >= 0 && afterIndex >= 0 && beforeIndex < afterIndex, message);
}

test('七个分镜 runner 完整注册且不增加产品不存在的能力', () => {
  assert.deepEqual(Object.keys(SHOT_RUNNERS), [
    'hook',
    'timeline',
    'report',
    'assistant',
    'privacy',
    'export',
    'outro',
  ]);
});

test('分镜路由保留当前同源地址并使用 hash 路由', () => {
  assert.equal(
    buildShotRouteUrl('http://127.0.0.1:5173/#/overview', '/timeline?date=2026-08-12'),
    'http://127.0.0.1:5173/#/timeline?date=2026-08-12',
  );
  assert.throws(() => buildShotRouteUrl('about:blank', '/report'), /HTTP/);
});

test('助手进入页面前固定为基础模板且保留用户主动选择标记', () => {
  assert.equal(ASSISTANT_STORAGE_KEY, 'work-review-assistant-state');
  assert.deepEqual(buildAssistantStorageState(), {
    messages: [],
    selectedModelId: '__basic__',
    hasUserSelectedModel: true,
    sending: false,
    sendingRequestId: null,
    conversationId: null,
  });
});

test('助手调用契约要求同一会话、两轮聊天、四次持久化和完整第一轮历史', () => {
  const fixtures = createDemoFixtures();
  assert.doesNotThrow(() => assertAssistantInvokeContract(validAssistantLog(), fixtures));

  const broken = validAssistantLog();
  const secondChat = [...broken].reverse().find((entry) => entry.command === 'chat_work_assistant');
  assert.ok(secondChat);
  secondChat.args.history = [];
  assert.throws(
    () => assertAssistantInvokeContract(broken, fixtures),
    /完整历史/,
  );
});

test('七镜头使用真实可见语义选择器、锁定包装和可审计动作帧', async () => {
  const fixtures = createDemoFixtures();
  const pages = new Map<StoryboardSceneId, FakePage>();

  for (const id of Object.keys(SHOT_RUNNERS) as StoryboardSceneId[]) {
    const page = new FakePage(validAssistantLog());
    pages.set(id, page);
    const context = {
      page: asPage(page),
      aspect: '16x9' as const,
      scene: scene(id),
      fixtures,
      captureActionFrame: async () => {
        page.interactions.push({ action: 'captureActionFrame', target: id });
      },
      markContentStart: () => {
        page.interactions.push({ action: 'markContentStart', target: id });
      },
    };
    await SHOT_RUNNERS[id](context);
  }

  for (const id of Object.keys(SHOT_RUNNERS) as StoryboardSceneId[]) {
    assert.equal(
      pages.get(id)!.interactions.filter((item) => item.action === 'markContentStart').length,
      1,
      `${id} 必须且只能标记一次有效成片起点`,
    );
  }

  const hookPage = pages.get('hook')!;
  assertInteractionOrder(
    hookPage,
    (item) => item.action === 'wait:visible' && item.target === 'selector=#work-review-demo-hook',
    (item) => item.action === 'markContentStart',
    '片头必须在夜间包装层稳定可见后标记成片起点',
  );
  assertInteractionOrder(
    hookPage,
    (item) => item.action === 'markContentStart',
    (item) => item.action === 'wait:hidden' && item.target === 'selector=#work-review-demo-hook',
    '片头必须先标记包装层，再切回产品主界面',
  );

  const hook = joinedInteractions(pages.get('hook')!);
  assert.match(hook, /"kind":"hook"/);
  assert.match(hook, /今天做了什么？/);
  assert.match(hook, /深色夜间桌面/);
  assert.match(hook, /空白日报/);
  assert.match(hook, /输入光标/);
  assert.match(hook, /selector=#work-review-demo-hook/);
  assert.match(hook, /wait:hidden selector=#work-review-demo-hook/);
  assert.match(hook, /role=heading name=日报/);
  assert.match(hook, /text=今日暂无日报/);
  assert.match(hook, /role=button name=.*生成今日日报/);

  const timelinePage = pages.get('timeline')!;
  const timeline = joinedInteractions(timelinePage);
  assertInteractionOrder(
    timelinePage,
    (item) => item.action === 'wait:visible' && item.target.includes('button.timeline-entry'),
    (item) => item.action === 'markContentStart',
    '时间线必须在聚合活动就绪后标记成片起点',
  );
  assertInteractionOrder(
    timelinePage,
    (item) => item.action === 'markContentStart',
    (item) => item.action === 'click' && item.target.includes('button.timeline-entry'),
    '时间线必须在打开详情前标记成片起点',
  );
  assert.match(timeline, /selector=button\.timeline-entry hasText=Aurora Board · 导出流程/);
  assert.match(timeline, /role=dialog name=Cursor/);
  assert.match(timeline, /role=dialog name=Cursor >> text=Aurora Board · 导出流程 exact=true/);
  assert.doesNotMatch(timeline, /export_report_markdown · Aurora Board/);
  assert.equal(timelinePage.interactions.filter((item) => item.action === 'captureActionFrame').length, 1);
  const timelineCapture = timelinePage.interactions.findIndex((item) => item.action === 'captureActionFrame');
  const timelineClose = timelinePage.interactions.findIndex(
    (item) => item.action === 'click' && item.target.includes('role=button name=关闭'),
  );
  assert.ok(timelineCapture >= 0 && timelineCapture < timelineClose, '时间线动作帧必须在详情弹窗关闭前捕获');
  assert.match(timeline, /wait:hidden/);

  const reportPage = pages.get('report')!;
  const report = joinedInteractions(reportPage);
  assertInteractionOrder(
    reportPage,
    (item) => item.action === 'wait:visible' && item.target === 'role=heading name=日报',
    (item) => item.action === 'markContentStart',
    '日报必须在页面标题就绪后标记成片起点',
  );
  assertInteractionOrder(
    reportPage,
    (item) => item.action === 'markContentStart',
    (item) => item.action === 'isVisible' && item.target.includes('今日暂无日报'),
    '日报必须在生成或编辑交互前标记成片起点',
  );
  assert.match(report, /role=button name=.*生成今日日报/);
  assert.match(report, /selector=\.report-section hasText=今日概览/);
  assert.match(report, /title=编辑/);
  assert.match(report, /role=dialog name=编辑/);
  assert.match(report, /fill .*textarea .*## 今日概览/);
  assert.match(report, /fill .*textarea [\s\S]*已显式保存段落/);
  assert.match(report, /wait:visible role=dialog name=编辑 >> role=button name=保存/);
  assert.equal(reportPage.interactions.filter((item) => item.action === 'captureActionFrame').length, 1);
  const reportCapture = reportPage.interactions.findIndex((item) => item.action === 'captureActionFrame');
  const reportSave = reportPage.interactions.findIndex(
    (item) => item.action === 'click' && item.target.includes('role=dialog name=编辑 >> role=button name=保存'),
  );
  assert.ok(reportCapture >= 0 && reportCapture < reportSave, '日报动作帧必须在编辑弹窗保存前捕获');
  assert.match(
    report,
    /selector=\.report-section hasText=今日概览 hasText=已显式保存段落/,
  );

  const assistantPage = pages.get('assistant')!;
  const assistant = joinedInteractions(assistantPage);
  assertInteractionOrder(
    assistantPage,
    (item) => item.action === 'wait:visible' && item.target === 'role=button name=发送消息',
    (item) => item.action === 'markContentStart',
    '助手必须在模型、输入框和发送按钮就绪后标记成片起点',
  );
  assertInteractionOrder(
    assistantPage,
    (item) => item.action === 'markContentStart',
    (item) => item.action === 'focus' && item.target.includes('选择助手模型'),
    '助手必须在模型交互前标记成片起点',
  );
  assert.match(assistant, /选择助手模型/);
  assert.match(assistant, /focus role=combobox name=选择助手模型/);
  assert.match(assistant, /selectOption role=combobox name=选择助手模型 __basic__/);
  assert.match(assistant, /inputValue role=combobox name=选择助手模型/);
  assert.match(assistant, /基础模板 · 已选中/);
  assert.match(assistant, /selectOption role=combobox name=选择助手模型 demo-ai/);
  assert.match(assistant, /演示 AI · 已选中/);
  assert.doesNotMatch(assistant, /press role=combobox name=选择助手模型 (Home|ArrowDown|End|Enter)/);
  assert.match(assistant, /问点什么/);
  assert.match(assistant, new RegExp(fixtures.assistant.basicQuestion));
  assert.match(assistant, new RegExp(fixtures.assistant.basicAnswer));
  assert.match(assistant, new RegExp(fixtures.assistant.aiQuestion));
  assert.match(assistant, new RegExp(fixtures.assistant.aiAnswer));

  const privacyPage = pages.get('privacy')!;
  const privacy = joinedInteractions(privacyPage);
  assertInteractionOrder(
    privacyPage,
    (item) => item.action === 'wait:visible' && item.target === 'role=button name=隐私',
    (item) => item.action === 'markContentStart',
    '隐私镜头必须在设置导航就绪后标记成片起点',
  );
  assertInteractionOrder(
    privacyPage,
    (item) => item.action === 'markContentStart',
    (item) => item.action === 'click' && item.target === 'role=button name=隐私',
    '隐私镜头必须在打开隐私页前标记成片起点',
  );
  assert.match(privacy, /role=button name=隐私/);
  assert.ok(
    privacyPage.interactions.some((item) => (
      item.action === 'click'
      && item.target === 'role=button name=/^\\+?\\s*添加规则$/'
    )),
    '隐私规则编辑器必须使用锚定名称打开，避免与提交按钮混淆',
  );
  assert.match(privacy, /fill selector=#app-name-input Aurora Notes/);
  assert.match(privacy, /role=radiogroup name=记录策略/);
  assert.match(privacy, /role=radiogroup name=记录策略 >> role=radio name=完全记录/);
  assert.match(privacy, /role=radiogroup name=记录策略 >> role=radio name=仅统计时长/);
  assert.match(privacy, /role=radiogroup name=记录策略 >> role=radio name=完全忽略/);
  const privacyFull = privacyPage.interactions.findIndex(
    (item) => item.action === 'click' && item.target.includes('role=radio name=完全记录'),
  );
  assert.equal(privacyPage.interactions.filter((item) => item.action === 'captureActionFrame').length, 1);
  const privacyCapture = privacyPage.interactions.findIndex((item) => item.action === 'captureActionFrame');
  const privacyAnonymized = privacyPage.interactions.findIndex(
    (item) => item.action === 'click' && item.target.includes('role=radio name=仅统计时长'),
  );
  assert.ok(
    privacyFull >= 0 && privacyFull < privacyCapture && privacyCapture < privacyAnonymized,
    '隐私镜头必须先真实选择完全记录，展示三档后再切换为仅统计时长',
  );
  assert.deepEqual(privacyPage.submittedPrivacyRules, [
    { appName: 'Aurora Notes', level: 'anonymized' },
  ]);
  assert.match(
    privacy,
    /wait:visible selector=\.settings-privacy-rule-row hasText=Aurora Notes hasText=仅统计时长/,
  );
  assert.match(privacy, /role=button name=存储/);
  assert.match(privacy, /role=switch name=启用截图/);
  assert.match(privacy, /getAttribute:aria-checked .*启用截图/);
  assert.equal(
    privacyPage.interactions.filter((item) => (
      item.action === 'click' && item.target.includes('role=switch name=启用截图')
    )).length,
    1,
    '截图开关必须且只能切换一次',
  );
  assert.equal(privacyPage.screenshotsEnabled, false, '隐私镜头结束时截图必须保持关闭');
  const screenshotReads = privacyPage.interactions
    .map((item, index) => ({ item, index }))
    .filter(({ item }) => (
      item.action === 'getAttribute:aria-checked'
      && item.target.includes('role=switch name=启用截图')
    ));
  const screenshotClick = privacyPage.interactions.findIndex((item) => (
    item.action === 'click' && item.target.includes('role=switch name=启用截图')
  ));
  const privacySave = privacyPage.interactions.findIndex((item) => (
    item.action === 'click' && item.target === 'role=button name=保存设置'
  ));
  assert.equal(screenshotReads.length, 2, '截图开关必须在切换前后各读取一次真实状态');
  assert.ok(
    screenshotReads[0]!.index < screenshotClick
      && screenshotClick < screenshotReads[1]!.index
      && screenshotReads[1]!.index < privacySave,
    '截图关闭状态必须在保存设置前得到确认',
  );
  assert.match(privacy, /role=button name=保存设置/);
  assert.match(privacy, /text=设置已保存/);

  const exportPage = pages.get('export')!;
  const exportShot = joinedInteractions(exportPage);
  assertInteractionOrder(
    exportPage,
    (item) => item.action === 'wait:visible' && item.target === 'role=button name=存储',
    (item) => item.action === 'markContentStart',
    '导出镜头必须在设置导航就绪后标记成片起点',
  );
  assertInteractionOrder(
    exportPage,
    (item) => item.action === 'markContentStart',
    (item) => item.action === 'click' && item.target === 'role=button name=存储',
    '导出镜头必须在打开存储页前标记成片起点',
  );
  assert.ok(
    exportShot.includes(
      `text=当前目录 exact=true >> .. >> text=${fixtures.dataDir} exact=true`,
    ),
    '数据目录等待必须限定在“当前目录”字段内',
  );
  assert.match(exportShot, /role=button name=打开当前目录/);
  assert.match(exportShot, /role=button name=导出/);
  assert.match(exportShot, /role=menuitem name=.*导出当日 Markdown/);
  assert.match(exportShot, new RegExp(fixtures.exportPath.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));

  const outroPage = pages.get('outro')!;
  const outro = joinedInteractions(outroPage);
  assertInteractionOrder(
    outroPage,
    (item) => item.action === 'wait:visible' && item.target === 'selector=#work-review-demo-outro',
    (item) => item.action === 'markContentStart',
    '片尾必须在专用包装层稳定可见后标记成片起点',
  );
  assert.match(outro, /"kind":"outro"/);
  assert.match(outro, /Work Review/);
  assert.match(outro, /回看今天，写下成果。/);
  assert.match(outro, /用一个真实工作日，找回你的第一份日报。/);
  assert.match(outro, /活动记录和截图默认保存在本机；外部 AI 与远程存储按配置使用。/);
  assert.match(outro, /selector=#work-review-demo-outro/);

  for (const id of ['hook', 'assistant', 'export', 'outro'] as const) {
    assert.equal(
      pages.get(id)!.interactions.filter((item) => item.action === 'captureActionFrame').length,
      0,
      `${id} 不应额外占用关键动作帧回调`,
    );
  }

  const all = [...pages.values()].map(joinedInteractions).join('\n');
  assert.doesNotMatch(all, /Shift|Control|Meta|drag|多选|框选|OCR 开关|关闭所有 AI|迁移目录/);

  const serializedBrowserFunctions = [...pages.values()]
    .flatMap((page) => page.serializedBrowserFunctions)
    .join('\n');
  assert.doesNotMatch(
    serializedBrowserFunctions,
    /\b__name\b/,
    '分镜浏览器回调序列化后不能依赖 tsx/esbuild 的 __name 辅助函数',
  );
  assert.match(
    serializedBrowserFunctions,
    /layer\.append\(shell\)/,
    '片头与片尾包装层必须把已构造的可见内容挂载到遮罩层',
  );
});

test('隐私镜头发现独立 OCR 或 AI 开关时必须阻断录制', async () => {
  const fixtures = createDemoFixtures();
  const page = new FakePage(validAssistantLog(), 1);

  await assert.rejects(
    SHOT_RUNNERS.privacy({
      page: asPage(page),
      aspect: '16x9',
      scene: scene('privacy'),
      fixtures,
      captureActionFrame: async () => undefined,
      markContentStart: () => undefined,
    }),
    /不得伪造独立 OCR 或 AI 开关/,
  );
});
