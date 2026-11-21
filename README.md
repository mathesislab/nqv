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
| `HOST:PATH...` | any of the above on another machine over ssh: the data is read there, the view drawn here (see below) |
| (none) | file browser |

Everything the viewer shows can be set from the command line; each `:` command in the viewer has a flag:

| Viewer | Flag | Sets |
|---|---|---|
| `:v NAME` | `-v NAME` | variable (fuzzy like the picker; `file:var` picks the file) |
| `:t VALUE`, `:t @INDEX` | `-t VALUE` | step along the first stepped dimension (usually time) |
| `:at DIM=VALUE` | `--at DIM=VALUE` | any dimension (repeatable); map dimensions pick the clicked cell |
| `:range MIN MAX`, `auto`, `sym`, `log [MIN MAX]` | `--range MIN,MAX`, `log,MIN,MAX` | colour range (y range for line plots); `log`: logarithmic colours or y axis, by default over at most four decades below the maximum |
| `:cmap NAME` | `--cmap NAME` | colour map: viridis, cividis, plasma, inferno, magma, RdBu_r, grays, jet |
| `:interp nearest\|bilinear` | `--interp bilinear` | map resampling: bilinear blends neighbouring cells where the screen is finer than the grid (smooth maps of coarse grids); missing and masked cells stay sharp |
| `:cvd on\|off` | `--cvd` | colour-blind mode (also `NQV_CVD=1`): `c` cycles only maps suited to colour vision deficiencies (all but jet); choosing jet anyway gives a warning |
| `:mask COND [and COND]`, `off` | `--mask "COND"` | hide where all conditions hold |
| `:contour VAR [step=S\|n=N\|levels=A,B,..]`, `off` | `--contour "..."` | contour lines of another variable over the map |
| `:zoom DIM=A:B [DIM=A:B]`, `off` | `--zoom "lat=50:80 lon=@1:@200"` | part of the map: coordinate values or `@` indices per map dimension |
| `:recipe NAME` | `-r NAME` | apply a recipe (see below); `nqv --recipes` lists them |
| `:save NAME` | | save the current settings as a recipe |
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
| `c` / `r` | next colour map / range: auto → symmetric → log → auto (also leaves a fixed range) |
| `i` | interpolation: nearest ↔ bilinear |
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

In PNGs, contour levels are labelled along the lines and the colour bar has round ticks (powers of ten on a log scale). Values outside the colour range take the end colours; the colour bar then shows a triangle at that end (◀ ▶ in the terminal). Line plots are cut at a fixed y range. Contours are drawn over maps: about 10 round levels by default, or `step=500`, `n=5`, `levels=0,1000,2000`. The contoured variable must be on the same grid; contours follow the mask. Lines are black or white, whichever contrasts with the colour underneath; in half-block terminals cells crossed by a line show it as braille dots.

### Recipes

A recipe is a file of viewer commands, one per line (`#` comments), in `~/.nqv/recipes/NAME`:

```
# Ice sheet: surface velocity on the ice, surface elevation contours
v ice_NH-16KM.nc:uxy_s
mask f_ice < 0.5
contour z_srf step=500
range log 1 3000
```

`nqv -r ice run/` applies it after opening the paths; options on the command line are applied after the recipe and override it. Works with `--png`, `-j`, `--vs`. In the viewer, `:recipe ice` applies it and `:save NAME` writes the current settings as a recipe.

The mask variable is looked up by exact name in the current file, then in the other files; `file.nc:var` names the file. It must lie on the same grid; dimensions it shares with the shown variable (such as time) follow the current step. The colour range covers visible values only.

Joined files must have the same variables and dimensions apart from the time length; time steps out of order between files give a warning. In a comparison, variables missing in the second file, or with different dimensions, show an error instead of a plot.

Beside a map (or on a line below it when there is no room) the terminal shows statistics of the slice: min, mean, max, standard deviation and the share of cells with a value, after masking. Mean and standard deviation are weighted (marked *, with what by): by the cell-area variable named in the CF `cell_measures` attribute, else by a variable called `area`, `cell_area`, `areacella` or `areacello` on the same grid in the same file, else by the cosine of latitude on latitude-longitude grids (y coordinate in degrees north, or named lat/latitude); otherwise they are unweighted. Slices over 4 million cells are sampled (marked ~).

### Remote files

`nqv albedo0:/path/to/run/ -v H_ice` (scp-style `HOST:PATH`, all paths on one host) starts `nqv serve --stdio` on the host over ssh, with your ssh config, keys and jump hosts. The server reads the data and sends screen-sized maps (16-bit, LZ4-compressed), so the terminal, images and PNGs come from your machine: `--png` writes the file locally. Everything else works as with local files, including the file browser (on the host's file system), `--diff`, `-j`, `--vs` and recipes (from your local `~/.nqv/recipes`, else from the host's). The host needs the same nqv version (`scripts/deploy.sh HOST`); it is found on the PATH or in `~/bin`, or give `--remote-cmd`.

Graphics output is detected at start. Terminals without an image protocol (Terminal.app, GNOME Terminal, anything under tmux) get half-block characters. Set a default with `NQV_GFX`, e.g. `export NQV_GFX=blocks256` for terminals without true colour.

## Build

On your own machine (for remote files, or local ones): `cargo install --path crates/nqv`, which puts `nqv` in `~/.cargo/bin`.


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
| `nqv-render` | Normalisation, colour maps, contour drawing; no dependencies |
| `nqv-session` | What a viewer does, for every front end: view state, `:` commands and flags, recipes, file switching, PNG output; reads data through a `Backend` (local files now, a server over ssh next) |
| `nqv-proto` | Protocol between a session and `nqv serve --stdio`: framed postcard messages, maps quantised to 16 bits and LZ4-compressed; the server loop and the `Remote` backend over ssh |
| `nqv` | Terminal app: drawing, keys, mouse; `nqv serve --stdio` |

`cargo run --release -p nqv-core --example probe -- FILE` lists the map variables of a file with view timings.
