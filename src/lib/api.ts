// The front end's view of an nqv session. One interface, so the same components run
// in the desktop app (Tauri commands) and, later, in a browser (a WebSocket).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Region = { y0: number; y1: number; x0: number; x1: number };

export type DimView = {
  name: string;
  len: number;
  index: number;
  value: number | null;
  units: string | null;
  role: "y" | "x" | "along" | "step" | "fixed";
};

export type ProbeView = {
  cell: [number, number];
  coords: [number | null, number | null];
  value: number | null;
  error: string | null;
};

export type ViewState = {
  host: string | null;
  files: string[];
  compare: boolean;
  show: "diff" | "a" | "b";
  source: number;
  file: string;
  var: string;
  long_name: string | null;
  units: string | null;
  kind: "map" | "line" | "value";
  dims: DimView[];
  cmap: string;
  range_mode: string;
  interp: string;
  cvd: boolean;
  mask: string | null;
  mask_error: string | null;
  contour: string | null;
  contour_error: string | null;
  zoom: Region | null;
  probe: ProbeView | null;
  command_line: string;
};

export type VarItem = {
  source: number;
  name: string;
  file: string;
  long_name: string | null;
  kind: "map" | "line" | "value";
  dims: string;
};

export type Colormap = { name: string; cvd_safe: boolean; lut: number[] };

export type StatsView = {
  min: number;
  max: number;
  mean: number;
  std: number;
  valid: number;
  total: number;
  exact: boolean;
  weighted_by: string | null;
};

export type MapHeader = {
  w: number;
  h: number;
  min: number;
  max: number;
  log: boolean;
  cmap: number;
  missing: [number, number, number];
  data_min: number;
  data_max: number;
  exact: boolean;
  extend: [boolean, boolean];
  ticks: [number, string][];
  contours: number[];
  labels: [number, number, string][];
  stats: StatsView | null;
};

export type MapData = { header: MapHeader; values: Float32Array };

export type SeriesView = {
  x: number[];
  y: number[];
  min: number;
  max: number;
  log: boolean;
  xlabel: string;
  ylabel: string;
  mark: number | null;
};

export type DirEntry = { name: string; path: string; is_dir: boolean; is_netcdf: boolean; size: number };

export type OpenRequest = {
  paths: string[];
  diff?: boolean;
  join?: boolean;
  join_dim?: string | null;
  vs?: string[];
  host?: string | null;
};

export interface Api {
  /** Show the window, once its first page is drawn. */
  ready(): Promise<void>;
  /** The version of an update found, and later ones as they are found. */
  onUpdate(f: (version: string | null) => void): Promise<void>;
  /** Install the update found and restart; `progress` gets the bytes so far and in all. */
  installUpdate(progress: (done: number, total: number | null) => void): Promise<void>;
  open(req: OpenRequest): Promise<[ViewState, string | null]>;
  state(): Promise<ViewState>;
  apply(line: string): Promise<[ViewState, string | null]>;
  step(k: number, delta: number): Promise<ViewState>;
  setIndex(dim: number, index: number): Promise<ViewState>;
  setVar(source: number, name: string): Promise<ViewState>;
  vars(query: string): Promise<VarItem[]>;
  colormaps(): Promise<Colormap[]>;
  map(w: number, h: number): Promise<MapData>;
  series(points: number): Promise<SeriesView | null>;
  probe(px: number, py: number): Promise<ViewState>;
  closeProbe(): Promise<ViewState>;
  zoomAt(px: number, py: number, factor: number): Promise<ViewState>;
  pan(dx: number, dy: number): Promise<ViewState>;
  zoomReset(): Promise<ViewState>;
  listDir(path: string): Promise<DirEntry[]>;
  homeDir(): Promise<string | null>;
  recipes(): Promise<[string, string][]>;
  savePng(path: string, w: number, h: number): Promise<string>;
}

/** Split the bytes of a map: u32 header length, JSON header, padding, f32 values. */
export function decodeMap(buf: ArrayBuffer): MapData {
  const n = new DataView(buf).getUint32(0, true);
  const header = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 4, n))) as MapHeader;
  const start = Math.ceil((4 + n) / 4) * 4;
  return { header, values: new Float32Array(buf, start, header.w * header.h) };
}

export const tauriApi: Api = {
  open: (req) => invoke("open", { req }),
  state: () => invoke("state"),
  apply: (line) => invoke("apply", { line }),
  step: (k, delta) => invoke("step", { k, delta }),
  setIndex: (dim, index) => invoke("set_index", { dim, index }),
  setVar: (source, name) => invoke("set_var", { source, name }),
  vars: (query) => invoke("vars", { query }),
  ready: () => invoke("ready"),
  onUpdate: async (f) => {
    await listen<string | null>("update", (e) => f(e.payload));
    f(await invoke("update_available"));
  },
  installUpdate: async (progress) => {
    const stop = await listen<[number, number | null]>("update-progress", (e) => progress(...e.payload));
    try {
      await invoke("install_update");
    } finally {
      stop();
    }
  },
  colormaps: () => invoke("colormaps"),
  map: async (w, h) => decodeMap(await invoke<ArrayBuffer>("map", { w, h })),
  series: (points) => invoke("series", { points }),
  probe: (px, py) => invoke("probe", { px, py }),
  closeProbe: () => invoke("close_probe"),
  zoomAt: (px, py, factor) => invoke("zoom_at", { px, py, factor }),
  pan: (dx, dy) => invoke("pan", { dx, dy }),
  zoomReset: () => invoke("zoom_reset"),
  listDir: (path) => invoke("list_dir", { path }),
  homeDir: () => invoke("home_dir"),
  recipes: () => invoke("recipes"),
  savePng: (path, w, h) => invoke("save_png", { path, w, h }),
};

/** Compact number format, as the terminal and PNGs write numbers. */
export function fmt(v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return String(v ?? "");
  const a = Math.abs(v);
  if (a !== 0 && (a < 1e-3 || a >= 1e5)) return v.toExponential(3);
  const decimals = a === 0 ? 0 : Math.max(0, 3 - Math.floor(Math.log10(a)));
  let s = v.toFixed(decimals);
  if (s.includes(".")) s = s.replace(/0+$/, "").replace(/\.$/, "");
  return s === "-0" ? "0" : s;
}

/** About n round values in [lo, hi]. */
export function niceTicks(lo: number, hi: number, n: number): number[] {
  if (!(Number.isFinite(lo) && Number.isFinite(hi)) || hi <= lo) return [];
  const raw = (hi - lo) / n;
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const step = [1, 2, 5, 10]
    .map((m) => m * mag)
    .reduce((a, b) => (Math.abs(Math.log(a / raw)) <= Math.abs(Math.log(b / raw)) ? a : b));
  const out: number[] = [];
  for (let k = Math.ceil(lo / step); k <= Math.floor(hi / step); k++) out.push(k * step);
  return out;
}

/** Copy text: the Clipboard API, else the older copy command (WebKitGTK may lack the
 * former). */
export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    // fall through
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  const ok = document.execCommand("copy");
  area.remove();
  if (!ok) throw new Error("cannot copy to the clipboard");
}
