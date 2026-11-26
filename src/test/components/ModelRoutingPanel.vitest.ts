import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { setLocale } from '$lib/i18n/index.ts';
import ModelRoutingPanel from '../../routes/settings/components/ModelRoutingPanel.svelte';

describe('ModelRoutingPanel (Svelte 5 runes behavioral tests)', () => {
  it('renders configured candidates and emits updated routing on selection change', async () => {
    setLocale('zh-CN');
    const onchange = vi.fn();

    const config = {
      text_model: {
        provider: 'ollama',
        endpoint: 'http://127.0.0.1:11434',
        model: 'qwen2.5:7b',
        api_key: null,
      },
      text_model_provider_cache: {
        openai: {
          provider: 'openai',
          endpoint: 'https://api.openai.com/v1',
          model: 'gpt-4o-mini',
          api_key: 'sk-test-valid',
        },
        deepseek: {
          provider: 'deepseek',
          endpoint: 'https://api.deepseek.com/v1',
          model: 'deepseek-chat',
          api_key: '', // 缺少必填 API Key，不应作为候选渲染
        },
      },
      text_model_profiles: [
        {
          id: 'fast-local',
          name: '极速本地档案',
          model_config: {
            provider: 'lmstudio',
            endpoint: 'http://127.0.0.1:1234/v1',
            model: 'llama-3.2',
            api_key: null,
          },
        },
      ],
      task_model_routing: {
        report_provider: null,
        assistant_provider: 'legacy-provider',
        classification_provider: null,
        proactive_provider: null,
      },
    };

    render(ModelRoutingPanel, {
      props: {
        config,
        providers: [
          { id: 'ollama', name: 'Ollama', requires_api_key: false },
          { id: 'openai', name: 'OpenAI', requires_api_key: true },
          { id: 'deepseek', name: 'DeepSeek', requires_api_key: true },
          { id: 'lmstudio', name: 'LM Studio', requires_api_key: false },
        ],
        onchange,
      },
    });

    const selects = screen.getAllByRole('combobox') as HTMLSelectElement[];
    expect(selects).toHaveLength(4);

    // 应包含已配置候选与保留的历史已选值，但不包含缺少 key 的 deepseek
    const reportSelect = selects[0];
    const optionValues = [...reportSelect.options].map((opt) => opt.value);
    expect(optionValues).toContain('');
    expect(optionValues).toContain('ollama');
    expect(optionValues).toContain('openai');
    expect(optionValues).toContain('fast-local');
    expect(optionValues).toContain('legacy-provider');
    expect(optionValues).not.toContain('deepseek');

    await fireEvent.change(reportSelect, { target: { value: 'openai' } });
    expect(onchange).toHaveBeenCalledTimes(1);
    const updatedConfig = onchange.mock.calls[0][0];
    expect(updatedConfig.task_model_routing.report_provider).toBe('openai');
    expect(updatedConfig.task_model_routing.assistant_provider).toBe('legacy-provider');
  });
});
