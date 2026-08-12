import assert from 'node:assert/strict';
import test from 'node:test';

import { createDemoFixtures } from './fixtures.ts';

test('演示夹具锁定日期、时区、活动和统计', () => {
  const fixtures = createDemoFixtures();
  assert.equal(fixtures.date, '2026-08-12');
  assert.equal(fixtures.timezone, 'Asia/Shanghai');
  assert.deepEqual(fixtures.activities.map(({ time, app, title }) => ({ time, app, title })), [
    { time: '09:12', app: 'Cursor', title: 'Aurora Board · 导出流程' },
    { time: '10:36', app: '浏览器', title: '设计规格复核 · docs.example.test' },
    { time: '11:18', app: '文档', title: 'Aurora Board 发布检查清单' },
    { time: '14:03', app: '会议', title: 'Aurora Board 周会' },
    { time: '15:24', app: 'Figma', title: '日报空态与竖屏构图' },
    { time: '16:40', app: 'Terminal', title: 'npm run verify:frontend' },
  ]);
  assert.equal(fixtures.stats.workTimeDuration, 19_800);
  const totals = {
    activities: fixtures.activities.reduce((total, item) => total + item.duration, 0),
    apps: fixtures.stats.apps.reduce((total, item) => total + item.duration, 0),
    categories: fixtures.stats.categories.reduce((total, item) => total + item.duration, 0),
    hourly: fixtures.stats.hourlyActivity.reduce((total, item) => total + item.duration, 0),
  };
  assert.deepEqual(totals, {
    activities: fixtures.stats.workTimeDuration,
    apps: fixtures.stats.workTimeDuration,
    categories: fixtures.stats.workTimeDuration,
    hourly: fixtures.stats.workTimeDuration,
  });
});

test('日报和助手回答符合批准规格', () => {
  const fixtures = createDemoFixtures();
  assert.ok(fixtures.report.sections.length >= 4);
  assert.match(fixtures.assistant.basicAnswer, /5 小时 30 分钟/);
  assert.match(fixtures.assistant.basicAnswer, /开发 3 小时 10 分钟（57\.6%）/);
  assert.match(fixtures.assistant.basicAnswer, /办公 1 小时 10 分钟（21\.2%）/);
  assert.match(fixtures.assistant.basicAnswer, /沟通 45 分钟（13\.6%）/);
  assert.match(fixtures.assistant.basicAnswer, /浏览 25 分钟（7\.6%）/);
  assert.match(fixtures.assistant.basicAnswer, /Cursor/);
  assert.match(fixtures.assistant.aiAnswer, /16:40/);
  assert.equal(fixtures.activities.find((activity) => activity.time === '16:40')?.title, 'npm run verify:frontend');
  assert.equal(fixtures.assistant.basicQuestion, '今天主要做了什么？');
  assert.equal(
    fixtures.assistant.aiQuestion,
    '结合刚才的今日记录，再查今天的活动，提炼一项有依据的日报成果。',
  );
});

test('夹具深拷贝且所有绝对路径限定在安全目录', () => {
  const first = createDemoFixtures();
  const second = createDemoFixtures();
  first.activities[0].title = '已修改';
  assert.notEqual(second.activities[0].title, '已修改');

  const serialized = JSON.stringify(second);
  const absolutePaths = serialized.match(/\/(?:[^"\\]|\\.)+/g) ?? [];
  for (const path of absolutePaths) {
    if (path.startsWith('/tmp/')) {
      assert.ok(path.startsWith('/tmp/work-review-demo/'), path);
    }
  }
  assert.doesNotMatch(serialized, /\/Users\//);
  assert.doesNotMatch(serialized, /github\.com|openai\.com/);
});
