import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('工作时间跨零点时应拆成当天首尾两段后计算并集', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.match(
    source,
    /function totalWorkSegmentMinutes\(segments(?:\s*:\s*WorkTimeSegment\[\])?\)/,
  );
  assert.match(source, /ranges\.push\(\[start, 24 \* 60\], \[0, end\]\)/);
  assert.match(source, /ranges\.sort/);
  assert.match(source, /Math\.max\(current\[1\], range\[1\]\)/);
});

test('开始时间等于结束时间时应按零时长处理', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /if \(start === end\) continue/);
  assert.match(source, /const diffMinutes = totalWorkSegmentMinutes\(workSegments\)/);
  assert.match(source, /formatDurationLocalized\(0\)/);
});

test('关闭开机自启动失败时不应吞掉所有异常并伪称已经移除成功', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.doesNotMatch(source, /未知原因报错os error -2147024891/);
  assert.doesNotMatch(source, /确实已经正常移除了开机自启动/);
});

test('自启动后端应避免强制依赖 Windows 自定义状态并为未初始化场景返回明确错误', async () => {
  const source = await readFile(
    new URL('../../../src-tauri/src/autostart.rs', import.meta.url),
    'utf8'
  );

  assert.match(source, /#\[cfg\(windows\)\]\s*const AUTOSTART_LAUNCH_ARG/);
  assert.match(source, /开机自启动服务未初始化/);
  assert.match(source, /try_state::<AutoLaunchConfig>/);
  assert.match(source, /tauri_plugin_autostart::ManagerExt/);
  assert.doesNotMatch(source, /tauri::State<'_,\s*AutoLaunch(Manager|Config)>/);
});

test('切换开机自启后应立即持久化 config，避免用户忘点保存导致下次开机行为失同步', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  const toggleMatch = source.match(/async function toggleAutoStart\([^)]*\)\s*\{[\s\S]*?\n\s{2}\}/);
  assert.ok(toggleMatch, '未找到 toggleAutoStart 函数');
  assert.match(
    toggleMatch[0],
    /updateConfigQueued<GeneralConfig>\(\(latestConfig\)\s*=>\s*\{[\s\S]*?latestConfig\.auto_start\s*=\s*autoStartEnabled/,
  );
});

test('开启自启时应把 silent 选择传给后端，注册表参数才能反映用户的显/隐模式', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  const toggleMatch = source.match(/async function toggleAutoStart\([^)]*\)\s*\{[\s\S]*?\n\s{2}\}/);
  assert.ok(toggleMatch, '未找到 toggleAutoStart 函数');
  assert.match(
    toggleMatch[0],
    /invoke(?:<[^>]+>)?\('enable_autostart'\s*,\s*\{\s*silent:\s*[^}]*auto_start_silent[^}]*\}\)/
  );
});

test('切换"静默驻留"模式后应立即持久化 config，否则开机决策仍会用旧值', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  const updateMatch = source.match(
    /async function updateAutoStartLaunchMode\([^)]*\)\s*\{[\s\S]*?\n\s{2}\}/
  );
  assert.ok(updateMatch, '未找到 async 版本的 updateAutoStartLaunchMode 函数');
  assert.match(updateMatch[0], /config\.auto_start_silent\s*=/);
  assert.match(
    updateMatch[0],
    /updateConfigQueued<GeneralConfig>\(\(latestConfig\)\s*=>\s*\{[\s\S]*?latestConfig\.auto_start_silent\s*=\s*silentMode/,
  );
});

test('切换静默模式时若自启已启用应重注册，让注册表参数与新 silent 值对齐', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  const updateMatch = source.match(
    /async function updateAutoStartLaunchMode\([^)]*\)\s*\{[\s\S]*?\n\s{2}\}/
  );
  assert.ok(updateMatch, '未找到 async 版本的 updateAutoStartLaunchMode 函数');
  assert.match(updateMatch[0], /if\s*\(\s*autoStartEnabled\s*\)/);
  assert.match(
    updateMatch[0],
    /invoke(?:<[^>]+>)?\('enable_autostart'\s*,\s*\{\s*silent:\s*silentMode\s*\}\)/
  );
});

test('onMount 同步注册表与 config 不一致时也应落盘，避免下次启动再次失同步', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  const onMountMatch = source.match(/onMount\(async \(\)\s*=>\s*\{[\s\S]*?\n\s{2}\}\);/);
  assert.ok(onMountMatch, '未找到 onMount 块');
  assert.match(onMountMatch[0], /config\.auto_start\s*!==\s*autoStartEnabled/);
  assert.match(
    onMountMatch[0],
    /updateConfigQueued<GeneralConfig>\(\(latestConfig\)\s*=>\s*\{[\s\S]*?latestConfig\.auto_start\s*=\s*autoStartEnabled/,
  );
});

test('基本设置的局部持久化不应排队保存完整旧配置快照', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.match(
    source,
    /import\s*\{\s*updateConfigQueued\s*\}\s*from\s*['"]\$lib\/utils\/configSaveQueue\.ts['"]/
  );
  assert.doesNotMatch(source, /saveConfigQueued/);
});

test('基本设置页不应继续直接承载本地 API 管理入口，相关能力应迁入节点 Beta 标签', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.doesNotMatch(source, /settingsGeneral\.localApi/);
  assert.doesNotMatch(source, /config\.localhost_api_enabled/);
  assert.doesNotMatch(source, /invoke(?:<[^>]+>)?\('get_localhost_api_status'\)/);
});

test('基本设置页不应继续展示本地 API token 动作，避免与节点 Beta 标签重复', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.doesNotMatch(source, /invoke(?:<[^>]+>)?\('reveal_localhost_api_token'\)/);
  assert.doesNotMatch(source, /invoke(?:<[^>]+>)?\('rotate_localhost_api_token'\)/);
  assert.doesNotMatch(source, /settingsGeneral\.localApiTokenReveal/);
  assert.doesNotMatch(source, /settingsGeneral\.localApiTokenRotate/);
});

test('基本设置页不应继续内嵌设备节点与控制面配置块，相关能力应放进节点 Beta 标签', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.doesNotMatch(source, /settingsGeneral\.nodeGateway/);
  assert.doesNotMatch(source, /config\.node_gateway/);
  assert.doesNotMatch(source, /invoke(?:<[^>]+>)?\('get_node_gateway_status'\)/);
  assert.doesNotMatch(source, /control_plane_endpoint/);
});

test('基本设置的开关和数值输入应提供本地化可访问名称', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.equal((source.match(/role="switch"/g) || []).length, 4);
  assert.equal((source.match(/aria-checked=\{/g) || []).length, 4);
  for (const key of [
    'workTime',
    'standardWorkHours',
    'idleThreshold',
    'workGoalHours',
    'reportAutoGenerateTime',
    'autoStart',
    'hideDockIcon',
    'lightweightMode',
  ]) {
    assert.match(source, new RegExp(`aria-label=\\{t\\('settingsGeneral\\.${key}'\\)\\}`));
  }
  assert.match(source, /aria-label=\{`\$\{t\('settingsGeneral\.segmentLabel'/);
  assert.match(source, /\$\{t\('settingsGeneral\.from'\)\}`\}/);
  assert.match(source, /\$\{t\('settingsGeneral\.to'\)\}`\}/);
});

test('工作时间设置应支持分段配置并写回 work_time_segments', async () => {
  const source = await readFile(
    new URL('./components/SettingsGeneral.svelte', import.meta.url),
    'utf8'
  );

  assert.match(source, /config\.work_time_segments/);
  assert.match(source, /function addWorkSegment\(/);
  assert.match(source, /function removeWorkSegment\(/);
  assert.match(source, /function updateSegment\(/);
});
