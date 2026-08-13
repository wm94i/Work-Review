import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

import { DEMO_DURATION_SECONDS, VOICEOVER_CUES } from './storyboard.ts';
import type { DemoAspect, StoryboardCue } from './types.ts';

export type SubtitleLanguage = 'zh' | 'en';

export interface SubtitleArtifactPaths {
  zh: string;
  en: string;
  bilingual: string;
}

const SYSTEM_FONT_STACK = [
  '-apple-system',
  'BlinkMacSystemFont',
  '"SF Pro Text"',
  '"PingFang SC"',
  '"Hiragino Sans GB"',
  '"Segoe UI"',
  '"Microsoft YaHei"',
  'sans-serif',
].join(', ');

const OVERLAY_LAYOUT: Record<DemoAspect, {
  width: number;
  height: number;
  safeBottom: number;
  horizontalPadding: number;
  zhFontSize: number;
  enFontSize: number;
}> = {
  '16x9': {
    width: 1920,
    height: 1080,
    safeBottom: 84,
    horizontalPadding: 120,
    zhFontSize: 54,
    enFontSize: 32,
  },
  '9x16': {
    width: 1080,
    height: 1920,
    safeBottom: 240,
    horizontalPadding: 72,
    zhFontSize: 50,
    enFontSize: 30,
  },
};

export function formatSrtTimestamp(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) {
    throw new TypeError('SRT 时间必须是非负有限数');
  }

  const totalMilliseconds = Math.round(seconds * 1000);
  const milliseconds = totalMilliseconds % 1000;
  const totalSeconds = Math.floor(totalMilliseconds / 1000);
  const secs = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const minutes = totalMinutes % 60;
  const hours = Math.floor(totalMinutes / 60);

  return [
    String(hours).padStart(2, '0'),
    String(minutes).padStart(2, '0'),
    `${String(secs).padStart(2, '0')},${String(milliseconds).padStart(3, '0')}`,
  ].join(':');
}

export function validateSubtitleTimeline(
  cues: readonly StoryboardCue[],
  durationSeconds = DEMO_DURATION_SECONDS,
): void {
  if (!Number.isFinite(durationSeconds) || durationSeconds <= 0) {
    throw new TypeError('视频时长必须是正有限数');
  }

  for (const [index, cue] of cues.entries()) {
    if (!Number.isFinite(cue.start) || !Number.isFinite(cue.end)) {
      throw new Error(`字幕 ${cue.id} 的时间必须是有限数`);
    }
    if (cue.start < 0) {
      throw new Error(`字幕 ${cue.id} 不得早于 0 秒`);
    }
    if (cue.end <= cue.start) {
      throw new Error(`字幕 ${cue.id} 的结束时间必须晚于开始时间`);
    }
    if (cue.end > durationSeconds) {
      throw new Error(`字幕 ${cue.id} 超过视频时长`);
    }

    const previous = cues[index - 1];
    if (previous && cue.start < previous.end) {
      throw new Error(`字幕 ${cue.id} 与上一条重叠`);
    }
  }
}

export function buildSrt(
  cues: readonly StoryboardCue[],
  language: SubtitleLanguage,
): string {
  validateSubtitleTimeline(cues);
  return buildSrtFromLines(cues, (cue) => [cue[language]]);
}

export function buildBilingualSrt(cues: readonly StoryboardCue[]): string {
  validateSubtitleTimeline(cues);
  return buildSrtFromLines(cues, (cue) => [cue.zh, cue.en]);
}

export function buildSubtitleOverlayHtml(cue: StoryboardCue, aspect: DemoAspect): string {
  const layout = OVERLAY_LAYOUT[aspect];
  const zh = escapeHtml(cue.zh);
  const en = escapeHtml(cue.en);

  return `<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <style>
    :root {
      --subtitle-safe-bottom: ${layout.safeBottom}px;
      --subtitle-horizontal-padding: ${layout.horizontalPadding}px;
    }

    * { box-sizing: border-box; }

    html,
    body {
      width: ${layout.width}px;
      height: ${layout.height}px;
      margin: 0;
      overflow: hidden;
      background: transparent;
    }

    body {
      color: #fff;
      font-family: ${SYSTEM_FONT_STACK};
      -webkit-font-smoothing: antialiased;
    }

    .subtitle-stage {
      position: absolute;
      right: var(--subtitle-horizontal-padding);
      bottom: var(--subtitle-safe-bottom);
      left: var(--subtitle-horizontal-padding);
      display: flex;
      justify-content: center;
      text-align: center;
    }

    .subtitle-panel {
      max-width: 100%;
      padding: 18px 30px 20px;
      border: 1px solid rgb(255 255 255 / 16%);
      border-radius: 20px;
      background: rgb(12 18 28 / 78%);
      box-shadow: 0 10px 36px rgb(0 0 0 / 30%);
      backdrop-filter: blur(12px);
    }

    .subtitle-zh {
      font-size: ${layout.zhFontSize}px;
      font-weight: 650;
      line-height: 1.3;
      letter-spacing: 0.01em;
      text-shadow: 0 2px 8px rgb(0 0 0 / 80%);
    }

    .subtitle-en {
      margin-top: 8px;
      color: rgb(255 255 255 / 88%);
      font-size: ${layout.enFontSize}px;
      font-weight: 500;
      line-height: 1.35;
      text-shadow: 0 2px 7px rgb(0 0 0 / 82%);
    }
  </style>
</head>
<body>
  <main class="subtitle-stage" aria-label="双语字幕">
    <div class="subtitle-panel">
      <div class="subtitle-zh" lang="zh-CN">${zh}</div>
      <div class="subtitle-en" lang="en">${en}</div>
    </div>
  </main>
</body>
</html>
`;
}

export async function writeSubtitleArtifacts(
  outputDir: string,
  cues: readonly StoryboardCue[] = VOICEOVER_CUES,
): Promise<SubtitleArtifactPaths> {
  validateSubtitleTimeline(cues);
  await mkdir(outputDir, { recursive: true });

  const paths: SubtitleArtifactPaths = {
    zh: join(outputDir, 'work-review.zh.srt'),
    en: join(outputDir, 'work-review.en.srt'),
    bilingual: join(outputDir, 'work-review.bilingual.srt'),
  };

  await Promise.all([
    writeFile(paths.zh, buildSrt(cues, 'zh'), 'utf8'),
    writeFile(paths.en, buildSrt(cues, 'en'), 'utf8'),
    writeFile(paths.bilingual, buildBilingualSrt(cues), 'utf8'),
  ]);

  return paths;
}

function buildSrtFromLines(
  cues: readonly StoryboardCue[],
  getLines: (cue: StoryboardCue) => readonly string[],
): string {
  const entries = cues.map((cue, index) => [
    String(index + 1),
    `${formatSrtTimestamp(cue.start)} --> ${formatSrtTimestamp(cue.end)}`,
    ...getLines(cue),
  ].join('\n'));

  return `${entries.join('\n\n')}\n`;
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  })[character] ?? character);
}
