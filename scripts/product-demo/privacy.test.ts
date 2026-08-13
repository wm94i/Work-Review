import assert from 'node:assert/strict';
import test from 'node:test';

import { createDemoFixtures } from './fixtures.ts';
import {
  assertNoSensitiveContent,
  scanDemoObject,
  scanSensitiveText,
} from './privacy.ts';
import { STORYBOARD } from './storyboard.ts';

function rulesOf(text: string): string[] {
  return scanSensitiveText(text, 'test-input').map((finding) => finding.rule);
}

test('识别 macOS、Windows 主目录和邮箱', () => {
  const findings = scanSensitiveText(
    [
      '截图位于 /Users/alice/Library/Application Support/Work Review/capture.png',
      String.raw`日志位于 C:\Users\alice\AppData\Local\Work Review\debug.log`,
      '联系 alice@example.com 获取演示数据',
    ].join('\n'),
    'paths-and-email',
  );

  assert.deepEqual(
    new Set(findings.map((finding) => finding.rule)),
    new Set(['macos-home', 'windows-home', 'email']),
  );
  assert.ok(findings.every((finding) => finding.source === 'paths-and-email'));
});

test('识别 Bearer、API key 和 Cookie，并掩码输出密钥内容', () => {
  const bearer = 'bearer-secret-1234567890';
  const apiKey = 'sk-demo-abcdefghijklmnop';
  const cookie = 'session=private-cookie-value; csrf=private-csrf-value';
  const findings = scanSensitiveText(
    [`Authorization: Bearer ${bearer}`, `OPENAI_API_KEY=${apiKey}`, `Cookie: ${cookie}`].join('\n'),
    'secrets',
  );

  assert.deepEqual(
    new Set(findings.map((finding) => finding.rule)),
    new Set(['bearer-token', 'api-key', 'cookie']),
  );

  const report = JSON.stringify(findings);
  assert.doesNotMatch(report, new RegExp(bearer));
  assert.doesNotMatch(report, new RegExp(apiKey));
  assert.doesNotMatch(report, /private-cookie-value|private-csrf-value/);
  assert.match(report, /\*{3,}|…/);
});

test('识别 github.com、openai.com 和其他非本地 HTTP(S) URL', () => {
  const findings = scanSensitiveText(
    [
      '源码镜像 github.com/example/private-repo',
      '模型端点 https://api.openai.com/v1/responses',
      '上传地址 https://uploads.example.org/demo?source=work-review',
    ].join('\n'),
    'external-links',
  );

  assert.ok(findings.some((finding) => finding.rule === 'forbidden-domain' && finding.match.includes('github.com')));
  assert.ok(findings.some((finding) => finding.rule === 'forbidden-domain' && finding.match.includes('openai.com')));
  assert.ok(findings.some((finding) => finding.rule === 'external-url' && finding.match.includes('uploads.example.org')));
});

test('放行安全目录、演示域名、本地地址以及 data/blob URL', () => {
  const safeText = [
    '/tmp/work-review-demo/screenshots/timeline.png',
    'https://docs.example.test/aurora-board/spec',
    'http://127.0.0.1:5173/timeline',
    'http://localhost:5173/report',
    'data:image/png;base64,AAAA',
    'blob:http://127.0.0.1:5173/7f6f7458-00f3-4ee8-a548-e3652e90beef',
  ].join('\n');

  assert.deepEqual(scanSensitiveText(safeText, 'allowed-content'), []);
});

test('识别禁止出现的虚构能力文案', () => {
  const rules = rulesOf(
    [
      '支持多选片段生成日报。',
      '提供独立 OCR 开关。',
      '可以关闭所有 AI。',
      '所有数据永远不会离开本机。',
    ].join('\n'),
  );

  assert.equal(rules.filter((rule) => rule === 'forbidden-capability').length, 4);
});

test('递归扫描对象，并允许已批准的分镜与演示夹具', () => {
  const nested = scanDemoObject(
    { export: { metadata: { owner: 'alice@example.com' } } },
    'nested-demo',
  );
  assert.equal(nested.length, 1);
  assert.equal(nested[0]?.rule, 'email');
  assert.equal(nested[0]?.source, 'nested-demo.export.metadata.owner');

  const approvedFindings = scanDemoObject(
    { storyboard: STORYBOARD, fixtures: createDemoFixtures() },
    'demo-source',
  );
  assert.doesNotThrow(() => assertNoSensitiveContent(approvedFindings));
});

test('门禁报错只包含已掩码的密钥命中', () => {
  const rawToken = 'token-that-must-never-appear-in-errors';
  const findings = scanSensitiveText(`Bearer ${rawToken}`, 'assistant-log');

  assert.throws(
    () => assertNoSensitiveContent(findings),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.match(error.message, /assistant-log/);
      assert.match(error.message, /bearer-token/);
      assert.doesNotMatch(error.message, new RegExp(rawToken));
      return true;
    },
  );
});

test('外部 URL 中的密钥参数不会通过 URL 命中泄露', () => {
  const rawKey = 'url-secret-abcdefghijklmnop';
  const findings = scanSensitiveText(
    `请求 https://uploads.example.org/demo?api_key=${rawKey}&mode=preview`,
    'network-log',
  );

  const report = JSON.stringify(findings);
  assert.ok(findings.some((finding) => finding.rule === 'api-key'));
  assert.ok(findings.some((finding) => finding.rule === 'external-url'));
  assert.doesNotMatch(report, new RegExp(rawKey));
  assert.match(report, /api_key=\*{3,}/);
});
