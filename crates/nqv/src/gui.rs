//! `nqv -g`: the view in the nqv desktop app on the laptop (see `nqv_proto::handoff`),
//! this process serving its data; else in the terminal.

use std::path::PathBuf;

use anyhow::Result;
use nqv_session::command::Command;
use nqv_session::{GridSpec, OpenMode};

/// Show the view in the desktop app, serving its data till the window lets go. False
/// (with the reason printed) when there is no app to show it: the terminal does.
#[cfg(unix)]
pub fn show(paths: &[PathBuf], mode: &OpenMode, commands: Vec<Command>) -> Result<bool> {
    use nqv_proto::handoff::{self, Answer, Handoff};
    use std::io::{BufReader, BufWriter};

    // Missing paths are errors here, as in the terminal, not a reason to fall back.
    let abs = |ps: &[PathBuf]| -> Result<Vec<PathBuf>> {
        ps.iter()
            .map(|p| std::fs::canonicalize(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display())))
            .collect()
    };
    let h = Handoff {
        nqv: env!("CARGO_PKG_VERSION").into(),
        host: handoff::hostname(),
        paths: abs(paths)?,
        mode: OpenMode {
            vs: abs(&mode.vs)?,
            grid: match &mode.grid {
                Some(GridSpec::File(f)) => abs(std::slice::from_ref(f))?.pop().map(GridSpec::File),
                g => g.clone(),
            },
            ..mode.clone()
        },
        commands,
    };
    let (answer, stream) = match handoff::offer(&handoff::sockets(), &h) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("nqv: {e:#}; showing it here");
            return Ok(false);
        }
    };
    if answer == Answer::Done {
        eprintln!("nqv: shown in the nqv desktop app");
        return Ok(true);
    }
    eprintln!(
        "nqv: shown in the nqv desktop app; serving its data till the window closes (Ctrl-C stops)"
    );
    let mut r = BufReader::new(stream.try_clone()?);
    let mut w = BufWriter::new(stream);
    nqv_proto::serve(&mut r, &mut w, env!("CARGO_PKG_VERSION"))?;
    Ok(true)
}

#[cfg(not(unix))]
pub fn show(_: &[PathBuf], _: &OpenMode, _: Vec<Command>) -> Result<bool> {
    eprintln!("nqv: -g needs a Unix socket; showing it here");
    Ok(false)
}
