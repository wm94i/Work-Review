import test from 'node:test';
import assert from 'node:assert/strict';

import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_PATH = fileURLToPath(new URL('./check-linux-glibc.sh', import.meta.url));
const BINARY_NAME = 'Work_Review';

type FailureMode = 'damaged-package' | 'extract-failure' | 'missing-main' | 'non-elf';

interface GateOptions {
  artifactExtension?: string;
  extraElfGlibcVersion?: string;
  failureMode?: FailureMode;
  glibcVersions?: string[];
  useRelativeArtifact?: boolean;
}

type GateResult = ReturnType<typeof spawnSync> & {
  readelfInvocations: string;
  remainingTempEntries: string[];
  summary: string;
};

function writeExecutable(path: string, source: string): void {
  writeFileSync(path, source, 'utf8');
  chmodSync(path, 0o755);
}

function runGate(args: string[], options: GateOptions = {}): GateResult {
  const {
    artifactExtension = 'deb',
    extraElfGlibcVersion = '',
    failureMode = '',
    glibcVersions = ['2.31'],
    useRelativeArtifact = false,
  } = options;
  const fixtureDir = mkdtempSync(join(tmpdir(), 'work-review-glibc-test-'));
  const fakeBinDir = join(fixtureDir, 'bin');
  const scriptTempDir = join(fixtureDir, 'script-tmp');
  const artifactPath = join(fixtureDir, `Work_Review_test_amd64.${artifactExtension}`);
  const readelfLogPath = join(fixtureDir, 'readelf.log');
  const summaryPath = join(fixtureDir, 'summary.md');
  mkdirSync(fakeBinDir);
  mkdirSync(scriptTempDir);

  if (artifactExtension === 'AppImage') {
    writeExecutable(
      artifactPath,
      `#!/usr/bin/env bash
set -euo pipefail
[[ "\${1:-}" == "--appimage-extract" ]] || exit 64
if [[ "\${FAKE_FAILURE_MODE:-}" == "extract-failure" ]]; then
  echo "AppImage extraction failed" >&2
  exit 65
fi
mkdir -p squashfs-root/usr/bin
if [[ "\${FAKE_FAILURE_MODE:-}" != "missing-main" ]]; then
  printf '\\177ELFfake\\n' > "squashfs-root/usr/bin/\${FAKE_BINARY_NAME:?}"
fi
if [[ -n "\${FAKE_EXTRA_ELF_GLIBC_VERSION:-}" ]]; then
  mkdir -p squashfs-root/usr/lib/work-review
  printf '\\177ELFextra\\n' > squashfs-root/usr/lib/work-review/libextra.so
fi
`,
    );
  } else {
    writeFileSync(artifactPath, `fake ${artifactExtension} fixture`, 'utf8');
  }

  writeExecutable(
    join(fakeBinDir, 'dpkg-deb'),
    `#!/usr/bin/env bash
set -euo pipefail
if [[ "\${FAKE_FAILURE_MODE:-}" == "damaged-package" ]]; then
  echo "corrupted DEB package" >&2
  exit 65
fi
mkdir -p "$3/usr/bin"
if [[ "\${FAKE_FAILURE_MODE:-}" != "missing-main" ]]; then
  printf '\\177ELFfake\\n' > "$3/usr/bin/\${FAKE_BINARY_NAME:?}"
fi
if [[ -n "\${FAKE_EXTRA_ELF_GLIBC_VERSION:-}" ]]; then
  mkdir -p "$3/usr/lib/work-review"
  printf '\\177ELFextra\\n' > "$3/usr/lib/work-review/libextra.so"
fi
`,
  );

  writeExecutable(
    join(fakeBinDir, 'readelf'),
    `#!/usr/bin/env bash
set -euo pipefail
binary="\${@: -1}"
printf '%s\n' "$binary" >> "\${FAKE_READELF_LOG:?}"
if [[ "$*" == *"--file-header"* ]]; then
  [[ "\${FAKE_FAILURE_MODE:-}" != "non-elf" ]] || exit 1
  printf 'ELF Header:\n'
  exit 0
fi
printf "Version needs section '.gnu.version_r' contains entries:\n"
versions="\${FAKE_GLIBC_VERSIONS:-}"
[[ "$binary" != *libextra.so ]] || versions="\${FAKE_EXTRA_ELF_GLIBC_VERSION:-}"
for version in $versions; do
  printf 'Name: GLIBC_%s\n' "$version"
done
`,
  );

  writeExecutable(
    join(fakeBinDir, 'rpm2cpio'),
    `#!/usr/bin/env bash
set -euo pipefail
printf 'fake rpm payload'
`,
  );
  writeExecutable(
    join(fakeBinDir, 'cpio'),
    `#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
mkdir -p usr/bin
if [[ "\${FAKE_FAILURE_MODE:-}" != "missing-main" ]]; then
  printf '\\177ELFfake\\n' > "usr/bin/\${FAKE_BINARY_NAME:?}"
fi
if [[ -n "\${FAKE_EXTRA_ELF_GLIBC_VERSION:-}" ]]; then
  mkdir -p usr/lib/work-review
  printf '\\177ELFextra\\n' > usr/lib/work-review/libextra.so
fi
`,
  );

  try {
    const artifactArgument = useRelativeArtifact
      ? artifactPath.slice(fixtureDir.length + 1)
      : artifactPath;
    const result = spawnSync(
      'bash',
      [SCRIPT_PATH, ...args.map((arg) => arg.replace('{artifact}', artifactArgument))],
      {
        encoding: 'utf8',
        cwd: useRelativeArtifact ? fixtureDir : undefined,
        env: {
          ...process.env,
          FAKE_BINARY_NAME: BINARY_NAME,
          FAKE_EXTRA_ELF_GLIBC_VERSION: extraElfGlibcVersion,
          FAKE_FAILURE_MODE: failureMode,
          FAKE_GLIBC_VERSIONS: glibcVersions.join(' '),
          FAKE_READELF_LOG: readelfLogPath,
          GITHUB_STEP_SUMMARY: summaryPath,
          PATH: `${fakeBinDir}${delimiter}${process.env.PATH ?? ''}`,
          TMPDIR: scriptTempDir,
        },
      },
    );
    return Object.assign(result, {
      readelfInvocations: existsSync(readelfLogPath) ? readFileSync(readelfLogPath, 'utf8') : '',
      remainingTempEntries: readdirSync(scriptTempDir),
      summary: existsSync(summaryPath) ? readFileSync(summaryPath, 'utf8') : '',
    });
  } finally {
    rmSync(fixtureDir, { recursive: true, force: true });
  }
}

function outputOf(result: GateResult): string {
  return `${result.stdout ?? ''}\n${result.stderr ?? ''}`;
}

test('GLIBC 门禁必须显式提供 --max', () => {
  const result = runGate(['--binary-name', BINARY_NAME, '{artifact}']);
  assert.notEqual(result.status, 0);
  assert.match(outputOf(result), /--max/);
});

test('GLIBC 门禁必须显式提供 --binary-name', () => {
  const result = runGate(['--max', '2.35', '{artifact}']);
  assert.notEqual(result.status, 0);
  assert.match(outputOf(result), /--binary-name/);
});

test('真实发布上限允许 GLIBC 2.35，并写入成功摘要', () => {
  const result = runGate(
    ['--max', '2.35', '--binary-name', BINARY_NAME, '{artifact}'],
    { glibcVersions: ['2.2.5', '2.17', '2.35'] },
  );
  assert.equal(result.status, 0, outputOf(result));
  assert.match(result.summary, /✅.*GLIBC_2\.35.*上限 GLIBC_2\.35/);
  assert.deepEqual(result.remainingTempEntries, []);
});

test('真实发布上限拒绝 GLIBC 2.36，并写入失败摘要', () => {
  const result = runGate(
    ['--max', '2.35', '--binary-name', BINARY_NAME, '{artifact}'],
    { glibcVersions: ['2.17', '2.36'] },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.summary, /❌.*GLIBC_2\.36.*上限 GLIBC_2\.35/);
  assert.deepEqual(result.remainingTempEntries, []);
});

test('GLIBC 版本按语义比较，2.9 不得被误判为高于 2.35', () => {
  const result = runGate(
    ['--max', '2.35', '--binary-name', BINARY_NAME, '{artifact}'],
    { glibcVersions: ['2.2.5', '2.9'] },
  );
  assert.equal(result.status, 0, outputOf(result));
});

test('门禁只检查主程序，不受包内额外 GLIBC 2.36 ELF 影响', () => {
  const result = runGate(
    ['--max', '2.35', '--binary-name', BINARY_NAME, '{artifact}'],
    { extraElfGlibcVersion: '2.36', glibcVersions: ['2.17', '2.35'] },
  );
  assert.equal(result.status, 0, outputOf(result));
  assert.match(result.readelfInvocations, /usr\/bin\/Work_Review/);
  assert.doesNotMatch(result.readelfInvocations, /libextra\.so/);
});

test('RPM 与 AppImage 支持相对产物路径', () => {
  for (const artifactExtension of ['rpm', 'AppImage']) {
    const result = runGate(
      ['--max', '2.35', '--binary-name', BINARY_NAME, '{artifact}'],
      { artifactExtension, glibcVersions: ['2.17', '2.35'], useRelativeArtifact: true },
    );
    assert.equal(result.status, 0, `${artifactExtension}: ${outputOf(result)}`);
  }
});

for (const failureCase of [
  ['损坏包', 'deb', 'damaged-package', /DEB 解包失败/],
  ['解包失败', 'AppImage', 'extract-failure', /AppImage 解包失败/],
  ['主程序缺失', 'deb', 'missing-main', /找不到主程序 usr\/bin\/Work_Review/],
  ['非 ELF', 'deb', 'non-elf', /主程序不是有效 ELF/],
] as const) {
  test(`${failureCase[0]}时写入明确失败摘要并清理临时目录`, () => {
    const result = runGate(
      ['--max', '2.35', '--binary-name', BINARY_NAME, '{artifact}'],
      { artifactExtension: failureCase[1], failureMode: failureCase[2] },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.summary, /❌/);
    assert.match(result.summary, failureCase[3]);
    assert.deepEqual(result.remainingTempEntries, []);
  });
}

test('主程序无 GLIBC 标签时写入明确失败摘要并清理临时目录', () => {
  const result = runGate(
    ['--max', '2.35', '--binary-name', BINARY_NAME, '{artifact}'],
    { glibcVersions: [] },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.summary, /❌.*未发现 GLIBC 版本要求/);
  assert.deepEqual(result.remainingTempEntries, []);
});
