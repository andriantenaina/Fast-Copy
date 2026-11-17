<script lang="ts">
  import Modal from './Modal.svelte';
  import Icon from './Icon.svelte';
  import { app, fail } from './store.svelte';
  import { t, fmtBytes, middleTruncate } from './i18n.svelte';
  import { api, type PendingRequest } from './api';

  let { req }: { req: Extract<PendingRequest, { type: 'disk-full' }> } = $props();

  async function answer(action: 'retry' | 'skip' | 'abort') {
    app.requests = app.requests.filter((r) => r.requestId !== req.requestId);
    try {
      await api.resolveDiskFull(req.requestId, action);
    } catch (e) {
      fail(e);
    }
  }
</script>

<Modal title={t('diskfull.title')} width={520} tone="danger" dismissable={false}>
  <div class="hero">
    <span class="ic"><Icon name="drive" size={26} /></span>
    <p>{t('diskfull.text')}</p>
  </div>
  <p class="file mono muted" title={req.path}>{middleTruncate(req.path, 64)}</p>
  <div class="nums">
    <div><span class="muted">{t('diskfull.needed')}</span><strong class="num">{fmtBytes(req.needed)}</strong></div>
    <div><span class="muted">{t('diskfull.available')}</span><strong class="num sev-error">{fmtBytes(req.available)}</strong></div>
  </div>
  <p class="muted hint">{t('diskfull.hint')}</p>

  {#snippet footer()}
    <button class="btn danger" onclick={() => answer('abort')}>{t('diskfull.abort')}</button>
    <button class="btn" onclick={() => answer('skip')}>{t('diskfull.skip')}</button>
    <button class="btn primary" onclick={() => answer('retry')} data-autofocus><Icon name="refresh" size={15} />{t('diskfull.retry')}</button>
  {/snippet}
</Modal>

<style>
  .hero {
    display: flex;
    gap: 14px;
    align-items: center;
  }
  .hero p {
    margin: 0;
    font-size: 15px;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    border-radius: 14px;
    color: var(--red);
    background: rgba(244, 63, 94, 0.14);
    flex: none;
  }
  .file {
    font-size: 12px;
    margin: 12px 0;
    word-break: break-all;
  }
  .nums {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .nums div {
    display: grid;
    gap: 2px;
    padding: 12px;
    border-radius: 12px;
    background: rgba(5, 8, 24, 0.45);
    border: 1px solid var(--line);
    font-size: 12.5px;
  }
  .nums strong {
    font-size: 20px;
    font-family: var(--font-display);
  }
  .hint {
    font-size: 12.5px;
    margin: 12px 0 0;
  }
</style>
