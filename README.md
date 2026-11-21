# nqv

Quick-look viewer for NetCDF files that runs in the terminal you are already in, local or over ssh. No X11 needed.

Status: spike. Shows 2D slices of regular-grid variables as maps, 1D variables (e.g. time series) as line plots. Dimensions of length 1 are ignored, so `(time, lat=1, lon=1)` is a line plot. Design: [docs/architecture.md](docs/architecture.md).

## Use

```bash
nqv file.nc [var] [--gfx auto|kitty|sixel|iterm|blocks|blocks256]
```

| Key | Action |
|---|---|
| ← → / Home End | first dimension not shown (usually time) |
| ↑ ↓ | second dimension not shown (e.g. level) |
| `v` | pick a variable (type to filter, Enter, Esc) |
| `c` | next colour map |
| `r` | toggle range: auto / symmetric around 0 |
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
