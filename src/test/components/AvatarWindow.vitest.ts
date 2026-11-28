import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import { mockIPC } from '@tauri-apps/api/mocks';
import { tick } from 'svelte';

const events = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
const callbacks = vi.hoisted(() => ({
  resized: null as (() => void) | null,
  moved: null as ((event: { payload: { x: number; y: number } }) => void) | null,
}));
const native = vi.hoisted(() => ({
  outerPosition: vi.fn(async () => ({ x: 50, y: 50 })),
  outerSize: vi.fn(async () => ({ width: 276, height: 252 })),
  onResized: vi.fn(async (listener: () => void) => {
    callbacks.resized = listener;
    return () => { callbacks.resized = null; };
  }),
  onMoved: vi.fn(async (listener: (event: { payload: { x: number; y: number } }) => void) => {
    callbacks.moved = listener;
    return () => { callbacks.moved = null; };
  }),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => native,
  currentMonitor: async () => ({
    position: { x: 0, y: 0 },
    size: { width: 1920, height: 1080 },
  }),
}));
vi.mock('@tauri-apps/api/webviewWindow', () => ({
  getCurrentWebviewWindow: () => ({
    listen: async (name: string, listener: (event: { payload: unknown }) => void) => {
      events.set(name, listener);
      return () => { events.delete(name); };
    },
  }),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: async () => () => {},
  emitTo: vi.fn(async () => {}),
}));

import AvatarWindow from '../../routes/avatar/AvatarWindow.svelte';

const expansionCalls: boolean[] = [];

beforeEach(() => {
  vi.useFakeTimers();
  events.clear();
  expansionCalls.length = 0;
  native.outerPosition.mockResolvedValue({ x: 50, y: 50 });
  native.outerPosition.mockClear();
  vi.stubGlobal('ResizeObserver', class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
  mockIPC((command, args) => {
    if (command === 'get_config') return { locale: 'zh-CN' };
    if (command === 'get_avatar_state') return {
      mode: 'idle', appName: 'Work Review', contextLabel: '待命中', hint: '',
      isIdle: true, isGeneratingReport: false, avatarOpacity: 0.82,
      avatarPreset: 'original-standard', avatarPersona: 'assistant', avatarBodyHidden: true,
    };
    if (command === 'set_avatar_window_expanded') {
      expansionCalls.push((args as { expanded: boolean }).expanded);
      return null;
    }
    if (command === 'set_avatar_interactive_regions' || command === 'persist_avatar_position') return null;
    throw new Error(`Unexpected IPC command: ${command}`);
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function mountReady() {
  const view = render(AvatarWindow);
  await vi.waitFor(() => expect(callbacks.resized).not.toBeNull());
  await vi.advanceTimersByTimeAsync(500);
  return view;
}

describe('Avatar window resize and movement', () => {
  it('cancels a queued size correction when a followup expands the window', async () => {
    await mountReady();
    expect(expansionCalls).toEqual([false]);
    callbacks.resized?.();
    await vi.advanceTimersByTimeAsync(60);
    events.get('avatar-followup-suggestion')?.({ payload: {
      projectKey: 'project', date: '2026-11-27', title: '继续处理任务',
      sourceApp: 'Editor', sourceTitle: 'Task', intentLabel: '跟进',
      confidence: 0.9, sessionAgeHours: 2, persona: 'assistant',
    } });
    await tick();
    await vi.advanceTimersByTimeAsync(180);
    expect(expansionCalls).toEqual([false, true]);
    callbacks.resized?.();
    await vi.advanceTimersByTimeAsync(30);
    expect(expansionCalls).toEqual([false, true]);
  });

  it('uses the current native position after drag debouncing to orient the bubble', async () => {
    const { container } = await mountReady();
    events.get('avatar-bubble')?.({ payload: {
      message: '方向检查', tone: 'info', persistent: false, durationMs: 1000,
    } });
    await tick();
    const before = native.outerPosition.mock.calls.length;
    native.outerPosition.mockResolvedValue({ x: 10, y: 10 });
    callbacks.moved?.({ payload: { x: 1700, y: 10 } });
    await vi.advanceTimersByTimeAsync(80);
    await tick();
    expect(native.outerPosition.mock.calls.length).toBe(before + 1);
    expect(container.querySelector('.avatar-popover-anchor')?.classList.contains('right-[6%]')).toBe(true);
  });
});
