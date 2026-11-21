<script lang="ts">
  // A line plot: the series of a 1D variable, or at the clicked cell of a map.
  import type { SeriesView } from "./api";
  import { fmt, niceTicks } from "./api";
  let { series, color = "#2166ac" }: { series: SeriesView; color?: string } = $props();
  let canvas: HTMLCanvasElement;
  let box: HTMLDivElement;
  let size = $state({ w: 0, h: 0 });

  $effect(() => {
    const ro = new ResizeObserver(() => {
      const r = box.getBoundingClientRect();
      size = { w: r.width, h: r.height };
    });
    ro.observe(box);
    return () => ro.disconnect();
  });

  $effect(() => {
    const { w, h } = size;
    if (w < 50 || h < 40) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    const ctx = canvas.getContext("2d")!;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, w, h);
    const css = getComputedStyle(box);
    const fg = css.getPropertyValue("--fg") || "#222";
    const grid = css.getPropertyValue("--line") || "#ccc";
    const L = 64, R = 12, T = 18, B = 34;
    const s = series;
    const xs = s.x.filter(Number.isFinite);
    let [x0, x1] = [Math.min(...xs), Math.max(...xs)];
    if (!(x1 > x0)) [x0, x1] = [x0 - 1, x0 + 1];
    let [y0, y1] = [s.min, s.max];
    if (!(y1 > y0)) [y0, y1] = [y0 - (Math.abs(y0) * 0.01 || 1), y0 + (Math.abs(y0) * 0.01 || 1)];
    const ty = (v: number) => (s.log ? Math.log10(v) : v);
    const [a, b] = [ty(y0), ty(y1)];
    const px = (x: number) => L + ((x - x0) / (x1 - x0)) * (w - L - R);
    const py = (y: number) => T + (1 - (ty(y) - a) / (b - a)) * (h - T - B);
    ctx.font = "11px system-ui";
    ctx.fillStyle = fg;
    ctx.strokeStyle = grid;
    ctx.lineWidth = 1;
    ctx.textAlign = "center";
    for (const t of niceTicks(x0, x1, 6)) {
      ctx.beginPath(); ctx.moveTo(px(t), T); ctx.lineTo(px(t), h - B); ctx.stroke();
      ctx.fillText(fmt(t), px(t), h - B + 14);
    }
    ctx.textAlign = "right";
    const yt = s.log
      ? niceTicks(a, b, 4).map((e) => Math.pow(10, e))
      : niceTicks(y0, y1, 5);
    for (const t of yt) {
      ctx.beginPath(); ctx.moveTo(L, py(t)); ctx.lineTo(w - R, py(t)); ctx.stroke();
      ctx.fillText(fmt(t), L - 6, py(t) + 4);
    }
    ctx.textAlign = "left";
    ctx.fillText(s.ylabel, 4, 12);
    ctx.textAlign = "right";
    ctx.fillText(s.xlabel, w - R, h - 4);
    if (s.mark !== null) {
      ctx.strokeStyle = "rgba(120,120,120,0.8)";
      ctx.beginPath(); ctx.moveTo(px(s.mark), T); ctx.lineTo(px(s.mark), h - B); ctx.stroke();
    }
    ctx.save();
    ctx.beginPath(); ctx.rect(L, T, w - L - R, h - T - B); ctx.clip();
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    let pen = false;
    s.x.forEach((x, i) => {
      const y = s.y[i];
      if (!Number.isFinite(x) || !Number.isFinite(y) || (s.log && y <= 0)) { pen = false; return; }
      if (pen) ctx.lineTo(px(x), py(y)); else ctx.moveTo(px(x), py(y));
      pen = true;
    });
    ctx.stroke();
    ctx.restore();
    ctx.strokeStyle = fg;
    ctx.strokeRect(L, T, w - L - R, h - T - B);
  });
</script>

<div class="series" bind:this={box}><canvas bind:this={canvas}></canvas></div>

<style>
  .series { width: 100%; height: 100%; min-height: 0; }
  canvas { width: 100%; height: 100%; display: block; }
</style>
