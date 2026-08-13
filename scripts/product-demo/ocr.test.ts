import assert from 'node:assert/strict';
import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import type { CommandResult, CommandRunner } from './ffmpeg.ts';
import {
  OCR_ASPECTS,
  OCR_FRAME_KINDS,
  buildKeyframeOcrInputs,
  extractKeyframeOcr,
  type KeyframeOcrInput,
} from './ocr.ts';
import { STORYBOARD } from './storyboard.ts';

const ok = (stdout = '', stderr = ''): CommandResult => ({
  stdout,
  stderr,
  exitCode: 0,
});

async function createExecutable(filePath: string): Promise<void> {
  await writeFile(filePath, '#!/bin/sh\nexit 0\n', 'utf8');
  await chmod(filePath, 0o755);
}

async function createFrameFiles(inputs: readonly KeyframeOcrInput[]): Promise<void> {
  await Promise.all(inputs.map(async (input) => {
    await mkdir(path.dirname(input.imagePath), { recursive: true });
    await writeFile(input.imagePath, 'safe png fixture', 'utf8');
  }));
}

function reverseInputs(inputs: readonly KeyframeOcrInput[]): KeyframeOcrInput[] {
  return [...inputs].reverse();
}

test('构建横竖屏七镜头 start/action/end 的确定性 42 帧 OCR 输入', () => {
  const intermediateDir = '/tmp/work-review-demo/intermediate';
  const inputs = buildKeyframeOcrInputs(intermediateDir);

  assert.equal(inputs.length, 42);
  assert.deepEqual(
    inputs.map(({ aspect, scene, frame }) => `${aspect}/${scene}/${frame}`),
    OCR_ASPECTS.flatMap((aspect) => STORYBOARD.flatMap((scene) => (
      OCR_FRAME_KINDS.map((frame) => `${aspect}/${scene.id}/${frame}`)
    ))),
  );
  assert.equal(
    inputs[0]?.imagePath,
    '/tmp/work-review-demo/intermediate/16x9/hook-start.png',
  );
  assert.equal(
    inputs.at(-1)?.imagePath,
    '/tmp/work-review-demo/intermediate/9x16/outro-end.png',
  );
});

test('通过 macOS Vision 批量提取真实图片 OCR，并生成确定性 UTF-8 文本与无绝对路径清单', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'work-review-ocr ;$(touch nope)-'));
  const intermediateDir = path.join(root, 'input frames');
  const outputDir = path.join(root, 'ocr output');
  const swiftCompilerPath = path.join(root, 'fake swiftc');
  const inputs = buildKeyframeOcrInputs(intermediateDir);
  await createExecutable(swiftCompilerPath);
  await createFrameFiles(inputs);

  const calls: Array<{ binary: string; args: readonly string[] }> = [];
  let helperSource = '';
  const runner: CommandRunner = async (binary, args) => {
    calls.push({ binary, args: [...args] });
    if (binary === swiftCompilerPath) {
      const sourcePath = args.find((arg) => arg.endsWith('.swift'));
      const outputIndex = args.indexOf('-o');
      const helperPath = args[outputIndex + 1];
      const moduleCacheIndex = args.indexOf('-module-cache-path');
      const moduleCachePath = args[moduleCacheIndex + 1];
      assert.ok(sourcePath, '编译命令必须传入 Swift 源文件路径');
      assert.ok(helperPath, '编译命令必须通过独立参数传入输出路径');
      assert.ok(moduleCacheIndex >= 0, '编译命令必须使用隔离的 Swift 模块缓存');
      assert.ok(moduleCachePath && path.isAbsolute(moduleCachePath));
      assert.equal(path.basename(moduleCachePath), 'module-cache');
      helperSource = await readFile(sourcePath, 'utf8');
      await createExecutable(helperPath);
      return ok();
    }

    assert.equal(args.length, inputs.length * 2);
    for (let index = 0; index < args.length; index += 2) {
      const imagePath = args[index];
      const rawOutputPath = args[index + 1];
      assert.ok(imagePath);
      assert.ok(rawOutputPath);
      await mkdir(path.dirname(rawOutputPath), { recursive: true });
      const order = index / 2;
      const text = order === 0
        ? '  今天完成 Aurora Notes 隐私设置\r\nAI 为可选增强  '
        : `安全演示帧 ${order + 1}`;
      await writeFile(rawOutputPath, text, 'utf8');
    }
    return ok();
  };

  try {
    const result = await extractKeyframeOcr({
      inputs: reverseInputs(inputs),
      outputDir,
      platform: 'darwin',
      swiftCompilerPath,
      runner,
    });

    assert.equal(calls.length, 2, 'Vision 帮助程序应只编译一次并批量处理全部图片');
    assert.equal(calls[0]?.binary, swiftCompilerPath);
    assert.match(helperSource, /import Vision/);
    assert.match(helperSource, /VNRecognizeTextRequest/);
    assert.match(helperSource, /zh-Hans/);
    assert.match(helperSource, /en-US/);

    const ocrCall = calls[1];
    assert.ok(ocrCall);
    assert.equal(ocrCall.args[0], inputs[0]?.imagePath);
    assert.equal(ocrCall.args[2], inputs[1]?.imagePath);
    assert.ok(ocrCall.args.includes(inputs[0]!.imagePath));
    assert.ok(!ocrCall.args.some((arg) => arg.includes(`"${inputs[0]!.imagePath}"`)));

    assert.equal(result.textFiles.length, 42);
    assert.equal(
      result.textFiles[0],
      path.join(outputDir, '16x9', 'hook-start.txt'),
    );
    assert.equal(
      await readFile(result.textFiles[0]!, 'utf8'),
      '今天完成 Aurora Notes 隐私设置\nAI 为可选增强\n',
    );

    const manifestText = await readFile(result.manifestPath, 'utf8');
    const manifest = JSON.parse(manifestText) as {
      schemaVersion: number;
      engine: string;
      recognitionLanguages: string[];
      entryCount: number;
      entries: Array<{
        aspect: string;
        scene: string;
        frame: string;
        sourceFile: string;
        textFile: string;
        characterCount: number;
      }>;
    };
    assert.equal(manifest.schemaVersion, 1);
    assert.equal(manifest.engine, 'macos-vision');
    assert.deepEqual(manifest.recognitionLanguages, ['zh-Hans', 'en-US']);
    assert.equal(manifest.entryCount, 42);
    assert.deepEqual(manifest.entries[0], {
      aspect: '16x9',
      scene: 'hook',
      frame: 'start',
      sourceFile: '16x9/hook-start.png',
      textFile: '16x9/hook-start.txt',
      characterCount: 31,
    });
    assert.deepEqual(
      manifest.entries.map((entry) => `${entry.aspect}/${entry.scene}/${entry.frame}`),
      inputs.map(({ aspect, scene, frame }) => `${aspect}/${scene}/${frame}`),
    );
    assert.ok(!manifestText.includes(root), 'OCR 清单不得泄漏真实绝对目录');
    assert.ok(manifestText.endsWith('\n'));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('拒绝缺帧、重复帧和非绝对路径，避免隐私扫描覆盖不完整', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'work-review-ocr-invalid-'));
  const compiler = path.join(root, 'swiftc');
  const inputs = buildKeyframeOcrInputs(path.join(root, 'frames'));
  await createExecutable(compiler);

  try {
    await assert.rejects(
      extractKeyframeOcr({
        inputs: inputs.slice(0, -1),
        outputDir: path.join(root, 'output'),
        platform: 'darwin',
        swiftCompilerPath: compiler,
        runner: async () => ok(),
      }),
      /必须完整包含 42 张关键帧.*缺少 9x16\/outro\/end/,
    );

    await assert.rejects(
      extractKeyframeOcr({
        inputs: [...inputs.slice(0, -1), inputs[0]!],
        outputDir: path.join(root, 'output'),
        platform: 'darwin',
        swiftCompilerPath: compiler,
        runner: async () => ok(),
      }),
      /重复的 OCR 关键帧.*16x9\/hook\/start/,
    );

    await assert.rejects(
      extractKeyframeOcr({
        inputs: inputs.map((input, index) => index === 0
          ? { ...input, imagePath: 'relative/hook-start.png' }
          : input),
        outputDir: path.join(root, 'output'),
        platform: 'darwin',
        swiftCompilerPath: compiler,
        runner: async () => ok(),
      }),
      /图片路径必须是绝对路径.*relative\/hook-start\.png/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('图片缺失、Swift 工具缺失和非 macOS 环境都明确失败', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'work-review-ocr-prerequisite-'));
  const inputs = buildKeyframeOcrInputs(path.join(root, 'frames'));
  const compiler = path.join(root, 'swiftc');
  await createExecutable(compiler);
  await createFrameFiles(inputs.slice(1));

  try {
    await assert.rejects(
      extractKeyframeOcr({
        inputs,
        outputDir: path.join(root, 'output'),
        platform: 'darwin',
        swiftCompilerPath: compiler,
        runner: async () => ok(),
      }),
      /OCR 图片不存在或不可读取.*hook-start\.png/,
    );

    await writeFile(inputs[0]!.imagePath, 'safe png fixture', 'utf8');
    await assert.rejects(
      extractKeyframeOcr({
        inputs,
        outputDir: path.join(root, 'output'),
        platform: 'darwin',
        swiftCompilerPath: path.join(root, 'missing-swiftc'),
        runner: async () => ok(),
      }),
      /macOS Vision OCR 编译工具不可用/,
    );

    await assert.rejects(
      extractKeyframeOcr({
        inputs,
        outputDir: path.join(root, 'output'),
        platform: 'linux',
        swiftCompilerPath: compiler,
        runner: async () => ok(),
      }),
      /仅支持 macOS Vision/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('编译失败、OCR 执行失败和任一输出缺失都明确失败', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'work-review-ocr-failure-'));
  const inputs = buildKeyframeOcrInputs(path.join(root, 'frames'));
  const compiler = path.join(root, 'swiftc');
  await createExecutable(compiler);
  await createFrameFiles(inputs);

  try {
    await assert.rejects(
      extractKeyframeOcr({
        inputs,
        outputDir: path.join(root, 'compile-output'),
        platform: 'darwin',
        swiftCompilerPath: compiler,
        runner: async () => ({ stdout: '', stderr: 'Vision 模块不可用', exitCode: 1 }),
      }),
      /编译 macOS Vision OCR 帮助程序失败.*Vision 模块不可用/,
    );

    let call = 0;
    await assert.rejects(
      extractKeyframeOcr({
        inputs,
        outputDir: path.join(root, 'run-output'),
        platform: 'darwin',
        swiftCompilerPath: compiler,
        runner: async (_binary, args) => {
          call += 1;
          if (call === 1) {
            const helperPath = args[args.indexOf('-o') + 1]!;
            await createExecutable(helperPath);
            return ok();
          }
          throw new Error('无法解码 PNG');
        },
      }),
      /执行 macOS Vision OCR 失败.*无法解码 PNG/,
    );

    call = 0;
    await assert.rejects(
      extractKeyframeOcr({
        inputs,
        outputDir: path.join(root, 'missing-output'),
        platform: 'darwin',
        swiftCompilerPath: compiler,
        runner: async (_binary, args) => {
          call += 1;
          if (call === 1) {
            const helperPath = args[args.indexOf('-o') + 1]!;
            await createExecutable(helperPath);
            return ok();
          }
          for (let index = 1; index < args.length - 2; index += 2) {
            await mkdir(path.dirname(args[index]!), { recursive: true });
            await writeFile(args[index]!, '可见文本', 'utf8');
          }
          return ok();
        },
      }),
      /OCR 输出缺失.*9x16\/outro\/end/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('成功返回前已实际写入全部清单文件', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'work-review-ocr-result-'));
  const inputs = buildKeyframeOcrInputs(path.join(root, 'frames'));
  const compiler = path.join(root, 'swiftc');
  await createExecutable(compiler);
  await createFrameFiles(inputs);
  let call = 0;

  const runner: CommandRunner = async (_binary, args) => {
    call += 1;
    if (call === 1) {
      const helperPath = args[args.indexOf('-o') + 1]!;
      await createExecutable(helperPath);
      return ok();
    }
    for (let index = 1; index < args.length; index += 2) {
      await mkdir(path.dirname(args[index]!), { recursive: true });
      await writeFile(args[index]!, '', 'utf8');
    }
    return ok();
  };

  try {
    const result = await extractKeyframeOcr({
      inputs,
      outputDir: path.join(root, 'ocr'),
      platform: 'darwin',
      swiftCompilerPath: compiler,
      runner,
    });
    await stat(result.manifestPath);
    await Promise.all(result.textFiles.map((textFile) => stat(textFile)));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
