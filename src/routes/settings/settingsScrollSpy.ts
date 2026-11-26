export interface SettingsSectionMeasurement<Id extends string = string> {
  id: Id;
  /** 分区顶部相对于滚动容器视口顶部的偏移（px） */
  top: number;
}

export interface SettingsScrollSpyOptions<Id extends string = string> {
  /** 分区顶部跨过该阈值即视为进入当前激活区（px） */
  activationOffset: number;
  /** 滚动容器是否已接近底部（确保末尾较短的分区也能被高亮） */
  atBottom: boolean;
  fallback: Id;
}

/**
 * 根据各设置分区相对滚动视口的顶部位置，计算出当前应高亮的导航项。
 */
export function resolveActiveSettingsSection<Id extends string>(
  measurements: readonly SettingsSectionMeasurement<Id>[],
  options: SettingsScrollSpyOptions<Id>,
): Id {
  if (measurements.length === 0) {
    return options.fallback;
  }

  if (options.atBottom) {
    return measurements[measurements.length - 1].id;
  }

  let activeId = measurements[0].id;
  for (const item of measurements) {
    if (item.top <= options.activationOffset) {
      activeId = item.id;
    } else {
      break;
    }
  }
  return activeId;
}
