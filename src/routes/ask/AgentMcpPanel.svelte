<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { t } from '$lib/i18n/index.ts';
  import { formatUserError } from '$lib/utils/errorDisplay.ts';
  import {
    isValidMcpEndpointInput,
    isValidMcpServerIdInput,
    type McpClientServerView,
    type McpServerTestResult,
  } from './agentHarnessTypes.ts';

  export let active = false;

  let servers: McpClientServerView[] = [];
  let loading = false;
  let error: string | null = null;
  let editing = false;
  let editingExistingId: string | null = null;
  let editingOriginalEndpoint = '';
  let editingHadToken = false;
  let formId = '';
  let formName = '';
  let formEndpoint = '';
  let formToken = '';
  let formClearToken = false;
  let formAllowLocalhost = false;
  let formEnabled = true;
  let saving = false;

  let confirmDeleteId: string | null = null;
  let deletingId: string | null = null;
  let testingId: string | null = null;
  let testResults: Record<string, McpServerTestResult> = {};
  let reqSeq = 0;
  let wasActive = false;

  $: if (active && !wasActive) {
    wasActive = true;
    confirmDeleteId = null;
    void loadServers();
  } else if (!active && wasActive) {
    wasActive = false;
    editing = false;
    editingExistingId = null;
    confirmDeleteId = null;
    formToken = '';
    testResults = {};
  }

  export async function loadServers() {
    const seq = ++reqSeq;
    loading = true;
    error = null;
    try {
      const result = await invoke<McpClientServerView[]>('list_mcp_client_servers');
      if (seq !== reqSeq) return;
      servers = Array.isArray(result) ? result : [];
    } catch (e) {
      if (seq !== reqSeq) return;
      error = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (seq === reqSeq) loading = false;
    }
  }

  function startNewServer() {
    confirmDeleteId = null;
    editingExistingId = null;
    editingOriginalEndpoint = '';
    editingHadToken = false;
    formId = '';
    formName = '';
    formEndpoint = '';
    formToken = '';
    formClearToken = false;
    formAllowLocalhost = false;
    formEnabled = true;
    error = null;
    editing = true;
  }

  function startEditServer(server: McpClientServerView) {
    confirmDeleteId = null;
    editingExistingId = server.id;
    editingOriginalEndpoint = server.endpoint;
    editingHadToken = server.hasAuthToken;
    formId = server.id;
    formName = server.name;
    formEndpoint = server.endpoint;
    formToken = '';
    formClearToken = false;
    formAllowLocalhost = server.allowLocalhost;
    formEnabled = server.enabled;
    error = null;
    editing = true;
  }

  async function submitServer() {
    error = null;
    const id = (editingExistingId ?? formId).trim();
    if (!isValidMcpServerIdInput(id)) {
      error = t('ask.harness.skillNameInvalid');
      return;
    }
    if (!isValidMcpEndpointInput(formEndpoint, formAllowLocalhost)) {
      error = t('ask.harness.mcpEndpointInvalid');
      return;
    }
    if (!formName.trim()) {
      error = t('ask.harness.mcpNameLabel');
      return;
    }
    saving = true;
    try {
      // 显式清空传 ""；输入新值传新字符串；留空传 null（同地址保留原值，换地址时后端自动清空）。
      const tokenPayload = formClearToken
        ? ''
        : formToken.trim()
          ? formToken.trim()
          : null;
      const updated = await invoke<McpClientServerView[]>('save_mcp_client_server', {
        input: {
          id,
          name: formName.trim(),
          endpoint: formEndpoint.trim(),
          enabled: formEnabled,
          allowLocalhost: formAllowLocalhost,
          authToken: tokenPayload,
        },
      });
      servers = Array.isArray(updated) ? updated : [];
      editing = false;
      editingExistingId = null;
      formToken = '';
      formClearToken = false;
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
      const updated = await invoke<McpClientServerView[]>('delete_mcp_client_server', { id });
      servers = Array.isArray(updated) ? updated : [];
      confirmDeleteId = null;
      const nextResults = { ...testResults };
      delete nextResults[id];
      testResults = nextResults;
    } catch (e) {
      error = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (deletingId === id) deletingId = null;
    }
  }

  async function handleTest(id: string) {
    if (testingId !== null) return;
    error = null;
    testingId = id;
    try {
      const res = await invoke<McpServerTestResult>('test_mcp_client_server', { id });
      testResults = { ...testResults, [id]: res };
    } catch (e) {
      error = formatUserError(e, t('common.loadFailedRetry'));
    } finally {
      if (testingId === id) testingId = null;
    }
  }
</script>

<p class="mb-3 rounded-xl border border-slate-200/80 bg-slate-50/80 px-3 py-2 text-[11px] leading-relaxed text-slate-500 dark:border-[#30363d] dark:bg-[#161b22] dark:text-[#8b949e]">
  {t('ask.harness.mcpHint')}
</p>

{#if error}
  <div role="alert" class="mb-3 rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2 text-xs text-rose-700 dark:text-rose-300">
    {error}
  </div>
{/if}

{#if editing}
  <form class="space-y-3" on:submit|preventDefault={submitServer}>
    <div class="flex items-center justify-end">
      <button
        type="button"
        class="text-xs text-slate-400 hover:text-slate-600 dark:text-[#8b949e]"
        on:click={() => {
          editing = false;
          editingExistingId = null;
          formToken = '';
          formClearToken = false;
          error = null;
        }}
      >
        {t('common.cancel')}
      </button>
    </div>

    <label class="block">
      <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.mcpIdLabel')}</span>
      <input
        type="text"
        bind:value={formId}
        disabled={editingExistingId !== null}
        placeholder="obsidian"
        class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 font-mono text-xs text-slate-800 outline-none focus:border-indigo-400 disabled:opacity-60 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
      />
    </label>

    <label class="block">
      <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.mcpNameLabel')}</span>
      <input
        type="text"
        bind:value={formName}
        placeholder="Obsidian Local MCP"
        class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 text-xs text-slate-800 outline-none focus:border-indigo-400 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
      />
    </label>

    <label class="block">
      <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.mcpEndpointLabel')}</span>
      <input
        type="url"
        bind:value={formEndpoint}
        placeholder="https://mcp.example.com/rpc"
        class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 font-mono text-xs text-slate-800 outline-none focus:border-indigo-400 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
      />
      <p class="mt-1 text-[10px] text-slate-400 dark:text-[#8b949e]">
        {t('ask.harness.mcpPublicHttpsRequired')}
      </p>
    </label>

    <label class="block">
      <span class="mb-1 block text-[11px] font-medium text-slate-600 dark:text-[#c9d1d9]">{t('ask.harness.mcpTokenLabel')}</span>
      <input
        type="password"
        autocomplete="off"
        bind:value={formToken}
        disabled={formClearToken}
        class="w-full rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 font-mono text-xs text-slate-800 outline-none focus:border-indigo-400 disabled:opacity-50 dark:border-[#30363d] dark:bg-[#0d1117] dark:text-[#e6edf3]"
      />
    </label>

    {#if editingExistingId !== null && editingHadToken}
      <label class="flex items-center gap-2 text-xs text-slate-600 dark:text-[#c9d1d9]">
        <input type="checkbox" bind:checked={formClearToken} class="rounded" />
        <span>{t('ask.harness.mcpClearToken')}</span>
      </label>
      {#if formEndpoint.trim() !== editingOriginalEndpoint}
        <p class="text-[11px] text-amber-600 dark:text-amber-300">
          {t('ask.harness.mcpEndpointChangedTokenHint')}
        </p>
      {/if}
    {/if}

    <label class="flex items-center gap-2 text-xs text-slate-600 dark:text-[#c9d1d9]">
      <input type="checkbox" bind:checked={formAllowLocalhost} class="rounded" />
      <span>{t('ask.harness.mcpAllowLocalhost')}</span>
    </label>

    <label class="flex items-center gap-2 text-xs text-slate-600 dark:text-[#c9d1d9]">
      <input type="checkbox" bind:checked={formEnabled} class="rounded" />
      <span>{t('ask.harness.mcpEnabled')}</span>
    </label>

    <button
      type="submit"
      disabled={saving}
      class="w-full rounded-lg bg-indigo-600 px-3 py-2 text-xs font-medium text-white transition hover:bg-indigo-500 disabled:opacity-50"
    >
      {saving ? t('ask.harness.savingSkill') : t('ask.harness.saveMcp')}
    </button>
  </form>
{:else}
  <div class="mb-3">
    <button
      type="button"
      class="rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-indigo-500"
      on:click={startNewServer}
    >
      + {t('ask.harness.newMcp')}
    </button>
  </div>

  {#if loading}
    <p role="status" aria-live="polite" class="py-6 text-center text-xs text-slate-400 dark:text-[#636c76]">{t('ask.harness.loading')}</p>
  {:else if servers.length === 0}
    <p class="py-8 text-center text-xs leading-relaxed text-slate-400 dark:text-[#636c76]">{t('ask.harness.mcpEmpty')}</p>
  {:else}
    <div class="space-y-2">
      {#each servers as server (server.id)}
        {@const testRes = testResults[server.id]}
        <div class="rounded-xl border border-slate-200/80 bg-white/70 p-3 dark:border-[#30363d] dark:bg-[#161b22]/80">
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <div class="flex flex-wrap items-center gap-1.5">
                <h4 class="truncate text-xs font-semibold text-slate-800 dark:text-[#e6edf3]">{server.name}</h4>
                <span class="rounded bg-slate-500/10 px-1.5 py-0.5 font-mono text-[10px] text-slate-500 dark:text-[#8b949e]">{server.id}</span>
                {#if !server.enabled}
                  <span class="rounded bg-slate-500/12 px-1.5 py-0.5 text-[10px] text-slate-400">OFF</span>
                {/if}
                {#if server.hasAuthToken}
                  <span class="rounded bg-emerald-500/12 px-1.5 py-0.5 text-[10px] text-emerald-700 dark:text-emerald-300">{t('ask.harness.mcpTokenSet')}</span>
                {/if}
              </div>
              <p class="mt-1 truncate font-mono text-[11px] text-slate-500 dark:text-[#8b949e]">{server.endpoint}</p>
            </div>
            <div class="flex shrink-0 items-center gap-1">
              {#if confirmDeleteId === server.id}
                <button
                  type="button"
                  disabled={deletingId !== null}
                  class="rounded bg-rose-600 px-2 py-1 text-[11px] font-medium text-white hover:bg-rose-500 disabled:opacity-50"
                  on:click={() => handleDelete(server.id)}
                >
                  {deletingId === server.id ? t('ask.harness.deletingSkill') : t('ask.harness.confirmDeleteSkill')}
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
                  disabled={testingId !== null || deletingId !== null}
                  class="rounded px-2 py-1 text-[11px] font-medium text-emerald-600 hover:bg-emerald-500/10 disabled:opacity-50 dark:text-emerald-300"
                  on:click={() => handleTest(server.id)}
                >
                  {testingId === server.id ? t('ask.harness.testingMcp') : t('ask.harness.testMcp')}
                </button>
                <button
                  type="button"
                  disabled={deletingId !== null}
                  class="rounded px-2 py-1 text-[11px] font-medium text-indigo-600 hover:bg-indigo-500/10 disabled:opacity-50 dark:text-indigo-300"
                  on:click={() => startEditServer(server)}
                >
                  {t('ask.harness.editSkill')}
                </button>
                <button
                  type="button"
                  disabled={deletingId !== null}
                  class="rounded px-2 py-1 text-[11px] font-medium text-rose-600 hover:bg-rose-500/10 disabled:opacity-50 dark:text-rose-300"
                  on:click={() => (confirmDeleteId = server.id)}
                >
                  {t('ask.harness.deleteSkill')}
                </button>
              {/if}
            </div>
          </div>

          {#if testRes}
            <div class="mt-2 rounded-lg px-2.5 py-1.5 text-[11px] {testRes.ok
              ? 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
              : 'bg-rose-500/10 text-rose-700 dark:text-rose-300'}">
              <p>{testRes.message}</p>
              {#if testRes.toolNames.length > 0}
                <p class="mt-1 truncate font-mono text-[10px] opacity-85">{testRes.toolNames.join(', ')}</p>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
{/if}
