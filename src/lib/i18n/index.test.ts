import test from 'node:test';
import assert from 'node:assert/strict';
import { get } from 'svelte/store';
import type {
  Locale,
  TranslationDictionary,
} from './index.ts';

type I18nModule = typeof import('./index.ts');
type EnvironmentName = 'window' | 'navigator' | 'document';
type TestEnvironment = Partial<Record<EnvironmentName, unknown>>;
type LeafSignature = [path: string, type: string, placeholders: string[]];

interface TestStorage extends Storage {
  writes: Array<[key: string, value: string]>;
}

let importSequence = 0;

async function importFresh(label: string): Promise<I18nModule> {
  importSequence += 1;
  const moduleUrl = new URL(
    `./index.ts?${label}=${Date.now()}-${importSequence}`,
    import.meta.url,
  );
  return import(moduleUrl.href);
}

async function withEnvironment<T>(
  environment: TestEnvironment,
  callback: () => Promise<T>,
): Promise<T> {
  const names: readonly EnvironmentName[] = ['window', 'navigator', 'document'];
  const descriptors = new Map(
    names.map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]),
  );

  try {
    for (const name of names) {
      if (Object.prototype.hasOwnProperty.call(environment, name)) {
        Object.defineProperty(globalThis, name, {
          configurable: true,
          writable: true,
          value: environment[name],
        });
      } else {
        Reflect.deleteProperty(globalThis, name);
      }
    }
    return await callback();
  } finally {
    for (const name of names) {
      const descriptor = descriptors.get(name);
      if (descriptor) {
        Object.defineProperty(globalThis, name, descriptor);
      } else {
        Reflect.deleteProperty(globalThis, name);
      }
    }
  }
}

function createStorage(initialValue: string | null = null): TestStorage {
  let value = initialValue;
  const writes: Array<[key: string, value: string]> = [];
  return {
    writes,
    get length() {
      return value === null ? 0 : 1;
    },
    clear: () => {
      value = null;
    },
    getItem: () => value,
    key: (index) => (index === 0 && value !== null ? 'work-review.locale' : null),
    removeItem: () => {
      value = null;
    },
    setItem: (key: string, nextValue: string) => {
      writes.push([key, nextValue]);
      value = nextValue;
    },
  };
}

test('初始化语言应按显式值、持久化值、浏览器语言和默认值排序', async () => {
  const cases = [
    {
      preferred: 'en-US',
      stored: 'ar',
      browserLanguages: ['zh-HK'],
      expected: 'en',
    },
    {
      preferred: null,
      stored: 'ar-EG',
      browserLanguages: ['zh-HK'],
      expected: 'ar',
    },
    {
      preferred: '',
      stored: 'ar',
      browserLanguages: ['en-US'],
      expected: 'ar',
    },
    {
      preferred: null,
      stored: null,
      browserLanguages: ['zh-HK'],
      expected: 'zh-TW',
    },
    {
      preferred: null,
      stored: null,
      browserLanguages: ['fr-FR', 'en-US'],
      expected: 'zh-CN',
    },
  ];

  for (const [index, testCase] of cases.entries()) {
    const storage = createStorage(testCase.stored);
    await withEnvironment({
      window: { localStorage: storage },
      navigator: {
        languages: testCase.browserLanguages,
        language: testCase.browserLanguages[0],
      },
    }, async () => {
      const i18n = await importFresh(`initialize-${index}`);
      assert.equal(i18n.initializeLocale(testCase.preferred), testCase.expected);
      assert.equal(get(i18n.locale), testCase.expected);
      assert.deepEqual(storage.writes.at(-1), [
        'work-review.locale',
        testCase.expected,
      ]);
    });
  }

  await withEnvironment({}, async () => {
    const i18n = await importFresh('initialize-default');
    assert.equal(i18n.initializeLocale(), 'zh-CN');
  });
});

test('语言设置、循环和文档方向应使用规范化后的 locale', async () => {
  const storage = createStorage();
  const documentElement = { lang: '', dir: '' };

  await withEnvironment({
    window: { localStorage: storage },
    document: { documentElement },
  }, async () => {
    const i18n = await importFresh('set-cycle-document');

    assert.equal(i18n.setLocale('zh-HK'), 'zh-TW');
    assert.equal(i18n.cycleLocale(), 'ar');
    assert.equal(i18n.cycleLocale(), 'zh-CN');
    assert.equal(i18n.setLocale('ar-EG'), 'ar');
    i18n.applyLocaleToDocument();
    assert.deepEqual(documentElement, { lang: 'ar', dir: 'rtl' });

    i18n.applyLocaleToDocument('en-US');
    assert.deepEqual(documentElement, { lang: 'en', dir: 'ltr' });
    assert.equal(i18n.getLocaleShortLabel('zh-HK'), 'TW');
    assert.equal(i18n.getLocaleLabel('en-US'), 'English');
  });
});

test('SSR 与 localStorage 读写异常不应中断语言初始化', async () => {
  await withEnvironment({}, async () => {
    const i18n = await importFresh('ssr');
    assert.equal(i18n.initializeLocale(), 'zh-CN');
    assert.doesNotThrow(() => i18n.applyLocaleToDocument('ar'));
  });

  const failingStorage: Storage = {
    length: 0,
    clear: () => {},
    getItem: () => {
      throw new Error('read failed');
    },
    key: () => null,
    removeItem: () => {},
    setItem: () => {
      throw new Error('write failed');
    },
  };

  await withEnvironment({
    window: {
      localStorage: failingStorage,
    },
    navigator: { languages: ['en-US'], language: 'en-US' },
  }, async () => {
    const i18n = await importFresh('storage-errors');
    assert.equal(i18n.initializeLocale(), 'en');
    assert.equal(i18n.setLocale('ar'), 'ar');
  });
});

test('翻译函数应支持回退、插值、数组消息和缺失键', async () => {
  await withEnvironment({}, async () => {
    const i18n = await importFresh('translations');
    const { default: english } = await import('./locales/en.ts');
    const originalExpandAll = english.common.expandAll;

    try {
      i18n.setLocale('en');
      assert.equal(i18n.t('common.expandAll', { count: 3 }), 'Show all 3');
      assert.equal(
        i18n.t('common.expandAll'),
        'Show all {count}',
        '未提供参数时应保留占位符',
      );
      assert.equal(i18n.t('missing.deep.key'), 'missing.deep.key');
      assert.equal(i18n.t('ask.starterPrompts'), 'ask.starterPrompts');
      assert.ok(Array.isArray(i18n.tm('ask.starterPrompts')));
      assert.equal(
        i18n.tm('ask.starterPrompts.0'),
        english.ask.starterPrompts[0],
      );
      assert.equal(
        i18n.t('ask.starterPrompts.0'),
        english.ask.starterPrompts[0],
      );
      assert.equal(typeof i18n.tm('report.blockNames.CATEGORY_TABLE'), 'string');
      assert.equal(i18n.tm('missing.deep.key'), undefined);

      english.common.expandAll = '{first}';
      assert.equal(
        i18n.t('common.expandAll', { first: '{second}', second: 'done' }),
        'done',
      );

      Reflect.deleteProperty(english.common, 'expandAll');
      assert.equal(i18n.t('common.expandAll', { count: 2 }), '展开全部 2 条');
    } finally {
      english.common.expandAll = originalExpandAll;
    }
  });
});

test('日期、时间和时长格式应跟随当前 locale', async () => {
  await withEnvironment({}, async () => {
    const i18n = await importFresh('formatting');
    const date = new Date('2024-01-02T03:04:05Z');
    const dateOptions: Intl.DateTimeFormatOptions = {
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      timeZone: 'UTC',
    };
    const timeOptions: Intl.DateTimeFormatOptions = {
      hour: '2-digit',
      minute: '2-digit',
      timeZone: 'UTC',
    };

    i18n.setLocale('en');
    assert.equal(
      i18n.formatLocalizedDate(date, dateOptions),
      new Intl.DateTimeFormat('en', dateOptions).format(date),
    );
    assert.equal(
      i18n.formatLocalizedTime(date, timeOptions),
      new Intl.DateTimeFormat('en', timeOptions).format(date),
    );
    const durationCases: Array<{ locale: Locale; expected: string[] }> = [
      {
        locale: 'zh-CN',
        expected: ['0分钟', '0分', '45秒', '2分钟', '1小时', '1时1分'],
      },
      {
        locale: 'en',
        expected: ['0s', '0m', '45s', '2m', '1h', '1h1m'],
      },
      {
        locale: 'zh-TW',
        expected: ['0分鐘', '0分', '45秒', '2分鐘', '1小時', '1時1分'],
      },
      {
        locale: 'ar',
        expected: ['0 ثانية', '0د', '45 ثانية', '2 دقيقة', '1 ساعة', '1س 1د'],
      },
    ];

    for (const durationCase of durationCases) {
      i18n.setLocale(durationCase.locale);
      assert.deepEqual([
        i18n.formatDurationLocalized(0),
        i18n.formatDurationLocalized(0, { compact: true }),
        i18n.formatDurationLocalized(45),
        i18n.formatDurationLocalized(120),
        i18n.formatDurationLocalized(3_600),
        i18n.formatDurationLocalized(3_660, { compact: true }),
      ], durationCase.expected, durationCase.locale);
    }
  });
});

test('分类翻译应命中当前语言并对未知值原样回退', async () => {
  await withEnvironment({}, async () => {
    const i18n = await importFresh('categories');
    i18n.setLocale('en');

    assert.equal(i18n.translateCategoryLabel('development'), 'Development');
    assert.equal(i18n.translateCategoryLabel('custom'), 'custom');
    assert.equal(i18n.translateSemanticCategoryLabel('编码开发'), 'Development');
    assert.equal(i18n.translateSemanticCategoryLabel('自定义活动'), '自定义活动');
  });
});

function collectLeafSignatures(
  value: TranslationDictionary,
  prefix = '',
  output: LeafSignature[] = [],
): LeafSignature[] {
  for (const [key, child] of Object.entries(value)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (child && typeof child === 'object' && !Array.isArray(child)) {
      collectLeafSignatures(child, path, output);
      continue;
    }

    const strings: string[] = Array.isArray(child) ? child : [child];
    const placeholders = [...new Set(
      strings.flatMap((item) => (
        typeof item === 'string'
          ? [...item.matchAll(/\{([^{}]+)\}/g)]
              .map((match) => match[1])
              .filter((placeholder): placeholder is string => placeholder !== undefined)
          : []
      )),
    )].sort();
    output.push([
      path,
      Array.isArray(child) ? 'string[]' : typeof child,
      placeholders,
    ]);
  }
  return output;
}

test('四语言词典的叶路径、类型与占位符应完全一致', async () => {
  const localeNames = ['zh-CN', 'en', 'zh-TW', 'ar'] as const;
  const dictionaries: TranslationDictionary[] = await Promise.all(
    localeNames.map(async (localeName) => (
      (await import(`./locales/${localeName}.ts`)).default
    )),
  );
  const byPath = (left: LeafSignature, right: LeafSignature) => (
    left[0].localeCompare(right[0])
  );
  const firstDictionary = dictionaries[0];
  assert.ok(firstDictionary);
  const expected = collectLeafSignatures(firstDictionary).sort(byPath);

  assert.equal(expected.length, 1_330);
  for (let index = 1; index < dictionaries.length; index += 1) {
    const dictionary = dictionaries[index];
    assert.ok(dictionary);
    assert.deepEqual(
      collectLeafSignatures(dictionary).sort(byPath),
      expected,
      `${localeNames[index]} 的词典结构或占位符与 zh-CN 不一致`,
    );
  }
});
