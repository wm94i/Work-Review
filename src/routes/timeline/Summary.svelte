<script lang="ts">
  import { onMount } from 'svelte';
  import { replace, router } from 'svelte-spa-router';
  import { t } from '$lib/i18n/index.ts';
  import { isValidLocalDateString } from '$lib/utils/dateValidation.ts';

  function getLocalDateString() {
    const now = new Date();
    return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
  }

  onMount(() => {
    const hash = typeof window !== 'undefined' ? window.location.hash : '';
    const match = hash.match(/summary\/(\d{4}-\d{2}-\d{2})(?:[/?#]|$)/);
    const routeParams = router.params;
    const paramDate =
      routeParams && !Array.isArray(routeParams) ? routeParams.date : undefined;
    const routeDate = match?.[1] ?? paramDate;
    const date = isValidLocalDateString(routeDate) ? routeDate : getLocalDateString();
    const target = `/timeline?date=${date}&summary=1`;
    replace(target);
  });
</script>

<div class="summary-route-redirect" aria-live="polite">
  <span class="summary-route-spinner" aria-hidden="true"></span>
  <p>{t('timelineSummary.title')}</p>
</div>

<style>
  .summary-route-redirect {
    min-height: 18rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.8rem;
    color: #78716c;
    text-align: center;
  }

  .summary-route-redirect p {
    margin: 0;
  }

  .summary-route-spinner {
    width: 1.25rem;
    height: 1.25rem;
    border: 2px solid rgba(120, 113, 108, 0.24);
    border-top-color: #d97706;
    border-radius: var(--radius-full);
    animation: summary-route-spin 700ms linear infinite;
  }

  :global(.dark) .summary-route-redirect {
    color: #8b949e;
  }

  @keyframes summary-route-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
