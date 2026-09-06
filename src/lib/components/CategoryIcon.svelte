<script lang="ts">
  import { resolveCategoryIcon } from '$lib/utils/categoryIcons.ts';

  export let categoryKey = 'other';
  export let iconKey: string | null | undefined = undefined;
  export let size = 18;
  export let strokeWidth = 1.8;

  $: Icon = resolveCategoryIcon(categoryKey, iconKey);
</script>

<svelte:component
  this={Icon}
  {size}
  {strokeWidth}
  aria-hidden="true"
/>
