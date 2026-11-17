<script lang="ts">
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import Icon from './Icon.svelte';
  import { app, analyse, refreshVolumes } from './store.svelte';
  import { t, fmtBytes } from './i18n.svelte';
  import { api, errorText, type VolumeView } from './api';

  let { v }: { v: VolumeView } = $props();

  const GiB = 1024 ** 3;
  const recommended = $derived(v.totalBytes <= 8 * GiB ? 32768 : 131072);
  const clusters = [32768, 131072, 1048576];

  let step = $state<'form' | 'confirm' | 'running' | 'done' | 'error'>('form');
  // Initial values only: the dialog is re-created for each drive.
  const initial = untrack(() => v);
  let label = $state(cleanLabel(initial.label || 'FASTCOPY'));
  let cluster = $state(initial.totalBytes <= 8 * GiB ? 32768 : 131072);
  let typed = $state('');
  let error = $state('');
  let result = $state<VolumeView | null>(null);

  const expected = $derived(v.label.trim() || t('format.eraseWord'));
  const matches = $derived(typed.trim() === expected);
  const cleaned = $derived(cleanLabel(label));

  function cleanLabel(s: string): string {
    return s
      .replace(/["'\\/:*?<>|=;,\u0000-\u001f]/g, '')
      .slice(0, 11)
      .trim();
  }
  function clusterName(c: number) {
    return c >= 1048576 ? t('format.cluster1m') : t('format.clusterK', { k: c / 1024 });
  }

  async function run() {
    step = 'running';
    try {
      result = await api.formatDrive(v.root, cleaned || 'FASTCOPY', cluster);
      step = 'done';
      await refreshVolumes();
      if (app.plan && app.plan.dstVolume.root === v.root) {
        app.plan = null;
        analyse();
      }
    } catch (e) {
      error = errorText(e);
      step = 'error';
    }
  }
  function close() {
    if (step === 'running') return;
    app.formatTarget = null;
  }
</script>

<Modal title={t('format.title')} width={600} tone="danger" dismissable={step !== 'running'} onclose={close}>
  {#if step === 'form' || step === 'confirm'}
    <div class="device">
      <span class="ic"><Icon name="usb" size={24} /></span>
      <div class="dinfo">
        <strong>{v.label || v.root} <span class="badge fs-{v.fs}">{v.fsLabel}</span></strong>
        <span class="muted">{v.model || t('drive.external')} · {fmtBytes(v.totalBytes)} · {v.bus.toUpperCase()} · {v.root}</span>
      </div>
    </div>
  {/if}

  {#if step === 'form'}
    <div class="warn" role="alert"><Icon name="alert" size={18} /><strong>{t('format.warning')}</strong></div>
    <p class="muted why">{t('format.why')}</p>
    <div class="fields">
      <label class="field">
        {t('format.label')}
        <input class="input" bind:value={label} maxlength="11" spellcheck="false" />
        <small class="muted">{t('format.labelHint', { n: cleaned.length })}</small>
      </label>
      <label class="field">
        {t('format.cluster')}
        <select class="select" bind:value={cluster}>
          {#each clusters as c}<option value={c}>{clusterName(c)}{c === recommended ? ` — ${t('format.recommended')}` : ''}</option>{/each}
        </select>
        <small class="muted">{t('format.clusterHint')}</small>
      </label>
    </div>
    <label class="field confirm">
      <span>{@html t('format.typeToConfirm', { word: `<code>${expected.replace(/[<>&]/g, '')}</code>` })}</span>
      <input class="input" bind:value={typed} spellcheck="false" autocomplete="off" data-autofocus />
    </label>
  {:else if step === 'confirm'}
    <div class="final" role="alert">
      <Icon name="alert-circle" size={40} stroke={1.6} />
      <h3>{t('format.lastChance')}</h3>
      <p>{t('format.lastChanceText', { name: v.label || v.root, size: fmtBytes(v.totalBytes) })}</p>
    </div>
  {:else if step === 'running'}
    <div class="state">
      <div class="big-spinner"></div>
      <h3>{t('format.running')}</h3>
      <p class="muted">{app.info?.platform === 'windows' ? t('format.uac') : t('format.wait')}</p>
    </div>
  {:else if step === 'done'}
    <div class="state ok">
      <div class="okc"><Icon name="check" size={34} stroke={2.6} /></div>
      <h3>{t('format.done')}</h3>
      <p class="muted">{t('format.doneText', { name: result?.label || cleaned, root: result?.root ?? v.root })}</p>
    </div>
  {:else}
    <div class="state err">
      <Icon name="alert-circle" size={40} stroke={1.6} />
      <h3>{t('format.failed')}</h3>
      <p class="selectable">{error}</p>
    </div>
  {/if}

  {#snippet footer()}
    {#if step === 'form'}
      <button class="btn" onclick={close}>{t('common.cancel')}</button>
      <button class="btn danger" disabled={!matches || !cleaned} onclick={() => (step = 'confirm')}>{t('format.next')}</button>
    {:else if step === 'confirm'}
      <button class="btn" onclick={() => (step = 'form')}>{t('common.back')}</button>
      <button class="btn danger-solid" onclick={run}><Icon name="format" size={15} />{t('format.go', { root: v.root })}</button>
    {:else if step === 'done' || step === 'error'}
      <button class="btn primary" onclick={close} data-autofocus>{t('common.close')}</button>
    {/if}
  {/snippet}
</Modal>

<style>
  .device {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 12px;
    border-radius: 12px;
    background: rgba(5, 8, 24, 0.45);
    border: 1px solid var(--line);
    margin-bottom: 14px;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border-radius: 12px;
    color: var(--cyan);
    background: rgba(34, 211, 238, 0.12);
    flex: none;
  }
  .dinfo {
    display: grid;
    gap: 3px;
    min-width: 0;
  }
  .dinfo strong {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .dinfo span {
    font-size: 12.5px;
  }
  .warn {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    border-radius: 12px;
    color: #ffd5dc;
    background: rgba(244, 63, 94, 0.14);
    border: 1px solid rgba(251, 113, 133, 0.5);
  }
  .warn :global(svg) {
    color: var(--red);
    flex: none;
  }
  .why {
    font-size: 12.5px;
    margin: 10px 0 14px;
  }
  .fields {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .fields small {
    font-weight: 400;
    font-size: 11.5px;
  }
  .confirm {
    margin-top: 16px;
  }
  .confirm :global(code) {
    font-family: var(--mono);
    color: var(--red);
    background: rgba(244, 63, 94, 0.12);
    padding: 1px 6px;
    border-radius: 5px;
    -webkit-user-select: all;
    user-select: all;
  }
  .final,
  .state {
    display: grid;
    justify-items: center;
    text-align: center;
    gap: 10px;
    padding: 18px 8px;
  }
  .final {
    color: var(--red);
  }
  .final p {
    color: var(--text);
    margin: 0;
    max-width: 440px;
  }
  .final h3,
  .state h3 {
    font-size: 18px;
  }
  .state p {
    margin: 0;
    max-width: 440px;
  }
  .state.err {
    color: var(--red);
  }
  .state.err p {
    color: var(--text-2);
    word-break: break-word;
  }
  .big-spinner {
    width: 56px;
    height: 56px;
    border-radius: 50%;
    border: 3px solid rgba(160, 175, 255, 0.15);
    border-top-color: var(--cyan);
    border-right-color: var(--violet);
    animation: spin 0.9s linear infinite;
    box-shadow: var(--glow-cyan);
  }
  .okc {
    display: grid;
    place-items: center;
    width: 68px;
    height: 68px;
    border-radius: 50%;
    color: #04121a;
    background: var(--grad);
    box-shadow: 0 0 40px rgba(34, 211, 238, 0.6);
    animation: pop 0.5s var(--spring);
  }
  @keyframes pop {
    from {
      transform: scale(0.4);
      opacity: 0;
    }
  }
</style>
