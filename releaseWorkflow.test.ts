import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

test('Release workflow 应在构建前执行类型检查、前端测试并使用 npm ci', () => {
  const source = readFileSync(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8');

  assert.match(source, /run:\s*npm ci/);
  assert.match(source, /name:\s*Run frontend type check/);
  assert.match(source, /run:\s*npm run check/);
  assert.match(source, /name:\s*Run frontend tests/);
  assert.match(source, /run:\s*npm run test:frontend/);
  assert.match(source, /name:\s*Build frontend assets for Rust tests/);
  assert.match(source, /run:\s*npm run build/);
  assert.match(source, /name:\s*Run Rust tests/);
  assert.match(source, /run:\s*cargo check --workspace --all-targets/);
  assert.match(source, /run:\s*cargo clippy --workspace --all-targets -- -D warnings/);
  assert.match(source, /run:\s*cargo test --workspace --all-targets/);

  const typeCheckIndex = source.indexOf('name: Run frontend type check');
  const frontendIndex = source.indexOf('name: Run frontend tests');
  const frontendBuildIndex = source.indexOf('name: Build frontend assets for Rust tests');
  const rustIndex = source.indexOf('name: Run Rust tests');
  const buildIndex = source.indexOf('name: Build application');

  assert.notEqual(typeCheckIndex, -1);
  assert.notEqual(frontendIndex, -1);
  assert.notEqual(frontendBuildIndex, -1);
  assert.notEqual(rustIndex, -1);
  assert.notEqual(buildIndex, -1);
  assert.ok(typeCheckIndex < frontendIndex, '类型检查必须先于前端测试执行');
  assert.ok(frontendIndex < buildIndex, '前端测试必须先于构建执行');
  assert.ok(frontendBuildIndex < rustIndex, 'Rust 测试前必须先生成 frontendDist');
  assert.ok(rustIndex < buildIndex, 'Rust 测试必须先于构建执行');
});

test('Release workflow 应校验严格 SemVer、五处版本一致并只允许当前 main 发布', () => {
  const source = readFileSync(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8');
  const bashPattern = source.match(
    /if \[\[ ! "\$GITHUB_REF_NAME" =~ (\^\S+\$) \]\]; then/,
  )?.[1];

  assert.match(source, /tags:\s*\n\s*-\s*'v\[0-9\]\+\.\[0-9\]\+\.\[0-9\]\+'/);
  assert.match(source, /name:\s*Validate release tag and version/);
  assert.ok(bashPattern, '应能读取发布工作流中的 SemVer 校验表达式');
  const strictSemver = new RegExp(bashPattern);
  for (const valid of ['v0.0.0', 'v1.1.1', 'v12.34.56']) {
    assert.match(valid, strictSemver);
  }
  for (const invalid of ['v01.2.3', 'v1.02.3', 'v1.2.03', 'v1.2', '1.2.3']) {
    assert.doesNotMatch(invalid, strictSemver);
  }
  assert.match(source, /npm run test:frontend/);
  assert.match(source, /git fetch origin main/);
  assert.match(source, /git rev-parse origin\/main/);
  assert.match(source, /GITHUB_REF_NAME/);
});

test('官方 Actions 应使用 Node 24 运行时版本', () => {
  const workflowFiles = [
    './.github/workflows/ci.yml',
    './.github/workflows/release.yml',
    './.github/workflows/security-audit.yml',
  ];
  const combined = workflowFiles
    .map((path) => readFileSync(new URL(path, import.meta.url), 'utf8'))
    .join('\n');

  assert.doesNotMatch(combined, /actions\/(?:checkout|setup-node|upload-artifact|download-artifact)@v4/);
  assert.match(combined, /actions\/checkout@v7/);
  assert.match(combined, /actions\/setup-node@v7/);
  assert.match(combined, /actions\/upload-artifact@v7/);
  assert.match(combined, /actions\/download-artifact@v8/);
});

test('Security Audit 应允许 rustsec Action 更新检查与告警 Issue', () => {
  const source = readFileSync(
    new URL('./.github/workflows/security-audit.yml', import.meta.url),
    'utf8',
  );
  const cargoAuditJob = source.match(
    /\n  cargo-audit:\n([\s\S]*?)(?=\n  \S|$)/,
  )?.[1] ?? '';
  const npmAuditJob = source.match(
    /\n  npm-audit:\n([\s\S]*?)(?=\n  \S|$)/,
  )?.[1] ?? '';
  const workflowPermissions = source.match(
    /\npermissions:\n([\s\S]*?)(?=\n\S)/,
  )?.[1] ?? '';
  const permissions = cargoAuditJob.match(
    /\n    permissions:\n([\s\S]*?)(?=\n    \S)/,
  )?.[1] ?? '';

  assert.ok(cargoAuditJob, '应能读取 cargo-audit job');
  assert.ok(npmAuditJob, '应能读取 npm-audit job');
  assert.match(permissions, /^\s+contents: read$/m);
  assert.match(permissions, /^\s+checks: write$/m);
  assert.match(permissions, /^\s+issues: write$/m);
  assert.doesNotMatch(workflowPermissions, /^\s+(?:checks|issues): write$/m);
  assert.doesNotMatch(npmAuditJob, /^\s+(?:checks|issues): write$/m);
});

test('Release workflow 缺少便携包、构建产物或附件时必须失败', () => {
  const source = readFileSync(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8');
  const portableStep = source.match(
    /- name: Package Windows portable[\s\S]*?(?=\n\s+- name:)/,
  )?.[0] ?? '';

  assert.ok(portableStep, '应存在 Windows 便携包步骤');
  assert.doesNotMatch(portableStep, /跳过便携版|exit 0/);
  assert.match(source, /require_file "\*\/release\/Work_Review_portable_x64\.zip"/);
  assert.match(source, /all-artifacts\/\*\*\/Work_Review_portable_\*\.zip/);
  assert.doesNotMatch(source, /all-artifacts\/\*_portable_\*\.zip/);
  assert.match(source, /if-no-files-found:\s*error/);
  assert.match(source, /artifactErrorsFailBuild:\s*true/);
});

test('macOS release 应保持 ad-hoc 签名且不导入自签证书', () => {
  const source = readFileSync(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8');

  assert.match(source, /export APPLE_SIGNING_IDENTITY="-"/);
  assert.doesNotMatch(source, /MACOS_CODESIGN_P12|security import|add-trusted-cert/);
  assert.doesNotMatch(source, /Verify stable macOS code signature/);
});

test('Release workflow 两种 Linux 架构应使用 Ubuntu 22.04 并构建三种包', () => {
  const source = readFileSync(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8');
  const x64MatrixEntry = source.match(
    /- platform: ubuntu[^\n]*\n\s+args: "[^"]*x86_64-unknown-linux-gnu[^"]*"\n\s+target: x86_64-unknown-linux-gnu/,
  )?.[0] ?? '';
  const arm64MatrixEntry = source.match(
    /- platform: ubuntu[^\n]*\n\s+args: "[^"]*aarch64-unknown-linux-gnu[^"]*"\n\s+target: aarch64-unknown-linux-gnu/,
  )?.[0] ?? '';

  assert.ok(x64MatrixEntry, '应能读取 Linux x86_64 构建矩阵配置');
  assert.match(x64MatrixEntry, /platform:\s*ubuntu-22\.04/);
  assert.match(
    x64MatrixEntry,
    /args:\s*"--target x86_64-unknown-linux-gnu --bundles deb,rpm,appimage"/,
  );
  assert.ok(arm64MatrixEntry, '应能读取 Linux ARM64 构建矩阵配置');
  assert.match(arm64MatrixEntry, /platform:\s*ubuntu-22\.04-arm/);
  assert.match(
    arm64MatrixEntry,
    /args:\s*"--target aarch64-unknown-linux-gnu --bundles deb,rpm,appimage"/,
  );
  assert.doesNotMatch(source, /platform:\s*ubuntu-24\.04-arm/);
});

test('Release workflow ARM64 应校验 AppImage、DEB 和 RPM', () => {
  const source = readFileSync(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8');
  const arm64Verification = source.match(
    /aarch64-unknown-linux-gnu\)([\s\S]*?)\n\s*;;/,
  )?.[1] ?? '';

  assert.ok(arm64Verification, '应能读取 Linux ARM64 产物校验分支');
  assert.match(
    arm64Verification,
    /require_file "\*\/release\/bundle\/appimage\/\*\.AppImage" "Linux ARM64 AppImage"/,
  );
  assert.match(
    arm64Verification,
    /require_file "\*\/release\/bundle\/deb\/\*\.deb" "Linux ARM64 DEB"/,
  );
  assert.match(
    arm64Verification,
    /require_file "\*\/release\/bundle\/rpm\/\*\.rpm" "Linux ARM64 RPM"/,
  );
});

test('Release workflow 应在产物校验后上传前执行带版本上限的 GLIBC 检查', () => {
  const source = readFileSync(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8');
  const glibcStep = source.match(
    /- name: [^\n]*GLIBC[^\n]*[\s\S]*?(?=\n\s+- name:)/i,
  )?.[0] ?? '';
  const verifyIndex = source.indexOf('name: Verify required artifacts');
  const glibcIndex = glibcStep ? source.indexOf(glibcStep) : -1;
  const uploadIndex = source.indexOf('name: Upload release assets');

  assert.notEqual(verifyIndex, -1, '应存在产物校验步骤');
  assert.ok(glibcStep, '应存在 Linux GLIBC 兼容性检查步骤');
  assert.match(
    glibcStep,
    /if:\s*startsWith\(matrix\.platform,\s*'ubuntu'\)/,
    'GLIBC 检查只能在 Linux 构建矩阵中执行',
  );
  assert.match(
    glibcStep,
    /MAX_GLIBC_VERSION:\s*["']?2\.35["']?/,
    'GLIBC 检查必须将最大允许版本精确锁定为 2.35',
  );
  assert.match(
    glibcStep,
    /-path "\*\/release\/bundle\/appimage\/\*\.AppImage"/,
  );
  assert.match(
    glibcStep,
    /-path "\*\/release\/bundle\/deb\/\*\.deb"/,
  );
  assert.match(
    glibcStep,
    /-path "\*\/release\/bundle\/rpm\/\*\.rpm"/,
  );
  assert.match(
    glibcStep,
    /bash scripts\/check-linux-glibc\.sh\s+\\\s+--max "\$MAX_GLIBC_VERSION"\s+\\\s+--binary-name Work_Review\s+\\\s+"\$\{artifacts\[@\]\}"/,
    'GLIBC 检查必须以完整参数调用真实门禁脚本',
  );
  assert.notEqual(uploadIndex, -1, '应存在发布附件上传步骤');
  assert.ok(verifyIndex < glibcIndex, 'GLIBC 检查必须位于产物校验之后');
  assert.ok(glibcIndex < uploadIndex, 'GLIBC 检查必须位于发布附件上传之前');
});

test('Release workflow 应构建并上传 Linux RPM 产物', () => {
  const source = readFileSync(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8');

  assert.match(source, /args:\s*"--target x86_64-unknown-linux-gnu --bundles deb,rpm,appimage"[\s\S]*target:\s*x86_64-unknown-linux-gnu/);
  assert.match(source, /args:\s*"--target aarch64-unknown-linux-gnu --bundles deb,rpm,appimage"[\s\S]*target:\s*aarch64-unknown-linux-gnu/);
  assert.match(source, /sudo apt-get install -y[\s\S]*\brpm\b/);
  assert.match(source, /-name "\*\.rpm"/);
  assert.match(source, /release\/bundle\/rpm\/\*\.rpm/);
  assert.match(source, /require_file "\*\/release\/bundle\/rpm\/\*\.rpm" "Linux x64 RPM"/);
  assert.match(source, /target\/\*\*\/release\/bundle\/rpm\/\*\.rpm/);
});
