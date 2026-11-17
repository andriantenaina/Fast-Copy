<script lang="ts">
  import Icon from './Icon.svelte';
  import { app, fail } from './store.svelte';
  import { t, fmtBytes, fmtNumber, baseName, middleTruncate } from './i18n.svelte';
  import { api, type QueueEntry } from './api';

  const hasFinished = $derived(app.queue.some((e) => typeof e.state === 'object'));

  function stateKey(e: QueueEntry): string {
    return typeof e.state === 'string' ? e.state : e.state.finished;
  }
  function progress(e: QueueEntry): number {
    const s = app.live[e.id]?.snap;
    if (typeof e.state === 'object') return e.state.finished === 'completed' ? 1 : s && s.bytes_total ? s.bytes_done / s.bytes_total : 0;
    return s && s.bytes_total ? s.bytes_done / s.bytes_total : 0;
  }
  function open(e: QueueEntry) {
    app.focusJob = e.id;
    app.view = 'live';
  }
  const run = (p: Promise<unknown>) => p.catch(fail);
</script>

<div class="queue">
  <header class="head enter">
    <div>
      <h1>{t('queue.title')}</h1>
      <p class="muted">{t('queue.subtitle')}</p>
    </div>
    <button class="btn" disabled={!hasFinished} onclick={() => run(api.clearFinished())}><Icon name="trash" size={15} />{t('queue.clear')}</button>
  </header>

  {#if !app.queue.length}
    <div class="empty glass enter">
      <Icon name="queue" size={34} stroke={1.5} />
      <p>{t('queue.empty')}</p>
    </div>
  {:else}
    <ul class="list">
      {#each app.queue as e, i (e.id)}
        {@const st = stateKey(e)}
        {@const p = progress(e)}
        <li class="item glass st-{st}" style="animation-delay:{i * 40}ms">
          <button class="main" onclick={() => open(e)} aria-label={t('queue.open')}>
            <span class="icon"><Icon name={e.mode === 'move' ? 'move' : 'copy'} size={18} /></span>
            <span class="info">
              <span class="name">
                {e.sources.length === 1 ? baseName(e.sources[0]) : t('plan.nSources', { count: e.sources.length })}
                <span class="state">{t(`queue.state.${st}`)}</span>
              </span>
              <span class="dest muted"><Icon name="arrow" size={12} />{middleTruncate(e.dest, 60)}</span>
              <span class="meta muted num">{fmtNumber(e.total_files)} · {fmtBytes(e.total_bytes)}{#if st === 'running' || st === 'paused'} · {fmtNumber(Math.floor(p * 100))} %{/if}</span>
            </span>
          </button>
          <div class="actions">
            {#if st === 'pending' || st === 'paused'}
              <button class="btn sm icon ghost" disabled={i === 0} aria-label={t('queue.up')} title={t('queue.up')} onclick={() => run(api.reorderJob(e.id, -1))}><Icon name="up" size={16} /></button>
              <button class="btn sm icon ghost" disabled={i === app.queue.length - 1} aria-label={t('queue.down')} title={t('queue.down')} onclick={() => run(api.reorderJob(e.id, 1))}><Icon name="down" size={16} /></button>
            {/if}
            {#if st === 'running' || st === 'pending'}
              <button class="btn sm icon" aria-label={t('live.pause')} title={t('live.pause')} onclick={() => run(api.pauseJob(e.id))}><Icon name="pause" size={14} /></button>
            {:else if st === 'paused'}
              <button class="btn sm icon" aria-label={t('live.resume')} title={t('live.resume')} onclick={() => run(api.resumeJob(e.id))}><Icon name="play" size={14} /></button>
            {/if}
            {#if typeof e.state === 'string'}
              <button class="btn sm icon danger" aria-label={t('live.cancel')} title={t('live.cancel')} onclick={() => run(api.cancelJob(e.id))}><Icon name="x" size={14} /></button>
            {/if}
          </div>
          {#if st !== 'pending'}
            <div class="bar" class:warn={st === 'failed' || st === 'cancelled'}><i style="width:{p * 100}%"></i></div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .queue {
    max-width: 1000px;
    margin: 0 auto;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 12px;
    margin-bottom: 18px;
  }
  .head h1 {
    font-size: 24px;
  }
  .head p {
    margin: 4px 0 0;
  }
  .empty {
    display: grid;
    justify-items: center;
    gap: 10px;
    padding: 48px;
    color: var(--muted);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }
  .item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px 16px;
    overflow: hidden;
    animation: fade-up 0.4s var(--ease) both;
  }
  .main {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    border: none;
    background: none;
    text-align: left;
    padding: 0;
    cursor: pointer;
    color: inherit;
  }
  .icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border-radius: 11px;
    color: var(--cyan);
    background: var(--grad-soft);
  }
  .info {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 650;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .state {
    font-size: 11px;
    font-weight: 700;
    padding: 1px 7px;
    border-radius: 6px;
    background: rgba(160, 175, 255, 0.1);
    color: var(--text-2);
  }
  .st-running .state {
    color: var(--cyan);
    background: rgba(34, 211, 238, 0.12);
  }
  .st-paused .state {
    color: var(--amber);
    background: rgba(251, 191, 36, 0.12);
  }
  .st-completed .state {
    color: var(--green);
    background: rgba(52, 211, 153, 0.12);
  }
  .st-completed-with-errors .state,
  .st-cancelled .state {
    color: var(--amber);
    background: rgba(251, 191, 36, 0.12);
  }
  .st-failed .state {
    color: var(--red);
    background: rgba(251, 113, 133, 0.12);
  }
  .dest,
  .meta {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12.5px;
    white-space: nowrap;
    overflow: hidden;
  }
  .actions {
    display: flex;
    gap: 4px;
    flex: none;
  }
  .item .bar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    border-radius: 0;
  }
</style>
