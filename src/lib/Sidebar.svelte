<script lang="ts">
  // Variables of the open files, filtered as you type (fuzzy, file:var), and the files.
  import type { Api, VarItem, ViewState } from "./api";
  let { api, view, onstate, onerror }: {
    api: Api; view: ViewState; onstate: (s: ViewState) => void; onerror: (e: string) => void;
  } = $props();
  let query = $state("");
  let items: VarItem[] = $state([]);
  $effect(() => {
    void view.files;
    api.vars(query).then((v) => (items = v)).catch((e) => onerror(String(e)));
  });
  async function pick(v: VarItem) {
    try { onstate(await api.setVar(v.source, v.name)); } catch (e) { onerror(String(e)); }
  }
  const several = $derived(view.files.length > 1);
</script>

<aside>
  <input placeholder="filter variables (file:var)" bind:value={query} />
  <ul>
    {#each items as v (v.source + ":" + v.name)}
      <li class:current={v.source === view.source && v.name === view.var}>
        <button onclick={() => pick(v)} title={`${v.file}: ${v.dims}`}>
          <span class="kind">{v.kind}</span>
          {#if several}<span class="file">{v.file}:</span>{/if}<b>{v.name}</b>
          {#if v.long_name}<span class="long">{v.long_name}</span>{/if}
        </button>
      </li>
    {/each}
  </ul>
  <div class="files">
    {#if view.host}<div class="host">on {view.host}</div>{/if}
    {#each view.files as f}<div>{f}</div>{/each}
  </div>
</aside>

<style>
  aside { display: flex; flex-direction: column; min-height: 0; border-right: 1px solid var(--line); background: var(--panel); }
  input { margin: 8px; padding: 4px 6px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--fg); }
  ul { list-style: none; margin: 0; padding: 0; overflow-y: auto; flex: 1; }
  li button { all: unset; display: block; box-sizing: border-box; width: 100%; padding: 3px 8px; cursor: pointer; font-size: 13px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  li button:hover { background: var(--hover); }
  li.current button { background: var(--accent); color: white; }
  .kind { display: inline-block; width: 34px; font-size: 10px; opacity: 0.6; }
  .file { opacity: 0.6; }
  .long { margin-left: 6px; opacity: 0.6; font-size: 12px; }
  .files { border-top: 1px solid var(--line); padding: 6px 8px; font-size: 11px; opacity: 0.7; max-height: 25%; overflow-y: auto; }
  .host { font-weight: bold; }
</style>
