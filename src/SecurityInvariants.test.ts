import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'svelte/compiler';

const projectFile = (path: string) => new URL(`../${path}`, import.meta.url);

async function collectSvelteFiles(relativeDirectory: string): Promise<string[]> {
  const directory = fileURLToPath(projectFile(relativeDirectory));
  return (await readdir(directory, { recursive: true }))
    .filter((entry) => {
      const basename = entry.split(/[\\/]/).pop() ?? entry;
      return basename.endsWith('.svelte') && !basename.startsWith('._');
    })
    .map((entry) => join(relativeDirectory, entry))
    .sort();
}

interface AstNode {
  type?: string;
  expression?: AstNode;
  callee?: AstNode;
  object?: AstNode;
  property?: AstNode;
  name?: string;
  [key: string]: unknown;
}

function collectHtmlTags(root: unknown): AstNode[] {
  const found: AstNode[] = [];
  const visit = (node: unknown) => {
    if (!node || typeof node !== 'object') return;
    if (Array.isArray(node)) {
      for (const item of node) visit(item);
      return;
    }
    const candidate = node as AstNode;
    if (candidate.type === 'HtmlTag' || candidate.type === 'RawMustacheTag') {
      found.push(candidate);
    }
    for (const value of Object.values(candidate)) {
      if (value && typeof value === 'object') {
        visit(value);
      }
    }
  };
  visit(root);
  return found;
}

function resolveCallName(expr: AstNode | undefined): string | null {
  if (!expr || expr.type !== 'CallExpression' || !expr.callee) return null;
  const callee = expr.callee;
  if (callee.type === 'Identifier' && typeof callee.name === 'string') {
    return callee.name;
  }
  if (
    callee.type === 'MemberExpression'
    && callee.object?.type === 'Identifier'
    && callee.property?.type === 'Identifier'
    && typeof callee.object.name === 'string'
    && typeof callee.property.name === 'string'
  ) {
    return `${callee.object.name}.${callee.property.name}`;
  }
  return null;
}

test('安全不变量：tauri.conf.json CSP（含可选 devCsp）必须收紧脚本、表单与对象源且禁止 unsafe-eval', async () => {
  const conf = JSON.parse(await readFile(projectFile('src-tauri/tauri.conf.json'), 'utf8'));
  const csp: string = conf?.app?.security?.csp ?? '';

  assert.ok(csp.length > 0, 'CSP 不得为空');
  assert.match(csp, /\bdefault-src\s+'self'/);
  assert.match(csp, /\bscript-src\s+'self'(?:;|$)/);
  assert.match(csp, /\bform-action\s+'none'/);
  assert.match(csp, /\bbase-uri\s+'self'/);
  assert.match(csp, /\bobject-src\s+'none'/);
  assert.doesNotMatch(csp, /'unsafe-eval'/);

  const devCsp: unknown = conf?.app?.security?.devCsp;
  if (typeof devCsp === 'string' && devCsp.trim().length > 0) {
    assert.doesNotMatch(devCsp, /'unsafe-eval'/, 'devCsp 禁止 unsafe-eval');
    assert.doesNotMatch(
      devCsp,
      /\bscript-src[^;]*'unsafe-inline'/,
      'devCsp script-src 禁止 unsafe-inline',
    );
  }
});

test('安全不变量：全部第一方 Svelte 模板 AST 中的 {@html} 必须经由 DOMPurify 清洗', async () => {
  const svelteFiles = await collectSvelteFiles('src');
  const htmlUsageFiles: string[] = [];
  const allowedSanitizers = new Set([
    'DOMPurify.sanitize',
    'renderMarkdown',
    'renderStreamingMarkdown',
  ]);

  for (const file of svelteFiles) {
    const source = await readFile(projectFile(file), 'utf8');
    const ast = parse(source, { modern: true });
    const htmlTags = collectHtmlTags(ast.fragment);
    if (htmlTags.length === 0) continue;

    htmlUsageFiles.push(file);
    assert.match(source, /import\s+DOMPurify\s+from\s+'dompurify'/, `${file} 使用了 {@html} 但未导入 DOMPurify`);
    assert.match(source, /DOMPurify\.sanitize\(/, `${file} 使用了 {@html} 但未调用 DOMPurify.sanitize`);
    assert.match(source, /FORBID_TAGS:\s*\[[^\]]*'form'[^\]]*'area'[^\]]*\]/, `${file} DOMPurify 必须禁用危险交互标签`);
    assert.match(source, /FORBID_ATTR:\s*\[[^\]]*'xlink:href'[^\]]*\]/, `${file} DOMPurify 必须禁用 xlink:href`);

    for (const tag of htmlTags) {
      const callName = resolveCallName(tag.expression);
      assert.ok(
        callName && allowedSanitizers.has(callName),
        `${file} 存在未经白名单清洗函数的 {@html} 节点（检测到调用: ${callName ?? tag.expression?.type ?? 'unknown'}）`,
      );
    }
  }

  assert.ok(htmlUsageFiles.length > 0, '应至少覆盖 Ask/Report 中的 Markdown {@html} 渲染路径');
});

test('安全不变量：src-tauri/capabilities/ 下全部能力清单必须保持最小权限且禁止 fs/shell-exec/updater 通配', async () => {
  const capDir = fileURLToPath(projectFile('src-tauri/capabilities'));
  const capFiles = (await readdir(capDir, { recursive: true }))
    .filter((entry) => {
      const basename = entry.split(/[\\/]/).pop() ?? entry;
      return basename.endsWith('.json') && !basename.startsWith('._');
    })
    .map((entry) => entry.replace(/\\/g, '/'))
    .sort();
  assert.ok(capFiles.length > 0, 'src-tauri/capabilities/ 不应为空');

  const allowedWindows = new Set(['main', 'avatar']);

  for (const filename of capFiles) {
    const cap = JSON.parse(
      await readFile(projectFile(`src-tauri/capabilities/${filename}`), 'utf8'),
    );
    const permissions: string[] = Array.isArray(cap.permissions) ? cap.permissions : [];
    const windows: string[] = Array.isArray(cap.windows) ? cap.windows : [];

    assert.equal(cap.local, true, `${filename}: capabilities 必须限定 local: true`);
    assert.ok(windows.length > 0, `${filename}: windows 清单不应为空`);
    for (const win of windows) {
      assert.ok(allowedWindows.has(win), `${filename}: 发现未授权窗口范围 ${win}`);
    }
    assert.ok(permissions.length > 0, `${filename}: 权限清单不应为空`);

    for (const perm of permissions) {
      assert.doesNotMatch(perm, /^fs:/, `${filename}: 禁止暴露前端 fs 权限: ${perm}`);
      assert.doesNotMatch(perm, /^shell:allow-execute/, `${filename}: 禁止暴露前端命令执行权限: ${perm}`);
      assert.doesNotMatch(perm, /^updater:/, `${filename}: 禁止前端直接调用 updater 权限: ${perm}`);
      assert.doesNotMatch(perm, /:allow-all$/, `${filename}: 禁止通配权限: ${perm}`);
    }
  }
});

test('安全不变量：Vite envPrefix 仅允许声明一次且限定为 VITE_ 与 TAURI_ENV_，防止签名私钥泄漏', async () => {
  const viteConfig = await readFile(projectFile('vite.config.ts'), 'utf8');
  const envPrefixMatches = [...viteConfig.matchAll(/\benvPrefix\s*:/g)];
  assert.equal(envPrefixMatches.length, 1, 'vite.config.ts 应且仅应声明一次 envPrefix');
  assert.match(viteConfig, /envPrefix:\s*\[\s*'VITE_'\s*,\s*'TAURI_ENV_'\s*\]/);
  assert.doesNotMatch(viteConfig, /'TAURI_'/);
});
