import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const readCss = () => readFile(new URL('./app.css', import.meta.url), 'utf8');

test('证据星图的全宽 page-shell 仅作用于概览与时间线', async () => {
  const css = await readCss();

  assert.doesNotMatch(
    css,
    /\.app-shell\.template-evidence-star-map\s+\.page-shell\s*\{[^}]*max-width:\s*none;[^}]*padding:\s*0;/,
    'Evidence 模板不能全局覆盖 Report、Ask、Settings 复用的经典 page-shell 约束',
  );
  assert.match(
    css,
    /\.app-shell\.template-evidence-star-map\s+\.page-shell:is\(\.evidence-overview-page,\s*\.evidence-timeline-page\)\s*\{[^}]*max-width:\s*none;[^}]*padding:\s*0;/,
    'Evidence 模板应只为 Overview 与 Timeline 清除 page-shell 宽度和内边距',
  );
});

const splitSelectorList = (selectorList: string) => {
  const selectors: string[] = [];
  let current = '';
  let parenthesisDepth = 0;

  for (const character of selectorList) {
    if (character === '(') parenthesisDepth += 1;
    if (character === ')') parenthesisDepth -= 1;

    if (character === ',' && parenthesisDepth === 0) {
      selectors.push(current.trim());
      current = '';
      continue;
    }

    current += character;
  }

  if (current.trim()) selectors.push(current.trim());
  return selectors;
};

const evidenceSelectors = (css: string) =>
  [...css.matchAll(/([^{}]+)\{/g)]
    .flatMap((match) => splitSelectorList(match[1]))
    .map((selector) => selector.replace(/\s+/g, ' ').trim())
    .filter((selector) => selector.startsWith('.app-shell.template-evidence-star-map'));

test('证据星图的主体通用类仅在概览与时间线内覆写', async () => {
  const css = await readCss();
  const commonClassPattern =
    /\.(?:page-toolbar|page-title-group|page-control-(?:btn(?:-icon)?|input)|overview-date-trigger|page-card(?:-soft)?|stats-card|settings-(?:muted|subtle)|text-slate-(?:500|600)|empty-state-lg|page-banner-error|empty-state-icon)(?![\w-])/;
  const overviewTimelineScopePattern =
    /\.page-shell:is\(\.evidence-overview-page,\s*\.evidence-timeline-page\)/;
  const leakingSelectors = evidenceSelectors(css).filter(
    (selector) => commonClassPattern.test(selector) && !overviewTimelineScopePattern.test(selector),
  );

  assert.deepEqual(
    leakingSelectors,
    [],
    `Evidence 主体通用类不能影响 Report、Ask、Settings、About：\n${leakingSelectors.join('\n')}`,
  );
});
