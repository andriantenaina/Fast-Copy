<script lang="ts">
  import Modal from './Modal.svelte';
  import Icon from './Icon.svelte';
  import { app, analyse, startPlan } from './store.svelte';
  import { t, fmtBytes, fmtNumber, middleTruncate } from './i18n.svelte';
  import type { Issue, PlanView } from './api';

  let { plan }: { plan: PlanView } = $props();

  let forceSpace = $state(false);
  let starting = $state(false);
  let showRenamed = $state(false);

  const sevIcon = { error: 'alert-circle', warning: 'alert', info: 'info' } as const;
  const order = { error: 0, warning: 1, info: 2 } as const;
  const issues = $derived([...plan.issues].sort((a, b) => order[a.severity] - order[b.severity]));
  const spaceIssue = $derived(plan.issues.some((i) => i.severity === 'error' && i.kind === 'insufficient-space'));
  const otherBlocking = $derived(plan.issues.filter((i) => i.severity === 'error' && i.kind !== 'insufficient-space'));
  const canStart = $derived(otherBlocking.length === 0 && (!spaceIssue || forceSpace) && !starting);
  const dst = $derived(plan.dstVolume);

  function issueText(i: Issue): string {
    const p = {
      count: i.count ?? 0,
      path: i.path ?? '',
      fs: i.fs ?? dst.fsLabel,
      needed: fmtBytes(i.needed),
      available: fmtBytes(i.available),
      largest: fmtBytes(i.largest),
      limit: fmtBytes(i.limit),
      error: i.error ?? '',
    };
    const key = `issue.${i.kind}`;
    const s = t(key, p);
    return s === key ? i.message : s;
  }

  async function start() {
    starting = true;
    await startPlan(forceSpace);
    starting = false;
  }

  function formatDst() {
    app.formatTarget = dst;
  }
</script>

<Modal title={t('plan.title')} width={780} onclose={() => (app.plan = null)}>
  <div class="route">
    <div class="vol">
      <span class="lbl muted">{t('plan.from')}</span>
      <strong title={plan.sources.join('\n')}>{plan.sources.length === 1 ? middleTruncate(plan.sources[0], 44) : t('plan.nSources', { count: plan.sources.length })}</strong>
      <span class="badge fs-{plan.srcVolume.fs}">{plan.srcVolume.fsLabel}</span>
    </div>
    <div class="arrow" aria-hidden="true"><Icon name={plan.mode === 'move' ? 'move' : 'arrow'} size={20} /></div>
    <div class="vol">
      <span class="lbl muted">{t('plan.to')}</span>
      <strong title={plan.dest}>{middleTruncate(plan.dest, 44)}</strong>
      <span class="badge fs-{dst.fs}">{dst.fsLabel}</span>
    </div>
  </div>

  <div class="totals">
    <div class="tot"><span class="v num">{fmtNumber(plan.totalFiles)}</span><span class="k muted">{t('plan.files', { count: plan.totalFiles })}</span></div>
    <div class="tot"><span class="v num">{fmtBytes(plan.totalBytes)}</span><span class="k muted">{t('plan.size')}</span></div>
    <div class="tot"><span class="v num">{fmtNumber(plan.totalDirs)}</span><span class="k muted">{t('plan.dirs', { count: plan.totalDirs })}</span></div>
    <div class="tot"><span class="v small">{plan.sameVolume ? t('plan.rename') : plan.profile}</span><span class="k muted">{t('plan.profile')}</span></div>
  </div>
  {#if dst.freeBytes != null && !plan.sameVolume}
    <div class="space">
      <div class="bar" class:warn={spaceIssue}>
        <i style="width:{Math.min(100, (plan.totalBytes / Math.max(1, dst.freeBytes)) * 100)}%"></i>
      </div>
      <span class="muted num">{t('plan.spaceUse', { size: fmtBytes(plan.totalBytes), free: fmtBytes(dst.freeBytes) })}</span>
    </div>
  {/if}

  {#if issues.length}
    <ul class="issues" aria-label={t('plan.issues')}>
      {#each issues as i, n}
        <li class="issue sev-{i.severity}" style="animation-delay:{n * 50}ms">
          <span class="sicon"><Icon name={sevIcon[i.severity]} size={18} /></span>
          <div class="itext">
            <p class="selectable">{issueText(i)}</p>
            <div class="iactions">
              {#if (i.kind === 'file-too-large' || i.kind === 'will-split' || i.kind === 'read-only-destination') && dst.formattable}
                <button class="btn sm" onclick={formatDst}><Icon name="format" size={14} />{t('plan.formatExfat')}</button>
              {/if}
              {#if i.kind === 'file-too-large'}
                <button class="btn sm" onclick={() => analyse({ split: 'auto' })}><Icon name="scissors" size={14} />{t('plan.split')}</button>
              {/if}
              {#if i.kind === 'insufficient-space'}
                <button class="btn sm" class:danger={!forceSpace} aria-pressed={forceSpace} onclick={() => (forceSpace = !forceSpace)}>
                  <Icon name={forceSpace ? 'check' : 'alert'} size={14} />{t('plan.forceAnyway')}
                </button>
              {/if}
              {#if i.kind === 'names-changed' && plan.renamedCount}
                <button class="btn sm ghost" aria-expanded={showRenamed} onclick={() => (showRenamed = !showRenamed)}>
                  <Icon name={showRenamed ? 'up' : 'down'} size={14} />{t('plan.showRenamed')}
                </button>
              {/if}
            </div>
            {#if i.kind === 'names-changed' && showRenamed}
              <ul class="renamed selectable">
                {#each plan.renamedSample as [a, b]}
                  <li><span class="mono">{a}</span><Icon name="arrow" size={12} /><span class="mono">{b}</span></li>
                {/each}
                {#if plan.renamedCount > plan.renamedSample.length}<li class="muted">{t('plan.andMore', { count: plan.renamedCount - plan.renamedSample.length })}</li>{/if}
              </ul>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="allgood"><Icon name="check" size={18} />{t('plan.allGood')}</p>
  {/if}

  {#snippet footer()}
    <button class="btn" onclick={() => (app.plan = null)}>{t('common.back')}</button>
    <button class="btn primary lg" disabled={!canStart} onclick={start} data-autofocus>
      {#if starting}<span class="spinner"></span>{:else}<Icon name="play" size={16} />{/if}
      {plan.mode === 'move' ? t('plan.startMove') : t('plan.start')}
    </button>
  {/snippet}
</Modal>

<style>
  .route {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    gap: 12px;
    align-items: center;
    padding: 14px;
    border-radius: 14px;
    background: rgba(5, 8, 24, 0.45);
    border: 1px solid var(--line);
  }
  .vol {
    display: grid;
    gap: 4px;
    justify-items: start;
    min-width: 0;
  }
  .vol strong {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lbl {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .arrow {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    color: var(--cyan);
    background: var(--grad-soft);
    box-shadow: var(--glow-cyan);
    animation: nudge 1.8s var(--ease) infinite;
  }
  @keyframes nudge {
    50% {
      transform: translateX(4px);
    }
  }
  .totals {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 10px;
    margin: 14px 0 10px;
  }
  .tot {
    display: grid;
    gap: 2px;
    padding: 12px 14px;
    border-radius: 12px;
    background: var(--glass);
    border: 1px solid var(--line);
  }
  .tot .v {
    font-family: var(--font-display);
    font-size: 22px;
    font-weight: 650;
  }
  .tot .v.small {
    font-size: 15px;
    padding-top: 5px;
    padding-bottom: 3px;
  }
  .tot .k {
    font-size: 12px;
  }
  .space {
    display: grid;
    gap: 6px;
    margin-bottom: 14px;
    font-size: 12.5px;
  }
  .issues {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }
  .issue {
    display: flex;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 12px;
    border: 1px solid color-mix(in srgb, currentColor 35%, transparent);
    background: color-mix(in srgb, currentColor 7%, rgba(5, 8, 24, 0.4));
    animation: fade-up 0.4s var(--ease) both;
  }
  .sicon {
    flex: none;
    padding-top: 1px;
  }
  .itext {
    flex: 1;
    min-width: 0;
  }
  .itext p {
    margin: 0;
    color: var(--text);
  }
  .iactions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .iactions:not(:empty) {
    margin-top: 10px;
  }
  .renamed {
    list-style: none;
    margin: 10px 0 0;
    padding: 8px 10px;
    max-height: 180px;
    overflow: auto;
    border-radius: 8px;
    background: rgba(0, 0, 0, 0.25);
    color: var(--text-2);
    font-size: 12px;
    display: grid;
    gap: 4px;
  }
  .renamed li {
    display: flex;
    align-items: center;
    gap: 6px;
    word-break: break-all;
  }
  .allgood {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--green);
    font-weight: 600;
    margin: 6px 0 0;
  }
</style>
