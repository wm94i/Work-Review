import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { mockIPC } from '@tauri-apps/api/mocks';
import { setLocale } from '$lib/i18n/index.ts';

vi.mock('@tauri-apps/plugin-dialog', () => ({
  ask: vi.fn(async () => true),
  open: vi.fn(async () => null),
}));

import SettingsStorage from '../../routes/settings/components/SettingsStorage.svelte';

function createStorageConfig(screenshotInterval = 30) {
  return {
    screenshot_interval: screenshotInterval,
    storage: {
      screenshot_retention_days: 7,
      metadata_cleanup_enabled: false,
      metadata_retention_days: 30,
      storage_limit_mb: 2048,
      screenshots_enabled: true,
      screenshot_display_mode: 'active_window',
      screenshot_width_mode: 'auto',
      max_image_width: 1280,
    },
    daily_report_export_dir: null,
    daily_report_auto_export: false,
    daily_report_export_frontmatter: false,
    daily_report_export_section_heading: null,
    git_report_enabled: false,
    git_repo_paths: [],
    git_author_filter: null,
    remote_storage: {
      provider: 'none',
      s3: {},
      webdav: {},
      sync_app_config: false,
    },
  };
}

describe('SettingsStorage polling interval (Issue #191 behavioral tests)', () => {
  it('supports the full 5~600s range without clamping values >120 or snapping non-multiples of 5 to 10', async () => {
    setLocale('zh-CN');
    const patchCalls: number[] = [];

    mockIPC((cmd, args) => {
      if (cmd === 'patch_config') {
        const patch = (args as { patch: { screenshot_interval: number } }).patch;
        patchCalls.push(patch.screenshot_interval);
        return { screenshot_interval: patch.screenshot_interval };
      }
      throw new Error(`Unexpected IPC command: ${cmd}`);
    });

    const config = createStorageConfig(300);
    render(SettingsStorage, { props: { config } });

    const numberInput = screen.getByRole('spinbutton', { name: '活动轮询间隔' }) as HTMLInputElement;
    const rangeInput = screen.getByRole('slider', { name: '活动轮询间隔' }) as HTMLInputElement;

    // 初始加载 300 秒不得被滑块钳位为 120
    expect(rangeInput.min).toBe('5');
    expect(rangeInput.max).toBe('600');
    expect(numberInput.value).toBe('300');
    expect(rangeInput.value).toBe('300');

    // 输入非 5 倍数（如 12）或低于旧下限 10（如 5）不得被滑块吸附回 10
    await fireEvent.focus(numberInput);
    await fireEvent.input(numberInput, { target: { value: '12' } });
    await fireEvent.change(numberInput, { target: { value: '12' } });
    await fireEvent.blur(numberInput);

    await waitFor(() => {
      expect(patchCalls).toEqual([12]);
    });
    expect(config.screenshot_interval).toBe(12);
    expect(numberInput.value).toBe('12');
    expect(rangeInput.value).toBe('12');

    await fireEvent.focus(numberInput);
    await fireEvent.input(numberInput, { target: { value: '5' } });
    await fireEvent.change(numberInput, { target: { value: '5' } });
    await fireEvent.blur(numberInput);

    await waitFor(() => {
      expect(patchCalls).toEqual([12, 5]);
    });
    expect(config.screenshot_interval).toBe(5);
    expect(rangeInput.value).toBe('5');

    // 拖动滑块到 420 秒并释放，立即提交局部配置补丁
    await fireEvent.input(rangeInput, { target: { value: '420' } });
    await fireEvent.change(rangeInput, { target: { value: '420' } });

    await waitFor(() => {
      expect(patchCalls).toEqual([12, 5, 420]);
    });
    expect(config.screenshot_interval).toBe(420);
    expect(numberInput.value).toBe('420');
  });

  it('flushes a pending debounced interval edit on unmount instead of discarding it', async () => {
    setLocale('zh-CN');
    const patchCalls: number[] = [];

    mockIPC((cmd, args) => {
      if (cmd === 'patch_config') {
        const patch = (args as { patch: { screenshot_interval: number } }).patch;
        patchCalls.push(patch.screenshot_interval);
        return { screenshot_interval: patch.screenshot_interval };
      }
      throw new Error(`Unexpected IPC command: ${cmd}`);
    });

    const config = createStorageConfig(30);
    const { unmount } = render(SettingsStorage, { props: { config } });

    const numberInput = screen.getByRole('spinbutton', { name: '活动轮询间隔' }) as HTMLInputElement;
    await fireEvent.focus(numberInput);
    await fireEvent.input(numberInput, { target: { value: '180' } });

    // 未等 600ms 防抖也未触发 change，立即卸载（模拟切走标签页/关闭设置）
    expect(patchCalls).toEqual([]);
    unmount();

    await waitFor(() => {
      expect(patchCalls).toEqual([180]);
    });
  });

  it('restores valid interval on empty blur and suppresses flush when restoreLocked is true', async () => {
    setLocale('zh-CN');
    const patchCalls: number[] = [];

    mockIPC((cmd, args) => {
      if (cmd === 'patch_config') {
        const patch = (args as { patch: { screenshot_interval: number } }).patch;
        patchCalls.push(patch.screenshot_interval);
        return { screenshot_interval: patch.screenshot_interval };
      }
      throw new Error(`Unexpected IPC command: ${cmd}`);
    });

    const config = createStorageConfig(90);
    const { unmount, rerender } = render(SettingsStorage, { props: { config, restoreLocked: false } });

    const numberInput = screen.getByRole('spinbutton', { name: '活动轮询间隔' }) as HTMLInputElement;
    await fireEvent.focus(numberInput);
    await fireEvent.input(numberInput, { target: { value: '' } });
    await fireEvent.blur(numberInput);

    // 清空失焦后恢复当前有效值 90，不得破坏 config.screenshot_interval 或发非法补丁
    expect(config.screenshot_interval).toBe(90);
    expect(numberInput.value).toBe('90');
    expect(patchCalls).toEqual([]);

    // 备份恢复锁定期间输入并卸载不得触发补丁落盘
    await fireEvent.focus(numberInput);
    await fireEvent.input(numberInput, { target: { value: '240' } });
    await rerender({ config, restoreLocked: true });
    unmount();

    await Promise.resolve();
    expect(patchCalls).toEqual([]);
  });

  it('prevents an older in-flight patch_config completion from clobbering a newer slider edit', async () => {
    setLocale('zh-CN');
    const patchCalls: number[] = [];
    const pendingFirst: { resolve?: () => void } = {};

    mockIPC((cmd, args) => {
      if (cmd === 'patch_config') {
        const value = (args as { patch: { screenshot_interval: number } }).patch.screenshot_interval;
        patchCalls.push(value);
        if (patchCalls.length === 1) {
          return new Promise((resolve) => {
            pendingFirst.resolve = () => resolve({ screenshot_interval: value });
          });
        }
        return { screenshot_interval: value };
      }
      throw new Error(`Unexpected IPC command: ${cmd}`);
    });

    const config = createStorageConfig(30);
    render(SettingsStorage, { props: { config } });
    const rangeInput = screen.getByRole('slider', { name: '活动轮询间隔' }) as HTMLInputElement;

    // 第一次提交 60 秒进入在途状态
    await fireEvent.input(rangeInput, { target: { value: '60' } });
    await fireEvent.change(rangeInput, { target: { value: '60' } });
    await waitFor(() => expect(patchCalls).toEqual([60]));

    // 在 60 秒补丁尚未返回前，继续把滑块拖到 240 秒
    await fireEvent.input(rangeInput, { target: { value: '240' } });
    expect(config.screenshot_interval).toBe(240);
    expect(rangeInput.value).toBe('240');

    // 旧请求 60 秒返回，不得把 config.screenshot_interval 或滑块回退为 60
    pendingFirst.resolve?.();
    await Promise.resolve();
    await Promise.resolve();
    expect(config.screenshot_interval).toBe(240);
    expect(rangeInput.value).toBe('240');

    // 松手提交 240 秒后，队列补发最新值
    await fireEvent.change(rangeInput, { target: { value: '240' } });
    await waitFor(() => {
      expect(patchCalls).toEqual([60, 240]);
    });
    expect(config.screenshot_interval).toBe(240);
  });
});
