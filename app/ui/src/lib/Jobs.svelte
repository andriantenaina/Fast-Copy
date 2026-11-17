<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from './Icon.svelte';
  import { app, fail, refreshInterrupted, toast } from './store.svelte';
  import { t, fmtBytes, fmtDate, fmtNumber, baseName, middleTruncate } from './i18n.svelte';
  import { api, type JobSummary } from './api';

  let jobs = $state<JobSummary[]>([]);
  let loading = $state(true);
  let confirmDiscard = $state<string | null>(null);

  async function load() {
    loading = true;
    try {
      jobs = await api.listJobs();
      app.interruptedCount = jobs.length;
    } catch (e) {
      fail(e);
    } finally {
      loading = false;
    }
  }
  onMount(load);

  async function resume(j: JobSummary) {
    try {
      const id = await api.resumeStoredJob(j.id);
      app.focusJob = id;
      app.view = 'live';
      refreshInterrupted();
    } catch (e) {
      fail(e);
    }
  }
  async function discard(j: JobSummary) {
    if (confirmDiscard !== j.id) {
      confirmDiscard = j.id;
      return;
    }
    confirmDiscard = null;
    try {
      const n = await api.discardStoredJob(j.id);
      toast(t('jobs.discarded', { count: n }), 'success');
      await load();
    } catch (e) {
      fail(e);
    }
  }
</script>

<div class="jobs">
  <header class="head enter">
    <div>
      <h1>{t('jobs.title')}</h1>
      <p class="muted">{t('jobs.subtitle')}</p>
    </div>
    <button class="btn" onclick={load} disabled={loading}><Icon name="refresh" size={15} />{t('common.refresh')}</button>
  </header>

  {#if !loading && !jobs.length}
    <div class="empty glass enter">
      <Icon name="shield" size={34} stroke={1.5} />
      <p>{t('jobs.empty')}</p>
    </div>
  {:else}
    <ul class="list">
      {#each jobs as j, i (j.id)}
        {@const p = j.total_files ? j.done_files / j.total_files : 0}
        <li class="item glass" style="animation-delay:{i * 40}ms">
          <div class="top">
            <span class="icon"><Icon name="history" size={18} /></span>
            <div class="info">
              <strong>{j.sources.length === 1 ? baseName(j.sources[0]) : t('plan.nSources', { count: j.sources.length })}</strong>
              <span class="muted dest"><Icon name="arrow" size={12} />{middleTruncate(j.dest, 64)}</span>
              <span class="muted meta num">
                {t('jobs.progress', { done: fmtNumber(j.done_files), total: fmtNumber(j.total_files) })} · {fmtBytes(j.total_bytes)} · {fmtDate(j.modified_ms)}
                · <em>{j.ended ? t(`jobs.ended.${j.ended}`) : t('jobs.interrupted')}</em>
              </span>
            </div>
            <div class="actions">
              <button class="btn sm primary" onclick={() => resume(j)}><Icon name="play" size={14} />{t('jobs.resume')}</button>
              <button class="btn sm" class:danger={confirmDiscard === j.id} onclick={() => discard(j)}>
                <Icon name="trash" size={14} />{confirmDiscard === j.id ? t('jobs.confirmDiscard') : t('jobs.discard')}
              </button>
            </div>
          </div>
          <div class="bar"><i style="width:{p * 100}%"></i></div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .jobs {
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
    max-width: 640px;
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
    padding: 14px;
    display: grid;
    gap: 12px;
    animation: fade-up 0.4s var(--ease) both;
  }
  .top {
    display: flex;
    gap: 12px;
    align-items: center;
  }
  .icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border-radius: 11px;
    color: var(--violet);
    background: var(--grad-soft);
  }
  .info {
    flex: 1;
    display: grid;
    gap: 2px;
    min-width: 0;
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
  em {
    font-style: normal;
    color: var(--amber);
  }
  .actions {
    display: flex;
    gap: 6px;
    flex: none;
  }
</style>
