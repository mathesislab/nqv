//! Extraction of screen-sized rasters and series from variables on regular grids.

use anyhow::{Result, bail, ensure};

use crate::mask::{check_mask, mask_index, mask_show};
use crate::{Catalog, DimInfo, Kind, Mask, Show, VarInfo, VarRef};

/// A map view: a variable, one index per dimension, and the output size in pixels.
/// Indices at the two map dimensions are ignored. Values hidden by `mask` are NaN.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewSpec {
    pub var: VarRef,
    pub index: Vec<usize>,
    pub size: (u32, u32),
    pub mask: Mask,
    pub interp: Interp,
}

/// How a map is resampled to the view size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interp {
    /// Each pixel shows one cell.
    #[default]
    Nearest,
    /// Where the view has more pixels than the data, pixels blend the four surrounding
    /// cells; where it has fewer, nearest. Pixels whose nearest cell is missing stay
    /// missing, so mask and coast outlines stay sharp.
    Bilinear,
}

/// A line view: values along dimension `along`, other dimensions fixed by `index`.
/// Reads with a stride so that at most about `2 * max_points` points are returned.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesSpec {
    pub var: VarRef,
    pub along: usize,
    pub index: Vec<usize>,
    pub max_points: usize,
    pub mask: Mask,
}

/// Values on the view's pixel grid, row 0 at the top. NaN marks missing data.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub w: u32,
    pub h: u32,
    pub values: Vec<f32>,
}

/// Values along one dimension, with coordinate values (or indices) as x.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeEstimate {
    pub min: f32,
    pub max: f32,
    /// True when every value of the slice was looked at.
    pub exact: bool,
}

/// Min and max of the finite values, if any.
pub fn finite_range(values: &[f32]) -> Option<(f32, f32)> {
    values
        .iter()
        .filter(|v| v.is_finite())
        .fold(None, |acc, &v| match acc {
            None => Some((v, v)),
            Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
        })
}

/// Largest size with the aspect ratio of an `nx` by `ny` grid that fits in `max_w` by `max_h`.
pub fn fit_size(nx: usize, ny: usize, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f64 / nx as f64).min(max_h as f64 / ny as f64);
    let w = ((nx as f64 * scale).round() as u32).clamp(1, max_w.max(1));
    let h = ((ny as f64 * scale).round() as u32).clamp(1, max_h.max(1));
    (w, h)
}

/// Check `index` against the dimensions of `var`, except those in `free`.
fn check_index(var: &VarInfo, index: &[usize], free: &[usize]) -> Result<()> {
    ensure!(
        index.len() == var.dims.len(),
        "{}: expected {} indices, got {}",
        var.name,
        var.dims.len(),
        index.len()
    );
    for (k, (d, &i)) in var.dims.iter().zip(index).enumerate() {
        if free.contains(&k) {
            ensure!(d.len > 0, "{}: dimension '{}' is empty", var.name, d.name);
            continue;
        }
        if d.len == 0 {
            bail!("{}: dimension '{}' has no records yet", var.name, d.name);
        }
        ensure!(
            i < d.len,
            "{}: index {i} out of range for '{}' ({})",
            var.name,
            d.name,
            d.len
        );
    }
    Ok(())
}

/// How a map is read with a stride and resampled to an output size by nearest neighbour.
struct Sampling {
    /// Map dimensions of the variable.
    y: usize,
    x: usize,
    /// Output size in pixels.
    w: usize,
    h: usize,
    /// Read strides and number of points read along y and x.
    sy: usize,
    sx: usize,
    my: usize,
    mx: usize,
    /// Whether output row 0 shows the last row read (y increasing upwards).
    flip: bool,
}

impl Sampling {
    fn new(cat: &Catalog, r: &VarRef, var: &VarInfo, size: (u32, u32)) -> Result<Self> {
        let Kind::Map { y, x } = var.kind() else {
            bail!("{} is not a map", var.name);
        };
        let (ny, nx) = (var.dims[y].len, var.dims[x].len);
        for d in [&var.dims[y], &var.dims[x]] {
            ensure!(d.len > 0, "{}: dimension '{}' is empty", var.name, d.name);
        }
        let (w, h) = (size.0.max(1) as usize, size.1.max(1) as usize);
        let sx = (nx / w).max(1);
        let sy = (ny / h).max(1);
        Ok(Self {
            y,
            x,
            w,
            h,
            sy,
            sx,
            my: ny.div_ceil(sy),
            mx: nx.div_ceil(sx),
            flip: !y_decreasing(cat, r.source, &var.dims[y]),
        })
    }

    /// Row of the strided read shown at output row `py`.
    fn row(&self, py: usize) -> usize {
        let r = nearest(py, self.h, self.my);
        if self.flip { self.my - 1 - r } else { r }
    }

    /// Column of the strided read shown at output column `px`.
    fn col(&self, px: usize) -> usize {
        nearest(px, self.w, self.mx)
    }

    /// Fractional row of the strided read at the centre of output row `py`, for
    /// bilinear interpolation: (lower row, upper row, weight of the upper one). Nearest
    /// when the view does not have more rows than the read.
    fn row_frac(&self, py: usize) -> (usize, usize, f32) {
        if self.h <= self.my {
            let r = self.row(py);
            return (r, r, 0.0);
        }
        let f = frac(py, self.h, self.my);
        let f = if self.flip {
            (self.my - 1) as f32 - f
        } else {
            f
        };
        let r0 = f.floor() as usize;
        (r0, (r0 + 1).min(self.my - 1), f - r0 as f32)
    }

    fn col_frac(&self, px: usize) -> (usize, usize, f32) {
        if self.w <= self.mx {
            let c = self.col(px);
            return (c, c, 0.0);
        }
        let f = frac(px, self.w, self.mx);
        let c0 = f.floor() as usize;
        (c0, (c0 + 1).min(self.mx - 1), f - c0 as f32)
    }
}

/// Source position (cell centres at whole numbers) of output pixel `i` of `n` over `m`
/// cells, clamped to the cells.
fn frac(i: usize, n: usize, m: usize) -> f32 {
    ((i as f32 + 0.5) * m as f32 / n as f32 - 0.5).clamp(0.0, (m - 1) as f32)
}

/// Bilinear blend of the four corners with weights `tx`, `ty` towards the second ones;
/// missing corners are left out, and the result is missing if `nearest` is.
fn blend(v: [f32; 4], tx: f32, ty: f32, nearest: f32) -> f32 {
    if !nearest.is_finite() {
        return f32::NAN;
    }
    let w = [
        (1.0 - tx) * (1.0 - ty),
        tx * (1.0 - ty),
        (1.0 - tx) * ty,
        tx * ty,
    ];
    let (mut sum, mut wsum) = (0.0, 0.0);
    for (x, w) in v.iter().zip(w) {
        if x.is_finite() {
            sum += x * w;
            wsum += w;
        }
    }
    if wsum > 0.0 { sum / wsum } else { nearest }
}

/// Read the map slice in `spec` with a stride matched to the output size, then
/// resample to exactly `spec.size` by nearest neighbour.
pub fn view(cat: &Catalog, spec: &ViewSpec) -> Result<Raster> {
    view_as(cat, spec, cat.show)
}

fn view_as(cat: &Catalog, spec: &ViewSpec, show: Show) -> Result<Raster> {
    let var = cat.var(&spec.var)?;
    let s = Sampling::new(cat, &spec.var, var, spec.size)?;
    let (y, x, my, mx) = (s.y, s.x, s.my, s.mx);
    check_index(var, &spec.index, &[y, x])?;

    let mut start = spec.index.clone();
    let mut count = vec![1; var.dims.len()];
    let mut stride = vec![1; var.dims.len()];
    (start[y], count[y], stride[y]) = (0, my, s.sy);
    (start[x], count[x], stride[x]) = (0, mx, s.sx);
    let data = cat.read_as(show, &spec.var, &start, &count, &stride)?;
    ensure!(
        data.len() == my * mx,
        "{}: read {} values, expected {}",
        var.name,
        data.len(),
        my * mx
    );

    // Row-major order puts the later dimension fastest; transpose if y comes after x.
    let at = |r: usize, c: usize| {
        if y < x {
            data[r * mx + c]
        } else {
            data[c * my + r]
        }
    };
    let mut values = Vec::with_capacity(s.w * s.h);
    match spec.interp {
        Interp::Nearest => {
            let cols: Vec<usize> = (0..s.w).map(|px| s.col(px)).collect();
            for py in 0..s.h {
                let r = s.row(py);
                values.extend(cols.iter().map(|&c| at(r, c)));
            }
        }
        Interp::Bilinear => {
            let cols: Vec<(usize, usize, f32)> = (0..s.w).map(|px| s.col_frac(px)).collect();
            for py in 0..s.h {
                let (r0, r1, ty) = s.row_frac(py);
                values.extend(cols.iter().map(|&(c0, c1, tx)| {
                    let v = [at(r0, c0), at(r0, c1), at(r1, c0), at(r1, c1)];
                    let near = v[(tx >= 0.5) as usize + 2 * (ty >= 0.5) as usize];
                    blend(v, tx, ty, near)
                }));
            }
        }
    }
    if !spec.mask.is_empty() {
        check_mask(cat, &spec.var, &spec.mask)?;
        let mut cond_values = vec![];
        for c in &spec.mask.conds {
            let m = cat.var(&c.var)?;
            let mspec = ViewSpec {
                var: c.var.clone(),
                index: mask_index(var, &spec.index, m)?,
                size: spec.size,
                mask: Mask::default(),
                interp: Interp::Nearest,
            };
            cond_values.push(view_as(cat, &mspec, mask_show(show))?.values);
        }
        spec.mask.apply(&mut values, &cond_values);
    }
    Ok(Raster {
        w: s.w as u32,
        h: s.h as u32,
        values,
    })
}

/// Map cell shown at pixel `px` (column, row from the top) of `view(cat, spec)`:
/// indices (y, x) in the variable's own map dimensions. None when `px` lies outside
/// `spec.size` or the variable is not a map.
pub fn cell_at(cat: &Catalog, spec: &ViewSpec, px: (u32, u32)) -> Option<(usize, usize)> {
    let var = cat.var(&spec.var).ok()?;
    let s = Sampling::new(cat, &spec.var, var, spec.size).ok()?;
    let (c, r) = (px.0 as usize, px.1 as usize);
    if c >= s.w || r >= s.h {
        return None;
    }
    Some((s.row(r) * s.sy, s.col(c) * s.sx))
}

/// Statistics of a map slice: finite (visible) values only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub min: f32,
    pub max: f32,
    /// Unweighted mean and standard deviation.
    pub mean: f64,
    pub std: f64,
    /// Cells with a value, and all cells looked at (the rest missing or masked).
    pub valid: usize,
    pub total: usize,
    /// True when every cell of the slice was looked at, not a sample.
    pub exact: bool,
}

/// Slices with more cells than this are sampled for statistics.
const STATS_MAX: usize = 4_000_000;

/// Statistics of the map slice in `spec` (with its mask and the comparison side shown),
/// over every cell, or a regular sample of about 2000 x 2000 for larger slices.
pub fn stats(cat: &Catalog, spec: &ViewSpec) -> Result<Stats> {
    let var = cat.var(&spec.var)?;
    let Kind::Map { y, x } = var.kind() else {
        bail!("{} is not a map", var.name);
    };
    let (ny, nx) = (var.dims[y].len, var.dims[x].len);
    let exact = nx * ny <= STATS_MAX;
    let size = match exact {
        true => (nx as u32, ny as u32),
        false => fit_size(nx, ny, 2000, 2000),
    };
    let r = view(
        cat,
        &ViewSpec {
            size,
            ..spec.clone()
        },
    )?;
    let finite: Vec<f64> = r
        .values
        .iter()
        .filter(|v| v.is_finite())
        .map(|&v| v as f64)
        .collect();
    let n = finite.len();
    let mean = finite.iter().sum::<f64>() / n.max(1) as f64;
    let var_ = finite.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n.max(1) as f64;
    let (min, max) = finite_range(&r.values).unwrap_or((f32::NAN, f32::NAN));
    Ok(Stats {
        min,
        max,
        mean: if n > 0 { mean } else { f64::NAN },
        std: if n > 0 { var_.sqrt() } else { f64::NAN },
        valid: n,
        total: r.values.len(),
        exact,
    })
}

/// Colour range from a sample of at most 256 x 256 points of the map slice. When a
/// comparison shows A or B, the range covers both, so their colours compare directly.
pub fn range(cat: &Catalog, spec: &ViewSpec) -> Result<RangeEstimate> {
    let own = range_as(cat, spec, cat.show)?;
    let other = cat.other_side().and_then(|s| range_as(cat, spec, s).ok());
    Ok(match other {
        Some(o) => RangeEstimate {
            min: own.min.min(o.min),
            max: own.max.max(o.max),
            exact: own.exact && o.exact,
        },
        None => own,
    })
}

fn range_as(cat: &Catalog, spec: &ViewSpec, show: Show) -> Result<RangeEstimate> {
    const N: usize = 256;
    let var = cat.var(&spec.var)?;
    let Kind::Map { y, x } = var.kind() else {
        bail!("{} is not a map", var.name);
    };
    let (ny, nx) = (var.dims[y].len, var.dims[x].len);
    let size = (nx.min(N) as u32, ny.min(N) as u32);
    let sample = view_as(
        cat,
        &ViewSpec {
            size,
            ..spec.clone()
        },
        show,
    )?;
    let (min, max) = finite_range(&sample.values).unwrap_or((f32::NAN, f32::NAN));
    Ok(RangeEstimate {
        min,
        max,
        exact: nx <= N && ny <= N,
    })
}

/// Values along `spec.along`, strided to about `spec.max_points`.
pub fn series(cat: &Catalog, spec: &SeriesSpec) -> Result<Series> {
    series_as(cat, spec, cat.show)
}

/// Range of the finite values of `s`, the series for `spec`; when a comparison shows A
/// or B, widened to cover the other one too.
pub fn series_range(cat: &Catalog, spec: &SeriesSpec, s: &Series) -> Option<(f32, f32)> {
    let own = finite_range(&s.y);
    let other = cat
        .other_side()
        .and_then(|show| series_as(cat, spec, show).ok())
        .and_then(|o| finite_range(&o.y));
    match (own, other) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
        (a, b) => a.or(b),
    }
}

fn series_as(cat: &Catalog, spec: &SeriesSpec, show: Show) -> Result<Series> {
    let var = cat.var(&spec.var)?;
    ensure!(
        spec.along < var.dims.len(),
        "{}: no dimension {}",
        var.name,
        spec.along
    );
    check_index(var, &spec.index, &[spec.along])?;
    let dim = &var.dims[spec.along];
    let st = (dim.len / spec.max_points.max(1)).max(1);
    let n = dim.len.div_ceil(st);

    let mut start = spec.index.clone();
    let mut count = vec![1; var.dims.len()];
    let mut stride = vec![1; var.dims.len()];
    (start[spec.along], count[spec.along], stride[spec.along]) = (0, n, st);
    let mut y = cat.read_as(show, &spec.var, &start, &count, &stride)?;
    if !spec.mask.is_empty() {
        check_mask(cat, &spec.var, &spec.mask)?;
        let mut cond_values = vec![];
        for c in &spec.mask.conds {
            let m = cat.var(&c.var)?;
            let index = mask_index(var, &spec.index, m)?;
            let along = m.dims.iter().position(|d| d.name == dim.name);
            cond_values.push(match along {
                Some(along) => {
                    let mspec = SeriesSpec {
                        var: c.var.clone(),
                        along,
                        index,
                        max_points: spec.max_points,
                        mask: Mask::default(),
                    };
                    series_as(cat, &mspec, mask_show(show))?.y
                }
                // Constant along the series: one value for every point.
                None => vec![value_as(cat, &c.var, &index, mask_show(show))?; n],
            });
        }
        spec.mask.apply(&mut y, &cond_values);
    }
    let x = match cat.coord_var(spec.var.source, dim) {
        Some(_) => {
            let c = VarRef {
                source: spec.var.source,
                name: dim.name.clone(),
            };
            cat.read(&c, &[0], &[n], &[st])?
        }
        None => (0..n).map(|i| (i * st) as f32).collect(),
    };
    Ok(Series { x, y })
}

/// Single value of a variable at `index`.
pub fn value(cat: &Catalog, var: &VarRef, index: &[usize]) -> Result<f32> {
    value_as(cat, var, index, cat.show)
}

fn value_as(cat: &Catalog, var: &VarRef, index: &[usize], show: Show) -> Result<f32> {
    let v = cat.var(var)?;
    check_index(v, index, &[])?;
    let one = vec![1; index.len()];
    Ok(cat.read_as(show, var, index, &one, &one)?[0])
}

/// Whether `mask` hides the value of `var` at `index`.
pub fn masked(cat: &Catalog, var: &VarRef, index: &[usize], mask: &Mask) -> Result<bool> {
    if mask.is_empty() {
        return Ok(false);
    }
    check_mask(cat, var, mask)?;
    let v = cat.var(var)?;
    for c in &mask.conds {
        let m = cat.var(&c.var)?;
        let x = value_as(cat, &c.var, &mask_index(v, index, m)?, mask_show(cat.show))?;
        if !c.holds(x) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Source index for output index `i` of `n` when sampling `m` source points.
fn nearest(i: usize, n: usize, m: usize) -> usize {
    (((i as f64 + 0.5) * m as f64 / n as f64) as usize).min(m - 1)
}

/// Whether the y coordinate decreases (first row is the top). Without a
/// coordinate variable, index 0 is taken as the bottom row, like ncview.
fn y_decreasing(cat: &Catalog, source: usize, ydim: &DimInfo) -> bool {
    if cat.coord_var(source, ydim).is_none() || ydim.len < 2 {
        return false;
    }
    let r = VarRef {
        source,
        name: ydim.name.clone(),
    };
    match cat.read(&r, &[0], &[2], &[1]) {
        Ok(c) => c[1] < c[0],
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cond, Kind, Op, Other, Reader, Show, Source, VarInfo};
    use std::path::PathBuf;

    /// In-memory reader: variable "f"(t, y, x) with f = 100 t + 10 y + x, and coordinate y.
    struct Mem {
        vars: Vec<VarInfo>,
        nt: usize,
        ny: usize,
        nx: usize,
        y_desc: bool,
        /// Added to every value of "f".
        offset: f32,
    }

    impl Mem {
        fn new(nt: usize, ny: usize, nx: usize, y_desc: bool) -> Self {
            let d = |n: &str, len| DimInfo {
                name: n.into(),
                len,
                unlimited: false,
            };
            let v = |n: &str, dims| VarInfo {
                name: n.into(),
                dims,
                long_name: None,
                units: None,
            };
            Self {
                vars: vec![
                    v("f", vec![d("t", nt), d("y", ny), d("x", nx)]),
                    v("y", vec![d("y", ny)]),
                    v("g", vec![d("y", ny), d("x", nx)]),
                ],
                nt,
                ny,
                nx,
                y_desc,
                offset: 0.0,
            }
        }
    }

    impl Reader for Mem {
        fn variables(&self) -> &[VarInfo] {
            &self.vars
        }
        fn read_slice(
            &self,
            var: &str,
            s: &[usize],
            c: &[usize],
            st: &[usize],
        ) -> Result<Vec<f32>> {
            let idx = |k: usize, i: usize| s[k] + i * st[k];
            if var == "y" {
                return Ok((0..c[0])
                    .map(|i| {
                        if self.y_desc {
                            (self.ny - idx(0, i)) as f32
                        } else {
                            idx(0, i) as f32
                        }
                    })
                    .collect());
            }
            if var == "g" {
                // g = 10 y + x
                return Ok((0..c[0])
                    .flat_map(|j| (0..c[1]).map(move |i| (10 * idx(0, j) + idx(1, i)) as f32))
                    .collect());
            }
            assert!(
                idx(0, c[0] - 1) < self.nt
                    && idx(1, c[1] - 1) < self.ny
                    && idx(2, c[2] - 1) < self.nx
            );
            let mut out = vec![];
            for t in 0..c[0] {
                for y in 0..c[1] {
                    for x in 0..c[2] {
                        out.push(
                            (100 * idx(0, t) + 10 * idx(1, y) + idx(2, x)) as f32 + self.offset,
                        );
                    }
                }
            }
            Ok(out)
        }
    }

    fn vr(name: &str) -> VarRef {
        VarRef {
            source: 0,
            name: name.into(),
        }
    }

    fn cat(m: Mem) -> Catalog {
        Catalog {
            sources: vec![Source {
                path: PathBuf::from("mem"),
                reader: Box::new(m),
                other: None,
                joined: 1,
            }],
            compare: false,
            show: Show::Diff,
            skipped: vec![],
            notes: vec![],
        }
    }

    /// Comparison of `a` with `b` (no counterpart when `b` is None).
    fn compare(a: Mem, b: Option<Mem>) -> Catalog {
        let mut c = cat(a);
        c.compare = true;
        c.sources[0].other = b.map(|b| Other {
            path: PathBuf::from("mem-b"),
            reader: Box::new(b),
        });
        c
    }

    #[test]
    fn compare_shows_difference_a_and_b() {
        let mut b = Mem::new(2, 3, 4, false);
        b.offset = 5.0;
        let mut c = compare(Mem::new(2, 3, 4, false), Some(b));
        let spec = ViewSpec {
            var: vr("f"),
            index: vec![1, 0, 0],
            size: (4, 3),
            mask: Mask::default(),
            interp: Interp::Nearest,
        };
        assert!(view(&c, &spec).unwrap().values.iter().all(|&v| v == -5.0));
        // Coordinates are not differenced: increasing y still puts y = 2 on top.
        c.show = Show::A;
        assert_eq!(view(&c, &spec).unwrap().values[0], 120.0);
        c.show = Show::B;
        assert_eq!(view(&c, &spec).unwrap().values[0], 125.0);
        // A and B share their range: A is 100..=123, B 105..=128.
        let r = range(&c, &spec).unwrap();
        assert_eq!((r.min, r.max), (100.0, 128.0));
        c.show = Show::A;
        let s = SeriesSpec {
            var: vr("f"),
            along: 0,
            index: vec![0, 0, 0],
            max_points: 10,
            mask: Mask::default(),
        };
        let series_a = series(&c, &s).unwrap();
        assert_eq!(series_range(&c, &s, &series_a), Some((0.0, 105.0)));
        c.show = Show::Diff;
        let r = range(&c, &spec).unwrap();
        assert_eq!((r.min, r.max), (-5.0, -5.0));
        c.show = Show::B;
        assert_eq!(value(&c, &vr("f"), &[1, 2, 3]).unwrap(), 128.0);
    }

    fn cond(var: &str, op: Op, values: &[f32]) -> Cond {
        Cond {
            var: vr(var),
            op,
            values: values.to_vec(),
        }
    }

    #[test]
    fn mask_hides_where_all_conditions_hold() {
        let c = cat(Mem::new(2, 3, 4, true)); // y decreasing: rows in index order
        let spec = |conds| ViewSpec {
            var: vr("f"),
            index: vec![1, 0, 0],
            size: (4, 3),
            mask: Mask { conds },
            interp: Interp::Nearest,
        };
        // g = 10 y + x: hide g >= 21, i.e. row 2 from x = 1.
        let r = view(&c, &spec(vec![cond("g", Op::Ge, &[21.0])])).unwrap();
        let hidden: Vec<usize> = (0..12).filter(|&i| r.values[i].is_nan()).collect();
        assert_eq!(hidden, [9, 10, 11]);
        // Comma values: hide g in {0, 1}; with a second condition only where both hold.
        let r = view(&c, &spec(vec![cond("g", Op::Eq, &[0.0, 1.0])])).unwrap();
        assert!(r.values[0].is_nan() && r.values[1].is_nan() && !r.values[2].is_nan());
        let both = vec![cond("g", Op::Eq, &[0.0, 1.0]), cond("g", Op::Gt, &[0.0])];
        let r = view(&c, &spec(both)).unwrap();
        assert!(!r.values[0].is_nan() && r.values[1].is_nan());
        // The range covers visible cells only: f = 100 + g, g < 21 visible.
        let rg = range(&c, &spec(vec![cond("g", Op::Ge, &[21.0])])).unwrap();
        assert_eq!((rg.min, rg.max), (100.0, 120.0));
        // != hides everything but the listed values.
        let r = view(&c, &spec(vec![cond("g", Op::Ne, &[23.0])])).unwrap();
        assert_eq!(r.values.iter().filter(|v| !v.is_nan()).count(), 1);
    }

    #[test]
    fn mask_on_series_and_values() {
        let c = cat(Mem::new(3, 3, 4, true));
        let spec = |conds| SeriesSpec {
            var: vr("f"),
            along: 0,
            index: vec![0, 2, 3],
            max_points: 10,
            mask: Mask { conds },
        };
        // g is constant along t: at (2, 3) g = 23, so g > 20 hides the whole series.
        let s = series(&c, &spec(vec![cond("g", Op::Gt, &[20.0])])).unwrap();
        assert!(s.y.iter().all(|v| v.is_nan()));
        let s = series(&c, &spec(vec![cond("g", Op::Lt, &[20.0])])).unwrap();
        assert!(s.y.iter().all(|v| !v.is_nan()));
        let m = Mask {
            conds: vec![cond("g", Op::Eq, &[23.0])],
        };
        assert!(masked(&c, &vr("f"), &[1, 2, 3], &m).unwrap());
        assert!(!masked(&c, &vr("f"), &[1, 2, 2], &m).unwrap());
    }

    #[test]
    fn bilinear_blends_when_upsampling() {
        let c = cat(Mem::new(1, 3, 4, true)); // no flip: rows in index order
        let spec = |interp, conds| ViewSpec {
            var: vr("f"),
            index: vec![0, 0, 0],
            size: (8, 6),
            mask: Mask { conds },
            interp,
        };
        let r = view(&c, &spec(Interp::Bilinear, vec![])).unwrap();
        // Pixel (1, 0): a quarter of the way from column 0 to 1, clamped to row 0.
        assert!((r.values[1] - 0.25).abs() < 1e-5, "{}", r.values[1]);
        // Values rise smoothly along each row (f = 10 y + x).
        assert!(r.values[..8].windows(2).all(|w| w[1] >= w[0]));
        // Same size as the data: identical to nearest.
        let same = |interp| {
            let s = ViewSpec {
                size: (4, 3),
                ..spec(interp, vec![])
            };
            view(&c, &s).unwrap().values
        };
        assert_eq!(same(Interp::Bilinear), same(Interp::Nearest));
        // A pixel whose nearest cell is masked stays masked.
        let masked = spec(Interp::Bilinear, vec![cond("g", Op::Ge, &[21.0])]);
        let n = view(&c, &spec(Interp::Nearest, masked.mask.conds.clone())).unwrap();
        let b = view(&c, &masked).unwrap();
        assert!(
            n.values
                .iter()
                .zip(&b.values)
                .all(|(n, b)| n.is_nan() == b.is_nan())
        );
    }

    #[test]
    fn slice_statistics() {
        let c = cat(Mem::new(2, 3, 4, true));
        let spec = |conds| ViewSpec {
            var: vr("f"),
            index: vec![1, 0, 0],
            size: (2, 2),
            mask: Mask { conds },
            interp: Interp::Nearest,
        };
        // f = 100 + 10 y + x over 3 x 4 cells, whatever the view size.
        let s = stats(&c, &spec(vec![])).unwrap();
        assert_eq!(
            (s.min, s.max, s.valid, s.total, s.exact),
            (100.0, 123.0, 12, 12, true)
        );
        assert!((s.mean - 111.5).abs() < 1e-9);
        // Masked cells do not count: hide g >= 21 (3 cells).
        let s = stats(&c, &spec(vec![cond("g", Op::Ge, &[21.0])])).unwrap();
        assert_eq!((s.valid, s.total, s.max), (9, 12, 120.0));
    }

    #[test]
    fn mask_off_grid_is_an_error() {
        let c = cat(Mem::new(2, 3, 4, true));
        let spec = ViewSpec {
            var: vr("f"),
            index: vec![0, 0, 0],
            size: (4, 3),
            mask: Mask {
                conds: vec![cond("y", Op::Gt, &[0.0])],
            },
            interp: Interp::Nearest,
        };
        let e = view(&c, &spec).unwrap_err().to_string();
        assert!(e.contains("not on the grid"), "{e}");
    }

    #[test]
    fn compare_errors_on_mismatch() {
        let spec = ViewSpec {
            var: vr("f"),
            index: vec![0, 0, 0],
            size: (4, 3),
            mask: Mask::default(),
            interp: Interp::Nearest,
        };
        let c = compare(Mem::new(2, 3, 4, false), Some(Mem::new(3, 3, 4, false)));
        let e = view(&c, &spec).unwrap_err().to_string();
        assert!(e.contains("differs in dimensions"), "{e}");
        let c = compare(Mem::new(2, 3, 4, false), None);
        let e = view(&c, &spec).unwrap_err().to_string();
        assert!(e.contains("no readable counterpart"), "{e}");
    }

    #[test]
    fn full_resolution_flips_increasing_y() {
        let c = cat(Mem::new(2, 3, 4, false));
        let r = view(
            &c,
            &ViewSpec {
                var: vr("f"),
                index: vec![1, 0, 0],
                size: (4, 3),
                mask: Mask::default(),
                interp: Interp::Nearest,
            },
        )
        .unwrap();
        assert_eq!(r.values[..4], [120.0, 121.0, 122.0, 123.0]); // top row is y = 2
        assert_eq!(r.values[8..], [100.0, 101.0, 102.0, 103.0]);
    }

    #[test]
    fn decreasing_y_is_not_flipped() {
        let c = cat(Mem::new(1, 3, 4, true));
        let r = view(
            &c,
            &ViewSpec {
                var: vr("f"),
                index: vec![0, 0, 0],
                size: (4, 3),
                mask: Mask::default(),
                interp: Interp::Nearest,
            },
        )
        .unwrap();
        assert_eq!(r.values[..4], [0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn downsampling_reads_strided() {
        let c = cat(Mem::new(1, 8, 8, true));
        let r = view(
            &c,
            &ViewSpec {
                var: vr("f"),
                index: vec![0, 0, 0],
                size: (4, 2),
                mask: Mask::default(),
                interp: Interp::Nearest,
            },
        )
        .unwrap();
        assert_eq!((r.w, r.h), (4, 2));
        assert_eq!(r.values, [0.0, 2.0, 4.0, 6.0, 40.0, 42.0, 44.0, 46.0]);
    }

    /// Every pixel of the view shows the value of the cell `cell_at` returns for it.
    #[test]
    fn cell_at_inverts_view() {
        // (ny, nx, size): full resolution, downsampled, upsampled, uneven.
        let cases = [
            (3, 4, (4, 3)),
            (8, 8, (4, 2)),
            (3, 4, (9, 7)),
            (7, 9, (4, 3)),
        ];
        for y_desc in [false, true] {
            for (ny, nx, size) in cases {
                let c = cat(Mem::new(2, ny, nx, y_desc));
                let spec = ViewSpec {
                    var: vr("f"),
                    index: vec![1, 0, 0],
                    size,
                    mask: Mask::default(),
                    interp: Interp::Nearest,
                };
                let r = view(&c, &spec).unwrap();
                for py in 0..r.h {
                    for px in 0..r.w {
                        let (cy, cx) = cell_at(&c, &spec, (px, py)).unwrap();
                        assert!(cy < ny && cx < nx);
                        let v = r.values[(py * r.w + px) as usize];
                        assert_eq!(
                            v,
                            (100 + 10 * cy + cx) as f32,
                            "{y_desc} {size:?} {px} {py}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cell_at_corners_and_outside() {
        let spec = |size| ViewSpec {
            var: vr("f"),
            index: vec![0, 0, 0],
            size,
            mask: Mask::default(),
            interp: Interp::Nearest,
        };
        let up = cat(Mem::new(1, 3, 4, false));
        assert_eq!(cell_at(&up, &spec((4, 3)), (0, 0)), Some((2, 0))); // top is last row
        assert_eq!(cell_at(&up, &spec((4, 3)), (3, 2)), Some((0, 3)));
        assert_eq!(cell_at(&up, &spec((4, 3)), (4, 0)), None);
        assert_eq!(cell_at(&up, &spec((4, 3)), (0, 3)), None);
        let down = cat(Mem::new(1, 3, 4, true));
        assert_eq!(cell_at(&down, &spec((4, 3)), (0, 0)), Some((0, 0)));
        let not_map = ViewSpec {
            var: vr("y"),
            index: vec![0],
            size: (4, 3),
            mask: Mask::default(),
            interp: Interp::Nearest,
        };
        assert_eq!(cell_at(&down, &not_map, (0, 0)), None);
    }

    #[test]
    fn kinds_skip_length_one_dims() {
        let k = |nt, ny, nx| Mem::new(nt, ny, nx, false).vars[0].kind();
        assert_eq!(k(5, 3, 4), Kind::Map { y: 1, x: 2 });
        assert_eq!(k(5, 1, 1), Kind::Series { along: 0 });
        assert_eq!(k(5, 1, 4), Kind::Map { y: 0, x: 2 });
        assert_eq!(k(1, 1, 1), Kind::Scalar);
        assert_eq!(k(0, 3, 4), Kind::Map { y: 1, x: 2 });
    }

    #[test]
    fn series_along_time_at_a_point() {
        let c = cat(Mem::new(5, 3, 4, false));
        let spec = SeriesSpec {
            var: vr("f"),
            along: 0,
            index: vec![0, 2, 3],
            max_points: 100,
            mask: Mask::default(),
        };
        let s = series(&c, &spec).unwrap();
        assert_eq!(s.y, [23.0, 123.0, 223.0, 323.0, 423.0]);
        assert_eq!(s.x, [0.0, 1.0, 2.0, 3.0, 4.0]); // no coordinate variable for t
        let s = series(
            &c,
            &SeriesSpec {
                max_points: 2,
                ..spec
            },
        )
        .unwrap();
        assert_eq!(s.y, [23.0, 223.0, 423.0]);
        assert_eq!(s.x, [0.0, 2.0, 4.0]);
    }

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit_size(781, 781, 200, 100), (100, 100));
        assert_eq!(fit_size(2880, 1440, 200, 200), (200, 100));
    }

    #[test]
    fn empty_record_dimension_is_an_error() {
        let c = cat(Mem::new(0, 3, 4, false));
        let e = view(
            &c,
            &ViewSpec {
                var: vr("f"),
                index: vec![0, 0, 0],
                size: (4, 3),
                mask: Mask::default(),
                interp: Interp::Nearest,
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("no records"));
    }
}
