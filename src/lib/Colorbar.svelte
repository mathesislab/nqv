<script lang="ts">
  // Colour bar: the colour map's gradient, tick labels, triangles at ends whose colours
  // are saturated (values outside the range), and the range's two ends as boxes to
  // type a fixed range into (Enter applies, Escape restores).
  import type { MapHeader } from "./api";
  import { fmt } from "./api";
  let {
    header,
    lut,
    onrange,
  }: {
    header: MapHeader;
    lut: number[];
    /** A fixed colour range typed at the ends (log when the bar is). */
    onrange?: (min: number, max: number) => void;
  } = $props();
  let canvas: HTMLCanvasElement;
  const rgb = (t: number) => {
    const i = Math.min(255, Math.max(0, Math.floor(t * 256))) * 3;
    return `rgb(${lut[i]},${lut[i + 1]},${lut[i + 2]})`;
  };
  $effect(() => {
    const ctx = canvas.getContext("2d")!;
    canvas.width = 512;
    canvas.height = 1;
    for (let x = 0; x < 512; x++) {
      ctx.fillStyle = rgb((x + 0.5) / 512);
      ctx.fillRect(x, 0, 1, 1);
    }
  });

  // The ends as typed; back to the bar's own range whenever it changes.
  let lo = $state("");
  let hi = $state("");
  $effect(() => {
    lo = fmt(header.min);
    hi = fmt(header.max);
  });
  let bad = $state(false);
  function apply() {
    const [a, b] = [Number(lo), Number(hi)];
    const ok = lo.trim() !== "" && hi.trim() !== "" && Number.isFinite(a) && Number.isFinite(b) && a < b && (!header.log || a > 0);
    bad = !ok;
    if (ok && (a !== header.min || b !== header.max)) onrange?.(a, b);
  }
  function key(e: KeyboardEvent) {
    // keep the viewer's single-key commands out of the boxes
    e.stopPropagation();
    if (e.key === "Enter") apply();
    else if (e.key === "Escape") {
      lo = fmt(header.min);
      hi = fmt(header.max);
      bad = false;
      (e.currentTarget as HTMLInputElement).blur();
    }
  }
</script>

<div class="bar">
  <div class="row">
    {#if onrange}
      <input class="lim" class:bad bind:value={lo} onkeydown={key} onblur={apply} title={`Colour range minimum${header.log ? " (log: above 0)" : ""}; Enter applies`} aria-label="Colour range minimum" />
    {/if}
    <span class="end" style:visibility={header.extend[0] ? "visible" : "hidden"} style:color={rgb(0)}>◀</span>
    <canvas bind:this={canvas}></canvas>
    <span class="end" style:visibility={header.extend[1] ? "visible" : "hidden"} style:color={rgb(0.999)}>▶</span>
    {#if onrange}
      <input class="lim" class:bad bind:value={hi} onkeydown={key} onblur={apply} title="Colour range maximum; Enter applies" aria-label="Colour range maximum" />
    {/if}
  </div>
  <div class="ticks" class:boxed={!!onrange}>
    {#each header.ticks as [t, label]}
      <span style:left={`${t * 100}%`}>{label}</span>
    {/each}
  </div>
</div>

<style>
  .bar { width: min(720px, 94%); margin: 4px auto 0; font-size: 12px; }
  .row { display: flex; align-items: center; gap: 2px; }
  /* a percentage width: the canvas's own 512 x 1 shape must not size the layout */
  canvas { flex: 1 1 0; min-width: 0; width: 100%; height: 14px; image-rendering: pixelated; border: 1px solid var(--line); }
  .end { font-size: 14px; line-height: 14px; }
  .lim { flex: none; box-sizing: border-box; width: 72px; font: inherit; font-variant-numeric: tabular-nums; padding: 1px 4px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--fg); text-align: center; }
  .lim:first-child { margin-right: 4px; }
  .lim:last-child { margin-left: 4px; }
  .lim.bad { border-color: #c0392b; }
  .ticks { position: relative; height: 16px; margin: 0 16px; }
  /* box 72 + margin 4 + gap 2 + triangle 14 + gap 2 */
  .ticks.boxed { margin: 0 94px; }
  .ticks span { position: absolute; transform: translateX(-50%); white-space: nowrap; color: var(--fg); }
</style>
