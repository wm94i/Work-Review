<script>
  import { createEventDispatcher, onDestroy, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { aiStore } from '$lib/stores/ai.js';
  import { locale, t } from '$lib/i18n/index.js';
  import AssistantMemoryManager from './AssistantMemoryManager.svelte';

  export let config;
  export let providers = [];

  const dispatch = createEventDispatcher();
  $: currentLocale = $locale;
  let aiModes = [];
  let localizedProviders = [];
  // 三入口分区：模型配置 / 助手联网 / 语义记忆（点击切换，互不打扰）
  let aiSection = 'model';

  // 思考模式下拉框的字符串表示（'default' | 'on' | 'off'），跟随配置回显
  let thinkingMode = 'default';
  $: thinkingMode =
    config?.text_model?.enable_thinking === true
      ? 'on'
      : config?.text_model?.enable_thinking === false
        ? 'off'
        : 'default';

  // ══════════ 语义记忆索引管理（查询走助手，管理入口在这里）══════════
  const emptySemanticState = () => ({
    status: 'idle',
    rebuildRequired: true,
    indexedActivities: 0,
    totalActivities: 0,
    lastError: null,
  });
  let semanticState = emptySemanticState();
  let semanticIndexing = false;
  let semanticIndexText = '';
  let semanticError = '';
  let semanticDestroyed = false;

  function semanticStatusKey() {
    if (semanticState.status === 'building') return 'settingsAI.semanticMemory.statusBuilding';
    if (semanticState.status === 'failed') return 'settingsAI.semanticMemory.statusFailed';
    if (semanticState.rebuildRequired) return 'settingsAI.semanticMemory.statusRebuildRequired';
    if (semanticState.status === 'ready') return 'settingsAI.semanticMemory.statusReady';
    return 'settingsAI.semanticMemory.statusIdle';
  }

  function semanticActionKey() {
    if (semanticState.status === 'failed') return 'settingsAI.semanticMemory.retryIndex';
    if (semanticState.status === 'ready') return 'settingsAI.semanticMemory.rebuildIndex';
    if (semanticState.rebuildRequired && semanticState.indexedActivities > 0) {
      return 'settingsAI.semanticMemory.rebuildIndex';
    }
    return 'settingsAI.semanticMemory.buildIndex';
  }

  async function refreshSemanticStats() {
    if (!config?.memory_semantic_enabled) return;
    try {
      semanticState = await invoke('semantic_memory_status');
    } catch (e) {
      console.warn('读取语义记忆状态失败:', e);
    }
  }

  async function startSemanticIndexing() {
    if (semanticIndexing) return;
    semanticIndexing = true;
    semanticError = '';
    semanticIndexText = '';
    try {
      // 后端持久化真实状态；前端只分批推进，直到状态不再是 building。
      while (!semanticDestroyed) {
        const progress = await invoke('index_semantic_memory');
        semanticState = progress.state;
        semanticIndexText = t('settingsAI.semanticMemory.progress', {
          indexed: semanticState.indexedActivities ?? 0,
          total: semanticState.totalActivities ?? 0,
        });
        if (semanticState.status !== 'building') break;
      }
    } catch (e) {
      semanticError = String(e);
    } finally {
      semanticIndexing = false;
      await refreshSemanticStats();
    }
  }

  // 日报生成模式：基础模板 vs AI 增强
  const aiModeConfigs = [
    {
      value: 'local',
      labelKey: 'settingsAI.modeLocal',
      descriptionKey: 'settingsAI.modeLocalDesc',
      requiresText: false
    },
    {
      value: 'summary',
      labelKey: 'settingsAI.modeSummary',
      descriptionKey: 'settingsAI.modeSummaryDesc',
      requiresText: true
    },
  ];
  $: {
    currentLocale;
    aiModes = aiModeConfigs.map((mode) => ({
      ...mode,
      label: t(mode.labelKey),
      description: t(mode.descriptionKey),
    }));
  }

  const providerLabels = {
    ollama: {
      'zh-CN': { name: 'Ollama (本地)', description: '本机运行开源模型，数据不出本机' },
      en: { name: 'Ollama (Local)', description: 'Runs open models on your device, data stays local' },
      'zh-TW': { name: 'Ollama（本機）', description: '在本機執行開源模型，資料不會離開本機' },
    },
    openai: {
      'zh-CN': { name: 'OpenAI / 兼容 API', description: '支持官方及兼容端点（Azure、Cloudflare 等）' },
      en: { name: 'OpenAI / Compatible', description: 'Official OpenAI and compatible endpoints (Azure, Cloudflare, etc.)' },
      'zh-TW': { name: 'OpenAI / 相容 API', description: '支援官方與相容端點（Azure、Cloudflare 等）' },
    },
    siliconflow: {
      'zh-CN': { name: '硅基流动 SiliconFlow', description: '国内高性价比 API' },
      en: { name: 'SiliconFlow', description: 'Cost-effective domestic API' },
      'zh-TW': { name: '矽基流動 SiliconFlow', description: '高性價比 API' },
    },
    deepseek: {
      'zh-CN': { name: 'DeepSeek', description: '国产开源模型，兼容 OpenAI 格式' },
      en: { name: 'DeepSeek', description: 'Open-source model with OpenAI-compatible format' },
      'zh-TW': { name: 'DeepSeek', description: '開源模型，支援 OpenAI 相容格式' },
    },
    qwen: {
      'zh-CN': { name: '通义千问 Qwen', description: '阿里云通义大模型' },
      en: { name: 'Qwen', description: 'Alibaba Tongyi models' },
      'zh-TW': { name: '通義千問 Qwen', description: '阿里雲通義模型' },
    },
    zhipu: {
      'zh-CN': { name: '智谱 ChatGLM', description: '智谱 AI 大模型' },
      en: { name: 'Zhipu ChatGLM', description: 'Large language models from Zhipu AI' },
      'zh-TW': { name: '智譜 ChatGLM', description: '智譜 AI 大模型' },
    },
    moonshot: {
      'zh-CN': { name: '月之暗面 Kimi', description: '擅长长文本' },
      en: { name: 'Moonshot Kimi', description: 'Optimized for long-context tasks' },
      'zh-TW': { name: '月之暗面 Kimi', description: '擅長長文本' },
    },
    doubao: {
      'zh-CN': { name: '火山引擎 豆包', description: '字节跳动大模型' },
      en: { name: 'Doubao', description: 'Models from Volcano Engine / ByteDance' },
      'zh-TW': { name: '火山引擎 豆包', description: '字節跳動大模型' },
    },
    openrouter: {
      'zh-CN': { name: 'OpenRouter', description: '多模型聚合网关，一个 Key 调百家模型' },
      en: { name: 'OpenRouter', description: 'One key for hundreds of models' },
      'zh-TW': { name: 'OpenRouter', description: '多模型聚合閘道，一個 Key 調百家模型' },
    },
    groq: {
      'zh-CN': { name: 'Groq', description: '超高速推理' },
      en: { name: 'Groq', description: 'Ultra-fast inference' },
      'zh-TW': { name: 'Groq', description: '超高速推理' },
    },
    xai: {
      'zh-CN': { name: 'xAI Grok', description: 'xAI 的 Grok 系列模型' },
      en: { name: 'xAI Grok', description: 'Grok models by xAI' },
      'zh-TW': { name: 'xAI Grok', description: 'xAI 的 Grok 系列模型' },
    },
    mistral: {
      'zh-CN': { name: 'Mistral', description: 'Mistral AI 系列模型' },
      en: { name: 'Mistral', description: 'Models by Mistral AI' },
      'zh-TW': { name: 'Mistral', description: 'Mistral AI 系列模型' },
    },
    lmstudio: {
      'zh-CN': { name: 'LM Studio (本地)', description: '本机运行，数据不出电脑' },
      en: { name: 'LM Studio (Local)', description: 'Runs locally, data stays on device' },
      'zh-TW': { name: 'LM Studio（本機）', description: '本機執行，資料不出電腦' },
    },
    custom: {
      'zh-CN': { name: '自定义接口', description: '任何 OpenAI 兼容接口' },
      en: { name: 'Custom endpoint', description: 'Any OpenAI-compatible API' },
      'zh-TW': { name: '自訂介面', description: '任何 OpenAI 相容介面' },
    },
    minimax: {
      'zh-CN': { name: '稀宇科技 MiniMax', description: 'MiniMax 文本模型' },
      en: { name: 'MiniMax', description: 'MiniMax text models' },
      'zh-TW': { name: '稀宇科技 MiniMax', description: 'MiniMax 文字模型' },
    },
    gemini: {
      'zh-CN': { name: 'Google Gemini', description: 'Google Gemini 系列模型' },
      en: { name: 'Google Gemini', description: 'Google Gemini family models' },
      'zh-TW': { name: 'Google Gemini', description: 'Google Gemini 系列模型' },
    },
    claude: {
      'zh-CN': { name: 'Anthropic Claude', description: 'Anthropic Claude 系列模型' },
      en: { name: 'Anthropic Claude', description: 'Anthropic Claude family models' },
      'zh-TW': { name: 'Anthropic Claude', description: 'Anthropic Claude 系列模型' },
    },
  };

  function getLocalizedProvider(provider) {
    const localized = providerLabels[provider?.id]?.[currentLocale];
    if (!localized) {
      return provider;
    }
    return {
      ...provider,
      name: localized.name,
      description: localized.description,
    };
  }
  $: {
    currentLocale;
    localizedProviders = providers.map(getLocalizedProvider);
  }

  // 提供商默认配置
  function getProviderDefaults(providerId) {
    const provider = localizedProviders.find(p => p.id === providerId);
    return {
      endpoint: provider?.default_endpoint || '',
      model: provider?.default_model || '',
      requiresApiKey: provider?.requires_api_key ?? true
    };
  }

  // 从全局 store 订阅测试状态
  let textTestStatus = null;
  let textTestMessage = '';
  let textConnectionVerified = false;
  let showApiKey = false;
  let fetchedModels = [];
  let modelsLoading = false;
  let modelsError = '';
  let modelsLoaded = 0;
  let showManualInput = false;

  const unsubscribe = aiStore.subscribe(state => {
    textTestStatus = state.textTestStatus;
    textTestMessage = state.textTestMessage;
    textConnectionVerified = state.textConnectionVerified;
  });

  // 是否已配置（必须测试成功）
  $: isTextModelConfigured = textConnectionVerified;
  $: hasTextModelConfig = !!(config?.text_model?.endpoint && config?.text_model?.model);

  // 本地 API（LM Studio / Ollama / 任意 localhost 端点）的自动测试会触发模型加载，
  // 耗时且无必要 —— 这类端点跳过进入页面时的自动测试，由用户手动点"测试"。
  function isLocalEndpoint() {
    const provider = config?.text_model?.provider;
    if (provider === 'ollama') return true;
    const endpoint = (config?.text_model?.endpoint || '').toLowerCase();
    return endpoint.includes('localhost') || endpoint.includes('127.0.0.1') || endpoint.includes('0.0.0.0');
  }

  // 当前提供商
  $: currentProvider = localizedProviders.find(p => p.id === config?.text_model?.provider) || localizedProviders[0];
  $: requiresApiKey = currentProvider?.requires_api_key ?? true;

  // 生成参数能力映射（get_ai_providers → generation_capabilities，唯一来源在
  // 后端 core）：不支持的提供商禁用对应设置项并给出提示，
  // 避免"保存成功但实际忽略"。
  $: generationCaps = currentProvider?.generation_capabilities || {};
  $: supportsThinkingToggle = generationCaps.thinking_toggle === true;
  $: supportsThinkingBudget = generationCaps.thinking_budget === true;
  $: supportsMaxOutputTokens = generationCaps.max_output_tokens === true;
  $: thinkingStreamingOnly = generationCaps.thinking_streaming_only === true;

  // 是否选择了 AI 增强模式（决定是否展开配置面板）
  $: isAiMode = config.ai_mode === 'summary';

  // 每个 provider 的配置缓存（切换时保留配置）
  let providerConfigs = {};
  let configInitialized = false;

  $: if (config?.text_model?.provider && !configInitialized) {
    providerConfigs[config.text_model.provider] = {
      endpoint: config.text_model.endpoint,
      model: config.text_model.model,
      api_key: config.text_model.api_key || ''
    };
    configInitialized = true;
  }

  // 服务商品牌色（卡片网格头像底色，参考各家官方主色的近似值）
  const PROVIDER_BRAND = {
    ollama: '#111827',
    openai: '#10a37f',
    siliconflow: '#6e4ff6',
    deepseek: '#4d6bfe',
    qwen: '#615ced',
    zhipu: '#2f6bff',
    moonshot: '#0f172a',
    doubao: '#3370ff',
    minimax: '#ff4d6a',
    gemini: '#4285f4',
    claude: '#d97757',
    openrouter: '#6467f2',
    groq: '#f55036',
    xai: '#000000',
    mistral: '#fa520f',
    lmstudio: '#4338ca',
    custom: '#64748b',
  };

  /** 品牌图标加载失败 → 回退到字母块（图标需运行 scripts/fetch-provider-icons.mjs 落盘） */
  let providerIconFailed = {};

  // ══════════ 联网搜索 / 嵌入模型 连通性测试 ══════════
  let webTestStatus = null; // null | 'testing' | 'success' | 'error'
  let webTestMessage = '';
  let memTestStatus = null;
  let memTestMessage = '';

  async function testWebSearch() {
    if (webTestStatus === 'testing') return;
    webTestStatus = 'testing';
    webTestMessage = '';
    try {
      const result = await invoke('test_assistant_search', {
        provider: config.assistant_search_provider,
        apiKey: config.assistant_search_api_key,
      });
      webTestStatus = 'success';
      webTestMessage = t('settingsAI.webAccess.testOk', {
        count: result?.resultCount ?? 1,
        ms: result?.latencyMs ?? 0,
      });
    } catch (e) {
      webTestStatus = 'error';
      webTestMessage = String(e);
    }
  }

  async function testEmbedding() {
    if (memTestStatus === 'testing') return;
    memTestStatus = 'testing';
    memTestMessage = '';
    try {
      const result = await invoke('test_embedding_model', {
        provider: config.embedding_provider,
        endpoint: config.embedding_endpoint,
        model: config.embedding_model,
        apiKey: config.embedding_api_key,
      });
      memTestStatus = 'success';
      memTestMessage = t('settingsAI.semanticMemory.testOk', {
        dim: result?.dimension ?? 0,
        ms: result?.latencyMs ?? 0,
      });
    } catch (e) {
      memTestStatus = 'error';
      memTestMessage = String(e);
    }
  }

  function providerInitial(id) {
    return String(id || '?').charAt(0).toUpperCase();
  }

  /** 卡片点击入口：复用 handleProviderChange 的缓存/默认值/状态重置逻辑。 */
  function selectProvider(providerId) {
    if ((config.text_model?.provider || 'ollama') === providerId) return;
    handleProviderChange({ target: { value: providerId } });
  }

  function handleProviderChange(e) {
    const providerId = e.target.value;

    // 缓存当前 provider 配置
    if (config.text_model.provider) {
      providerConfigs[config.text_model.provider] = {
        endpoint: config.text_model.endpoint,
        model: config.text_model.model,
        api_key: config.text_model.api_key || ''
      };
    }

    // 恢复缓存或使用默认值
    const defaults = getProviderDefaults(providerId);
    const cached = providerConfigs[providerId];

    config.text_model.provider = providerId;
    config.text_model.endpoint = cached?.endpoint || defaults.endpoint;
    config.text_model.model = cached?.model || defaults.model;
    config.text_model.api_key = cached?.api_key || '';

    // 切换提供商时清空状态
    textTestStatus = null;
    textTestMessage = '';
    textConnectionVerified = false;
    modelsError = '';
    fetchedModels = [];
    modelsLoaded = 0;
    aiStore.reset();
    refreshModels();
    dispatch('change', config);
  }

  function handleChange() {
    if (config.ai_mode === 'summary' && !isTextModelConfigured) {
      aiStore.setError(t('settingsAI.saveRequiresVerifiedModel'));
      return;
    }
    dispatch('change', config);
  }

  function shouldHideRawMessage(message) {
    return currentLocale === 'en' && /[一-鿿]/.test(message);
  }

  function parseTestErrorMessage(raw) {
    const msg = String(raw || '').trim();
    if (!msg) return null;

    const lower = msg.toLowerCase();

    // HTTP 状态码匹配
    if (/\b401\b/.test(msg) || /unauthorized|invalid.api.?key|authentication/i.test(lower))
      return t('settingsAI.testError.invalidKey');
    if (/\b403\b/.test(msg) || /forbidden/i.test(lower))
      return t('settingsAI.testError.forbidden');
    if (/\b404\b/.test(msg) || /not.?found/i.test(lower))
      return t('settingsAI.testError.notFound');
    if (/\b429\b/.test(msg) || /rate.?limit|too.?many.?request/i.test(lower))
      return t('settingsAI.testError.rateLimit');
    if (/\b500\b/.test(msg) || /internal.?server.?error/i.test(lower))
      return t('settingsAI.testError.serverError');
    if (/\b502\b/.test(msg) || /bad.?gateway/i.test(lower))
      return t('settingsAI.testError.badGateway');
    if (/\b503\b/.test(msg) || /service.?unavailable/i.test(lower))
      return t('settingsAI.testError.unavailable');

    // 常见文本匹配
    if (/model.*not.*found|model.*does.*not.*exist|没有这个?模型/i.test(lower))
      return t('settingsAI.testError.modelNotFound');
    if (/insufficient.*quota|余额不足|out.?of.?credit/i.test(lower))
      return t('settingsAI.testError.quota');
    if (/connection|网络|timeout|超时|econnrefused|dns/i.test(lower))
      return t('settingsAI.testError.connection');
    if (/ssl|tls|certificate|证书/i.test(lower))
      return t('settingsAI.testError.ssl');
    if (/未开通|not.?activated|model.*not.?available/i.test(lower))
      return t('settingsAI.testError.notActivated');

    return null;
  }

  function formatTestError(raw) {
    const parsed = parseTestErrorMessage(raw);
    if (parsed) return parsed;
    const rawTrimmed = String(raw || '').trim();
    if (rawTrimmed && !shouldHideRawMessage(rawTrimmed)) return rawTrimmed;
    return t('settingsAI.genericTestFailed');
  }

  async function testTextModel() {
    aiStore.startTesting();
    try {
      const result = await invoke('test_model', {
        modelConfig: {
          provider: config.text_model.provider,
          endpoint: config.text_model.endpoint,
          api_key: config.text_model.api_key,
          model: config.text_model.model,
        }
      });
      if (result.success) {
        aiStore.setSuccess(
          result.response_time_ms
            ? t('settingsAI.saveAfterTestWithLatency', { ms: result.response_time_ms })
            : t('settingsAI.saveAfterTest')
        );
      } else {
        aiStore.setError(formatTestError(result?.message));
      }
    } catch (e) {
      aiStore.setError(formatTestError(e));
    }
  }

  async function refreshModels() {
    modelsError = '';
    fetchedModels = [];
    modelsLoaded = 0;

    if (!config?.text_model?.endpoint) return;

    const provider = providers.find(p => p.id === config.text_model.provider);
    const needsApiKey = provider?.requires_api_key ?? true;
    if (needsApiKey && !config.text_model.api_key) return;

    modelsLoading = true;
    try {
      const models = await invoke('fetch_models', {
        provider: config.text_model.provider,
        endpoint: config.text_model.endpoint,
        apiKey: config.text_model.api_key || null,
      });
      fetchedModels = Array.isArray(models) ? models : [];
      modelsLoaded = fetchedModels.length;
      if (fetchedModels.length > 0 && !config.text_model.model?.trim()) {
        config.text_model.model = fetchedModels[0];
        dispatch('change', config);
      }
    } catch (e) {
      fetchedModels = [];
      modelsLoaded = 0;
      const msg = e.toString();
      modelsError = msg;
      aiStore.setError(
        msg && !shouldHideRawMessage(msg)
          ? msg
          : t('settingsAI.genericTestFailed')
      );
    } finally {
      modelsLoading = false;
    }
  }

  function getConfigHash() {
    if (!config?.text_model) return null;
    const { provider, endpoint, model, api_key } = config.text_model;
    return `${provider}|${endpoint}|${model}|${api_key || ''}`;
  }

  onMount(async () => {
    refreshSemanticStats();
    await new Promise(r => setTimeout(r, 200));

    const currentHash = getConfigHash();
    let lastHash = null;
    const unsub = aiStore.subscribe(s => { lastHash = s.lastTestedConfigHash; });
    unsub();

    // 本地端点跳过自动测试：LM Studio/Ollana 的连接测试会触发大模型加载，
    // 每次进设置页都自动跑是负担而非帮助，交给用户手动点"测试连接"即可。
    if (hasTextModelConfig && currentHash !== lastHash && !isLocalEndpoint()) {
      aiStore.setConfigHash(currentHash);
      await testTextModel();
    }

    if (config?.text_model?.endpoint && (!requiresApiKey || config.text_model.api_key)) {
      await refreshModels();
    }
  });

  onDestroy(() => {
    semanticDestroyed = true;
    unsubscribe();
  });
</script>

<!-- 日报模式切换 -->
<fieldset class="mb-5" data-locale={currentLocale}>
  <legend class="settings-label mb-2">{t('settingsAI.modeLegend')}</legend>
  <div class="flex gap-2">
    {#each aiModes as mode}
      {@const isSelected = config.ai_mode === mode.value}
      <button
        type="button"
        on:click={() => {
          if (mode.requiresText && !isTextModelConfigured) {
            config.ai_mode = mode.value;
            aiStore.setError(t('settingsAI.switchRequiresVerifiedModel'));
          } else {
            config.ai_mode = mode.value;
            handleChange();
          }
        }}
        class="flex-1 min-h-16 px-3 py-2.5 rounded-lg text-sm font-medium leading-none transition-all duration-150
               {isSelected
                 ? 'settings-segment-active'
                 : 'settings-segment-base'}"
      >
        <div class="flex h-full flex-col items-center justify-center gap-1 text-center">
          <div class="leading-none">{mode.label}</div>
          <div class="text-[10px] leading-none {isSelected ? 'text-white/70' : 'settings-subtle'}">{mode.description}</div>
        </div>
      </button>
    {/each}
  </div>
</fieldset>

<!-- AI 能力配置：三入口分区 -->
{#if isAiMode}
  <div class="pt-3 border-t border-slate-200 dark:border-[#30363d]">
    <div class="mb-3 grid grid-cols-1 gap-2 md:grid-cols-3">
      {#each [
        { id: 'model', label: t('settingsAI.sectionModel'), on: isTextModelConfigured },
        { id: 'web', label: t('settingsAI.sectionWeb'), on: Boolean(config.assistant_web_access_enabled) },
        { id: 'memory', label: t('settingsAI.sectionMemory'), on: Boolean(config.memory_semantic_enabled || config.assistant_memory_enabled) },
      ] as section (section.id)}
        <button
          type="button"
          class="segment-btn rounded-lg border px-3 py-2 text-sm flex items-center justify-center gap-1.5
            {aiSection === section.id ? 'settings-segment-success' : 'settings-segment-idle'}"
          on:click={() => (aiSection = section.id)}
        >
          <span class="inline-block h-1.5 w-1.5 rounded-full {section.on ? 'bg-emerald-400' : 'bg-slate-300 dark:bg-[#484f58]'}"></span>
          <span>{section.label}</span>
        </button>
      {/each}
    </div>

    {#if aiSection === 'model'}
    <div class="settings-block">
    <!-- 提供商：品牌卡片网格（主流 AI 客户端形态） -->
    <div>
      <span class="settings-label mb-1.5 block">{t('settingsAI.provider')}</span>
      <div class="grid grid-cols-2 gap-2 sm:grid-cols-3">
        {#each localizedProviders as provider (provider.id)}
          {@const active = (config.text_model?.provider || 'ollama') === provider.id}
          <button
            type="button"
            class="flex items-center gap-2.5 rounded-xl border px-3 py-2.5 text-start transition
              {active
                ? 'border-indigo-400 bg-indigo-50/70 ring-1 ring-indigo-300/60 dark:border-indigo-500/70 dark:bg-indigo-950/30 dark:ring-indigo-500/40'
                : 'border-slate-200 bg-white hover:border-slate-300 hover:bg-slate-50 dark:border-[#30363d] dark:bg-[#161b22] dark:hover:border-[#484f58] dark:hover:bg-[#21262d]'}"
            on:click={() => selectProvider(provider.id)}
          >
            {#if !providerIconFailed[provider.id]}
              <img
                src={`/icons/providers/${provider.id}.svg`}
                alt=""
                class="h-8 w-8 shrink-0 rounded-lg bg-white object-contain p-1 ring-1 ring-slate-200/70 dark:ring-[#30363d]"
                on:error={() => (providerIconFailed = { ...providerIconFailed, [provider.id]: true })}
              />
            {:else}
              <span
                class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-[13px] font-bold text-white"
                style="background: {PROVIDER_BRAND[provider.id] || '#64748b'}"
              >
                {providerInitial(provider.id)}
              </span>
            {/if}
            <span class="min-w-0 flex-1">
              <span class="block truncate text-[13px] font-medium text-slate-800 dark:text-[#e6edf3]">{provider.name}</span>
              {#if active}
                <span class="block text-[10px] leading-tight text-indigo-500 dark:text-indigo-400">{t('settingsAI.providerActive')}</span>
              {/if}
            </span>
          </button>
        {/each}
      </div>
    </div>

    <!-- 连接配置卡：字段分组 + 常驻状态徽标 + 紧凑测试入口 -->
    <div class="overflow-hidden rounded-xl border border-slate-200 dark:border-[#30363d]">
      <div class="flex items-center justify-between gap-3 border-b border-slate-200/80 bg-slate-50/70 px-3.5 py-2.5 dark:border-[#30363d]/80 dark:bg-[#161b22]/70">
        <div class="flex min-w-0 items-center gap-2">
          <span class="text-[13px] font-semibold text-slate-700 dark:text-[#c9d1d9]">{t('settingsAI.connectionTitle')}</span>
          {#if textTestStatus === 'success'}
            <span class="rounded-full bg-emerald-50 px-2 py-0.5 text-[10px] font-medium text-emerald-600 dark:bg-emerald-950/40 dark:text-emerald-400">✓ {t('settingsAI.statusConnected')}</span>
          {:else if textTestStatus === 'error'}
            <span class="rounded-full bg-rose-50 px-2 py-0.5 text-[10px] font-medium text-rose-500 dark:bg-rose-950/40 dark:text-rose-400">✗ {t('settingsAI.statusFailed')}</span>
          {:else if textTestStatus === 'testing'}
            <span class="inline-flex items-center gap-1 rounded-full bg-slate-100 px-2 py-0.5 text-[10px] font-medium text-slate-500 dark:bg-[#21262d] dark:text-[#7d8590]">
              <span class="h-2 w-2 animate-spin rounded-full border border-current border-t-transparent"></span>
              {t('settingsAI.testing')}
            </span>
          {:else}
            <span class="rounded-full bg-slate-100 px-2 py-0.5 text-[10px] font-medium text-slate-400 dark:bg-[#21262d] dark:text-[#636c76]">{t('settingsAI.statusUntested')}</span>
          {/if}
        </div>
        <button
          on:click={testTextModel}
          disabled={textTestStatus === 'testing' || !hasTextModelConfig}
          class="shrink-0 rounded-lg px-3 py-1.5 text-xs font-medium transition settings-action-secondary disabled:cursor-not-allowed disabled:opacity-40"
        >
          {t('settingsAI.testConnection')}
        </button>
      </div>
      <div class="settings-block p-3.5">

    <!-- API 地址 -->
    <div>
      <label for="ai-endpoint" class="settings-label mb-1.5">{t('settingsAI.endpoint')}</label>
      <input
        id="ai-endpoint"
        type="text"
        bind:value={config.text_model.endpoint}
        on:change={handleChange}
        class="control-input-mono"
        placeholder={currentProvider?.default_endpoint || 'http://localhost:11434'}
      />
    </div>

    <!-- API 密钥（自定义/LM Studio 也展示：部分自建端点需要 Key，可留空） -->
    {#if requiresApiKey || config.text_model?.provider === 'custom' || config.text_model?.provider === 'lmstudio'}
      <div>
        <label for="ai-apikey" class="settings-label mb-1.5">{t('settingsAI.apiKey')}</label>
        <div class="relative">
          {#if showApiKey}
            <input
              id="ai-apikey"
              type="text"
              bind:value={config.text_model.api_key}
              on:change={handleChange}
              class="control-input pr-12"
              placeholder="sk-..."
            />
          {:else}
            <input
              id="ai-apikey"
              type="password"
              bind:value={config.text_model.api_key}
              on:change={handleChange}
              class="control-input pr-12"
              placeholder="sk-..."
            />
          {/if}
          <button
            type="button"
            class="absolute inset-y-0 right-3 inline-flex items-center justify-center text-slate-400 transition hover:text-slate-700 dark:text-[#636c76] dark:hover:text-[#adbac7]"
            aria-label={showApiKey ? t('settingsAI.hideApiKey') : t('settingsAI.showApiKey')}
            title={showApiKey ? t('settingsAI.hideApiKey') : t('settingsAI.showApiKey')}
            on:click={() => showApiKey = !showApiKey}
          >
            <svg class="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
              <path stroke-linecap="round" stroke-linejoin="round" d="M2.25 12s3.75-6.75 9.75-6.75S21.75 12 21.75 12 18 18.75 12 18.75 2.25 12 2.25 12Z" />
              <circle cx="12" cy="12" r="3.25" />
            </svg>
          </button>
        </div>
      </div>
    {/if}

    <!-- 模型选择 -->
    <div>
      <div class="flex items-end gap-2">
        <div class="flex-1">
          <label for="ai-model" class="settings-label mb-1.5">{t('settingsAI.model')}</label>
          {#if showManualInput || fetchedModels.length === 0}
            <input
              id="ai-model"
              type="text"
              bind:value={config.text_model.model}
              on:change={handleChange}
              class="control-input"
              placeholder={currentProvider?.default_model || 'qwen2.5'}
            />
          {:else}
            <select
              id="ai-model"
              value={config.text_model.model}
              on:change={(e) => {
                if (e.target.value === '__manual__') {
                  showManualInput = true;
                  config.text_model.model = '';
                  return;
                }
                config.text_model.model = e.target.value;
                handleChange();
              }}
              class="control-input"
            >
              <option value="__manual__">{t('settingsAI.manualModel')}</option>
              {#each fetchedModels as model (model)}
                <option value={model}>{model}</option>
              {/each}
            </select>
          {/if}
        </div>

        <button
          type="button"
          on:click={refreshModels}
          disabled={modelsLoading || !config.text_model.endpoint || (requiresApiKey && !config.text_model.api_key)}
          class="shrink-0 min-h-10 px-3 py-2 text-xs font-medium rounded-lg leading-none transition-all settings-action-secondary disabled:opacity-40 disabled:cursor-not-allowed"
        >
          {#if modelsLoading}
            <span class="inline-flex items-center gap-1">
              <span class="w-3 h-3 border-2 border-current border-t-transparent rounded-full animate-spin"></span>
            </span>
          {:else}
            {t('settingsAI.refreshModels')}
          {/if}
        </button>
      </div>

      {#if showManualInput && fetchedModels.length > 0}
        <button
          type="button"
          on:click={() => { showManualInput = false; }}
          class="settings-link-action mt-1"
        >
          {t('settingsAI.backToList')}
        </button>
      {/if}

      {#if modelsError}
        <p class="settings-note text-rose-500 dark:text-rose-400">{modelsError}</p>
      {:else if modelsLoaded > 0}
        <p class="settings-note">{t('settingsAI.loadedModels', { count: modelsLoaded })}</p>
      {/if}
    </div>

    <!-- 测试结果详情 -->
    {#if textTestMessage}
      <div class="px-3 py-2 rounded-lg text-xs {textTestStatus === 'success' ? 'settings-tone-success' : 'settings-tone-danger'}">
        {textTestMessage}
      </div>
    {/if}

    <!-- 生成超时：模型回答/日报生成的等待上限，本地大模型慢时可调大 -->
    <div class="settings-block p-3.5 space-y-3">
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <span class="settings-text text-sm">{t('settingsAI.timeouts.assistant')}</span>
          <p class="settings-muted mt-0.5">{t('settingsAI.timeouts.assistantHint')}</p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <input
            type="number"
            aria-label={t('settingsAI.timeouts.assistant')}
            min="30"
            max="900"
            step="10"
            bind:value={config.assistant_timeout_secs}
            on:change={() => {
              config.assistant_timeout_secs = Math.max(30, Math.min(900, Number(config.assistant_timeout_secs) || 120));
              handleChange();
            }}
            class="w-20 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-[#484f58] dark:bg-[#21262d]"
          />
          <span class="text-xs settings-subtle">{t('settingsAI.timeouts.secondsUnit')}</span>
        </div>
      </div>
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <span class="settings-text text-sm">{t('settingsAI.timeouts.report')}</span>
          <p class="settings-muted mt-0.5">{t('settingsAI.timeouts.reportHint')}</p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <input
            type="number"
            aria-label={t('settingsAI.timeouts.report')}
            min="60"
            max="1800"
            step="30"
            bind:value={config.report_generation_timeout_secs}
            on:change={() => {
              config.report_generation_timeout_secs = Math.max(60, Math.min(1800, Number(config.report_generation_timeout_secs) || 300));
              handleChange();
            }}
            class="w-20 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-[#484f58] dark:bg-[#21262d]"
          />
          <span class="text-xs settings-subtle">{t('settingsAI.timeouts.secondsUnit')}</span>
        </div>
      </div>
    </div>

    <!-- 生成参数：思考模式与 token 上限，针对思考型模型（Qwen3/DeepSeek-R1 等）。
         按提供商能力映射禁用不支持的项，避免"保存成功但实际忽略"。 -->
    <div class="settings-block p-3.5 space-y-3">
      {#if !supportsThinkingToggle && !supportsMaxOutputTokens}
        <p class="settings-note settings-muted">{t('settingsAI.generation.unsupportedProvider')}</p>
      {/if}
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <span class="settings-text text-sm">{t('settingsAI.generation.thinkingMode')}</span>
          <p class="settings-muted mt-0.5">{t('settingsAI.generation.thinkingModeHint')}</p>
        </div>
        <select
          aria-label={t('settingsAI.generation.thinkingMode')}
          value={thinkingMode}
          disabled={!supportsThinkingToggle}
          on:change={(e) => {
            thinkingMode = e.target.value;
            config.text_model.enable_thinking =
              thinkingMode === 'default' ? null : thinkingMode === 'on';
            handleChange();
          }}
          class="w-36 rounded-md border border-slate-200 bg-white px-2 py-1 text-sm dark:border-[#484f58] dark:bg-[#21262d] disabled:opacity-40 disabled:cursor-not-allowed"
        >
          <option value="default">{t('settingsAI.generation.serverDefault')}</option>
          <option value="on">{t('settingsAI.generation.thinkingOn')}</option>
          <option value="off">{t('settingsAI.generation.thinkingOff')}</option>
        </select>
      </div>
      {#if thinkingStreamingOnly && supportsThinkingToggle}
        <p class="settings-note settings-muted">{t('settingsAI.generation.thinkingStreamingOnly')}</p>
      {/if}
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <span class="settings-text text-sm">{t('settingsAI.generation.thinkingBudget')}</span>
          <p class="settings-muted mt-0.5">{t('settingsAI.generation.thinkingBudgetHint')}</p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <input
            type="number"
            aria-label={t('settingsAI.generation.thinkingBudget')}
            min="0"
            max="32768"
            step="64"
            disabled={!supportsThinkingBudget}
            value={config.text_model.thinking_budget ?? ''}
            placeholder={t('settingsAI.generation.unlimited')}
            on:change={(e) => {
              const raw = e.target.value;
              const val = raw === '' ? null : Math.max(0, Math.min(32768, Number(raw) || 0));
              config.text_model.thinking_budget = val;
              e.target.value = val ?? '';
              handleChange();
            }}
            class="w-24 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-[#484f58] dark:bg-[#21262d] disabled:opacity-40 disabled:cursor-not-allowed"
          />
          <span class="text-xs settings-subtle">tokens</span>
        </div>
      </div>
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <span class="settings-text text-sm">{t('settingsAI.generation.maxOutputTokens')}</span>
          <p class="settings-muted mt-0.5">{t('settingsAI.generation.maxOutputTokensHint')}</p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <input
            type="number"
            aria-label={t('settingsAI.generation.maxOutputTokens')}
            min="0"
            max="131072"
            step="256"
            disabled={!supportsMaxOutputTokens}
            value={config.text_model.max_output_tokens ?? ''}
            placeholder={t('settingsAI.generation.unlimited')}
            on:change={(e) => {
              const raw = e.target.value;
              const val = raw === '' ? null : Math.max(0, Math.min(131072, Number(raw) || 0));
              config.text_model.max_output_tokens = val;
              e.target.value = val ?? '';
              handleChange();
            }}
            class="w-24 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-[#484f58] dark:bg-[#21262d] disabled:opacity-40 disabled:cursor-not-allowed"
          />
          <span class="text-xs settings-subtle">tokens</span>
        </div>
      </div>
    </div>

      </div>
    </div>

    </div>
    {:else if aiSection === 'web'}
    <!-- 助手联网能力：默认关闭；开启后助手可读网页/查天气，配搜索 Key 后可联网搜索 -->
    <div class="space-y-3">
      <label class="flex items-center justify-between gap-3 cursor-pointer">
        <div class="min-w-0">
          <span class="settings-text text-sm inline-flex items-center gap-2">
            {t('settingsAI.webAccess.title')}
            <span class="rounded-full px-1.5 py-0.5 text-[10px] font-medium {config.assistant_web_access_enabled ? 'bg-emerald-50 text-emerald-600 dark:bg-emerald-950/40 dark:text-emerald-400' : 'bg-slate-100 text-slate-400 dark:bg-[#21262d] dark:text-[#636c76]'}">
              {config.assistant_web_access_enabled ? t('settingsAI.statusEnabled') : t('settingsAI.statusDisabled')}
            </span>
          </span>
          <p class="settings-muted mt-0.5">{t('settingsAI.webAccess.hint')}</p>
        </div>
        <button
          type="button"
          class="switch-track shrink-0 {config.assistant_web_access_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
          role="switch"
          aria-label={t('settingsAI.webAccess.title')}
          aria-checked={config.assistant_web_access_enabled}
          on:click={() => {
            config.assistant_web_access_enabled = !config.assistant_web_access_enabled;
            handleChange();
          }}
        >
          <span class="switch-thumb {config.assistant_web_access_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
        </button>
      </label>

      {#if config.assistant_web_access_enabled}
        <div class="space-y-2">
          <div class="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-2">
            <label class="settings-muted text-xs" for="assistant-search-provider">{t('settingsAI.webAccess.searchProvider')}</label>
            <select
              id="assistant-search-provider"
              bind:value={config.assistant_search_provider}
              on:change={handleChange}
              class="control-input"
            >
              <option value="duckduckgo">{t('settingsAI.webAccess.providerFreeBing')}</option>
              <option value="tavily">Tavily</option>
              <option value="bocha">{t('settingsAI.webAccess.providerBocha')}</option>
            </select>

            {#if config.assistant_search_provider !== 'duckduckgo'}
            <label class="settings-muted text-xs" for="assistant-search-key">{t('settingsAI.webAccess.searchKey')}</label>
            <input
              id="assistant-search-key"
              type="password"
              bind:value={config.assistant_search_api_key}
              on:change={handleChange}
              class="control-input"
              placeholder={t('settingsAI.webAccess.searchKeyPlaceholder')}
              autocomplete="off"
            />
            {/if}
          </div>
          <p class="settings-muted">{config.assistant_search_provider === 'duckduckgo' ? t('settingsAI.webAccess.duckDuckGoHint') : t('settingsAI.webAccess.keyHint')}</p>

          <!-- 联网搜索连通性测试 -->
          <div class="flex items-center gap-2 pt-1">
            <button
              type="button"
              class="rounded-lg px-3 py-1.5 text-xs font-medium transition settings-action-secondary disabled:cursor-not-allowed disabled:opacity-40"
              on:click={testWebSearch}
              disabled={webTestStatus === 'testing'}
            >
              {#if webTestStatus === 'testing'}
                <span class="inline-flex items-center gap-1.5"><span class="h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent"></span>{t('settingsAI.testing')}</span>
              {:else}
                {t('settingsAI.webAccess.testSearch')}
              {/if}
            </button>
            {#if webTestMessage}
              <span class="min-w-0 flex-1 truncate text-xs {webTestStatus === 'success' ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-500 dark:text-rose-400'}" title={webTestMessage}>{webTestMessage}</span>
            {/if}
          </div>
        </div>
      {/if}
    </div>
    {:else if aiSection === 'memory'}
    <!-- 语义记忆（屏幕级数字记忆）：默认关闭；本地 Ollama 数据不出机，云端接口明示出网 -->
    <div class="space-y-3">
      <!-- 开启前后对比示例 + 模型类型说明 -->
      <div class="space-y-1.5 rounded-lg bg-slate-50 px-3 py-2.5 dark:bg-[#161b22]/60">
        <p class="settings-muted"><span class="font-medium text-slate-600 dark:text-[#adbac7]">{t('settingsAI.semanticMemory.offLabel')}</span>{t('settingsAI.semanticMemory.offExample')}</p>
        <p class="settings-muted"><span class="font-medium text-slate-600 dark:text-[#adbac7]">{t('settingsAI.semanticMemory.onLabel')}</span>{t('settingsAI.semanticMemory.onExample')}</p>
        <p class="settings-muted">{t('settingsAI.semanticMemory.modelTypeNote')}</p>
      </div>
      <label class="flex items-center justify-between gap-3 cursor-pointer">
        <div class="min-w-0">
          <span class="settings-text text-sm inline-flex items-center gap-2">
            {t('settingsAI.semanticMemory.title')}
            <span class="rounded-full px-1.5 py-0.5 text-[10px] font-medium {config.memory_semantic_enabled ? 'bg-emerald-50 text-emerald-600 dark:bg-emerald-950/40 dark:text-emerald-400' : 'bg-slate-100 text-slate-400 dark:bg-[#21262d] dark:text-[#636c76]'}">
              {config.memory_semantic_enabled ? t('settingsAI.statusEnabled') : t('settingsAI.statusDisabled')}
            </span>
          </span>
          <p class="settings-muted mt-0.5">{t('settingsAI.semanticMemory.hint')}</p>
        </div>
        <button
          type="button"
          class="switch-track shrink-0 {config.memory_semantic_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
          role="switch"
          aria-label={t('settingsAI.semanticMemory.title')}
          aria-checked={config.memory_semantic_enabled}
          on:click={() => {
            config.memory_semantic_enabled = !config.memory_semantic_enabled;
            handleChange();
          }}
        >
          <span class="switch-thumb {config.memory_semantic_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
        </button>
      </label>

      {#if config.memory_semantic_enabled}
        <div class="space-y-2">
          <div class="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-2">
            <label class="settings-muted text-xs" for="embedding-provider">{t('settingsAI.semanticMemory.provider')}</label>
            <select
              id="embedding-provider"
              bind:value={config.embedding_provider}
              on:change={handleChange}
              class="control-input"
            >
              <option value="ollama">{t('settingsAI.semanticMemory.providerOllama')}</option>
              <option value="openai">{t('settingsAI.semanticMemory.providerOpenai')}</option>
            </select>

            <label class="settings-muted text-xs" for="embedding-endpoint">{t('settingsAI.semanticMemory.endpoint')}</label>
            <input
              id="embedding-endpoint"
              type="text"
              bind:value={config.embedding_endpoint}
              on:change={handleChange}
              class="control-input"
              placeholder={config.embedding_provider === 'ollama' ? 'http://localhost:11434' : 'https://api.siliconflow.cn'}
              autocomplete="off"
            />

            <label class="settings-muted text-xs" for="embedding-model">{t('settingsAI.semanticMemory.model')}</label>
            <input
              id="embedding-model"
              type="text"
              bind:value={config.embedding_model}
              on:change={handleChange}
              class="control-input"
              placeholder={config.embedding_provider === 'ollama' ? 'nomic-embed-text' : 'BAAI/bge-m3'}
              autocomplete="off"
            />

            {#if config.embedding_provider === 'openai'}
            <label class="settings-muted text-xs" for="embedding-key">{t('settingsAI.semanticMemory.apiKey')}</label>
            <input
              id="embedding-key"
              type="password"
              bind:value={config.embedding_api_key}
              on:change={handleChange}
              class="control-input"
              placeholder="sk-..."
              autocomplete="off"
            />
            {/if}
          </div>
          <p class="settings-muted">{config.embedding_provider === 'ollama' ? t('settingsAI.semanticMemory.ollamaHint') : t('settingsAI.semanticMemory.cloudHint')}</p>

          <!-- 嵌入模型连通性测试 -->
          <div class="flex items-center gap-2 pt-1">
            <button
              type="button"
              class="rounded-lg px-3 py-1.5 text-xs font-medium transition settings-action-secondary disabled:cursor-not-allowed disabled:opacity-40"
              on:click={testEmbedding}
              disabled={memTestStatus === 'testing' || !config.embedding_endpoint || !config.embedding_model}
            >
              {#if memTestStatus === 'testing'}
                <span class="inline-flex items-center gap-1.5"><span class="h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent"></span>{t('settingsAI.testing')}</span>
              {:else}
                {t('settingsAI.semanticMemory.testEmbedding')}
              {/if}
            </button>
            {#if memTestMessage}
              <span class="min-w-0 flex-1 truncate text-xs {memTestStatus === 'success' ? 'text-emerald-600 dark:text-emerald-400' : 'text-rose-500 dark:text-rose-400'}" title={memTestMessage}>{memTestMessage}</span>
            {/if}
          </div>

          <!-- 索引管理：状态由后端持久化，未 ready 时助手自动降级为 FTS -->
          <div class="space-y-2 rounded-lg border border-slate-200 px-3 py-2.5 dark:border-[#30363d]">
            <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
              <span class="settings-muted">
                {t('settingsAI.semanticMemory.statusLabel')}：
                <strong class="text-slate-700 dark:text-[#c9d1d9]">{t(semanticStatusKey())}</strong>
              </span>
              <button
                type="button"
                class="text-indigo-500 hover:text-indigo-600 dark:text-indigo-400 dark:hover:text-indigo-300 disabled:opacity-50"
                on:click={startSemanticIndexing}
                disabled={semanticIndexing}
              >
                {semanticIndexing ? t('settingsAI.semanticMemory.statusBuilding') : t(semanticActionKey())}
              </button>
            </div>
            <p class="settings-muted">
              {t('settingsAI.semanticMemory.progress', {
                indexed: semanticState.indexedActivities ?? 0,
                total: semanticState.totalActivities ?? 0,
              })}
            </p>
            {#if semanticState.lastError}
              <p class="text-xs text-rose-500 dark:text-rose-400 break-words">
                {t('settingsAI.semanticMemory.lastError', { error: semanticState.lastError })}
              </p>
            {/if}
            {#if semanticError}
              <p class="text-xs text-rose-500 dark:text-rose-400 break-words">{semanticError}</p>
            {/if}
            {#if semanticIndexText}
              <p class="settings-muted">{semanticIndexText}</p>
            {/if}
            <p class="settings-muted">{t('settingsAI.semanticMemory.ftsFallback')}</p>
          </div>
          <p class="settings-muted">{t('settingsAI.semanticMemory.askHint')}</p>
        </div>
      {/if}

      <div class="space-y-3 border-t border-slate-200 pt-4 dark:border-[#30363d]">
        <label class="flex items-center justify-between gap-3 cursor-pointer">
          <div class="min-w-0">
            <span class="settings-text text-sm inline-flex items-center gap-2">
              {t('settingsAI.assistantMemory.title')}
              <span class="rounded-full px-1.5 py-0.5 text-[10px] font-medium {config.assistant_memory_enabled ? 'bg-emerald-50 text-emerald-600 dark:bg-emerald-950/40 dark:text-emerald-400' : 'bg-slate-100 text-slate-400 dark:bg-[#21262d] dark:text-[#636c76]'}">
                {config.assistant_memory_enabled ? t('settingsAI.statusEnabled') : t('settingsAI.statusDisabled')}
              </span>
            </span>
            <p class="settings-muted mt-0.5">{t('settingsAI.assistantMemory.hint')}</p>
          </div>
          <button
            type="button"
            class="switch-track shrink-0 {config.assistant_memory_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
            role="switch"
            aria-label={t('settingsAI.assistantMemory.enabled')}
            aria-checked={config.assistant_memory_enabled}
            on:click={() => {
              config.assistant_memory_enabled = !config.assistant_memory_enabled;
              handleChange();
            }}
          >
            <span class="switch-thumb {config.assistant_memory_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
          </button>
        </label>

        <AssistantMemoryManager enabled={Boolean(config.assistant_memory_enabled)} {t} />
      </div>
    </div>
    {/if}
  </div>
{:else}
  <div class="pt-3 border-t border-slate-200 dark:border-[#30363d]">
    <p class="settings-empty">{t('settingsAI.aiModeDisabled')}</p>
  </div>
{/if}
