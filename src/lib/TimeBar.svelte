<script lang="ts">
  // A slider per stepped dimension (time, levels), with its coordinate value.
  import type { Api, ViewState } from "./api";
  import { fmt } from "./api";
  let { api, view, onstate, onerror }: {
    api: Api; view: ViewState; onstate: (s: ViewState) => void; onerror: (e: string) => void;
  } = $props();
  const steps = $derived(view.dims.map((d, k) => ({ d, k })).filter(({ d }) => d.role === "step"));
  async function set(k: number, i: number) {
    try { onstate(await api.setIndex(k, i)); } catch (e) { onerror(String(e)); }
  }
</script>

{#each steps as { d, k } (d.name)}
  <div class="step">
    <span class="name">{d.name}</span>
    <input type="range" min="0" max={d.len - 1} value={d.index}
      oninput={(e) => set(k, +(e.currentTarget as HTMLInputElement).value)} />
    <span class="value">{d.index + 1}/{d.len}{#if d.value !== null}&nbsp;({fmt(d.value)}{d.units ? " " + d.units : ""}){/if}</span>
  </div>
{/each}

<style>
  .step { display: flex; align-items: center; gap: 8px; padding: 2px 10px; font-size: 13px; }
  .name { width: 70px; text-align: right; opacity: 0.8; }
  input { flex: 1; }
  .value { min-width: 160px; font-variant-numeric: tabular-nums; }
</style>
