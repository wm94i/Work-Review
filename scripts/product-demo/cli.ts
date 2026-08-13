import { spawn } from 'node:child_process';
import {
  copyFile,
  mkdir,
  readFile,
  writeFile,
} from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import type { ExtractKeyframeOcrOptions, KeyframeOcrResult } from './ocr.ts';
import type { DemoAspect, StoryboardSceneId } from './types.ts';
import type { KeyframeOcrFrame, KeyframeOcrRunner } from './validate.ts';

export type DemoCommand =
  | 'check'
  | 'capture'
  | 'subtitles'
  | 'audio'
  | 'compose'
  | 'validate'
  | 'build';

export interface ParsedCliArgs {
  command: DemoCommand;
  scene?: StoryboardSceneId;
  aspect?: DemoAspect;
}

export interface DemoPaths {
  root: string;
  intermediate: string;
  subtitleSources: string;
  subtitleOverlays: string;
  audio: string;
  voiceover: string;
  logs: string;
  exports: string;
  voiceoverMix: string;
  backgroundMusic: string;
  soundEffects: string;
  finalMix: string;
  consoleLog: string;
  networkLog: string;
  exportMarkdown: string;
  subtitleZh: string;
  subtitleEn: string;
  subtitleBilingual: string;
  video16x9: string;
  video9x16: string;
  cover16x9: string;
  cover9x16: string;
}

export interface CliCommandContext {
  cwd: string;
  paths: DemoPaths;
  args: ParsedCliArgs;
}

export interface CliDependencies {
  check(context: CliCommandContext): Promise<void>;
  capture(context: CliCommandContext): Promise<void>;
  subtitles(context: CliCommandContext): Promise<void>;
  audio(context: CliCommandContext): Promise<void>;
  compose(context: CliCommandContext): Promise<void>;
  validate(context: CliCommandContext): Promise<void>;
}

export interface RunCliOptions {
  cwd?: string;
  dependencies?: CliDependencies;
}

export const BUILD_COMMAND_ORDER = [
  'check',
  'capture',
  'subtitles',
  'audio',
  'compose',
  'validate',
] as const satisfies readonly Exclude<DemoCommand, 'build'>[];

const COMMANDS = new Set<DemoCommand>([...BUILD_COMMAND_ORDER, 'build']);
const SCENE_ORDER = [
  'hook',
  'timeline',
  'report',
  'assistant',
  'privacy',
  'export',
  'outro',
] as const satisfies readonly StoryboardSceneId[];
const ASPECT_ORDER = ['16x9', '9x16'] as const satisfies readonly DemoAspect[];
const SCENES = new Set<StoryboardSceneId>(SCENE_ORDER);
const ASPECTS = new Set<DemoAspect>(ASPECT_ORDER);

export function createDemoPaths(cwd: string): DemoPaths {
  const root = path.resolve(cwd, 'artifacts', 'product-demo');
  const intermediate = path.join(root, 'intermediate');
  const subtitleSources = path.join(intermediate, 'subtitles');
  const subtitleOverlays = path.join(intermediate, 'subtitle-overlays');
  const audio = path.join(intermediate, 'audio');
  const voiceover = path.join(audio, 'voiceover');
  const logs = path.join(intermediate, 'logs');
  const exports = path.join(intermediate, 'exports');

  return {
    root,
    intermediate,
    subtitleSources,
    subtitleOverlays,
    audio,
    voiceover,
    logs,
    exports,
    voiceoverMix: path.join(voiceover, 'voiceover.wav'),
    backgroundMusic: path.join(audio, 'background-music.wav'),
    soundEffects: path.join(audio, 'sound-effects.wav'),
    finalMix: path.join(audio, 'work-review-demo-mix.wav'),
    consoleLog: path.join(logs, 'playwright-console.json'),
    networkLog: path.join(logs, 'playwright-network.json'),
    exportMarkdown: path.join(exports, '2026-08-12.md'),
    subtitleZh: path.join(root, 'work-review-demo-zh.srt'),
    subtitleEn: path.join(root, 'work-review-demo-en.srt'),
    subtitleBilingual: path.join(root, 'work-review-demo-zh-en.srt'),
    video16x9: path.join(root, 'work-review-demo-16x9.mp4'),
    video9x16: path.join(root, 'work-review-demo-9x16.mp4'),
    cover16x9: path.join(root, 'work-review-demo-cover-16x9.png'),
    cover9x16: path.join(root, 'work-review-demo-cover-9x16.png'),
  };
}

function requireOptionValue(argv: readonly string[], index: number, option: string): string {
  const value = argv[index + 1];
  if (!value || value.startsWith('--')) throw new Error(`参数 ${option} 缺少值`);
  return value;
}

export function parseCliArgs(argv: readonly string[]): ParsedCliArgs {
  const commandValue = argv[0];
  if (!commandValue) {
    throw new Error(
      '缺少命令。支持：check、capture、subtitles、audio、compose、validate、build',
    );
  }
  if (!COMMANDS.has(commandValue as DemoCommand)) {
    throw new Error(`未知命令：${commandValue}`);
  }

  const command = commandValue as DemoCommand;
  if (command !== 'capture') {
    if (argv.length > 1) {
      throw new Error(`${command} 命令不支持参数：${argv.slice(1).join(' ')}`);
    }
    return { command };
  }

  let scene: StoryboardSceneId | undefined;
  let aspect: DemoAspect | undefined;
  for (let index = 1; index < argv.length; index += 1) {
    const option = argv[index];
    if (option === '--scene') {
      if (scene) throw new Error('参数 --scene 不得重复');
      const value = requireOptionValue(argv, index, option);
      if (!SCENES.has(value as StoryboardSceneId)) throw new Error(`无效分镜：${value}`);
      scene = value as StoryboardSceneId;
      index += 1;
      continue;
    }
    if (option === '--aspect') {
      if (aspect) throw new Error('参数 --aspect 不得重复');
      const value = requireOptionValue(argv, index, option);
      if (!ASPECTS.has(value as DemoAspect)) throw new Error(`无效画幅：${value}`);
      aspect = value as DemoAspect;
      index += 1;
      continue;
    }
    throw new Error(`未知参数：${option}`);
  }

  return { command, ...(scene ? { scene } : {}), ...(aspect ? { aspect } : {}) };
}

function runProcess(binary: string, args: readonly string[], cwd: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, [...args], {
      cwd,
      shell: false,
      stdio: 'inherit',
      windowsHide: true,
    });
    child.once('error', (error) => reject(new Error(`无法启动 ${binary}：${error.message}`)));
    child.once('close', (exitCode, signal) => {
      if (exitCode === 0) {
        resolve();
        return;
      }
      const status = exitCode === null ? `信号 ${signal ?? 'unknown'}` : `退出码 ${exitCode}`;
      reject(new Error(`命令 ${binary} 执行失败（${status}）`));
    });
  });
}

function relativeArtifactPath(paths: DemoPaths, absolutePath: string): string {
  const relative = path.relative(paths.root, absolutePath);
  if (!relative || relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error(`产物路径逃逸 artifacts/product-demo：${absolutePath}`);
  }
  return relative.split(path.sep).join('/');
}

export interface CaptureDiagnosticIdentity {
  scene: StoryboardSceneId;
  aspect: DemoAspect;
}

function captureDiagnosticKey(entry: CaptureDiagnosticIdentity): string {
  return `${entry.aspect}:${entry.scene}`;
}

function assertCaptureDiagnosticIdentity(
  value: unknown,
  label: string,
): asserts value is CaptureDiagnosticIdentity & Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label}必须是对象`);
  }
  const entry = value as Record<string, unknown>;
  if (!SCENES.has(entry.scene as StoryboardSceneId)) {
    throw new Error(`${label}包含无效分镜：${String(entry.scene)}`);
  }
  if (!ASPECTS.has(entry.aspect as DemoAspect)) {
    throw new Error(`${label}包含无效画幅：${String(entry.aspect)}`);
  }
}

function diagnosticMap<T extends CaptureDiagnosticIdentity>(
  entries: readonly T[],
  label: string,
): Map<string, T> {
  const result = new Map<string, T>();
  entries.forEach((entry, index) => {
    assertCaptureDiagnosticIdentity(entry, `${label}第 ${index + 1} 项`);
    const key = captureDiagnosticKey(entry);
    if (result.has(key)) throw new Error(`${label}存在重复画幅分镜组合：${entry.aspect} ${entry.scene}`);
    result.set(key, entry);
  });
  return result;
}

/** 局部重录时按画幅和分镜覆盖对应诊断，同时保留其他镜头。 */
export function mergeCaptureDiagnosticEntries<T extends CaptureDiagnosticIdentity>(
  existing: readonly T[],
  incoming: readonly T[],
): T[] {
  const merged = diagnosticMap(existing, '已有诊断');
  const updates = diagnosticMap(incoming, '新增诊断');
  for (const [key, entry] of updates) merged.set(key, entry);
  return ASPECT_ORDER.flatMap((aspect) => SCENE_ORDER.flatMap((scene) => {
    const entry = merged.get(`${aspect}:${scene}`);
    return entry ? [entry] : [];
  }));
}

function requireDiagnosticArray(
  entry: CaptureDiagnosticIdentity & Record<string, unknown>,
  field: string,
  label: string,
): readonly unknown[] {
  const value = entry[field];
  if (!Array.isArray(value)) {
    throw new Error(`${label} ${entry.aspect} ${entry.scene} 的 ${field} 必须是数组`);
  }
  return value;
}

/** 最终交付前强制确认横竖屏七镜头的浏览器诊断完整且为空。 */
export function assertCompleteCaptureDiagnosticEntries(
  consoleEntries: readonly unknown[],
  networkEntries: readonly unknown[],
): void {
  const consoleMap = diagnosticMap(
    consoleEntries as Array<CaptureDiagnosticIdentity & Record<string, unknown>>,
    '控制台诊断',
  );
  const networkMap = diagnosticMap(
    networkEntries as Array<CaptureDiagnosticIdentity & Record<string, unknown>>,
    '网络诊断',
  );

  for (const aspect of ASPECT_ORDER) {
    for (const scene of SCENE_ORDER) {
      const key = `${aspect}:${scene}`;
      const consoleEntry = consoleMap.get(key);
      if (!consoleEntry) throw new Error(`控制台诊断缺少 ${aspect} ${scene}`);
      const networkEntry = networkMap.get(key);
      if (!networkEntry) throw new Error(`网络诊断缺少 ${aspect} ${scene}`);

      for (const field of ['pageErrors', 'consoleErrors', 'unhandledCommands'] as const) {
        const values = requireDiagnosticArray(consoleEntry, field, '控制台诊断');
        if (values.length > 0) {
          throw new Error(`${aspect} ${scene} 的 ${field} 非空：${values.map(String).join('；')}`);
        }
      }
      const blockedRequests = requireDiagnosticArray(networkEntry, 'blockedRequests', '网络诊断');
      if (blockedRequests.length > 0) {
        throw new Error(`${aspect} ${scene} 的 blockedRequests 非空：${blockedRequests.map(String).join('；')}`);
      }
    }
  }
}

async function readDiagnosticEntries(filePath: string): Promise<Array<CaptureDiagnosticIdentity & Record<string, unknown>>> {
  let raw: string;
  try {
    raw = await readFile(filePath, 'utf8');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return [];
    throw error;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`录制诊断 JSON 无效：${filePath}（${message}）`);
  }
  if (!Array.isArray(parsed)) throw new Error(`录制诊断必须是数组：${filePath}`);
  parsed.forEach((entry, index) => assertCaptureDiagnosticIdentity(entry, `录制诊断第 ${index + 1} 项`));
  return parsed as Array<CaptureDiagnosticIdentity & Record<string, unknown>>;
}

export interface CompositionCaptureMetadata {
  metadataPaths: string[];
  sourceStartOffsetsSeconds: number[];
  sourceDurationsSeconds: number[];
}

export interface LoadCompositionCaptureMetadataOptions {
  intermediateDir: string;
  aspect: DemoAspect;
  sceneIds: readonly StoryboardSceneId[];
}

function requireMetadataObject(value: unknown, metadataPath: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`分镜 metadata 必须是对象：${metadataPath}`);
  }
  return value as Record<string, unknown>;
}

/** 按故事板顺序读取录制元数据，避免预卷进入正式成片。 */
export async function loadCompositionCaptureMetadata(
  options: LoadCompositionCaptureMetadataOptions,
): Promise<CompositionCaptureMetadata> {
  const metadataPaths: string[] = [];
  const sourceStartOffsetsSeconds: number[] = [];
  const sourceDurationsSeconds: number[] = [];

  for (const expectedScene of options.sceneIds) {
    const metadataPath = path.join(options.intermediateDir, options.aspect, `${expectedScene}.json`);
    let raw: string;
    try {
      raw = await readFile(metadataPath, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
        throw new Error(`缺少分镜 metadata：${expectedScene}（${options.aspect}）`);
      }
      throw error;
    }

    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`分镜 metadata JSON 无效：${expectedScene}（${message}）`);
    }
    const metadata = requireMetadataObject(parsed, metadataPath);

    if (metadata.scene !== expectedScene) {
      throw new Error(
        `分镜 metadata 分镜不匹配：期望 ${expectedScene}，实际 ${String(metadata.scene)}`,
      );
    }
    if (metadata.aspect !== options.aspect) {
      throw new Error(
        `分镜 metadata 画幅不匹配：期望 ${options.aspect}，实际 ${String(metadata.aspect)}`,
      );
    }

    const durationSeconds = metadata.durationSeconds;
    if (typeof durationSeconds !== 'number' || !Number.isFinite(durationSeconds) || durationSeconds <= 0) {
      throw new Error(`分镜 ${expectedScene} 源时长必须是正有限数`);
    }
    const contentStartOffsetSeconds = metadata.contentStartOffsetSeconds;
    if (
      typeof contentStartOffsetSeconds !== 'number'
      || !Number.isFinite(contentStartOffsetSeconds)
      || contentStartOffsetSeconds < 0
    ) {
      throw new Error(`分镜 ${expectedScene} 内容起点必须是非负有限数`);
    }
    if (contentStartOffsetSeconds >= durationSeconds) {
      throw new Error(`分镜 ${expectedScene} 内容起点必须小于源时长`);
    }

    metadataPaths.push(metadataPath);
    sourceStartOffsetsSeconds.push(contentStartOffsetSeconds);
    sourceDurationsSeconds.push(durationSeconds);
  }

  return { metadataPaths, sourceStartOffsetsSeconds, sourceDurationsSeconds };
}

async function runCheck(context: CliCommandContext): Promise<void> {
  const [{ validateStoryboard }, { createDemoFixtures }, privacy] = await Promise.all([
    import('./storyboard.ts'),
    import('./fixtures.ts'),
    import('./privacy.ts'),
  ]);
  validateStoryboard();
  privacy.assertNoSensitiveContent(
    privacy.scanDemoObject(createDemoFixtures(), 'demo-fixtures'),
  );
  await runProcess(process.execPath, [
    '--test-timeout=60000',
    '--import',
    'tsx',
    '--test',
    'scripts/product-demo/*.test.ts',
  ], context.cwd);
}

export async function writeCaptureSupportFiles(
  context: CliCommandContext,
  results: readonly Awaited<ReturnType<typeof import('./capture.ts')['captureAllScenes']>>[number][],
): Promise<void> {
  await Promise.all([
    mkdir(context.paths.logs, { recursive: true }),
    mkdir(context.paths.exports, { recursive: true }),
  ]);

  const consoleEntries = results.map((result) => ({
    scene: result.scene,
    aspect: result.aspect,
    pageErrors: result.diagnostics.pageErrors,
    consoleErrors: result.diagnostics.consoleErrors,
    unhandledCommands: result.diagnostics.unhandledCommands,
    invokeCommands: result.invokeCommands,
  }));
  const networkEntries = results.map((result) => ({
    scene: result.scene,
    aspect: result.aspect,
    blockedRequests: result.diagnostics.blockedRequests,
    policy: 'same-origin-data-blob-only',
  }));

  const fullCapture = !context.args.scene && !context.args.aspect;
  const [existingConsoleEntries, existingNetworkEntries] = fullCapture
    ? [[], []]
    : await Promise.all([
        readDiagnosticEntries(context.paths.consoleLog),
        readDiagnosticEntries(context.paths.networkLog),
      ]);
  const mergedConsoleEntries = mergeCaptureDiagnosticEntries(existingConsoleEntries, consoleEntries);
  const mergedNetworkEntries = mergeCaptureDiagnosticEntries(existingNetworkEntries, networkEntries);

  const exportResults = results.filter((result) => result.scene === 'export');
  if (fullCapture) {
    for (const aspect of ASPECT_ORDER) {
      if (!exportResults.some((result) => result.aspect === aspect && result.exportedMarkdown)) {
        throw new Error(`完整录制缺少 ${aspect} export 镜头的 UI 实际导出 Markdown`);
      }
    }
  }

  const exportedMarkdown = exportResults
    .map((result) => result.exportedMarkdown)
    .filter((value) => value !== undefined);
  const expectedFileName = path.basename(context.paths.exportMarkdown);
  for (const exported of exportedMarkdown) {
    if (exported.fileName !== expectedFileName) {
      throw new Error(`UI 实际导出文件名错误：期望 ${expectedFileName}，实际 ${exported.fileName}`);
    }
    if (!exported.content.trim()) throw new Error('UI 实际导出 Markdown 内容不能为空');
  }
  if (new Set(exportedMarkdown.map((value) => value.content)).size > 1) {
    throw new Error('横竖屏 export 镜头的 UI 实际导出 Markdown 内容不一致');
  }

  const writes: Promise<void>[] = [
    writeFile(context.paths.consoleLog, `${JSON.stringify(mergedConsoleEntries, null, 2)}
`, 'utf8'),
    writeFile(context.paths.networkLog, `${JSON.stringify(mergedNetworkEntries, null, 2)}
`, 'utf8'),
  ];
  const capturedExport = exportedMarkdown[0];
  if (capturedExport) {
    writes.push(writeFile(
      context.paths.exportMarkdown,
      `${capturedExport.content.trimEnd()}
`,
      'utf8',
    ));
  }
  await Promise.all(writes);
}

async function runCapture(context: CliCommandContext): Promise<void> {
  const { captureAllScenes } = await import('./capture.ts');
  const results = await captureAllScenes({
    outputDir: context.paths.intermediate,
    cwd: context.cwd,
    ...(context.args.scene ? { sceneIds: [context.args.scene] } : {}),
    ...(context.args.aspect ? { aspects: [context.args.aspect] } : {}),
  });
  await writeCaptureSupportFiles(context, results);
}

async function runSubtitles(context: CliCommandContext): Promise<void> {
  const { writeSubtitleArtifacts } = await import('./subtitles.ts');
  const generated = await writeSubtitleArtifacts(context.paths.subtitleSources);
  await mkdir(context.paths.root, { recursive: true });
  await Promise.all([
    copyFile(generated.zh, context.paths.subtitleZh),
    copyFile(generated.en, context.paths.subtitleEn),
    copyFile(generated.bilingual, context.paths.subtitleBilingual),
  ]);
}

async function runAudio(context: CliCommandContext): Promise<void> {
  const [audio, { VOICEOVER_CUES }] = await Promise.all([
    import('./audio.ts'),
    import('./storyboard.ts'),
  ]);
  const voiceoverPath = await audio.buildVoiceover(VOICEOVER_CUES, {
    outputDir: context.paths.voiceover,
  });
  const musicPath = await audio.buildBackgroundMusic(context.paths.backgroundMusic);
  const effectsPath = await audio.buildSoundEffects(context.paths.soundEffects);
  await audio.mixDemoAudio({
    voiceoverPath,
    musicPath,
    effectsPath,
    outputPath: context.paths.finalMix,
    voiceoverCues: VOICEOVER_CUES,
  });
}

async function runCompose(context: CliCommandContext): Promise<void> {
  const [{ launchDemoBrowser }, composition, { STORYBOARD }] = await Promise.all([
    import('./browser.ts'),
    import('./compose.ts'),
    import('./storyboard.ts'),
  ]);
  const browser = await launchDemoBrowser();

  try {
    for (const aspect of ['16x9', '9x16'] as const) {
      const subtitlePngPaths = await composition.renderSubtitleOverlayPngs({
        browser,
        aspect,
        outputDir: context.paths.subtitleOverlays,
      });
      const sceneVideoPaths = STORYBOARD.map((scene) => (
        path.join(context.paths.intermediate, aspect, `${scene.id}.webm`)
      ));
      const captureMetadata = await loadCompositionCaptureMetadata({
        intermediateDir: context.paths.intermediate,
        aspect,
        sceneIds: STORYBOARD.map((scene) => scene.id),
      });
      const outputPath = aspect === '16x9'
        ? context.paths.video16x9
        : context.paths.video9x16;
      const coverPath = aspect === '16x9'
        ? context.paths.cover16x9
        : context.paths.cover9x16;

      await composition.composeDemoVideo({
        aspect,
        sceneVideoPaths,
        subtitlePngPaths,
        mixPath: context.paths.finalMix,
        outputPath,
        sourceDurationsSeconds: captureMetadata.sourceDurationsSeconds,
        sourceStartOffsetsSeconds: captureMetadata.sourceStartOffsetsSeconds,
      });
      await composition.renderCoverPng({
        browser,
        aspect,
        outputPath: coverPath,
        iconPath: path.join(context.cwd, 'public', 'icon.png'),
      });
    }
  } finally {
    await browser.close();
  }
}

async function readPackageVersion(cwd: string): Promise<string> {
  const value = JSON.parse(await readFile(path.join(cwd, 'package.json'), 'utf8')) as {
    version?: unknown;
  };
  if (typeof value.version !== 'string' || !value.version.trim()) {
    throw new Error('package.json 缺少有效版本号');
  }
  return value.version;
}

async function readGitCommit(cwd: string): Promise<string> {
  const { runCommand } = await import('./ffmpeg.ts');
  const result = await runCommand('git', ['rev-parse', 'HEAD'], { cwd });
  const commit = result.stdout.trim();
  if (!/^[0-9a-f]{40}$/u.test(commit)) throw new Error(`无法确定 Git commit：${commit}`);
  return commit;
}

export function buildValidationSourceTextFiles(paths: DemoPaths): string[] {
  const subtitleFiles = [paths.subtitleZh, paths.subtitleEn, paths.subtitleBilingual]
    .map((filePath) => relativeArtifactPath(paths, filePath));
  const metadataFiles = ASPECT_ORDER.flatMap((aspect) => SCENE_ORDER.map((scene) => (
    relativeArtifactPath(paths, path.join(paths.intermediate, aspect, `${scene}.json`))
  )));
  return [...subtitleFiles, ...metadataFiles];
}

function buildValidationOcrTextFiles(paths: DemoPaths): string[] {
  return ASPECT_ORDER.flatMap((aspect) => SCENE_ORDER.flatMap((scene) => (
    ['start', 'action', 'end'].map((phase) => (
      relativeArtifactPath(
        paths,
        path.join(paths.intermediate, 'ocr', aspect, `${scene}-${phase}.txt`),
      )
    ))
  )));
}

type KeyframeOcrExtractor = (
  options: ExtractKeyframeOcrOptions,
) => Promise<KeyframeOcrResult>;

function frameSignature(frames: readonly KeyframeOcrFrame[]): string {
  return JSON.stringify(frames.map(({ aspect, scene, phase, relativePath, absolutePath }) => ({
    aspect,
    scene,
    phase,
    relativePath,
    absolutePath,
  })));
}

/** 初次验证和完整性复验共享同一次真实 OCR，避免重复编译和识别 42 张关键帧。 */
export function createCachedKeyframeOcrRunner(
  outputDir: string,
  injectedExtractor?: KeyframeOcrExtractor,
): KeyframeOcrRunner {
  let cachedSignature: string | undefined;
  let cachedResult: ReturnType<KeyframeOcrRunner> | undefined;

  return async (frames) => {
    const signature = frameSignature(frames);
    if (cachedSignature && cachedSignature !== signature) {
      throw new Error('自动 OCR 复验收到的关键帧集合与初次验证不一致');
    }
    cachedSignature ??= signature;
    cachedResult ??= (async () => {
      const extractor = injectedExtractor
        ?? (await import('./ocr.ts')).extractKeyframeOcr;
      const extraction = await extractor({
        outputDir,
        inputs: frames.map((frame) => ({
          aspect: frame.aspect,
          scene: frame.scene,
          frame: frame.phase,
          imagePath: frame.absolutePath,
        })),
      });
      const entryByKey = new Map(extraction.manifest.entries.map((entry) => [
        `${entry.aspect}/${entry.scene}/${entry.frame}`,
        entry,
      ]));
      const evidence = await Promise.all(frames.map(async (frame) => {
        const key = `${frame.aspect}/${frame.scene}/${frame.phase}`;
        const entry = entryByKey.get(key);
        if (!entry) throw new Error(`macOS Vision OCR 清单缺少关键帧：${key}`);
        return {
          relativePath: frame.relativePath,
          text: await readFile(path.join(extraction.outputDir, entry.textFile), 'utf8'),
        };
      }));
      return {
        available: true as const,
        engine: extraction.manifest.engine,
        evidence,
      };
    })();
    return cachedResult;
  };
}

async function runValidate(context: CliCommandContext): Promise<void> {
  const [consoleEntries, networkEntries] = await Promise.all([
    readDiagnosticEntries(context.paths.consoleLog),
    readDiagnosticEntries(context.paths.networkLog),
  ]);
  assertCompleteCaptureDiagnosticEntries(consoleEntries, networkEntries);

  const [{ createDemoFixtures }, validation] = await Promise.all([
    import('./fixtures.ts'),
    import('./validate.ts'),
  ]);
  const logFiles = [context.paths.consoleLog, context.paths.networkLog]
    .map((filePath) => relativeArtifactPath(context.paths, filePath));
  const markdownFiles = [relativeArtifactPath(context.paths, context.paths.exportMarkdown)];
  const sourceTextFiles = buildValidationSourceTextFiles(context.paths);
  const ocrTextFiles = buildValidationOcrTextFiles(context.paths);
  const runKeyframeOcr = createCachedKeyframeOcrRunner(
    path.join(context.paths.intermediate, 'ocr'),
  );
  const commonOptions = {
    artifactDir: context.paths.root,
    logFiles,
    markdownFiles,
    sourceTextFiles,
    ocrTextFiles,
    runKeyframeOcr,
    privacyObjects: [{ source: 'demo-fixtures', value: createDemoFixtures() }],
  };

  const initialValidation = await validation.validateDelivery(commonOptions);
  await validation.writeManifest({
    artifactDir: context.paths.root,
    version: await readPackageVersion(context.cwd),
    builtAt: new Date().toISOString(),
    gitCommit: await readGitCommit(context.cwd),
    validation: initialValidation,
  });
  await validation.validateDelivery({
    ...commonOptions,
    requireIntegrityFiles: true,
  });
}

export function createDefaultCliDependencies(): CliDependencies {
  return {
    check: runCheck,
    capture: runCapture,
    subtitles: runSubtitles,
    audio: runAudio,
    compose: runCompose,
    validate: runValidate,
  };
}

export async function runCli(
  argv: readonly string[],
  options: RunCliOptions = {},
): Promise<void> {
  const args = parseCliArgs(argv);
  const cwd = path.resolve(options.cwd ?? process.cwd());
  const paths = createDemoPaths(cwd);
  const dependencies = options.dependencies ?? createDefaultCliDependencies();

  if (args.command === 'build') {
    for (const command of BUILD_COMMAND_ORDER) {
      await dependencies[command]({ cwd, paths, args: { command } });
    }
    return;
  }

  await dependencies[args.command]({ cwd, paths, args });
}

function isDirectExecution(): boolean {
  const entry = process.argv[1];
  return Boolean(entry) && import.meta.url === pathToFileURL(path.resolve(entry)).href;
}

if (isDirectExecution()) {
  runCli(process.argv.slice(2)).catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
