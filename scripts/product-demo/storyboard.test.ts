import assert from 'node:assert/strict';
import test from 'node:test';

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
