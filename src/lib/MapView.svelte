<script lang="ts">
  // The map: drawn with WebGL (else a 2D canvas) at the window's pixel size, contours and the clicked cell
  // drawn over it. Wheel zooms around the pointer, drag pans, click shows a cell's value
  // and series; hovering reports the value under the pointer.
  import { onMount } from "svelte";
  import type { Api, Colormap, MapData, ViewState } from "./api";
  import { fmt } from "./api";
  import { painter, type Painter } from "./painter";
  import Colorbar from "./Colorbar.svelte";

  let {
    api,
    view,
    colormaps,
    onstate,
    onhover,
    onerror,
    ondrawn,
  }: {
    api: Api;
    view: ViewState;
    colormaps: Colormap[];
    onstate: (s: ViewState) => void;
    onhover: (text: string) => void;
    onerror: (e: string) => void;
    /** After each map drawn. */
    ondrawn?: () => void;
  } = $props();

  let box: HTMLDivElement;
  let glCanvas: HTMLCanvasElement;
  let overlay: HTMLCanvasElement;
  let gl: Painter | null = null;
  let map: MapData | null = $state(null);
  let size = $state({ w: 0, h: 0 });

  // Statistics of the slice shown, as the terminal viewer's one-line summary.
  const statsLine = $derived.by(() => {
    const st = map?.header.stats;
    if (!st) return "";
    const approx = st.exact ? "" : "~";
    const by = st.weighted_by ? ` (by ${st.weighted_by})` : "";
    const share = (100 * st.valid) / Math.max(st.total, 1);
    return `${approx}min ${fmt(st.min)}  mean${by} ${fmt(st.mean)}  max ${fmt(st.max)}  std ${fmt(st.std)}  cells ${st.valid.toLocaleString()} (${share.toFixed(0)}%)${st.exact ? "" : "  (~ sample)"}`;
  });
  let busy = false;
  let again = false;

  onMount(() => {
    gl = painter(glCanvas);
    const ro = new ResizeObserver(() => {
      const r = box.getBoundingClientRect();
      size = { w: Math.floor(r.width), h: Math.floor(r.height) };
    });
    ro.observe(box);
    return () => ro.disconnect();
  });

  // New data whenever the view or the room for the map changes.
  $effect(() => {
    void view;
    if (size.w > 0 && size.h > 0) load(size.w, size.h);
  });

  async function load(w: number, h: number) {
    if (busy) {
      again = true;
      return;
    }
    busy = true;
    try {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      map = await api.map(Math.round(w * dpr), Math.round((h - 58) * dpr));
      draw();
      ondrawn?.();
    } catch (e) {
      onerror(String(e));
      ondrawn?.();
    } finally {
      busy = false;
      if (again) {
        again = false;
        load(size.w, size.h);
      }
    }
  }

  function draw() {
    if (!map || !gl) return;
    gl.setColormap(colormaps[map.header.cmap].lut);
    gl.draw(map.header, map.values);
    const { w, h } = map.header;
    overlay.width = w;
    overlay.height = h;
    const ctx = overlay.getContext("2d")!;
    const c = map.header.contours;
    // White lines with a dark edge read on any colours.
    for (const [style, width] of [["rgba(0,0,0,0.7)", 2.5], ["white", 1]] as const) {
      ctx.strokeStyle = style;
      ctx.lineWidth = width;
      ctx.beginPath();
      for (let i = 0; i < c.length; i += 4) {
        ctx.moveTo(c[i], c[i + 1]);
        ctx.lineTo(c[i + 2], c[i + 3]);
      }
      ctx.stroke();
    }
    ctx.font = `${11 * Math.min(window.devicePixelRatio || 1, 2)}px system-ui`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    for (const [x, y, label] of map.header.labels) {
      const tw = ctx.measureText(label).width + 6;
      ctx.fillStyle = "rgba(255,255,255,0.85)";
      ctx.fillRect(x - tw / 2, y - 8, tw, 16);
      ctx.fillStyle = "black";
      ctx.fillText(label, x, y);
    }
  }

  // Redraw with another colour map without new data.
  $effect(() => {
    void colormaps;
    draw();
  });

  /** Map pixel under a mouse event. */
  function pixel(e: MouseEvent): [number, number] | null {
    if (!map) return null;
    const r = glCanvas.getBoundingClientRect();
    const x = Math.floor(((e.clientX - r.left) / r.width) * map.header.w);
    const y = Math.floor(((e.clientY - r.top) / r.height) * map.header.h);
    if (x < 0 || y < 0 || x >= map.header.w || y >= map.header.h) return null;
    return [x, y];
  }

  let drag: { x: number; y: number; moved: boolean } | null = null;

  function down(e: MouseEvent) {
    drag = { x: e.clientX, y: e.clientY, moved: false };
  }

  async function up(e: MouseEvent) {
    const d = drag;
    drag = null;
    if (!d || !map) return;
    const r = glCanvas.getBoundingClientRect();
    const scale = map.header.w / r.width;
    const dx = (e.clientX - d.x) * scale;
    const dy = (e.clientY - d.y) * scale;
    try {
      if (Math.hypot(dx, dy) > 3 * scale) {
        if (view.zoom) onstate(await api.pan(Math.round(dx), Math.round(dy)));
      } else {
        const p = pixel(e);
        if (p) onstate(await api.probe(p[0], p[1]));
      }
    } catch (err) {
      onerror(String(err));
    }
  }

  function move(e: MouseEvent) {
    const p = pixel(e);
    if (!p || !map) return onhover("");
    const v = map.values[p[1] * map.header.w + p[0]];
    onhover(Number.isFinite(v) ? fmt(v) : "missing");
  }

  let wheelBusy = false;
  async function wheel(e: WheelEvent) {
    e.preventDefault();
    const p = pixel(e);
    if (!p || wheelBusy) return;
    wheelBusy = true;
    try {
      onstate(await api.zoomAt(p[0], p[1], e.deltaY < 0 ? 0.8 : 1.25));
    } catch (err) {
      onerror(String(err));
    } finally {
      wheelBusy = false;
    }
  }
</script>

<div class="map" bind:this={box}>
  <div class="stack" style:aspect-ratio={map ? `${map.header.w} / ${map.header.h}` : "auto"}>
    <canvas
      bind:this={glCanvas}
      onmousedown={down}
      onmouseup={up}
      onmousemove={move}
      onmouseleave={() => onhover("")}
      onwheel={wheel}
    ></canvas>
    <canvas class="overlay" bind:this={overlay}></canvas>
  </div>
  {#if map}
    <Colorbar header={map.header} lut={colormaps[map.header.cmap].lut} />
    {#if statsLine}<div class="stats">{statsLine}</div>{/if}
  {/if}
</div>

<style>
  .map { width: 100%; height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; min-height: 0; }
  .stack { position: relative; max-width: 100%; max-height: calc(100% - 58px); }
  .stats { font-size: 12px; opacity: 0.7; white-space: pre; font-variant-numeric: tabular-nums; }
  canvas { display: block; width: 100%; height: 100%; cursor: crosshair; }
  .overlay { position: absolute; inset: 0; pointer-events: none; }
</style>
