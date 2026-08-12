import { spawn } from 'node:child_process';
import { constants as fsConstants } from 'node:fs';
import { access } from 'node:fs/promises';
import { delimiter, join } from 'node:path';
import {
  DEMO_DURATION_SECONDS,
  DEMO_FPS,
  DEMO_TOTAL_FRAMES,
} from './storyboard.ts';

const DEFAULT_CAPTURE_BYTES = 1024 * 1024;
const LOUDNESS_FILTER = 'loudnorm=I=-14:TP=-1:LRA=7';

export interface CommandResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export interface RunCommandOptions {
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  maxCaptureBytes?: number;
  signal?: AbortSignal;
}

export type CommandRunner = (
  binary: string,
  args: readonly string[],
  options?: RunCommandOptions,
) => Promise<CommandResult>;

export interface LoudnormMeasurement {
  inputI: number;
  inputTp: number;
  inputLra: number;
  inputThresh: number;
  targetOffset: number;
}

export interface VideoProbe {
  codec: string | null;
  pixelFormat: string | null;
  width: number | null;
  height: number | null;
  frameRate: number | null;
  averageFrameRate: number | null;
  frameCount: number | null;
  durationSeconds: number | null;
}

export interface AudioProbe {
  codec: string | null;
  sampleRate: number | null;
  channels: number | null;
  channelLayout: string | null;
  durationSeconds: number | null;
}

export interface MediaProbe {
  durationSeconds: number | null;
  formatName: string | null;
  video: VideoProbe | null;
  audio: AudioProbe | null;
}

interface ProbeStream {
  codec_type?: unknown;
  codec_name?: unknown;
  pix_fmt?: unknown;
  width?: unknown;
  height?: unknown;
  r_frame_rate?: unknown;
  avg_frame_rate?: unknown;
  nb_frames?: unknown;
  nb_read_frames?: unknown;
  duration?: unknown;
  sample_rate?: unknown;
  channels?: unknown;
  channel_layout?: unknown;
}

interface ProbeDocument {
  format?: {
    duration?: unknown;
    format_name?: unknown;
  };
  streams?: unknown;
}

interface CandidateOptions {
  env?: NodeJS.ProcessEnv | Record<string, string | undefined>;
  platform?: NodeJS.Platform;
}

export interface DiscoverMediaBinariesOptions extends CandidateOptions {
  runner?: CommandRunner;
  isExecutable?: (candidate: string) => Promise<boolean>;
}

export interface MediaBinaries {
  ffmpeg: string;
  ffprobe: string;
}

export interface ProbeMediaOptions {
  ffprobePath?: string;
  runner?: CommandRunner;
  discovery?: DiscoverMediaBinariesOptions;
}

export interface FinalVideoOptions {
  videoPath: string;
  audioPath: string;
  outputPath: string;
  width: number;
  height: number;
}

class BoundedTextCapture {
  private buffer = Buffer.alloc(0);
  private totalBytes = 0;

  constructor(
    private readonly limit: number,
    private readonly retain: 'head' | 'tail' = 'head',
  ) {}

  append(chunk: Buffer | string): void {
    const incoming = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    this.totalBytes += incoming.length;

    if (this.retain === 'head') {
      const remaining = this.limit - this.buffer.length;
      if (remaining > 0) {
        this.buffer = Buffer.concat([this.buffer, incoming.subarray(0, remaining)]);
      }
      return;
    }

    const combined = Buffer.concat([this.buffer, incoming]);
    this.buffer = combined.length > this.limit
      ? Buffer.from(combined.subarray(combined.length - this.limit))
      : combined;
  }

  text(): string {
    return this.buffer.toString('utf8');
  }

  get truncated(): boolean {
    return this.totalBytes > this.buffer.length;
  }
}

function assertNonEmpty(value: string, label: string): void {
  if (!value.trim()) throw new Error(`${label}不能为空`);
}

function formatCommandFailure(
  binary: string,
  exitCode: number | null,
  signal: NodeJS.Signals | null,
  stderr: BoundedTextCapture,
): Error {
  const status = exitCode === null ? `信号 ${signal ?? 'unknown'}` : `退出码 ${exitCode}`;
  const details = stderr.text().trim() || '无 stderr 输出';
  const suffix = stderr.truncated ? '\n[stderr 已截断]' : '';
  return new Error(`命令 ${binary} 执行失败（${status}）：${details}${suffix}`);
}

/**
 * 使用参数数组启动子进程。这里显式禁用 shell，避免路径或字幕文本被解释为命令。
 */
export function runCommand(
  binary: string,
  args: readonly string[],
  options: RunCommandOptions = {},
): Promise<CommandResult> {
  assertNonEmpty(binary, '可执行文件路径');
  if (!Array.isArray(args) || args.some((arg) => typeof arg !== 'string')) {
    return Promise.reject(new Error('命令参数必须是字符串数组'));
  }

  const maxCaptureBytes = options.maxCaptureBytes ?? DEFAULT_CAPTURE_BYTES;
  if (!Number.isInteger(maxCaptureBytes) || maxCaptureBytes <= 0) {
    return Promise.reject(new Error('maxCaptureBytes 必须是正整数'));
  }

  return new Promise((resolve, reject) => {
    const stdout = new BoundedTextCapture(maxCaptureBytes, 'head');
    const stderr = new BoundedTextCapture(maxCaptureBytes, 'tail');
    const child = spawn(binary, [...args], {
      cwd: options.cwd,
      env: options.env,
      signal: options.signal,
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });

    child.stdout.on('data', (chunk: Buffer) => stdout.append(chunk));
    child.stderr.on('data', (chunk: Buffer) => stderr.append(chunk));
    child.once('error', (error) => {
      reject(new Error(`无法启动命令 ${binary}：${error.message}`));
    });
    child.once('close', (exitCode, signal) => {
      if (exitCode !== 0) {
        reject(formatCommandFailure(binary, exitCode, signal, stderr));
        return;
      }
      resolve({
        stdout: stdout.text(),
        stderr: stderr.text(),
        exitCode: 0,
      });
    });
  });
}

function executableOverrideName(binary: 'ffmpeg' | 'ffprobe'): 'FFMPEG_PATH' | 'FFPROBE_PATH' {
  return binary === 'ffmpeg' ? 'FFMPEG_PATH' : 'FFPROBE_PATH';
}

function executableNames(binary: 'ffmpeg' | 'ffprobe', platform: NodeJS.Platform): string[] {
  if (platform === 'win32') return [`${binary}.exe`, binary];
  return [binary];
}

function unique(values: readonly string[]): string[] {
  return [...new Set(values.filter((value) => value.trim().length > 0))];
}

/** 构建可审计、确定性的 ffmpeg/ffprobe 搜索顺序。 */
export function buildExecutableCandidates(
  binary: 'ffmpeg' | 'ffprobe',
  options: CandidateOptions = {},
): string[] {
  const env = options.env ?? process.env;
  const platform = options.platform ?? process.platform;
  const override = env[executableOverrideName(binary)];
  const names = executableNames(binary, platform);
  const pathEntries = (env.PATH ?? '')
    .split(platform === 'win32' ? ';' : delimiter)
    .filter(Boolean);
  const candidates: string[] = [];

  if (override) candidates.push(override);
  for (const directory of pathEntries) {
    for (const name of names) candidates.push(join(directory, name));
  }

  const commonDirectories = platform === 'darwin'
    ? ['/opt/homebrew/bin', '/usr/local/bin', '/usr/bin']
    : platform === 'linux'
      ? ['/usr/local/bin', '/usr/bin', '/bin']
      : [];
  for (const directory of commonDirectories) {
    for (const name of names) candidates.push(join(directory, name));
  }

  return unique(candidates);
}

async function defaultIsExecutable(candidate: string): Promise<boolean> {
  try {
    await access(candidate, fsConstants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function discoverExecutable(
  binary: 'ffmpeg' | 'ffprobe',
  options: DiscoverMediaBinariesOptions,
): Promise<string> {
  const runner = options.runner ?? runCommand;
  const isExecutable = options.isExecutable ?? defaultIsExecutable;
  const candidates = buildExecutableCandidates(binary, options);

  for (const candidate of candidates) {
    if (!await isExecutable(candidate)) continue;
    try {
      await runner(candidate, ['-version'], { maxCaptureBytes: 64 * 1024 });
      return candidate;
    } catch {
      // 文件存在不代表可正常运行，继续检查下一个候选项。
    }
  }

  throw new Error(
    `未找到可用的 ${binary}。请安装 FFmpeg 工具链，或通过 ${executableOverrideName(binary)} 指定可执行文件。`,
  );
}

export async function discoverMediaBinaries(
  options: DiscoverMediaBinariesOptions = {},
): Promise<MediaBinaries> {
  const ffmpeg = await discoverExecutable('ffmpeg', options);
  const ffprobe = await discoverExecutable('ffprobe', options);
  return { ffmpeg, ffprobe };
}

export function buildProbeArgs(mediaPath: string): string[] {
  assertNonEmpty(mediaPath, '媒体路径');
  return [
    '-v', 'error',
    '-count_frames',
    '-show_streams',
    '-show_format',
    '-print_format', 'json',
    mediaPath,
  ];
}

function optionalString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function optionalNumber(value: unknown): number | null {
  if (typeof value === 'number') return Number.isFinite(value) ? value : null;
  if (typeof value !== 'string' || !value.trim() || value === 'N/A') return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function optionalInteger(value: unknown): number | null {
  const parsed = optionalNumber(value);
  return parsed !== null && Number.isInteger(parsed) ? parsed : null;
}

function parseRate(value: unknown): number | null {
  if (typeof value !== 'string' || !value.trim() || value === 'N/A') return null;
  const [numeratorText, denominatorText] = value.split('/');
  if (denominatorText === undefined) return optionalNumber(value);
  const numerator = Number(numeratorText);
  const denominator = Number(denominatorText);
  if (!Number.isFinite(numerator) || !Number.isFinite(denominator) || denominator === 0) return null;
  return numerator / denominator;
}

function streamDuration(stream: ProbeStream): number | null {
  return optionalNumber(stream.duration);
}

function parseVideoStream(stream: ProbeStream): VideoProbe {
  return {
    codec: optionalString(stream.codec_name),
    pixelFormat: optionalString(stream.pix_fmt),
    width: optionalInteger(stream.width),
    height: optionalInteger(stream.height),
    frameRate: parseRate(stream.r_frame_rate),
    averageFrameRate: parseRate(stream.avg_frame_rate),
    frameCount: optionalInteger(stream.nb_read_frames) ?? optionalInteger(stream.nb_frames),
    durationSeconds: streamDuration(stream),
  };
}

function parseAudioStream(stream: ProbeStream): AudioProbe {
  return {
    codec: optionalString(stream.codec_name),
    sampleRate: optionalInteger(stream.sample_rate),
    channels: optionalInteger(stream.channels),
    channelLayout: optionalString(stream.channel_layout),
    durationSeconds: streamDuration(stream),
  };
}

export function parseMediaProbe(stdout: string): MediaProbe {
  let document: ProbeDocument;
  try {
    document = JSON.parse(stdout) as ProbeDocument;
  } catch (error) {
    throw new Error(`无法解析 ffprobe JSON 输出：${error instanceof Error ? error.message : String(error)}`);
  }
  if (typeof document !== 'object' || document === null || Array.isArray(document)) {
    throw new Error('ffprobe JSON 顶层必须是对象');
  }

  if (!Array.isArray(document.streams)) {
    throw new Error('ffprobe JSON 缺少 streams 数组');
  }
  const streams = document.streams.filter(
    (stream): stream is ProbeStream => typeof stream === 'object' && stream !== null,
  );
  const videoStream = streams.find((stream) => stream.codec_type === 'video');
  const audioStream = streams.find((stream) => stream.codec_type === 'audio');
  if (!videoStream && !audioStream) throw new Error('ffprobe 未发现音视频流');

  const streamDurations = streams
    .map(streamDuration)
    .filter((value): value is number => value !== null);
  const durationSeconds = optionalNumber(document.format?.duration)
    ?? (streamDurations.length > 0 ? Math.max(...streamDurations) : null);

  return {
    durationSeconds,
    formatName: optionalString(document.format?.format_name),
    video: videoStream ? parseVideoStream(videoStream) : null,
    audio: audioStream ? parseAudioStream(audioStream) : null,
  };
}

export async function probeMedia(
  mediaPath: string,
  options: ProbeMediaOptions = {},
): Promise<MediaProbe> {
  const runner = options.runner ?? runCommand;
  const ffprobePath = options.ffprobePath
    ?? await discoverExecutable('ffprobe', { ...options.discovery, runner });
  const result = await runner(ffprobePath, buildProbeArgs(mediaPath));
  return parseMediaProbe(result.stdout);
}

function assertDimension(value: number, label: string): void {
  if (!Number.isInteger(value) || value <= 0 || value % 2 !== 0) {
    throw new Error(`${label}必须是正偶数`);
  }
}

/** 构建最终 MP4 的固定发布参数；所有路径均保留为独立 argv 元素。 */
export function buildFinalVideoArgs(options: FinalVideoOptions): string[] {
  assertNonEmpty(options.videoPath, '视频输入路径');
  assertNonEmpty(options.audioPath, '音频输入路径');
  assertNonEmpty(options.outputPath, '视频输出路径');
  assertDimension(options.width, '视频宽度');
  assertDimension(options.height, '视频高度');

  return [
    '-hide_banner',
    '-nostdin',
    '-y',
    '-i', options.videoPath,
    '-i', options.audioPath,
    '-map', '0:v:0',
    '-map', '1:a:0',
    '-vf', `scale=${options.width}:${options.height}:flags=lanczos,setsar=1,fps=${DEMO_FPS}`,
    '-r', String(DEMO_FPS),
    '-fps_mode', 'cfr',
    '-frames:v', String(DEMO_TOTAL_FRAMES),
    '-c:v', 'libx264',
    '-preset', 'slow',
    '-crf', '18',
    '-pix_fmt', 'yuv420p',
    '-c:a', 'aac',
    '-b:a', '192k',
    '-ar', '48000',
    '-ac', '2',
    '-t', DEMO_DURATION_SECONDS.toFixed(3),
    '-movflags', '+faststart',
    options.outputPath,
  ];
}

export function buildLoudnormAnalysisArgs(inputPath: string): string[] {
  assertNonEmpty(inputPath, '响度分析输入路径');
  return [
    '-hide_banner',
    '-nostdin',
    '-i', inputPath,
    '-af', `${LOUDNESS_FILTER}:print_format=json`,
    '-f', 'null',
    '-',
  ];
}

function finiteLoudnormField(
  record: Record<string, unknown>,
  sourceName: string,
  targetName: keyof LoudnormMeasurement,
  target: Partial<LoudnormMeasurement>,
): void {
  const parsed = optionalNumber(record[sourceName]);
  if (parsed === null) throw new Error(`loudnorm 首遍结果缺少有效字段 ${sourceName}`);
  target[targetName] = parsed;
}

export function parseLoudnormMeasurement(stderr: string): LoudnormMeasurement {
  const jsonBlocks = stderr.match(/\{[^{}]*\}/gs) ?? [];
  for (const block of jsonBlocks.reverse()) {
    let record: Record<string, unknown>;
    try {
      record = JSON.parse(block) as Record<string, unknown>;
    } catch {
      continue;
    }
    if (!('input_i' in record)) continue;

    const measurement: Partial<LoudnormMeasurement> = {};
    finiteLoudnormField(record, 'input_i', 'inputI', measurement);
    finiteLoudnormField(record, 'input_tp', 'inputTp', measurement);
    finiteLoudnormField(record, 'input_lra', 'inputLra', measurement);
    finiteLoudnormField(record, 'input_thresh', 'inputThresh', measurement);
    finiteLoudnormField(record, 'target_offset', 'targetOffset', measurement);
    return measurement as LoudnormMeasurement;
  }
  throw new Error('未在 FFmpeg stderr 中找到有效的 loudnorm JSON 字段');
}

export function buildLoudnormRenderArgs(
  inputPath: string,
  outputPath: string,
  measurement: LoudnormMeasurement,
): string[] {
  assertNonEmpty(inputPath, '响度渲染输入路径');
  assertNonEmpty(outputPath, '响度渲染输出路径');
  const values = Object.values(measurement);
  if (values.some((value) => !Number.isFinite(value))) {
    throw new Error('loudnorm 测量值必须全部为有限数');
  }

  const filter = [
    LOUDNESS_FILTER,
    `measured_I=${measurement.inputI}`,
    `measured_TP=${measurement.inputTp}`,
    `measured_LRA=${measurement.inputLra}`,
    `measured_thresh=${measurement.inputThresh}`,
    `offset=${measurement.targetOffset}`,
    'linear=true',
    'print_format=summary',
  ].join(':');

  return [
    '-hide_banner',
    '-nostdin',
    '-y',
    '-i', inputPath,
    '-af', filter,
    '-ar', '48000',
    '-ac', '2',
    '-c:a', 'pcm_s24le',
    '-t', DEMO_DURATION_SECONDS.toFixed(3),
    outputPath,
  ];
}
