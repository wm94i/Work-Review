import assert from 'node:assert/strict';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import type { Browser, BrowserContextOptions } from 'playwright';

import {
  ASPECT_OUTPUT,
  buildCompositionArgs,
  buildCoverHtml,
  composeDemoVideo,
  renderCoverPng,
  renderSubtitleOverlayPngs,
} from './compose.ts';
import {
  DEMO_DURATION_SECONDS,
  DEMO_FPS,
  DEMO_TOTAL_FRAMES,
  STORYBOARD,
  VOICEOVER_CUES,
} from './storyboard.ts';
import type { CommandResult } from './ffmpeg.ts';

const ok = (): CommandResult => ({ stdout: '', stderr: '', exitCode: 0 });

function valueAfter(args: readonly string[], flag: string): string | undefined {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] : undefined;
}

function occurrenceCount(value: string, pattern: RegExp): number {
  return [...value.matchAll(pattern)].length;
}

function compositionFixture(aspect: '16x9' | '9x16') {
  return {
    aspect,
    sceneVideoPaths: STORYBOARD.map((scene) => `/tmp/work-review-demo/captures/${aspect}/${scene.id}.webm`),
    subtitlePngPaths: VOICEOVER_CUES.map((cue, index) => (
      `/tmp/work-review-demo/overlays/${aspect}/${String(index + 1).padStart(2, '0')}-${cue.id}.png`
    )),
    mixPath: '/tmp/work-review-demo/audio/mix.wav',
    outputPath: `/tmp/work-review-demo/final/work-review-${aspect}.mp4`,
    sourceStartOffsetsSeconds: STORYBOARD.map((_, index) => 1.25 + index * 0.1),
  };
}

test('横屏合成逐段定长、拼接 85 秒，并在最终缩放后按时间窗叠加七条字幕', () => {
  const fixture = compositionFixture('16x9');
  const sourceDurationsSeconds = STORYBOARD.map((scene, index) => (
    fixture.sourceStartOffsetsSeconds[index]!
    + (scene.end - scene.start)
    + (index === 2 ? -0.5 : 0.5)
  ));
  const args = buildCompositionArgs({ ...fixture, sourceDurationsSeconds });
  const filter = valueAfter(args, '-filter_complex') ?? '';
  const chains = filter.split(';');

  STORYBOARD.forEach((scene, index) => {
    const duration = (scene.end - scene.start).toFixed(3);
    const chain = chains.find((value) => value.startsWith(`[${index}:v]`)) ?? '';
    const start = fixture.sourceStartOffsetsSeconds[index]!.toFixed(3).replace('.', '\\.');
    assert.match(chain, new RegExp(`trim=start=${start}:duration=${duration.replace('.', '\\.')}`));
    assert.match(chain, /setpts=PTS-STARTPTS/);
    assert.match(chain, new RegExp(`fps=${DEMO_FPS}`));
    assert.match(chain, /crop=1920:1080:0:0/);
    assert.match(chain, /scale=1920:1080:flags=lanczos/);
    assert.match(chain, /pad=1920:1080:\(ow-iw\)\/2:\(oh-ih\)\/2/);
    assert.match(chain, new RegExp(`trim=duration=${duration.replace('.', '\\.')}`));
    if (index === 2) assert.match(chain, /tpad=stop_mode=clone/);
    else assert.doesNotMatch(chain, /tpad=stop_mode=clone/);
  });

  assert.match(
    filter,
    /\[scene_0\]\[scene_1\]\[scene_2\]\[scene_3\]\[scene_4\]\[scene_5\]\[scene_6\]concat=n=7:v=1:a=0,trim=duration=85\.000,setpts=PTS-STARTPTS\[concatv\]/,
  );
  assert.equal(occurrenceCount(filter, /overlay=x=0:y=0:format=auto:enable='/g), VOICEOVER_CUES.length);
  VOICEOVER_CUES.forEach((cue, index) => {
    assert.match(
      filter,
      new RegExp(`\\[${STORYBOARD.length + index}:v\\]format=rgba,scale=1920:1080:flags=lanczos\\[subtitle_${index}\\]`),
    );
    assert.ok(filter.includes(`between(t,${cue.start.toFixed(3)},${cue.end.toFixed(3)})`));
  });

  assert.equal(occurrenceCount(args.join('\n'), /^-loop$/gm), VOICEOVER_CUES.length);
  assert.equal(valueAfter(args, '-map'), '[finalv]');
  const firstMap = args.indexOf('-map');
  assert.equal(args[firstMap + 3], `${STORYBOARD.length + VOICEOVER_CUES.length}:a:0`);
});

test('未提供源时长时保守追加末帧并在目标时长再次裁切', () => {
  const args = buildCompositionArgs(compositionFixture('16x9'));
  const filter = valueAfter(args, '-filter_complex') ?? '';
  const chains = filter.split(';');

  STORYBOARD.forEach((scene, index) => {
    const duration = (scene.end - scene.start).toFixed(3);
    const chain = chains.find((value) => value.startsWith(`[${index}:v]`)) ?? '';
    assert.match(chain, new RegExp(`tpad=stop_mode=clone:stop_duration=${duration.replace('.', '\\.')}`));
    assert.match(chain, new RegExp(`trim=duration=${duration.replace('.', '\\.')}`));
  });
});

test('源时长按内容起点扣除预卷后判断是否需要补帧', () => {
  const fixture = compositionFixture('16x9');
  const args = buildCompositionArgs({
    ...fixture,
    sourceDurationsSeconds: STORYBOARD.map((scene, index) => (
      fixture.sourceStartOffsetsSeconds[index]! + (scene.end - scene.start) - (index === 0 ? 0.25 : -0.25)
    )),
  });
  const chains = (valueAfter(args, '-filter_complex') ?? '').split(';');
  assert.match(chains[0] ?? '', /tpad=stop_mode=clone:stop_duration=0\.250/);
  assert.doesNotMatch(chains[1] ?? '', /tpad=stop_mode=clone/);
});

test('竖屏严格逐镜头使用 STORYBOARD 的独立 crop/scale，不做 concat 后全局裁切', () => {
  const fixture = compositionFixture('9x16');
  const args = buildCompositionArgs({
    ...fixture,
    sourceDurationsSeconds: STORYBOARD.map((scene) => scene.end - scene.start + 0.25),
  });
  const filter = valueAfter(args, '-filter_complex') ?? '';
  const chains = filter.split(';');

  STORYBOARD.forEach((scene, index) => {
    const chain = chains.find((value) => value.startsWith(`[${index}:v]`)) ?? '';
    assert.match(chain, new RegExp(`crop=${scene.composition['9x16'].crop.replaceAll(':', '\\:')}`));
    assert.match(chain, new RegExp(`scale=${scene.composition['9x16'].scale.replaceAll(':', '\\:')}:flags=lanczos`));
    assert.match(chain, /pad=1080:1920:\(ow-iw\)\/2:\(oh-ih\)\/2/);
    assert.match(chain, /setsar=1/);
  });

  const portraitCrops = STORYBOARD.map((scene) => scene.composition['9x16'].crop);
  assert.ok(new Set(portraitCrops).size > 1, '竖屏分镜必须具有不同构图');
  assert.doesNotMatch(filter, /\[concatv\]crop=/);
  assert.deepEqual(ASPECT_OUTPUT['9x16'], { width: 1080, height: 1920 });
});

test('最终 MP4 固定 H.264 CRF17 slow GOP60、AAC-LC 48k、faststart、Rec.709 和 2550 帧', () => {
  const fixture = compositionFixture('16x9');
  const args = buildCompositionArgs(fixture);

  assert.equal(valueAfter(args, '-c:v'), 'libx264');
  assert.equal(valueAfter(args, '-crf'), '17');
  assert.equal(valueAfter(args, '-preset'), 'slow');
  assert.equal(valueAfter(args, '-g'), '60');
  assert.equal(valueAfter(args, '-keyint_min'), '60');
  assert.equal(valueAfter(args, '-pix_fmt'), 'yuv420p');
  assert.equal(valueAfter(args, '-r'), String(DEMO_FPS));
  assert.equal(valueAfter(args, '-fps_mode'), 'cfr');
  assert.equal(valueAfter(args, '-frames:v'), String(DEMO_TOTAL_FRAMES));
  assert.equal(valueAfter(args, '-c:a'), 'aac');
  assert.equal(valueAfter(args, '-profile:a'), 'aac_low');
  assert.equal(valueAfter(args, '-ar'), '48000');
  assert.equal(valueAfter(args, '-ac'), '2');
  assert.equal(valueAfter(args, '-t'), DEMO_DURATION_SECONDS.toFixed(3));
  assert.equal(valueAfter(args, '-movflags'), '+faststart');
  assert.equal(valueAfter(args, '-color_primaries'), 'bt709');
  assert.equal(valueAfter(args, '-color_trc'), 'bt709');
  assert.equal(valueAfter(args, '-colorspace'), 'bt709');
  const filter = valueAfter(args, '-filter_complex') ?? '';
  assert.match(
    filter,
    /setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709\[finalv\]$/u,
  );
  assert.equal(args.at(-1), fixture.outputPath);
  assert.ok(args.includes(fixture.mixPath));
  assert.ok(!args.includes('subtitles'));
  assert.ok(!args.includes('ass'));
  assert.ok(!args.includes('drawtext'));
});

test('透明双语字幕 PNG 通过可注入浏览器按目标画幅渲染', async () => {
  const outputDir = await mkdtemp(join(tmpdir(), 'work-review-subtitle-overlays-'));
  const calls = createBrowserSpy();
  const cue = VOICEOVER_CUES[0]!;

  const paths = await renderSubtitleOverlayPngs({
    browser: calls.browser,
    aspect: '9x16',
    outputDir,
    cues: [cue],
  });

  assert.deepEqual(calls.contextOptions[0]?.viewport, { width: 1080, height: 1920 });
  assert.equal(paths.length, 1);
  assert.equal(paths[0], join(outputDir, '9x16', 'subtitle-01-hook.png'));
  assert.match(calls.html[0] ?? '', /width: 1080px/);
  assert.match(calls.html[0] ?? '', /height: 1920px/);
  assert.match(calls.html[0] ?? '', new RegExp(cue.zh));
  assert.match(calls.html[0] ?? '', new RegExp(cue.en.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  assert.deepEqual(calls.screenshots[0], {
    path: paths[0],
    omitBackground: true,
    animations: 'disabled',
    caret: 'hide',
  });
  assert.equal(calls.closed, 1);
});

test('封面用 public 图标数据和 HTML/CSS 独立构图，不依赖 FFmpeg 文字滤镜', async () => {
  const outputDir = await mkdtemp(join(tmpdir(), 'work-review-cover-'));
  const iconPath = join(outputDir, 'icon.png');
  const outputPath = join(outputDir, 'cover-16x9.png');
  await writeFile(iconPath, Buffer.from([0x89, 0x50, 0x4e, 0x47]));
  const calls = createBrowserSpy();

  const html = buildCoverHtml('16x9', 'data:image/png;base64,iVBORw==');
  assert.match(html, /Work-Review/);
  assert.match(html, /回看今天，写下成果。/);
  assert.match(html, /data:image\/png;base64,iVBORw==/);
  assert.match(html, /width: 1920px/);
  assert.match(html, /height: 1080px/);
  assert.match(html, /width: calc\(100% - 32px\)/);
  assert.match(html, /height: calc\(100% - 32px\)/);
  assert.match(html, /border-radius: 42px/);
  assert.doesNotMatch(html, /drawtext|subtitles|\.ass/u);

  await renderCoverPng({
    browser: calls.browser,
    aspect: '16x9',
    iconPath,
    outputPath,
  });

  assert.match(calls.html[0] ?? '', /data:image\/png;base64,iVBORw==/);
  assert.equal(calls.screenshots[0]?.path, outputPath);
  assert.equal(calls.screenshots[0]?.omitBackground, true);
  assert.equal(calls.closed, 1);
});

test('合成执行器可注入 runner，路径始终作为独立 argv 传递', async () => {
  const outputDir = await mkdtemp(join(tmpdir(), 'work-review-compose-'));
  const fixture = {
    ...compositionFixture('16x9'),
    outputPath: join(outputDir, 'final clip;$(touch nope).mp4'),
  };
  const calls: Array<{ binary: string; args: readonly string[] }> = [];

  await composeDemoVideo({
    ...fixture,
    ffmpegPath: '/mock/bin/ffmpeg',
    runner: async (binary, args) => {
      calls.push({ binary, args });
      return ok();
    },
  });

  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.binary, '/mock/bin/ffmpeg');
  assert.equal(calls[0]?.args.at(-1), fixture.outputPath);
  assert.ok(calls[0]?.args.includes(fixture.outputPath));
  assert.ok(!calls[0]?.args.some((value) => value.includes(`ffmpeg ${fixture.outputPath}`)));
});

function createBrowserSpy(): {
  browser: Browser;
  contextOptions: BrowserContextOptions[];
  html: string[];
  screenshots: Array<Record<string, unknown>>;
  closed: number;
} {
  const state = {
    contextOptions: [] as BrowserContextOptions[],
    html: [] as string[],
    screenshots: [] as Array<Record<string, unknown>>,
    closed: 0,
  };
  const browser = {
    async newContext(options: BrowserContextOptions) {
      state.contextOptions.push(options);
      return {
        async newPage() {
          return {
            async setContent(html: string) {
              state.html.push(html);
            },
            async evaluate() {
              return undefined;
            },
            async screenshot(options: Record<string, unknown>) {
              state.screenshots.push(options);
              return Buffer.alloc(0);
            },
          };
        },
        async close() {
          state.closed += 1;
        },
      };
    },
  } as unknown as Browser;

  return {
    browser,
    contextOptions: state.contextOptions,
    html: state.html,
    screenshots: state.screenshots,
    get closed() { return state.closed; },
  };
}
