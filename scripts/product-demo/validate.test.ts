import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import {
  lstat,
  mkdtemp,
  mkdir,
  readFile,
  readdir,
  stat,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

import type { MediaProbe } from './ffmpeg.ts';
import {
  buildDeliveryManifest,
  formatSha256Sums,
  parsePngDimensions,
  parseSrtArtifact,
  sha256Hex,
  validateCoverArtifact,
  validateDelivery,
  validateMediaArtifact,
  validateSubtitleArtifact,
  writeManifest,
  type KeyframeOcrFrame,
} from './validate.ts';

const KEYFRAME_ASPECTS = ['16x9', '9x16'] as const;
const KEYFRAME_SCENES = ['hook', 'timeline', 'report', 'assistant', 'privacy', 'export', 'outro'] as const;
const KEYFRAME_PHASES = ['start', 'action', 'end'] as const;

const SAFE_SRT = [
  '1',
  '00:00:00,350 --> 00:00:06,650',
  '下班时，你还记得今天到底做了什么吗？',
  '',
  '2',
  '00:00:07,350 --> 00:00:18,650',
  '当天轨迹按应用、页面和时间片整理。',
  '',
].join('\n');

function validProbe(width = 1920, height = 1080): MediaProbe {
  return {
    durationSeconds: 85,
    formatName: 'mov,mp4,m4a,3gp,3g2,mj2',
    video: {
      codec: 'h264',
      pixelFormat: 'yuv420p',
      width,
      height,
      frameRate: 30,
      averageFrameRate: 30,
      frameCount: 2550,
      durationSeconds: 85,
    },
    audio: {
      codec: 'aac',
      sampleRate: 48_000,
      channels: 2,
      channelLayout: 'stereo',
      durationSeconds: 85,
    },
  };
}

function pngHeader(width: number, height: number): Buffer {
  const buffer = Buffer.alloc(24);
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]).copy(buffer, 0);
  buffer.writeUInt32BE(13, 8);
  buffer.write('IHDR', 12, 'ascii');
  buffer.writeUInt32BE(width, 16);
  buffer.writeUInt32BE(height, 20);
  return buffer;
}

async function createDeliveryFixture(): Promise<{
  artifactDir: string;
  logFiles: string[];
  markdownFiles: string[];
  sourceTextFiles: string[];
  privacyObjects: Array<{ source: string; value: unknown }>;
  runKeyframeOcr: (frames: readonly KeyframeOcrFrame[]) => Promise<{
    available: true;
    engine: string;
    evidence: Array<{ relativePath: string; text: string }>;
  }>;
}> {
  const artifactDir = await mkdtemp(path.join(tmpdir(), 'work-review-validate-'));
  await mkdir(path.join(artifactDir, 'intermediate', 'exports'), { recursive: true });
  await Promise.all(KEYFRAME_ASPECTS.map((aspect) => (
    mkdir(path.join(artifactDir, 'intermediate', aspect), { recursive: true })
  )));

  const keyframeFiles = KEYFRAME_ASPECTS.flatMap((aspect) => (
    KEYFRAME_SCENES.flatMap((scene) => (
      KEYFRAME_PHASES.map((phase) => `intermediate/${aspect}/${scene}-${phase}.png`)
    ))
  ));

  await Promise.all([
    writeFile(path.join(artifactDir, 'work-review-demo-16x9.mp4'), 'fake-horizontal-video'),
    writeFile(path.join(artifactDir, 'work-review-demo-9x16.mp4'), 'fake-vertical-video'),
    writeFile(path.join(artifactDir, 'work-review-demo-zh.srt'), SAFE_SRT),
    writeFile(path.join(artifactDir, 'work-review-demo-en.srt'), SAFE_SRT.replaceAll('下班时，你还记得今天到底做了什么吗？', 'Can you still recall what you accomplished today?').replaceAll('当天轨迹按应用、页面和时间片整理。', 'Your day is organized by apps, pages, and time blocks.')),
    writeFile(path.join(artifactDir, 'work-review-demo-zh-en.srt'), SAFE_SRT.replaceAll('下班时，你还记得今天到底做了什么吗？', '下班时，你还记得今天到底做了什么吗？\nCan you still recall what you accomplished today?').replaceAll('当天轨迹按应用、页面和时间片整理。', '当天轨迹按应用、页面和时间片整理。\nYour day is organized by apps, pages, and time blocks.')),
    writeFile(path.join(artifactDir, 'work-review-demo-cover-16x9.png'), pngHeader(1920, 1080)),
    writeFile(path.join(artifactDir, 'work-review-demo-cover-9x16.png'), pngHeader(1080, 1920)),
    writeFile(path.join(artifactDir, 'intermediate', 'capture.log'), '仅访问 http://127.0.0.1:5173/#/report\n'),
    writeFile(path.join(artifactDir, 'intermediate', 'rendered.html'), '<main>Aurora Board 产品演示</main>\n'),
    writeFile(path.join(artifactDir, 'intermediate', 'exports', '2026-08-12.md'), '# Aurora Board 日报\n\n今日完成导出流程。\n'),
    ...keyframeFiles.map((relativePath) => (
      writeFile(path.join(artifactDir, relativePath), pngHeader(4, 4))
    )),
  ]);

  return {
    artifactDir,
    logFiles: ['intermediate/capture.log'],
    markdownFiles: ['intermediate/exports/2026-08-12.md'],
    sourceTextFiles: ['intermediate/rendered.html'],
    privacyObjects: [{ source: 'demo-fixtures', value: { safeRoot: '/tmp/work-review-demo/' } }],
    runKeyframeOcr: async (frames) => ({
      available: true,
      engine: 'unit-test-ocr',
      evidence: frames.map((frame) => ({
        relativePath: frame.relativePath,
        text: `安全演示画面 ${frame.aspect} ${frame.scene} ${frame.phase}`,
      })),
    }),
  };
}

test('验证横屏与竖屏媒体的完整目标规格', () => {
  const horizontal = validateMediaArtifact({
    aspect: '16x9',
    relativePath: 'work-review-demo-16x9.mp4',
    sizeBytes: 1024,
    probe: validProbe(),
  });
  const vertical = validateMediaArtifact({
    aspect: '9x16',
    relativePath: 'work-review-demo-9x16.mp4',
    sizeBytes: 2048,
    probe: validProbe(1080, 1920),
  });

  assert.deepEqual(
    [horizontal.width, horizontal.height, horizontal.frameRate, horizontal.frameCount],
    [1920, 1080, 30, 2550],
  );
  assert.deepEqual(
    [vertical.width, vertical.height, vertical.frameRate, vertical.frameCount],
    [1080, 1920, 30, 2550],
  );
  assert.equal(horizontal.videoCodec, 'h264');
  assert.equal(horizontal.audioCodec, 'aac');
  assert.equal(horizontal.pixelFormat, 'yuv420p');
});

test('音轨时长必须接近固定 85 秒并允许一帧误差', () => {
  const boundaryProbe = validProbe();
  boundaryProbe.audio!.durationSeconds = 85 - (1 / 30);
  assert.doesNotThrow(() => validateMediaArtifact({
    aspect: '16x9',
    relativePath: 'work-review-demo-16x9.mp4',
    sizeBytes: 1,
    probe: boundaryProbe,
  }));

  const shortProbe = validProbe();
  shortProbe.audio!.durationSeconds = 85 - (1 / 30) - 0.001;
  assert.throws(
    () => validateMediaArtifact({
      aspect: '16x9',
      relativePath: 'work-review-demo-16x9.mp4',
      sizeBytes: 1,
      probe: shortProbe,
    }),
    /音频时长.*85 秒.*一帧误差/,
  );
});

test('媒体验证明确报告时长、CFR、帧数、分辨率、编码、音轨和像素格式错误', () => {
  const cases: Array<{ name: string; mutate: (probe: MediaProbe) => void; expected: RegExp }> = [
    { name: '时长', mutate: (probe) => { probe.durationSeconds = 84.9; }, expected: /时长.*85/ },
    { name: '视频流时长', mutate: (probe) => { probe.video!.durationSeconds = 84.9; }, expected: /视频流时长.*85/ },
    { name: '视频流时长缺失', mutate: (probe) => { probe.video!.durationSeconds = null; }, expected: /视频流时长.*缺失/ },
    { name: '标称帧率', mutate: (probe) => { probe.video!.frameRate = 29.97; }, expected: /恒定 30fps|标称帧率/ },
    { name: '平均帧率', mutate: (probe) => { probe.video!.averageFrameRate = 29.5; }, expected: /恒定 30fps|平均帧率/ },
    { name: '帧数', mutate: (probe) => { probe.video!.frameCount = 2549; }, expected: /2550 帧/ },
    { name: '缺失帧数', mutate: (probe) => { probe.video!.frameCount = null; }, expected: /准确帧数|count_frames/ },
    { name: '分辨率', mutate: (probe) => { probe.video!.width = 1280; }, expected: /1920×1080/ },
    { name: '视频编码', mutate: (probe) => { probe.video!.codec = 'hevc'; }, expected: /H\.264/ },
    { name: '缺少音轨', mutate: (probe) => { probe.audio = null; }, expected: /音轨/ },
    { name: '音频编码', mutate: (probe) => { probe.audio!.codec = 'opus'; }, expected: /AAC/ },
    { name: '像素格式', mutate: (probe) => { probe.video!.pixelFormat = 'yuv444p'; }, expected: /yuv420p/ },
    { name: '音频残留', mutate: (probe) => { probe.video!.durationSeconds = 84.98; probe.audio!.durationSeconds = 85.02; }, expected: /音频.*超出/ },
    { name: '音频时长缺失', mutate: (probe) => { probe.audio!.durationSeconds = null; }, expected: /音频时长.*缺失/ },
    { name: '容器', mutate: (probe) => { probe.formatName = 'matroska,webm'; }, expected: /MP4/ },
  ];

  for (const item of cases) {
    const probe = structuredClone(validProbe());
    item.mutate(probe);
    assert.throws(
      () => validateMediaArtifact({
        aspect: '16x9',
        relativePath: 'work-review-demo-16x9.mp4',
        sizeBytes: 1,
        probe,
      }),
      item.expected,
      item.name,
    );
  }
});

test('解析并验证 SRT 时间线，拒绝越界、重叠和倒序', () => {
  const parsed = parseSrtArtifact(SAFE_SRT, 'zh.srt');
  assert.equal(parsed.length, 2);
  assert.deepEqual(parsed.map((cue) => [cue.startMilliseconds, cue.endMilliseconds]), [
    [350, 6650],
    [7350, 18650],
  ]);
  assert.equal(validateSubtitleArtifact(SAFE_SRT, 'zh.srt').cueCount, 2);

  assert.throws(
    () => validateSubtitleArtifact(SAFE_SRT.replace('00:00:18,650', '00:01:25,001'), 'overflow.srt'),
    /超过.*85|越界/,
  );
  assert.throws(
    () => validateSubtitleArtifact(SAFE_SRT.replace('00:00:07,350', '00:00:06,000'), 'overlap.srt'),
    /重叠|倒序/,
  );
  assert.throws(
    () => validateSubtitleArtifact('1\nnot-a-timecode\n字幕\n', 'broken.srt'),
    /时间码|格式/,
  );
});

test('解析 PNG 封面尺寸并按画幅验证', () => {
  assert.deepEqual(parsePngDimensions(pngHeader(1920, 1080)), { width: 1920, height: 1080 });
  assert.deepEqual(
    validateCoverArtifact(pngHeader(1080, 1920), '9x16', 'cover.png'),
    { width: 1080, height: 1920 },
  );
  assert.throws(
    () => validateCoverArtifact(pngHeader(1000, 1000), '16x9', 'bad-cover.png'),
    /1920×1080/,
  );
  assert.throws(() => parsePngDimensions(Buffer.from('not-png')), /PNG/);
});

test('完整交付验证检查媒体、三份 SRT 对齐、非空文件、封面、日志、虚拟 Markdown 与隐私', async () => {
  const fixture = await createDeliveryFixture();
  const result = await validateDelivery({
    ...fixture,
    probeMediaFile: async (filePath) => filePath.includes('9x16')
      ? validProbe(1080, 1920)
      : validProbe(),
  });

  assert.equal(result.files.length, 52);
  assert.equal(result.media['16x9'].frameCount, 2550);
  assert.equal(result.media['9x16'].height, 1920);
  assert.deepEqual(result.subtitleTimelineMilliseconds, [[350, 6650], [7350, 18650]]);
  assert.deepEqual(result.manualChecks, []);
  assert.deepEqual(result.keyframeOcr, { engine: 'unit-test-ocr', evidenceCount: 42 });
  assert.equal(result.integrityVerified, false);
});


test('关键帧 OCR 未接线或自动 OCR 明确不可用时阻断交付', async () => {
  const fixture = await createDeliveryFixture();
  const { runKeyframeOcr: _runKeyframeOcr, ...withoutOcr } = fixture;
  const probeMediaFile = async (filePath: string) => filePath.includes('9x16')
    ? validProbe(1080, 1920)
    : validProbe();

  await assert.rejects(
    validateDelivery({ ...withoutOcr, probeMediaFile }),
    /自动 OCR 不可用|未接线.*OCR/,
  );
  await assert.rejects(
    validateDelivery({
      ...fixture,
      probeMediaFile,
      runKeyframeOcr: async () => ({
        available: false,
        reason: '测试环境没有可用的自动 OCR 引擎',
      }),
    }),
    /自动 OCR 不可用.*测试环境没有可用的自动 OCR 引擎/,
  );
});

test('关键帧 OCR 必须覆盖横竖屏七镜头的 start、action、end，拒绝缺失和重复证据', async () => {
  const fixture = await createDeliveryFixture();
  const probeMediaFile = async (filePath: string) => filePath.includes('9x16')
    ? validProbe(1080, 1920)
    : validProbe();

  await assert.rejects(
    validateDelivery({
      ...fixture,
      probeMediaFile,
      runKeyframeOcr: async (frames) => ({
        available: true,
        engine: 'unit-test-ocr',
        evidence: frames.slice(0, -1).map((frame) => ({
          relativePath: frame.relativePath,
          text: '安全演示画面',
        })),
      }),
    }),
    /OCR 证据缺失.*9x16.*outro-end\.png/,
  );

  await assert.rejects(
    validateDelivery({
      ...fixture,
      probeMediaFile,
      runKeyframeOcr: async (frames) => ({
        available: true,
        engine: 'unit-test-ocr',
        evidence: [
          ...frames.map((frame) => ({ relativePath: frame.relativePath, text: '安全演示画面' })),
          { relativePath: frames[0]!.relativePath, text: '重复证据' },
        ],
      }),
    }),
    /OCR 证据.*重复/,
  );
});

test('扫描关键帧 OCR 文本，并继续扫描日志、Markdown、源码文本、旧 OCR 文本与隐私对象', async () => {
  const probeMediaFile = async (filePath: string) => filePath.includes('9x16')
    ? validProbe(1080, 1920)
    : validProbe();

  const ocrFixture = await createDeliveryFixture();
  await assert.rejects(
    validateDelivery({
      ...ocrFixture,
      probeMediaFile,
      runKeyframeOcr: async (frames) => ({
        available: true,
        engine: 'unit-test-ocr',
        evidence: frames.map((frame, index) => ({
          relativePath: frame.relativePath,
          text: index === 0 ? '来自 /Users/alice/Desktop/private.png' : '安全演示画面',
        })),
      }),
    }),
    /隐私|macos-home|敏感/,
  );

  const markdownFixture = await createDeliveryFixture();
  await writeFile(
    path.join(markdownFixture.artifactDir, markdownFixture.markdownFiles[0]!),
    '联系 alice@example.com',
  );
  await assert.rejects(
    validateDelivery({ ...markdownFixture, probeMediaFile }),
    /隐私|email|敏感/,
  );

  const sourceFixture = await createDeliveryFixture();
  await writeFile(
    path.join(sourceFixture.artifactDir, sourceFixture.sourceTextFiles[0]!),
    '<main>https://github.com/example/private</main>',
  );
  await assert.rejects(
    validateDelivery({ ...sourceFixture, probeMediaFile }),
    /隐私|forbidden-domain|敏感/,
  );

  const legacyOcrFixture = await createDeliveryFixture();
  const legacyOcrPath = 'intermediate/legacy-ocr.txt';
  await writeFile(path.join(legacyOcrFixture.artifactDir, legacyOcrPath), 'Bearer abcdefghijklmnop');
  await assert.rejects(
    validateDelivery({
      ...legacyOcrFixture,
      probeMediaFile,
      ocrTextFiles: [legacyOcrPath],
    }),
    /隐私|bearer-token|敏感/,
  );

  const objectFixture = await createDeliveryFixture();
  await assert.rejects(
    validateDelivery({
      ...objectFixture,
      probeMediaFile,
      privacyObjects: [{ source: 'demo-fixtures', value: { email: 'alice@example.com' } }],
    }),
    /隐私|email|敏感/,
  );
});

test('完整交付验证拒绝空文件、SRT 时间轴漂移和敏感日志', async () => {
  const emptyFixture = await createDeliveryFixture();
  await writeFile(path.join(emptyFixture.artifactDir, 'work-review-demo-en.srt'), '');
  await assert.rejects(
    validateDelivery({
      ...emptyFixture,
      probeMediaFile: async (filePath) => filePath.includes('9x16')
        ? validProbe(1080, 1920)
        : validProbe(),
    }),
    /非空|为空/,
  );

  const driftFixture = await createDeliveryFixture();
  const englishPath = path.join(driftFixture.artifactDir, 'work-review-demo-en.srt');
  const english = await readFile(englishPath, 'utf8');
  await writeFile(englishPath, english.replace('00:00:07,350', '00:00:07,450'));
  await assert.rejects(
    validateDelivery({
      ...driftFixture,
      probeMediaFile: async (filePath) => filePath.includes('9x16')
        ? validProbe(1080, 1920)
        : validProbe(),
    }),
    /时间轴.*一致|漂移/,
  );

  const privacyFixture = await createDeliveryFixture();
  await writeFile(
    path.join(privacyFixture.artifactDir, privacyFixture.logFiles[0]),
    '截图来自 /Users/alice/Desktop/private.png',
  );
  await assert.rejects(
    validateDelivery({
      ...privacyFixture,
      probeMediaFile: async (filePath) => filePath.includes('9x16')
        ? validProbe(1080, 1920)
        : validProbe(),
    }),
    /隐私|macos-home|敏感/,
  );
});

test('交付路径不得逃逸产物目录，日志和虚拟 Markdown 都必须提供', async () => {
  const fixture = await createDeliveryFixture();
  const probeMediaFile = async (filePath: string) => filePath.includes('9x16')
    ? validProbe(1080, 1920)
    : validProbe();

  await assert.rejects(
    validateDelivery({ ...fixture, logFiles: ['../outside.log'], probeMediaFile }),
    /产物目录|相对路径/,
  );
  await assert.rejects(
    validateDelivery({ ...fixture, logFiles: [], probeMediaFile }),
    /日志/,
  );
  await assert.rejects(
    validateDelivery({ ...fixture, markdownFiles: [], probeMediaFile }),
    /Markdown/,
  );
  await assert.rejects(
    validateDelivery({ ...fixture, sourceTextFiles: [], probeMediaFile }),
    /源码|HTML/,
  );
  await assert.rejects(
    validateDelivery({ ...fixture, privacyObjects: [], probeMediaFile }),
    /夹具|隐私扫描对象/,
  );
});

test('纯函数生成稳定 manifest、SHA-256 与校验和文本', () => {
  assert.equal(
    sha256Hex('Work Review'),
    '9eb95b78076ede5533a0ee0824633e1180a59bfdbd146bef4fc278a6bd2abd78',
  );
  assert.equal(
    formatSha256Sums([
      { relativePath: 'z.txt', sha256: 'b'.repeat(64) },
      { relativePath: 'a.txt', sha256: 'a'.repeat(64) },
    ]),
    `${'a'.repeat(64)}  a.txt\n${'b'.repeat(64)}  z.txt\n`,
  );

  const manifest = buildDeliveryManifest({
    version: '1.1.1-demo',
    builtAt: '2026-08-12T12:00:00.000Z',
    gitCommit: 'abc1234',
    media: {
      '16x9': validateMediaArtifact({
        aspect: '16x9',
        relativePath: 'work-review-demo-16x9.mp4',
        sizeBytes: 10,
        probe: validProbe(),
      }),
      '9x16': validateMediaArtifact({
        aspect: '9x16',
        relativePath: 'work-review-demo-9x16.mp4',
        sizeBytes: 20,
        probe: validProbe(1080, 1920),
      }),
    },
    files: [{ relativePath: 'work-review-demo-zh.srt', sizeBytes: 99, sha256: 'a'.repeat(64) }],
    manualChecks: ['检查关键帧'],
  });

  assert.equal(manifest.schemaVersion, 1);
  assert.equal(manifest.media['16x9'].resolution, '1920x1080');
  assert.equal(manifest.media['9x16'].frameCount, 2550);
  assert.equal(manifest.files[0]?.sizeBytes, 99);
});

test('写入 manifest.json 与 SHA256SUMS，并可从文件入口复验完整性', async () => {
  const fixture = await createDeliveryFixture();
  const options = {
    ...fixture,
    probeMediaFile: async (filePath: string) => filePath.includes('9x16')
      ? validProbe(1080, 1920)
      : validProbe(),
  };
  const validation = await validateDelivery(options);
  const written = await writeManifest({
    artifactDir: fixture.artifactDir,
    version: '1.1.1-demo',
    builtAt: '2026-08-12T12:00:00.000Z',
    gitCommit: 'abc1234',
    validation,
  });

  assert.equal((await stat(written.manifestPath)).size > 0, true);
  assert.equal((await stat(written.sha256SumsPath)).size > 0, true);
  const manifest = JSON.parse(await readFile(written.manifestPath, 'utf8')) as {
    version: string;
    media: { '16x9': { durationSeconds: number; videoCodec: string } };
  };
  assert.equal(manifest.version, '1.1.1-demo');
  assert.equal(manifest.media['16x9'].durationSeconds, 85);
  assert.equal(manifest.media['16x9'].videoCodec, 'h264');

  const verified = await validateDelivery({ ...options, requireIntegrityFiles: true });
  assert.equal(verified.integrityVerified, true);

  await writeFile(path.join(fixture.artifactDir, 'work-review-demo-zh.srt'), SAFE_SRT.replace('今日完成导出流程', '今日完成安全导出流程').replace('当天轨迹', '当日轨迹'));
  await assert.rejects(
    validateDelivery({ ...options, requireIntegrityFiles: true }),
    /SHA-256|校验和/,
  );
});

test('原子写使用不可预测临时路径，不能跟随旧式可预测路径上的符号链接', async () => {
  const fixture = await createDeliveryFixture();
  const validation = await validateDelivery({
    ...fixture,
    probeMediaFile: async (filePath) => filePath.includes('9x16')
      ? validProbe(1080, 1920)
      : validProbe(),
  });
  const victimPath = path.join(fixture.artifactDir, 'victim.txt');
  const predictableTemporaryPath = path.join(
    fixture.artifactDir,
    `manifest.json.tmp-${process.pid}`,
  );
  await writeFile(victimPath, '不得修改');
  await symlink(victimPath, predictableTemporaryPath);

  const written = await writeManifest({
    artifactDir: fixture.artifactDir,
    version: '1.1.1-demo',
    builtAt: '2026-08-12T12:00:00.000Z',
    gitCommit: 'abc1234',
    validation,
  });

  assert.equal(await readFile(victimPath, 'utf8'), '不得修改');
  assert.equal((await lstat(written.manifestPath)).isSymbolicLink(), false);
});

test('原子写通过 wx 独占创建拒绝已存在的随机临时路径', async () => {
  const fixture = await createDeliveryFixture();
  const validation = await validateDelivery({
    ...fixture,
    probeMediaFile: async (filePath) => filePath.includes('9x16')
      ? validProbe(1080, 1920)
      : validProbe(),
  });
  const collisionToken = '00000000-0000-4000-8000-000000000000';
  const victimPath = path.join(fixture.artifactDir, 'collision-victim.txt');
  const occupiedTemporaryPath = path.join(
    fixture.artifactDir,
    `manifest.json.tmp-${collisionToken}`,
  );
  await writeFile(victimPath, '不得覆盖');
  await symlink(victimPath, occupiedTemporaryPath);

  const originalRandomUuid = crypto.randomUUID;
  crypto.randomUUID = () => collisionToken;
  syncBuiltinESMExports();
  try {
    await assert.rejects(
      writeManifest({
        artifactDir: fixture.artifactDir,
        version: '1.1.1-demo',
        builtAt: '2026-08-12T12:00:00.000Z',
        gitCommit: 'abc1234',
        validation,
      }),
      (error: NodeJS.ErrnoException) => error.code === 'EEXIST',
    );
  } finally {
    crypto.randomUUID = originalRandomUuid;
    syncBuiltinESMExports();
  }

  assert.equal(await readFile(victimPath, 'utf8'), '不得覆盖');
  assert.equal((await lstat(occupiedTemporaryPath)).isSymbolicLink(), true);
});

test('原子写重命名失败后清理本次创建的临时文件', async () => {
  const fixture = await createDeliveryFixture();
  const validation = await validateDelivery({
    ...fixture,
    probeMediaFile: async (filePath) => filePath.includes('9x16')
      ? validProbe(1080, 1920)
      : validProbe(),
  });
  await mkdir(path.join(fixture.artifactDir, 'SHA256SUMS'));

  await assert.rejects(writeManifest({
    artifactDir: fixture.artifactDir,
    version: '1.1.1-demo',
    builtAt: '2026-08-12T12:00:00.000Z',
    gitCommit: 'abc1234',
    validation,
  }));

  const leakedTemporaryFiles = (await readdir(fixture.artifactDir))
    .filter((name) => name.startsWith('SHA256SUMS.tmp-'));
  assert.deepEqual(leakedTemporaryFiles, []);
});
