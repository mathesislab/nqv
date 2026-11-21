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

use crate::app::App;
use crate::command::Command;

#[derive(Parser)]
#[command(version, about = "Quick-look viewer for NetCDF files")]
#[command(
    after_help = "PATH: one file, or any number of files and folders (all NetCDF files in \
the folders), opened together.\n\
With --diff: two files or two folders, compared as first minus second (files in folders \
paired by name).\n\
With --join: the files joined along time (the record dimension) into one; for folders, \
the files of the same name across them joined, e.g. restart segments."
)]
struct Args {
    /// Files or folders; none opens the file browser
    paths: Vec<PathBuf>,
    /// Variable to show first (matched like the variable picker: fuzzy, file:var)
    #[arg(short, long)]
    var: Option<String>,
    /// Step to show first: a coordinate value (e.g. 1990, -21000) or @index (@1, @-1)
    #[arg(short, long, allow_hyphen_values = true)]
    time: Option<String>,
    /// Colour range: MIN,MAX (or "MIN MAX"), auto or sym
    #[arg(long, allow_hyphen_values = true)]
    range: Option<String>,
    /// Graphics output; default from $NQV_GFX, else detected
    #[arg(long, value_enum)]
    gfx: Option<Gfx>,
    /// Compare two files or two folders: first minus second (d cycles A-B, A, B)
    #[arg(short, long)]
    diff: bool,
    /// Join the files along time (the record dimension); folders: same-named files across them
    #[arg(short, long)]
    join: bool,
    /// Dimension to join along, instead of the record dimension
    #[arg(long, value_name = "DIM", requires = "join")]
    join_dim: Option<String>,
}

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
            let time = args.time.as_deref().map(command::parse_time).transpose();
            let range = args.range.as_deref().map(command::parse_range).transpose();
            let cmds = [
                time.map_err(anyhow::Error::msg)?.map(Command::Time),
                range.map_err(anyhow::Error::msg)?.map(Command::Range),
            ];
            for c in cmds.into_iter().flatten() {
                app.apply(c).map_err(anyhow::Error::msg)?;
            }
            Some(app)
        }
    };

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

fn open(paths: &[PathBuf], args: &Args) -> Result<Catalog> {
    if let Some(p) = paths.iter().find(|p| !p.exists()) {
        bail!("{}: no such file or folder", p.display());
    }
    match (args.diff, args.join, paths) {
        (true, true, _) => bail!("--diff with --join is not supported yet"),
        (true, false, [a, b]) => Catalog::compare(a, b),
        (true, false, _) => bail!("--diff needs two files or two folders"),
        (false, true, _) => Catalog::join(paths, args.join_dim.as_deref()),
        (false, false, _) => Catalog::open(paths),
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
