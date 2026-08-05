<script>
  import { afterUpdate, onDestroy, onMount, tick } from 'svelte';
  import { fly } from 'svelte/transition';
  import { invoke, Channel } from '@tauri-apps/api/core';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';
  import { assistantStore, BASIC_ASSISTANT_MODEL_ID } from '../../lib/stores/assistant.js';
  import { buildHistoryPayload } from './historyPayload.js';
  import { MODEL_PROVIDER_DISPLAY_NAMES, resolveModelOptionLabel } from './modelPresentation.js';
  import { selectStarterPrompts } from './starterPromptPresentation.js';
  import { createRequestEventGate } from './requestEventGate.js';
  import { reduceStreamEvent } from './streamEvent.js';
  import { formatDurationLocalized, locale, t, tm, translateCategoryLabel } from '$lib/i18n/index.js';
  import { formatUserError } from '$lib/utils/errorDisplay.js';
  import { trapFocus } from '$lib/utils/focusTrap.js';

  marked.use({
    gfm: true,
    breaks: true,
  });

  let input = '';
  let error = null;
  let chatBody;
  let composer;
  let bottomAnchor;
  let assistantState = {};
  let unsubscribeAssistant = () => {};
  let destroyed = false;
  let activeSendingRequestId = null;
  let stickToBottom = true;
  $: sending = assistantState.sending ?? false;
  $: messages = assistantState.messages ?? [];
  $: currentLocale = $locale;
  let starterPrompts = [];
  let dynamicPrompts = [];
  let starterPromptLocale = '';
  let starterPromptRequestId = 0;

  $: if (currentLocale && currentLocale !== starterPromptLocale) {
    const shouldRefreshDynamicPrompts = starterPromptLocale !== '';
    // 语言切换时立即丢弃旧语言问题，并让仍在执行的动态请求失效。
    invalidateStarterPromptRequest();
    dynamicPrompts = [];
    refreshStarterPrompts([]);
    if (shouldRefreshDynamicPrompts) {
      void refreshDynamicPrompts();
    }
  }

  // 模型选择器
  let modelProfiles = [];
  let selectedModelId = BASIC_ASSISTANT_MODEL_ID;
  let modelSelectEl;
  let modelSelectWidth = 'auto';
  let modelMeasureEl;
  let currentModelLabel = '';
  $: currentModelLabel = resolveModelOptionLabel(selectedModelId, modelProfiles, currentLocale, t);

  const providerDisplayNames = MODEL_PROVIDER_DISPLAY_NAMES;

  function localizedProviderName(providerId) {
    return providerDisplayNames[providerId]?.[currentLocale]
      || providerDisplayNames[providerId]?.en
      || providerId
      || '';
  }

  function displayModelProfileName(profile) {
    if (!profile) return '';
    // 优先用 profile.name（后端 default_profile_name 已拼好完整显示名，或用户自定义名）。
    // 避免再次拼接 provider · model_id，那样会与后端重复且暴露裸 API id（如 Qwen/Qwen3-8B）。
    const profileName = profile.name?.trim();
    if (profileName) {
      return profileName;
    }
    // fallback：profile.name 缺失时才用 provider · model 拼一个
    const localizedProvider = localizedProviderName(profile.model_config?.provider);
    const modelName = profile.model_config?.model?.trim();
    if (localizedProvider && modelName) {
      return `${localizedProvider} · ${modelName}`;
    }
    if (modelName) {
      return modelName;
    }
    return '';
  }

  // Measure collapsed select width: text width + padding(px-3=24) + arrow(pr-8=32) + border(2) ≈ text + 46.
  // Use a hidden mirror span with the select's font props to measure precisely,
  // avoiding width:max-content which sizes to the longest option.
  function measureModelSelectWidth() {
    if (!modelMeasureEl) return;
    modelMeasureEl.textContent = currentModelLabel || '';
    const textWidth = modelMeasureEl.offsetWidth;
    const clamped = Math.max(textWidth + 46, 72); // min 72px, max aligns with max-w-[260px]
    modelSelectWidth = Math.min(clamped, 260) + 'px';
  }

  // Re-measure when selection / profile list / locale changes
  $: measureModelSelectWidth(currentModelLabel);

  onMount(async () => {
    unsubscribeAssistant = assistantStore.subscribe((state) => {
      assistantState = state;
      const nextMessages = state.messages || [];
      const previousCount = messages.length;
      const messageCountIncreased = nextMessages.length > previousCount;
      const latestMessage = nextMessages[nextMessages.length - 1];

      selectedModelId = state.selectedModelId || BASIC_ASSISTANT_MODEL_ID;

      if (!nextMessages.length) {
        stickToBottom = true;
        return;
      }

      if (previousCount === 0) {
        void scrollToBottom('auto', 3);
        return;
      }

      if (messageCountIncreased && (stickToBottom || latestMessage?.role === 'user')) {
        void scrollToBottom(latestMessage?.role === 'assistant' ? 'smooth' : 'auto', 2);
      }
    });

    // 加载模型档案
    try {
      const config = await invoke('get_config');
      modelProfiles = config.text_model_profiles || [];
      if (config.assistant_timeout_secs) {
        askTimeoutMs = (config.assistant_timeout_secs + 30) * 1000;
      }
      if (
        selectedModelId !== BASIC_ASSISTANT_MODEL_ID &&
        !modelProfiles.some((profile) => profile.id === selectedModelId)
      ) {
        selectedModelId = BASIC_ASSISTANT_MODEL_ID;
        assistantStore.setSelectedModelId(BASIC_ASSISTANT_MODEL_ID, { userInitiated: false });
      }

      // 首次使用：用户从未手动选过模型，但有已配置的模型档案 → 自动选中第一个
      // （解决"设置页配了模型，助手页却默认用基础模板"的困惑，issue #133）
      if (
        selectedModelId === BASIC_ASSISTANT_MODEL_ID &&
        !assistantState.hasUserSelectedModel &&
        modelProfiles.length > 0
      ) {
        selectedModelId = modelProfiles[0].id;
        assistantStore.setSelectedModelId(modelProfiles[0].id, { userInitiated: false });
      }
    } catch (e) {
      console.warn('加载模型配置失败:', e);
    }

    resizeComposer();
    await scrollToBottom('auto', 3);
    composer?.focus();

    // 先随机展示本地问题；配置 AI 模型时再合并动态问题重新抽取。
    if (starterPrompts.length === 0) {
      refreshStarterPrompts();
    }
    void refreshDynamicPrompts();

    // P3：加载会话列表；DB 为空且 localStorage 有旧历史时做一次性导入
    await loadConversations();
    if (!destroyed) {
      await migrateLegacyMessagesIfNeeded();
    }
  });

  onDestroy(() => {
    destroyed = true;
    // 注意：不在这里清 sending 状态——请求在后台继续跑（切页面不取消），
    // 全局 store 保留"生成中"标记,切回助手页能看到进行中的气泡;
    // submitQuestion 的 finally（120s 超时兜底）保证状态必然收尾,不会僵尸。
    unsubscribeAssistant();
  });

  function sourceLabel(sourceType) {
    const labels = {
      activity: t('ask.referenceTypes.activity'),
      hourly_summary: t('ask.referenceTypes.hourly_summary'),
      daily_report: t('ask.referenceTypes.daily_report'),
    };
    return labels[sourceType] || sourceType;
  }

  // 已知的段落标题——后端模板和 AI 模型都可能输出这些词作为独立行
  const SECTION_TITLES = new Set([
    '结论', '依据', '关键发现', '本期概览', '重点工作',
    '核心观察', '风险与提醒', '下阶段建议', '工作复盘',
    '主要意图', '主要工作', '待跟进事项', '代表性 Session',
    '相关记录依据',
  ]);
  const renderedMarkdownCache = new Map();
  // Streaming render throttle: reuse last HTML within STREAM_RENDER_INTERVAL_MS,
  // so we don't run marked.parse on every token.
  const STREAM_RENDER_INTERVAL_MS = 250;
  const streamRenderState = new Map(); // messageIndex -> { html, at }

  function normalizeAssistantContent(content) {
    const text = (content || '').replace(/\r\n/g, '\n').trim();
    if (!text) return '';

    const lines = text.split('\n');

    // ——— 第 1 步：去掉模板自引用句 ———
    const filtered = [];
    let inCodeBlock = false;
    for (const line of lines) {
      const t = line.trim();
      if (t.startsWith('```')) inCodeBlock = !inCodeBlock;
      if (!inCodeBlock && (
        t.includes('我基于周报复盘') ||
        t.includes('我基于意图识别') ||
        t.includes('我基于 Session 聚合') ||
        t.includes('我基于记忆检索')
      )) continue;
      filtered.push(line);
    }

    // ——— 第 2 步：逐行补全 markdown 格式（兼容已有部分格式的内容）———
    const result = [];
    inCodeBlock = false;

    for (let i = 0; i < filtered.length; i++) {
      const raw = filtered[i];
      const t = raw.trim();

      // 空行保留（段落分隔）
      if (!t) { result.push(''); continue; }

      // 代码块原样透传
      if (t.startsWith('```')) { inCodeBlock = !inCodeBlock; result.push(raw); continue; }
      if (inCodeBlock) { result.push(raw); continue; }

      // 表格行原样透传（避免被下面的"标题/列表"规则误伤，破坏表格语法）
      if (/^\|.*\|$/.test(t)) {
        result.push(raw);
        continue;
      }

      // 已有 markdown 标题 → 保留
      if (/^#{1,6}\s/.test(t)) {
        result.push(raw);
        continue;
      }

      // 已有列表/引用标记 → 保留
      if (/^[-*+]\s/.test(t) || /^\d+\.\s/.test(t) || /^>\s/.test(t)) {
        result.push(raw);
        continue;
      }

      // 已知段落标题（无 # 前缀的纯文本）→ ## 标题
      if (SECTION_TITLES.has(t)) {
        result.push('', `## ${t}`, '');
        continue;
      }

      // "标题（说明）" 格式 → ### 副标题（排除含"："的行，避免与下方 key:value 规则重叠误判）
      if (/^[^（）()。，！？：]{2,20}[（(].+[）)]$/.test(t) && !t.includes('。')) {
        result.push('', `### ${t}`, '');
        continue;
      }

      // 短 key：value 数据行（key ≤ 6 字符，无句号结尾，总长 < 32，不含反引号）→ 列表项
      // 收窄阈值避免把自然语言句子（如"结论：本次工作正常"）误转成列表项
      if (
        /^[^：。！？，`]{1,6}：/.test(t) &&
        !/[。！？]$/.test(t) &&
        !t.includes('`') &&
        t.length < 32
      ) {
        result.push(`- ${t}`);
        continue;
      }

      // 普通文本
      result.push(t);
    }

    return result.join('\n');
  }

  function renderMarkdown(content) {
    const normalized = normalizeAssistantContent(content);
    if (!normalized) return '';

    const cached = renderedMarkdownCache.get(normalized);
    if (cached) return cached;

    const html = DOMPurify.sanitize(marked.parse(normalized));
    renderedMarkdownCache.set(normalized, html);

    // 控制缓存上限，避免长会话内存持续增长
    if (renderedMarkdownCache.size > 120) {
      const oldestKey = renderedMarkdownCache.keys().next().value;
      renderedMarkdownCache.delete(oldestKey);
    }

    return html;
  }

  // 流式渲染：节流，STREAM_RENDER_INTERVAL_MS 内复用上次 HTML，避免每个 token 都跑 marked.parse。
  // key 用消息在数组中的下标，收尾时由 renderMarkdown 接管（命中缓存，无额外开销）。
  function renderStreamingMarkdown(content, key) {
    const now = Date.now();
    const state = streamRenderState.get(key);
    if (state && now - state.at < STREAM_RENDER_INTERVAL_MS) {
      return state.html;
    }
    const html = renderMarkdown(content);
    streamRenderState.set(key, { html, at: now });
    return html;
  }

  function resizeComposer() {
    if (!composer) return;
    composer.style.height = '0px';
    composer.style.height = `${Math.min(composer.scrollHeight, 220)}px`;
  }

  function isNearBottom(threshold = 120) {
    if (!chatBody) return true;
    return chatBody.scrollHeight - chatBody.scrollTop - chatBody.clientHeight <= threshold;
  }

  function syncStickToBottom() {
    stickToBottom = isNearBottom();
  }

  async function scrollToBottom(behavior = 'smooth', attempts = 1) {
    await tick();
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      await new Promise((resolve) => requestAnimationFrame(resolve));
      if (bottomAnchor?.scrollIntoView) {
        bottomAnchor.scrollIntoView({ block: 'end', behavior });
      } else if (chatBody) {
        chatBody.scrollTop = chatBody.scrollHeight;
      }
    }
  }

  // 流式更新时自动滚动：仅在用户位于底部附近时才滚（主流 chat 体验）
  function autoScrollOnStream() {
    if (destroyed || !stickToBottom) return;
    void scrollToBottom('auto', 1);
  }

  function getSelectedModelConfig() {
    if (selectedModelId === BASIC_ASSISTANT_MODEL_ID) {
      return null;
    }
    const profile = modelProfiles.find((p) => p.id === selectedModelId);
    return profile ? profile.model_config : null;
  }

  function handleModelChange(event) {
    selectedModelId = event.currentTarget.value;
    assistantStore.setSelectedModelId(selectedModelId);
    dynamicPrompts = [];
    refreshStarterPrompts([]);
    void refreshDynamicPrompts();
  }

  async function clearConversation() {
    if (sending) return;
    // "新对话"：不删除旧会话，只解绑并清空当前视图；下次发送时自动落库新会话
    assistantStore.setConversation(null, []);
    error = null;
    refreshStarterPrompts();
    await tick();
    await scrollToBottom('auto', 2);
    composer?.focus();
    loadConversations();
  }

  // ══════════ P3：会话持久化 ══════════
  let conversations = [];
  let showConversationList = false;
  $: conversationId = assistantState.conversationId ?? null;

  function openConversationDrawer() {
    showConversationList = true;
    loadConversations();
  }

  /** 迁移期把未翻译的 key 存进过标题的历史数据，显示时兜底翻译。 */
  function displayConversationTitle(title) {
    return title === 'ask.importedConversation' ? t('ask.importedConversation') : title;
  }

  $: currentConversation = conversations.find((item) => item.id === conversationId);
  $: currentConversationTitle = currentConversation?.title
    ? displayConversationTitle(currentConversation.title)
    : t('ask.newConversationSubtitle');

  function toolSummaryText(message) {
    const steps = message.steps || [];
    const pending = steps.find((step) => step.confirmStatus === 'pending');
    if (pending) return t('ask.toolsNeedsConfirmation');

    const running = steps.find((step) => step.status === 'running');
    if (running) return t('ask.toolsRunning', { label: running.label });

    const failedCount = steps.filter((step) => step.ok === false).length;
    if (failedCount > 0) return t('ask.toolsFailed', { count: failedCount });

    return t('ask.toolsCompleted', { count: steps.length });
  }

  function toolSummaryState(message) {
    const steps = message.steps || [];
    if (steps.some((step) => step.confirmStatus === 'pending')) return 'pending';
    if (steps.some((step) => step.status === 'running')) return 'running';
    if (steps.some((step) => step.ok === false)) return 'failed';
    return 'done';
  }

  function shouldExpandToolSummary(message) {
    return (message.steps || []).some(
      (step) => step.confirmStatus === 'pending' || step.status === 'running'
    );
  }

  async function loadConversations() {
    try {
      conversations = await invoke('list_assistant_conversations', { limit: 30 });
    } catch (e) {
      console.warn('加载助手会话列表失败:', e);
      conversations = [];
    }
  }

  function storedMessageToUiMessage(row) {
    let steps = [];
    if (row.toolDigest) {
      try {
        const parsed = JSON.parse(row.toolDigest);
        if (Array.isArray(parsed)) {
          steps = parsed.map((s) => ({
            tool: s.tool,
            label: s.label || s.tool,
            status: 'done',
            ok: s.ok,
            hits: s.hits,
            digest: s.digest,
            references: [],
          }));
        }
      } catch {
        // ignore corrupted digest, keep message body
      }
    }
    return {
      role: row.role,
      content: row.content,
      steps,
      streaming: false,
      failed: false,
      modelName: row.modelName || undefined,
      usedAi: Boolean(row.modelName),
    };
  }

  async function switchConversation(id) {
    if (sending) return;
    showConversationList = false;
    try {
      const rows = await invoke('get_assistant_messages', { conversationId: id });
      assistantStore.setConversation(id, rows.map(storedMessageToUiMessage));
      error = null;
      await tick();
      await scrollToBottom('auto', 2);
    } catch (e) {
      console.warn('加载会话消息失败:', e);
    }
  }

  async function deleteConversation(id) {
    if (sending) return;
    try {
      await invoke('delete_assistant_conversation', { conversationId: id });
      if (conversationId === id) {
        assistantStore.setConversation(null, []);
      }
      await loadConversations();
    } catch (e) {
      console.warn('删除会话失败:', e);
    }
  }

  /** 确保当前对话已落库；返回会话 id（失败时返回 null，不阻塞聊天）。 */
  async function ensureConversation(firstQuestion) {
    if (conversationId != null) return conversationId;
    try {
      const title = String(firstQuestion || '').slice(0, 24) || t('ask.newConversation');
      const id = await invoke('create_assistant_conversation', { title });
      assistantStore.setConversationId(id);
      loadConversations();
      return id;
    } catch (e) {
      console.warn('创建会话失败（本轮仅内存保存）:', e);
      return null;
    }
  }

  /** 每轮完成后把 user + assistant 消息写入 SQLite。 */
  async function persistRound(convId, question, assistantMessage) {
    if (convId == null || !assistantMessage) return;
    try {
      await invoke('append_assistant_message', {
        conversationId: convId,
        role: 'user',
        content: question,
        toolDigest: null,
        modelName: null,
      });
      const digest = (assistantMessage.steps || [])
        .filter((s) => s.status === 'done')
        .map((s) => ({ tool: s.tool, label: s.label, ok: s.ok, hits: s.hits, digest: s.digest }));
      await invoke('append_assistant_message', {
        conversationId: convId,
        role: 'assistant',
        content: assistantMessage.content || '',
        toolDigest: digest.length ? JSON.stringify(digest) : null,
        modelName: assistantMessage.modelName || null,
      });
    } catch (e) {
      console.warn('保存会话消息失败:', e);
    }
  }

  /** 一次性迁移：DB 无会话且 localStorage 有历史 → 导入为一个会话。 */
  async function migrateLegacyMessagesIfNeeded() {
    try {
      if (conversations.length > 0 || !messages.length) return;
      const legacy = messages.filter(
        (m) => (m.role === 'user' || m.role === 'assistant') && !m.streaming && m.content
      );
      if (!legacy.length) return;
      const id = await invoke('create_assistant_conversation', {
        title: t('ask.importedConversation'),
      });
      for (const m of legacy) {
        await invoke('append_assistant_message', {
          conversationId: id,
          role: m.role,
          content: m.content,
          toolDigest: null,
          modelName: m.modelName || null,
        });
      }
      assistantStore.setConversationId(id);
      await loadConversations();
    } catch (e) {
      console.warn('迁移历史对话失败:', e);
    }
  }

  // ══════════ P2：行动确认 / P0：停止 ══════════
  async function respondConfirm(messageId, step, approved) {
    if (!step?.confirmId || step.confirmStatus !== 'pending') return;
    try {
      await invoke('confirm_assistant_action', {
        confirmId: step.confirmId,
        approved,
      });
      assistantStore.updateMessageById(messageId, (m) => ({
        ...m,
        steps: (m.steps || []).map((s) =>
          s.confirmId === step.confirmId
            ? { ...s, confirmStatus: approved ? 'approved' : 'denied' }
            : s
        ),
      }));
    } catch (e) {
      console.warn('回传确认结果失败:', e);
    }
  }

  async function stopCurrentRequest() {
    if (!activeSendingRequestId) return;
    try {
      await invoke('cancel_assistant_request', { requestId: activeSendingRequestId });
    } catch (e) {
      console.warn('发送停止信号失败:', e);
    }
  }

  // 助手回答超时来自用户配置（assistant_timeout_secs）；前端兜底比后端墙钟多留
  // 30s，让后端收束路径（基于已有工具结果产出答案）能正常返回，而不是被前端先掐断。
  let askTimeoutMs = 150_000;

  function withTimeout(promise, ms) {
    let timer;
    const timeout = new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(t('ask.timeoutError'))), ms);
    });
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
  }

  async function submitQuestion(question = input) {
    const trimmed = question.trim();
    if (!trimmed || sending) return;

    // 用户主动发送 → 强制切回底部跟随模式
    stickToBottom = true;
    error = null;

    const history = buildHistoryPayload(messages);

    // P3：确保会话已落库（失败不阻塞聊天，仅本轮不持久化）
    const convId = await ensureConversation(trimmed);

    assistantStore.appendMessage({
      role: 'user',
      content: trimmed,
    });

    // 为本次回答绑定稳定 ID；所有流式事件只允许更新这条消息。
    const assistantMessageId =
      typeof crypto !== 'undefined' && crypto.randomUUID
        ? crypto.randomUUID()
        : `ask-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    activeSendingRequestId = assistantMessageId;
    assistantStore.beginSending(assistantMessageId);

    // 发送即插入占位 assistant message，流式事件会逐步更新它（步骤/引用/答案）
    assistantStore.appendMessage({
      id: assistantMessageId,
      role: 'assistant',
      content: '',
      streaming: true,
      steps: [],
      references: [],
      toolLabels: [],
      usedAi: false,
      failed: false,
    });

    input = '';
    resizeComposer();
    await tick();
    await scrollToBottom('auto', 2);

    let streamSettled = false;
    let requestGate = null;
    try {
      const channel = new Channel();
      requestGate = createRequestEventGate({
        isDestroyed: () => destroyed,
        onEvent: (event) => handleStreamEvent(assistantMessageId, event),
      });
      channel.onmessage = (event) => {
        if (requestGate.handle(event)) streamSettled = true;
      };
      const answer = await withTimeout(
        invoke('chat_work_assistant', {
          question: trimmed,
          history,
          modelConfig: getSelectedModelConfig(),
          locale: currentLocale,
          requestId: assistantMessageId,
          onEvent: channel,
        }),
        askTimeoutMs
      );

      // 事件优先：已收到 done/error 则保留事件内容；否则用 await 返回值兜底。
      assistantStore.updateMessageById(assistantMessageId, (m) => ({
        ...m,
        ...(streamSettled
          ? {}
          : {
              content: answer?.answer?.trim() || t('ask.emptyResponse'),
              references: answer?.references || m.references,
              toolLabels: answer?.toolLabels || m.toolLabels,
              streaming: false,
              failed: false,
            }),
        // Done 事件不携带模型元数据，因此无论是否已收尾都按 ID 补写。
        usedAi: answer?.usedAi ?? m.usedAi,
        modelName: answer?.modelName ?? m.modelName,
      }));

      // P3：本轮完成 → 持久化（从 store 取终态消息）
      const finalMessage = (assistantState.messages || []).find(
        (m) => m.id === assistantMessageId
      );
      persistRound(convId, trimmed, finalMessage);
    } catch (e) {
      requestGate?.close();
      if (!destroyed) {
        error = formatUserError(e, t('common.loadFailedRetry'));
      }
      // 只把错误写入本次占位消息，迟到的旧事件不会影响后续请求。
      assistantStore.updateMessageById(assistantMessageId, (m) => ({
        ...m,
        content: m.content || `${t('ask.requestFailed')}: ${e}`,
        streaming: false,
        failed: true,
      }));
    } finally {
      requestGate?.close();
      assistantStore.finishSending(assistantMessageId);
      if (activeSendingRequestId === assistantMessageId) {
        activeSendingRequestId = null;
      }
      if (destroyed) return;
      await tick();
      resizeComposer();
      composer?.focus();
    }
  }

  // 处理后端流式事件，返回 true 表示终态（done/error）。
  function handleStreamEvent(messageId, event) {
    let terminal = false;
    assistantStore.updateMessageById(messageId, (message) => {
      const result = reduceStreamEvent(message, event, t('ask.requestFailed'));
      terminal = result.terminal;
      return result.message;
    });

    if (event?.type === 'stepStart' || event?.type === 'stepResult' || event?.type === 'token') {
      autoScrollOnStream();
    } else if (event?.type === 'done' && !destroyed) {
      // done：用户在底部时强制滚一次（确保完整内容可见）
      void scrollToBottom('auto', 2);
    }
    return terminal;
  }

  function handleComposerKeydown(event) {
    // 中文等输入法确认候选词时会触发 Enter，组合输入期间不得提交。
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      submitQuestion();
    }
  }

  function invalidateStarterPromptRequest() {
    starterPromptRequestId += 1;
  }

  function refreshStarterPrompts(extraPrompts = dynamicPrompts) {
    const localPrompts = tm('ask.starterPrompts') || [];
    starterPrompts = selectStarterPrompts({
      localPrompts,
      dynamicPrompts: extraPrompts,
      previousPrompts: starterPrompts,
      count: 4,
    });
    starterPromptLocale = currentLocale;
  }

  async function refreshDynamicPrompts() {
    const requestId = ++starterPromptRequestId;

    // 未配置 AI 模型时直接使用本地问题池，避免欢迎态等待网络结果。
    if (selectedModelId === BASIC_ASSISTANT_MODEL_ID) {
      dynamicPrompts = [];
      refreshStarterPrompts([]);
      return;
    }
    const profile = modelProfiles.find((p) => p.id === selectedModelId);
    if (!profile) {
      dynamicPrompts = [];
      refreshStarterPrompts([]);
      return;
    }
    try {
      const stats = await invoke('get_today_stats');
      if (requestId !== starterPromptRequestId || destroyed) return;

      const recentApps = (stats?.app_usage || []).slice(0, 3).map((a) => a.app_name).join(t('common.listSeparator'));
      const topCategory = translateCategoryLabel(stats?.category_usage?.[0]?.category || '');
      const workMinutes = Math.round((stats?.work_time_duration || 0) / 60);

      const systemPrompt = t('ask.starterSystemPrompt');
      const userPrompt = t('ask.starterUserPrompt', {
        workMinutes,
        recentApps: recentApps || t('common.none'),
        topCategory: topCategory || t('common.none'),
      });

      const result = await invoke('generate_text_with_model', {
        modelConfig: profile.model_config,
        systemPrompt,
        prompt: userPrompt,
      });

      if (requestId !== starterPromptRequestId || destroyed) return;
      const parsed = JSON.parse(result);
      dynamicPrompts = Array.isArray(parsed)
        ? parsed.filter((prompt) => typeof prompt === 'string' && prompt.trim())
        : [];
      refreshStarterPrompts(dynamicPrompts);
    } catch (e) {
      if (requestId !== starterPromptRequestId || destroyed) return;
      console.warn('动态 starter 生成失败，改用本地问题池:', e);
      dynamicPrompts = [];
      refreshStarterPrompts([]);
    }
  }

  $: hasConversation = messages.length > 0;
  $: input, resizeComposer();

  // afterUpdate：每次 DOM 更新后，如果用户在底部附近，直接同步滚到底
  // 这是 Svelte 推荐的"保持滚到底部"方案，比 async scrollToBottom 可靠
  afterUpdate(() => {
    if (stickToBottom && chatBody) {
      chatBody.scrollTop = chatBody.scrollHeight;
    }
  });
</script>

<svelte:window
  on:keydown={(e) => {
    if (e.key === 'Escape' && showConversationList) {
      showConversationList = false;
    }
  }}
/>

<div
  class="page-shell ask-workbench-shell h-full"
  data-locale={currentLocale}
>
  <div class="ask-workbench-frame flex h-full min-h-0 flex-col overflow-hidden">
    <div class="page-header page-axis-operation">
      <div class="page-title-group">
        <div class="page-title-badge" aria-hidden="true">
          <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M8 10h8M8 14h4m-6 6h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" />
          </svg>
        </div>
        <div class="page-title-copy">
          <h2>{t('sidebar.nav.ask')}</h2>
          <p>{currentConversationTitle}</p>
        </div>
      </div>

      <div class="ask-header-actions">
        <button
          type="button"
          class="ask-header-action ask-header-history"
          on:click={openConversationDrawer}
          aria-label={t('ask.conversationHistory')}
          title={t('ask.conversationHistory')}
        >
          <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M12 8v4l2.5 1.5M4.8 6.8A8 8 0 1 1 4 15.5M4.8 6.8V3.5m0 3.3H8" />
          </svg>
          <span>{t('ask.conversationHistory')}</span>
        </button>
        <button
          type="button"
          class="ask-header-action ask-header-action-primary ask-header-new"
          on:click={clearConversation}
          disabled={sending || (!hasConversation && conversationId == null)}
          aria-label={t('ask.newConversation')}
          title={t('ask.newConversation')}
        >
          <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.9" d="M12 5v14M5 12h14" />
          </svg>
          <span>{t('ask.newConversation')}</span>
        </button>
      </div>
    </div>

    <div bind:this={chatBody} class="ask-chat-scroll flex-1 min-h-0 overflow-y-auto" on:scroll={syncStickToBottom}>
      {#if !hasConversation}
        <section class="ask-welcome-panel page-axis-reading" aria-labelledby="ask-welcome-title">
          <div class="ask-welcome-product-mark" aria-hidden="true">
            <img src="/icons/128x128.png" alt="" />
          </div>
          <div class="ask-welcome-copy">
            <h2 id="ask-welcome-title">{t('ask.welcomeTitle')}</h2>
            <p>{t('ask.welcomeBrief')}</p>
          </div>
          <div class="ask-starter-grid">
            {#each starterPrompts.slice(0, 4) as prompt, promptIndex}
              <button
                class="ask-starter-card"
                on:click={() => submitQuestion(prompt)}
                disabled={sending}
              >
                <span>{prompt}</span>
              </button>
            {/each}
          </div>
        </section>
      {:else}
        <div class="ask-thread-shell page-axis-reading flex min-h-full flex-col">
          {#each messages as message, messageIndex}
            <div class={message.role === 'user' ? 'ask-message-row ask-message-row-user' : 'ask-message-row ask-message-row-assistant'}>
              <article
                in:fly={{ y: 8, duration: 200 }}
                class={message.role === 'user'
                  ? 'ask-message-card ask-message-card-user'
                  : 'ask-message-card ask-assistant-response'}
                aria-busy={message.role === 'assistant' && Boolean(message.streaming)}
              >
                {#if message.role === 'assistant'}
                  <div class="ask-response-identity">
                    <span class="ask-assistant-mark ask-assistant-mark-small" aria-hidden="true">
                      <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.7" d="M6.8 5.5h7.7a4 4 0 0 1 4 4v2.8a4 4 0 0 1-4 4h-3.6l-3.6 2.5.7-2.5H6.8a4 4 0 0 1-4-4V9.5a4 4 0 0 1 4-4Z" />
                        <path stroke-linecap="round" stroke-width="1.6" d="M18.2 3v3.2M16.6 4.6h3.2" />
                      </svg>
                    </span>
                    <span>{t('ask.title')}</span>
                  </div>

                  {#if message.steps?.length}
                    <details
                      class="ask-tool-summary group/tool"
                      data-state={toolSummaryState(message)}
                      open={shouldExpandToolSummary(message) || undefined}
                    >
                      <summary>
                        <span class="ask-tool-status-dot" aria-hidden="true"></span>
                        <span class="min-w-0 flex-1 truncate">{toolSummaryText(message)}</span>
                        <span class="ask-tool-expand-label">{t('ask.showSteps')}</span>
                        <svg class="h-3.5 w-3.5 shrink-0 transition-transform group-open/tool:rotate-180" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
                          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m6 9 6 6 6-6" />
                        </svg>
                      </summary>

                      <div class="ask-tool-step-list">
                        {#each message.steps as step, si}
                          <div class="ask-tool-step" data-state={step.confirmStatus === 'pending' ? 'pending' : step.status === 'running' ? 'running' : step.ok === false ? 'failed' : 'done'} in:fly={{ x: -4, duration: 160 }}>
                            <div class="ask-tool-step-head">
                              <span class="ask-tool-step-dot" aria-hidden="true"></span>
                              <span class="min-w-0 flex-1 font-medium">{step.label}</span>
                              {#if step.confirmStatus === 'pending'}
                                <span class="ask-tool-step-meta">{t('ask.actionPending')}</span>
                              {:else if step.confirmStatus === 'denied'}
                                <span class="ask-tool-step-meta">{t('ask.actionDenied')}</span>
                              {:else if step.status === 'done' && step.ok === false}
                                <span class="ask-tool-step-meta">{t('ask.stepFailed')}</span>
                              {:else if step.status === 'done' && step.tool === 'search_memory' && step.ok === true && step.hits != null}
                                <span class="ask-tool-step-meta">{step.hits} {t('ask.hits')}</span>
                              {/if}
                            </div>

                            {#if step.confirmId && step.summary}
                              <div class="ask-confirm-panel">
                                <p>{step.summary}</p>
                                {#if step.confirmStatus === 'pending'}
                                  <div class="ask-confirm-actions">
                                    <button
                                      class="ask-confirm-primary"
                                      on:click={() => respondConfirm(message.id, step, true)}
                                    >
                                      {t('ask.approveAction')}
                                    </button>
                                    <button
                                      class="ask-confirm-secondary"
                                      on:click={() => respondConfirm(message.id, step, false)}
                                    >
                                      {t('ask.denyAction')}
                                    </button>
                                  </div>
                                {:else if step.confirmStatus === 'approved'}
                                  <p class="ask-confirm-result ask-confirm-result-approved">{t('ask.actionApproved')}</p>
                                {:else if step.confirmStatus === 'denied'}
                                  <p class="ask-confirm-result">{t('ask.actionDeniedNote')}</p>
                                {/if}
                              </div>
                            {/if}

                            {#if step.references?.length}
                              <div class="ask-tool-reference-list">
                                {#each step.references as ref}
                                  <div>
                                    {#if ref.app_name}<span class="font-medium">{ref.app_name}</span> · {/if}
                                    <span>{ref.title}</span>
                                    <span class="ask-tool-reference-date">— {ref.date}</span>
                                  </div>
                                {/each}
                              </div>
                            {/if}
                          </div>
                        {/each}
                      </div>
                    </details>
                  {/if}

                  <div class="markdown-body assistant-markdown min-w-0 max-w-none">
                    {#if message.streaming}
                      <div class="streaming-content">
                        {#if message.content}
                          {@html renderStreamingMarkdown(message.content, messageIndex)}
                        {:else}
                          <p class="ask-thinking-state">{t('ask.thinking')}</p>
                        {/if}
                        <span class="ask-streaming-cursor" aria-hidden="true">▍</span>
                      </div>
                    {:else}
                      {@html renderMarkdown(message.content)}
                    {/if}
                  </div>

                  {#if message.references?.length}
                    <details class="ask-reference-trail">
                      <summary aria-label={t('ask.referenceTrail', { count: message.references.length })}>
                        <span>{t('ask.referenceTrail', { count: message.references.length })}</span>
                        <span class="ask-reference-line" aria-hidden="true"></span>
                        <svg class="h-3.5 w-3.5 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
                          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m6 9 6 6 6-6" />
                        </svg>
                      </summary>
                      <div class="ask-reference-list">
                        {#each message.references as item}
                          <article class="ask-reference-item">
                            <div class="ask-reference-meta">
                              <span>{sourceLabel(item.sourceType)}</span>
                              <span>{item.date}</span>
                              {#if item.appName}
                                <span>{item.appName}</span>
                              {/if}
                              {#if item.duration}
                                <span>{formatDurationLocalized(item.duration)}</span>
                              {/if}
                            </div>
                            <h3>{item.title}</h3>
                            {#if item.excerpt}
                              <p>{item.excerpt}</p>
                            {/if}
                          </article>
                        {/each}
                      </div>
                    </details>
                  {/if}
                {:else}
                  <p class="whitespace-pre-wrap break-words">{message.content}</p>
                {/if}
              </article>
            </div>
          {/each}

          {#if error}
            <div class="ask-error-callout" role="alert" in:fly={{ y: -8, duration: 220 }}>
              <svg class="h-5 w-5 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" aria-hidden="true">
                <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m9-.75a9 9 0 11-18 0 9 9 0 0118 0zm-9 3.75h.008v.008H12v-.008z" />
              </svg>
              <div class="min-w-0 flex-1">
                <p class="font-medium">{t('ask.requestFailed')}</p>
                <p>{error}</p>
              </div>
            </div>
          {/if}

          <div bind:this={bottomAnchor} class="h-px w-full"></div>
        </div>
      {/if}
    </div>

    <div class="ask-composer-dock">
      <div class="ask-composer-shell page-axis-reading">
        <textarea
          bind:this={composer}
          bind:value={input}
          rows="1"
          class="ask-composer-input"
          placeholder={t('ask.placeholder')}
          aria-label={t('ask.placeholder')}
          on:input={resizeComposer}
          on:keydown={handleComposerKeydown}
        />

        <div class="ask-composer-toolbar">
          <div class="ask-composer-controls">
            <details class="ask-context-menu">
              <summary aria-label={t('ask.recordContext')} title={t('ask.recordContext')}>
                <span class="ask-context-live" aria-hidden="true"></span>
                <span>{t('ask.recordContext')}</span>
                <svg class="h-3 w-3 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m6 9 6 6 6-6" />
                </svg>
              </summary>
              <div class="ask-context-popover">
                <div>
                  <span>{t('ask.contextScope')}</span>
                </div>
                <p>{t('ask.contextSources')}</p>
              </div>
            </details>

            <div class="ask-composer-model-group">
              <span bind:this={modelMeasureEl} class="invisible pointer-events-none absolute left-0 top-0 -z-10 whitespace-nowrap text-[11px] font-medium" aria-hidden="true"></span>
              <select
                bind:this={modelSelectEl}
                bind:value={selectedModelId}
                on:change={handleModelChange}
                class="ask-model-select"
                style="width: {modelSelectWidth}; background-image: url(&quot;data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2394a3b8' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E&quot;); background-repeat: no-repeat; background-position: right 10px center;"
                aria-label={t('ask.modelSelector')}
              >
                <option value={BASIC_ASSISTANT_MODEL_ID}>{t('ask.basicTemplate')}</option>
                {#each modelProfiles as profile}
                  <option value={profile.id}>{displayModelProfileName(profile) || t('ask.aiEnhanced')}</option>
                {/each}
              </select>
            </div>
          </div>

          {#if sending}
            <button
              type="button"
              class="ask-send-button ask-send-button-stop"
              on:click={stopCurrentRequest}
              aria-label={t('ask.stopGenerating')}
              title={t('ask.stopGenerating')}
            >
              <span class="h-2.5 w-2.5 rounded-[var(--radius-xs)] bg-current" aria-hidden="true"></span>
            </button>
          {:else}
            <button
              type="button"
              class="ask-send-button"
              on:click={() => submitQuestion()}
              disabled={!input.trim()}
              aria-label={t('ask.sendMessage')}
              title={t('ask.sendMessage')}
            >
              <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M12 17V7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"></path>
                <path d="M8 11L12 7L16 11" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"></path>
              </svg>
            </button>
          {/if}
        </div>
      </div>
      <p class="ask-composer-hint">{t('ask.composerHint')}</p>
    </div>
  </div>
</div>

<!-- 历史会话抽屉：页内侧滑面板，选中即回到对话 -->
{#if showConversationList}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="fixed inset-0 z-[160] flex justify-end bg-slate-950/32 backdrop-blur-sm animate-fadeIn"
    role="presentation"
    on:click|self={() => (showConversationList = false)}
  >
    <div
      use:trapFocus
      role="dialog"
      aria-modal="true"
      aria-label={t('ask.conversationHistory')}
      class="ask-history-drawer flex h-full w-80 max-w-[85vw] flex-col"
      in:fly={{ x: 320, duration: 220 }}
    >
      <div class="ask-history-head">
        <h3>{t('ask.conversationHistory')}</h3>
        <div class="flex items-center gap-1.5">
          <button
            type="button"
            class="rounded-lg px-2.5 py-1 text-xs font-medium text-indigo-500 transition hover:bg-indigo-50 disabled:cursor-not-allowed disabled:opacity-40 dark:text-indigo-400 dark:hover:bg-indigo-950/40"
            on:click={() => { showConversationList = false; clearConversation(); }}
            disabled={sending}
          >
            {t('ask.newConversation')}
          </button>
          <button
            type="button"
            class="rounded-lg p-1.5 text-slate-400 transition hover:bg-slate-100 hover:text-slate-600 dark:text-[#636c76] dark:hover:bg-[#21262d] dark:hover:text-[#adbac7]"
            on:click={() => (showConversationList = false)}
            title={t('common.cancel')}
          >
            <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" /></svg>
          </button>
        </div>
      </div>
      <div class="app-floating-scroll flex-1 overflow-y-auto p-2">
        {#each conversations as conv (conv.id)}
          <div class="ask-history-item group/conv {conv.id === conversationId ? 'ask-history-item-active' : ''}">
            <button
              type="button"
              class="min-w-0 flex-1 text-start"
              on:click={() => switchConversation(conv.id)}
            >
              <span class="block truncate text-[13px] font-medium text-slate-700 dark:text-[#c9d1d9]">{displayConversationTitle(conv.title)}</span>
              <span class="block text-[11px] text-slate-400 dark:text-[#636c76]">{t('ask.conversationMeta', { count: conv.messageCount })}</span>
            </button>
            <button
              type="button"
              class="ask-history-delete"
              on:click|stopPropagation={() => deleteConversation(conv.id)}
              disabled={sending}
              aria-label={t('ask.deleteConversation')}
              title={t('ask.deleteConversation')}
            >
              <svg class="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M3 6h18M8 6V4.5A1.5 1.5 0 0 1 9.5 3h5A1.5 1.5 0 0 1 16 4.5V6m2 0-1 14H7L6 6m4 4v6m4-6v6" /></svg>
            </button>
          </div>
        {:else}
          <p class="px-3 py-6 text-center text-xs text-slate-400 dark:text-[#636c76]">{t('ask.conversationEmpty')}</p>
        {/each}
      </div>
    </div>
  </div>
{/if}
