<script lang="ts">
  // Ambient background: three blurred aurora blobs (compositor-only CSS transforms) and a
  // sparse field of drifting light particles on a canvas at ~30 fps. Everything pauses
  // when the window is hidden or the user prefers reduced motion.
  import { app } from './store.svelte';

  let { energy = 0 }: { energy?: number } = $props();

  let canvas: HTMLCanvasElement;
  const reduce = typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;

  $effect(() => {
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    let w = 0;
    let h = 0;
    let dpr = 1;
    type P = { x: number; y: number; r: number; vx: number; vy: number; a: number; hue: number };
    let parts: P[] = [];

    const resize = () => {
      dpr = Math.min(window.devicePixelRatio || 1, 2);
      w = canvas.clientWidth;
      h = canvas.clientHeight;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const n = Math.round(Math.min(70, (w * h) / 22000));
      parts = Array.from({ length: n }, () => ({
        x: Math.random() * w,
        y: Math.random() * h,
        r: 0.6 + Math.random() * 1.6,
        vx: 0.05 + Math.random() * 0.25,
        vy: (Math.random() - 0.5) * 0.08,
        a: 0.15 + Math.random() * 0.5,
        hue: Math.random(),
      }));
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    let raf = 0;
    let last = 0;
    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      if (now - last < 33) return; // ~30 fps is plenty for ambience
      const dt = Math.min(3, (now - last) / 16.7);
      last = now;
      const boost = 1 + energy * 5;
      ctx.clearRect(0, 0, w, h);
      for (const p of parts) {
        p.x += p.vx * dt * boost;
        p.y += p.vy * dt;
        if (p.x > w + 10) {
          p.x = -10;
          p.y = Math.random() * h;
        }
        if (p.y < -10) p.y = h + 10;
        if (p.y > h + 10) p.y = -10;
        const tw = 0.6 + 0.4 * Math.sin(now / 900 + p.hue * 20);
        ctx.fillStyle = p.hue > 0.5 ? `rgba(34,211,238,${p.a * tw})` : `rgba(167,139,250,${p.a * tw})`;
        ctx.beginPath();
        ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
        ctx.fill();
        if (energy > 0.05 && p.r > 1.4) {
          // speed streaks while copying
          ctx.fillRect(p.x - p.vx * 14 * boost, p.y - 0.4, p.vx * 14 * boost, 0.8);
        }
      }
    };

    const drawStatic = () => {
      ctx.clearRect(0, 0, w, h);
      for (const p of parts) {
        ctx.fillStyle = `rgba(167,139,250,${p.a * 0.6})`;
        ctx.beginPath();
        ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
        ctx.fill();
      }
    };

    if (reduce || !app.visible) drawStatic();
    else raf = requestAnimationFrame(draw);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  });
</script>

<div class="bg" class:paused={!app.visible || reduce} aria-hidden="true">
  <div class="base"></div>
  <div class="blob b1"></div>
  <div class="blob b2"></div>
  <div class="blob b3"></div>
  <div class="grid"></div>
  <canvas bind:this={canvas}></canvas>
  <div class="vignette"></div>
</div>

<style>
  .bg {
    position: fixed;
    inset: 0;
    z-index: 0;
    overflow: hidden;
    pointer-events: none;
  }
  .base {
    position: absolute;
    inset: 0;
    background: radial-gradient(120% 90% at 10% 0%, #0d1534 0%, var(--bg) 55%), var(--bg);
  }
  :global(html[data-effect='mica']) .base,
  :global(html[data-effect='vibrancy']) .base {
    background: radial-gradient(120% 90% at 10% 0%, rgba(13, 21, 52, 0.78) 0%, rgba(5, 7, 20, 0.86) 55%);
  }
  .blob {
    position: absolute;
    width: 60vmax;
    height: 60vmax;
    border-radius: 50%;
    filter: blur(80px);
    opacity: 0.32;
    will-change: transform;
    animation: drift 26s ease-in-out infinite alternate;
  }
  .b1 {
    background: radial-gradient(circle, #22d3ee 0%, transparent 62%);
    top: -30vmax;
    left: -18vmax;
  }
  .b2 {
    background: radial-gradient(circle, #a78bfa 0%, transparent 62%);
    bottom: -34vmax;
    right: -20vmax;
    animation-duration: 32s;
    animation-delay: -8s;
  }
  .b3 {
    background: radial-gradient(circle, #3b82f6 0%, transparent 60%);
    top: 20%;
    left: 35%;
    width: 40vmax;
    height: 40vmax;
    opacity: 0.16;
    animation-duration: 38s;
    animation-delay: -16s;
  }
  .grid {
    position: absolute;
    inset: 0;
    background-image: linear-gradient(rgba(160, 175, 255, 0.035) 1px, transparent 1px),
      linear-gradient(90deg, rgba(160, 175, 255, 0.035) 1px, transparent 1px);
    background-size: 48px 48px;
    -webkit-mask-image: radial-gradient(ellipse at 50% 40%, #000 0%, transparent 75%);
    mask-image: radial-gradient(ellipse at 50% 40%, #000 0%, transparent 75%);
  }
  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }
  .vignette {
    position: absolute;
    inset: 0;
    background: radial-gradient(ellipse at center, transparent 50%, rgba(0, 0, 0, 0.45) 100%);
  }
  .paused .blob {
    animation-play-state: paused;
  }
  @keyframes drift {
    0% {
      transform: translate3d(0, 0, 0) scale(1);
    }
    50% {
      transform: translate3d(8vmax, 6vmax, 0) scale(1.12);
    }
    100% {
      transform: translate3d(-4vmax, 10vmax, 0) scale(0.95);
    }
  }
</style>
