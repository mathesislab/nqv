<script lang="ts">
  // The nqv window: toolbar, variables, the map (or line plot) with its colour bar, the
  // series at a clicked cell, sliders for time and levels, a status line, and the same
  // `:` commands as the terminal viewer.
  import { onMount, tick } from "svelte";
  import { save } from "@tauri-apps/plugin-dialog";
  import { tauriApi as api, copyText, fmt } from "$lib/api";
  import type { Colormap, SeriesView, ViewState } from "$lib/api";
  import MapView from "$lib/MapView.svelte";
  import SeriesPanel from "$lib/SeriesPanel.svelte";
  import Sidebar from "$lib/Sidebar.svelte";
  import TimeBar from "$lib/TimeBar.svelte";
  import OpenDialog from "$lib/OpenDialog.svelte";

  let view: ViewState | null = $state(null);
  let colormaps: Colormap[] = $state([]);
  let series: SeriesView | null = $state(null);
  let message = $state("");
  let error = $state("");
  let hover = $state("");
  let opening = $state(false);
  let cmdline: string | null = $state(null);
  let cmdInput: HTMLInputElement | undefined = $state();
  let update: string | null = $state(null);
  let installing: string | null = $state(null);

  // The window stays hidden till its first page is drawn (no half-drawn page on Linux):
  // a map once MapView has drawn it, anything else at once.
  let shown = false;
  function showWindow() {
    if (shown) return;
    shown = true;
    api.ready();
  }

  onMount(async () => {
    api.onUpdate((v) => (update = v));
    api.onOpened(async () => {
      try {
        setState(await api.state());
        opening = false;
      } catch {
        opening = true;
      }
      await showNote();
    });
    colormaps = await api.colormaps();
    try {
      view = await api.state();
    } catch {
      opening = true;
    }
    await showNote();
    await tick();
    // not requestAnimationFrame: a hidden webview does not run it
    if (view?.kind !== "map") setTimeout(showWindow, 50);
  });

  async function showNote() {
    const n = await api.takeNote();
    if (n?.error) error = n.text;
    else if (n) message = n.text;
  }

  function setState(s: ViewState) {
    view = s;
    error = "";
  }

  // The series: of a line variable, or at the clicked cell of a map.
  $effect(() => {
    const s = view;
    if (!s || (s.kind === "map" && !s.probe) || s.kind === "value") {
      series = null;
      return;
    }
    api.series(800).then((v) => (series = v)).catch((e) => (error = String(e)));
  });

  async function run(line: string) {
    try {
      const [s, msg] = await api.apply(line);
      setState(s);
      message = msg ?? "";
    } catch (e) {
      error = String(e);
    }
  }

  async function act(f: () => Promise<ViewState>) {
    try { setState(await f()); } catch (e) { error = String(e); }
  }

  async function savePng() {
    const path = await save({ defaultPath: `${view?.var ?? "nqv"}.png`, filters: [{ name: "PNG", extensions: ["png"] }] });
    if (!path) return;
    try { message = await api.savePng(path, 1600, 1100); } catch (e) { error = String(e); }
  }

  async function installUpdate() {
    installing = "downloading";
    try {
      await api.installUpdate((done, total) => {
        installing = total ? `${Math.round((100 * done) / total)} %` : `${(done / 1e6).toFixed(1)} MB`;
      });
    } catch (e) {
      error = `update: ${e}`;
      installing = null;
    }
  }

  async function copyCommand() {
    if (!view) return;
    try {
      await copyText(view.command_line);
      message = "copied: " + view.command_line;
    } catch (e) {
      error = String(e);
    }
  }

  function key(e: KeyboardEvent) {
    if (!view || opening || cmdline !== null) return;
    const t = e.target as HTMLElement;
    if (t.tagName === "INPUT" || t.tagName === "SELECT") return;
    const k = e.key;
    if (k === "ArrowRight") act(() => api.step(0, 1));
    else if (k === "ArrowLeft") act(() => api.step(0, -1));
    else if (k === "ArrowUp") act(() => api.step(1, 1));
    else if (k === "ArrowDown") act(() => api.step(1, -1));
    else if (k === "Home") act(() => api.step(0, -1e9));
    else if (k === "End") act(() => api.step(0, 1e9));
    else if (k === ":") { cmdline = ""; setTimeout(() => cmdInput?.focus()); }
    else if (k === "Escape" && view.probe) act(() => api.closeProbe());
    else if (k === "c") nextCmap();
    else if (k === "r") run(`range ${view.range_mode === "auto" ? "sym" : view.range_mode === "sym" ? "log" : "auto"}`);
    else if (k === "i") run(`interp ${view.interp === "nearest" ? "bilinear" : "nearest"}`);
    else if (k === "d" && view.compare) run(`show ${view.show === "diff" ? "a" : view.show === "a" ? "b" : "diff"}`);
    else if (k === "z") act(() => api.zoomReset());
    else if (k === "o") opening = true;
    else if (k === "p") copyCommand();
    else return;
    e.preventDefault();
  }

  function nextCmap() {
    if (!view) return;
    const ok = colormaps.filter((c) => !view!.cvd || c.cvd_safe);
    const i = ok.findIndex((c) => c.name === view!.cmap);
    run(`cmap ${ok[(i + 1) % ok.length].name}`);
  }

  async function submitCommand() {
    const line = cmdline ?? "";
    cmdline = null;
    if (line.trim()) await run(line);
  }

  const probeText = $derived.by(() => {
    const p = view?.probe;
    if (!view || !p) return "";
    const [y, x] = view.dims.filter((d) => d.role === "y" || d.role === "x");
    const at = (d: typeof y, i: number, c: number | null) => `${d.name}=${i + 1}${c !== null ? ` (${fmt(c)})` : ""}`;
    const ll = p.lonlat ? ` (${p.lonlat})` : "";
    return `@ ${at(y, p.cell[0], p.coords[0])} ${at(x, p.cell[1], p.coords[1])}${ll} = ${p.value !== null ? fmt(p.value) : p.error}`;
  });

  // Projections offered: on its own grid, or one of these (a globe centred on the map).
  const projections: [string, string][] = [
    ["native", "own grid"],
    ["latlon", "lat-lon"],
    ["stere north", "polar N"],
    ["stere south", "polar S"],
    ["laea north", "equal-area N"],
    ["laea south", "equal-area S"],
    ["globe", "globe"],
  ];
  const projChoice = $derived.by(() => {
    const p = view?.proj;
    return !p ? "native" : p.startsWith("ortho") ? "globe" : p;
  });
  function setProj(choice: string) {
    const c = view?.centre;
    const spec = choice === "globe" ? (c ? `ortho ${c[0].toFixed(1)},${c[1].toFixed(1)}` : "ortho 0,60") : choice;
    run(`proj ${spec}`);
  }
</script>

<svelte:window onkeydown={key} />

<div class="app">
  <header>
    <button onclick={() => (opening = true)} title="Open (o)">Open…</button>
    {#if view}
      <label>
        colours
        <select value={view.cmap} onchange={(e) => run(`cmap ${(e.currentTarget as HTMLSelectElement).value}`)}>
          {#each colormaps as c}
            <option value={c.name} disabled={view.cvd && !c.cvd_safe}>{c.name}{c.cvd_safe ? "" : " (not cvd)"}</option>
          {/each}
        </select>
      </label>
      <div class="seg" title="Range (r)">
        {#each ["auto", "sym", "log"] as m}
          <button class:on={view.range_mode.startsWith(m)} onclick={() => run(`range ${m}`)}>{m}</button>
        {/each}
      </div>
      <button class:on={view.interp === "bilinear"} onclick={() => run(`interp ${view?.interp === "nearest" ? "bilinear" : "nearest"}`)} title="Bilinear interpolation (i)">smooth</button>
      <button class:on={view.cvd} onclick={() => run(`cvd ${view?.cvd ? "off" : "on"}`)} title="Colour-blind mode">cvd</button>
      {#if view.compare}
        <div class="seg" title="Comparison (d)">
          {#each [["diff", "A−B"], ["a", "A"], ["b", "B"]] as [m, label]}
            <button class:on={view.show === m} onclick={() => run(`show ${m}`)}>{label}</button>
          {/each}
        </div>
      {/if}
      {#if view.kind === "map" && (view.centre || view.proj)}
        <label title="Projection">
          projection
          <select value={projChoice} onchange={(e) => setProj((e.currentTarget as HTMLSelectElement).value)}>
            {#each projections as [spec, label]}<option value={spec}>{label}</option>{/each}
            {#if !projections.some(([s]) => s === projChoice)}<option value={projChoice}>{projChoice}</option>{/if}
          </select>
        </label>
      {/if}
      {#if view.kind === "map" && view.contour}
        <button class:on={view.labels} onclick={() => run(`labels ${view?.labels ? "off" : "on"}`)} title="Values on the contour lines">labels</button>
      {/if}
      {#if view.kind === "map" && view.centre}
        <button class:on={view.coast} onclick={() => run(`coast ${view?.coast ? "off" : "on"}`)} title="Coastlines">coast</button>
      {/if}
      {#if view.zoomed}<button onclick={() => act(() => api.zoomReset())} title="Whole map (z)">whole map</button>{/if}
      <span class="spacer"></span>
      <button onclick={copyCommand} title="Copy the command line for this view (p)">copy command</button>
      <button onclick={savePng}>Save PNG…</button>
    {/if}
    {#if update}
      {#if !view}<span class="spacer"></span>{/if}
      <button class="primary" disabled={installing !== null} onclick={installUpdate} title="Install and restart">
        {installing ? `updating… ${installing}` : `nqv ${update} available · Install`}
      </button>
    {/if}
  </header>

  {#if view}
    <Sidebar {api} {view} onstate={setState} onerror={(e) => (error = e)} />
    <main class:with-series={view.kind === "map" && series}>
      {#if view.kind === "map"}
        <div class="plot">
          <MapView {api} {view} {colormaps} onstate={setState} onhover={(t) => (hover = t)} onerror={(e) => (error = e)} ondrawn={showWindow} />
        </div>
        {#if series}
          <div class="panel">
            <SeriesPanel {series} />
            <button class="close" onclick={() => act(() => api.closeProbe())} title="Close (Esc)">×</button>
          </div>
        {/if}
      {:else if view.kind === "line" && series}
        <div class="plot"><SeriesPanel {series} /></div>
      {:else}
        <div class="plot value">{view.var} = {view.probe?.value ?? "…"} {view.units ?? ""}</div>
      {/if}
      <TimeBar {api} {view} onstate={setState} onerror={(e) => (error = e)} />
    </main>
    <footer>
      {#if cmdline !== null}
        <form onsubmit={(e) => { e.preventDefault(); submitCommand(); }}>
          :<input bind:this={cmdInput} bind:value={cmdline} onkeydown={(e) => e.key === "Escape" && (cmdline = null)}
            placeholder="v NAME | t VALUE | at DIM=VALUE | range … | cmap NAME | mask … | contour … | zoom … | recipe NAME | save NAME" />
        </form>
      {:else}
        <span class="var">{#if view.compare}<b>{view.show === "diff" ? "A−B" : view.show.toUpperCase()}</b> {/if}{view.file}:<b>{view.var}</b>{view.units ? ` [${view.units}]` : ""}</span>
        {#if view.mask}<span class:bad={view.mask_error}>mask: {view.mask}{view.mask_error ? ` (not applied: ${view.mask_error})` : ""}</span>{/if}
        {#if view.contour}<span class:bad={view.contour_error}>contour: {view.contour}{view.contour_error ? ` (not drawn: ${view.contour_error})` : ""}</span>{/if}
        {#if probeText}<span>{probeText}</span>{/if}
        {#if hover}<span class="hover">pointer: {hover}</span>{/if}
        <span class="spacer"></span>
        {#if error}<span class="bad">{error}</span>{:else if message}<span>{message}</span>{/if}
      {/if}
    </footer>
  {:else}
    <div class="empty"><p>nqv: open files or a folder, here or on a host.</p></div>
  {/if}
</div>

{#if opening}
  <OpenDialog {api}
    onopened={(s, msg) => { setState(s); message = msg ?? ""; opening = false; }}
    onclose={() => (opening = false)}
    onerror={(e) => (error = e)} />
{/if}

<style>
  :global(:root) {
    --bg: #fff; --fg: #222; --panel: #f6f6f7; --line: #d5d5d8; --hover: #e8e8ec; --accent: #2a6fdb;
    font-family: system-ui, -apple-system, sans-serif; color: var(--fg); background: var(--bg);
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) { --bg: #1d1e20; --fg: #e4e4e6; --panel: #25262a; --line: #3a3b40; --hover: #303136; --accent: #3d7fe6; }
  }
  :global(body) { margin: 0; }
  :global(button) { font: inherit; font-size: 13px; padding: 3px 9px; border: 1px solid var(--line); border-radius: 5px; background: var(--panel); color: var(--fg); cursor: pointer; }
  :global(button.on), :global(button.primary) { background: var(--accent); color: white; border-color: var(--accent); }
  .app { display: grid; grid-template-columns: 260px 1fr; grid-template-rows: auto 1fr auto; height: 100vh; }
  header { grid-column: 1 / 3; display: flex; align-items: center; gap: 8px; padding: 6px 10px; border-bottom: 1px solid var(--line); background: var(--panel); font-size: 13px; }
  header select { font: inherit; font-size: 13px; }
  .seg { display: flex; }
  .seg button { border-radius: 0; margin-left: -1px; }
  .seg button:first-child { border-radius: 5px 0 0 5px; }
  .seg button:last-child { border-radius: 0 5px 5px 0; }
  .spacer { flex: 1; }
  main { display: grid; grid-template-rows: 1fr auto; min-height: 0; min-width: 0; }
  main.with-series { grid-template-rows: 2fr 1fr auto; }
  .plot { min-height: 0; padding: 8px; }
  .plot.value { display: flex; align-items: center; justify-content: center; font-size: 20px; }
  .panel { position: relative; min-height: 0; border-top: 1px solid var(--line); padding: 4px 8px; }
  .close { position: absolute; top: 4px; right: 8px; padding: 0 6px; }
  footer { grid-column: 1 / 3; display: flex; gap: 14px; align-items: center; padding: 4px 10px; border-top: 1px solid var(--line); background: var(--panel); font-size: 12px; min-height: 22px; overflow: hidden; white-space: nowrap; }
  footer form { flex: 1; display: flex; gap: 4px; }
  footer form input { flex: 1; font: inherit; background: var(--bg); color: var(--fg); border: 1px solid var(--line); padding: 2px 4px; }
  .bad { color: #d33; }
  .hover { font-variant-numeric: tabular-nums; }
  .empty { grid-column: 1 / 3; display: flex; align-items: center; justify-content: center; opacity: 0.6; }
</style>
