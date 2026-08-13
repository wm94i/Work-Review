import assert from 'node:assert/strict';

import type { DemoFixtures, ShotContext, ShotRunner, StoryboardSceneId } from './types.ts';

export const ASSISTANT_STORAGE_KEY = 'work-review-assistant-state';
const ASSISTANT_INVOKE_READER = '__WORK_REVIEW_DEMO_SHOT_INVOKES__';

export interface AssistantInvokeObservation {
  command: string;
  args: Record<string, unknown>;
}

interface AssistantStorageState {
  messages: never[];
  selectedModelId: '__basic__';
  hasUserSelectedModel: true;
  sending: false;
  sendingRequestId: null;
  conversationId: null;
}

interface AssistantInvokeExpectation {
  chats: number;
  appends: number;
}

export function buildAssistantStorageState(): AssistantStorageState {
  return {
    messages: [],
    selectedModelId: '__basic__',
    hasUserSelectedModel: true,
    sending: false,
    sendingRequestId: null,
    conversationId: null,
  };
}

export function buildShotRouteUrl(currentUrl: string, route: string): string {
  let url: URL;
  try {
    url = new URL(currentUrl);
  } catch {
    throw new Error(`产品演示分镜只能从 HTTP(S) 页面跳转：${currentUrl}`);
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`产品演示分镜只能从 HTTP(S) 页面跳转：${currentUrl}`);
  }
  const normalizedRoute = route.startsWith('/') ? route : `/${route}`;
  url.hash = `#${normalizedRoute}`;
  return url.href;
}

function relevantAssistantInvokes(
  observations: readonly AssistantInvokeObservation[],
): AssistantInvokeObservation[] {
  const relevant = new Set([
    'create_assistant_conversation',
    'chat_work_assistant',
    'append_assistant_message',
  ]);
  return observations.filter((entry) => relevant.has(entry.command));
}

function asHistory(value: unknown): Array<{ role?: unknown; content?: unknown }> {
  return Array.isArray(value)
    ? value.filter((item): item is { role?: unknown; content?: unknown } => (
        typeof item === 'object' && item !== null
      ))
    : [];
}

export function assertAssistantInvokeContract(
  observations: readonly AssistantInvokeObservation[],
  fixtures: DemoFixtures,
): void {
  const relevant = relevantAssistantInvokes(observations);
  const creates = relevant.filter((entry) => entry.command === 'create_assistant_conversation');
  const chats = relevant.filter((entry) => entry.command === 'chat_work_assistant');
  const appends = relevant.filter((entry) => entry.command === 'append_assistant_message');

  assert.equal(creates.length, 1, '助手演示必须且只能创建一次会话');
  assert.equal(chats.length, 2, '助手演示必须完成基础模板与演示 AI 两轮聊天');
  assert.equal(appends.length, 4, '助手演示必须持久化两轮 user + assistant 共四条消息');

  assert.equal(chats[0]?.args.question, fixtures.assistant.basicQuestion, '第一轮必须使用基础模板问题');
  assert.equal(chats[0]?.args.modelConfig, null, '第一轮必须使用基础模板而不是外部 AI');
  assert.equal(chats[1]?.args.question, fixtures.assistant.aiQuestion, '第二轮必须使用演示 AI 问题');
  assert.deepEqual(
    chats[1]?.args.modelConfig,
    fixtures.assistant.modelProfile.model_config,
    '第二轮必须使用隔离的演示 AI 配置',
  );

  const conversationIds = appends.map((entry) => entry.args.conversationId);
  assert.ok(
    conversationIds.every((id) => id === fixtures.assistant.conversationId),
    '两轮助手消息必须写入同一会话',
  );

  const expectedMessages = [
    ['user', fixtures.assistant.basicQuestion],
    ['assistant', fixtures.assistant.basicAnswer],
    ['user', fixtures.assistant.aiQuestion],
    ['assistant', fixtures.assistant.aiAnswer],
  ] as const;
  for (const [index, [role, content]] of expectedMessages.entries()) {
    assert.equal(appends[index]?.args.role, role, `第 ${index + 1} 条持久化消息角色错误`);
    assert.equal(appends[index]?.args.content, content, `第 ${index + 1} 条持久化消息内容不完整`);
  }

  const secondHistory = asHistory(chats[1]?.args.history);
  const completeFirstRound = secondHistory.length >= 2
    && secondHistory[0]?.role === 'user'
    && secondHistory[0]?.content === fixtures.assistant.basicQuestion
    && secondHistory[1]?.role === 'assistant'
    && typeof secondHistory[1]?.content === 'string'
    && secondHistory[1].content.startsWith(fixtures.assistant.basicAnswer);
  assert.ok(completeFirstRound, '第二轮演示 AI 请求必须携带第一轮完整历史');
}

async function waitForInvokeCounts(
  page: ShotContext['page'],
  expectation: AssistantInvokeExpectation,
): Promise<void> {
  await page.waitForFunction(
    async ({ readerName, chats, appends }) => {
      const reader = (window as unknown as Record<string, unknown>)[readerName];
      if (typeof reader !== 'function') return false;
      const observations = await (reader as () => Promise<AssistantInvokeObservation[]>)();
      return observations.filter((entry) => entry.command === 'chat_work_assistant').length >= chats
        && observations.filter((entry) => entry.command === 'append_assistant_message').length >= appends;
    },
    { readerName: ASSISTANT_INVOKE_READER, ...expectation },
    { timeout: 10_000 },
  );
}

async function readAssistantInvokes(page: ShotContext['page']): Promise<AssistantInvokeObservation[]> {
  const result = await page.evaluate(
    async (readerName) => {
      const reader = (window as unknown as Record<string, unknown>)[readerName];
      if (typeof reader !== 'function') return [];
      return (reader as () => Promise<AssistantInvokeObservation[]>)();
    },
    ASSISTANT_INVOKE_READER,
  );
  return Array.isArray(result) ? result as AssistantInvokeObservation[] : [];
}

type DemoShotContext = ShotContext & {
  captureActionFrame?: () => Promise<void>;
  markContentStart?: () => void;
};

interface DemoPackagingContent {
  id: 'work-review-demo-hook' | 'work-review-demo-outro';
  kind: 'hook' | 'outro';
  eyebrow: string;
  title: string;
  subtitle: string;
  callToAction: string;
  boundary: string;
  ariaLabel: string;
}

async function captureActionFrameIfAvailable(context: ShotContext): Promise<void> {
  const captureActionFrame = (context as DemoShotContext).captureActionFrame;
  if (captureActionFrame) await captureActionFrame();
}

function markContentStartIfAvailable(context: ShotContext): void {
  (context as DemoShotContext).markContentStart?.();
}

async function showDemoPackaging(
  page: ShotContext['page'],
  content: DemoPackagingContent,
): Promise<void> {
  await page.evaluate((payload) => {
    const existing = document.getElementById(payload.id);
    existing?.remove();

    const styleId = `${payload.id}-style`;
    document.getElementById(styleId)?.remove();
    const style = document.createElement('style');
    style.id = styleId;
    style.textContent = `
      #${payload.id} {
        position: fixed;
        inset: 0;
        z-index: 2147483647;
        overflow: hidden;
        box-sizing: border-box;
        display: grid;
        place-items: center;
        font-family: Inter, "PingFang SC", "Microsoft YaHei", sans-serif;
        color: #f8fafc;
        background:
          radial-gradient(circle at 18% 14%, rgba(56, 189, 248, 0.20), transparent 34%),
          radial-gradient(circle at 82% 84%, rgba(129, 140, 248, 0.18), transparent 36%),
          linear-gradient(145deg, #07111f 0%, #0b1728 48%, #111827 100%);
      }
      #${payload.id} * { box-sizing: border-box; }
      #${payload.id} .wr-demo-shell {
        position: relative;
        width: min(88vw, 1260px);
        min-height: min(78vh, 780px);
        display: grid;
        align-content: center;
        justify-items: center;
        gap: 24px;
      }
      #${payload.id} .wr-demo-window {
        width: min(82vw, 1080px);
        overflow: hidden;
        border: 1px solid rgba(148, 163, 184, 0.28);
        border-radius: 26px;
        background: rgba(15, 23, 42, 0.82);
        box-shadow: 0 40px 100px rgba(0, 0, 0, 0.48);
        backdrop-filter: blur(20px);
      }
      #${payload.id} .wr-demo-window-bar {
        height: 52px;
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 0 22px;
        color: #94a3b8;
        border-bottom: 1px solid rgba(148, 163, 184, 0.18);
        font-size: 14px;
        letter-spacing: 0.04em;
      }
      #${payload.id} .wr-demo-dots { display: flex; gap: 8px; }
      #${payload.id} .wr-demo-dot {
        width: 10px;
        height: 10px;
        border-radius: 999px;
        background: #475569;
      }
      #${payload.id} .wr-demo-paper {
        position: relative;
        width: min(72vw, 850px);
        min-height: 430px;
        margin: 56px auto 64px;
        padding: 58px 64px;
        border: 1px solid rgba(226, 232, 240, 0.18);
        border-radius: 18px;
        background: linear-gradient(160deg, rgba(30, 41, 59, 0.96), rgba(15, 23, 42, 0.94));
      }
      #${payload.id} .wr-demo-eyebrow {
        display: inline-flex;
        align-items: center;
        min-height: 30px;
        padding: 5px 12px;
        border: 1px solid rgba(125, 211, 252, 0.35);
        border-radius: 999px;
        color: #bae6fd;
        background: rgba(14, 116, 144, 0.18);
        font-size: 13px;
        font-weight: 700;
        letter-spacing: 0.08em;
      }
      #${payload.id} .wr-demo-title {
        max-width: 980px;
        margin: 22px 0 0;
        font-size: clamp(42px, 5vw, 78px);
        line-height: 1.08;
        letter-spacing: -0.035em;
        text-align: center;
      }
      #${payload.id} .wr-demo-hook-title {
        margin-top: 28px;
        color: #f8fafc;
        font-size: clamp(38px, 4.4vw, 66px);
        line-height: 1.12;
        letter-spacing: -0.025em;
      }
      #${payload.id} .wr-demo-subtitle {
        max-width: 900px;
        margin: 0;
        color: #cbd5e1;
        font-size: clamp(20px, 2vw, 30px);
        line-height: 1.55;
        text-align: center;
      }
      #${payload.id} .wr-demo-empty-line {
        display: flex;
        align-items: center;
        height: 76px;
        margin-top: 42px;
        border-bottom: 1px solid rgba(148, 163, 184, 0.30);
      }
      #${payload.id} .wr-demo-caret {
        width: 3px;
        height: 38px;
        border-radius: 999px;
        background: #38bdf8;
        box-shadow: 0 0 18px rgba(56, 189, 248, 0.75);
      }
      #${payload.id} .wr-demo-logo {
        width: 112px;
        height: 112px;
        display: grid;
        place-items: center;
        border: 1px solid rgba(125, 211, 252, 0.42);
        border-radius: 28px;
        color: #e0f2fe;
        background: linear-gradient(145deg, rgba(14, 165, 233, 0.32), rgba(79, 70, 229, 0.34));
        box-shadow: 0 24px 70px rgba(14, 165, 233, 0.22);
        font-size: 48px;
        font-weight: 800;
        letter-spacing: -0.08em;
      }
      #${payload.id} .wr-demo-cta {
        margin: 4px 0 0;
        padding: 14px 24px;
        border-radius: 999px;
        color: #082f49;
        background: #e0f2fe;
        font-size: clamp(17px, 1.5vw, 23px);
        font-weight: 800;
        text-align: center;
      }
      #${payload.id} .wr-demo-boundary {
        max-width: 940px;
        margin: 4px 0 0;
        color: #94a3b8;
        font-size: clamp(14px, 1.2vw, 18px);
        line-height: 1.55;
        text-align: center;
      }
      @media (max-aspect-ratio: 3/4) {
        #${payload.id} { padding: 7vh 6vw; }
        #${payload.id} .wr-demo-shell {
          width: 100%;
          min-height: 82vh;
          gap: 30px;
        }
        #${payload.id} .wr-demo-window { width: 100%; }
        #${payload.id} .wr-demo-paper {
          width: calc(100% - 44px);
          min-height: 760px;
          margin: 42px 22px 48px;
          padding: 72px 44px;
        }
        #${payload.id} .wr-demo-title { font-size: clamp(52px, 9vw, 82px); }
        #${payload.id} .wr-demo-hook-title { font-size: clamp(46px, 8vw, 72px); }
        #${payload.id} .wr-demo-subtitle { font-size: clamp(25px, 4.2vw, 36px); }
        #${payload.id} .wr-demo-logo { width: 132px; height: 132px; font-size: 56px; }
        #${payload.id} .wr-demo-cta { font-size: clamp(21px, 3.5vw, 30px); }
        #${payload.id} .wr-demo-boundary { font-size: clamp(17px, 2.8vw, 23px); }
      }
    `;

    const layer = document.createElement('section');
    layer.id = payload.id;
    layer.dataset.demoPackaging = payload.kind;
    layer.setAttribute('role', 'region');
    layer.setAttribute('aria-label', payload.ariaLabel);

    const shell = document.createElement('div');
    shell.className = 'wr-demo-shell';

    if (payload.kind === 'hook') {
      const windowFrame = document.createElement('div');
      windowFrame.className = 'wr-demo-window';
      const windowBar = document.createElement('div');
      windowBar.className = 'wr-demo-window-bar';
      const dots = document.createElement('div');
      dots.className = 'wr-demo-dots';
      for (let index = 0; index < 3; index += 1) {
        const dot = document.createElement('span');
        dot.className = 'wr-demo-dot';
        dots.append(dot);
      }
      const windowTitle = document.createElement('span');
      windowTitle.textContent = '23:47 · 今日日报';
      windowBar.append(dots, windowTitle);

      const paper = document.createElement('div');
      paper.className = 'wr-demo-paper';
      const eyebrow = document.createElement('span');
      eyebrow.className = 'wr-demo-eyebrow';
      eyebrow.textContent = payload.eyebrow;
      const title = document.createElement('h1');
      title.className = 'wr-demo-hook-title';
      title.textContent = payload.title;
      const subtitle = document.createElement('p');
      subtitle.className = 'wr-demo-subtitle';
      subtitle.textContent = payload.subtitle;
      paper.append(eyebrow, title, subtitle);
      const emptyLine = document.createElement('div');
      emptyLine.className = 'wr-demo-empty-line';
      const caret = document.createElement('span');
      caret.className = 'wr-demo-caret';
      caret.setAttribute('aria-label', '输入光标');
      emptyLine.append(caret);
      paper.append(emptyLine);
      windowFrame.append(windowBar, paper);
      shell.append(windowFrame);
    } else {
      const sourceLogo = document.querySelector<HTMLImageElement>('main img[alt="Work Review"]');
      const logo = sourceLogo
        ? sourceLogo.cloneNode(true) as HTMLImageElement
        : document.createElement('div');
      logo.className = 'wr-demo-logo';
      logo.setAttribute('role', 'img');
      logo.setAttribute('aria-label', 'Work Review Logo');
      if (logo instanceof HTMLImageElement) {
        logo.removeAttribute('width');
        logo.removeAttribute('height');
        logo.style.objectFit = 'contain';
        logo.style.padding = '16px';
      } else {
        logo.textContent = 'W';
      }
      const eyebrow = document.createElement('span');
      eyebrow.className = 'wr-demo-eyebrow';
      eyebrow.textContent = payload.eyebrow;
      const title = document.createElement('h1');
      title.className = 'wr-demo-title';
      title.textContent = payload.title;
      const subtitle = document.createElement('p');
      subtitle.className = 'wr-demo-subtitle';
      subtitle.textContent = payload.subtitle;
      const callToAction = document.createElement('p');
      callToAction.className = 'wr-demo-cta';
      callToAction.textContent = payload.callToAction;
      const boundary = document.createElement('p');
      boundary.className = 'wr-demo-boundary';
      boundary.textContent = payload.boundary;
      shell.append(logo, eyebrow, title, subtitle, callToAction, boundary);
    }

    layer.append(shell);
    document.head.append(style);
    document.body.append(layer);
  }, content);
  await page.locator(`#${content.id}`).waitFor();
}

async function hideDemoPackaging(
  page: ShotContext['page'],
  id: DemoPackagingContent['id'],
): Promise<void> {
  await page.evaluate((targetId) => {
    document.getElementById(targetId)?.remove();
    document.getElementById(`${targetId}-style`)?.remove();
  }, id);
  await page.locator(`#${id}`).waitFor({ state: 'hidden' });
}

async function showAssistantModelSelection(
  page: ShotContext['page'],
  label: '基础模板 · 已选中' | '演示 AI · 已选中',
  mode: 'basic' | 'ai',
): Promise<void> {
  await page.evaluate(({ text, selectedMode }) => {
    const id = 'work-review-demo-model-selection';
    const existing = document.getElementById(id);
    existing?.remove();
    const selector = document.querySelector<HTMLSelectElement>('select[aria-label="选择助手模型"]');
    if (!selector) throw new Error('助手模型选择器不存在');
    const badge = document.createElement('span');
    badge.id = id;
    badge.dataset.selectedModel = selectedMode;
    badge.setAttribute('role', 'status');
    badge.textContent = text;
    Object.assign(badge.style, {
      display: 'inline-flex',
      alignItems: 'center',
      minHeight: '28px',
      marginLeft: '10px',
      padding: '4px 10px',
      border: `1px solid ${selectedMode === 'ai' ? '#818cf8' : '#38bdf8'}`,
      borderRadius: '999px',
      color: selectedMode === 'ai' ? '#c7d2fe' : '#bae6fd',
      background: selectedMode === 'ai' ? 'rgba(79, 70, 229, 0.22)' : 'rgba(14, 116, 144, 0.20)',
      fontSize: '12px',
      fontWeight: '700',
      whiteSpace: 'nowrap',
    });
    selector.parentElement?.insertAdjacentElement('afterend', badge);
    selector.style.outline = `2px solid ${selectedMode === 'ai' ? '#818cf8' : '#38bdf8'}`;
    selector.style.outlineOffset = '3px';
  }, { text: label, selectedMode: mode });
  await page.getByText(label, { exact: true }).waitFor();
}

async function runHook(context: ShotContext): Promise<void> {
  const { page } = context;
  await page.getByRole('heading', { name: '日报', exact: true }).waitFor();
  await page.getByText('今日暂无日报', { exact: true }).waitFor();
  await page.getByRole('button', { name: /生成今日日报/ }).waitFor();

  await showDemoPackaging(page, {
    id: 'work-review-demo-hook',
    kind: 'hook',
    eyebrow: '空白日报',
    title: '今天做了什么？',
    subtitle: '下班时，你还记得今天到底做了什么吗？',
    callToAction: '',
    boundary: '',
    ariaLabel: '深色夜间桌面、空白日报与输入光标',
  });
  markContentStartIfAvailable(context);
  await page.waitForTimeout(4_500);
  await hideDemoPackaging(page, 'work-review-demo-hook');
  await page.waitForTimeout(1_200);
}

async function runTimeline(context: ShotContext): Promise<void> {
  const { page, fixtures } = context;
  const activity = page
    .locator('button.timeline-entry')
    .filter({ hasText: 'Aurora Board · 导出流程' });
  await activity.waitFor();
  markContentStartIfAvailable(context);
  await page.waitForTimeout(900);
  await activity.click();

  const dialog = page.getByRole('dialog', { name: 'Cursor' });
  await dialog.waitFor();
  await dialog.getByText(fixtures.activities[0].title, { exact: true }).waitFor();
  await page.waitForTimeout(700);
  await captureActionFrameIfAvailable(context);
  await page.waitForTimeout(1_800);
  await dialog.getByRole('button', { name: '关闭', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  await page.waitForTimeout(700);
}

async function generateReportIfEmpty(page: ShotContext['page']): Promise<void> {
  const empty = page.getByText('今日暂无日报', { exact: true });
  if (await empty.isVisible()) {
    await page.getByRole('button', { name: /生成今日日报/ }).click();
    await page.locator('.report-section').filter({ hasText: '今日概览' }).waitFor();
  }
}

async function runReport(context: ShotContext): Promise<void> {
  const { page, fixtures } = context;
  await page.getByRole('heading', { name: '日报', exact: true }).waitFor();
  markContentStartIfAvailable(context);
  await generateReportIfEmpty(page);
  const section = page.locator('.report-section').filter({ hasText: '今日概览' });
  await section.waitFor();
  await page.waitForTimeout(900);
  await section.hover();
  await section.getByTitle('编辑').click();

  const dialog = page.getByRole('dialog', { name: '编辑' });
  await dialog.waitFor();
  const editor = dialog.locator('textarea');
  const updatedContent = `## ${fixtures.report.sections[0].title}\n\n`
    + `${fixtures.report.sections[0].markdown} 已显式保存段落。`;
  await editor.fill(updatedContent);
  const save = dialog.getByRole('button', { name: '保存', exact: true });
  await save.waitFor();
  await page.waitForTimeout(700);
  await captureActionFrameIfAvailable(context);
  await page.waitForTimeout(800);
  await save.click();
  await dialog.waitFor({ state: 'hidden' });
  await page
    .locator('.report-section')
    .filter({ hasText: fixtures.report.sections[0].title })
    .filter({ hasText: '已显式保存段落。' })
    .waitFor();
  await page.waitForTimeout(900);
}

async function chooseAssistantModel(
  page: ShotContext['page'],
  modelSelector: ReturnType<ShotContext['page']['getByRole']>,
  target: '__basic__' | 'demo-ai',
): Promise<void> {
  await modelSelector.focus();
  await modelSelector.click();
  await page.waitForTimeout(450);
  await modelSelector.selectOption(target);
  await page.waitForTimeout(250);
  assert.equal(await modelSelector.inputValue(), target, `助手模型必须真实选中 ${target}`);
  await showAssistantModelSelection(
    page,
    target === '__basic__' ? '基础模板 · 已选中' : '演示 AI · 已选中',
    target === '__basic__' ? 'basic' : 'ai',
  );
}

async function runAssistant(context: ShotContext): Promise<void> {
  const { page, scene, fixtures } = context;
  await page.evaluate(
    ({ key, state }) => window.localStorage.setItem(key, JSON.stringify(state)),
    { key: ASSISTANT_STORAGE_KEY, state: buildAssistantStorageState() },
  );
  await page.goto(buildShotRouteUrl(page.url(), scene.route), { waitUntil: 'networkidle' });

  const modelSelector = page.getByRole('combobox', { name: '选择助手模型' });
  const composer = page.getByRole('textbox', { name: /问点什么/ });
  const send = page.getByRole('button', { name: '发送消息' });
  await modelSelector.waitFor();
  await composer.waitFor();
  await send.waitFor();
  markContentStartIfAvailable(context);
  await chooseAssistantModel(page, modelSelector, '__basic__');
  await page.waitForTimeout(700);

  await composer.fill(fixtures.assistant.basicQuestion);
  await send.click();
  await page.getByText(fixtures.assistant.basicAnswer, { exact: true }).waitFor();
  await waitForInvokeCounts(page, { chats: 1, appends: 2 });
  await page.waitForTimeout(1_500);

  await chooseAssistantModel(page, modelSelector, 'demo-ai');
  await page.waitForTimeout(650);
  await composer.fill(fixtures.assistant.aiQuestion);
  await send.click();
  await page.getByText(fixtures.assistant.aiAnswer, { exact: true }).waitFor();
  await waitForInvokeCounts(page, { chats: 2, appends: 4 });
  await page.waitForTimeout(1_500);

  assertAssistantInvokeContract(await readAssistantInvokes(page), fixtures);
}

async function runPrivacy(context: ShotContext): Promise<void> {
  const { page } = context;
  const privacyTab = page.getByRole('button', { name: '隐私', exact: true });
  await privacyTab.waitFor();
  markContentStartIfAvailable(context);
  await privacyTab.click();
  await page.getByRole('button', { name: /^\+?\s*添加规则$/ }).click();
  await page.locator('#app-name-input').fill('Aurora Notes');

  const strategyGroup = page.getByRole('radiogroup', { name: '记录策略', exact: true });
  const full = strategyGroup.getByRole('radio', { name: '完全记录', exact: true });
  const anonymized = strategyGroup.getByRole('radio', { name: '仅统计时长', exact: true });
  const ignored = strategyGroup.getByRole('radio', { name: '完全忽略', exact: true });
  await full.waitFor();
  await anonymized.waitFor();
  await ignored.waitFor();
  await full.click();
  assert.equal(await full.getAttribute('aria-checked'), 'true', '隐私镜头开始时必须选中完全记录');
  await page.waitForTimeout(700);
  await captureActionFrameIfAvailable(context);
  await page.waitForTimeout(400);
  await anonymized.click();
  assert.equal(await anonymized.getAttribute('aria-checked'), 'true', '隐私镜头必须选择仅统计时长');
  await page.getByRole('button', { name: '添加规则', exact: true }).click();
  await page.locator('.settings-privacy-rule-row')
    .filter({ hasText: 'Aurora Notes' })
    .filter({ hasText: '仅统计时长' })
    .waitFor();
  await page.waitForTimeout(900);

  await page.getByRole('button', { name: '存储', exact: true }).click();
  const screenshotSwitch = page.getByRole('switch', { name: '启用截图', exact: true });
  await screenshotSwitch.waitFor();
  assert.equal(await screenshotSwitch.getAttribute('aria-checked'), 'true', '隐私镜头开始时截图必须开启');
  assert.equal(await page.getByRole('switch', { name: /OCR|AI/ }).count(), 0, '不得伪造独立 OCR 或 AI 开关');
  await page.waitForTimeout(1_000);
  await screenshotSwitch.click();
  assert.equal(await screenshotSwitch.getAttribute('aria-checked'), 'false', '隐私镜头必须真实关闭截图');
  await page.getByRole('button', { name: '保存设置', exact: true }).click();
  await page.getByText('设置已保存', { exact: true }).waitFor();
  await page.waitForTimeout(1_200);
}

async function runExport(context: ShotContext): Promise<void> {
  const { page, fixtures } = context;
  const storageTab = page.getByRole('button', { name: '存储', exact: true });
  await storageTab.waitFor();
  markContentStartIfAvailable(context);
  await storageTab.click();
  const currentDataDir = page
    .getByText('当前目录', { exact: true })
    .locator('..')
    .getByText(fixtures.dataDir, { exact: true });
  await currentDataDir.waitFor();
  await page.getByRole('button', { name: '打开当前目录', exact: true }).click();
  await page.waitForTimeout(900);

  await page.getByRole('link', { name: '日报', exact: true }).click();
  await page.getByRole('heading', { name: '日报', exact: true }).waitFor();
  await generateReportIfEmpty(page);
  await page.waitForTimeout(900);
  await page.getByRole('button', { name: '导出', exact: true }).click();
  await page.getByRole('menuitem', { name: /导出当日 Markdown/ }).click();
  await page.getByText(
    `日报已导出到 ${fixtures.exportPath}`,
    { exact: true },
  ).waitFor();
  await page.waitForTimeout(1_200);
}

async function runOutro(context: ShotContext): Promise<void> {
  const { page } = context;
  const main = page.getByRole('main');
  await main.getByRole('heading', { name: 'Work Review', exact: true }).waitFor();
  await main.getByRole('img', { name: 'Work Review', exact: true }).waitFor();
  await showDemoPackaging(page, {
    id: 'work-review-demo-outro',
    kind: 'outro',
    eyebrow: 'WORK REVIEW',
    title: 'Work Review',
    subtitle: '回看今天，写下成果。',
    callToAction: '用一个真实工作日，找回你的第一份日报。',
    boundary: '活动记录和截图默认保存在本机；外部 AI 与远程存储按配置使用。',
    ariaLabel: 'Work Review 产品片尾',
  });
  markContentStartIfAvailable(context);
  await page.waitForTimeout(1_200);
}

export const SHOT_RUNNERS: Record<StoryboardSceneId, ShotRunner> = {
  hook: runHook,
  timeline: runTimeline,
  report: runReport,
  assistant: runAssistant,
  privacy: runPrivacy,
  export: runExport,
  outro: runOutro,
};
