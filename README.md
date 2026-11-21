# nqv

Quick-look viewer for NetCDF files that runs in the terminal you are already in, local or over ssh. No X11 needed.

Status: spike. Shows 2D slices of regular-grid variables as maps, 1D variables (e.g. time series) as line plots. Dimensions of length 1 are ignored, so `(time, lat=1, lon=1)` is a line plot. Design: [docs/architecture.md](docs/architecture.md).

## Use

```bash
nqv [--diff] [PATH...] [-v VAR] [--gfx auto|kitty|sixel|iterm|blocks|blocks256]
```

Without a path, `nqv` opens a file browser on the current folder.

| PATH | Opens |
|---|---|
| `file.nc` | that file |
| `run/` | all NetCDF files in the folder; the variable picker lists `file:var` |
| `a.nc b.nc run2/ ...` | all of them together, as with one folder |
| `--diff a.nc b.nc` | comparison: a − b, or a or b (`d`) |
| `--diff run1/ run2/` | comparison of the files with the same name in both folders |

In a comparison, variables missing in the second file, or with different dimensions, show an error instead of a plot.

| Key | Action |
|---|---|
| ← → / Home End | first dimension not shown (usually time) |
| ↑ ↓ | second dimension not shown (e.g. level) |
| `v` | pick a variable: type to filter (fuzzy: `hice` finds `H_ice`; `ice:` limits to files matching `ice`), ↑↓, Enter, Esc |
| `c` | next colour map |
| `r` | toggle range: auto / symmetric around 0 |
| `d` | comparison: cycle A − B, A, B |
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
