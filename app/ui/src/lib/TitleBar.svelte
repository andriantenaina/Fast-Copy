<script lang="ts">
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import Icon from './Icon.svelte';
  import { app } from './store.svelte';
  import { t } from './i18n.svelte';
  import { api, isTauri } from './api';

  const mac = $derived(app.info?.platform === 'macos');
  const win = isTauri ? getCurrentWindow() : null;
  let maximized = $state(false);

  $effect(() => {
    if (!win) return;
    let un: (() => void) | undefined;
    win.isMaximized().then((m) => (maximized = m));
    win.onResized(async () => (maximized = await win.isMaximized())).then((u) => (un = u));
    return () => un?.();
  });
</script>

<header class="titlebar" class:mac data-tauri-drag-region>
  <div class="brand" data-tauri-drag-region>
    <svg class="logo" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="lg" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stop-color="#22d3ee" /><stop offset="1" stop-color="#a78bfa" />
        </linearGradient>
      </defs>
      <path d="M8 8l8 8-8 8M17 8l8 8-8 8" fill="none" stroke="url(#lg)" stroke-width="3.4" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
    <span data-tauri-drag-region>Fast Copy</span>
  </div>
  <div class="spacer" data-tauri-drag-region></div>
  {#if !mac}
    <div class="controls">
      <button class="wc" aria-label={t('window.minimize')} title={t('window.minimize')} onclick={() => win?.minimize()}>
        <Icon name="min" size={14} />
      </button>
      <button class="wc" aria-label={t(maximized ? 'window.restore' : 'window.maximize')} title={t(maximized ? 'window.restore' : 'window.maximize')} onclick={() => win?.toggleMaximize()}>
        {#if maximized}
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M3 1.5h7.5V9M1.5 3h7.5v7.5H1.5z" fill="none" stroke="currentColor" stroke-width="1.2" /></svg>
        {:else}
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><rect x="1.5" y="1.5" width="9" height="9" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" /></svg>
        {/if}
      </button>
      <button class="wc close" aria-label={t('window.close')} title={t('window.close')} onclick={() => api.closeWindow()}>
        <Icon name="x" size={15} />
      </button>
    </div>
  {/if}
</header>

<style>
  .titlebar {
    position: relative;
    z-index: 50;
    display: flex;
    align-items: center;
    height: var(--titlebar);
    padding-left: 14px;
    flex: none;
  }
  .titlebar.mac {
    padding-left: 84px; /* traffic lights */
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    font-family: var(--font-display);
    font-weight: 650;
    font-size: 13.5px;
    letter-spacing: 0.02em;
    color: var(--text);
  }
  .logo {
    width: 20px;
    height: 20px;
    filter: drop-shadow(0 0 6px rgba(34, 211, 238, 0.6));
  }
  .spacer {
    flex: 1;
    align-self: stretch;
  }
  .controls {
    display: flex;
    align-self: stretch;
  }
  .wc {
    width: 46px;
    border: none;
    background: transparent;
    color: var(--text-2);
    display: grid;
    place-items: center;
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
  }
  .wc:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }
  .wc.close:hover {
    background: #e81123;
    color: #fff;
  }
  .wc:focus-visible {
    outline-offset: -2px;
    border-radius: 0;
  }
</style>
