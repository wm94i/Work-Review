import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('概览页面的浏览器详情列表应格式化显示 URL', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(
    source,
    /formatBrowserUrlForDisplay\(url\.url\)/,
    '概览页的浏览器详情列表应对原始 URL 做可读化处理'
  );
});

test('概览页面应支持在网站访问弹层中直接修改域名语义分类并回填历史', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /invoke(?:<[^>]+>)?\('set_domain_semantic_rule'/);
  assert.match(source, /semanticCategoryStore/);
  assert.match(source, /overview\.changeDomainCategoryMessage/);
  assert.match(source, /overview\.selectCategory/);
  assert.match(source, /overview\.currentCategory/);
  assert.match(source, /getSemanticCategoryDisplayName/);
  assert.match(source, /\$semanticCategoryStore/);
  assert.match(source, /save_custom_semantic_category/);
  assert.match(source, /delete_custom_semantic_category/);
});

test('常驻网站应使用纯文字来源与分段轨道，不再展示域名印章或浏览器图标', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /overviewDomainPresentation\.ts/);
  assert.match(source, /buildDomainPresentation/);
  assert.match(source, /overview-domain-row/);
  assert.match(source, /overview-domain-source-list/);
  assert.match(source, /overview-domain-source-track/);
  assert.match(source, /overview-domain-source-segment/);
  assert.doesNotMatch(source, /overview-domain-stamp/);
  assert.doesNotMatch(source, /overview-domain-source-badge/);
  assert.doesNotMatch(source, /overview-domain-source-icon/);
  assert.doesNotMatch(source, /getDomainInitials/);
  assert.doesNotMatch(source, /getDomainStampClass/);
  assert.doesNotMatch(source, /getPrimaryDomainBrowser/);
});

test('网站语义分类保存应保持并发安全并刷新当前域名详情', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /let pendingDomainSemanticRequests(?:\s*:[^=]+)?\s*= new Map(?:<[^>]+>)?\(\);/);
  assert.match(source, /let nextDomainSemanticRequestId = 0;/);
  assert.match(source, /let domainSemanticEditSessionId = 0;/);
  assert.match(source, /function isDomainSemanticSavePending\(domainKey(?:\s*:[^)]+)?\)/);
  assert.match(source, /function isCurrentDomainSemanticSave\(domainKey(?:\s*:[^,]+)?, requestId(?:\s*:[^,]+)?, editSessionId(?:\s*:[^)]+)?\)/);
  assert.match(source, /function closeDomainOverlay\(\)[\s\S]*cancelDomainSemanticEdit\(\{ restoreFocus: false \}\)/);

  const cancelStart = source.indexOf('function cancelDomainSemanticEdit');
  const cancelEnd = source.indexOf('function getOverviewDomainParams', cancelStart);
  const cancelSource = source.slice(cancelStart, cancelEnd);
  assert.ok(cancelStart >= 0 && cancelEnd > cancelStart, '应保留独立的编辑会话关闭函数');
  assert.doesNotMatch(
    cancelSource,
    /pendingDomainSemanticRequests|savingDomainKey/,
    '关闭 Popover 或详情时不能清除仍在进行的后端保存请求'
  );

  const refreshStart = source.indexOf('async function refreshCurrentDomainDetail');
  const refreshEnd = source.indexOf('function shouldUseOverviewCache', refreshStart);
  const refreshSource = source.slice(refreshStart, refreshEnd);
  assert.match(refreshSource, /get_overview_domain_detail/);
  assert.match(refreshSource, /isCurrent/);
  assert.ok(
    (refreshSource.match(/if \(!isCurrent\(\)\) return false;/g) ?? []).length >= 2,
    '刷新前后都应验证请求仍属于当前编辑会话，避免重新打开已关闭详情'
  );

  const saveStart = source.indexOf('async function saveDomainSemanticRule');
  const saveEnd = source.indexOf('async function refreshOverviewStats', saveStart);
  const saveSource = source.slice(saveStart, saveEnd);
  assert.match(saveSource, /const domainKey = domain\.domain;/);
  assert.match(saveSource, /const editSessionId = domainSemanticEditSessionId;/);
  assert.match(saveSource, /const requestId = \+\+nextDomainSemanticRequestId;/);
  assert.match(saveSource, /setDomainSemanticSavePending\(domainKey, requestId\)/);
  assert.match(saveSource, /isCurrentDomainSemanticSave\(domainKey, requestId, editSessionId\)/);
  assert.match(saveSource, /refreshCurrentDomainDetail\(domainKey, isCurrent\)/);
  assert.match(saveSource, /finally \{[\s\S]*clearDomainSemanticSavePending\(domainKey, requestId\)/);
  assert.doesNotMatch(source, /let savingDomainKey/);
  assert.match(source, /disabled=\{isDomainSemanticSavePending\(domain\.domain\)\}/);
});

test('常驻网站应按需加载完整摘要并在卡片内展开，同时保留单域名详情弹层', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /let domainUsageExpanded = false/);
  assert.match(source, /let expandedDomainUsageItems(?:\s*:[^=]+)?\s*= \[\]/);
  assert.match(source, /domainUsageExpanded && expandedDomainUsageItems\.length > 0/);
  assert.match(source, /async function toggleDomainUsageExpanded\(\)/);
  assert.match(source, /invoke(?:<[^>]+>)?\('get_overview_domains', getOverviewDomainParams\(\)\)/);
  assert.match(source, /requestId !== domainUsageRequestId/);
  assert.match(source, /\(stats\.domain_total_count \|\| domainUsageItems\.length\) > 6/);
  assert.match(source, /disabled=\{domainUsageLoading\}/);
  assert.match(source, /on:click=\{toggleDomainUsageExpanded\}/);
  assert.doesNotMatch(source, /openAllDomainsDetail/);
  assert.match(source, /invoke(?:<[^>]+>)?\('get_overview_domain_detail'/);
  assert.match(source, /async function openDomainDetail\(domain(?:\s*:[^)]+)?\)/);
  assert.match(source, /const availableDomains = expandedDomainUsageItems\.length > 0/);
  assert.match(source, /browser_sources: item\.browser_sources/);
  assert.match(source, /stats\?\.domain_total_count \|\| availableDomains\.length/);
  assert.match(source, /use:trapFocus/);
  assert.match(source, /aria-labelledby="overview-domain-overlay-title"/);
  assert.match(source, /id="overview-domain-overlay-title"/);
  assert.match(source, /bind:this=\{domainOverlayDialog\}/);
  assert.match(source, /function focusDomainOverlayView\(\)/);
  assert.match(source, /domainOverlayDialog\?\.querySelector(?:<[^>]+>)?\('\[data-domain-summary\]'\)/);
  assert.match(source, /bind:this=\{domainOverlayBackButton\}/);
  assert.match(source, /data-domain-summary/);
  assert.match(source, /on:click\|self=\{closeDomainOverlay\}/);
});

test('概览总时长 KPI 应使用紧凑格式并保持单行', async () => {
  const [overviewSource, statsCardSource] = await Promise.all([
    readFile(new URL('./Overview.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../lib/components/StatsCard.svelte', import.meta.url), 'utf8'),
  ]);

  assert.match(overviewSource, /formatDurationLocalized\(stats\.total_duration, \{ compact: true \}\)/);
  assert.match(overviewSource, /formatDurationLocalized\(stats\.work_time_duration \|\| 0, \{ compact: true \}\)/);
  assert.match(statsCardSource, /whitespace-nowrap text-\[1\.9rem\]/);
});

test('小时应用明细只应接受最后一次日期范围请求的结果', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');
  const start = source.indexOf('async function loadHourlyBreakdown');
  const end = source.indexOf('// 切换日期时刷新 hourly 分类细分', start);
  const loadSource = source.slice(start, end);

  assert.match(source, /let hourlyBreakdownRequestId = 0;/);
  assert.match(loadSource, /const requestId = \+\+hourlyBreakdownRequestId;/);
  assert.match(loadSource, /const breakdown = await invoke(?:<[^>]+>)?\('get_hourly_app_breakdown'/);
  assert.match(loadSource, /if \(requestId === hourlyBreakdownRequestId\) \{\s*hourlyAppBreakdown = breakdown;\s*\}/);
  assert.match(loadSource, /catch \(e\) \{\s*if \(requestId === hourlyBreakdownRequestId\) \{\s*hourlyBreakdownError = formatUserError/);
  assert.doesNotMatch(loadSource, /hourlyAppBreakdown = \[\];/);
  assert.match(source, /hourlyBreakdownError/);
});

test('网站语义分类应使用含色点、名称与当前项勾选的紧凑 Popover', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /overview-semantic-popover/);
  assert.match(source, /role="dialog"/);
  assert.match(source, /overview-semantic-option/);
  assert.match(source, /overview-semantic-color-dot/);
  assert.match(source, /getSemanticCategoryColor\(cat\.key\)/);
  assert.match(source, /editingSemanticCategory === cat\.key[\s\S]*overview-semantic-check/);
  assert.match(source, /aria-pressed=\{editingSemanticCategory === cat\.key\}/);
  assert.match(source, /syncHistory:\s*true/);
  assert.match(source, /save_custom_semantic_category/);
  assert.match(source, /delete_custom_semantic_category/);
  assert.match(source, /startRenameSemanticCategory/);
  assert.match(source, /use:registerDomainSemanticTrigger=\{domain\.domain\}/);
  assert.match(source, /bind:this=\{semanticCategoryPopover\}/);
  assert.match(source, /semanticCategoryPopover\?\.focus\(\)/);
  assert.match(source, /domainSemanticTriggers\.get\(domainKey\)\?\.focus\(\)/);
  assert.match(source, /getViewportPopoverPlacement/);
  assert.match(source, /style=\{semanticPopoverStyle\}/);
  assert.match(source, /overview-semantic-popover fixed/);
});

test('网站语义分类弹层的同步说明应只显示一次', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');
  const helpTextOccurrences = source.match(/t\('overview\.semanticCategoryHelp'\)/g) ?? [];

  assert.equal(helpTextOccurrences.length, 1, '同步说明不应在弹层正文和操作栏重复显示');
});

test('概览页面应展示按小时活跃度柱状图', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /ActivityHourlyChart/);
  assert.match(source, /overview\.todayRhythm/);
  assert.match(source, /stats\.hourly_activity_distribution/);
  assert.match(source, /hourlyChartDistributionTitle/);
  assert.match(source, /hourlyChartDistributionSubtitleKey/);
  assert.match(source, /hourlyChart\.distributionTitleToday/);
  assert.match(source, /hourlyChart\.distributionTitleWeek/);
  assert.match(source, /hourlyChart\.distributionTitleRange/);
  // 2026-07 概览改版（有意变更）：按小时活跃度并入「节奏」主视觉卡，
  // 上移为 KPI 之下的第一视觉，应用投入退居下方双栏 —— 原顺序断言反转。
  assert.ok(
    source.indexOf('<ActivityHourlyChart') < source.indexOf("t('overview.appUsage')"),
    '改版后按小时活跃度（节奏卡）应位于应用使用模块上方'
  );
});

test('概览改版应提供可选择的分类构成条、分类摘要与按天投入', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /compositionTotals = stats\?\.category_usage/);
  assert.match(source, /compositionSegments/);
  assert.match(source, /let selectedCompositionCategory(?:\s*:[^=]+)?\s*= null/);
  assert.match(source, /buildCategoryCompositionSummary/);
  assert.match(source, /function toggleCompositionCategory\(category(?:\s*:[^)]+)?\)/);
  assert.match(source, /selectedCompositionCategory === category \? null : category/);
  assert.match(source, /overview-composition-segment/);
  assert.match(source, /aria-pressed=\{selectedCompositionCategory === segment\.category\}/);
  assert.match(source, /overview-composition-summary/);
  assert.match(source, /selectedCategory=\{selectedCompositionCategory\}/);
  assert.match(source, /function clearSelectedCompositionCategory\(\)/);

  for (const handlerName of ['setOverviewMode', 'handleOverviewDateChange', 'stepOverviewDateRange']) {
    const start = source.indexOf(`function ${handlerName}`);
    const end = source.indexOf('\n  function ', start + 1);
    assert.match(source.slice(start, end), /clearSelectedCompositionCategory\(\)/, `${handlerName} 应清除分类选择`);
  }

  assert.match(source, /invoke(?:<[^>]+>)?\('get_range_daily_totals'/);
  assert.match(source, /\{#if overviewMode !== 'today'\}/);
  assert.match(source, /overview\.dailyInvest/);
  assert.match(source, /overview\.heaviestDay/);
});

test('按天投入命令应注册为 get_range_daily_totals', async () => {
  const source = await readFile(new URL('../../src-tauri/src/main.rs', import.meta.url), 'utf8');

  assert.match(source, /commands::get_range_daily_totals/);
});

test('概览页面在不可见时应暂停时钟与定时刷新', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /document\.addEventListener\('visibilitychange'/);
  assert.match(
    source,
    /if\s*\(document\.hidden\)[\s\S]*clearInterval\(clockInterval\)[\s\S]*clearInterval\(refreshInterval\)/
  );
  assert.match(
    source,
    /else\s*\{[\s\S]*clockInterval\s*=\s*setInterval[\s\S]*refreshInterval\s*=\s*setInterval/
  );
  assert.match(source, /document\.removeEventListener\('visibilitychange'/);
});

test('概览页面在浏览器预览环境缺少 Tauri event metadata 时不应中断挂载', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /async function safeListen(?:<[^>]+>)?\([\s\S]*?eventName:\s*string,[\s\S]*?handler:/);
  assert.match(source, /return await listen\(eventName, handler\);/);
  assert.match(source, /return \(\) => \{\};/);
  assert.match(source, /await safeListen\('screenshot-taken'/);
  // 销毁后注册完成的监听器应被立即释放，避免泄漏
  assert.match(source, /if \(componentDestroyed\)/);
});

test('概览页面应支持今日、指定日期与本周三种时间视角切换', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /invoke(?:<[^>]+>)?\('get_overview_stats'/);
  assert.match(source, /overview\.modeToday/);
  assert.match(source, /overview\.modeDate/);
  assert.match(source, /overview\.modeWeek/);
  assert.match(source, /\{#if overviewMode === 'date'\}/);
  assert.match(source, /selectedDateFrom/);
  assert.match(source, /selectedDateTo/);
  assert.match(source, /LocalizedDatePicker/);
  assert.match(source, /mode="range"/);
  assert.match(source, /bind:startDate=\{selectedDateFrom\}/);
  assert.match(source, /bind:endDate=\{selectedDateTo\}/);
  assert.match(source, /stepOverviewDateRange/);
  assert.doesNotMatch(source, /commitOverviewDateInput/);
  assert.doesNotMatch(source, /overviewDateInputFrom/);
  assert.doesNotMatch(source, /overviewDateInputTo/);
  assert.doesNotMatch(source, /editingOverviewDateFrom/);
  assert.doesNotMatch(source, /editingOverviewDateTo/);
  assert.doesNotMatch(source, /inputmode="numeric"/);
  assert.match(source, /dateFrom: overviewMode === 'date'/);
  assert.match(source, /dateTo: overviewMode === 'date'/);
  const todayIndex = source.indexOf("setOverviewMode('today')");
  const weekIndex = source.indexOf("setOverviewMode('week')");
  const dateIndex = source.indexOf("setOverviewMode('date')");
  assert.ok(
    todayIndex < weekIndex && weekIndex < dateIndex,
    '顶部视角顺序应为 今日 -> 本周 -> 指定日期'
  );
});

test('概览页面的指定日期选择框应跟随当前语言并使用紧凑控件样式', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /overview-date-bar/);
  assert.match(source, /triggerClass="overview-date-trigger"/);
  assert.match(source, /localeCode=\{currentLocale\}/);
  assert.match(source, /max=\{getLocalDateString\(\)\}/);
  assert.doesNotMatch(source, /inlinePanel=\{true\}/);
  assert.match(source, /on:change=\{handleOverviewDateChange\}/);
  assert.match(source, /stepOverviewDateRange\(-1\)/);
  assert.match(source, /stepOverviewDateRange\(1\)/);
  assert.match(source, /disabled=\{!canStepOverviewDateForward\}/);
  assert.doesNotMatch(source, /stepOverviewDateBoundary/);
  assert.doesNotMatch(source, /overview-date-input/);
  assert.doesNotMatch(source, /overview-date-field/);
});

test('概览卡片标题应随今日、单日、范围和本周视角切换', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /overviewTotalActivityTitle/);
  assert.match(source, /overviewWorkDurationTitle/);
  assert.match(source, /overview\.totalActivityToday/);
  assert.match(source, /overview\.totalActivityDate/);
  assert.match(source, /overview\.totalActivityRange/);
  assert.match(source, /overview\.totalActivityWeek/);
  assert.match(source, /overview\.workDurationToday/);
  assert.match(source, /overview\.workDurationDate/);
  assert.match(source, /overview\.workDurationRange/);
  assert.match(source, /overview\.workDurationWeek/);
});

test('概览页只保留应用使用视图偏好，小时活跃度固定为竖向图', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /appUsageViewMode/);
  assert.match(source, /APP_USAGE_VIEW_MODE_KEY = 'overview\.appUsage\.viewMode'/);
  assert.match(source, /overview\.appUsageBar/);
  assert.match(source, /overview\.appUsageColumn/);
  assert.match(source, /readStoredOverviewViewMode/);
  assert.match(source, /persistOverviewViewMode/);
  assert.match(source, /localStorage\.getItem\(key\)/);
  assert.match(source, /localStorage\.setItem\(key, value\)/);
  assert.doesNotMatch(source, /hourlyActivityViewMode/);
  assert.doesNotMatch(source, /HOURLY_ACTIVITY_VIEW_MODE_KEY/);
  assert.doesNotMatch(source, /overview\.hourlyActivityBar/);
  assert.doesNotMatch(source, /overview\.hourlyActivityColumn/);
  assert.doesNotMatch(source, /mode=\{hourlyActivityViewMode\}/);
});

test('概览页小时应用明细应与当前概览日期范围保持同口径', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /getHourlyBreakdownRange\(\)/);
  assert.match(source, /dateFrom:\s*range\.dateFrom/);
  assert.match(source, /dateTo:\s*range\.dateTo/);
  assert.match(source, /mode:\s*overviewMode/);
});

test('概览页按小时活跃度分类图例应按当前语言翻译系统分类', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /translateCategoryLabel/);
  assert.match(source, /translatedCategoryName = translateCategoryLabel\(c\.key\)/);
  assert.match(source, /isKnownSystemCategory = c\.is_system \|\| translatedCategoryName !== c\.key/);
  assert.match(source, /acc\[c\.key\] = isKnownSystemCategory \? translatedCategoryName : \(c\.name \|\| translatedCategoryName\)/);
  assert.doesNotMatch(
    source,
    /hourlyCategoryNames = categoryList\.reduce\(\(acc, c\) => \{[\s\S]*acc\[c\.key\] = c\.name;/,
    '不能直接使用 get_categories 返回的中文系统分类名，否则英文/繁中切换后图例不会变'
  );
});

test('概览页网站访问弹层应按当前语言翻译内置语义分类', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /translatedSemanticCategoryName = translateSemanticCategoryLabel\(cat\.key\)/);
  assert.match(source, /isKnownSemanticCategory = cat\.is_system \|\| translatedSemanticCategoryName !== cat\.key/);
  assert.match(
    source,
    /return isKnownSemanticCategory \? translatedSemanticCategoryName : \(cat\.name \|\| translatedSemanticCategoryName\)/
  );
  assert.doesNotMatch(
    source,
    /return\s+cat\.name\s*\|\|\s*translateSemanticCategoryLabel\(cat\.key\)/,
    '网站语义分类选择器不能直接优先显示后端中文名称'
  );
});

test('概览页未识别页面域名标题应走 i18n 文案而不是直接渲染中文 sentinel', async () => {
  const source = await readFile(new URL('./Overview.svelte', import.meta.url), 'utf8');

  assert.match(source, /function getBrowserDomainDisplayLabel\(domain(?:\s*:[^)]+)?\)/);
  assert.match(source, /isUnresolvedBrowserDomain\(domain\) \? t\('overview\.unresolvedPage'\) : domain\.domain/);
  assert.match(source, /\{getBrowserDomainDisplayLabel\(domain\)\}/);
  assert.doesNotMatch(
    source,
    /<span class="font-medium text-slate-700 dark:text-\[#c9d1d9\]">\{domain\.domain\}<\/span>/,
    '英文界面不能把后端未识别页面 sentinel 直接显示成中文'
  );
});

test('概览统计命令应注册为 get_overview_stats', async () => {
  const source = await readFile(new URL('../../src-tauri/src/main.rs', import.meta.url), 'utf8');

  assert.match(source, /commands::get_overview_stats/);
});
