//! What the window needs from a session, as plain data: the view state, maps as bytes
//! (values, coloured on the GPU), series, colour tables, and zoom or pan from pixel
//! positions on the map. No Tauri here, so it is tested on its own.

use std::path::PathBuf;

use anyhow::{Result, bail};
use nqv_core::{Kind, Show, ViewSpec, finite_range};
use nqv_proto::{Connection, Prompt, Remote};
use nqv_render::{CMAPS, MISSING, extends};
use nqv_session::command::RangeMode;
use nqv_session::{OpenMode, Origin, Session};
use serde::{Deserialize, Serialize};

/// How to open paths, as the open dialog gives it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenRequest {
    pub paths: Vec<PathBuf>,
    #[serde(default)]
    pub diff: bool,
    #[serde(default)]
    pub join: bool,
    pub join_dim: Option<String>,
    #[serde(default)]
    pub vs: Vec<PathBuf>,
    /// A file giving the geography of maps whose files lack it.
    #[serde(default)]
    pub grid: Option<PathBuf>,
    /// An ssh host the paths are on.
    pub host: Option<String>,
}

pub fn open(req: OpenRequest) -> Result<Session> {
    let mode = OpenMode {
        diff: req.diff,
        join: req.join,
        join_dim: req.join_dim,
        vs: req.vs,
        grid: req.grid.map(nqv_session::GridSpec::File),
    };
    match req.host.filter(|h| !h.is_empty()) {
        None => Session::open(&req.paths, &mode),
        Some(h) => {
            let conn = Connection::ssh(&h, None, Prompt::Batch)?;
            let origin = Origin {
                paths: req.paths.clone(),
                mode: mode.clone(),
                host: Some(h),
            };
            Session::new(Box::new(Remote::open(conn, &req.paths, &mode)?), origin)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DimView {
    pub name: String,
    pub len: usize,
    pub index: usize,
    /// Coordinate value at `index`, and the coordinate's units.
    pub value: Option<f32>,
    pub units: Option<String>,
    /// "y" or "x" (map), "along" (line plot), "step" (stepped), "fixed" (length 1).
    pub role: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeView {
    pub cell: (usize, usize),
    pub coords: (Option<f32>, Option<f32>),
    /// Longitude and latitude, as text, where the grid's geography is known.
    pub lonlat: Option<String>,
    pub value: Option<f32>,
    pub error: Option<String>,
}

/// Everything the window shows besides the data itself.
#[derive(Debug, Clone, Serialize)]
pub struct ViewState {
    pub host: Option<String>,
    pub files: Vec<String>,
    pub compare: bool,
    /// "diff", "a" or "b".
    pub show: &'static str,
    pub source: usize,
    pub file: String,
    pub var: String,
    pub long_name: Option<String>,
    pub units: Option<String>,
    /// "map", "line" or "value".
    pub kind: &'static str,
    pub dims: Vec<DimView>,
    pub cmap: String,
    /// "auto", "sym", "fixed", "log" or "log fixed".
    pub range_mode: &'static str,
    pub interp: &'static str,
    pub cvd: bool,
    pub mask: Option<String>,
    pub mask_error: Option<String>,
    pub contour: Option<String>,
    pub contour_error: Option<String>,
    /// Not the whole map: zoomed in on its grid, or in a projection (where the first
    /// view is the whole map).
    pub zoomed: bool,
    /// The projection maps are shown in (`:proj` syntax); None: on their own grid.
    pub proj: Option<String>,
    /// Longitude and latitude of the map's middle cell, where its geography is known
    /// (so it can be projected; a globe is centred there).
    pub centre: Option<(f64, f64)>,
    pub probe: Option<ProbeView>,
    pub command_line: String,
}

pub fn state(s: &Session) -> ViewState {
    let info = s.info();
    let v = s.var_info();
    let kind = v.kind();
    let steps = v.step_dims();
    let dims = v
        .dims
        .iter()
        .enumerate()
        .map(|(k, d)| DimView {
            name: d.name.clone(),
            len: d.len,
            index: s.index[k],
            value: s.coord_at(k, s.index[k]),
            units: info
                .coord_var(s.var.source, d)
                .and_then(|c| c.units.clone()),
            role: match kind {
                Kind::Map { y, .. } if k == y => "y",
                Kind::Map { x, .. } if k == x => "x",
                Kind::Series { along } if k == along => "along",
                _ if steps.contains(&k) => "step",
                _ => "fixed",
            },
        })
        .collect();
    let o = s.overlays();
    let mask: Vec<String> = s.mask.iter().map(|c| c.text()).collect();
    ViewState {
        host: s.origin.host.clone(),
        files: info.sources.iter().map(|x| x.label.clone()).collect(),
        compare: info.compare,
        show: match info.show {
            Show::Diff => "diff",
            Show::A => "a",
            Show::B => "b",
        },
        source: s.var.source,
        file: info.label(s.var.source).to_string(),
        var: v.name.clone(),
        long_name: v.long_name.clone(),
        units: v.units.clone(),
        kind: match kind {
            Kind::Map { .. } => "map",
            Kind::Series { .. } => "line",
            Kind::Scalar => "value",
        },
        dims,
        cmap: CMAPS[s.cmap].name.to_string(),
        range_mode: match s.range_mode {
            RangeMode::Auto => "auto",
            RangeMode::Symmetric => "sym",
            RangeMode::Fixed(..) => "fixed",
            RangeMode::Log(None) => "log",
            RangeMode::Log(Some(_)) => "log fixed",
        },
        interp: match s.interp {
            nqv_core::Interp::Nearest => "nearest",
            nqv_core::Interp::Bilinear => "bilinear",
        },
        cvd: s.cvd,
        mask: (!mask.is_empty()).then(|| mask.join(" and ")),
        mask_error: o.mask_error,
        contour: s.contour.as_ref().map(|c| c.text()),
        contour_error: o.contour_error,
        zoomed: s.region.is_some() || s.target.is_some(),
        proj: s.target.as_ref().map(|t| t.proj.text()),
        centre: match kind {
            Kind::Map { y, x } => s
                .backend
                .lonlat(&s.var, (v.dims[y].len / 2, v.dims[x].len / 2)),
            _ => None,
        },
        probe: s.probe_info().map(|p| ProbeView {
            cell: p.cell,
            coords: p.coords,
            lonlat: p
                .lonlat
                .map(|(lon, lat)| nqv_session::format::lonlat(lon, lat)),
            value: p.value.as_ref().ok().copied(),
            error: p.value.err(),
        }),
        command_line: s.command_line(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VarItem {
    pub source: usize,
    pub name: String,
    pub file: String,
    pub long_name: Option<String>,
    pub kind: &'static str,
    pub dims: String,
}

/// Variables matching a picker query (all for ""), best first.
pub fn vars(s: &Session, query: &str) -> Vec<VarItem> {
    s.filtered_vars(query)
        .into_iter()
        .map(|(r, v)| VarItem {
            file: s.info().label(r.source).to_string(),
            source: r.source,
            name: v.name.clone(),
            long_name: v.long_name.clone(),
            kind: match v.kind() {
                Kind::Map { .. } => "map",
                Kind::Series { .. } => "line",
                Kind::Scalar => "value",
            },
            dims: v
                .dims
                .iter()
                .map(|d| format!("{}={}", d.name, d.len))
                .collect::<Vec<_>>()
                .join(", "),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct Colormap {
    pub name: String,
    pub cvd_safe: bool,
    /// 256 RGB triples.
    pub lut: Vec<u8>,
}

pub fn colormaps() -> Vec<Colormap> {
    CMAPS
        .iter()
        .map(|c| Colormap {
            name: c.name.to_string(),
            cvd_safe: c.cvd_safe,
            lut: (0..256)
                .flat_map(|i| c.at((i as f32 + 0.5) / 256.0))
                .collect(),
        })
        .collect()
}

/// The header of a map: how to colour and label its values.
#[derive(Debug, Clone, Serialize)]
pub struct MapHeader {
    pub w: u32,
    pub h: u32,
    pub min: f32,
    pub max: f32,
    pub log: bool,
    /// The colour map's index in `colormaps()`, and the colour of missing values.
    pub cmap: usize,
    pub missing: [u8; 3],
    /// Data range of the slice (sampled unless `exact`), and values outside the colour range.
    pub data_min: f32,
    pub data_max: f32,
    pub exact: bool,
    pub extend: (bool, bool),
    /// Colour bar tick labels: positions 0..1 and text.
    pub ticks: Vec<(f32, String)>,
    /// Contour segments in map pixels, x0 y0 x1 y1 each; and their labels.
    pub contours: Vec<f32>,
    /// Meridians and parallels of a projected map, as `contours`.
    pub grid: Vec<f32>,
    pub labels: Vec<(f32, f32, String)>,
    pub stats: Option<StatsView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatsView {
    pub min: f32,
    pub max: f32,
    pub mean: f64,
    pub std: f64,
    pub valid: usize,
    pub total: usize,
    pub exact: bool,
    pub weighted_by: Option<String>,
}

/// The current map at `w` by `h` pixels (fitted to the data's aspect), as bytes: a
/// little-endian u32 header length, the header (JSON), padding to 4 bytes, then the
/// values as little-endian f32 (NaN missing), row 0 at the top. Also returns the view,
/// for clicks, zoom and pan on it.
pub fn map(s: &Session, w: u32, h: u32) -> Result<(Vec<u8>, ViewSpec)> {
    let Some(size) = s.map_size(w.max(16), h.max(16)) else {
        bail!("{} is not a map", s.var_info().name);
    };
    let m = s.map(size, true)?;
    let norm = m.norm;
    let header = MapHeader {
        w: m.raster.w,
        h: m.raster.h,
        min: norm.min,
        max: norm.max,
        log: norm.log,
        cmap: s.cmap,
        missing: MISSING,
        data_min: m.range.min,
        data_max: m.range.max,
        exact: m.range.exact,
        extend: finite_range(&m.raster.values)
            .map_or((false, false), |(lo, hi)| extends(&norm, lo, hi)),
        ticks: nqv_session::png::bar_ticks(&norm),
        contours: m
            .contours
            .iter()
            .flat_map(|l| l.segments.iter().flat_map(|[a, b]| [a.0, a.1, b.0, b.1]))
            .collect(),
        labels: nqv_session::png::contour_labels(&m.contours, m.raster.w, m.raster.h),
        grid: s
            .graticule((m.raster.w, m.raster.h))
            .map(|g| g.meridians.into_iter().chain(g.parallels))
            .into_iter()
            .flatten()
            .flat_map(|l| l.segments)
            .flat_map(|[a, b]| [a.0, a.1, b.0, b.1])
            .collect(),
        stats: m.stats.map(|st| StatsView {
            min: st.min,
            max: st.max,
            mean: st.mean,
            std: st.std,
            valid: st.valid,
            total: st.total,
            exact: st.exact,
            weighted_by: st.weighted_by,
        }),
    };
    let json = serde_json::to_vec(&header)?;
    let mut out = Vec::with_capacity(8 + json.len() + 4 * m.raster.values.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    while out.len() % 4 != 0 {
        out.push(b' ');
    }
    for v in &m.raster.values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    Ok((out, m.spec))
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesView {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub min: f32,
    pub max: f32,
    pub log: bool,
    pub xlabel: String,
    pub ylabel: String,
    /// The current step along x, to mark.
    pub mark: Option<f32>,
}

/// The line plot of a series variable, or the series at the clicked cell of a map.
pub fn series(s: &Session, points: usize) -> Result<Option<SeriesView>> {
    let v = s.var_info();
    let (index, along) = match (v.kind(), s.probe, s.series_dim()) {
        (Kind::Series { along }, _, _) => (s.index.clone(), along),
        (Kind::Map { .. }, Some(cell), Some(along)) => {
            let mut index = s.index_at(cell);
            index[along] = 0;
            (index, along)
        }
        _ => return Ok(None),
    };
    let d = s.series_at(index, along, points.max(2))?;
    let dim = &v.dims[along];
    let units = s
        .info()
        .coord_var(s.var.source, dim)
        .and_then(|c| c.units.clone());
    Ok(Some(SeriesView {
        x: d.series.x,
        y: d.series.y,
        min: d.norm.min,
        max: d.norm.max,
        log: d.norm.log,
        xlabel: match units {
            Some(u) => format!("{} [{u}]", dim.name),
            None => dim.name.clone(),
        },
        ylabel: v.units.clone().unwrap_or_default(),
        mark: match v.kind() {
            Kind::Map { .. } => s
                .coord_at(along, s.index[along])
                .or(Some(s.index[along] as f32)),
            _ => None,
        },
    }))
}

/// The clicked cell under pixel (`px`, `py`) of the map `spec`.
pub fn probe(s: &mut Session, spec: &ViewSpec, px: u32, py: u32) {
    if let Some(cell) = s.cell_at(spec, (px, py)) {
        s.probe = Some(cell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Session {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/nqv-proto/tests/data/small.nc");
        open(OpenRequest {
            paths: vec![path],
            ..OpenRequest::default()
        })
        .unwrap()
    }

    #[test]
    fn state_describes_the_view() {
        let s = small();
        let st = state(&s);
        assert_eq!((st.var.as_str(), st.kind), ("temp", "map"));
        let roles: Vec<&str> = st.dims.iter().map(|d| d.role).collect();
        assert_eq!(roles, ["step", "y", "x"]);
        assert_eq!(st.dims[0].value, Some(0.0));
        assert!(st.command_line.contains("-v temp"));
        assert_eq!(vars(&s, "").len(), 3);
        assert_eq!(colormaps()[0].lut.len(), 768);
    }

    #[test]
    fn map_bytes_hold_header_and_values() {
        let s = small();
        let (bytes, spec) = map(&s, 80, 60).unwrap();
        let n = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&bytes[4..4 + n]).unwrap();
        let (w, h) = (header["w"].as_u64().unwrap(), header["h"].as_u64().unwrap());
        assert_eq!((w as u32, h as u32), spec.size);
        let start = (4 + n).div_ceil(4) * 4;
        assert_eq!(bytes.len() - start, 4 * (w * h) as usize);
        assert!(header["stats"]["valid"].as_u64().unwrap() > 0);
    }

    #[test]
    fn zoom_and_pan_from_pixels() {
        let mut s = small();
        let (_, spec) = map(&s, 80, 60).unwrap();
        s.zoom_at(&spec, 40, 30, 0.5);
        let r = s.region.expect("zoomed in");
        assert_eq!((r.y1 - r.y0, r.x1 - r.x0), (3, 4));
        let (_, spec) = map(&s, 80, 60).unwrap();
        s.pan(&spec, -1000, 0);
        let p = s.region.unwrap();
        assert_eq!((p.x1, p.y0), (8, r.y0)); // dragged left: shows the right edge
        let (_, spec) = map(&s, 80, 60).unwrap();
        s.zoom_at(&spec, 10, 10, 4.0);
        assert!(s.region.is_none(), "zoomed out past the map");
    }

    #[test]
    fn series_at_a_click() {
        let mut s = small();
        assert!(series(&s, 100).unwrap().is_none());
        let (_, spec) = map(&s, 80, 60).unwrap();
        probe(&mut s, &spec, 5, 5);
        let sv = series(&s, 100)
            .unwrap()
            .expect("series at the clicked cell");
        assert_eq!(sv.x, [0.0, 10.0, 20.0]);
        assert_eq!(sv.xlabel, "time [years]");
    }
}
