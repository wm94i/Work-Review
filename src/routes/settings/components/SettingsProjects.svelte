<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { cache } from '../../../lib/stores/cache.ts';
  import { formatDurationLocalized, locale, t } from '$lib/i18n/index.ts';
  import { showToast } from '$lib/stores/toast.ts';
  import { formatUserError } from '$lib/utils/errorDisplay.ts';

  type ProjectRuleField = 'app' | 'title' | 'domain' | 'url_path' | 'executable_path';
  type ProjectRuleOp = 'equals' | 'contains' | 'regex';

  interface ProjectRule {
    field: ProjectRuleField;
    op: ProjectRuleOp;
    value: string;
  }

  interface DraftProjectRule extends ProjectRule {
    uid: number;
  }

  interface ProjectDefinition {
    key: string;
    name: string;
    color: string;
    archived: boolean;
    rules: ProjectRule[];
  }

  interface ProjectRulePreviewSample {
    date: string;
    appName: string;
    windowTitle: string;
    browserUrl: string | null;
    duration: number;
  }

  interface ProjectRulePreviewResult {
    dateFrom: string;
    dateTo: string;
    matchedCount: number;
    totalDuration: number;
    sampleActivities: ProjectRulePreviewSample[];
  }

  interface ProjectsConfig {
    projects?: ProjectDefinition[];
  }

  export let config: ProjectsConfig;

  const dispatch = createEventDispatcher<{
    change: { autosaved: true };
  }>();

  const COLOR_PRESETS = [
    '#3B82F6',
    '#10B981',
    '#F59E0B',
    '#8B5CF6',
    '#EC4899',
    '#06B6D4',
    '#EF4444',
    '#6366F1',
  ];

  $: currentLocale = $locale;

  let projects: ProjectDefinition[] = [];
  let loading = true;
  let saving = false;
  let projectsSyncVersion = 0;
  let editing = false;
  let editingIsNew = true;
  let draftKey = '';
  let draftName = '';
  let draftColor = '#3B82F6';
  let draftArchived = false;
  let draftRules: DraftProjectRule[] = [];
  let nextDraftRuleUid = 1;

  let previewingUid: number | null = null;
  let previewResult: ProjectRulePreviewResult | null = null;
  let previewRuleUid: number | null = null;
  let previewError = '';
  let previewRequestId = 0;

  function makeDraftRule(rule: ProjectRule): DraftProjectRule {
    return { ...rule, uid: nextDraftRuleUid++ };
  }

  function syncProjectsToConfig(nextProjects: ProjectDefinition[]) {
    projectsSyncVersion += 1;
    loading = false;
    projects = nextProjects;
    config.projects = nextProjects;
    cache.invalidate('overview');
    dispatch('change', { autosaved: true });
  }

  async function loadProjects() {
    const version = ++projectsSyncVersion;
    loading = true;
    try {
      const loaded = await invoke<ProjectDefinition[]>('get_projects');
      if (version !== projectsSyncVersion) return;
      projects = loaded;
      config.projects = loaded;
    } catch (e) {
      if (version !== projectsSyncVersion) return;
      showToast(formatUserError(e, t('common.loadFailedRetry')), 'error');
    } finally {
      if (version === projectsSyncVersion) {
        loading = false;
      }
    }
  }

  function startCreate() {
    editing = true;
    editingIsNew = true;
    draftKey = '';
    draftName = '';
    draftColor = COLOR_PRESETS[projects.length % COLOR_PRESETS.length];
    draftArchived = false;
    draftRules = [makeDraftRule({ field: 'title', op: 'contains', value: '' })];
    clearPreview();
  }

  function startEdit(project: ProjectDefinition) {
    editing = true;
    editingIsNew = false;
    draftKey = project.key;
    draftName = project.name;
    draftColor = project.color || '#3B82F6';
    draftArchived = Boolean(project.archived);
    draftRules = project.rules.length > 0
      ? project.rules.map((r) => makeDraftRule(r))
      : [makeDraftRule({ field: 'title', op: 'contains', value: '' })];
    clearPreview();
  }

  function cancelEdit() {
    editing = false;
    clearPreview();
  }

  function clearPreview() {
    previewRequestId += 1;
    previewingUid = null;
    previewResult = null;
    previewRuleUid = null;
    previewError = '';
  }

  function invalidatePreviewForRule(uid: number) {
    if (previewRuleUid === uid || previewingUid === uid) {
      clearPreview();
    }
  }

  function addRuleRow() {
    draftRules = [...draftRules, makeDraftRule({ field: 'app', op: 'contains', value: '' })];
  }

  function removeRuleRow(uid: number) {
    draftRules = draftRules.filter((r) => r.uid !== uid);
    invalidatePreviewForRule(uid);
  }

  async function previewRule(uid: number) {
    const rule = draftRules.find((r) => r.uid === uid);
    if (!rule || !rule.value.trim()) {
      previewRequestId += 1;
      previewingUid = null;
      previewError = t('settingsProjects.ruleValueRequired');
      previewResult = null;
      previewRuleUid = uid;
      return;
    }
    const reqId = ++previewRequestId;
    previewingUid = uid;
    previewRuleUid = uid;
    previewError = '';
    previewResult = null;
    try {
      const result = await invoke<ProjectRulePreviewResult>('preview_project_rule', {
        rule: {
          field: rule.field,
          op: rule.op,
          value: rule.value.trim(),
        },
        date: null,
      });
      if (reqId !== previewRequestId) return;
      previewResult = result;
    } catch (e) {
      if (reqId !== previewRequestId) return;
      previewError = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (reqId === previewRequestId) {
        previewingUid = null;
      }
    }
  }

  async function saveDraft() {
    if (saving) return;
    const name = draftName.trim();
    if (!name) {
      showToast(t('settingsProjects.nameRequired'), 'error');
      return;
    }
    const cleanedRules = draftRules
      .map((r) => ({
        field: r.field,
        op: r.op,
        value: r.value.trim(),
      }))
      .filter((r) => r.value.length > 0);

    saving = true;
    try {
      const updated = await invoke<ProjectDefinition[]>('save_project', {
        project: {
          key: editingIsNew ? '' : draftKey,
          name,
          color: draftColor,
          archived: draftArchived,
          rules: cleanedRules,
        },
      });
      syncProjectsToConfig(updated);
      editing = false;
      clearPreview();
      showToast(t('settingsProjects.saved'), 'success');
    } catch (e) {
      showToast(formatUserError(e, t('common.loadFailedRetry')), 'error');
    } finally {
      saving = false;
    }
  }

  async function toggleArchive(project: ProjectDefinition) {
    if (saving) return;
    saving = true;
    try {
      const updated = await invoke<ProjectDefinition[]>('save_project', {
        project: {
          ...project,
          archived: !project.archived,
        },
      });
      syncProjectsToConfig(updated);
      showToast(t('settingsProjects.saved'), 'success');
    } catch (e) {
      showToast(formatUserError(e, t('common.loadFailedRetry')), 'error');
    } finally {
      saving = false;
    }
  }

  async function removeProject(project: ProjectDefinition) {
    if (saving) return;
    const confirmed = await ask(
      t('settingsProjects.deleteConfirmMessage', { name: project.name }),
      {
        title: t('settingsProjects.deleteConfirmTitle'),
        kind: 'warning',
      },
    );
    if (!confirmed) return;

    saving = true;
    try {
      const updated = await invoke<ProjectDefinition[]>('delete_project', {
        key: project.key,
      });
      syncProjectsToConfig(updated);
      if (editing && !editingIsNew && draftKey === project.key) {
        editing = false;
        clearPreview();
      }
      showToast(t('settingsProjects.deleted'), 'success');
    } catch (e) {
      showToast(formatUserError(e, t('common.loadFailedRetry')), 'error');
    } finally {
      saving = false;
    }
  }

  function fieldLabel(field: ProjectRuleField): string {
    switch (field) {
      case 'app':
        return t('settingsProjects.fieldApp');
      case 'title':
        return t('settingsProjects.fieldTitle');
      case 'domain':
        return t('settingsProjects.fieldDomain');
      case 'url_path':
        return t('settingsProjects.fieldUrlPath');
      case 'executable_path':
        return t('settingsProjects.fieldExecutablePath');
    }
  }

  function opLabel(op: ProjectRuleOp): string {
    switch (op) {
      case 'contains':
        return t('settingsProjects.opContains');
      case 'equals':
        return t('settingsProjects.opEquals');
      case 'regex':
        return t('settingsProjects.opRegex');
    }
  }

  onMount(() => {
    void loadProjects();
  });
</script>

<div class="settings-card mb-5" data-locale={currentLocale}>
  <div class="flex flex-wrap items-start justify-between gap-3 mb-4">
    <div class="min-w-0">
      <h3 class="settings-card-title !mb-1">{t('settingsProjects.title')}</h3>
      <p class="settings-muted">{t('settingsProjects.description')}</p>
    </div>
    {#if !editing}
      <button
        type="button"
        on:click={startCreate}
        disabled={loading || saving}
        class="settings-action-primary shrink-0 disabled:opacity-50"
      >
        {t('settingsProjects.addProject')}
      </button>
    {/if}
  </div>

  {#if editing}
    <div class="settings-subsection space-y-4 mb-5">
      <div class="flex items-center justify-between gap-2">
        <h4 class="settings-text font-semibold">
          {editingIsNew ? t('settingsProjects.createTitle') : t('settingsProjects.editTitle')}
        </h4>
        <button
          type="button"
          on:click={cancelEdit}
          class="settings-link-action"
        >
          {t('common.cancel')}
        </button>
      </div>

      <div class="grid grid-cols-1 gap-3 md:grid-cols-[minmax(0,1fr)_auto]">
        <label class="block">
          <span class="settings-label">{t('settingsProjects.nameLabel')}</span>
          <input
            type="text"
            bind:value={draftName}
            maxlength="64"
            placeholder={t('settingsProjects.namePlaceholder')}
            class="control-input mt-1"
          />
        </label>

        <div>
          <span class="settings-label block">{t('settingsProjects.colorLabel')}</span>
          <div class="mt-1.5 flex flex-wrap items-center gap-1.5">
            {#each COLOR_PRESETS as swatch}
              <button
                type="button"
                aria-label={swatch}
                on:click={() => (draftColor = swatch)}
                class="h-6 w-6 rounded-md border transition-transform {draftColor.toLowerCase() === swatch.toLowerCase() ? 'scale-110 border-slate-900 dark:border-white ring-2 ring-primary-400/50' : 'border-slate-200 dark:border-[#484f58]'}"
                style={`background-color: ${swatch};`}
              ></button>
            {/each}
            <input
              type="color"
              bind:value={draftColor}
              aria-label={t('settingsProjects.colorLabel')}
              class="h-6 w-7 cursor-pointer rounded border border-slate-200 bg-transparent p-0 dark:border-[#484f58]"
            />
          </div>
        </div>
      </div>

      <label class="inline-flex items-center gap-2 text-xs settings-muted cursor-pointer select-none">
        <input
          type="checkbox"
          bind:checked={draftArchived}
          class="h-4 w-4 rounded border-slate-300 dark:border-[#484f58]"
        />
        <span>{t('settingsProjects.archivedLabel')}</span>
      </label>

      <div class="space-y-2.5 border-t border-slate-200/70 pt-3 dark:border-[#30363d]/80">
        <div class="flex items-center justify-between gap-2">
          <div>
            <span class="settings-label">{t('settingsProjects.rulesTitle')}</span>
            <p class="settings-muted mt-0.5">{t('settingsProjects.rulesHint')}</p>
          </div>
          <button
            type="button"
            on:click={addRuleRow}
            class="settings-action-secondary shrink-0"
          >
            {t('settingsProjects.addRule')}
          </button>
        </div>

        {#if draftRules.length === 0}
          <p class="settings-muted">{t('settingsProjects.noRulesHint')}</p>
        {:else}
          <div class="space-y-2">
            {#each draftRules as rule (rule.uid)}
              <div class="flex flex-wrap items-center gap-2 rounded-lg border border-slate-200/80 bg-slate-50/70 p-2.5 dark:border-[#30363d] dark:bg-[#161b22]/50">
                <select
                  bind:value={rule.field}
                  on:change={() => invalidatePreviewForRule(rule.uid)}
                  aria-label={t('settingsProjects.ruleFieldAria')}
                  class="rounded-md border border-slate-200 bg-white px-2.5 py-1.5 text-xs text-slate-700 dark:border-[#484f58] dark:bg-[#21262d] dark:text-[#c9d1d9]"
                >
                  <option value="title">{t('settingsProjects.fieldTitle')}</option>
                  <option value="app">{t('settingsProjects.fieldApp')}</option>
                  <option value="domain">{t('settingsProjects.fieldDomain')}</option>
                  <option value="url_path">{t('settingsProjects.fieldUrlPath')}</option>
                  <option value="executable_path">{t('settingsProjects.fieldExecutablePath')}</option>
                </select>

                <select
                  bind:value={rule.op}
                  on:change={() => invalidatePreviewForRule(rule.uid)}
                  aria-label={t('settingsProjects.ruleOpAria')}
                  class="rounded-md border border-slate-200 bg-white px-2.5 py-1.5 text-xs text-slate-700 dark:border-[#484f58] dark:bg-[#21262d] dark:text-[#c9d1d9]"
                >
                  <option value="contains">{t('settingsProjects.opContains')}</option>
                  <option value="equals">{t('settingsProjects.opEquals')}</option>
                  <option value="regex">{t('settingsProjects.opRegex')}</option>
                </select>

                <input
                  type="text"
                  bind:value={rule.value}
                  on:input={() => invalidatePreviewForRule(rule.uid)}
                  placeholder={t('settingsProjects.ruleValuePlaceholder')}
                  aria-label={t('settingsProjects.ruleValuePlaceholder')}
                  class="min-w-[10rem] flex-1 rounded-md border border-slate-200 bg-white px-2.5 py-1.5 text-xs text-slate-900 dark:border-[#484f58] dark:bg-[#21262d] dark:text-[#e6edf3]"
                />

                <button
                  type="button"
                  on:click={() => previewRule(rule.uid)}
                  disabled={previewingUid === rule.uid}
                  class="settings-action-secondary !py-1 !px-2.5 text-xs"
                >
                  {previewingUid === rule.uid
                    ? t('settingsProjects.previewing')
                    : t('settingsProjects.previewRule')}
                </button>

                <button
                  type="button"
                  on:click={() => removeRuleRow(rule.uid)}
                  class="settings-link-danger text-xs"
                >
                  {t('settingsProjects.removeRule')}
                </button>
              </div>
            {/each}
          </div>
        {/if}

        {#if previewError}
          <p class="text-xs text-rose-500 dark:text-rose-400">{previewError}</p>
        {:else if previewResult}
          <div class="rounded-lg border border-slate-200/80 bg-white/80 p-3 text-xs dark:border-[#30363d] dark:bg-[#0d1117]/60">
            <p class="font-medium text-slate-700 dark:text-[#c9d1d9]">
              {t('settingsProjects.previewSummary', {
                from: previewResult.dateFrom,
                to: previewResult.dateTo,
                count: previewResult.matchedCount,
                duration: formatDurationLocalized(previewResult.totalDuration, { compact: true }),
              })}
            </p>
            {#if previewResult.sampleActivities.length > 0}
              <ul class="mt-2 space-y-1 text-slate-500 dark:text-[#7d8590]">
                {#each previewResult.sampleActivities.slice(0, 6) as sample}
                  <li class="flex items-center justify-between gap-2">
                    <span class="min-w-0 truncate">
                      <span class="font-mono text-[11px] opacity-75">{sample.date}</span>
                      <span class="mx-1 font-medium text-slate-700 dark:text-[#c9d1d9]">{sample.appName}</span>
                      <span>{sample.windowTitle}</span>
                    </span>
                    <span class="shrink-0 tabular-nums">
                      {formatDurationLocalized(sample.duration, { compact: true })}
                    </span>
                  </li>
                {/each}
              </ul>
            {:else}
              <p class="mt-1.5 settings-muted">{t('settingsProjects.previewEmpty')}</p>
            {/if}
          </div>
        {/if}
      </div>

      <div class="flex items-center justify-end gap-2 pt-2">
        <button
          type="button"
          on:click={cancelEdit}
          disabled={saving}
          class="settings-action-secondary"
        >
          {t('common.cancel')}
        </button>
        <button
          type="button"
          on:click={saveDraft}
          disabled={saving}
          class="settings-action-primary"
        >
          {saving ? t('settings.saving') : t('settingsProjects.saveProject')}
        </button>
      </div>
    </div>
  {/if}

  {#if loading}
    <p class="settings-muted py-4 text-center">{t('common.loading')}</p>
  {:else if projects.length === 0}
    <div class="rounded-xl border border-dashed border-slate-200/90 p-5 text-center dark:border-[#30363d]">
      <p class="settings-text">{t('settingsProjects.emptyTitle')}</p>
      <p class="settings-muted mt-1">{t('settingsProjects.emptyHint')}</p>
    </div>
  {:else}
    <ul class="divide-y divide-slate-200/70 dark:divide-[#30363d]/70">
      {#each projects as project (project.key)}
        <li class="py-3 flex flex-wrap items-start justify-between gap-3">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span
                class="h-2.5 w-2.5 shrink-0 rounded-[var(--radius-xs)]"
                style={`background-color: ${project.color || '#3B82F6'};`}
              ></span>
              <span class="settings-text font-medium truncate">{project.name}</span>
              {#if project.archived}
                <span class="rounded border border-slate-200 bg-slate-100 px-1.5 py-0.5 text-[10px] text-slate-500 dark:border-[#30363d] dark:bg-[#21262d] dark:text-[#8b949e]">
                  {t('settingsProjects.archivedBadge')}
                </span>
              {/if}
              <span class="text-xs settings-subtle">
                {t('settingsProjects.rulesCount', { count: project.rules.length })}
              </span>
            </div>
            {#if project.rules.length > 0}
              <div class="mt-1.5 flex flex-wrap gap-1.5">
                {#each project.rules.slice(0, 4) as rule}
                  <span class="rounded bg-slate-100 px-2 py-0.5 text-[11px] text-slate-600 dark:bg-[#21262d] dark:text-[#adbac7]">
                    {fieldLabel(rule.field)} · {opLabel(rule.op)}: {rule.value}
                  </span>
                {/each}
                {#if project.rules.length > 4}
                  <span class="text-[11px] settings-subtle">+{project.rules.length - 4}</span>
                {/if}
              </div>
            {/if}
          </div>

          <div class="flex shrink-0 items-center gap-2">
            <button
              type="button"
              on:click={() => startEdit(project)}
              disabled={saving}
              class="settings-link-action"
            >
              {t('settingsProjects.edit')}
            </button>
            <button
              type="button"
              on:click={() => toggleArchive(project)}
              disabled={saving || (editing && !editingIsNew && draftKey === project.key)}
              class="settings-link-action disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {project.archived ? t('settingsProjects.unarchive') : t('settingsProjects.archive')}
            </button>
            <button
              type="button"
              on:click={() => removeProject(project)}
              disabled={saving || (editing && !editingIsNew && draftKey === project.key)}
              class="settings-link-danger disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {t('settingsProjects.delete')}
            </button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}

  <p class="settings-note mt-4">{t('settingsProjects.priorityNote')}</p>
</div>
