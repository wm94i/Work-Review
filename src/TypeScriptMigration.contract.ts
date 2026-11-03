import type { Readable, Writable } from 'svelte/store';
import {
  buildHistoryPayload,
  buildStepDigestForHistory,
  summarizeStepsForHistory,
  type HistoryPayloadEntry,
} from './routes/ask/historyPayload.ts';
import { resolveModelOptionLabel } from './routes/ask/modelPresentation.ts';
import {
  createRequestEventGate,
  type RequestEventGate,
  type RequestEventGateOptions,
} from './routes/ask/requestEventGate.ts';
import {
  selectStarterPrompts,
  type StarterPromptOptions,
} from './routes/ask/starterPromptPresentation.ts';
import {
  reduceStreamEvent,
  type StreamEventResult,
  type StreamMessage,
} from './routes/ask/streamEvent.ts';
import { formatBubbleMessage } from './lib/components/Avatar/bubbleMessage.ts';
import { formatUserError } from './lib/utils/errorDisplay.ts';
import {
  prepareTimelineActivities,
  type TimelineActivity,
  upsertTimelineActivity,
} from './routes/timeline/timelineData.ts';
import {
  createReportGenerationOwnership,
  createReportRequestSnapshot,
  type ReportGenerationOwnership,
  type ReportRequestSnapshot,
  shiftIsoDate,
} from './routes/report/reportDateNavigation.ts';
import {
  type ReportConfigMetaInput,
  type ReportMetaInput,
  type ResolvedReportMeta,
  resolveReportMeta,
} from './routes/report/reportMeta.ts';
import {
  type PromptAppliedToastInput,
  shouldShowPromptAppliedToast,
} from './routes/report/reportPromptFeedback.ts';
import {
  formatHourRange,
  getFullSummary,
  getMainApps,
  getPrimarySummary,
  getSecondarySummary,
  getSummaryDisplayParts,
  getSummaryRhythmTone,
  type HourSummaryOrderable,
  orderHourlySummariesForDisplay,
  type SummaryDisplayParts,
  type SummaryRhythmTone,
} from './routes/timeline/summaryPresentation.ts';
import {
  extractReportBlockName,
  getVisibleReportSections,
  parseReportSections,
  type ReportSection,
  type ReportSectionText,
  reportSectionMarkdownForDisplay,
  reportSectionMarkdownForStorage,
  type VisibleReportSection,
} from './routes/report/reportSections.ts';
import {
  buildCategoryCompositionSummary,
  type CategoryCompositionSummary,
  type CategoryCompositionSummaryOptions,
} from './routes/overviewCategoryPresentation.ts';
import {
  type BrowserUsageInput,
  buildDomainPresentation,
  buildDomainSourceTrack,
  collectDomainBrowserSources,
  type DomainBrowserSource,
  type DomainBrowserSourceInput,
  type DomainPresentation,
  type DomainPresentationInput,
  type DomainSourceTrackItem,
  getSemanticCategoryColor,
} from './routes/overviewDomainPresentation.ts';
import {
  type AvatarActionLoopMeta,
  type AvatarBubbleLocale,
  type AvatarIdleMotionMeta,
  type AvatarMode,
  type AvatarModeMeta,
  type AvatarPersona,
  type AvatarStateBubble,
  type AvatarTransitionMeta,
  getAvatarActionLoopMeta,
  getAvatarIdleMotionMeta,
  getAvatarModeMeta,
  getAvatarMotionStepDelay,
  getAvatarStateBubble,
  getAvatarTransitionMeta,
} from './lib/components/Avatar/avatarStateMeta.ts';
import {
  AVATAR_PRESET_DEFAULT,
  AVATAR_PRESET_OPTIONS,
  type AvailableAvatarPresetId,
  type AvatarPresetDefinition,
  type AvatarPresetOption,
  getAvatarPresetDefinition,
  getAvatarPresetOption,
  normalizeAvatarPresetId,
} from './lib/components/Avatar/avatarPresetRegistry.ts';
import {
  AVATAR_OPACITY_DEFAULT,
  AVATAR_OPACITY_MAX,
  AVATAR_OPACITY_MIN,
  AVATAR_SCALE_DEFAULT,
  AVATAR_SCALE_MAX,
  AVATAR_SCALE_MIN,
  type AvatarConfigSaver,
  type AvatarSettingsConfig,
  type AvatarToggleUiState,
  clampAvatarOpacity,
  clampAvatarScale,
  formatAvatarOpacityLabel,
  formatAvatarScaleLabel,
  getAvatarToggleToast,
  getAvatarToggleUiState,
  toggleAvatarSetting,
  updateAvatarOpacitySetting,
  updateAvatarScaleSetting,
} from './lib/utils/avatarToggle.ts';
import {
  type FocusTrapActionResult,
  trapFocus,
} from './lib/utils/focusTrap.ts';
import {
  isActiveRecording,
  type RecordingState,
  type RecordingStateInput,
  type RecordingStore,
  recordingStore,
} from './lib/stores/recording.ts';
import {
  clearToast,
  showToast,
  type ToastState,
  type ToastStore,
  type ToastType,
  toast,
} from './lib/stores/toast.ts';
import {
  confirm as openConfirm,
  type ConfirmDialogState,
  type ConfirmDialogStore,
  type ConfirmOptions,
  type ConfirmTone,
  confirmDialog,
  resolveConfirm,
} from './lib/stores/confirm.ts';
import {
  type AiConfigInput,
  type AiStore,
  type AiStoreState,
  type AiTestStatus,
  type AiTextModelConfigInput,
  aiStore,
} from './lib/stores/ai.ts';
import {
  type CategoryInfo,
  type CategoryMeta,
  type CategoryStore,
  categoryStore,
  hexToRGBA,
  type SemanticCategoryInfo,
  type SemanticCategoryStore,
  semanticCategoryStore,
} from './lib/stores/categories.ts';
import {
  type AssistantCard,
  type AssistantConfirmStatus,
  type AssistantMessage,
  type AssistantMessageInput,
  type AssistantMessageRole,
  type AssistantMessageUpdater,
  type AssistantModelSelectionOptions,
  type AssistantReference,
  type AssistantState,
  type AssistantStep,
  type AssistantStepStatus,
  type AssistantStore,
  assistantStore,
  BASIC_ASSISTANT_MODEL_ID,
} from './lib/stores/assistant.ts';
import {
  createUpdateFlow,
  type GithubUpdateInfo,
  type GithubUpdateInstallResult,
  type GithubUpdateStatusPayload,
  type RunUpdateFlow,
  type RunUpdateFlowOptions,
  type RunUpdateFlowResult,
  runUpdateFlow,
  type UpdateFlowDependencies,
} from './lib/utils/updater.ts';
import {
  cache,
  type CacheActivity,
  type CacheEntry,
  type CacheInvalidationType,
  type CacheValidityKey,
  type CacheState,
  type CacheStore,
  getLocalDate,
  type OverviewCacheEntry,
  type TimelineCacheEntry,
} from './lib/stores/cache.ts';
import {
  appIconStore,
  type AppIconCacheState,
  type AppIconCacheValue,
  type AppIconInvoke,
  type AppIconLoadOptions,
  type AppIconRequest,
  type AppIconRequestEntry,
  type AppIconStore,
  type GetAppIconArgs,
  getIconCacheKey,
  loadAppIcon,
  preloadAppIcons,
} from './lib/stores/iconCache.ts';
import {
  applyLocaleToDocument,
  cycleLocale,
  type DurationFormatOptions,
  formatDurationLocalized,
  formatLocalizedDate,
  formatLocalizedTime,
  getLocaleLabel,
  getLocaleShortLabel,
  initializeLocale,
  type InterpolationParams,
  type Locale,
  locale as localeStore,
  setLocale,
  SUPPORTED_LOCALES,
  t as translate,
  tm as translateMessages,
  type TranslationDictionary,
  type TranslationValue,
  translateCategoryLabel,
  translateSemanticCategoryLabel,
} from './lib/i18n/index.ts';

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2)
    ? true
    : false;

type Expect<Value extends true> = Value;

type ExpectedAvatarSettingsConfig = {
  avatar_enabled?: unknown;
  break_reminder_enabled?: unknown;
  avatar_scale?: unknown;
  avatar_opacity?: unknown;
};

type ExpectedAvatarToggleUiState = {
  trackClass: string;
  thumbClass: string;
  buttonClass: string;
  ariaLabel: string;
};

type ExpectedFocusTrapActionResult = {
  destroy(): void;
};

type ExpectedRecordingStateInput = {
  isRecording?: unknown;
  isPaused?: unknown;
};

type ExpectedRecordingState = {
  isRecording: boolean;
  isPaused: boolean;
};

type ExpectedRecordingStore = {
  subscribe: Readable<ExpectedRecordingState>['subscribe'];
  set: Writable<ExpectedRecordingState>['set'];
  setState: (isRecording: unknown, isPaused: unknown) => void;
  reset: () => void;
};

type ExpectedToastType = 'info' | 'success' | 'error' | 'warning';

type ExpectedToastState = {
  id: number;
  message: string;
  type: ExpectedToastType;
};

type ExpectedToastStore = Readable<ExpectedToastState | null>;

type ExpectedConfirmTone = 'info' | 'warning' | 'error';

type ExpectedConfirmOptions = {
  title?: string;
  message?: string;
  confirmText?: string;
  cancelText?: string;
  tone?: ExpectedConfirmTone;
};

type ExpectedConfirmDialogState = {
  id: number;
  title: string;
  message: string;
  confirmText: string;
  cancelText: string;
  tone: ExpectedConfirmTone;
};

type ExpectedConfirmDialogStore = Readable<ExpectedConfirmDialogState | null>;

type ExpectedAiTestStatus = null | 'testing' | 'success' | 'error';

type ExpectedAiTextModelConfigInput = {
  provider: string;
  endpoint: string;
  model: string;
  api_key?: string | null;
};

type ExpectedAiConfigInput = {
  text_model?: ExpectedAiTextModelConfigInput | null;
};

type ExpectedAiStoreState = {
  textTestStatus: ExpectedAiTestStatus;
  textTestMessage: string;
  textConnectionVerified: boolean;
  lastTestedConfigHash: string | null;
};

type ExpectedAiStore = {
  subscribe: Readable<ExpectedAiStoreState>['subscribe'];
  startTesting: () => void;
  setSuccess: (message: string) => void;
  setError: (message: string) => void;
  reset: () => void;
  setConfigHash: (hash: string | null) => void;
  getConfigHash: (config?: ExpectedAiConfigInput | null) => string | null;
};

type ExpectedCategoryInfo = {
  key: string;
  name: string;
  color: string;
  icon: string;
  is_system: boolean;
};

type ExpectedCategoryMeta = {
  color: string;
  icon: string;
  name: string;
  isSystem: boolean;
};

type ExpectedCategoryStore = {
  subscribe: Readable<ExpectedCategoryInfo[]>['subscribe'];
  set: Writable<ExpectedCategoryInfo[]>['set'];
  update: Writable<ExpectedCategoryInfo[]>['update'];
  refresh: () => Promise<void>;
  getCategoryMeta: (key: string) => ExpectedCategoryMeta;
  getAllCategories: () => ExpectedCategoryInfo[];
};

type ExpectedSemanticCategoryInfo = {
  key: string;
  name: string;
  is_system: boolean;
};

type ExpectedSemanticCategoryStore = {
  subscribe: Readable<ExpectedSemanticCategoryInfo[]>['subscribe'];
  set: Writable<ExpectedSemanticCategoryInfo[]>['set'];
  refresh: () => Promise<void>;
  getSemanticCategoryDisplayName: (key: string) => string;
  getAllSemanticCategories: () => ExpectedSemanticCategoryInfo[];
};

type ExpectedAssistantMessageRole = 'user' | 'assistant';
type ExpectedAssistantStepStatus = 'running' | 'done';
type ExpectedAssistantConfirmStatus = 'pending' | 'approved' | 'denied' | 'rejected';

type ExpectedAssistantReference = {
  sourceType: string;
  sourceId: number | null;
  date: string;
  timestamp: number;
  title: string;
  excerpt: string;
  appName: string | null;
  browserUrl: string | null;
  duration: number | null;
  score: number;
  [key: string]: unknown;
};

type ExpectedAssistantCard = Record<string, unknown>;

type ExpectedAssistantStep = {
  tool?: string;
  label?: string;
  status?: ExpectedAssistantStepStatus;
  ok?: boolean;
  hits?: number;
  references?: ExpectedAssistantReference[];
  digest?: string;
  confirmId?: string;
  summary?: string;
  confirmStatus?: ExpectedAssistantConfirmStatus;
  [key: string]: unknown;
};

type ExpectedAssistantMessageInput = {
  id?: string;
  role?: ExpectedAssistantMessageRole;
  content?: string | null;
  cards?: unknown[];
  references?: unknown[];
  toolLabels?: unknown[];
  steps?: unknown[];
  streaming?: boolean;
  usedAi?: boolean;
  failed?: boolean;
  stopped?: boolean;
  modelName?: string;
  [key: string]: unknown;
};

type ExpectedAssistantMessage = {
  id: string;
  role?: ExpectedAssistantMessageRole;
  content?: string | null;
  cards: ExpectedAssistantCard[];
  references: ExpectedAssistantReference[];
  toolLabels: string[];
  steps: ExpectedAssistantStep[];
  streaming: boolean;
  usedAi?: boolean;
  failed?: boolean;
  stopped?: boolean;
  modelName?: string;
  [key: string]: unknown;
};

type ExpectedAssistantState = {
  messages: ExpectedAssistantMessage[];
  selectedModelId: string;
  hasUserSelectedModel: boolean;
  sending: boolean;
  sendingRequestId: string | null;
  conversationId: number | null;
};

type ExpectedAssistantMessageUpdater = (
  message: ExpectedAssistantMessage,
) => ExpectedAssistantMessageInput;

type ExpectedAssistantModelSelectionOptions = {
  userInitiated?: boolean;
};

type ExpectedAssistantStore = {
  subscribe: Readable<ExpectedAssistantState>['subscribe'];
  appendMessage: (message: ExpectedAssistantMessageInput) => void;
  clearMessages: () => void;
  setSelectedModelId: (
    selectedModelId: unknown,
    options?: ExpectedAssistantModelSelectionOptions,
  ) => void;
  setMessages: (
    messages: ExpectedAssistantMessageInput[] | null | undefined,
  ) => void;
  setConversation: (
    conversationId: number | null | undefined,
    messages: ExpectedAssistantMessageInput[] | null | undefined,
  ) => void;
  setConversationId: (conversationId: number | null | undefined) => void;
  beginSending: (requestId: string) => void;
  finishSending: (requestId: string) => void;
  updateLastStreaming: (updater: ExpectedAssistantMessageUpdater) => void;
  updateMessageById: (
    messageId: string,
    updater: ExpectedAssistantMessageUpdater,
  ) => void;
  reset: () => void;
};

type ExpectedGithubUpdateInfo = {
  currentVersion: string;
  latestVersion: string;
  available: boolean;
  autoUpdateReady: boolean;
  prerelease: boolean;
  releaseUrl: string;
  body: string | null;
  source: string | null;
};

type ExpectedGithubUpdateInstallResult = {
  updated: boolean;
  available: boolean;
  version: string | null;
  source: string | null;
  message: string;
  attemptedSources: string[];
};

type ExpectedGithubUpdateStatusPayload = {
  stage: string;
  message: string;
  source: string | null;
  version: string | null;
  downloadedBytes: number | null;
  totalBytes: number | null;
  percent: number | null;
};

type ExpectedRunUpdateFlowOptions = {
  silentWhenUpToDate?: boolean;
  confirmBeforeDownload?: boolean;
  onStatusChange?: (status: string) => void;
};

type ExpectedRunUpdateFlowResult =
  | { skipped: true; reason: 'in-flight' }
  | { updated: false; available: false }
  | {
      updated: false;
      available: true;
      autoUpdateReady: false;
      releaseUrl: string;
    }
  | { updated: false; cancelled: true }
  | { updated: true; handoffToInstaller: true }
  | { updated: true }
  | { updated: false; error: string };

type ExpectedRunUpdateFlow = (
  options?: ExpectedRunUpdateFlowOptions,
) => Promise<ExpectedRunUpdateFlowResult>;

type ExpectedUpdateFlowDependencies = {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  listen: <T>(
    event: string,
    handler: (event: { payload: T }) => void,
  ) => Promise<() => void>;
  relaunch: () => Promise<void>;
  open: (url: string) => Promise<void>;
  confirm: (options?: ExpectedConfirmOptions) => Promise<boolean>;
  showToast: (
    message: unknown,
    type?: ExpectedToastType,
    duration?: number,
  ) => void;
  translate: (key: string, params?: Record<string, unknown>) => string;
  warn: (message: string, error: unknown) => void;
  error: (message: string, error: unknown) => void;
};

type ExpectedCacheActivity = {
  readonly id: number | null;
};

type ExpectedCacheEntry = {
  data: unknown;
  timestamp: number;
};

type ExpectedOverviewCacheEntry = {
  data: unknown | null;
  timestamp: number;
  loading: boolean;
  date: string | null;
};

type ExpectedTimelineCacheEntry = {
  data: ExpectedCacheActivity[];
  timestamp: number;
  summaries: unknown[];
};

type ExpectedCacheState = {
  overview: ExpectedOverviewCacheEntry;
  timeline: Record<string, ExpectedTimelineCacheEntry>;
  reports: Record<string, ExpectedCacheEntry>;
  hourlySummaries: Record<string, unknown>;
  reportGenerating: boolean;
  config: unknown | null;
};

type ExpectedCacheInvalidationType = 'overview' | 'timeline' | 'report';
type ExpectedCacheValidityKey = 'overview' | 'timeline' | 'reports';

type ExpectedCacheStore = {
  subscribe: Readable<ExpectedCacheState>['subscribe'];
  isValid(state: ExpectedCacheState, key: 'overview'): boolean;
  isValid(
    entry: ExpectedCacheEntry | null | undefined,
    key?: ExpectedCacheValidityKey | null,
  ): boolean;
  setOverview: (data: unknown) => void;
  setTimeline: (
    date: string,
    data: ExpectedCacheActivity[],
    summaries: unknown[],
  ) => void;
  setReport: (date: string, data: unknown) => void;
  setReportGenerating: (generating: boolean) => void;
  setConfig: (data: unknown | null) => void;
  addActivity: (activity: ExpectedCacheActivity) => void;
  clear: () => void;
  invalidate: (
    type: ExpectedCacheInvalidationType,
    date?: string | null,
  ) => void;
};

type ExpectedAppIconCacheValue = string | null;
type ExpectedAppIconCacheState = Partial<
  Record<string, ExpectedAppIconCacheValue>
>;

type ExpectedAppIconRequestEntry = {
  appName?: string | null;
  app_name?: string | null;
  browserName?: string | null;
  browser_name?: string | null;
  executablePath?: string | null;
  executable_path?: string | null;
};

type ExpectedAppIconRequest =
  | string
  | ExpectedAppIconRequestEntry
  | null
  | undefined;

type ExpectedAppIconLoadOptions = {
  priority?: boolean;
};

type ExpectedGetAppIconArgs = {
  appName: string;
  executablePath: string | null;
};

type ExpectedAppIconInvoke = (
  command: 'get_app_icon',
  args: ExpectedGetAppIconArgs,
) => Promise<string>;

type ExpectedAppIconStore = Writable<ExpectedAppIconCacheState>;

type ExpectedLocale = 'zh-CN' | 'en' | 'zh-TW' | 'ar';

type ExpectedTranslationValue =
  | string
  | string[]
  | ExpectedTranslationDictionary;

interface ExpectedTranslationDictionary {
  [key: string]: ExpectedTranslationValue;
}

type ExpectedInterpolationParams = Readonly<Record<string, unknown>>;

type ExpectedDurationFormatOptions = {
  compact?: boolean;
};

type ExpectedLocaleStore = Writable<ExpectedLocale>;

interface NamedAvatarSettingsConfig {
  avatar_enabled: boolean;
  break_reminder_enabled: boolean;
  avatar_scale: number;
  avatar_opacity: number;
  marker: string;
}

declare const namedAvatarSettingsConfig: NamedAvatarSettingsConfig;
declare const saveNamedAvatarSettingsConfig: AvatarConfigSaver<NamedAvatarSettingsConfig>;

void toggleAvatarSetting(namedAvatarSettingsConfig, saveNamedAvatarSettingsConfig);
void updateAvatarScaleSetting(namedAvatarSettingsConfig, 1, saveNamedAvatarSettingsConfig);
void updateAvatarOpacitySetting(namedAvatarSettingsConfig, 1, saveNamedAvatarSettingsConfig);

type HistoryBuilderContract = Expect<Equal<
  typeof buildHistoryPayload,
  (messages: unknown) => HistoryPayloadEntry[]
>>;

type HistorySummaryContract = Expect<Equal<
  typeof summarizeStepsForHistory,
  (steps: unknown) => string | null
>>;

type HistoryDigestContract = Expect<Equal<
  typeof buildStepDigestForHistory,
  (steps: unknown) => string | null
>>;

type ModelLabelContract = Expect<Equal<
  typeof resolveModelOptionLabel,
  (
    selectedModelId: unknown,
    modelProfiles: unknown,
    locale: unknown,
    translate: unknown,
  ) => string
>>;

type RequestGateContract = Expect<Equal<
  typeof createRequestEventGate,
  <TEvent>(options: RequestEventGateOptions<TEvent>) => RequestEventGate<TEvent>
>>;

type StarterPromptContract = Expect<Equal<
  typeof selectStarterPrompts,
  (options?: StarterPromptOptions) => string[]
>>;

type StreamReducerContract = Expect<Equal<
  typeof reduceStreamEvent,
  (
    message: StreamMessage,
    event: unknown,
    fallbackError?: string,
  ) => StreamEventResult
>>;

type BubbleMessageContract = Expect<Equal<
  typeof formatBubbleMessage,
  (message: unknown) => string
>>;

type ErrorDisplayContract = Expect<Equal<
  typeof formatUserError,
  (error: unknown, fallback: string) => string
>>;

type TimelinePreparationContract = Expect<Equal<
  typeof prepareTimelineActivities,
  (activitiesData: readonly TimelineActivity[]) => TimelineActivity[]
>>;

type TimelineUpsertContract = Expect<Equal<
  typeof upsertTimelineActivity,
  (
    currentActivities: readonly TimelineActivity[],
    newActivity: TimelineActivity,
  ) => TimelineActivity[]
>>;

type ReportDateShiftContract = Expect<Equal<
  typeof shiftIsoDate,
  (dateValue: string, offsetDays: number) => string
>>;

type ReportSnapshotContract = Expect<Equal<
  typeof createReportRequestSnapshot,
  (
    requestId: number,
    selectedDate: string,
    locale: string,
  ) => ReportRequestSnapshot
>>;

type ReportOwnershipContract = Expect<Equal<
  typeof createReportGenerationOwnership,
  () => ReportGenerationOwnership
>>;

type ReportMetaContract = Expect<Equal<
  typeof resolveReportMeta,
  (
    reportData: ReportMetaInput | null | undefined,
    currentConfig: ReportConfigMetaInput | null | undefined,
  ) => ResolvedReportMeta
>>;

type ReportPromptFeedbackContract = Expect<Equal<
  typeof shouldShowPromptAppliedToast,
  (input: PromptAppliedToastInput) => boolean
>>;

type SummaryOrderContract = Expect<Equal<
  typeof orderHourlySummariesForDisplay,
  <T extends HourSummaryOrderable>(summaries?: readonly T[] | null) => T[]
>>;

type SummaryHourRangeContract = Expect<Equal<
  typeof formatHourRange,
  (hour?: number | string | null) => string
>>;

type SummaryTextContract = Expect<Equal<
  typeof getFullSummary,
  (text?: string | null) => string
>>;

type SummaryPrimaryContract = Expect<Equal<
  typeof getPrimarySummary,
  (text?: string | null) => string
>>;

type SummarySecondaryContract = Expect<Equal<
  typeof getSecondarySummary,
  (text?: string | null) => string
>>;

type SummaryPartsContract = Expect<Equal<
  typeof getSummaryDisplayParts,
  (text?: string | null, expanded?: boolean) => SummaryDisplayParts
>>;

type SummaryAppsContract = Expect<Equal<
  typeof getMainApps,
  (mainApps?: string | null) => string[]
>>;

type SummaryRhythmContract = Expect<Equal<
  typeof getSummaryRhythmTone,
  (totalDuration?: number) => SummaryRhythmTone
>>;

type ReportSectionsParseContract = Expect<Equal<
  typeof parseReportSections,
  (content?: string | null) => ReportSection[]
>>;

type ReportBlockNameContract = Expect<Equal<
  typeof extractReportBlockName,
  (section?: ReportSectionText | null) => string | null
>>;

type ReportVisibleSectionsContract = Expect<Equal<
  typeof getVisibleReportSections,
  (
    sections: readonly ReportSection[],
    pinnedBlocks?: readonly string[],
    hiddenBlocks?: readonly string[],
  ) => VisibleReportSection[]
>>;

type ReportDisplayMarkdownContract = Expect<Equal<
  typeof reportSectionMarkdownForDisplay,
  (
    section: ReportSectionText | null | undefined,
    visibleIndex: number,
    localeCode: string,
  ) => string
>>;

type ReportStorageMarkdownContract = Expect<Equal<
  typeof reportSectionMarkdownForStorage,
  (section?: ReportSectionText | null) => string
>>;

type OverviewCategoryContract = Expect<Equal<
  typeof buildCategoryCompositionSummary,
  (options?: CategoryCompositionSummaryOptions) => CategoryCompositionSummary
>>;

type DomainSourcesContract = Expect<Equal<
  typeof collectDomainBrowserSources,
  (
    domainName: unknown,
    browserUsage?: readonly (BrowserUsageInput | null | undefined)[] | null,
    explicitSources?: readonly (DomainBrowserSourceInput | null | undefined)[] | null,
  ) => DomainBrowserSource[]
>>;

type DomainTrackContract = Expect<Equal<
  typeof buildDomainSourceTrack,
  (
    browserSources?: readonly (DomainBrowserSourceInput | null | undefined)[] | null,
    domainDuration?: unknown,
  ) => DomainSourceTrackItem[]
>>;

type DomainPresentationContract = Expect<Equal<
  typeof buildDomainPresentation,
  (
    domain?: DomainPresentationInput | null,
    browserUsage?: readonly (BrowserUsageInput | null | undefined)[] | null,
  ) => DomainPresentation
>>;

type SemanticCategoryColorContract = Expect<Equal<
  typeof getSemanticCategoryColor,
  (categoryKey: unknown) => string
>>;

type AvatarModeMetaContract = Expect<Equal<
  typeof getAvatarModeMeta,
  (mode: AvatarMode, contextLabel?: string) => AvatarModeMeta
>>;

type AvatarStateBubbleContract = Expect<Equal<
  typeof getAvatarStateBubble,
  (
    mode: AvatarMode,
    locale?: AvatarBubbleLocale,
    contextLabel?: string,
    persona?: AvatarPersona,
  ) => AvatarStateBubble | null
>>;

type AvatarActionLoopContract = Expect<Equal<
  typeof getAvatarActionLoopMeta,
  (mode: AvatarMode, contextLabel?: string, beat?: number) => AvatarActionLoopMeta
>>;

type AvatarMotionDelayContract = Expect<Equal<
  typeof getAvatarMotionStepDelay,
  (mode: AvatarMode, contextLabel?: string, beat?: number) => number
>>;

type AvatarTransitionContract = Expect<Equal<
  typeof getAvatarTransitionMeta,
  (
    fromMode: AvatarMode,
    toMode: AvatarMode,
    fromContextLabel?: string,
    toContextLabel?: string,
  ) => AvatarTransitionMeta
>>;

type AvatarIdleMotionContract = Expect<Equal<
  typeof getAvatarIdleMotionMeta,
  (mode: AvatarMode, contextLabel?: string, beat?: number) => AvatarIdleMotionMeta
>>;

type AvatarPresetDefaultContract = Expect<Equal<
  typeof AVATAR_PRESET_DEFAULT,
  'original-standard'
>>;

type AvatarPresetOptionsContract = Expect<Equal<
  typeof AVATAR_PRESET_OPTIONS,
  readonly AvatarPresetOption[]
>>;

type AvatarPresetNormalizeContract = Expect<Equal<
  typeof normalizeAvatarPresetId,
  (presetId: string) => AvailableAvatarPresetId
>>;

type AvatarPresetDefinitionContract = Expect<Equal<
  typeof getAvatarPresetDefinition,
  (presetId: string) => AvatarPresetDefinition
>>;

type AvatarPresetOptionContract = Expect<Equal<
  typeof getAvatarPresetOption,
  (presetId: string) => AvatarPresetOption
>>;

type AvatarScaleMinContract = Expect<Equal<typeof AVATAR_SCALE_MIN, 0.4>>;
type AvatarScaleMaxContract = Expect<Equal<typeof AVATAR_SCALE_MAX, 1.3>>;
type AvatarScaleDefaultContract = Expect<Equal<typeof AVATAR_SCALE_DEFAULT, 0.9>>;
type AvatarOpacityMinContract = Expect<Equal<typeof AVATAR_OPACITY_MIN, 0.45>>;
type AvatarOpacityMaxContract = Expect<Equal<typeof AVATAR_OPACITY_MAX, 1>>;
type AvatarOpacityDefaultContract = Expect<Equal<typeof AVATAR_OPACITY_DEFAULT, 0.82>>;

type AvatarSettingsConfigContract = Expect<Equal<
  AvatarSettingsConfig,
  ExpectedAvatarSettingsConfig
>>;

type AvatarConfigSaverContract = Expect<Equal<
  AvatarConfigSaver<{ marker: string }>,
  (config: { marker: string }) => void | PromiseLike<void>
>>;

type AvatarToggleUiStateContract = Expect<Equal<
  AvatarToggleUiState,
  ExpectedAvatarToggleUiState
>>;

type AvatarToggleToastContract = Expect<Equal<
  typeof getAvatarToggleToast,
  (enabled: boolean) => string
>>;

type AvatarToggleUiContract = Expect<Equal<
  typeof getAvatarToggleUiState,
  (enabled: boolean, saving?: boolean) => ExpectedAvatarToggleUiState
>>;

type AvatarToggleSettingContract = Expect<Equal<
  typeof toggleAvatarSetting,
  <TConfig extends ExpectedAvatarSettingsConfig>(
    config: TConfig,
    saveConfig: (config: TConfig) => void | PromiseLike<void>,
  ) => Promise<boolean>
>>;

type AvatarScaleClampContract = Expect<Equal<
  typeof clampAvatarScale,
  (value: unknown) => number
>>;

type AvatarScaleLabelContract = Expect<Equal<
  typeof formatAvatarScaleLabel,
  (value: unknown) => string
>>;

type AvatarScaleUpdateContract = Expect<Equal<
  typeof updateAvatarScaleSetting,
  <TConfig extends ExpectedAvatarSettingsConfig>(
    config: TConfig,
    nextScale: unknown,
    saveConfig: (config: TConfig) => void | PromiseLike<void>,
  ) => Promise<number>
>>;

type AvatarOpacityClampContract = Expect<Equal<
  typeof clampAvatarOpacity,
  (value: unknown) => number
>>;

type AvatarOpacityLabelContract = Expect<Equal<
  typeof formatAvatarOpacityLabel,
  (value: unknown) => string
>>;

type AvatarOpacityUpdateContract = Expect<Equal<
  typeof updateAvatarOpacitySetting,
  <TConfig extends ExpectedAvatarSettingsConfig>(
    config: TConfig,
    nextOpacity: unknown,
    saveConfig: (config: TConfig) => void | PromiseLike<void>,
  ) => Promise<number>
>>;

type FocusTrapActionResultContract = Expect<Equal<
  FocusTrapActionResult,
  ExpectedFocusTrapActionResult
>>;
type FocusTrapContract = Expect<Equal<
  typeof trapFocus,
  (node: HTMLElement) => ExpectedFocusTrapActionResult
>>;

type RecordingStateInputContract = Expect<Equal<
  RecordingStateInput,
  ExpectedRecordingStateInput
>>;
type RecordingStateContract = Expect<Equal<RecordingState, ExpectedRecordingState>>;
type RecordingStoreContract = Expect<Equal<RecordingStore, ExpectedRecordingStore>>;
type RecordingStoreValueContract = Expect<Equal<
  typeof recordingStore,
  ExpectedRecordingStore
>>;
type RecordingActiveContract = Expect<Equal<
  typeof isActiveRecording,
  (state?: ExpectedRecordingStateInput | null) => boolean
>>;

type ToastTypeContract = Expect<Equal<ToastType, ExpectedToastType>>;
type ToastStateContract = Expect<Equal<ToastState, ExpectedToastState>>;
type ToastStoreContract = Expect<Equal<ToastStore, ExpectedToastStore>>;
type ToastStoreValueContract = Expect<Equal<typeof toast, ExpectedToastStore>>;
type ToastShowContract = Expect<Equal<
  typeof showToast,
  (message: unknown, type?: ExpectedToastType, duration?: number) => void
>>;
type ToastClearContract = Expect<Equal<typeof clearToast, () => void>>;

type ConfirmToneContract = Expect<Equal<ConfirmTone, ExpectedConfirmTone>>;
type ConfirmOptionsContract = Expect<Equal<ConfirmOptions, ExpectedConfirmOptions>>;
type ConfirmDialogStateContract = Expect<Equal<
  ConfirmDialogState,
  ExpectedConfirmDialogState
>>;
type ConfirmDialogStoreContract = Expect<Equal<
  ConfirmDialogStore,
  ExpectedConfirmDialogStore
>>;
type ConfirmStoreValueContract = Expect<Equal<
  typeof confirmDialog,
  ExpectedConfirmDialogStore
>>;
type ConfirmOpenContract = Expect<Equal<
  typeof openConfirm,
  (options?: ExpectedConfirmOptions) => Promise<boolean>
>>;
type ConfirmResolveContract = Expect<Equal<typeof resolveConfirm, (result: boolean) => void>>;

type AiTestStatusContract = Expect<Equal<AiTestStatus, ExpectedAiTestStatus>>;
type AiTextModelConfigInputContract = Expect<Equal<
  AiTextModelConfigInput,
  ExpectedAiTextModelConfigInput
>>;
type AiConfigInputContract = Expect<Equal<AiConfigInput, ExpectedAiConfigInput>>;
type AiStoreStateContract = Expect<Equal<AiStoreState, ExpectedAiStoreState>>;
type AiStoreContract = Expect<Equal<AiStore, ExpectedAiStore>>;
type AiStoreValueContract = Expect<Equal<typeof aiStore, ExpectedAiStore>>;

type CategoryInfoContract = Expect<Equal<CategoryInfo, ExpectedCategoryInfo>>;
type CategoryMetaContract = Expect<Equal<CategoryMeta, ExpectedCategoryMeta>>;
type CategoryStoreContract = Expect<Equal<CategoryStore, ExpectedCategoryStore>>;
type CategoryStoreValueContract = Expect<Equal<
  typeof categoryStore,
  ExpectedCategoryStore
>>;
type SemanticCategoryInfoContract = Expect<Equal<
  SemanticCategoryInfo,
  ExpectedSemanticCategoryInfo
>>;
type SemanticCategoryStoreContract = Expect<Equal<
  SemanticCategoryStore,
  ExpectedSemanticCategoryStore
>>;
type SemanticCategoryStoreValueContract = Expect<Equal<
  typeof semanticCategoryStore,
  ExpectedSemanticCategoryStore
>>;
type CategoryHexContract = Expect<Equal<
  typeof hexToRGBA,
  (hex: string | null | undefined, alpha: number) => string
>>;

type AssistantMessageRoleContract = Expect<Equal<
  AssistantMessageRole,
  ExpectedAssistantMessageRole
>>;
type AssistantStepStatusContract = Expect<Equal<
  AssistantStepStatus,
  ExpectedAssistantStepStatus
>>;
type AssistantConfirmStatusContract = Expect<Equal<
  AssistantConfirmStatus,
  ExpectedAssistantConfirmStatus
>>;
type AssistantReferenceContract = Expect<Equal<
  AssistantReference,
  ExpectedAssistantReference
>>;
type AssistantCardContract = Expect<Equal<AssistantCard, ExpectedAssistantCard>>;
type AssistantStepContract = Expect<Equal<AssistantStep, ExpectedAssistantStep>>;
type AssistantMessageInputContract = Expect<Equal<
  AssistantMessageInput,
  ExpectedAssistantMessageInput
>>;
type AssistantMessageContract = Expect<Equal<
  AssistantMessage,
  ExpectedAssistantMessage
>>;
type AssistantStateContract = Expect<Equal<AssistantState, ExpectedAssistantState>>;
type AssistantMessageUpdaterContract = Expect<Equal<
  AssistantMessageUpdater,
  ExpectedAssistantMessageUpdater
>>;
type AssistantModelSelectionOptionsContract = Expect<Equal<
  AssistantModelSelectionOptions,
  ExpectedAssistantModelSelectionOptions
>>;
type AssistantStoreContract = Expect<Equal<AssistantStore, ExpectedAssistantStore>>;
type BasicAssistantModelIdValueContract = Expect<Equal<
  typeof BASIC_ASSISTANT_MODEL_ID,
  '__basic__'
>>;
type AssistantStoreValueContract = Expect<Equal<
  typeof assistantStore,
  ExpectedAssistantStore
>>;
type AssistantStreamCompatibilityContract = Expect<
  AssistantMessage extends StreamMessage ? true : false
>;

type GithubUpdateInfoContract = Expect<Equal<
  GithubUpdateInfo,
  ExpectedGithubUpdateInfo
>>;
type GithubUpdateInstallResultContract = Expect<Equal<
  GithubUpdateInstallResult,
  ExpectedGithubUpdateInstallResult
>>;
type GithubUpdateStatusPayloadContract = Expect<Equal<
  GithubUpdateStatusPayload,
  ExpectedGithubUpdateStatusPayload
>>;
type RunUpdateFlowOptionsContract = Expect<Equal<
  RunUpdateFlowOptions,
  ExpectedRunUpdateFlowOptions
>>;
type RunUpdateFlowResultContract = Expect<Equal<
  RunUpdateFlowResult,
  ExpectedRunUpdateFlowResult
>>;
type RunUpdateFlowContract = Expect<Equal<
  RunUpdateFlow,
  ExpectedRunUpdateFlow
>>;
type UpdateFlowDependenciesContract = Expect<Equal<
  UpdateFlowDependencies,
  ExpectedUpdateFlowDependencies
>>;
type CreateUpdateFlowContract = Expect<Equal<
  typeof createUpdateFlow,
  (dependencies: ExpectedUpdateFlowDependencies) => ExpectedRunUpdateFlow
>>;
type RunUpdateFlowValueContract = Expect<Equal<
  typeof runUpdateFlow,
  ExpectedRunUpdateFlow
>>;

type CacheActivityContract = Expect<Equal<
  CacheActivity,
  ExpectedCacheActivity
>>;
type CacheEntryContract = Expect<Equal<CacheEntry, ExpectedCacheEntry>>;
type OverviewCacheEntryContract = Expect<Equal<
  OverviewCacheEntry,
  ExpectedOverviewCacheEntry
>>;
type TimelineCacheEntryContract = Expect<Equal<
  TimelineCacheEntry,
  ExpectedTimelineCacheEntry
>>;
type CacheStateContract = Expect<Equal<CacheState, ExpectedCacheState>>;
type CacheInvalidationTypeContract = Expect<Equal<
  CacheInvalidationType,
  ExpectedCacheInvalidationType
>>;
type CacheValidityKeyContract = Expect<Equal<
  CacheValidityKey,
  ExpectedCacheValidityKey
>>;
type CacheStoreContract = Expect<Equal<CacheStore, ExpectedCacheStore>>;
type CacheStoreValueContract = Expect<Equal<typeof cache, ExpectedCacheStore>>;
type GetLocalDateContract = Expect<Equal<typeof getLocalDate, () => string>>;
type CacheTimelineCompatibilityContract = Expect<
  TimelineActivity extends CacheActivity ? true : false
>;

type AppIconCacheValueContract = Expect<Equal<
  AppIconCacheValue,
  ExpectedAppIconCacheValue
>>;
type AppIconCacheStateContract = Expect<Equal<
  AppIconCacheState,
  ExpectedAppIconCacheState
>>;
type AppIconRequestEntryContract = Expect<Equal<
  AppIconRequestEntry,
  ExpectedAppIconRequestEntry
>>;
type AppIconRequestContract = Expect<Equal<
  AppIconRequest,
  ExpectedAppIconRequest
>>;
type AppIconLoadOptionsContract = Expect<Equal<
  AppIconLoadOptions,
  ExpectedAppIconLoadOptions
>>;
type GetAppIconArgsContract = Expect<Equal<
  GetAppIconArgs,
  ExpectedGetAppIconArgs
>>;
type AppIconInvokeContract = Expect<Equal<
  AppIconInvoke,
  ExpectedAppIconInvoke
>>;
type AppIconStoreContract = Expect<Equal<
  AppIconStore,
  ExpectedAppIconStore
>>;
type AppIconStoreValueContract = Expect<Equal<
  typeof appIconStore,
  ExpectedAppIconStore
>>;
type GetIconCacheKeyContract = Expect<Equal<
  typeof getIconCacheKey,
  (entry: ExpectedAppIconRequest) => string
>>;
type LoadAppIconContract = Expect<Equal<
  typeof loadAppIcon,
  (
    entry: ExpectedAppIconRequest,
    invoke: ExpectedAppIconInvoke,
    options?: ExpectedAppIconLoadOptions,
  ) => void
>>;
type PreloadAppIconsContract = Expect<Equal<
  typeof preloadAppIcons,
  (
    entries: readonly ExpectedAppIconRequest[] | null | undefined,
    invoke: ExpectedAppIconInvoke,
    options?: ExpectedAppIconLoadOptions,
  ) => void
>>;

type LocaleContract = Expect<Equal<Locale, ExpectedLocale>>;
type TranslationValueContract = Expect<Equal<
  TranslationValue,
  ExpectedTranslationValue
>>;
type TranslationDictionaryContract = Expect<Equal<
  TranslationDictionary,
  ExpectedTranslationDictionary
>>;
type InterpolationParamsContract = Expect<Equal<
  InterpolationParams,
  ExpectedInterpolationParams
>>;
type DurationFormatOptionsContract = Expect<Equal<
  DurationFormatOptions,
  ExpectedDurationFormatOptions
>>;
type SupportedLocalesContract = Expect<Equal<
  typeof SUPPORTED_LOCALES,
  readonly ['zh-CN', 'en', 'zh-TW', 'ar']
>>;
type LocaleStoreValueContract = Expect<Equal<
  typeof localeStore,
  ExpectedLocaleStore
>>;
type InitializeLocaleContract = Expect<Equal<
  typeof initializeLocale,
  (preferredLocale?: string | null) => ExpectedLocale
>>;
type SetLocaleContract = Expect<Equal<
  typeof setLocale,
  (nextLocale?: string | null) => ExpectedLocale
>>;
type CycleLocaleContract = Expect<Equal<
  typeof cycleLocale,
  () => ExpectedLocale
>>;
type GetLocaleShortLabelContract = Expect<Equal<
  typeof getLocaleShortLabel,
  (localeCode?: string | null) => string
>>;
type GetLocaleLabelContract = Expect<Equal<
  typeof getLocaleLabel,
  (localeCode?: string | null) => string
>>;
type ApplyLocaleToDocumentContract = Expect<Equal<
  typeof applyLocaleToDocument,
  (nextLocale?: string | null) => void
>>;
type TranslateContract = Expect<Equal<
  typeof translate,
  (key: string, params?: ExpectedInterpolationParams) => string
>>;
type TranslateMessagesContract = Expect<Equal<
  typeof translateMessages,
  (key: string) => ExpectedTranslationValue | undefined
>>;
type FormatLocalizedDateContract = Expect<Equal<
  typeof formatLocalizedDate,
  (
    date: Date | number,
    options?: Intl.DateTimeFormatOptions,
  ) => string
>>;
type FormatLocalizedTimeContract = Expect<Equal<
  typeof formatLocalizedTime,
  (
    date: Date | number,
    options?: Intl.DateTimeFormatOptions,
  ) => string
>>;
type FormatDurationLocalizedContract = Expect<Equal<
  typeof formatDurationLocalized,
  (
    seconds: number | null | undefined,
    options?: ExpectedDurationFormatOptions,
  ) => string
>>;
type TranslateCategoryLabelContract = Expect<Equal<
  typeof translateCategoryLabel,
  (categoryKey: string) => string
>>;
type TranslateSemanticCategoryLabelContract = Expect<Equal<
  typeof translateSemanticCategoryLabel,
  (label: string) => string
>>;

export type TypeScriptMigrationContracts =
  | HistoryBuilderContract
  | HistorySummaryContract
  | HistoryDigestContract
  | ModelLabelContract
  | RequestGateContract
  | StarterPromptContract
  | StreamReducerContract
  | BubbleMessageContract
  | ErrorDisplayContract
  | TimelinePreparationContract
  | TimelineUpsertContract
  | ReportDateShiftContract
  | ReportSnapshotContract
  | ReportOwnershipContract
  | ReportMetaContract
  | ReportPromptFeedbackContract
  | SummaryOrderContract
  | SummaryHourRangeContract
  | SummaryTextContract
  | SummaryPrimaryContract
  | SummarySecondaryContract
  | SummaryPartsContract
  | SummaryAppsContract
  | SummaryRhythmContract
  | ReportSectionsParseContract
  | ReportBlockNameContract
  | ReportVisibleSectionsContract
  | ReportDisplayMarkdownContract
  | ReportStorageMarkdownContract
  | OverviewCategoryContract
  | DomainSourcesContract
  | DomainTrackContract
  | DomainPresentationContract
  | SemanticCategoryColorContract
  | AvatarModeMetaContract
  | AvatarStateBubbleContract
  | AvatarActionLoopContract
  | AvatarMotionDelayContract
  | AvatarTransitionContract
  | AvatarIdleMotionContract
  | AvatarPresetDefaultContract
  | AvatarPresetOptionsContract
  | AvatarPresetNormalizeContract
  | AvatarPresetDefinitionContract
  | AvatarPresetOptionContract
  | AvatarScaleMinContract
  | AvatarScaleMaxContract
  | AvatarScaleDefaultContract
  | AvatarOpacityMinContract
  | AvatarOpacityMaxContract
  | AvatarOpacityDefaultContract
  | AvatarSettingsConfigContract
  | AvatarConfigSaverContract
  | AvatarToggleUiStateContract
  | AvatarToggleToastContract
  | AvatarToggleUiContract
  | AvatarToggleSettingContract
  | AvatarScaleClampContract
  | AvatarScaleLabelContract
  | AvatarScaleUpdateContract
  | AvatarOpacityClampContract
  | AvatarOpacityLabelContract
  | AvatarOpacityUpdateContract
  | FocusTrapActionResultContract
  | FocusTrapContract
  | RecordingStateInputContract
  | RecordingStateContract
  | RecordingStoreContract
  | RecordingStoreValueContract
  | RecordingActiveContract
  | ToastTypeContract
  | ToastStateContract
  | ToastStoreContract
  | ToastStoreValueContract
  | ToastShowContract
  | ToastClearContract
  | ConfirmToneContract
  | ConfirmOptionsContract
  | ConfirmDialogStateContract
  | ConfirmDialogStoreContract
  | ConfirmStoreValueContract
  | ConfirmOpenContract
  | ConfirmResolveContract
  | AiTestStatusContract
  | AiTextModelConfigInputContract
  | AiConfigInputContract
  | AiStoreStateContract
  | AiStoreContract
  | AiStoreValueContract
  | CategoryInfoContract
  | CategoryMetaContract
  | CategoryStoreContract
  | CategoryStoreValueContract
  | SemanticCategoryInfoContract
  | SemanticCategoryStoreContract
  | SemanticCategoryStoreValueContract
  | CategoryHexContract
  | AssistantMessageRoleContract
  | AssistantStepStatusContract
  | AssistantConfirmStatusContract
  | AssistantReferenceContract
  | AssistantCardContract
  | AssistantStepContract
  | AssistantMessageInputContract
  | AssistantMessageContract
  | AssistantStateContract
  | AssistantMessageUpdaterContract
  | AssistantModelSelectionOptionsContract
  | AssistantStoreContract
  | BasicAssistantModelIdValueContract
  | AssistantStoreValueContract
  | AssistantStreamCompatibilityContract
  | GithubUpdateInfoContract
  | GithubUpdateInstallResultContract
  | GithubUpdateStatusPayloadContract
  | RunUpdateFlowOptionsContract
  | RunUpdateFlowResultContract
  | RunUpdateFlowContract
  | UpdateFlowDependenciesContract
  | CreateUpdateFlowContract
  | RunUpdateFlowValueContract
  | CacheActivityContract
  | CacheEntryContract
  | OverviewCacheEntryContract
  | TimelineCacheEntryContract
  | CacheStateContract
  | CacheInvalidationTypeContract
  | CacheValidityKeyContract
  | CacheStoreContract
  | CacheStoreValueContract
  | GetLocalDateContract
  | CacheTimelineCompatibilityContract
  | AppIconCacheValueContract
  | AppIconCacheStateContract
  | AppIconRequestEntryContract
  | AppIconRequestContract
  | AppIconLoadOptionsContract
  | GetAppIconArgsContract
  | AppIconInvokeContract
  | AppIconStoreContract
  | AppIconStoreValueContract
  | GetIconCacheKeyContract
  | LoadAppIconContract
  | PreloadAppIconsContract
  | LocaleContract
  | TranslationValueContract
  | TranslationDictionaryContract
  | InterpolationParamsContract
  | DurationFormatOptionsContract
  | SupportedLocalesContract
  | LocaleStoreValueContract
  | InitializeLocaleContract
  | SetLocaleContract
  | CycleLocaleContract
  | GetLocaleShortLabelContract
  | GetLocaleLabelContract
  | ApplyLocaleToDocumentContract
  | TranslateContract
  | TranslateMessagesContract
  | FormatLocalizedDateContract
  | FormatLocalizedTimeContract
  | FormatDurationLocalizedContract
  | TranslateCategoryLabelContract
  | TranslateSemanticCategoryLabelContract;
