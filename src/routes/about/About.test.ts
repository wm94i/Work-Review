import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('关于页应提供赞助支持按钮并展示微信与支付宝收款码', async () => {
  const source = await readFile(new URL('./About.svelte', import.meta.url), 'utf8');

  assert.match(source, /let isSponsorshipOpen = false;/);
  assert.match(source, /about\.sponsorship/);
  assert.match(source, /text-rose-500/);
  assert.match(source, /about\.wechat/);
  assert.match(source, /about\.alipay/);
  assert.match(source, /docs\/sponsorship\/vx\.png/);
  assert.match(source, /docs\/sponsorship\/zfb\.png/);
});

test('关于页赞助弹层应保留完整交互，检查更新应位于三个轻量操作之后', async () => {
  const source = await readFile(new URL('./About.svelte', import.meta.url), 'utf8');

  assert.match(source, /about\.supportCopy/);
  assert.match(source, /about\.supportCopy2/);
  assert.doesNotMatch(source, /推荐微信扫码支持/);
  assert.doesNotMatch(source, /也可以使用支付宝扫码/);
  assert.match(source, /if \(event\.key !== 'Escape'\) return;/);
  assert.match(source, /if \(isSponsorshipOpen\)/);
  assert.match(source, /isUpdateDetailsOpen = false/);
  assert.match(source, /on:click=\{\(\) => zoomedQr = wechatSponsorshipQr\}/);
  assert.match(source, /on:click=\{\(\) => zoomedQr = alipaySponsorshipQr\}/);
  assert.match(source, /role="switch"/);
  assert.match(source, /aria-checked=\{autoCheckUpdate\}/);
  assert.ok(
    source.indexOf("t('about.sponsorship')")
      < source.indexOf("t('about.checkUpdates')"),
    'GitHub、数据目录和赞助支持应先展示，检查更新应位于下方状态区'
  );
});

test('关于页应独立读取版本与更新设置，失败时不得伪造旧版本号', async () => {
  const source = await readFile(new URL('./About.svelte', import.meta.url), 'utf8');

  assert.match(source, /typeof version === 'string' && version\.trim\(\) \? version : '--'/);
  assert.match(source, /settings\?\.autoCheck \?\? true/);
  assert.doesNotMatch(source, /appVersion = '1\.0\.0'/);
  assert.ok(source.indexOf("console.error('获取版本失败:'") < source.indexOf("console.error('读取更新设置失败:'"));
});

test('关于页不应恢复 Linux 会话状态查询或 Wayland 详情卡片', async () => {
  const source = await readFile(new URL('./About.svelte', import.meta.url), 'utf8');

  assert.match(source, /open_data_dir/);
  assert.doesNotMatch(source, /invoke\('get_linux_session_support'\)/);
  assert.doesNotMatch(source, /about\.linuxSessionTitle/);
  assert.doesNotMatch(source, /activeWindowProvider/);
  assert.doesNotMatch(source, /browserUrlSupportLevel/);
});
