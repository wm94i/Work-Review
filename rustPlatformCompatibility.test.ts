import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const readSource = (path: string) =>
  readFileSync(new URL(path, import.meta.url), 'utf8');

test('Linux 发布构建不应编译仅供 macOS 或测试使用的符号', () => {
  const avatar = readSource('./src-tauri/src/commands/avatar.rs');
  const system = readSource('./src-tauri/src/commands/system.rs');
  const main = readSource('./src-tauri/src/main.rs');
  const monitor = readSource('./src-tauri/src/monitor.rs');
  const screenshot = readSource('./src-tauri/src/screenshot.rs');

  assert.doesNotMatch(
    avatar,
    /current_linux_desktop_session, LinuxDesktopSession/,
    'avatar 命令不得导入未使用的 LinuxDesktopSession',
  );
  assert.match(
    system,
    /#\[cfg\(any\(target_os = "macos", target_os = "windows"\)\)\]\nuse std::path::Path;/,
  );
  assert.match(
    main,
    /#\[cfg\(any\(target_os = "macos", test\)\)\]\nfn should_request_screen_capture_permission/,
  );
  assert.match(
    monitor,
    /#\[cfg\(any\(target_os = "macos", test\)\)\]\nfn normalize_electron_app_name/,
  );
  for (const functionName of [
    'firefox_family_profile_dir_from_ini',
    'decode_mozlz4_bytes',
    'normalize_session_store_title',
    'extract_active_tab_url_from_session_store_value',
  ]) {
    assert.match(
      monitor,
      new RegExp(
        `#\\[cfg\\(any\\(target_os = "macos", target_os = "linux", test\\)\\)\\]\\nfn ${functionName}`,
      ),
    );
  }
  assert.doesNotMatch(monitor, /\binfer_browser_page_hint_from_text\b/);
  assert.match(
    screenshot,
    /#\[cfg\(any\(target_os = "macos", target_os = "windows", test\)\)\]\n    fn persist_dynamic_image_capture/,
  );
});

test('Windows 发布构建应保留明确类型并隔离 Unix 测试依赖', () => {
  const monitor = readSource('./src-tauri/src/monitor.rs');
  const ocr = readSource('./src-tauri/src/ocr.rs');
  const screenshot = readSource('./src-tauri/src/screenshot.rs');
  const system = readSource('./src-tauri/src/commands/system.rs');

  assert.match(
    monitor,
    /let queue: Arc<\(Mutex<LatestOnlySlot<BrowserUrlQueryJob>>, Condvar\)>\s*=\s*Arc::new/,
    'Windows 最新任务槽必须显式标注类型',
  );
  assert.match(
    monitor,
    /#\[cfg\(any\(target_os = "macos", target_os = "linux", test\)\)\]\nuse std::path::\{Path, PathBuf\};/,
    'Firefox 会话路径依赖不应进入 Windows 生产构建',
  );
  assert.match(
    monitor,
    /#\[cfg\(any\(target_os = "macos", target_os = "linux", test\)\)\]\nuse serde_json::Value;/,
    'Firefox 与 Linux 窗口解析依赖不应进入 Windows 生产构建',
  );
  assert.match(
    ocr,
    /#\[cfg\(unix\)\]\n    use super::\{[\s\S]*?PaddleModelConfig[\s\S]*?\};/,
    'Paddle worker 测试依赖只应在 Unix 测试中导入',
  );
  assert.match(
    screenshot,
    /#\[cfg\(any\(target_os = "macos", target_os = "linux"\)\)\]\nuse std::process::\{Command, Output, Stdio\};/,
  );
  assert.match(
    system,
    /#\[cfg\(target_os = "macos"\)\]\nuse std::path::PathBuf;/,
  );
  assert.match(
    system,
    /#\[cfg\(any\(target_os = "macos", test\)\)\]\nfn normalize_app_icon_png/,
  );
});

test('Tauri 构建的 macOS SDK 应跟随所选 Xcode，而不是固定为 CLT SDK', () => {
  const config = readSource('./src-tauri/.cargo/config.toml');
  assert.doesNotMatch(config, /^\s*SDKROOT\s*=/m,
    '固定的 CLT SDK 可能高于所选 Xcode 链接器，导致 libSystem.tbd unknown architecture');
  for (const target of ['aarch64_apple_darwin', 'x86_64_apple_darwin']) {
    assert.ok(config.includes(`CC_${target} = "/usr/bin/cc"`), '保留显式系统编译器，防止 PATH 中的同名工具覆盖');
  }
});

test('常规 CI 应在 macOS、Linux 和 Windows 执行 Rust 零警告门禁', () => {
  const workflow = readSource('./.github/workflows/ci.yml');
  const backend = workflow.match(/\n  backend:\n[\s\S]*$/)?.[0] ?? '';

  assert.ok(backend, '应能读取 backend job');
  assert.match(backend, /runs-on: \$\{\{ matrix\.platform \}\}/);
  assert.match(backend, /platform: macos-latest/);
  assert.match(backend, /platform: ubuntu-22\.04/);
  assert.match(backend, /platform: windows-latest/);
  assert.match(backend, /if: startsWith\(matrix\.platform, 'ubuntu'\)/);
  assert.match(backend, /components: rustfmt, clippy/);

  const requiredSteps = [
    ['cargo fmt', 'cargo fmt --all -- --check'],
    ['cargo check（0 warning 基线）', 'cargo check --workspace --all-targets'],
    ['cargo clippy', 'cargo clippy --workspace --all-targets -- -D warnings'],
    ['cargo test', 'cargo test --workspace --all-targets'],
  ] as const;

  for (const [name, command] of requiredSteps) {
    const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const step = backend.match(
      new RegExp(`- name: ${escapedName}\\n[\\s\\S]*?(?=\\n      - name:|$)`),
    )?.[0] ?? '';
    assert.ok(step, `backend 应包含 ${name} step`);
    assert.match(step, new RegExp(`run: ${command.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`));
    assert.doesNotMatch(step, /\n\s+if:/, `${name} 不得只在单一平台执行`);
  }

  assert.match(backend, /RUSTFLAGS: -D warnings/);
});
