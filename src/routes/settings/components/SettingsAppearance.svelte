<script lang="ts">
  import { createEventDispatcher, onDestroy, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { showToast } from '$lib/stores/toast.ts';
  import { cache } from '$lib/stores/cache.ts';
  import { locale, t } from '$lib/i18n/index.ts';
  import { updateConfigQueued } from '$lib/utils/configSaveQueue.ts';
  import {
    applyConfigFields,
    createPersistedConfigSnapshot,
  } from '$lib/utils/appearancePersistence.ts';
  import {
    AVATAR_OPACITY_DEFAULT,
    AVATAR_SCALE_DEFAULT,
    clampAvatarOpacity,
    clampAvatarScale,
    formatAvatarOpacityLabel,
    formatAvatarScaleLabel,
    getAvatarToggleToast,
    getAvatarToggleUiState,
    toggleAvatarSetting,
    updateAvatarOpacitySetting,
    updateAvatarScaleSetting,
  } from '$lib/utils/avatarToggle.ts';
  import type { AvatarToggleUiState } from '$lib/utils/avatarToggle.ts';
  import {
    AVATAR_PRESET_OPTIONS,
    type AvailableAvatarPresetId,
  } from '$lib/components/Avatar/avatarPresetRegistry.ts';
  import AvatarPresetPreview from '$lib/components/Avatar/AvatarPresetPreview.svelte';

  type AppearanceMode = 'full' | 'avatar-only' | 'background-only';
  type AvatarPersona = 'companion' | 'assistant' | 'coach';
  type UiVisualStyle = 'a' | 'b' | 'c';
  type UiTemplate = 'classic' | 'evidence-star-map';

  interface AppearanceConfig {
    avatar_enabled: boolean;
    avatar_scale?: number | null;
    avatar_opacity?: number | null;
    avatar_persona: AvatarPersona;
    avatar_preset: AvailableAvatarPresetId;
    avatar_click_through: boolean;
    avatar_body_hidden: boolean;
    avatar_proactive_ai_enabled: boolean;
    break_reminder_enabled: boolean;
    break_reminder_interval_minutes: number;
    ui_visual_style: UiVisualStyle;
    ui_template: UiTemplate;
    background_image: string | null;
    background_opacity?: number | null;
    background_blur?: number | null;
  }

  type AppearanceConfigField = keyof AppearanceConfig;

  type AppearanceChangeDetail = AppearanceConfig | {
    autosaved: true;
    config: AppearanceConfig;
  };

  function configsMatch(current: AppearanceConfig, snapshot: AppearanceConfig): boolean {
    return JSON.stringify(current) === JSON.stringify(snapshot);
  }

  export let config: AppearanceConfig;
  export let mode: AppearanceMode = 'full';

  const dispatch = createEventDispatcher<{ change: AppearanceChangeDetail }>();
  $: currentLocale = $locale;
  $: showAvatarControls = mode === 'full' || mode === 'avatar-only';
  $: showInterfaceStyleSettings = mode === 'full' || mode === 'background-only';
  $: showBackgroundSettings = mode === 'full' || mode === 'background-only';
  let avatarSaving = false;
  let avatarScaleSaving = false;
  let avatarOpacitySaving = false;
  let avatarPersonaSaving = false;
  let avatarPresetSaving = false;
  let uiVisualStyleSaving = false;
  let uiTemplateSaving = false;
  let pendingUiTemplate: UiTemplate | null = null;
  let persistedUiTemplate: UiTemplate = config.ui_template || 'classic';
  const persistedConfigSnapshot = createPersistedConfigSnapshot(config);
  let observedConfig = config;

  async function saveAppearanceConfig(
    nextConfig: AppearanceConfig,
    fields: readonly AppearanceConfigField[],
  ): Promise<AppearanceConfig> {
    const configSnapshot = structuredClone(nextConfig);
    const savedConfig = await updateConfigQueued<AppearanceConfig>((latestConfig) => (
      applyConfigFields(latestConfig, configSnapshot, fields)
    ));
    persistedConfigSnapshot.recordSuccessfulSave(savedConfig);
    return savedConfig;
  }
  $: if (config !== observedConfig && !uiTemplateSaving && pendingUiTemplate === null) {
    observedConfig = config;
    persistedUiTemplate = config.ui_template || 'classic';
    persistedConfigSnapshot.recordSuccessfulSave(config);
  }

  let avatarScaleTimer: ReturnType<typeof setTimeout> | null = null;
  let avatarOpacityTimer: ReturnType<typeof setTimeout> | null = null;
  const breakReminderIntervals = [30, 45, 50, 60, 90, 120];
  const UI_TEMPLATE_OPTIONS: readonly { id: UiTemplate; titleKey: string; descriptionKey: string }[] = [
    { id: 'classic', titleKey: 'settingsAppearance.uiTemplateClassic', descriptionKey: 'settingsAppearance.uiTemplateClassicDesc' },
    { id: 'evidence-star-map', titleKey: 'settingsAppearance.evidenceStarMap', descriptionKey: 'settingsAppearance.evidenceStarMapDesc' },
  ];
  const UI_VISUAL_STYLE_OPTIONS: readonly {
    id: UiVisualStyle;
    titleKey: string;
    descriptionKey: string;
    badgeKey: string;
  }[] = [
    {
      id: 'a',
      titleKey: 'settingsAppearance.uiStyleATitle',
      descriptionKey: 'settingsAppearance.uiStyleADesc',
      badgeKey: 'settingsAppearance.uiStyleABadge',
    },
    {
      id: 'b',
      titleKey: 'settingsAppearance.uiStyleBTitle',
      descriptionKey: 'settingsAppearance.uiStyleBDesc',
      badgeKey: 'settingsAppearance.uiStyleBBadge',
    },
    {
      id: 'c',
      titleKey: 'settingsAppearance.uiStyleCTitle',
      descriptionKey: 'settingsAppearance.uiStyleCDesc',
      badgeKey: 'settingsAppearance.uiStyleCBadge',
    },
  ];
  const AVATAR_PERSONA_OPTIONS: readonly {
    id: AvatarPersona;
    titleKey: string;
    descriptionKey: string;
  }[] = [
    {
      id: 'companion',
      titleKey: 'settingsAppearance.avatarPersonaCompanionTitle',
      descriptionKey: 'settingsAppearance.avatarPersonaCompanionDesc',
    },
    {
      id: 'assistant',
      titleKey: 'settingsAppearance.avatarPersonaAssistantTitle',
      descriptionKey: 'settingsAppearance.avatarPersonaAssistantDesc',
    },
    {
      id: 'coach',
      titleKey: 'settingsAppearance.avatarPersonaCoachTitle',
      descriptionKey: 'settingsAppearance.avatarPersonaCoachDesc',
    },
  ];
  let blurLabels: string[] = [];
  let avatarToggleUi: AvatarToggleUiState = getAvatarToggleUiState(false);
  // === 背景图片 ===
  let bgPreview: string | null = null;
  let bgUploading = false;
  let appearanceDestroyed = false;

  $: {
    currentLocale;
    blurLabels = [
      t('settingsAppearance.blurClear'),
      t('settingsAppearance.blurLight'),
      t('settingsAppearance.blurMedium'),
    ];
  }
  $: {
    currentLocale;
    avatarToggleUi = getAvatarToggleUiState(Boolean(config.avatar_enabled), avatarSaving);
  }
  $: avatarScale = clampAvatarScale(config.avatar_scale ?? AVATAR_SCALE_DEFAULT);
  $: avatarScaleLabel = formatAvatarScaleLabel(avatarScale);
  $: avatarOpacity = clampAvatarOpacity(config.avatar_opacity ?? AVATAR_OPACITY_DEFAULT);
  $: avatarOpacityLabel = formatAvatarOpacityLabel(avatarOpacity);
  onMount(async () => {
    if (showBackgroundSettings) {
      try {
        const b64 = await invoke<string | null>('get_background_image');
        if (b64) bgPreview = `data:image/jpeg;base64,${b64}`;
      } catch (e) { /* ignore */ }
    }
  });

  onDestroy(() => {
    appearanceDestroyed = true;
    if (avatarScaleTimer !== null) clearTimeout(avatarScaleTimer);
    if (avatarOpacityTimer !== null) clearTimeout(avatarOpacityTimer);
  });

  async function toggleAvatarMode() {
    if (avatarSaving) {
      return;
    }

    avatarSaving = true;

    try {
      if (config.avatar_enabled) {
        try {
          await invoke<void>('persist_avatar_position');
        } catch (persistError) {
          console.warn('关闭桌面助手前持久化位置失败:', persistError);
        }
      }

      const enabled = await toggleAvatarSetting(config, async (nextConfig) => {
        await saveAppearanceConfig(nextConfig, ['avatar_enabled', 'break_reminder_enabled']);
      });

      dispatch('change', config);
      showToast(getAvatarToggleToast(enabled), enabled ? 'success' : 'info');
    } catch (e) {
      console.error('设置桌宠失败:', e);
      showToast(t('settingsAppearance.avatarToggleFailed', { error: e }), 'error');
    } finally {
      avatarSaving = false;
    }
  }

  function queueAvatarScaleSave(nextScale: number) {
    if (avatarScaleTimer !== null) clearTimeout(avatarScaleTimer);
    avatarScaleTimer = setTimeout(async () => {
      avatarScaleSaving = true;

      try {
        const savedScale = await updateAvatarScaleSetting(config, nextScale, async (nextConfig) => {
          await saveAppearanceConfig(nextConfig, ['avatar_scale']);
        });
        config.avatar_scale = savedScale;
        dispatch('change', config);
      } catch (e) {
        console.error('保存桌宠缩放失败:', e);
        showToast(t('settingsAppearance.avatarScaleSaveFailed', { error: e }), 'error');
      } finally {
        avatarScaleSaving = false;
      }
    }, 120);
  }

  function handleAvatarScaleInput(event: Event) {
    const input = event.currentTarget;
    if (!(input instanceof HTMLInputElement)) return;
    const nextScale = clampAvatarScale(Number(input.value));
    config.avatar_scale = nextScale;
    dispatch('change', config);
    queueAvatarScaleSave(nextScale);
  }

  function queueAvatarOpacitySave(nextOpacity: number) {
    if (avatarOpacityTimer !== null) clearTimeout(avatarOpacityTimer);
    avatarOpacityTimer = setTimeout(async () => {
      avatarOpacitySaving = true;

      try {
        const savedOpacity = await updateAvatarOpacitySetting(
          config,
          nextOpacity,
          async (nextConfig) => {
            await saveAppearanceConfig(nextConfig, ['avatar_opacity']);
          }
        );
        config.avatar_opacity = savedOpacity;
        dispatch('change', config);
      } catch (e) {
        console.error('保存桌宠透明度失败:', e);
        showToast(t('settingsAppearance.avatarOpacitySaveFailed', { error: e }), 'error');
      } finally {
        avatarOpacitySaving = false;
      }
    }, 120);
  }

  function handleAvatarOpacityInput(event: Event) {
    const input = event.currentTarget;
    if (!(input instanceof HTMLInputElement)) return;
    const nextOpacity = clampAvatarOpacity(Number(input.value));
    config.avatar_opacity = nextOpacity;
    dispatch('change', config);
    queueAvatarOpacitySave(nextOpacity);
  }

  async function selectAvatarPreset(presetId: AvailableAvatarPresetId) {
    if (avatarPresetSaving || config.avatar_preset === presetId) {
      return;
    }

    avatarPresetSaving = true;
    const previousPreset = config.avatar_preset;
    config.avatar_preset = presetId;
    dispatch('change', config);

    try {
      await saveAppearanceConfig(config, ['avatar_preset']);
    } catch (e) {
      config.avatar_preset = previousPreset;
      dispatch('change', config);
      console.error('保存桌宠预设失败:', e);
      showToast(t('settingsAppearance.avatarPresetSaveFailed', { error: e }), 'error');
    } finally {
      avatarPresetSaving = false;
    }
  }

  async function selectAvatarPersona(personaId: AvatarPersona) {
    if (avatarPersonaSaving || config.avatar_persona === personaId) {
      return;
    }

    avatarPersonaSaving = true;
    const previousPersona = config.avatar_persona;
    config.avatar_persona = personaId;
    dispatch('change', config);

    try {
      await saveAppearanceConfig(config, ['avatar_persona']);
    } catch (e) {
      config.avatar_persona = previousPersona;
      dispatch('change', config);
      console.error('保存桌宠互动风格失败:', e);
      showToast(t('settingsAppearance.avatarPersonaSaveFailed', { error: e }), 'error');
    } finally {
      avatarPersonaSaving = false;
    }
  }

  async function selectUiTemplate(templateId: UiTemplate) {
    if (config.ui_template === templateId && pendingUiTemplate === null) return;

    pendingUiTemplate = templateId;
    config.ui_template = templateId;
    dispatch('change', config);
    window.dispatchEvent(new CustomEvent('ui-template-changed', { detail: { template: templateId } }));

    if (uiTemplateSaving) return;

    uiTemplateSaving = true;
    try {
      while (pendingUiTemplate !== null) {
        const templateToSave = pendingUiTemplate;
        pendingUiTemplate = null;
        const previousTemplate = persistedUiTemplate;
        const configSnapshot = structuredClone(config);
        configSnapshot.ui_template = templateToSave;

        try {
          const savedConfig = await saveAppearanceConfig(configSnapshot, ['ui_template']);
          persistedUiTemplate = templateToSave;

          if (pendingUiTemplate === null && configsMatch(config, configSnapshot)) {
            cache.setConfig(structuredClone(savedConfig));
            dispatch('change', { autosaved: true, config });
          } else {
            dispatch('change', config);
          }
        } catch (e) {
          if (pendingUiTemplate !== null) {
            console.warn('较早的界面模板保存失败，继续保存最新选择:', e);
            continue;
          }

          config.ui_template = previousTemplate;
          persistedUiTemplate = previousTemplate;
          cache.setConfig(persistedConfigSnapshot.read());
          dispatch('change', config);
          window.dispatchEvent(new CustomEvent('ui-template-changed', { detail: { template: previousTemplate } }));
          showToast(t('settingsAppearance.uiTemplateSaveFailed', { error: e }), 'error');
        }
      }
    } finally {
      uiTemplateSaving = false;
    }
  }

  async function selectUiVisualStyle(styleId: UiVisualStyle) {
    if (uiVisualStyleSaving || config.ui_visual_style === styleId) {
      return;
    }

    // 与 Settings.svelte 的归一化默认值保持一致（缺省风格为 'c'）
    const previousStyle = config.ui_visual_style || 'c';
    uiVisualStyleSaving = true;
    config.ui_visual_style = styleId;
    dispatch('change', { autosaved: true, config });
    window.dispatchEvent(new CustomEvent('ui-visual-style-changed', {
      detail: { style: styleId },
    }));

    try {
      await saveAppearanceConfig(config, ['ui_visual_style']);
    } catch (e) {
      config.ui_visual_style = previousStyle;
      dispatch('change', { autosaved: true, config });
      window.dispatchEvent(new CustomEvent('ui-visual-style-changed', {
        detail: { style: previousStyle },
      }));
      console.error('保存界面风格失败:', e);
      showToast(t('settingsAppearance.uiVisualStyleSaveFailed', { error: e }), 'error');
    } finally {
      uiVisualStyleSaving = false;
    }
  }

  function toggleBreakReminder() {
    if (!config.avatar_enabled) {
      return;
    }

    config.break_reminder_enabled = !config.break_reminder_enabled;
    dispatch('change', config);
    saveConfigQuietly(['break_reminder_enabled']);
  }

  function toggleAvatarProactiveAi() {
    if (!config.avatar_enabled) {
      return;
    }

    config.avatar_proactive_ai_enabled = !Boolean(config.avatar_proactive_ai_enabled);
    dispatch('change', config);
    saveConfigQuietly(['avatar_proactive_ai_enabled']);
  }

  function handleBreakReminderIntervalChange() {
    dispatch('change', config);
    saveConfigQuietly(['break_reminder_interval_minutes']);
  }

  function handleBgFileSelect(event: Event) {
    const input = event.currentTarget;
    if (!(input instanceof HTMLInputElement)) return;
    const file = input.files?.[0];
    if (!file) return;
    if (!file.type.startsWith('image/')) return;
    if (file.size > 10 * 1024 * 1024) {
      showToast(t('settingsAppearance.imageTooLarge'), 'warning');
      return;
    }

    bgUploading = true;
    const reader = new FileReader();
    reader.onload = async () => {
      if (appearanceDestroyed) return;

      try {
        const b64Data = typeof reader.result === 'string' ? reader.result.split(',')[1] : null;
        if (!b64Data) {
          throw new Error(t('settingsAppearance.imageReadFailed'));
        }
        await invoke<void>('save_background_image', { data: b64Data });
        if (appearanceDestroyed) return;
        config.background_image = 'background.jpg';
        await saveAppearanceConfig(config, ['background_image']);
        const freshB64 = await invoke<string | null>('get_background_image');
        if (appearanceDestroyed) return;
        const imageUrl = freshB64 ? `data:image/jpeg;base64,${freshB64}` : null;
        bgPreview = imageUrl;
        dispatchBgEvent(imageUrl);
      } catch (e) {
        if (appearanceDestroyed) return;
        console.error('上传背景图失败:', e);
        showToast(t('settingsAppearance.uploadFailed', { error: e }), 'error');
      } finally {
        if (!appearanceDestroyed) {
          bgUploading = false;
        }
      }
    };
    reader.readAsDataURL(file);
  }

  async function clearBg() {
    try {
      await invoke<void>('clear_background_image');
      bgPreview = null;
      config.background_image = null;
      dispatchBgEvent(null);
      await saveAppearanceConfig(config, ['background_image']);
    } catch (e) {
      console.error('清除背景图失败:', e);
      showToast(t('settingsAppearance.clearFailed', { error: e }), 'error');
    }
  }

  function updateBgOpacity(val: string | number) {
    config.background_opacity = parseFloat(String(val));
    dispatch('change', config);
    dispatchBgEvent(bgPreview);
    saveConfigQuietly(['background_opacity']);
  }

  function updateBgBlur(val: string | number) {
    config.background_blur = parseInt(String(val), 10);
    dispatch('change', config);
    dispatchBgEvent(bgPreview);
    saveConfigQuietly(['background_blur']);
  }

  function dispatchBgEvent(image: string | null) {
    window.dispatchEvent(new CustomEvent('background-changed', {
      detail: {
        image,
        opacity: config.background_opacity ?? 0.25,
        blur: config.background_blur ?? 1,
      }
    }));
  }

  async function saveConfigQuietly(fields: readonly AppearanceConfigField[]) {
    try {
      await saveAppearanceConfig(config, fields);
    } catch (e) {
      console.error('自动保存配置失败:', e);
    }
  }
</script>

{#if showAvatarControls}
<div class="settings-card" data-locale={currentLocale}>
  <div class="settings-section">
    <div class="flex items-center justify-between gap-4">
      <div>
        <div class="flex items-center gap-2">
          <div class="settings-text">{t('settingsAppearance.avatar')}</div>
        </div>
        <div class="settings-muted mt-0.5">{t('settingsAppearance.avatarDesc')}</div>
        <div class="settings-muted mt-0.5">{t('settingsAppearance.avatarBetaHint')}</div>
      </div>
      <button
        type="button"
        on:click={toggleAvatarMode}
        class="switch-track {avatarToggleUi.trackClass} {avatarToggleUi.buttonClass}"
        disabled={avatarSaving}
        role="switch"
        aria-label={avatarToggleUi.ariaLabel}
        aria-checked={config.avatar_enabled}
      >
        <span class="switch-thumb {avatarToggleUi.thumbClass}"></span>
      </button>
    </div>
    <div class="settings-block pt-1">
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="settings-text">{t('settingsAppearance.avatarSize')}</div>
        </div>
        <div class="text-sm font-semibold text-slate-700 dark:text-[#c9d1d9]">
          {avatarScaleLabel}
          {#if avatarScaleSaving}
            <span class="ml-2 text-xs font-normal text-slate-400 dark:text-[#636c76]">{t('settingsAppearance.syncing')}</span>
          {/if}
        </div>
      </div>

      <input
        type="range"
        min="0.4"
        max="1.3"
        step="0.05"
        value={avatarScale}
        on:input={handleAvatarScaleInput}
        class="mt-3 w-full accent-primary-500"
        aria-label={t('settingsAppearance.avatarSizeAria')}
      />
      <div class="mt-2 flex justify-between text-[11px] text-slate-400 dark:text-[#636c76]">
        <span>{t('settingsAppearance.smaller')}</span>
        <span>{t('settingsAppearance.default90')}</span>
        <span>{t('settingsAppearance.larger')}</span>
      </div>
    </div>

    <div class="settings-block pt-1">
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="settings-text">{t('settingsAppearance.avatarOpacity')}</div>
          <div class="settings-muted mt-0.5">{t('settingsAppearance.avatarOpacityHint')}</div>
        </div>
        <div class="text-sm font-semibold text-slate-700 dark:text-[#c9d1d9]">
          {avatarOpacityLabel}
          {#if avatarOpacitySaving}
            <span class="ml-2 text-xs font-normal text-slate-400 dark:text-[#636c76]">{t('settingsAppearance.syncing')}</span>
          {/if}
        </div>
      </div>

      <input
        type="range"
        min="0.45"
        max="1"
        step="0.05"
        value={avatarOpacity}
        on:input={handleAvatarOpacityInput}
        class="mt-3 w-full accent-primary-500"
        aria-label={t('settingsAppearance.avatarOpacityAria')}
      />
      <div class="mt-2 flex justify-between text-[11px] text-slate-400 dark:text-[#636c76]">
        <span>{t('settingsAppearance.moreTransparent')}</span>
        <span>{t('settingsAppearance.default82')}</span>
        <span>{t('settingsAppearance.moreSolid')}</span>
      </div>
    </div>

    <div class="settings-block pt-1">
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="settings-text">{t('settingsAppearance.avatarPersona')}</div>
        </div>
        {#if avatarPersonaSaving}
          <div class="text-xs text-slate-400 dark:text-[#636c76]">{t('settingsAppearance.syncing')}</div>
        {/if}
      </div>

      <div class="mt-3 grid gap-3 md:grid-cols-3">
        {#each AVATAR_PERSONA_OPTIONS as persona}
          <button
            type="button"
            class="rounded-lg border p-3 text-left transition {config.avatar_persona === persona.id ? 'border-emerald-400 bg-emerald-50/80 shadow-sm dark:shadow-none dark:border-emerald-400/70 dark:bg-emerald-500/10' : 'border-slate-200 bg-white hover:border-slate-300 dark:border-[#30363d] dark:bg-[#161b22]/60 dark:hover:border-[#484f58]'}"
            on:click={() => selectAvatarPersona(persona.id)}
            aria-pressed={config.avatar_persona === persona.id}
          >
            <div class="flex items-center justify-between gap-3">
              <div class="text-sm font-semibold text-slate-900 dark:text-[#e6edf3]">
                {t(persona.titleKey)}
              </div>
              {#if config.avatar_persona === persona.id}
                <span class="rounded-full bg-emerald-500/12 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-[0.08em] text-emerald-700 dark:text-emerald-300">
                  {t('settingsAppearance.avatarPersonaCurrent')}
                </span>
              {/if}
            </div>
            <div class="mt-2 text-xs leading-5 text-slate-500 dark:text-[#7d8590]">
              {t(persona.descriptionKey)}
            </div>
          </button>
        {/each}
      </div>
    </div>

    <div class="settings-block pt-1">
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="settings-text">{t('settingsAppearance.avatarPreset')}</div>
        </div>
        {#if avatarPresetSaving}
          <div class="text-xs text-slate-400 dark:text-[#636c76]">{t('settingsAppearance.syncing')}</div>
        {/if}
      </div>

      <div class="mt-3 grid gap-3 md:grid-cols-3">
        {#each AVATAR_PRESET_OPTIONS as preset}
          <button
            type="button"
            class="rounded-[var(--radius-md)] border p-3 text-left transition {config.avatar_preset === preset.id ? 'border-primary-500 bg-primary-50/70 shadow-sm dark:shadow-none dark:border-primary-400 dark:bg-primary-500/10' : 'border-slate-200 bg-white hover:border-slate-300 dark:border-[#30363d] dark:bg-[#161b22]/60 dark:hover:border-[#484f58]'}"
            on:click={() => selectAvatarPreset(preset.id)}
            aria-pressed={config.avatar_preset === preset.id}
          >
            <div class="h-24 w-full rounded-xl border border-slate-200 bg-slate-50 dark:border-[#30363d] dark:bg-[#0d1117]/70">
              <AvatarPresetPreview presetId={preset.id} selected={config.avatar_preset === preset.id} />
            </div>
            <div class="mt-3 text-sm font-semibold text-slate-900 dark:text-[#e6edf3]">
              {t(preset.titleKey)}
            </div>
            <div class="mt-1 text-xs leading-5 text-slate-500 dark:text-[#7d8590]">
              {t(preset.descriptionKey)}
            </div>
          </button>
        {/each}
      </div>
    </div>

    <hr class="border-slate-200 dark:border-[#30363d]" />

    <div class="flex items-center justify-between gap-4">
      <div>
        <div class="settings-text">{t('settingsAppearance.avatarClickThrough')}</div>
        <div class="settings-muted mt-1 text-xs">{t('settingsAppearance.avatarClickThroughDescription')}</div>
      </div>
      <button
        type="button"
        on:click={() => { config.avatar_click_through = !config.avatar_click_through; }}
        class="switch-track {config.avatar_click_through ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
        role="switch"
        aria-label={t('settingsAppearance.avatarClickThrough')}
        aria-checked={config.avatar_click_through}
      >
        <span class="switch-thumb {config.avatar_click_through ? 'translate-x-5' : 'translate-x-0'}"></span>
      </button>
    </div>

    <hr class="border-slate-200 dark:border-[#30363d]" />

    <div class="flex items-center justify-between gap-4">
      <div>
        <div class="settings-text">{t('settingsAppearance.avatarBodyHidden')}</div>
        <div class="settings-muted mt-1 text-xs">{t('settingsAppearance.avatarBodyHiddenDescription')}</div>
      </div>
      <button
        type="button"
        on:click={() => { config.avatar_body_hidden = !config.avatar_body_hidden; }}
        class="switch-track {config.avatar_body_hidden ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'} {!config.avatar_enabled ? 'cursor-not-allowed opacity-50' : ''}"
        disabled={!config.avatar_enabled}
        role="switch"
        aria-label={t('settingsAppearance.avatarBodyHidden')}
        aria-checked={config.avatar_body_hidden}
      >
        <span class="switch-thumb {config.avatar_body_hidden ? 'translate-x-5' : 'translate-x-0'}"></span>
      </button>
    </div>

    <hr class="border-slate-200 dark:border-[#30363d]" />

    <div class="flex items-center justify-between gap-4">
      <div>
        <div class="settings-text">{t('settingsAppearance.breakReminder')}</div>
        <div class="settings-muted mt-1 text-xs">{t('settingsAppearance.breakReminderDescription')}</div>
        {#if !config.avatar_enabled}
          <div class="settings-muted mt-1 text-xs">{t('settingsAppearance.breakReminderRequiresAvatar')}</div>
        {/if}
      </div>
      <button
        type="button"
        on:click={toggleBreakReminder}
        class="switch-track {config.break_reminder_enabled && config.avatar_enabled ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'} {!config.avatar_enabled ? 'cursor-not-allowed opacity-50' : ''}"
        disabled={!config.avatar_enabled}
        role="switch"
        aria-label={t('settingsAppearance.breakReminder')}
        aria-checked={config.break_reminder_enabled}
      >
        <span class="switch-thumb {config.break_reminder_enabled && config.avatar_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
      </button>
    </div>

    {#if config.break_reminder_enabled}
      <div class="settings-block pt-3 border-t border-slate-200 dark:border-[#30363d]">
        <label for="break-reminder-interval" class="settings-label mb-1.5">
          {t('settingsAppearance.breakReminderInterval')}
        </label>
        <select
          id="break-reminder-interval"
          bind:value={config.break_reminder_interval_minutes}
          on:change={handleBreakReminderIntervalChange}
          class="control-input"
          disabled={!config.avatar_enabled}
        >
          {#each breakReminderIntervals as interval}
            <option value={interval}>{interval} {t('common.minutes')}</option>
          {/each}
        </select>
      </div>
    {/if}

    <div class="settings-muted text-xs leading-5">
      {t('settingsAppearance.avatarLocalReminderNote')}
    </div>

    <hr class="border-slate-200 dark:border-[#30363d]" />

    <div class="flex items-start justify-between gap-4">
      <div>
        <div class="settings-text">{t('settingsAppearance.avatarProactiveAi')}</div>
        <div class="settings-muted mt-1 text-xs">{t('settingsAppearance.avatarProactiveAiDescription')}</div>
        <div class="settings-muted mt-1 text-xs">{t('settingsAppearance.avatarProactiveAiDataNotice')}</div>
        {#if !config.avatar_enabled}
          <div class="settings-muted mt-1 text-xs">{t('settingsAppearance.avatarProactiveAiRequiresAvatar')}</div>
        {/if}
      </div>
      <button
        type="button"
        on:click={toggleAvatarProactiveAi}
        class="switch-track {config.avatar_proactive_ai_enabled && config.avatar_enabled ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'} {!config.avatar_enabled ? 'cursor-not-allowed opacity-50' : ''}"
        disabled={!config.avatar_enabled}
        role="switch"
        aria-checked={config.avatar_proactive_ai_enabled}
        aria-label={t('settingsAppearance.avatarProactiveAi')}
      >
        <span class="switch-thumb {config.avatar_proactive_ai_enabled && config.avatar_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
      </button>
    </div>
  </div>
</div>
{/if}

{#if showInterfaceStyleSettings}
<div class="settings-card evidence-template-settings">
  <div class="flex items-start justify-between gap-4">
    <div><h3 class="settings-card-title">{t('settingsAppearance.uiTemplate')}</h3><p class="settings-muted mt-1">{t('settingsAppearance.uiTemplateDesc')}</p></div>
    {#if uiTemplateSaving}<span class="text-xs settings-subtle">{t('settingsAppearance.syncing')}</span>{/if}
  </div>
  <div class="mt-4 grid gap-3 md:grid-cols-2">
    {#each UI_TEMPLATE_OPTIONS as option}
      <button type="button" class="settings-style-option evidence-template-option" class:settings-style-option-active={config.ui_template === option.id} on:click={() => selectUiTemplate(option.id)} aria-pressed={config.ui_template === option.id}>
        <div class="evidence-template-preview evidence-template-preview--{option.id}" aria-hidden="true"><span></span><span></span><span></span></div>
        <div class="mt-3" style="text-align: start;"><div class="text-sm font-semibold settings-text">{t(option.titleKey)}</div><p class="mt-2 text-xs leading-5 settings-muted">{t(option.descriptionKey)}</p></div>
      </button>
    {/each}
  </div>
</div>

<div class="settings-card" data-locale={currentLocale}>
  <div class="flex items-start justify-between gap-4">
    <div>
      <h3 class="settings-card-title">
        {t('settingsAppearance.uiVisualStyle')}
        <span class="ml-1.5 inline-flex items-center rounded-full border border-amber-200 bg-amber-50 px-1 py-px text-[10px] font-semibold uppercase tracking-[0.06em] text-amber-700 align-middle dark:border-amber-500/30 dark:bg-amber-500/10 dark:text-amber-200">Beta</span>
      </h3>
      <p class="settings-muted mt-1">{t('settingsAppearance.uiVisualStyleDesc')}</p>
    </div>
    {#if uiVisualStyleSaving}
      <span class="text-xs text-slate-400 dark:text-[#636c76]">{t('settingsAppearance.syncing')}</span>
    {/if}
  </div>

  <div class="mt-4 grid gap-3 md:grid-cols-3">
    {#each UI_VISUAL_STYLE_OPTIONS as option}
      <button
        type="button"
        class="settings-style-option {config.ui_visual_style === option.id ? 'settings-style-option-active' : ''}"
        on:click={() => selectUiVisualStyle(option.id)}
        aria-pressed={config.ui_visual_style === option.id}
      >
        <div class="settings-style-preview settings-style-preview--{option.id}" aria-hidden="true">
          <div class="settings-style-preview__sidebar"></div>
          <div class="settings-style-preview__topbar"></div>
          <div class="settings-style-preview__metric"></div>
          <div class="settings-style-preview__chart">
            <span></span>
            <span></span>
            <span></span>
          </div>
        </div>
        <div class="mt-3 flex items-center justify-between gap-2">
          <div class="text-sm font-semibold text-slate-900 dark:text-[#e6edf3]">
            {t(option.titleKey)}
          </div>
          <div class="flex items-center gap-1.5">
            {#if config.ui_visual_style === option.id}
              <span class="settings-style-current-mark">
                <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
                  <path d="M3.5 8.2L6.5 11L12.5 5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
                </svg>
                {t('settingsAppearance.uiStyleCurrent')}
              </span>
            {/if}
            <span class="rounded-full bg-slate-100 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-[0.08em] text-slate-500 dark:bg-[#30363d] dark:text-[#adbac7]">
              {t(option.badgeKey)}
            </span>
          </div>
        </div>
        <p class="mt-2 text-xs leading-5 text-slate-500 dark:text-[#7d8590]">
          {t(option.descriptionKey)}
        </p>
        <p class="mt-2 text-[11px] font-medium text-slate-400 dark:text-[#636c76]">
          {t('settingsAppearance.uiVisualStyleApplyHint')}
        </p>
      </button>
    {/each}
  </div>
</div>
{/if}

<!-- 背景图片 -->
{#if showBackgroundSettings}
<div class="settings-card" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsAppearance.backgroundImage')}</h3>

  <div class="settings-section">
    <!-- 预览 + 上传 -->
    <div class="flex items-start gap-4">
      {#if bgPreview}
        <div class="w-32 h-20 rounded-lg overflow-hidden border border-slate-200 dark:border-[#30363d] flex-shrink-0">
          <img src={bgPreview} alt={t('settingsAppearance.bgPreviewAlt')} class="w-full h-full object-cover" />
        </div>
      {:else}
        <div class="w-32 h-20 rounded-lg border-2 border-dashed border-slate-200 dark:border-[#30363d] flex items-center justify-center flex-shrink-0">
          <span class="settings-subtle">{t('settingsAppearance.noBackground')}</span>
        </div>
      {/if}

      <div class="flex-1 settings-field">
        <label class="settings-action-secondary cursor-pointer">
          {#if bgUploading}
            <div class="animate-spin rounded-full h-3 w-3 border-2 border-slate-500 border-t-transparent"></div>
            {t('common.processing')}
          {:else}
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" /></svg>
            {t('settingsAppearance.chooseImage')}
          {/if}
          <input type="file" accept="image/*" class="hidden" on:change={handleBgFileSelect} disabled={bgUploading} />
        </label>
        {#if bgPreview}
          <button
            on:click={clearBg}
            class="settings-link-danger"
          >
            {t('settingsAppearance.clearBackground')}
          </button>
        {/if}
        <p class="settings-muted">{t('settingsAppearance.bgSupport')}</p>
      </div>
    </div>

    {#if bgPreview || config.background_image}
      <hr class="border-slate-200 dark:border-[#30363d]" />

      <!-- 显示强度 -->
      <div class="settings-block">
        <div class="flex items-center justify-between">
          <span class="settings-text">{t('settingsAppearance.bgStrength')}</span>
          <span class="settings-value">{Math.round((config.background_opacity ?? 0.25) * 100)}%</span>
        </div>
        <input
          type="range"
          min="0.05"
          max="0.60"
          step="0.01"
          value={config.background_opacity ?? 0.25}
          on:input={(e) => updateBgOpacity(e.currentTarget.value)}
          class="range-input"
        />
        <div class="flex justify-between text-[10px] settings-subtle">
          <span>{t('settingsAppearance.bgLight')}</span>
          <span>{t('settingsAppearance.bgStrong')}</span>
        </div>
      </div>

      <!-- 模糊度 -->
      <div class="settings-block">
        <div class="flex items-center justify-between">
          <span class="settings-text">{t('settingsAppearance.bgBlur')}</span>
          <span class="settings-muted">{blurLabels[config.background_blur ?? 1]}</span>
        </div>
        <div class="flex gap-2">
          {#each [0, 1, 2] as level}
            <button
              on:click={() => updateBgBlur(level)}
              class="segment-btn
                {(config.background_blur ?? 1) === level
                  ? 'settings-segment-active'
                  : 'settings-segment-base'}"
            >
              {blurLabels[level]}
            </button>
          {/each}
        </div>
      </div>
    {/if}
  </div>
</div>
{/if}
