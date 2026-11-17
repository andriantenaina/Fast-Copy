<script lang="ts">
  // Real-time throughput graph: 60 s sliding window, neon line with glow and gradient
  // fill, devicePixelRatio aware, rendered with rAF only while visible.
  import { app, speedHistory } from './store.svelte';
  import { speedParts } from './i18n.svelte';

  let { jobId, active = true }: { jobId: string; active?: boolean } = $props();

  let canvas: HTMLCanvasElement;
  const WINDOW = 60_000;
  const reduce = typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;

  $effect(() => {
    const id = jobId;
    const visible = app.visible;
    const running = active;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    let w = 0;
    let h = 0;
    let dpr = 1;
    let peakSmooth = 1;
    let frozenAt = 0;

    const resize = () => {
      dpr = Math.min(window.devicePixelRatio || 1, 3);
      w = canvas.clientWidth;
      h = canvas.clientHeight;
      canvas.width = Math.max(1, Math.round(w * dpr));
      canvas.height = Math.max(1, Math.round(h * dpr));
      draw(performance.now());
    };

    const draw = (now: number) => {
      const hist = speedHistory.get(id) ?? [];
      if (!running) {
        // Finished job: freeze the time axis at the last sample.
        frozenAt = hist.length ? hist[hist.length - 1].t : now;
        now = frozenAt;
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const padT = 18;
      const padB = 4;
      const gh = h - padT - padB;

      const visibleSamples = hist.filter((p) => now - p.t <= WINDOW + 1500);
      const peak = Math.max(1, ...visibleSamples.map((p) => p.v));
      peakSmooth = peakSmooth + (peak * 1.15 - peakSmooth) * 0.08;
      if (peakSmooth < peak) peakSmooth = peak * 1.05;
      const max = peakSmooth;

      // grid
      ctx.strokeStyle = 'rgba(160,175,255,0.08)';
      ctx.lineWidth = 1;
      ctx.font = '11px ' + getComputedStyle(canvas).fontFamily;
      ctx.fillStyle = 'rgba(182,189,224,0.75)';
      for (let i = 0; i <= 3; i++) {
        const y = Math.round(padT + (gh * i) / 3) + 0.5;
        ctx.beginPath();
        ctx.moveTo(0, y);
        ctx.lineTo(w, y);
        ctx.stroke();
        if (i < 3) {
          const [n, u] = speedParts((max * (3 - i)) / 3);
          ctx.fillText(`${n} ${u}`, 6, y - 4);
        }
      }
      for (let s = 10; s < 60; s += 10) {
        const x = Math.round(w - (s * 1000 * w) / WINDOW) + 0.5;
        ctx.beginPath();
        ctx.moveTo(x, padT);
        ctx.lineTo(x, h - padB);
        ctx.stroke();
      }

      if (visibleSamples.length < 2) return;
      const xy = visibleSamples.map((p) => [w - ((now - p.t) / WINDOW) * w, padT + gh - (p.v / max) * gh] as const);

      const path = new Path2D();
      path.moveTo(xy[0][0], xy[0][1]);
      for (let i = 1; i < xy.length; i++) {
        const [x0, y0] = xy[i - 1];
        const [x1, y1] = xy[i];
        const mx = (x0 + x1) / 2;
        path.bezierCurveTo(mx, y0, mx, y1, x1, y1);
      }

      // gradient fill
      const fill = new Path2D(path);
      fill.lineTo(xy[xy.length - 1][0], h);
      fill.lineTo(xy[0][0], h);
      fill.closePath();
      const g = ctx.createLinearGradient(0, padT, 0, h);
      g.addColorStop(0, 'rgba(34,211,238,0.35)');
      g.addColorStop(0.6, 'rgba(167,139,250,0.10)');
      g.addColorStop(1, 'rgba(167,139,250,0)');
      ctx.fillStyle = g;
      ctx.fill(fill);

      // glowing line
      const lg = ctx.createLinearGradient(0, 0, w, 0);
      lg.addColorStop(0, '#a78bfa');
      lg.addColorStop(1, '#22d3ee');
      ctx.save();
      ctx.shadowColor = 'rgba(34,211,238,0.85)';
      ctx.shadowBlur = 14;
      ctx.strokeStyle = lg;
      ctx.lineWidth = 2.2;
      ctx.lineJoin = 'round';
      ctx.stroke(path);
      ctx.restore();

      // head dot
      const [hx, hy] = xy[xy.length - 1];
      const pulse = running && !reduce ? 3 + Math.sin(now / 180) * 1.2 : 3;
      ctx.fillStyle = '#e0fbff';
      ctx.shadowColor = '#22d3ee';
      ctx.shadowBlur = 16;
      ctx.beginPath();
      ctx.arc(hx, hy, pulse, 0, Math.PI * 2);
      ctx.fill();
      ctx.shadowBlur = 0;
    };

    const ro = new ResizeObserver(resize);
    ro.observe(canvas);
    resize();

    let raf = 0;
    if (visible && running) {
      let last = 0;
      const loop = (now: number) => {
        raf = requestAnimationFrame(loop);
        // 60 fps normally, 15 fps when reduced motion is requested
        if (reduce && now - last < 66) return;
        last = now;
        draw(now);
      };
      raf = requestAnimationFrame(loop);
    }
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  });
</script>

<canvas bind:this={canvas} class="graph" aria-hidden="true"></canvas>

<style>
  .graph {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
