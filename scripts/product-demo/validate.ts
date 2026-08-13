import { createHash, randomUUID } from 'node:crypto';
import {
  lstat,
  open,
  readFile,
  realpath,
  rename,
  stat,
  unlink,
  writeFile,
} from 'node:fs/promises';
import path from 'node:path';

import {
  probeMedia,
  type MediaProbe,
} from './ffmpeg.ts';
import {
  assertNoSensitiveContent,
  scanDemoObject,
  scanSensitiveText,
  type PrivacyFinding,
} from './privacy.ts';
import {
  DEMO_DURATION_SECONDS,
  DEMO_FPS,
  DEMO_TOTAL_FRAMES,
  STORYBOARD,
} from './storyboard.ts';
import type { DemoAspect, StoryboardSceneId } from './types.ts';

const FRAME_TOLERANCE_SECONDS = 1 / DEMO_FPS;
const RATE_TOLERANCE = 1e-6;
const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const KEYFRAME_PHASES = ['start', 'action', 'end'] as const;

const MEDIA_FILES: Readonly<Record<DemoAspect, string>> = {
  '16x9': 'work-review-demo-16x9.mp4',
  '9x16': 'work-review-demo-9x16.mp4',
};

const SUBTITLE_FILES = [
  'work-review-demo-zh.srt',
  'work-review-demo-en.srt',
  'work-review-demo-zh-en.srt',
] as const;

const COVER_FILES: Readonly<Record<DemoAspect, string>> = {
  '16x9': 'work-review-demo-cover-16x9.png',
  '9x16': 'work-review-demo-cover-9x16.png',
};

const TARGET_DIMENSIONS: Readonly<Record<DemoAspect, { width: number; height: number }>> = {
  '16x9': { width: 1920, height: 1080 },
  '9x16': { width: 1080, height: 1920 },
};

export interface MediaArtifactInput {
  aspect: DemoAspect;
  relativePath: string;
  sizeBytes: number;
  probe: MediaProbe;
}

export interface ValidatedMediaArtifact {
  aspect: DemoAspect;
  relativePath: string;
  sizeBytes: number;
  width: number;
  height: number;
  resolution: string;
  durationSeconds: number;
  frameRate: number;
  frameCount: number;
  videoCodec: 'h264';
  audioCodec: 'aac';
  pixelFormat: 'yuv420p';
}

export interface ParsedSubtitleCue {
  index: number;
  startMilliseconds: number;
  endMilliseconds: number;
  text: string;
}

export interface ValidatedSubtitleArtifact {
  relativePath: string;
  cueCount: number;
  timelineMilliseconds: ReadonlyArray<readonly [number, number]>;
}

export interface DeliveryFileRecord {
  relativePath: string;
  sizeBytes: number;
  sha256: string;
}

export interface PrivacyObjectInput {
  source: string;
  value: unknown;
}

export type KeyframeOcrPhase = typeof KEYFRAME_PHASES[number];

/** 自动 OCR 引擎需要处理的真实关键帧；绝对路径仅在运行期传递，不写入交付清单。 */
export interface KeyframeOcrFrame {
  aspect: DemoAspect;
  scene: StoryboardSceneId;
  phase: KeyframeOcrPhase;
  relativePath: string;
  absolutePath: string;
  imageBytes: Uint8Array;
}

export interface KeyframeOcrEvidence {
  relativePath: string;
  text: string;
}

export type KeyframeOcrRunResult =
  | {
      available: true;
      engine: string;
      evidence: readonly KeyframeOcrEvidence[];
    }
  | {
      available: false;
      reason: string;
    };

export type KeyframeOcrRunner = (
  frames: readonly KeyframeOcrFrame[],
) => Promise<KeyframeOcrRunResult>;

export interface ValidateDeliveryOptions {
  artifactDir: string;
  logFiles: readonly string[];
  markdownFiles: readonly string[];
  sourceTextFiles?: readonly string[];
  ocrTextFiles?: readonly string[];
  privacyObjects?: readonly PrivacyObjectInput[];
  runKeyframeOcr?: KeyframeOcrRunner;
  probeMediaFile?: (filePath: string) => Promise<MediaProbe>;
  requireIntegrityFiles?: boolean;
}

export interface DeliveryValidationResult {
  artifactDir: string;
  files: DeliveryFileRecord[];
  media: Record<DemoAspect, ValidatedMediaArtifact>;
  subtitles: Record<string, ValidatedSubtitleArtifact>;
  covers: Record<DemoAspect, { width: number; height: number }>;
  subtitleTimelineMilliseconds: ReadonlyArray<readonly [number, number]>;
  manualChecks: string[];
  keyframeOcr: {
    engine: string;
    evidenceCount: number;
  };
  integrityVerified: boolean;
}

export interface DeliveryManifestInput {
  version: string;
  builtAt: string;
  gitCommit: string;
  media: Record<DemoAspect, ValidatedMediaArtifact>;
  files: readonly DeliveryFileRecord[];
  manualChecks: readonly string[];
}

export interface DeliveryManifest {
  schemaVersion: 1;
  version: string;
  builtAt: string;
  gitCommit: string;
  media: Record<DemoAspect, {
    path: string;
    resolution: string;
    width: number;
    height: number;
    durationSeconds: number;
    frameRate: number;
    frameCount: number;
    videoCodec: string;
    audioCodec: string;
    pixelFormat: string;
    sizeBytes: number;
  }>;
  files: DeliveryFileRecord[];
  manualChecks: string[];
}

export interface WriteManifestOptions {
  artifactDir: string;
  version: string;
  builtAt: string;
  gitCommit: string;
  validation: DeliveryValidationResult;
}

export interface WrittenManifestPaths {
  manifestPath: string;
  sha256SumsPath: string;
}

interface LoadedFile {
  relativePath: string;
  absolutePath: string;
  bytes: Buffer;
  sizeBytes: number;
}

function assertNonEmptyString(value: string, label: string): void {
  if (!value.trim()) throw new Error(`${label}不得为空`);
}

function approximatelyEqual(left: number, right: number, tolerance: number): boolean {
  return Number.isFinite(left)
    && Number.isFinite(right)
    && Math.abs(left - right) <= tolerance;
}

function requiredFiniteNumber(value: number | null, label: string): number {
  if (value === null || !Number.isFinite(value)) throw new Error(`${label}缺失或无效`);
  return value;
}

function requiredPositiveInteger(value: number | null, label: string): number {
  if (value === null || !Number.isInteger(value) || value <= 0) {
    throw new Error(`${label}缺失或无效`);
  }
  return value;
}

function assertExpectedCodec(
  actual: string | null,
  expected: 'h264' | 'aac' | 'yuv420p',
  label: string,
): void {
  if (actual?.toLowerCase() !== expected) {
    const displayExpected = expected === 'h264' ? 'H.264' : expected === 'aac' ? 'AAC' : expected;
    throw new Error(`${label}必须为 ${displayExpected}，实际为 ${actual ?? '缺失'}`);
  }
}

/** 验证单个成片的发布媒体规格，并返回可写入清单的规范化数据。 */
export function validateMediaArtifact(input: MediaArtifactInput): ValidatedMediaArtifact {
  if (!Number.isInteger(input.sizeBytes) || input.sizeBytes <= 0) {
    throw new Error(`媒体文件 ${input.relativePath} 必须存在且非空`);
  }

  const expected = TARGET_DIMENSIONS[input.aspect];
  const { probe } = input;
  const formatNames = new Set(
    (probe.formatName ?? '').toLowerCase().split(',').map((value) => value.trim()).filter(Boolean),
  );
  if (!formatNames.has('mp4')) {
    throw new Error(`媒体文件 ${input.relativePath} 必须使用 MP4 容器`);
  }
  if (!probe.video) throw new Error(`媒体文件 ${input.relativePath} 缺少视频轨`);
  if (!probe.audio) throw new Error(`媒体文件 ${input.relativePath} 缺少 AAC 音轨`);

  const durationSeconds = requiredFiniteNumber(probe.durationSeconds, '容器时长');
  if (!approximatelyEqual(durationSeconds, DEMO_DURATION_SECONDS, FRAME_TOLERANCE_SECONDS)) {
    throw new Error(
      `媒体文件 ${input.relativePath} 时长必须为 ${DEMO_DURATION_SECONDS} 秒（允许一帧误差），实际为 ${durationSeconds} 秒`,
    );
  }

  const videoDurationSeconds = requiredFiniteNumber(probe.video.durationSeconds, '视频流时长');
  if (!approximatelyEqual(videoDurationSeconds, DEMO_DURATION_SECONDS, FRAME_TOLERANCE_SECONDS)) {
    throw new Error(
      `视频流时长必须为 ${DEMO_DURATION_SECONDS} 秒（允许一帧误差），实际为 ${videoDurationSeconds} 秒`,
    );
  }

  const width = requiredPositiveInteger(probe.video.width, '视频宽度');
  const height = requiredPositiveInteger(probe.video.height, '视频高度');
  if (width !== expected.width || height !== expected.height) {
    throw new Error(
      `${input.aspect} 媒体分辨率必须为 ${expected.width}×${expected.height}，实际为 ${width}×${height}`,
    );
  }

  const nominalFrameRate = requiredFiniteNumber(probe.video.frameRate, '视频标称帧率');
  const averageFrameRate = requiredFiniteNumber(probe.video.averageFrameRate, '视频平均帧率');
  if (!approximatelyEqual(nominalFrameRate, DEMO_FPS, RATE_TOLERANCE)) {
    throw new Error(`视频标称帧率必须为恒定 ${DEMO_FPS}fps，实际为 ${nominalFrameRate}fps`);
  }
  if (!approximatelyEqual(averageFrameRate, DEMO_FPS, RATE_TOLERANCE)
    || !approximatelyEqual(nominalFrameRate, averageFrameRate, RATE_TOLERANCE)) {
    throw new Error(`视频平均帧率必须证明恒定 ${DEMO_FPS}fps，实际为 ${averageFrameRate}fps`);
  }

  if (probe.video.frameCount === null) {
    throw new Error('视频缺少准确帧数；必须使用 ffprobe -count_frames 获取 nb_read_frames');
  }
  const frameCount = requiredPositiveInteger(probe.video.frameCount, '视频帧数');
  if (frameCount !== DEMO_TOTAL_FRAMES) {
    throw new Error(`视频必须包含 ${DEMO_TOTAL_FRAMES} 帧，实际为 ${frameCount} 帧`);
  }

  assertExpectedCodec(probe.video.codec, 'h264', '视频编码');
  assertExpectedCodec(probe.audio.codec, 'aac', '音频编码');
  assertExpectedCodec(probe.video.pixelFormat, 'yuv420p', '像素格式');

  const audioDurationSeconds = requiredFiniteNumber(probe.audio.durationSeconds, '音频时长');
  if (!approximatelyEqual(audioDurationSeconds, DEMO_DURATION_SECONDS, FRAME_TOLERANCE_SECONDS)) {
    throw new Error(
      `音频时长必须为 ${DEMO_DURATION_SECONDS} 秒（允许一帧误差），实际为 ${audioDurationSeconds} 秒`,
    );
  }
  const allowedEnd = videoDurationSeconds + FRAME_TOLERANCE_SECONDS;
  if (audioDurationSeconds > allowedEnd) {
    throw new Error(
      `音频结尾不得超出视频画面，音频为 ${audioDurationSeconds} 秒，画面为 ${durationSeconds} 秒`,
    );
  }

  return {
    aspect: input.aspect,
    relativePath: input.relativePath,
    sizeBytes: input.sizeBytes,
    width,
    height,
    resolution: `${width}x${height}`,
    durationSeconds,
    frameRate: averageFrameRate,
    frameCount,
    videoCodec: 'h264',
    audioCodec: 'aac',
    pixelFormat: 'yuv420p',
  };
}

function parseSrtTimestamp(value: string, source: string): number {
  const match = /^(\d{2,}):(\d{2}):(\d{2}),(\d{3})$/u.exec(value);
  if (!match) throw new Error(`SRT ${source} 包含无效时间码：${value}`);

  const [, hoursText, minutesText, secondsText, millisecondsText] = match;
  const hours = Number(hoursText);
  const minutes = Number(minutesText);
  const seconds = Number(secondsText);
  const milliseconds = Number(millisecondsText);
  if (minutes > 59 || seconds > 59) {
    throw new Error(`SRT ${source} 包含越界时间码：${value}`);
  }
  return (((hours * 60) + minutes) * 60 + seconds) * 1000 + milliseconds;
}

/** 严格解析 SRT；返回时间线供三语文件做逐条一致性比对。 */
export function parseSrtArtifact(content: string, source: string): ParsedSubtitleCue[] {
  assertNonEmptyString(source, 'SRT 来源');
  if (!content.trim()) throw new Error(`SRT ${source} 必须存在且非空`);

  const normalized = content.replaceAll('\r\n', '\n').replaceAll('\r', '\n').trim();
  const blocks = normalized.split(/\n{2,}/u);
  const cues: ParsedSubtitleCue[] = [];

  for (const [position, block] of blocks.entries()) {
    const lines = block.split('\n');
    if (lines.length < 3) throw new Error(`SRT ${source} 第 ${position + 1} 段格式无效`);

    const index = Number(lines[0]);
    if (!Number.isInteger(index) || index !== position + 1) {
      throw new Error(`SRT ${source} 序号必须从 1 连续递增`);
    }

    const timing = /^(\S+)\s+-->\s+(\S+)$/u.exec(lines[1] ?? '');
    if (!timing) throw new Error(`SRT ${source} 第 ${index} 条时间码格式无效`);
    const startMilliseconds = parseSrtTimestamp(timing[1], source);
    const endMilliseconds = parseSrtTimestamp(timing[2], source);
    const text = lines.slice(2).join('\n').trim();
    if (!text) throw new Error(`SRT ${source} 第 ${index} 条字幕文本为空`);
    if (endMilliseconds <= startMilliseconds) {
      throw new Error(`SRT ${source} 第 ${index} 条时间轴倒序或时长无效`);
    }
    if (endMilliseconds > DEMO_DURATION_SECONDS * 1000) {
      throw new Error(`SRT ${source} 第 ${index} 条超过 ${DEMO_DURATION_SECONDS} 秒视频边界`);
    }

    const previous = cues.at(-1);
    if (previous && startMilliseconds < previous.endMilliseconds) {
      throw new Error(`SRT ${source} 第 ${index} 条与上一条重叠或倒序`);
    }

    cues.push({ index, startMilliseconds, endMilliseconds, text });
  }

  return cues;
}

export function validateSubtitleArtifact(
  content: string,
  relativePath: string,
): ValidatedSubtitleArtifact {
  const cues = parseSrtArtifact(content, relativePath);
  return {
    relativePath,
    cueCount: cues.length,
    timelineMilliseconds: cues.map(
      (cue) => [cue.startMilliseconds, cue.endMilliseconds] as const,
    ),
  };
}

/** 从 PNG IHDR 读取画布尺寸，不依赖图像解码器。 */
export function parsePngDimensions(bytes: Uint8Array): { width: number; height: number } {
  const buffer = Buffer.from(bytes);
  if (buffer.length < 24
    || !buffer.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)
    || buffer.toString('ascii', 12, 16) !== 'IHDR') {
    throw new Error('封面文件不是有效的 PNG，或缺少 IHDR');
  }
  const width = buffer.readUInt32BE(16);
  const height = buffer.readUInt32BE(20);
  if (width <= 0 || height <= 0) throw new Error('PNG 封面尺寸无效');
  return { width, height };
}

export function validateCoverArtifact(
  bytes: Uint8Array,
  aspect: DemoAspect,
  relativePath: string,
): { width: number; height: number } {
  if (bytes.byteLength === 0) throw new Error(`封面 ${relativePath} 必须存在且非空`);
  const dimensions = parsePngDimensions(bytes);
  const expected = TARGET_DIMENSIONS[aspect];
  if (dimensions.width !== expected.width || dimensions.height !== expected.height) {
    throw new Error(
      `${aspect} 封面必须为 ${expected.width}×${expected.height}，实际为 ${dimensions.width}×${dimensions.height}`,
    );
  }
  return dimensions;
}

export function sha256Hex(value: string | Uint8Array): string {
  return createHash('sha256').update(value).digest('hex');
}

function validateChecksumRecord(record: Pick<DeliveryFileRecord, 'relativePath' | 'sha256'>): void {
  assertNonEmptyString(record.relativePath, '校验和相对路径');
  if (record.relativePath.includes('\n') || record.relativePath.includes('\r')) {
    throw new Error('校验和相对路径不得包含换行');
  }
  if (!/^[0-9a-f]{64}$/u.test(record.sha256)) {
    throw new Error(`文件 ${record.relativePath} 的 SHA-256 无效`);
  }
}

export function formatSha256Sums(
  records: readonly Pick<DeliveryFileRecord, 'relativePath' | 'sha256'>[],
): string {
  const seen = new Set<string>();
  const sorted = [...records].sort((left, right) => left.relativePath.localeCompare(right.relativePath));
  for (const record of sorted) {
    validateChecksumRecord(record);
    if (seen.has(record.relativePath)) throw new Error(`校验和包含重复路径：${record.relativePath}`);
    seen.add(record.relativePath);
  }
  return sorted.map((record) => `${record.sha256}  ${record.relativePath}\n`).join('');
}

function manifestMedia(media: ValidatedMediaArtifact): DeliveryManifest['media'][DemoAspect] {
  return {
    path: media.relativePath,
    resolution: media.resolution,
    width: media.width,
    height: media.height,
    durationSeconds: media.durationSeconds,
    frameRate: media.frameRate,
    frameCount: media.frameCount,
    videoCodec: media.videoCodec,
    audioCodec: media.audioCodec,
    pixelFormat: media.pixelFormat,
    sizeBytes: media.sizeBytes,
  };
}

/** 构建稳定、可序列化且不读取环境状态的交付清单。 */
export function buildDeliveryManifest(input: DeliveryManifestInput): DeliveryManifest {
  assertNonEmptyString(input.version, '版本');
  assertNonEmptyString(input.gitCommit, 'Git commit');
  if (!Number.isFinite(Date.parse(input.builtAt))) throw new Error('构建时间必须是有效 ISO 时间');

  const files = [...input.files]
    .sort((left, right) => left.relativePath.localeCompare(right.relativePath))
    .map((file) => ({ ...file }));
  for (const file of files) {
    validateChecksumRecord(file);
    if (!Number.isInteger(file.sizeBytes) || file.sizeBytes <= 0) {
      throw new Error(`清单文件 ${file.relativePath} 必须非空`);
    }
  }

  return {
    schemaVersion: 1,
    version: input.version,
    builtAt: input.builtAt,
    gitCommit: input.gitCommit,
    media: {
      '16x9': manifestMedia(input.media['16x9']),
      '9x16': manifestMedia(input.media['9x16']),
    },
    files,
    manualChecks: [...input.manualChecks],
  };
}

function isPathInside(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate);
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}

function resolveArtifactPath(artifactDir: string, relativePath: string): string {
  assertNonEmptyString(relativePath, '交付文件相对路径');
  if (path.isAbsolute(relativePath)) throw new Error(`交付文件必须使用相对路径：${relativePath}`);
  const absoluteRoot = path.resolve(artifactDir);
  const absolutePath = path.resolve(absoluteRoot, relativePath);
  if (!isPathInside(absoluteRoot, absolutePath) || absolutePath === absoluteRoot) {
    throw new Error(`交付文件路径不得逃逸产物目录：${relativePath}`);
  }
  return absolutePath;
}

async function loadRequiredFile(artifactDir: string, relativePath: string): Promise<LoadedFile> {
  const absolutePath = resolveArtifactPath(artifactDir, relativePath);
  let fileInfo;
  try {
    fileInfo = await lstat(absolutePath);
  } catch (error) {
    throw new Error(`缺少交付文件 ${relativePath}：${error instanceof Error ? error.message : String(error)}`);
  }
  if (fileInfo.isSymbolicLink()) throw new Error(`交付文件不得是符号链接：${relativePath}`);
  if (!fileInfo.isFile()) throw new Error(`交付路径不是普通文件：${relativePath}`);
  if (fileInfo.size <= 0) throw new Error(`交付文件 ${relativePath} 必须存在且非空`);

  const [rootRealPath, fileRealPath] = await Promise.all([realpath(artifactDir), realpath(absolutePath)]);
  if (!isPathInside(rootRealPath, fileRealPath)) {
    throw new Error(`交付文件真实路径逃逸产物目录：${relativePath}`);
  }

  const bytes = await readFile(absolutePath);
  if (bytes.length <= 0) throw new Error(`交付文件 ${relativePath} 读取后为空`);
  return {
    relativePath,
    absolutePath,
    bytes,
    sizeBytes: bytes.length,
  };
}

function fileRecord(file: LoadedFile): DeliveryFileRecord {
  return {
    relativePath: file.relativePath,
    sizeBytes: file.sizeBytes,
    sha256: sha256Hex(file.bytes),
  };
}

function uniquePaths(paths: readonly string[], label: string): string[] {
  if (paths.length === 0) throw new Error(`交付验证必须提供至少一个${label}`);
  const unique = [...new Set(paths)];
  if (unique.length !== paths.length) throw new Error(`${label}路径不得重复`);
  return unique;
}

function optionalUniquePaths(paths: readonly string[] | undefined, label: string): string[] {
  const values = paths ?? [];
  const unique = [...new Set(values)];
  if (unique.length !== values.length) throw new Error(`${label}路径不得重复`);
  return unique;
}

function expectedKeyframeDescriptors(): Array<{
  aspect: DemoAspect;
  scene: StoryboardSceneId;
  phase: KeyframeOcrPhase;
  relativePath: string;
}> {
  return (['16x9', '9x16'] as const).flatMap((aspect) => (
    STORYBOARD.flatMap((scene) => (
      KEYFRAME_PHASES.map((phase) => ({
        aspect,
        scene: scene.id,
        phase,
        relativePath: `intermediate/${aspect}/${scene.id}-${phase}.png`,
      }))
    ))
  ));
}

function assertKeyframeOcrEvidence(
  result: KeyframeOcrRunResult,
  frames: readonly KeyframeOcrFrame[],
): { engine: string; evidence: KeyframeOcrEvidence[] } {
  if (!result || typeof result !== 'object') {
    throw new Error('自动 OCR 返回了无效结果，无法验证关键帧隐私');
  }
  if (!result.available) {
    const reason = typeof result.reason === 'string' && result.reason.trim()
      ? result.reason.trim()
      : 'OCR 引擎未提供原因';
    throw new Error(`自动 OCR 不可用，交付已阻断：${reason}`);
  }
  if (typeof result.engine !== 'string' || !result.engine.trim()) {
    throw new Error('自动 OCR 未报告有效引擎名称，无法把结果作为交付证据');
  }
  if (!Array.isArray(result.evidence)) {
    throw new Error('自动 OCR 未返回关键帧 OCR 证据数组');
  }

  const expectedPaths = new Set(frames.map((frame) => frame.relativePath));
  const evidenceByPath = new Map<string, KeyframeOcrEvidence>();
  for (const item of result.evidence) {
    if (!item || typeof item !== 'object'
      || typeof item.relativePath !== 'string'
      || typeof item.text !== 'string') {
      throw new Error('自动 OCR 返回了无效证据；每项必须包含关键帧相对路径与真实 OCR 文本');
    }
    if (!expectedPaths.has(item.relativePath)) {
      throw new Error(`自动 OCR 返回了未知关键帧证据：${item.relativePath}`);
    }
    if (evidenceByPath.has(item.relativePath)) {
      throw new Error(`关键帧 OCR 证据路径重复：${item.relativePath}`);
    }
    evidenceByPath.set(item.relativePath, item);
  }

  for (const frame of frames) {
    if (!evidenceByPath.has(frame.relativePath)) {
      throw new Error(`关键帧 OCR 证据缺失：${frame.relativePath}`);
    }
  }
  return {
    engine: result.engine.trim(),
    evidence: frames.map((frame) => evidenceByPath.get(frame.relativePath)!),
  };
}

function sameTimeline(
  left: ReadonlyArray<readonly [number, number]>,
  right: ReadonlyArray<readonly [number, number]>,
): boolean {
  return left.length === right.length && left.every(
    (range, index) => range[0] === right[index]?.[0] && range[1] === right[index]?.[1],
  );
}

function parseSha256Sums(content: string): Map<string, string> {
  const entries = new Map<string, string>();
  const lines = content.replaceAll('\r\n', '\n').trim().split('\n');
  for (const line of lines) {
    const match = /^([0-9a-f]{64})  (.+)$/u.exec(line);
    if (!match) throw new Error(`SHA256SUMS 包含无效行：${line}`);
    const [, hash, relativePath] = match;
    validateChecksumRecord({ relativePath, sha256: hash });
    if (entries.has(relativePath)) throw new Error(`SHA256SUMS 包含重复路径：${relativePath}`);
    entries.set(relativePath, hash);
  }
  return entries;
}

function parseManifest(content: string): DeliveryManifest {
  let value: unknown;
  try {
    value = JSON.parse(content);
  } catch (error) {
    throw new Error(`manifest.json 不是有效 JSON：${error instanceof Error ? error.message : String(error)}`);
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('manifest.json 顶层必须是对象');
  }
  const candidate = value as Partial<DeliveryManifest>;
  if (candidate.schemaVersion !== 1 || !Array.isArray(candidate.files) || !candidate.media) {
    throw new Error('manifest.json 缺少受支持的清单结构');
  }
  return candidate as DeliveryManifest;
}

function assertManifestMatches(
  manifest: DeliveryManifest,
  files: readonly DeliveryFileRecord[],
  media: Record<DemoAspect, ValidatedMediaArtifact>,
): void {
  const expectedFiles = new Map(files.map((file) => [file.relativePath, file]));
  if (manifest.files.length !== expectedFiles.size) {
    throw new Error('manifest.json 文件清单数量与当前交付不一致');
  }
  for (const manifestFile of manifest.files) {
    const current = expectedFiles.get(manifestFile.relativePath);
    if (!current
      || current.sizeBytes !== manifestFile.sizeBytes
      || current.sha256 !== manifestFile.sha256) {
      throw new Error(`manifest.json 的 SHA-256 或大小与当前文件不一致：${manifestFile.relativePath}`);
    }
  }

  for (const aspect of ['16x9', '9x16'] as const) {
    const listed = manifest.media[aspect];
    const current = media[aspect];
    if (!listed
      || listed.path !== current.relativePath
      || listed.resolution !== current.resolution
      || listed.durationSeconds !== current.durationSeconds
      || listed.frameRate !== current.frameRate
      || listed.frameCount !== current.frameCount
      || listed.videoCodec !== current.videoCodec
      || listed.audioCodec !== current.audioCodec
      || listed.pixelFormat !== current.pixelFormat
      || listed.sizeBytes !== current.sizeBytes) {
      throw new Error(`manifest.json 的 ${aspect} 媒体规格与当前文件不一致`);
    }
  }
}

async function verifyIntegrityFiles(
  artifactDir: string,
  payloadFiles: readonly DeliveryFileRecord[],
  media: Record<DemoAspect, ValidatedMediaArtifact>,
  findings: PrivacyFinding[],
): Promise<DeliveryFileRecord[]> {
  const manifestFile = await loadRequiredFile(artifactDir, 'manifest.json');
  const sumsFile = await loadRequiredFile(artifactDir, 'SHA256SUMS');
  const manifestText = manifestFile.bytes.toString('utf8');
  findings.push(...scanSensitiveText(manifestText, 'manifest.json'));
  findings.push(...scanSensitiveText('manifest.json\nSHA256SUMS', 'integrity-filenames'));

  const manifest = parseManifest(manifestText);
  assertManifestMatches(manifest, payloadFiles, media);

  const sums = parseSha256Sums(sumsFile.bytes.toString('utf8'));
  const actualRecords = [...payloadFiles, fileRecord(manifestFile)];
  if (sums.size !== actualRecords.length) {
    throw new Error('SHA256SUMS 校验和条目数量与交付文件不一致');
  }
  for (const record of actualRecords) {
    if (sums.get(record.relativePath) !== record.sha256) {
      throw new Error(`SHA-256 校验和失败：${record.relativePath}`);
    }
  }
  return [fileRecord(manifestFile), fileRecord(sumsFile)];
}

/**
 * 验证完整产品演示交付目录。
 *
 * 关键帧必须由调用方注入真实自动 OCR 引擎；未接线、不可用或证据不完整都会阻断交付。
 * `ocrTextFiles` 保留用于扫描额外的既有 OCR 文本，不能替代 42 张关键帧的自动 OCR。
 */
export async function validateDelivery(
  options: ValidateDeliveryOptions,
): Promise<DeliveryValidationResult> {
  assertNonEmptyString(options.artifactDir, '产物目录');
  const artifactInfo = await stat(options.artifactDir).catch((error: unknown) => {
    throw new Error(`产物目录不存在：${error instanceof Error ? error.message : String(error)}`);
  });
  if (!artifactInfo.isDirectory()) throw new Error('产物目录必须是目录');

  const logFiles = uniquePaths(options.logFiles, 'Playwright 控制台或网络日志');
  const markdownFiles = uniquePaths(options.markdownFiles, '虚拟导出 Markdown');
  const ocrTextFiles = optionalUniquePaths(options.ocrTextFiles, '既有 OCR 文本');
  const sourceTextFiles = uniquePaths(options.sourceTextFiles ?? [], '源码或 HTML 文本');
  const privacyObjects = options.privacyObjects ?? [];
  if (privacyObjects.length === 0) {
    throw new Error('交付验证必须提供至少一个演示夹具或隐私扫描对象');
  }
  const probeMediaFile = options.probeMediaFile ?? ((filePath: string) => probeMedia(filePath));

  const loadedByPath = new Map<string, LoadedFile>();
  const load = async (relativePath: string): Promise<LoadedFile> => {
    const existing = loadedByPath.get(relativePath);
    if (existing) return existing;
    const loaded = await loadRequiredFile(options.artifactDir, relativePath);
    loadedByPath.set(relativePath, loaded);
    return loaded;
  };

  const media = {} as Record<DemoAspect, ValidatedMediaArtifact>;
  for (const aspect of ['16x9', '9x16'] as const) {
    const relativePath = MEDIA_FILES[aspect];
    const file = await load(relativePath);
    media[aspect] = validateMediaArtifact({
      aspect,
      relativePath,
      sizeBytes: file.sizeBytes,
      probe: await probeMediaFile(file.absolutePath),
    });
  }

  const subtitles: Record<string, ValidatedSubtitleArtifact> = {};
  let sharedTimeline: ReadonlyArray<readonly [number, number]> | null = null;
  for (const relativePath of SUBTITLE_FILES) {
    const file = await load(relativePath);
    const validation = validateSubtitleArtifact(file.bytes.toString('utf8'), relativePath);
    if (sharedTimeline && !sameTimeline(sharedTimeline, validation.timelineMilliseconds)) {
      throw new Error(`三个 SRT 的时间轴必须一致，检测到漂移：${relativePath}`);
    }
    sharedTimeline ??= validation.timelineMilliseconds;
    subtitles[relativePath] = validation;
  }

  const covers = {} as Record<DemoAspect, { width: number; height: number }>;
  for (const aspect of ['16x9', '9x16'] as const) {
    const relativePath = COVER_FILES[aspect];
    const file = await load(relativePath);
    covers[aspect] = validateCoverArtifact(file.bytes, aspect, relativePath);
  }

  if (!options.runKeyframeOcr) {
    throw new Error('交付验证未接线自动 OCR；必须对横竖屏七镜头的 start/action/end 关键帧执行真实 OCR');
  }
  const keyframeFrames: KeyframeOcrFrame[] = [];
  for (const descriptor of expectedKeyframeDescriptors()) {
    const file = await load(descriptor.relativePath);
    keyframeFrames.push({
      ...descriptor,
      absolutePath: file.absolutePath,
      imageBytes: file.bytes,
    });
  }
  let rawOcrResult: KeyframeOcrRunResult;
  try {
    rawOcrResult = await options.runKeyframeOcr(keyframeFrames);
  } catch (error) {
    throw new Error(
      `自动 OCR 执行失败，交付已阻断：${error instanceof Error ? error.message : String(error)}`,
    );
  }
  const keyframeOcr = assertKeyframeOcrEvidence(rawOcrResult, keyframeFrames);

  const textPaths = [...new Set([
    ...SUBTITLE_FILES,
    ...logFiles,
    ...markdownFiles,
    ...sourceTextFiles,
    ...ocrTextFiles,
  ])];
  const findings: PrivacyFinding[] = [];
  for (const relativePath of textPaths) {
    const file = await load(relativePath);
    findings.push(...scanSensitiveText(file.bytes.toString('utf8'), relativePath));
    findings.push(...scanSensitiveText(relativePath, 'delivery-filename'));
  }
  for (const relativePath of [...Object.values(MEDIA_FILES), ...Object.values(COVER_FILES)]) {
    findings.push(...scanSensitiveText(relativePath, 'delivery-filename'));
  }
  for (const evidence of keyframeOcr.evidence) {
    findings.push(...scanSensitiveText(evidence.text, `关键帧 OCR:${evidence.relativePath}`));
  }
  for (const object of privacyObjects) {
    findings.push(...scanDemoObject(object.value, object.source));
  }

  let files = [...loadedByPath.values()].map(fileRecord);
  let integrityVerified = false;
  if (options.requireIntegrityFiles) {
    const integrityFiles = await verifyIntegrityFiles(options.artifactDir, files, media, findings);
    files = [...files, ...integrityFiles];
    integrityVerified = true;
  }

  assertNoSensitiveContent(findings);
  files.sort((left, right) => left.relativePath.localeCompare(right.relativePath));

  return {
    artifactDir: path.resolve(options.artifactDir),
    files,
    media,
    subtitles,
    covers,
    subtitleTimelineMilliseconds: sharedTimeline ?? [],
    manualChecks: [],
    keyframeOcr: {
      engine: keyframeOcr.engine,
      evidenceCount: keyframeOcr.evidence.length,
    },
    integrityVerified,
  };
}

async function writeAtomically(filePath: string, content: string): Promise<void> {
  const temporaryPath = `${filePath}.tmp-${randomUUID()}`;
  let temporaryCreated = false;
  let temporaryFile: Awaited<ReturnType<typeof open>> | undefined;
  try {
    temporaryFile = await open(temporaryPath, 'wx');
    temporaryCreated = true;
    await temporaryFile.writeFile(content, 'utf8');
    await temporaryFile.close();
    temporaryFile = undefined;
    await rename(temporaryPath, filePath);
  } catch (error) {
    const cleanupErrors: unknown[] = [];
    if (temporaryFile) {
      try {
        await temporaryFile.close();
      } catch (cleanupError) {
        cleanupErrors.push(cleanupError);
      }
    }
    if (temporaryCreated) {
      try {
        await unlink(temporaryPath);
      } catch (cleanupError) {
        if ((cleanupError as NodeJS.ErrnoException).code !== 'ENOENT') {
          cleanupErrors.push(cleanupError);
        }
      }
    }
    if (cleanupErrors.length > 0) {
      throw new AggregateError(
        [error, ...cleanupErrors],
        `原子写入 ${filePath} 失败，且无法完整清理临时资源`,
      );
    }
    throw error;
  }
}

/** 写入 manifest.json 和覆盖全部载荷文件及清单本身的 SHA256SUMS。 */
export async function writeManifest(options: WriteManifestOptions): Promise<WrittenManifestPaths> {
  const manifest = buildDeliveryManifest({
    version: options.version,
    builtAt: options.builtAt,
    gitCommit: options.gitCommit,
    media: options.validation.media,
    files: options.validation.files.filter(
      (file) => file.relativePath !== 'manifest.json' && file.relativePath !== 'SHA256SUMS',
    ),
    manualChecks: options.validation.manualChecks,
  });
  const manifestContent = `${JSON.stringify(manifest, null, 2)}\n`;
  assertNoSensitiveContent(scanSensitiveText(manifestContent, 'manifest.json'));

  const manifestPath = resolveArtifactPath(options.artifactDir, 'manifest.json');
  const sha256SumsPath = resolveArtifactPath(options.artifactDir, 'SHA256SUMS');
  await writeAtomically(manifestPath, manifestContent);

  const manifestRecord: DeliveryFileRecord = {
    relativePath: 'manifest.json',
    sizeBytes: Buffer.byteLength(manifestContent),
    sha256: sha256Hex(manifestContent),
  };
  const payloadRecords = manifest.files.map((file) => ({
    relativePath: file.relativePath,
    sha256: file.sha256,
  }));
  await writeAtomically(
    sha256SumsPath,
    formatSha256Sums([...payloadRecords, manifestRecord]),
  );

  return { manifestPath, sha256SumsPath };
}
