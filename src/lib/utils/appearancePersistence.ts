export interface PersistedConfigSnapshot<TConfig> {
  recordSuccessfulSave(config: TConfig): void;
  read(): TConfig;
}

/**
 * 只把本次交互负责的字段合并到最新配置，避免携带其他未保存的乐观状态。
 */
export function applyConfigFields<TConfig extends object, TField extends keyof TConfig>(
  target: TConfig,
  source: TConfig,
  fields: readonly TField[],
): TConfig {
  const next = structuredClone(target);
  for (const field of fields) {
    next[field] = structuredClone(source[field]);
  }
  return next;
}

export function createPersistedConfigSnapshot<TConfig>(
  initialConfig: TConfig,
): PersistedConfigSnapshot<TConfig> {
  let snapshot = structuredClone(initialConfig);

  return {
    recordSuccessfulSave(config) {
      snapshot = structuredClone(config);
    },
    read() {
      return structuredClone(snapshot);
    },
  };
}
