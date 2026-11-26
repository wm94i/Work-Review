<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import type { ComponentProps } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { cache } from '../../lib/stores/cache.ts';
  import { locale, t } from '$lib/i18n/index.ts';
  import { formatUserError } from '$lib/utils/errorDisplay.ts';
  import { showToast } from '../../lib/stores/toast.ts';
  import { resolveActiveSettingsSection } from './settingsScrollSpy.ts';

  import SettingsGeneral from './components/SettingsGeneral.svelte';
  import SettingsAppearance from './components/SettingsAppearance.svelte';
  import SettingsAI from './components/SettingsAI.svelte';
  import SettingsAvatar from './components/SettingsAvatar.svelte';
  import SettingsNodeGateway from './components/SettingsNodeGateway.svelte';
  import SettingsSystem from './components/SettingsSystem.svelte';
  import SettingsPrivacy from './components/SettingsPrivacy.svelte';
  import SettingsProjects from './components/SettingsProjects.svelte';
  import SettingsStorage from './components/SettingsStorage.svelte';

  type GeneralConfig = ComponentProps<SettingsGeneral>['config'];
  type AppearanceConfig = ComponentProps<SettingsAppearance>['config'];
  type AiConfig = ComponentProps<SettingsAI>['config'];
  type NodeGatewayConfig = ComponentProps<SettingsNodeGateway>['config'];
  type PrivacyConfig = ComponentProps<SettingsPrivacy>['config'];
  type ProjectsConfig = ComponentProps<SettingsProjects>['config'];
  type StorageConfig = ComponentProps<SettingsStorage>['config'];
  type AiProvider = NonNullable<ComponentProps<SettingsAI>['providers']>[number];
  type StorageStats = NonNullable<ComponentProps<SettingsStorage>['storageStats']>;

  interface LegacyModelConfig {
    provider: string;
    endpoint: string;
    api_key: string | null;
    model: string;
    vision_model?: string;
  }

  interface RootSettingsFields {
    ai_provider: LegacyModelConfig;
    daily_report_custom_prompt: string;
    daily_report_prompt_presets: unknown[];
    vision_model: LegacyModelConfig;
    app_category_rules: unknown[];
    privacy: PrivacyConfig['privacy'] & { sensitive_keywords?: string[] };
    storage: StorageConfig['storage'] & {
      metadata_retention_days: number;
      jpeg_quality: number;
    };
  }

  type SettingsConfig = GeneralConfig
    & AppearanceConfig
    & AiConfig
    & NodeGatewayConfig
    & PrivacyConfig
    & ProjectsConfig
    & StorageConfig
    & RootSettingsFields;

  type NullableCredentialKey =
    | 'telegram_bot_token'
    | 'telegram_bot_proxy'
    | 'feishu_app_id'
    | 'feishu_app_secret'
    | 'feishu_verification_token'
    | 'feishu_encrypt_key'
    | 'wecom_corp_id'
    | 'wecom_token'
    | 'wecom_encoding_aes_key'
    | 'wecom_bot_id'
    | 'wecom_bot_secret'
    | 'dingtalk_app_secret';

  type DraftPrivacySettings = Partial<PrivacyConfig['privacy']> & {
    sensitive_keywords?: string[];
  };

  type DraftSettingsConfig = Omit<
    SettingsConfig,
    | NullableCredentialKey
    | 'ai_provider'
    | 'text_model'
    | 'text_model_profiles'
    | 'daily_report_custom_prompt'
    | 'daily_report_prompt_presets'
    | 'node_gateway'
    | 'vision_model'
    | 'storage'
    | 'app_category_rules'
    | 'privacy'
    | 'ui_visual_style'
  > & {
    [Key in NullableCredentialKey]: string | null;
  } & {
    ai_provider: LegacyModelConfig | null;
    text_model: AiConfig['text_model'] | null;
    text_model_profiles?: AiConfig['text_model_profiles'];
    daily_report_custom_prompt?: string;
    daily_report_prompt_presets?: unknown[];
    node_gateway: (Partial<NodeGatewayConfig['node_gateway']> & { device_name: string | null }) | null;
    vision_model: LegacyModelConfig | null;
    storage: (Partial<RootSettingsFields['storage']> & {
      screenshots_enabled?: boolean;
      screenshot_display_mode?: string;
      screenshot_width_mode?: string;
    }) | null;
    app_category_rules?: unknown[];
    privacy: DraftPrivacySettings | null;
    ui_visual_style: string;
  };

  type SettingsTabId = 'general' | 'appearance' | 'ai' | 'avatar' | 'projects' | 'privacy' | 'storage' | 'node';

  interface SettingsTab {
    id: SettingsTabId;
    labelKey: string;
    icon: SettingsTabId;
    beta?: boolean;
  }

  let config: SettingsConfig | null = null;
  let loading = true;
  let saving = false;
  let dirty = false;
  let restoringBackup = false;
  let saveConfigPromise: Promise<void> | null = null;
  let error: string | null = null;
  let success = false;
  let providers: AiProvider[] = [];
  let runningApps: string[] = [];
  let recentApps: string[] = [];
  let storageStats: StorageStats | null = null;
  let dataDir = '';
  let defaultDataDir = '';
  let settingsRuntimePlatform = '';
  let successTimer: ReturnType<typeof setTimeout> | null = null;
  $: currentLocale = $locale;

  // 当前激活的导航项（连续滚动时随视口自动高亮，点击导航可平滑定位）
  let activeTab: SettingsTabId = 'general';
  let tabRailEl: HTMLElement | null = null;
  let stageShellEl: HTMLElement | null = null;
  let activeScrollContainer: HTMLElement | null = null;
  let scrollLockTarget: SettingsTabId | null = null;
  let scrollLockTimer: ReturnType<typeof setTimeout> | null = null;

  let scrollLockRafId: number | null = null;

  const SCROLL_SPY_ACTIVATION_OFFSET = 120;
  const SCROLL_LOCK_DURATION_MS = 700;

  function clearScrollLock() {
    scrollLockTarget = null;
    if (scrollLockTimer !== null) {
      clearTimeout(scrollLockTimer);
      scrollLockTimer = null;
    }
    if (scrollLockRafId !== null && typeof cancelAnimationFrame === 'function') {
      cancelAnimationFrame(scrollLockRafId);
      scrollLockRafId = null;
    }
  }

  function isHorizontalTabRail(): boolean {
    if (!tabRailEl || typeof window === 'undefined' || typeof window.getComputedStyle !== 'function') {
      return false;
    }
    return window.getComputedStyle(tabRailEl).flexDirection === 'row';
  }

  function getStickyScrollTopOffset(): number {
    if (tabRailEl && isHorizontalTabRail()) {
      return Math.ceil(tabRailEl.getBoundingClientRect().height) + 12;
    }
    return 16;
  }

  function keepActiveNavItemVisible(tabId: SettingsTabId) {
    if (!tabRailEl) return;
    const activeButton = tabRailEl.querySelector<HTMLElement>(`[data-settings-nav="${tabId}"]`);
    if (!activeButton) return;

    // 仅在导航容器内部按需滚动，避免 scrollIntoView 误触发外层主滚动区回弹
    const railRect = tabRailEl.getBoundingClientRect();
    const btnRect = activeButton.getBoundingClientRect();
    if (tabRailEl.scrollWidth > tabRailEl.clientWidth + 1) {
      if (btnRect.left < railRect.left) {
        tabRailEl.scrollLeft -= railRect.left - btnRect.left + 8;
      } else if (btnRect.right > railRect.right) {
        tabRailEl.scrollLeft += btnRect.right - railRect.right + 8;
      }
    }
    if (tabRailEl.scrollHeight > tabRailEl.clientHeight + 1) {
      if (btnRect.top < railRect.top) {
        tabRailEl.scrollTop -= railRect.top - btnRect.top + 8;
      } else if (btnRect.bottom > railRect.bottom) {
        tabRailEl.scrollTop += btnRect.bottom - railRect.bottom + 8;
      }
    }
  }

  function syncActiveSectionFromScroll() {
    if (!stageShellEl || scrollLockTarget !== null) return;

    const sectionEls = Array.from(
      stageShellEl.querySelectorAll<HTMLElement>('[data-settings-section]'),
    );
    if (sectionEls.length === 0) return;

    const containerTop = activeScrollContainer
      ? activeScrollContainer.getBoundingClientRect().top
      : 0;
    const measurements = sectionEls
      .map((el) => ({
        id: el.dataset.settingsSection as SettingsTabId,
        top: el.getBoundingClientRect().top - containerTop,
      }))
      .filter((item) => Boolean(item.id));

    const atBottom = activeScrollContainer
      ? activeScrollContainer.scrollHeight > activeScrollContainer.clientHeight + 8
        && activeScrollContainer.scrollTop + activeScrollContainer.clientHeight
          >= activeScrollContainer.scrollHeight - 12
      : typeof document !== 'undefined'
        && document.documentElement.scrollHeight > window.innerHeight + 8
        && window.scrollY + window.innerHeight >= document.documentElement.scrollHeight - 12;

    const stickyExtra = isHorizontalTabRail() ? getStickyScrollTopOffset() : 0;
    const nextActive = resolveActiveSettingsSection(measurements, {
      activationOffset: SCROLL_SPY_ACTIVATION_OFFSET + stickyExtra,
      atBottom,
      fallback: activeTab,
    });

    if (nextActive !== activeTab) {
      activeTab = nextActive;
      keepActiveNavItemVisible(nextActive);
    }
  }

  function scrollToSection(tabId: SettingsTabId) {
    activeTab = tabId;
    keepActiveNavItemVisible(tabId);

    if (!stageShellEl) return;
    const target = stageShellEl.querySelector<HTMLElement>(`[data-settings-section="${tabId}"]`);
    if (!target) return;

    const prefersReducedMotion = typeof window !== 'undefined'
      && typeof window.matchMedia === 'function'
      && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const behavior: ScrollBehavior = prefersReducedMotion ? 'auto' : 'smooth';

    clearScrollLock();
    scrollLockTarget = tabId;
    if (behavior === 'auto' && typeof requestAnimationFrame === 'function') {
      scrollLockRafId = requestAnimationFrame(() => {
        scrollLockTarget = null;
        scrollLockRafId = null;
      });
    } else {
      scrollLockTimer = setTimeout(() => {
        scrollLockTarget = null;
        scrollLockTimer = null;
      }, SCROLL_LOCK_DURATION_MS);
    }

    const topOffset = getStickyScrollTopOffset();
    if (activeScrollContainer && typeof activeScrollContainer.scrollTo === 'function') {
      const containerRect = activeScrollContainer.getBoundingClientRect();
      const targetRect = target.getBoundingClientRect();
      const nextTop = Math.max(
        0,
        activeScrollContainer.scrollTop + (targetRect.top - containerRect.top) - topOffset,
      );
      activeScrollContainer.scrollTo({ top: nextTop, behavior });
    } else if (typeof window !== 'undefined' && typeof window.scrollTo === 'function') {
      const nextTop = Math.max(
        0,
        window.scrollY + target.getBoundingClientRect().top - topOffset,
      );
      window.scrollTo({ top: nextTop, behavior });
    } else if (typeof target.scrollIntoView === 'function') {
      target.scrollIntoView({ behavior, block: 'start' });
    }
  }

  const SCROLL_INTERRUPT_KEYS = new Set([
    'ArrowDown',
    'ArrowUp',
    'PageDown',
    'PageUp',
    'Home',
    'End',
    ' ',
  ]);

  function attachScrollSpy(node: HTMLElement) {
    stageShellEl = node;
    tabRailEl = node.parentElement?.querySelector<HTMLElement>('.settings-tab-rail') ?? null;
    activeScrollContainer = node.closest<HTMLElement>('.app-shell-main-scroll');
    const scrollTarget: HTMLElement | Window = activeScrollContainer ?? window;
    let rafId: number | null = null;

    const cancelScheduledSync = () => {
      if (rafId !== null) {
        cancelAnimationFrame(rafId);
        rafId = null;
      }
    };

    const scheduleSync = () => {
      // 点击定位锁定期间禁止入队 rAF，防止最后一帧 scroll → scrollend 解除锁 → 旧 rAF 误回写
      if (scrollLockTarget !== null || rafId !== null) return;
      rafId = requestAnimationFrame(() => {
        rafId = null;
        syncActiveSectionFromScroll();
      });
    };

    const handleUserInterrupt = () => {
      if (scrollLockTarget !== null) {
        clearScrollLock();
        scheduleSync();
      }
    };

    const handlePointerInterrupt = (event: Event) => {
      if (scrollLockTarget === null) return;
      if (tabRailEl && event.target instanceof Node && tabRailEl.contains(event.target)) {
        return;
      }
      handleUserInterrupt();
    };

    const handleKeyInterrupt = (event: KeyboardEvent) => {
      if (scrollLockTarget === null || !SCROLL_INTERRUPT_KEYS.has(event.key)) return;
      const targetEl = event.target as HTMLElement | null;
      if (
        targetEl
        && (targetEl.tagName === 'INPUT'
          || targetEl.tagName === 'TEXTAREA'
          || targetEl.tagName === 'SELECT'
          || targetEl.isContentEditable)
      ) {
        return;
      }
      handleUserInterrupt();
    };

    const handleScrollEnd = () => {
      if (scrollLockTarget !== null) {
        // 先取消任何残余 rAF 再解除点击定位锁，保留用户显式选中的导航项，避免末尾较短分组被 atBottom 抢占
        cancelScheduledSync();
        clearScrollLock();
      }
    };

    let resizeObserver: ResizeObserver | null = null;
    if (typeof ResizeObserver === 'function') {
      resizeObserver = new ResizeObserver(() => {
        scheduleSync();
      });
      resizeObserver.observe(node);
    }

    scrollTarget.addEventListener('scroll', scheduleSync, { passive: true });
    scrollTarget.addEventListener('scrollend', handleScrollEnd, { passive: true });
    scrollTarget.addEventListener('wheel', handleUserInterrupt, { passive: true });
    scrollTarget.addEventListener('touchstart', handleUserInterrupt, { passive: true });
    scrollTarget.addEventListener('pointerdown', handlePointerInterrupt, { passive: true });
    window.addEventListener('keydown', handleKeyInterrupt, { passive: true });
    window.addEventListener('resize', scheduleSync, { passive: true });
    scheduleSync();

    return {
      destroy() {
        scrollTarget.removeEventListener('scroll', scheduleSync);
        scrollTarget.removeEventListener('scrollend', handleScrollEnd);
        scrollTarget.removeEventListener('wheel', handleUserInterrupt);
        scrollTarget.removeEventListener('touchstart', handleUserInterrupt);
        scrollTarget.removeEventListener('pointerdown', handlePointerInterrupt);
        window.removeEventListener('keydown', handleKeyInterrupt);
        window.removeEventListener('resize', scheduleSync);
        resizeObserver?.disconnect();
        cancelScheduledSync();
        clearScrollLock();
        if (stageShellEl === node) {
          stageShellEl = null;
          tabRailEl = null;
          activeScrollContainer = null;
        }
      },
    };
  }

  onDestroy(() => {
    clearScrollLock();
  });

  const tabs: SettingsTab[] = [
    { id: 'general', labelKey: 'settings.tabs.general', icon: 'general' },
    { id: 'appearance', labelKey: 'settings.tabs.appearance', icon: 'appearance' },
    { id: 'ai', labelKey: 'settings.tabs.ai', icon: 'ai' },
    { id: 'avatar', labelKey: 'settings.tabs.avatar', icon: 'avatar', beta: true },
    { id: 'projects', labelKey: 'settings.tabs.projects', icon: 'projects' },
    { id: 'privacy', labelKey: 'settings.tabs.privacy', icon: 'privacy' },
    { id: 'storage', labelKey: 'settings.tabs.storage', icon: 'storage' },
    { id: 'node', labelKey: 'settings.tabs.node', icon: 'node', beta: true },
  ];

  // 加载配置
  async function loadConfig() {
    loading = true;
    error = null;
    try {
      const [loadedConfig, loadedProviders, loadedStorageStats, loadedDataDir, loadedDefaultDataDir, loadedRuntimePlatform] = await Promise.all([
        invoke<DraftSettingsConfig>('get_config'),
        invoke<AiProvider[]>('get_ai_providers'),
        invoke<StorageStats>('get_storage_stats'),
        invoke<string>('get_data_dir'),
        invoke<string>('get_default_data_dir'),
        invoke<string>('get_runtime_platform'),
      ]);

      providers = loadedProviders;
      storageStats = loadedStorageStats;
      dataDir = loadedDataDir;
      defaultDataDir = loadedDefaultDataDir;
      settingsRuntimePlatform = loadedRuntimePlatform;

      // 确保对象存在
      if (!loadedConfig.ai_provider) {
        loadedConfig.ai_provider = { provider: 'ollama', endpoint: 'http://localhost:11434', api_key: null, model: 'llava', vision_model: 'llava' };
      }
      if (!loadedConfig.text_model) {
        loadedConfig.text_model = { provider: 'ollama', endpoint: 'http://localhost:11434', api_key: null, model: 'qwen2.5' };
      }
      if (!loadedConfig.text_model_profiles) {
        loadedConfig.text_model_profiles = [];
      }
      if (!loadedConfig.text_model_provider_cache) {
        loadedConfig.text_model_provider_cache = {};
      }
      if (!loadedConfig.task_model_routing || typeof loadedConfig.task_model_routing !== 'object') {
        loadedConfig.task_model_routing = {
          report_provider: null,
          assistant_provider: null,
          classification_provider: null,
          proactive_provider: null,
        };
      }
      if (typeof loadedConfig.daily_report_custom_prompt !== 'string') {
        loadedConfig.daily_report_custom_prompt = '';
      }
      if (typeof loadedConfig.daily_report_export_dir !== 'string' && loadedConfig.daily_report_export_dir !== null) {
        loadedConfig.daily_report_export_dir = null;
      }
      if (typeof loadedConfig.daily_report_auto_export !== 'boolean') {
        loadedConfig.daily_report_auto_export = false;
      }
      if (typeof loadedConfig.daily_report_export_frontmatter !== 'boolean') {
        loadedConfig.daily_report_export_frontmatter = false;
      }
      if (
        typeof loadedConfig.daily_report_export_section_heading !== 'string' &&
        loadedConfig.daily_report_export_section_heading !== null
      ) {
        loadedConfig.daily_report_export_section_heading = null;
      }
      if (typeof loadedConfig.git_report_enabled !== 'boolean') {
        loadedConfig.git_report_enabled = false;
      }
      if (!Array.isArray(loadedConfig.git_repo_paths)) {
        loadedConfig.git_repo_paths = [];
      }
      if (typeof loadedConfig.git_author_filter !== 'string' && loadedConfig.git_author_filter !== null) {
        loadedConfig.git_author_filter = null;
      }
      if (!Array.isArray(loadedConfig.daily_report_prompt_presets)) {
        loadedConfig.daily_report_prompt_presets = [];
      }
      if (typeof loadedConfig.localhost_api_enabled !== 'boolean') {
        loadedConfig.localhost_api_enabled = false;
      }
      if (!Number.isInteger(loadedConfig.localhost_api_port) || loadedConfig.localhost_api_port <= 0) {
        loadedConfig.localhost_api_port = 47831;
      }
      if (typeof loadedConfig.localhost_api_host !== 'string' && loadedConfig.localhost_api_host !== null) {
        loadedConfig.localhost_api_host = null;
      }
      if (!['a', 'b', 'c'].includes(loadedConfig.ui_visual_style)) {
        loadedConfig.ui_visual_style = 'c';
      }
      if (typeof loadedConfig.avatar_proactive_ai_enabled !== 'boolean') {
        loadedConfig.avatar_proactive_ai_enabled = false;
      }
      if (typeof loadedConfig.telegram_bot_enabled !== 'boolean') {
        loadedConfig.telegram_bot_enabled = false;
      }
      if (typeof loadedConfig.telegram_bot_token !== 'string' && loadedConfig.telegram_bot_token !== null) {
        loadedConfig.telegram_bot_token = null;
      }
      if (typeof loadedConfig.telegram_bot_proxy !== 'string' && loadedConfig.telegram_bot_proxy !== null) {
        loadedConfig.telegram_bot_proxy = null;
      }
      if (!Array.isArray(loadedConfig.node_devices)) {
        loadedConfig.node_devices = [];
      }
      if (typeof loadedConfig.feishu_bot_enabled !== 'boolean') {
        loadedConfig.feishu_bot_enabled = false;
      }
      if (typeof loadedConfig.feishu_app_id !== 'string' && loadedConfig.feishu_app_id !== null) {
        loadedConfig.feishu_app_id = null;
      }
      if (typeof loadedConfig.feishu_app_secret !== 'string' && loadedConfig.feishu_app_secret !== null) {
        loadedConfig.feishu_app_secret = null;
      }
      if (typeof loadedConfig.feishu_verification_token !== 'string' && loadedConfig.feishu_verification_token !== null) {
        loadedConfig.feishu_verification_token = null;
      }
      if (typeof loadedConfig.feishu_encrypt_key !== 'string' && loadedConfig.feishu_encrypt_key !== null) {
        loadedConfig.feishu_encrypt_key = null;
      }
      if (typeof loadedConfig.wecom_bot_enabled !== 'boolean') {
        loadedConfig.wecom_bot_enabled = false;
      }
      if (typeof loadedConfig.wecom_corp_id !== 'string' && loadedConfig.wecom_corp_id !== null) {
        loadedConfig.wecom_corp_id = null;
      }
      if (typeof loadedConfig.wecom_token !== 'string' && loadedConfig.wecom_token !== null) {
        loadedConfig.wecom_token = null;
      }
      if (typeof loadedConfig.wecom_encoding_aes_key !== 'string' && loadedConfig.wecom_encoding_aes_key !== null) {
        loadedConfig.wecom_encoding_aes_key = null;
      }
      if (typeof loadedConfig.wecom_bot_id !== 'string' && loadedConfig.wecom_bot_id !== null) {
        loadedConfig.wecom_bot_id = null;
      }
      if (typeof loadedConfig.wecom_bot_secret !== 'string' && loadedConfig.wecom_bot_secret !== null) {
        loadedConfig.wecom_bot_secret = null;
      }
      if (typeof loadedConfig.dingtalk_bot_enabled !== 'boolean') {
        loadedConfig.dingtalk_bot_enabled = false;
      }
      if (typeof loadedConfig.dingtalk_app_secret !== 'string' && loadedConfig.dingtalk_app_secret !== null) {
        loadedConfig.dingtalk_app_secret = null;
      }
      if (typeof loadedConfig.semantic_auto_index !== 'boolean') {
        loadedConfig.semantic_auto_index = false;
      }
      if (typeof loadedConfig.bot_natural_language_enabled !== 'boolean') {
        loadedConfig.bot_natural_language_enabled = false;
      }
      if (
        !loadedConfig.node_gateway ||
        typeof loadedConfig.node_gateway !== 'object' ||
        Array.isArray(loadedConfig.node_gateway)
      ) {
        loadedConfig.node_gateway = {
          device_name: null,
        };
      }
      if (
        typeof loadedConfig.node_gateway.device_name !== 'string' &&
        loadedConfig.node_gateway.device_name !== null
      ) {
        loadedConfig.node_gateway.device_name = null;
      }
      if (
        !loadedConfig.vision_model ||
        typeof loadedConfig.vision_model !== 'object' ||
        Array.isArray(loadedConfig.vision_model)
      ) {
        loadedConfig.vision_model = { provider: 'ollama', endpoint: 'http://localhost:11434', api_key: null, model: 'llava' };
      } else {
        if (typeof loadedConfig.vision_model.provider !== 'string' || !loadedConfig.vision_model.provider.trim()) {
          loadedConfig.vision_model.provider = 'ollama';
        }
        if (typeof loadedConfig.vision_model.endpoint !== 'string') {
          loadedConfig.vision_model.endpoint = 'http://localhost:11434';
        }
        if (typeof loadedConfig.vision_model.model !== 'string') {
          loadedConfig.vision_model.model = 'llava';
        }
        if (typeof loadedConfig.vision_model.api_key !== 'string' && loadedConfig.vision_model.api_key !== null) {
          loadedConfig.vision_model.api_key = null;
        }
      }
      if (typeof loadedConfig.lightweight_mode !== 'boolean') {
        loadedConfig.lightweight_mode = false;
      }
      if (typeof loadedConfig.break_reminder_enabled !== 'boolean') {
        loadedConfig.break_reminder_enabled = false;
      }
      if (![30, 45, 50, 60, 90, 120].includes(loadedConfig.break_reminder_interval_minutes)) {
        loadedConfig.break_reminder_interval_minutes = 50;
      }
      if (typeof loadedConfig.auto_start_silent !== 'boolean') {
        loadedConfig.auto_start_silent = false;
      }
      if (!loadedConfig.storage) {
        loadedConfig.storage = {
          screenshot_retention_days: 7,
          metadata_cleanup_enabled: false,
          metadata_retention_days: 30,
          storage_limit_mb: 2048,
          jpeg_quality: 85,
          max_image_width: 1280,
          screenshots_enabled: true,
          screenshot_display_mode: 'active_window',
          screenshot_width_mode: 'auto',
        };
      }
      if (typeof loadedConfig.storage.metadata_cleanup_enabled !== 'boolean') {
        loadedConfig.storage.metadata_cleanup_enabled = false;
      }
      if (typeof loadedConfig.storage.screenshots_enabled !== 'boolean') {
        loadedConfig.storage.screenshots_enabled = true;
      }
      if (!loadedConfig.storage.screenshot_display_mode) {
        loadedConfig.storage.screenshot_display_mode = 'active_window';
      }
      if (!loadedConfig.storage.screenshot_width_mode || !['auto', 'fixed'].includes(loadedConfig.storage.screenshot_width_mode)) {
        loadedConfig.storage.screenshot_width_mode = 'auto';
      }
      if (!loadedConfig.app_category_rules) loadedConfig.app_category_rules = [];
      if (!Array.isArray(loadedConfig.projects)) loadedConfig.projects = [];
      if (!loadedConfig.privacy) loadedConfig.privacy = {};
      if (!loadedConfig.privacy.app_rules) loadedConfig.privacy.app_rules = [];
      if (!loadedConfig.privacy.excluded_keywords) loadedConfig.privacy.excluded_keywords = [];
      if (typeof loadedConfig.privacy.skip_private_windows !== 'boolean') {
        loadedConfig.privacy.skip_private_windows = true;
      }
      if (
        loadedConfig.privacy.unknown_url_fallback !== 'record' &&
        loadedConfig.privacy.unknown_url_fallback !== 'anonymize'
      ) {
        loadedConfig.privacy.unknown_url_fallback = null;
      }
      delete loadedConfig.privacy.sensitive_keywords;

      config = loadedConfig as SettingsConfig;
      cache.setConfig(config);
    } catch (e) {
      error = formatUserError(e, t('common.loadFailedRetry'));
      console.error('加载配置失败:', e);
      settingsRuntimePlatform = '';
    } finally {
      loading = false;
    }
  }

  // 加载运行中的应用
  async function loadRunningApps() {
    try {
      runningApps = await invoke<string[]>('get_running_apps');
    } catch (e) {
      console.error('获取运行应用失败:', e);
      runningApps = [];
    }
  }

  // 加载历史应用列表
  async function loadRecentApps() {
    try {
      recentApps = await invoke<string[]>('get_recent_apps');
    } catch (e) {
      console.error('获取历史应用失败:', e);
      recentApps = [];
    }
  }

  // 保存配置
  async function saveConfig() {
    if (!config || restoringBackup) return;
    if (saveConfigPromise) return saveConfigPromise;
    const currentConfig = config;
    saving = true;
    error = null;
    success = false;

    const run = (async () => {
      try {
        const rawInterval = Number(currentConfig.screenshot_interval);
        currentConfig.screenshot_interval = Number.isFinite(rawInterval)
          ? Math.max(5, Math.min(600, Math.round(rawInterval)))
          : 30;
        delete currentConfig.privacy?.sensitive_keywords;
        await invoke<void>('save_config', { config: currentConfig });
        success = true;
        dirty = false;
        cache.setConfig(currentConfig);
        showToast(t('settings.saveSuccessToast'), 'success');

        if (successTimer !== null) clearTimeout(successTimer);
        successTimer = setTimeout(() => {
          success = false;
          successTimer = null;
        }, 3000);
      } catch (e) {
        error = formatUserError(e, t('common.loadFailedRetry'));
      } finally {
        saving = false;
      }
    })();

    saveConfigPromise = run;
    try {
      await run;
    } finally {
      if (saveConfigPromise === run) {
        saveConfigPromise = null;
      }
    }
  }

  async function waitForPendingSave(): Promise<void> {
    if (saveConfigPromise) {
      try {
        await saveConfigPromise;
      } catch {
        // 保存失败已在 saveConfig 内处理
      }
    }
  }

  function handleSettingsChange(event: CustomEvent<unknown>) {
    if (restoringBackup) return;
    const detail = event.detail;
    const autosaved = typeof detail === 'object'
      && detail !== null
      && 'autosaved' in detail
      && detail.autosaved === true;
    if (!autosaved) {
      dirty = true;
    }
  }

  function handleChildToast(event: CustomEvent<{ message: string; type: 'success' | 'error' }>) {
    showToast(event.detail.message, event.detail.type);
  }

  // 清理缓存回调
  async function handleClearCache() {
    try {
      const [latestStats, latestDataDir] = await Promise.all([
        invoke<StorageStats>('get_storage_stats'),
        invoke<string>('get_data_dir'),
      ]);
      storageStats = latestStats;
      dataDir = latestDataDir;
    } catch (e) {
      console.error('刷新存储状态失败:', e);
    }
  }

  async function handleDataDirChanged() {
    try {
      const [latestStats, latestDataDir] = await Promise.all([
        invoke<StorageStats>('get_storage_stats'),
        invoke<string>('get_data_dir'),
      ]);
      storageStats = latestStats;
      dataDir = latestDataDir;
      cache.clear();
    } catch (e) {
      console.error('切换数据目录后刷新状态失败:', e);
    }
  }

  function handleRestoreStateChange(event: CustomEvent<{ restoring: boolean }>) {
    restoringBackup = event.detail.restoring;
  }

  async function handleBackupRestored() {
    dirty = false;
    restoringBackup = true;
    cache.clear();
    await loadConfig();
  }

  onMount(() => {
    const unsubscribeCache = cache.subscribe((state) => {
      if (!state.config) return;
      // 保存中或用户已编辑配置时，不覆盖（避免丢弃未保存的修改）
      if (saving) return;
      if (config && dirty) return;
      config = state.config as SettingsConfig;
    });

    loadConfig();
    loadRunningApps();
    loadRecentApps();

    return () => {
      unsubscribeCache();
      if (successTimer !== null) clearTimeout(successTimer);
    };
  });
</script>

<div class="page-shell settings-editorial-shell" data-locale={currentLocale}>
  <div class="page-header page-axis-operation">
    <div class="page-title-group">
      <div class="page-title-badge">
        <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
        </svg>
      </div>
      <div class="page-title-copy">
        <h2>{t('settings.title')}</h2>
        <p>{t('settings.subtitle')}</p>
      </div>
    </div>

    <!-- 保存按钮 -->
    <div class="settings-save-dock">
      <button
        on:click={saveConfig}
        disabled={loading || saving || restoringBackup}
        class="settings-action-primary px-4 rounded-xl"
      >
        {#if saving}
          <div class="animate-spin rounded-full h-4 w-4 border-2 border-white border-t-transparent"></div>
          {t('settings.saving')}
        {:else if success}
          <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" /></svg>
          {t('settings.saved')}
        {:else}
          {t('settings.save')}
        {/if}
      </button>
    </div>
  </div>

  {#if loading}
    <div class="flex justify-center py-12">
      <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary-500"></div>
    </div>
  {:else if error}
    <div class="page-banner-error mb-6">
      <div>
        <p class="font-semibold">{t('settings.loadError')}</p>
        <p class="text-sm mt-1">{error}</p>
      </div>
      <button on:click={loadConfig} class="page-action-brand">{t('settings.retry')}</button>
    </div>
  {:else if config}
    <div class="w-full settings-editorial-board page-axis-operation">
      {#if settingsRuntimePlatform === 'macos'}
        <div class="settings-card settings-top-status-zone">
          <SettingsSystem />
        </div>
      {/if}

      <div class="settings-stage-layout">
        <nav class="settings-tab-rail" aria-label={t('settings.title')}>
          {#each tabs as tab}
            <button
              type="button"
              data-settings-nav={tab.id}
              on:click={() => scrollToSection(tab.id)}
              class="settings-tab-rail-item {activeTab === tab.id ? 'settings-tab-rail-item-active' : ''}"
              aria-current={activeTab === tab.id ? 'page' : undefined}
            >
              <span class="settings-tab-rail-icon">
                {#if tab.icon === 'general'}
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" /></svg>
                {:else if tab.icon === 'appearance'}
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 3.75a8.25 8.25 0 100 16.5h1.15a1.85 1.85 0 001.05-3.37 1.15 1.15 0 01.64-2.1h1.01A4.4 4.4 0 0020.25 10.4 6.65 6.65 0 0013.6 3.75H12z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M7.8 10.1h.01M9.75 7.4h.01M13 7.05h.01M15.9 9.25h.01" /></svg>
                {:else if tab.icon === 'ai'}
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" /></svg>
                {:else if tab.icon === 'node'}
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M5 7h14M5 12h14M5 17h10" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4 5a2 2 0 012-2h12a2 2 0 012 2v14a2 2 0 01-2 2H6a2 2 0 01-2-2V5z" /></svg>
                {:else if tab.icon === 'avatar'}
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M7.5 9.5l1.5-3 3 2 3-2 1.5 3M7 14.5c0-2.5 2.239-4.5 5-4.5s5 2 5 4.5S14.761 19 12 19s-5-2-5-4.5z" /><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M10 14h.01M14 14h.01M10.5 16.5c.6.5 1 .75 1.5.75s.9-.25 1.5-.75" /></svg>
                {:else if tab.icon === 'projects'}
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2V7z" /></svg>
                {:else if tab.icon === 'privacy'}
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" /></svg>
                {:else if tab.icon === 'storage'}
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" /></svg>
                {/if}
              </span>
              <span class="inline-flex items-center gap-1 whitespace-nowrap">
                <span>{t(tab.labelKey)}</span>
                {#if tab.beta}
                  <span class="inline-flex items-center rounded-full border border-amber-200 bg-amber-50 px-1 py-px text-[10px] font-semibold uppercase tracking-[0.06em] text-amber-700 dark:border-amber-500/30 dark:bg-amber-500/10 dark:text-amber-200">
                    {t('settings.tabs.beta')}
                  </span>
                {/if}
              </span>
            </button>
          {/each}
        </nav>

        <div class="settings-stage-shell" use:attachScrollSpy>
          <section
            id="settings-section-general"
            data-settings-section="general"
            class="settings-scroll-section"
            class:settings-scroll-section-active={activeTab === 'general'}
          >
            <div class="settings-scroll-section-header">
              <span class="settings-scroll-section-title">{t('settings.tabs.general')}</span>
              <span class="settings-scroll-section-line" aria-hidden="true"></span>
            </div>
            <SettingsGeneral bind:config on:change={() => dirty = true} />
          </section>

          <section
            id="settings-section-appearance"
            data-settings-section="appearance"
            class="settings-scroll-section"
            class:settings-scroll-section-active={activeTab === 'appearance'}
          >
            <div class="settings-scroll-section-header">
              <span class="settings-scroll-section-title">{t('settings.tabs.appearance')}</span>
              <span class="settings-scroll-section-line" aria-hidden="true"></span>
            </div>
            <SettingsAppearance bind:config mode="background-only" on:change={handleSettingsChange} />
          </section>

          <section
            id="settings-section-ai"
            data-settings-section="ai"
            class="settings-scroll-section"
            class:settings-scroll-section-active={activeTab === 'ai'}
          >
            <div class="settings-scroll-section-header">
              <span class="settings-scroll-section-title">{t('settings.tabs.ai')}</span>
              <span class="settings-scroll-section-line" aria-hidden="true"></span>
            </div>
            <div class="settings-card settings-ai-shell">
              <h3 class="settings-card-title">{t('settings.aiCardTitle')}</h3>
              <p class="settings-card-desc">{t('settings.aiCardDescription')}</p>
              <SettingsAI bind:config {providers} on:change={() => dirty = true} />
            </div>
          </section>

          <section
            id="settings-section-avatar"
            data-settings-section="avatar"
            class="settings-scroll-section"
            class:settings-scroll-section-active={activeTab === 'avatar'}
          >
            <div class="settings-scroll-section-header">
              <span class="settings-scroll-section-title">{t('settings.tabs.avatar')}</span>
              <span class="settings-scroll-section-line" aria-hidden="true"></span>
            </div>
            <SettingsAvatar bind:config on:change={() => dirty = true} />
          </section>

          <section
            id="settings-section-projects"
            data-settings-section="projects"
            class="settings-scroll-section"
            class:settings-scroll-section-active={activeTab === 'projects'}
          >
            <div class="settings-scroll-section-header">
              <span class="settings-scroll-section-title">{t('settings.tabs.projects')}</span>
              <span class="settings-scroll-section-line" aria-hidden="true"></span>
            </div>
            <SettingsProjects bind:config on:change={handleSettingsChange} />
          </section>

          <section
            id="settings-section-privacy"
            data-settings-section="privacy"
            class="settings-scroll-section"
            class:settings-scroll-section-active={activeTab === 'privacy'}
          >
            <div class="settings-scroll-section-header">
              <span class="settings-scroll-section-title">{t('settings.tabs.privacy')}</span>
              <span class="settings-scroll-section-line" aria-hidden="true"></span>
            </div>
            <SettingsPrivacy
              bind:config
              {runningApps}
              {recentApps}
              on:change={handleSettingsChange}
              on:refresh-apps={() => { loadRunningApps(); loadRecentApps(); }}
            />
          </section>

          <section
            id="settings-section-storage"
            data-settings-section="storage"
            class="settings-scroll-section"
            class:settings-scroll-section-active={activeTab === 'storage'}
          >
            <div class="settings-scroll-section-header">
              <span class="settings-scroll-section-title">{t('settings.tabs.storage')}</span>
              <span class="settings-scroll-section-line" aria-hidden="true"></span>
            </div>
            <SettingsStorage
              bind:config
              {storageStats}
              {dataDir}
              {defaultDataDir}
              hasUnsavedChanges={dirty}
              restoreLocked={restoringBackup}
              {waitForPendingSave}
              on:change={handleSettingsChange}
              on:clearCache={handleClearCache}
              on:dataDirChanged={handleDataDirChanged}
              on:restoreStateChange={handleRestoreStateChange}
              on:backupRestored={handleBackupRestored}
            />
          </section>

          <section
            id="settings-section-node"
            data-settings-section="node"
            class="settings-scroll-section"
            class:settings-scroll-section-active={activeTab === 'node'}
          >
            <div class="settings-scroll-section-header">
              <span class="settings-scroll-section-title">{t('settings.tabs.node')}</span>
              <span class="settings-scroll-section-line" aria-hidden="true"></span>
            </div>
            <SettingsNodeGateway bind:config {dataDir} on:change={handleSettingsChange} on:toast={handleChildToast} />
          </section>
        </div>
      </div>
    </div>
  {/if}
</div>
