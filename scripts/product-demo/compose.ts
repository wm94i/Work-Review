import { mkdir, readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type { Browser, BrowserContextOptions, Page } from 'playwright';

import {
  DEMO_DURATION_SECONDS,
  DEMO_FPS,
  DEMO_TOTAL_FRAMES,
  STORYBOARD,
  VOICEOVER_CUES,
} from './storyboard.ts';
import { buildSubtitleOverlayHtml } from './subtitles.ts';
import {
  discoverMediaBinaries,
  runCommand,
  type CommandResult,
  type CommandRunner,
} from './ffmpeg.ts';
import type {
  DemoAspect,
  StoryboardCue,
  StoryboardScene,
} from './types.ts';

export const ASPECT_OUTPUT = {
  '16x9': { width: 1920, height: 1080 },
  '9x16': { width: 1080, height: 1920 },
} as const satisfies Record<DemoAspect, { width: number; height: number }>;

const COVER_COPY = '回看今天，写下成果。';
const SYSTEM_FONT_STACK = [
  '-apple-system',
  'BlinkMacSystemFont',
  '"PingFang SC"',
  '"Microsoft YaHei"',
  '"Noto Sans CJK SC"',
  'sans-serif',
].join(', ');

export interface BuildCompositionOptions {
  aspect: DemoAspect;
  sceneVideoPaths: readonly string[];
  subtitlePngPaths: readonly string[];
  mixPath: string;
  outputPath: string;
  sourceDurationsSeconds?: readonly number[];
  sourceStartOffsetsSeconds?: readonly number[];
  scenes?: readonly StoryboardScene[];
  cues?: readonly StoryboardCue[];
}

export interface ComposeDemoVideoOptions extends BuildCompositionOptions {
  ffmpegPath?: string;
  runner?: CommandRunner;
}

export interface RenderSubtitleOverlayOptions {
  browser: Browser;
  aspect: DemoAspect;
  outputDir: string;
  cues?: readonly StoryboardCue[];
}

export interface RenderCoverOptions {
  browser: Browser;
  aspect: DemoAspect;
  outputPath: string;
  iconPath?: string;
}

function assertNonEmpty(value: string, label: string): void {
  if (!value.trim()) throw new Error(`${label}不能为空`);
}

function assertFinitePositive(value: number, label: string): void {
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error(`${label}必须是正有限数`);
  }
}

function fixedSeconds(value: number): string {
  return value.toFixed(3);
}

function parseCompositionValue(value: string, expectedParts: number, label: string): number[] {
  const parts = value.split(':').map(Number);
  if (parts.length !== expectedParts || parts.some((part) => !Number.isFinite(part) || part < 0)) {
    throw new Error(`${label}格式无效：${value}`);
  }
  return parts;
}

function validateCompositionOptions(
  options: BuildCompositionOptions,
  scenes: readonly StoryboardScene[],
  cues: readonly StoryboardCue[],
): void {
  if (scenes.length !== 7) throw new Error('产品演示合成必须包含七个分镜');
  if (options.sceneVideoPaths.length !== scenes.length) {
    throw new Error(`分镜视频数量必须为 ${scenes.length}`);
  }
  if (options.subtitlePngPaths.length !== cues.length) {
    throw new Error(`字幕 PNG 数量必须为 ${cues.length}`);
  }
  if (
    options.sourceDurationsSeconds
    && options.sourceDurationsSeconds.length !== scenes.length
  ) {
    throw new Error(`源视频时长数量必须为 ${scenes.length}`);
  }
  if (
    options.sourceStartOffsetsSeconds
    && options.sourceStartOffsetsSeconds.length !== scenes.length
  ) {
    throw new Error(`源视频内容起点数量必须为 ${scenes.length}`);
  }

  options.sceneVideoPaths.forEach((value, index) => assertNonEmpty(value, `分镜视频 ${index + 1}`));
  options.subtitlePngPaths.forEach((value, index) => assertNonEmpty(value, `字幕 PNG ${index + 1}`));
  options.sourceDurationsSeconds?.forEach((value, index) => (
    assertFinitePositive(value, `源视频 ${index + 1} 时长`)
  ));
  options.sourceStartOffsetsSeconds?.forEach((value, index) => {
    if (!Number.isFinite(value) || value < 0) {
      throw new Error(`源视频 ${index + 1} 内容起点必须是非负有限数`);
    }
  });
  assertNonEmpty(options.mixPath, '最终混音路径');
  assertNonEmpty(options.outputPath, '输出视频路径');
}

function buildSceneFilter(
  scene: StoryboardScene,
  aspect: DemoAspect,
  inputIndex: number,
  sourceDurationSeconds?: number,
  sourceStartOffsetSeconds = 0,
): string {
  const targetDuration = scene.end - scene.start;
  assertFinitePositive(targetDuration, `分镜 ${scene.id} 目标时长`);
  const composition = scene.composition[aspect];
  const [cropWidth, cropHeight, cropX, cropY] = parseCompositionValue(
    composition.crop,
    4,
    `分镜 ${scene.id} crop`,
  );
  const [scaleWidth, scaleHeight] = parseCompositionValue(
    composition.scale,
    2,
    `分镜 ${scene.id} scale`,
  );
  const output = ASPECT_OUTPUT[aspect];
  if (scaleWidth !== output.width || scaleHeight !== output.height) {
    throw new Error(
      `分镜 ${scene.id} 的 ${aspect} scale 必须统一为 ${output.width}:${output.height}`,
    );
  }

  const filters = [
    `trim=start=${fixedSeconds(sourceStartOffsetSeconds)}:duration=${fixedSeconds(targetDuration)}`,
    'setpts=PTS-STARTPTS',
    `fps=${DEMO_FPS}`,
  ];

  if (aspect === '9x16') {
    // 竖屏录制已是独立响应式页面；先放大到 1920 宽参考画布，
    // 再使用 STORYBOARD 为每个镜头定义的不同焦点坐标进行重构。
    filters.push('scale=1920:-2:flags=lanczos');
  }

  filters.push(
    `crop=${cropWidth}:${cropHeight}:${cropX}:${cropY}`,
    `scale=${scaleWidth}:${scaleHeight}:flags=lanczos`,
    `pad=${output.width}:${output.height}:(ow-iw)/2:(oh-ih)/2:color=black`,
    'setsar=1',
  );

  const availableDuration = sourceDurationSeconds === undefined
    ? undefined
    : Math.max(0, sourceDurationSeconds - sourceStartOffsetSeconds);
  if (availableDuration === undefined || availableDuration < targetDuration) {
    const paddingDuration = sourceDurationSeconds === undefined
      ? targetDuration
      : targetDuration - availableDuration!;
    filters.push(`tpad=stop_mode=clone:stop_duration=${fixedSeconds(paddingDuration)}`);
  }

  filters.push(
    `trim=duration=${fixedSeconds(targetDuration)}`,
    'setpts=PTS-STARTPTS',
    'format=yuv420p',
  );

  return `[${inputIndex}:v]${filters.join(',')}[scene_${inputIndex}]`;
}

function buildSubtitleFilters(
  cues: readonly StoryboardCue[],
  subtitleInputOffset: number,
  output: { width: number; height: number },
): { filters: string[]; finalLabel: string } {
  if (cues.length === 0) return { filters: [], finalLabel: 'concatv' };

  const filters: string[] = [];
  let baseLabel = 'concatv';
  cues.forEach((cue, index) => {
    const subtitleLabel = `subtitle_${index}`;
    const outputLabel = index === cues.length - 1 ? 'subtitle_done' : `overlay_${index}`;
    filters.push(
      `[${subtitleInputOffset + index}:v]format=rgba,scale=${output.width}:${output.height}:flags=lanczos[${subtitleLabel}]`,
    );
    filters.push(
      `[${baseLabel}][${subtitleLabel}]overlay=x=0:y=0:format=auto:enable='between(t,${fixedSeconds(cue.start)},${fixedSeconds(cue.end)})'[${outputLabel}]`,
    );
    baseLabel = outputLabel;
  });

  return { filters, finalLabel: 'subtitle_done' };
}

/** 构建七段视频、透明字幕和最终混音的确定性 FFmpeg 参数。 */
export function buildCompositionArgs(options: BuildCompositionOptions): string[] {
  const scenes = options.scenes ?? STORYBOARD;
  const cues = options.cues ?? VOICEOVER_CUES;
  validateCompositionOptions(options, scenes, cues);
  const output = ASPECT_OUTPUT[options.aspect];
  const args: string[] = ['-y', '-hide_banner'];

  for (const videoPath of options.sceneVideoPaths) {
    args.push('-i', videoPath);
  }
  for (const subtitlePath of options.subtitlePngPaths) {
    args.push('-loop', '1', '-framerate', String(DEMO_FPS), '-i', subtitlePath);
  }
  const audioInputIndex = scenes.length + cues.length;
  args.push('-i', options.mixPath);

  const filters = scenes.map((scene, index) => buildSceneFilter(
    scene,
    options.aspect,
    index,
    options.sourceDurationsSeconds?.[index],
    options.sourceStartOffsetsSeconds?.[index],
  ));
  filters.push(
    `${scenes.map((_, index) => `[scene_${index}]`).join('')}concat=n=${scenes.length}:v=1:a=0,trim=duration=${fixedSeconds(DEMO_DURATION_SECONDS)},setpts=PTS-STARTPTS[concatv]`,
  );
  const subtitleFilters = buildSubtitleFilters(cues, scenes.length, output);
  filters.push(...subtitleFilters.filters);
  filters.push(
    `[${subtitleFilters.finalLabel}]setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709[finalv]`,
  );

  args.push(
    '-filter_complex', filters.join(';'),
    '-map', '[finalv]',
    '-map', `${audioInputIndex}:a:0`,
    '-c:v', 'libx264',
    '-crf', '17',
    '-preset', 'slow',
    '-g', '60',
    '-keyint_min', '60',
    '-sc_threshold', '0',
    '-pix_fmt', 'yuv420p',
    '-r', String(DEMO_FPS),
    '-fps_mode', 'cfr',
    '-frames:v', String(DEMO_TOTAL_FRAMES),
    '-c:a', 'aac',
    '-profile:a', 'aac_low',
    '-b:a', '192k',
    '-ar', '48000',
    '-ac', '2',
    '-t', fixedSeconds(DEMO_DURATION_SECONDS),
    '-movflags', '+faststart',
    '-color_primaries', 'bt709',
    '-color_trc', 'bt709',
    '-colorspace', 'bt709',
    options.outputPath,
  );

  return args;
}

/** 执行最终视频合成；runner 可在测试或编排层注入。 */
export async function composeDemoVideo(
  options: ComposeDemoVideoOptions,
): Promise<CommandResult> {
  await mkdir(dirname(options.outputPath), { recursive: true });
  const runner = options.runner ?? runCommand;
  const ffmpegPath = options.ffmpegPath
    ?? (await discoverMediaBinaries({ runner })).ffmpeg;
  return runner(ffmpegPath, buildCompositionArgs(options), {
    maxCaptureBytes: 4 * 1024 * 1024,
  });
}

function createVisualContextOptions(aspect: DemoAspect): BrowserContextOptions {
  return {
    viewport: { ...ASPECT_OUTPUT[aspect] },
    deviceScaleFactor: 1,
    locale: 'zh-CN',
    timezoneId: 'Asia/Shanghai',
    reducedMotion: 'reduce',
    colorScheme: 'light',
  };
}

async function waitForFonts(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await document.fonts?.ready;
  });
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

/** 用 Playwright 把双语字幕 HTML/CSS 渲染为带 alpha 的整帧 PNG。 */
export async function renderSubtitleOverlayPngs(
  options: RenderSubtitleOverlayOptions,
): Promise<string[]> {
  const cues = options.cues ?? VOICEOVER_CUES;
  const aspectDir = join(options.outputDir, options.aspect);
  await mkdir(aspectDir, { recursive: true });
  const context = await options.browser.newContext(createVisualContextOptions(options.aspect));

  try {
    const page = await context.newPage();
    const paths: string[] = [];
    for (const [index, cue] of cues.entries()) {
      const outputPath = join(
        aspectDir,
        `subtitle-${String(index + 1).padStart(2, '0')}-${cue.id}.png`,
      );
      await page.setContent(buildSubtitleOverlayHtml(cue, options.aspect), { waitUntil: 'load' });
      await waitForFonts(page);
      await page.screenshot({
        path: outputPath,
        omitBackground: true,
        animations: 'disabled',
        caret: 'hide',
      });
      paths.push(outputPath);
    }
    return paths;
  } finally {
    await context.close();
  }
}

/** 构建横竖屏分别排版的产品封面 HTML。 */
export function buildCoverHtml(aspect: DemoAspect, iconDataUrl: string): string {
  assertNonEmpty(iconDataUrl, '封面图标数据');
  const output = ASPECT_OUTPUT[aspect];
  const portrait = aspect === '9x16';
  const iconSize = portrait ? 300 : 240;
  const titleSize = portrait ? 104 : 92;
  const taglineSize = portrait ? 48 : 42;
  const padding = portrait ? 96 : 120;

  return `<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <style>
    * { box-sizing: border-box; }
    html, body {
      width: ${output.width}px;
      height: ${output.height}px;
      margin: 0;
      overflow: hidden;
      background: transparent;
    }
    body {
      display: grid;
      place-items: center;
      color: #f8fafc;
      font-family: ${SYSTEM_FONT_STACK};
      -webkit-font-smoothing: antialiased;
    }
    .cover {
      position: relative;
      width: calc(100% - 32px);
      height: calc(100% - 32px);
      display: flex;
      flex-direction: ${portrait ? 'column' : 'row'};
      align-items: center;
      justify-content: center;
      gap: ${portrait ? 74 : 64}px;
      padding: ${padding}px;
      border-radius: 42px;
      background:
        radial-gradient(circle at 20% 18%, rgb(56 189 248 / 32%), transparent 38%),
        radial-gradient(circle at 82% 78%, rgb(129 140 248 / 30%), transparent 42%),
        linear-gradient(145deg, #07111f 0%, #101a2e 52%, #07111f 100%);
    }
    .cover::after {
      content: '';
      position: absolute;
      inset: 28px;
      border: 1px solid rgb(255 255 255 / 12%);
      border-radius: 34px;
      pointer-events: none;
    }
    .icon-shell {
      flex: 0 0 auto;
      width: ${iconSize}px;
      height: ${iconSize}px;
      padding: ${portrait ? 30 : 24}px;
      border: 1px solid rgb(255 255 255 / 20%);
      border-radius: ${portrait ? 72 : 58}px;
      background: rgb(255 255 255 / 10%);
      box-shadow: 0 34px 90px rgb(0 0 0 / 38%);
      backdrop-filter: blur(18px);
    }
    .icon-shell img { width: 100%; height: 100%; object-fit: contain; }
    .copy { text-align: ${portrait ? 'center' : 'left'}; }
    h1 {
      margin: 0;
      font-size: ${titleSize}px;
      font-weight: 760;
      line-height: 1.04;
      letter-spacing: -0.035em;
    }
    p {
      margin: ${portrait ? 34 : 28}px 0 0;
      color: rgb(226 232 240 / 88%);
      font-size: ${taglineSize}px;
      font-weight: 520;
      line-height: 1.35;
      letter-spacing: 0.02em;
    }
  </style>
</head>
<body>
  <main class="cover" aria-label="Work-Review 产品封面">
    <div class="icon-shell"><img src="${escapeHtml(iconDataUrl)}" alt="Work-Review"></div>
    <div class="copy">
      <h1>Work-Review</h1>
      <p>${COVER_COPY}</p>
    </div>
  </main>
</body>
</html>
`;
}

/** 使用 public/icon.png 和 Playwright HTML/CSS 生成封面 PNG。 */
export async function renderCoverPng(options: RenderCoverOptions): Promise<string> {
  const iconPath = options.iconPath ?? join(process.cwd(), 'public', 'icon.png');
  const iconBytes = await readFile(iconPath);
  const iconDataUrl = `data:image/png;base64,${iconBytes.toString('base64')}`;
  await mkdir(dirname(options.outputPath), { recursive: true });
  const context = await options.browser.newContext(createVisualContextOptions(options.aspect));

  try {
    const page = await context.newPage();
    await page.setContent(buildCoverHtml(options.aspect, iconDataUrl), { waitUntil: 'load' });
    await waitForFonts(page);
    await page.screenshot({
      path: options.outputPath,
      omitBackground: true,
      animations: 'disabled',
      caret: 'hide',
    });
    return options.outputPath;
  } finally {
    await context.close();
  }
}
