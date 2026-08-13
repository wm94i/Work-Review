import test from 'node:test';
import assert from 'node:assert/strict';
import { access, readFile, readdir } from 'node:fs/promises';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const projectFile = (path: string) => new URL(`../${path}`, import.meta.url);
const migratedModules = [
  'dateValidation',
  'browserUrl',
  'popoverPosition',
  'appVisuals',
  'appDisplay',
  'historyPayload',
  'modelPresentation',
  'requestEventGate',
  'starterPromptPresentation',
  'streamEvent',
  'bubbleMessage',
  'errorDisplay',
  'timelineData',
  'reportDateNavigation',
  'reportMeta',
  'reportPromptFeedback',
  'summaryPresentation',
  'reportSections',
  'overviewCategoryPresentation',
  'overviewDomainPresentation',
  'avatarStateMeta',
  'avatarPresetRegistry',
  'avatarToggle',
  'focusTrap',
  'recording',
  'toast',
  'confirm',
  'ai',
  'main',
  'categories',
  'assistant',
  'updater',
  'cache',
  'iconCache',
];

const typedContractModules = [
  'routes/ask/historyPayload',
  'routes/ask/modelPresentation',
  'routes/ask/requestEventGate',
  'routes/ask/starterPromptPresentation',
  'routes/ask/streamEvent',
  'lib/components/Avatar/bubbleMessage',
  'lib/utils/errorDisplay',
  'routes/timeline/timelineData',
  'routes/report/reportDateNavigation',
  'routes/report/reportMeta',
  'routes/report/reportPromptFeedback',
  'routes/timeline/summaryPresentation',
  'routes/report/reportSections',
  'routes/overviewCategoryPresentation',
  'routes/overviewDomainPresentation',
  'lib/components/Avatar/avatarStateMeta',
  'lib/components/Avatar/avatarPresetRegistry',
  'lib/utils/avatarToggle',
  'lib/utils/focusTrap',
  'lib/stores/recording',
  'lib/stores/toast',
  'lib/stores/confirm',
  'lib/stores/ai',
  'lib/stores/categories',
  'lib/stores/assistant',
  'lib/utils/updater',
  'lib/stores/cache',
  'lib/stores/iconCache',
  'lib/i18n/index',
];

const sixthBatchPublicTypeContracts = [
  'AvatarSettingsConfig',
  'AvatarConfigSaver',
  'AvatarToggleUiState',
  'FocusTrapActionResult',
  'RecordingStateInput',
  'RecordingState',
  'RecordingStore',
  'ToastType',
  'ToastState',
  'ToastStore',
  'ConfirmTone',
  'ConfirmOptions',
  'ConfirmDialogState',
  'ConfirmDialogStore',
];

const seventhBatchPublicTypeContracts = [
  'AiTestStatus',
  'AiTextModelConfigInput',
  'AiConfigInput',
  'AiStoreState',
  'AiStore',
];

const eighthBatchPublicTypeContracts = [
  'CategoryInfo',
  'CategoryMeta',
  'CategoryStore',
  'SemanticCategoryInfo',
  'SemanticCategoryStore',
  'AssistantMessageRole',
  'AssistantStepStatus',
  'AssistantConfirmStatus',
  'AssistantReference',
  'AssistantCard',
  'AssistantStep',
  'AssistantMessageInput',
  'AssistantMessage',
  'AssistantState',
  'AssistantMessageUpdater',
  'AssistantModelSelectionOptions',
  'AssistantStore',
];

const ninthBatchPublicTypeContracts = [
  'GithubUpdateInfo',
  'GithubUpdateInstallResult',
  'GithubUpdateStatusPayload',
  'RunUpdateFlowOptions',
  'RunUpdateFlowResult',
  'RunUpdateFlow',
  'UpdateFlowDependencies',
];

const tenthBatchPublicTypeContracts = [
  'CacheActivity',
  'CacheEntry',
  'OverviewCacheEntry',
  'TimelineCacheEntry',
  'CacheState',
  'CacheInvalidationType',
  'CacheValidityKey',
  'CacheStore',
];

const eleventhBatchPublicTypeContracts = [
  'AppIconCacheValue',
  'AppIconCacheState',
  'AppIconRequestEntry',
  'AppIconRequest',
  'AppIconLoadOptions',
  'GetAppIconArgs',
  'AppIconInvoke',
  'AppIconStore',
];

const twelfthBatchPublicTypeContracts = [
  'Locale',
  'TranslationValue',
  'TranslationDictionary',
  'InterpolationParams',
  'DurationFormatOptions',
];

const i18nModules = [
  'src/lib/i18n/index',
  'src/lib/i18n/locales/zh-CN',
  'src/lib/i18n/locales/en',
  'src/lib/i18n/locales/zh-TW',
  'src/lib/i18n/locales/ar',
];

async function assertMigratedToTypeScript(relativePath: string) {
  await access(projectFile(`${relativePath}.ts`));
  await assert.rejects(access(projectFile(`${relativePath}.js`)));
}

async function collectTestFiles(relativeDirectory: string) {
  const directory = fileURLToPath(projectFile(relativeDirectory));
  return (await readdir(directory, { recursive: true }))
    .filter((path) => /\.test\.(?:js|mjs|ts)$/.test(path))
    .map((path) => join(relativeDirectory, path));
}

async function collectSvelteFiles(relativeDirectory: string) {
  const directory = fileURLToPath(projectFile(relativeDirectory));
  return (await readdir(directory, { recursive: true }))
    .filter((path) => path.endsWith('.svelte'))
    .map((path) => join(relativeDirectory, path));
}

test('前端脚本应提供类型检查和统一验证入口', async () => {
  const packageJson = JSON.parse(await readFile(projectFile('package.json'), 'utf8'));

  assert.equal(packageJson.scripts.check, 'svelte-check --tsconfig ./tsconfig.json --threshold error');
  assert.equal(
    packageJson.scripts['test:frontend'],
    'node --import tsx --test "**/*.test.ts"'
  );
  assert.equal(
    packageJson.scripts['verify:frontend'],
    'npm run check && npm run test:frontend && npm run build'
  );
});

test('全部第一方前端测试应迁移到 TypeScript', async () => {
  const projectRoot = fileURLToPath(projectFile('.'));
  const rootTests = (await readdir(projectRoot))
    .filter((path) => /\.test\.(?:js|mjs|ts)$/.test(path));
  const nestedTests = (
    await Promise.all(['src', 'src-tauri', 'scripts'].map(collectTestFiles))
  ).flat();
  const testFiles = [...rootTests, ...nestedTests].sort();
  const legacyTests = testFiles.filter((path) => /\.test\.(?:js|mjs)$/.test(path));
  const typeScriptTests = testFiles.filter((path) => path.endsWith('.test.ts'));

  assert.deepEqual(legacyTests, []);
  assert.ok(typeScriptTests.length >= 115, `TypeScript 测试数量意外回退：${typeScriptTests.length}`);
  assert.ok(
    typeScriptTests.includes(join('scripts', 'check-linux-glibc.test.ts')),
    'GLIBC 门禁测试应作为第一方 TypeScript 测试存在',
  );
});

test('全部第一方 Node 工具脚本应迁移到 TypeScript', async () => {
  for (const script of [
    'build-icons',
    'capture-readme-pages',
    'capture-star-history',
    'fetch-provider-icons',
  ]) {
    await access(projectFile(`scripts/${script}.ts`));
    await assert.rejects(access(projectFile(`scripts/${script}.mjs`)));
  }
});

test('前端工具配置应迁移到 TypeScript，并保留 Svelte 工具链兼容入口', async () => {
  await Promise.all([
    assertMigratedToTypeScript('vite.config'),
    assertMigratedToTypeScript('tailwind.config'),
  ]);
  await access(projectFile('svelte.config.js'));
  await assert.rejects(access(projectFile('svelte.config.ts')));
  for (const extension of ['js', 'mjs', 'cjs', 'ts']) {
    await assert.rejects(access(projectFile(`postcss.config.${extension}`)));
  }

  const svelteConfig = await readFile(projectFile('svelte.config.js'), 'utf8');
  const viteConfig = await readFile(projectFile('vite.config.ts'), 'utf8');
  assert.match(svelteConfig, /vitePreprocess/);
  assert.match(svelteConfig, /preprocess:\s*vitePreprocess\(\)/);
  assert.match(viteConfig, /plugins:\s*\[svelte\(\)\]/);
  assert.doesNotMatch(viteConfig, /vitePreprocess/);
  assert.match(viteConfig, /plugins:\s*\[tailwindcss\(\),\s*autoprefixer\(\)\]/);
});

test('Svelte 脚本不应包含会破坏 Vite 开发依赖扫描的原始 HTML 注释起始符', async () => {
  const svelteFiles = await collectSvelteFiles('src');
  const offenders: string[] = [];

  await Promise.all(svelteFiles.map(async (path) => {
    const source = await readFile(projectFile(path), 'utf8');
    const scripts = source.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/g);
    if ([...scripts].some((match) => match[1].includes('<!--'))) {
      offenders.push(path);
    }
  }));

  assert.deepEqual(offenders.sort(), []);
});

test('TypeScript 配置应启用严格检查并覆盖全部第一方脚本', async () => {
  const tsconfig = JSON.parse(await readFile(projectFile('tsconfig.json'), 'utf8'));
  const viteTypes = await readFile(projectFile('src/vite-env.d.ts'), 'utf8');

  assert.equal(tsconfig.extends, '@tsconfig/svelte/tsconfig.json');
  assert.equal(tsconfig.compilerOptions.strict, true);
  assert.equal(tsconfig.compilerOptions.noEmit, true);
  assert.equal(tsconfig.compilerOptions.allowJs, false);
  assert.equal(tsconfig.compilerOptions.checkJs, false);
  assert.equal(tsconfig.compilerOptions.allowImportingTsExtensions, true);
  assert.match(viteTypes, /^\/\/\/ <reference types="vite\/client" \/>$/m);
});

test('全部第一方 Svelte 组件脚本应启用 TypeScript', async () => {
  const svelteFiles = (await collectSvelteFiles('src')).sort();
  const untypedComponents: string[] = [];

  for (const file of svelteFiles) {
    const source = await readFile(projectFile(file), 'utf8');
    if (!/<script\s+lang=["']ts["'][^>]*>/.test(source)) {
      untypedComponents.push(file);
    }
  }

  assert.equal(svelteFiles.length, 38);
  assert.deepEqual(untypedComponents, []);
});

test('日期校验工具应迁移到 TypeScript 且不保留 JavaScript 副本', async () => {
  await assertMigratedToTypeScript('src/lib/utils/dateValidation');
});

test('浏览器 URL 展示工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/utils/browserUrl');
});

test('Popover 定位工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/utils/popoverPosition');
});

test('应用图标展示工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/utils/appVisuals');
});

test('时间线应用名展示工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/utils/appDisplay');
});

test('Ask 历史载荷工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/ask/historyPayload');
});

test('Ask 模型标签工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/ask/modelPresentation');
});

test('Ask 请求事件门闩应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/ask/requestEventGate');
});

test('Ask 欢迎问题工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/ask/starterPromptPresentation');
});

test('Ask 流式事件归约工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/ask/streamEvent');
});

test('桌宠气泡文案工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/components/Avatar/bubbleMessage');
});

test('共享错误展示工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/utils/errorDisplay');
});

test('时间线数据工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/timeline/timelineData');
});

test('日报日期导航工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/report/reportDateNavigation');
});

test('日报元数据工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/report/reportMeta');
});

test('日报提示反馈工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/report/reportPromptFeedback');
});

test('时间线摘要展示工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/timeline/summaryPresentation');
});

test('日报段落工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/report/reportSections');
});

test('概览分类展示工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/overviewCategoryPresentation');
});

test('概览域名展示工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/routes/overviewDomainPresentation');
});

test('桌宠状态元数据应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/components/Avatar/avatarStateMeta');
});

test('桌宠预设注册表应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/components/Avatar/avatarPresetRegistry');
});

test('桌宠设置工具应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/utils/avatarToggle');
});

test('弹窗焦点陷阱应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/utils/focusTrap');
});

test('录制状态 Store 应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/stores/recording');
});

test('Toast Store 应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/stores/toast');
});

test('确认弹窗 Store 应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/stores/confirm');
});

test('AI 连接状态 Store 应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/stores/ai');
  await access(projectFile('src/lib/stores/ai.test.ts'));
});

test('分类 Store 应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/stores/categories');
  await access(projectFile('src/lib/stores/categories.test.ts'));
});

test('助手 Store 应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/stores/assistant');
  await access(projectFile('src/AssistantStreaming.test.ts'));
});

test('更新流程应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/utils/updater');
  await access(projectFile('src/lib/utils/updater.test.ts'));
});

test('共享页面缓存 Store 应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/stores/cache');
  await access(projectFile('src/lib/stores/cache.test.ts'));
});

test('应用图标缓存与并发调度器应迁移到 TypeScript', async () => {
  await assertMigratedToTypeScript('src/lib/stores/iconCache');
  await access(projectFile('src/lib/stores/iconCache.test.ts'));
});

test('i18n 入口与四语言词典应原子迁移到 TypeScript', async () => {
  await Promise.all(i18nModules.map(assertMigratedToTypeScript));
  await access(projectFile('src/lib/i18n/index.test.ts'));
});

test('前端入口应迁移到 TypeScript 并由 HTML 加载新入口', async () => {
  await assertMigratedToTypeScript('src/main');

  const html = await readFile(projectFile('index.html'), 'utf8');
  assert.match(html, /src="\/src\/main\.ts"/);
  assert.doesNotMatch(html, /src="\/src\/main\.js"/);
});

test('已迁移公共类型应由 TypeScript 编译期契约约束', async () => {
  const contract = await readFile(projectFile('src/TypeScriptMigration.contract.ts'), 'utf8');

  for (const modulePath of typedContractModules) {
    assert.match(contract, new RegExp(`${modulePath}\\.ts`));
  }
});

test('第六批全部公共类型应由独立结构契约约束', async () => {
  const contract = await readFile(projectFile('src/TypeScriptMigration.contract.ts'), 'utf8');

  for (const typeName of sixthBatchPublicTypeContracts) {
    assert.match(
      contract,
      new RegExp(`type ${typeName}Contract\\s*=\\s*Expect<Equal<`),
      `${typeName} 缺少独立的编译期结构契约`
    );
  }
});

test('第七批全部公共类型应由独立结构契约约束', async () => {
  const contract = await readFile(projectFile('src/TypeScriptMigration.contract.ts'), 'utf8');

  for (const typeName of seventhBatchPublicTypeContracts) {
    assert.match(
      contract,
      new RegExp(`type ${typeName}Contract\\s*=\\s*Expect<Equal<`),
      `${typeName} 缺少独立的编译期结构契约`
    );
  }

  assert.match(
    contract,
    /type AiStoreValueContract\s*=\s*Expect<Equal</,
    'aiStore 缺少独立的编译期结构契约'
  );
});

test('第八批全部公共类型与公共值应由独立结构契约约束', async () => {
  const contract = await readFile(projectFile('src/TypeScriptMigration.contract.ts'), 'utf8');

  for (const typeName of eighthBatchPublicTypeContracts) {
    assert.match(
      contract,
      new RegExp(`type ${typeName}Contract\\s*=\\s*Expect<Equal<`),
      `${typeName} 缺少独立的编译期结构契约`
    );
  }

  for (const valueContract of [
    'CategoryStoreValueContract',
    'SemanticCategoryStoreValueContract',
    'BasicAssistantModelIdValueContract',
    'AssistantStoreValueContract',
  ]) {
    assert.match(
      contract,
      new RegExp(`type ${valueContract}\\s*=\\s*Expect<Equal<`),
      `${valueContract} 缺少独立的编译期值契约`
    );
  }

  assert.match(
    contract,
    /type AssistantStreamCompatibilityContract\s*=\s*Expect</,
    'AssistantMessage 缺少流式归约器兼容性契约'
  );
});

test('第九批全部公共类型与公共函数应由独立结构契约约束', async () => {
  const contract = await readFile(projectFile('src/TypeScriptMigration.contract.ts'), 'utf8');

  for (const typeName of ninthBatchPublicTypeContracts) {
    assert.match(
      contract,
      new RegExp(`type ${typeName}Contract\\s*=\\s*Expect<Equal<`),
      `${typeName} 缺少独立的编译期结构契约`
    );
  }

  for (const functionContract of [
    'CreateUpdateFlowContract',
    'RunUpdateFlowValueContract',
  ]) {
    assert.match(
      contract,
      new RegExp(`type ${functionContract}\\s*=\\s*Expect<Equal<`),
      `${functionContract} 缺少独立的编译期函数契约`
    );
  }
});

test('第十批缓存公共类型与公共值应由独立结构契约约束', async () => {
  const contract = await readFile(projectFile('src/TypeScriptMigration.contract.ts'), 'utf8');

  for (const typeName of tenthBatchPublicTypeContracts) {
    assert.match(
      contract,
      new RegExp(`type ${typeName}Contract\\s*=\\s*Expect<Equal<`),
      `${typeName} 缺少独立的编译期结构契约`
    );
  }

  for (const valueContract of ['CacheStoreValueContract', 'GetLocalDateContract']) {
    assert.match(
      contract,
      new RegExp(`type ${valueContract}\\s*=\\s*Expect<Equal<`),
      `${valueContract} 缺少独立的编译期值契约`
    );
  }
});

test('第十一批图标缓存公共类型与函数应由独立结构契约约束', async () => {
  const contract = await readFile(projectFile('src/TypeScriptMigration.contract.ts'), 'utf8');

  for (const typeName of eleventhBatchPublicTypeContracts) {
    assert.match(
      contract,
      new RegExp(`type ${typeName}Contract\\s*=\\s*Expect<Equal<`),
      `${typeName} 缺少独立的编译期结构契约`
    );
  }

  for (const valueContract of [
    'AppIconStoreValueContract',
    'GetIconCacheKeyContract',
    'LoadAppIconContract',
    'PreloadAppIconsContract',
  ]) {
    assert.match(
      contract,
      new RegExp(`type ${valueContract}\\s*=\\s*Expect<Equal<`),
      `${valueContract} 缺少独立的编译期值契约`
    );
  }
});

test('第十二批 i18n 公共类型与函数应由独立结构契约约束', async () => {
  const contract = await readFile(projectFile('src/TypeScriptMigration.contract.ts'), 'utf8');

  for (const typeName of twelfthBatchPublicTypeContracts) {
    assert.match(
      contract,
      new RegExp(`type ${typeName}Contract\\s*=\\s*Expect<Equal<`),
      `${typeName} 缺少独立的编译期结构契约`
    );
  }

  for (const valueContract of [
    'SupportedLocalesContract',
    'LocaleStoreValueContract',
    'InitializeLocaleContract',
    'SetLocaleContract',
    'CycleLocaleContract',
    'GetLocaleShortLabelContract',
    'GetLocaleLabelContract',
    'ApplyLocaleToDocumentContract',
    'TranslateContract',
    'TranslateMessagesContract',
    'FormatLocalizedDateContract',
    'FormatLocalizedTimeContract',
    'FormatDurationLocalizedContract',
    'TranslateCategoryLabelContract',
    'TranslateSemanticCategoryLabelContract',
  ]) {
    assert.match(
      contract,
      new RegExp(`type ${valueContract}\\s*=\\s*Expect<Equal<`),
      `${valueContract} 缺少独立的编译期值契约`
    );
  }
});

test('已迁移工具模块不应保留 JavaScript 扩展名引用', async () => {
  const sourceRoot = fileURLToPath(projectFile('src/'));
  const sourcePaths = (await readdir(sourceRoot, { recursive: true }))
    .filter((path) => ['.js', '.ts', '.svelte'].includes(extname(path)));

  for (const sourcePath of sourcePaths) {
    const source = await readFile(join(sourceRoot, sourcePath), 'utf8');

    for (const moduleName of migratedModules) {
      assert.doesNotMatch(
        source,
        new RegExp(`${moduleName}\\.js`),
        `${sourcePath} 仍在引用已迁移模块 ${moduleName}.js`
      );
    }
  }
});

test('CI 和 Release 应执行类型检查及统一前端测试', async () => {
  const ci = await readFile(projectFile('.github/workflows/ci.yml'), 'utf8');
  const release = await readFile(projectFile('.github/workflows/release.yml'), 'utf8');

  for (const workflow of [ci, release]) {
    assert.match(workflow, /run:\s*npm run check/);
    assert.match(workflow, /run:\s*npm run test:frontend/);
  }
});

test('本地过程文件与可再生成产物应被 Git 忽略', async () => {
  const gitignore = await readFile(projectFile('.gitignore'), 'utf8');

  assert.match(gitignore, /^\/docs\/superpowers\/plans\/$/m);
  assert.match(gitignore, /^\/docs\/superpowers\/specs\/$/m);
  assert.match(gitignore, /^\/lib\/$/m);
  assert.match(gitignore, /^\/learning\/$/m);
  assert.match(gitignore, /^\/src-tauri\/gen\/schemas\/$/m);
  assert.match(gitignore, /^\/all-artifacts\/$/m);
  assert.match(gitignore, /^\/release_notes\.md$/m);
  assert.match(gitignore, /^\/updater\.json$/m);
  assert.match(gitignore, /^\/updater-ghproxy\.json$/m);
  assert.match(gitignore, /^\/updater-ghp\.json$/m);
  assert.match(gitignore, /^\*\.tsbuildinfo$/m);
  assert.match(gitignore, /^\/plan\.md$/m);
});
