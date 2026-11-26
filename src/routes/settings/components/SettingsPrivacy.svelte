<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { locale, t } from '$lib/i18n/index.ts';

  interface AppPrivacyRule {
    app_name: string;
    level: string;
  }

  interface PrivacySettings {
    app_rules: AppPrivacyRule[];
    excluded_keywords: string[];
    excluded_domains?: string[];
    skip_private_windows?: boolean;
    unknown_url_fallback?: 'record' | 'anonymize' | null;
  }

  interface PrivacyConfig {
    privacy: PrivacySettings;
  }

  interface PrivacyChangeDetail {
    autosaved: true;
    config: PrivacyConfig;
  }

  interface PrivacyLevelConfig {
    value: string;
    labelKey: string;
    descKey: string;
  }

  interface LocalizedPrivacyLevel extends PrivacyLevelConfig {
    label: string;
    desc: string;
  }
  
  export let config: PrivacyConfig;
  export let runningApps: string[] = [];
  export let recentApps: string[] = [];
  
  const dispatch = createEventDispatcher<{
    change: PrivacyConfig | PrivacyChangeDetail;
    'refresh-apps': void;
  }>();
  $: currentLocale = $locale;
  let privacyLevels: LocalizedPrivacyLevel[] = [];
  let keywordCount = 0;
  let domainCount = 0;
  
  // 内联输入状态
  let showAppInput = false;
  let selectedApp = '';
  // 「内容过滤」（敏感词 + 域名黑名单）默认折叠，属于进阶配置
  let showContentFilter = false;
  let selectedLevel = 'ignored';
  let batchSelectedApps = new Set<string>();
  let appSearchQuery = '';
  let showAllRecentApps = false;
  let showAllRunningApps = false;
  let showKeywordInput = false;
  let newKeyword = '';
  let showDomainInput = false;
  let newDomain = '';
  let keywordSaveQueue: Promise<void> = Promise.resolve();
  let keywordRevision = 0;

  // 三种策略只表达记录方式，不使用大面积语义色。
  const privacyLevelConfigs: PrivacyLevelConfig[] = [
    { value: 'full', labelKey: 'settingsPrivacy.full', descKey: 'settingsPrivacy.fullDesc' },
    { value: 'anonymized', labelKey: 'settingsPrivacy.anonymized', descKey: 'settingsPrivacy.anonymizedDesc' },
    { value: 'ignored', labelKey: 'settingsPrivacy.ignored', descKey: 'settingsPrivacy.ignoredDesc' },
  ];
  $: {
    currentLocale;
    privacyLevels = privacyLevelConfigs.map((level) => ({
      ...level,
      label: t(level.labelKey),
      desc: t(level.descKey),
    }));
  }
  $: keywordCount = config?.privacy?.excluded_keywords?.length || 0;
  $: domainCount = config?.privacy?.excluded_domains?.length || 0;

  function getPrivacyLevel(levelValue: string): LocalizedPrivacyLevel | undefined {
    return privacyLevels.find((level) => level.value === levelValue) || privacyLevels[0];
  }

  function toggleAppRuleEditor() {
    const opening = !showAppInput;
    showAppInput = opening;
    appSearchQuery = '';
    showAllRecentApps = false;
    showAllRunningApps = false;
    if (opening) dispatch('refresh-apps');
  }

  function cancelAppRuleEditor() {
    showAppInput = false;
    selectedApp = '';
    batchSelectedApps = new Set();
    appSearchQuery = '';
  }

  function addAppRule() {
    const appsToAdd = new Set(batchSelectedApps);
    if (selectedApp) appsToAdd.add(selectedApp);
    if (appsToAdd.size === 0) return;
    let rules = [...config.privacy.app_rules];
    for (const appName of appsToAdd) {
      const existingIndex = rules.findIndex(r => r.app_name === appName);
      if (existingIndex >= 0) {
        rules[existingIndex].level = selectedLevel;
      } else {
        rules.push({ app_name: appName, level: selectedLevel });
      }
    }
    config.privacy.app_rules = rules;
    showAppInput = false;
    selectedApp = '';
    batchSelectedApps = new Set();
    dispatch('change', config);
  }

  function removeAppRule(index: number) {
    const rules = [...config.privacy.app_rules];
    rules.splice(index, 1);
    config.privacy.app_rules = rules;
    dispatch('change', config);
  }

  function persistKeywordChange() {
    const revision = ++keywordRevision;
    dispatch('change', config);
    keywordSaveQueue = keywordSaveQueue
      .then(async () => {
        const savedConfig = JSON.stringify(config);
        await invoke<void>('save_config', { config: JSON.parse(savedConfig) });
        // 保存期间其他隐私项或标签页的修改仍需保留未保存状态。
        if (revision === keywordRevision && JSON.stringify(config) === savedConfig) {
          dispatch('change', { autosaved: true, config });
        }
      })
      .catch((error) => console.error('保存敏感词失败:', error));
  }

  function addKeyword() {
    if (!newKeyword.trim()) return;
    // 避免重复添加
    if (config.privacy.excluded_keywords.includes(newKeyword.trim())) {
      newKeyword = '';
      return;
    }
    config.privacy.excluded_keywords = [
      ...config.privacy.excluded_keywords,
      newKeyword.trim()
    ];
    newKeyword = '';
    showKeywordInput = false;
    persistKeywordChange();
  }

  function removeKeyword(index: number) {
    const keywords = [...config.privacy.excluded_keywords];
    keywords.splice(index, 1);
    config.privacy.excluded_keywords = keywords;
    persistKeywordChange();
  }

  // 域名黑名单管理
  function addDomain() {
    if (!newDomain.trim()) return;
    const domains = config.privacy.excluded_domains || [];
    // 避免重复
    if (domains.includes(newDomain.trim())) {
      newDomain = '';
      return;
    }
    config.privacy.excluded_domains = [...domains, newDomain.trim()];
    newDomain = '';
    showDomainInput = false;
    dispatch('change', config);
  }

  function removeDomain(index: number) {
    const domains = [...(config.privacy.excluded_domains || [])];
    domains.splice(index, 1);
    config.privacy.excluded_domains = domains;
    dispatch('change', config);
  }

  // 快捷选择应用（多选切换）
  function toggleBatchApp(appName: string) {
    if (batchSelectedApps.has(appName)) {
      batchSelectedApps.delete(appName);
    } else {
      batchSelectedApps.add(appName);
    }
    batchSelectedApps = batchSelectedApps; // 触发响应式更新
  }

  function handleKeywordKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter') addKeyword();
  }

  function handleDomainKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter') addDomain();
  }

  type UnknownUrlFallbackChoice = 'auto' | 'anonymize' | 'record';

  const unknownUrlFallbackOptions: { value: UnknownUrlFallbackChoice; labelKey: string }[] = [
    { value: 'auto', labelKey: 'settingsPrivacy.unknownUrlFallbackAuto' },
    { value: 'anonymize', labelKey: 'settingsPrivacy.unknownUrlFallbackAnonymize' },
    { value: 'record', labelKey: 'settingsPrivacy.unknownUrlFallbackRecord' },
  ];

  $: skipPrivateWindows = config?.privacy?.skip_private_windows !== false;
  $: unknownUrlFallbackChoice = (
    config?.privacy?.unknown_url_fallback === 'record'
      ? 'record'
      : config?.privacy?.unknown_url_fallback === 'anonymize'
        ? 'anonymize'
        : 'auto'
  ) as UnknownUrlFallbackChoice;

  function toggleSkipPrivateWindows() {
    config.privacy.skip_private_windows = !skipPrivateWindows;
    dispatch('change', config);
  }

  function setUnknownUrlFallback(mode: UnknownUrlFallbackChoice) {
    config.privacy.unknown_url_fallback = mode === 'auto' ? null : mode;
    dispatch('change', config);
  }
</script>

<div class="settings-card settings-privacy" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsPrivacy.title')}</h3>
  <p class="settings-card-desc settings-privacy-intro">
    {t('settingsPrivacy.storageAndTransferDescription')}
  </p>

  <div class="settings-section settings-privacy-sections">
    <section
      class="settings-block settings-privacy-app-rules"
      aria-labelledby="settings-privacy-app-rules-title"
    >
      <div class="settings-privacy-section-header flex items-start justify-between gap-4">
        <div class="min-w-0">
          <h4 id="settings-privacy-app-rules-title" class="settings-text">
            {t('settingsPrivacy.appRules')}
          </h4>
          <p class="settings-muted mt-1">{t('settingsPrivacy.appRulesHint')}</p>
        </div>
        <button
          type="button"
          on:click={toggleAppRuleEditor}
          class="settings-link-action settings-privacy-add-rule shrink-0"
          aria-expanded={showAppInput}
          aria-controls="settings-privacy-rule-editor"
        >
          {showAppInput ? t('settingsPrivacy.collapse') : t('settingsPrivacy.addRule')}
        </button>
      </div>

      {#if showAppInput}
        <div
          id="settings-privacy-rule-editor"
          class="settings-privacy-rule-editor animate-fadeIn space-y-4 pt-3"
        >
          <div class="settings-field">
            <label for="app-name-input" class="settings-label">
              {t('settingsPrivacy.appInputLabel')}
            </label>
            <input
              id="app-name-input"
              type="text"
              bind:value={selectedApp}
              class="control-input"
              placeholder={t('settingsPrivacy.appPlaceholder')}
            />
          </div>

          {#if recentApps.length > 0 || runningApps.length > 0}
            <div class="settings-privacy-app-picker space-y-3">
              <input
                type="search"
                bind:value={appSearchQuery}
                class="control-input"
                placeholder={t('settingsPrivacy.searchApps')}
                aria-label={t('settingsPrivacy.searchApps')}
              />

              {#if runningApps.length > 0}
                <div class="settings-privacy-app-source">
                  <span class="settings-subtle block mb-1.5">
                    {t('settingsPrivacy.runningApps')}
                  </span>
                  <div class="flex flex-wrap gap-1.5">
                    {#each runningApps
                      .filter(app => !appSearchQuery || app.toLowerCase().includes(appSearchQuery.toLowerCase()))
                      .slice(0, showAllRunningApps ? undefined : 8) as app}
                      <button
                        type="button"
                        on:click={() => toggleBatchApp(app)}
                        class="settings-chip-button {batchSelectedApps.has(app) ? 'settings-chip-button-active' : ''}"
                        aria-pressed={batchSelectedApps.has(app)}
                      >
                        {batchSelectedApps.has(app) ? '✓ ' : ''}{app}
                      </button>
                    {/each}
                  </div>
                  {#if runningApps.length > 8 && !appSearchQuery}
                    <button
                      type="button"
                      on:click={() => showAllRunningApps = !showAllRunningApps}
                      class="settings-link-action mt-1"
                    >
                      {showAllRunningApps
                        ? t('settingsPrivacy.collapse')
                        : `+${runningApps.length - 8} ${t('settingsPrivacy.moreApps')}`}
                    </button>
                  {/if}
                </div>
              {/if}

              {#if recentApps.length > 0}
                <div class="settings-privacy-app-source">
                  <span class="settings-subtle block mb-1.5">
                    {t('settingsPrivacy.historyApps')}
                  </span>
                  <div class="flex flex-wrap gap-1.5">
                    {#each recentApps
                      .filter(app => !appSearchQuery || app.toLowerCase().includes(appSearchQuery.toLowerCase()))
                      .slice(0, showAllRecentApps ? undefined : 12) as app}
                      <button
                        type="button"
                        on:click={() => toggleBatchApp(app)}
                        class="settings-chip-button {batchSelectedApps.has(app) ? 'settings-chip-button-active' : ''}"
                        aria-pressed={batchSelectedApps.has(app)}
                      >
                        {batchSelectedApps.has(app) ? '✓ ' : ''}{app}
                      </button>
                    {/each}
                  </div>
                  {#if recentApps.length > 12 && !appSearchQuery}
                    <button
                      type="button"
                      on:click={() => showAllRecentApps = !showAllRecentApps}
                      class="settings-link-action mt-1"
                    >
                      {showAllRecentApps
                        ? t('settingsPrivacy.collapse')
                        : `+${recentApps.length - 12} ${t('settingsPrivacy.moreApps')}`}
                    </button>
                  {/if}
                </div>
              {/if}
            </div>
          {/if}

          <div class="settings-field">
            <span id="settings-privacy-strategy-label" class="settings-label">
              {t('settingsPrivacy.strategy')}
            </span>
            <div
              class="settings-privacy-strategy-segments flex overflow-hidden rounded-lg border border-slate-200 dark:border-[#484f58]"
              role="radiogroup"
              aria-labelledby="settings-privacy-strategy-label"
            >
              {#each privacyLevels as level}
                <button
                  type="button"
                  on:click={() => selectedLevel = level.value}
                  class="settings-segment-base {selectedLevel === level.value ? 'settings-segment-active' : ''}"
                  role="radio"
                  aria-checked={selectedLevel === level.value}
                >
                  {level.label}
                </button>
              {/each}
            </div>
            <p class="settings-muted">
              {getPrivacyLevel(selectedLevel)?.desc || ''}
            </p>
          </div>

          <div class="settings-actions settings-privacy-rule-editor-actions">
            <button
              type="button"
              on:click={cancelAppRuleEditor}
              class="settings-action-secondary"
            >
              {t('common.cancel')}
            </button>
            <button
              type="button"
              on:click={addAppRule}
              class="settings-action-primary"
              disabled={!selectedApp && batchSelectedApps.size === 0}
            >
              {batchSelectedApps.size > 1
                ? t('settingsPrivacy.batchAdd', { count: batchSelectedApps.size })
                : t('settingsPrivacy.addRuleAction')}
            </button>
          </div>
        </div>
      {/if}

      {#if config.privacy.app_rules.length > 0}
        <ul class="settings-privacy-rule-list mt-3 divide-y divide-slate-200/70 dark:divide-[#30363d]/70">
          {#each config.privacy.app_rules as rule, i}
            <li class="settings-privacy-rule-row flex flex-wrap items-center gap-x-3 gap-y-1 py-2.5">
              <span class="settings-privacy-rule-app settings-text min-w-0 flex-1">{rule.app_name}</span>
              <span class="settings-privacy-rule-policy settings-muted inline-flex items-center gap-1.5">
                <span class="settings-privacy-status-dot" aria-hidden="true"></span>
                {getPrivacyLevel(rule.level)?.label || rule.level}
              </span>
              <button
                type="button"
                on:click={() => removeAppRule(i)}
                class="settings-link-danger settings-privacy-rule-delete shrink-0"
                aria-label={`${t('settingsPrivacy.delete')} ${rule.app_name}`}
              >
                {t('settingsPrivacy.delete')}
              </button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="settings-empty">{t('settingsPrivacy.noRules')}</p>
      {/if}
    </section>

    <section
      class="settings-block settings-privacy-browser-protection"
      aria-labelledby="settings-privacy-browser-protection-title"
    >
      <h4 id="settings-privacy-browser-protection-title" class="settings-text mb-3">
        {t('settingsPrivacy.browserProtectionTitle')}
      </h4>

      <div class="settings-row">
        <div class="min-w-0">
          <span class="settings-text">{t('settingsPrivacy.skipPrivateWindows')}</span>
          <p class="settings-muted mt-0.5">{t('settingsPrivacy.skipPrivateWindowsHint')}</p>
        </div>
        <button
          type="button"
          role="switch"
          aria-label={t('settingsPrivacy.skipPrivateWindows')}
          aria-checked={skipPrivateWindows}
          on:click={toggleSkipPrivateWindows}
          class="switch-track {skipPrivateWindows ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
        >
          <span class="switch-thumb {skipPrivateWindows ? 'translate-x-5' : 'translate-x-0'}"></span>
        </button>
      </div>

      <div class="mt-4 space-y-2">
        <span id="settings-privacy-unknown-url-label" class="settings-text block">
          {t('settingsPrivacy.unknownUrlFallback')}
        </span>
        <p class="settings-muted">{t('settingsPrivacy.unknownUrlFallbackHint')}</p>
        <div
          class="settings-privacy-strategy-segments"
          role="group"
          aria-labelledby="settings-privacy-unknown-url-label"
        >
          {#each unknownUrlFallbackOptions as opt}
            <button
              type="button"
              aria-pressed={unknownUrlFallbackChoice === opt.value}
              on:click={() => setUnknownUrlFallback(opt.value)}
              class="settings-segment-base {unknownUrlFallbackChoice === opt.value ? 'settings-segment-active' : ''}"
            >
              {t(opt.labelKey)}
            </button>
          {/each}
        </div>
        <p class="settings-note">
          {unknownUrlFallbackChoice === 'auto'
            ? t('settingsPrivacy.unknownUrlFallbackAutoDesc')
            : unknownUrlFallbackChoice === 'anonymize'
              ? t('settingsPrivacy.unknownUrlFallbackAnonymizeDesc')
              : t('settingsPrivacy.unknownUrlFallbackRecordDesc')}
        </p>
      </div>
    </section>

    <section
      class="settings-block settings-privacy-content-filter"
      aria-labelledby="settings-privacy-content-filter-title"
    >
      <button
        type="button"
        on:click={() => showContentFilter = !showContentFilter}
        class="settings-privacy-content-filter-toggle flex w-full items-start justify-between gap-4 text-start"
        aria-expanded={showContentFilter}
        aria-controls="settings-privacy-content-filter-panel"
      >
        <span class="min-w-0">
          <span id="settings-privacy-content-filter-title" class="settings-text block">
            {t('settingsPrivacy.contentFilter')}
          </span>
          <span class="settings-muted mt-1 block">
            {t('settingsPrivacy.contentFilterSummary', { keywordCount, domainCount })}
          </span>
        </span>
        <svg
          class="mt-0.5 h-4 w-4 shrink-0 text-slate-400 transition-transform {showContentFilter ? 'rotate-180' : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
          aria-hidden="true"
        >
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
        </svg>
      </button>
      <p class="settings-muted">{t('settingsPrivacy.contentFilterDesc')}</p>

      {#if showContentFilter}
        <div
          id="settings-privacy-content-filter-panel"
          class="settings-privacy-content-filter-panel space-y-5 pt-3"
          role="region"
          aria-labelledby="settings-privacy-content-filter-title"
        >
          <div class="settings-privacy-filter-group space-y-2">
            <div class="flex items-center justify-between gap-3">
              <span class="settings-label">{t('settingsPrivacy.keywords')}</span>
              <button
                type="button"
                on:click={() => showKeywordInput = !showKeywordInput}
                class="settings-link-action"
                aria-expanded={showKeywordInput}
              >
                {showKeywordInput ? t('settingsPrivacy.collapse') : t('settingsPrivacy.add')}
              </button>
            </div>

            {#if showKeywordInput}
              <div class="settings-privacy-inline-input flex gap-2 animate-fadeIn">
                <input
                  type="text"
                  bind:value={newKeyword}
                  class="control-input flex-1"
                  placeholder={t('settingsPrivacy.keywordPlaceholder')}
                  aria-label={t('settingsPrivacy.keywordPlaceholder')}
                  on:keydown={handleKeywordKeydown}
                />
                <button type="button" on:click={addKeyword} class="settings-action-primary">
                  {t('common.add')}
                </button>
              </div>
            {/if}

            <div class="settings-privacy-filter-values flex flex-wrap gap-1.5">
              {#each config.privacy.excluded_keywords as keyword, i}
                <div class="settings-chip-neutral settings-privacy-filter-value">
                  <span>{keyword}</span>
                  <button
                    type="button"
                    on:click={() => removeKeyword(i)}
                    class="settings-link-danger settings-privacy-filter-delete ml-1.5"
                    aria-label={`${t('settingsPrivacy.delete')} ${keyword}`}
                  >
                    ×
                  </button>
                </div>
              {/each}
              {#if config.privacy.excluded_keywords.length === 0}
                <span class="settings-subtle">{t('settingsPrivacy.noKeywords')}</span>
              {/if}
            </div>
            <p class="settings-note">{t('settingsPrivacy.keywordHint')}</p>
          </div>

          <div class="settings-privacy-filter-group space-y-2">
            <div class="flex items-center justify-between gap-3">
              <span class="settings-label">{t('settingsPrivacy.domainBlacklist')}</span>
              <button
                type="button"
                on:click={() => showDomainInput = !showDomainInput}
                class="settings-link-action"
                aria-expanded={showDomainInput}
              >
                {showDomainInput ? t('settingsPrivacy.collapse') : t('settingsPrivacy.add')}
              </button>
            </div>

            {#if showDomainInput}
              <div class="settings-privacy-inline-input flex gap-2 animate-fadeIn">
                <input
                  type="text"
                  bind:value={newDomain}
                  class="control-input flex-1"
                  placeholder={t('settingsPrivacy.domainPlaceholder')}
                  aria-label={t('settingsPrivacy.domainPlaceholder')}
                  on:keydown={handleDomainKeydown}
                />
                <button type="button" on:click={addDomain} class="settings-action-primary">
                  {t('common.add')}
                </button>
              </div>
            {/if}

            <div class="settings-privacy-filter-values flex flex-wrap gap-1.5">
              {#each (config.privacy.excluded_domains || []) as domain, i}
                <div class="settings-chip-neutral settings-privacy-filter-value">
                  <span>{domain}</span>
                  <button
                    type="button"
                    on:click={() => removeDomain(i)}
                    class="settings-link-danger settings-privacy-filter-delete ml-1.5"
                    aria-label={`${t('settingsPrivacy.delete')} ${domain}`}
                  >
                    ×
                  </button>
                </div>
              {/each}
              {#if (config.privacy.excluded_domains || []).length === 0}
                <span class="settings-subtle">{t('settingsPrivacy.noDomains')}</span>
              {/if}
            </div>
            <p class="settings-note">{t('settingsPrivacy.domainHint')}</p>
          </div>
        </div>
      {/if}
    </section>
  </div>
</div>
