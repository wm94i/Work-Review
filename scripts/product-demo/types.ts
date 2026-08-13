import type { Page } from 'playwright';

export type DemoAspect = '16x9' | '9x16';
export type StoryboardSceneId =
  | 'hook'
  | 'timeline'
  | 'report'
  | 'assistant'
  | 'privacy'
  | 'export'
  | 'outro';
export type AssistantStage = 'basic-template' | 'demo-ai';

export interface DemoComposition {
  crop: string;
  scale: string;
  focus: string;
}

export interface StoryboardCue {
  id: string;
  start: number;
  end: number;
  zh: string;
  en: string;
}

export interface StoryboardScene {
  id: StoryboardSceneId;
  start: number;
  end: number;
  route: string;
  voiceoverZh: string;
  subtitleEn: string;
  composition: Record<DemoAspect, DemoComposition>;
  assistantStages?: AssistantStage[];
}

export interface DemoActivity {
  id: number;
  time: string;
  timestamp: number;
  app: string;
  title: string;
  category: string;
  duration: number;
  browserUrl: string | null;
  screenshotPath: string;
  ocrText: string | null;
}

export interface DemoCategoryStat {
  category: string;
  duration: number;
}

export interface DemoAppStat {
  appName: string;
  duration: number;
  count: number;
}

export interface DemoReportSection {
  title: string;
  markdown: string;
}

export interface DemoFixtures {
  date: string;
  timezone: string;
  locale: 'zh-CN';
  project: string;
  safeRoot: string;
  dataDir: string;
  exportPath: string;
  activities: DemoActivity[];
  stats: {
    totalDuration: number;
    workTimeDuration: number;
    screenshotCount: number;
    categories: DemoCategoryStat[];
    apps: DemoAppStat[];
    hourlyActivity: Array<{ hour: number; duration: number }>;
  };
  report: {
    createdAt: number;
    sections: DemoReportSection[];
    content: string;
  };
  assistant: {
    conversationId: number;
    basicQuestion: string;
    basicAnswer: string;
    aiQuestion: string;
    aiAnswer: string;
    modelProfile: {
      id: 'demo-ai';
      name: string;
      model_config: {
        provider: string;
        endpoint: string;
        api_key: null;
        model: string;
      };
    };
  };
  privacy: {
    appRules: Array<{ app_name: string; level: 'full' | 'anonymized' | 'ignored' }>;
    screenshotsEnabled: boolean;
  };
}

export interface DemoInvokeLogEntry {
  command: string;
  args: Record<string, unknown>;
  at: number;
}

export interface DemoStoredAssistantMessage {
  id: number;
  conversationId: number;
  role: 'user' | 'assistant';
  content: string;
  toolDigest: string | null;
  modelName: string | null;
  createdAt: number;
}

export interface DemoAssistantHistoryMessage {
  role: 'user' | 'assistant';
  content: string;
}

export interface DemoAssistantRequest {
  question: string;
  history: DemoAssistantHistoryMessage[];
  modelConfig: DemoFixtures['assistant']['modelProfile']['model_config'] | null;
  requestId: string;
  channelId: number | null;
}

export interface DemoChannelEnvelope {
  index: number;
  message?: Record<string, unknown>;
  end?: true;
}

export interface DemoMockState {
  fixtures: DemoFixtures;
  config: Record<string, unknown>;
  reportGenerated: boolean;
  reportContent: string;
  selectedAssistantModel: '__basic__' | 'demo-ai';
  assistantMessages: DemoStoredAssistantMessage[];
  assistantConversationCreated: boolean;
  privacyRules: DemoFixtures['privacy']['appRules'];
  screenshotsEnabled: boolean;
  exportedFiles: Record<string, string>;
  openedDirectories: string[];
  invokeLog: DemoInvokeLogEntry[];
  nextMessageId: number;
  conversationCreateCalls: number;
  chatCalls: number;
  appendCalls: number;
  chatRequests: DemoAssistantRequest[];
  pendingChannelEvents: Record<number, DemoChannelEnvelope[]>;
  unhandledCommands: string[];
}

export interface ShotContext {
  page: Page;
  aspect: DemoAspect;
  scene: StoryboardScene;
  fixtures: DemoFixtures;
  /** 标记源录屏中正式内容开始的位置，合成时会跳过导航和字体加载预卷。 */
  markContentStart?: () => void;
  /** 在弹窗、三档隐私等关键瞬态仍可见时主动保存动作验收帧。 */
  captureActionFrame?: () => Promise<void>;
}

export type ShotRunner = (context: ShotContext) => Promise<void>;
