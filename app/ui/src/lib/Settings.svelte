<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from './Icon.svelte';
  import { app, fail, saveSettings, toast } from './store.svelte';
  import { t } from './i18n.svelte';
  import { api, type OnConflict, type Settings, type VerifyMode } from './api';

  let shell = $state<boolean | null>(null);
  let shellBusy = $state(false);

  onMount(async () => {
    try {
      shell = await api.shellStatus();
    } catch {
      shell = false;
    }
  });

  function update(patch: Partial<Settings>) {
    if (!app.settings) return;
    const s = { ...app.settings, ...patch };
    saveSettings(s);
    if (patch.verify) app.form.verify = patch.verify;
    if (patch.onConflict) app.form.onConflict = patch.onConflict;
  }

  async function toggleShell() {
    shellBusy = true;
    try {
      if (shell) await api.shellUninstall();
      else await api.shellInstall();
      shell = await api.shellStatus();
      // The backend records the choice (autoShell): keep our copy in sync.
      app.settings = await api.getSettings();
      toast(t(shell ? 'settings.shellOn' : 'settings.shellOff'), 'success');
    } catch (e) {
      fail(e);
    } finally {
      shellBusy = false;
    }
  }

  const verifyModes: VerifyMode[] = ['fast', 'full', 'paranoid'];
  const conflicts: OnConflict[] = ['ask', 'if-different', 'newer', 'skip', 'overwrite', 'keep-both'];
  const s = $derived(app.settings);
  const effectName = $derived(app.info?.effect === 'none' ? t('settings.effectNone') : app.info?.effect ?? '—');
</script>

<div class="settings">
  <header class="enter">
    <h1>{t('settings.title')}</h1>
  </header>

  {#if s}
    <section class="glass panel enter" style="animation-delay:40ms">
      <h2 class="panel-title"><Icon name="globe" size={16} />{t('settings.language')}</h2>
      <div class="seg" role="radiogroup" aria-label={t('settings.language')}>
        {#each ['auto', 'fr', 'en'] as const as l}
          <button role="radio" aria-checked={s.lang === l} onclick={() => update({ lang: l })}>{t(`settings.lang.${l}`)}</button>
        {/each}
      </div>
    </section>

    <section class="glass panel enter" style="animation-delay:80ms">
      <h2 class="panel-title"><Icon name="shield" size={16} />{t('settings.defaults')}</h2>
      <div class="rows">
        <label class="field">
          {t('home.verify')}
          <select class="select" value={s.verify} onchange={(e) => update({ verify: (e.currentTarget as HTMLSelectElement).value as VerifyMode })}>
            {#each verifyModes as m}<option value={m}>{t(`verify.${m}`)} — {t(`verify.${m}.desc`)}</option>{/each}
          </select>
        </label>
        <label class="field">
          {t('home.conflicts')}
          <select class="select" value={s.onConflict} onchange={(e) => update({ onConflict: (e.currentTarget as HTMLSelectElement).value as OnConflict })}>
            {#each conflicts as c}<option value={c}>{t(`conflict.policy.${c}`)}</option>{/each}
          </select>
        </label>
      </div>
      <div class="toggle-row">
        <div>
          <strong id="sj">{t('settings.skipJunk')}</strong>
          <p class="muted">{t('settings.skipJunkDesc')}</p>
        </div>
        <button class="switch" role="switch" aria-checked={s.skipJunk} aria-labelledby="sj" onclick={() => update({ skipJunk: !s.skipJunk })}></button>
      </div>
      <div class="toggle-row">
        <div>
          <strong id="fe">{t('settings.fileEvents')}</strong>
          <p class="muted">{t('settings.fileEventsDesc')}</p>
        </div>
        <button class="switch" role="switch" aria-checked={s.fileEvents} aria-labelledby="fe" onclick={() => update({ fileEvents: !s.fileEvents })}></button>
      </div>
      <div class="toggle-row">
        <div>
          <strong id="nt">{t('settings.notify')}</strong>
          <p class="muted">{t('settings.notifyDesc')}</p>
        </div>
        <button class="switch" role="switch" aria-checked={s.notify} aria-labelledby="nt" onclick={() => update({ notify: !s.notify })}></button>
      </div>
    </section>

    <section class="glass panel enter" style="animation-delay:120ms">
      <h2 class="panel-title"><Icon name="menu" size={16} />{t('settings.integration')}</h2>
      <div class="toggle-row">
        <div>
          <strong id="sh">{t('settings.shell')}</strong>
          <p class="muted">{app.info?.platform === 'macos' ? t('settings.shellDescMac') : t('settings.shellDescWin')}</p>
        </div>
        <button class="switch" role="switch" aria-checked={!!shell} aria-labelledby="sh" disabled={shell === null || shellBusy} onclick={toggleShell}></button>
      </div>
    </section>
  {/if}

  <section class="glass panel about enter" style="animation-delay:160ms">
    <h2 class="panel-title"><Icon name="info" size={16} />{t('settings.about')}</h2>
    <div class="about-row">
      <svg class="logo" viewBox="0 0 32 32" aria-hidden="true">
        <defs><linearGradient id="lg2" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#22d3ee" /><stop offset="1" stop-color="#a78bfa" /></linearGradient></defs>
        <path d="M8 8l8 8-8 8M17 8l8 8-8 8" fill="none" stroke="url(#lg2)" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
      <div>
        <strong>Fast Copy {app.info?.version}</strong>
        <p class="muted">{t('settings.aboutDesc')}</p>
        <p class="muted small">{t('settings.engine', { v: app.info?.coreVersion ?? '' })} · {t('settings.effect')} : {effectName}</p>
      </div>
    </div>
  </section>
</div>

<style>
  .settings {
    max-width: 820px;
    margin: 0 auto;
    display: grid;
    gap: 16px;
  }
  h1 {
    font-size: 24px;
  }
  .rows {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
    margin-bottom: 8px;
  }
  @media (max-width: 760px) {
    .rows {
      grid-template-columns: 1fr;
    }
  }
  .toggle-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    padding: 12px 0;
    border-top: 1px solid var(--line);
  }
  .toggle-row:first-of-type {
    border-top: none;
  }
  .toggle-row p {
    margin: 2px 0 0;
    font-size: 12.5px;
    max-width: 560px;
  }
  .about-row {
    display: flex;
    gap: 16px;
    align-items: center;
  }
  .about-row p {
    margin: 2px 0 0;
  }
  .logo {
    width: 52px;
    height: 52px;
    padding: 8px;
    border-radius: 14px;
    background: rgba(5, 8, 24, 0.6);
    border: 1px solid var(--line-strong);
    filter: drop-shadow(0 0 10px rgba(34, 211, 238, 0.4));
    flex: none;
  }
  .small {
    font-size: 12px;
  }
</style>
