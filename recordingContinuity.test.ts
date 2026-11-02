import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const source = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('采集循环必须先处理连续性边界，再查询窗口和计算时长', () => {
  const main = source('./src-tauri/src/main.rs');
  const loop = main.slice(main.indexOf('async fn background_screenshot_task('));
  const guard = loop.indexOf('let Some(poll_sample) = activity_clock.sample() else');
  const lookup = loop.indexOf('monitor::get_active_window()');
  assert.ok(guard >= 0 && lookup > guard);
  const reset = loop.slice(guard, lookup);
  for (const required of ['reset_recording_session(', 'idle_detector.reset()', 'cached_active_window = None']) {
    assert.ok(reset.includes(required), `采样边界必须执行 ${required}`);
  }
  const tail = reset.slice(0, reset.indexOf('reset_recording_session('));
  assert.ok(tail.includes('last_observed.timestamp()'), '尾段只结算到中断前的采样时刻');
  assert.ok(tail.includes('check_privacy_full(') && tail.includes('PrivacyAction::Skip'), '忽略应用不得在切段时被补写');
  assert.match(loop, /if !accounting_sample\.follows\(poll_sample\)/);
  assert.match(loop, /let current_timestamp = accounting_sample\.timestamp\(\)/);
  assert.doesNotMatch(loop, /last_capture_time\.elapsed\(\)/);
  assert.doesNotMatch(loop, /timestamp: screenshot_result\.timestamp/);
});

test('截图采集必须携带原采样上下文并丢弃跨待机的结果', () => {
  const main = source('./src-tauri/src/main.rs');
  const start = main.indexOf('async fn capture_screenshot_for_window(');
  const end = main.indexOf('async fn background_screenshot_task(', start);
  const capture = main.slice(start, end);
  assert.match(capture, /sample: activity_clock::Sample/);
  assert.match(capture, /if !activity_clock::Sample::now\(\)\.follows\(sample\)/);
  assert.match(capture, /std::fs::remove_file\(&result\.path\)/);
  assert.ok(capture.indexOf('截图跨越待机') < capture.indexOf('Ok(result)'));
});
