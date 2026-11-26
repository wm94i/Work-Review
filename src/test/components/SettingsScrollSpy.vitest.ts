import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { mockIPC } from '@tauri-apps/api/mocks';
import { setLocale } from '$lib/i18n/index.ts';
import { resolveActiveSettingsSection } from '../../routes/settings/settingsScrollSpy.ts';

let reducedMotionMatches = false;

vi.hoisted(() => {
  if (typeof window !== 'undefined' && typeof window.matchMedia !== 'function') {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      configurable: true,
      value: (query: string) => ({
        matches: query.includes('prefers-reduced-motion') ? false : false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }),
    });
  }
});

import Settings from '../../routes/settings/Settings.svelte';

afterEach(() => {
  reducedMotionMatches = false;
  cleanup();
  vi.restoreAllMocks();
});

describe('resolveActiveSettingsSection (unit tests)', () => {
  it('returns fallback when measurements list is empty', () => {
    expect(
      resolveActiveSettingsSection([], {
        activationOffset: 120,
        atBottom: false,
        fallback: 'general',
      }),
    ).toBe('general');
  });

  it('activates the first section at top of scroll', () => {
    const sections = [
      { id: 'general', top: 0 },
      { id: 'appearance', top: 420 },
      { id: 'ai', top: 960 },
    ] as const;

    expect(
      resolveActiveSettingsSection(sections, {
        activationOffset: 120,
        atBottom: false,
        fallback: 'general',
      }),
    ).toBe('general');
  });

  it('tracks the deepest section whose top has crossed activationOffset', () => {
    const sections = [
      { id: 'general', top: -700 },
      { id: 'appearance', top: -240 },
      { id: 'ai', top: 90 },
      { id: 'avatar', top: 520 },
    ] as const;

    expect(
      resolveActiveSettingsSection(sections, {
        activationOffset: 120,
        atBottom: false,
        fallback: 'general',
      }),
    ).toBe('ai');
  });

  it('activates the last section when scrolled to bottom even if short', () => {
    const sections = [
      { id: 'general', top: -1800 },
      { id: 'storage', top: 40 },
      { id: 'node', top: 380 },
    ] as const;

    expect(
      resolveActiveSettingsSection(sections, {
        activationOffset: 120,
        atBottom: true,
        fallback: 'general',
      }),
    ).toBe('node');
  });
});

describe('Settings continuous scroll-spy & click-to-scroll (behavioral tests)', () => {
  function setupSettingsMocks() {
    mockIPC((cmd) => {
      switch (cmd) {
        case 'get_config':
          return {
            auto_start: false,
            auto_start_silent: false,
            screenshot_interval: 30,
            ai_mode: 'local',
            ui_visual_style: 'c',
            avatar_enabled: false,
            avatar_persona: 'companion',
            avatar_preset: 'original',
            avatar_click_through: false,
            avatar_body_hidden: false,
            avatar_proactive_ai_enabled: false,
            break_reminder_enabled: false,
            break_reminder_interval_minutes: 50,
            background_image: null,
            background_opacity: 0.15,
            background_blur: 0,
            projects: [],
            privacy: {
              app_rules: [],
              excluded_keywords: [],
              excluded_domains: [],
              skip_private_windows: true,
              unknown_url_fallback: null,
            },
            storage: {
              screenshot_retention_days: 7,
              metadata_cleanup_enabled: false,
              metadata_retention_days: 30,
              storage_limit_mb: 2048,
              jpeg_quality: 85,
              max_image_width: 1280,
              screenshots_enabled: true,
              screenshot_display_mode: 'active_window',
              screenshot_width_mode: 'auto',
            },
            text_model: {
              provider: 'ollama',
              endpoint: 'http://localhost:11434',
              model: 'qwen2.5',
              api_key: null,
            },
            assistant_web_access_enabled: false,
            assistant_search_provider: 'duckduckgo',
            assistant_search_api_key: '',
            memory_semantic_enabled: false,
            semantic_auto_index: false,
            embedding_provider: 'ollama',
            embedding_endpoint: 'http://localhost:11434',
            embedding_model: 'nomic-embed-text',
            embedding_api_key: '',
            assistant_memory_enabled: false,
            localhost_api_enabled: false,
            localhost_api_port: 47831,
            localhost_api_host: null,
            node_devices: [],
            node_gateway: { device_name: null },
          };
        case 'get_ai_providers':
          return [
            {
              id: 'ollama',
              name: 'Ollama',
              description: 'Local',
              default_endpoint: 'http://localhost:11434',
              default_model: 'qwen2.5',
              requires_api_key: false,
            },
          ];
        case 'get_storage_stats':
          return {
            database_size_mb: 1.2,
            screenshots_size_mb: 12.5,
            total_size_mb: 13.7,
            screenshot_count: 10,
            activity_count: 25,
          };
        case 'get_data_dir':
        case 'get_default_data_dir':
          return 'D:\\data\\mock';
        case 'get_runtime_platform':
          return 'windows';
        case 'get_running_apps':
        case 'get_recent_apps':
        case 'get_projects':
        case 'list_assistant_memory_entries':
        case 'fetch_models':
          return [];
        case 'get_node_gateway_status':
          return { deviceId: 'dev-1', protocolVersion: '1', deviceName: 'Mock' };
        case 'get_localhost_api_status':
          return { enabled: false, baseUrl: '', tokenPreview: '', lastError: null };
        case 'get_telegram_bot_status':
        case 'get_wecom_bot_status':
          return null;
        case 'is_autostart_enabled':
          return false;
        case 'get_background_image':
          return null;
        case 'semantic_memory_status':
          return {
            status: 'idle',
            rebuildRequired: false,
            indexedActivities: 0,
            totalActivities: 0,
            pendingReindexDates: 0,
            lastError: null,
          };
        default:
          return null;
      }
    });
  }

  function createScrollHarness() {
    const scrollHost = document.createElement('main');
    scrollHost.className = 'app-shell-main-scroll';
    document.body.appendChild(scrollHost);

    const scrollToSpy = vi.fn();
    scrollHost.scrollTo = scrollToSpy;

    const rafQueue = new Map<number, FrameRequestCallback>();
    let nextRafId = 1;
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      const id = nextRafId++;
      rafQueue.set(id, cb);
      return id;
    });
    vi.spyOn(window, 'cancelAnimationFrame').mockImplementation((id) => {
      rafQueue.delete(id);
    });

    vi.spyOn(window, 'matchMedia').mockImplementation((query: string) => ({
      matches: query.includes('prefers-reduced-motion') ? reducedMotionMatches : false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }));

    const flushRaf = () => {
      const pending = Array.from(rafQueue.values());
      rafQueue.clear();
      for (const cb of pending) {
        cb(16);
      }
    };

    const tops: Record<string, number> = {
      general: 0,
      appearance: 400,
      ai: 800,
      avatar: 1200,
      projects: 1600,
      privacy: 2000,
      storage: 2400,
      node: 2800,
    };

    scrollHost.getBoundingClientRect = () => ({
      top: 0,
      bottom: 800,
      left: 0,
      right: 1200,
      width: 1200,
      height: 800,
      x: 0,
      y: 0,
      toJSON: () => {},
    });
    Object.defineProperty(scrollHost, 'clientHeight', { value: 800, configurable: true });
    Object.defineProperty(scrollHost, 'scrollHeight', { value: 3400, configurable: true });
    Object.defineProperty(scrollHost, 'scrollTop', { value: 0, writable: true, configurable: true });

    const bindSectionGeometry = (container: HTMLElement) => {
      for (const section of container.querySelectorAll<HTMLElement>('[data-settings-section]')) {
        const id = section.dataset.settingsSection!;
        section.getBoundingClientRect = () => ({
          top: tops[id] - scrollHost.scrollTop,
          bottom: tops[id] - scrollHost.scrollTop + 360,
          left: 0,
          right: 900,
          width: 900,
          height: 360,
          x: 0,
          y: tops[id] - scrollHost.scrollTop,
          toJSON: () => {},
        });
      }
    };

    return { scrollHost, scrollToSpy, flushRaf, bindSectionGeometry };
  }

  it('mounts all 8 sections continuously, keeps clicked near-bottom item across scroll+scrollend, tracks scroll/atBottom, respects reduced-motion, and cleans up on unmount', async () => {
    setLocale('zh-CN');
    setupSettingsMocks();
    const { scrollHost, scrollToSpy, flushRaf, bindSectionGeometry } = createScrollHarness();

    try {
      const { container, unmount } = render(Settings, { target: scrollHost });

      await waitFor(() => {
        expect(container.querySelectorAll('[data-settings-section]').length).toBe(8);
      });

      const sectionIds = Array.from(container.querySelectorAll<HTMLElement>('[data-settings-section]'))
        .map((el) => el.dataset.settingsSection);
      expect(sectionIds).toEqual([
        'general',
        'appearance',
        'ai',
        'avatar',
        'projects',
        'privacy',
        'storage',
        'node',
      ]);

      bindSectionGeometry(container);

      // 1. 点击靠近底部的“存储与数据”，模拟浏览器同帧先 scroll 后 scrollend 再 flush rAF：
      // 即使此时容器滚到底部（atBottom=true），也不应被末尾 node 抢占高亮
      const storageNav = container.querySelector<HTMLButtonElement>('[data-settings-nav="storage"]')!;
      await fireEvent.click(storageNav);

      expect(storageNav.getAttribute('aria-current')).toBe('page');
      expect(scrollToSpy).toHaveBeenCalledTimes(1);
      expect(scrollToSpy).toHaveBeenCalledWith({ top: 2400 - 16, behavior: 'smooth' });

      scrollHost.scrollTop = 2600; // scrollTop(2600) + clientHeight(800) === scrollHeight(3400) -> atBottom=true
      scrollHost.dispatchEvent(new Event('scroll'));
      scrollHost.dispatchEvent(new Event('scrollend'));
      flushRaf();

      expect(storageNav.getAttribute('aria-current')).toBe('page');

      // 2. 用户滚轮滚动到“隐私控制”区域，自动跟随高亮隐私导航
      scrollHost.scrollTop = 1950;
      await fireEvent.wheel(scrollHost);
      scrollHost.dispatchEvent(new Event('scroll'));
      flushRaf();

      await waitFor(() => {
        const privacyNav = container.querySelector<HTMLButtonElement>('[data-settings-nav="privacy"]')!;
        expect(privacyNav.getAttribute('aria-current')).toBe('page');
      });

      // 3. 用户滚至容器最底部，atBottom 应高亮最后一个分区 node
      scrollHost.scrollTop = 2600;
      scrollHost.dispatchEvent(new Event('scroll'));
      flushRaf();

      await waitFor(() => {
        const nodeNav = container.querySelector<HTMLButtonElement>('[data-settings-nav="node"]')!;
        expect(nodeNav.getAttribute('aria-current')).toBe('page');
      });

      // 4. prefers-reduced-motion: reduce 应降级为 behavior: 'auto'
      reducedMotionMatches = true;
      const aiNav = container.querySelector<HTMLButtonElement>('[data-settings-nav="ai"]')!;
      await fireEvent.click(aiNav);
      expect(scrollToSpy).toHaveBeenLastCalledWith({ top: 800 - 16, behavior: 'auto' });

      expect(screen.getAllByRole('navigation').length).toBeGreaterThanOrEqual(1);

      // 5. 卸载后触发事件不应报错
      unmount();
      expect(() => {
        scrollHost.dispatchEvent(new Event('scroll'));
        scrollHost.dispatchEvent(new Event('wheel'));
        scrollHost.dispatchEvent(new Event('scrollend'));
        flushRaf();
      }).not.toThrow();
    } finally {
      scrollHost.remove();
    }
  });
});
