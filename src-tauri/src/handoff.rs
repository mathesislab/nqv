//! Views handed over by `nqv -g` (see `nqv_proto::handoff`): the app listens on
//! `~/.nqv/desktop.sock` (ssh forwards it to the clusters), and shows each view handed
//! to it in a window of its own, its data served by the `nqv -g` that sent it.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use nqv_proto::Remote;
use nqv_proto::handoff::{self, Handoff};
use nqv_session::command::Command;
use nqv_session::{Effect, Origin, Session};
use tauri::AppHandle;

use crate::{Note, present, title};

/// The socket listened on, removed on exit.
static BOUND: OnceLock<PathBuf> = OnceLock::new();

/// Listen for handoffs (unless another nqv app does).
pub fn listen(app: &AppHandle) {
    let Some(path) = handoff::desktop_socket() else {
        return;
    };
    let listener = match bind(&path) {
        Ok(l) => l,
        Err(e) => return eprintln!("nqv: not listening for nqv -g: {e:#}"),
    };
    let _ = BOUND.set(path);
    let app = app.clone();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let app = app.clone();
            std::thread::spawn(move || {
                if let Err(e) = receive(&app, s) {
                    eprintln!("nqv: a view from nqv -g: {e:#}");
                }
            });
        }
    });
}

/// Remove the socket (the app quits).
pub fn stop() {
    if let Some(p) = BOUND.get() {
        let _ = std::fs::remove_file(p);
    }
}

/// The socket, for this user only. One left by an app that is gone is replaced.
fn bind(path: &Path) -> Result<UnixListener> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("another nqv app listens on {}", path.display());
        }
        std::fs::remove_file(path)?;
    }
    let l = UnixListener::bind(path).with_context(|| path.display().to_string())?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// A view handed over: opened from the files when they are on this machine, else
/// through the sender; then its options applied.
fn receive(app: &AppHandle, s: UnixStream) -> Result<()> {
    let me = handoff::hostname();
    let (h, conn) = handoff::accept(s, |h| h.host == me)?;
    let host = conn.as_ref().map(|_| h.host.clone());
    let opened = match conn {
        None => Session::open(&h.paths, &h.mode),
        Some(c) => Remote::open(c, &h.paths, &h.mode).and_then(|r| {
            let origin = Origin {
                paths: h.paths.clone(),
                mode: h.mode.clone(),
                host: host.clone(),
            };
            Session::new(Box::new(r), origin)
        }),
    };
    let Handoff {
        paths, commands, ..
    } = h;
    let (opened, note) = match opened {
        Ok(mut s) => {
            let note = apply(&mut s, commands);
            (Ok(s), note)
        }
        Err(e) => (Err(e), None),
    };
    present(app, &title(host.as_deref(), &paths), opened, note);
    Ok(())
}

/// Apply the view options as the terminal viewer does; the first error, else the last
/// message, is the note.
fn apply(s: &mut Session, commands: Vec<Command>) -> Option<Note> {
    let mut note = None;
    for c in commands {
        match s.apply(c) {
            Ok(Effect::Message(text)) => note = Some(Note { text, error: false }),
            Ok(_) => {}
            Err(text) => return Some(Note { text, error: true }),
        }
    }
    note
}
