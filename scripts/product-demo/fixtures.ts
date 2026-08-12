import type { DemoFixtures } from './types.ts';
import { DEMO_DATE, DEMO_TIMEZONE } from './storyboard.ts';

const SAFE_ROOT = '/tmp/work-review-demo/';
const epoch = (time: string) => Math.floor(Date.parse(`${DEMO_DATE}T${time}:00+08:00`) / 1000);

const REPORT_SECTIONS = [
  {
    title: '今日概览',
    markdown: '全天有效投入 **5 小时 30 分钟**。工作围绕 Aurora Board 导出流程、发布检查清单和日报体验验证展开。',
  },
  {
    title: '重点进展',
    markdown: '- 完成 Aurora Board 导出流程实现与端到端验证。\n- 整理发布检查清单并复核设计规格。\n- 完成日报空态与竖屏构图方案。',
  },
  {
    title: '专注与协作',
    markdown: '开发投入 3 小时 10 分钟，办公 1 小时 10 分钟，沟通 45 分钟，浏览 25 分钟；14:03 的周会集中同步了发布范围。',
  },
  {
    title: '明日计划',
    markdown: '1. 跟进导出边界场景。\n2. 完成发布前回归。\n3. 整理演示反馈。',
  },
];

function buildReportContent(): string {
  return [
    '# 2026 年 8 月 12 日工作日报',
    '> 今天聚焦 Aurora Board 导出流程与发布验证。',
    ...REPORT_SECTIONS.flatMap((section) => [`## ${section.title}`, section.markdown]),
  ].join('\n\n');
}

const TEMPLATE: DemoFixtures = {
  date: DEMO_DATE,
  timezone: DEMO_TIMEZONE,
  locale: 'zh-CN',
  project: 'Aurora Board',
  safeRoot: SAFE_ROOT,
  dataDir: `${SAFE_ROOT}data/`,
  exportPath: `${SAFE_ROOT}exports/${DEMO_DATE}.md`,
  activities: [
    { id: 101, time: '09:12', timestamp: epoch('09:12'), app: 'Cursor', title: 'Aurora Board · 导出流程', category: 'development', duration: 6_300, browserUrl: null, screenshotPath: `${SAFE_ROOT}screenshots/cursor-export.png`, ocrText: 'export_report_markdown · Aurora Board' },
    { id: 102, time: '10:36', timestamp: epoch('10:36'), app: '浏览器', title: '设计规格复核 · docs.example.test', category: 'browser', duration: 1_500, browserUrl: 'https://docs.example.test/aurora-board/spec', screenshotPath: `${SAFE_ROOT}screenshots/spec-review.png`, ocrText: 'Aurora Board 产品演示规格' },
    { id: 103, time: '11:18', timestamp: epoch('11:18'), app: '文档', title: 'Aurora Board 发布检查清单', category: 'office', duration: 2_700, browserUrl: null, screenshotPath: `${SAFE_ROOT}screenshots/release-checklist.png`, ocrText: '发布检查清单：导出、验证、回归' },
    { id: 104, time: '14:03', timestamp: epoch('14:03'), app: '会议', title: 'Aurora Board 周会', category: 'communication', duration: 2_700, browserUrl: null, screenshotPath: `${SAFE_ROOT}screenshots/weekly-meeting.png`, ocrText: 'Aurora Board 周会 · 发布范围同步' },
    { id: 105, time: '15:24', timestamp: epoch('15:24'), app: 'Figma', title: '日报空态与竖屏构图', category: 'office', duration: 1_500, browserUrl: null, screenshotPath: `${SAFE_ROOT}screenshots/vertical-layout.png`, ocrText: '日报空态 · 9:16 构图' },
    { id: 106, time: '16:40', timestamp: epoch('16:40'), app: 'Terminal', title: 'npm run verify:frontend', category: 'development', duration: 5_100, browserUrl: null, screenshotPath: `${SAFE_ROOT}screenshots/frontend-verify.png`, ocrText: 'svelte-check 0 errors · tests passed · vite build complete' },
  ],
  stats: {
    totalDuration: 19_800,
    workTimeDuration: 19_800,
    screenshotCount: 146,
    categories: [
      { category: 'development', duration: 11_400 },
      { category: 'office', duration: 4_200 },
      { category: 'communication', duration: 2_700 },
      { category: 'browser', duration: 1_500 },
    ],
    apps: [
      { appName: 'Cursor', duration: 6_300, count: 42 },
      { appName: 'Terminal', duration: 5_100, count: 18 },
      { appName: '文档', duration: 2_700, count: 16 },
      { appName: '会议', duration: 2_700, count: 6 },
      { appName: '浏览器', duration: 1_500, count: 12 },
      { appName: 'Figma', duration: 1_500, count: 10 },
    ],
    hourlyActivity: Array.from({ length: 24 }, (_, hour) => ({
      hour,
      duration: ({ 9: 2_880, 10: 2_700, 11: 2_520, 14: 2_700, 15: 2_700, 16: 4_500, 17: 1_800 } as Record<number, number>)[hour] ?? 0,
    })),
  },
  report: {
    createdAt: epoch('17:30'),
    sections: REPORT_SECTIONS,
    content: buildReportContent(),
  },
  assistant: {
    conversationId: 7_001,
    basicQuestion: '今天主要做了什么？',
    basicAnswer: '今天共记录有效工作 5 小时 30 分钟。开发 3 小时 10 分钟（57.6%），办公 1 小时 10 分钟（21.2%），沟通 45 分钟（13.6%），浏览 25 分钟（7.6%）。使用最多的应用包括 Cursor、Terminal 和文档；当天还使用了浏览器、会议与 Figma。',
    aiQuestion: '结合刚才的今日记录，再查今天的活动，提炼一项有依据的日报成果。',
    aiAnswer: '最值得写进日报的成果，是完成 Aurora Board 导出流程的端到端验证，并整理发布检查清单。依据包括上午的实现与文档记录，以及 16:40 执行的前端验证。',
    modelProfile: {
      id: 'demo-ai',
      name: '演示 AI',
      model_config: {
        provider: 'ollama',
        endpoint: 'http://127.0.0.1:11434',
        api_key: null,
        model: 'demo-local-7b',
      },
    },
  },
  privacy: {
    appRules: [
      { app_name: 'Cursor', level: 'full' },
      { app_name: '浏览器', level: 'anonymized' },
      { app_name: '会议', level: 'ignored' },
    ],
    screenshotsEnabled: true,
  },
};

export function createDemoFixtures(): DemoFixtures {
  return structuredClone(TEMPLATE);
}
