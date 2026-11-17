<script lang="ts">
  import { onMount } from 'svelte';
  import TitleBar from './lib/TitleBar.svelte';
  import Background from './lib/Background.svelte';
  import Icon from './lib/Icon.svelte';
  import Home from './lib/Home.svelte';
  import PlanReview from './lib/PlanReview.svelte';
  import LiveView from './lib/LiveView.svelte';
  import Queue from './lib/Queue.svelte';
  import Jobs from './lib/Jobs.svelte';
  import Settings from './lib/Settings.svelte';
  import ConflictDialog from './lib/ConflictDialog.svelte';
  import DiskFullDialog from './lib/DiskFullDialog.svelte';
  import FormatDialog from './lib/FormatDialog.svelte';
  import Toasts from './lib/Toasts.svelte';
  import { app, init, fail, activeCount, type View } from './lib/store.svelte';
  import { t, fmtNumber } from './lib/i18n.svelte';

  onMount(() => {
    init().catch(fail);
  });

  const running = $derived(app.queue.find((e) => e.state === 'running'));
  const runningPct = $derived.by(() => {
    const s = running ? app.live[running.id]?.snap : null;
    return s && s.bytes_total ? s.bytes_done / s.bytes_total : 0;
  });
  // Particle "energy" follows the live throughput of the running job.
  const energy = $derived(running && app.live[running.id]?.snap && !app.live[running.id]?.snap?.paused ? 1 : 0);
  const active = $derived(activeCount());
  const request = $derived(app.requests[0]);

  const nav: { id: View; icon: string; key: string }[] = [
    { id: 'home', icon: 'plus', key: 'nav.home' },
    { id: 'live', icon: 'gauge', key: 'nav.live' },
    { id: 'queue', icon: 'queue', key: 'nav.queue' },
    { id: 'jobs', icon: 'history', key: 'nav.jobs' },
    { id: 'settings', icon: 'settings', key: 'nav.settings' },
  ];

  function onKey(e: KeyboardEvent) {
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    const i = ['1', '2', '3', '4', '5'].indexOf(e.key);
    if (i >= 0) {
      e.preventDefault();
      app.view = nav[i].id;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<Background {energy} />

<div class="shell">
  <TitleBar />
  <div class="body">
    <nav class="rail" aria-label={t('nav.label')}>
      {#each nav as n, i}
        <button class="nav" class:active={app.view === n.id} aria-current={app.view === n.id ? 'page' : undefined} onclick={() => (app.view = n.id)} title={`${t(n.key)} (Ctrl+${i + 1})`}>
          <span class="ni"><Icon name={n.icon} size={19} /></span>
          <span class="nl">{t(n.key)}</span>
          {#if n.id === 'queue' && active > 0}<span class="count num">{active}</span>{/if}
          {#if n.id === 'jobs' && app.interruptedCount > 0}<span class="count warn num">{app.interruptedCount}</span>{/if}
          {#if n.id === 'live' && running}<span class="live-dot" aria-hidden="true"></span>{/if}
        </button>
      {/each}
      <div class="grow"></div>
      {#if running}
        <button class="mini glass" onclick={() => { app.focusJob = running.id; app.view = 'live'; }} aria-label={t('nav.live')}>
          <svg viewBox="0 0 36 36" aria-hidden="true">
            <circle cx="18" cy="18" r="15" class="mt" />
            <circle cx="18" cy="18" r="15" class="mv" stroke-dasharray="94.25" stroke-dashoffset={94.25 * (1 - runningPct)} />
          </svg>
          <span class="mini-pct num">{fmtNumber(Math.floor(runningPct * 100))}%</span>
        </button>
      {/if}
    </nav>

    <main class="content" class:wide={app.view === 'live'}>
      {#if app.ready}
        {#key app.view}
          <div class="view">
            {#if app.view === 'home'}<Home />
            {:else if app.view === 'live'}<LiveView />
            {:else if app.view === 'queue'}<Queue />
            {:else if app.view === 'jobs'}<Jobs />
            {:else}<Settings />{/if}
          </div>
        {/key}
      {:else}
        <div class="boot"><div class="spinner"></div></div>
      {/if}
    </main>
  </div>
</div>

{#if app.plan}
  {#key app.plan.id}<PlanReview plan={app.plan} />{/key}
{/if}
{#if app.formatTarget}
  <FormatDialog v={app.formatTarget} />
{/if}
{#if request}
  {#key request.requestId}
    {#if request.type === 'conflict'}<ConflictDialog req={request} />{:else}<DiskFullDialog req={request} />{/if}
  {/key}
{/if}
<Toasts />

<style>
  .shell {
    position: relative;
    z-index: 1;
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .rail {
    flex: none;
    width: 208px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 12px 14px;
  }
  .nav {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    height: 42px;
    padding: 0 12px;
    border: 1px solid transparent;
    border-radius: 12px;
    background: transparent;
    color: var(--text-2);
    font-weight: 560;
    cursor: pointer;
    text-align: left;
    transition: background 0.25s var(--ease), color 0.2s, border-color 0.25s;
  }
  .nav:hover {
    background: var(--glass);
    color: var(--text);
  }
  .nav.active {
    color: var(--text);
    background: var(--grad-soft);
    border-color: rgba(34, 211, 238, 0.3);
    box-shadow: 0 10px 30px -18px rgba(34, 211, 238, 0.8);
  }
  .nav.active::before {
    content: '';
    position: absolute;
    left: -12px;
    top: 10px;
    bottom: 10px;
    width: 3px;
    border-radius: 0 3px 3px 0;
    background: var(--grad);
    box-shadow: var(--glow-cyan);
  }
  .ni {
    display: grid;
    place-items: center;
  }
  .nav.active .ni {
    color: var(--cyan);
  }
  .nl {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .count {
    min-width: 20px;
    height: 20px;
    padding: 0 6px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    font-size: 11px;
    font-weight: 700;
    color: #04121a;
    background: var(--cyan);
  }
  .count.warn {
    background: var(--amber);
  }
  .live-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--cyan);
    box-shadow: 0 0 10px var(--cyan);
    animation: blink 1.4s ease-in-out infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0.3;
    }
  }
  .grow {
    flex: 1;
  }
  .mini {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    cursor: pointer;
    color: var(--text);
    border-radius: 12px;
  }
  .mini svg {
    width: 34px;
    height: 34px;
    transform: rotate(-90deg);
  }
  .mt {
    fill: none;
    stroke: rgba(160, 175, 255, 0.15);
    stroke-width: 4;
  }
  .mv {
    fill: none;
    stroke: var(--cyan);
    stroke-width: 4;
    stroke-linecap: round;
    transition: stroke-dashoffset 0.5s var(--ease);
    filter: drop-shadow(0 0 4px var(--cyan));
  }
  .mini-pct {
    font-weight: 700;
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow: auto;
    padding: 8px 26px 28px 14px;
  }
  .view {
    min-height: 100%;
    animation: view-in 0.45s var(--ease) both;
  }
  @keyframes view-in {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.995);
    }
  }
  .boot {
    height: 100%;
    display: grid;
    place-items: center;
  }
  @media (max-width: 960px) {
    .rail {
      width: 68px;
      padding: 10px 10px 14px;
    }
    .nl,
    .mini-pct {
      display: none;
    }
    .nav {
      justify-content: center;
      padding: 0;
    }
    .count {
      position: absolute;
      top: 2px;
      right: 2px;
      min-width: 16px;
      height: 16px;
      font-size: 10px;
      padding: 0 4px;
    }
    .live-dot {
      position: absolute;
      top: 6px;
      right: 8px;
    }
    .mini {
      justify-content: center;
      padding: 8px 0;
    }
    .nav.active::before {
      left: -10px;
    }
  }
</style>
