# Work Review Evidence Star Map UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 保留经典界面并新增可持久化的“证据星图”界面模板，第一阶段覆盖应用 Shell、概览和时间线。

**Architecture:** 新增独立 `ui_template` 配置，不复用经典模式的 `ui_visual_style` 语义。应用层通过模板 Store 和根节点属性选择 Shell；概览和时间线继续共享现有业务数据，通过模板专用组件与作用域样式呈现。

**Tech Stack:** Rust/Serde、Tauri 2、Svelte 4、TypeScript、lucide-svelte 1.0.1、CSS、Node test runner。

## Global Constraints

- 默认模板必须是 `classic`。
- 新模板配置值固定为 `evidence-star-map`。
- 经典模式现有 `a/b/c` 外观必须继续工作。
- 功能性图标统一使用 Lucide；触及范围不使用 Emoji 或字符图标。
- 不引入 WebGL、Three.js 或持续 Canvas 循环。
- 支持 `prefers-reduced-motion`。
- 不修改 README，不提交演示视频资产。
- 每条测试命令最大运行 60 秒。

---

### Task 1: 模板配置与状态

**Files:**
- Create: `src/lib/stores/uiTemplate.ts`
- Create: `src/lib/stores/uiTemplate.test.ts`
- Modify: `crates/core/src/config.rs`
- Modify: `src/App.svelte`
- Modify: `src/routes/settings/Settings.svelte`

**Interfaces:**
- Produces: `UiTemplate = 'classic' | 'evidence-star-map'`
- Produces: `normalizeUiTemplate(value: unknown): UiTemplate`
- Produces: `uiTemplate` Svelte store and `applyUiTemplate(value)`

- [ ] 写失败测试，验证默认值、非法值归一化和根节点属性。
- [ ] 运行定向测试并确认因接口缺失失败。
- [ ] 实现 TypeScript Store 与 Rust 配置字段、默认值、归一化和测试。
- [ ] 让 `App.svelte` 启动、配置同步和自定义事件均应用模板。
- [ ] 运行前端定向测试和 Rust `config` 定向测试。

### Task 2: Lucide 与模板 Shell

**Files:**
- Modify: `package.json`
- Modify: `package-lock.json`
- Create: `src/lib/components/evidence/EvidenceShell.svelte`
- Create: `src/lib/components/evidence/EvidenceNavigation.svelte`
- Create: `src/EvidenceTemplate.test.ts`
- Modify: `src/App.svelte`
- Modify: `src/app.css`

**Interfaces:**
- Consumes: `uiTemplate`
- Produces: `EvidenceShell` 的默认 slot、导航与页面舞台。

- [ ] 写失败的源代码契约测试，要求 Lucide、模板 Shell、ARIA 和作用域 Token。
- [ ] 安装 `lucide-svelte@1.0.1` 并实现 Shell/导航。
- [ ] 在 App 中按模板渲染 Classic/Evidence Shell，保留同一 Router。
- [ ] 加入窄窗口与减少动效样式。
- [ ] 运行定向测试和 Svelte 检查。

### Task 3: 设置页模板选择器

**Files:**
- Modify: `src/routes/settings/components/SettingsAppearance.svelte`
- Modify: `src/routes/settings/Settings.svelte`
- Modify: `src/lib/i18n/locales/zh-CN.ts`
- Modify: `src/lib/i18n/locales/zh-TW.ts`
- Modify: `src/lib/i18n/locales/en.ts`
- Modify: `src/lib/i18n/locales/ar.ts`
- Create: `src/UiTemplateSettings.test.ts`

**Interfaces:**
- Emits: `ui-template-changed` with `{ template: UiTemplate }`
- Persists: complete config through existing `save_config`
- Emits settings change `{ autosaved: true }` after success.

- [ ] 写失败测试，验证双模板选项、保存、回滚和翻译键。
- [ ] 实现模板卡片、即时应用、最后请求保护和失败回滚。
- [ ] 运行定向测试和 Svelte 检查。

### Task 4: 证据星图概览与时间线视觉层

**Files:**
- Create: `src/lib/components/evidence/EvidenceOverviewHeader.svelte`
- Create: `src/lib/components/evidence/EvidenceTimelineHeader.svelte`
- Modify: `src/routes/Overview.svelte`
- Modify: `src/routes/timeline/Timeline.svelte`
- Modify: `src/app.css`
- Create: `src/routes/EvidenceViews.test.ts`

**Interfaces:**
- Overview header props: `dateLabel`, `totalDuration`, `evidenceCount`, `isRecording`.
- Timeline header props: `dateLabel`, `activityCount`, `selectedRangeLabel`.

- [ ] 写失败测试，验证模板专用标题、证据场、轨迹层和文本摘要。
- [ ] 在现有数据加载逻辑上接入模板标题组件，不复制 Tauri 请求。
- [ ] 使用模板作用域 CSS 将概览和时间线改造为星图画布与证据带。
- [ ] 增加空数据、窄窗口、焦点和减少动效处理。
- [ ] 运行定向测试、静态检查和生产构建。

### Task 5: 总体验证与审查

**Files:**
- Modify only when verification exposes defects.

- [ ] 运行 `npm run check`（60 秒上限）。
- [ ] 运行 `npm run test:frontend`（60 秒上限）。
- [ ] 运行 `npm run build`（60 秒上限）。
- [ ] 运行 Rust 配置定向测试（60 秒上限）。
- [ ] 检查 Git diff、Emoji/字符图标、README 和媒体资产边界。
- [ ] 进行代码审查并修复高优先级问题。
