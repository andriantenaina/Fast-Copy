<script lang="ts">
  import Icon from './Icon.svelte';
  import { app, dismissToast } from './store.svelte';
  import { t } from './i18n.svelte';
</script>

<div class="toasts" role="status" aria-live="polite">
  {#each app.toasts as x (x.id)}
    <div class="toast glass k-{x.kind}">
      <Icon name={x.kind === 'error' ? 'alert-circle' : x.kind === 'success' ? 'check' : 'info'} size={17} />
      <p class="selectable">{x.text}</p>
      <button class="btn sm icon ghost" aria-label={t('common.close')} onclick={() => dismissToast(x.id)}><Icon name="x" size={14} /></button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 18px;
    bottom: 18px;
    z-index: 200;
    display: grid;
    gap: 8px;
    width: min(420px, calc(100vw - 36px));
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 8px 10px 14px;
    background: var(--glass-strong);
    border-color: var(--line-strong);
    animation: slide 0.4s var(--spring) both;
  }
  .toast p {
    flex: 1;
    margin: 1px 0 0;
    font-size: 13px;
    word-break: break-word;
  }
  .toast > :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .k-error {
    border-color: rgba(251, 113, 133, 0.5);
  }
  .k-error > :global(svg) {
    color: var(--red);
  }
  .k-success > :global(svg) {
    color: var(--green);
  }
  .k-info > :global(svg) {
    color: var(--cyan);
  }
  @keyframes slide {
    from {
      opacity: 0;
      transform: translateX(30px);
    }
  }
</style>
