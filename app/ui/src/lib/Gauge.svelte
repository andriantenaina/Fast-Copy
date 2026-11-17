<script lang="ts">
  // Circular gauge: main ring = bytes written, inner ring = bytes verified.
  // Morphs into an animated check mark (or cross) when the job ends.
  import { fmtNumber } from './i18n.svelte';
  import type { Snippet } from 'svelte';

  let {
    value = 0,
    verified = 0,
    state = 'running',
    paused = false,
    children,
  }: { value?: number; verified?: number; state?: 'running' | 'success' | 'warning' | 'error'; paused?: boolean; children?: Snippet } = $props();

  const R = 108;
  const R2 = 92;
  const C = 2 * Math.PI * R;
  const C2 = 2 * Math.PI * R2;
  const v = $derived(Math.max(0, Math.min(1, value)));
  const vv = $derived(Math.max(0, Math.min(1, verified)));
  const done = $derived(state !== 'running');
</script>

<div class="gauge" class:paused class:done data-state={state} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(v * 100)}>
  <svg viewBox="0 0 260 260" aria-hidden="true">
    <defs>
      <linearGradient id="gauge-g" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stop-color="#22d3ee" />
        <stop offset="1" stop-color="#a78bfa" />
      </linearGradient>
      <filter id="gauge-glow" x="-30%" y="-30%" width="160%" height="160%">
        <feGaussianBlur stdDeviation="5" result="b" />
        <feMerge><feMergeNode in="b" /><feMergeNode in="SourceGraphic" /></feMerge>
      </filter>
    </defs>
    <!-- tick marks -->
    <g class="ticks">
      {#each Array(60) as _, i}
        <line x1="130" y1="6" x2="130" y2={i % 5 === 0 ? 14 : 10} transform="rotate({i * 6} 130 130)" />
      {/each}
    </g>
    <circle class="track" cx="130" cy="130" r={R} />
    <circle
      class="ring"
      cx="130"
      cy="130"
      r={R}
      stroke="url(#gauge-g)"
      filter="url(#gauge-glow)"
      stroke-dasharray={C}
      stroke-dashoffset={C * (1 - (done && state === 'success' ? 1 : v))} />
    <circle class="track thin" cx="130" cy="130" r={R2} />
    <circle class="ring2" cx="130" cy="130" r={R2} stroke-dasharray={C2} stroke-dashoffset={C2 * (1 - vv)} />
    {#if !done && v > 0.002}
      <!-- comet head -->
      <g style="transform: rotate({v * 360}deg); transform-origin: 130px 130px" class="head-wrap">
        <circle class="head" cx={130 + R} cy="130" r="5" />
      </g>
    {/if}
    {#if state === 'success' || state === 'warning'}
      <path class="mark" d="M92 132 l26 26 l52 -56" />
    {:else if state === 'error'}
      <path class="mark x" d="M100 100 l60 60 M160 100 l-60 60" />
    {/if}
  </svg>
  <div class="center">
    {#if !done}
      <div class="pct num"><span>{fmtNumber(Math.floor(v * 1000) / 10, 1)}</span><small>%</small></div>
    {/if}
    {@render children?.()}
  </div>
</div>

<style>
  .gauge {
    position: relative;
    width: 100%;
    max-width: 270px;
    aspect-ratio: 1;
  }
  svg {
    width: 100%;
    height: 100%;
    transform: rotate(-90deg);
    overflow: visible;
  }
  .ticks line {
    stroke: rgba(160, 175, 255, 0.18);
    stroke-width: 1.5;
    stroke-linecap: round;
  }
  .track {
    fill: none;
    stroke: rgba(160, 175, 255, 0.09);
    stroke-width: 14;
  }
  .track.thin {
    stroke-width: 5;
  }
  .ring {
    fill: none;
    stroke-width: 14;
    stroke-linecap: round;
    transition: stroke-dashoffset 0.6s var(--ease), stroke 0.4s;
  }
  .ring2 {
    fill: none;
    stroke: #a78bfa;
    stroke-opacity: 0.85;
    stroke-width: 5;
    stroke-linecap: round;
    transition: stroke-dashoffset 0.6s var(--ease);
  }
  .head-wrap {
    transition: transform 0.6s var(--ease);
  }
  .head {
    fill: #fff;
    filter: drop-shadow(0 0 6px #22d3ee) drop-shadow(0 0 12px #22d3ee);
  }
  .paused .ring {
    animation: breathe 1.6s ease-in-out infinite;
  }
  @keyframes breathe {
    50% {
      opacity: 0.45;
    }
  }
  .mark {
    fill: none;
    stroke: url(#gauge-g);
    stroke-width: 14;
    stroke-linecap: round;
    stroke-linejoin: round;
    stroke-dasharray: 140;
    stroke-dashoffset: 140;
    transform: rotate(90deg);
    transform-origin: 130px 130px;
    filter: drop-shadow(0 0 10px rgba(34, 211, 238, 0.8));
    animation: draw 0.7s 0.35s var(--ease) forwards;
  }
  .mark.x {
    stroke: var(--red);
    filter: drop-shadow(0 0 10px rgba(251, 113, 133, 0.8));
    stroke-dasharray: 180;
    stroke-dashoffset: 180;
  }
  [data-state='warning'] .mark {
    stroke: var(--amber);
    filter: drop-shadow(0 0 10px rgba(251, 191, 36, 0.7));
  }
  [data-state='error'] .ring {
    stroke: var(--red);
  }
  [data-state='warning'] .ring {
    stroke: var(--amber);
  }
  .done[data-state='success'] {
    animation: pop 0.6s var(--spring);
  }
  @keyframes draw {
    to {
      stroke-dashoffset: 0;
    }
  }
  @keyframes pop {
    40% {
      transform: scale(1.06);
    }
  }
  .center {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    pointer-events: none;
  }
  .done .center {
    justify-content: flex-end;
    padding-bottom: 22%;
  }
  .pct {
    font-family: var(--font-display);
    font-size: 54px;
    font-weight: 700;
    letter-spacing: -0.03em;
    line-height: 1;
    background: linear-gradient(180deg, #fff, #bfe9ff);
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  .pct small {
    font-size: 24px;
    margin-left: 2px;
    color: var(--text-2);
    -webkit-text-fill-color: var(--text-2);
  }
</style>
