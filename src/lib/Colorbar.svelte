<script lang="ts">
  // Colour bar: the colour map's gradient, tick labels, and triangles at ends whose
  // colours are saturated (values outside the range).
  import type { MapHeader } from "./api";
  let { header, lut }: { header: MapHeader; lut: number[] } = $props();
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
</script>

<div class="bar">
  <div class="row">
    <span class="end" style:visibility={header.extend[0] ? "visible" : "hidden"} style:color={rgb(0)}>◀</span>
    <canvas bind:this={canvas}></canvas>
    <span class="end" style:visibility={header.extend[1] ? "visible" : "hidden"} style:color={rgb(0.999)}>▶</span>
  </div>
  <div class="ticks">
    {#each header.ticks as [t, label]}
      <span style:left={`${t * 100}%`}>{label}</span>
    {/each}
  </div>
</div>

<style>
  .bar { width: min(560px, 90%); margin: 4px auto 0; font-size: 12px; }
  .row { display: flex; align-items: center; gap: 2px; }
  canvas { flex: 1; height: 14px; width: 100%; image-rendering: pixelated; border: 1px solid var(--line); }
  .end { font-size: 14px; line-height: 14px; }
  .ticks { position: relative; height: 16px; margin: 0 16px; }
  .ticks span { position: absolute; transform: translateX(-50%); white-space: nowrap; color: var(--fg); }
</style>
