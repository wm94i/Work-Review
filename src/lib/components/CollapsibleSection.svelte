<script lang="ts">
  export let title = '';
  export let subtitle = '';
  export let storageKey = '';
  export let defaultOpen = false;

  let open = defaultOpen;
  if (storageKey && typeof localStorage !== 'undefined') {
    const saved = localStorage.getItem(storageKey);
    if (saved !== null) open = saved === '1';
  }
  let everOpened = open;

  function toggle() {
    open = !open;
    if (open) everOpened = true;
    if (storageKey && typeof localStorage !== 'undefined') {
      localStorage.setItem(storageKey, open ? '1' : '0');
    }
  }
</script>

<div class="settings-card">
  <button
    type="button"
    class="flex w-full items-center justify-between gap-3 text-left"
    aria-expanded={open}
    on:click={toggle}
  >
    <div class="min-w-0">
      <p class="settings-card-title">{title}</p>
      {#if subtitle}<p class="settings-card-desc">{subtitle}</p>{/if}
    </div>
    <svg
      class="h-4 w-4 shrink-0 text-slate-400 dark:text-[#7d8590] transition-transform duration-200 {open ? 'rotate-180' : ''}"
      fill="none"
      stroke="currentColor"
      viewBox="0 0 24 24"
    >
      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
    </svg>
  </button>
  <div class="ui-fold {open ? 'ui-fold-open' : ''}" aria-hidden={!open} inert={!open ? true : undefined}>
    <div class="ui-fold-inner">
      {#if everOpened}
        <div class="mt-3 settings-block">
          <slot />
        </div>
      {/if}
    </div>
  </div>
</div>
