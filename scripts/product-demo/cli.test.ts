import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  BUILD_COMMAND_ORDER,
  buildValidationSourceTextFiles,
  createCachedKeyframeOcrRunner,
  createDemoPaths,
  assertCompleteCaptureDiagnosticEntries,
  loadCompositionCaptureMetadata,
  mergeCaptureDiagnosticEntries,
  parseCliArgs,
  runCli,
  writeCaptureSupportFiles,
  type CliDependencies,
  type DemoCommand,
} from './cli.ts';
import type { ExtractKeyframeOcrOptions, KeyframeOcrResult } from './ocr.ts';
import { STORYBOARD } from './storyboard.ts';
import type { SceneCaptureResult } from './capture.ts';
import type { KeyframeOcrFrame } from './validate.ts';

function createDependencies(calls: string[] = []): CliDependencies {
  const record = (command: DemoCommand) => async () => {
    calls.push(command);
  };
  return {
    check: record('check'),
    capture: record('capture'),
    subtitles: record('subtitles'),
    audio: record('audio'),
    compose: record('compose'),
    validate: record('validate'),
  };
}

test('解析七个受支持的产品演示命令', () => {
  for (const command of [
    'check',
    'capture',
    'subtitles',
    'audio',
    'compose',
    'validate',
    'build',
  ] as const) {
    assert.deepEqual(parseCliArgs([command]), { command });
  }
});

test('capture 支持限定单个 timeline 分镜和 16x9 画幅', () => {
  assert.deepEqual(
    parseCliArgs(['capture', '--scene', 'timeline', '--aspect', '16x9']),
    { command: 'capture', scene: 'timeline', aspect: '16x9' },
  );
});

test('capture 接受全部合法分镜和两种画幅', () => {
  for (const scene of [
    'hook',
    'timeline',
    'report',
    'assistant',
    'privacy',
    'export',
    'outro',
  ]) {
    assert.equal(parseCliArgs(['capture', '--scene', scene]).scene, scene);
  }
  assert.equal(parseCliArgs(['capture', '--aspect', '9x16']).aspect, '9x16');
});

test('未知命令、缺失命令和无效参数会给出明确错误', () => {
  assert.throws(() => parseCliArgs([]), /缺少命令.*check.*capture/u);
  assert.throws(() => parseCliArgs(['publish']), /未知命令.*publish/u);
  assert.throws(
    () => parseCliArgs(['capture', '--scene', 'missing']),
    /无效分镜.*missing/u,
  );
  assert.throws(
    () => parseCliArgs(['capture', '--aspect', 'square']),
    /无效画幅.*square/u,
  );
  assert.throws(
    () => parseCliArgs(['capture', '--scene']),
    /--scene.*缺少值/u,
  );
  assert.throws(
    () => parseCliArgs(['compose', '--aspect', '16x9']),
    /compose.*不支持参数/u,
  );
  assert.throws(
    () => parseCliArgs(['capture', '--unknown', 'value']),
    /未知参数.*--unknown/u,
  );
});

test('恶意参数在调用依赖前被拒绝，不会进入任何录制或合成步骤', async () => {
  const calls: string[] = [];
  await assert.rejects(
    runCli(['capture', '--scene', 'timeline; touch /tmp/unsafe'], {
      cwd: '/workspace/work-review',
      dependencies: createDependencies(calls),
    }),
    /无效分镜/u,
  );
  assert.deepEqual(calls, []);
});

test('所有生成路径都集中在仓库 artifacts/product-demo 目录', () => {
  const cwd = path.resolve('/workspace/work-review');
  const paths = createDemoPaths(cwd);
  const expectedRoot = path.join(cwd, 'artifacts', 'product-demo');

  assert.equal(paths.root, expectedRoot);
  for (const artifactPath of Object.values(paths)) {
    assert.equal(
      artifactPath === expectedRoot || artifactPath.startsWith(`${expectedRoot}${path.sep}`),
      true,
      `${artifactPath} 必须位于 ${expectedRoot}`,
    );
  }
});

test('命令通过可注入依赖执行，单元测试不会真实录制或合成', async () => {
  const calls: string[] = [];
  const dependencies = createDependencies(calls);

  await runCli(['capture', '--scene', 'timeline', '--aspect', '16x9'], {
    cwd: '/workspace/work-review',
    dependencies,
  });

  assert.deepEqual(calls, ['capture']);
});

test('build 严格按 check → capture → subtitles → audio → compose → validate 执行', async () => {
  const calls: string[] = [];
  assert.deepEqual(BUILD_COMMAND_ORDER, [
    'check',
    'capture',
    'subtitles',
    'audio',
    'compose',
    'validate',
  ]);

  await runCli(['build'], {
    cwd: '/workspace/work-review',
    dependencies: createDependencies(calls),
  });

  assert.deepEqual(calls, BUILD_COMMAND_ORDER);
});

test('每个非 build 命令只调用对应依赖一次', async () => {
  for (const command of BUILD_COMMAND_ORDER) {
    const calls: string[] = [];
    await runCli([command], {
      cwd: '/workspace/work-review',
      dependencies: createDependencies(calls),
    });
    assert.deepEqual(calls, [command]);
  }
});

test('package scripts 暴露完整产品演示命令且单元测试有 60 秒超时', async () => {
  const packageJson = JSON.parse(await readFile('package.json', 'utf8')) as {
    scripts: Record<string, string>;
  };

  assert.equal(
    packageJson.scripts['demo:check'],
    'node --test-timeout=60000 --import tsx --test "scripts/product-demo/*.test.ts"',
  );
  for (const command of ['capture', 'subtitles', 'audio', 'compose', 'validate', 'build']) {
    assert.equal(
      packageJson.scripts[`demo:${command}`],
      `node --import tsx scripts/product-demo/cli.ts ${command}`,
    );
  }
});

test('gitignore 忽略产品演示大体积产物目录', async () => {
  const gitignore = await readFile('.gitignore', 'utf8');
  assert.match(gitignore, /^\/artifacts\/product-demo\/$/mu);
});


test('合成按故事板顺序读取七个录制 metadata 的内容起点和真实源时长', async () => {
  const intermediate = await mkdtemp(path.join(tmpdir(), 'work-review-cli-metadata-'));
  const aspectDir = path.join(intermediate, '16x9');
  await mkdir(aspectDir, { recursive: true });
  const sceneIds = ['hook', 'timeline', 'report'] as const;

  await Promise.all(sceneIds.map((scene, index) => writeFile(
    path.join(aspectDir, `${scene}.json`),
    JSON.stringify({
      scene,
      aspect: '16x9',
      contentStartOffsetSeconds: 0.4 + index * 0.1,
      durationSeconds: 9 + index,
    }),
    'utf8',
  )));

  const result = await loadCompositionCaptureMetadata({
    intermediateDir: intermediate,
    aspect: '16x9',
    sceneIds,
  });

  assert.deepEqual(result.sourceStartOffsetsSeconds, [0.4, 0.5, 0.6000000000000001]);
  assert.deepEqual(result.sourceDurationsSeconds, [9, 10, 11]);
  assert.deepEqual(result.metadataPaths, sceneIds.map((scene) => path.join(aspectDir, `${scene}.json`)));
});

test('合成拒绝缺失、错位或无效的录制 metadata', async (t) => {
  const cases = [
    {
      name: '缺失文件',
      value: undefined,
      expected: /缺少分镜 metadata.*hook/u,
    },
    {
      name: '分镜错位',
      value: { scene: 'timeline', aspect: '16x9', contentStartOffsetSeconds: 0.2, durationSeconds: 9 },
      expected: /分镜不匹配.*hook.*timeline/u,
    },
    {
      name: '画幅错位',
      value: { scene: 'hook', aspect: '9x16', contentStartOffsetSeconds: 0.2, durationSeconds: 9 },
      expected: /画幅不匹配.*16x9.*9x16/u,
    },
    {
      name: '负数起点',
      value: { scene: 'hook', aspect: '16x9', contentStartOffsetSeconds: -0.1, durationSeconds: 9 },
      expected: /内容起点.*非负有限数/u,
    },
    {
      name: '起点超出源时长',
      value: { scene: 'hook', aspect: '16x9', contentStartOffsetSeconds: 9, durationSeconds: 9 },
      expected: /内容起点.*小于源时长/u,
    },
    {
      name: '非正源时长',
      value: { scene: 'hook', aspect: '16x9', contentStartOffsetSeconds: 0, durationSeconds: 0 },
      expected: /源时长.*正有限数/u,
    },
  ] as const;

  for (const entry of cases) {
    await t.test(entry.name, async () => {
      const intermediate = await mkdtemp(path.join(tmpdir(), 'work-review-cli-invalid-metadata-'));
      const aspectDir = path.join(intermediate, '16x9');
      await mkdir(aspectDir, { recursive: true });
      if (entry.value) {
        await writeFile(path.join(aspectDir, 'hook.json'), JSON.stringify(entry.value), 'utf8');
      }

      await assert.rejects(
        loadCompositionCaptureMetadata({
          intermediateDir: intermediate,
          aspect: '16x9',
          sceneIds: ['hook'],
        }),
        entry.expected,
      );
    });
  }
});


test('局部重录按画幅和分镜合并诊断，不覆盖其他十三个镜头', () => {
  const existing: Array<{ scene: 'hook' | 'timeline'; aspect: '16x9' | '9x16'; pageErrors: readonly string[] }> = [
    { scene: 'hook', aspect: '16x9', pageErrors: ['old'] },
    { scene: 'timeline', aspect: '16x9', pageErrors: [] },
    { scene: 'hook', aspect: '9x16', pageErrors: [] },
  ] as const;
  const incoming: Array<{ scene: 'hook' | 'timeline'; aspect: '16x9' | '9x16'; pageErrors: readonly string[] }> = [
    { scene: 'hook', aspect: '16x9', pageErrors: [] },
  ] as const;

  assert.deepEqual(mergeCaptureDiagnosticEntries(existing, incoming), [
    { scene: 'hook', aspect: '16x9', pageErrors: [] },
    { scene: 'timeline', aspect: '16x9', pageErrors: [] },
    { scene: 'hook', aspect: '9x16', pageErrors: [] },
  ]);
});

test('完整录制诊断必须恰好覆盖横竖屏七个镜头且全部无错误', () => {
  const scenes = ['hook', 'timeline', 'report', 'assistant', 'privacy', 'export', 'outro'] as const;
  const consoleEntries = (['16x9', '9x16'] as const).flatMap((aspect) => scenes.map((scene) => ({
    scene,
    aspect,
    pageErrors: [],
    consoleErrors: [],
    unhandledCommands: [],
    invokeCommands: [],
  })));
  const networkEntries = consoleEntries.map(({ scene, aspect }) => ({
    scene,
    aspect,
    blockedRequests: [],
    policy: 'same-origin-data-blob-only',
  }));

  assert.doesNotThrow(() => assertCompleteCaptureDiagnosticEntries(consoleEntries, networkEntries));
  assert.throws(
    () => assertCompleteCaptureDiagnosticEntries(consoleEntries.slice(1), networkEntries),
    /控制台诊断.*缺少.*16x9.*hook/u,
  );
  assert.throws(
    () => assertCompleteCaptureDiagnosticEntries(
      consoleEntries.map((entry, index) => index === 0 ? { ...entry, pageErrors: ['boom'] } : entry),
      networkEntries,
    ),
    /hook.*pageErrors.*boom/u,
  );
  assert.throws(
    () => assertCompleteCaptureDiagnosticEntries(
      consoleEntries,
      networkEntries.map((entry, index) => index === 0 ? { ...entry, blockedRequests: ['https:\/\/example.com'] } : entry),
    ),
    /hook.*blockedRequests.*example.com/u,
  );
});

test('诊断合并拒绝重复或未知的画幅分镜组合', () => {
  assert.throws(
    () => mergeCaptureDiagnosticEntries([], [
      { scene: 'hook', aspect: '16x9' },
      { scene: 'hook', aspect: '16x9' },
    ]),
    /重复.*16x9.*hook/u,
  );
  assert.throws(
    () => mergeCaptureDiagnosticEntries([], [
      { scene: 'missing', aspect: '16x9' } as never,
    ]),
    /无效.*missing/u,
  );
});


test('交付隐私扫描包含三个字幕文件和横竖屏全部十四个录制 metadata', () => {
  const paths = createDemoPaths('/workspace/work-review');
  const sourceTextFiles = buildValidationSourceTextFiles(paths);

  assert.equal(sourceTextFiles.length, 17);
  assert.deepEqual(sourceTextFiles.slice(0, 3), [
    'work-review-demo-zh.srt',
    'work-review-demo-en.srt',
    'work-review-demo-zh-en.srt',
  ]);
  assert.deepEqual(
    sourceTextFiles.slice(3),
    (['16x9', '9x16'] as const).flatMap((aspect) => STORYBOARD.map((scene) => (
      `intermediate/${aspect}/${scene.id}.json`
    ))),
  );
});

test('真实 OCR 适配器映射验证关键帧、返回对应证据并在完整性复验时复用结果', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'work-review-cli-ocr-'));
  const outputDir = path.join(root, 'ocr');
  const frames: KeyframeOcrFrame[] = [
    {
      aspect: '16x9',
      scene: 'hook',
      phase: 'start',
      relativePath: 'intermediate/16x9/hook-start.png',
      absolutePath: path.join(root, 'hook-start.png'),
      imageBytes: new Uint8Array([1]),
    },
    {
      aspect: '9x16',
      scene: 'outro',
      phase: 'end',
      relativePath: 'intermediate/9x16/outro-end.png',
      absolutePath: path.join(root, 'outro-end.png'),
      imageBytes: new Uint8Array([2]),
    },
  ];
  let extractionCalls = 0;
  const extract = async (options: ExtractKeyframeOcrOptions): Promise<KeyframeOcrResult> => {
    extractionCalls += 1;
    assert.equal(options.outputDir, outputDir);
    assert.deepEqual(options.inputs, [
      { aspect: '16x9', scene: 'hook', frame: 'start', imagePath: frames[0]!.absolutePath },
      { aspect: '9x16', scene: 'outro', frame: 'end', imagePath: frames[1]!.absolutePath },
    ]);
    const textFiles = [path.join(outputDir, 'hook.txt'), path.join(outputDir, 'outro.txt')];
    await mkdir(outputDir, { recursive: true });
    await Promise.all([
      writeFile(textFiles[0]!, '今天完成安全演示\n', 'utf8'),
      writeFile(textFiles[1]!, '记录默认留在设备上\n', 'utf8'),
    ]);
    return {
      outputDir,
      manifestPath: path.join(outputDir, 'manifest.json'),
      textFiles,
      manifest: {
        schemaVersion: 1,
        engine: 'macos-vision',
        recognitionLanguages: ['zh-Hans', 'en-US'],
        entryCount: 2,
        entries: [
          {
            aspect: '16x9',
            scene: 'hook',
            frame: 'start',
            sourceFile: '16x9/hook-start.png',
            textFile: 'hook.txt',
            characterCount: 8,
          },
          {
            aspect: '9x16',
            scene: 'outro',
            frame: 'end',
            sourceFile: '9x16/outro-end.png',
            textFile: 'outro.txt',
            characterCount: 10,
          },
        ],
      },
    };
  };

  try {
    const runner = createCachedKeyframeOcrRunner(outputDir, extract);
    const first = await runner(frames);
    const second = await runner(frames.map((frame) => ({ ...frame })));

    assert.equal(extractionCalls, 1);
    assert.deepEqual(second, first);
    assert.deepEqual(first, {
      available: true,
      engine: 'macos-vision',
      evidence: [
        {
          relativePath: 'intermediate/16x9/hook-start.png',
          text: '今天完成安全演示\n',
        },
        {
          relativePath: 'intermediate/9x16/outro-end.png',
          text: '记录默认留在设备上\n',
        },
      ],
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});


function captureResult(
  scene: SceneCaptureResult['scene'],
  aspect: SceneCaptureResult['aspect'],
  exportedMarkdown?: SceneCaptureResult['exportedMarkdown'],
): SceneCaptureResult {
  const base = `/tmp/work-review-demo/${aspect}/${scene}`;
  return {
    scene,
    aspect,
    paths: {
      aspectDir: path.dirname(base),
      videoPath: `${base}.webm`,
      startFramePath: `${base}-start.png`,
      actionFramePath: `${base}-action.png`,
      endFramePath: `${base}-end.png`,
      metadataPath: `${base}.json`,
    },
    contentStartOffsetSeconds: 0.5,
    durationSeconds: 10,
    diagnostics: {
      pageErrors: [],
      consoleErrors: [],
      blockedRequests: [],
      unhandledCommands: [],
    },
    invokeCommands: [],
    ...(exportedMarkdown ? { exportedMarkdown } : {}),
  };
}

test('录制支持文件写入横竖屏一致的 UI 实际导出 Markdown', async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'work-review-cli-export-'));
  const paths = createDemoPaths(cwd);
  const content = '# 2026-08-12 日报\n\n来自 UI 导出';
  const context = { cwd, paths, args: { command: 'capture' as const } };

  try {
    await writeCaptureSupportFiles(context, [
      captureResult('export', '16x9', { fileName: '2026-08-12.md', content }),
      captureResult('export', '9x16', { fileName: '2026-08-12.md', content }),
    ]);
    assert.equal(await readFile(paths.exportMarkdown, 'utf8'), `${content}\n`);
  } finally {
    await rm(cwd, { recursive: true, force: true });
  }
});

test('完整录制缺少任一画幅的 UI 实际导出 Markdown 时阻断', async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'work-review-cli-export-missing-'));
  const paths = createDemoPaths(cwd);
  const context = { cwd, paths, args: { command: 'capture' as const } };

  try {
    await assert.rejects(
      writeCaptureSupportFiles(context, [
        captureResult('export', '16x9', {
          fileName: '2026-08-12.md',
          content: '# 日报',
        }),
        captureResult('export', '9x16'),
      ]),
      /完整录制.*9x16.*实际导出/u,
    );
  } finally {
    await rm(cwd, { recursive: true, force: true });
  }
});

test('局部重录非 export 镜头不会覆盖已有导出 Markdown', async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'work-review-cli-export-preserve-'));
  const paths = createDemoPaths(cwd);
  const context = {
    cwd,
    paths,
    args: { command: 'capture' as const, scene: 'hook' as const, aspect: '16x9' as const },
  };

  try {
    await mkdir(paths.exports, { recursive: true });
    await writeFile(paths.exportMarkdown, '保留现有 UI 导出\n', 'utf8');
    await writeCaptureSupportFiles(context, [captureResult('hook', '16x9')]);
    assert.equal(await readFile(paths.exportMarkdown, 'utf8'), '保留现有 UI 导出\n');
  } finally {
    await rm(cwd, { recursive: true, force: true });
  }
});
