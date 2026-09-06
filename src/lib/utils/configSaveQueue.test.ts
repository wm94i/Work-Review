import test from 'node:test';
import assert from 'node:assert/strict';
import {
  createConfigMutationQueue,
  createConfigSaveQueue,
  createConfigUpdateQueue,
} from './configSaveQueue.ts';

test('配置变更队列应让不同类型的配置写入共享同一串行顺序', async () => {
  const started: string[] = [];
  const resolvers: Array<() => void> = [];
  const enqueueMutation = createConfigMutationQueue();

  const categoryMutation = enqueueMutation(async () => {
    started.push('category');
    await new Promise<void>((resolve) => resolvers.push(resolve));
    return 3;
  });
  const privacyMutation = enqueueMutation(async () => {
    started.push('privacy');
    return 'saved';
  });

  await Promise.resolve();
  assert.deepEqual(started, ['category']);

  resolvers.shift()?.();
  assert.equal(await categoryMutation, 3);
  assert.equal(await privacyMutation, 'saved');
  assert.deepEqual(started, ['category', 'privacy']);
});

test('配置保存队列应严格按提交顺序执行', async () => {
  const started: string[] = [];
  const resolvers: Array<() => void> = [];
  const enqueue = createConfigSaveQueue(async (config: { name: string }) => {
    started.push(config.name);
    await new Promise<void>((resolve) => resolvers.push(resolve));
  });

  const first = enqueue({ name: 'first' });
  const second = enqueue({ name: 'second' });

  await Promise.resolve();
  assert.deepEqual(started, ['first']);

  resolvers.shift()?.();
  await first;
  await Promise.resolve();
  assert.deepEqual(started, ['first', 'second']);

  resolvers.shift()?.();
  await second;
});

test('配置保存队列应在入队时创建深快照', async () => {
  const saved: Array<{ template: string; nested: { enabled: boolean } }> = [];
  const enqueue = createConfigSaveQueue<{ template: string; nested: { enabled: boolean } }>(async (config) => {
    saved.push(config);
  });
  const config = {
    template: 'classic',
    nested: { enabled: false },
  };

  const pending = enqueue(config);
  config.template = 'evidence-star-map';
  config.nested.enabled = true;
  await pending;

  assert.deepEqual(saved, [{ template: 'classic', nested: { enabled: false } }]);
});

test('单次保存失败后，队列仍应继续处理后续配置', async () => {
  const saved: string[] = [];
  let shouldFail = true;
  const enqueue = createConfigSaveQueue(async (config: { name: string }) => {
    if (shouldFail) {
      shouldFail = false;
      throw new Error('save failed');
    }
    saved.push(config.name);
  });

  await assert.rejects(enqueue({ name: 'first' }), /save failed/);
  await enqueue({ name: 'second' });

  assert.deepEqual(saved, ['second']);
});

test('不同类型的配置变更失败后也不应阻断后续任务', async () => {
  const enqueueMutation = createConfigMutationQueue();

  await assert.rejects(
    enqueueMutation(async () => {
      throw new Error('category failed');
    }),
    /category failed/,
  );

  assert.equal(await enqueueMutation(async () => 'privacy saved'), 'privacy saved');
});

test('局部配置更新必须在真正执行时读取最新配置，不能用排队前的旧快照覆盖其他字段', async () => {
  type Config = { theme: string; ui_visual_style: string };
  let persisted: Config = { theme: 'system', ui_visual_style: 'c' };
  const enqueueMutation = createConfigMutationQueue();
  let releaseFirstSave: (() => void) | undefined;

  const writeConfig = async (config: Config) => {
    if (config.ui_visual_style === 'a' && config.theme === 'system') {
      await new Promise<void>((resolve) => {
        releaseFirstSave = resolve;
      });
    }
    persisted = structuredClone(config);
  };

  const saveConfig = createConfigSaveQueue(writeConfig, enqueueMutation);
  const updateConfig = createConfigUpdateQueue(
    async () => structuredClone(persisted),
    writeConfig,
    enqueueMutation,
  );

  const styleSave = saveConfig({ theme: 'system', ui_visual_style: 'a' });
  const themeSave = updateConfig((latest) => {
    latest.theme = 'dark';
  });

  await Promise.resolve();
  releaseFirstSave?.();
  await Promise.all([styleSave, themeSave]);

  assert.deepEqual(persisted, { theme: 'dark', ui_visual_style: 'a' });
});
