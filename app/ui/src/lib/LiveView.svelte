<script lang="ts">
  import Icon from './Icon.svelte';
  import Gauge from './Gauge.svelte';
  import SpeedGraph from './SpeedGraph.svelte';
  import { app, fail, toast } from './store.svelte';
  import { t, fmtBytes, fmtDuration, fmtNumber, speedParts, middleTruncate, baseName } from './i18n.svelte';
  import { api, type Phase } from './api';

  const job = $derived(app.focusJob ? app.live[app.focusJob] : null);
  const entry = $derived(app.focusJob ? app.queue.find((e) => e.id === app.focusJob) : undefined);
  const snap = $derived(job?.snap ?? null);
  const report = $derived(job?.report ?? null);
  const finished = $derived(!!report);
  const pending = $derived(entry?.state === 'pending');
  const paused = $derived(entry?.state === 'paused' || !!snap?.paused);
  const total = $derived(snap?.bytes_total || entry?.total_bytes || 0);
  const pct = $derived(total ? (snap?.bytes_done ?? 0) / total : 0);
  const vpct = $derived(total ? (snap?.bytes_verified ?? 0) / total : 0);
  const gaugeState = $derived(
    !report ? 'running' : report.status === 'completed' ? 'success' : report.status === 'completed-with-errors' ? 'warning' : report.status === 'cancelled' ? 'warning' : 'error',
  );
  const speed = $derived(speedParts(snap && !paused ? snap.speed : 0));
  const phases: Phase[] = ['planning', 'copying', 'verifying', 'finalizing'];
  const phaseIdx = $derived(report ? 4 : snap ? phases.indexOf(snap.phase) : 0);
  const failedList = $derived(report?.errors?.length ? report.errors : (job?.failed ?? []));

  let confirmCancel = $state(false);
  let cancelTimer: ReturnType<typeof setTimeout> | undefined;
  let ejecting = $state(false);
  let showErrors = $state(true);
  let showRenamed = $state(false);

  function askCancel() {
    if (!job) return;
    if (!confirmCancel) {
      confirmCancel = true;
      clearTimeout(cancelTimer);
      cancelTimer = setTimeout(() => (confirmCancel = false), 3500);
      return;
    }
    confirmCancel = false;
    api.cancelJob(job.id).catch(fail);
  }
  function togglePause() {
    if (!job) return;
    (paused ? api.resumeJob(job.id) : api.pauseJob(job.id)).catch(fail);
  }
  async function eject() {
    if (!job?.dstVolume) return;
    ejecting = true;
    try {
      await api.ejectVolume(job.dstVolume.root);
      toast(t('live.ejected', { name: job.dstVolume.label || job.dstVolume.root }), 'success');
    } catch (e) {
      fail(e);
    } finally {
      ejecting = false;
    }
  }
  function newCopy() {
    app.view = 'home';
  }

  function onKey(e: KeyboardEvent) {
    if (!job || finished || (e.target as HTMLElement)?.closest('input,select,textarea,[role="dialog"]')) return;
    if (e.key === ' ' && !(e.target as HTMLElement)?.closest('button')) {
      e.preventDefault();
      togglePause();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if !job}
  <div class="empty enter">
    <div class="empty-icon"><Icon name="gauge" size={40} stroke={1.5} /></div>
    <h2>{t('live.none')}</h2>
    <p class="muted">{t('live.noneHint')}</p>
    <button class="btn primary" onclick={newCopy}><Icon name="plus" size={16} />{t('nav.home')}</button>
  </div>
{:else}
  <div class="live">
    <header class="head enter">
      <div class="title">
        <h1>
          {#if report}
            {t(`live.status.${report.status}`)}
          {:else if pending}
            {t('live.waiting')}
          {:else if paused}
            {t('live.paused')}
          {:else}
            {job.mode === 'move' ? t('live.moving') : t('live.copying')}
          {/if}
        </h1>
        <p class="route muted" title={`${job.sources.join('\n')}\n→ ${job.dest}`}>
          <span>{job.sources.length === 1 ? baseName(job.sources[0]) : t('plan.nSources', { count: job.sources.length })}</span>
          <Icon name="arrow" size={14} />
          <span>{middleTruncate(job.dest, 50)}</span>
        </p>
      </div>
      <ol class="phases" aria-label={t('live.phases')}>
        {#each phases as p, i}
          <li class:done={i < phaseIdx} class:active={i === phaseIdx && !report} aria-current={i === phaseIdx && !report ? 'step' : undefined}>
            <span class="dot">{#if i < phaseIdx}<Icon name="check" size={11} stroke={3} />{/if}</span>{t(`phase.${p}`)}
          </li>
          {#if i < phases.length - 1}<li class="sep" class:done={i < phaseIdx} aria-hidden="true"></li>{/if}
        {/each}
      </ol>
    </header>

    <div class="grid">
      <section class="glass gauge-panel enter" style="animation-delay:60ms">
        <div class="gauge-wrap">
          {#if gaugeState === 'success'}
            <div class="burst" aria-hidden="true">{#each Array(14) as _, i}<i style="--a:{(i * 360) / 14}deg; --d:{(i % 3) * 60}ms"></i>{/each}</div>
          {/if}
          <Gauge value={pct} verified={vpct} state={gaugeState} {paused}>
            {#if !report}
              <div class="bytes num">{fmtBytes(snap?.bytes_done ?? 0)} <span class="muted">/ {fmtBytes(total)}</span></div>
              {#if vpct > 0}<div class="verified num"><i></i>{t('live.verified', { size: fmtBytes(snap?.bytes_verified ?? 0) })}</div>{/if}
            {:else}
              <div class="bytes num">{fmtBytes(report.bytes_copied)}</div>
            {/if}
          </Gauge>
        </div>
        {#if !report}
          <div class="controls">
            <button class="btn" onclick={togglePause} disabled={pending && !paused} aria-keyshortcuts="Space">
              <Icon name={paused ? 'play' : 'pause'} size={16} />{paused ? t('live.resume') : t('live.pause')}
            </button>
            <button class="btn" class:danger={confirmCancel} onclick={askCancel}>
              <Icon name="x" size={16} />{confirmCancel ? t('live.confirmCancel') : t('live.cancel')}
            </button>
          </div>
        {:else}
          <div class="controls">
            {#if job.dstVolume?.external}
              <button class="btn primary" onclick={eject} disabled={ejecting}>
                {#if ejecting}<span class="spinner"></span>{:else}<Icon name="eject" size={16} />{/if}{t('live.eject')}
              </button>
            {/if}
            <button class="btn" onclick={newCopy}><Icon name="plus" size={16} />{t('live.newCopy')}</button>
          </div>
        {/if}
      </section>

      <section class="glass stats enter" style="animation-delay:120ms">
        {#if !report}
          <div class="speed">
            <span class="lbl muted">{t('live.speed')}</span>
            <div class="big num"><span class="grad-text">{speed[0]}</span><small>{speed[1]}</small></div>
          </div>
          <div class="kv">
            <div><span class="lbl muted">{t('live.eta')}</span><strong class="num">{paused ? '—' : fmtDuration(snap?.eta_secs)}</strong></div>
            <div><span class="lbl muted">{t('live.elapsed')}</span><strong class="num">{fmtDuration((snap?.elapsed_ms ?? 0) / 1000)}</strong></div>
            <div><span class="lbl muted">{t('live.files')}</span><strong class="num">{fmtNumber(snap?.files_done ?? 0)} / {fmtNumber(snap?.files_total ?? entry?.total_files ?? 0)}</strong></div>
          </div>
          <div class="counters">
            <span class="chip ok"><Icon name="check" size={13} stroke={2.6} />{t('live.copied', { count: snap?.files_done ?? 0 })}</span>
            <span class="chip skip"><Icon name="skip" size={13} />{t('live.skipped', { count: snap?.files_skipped ?? 0 })}</span>
            <span class="chip err" class:has={(snap?.files_failed ?? 0) > 0}><Icon name="alert-circle" size={13} />{t('live.errors', { count: snap?.files_failed ?? 0 })}</span>
          </div>
          <div class="current">
            <span class="lbl muted">{t('live.current')}</span>
            <p class="mono" title={snap?.current}>{snap?.current ? middleTruncate(snap.current, 72) : '—'}</p>
          </div>
        {:else}
          <div class="summary">
            <div><span class="lbl muted">{t('live.avgSpeed')}</span><strong class="num">{fmtBytes(report.avg_speed)}/s</strong></div>
            <div><span class="lbl muted">{t('live.duration')}</span><strong class="num">{fmtDuration(report.elapsed_ms / 1000)}</strong></div>
            <div><span class="lbl muted">{t('live.filesCopied')}</span><strong class="num">{fmtNumber(report.files_copied)}</strong></div>
            <div><span class="lbl muted">{t('live.filesSkipped')}</span><strong class="num">{fmtNumber(report.files_skipped)}</strong></div>
          </div>
          {#if report.status === 'completed' && job.dstVolume?.external && (job.dstVolume.fs === 'fat32' || job.dstVolume.fs === 'exfat')}
            <p class="hint"><Icon name="info" size={14} />{t('live.ejectHint')}</p>
          {/if}
          {#if report.warnings.length || job.warnings.length}
            <div class="list warn">
              <h3><Icon name="alert" size={15} />{t('live.warnings', { count: report.warnings.length || job.warnings.length })}</h3>
              <ul class="selectable">{#each (report.warnings.length ? report.warnings : job.warnings).slice(0, 50) as w}<li>{w}</li>{/each}</ul>
            </div>
          {/if}
          {#if report.renamed.length}
            <div class="list">
              <button class="btn sm ghost" aria-expanded={showRenamed} onclick={() => (showRenamed = !showRenamed)}>
                <Icon name={showRenamed ? 'up' : 'down'} size={14} />{t('live.renamed', { count: report.renamed.length })}
              </button>
              {#if showRenamed}
                <ul class="selectable mono small">{#each report.renamed.slice(0, 200) as [a, b]}<li>{a} → {b}</li>{/each}</ul>
              {/if}
            </div>
          {/if}
        {/if}
        {#if failedList.length}
          <div class="list err">
            <button class="btn sm ghost" aria-expanded={showErrors} onclick={() => (showErrors = !showErrors)}>
              <Icon name={showErrors ? 'up' : 'down'} size={14} />{t('live.errorList', { count: failedList.length })}
            </button>
            {#if showErrors}
              <ul class="selectable small">
                {#each failedList.slice(0, 200) as f}<li><span class="mono">{f.path || '—'}</span><span class="e">{f.error}</span></li>{/each}
              </ul>
            {/if}
          </div>
        {/if}
      </section>
    </div>

    <section class="glass graph-panel enter" style="animation-delay:180ms">
      <div class="graph-head">
        <h2 class="panel-title">{t('live.throughput')}</h2>
        <span class="muted small">{t('live.window')}</span>
      </div>
      <div class="graph-box"><SpeedGraph jobId={job.id} active={!finished} /></div>
      {#if job.recent.length && !finished}
        <div class="ticker" aria-live="off">
          {#each job.recent.slice(0, 4) as r (r)}<span class="mono">{baseName(r)}</span>{/each}
        </div>
      {/if}
    </section>
  </div>
{/if}

<style>
  .empty {
    height: 100%;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 10px;
    text-align: center;
  }
  .empty-icon {
    width: 88px;
    height: 88px;
    border-radius: 28px;
    display: grid;
    place-items: center;
    color: var(--cyan);
    background: var(--grad-soft);
    border: 1px solid var(--line-strong);
    box-shadow: var(--glow-cyan);
    margin-bottom: 8px;
  }
  .empty p {
    margin: 0 0 8px;
  }
  .live {
    max-width: 1200px;
    margin: 0 auto;
    display: grid;
    gap: 16px;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    align-items: flex-end;
    gap: 14px;
  }
  .title h1 {
    font-size: 24px;
  }
  .route {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 4px 0 0;
    max-width: 560px;
  }
  .route span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .phases {
    display: flex;
    align-items: center;
    list-style: none;
    margin: 0;
    padding: 6px 10px;
    border-radius: 99px;
    background: var(--glass);
    border: 1px solid var(--line);
    gap: 6px;
    font-size: 12.5px;
    color: var(--muted);
  }
  .phases li:not(.sep) {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 8px;
    border-radius: 99px;
    transition: color 0.3s, background 0.3s;
  }
  .phases .dot {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1.5px solid currentColor;
  }
  .phases li.done {
    color: var(--text-2);
  }
  .phases li.done .dot {
    background: var(--grad);
    border-color: transparent;
    color: #04121a;
  }
  .phases li.active {
    color: var(--text);
    background: var(--grad-soft);
  }
  .phases li.active .dot {
    border-color: var(--cyan);
    box-shadow: 0 0 0 0 rgba(34, 211, 238, 0.6);
    animation: ping 1.6s infinite;
  }
  .phases .sep {
    width: 18px;
    height: 1.5px;
    background: var(--line-strong);
  }
  .phases .sep.done {
    background: var(--grad);
  }
  @keyframes ping {
    70% {
      box-shadow: 0 0 0 7px rgba(34, 211, 238, 0);
    }
    100% {
      box-shadow: 0 0 0 0 rgba(34, 211, 238, 0);
    }
  }

  .grid {
    display: grid;
    grid-template-columns: minmax(0, 0.95fr) minmax(0, 1.05fr);
    gap: 16px;
  }
  @media (max-width: 980px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
  .gauge-panel {
    display: grid;
    justify-items: center;
    gap: 14px;
    padding: 22px;
  }
  .gauge-wrap {
    position: relative;
    width: 100%;
    display: grid;
    place-items: center;
  }
  .bytes {
    margin-top: 6px;
    font-size: 14px;
    font-weight: 600;
  }
  .verified {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    font-size: 12px;
    color: var(--violet);
  }
  .verified i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--violet);
    box-shadow: var(--glow-violet);
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px;
  }
  .burst {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    pointer-events: none;
  }
  .burst i {
    position: absolute;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--cyan);
    box-shadow: 0 0 12px var(--cyan);
    opacity: 0;
    animation: burst 1.1s calc(0.35s + var(--d)) var(--ease) forwards;
  }
  .burst i:nth-child(even) {
    background: var(--violet);
    box-shadow: 0 0 12px var(--violet);
  }
  @keyframes burst {
    0% {
      opacity: 1;
      transform: rotate(var(--a)) translateX(60px) scale(1);
    }
    100% {
      opacity: 0;
      transform: rotate(var(--a)) translateX(175px) scale(0.3);
    }
  }

  .stats {
    padding: 22px;
    display: flex;
    flex-direction: column;
    gap: 18px;
    min-width: 0;
  }
  .lbl {
    display: block;
    font-size: 11.5px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    margin-bottom: 4px;
  }
  .big {
    font-family: var(--font-display);
    font-size: 56px;
    font-weight: 700;
    letter-spacing: -0.03em;
    line-height: 1;
  }
  .big small {
    font-size: 20px;
    margin-left: 8px;
    color: var(--text-2);
    letter-spacing: 0;
  }
  .kv,
  .summary {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
  }
  .summary {
    grid-template-columns: repeat(2, 1fr);
  }
  .kv > div,
  .summary > div {
    padding: 12px;
    border-radius: 12px;
    background: rgba(5, 8, 24, 0.4);
    border: 1px solid var(--line);
  }
  .kv strong,
  .summary strong {
    font-size: 17px;
    font-family: var(--font-display);
  }
  .summary strong {
    font-size: 22px;
  }
  .counters {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border-radius: 99px;
    font-size: 12.5px;
    font-weight: 600;
    border: 1px solid var(--line);
    background: var(--glass);
  }
  .chip.ok {
    color: var(--green);
  }
  .chip.skip {
    color: var(--text-2);
  }
  .chip.err {
    color: var(--text-2);
  }
  .chip.err.has {
    color: var(--red);
    border-color: rgba(251, 113, 133, 0.4);
  }
  .current p {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    color: var(--cyan);
    font-size: 13px;
  }
  .list h3 {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13.5px;
    margin-bottom: 6px;
  }
  .list.warn h3 {
    color: var(--amber);
  }
  .list.err > .btn {
    color: var(--red);
  }
  .list ul {
    list-style: none;
    margin: 6px 0 0;
    padding: 8px 10px;
    border-radius: 10px;
    background: rgba(0, 0, 0, 0.25);
    max-height: 200px;
    overflow: auto;
    display: grid;
    gap: 6px;
    color: var(--text-2);
    font-size: 12.5px;
  }
  .list li {
    word-break: break-all;
  }
  .list.err li {
    display: grid;
    gap: 2px;
  }
  .list.err .e {
    color: #fda4b4;
  }
  .small {
    font-size: 12px;
  }
  .graph-panel {
    padding: 16px 18px 14px;
  }
  .graph-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .graph-head .panel-title {
    margin-bottom: 8px;
  }
  .graph-box {
    height: 150px;
  }
  .ticker {
    display: flex;
    gap: 14px;
    margin-top: 8px;
    overflow: hidden;
    font-size: 11.5px;
    color: var(--muted);
    white-space: nowrap;
  }
  .ticker span {
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 25%;
    animation: fade-up 0.35s var(--ease) both;
  }
</style>
