# nqv: NetCDF Quick View

Quick-look viewer for NetCDF files that runs in the terminal you are already in, local or over ssh. No X11 needed.

Website and downloads: <https://mathesislab.github.io/nqv/>. Shows 2D slices of regular-grid variables as maps, 1D variables (e.g. time series) as line plots. Dimensions of length 1 are ignored, so `(time, lat=1, lon=1)` is a line plot. Design: [docs/architecture.md](docs/architecture.md).

## Install

From the [latest release](https://github.com/mathesislab/nqv/releases/latest):

- terminal viewer, one static file (HPC systems, any Linux on x86-64 or ARM, Macs with Apple silicon):
  ```bash
  curl -fsSL https://mathesislab.github.io/nqv/install.sh | sh
  ```
  It installs the build for the machine (`nqv-linux-x86_64`, `nqv-linux-aarch64`, `nqv-macos-aarch64`) in `~/bin`, or `$NQV_DEST`; `NQV_VERSION=v0.3.0` for a given release; run again to update. The script is [site/install.sh](site/install.sh).
- desktop app: `nqv-desktop-macos-aarch64.dmg`, `nqv-desktop-linux-amd64.AppImage` or `.deb` (it updates itself when asked).

Or build it (below).

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
| `-g PATH...` | any of the above in the desktop app on your laptop, from an ssh session on a cluster (see below); else in the terminal |
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
| `:zoom DIM=A:B [DIM=A:B]`, `off` | `--zoom "lat=50:80 lon=@1:@200"` | part of the map: coordinate values or `@` indices per map dimension; in a projection `x=A:B y=C:D` (km) or `lon=A:B lat=C:D` |
| `:proj SPEC`, `native` | `--proj SPEC` | maps in a projection (see below) |
| `:labels on\|off` | `--labels off` | values written on contour lines (desktop app, PNGs; on by default) |
| `:coast on\|off` | `--coast off` | coastlines over maps whose geography is known (on by default) |
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
| `R` | type a fixed range: the `:` line with `range MIN MAX` of the colours shown, to edit |
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

### Projections

`--proj SPEC` (`:proj` in the viewer, a menu in the desktop app) shows maps in another projection, with meridians and parallels over them (labelled in PNGs):

| SPEC | |
|---|---|
| `latlon` | longitude and latitude, plate carrée |
| `stere north\|south [lon0=L] [lat_ts=T]` | polar stereographic, WGS84; defaults as EPSG:3413 (−45, 70) and 3031 (0, −71) |
| `laea north\|south [lon0=L]`, `laea LON,LAT` | Lambert azimuthal equal-area, WGS84; at a pole as EASE-Grid 2.0 (EPSG:6931/6932) |
| `ortho LON,LAT` | a globe centred there |
| `native` | the grid as stored (the default) |

Where the cells lie comes from the file's CF metadata: a `grid_mapping` (polar stereographic in both variants, Lambert azimuthal equal-area, orthographic, latitude-longitude) with projected x/y in m or km, else 1D latitude and longitude. For files without it, `--grid FILE` takes it from another file with the same grid, and `--grid SPEC` gives the projection of the file's own x/y: CLIMBER-X's NH grids are `--grid "stere north"`. Each pixel goes back through the projections to its cell (nearest or `smooth`), so masks, contours, clicks (now with longitude and latitude) and series work as on the grid; statistics and colour ranges are of the cells shown. `zoom lon=-80:-10 lat=58:85` shows a box; the wheel and a drag zoom and pan in the desktop app. A global latitude-longitude grid can be shown in a polar projection too. Wherever a map's geography is known, in a projection or on its own grid, coastlines are drawn over it (Natural Earth 1:50m, built in; `--coast off` hides them). The projections are written out in nqv (Snyder 1987) and checked against PROJ.

```bash
nqv yelmo.nc -v uxy_s --proj "ortho -40,72" --contour "z_srf step=500" --png greenland.png
nqv ice_NH-16KM.nc --grid "stere north" -v H_ice --proj latlon
```

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

### Defaults

`~/.nqv/defaults` holds settings for every view, in the recipe format, e.g.

```
cmap cividis
labels off
interp bilinear
coast off
```

They are applied whenever files are opened (terminal, `--png`, the desktop app), before `-r` recipes and the command-line options, which win. The colour map and range apply to every view but differences (A − B keeps its diverging colours). Lines that do not fit the files opened are skipped with a note. `:save defaults` writes the current colours and display settings there; `--no-defaults` starts from the built-in settings. `p` and copy command include what the defaults set, so the command line shows the same view without them. With `HOST:PATH` (and in the desktop app) your local defaults apply; `nqv` run on a cluster reads the cluster's.

### Remote files

`nqv albedo0:/path/to/run/ -v H_ice` (scp-style `HOST:PATH`, all paths on one host) starts `nqv serve --stdio` on the host over ssh, with your ssh config, keys and jump hosts. The server reads the data and sends screen-sized maps (16-bit, LZ4-compressed), so the terminal, images and PNGs come from your machine: `--png` writes the file locally. Everything else works as with local files, including the file browser (on the host's file system), `--diff`, `-j`, `--vs` and recipes (from your local `~/.nqv/recipes`, else from the host's). The host needs the same nqv version (`scripts/deploy.sh HOST`); it is found on the PATH or in `~/bin`, or give `--remote-cmd`.

Graphics output is detected at start. Terminals without an image protocol (Terminal.app, GNOME Terminal, anything under tmux) get half-block characters. Set a default with `NQV_GFX`, e.g. `export NQV_GFX=blocks256` for terminals without true colour.

### Desktop app

A window on the same session (Tauri 2 + SvelteKit; macOS and Linux): the map drawn with WebGL at full screen resolution, colour bar and statistics, wheel to zoom around the pointer, drag to pan, click for a cell's value and series, sliders for time and levels, the toolbar for colour map, range, interpolation, colour-blind mode and comparison side, boxes at the colour bar's ends to type a fixed range into (Enter applies; `:range MIN MAX` does the same), and the same `:` commands and keys (`z` whole map, `o` open). Open local files or folders, or paths on a host over ssh (as `nqv HOST:PATH`); `nqv-desktop FILE...` opens local files at start, as does a double click on an `.nc` file. Hosts need ssh to log in without a prompt (a key in ssh-agent): the window has no terminal to ask in. On Linux, a blank window on NVIDIA's driver needs `WEBKIT_DISABLE_DMABUF_RENDERER=1`. `Save PNG…` writes the PNG of `--png`, `copy command` the terminal command line for the view.

```bash
npm install
npm run tauri dev
```

`scripts/linux-docker.sh` builds and tries the Linux app from a Mac (Docker). Releases: [docs/releasing.md](docs/releasing.md).

### From a cluster to the desktop app

`nqv -g run/ice.nc -v H_ice` in an ssh session on a cluster opens the view in the desktop app on your laptop, as `code file` does in VS Code's terminals; the same options as in the terminal apply. The `nqv` on the cluster reads the data and serves it to the window till the window closes (Ctrl-C stops it; `&` gives the prompt back). Each view handed over gets a window of its own. Without the app, or without the forward below, `nqv -g` says why and draws in the terminal. `NQV_GUI=1` makes `-g` the default.

The app listens on `~/.nqv/desktop.sock` while it runs; ssh forwards that socket to the cluster. On your laptop, in `~/.ssh/config`:

```
Host albedo0
  RemoteForward /run/user/60063/nqv.sock %d/.nqv/desktop.sock
```

`60063` is your user id on the cluster (`id -u` there); `nqv -g` prints the line for the host it runs on. The socket goes in `/run/user/UID`, which goes when your last session on the login node ends: sshd leaves forwarded sockets behind and cannot forward to the same path again (the `/tmp` path `nqv -g` also tries, for systems without `/run/user`, keeps that problem). A socket left behind anyway (two sessions on one node, the first closed) is removed by `nqv -g`, which then asks you to log in again. The cluster needs the same nqv version as the app. `nqv -g` on the laptop itself opens the files in the app directly.

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
| `nqv-session` | What a viewer does, for every front end: view state, `:` commands and flags, recipes, file switching, PNG output; reads data through a `Backend` (local files, or a server over ssh) |
| `nqv-proto` | Protocol between a session and `nqv serve --stdio`: framed postcard messages, maps quantised to 16 bits and LZ4-compressed; the server loop and the `Remote` backend over ssh |
| `src-tauri` (`nqv-desktop`), `src/` | Desktop app: Tauri commands around a session (maps as raw f32 bytes), Svelte front end with a WebGL map |
| `nqv` | Terminal app: drawing, keys, mouse; `nqv serve --stdio` |

`cargo run --release -p nqv-core --example probe -- FILE` lists the map variables of a file with view timings.
