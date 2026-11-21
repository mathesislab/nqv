//! nqv: quick-look viewer for NetCDF files in any terminal.

mod app;
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
struct Args {
    /// NetCDF file
    file: PathBuf,
    /// Variable to show first
    var: Option<String>,
    /// Graphics output; default from $NQV_GFX, else detected
    #[arg(long, value_enum)]
    gfx: Option<Gfx>,
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
    let cat = Catalog::open(&args.file)?;
    let mut app = App::new(cat, args.var.as_deref())?;

    let mut terminal = ratatui::init();
    release_mouse_on_panic();
    let result = select_output(gfx).and_then(|out| {
        // After the terminal query in select_output, so mouse reports can't mix into its reply.
        execute!(stdout(), EnableMouseCapture)?;
        app.run(&mut terminal, out)
    });
    // Harmless when capture was never enabled.
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
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
