# nqv

Quick-look viewer for NetCDF files that runs in the terminal you are already in, local or over ssh. No X11 needed.

Status: spike. Shows 2D slices of regular-grid variables as maps, 1D variables (e.g. time series) as line plots. Dimensions of length 1 are ignored, so `(time, lat=1, lon=1)` is a line plot. Design: [docs/architecture.md](docs/architecture.md).

## Use

```bash
nqv [PATH...] [options]
```

`nqv -h` lists the options, `nqv --help` adds value syntax and examples. In the viewer, `?` shows all keys and commands, and `p` prints the command line that reproduces the current view (also saved to `~/.cache/nqv/last-command` on quit).

| PATH | Opens |
|---|---|
| `file.nc` | that file |
| `run/` | all NetCDF files in the folder; the variable picker lists `file:var` |
| `a.nc b.nc run2/ ...` | all of them together, as with one folder |
| `--diff a.nc b.nc` | comparison: a − b, or a or b |
| `--diff run1/ run2/` | comparison of the files with the same name in both folders |
| `-j a.nc b.nc ...` | the files joined along time (the record dimension) into one, ordered by their first time value |
| `-j seg1/ seg2/ ...` | the files of the same name across the folders joined, e.g. restart segments |
| `A... --vs B...` | comparison of two sets of paths, each opened as without `--vs`; with `-j` both sides are joined: `nqv -j run1/seg* --vs run2/seg*` |
| (none) | file browser |

Everything the viewer shows can be set from the command line; each `:` command in the viewer has a flag:

| Viewer | Flag | Sets |
|---|---|---|
| `:v NAME` | `-v NAME` | variable (fuzzy like the picker; `file:var` picks the file) |
| `:t VALUE`, `:t @INDEX` | `-t VALUE` | step along the first stepped dimension (usually time) |
| `:at DIM=VALUE` | `--at DIM=VALUE` | any dimension (repeatable); map dimensions pick the clicked cell |
| `:range MIN MAX`, `auto`, `sym` | `--range MIN,MAX` | colour range (y range for line plots) |
| `:cmap NAME` | `--cmap NAME` | colour map: viridis, RdBu_r, grays |
| `:mask COND [and COND]`, `off` | `--mask "COND"` | hide where all conditions hold |
| `:show diff\|a\|b` | `--show diff\|a\|b` | comparison side |
| | `--png FILE [--size WxH]` | write the view to a PNG without the viewer: a map with axes and colour bar, or a line plot |

VALUE is a coordinate value (`1990`, `-21000`) or a 1-based index `@12` (`@-1` is the last).

Keys:

| Key | Action |
|---|---|
| ← → / Home End | first stepped dimension (usually time) |
| ↑ ↓ | second stepped dimension (e.g. level) |
| click | value at a map cell, and its series below the map; Esc closes |
| `v` | variable picker: type to filter (fuzzy: `hice` finds `H_ice`; `ice:` limits to files matching `ice`), ↑↓, Enter, Esc |
| `c` / `r` | next colour map / range auto ↔ symmetric (also leaves a fixed range) |
| `d` | comparison: A − B, A, B (A and B share one colour range) |
| `o` / `O` | file browser (Enter open, Backspace up, `c` folder as collection) / current folder as a collection |
| `[` `]` | previous / next file (in the folder or the collection); the variable stays where it exists |
| `:` | command line |
| `?` / `p` | help / command line for this view |
| `q` | quit |

Masks hide values where conditions on other variables hold:

```
:mask mask_ice != 2              hide where mask_ice is not 2
:mask basins = 1,3               hide where basins is 1 or 3 (commas: any of these values)
:mask H_ice < 10 and z_bed > 0   also <= > >=; and: hide where all conditions hold
:mask off
```

The mask variable is looked up by exact name in the current file, then in the other files; `file.nc:var` names the file. It must lie on the same grid; dimensions it shares with the shown variable (such as time) follow the current step. The colour range covers visible values only.

Joined files must have the same variables and dimensions apart from the time length; time steps out of order between files give a warning. In a comparison, variables missing in the second file, or with different dimensions, show an error instead of a plot.

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
