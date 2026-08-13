import { constants as fsConstants } from 'node:fs';
import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { runCommand, type CommandResult, type CommandRunner } from './ffmpeg.ts';
import { STORYBOARD } from './storyboard.ts';
import type { DemoAspect, StoryboardSceneId } from './types.ts';

export const OCR_ASPECTS = ['16x9', '9x16'] as const satisfies readonly DemoAspect[];
export const OCR_FRAME_KINDS = ['start', 'action', 'end'] as const;
export const OCR_RECOGNITION_LANGUAGES = ['zh-Hans', 'en-US'] as const;
export const OCR_MANIFEST_FILENAME = 'manifest.json';

export type OcrFrameKind = typeof OCR_FRAME_KINDS[number];

export interface KeyframeOcrInput {
  aspect: DemoAspect;
  scene: StoryboardSceneId;
  frame: OcrFrameKind;
  imagePath: string;
}

export interface KeyframeOcrManifestEntry {
  aspect: DemoAspect;
  scene: StoryboardSceneId;
  frame: OcrFrameKind;
  sourceFile: string;
  textFile: string;
  characterCount: number;
}

export interface KeyframeOcrManifest {
  schemaVersion: 1;
  engine: 'macos-vision';
  recognitionLanguages: string[];
  entryCount: number;
  entries: KeyframeOcrManifestEntry[];
}

export interface ExtractKeyframeOcrOptions {
  inputs: readonly KeyframeOcrInput[];
  outputDir: string;
  runner?: CommandRunner;
  platform?: NodeJS.Platform;
  swiftCompilerPath?: string;
}

export interface KeyframeOcrResult {
  outputDir: string;
  manifestPath: string;
  textFiles: string[];
  manifest: KeyframeOcrManifest;
}

interface CanonicalOcrFrame {
  aspect: DemoAspect;
  scene: StoryboardSceneId;
  frame: OcrFrameKind;
  key: string;
  sourceFile: string;
  textFile: string;
}

const VISION_HELPER_SOURCE = String.raw`import AppKit
import Darwin
import Foundation
import Vision

struct RecognizedLine {
    let text: String
    let bounds: CGRect
    let originalIndex: Int
}

func writeStandardError(_ message: String) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
}

func loadCGImage(at path: String) throws -> CGImage {
    guard let image = NSImage(contentsOfFile: path) else {
        throw NSError(
            domain: "WorkReviewVisionOCR",
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: "无法解码图片：\(path)"]
        )
    }

    var proposedRect = CGRect(origin: .zero, size: image.size)
    guard let cgImage = image.cgImage(
        forProposedRect: &proposedRect,
        context: nil,
        hints: nil
    ) else {
        throw NSError(
            domain: "WorkReviewVisionOCR",
            code: 2,
            userInfo: [NSLocalizedDescriptionKey: "无法创建 CGImage：\(path)"]
        )
    }
    return cgImage
}

func recognizeText(in imagePath: String) throws -> String {
    let cgImage = try loadCGImage(at: imagePath)
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = true
    request.recognitionLanguages = ["zh-Hans", "en-US"]

    let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])
    try handler.perform([request])

    let lines = (request.results ?? []).enumerated().compactMap { item -> RecognizedLine? in
        guard let candidate = item.element.topCandidates(1).first else { return nil }
        return RecognizedLine(
            text: candidate.string,
            bounds: item.element.boundingBox,
            originalIndex: item.offset
        )
    }.sorted { lhs, rhs in
        let verticalDifference = abs(lhs.bounds.midY - rhs.bounds.midY)
        if verticalDifference > 0.01 {
            return lhs.bounds.midY > rhs.bounds.midY
        }
        let horizontalDifference = abs(lhs.bounds.minX - rhs.bounds.minX)
        if horizontalDifference > 0.01 {
            return lhs.bounds.minX < rhs.bounds.minX
        }
        return lhs.originalIndex < rhs.originalIndex
    }

    return lines.map(\.text).joined(separator: "\n")
}

let arguments = Array(CommandLine.arguments.dropFirst())
if arguments.isEmpty || arguments.count % 2 != 0 {
    writeStandardError("用法：vision-ocr <图片路径> <输出路径> [<图片路径> <输出路径> ...]")
    exit(64)
}

do {
    for index in stride(from: 0, to: arguments.count, by: 2) {
        let imagePath = arguments[index]
        let outputPath = arguments[index + 1]
        let recognizedText = try recognizeText(in: imagePath)
        try recognizedText.write(
            to: URL(fileURLWithPath: outputPath),
            atomically: true,
            encoding: .utf8
        )
    }
} catch {
    writeStandardError("Vision OCR 失败：\(error.localizedDescription)")
    exit(1)
}
`;

function buildCanonicalFrames(): CanonicalOcrFrame[] {
  return OCR_ASPECTS.flatMap((aspect) => STORYBOARD.flatMap((scene) => (
    OCR_FRAME_KINDS.map((frame) => ({
      aspect,
      scene: scene.id,
      frame,
      key: `${aspect}/${scene.id}/${frame}`,
      sourceFile: `${aspect}/${scene.id}-${frame}.png`,
      textFile: `${aspect}/${scene.id}-${frame}.txt`,
    }))
  )));
}

const CANONICAL_FRAMES = buildCanonicalFrames();
const CANONICAL_FRAME_BY_KEY = new Map(CANONICAL_FRAMES.map((frame) => [frame.key, frame]));

function frameKey(input: Pick<KeyframeOcrInput, 'aspect' | 'scene' | 'frame'>): string {
  return `${input.aspect}/${input.scene}/${input.frame}`;
}

function assertAbsolutePath(value: string, label: string): void {
  if (!value || !path.isAbsolute(value)) {
    throw new Error(`${label}必须是绝对路径：${value || '<空>'}`);
  }
}

function normalizeOcrText(value: string): string {
  return value
    .replace(/\r\n?/gu, '\n')
    .split('\n')
    .map((line) => line.trim())
    .join('\n')
    .trim();
}

function formatFailureDetails(result: CommandResult): string {
  const details = result.stderr.trim() || result.stdout.trim();
  return details || `退出码 ${result.exitCode}`;
}

async function assertReadable(filePath: string, message: string): Promise<void> {
  try {
    await access(filePath, fsConstants.R_OK);
  } catch {
    throw new Error(`${message}：${filePath}`);
  }
}

async function assertExecutable(filePath: string, message: string): Promise<void> {
  try {
    await access(filePath, fsConstants.X_OK);
  } catch {
    throw new Error(`${message}：${filePath}`);
  }
}

function validateAndSortInputs(inputs: readonly KeyframeOcrInput[]): KeyframeOcrInput[] {
  if (!Array.isArray(inputs)) {
    throw new Error('OCR 关键帧输入必须是数组');
  }

  const inputByKey = new Map<string, KeyframeOcrInput>();
  for (const input of inputs) {
    const key = frameKey(input);
    if (inputByKey.has(key)) {
      throw new Error(`存在重复的 OCR 关键帧：${key}`);
    }
    if (!CANONICAL_FRAME_BY_KEY.has(key)) {
      throw new Error(`存在未知的 OCR 关键帧：${key}`);
    }
    assertAbsolutePath(input.imagePath, 'OCR 图片路径');
    inputByKey.set(key, input);
  }

  const missing = CANONICAL_FRAMES
    .filter((frame) => !inputByKey.has(frame.key))
    .map((frame) => frame.key);
  if (inputs.length !== CANONICAL_FRAMES.length || missing.length > 0) {
    const suffix = missing.length > 0 ? `，缺少 ${missing.join('、')}` : '';
    throw new Error(`OCR 输入必须完整包含 42 张关键帧${suffix}`);
  }

  return CANONICAL_FRAMES.map((frame) => inputByKey.get(frame.key)!);
}

async function runRequiredCommand(
  description: string,
  runner: CommandRunner,
  binary: string,
  args: readonly string[],
): Promise<CommandResult> {
  let result: CommandResult;
  try {
    result = await runner(binary, args);
  } catch (error) {
    const details = error instanceof Error ? error.message : String(error);
    throw new Error(`${description}：${details}`);
  }
  if (result.exitCode !== 0) {
    throw new Error(`${description}：${formatFailureDetails(result)}`);
  }
  return result;
}

/**
 * 根据 capture.ts 的命名契约构造完整 42 帧输入。
 */
export function buildKeyframeOcrInputs(intermediateDir: string): KeyframeOcrInput[] {
  assertAbsolutePath(intermediateDir, '演示中间目录');
  return CANONICAL_FRAMES.map((frame) => ({
    aspect: frame.aspect,
    scene: frame.scene,
    frame: frame.frame,
    imagePath: path.join(intermediateDir, frame.sourceFile),
  }));
}

/**
 * 使用 macOS Vision 对全部演示关键帧执行真实 OCR。
 *
 * 所有路径均以独立 argv 传入子进程，绝不经过 shell 拼接。只有 42 张图片均成功
 * 生成 OCR 文本后才会写入最终清单，任何缺帧、工具或执行错误都会阻断交付。
 */
export async function extractKeyframeOcr(
  options: ExtractKeyframeOcrOptions,
): Promise<KeyframeOcrResult> {
  const platform = options.platform ?? process.platform;
  if (platform !== 'darwin') {
    throw new Error(`关键帧 OCR 仅支持 macOS Vision，当前平台：${platform}`);
  }

  assertAbsolutePath(options.outputDir, 'OCR 输出目录');
  const inputs = validateAndSortInputs(options.inputs);
  const swiftCompilerPath = options.swiftCompilerPath ?? '/usr/bin/swiftc';
  assertAbsolutePath(swiftCompilerPath, 'Swift 编译工具路径');

  await assertExecutable(swiftCompilerPath, 'macOS Vision OCR 编译工具不可用');
  for (const input of inputs) {
    await assertReadable(input.imagePath, 'OCR 图片不存在或不可读取');
  }

  const runner = options.runner ?? runCommand;
  const temporaryDir = await mkdtemp(path.join(tmpdir(), 'work-review-vision-ocr-'));
  const helperSourcePath = path.join(temporaryDir, 'VisionOcr.swift');
  const helperPath = path.join(temporaryDir, 'vision-ocr');
  const rawOutputDir = path.join(temporaryDir, 'raw');
  const moduleCachePath = path.join(temporaryDir, 'module-cache');

  try {
    await mkdir(rawOutputDir, { recursive: true });
    await writeFile(helperSourcePath, VISION_HELPER_SOURCE, 'utf8');

    await runRequiredCommand(
      '编译 macOS Vision OCR 帮助程序失败',
      runner,
      swiftCompilerPath,
      [
        helperSourcePath,
        '-O',
        '-module-cache-path',
        moduleCachePath,
        '-framework',
        'Vision',
        '-framework',
        'AppKit',
        '-o',
        helperPath,
      ],
    );
    await assertExecutable(helperPath, '编译 macOS Vision OCR 帮助程序失败，未生成可执行文件');

    const rawOutputPaths = inputs.map((_, index) => (
      path.join(rawOutputDir, `${String(index).padStart(2, '0')}.txt`)
    ));
    const ocrArgs = inputs.flatMap((input, index) => [
      input.imagePath,
      rawOutputPaths[index]!,
    ]);
    await runRequiredCommand(
      '执行 macOS Vision OCR 失败',
      runner,
      helperPath,
      ocrArgs,
    );

    const normalizedTexts: string[] = [];
    for (const [index, rawOutputPath] of rawOutputPaths.entries()) {
      let rawText: string;
      try {
        rawText = await readFile(rawOutputPath, 'utf8');
      } catch {
        const input = inputs[index]!;
        throw new Error(`OCR 输出缺失：${frameKey(input)}（${rawOutputPath}）`);
      }
      normalizedTexts.push(normalizeOcrText(rawText));
    }

    const textFiles: string[] = [];
    const entries: KeyframeOcrManifestEntry[] = [];
    for (const [index, frame] of CANONICAL_FRAMES.entries()) {
      const outputPath = path.join(options.outputDir, frame.textFile);
      const normalizedText = normalizedTexts[index]!;
      await mkdir(path.dirname(outputPath), { recursive: true });
      await writeFile(outputPath, normalizedText ? `${normalizedText}\n` : '', 'utf8');
      textFiles.push(outputPath);
      entries.push({
        aspect: frame.aspect,
        scene: frame.scene,
        frame: frame.frame,
        sourceFile: frame.sourceFile,
        textFile: frame.textFile,
        characterCount: [...normalizedText].length,
      });
    }

    const manifest: KeyframeOcrManifest = {
      schemaVersion: 1,
      engine: 'macos-vision',
      recognitionLanguages: [...OCR_RECOGNITION_LANGUAGES],
      entryCount: entries.length,
      entries,
    };
    const manifestPath = path.join(options.outputDir, OCR_MANIFEST_FILENAME);
    await mkdir(options.outputDir, { recursive: true });
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');

    return {
      outputDir: options.outputDir,
      manifestPath,
      textFiles,
      manifest,
    };
  } finally {
    await rm(temporaryDir, { recursive: true, force: true });
  }
}
