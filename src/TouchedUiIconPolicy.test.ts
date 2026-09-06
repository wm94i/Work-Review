import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const read = (path: string) => readFile(new URL(path, import.meta.url), 'utf8');

function markupOf(source: string): string {
  const scriptEnd = source.indexOf('</script>');
  assert.notEqual(scriptEnd, -1, '组件应包含 script 区块');
  return source.slice(scriptEnd + '</script>'.length);
}

test('本阶段触及页面的界面标记不得使用 Emoji 充当图标', async () => {
  const [overview, timeline] = await Promise.all([
    read('./routes/Overview.svelte'),
    read('./routes/timeline/Timeline.svelte'),
  ]);

  for (const markup of [markupOf(overview), markupOf(timeline)]) {
    assert.doesNotMatch(markup, /[\u{1F000}-\u{1FAFF}]/u);
  }
});

test('时间线分类图标源码不得再定义或直接渲染 Emoji', async () => {
  const timeline = await read('./routes/timeline/Timeline.svelte');

  assert.doesNotMatch(timeline, /[\u{1F000}-\u{1FAFF}]/u);
  assert.doesNotMatch(timeline, />\s*\{info\.icon\}\s*</);
  assert.doesNotMatch(timeline, /CATEGORY_EMOJIS/);
  assert.match(timeline, /CategoryIcon/);
});

test('本阶段触及页面不得使用字符充当新增、编辑、删除或选中图标', async () => {
  const [overview, timeline] = await Promise.all([
    read('./routes/Overview.svelte'),
    read('./routes/timeline/Timeline.svelte'),
  ]);
  const markup = `${markupOf(overview)}\n${markupOf(timeline)}`;

  assert.doesNotMatch(markup, />\s*[✓✎×+]\s*</u);
});
