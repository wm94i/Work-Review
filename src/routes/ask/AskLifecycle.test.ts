import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';
import { get } from 'svelte/store';
import { assistantStore } from '../../lib/stores/assistant.ts';

const source = await readFile(new URL('./Ask.svelte', import.meta.url), 'utf8');

// 执行组件中的实际代码，而不是在测试中重写修复后的取值逻辑。
function evaluate(code: string, context: Record<string, unknown>) {
  return runInNewContext(ts.transpileModule(code, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
  }).outputText, context);
}

const persistRound = source.match(/  async function persistRound\([\s\S]*?\n  \}/)?.[0];
const completion = source.match(/      \/\/ 事件优先：[\s\S]*?(?=    \} catch \(e\) \{)/)?.[0];
const stopCurrentRequest = source.match(/  async function stopCurrentRequest\(\) \{[\s\S]*?\n  \}/)?.[0];
assert.ok(persistRound && completion && stopCurrentRequest, '应找到实际持久化、请求完成与停止代码');
assert.equal(typeof window, 'undefined', '仅在 Node 内存环境测试，不访问用户 localStorage');

test.afterEach(() => assistantStore.reset());

for (const initialContent of ['', '部分回答']) {
  for (const streamSettled of [false, true]) {
    test(`切页解除订阅后持久化完整终态：旧内容=${JSON.stringify(initialContent)}，done=${streamSettled}`, async () => {
      assistantStore.reset();
      const assistantMessageId = 'background-answer';
      assistantStore.appendMessage({
        id: assistantMessageId,
        role: 'assistant',
        content: initialContent,
        streaming: true,
      });
      assistantStore.beginSending(assistantMessageId);
      let assistantState = get(assistantStore);
      const unsubscribe = assistantStore.subscribe((state) => { assistantState = state; });
      unsubscribe(); // onDestroy：只解除订阅，后台请求继续。

      const fullContent = '部分回答，以及后台生成的完整结论。';
      const step = { tool: 'search_memory', label: '检索', status: 'done' as const, ok: true, hits: 2, digest: '完整摘要' };
      assistantStore.updateMessageById(assistantMessageId, (message) => ({
        ...message,
        ...(streamSettled ? { content: fullContent, streaming: false } : {}),
        steps: [step],
      }));
      // 另一条消息不能被误当成本轮终态；读取必须按稳定 ID 定位。
      assistantStore.appendMessage({ id: 'other-answer', role: 'assistant', content: '其他回答' });
      assert.equal(assistantState.messages[0].content, initialContent, '卸载后的局部快照确实已过期');

      const writes: Array<{ command: string; args: Record<string, unknown> }> = [];
      await evaluate(`${persistRound}\n(async () => {\n${completion}\nawait Promise.resolve();\n})()`, {
        get, assistantStore, assistantState, assistantMessageId, streamSettled,
        convId: 7,
        trimmed: '问题',
        answer: { answer: fullContent, usedAi: true, modelName: 'test-model' },
        t: (key: string) => key,
        invoke: async (command: string, args: Record<string, unknown>) => { writes.push({ command, args }); },
      });

      assert.deepEqual(writes.map(({ command }) => command), ['append_assistant_message', 'append_assistant_message']);
      assert.equal(writes[0].args.role, 'user');
      assert.equal(writes[0].args.content, '问题');
      assert.equal(writes[1].args.conversationId, 7);
      assert.equal(writes[1].args.role, 'assistant');
      assert.equal(writes[1].args.content, fullContent);
      assert.equal(writes[1].args.modelName, 'test-model');
      assert.deepEqual(JSON.parse(String(writes[1].args.toolDigest)), [
        { tool: 'search_memory', label: '检索', ok: true, hits: 2, digest: '完整摘要' },
      ]);
    });
  }
}

test('返回新组件后停止使用 store 最新请求 ID，无请求时不调用取消', async () => {
  assistantStore.reset();
  const cancelled: string[] = [];
  const stop = evaluate(`${stopCurrentRequest}\nstopCurrentRequest;`, {
    get, assistantStore,
    activeSendingRequestId: null, // 新组件的局部变量尚未发起过请求。
    invoke: async (command: string, args: { requestId: string }) => {
      assert.equal(command, 'cancel_assistant_request');
      cancelled.push(args.requestId);
    },
  }) as () => Promise<void>;

  assistantStore.beginSending('background-request');
  await stop();
  assistantStore.finishSending('background-request');
  await stop();
  assistantStore.beginSending('next-request');
  await stop();
  assert.deepEqual(cancelled, ['background-request', 'next-request']);
});

test('旧组件超时仍只取消自身请求，不误取消 store 中的后续请求', async () => {
  assistantStore.reset();
  assistantStore.beginSending('new-request');
  const cancelled: string[] = [];
  await evaluate(`${stopCurrentRequest}\nstopCurrentRequest();`, {
    get, assistantStore,
    activeSendingRequestId: 'old-request',
    invoke: async (_command: string, args: { requestId: string }) => { cancelled.push(args.requestId); },
  });
  assert.deepEqual(cancelled, ['old-request']);
});

test('chat_work_assistant 调用应透传 conversationId 以关联服务端 Agent Run 账本', () => {
  const invokeBlock = source.match(/invoke<AssistantAnswer>\('chat_work_assistant',\s*\{[\s\S]*?\}\)/)?.[0] ?? '';
  assert.ok(invokeBlock, '应找到 chat_work_assistant 调用');
  assert.match(invokeBlock, /\bconversationId:\s*convId\b/, '应透传当前会话 ID 给后端 Harness 账本');
});
