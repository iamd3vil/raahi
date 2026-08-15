<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    icon,
    title,
    description,
    action,
  }: { icon?: string; title: string; description?: string; action?: Snippet } = $props();
</script>

<div class="empty-state">
  {#if icon}
    <svg viewBox="0 0 24 24" width="36" height="36" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
      <path d={icon} />
    </svg>
  {/if}
  <h3>{title}</h3>
  {#if description}
    <p>{description}</p>
  {/if}
  {#if action}
    {@render action()}
  {/if}
</div>
