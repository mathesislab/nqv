We use ncview still as a quick viewer of netcdf files on the terminal. Recently colleagues from awi tried to improve on the concept and support unstructured grids etc. but the didn’t bundle it into a package. I’m wondering what other solution there might be. Basically on osx relying on x11/xquartz is not robust. Maybe building something fast and native in rust makes sense where it can process things on the fly server side via ssh and just map the right output to your local screen like x11 but without using that protocol. Brainstorm with me. Also scan and see what’s out there already n

You’ve landed on the right architecture. Terminal graphics protocols (kitty, iTerm2, sixel) already do “X11 without X11”: the server renders a frame and pushes it through plain ssh -t to a modern Mac terminal. Some pieces of this already exist, but nobody has put them together into one tool.

What’s out there
ushow / uterm. This is probably the tool your AWI colleagues made: it ships build steps for DKRZ Levante and AWI’s Albedo. It’s almost entirely C. It maps points to a regular grid with KDTree nearest-neighbour lookup and draws in an X11/Xaw GUI. It handles FESOM, ICON and MPAS. The terminal binary uterm doesn’t need X, but it draws only in ASCII, half-block or braille modes. And no releases or packages are published. 
r-mj
psy-view (Python/Qt): decodes CF conventions and supports unstructured grids like ICON or UGRID. 
libraries
ngview (Met Office, Iris + GeoVista): structured grids and UGRID, plus NEMO, LFRic and CICE. 
github
NCVIS (E3SM people, C++/wxWidgets): only needs lat/lon arrays, not a full mesh description. 
energy
ncvue (Python/Tk): ships notarized macOS apps, but supports mostly structured grids. 
pypi
zenodo
xr-tui: a terminal UI built on xarray for NetCDF, Zarr and HDF5. As far as I can tell it’s for browsing the file, not proper map plots. 
pypi
Gridlook (EGU26): a browser-based WebGL viewer for Zarr that renders on the native grid client-side, including FESOM. 
copernicus
rossby (Rust): an in-memory server that turns NetCDF into an HTTP API with image rendering. It’s early, but worth reading the code. 
docs
Brainstorm: three designs

1. Pixels over the terminal (fastest to build). One static Rust binary on the HPC side. You run ssh -t albedo nqv file.nc, it renders RGBA on the server and outputs kitty/iTerm2 graphics. ratatui-image already unifies Sixel, Kitty and iTerm2 rendering. On the Mac, Ghostty, kitty, WezTerm and iTerm2 all work. Bandwidth depends on window size, not mesh size, so even 1000×600 PNG frames get you usable animation. The catch is tmux: kitty is the only protocol that gets around tmux’s reluctance to pass graphics through. 
docs
debian

2. Values over the wire, colormap on the client. A thin native Mac client (Rust with egui/wgpu) launches ssh host nqv --stdio, like VS Code Remote does. The server sends a screen-resolution raster of data values (f16), not colours. Colormap, range and contrast changes are then instant on the client, and only zooming or a new timestep needs a round trip. This feels much snappier than ncview over X ever did.

3. Mesh once, values per step. Send the triangulation once and only node values after that, then render on the GPU the way Gridlook does. This is great for zooming into FESOM at the coast. But 3M+ nodes per step means compression and level-of-detail work. Save it for v3.

Unstructured-grid trick, whatever the design: for each view, build the pixel→node index once with a KD-tree (the kiddo crate), the way ushow does, and cache it by mesh hash. After that, every timestep is just a gather, which takes microseconds.

Rust pieces: the netcdf crate (bindings to the C library; the HDF5 linking is the pain point) or the new pure-Rust netcdf-reader, which covers classic and NetCDF-4 and would make a fully static musl binary possible. Add zarrs, rayon and ratatui. 
docs

Cheaper wins first
Add kitty/iTerm2 output to uterm. It’s C and already understands FESOM and ICON, so that might take a day and validates design 1.
As a stopgap for the XQuartz pain, run ncview or ushow inside Xpra on the server and view it through Xpra’s HTML5 client.

My pick would be design 1 as an MVP, built so the renderer can later send f16 values instead of pixels (design 2) without a rewrite.

Two things would decide a lot: do your people live inside tmux on the clusters, and how big are the meshes you typically look at (CORE2 vs. 1–5 km setups)?

I typically login using terminal via ssh and then run ncview from there. Colleagues do the same on Linux. Grids are getting higher resolution every day so ability to handle large files is critical. Also the interface needs to be simple and intuitive but also let you make some key choices. 

The trick I see is that if I am exploring files or paths I usually do this in the terminal on the hpc. When I find what I want to look at I usually. Want to run a command there. So ok I could copy file path but it should be quick with the hands to get the viewer going.

Since you’re already in an ssh shell on the HPC when you find the file, the tool should run there and take over the terminal you’re in, the way less or htop does. You type nqv file.nc, look at it, press q, and you’re back at your prompt. You never copy a path or switch windows. That makes design 1 (render on the server, send pixels to the terminal) the core of the tool, not just an MVP.

Launching it fast
Same feel as ncview: nqv file.nc, or nqv run/*.nc to stitch the files along time.
Browse mode: nqv or nqv . opens a file list for the current folder. It shows the variables and dimensions of the highlighted file, and possibly a small thumbnail. This covers the “I’m still exploring paths” phase, with no separate ncdump -h step.
Jump straight to a view: nqv f.nc temp -t -1 -z 100 --region arctic. Every view should also be able to print its own command line, so you can rerun it or paste it to a colleague.
Works with file managers: registers as the opener for .nc files in yazi/lf/ranger and as a target for fzf.
Optional native window later: an ssh RemoteForward socket lets the HPC-side command ask a Mac app to open a native window, the way VS Code’s code command works in remote terminals. If the socket isn’t there, it falls back to the terminal. Same command either way.
Big files
Never read more than the screen needs. Read only the current 2D slice, with a stride for the zoomed-out overview, and follow the file’s chunk layout. Load the next timestep in the background.
Estimate the colour range from a sample. ncview’s full min/max scan is exactly what dies on large files. Compute the exact range in the background and refine when it arrives.
Cache the mesh index. Build the pixel→node lookup once per mesh and view, and save it in ~/.cache keyed by mesh file. The first time a 1 km mesh is opened it might take a few seconds; after that it opens instantly.
Find the mesh automatically: search parent folders for the mesh file, with a .nqv.toml per experiment as override.
Mind login nodes: keep memory use bounded, and add a --srun flag for really big cases.
Simple screen, keys for the important choices
┌──────────────── map (kitty/iTerm2 image) ───────────────┐
│                                                         │
└─────────────────────────────────────────────────────────┘
temp  t=124/365 1990-05-04  z=100m  [-2.0,30.0] thermal
←→ time  ↑↓ level  v var  c cmap  r range  / region  ? help
v: pick a variable with fuzzy search.
r: cycle the range between auto, symmetric around 0, and type your own.
/: pick a region (global, Arctic, Antarctic, or draw a box), and set the projection.
Click: shows the value, lon/lat and node index at that point.
Space: animate.
s: save a PNG, alongside a line with the command that reproduces it.
d: compare with a second file (difference). This is the thing ncview users always miss.
Next steps I’d suggest
Make a 200-line spike: Rust + ratatui-image, showing one FESOM slice through kitty graphics over ssh from Albedo to Ghostty or iTerm2 on your Mac. That tests the feel and the speed before anything else.
Then add the mesh index cache and background loading, and measure on your largest mesh.

Want me to write that spike? It’s a small enough project to scaffold here and hand to you as a repo.
