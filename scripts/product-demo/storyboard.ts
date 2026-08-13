import type { StoryboardCue, StoryboardScene } from './types.ts';

export const DEMO_DATE = '2026-08-12';
export const DEMO_TIMEZONE = 'Asia/Shanghai';
export const DEMO_FPS = 30;
export const DEMO_DURATION_SECONDS = 85;
export const DEMO_TOTAL_FRAMES = DEMO_DURATION_SECONDS * DEMO_FPS;

export const STORYBOARD: readonly StoryboardScene[] = [
  {
    id: 'hook',
    start: 0,
    end: 7,
    route: '/report',
    voiceoverZh: '下班时，你还记得今天到底做了什么吗？',
    subtitleEn: 'At the end of the day, can you still remember what you actually worked on?',
    composition: {
      '16x9': { crop: '1920:1080:0:0', scale: '1920:1080', focus: '日报空态与问题标题' },
      '9x16': { crop: '900:1600:510:0', scale: '1080:1920', focus: '日报空态中央区域' },
    },
  },
  {
    id: 'timeline',
    start: 7,
    end: 19,
    route: '/timeline?date=2026-08-12',
    voiceoverZh: 'Work-Review 把当天用过的应用、页面和时间整理成一条可回看的工作轨迹。',
    subtitleEn: 'Work-Review turns the apps, pages, and time from your day into a timeline you can revisit.',
    composition: {
      '16x9': { crop: '1920:1080:0:0', scale: '1920:1080', focus: '时间线与活动详情抽屉' },
      '9x16': { crop: '760:1350:930:0', scale: '1080:1920', focus: '右侧时间线与详情抽屉' },
    },
  },
  {
    id: 'report',
    start: 19,
    end: 32,
    route: '/report',
    voiceoverZh: '选择日期，就能根据整日记录生成日报草稿；每个段落都可以再由我修改和保存。',
    subtitleEn: 'Choose a date to draft a report from the full day’s records, then edit and save each section yourself.',
    composition: {
      '16x9': { crop: '1920:1080:0:0', scale: '1920:1080', focus: '生成按钮、日报正文与段落编辑弹窗' },
      '9x16': { crop: '860:1528:600:0', scale: '1080:1920', focus: '日报正文和编辑弹窗' },
    },
  },
  {
    id: 'assistant',
    start: 32,
    end: 51,
    route: '/ask',
    voiceoverZh: '先用基础模板查看今天的时长、分类和常用应用；保持同一对话，切换到配置好的 AI，再查今日记录，提炼一项有依据的成果。',
    subtitleEn: 'Start with the basic template for time, categories, and top apps. In the same conversation, switch to your configured AI and turn today’s records into an evidence-based outcome.',
    composition: {
      '16x9': { crop: '1920:1080:0:0', scale: '1920:1080', focus: '模型选择器与对话内容' },
      '9x16': { crop: '850:1511:820:0', scale: '1080:1920', focus: '模型选择器、输入框与消息流' },
    },
    assistantStages: ['basic-template', 'demo-ai'],
  },
  {
    id: 'privacy',
    start: 51,
    end: 68,
    route: '/settings',
    voiceoverZh: '它不是监控软件。每个应用可以完全记录、只统计时长或完全忽略；关闭截图后，截图和 OCR 也会停止。',
    subtitleEn: 'It is not monitoring software. Each app can be fully recorded, time-only, or ignored; turn off screenshots and both screenshots and OCR stop.',
    composition: {
      '16x9': { crop: '1920:1080:0:0', scale: '1920:1080', focus: '隐私三档与截图开关' },
      '9x16': { crop: '900:1600:690:0', scale: '1080:1920', focus: '设置内容区域' },
    },
  },
  {
    id: 'export',
    start: 68,
    end: 78,
    route: '/settings',
    voiceoverZh: '活动记录和截图默认本机；查目录、导出 Markdown；外部 AI、远程存储按配置使用。',
    subtitleEn: 'Activity records and screenshots stay on your device by default. Inspect the folder and export reports as Markdown; external AI and remote storage follow your configuration.',
    composition: {
      '16x9': { crop: '1920:1080:0:0', scale: '1920:1080', focus: '本地数据目录与导出成功提示' },
      '9x16': { crop: '900:1600:650:0', scale: '1080:1920', focus: '目录路径与导出反馈' },
    },
  },
  {
    id: 'outro',
    start: 78,
    end: 85,
    route: '/about',
    voiceoverZh: '用一个真实工作日，找回你的第一份日报。',
    subtitleEn: 'Try it with one real workday and rediscover your first report.',
    composition: {
      '16x9': { crop: '1920:1080:0:0', scale: '1920:1080', focus: 'Logo 与片尾文案' },
      '9x16': { crop: '900:1600:510:0', scale: '1080:1920', focus: '竖屏 Logo 与片尾文案' },
    },
  },
] as const;

export const VOICEOVER_CUES: readonly StoryboardCue[] = STORYBOARD.map((scene) => ({
  id: scene.id,
  start: scene.start + 0.35,
  end: scene.end - 0.35,
  zh: scene.voiceoverZh,
  en: scene.subtitleEn,
}));

export function validateStoryboard(): void {
  if (STORYBOARD.length !== 7) throw new Error('演示视频必须包含七个分镜');
  if (STORYBOARD[0]?.start !== 0) throw new Error('首个分镜必须从 0 秒开始');
  if (STORYBOARD.at(-1)?.end !== DEMO_DURATION_SECONDS) {
    throw new Error('末个分镜必须在 85 秒结束');
  }

  for (const [index, scene] of STORYBOARD.entries()) {
    if (scene.end <= scene.start) throw new Error(`分镜 ${scene.id} 时长无效`);
    if (!Number.isInteger(scene.start * DEMO_FPS) || !Number.isInteger(scene.end * DEMO_FPS)) {
      throw new Error(`分镜 ${scene.id} 未对齐整数帧`);
    }
    if (!scene.composition['16x9'] || !scene.composition['9x16']) {
      throw new Error(`分镜 ${scene.id} 缺少横屏或竖屏构图`);
    }
    const previous = STORYBOARD[index - 1];
    if (previous && previous.end !== scene.start) {
      throw new Error(`分镜 ${previous.id} 与 ${scene.id} 不连续`);
    }
  }

  const assistant = STORYBOARD.find((scene) => scene.id === 'assistant');
  if (!assistant || assistant.assistantStages?.join(',') !== 'basic-template,demo-ai') {
    throw new Error('助手分镜必须依次展示基础模板和演示 AI');
  }

  if (VOICEOVER_CUES.some((cue, index) => cue.start < 0
    || cue.end > DEMO_DURATION_SECONDS
    || cue.end <= cue.start
    || (index > 0 && cue.start < VOICEOVER_CUES[index - 1].end))) {
    throw new Error('旁白时间轴越界或重叠');
  }
}
