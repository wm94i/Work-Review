import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const askSource = await readFile(new URL('./Ask.svelte', import.meta.url), 'utf8');

test('Ask 流式事件按本次 assistant 消息 ID 更新，不再依赖全局最后一条 streaming', () => {
  assert.match(askSource, /createRequestEventGate/);
  assert.match(askSource, /id:\s*assistantMessageId/);
  assert.match(askSource, /updateMessageById\(assistantMessageId/);
  assert.doesNotMatch(askSource, /updateLastStreaming\(/);
});

test('Ask 请求终态或异常后会关闭事件门闩，拒绝迟到事件', () => {
  assert.match(askSource, /requestGate\.handle\(event\)/);
  assert.match(askSource, /requestGate\?\.close\(\)/);
});

test('Ask 在流式 done 后仍按消息 ID补写模型元数据', () => {
  assert.match(
    askSource,
    /updateMessageById\(assistantMessageId,[\s\S]*usedAi:[\s\S]*modelName:/,
  );
});

test('失败工具步骤使用失败文案且不会显示命中数', () => {
  assert.match(askSource, /step\.ok === false/);
  assert.match(askSource, /t\('ask\.stepFailed'\)/);
  assert.match(askSource, /step\.tool === 'search_memory' && step\.ok === true && step\.hits != null/);
});

test('Ask 将异常占位消息标记为失败，避免进入下一轮历史', () => {
  assert.match(
    askSource,
    /catch \(e\)[\s\S]*updateMessageById\(assistantMessageId,[\s\S]*failed:\s*true/,
  );
});

test('Ask 使用请求级 sending，后台请求跨页面存活且旧 finally 不会释放新请求', () => {
  assert.match(askSource, /let activeSendingRequestId = null/);
  assert.match(askSource, /assistantStore\.beginSending\(assistantMessageId\)/);
  // 行为变更（切页面不取消生成）：onDestroy 不得清 sending——
  // 请求在后台继续，全局 store 保留"生成中"标记，切回助手页可见进行中状态；
  // 状态收尾由 submitQuestion 的 finally（120s 超时兜底）负责，且按请求 ID 隔离。
  assert.doesNotMatch(
    askSource,
    /onDestroy\(\(\) => \{[^}]*assistantStore\.finishSending/,
  );
  assert.match(
    askSource,
    /finally \{[\s\S]*assistantStore\.finishSending\(assistantMessageId\)/,
  );
  assert.doesNotMatch(askSource, /assistantStore\.setSending\(/);
});

test('Ask 生成期间禁止清空或删除会话，但仍允许打开历史抽屉', () => {
  const clearConversation = askSource.match(/async function clearConversation\(\) \{[\s\S]*?\n  \}/)?.[0] ?? '';
  const deleteConversation = askSource.match(/async function deleteConversation\(id\) \{[\s\S]*?\n  \}/)?.[0] ?? '';
  const headerNewButton = askSource.match(/class="ask-header-action ask-header-action-primary ask-header-new"[\s\S]*?<\/button>/)?.[0] ?? '';
  const drawerNewButton = askSource.match(/class="rounded-lg px-2\.5 py-1 text-xs[\s\S]*?<\/button>/)?.[0] ?? '';
  const deleteButton = askSource.match(/class="ask-history-delete"[\s\S]*?<\/button>/)?.[0] ?? '';

  assert.match(clearConversation, /if \(sending\) return;/);
  assert.match(deleteConversation, /if \(sending\) return;/);
  assert.match(headerNewButton, /disabled=\{sending \|\| \(!hasConversation && conversationId == null\)\}/);
  assert.match(drawerNewButton, /disabled=\{sending\}/);
  assert.match(deleteButton, /disabled=\{sending\}/);
  assert.doesNotMatch(askSource, /class="ask-header-action ask-header-history"[\s\S]{0,240}disabled=\{sending\}/);
});

test('Ask 兜底超时应通知后端取消，不能只结束 UI 等待让任务继续跑', () => {
  // withTimeout 支持超时回调；chat_work_assistant 的超时兜底绑定了
  // cancel_assistant_request，避免后端任务在 UI 超时后继续消耗模型配额。
  assert.match(askSource, /function withTimeout\(promise, ms, onTimeout\)/);
  assert.match(
    askSource,
    /await withTimeout\([\s\S]*?chat_work_assistant[\s\S]*?askTimeoutMs,\s*\(\) => invoke\('cancel_assistant_request', \{ requestId: assistantMessageId \}\)\s*\)/,
  );
});
