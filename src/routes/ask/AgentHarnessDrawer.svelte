<script lang="ts">
  import { fly } from 'svelte/transition';
  import { invoke } from '@tauri-apps/api/core';
  import { locale, t } from '$lib/i18n/index.ts';
  import { formatUserError } from '$lib/utils/errorDisplay.ts';
  import { trapFocus } from '$lib/utils/focusTrap.ts';
  import AgentRoutinesPanel from './AgentRoutinesPanel.svelte';
  import AgentMcpPanel from './AgentMcpPanel.svelte';
  import {
    auditDecisionI18nKey,
    formatEventPayloadSummary,
    formatUnixTimestamp,
    isValidSkillNameInput,
    parseAllowedToolsInput,
    parseTaintFlags,
    runStatusI18nKey,
    type AgentRunSummary,
    type AgentRunTrace,
    type SkillDetail,
    type SkillMetadata,
  } from './agentHarnessTypes.ts';

  export let open = false;
  export let conversationId: number | null = null;
  export let onClose: () => void = () => {};
  export let onSkillsChanged: (skills: SkillMetadata[]) => void = () => {};

  let activeTab: 'runs' | 'skills' | 'routines' | 'mcp' = 'runs';
  let routinesPanel: AgentRoutinesPanel | null = null;
  let mcpPanel: AgentMcpPanel | null = null;
  let scopeAll = false;
  let runs: AgentRunSummary[] = [];
  let loadingRuns = false;
  let selectedTrace: AgentRunTrace | null = null;
  let loadingTrace = false;
  let runsError: string | null = null;

  let skills: SkillMetadata[] = [];
  let loadingSkills = false;
  let skillsError: string | null = null;
  let editingSkill = false;
  let editingExistingName: string | null = null;
  let formName = '';
  let formDescription = '';
  let formAllowedTools = '';
  let formBody = '';
  let savingSkill = false;
  let confirmDeleteSkillName: string | null = null;
  let deletingSkillName: string | null = null;

  let runsReqSeq = 0;
  let traceReqSeq = 0;
  let skillsReqSeq = 0;
  let editReqSeq = 0;
  let wasOpen = false;

  $: currentLocale = $locale;
  $: if (open && !wasOpen) {
    wasOpen = true;
    selectedTrace = null;
    confirmDeleteSkillName = null;
    scopeAll = conversationId == null;
    void loadRuns();
    void loadSkills();
  } else if (!open && wasOpen) {
    wasOpen = false;
    editReqSeq += 1;
    editingSkill = false;
    editingExistingName = null;
    confirmDeleteSkillName = null;
    selectedTrace = null;
  }

  async function loadRuns() {
    const seq = ++runsReqSeq;
    loadingRuns = true;
    runsError = null;
    try {
      const result = await invoke<AgentRunSummary[]>('list_agent_runs', {
        conversationId: scopeAll ? null : conversationId,
        limit: 40,
      });
      if (seq !== runsReqSeq) return;
      runs = Array.isArray(result) ? result : [];
    } catch (e) {
      if (seq !== runsReqSeq) return;
      runsError = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (seq === runsReqSeq) loadingRuns = false;
    }
  }

  async function openRunTrace(runId: string) {
    const seq = ++traceReqSeq;
    loadingTrace = true;
    runsError = null;
    try {
      const trace = await invoke<AgentRunTrace | null>('get_agent_run_trace', { runId });
      if (seq !== traceReqSeq) return;
      selectedTrace = trace;
    } catch (e) {
      if (seq !== traceReqSeq) return;
      runsError = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (seq === traceReqSeq) loadingTrace = false;
    }
  }

  async function loadSkills() {
    const seq = ++skillsReqSeq;
    loadingSkills = true;
    skillsError = null;
    try {
      const result = await invoke<SkillMetadata[]>('list_personal_skills');
      if (seq !== skillsReqSeq) return;
      skills = Array.isArray(result) ? result : [];
      onSkillsChanged(skills);
    } catch (e) {
      if (seq !== skillsReqSeq) return;
      skillsError = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (seq === skillsReqSeq) loadingSkills = false;
    }
  }

  function startNewSkill() {
    editReqSeq += 1;
    confirmDeleteSkillName = null;
    editingExistingName = null;
    formName = '';
    formDescription = '';
    formAllowedTools = '';
    formBody = '';
    skillsError = null;
    editingSkill = true;
  }

  async function startEditSkill(name: string) {
    const seq = ++editReqSeq;
    confirmDeleteSkillName = null;
    skillsError = null;
    try {
      const detail = await invoke<SkillDetail>('get_personal_skill', { name });
      if (seq !== editReqSeq || !open) return;
      editingExistingName = detail.name;
      formName = detail.name;
      formDescription = detail.description;
      formAllowedTools = detail.allowedTools.join(', ');
      formBody = detail.body;
      editingSkill = true;
    } catch (e) {
      if (seq !== editReqSeq || !open) return;
      skillsError = formatUserError(e, t('common.loadFailedRetry'));
    }
  }

  function fillWeeklyTemplate() {
    if (!editingExistingName && !formName.trim()) {
      formName = 'weekly-review-brief';
    }
    if (!formDescription.trim()) {
      formDescription = currentLocale === 'en'
        ? 'Compile weekly project hours, highlights, and blockers into a concise brief'
        : '汇总本周项目工时、关键进展与阻塞项并整理为结构化周复盘';
    }
    if (!formAllowedTools.trim()) {
      formAllowedTools = 'weekly_review, project_stats, list_todos';
    }
    if (!formBody.trim()) {
      formBody = currentLocale === 'en'
        ? '1. Call `weekly_review` and `project_stats` for the current week.\n2. Call `list_todos` to identify open follow-ups.\n3. Present findings in three sections: Key Wins, Project Time Breakdown, and Next Week Focus.'
        : '1. 调用 `weekly_review` 与 `project_stats` 获取本周工作分布与项目工时。\n2. 调用 `list_todos` 检查尚未完成的待办与跟进项。\n3. 按「本周关键产出 / 项目耗时分布 / 下周重点与阻塞」三部分输出精炼复盘。';
    }
  }

  async function submitSkillForm() {
    skillsError = null;
    const targetName = (editingExistingName ?? formName).trim();
    if (!isValidSkillNameInput(targetName)) {
      skillsError = t('ask.harness.skillNameInvalid');
      return;
    }
    if (!formBody.trim()) {
      skillsError = t('ask.harness.skillBodyRequired');
      return;
    }
    savingSkill = true;
    try {
      const updated = await invoke<SkillMetadata[]>('save_personal_skill', {
        name: targetName,
        description: formDescription.trim(),
        allowedTools: parseAllowedToolsInput(formAllowedTools),
        body: formBody,
      });
      skills = Array.isArray(updated) ? updated : [];
      onSkillsChanged(skills);
      editingSkill = false;
      editingExistingName = null;
    } catch (e) {
      skillsError = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      savingSkill = false;
    }
  }

  async function handleDeleteSkill(name: string) {
    if (deletingSkillName !== null) return;
    skillsError = null;
    deletingSkillName = name;
    try {
      const updated = await invoke<SkillMetadata[]>('delete_personal_skill', { name });
      skills = Array.isArray(updated) ? updated : [];
      onSkillsChanged(skills);
      confirmDeleteSkillName = null;
    } catch (e) {
      skillsError = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (deletingSkillName === name) {
        deletingSkillName = null;
      }
    }
  }

  async function handleOpenSkillsDir() {
    skillsError = null;
    try {
      await invoke<string>('open_personal_skills_dir');
    } catch (e) {
      skillsError = formatUserError(e, t('common.loadFailedRetry'));
    }
  }

  function statusBadgeClass(status: string): string {
    switch (status) {
      case 'completed':
        return 'bg-emerald-500/12 text-emerald-700 dark:text-emerald-300';
      case 'running':
      case 'waiting_approval':
        return 'bg-amber-500/15 text-amber-700 dark:text-amber-300';
      case 'failed':
      case 'timed_out':
        return 'bg-rose-500/15 text-rose-700 dark:text-rose-300';
      default:
        return 'bg-slate-500/12 text-slate-600 dark:text-slate-300';
    }
  }

  function decisionBadgeClass(decision: string): string {
    switch (decision) {
      case 'allow':
      case 'approved':
        return 'bg-emerald-500/12 text-emerald-700 dark:text-emerald-300';
      case 'deny':
      case 'denied':
      case 'timed_out':
        return 'bg-rose-500/15 text-rose-700 dark:text-rose-300';
      default:
        return 'bg-amber-500/15 text-amber-700 dark:text-amber-300';
    }
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="fixed inset-0 z-[165] flex justify-end bg-slate-950/32 backdrop-blur-sm animate-fadeIn"
    role="presentation"
    on:click|self={onClose}
  >
    <div
      use:trapFocus
      role="dialog"
      aria-modal="true"
      aria-label={t('ask.harness.drawerTitle')}
      class="ask-history-drawer flex h-full w-[440px] max-w-[92vw] flex-col"
      in:fly={{ x: 360, duration: 220 }}
    >
      <div class="ask-history-head">
        <h3>{t('ask.harness.drawerTitle')}</h3>
        <div class="flex items-center gap-1.5">
          <button
            type="button"
            class="rounded-lg px-2 py-1 text-xs font-medium text-slate-500 transition hover:bg-slate-100 hover:text-slate-700 dark:text-[#8b949e] dark:hover:bg-[#21262d] dark:hover:text-[#c9d1d9]"
            on:click={() => {
              if (activeTab === 'runs') void loadRuns();
              else if (activeTab === 'skills') void loadSkills();
              else if (activeTab === 'routines') void routinesPanel?.loadRoutines();
              else if (activeTab === 'mcp') void mcpPanel?.loadServers();
            }}
          >
            {t('ask.harness.refresh')}
          </button>
          <button
            type="button"
            class="rounded-lg p-1.5 text-slate-400 transition hover:bg-slate-100 hover:text-slate-600 dark:text-[#636c76] dark:hover:bg-[#21262d] dark:hover:text-[#adbac7]"
            on:click={onClose}
            aria-label={t('common.cancel')}
            title={t('common.cancel')}
          >
            <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
      </div>

      <div class="flex flex-wrap items-center gap-1 border-b border-slate-200/80 px-3 py-2 dark:border-[#30363d]">
        <button
          type="button"
          class="rounded-lg px-2.5 py-1.5 text-xs font-medium transition {activeTab === 'runs'
            ? 'bg-indigo-500/12 text-indigo-600 dark:bg-indigo-500/20 dark:text-indigo-300'
            : 'text-slate-500 hover:bg-slate-100 dark:text-[#8b949e] dark:hover:bg-[#21262d]'}"
          on:click={() => {
            activeTab = 'runs';
            void loadRuns();
          }}
        >
          {t('ask.harness.tabRuns')}
        </button>
        <button
          type="button"
          class="rounded-lg px-2.5 py-1.5 text-xs font-medium transition {activeTab === 'skills'
            ? 'bg-indigo-500/12 text-indigo-600 dark:bg-indigo-500/20 dark:text-indigo-300'
            : 'text-slate-500 hover:bg-slate-100 dark:text-[#8b949e] dark:hover:bg-[#21262d]'}"
          on:click={() => {
            activeTab = 'skills';
            void loadSkills();
          }}
        >
          {t('ask.harness.tabSkills')}
          {#if skills.length > 0}
            <span class="ms-1 rounded-full bg-indigo-500/15 px-1.5 py-0.5 text-[10px]">{skills.length}</span>
          {/if}
        </button>
        <button
          type="button"
          class="rounded-lg px-2.5 py-1.5 text-xs font-medium transition {activeTab === 'routines'
            ? 'bg-indigo-500/12 text-indigo-600 dark:bg-indigo-500/20 dark:text-indigo-300'
            : 'text-slate-500 hover:bg-slate-100 dark:text-[#8b949e] dark:hover:bg-[#21262d]'}"
          on:click={() => {
            activeTab = 'routines';
          }}
        >
          {t('ask.harness.tabRoutines')}
        </button>
        <button
          type="button"
          class="rounded-lg px-2.5 py-1.5 text-xs font-medium transition {activeTab === 'mcp'
            ? 'bg-indigo-500/12 text-indigo-600 dark:bg-indigo-500/20 dark:text-indigo-300'
            : 'text-slate-500 hover:bg-slate-100 dark:text-[#8b949e] dark:hover:bg-[#21262d]'}"
          on:click={() => {
            activeTab = 'mcp';
          }}
        >
          {t('ask.harness.tabMcp')}
        </button>
      </div>

      <div class="app-floating-scroll flex-1 overflow-y-auto p-3">
        {#if activeTab === 'runs'}
          {#if runsError}
            <div role="alert" class="mb-3 rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2 text-xs text-rose-700 dark:text-rose-300">
              {runsError}
            </div>
          {/if}

          {#if selectedTrace}
            <div class="space-y-3">
              <button
                type="button"
                class="inline-flex items-center gap-1 rounded-lg px-2 py-1 text-xs font-medium text-indigo-600 transition hover:bg-indigo-500/10 dark:text-indigo-300"
                on:click={() => (selectedTrace = null)}
              >
                ← {t('ask.harness.backToRuns')}
              </button>

              <div class="rounded-xl border border-slate-200/80 bg-slate-50/70 p-3 dark:border-[#30363d] dark:bg-[#161b22]">
                <div class="flex flex-wrap items-center justify-between gap-2">
                  <span class="rounded-full px-2 py-0.5 text-[11px] font-medium {statusBadgeClass(selectedTrace.run.status)}">
                    {t(runStatusI18nKey(selectedTrace.run.status))}
                  </span>
                  <span class="text-[11px] text-slate-400 dark:text-[#8b949e]">
                    {selectedTrace.run.source} · {selectedTrace.run.requestMode}
                  </span>
                </div>
                <p class="mt-2 break-words text-xs font-medium text-slate-700 dark:text-[#c9d1d9]">
                  {selectedTrace.run.inputPreview}
                </p>
                <div class="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-slate-500 dark:text-[#8b949e]">
                  {#if selectedTrace.run.modelName}
                    <span class="font-mono">{selectedTrace.run.modelName}</span>
                  {/if}
                  <span>{t('ask.harness.runMetaTools', { count: selectedTrace.run.toolCallCount })}</span>
                  <span>{t('ask.harness.runMetaCompactions', { count: selectedTrace.run.compactionCount })}</span>
                </div>
                <div class="mt-2 flex flex-wrap items-center gap-1.5">
                  <span class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('ask.harness.taintLabel')}:</span>
                  {#each parseTaintFlags(selectedTrace.run.taintFlags) as flag}
                    <span class="rounded bg-amber-500/12 px-1.5 py-0.5 font-mono text-[10px] text-amber-700 dark:text-amber-300">{flag}</span>
                  {:else}
                    <span class="text-[11px] text-slate-400 dark:text-[#636c76]">{t('ask.harness.taintNone')}</span>
                  {/each}
                </div>
                {#if selectedTrace.run.errorMessage}
                  <p class="mt-2 rounded bg-rose-500/10 px-2 py-1 text-[11px] text-rose-700 dark:text-rose-300">
                    {selectedTrace.run.errorMessage}
                  </p>
                {/if}
              </div>

              <div>
                <h4 class="mb-2 text-xs font-semibold text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.auditTitle')}</h4>
                {#if selectedTrace.audits.length === 0}
                  <p class="text-xs text-slate-400 dark:text-[#636c76]">{t('ask.harness.auditEmpty')}</p>
                {:else}
                  <div class="space-y-1.5">
                    {#each selectedTrace.audits as audit (audit.id)}
                      <div class="rounded-lg border border-slate-200/70 bg-white/70 px-2.5 py-2 dark:border-[#30363d] dark:bg-[#0d1117]/60">
                        <div class="flex flex-wrap items-center justify-between gap-1.5">
                          <span class="font-mono text-xs font-medium text-slate-700 dark:text-[#c9d1d9]">{audit.toolName}</span>
                          <div class="flex items-center gap-1">
                            <span class="rounded bg-slate-500/10 px-1.5 py-0.5 font-mono text-[10px] text-slate-500 dark:text-[#8b949e]">{audit.riskTier}</span>
                            <span class="rounded px-1.5 py-0.5 text-[10px] font-medium {decisionBadgeClass(audit.decision)}">
                              {t(auditDecisionI18nKey(audit.decision))}
                            </span>
                            <span class="font-mono text-[10px] text-slate-400 dark:text-[#636c76]">{audit.durationMs}ms</span>
                          </div>
                        </div>
                        {#if audit.reason}
                          <p class="mt-1 text-[11px] text-slate-500 dark:text-[#8b949e]">{audit.reason}</p>
                        {/if}
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>

              <div>
                <h4 class="mb-2 text-xs font-semibold text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.eventsTitle')}</h4>
                {#if selectedTrace.events.length === 0}
                  <p class="text-xs text-slate-400 dark:text-[#636c76]">{t('ask.harness.eventsEmpty')}</p>
                {:else}
                  <ol class="space-y-1">
                    {#each selectedTrace.events as ev (ev.id)}
                      {@const payloadSummary = formatEventPayloadSummary(ev.payloadJson)}
                      <li class="rounded-lg border border-slate-200/60 px-2.5 py-1.5 text-[11px] dark:border-[#30363d]">
                        <div class="flex items-center justify-between gap-2">
                          <span class="font-mono font-medium text-slate-700 dark:text-[#c9d1d9]">
                            #{ev.seq} {ev.eventKind}{ev.toolName ? ` · ${ev.toolName}` : ''}
                          </span>
                          <span class="text-[10px] text-slate-400 dark:text-[#636c76]">{formatUnixTimestamp(ev.createdAt)}</span>
                        </div>
                        {#if payloadSummary}
                          <p class="mt-0.5 truncate font-mono text-[10px] text-slate-500 dark:text-[#8b949e]">
                            {payloadSummary}
                          </p>
                        {/if}
                      </li>
                    {/each}
                  </ol>
                {/if}
              </div>
            </div>
          {:else}
            <div class="mb-3 flex items-center gap-1.5">
              {#if conversationId != null}
                <button
                  type="button"
                  aria-pressed={!scopeAll}
                  class="rounded-lg px-2.5 py-1 text-[11px] font-medium transition {!scopeAll
                    ? 'bg-slate-900 text-white dark:bg-[#30363d] dark:text-[#e6edf3]'
                    : 'bg-slate-100 text-slate-600 dark:bg-[#161b22] dark:text-[#8b949e]'}"
                  on:click={() => {
                    scopeAll = false;
                    void loadRuns();
                  }}
                >
                  {t('ask.harness.scopeCurrent')}
                </button>
              {/if}
              <button
                type="button"
                aria-pressed={scopeAll || conversationId == null}
                class="rounded-lg px-2.5 py-1 text-[11px] font-medium transition {scopeAll || conversationId == null
                  ? 'bg-slate-900 text-white dark:bg-[#30363d] dark:text-[#e6edf3]'
                  : 'bg-slate-100 text-slate-600 dark:bg-[#161b22] dark:text-[#8b949e]'}"
                on:click={() => {
                  scopeAll = true;
                  void loadRuns();
                }}
              >
                {t('ask.harness.scopeAll')}
              </button>
            </div>

            {#if loadingRuns || loadingTrace}
              <p role="status" aria-live="polite" class="py-6 text-center text-xs text-slate-400 dark:text-[#636c76]">{t('ask.harness.loading')}</p>
            {:else if runs.length === 0}
              <p class="py-8 text-center text-xs leading-relaxed text-slate-400 dark:text-[#636c76]">
                {t('ask.harness.runsEmpty')}
              </p>
            {:else}
              <div class="space-y-2">
                {#each runs as run (run.id)}
                  <button
                    type="button"
                    class="w-full rounded-xl border border-slate-200/80 bg-white/70 p-3 text-start transition hover:border-indigo-400/60 hover:bg-indigo-50/30 dark:border-[#30363d] dark:bg-[#161b22]/80 dark:hover:border-indigo-500/50"
                    on:click={() => openRunTrace(run.id)}
                  >
                    <div class="flex items-center justify-between gap-2">
                      <span class="rounded-full px-2 py-0.5 text-[10px] font-medium {statusBadgeClass(run.status)}">
                        {t(runStatusI18nKey(run.status))}
                      </span>
                      <span class="text-[10px] text-slate-400 dark:text-[#636c76]">{formatUnixTimestamp(run.startedAt)}</span>
                    </div>
                    <p class="mt-1.5 line-clamp-2 text-xs font-medium text-slate-700 dark:text-[#c9d1d9]">
                      {run.inputPreview}
                    </p>
                    <div class="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-slate-500 dark:text-[#8b949e]">
                      <span>{run.source}</span>
                      <span>·</span>
                      <span>{t('ask.harness.runMetaTools', { count: run.toolCallCount })}</span>
                      {#if run.compactionCount > 0}
                        <span>·</span>
                        <span>{t('ask.harness.runMetaCompactions', { count: run.compactionCount })}</span>
                      {/if}
                    </div>
                  </button>
                {/each}
              </div>
            {/if}
          {/if}
        {:else if activeTab === 'skills'}
          <p class="mb-3 rounded-xl border border-slate-200/80 bg-slate-50/80 px-3 py-2 text-[11px] leading-relaxed text-slate-500 dark:border-[#30363d] dark:bg-[#161b22] dark:text-[#8b949e]">
            {t('ask.harness.skillsHint')}
          </p>

          {#if skillsError}
            <div role="alert" class="mb-3 rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2 text-xs text-rose-700 dark:text-rose-300">
              {skillsError}
            </div>
          {/if}

          {#if editingSkill}
            <form class="space-y-3" on:submit|preventDefault={submitSkillForm}>
              <div class="flex items-center justify-between">
                <button
                  type="button"
                  class="rounded-lg bg-indigo-500/10 px-2.5 py-1 text-[11px] font-medium text-indigo-600 transition hover:bg-indigo-500/20 dark:text-indigo-300"
                  on:click={fillWeeklyTemplate}
                >
                  {t('ask.harness.insertTemplate')}
                </button>
                <button
                  type="button"
                  class="text-xs text-slate-400 hover:text-slate-600 dark:text-[#8b949e]"
                  on:click={() => {
                    editingSkill = false;
                    editingExistingName = null;
                    skillsError = null;
                  }}
                >
                  {t('common.cancel')}
                </button>
              </div>

              <label class="block">
                <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">
                  {t('ask.harness.skillNameLabel')}
                </span>
                <input
                  type="text"
                  bind:value={formName}
                  disabled={editingExistingName !== null}
                  placeholder={t('ask.harness.skillNamePlaceholder')}
                  class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 font-mono text-xs text-slate-800 outline-none focus:border-indigo-400 disabled:opacity-60 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
                />
              </label>

              <label class="block">
                <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">
                  {t('ask.harness.skillDescLabel')}
                </span>
                <input
                  type="text"
                  bind:value={formDescription}
                  placeholder={t('ask.harness.skillDescPlaceholder')}
                  class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 text-xs text-slate-800 outline-none focus:border-indigo-400 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
                />
              </label>

              <label class="block">
                <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">
                  {t('ask.harness.skillToolsLabel')}
                </span>
                <input
                  type="text"
                  bind:value={formAllowedTools}
                  placeholder={t('ask.harness.skillToolsPlaceholder')}
                  class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 font-mono text-xs text-slate-800 outline-none focus:border-indigo-400 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
                />
              </label>

              <label class="block">
                <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">
                  {t('ask.harness.skillBodyLabel')}
                </span>
                <textarea
                  bind:value={formBody}
                  rows="7"
                  placeholder={t('ask.harness.skillBodyPlaceholder')}
                  class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-2 text-xs leading-relaxed text-slate-800 outline-none focus:border-indigo-400 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
                ></textarea>
              </label>

              <button
                type="submit"
                disabled={savingSkill}
                class="w-full rounded-lg bg-indigo-600 px-3 py-2 text-xs font-medium text-white transition hover:bg-indigo-500 disabled:opacity-50"
              >
                {savingSkill ? t('ask.harness.savingSkill') : t('ask.harness.saveSkill')}
              </button>
            </form>
          {:else}
            <div class="mb-3 flex items-center gap-2">
              <button
                type="button"
                class="rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-indigo-500"
                on:click={startNewSkill}
              >
                + {t('ask.harness.newSkill')}
              </button>
              <button
                type="button"
                class="rounded-lg border border-slate-200 px-3 py-1.5 text-xs font-medium text-slate-600 transition hover:bg-slate-100 dark:border-[#30363d] dark:text-[#c9d1d9] dark:hover:bg-[#21262d]"
                on:click={handleOpenSkillsDir}
              >
                {t('ask.harness.openSkillsDir')}
              </button>
            </div>

            {#if loadingSkills}
              <p role="status" aria-live="polite" class="py-6 text-center text-xs text-slate-400 dark:text-[#636c76]">{t('ask.harness.loading')}</p>
            {:else if skills.length === 0}
              <p class="py-8 text-center text-xs leading-relaxed text-slate-400 dark:text-[#636c76]">
                {t('ask.harness.skillsEmpty')}
              </p>
            {:else}
              <div class="space-y-2">
                {#each skills as skill (skill.name)}
                  <div class="rounded-xl border border-slate-200/80 bg-white/70 p-3 dark:border-[#30363d] dark:bg-[#161b22]/80">
                    <div class="flex items-start justify-between gap-2">
                      <div class="min-w-0">
                        <h4 class="truncate font-mono text-xs font-semibold text-slate-800 dark:text-[#e6edf3]">
                          {skill.name}
                        </h4>
                        <p class="mt-1 text-xs text-slate-500 dark:text-[#8b949e]">{skill.description}</p>
                      </div>
                      <div class="flex shrink-0 items-center gap-1">
                        {#if confirmDeleteSkillName === skill.name}
                          <button
                            type="button"
                            disabled={deletingSkillName !== null}
                            class="rounded bg-rose-600 px-2 py-1 text-[11px] font-medium text-white transition hover:bg-rose-500 disabled:opacity-50"
                            on:click={() => handleDeleteSkill(skill.name)}
                          >
                            {deletingSkillName === skill.name ? t('ask.harness.deletingSkill') : t('ask.harness.confirmDeleteSkill')}
                          </button>
                          <button
                            type="button"
                            disabled={deletingSkillName !== null}
                            class="rounded px-2 py-1 text-[11px] font-medium text-slate-500 hover:bg-slate-100 disabled:opacity-50 dark:text-[#8b949e] dark:hover:bg-[#21262d]"
                            on:click={() => (confirmDeleteSkillName = null)}
                          >
                            {t('common.cancel')}
                          </button>
                        {:else}
                          <button
                            type="button"
                            disabled={deletingSkillName !== null}
                            class="rounded px-2 py-1 text-[11px] font-medium text-indigo-600 hover:bg-indigo-500/10 disabled:opacity-50 dark:text-indigo-300"
                            on:click={() => startEditSkill(skill.name)}
                          >
                            {t('ask.harness.editSkill')}
                          </button>
                          <button
                            type="button"
                            disabled={deletingSkillName !== null}
                            class="rounded px-2 py-1 text-[11px] font-medium text-rose-600 hover:bg-rose-500/10 disabled:opacity-50 dark:text-rose-300"
                            on:click={() => (confirmDeleteSkillName = skill.name)}
                          >
                            {t('ask.harness.deleteSkill')}
                          </button>
                        {/if}
                      </div>
                    </div>
                    {#if skill.allowedTools?.length}
                      <div class="mt-2 flex flex-wrap gap-1">
                        {#each skill.allowedTools as tool}
                          <span class="rounded bg-slate-500/10 px-1.5 py-0.5 font-mono text-[10px] text-slate-600 dark:text-[#8b949e]">
                            {tool}
                          </span>
                        {/each}
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          {/if}
        {:else if activeTab === 'routines'}
          <AgentRoutinesPanel
            bind:this={routinesPanel}
            active={open && activeTab === 'routines'}
            {skills}
            onInspectRun={(runId) => {
              activeTab = 'runs';
              scopeAll = true;
              void openRunTrace(runId);
            }}
          />
        {:else}
          <AgentMcpPanel
            bind:this={mcpPanel}
            active={open && activeTab === 'mcp'}
          />
        {/if}
      </div>
    </div>
  </div>
{/if}
