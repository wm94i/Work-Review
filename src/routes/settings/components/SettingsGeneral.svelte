<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { formatDurationLocalized, locale, t } from '$lib/i18n/index.ts';
  import { updateConfigQueued } from '$lib/utils/configSaveQueue.ts';
  import CollapsibleSection from '../../../lib/components/CollapsibleSection.svelte';

  interface WorkTimeSegment {
    start_hour: number;
    start_minute: number;
    end_hour: number;
    end_minute: number;
  }

  interface GeneralConfig {
    auto_start: boolean;
    auto_start_silent: boolean;
    hide_dock_icon: boolean;
    lightweight_mode: boolean;
    work_time_enabled: boolean;
    work_time_segments?: WorkTimeSegment[] | null;
    work_start_hour: number;
    work_start_minute: number;
    work_end_hour: number;
    work_end_minute: number;
    standard_work_hours?: number | null;
    idle_threshold_minutes: number;
    daily_work_goal_minutes: number | null;
    goal_notifications: boolean;
    memory_enabled: boolean;
    daily_report_auto_generate_time: string | null;
  }

  type TimePart = number | string | null | undefined;

  export let config: GeneralConfig;

  const dispatch = createEventDispatcher<{ change: GeneralConfig }>();
  $: currentLocale = $locale;
  let workHours = '—';
  let autoStartEnabled = false;
  const MAX_WORK_SEGMENTS = 8;

  onMount(async () => {
    try {
      autoStartEnabled = await invoke<boolean>('is_autostart_enabled');
      if (config.auto_start !== autoStartEnabled) {
        config.auto_start = autoStartEnabled;
        try {
          await updateConfigQueued<GeneralConfig>((latestConfig) => {
            latestConfig.auto_start = autoStartEnabled;
          });
        } catch (e) {
          console.error('对齐注册表自启状态时写盘失败:', e);
        }
        dispatch('change', config);
      }
    } catch (e) {
      console.error('查询自启动状态失败:', e);
    }
  });

  function normalizeHour(value: TimePart): number {
    const parsed = Number.parseInt(String(value ?? ''), 10);
    if (!Number.isFinite(parsed)) return 0;
    return Math.min(Math.max(parsed, 0), 23);
  }

  function normalizeMinute(value: TimePart): number {
    const parsed = Number.parseInt(String(value ?? ''), 10);
    if (!Number.isFinite(parsed)) return 0;
    return Math.min(Math.max(parsed, 0), 59);
  }

  function parseTimeInput(value: string | null | undefined): [number, number] {
    const [hour = '0', minute = '0'] = String(value ?? '').split(':');
    return [normalizeHour(hour), normalizeMinute(minute)];
  }

  function segmentToTimeValue(hour: TimePart, minute: TimePart): string {
    return `${String(normalizeHour(hour)).padStart(2, '0')}:${String(normalizeMinute(minute)).padStart(2, '0')}`;
  }

  function normalizeSegment(segment?: Partial<WorkTimeSegment> | null): WorkTimeSegment {
    return {
      start_hour: normalizeHour(segment?.start_hour),
      start_minute: normalizeMinute(segment?.start_minute),
      end_hour: normalizeHour(segment?.end_hour),
      end_minute: normalizeMinute(segment?.end_minute),
    };
  }

  function normalizeWorkSegments(segments?: WorkTimeSegment[] | null): WorkTimeSegment[] {
    if (Array.isArray(segments) && segments.length > 0) {
      return segments.slice(0, MAX_WORK_SEGMENTS).map(normalizeSegment);
    }
    return [
      normalizeSegment({
        start_hour: config?.work_start_hour ?? 9,
        start_minute: config?.work_start_minute ?? 0,
        end_hour: config?.work_end_hour ?? 18,
        end_minute: config?.work_end_minute ?? 0,
      }),
    ];
  }

  function syncLegacyWorkRange(segments: WorkTimeSegment[]): void {
    if (!segments.length) return;
    const first = segments[0];
    const last = segments[segments.length - 1];
    config.work_start_hour = first.start_hour;
    config.work_start_minute = first.start_minute;
    config.work_end_hour = last.end_hour;
    config.work_end_minute = last.end_minute;
  }

  function totalWorkSegmentMinutes(segments: WorkTimeSegment[]): number {
    const ranges: [number, number][] = [];
    for (const segment of segments) {
      const start = segment.start_hour * 60 + segment.start_minute;
      const end = segment.end_hour * 60 + segment.end_minute;
      if (start === end) continue;
      if (end > start) {
        ranges.push([start, end]);
      } else {
        ranges.push([start, 24 * 60], [0, end]);
      }
    }
    ranges.sort((left, right) => left[0] - right[0] || left[1] - right[1]);

    let total = 0;
    let current: [number, number] | null = null;
    for (const range of ranges) {
      if (!current || range[0] > current[1]) {
        if (current) total += current[1] - current[0];
        current = [...range];
      } else {
        current[1] = Math.max(current[1], range[1]);
      }
    }
    return total + (current ? current[1] - current[0] : 0);
  }

  $: workSegments = normalizeWorkSegments(config?.work_time_segments);

  $: {
    currentLocale;
    const diffMinutes = totalWorkSegmentMinutes(workSegments);
    const diffSeconds = diffMinutes * 60;
    workHours = diffSeconds === 0 ? formatDurationLocalized(0) : formatDurationLocalized(diffSeconds);
  }

  function updateSegment(index: number, type: 'start' | 'end', value: string) {
    const segments = normalizeWorkSegments(config.work_time_segments);
    const target = { ...segments[index] };
    const [hour, minute] = parseTimeInput(value);
    if (type === 'start') {
      target.start_hour = hour;
      target.start_minute = minute;
    } else {
      target.end_hour = hour;
      target.end_minute = minute;
    }
    segments[index] = normalizeSegment(target);
    config.work_time_segments = segments;
    syncLegacyWorkRange(segments);
    dispatch('change', config);
  }

  function addWorkSegment() {
    const segments = normalizeWorkSegments(config.work_time_segments);
    if (segments.length >= MAX_WORK_SEGMENTS) return;

    const last = segments[segments.length - 1];
    const nextStartHour = normalizeHour(last?.end_hour ?? 9);
    const nextStartMinute = normalizeMinute(last?.end_minute ?? 0);
    const nextEndHour = (nextStartHour + 1) % 24;
    const nextSegment = normalizeSegment({
      start_hour: nextStartHour,
      start_minute: nextStartMinute,
      end_hour: nextEndHour,
      end_minute: nextStartMinute,
    });

    segments.push(nextSegment);
    config.work_time_segments = segments;
    syncLegacyWorkRange(segments);
    dispatch('change', config);
  }

  function removeWorkSegment(index: number) {
    const segments = normalizeWorkSegments(config.work_time_segments);
    if (segments.length <= 1) return;
    segments.splice(index, 1);
    config.work_time_segments = segments;
    syncLegacyWorkRange(segments);
    dispatch('change', config);
  }

  function handleChange() {
    dispatch('change', config);
  }

  async function toggleAutoStart() {
    const targetState = !autoStartEnabled;
    try {
      if (targetState) {
        await invoke<void>('enable_autostart', { silent: !!config.auto_start_silent });
      } else {
        await invoke<void>('disable_autostart');
      }
    } catch (e) {
      console.warn(`切换系统自启失败/警告 (目标状态: ${targetState}):`, e);
    }
    try {
      autoStartEnabled = await invoke<boolean>('is_autostart_enabled');
      config.auto_start = autoStartEnabled;
      try {
        await updateConfigQueued<GeneralConfig>((latestConfig) => {
          latestConfig.auto_start = autoStartEnabled;
        });
      } catch (e) {
        console.error('保存开机自启状态失败:', e);
      }
      dispatch('change', config);
    } catch (e) {
      console.error('重新校验开机自启状态失败:', e);
    }
  }

  async function toggleDockIcon() {
    config.hide_dock_icon = !config.hide_dock_icon;
    try {
      await invoke<void>('set_dock_visibility', { visible: !config.hide_dock_icon });
    } catch (e) {
      console.error('设置 Dock 图标失败:', e);
    }
    dispatch('change', config);
  }

  function toggleLightweightMode() {
    config.lightweight_mode = !config.lightweight_mode;
    dispatch('change', config);
  }

  async function updateAutoStartLaunchMode(silentMode: boolean) {
    config.auto_start_silent = silentMode;
    try {
      await updateConfigQueued<GeneralConfig>((latestConfig) => {
        latestConfig.auto_start_silent = silentMode;
      });
    } catch (e) {
      console.error('保存启动模式失败:', e);
    }
    if (autoStartEnabled) {
      try {
        await invoke<void>('enable_autostart', { silent: silentMode });
      } catch (e) {
        console.error('更新自启动参数失败:', e);
      }
    }
    dispatch('change', config);
  }
</script>

<div class="settings-card" data-locale={currentLocale}>
  <h3 class="settings-card-title">{t('settingsGeneral.title')}</h3>

  <div class="settings-section">
    <!-- 工作时段 -->
    <div class="settings-block">
      <div class="flex items-center justify-between">
        <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <span class="settings-text">{t('settingsGeneral.workTime')}</span>
          {#if config.work_time_enabled}
            <span class="settings-muted">{t('settingsGeneral.totalWorkHours', { duration: workHours })}</span>
          {:else}
            <span class="settings-muted">{t('settingsGeneral.workTimeDisabledHint')}</span>
          {/if}
        </div>
        <button
          type="button"
          on:click={() => {
            config.work_time_enabled = !config.work_time_enabled;
            handleChange();
          }}
          class="switch-track {config.work_time_enabled ? 'bg-emerald-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
          role="switch"
          aria-label={t('settingsGeneral.workTime')}
          aria-checked={config.work_time_enabled}
        >
          <span class="switch-thumb {config.work_time_enabled ? 'translate-x-5' : 'translate-x-0'}"></span>
        </button>
      </div>

      {#if config.work_time_enabled}
      <div class="space-y-2.5">
        {#each workSegments as segment, index}
          <div class="flex flex-wrap items-center gap-2.5">
            <span class="settings-subtle min-w-16">{t('settingsGeneral.segmentLabel', { index: index + 1 })}</span>
            <div class="control-inline">
              <span class="settings-subtle">{t('settingsGeneral.from')}</span>
              <input
                type="time"
                aria-label={`${t('settingsGeneral.segmentLabel', { index: index + 1 })} ${t('settingsGeneral.from')}`}
                value={segmentToTimeValue(segment.start_hour, segment.start_minute)}
                on:change={(e) => updateSegment(index, 'start', e.currentTarget.value)}
                class="w-24 bg-transparent text-sm font-mono text-slate-900 dark:text-[#e6edf3] focus:outline-none"
              />
            </div>

            <span class="text-slate-400 dark:text-[#484f58]">—</span>

            <div class="control-inline">
              <span class="settings-subtle">{t('settingsGeneral.to')}</span>
              <input
                type="time"
                aria-label={`${t('settingsGeneral.segmentLabel', { index: index + 1 })} ${t('settingsGeneral.to')}`}
                value={segmentToTimeValue(segment.end_hour, segment.end_minute)}
                on:change={(e) => updateSegment(index, 'end', e.currentTarget.value)}
                class="w-24 bg-transparent text-sm font-mono text-slate-900 dark:text-[#e6edf3] focus:outline-none"
              />
            </div>

            <button
              type="button"
              class="settings-action-secondary px-2.5 py-1.5 text-xs"
              disabled={workSegments.length <= 1}
              on:click={() => removeWorkSegment(index)}
            >
              {t('settingsGeneral.removeSegment')}
            </button>
          </div>
        {/each}
      </div>
      <button
        type="button"
        class="settings-action-secondary mt-3 px-3 py-1.5 text-xs"
        on:click={addWorkSegment}
        disabled={workSegments.length >= MAX_WORK_SEGMENTS}
      >
        {t('settingsGeneral.addSegment')}
      </button>
      <p class="settings-note">{t('settingsGeneral.workTimeHint')}</p>
      {/if}

      <!-- 高级工时设置（折叠） -->
      <CollapsibleSection title={t('settingsGeneral.advancedWorkSettings')} storageKey="settings.general.advancedWork">
        <!-- 标准工时（加班计算基准） -->
        <div class="flex items-center justify-between mt-3">
          <div>
            <span class="settings-text text-sm">{t('settingsGeneral.standardWorkHours')}</span>
            <p class="settings-muted mt-0.5">{t('settingsGeneral.standardWorkHoursHint')}</p>
          </div>
          <div class="flex items-center gap-2">
            <input
              type="number"
              aria-label={t('settingsGeneral.standardWorkHours')}
              min="1"
              max="24"
              step="0.5"
              value={config.standard_work_hours ?? 8}
              on:change={(e) => {
                // 非法值不再静默忽略(此前输入 30 会保留旧值但输入框仍显示 30):
                // 数字越界就 clamp 到 1~24,非数字回退当前值,并把结果回写到输入框
                const val = parseFloat(e.currentTarget.value);
                if (isNaN(val)) {
                  e.currentTarget.value = String(config.standard_work_hours ?? 8);
                  return;
                }
                const clamped = Math.min(24, Math.max(1, val));
                e.currentTarget.value = String(clamped);
                if (config.standard_work_hours !== clamped) {
                  config.standard_work_hours = clamped;
                  handleChange();
                }
              }}
              class="w-20 rounded-md border border-slate-200 bg-white px-2.5 py-1.5 text-center text-sm font-mono text-slate-900 focus:border-primary-400 focus:outline-none dark:border-[#30363d] dark:bg-[#161b22] dark:text-[#e6edf3]"
            />
            <span class="text-xs text-slate-500 dark:text-[#7d8590]">{t('settingsGeneral.hours')}</span>
          </div>
        </div>

        <!-- 空闲检测阈值 -->
        <div class="flex items-center justify-between mt-3">
          <div>
            <span class="settings-text text-sm">{t('settingsGeneral.idleThreshold')}</span>
            <p class="settings-muted mt-0.5">{t('settingsGeneral.idleThresholdHint')}</p>
          </div>
          <div class="flex items-center gap-2">
            <input
              type="number"
              aria-label={t('settingsGeneral.idleThreshold')}
              min="1"
              max="60"
              step="1"
              bind:value={config.idle_threshold_minutes}
              on:change={() => {
                config.idle_threshold_minutes = Math.max(1, Math.min(60, Number(config.idle_threshold_minutes) || 5));
                handleChange();
              }}
              class="w-16 rounded-md border border-slate-200 bg-white px-2 py-1 text-center text-sm dark:border-[#484f58] dark:bg-[#21262d]"
            />
            <span class="text-xs settings-subtle">{t('settingsGeneral.minutesUnit')}</span>
          </div>
        </div>
      </CollapsibleSection>
    </div>

    <!-- 工作目标 -->
    <CollapsibleSection title={t('settingsGeneral.workGoalTitle')} storageKey="settings.general.workGoal">
      <div class="flex items-center justify-between mt-3">
        <div>
          <span class="settings-text text-sm">{t('settingsGeneral.workGoalHours')}</span>
          <p class="settings-muted mt-0.5">{t('settingsGeneral.workGoalHint')}</p>
        </div>
        <div class="flex items-center gap-2">
          <input
            type="number"
            aria-label={t('settingsGeneral.workGoalHours')}
            min="0"
            max="16"
            step="0.5"
            value={config.daily_work_goal_minutes ? config.daily_work_goal_minutes / 60 : 0}
            on:change={(e) => {
              const hours = parseFloat(e.currentTarget.value);
              config.daily_work_goal_minutes = (!isNaN(hours) && hours > 0) ? Math.round(hours * 60) : null;
              handleChange();
            }}
            class="w-20 rounded-md border border-slate-200 bg-white px-2.5 py-1.5 text-center text-sm font-mono text-slate-900 focus:border-primary-400 focus:outline-none dark:border-[#30363d] dark:bg-[#161b22] dark:text-[#e6edf3]"
          />
          <span class="text-xs text-slate-500 dark:text-[#7d8590]">{t('settingsGeneral.hours')}</span>
        </div>
      </div>
      <label class="flex items-center justify-between mt-3 cursor-pointer">
        <span class="settings-text text-sm">{t('settingsGeneral.workGoalNotifications')}</span>
        <input type="checkbox" bind:checked={config.goal_notifications} on:change={handleChange} class="accent-primary-500" />
      </label>
    </CollapsibleSection>

    <!-- AI 工作记忆 -->
    <CollapsibleSection title={t('settingsGeneral.memoryTitle')} storageKey="settings.general.memory">
      <label class="flex items-center justify-between mt-3 cursor-pointer">
        <div>
          <span class="settings-text text-sm">{t('settingsGeneral.memoryEnabled')}</span>
          <p class="settings-muted mt-0.5">{t('settingsGeneral.memoryHint')}</p>
        </div>
        <input type="checkbox" bind:checked={config.memory_enabled} on:change={handleChange} class="accent-primary-500" />
      </label>
    </CollapsibleSection>
    <div class="settings-block pt-4 border-t border-slate-200 dark:border-[#30363d]">
      <div class="flex flex-wrap items-center gap-3">
        <span class="settings-text">{t('settingsGeneral.reportAutoGenerateTime')}</span>
        <div class="control-inline">
          <input
            type="time"
            aria-label={t('settingsGeneral.reportAutoGenerateTime')}
            value={config.daily_report_auto_generate_time ?? ''}
            on:change={(e) => {
              config.daily_report_auto_generate_time = e.currentTarget.value || null;
              dispatch('change', config);
            }}
            class="w-20 bg-transparent text-sm font-mono text-slate-900 dark:text-[#e6edf3] focus:outline-none"
          />
        </div>
        {#if config.daily_report_auto_generate_time}
          <button
            type="button"
            class="inline-flex items-center gap-1 px-2.5 py-1.5 text-xs rounded-lg text-rose-500 hover:bg-rose-50 dark:text-rose-400 dark:hover:bg-rose-950/30 transition-colors"
            on:click={() => {
              config.daily_report_auto_generate_time = null;
              dispatch('change', config);
            }}
          >
            {t('settingsGeneral.reportAutoGenerateReset')}
          </button>
        {/if}
      </div>
      <p class="settings-note">{t('settingsGeneral.reportAutoGenerateTimeHint')}</p>
    </div>

    <!-- 系统行为 -->
    <div class="settings-block pt-4 border-t border-slate-200 dark:border-[#30363d]">
      <div class="space-y-2.5">
        <div class="settings-row">
          <span class="settings-text">{t('settingsGeneral.autoStart')}</span>
          <button
            type="button"
            on:click={toggleAutoStart}
            class="switch-track {autoStartEnabled ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
            role="switch"
            aria-label={t('settingsGeneral.autoStart')}
            aria-checked={autoStartEnabled}
          >
            <span class="switch-thumb {autoStartEnabled ? 'translate-x-5' : 'translate-x-0'}"></span>
          </button>
        </div>

        {#if autoStartEnabled}
          <div class="ml-3 pl-3 border-l-2 border-primary-200/60 dark:border-primary-800/40">
            <span class="settings-label">{t('settingsGeneral.autoStartLaunchMode')}</span>
            <div class="mt-2 flex gap-2">
              <button
                type="button"
                on:click={() => updateAutoStartLaunchMode(false)}
                class="segment-btn {config.auto_start_silent ? 'settings-segment-base' : 'settings-segment-active'}"
              >
                {t('settingsGeneral.autoStartLaunchShow')}
              </button>
              <button
                type="button"
                on:click={() => updateAutoStartLaunchMode(true)}
                class="segment-btn {config.auto_start_silent ? 'settings-segment-active' : 'settings-segment-base'}"
              >
                {t('settingsGeneral.autoStartLaunchSilent')}
              </button>
            </div>
          </div>
        {/if}

        <div class="settings-row">
          <span class="settings-text">{t('settingsGeneral.hideDockIcon')}</span>
          <button
            type="button"
            on:click={toggleDockIcon}
            class="switch-track {config.hide_dock_icon ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
            role="switch"
            aria-label={t('settingsGeneral.hideDockIcon')}
            aria-checked={config.hide_dock_icon}
          >
            <span class="switch-thumb {config.hide_dock_icon ? 'translate-x-5' : 'translate-x-0'}"></span>
          </button>
        </div>

        <div class="settings-row">
          <div>
            <span class="settings-text">{t('settingsGeneral.lightweightMode')}</span>
            <p class="settings-muted mt-0.5">{t('settingsGeneral.lightweightModeDescription')}</p>
          </div>
          <button
            type="button"
            on:click={toggleLightweightMode}
            class="switch-track {config.lightweight_mode ? 'bg-primary-500' : 'bg-slate-300 dark:bg-[#484f58]'}"
            role="switch"
            aria-label={t('settingsGeneral.lightweightMode')}
            aria-checked={config.lightweight_mode}
          >
            <span class="switch-thumb {config.lightweight_mode ? 'translate-x-5' : 'translate-x-0'}"></span>
          </button>
        </div>
      </div>
    </div>
  </div>
</div>
