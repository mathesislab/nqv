//! nqv: quick-look viewer for NetCDF files in any terminal.

mod app;
mod browser;
mod command;
mod fuzzy;
mod ui;

use std::io::stdout;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, ValueEnum};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use nqv_core::Catalog;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::app::{App, Origin};
use crate::command::Command;

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
    /// Join files along time; folders: same-named files across them
    #[arg(short, long, help_heading = "Open")]
    join: bool,
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
    /// In a comparison, show A - B (diff), A or B
    #[arg(long, value_name = "diff|a|b", help_heading = "View")]
    show: Option<String>,

    /// Colour range: MIN,MAX, auto or sym (symmetric around 0)
    #[arg(
        long,
        value_name = "RANGE",
        allow_hyphen_values = true,
        help_heading = "Display"
    )]
    range: Option<String>,
    /// Colour map: viridis, RdBu_r, grays
    #[arg(long, value_name = "NAME", help_heading = "Display")]
    cmap: Option<String>,
    /// Graphics output; default from $NQV_GFX, else detected
    #[arg(long, value_enum, help_heading = "Display")]
    gfx: Option<Gfx>,

    /// Write the map to a PNG file instead of opening the viewer
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    png: Option<PathBuf>,
    /// Largest PNG size, WIDTHxHEIGHT; the data aspect is kept
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
  nqv A... --vs B...          comparison of two sets of paths, each opened as without --vs;
                              with -j both sides are joined (two segmented runs)
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
    let args = Args::parse();
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
            let mut app = App::new(open(&args.paths, &args)?, args.var.as_deref())?;
            app.origin = Origin {
                paths: args.paths.clone(),
                diff: args.diff,
                join: args.join,
                join_dim: args.join_dim.clone(),
                vs: args.vs.clone(),
            };
            for c in commands(&args).map_err(anyhow::Error::msg)? {
                app.apply(c).map_err(anyhow::Error::msg)?;
            }
            Some(app)
        }
    };
    if let Some(png) = &args.png {
        let Some(mut app) = app else {
            bail!("--png needs a file to read");
        };
        let size = parse_size(&args.size)?;
        println!("{}", app.write_png(png, size)?);
        return Ok(());
    }

    let mut terminal = ratatui::init();
    release_mouse_on_panic();
    let result = select_output(gfx).and_then(|out| {
        // After the terminal query in select_output, so mouse reports can't mix into its reply.
        execute!(stdout(), EnableMouseCapture)?;
        let app = match app {
            Some(app) => Some(app),
            None => App::pick(&mut terminal, &std::env::current_dir()?)?,
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
    if let Some(m) = &args.mask {
        cmds.push(Command::Mask(command::parse_mask(m)?));
    }
    Ok(cmds)
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

fn open(paths: &[PathBuf], args: &Args) -> Result<Catalog> {
    if let Some(p) = paths.iter().chain(&args.vs).find(|p| !p.exists()) {
        bail!("{}: no such file or folder", p.display());
    }
    let side = |p: &[PathBuf]| match args.join {
        true => Catalog::join(p, args.join_dim.as_deref()),
        false => Catalog::open(p),
    };
    match (args.diff, args.vs.is_empty(), paths) {
        (true, false, _) => bail!("use --diff A B or A... --vs B..., not both"),
        (true, true, _) if args.join => bail!("to compare joined files use -j A... --vs B..."),
        (true, true, [a, b]) => Catalog::compare(a, b),
        (true, true, _) => bail!("--diff needs two files or two folders"),
        (false, false, _) => Catalog::pair(side(paths)?, side(&args.vs)?),
        (false, true, _) => side(paths),
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
