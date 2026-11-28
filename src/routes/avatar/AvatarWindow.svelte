<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type { Unsubscriber } from 'svelte/store';
  import { invoke } from '@tauri-apps/api/core';
  import { emitTo, listen } from '@tauri-apps/api/event';
  import type { PhysicalPosition } from '@tauri-apps/api/dpi';
  import { getCurrentWindow, currentMonitor } from '@tauri-apps/api/window';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  import AvatarCanvas from '../../lib/components/Avatar/AvatarCanvas.svelte';
  import AvatarFollowupCard from '../../lib/components/Avatar/AvatarFollowupCard.svelte';
  import AvatarPopover from '../../lib/components/Avatar/AvatarPopover.svelte';
  import {
    applyLocaleToDocument,
    initializeLocale,
    locale,
    t,
    type Locale,
  } from '$lib/i18n/index.ts';
  import {
    getAvatarMotionStepDelay,
    getAvatarStateBubble,
    getAvatarTransitionMeta,
    type AvatarPersona,
    type AvatarTransitionClass,
  } from '../../lib/components/Avatar/avatarStateMeta.ts';
  import {
    normalizeAvatarInputActivity,
    parseAvatarBubblePayload,
    parseAvatarFollowupPayload,
    parseAvatarState,
    type AvatarBubblePayload,
    type AvatarFollowupPayload,
    type AvatarInputActivity,
    type AvatarState,
  } from './avatarPayload.ts';

  type AvatarFollowupAction = 'timeline' | 'focus' | 'remember' | 'snooze' | 'dismiss';

  interface AvatarFocusSession {
    projectKey: string;
    title: string;
    endsAtMs: number;
  }

  interface AvatarExpansionOptions {
    force?: boolean;
  }

  interface FollowupPersonaTheme {
    badgeClass: string;
    primaryClass: string;
    surfaceClass: string;
    strategyKey: string;
    focusKey: string;
    focusFullKey: string;
    rememberKey: string;
    rememberFullKey: string;
    snoozeKey: string;
    snoozeFullKey: string;
    timelineOpeningKey: string;
    rememberedKey: string;
    snoozedKey: string;
    focusStartedKey: string;
    focusStoppedKey: string;
    focusFinishedKey: string;
  }

  type AvatarPointerEvent = MouseEvent | CustomEvent<{ originalEvent: MouseEvent }>;

  const appWindow = getCurrentWebviewWindow();
  const nativeWindow = getCurrentWindow();

  let state: AvatarState = {
    mode: 'idle',
    appName: 'Work Review',
    contextLabel: '待命中',
    hint: '准备陪你开始工作',
    isIdle: true,
    isGeneratingReport: false,
    avatarOpacity: 0.82,
    avatarPreset: 'original-standard',
    avatarPersona: 'assistant',
    avatarBodyHidden: false,
  };
  let inputActivity: AvatarInputActivity = {
    keyboardActive: false,
    mouseActive: false,
    keyboardGroup: 'idle',
    keyboardVisualKey: '',
    mouseGroup: 'idle',
    cursorRatioX: 0.5,
    cursorRatioY: 0.5,
    lastKeyboardInputAtMs: 0,
    lastMouseInputAtMs: 0,
  };
  let bubbleSource: AvatarBubblePayload | null = null;
  // 气泡边缘锚点：窗口靠近屏幕右侧时气泡向左展开，避免靠右下角时被裁剪（#137 诉求一）
  let bubbleFlipLeft = false;
  let bubble: AvatarBubblePayload | null = null;
  let bubbleTimer: ReturnType<typeof setTimeout> | null = null;
  let followup: AvatarFollowupPayload | null = null;
  let focusSession: AvatarFocusSession | null = null;
  let focusTimer: ReturnType<typeof setInterval> | null = null;
  let focusNowMs = 0;
  let focusIntroUntilMs = 0;
  let lastStateBubbleAt = 0;
  let transitionClass: AvatarTransitionClass = '';
  let transitionTimer: ReturnType<typeof setTimeout> | null = null;
  let motionBeat = 0;
  let motionTimer: ReturnType<typeof setTimeout> | null = null;
  let lastInputMotionBeatAt = 0;
  let windowVisible = true;
  let positionSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let suppressPositionSaveUntil = 0;
  let bubbleEdgeTimer: ReturnType<typeof setTimeout> | null = null;
  let bubbleEdgeRequestSeq = 0;
  let sizeCorrectionTimer: ReturnType<typeof setTimeout> | null = null;
  let resizeGuardTimer: ReturnType<typeof setTimeout> | null = null;
  let suppressNextResizeCorrection = false;
  let lastSavedPositionKey: string | null = null;
  let unsubscribeLocale: Unsubscriber = () => {};
  let handleVisibilityChange: (() => void) | null = null;
  let handleContextMenu: ((event: MouseEvent) => void) | null = null;
  let handleKeydown: ((event: KeyboardEvent) => void) | null = null;
  let avatarExpanded = null as boolean | null;
  let interactiveRegionObserver: ResizeObserver | null = null;
  let interactiveRegionFrame: number | null = null;
  let interactiveRegionElements: Element[] = [];
  let interactiveRegionSyncPending = false;
  let interactiveRegionSyncRequested = false;
  let interactiveRegionsMounted = false;
  let lastInteractiveRegionsSignature: string | null = null;
  $: currentLocale = $locale;

  const RUNTIME_BUBBLE_MESSAGES: Partial<Record<string, Partial<Record<Locale, string>>>> = {
    __avatar_nudge_switch_companion__: {
      'zh-CN': '你切得有点快，我陪你回主线。',
      'zh-TW': '你切得有點快，我陪你回主線。',
      en: 'You are switching fast. Let us get back to the main thread.',
      ar: 'أنت تبدّل بسرعة، لنعد معاً إلى المهمة الرئيسية.',
    },
    __avatar_nudge_switch_assistant__: {
      'zh-CN': '切换有点频繁，建议先回到当前主线。',
      'zh-TW': '切換有點頻繁，建議先回到目前主線。',
      en: 'Lots of switching. It may help to return to the current thread first.',
      ar: 'التبديل متكرر، يُستحسن العودة إلى المهمة الحالية أولاً.',
    },
    __avatar_nudge_switch_coach__: {
      'zh-CN': '别再切了，先把手上这段收住。',
      'zh-TW': '別再切了，先把手上這段收住。',
      en: 'Enough switching. Close this stretch before moving on.',
      ar: 'توقف عن التبديل وأنهِ ما بين يديك أولاً.',
    },
    __avatar_goal_achieved__: {
      'zh-CN': '今日专注目标达成！干得漂亮！',
      'zh-TW': '今日專注目標達成！幹得漂亮！',
      en: 'Daily focus goal reached! Great job!',
      ar: 'تم تحقيق هدف التركيز اليومي! عمل رائع!',
    },
    '先放松一下，待会再继续推进。': {
      'zh-CN': '先放松一下，待会再继续推进。',
      'zh-TW': '先放鬆一下，待會再繼續推進。',
      en: 'Take a short break, then continue when you are ready.',
      ar: 'خذ استراحة قصيرة ثم واصل لاحقاً.',
    },
    '该休息一下了，起来活动活动吧。': {
      'zh-CN': '该休息一下了，起来活动活动吧。',
      'zh-TW': '該休息一下了，起來活動活動吧。',
      en: 'Time for a break. Stand up and stretch a bit.',
      ar: 'حان وقت الاستراحة، انهض وتحرّك قليلاً.',
    },
    '开始整理日报，稍等我一下。': {
      'zh-CN': '开始整理日报，稍等我一下。',
      'zh-TW': '開始整理日報，稍等我一下。',
      en: "I'm preparing your daily report. Give me a moment.",
      ar: 'أُحضّر تقريرك اليومي، لحظة من فضلك.',
    },
    '日报整理好了，可以回来看看。': {
      'zh-CN': '日报整理好了，可以回来看看。',
      'zh-TW': '日報整理好了，可以回來看看。',
      en: 'Your daily report is ready. You can check it now.',
      ar: 'تقريرك اليومي جاهز، يمكنك الاطلاع عليه الآن.',
    },
    '这次日报整理失败了，稍后可以再试。': {
      'zh-CN': '这次日报整理失败了，稍后可以再试。',
      'zh-TW': '這次日報整理失敗了，稍後可以再試。',
      en: 'This report run failed. Please try again later.',
      ar: 'فشل إعداد التقرير هذه المرة، حاول لاحقاً.',
    },
  };

  const FOLLOWUP_PERSONA_LABEL_KEY: Record<AvatarPersona, string> = {
    companion: 'settingsAppearance.avatarPersonaCompanionTitle',
    assistant: 'settingsAppearance.avatarPersonaAssistantTitle',
    coach: 'settingsAppearance.avatarPersonaCoachTitle',
  };

  const FOLLOWUP_LEAD_KEY: Record<AvatarPersona, string> = {
    companion: 'settingsAppearance.avatarFollowupCompanionLead',
    assistant: 'settingsAppearance.avatarFollowupAssistantLead',
    coach: 'settingsAppearance.avatarFollowupCoachLead',
  };

  const FOLLOWUP_PERSONA_THEME: Record<AvatarPersona, FollowupPersonaTheme> = {
    companion: {
      badgeClass: 'bg-emerald-500/12 text-emerald-700',
      primaryClass: 'bg-emerald-500 hover:bg-emerald-600 text-white',
      surfaceClass: 'border-emerald-200/95 bg-[linear-gradient(180deg,rgba(255,255,255,0.98),rgba(236,253,245,0.98))]',
      strategyKey: 'settingsAppearance.avatarFollowupCompanionStrategy',
      focusKey: 'settingsAppearance.avatarFollowupFocusCompanion',
      focusFullKey: 'settingsAppearance.avatarFollowupFocusFullCompanion',
      rememberKey: 'settingsAppearance.avatarFollowupRememberCompanion',
      rememberFullKey: 'settingsAppearance.avatarFollowupRememberFullCompanion',
      snoozeKey: 'settingsAppearance.avatarFollowupSnoozeCompanion',
      snoozeFullKey: 'settingsAppearance.avatarFollowupSnoozeFullCompanion',
      timelineOpeningKey: 'settingsAppearance.avatarFollowupTimelineOpeningCompanion',
      rememberedKey: 'settingsAppearance.avatarFollowupRememberedCompanion',
      snoozedKey: 'settingsAppearance.avatarFollowupSnoozedCompanion',
      focusStartedKey: 'settingsAppearance.avatarFollowupFocusStartedCompanion',
      focusStoppedKey: 'settingsAppearance.avatarFollowupFocusStoppedCompanion',
      focusFinishedKey: 'settingsAppearance.avatarFollowupFocusFinishedCompanion',
    },
    assistant: {
      badgeClass: 'bg-sky-500/12 text-sky-700',
      primaryClass: 'bg-sky-500 hover:bg-sky-600 text-white',
      surfaceClass: 'border-sky-200/95 bg-[linear-gradient(180deg,rgba(255,255,255,0.98),rgba(239,246,255,0.98))]',
      strategyKey: 'settingsAppearance.avatarFollowupAssistantStrategy',
      focusKey: 'settingsAppearance.avatarFollowupFocus',
      focusFullKey: 'settingsAppearance.avatarFollowupFocusFull',
      rememberKey: 'settingsAppearance.avatarFollowupRemember',
      rememberFullKey: 'settingsAppearance.avatarFollowupRememberFull',
      snoozeKey: 'settingsAppearance.avatarFollowupSnooze',
      snoozeFullKey: 'settingsAppearance.avatarFollowupSnoozeFull',
      timelineOpeningKey: 'settingsAppearance.avatarFollowupTimelineOpening',
      rememberedKey: 'settingsAppearance.avatarFollowupRemembered',
      snoozedKey: 'settingsAppearance.avatarFollowupSnoozed',
      focusStartedKey: 'settingsAppearance.avatarFollowupFocusStarted',
      focusStoppedKey: 'settingsAppearance.avatarFollowupFocusStopped',
      focusFinishedKey: 'settingsAppearance.avatarFollowupFocusFinished',
    },
    coach: {
      badgeClass: 'bg-amber-500/14 text-amber-800',
      primaryClass: 'bg-amber-500 hover:bg-amber-600 text-slate-900',
      surfaceClass: 'border-amber-200/95 bg-[linear-gradient(180deg,rgba(255,255,255,0.98),rgba(255,251,235,0.98))]',
      strategyKey: 'settingsAppearance.avatarFollowupCoachStrategy',
      focusKey: 'settingsAppearance.avatarFollowupFocusCoach',
      focusFullKey: 'settingsAppearance.avatarFollowupFocusFullCoach',
      rememberKey: 'settingsAppearance.avatarFollowupRememberCoach',
      rememberFullKey: 'settingsAppearance.avatarFollowupRememberFullCoach',
      snoozeKey: 'settingsAppearance.avatarFollowupSnoozeCoach',
      snoozeFullKey: 'settingsAppearance.avatarFollowupSnoozeFullCoach',
      timelineOpeningKey: 'settingsAppearance.avatarFollowupTimelineOpeningCoach',
      rememberedKey: 'settingsAppearance.avatarFollowupRememberedCoach',
      snoozedKey: 'settingsAppearance.avatarFollowupSnoozedCoach',
      focusStartedKey: 'settingsAppearance.avatarFollowupFocusStartedCoach',
      focusStoppedKey: 'settingsAppearance.avatarFollowupFocusStoppedCoach',
      focusFinishedKey: 'settingsAppearance.avatarFollowupFocusFinishedCoach',
    },
  };

  function localizeBacklogNudgeMessage(message: string, nextLocale: Locale): string | null {
    if (!message?.startsWith('__avatar_backlog_nudge__:')) {
      return null;
    }

    const [, persona = 'assistant', countRaw = '0'] = message.split(':');
    const count = Number(countRaw) || 0;
    const key =
      persona === 'companion'
        ? 'settingsAppearance.avatarNudgeBacklogCompanion'
        : persona === 'coach'
          ? 'settingsAppearance.avatarNudgeBacklogCoach'
          : 'settingsAppearance.avatarNudgeBacklogAssistant';

    return t(key, { count, locale: nextLocale });
  }

  function localizeBubblePayload(
    payload: AvatarBubblePayload | null,
    nextLocale: Locale = currentLocale,
  ): AvatarBubblePayload | null {
    if (!payload) {
      return null;
    }

    if (payload.clear) {
      return payload;
    }

    const localizedMessage =
      localizeBacklogNudgeMessage(payload.message, nextLocale)
      || RUNTIME_BUBBLE_MESSAGES[payload.message]?.[nextLocale]
      || payload.message;

    return {
      ...payload,
      message: localizedMessage,
    };
  }

  function formatFocusCountdown(ms: number): string {
    const safeMs = Math.max(0, ms);
    const totalSeconds = Math.ceil(safeMs / 1000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    return `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`;
  }

  function buildFocusBubblePayload(session: AvatarFocusSession | null): AvatarBubblePayload | null {
    if (!session) {
      return null;
    }

    const countdown = formatFocusCountdown(session.endsAtMs - focusNowMs);
    return {
      message: t('settingsAppearance.avatarFollowupFocusActive', {
        countdown,
      }),
      persistent: true,
      tone: 'success',
    };
  }

  $: focusBubble = (() => {
    // focusNowMs must appear in the reactive expression so Svelte
    // re-evaluates every second when the timer ticks.
    void focusNowMs;
    if (focusSession && bubbleSource && focusNowMs < focusIntroUntilMs) {
      return null;
    }
    return buildFocusBubblePayload(focusSession);
  })();
  $: bubble = localizeBubblePayload(focusBubble || bubbleSource, currentLocale);
  $: followupCopy = buildFollowupCopy(followup);
  $: syncAvatarExpansion(followup != null);
  $: {
    // 以下状态均可能改变通知区域的位置或尺寸，DOM 更新后统一重新采集。
    void bubble;
    void followup;
    void bubbleFlipLeft;
    void currentLocale;
    void state.avatarBodyHidden;
    scheduleInteractiveRegionsSync();
  }

  function refreshInteractiveRegionObserver(elements: Element[]): void {
    const unchanged =
      elements.length === interactiveRegionElements.length
      && elements.every((element, index) => element === interactiveRegionElements[index]);

    if (unchanged) {
      return;
    }

    interactiveRegionObserver?.disconnect();
    interactiveRegionElements = elements;
    for (const element of elements) {
      interactiveRegionObserver?.observe(element);
    }
  }

  async function syncAvatarInteractiveRegions(): Promise<void> {
    if (interactiveRegionSyncPending) {
      interactiveRegionSyncRequested = true;
      return;
    }

    interactiveRegionSyncPending = true;
    try {
      do {
        interactiveRegionSyncRequested = false;
        await tick();

        if (!interactiveRegionsMounted) {
          return;
        }

        const precise = !!state.avatarBodyHidden;
        const elements = precise
          ? Array.from(document.querySelectorAll('[data-avatar-hit-region]'))
          : [];
        refreshInteractiveRegionObserver(elements);

        const regions = precise
          ? elements
              .map((element) => {
                const rect = element.getBoundingClientRect();
                return {
                  x: rect.x,
                  y: rect.y,
                  width: rect.width,
                  height: rect.height,
                };
              })
              .filter((region) => region.width > 0 && region.height > 0)
          : [];

        const signature = `${precise ? '1' : '0'}:${regions
          .map(
            (region) =>
              `${Math.round(region.x)},${Math.round(region.y)},${Math.round(region.width)},${Math.round(region.height)}`,
          )
          .join(';')}`;

        if (signature === lastInteractiveRegionsSignature) {
          continue;
        }

        try {
          await invoke('set_avatar_interactive_regions', {
            precise: !!state.avatarBodyHidden,
            regions,
          });
          lastInteractiveRegionsSignature = signature;
        } catch (e) {
          console.error('更新桌宠精确交互区域失败:', e);
        }
      } while (interactiveRegionSyncRequested && interactiveRegionsMounted);
    } finally {
      interactiveRegionSyncPending = false;
    }
  }

  function scheduleInteractiveRegionsSync(): void {
    if (!interactiveRegionsMounted) {
      return;
    }

    interactiveRegionSyncRequested = true;
    if (interactiveRegionSyncPending) {
      return;
    }

    if (interactiveRegionFrame !== null) {
      cancelAnimationFrame(interactiveRegionFrame);
    }
    interactiveRegionFrame = requestAnimationFrame(() => {
      interactiveRegionFrame = null;
      void syncAvatarInteractiveRegions();
    });
  }

  async function syncAvatarExpansion(
    expanded: boolean,
    options: AvatarExpansionOptions = {},
  ): Promise<void> {
    const { force = false } = options;

    if (!force && avatarExpanded === expanded) {
      return;
    }
    const previous = avatarExpanded;
    avatarExpanded = expanded;
    if (sizeCorrectionTimer !== null) {
      clearTimeout(sizeCorrectionTimer);
      sizeCorrectionTimer = null;
    }
    suppressNextResizeCorrection = true;
    suppressPositionSaveUntil = Date.now() + 450;
    if (resizeGuardTimer !== null) {
      clearTimeout(resizeGuardTimer);
      resizeGuardTimer = null;
    }
    try {
      await invoke('set_avatar_window_expanded', { expanded });
      void refreshBubbleEdge();
    } catch (e) {
      if (!force) {
        avatarExpanded = previous;
      }
      console.error('更新桌宠窗口尺寸失败:', e);
    } finally {
      if (resizeGuardTimer !== null) {
        clearTimeout(resizeGuardTimer);
      }
      resizeGuardTimer = setTimeout(() => {
        suppressNextResizeCorrection = false;
        resizeGuardTimer = null;
      }, 220);
    }
  }

  function scheduleAvatarSizeCorrection(): void {
    if (suppressNextResizeCorrection) {
      return;
    }

    if (sizeCorrectionTimer !== null) {
      clearTimeout(sizeCorrectionTimer);
    }
    sizeCorrectionTimer = setTimeout(async () => {
      sizeCorrectionTimer = null;
      const expanded = avatarExpanded === null ? followup != null : avatarExpanded;
      await syncAvatarExpansion(expanded, { force: true });
    }, 120);
  }

  function clearBubble(): void {
    bubbleSource = null;
    if (bubbleTimer !== null) {
      clearTimeout(bubbleTimer);
    }
    bubbleTimer = null;
  }

  function showBubble(payload: AvatarBubblePayload): void {
    if (payload?.clear) {
      clearBubble();
      return;
    }

    if ((focusSession || bubbleSource?.persistent) && !payload?.persistent) {
      return;
    }

    bubbleSource = payload;
    if (bubbleTimer !== null) {
      clearTimeout(bubbleTimer);
    }

    if (!payload?.persistent) {
      const durationMs =
        typeof payload?.durationMs === 'number' && Number.isFinite(payload.durationMs) && payload.durationMs > 0
          ? payload.durationMs
          : 4200;
      bubbleTimer = setTimeout(() => {
        bubbleSource = null;
        bubbleTimer = null;
      }, durationMs);
    }
  }

  function dismissBubble(): void {
    if (focusSession) {
      stopFocusSession(true);
      return;
    }
    clearBubble();
  }

  function getFollowupPersonaLabelKey(persona: AvatarPersona): string {
    return FOLLOWUP_PERSONA_LABEL_KEY[persona] || FOLLOWUP_PERSONA_LABEL_KEY.assistant;
  }

  function getFollowupLeadKey(persona: AvatarPersona): string {
    return FOLLOWUP_LEAD_KEY[persona] || FOLLOWUP_LEAD_KEY.assistant;
  }

  function formatFollowupAge(hours: number): string {
    const normalizedHours = Number(hours) || 0;
    if (normalizedHours <= 1) {
      return t('settingsAppearance.avatarFollowupAgeRecent');
    }
    return t('settingsAppearance.avatarFollowupAgeHours', { count: normalizedHours });
  }

  function truncateFollowupTitle(title: string, maxLength = 34): string {
    const normalized = title.trim();
    if (!normalized) {
      return '';
    }
    if (normalized.length <= maxLength) {
      return normalized;
    }
    return `${normalized.slice(0, Math.max(1, maxLength - 1)).trimEnd()}…`;
  }

  function buildFollowupCopy(payload: AvatarFollowupPayload | null) {
    if (!payload) {
      return null;
    }

    const theme =
      FOLLOWUP_PERSONA_THEME[payload.persona] || FOLLOWUP_PERSONA_THEME.assistant;

    return {
      title: t('settingsAppearance.avatarFollowupTitle'),
      personaLabel: t(getFollowupPersonaLabelKey(payload.persona)),
      summary: t(getFollowupLeadKey(payload.persona), {
        title: truncateFollowupTitle(payload.title),
      }),
      strategy: t(theme.strategyKey),
      meta: [
        payload.sourceApp,
        payload.intentLabel,
        formatFollowupAge(payload.sessionAgeHours),
      ].filter(Boolean).join(' · '),
      openTimeline: t('settingsAppearance.avatarFollowupOpenTimeline'),
      focus: t(theme.focusKey),
      focusFull: t(theme.focusFullKey),
      remember: t(theme.rememberKey),
      rememberFull: t(theme.rememberFullKey),
      snooze: t(theme.snoozeKey),
      snoozeFull: t(theme.snoozeFullKey),
      dismissLabel: t('settingsAppearance.avatarFollowupDismiss'),
      badgeClass: theme.badgeClass,
      primaryClass: theme.primaryClass,
      surfaceClass: theme.surfaceClass,
    };
  }

  function getFollowupTheme(persona: AvatarPersona): FollowupPersonaTheme {
    return FOLLOWUP_PERSONA_THEME[persona] || FOLLOWUP_PERSONA_THEME.assistant;
  }

  function buildFollowupActionInput(action: AvatarFollowupAction) {
    if (!followup) {
      return null;
    }

    return {
      action,
      projectKey: followup.projectKey,
      title: followup.title,
      date: followup.date,
      sourceApp: followup.sourceApp,
      sourceTitle: followup.sourceTitle,
      persona: followup.persona,
    };
  }

  async function submitFollowupAction(
    action: AvatarFollowupAction,
  ): Promise<AvatarFollowupPayload | null> {
    const snapshot = followup;
    if (!snapshot) {
      return null;
    }

    const input = buildFollowupActionInput(action);
    if (!input) {
      return null;
    }

    await invoke('handle_avatar_followup_action', { input });
    return snapshot;
  }

  function clearFollowup(): void {
    followup = null;
  }

  function clearFocusTimer(): void {
    if (focusTimer !== null) {
      clearInterval(focusTimer);
    }
    focusTimer = null;
  }

  function finishFocusSession(): void {
    const completedSession = focusSession;
    focusSession = null;
    focusNowMs = 0;
    clearFocusTimer();
    if (!completedSession) {
      return;
    }
    const theme = getFollowupTheme(state.avatarPersona);
    showBubble({
      message: t(theme.focusFinishedKey),
      tone: 'success',
      persistent: true,
    });
  }

  function ensureFocusTicking(): void {
    clearFocusTimer();
    if (!focusSession) {
      return;
    }
    focusNowMs = Date.now();
    focusTimer = setInterval(() => {
      focusNowMs = Date.now();
      if (focusSession && focusNowMs >= focusSession.endsAtMs) {
        finishFocusSession();
      }
    }, 1000);
  }

  function stopFocusSession(showEndedBubble = false): void {
    if (!focusSession) {
      return;
    }
    focusSession = null;
    focusNowMs = 0;
    focusIntroUntilMs = 0;
    clearFocusTimer();
    if (showEndedBubble) {
      clearBubble();
      const theme = getFollowupTheme(state.avatarPersona);
      showBubble({
        message: t(theme.focusStoppedKey),
        tone: 'success',
      });
    }
  }

  async function startFollowupFocus() {
    try {
      const payload = await submitFollowupAction('focus');
      if (!payload) {
        return;
      }

      clearFollowup();
      clearBubble();
      const now = Date.now();
      const theme = getFollowupTheme(payload.persona);
      focusIntroUntilMs = now + 2600;
      // 必须在写入 focusSession 之前展示开场提示，否则 showBubble 会把专注期非持久气泡过滤掉。
      showBubble({
        message: t(theme.focusStartedKey),
        tone: 'success',
        durationMs: 2600,
      });
      focusSession = {
        projectKey: payload.projectKey,
        title: payload.title,
        endsAtMs: now + 25 * 60 * 1000,
      };
      ensureFocusTicking();
    } catch (e) {
      console.error('桌宠开始专注失败:', e);
      showBubble({
        message: t('settingsAppearance.avatarFollowupActionFailed', { error: e }),
        persistent: true,
      });
    }
  }

  async function openFollowupTimeline() {
    try {
      const payload = await submitFollowupAction('timeline');
      if (!payload) {
        return;
      }

      clearFollowup();
      const theme = getFollowupTheme(payload.persona);
      showBubble({
        message: t(theme.timelineOpeningKey),
        tone: 'success',
      });
      await invoke('show_main_window', { sourceWindowLabel: appWindow.label });
      await emitTo('main', 'avatar-open-timeline', {
        date: payload.date,
        projectKey: payload.projectKey,
        title: payload.title,
      });
    } catch (e) {
      console.error('桌宠打开时间线失败:', e);
      showBubble({
        message: t('settingsAppearance.avatarFollowupActionFailed', { error: e }),
        persistent: true,
      });
    }
  }

  async function rememberFollowup() {
    try {
      const payload = await submitFollowupAction('remember');
      if (!payload) {
        return;
      }

      clearFollowup();
      const theme = getFollowupTheme(payload.persona);
      showBubble({
        message: t(theme.rememberedKey),
        tone: 'success',
      });
    } catch (e) {
      console.error('桌宠记为待跟进失败:', e);
      showBubble({
        message: t('settingsAppearance.avatarFollowupActionFailed', { error: e }),
        persistent: true,
      });
    }
  }

  async function snoozeFollowup() {
    try {
      const payload = await submitFollowupAction('snooze');
      if (!payload) {
        return;
      }

      clearFollowup();
      const theme = getFollowupTheme(payload.persona);
      showBubble({
        message: t(theme.snoozedKey),
        tone: 'success',
      });
    } catch (e) {
      console.error('桌宠稍后提醒失败:', e);
      showBubble({
        message: t('settingsAppearance.avatarFollowupActionFailed', { error: e }),
        persistent: true,
      });
    }
  }

  async function dismissFollowup() {
    try {
      await submitFollowupAction('dismiss');
    } catch (e) {
      console.error('关闭桌宠继续提醒失败:', e);
    } finally {
      clearFollowup();
    }
  }

  async function openMainWindow() {
    try {
      await invoke('show_main_window', { sourceWindowLabel: appWindow.label });
    } catch (e) {
      console.error('显示主窗口失败:', e);
    }
  }

  async function startAvatarDrag(event: AvatarPointerEvent): Promise<void> {
    const originalEvent = event instanceof CustomEvent ? event.detail.originalEvent : event;

    if (originalEvent.button !== 0) {
      return;
    }

    originalEvent.preventDefault?.();

    try {
      await nativeWindow.startDragging();
    } catch (e) {
      console.error('拖动桌宠失败:', e);
    }
  }

  function scheduleAvatarPositionSave(position: PhysicalPosition): void {
    if (Date.now() < suppressPositionSaveUntil) {
      return;
    }

    const nextX = Math.round(position.x);
    const nextY = Math.round(position.y);
    const nextKey = `${nextX},${nextY}`;

    if (positionSaveTimer !== null) {
      clearTimeout(positionSaveTimer);
    }
    positionSaveTimer = setTimeout(async () => {
      if (nextKey === lastSavedPositionKey) {
        return;
      }

      try {
        await invoke('save_avatar_position', { x: nextX, y: nextY });
        lastSavedPositionKey = nextKey;
      } catch (e) {
        console.error('保存桌宠位置失败:', e);
      }
    }, 240);
  }

  // 根据窗口在屏幕中的水平位置决定气泡朝向：靠近右侧时气泡向左展开，
  // 避免桌宠缩小并贴右下角时气泡向右溢出被裁（#137 诉求一）。
  async function refreshBubbleEdge() {
    const requestSeq = ++bubbleEdgeRequestSeq;
    try {
      const [position, outerSize, monitor] = await Promise.all([
        nativeWindow.outerPosition(),
        nativeWindow.outerSize().catch(() => null),
        currentMonitor(),
      ]);
      if (requestSeq !== bubbleEdgeRequestSeq || !monitor) return;
      const monitorRight = monitor.position.x + monitor.size.width;
      const halfWidth = outerSize && outerSize.width > 0 ? outerSize.width / 2 : 138;
      // 窗口中心越过显示器中线 → 视为靠近右侧，气泡翻转朝左
      const windowCenterX = position.x + halfWidth;
      bubbleFlipLeft = windowCenterX > (monitor.position.x + monitorRight) / 2;
    } catch (e) {
      // 取不到显示器信息时保持默认（右锚点），不影响主流程
    }
  }

  function scheduleBubbleEdgeRefresh(): void {
    if (bubbleEdgeTimer !== null) {
      clearTimeout(bubbleEdgeTimer);
    }
    bubbleEdgeTimer = setTimeout(() => {
      bubbleEdgeTimer = null;
      void refreshBubbleEdge();
    }, 80);
  }

  function scheduleNextMotionStep(): void {
    if (motionTimer !== null) {
      clearTimeout(motionTimer);
    }
    if (document.hidden || !windowVisible) {
      motionTimer = null;
      return;
    }
    const delay = getAvatarMotionStepDelay(state.mode, state.contextLabel, motionBeat);
    motionTimer = setTimeout(() => {
      motionBeat = (motionBeat + 1) % 96;
      scheduleNextMotionStep();
    }, delay);
  }

  onMount(() => {
    let disposed = false;
    let unlistenState = () => {};
    let unlistenBubble = () => {};
    let unlistenFollowup = () => {};
    let unlistenInput = () => {};
    let unlistenVisibility = () => {};
    let unlistenMoved = () => {};
    let unlistenResized = () => {};
    let unlistenLocaleChanged = () => {};
    interactiveRegionsMounted = true;
    interactiveRegionObserver = new ResizeObserver(() => {
      scheduleInteractiveRegionsSync();
    });
    scheduleInteractiveRegionsSync();
    initializeLocale();
    unsubscribeLocale = locale.subscribe((nextLocale) => {
      applyLocaleToDocument(nextLocale);
    });

    // 初始计算一次气泡朝向（窗口已定位后再读位置）
    const initialBubbleEdgeTimer = setTimeout(() => {
      if (!disposed) {
        void refreshBubbleEdge();
      }
    }, 0);
    if (!document.hidden && windowVisible) {
      scheduleNextMotionStep();
    }

    handleVisibilityChange = () => {
      if (document.hidden) {
        if (motionTimer !== null) {
          clearTimeout(motionTimer);
        }
        motionTimer = null;
      } else {
        scheduleNextMotionStep();
      }
    };
    document.addEventListener('visibilitychange', handleVisibilityChange);

    // 桌宠窗口不需要浏览器原生右键菜单和打印能力，避免误触后弹出系统界面。
    handleContextMenu = (event) => {
      event.preventDefault();
      event.stopPropagation();
    };
    handleKeydown = (event) => {
      if (
        (event.metaKey || event.ctrlKey) &&
        (event.key === 'p' || event.key === 'P')
      ) {
        event.preventDefault();
        event.stopPropagation();
      }
    };
    document.addEventListener('contextmenu', handleContextMenu);
    document.addEventListener('keydown', handleKeydown, true);

    const registerUnlisten = (unlisten: () => void, assign: (fn: () => void) => void): boolean => {
      if (disposed) {
        try {
          unlisten();
        } catch {}
        return false;
      }
      assign(unlisten);
      return true;
    };

    (async () => {
      try {
        const payload = await invoke<unknown>('get_avatar_state');
        if (disposed) return;
        const nextState = parseAvatarState(payload);
        if (nextState) {
          state = nextState;
        } else {
          console.warn('桌宠初始状态载荷格式无效，已保留默认状态');
        }
      } catch (e) {
        console.error('获取桌宠状态失败:', e);
      }
      if (disposed) return;

      const stateOff = await appWindow.listen<unknown>('avatar-state-changed', (event) => {
        const nextState = parseAvatarState(event.payload);
        if (!nextState) {
          console.warn('桌宠状态事件载荷格式无效，已忽略');
          return;
        }
        const stateChanged =
          nextState.mode !== state.mode || nextState.contextLabel !== state.contextLabel;
        const stateBubble = getAvatarStateBubble(
          nextState.mode,
          currentLocale,
          nextState.contextLabel,
          nextState.avatarPersona,
        );
        const transition = getAvatarTransitionMeta(
          state.mode,
          nextState.mode,
          state.contextLabel,
          nextState.contextLabel,
        );

        if (
          stateBubble &&
          stateChanged &&
          Date.now() - lastStateBubbleAt > 900
        ) {
          lastStateBubbleAt = Date.now();
          showBubble(stateBubble);
        }

        if (
          transition.className &&
          (
            nextState.mode !== state.mode ||
            nextState.contextLabel !== state.contextLabel
          )
        ) {
          transitionClass = transition.className;
          if (transitionTimer !== null) {
            clearTimeout(transitionTimer);
          }
          transitionTimer = setTimeout(() => {
            transitionClass = '';
            transitionTimer = null;
          }, transition.durationMs);
        }

        state = nextState;
        scheduleNextMotionStep();
      });
      if (!registerUnlisten(stateOff, (fn) => { unlistenState = fn; })) return;

      const bubbleOff = await appWindow.listen<unknown>('avatar-bubble', (event) => {
        const payload = parseAvatarBubblePayload(event.payload);
        if (!payload) {
          console.warn('桌宠气泡事件载荷格式无效，已忽略');
          return;
        }
        showBubble(payload);
      });
      if (!registerUnlisten(bubbleOff, (fn) => { unlistenBubble = fn; })) return;

      const followupOff = await appWindow.listen<unknown>('avatar-followup-suggestion', (event) => {
        const payload = parseAvatarFollowupPayload(event.payload);
        if (!payload) {
          console.warn('桌宠跟进事件载荷格式无效，已忽略');
          return;
        }
        followup = payload;
      });
      if (!registerUnlisten(followupOff, (fn) => { unlistenFollowup = fn; })) return;

      const inputOff = await appWindow.listen<unknown>('avatar-input-changed', (event) => {
        inputActivity = normalizeAvatarInputActivity(event.payload);

        if (inputActivity.keyboardActive || inputActivity.mouseActive) {
          const now = Date.now();
          if (!state.avatarBodyHidden && now - lastInputMotionBeatAt >= 180) {
            lastInputMotionBeatAt = now;
            motionBeat = (motionBeat + 1) % 96;
            scheduleNextMotionStep();
          }
        }
      });
      if (!registerUnlisten(inputOff, (fn) => { unlistenInput = fn; })) return;

      const visibilityOff = await appWindow.listen<boolean>('avatar-window-visibility', (event) => {
        windowVisible = event.payload !== false;
        document.body.classList.toggle('avatar-window-hidden', !windowVisible);
        if (!windowVisible) {
          if (motionTimer !== null) {
            clearTimeout(motionTimer);
          }
          motionTimer = null;
        } else {
          scheduleNextMotionStep();
          scheduleBubbleEdgeRefresh();
          scheduleInteractiveRegionsSync();
        }
      });
      if (!registerUnlisten(visibilityOff, (fn) => { unlistenVisibility = fn; })) return;

      const localeOff = await listen('locale-changed', (event) => {
        const nextLocale = event.payload;
        if (typeof nextLocale === 'string' && nextLocale) {
          initializeLocale(nextLocale);
        }
      });
      if (!registerUnlisten(localeOff, (fn) => { unlistenLocaleChanged = fn; })) return;

      const movedOff = await nativeWindow.onMoved(({ payload: position }) => {
        scheduleAvatarPositionSave(position);
        scheduleBubbleEdgeRefresh();
      });
      if (!registerUnlisten(movedOff, (fn) => { unlistenMoved = fn; })) return;

      // Windows 拖拽到桌面顶部附近时，系统可能短暂调整窗口尺寸。
      // 这里在 resize 后回正到当前配置尺寸，避免出现“拖一下就变小一圈”。
      const resizedOff = await nativeWindow.onResized(() => {
        scheduleAvatarSizeCorrection();
        scheduleInteractiveRegionsSync();
      });
      if (!registerUnlisten(resizedOff, (fn) => { unlistenResized = fn; })) return;
    })();

    return () => {
      disposed = true;
      clearTimeout(initialBubbleEdgeTimer);
      document.body.classList.remove('avatar-window-hidden');
      interactiveRegionsMounted = false;
      interactiveRegionSyncRequested = false;
      if (interactiveRegionFrame !== null) {
        cancelAnimationFrame(interactiveRegionFrame);
        interactiveRegionFrame = null;
      }
      interactiveRegionObserver?.disconnect();
      interactiveRegionObserver = null;
      interactiveRegionElements = [];
      if (bubbleTimer !== null) clearTimeout(bubbleTimer);
      if (bubbleEdgeTimer !== null) clearTimeout(bubbleEdgeTimer);
      if (transitionTimer !== null) clearTimeout(transitionTimer);
      if (positionSaveTimer !== null) clearTimeout(positionSaveTimer);
      if (motionTimer !== null) clearTimeout(motionTimer);
      if (sizeCorrectionTimer !== null) clearTimeout(sizeCorrectionTimer);
      if (resizeGuardTimer !== null) clearTimeout(resizeGuardTimer);
      clearFocusTimer();
      if (handleVisibilityChange) document.removeEventListener('visibilitychange', handleVisibilityChange);
      if (handleContextMenu) document.removeEventListener('contextmenu', handleContextMenu);
      if (handleKeydown) document.removeEventListener('keydown', handleKeydown, true);
      unsubscribeLocale();
      unlistenState();
      unlistenBubble();
      unlistenFollowup();
      unlistenInput();
      unlistenVisibility();
      unlistenLocaleChanged();
      unlistenMoved();
      unlistenResized();
    };
  });
</script>

<svelte:window on:dragover|preventDefault on:drop|preventDefault />

<div
  role="presentation"
  class="relative h-screen w-screen overflow-visible bg-transparent select-none"
  class:pointer-events-none={state.avatarBodyHidden}
  on:mousedown={(e) => {
    const target = e.target;
    if (
      state.avatarBodyHidden
      || (target instanceof Element
        && target.closest('button, a, section, .avatar-popover-anchor, [role="button"]'))
    ) return;
    startAvatarDrag(e);
  }}
>
  <div class="absolute inset-x-0 top-0 h-[86px] overflow-visible">
    <AvatarPopover {bubble} flipLeft={bubbleFlipLeft} onClose={dismissBubble} />
  </div>

  <AvatarFollowupCard
    followup={followup}
    copy={followupCopy}
    flipLeft={bubbleFlipLeft}
    onTimeline={openFollowupTimeline}
    onFocus={startFollowupFocus}
    onRemember={rememberFollowup}
    onSnooze={snoozeFollowup}
    onDismiss={dismissFollowup}
  />

  {#if !state.avatarBodyHidden}
    <div class="absolute inset-x-0 bottom-0 top-[78px] flex items-end justify-center overflow-visible">
      <div class="h-full w-[82%] pointer-events-auto">
        <AvatarCanvas
          {state}
          {inputActivity}
          {transitionClass}
          {motionBeat}
          on:avatarpointerdown={startAvatarDrag}
          on:avataractivate={openMainWindow}
        />
      </div>
    </div>
  {/if}
</div>

<style>
  :global(:root),
  :global(html),
  :global(body) {
    background: transparent !important;
  }

  :global(body) {
    margin: 0;
    overflow: hidden;
  }

  :global(body.avatar-window-hidden *),
  :global(body.avatar-window-hidden *::before),
  :global(body.avatar-window-hidden *::after) {
    animation-play-state: paused !important;
  }
</style>
