import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const read = (path: string) => readFile(new URL(path, import.meta.url), 'utf8');

const evidenceComponents = [
  './lib/components/evidence/EvidenceShell.svelte',
  './lib/components/evidence/EvidenceNavigation.svelte',
  './lib/components/evidence/EvidenceOverviewHeader.svelte',
  './lib/components/evidence/EvidenceTimelineHeader.svelte',
] as const;

function extractRule(source: string, selector: string): string {
  const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = source.match(new RegExp(`${escapedSelector}\\s*\\{([^}]*)\\}`));
  assert.ok(match, `缺少主题规则：${selector}`);
  return match[1];
}

test('证据星图默认使用浅色 Token，并由根 dark 类覆盖为现有深色 Token', async () => {
  const css = await read('./app.css');
  const lightTokens = extractRule(css, '[data-ui-template="evidence-star-map"]');
  const darkTokens = extractRule(css, 'html.dark[data-ui-template="evidence-star-map"]');

  assert.match(lightTokens, /color-scheme:\s*light;/);
  assert.match(lightTokens, /--evidence-canvas:\s*#[0-9a-f]{6};/i);
  assert.match(lightTokens, /--evidence-text:\s*#[0-9a-f]{6};/i);
  assert.doesNotMatch(lightTokens, /--evidence-canvas:\s*#080a0e;/i);

  assert.match(darkTokens, /color-scheme:\s*dark;/);
  assert.match(darkTokens, /--evidence-canvas:\s*#080a0e;/i);
  assert.match(darkTokens, /--evidence-surface:\s*#0d1117;/i);
  assert.match(darkTokens, /--evidence-text:\s*#eff2ec;/i);
  assert.match(darkTokens, /--evidence-acid:\s*#c8ff2f;/i);

  assert.doesNotMatch(css, /@media[^\{]*prefers-color-scheme[\s\S]*?evidence-star-map/i);
});

test('证据星图 Shell、导航和 Header 的颜色必须全部来自 evidence Token', async () => {
  const sources = await Promise.all(evidenceComponents.map(read));

  for (const [index, source] of sources.entries()) {
    const style = source.match(/<style>([\s\S]*?)<\/style>/)?.[1];
    assert.ok(style, `${evidenceComponents[index]} 缺少样式块`);
    assert.match(style, /var\(--evidence-[a-z0-9-]+\)/, `${evidenceComponents[index]} 未使用 evidence Token`);
    assert.doesNotMatch(
      style,
      /#[0-9a-f]{3,8}|rgba?\(|hsla?\(/i,
      `${evidenceComponents[index]} 仍包含固定颜色，浅色主题会失效`,
    );
  }
});

test('深色主题应通过细粒度 Token 保留现有导航与 Header 视觉', async () => {
  const [css, navigation, overviewHeader, timelineHeader] = await Promise.all([
    read('./app.css'),
    read('./lib/components/evidence/EvidenceNavigation.svelte'),
    read('./lib/components/evidence/EvidenceOverviewHeader.svelte'),
    read('./lib/components/evidence/EvidenceTimelineHeader.svelte'),
  ]);
  const darkTokens = extractRule(css, 'html.dark[data-ui-template="evidence-star-map"]');

  assert.match(darkTokens, /--evidence-brand-shadow:\s*0 0 28px rgba\(200, 255, 47, 0\.14\);/);
  assert.match(darkTokens, /--evidence-header-divider:\s*rgba\(255, 255, 255, 0\.07\);/);
  assert.match(darkTokens, /--evidence-switch-active-border:\s*rgba\(200, 255, 47, 0\.24\);/);
  assert.match(navigation, /box-shadow:\s*var\(--evidence-brand-shadow\);/);
  assert.match(overviewHeader, /border-bottom:\s*1px solid var\(--evidence-header-divider\);/);
  assert.match(timelineHeader, /border-color:\s*var\(--evidence-switch-active-border\);/);
});

test('证据星图主题实现不得引入 Emoji、手写 SVG 或本地 SVG', async () => {
  const sources = await Promise.all(evidenceComponents.map(read));
  const combined = sources.join('\n');

  assert.doesNotMatch(combined, /<svg\b|data:image|\.svg(?:['"?)]|$)/i);
  assert.doesNotMatch(combined, /[\u{1F000}-\u{1FAFF}]/u);
});
