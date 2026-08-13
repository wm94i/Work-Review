import assert from 'node:assert/strict';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { DEMO_DURATION_SECONDS, VOICEOVER_CUES } from './storyboard.ts';
import {
  buildBilingualSrt,
  buildSrt,
  buildSubtitleOverlayHtml,
  formatSrtTimestamp,
  validateSubtitleTimeline,
  writeSubtitleArtifacts,
} from './subtitles.ts';
import type { StoryboardCue } from './types.ts';

interface ParsedSrtEntry {
  index: number;
  start: string;
  end: string;
  lines: string[];
}

function parseSrt(content: string): ParsedSrtEntry[] {
  return content
    .trim()
    .split(/\r?\n\r?\n/)
    .map((block) => {
      const [indexLine, timeLine, ...lines] = block.split(/\r?\n/);
      assert.ok(indexLine);
      assert.ok(timeLine);
      const [start, end] = timeLine.split(' --> ');
      assert.ok(start);
      assert.ok(end);
      return { index: Number(indexLine), start, end, lines };
    });
}

test('将秒数格式化为标准 SRT 时间戳', () => {
  assert.equal(formatSrtTimestamp(0), '00:00:00,000');
  assert.equal(formatSrtTimestamp(7.04), '00:00:07,040');
  assert.equal(formatSrtTimestamp(65.004), '00:01:05,004');
  assert.equal(formatSrtTimestamp(3599.9996), '01:00:00,000');
  assert.throws(() => formatSrtTimestamp(-0.001), /非负有限数/);
  assert.throws(() => formatSrtTimestamp(Number.NaN), /非负有限数/);
});

test('生成数量与时间轴一致的中文和英文 SRT', () => {
  const zhEntries = parseSrt(buildSrt(VOICEOVER_CUES, 'zh'));
  const enEntries = parseSrt(buildSrt(VOICEOVER_CUES, 'en'));

  assert.equal(zhEntries.length, VOICEOVER_CUES.length);
  assert.equal(enEntries.length, VOICEOVER_CUES.length);
  assert.deepEqual(
    zhEntries.map(({ index, start, end }) => ({ index, start, end })),
    enEntries.map(({ index, start, end }) => ({ index, start, end })),
  );
  assert.match(zhEntries[0]?.lines.join('\n') ?? '', /下班时/);
  assert.match(enEntries[0]?.lines.join('\n') ?? '', /At the end of the day/);
});

test('双语 SRT 每条均为中文在上、英文在下', () => {
  const entries = parseSrt(buildBilingualSrt(VOICEOVER_CUES));

  assert.equal(entries.length, VOICEOVER_CUES.length);
  entries.forEach((entry, index) => {
    assert.deepEqual(entry.lines, [VOICEOVER_CUES[index]?.zh, VOICEOVER_CUES[index]?.en]);
  });
  assert.match(buildBilingualSrt(VOICEOVER_CUES), /下班时[\s\S]*At the end of the day/);
});

test('校验字幕时间轴单调、不重叠且不超过视频时长', () => {
  assert.doesNotThrow(() => validateSubtitleTimeline(VOICEOVER_CUES, DEMO_DURATION_SECONDS));

  const invalidCases: Array<{ cue: StoryboardCue[]; message: RegExp }> = [
    {
      cue: [{ id: 'negative', start: -0.1, end: 1, zh: '中', en: 'en' }],
      message: /不得早于 0 秒/,
    },
    {
      cue: [{ id: 'empty', start: 1, end: 1, zh: '中', en: 'en' }],
      message: /结束时间必须晚于开始时间/,
    },
    {
      cue: [{ id: 'late', start: 84, end: 85.1, zh: '中', en: 'en' }],
      message: /超过视频时长/,
    },
    {
      cue: [
        { id: 'first', start: 1, end: 3, zh: '中一', en: 'one' },
        { id: 'second', start: 2.9, end: 4, zh: '中二', en: 'two' },
      ],
      message: /与上一条重叠/,
    },
  ];

  for (const invalid of invalidCases) {
    assert.throws(
      () => validateSubtitleTimeline(invalid.cue, DEMO_DURATION_SECONDS),
      invalid.message,
    );
  }
});

test('横竖屏字幕层使用透明背景、系统字体和不同安全边距', () => {
  const cue = VOICEOVER_CUES[0];
  assert.ok(cue);

  const landscape = buildSubtitleOverlayHtml(cue, '16x9');
  const portrait = buildSubtitleOverlayHtml(cue, '9x16');

  for (const html of [landscape, portrait]) {
    assert.match(html, /background:\s*transparent/);
    assert.match(html, /-apple-system/);
    assert.match(html, /"PingFang SC"/);
    assert.match(html, /"Microsoft YaHei"/);
    assert.match(html, /text-shadow:/);
    assert.ok(html.indexOf(cue.zh) < html.indexOf(cue.en));
  }

  assert.match(landscape, /--subtitle-safe-bottom:\s*84px/);
  assert.match(landscape, /\.subtitle-zh\s*\{[^}]*font-size:\s*54px/s);
  assert.match(landscape, /\.subtitle-en\s*\{[^}]*font-size:\s*32px/s);

  assert.match(portrait, /--subtitle-safe-bottom:\s*240px/);
  assert.match(portrait, /\.subtitle-zh\s*\{[^}]*font-size:\s*50px/s);
  assert.match(portrait, /\.subtitle-en\s*\{[^}]*font-size:\s*30px/s);
});

test('字幕层转义文本，避免字幕内容破坏 HTML', () => {
  const html = buildSubtitleOverlayHtml(
    {
      id: 'escape',
      start: 0,
      end: 1,
      zh: '<中文 & 字幕>',
      en: '"English" <script>',
    },
    '16x9',
  );

  assert.match(html, /&lt;中文 &amp; 字幕&gt;/);
  assert.match(html, /&quot;English&quot; &lt;script&gt;/);
  assert.doesNotMatch(html, /<script>/);
});

test('写出中文、英文和双语三个 UTF-8 SRT 文件', async () => {
  const outputDir = await mkdtemp(join(tmpdir(), 'work-review-subtitles-'));
  const paths = await writeSubtitleArtifacts(outputDir, VOICEOVER_CUES);

  assert.deepEqual(Object.keys(paths).sort(), ['bilingual', 'en', 'zh']);
  assert.match(await readFile(paths.zh, 'utf8'), /下班时/);
  assert.match(await readFile(paths.en, 'utf8'), /At the end of the day/);
  assert.match(await readFile(paths.bilingual, 'utf8'), /下班时[\s\S]*At the end of the day/);
});
