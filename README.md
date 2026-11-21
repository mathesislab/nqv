# nqv

Quick-look viewer for NetCDF files that runs in the terminal you are already in, local or over ssh. No X11 needed.

Status: spike. Shows 2D slices of regular-grid variables as maps, 1D variables (e.g. time series) as line plots. Dimensions of length 1 are ignored, so `(time, lat=1, lon=1)` is a line plot. Design: [docs/architecture.md](docs/architecture.md).

## Use

```bash
nqv [--diff | -j [--join-dim DIM]] [PATH...] [-v VAR] [-t TIME] [--range MIN,MAX] [--gfx auto|kitty|sixel|iterm|blocks|blocks256]
```

`-v` matches like the variable picker; `-t` takes a coordinate value (`1990`, `-21000`) or a 1-based index `@12`, `@-1` (last); `--range` takes `MIN,MAX`, `auto` or `sym`. The same commands work inside the viewer after `:`.

Masks hide values where conditions on other variables hold (`--mask "..."` or `:mask ...`):

```
:mask mask_ice != 2              hide where mask_ice is not 2
:mask basins = 1,3               hide where basins is 1 or 3 (commas: any of these values)
:mask H_ice < 10 and z_bed > 0   also <= > >=; and: hide where all conditions hold
:mask off
```

The mask variable is looked up by exact name in the current file, then in the other files; `file.nc:var` names the file. It must lie on the same grid; dimensions it shares with the shown variable (such as time) follow the current step. The colour range covers visible values only.

Without a path, `nqv` opens a file browser on the current folder.

| PATH | Opens |
|---|---|
| `file.nc` | that file |
| `run/` | all NetCDF files in the folder; the variable picker lists `file:var` |
| `a.nc b.nc run2/ ...` | all of them together, as with one folder |
| `--diff a.nc b.nc` | comparison: a − b, or a or b (`d`) |
| `--diff run1/ run2/` | comparison of the files with the same name in both folders |
| `-j a.nc b.nc ...` | the files joined along time (the record dimension) into one, ordered by their first time value |
| `-j seg1/ seg2/ ...` | the files of the same name across the folders joined, e.g. restart segments |

Joined files must have the same variables and dimensions apart from the time length; time steps out of order between files give a warning. In a comparison, variables missing in the second file, or with different dimensions, show an error instead of a plot.

| Key | Action |
|---|---|
| ← → / Home End | first dimension not shown (usually time) |
| ↑ ↓ | second dimension not shown (e.g. level) |
| `v` | pick a variable: type to filter (fuzzy: `hice` finds `H_ice`; `ice:` limits to files matching `ice`), ↑↓, Enter, Esc |
| `c` | next colour map |
| `r` | toggle range: auto / symmetric around 0 (also leaves a fixed range) |
| `:` | command: `v NAME`, `t VALUE`, `t @INDEX`, `range MIN MAX`, `range auto`, `range sym`, `mask ...`, `q` |
| `d` | comparison: cycle A − B, A, B (A and B share one colour range) |
| `o` | file browser (Enter open, Backspace up, `c` open folder as collection, Esc close) |
| `O` | open the current file's folder as a collection |
| `[` `]` | previous / next file (in the folder, or in the collection); the variable stays where it exists |
| click on a map | value, cell and coordinates in the status line; series along the ← → dimension below the map |
| `Esc` | close the clicked cell and its series; else quit |
| `q` | quit |

Graphics output is detected at start. Terminals without an image protocol (Terminal.app, GNOME Terminal, anything under tmux) get half-block characters. Set a default with `NQV_GFX`, e.g. `export NQV_GFX=blocks256` for terminals without true colour.

## Build

```bash
cargo build --release
```

Static Linux binary for HPC systems, cross-compiled from macOS or Linux (pure Rust, no C toolchain):

```bash
scripts/deploy.sh albedo0 levante
```

This installs `~/bin/nqv` on each host (`NQV_DEST` changes the folder). Add `~/bin` to `PATH` there if needed.

## Layout

| Crate | Role |
|---|---|
| `nqv-core` | `Reader` trait, NetCDF reader (`netcdf-reader`), screen-sized raster views, colour range |
| `nqv-render` | Normalisation, colour maps; no dependencies |
| `nqv` | Terminal app |

`cargo run --release -p nqv-core --example probe -- FILE` lists the map variables of a file with view timings.
