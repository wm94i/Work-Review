import { mkdir } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';
import type { StoryboardCue } from './types.ts';
import { DEMO_DURATION_SECONDS } from './storyboard.ts';
import {
  buildLoudnormAnalysisArgs,
  buildLoudnormRenderArgs,
  discoverMediaBinaries,
  parseLoudnormMeasurement,
  probeMedia,
  runCommand,
  type CommandResult,
  type CommandRunner,
  type MediaProbe,
} from './ffmpeg.ts';

export const AUDIO_SAMPLE_RATE = 48_000;
export const AUDIO_CHANNELS = 2;
export const LAST_VOICEOVER_END_SECONDS = 83;
const DUCKING_GAIN = 0.316228; // -10 dB，位于规格要求的 8–12 dB 范围内。
const TRUE_PEAK_LIMIT = 0.891251; // -1 dBFS，最终仍会再做两遍 loudnorm。

export interface CommandDescription {
  binary: string;
  args: string[];
}

export interface VoiceoverInput {
  path: string;
  startSeconds: number;
}

export type SoundEffectKind = 'click' | 'switch' | 'complete' | 'outro';

export interface SoundEffectCue {
  kind: SoundEffectKind;
  atSeconds: number;
  frequency: number;
  durationSeconds: number;
  gain: number;
}

export const DEFAULT_SOUND_EFFECTS: readonly SoundEffectCue[] = [
  { kind: 'click', atSeconds: 8.4, frequency: 880, durationSeconds: 0.09, gain: 0.08 },
  { kind: 'switch', atSeconds: 41.2, frequency: 660, durationSeconds: 0.16, gain: 0.07 },
  { kind: 'complete', atSeconds: 29.4, frequency: 990, durationSeconds: 0.28, gain: 0.075 },
  { kind: 'outro', atSeconds: 78.4, frequency: 523.25, durationSeconds: 0.8, gain: 0.055 },
] as const;

export interface AudioExecutionOptions {
  ffmpegPath?: string;
  runner?: CommandRunner;
}

export interface BuildVoiceoverOptions extends AudioExecutionOptions {
  outputDir: string;
  ffprobePath?: string;
  probe?: (mediaPath: string) => Promise<MediaProbe>;
}

export interface MixDemoAudioOptions extends AudioExecutionOptions {
  voiceoverPath: string;
  musicPath: string;
  effectsPath: string;
  outputPath: string;
  voiceoverCues: readonly StoryboardCue[];
}

export interface BuildAudioMixOptions {
  voiceoverPath: string;
  musicPath: string;
  effectsPath: string;
  outputPath: string;
  voiceoverCues: readonly StoryboardCue[];
}

function assertPath(value: string, label: string): void {
  if (!value.trim()) throw new Error(`${label}不能为空`);
}

function assertFiniteTime(value: number, label: string): void {
  if (!Number.isFinite(value) || value < 0) throw new Error(`${label}必须是非负有限数`);
}

function formatSeconds(value: number): string {
  return value.toFixed(3);
}

function milliseconds(value: number): number {
  return Math.round(value * 1000);
}

function pcmOutputArgs(outputPath: string): string[] {
  return [
    '-ar', String(AUDIO_SAMPLE_RATE),
    '-ac', String(AUDIO_CHANNELS),
    '-c:a', 'pcm_s24le',
    '-t', DEMO_DURATION_SECONDS.toFixed(3),
    outputPath,
  ];
}

async function resolveFfmpegPath(options: AudioExecutionOptions): Promise<string> {
  if (options.ffmpegPath) return options.ffmpegPath;
  return (await discoverMediaBinaries({ runner: options.runner })).ffmpeg;
}

export function buildSayVoiceListCommand(): CommandDescription {
  return { binary: '/usr/bin/say', args: ['-v', '?'] };
}

export function hasTingtingVoice(voiceList: string): boolean {
  return voiceList
    .split(/\r?\n/)
    .some((line) => /^\s*Tingting\s+/i.test(line) && /\bzh[_-]CN\b/i.test(line));
}

export function buildSayCommand(text: string, outputPath: string): CommandDescription {
  if (!text.trim()) throw new Error('旁白文本不能为空');
  assertPath(outputPath, '旁白 AIFF 输出路径');
  return {
    binary: '/usr/bin/say',
    args: ['-v', 'Tingting', '-o', outputPath, text],
  };
}

export async function runSayCommand(
  command: CommandDescription,
  runner: CommandRunner = runCommand,
): Promise<CommandResult> {
  if (command.binary !== '/usr/bin/say') throw new Error('旁白命令必须使用 /usr/bin/say');
  return runner(command.binary, command.args);
}

export function buildVoiceoverConversionArgs(inputPath: string, outputPath: string): string[] {
  assertPath(inputPath, '旁白 AIFF 输入路径');
  assertPath(outputPath, '旁白 WAV 输出路径');
  return [
    '-hide_banner',
    '-nostdin',
    '-y',
    '-i', inputPath,
    '-vn',
    '-ar', String(AUDIO_SAMPLE_RATE),
    '-ac', String(AUDIO_CHANNELS),
    '-c:a', 'pcm_s24le',
    outputPath,
  ];
}

export function buildVoiceoverMixArgs(
  inputs: readonly VoiceoverInput[],
  outputPath: string,
): string[] {
  if (inputs.length === 0) throw new Error('旁白混音至少需要一个输入');
  assertPath(outputPath, '旁白混音输出路径');

  const args = ['-hide_banner', '-nostdin', '-y'];
  for (const input of inputs) {
    assertPath(input.path, '旁白片段路径');
    assertFiniteTime(input.startSeconds, '旁白开始时间');
    args.push('-i', input.path);
  }

  const chains = inputs.map((input, index) => {
    const delay = milliseconds(input.startSeconds);
    return `[${index}:a]aformat=sample_rates=${AUDIO_SAMPLE_RATE}:channel_layouts=stereo,adelay=${delay}|${delay}[voice${index}]`;
  });
  const labels = inputs.map((_, index) => `[voice${index}]`).join('');
  chains.push(
    `${labels}amix=inputs=${inputs.length}:normalize=0:duration=longest,`
      + `atrim=0:${DEMO_DURATION_SECONDS},apad=whole_dur=${DEMO_DURATION_SECONDS}[voiceout]`,
  );

  args.push(
    '-filter_complex', chains.join(';'),
    '-map', '[voiceout]',
    ...pcmOutputArgs(outputPath),
  );
  return args;
}

function safeCueFileStem(index: number, cue: StoryboardCue): string {
  const safeId = cue.id.replace(/[^a-zA-Z0-9_-]+/g, '-').replace(/^-+|-+$/g, '') || 'cue';
  return `${String(index + 1).padStart(2, '0')}-${safeId}`;
}

function validateVoiceoverCue(cue: StoryboardCue): void {
  assertFiniteTime(cue.start, `旁白 ${cue.id} 开始时间`);
  assertFiniteTime(cue.end, `旁白 ${cue.id} 结束时间`);
  if (cue.end <= cue.start) throw new Error(`旁白 ${cue.id} 的结束时间必须晚于开始时间`);
  if (!cue.zh.trim()) throw new Error(`旁白 ${cue.id} 缺少中文文本`);
}

export async function buildVoiceover(
  cues: readonly StoryboardCue[],
  options: BuildVoiceoverOptions,
): Promise<string> {
  if (cues.length === 0) throw new Error('至少需要一条中文旁白');
  assertPath(options.outputDir, '旁白输出目录');
  await mkdir(options.outputDir, { recursive: true });

  const runner = options.runner ?? runCommand;
  const voiceListCommand = buildSayVoiceListCommand();
  const voiceList = await runner(voiceListCommand.binary, voiceListCommand.args);
  if (!hasTingtingVoice(voiceList.stdout)) {
    throw new Error('系统未提供中文 Tingting 语音，无法生成批准的演示旁白');
  }

  let ffmpegPath = options.ffmpegPath;
  let ffprobePath = options.ffprobePath;
  if (!ffmpegPath || (!ffprobePath && !options.probe)) {
    const binaries = await discoverMediaBinaries({ runner });
    ffmpegPath ??= binaries.ffmpeg;
    ffprobePath ??= binaries.ffprobe;
  }

  const probe = options.probe ?? ((mediaPath: string) => probeMedia(mediaPath, {
    ffprobePath,
    runner,
  }));
  const mixInputs: VoiceoverInput[] = [];

  for (const [index, cue] of cues.entries()) {
    validateVoiceoverCue(cue);
    const stem = safeCueFileStem(index, cue);
    const aiffPath = join(options.outputDir, `${stem}.aiff`);
    const wavPath = join(options.outputDir, `${stem}.wav`);
    await runSayCommand(buildSayCommand(cue.zh, aiffPath), runner);
    await runner(ffmpegPath, buildVoiceoverConversionArgs(aiffPath, wavPath));

    const media = await probe(wavPath);
    const actualDuration = media.audio?.durationSeconds ?? media.durationSeconds;
    if (actualDuration === null || !Number.isFinite(actualDuration)) {
      throw new Error(`无法确定旁白 ${cue.id} 的实际时长`);
    }
    const deadline = index === cues.length - 1
      ? Math.min(cue.end, LAST_VOICEOVER_END_SECONDS)
      : cue.end;
    const availableDuration = deadline - cue.start;
    if (availableDuration <= 0) {
      throw new Error(`旁白 ${cue.id} 没有可用时间，必须在 ${formatSeconds(deadline)} 秒前结束`);
    }
    if (actualDuration > availableDuration) {
      throw new Error(
        `旁白 ${cue.id} 实际 ${formatSeconds(actualDuration)} 秒，超过可用的 `
          + `${formatSeconds(availableDuration)} 秒；必须在 ${formatSeconds(deadline)} 秒前结束，`
          + '不会通过强制加速降低可懂度。',
      );
    }
    mixInputs.push({ path: wavPath, startSeconds: cue.start });
  }

  const outputPath = join(options.outputDir, 'voiceover.wav');
  await runner(ffmpegPath, buildVoiceoverMixArgs(mixInputs, outputPath));
  return outputPath;
}

export function buildBackgroundMusicArgs(outputPath: string): string[] {
  assertPath(outputPath, '背景音乐输出路径');
  const args = ['-hide_banner', '-nostdin', '-y'];
  const tones = [110, 164.81, 220];
  for (const frequency of tones) {
    args.push(
      '-f', 'lavfi',
      '-i', `sine=frequency=${frequency}:sample_rate=${AUDIO_SAMPLE_RATE}:duration=${DEMO_DURATION_SECONDS}`,
    );
  }

  const filter = [
    '[0:a]volume=0.035[base]',
    '[1:a]volume=0.020[mid]',
    '[2:a]volume=0.012[air]',
    '[base][mid][air]amix=inputs=3:normalize=0:duration=longest,'
      + 'lowpass=f=1200,highpass=f=70,aformat=sample_rates=48000:channel_layouts=stereo,'
      + `afade=t=in:st=0:d=2,afade=t=out:st=80:d=5,atrim=0:${DEMO_DURATION_SECONDS}[musicout]`,
  ].join(';');

  args.push(
    '-filter_complex', filter,
    '-map', '[musicout]',
    ...pcmOutputArgs(outputPath),
  );
  return args;
}

export async function buildBackgroundMusic(
  outputPath: string,
  options: AudioExecutionOptions = {},
): Promise<string> {
  await mkdir(dirname(outputPath), { recursive: true });
  const runner = options.runner ?? runCommand;
  const ffmpegPath = await resolveFfmpegPath(options);
  await runner(ffmpegPath, buildBackgroundMusicArgs(outputPath));
  return outputPath;
}

function validateSoundEffect(effect: SoundEffectCue): void {
  assertFiniteTime(effect.atSeconds, `${effect.kind} 音效时间`);
  if (!Number.isFinite(effect.frequency) || effect.frequency <= 0) {
    throw new Error(`${effect.kind} 音效频率必须是正有限数`);
  }
  if (!Number.isFinite(effect.durationSeconds) || effect.durationSeconds <= 0) {
    throw new Error(`${effect.kind} 音效时长必须是正有限数`);
  }
  if (!Number.isFinite(effect.gain) || effect.gain <= 0 || effect.gain > 1) {
    throw new Error(`${effect.kind} 音效增益必须位于 0 到 1 之间`);
  }
  if (effect.atSeconds + effect.durationSeconds > DEMO_DURATION_SECONDS) {
    throw new Error(`${effect.kind} 音效超过 ${DEMO_DURATION_SECONDS} 秒时间线`);
  }
}

export function buildSoundEffectsArgs(
  effects: readonly SoundEffectCue[],
  outputPath: string,
): string[] {
  if (effects.length === 0) throw new Error('至少需要一个程序化音效');
  assertPath(outputPath, '音效输出路径');
  const args = ['-hide_banner', '-nostdin', '-y'];

  for (const effect of effects) {
    validateSoundEffect(effect);
    args.push(
      '-f', 'lavfi',
      '-i', `sine=frequency=${effect.frequency}:sample_rate=${AUDIO_SAMPLE_RATE}:duration=${effect.durationSeconds}`,
    );
  }

  const chains = effects.map((effect, index) => {
    const fadeStart = Math.max(0, effect.durationSeconds - Math.min(0.12, effect.durationSeconds / 2));
    const fadeDuration = effect.durationSeconds - fadeStart;
    const delay = milliseconds(effect.atSeconds);
    return `[${index}:a]volume=${effect.gain},afade=t=out:st=${formatSeconds(fadeStart)}:`
      + `d=${formatSeconds(fadeDuration)},aformat=sample_rates=48000:channel_layouts=stereo,`
      + `adelay=${delay}|${delay}[effect${index}]`;
  });
  const labels = effects.map((_, index) => `[effect${index}]`).join('');
  chains.push(
    `${labels}amix=inputs=${effects.length}:normalize=0:duration=longest,`
      + `atrim=0:${DEMO_DURATION_SECONDS},apad=whole_dur=${DEMO_DURATION_SECONDS}[effectsout]`,
  );

  args.push(
    '-filter_complex', chains.join(';'),
    '-map', '[effectsout]',
    ...pcmOutputArgs(outputPath),
  );
  return args;
}

export async function buildSoundEffects(
  outputPath: string,
  options: AudioExecutionOptions & { effects?: readonly SoundEffectCue[] } = {},
): Promise<string> {
  await mkdir(dirname(outputPath), { recursive: true });
  const runner = options.runner ?? runCommand;
  const ffmpegPath = await resolveFfmpegPath(options);
  await runner(
    ffmpegPath,
    buildSoundEffectsArgs(options.effects ?? DEFAULT_SOUND_EFFECTS, outputPath),
  );
  return outputPath;
}

function buildDuckingExpression(cues: readonly StoryboardCue[]): string {
  if (cues.length === 0) return '';
  return cues.map((cue) => {
    assertFiniteTime(cue.start, `旁白 ${cue.id} 开始时间`);
    assertFiniteTime(cue.end, `旁白 ${cue.id} 结束时间`);
    if (cue.end <= cue.start) throw new Error(`旁白 ${cue.id} 时间范围无效`);
    return `between(t,${formatSeconds(cue.start)},${formatSeconds(cue.end)})`;
  }).join('+');
}

export function buildAudioMixArgs(options: BuildAudioMixOptions): string[] {
  assertPath(options.voiceoverPath, '旁白路径');
  assertPath(options.musicPath, '背景音乐路径');
  assertPath(options.effectsPath, '音效路径');
  assertPath(options.outputPath, '原始混音输出路径');
  const duckingExpression = buildDuckingExpression(options.voiceoverCues);
  const musicVolume = duckingExpression
    ? `volume=${DUCKING_GAIN}:enable='${duckingExpression}'`
    : 'anull';
  const filter = [
    `[0:a]aformat=sample_rates=48000:channel_layouts=stereo[voice]`,
    `[1:a]aformat=sample_rates=48000:channel_layouts=stereo,${musicVolume}[music]`,
    `[2:a]aformat=sample_rates=48000:channel_layouts=stereo[effects]`,
    '[voice][music][effects]amix=inputs=3:normalize=0:duration=longest,'
      + `alimiter=limit=${TRUE_PEAK_LIMIT},atrim=0:${DEMO_DURATION_SECONDS},`
      + `apad=whole_dur=${DEMO_DURATION_SECONDS}[mixout]`,
  ].join(';');

  return [
    '-hide_banner',
    '-nostdin',
    '-y',
    '-i', options.voiceoverPath,
    '-i', options.musicPath,
    '-i', options.effectsPath,
    '-filter_complex', filter,
    '-map', '[mixout]',
    ...pcmOutputArgs(options.outputPath),
  ];
}

function unmasteredPath(outputPath: string): string {
  const extension = basename(outputPath).match(/(\.[^.]+)$/)?.[1] ?? '.wav';
  const stem = basename(outputPath, extension);
  return join(dirname(outputPath), `${stem}.unmastered${extension}`);
}

/**
 * 执行两遍响度规范化：第一遍只分析，第二遍使用测量值线性渲染。
 */
export async function mixDemoAudio(options: MixDemoAudioOptions): Promise<string> {
  await mkdir(dirname(options.outputPath), { recursive: true });
  const runner = options.runner ?? runCommand;
  const ffmpegPath = await resolveFfmpegPath(options);
  const rawMixPath = unmasteredPath(options.outputPath);

  await runner(ffmpegPath, buildAudioMixArgs({
    voiceoverPath: options.voiceoverPath,
    musicPath: options.musicPath,
    effectsPath: options.effectsPath,
    outputPath: rawMixPath,
    voiceoverCues: options.voiceoverCues,
  }));
  const analysis = await runner(ffmpegPath, buildLoudnormAnalysisArgs(rawMixPath));
  const measurement = parseLoudnormMeasurement(`${analysis.stdout}\n${analysis.stderr}`);
  await runner(
    ffmpegPath,
    buildLoudnormRenderArgs(rawMixPath, options.outputPath, measurement),
  );
  return options.outputPath;
}
