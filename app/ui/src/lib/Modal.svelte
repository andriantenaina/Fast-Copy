<script lang="ts">
  import type { Snippet } from 'svelte';
  import { t } from './i18n.svelte';

  let {
    title,
    onclose,
    tone = 'default',
    width = 560,
    dismissable = true,
    children,
    footer,
  }: {
    title: string;
    onclose?: () => void;
    tone?: 'default' | 'danger' | 'warning';
    width?: number;
    dismissable?: boolean;
    children: Snippet;
    footer?: Snippet;
  } = $props();

  let dialog: HTMLDivElement;
  const id = `m${Math.random().toString(36).slice(2, 8)}`;

  $effect(() => {
    const prev = document.activeElement as HTMLElement | null;
    queueMicrotask(() => {
      const first = dialog.querySelector<HTMLElement>('[data-autofocus]') ?? dialog.querySelector<HTMLElement>('button, input, select, [tabindex]:not([tabindex="-1"])');
      first?.focus();
    });
    return () => prev?.focus?.();
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && dismissable && onclose) {
      e.stopPropagation();
      onclose();
    }
    if (e.key === 'Tab') {
      const items = Array.from(dialog.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])'));
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && dismissable && onclose?.()}>
  <div
    bind:this={dialog}
    class="dialog glass tone-{tone}"
    style="max-width:{width}px"
    role="dialog"
    aria-modal="true"
    aria-labelledby={id}
    tabindex="-1"
    onkeydown={onKey}>
    <header>
      <h2 {id}>{title}</h2>
      {#if dismissable && onclose}
        <button class="btn sm icon ghost" aria-label={t("common.close")} onclick={onclose}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12" /></svg>
        </button>
      {/if}
    </header>
    <div class="body">{@render children()}</div>
    {#if footer}<footer>{@render footer()}</footer>{/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    padding: 24px;
    background: rgba(2, 4, 12, 0.6);
    -webkit-backdrop-filter: blur(6px);
    backdrop-filter: blur(6px);
    animation: fade 0.25s var(--ease) both;
  }
  .dialog {
    width: 100%;
    max-height: calc(100vh - 80px);
    display: flex;
    flex-direction: column;
    background: var(--glass-strong);
    border-color: var(--line-strong);
    box-shadow: 0 40px 100px -30px rgba(0, 0, 0, 0.9), 0 0 0 1px rgba(255, 255, 255, 0.03) inset, 0 0 60px -30px rgba(34, 211, 238, 0.5);
    animation: pop 0.4s var(--spring) both;
  }
  .tone-danger {
    border-color: rgba(251, 113, 133, 0.45);
    box-shadow: 0 40px 100px -30px rgba(0, 0, 0, 0.9), 0 0 70px -30px rgba(244, 63, 94, 0.6);
  }
  .tone-warning {
    border-color: rgba(251, 191, 36, 0.4);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 18px 20px 6px;
  }
  h2 {
    font-size: 18px;
  }
  .body {
    padding: 10px 20px 18px;
    overflow: auto;
  }
  footer {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    padding: 14px 20px 18px;
    border-top: 1px solid var(--line);
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(16px) scale(0.96);
    }
  }
</style>
