# nqv architecture

Status: draft for review, 2026-11-21. Background and prior art: [initial-discussion.md](initial-discussion.md).

## Goals

- A quick-look viewer for NetCDF (later Zarr, HDF5) that replaces ncview without X11.
- Launched from the ssh shell on the HPC where the file was found: `nqv file.nc`, look, `q`, back at the prompt.
- Works in any terminal. A better terminal or a local app gives a better picture, never a different workflow.
- Scales to large structured and unstructured grids: cost follows screen size, not grid size.
- Simple screen, with keys for the choices that matter (variable, slice, range, colour map, region).

## Layers

Everything is one pipeline. Display front ends differ only in where the pipeline is cut and which output encoder sits at the end.

```
source(s) ─► nqv-core ─► Raster ─► nqv-render ─► RGBA ─► encoder
             read, slice,   values    norm, cmap,          terminal tiers,
             grid lookup,   + mask    colour bar           native window,
             ranges, probe  at screen                      browser canvas
             size
```

| Crate | Role | Depends on |
|---|---|---|
| `nqv-core` | Catalog (files, groups, variables, grids), navigation, slicing, pixel→cell lookup, ranges, probes, diffs, caching, prefetch. No display code. | NetCDF reader, kiddo, rayon |
| `nqv-render` | Normalisation, colour maps, colour bar, line plots. Pure functions; also builds to WASM. | none |
| `nqv-proto` | Protocol messages and Raster encoding (f16 + zstd). | core types |
| `nqv` | The single HPC binary: terminal UI, browse mode, `nqv serve --stdio`. | all of the above, ratatui |
| `nqv-local` | Laptop agent: native window and web client, reached from the HPC via a forwarded socket. | proto, render |

Cut points:

- **Terminal (default).** Whole pipeline in one process on the HPC. No protocol involved.
- **Remote.** Cut after `Raster`. Values cross the wire, the client colours them, so colour and range changes are instant.
- **Mesh (later).** Send the mesh once, then per-step node values, render on the GPU. A separate negotiated wire format.

The `Raster` (values + mask on the view's pixel grid) is the common format for all grid types. Regular grids produce it with a strided read; curvilinear and unstructured grids with a cached pixel→cell index (KD-tree, built once per grid and view, stored in `~/.cache/nqv` keyed by grid hash).

## Terminal output tiers

Chosen at start-up by querying the terminal; `--gfx=kitty|sixel|iterm|blocks|blocks256` overrides.

| Tier | Resolution | Terminals (checked 2026-11) |
|---|---|---|
| kitty graphics | full pixels | Ghostty, kitty, WezTerm, Konsole (partial), iTerm2 (partial) |
| sixel | full pixels | iTerm2, WezTerm, Konsole, foot, Windows Terminal ≥ 1.22, xterm (only as VT340), VS Code (setting off by default) |
| iTerm2 inline | full pixels | iTerm2, WezTerm, Konsole, VS Code (same setting) |
| half-block `▀`, 24-bit | 2 px per cell | everything else with true colour, incl. Terminal.app (macOS 26+) and GNOME Terminal |
| half-block, 256 colour | 2 px per cell | older Terminal.app and anything else |

The half-block tier is the guaranteed path and a first-class target, not a degraded mode. Two common terminals have no image protocol at all: Terminal.app and GNOME Terminal (VTE builds sixel only if compiled with it; major distros don't). Under tmux, half-blocks are the only reliable tier: sixel needs a tmux built with image support, and kitty graphics only work through DCS passthrough, where they break when panes move. Line plots (1D variables) draw with braille characters in every tier.

`ratatui-image` (11.x) already implements kitty, sixel, iTerm2 and half-blocks, with detection via `Picker::from_query_stdio()`.

## Data model

```
Catalog ─► Source ─► Group ─► Variable
                     │
                     └─ Grid (shared by variables with identical coordinates)
```

- **Source**: one file, or a set of files stitched along a record dimension (`nqv run/*.nc` where files share a structure and differ only in time).
- **Group**: a NetCDF-4 group, or a whole file when a folder is opened as a collection.
- **Variable**: any numeric variable. Its *view kind* follows from the dimensions left free after fixing the others: two spatial dims → map, one dim → line plot.
- **Grid**: identified by a hash of coordinate *values* (and `grid_mapping`), not by dimension names. Example: `CLIMBER-X_SMB_NH-16KM_*.nc` uses `x,y` and `ice_NH-16KM.nc` uses `xc,yc`, but both are the same 781×781 grid.

### Folder as a collection

`nqv .` on a run folder opens every NetCDF file as one catalog, each file a group. The variable picker (`v`, fuzzy search) searches across all files and shows `file:var`. Non-NetCDF files are ignored; subfolders are not entered unless asked (`--recursive`).

Reference case, a CLIMBER-X run on Albedo (`.../climber-x/output/lgm_ice_nh_coords/igeo2`):

- 32 NetCDF files, mixed netCDF-4 and classic formats.
- At least five grids: 5° lat/lon (72×36, with staggered `lonu`/`latv`), 0.125° (2880×1440), 512×256 (VILMA), 16 km polar stereographic (781×781 with `x2D`/`y2D` and `crs`), and a 171×171 `K,L` grid.
- Record dimension named `time` or `epoch`.
- `*_ts.nc` files are time series: mostly variables of `(time)` only, though some declare grid dimensions (`ice_NH-16KM_ts.nc` has `xc, yc` = 781 and one `(yc, xc)` field next to 42 `(time)` series). View kinds follow each variable's dimensions, not file names or the file's dimension list.
- Several files have zero records (`time = UNLIMITED (0)`). They must open and say so, not fail.

Variables on different grids can still be combined *for display*, because each becomes a `Raster` on the same view pixel grid. That makes cross-grid masking, differences and overlays possible when both grids have geographic coordinates. The result is a nearest-pixel approximation, which is fine for viewing.

### Comparisons

`nqv --diff A B` opens two files, or two folders, as a comparison (implemented). Without `--diff`, any set of files and folders opens as one collection.

- Folders are paired file by file, by name. Files only in the second folder are reported as not opened.
- `Catalog::read` returns A, B or A − B according to `Catalog::show` (`d` cycles them), so maps, series, values and clicked cells work unchanged. Coordinate variables are never differenced.
- A variable missing from B, or with different dimensions there, gives an error in place of the plot. Comparing different grids or record lengths (common time prefix, regridding through the Raster) is left for later.
- A − B opens with a diverging colour map and a symmetric range; A and B with viridis and their own range.

## Navigation

Navigation lives in the core and the protocol, so every front end can change file or folder without going back to the shell.

- `o`: file browser rooted at the current file's folder. Shows each highlighted file's variables and dimensions (no separate `ncdump -h`).
- `O`: open a folder as a collection.
- `[` / `]`: previous / next file in the folder (same variable and slice where it exists).
- Paths are always HPC paths. Remote clients navigate the server's file system, limited to what the user can read.

## Launch

- `nqv file.nc [var]`, `nqv run/` (collection), `nqv f1 f2 run2/ ...` (collection), `nqv --diff a.nc b.nc` or `nqv --diff run1/ run2/` (comparison), `nqv` (browser on cwd). Stitching files along time is not implemented yet.
- `nqv f.nc temp -t -1 -z 100 --region arctic`: jump straight to a view. Every view can print the command line that reproduces it.
- Opener for `.nc` in yazi/lf/ranger; target for fzf.
- **Handoff to the laptop.** If `nqv-local` runs on the laptop and `~/.ssh/config` has `RemoteForward ~/.nqv/sock /path/to/local.sock`, then `nqv file.nc` on the HPC finds the socket and opens the view in the native or web client. The HPC process becomes the server for that view. Without the socket it draws in the terminal. Same command either way. (Works on Albedo and Levante; see Verified.)
- `nqv serve --stdio`: for clients that start the server themselves over `ssh host nqv serve --stdio`.

## Core API (sketch)

```rust
// Catalog and navigation
fn open(target: &Target, cfg: &Config) -> Result<Catalog>      // file, file set, or folder
fn list_dir(path: &Path) -> Result<Vec<DirEntry>>              // with cheap header summaries
impl Catalog {
    fn tree(&self) -> &[Source];                               // sources → groups → variables
    fn grid(&self, var: VarRef) -> &Grid;
}

// Views
struct ViewSpec {
    var: VarRef,
    fixed: Vec<(DimRef, usize)>,      // indices for the non-displayed dims
    region: Region, proj: Proj,
    size: (u32, u32),                 // pixels
    diff: Option<VarRef>,             // d: difference with another variable
}
fn view(cat: &Catalog, spec: &ViewSpec) -> Result<View>        // Raster (map) or Series (line)
fn range(cat: &Catalog, spec: &ViewSpec) -> RangeEstimate      // sampled now, exact later
fn probe(cat: &Catalog, spec: &ViewSpec, px: (u32, u32)) -> Probe  // value, lon/lat, cell index

// nqv-render
fn colorize(w: u32, h: u32, values: &[f32], norm: &Norm, cmap: &Cmap) -> Rgba  // no core types: render has no dependencies and builds to WASM
```

Protocol methods mirror this: `list_dir`, `open`, `close`, `tree`, `view`, `range`, `probe`, `series` (time series at a point), `cancel`. Requests are cancellable, so fast scrolling through time drops stale work. Messages are framed like LSP; Raster payloads are binary.

## Big files

- Read only the current slice; strided reads for zoomed-out views, aligned with the file's chunking.
- Prefetch the next record in the background.
- Colour range from a sample first; exact range computed in the background.
- Bounded memory on login nodes; `--srun` for very large cases.
- Find unstructured meshes automatically (search parent folders), with `.nqv.toml` per experiment as override.

## Screen and keys

```
┌──────────────────────── map ────────────────────────┐
│                                                     │
└─────────────────────────────────────────────────────┘
ice:H_ice  t=124/501  [0, 3500] viridis
←→ time  ↑↓ level  v var  c cmap  r range  / region  o open  ? help
```

`v` variable (fuzzy, across the collection) · `c` colour map · `r` range: auto / symmetric / typed · `/` region and projection · click: value, lon/lat, cell index · space: animate · `s` save PNG + command line · `d` difference with a second variable or file · `o`/`O`/`[`/`]` navigation.

## Wish list (not in v1)

- **Masking by variable.** `mask = X` hides cells where a chosen mask variable equals X; `mask != X` hides cells where it doesn't. Example: `ice:mask_ice != 2` to show only dynamic ice. Needs rules for the mask's own free dimensions (same time index when the time axes match, else a fixed index) and for masks on another grid (via the Raster, see above). Later perhaps simple expressions. To be filed as a GitHub issue.
- Overlays: contours of one variable on another, possibly from another grid.
- Mesh-on-GPU wire format (design 3) for zooming into very large unstructured meshes.
- Zarr and plain HDF5 sources.

## Verified (2026-11-21)

- **Socket forwarding works on Albedo (albedo0) and Levante.** Tested end to end: `ssh -o StreamLocalBindUnlink=yes -R <remote.sock>:<local.sock>` created a `srw-------` socket in the HPC home folder, and a message sent from the HPC arrived on the laptop.
  - Login nodes share home folders but sockets are node-local. A socket path in `$HOME` is visible from a session on another login node but can't be connected to there, so `nqv` must try to connect and fall back to the terminal, not just check the file exists.
  - Several ssh sessions to the same host share one forwarded path. That is fine: they all lead to the same laptop agent.
- **NetCDF reading.** Two options:
  - `netcdf` crate (0.12.1): bindings to netcdf-c. A `static` feature builds netcdf-c, HDF5 and zlib from source (needs cmake and a C++ compiler). Every call goes through a global mutex, because netcdf-c is not thread-safe. Its static build disables filter plugins, so zstd-compressed variables are probably unreadable that way.
  - `netcdf-reader` (0.13.0, pure Rust, with sibling `hdf5-reader`): reads CDF-1/2/5 and NetCDF-4 with chunking, deflate, shuffle, lz4, szip, scale-offset, and does CF unpacking and masking. No zstd. Very young (first release 2026-03, 13 minor versions since), so expect API churn.
  - The CLIMBER-X reference files use chunked storage with deflate level 1 + shuffle, which `netcdf-reader` supports. Albedo's netcdf-c 4.8.1 predates zstd support (4.9.0), so files written with it can't be zstd anyway.
  - Decision proposed: `nqv-core` reads through a `Reader` trait (needed anyway for Zarr later). First implementation: `netcdf-reader`, for a fully static musl binary and parallel reads. Test it on CLIMBER-X, Yelmo and FESOM output before committing to it.

## Open questions

- `netcdf-reader` on large real files: correctness, speed, memory (spike step 1).
- Rules for auto-detecting "stitch along time" versus "collection" for a set of files; `--stitch` / `--collection` as overrides.
- Socket path for the handoff: node-local (`/tmp/nqv-%r.sock`) or in `$HOME` with connect-and-fall-back.

## Build order

1. Spike: `nqv file.nc` with core slice reading, render, and the half-block + kitty tiers with detection. Test from Albedo into Terminal.app and Ghostty. The real `Raster` type from the start.
2. Core proper: catalog, collections, navigation, lazy reads, range estimates, prefetch, mesh index cache.
3. Protocol and `nqv serve --stdio`; socket handoff to `nqv-local`.
4. Native and web clients.
