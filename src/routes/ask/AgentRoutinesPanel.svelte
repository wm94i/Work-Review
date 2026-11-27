<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { locale, t } from '$lib/i18n/index.ts';
  import { formatUserError } from '$lib/utils/errorDisplay.ts';
  import {
    formatRoutineSchedule,
    formatUnixTimestamp,
    isValidSkillNameInput,
    runStatusI18nKey,
    type AgentRoutine,
    type RoutineRunOutcome,
    type RoutineSchedule,
    type SkillMetadata,
  } from './agentHarnessTypes.ts';

  export let active = false;
  export let skills: SkillMetadata[] = [];
  export let onInspectRun: (runId: string) => void = () => {};

  let routines: AgentRoutine[] = [];
  let loading = false;
  let error: string | null = null;
  let editing = false;
  let editingExistingId: string | null = null;
  let formId = '';
  let formName = '';
  let formScheduleKind: 'daily_at' | 'interval_minutes' | 'manual_only' = 'daily_at';
  let formHour = 18;
  let formMinute = 30;
  let formIntervalMinutes = 60;
  let formSkillName = '';
  let formPrompt = '';
  let formEnabled = true;
  let formAllowCloudModel = false;
  let saving = false;

  let confirmDeleteId: string | null = null;
  let deletingId: string | null = null;
  let runningId: string | null = null;
  let reqSeq = 0;
  let wasActive = false;

  $: currentLocale = $locale;
  $: if (active && !wasActive) {
    wasActive = true;
    confirmDeleteId = null;
    void loadRoutines();
  } else if (!active && wasActive) {
    wasActive = false;
    editing = false;
    editingExistingId = null;
    confirmDeleteId = null;
  }

  export async function loadRoutines() {
    const seq = ++reqSeq;
    loading = true;
    error = null;
    try {
      const result = await invoke<AgentRoutine[]>('list_agent_routines');
      if (seq !== reqSeq) return;
      routines = Array.isArray(result) ? result : [];
    } catch (e) {
      if (seq !== reqSeq) return;
      error = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (seq === reqSeq) loading = false;
    }
  }

  function startNewRoutine() {
    confirmDeleteId = null;
    editingExistingId = null;
    formId = '';
    formName = '';
    formScheduleKind = 'daily_at';
    formHour = 18;
    formMinute = 30;
    formIntervalMinutes = 60;
    formSkillName = '';
    formPrompt = '';
    formEnabled = true;
    formAllowCloudModel = false;
    error = null;
    editing = true;
  }

  function startEditRoutine(item: AgentRoutine) {
    confirmDeleteId = null;
    editingExistingId = item.id;
    formId = item.id;
    formName = item.name;
    formSkillName = item.skillName ?? '';
    formPrompt = item.prompt;
    formEnabled = item.enabled;
    formAllowCloudModel = Boolean(item.allowCloudModel);
    if (item.schedule.kind === 'daily_at') {
      formScheduleKind = 'daily_at';
      formHour = item.schedule.hour;
      formMinute = item.schedule.minute;
    } else if (item.schedule.kind === 'interval_minutes') {
      formScheduleKind = 'interval_minutes';
      formIntervalMinutes = item.schedule.every_minutes;
    } else {
      formScheduleKind = 'manual_only';
    }
    error = null;
    editing = true;
  }

  function fillDailyTemplate() {
    if (!editingExistingId && !formId.trim()) formId = 'evening-wrapup';
    if (!formName.trim()) {
      formName = currentLocale === 'en' ? 'Daily Evening Wrap-Up' : '每日下班前复盘';
    }
    if (!formPrompt.trim()) {
      formPrompt = currentLocale === 'en'
        ? 'Summarize today work highlights, time distribution by project, and top 3 open follow-ups for tomorrow.'
        : '汇总我今天的主要工作产出、各项目耗时分布以及明天需要优先推进的 3 项待办。';
    }
    formScheduleKind = 'daily_at';
    formHour = 18;
    formMinute = 30;
  }

  function buildSchedulePayload(): RoutineSchedule {
    if (formScheduleKind === 'daily_at') {
      return {
        kind: 'daily_at',
        hour: Math.max(0, Math.min(23, Math.floor(Number(formHour) || 0))),
        minute: Math.max(0, Math.min(59, Math.floor(Number(formMinute) || 0))),
      };
    }
    if (formScheduleKind === 'interval_minutes') {
      return {
        kind: 'interval_minutes',
        every_minutes: Math.max(15, Math.min(1440, Math.floor(Number(formIntervalMinutes) || 60))),
      };
    }
    return { kind: 'manual_only' };
  }

  async function submitRoutine() {
    error = null;
    const id = (editingExistingId ?? formId).trim();
    if (!isValidSkillNameInput(id)) {
      error = t('ask.harness.skillNameInvalid');
      return;
    }
    if (!formName.trim() || !formPrompt.trim()) {
      error = t('ask.harness.skillBodyRequired');
      return;
    }
    saving = true;
    try {
      const updated = await invoke<AgentRoutine[]>('save_agent_routine', {
        input: {
          id,
          name: formName.trim(),
          prompt: formPrompt,
          enabled: formEnabled,
          allowCloudModel: formAllowCloudModel,
          schedule: buildSchedulePayload(),
          skillName: formSkillName.trim() || null,
        },
      });
      routines = Array.isArray(updated) ? updated : [];
      editing = false;
      editingExistingId = null;
    } catch (e) {
      error = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      saving = false;
    }
  }

  async function handleDelete(id: string) {
    if (deletingId !== null) return;
    error = null;
    deletingId = id;
    try {
      const updated = await invoke<AgentRoutine[]>('delete_agent_routine', { id });
      routines = Array.isArray(updated) ? updated : [];
      confirmDeleteId = null;
    } catch (e) {
      error = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (deletingId === id) deletingId = null;
    }
  }

  async function handleRunNow(id: string) {
    if (runningId !== null) return;
    error = null;
    runningId = id;
    try {
      const outcome = await invoke<RoutineRunOutcome>('run_agent_routine_now', { id });
      await loadRoutines();
      if (outcome?.status === 'failed' && outcome.summary) {
        error = outcome.summary;
      }
    } catch (e) {
      error = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (runningId === id) runningId = null;
    }
  }
</script>

<p class="mb-3 rounded-xl border border-slate-200/80 bg-slate-50/80 px-3 py-2 text-[11px] leading-relaxed text-slate-500 dark:border-[#30363d] dark:bg-[#161b22] dark:text-[#8b949e]">
  {t('ask.harness.routinesHint')}
</p>

{#if error}
  <div role="alert" class="mb-3 rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2 text-xs text-rose-700 dark:text-rose-300">
    {error}
  </div>
{/if}

{#if editing}
  <form class="space-y-3" on:submit|preventDefault={submitRoutine}>
    <div class="flex items-center justify-between">
      <button
        type="button"
        class="rounded-lg bg-indigo-500/10 px-2.5 py-1 text-[11px] font-medium text-indigo-600 transition hover:bg-indigo-500/20 dark:text-indigo-300"
        on:click={fillDailyTemplate}
      >
        {t('ask.harness.insertDailyRoutineTemplate')}
      </button>
      <button
        type="button"
        class="text-xs text-slate-400 hover:text-slate-600 dark:text-[#8b949e]"
        on:click={() => {
          editing = false;
          editingExistingId = null;
          error = null;
        }}
      >
        {t('common.cancel')}
      </button>
    </div>

    <label class="block">
      <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.routineIdLabel')}</span>
      <input
        type="text"
        bind:value={formId}
        disabled={editingExistingId !== null}
        placeholder="evening-wrapup"
        class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 font-mono text-xs text-slate-800 outline-none focus:border-indigo-400 disabled:opacity-60 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
      />
    </label>

    <label class="block">
      <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.routineNameLabel')}</span>
      <input
        type="text"
        bind:value={formName}
        class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 text-xs text-slate-800 outline-none focus:border-indigo-400 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
      />
    </label>

    <div class="space-y-2">
      <span class="block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.routineScheduleLabel')}</span>
      <div class="flex flex-wrap items-center gap-1.5">
        <select
          bind:value={formScheduleKind}
          class="rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 text-xs text-slate-800 outline-none dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
        >
          <option value="daily_at">{t('ask.harness.scheduleDaily')}</option>
          <option value="interval_minutes">{t('ask.harness.scheduleInterval')}</option>
          <option value="manual_only">{t('ask.harness.scheduleManual')}</option>
        </select>

        {#if formScheduleKind === 'daily_at'}
          <input type="number" min="0" max="23" bind:value={formHour} class="w-16 rounded-lg border border-slate-200 bg-white px-2 py-1.5 text-xs dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]" />
          <span class="text-xs text-slate-400">:</span>
          <input type="number" min="0" max="59" bind:value={formMinute} class="w-16 rounded-lg border border-slate-200 bg-white px-2 py-1.5 text-xs dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]" />
        {:else if formScheduleKind === 'interval_minutes'}
          <input type="number" min="15" max="1440" bind:value={formIntervalMinutes} class="w-24 rounded-lg border border-slate-200 bg-white px-2 py-1.5 text-xs dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]" />
        {/if}
      </div>
    </div>

    {#if skills.length > 0}
      <label class="block">
        <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.routineSkillLabel')}</span>
        <select
          bind:value={formSkillName}
          class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 font-mono text-xs text-slate-800 outline-none dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
        >
          <option value="">—</option>
          {#each skills as s (s.name)}
            <option value={s.name}>{s.name}</option>
          {/each}
        </select>
      </label>
    {/if}

    <label class="block">
      <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.routinePromptLabel')}</span>
      <textarea
        bind:value={formPrompt}
        rows="5"
        class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-2 text-xs leading-relaxed text-slate-800 outline-none focus:border-indigo-400 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
      ></textarea>
    </label>

    <label class="flex items-center gap-2 text-xs text-slate-600 dark:text-[#c9d1d9]">
      <input type="checkbox" bind:checked={formEnabled} class="rounded" />
      <span>{t('ask.harness.routineEnabled')}</span>
    </label>

    <label class="flex items-start gap-2 text-xs text-slate-600 dark:text-[#c9d1d9]">
      <input type="checkbox" bind:checked={formAllowCloudModel} class="mt-0.5 rounded" />
      <span>{t('ask.harness.routineAllowCloud')}</span>
    </label>

    <button
      type="submit"
      disabled={saving}
      class="w-full rounded-lg bg-indigo-600 px-3 py-2 text-xs font-medium text-white transition hover:bg-indigo-500 disabled:opacity-50"
    >
      {saving ? t('ask.harness.savingSkill') : t('ask.harness.saveRoutine')}
    </button>
  </form>
{:else}
  <div class="mb-3">
    <button
      type="button"
      class="rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-indigo-500"
      on:click={startNewRoutine}
    >
      + {t('ask.harness.newRoutine')}
    </button>
  </div>

  {#if loading}
    <p role="status" aria-live="polite" class="py-6 text-center text-xs text-slate-400 dark:text-[#636c76]">{t('ask.harness.loading')}</p>
  {:else if routines.length === 0}
    <p class="py-8 text-center text-xs leading-relaxed text-slate-400 dark:text-[#636c76]">{t('ask.harness.routinesEmpty')}</p>
  {:else}
    <div class="space-y-2">
      {#each routines as item (item.id)}
        <div class="rounded-xl border border-slate-200/80 bg-white/70 p-3 dark:border-[#30363d] dark:bg-[#161b22]/80">
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <div class="flex flex-wrap items-center gap-1.5">
                <h4 class="truncate text-xs font-semibold text-slate-800 dark:text-[#e6edf3]">{item.name}</h4>
                <span class="rounded bg-slate-500/10 px-1.5 py-0.5 font-mono text-[10px] text-slate-500 dark:text-[#8b949e]">
                  {formatRoutineSchedule(item.schedule, currentLocale)}
                </span>
                <span class="rounded px-1.5 py-0.5 text-[10px] {item.allowCloudModel
                  ? 'bg-amber-500/15 text-amber-700 dark:text-amber-300'
                  : 'bg-emerald-500/12 text-emerald-700 dark:text-emerald-300'}">
                  {item.allowCloudModel ? t('ask.harness.routineCloudBadge') : t('ask.harness.routineLocalBadge')}
                </span>
                {#if !item.enabled}
                  <span class="rounded bg-slate-500/12 px-1.5 py-0.5 text-[10px] text-slate-400">OFF</span>
                {/if}
              </div>
              <p class="mt-1 line-clamp-2 text-[11px] text-slate-500 dark:text-[#8b949e]">{item.prompt}</p>
            </div>
            <div class="flex shrink-0 items-center gap-1">
              {#if confirmDeleteId === item.id}
                <button
                  type="button"
                  disabled={deletingId !== null}
                  class="rounded bg-rose-600 px-2 py-1 text-[11px] font-medium text-white hover:bg-rose-500 disabled:opacity-50"
                  on:click={() => handleDelete(item.id)}
                >
                  {deletingId === item.id ? t('ask.harness.deletingSkill') : t('ask.harness.confirmDeleteSkill')}
                </button>
                <button
                  type="button"
                  disabled={deletingId !== null}
                  class="rounded px-2 py-1 text-[11px] text-slate-500 hover:bg-slate-100 dark:text-[#8b949e]"
                  on:click={() => (confirmDeleteId = null)}
                >
                  {t('common.cancel')}
                </button>
              {:else}
                <button
                  type="button"
                  disabled={runningId !== null || deletingId !== null}
                  class="rounded px-2 py-1 text-[11px] font-medium text-emerald-600 hover:bg-emerald-500/10 disabled:opacity-50 dark:text-emerald-300"
                  on:click={() => handleRunNow(item.id)}
                >
                  {runningId === item.id ? t('ask.harness.runningRoutine') : t('ask.harness.runRoutineNow')}
                </button>
                <button
                  type="button"
                  disabled={runningId !== null || deletingId !== null}
                  class="rounded px-2 py-1 text-[11px] font-medium text-indigo-600 hover:bg-indigo-500/10 disabled:opacity-50 dark:text-indigo-300"
                  on:click={() => startEditRoutine(item)}
                >
                  {t('ask.harness.editSkill')}
                </button>
                <button
                  type="button"
                  disabled={runningId !== null || deletingId !== null}
                  class="rounded px-2 py-1 text-[11px] font-medium text-rose-600 hover:bg-rose-500/10 disabled:opacity-50 dark:text-rose-300"
                  on:click={() => (confirmDeleteId = item.id)}
                >
                  {t('ask.harness.deleteSkill')}
                </button>
              {/if}
            </div>
          </div>

          {#if item.lastRunAt}
            <div class="mt-2 rounded-lg bg-slate-500/6 px-2.5 py-1.5 text-[11px] text-slate-500 dark:text-[#8b949e]">
              <div class="flex items-center justify-between gap-2">
                <span>
                  {item.lastStatus ? t(runStatusI18nKey(item.lastStatus)) : ''} · {formatUnixTimestamp(item.lastRunAt)}
                </span>
                {#if item.lastRunId}
                  <button
                    type="button"
                    class="font-medium text-indigo-600 hover:underline dark:text-indigo-300"
                    on:click={() => item.lastRunId && onInspectRun(item.lastRunId)}
                  >
                    {t('ask.harness.viewLastTrace')}
                  </button>
                {/if}
              </div>
              {#if item.lastSummary}
                <p class="mt-1 line-clamp-2 text-[11px] text-slate-600 dark:text-[#c9d1d9]">{item.lastSummary}</p>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
{/if}
