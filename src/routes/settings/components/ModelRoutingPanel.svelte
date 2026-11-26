<script lang="ts">
  import { locale, t } from '$lib/i18n/index.ts';

  type ProviderId = string;

  interface TaskModelRouting {
    report_provider: string | null;
    assistant_provider: string | null;
    classification_provider: string | null;
    proactive_provider: string | null;
  }

  interface CachedProviderConfig {
    provider?: ProviderId;
    endpoint?: string;
    model?: string;
    api_key?: string | null;
  }

  interface TextModelProfileItem {
    id: string;
    name: string;
    model_config?: CachedProviderConfig;
  }

  interface ProviderMeta {
    id: ProviderId;
    name: string;
    requires_api_key?: boolean;
  }

  interface RoutingSettingsConfig {
    text_model: {
      provider: ProviderId;
      endpoint: string;
      model: string;
      api_key: string | null;
    };
    text_model_provider_cache?: Partial<Record<ProviderId, CachedProviderConfig>>;
    text_model_profiles?: TextModelProfileItem[];
    task_model_routing?: TaskModelRouting;
  }

  interface Props {
    config: RoutingSettingsConfig;
    providers?: ProviderMeta[];
    onchange?: (config: RoutingSettingsConfig) => void;
  }

  let { config = $bindable(), providers = [], onchange }: Props = $props();

  const currentLocale = $derived($locale);

  interface RouteOption {
    value: string;
    label: string;
  }

  type RoutingField = keyof TaskModelRouting;

  const routingRows: Array<{
    field: RoutingField;
    labelKey: string;
    hintKey: string;
  }> = [
    {
      field: 'report_provider',
      labelKey: 'settingsAI.routingReport',
      hintKey: 'settingsAI.routingReportHint',
    },
    {
      field: 'assistant_provider',
      labelKey: 'settingsAI.routingAssistant',
      hintKey: 'settingsAI.routingAssistantHint',
    },
    {
      field: 'classification_provider',
      labelKey: 'settingsAI.routingClassification',
      hintKey: 'settingsAI.routingClassificationHint',
    },
    {
      field: 'proactive_provider',
      labelKey: 'settingsAI.routingProactive',
      hintKey: 'settingsAI.routingProactiveHint',
    },
  ];

  function providerRequiresKey(providerId: string): boolean {
    const matched = providers.find((item) => item.id === providerId);
    if (matched && typeof matched.requires_api_key === 'boolean') {
      return matched.requires_api_key;
    }
    return !['ollama', 'lmstudio', 'custom'].includes(providerId);
  }

  function isConfiguredCandidate(providerId: string, candidate: CachedProviderConfig | undefined): boolean {
    if (!candidate) return false;
    const endpoint = (candidate.endpoint || '').trim();
    const model = (candidate.model || '').trim();
    if (!endpoint || !model) return false;
    if (!providerRequiresKey(providerId)) return true;
    return Boolean((candidate.api_key || '').trim());
  }

  function getProviderDisplayName(providerId: string): string {
    return providers.find((item) => item.id === providerId)?.name || providerId;
  }

  const routing = $derived<TaskModelRouting>(
    config.task_model_routing || {
      report_provider: null,
      assistant_provider: null,
      classification_provider: null,
      proactive_provider: null,
    },
  );

  const routeOptions = $derived.by(() => {
    const options: RouteOption[] = [];
    const seen = new Set<string>();

    const currentProvider = config?.text_model?.provider;
    if (currentProvider && isConfiguredCandidate(currentProvider, config.text_model)) {
      seen.add(currentProvider);
      options.push({
        value: currentProvider,
        label: `${getProviderDisplayName(currentProvider)} · ${config.text_model.model.trim()}`,
      });
    }

    for (const [providerId, cached] of Object.entries(config?.text_model_provider_cache || {})) {
      if (!providerId || seen.has(providerId) || !isConfiguredCandidate(providerId, cached)) {
        continue;
      }
      seen.add(providerId);
      options.push({
        value: providerId,
        label: `${getProviderDisplayName(providerId)} · ${(cached?.model || '').trim()}`,
      });
    }

    for (const profile of config?.text_model_profiles || []) {
      if (!profile?.id || profile.id === 'default-text-model' || seen.has(profile.id)) {
        continue;
      }
      const providerId = profile.model_config?.provider || '';
      if (!isConfiguredCandidate(providerId, profile.model_config)) {
        continue;
      }
      seen.add(profile.id);
      options.push({
        value: profile.id,
        label: profile.name || `${getProviderDisplayName(providerId)} · ${(profile.model_config?.model || '').trim()}`,
      });
    }

    // 保留已保存但当前暂时未通过完整校验的值，避免打开页面时下拉值悄悄丢失
    for (const row of routingRows) {
      const savedValue = routing[row.field]?.trim();
      if (savedValue && !seen.has(savedValue)) {
        seen.add(savedValue);
        options.push({
          value: savedValue,
          label: getProviderDisplayName(savedValue),
        });
      }
    }

    return options;
  });

  function updateRoute(field: RoutingField, value: string): void {
    const nextValue = value.trim() || null;
    const nextRouting: TaskModelRouting = {
      report_provider: routing.report_provider ?? null,
      assistant_provider: routing.assistant_provider ?? null,
      classification_provider: routing.classification_provider ?? null,
      proactive_provider: routing.proactive_provider ?? null,
      [field]: nextValue,
    };
    config = {
      ...config,
      task_model_routing: nextRouting,
    };
    onchange?.(config);
  }
</script>

<div
  class="mt-4 overflow-hidden rounded-xl border border-slate-200 dark:border-[#30363d]"
  data-locale={currentLocale}
>
  <div class="border-b border-slate-200/80 bg-slate-50/70 px-3.5 py-2.5 dark:border-[#30363d]/80 dark:bg-[#161b22]/70">
    <p class="text-[13px] font-semibold text-slate-700 dark:text-[#c9d1d9]">{t('settingsAI.routingTitle')}</p>
    <p class="settings-note mt-0.5">{t('settingsAI.routingHint')}</p>
  </div>

  <div class="settings-block p-3.5">
    <div class="grid grid-cols-1 gap-3 md:grid-cols-2">
      {#each routingRows as row (row.field)}
        <label class="block">
          <span class="settings-label">{t(row.labelKey)}</span>
          <select
            class="control-input mt-1"
            aria-label={t(row.labelKey)}
            value={routing[row.field] || ''}
            onchange={(e) => updateRoute(row.field, e.currentTarget.value)}
          >
            <option value="">{t('settingsAI.routingFollowDefault')}</option>
            {#each routeOptions as option (option.value)}
              <option value={option.value}>{option.label}</option>
            {/each}
          </select>
          <p class="settings-note mt-1">{t(row.hintKey)}</p>
        </label>
      {/each}
    </div>

    {#if routeOptions.length <= 1}
      <p class="settings-note mt-2">{t('settingsAI.routingNoExtraHint')}</p>
    {/if}
  </div>
</div>
