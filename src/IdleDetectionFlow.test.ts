import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('脱敏记录与浮动窗口都应接入空闲剔除逻辑', async () => {
  const source = await readFile(new URL('../src-tauri/src/tasks/recording.rs', import.meta.url), 'utf8');

  assert.match(
    source,
    /PrivacyAction::Anonymize[\s\S]*anonymized_is_confirmed_idle[\s\S]*should_confirm_idle\(input_idle,\s*input_idle_seconds,\s*false,\s*false\)/
  );
  assert.match(
    source,
    /let overlay_is_confirmed_idle =[\s\S]*should_confirm_idle\(input_idle,\s*input_idle_seconds,\s*false,\s*false\);/
  );
  assert.match(source, /if overlay_is_confirmed_idle \{/);
});
