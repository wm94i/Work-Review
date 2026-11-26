import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { mockIPC } from '@tauri-apps/api/mocks';
import { setLocale, t } from '$lib/i18n/index.ts';
import { isLocalLoopbackEmbeddingEndpoint } from '../../routes/settings/embeddingEndpoints.ts';
import SettingsAI from '../../routes/settings/components/SettingsAI.svelte';

afterEach(() => {
  cleanup();
});

function createSemanticAiConfig() {
  return {
    ai_mode: 'summary' as const,
    text_model: {
      provider: 'ollama' as const,
      endpoint: 'http://localhost:11434',
      model: 'qwen2.5',
      api_key: null,
    },
    text_model_profiles: [],
    text_model_provider_cache: {},
    task_model_routing: {
      report_provider: null,
      assistant_provider: null,
      classification_provider: null,
      proactive_provider: null,
    },
    assistant_web_access_enabled: false,
    assistant_search_provider: 'duckduckgo' as const,
    assistant_search_api_key: '',
    memory_semantic_enabled: true,
    semantic_auto_index: false,
    embedding_provider: 'ollama' as 'ollama' | 'openai',
    embedding_endpoint: 'http://localhost:11434',
    embedding_model: 'nomic-embed-text',
    embedding_api_key: '',
    assistant_memory_enabled: false,
  };
}

const sampleProviders = [
  {
    id: 'ollama' as const,
    name: 'Ollama',
    description: 'Local',
    default_endpoint: 'http://localhost:11434',
    default_model: 'qwen2.5',
    requires_api_key: false,
  },
];

describe('isLocalLoopbackEmbeddingEndpoint', () => {
  it('recognizes strict loopback hosts and rejects remote, LAN, and spoofed URLs', () => {
    expect(isLocalLoopbackEmbeddingEndpoint('http://localhost:11434')).toBe(true);
    expect(isLocalLoopbackEmbeddingEndpoint('http://localhost.:11434')).toBe(true);
    expect(isLocalLoopbackEmbeddingEndpoint('http://api.localhost:1234/v1')).toBe(true);
    expect(isLocalLoopbackEmbeddingEndpoint('http://127.0.0.1:1234/v1')).toBe(true);
    expect(isLocalLoopbackEmbeddingEndpoint('http://127.0.1.1:8080/v1')).toBe(true);
    expect(isLocalLoopbackEmbeddingEndpoint('http://[::1]:8080/v1')).toBe(true);

    // IPv4-mapped IPv6 is intentionally treated conservatively as non-local
    expect(isLocalLoopbackEmbeddingEndpoint('http://[::ffff:127.0.0.1]:1234/v1')).toBe(false);
    expect(isLocalLoopbackEmbeddingEndpoint('https://api.siliconflow.cn')).toBe(false);
    expect(isLocalLoopbackEmbeddingEndpoint('http://192.168.1.20:11434')).toBe(false);
    expect(isLocalLoopbackEmbeddingEndpoint('http://127.0.0.1.evil.com/v1')).toBe(false);
    expect(isLocalLoopbackEmbeddingEndpoint('http://localhost@evil.com/v1')).toBe(false);
    expect(isLocalLoopbackEmbeddingEndpoint('http://user:pass@127.0.0.1:1234/v1')).toBe(false);
    expect(isLocalLoopbackEmbeddingEndpoint('ftp://127.0.0.1:1234')).toBe(false);
    expect(isLocalLoopbackEmbeddingEndpoint('')).toBe(false);
  });
});

describe('SettingsAI local small embedding presets and privacy hints', () => {
  it('supports Ollama small-model presets, local OpenAI-compatible presets, and honest privacy hints', async () => {
    setLocale('zh-CN');
    const calls: Array<{ cmd: string; payload: unknown }> = [];
    mockIPC((cmd, payload) => {
      calls.push({ cmd, payload });
      if (cmd === 'fetch_models') return ['qwen2.5'];
      if (cmd === 'semantic_memory_status') {
        return {
          status: 'idle',
          rebuildRequired: true,
          indexedActivities: 0,
          totalActivities: 0,
          pendingReindexDates: 0,
          lastError: null,
        };
      }
      if (cmd === 'list_assistant_memory_entries') return [];
      if (cmd === 'test_embedding_model') {
        return { dimension: 512, latencyMs: 18 };
      }
      return null;
    });

    const config = createSemanticAiConfig();
    render(SettingsAI, { props: { config, providers: sampleProviders } });

    const memoryTab = screen.getByRole('button', { name: new RegExp(t('settingsAI.sectionMemory')) });
    await fireEvent.click(memoryTab);

    // 1. Ollama 默认本地提示 + 自定义端口下点击模型预设保留自定义端点
    expect(screen.getByText(t('settingsAI.semanticMemory.ollamaHint'))).toBeTruthy();
    const endpointInput = screen.getByLabelText(t('settingsAI.semanticMemory.endpoint')) as HTMLInputElement;
    await fireEvent.input(endpointInput, { target: { value: 'http://127.0.0.1:11435' } });
    await fireEvent.click(screen.getByRole('button', { name: t('settingsAI.semanticMemory.presetZhSmall') }));
    expect(config.embedding_model).toBe('shaw/dmeta-embedding-zh');
    expect(config.embedding_endpoint).toBe('http://127.0.0.1:11435');

    // 2. 改为远程 Ollama 地址时，提示应诚实切换为外发提示
    await fireEvent.input(endpointInput, { target: { value: 'https://ollama.example.com' } });
    expect(screen.getByText(t('settingsAI.semanticMemory.cloudHint'))).toBeTruthy();

    // 3. 切到 OpenAI 兼容接口并通过真实输入填写旧云端 Key，再点击 LM Studio 本地小模型预设
    const providerSelect = screen.getByLabelText(t('settingsAI.semanticMemory.provider')) as HTMLSelectElement;
    await fireEvent.change(providerSelect, { target: { value: 'openai' } });
    const apiKeyInput = screen.getByLabelText(t('settingsAI.semanticMemory.apiKey')) as HTMLInputElement;
    await fireEvent.input(apiKeyInput, { target: { value: 'sk-old-cloud-key' } });
    expect(apiKeyInput.value).toBe('sk-old-cloud-key');

    await fireEvent.click(screen.getByRole('button', { name: t('settingsAI.semanticMemory.presetLmStudio') }));
    expect(config.embedding_endpoint).toBe('http://127.0.0.1:1234/v1');
    expect(config.embedding_model).toBe('text-embedding-bge-small-zh-v1.5');
    expect(apiKeyInput.value).toBe('');
    expect(screen.getByText(t('settingsAI.semanticMemory.localCompatHint'))).toBeTruthy();
    expect(apiKeyInput.placeholder).toBe(t('settingsAI.semanticMemory.apiKeyOptionalPlaceholder'));

    // 4. 填入欺骗地址时，切回云端提示与 sk-... 占位符
    await fireEvent.input(endpointInput, { target: { value: 'http://127.0.0.1.evil.com/v1' } });
    expect(screen.getByText(t('settingsAI.semanticMemory.cloudHint'))).toBeTruthy();
    expect(apiKeyInput.placeholder).toBe('sk-...');

    // 5. 点击 llama.cpp 本地预设并触发连通性测试，再手动改地址应立即清除过期“可用”提示
    await fireEvent.click(screen.getByRole('button', { name: t('settingsAI.semanticMemory.presetLlamaCpp') }));
    expect(config.embedding_endpoint).toBe('http://127.0.0.1:8080/v1');
    expect(config.embedding_model).toBe('bge-small-zh-v1.5');

    await fireEvent.click(screen.getByRole('button', { name: t('settingsAI.semanticMemory.testEmbedding') }));
    const embeddingTestCalls = calls.filter((c) => c.cmd === 'test_embedding_model');
    expect(embeddingTestCalls.length).toBe(1);
    expect(embeddingTestCalls[0].payload).toEqual({
      provider: 'openai',
      endpoint: 'http://127.0.0.1:8080/v1',
      model: 'bge-small-zh-v1.5',
      apiKey: '',
    });
    expect(JSON.stringify(embeddingTestCalls)).not.toContain('sk-old-cloud-key');

    const okMessage = t('settingsAI.semanticMemory.testOk', { dim: 512, ms: 18 });
    expect(await screen.findByText(okMessage)).toBeTruthy();

    await fireEvent.input(endpointInput, { target: { value: 'https://api.siliconflow.cn' } });
    expect(screen.queryByText(okMessage)).toBeNull();

    // 6. 填云端 Key 后切回 Ollama，应清空隐藏的 API Key
    await fireEvent.input(apiKeyInput, { target: { value: 'sk-temp-cloud-key' } });
    await fireEvent.change(providerSelect, { target: { value: 'ollama' } });
    expect(config.embedding_api_key).toBe('');
  });
});
