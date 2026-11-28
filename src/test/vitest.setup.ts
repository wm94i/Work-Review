import { webcrypto } from 'node:crypto';
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/svelte';
import { clearMocks } from '@tauri-apps/api/mocks';

// @tauri-apps/api/mocks 依赖 Web Crypto getRandomValues 生成 IPC 回调 ID
if (typeof globalThis.crypto === 'undefined' || typeof globalThis.crypto.getRandomValues !== 'function') {
  Object.defineProperty(globalThis, 'crypto', {
    value: webcrypto,
    configurable: true,
  });
}

// jsdom 未实现 matchMedia；Svelte motion 在模块初始化时查询减少动态效果偏好。
if (typeof window.matchMedia !== 'function') {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    writable: true,
    value: (media: string): MediaQueryList => ({
      media, matches: false, onchange: null,
      addEventListener() {}, removeEventListener() {},
      addListener() {}, removeListener() {},
      dispatchEvent: () => true,
    }),
  });
}

afterEach(() => {
  cleanup();
  // 清理 @tauri-apps/api/mocks 注册的全局 IPC 拦截器（独立于 Vitest 的 restoreMocks）
  clearMocks();
});
