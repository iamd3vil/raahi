<script lang="ts">
  import { toasts, dismiss } from '../state.svelte';
</script>

<div class="toasts">
  {#each toasts as t (t.id)}
    <button class="toast {t.kind}" onclick={() => dismiss(t.id)}>
      <span class="dot {t.kind === 'ok' ? 'ok' : t.kind === 'err' ? 'err' : ''}"></span>
      <span>{t.msg}</span>
    </button>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    bottom: 20px;
    right: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 1000;
  }
  .toast {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 11px 14px;
    border-radius: var(--r-md);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    color: var(--text);
    box-shadow: var(--shadow-2);
    font: inherit;
    cursor: pointer;
    max-width: 360px;
    text-align: left;
    animation: slide 0.2s ease;
  }
  .toast.err {
    border-color: rgba(251, 113, 133, 0.45);
  }
  .toast.ok {
    border-color: rgba(52, 211, 153, 0.45);
  }
  @keyframes slide {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
  }
</style>
