import assert from 'node:assert/strict';
import test from 'node:test';

import { DEMO_RECORDING_CONFIG } from './browser.ts';
import {
  DEMO_DURATION_SECONDS,
  DEMO_FPS,
  DEMO_TOTAL_FRAMES,
  STORYBOARD,
  VOICEOVER_CUES,
  validateStoryboard,
} from './storyboard.ts';

test('分镜固定为 85 秒、30fps 和 2550 帧', () => {
  assert.equal(DEMO_DURATION_SECONDS, 85);
  assert.equal(DEMO_FPS, 30);
  assert.equal(DEMO_TOTAL_FRAMES, 2550);
  assert.equal(STORYBOARD.length, 7);
  assert.deepEqual(STORYBOARD.map(({ start, end }) => [start, end]), [
    [0, 7],
    [7, 19],
    [19, 32],
    [32, 51],
    [51, 68],
    [68, 78],
    [78, 85],
  ]);
  assert.doesNotThrow(validateStoryboard);
});

test('每个分镜都有横竖屏独立构图', () => {
  for (const scene of STORYBOARD) {
    assert.ok(scene.composition['16x9']);
    assert.ok(scene.composition['9x16']);
    assert.notDeepEqual(scene.composition['16x9'], scene.composition['9x16']);
  }
});

test('助手只包含基础模板和演示 AI 两个阶段', () => {
  const assistant = STORYBOARD.find((scene) => scene.id === 'assistant');
  assert.ok(assistant);
  assert.deepEqual(assistant.assistantStages, ['basic-template', 'demo-ai']);
  assert.equal(VOICEOVER_CUES.length, 7);
});

test('导出旁白保留准确数据边界并控制在 Tingting 可用时长内', () => {
  const cue = VOICEOVER_CUES.find((item) => item.id === 'export');
  assert.ok(cue);
  assert.match(cue.zh, /活动记录.*截图.*默认.*本机/u);
  assert.match(cue.zh, /Markdown/u);
  assert.match(cue.zh, /外部 AI.*远程存储.*按配置使用/u);
  assert.ok([...cue.zh].length <= 44, `导出旁白过长：${[...cue.zh].length} 字`);
});


test('浏览器录制配置固定横竖屏视口、语言、时区和动效偏好', () => {
  assert.deepEqual(DEMO_RECORDING_CONFIG['16x9'], {
    viewport: { width: 1920, height: 1080 },
    videoSize: { width: 1920, height: 1080 },
    deviceScaleFactor: 1,
    locale: 'zh-CN',
    timezoneId: 'Asia/Shanghai',
    reducedMotion: 'reduce',
  });
  assert.deepEqual(DEMO_RECORDING_CONFIG['9x16'], {
    viewport: { width: 1080, height: 1920 },
    videoSize: { width: 1080, height: 1920 },
    deviceScaleFactor: 1,
    locale: 'zh-CN',
    timezoneId: 'Asia/Shanghai',
    reducedMotion: 'reduce',
  });
  for (const scene of STORYBOARD) {
    assert.ok(scene.composition['9x16'], `${scene.id} 缺少竖屏独立构图`);
  }
});
