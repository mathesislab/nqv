//! Handing a view to the desktop app: `nqv -g file.nc` on a cluster opens the view in
//! the nqv window on the laptop.
//!
//! The desktop app listens on `~/.nqv/desktop.sock`; ssh forwards it to the cluster
//! (`RemoteForward /tmp/nqv-%r.sock ~/.nqv/desktop.sock`). `nqv -g` connects there,
//! sends the protocol version and a `Handoff` (paths and view as commands), and reads
//! the `Answer`. On `Serve` the same stream carries the protocol, with `nqv -g` as the
//! server (`serve`) and the window as the client, till the window lets go.

use std::io::{BufReader, BufWriter, ErrorKind};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nqv_session::OpenMode;
use nqv_session::command::Command;
use serde::{Deserialize, Serialize};

use crate::remote::Connection;
use crate::{VERSION, frame};

/// A view to show: what the terminal viewer would have opened, and its options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handoff {
    /// The sender's nqv version, for messages.
    pub nqv: String,
    /// The sender's host name (`hostname -s`).
    pub host: String,
    /// Absolute paths on that host.
    pub paths: Vec<PathBuf>,
    pub mode: OpenMode,
    /// The view options, applied in order after opening.
    pub commands: Vec<Command>,
}

/// The desktop's answer to a handoff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Answer {
    /// Shown; serve the data on this stream.
    Serve,
    /// Shown from the files directly (same machine): nothing more to do.
    Done,
    /// Not shown, and why.
    Refused(String),
}

/// Where `nqv -g` looks for the desktop app: `$NQV_GUI_SOCKET`, else the forwarded
/// `/tmp/nqv-$USER.sock` (node-local, as ssh makes it on a cluster's login node) and
/// the app's own socket (the same machine).
pub fn sockets() -> Vec<PathBuf> {
    if let Some(s) = std::env::var_os("NQV_GUI_SOCKET").filter(|s| !s.is_empty()) {
        return vec![PathBuf::from(s)];
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default();
    let mut v = vec![PathBuf::from(format!("/tmp/nqv-{user}.sock"))];
    v.extend(desktop_socket());
    v
}

/// The socket the desktop app listens on.
pub fn desktop_socket() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".nqv/desktop.sock"))
}

/// This machine's short name.
pub fn hostname() -> String {
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

/// Hand `h` to the desktop app at the first of `sockets` that answers. On `Serve`, the
/// stream is for `serve`. An error says why there is no window (none reachable, or
/// refused).
pub fn offer(sockets: &[PathBuf], h: &Handoff) -> Result<(Answer, UnixStream)> {
    let mut why = vec![];
    for path in sockets {
        match connect(path) {
            Ok(mut s) => {
                frame::write(&mut s, &VERSION)?;
                frame::write(&mut s, h)?;
                let answer = frame::read::<Answer>(&mut s)?
                    .context("the nqv desktop app closed the connection")?;
                if let Answer::Refused(e) = answer {
                    bail!("{e}");
                }
                return Ok((answer, s));
            }
            Err(e) => why.push(format!("{}: {e:#}", path.display())),
        }
    }
    bail!("no nqv desktop app reachable ({})", why.join("; "))
}

/// Connect to a socket of ours. One that refuses is left from an ssh session or an app
/// that is gone: it is removed, so the next ssh login can forward there again.
fn connect(path: &Path) -> Result<UnixStream> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => bail!("none"),
        Err(e) => return Err(e.into()),
    };
    if !meta.file_type().is_socket() {
        bail!("not a socket");
    }
    // Anyone may create a file in /tmp: only a socket of the user's own is trusted with
    // the view (the home folder's owner is the user).
    let home = std::env::var_os("HOME").context("no HOME")?;
    if meta.uid() != std::fs::metadata(home)?.uid() {
        bail!("belongs to another user");
    }
    match UnixStream::connect(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
            let _ = std::fs::remove_file(path);
            bail!("nothing listening (removed)")
        }
        Err(e) => Err(e.into()),
    }
}

/// The desktop side: read a handoff from a new connection. `local` decides whether it
/// is shown from the files directly (`Answer::Done`), else it is served over the stream
/// and the connection to the sender is returned.
pub fn accept(
    mut s: UnixStream,
    local: impl FnOnce(&Handoff) -> bool,
) -> Result<(Handoff, Option<Connection>)> {
    let version = frame::read::<u32>(&mut s)?.context("closed before the handoff")?;
    if version != VERSION {
        let e = format!(
            "nqv desktop {} speaks protocol {VERSION}, this nqv {version}: use the same \
             version on both",
            env!("CARGO_PKG_VERSION")
        );
        frame::write(&mut s, &Answer::Refused(e.clone()))?;
        bail!("{e}");
    }
    let h = frame::read::<Handoff>(&mut s)?.context("closed before the handoff")?;
    if local(&h) {
        frame::write(&mut s, &Answer::Done)?;
        return Ok((h, None));
    }
    frame::write(&mut s, &Answer::Serve)?;
    let r = BufReader::new(s.try_clone()?);
    let conn = Connection::over(Box::new(r), Box::new(BufWriter::new(s)), &h.host)?;
    Ok((h, Some(conn)))
}
