import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const projectFile = (path: string) => new URL(`../../../${path}`, import.meta.url);

test('外部链接只放行 http(s) 与 mailto', async () => {
  const { resolveExternalHref } = await import('./externalLinks.ts');
  assert.equal(resolveExternalHref('https://example.com/a?b=1'), 'https://example.com/a?b=1');
  assert.equal(resolveExternalHref('http://example.com'), 'http://example.com/');
  assert.equal(resolveExternalHref('mailto:someone@example.com'), 'mailto:someone@example.com');
  assert.equal(resolveExternalHref('javascript:alert(1)'), null);
  assert.equal(resolveExternalHref('file:///C:/Windows/System32/calc.exe'), null);
  assert.equal(resolveExternalHref('ms-settings:privacy'), null);
  assert.equal(resolveExternalHref('/relative/path'), null);
  assert.equal(resolveExternalHref('#anchor'), null);
  assert.equal(resolveExternalHref(''), null);
  assert.equal(resolveExternalHref(null), null);
});

test('助手 Markdown 回复区必须拦截链接导航', async () => {
  const source = await readFile(projectFile('src/routes/ask/Ask.svelte'), 'utf8');
  assert.match(source, /import \{ interceptExternalLinks \} from '\$lib\/actions\/externalLinks\.ts';/);
  assert.match(source, /use:interceptExternalLinks/);
});

test('日报正文链接走同一套协议白名单拦截', async () => {
  const source = await readFile(projectFile('src/routes/report/Report.svelte'), 'utf8');
  assert.match(source, /import \{ interceptExternalLinks \} from '\$lib\/actions\/externalLinks\.ts';/);
  assert.match(source, /use:interceptExternalLinks/);
  assert.doesNotMatch(source, /from '@tauri-apps\/plugin-shell'/, '日报页不应再直接调用 shell open');
});

test('外部链接拦截覆盖中键、空链接、SPA 路由、area 标签与表单提交', async () => {
  const source = await readFile(projectFile('src/lib/actions/externalLinks.ts'), 'utf8');
  assert.match(source, /a\[href\],\s*area\[href\]/);
  assert.match(source, /rawHref\.startsWith\('#\/'\)/);
  assert.match(source, /addEventListener\('auxclick'/);
  assert.match(source, /addEventListener\('submit'/);
});

test('Markdown DOMPurify 配置禁止表单与可跳转交互标签', async () => {
  for (const file of ['src/routes/ask/Ask.svelte', 'src/routes/report/Report.svelte']) {
    const source = await readFile(projectFile(file), 'utf8');
    assert.match(source, /FORBID_TAGS:\s*\['form'/);
    assert.match(source, /FORBID_ATTR:\s*\['xlink:href'\]/);
  }
});
