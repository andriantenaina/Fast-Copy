<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import Icon from './Icon.svelte';
  import DriveCard from './DriveCard.svelte';
  import { app, addSources, analyse, fail, refreshVolumes } from './store.svelte';
  import { t, baseName, middleTruncate } from './i18n.svelte';
  import { api, type OnConflict, type VerifyMode } from './api';

  const verifyModes: { id: VerifyMode; icon: string }[] = [
    { id: 'fast', icon: 'bolt' },
    { id: 'full', icon: 'shield' },
    { id: 'paranoid', icon: 'eye' },
  ];
  const conflictPolicies: OnConflict[] = ['ask', 'if-different', 'newer', 'skip', 'overwrite', 'keep-both'];

  const canAnalyse = $derived(app.form.sources.length > 0 && app.form.dest.trim().length > 0 && !app.planning);
  const selectedRoot = $derived(
    app.volumes.find((v) => app.form.dest && app.form.dest.toLowerCase().startsWith(v.root.toLowerCase()))?.root ?? null,
  );

  $effect(() => {
    // Keep the drive strip fresh (USB keys come and go) while this screen is shown.
    if (!app.visible) return;
    const id = setInterval(refreshVolumes, 4000);
    return () => clearInterval(id);
  });

  async function pickFiles() {
    try {
      const r = await open({ multiple: true, directory: false, title: t('home.pickFiles') });
      if (r) addSources(Array.isArray(r) ? r : [r]);
    } catch (e) {
      fail(e);
    }
  }
  async function pickFolders() {
    try {
      const r = await open({ multiple: true, directory: true, title: t('home.pickFolders') });
      if (r) addSources(Array.isArray(r) ? r : [r]);
    } catch (e) {
      fail(e);
    }
  }
  async function pickDest() {
    try {
      const r = await open({ multiple: false, directory: true, title: t('home.pickDest'), defaultPath: app.form.dest || undefined });
      if (typeof r === 'string') app.form.dest = r;
    } catch (e) {
      fail(e);
    }
  }
  async function pasteFromClipboard() {
    try {
      const p = await api.pasteSource();
      if (p && p.paths.length) {
        addSources(p.paths);
        app.form.mode = p.mode;
      } else {
        fail(t('home.clipboardEmpty'));
      }
    } catch (e) {
      fail(e);
    }
  }
  function removeSource(i: number) {
    app.form.sources.splice(i, 1);
  }
  function onKey(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter' && canAnalyse) {
      e.preventDefault();
      analyse();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="home">
  <header class="hero enter">
    <h1>{t('home.title')}</h1>
    <p class="muted">{t('home.subtitle')}</p>
  </header>

  <div class="cols">
    <!-- Sources -->
    <section class="glass panel sources enter" style="animation-delay: 60ms" aria-labelledby="src-title">
      <h2 class="panel-title" id="src-title"><span class="step">1</span>{t('home.sources')}</h2>

      <div class="drop" class:over={app.dragOver} class:compact={app.form.sources.length > 0}>
        <div class="drop-inner">
          <div class="drop-icon">
            <Icon name="copy" size={app.form.sources.length ? 22 : 34} stroke={1.6} />
          </div>
          <div class="drop-text">
            <strong>{app.dragOver ? t('home.dropNow') : t('home.drop')}</strong>
            {#if !app.form.sources.length}<span class="muted">{t('home.dropHint')}</span>{/if}
          </div>
        </div>
      </div>

      <div class="src-actions">
        <button class="btn sm" onclick={pickFiles}><Icon name="file-plus" size={16} />{t('home.addFiles')}</button>
        <button class="btn sm" onclick={pickFolders}><Icon name="folder-plus" size={16} />{t('home.addFolders')}</button>
        <button class="btn sm ghost" onclick={pasteFromClipboard} title={t('home.pasteHint')}><Icon name="clipboard" size={16} />{t('home.paste')}</button>
        {#if app.form.sources.length}
          <button class="btn sm ghost clear" onclick={() => (app.form.sources = [])}><Icon name="trash" size={15} />{t('home.clear')}</button>
        {/if}
      </div>

      {#if app.form.sources.length}
        <ul class="src-list" aria-label={t('home.sources')}>
          {#each app.form.sources as s, i (s)}
            <li class="src-item">
              <Icon name={/\.[a-z0-9]{1,6}$/i.test(baseName(s)) ? 'file' : 'folder'} size={16} />
              <div class="src-names">
                <span class="name">{baseName(s)}</span>
                <span class="path muted" title={s}>{middleTruncate(s, 70)}</span>
              </div>
              <button class="btn sm icon ghost" aria-label={t('home.remove', { name: baseName(s) })} onclick={() => removeSource(i)}>
                <Icon name="x" size={14} />
              </button>
            </li>
          {/each}
        </ul>
        <p class="count muted">{t('home.sourceCount', { count: app.form.sources.length })}</p>
      {/if}
    </section>

    <div class="right">
      <!-- Destination -->
      <section class="glass panel enter" style="animation-delay: 120ms" aria-labelledby="dst-title">
        <h2 class="panel-title" id="dst-title"><span class="step">2</span>{t('home.destination')}</h2>
        <div class="dest-row">
          <input class="input" bind:value={app.form.dest} placeholder={t('home.destPlaceholder')} aria-label={t('home.destination')} spellcheck="false" />
          <button class="btn" onclick={pickDest}><Icon name="folder" size={16} />{t('home.browse')}</button>
        </div>
        {#if app.volumes.length}
          <div class="drives" role="group" aria-label={t('home.drives')}>
            {#each app.volumes as v (v.root)}
              <DriveCard {v} selected={selectedRoot === v.root} onpick={() => (app.form.dest = v.root)} />
            {/each}
          </div>
        {/if}
      </section>

      <!-- Options -->
      <section class="glass panel enter" style="animation-delay: 180ms" aria-labelledby="opt-title">
        <h2 class="panel-title" id="opt-title"><span class="step">3</span>{t('home.options')}</h2>

        <div class="opt-row">
          <div class="seg" role="radiogroup" aria-label={t('home.mode')}>
            <button role="radio" aria-checked={app.form.mode === 'copy'} onclick={() => (app.form.mode = 'copy')}><Icon name="copy" size={15} />{t('mode.copy')}</button>
            <button role="radio" aria-checked={app.form.mode === 'move'} onclick={() => (app.form.mode = 'move')}><Icon name="move" size={15} />{t('mode.move')}</button>
          </div>
          <label class="field conflict">
            {t('home.conflicts')}
            <select class="select" bind:value={app.form.onConflict}>
              {#each conflictPolicies as c}<option value={c}>{t(`conflict.policy.${c}`)}</option>{/each}
            </select>
          </label>
        </div>

        <div class="verify" role="radiogroup" aria-label={t('home.verify')}>
          {#each verifyModes as m}
            <button class="vcard" role="radio" aria-checked={app.form.verify === m.id} onclick={() => (app.form.verify = m.id)}>
              <span class="vicon"><Icon name={m.icon} size={18} /></span>
              <span class="vtext">
                <strong>{t(`verify.${m.id}`)}{#if m.id === 'full'}<em class="default">{t('verify.default')}</em>{/if}</strong>
                <small>{t(`verify.${m.id}.desc`)}</small>
              </span>
            </button>
          {/each}
        </div>
        {#if app.form.mode === 'move' && app.form.verify === 'fast'}
          <p class="note muted"><Icon name="info" size={14} />{t('verify.moveNote')}</p>
        {/if}

        <div class="go">
          <span class="kbd muted">Ctrl ↵</span>
          <button class="btn primary lg" disabled={!canAnalyse} onclick={() => analyse()}>
            {#if app.planning}<span class="spinner"></span>{t('home.analysing')}{:else}<Icon name="sparkle" size={18} />{t('home.analyse')}{/if}
          </button>
        </div>
      </section>
    </div>
  </div>
</div>

<style>
  .home {
    max-width: 1200px;
    margin: 0 auto;
  }
  .hero {
    margin: 4px 0 18px;
  }
  .hero h1 {
    font-size: 26px;
    background: linear-gradient(90deg, #fff 30%, #c4d3ff);
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  .hero p {
    margin: 4px 0 0;
  }
  .cols {
    display: grid;
    grid-template-columns: minmax(0, 0.9fr) minmax(0, 1.1fr);
    gap: 18px;
    align-items: start;
  }
  @media (max-width: 900px) {
    .cols {
      grid-template-columns: 1fr;
    }
  }
  .right {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 18px;
    min-width: 0;
  }
  .right > section {
    min-width: 0;
  }
  .sources {
    min-width: 0;
  }

  .drop {
    position: relative;
    display: grid;
    place-items: center;
    min-height: 220px;
    border-radius: 15px;
    background: radial-gradient(120% 120% at 50% 0%, rgba(34, 211, 238, 0.08), transparent 60%), rgba(5, 8, 24, 0.35);
    transition: min-height 0.45s var(--ease), background 0.3s, transform 0.3s var(--ease), box-shadow 0.3s;
  }
  .drop.compact {
    min-height: 84px;
  }
  .drop.over {
    background: radial-gradient(120% 120% at 50% 0%, rgba(34, 211, 238, 0.22), rgba(167, 139, 250, 0.1) 70%), rgba(5, 8, 24, 0.35);
    transform: scale(1.01);
    box-shadow: 0 0 0 1px rgba(34, 211, 238, 0.5), 0 0 50px -10px rgba(34, 211, 238, 0.6);
  }
  .drop-inner {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    text-align: center;
    padding: 16px;
  }
  .compact .drop-inner {
    flex-direction: row;
    text-align: left;
  }
  .drop-icon {
    display: grid;
    place-items: center;
    width: 72px;
    height: 72px;
    border-radius: 22px;
    color: var(--cyan);
    background: var(--grad-soft);
    border: 1px solid var(--line-strong);
    box-shadow: var(--glow-cyan);
    animation: float 4s ease-in-out infinite;
    transition: width 0.4s var(--ease), height 0.4s var(--ease);
  }
  .compact .drop-icon {
    width: 44px;
    height: 44px;
    border-radius: 14px;
    animation: none;
  }
  .over .drop-icon {
    animation: pulse 0.9s ease-in-out infinite;
  }
  .drop-text {
    display: grid;
    gap: 2px;
  }
  .drop-text strong {
    font-size: 15px;
  }
  /* marching-ants border (pure CSS, works on old WebKit too) */
  .drop::before {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: inherit;
    pointer-events: none;
    --c: rgba(34, 211, 238, 0.45);
    background:
      linear-gradient(90deg, var(--c) 50%, transparent 0) repeat-x 0 0 / 16px 1.5px,
      linear-gradient(90deg, var(--c) 50%, transparent 0) repeat-x 0 100% / 16px 1.5px,
      linear-gradient(0deg, var(--c) 50%, transparent 0) repeat-y 0 0 / 1.5px 16px,
      linear-gradient(0deg, var(--c) 50%, transparent 0) repeat-y 100% 0 / 1.5px 16px;
    animation: march 1.2s linear infinite;
  }
  .drop.over::before {
    --c: var(--cyan);
    animation-duration: 0.4s;
  }
  @keyframes march {
    to {
      background-position: 16px 0, -16px 100%, 0 -16px, 100% 16px;
    }
  }
  @keyframes float {
    50% {
      transform: translateY(-5px);
    }
  }
  @keyframes pulse {
    50% {
      transform: scale(1.08);
      box-shadow: 0 0 40px rgba(34, 211, 238, 0.7);
    }
  }

  .src-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 14px;
  }
  .clear {
    margin-left: auto;
  }
  .src-list {
    list-style: none;
    margin: 14px 0 0;
    padding: 0;
    max-height: 300px;
    overflow: auto;
    display: grid;
    gap: 6px;
  }
  .src-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 8px 8px 12px;
    border-radius: 10px;
    background: rgba(5, 8, 24, 0.4);
    border: 1px solid var(--line);
    color: var(--cyan);
    animation: fade-up 0.35s var(--ease) both;
  }
  .src-names {
    display: grid;
    min-width: 0;
    flex: 1;
  }
  .src-names .name {
    color: var(--text);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .src-names .path {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    margin: 10px 2px 0;
    font-size: 12.5px;
  }

  .dest-row {
    display: flex;
    gap: 8px;
  }
  .drives {
    display: flex;
    gap: 10px;
    margin-top: 14px;
    overflow-x: auto;
    padding: 2px 2px 8px;
    scroll-snap-type: x proximity;
  }

  .opt-row {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    gap: 14px;
    justify-content: space-between;
  }
  .conflict {
    min-width: 220px;
    flex: 1;
    max-width: 320px;
  }
  .verify {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 10px;
    margin-top: 16px;
  }
  .vcard {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    text-align: left;
    padding: 12px;
    border-radius: 12px;
    border: 1px solid var(--line);
    background: rgba(5, 8, 24, 0.35);
    color: var(--text);
    cursor: pointer;
    transition: border-color 0.2s, background 0.25s, box-shadow 0.25s, transform 0.2s var(--ease);
  }
  .vcard:hover {
    border-color: var(--line-strong);
    transform: translateY(-1px);
  }
  .vcard[aria-checked='true'] {
    border-color: rgba(34, 211, 238, 0.6);
    background: linear-gradient(135deg, rgba(34, 211, 238, 0.12), rgba(167, 139, 250, 0.08));
    box-shadow: 0 0 0 1px rgba(34, 211, 238, 0.25) inset, 0 10px 30px -15px rgba(34, 211, 238, 0.6);
  }
  .vicon {
    display: grid;
    place-items: center;
    flex: none;
    width: 32px;
    height: 32px;
    border-radius: 9px;
    background: var(--grad-soft);
    color: var(--cyan);
  }
  .vcard[aria-checked='true'] .vicon {
    background: var(--grad);
    color: #04121a;
  }
  .vtext {
    display: grid;
    gap: 3px;
  }
  .vtext strong {
    font-size: 13.5px;
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .vtext small {
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.35;
  }
  .default {
    font-style: normal;
    font-size: 10.5px;
    font-weight: 700;
    padding: 1px 6px;
    border-radius: 5px;
    color: var(--cyan);
    background: rgba(34, 211, 238, 0.12);
  }
  .note {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    margin: 10px 0 0;
  }
  .go {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 14px;
    margin-top: 18px;
  }
  .kbd {
    font-size: 12px;
    font-family: var(--mono);
  }
</style>
