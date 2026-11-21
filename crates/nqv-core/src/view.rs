//! Extraction of screen-sized rasters and series from variables on regular grids.

use anyhow::{Result, bail, ensure};

use crate::{Catalog, DimInfo, Kind, Show, VarInfo, VarRef};

/// A map view: a variable, one index per dimension, and the output size in pixels.
/// Indices at the two map dimensions are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewSpec {
    pub var: VarRef,
    pub index: Vec<usize>,
    pub size: (u32, u32),
}

/// A line view: values along dimension `along`, other dimensions fixed by `index`.
/// Reads with a stride so that at most about `2 * max_points` points are returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesSpec {
    pub var: VarRef,
    pub along: usize,
    pub index: Vec<usize>,
    pub max_points: usize,
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
    let cols: Vec<usize> = (0..s.w).map(|px| s.col(px)).collect();
    let mut values = Vec::with_capacity(s.w * s.h);
    for py in 0..s.h {
        let r = s.row(py);
        values.extend(cols.iter().map(|&c| at(r, c)));
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
    let y = cat.read_as(show, &spec.var, &start, &count, &stride)?;
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
    let v = cat.var(var)?;
    check_index(v, index, &[])?;
    let one = vec![1; index.len()];
    Ok(cat.read(var, index, &one, &one)?[0])
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
    use crate::{Kind, Other, Reader, Show, Source, VarInfo};
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
            }],
            compare: false,
            show: Show::Diff,
            skipped: vec![],
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
        };
        let series_a = series(&c, &s).unwrap();
        assert_eq!(series_range(&c, &s, &series_a), Some((0.0, 105.0)));
        c.show = Show::Diff;
        let r = range(&c, &spec).unwrap();
        assert_eq!((r.min, r.max), (-5.0, -5.0));
        c.show = Show::B;
        assert_eq!(value(&c, &vr("f"), &[1, 2, 3]).unwrap(), 128.0);
    }

    #[test]
    fn compare_errors_on_mismatch() {
        let spec = ViewSpec {
            var: vr("f"),
            index: vec![0, 0, 0],
            size: (4, 3),
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
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("no records"));
    }
}
