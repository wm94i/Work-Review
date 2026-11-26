import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { mockIPC } from '@tauri-apps/api/mocks';
import { setLocale, t } from '$lib/i18n/index.ts';

afterEach(() => {
  cleanup();
});

import SettingsAI from '../../routes/settings/components/SettingsAI.svelte';
import SettingsPrivacy from '../../routes/settings/components/SettingsPrivacy.svelte';

function createAiConfig(aiMode: 'local' | 'summary' = 'local') {
  return {
    ai_mode: aiMode,
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
    memory_semantic_enabled: false,
    semantic_auto_index: false,
    embedding_provider: 'ollama' as const,
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
  {
    id: 'openai' as const,
    name: 'OpenAI',
    description: 'OpenAI',
    default_endpoint: 'https://api.openai.com/v1',
    default_model: 'gpt-4o-mini',
    requires_api_key: true,
  },
];

describe('Settings choice cards and recessed segment groups (behavioral tests)', () => {
  it('toggles SettingsAI choice cards and 3-section recessed tabs with honest aria-pressed semantics', async () => {
    setLocale('zh-CN');
    mockIPC((cmd) => {
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
      return null;
    });

    const config = createAiConfig('local');
    render(SettingsAI, { props: { config, providers: sampleProviders } });

    const modeGroup = screen.getByRole('group', { name: t('settingsAI.modeLegend') });
    const localBtn = within(modeGroup).getByRole('button', { name: new RegExp(t('settingsAI.modeLocal')) });
    const summaryBtn = within(modeGroup).getByRole('button', { name: new RegExp(t('settingsAI.modeSummary')) });

    expect(localBtn.getAttribute('aria-pressed')).toBe('true');
    expect(summaryBtn.getAttribute('aria-pressed')).toBe('false');
    expect(localBtn.className).toContain('ui-choice-card-active');

    await fireEvent.click(summaryBtn);

    expect(config.ai_mode).toBe('summary');
    expect(summaryBtn.getAttribute('aria-pressed')).toBe('true');
    expect(localBtn.getAttribute('aria-pressed')).toBe('false');

    const sectionGroup = screen.getByRole('group', { name: t('settings.aiCardTitle') });
    const modelTab = within(sectionGroup).getByRole('button', { name: new RegExp(t('settingsAI.sectionModel')) });
    const webTab = within(sectionGroup).getByRole('button', { name: new RegExp(t('settingsAI.sectionWeb')) });
    const memoryTab = within(sectionGroup).getByRole('button', { name: new RegExp(t('settingsAI.sectionMemory')) });

    expect(modelTab.getAttribute('aria-pressed')).toBe('true');
    expect(webTab.getAttribute('aria-pressed')).toBe('false');

    await fireEvent.click(webTab);
    expect(webTab.getAttribute('aria-pressed')).toBe('true');
    expect(modelTab.getAttribute('aria-pressed')).toBe('false');

    await fireEvent.click(memoryTab);
    expect(memoryTab.getAttribute('aria-pressed')).toBe('true');
    expect(webTab.getAttribute('aria-pressed')).toBe('false');
  });

  it('maps SettingsPrivacy unknown-URL fallback toggle group cleanly between null, anonymize, and record', async () => {
    setLocale('zh-CN');
    mockIPC(() => null);

    const config = {
      privacy: {
        app_rules: [],
        excluded_keywords: [],
        excluded_domains: [],
        skip_private_windows: true,
        unknown_url_fallback: null as 'record' | 'anonymize' | null,
      },
    };

    render(SettingsPrivacy, { props: { config, runningApps: [], recentApps: [] } });

    const fallbackGroup = screen.getByRole('group', { name: t('settingsPrivacy.unknownUrlFallback') });
    const buttons = within(fallbackGroup).getAllByRole('button');
    expect(buttons.length).toBe(3);
    expect(buttons[0].getAttribute('aria-pressed')).toBe('true');

    await fireEvent.click(buttons[1]);
    expect(config.privacy.unknown_url_fallback).toBe('anonymize');
    expect(buttons[1].getAttribute('aria-pressed')).toBe('true');

    await fireEvent.click(buttons[2]);
    expect(config.privacy.unknown_url_fallback).toBe('record');
    expect(buttons[2].getAttribute('aria-pressed')).toBe('true');

    await fireEvent.click(buttons[0]);
    expect(config.privacy.unknown_url_fallback).toBeNull();
    expect(buttons[0].getAttribute('aria-pressed')).toBe('true');
  });
});
