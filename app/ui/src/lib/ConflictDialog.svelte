<script lang="ts">
  import Modal from './Modal.svelte';
  import Icon from './Icon.svelte';
  import { app, fail } from './store.svelte';
  import { t, fmtBytes, fmtDate, baseName, middleTruncate } from './i18n.svelte';
  import { api, type OnConflict, type PendingRequest } from './api';

  let { req }: { req: Extract<PendingRequest, { type: 'conflict' }> } = $props();

  let applyAll = $state(false);
  const info = $derived(req.info);
  const srcNewer = $derived(info.src_mtime_ms > info.dst_mtime_ms + 2000);
  const dstNewer = $derived(info.dst_mtime_ms > info.src_mtime_ms + 2000);
  const same = $derived(info.src_size === info.dst_size && !srcNewer && !dstNewer);
  const others = $derived(app.requests.filter((r) => r.type === 'conflict').length - 1);

  async function answer(action: OnConflict) {
    const id = req.requestId;
    app.requests = app.requests.filter((r) => r.requestId !== id);
    if (applyAll) app.requests = app.requests.filter((r) => r.type !== 'conflict');
    try {
      await api.resolveConflict(id, action, applyAll);
    } catch (e) {
      fail(e);
    }
  }
</script>

<Modal title={t('conflict.title')} width={640} tone="warning" dismissable={false}>
  <div class="file">
    <span class="ic"><Icon name="file" size={22} /></span>
    <div class="names">
      <strong class="selectable">{baseName(info.path)}</strong>
      <span class="muted mono" title={info.path}>{middleTruncate(info.path, 70)}</span>
    </div>
  </div>

  <div class="compare">
    <div class="side" class:newer={srcNewer}>
      <span class="lbl">{t('conflict.source')}</span>
      {#if srcNewer}<span class="tag">{t('conflict.newer')}</span>{/if}
      <dl>
        <dt>{t('conflict.size')}</dt><dd class="num">{fmtBytes(info.src_size)}</dd>
        <dt>{t('conflict.modified')}</dt><dd class="num">{fmtDate(info.src_mtime_ms)}</dd>
      </dl>
    </div>
    <div class="vs" aria-hidden="true">VS</div>
    <div class="side" class:newer={dstNewer}>
      <span class="lbl">{t('conflict.dest')}</span>
      {#if dstNewer}<span class="tag">{t('conflict.newer')}</span>{/if}
      <dl>
        <dt>{t('conflict.size')}</dt><dd class="num">{fmtBytes(info.dst_size)}</dd>
        <dt>{t('conflict.modified')}</dt><dd class="num">{fmtDate(info.dst_mtime_ms)}</dd>
      </dl>
    </div>
  </div>
  {#if same}<p class="same muted"><Icon name="info" size={14} />{t('conflict.identical')}</p>{/if}

  <div class="choices">
    <button class="btn primary" onclick={() => answer('overwrite')} data-autofocus><Icon name="refresh" size={15} />{t('conflict.replace')}</button>
    <button class="btn" onclick={() => answer('skip')}><Icon name="skip" size={15} />{t('conflict.skip')}</button>
    <button class="btn" onclick={() => answer('keep-both')}><Icon name="copy" size={15} />{t('conflict.keepBoth')}</button>
    <button class="btn" onclick={() => answer('newer')}><Icon name="clock" size={15} />{t('conflict.replaceNewer')}</button>
    <button class="btn" onclick={() => answer('if-different')}><Icon name="eye" size={15} />{t('conflict.replaceDifferent')}</button>
  </div>

  <label class="all">
    <input type="checkbox" bind:checked={applyAll} />
    <span>{t('conflict.applyAll')}{#if others > 0} <span class="muted">({t('conflict.others', { count: others })})</span>{/if}</span>
  </label>
</Modal>

<style>
  .file {
    display: flex;
    gap: 12px;
    align-items: center;
    margin-bottom: 14px;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border-radius: 12px;
    color: var(--amber);
    background: rgba(251, 191, 36, 0.12);
    flex: none;
  }
  .names {
    display: grid;
    min-width: 0;
  }
  .names strong {
    font-size: 16px;
    word-break: break-all;
  }
  .names span {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .compare {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    gap: 10px;
    align-items: stretch;
  }
  .side {
    position: relative;
    padding: 12px 14px;
    border-radius: 12px;
    background: rgba(5, 8, 24, 0.45);
    border: 1px solid var(--line);
    transition: border-color 0.3s, box-shadow 0.3s;
  }
  .side.newer {
    border-color: rgba(52, 211, 153, 0.55);
    box-shadow: 0 0 30px -12px rgba(52, 211, 153, 0.6);
  }
  .lbl {
    font-size: 11.5px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-2);
  }
  .tag {
    position: absolute;
    top: 10px;
    right: 10px;
    font-size: 11px;
    font-weight: 700;
    padding: 2px 7px;
    border-radius: 6px;
    color: var(--green);
    background: rgba(52, 211, 153, 0.14);
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    margin: 10px 0 0;
  }
  dt {
    color: var(--muted);
    font-size: 12.5px;
  }
  dd {
    margin: 0;
    font-weight: 600;
  }
  .vs {
    align-self: center;
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    letter-spacing: 0.1em;
  }
  .same {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    margin: 10px 0 0;
  }
  .choices {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 8px;
    margin-top: 16px;
  }
  .all {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 16px;
    cursor: pointer;
  }
  .all input {
    width: 18px;
    height: 18px;
    accent-color: var(--cyan);
  }
</style>
