# Work-Review 85 秒产品演示视频实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 基于真实 Work-Review Svelte 界面和完全虚构的隔离数据，自动录制、合成并验证横屏与竖屏两套 85 秒产品演示视频，同时生成中英字幕、封面和交付清单。

**Architecture:** 复用现有 Playwright 截图脚本中的固定时区、网络拦截和 Tauri Browser Mock 方案，将演示夹具、分镜、字幕、Mock、录制、合成和验证拆成独立 TypeScript 模块。Playwright 逐分镜录制真实页面交互，Playwright 用 HTML/CSS 渲染透明双语字幕层和封面，FFmpeg 负责精确时长、横竖屏构图、字幕层叠加、中文旁白、程序化背景音乐和最终 H.264/AAC 输出；所有外部网络、真实 SQLite、真实 AI 和真实目录操作都被禁止。

**Tech Stack:** Node.js 22、TypeScript 5.9、tsx、Node Test Runner、Svelte 4/Vite 5、Playwright 1.61.1、FFmpeg/ffprobe 8.x、macOS `say`、现有 Work-Review 前端与 Tauri API Browser Mock。

## Global Constraints

- 总时长固定为 `85.000` 秒、`30fps CFR`、`2550` 帧。
- 横屏固定为 `1920×1080`；竖屏固定为 `1080×1920`，逐镜头重新构图，禁止简单中央裁切。
- 发布格式固定为 MP4 / H.264 / AAC / `yuv420p`。
- 固定日期为 `2026-08-12`，时区为 `Asia/Shanghai`，语言为简体中文。
- 演示项目、活动、模型、路径和回答必须全部使用虚构数据；允许的绝对路径仅限 `/tmp/work-review-demo/`。
- 助手必须在同一会话中完整展示一次“基础模板”和一次“演示 AI”，不得增加第三轮助手问答。
- 日报按所选日期的整日记录生成；时间线只打开单个聚合活动详情，禁止多选或编辑时间片。
- 隐私 UI 只展示“完全记录 / 仅统计时长 / 完全忽略”；关闭截图时说明截图与 OCR 同时停止。
- 不得伪造独立 OCR 开关、全局 AI 关闭开关、整篇 WYSIWYG 日报编辑或自动保存。
- 活动记录和截图默认保存在本机；外部 AI 与远程存储只能描述为按配置使用。
- 不调用真实外部 AI、远程存储或生产 API，不读取或写入真实 Work-Review 数据库。
- README 不在本次范围，任何任务都不得修改 README。
- 自动化测试单次最长运行 60 秒。
- `artifacts/product-demo/` 只存放生成产物，不提交大体积视频或中间媒体。

---

## 文件结构与职责

### 新增源文件

- `scripts/product-demo/types.ts`：跨模块共享的分镜、字幕、构图和媒体类型。
- `scripts/product-demo/storyboard.ts`：七段分镜、85 秒时间轴、旁白和横竖屏构图定义。
- `scripts/product-demo/fixtures.ts`：Aurora Board 虚构配置、统计、时间线、日报、助手和安全路径夹具。
- `scripts/product-demo/subtitles.ts`：中文、英文、双语 SRT，以及 HTML/CSS 透明硬字幕层生成。
- `scripts/product-demo/privacy.ts`：敏感字段、外部 URL、真实主目录和禁止产品能力扫描。
- `scripts/product-demo/tauriMock.ts`：浏览器内 Tauri invoke、callback、Channel 和副作用 Mock。
- `scripts/product-demo/browser.ts`：Vite 生命周期、Chromium 上下文、网络拦截、固定时间和演示光标。
- `scripts/product-demo/shots.ts`：七个分镜的真实 UI 操作序列与横竖屏焦点。
- `scripts/product-demo/capture.ts`：逐分镜录制 WebM、截图关键帧并输出录制元数据。
- `scripts/product-demo/audio.ts`：中文旁白、程序化背景音乐、点击/完成音效和音频规范化。
- `scripts/product-demo/ffmpeg.ts`：纯函数构建 FFmpeg/ffprobe 参数和媒体探测结果。
- `scripts/product-demo/compose.ts`：横竖屏分镜裁切、拼接、透明字幕层、音频和封面合成。
- `scripts/product-demo/validate.ts`：媒体参数、字幕、隐私、帧数和交付文件验证。
- `scripts/product-demo/cli.ts`：`check/capture/subtitles/audio/compose/validate/build` 命令入口。

### 新增测试文件

- `scripts/product-demo/storyboard.test.ts`
- `scripts/product-demo/fixtures.test.ts`
- `scripts/product-demo/subtitles.test.ts`
- `scripts/product-demo/privacy.test.ts`
- `scripts/product-demo/tauriMock.test.ts`
- `scripts/product-demo/ffmpeg.test.ts`
- `scripts/product-demo/validate.test.ts`

### 修改文件

- `package.json`：增加产品演示检查、录制、合成、验证和全流程脚本。
- `.gitignore`：忽略 `artifacts/product-demo/`，但不忽略源代码和小型测试夹具。

### 生成但不提交

- `artifacts/product-demo/intermediate/`：分镜 WebM、关键帧、WAV、透明字幕 PNG 和 FFmpeg 临时文件。
- `artifacts/product-demo/work-review-demo-16x9.mp4`
- `artifacts/product-demo/work-review-demo-9x16.mp4`
- `artifacts/product-demo/work-review-demo-zh.srt`
- `artifacts/product-demo/work-review-demo-en.srt`
- `artifacts/product-demo/work-review-demo-zh-en.srt`
- `artifacts/product-demo/work-review-demo-cover-16x9.png`
- `artifacts/product-demo/work-review-demo-cover-9x16.png`
- `artifacts/product-demo/manifest.json`
- `artifacts/product-demo/SHA256SUMS`

---

### Task 1: 锁定分镜、媒体规格与安全夹具

**Files:**
- Create: `scripts/product-demo/types.ts`
- Create: `scripts/product-demo/storyboard.ts`
- Create: `scripts/product-demo/fixtures.ts`
- Test: `scripts/product-demo/storyboard.test.ts`
- Test: `scripts/product-demo/fixtures.test.ts`

**Interfaces:**
- Produces: `DEMO_DATE`, `DEMO_TIMEZONE`, `DEMO_FPS`, `DEMO_DURATION_SECONDS`, `DEMO_TOTAL_FRAMES`。
- Produces: `StoryboardScene`, `StoryboardCue`, `DemoFixtures` 类型。
- Produces: `STORYBOARD`, `VOICEOVER_CUES`, `createDemoFixtures()` 和 `validateStoryboard()`。
- Consumes: 设计规格中的七段时间边界、旁白和 Aurora Board 固定数据。

- [ ] **Step 1: 写分镜失败测试**

  在 `storyboard.test.ts` 中断言：

  ```ts
  assert.equal(DEMO_DURATION_SECONDS, 85);
  assert.equal(DEMO_TOTAL_FRAMES, 2550);
  assert.equal(STORYBOARD.length, 7);
  assert.deepEqual(STORYBOARD.map(({ start, end }) => [start, end]), [
    [0, 7], [7, 19], [19, 32], [32, 51], [51, 68], [68, 78], [78, 85],
  ]);
  assert.doesNotThrow(validateStoryboard);
  ```

  额外断言助手分镜包含且只包含 `basic-template` 和 `demo-ai` 两个阶段，横竖屏构图都有定义。

- [ ] **Step 2: 运行测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/storyboard.test.ts`

  Expected: FAIL，原因是 `storyboard.ts` 和导出尚不存在。

- [ ] **Step 3: 实现最小分镜模型**

  在 `types.ts` 定义：

  ```ts
  export type DemoAspect = '16x9' | '9x16';
  export type StoryboardSceneId =
    | 'hook' | 'timeline' | 'report' | 'assistant'
    | 'privacy' | 'export' | 'outro';

  export interface StoryboardCue {
    id: string;
    start: number;
    end: number;
    zh: string;
    en: string;
  }

  export interface StoryboardScene {
    id: StoryboardSceneId;
    start: number;
    end: number;
    route: string;
    voiceoverZh: string;
    subtitleEn: string;
    composition: Record<DemoAspect, { crop: string; scale: string; focus: string }>;
  }
  ```

  `validateStoryboard()` 必须检查连续、无重叠、首帧为 0、末帧为 85、每段时长为整数帧，并检查助手阶段数量。

- [ ] **Step 4: 写夹具失败测试**

  在 `fixtures.test.ts` 断言：

  - 日期和时区固定。
  - 六条活动时间和标题与规格一致。
  - 总投入为 19,800 秒，分类合计相等。
  - 报告至少有四个段落。
  - 基础模板完整回答“今天主要做了什么？”，只陈述 19,800 秒（5 小时 30 分钟）的统计与六条虚构活动。
  - AI 完整回答“结合刚才的今日记录，再查今天的活动，提炼一项有依据的日报成果。”，固定结论包含 `16:40` 和 `npm run verify:frontend` 证据。
  - 所有路径以 `/tmp/work-review-demo/` 开头。

- [ ] **Step 5: 运行夹具测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/fixtures.test.ts`

  Expected: FAIL，原因是 `createDemoFixtures()` 不存在。

- [ ] **Step 6: 实现最小安全夹具**

  `createDemoFixtures()` 返回全新的深拷贝对象，避免分镜之间共享可变状态。夹具中不得出现真实用户名、`github.com`、`openai.com` 或仓库真实路径。

- [ ] **Step 7: 运行本任务测试并确认 GREEN**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/storyboard.test.ts scripts/product-demo/fixtures.test.ts`

  Expected: PASS，0 failures。

- [ ] **Step 8: 提交**

  ```bash
  git add scripts/product-demo/types.ts scripts/product-demo/storyboard.ts scripts/product-demo/fixtures.ts scripts/product-demo/storyboard.test.ts scripts/product-demo/fixtures.test.ts
  git commit -m "feat(demo): define storyboard and safe fixtures"
  ```

---

### Task 2: 生成可复用的中英字幕时间轴

**Files:**
- Create: `scripts/product-demo/subtitles.ts`
- Test: `scripts/product-demo/subtitles.test.ts`

**Interfaces:**
- Consumes: `VOICEOVER_CUES`、`StoryboardCue`、横竖屏安全区。
- Produces: `formatSrtTimestamp(seconds)`, `buildSrt(cues, language)`, `buildBilingualSrt(cues)`, `buildSubtitleOverlayHtml(cue, aspect)` 和 `writeSubtitleArtifacts(outputDir)`；透明 PNG 的浏览器渲染在 Task 7 执行。

- [ ] **Step 1: 写字幕失败测试**

  覆盖：

  ```ts
  assert.equal(formatSrtTimestamp(7.04), '00:00:07,040');
  assert.match(buildSrt(VOICEOVER_CUES, 'zh'), /下班时/);
  assert.match(buildSrt(VOICEOVER_CUES, 'en'), /At the end of the day/);
  assert.match(buildBilingualSrt(VOICEOVER_CUES), /下班时[\s\S]*At the end/);
  ```

  同时验证时间码单调、末条不超过 85 秒、相邻条目不重叠、中文和英文条目数量相等、双语每条中文在上英文在下。

- [ ] **Step 2: 运行测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/subtitles.test.ts`

  Expected: FAIL，原因是字幕模块尚不存在。

- [ ] **Step 3: 实现 SRT 与 HTML/CSS 字幕层生成器**

  - SRT 使用 UTF-8 和 CRLF 或 LF 均可读格式。
  - HTML/CSS 字幕层横屏字号 `54/32`，竖屏字号 `50/30`。
  - 字体使用与产品一致的系统字体栈，不复制或提交系统字体文件。
  - 竖屏底部安全边距大于横屏，并为平台交互区留白；背景透明，字幕带高对比底板与阴影。

- [ ] **Step 4: 运行测试并确认 GREEN**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/subtitles.test.ts`

  Expected: PASS，0 failures。

- [ ] **Step 5: 提交**

  ```bash
  git add scripts/product-demo/subtitles.ts scripts/product-demo/subtitles.test.ts
  git commit -m "feat(demo): generate bilingual subtitles"
  ```

---

### Task 3: 建立隐私扫描和产品真实性门禁

**Files:**
- Create: `scripts/product-demo/privacy.ts`
- Test: `scripts/product-demo/privacy.test.ts`

**Interfaces:**
- Consumes: 任意字符串、夹具对象、字幕内容、日志和导出 Markdown。
- Produces: `scanSensitiveText(text, source)`, `scanDemoObject(value, source)`, `assertNoSensitiveContent(findings)`。
- Produces: `PrivacyFinding`，包含 `source`, `rule`, `match`，但输出时对密钥类内容做掩码。

- [ ] **Step 1: 写失败测试**

  测试必须识别：

  - `/Users/alice/`、`C:\\Users\\alice\\`。
  - 邮箱、Bearer Token、API Key、Cookie。
  - `github.com`、`openai.com` 和非本地 HTTP(S) URL。
  - 禁止能力文案：“多选片段生成”“独立 OCR 开关”“关闭所有 AI”“所有数据永远不会离开本机”。

  测试必须放行：

  - `/tmp/work-review-demo/`
  - `docs.example.test`
  - `http://127.0.0.1:5173`
  - `data:` 和 `blob:` URL。

- [ ] **Step 2: 运行测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/privacy.test.ts`

  Expected: FAIL，原因是扫描器尚不存在。

- [ ] **Step 3: 实现最小扫描器**

  规则必须是显式白名单 + 可解释正则，而不是模糊启发式。扫描报告只显示命中的短片段，避免二次泄露完整密钥。

- [ ] **Step 4: 对分镜和夹具执行扫描**

  在测试中调用：

  ```ts
  assertNoSensitiveContent(scanDemoObject({ storyboard: STORYBOARD, fixtures: createDemoFixtures() }, 'demo-source'));
  ```

- [ ] **Step 5: 运行测试并确认 GREEN**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/privacy.test.ts scripts/product-demo/storyboard.test.ts scripts/product-demo/fixtures.test.ts`

  Expected: PASS，0 failures。

- [ ] **Step 6: 提交**

  ```bash
  git add scripts/product-demo/privacy.ts scripts/product-demo/privacy.test.ts
  git commit -m "test(demo): enforce privacy and product truth"
  ```

---

### Task 4: 实现隔离的 Tauri Browser Mock

**Files:**
- Modify: `scripts/product-demo/types.ts`
- Create: `scripts/product-demo/tauriMock.ts`
- Test: `scripts/product-demo/tauriMock.test.ts`
- Reference: `scripts/capture-readme-pages.ts:499-645`
- Reference: `src/routes/ask/Ask.svelte`
- Reference: `src/AssistantStreaming.test.ts`

**Interfaces:**
- Consumes: `DemoFixtures` 和 `DemoMockState`。
- Produces: `createDemoMockState(fixtures)`, `handleDemoInvoke(state, command, args)`, `installDemoTauriMock(context, fixtures)`。
- Produces: `DemoMockState`，包含可变内存状态 `reportGenerated`, `reportContent`, `selectedAssistantModel`, `assistantMessages`, `privacyRules`, `screenshotsEnabled`, `exportedFiles`, `invokeLog`。

- [ ] **Step 1: 写 invoke 行为失败测试**

  使用纯函数 `handleDemoInvoke()` 测试：

  - `get_config`、统计、时间线、单活动详情、截图缩略图。
  - `generate_report` 将 `reportGenerated` 置为 true。
  - `get_saved_report` 在生成前返回空，生成后返回完整报告。
  - `update_report_content` 修改内存报告。
  - `save_config` 保存隐私三档和 `screenshots_enabled`。
  - `open_data_dir` 只记录打开 `/tmp/work-review-demo/data/`，不触发真实目录操作。
  - `export_report_markdown` 返回 `/tmp/work-review-demo/exports/2026-08-12.md` 并记录虚拟导出内容。
  - 助手会话创建、消息追加、模型配置和历史读取。

- [ ] **Step 2: 运行测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/tauriMock.test.ts`

  Expected: FAIL，原因是 Mock 模块尚不存在。

- [ ] **Step 3: 实现纯内存 invoke 路由**

  复用现有 `__TAURI_INTERNALS__` 的 metadata、callbacks、`transformCallback`、`runCallback` 和 `convertFileSrc` 形状。默认分支必须抛出包含命令名的错误，禁止像 README 截图脚本一样静默返回 `null`，避免录制遗漏命令而不自知。

- [ ] **Step 4: 实现助手 Channel 固定流式序列**

  基础模板和 AI 必须分别发送确定的事件序列：

  ```text
  stepStart → stepResult → token... → done → Channel end
  ```

  `DemoMockState` 在 `types.ts` 中定义，至少包含本任务列出的状态字段和会话调用计数。事件必须使用 `{ index, message }` Channel 信封并携带当前 request/message id；先发送 `done`，再发送 Channel `end`，最后 resolve `chat_work_assistant`。`query_activities` 固定返回 `hits: 0` 和空 `references`；AI 的最终文本必须引用同会话前一轮和 16:40 验证记录。不得发起网络请求。`generate_text_with_model` 固定返回 JSON 字符串 `'["今天最值得总结的成果是什么？"]'`。

- [ ] **Step 5: 写助手上下文断言**

  断言第二次 AI 调用收到的 history 含第一轮用户问题和基础模板完整回答，且会话 ID 未变化。

- [ ] **Step 6: 运行测试并确认 GREEN**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/tauriMock.test.ts src/routes/ask/historyPayload.test.ts src/AssistantStreaming.test.ts`

  Expected: PASS，0 failures。

- [ ] **Step 7: 提交**

  ```bash
  git add scripts/product-demo/types.ts scripts/product-demo/tauriMock.ts scripts/product-demo/tauriMock.test.ts
  git commit -m "feat(demo): add isolated Tauri mock"
  ```

---

### Task 5: 实现确定性的 Playwright 分镜录制

**Files:**
- Modify: `scripts/product-demo/types.ts`
- Create: `scripts/product-demo/browser.ts`
- Create: `scripts/product-demo/shots.ts`
- Create: `scripts/product-demo/capture.ts`
- Create: `scripts/product-demo/cli.ts`
- Modify: `package.json`
- Reference: `scripts/capture-readme-pages.ts:52-80,646-748`

**Interfaces:**
- Consumes: `STORYBOARD`, `createDemoFixtures()`, `installDemoTauriMock()`。
- Produces: `startDemoServer()`, `createDemoContext(browser, aspect)`, `recordScene(scene, aspect, outputDir)`, `captureAllScenes(options)` 和首版 `cli.ts` 的 `check/capture` 命令。
- Produces: `artifacts/product-demo/intermediate/<aspect>/<scene>.webm` 和关键帧 PNG。

- [ ] **Step 1: 在 Task 1 测试中补充录制配置失败断言**

  断言横屏录制视口、竖屏高分辨率源视口、deviceScaleFactor、时区、locale 和 reducedMotion 是固定值。竖屏可以用高分辨率源素材，但每个镜头必须使用独立 `composition['9x16']`。

- [ ] **Step 2: 运行测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/storyboard.test.ts`

  Expected: FAIL，原因是浏览器录制配置导出尚不存在。

- [ ] **Step 3: 实现浏览器和 Vite 生命周期**

  - 用 `spawn` 启动 `npm run dev -- --host 127.0.0.1 --port 5173 --strictPort`。
  - 使用 HTTP 轮询确认服务就绪，不增加新依赖。
  - Chromium 使用现有截图脚本的字体稳定参数和 sRGB 参数。
  - `context.route('**/*')` 只放行同源、`data:` 和 `blob:`。
  - `addInitScript` 固定 Date、时区、locale、Tauri Mock 和演示光标。
  - `finally` 中关闭 Context、Browser 和 Vite 子进程。

- [ ] **Step 4: 实现七个真实 UI 操作函数**

  `shots.ts` 必须提供：

  ```ts
  export interface ShotContext {
    page: import('playwright').Page;
    aspect: DemoAspect;
    scene: StoryboardScene;
    fixtures: DemoFixtures;
  }
  export type ShotRunner = (context: ShotContext) => Promise<void>;
  export const SHOT_RUNNERS: Record<StoryboardSceneId, ShotRunner>;
  ```

  每个 runner 只能使用用户可见的角色、文本或稳定语义选择器，并在动作后断言真实结果：

  - `hook`：打开日报空态并展示标题包装。
  - `timeline`：进入日期时间线，点击一个聚合项，等待单个详情抽屉，关闭。
  - `report`：生成整日日报，展示骨架后由 Mock 完成；打开段落编辑弹窗，修改并点击“保存段落”。
  - `assistant`：进入页面前把 `work-review-assistant-state` 写为 `selectedModelId: '__basic__'`、`hasUserSelectedModel: true`；选择“基础模板”并完整回答“今天主要做了什么？”；不新建对话，切到“演示 AI”并完整回答第二问；断言 `create_assistant_conversation` 仅一次、两次 `chat_work_assistant`、四次 `append_assistant_message` 使用同一 conversationId，且第二次 history 含第一轮 user + assistant。
  - `privacy`：展示三档，选择“仅统计时长”，再关闭“启用截图”；不展示独立 OCR 或全局 AI 开关。
  - `export`：展示安全数据目录；Mock “打开当前目录”；导出 Markdown 并显示安全路径。
  - `outro`：使用真实 `public/icon.png` 和生成的片尾层。

- [ ] **Step 5: 实现分镜录制器**

  - 每个分镜独立 BrowserContext，避免状态泄漏。
  - Playwright `recordVideo` 输出 WebM，片段原始长度可以略长于目标，后续 FFmpeg 精确截断。
  - 录制同时保存开始、动作节点、结束关键帧，用于人工检查和失败定位。
  - 页面错误、控制台 error、未处理 Tauri 命令或外部请求都必须使分镜失败。

- [ ] **Step 6: 增加 package scripts**

  ```json
  {
    "demo:check": "node --test-timeout=60000 --import tsx --test \"scripts/product-demo/*.test.ts\"",
    "demo:capture": "node --import tsx scripts/product-demo/cli.ts capture"
  }
  ```

- [ ] **Step 7: 运行静态检查与录制烟测**

  Run: `npm run demo:check`

  Expected: PASS。

  Run: `npm run check`

  Expected: PASS。

  Run: `node --import tsx scripts/product-demo/cli.ts capture --scene timeline --aspect 16x9`

  Expected: 生成 `timeline.webm` 和关键帧，过程无页面错误、外部请求或未处理命令。

- [ ] **Step 8: 提交**

  ```bash
  git add package.json scripts/product-demo/types.ts scripts/product-demo/browser.ts scripts/product-demo/shots.ts scripts/product-demo/capture.ts scripts/product-demo/cli.ts scripts/product-demo/storyboard.test.ts
  git commit -m "feat(demo): record deterministic product scenes"
  ```

---

### Task 6: 实现旁白、音乐和音效生成

**Files:**
- Create: `scripts/product-demo/audio.ts`
- Create: `scripts/product-demo/ffmpeg.ts`
- Test: `scripts/product-demo/ffmpeg.test.ts`
- Modify: `scripts/product-demo/cli.ts`

**Interfaces:**
- Consumes: `VOICEOVER_CUES`、目标响度和输出目录。
- Produces: `runCommand()`, `probeMedia()`, `buildVoiceover()`, `buildBackgroundMusic()`, `buildSoundEffects()`, `mixDemoAudio()`，并给 `cli.ts` 增加 `audio` 命令。
- Produces: 48kHz stereo WAV 中间音轨和 85 秒最终混音。

- [ ] **Step 1: 写 FFmpeg 参数失败测试**

  断言：

  - 输出固定 `-r 30`、`-pix_fmt yuv420p`、`libx264`、AAC 48kHz。
  - 音频混合包含 固定两遍 `loudnorm=I=-14:TP=-1:LRA=7` 规范化流程。
  - 最终视频命令包含 `-movflags +faststart`。
  - 任何传入路径都作为独立参数传给 `spawn`，不通过 shell 拼接。

- [ ] **Step 2: 运行测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/ffmpeg.test.ts`

  Expected: FAIL，原因是 FFmpeg 模块尚不存在。

- [ ] **Step 3: 实现安全进程与媒体探测封装**

  `runCommand(binary, args)` 固定使用 `spawn` 的参数数组；错误包含退出码和截断后的 stderr。`probeMedia(path)` 使用 JSON 输出解析时长、帧率、帧数、编解码器、像素格式和分辨率。

- [ ] **Step 4: 实现中文旁白**

  - 先检测 `say -v Tingting` 是否可用。
  - 按每条 cue 分句生成 AIFF，再用 FFmpeg 转为 48kHz stereo WAV。
  - 通过 `adelay` 把每句放在固定时间点；最后一句在 83 秒前结束。
  - 若单句超出 cue 时长，脚本必须失败并报告实际长度，不做不可懂的强制高速压缩。

- [ ] **Step 5: 实现无授权风险的程序化音乐和音效**

  - 使用 FFmpeg 正弦波、低通滤波和淡入淡出生成克制的轻电子氛围底乐，不下载第三方音乐。
  - 点击、切换、完成和片尾音效由短包络正弦波生成。
  - 旁白存在时通过预设音量包络把音乐降低 8–12 dB。

- [ ] **Step 6: 混音并验证**

  Run: `node --import tsx scripts/product-demo/cli.ts audio`

  Expected: 生成 85 秒混音 WAV，48kHz stereo，无削波。

  Run: `ffmpeg -hide_banner -i artifacts/product-demo/intermediate/mix.wav -filter_complex ebur128=peak=true -f null -`

  Expected: 综合响度接近 `-14 ±1 LUFS-I`，True Peak 不高于 `-1 dBTP`。

- [ ] **Step 7: 运行测试并确认 GREEN**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/ffmpeg.test.ts`

  Expected: PASS，0 failures。

- [ ] **Step 8: 提交**

  ```bash
  git add scripts/product-demo/audio.ts scripts/product-demo/ffmpeg.ts scripts/product-demo/ffmpeg.test.ts scripts/product-demo/cli.ts
  git commit -m "feat(demo): generate voiceover and licensed-safe audio"
  ```

---

### Task 7: 合成横屏、竖屏、字幕和封面

**Files:**
- Create: `scripts/product-demo/compose.ts`
- Modify: `scripts/product-demo/cli.ts`
- Modify: `package.json`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: 七个分镜 WebM、`STORYBOARD` 构图、透明字幕 PNG、最终混音、`public/icon.png`。
- Produces: 两套 MP4、两张 PNG 封面、三个 SRT、内部合成日志。

- [ ] **Step 1: 在 FFmpeg 测试中写合成失败断言**

  断言横屏和竖屏滤镜链：

  - 每段 `trim/setpts/fps=30`。
  - 横屏统一到 `1920:1080`。
  - 竖屏逐段使用各自的 crop/scale/pad 参数，并统一到 `1080:1920`。
  - 七段 concat 后强制 `trim=duration=85`。
  - 每条透明双语字幕 PNG 在最后缩放完成后通过 `overlay` 的固定启停时间窗叠加。

- [ ] **Step 2: 运行测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/ffmpeg.test.ts`

  Expected: FAIL，原因是合成参数构造器尚不存在。

- [ ] **Step 3: 实现合成器**

  - 为每个分镜根据目标时长选择 `trim` 或最后一帧短暂冻结，不使用全局变速。
  - 使用场景定义中的独立横竖屏构图。
  - 音轨映射最终混音 WAV，输出 AAC-LC。
  - H.264 使用 CRF 17、preset slow、GOP 60、Fast Start。
  - 输出 Rec.709 色彩标记。

- [ ] **Step 4: 实现透明字幕层和封面生成**

  `subtitles.ts` 为每条 cue 和每种画幅生成一个透明 HTML/CSS 页面，并用 Playwright 截成带 alpha 的 PNG；中文在上、英文在下，竖屏安全区高于横屏。封面也由 HTML/CSS + 真实图标渲染，叠加 `Work-Review` 和“回看今天，写下成果。”；横竖屏分别构图。不得依赖当前 FFmpeg 构建缺失的 `subtitles`、`ass` 或 `drawtext` 滤镜。

- [ ] **Step 5: 完成 CLI 与 package scripts**

  ```json
  {
    "demo:compose": "node --import tsx scripts/product-demo/cli.ts compose",
    "demo:validate": "node --import tsx scripts/product-demo/cli.ts validate",
    "demo:build": "node --import tsx scripts/product-demo/cli.ts build"
  }
  ```

  `build` 顺序固定为 `check → capture → subtitles → audio → compose → validate`。

- [ ] **Step 6: 忽略大体积产物**

  在 `.gitignore` 增加：

  ```gitignore
  /artifacts/product-demo/
  ```

  不修改 README。

- [ ] **Step 7: 运行测试并确认 GREEN**

  Run: `npm run demo:check`

  Expected: PASS。

- [ ] **Step 8: 生成两套成片**

  Run: `npm run demo:build`

  Expected: 生成两套 MP4、三个 SRT、两张封面和中间日志。完整构建可以超过 60 秒；内部每个页面等待必须有明确超时，所有 Node 单元测试使用 `--test-timeout=60000`。

- [ ] **Step 9: 提交**

  ```bash
  git add .gitignore package.json scripts/product-demo/compose.ts scripts/product-demo/cli.ts scripts/product-demo/ffmpeg.test.ts
  git commit -m "feat(demo): compose horizontal and vertical videos"
  ```

---

### Task 8: 自动验证媒体、字幕、隐私与交付清单

**Files:**
- Create: `scripts/product-demo/validate.ts`
- Test: `scripts/product-demo/validate.test.ts`
- Modify: `scripts/product-demo/cli.ts`
- Modify: `scripts/product-demo/privacy.ts`
- Modify: `scripts/product-demo/privacy.test.ts`

**Interfaces:**
- Consumes: 目标媒体规格、生成产物、SRT、日志、夹具和虚拟导出 Markdown。
- Produces: `validateMediaArtifact()`, `validateSubtitleArtifact()`, `validateDelivery()`, `writeManifest()`。
- Produces: `manifest.json` 和 `SHA256SUMS`。

- [ ] **Step 1: 写验证器失败测试**

  使用临时 JSON probe fixture 测试以下错误都能被明确报告：时长错误、帧率非 30、帧数非 2550、分辨率错误、缺少 AAC、像素格式错误、SRT 越界、敏感路径命中和文件为空。

- [ ] **Step 2: 运行测试并确认 RED**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/validate.test.ts`

  Expected: FAIL，原因是交付验证函数尚不存在。

- [ ] **Step 3: 实现媒体与字幕验证**

  媒体允许的容器时长误差不超过一帧。若 `nb_frames` 缺失，使用 `ffprobe -count_frames` 获取准确帧数，不能只用时长乘帧率估算。

- [ ] **Step 4: 实现产物隐私扫描**

  扫描：

  - 三个 SRT。
  - `manifest.json`。
  - Playwright 控制台和网络日志。
  - 虚拟导出 Markdown。
  - 关键帧 OCR；若系统没有 OCR 工具，则至少对源码、HTML 文本和文件名执行强制扫描，并把图像 OCR 标记为人工检查项。

- [ ] **Step 5: 写清单和 SHA-256**

  `manifest.json` 记录版本、构建时间、Git commit、分辨率、时长、帧数、编码和文件大小；`SHA256SUMS` 每行记录哈希和相对文件名。

- [ ] **Step 6: 运行验证器测试并确认 GREEN**

  Run: `node --test-timeout=60000 --import tsx --test scripts/product-demo/validate.test.ts scripts/product-demo/privacy.test.ts scripts/product-demo/ffmpeg.test.ts`

  Expected: PASS，0 failures。

- [ ] **Step 7: 运行完整验证**

  Run: `npm run demo:validate`

  Expected: 所有媒体、字幕、隐私和交付文件检查 PASS；失败时进程退出码非 0。

- [ ] **Step 8: 提交**

  ```bash
  git add scripts/product-demo/validate.ts scripts/product-demo/validate.test.ts scripts/product-demo/cli.ts scripts/product-demo/privacy.ts scripts/product-demo/privacy.test.ts
  git commit -m "feat(demo): validate media and delivery artifacts"
  ```

---

### Task 9: 人工视觉检查、代码审查与最终回归

**Files:**
- Modify only if review discovers defects in the files above.

**Interfaces:**
- Consumes: 最终 MP4、封面、关键帧、设计规格和本计划。
- Produces: 已修复且重新验证的最终交付物和清洁提交历史。

- [ ] **Step 1: 抽取人工检查帧**

  从横竖屏成片的 `3s, 12s, 25s, 41s, 58s, 73s, 82s` 抽取 PNG，使用图像查看工具逐张检查：

  - 关键 UI 可读。
  - 竖屏不是简单裁切。
  - 字幕不遮挡模型选择器、隐私三档和导出路径。
  - 无真实用户名、通知、浏览器错误页和加载失败。

- [ ] **Step 2: 完整播放两套视频**

  人工观看 85 秒横屏和 85 秒竖屏，记录任何音画不同步、机械断句、字幕过快、鼠标误点、转场跳帧或 Logo 收束问题。

- [ ] **Step 3: 请求代码审查**

  使用 `requesting-code-review` 工作流，审查从 `417c552` 到当前 HEAD 的实现，重点检查：

  - 是否可能触达真实数据库、真实 AI 或外网。
  - 是否存在 shell 注入、进程泄漏或未关闭 BrowserContext。
  - 是否所有产品能力描述与当前代码一致。
  - 是否遗漏横竖屏、字幕、隐私或媒体参数验收。

- [ ] **Step 4: 修复 Critical/Important 问题并重新生成受影响产物**

  每项修复先增加或调整失败测试，再做最小修改，遵循 RED→GREEN。

- [ ] **Step 5: 运行全量新鲜验证**

  Run: `npm run demo:check`

  Expected: PASS。

  Run: `npm run verify:frontend`

  Expected: `svelte-check`、全部前端测试和 Vite build 均 PASS。

  Run: `npm run demo:validate`

  Expected: 两套媒体、字幕、隐私和交付清单全部 PASS。

- [ ] **Step 6: 核对 Git 范围**

  Run: `git diff --name-only 417c552..HEAD`

  Expected: 只包含 `.gitignore`、`package.json`、`scripts/product-demo/**` 和本实施计划；README 不出现，`artifacts/product-demo/` 不被跟踪，根目录既有零字节临时文件不被纳入提交。

- [ ] **Step 7: 提交最终修复**

  如有修复：

  ```bash
  git add <reviewed-source-files>
  git commit -m "fix(demo): address final video review"
  ```

  如无修复，不创建空提交。

---

## 最终验收命令

```bash
npm run demo:check
npm run verify:frontend
npm run demo:build
npm run demo:validate
git diff --check 417c552..HEAD
git status --short --branch
```

## 最终交付检查表

- [ ] 横屏 `1920×1080` MP4 为 85 秒、30fps、2550 帧。
- [ ] 竖屏 `1080×1920` MP4 为 85 秒、30fps、2550 帧。
- [ ] H.264、AAC、`yuv420p`、Fast Start 和 Rec.709 参数正确。
- [ ] 中文、英文和中英双语 SRT 均存在且时间轴一致。
- [ ] 横屏、竖屏双语硬字幕都位于安全区。
- [ ] 助手基础模板和演示 AI 各一次，且在同一会话。
- [ ] 日报按整日记录生成，并按段落编辑、显式保存。
- [ ] 隐私三档、截图/OCR 联动和本地目录表述准确。
- [ ] 时间线没有多选，界面没有虚构的独立 OCR 或全局 AI 开关。
- [ ] 所有数据和路径均为虚构且通过隐私扫描。
- [ ] 两张封面、`manifest.json` 和 `SHA256SUMS` 齐全。
- [ ] README 未修改；大体积产物未进入 Git。
