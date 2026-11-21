//! The client side: a `Backend` whose data comes from `nqv serve --stdio` on another
//! machine, started over ssh (or any pair of streams).

use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use nqv_core::{
    ContourLine, ContourSpec, DirEntry, Mask, RangeEstimate, Raster, Series, SeriesSpec, Show,
    Stats, VarInfo, VarRef, ViewSpec,
};
use nqv_session::{Backend, CatalogInfo, Files, MapParts, OpenMode};

use crate::VERSION;
use crate::frame;
use crate::message::{Envelope, Request, Response};

/// What runs on the remote machine when no command is given: `nqv` from the PATH, else
/// ~/bin/nqv (where scripts/deploy.sh puts it; non-interactive ssh shells often lack
/// ~/bin in their PATH).
pub const DEFAULT_REMOTE_CMD: &str =
    "command -v nqv >/dev/null 2>&1 && exec nqv serve --stdio || exec ~/bin/nqv serve --stdio";

/// A connection to a server: its streams and, over ssh, the ssh process.
pub struct Connection {
    r: Box<dyn Read + Send>,
    w: Box<dyn Write + Send>,
    next: u64,
    child: Option<Child>,
    /// Where the server runs, for messages.
    pub host: String,
    /// The server's nqv version.
    pub server_version: String,
}

impl Connection {
    /// Start `cmd` (default `DEFAULT_REMOTE_CMD`) on `host` with ssh and shake hands.
    /// ssh asks for passwords or keys on the terminal, as usual.
    pub fn ssh(host: &str, cmd: Option<&str>) -> Result<Self> {
        let mut child = Command::new("ssh")
            .arg("-T")
            .arg(host)
            .arg(cmd.unwrap_or(DEFAULT_REMOTE_CMD))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("starting ssh")?;
        let r = BufReader::new(child.stdout.take().expect("piped stdout"));
        let w = BufWriter::new(child.stdin.take().expect("piped stdin"));
        let mut c = Self::over(Box::new(r), Box::new(w), host)?;
        c.child = Some(child);
        Ok(c)
    }

    /// A connection over the given streams; shakes hands.
    pub fn over(r: Box<dyn Read + Send>, w: Box<dyn Write + Send>, host: &str) -> Result<Self> {
        let mut c = Self {
            r,
            w,
            next: 0,
            child: None,
            host: host.to_string(),
            server_version: String::new(),
        };
        let hello = c
            .exchange(Request::Hello { version: VERSION })
            .with_context(|| format!("no nqv server on {host}"))?;
        match hello {
            Response::Hello { version, nqv } if version == VERSION => c.server_version = nqv,
            Response::Hello { version, nqv } => bail!(
                "nqv {nqv} on {host} speaks protocol {version}, this one {VERSION}: \
                 update nqv there (scripts/deploy.sh {host})"
            ),
            r => bail!("unexpected answer from {host}: {r:?}"),
        }
        Ok(c)
    }

    /// Send a request and wait for its response.
    fn exchange(&mut self, body: Request) -> Result<Response> {
        let id = self.next;
        self.next += 1;
        frame::write(&mut self.w, &Envelope { id, body })?;
        let Some(resp) = frame::read::<Envelope<Response>>(&mut self.r)? else {
            bail!("the nqv server on {} closed the connection", self.host);
        };
        if resp.id != id {
            bail!("answer {} to request {id} from {}", resp.id, self.host);
        }
        Ok(resp.body)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Closing stdin ends the server; wait so ssh does not linger.
        if let Some(mut child) = self.child.take() {
            self.w = Box::new(std::io::sink());
            let _ = child.wait();
        }
    }
}

/// Coordinate values by source and dimension (None: no coordinate variable).
type CoordCache = HashMap<(usize, String), Option<Vec<f32>>>;

/// Data from a server, through one connection.
pub struct Remote {
    conn: Mutex<Connection>,
    info: CatalogInfo,
    /// Coordinate values asked for so far.
    coords: Mutex<CoordCache>,
}

impl Remote {
    /// Open `paths` (on the server) as the command line does with `mode`.
    pub fn open(mut conn: Connection, paths: &[PathBuf], mode: &OpenMode) -> Result<Self> {
        let info = opened(conn.exchange(Request::Open {
            paths: paths.to_vec(),
            mode: mode.clone(),
        })?)?;
        Ok(Self {
            conn: Mutex::new(conn),
            info,
            coords: Mutex::new(HashMap::new()),
        })
    }

    fn call(&self, req: Request) -> Result<Response> {
        let mut c = self.conn.lock().expect("connection lock");
        match c.exchange(req)? {
            Response::Error(e) => bail!("{e}"),
            r => Ok(r),
        }
    }
}

fn opened(r: Response) -> Result<CatalogInfo> {
    match r {
        Response::Opened(info) => Ok(info),
        Response::Error(e) => bail!("{e}"),
        r => bail!("unexpected answer: {r:?}"),
    }
}

/// The response's payload, or an error for any other kind of response.
macro_rules! expect {
    ($resp:expr, $pat:pat => $out:expr) => {
        match $resp {
            $pat => Ok($out),
            r => Err(anyhow::anyhow!("unexpected answer: {r:?}")),
        }
    };
}

impl Files for Remote {
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        expect!(self.call(Request::ListDir(path.to_path_buf()))?, Response::Dir(d) => d)
    }

    fn describe(&self, path: &Path) -> Result<Vec<VarInfo>> {
        expect!(self.call(Request::Describe(path.to_path_buf()))?, Response::Vars(v) => v)
    }
}

impl Backend for Remote {
    fn reopen(&mut self, paths: &[PathBuf], mode: &OpenMode) -> Result<()> {
        let req = Request::Open {
            paths: paths.to_vec(),
            mode: mode.clone(),
        };
        self.info = opened(
            self.conn
                .get_mut()
                .expect("connection lock")
                .exchange(req)?,
        )?;
        self.coords.get_mut().expect("coords lock").clear();
        Ok(())
    }

    fn info(&self) -> &CatalogInfo {
        &self.info
    }

    fn set_show(&mut self, show: Show) {
        // On failure the next request reports the broken connection.
        if self.call(Request::SetShow(show)).is_ok() {
            self.info.show = show;
        }
    }

    fn view(&self, spec: &ViewSpec) -> Result<Raster> {
        expect!(self.call(Request::View(spec.clone()))?, Response::Raster(r) => r)?.decode()
    }

    fn range(&self, spec: &ViewSpec) -> Result<RangeEstimate> {
        expect!(self.call(Request::Range(spec.clone()))?, Response::Range(r) => r)
    }

    fn stats(&self, spec: &ViewSpec) -> Result<Stats> {
        expect!(self.call(Request::Stats(spec.clone()))?, Response::Stats(s) => s)
    }

    fn contours(&self, spec: &ViewSpec, c: &ContourSpec) -> Result<Vec<ContourLine>> {
        let req = Request::Contours(spec.clone(), c.clone());
        expect!(self.call(req)?, Response::Contours(c) => c)
    }

    fn cell_at(&self, spec: &ViewSpec, px: (u32, u32)) -> Option<(usize, usize)> {
        let r = self.call(Request::CellAt(spec.clone(), px)).ok()?;
        expect!(r, Response::Cell(c) => c).ok().flatten()
    }

    fn series(&self, spec: &SeriesSpec) -> Result<(Series, Option<(f32, f32)>)> {
        expect!(self.call(Request::Series(spec.clone()))?, Response::Series(s, r) => (s, r))
    }

    fn value(&self, var: &VarRef, index: &[usize], mask: &Mask) -> Result<Option<f32>> {
        let req = Request::Value {
            var: var.clone(),
            index: index.to_vec(),
            mask: mask.clone(),
        };
        expect!(self.call(req)?, Response::Value(v) => v)
    }

    fn coord(&self, source: usize, dim: &str) -> Option<Vec<f32>> {
        let key = (source, dim.to_string());
        if let Some(c) = self.coords.lock().expect("coords lock").get(&key) {
            return c.clone();
        }
        let req = Request::Coord {
            source,
            dim: dim.to_string(),
        };
        let c = expect!(self.call(req).ok()?, Response::Coord(c) => c).ok()?;
        self.coords
            .lock()
            .expect("coords lock")
            .insert(key, c.clone());
        c
    }

    fn check_overlay(
        &self,
        var: &VarRef,
        mask: &Mask,
        contour: Option<&ContourSpec>,
    ) -> Result<()> {
        let req = Request::CheckOverlay {
            var: var.clone(),
            mask: mask.clone(),
            contour: contour.cloned(),
        };
        expect!(self.call(req)?, Response::Done => ())
    }

    fn map_data(
        &self,
        spec: &ViewSpec,
        contour: Option<&ContourSpec>,
        with_stats: bool,
    ) -> Result<MapParts> {
        let req = Request::MapData {
            spec: spec.clone(),
            contour: contour.cloned(),
            with_stats,
        };
        let (raster, range, contours, stats) = expect!(self.call(req)?,
            Response::Map { raster, range, contours, stats } => (raster, range, contours, stats))?;
        Ok(MapParts {
            raster: raster.decode()?,
            range,
            contours,
            stats,
        })
    }
}
