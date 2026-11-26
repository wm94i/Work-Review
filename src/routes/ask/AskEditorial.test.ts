import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const [source, appCssSource, sidebarSource] = await Promise.all([
  readFile(new URL('./Ask.svelte', import.meta.url), 'utf8'),
  readFile(new URL('../../app.css', import.meta.url), 'utf8'),
  readFile(new URL('../../lib/components/Sidebar.svelte', import.meta.url), 'utf8'),
]);

function countMatches(value: string, pattern: RegExp): number {
  return value.match(pattern)?.length ?? 0;
}

test('助手左上角应复用其他页面的统一页头结构', () => {
  const header = source.match(/<div class="page-header page-axis-operation">[\s\S]*?<\/div>\s*<div bind:this=\{chatBody\}/)?.[0] ?? '';

  assert.ok(header, '应渲染统一 page-header');
  assert.match(header, /page-title-group/);
  assert.match(header, /page-title-badge/);
  assert.match(header, /page-title-copy/);
  assert.match(header, /t\('sidebar\.nav\.ask'\)/);
  assert.match(header, /currentConversationTitle/);
  assert.match(header, /ask-header-history/);
  assert.match(header, /ask-header-new/);
  const sharedAssistantIcon = /d="M8 10h8M8 14h4m-6 6h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z"/;
  assert.match(header, sharedAssistantIcon);
  assert.match(sidebarSource, sharedAssistantIcon);
  assert.doesNotMatch(source, /ask-conversation-bar/);
  assert.doesNotMatch(source, /ask-context-strip/);
});

test('历史会话与新对话入口应位于统一页头，而不是输入器内部', () => {
  const header = source.match(/<div class="page-header page-axis-operation">[\s\S]*?<\/div>\s*<div bind:this=\{chatBody\}/)?.[0] ?? '';
  const composer = source.match(/<div class="ask-composer-shell[\s\S]*?<\/div>\s*<\/div>/)?.[0] ?? '';

  assert.match(header, /ask-header-history/);
  assert.match(header, /ask-header-new/);
  assert.doesNotMatch(composer, /ask-header-history|ask-header-new/);
});

test('欢迎态应使用短标题和短说明，并保持四条快捷问题', () => {
  const welcome = source.match(/<section class="[^"]*ask-welcome-panel[^"]*"[\s\S]*?<\/section>/)?.[0] ?? '';

  assert.ok(welcome, '应保留欢迎态容器');
  assert.match(welcome, /ask-welcome-product-mark/);
  assert.match(welcome, /src="\/icons\/128x128\.png" alt=""/);
  assert.doesNotMatch(welcome, /ask-welcome-mark/);
  assert.match(welcome, /t\('ask\.welcomeTitle'\)/);
  assert.match(welcome, /t\('ask\.welcomeBrief'\)/);
  assert.match(welcome, /ask-starter-grid/);
  assert.match(welcome, /starterPrompts\.slice\(0, 4\)/);
  assert.match(appCssSource, /\.ask-starter-card\s*\{[^}]*justify-content:\s*center[^}]*text-align:\s*center/s);
  assert.doesNotMatch(welcome, /ask-kicker|ask-starter-index/);
});

test('快捷问题应从扩展问题池随机抽取，并在关键操作后刷新', () => {
  assert.match(source, /import \{ selectStarterPrompts \} from '\.\/starterPromptPresentation\.ts';/);
  assert.match(source, /function refreshStarterPrompts\(/);
  assert.match(source, /tm\('ask\.starterPrompts'\)/);
  assert.match(source, /selectStarterPrompts\(\{/);
  assert.ok(countMatches(source, /refreshStarterPrompts\(/g) >= 4, '进入页面、新对话、切换模型和动态生成后都应刷新');
});

test('语言切换应刷新对应语言的动态问题，并避免首次挂载重复洗牌', () => {
  const localeBlock = source.match(/\$: if \(currentLocale && currentLocale !== starterPromptLocale\) \{[\s\S]*?\n  \}/)?.[0] ?? '';
  const mountBlock = source.match(/onMount\(async \(\) => \{[\s\S]*?\n  \}\);/)?.[0] ?? '';

  assert.match(localeBlock, /void refreshDynamicPrompts\(\)/);
  assert.match(mountBlock, /if \(starterPrompts\.length === 0\)/);
});

test('动态问题请求在读取统计后应再次校验请求是否仍有效', () => {
  const dynamicFunction = source.match(/async function refreshDynamicPrompts\(\) \{[\s\S]*?\n  \}/)?.[0] ?? '';
  const statsIndex = dynamicFunction.indexOf("'get_today_stats'");
  const generateIndex = dynamicFunction.indexOf("'generate_text_with_model'");
  const staleCheckIndex = dynamicFunction.indexOf('requestId !== starterPromptRequestId', statsIndex);

  assert.ok(statsIndex >= 0 && generateIndex > statsIndex, '应先读取统计，再调用模型');
  assert.ok(staleCheckIndex > statsIndex && staleCheckIndex < generateIndex, '模型调用前应丢弃已失效请求');
});

test('助手回答应直接排版在阅读画布上，不再使用整张回答卡', () => {
  assert.match(source, /assistant-markdown/);
  assert.match(source, /ask-message-row-assistant/);
  assert.doesNotMatch(source, /ask-answer-card/);
});

test('工具过程与高风险操作确认能力应继续保留', () => {
  assert.match(source, /ask-tool-summary/);
  assert.match(source, /respondConfirm\(message\.id, step, true\)/);
  assert.match(source, /respondConfirm\(message\.id, step, false\)/);
});

test('展开详情仅在原本跟随底部时继续追底，且两个入口均已绑定', () => {
  const handler = source.match(
    /function handleExpandScroll\(event(?:\s*:[^)]+)?\)[^{]*\{[\s\S]*?\n  \}/,
  )?.[0] ?? '';

  assert.ok(handler, '应存在详情展开滚动处理器');
  assert.match(handler, /if\s*\(\s*!details\.open\s*\|\|\s*!stickToBottom\s*\)\s*return;/);
  assert.doesNotMatch(handler, /stickToBottom\s*=\s*true/);
  assert.match(handler, /void scrollToBottom\('auto',\s*3\)/);
  assert.equal(countMatches(source, /on:toggle=\{handleExpandScroll\}/g), 2);
  assert.match(
    source,
    /class="ask-tool-summary[^"]*"[\s\S]{0,180}on:toggle=\{handleExpandScroll\}/,
  );
  assert.match(
    source,
    /class="ask-reference-trail"[^>]*on:toggle=\{handleExpandScroll\}/,
  );
});

test('输入器中的上下文说明入口应具备原生键盘语义和本地化名称', () => {
  const start = source.indexOf('<div class="ask-composer-shell');
  const end = source.indexOf('<p class="ask-composer-hint"', start);
  const composer = start >= 0 && end > start ? source.slice(start, end) : '';
  const context = composer.match(
    /<details class="ask-context-menu"[^>]*>\s*<summary\b[\s\S]*?<\/details>/,
  )?.[0] ?? '';

  assert.ok(context, '输入器内应保留轻量上下文说明入口');
  assert.match(
    context,
    /^<details[^>]*>\s*<summary[^>]*aria-label=\{t\('ask\.recordContext'\)\}/,
  );
  assert.match(context, /title=\{t\('ask\.recordContext'\)\}/);
  assert.match(context, /t\('ask\.contextScope'\)/);
  assert.match(context, /t\('ask\.contextSources'\)/);
  assert.match(context, /aria-hidden="true"/);
  assert.doesNotMatch(composer, /currentContextDate|ask\.workContext/);
  assert.doesNotMatch(composer, /\{currentModelLabel\}/);
});

test('引用应改为回答依据线，不再默认展示来源卡片网格', () => {
  assert.match(source, /ask-reference-trail/);
  assert.match(source, /t\('ask\.referenceTrail', \{ count: message\.references\.length \}\)/);
  assert.match(source, /aria-label=\{t\('ask\.referenceTrail', \{ count: message\.references\.length \}\)\}/);
  assert.doesNotMatch(source, /ask-reference-grid|ask-reference-card/);
});

test('真实模型标签应只在输入框内可见一次', () => {
  assert.match(source, /resolveModelOptionLabel/);
  assert.equal(countMatches(source, /\{currentModelLabel\}/g), 0);
  assert.doesNotMatch(source, /t\('ask\.contextModel'/);
});

test('模型选项应逐 profile 展示，不能复用当前选中标签', () => {
  const selector = source.match(/<select[\s\S]*?class="ask-model-select"[\s\S]*?<\/select>/)?.[0] ?? '';

  assert.ok(selector);
  assert.match(selector, /<option value=\{BASIC_ASSISTANT_MODEL_ID\}>[\s\S]*?t\('ask\.basicTemplate'\)[\s\S]*?<\/option>/);
  assert.match(selector, /displayModelProfileName\(profile\)/);
  assert.doesNotMatch(selector, /currentModelLabel/);
});

test('流式回答、错误状态和历史会话能力应继续保留', () => {
  assert.match(source, /aria-busy=\{message\.role === 'assistant'/);
  assert.match(source, /class="ask-error-callout" role="alert"/);
  assert.match(source, /ask-history-drawer/);
  assert.match(source, /ask-history-delete/);
});

test('欢迎态应减少空白、调整字体并固定为两列两行', () => {
  const markRule = appCssSource.match(/\.ask-welcome-product-mark\s*\{[^}]*\}/)?.[0] ?? '';
  const markImageRule = appCssSource.match(/\.ask-welcome-product-mark img\s*\{[^}]*\}/)?.[0] ?? '';
  const welcomeRule = appCssSource.match(/\.ask-welcome-panel\s*\{[^}]*\}/)?.[0] ?? '';
  const titleRule = appCssSource.match(/\.ask-welcome-copy h2\s*\{[^}]*\}/)?.[0] ?? '';
  const gridRule = appCssSource.match(/\.ask-starter-grid\s*\{[^}]*\}/)?.[0] ?? '';
  const cardRule = appCssSource.match(/\.ask-starter-card\s*\{[^}]*\}/)?.[0] ?? '';
  const mobileBlock = appCssSource.match(/@media \(max-width: 520px\) \{[\s\S]*?\n\}/)?.[0] ?? '';

  assert.match(markRule, /width:\s*3\.75rem/);
  assert.match(markImageRule, /object-fit:\s*contain/);
  assert.match(markImageRule, /filter:\s*drop-shadow/);
  assert.match(welcomeRule, /min-height:\s*clamp\((?:18|19|20)rem/);
  assert.match(welcomeRule, /padding:\s*clamp\((?:1\.5|2|2\.25)rem/);
  assert.match(titleRule, /font-family:\s*inherit/);
  assert.match(titleRule, /white-space:\s*nowrap/);
  assert.match(gridRule, /grid-template-columns:\s*repeat\(2, minmax\(0, 1fr\)\)/);
  assert.match(cardRule, /font-size:\s*(?:0\.78|0\.8|0\.82|0\.84)rem/);
  assert.match(cardRule, /overflow-wrap:\s*anywhere/);
  assert.doesNotMatch(mobileBlock, /\.ask-starter-grid\s*\{[^}]*grid-template-columns:\s*1fr/s);
});

test('新聊天画布应继续保留阅读轴、输入框和深色低对比边界', () => {
  assert.match(appCssSource, /\.ask-thread-shell\s*\{[^}]*max-width:\s*min\(100%,\s*72rem\)/s);
  assert.match(appCssSource, /\.ask-composer-shell\s*\{[^}]*max-width:\s*min\(100%,\s*74rem\)/s);
  assert.match(appCssSource, /\.dark \.ask-composer-shell\s*\{[^}]*border-color:\s*var\(--surface-border-default\)[^}]*box-shadow:\s*none/s);
});
