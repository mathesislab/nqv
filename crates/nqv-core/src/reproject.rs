//! Maps in a chosen projection: each pixel of the view is taken back through the
//! target projection to longitude and latitude, then forward through the grid's own
//! projection to a (fractional) cell. The pixel-to-cell table costs two projections per
//! pixel; it is kept per grid, target and size, so stepping through time only reads.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow, bail};

use crate::{Catalog, Ellipsoid, Geo, Kind, Projection, VarRef, ViewSpec};

/// A view in a map projection: the projection and the part of it shown, in its units
/// (metres; degrees for `LatLon`), as [x0, x1, y0, y1] with x0 < x1 and y0 < y1 (y up).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Target {
    pub proj: Projection,
    pub bounds: [f64; 4],
}

impl Target {
    /// Projected point at the centre of pixel (`px`, `py`) of a `w` by `h` view.
    pub fn at(&self, px: f64, py: f64, w: u32, h: u32) -> (f64, f64) {
        let [x0, x1, y0, y1] = self.bounds;
        (
            x0 + (px + 0.5) / w as f64 * (x1 - x0),
            y1 - (py + 0.5) / h as f64 * (y1 - y0),
        )
    }

    /// Pixel (fractional) of projected point (`x`, `y`) in a `w` by `h` view.
    pub fn pixel(&self, x: f64, y: f64, w: u32, h: u32) -> (f64, f64) {
        let [x0, x1, y0, y1] = self.bounds;
        (
            (x - x0) / (x1 - x0) * w as f64,
            (y1 - y) / (y1 - y0) * h as f64,
        )
    }
}

/// Cells of a grid under each pixel of a view: fractional (row, column) in the whole
/// grid, None where the pixel shows no cell; and the window of cells used.
pub(crate) struct Lookup {
    pub w: usize,
    pub h: usize,
    pub at: Vec<Option<(f32, f32)>>,
    /// Rows and columns touched (half-open), for reading; None when nothing shows.
    pub window: Option<(usize, usize, usize, usize)>,
    /// Columns that wrap around (a global longitude axis): their number.
    pub wrap: Option<usize>,
}

/// A grid: its source and map dimensions.
type GridKey = (usize, String, String);
/// A grid's geography as worked out (or why it could not be).
type GeoEntry = Result<Option<Arc<Geo>>, String>;

/// Geographies and lookups already worked out.
#[derive(Default)]
pub(crate) struct Cache {
    geos: Mutex<HashMap<GridKey, GeoEntry>>,
    lookups: Mutex<Vec<(String, Arc<Lookup>)>>,
}

/// Lookups kept (most recent first): a few sizes and overlays of the view shown.
const LOOKUPS: usize = 12;

impl Catalog {
    /// `geo`, kept per source and map dimensions.
    pub(crate) fn geo_cached(&self, r: &VarRef) -> Result<Option<Arc<Geo>>> {
        let v = self.var(r)?;
        let Kind::Map { y, x } = v.kind() else {
            return Ok(None);
        };
        let key = (r.source, v.dims[y].name.clone(), v.dims[x].name.clone());
        if let Some(g) = self.cache.geos.lock().expect("geo cache").get(&key) {
            return g.clone().map_err(|e| anyhow!(e));
        }
        let g = self
            .geo(r)
            .map(|g| g.map(Arc::new))
            .map_err(|e| format!("{e:#}"));
        self.cache
            .geos
            .lock()
            .expect("geo cache")
            .insert(key, g.clone());
        g.map_err(|e| anyhow!(e))
    }
}

/// The grid's geography, or why there is none to project.
fn need_geo(cat: &Catalog, r: &VarRef) -> Result<Arc<Geo>> {
    cat.geo_cached(r)?.ok_or_else(|| {
        anyhow!(
            "{}: where its cells lie is not known (no grid mapping, no latitude and \
             longitude): give a grid file with --grid FILE",
            r.name
        )
    })
}

/// The pixel-to-cell table of `spec` (which has a target).
pub(crate) fn lookup(cat: &Catalog, spec: &ViewSpec) -> Result<Arc<Lookup>> {
    let target = spec.proj.as_ref().expect("a projected view");
    let geo = need_geo(cat, &spec.var)?;
    let v = cat.var(&spec.var)?;
    let Kind::Map { y, x } = v.kind() else {
        bail!("{} is not a map", v.name);
    };
    let (w, h) = (spec.size.0.max(1) as usize, spec.size.1.max(1) as usize);
    let key = format!(
        "{}|{}|{}|{:?}|{w}x{h}",
        spec.var.source, v.dims[y].name, v.dims[x].name, target
    );
    {
        let mut kept = cat.cache.lookups.lock().expect("lookup cache");
        if let Some(k) = kept.iter().position(|(k, _)| *k == key) {
            let hit = kept.remove(k);
            let l = hit.1.clone();
            kept.insert(0, hit);
            return Ok(l);
        }
    }
    let l = Arc::new(build(&geo, target, w, h));
    let mut kept = cat.cache.lookups.lock().expect("lookup cache");
    kept.insert(0, (key, l.clone()));
    kept.truncate(LOOKUPS);
    Ok(l)
}

/// A coordinate axis of a grid: cell centres, uniform or not, monotonic; a longitude
/// axis around the globe wraps.
struct Axis {
    v: Vec<f64>,
    uniform: Option<(f64, f64)>,
    /// Longitudes: values are brought within 180 degrees of this centre first.
    lon_centre: Option<f64>,
    wrap: bool,
}

impl Axis {
    fn new(v: &[f64], lon: bool) -> Self {
        let n = v.len();
        let d = if n > 1 {
            (v[n - 1] - v[0]) / (n - 1) as f64
        } else {
            1.0
        };
        let uniform = (n > 1
            && d != 0.0
            && v.windows(2)
                .all(|p| ((p[1] - p[0]) - d).abs() <= 1e-4 * d.abs()))
        .then_some((v[0], d));
        let wrap = lon && n > 1 && ((n as f64 * d).abs() - 360.0).abs() < 1e-3 * d.abs().max(1e-9);
        Self {
            v: v.to_vec(),
            uniform,
            lon_centre: lon.then(|| (v[0] + v[n - 1]) / 2.0),
            wrap,
        }
    }

    /// Fractional cell index of `value` (cells cover index ± 0.5); None outside.
    fn frac(&self, value: f64) -> Option<f64> {
        let n = self.v.len();
        let value = match self.lon_centre {
            Some(c) => c + (value - c + 180.0).rem_euclid(360.0) - 180.0,
            None => value,
        };
        let f = match self.uniform {
            Some((v0, d)) => (value - v0) / d,
            None => {
                let inc = self.v[n - 1] > self.v[0];
                let i = self
                    .v
                    .partition_point(|&a| if inc { a <= value } else { a >= value });
                let (a, b) = match i {
                    0 => (0, 1),
                    i if i >= n => (n - 2, n - 1),
                    i => (i - 1, i),
                };
                a as f64 + (value - self.v[a]) / (self.v[b] - self.v[a])
            }
        };
        if self.wrap {
            return Some(f.rem_euclid(n as f64));
        }
        (f >= -0.5 && f <= n as f64 - 0.5).then_some(f)
    }
}

/// Work out the table: each pixel through the target's inverse and the grid's forward
/// projection (or neither when they are the same), rows split among threads.
fn build(geo: &Geo, target: &Target, w: usize, h: usize) -> Lookup {
    let lon = geo.proj == Projection::LatLon;
    let (ax, ay) = (Axis::new(&geo.x, lon), Axis::new(&geo.y, false));
    let same = geo.proj == target.proj;
    let cell = |px: usize, py: usize| -> Option<(f32, f32)> {
        let (tx, ty) = target.at(px as f64, py as f64, w as u32, h as u32);
        let (sx, sy) = if same {
            (tx, ty)
        } else {
            let (lon, lat) = target.proj.inverse(tx, ty)?;
            geo.proj.forward(lon, lat)?
        };
        Some((ay.frac(sy)? as f32, ax.frac(sx)? as f32))
    };
    let mut at = vec![None; w * h];
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(16);
    let rows = h.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (k, chunk) in at.chunks_mut(rows * w).enumerate() {
            let cell = &cell;
            s.spawn(move || {
                for (i, out) in chunk.iter_mut().enumerate() {
                    *out = cell(i % w, k * rows + i / w);
                }
            });
        }
    });
    let (ny, nx) = (geo.y.len(), geo.x.len());
    let mut win: Option<(usize, usize, usize, usize)> = None;
    for &(r, c) in at.iter().flatten() {
        // the cell and its neighbours, for bilinear interpolation
        let (r0, r1) = (
            (r.floor().max(0.0)) as usize,
            (r.ceil() as usize + 1).min(ny),
        );
        let (c0, c1) = (
            (c.floor().max(0.0)) as usize,
            (c.ceil() as usize + 1).min(nx),
        );
        win = Some(match win {
            None => (r0, r1, c0, c1),
            Some((a, b, c, d)) => (a.min(r0), b.max(r1), c.min(c0), d.max(c1)),
        });
    }
    let wrap = ax.wrap.then_some(nx);
    // a wrapping axis is read whole, so neighbours across the seam are there
    let window = win.map(|(a, b, c, d)| match wrap {
        Some(n) => (a, b, 0, n),
        None => (a, b, c, d.max(c + 1)),
    });
    Lookup {
        w,
        h,
        at,
        window,
        wrap,
    }
}

/// The part of `proj` to show for map `r` at first: around where its cells project to,
/// within the projection's own extent (a polar projection down to the equator, the
/// visible half of a globe), with a small margin.
pub fn default_bounds(cat: &Catalog, r: &VarRef, proj: &Projection) -> Result<[f64; 4]> {
    let geo = need_geo(cat, r)?;
    const N: usize = 64;
    let pick = |v: &[f64], k: usize| v[k * (v.len() - 1) / N];
    let mut b = [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for j in 0..=N {
        for i in 0..=N {
            let (sx, sy) = (pick(&geo.x, i), pick(&geo.y, j));
            let Some((lon, lat)) = geo.proj.inverse(sx, sy) else {
                continue;
            };
            let Some((mut x, y)) = proj.forward(lon, lat) else {
                continue;
            };
            if *proj == Projection::LatLon {
                x = (x + 180.0).rem_euclid(360.0) - 180.0;
            }
            b = [b[0].min(x), b[1].max(x), b[2].min(y), b[3].max(y)];
        }
    }
    if !b[0].is_finite() {
        bail!("{}: none of it is visible in {}", r.name, proj.text());
    }
    let extent = extent(proj);
    let pad = |lo: f64, hi: f64| {
        let m = 0.02 * (hi - lo).max(1e-9);
        (lo - m, hi + m)
    };
    let (x0, x1) = pad(b[0], b[1]);
    let (y0, y1) = pad(b[2], b[3]);
    Ok([
        x0.max(extent[0]),
        x1.min(extent[1]),
        y0.max(extent[2]),
        y1.min(extent[3]),
    ])
}

/// All of a projection worth showing, as bounds.
fn extent(proj: &Projection) -> [f64; 4] {
    let r = match *proj {
        Projection::LatLon => return [-180.0, 180.0, -90.0, 90.0],
        Projection::Ortho { .. } => Ellipsoid::SPHERE.a,
        // the equator's distance from the pole (or the centre's antipodal disk)
        Projection::PolarStereo { lon0, .. } => {
            let (x, y) = proj.forward(lon0, 0.0).unwrap_or((0.0, 0.0));
            x.hypot(y)
        }
        Projection::Laea {
            lat0, ellipsoid, ..
        } => {
            if lat0.abs() == 90.0 {
                let (x, y) = proj.forward(0.0, 0.0).unwrap_or((0.0, 0.0));
                x.hypot(y)
            } else {
                2.0 * ellipsoid.a
            }
        }
    };
    [-r, r, -r, r]
}
