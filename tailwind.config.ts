import type { Config } from 'tailwindcss';

export default {
  content: ['./src/**/*.{html,js,svelte,ts}', './index.html'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        primary: {
          50: '#eff6ff',
          100: '#dbeafe',
          200: '#bfdbfe',
          300: '#93c5fd',
          400: '#60a5fa',
          500: '#3b82f6',
          600: '#2563eb',
          700: '#1d4ed8',
          800: '#1e40af',
          900: '#1e3a8a',
        },
      },
      fontFamily: {
        sans: [
          '-apple-system',
          'BlinkMacSystemFont',
          'SF Pro Text',
          'PingFang SC',
          'Hiragino Sans GB',
          'Segoe UI Variable Text',
          'Microsoft YaHei',
          'sans-serif',
        ],
        mono: [
          'Cascadia Mono',
          'SF Mono',
          'Fira Code',
          'Menlo',
          'Consolas',
          'Microsoft YaHei UI',
          'Microsoft YaHei',
          'monospace',
        ],
      },
    },
  },
  plugins: [],
} satisfies Config;
