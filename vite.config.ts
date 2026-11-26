import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import autoprefixer from 'autoprefixer';
import path from 'path';
import tailwindcss from 'tailwindcss';

export default defineConfig({
  plugins: [svelte()],
  css: {
    postcss: {
      plugins: [tailwindcss(), autoprefixer()],
    },
  },
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    cors: {
      origin: /^https?:\/\/(?:127\.0\.0\.1|localhost)(?::\d+)?$/,
    },
    hmr: {
      host: '127.0.0.1',
    },
  },
  // 只暴露 Tauri 2 注入的 TAURI_ENV_* 构建信息；不能用宽泛的 TAURI_ 前缀，
  // 否则发布流程中的 TAURI_SIGNING_PRIVATE_KEY 等变量可能被打进前端产物。
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  optimizeDeps: {
    entries: ['index.html'],
  },
  build: {
    target: ['es2021', 'chrome100', 'safari13'],
    minify: !process.env.TAURI_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
  resolve: {
    alias: {
      '$lib': path.resolve('./src/lib'),
    },
  },
});
