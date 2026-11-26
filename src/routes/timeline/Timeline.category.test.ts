import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { parse } from 'svelte/compiler';

test('所有非系统分类均应独立提供管理入口，不经过分类选择和历史同步', async () => {
  const source = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');
  const start = source.indexOf('{#each allCategories as cat (cat.key)}');
  const end = source.indexOf('{/each}', start) + '{/each}'.length;
  const html = parse(source.slice(start, end)).html;
  assert.ok(html.children);
  const each = html.children[0];
  assert.ok(each.children);
  const group = each.children.find((node: { type: string }) => node.type === 'Element');
  assert.ok(group?.children);
  assert.equal(group.name, 'span', '管理按钮不能嵌套在分类选择按钮内');
  assert.ok(!group.attributes.some((attr: { type: string }) => attr.type === 'EventHandler'));
  const branches = group.children.filter((node: { type: string }) => node.type === 'IfBlock');
  assert.equal(branches.length, 2, '当前分类展示与非系统分类管理必须为并列条件');
  assert.equal(branches[0].expression.operator, '===');
  assert.equal(branches[0].expression.right.name, 'currentCategoryKey');
  const management = branches[1];
  assert.equal(management.expression.operator, '!');
  assert.equal(management.expression.argument.object.name, 'cat');
  assert.equal(management.expression.argument.property.name, 'is_system');
  assert.ok(management.children);
  const actions = management.children.find((node: { type: string }) => node.type === 'Element');
  assert.ok(actions?.children);
  const buttons = actions.children.filter((node: { type: string }) => node.type === 'Element');
  assert.equal(buttons.length, 2);
  for (const button of buttons) {
    assert.equal(button.name, 'button');
    const disabled = button.attributes.find((attr: { name: string }) => attr.name === 'disabled');
    assert.equal(disabled.value[0].expression.name, 'categorySaving');
  }
  const markup = source.slice(start, end);
  const managementSource = markup.slice(management.start, management.end);
  assert.match(managementSource, /startRenameCategory\(cat\)/);
  assert.match(managementSource, /prepareCategoryConfirmation\(\)/);
  assert.match(managementSource, /pendingDeleteCategory = \{ key: cat.key, name: getCategoryDisplayName\(cat\) \}/);
  assert.doesNotMatch(managementSource, /selectActivityCategory|doChangeAppCategory|syncHistory|invoke\(/);
  assert.match(source, /function startRenameCategory\(cat: CategoryInfo\): void \{\s*clearPendingChip\(\)/);
  assert.match(source, /function prepareCategoryConfirmation\(\): void \{\s*clearPendingChip\(\)/);
});

test('时间线详情应支持修改应用默认分类并二次确认后回填历史', async () => {
  const source = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');

  assert.match(source, /invoke\('set_app_category_rule'/);
  assert.match(source, /timeline\.changeCategoryMessage/);
  assert.match(source, /timeline\.categoryUpdated/);
  // 二次点击确认：首次点击进入待确认态，3 秒内再次点击同一分类才应用
  assert.match(source, /let pendingChipCategory: string \| null = null/);
  assert.match(source, /function armPendingChip\(nextCategory: string\)/);
  assert.match(source, /pendingChipCategory === nextCategory/);
  assert.match(source, /timeline\.detail\.appCategoryPendingConfirm/);
  assert.doesNotMatch(source, /pendingChangeCategory/, '修改分类不再走全屏确认层');
  assert.match(source, /doChangeAppCategory/);
});

test('时间线详情分类选择器应按当前语言翻译内置分类', async () => {
  const source = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');

  assert.match(source, /translatedCategoryName = translateCategoryLabel\(cat\.key\)/);
  assert.match(source, /isKnownSystemCategory = cat\.is_system \|\| translatedCategoryName !== cat\.key/);
  assert.match(
    source,
    /return isKnownSystemCategory \? translatedCategoryName : \(cat\.name \|\| translatedCategoryName\)/
  );
  assert.doesNotMatch(
    source,
    /function getCategoryDisplayName\(cat\) \{[\s\S]*return cat\.name \|\| translateCategoryLabel\(cat\.key\);[\s\S]*\}/,
    '分类选择器不能直接优先显示 get_categories 返回的中文内置分类名'
  );
});

test('时间线详情分类应使用行内标签选择，不再弹出盖住截图的管理面板', async () => {
  const source = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');

  assert.match(source, /splitCategoriesForPicker\(\$categoryStore, selectedActivity.category\)/);
  assert.match(source, /timeline-category-chip-current/);
  assert.match(source, /timeline-category-chips/);
  assert.match(source, /timeline-category-chip/);
  assert.match(source, /#each allCategories as cat \(cat.key\)/);
  assert.doesNotMatch(source, /timeline-category-current-name/, '当前分类不应再使用独立大芯片');
  assert.doesNotMatch(source, /role="listbox"/);
  assert.doesNotMatch(source, /role="radiogroup"/);
  assert.match(source, /timeline-category-dot/);
  assert.match(source, /bind:this=\{categoryTrigger\}/);
  assert.match(source, /on:click=\{toggleCreateCategory\}/);
  assert.match(source, /function handleDetailDismiss\(\)/);
  assert.match(source, /on:click\|self=\{handleDetailDismiss\}/);
  assert.match(source, /handleDetailOverlayKeydown/);
  assert.match(source, /categoryTrigger\?\.focus\(\)/);
  assert.doesNotMatch(source, /showCategoryPopover/);
  assert.doesNotMatch(source, /timeline-category-popover/);
  assert.doesNotMatch(source, /timeline-category-trigger/);
  assert.doesNotMatch(source, /getViewportPopoverPlacement/);
  assert.equal(
    (source.match(/use:portalToBody/g) || []).length,
    3,
    '时间线应保留详情遮罩、分类确认层与批量清理面板三处 portal（分类选择本体为行内标签，不挂视口弹层；确认层/清理面板不 portal 会被 z-[140] 详情遮罩盖住）',
  );
});

test('分类确认层应接管键盘焦点，并在分类入口触发时恢复到分类按钮', async () => {
  const source = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');

  assert.match(source, /function prepareCategoryConfirmation\(\)/);
  assert.match(source, /showCreateCategory = false;[\s\S]*showRenameCategory = false;[\s\S]*categoryTrigger\?\.focus\(\)/);
  assert.ok(
    (source.match(/prepareCategoryConfirmation\(\)/g) || []).length >= 4,
    '分类选择、创建后应用、删除入口都应先把焦点交还分类触发按钮'
  );
  assert.match(
    source,
    /class="timeline-action-confirm-dialog"\s+use:trapFocus\s+role="dialog"\s+aria-modal="true"\s+aria-labelledby="timeline-action-confirm-title"\s+tabindex="-1"/
  );
  assert.equal(
    (source.match(/id="timeline-action-confirm-title"/g) || []).length,
    3,
    '删除、隐私和分类变更三个标题分支都应为确认对话框提供可访问名称'
  );
  assert.match(source, /function handleTimelineWindowKeydown\(event\)[\s\S]*cancelPendingAction\(\)/);
  assert.match(source, /<svelte:window[\s\S]*on:keydown=\{handleTimelineWindowKeydown\}/);
  const confirmDialogStart = source.indexOf('class="timeline-action-confirm-dialog"');
  const confirmDialogEnd = source.indexOf('>', confirmDialogStart);
  assert.ok(confirmDialogStart >= 0 && confirmDialogEnd > confirmDialogStart);
  assert.doesNotMatch(
    source.slice(confirmDialogStart, confirmDialogEnd),
    /on:keydown=/,
    '确认对话框的 Escape 应由 window 层处理，避免在非交互 div 上绑定键盘监听器'
  );
});

test('分类确认保存结束后应在按钮重新可用时恢复焦点，且不抢占用户主动移动的焦点', async () => {
  const source = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');

  assert.match(
    source,
    /async function restoreCategoryTriggerAfterSaving\(\)[\s\S]*await tick\(\)[\s\S]*document\.activeElement[\s\S]*categoryTrigger\?\.focus\(\)/
  );
  assert.ok(
    (source.match(/categorySaving = false;\s*await restoreCategoryTriggerAfterSaving\(\)/g) || []).length >= 2,
    '分类修改和分类删除保存完成后都应在触发按钮重新可用时恢复焦点'
  );
});

test('自定义分类的重命名与删除符号按钮应提供明确名称并隐藏装饰符号', async () => {
  const source = await readFile(new URL('./Timeline.svelte', import.meta.url), 'utf8');
  const actionsStart = source.indexOf('class="timeline-category-chip-actions"');
  const actionsEnd = source.indexOf('</div>', actionsStart);
  const actionsSource = source.slice(actionsStart, actionsEnd);

  assert.ok(actionsStart >= 0 && actionsEnd > actionsStart, '应能定位自定义分类操作区');
  assert.match(
    actionsSource,
    /aria-label=\{t\('timeline\.renameCategory'\)\}[\s\S]*?<span aria-hidden="true">✎<\/span>/
  );
  assert.match(
    actionsSource,
    /aria-label=\{t\('timeline\.deleteCategory'\)\}[\s\S]*?<span aria-hidden="true">×<\/span>/
  );
});
