import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { svelteTesting } from '@testing-library/svelte/vite';
import path from 'path';

export default defineConfig({
  plugins: [svelte({ hot: false }), svelteTesting()],
  resolve: {
    alias: {
      $lib: path.resolve('./src/lib'),
    },
  },
  test: {
    environment: 'jsdom',
    isolate: true,
    include: ['src/**/*.vitest.ts'],
    setupFiles: ['./src/test/vitest.setup.ts'],
    restoreMocks: true,
  },
});
