<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { get } from 'svelte/store';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type Event as TauriEvent } from '@tauri-apps/api/event';
  import ChartNoAxesColumn from 'lucide-svelte/icons/chart-no-axes-column';
  import ChartNoAxesGantt from 'lucide-svelte/icons/chart-no-axes-gantt';
  import Check from 'lucide-svelte/icons/check';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';
  import ChevronUp from 'lucide-svelte/icons/chevron-up';
  import Globe2 from 'lucide-svelte/icons/globe-2';
  import LayoutDashboard from 'lucide-svelte/icons/layout-dashboard';
  import Pencil from 'lucide-svelte/icons/pencil';
  import Plus from 'lucide-svelte/icons/plus';
  import Sparkles from 'lucide-svelte/icons/sparkles';
  import X from 'lucide-svelte/icons/x';
  import StatsCard from '../lib/components/StatsCard.svelte';
  import AppUsageChart from '../lib/components/AppUsageChart.svelte';
  import ActivityHourlyChart from '../lib/components/ActivityHourlyChart.svelte';
  import LocalizedDatePicker from '../lib/components/LocalizedDatePicker.svelte';
  import EvidenceOverviewHeader from '../lib/components/evidence/EvidenceOverviewHeader.svelte';
  import { cache } from '../lib/stores/cache.ts';
  import { recordingStore, isActiveRecording } from '../lib/stores/recording.ts';
  import { uiTemplate } from '../lib/stores/uiTemplate.ts';
  import { confirm } from '../lib/stores/confirm.ts';
  import { showToast } from '../lib/stores/toast.ts';
  import { preloadAppIcons, type AppIconInvoke } from '../lib/stores/iconCache.ts';
  import {
    formatDurationLocalized,
    formatLocalizedDate,
    formatLocalizedTime,
    locale,
    t,
    translateCategoryLabel,
    translateSemanticCategoryLabel,
  } from '$lib/i18n/index.ts';
  import { formatUserError } from '$lib/utils/errorDisplay.ts';
  import { trapFocus } from '$lib/utils/focusTrap.ts';
  import { formatBrowserUrlForDisplay } from '../lib/utils/browserUrl.ts';
  import { getViewportPopoverPlacement } from '../lib/utils/popoverPosition.ts';
  import {
    semanticCategoryStore,
    type CategoryInfo,
    type SemanticCategoryInfo,
  } from '../lib/stores/categories.ts';
  import {
    buildDomainPresentation,
    getSemanticCategoryColor,
  } from './overviewDomainPresentation.ts';
  import { buildCategoryCompositionSummary } from './overviewCategoryPresentation.ts';

  interface AppUsage {
    app_name: string;
    duration: number;
    count: number;
    executable_path: string | null;
    screenshot_url?: string | null;
  }

  interface CategoryUsage {
    category: string;
    duration: number;
  }

  interface HourlyActivityBucket {
    hour: number;
    duration: number;
  }

  interface HourlyAppDuration {
    app_name: string;
    duration: number;
    category: string;
    screenshot_url?: string | null;
  }

  interface HourlyAppBucket {
    hour: number;
    total_duration: number;
    apps: HourlyAppDuration[];
  }

  interface UrlDetail {
    url: string;
    duration: number;
  }

  interface DomainBrowserSource {
    browser_name: string;
    duration: number;
    percentage: number;
    urls?: UrlDetail[];
  }

  interface DomainItem {
    domain: string;
    duration: number;
    semantic_category: string | null;
    urls?: UrlDetail[];
    page_count?: number;
    browser_sources?: DomainBrowserSource[];
  }

  interface BrowserUsage {
    browser_name: string;
    duration: number;
    executable_path: string | null;
    domains: DomainItem[];
  }

  interface DailyStats {
    total_duration: number;
    screenshot_count: number;
    app_usage: AppUsage[];
    category_usage: CategoryUsage[];
    browser_duration: number;
    domain_usage: DomainItem[];
    domain_total_count: number;
    browser_usage: BrowserUsage[];
    work_time_duration: number;
    overtime_duration: number;
    hourly_activity_distribution: HourlyActivityBucket[];
  }

  interface RangeDailyTotal {
    date: string;
    total_duration: number;
  }

  interface OverviewDomainCollection {
    total_count: number;
    domains: DomainItem[];
  }

  interface OverviewDomainDetail extends DomainItem {
    page_count: number;
    urls: UrlDetail[];
    browser_sources: DomainBrowserSource[];
  }

  interface DeleteSemanticCategoryTarget {
    key: string;
    name: string;
  }

  interface WorkGoalConfig {
    daily_work_goal_minutes?: number | null;
  }

  interface CategoryActiveRange {
    startHour: number;
    endHour: number;
  }

  interface OverviewRefreshOptions {
    silent?: boolean;
  }

  type OverviewMode = 'today' | 'week' | 'date';
  type AppUsageViewMode = 'row' | 'column';
  type DomainOverlayView = 'detail' | 'all';

  function isDailyStats(value: unknown): value is DailyStats {
    return typeof value === 'object'
      && value !== null
      && 'app_usage' in value
      && Array.isArray(Reflect.get(value, 'app_usage'))
      && 'category_usage' in value
      && Array.isArray(Reflect.get(value, 'category_usage'));
  }

  async function safeListen<T>(
    eventName: string,
    handler: (event: TauriEvent<T>) => void,
  ): Promise<() => void> {
    try {
      return await listen(eventName, handler);
    } catch (e) {
      console.warn(`当前环境无法注册 Tauri 事件 ${eventName}，已跳过:`, e);
      return () => {};
    }
  }

  function getLocalDateString() {
    const now = new Date();
    const year = now.getFullYear();
    const month = String(now.getMonth() + 1).padStart(2, '0');
    const day = String(now.getDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
  }

  function parseDateString(dateValue: string): Date {
    return new Date(`${dateValue}T12:00:00`);
  }

  function getDateRangeLabel(dateFrom: string, dateTo: string): string {
    if (!dateFrom && !dateTo) {
      return '';
    }
    if (dateFrom && !dateTo) {
      return formatLocalizedDate(parseDateString(dateFrom), {
        year: 'numeric',
        month: 'long',
        day: 'numeric',
        weekday: 'short',
      });
    }
    if (!dateFrom && dateTo) {
      return formatLocalizedDate(parseDateString(dateTo), {
        year: 'numeric',
        month: 'long',
        day: 'numeric',
        weekday: 'short',
      });
    }
    if (dateFrom === dateTo) {
      return formatLocalizedDate(parseDateString(dateFrom), {
        year: 'numeric',
        month: 'long',
        day: 'numeric',
        weekday: 'short',
      });
    }
    return `${formatLocalizedDate(parseDateString(dateFrom), { year: 'numeric', month: 'short', day: 'numeric' })} - ${formatLocalizedDate(parseDateString(dateTo), { year: 'numeric', month: 'short', day: 'numeric' })}`;
  }

  function getWeekRangeLabel(dateValue: string): string {
    const anchor = parseDateString(dateValue);
    const monday = new Date(anchor);
    monday.setDate(anchor.getDate() - ((anchor.getDay() + 6) % 7));
    return `${formatLocalizedDate(monday, { year: 'numeric', month: 'short', day: 'numeric' })} - ${formatLocalizedDate(anchor, { year: 'numeric', month: 'short', day: 'numeric' })}`;
  }

  function getWeekDateRange(dateValue: string) {
    const anchor = parseDateString(dateValue);
    const monday = new Date(anchor);
    monday.setDate(anchor.getDate() - ((anchor.getDay() + 6) % 7));
    return {
      dateFrom: formatIsoDate(monday),
      dateTo: formatIsoDate(anchor),
    };
  }

  function formatIsoDate(date: Date): string {
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
  }

  function shiftIsoDate(dateValue: string, offsetDays: number): string {
    const next = parseDateString(dateValue);
    next.setDate(next.getDate() + offsetDays);
    return formatIsoDate(next);
  }

  function diffIsoDateDays(leftDateValue: string, rightDateValue: string): number {
    const dayInMs = 24 * 60 * 60 * 1000;
    return Math.round((parseDateString(leftDateValue).getTime() - parseDateString(rightDateValue).getTime()) / dayInMs);
  }

  // 应用投入保留用户偏好的展示方式；今日节奏固定使用竖向小时图。
  const APP_USAGE_VIEW_MODE_KEY = 'overview.appUsage.viewMode';
  const invokeAppIcon: AppIconInvoke = (command, args) => invoke<string>(command, {
    appName: args.appName,
    executablePath: args.executablePath,
  });

  let stats: DailyStats | null = null;
  let loading = true;
  let error: string | null = null;
  let unlisten: (() => void) | null = null;
  let componentDestroyed = false;
  let currentTime = new Date();
  let overviewMode: OverviewMode = 'today';
  let selectedCompositionCategory: string | null = null;
  let selectedDateFrom = getLocalDateString();
  let selectedDateTo = getLocalDateString();
  let clockInterval: ReturnType<typeof setInterval> | null = null;
  let refreshInterval: ReturnType<typeof setInterval> | null = null;
  let handleActivityAdded: EventListener | null = null;
  let handleVisibilityChange: EventListener | null = null;
  let overviewRefreshPromise: Promise<void> | null = null;
  let overviewRefreshKey = '';
  let overviewRequestId = 0;
  let lastCheckDate = currentTime.getDate();
  let appUsageViewMode: AppUsageViewMode = 'row';
  let domainUsageExpanded = false;
  let expandedDomainUsageItems: DomainItem[] = [];
  let domainUsageLoading = false;
  let domainUsageRequestId = 0;
  // #104: 按分类着色的柱状图（堆叠）
  let hourlyAppBreakdown: HourlyAppBucket[] = [];
  let categoryList: CategoryInfo[] = [];
  let workGoalMinutes: number | null = null;
  // ── 2026-07 概览改版 ──
  // 上周同日基线（today 模式的 KPI 差值与洞察条；加载失败时保持 null，不显示 delta）
  let lastWeekStats: DailyStats | null = null;
  let lastWeekStatsDate: string | null = null;
  let lastWeekStatsPromise: Promise<void> | null = null;
  // week/date 模式「按天投入」：来自新命令 get_range_daily_totals
  let rangeDailyTotals: RangeDailyTotal[] = [];
  let rangeDailyLoading = false;
  let rangeDailyRequestId = 0;
  let hourlyBreakdownRequestId = 0;
  function getHourlyBreakdownRange() {
    if (overviewMode === 'week') {
      return getWeekDateRange(getLocalDateString());
    }
    if (overviewMode === 'date') {
      return {
        dateFrom: selectedDateFrom,
        dateTo: selectedDateTo,
      };
    }
    const today = getLocalDateString();
    return {
      dateFrom: today,
      dateTo: today,
    };
  }

  async function loadHourlyBreakdown() {
    const range = getHourlyBreakdownRange();
    const requestId = ++hourlyBreakdownRequestId;
    try {
      const breakdown = await invoke<HourlyAppBucket[]>('get_hourly_app_breakdown', {
        mode: overviewMode,
        date: range.dateTo,
        dateFrom: range.dateFrom,
        dateTo: range.dateTo,
      });
      if (requestId === hourlyBreakdownRequestId) {
        hourlyAppBreakdown = breakdown;
      }
    } catch (e) {
      if (requestId === hourlyBreakdownRequestId) {
        hourlyAppBreakdown = [];
      }
    }
  }
  // 切换日期时刷新 hourly 分类细分
  $: { overviewMode; selectedDateFrom; selectedDateTo; if (categoryList.length) loadHourlyBreakdown(); }

  // week/date 模式的「按天投入」数据（today 模式不显示该卡，直接清空）
  async function loadRangeDailyTotals() {
    if (overviewMode === 'today') {
      rangeDailyTotals = [];
      return;
    }
    const range = getHourlyBreakdownRange();
    const requestId = ++rangeDailyRequestId;
    rangeDailyLoading = true;
    try {
      const totals = await invoke<RangeDailyTotal[]>('get_range_daily_totals', {
        dateFrom: range.dateFrom,
        dateTo: range.dateTo,
      });
      if (requestId === rangeDailyRequestId) {
        rangeDailyTotals = totals;
      }
    } catch (e) {
      if (requestId === rangeDailyRequestId) {
        rangeDailyTotals = [];
      }
    } finally {
      if (requestId === rangeDailyRequestId) {
        rangeDailyLoading = false;
      }
    }
  }
  // 切换模式/日期时刷新按天投入
  $: { overviewMode; selectedDateFrom; selectedDateTo; loadRangeDailyTotals(); }

  // today 模式并行拉取「上周同日」基线；绕过 overview 缓存
  // （cache.setOverview 只保存一份"今天"快照，写入 date 模式数据会互相污染），
  // 同日期仅拉一次，失败保持 null（界面不显示 delta）。
  function ensureLastWeekBaseline() {
    if (overviewMode !== 'today') {
      return;
    }
    const baselineDate = shiftIsoDate(getLocalDateString(), -7);
    if (lastWeekStatsDate === baselineDate && (lastWeekStats || lastWeekStatsPromise)) {
      return;
    }
    lastWeekStatsDate = baselineDate;
    lastWeekStatsPromise = invoke<DailyStats>('get_overview_stats', {
      mode: 'date',
      dateFrom: baselineDate,
      dateTo: baselineDate,
    })
      .then((baseline) => {
        lastWeekStats = baseline;
      })
      .catch((e) => {
        console.warn('加载上周同日基线失败:', e);
        lastWeekStats = null;
      })
      .finally(() => {
        lastWeekStatsPromise = null;
      });
  }
  $: hourlyCategoryColors = categoryList.reduce<Record<string, string>>((acc, c) => {
    acc[c.key] = c.color;
    return acc;
  }, {});
  $: hourlyCategoryNames = categoryList.reduce<Record<string, string>>((acc, c) => {
    currentLocale;
    const translatedCategoryName = translateCategoryLabel(c.key);
    const isKnownSystemCategory = c.is_system || translatedCategoryName !== c.key;
    acc[c.key] = isKnownSystemCategory ? translatedCategoryName : (c.name || translatedCategoryName);
    return acc;
  }, {});
  $: hourlyCategoryBreakdown = hourlyAppBreakdown.reduce<Record<string, CategoryUsage[]>>((acc, bucket) => {
    const cats: Record<string, number> = {};
    for (const app of bucket.apps || []) {
      const k = app.category || 'other';
      cats[k] = (cats[k] || 0) + app.duration;
    }
    acc[bucket.hour] = Object.entries(cats).map(([category, duration]) => ({ category, duration }));
    return acc;
  }, {});

  // ── 分类构成（构成条+图例、娱乐占比 KPI、洞察句主分类共用）：
  //    与 KPI 总投入/应用列表同源，优先 stats.category_usage（逐条裁剪口径）；
  //    缺失或为空时回退到 hourlyCategoryBreakdown 跨小时求和。
  //    小时图本身不动（小时口径，与日合计允许既知偏差）。
  $: hourlyCompositionTotals = Object.values(hourlyCategoryBreakdown).reduce<Record<string, number>>((acc, segments) => {
    for (const segment of segments || []) {
      acc[segment.category] = (acc[segment.category] || 0) + segment.duration;
    }
    return acc;
  }, {});
  $: compositionTotals = stats?.category_usage?.length
    ? stats.category_usage.reduce<Record<string, number>>((acc, item) => {
        acc[item.category] = (acc[item.category] || 0) + item.duration;
        return acc;
      }, {})
    : hourlyCompositionTotals;
  $: compositionTotalDuration = Object.values(compositionTotals).reduce((sum, duration) => sum + duration, 0);
  $: compositionSegments = compositionTotalDuration > 0
    ? Object.entries(compositionTotals)
        .map(([category, duration]) => ({
          category,
          duration,
          name: hourlyCategoryNames[category] || category,
          color: hourlyCategoryColors[category] || '#94a3b8',
          widthPct: (duration / compositionTotalDuration) * 100,
          percent: Math.round((duration / compositionTotalDuration) * 100),
        }))
        .sort((left, right) => right.duration - left.duration)
    : [];

  $: selectedCompositionSummary = selectedCompositionCategory
    ? buildCategoryCompositionSummary({
        category: selectedCompositionCategory,
        compositionTotals,
        hourlyBreakdown: hourlyCategoryBreakdown,
        appBreakdown: hourlyAppBreakdown,
      })
    : null;

  function toggleCompositionCategory(category: string) {
    selectedCompositionCategory = selectedCompositionCategory === category ? null : category;
  }

  function clearSelectedCompositionCategory() {
    selectedCompositionCategory = null;
  }

  function formatCompositionActiveRange(activeRange: CategoryActiveRange | null): string {
    if (!activeRange) return t('overview.compositionNoActiveRange');
    return `${String(activeRange.startHour).padStart(2, '0')}:00–${String(activeRange.endHour + 1).padStart(2, '0')}:00`;
  }

  // ── 专注峰值：hourly 分布最大桶向相邻延伸（相邻桶 ≥ 最大值 60% 时并入窗口） ──
  function computePeakWindow(distribution: readonly HourlyActivityBucket[]) {
    const buckets = Array.from({ length: 24 }, (_, hour) => {
      const found = (distribution || []).find((bucket) => bucket.hour === hour);
      return found?.duration || 0;
    });
    const maxDuration = Math.max(...buckets);
    if (maxDuration <= 0) {
      return null;
    }
    const peakHour = buckets.indexOf(maxDuration);
    const threshold = maxDuration * 0.6;
    let startHour = peakHour;
    let endHour = peakHour;
    while (startHour > 0 && buckets[startHour - 1] >= threshold) {
      startHour -= 1;
    }
    while (endHour < 23 && buckets[endHour + 1] >= threshold) {
      endHour += 1;
    }
    const totalDuration = buckets
      .slice(startHour, endHour + 1)
      .reduce((sum, duration) => sum + duration, 0);
    return { startHour, endHour, totalDuration };
  }

  function formatSignedCompactDuration(diffSeconds: number): string {
    const sign = diffSeconds >= 0 ? '+' : '−';
    return `${sign}${formatDurationLocalized(Math.abs(diffSeconds), { compact: true })}`;
  }

  function entertainmentSharePctOf(source: DailyStats | null): number | null {
    if (!source || !(source.total_duration > 0)) {
      return null;
    }
    const entertainment = (source.category_usage || []).find((item) => item.category === 'entertainment');
    return Math.round(((entertainment?.duration || 0) / source.total_duration) * 100);
  }

  $: peakWindow = stats ? computePeakWindow(stats.hourly_activity_distribution) : null;
  $: peakWindowValue = peakWindow
    ? t('overview.peakHoursValue', { from: peakWindow.startHour, to: peakWindow.endHour + 1 })
    : '--';
  $: peakWindowClockLabel = peakWindow
    ? `${String(peakWindow.startHour).padStart(2, '0')}:00–${String(peakWindow.endHour + 1).padStart(2, '0')}:00`
    : '';
  $: peakWindowSubtitle = peakWindow
    ? t('overview.peakWindowDuration', { dur: formatDurationLocalized(peakWindow.totalDuration, { compact: true }) })
    : null;

  // ── KPI 参照系（today 模式且基线可用时才显示 delta） ──
  $: totalDeltaSubtitle = overviewMode === 'today' && stats && lastWeekStats
    ? t('overview.deltaVsLastWeek', {
        delta: formatSignedCompactDuration(stats.total_duration - lastWeekStats.total_duration),
      })
    : null;
  $: workShareSubtitle = stats && stats.total_duration > 0
    ? `${t('overview.workShare', {
        percent: Math.round(((stats.work_time_duration || 0) / stats.total_duration) * 100),
      })}${stats.overtime_duration > 0 ? ` · ${t('overview.overtimeBadge', { dur: formatDurationLocalized(stats.overtime_duration) })}` : ''}`
    : null;
  // 娱乐占比 = 构成聚合里 key == 'entertainment' 的时长 / 总投入
  $: entertainmentSharePct = stats && stats.total_duration > 0
    ? Math.round(((compositionTotals.entertainment || 0) / stats.total_duration) * 100)
    : null;
  $: entertainmentShareValueText = entertainmentSharePct == null ? '--' : `${entertainmentSharePct}%`;
  $: entertainmentDeltaSubtitle = (() => {
    if (overviewMode !== 'today' || entertainmentSharePct == null) {
      return null;
    }
    // 基线只有 category_usage 可用，同口径取 entertainment / total
    const baselinePct = entertainmentSharePctOf(lastWeekStats);
    if (baselinePct == null) {
      return null;
    }
    const diff = entertainmentSharePct - baselinePct;
    return t('overview.deltaVsLastWeek', { delta: `${diff >= 0 ? '+' : '−'}${Math.abs(diff)}%` });
  })();

  // ── 洞察条（仅 today 模式、数据非空、基线可用时组句） ──
  $: insightSentence = (() => {
    if (overviewMode !== 'today' || !stats || !(stats.total_duration > 0) || !peakWindow || !lastWeekStats) {
      return null;
    }
    const diff = stats.total_duration - lastWeekStats.total_duration;
    const deltaText = formatDurationLocalized(Math.abs(diff));
    if (diff < 0) {
      return t('overview.insightSentenceLess', { peak: peakWindowClockLabel, delta: deltaText });
    }
    const topCategory = compositionSegments[0];
    if (!topCategory) {
      return null;
    }
    return t('overview.insightSentence', {
      peak: peakWindowClockLabel,
      delta: deltaText,
      category: topCategory.name,
    });
  })();

  // ── 节奏主视觉卡标题（today/week/date 三态） ──
  $: rhythmCardTitle = overviewMode === 'week'
    ? t('overview.typicalDayTitle')
    : overviewMode === 'date'
      ? t('overview.rhythmRangeTitle')
      : t('overview.todayRhythm');

  // ── 按天投入（week/date 模式） ──
  function formatDailyBarDayLabel(dateValue: string, totalDays: number): string {
    const parsed = parseDateString(dateValue);
    return totalDays <= 7
      ? formatLocalizedDate(parsed, { weekday: 'short' })
      : formatLocalizedDate(parsed, { day: 'numeric' });
  }
  $: maxRangeDailyTotal = rangeDailyTotals.reduce((max, day) => Math.max(max, day.total_duration || 0), 0);
  $: heaviestDailyEntry = maxRangeDailyTotal > 0
    ? rangeDailyTotals.find((day) => day.total_duration === maxRangeDailyTotal)
    : null;
  $: dailyBars = rangeDailyTotals.map((day) => ({
    date: day.date,
    total: day.total_duration || 0,
    label: formatDailyBarDayLabel(day.date, rangeDailyTotals.length),
    isToday: day.date === getLocalDateString(),
    isHeaviest: !!heaviestDailyEntry && day.date === heaviestDailyEntry.date,
    heightPx: maxRangeDailyTotal > 0 && day.total_duration > 0
      ? Math.round(((day.total_duration || 0) / maxRangeDailyTotal) * 110) + 4
      : 3,
  }));

  // ── 常驻网站：首页只带前 6 条，展开时按需请求完整轻量摘要。 ──
  $: domainUsageItems = stats?.domain_usage || [];
  $: topDomains = domainUsageExpanded && expandedDomainUsageItems.length > 0
    ? expandedDomainUsageItems
    : domainUsageItems.slice(0, 6);
  $: topDomainPresentations = topDomains.map((domain) => ({
    ...domain,
    presentation: buildDomainPresentation(domain, stats?.browser_usage || []),
  }));
  $: domainBrowsersLabel = (stats?.browser_usage || [])
    .map((browser) => browser.browser_name)
    .filter(Boolean)
    .join(', ');
  let overviewViewModeReady = false;
  
  let expandedDomains = new Set<string>();
  let editingDomainKey: string | null = null;
  let editingSemanticCategory = '';
  let pendingDomainSemanticRequests = new Map<string, number>();
  let nextDomainSemanticRequestId = 0;
  let domainSemanticEditSessionId = 0;
  let semanticCategoryPopover: HTMLDivElement | null = null;
  let semanticPopoverStyle = '';
  const domainSemanticTriggers = new Map<string, HTMLElement>();

  // 语义分类（新建 + 删除 + 重命名）
  let showCreateSemanticCategory = false;
  let newSemanticCategoryName = '';
  let semanticCategorySaving = false;
  let pendingDeleteSemanticCategory: DeleteSemanticCategoryTarget | null = null;

  // 重命名语义分类
  let showRenameSemanticCategory = false;
  let renameSemanticKey = '';
  let renameSemanticName = '';

  function startRenameSemanticCategory(cat: SemanticCategoryInfo) {
    renameSemanticKey = cat.key;
    renameSemanticName = cat.name;
    showCreateSemanticCategory = false;
    showRenameSemanticCategory = true;
  }

  async function saveRenameSemanticCategory() {
    const name = renameSemanticName.trim();
    if (!name) return;
    semanticCategorySaving = true;
    try {
      await invoke('save_custom_semantic_category', {
        key: renameSemanticKey,
        name,
      });
      await semanticCategoryStore.refresh();
      showRenameSemanticCategory = false;
      showToast(t('overview.semanticCategoryRenamed'), 'success');
    } catch (e) {
      showToast(String(e), 'error');
    } finally {
      semanticCategorySaving = false;
    }
  }

  function cancelDeleteSemanticCategory() { pendingDeleteSemanticCategory = null; }
  async function confirmDeleteSemanticCategory() {
    if (!pendingDeleteSemanticCategory) return;
    const { key, name } = pendingDeleteSemanticCategory;
    pendingDeleteSemanticCategory = null;
    semanticCategorySaving = true;
    try {
      const affected = await invoke<number>('delete_custom_semantic_category', { key });
      await semanticCategoryStore.refresh();
      showToast(
        t('overview.semanticCategoryDeleted', { category: name, count: affected }),
        'success'
      );
    } catch (e) {
      showToast(String(e), 'error');
    } finally {
      semanticCategorySaving = false;
    }
  }

  async function createCustomSemanticCategory() {
    const name = newSemanticCategoryName.trim();
    if (!name) {
      showToast(t('overview.semanticCategoryNameRequired'), 'error');
      return;
    }
    try {
      let key = name.toLowerCase().replace(/[^a-z0-9-]/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '');
      if (!key || key === '-') {
        let hash = 0;
        for (let i = 0; i < name.length; i++) {
          hash = ((hash << 5) - hash + name.charCodeAt(i)) | 0;
        }
        key = 'scat-' + Math.abs(hash).toString(36);
      }
      await invoke('save_custom_semantic_category', { key, name });
      await semanticCategoryStore.refresh();
      showCreateSemanticCategory = false;
      newSemanticCategoryName = '';
      showToast(t('overview.semanticCategoryCreated'), 'success');
    } catch (e) {
      showToast(String(e), 'error');
    }
  }

  function getSemanticCategoryDisplayName(cat: SemanticCategoryInfo): string {
    const translatedSemanticCategoryName = translateSemanticCategoryLabel(cat.key);
    const isKnownSemanticCategory = cat.is_system || translatedSemanticCategoryName !== cat.key;
    return isKnownSemanticCategory ? translatedSemanticCategoryName : (cat.name || translatedSemanticCategoryName);
  }
  
  // 域名摘要 / 单域名详情浮层
  let domainOverlayOpen = false;
  let domainOverlayView: DomainOverlayView = 'detail';
  let domainCollection: DomainItem[] = [];
  let domainCollectionTotalCount = 0;
  let selectedDomainDetail: OverviewDomainDetail | null = null;
  let domainOverlayLoading = false;
  let domainOverlayError: string | null = null;
  let domainOverlayRequestId = 0;
  let domainOverlayDialog: HTMLDivElement | null = null;
  let domainOverlayBackButton: HTMLButtonElement | null = null;
  $: currentLocale = $locale;
  $: isSingleSelectedDate = selectedDateFrom === selectedDateTo;
  $: canStepOverviewDateForward = selectedDateTo < getLocalDateString();
  $: overviewSubtitle = overviewMode === 'date'
    ? getDateRangeLabel(selectedDateFrom, selectedDateTo)
    : overviewMode === 'week'
      ? `${t('overview.modeWeek')} · ${getWeekRangeLabel(getLocalDateString())}`
      : formatLocalizedDate(new Date(), { year: 'numeric', month: 'long', day: 'numeric', weekday: 'short' });
  $: overviewStatusLabel = overviewMode === 'today' ? t('overview.live') : t(`overview.${overviewMode === 'date' ? 'modeDate' : 'modeWeek'}`);
  $: overviewIsLive = overviewMode !== 'date';
  // 圆点绿+脉冲还需要"正在录制"：停止记录后即便在"今天"模式，圆点也应灰掉（issue #131）
  $: recordingState = $recordingStore;
  $: overviewDotActive = overviewIsLive && isActiveRecording(recordingState);
  $: overviewTotalActivityTitle = overviewMode === 'week'
    ? t('overview.totalActivityWeek')
    : overviewMode === 'date'
      ? t(isSingleSelectedDate ? 'overview.totalActivityDate' : 'overview.totalActivityRange')
      : t('overview.totalActivityToday');
  $: overviewWorkDurationTitle = overviewMode === 'week'
    ? t('overview.workDurationWeek')
    : overviewMode === 'date'
      ? t(isSingleSelectedDate ? 'overview.workDurationDate' : 'overview.workDurationRange')
      : t('overview.workDurationToday');
  $: appUsageViewModeLabel = appUsageViewMode === 'column' ? t('overview.appUsageColumn') : t('overview.appUsageBar');
  $: hourlyChartPeakHourLabel = overviewMode === 'week'
    ? t('hourlyChart.peakHourRange')
    : overviewMode === 'date'
      ? t(isSingleSelectedDate ? 'hourlyChart.peakHour' : 'hourlyChart.peakHourRange')
      : t('hourlyChart.peakHour');
  $: hourlyChartPeakDurationLabel = overviewMode === 'week'
    ? t('hourlyChart.peakDurationRange')
    : overviewMode === 'date'
      ? t(isSingleSelectedDate ? 'hourlyChart.peakDuration' : 'hourlyChart.peakDurationRange')
      : t('hourlyChart.peakDuration');
  $: hourlyChartDistributionTitle = overviewMode === 'week'
    ? t('hourlyChart.distributionTitleWeek')
    : overviewMode === 'date'
      ? t(isSingleSelectedDate ? 'hourlyChart.distributionTitleDate' : 'hourlyChart.distributionTitleRange')
      : t('hourlyChart.distributionTitleToday');
  $: hourlyChartDistributionSubtitleKey = overviewMode === 'week'
    ? 'hourlyChart.distributionSubtitleRange'
    : overviewMode === 'date'
      ? (isSingleSelectedDate ? 'hourlyChart.distributionSubtitle' : 'hourlyChart.distributionSubtitleRange')
      : 'hourlyChart.distributionSubtitle';
  $: overviewNoWebsiteVisitsText = overviewMode === 'week'
    ? t('overview.noWebsiteVisitsWeek')
    : overviewMode === 'date'
      ? t(isSingleSelectedDate ? 'overview.noWebsiteVisitsDate' : 'overview.noWebsiteVisitsRange')
      : t('overview.noWebsiteVisitsToday');
  $: overviewNoAppStatsText = overviewMode === 'week'
    ? t('overview.noAppStatsWeek')
    : overviewMode === 'date'
      ? t(isSingleSelectedDate ? 'overview.noAppStatsDate' : 'overview.noAppStatsRange')
      : t('overview.noAppStatsToday');

  function readStoredOverviewViewMode(key: string, fallback: AppUsageViewMode): AppUsageViewMode {
    try {
      const value = window.localStorage.getItem(key);
      return value === 'column' || value === 'row' ? value : fallback;
    } catch {
      return fallback;
    }
  }

  function persistOverviewViewMode(key: string, value: AppUsageViewMode) {
    try {
      window.localStorage.setItem(key, value);
    } catch {
      // ignore persistence errors
    }
  }

  // 响应式图标加载：stats 变化时自动触发
  $: if (stats) {
    if (stats.app_usage?.length) {
      preloadAppIcons(stats.app_usage.slice(0, 10).map(a => ({
        appName: a.app_name,
        executablePath: a.executable_path,
      })), invokeAppIcon);
    }
  }

  function formatDuration(seconds: number): string {
    return formatDurationLocalized(seconds);
  }

  const UNRESOLVED_BROWSER_DOMAIN_LABEL = '未识别页面';

  function isUnresolvedBrowserDomain(domain: DomainItem): boolean {
    return domain?.domain === UNRESOLVED_BROWSER_DOMAIN_LABEL;
  }

  function getBrowserDomainDisplayLabel(domain: DomainItem): string {
    return isUnresolvedBrowserDomain(domain) ? t('overview.unresolvedPage') : domain.domain;
  }

  function getDomainSemanticLabel(domain: DomainItem): string {
    if (!domain?.semantic_category?.trim()) return t('overview.autoDetected');
    return semanticCategoryStore.getSemanticCategoryDisplayName(domain.semantic_category.trim());
  }

  function registerDomainSemanticTrigger(node: HTMLElement, domainKey: string) {
    let currentDomainKey = domainKey;
    if (currentDomainKey) domainSemanticTriggers.set(currentDomainKey, node);

    return {
      update(nextDomainKey: string) {
        if (currentDomainKey && domainSemanticTriggers.get(currentDomainKey) === node) {
          domainSemanticTriggers.delete(currentDomainKey);
        }
        currentDomainKey = nextDomainKey;
        if (currentDomainKey) domainSemanticTriggers.set(currentDomainKey, node);
      },
      destroy() {
        if (currentDomainKey && domainSemanticTriggers.get(currentDomainKey) === node) {
          domainSemanticTriggers.delete(currentDomainKey);
        }
      },
    };
  }

  function updateSemanticPopoverPosition() {
    if (!editingDomainKey || typeof window === 'undefined') {
      semanticPopoverStyle = '';
      return;
    }
    const trigger = domainSemanticTriggers.get(editingDomainKey);
    if (!trigger) {
      semanticPopoverStyle = '';
      return;
    }

    const position = getViewportPopoverPlacement(trigger.getBoundingClientRect(), {
      viewportWidth: window.innerWidth,
      viewportHeight: window.innerHeight,
      preferredWidth: 352,
    });
    const verticalStyle = position.top === null
      ? `top: auto; bottom: ${position.bottom}px;`
      : `top: ${position.top}px; bottom: auto;`;
    semanticPopoverStyle = `left: ${position.left}px; width: ${position.width}px; max-height: ${position.maxHeight}px; ${verticalStyle}`;
  }

  function handleSemanticPopoverViewportChange() {
    if (editingDomainKey) updateSemanticPopoverPosition();
  }

  async function startDomainSemanticEdit(domain: DomainItem) {
    domainSemanticEditSessionId += 1;
    editingDomainKey = domain.domain;
    editingSemanticCategory = domain.semantic_category?.trim() || '';
    showCreateSemanticCategory = false;
    newSemanticCategoryName = '';
    showRenameSemanticCategory = false;
    renameSemanticKey = '';
    renameSemanticName = '';
    await tick();
    updateSemanticPopoverPosition();
    await tick();
    semanticCategoryPopover?.focus();
  }

  function getSemanticCategoryOptions() {
    const options = [...$semanticCategoryStore];
    if (
      editingSemanticCategory &&
      !options.some((category) => category.key === editingSemanticCategory)
    ) {
      return [{
        key: editingSemanticCategory,
        name: semanticCategoryStore.getSemanticCategoryDisplayName(editingSemanticCategory),
        is_system: true,
      }, ...options];
    }
    return options;
  }

  function isDomainSemanticSavePending(domainKey: string): boolean {
    return pendingDomainSemanticRequests.has(domainKey);
  }

  function setDomainSemanticSavePending(domainKey: string, requestId: number) {
    pendingDomainSemanticRequests = new Map(pendingDomainSemanticRequests);
    pendingDomainSemanticRequests.set(domainKey, requestId);
  }

  function clearDomainSemanticSavePending(domainKey: string, requestId: number) {
    if (pendingDomainSemanticRequests.get(domainKey) !== requestId) return;
    pendingDomainSemanticRequests = new Map(pendingDomainSemanticRequests);
    pendingDomainSemanticRequests.delete(domainKey);
  }

  function isCurrentDomainSemanticEdit(domainKey: string, editSessionId: number): boolean {
    return domainSemanticEditSessionId === editSessionId
      && editingDomainKey === domainKey
      && domainOverlayOpen
      && selectedDomainDetail?.domain === domainKey;
  }

  function isCurrentDomainSemanticSave(domainKey: string, requestId: number, editSessionId: number): boolean {
    return pendingDomainSemanticRequests.get(domainKey) === requestId
      && isCurrentDomainSemanticEdit(domainKey, editSessionId);
  }

  function cancelDomainSemanticEdit({ restoreFocus = true }: { restoreFocus?: boolean } = {}) {
    const domainKey = editingDomainKey;
    domainSemanticEditSessionId += 1;
    editingDomainKey = null;
    editingSemanticCategory = '';
    semanticPopoverStyle = '';
    showCreateSemanticCategory = false;
    newSemanticCategoryName = '';
    showRenameSemanticCategory = false;
    renameSemanticKey = '';
    renameSemanticName = '';
    if (!restoreFocus || !domainKey) return;
    tick().then(() => domainSemanticTriggers.get(domainKey)?.focus());
  }

  function closeDomainOverlay() {
    domainOverlayRequestId += 1;
    domainOverlayOpen = false;
    domainOverlayView = 'detail';
    domainCollection = [];
    domainCollectionTotalCount = 0;
    selectedDomainDetail = null;
    domainOverlayLoading = false;
    domainOverlayError = null;
    cancelDomainSemanticEdit({ restoreFocus: false });
  }

  function focusDomainOverlayView() {
    tick().then(() => {
      if (!domainOverlayOpen) return;
      const summaryButton = domainOverlayDialog?.querySelector<HTMLElement>('[data-domain-summary]');
      const target = domainOverlayView === 'all'
        ? (summaryButton || domainOverlayDialog)
        : (domainOverlayBackButton || domainOverlayDialog);
      target?.focus();
    });
  }

  function getOverviewDomainParams(domain?: string) {
    const range = getHourlyBreakdownRange();
    return {
      ...(domain ? { domain } : {}),
      mode: overviewMode,
      date: range.dateTo,
      dateFrom: range.dateFrom,
      dateTo: range.dateTo,
    };
  }

  async function loadDomainDetail(domainKey: string): Promise<boolean> {
    const requestId = ++domainOverlayRequestId;
    domainOverlayOpen = true;
    domainOverlayView = 'detail';
    selectedDomainDetail = null;
    domainOverlayLoading = true;
    domainOverlayError = null;
    focusDomainOverlayView();
    try {
      const detail = await invoke<OverviewDomainDetail>('get_overview_domain_detail', getOverviewDomainParams(domainKey));
      if (requestId !== domainOverlayRequestId || !domainOverlayOpen) return false;
      selectedDomainDetail = detail;
      return true;
    } catch (e) {
      if (requestId !== domainOverlayRequestId || !domainOverlayOpen) return false;
      domainOverlayError = formatUserError(e, t('common.loadFailedRetry'));
      return false;
    } finally {
      if (requestId === domainOverlayRequestId) domainOverlayLoading = false;
    }
  }

  async function openDomainDetail(domain: DomainItem) {
    if (!domain?.domain) return;
    cancelDomainSemanticEdit({ restoreFocus: false });
    const availableDomains = expandedDomainUsageItems.length > 0
      ? expandedDomainUsageItems
      : domainUsageItems;
    domainCollection = availableDomains.map((item) => ({
      ...item,
      browser_sources: item.browser_sources
        || buildDomainPresentation(item, stats?.browser_usage || []).browserSources,
    }));
    domainCollectionTotalCount = stats?.domain_total_count || availableDomains.length;
    await loadDomainDetail(domain.domain);
  }

  async function toggleDomainUsageExpanded() {
    if (domainUsageExpanded) {
      domainUsageRequestId += 1;
      domainUsageExpanded = false;
      expandedDomainUsageItems = [];
      domainUsageLoading = false;
      return;
    }

    if ((stats?.domain_total_count || domainUsageItems.length) <= domainUsageItems.length) {
      expandedDomainUsageItems = domainUsageItems;
      domainUsageExpanded = true;
      return;
    }

    const requestId = ++domainUsageRequestId;
    domainUsageLoading = true;
    try {
      const collection = await invoke<OverviewDomainCollection>('get_overview_domains', getOverviewDomainParams());
      if (requestId !== domainUsageRequestId) return;
      expandedDomainUsageItems = collection?.domains || [];
      domainUsageExpanded = true;
    } catch (e) {
      if (requestId !== domainUsageRequestId) return;
      showToast(t('overview.domainLoadFailed'), 'error');
    } finally {
      if (requestId === domainUsageRequestId) {
        domainUsageLoading = false;
      }
    }
  }

  function resetDomainUsageExpansion() {
    domainUsageRequestId += 1;
    domainUsageExpanded = false;
    expandedDomainUsageItems = [];
    domainUsageLoading = false;
  }

  async function selectDomainFromCollection(domain: DomainItem) {
    if (!domain?.domain) return;
    cancelDomainSemanticEdit({ restoreFocus: false });
    await loadDomainDetail(domain.domain);
  }

  function showAllDomainSummaries() {
    cancelDomainSemanticEdit({ restoreFocus: false });
    domainOverlayView = 'all';
    selectedDomainDetail = null;
    domainOverlayError = null;
    focusDomainOverlayView();
  }

  async function refreshCurrentDomainDetail(
    domainKey: string,
    isCurrent: () => boolean = () => true,
  ): Promise<boolean> {
    if (!isCurrent()) return false;
    await loadStats(true);
    if (!isCurrent()) return false;
    const detail = await invoke<OverviewDomainDetail>('get_overview_domain_detail', getOverviewDomainParams(domainKey));
    if (!isCurrent()) return false;
    selectedDomainDetail = detail;
    domainCollection = domainCollection.map((domain) =>
      domain.domain === domainKey
        ? { ...domain, semantic_category: detail.semantic_category, duration: detail.duration }
        : domain
    );
    expandedDomainUsageItems = expandedDomainUsageItems.map((domain) =>
      domain.domain === domainKey
        ? { ...domain, semantic_category: detail.semantic_category, duration: detail.duration }
        : domain
    );
    return true;
  }

  function shouldUseOverviewCache() {
    return overviewMode === 'today';
  }

  function shouldAutoRefreshOverview() {
    return overviewMode !== 'date';
  }

  function setOverviewMode(mode: OverviewMode) {
    if (overviewMode === mode) {
      return;
    }
    overviewMode = mode;
    if (mode === 'date') {
      selectedDateFrom = getLocalDateString();
      selectedDateTo = getLocalDateString();
    }
    clearSelectedCompositionCategory();
    resetDomainUsageExpansion();
    closeDomainOverlay();
    loadStats(true);
  }

  function normalizeSelectedDateRange() {
    if (selectedDateTo < selectedDateFrom) {
      selectedDateTo = selectedDateFrom;
    }
  }

  function handleOverviewDateChange() {
    normalizeSelectedDateRange();
    clearSelectedCompositionCategory();
    resetDomainUsageExpansion();
    closeDomainOverlay();
    loadStats(true);
  }

  function stepOverviewDateRange(offsetDays: number) {
    clearSelectedCompositionCategory();
    const today = getLocalDateString();
    if (offsetDays > 0 && !canStepOverviewDateForward) {
      return;
    }

    normalizeSelectedDateRange();
    let nextStart = shiftIsoDate(selectedDateFrom, offsetDays);
    let nextEnd = shiftIsoDate(selectedDateTo, offsetDays);

    if (nextEnd > today) {
      const overshootDays = diffIsoDateDays(nextEnd, today);
      nextStart = shiftIsoDate(nextStart, -overshootDays);
      nextEnd = today;
    }

    selectedDateFrom = nextStart;
    selectedDateTo = nextEnd;
    handleOverviewDateChange();
  }

  function stepEvidenceOverviewDay(offsetDays: number) {
    const today = getLocalDateString();
    const anchorDate = overviewMode === 'date' ? selectedDateTo : today;
    const nextDate = shiftIsoDate(anchorDate, offsetDays);

    if (nextDate > today) return;

    overviewMode = 'date';
    selectedDateFrom = nextDate;
    selectedDateTo = nextDate;
    handleOverviewDateChange();
  }

  function returnEvidenceOverviewToday() {
    if (overviewMode === 'today') {
      loadStats(true);
      return;
    }

    setOverviewMode('today');
  }

  async function saveDomainSemanticRule(domain: DomainItem) {
    const nextCategory = editingSemanticCategory.trim();
    if (!domain) return;
    const domainKey = domain.domain;
    if (!domainKey || !nextCategory || isDomainSemanticSavePending(domainKey)) return;

    const editSessionId = domainSemanticEditSessionId;
    if ((domain.semantic_category?.trim() || '') === nextCategory) {
      cancelDomainSemanticEdit();
      return;
    }

    const confirmed = await confirm({
      title: t('overview.changeDomainCategoryTitle'),
      message: t('overview.changeDomainCategoryMessage', {
        domain: domainKey,
        category: semanticCategoryStore.getSemanticCategoryDisplayName(nextCategory),
      }),
      confirmText: t('overview.confirmChange'),
      cancelText: t('overview.cancel'),
      tone: 'warning',
    });
    if (
      !confirmed
      || !isCurrentDomainSemanticEdit(domainKey, editSessionId)
      || isDomainSemanticSavePending(domainKey)
    ) return;

    const requestId = ++nextDomainSemanticRequestId;
    setDomainSemanticSavePending(domainKey, requestId);
    const isCurrent = () => isCurrentDomainSemanticSave(domainKey, requestId, editSessionId);

    try {
      const updatedCount = await invoke<number>('set_domain_semantic_rule', {
        domain: domainKey,
        semanticCategory: nextCategory,
        syncHistory: true,
      });

      const refreshed = await refreshCurrentDomainDetail(domainKey, isCurrent);
      if (!refreshed) return;
      cancelDomainSemanticEdit();
      showToast(
        t('overview.domainSemanticUpdated', {
          domain: domainKey,
          category: semanticCategoryStore.getSemanticCategoryDisplayName(nextCategory),
          count: updatedCount,
        }),
        'success'
      );
    } catch (e) {
      if (!isCurrent()) return;
      console.error('修改网站语义分类失败:', e);
      showToast(
        t('overview.domainSemanticUpdateFailed', {
          domain: domainKey,
          error: e,
        }),
        'error'
      );
    } finally {
      clearDomainSemanticSavePending(domainKey, requestId);
    }
  }

  async function refreshOverviewStats(
    { silent = false }: OverviewRefreshOptions = {},
  ): Promise<void> {
    const params = {
      mode: overviewMode,
      dateFrom: overviewMode === 'date' ? selectedDateFrom : undefined,
      dateTo: overviewMode === 'date' ? selectedDateTo : undefined,
    };
    const paramsKey = `${params.mode}|${params.dateFrom || ''}|${params.dateTo || ''}`;
    // 仅当在途请求与当前模式/日期完全一致时才复用，避免切换模式后拿到旧模式数据
    if (overviewRefreshPromise && overviewRefreshKey === paramsKey) {
      return overviewRefreshPromise;
    }

    const requestId = ++overviewRequestId;
    overviewRefreshKey = paramsKey;
    overviewRefreshPromise = invoke<DailyStats>('get_overview_stats', params)
      .then((newStats) => {
        if (requestId !== overviewRequestId) {
          return;
        }
        stats = newStats;
        if (shouldUseOverviewCache()) {
          cache.setOverview(newStats);
        }
        error = null;
      })
      .catch((e) => {
        if (requestId !== overviewRequestId) {
          return;
        }
        if (silent) {
          console.warn('后台刷新失败:', e);
          return;
        }
        error = formatUserError(e, t('common.loadFailedRetry'));
      })
      .finally(() => {
        if (requestId === overviewRequestId) {
          overviewRefreshPromise = null;
          loading = false;
        }
      });

    return overviewRefreshPromise;
  }

  async function loadStats(forceRefresh = false): Promise<void> {
    // today 模式并行补齐「上周同日」基线（有同日期基线时为空操作）
    ensureLastWeekBaseline();
    if (!shouldUseOverviewCache()) {
      stats = null;
      loading = true;
      error = null;
      await refreshOverviewStats();
      return;
    }

    // 乐观更新策略：先显示缓存数据，后台刷新后再更新
    const cacheData = get(cache);
    
    // 如果有缓存数据，立即显示（不显示 loading）
    if (isDailyStats(cacheData.overview.data)) {
      stats = cacheData.overview.data;
      loading = false;
      
      // 如果缓存有效且非强制刷新，直接返回
      if (!forceRefresh && cache.isValid(cacheData, 'overview')) {
        return;
      }

      await refreshOverviewStats({ silent: true });
    } else {
      // 首次加载，显示 loading
      loading = true;
      error = null;
      await refreshOverviewStats();
    }
  }

  onMount(async () => {
    semanticCategoryStore.refresh();
    appUsageViewMode = readStoredOverviewViewMode(APP_USAGE_VIEW_MODE_KEY, 'row');
    overviewViewModeReady = true;
    try { categoryList = await invoke<CategoryInfo[]>('get_categories'); } catch (e) { categoryList = []; }
    try {
      const cfg = await invoke<WorkGoalConfig>('get_config');
      workGoalMinutes = cfg.daily_work_goal_minutes ?? null;
    } catch (e) {
      workGoalMinutes = null;
    }
    loadHourlyBreakdown();
    loadStats();
    if (!document.hidden) {
      clockInterval = setInterval(() => {
        // 界面只展示到分钟：仅分钟变化时才更新状态，避免每秒触发整页响应式重算
        const now = new Date();
        if (now.getMinutes() !== currentTime.getMinutes() || now.getHours() !== currentTime.getHours()) {
          currentTime = now;
        }
        if (!shouldAutoRefreshOverview()) {
          return;
        }
        // 跨天检测
        const newDate = now.getDate();
        if (newDate !== lastCheckDate) {
          lastCheckDate = newDate;
          loadStats(true);
        }
      }, 1000);
      refreshInterval = setInterval(() => {
        if (shouldAutoRefreshOverview()) {
          loadStats();
        }
      }, 30000);
    }

    handleVisibilityChange = () => {
      if (document.hidden) {
        if (clockInterval) clearInterval(clockInterval);
        if (refreshInterval) clearInterval(refreshInterval);
        clockInterval = null;
        refreshInterval = null;
      } else {
        currentTime = new Date();
        lastCheckDate = currentTime.getDate();
        clockInterval = setInterval(() => {
          const now = new Date();
          if (now.getMinutes() !== currentTime.getMinutes() || now.getHours() !== currentTime.getHours()) {
            currentTime = now;
          }
          if (!shouldAutoRefreshOverview()) {
            return;
          }
          const newDate = now.getDate();
          if (newDate !== lastCheckDate) {
            lastCheckDate = newDate;
            loadStats(true);
          }
        }, 1000);
        refreshInterval = setInterval(() => {
          if (shouldAutoRefreshOverview()) {
            loadStats();
          }
        }, 30000);
        loadStats(true);
      }
    };
    document.addEventListener('visibilitychange', handleVisibilityChange);
    
    // 监听 Tauri 截屏事件（后备）
    const un = await safeListen('screenshot-taken', () => {
      if (!document.hidden && shouldAutoRefreshOverview()) {
        loadStats(true);
      }
    });
    // 组件可能在 await 期间已销毁，避免监听器泄漏
    if (componentDestroyed) {
      if (un) un();
    } else {
      unlisten = un;
    }
    
    // 监听全局 activity-added 事件（实时同步）
    handleActivityAdded = () => {
      if (!document.hidden && shouldAutoRefreshOverview()) {
        loadStats(true);
      }
    };
    window.addEventListener('activity-added', handleActivityAdded);
  });

  $: if (overviewViewModeReady) {
    persistOverviewViewMode(APP_USAGE_VIEW_MODE_KEY, appUsageViewMode);
  }

  onDestroy(() => {
    componentDestroyed = true;
    hourlyBreakdownRequestId += 1;
    if (unlisten) unlisten();
    if (clockInterval) clearInterval(clockInterval);
    if (refreshInterval) clearInterval(refreshInterval);
    if (handleActivityAdded) window.removeEventListener('activity-added', handleActivityAdded);
    if (handleVisibilityChange) document.removeEventListener('visibilitychange', handleVisibilityChange);
  });
</script>

<svelte:window
  on:resize={handleSemanticPopoverViewportChange}
  on:keydown={(e) => {
    if (e.key !== 'Escape') return;
    // 弹窗 Escape 统一在 window 层处理（遮罩不再依赖自身聚焦才能响应）
    if (pendingDeleteSemanticCategory) {
      cancelDeleteSemanticCategory();
    } else if (editingDomainKey) {
      cancelDomainSemanticEdit();
    } else if (domainOverlayOpen) {
      closeDomainOverlay();
    }
  }}
/>

<div class="page-shell evidence-overview-page" data-locale={currentLocale}>
  {#if $uiTemplate === 'evidence-star-map'}
    <EvidenceOverviewHeader
      dateLabel={overviewSubtitle}
      totalDuration={stats ? formatDurationLocalized(stats.total_duration, { compact: true }) : '--'}
      evidenceCount={stats?.screenshot_count ?? 0}
      isRecording={overviewDotActive}
      canGoNext={overviewMode === 'date' && canStepOverviewDateForward}
      on:previous={() => stepEvidenceOverviewDay(-1)}
      on:today={returnEvidenceOverviewToday}
      on:next={() => stepEvidenceOverviewDay(1)}
    />
  {/if}

  <!-- 页面标题 -->
  <div class="page-header overview-support-toolbar">
    <div class="page-title-group">
      <div class="page-title-badge">
        <LayoutDashboard strokeWidth={1.8} aria-hidden="true" />
      </div>
      <div class="page-title-copy">
        <h2>{t('overview.title')}</h2>
        <p>
        {overviewSubtitle}
        {#if overviewMode === 'today'}
          <span class="ml-1.5 font-mono text-xs">{formatLocalizedTime(currentTime, { hour: '2-digit', minute: '2-digit' })}</span>
        {/if}
        <!-- #131 录制状态点：改版后状态胶囊并入日期行，仍随录制状态灰/绿 -->
        <span
          class="ms-1.5 inline-block h-1.5 w-1.5 rounded-full align-middle {overviewDotActive ? 'bg-emerald-500 animate-pulse' : 'bg-slate-400 dark:bg-[#484f58]'}"
          title={overviewStatusLabel}
        ></span>
        </p>
      </div>
    </div>
    <!-- 改版：原 overview-lead-card 模式切换整卡删除，分段切换（今天/本周/自定义）并入页头右侧一行 -->
    <div class="overview-command-deck">
      <button
        type="button"
        class="page-control-btn {overviewMode === 'today' ? 'page-control-btn-active' : ''}"
        on:click={() => setOverviewMode('today')}
      >
        {t('overview.modeToday')}
      </button>
      <button
        type="button"
        class="page-control-btn {overviewMode === 'week' ? 'page-control-btn-active' : ''}"
        on:click={() => setOverviewMode('week')}
      >
        {t('overview.modeWeek')}
      </button>
      <button
        type="button"
        class="page-control-btn {overviewMode === 'date' ? 'page-control-btn-active' : ''}"
        on:click={() => setOverviewMode('date')}
      >
        {t('overview.modeDate')}
      </button>

      {#if overviewMode === 'date'}
        <div class="overview-date-bar">
          <button
            type="button"
            class="page-control-btn-icon"
            title={t('common.previous')}
            on:click={() => stepOverviewDateRange(-1)}
          >
            <ChevronLeft class="h-4 w-4 text-slate-500" strokeWidth={1.8} aria-hidden="true" />
          </button>

          <LocalizedDatePicker
            mode="range"
            bind:startDate={selectedDateFrom}
            bind:endDate={selectedDateTo}
            localeCode={currentLocale}
            max={getLocalDateString()}
            triggerClass="overview-date-trigger"
            on:change={handleOverviewDateChange}
          />

          <button
            type="button"
            class="page-control-btn-icon"
            title={t('common.next')}
            disabled={!canStepOverviewDateForward}
            on:click={() => stepOverviewDateRange(1)}
          >
            <ChevronRight class="h-4 w-4 text-slate-500" strokeWidth={1.8} aria-hidden="true" />
          </button>
        </div>
      {/if}
    </div>
  </div>

  <div class="overview-editorial-shell">
  <!-- 洞察条：仅 today 模式、数据非空且上周同日基线可用时组句显示 -->
  {#if overviewMode === 'today' && insightSentence}
    <!-- 窄屏精修：flex-wrap 允许洞察句换行,链接自动下移到第二行,避免最小窗口横向溢出 -->
    <div class="mb-4 flex flex-wrap items-center gap-3.5 rounded-lg border border-blue-100 bg-gradient-to-r from-blue-50 via-white to-white px-5 py-3.5 dark:border-blue-900/40 dark:from-blue-950/35 dark:via-[#161b22] dark:to-[#161b22]">
      <span class="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-blue-100 text-blue-500 dark:bg-blue-900/40 dark:text-blue-300">
        <Sparkles class="h-[17px] w-[17px]" aria-hidden="true" />
      </span>
      <p class="min-w-0 flex-1 basis-52 text-sm text-slate-600 dark:text-[#adbac7]">{insightSentence}</p>
      <a
        href="#/report"
        class="shrink-0 whitespace-nowrap text-[13px] font-semibold text-primary-600 transition-colors hover:text-primary-700 dark:text-primary-400 dark:hover:text-primary-300"
      >
        {t('overview.insightWeekLink')}
      </a>
    </div>
  {/if}

  <div class="overview-summary-grid grid grid-cols-2 lg:grid-cols-4 gap-3 mb-4">
    {#if loading || !stats}
      {#each [1,2,3,4] as _}
        <div class="min-h-[116px] rounded-lg border border-slate-100 bg-white p-5 animate-pulse dark:border-[#30363d]/60 dark:bg-[#21262d]/80">
          <div class="flex h-full items-center justify-between gap-4">
            <div class="flex-1">
              <div class="h-3 rounded bg-slate-200 dark:bg-[#30363d] w-20"></div>
              <div class="mt-6 h-8 w-1/2 rounded bg-slate-200 dark:bg-[#30363d]"></div>
            </div>
            <div class="h-11 w-11 rounded-lg bg-slate-100 dark:bg-[#30363d] shrink-0"></div>
          </div>
        </div>
      {/each}
    {:else}
      <!-- 改版 KPI：总投入 / 工作时长 / 专注峰值 / 娱乐占比（原浏览器时长、应用数两卡移除） -->
      <StatsCard
        title={overviewTotalActivityTitle}
        value={formatDurationLocalized(stats.total_duration, { compact: true })}
        icon="duration"
        color="indigo"
        subtitle={totalDeltaSubtitle}
      />
      <StatsCard
        title={overviewWorkDurationTitle}
        value={formatDurationLocalized(stats.work_time_duration || 0, { compact: true })}
        icon="focus"
        color="emerald"
        subtitle={workShareSubtitle}
      />
      <StatsCard
        title={t('overview.peakFocus')}
        value={peakWindowValue}
        icon="duration"
        color="blue"
        subtitle={peakWindowSubtitle}
      />
      <StatsCard
        title={t('overview.entertainmentShare')}
        value={entertainmentShareValueText}
        icon="apps"
        color="rose"
        subtitle={entertainmentDeltaSubtitle}
      />
    {/if}
  </div>

  {#if error}
    <div class="page-banner-error mb-4">
      <div>
        <p class="font-semibold">{t('overview.loadError')}</p>
        <p class="text-sm mt-1">{error}</p>
      </div>
      <button class="page-action-brand" on:click={() => loadStats()}>{t('overview.retry')}</button>
    </div>
  {/if}

  <!-- week/date 模式：「按天投入」卡（置于节奏卡上方；today 模式不显示） -->
  {#if overviewMode !== 'today'}
    <div class="page-card overview-panel overview-panel-subtle mb-4">
      <div class="mb-3 flex items-baseline justify-between gap-3">
        <h3 class="page-section-title !mb-0">{t('overview.dailyInvest')}</h3>
        {#if heaviestDailyEntry}
          <span class="text-xs text-slate-400 dark:text-[#636c76]">
            {t('overview.heaviestDay', {
              day: formatDailyBarDayLabel(heaviestDailyEntry.date, rangeDailyTotals.length),
              dur: formatDurationLocalized(heaviestDailyEntry.total_duration, { compact: true }),
            })}
          </span>
        {/if}
      </div>
      {#if rangeDailyLoading && rangeDailyTotals.length === 0}
        <div class="flex h-[150px] items-end gap-2 px-1 animate-pulse">
          {#each [1, 2, 3, 4, 5, 6, 7] as pulseIndex}
            <div
              class="flex-1 rounded-t-lg bg-slate-200 dark:bg-[#30363d]"
              style={`height: ${24 + (pulseIndex % 4) * 22}px;`}
            ></div>
          {/each}
        </div>
      {:else if dailyBars.length > 0}
        <div class="flex items-end gap-2 px-1">
          {#each dailyBars as bar (bar.date)}
            <div class="flex min-w-0 flex-1 flex-col items-center justify-end gap-1.5">
              {#if bar.isHeaviest && bar.total > 0}
                <span class="text-[11px] font-semibold text-slate-600 dark:text-[#adbac7]">
                  {formatDurationLocalized(bar.total, { compact: true })}
                </span>
              {/if}
              <span
                class="block w-full max-w-[44px] rounded-t-md {bar.isToday || bar.isHeaviest ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#30363d]'}"
                style={`height: ${bar.heightPx}px;`}
              ></span>
              <span class="max-w-full truncate text-[11px] {bar.isToday ? 'font-bold text-primary-600 dark:text-primary-400' : 'text-slate-400 dark:text-[#636c76]'}">
                {bar.label}
              </span>
            </div>
          {/each}
        </div>
      {:else}
        <p class="py-6 text-center text-xs text-slate-400 dark:text-[#636c76]">{t('common.noRecords')}</p>
      {/if}
    </div>
  {/if}

  <!-- 主视觉：节奏卡 = 分类构成条 + 既有按小时活跃度图 -->
  <div class="page-card overview-panel overview-panel-featured mb-4">
    <div class="mb-3 flex items-center justify-between gap-3">
      <h3 class="page-section-title !mb-0">{rhythmCardTitle}</h3>
      <span class="hidden text-xs text-slate-400 dark:text-[#636c76] sm:inline">{t('overview.rhythmHint')}</span>
    </div>
    {#if loading || !stats}
      <div class="animate-pulse">
        <div class="mb-5 h-3.5 rounded-full bg-slate-200 dark:bg-[#30363d]"></div>
        <div class="mb-4 grid grid-cols-2 gap-3 lg:grid-cols-4">
          {#each [1,2,3,4] as _}
            <div class="min-h-[88px] rounded-lg bg-slate-50/88 p-4 dark:bg-[#161b22]/30">
              <div class="h-3 w-16 rounded bg-slate-200 dark:bg-[#30363d]"></div>
              <div class="mt-4 h-7 w-20 rounded bg-slate-200 dark:bg-[#30363d]"></div>
            </div>
          {/each}
        </div>
        <div class="rounded-lg bg-slate-50/90 p-4 dark:bg-[#161b22]/40">
          <div class="flex h-40 items-end gap-1.5">
            {#each Array(24) as _, hour}
              <div class="flex h-full flex-1 flex-col items-center justify-end">
                <div
                  class="w-full rounded-t-lg bg-slate-200 dark:bg-[#30363d]"
                  style={`height: ${Math.max(((hour % 6) + 2) * 12, 18)}%; opacity: 0.8;`}
                ></div>
                <div class="mt-2 h-2 w-7 rounded bg-slate-100 dark:bg-[#30363d]/60"></div>
              </div>
            {/each}
          </div>
        </div>
      </div>
    {:else}
      {#if compositionSegments.length > 0}
        <div class="mb-5">
          <div class="flex h-3.5 w-full gap-[2px]" role="group" aria-label={t('overview.compositionFilter')}>
            {#each compositionSegments as segment (segment.category)}
              <button
                type="button"
                class={`overview-composition-segment block h-full first:rounded-s-full last:rounded-e-full transition-[opacity,transform] focus:outline-none focus:ring-2 focus:ring-sky-300 ${selectedCompositionCategory && selectedCompositionCategory !== segment.category ? 'opacity-30' : 'opacity-100'} ${selectedCompositionCategory === segment.category ? 'scale-y-125' : ''}`}
                style={`width: ${segment.widthPct.toFixed(1)}%; min-width: 5px; background: ${segment.color};`}
                aria-label={`${segment.name} · ${formatDurationLocalized(segment.duration, { compact: true })} · ${segment.percent}%`}
                aria-pressed={selectedCompositionCategory === segment.category}
                on:click={() => toggleCompositionCategory(segment.category)}
              ></button>
            {/each}
          </div>
          <div class="mt-2.5 flex flex-wrap justify-center gap-x-2 gap-y-1.5">
            {#each compositionSegments as segment (segment.category)}
              <button
                type="button"
                class={`inline-flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs transition-colors ${selectedCompositionCategory === segment.category ? 'bg-slate-100 text-slate-800 dark:bg-[#30363d] dark:text-[#e6edf3]' : 'text-slate-500 hover:bg-slate-50 dark:text-[#7d8590] dark:hover:bg-[#21262d]'}`}
                aria-pressed={selectedCompositionCategory === segment.category}
                on:click={() => toggleCompositionCategory(segment.category)}
              >
                <span class="inline-block h-2 w-2 rounded-[var(--radius-xs)]" style={`background: ${segment.color};`}></span>
                <span class="font-medium">{segment.name}</span>
                {formatDurationLocalized(segment.duration, { compact: true })}
                <span class="text-slate-400 dark:text-[#636c76]">{segment.percent}%</span>
              </button>
            {/each}
          </div>

          {#if selectedCompositionSummary}
            <div class="overview-composition-summary mx-auto mt-4 max-w-3xl rounded-lg bg-slate-50/90 px-4 py-3 text-center dark:bg-[#161b22]/35">
              <div class="grid gap-2 sm:grid-cols-3">
                <div class="overview-composition-kpi rounded-xl px-3 py-2 text-center">
                  <p class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('overview.compositionDuration')}</p>
                  <p class="mt-1 text-sm font-semibold text-slate-800 dark:text-[#e6edf3]">{formatDurationLocalized(selectedCompositionSummary.duration, { compact: true })}</p>
                </div>
                <div class="overview-composition-kpi rounded-xl px-3 py-2 text-center">
                  <p class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('overview.compositionShare')}</p>
                  <p class="mt-1 text-sm font-semibold text-slate-800 dark:text-[#e6edf3]">{selectedCompositionSummary.percentage}%</p>
                </div>
                <div class="overview-composition-kpi rounded-xl px-3 py-2 text-center">
                  <p class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('overview.compositionActiveRange')}</p>
                  <p class="mt-1 text-sm font-semibold text-slate-800 dark:text-[#e6edf3]">{formatCompositionActiveRange(selectedCompositionSummary.activeRange)}</p>
                </div>
              </div>
              {#if selectedCompositionSummary.primaryApps.length > 0}
                <div class="overview-composition-primary-apps mt-2 flex flex-wrap items-center justify-center gap-2 text-xs text-slate-500 dark:text-[#7d8590]">
                  <span>{t('overview.compositionPrimaryApps')}</span>
                  {#each selectedCompositionSummary.primaryApps as app (app.appName)}
                    <span class="rounded-full bg-white px-2.5 py-1 dark:bg-[#21262d]">
                      {app.appName} · {formatDurationLocalized(app.duration, { compact: true })}
                    </span>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
      <ActivityHourlyChart
        embedded
        data={stats.hourly_activity_distribution}
        peakHourLabel={hourlyChartPeakHourLabel}
        peakDurationLabel={hourlyChartPeakDurationLabel}
        distributionTitle={hourlyChartDistributionTitle}
        distributionSubtitleKey={hourlyChartDistributionSubtitleKey}
        selectedCategory={selectedCompositionCategory}
        categoryBreakdown={hourlyCategoryBreakdown}
        categoryColors={hourlyCategoryColors}
        categoryNames={hourlyCategoryNames}
        appBreakdown={hourlyAppBreakdown}
        workDuration={stats?.work_time_duration || 0}
        workGoalMinutes={workGoalMinutes}
      />
    {/if}
  </div>

  <div class="overview-section-grid">
    <!-- 常驻网站：domain_usage 前 6，按域名聚合；点击行打开既有浏览器详情弹窗 -->
    <section class="page-card overview-section-card overview-panel overview-panel-subtle">
      <div class="mb-3 flex items-baseline justify-between gap-3">
        <h3 class="page-section-title !mb-0">{t('overview.topDomains')}</h3>
        <span class="text-xs text-slate-500 dark:text-[#7d8590]">{t('overview.byDomainAggregated')}</span>
      </div>
      {#if loading || !stats}
        <div class="overview-domain-skeleton-list overview-browser-gallery animate-pulse space-y-1">
          {#each [1, 2, 3, 4, 5, 6] as _}
            <div class="overview-domain-row overview-domain-skeleton-row grid w-full grid-cols-[minmax(0,11rem)_minmax(7rem,1fr)_auto] items-center gap-3 px-2 py-2.5">
              <div class="overview-domain-heading overview-domain-skeleton-heading min-w-0 space-y-1.5">
                <div class="h-3 w-28 max-w-full rounded bg-slate-200 dark:bg-[#30363d]"></div>
                <div class="h-2 w-20 max-w-full rounded bg-slate-100 dark:bg-[#30363d]/50"></div>
              </div>
              <div class="overview-domain-skeleton-source min-w-0">
                <div class="overview-domain-skeleton-source-label h-2 w-24 max-w-full rounded bg-slate-200 dark:bg-[#30363d]"></div>
                <div class="overview-domain-source-track overview-domain-skeleton-source-track mt-1.5 h-2 w-full rounded-full !bg-slate-100 dark:!bg-[#30363d]/50"></div>
              </div>
              <div class="overview-domain-skeleton-duration h-3 w-12 justify-self-end rounded bg-slate-100 dark:bg-[#30363d]/50"></div>
            </div>
          {/each}
        </div>
      {:else if topDomainPresentations.length > 0}
        <div class="overview-browser-gallery flex flex-col gap-1">
          {#each topDomainPresentations as domain (domain.domain)}
            <button
              type="button"
              class="overview-domain-row grid w-full grid-cols-[minmax(0,11rem)_minmax(7rem,1fr)_auto] items-center gap-3 rounded-lg !bg-transparent px-2 py-2.5 text-start transition-colors hover:!bg-slate-100/70 focus:outline-none focus-visible:!bg-slate-100/70 dark:hover:!bg-[#21262d]/70 dark:focus-visible:!bg-[#21262d]/70"
              on:click={() => openDomainDetail(domain)}
            >
              <span class="overview-domain-heading min-w-0">
                <span class="overview-domain-name block truncate">
                  {getBrowserDomainDisplayLabel(domain)}
                </span>
                <span class="overview-domain-meta overview-domain-category-meta mt-0.5 flex items-center gap-1.5 truncate text-[11px] text-slate-500 dark:text-[#7d8590]">
                  <span>{t('overview.sitePagesMeta', { count: domain.presentation.pageCount })}</span>
                  <span aria-hidden="true">·</span>
                  <span class="overview-semantic-color-dot h-1.5 w-1.5 shrink-0 rounded-full" style={`background-color: ${getSemanticCategoryColor(domain.semantic_category)};`}></span>
                  <span class="truncate">{getDomainSemanticLabel(domain)}</span>
                </span>
              </span>
              <span class="min-w-0">
                <span class="overview-domain-source-list block truncate text-[11px] text-slate-500 dark:text-[#7d8590]">
                  {domain.presentation.sourceLabel || t('overview.domainSourcesUnknown')}
                </span>
                <span class="overview-domain-source-track mt-1.5 flex h-2 overflow-hidden rounded-full !bg-slate-100 dark:!bg-[#30363d]/50">
                  {#each domain.presentation.sourceTrack as source, sourceIndex (source.browser_name)}
                    <span
                      class="overview-domain-source-segment block h-full"
                      style={`width: ${source.widthPct}%; background: hsl(${205 + sourceIndex * 38} 62% 58%);`}
                      title={`${source.browser_name} · ${Math.round(source.percentage)}%`}
                    ></span>
                  {/each}
                </span>
              </span>
              <span class="overview-domain-duration min-w-[4.5rem] whitespace-nowrap text-end text-xs font-semibold tabular-nums text-slate-600 dark:text-[#adbac7]">
                {formatDurationLocalized(domain.duration, { compact: true })}
              </span>
            </button>
          {/each}
        </div>
        <p class="mt-3 text-center text-xs text-slate-500 dark:text-[#7d8590]">
          {t('overview.domainsFooter', { count: stats.domain_total_count || domainUsageItems.length, browsers: domainBrowsersLabel })}
          {#if (stats.domain_total_count || domainUsageItems.length) > 6}
            ·
            <button
              type="button"
              class="font-semibold text-primary-600 transition-colors hover:text-primary-700 disabled:cursor-wait disabled:opacity-60 dark:text-primary-400 dark:hover:text-primary-300"
              disabled={domainUsageLoading}
              on:click={toggleDomainUsageExpanded}
            >
              {domainUsageLoading
                ? t('common.loading')
                : domainUsageExpanded
                  ? t('common.collapse')
                  : t('overview.viewAll')}
            </button>
          {/if}
        </p>
      {:else}
        <div class="empty-state-compact">
          <div class="empty-state-icon !w-12 !h-12 !mb-3 shadow-none">
            <Globe2 class="h-5 w-5" strokeWidth={1.7} aria-hidden="true" />
          </div>
          <p class="empty-state-copy">{overviewNoWebsiteVisitsText}</p>
        </div>
      {/if}
    </section>

    <section class="page-card overview-section-card overview-panel overview-panel-subtle">
      <div class="mb-3 flex items-center justify-between gap-3">
        <h3 class="page-section-title !mb-0">{t('overview.appUsage')}</h3>
        <button
          type="button"
          class="page-control-btn-icon"
          title={appUsageViewModeLabel}
          on:click={() => {
            appUsageViewMode = appUsageViewMode === 'row' ? 'column' : 'row';
          }}
        >
          {#if appUsageViewMode === 'row'}
            <ChartNoAxesGantt class="h-4 w-4 text-slate-500" strokeWidth={1.8} aria-hidden="true" />
          {:else}
            <ChartNoAxesColumn class="h-4 w-4 text-slate-500" strokeWidth={1.8} aria-hidden="true" />
          {/if}
        </button>
      </div>
      {#if loading || !stats}
        <div class="app-usage-chart__rows animate-pulse">
          {#each [1, 2, 3, 4, 5, 6] as _}
            <div class="app-usage-chart__row">
              <div class="app-usage-chart__heading gap-2.5">
                <div class="h-5 w-5 shrink-0 rounded bg-slate-200 dark:bg-[#30363d]"></div>
                <div class="min-w-0 flex-1 space-y-1.5">
                  <div class="h-3 w-24 max-w-full rounded bg-slate-200 dark:bg-[#30363d]"></div>
                  <div class="h-2 w-16 max-w-full rounded bg-slate-100 dark:bg-[#30363d]/50"></div>
                </div>
              </div>
              <div class="app-usage-chart__track !bg-slate-100 dark:!bg-[#30363d]/50"></div>
              <div class="app-usage-chart__duration h-3 w-12 justify-self-end rounded bg-slate-100 dark:bg-[#30363d]/50"></div>
            </div>
          {/each}
        </div>
      {:else if stats.app_usage.length > 0}
        <AppUsageChart data={stats.app_usage} mode={appUsageViewMode} embedded />
      {:else}
        <div class="empty-state-compact">
          <div class="empty-state-icon !w-12 !h-12 !mb-3 shadow-none">
            <ChartNoAxesColumn class="h-5 w-5" strokeWidth={1.7} aria-hidden="true" />
          </div>
          <p class="empty-state-copy">{overviewNoAppStatsText}</p>
        </div>
      {/if}
    </section>
  </div>
  </div>
</div>

<!-- 域名摘要 / 单域名详情浮层 -->
{#if domainOverlayOpen}
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="fixed inset-0 z-[140] flex items-center justify-center bg-slate-950/52 p-4 backdrop-blur-md animate-fadeIn"
  role="presentation"
  on:click|self={closeDomainOverlay}
>
  <div
    bind:this={domainOverlayDialog}
    use:trapFocus
    class="card overview-domain-dialog flex max-h-[85vh] w-full max-w-2xl flex-col overflow-hidden p-0"
    role="dialog"
    aria-modal="true"
    aria-labelledby="overview-domain-overlay-title"
    tabindex="-1"
  >
    <div class="flex items-center justify-between border-b border-slate-200 bg-gradient-to-r from-slate-50 to-white p-5 dark:border-[#30363d] dark:from-[#21262d] dark:to-[#161b22]">
      <div class="flex min-w-0 items-center gap-3">
        {#if domainOverlayView === 'detail' && domainCollection.length > 0}
          <button
            bind:this={domainOverlayBackButton}
            type="button"
            class="rounded-lg p-2 text-slate-500 transition-colors hover:bg-slate-100 dark:text-[#7d8590] dark:hover:bg-[#30363d]"
            title={t('overview.viewAll')}
            on:click={showAllDomainSummaries}
          >
            <ChevronLeft class="h-4 w-4" aria-hidden="true" />
          </button>
        {/if}
        <div class="min-w-0">
          {#if domainOverlayView === 'all'}
            <h3 id="overview-domain-overlay-title" class="truncate text-lg font-bold text-slate-900 dark:text-[#e6edf3]">{t('overview.domainListTitle')}</h3>
            <p class="truncate text-sm text-slate-500 dark:text-[#7d8590]">{t('overview.sitesCount', { count: domainCollectionTotalCount })}</p>
          {:else}
            <h3 id="overview-domain-overlay-title" class="truncate text-lg font-bold text-slate-900 dark:text-[#e6edf3]">
              {selectedDomainDetail ? getBrowserDomainDisplayLabel(selectedDomainDetail) : t('overview.domainDetailTitle')}
            </h3>
            {#if selectedDomainDetail}
              <p class="truncate text-sm text-slate-500 dark:text-[#7d8590]">
                {formatDuration(selectedDomainDetail.duration)} · {t('overview.pagesCount', { count: selectedDomainDetail.urls?.length || 0 })}
              </p>
            {/if}
          {/if}
        </div>
      </div>
      <button
        type="button"
        class="shrink-0 rounded-lg p-2 transition-colors hover:bg-slate-100 dark:hover:bg-[#30363d]"
        title={t('overview.cancel')}
        on:click={closeDomainOverlay}
      >
        <X class="h-5 w-5 text-slate-500" aria-hidden="true" />
      </button>
    </div>

    <div class="flex-1 space-y-4 overflow-y-auto p-5" on:scroll={handleSemanticPopoverViewportChange}>
      {#if domainOverlayLoading}
        <div class="py-10 text-center text-sm text-slate-400 dark:text-[#636c76]">{t('common.loading')}</div>
      {:else if domainOverlayError}
        <div class="mx-auto max-w-md rounded-lg bg-red-50 px-4 py-5 text-center text-sm text-red-600 dark:bg-red-950/20 dark:text-red-300">
          <p>{t('overview.domainLoadFailed')}</p>
          <p class="mt-1 break-words text-xs opacity-80">{domainOverlayError}</p>
        </div>
      {:else if domainOverlayView === 'all'}
        <div class="overview-domain-summary-list space-y-2">
          {#each domainCollection as domain (domain.domain)}
            {@const summaryPresentation = buildDomainPresentation(domain)}
            <button
              type="button"
              data-domain-summary
              class="overview-domain-summary-row grid w-full grid-cols-[minmax(0,1fr)_auto] gap-3 rounded-lg border border-slate-200/80 px-4 py-3 text-start transition-colors hover:border-sky-200 hover:bg-sky-50/45 focus:outline-none focus:ring-2 focus:ring-sky-300 dark:border-[#30363d] dark:hover:border-sky-800/70 dark:hover:bg-sky-950/10"
              on:click={() => selectDomainFromCollection(domain)}
            >
              <span class="min-w-0">
                <span class="overview-domain-heading block truncate text-sm font-semibold text-slate-800 dark:text-[#e6edf3]">{getBrowserDomainDisplayLabel(domain)}</span>
                <span class="overview-domain-meta mt-1 flex items-center gap-1.5 text-xs text-slate-400 dark:text-[#636c76]">
                  <span>{t('overview.sitePagesMeta', { count: summaryPresentation.pageCount })}</span>
                  <span aria-hidden="true">·</span>
                  <span class="h-1.5 w-1.5 rounded-full" style={`background-color: ${getSemanticCategoryColor(domain.semantic_category)};`}></span>
                  <span class="truncate">{getDomainSemanticLabel(domain)}</span>
                </span>
                <span class="overview-domain-source-list mt-2 block truncate text-[11px] text-slate-500 dark:text-[#7d8590]">
                  {summaryPresentation.sourceLabel || t('overview.domainSourcesUnknown')}
                </span>
                <span class="overview-domain-source-track mt-1.5 flex h-1.5 overflow-hidden rounded-full bg-slate-100 dark:bg-[#30363d]/70">
                  {#each summaryPresentation.sourceTrack as source, sourceIndex (source.browser_name)}
                    <span
                      class="overview-domain-source-segment block h-full"
                      style={`width: ${source.widthPct}%; background: hsl(${205 + sourceIndex * 38} 62% 58%);`}
                    ></span>
                  {/each}
                </span>
              </span>
              <span class="self-center text-xs font-semibold text-slate-600 dark:text-[#adbac7]">{formatDurationLocalized(domain.duration, { compact: true })}</span>
            </button>
          {/each}
          {#if domainCollection.length === 0}
            <div class="py-10 text-center text-sm text-slate-400 dark:text-[#636c76]">{t('common.noRecords')}</div>
          {/if}
        </div>
      {:else if domainOverlayView === 'detail'}
      {#each (selectedDomainDetail ? [selectedDomainDetail] : []) as domain}
        {@const domainPresentation = buildDomainPresentation(domain)}
        <div class="overview-domain-detail-source rounded-lg bg-slate-50/80 p-3 dark:bg-[#21262d]/45">
          <div class="overview-domain-source-list flex flex-wrap items-center justify-between gap-2 text-xs text-slate-500 dark:text-[#7d8590]">
            <span>{domainPresentation.sourceLabel || t('overview.domainSourcesUnknown')}</span>
            <span>{formatDurationLocalized(domain.duration, { compact: true })}</span>
          </div>
          <div class="overview-domain-source-track mt-2 flex h-2 overflow-hidden rounded-full bg-slate-100 dark:bg-[#30363d]/70">
            {#each domainPresentation.sourceTrack as source, sourceIndex (source.browser_name)}
              <span
                class="overview-domain-source-segment block h-full"
                style={`width: ${source.widthPct}%; background: hsl(${205 + sourceIndex * 38} 62% 58%);`}
                title={`${source.browser_name} · ${Math.round(source.percentage)}%`}
              ></span>
            {/each}
          </div>
        </div>
        <div class="overview-domain-detail relative rounded-lg border border-slate-200 dark:border-[#30363d]">
          <!-- 域名头部 -->
          <div class="flex items-center justify-between rounded-t-lg p-3 bg-slate-50 dark:bg-[#21262d]/50">
            <div class="flex items-center gap-2">
              <span class="w-2 h-2 rounded-full bg-primary-500"></span>
              <span class="font-medium text-slate-700 dark:text-[#c9d1d9]">{getBrowserDomainDisplayLabel(domain)}</span>
              <span class="text-xs text-slate-400 bg-slate-200 dark:bg-[#30363d] px-1.5 py-0.5 rounded">
                {t('overview.modalPages', { count: domain.urls.length })}
              </span>
            </div>
            <div class="flex items-center gap-2">
              {#if isUnresolvedBrowserDomain(domain)}
                <span class="text-xs px-2 py-1 rounded-full bg-amber-50 text-amber-700 dark:bg-amber-900/30 dark:text-amber-300">
                  {t('overview.unresolvedPage')}
                </span>
              {:else}
                <span class="flex items-center gap-1.5 rounded-full bg-primary-50 px-2 py-1 text-xs text-primary-700 dark:bg-primary-900/20 dark:text-primary-300">
                  <span
                    class="overview-semantic-color-dot h-1.5 w-1.5 shrink-0 rounded-full"
                    style={`background-color: ${getSemanticCategoryColor(domain.semantic_category)};`}
                  ></span>
                  {t('overview.currentCategory', { label: getDomainSemanticLabel(domain) })}
                </span>
                <button
                  class="text-xs px-2 py-1 rounded-lg border border-slate-200 dark:border-[#484f58] text-slate-700 dark:text-[#adbac7] hover:border-primary-300 hover:text-primary-600 transition-colors"
                  aria-haspopup="dialog"
                  aria-expanded={editingDomainKey === domain.domain}
                  aria-controls={`semantic-category-popover-${domain.domain}`}
                  use:registerDomainSemanticTrigger={domain.domain}
                  on:click={() => {
                    if (editingDomainKey === domain.domain) {
                      cancelDomainSemanticEdit();
                    } else {
                      startDomainSemanticEdit(domain);
                    }
                  }}
                >
                  {t('overview.changeCategory')}
                </button>
              {/if}
              <span class="text-sm font-medium text-slate-700 dark:text-[#adbac7]">{formatDuration(domain.duration)}</span>
            </div>
          </div>

          {#if !isUnresolvedBrowserDomain(domain) && editingDomainKey === domain.domain}
            <div
              bind:this={semanticCategoryPopover}
              id={`semantic-category-popover-${domain.domain}`}
                  class="overview-semantic-popover fixed z-[160] overflow-y-auto rounded-lg border border-slate-200 bg-white p-3 shadow-xl dark:border-[#30363d] dark:bg-[#161b22] dark:shadow-black/30"
              role="dialog"
              tabindex="-1"
              aria-label={t('overview.changeCategory')}
              style={semanticPopoverStyle}
            >
              <div class="flex items-start justify-between gap-3 border-b border-slate-100 pb-2.5 dark:border-[#30363d]">
                <div class="min-w-0">
                  <p class="text-sm font-semibold text-slate-800 dark:text-[#e6edf3]">{t('overview.selectCategory')}</p>
                  <p class="mt-0.5 truncate text-[11px] text-slate-400 dark:text-[#636c76]">{getBrowserDomainDisplayLabel(domain)}</p>
                </div>
                <button
                  type="button"
                  class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg text-slate-400 transition-colors hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-[#21262d] dark:hover:text-[#adbac7]"
                  on:click={() => cancelDomainSemanticEdit()}
                  aria-label={t('overview.cancel')}
                >
                  <X class="h-4 w-4" strokeWidth={2} aria-hidden="true" />
                </button>
              </div>

              <p class="py-2 text-[11px] leading-relaxed text-slate-400 dark:text-[#7d8590]">
                {t('overview.semanticCategoryHelp')}
              </p>

              <div class="space-y-1">
                {#each getSemanticCategoryOptions() as cat (cat.key)}
                  <div class="group flex items-center gap-1">
                    <button
                      type="button"
                      class="overview-semantic-option flex min-w-0 flex-1 items-center justify-between gap-3 rounded-xl px-2.5 py-2 text-start text-sm transition-colors
                        {editingSemanticCategory === cat.key
                          ? 'bg-primary-50 text-primary-700 dark:bg-primary-900/20 dark:text-primary-200'
                          : 'text-slate-700 hover:bg-slate-50 dark:text-[#adbac7] dark:hover:bg-[#21262d]'}"
                      aria-pressed={editingSemanticCategory === cat.key}
                      disabled={isDomainSemanticSavePending(domain.domain)}
                      on:click={() => editingSemanticCategory = cat.key}
                    >
                      <span class="flex min-w-0 items-center gap-2">
                        <span
                          class="overview-semantic-color-dot h-2 w-2 shrink-0 rounded-full"
                          style={`background-color: ${getSemanticCategoryColor(cat.key)};`}
                        ></span>
                        <span class="truncate">{getSemanticCategoryDisplayName(cat)}</span>
                      </span>
                      {#if editingSemanticCategory === cat.key}
                        <Check class="overview-semantic-check h-4 w-4 shrink-0" strokeWidth={2.5} aria-hidden="true" />
                      {/if}
                    </button>
                    {#if !cat.is_system}
                      <button
                        type="button"
                        on:click={() => startRenameSemanticCategory(cat)}
                        class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-xs text-slate-400 opacity-0 transition-all hover:bg-blue-50 hover:text-blue-600 focus-visible:opacity-100 group-hover:opacity-100 dark:hover:bg-blue-900/20 dark:hover:text-blue-300"
                        disabled={semanticCategorySaving}
                        title={t('overview.renameSemanticCategory')}
                        aria-label={t('overview.renameSemanticCategory')}
                      ><Pencil class="h-3.5 w-3.5" strokeWidth={2} aria-hidden="true" /></button>
                      <button
                        type="button"
                        on:click={() => pendingDeleteSemanticCategory = { key: cat.key, name: getSemanticCategoryDisplayName(cat) }}
                        class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-sm text-slate-400 opacity-0 transition-all hover:bg-red-50 hover:text-red-600 focus-visible:opacity-100 group-hover:opacity-100 dark:hover:bg-red-900/20 dark:hover:text-red-300"
                        disabled={semanticCategorySaving}
                        title={t('overview.deleteSemanticCategory')}
                        aria-label={t('overview.deleteSemanticCategory')}
                      ><X class="h-4 w-4" strokeWidth={2} aria-hidden="true" /></button>
                    {/if}
                  </div>
                {/each}
              </div>

              <button
                type="button"
                on:click={() => {
                  showCreateSemanticCategory = !showCreateSemanticCategory;
                  showRenameSemanticCategory = false;
                }}
                class="mt-2 flex w-full items-center justify-center gap-1.5 rounded-xl border border-dashed border-slate-200 px-3 py-2 text-xs font-medium text-slate-500 transition-colors hover:border-primary-300 hover:text-primary-600 dark:border-[#30363d] dark:text-[#7d8590] dark:hover:border-[#484f58] dark:hover:text-primary-300"
                disabled={semanticCategorySaving}
              >
                {#if showCreateSemanticCategory}
                  <X class="h-3.5 w-3.5" strokeWidth={2} aria-hidden="true" />
                {:else}
                  <Plus class="h-3.5 w-3.5" strokeWidth={2} aria-hidden="true" />
                {/if}
                <span>{t('overview.createSemanticCategory')}</span>
              </button>

              {#if showCreateSemanticCategory}
                <div class="mt-2 space-y-2 rounded-xl bg-slate-50 p-2.5 dark:bg-[#21262d]/70">
                  <p class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('overview.createSemanticCategoryHint')}</p>
                  <input
                    type="text"
                    bind:value={newSemanticCategoryName}
                    placeholder={t('overview.semanticCategoryNamePlaceholder')}
                    class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-2 text-sm dark:border-[#30363d] dark:bg-[#0d1117]"
                  />
                  <div class="flex justify-end gap-2">
                    <button
                      type="button"
                      on:click={() => showCreateSemanticCategory = false}
                      class="px-2.5 py-1.5 text-xs text-slate-500 dark:text-[#7d8590]"
                    >{t('overview.cancel')}</button>
                    <button
                      type="button"
                      on:click={createCustomSemanticCategory}
                      class="rounded-lg bg-primary-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-primary-700"
                    >{t('overview.confirmChange')}</button>
                  </div>
                </div>
              {/if}

              {#if showRenameSemanticCategory}
                <div class="mt-2 space-y-2 rounded-xl bg-blue-50/70 p-2.5 dark:bg-blue-900/15">
                  <p class="text-[11px] text-slate-500 dark:text-[#7d8590]">{t('overview.renameSemanticCategory')}</p>
                  <input
                    type="text"
                    bind:value={renameSemanticName}
                    placeholder={t('overview.semanticCategoryNamePlaceholder')}
                    class="w-full rounded-lg border border-blue-200 bg-white px-2.5 py-2 text-sm dark:border-blue-900/50 dark:bg-[#0d1117]"
                  />
                  <div class="flex justify-end gap-2">
                    <button
                      type="button"
                      on:click={() => showRenameSemanticCategory = false}
                      class="px-2.5 py-1.5 text-xs text-slate-500 dark:text-[#7d8590]"
                    >{t('overview.cancel')}</button>
                    <button
                      type="button"
                      on:click={saveRenameSemanticCategory}
                      class="rounded-lg bg-primary-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-primary-700"
                    >{t('overview.confirmChange')}</button>
                  </div>
                </div>
              {/if}

              <div class="mt-3 flex items-center justify-end gap-3 border-t border-slate-100 pt-3 dark:border-[#30363d]">
                <div class="flex shrink-0 items-center gap-2">
                  <button
                    type="button"
                    class="rounded-lg px-2.5 py-1.5 text-xs text-slate-500 transition-colors hover:bg-slate-100 dark:text-[#7d8590] dark:hover:bg-[#21262d]"
                    disabled={isDomainSemanticSavePending(domain.domain)}
                    on:click={() => cancelDomainSemanticEdit()}
                  >{t('overview.cancel')}</button>
                  <button
                    type="button"
                    class="rounded-lg bg-primary-600 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-primary-700 disabled:opacity-50"
                    disabled={!editingSemanticCategory.trim() || isDomainSemanticSavePending(domain.domain)}
                    on:click={() => saveDomainSemanticRule(domain)}
                  >
                    {isDomainSemanticSavePending(domain.domain) ? t('overview.saving') : t('overview.save')}
                  </button>
                </div>
              </div>
            </div>
          {/if}
          
          <!-- URL 列表，支持展开/收起超出的部分 -->
          <div class="overflow-hidden rounded-b-lg divide-y divide-slate-100 dark:divide-[#30363d]/50">
            {#each (expandedDomains.has(domain.domain) ? domain.urls : domain.urls.slice(0, 10)) as url}
              <div class="flex items-center justify-between p-3 hover:bg-slate-50 dark:hover:bg-[#21262d]/30 transition-colors">
                <div class="flex-1 min-w-0 mr-3">
                  <p
                    class="text-sm text-slate-700 dark:text-[#adbac7] truncate"
                    title={formatBrowserUrlForDisplay(url.url)}
                  >
                    {formatBrowserUrlForDisplay(url.url)}
                  </p>
                </div>
                <span class="text-xs text-slate-400 whitespace-nowrap">{formatDuration(url.duration)}</span>
              </div>
            {/each}
            {#if domain.urls.length > 10}
              <!-- 展开/收起按钮，让用户可以查看全部 URL -->
              <button
                class="w-full p-3 text-center text-xs text-primary-600 hover:text-primary-600 dark:text-primary-400 hover:bg-primary-50 dark:hover:bg-primary-900/10 transition-colors flex items-center justify-center gap-1"
                on:click={() => {
                  if (expandedDomains.has(domain.domain)) {
                    expandedDomains.delete(domain.domain);
                  } else {
                    expandedDomains.add(domain.domain);
                  }
                  expandedDomains = expandedDomains;
                }}
              >
                {#if expandedDomains.has(domain.domain)}
                  <ChevronUp class="w-3 h-3" aria-hidden="true" />
                  {t('common.collapse')}
                {:else}
                  <ChevronDown class="w-3 h-3" aria-hidden="true" />
                  {t('common.expandAll', { count: domain.urls.length })}
                {/if}
              </button>
            {/if}
          </div>
        </div>
      {/each}

      {/if}
    </div>
  </div>
</div>
{/if}

<!-- 语义分类删除确认弹窗 -->
{#if pendingDeleteSemanticCategory}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="fixed inset-0 z-[190] bg-slate-950/40 backdrop-blur-sm flex items-center justify-center animate-fadeIn"
    role="presentation"
    on:click|self={cancelDeleteSemanticCategory}
  >
        <div use:trapFocus role="dialog" aria-modal="true" class="w-full max-w-sm rounded-xl border border-slate-200 dark:border-[#30363d] bg-white dark:bg-[#161b22] shadow-2xl p-6 mx-4">
      <h3 class="text-base font-semibold text-slate-900 dark:text-[#e6edf3]">{t('overview.deleteSemanticCategoryTitle')}</h3>
      <p class="mt-2 text-sm text-slate-700 dark:text-[#7d8590] leading-relaxed">
        {t('overview.deleteSemanticCategoryMessage', { category: pendingDeleteSemanticCategory.name })}
      </p>
      <div class="mt-5 flex justify-end gap-2">
        <button
          on:click={cancelDeleteSemanticCategory}
          class="px-4 py-2 text-sm rounded-lg text-slate-500 hover:text-slate-700 dark:text-[#7d8590] dark:hover:text-[#c9d1d9] border border-slate-200 dark:border-[#30363d]"
        >
          {t('overview.cancel')}
        </button>
        <button
          on:click={confirmDeleteSemanticCategory}
          class="px-4 py-2 text-sm rounded-lg bg-red-600 text-white hover:bg-red-700"
        >
          {t('overview.confirmDeleteSemanticCategory')}
        </button>
      </div>
    </div>
  </div>
{/if}
