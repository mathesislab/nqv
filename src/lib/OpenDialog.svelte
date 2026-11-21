<script lang="ts">
  // Open files or folders: here (native dialogs) or on a host over ssh (a path there).
  // Compare two of them (A - B) or join files along time, as the command line can.
  import { open as pickPaths } from "@tauri-apps/plugin-dialog";
  import type { Api, OpenRequest, ViewState } from "./api";
  let { api, onopened, onclose, onerror }: {
    api: Api;
    onopened: (s: ViewState, message: string | null) => void;
    onclose: () => void;
    onerror: (e: string) => void;
  } = $props();
  let paths: string[] = $state([]);
  let host = $state("");
  let remotePath = $state("");
  let diff = $state(false);
  let join = $state(false);
  let busy = $state(false);

  async function choose(directory: boolean) {
    const r = await pickPaths({ multiple: true, directory, filters: directory ? undefined : [{ name: "NetCDF", extensions: ["nc", "nc4"] }] });
    if (r) paths = [...paths, ...(Array.isArray(r) ? r : [r])];
  }

  async function go() {
    const remote = host.trim() !== "";
    const list = remote ? remotePath.split(/\s+/).filter(Boolean) : paths;
    if (list.length === 0) return onerror(remote ? "give a path on the host" : "choose files or folders");
    const req: OpenRequest = { paths: list, diff, join, host: remote ? host.trim() : null };
    busy = true;
    try {
      const [s, msg] = await api.open(req);
      onopened(s, msg);
    } catch (e) {
      onerror(String(e));
    } finally {
      busy = false;
    }
  }
</script>

<div class="backdrop">
  <div class="dialog">
    <h2>Open</h2>
    <section>
      <h3>On this computer</h3>
      <button onclick={() => choose(false)}>Files…</button>
      <button onclick={() => choose(true)}>Folder…</button>
      {#if paths.length}
        <ul>{#each paths as p}<li>{p}</li>{/each}</ul>
        <button class="link" onclick={() => (paths = [])}>clear</button>
      {/if}
    </section>
    <section>
      <h3>On a host (over ssh)</h3>
      <input placeholder="host, e.g. albedo0" bind:value={host} />
      <input class="wide" placeholder="path(s) on the host, e.g. /albedo/work/…/run/" bind:value={remotePath} />
    </section>
    <section class="options">
      <label><input type="checkbox" bind:checked={diff} /> compare two (A − B)</label>
      <label><input type="checkbox" bind:checked={join} /> join along time</label>
    </section>
    <div class="actions">
      <button onclick={onclose}>Cancel</button>
      <button class="primary" disabled={busy} onclick={go}>{busy ? "Opening…" : "Open"}</button>
    </div>
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.35); display: flex; align-items: center; justify-content: center; z-index: 10; }
  .dialog { background: var(--bg); color: var(--fg); border-radius: 8px; padding: 16px 20px; width: min(620px, 92vw); box-shadow: 0 10px 40px rgba(0,0,0,0.3); }
  h2 { margin: 0 0 8px; font-size: 18px; }
  h3 { margin: 0 0 6px; font-size: 13px; opacity: 0.7; font-weight: 600; }
  section { margin: 12px 0; }
  ul { margin: 6px 0; padding-left: 18px; font-size: 12px; max-height: 120px; overflow-y: auto; }
  input:not([type]) { padding: 4px 6px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--fg); margin: 2px 0; }
  .wide { width: 100%; box-sizing: border-box; }
  .options label { margin-right: 16px; font-size: 13px; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; }
  .link { all: unset; font-size: 12px; color: var(--accent); cursor: pointer; }
</style>
