<script lang="ts">
  import { onMount, tick, type ComponentType } from 'svelte';
  import Router, { location, push } from 'svelte-spa-router';
  import { wrap } from 'svelte-spa-router/wrap';
  import Sidebar from './lib/components/Sidebar.svelte';
  import EvidenceShell from './lib/components/evidence/EvidenceShell.svelte';
  import EvidenceNavigation from './lib/components/evidence/EvidenceNavigation.svelte';
  import { applyUiTemplate, uiTemplate } from './lib/stores/uiTemplate.ts';
  import Toast from './lib/components/Toast.svelte';
  import ConfirmDialog from './lib/components/ConfirmDialog.svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  import { cache, getLocalDate } from './lib/stores/cache.ts';
  import { recordingStore } from './lib/stores/recording.ts';
  import { applyLocaleToDocument, initializeLocale, locale, t } from '$lib/i18n/index.ts';
  import { preloadAppIcons, type AppIconInvoke } from './lib/stores/iconCache.ts';
  import { updateConfigQueued } from './lib/utils/configSaveQueue.ts';
  import { runUpdateFlow } from './lib/utils/updater.ts';
  import { isTimelineActivity } from './routes/timeline/timelineData.ts';
  import { timelineGateway } from './routes/timeline/timelineGateway.ts';
  import Minus from 'lucide-svelte/icons/minus';
  import Square from 'lucide-svelte/icons/square';
  import X from 'lucide-svelte/icons/x';

  type Theme = 'system' | 'light' | 'dark';
  type UiVisualStyle = 'a' | 'b' | 'c';

  interface WorkTimeSegment {
    end_hour?: number;
    end_minute?: number;
  }

  interface RuntimeConfig {
    lightweight_mode?: boolean;
    theme?: Theme;
    ui_visual_style?: string;
    ui_template?: string;
    background_image?: string | null;
    background_opacity?: number;
    background_blur?: number;
    daily_report_auto_generate_time?: string | null;
    work_end_hour?: number;
    work_end_minute?: number;
    work_time_segments?: WorkTimeSegment[];
    memory_enabled?: boolean;
    memory_last_synthesis_date?: string | null;
  }

  interface DailyStatsPreview {
    browser_usage?: Array<{
      browser_name: string;
      executable_path?: string | null;
    }>;
    app_usage?: Array<{
      app_name: string;
      executable_path?: string | null;
    }>;
  }

  interface RecordingStateChangedPayload {
    isRecording: boolean;
    isPaused: boolean;
  }

  interface AvatarTimelinePayload {
    date?: string;
  }

  interface BackgroundChangeDetail {
    image?: string | null;
    opacity?: number;
    blur?: number;
  }

  interface UiVisualStyleChangeDetail {
    style?: string;
  }

  interface AutoReportWorkEnd {
    hour: number;
    minute: number;
  }

  interface AutoReportCandidate extends AutoReportWorkEnd {
    score: number;
  }

  interface BrowserPreviewWindow {
    label: string;
    startDragging: () => Promise<void>;
    close: () => Promise<void>;
    hide: () => Promise<void>;
    minimize: () => Promise<void>;
    isMaximized: () => Promise<boolean>;
    unmaximize: () => Promise<void>;
    maximize: () => Promise<void>;
    isVisible: () => Promise<boolean>;
  }

  function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
  }

  function isTheme(value: unknown): value is Theme {
    return value === 'system' || value === 'light' || value === 'dark';
  }

  function isRuntimeConfig(value: unknown): value is RuntimeConfig {
    if (!isRecord(value)) return false;

    const optionalStringFields = [
      'ui_visual_style',
      'ui_template',
      'background_image',
      'daily_report_auto_generate_time',
      'memory_last_synthesis_date',
    ];
    const optionalNumberFields = [
      'background_opacity',
      'background_blur',
      'work_end_hour',
      'work_end_minute',
    ];
    const optionalBooleanFields = ['lightweight_mode', 'memory_enabled'];

    if (optionalStringFields.some((field) => (
      value[field] !== undefined
      && value[field] !== null
      && typeof value[field] !== 'string'
    ))) return false;
    if (optionalNumberFields.some((field) => (
      value[field] !== undefined && typeof value[field] !== 'number'
    ))) return false;
    if (optionalBooleanFields.some((field) => (
      value[field] !== undefined && typeof value[field] !== 'boolean'
    ))) return false;
    if (value.theme !== undefined && !isTheme(value.theme)) return false;
    if (
      value.work_time_segments !== undefined
      && (
        !Array.isArray(value.work_time_segments)
        || value.work_time_segments.some((segment) => (
          !isRecord(segment)
          || (segment.end_hour !== undefined && typeof segment.end_hour !== 'number')
          || (segment.end_minute !== undefined && typeof segment.end_minute !== 'number')
        ))
      )
    ) return false;

    return true;
  }

  function isUiVisualStyle(value: string): value is UiVisualStyle {
    return value === 'a' || value === 'b' || value === 'c';
  }

  function isBoolean(value: unknown): value is boolean {
    return typeof value === 'boolean';
  }

  function isRecordingStateChangedPayload(
    value: unknown,
  ): value is RecordingStateChangedPayload {
    return isRecord(value)
      && typeof value.isRecording === 'boolean'
      && typeof value.isPaused === 'boolean';
  }

  function isRecordingStateTuple(value: unknown): value is [boolean, boolean] {
    return Array.isArray(value)
      && value.length === 2
      && value.every((item) => typeof item === 'boolean');
  }

  function isAvatarTimelinePayload(value: unknown): value is AvatarTimelinePayload {
    return isRecord(value)
      && (value.date === undefined || typeof value.date === 'string');
  }

  async function getRuntimeConfig(): Promise<RuntimeConfig> {
    const payload = await invoke<unknown>('get_config');
    if (!isRuntimeConfig(payload)) {
      throw new TypeError('配置载荷格式无效');
    }
    return payload;
  }

  // Development debug log: not output in production build to avoid polluting user console
  const devLog = (...args: unknown[]): void => {
    if (import.meta.env.DEV) console.log(...args);
  };

  function createBrowserPreviewWindow(): BrowserPreviewWindow {
    return {
      label: 'main',
      startDragging: async () => {},
      close: async () => {},
      hide: async () => {},
      minimize: async () => {},
      isMaximized: async () => false,
      unmaximize: async () => {},
      maximize: async () => {},
      isVisible: async () => true,
    };
  }

  function getSafeCurrentWebviewWindow() {
    try {
      return getCurrentWebviewWindow();
    } catch (e) {
      console.warn('Tauri window metadata missing in current environment, switched to browser preview mode:', e);
      return createBrowserPreviewWindow();
    }
  }

  async function safeListen<T>(
    eventName: string,
    guard: (payload: unknown) => payload is T,
    handler: (payload: T) => void | Promise<void>,
  ): Promise<() => void> {
    try {
      return await listen<unknown>(eventName, (event) => {
        if (!guard(event.payload)) {
          console.warn(`Tauri 事件 ${eventName} 的载荷格式无效，已忽略`);
          return;
        }
        try {
          const result = handler(event.payload);
          void Promise.resolve(result).catch((error) => {
            console.error(`处理 Tauri 事件 ${eventName} 失败:`, error);
          });
        } catch (error) {
          console.error(`处理 Tauri 事件 ${eventName} 失败:`, error);
        }
      });
    } catch (e) {
      console.warn(`Cannot register Tauri event ${eventName} in current environment, skipped:`, e);
      return () => {};
    }
  }

  const appWindow = getSafeCurrentWebviewWindow();
  const currentWindowLabel = appWindow.label;
  const isAvatarWindow = currentWindowLabel === 'avatar';
  let AvatarWindowComponent: ComponentType | null = null;

  if (isAvatarWindow) {
    import('./routes/avatar/AvatarWindow.svelte').then((module) => {
      AvatarWindowComponent = module.default;
    });
  }

  // 視窗拖拽（Linux WebKitGTK 不支援 -webkit-app-region: drag，改用 Tauri API）
  let lastDragClick = 0;
  async function startDrag(e: MouseEvent): Promise<void> {
    const target = e.target;
    if (e.button !== 0 || (target instanceof Element && target.closest('button'))) return;
    const now = Date.now();
    if (now - lastDragClick < 350) {
      lastDragClick = 0;
      await maximizeWindow();
      return;
    }
    lastDragClick = now;
    await appWindow.startDragging();
  }

  // 窗口控制函数
  async function closeWindow(): Promise<void> {
    if (runtimeConfig?.lightweight_mode) {
      await appWindow.close();
      return;
    }

    await appWindow.hide();
    syncMainWindowVisibility(false);
  }

  async function minimizeWindow(): Promise<void> {
    await appWindow.minimize();
  }

  async function maximizeWindow(): Promise<void> {
    const isMaximized = await appWindow.isMaximized();
    if (isMaximized) {
      await appWindow.unmaximize();
    } else {
      await appWindow.maximize();
    }
  }

  async function toggleEvidenceRecording(): Promise<void> {
    if (!isRecording || recordingTransitionPending) return;

    recordingTransitionPending = true;
    try {
      await invoke(isPaused ? 'resume_recording' : 'pause_recording');
    } catch (error) {
      console.error('切换录制状态失败:', error);
    } finally {
      recordingTransitionPending = false;
    }
  }

  // 预加载核心数据
  const invokeAppIcon: AppIconInvoke = (command, args) => invoke<string>(command, {
    appName: args.appName,
    executablePath: args.executablePath,
  });

  async function preloadApp(): Promise<void> {
    devLog('开始预加载数据...');
    const today = getLocalDate();
    
    // 并行预加载：概览、时间线(今天)、日报(今天)
    Promise.all([
      // 1. 概览
      invoke<DailyStatsPreview>('get_today_stats').then(stats => {
        cache.setOverview(stats);

        preloadAppIcons(
          (stats?.browser_usage || []).map((browser) => ({
            appName: browser.browser_name,
            executablePath: browser.executable_path,
          })),
          invokeAppIcon,
          { priority: true }
        );

        preloadAppIcons(
          (stats?.app_usage || []).slice(0, 6).map((app) => ({
            appName: app.app_name,
            executablePath: app.executable_path,
          })),
          invokeAppIcon
        );
      }),
      // 2. 时间线 (今天) - 仅预加载前 20 条
      Promise.all([
        timelineGateway.getPage({ date: today, limit: 20, offset: 0 }),
        timelineGateway.getHourlySummaries(today),
      ]).then(([activities, summaries]) => cache.setTimeline(
        today,
        activities,
        summaries,
      )),
      // 3. 日报 (今天) - 检查是否已存在（必须带上当前语言，否则会把中文日报缓存到其他语言的 key 下）
      invoke<unknown>('get_saved_report', { date: today, locale: $locale }).then(report => {
        if (report) cache.setReport(`${today}:${$locale}`, report);
      })
    ]).then(() => {
      devLog('预加载完成');
    }).catch(e => {
      console.warn('预加载部分失败:', e);
    });
  }

  const routes = {
    '/': wrap({ asyncComponent: () => import('./routes/Overview.svelte') }),
    '/timeline': wrap({ asyncComponent: () => import('./routes/timeline/Timeline.svelte') }),
    '/timeline/summary/:date': wrap({ asyncComponent: () => import('./routes/timeline/Summary.svelte') }),
    '/timeline/summary': wrap({ asyncComponent: () => import('./routes/timeline/Summary.svelte') }),
    '/report': wrap({ asyncComponent: () => import('./routes/report/Report.svelte') }),
    '/ask': wrap({ asyncComponent: () => import('./routes/ask/Ask.svelte') }),
    '/settings': wrap({ asyncComponent: () => import('./routes/settings/Settings.svelte') }),
    '/about': wrap({ asyncComponent: () => import('./routes/about/About.svelte') }),
  };

  let theme: Theme = 'system';
  let isDark = false;
  let isRecording = true;
  let isPaused = false;
  let recordingTransitionPending = false;
  let platform = '';
  let backgroundImage: string | null = null;
  let backgroundOpacity = 0.25;
  let backgroundBlur = 1;
  let runtimeConfig: RuntimeConfig | null = null;
  let uiVisualStyle: UiVisualStyle = 'c';
  let unsubscribeLocale: () => void = () => {};
  $: currentLocale = $locale;

  function detectSystemTheme(): boolean {
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
  }

  function applyTheme(newTheme: Theme): void {
    theme = newTheme;
    isDark = theme === 'system' ? detectSystemTheme() : theme === 'dark';
    
    if (isDark) {
      document.documentElement.classList.add('dark');
    } else {
      document.documentElement.classList.remove('dark');
    }
  }

  function normalizeUiVisualStyle(value: unknown): UiVisualStyle {
    const nextStyle = typeof value === 'string' ? value.trim().toLowerCase() : '';
    return isUiVisualStyle(nextStyle) ? nextStyle : 'c';
  }

  function applyUiVisualStyle(value: unknown): void {
    uiVisualStyle = normalizeUiVisualStyle(value);
    document.documentElement.dataset.uiVisualStyle = uiVisualStyle;
  }

  async function handleThemeChange(event: CustomEvent<Theme>): Promise<void> {
    const newTheme = event.detail;
    applyTheme(newTheme);

    try {
      const savedConfig = await updateConfigQueued<RuntimeConfig>((config) => {
        config.theme = newTheme;
      });
      cache.setConfig(savedConfig);
    } catch (e) {
      console.error('保存主题配置失败:', e);
    }
  }

  async function loadBackground() {
    try {
      const config = await getRuntimeConfig();
      backgroundOpacity = config.background_opacity ?? 0.25;
      backgroundBlur = config.background_blur ?? 1;
      if (config.background_image) {
        const b64 = await invoke<string | null>('get_background_image');
        if (b64) {
          backgroundImage = `data:image/jpeg;base64,${b64}`;
        }
      } else {
        backgroundImage = null;
      }
    } catch (e) {
      console.warn('加载背景图失败:', e);
    }
  }

  // 实时响应设置页的背景参数变更（不需要保存即可生效）
  function handleBackgroundChanged(e: Event): void {
    if (!(e instanceof CustomEvent)) return;
    const d: BackgroundChangeDetail = e.detail;
    if (d) {
      if (d.image !== undefined) backgroundImage = d.image;
      if (d.opacity !== undefined) backgroundOpacity = d.opacity;
      if (d.blur !== undefined) backgroundBlur = d.blur;
    }
  }

  function syncMainWindowVisibility(visible: boolean): void {
    document.body.classList.toggle('app-window-hidden', visible === false);
  }

  // 阻止文件拖拽到窗口时 WebView 导航到文件 URL
  function preventFileDrop(e: DragEvent): void {
    e.preventDefault();
  }

  // #126: 屏蔽 WebView 原生右键菜单，避免暴露浏览器默认上下文菜单
  function preventNativeContextMenu(e: MouseEvent): void {
    e.preventDefault();
  }

  function normalizeTimePart(value: unknown, upperBound: number): number {
    const parsed = Number.parseInt(String(value), 10);
    if (!Number.isFinite(parsed)) return 0;
    return Math.min(Math.max(parsed, 0), upperBound);
  }

  function resolveAutoReportWorkEnd(
    config: RuntimeConfig | null | undefined,
  ): AutoReportWorkEnd {
    // 优先使用用户自定义的日报生成时间
    const customTime = config?.daily_report_auto_generate_time;
    if (customTime && typeof customTime === 'string') {
      const parts = customTime.split(':').map(Number);
      if (parts.length >= 2 && Number.isFinite(parts[0]) && Number.isFinite(parts[1])) {
        return { hour: Math.min(Math.max(parts[0], 0), 23), minute: Math.min(Math.max(parts[1], 0), 59) };
      }
    }

    const fallbackHour = normalizeTimePart(config?.work_end_hour ?? 18, 23);
    const fallbackMinute = normalizeTimePart(config?.work_end_minute ?? 0, 59);
    const segments = Array.isArray(config?.work_time_segments) ? config.work_time_segments : [];
    if (segments.length === 0) {
      return { hour: fallbackHour, minute: fallbackMinute };
    }

    const latest = segments.reduce<AutoReportCandidate | null>((best, segment) => {
      const hour = normalizeTimePart(segment?.end_hour, 23);
      const minute = normalizeTimePart(segment?.end_minute, 59);
      const score = hour * 60 + minute;
      if (!best || score > best.score) {
        return { score, hour, minute };
      }
      return best;
    }, null);

    if (!latest) {
      return { hour: fallbackHour, minute: fallbackMinute };
    }
    return { hour: latest.hour, minute: latest.minute };
  }

  onMount(() => {
    // 全局阻止文件拖放导致页面导航（如拖入 PDF 会替换整个应用）
    window.addEventListener('dragover', preventFileDrop);
    window.addEventListener('drop', preventFileDrop);
    window.addEventListener('contextmenu', preventNativeContextMenu);

    if (isAvatarWindow) {
      return () => {
        window.removeEventListener('dragover', preventFileDrop);
        window.removeEventListener('drop', preventFileDrop);
        window.removeEventListener('contextmenu', preventNativeContextMenu);
      };
    }

    initializeLocale();
    unsubscribeLocale = locale.subscribe((nextLocale) => {
      applyLocaleToDocument(nextLocale);
    });

    let disposed = false;
    const pendingCleanup: Array<() => void> = [];

    // #118: 主窗口隐藏（静默驻留/轻量）时暂停 CSS 动画，降低后台 WebView2 GPU 占用
    safeListen('main-window-visibility', isBoolean, (payload) => {
      syncMainWindowVisibility(payload);
    }).then((unlisten) => {
      if (disposed) {
        try { if (unlisten) unlisten(); } catch {}
      } else {
        pendingCleanup.push(unlisten);
      }
    });

    // 同步注册的 locale subscription 立即可清理
    pendingCleanup.push(() => unsubscribeLocale());
    pendingCleanup.push(() => window.removeEventListener('dragover', preventFileDrop));
    pendingCleanup.push(() => window.removeEventListener('drop', preventFileDrop));
    pendingCleanup.push(() => window.removeEventListener('contextmenu', preventNativeContextMenu));

    (async () => {
      try {
        const visible = await appWindow.isVisible();
        if (!disposed) syncMainWindowVisibility(visible);
      } catch (e) {
        console.warn('同步主窗口可见性失败:', e);
      }
      if (disposed) return;

      // 获取平台信息
      try {
        platform = await invoke<string>('get_platform');
        devLog('当前平台:', platform);
      } catch (e) {
        console.error('获取平台信息失败:', e);
      }
      if (disposed) return;

      // 加载配置并应用主题
      let config: RuntimeConfig;
      try {
        config = await getRuntimeConfig();
        runtimeConfig = config;
        cache.setConfig(config);
        applyTheme(config.theme || 'system');
        applyUiVisualStyle(config.ui_visual_style || 'c');
        applyUiTemplate(config.ui_template || 'classic');
      } catch (e) {
        console.error('加载配置失败:', e);
        applyTheme('system');
        applyUiVisualStyle('c');
        applyUiTemplate('classic');
        config = { work_end_hour: 18 };
        runtimeConfig = config;
      }
      if (disposed) return;

      // 加载背景图
      loadBackground();

      try {
        const recordingState = await invoke<unknown>('get_recording_state');
        if (!isRecordingStateTuple(recordingState)) {
          throw new TypeError('录制状态载荷格式无效');
        }
        const [recording, paused] = recordingState;
        isRecording = recording;
        isPaused = paused;
        recordingStore.setState(recording, paused);
      } catch (e) {
        console.error('获取录制状态失败:', e);
      }
      if (disposed) return;

      const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
      const handleSystemThemeChange = () => {
        if (theme === 'system') applyTheme('system');
      };
      mediaQuery.addEventListener('change', handleSystemThemeChange);
      pendingCleanup.push(() => mediaQuery.removeEventListener('change', handleSystemThemeChange));

      const unsubscribeCache = cache.subscribe((state) => {
        if (!isRuntimeConfig(state.config)) return;
        runtimeConfig = state.config;

        if (state.config.theme && state.config.theme !== theme) {
          applyTheme(state.config.theme);
        }

        if (state.config.ui_visual_style && state.config.ui_visual_style !== uiVisualStyle) {
          applyUiVisualStyle(state.config.ui_visual_style);
        }

        applyUiTemplate(state.config.ui_template || 'classic');
      });
      pendingCleanup.push(unsubscribeCache);

      const unlistenRecordingState = await safeListen(
        'recording-state-changed',
        isRecordingStateChangedPayload,
        (payload) => {
          isRecording = payload.isRecording;
          isPaused = payload.isPaused;
          recordingStore.setState(payload.isRecording, payload.isPaused);
        },
      );
      if (disposed) { try { if (unlistenRecordingState) unlistenRecordingState(); } catch {} return; }
      pendingCleanup.push(unlistenRecordingState);

      const unlistenConfigChanged = await safeListen(
        'config-changed',
        isRuntimeConfig,
        (payload) => {
          runtimeConfig = payload;
          applyUiVisualStyle(payload.ui_visual_style || 'c');
          applyUiTemplate(payload.ui_template || 'classic');
          cache.setConfig(payload);
        },
      );
      if (disposed) { try { if (unlistenConfigChanged) unlistenConfigChanged(); } catch {} return; }
      pendingCleanup.push(unlistenConfigChanged);

      const unlistenAvatarTimeline = await safeListen(
        'avatar-open-timeline',
        isAvatarTimelinePayload,
        async (payload) => {
          const nextDate = typeof payload.date === 'string' ? payload.date.trim() : '';

          try {
            await push('/timeline');
            if (nextDate) {
              window.history.replaceState(
                window.history.state,
                '',
                `/timeline?date=${encodeURIComponent(nextDate)}`
              );
            }
            await tick();
            window.dispatchEvent(new CustomEvent('timeline-focus-date', { detail: payload }));
          } catch (e) {
            console.error('桌宠跳转时间线失败:', e);
          }
        },
      );
      if (disposed) { try { if (unlistenAvatarTimeline) unlistenAvatarTimeline(); } catch {} return; }
      pendingCleanup.push(unlistenAvatarTimeline);

      // 监听背景图更新事件（来自设置页，实时预览）
      const handleBgChange = (e: Event) => handleBackgroundChanged(e);
      window.addEventListener('background-changed', handleBgChange);
      pendingCleanup.push(() => window.removeEventListener('background-changed', handleBgChange));

      const handleUiVisualStyleChange = (event: Event) => {
        if (!(event instanceof CustomEvent)) return;
        const detail: UiVisualStyleChangeDetail = event.detail;
        applyUiVisualStyle(detail?.style || 'c');
      };
      window.addEventListener('ui-visual-style-changed', handleUiVisualStyleChange);
      pendingCleanup.push(() => window.removeEventListener('ui-visual-style-changed', handleUiVisualStyleChange));

      const handleUiTemplateChange = (event: Event) => {
        const detail = (event as CustomEvent<{ template?: string }>).detail;
        applyUiTemplate(detail?.template || 'classic');
      };
      window.addEventListener('ui-template-changed', handleUiTemplateChange);
      pendingCleanup.push(() => window.removeEventListener('ui-template-changed', handleUiTemplateChange));

      // 启动预加载
      preloadApp();

      // 启动后延迟执行一次自动更新检查，避免阻塞首屏渲染
      const autoUpdateTimer = setTimeout(async () => {
        try {
          const shouldCheck = await invoke<boolean>('should_check_updates');
          if (!shouldCheck) return;

          await runUpdateFlow({
            silentWhenUpToDate: true,
            confirmBeforeDownload: true,
            onStatusChange: () => {},
          });
        } catch (e) {
          console.warn('自动检查更新失败:', e);
        }
      }, 2000);
      pendingCleanup.push(() => clearTimeout(autoUpdateTimer));

      // 日报自动生成检测：每分钟检查一次
      let lastAutoGenDate: string | null = null;  // 防止同一天重复触发
      let autoGenRunning = false;  // 防止并发生成
      let memorySynthRunning = false;  // 防止洞察合成并发
      const autoReportTimer = setInterval(async () => {
        if (autoGenRunning) return;  // 上一轮还没完成，跳过
        const now = new Date();
        const currentTotalMinutes = now.getHours() * 60 + now.getMinutes();
        const today = getLocalDate();

        // 检查是否达到或已过工作结束时间
        const { hour: workEndHour, minute: workEndMinute } =
          resolveAutoReportWorkEnd(runtimeConfig);
        const workEndTotalMinutes = workEndHour * 60 + workEndMinute;

        // 条件：当前时间 >= 工作结束时间，且今天未自动生成过
        if (currentTotalMinutes >= workEndTotalMinutes && lastAutoGenDate !== today) {
          try {
            // 检查今日是否已有日报
            const existingReport = await invoke<unknown>('get_saved_report', { date: today, locale: currentLocale });
            if (!existingReport) {
              devLog('工作结束时间到达，自动生成日报...');
              autoGenRunning = true;
              try {
                await invoke('generate_report', { date: today, force: false, locale: currentLocale });
                cache.invalidate('report', `${today}:${currentLocale}`);
                lastAutoGenDate = today;
                devLog('日报自动生成完成');
              } finally {
                autoGenRunning = false;
              }
            } else {
              lastAutoGenDate = today;  // 已有日报，标记今天不再触发
            }
          } catch (e) {
            console.warn('日报自动生成失败:', e);
          }
        }

        // AI 工作记忆：每天工作结束后自动合成洞察
        if (currentTotalMinutes >= workEndTotalMinutes && !memorySynthRunning) {
          try {
            const config = await getRuntimeConfig();
            if (config.memory_enabled && config.memory_last_synthesis_date !== today) {
              memorySynthRunning = true;
              try {
                await invoke('synthesize_insights', {});
                // 在共享队列执行时读取最新配置，只更新合成日期字段。
                await updateConfigQueued<RuntimeConfig>((config) => {
                  config.memory_last_synthesis_date = today;
                });
                devLog('工作记忆合成完成');
              } finally {
                memorySynthRunning = false;
              }
            }
          } catch (e) {
            console.warn('工作记忆合成失败:', e);
          }
        }
      }, 60000);  // 每分钟检查一次
      pendingCleanup.push(() => clearInterval(autoReportTimer));

      const unlisten = await safeListen(
        'screenshot-taken',
        isTimelineActivity,
        (payload) => {
          devLog('截屏完成:', payload);

          // 1. 增量更新时间线缓存
          cache.addActivity(payload);

          // 2. 使概览缓存过期（下次访问或当前页面监听时刷新）
          cache.invalidate('overview');

          // 3. 发射自定义事件，通知当前页面实时更新
          window.dispatchEvent(new CustomEvent('activity-added', { detail: payload }));

          // 4. 抢先预热当前应用图标，浏览器记录优先级更高
          preloadAppIcons(
            [{
              appName: payload.app_name,
              executablePath: payload.executable_path,
            }],
            invokeAppIcon,
            { priority: Boolean(payload.browser_url) }
          );
        },
      );
      if (disposed) { try { if (unlisten) unlisten(); } catch {} return; }
      pendingCleanup.push(unlisten);
    })();

    return () => {
      disposed = true;
      pendingCleanup.forEach(fn => { try { fn(); } catch {} });
    };
  });
</script>

{#if isAvatarWindow}
  {#if AvatarWindowComponent}
    <svelte:component this={AvatarWindowComponent} />
  {/if}
{:else}
<div class="app-shell ui-style-{uiVisualStyle} template-{$uiTemplate} flex h-screen overflow-hidden relative">
  <div class="app-shell-ambient pointer-events-none absolute inset-0 z-0 opacity-80">
    <div class="absolute inset-x-0 top-0 h-40 bg-[radial-gradient(circle_at_top,rgba(99,102,241,0.14),transparent_62%)] dark:bg-[radial-gradient(circle_at_top,rgba(99,102,241,0.18),transparent_62%)]"></div>
    <div class="absolute -right-16 top-24 h-48 w-48 rounded-full bg-indigo-200/20 blur-3xl dark:bg-indigo-500/12"></div>
    <div class="absolute left-8 bottom-10 h-44 w-44 rounded-full bg-sky-200/20 blur-3xl dark:bg-sky-500/10"></div>
  </div>
  <!-- 背景图层：图片全强度 + 半透明遮罩控制显隐 -->
  {#if backgroundImage}
    <div class="absolute inset-0 z-0 overflow-hidden pointer-events-none">
      <!-- 图片（全强度，不用 opacity 避免色彩发白） -->
      <div
        class="absolute inset-[-20px] bg-cover bg-center bg-no-repeat"
        style="background-image: url({backgroundImage}); filter: blur({backgroundBlur === 0 ? 0 : backgroundBlur === 1 ? 8 : 16}px);"
      ></div>
      <!-- 半透明遮罩：遮罩越透明 = 背景图越明显 -->
      <div
        class="absolute inset-0 bg-slate-50 dark:bg-[#161b22] transition-opacity duration-300"
        style="opacity: {Math.max(0, 1 - backgroundOpacity)};"
      ></div>
    </div>
  {/if}

  <!--
    全局顶部拖拽层 (Invisible Drag Layer)
    1. 覆盖在所有内容之上 (z-50)
    2. 负责处理窗口拖动 (-webkit-app-region: drag)
    3. 按钮区域排除拖动 (-webkit-app-region: no-drag)
  -->
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div class="app-shell-windowbar absolute top-0 left-0 w-full h-7 z-50" style="-webkit-app-region: drag;" on:mousedown={startDrag}>
    <!-- 仅 Windows/Linux 平台显示自定义窗口控制按钮，macOS 使用原生控件 -->
    {#if platform && platform !== 'macos'}
    <!-- Windows 风格窗口控制按钮 (右上角) -->
    <div class="app-shell-window-controls absolute right-0 top-0 flex items-stretch h-7" style="-webkit-app-region: no-drag;">
      <!-- Minimize -->
      <button
        on:click={minimizeWindow}
        class="app-shell-window-btn"
        title={t('window.minimize')}
      >
        <Minus size={15} strokeWidth={1.8} aria-hidden="true" />
      </button>

      <!-- Maximize -->
      <button
        on:click={maximizeWindow}
        class="app-shell-window-btn"
        title={t('window.maximize')}
      >
        <Square size={13} strokeWidth={1.7} aria-hidden="true" />
      </button>

      <!-- Close -->
      <button
        on:click={closeWindow}
        class="app-shell-window-btn app-shell-window-btn-close"
        title={t('window.close')}
      >
        <X size={15} strokeWidth={1.8} aria-hidden="true" />
      </button>
    </div>
    {/if}
  </div>

  <!-- 注意：这里不能加 z-index（如 z-10），否则会形成层叠上下文，
       把内部弹窗/Toast（z-[100..210]）整体压到拖拽条 z-50 之下，
       导致弹窗顶部 28px 被拖拽层拦截成"点击变拖动窗口"。 -->
  {#if $uiTemplate === 'evidence-star-map'}
    <EvidenceShell>
      <svelte:fragment slot="navigation">
        <EvidenceNavigation
          activeRoute={$location}
          {isRecording}
          {isPaused}
          {recordingTransitionPending}
          {theme}
          on:toggle-recording={toggleEvidenceRecording}
          on:themeChange={handleThemeChange}
        />
      </svelte:fragment>
      <section class="evidence-shell-route" aria-label={t('app.mainContent')}>
        {#key currentLocale}<Router {routes} />{/key}
      </section>
      <svelte:fragment slot="inspector"><div class="evidence-shell-inspector" aria-hidden="true"></div></svelte:fragment>
    </EvidenceShell>
    <Toast /><ConfirmDialog />
  {:else}
  <div class="app-shell-stage relative flex-1 grid grid-cols-[13.5rem_minmax(0,1fr)] gap-3 m-2 {platform !== 'macos' ? 'app-shell-stage--windowbar' : 'app-shell-stage--macos'}">
    <!-- 左侧边栏 -->
    <aside class="app-shell-sidebar-frame min-h-0">
      <div class="app-shell-sidebar h-full flex flex-col overflow-hidden">
        <Sidebar {isRecording} {isPaused} {theme} on:themeChange={handleThemeChange} />
      </div>
    </aside>

    <!-- 右侧主内容区域 -->
    <section class="app-shell-main-frame min-h-0">
      <div class="app-shell-main relative h-full flex flex-col overflow-hidden">
        <main class="app-shell-main-scroll flex-1 overflow-auto">
          {#key currentLocale}
            <Router {routes} />
          {/key}
        </main>
        <Toast />
        <ConfirmDialog />
      </div>
    </section>
  </div>
  {/if}
</div>
{/if}
