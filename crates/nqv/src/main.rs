//! nqv: quick-look viewer for NetCDF files in any terminal.

mod app;
mod browser;
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

#[derive(Parser)]
#[command(version, about = "Quick-look viewer for NetCDF files")]
#[command(
    after_help = "PATH: one file, or any number of files and folders (all NetCDF files in \
the folders), opened together.\n\
With --diff: two files or two folders, compared as first minus second (files in folders \
paired by name).\n\
A last argument that is not an existing path is taken as the variable to show first."
)]
struct Args {
    /// Files or folders, optionally followed by a variable name; none opens the file browser
    paths: Vec<PathBuf>,
    /// Graphics output; default from $NQV_GFX, else detected
    #[arg(long, value_enum)]
    gfx: Option<Gfx>,
    /// Compare two files or two folders: first minus second (d cycles A-B, A, B)
    #[arg(short, long)]
    diff: bool,
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
    let (paths, var) = split_var(args.paths);
    // Open before taking over the terminal, so errors print plainly.
    let app = match paths.is_empty() {
        true => None,
        false => Some(App::new(open(&paths, args.diff)?, var.as_deref())?),
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

fn open(paths: &[PathBuf], diff: bool) -> Result<Catalog> {
    match (diff, paths) {
        (false, _) => Catalog::open(paths),
        (true, [a, b]) => Catalog::compare(a, b),
        (true, _) => bail!("--diff needs two files or two folders"),
    }
}

/// Take a trailing argument that is not an existing path as a variable name.
fn split_var(mut paths: Vec<PathBuf>) -> (Vec<PathBuf>, Option<String>) {
    match paths.last() {
        Some(p) if paths.len() > 1 && !p.exists() => {
            let var = paths.pop().map(|p| p.to_string_lossy().into_owned());
            (paths, var)
        }
        _ => (paths, None),
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
