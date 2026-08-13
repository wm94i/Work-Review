import { deflateSync } from 'node:zlib';

import type { BrowserContext } from 'playwright';

import type {
  DemoAssistantHistoryMessage,
  DemoAssistantRequest,
  DemoChannelEnvelope,
  DemoFixtures,
  DemoMockState,
  DemoStoredAssistantMessage,
} from './types.ts';

const REPORT_GENERATION_DELAY_MS = 700;
const ASSISTANT_STREAM_DELAY_MS = 140;
const PREVIEW_WIDTH = 640;
const PREVIEW_HEIGHT = 360;
const PREVIEW_DEFINITION = [
  { app: 'Nebula IDE', context: 'Export flow implementation', accent: [79, 70, 229] },
  { app: 'Atlas Browser', context: 'Product specification review', accent: [14, 165, 233] },
  { app: 'Paperwork Docs', context: 'Release checklist notes', accent: [16, 185, 129] },
  { app: 'Orbit Meet', context: 'Weekly release sync', accent: [245, 158, 11] },
  { app: 'Canvas Lab', context: 'Vertical layout concept', accent: [236, 72, 153] },
  { app: 'Nova Console', context: 'Frontend verification', accent: [139, 92, 246] },
] as const;

const REDACTED_GLYPHS = [
  [0x0, 0x0, 0x1ffe0, 0x20, 0x20, 0x20, 0x18020, 0x1ffe0, 0x18020, 0x18020, 0x18000, 0x18000, 0x18000, 0x18008, 0x18018, 0xfff0, 0x0, 0x0, 0x0, 0x0],
  [0x0, 0x10, 0x630, 0x1f320, 0x11160, 0x117f8, 0x11408, 0x1f408, 0x11408, 0x11408, 0x1f7f8, 0x11160, 0x11160, 0x11160, 0x31360, 0x31764, 0x27e3c, 0x6400, 0x0, 0x0],
  [0x0, 0xc0c0, 0x80c0, 0x1fe80, 0x10080, 0x201fc, 0x1fd18, 0x14f10, 0x12d90, 0x10890, 0x3fe90, 0x14890, 0x128e0, 0x10860, 0x1fe60, 0x8b0, 0x3b1c, 0x2604, 0x0, 0x0],
] as const;


const BITMAP_FONT: Record<string, readonly number[]> = {
  ' ': [0, 0, 0, 0, 0, 0, 0],
  A: [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
  B: [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
  C: [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
  D: [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
  E: [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
  F: [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
  G: [0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
  H: [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
  I: [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
  J: [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
  K: [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
  L: [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
  M: [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
  N: [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
  O: [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
  P: [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
  Q: [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
  R: [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
  S: [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
  T: [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
  U: [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
  V: [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
  W: [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
  X: [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
  Y: [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
  Z: [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
};

interface AssistantAnswer {
  answer: string;
  references: unknown[];
  usedAi: boolean;
  modelName: string | null;
  toolLabels: string[];
}

interface DemoAssistantReference {
  sourceType: 'activity';
  sourceId: number;
  date: string;
  timestamp: number;
  title: string;
  excerpt: string;
  appName: string;
  browserUrl: null;
  duration: number;
  score: number;
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ ((crc & 1) === 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function pngChunk(type: string, data: Uint8Array): Buffer {
  const typeBytes = Buffer.from(type, 'ascii');
  const body = Buffer.concat([typeBytes, data]);
  const chunk = Buffer.alloc(data.length + 12);
  chunk.writeUInt32BE(data.length, 0);
  body.copy(chunk, 4);
  chunk.writeUInt32BE(crc32(body), data.length + 8);
  return chunk;
}

function createInternationalTextChunk(text: string): Buffer {
  return pngChunk(
    'iTXt',
    Buffer.concat([
      Buffer.from('Description', 'ascii'),
      Buffer.from([0, 0, 0, 0, 0]),
      Buffer.from(text, 'utf8'),
    ]),
  );
}

function fillRect(
  pixels: Buffer,
  x: number,
  y: number,
  width: number,
  height: number,
  color: readonly number[],
): void {
  const startX = Math.max(0, x);
  const startY = Math.max(0, y);
  const endX = Math.min(PREVIEW_WIDTH, x + width);
  const endY = Math.min(PREVIEW_HEIGHT, y + height);
  for (let row = startY; row < endY; row += 1) {
    for (let column = startX; column < endX; column += 1) {
      const offset = (row * PREVIEW_WIDTH + column) * 4;
      pixels[offset] = color[0] ?? 0;
      pixels[offset + 1] = color[1] ?? 0;
      pixels[offset + 2] = color[2] ?? 0;
      pixels[offset + 3] = color[3] ?? 255;
    }
  }
}

function drawBitmapText(
  pixels: Buffer,
  x: number,
  y: number,
  text: string,
  scale: number,
  color: readonly number[],
): void {
  let cursorX = x;
  for (const character of text.toUpperCase()) {
    const rows = BITMAP_FONT[character] ?? BITMAP_FONT[' '];
    for (const [rowIndex, rowBits] of rows.entries()) {
      for (let column = 0; column < 5; column += 1) {
        if ((rowBits & (1 << (4 - column))) === 0) continue;
        fillRect(
          pixels,
          cursorX + column * scale,
          y + rowIndex * scale,
          scale,
          scale,
          color,
        );
      }
    }
    cursorX += 6 * scale;
  }
}

function drawRedactedBadge(pixels: Buffer): void {
  const scale = 3;
  const glyphWidth = 20 * scale;
  const gap = 8;
  const badgeWidth = REDACTED_GLYPHS.length * glyphWidth + (REDACTED_GLYPHS.length - 1) * gap + 48;
  const badgeX = PREVIEW_WIDTH - badgeWidth - 28;
  const badgeY = 24;
  fillRect(pixels, badgeX, badgeY, badgeWidth, 84, [15, 23, 42, 235]);
  for (const [glyphIndex, rows] of REDACTED_GLYPHS.entries()) {
    const glyphX = badgeX + 24 + glyphIndex * (glyphWidth + gap);
    for (const [rowIndex, rowBits] of rows.entries()) {
      for (let column = 0; column < 20; column += 1) {
        if ((rowBits & (1 << (19 - column))) === 0) continue;
        fillRect(
          pixels,
          glyphX + column * scale,
          badgeY + 12 + rowIndex * scale,
          scale,
          scale,
          [255, 255, 255, 255],
        );
      }
    }
  }
}

function buildPreviewPng(metadata: string, accent: readonly number[], variant: number): string {
  const pixels = Buffer.alloc(PREVIEW_WIDTH * PREVIEW_HEIGHT * 4);
  fillRect(pixels, 0, 0, PREVIEW_WIDTH, PREVIEW_HEIGHT, [241, 245, 249, 255]);
  fillRect(pixels, 0, 0, PREVIEW_WIDTH, 48, [15, 23, 42, 255]);
  drawBitmapText(pixels, 24, 16, 'Aurora Board', 2, [255, 255, 255, 255]);
  fillRect(pixels, 24, 76, 592, 260, [255, 255, 255, 255]);
  fillRect(pixels, 24, 76, 12, 260, [...accent, 255]);
  drawBitmapText(
    pixels,
    62,
    102,
    PREVIEW_DEFINITION[variant % PREVIEW_DEFINITION.length].app,
    3,
    [51, 65, 85, 255],
  );
  fillRect(pixels, 62, 138, 420 - variant * 13, 12, [148, 163, 184, 255]);
  fillRect(pixels, 62, 168, 360 + variant * 17, 12, [203, 213, 225, 255]);
  fillRect(pixels, 62, 214, 128, 88, [...accent, 42]);
  fillRect(pixels, 210, 214, 174, 88, [226, 232, 240, 255]);
  fillRect(pixels, 404, 214, 170, 88, [226, 232, 240, 255]);
  drawRedactedBadge(pixels);

  const scanlines = Buffer.alloc(PREVIEW_HEIGHT * (PREVIEW_WIDTH * 4 + 1));
  for (let row = 0; row < PREVIEW_HEIGHT; row += 1) {
    const destination = row * (PREVIEW_WIDTH * 4 + 1);
    scanlines[destination] = 0;
    pixels.copy(scanlines, destination + 1, row * PREVIEW_WIDTH * 4, (row + 1) * PREVIEW_WIDTH * 4);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(PREVIEW_WIDTH, 0);
  header.writeUInt32BE(PREVIEW_HEIGHT, 4);
  header[8] = 8;
  header[9] = 6;
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    pngChunk('IHDR', header),
    createInternationalTextChunk(metadata),
    pngChunk('IDAT', deflateSync(scanlines, { level: 9 })),
    pngChunk('IEND', Buffer.alloc(0)),
  ]).toString('base64');
}

function buildSafePreview(fixtures: DemoFixtures, path: unknown): string {
  const index = fixtures.activities.findIndex((activity) => activity.screenshotPath === path);
  const variant = index >= 0 ? index : 0;
  const definition = PREVIEW_DEFINITION[variant % PREVIEW_DEFINITION.length];
  const metadata = `已脱敏 | Aurora Board | ${definition.app} | ${definition.context}`;
  return buildPreviewPng(metadata, definition.accent, variant);
}

function buildAiReferences(fixtures: DemoFixtures): DemoAssistantReference[] {
  const selected = [fixtures.activities[0], fixtures.activities[2], fixtures.activities[5]];
  const titles = [
    'Aurora Board 上午导出流程实现',
    'Aurora Board 上午发布检查清单文档',
    '16:40 Aurora Board 前端验证通过',
  ];
  const excerpts = [
    '上午完成导出流程实现与端到端整理。',
    '上午补充发布检查清单与文档记录。',
    '16:40 完成前端验证，检查结果通过。',
  ];
  return selected.map((activity, index) => ({
    sourceType: 'activity',
    sourceId: activity.id,
    date: fixtures.date,
    timestamp: activity.timestamp,
    title: titles[index],
    excerpt: excerpts[index],
    appName: PREVIEW_DEFINITION[[0, 2, 5][index]].app,
    browserUrl: null,
    duration: activity.duration,
    score: 1 - index * 0.05,
  }));
}

function createDemoConfig(fixtures: DemoFixtures): Record<string, unknown> {
  return {
    theme: 'light',
    ui_visual_style: 'b',
    background_image: null,
    background_opacity: 0.25,
    background_blur: 1,
    daily_work_goal_minutes: 420,
    standard_work_hours: 7.5,
    work_time_enabled: true,
    work_time_segments: [
      { start_hour: 9, start_minute: 0, end_hour: 12, end_minute: 0 },
      { start_hour: 13, start_minute: 30, end_hour: 18, end_minute: 0 },
    ],
    work_start_hour: 9,
    work_start_minute: 0,
    work_end_hour: 18,
    work_end_minute: 0,
    auto_start: false,
    auto_start_silent: true,
    hide_dock_icon: false,
    lightweight_mode: false,
    break_reminder_enabled: false,
    break_reminder_interval_minutes: 50,
    avatar_enabled: false,
    avatar_scale: 0.9,
    avatar_opacity: 0.82,
    avatar_preset: 'original-standard',
    avatar_persona: 'assistant',
    avatar_click_through: false,
    avatar_proactive_ai_enabled: false,
    idle_threshold_minutes: 5,
    goal_notifications: false,
    memory_enabled: false,
    daily_report_auto_generate_time: null,
    daily_report_custom_prompt: '',
    daily_report_export_dir: `${fixtures.safeRoot}exports/`,
    daily_report_auto_export: false,
    daily_report_prompt_presets: [],
    daily_report_system_prompt_override: '',
    daily_report_pinned_blocks: [],
    daily_report_hidden_blocks: [],
    screenshot_interval: 30,
    storage: {
      screenshot_retention_days: 14,
      metadata_retention_days: 60,
      storage_limit_mb: 4096,
      jpeg_quality: 85,
      max_image_width: 1920,
      screenshots_enabled: fixtures.privacy.screenshotsEnabled,
      screenshot_display_mode: 'active_window',
      screenshot_width_mode: 'auto',
    },
    remote_storage: {
      provider: 'none',
      s3: {
        endpoint: '',
        region: '',
        bucket: '',
        access_key: '',
        secret_key: '',
        path_prefix: '',
        public_url_base: null,
      },
      webdav: {
        url: '',
        username: '',
        password: '',
        path_prefix: '',
        public_url_base: null,
      },
    },
    privacy: {
      app_rules: structuredClone(fixtures.privacy.appRules),
      excluded_keywords: [],
      excluded_domains: [],
    },
    app_category_rules: [],
    ai_mode: 'local',
    ai_provider: {
      provider: 'ollama',
      endpoint: 'http://127.0.0.1:11434',
      api_key: null,
      model: 'demo-vision',
      vision_model: 'demo-vision',
    },
    text_model: structuredClone(fixtures.assistant.modelProfile.model_config),
    text_model_profiles: [structuredClone(fixtures.assistant.modelProfile)],
    assistant_web_access_enabled: false,
    memory_semantic_enabled: false,
    node_devices: [],
    node_gateway: { device_name: 'Work Review Demo' },
    mcp_server_enabled: false,
    localhost_api_enabled: false,
    localhost_api_port: 47831,
    localhost_api_host: '127.0.0.1',
    telegram_bot_enabled: false,
    telegram_bot_token: null,
    telegram_bot_proxy: null,
    feishu_bot_enabled: false,
    feishu_app_id: null,
    feishu_app_secret: null,
    wecom_bot_enabled: false,
    wecom_corp_id: null,
    wecom_token: null,
    wecom_encoding_aes_key: null,
    dingtalk_bot_enabled: false,
    dingtalk_app_secret: null,
  };
}

export function createDemoMockState(fixtures: DemoFixtures): DemoMockState {
  const safeFixtures = structuredClone(fixtures);
  return {
    fixtures: safeFixtures,
    config: createDemoConfig(safeFixtures),
    reportGenerated: false,
    reportContent: safeFixtures.report.content,
    selectedAssistantModel: '__basic__',
    assistantMessages: [],
    assistantConversationCreated: false,
    privacyRules: structuredClone(safeFixtures.privacy.appRules),
    screenshotsEnabled: safeFixtures.privacy.screenshotsEnabled,
    exportedFiles: {},
    openedDirectories: [],
    invokeLog: [],
    nextMessageId: 1,
    conversationCreateCalls: 0,
    chatCalls: 0,
    appendCalls: 0,
    chatRequests: [],
    pendingChannelEvents: {},
    unhandledCommands: [],
  };
}

function recordInvoke(
  state: DemoMockState,
  command: string,
  args: Record<string, unknown>,
): void {
  state.invokeLog.push({ command, args: structuredClone(args), at: Date.now() });
}

function toTimelineActivity(activity: DemoFixtures['activities'][number]) {
  return {
    id: activity.id,
    timestamp: activity.timestamp,
    app_name: activity.app,
    window_title: activity.title,
    screenshot_path: activity.screenshotPath,
    ocr_text: activity.ocrText,
    category: activity.category,
    duration: activity.duration,
    browser_url: activity.browserUrl,
    executable_path: null,
    semantic_category: null,
    semantic_confidence: null,
  };
}

function toStats(fixtures: DemoFixtures) {
  return {
    total_duration: fixtures.stats.totalDuration,
    work_time_duration: fixtures.stats.workTimeDuration,
    screenshot_count: fixtures.stats.screenshotCount,
    app_usage: fixtures.stats.apps.map((item) => ({
      app_name: item.appName,
      duration: item.duration,
      count: item.count,
    })),
    category_usage: structuredClone(fixtures.stats.categories),
    hourly_activity: structuredClone(fixtures.stats.hourlyActivity),
    browser_duration: fixtures.stats.categories.find((item) => item.category === 'browser')?.duration ?? 0,
    url_usage: [],
    domain_usage: [],
    domain_total_count: 0,
    browser_usage: [],
  };
}

function toHourlySummaries(fixtures: DemoFixtures) {
  return fixtures.activities.map((activity) => ({
    id: 10_000 + activity.id,
    date: fixtures.date,
    hour: Number(activity.time.slice(0, 2)),
    summary: `${activity.time} 使用 ${activity.app}：${activity.title}。`,
    main_apps: activity.app,
    activity_count: 1,
    total_duration: activity.duration,
    representative_screenshots: activity.screenshotPath,
    created_at: activity.timestamp,
  }));
}

function extractChannelId(value: unknown): number | null {
  if (typeof value === 'number' && Number.isInteger(value)) return value;
  if (typeof value === 'string') {
    const match = /^__CHANNEL__:(\d+)$/.exec(value);
    return match ? Number(match[1]) : null;
  }
  if (typeof value === 'object' && value !== null && 'id' in value) {
    const id = (value as { id?: unknown }).id;
    return typeof id === 'number' && Number.isInteger(id) ? id : null;
  }
  return null;
}

function normalizeHistory(value: unknown): DemoAssistantHistoryMessage[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (typeof item !== 'object' || item === null) return [];
    const { role, content } = item as { role?: unknown; content?: unknown };
    if ((role !== 'user' && role !== 'assistant') || typeof content !== 'string') return [];
    return [{ role, content }];
  });
}

function chunkAnswer(answer: string): string[] {
  return Array.from(answer.matchAll(/[\s\S]{1,18}/gu), (match) => match[0]);
}

function buildAssistantEvents(
  request: DemoAssistantRequest,
  answer: AssistantAnswer,
  isAi: boolean,
): DemoChannelEnvelope[] {
  const tool = isAi ? 'query_activities' : 'get_today_stats';
  const label = isAi ? '查询今日活动' : '汇总今日统计';
  const digest = isAi
    ? '命中 3 条 Aurora Board 记录：上午实现、上午文档和 16:40 前端验证。'
    : '已汇总今天的工作统计';
  const references = answer.references;
  const messages: Array<Record<string, unknown>> = [
    {
      type: 'stepStart',
      requestId: request.requestId,
      tool,
      label,
    },
    {
      type: 'stepResult',
      requestId: request.requestId,
      tool,
      ok: true,
      hits: references.length,
      references,
      digest,
    },
    ...chunkAnswer(answer.answer).map((token) => ({
      type: 'token',
      requestId: request.requestId,
      token,
    })),
    {
      type: 'done',
      requestId: request.requestId,
      answer: answer.answer,
      references,
      toolLabels: [label],
      usedAi: answer.usedAi,
      modelName: answer.modelName,
    },
  ];
  return [
    ...messages.map((message, index) => ({ index, message })),
    { index: messages.length, end: true },
  ];
}

function assertAiHistory(state: DemoMockState, history: DemoAssistantHistoryMessage[]): void {
  const expected = [
    { role: 'user', content: state.fixtures.assistant.basicQuestion },
    { role: 'assistant', content: state.fixtures.assistant.basicAnswer },
  ];
  const hasExpectedRound = expected.every((item, index) => {
    const actual = history[index];
    if (actual?.role !== item.role) return false;
    return item.role === 'assistant'
      ? actual.content.startsWith(item.content)
      : actual.content === item.content;
  });
  if (!hasExpectedRound) {
    throw new Error('演示 AI 请求缺少同一会话中的基础模板完整历史');
  }
}

function clone<T>(value: T): T {
  return structuredClone(value);
}

export async function handleDemoInvoke(
  state: DemoMockState,
  command: string,
  args: Record<string, unknown> = {},
): Promise<unknown> {
  recordInvoke(state, command, args);

  switch (command) {
    case 'plugin:event|listen':
      return args.handler;
    case 'plugin:event|unlisten':
    case 'plugin:event|emit':
    case 'plugin:event|emit_to':
      return null;
    case 'plugin:window|is_visible':
      return true;
    case 'plugin:app|version':
      return '1.1.1-demo';
    case 'get_platform':
    case 'get_runtime_platform':
      return 'macos';
    case 'check_permissions':
      return {
        screen_capture: true,
        accessibility: true,
        input_monitoring: true,
        screenshot_supported: true,
        avatar_input_supported: true,
        all_granted: true,
        platform: 'macos',
      };
    case 'get_config':
      return clone(state.config);
    case 'get_update_settings':
      return {
        autoCheck: true,
        lastCheckTime: 0,
        checkIntervalHours: 24,
      };
    case 'save_update_settings':
      return null;
    case 'should_check_updates':
      return false;
    case 'save_config': {
      const config = args.config;
      if (typeof config !== 'object' || config === null || Array.isArray(config)) {
        throw new TypeError('save_config 缺少有效 config');
      }
      state.config = clone(config as Record<string, unknown>);
      const privacy = (state.config.privacy ?? {}) as { app_rules?: DemoFixtures['privacy']['appRules'] };
      const storage = (state.config.storage ?? {}) as { screenshots_enabled?: boolean };
      state.privacyRules = clone(privacy.app_rules ?? []);
      state.screenshotsEnabled = storage.screenshots_enabled !== false;
      return null;
    }
    case 'set_app_locale':
    case 'set_report_block_preference':
    case 'pause_recording':
    case 'resume_recording':
      return null;
    case 'get_recording_state':
      return [true, false];
    case 'get_today_stats':
    case 'get_daily_stats':
    case 'get_overview_stats':
      return toStats(state.fixtures);
    case 'get_overview_domains':
      return { domains: [], total_count: 0 };
    case 'get_timeline':
      return args.date === state.fixtures.date
        ? state.fixtures.activities.map(toTimelineActivity)
        : [];
    case 'get_activity': {
      const activity = state.fixtures.activities.find((item) => item.id === args.id);
      return activity ? toTimelineActivity(activity) : null;
    }
    case 'get_hourly_summaries':
      return args.date === state.fixtures.date ? toHourlySummaries(state.fixtures) : [];
    case 'get_hourly_app_breakdown':
      return state.fixtures.activities.map((activity) => ({
        hour: Number(activity.time.slice(0, 2)),
        apps: [{
          app_name: activity.app,
          category: activity.category,
          duration: activity.duration,
        }],
      }));
    case 'get_app_icon':
      // 演示不读取宿主机应用图标；返回空值让前端使用确定性的内置回退图标。
      return '';
    case 'get_screenshot_thumbnail':
    case 'get_screenshot_full':
      return buildSafePreview(state.fixtures, args.path);
    case 'get_saved_report':
      if (!state.reportGenerated || args.date !== state.fixtures.date) return null;
      return {
        date: state.fixtures.date,
        content: state.reportContent,
        created_at: state.fixtures.report.createdAt,
        ai_mode: 'local',
      };
    case 'generate_report':
      await sleep(REPORT_GENERATION_DELAY_MS);
      state.reportGenerated = true;
      state.reportContent = state.fixtures.report.content;
      return null;
    case 'update_report_content': {
      if (typeof args.content !== 'string') {
        throw new TypeError('update_report_content 缺少 content');
      }
      state.reportGenerated = true;
      state.reportContent = args.content;
      return null;
    }
    case 'export_report_markdown': {
      if (typeof args.content !== 'string') {
        throw new TypeError('export_report_markdown 缺少 content');
      }
      state.exportedFiles[state.fixtures.exportPath] = args.content;
      return state.fixtures.exportPath;
    }
    case 'open_data_dir':
      state.openedDirectories.push(state.fixtures.dataDir);
      return null;
    case 'get_data_dir':
    case 'get_default_data_dir':
      return state.fixtures.dataDir;
    case 'get_storage_stats':
      return {
        total_files: state.fixtures.stats.screenshotCount,
        total_size_mb: 48,
        storage_limit_mb: 4096,
        retention_days: 14,
        oldest_file_date: state.fixtures.date,
      };
    case 'get_categories':
      return [
        { key: 'development', name: '开发', color: '#6366f1', icon: 'code' },
        { key: 'office', name: '办公', color: '#0ea5e9', icon: 'briefcase' },
        { key: 'communication', name: '沟通', color: '#f59e0b', icon: 'message-circle' },
        { key: 'browser', name: '浏览', color: '#10b981', icon: 'globe' },
      ];
    case 'get_semantic_categories':
    case 'get_custom_semantic_categories':
      return [];
    case 'get_running_apps':
    case 'get_recent_apps':
      return state.fixtures.activities.map((activity) => activity.app);
    case 'get_ai_providers':
      return [{ id: 'ollama', name: 'Ollama', endpoint: 'http://127.0.0.1:11434' }];
    case 'is_autostart_enabled':
      return false;
    case 'list_assistant_conversations':
      return state.assistantConversationCreated
        ? [{
            id: state.fixtures.assistant.conversationId,
            title: state.fixtures.assistant.basicQuestion,
            createdAt: state.fixtures.report.createdAt,
            updatedAt: state.fixtures.report.createdAt,
            messageCount: state.assistantMessages.length,
          }]
        : [];
    case 'create_assistant_conversation':
      if (state.assistantConversationCreated) {
        throw new Error('演示助手只允许创建一次会话');
      }
      state.assistantConversationCreated = true;
      state.conversationCreateCalls += 1;
      return state.fixtures.assistant.conversationId;
    case 'append_assistant_message': {
      const conversationId = Number(args.conversationId);
      const role = args.role;
      const content = args.content;
      if (
        conversationId !== state.fixtures.assistant.conversationId
        || (role !== 'user' && role !== 'assistant')
        || typeof content !== 'string'
      ) {
        throw new TypeError('append_assistant_message 参数无效');
      }
      const message: DemoStoredAssistantMessage = {
        id: state.nextMessageId++,
        conversationId,
        role,
        content,
        toolDigest: typeof args.toolDigest === 'string' ? args.toolDigest : null,
        modelName: typeof args.modelName === 'string' ? args.modelName : null,
        createdAt: state.fixtures.report.createdAt + state.nextMessageId,
      };
      state.assistantMessages.push(message);
      state.appendCalls += 1;
      return message.id;
    }
    case 'get_assistant_messages':
      return state.assistantMessages
        .filter((message) => message.conversationId === Number(args.conversationId))
        .map((message) => clone(message));
    case 'generate_text_with_model':
      return '["今天最值得总结的成果是什么？"]';
    case 'chat_work_assistant': {
      const modelConfig = args.modelConfig === null
        ? null
        : clone(args.modelConfig as DemoFixtures['assistant']['modelProfile']['model_config']);
      const request: DemoAssistantRequest = {
        question: String(args.question ?? ''),
        history: normalizeHistory(args.history),
        modelConfig,
        requestId: String(args.requestId ?? ''),
        channelId: extractChannelId(args.onEvent),
      };
      const isAi = modelConfig !== null;
      if (isAi) assertAiHistory(state, request.history);
      const answer: AssistantAnswer = {
        answer: isAi ? state.fixtures.assistant.aiAnswer : state.fixtures.assistant.basicAnswer,
        references: isAi ? buildAiReferences(state.fixtures) : [],
        usedAi: isAi,
        modelName: isAi ? state.fixtures.assistant.modelProfile.name : null,
        toolLabels: [isAi ? '查询今日活动' : '汇总今日统计'],
      };
      state.selectedAssistantModel = isAi ? 'demo-ai' : '__basic__';
      state.chatCalls += 1;
      state.chatRequests.push(request);
      if (request.channelId !== null) {
        state.pendingChannelEvents[request.channelId] = buildAssistantEvents(request, answer, isAi);
      }
      return answer;
    }
    case 'cancel_assistant_request':
    case 'confirm_assistant_action':
      return null;
    default:
      state.unhandledCommands.push(command);
      throw new Error(`产品演示 Tauri Mock 未实现命令：${command}`);
  }
}

export function takePendingChannelEvents(
  state: DemoMockState,
  channelId: number,
): DemoChannelEnvelope[] {
  const events = state.pendingChannelEvents[channelId] ?? [];
  delete state.pendingChannelEvents[channelId];
  return clone(events);
}

function buildDemoTauriInitScript(locale: DemoFixtures['locale']): string {
  const serializedLocale = JSON.stringify(locale);
  return `(() => {
    window.localStorage.setItem('work-review.locale', ${serializedLocale});
    window.localStorage.setItem('theme', 'light');
    window.localStorage.setItem(
      'work-review-assistant-state',
      JSON.stringify({
        messages: [],
        selectedModelId: '__basic__',
        hasUserSelectedModel: true,
        sending: false,
        sendingRequestId: null,
        conversationId: null,
      }),
    );

    const callbacks = new Map();
    let nextCallbackId = 1;
    const registerCallback = (callback, once = false) => {
      const id = nextCallbackId++;
      callbacks.set(id, (data) => {
        if (once) callbacks.delete(id);
        return callback?.(data);
      });
      return id;
    };

    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: 'main' },
        currentWebview: { windowLabel: 'main', label: 'main' },
      },
      callbacks,
      transformCallback: registerCallback,
      unregisterCallback: (id) => callbacks.delete(id),
      runCallback: (id, data) => callbacks.get(id)?.(data),
      convertFileSrc: (filePath) => filePath,
      invoke: async (command, args = {}) => {
        const normalizedArgs = { ...args };
        const onEvent = normalizedArgs.onEvent;
        if (typeof onEvent === 'object' && onEvent !== null && 'id' in onEvent) {
          normalizedArgs.onEvent = { id: onEvent.id };
        }
        const response = await window.__WORK_REVIEW_DEMO_INVOKE__(command, normalizedArgs);
        if (response.channelId !== null) {
          const deliver = callbacks.get(response.channelId);
          for (const event of response.events) {
            await new Promise((resolve) => setTimeout(resolve, ${ASSISTANT_STREAM_DELAY_MS}));
            deliver?.(event);
          }
        }
        return response.result;
      },
    };
  })();`;
}

export async function installDemoTauriMock(
  context: BrowserContext,
  fixtures: DemoFixtures,
): Promise<DemoMockState> {
  const state = createDemoMockState(fixtures);

  await context.exposeFunction(
    '__WORK_REVIEW_DEMO_INVOKE__',
    async (command: string, args: Record<string, unknown> = {}) => {
      const result = await handleDemoInvoke(state, command, args);
      const channelId = extractChannelId(args.onEvent);
      return {
        result,
        channelId,
        events: channelId === null ? [] : takePendingChannelEvents(state, channelId),
      };
    },
  );

  await context.exposeFunction(
    '__WORK_REVIEW_DEMO_SHOT_INVOKES__',
    async () => clone(state.invokeLog),
  );

  await context.exposeFunction(
    '__WORK_REVIEW_DEMO_EXPORTED_FILES__',
    async () => clone(Object.entries(state.exportedFiles).map(([path, content]) => ({
      path,
      content,
      kind: 'markdown' as const,
    }))),
  );

  // 通过纯 JavaScript 字符串注入，避免 tsx/esbuild 给序列化函数插入浏览器中不存在的 __name 辅助函数。
  await context.addInitScript({ content: buildDemoTauriInitScript(fixtures.locale) });

  return state;
}
