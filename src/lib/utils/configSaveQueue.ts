import { invoke } from '@tauri-apps/api/core';

export type ConfigSaveExecutor<T> = (config: T) => Promise<void>;
export type ConfigMutation<T> = () => Promise<T>;
export type ConfigReader<T> = () => Promise<T>;
export type ConfigUpdate<T> = (config: T) => void | T | Promise<void | T>;

/**
 * 让所有会持久化配置的前端操作共享同一条严格串行队列。
 * 单个任务失败只会返回给调用方，不会阻断后续配置操作。
 */
export function createConfigMutationQueue() {
  let queueTail: Promise<void> = Promise.resolve();

  return <T>(mutation: ConfigMutation<T>): Promise<T> => {
    const task = queueTail.then(mutation);
    queueTail = task.then(() => undefined, () => undefined);
    return task;
  };
}

/**
 * 为完整配置写入建立严格串行队列，并在入队时创建快照。
 * 这样较早的慢请求不会在较新的用户操作之后完成并覆盖新配置。
 */
export function createConfigSaveQueue<T>(
  executor: ConfigSaveExecutor<T>,
  enqueueMutation = createConfigMutationQueue(),
) {
  return (config: T): Promise<void> => {
    const snapshot = structuredClone(config);
    return enqueueMutation(() => executor(snapshot));
  };
}

/**
 * 把局部配置更新放进共享队列，并在任务真正开始时读取最新持久化配置。
 * 这样排队前读取到的旧快照不会覆盖前序任务刚保存的其他字段。
 */
export function createConfigUpdateQueue<T>(
  reader: ConfigReader<T>,
  executor: ConfigSaveExecutor<T>,
  enqueueMutation = createConfigMutationQueue(),
) {
  return (update: ConfigUpdate<T>): Promise<T> => enqueueMutation(async () => {
    const current = await reader();
    const draft = structuredClone(current);
    const updated = await update(draft);
    const next = updated === undefined ? draft : updated;
    const snapshot = structuredClone(next);
    await executor(snapshot);
    return snapshot;
  });
}

export const runConfigMutationQueued = createConfigMutationQueue();

export const saveConfigQueued = createConfigSaveQueue<unknown>(async (config) => {
  await invoke<void>('save_config', { config });
}, runConfigMutationQueued);

/**
 * 在共享队列真正执行任务时读取最新配置，再应用局部更新并保存。
 * 避免入队前读取的旧完整快照覆盖更早已成功持久化的其他字段。
 */
export function updateConfigQueued<T>(
  update: ConfigUpdate<T>,
): Promise<T> {
  return createConfigUpdateQueue<T>(
    () => invoke<T>('get_config'),
    (config) => invoke<void>('save_config', { config }),
    runConfigMutationQueued,
  )(update);
}
