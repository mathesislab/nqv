//! nqv: quick-look viewer for NetCDF files in any terminal.

mod app;
mod browser;
mod gui;
mod ui;

use std::io::stdout;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, ValueEnum};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::app::App;
use nqv_proto::{Connection, Prompt, Remote};
use nqv_session::command::{self, Command};
use nqv_session::recipe;
use nqv_session::{GridSpec, OpenMode, Origin, Session};

#[derive(Parser)]
#[command(
    version,
    about = "Quick-look viewer for NetCDF files, in any terminal",
    long_about = "Quick-look viewer for NetCDF files, in any terminal. Everything the viewer \
shows can also be set here, and `p` in the viewer prints the command for the current view.",
    after_help = "Run with --help for value syntax and examples.",
    after_long_help = AFTER_LONG_HELP
)]
struct Args {
    /// Files or folders; none opens the file browser
    #[arg(help_heading = "Open", value_name = "PATH")]
    paths: Vec<PathBuf>,
    /// Compare two files or two folders: A - B (d in the viewer cycles A-B, A, B)
    #[arg(short, long, help_heading = "Open")]
    diff: bool,
    /// Compare PATH... (side A) with these paths (side B), opened the same way (-j too)
    #[arg(long, value_name = "PATH", num_args = 1.., help_heading = "Open")]
    vs: Vec<PathBuf>,
    /// Command that starts the server on a HOST:PATH host (default: nqv, else ~/bin/nqv)
    #[arg(long, value_name = "CMD", help_heading = "Open")]
    remote_cmd: Option<String>,
    /// Show in the nqv desktop app on your laptop (through an ssh-forwarded socket), else
    /// here; also with NQV_GUI=1
    #[arg(short, long, help_heading = "Open")]
    gui: bool,
    /// Join files along time; folders: same-named files across them
    #[arg(short, long, help_heading = "Open")]
    join: bool,
    /// Geography of maps whose files lack it: a file with the grid (CF grid_mapping and
    /// x/y, or latitude-longitude; matched by grid size), or the projection of their own
    /// x/y coordinates, as --proj takes it ("stere north")
    #[arg(long, value_name = "FILE|SPEC", help_heading = "Open")]
    grid: Option<String>,
    /// Dimension to join along, instead of the record dimension
    #[arg(long, value_name = "DIM", requires = "join", help_heading = "Open")]
    join_dim: Option<String>,

    /// Variable to show (fuzzy like the picker; FILE:VAR picks the file)
    #[arg(short, long, value_name = "NAME", help_heading = "View")]
    var: Option<String>,
    /// Step along the first stepped dimension, usually time: VALUE or @INDEX
    #[arg(
        short,
        long,
        value_name = "VALUE",
        allow_hyphen_values = true,
        help_heading = "View"
    )]
    time: Option<String>,
    /// Position along any dimension (repeatable): DIM=VALUE or DIM=@INDEX
    ///
    /// Stepped dimensions (time, levels) go to that step. On the two map dimensions it
    /// picks the cell whose value and series are shown, as a click would.
    #[arg(
        long,
        value_name = "DIM=VALUE",
        allow_hyphen_values = true,
        help_heading = "View"
    )]
    at: Vec<String>,
    /// Hide values where all conditions hold, e.g. "mask_ice != 2"
    #[arg(
        long,
        value_name = "COND",
        allow_hyphen_values = true,
        help_heading = "View"
    )]
    mask: Option<String>,
    /// Part of the map: "DIM=A:B [DIM=A:B]" per map dimension, values or @indices
    #[arg(
        long,
        value_name = "RANGES",
        allow_hyphen_values = true,
        help_heading = "View"
    )]
    zoom: Option<String>,
    /// Contour lines of another variable: "VAR [step=S | n=N | levels=A,B,..]"
    #[arg(
        long,
        value_name = "SPEC",
        allow_hyphen_values = true,
        help_heading = "View"
    )]
    contour: Option<String>,
    /// Apply a recipe (commands in ~/.nqv/recipes/NAME, or a file) before the options here
    #[arg(short, long, value_name = "NAME", help_heading = "View")]
    recipe: Option<String>,
    /// List the recipes in ~/.nqv/recipes and exit
    #[arg(long, help_heading = "View")]
    recipes: bool,
    /// In a comparison, show A - B (diff), A or B
    #[arg(long, value_name = "diff|a|b", help_heading = "View")]
    show: Option<String>,

    /// Maps in a projection: latlon, "stere north|south [lon0=L] [lat_ts=T]",
    /// "laea north|south", "laea LON,LAT", "ortho LON,LAT" (native: on their own grid)
    #[arg(
        long,
        value_name = "SPEC",
        allow_hyphen_values = true,
        help_heading = "Display"
    )]
    proj: Option<String>,
    /// Colour range: MIN,MAX, auto or sym (symmetric around 0)
    #[arg(
        long,
        value_name = "RANGE",
        allow_hyphen_values = true,
        help_heading = "Display"
    )]
    range: Option<String>,
    /// Colour map: viridis cividis plasma inferno magma RdBu_r grays jet
    #[arg(long, value_name = "NAME", help_heading = "Display")]
    cmap: Option<String>,
    /// Map resampling: nearest (default) or bilinear (smooth, for coarse grids)
    #[arg(long, value_name = "MODE", help_heading = "Display")]
    interp: Option<String>,
    /// Colour-blind mode: suggest only colour maps suited to colour vision deficiencies
    /// (also with NQV_CVD=1)
    #[arg(long, help_heading = "Display")]
    cvd: bool,
    /// Graphics output; default from $NQV_GFX, else detected
    #[arg(long, value_enum, help_heading = "Display")]
    gfx: Option<Gfx>,

    /// Write the view (map or line plot) to a PNG file instead of opening the viewer
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    png: Option<PathBuf>,
    /// Largest PNG size, WIDTHxHEIGHT; maps keep the data aspect
    #[arg(
        long,
        value_name = "WxH",
        default_value = "1200x800",
        requires = "png",
        help_heading = "Output"
    )]
    size: String,
}

const AFTER_LONG_HELP: &str = "\
Opening:
  nqv FILE                    one file
  nqv FOLDER | PATH...        all NetCDF files in the folders, together with the files given
  nqv --diff A B              A - B for two files, or for two folders file by file (same names)
  nqv -j FILE...              files joined along time, ordered by their first time value
  nqv -j FOLDER...            same-named files across the folders joined (restart segments)
  nqv HOST:PATH...            the same, on another machine over ssh: the data is read there
                              (by nqv serve --stdio), shown here
  nqv A... --vs B...          comparison of two sets of paths, each opened as without --vs;
                              with -j both sides are joined (two segmented runs)
  nqv -g PATH...              in the nqv desktop app on your laptop, if it is reachable
                              (README: Desktop window from the cluster); else here
  nqv                         file browser

Values:
  VALUE                       nearest coordinate value, e.g. 1990, -21000, 70.5
  @INDEX                      1-based index; @-1 is the last
  COND (--mask)               VAR OP VALUES [and VAR OP VALUES ...]
                              OP: = != < <= > >=; = and != take comma lists (basins = 1,3)
                              hides where all conditions hold; VAR on the same grid

Examples:
  nqv run/ice.nc -v H_ice -t @-1
  nqv run/ice.nc -v H_ice --at zeta=@1 --at xc=0 --at yc=0 --cmap grays
  nqv --diff run1/ run2/ -v H_ice --range sym --mask 'mask_ice != 2'
  nqv -j seg1/ seg2/ seg3/ -v V_ice
  nqv -j run1/seg* --vs run2/seg* -v V_ice
  nqv run/ice.nc -v H_ice -t 1990 --png H_ice_1990.png
  nqv run/ice.nc -v uxy_s --mask 'f_ice < 0.5' --contour 'z_srf step=500'
  nqv -r ice run/ --png ice.png

Recipes:
  A recipe is a file of viewer commands (the : syntax), one per line, # for comments,
  in ~/.nqv/recipes/NAME. -r NAME applies it; options given here are applied after it.
  In the viewer, :recipe NAME applies one and :save NAME writes the current settings.
  Example ~/.nqv/recipes/ice:
    # Ice sheet: surface velocity on the ice, surface elevation contours
    v ice:uxy_s
    mask f_ice < 0.5
    contour z_srf step=500
    range 0 1000

In the viewer, ? lists the keys and : commands (the same as these options),
and p prints the command for the current view.";

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Gfx {
    Auto,
    Kitty,
    Sixel,
    Iterm,
    Blocks,
    Blocks256,
}

/// How the map reaches the screen.
pub enum Output {
    /// Image protocol; the picker knows the protocol and the font size in pixels.
    Pixels(Picker),
    /// Half-block characters, two pixels per cell.
    Blocks { truecolor: bool },
}

impl Output {
    pub fn name(&self) -> &'static str {
        match self {
            Output::Pixels(p) => match p.protocol_type() {
                ProtocolType::Kitty => "kitty",
                ProtocolType::Sixel => "sixel",
                ProtocolType::Iterm2 => "iterm",
                ProtocolType::Halfblocks => "blocks",
            },
            Output::Blocks { truecolor: true } => "blocks",
            Output::Blocks { truecolor: false } => "blocks256",
        }
    }
}

fn main() -> Result<()> {
    // `nqv serve --stdio`: answer a remote session on stdin/stdout (started over ssh).
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().is_some_and(|a| a == "serve") {
        if argv.get(1).map(String::as_str) != Some("--stdio") || argv.len() != 2 {
            bail!("usage: nqv serve --stdio");
        }
        let (stdin, stdout) = (std::io::stdin(), std::io::stdout());
        let mut r = std::io::BufReader::new(stdin.lock());
        let mut w = std::io::BufWriter::new(stdout.lock());
        return nqv_proto::serve(&mut r, &mut w, env!("CARGO_PKG_VERSION"));
    }
    let args = Args::parse();
    if args.recipes {
        let list = recipe::list();
        match recipe::dir() {
            Some(d) if list.is_empty() => println!("no recipes in {}", d.display()),
            _ => {}
        }
        for (name, about) in list {
            println!("{name:<16} {about}");
        }
        return Ok(());
    }
    let gfx = match (args.gfx, std::env::var("NQV_GFX")) {
        (Some(g), _) => g,
        (None, Ok(s)) if !s.is_empty() => match Gfx::from_str(&s, true) {
            Ok(g) => g,
            Err(_) => bail!("NQV_GFX={s}: expected auto|kitty|sixel|iterm|blocks|blocks256"),
        },
        _ => Gfx::Auto,
    };
    // Open before taking over the terminal, so errors print plainly.
    let app = match args.paths.is_empty() {
        true => None,
        false => {
            let (host, paths) = split_host(&args.paths)?;
            let (vs_host, vs) = split_host(&args.vs)?;
            if !vs.is_empty() && vs_host != host {
                bail!("--vs paths must be on the same machine as the others");
            }
            let grid = match args.grid.as_deref().map(GridSpec::parse) {
                Some(GridSpec::File(f)) => {
                    let (grid_host, mut g) = split_host(&[f])?;
                    if grid_host != host {
                        bail!("--grid must be on the same machine as the paths");
                    }
                    g.pop().map(GridSpec::File)
                }
                g => g,
            };
            let mode = OpenMode {
                diff: args.diff,
                join: args.join,
                join_dim: args.join_dim.clone(),
                vs,
                grid,
            };
            let gui = args.gui || std::env::var("NQV_GUI").is_ok_and(|v| v == "1");
            if gui && args.png.is_none() {
                if host.is_some() {
                    bail!("-g with HOST:PATH: open it in the desktop app (Open…)");
                }
                if gui::show(&paths, &mode, commands(&args).map_err(anyhow::Error::msg)?)? {
                    return Ok(());
                }
            }
            let session = match &host {
                None => Session::open(&paths, &mode)?,
                Some(h) => {
                    let conn = Connection::ssh(h, args.remote_cmd.as_deref(), Prompt::Terminal)?;
                    let origin = Origin {
                        paths: paths.clone(),
                        mode: mode.clone(),
                        host: host.clone(),
                    };
                    Session::new(Box::new(Remote::open(conn, &paths, &mode)?), origin)?
                }
            };
            let mut app = App::new(session);
            for c in commands(&args).map_err(anyhow::Error::msg)? {
                app.apply(c).map_err(anyhow::Error::msg)?;
            }
            Some(app)
        }
    };
    if let Some(png) = &args.png {
        let Some(app) = app else {
            bail!("--png needs a file to read");
        };
        let size = parse_size(&args.size)?;
        if let Some(m) = &app.message {
            eprintln!("note: {m}");
        }
        println!("{}", app.s.write_png(png, size)?);
        return Ok(());
    }

    let mut terminal = ratatui::init();
    release_mouse_on_panic();
    let result = select_output(gfx).and_then(|out| {
        // After the terminal query in select_output, so mouse reports can't mix into its reply.
        execute!(stdout(), EnableMouseCapture)?;
        let app = match app {
            Some(app) => Some(app),
            None => App::pick(&mut terminal, &std::env::current_dir()?)?.map(|mut a| {
                a.s.cvd |= args.cvd;
                a
            }),
        };
        match app {
            Some(mut app) => app.run(&mut terminal, out),
            None => Ok(()),
        }
    });
    // Harmless when capture was never enabled.
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// The view options as commands, in the order they apply: comparison side first (it
/// sets default colours), then positions, then colours and mask.
fn commands(args: &Args) -> Result<Vec<Command>, String> {
    let mut cmds = vec![];
    if args.cvd {
        cmds.push(Command::Cvd(true));
    }
    if let Some(r) = &args.recipe {
        cmds.push(Command::Recipe(r.clone()));
    }
    if let Some(v) = &args.var {
        cmds.push(Command::Var(v.clone()));
    }
    if let Some(s) = &args.show {
        cmds.push(Command::Show(command::parse_show(s)?));
    }
    if let Some(t) = &args.time {
        cmds.push(Command::Time(command::parse_time(t)?));
    }
    for a in &args.at {
        let (dim, pos) = command::parse_at(a)?;
        cmds.push(Command::At(dim, pos));
    }
    if let Some(r) = &args.range {
        cmds.push(Command::Range(command::parse_range(r)?));
    }
    if let Some(c) = &args.cmap {
        cmds.push(Command::Cmap(c.clone()));
    }
    if let Some(i) = &args.interp {
        cmds.push(Command::Interp(command::parse_interp(i)?));
    }
    if let Some(m) = &args.mask {
        cmds.push(Command::Mask(command::parse_mask(m)?));
    }
    if let Some(c) = &args.contour {
        cmds.push(Command::Contour(command::parse_contour(c)?));
    }
    if let Some(p) = &args.proj {
        cmds.push(Command::Proj(command::parse_proj(p)?));
    }
    if let Some(z) = &args.zoom {
        cmds.push(Command::Zoom(command::parse_zoom(z)?));
    }
    Ok(cmds)
}

/// Paths as given, or all on one ssh host as HOST:PATH (scp style): the host and the
/// paths on it. A path that exists here is never taken as remote.
fn split_host(paths: &[PathBuf]) -> Result<(Option<String>, Vec<PathBuf>)> {
    let remote = |p: &PathBuf| -> Option<(String, PathBuf)> {
        let s = p.to_str()?;
        let (host, path) = s.split_once(':')?;
        let ok = !p.exists() && !host.is_empty() && !host.contains('/') && !path.is_empty();
        ok.then(|| (host.to_string(), PathBuf::from(path)))
    };
    let split: Vec<Option<(String, PathBuf)>> = paths.iter().map(remote).collect();
    if split.iter().all(Option::is_none) {
        return Ok((None, paths.to_vec()));
    }
    let Some(Some((host, _))) = split.first() else {
        bail!("give all paths on one machine: all HOST:PATH, or all local");
    };
    let host = host.clone();
    let mut out = vec![];
    for s in split {
        match s {
            Some((h, p)) if h == host => out.push(p),
            _ => bail!("give all paths on one machine: all {host}:PATH, or all local"),
        }
    }
    Ok((Some(host), out))
}

/// `WIDTHxHEIGHT`, both at least 16.
fn parse_size(s: &str) -> Result<(u32, u32)> {
    let parsed = s
        .split_once(['x', 'X'])
        .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)));
    match parsed {
        Some((w, h)) if w >= 16 && h >= 16 => Ok((w, h)),
        _ => bail!("--size: expected WIDTHxHEIGHT (at least 16x16), got '{s}'"),
    }
}

/// Chain a panic hook that turns mouse capture off before ratatui's hook restores the terminal.
fn release_mouse_on_panic() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        hook(info);
    }));
}

/// Pick the output. Must run after entering the alternate screen and before reading events.
fn select_output(gfx: Gfx) -> Result<Output> {
    let with_protocol = |p: ProtocolType| -> Result<Output> {
        let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        picker.set_protocol_type(p);
        Ok(Output::Pixels(picker))
    };
    match gfx {
        Gfx::Blocks => Ok(Output::Blocks { truecolor: true }),
        Gfx::Blocks256 => Ok(Output::Blocks { truecolor: false }),
        Gfx::Kitty => with_protocol(ProtocolType::Kitty),
        Gfx::Sixel => with_protocol(ProtocolType::Sixel),
        Gfx::Iterm => with_protocol(ProtocolType::Iterm2),
        Gfx::Auto => Ok(match Picker::from_query_stdio() {
            Ok(p) if p.protocol_type() != ProtocolType::Halfblocks => Output::Pixels(p),
            _ => Output::Blocks { truecolor: true },
        }),
    }
}
