<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
  import { open } from '@tauri-apps/plugin-shell';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';
  import { get } from 'svelte/store';
  import { showToast } from '../../lib/stores/toast.ts';
  import { confirm } from '../../lib/stores/confirm.ts';
  import { cache, type CacheState } from '../../lib/stores/cache.ts';
  import { formatLocalizedDate, formatLocalizedTime, formatDurationLocalized, locale, t, tm, translateCategoryLabel } from '$lib/i18n/index.ts';
  import { formatUserError } from '$lib/utils/errorDisplay.ts';
  import { updateConfigQueued } from '$lib/utils/configSaveQueue.ts';
  import { shouldShowPromptAppliedToast } from './reportPromptFeedback.ts';
  import { resolveReportMeta, type ResolvedReportMeta } from './reportMeta.ts';
  import {
    extractReportBlockName,
    getVisibleReportSections,
    parseReportSections,
    reportSectionMarkdownForDisplay,
    reportSectionMarkdownForStorage,
    type ReportSection,
    type VisibleReportSection,
  } from './reportSections.ts';
  import { createReportGenerationOwnership, createReportRequestSnapshot, shiftIsoDate } from './reportDateNavigation.ts';
  import LocalizedDatePicker from '../../lib/components/LocalizedDatePicker.svelte';

  interface DailyReport {
    date: string;
    locale?: string;
    content: string;
    ai_mode?: string;
    model_name?: string | null;
    fallback_reason?: string | null;
    created_at: number;
  }

  interface PromptPreset {
    name: string;
    prompt: string;
  }

  interface ReportConfig {
    ai_mode: string;
    daily_report_custom_prompt: string;
    daily_report_prompt_presets: PromptPreset[];
    daily_report_export_dir: string | null;
    daily_report_pinned_blocks: string[];
    daily_report_hidden_blocks: string[];
    daily_report_system_prompt_override?: string | null;
    [key: string]: unknown;
  }

  interface CategoryUsage {
    category: string;
    duration: number;
  }

  interface HourlyActivityBucket {
    hour: number;
    duration: number;
  }

  interface DailyStats {
    total_duration: number;
    screenshot_count: number;
    app_usage: Array<{ app_name: string }>;
    category_usage: CategoryUsage[];
    hourly_activity_distribution: HourlyActivityBucket[];
  }

  interface CategoryDefinition {
    key: string;
    name: string;
    color: string;
  }

  interface ExportRangeResult {
    path: string;
    count: number;
  }

  type BatchPreset = 'thisWeek' | 'lastWeek' | 'thisMonth' | 'lastMonth';

  function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
  }

  function isDailyReport(value: unknown): value is DailyReport {
    return isRecord(value)
      && typeof value.date === 'string'
      && typeof value.content === 'string'
      && typeof value.created_at === 'number';
  }

  function isReportConfig(value: unknown): value is ReportConfig {
    return isRecord(value)
      && typeof value.ai_mode === 'string'
      && typeof value.daily_report_custom_prompt === 'string'
      && Array.isArray(value.daily_report_prompt_presets)
      && Array.isArray(value.daily_report_pinned_blocks)
      && Array.isArray(value.daily_report_hidden_blocks)
      && (value.daily_report_export_dir === null || typeof value.daily_report_export_dir === 'string');
  }

  function getLocalDateString() {
    const now = new Date();
    return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
  }

  function getYesterdayDateString() {
    return shiftIsoDate(getLocalDateString(), -1);
  }

  let report: DailyReport | null = null;
  let loading = false;
  let generating = false;
  let error: string | null = null;
  let selectedDate = getLocalDateString();
  let freshStats: DailyStats | null = null;
  let lastWeekStats: DailyStats | null = null; // 上周同日基线（KPI 参照系;加载失败保持 null,不显示 delta）
  let isYesterdayReport = false; // 标记是否显示的是昨日日报
  let showPresetModal = false;
  let presetSaving = false;
  $: activePresetName = (config?.daily_report_prompt_presets || []).find(p => p.prompt === config?.daily_report_custom_prompt)?.name || '';
  let editingPresetIndex = -1;
  let editingPresetName = '';
  let editingPresetPrompt = '';
  let config: ReportConfig | null = null;
  let lastLoadedDate = '';
  let reportRequestId = 0;
  const reportGenerationOwnership = createReportGenerationOwnership();
  let exportInProgress = false;
  let promptSaving = false;
  let pendingReportPrompt: string | null = null;
  let promptSaveTask: Promise<void> | null = null;
  let cacheData: CacheState | null = null;
  // ── 2026-07 日报改版：页头动作收敛为「导出 ▾」菜单 + 生成设置抽屉 ──
  let showExportMenu = false;
  let showGenerateDrawer = false;
  let categoryList: CategoryDefinition[] = []; // 分类色板（数据对照面板构成条着色,与概览同源）
  const unsubscribeCache = cache.subscribe(v => {
    cacheData = v;
    // 首次或缓存有值时，立即从缓存恢复配置（避免页面切换闪烁）
    if (!config && isReportConfig(v.config)) {
      config = v.config;
    }
  });
  onDestroy(unsubscribeCache);
  $: generating = cacheData?.reportGenerating ?? false;
  $: currentLocale = $locale;
  $: currentReportCacheKey = `${selectedDate}:${currentLocale}`;

  // 获取 AI 模式显示名称
  function getAiModeName(mode: unknown): string {
    const normalizedMode = (mode || '').toString().trim().toLowerCase();
    const modeNames: Record<string, string> = {
      'local': t('report.modeNames.local'),
      'summary': t('report.modeNames.summary'),
      'cloud': t('report.modeNames.cloud')
    };
    return modeNames[normalizedMode] || String(mode || '') || t('report.modeNames.unknown');
  }

  function getFallbackReasonText(meta: ResolvedReportMeta): string {
    return meta?.fallbackReason || t('report.savedReportNotAi');
  }

  async function loadConfig() {
    try {
      const cfg = await invoke<ReportConfig>('get_config');
      cache.setConfig(cfg);
    } catch (e) {
      console.error('加载配置失败:', e);
    }
  }

  async function loadReport(previousReport: DailyReport | null = null) {
    const { requestId, targetDate, targetLocale, targetCacheKey } = createReportRequestSnapshot(
      ++reportRequestId,
      selectedDate,
      currentLocale,
    );
    freshStats = null;
    lastWeekStats = null;

    // 并行加载实时统计 + 上周同日基线（KPI 参照系）
    invoke<DailyStats>('get_daily_stats', { date: targetDate })
      .then(stats => { if (requestId === reportRequestId) freshStats = stats; })
      .catch(() => {});
    invoke<DailyStats>('get_daily_stats', { date: shiftIsoDate(targetDate, -7) })
      .then(stats => { if (requestId === reportRequestId) lastWeekStats = stats; })
      .catch(() => {});

    // 乐观更新：先显示缓存数据
    const cachedState = get(cache);
    const cachedReport = cachedState.reports[targetCacheKey]?.data;

    if (isDailyReport(cachedReport)) {
      report = cachedReport;
      isYesterdayReport = false;
      loading = false;

      // 缓存有效则直接返回
      if (cache.isValid(cachedState.reports[targetCacheKey], 'reports')) {
        return;
      }

      // 后台静默刷新
      try {
        const savedReport = await invoke<DailyReport | null>('get_saved_report', { date: targetDate, locale: targetLocale });
        if (requestId !== reportRequestId) return;
        if (savedReport) {
          report = savedReport;
          cache.setReport(targetCacheKey, savedReport);
        }
      } catch (e) {
        console.warn('后台刷新日报失败:', e);
      }
    } else {
      // 首次加载
      loading = true;
      error = null;
      try {
        const savedReport = await invoke<DailyReport | null>('get_saved_report', { date: targetDate, locale: targetLocale });
        if (requestId !== reportRequestId) return;
        if (savedReport) {
          report = savedReport;
          isYesterdayReport = false;
          cache.setReport(targetCacheKey, savedReport);
        } else {
          if (
            !savedReport
            && previousReport?.date === targetDate
            && previousReport?.content
            && reportGenerationOwnership.claim(requestId, cacheData?.reportGenerating ?? false)
          ) {
            cache.setReportGenerating(true);
            await invoke('generate_report', { date: targetDate, force: false, locale: targetLocale });
            if (requestId !== reportRequestId) return;
            const localizedReport = await invoke<DailyReport | null>('get_saved_report', { date: targetDate, locale: targetLocale });
            if (requestId !== reportRequestId) return;

            if (localizedReport) {
              report = localizedReport;
              isYesterdayReport = false;
              cache.setReport(targetCacheKey, localizedReport);
              return;
            }
          }

          // 如果选择今天且今天无日报，尝试加载昨日日报
          if (targetDate === getLocalDateString()) {
            const yesterday = shiftIsoDate(targetDate, -1);
            const yesterdayReport = await invoke<DailyReport | null>('get_saved_report', { date: yesterday, locale: targetLocale });
            if (requestId !== reportRequestId) return;
            if (yesterdayReport) {
              report = yesterdayReport;
              isYesterdayReport = true;
            } else {
              report = null;
              isYesterdayReport = false;
            }
          } else {
             report = null;
             isYesterdayReport = false;
          }
        }
      } catch (e) {
        if (requestId === reportRequestId) {
          error = formatUserError(e, t('common.loadFailedRetry'));
        }
      } finally {
        if (requestId === reportRequestId) {
          loading = false;
        }
        if (reportGenerationOwnership.release(requestId)) {
          cache.setReportGenerating(false);
        }
      }
    }
  }

  function selectPreviousDay() {
    selectDate(shiftIsoDate(selectedDate, -1));
  }

  function selectDate(date: string) {
    if (!date || date === selectedDate) return;
    selectedDate = date;
  }

  async function generateReport(force = true) {
    const { requestId, targetDate, targetLocale, targetCacheKey } = createReportRequestSnapshot(
      ++reportRequestId,
      selectedDate,
      currentLocale,
    );
    if (!reportGenerationOwnership.claim(requestId, cacheData?.reportGenerating ?? false)) return;
    cache.setReportGenerating(true);
    error = null;
    try {
      if (config?.ai_mode === 'summary') {
        await persistReportPrompt();
        if (requestId !== reportRequestId) return;
      }
      await invoke('generate_report', { date: targetDate, force, locale: targetLocale });
      if (requestId !== reportRequestId) return;
      const savedReport = await invoke<DailyReport | null>('get_saved_report', { date: targetDate, locale: targetLocale });
      if (requestId !== reportRequestId) return;
      report = savedReport || { date: targetDate, content: '', created_at: Date.now() / 1000 };
      isYesterdayReport = false;
      cache.setReport(targetCacheKey, report);
      // 历史周条即时点亮当天（无需重新探测整周）
      if (savedReport && targetDate in weekReportStatus) {
        weekReportStatus = { ...weekReportStatus, [targetDate]: true };
      }

      if (
        shouldShowPromptAppliedToast({
          configAiMode: config?.ai_mode,
          customPrompt: config?.daily_report_custom_prompt,
          reportAiMode: savedReport?.ai_mode,
        })
      ) {
        showToast(t('report.promptApplied'), 'success');
      }
    } catch (e) {
      if (requestId === reportRequestId) {
        error = formatUserError(e, t('common.loadFailedRetry'));
      }
    } finally {
      if (reportGenerationOwnership.release(requestId)) {
        cache.setReportGenerating(false);
      }
    }
  }

  async function persistReportPrompt() {
    if (!config || config.ai_mode !== 'summary') {
      return;
    }

    const customPrompt = (config.daily_report_custom_prompt || '').trim();
    config.daily_report_custom_prompt = customPrompt;
    pendingReportPrompt = customPrompt;

    if (promptSaveTask) {
      return promptSaveTask;
    }

    promptSaving = true;
    const drainTask = (async () => {
      let firstSaveError: unknown;
      let hasSaveError = false;

      while (pendingReportPrompt !== null) {
        const promptToSave = pendingReportPrompt;
        pendingReportPrompt = null;
        try {
          await updateConfigQueued<ReportConfig>((latestConfig) => {
            latestConfig.daily_report_custom_prompt = promptToSave;
          });
        } catch (e) {
          if (!hasSaveError) {
            firstSaveError = e;
            hasSaveError = true;
          }
        }
      }

      if (hasSaveError) {
        throw firstSaveError;
      }
    })();
    promptSaveTask = drainTask.finally(() => {
      promptSaving = false;
      promptSaveTask = null;
    });
    return promptSaveTask;
  }

  /** 预设数量上限：防止胶片区无界增高（与工作时段 MAX_WORK_SEGMENTS 同类防御）。 */
  const MAX_PROMPT_PRESETS = 12;

  async function persistPromptPresets() {
    if (!config) return;
    const promptPresets = structuredClone(config.daily_report_prompt_presets);
    try {
      await updateConfigQueued<ReportConfig>((latestConfig) => {
        latestConfig.daily_report_prompt_presets = promptPresets;
      });
    } catch (e) {
      console.error('保存预设失败:', e);
    }
  }

  // 把节点移到 document.body，规避祖先的 backdrop-filter / overflow 对 position:fixed 的干扰
  async function exportReportMarkdown() {
    if (!report) return;

    exportInProgress = true;
    try {
      let exportDir = config?.daily_report_export_dir || null;
      if (!exportDir) {
        const selected = await openDialog({
          directory: true,
          multiple: false,
        });

        if (!selected || Array.isArray(selected)) {
          return;
        }

        exportDir = selected;
      }

      const exportPath = await invoke<string>('export_report_markdown', {
        date: report.date || selectedDate,
        content: report.content,
        exportDir,
      });
      showToast(t('report.exportSuccess', { path: exportPath }), 'success');
    } catch (e) {
      showToast(t('report.exportFailed', { error: e }), 'error');
    } finally {
      exportInProgress = false;
    }
  }

  /** 导出菜单项：导出当日 Markdown（先关菜单再走既有导出管线）。 */
  function handleExportCurrent() {
    showExportMenu = false;
    exportReportMarkdown();
  }

  /** 导出菜单项（新增）：复制全文——喂周报 / IM 的最短路径,不落盘。 */
  async function copyReportContent() {
    if (!report?.content) return;
    showExportMenu = false;
    try {
      await navigator.clipboard.writeText(report.content);
      showToast(t('report.copySuccess'), 'success');
    } catch (e) {
      showToast(t('report.copyFailed'), 'error');
    }
  }

  // ===== 批量日报合并导出 =====
  let showBatchExportModal = false;
  let batchExporting = false;
  let batchStartDate = '';
  let batchEndDate = '';

  // ISO 日期字符串工具（避开 toISOString 的 UTC 时区坑）
  function toIsoDate(date: Date): string {
    const y = date.getFullYear();
    const m = String(date.getMonth() + 1).padStart(2, '0');
    const d = String(date.getDate()).padStart(2, '0');
    return `${y}-${m}-${d}`;
  }

  // 计算"本周/上周"的范围，约定周一为一周开始
  // 注：getDay() 周日=0，周一=1，所以 (day + 6) % 7 是距离本周一的天数
  function weekRange(offsetWeeks: number): { start: string; end: string } {
    const today = new Date();
    const dayFromMonday = (today.getDay() + 6) % 7;
    const monday = new Date(today);
    monday.setDate(today.getDate() - dayFromMonday + offsetWeeks * 7);
    const sunday = new Date(monday);
    sunday.setDate(monday.getDate() + 6);
    return { start: toIsoDate(monday), end: toIsoDate(sunday) };
  }

  function monthRange(offsetMonths: number): { start: string; end: string } {
    const today = new Date();
    const start = new Date(today.getFullYear(), today.getMonth() + offsetMonths, 1);
    const end = new Date(today.getFullYear(), today.getMonth() + offsetMonths + 1, 0);
    return { start: toIsoDate(start), end: toIsoDate(end) };
  }

  function applyBatchPreset(preset: BatchPreset) {
    let range: { start: string; end: string } | null = null;
    if (preset === 'thisWeek') range = weekRange(0);
    else if (preset === 'lastWeek') range = weekRange(-1);
    else if (preset === 'thisMonth') range = monthRange(0);
    else if (preset === 'lastMonth') range = monthRange(-1);
    if (range) {
      batchStartDate = range.start;
      batchEndDate = range.end;
    }
  }

  function openBatchExportModal() {
    showExportMenu = false;
    // 默认填本月范围，省一步点击
    if (!batchStartDate || !batchEndDate) {
      applyBatchPreset('thisMonth');
    }
    showBatchExportModal = true;
  }

  /** 历史周条尾部入口：预填本周范围直接进合并导出（导出与历史在同一处闭环）。 */
  function openWeekBatchExport() {
    applyBatchPreset('thisWeek');
    showBatchExportModal = true;
  }

  async function exportReportsRange() {
    if (batchExporting) return;
    if (!batchStartDate || !batchEndDate) {
      showToast(t('report.batchExportInvalidRange'), 'error');
      return;
    }
    if (batchStartDate > batchEndDate) {
      showToast(t('report.batchExportInvalidRange'), 'error');
      return;
    }

    const targetPath = await saveDialog({
      defaultPath: `reports-${batchStartDate}_to_${batchEndDate}.md`,
      filters: [{ name: 'Markdown', extensions: ['md'] }],
    });
    if (!targetPath) return;

    batchExporting = true;
    try {
      const result = await invoke<ExportRangeResult>('export_reports_range', {
        startDate: batchStartDate,
        endDate: batchEndDate,
        targetPath,
        locale: currentLocale,
      });
      showToast(
        t('report.batchExportSuccess', { path: result.path, count: result.count }),
        'success',
      );
      showBatchExportModal = false;
    } catch (e) {
      showToast(t('report.batchExportFailed', { error: e }), 'error');
    } finally {
      batchExporting = false;
    }
  }

  function renderMarkdown(content: string): string {
    const rawHtml = marked.parse(content);
    return typeof rawHtml === 'string' ? DOMPurify.sanitize(rawHtml) : '';
  }

  async function handleReportLinkClick(event: MouseEvent) {
    const target = event.target;
    const link = target instanceof Element ? target.closest('a[href]') : null;
    if (!link) return;

    const href = link.getAttribute('href');
    if (!href || href.startsWith('#')) return;

    event.preventDefault();
    try {
      await open(href);
    } catch (e) {
      console.error('打开日报链接失败:', e);
    }
  }

  function interceptReportLinks(node: HTMLElement) {
    const listener = (event: MouseEvent) => {
      handleReportLinkClick(event);
    };

    node.addEventListener('click', listener);

    return {
      destroy() {
        node.removeEventListener('click', listener);
      }
    };
  }

  // 结构化编辑：将 markdown 按 ## 标题拆分为段落
  let editingSection = -1; // 当前正在编辑的段落索引
  let editingContent = ''; // 编辑中的内容

  function startEditSection(sections: readonly ReportSection[], index: number) {
    editingSection = index;
    const section = sections[index];
    editingContent = reportSectionMarkdownForStorage(section);
  }

  /** 删除预设：走全局确认弹窗(与全应用删除交互一致);删除当前生效预设时清空提示词。 */
  async function deletePreset(index: number) {
    if (!config) return;
    const currentConfig = config;
    const preset = currentConfig.daily_report_prompt_presets[index];
    if (!preset) return;
    const ok = await confirm({
      tone: 'warning',
      title: t('report.confirmDeletePreset', { name: preset.name }),
      message: preset.prompt,
      confirmText: t('common.confirm'),
      cancelText: t('common.cancel'),
    });
    if (!ok) return;
    const wasActive = currentConfig.daily_report_custom_prompt === preset.prompt;
    currentConfig.daily_report_prompt_presets = currentConfig.daily_report_prompt_presets.filter((_, j) => j !== index);
    if (wasActive) {
      currentConfig.daily_report_custom_prompt = '';
      persistReportPrompt();
    }
    await persistPromptPresets();
  }

  function selectPromptPreset(preset: PromptPreset, active: boolean) {
    if (!config) return;
    config.daily_report_custom_prompt = active ? '' : preset.prompt;
    void persistReportPrompt();
  }

  function resetSystemPromptOverride() {
    if (config) config.daily_report_system_prompt_override = null;
  }

  async function savePresetEditor() {
    if (presetSaving || !config) return;
    presetSaving = true;
    try {
      const presets = [...config.daily_report_prompt_presets];
      const entry = { name: editingPresetName.trim(), prompt: editingPresetPrompt.trim() };
      if (editingPresetIndex >= 0) {
        presets[editingPresetIndex] = entry;
      } else {
        presets.push(entry);
      }
      config.daily_report_prompt_presets = presets;
      await persistPromptPresets();
      showPresetModal = false;
    } finally {
      presetSaving = false;
    }
  }

  function cancelEditSection() {
    editingSection = -1;
    editingContent = '';
  }

  let savingSection = false;
  async function saveEditSection(sections: readonly ReportSection[], index: number) {
    if (savingSection || !report) return;
    savingSection = true;
    const newContent = editingContent.trim();
    const newSections = [...sections];
    const parsed = parseReportSections(newContent || '');
    if (parsed.length > 0) {
      newSections[index] = parsed[0];
      // If user added more ## headers, merge them in
      if (parsed.length > 1) {
        newSections.splice(index + 1, 0, ...parsed.slice(1));
      }
    }

    const fullContent = newSections.map(reportSectionMarkdownForStorage).join('\n');

    try {
      await invoke('update_report_content', { date: selectedDate, locale: currentLocale, content: fullContent });
      report = { ...report, content: fullContent };
      cache.setReport(currentReportCacheKey, report);
      editingSection = -1;
      editingContent = '';
    } catch (e) {
      showToast(t('report.editSectionFailed') + ': ' + e, 'error');
    } finally {
      savingSection = false;
    }
  }

  function formatReportDate(dateStr: string): string {
    // 用正午时间避免 "YYYY-MM-DD" 被按 UTC 午夜解析导致西时区日期偏移一天
    const date = new Date(`${dateStr}T12:00:00`);
    return formatLocalizedDate(date, { year: 'numeric', month: 'long', day: 'numeric', weekday: 'long' });
  }

  $: if (currentReportCacheKey && currentReportCacheKey !== lastLoadedDate) {
    const previousReport = report;
    lastLoadedDate = currentReportCacheKey;
    report = null;
    editingSection = -1;
    isYesterdayReport = false;
    loadReport(previousReport);
  }

  $: reportSections = parseReportSections(report?.content || '');
  // 钉选/隐藏偏好（从 config 读取，前端即时过滤）
  $: pinnedBlocks = config?.daily_report_pinned_blocks || [];
  $: hiddenBlocks = config?.daily_report_hidden_blocks || [];

  $: visibleSections = getVisibleReportSections(reportSections, pinnedBlocks, hiddenBlocks);

  async function togglePinBlock(section: ReportSection) {
    if (!config) return;
    const currentConfig = config;
    const blockName = extractReportBlockName(section);
    if (!blockName) return;
    const newPinned = pinnedBlocks.includes(blockName)
      ? pinnedBlocks.filter((b) => b !== blockName)
      : [...pinnedBlocks, blockName];
    try {
      await invoke('set_report_block_preference', {
        pinnedBlocks: newPinned,
        hiddenBlocks,
      });
      config = { ...currentConfig, daily_report_pinned_blocks: newPinned };
    } catch (e) { console.error('设置钉选失败:', e); }
  }

  async function toggleHideBlock(section: ReportSection) {
    if (!config) return;
    const currentConfig = config;
    const blockName = extractReportBlockName(section);
    if (!blockName) return;
    const newHidden = hiddenBlocks.includes(blockName)
      ? hiddenBlocks.filter((b) => b !== blockName)
      : [...hiddenBlocks, blockName];
    try {
      await invoke('set_report_block_preference', {
        pinnedBlocks,
        hiddenBlocks: newHidden,
      });
      config = { ...currentConfig, daily_report_hidden_blocks: newHidden };
    } catch (e) { console.error('设置隐藏失败:', e); }
  }

  $: reportMeta = resolveReportMeta(report, config);

  // ══════════ 洞察（TL;DR）：文章头居中 lead 段,首版取正文首个 blockquote / 首段摘要 ══════════
  // 生成端输出专用 summary 字段属后端增强,留待后端批次;派生失败时整段隐藏,不占版面。
  const INSIGHT_SCAN_LINES = 40;
  const INSIGHT_MAX_LENGTH = 160;
  const HTML_COMMENT_START = '<' + '!--';
  const HTML_COMMENT_RE = new RegExp(`${HTML_COMMENT_START}[\\s\\S]*?-->`, 'g');

  function stripInlineMarkdown(text: string): string {
    return (text || '')
      .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
      .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
      .replace(/[*_`~]/g, '')
      .trim();
  }

  function deriveReportInsight(content: string): string {
    if (!content) return '';
    const lines = content.split('\n').slice(0, INSIGHT_SCAN_LINES);
    let firstParagraph = '';
    for (const rawLine of lines) {
      const line = rawLine.trim();
      if (!line) continue;
      if (
        line.startsWith(HTML_COMMENT_START) ||
        line.startsWith('<details') ||
        line.startsWith('#') ||
        line.startsWith('|') ||
        line.startsWith('---')
      ) {
        continue;
      }
      // 首个 blockquote 优先——生成端通常把结论放在引用块里
      if (line.startsWith('>')) {
        const quote = stripInlineMarkdown(line.replace(/^>+\s*/, ''));
        if (quote) return quote.slice(0, INSIGHT_MAX_LENGTH);
        continue;
      }
      if (!firstParagraph && !line.startsWith('-') && !line.startsWith('*')) {
        firstParagraph = stripInlineMarkdown(line);
      }
    }
    return firstParagraph.slice(0, INSIGHT_MAX_LENGTH);
  }

  $: reportInsight = report && !isYesterdayReport ? deriveReportInsight(report.content) : '';

  /** 字数统计（文章头元信息行）：剔除注释与 markdown 标记后按非空白字符计。 */
  function countReportChars(content: string | null | undefined): number {
    if (!content) return 0;
    return content
      .replace(HTML_COMMENT_RE, '')
      .replace(/[#>*`_\-|[\]()!]/g, '')
      .replace(/\s+/g, '')
      .length;
  }

  $: reportCharCount = countReportChars(report?.content);

  // 「跳到明日建议」：按标题关键词定位建议段,找不到则不显示链接
  const ADVICE_TITLE_RE = /(明日|明天|建议|建議|tomorrow|suggest|advice|recommend|اقتراح|توصي)/i;
  $: adviceSectionIndex = visibleSections.findIndex((section) => ADVICE_TITLE_RE.test(tocTitle(section)));

  // ══════════ 历史周条：哪天有报告一眼可见（本周 7 天,点击切换日期,弱化为工具栏下细行） ══════════
  // 带首句摘要的历史列表需 `list_report_dates` 汇总接口,留待后端批次;
  // 这里用既有 get_saved_report 逐日探测"有/无",每个 locale 只探测一次。
  let weekReportStatus: Record<string, boolean> = {}; // date -> bool
  let weekStatusLocale = '';
  let weekStatusRequestId = 0;

  function getCurrentWeekDates(): string[] {
    const today = new Date();
    const dayFromMonday = (today.getDay() + 6) % 7;
    const monday = new Date(today);
    monday.setDate(today.getDate() - dayFromMonday);
    return Array.from({ length: 7 }, (_, i) => {
      const day = new Date(monday);
      day.setDate(monday.getDate() + i);
      return toIsoDate(day);
    });
  }

  async function loadWeekStripStatus() {
    if (weekStatusLocale === currentLocale) return;
    weekStatusLocale = currentLocale;
    const requestId = ++weekStatusRequestId;
    const todayStr = getLocalDateString();
    const dates = getCurrentWeekDates().filter((date) => date <= todayStr);
    const entries = await Promise.all(
      dates.map(async (date) => {
        try {
          const saved = await invoke<DailyReport | null>('get_saved_report', { date, locale: currentLocale });
          return [date, Boolean(saved)] as const;
        } catch {
          return [date, false] as const;
        }
      })
    );
    if (requestId !== weekStatusRequestId) return;
    weekReportStatus = Object.fromEntries(entries);
  }

  $: { currentLocale; loadWeekStripStatus(); }

  $: weekStripDays = (() => {
    // currentLocale 依赖:星期短标签随语言切换
    currentLocale;
    const todayStr = getLocalDateString();
    return getCurrentWeekDates().map((date) => {
      const parsed = new Date(`${date}T12:00:00`);
      return {
        date,
        label: formatLocalizedDate(parsed, { weekday: 'narrow' }),
        dayNum: parsed.getDate(),
        hasReport: !!weekReportStatus[date],
        isFuture: date > todayStr,
        isToday: date === todayStr,
      };
    });
  })();
  $: weekElapsedCount = weekStripDays.filter((day) => !day.isFuture).length;
  $: weekGeneratedCount = weekStripDays.filter((day) => day.hasReport).length;

  // ══════════ KPI 参照系（与报告同页的四个答案;快照固化留待后端批次,当前为实时口径） ══════════
  /** 专注峰值：hourly 分布最大桶向相邻延伸（相邻桶 ≥ 最大值 60% 时并入窗口），与概览定稿同一算法。 */
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

  function categorySharePct(stats: DailyStats | null, categoryKey: string): number | null {
    if (!stats || !(stats.total_duration > 0)) return null;
    const item = (stats.category_usage || []).find((c) => c.category === categoryKey);
    return Math.round(((item?.duration || 0) / stats.total_duration) * 100);
  }

  $: kpiTotalDeltaText = freshStats && lastWeekStats && lastWeekStats.total_duration > 0
    ? t('report.kpiDeltaVsLastWeek', {
        delta: formatSignedCompactDuration(freshStats.total_duration - lastWeekStats.total_duration),
      })
    : t('report.kpiNoBaseline');
  $: peakWindow = freshStats ? computePeakWindow(freshStats.hourly_activity_distribution) : null;
  $: peakWindowValue = peakWindow
    ? `${String(peakWindow.startHour).padStart(2, '0')}:00–${String(peakWindow.endHour + 1).padStart(2, '0')}:00`
    : '--';
  $: commSharePct = categorySharePct(freshStats, 'communication');
  $: commShareBaselinePct = categorySharePct(lastWeekStats, 'communication');
  $: commShareDeltaText = commSharePct != null && commShareBaselinePct != null
    ? t('report.kpiDeltaPt', {
        delta: `${commSharePct - commShareBaselinePct >= 0 ? '+' : '−'}${Math.abs(commSharePct - commShareBaselinePct)}`,
      })
    : t('report.kpiNoBaseline');

  // ══════════ 数据对照面板（文末折叠）：分类构成条,与正文百分比互证 ══════════
  // 证据 chips 与跳时间线定位依赖生成端结构化引用,留待后端批次。
  function categoryDisplayName(categoryKey: string, cats: readonly CategoryDefinition[]): string {
    const translated = translateCategoryLabel(categoryKey);
    if (translated !== categoryKey) return translated;
    const found = cats.find((c) => c.key === categoryKey);
    return found?.name || categoryKey;
  }

  $: proofSegments = (() => {
    currentLocale;
    const usage = freshStats?.category_usage || [];
    const total = usage.reduce((sum, item) => sum + item.duration, 0);
    if (total <= 0) return [];
    const colorMap = new Map(categoryList.map((c) => [c.key, c.color]));
    return usage
      .slice()
      .sort((left, right) => right.duration - left.duration)
      .map((item) => ({
        key: item.category,
        name: categoryDisplayName(item.category, categoryList),
        color: colorMap.get(item.category) || '#94a3b8',
        duration: item.duration,
        widthPct: (item.duration / total) * 100,
        percent: Math.round((item.duration / total) * 100),
      }));
  })();

  // ══════════ 段落目录（宽屏贴右侧悬浮,≥1024px;窄窗折叠为文章顶部锚点条） ══════════
  let activeSectionIndex = 0;
  let sectionObserver: IntersectionObserver | null = null;

  function tocTitle(section: VisibleReportSection): string {
    return (section?.title || '').replace(/^#+\s*/, '').trim();
  }

  /** 段落锚点 action:注册到 IntersectionObserver,滚动时高亮当前段。 */
  function tocAnchor(node: HTMLElement, index: number) {
    node.dataset.tocIndex = String(index);
    if (!sectionObserver && typeof IntersectionObserver !== 'undefined') {
      sectionObserver = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) {
              activeSectionIndex = Number((entry.target as HTMLElement).dataset.tocIndex) || 0;
            }
          }
        },
        { rootMargin: '-20% 0px -70% 0px' }
      );
    }
    sectionObserver?.observe(node);
    return {
      update(nextIndex: number) {
        node.dataset.tocIndex = String(nextIndex);
      },
      destroy() {
        sectionObserver?.unobserve(node);
      },
    };
  }

  function scrollToSection(index: number) {
    document
      .getElementById(`report-sec-${index}`)
      ?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }

  async function restoreHiddenBlock(blockName: string) {
    if (!config) return;
    const currentConfig = config;
    const newHidden = hiddenBlocks.filter((item) => item !== blockName);
    try {
      await invoke('set_report_block_preference', { pinnedBlocks, hiddenBlocks: newHidden });
      config = { ...currentConfig, daily_report_hidden_blocks: newHidden };
    } catch (e) {
      console.error('设置隐藏失败:', e);
    }
  }

  onMount(() => {
    loadConfig();
    // 分类色板：数据对照面板构成条着色（与概览/时间线同一色源）
    invoke<CategoryDefinition[]>('get_categories')
      .then((cats) => { categoryList = cats || []; })
      .catch(() => { categoryList = []; });
    return () => {
      sectionObserver?.disconnect();
      sectionObserver = null;
    };
  });

  // 页面重新获得焦点时刷新配置，确保 AI 增强状态为最新
  let configRefreshTimer = 0;
  function refreshConfigOnFocus() {
    const now = Date.now();
    if (now - configRefreshTimer < 2000) return;
    configRefreshTimer = now;
    loadConfig();
  }
</script>

<svelte:window on:focusin={refreshConfigOnFocus} on:visibilitychange={() => {
  if (document.visibilityState === 'visible') refreshConfigOnFocus();
}} />

<div class="page-shell report-editorial-shell" data-locale={currentLocale}>
  <!-- 页头：与概览同构——左=「日报」页面标题,右=控制组(日期切换器+导出▾+生成设置+重新生成);
       文章头保持下沉到正文卡内居中 -->
  <div class="report-hero">
    <div class="report-hero-main">
      <div class="page-title-group report-hero-copy">
        <div class="page-title-badge">
          <svg fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M8 7h8M8 12h8M8 17h5" />
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M7 3h7l5 5v10a3 3 0 01-3 3H7a3 3 0 01-3-3V6a3 3 0 013-3Z" />
          </svg>
        </div>
        <div class="page-title-copy">
          <h2>{t('sidebar.nav.report')}</h2>
        </div>
      </div>
      <div class="report-hero-actions">
        <div class="page-toolbar-end">
          <button
            type="button"
            class="page-control-btn"
            on:click={selectPreviousDay}
          >
            {t('report.previousDay')}
          </button>
          <button
            class="page-control-btn {selectedDate === getLocalDateString() ? 'page-control-btn-active' : ''}"
            on:click={() => selectDate(getLocalDateString())}
          >
            {t('report.today')}
          </button>
          <button
            class="page-control-btn {selectedDate === getYesterdayDateString() ? 'page-control-btn-active' : ''}"
            on:click={() => selectDate(getYesterdayDateString())}
          >
            {t('report.yesterday')}
          </button>
          {#key `report-date-${currentLocale}`}
            <LocalizedDatePicker
              bind:value={selectedDate}
              max={getLocalDateString()}
              localeCode={currentLocale}
              triggerClass="page-control-input w-auto"
            />
          {/key}
        </div>
        {#if report}
          <div class="report-export-menu">
            <button
              type="button"
              class="page-action-secondary min-h-9 px-3.5 py-1.5"
              aria-haspopup="menu"
              aria-expanded={showExportMenu}
              on:click={() => (showExportMenu = !showExportMenu)}
            >
              {#if exportInProgress}
                <div class="animate-spin rounded-full h-4 w-4 border-2 border-current border-t-transparent"></div>
                {t('report.exporting')}
              {:else}
                {t('report.exportMenu')}
                <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
                </svg>
              {/if}
            </button>
            {#if showExportMenu}
              <button
                type="button"
                class="report-export-menu-backdrop"
                aria-label={t('common.cancel')}
                on:click={() => (showExportMenu = false)}
              ></button>
              <div class="report-export-menu-panel" role="menu">
                <button
                  type="button"
                  class="report-export-menu-item"
                  role="menuitem"
                  on:click={handleExportCurrent}
                  disabled={exportInProgress}
                >
                  <span class="report-export-menu-item-title">{t('report.exportMenuCurrent')}</span>
                  <span class="report-export-menu-item-sub">{config?.daily_report_export_dir || t('report.exportWithoutDefaultDir')}</span>
                </button>
                <button
                  type="button"
                  class="report-export-menu-item"
                  role="menuitem"
                  on:click={openBatchExportModal}
                  disabled={batchExporting}
                >
                  <span class="report-export-menu-item-title">{t('report.exportMenuRange')}</span>
                  <span class="report-export-menu-item-sub">{t('report.batchExportTitle')}</span>
                </button>
                <button
                  type="button"
                  class="report-export-menu-item"
                  role="menuitem"
                  on:click={copyReportContent}
                >
                  <span class="report-export-menu-item-title">{t('report.exportMenuCopy')}</span>
                  <span class="report-export-menu-item-sub">{t('report.exportMenuCopyHint')}</span>
                </button>
              </div>
            {/if}
          </div>
        {/if}
        {#if config?.ai_mode === 'summary' || hiddenBlocks.length > 0}
          <button
            type="button"
            class="page-action-secondary min-h-9 px-3 py-1.5"
            title={t('report.generateSettings')}
            aria-expanded={showGenerateDrawer}
            on:click={() => (showGenerateDrawer = !showGenerateDrawer)}
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
            </svg>
            {t('report.generateSettings')}
            {#if hiddenBlocks.length > 0}
              <span class="ml-1 rounded-full bg-slate-200 dark:bg-[#484f58] px-1.5 text-[10px] font-semibold">{hiddenBlocks.length}</span>
            {/if}
          </button>
        {/if}
        {#if report}
          <button
            class="page-action-warn"
            on:click={() => generateReport(true)}
            disabled={generating}
          >
            {#if generating}
              <div class="animate-spin rounded-full h-4 w-4 border-2 border-white border-t-transparent"></div>
              {t('report.generating')}
            {:else}
              <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
              </svg>
              {t('report.regenerate')}
            {/if}
          </button>
        {/if}
      </div>
    </div>
  </div>

  <div class="report-editorial-stack">
  <!-- 历史周条：工具栏下方一行,弱化为无卡片细行（解 A6） -->
  <div class="report-weekstrip" aria-label={t('report.weekStripLabel')}>
    <span class="report-weekstrip-label">{t('report.weekStripLabel')}</span>
    <div class="report-weekstrip-days">
      {#each weekStripDays as day (day.date)}
        <button
          type="button"
          class="report-weekstrip-day {day.hasReport ? '' : 'report-weekstrip-day-none'} {day.date === selectedDate ? 'report-weekstrip-day-active' : ''}"
          disabled={day.isFuture}
          title={day.isFuture ? '' : day.hasReport ? t('report.weekStripHasReport') : t('report.weekStripNoReport')}
          on:click={() => selectDate(day.date)}
        >
          <span class="report-weekstrip-day-label">{day.label} {day.dayNum}</span>
          <i></i>
        </button>
      {/each}
    </div>
    <span class="report-weekstrip-meta">
      {t('report.weekStripGenerated', { count: weekGeneratedCount, total: weekElapsedCount })}
      ·
      <button type="button" class="report-weekstrip-export" on:click={openWeekBatchExport}>
        {t('report.weekStripExport')}
      </button>
    </span>
  </div>

  <!-- 生成设置抽屉：提示词预设 + 系统提示词覆盖 + 已隐藏段落管理（配置只在要生成时出现,解 A4） -->
  {#if showGenerateDrawer && (config?.ai_mode === 'summary' || hiddenBlocks.length > 0)}
    <div class="page-card report-sheet-controls report-generate-drawer">
      <div class="report-drawer-head">
        <h3 class="text-sm font-semibold">{t('report.generateSettings')}</h3>
        <button
          class="text-slate-400 hover:text-slate-600 dark:text-[#7d8590] dark:hover:text-[#c9d1d9]"
          title={t('report.cancelEdit')}
          on:click={() => (showGenerateDrawer = false)}
        >
          <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" /></svg>
        </button>
      </div>

      {#if config && config.ai_mode === 'summary'}
        <div class="report-drawer-section">
          <label for="daily-report-custom-prompt" class="settings-label mb-1.5">{t('report.promptLabel')}</label>

          <!-- 预设胶片集合：与分类管理同一套交互语言(点选应用,悬停浮出编辑/删除角标) -->
          <div class="mb-2 flex flex-wrap gap-2">
            {#each (config?.daily_report_prompt_presets || []) as preset, i}
              {@const presetActive = config.daily_report_custom_prompt === preset.prompt}
              <div class="group/preset relative">
                <button
                  type="button"
                  class="segment-btn flex-none rounded-lg border px-3 py-1.5 text-xs max-w-56 truncate
                    {presetActive ? 'settings-segment-success' : 'settings-segment-idle'}"
                  title={presetActive ? t('report.presetClickToUnselect') : preset.prompt}
                  on:click={() => selectPromptPreset(preset, presetActive)}
                >
                  {preset.name}
                </button>
                <button
                  type="button"
                  class="absolute -top-1.5 left-1/2 -translate-x-1/2 flex h-5 w-5 items-center justify-center rounded-[var(--radius-sm)] bg-blue-500 text-xs leading-none text-white opacity-0 shadow-sm transition-opacity hover:bg-blue-600 group-hover/preset:opacity-100 focus-visible:opacity-100 dark:shadow-none"
                  title={t('report.editPreset')}
                  on:click|stopPropagation={() => {
                    editingPresetIndex = i;
                    editingPresetName = preset.name;
                    editingPresetPrompt = preset.prompt;
                    showPresetModal = true;
                  }}
                >✎</button>
                <button
                  type="button"
                  class="absolute -top-1.5 -end-1.5 flex h-5 w-5 items-center justify-center rounded-[var(--radius-sm)] bg-red-500 text-xs leading-none text-white opacity-0 shadow-sm transition-opacity hover:bg-red-600 group-hover/preset:opacity-100 focus-visible:opacity-100 dark:shadow-none"
                  title={t('common.delete')}
                  on:click|stopPropagation={() => deletePreset(i)}
                >×</button>
              </div>
            {/each}
            {#if (config?.daily_report_prompt_presets || []).length < MAX_PROMPT_PRESETS}
              <button
                type="button"
                class="segment-btn settings-segment-idle flex-none rounded-lg border border-dashed px-3 py-1.5 text-xs"
                on:click={() => {
                  editingPresetIndex = -1;
                  editingPresetName = '';
                  editingPresetPrompt = '';
                  showPresetModal = true;
                }}
              >
                + {t('report.addPreset')}
              </button>
            {:else}
              <span class="inline-flex items-center px-2 text-xs text-slate-400 dark:text-[#636c76]" title={t('report.presetLimitReached', { max: MAX_PROMPT_PRESETS })}>
                {t('report.presetLimitReached', { max: MAX_PROMPT_PRESETS })}
              </span>
            {/if}
          </div>
          <textarea
            id="daily-report-custom-prompt"
            bind:value={config.daily_report_custom_prompt}
            on:change={persistReportPrompt}
            rows="3"
            class="control-input resize-y min-h-[80px]"
            placeholder={t('report.promptPlaceholder')}
          ></textarea>

          <!-- 系统提示词覆盖 -->
          <div class="mt-4 pt-3 border-t border-slate-200 dark:border-[#30363d]">
            <div class="flex items-center justify-between mb-2">
              <label for="daily-report-system-prompt-override" class="text-sm font-medium text-slate-700 dark:text-[#adbac7]">
                {t('report.systemPromptOverride')}
              </label>
              <button
                type="button"
                class="text-xs text-slate-400 hover:text-slate-700 dark:hover:text-[#adbac7] transition"
                on:click={resetSystemPromptOverride}
                disabled={!config.daily_report_system_prompt_override}
              >
                {t('report.resetSystemPrompt')}
              </button>
            </div>
            <p class="text-xs text-slate-400 dark:text-[#636c76] mb-2">{t('report.systemPromptOverrideHint')}</p>
            <textarea
              id="daily-report-system-prompt-override"
              rows="6"
              class="control-input resize-y min-h-[100px] font-mono text-xs"
              bind:value={config.daily_report_system_prompt_override}
              on:change={persistReportPrompt}
              placeholder={t('report.systemPromptOverridePlaceholder')}
            ></textarea>
          </div>
        </div>
      {/if}

      <!-- 段落管理入口迁入抽屉：恢复被隐藏的段落 -->
      {#if hiddenBlocks.length > 0}
        <div class="report-drawer-section">
          <h4 class="text-xs font-semibold text-slate-500 dark:text-[#7d8590] mb-2">{t('report.manageBlocksTitle')}</h4>
          <div class="flex flex-wrap gap-2">
            {#each hiddenBlocks as blockName}
              <button
                class="inline-flex items-center gap-1.5 rounded-lg border border-slate-200 dark:border-[#30363d] bg-white dark:bg-[#21262d] px-3 py-1.5 text-xs font-medium text-slate-700 dark:text-[#adbac7] hover:border-blue-300 dark:hover:border-blue-500 transition-colors"
                on:click={() => restoreHiddenBlock(blockName)}
              >
                <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>
                {tm(`report.blockNames.${blockName}`) || blockName}
              </button>
            {/each}
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <!-- KPI：与报告同页的四个答案（解 A2;实时口径,快照固化留待后端批次;汇总数居中） -->
  {#if report && !loading && !error && !isYesterdayReport && freshStats}
    <div class="report-kpi-grid grid grid-cols-2 lg:grid-cols-4 gap-3">
      <div class="report-stat-card">
        <div class="report-stat-label">{t('report.kpiTotal')}</div>
        <div class="report-stat-value">{formatDurationLocalized(freshStats.total_duration)}</div>
        <div class="report-stat-sub">{kpiTotalDeltaText}</div>
      </div>
      <div class="report-stat-card">
        <div class="report-stat-label">{t('report.kpiPeakFocus')}</div>
        <div class="report-stat-value">{peakWindowValue}</div>
        <div class="report-stat-sub">
          {peakWindow ? t('report.kpiPeakTotal', { dur: formatDurationLocalized(peakWindow.totalDuration, { compact: true }) }) : t('report.kpiNoBaseline')}
        </div>
      </div>
      <div class="report-stat-card">
        <div class="report-stat-label">{t('report.kpiCommShare')}</div>
        <div class="report-stat-value">{commSharePct == null ? '--' : `${commSharePct}%`}</div>
        <div class="report-stat-sub">{commShareDeltaText}</div>
      </div>
      <div class="report-stat-card">
        <div class="report-stat-label">{t('report.kpiDataBase')}</div>
        <div class="report-stat-value">{freshStats.screenshot_count}</div>
        <div class="report-stat-sub" title={t('report.liveBasisTitle')}>{t('report.kpiDataBaseMeta', { apps: freshStats.app_usage?.length ?? 0 })}</div>
      </div>
    </div>
  {/if}

  <!-- 日报内容 -->
  {#if loading}
    <div class="empty-state-lg">
      <div class="empty-state-icon">
        <div class="animate-spin rounded-full h-8 w-8 border-2 border-blue-500 border-t-transparent"></div>
      </div>
      <h3 class="empty-state-title">{t('report.loadingTitle')}</h3>
      <p class="empty-state-copy mt-1">{t('report.loadingCopy')}</p>
    </div>
  {:else if error}
    <div class="page-banner-error">
      <div>
        <div class="flex items-center gap-3 text-red-500 mb-2">
        <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
        </svg>
        <span class="font-medium">{t('report.generateFailed')}</span>
      </div>
      <p class="text-sm">{error}</p>
      </div>
      <button class="page-action-brand" on:click={() => generateReport(true)}>{t('common.retry')}</button>
    </div>
  {:else if report}
    <div class="report-reading-layout">
      <div class="page-card report-sheet report-article-card min-w-0">
      <div class="report-sheet-content">
        <!-- 昨日回退唯一横幅：页级警示横幅已删,同一信息不再占据两条首屏位（解 A5） -->
        {#if isYesterdayReport}
          <div class="page-banner-warning report-fallback-banner mb-4">
            <div class="report-fallback-copy">
              <div class="flex items-center gap-2 text-sm">
                <svg class="w-4 h-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                </svg>
                {t('report.showingYesterday', { date: formatReportDate(report.date) })}
              </div>
            </div>
            <div class="report-fallback-action">
              <button
                class="page-action-warn report-fallback-button min-h-9 px-3 text-xs rounded-lg shadow-none"
                on:click={() => generateReport(false)}
                disabled={generating}
              >
                {#if generating}
                  <div class="inline-flex items-center gap-2">
                    <div class="animate-spin rounded-full h-3 w-3 border-2 border-white border-t-transparent"></div>
                    <span>{t('report.generating')}</span>
                  </div>
                {:else}
                  ✨ {t('report.generateTodayNow')}
                {/if}
              </button>
            </div>
          </div>
        {/if}

        <!-- 文章头（居中）：kicker + 日期大标题 + 元信息行（字数 · 生成时间 · 模式徽章）+ TL;DR lead 段 -->
        <header class="report-article-head">
          <p class="report-article-kicker">{selectedDate === getLocalDateString() ? t('report.todayReport') : t('report.historyReport')}</p>
          <h1 class="report-article-title">{formatReportDate(isYesterdayReport ? report.date : selectedDate)}</h1>
          <div class="report-hero-meta">
            <div class="report-hero-date-row report-article-metaline">
              {#if reportCharCount > 0}
                <span>{t('report.wordCount', { count: reportCharCount })}</span>
                <span class="report-article-metadot">·</span>
              {/if}
              <span>{t('report.generatedAt', { time: formatLocalizedDate(new Date(report.created_at * 1000), { month: '2-digit', day: '2-digit' }) + ' ' + formatLocalizedTime(new Date(report.created_at * 1000), { hour: '2-digit', minute: '2-digit' }) })}</span>
              <span class="report-article-metadot">·</span>
              <span class="report-hero-mode-chip">{getAiModeName(reportMeta.reportMode)}</span>
              <span class="report-meta-pill" title={t('report.liveBasisTitle')}>{t('report.liveBasisChip')}</span>
            </div>
            {#if reportMeta.showUsageMismatchNotice}
              <p class="report-hero-mode-note">{t('report.aiNotAppliedPrefix')}{getFallbackReasonText(reportMeta)}</p>
            {/if}
          </div>
          {#if reportInsight}
            <p class="report-article-lead">{reportInsight}</p>
            {#if adviceSectionIndex >= 0}
              <button type="button" class="report-insight-link" on:click={() => scrollToSection(adviceSectionIndex)}>
                {t('report.insightJumpAdvice')}
              </button>
            {/if}
          {/if}
        </header>

        <!-- 窄窗口锚点条：目录折叠后的横向导航（<1024px 显示,修 V4） -->
        {#if visibleSections.length > 1}
          <div class="report-anchor-bar" role="navigation" aria-label={t('report.tocLabel')}>
            {#each visibleSections as section, i}
              {@const anchorLabel = tocTitle(section)}
              {#if anchorLabel}
                <button
                  type="button"
                  class="report-anchor-chip {activeSectionIndex === i ? 'report-anchor-chip-active' : ''}"
                  on:click={() => scrollToSection(i)}
                >
                  {anchorLabel}
                </button>
              {/if}
            {/each}
          </div>
        {/if}

        <div class="markdown-body report-sheet-body prose prose-slate dark:prose-invert max-w-none">
          {#each visibleSections as section, i}
            {@const blockName = extractReportBlockName(section)}
            <div class="report-section group/section" id={`report-sec-${i}`} use:tocAnchor={i}>
              <div class="report-section-header">
                <div
                  use:interceptReportLinks
                  class="report-section-content"
                >
                  {@html renderMarkdown(reportSectionMarkdownForDisplay(section, section.displaySectionIndex ?? i, currentLocale))}
                </div>
                <div class="report-section-actions flex items-center gap-1 opacity-0 group-hover/section:opacity-100 focus-within:opacity-100 transition-opacity">
                  {#if blockName}
                    <button
                      class="report-section-edit-btn"
                      on:click={() => togglePinBlock(section)}
                      title={pinnedBlocks.includes(blockName) ? t('report.unpinBlock') : t('report.pinBlock')}
                    >
                      <svg class="w-3.5 h-3.5" fill={pinnedBlocks.includes(blockName) ? 'currentColor' : 'none'} stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 5a2 2 0 012-2h10a2 2 0 012 2v16l-7-3.5L5 21V5z" />
                      </svg>
                    </button>
                    <button
                      class="report-section-edit-btn"
                      on:click={() => toggleHideBlock(section)}
                      title={t('report.hideBlock')}
                    >
                      <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.578 7.578l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21" />
                      </svg>
                    </button>
                  {/if}
                  <button
                    class="report-section-edit-btn"
                    on:click={() => startEditSection(reportSections, section.originalIndex ?? i)}
                    title={t('report.editSection')}
                  >
                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15.232 5.232l3.536 3.536m-2.036-5.036a2.5 2.5 0 113.536 3.536L6.5 21.036H3v-3.572L16.732 3.732z" />
                    </svg>
                  </button>
                </div>
              </div>
            </div>
          {/each}
        </div>

        <!-- 数据对照面板：收为文末折叠（原则 2/3 的互证保留,不打断阅读动线） -->
        {#if !isYesterdayReport && proofSegments.length > 0}
          <details class="report-proof-details">
            <summary>
              <b>{t('report.proofTitle')}</b>
              <span>{t('report.proofCaption')}</span>
            </summary>
            <div class="report-proof-panel">
              <div class="report-proof-bar">
                {#each proofSegments as segment (segment.key)}
                  <i style={`width: ${segment.widthPct.toFixed(1)}%; min-width: 5px; background: ${segment.color};`}></i>
                {/each}
              </div>
              <div class="report-proof-legend">
                {#each proofSegments as segment (segment.key)}
                  <span>
                    <i style={`background: ${segment.color};`}></i>
                    <b>{segment.name}</b>
                    {formatDurationLocalized(segment.duration, { compact: true })}
                    <em>{segment.percent}%</em>
                  </span>
                {/each}
              </div>
            </div>
          </details>
        {/if}
      </div>
      </div>

      <!-- 段落目录：宽屏贴右侧悬浮（≥1024px,点击平滑滚动,滚动时高亮当前段） -->
      {#if visibleSections.length > 1}
        <nav class="report-toc" aria-label={t('report.tocLabel')}>
          <p class="report-toc-title">{t('report.tocLabel')}</p>
          <ul>
            {#each visibleSections as section, i}
              {@const label = tocTitle(section)}
              {#if label}
                <li>
                  <button
                    type="button"
                    class="report-toc-item {activeSectionIndex === i ? 'report-toc-item-active' : ''}"
                    on:click={() => scrollToSection(i)}
                  >
                    {label}
                  </button>
                </li>
              {/if}
            {/each}
          </ul>
          <!-- 目录底部固定生成元信息,与文章头元信息互为冗余 -->
          <div class="report-toc-foot">
            <p>{t('report.generatedAt', { time: formatLocalizedTime(new Date(report.created_at * 1000), { hour: '2-digit', minute: '2-digit' }) })}</p>
            <p>{getAiModeName(reportMeta.reportMode)} · <span title={t('report.liveBasisTitle')}>{t('report.liveBasisChip')}</span></p>
          </div>
        </nav>
      {/if}
    </div>
    {:else if generating}
    <!-- 生成中骨架屏：替代空白等待 -->
    <div class="page-card report-sheet report-article-card">
      <div class="report-sheet-content animate-pulse space-y-4 py-2">
        <div class="h-3 w-40 rounded-full bg-slate-200/80 dark:bg-[#21262d] mx-auto"></div>
        <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {#each Array(4) as _}
            <div class="h-16 rounded-lg bg-slate-100/90 dark:bg-[#161b22]"></div>
          {/each}
        </div>
        <div class="h-6 w-1/3 rounded-full bg-slate-200/80 dark:bg-[#21262d]"></div>
        <div class="space-y-2.5">
          {#each Array(3) as _}
            <div class="h-3.5 rounded-full bg-slate-100/90 dark:bg-[#161b22]"></div>
          {/each}
          <div class="h-3.5 w-2/3 rounded-full bg-slate-100/90 dark:bg-[#161b22]"></div>
        </div>
        <div class="h-6 w-1/4 rounded-full bg-slate-200/80 dark:bg-[#21262d]"></div>
        <div class="space-y-2.5">
          {#each Array(4) as _}
            <div class="h-3.5 rounded-full bg-slate-100/90 dark:bg-[#161b22]"></div>
          {/each}
        </div>
        <p class="pt-2 text-center text-xs text-slate-400 dark:text-[#636c76]">{t('report.generating')}…</p>
      </div>
    </div>
    {:else}
    <div class="empty-state-lg">
      <div class="empty-state-icon !w-16 !h-16 !mb-5 bg-amber-50 dark:bg-amber-950/30">
        <span class="text-3xl">📝</span>
      </div>
      <h3 class="empty-state-title">
        {selectedDate === getLocalDateString() ? t('report.noReportToday') : t('report.noReportForDate', { date: selectedDate })}
      </h3>
      <p class="empty-state-copy mb-5">
        {t('report.aiWillGenerate')}
      </p>
      <button
        class="page-action-warn min-h-11 px-6 py-3"
        on:click={() => generateReport(false)}
        disabled={generating}
      >
        {#if generating}
          <div class="inline-flex items-center gap-2">
            <div class="animate-spin rounded-full h-4 w-4 border-2 border-white border-t-transparent"></div>
            {t('report.generating')}
          </div>
        {:else}
          ✨ {selectedDate === getLocalDateString() ? t('report.generatingToday') : t('report.generatingSelected')}
        {/if}
      </button>
    </div>
  {/if}
</div>
</div>

<!-- 段落编辑弹窗 -->
{#if editingSection >= 0}
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="report-edit-section-title">
    <button type="button" class="absolute inset-0 cursor-default" aria-label={t('report.cancelEdit')} on:click={cancelEditSection}></button>
    <div class="modal-panel relative z-10">
      <div class="modal-header">
        <h3 id="report-edit-section-title" class="modal-title">{t('report.editSection')}</h3>
        <button class="modal-close" on:click={cancelEditSection}>
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </div>
      <div class="modal-body">
        <textarea
          class="report-edit-textarea"
          bind:value={editingContent}
        ></textarea>
      </div>
      <div class="modal-footer">
        <button class="page-control-btn" on:click={cancelEditSection}>
          {t('report.cancelEdit')}
        </button>
        <button
          class="page-action-brand"
          on:click={() => saveEditSection(reportSections, editingSection)}
          disabled={savingSection}
        >
          {#if savingSection}
            <div class="inline-flex items-center gap-2">
              <div class="animate-spin rounded-full h-4 w-4 border-2 border-white border-t-transparent"></div>
              {t('report.saveSection')}
            </div>
          {:else}
            {t('report.saveSection')}
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- 表格 / 标题 / 列表等 markdown 样式已统一放到 app.css .markdown-body -->

{#if showPresetModal}
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="report-preset-dialog-title">
    <button type="button" class="absolute inset-0 cursor-default" aria-label={t('report.cancelEdit')} on:click={() => { showPresetModal = false; }}></button>
    <div class="modal-panel relative z-10" style="max-width: 36rem;">
      <div class="modal-header">
        <h3 id="report-preset-dialog-title" class="modal-title">{editingPresetIndex >= 0 ? editingPresetName || t('report.presetsTitle') : t('report.addPreset')}</h3>
        <button class="modal-close" on:click={() => { showPresetModal = false; }}>
          <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
        </button>
      </div>
      <div class="modal-body space-y-4">
        <div>
          <label for="report-preset-name" class="block text-xs font-medium text-slate-500 dark:text-[#7d8590] mb-1.5">{t('report.presetNamePlaceholder')}</label>
          <input
            id="report-preset-name"
            type="text"
            class="w-full px-3 py-2 text-sm rounded-lg border border-slate-200 dark:border-[#484f58] bg-white dark:bg-[#21262d] text-slate-900 dark:text-[#c9d1d9] placeholder-slate-400 dark:placeholder-[#636c76] focus:outline-none focus:ring-2 focus:ring-blue-500/40 focus:border-blue-400 transition-colors"
            placeholder={t('report.presetNamePlaceholder')}
            bind:value={editingPresetName}
          />
        </div>
        <div>
          <label for="report-preset-prompt" class="block text-xs font-medium text-slate-500 dark:text-[#7d8590] mb-1.5">{t('report.promptLabel')}</label>
          <textarea
            id="report-preset-prompt"
            class="w-full px-3 py-2 text-sm rounded-lg border border-slate-200 dark:border-[#484f58] bg-white dark:bg-[#21262d] text-slate-900 dark:text-[#c9d1d9] placeholder-slate-400 dark:placeholder-[#636c76] focus:outline-none focus:ring-2 focus:ring-blue-500/40 focus:border-blue-400 transition-colors resize-y min-h-[160px] leading-relaxed"
            placeholder={t('report.presetPromptPlaceholder')}
            bind:value={editingPresetPrompt}
            rows="6"
          ></textarea>
        </div>
      </div>
      <div class="modal-footer">
        <button
          class="px-4 py-2 text-sm font-medium rounded-lg text-slate-700 dark:text-[#7d8590] hover:bg-slate-100 dark:hover:bg-[#30363d] transition-colors"
          on:click={() => { showPresetModal = false; }}
        >
          {t('report.cancelEdit')}
        </button>
        <button
          class="px-4 py-2 text-sm font-medium rounded-lg bg-blue-500 hover:bg-blue-600 text-white shadow-sm dark:shadow-none transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={!editingPresetName.trim() || !editingPresetPrompt.trim() || presetSaving}
          on:click={savePresetEditor}
        >
          {#if presetSaving}
            <span class="inline-flex items-center gap-1.5">
              <span class="w-3.5 h-3.5 border-2 border-white border-t-transparent rounded-full animate-spin"></span>
              {t('report.saving')}
            </span>
          {:else}
            {t('report.saveSection')}
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if showBatchExportModal}
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-labelledby="report-batch-export-dialog-title">
    <button type="button" class="absolute inset-0 cursor-default" aria-label={t('report.cancelEdit')} on:click={() => { if (!batchExporting) showBatchExportModal = false; }}></button>
    <div class="modal-panel relative z-10" style="max-width: 32rem;">
      <div class="modal-header">
        <h3 id="report-batch-export-dialog-title" class="modal-title">{t('report.batchExportModalTitle')}</h3>
        <button
          class="modal-close"
          on:click={() => { if (!batchExporting) showBatchExportModal = false; }}
          disabled={batchExporting}
        >
          <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
        </button>
      </div>
      <div class="modal-body space-y-4">
        <p class="text-xs text-slate-500 dark:text-[#7d8590]">{t('report.batchExportHint')}</p>

        <div class="flex flex-wrap gap-2">
          <button class="page-control-btn" on:click={() => applyBatchPreset('thisWeek')}>{t('report.batchPresetThisWeek')}</button>
          <button class="page-control-btn" on:click={() => applyBatchPreset('lastWeek')}>{t('report.batchPresetLastWeek')}</button>
          <button class="page-control-btn" on:click={() => applyBatchPreset('thisMonth')}>{t('report.batchPresetThisMonth')}</button>
          <button class="page-control-btn" on:click={() => applyBatchPreset('lastMonth')}>{t('report.batchPresetLastMonth')}</button>
        </div>

        <div class="grid gap-3 grid-cols-2">
          <label class="block">
            <span class="text-xs font-medium text-slate-500 dark:text-[#7d8590]">{t('report.batchStartDate')}</span>
            <input
              type="date"
              bind:value={batchStartDate}
              max={getLocalDateString()}
              class="mt-1 w-full px-3 py-2 text-sm rounded-lg border border-slate-200 dark:border-[#484f58] bg-white dark:bg-[#21262d] text-slate-900 dark:text-[#c9d1d9] focus:outline-none focus:ring-2 focus:ring-blue-500/40"
            />
          </label>
          <label class="block">
            <span class="text-xs font-medium text-slate-500 dark:text-[#7d8590]">{t('report.batchEndDate')}</span>
            <input
              type="date"
              bind:value={batchEndDate}
              max={getLocalDateString()}
              class="mt-1 w-full px-3 py-2 text-sm rounded-lg border border-slate-200 dark:border-[#484f58] bg-white dark:bg-[#21262d] text-slate-900 dark:text-[#c9d1d9] focus:outline-none focus:ring-2 focus:ring-blue-500/40"
            />
          </label>
        </div>
      </div>
      <div class="modal-footer">
        <button
          class="px-4 py-2 text-sm font-medium rounded-lg text-slate-700 dark:text-[#7d8590] hover:bg-slate-100 dark:hover:bg-[#30363d] transition-colors"
          on:click={() => { if (!batchExporting) showBatchExportModal = false; }}
          disabled={batchExporting}
        >
          {t('report.cancelEdit')}
        </button>
        <button
          class="px-4 py-2 text-sm font-medium rounded-lg bg-blue-500 hover:bg-blue-600 text-white shadow-sm dark:shadow-none transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          on:click={exportReportsRange}
          disabled={batchExporting || !batchStartDate || !batchEndDate}
        >
          {#if batchExporting}
            <span class="inline-flex items-center gap-1.5">
              <span class="w-3.5 h-3.5 border-2 border-white border-t-transparent rounded-full animate-spin"></span>
              {t('report.batchExporting')}
            </span>
          {:else}
            {t('report.batchExportConfirm')}
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}
