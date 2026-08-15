<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    open = $bindable(false),
    title,
    onclose,
    children,
    footer,
  }: {
    open?: boolean;
    title: string;
    onclose?: () => void;
    children: Snippet;
    footer?: Snippet;
  } = $props();

  function close() {
    open = false;
    onclose?.();
  }
</script>

<svelte:window onkeydown={(e) => open && e.key === 'Escape' && close()} />

{#if open}
  <div class="overlay" onclick={(e) => e.target === e.currentTarget && close()} role="presentation">
    <div class="drawer" role="dialog" aria-modal="true" tabindex="-1">
      <div class="head">
        <h2>{title}</h2>
        <button class="btn btn-ghost btn-sm" onclick={close} aria-label="Close">✕</button>
      </div>
      <div class="body">
        {@render children()}
      </div>
      {#if footer}
        <div class="foot">{@render footer()}</div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    backdrop-filter: blur(2px);
    display: flex;
    justify-content: flex-end;
    z-index: 900;
    animation: fade 0.15s ease;
  }
  .drawer {
    width: min(480px, 100%);
    height: 100%;
    background: var(--surface);
    border-left: 1px solid var(--border-strong);
    box-shadow: var(--shadow-2);
    display: flex;
    flex-direction: column;
    animation: slidein 0.2s cubic-bezier(0.22, 1, 0.36, 1);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 18px 20px;
    border-bottom: 1px solid var(--border);
  }
  .head h2 {
    font-size: 16px;
  }
  .body {
    padding: 20px;
    overflow-y: auto;
    flex: 1;
  }
  .foot {
    padding: 16px 20px;
    border-top: 1px solid var(--border);
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }
  @keyframes slidein {
    from {
      transform: translateX(30px);
      opacity: 0.4;
    }
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
