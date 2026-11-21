//! Extraction of screen-sized rasters and series from variables on regular grids.

use anyhow::{Result, bail, ensure};

use crate::{Catalog, DimInfo, Kind, VarInfo};

/// A map view: a variable, one index per dimension, and the output size in pixels.
/// Indices at the two map dimensions are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewSpec {
    pub var: String,
    pub index: Vec<usize>,
    pub size: (u32, u32),
}

/// A line view: values along dimension `along`, other dimensions fixed by `index`.
/// Reads with a stride so that at most about `2 * max_points` points are returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesSpec {
    pub var: String,
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

/// Read the map slice in `spec` with a stride matched to the output size, then
/// resample to exactly `spec.size` by nearest neighbour.
pub fn view(cat: &Catalog, spec: &ViewSpec) -> Result<Raster> {
    let var = cat.var(&spec.var)?;
    let Kind::Map { y, x } = var.kind() else {
        bail!("{} is not a map", var.name);
    };
    check_index(var, &spec.index, &[y, x])?;
    let (ydim, xdim) = (&var.dims[y], &var.dims[x]);
    let (ny, nx) = (ydim.len, xdim.len);
    let (w, h) = (spec.size.0.max(1) as usize, spec.size.1.max(1) as usize);

    let sx = (nx / w).max(1);
    let sy = (ny / h).max(1);
    let mx = nx.div_ceil(sx);
    let my = ny.div_ceil(sy);

    let mut start = spec.index.clone();
    let mut count = vec![1; var.dims.len()];
    let mut stride = vec![1; var.dims.len()];
    (start[y], count[y], stride[y]) = (0, my, sy);
    (start[x], count[x], stride[x]) = (0, mx, sx);
    let data = cat.reader.read_slice(&var.name, &start, &count, &stride)?;
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
    let flip = !y_decreasing(cat, ydim);
    let cols: Vec<usize> = (0..w).map(|px| nearest(px, w, mx)).collect();
    let mut values = Vec::with_capacity(w * h);
    for py in 0..h {
        let r = nearest(py, h, my);
        let r = if flip { my - 1 - r } else { r };
        values.extend(cols.iter().map(|&c| at(r, c)));
    }
    Ok(Raster {
        w: w as u32,
        h: h as u32,
        values,
    })
}

/// Colour range from a sample of at most 256 x 256 points of the map slice.
pub fn range(cat: &Catalog, spec: &ViewSpec) -> Result<RangeEstimate> {
    const N: usize = 256;
    let var = cat.var(&spec.var)?;
    let Kind::Map { y, x } = var.kind() else {
        bail!("{} is not a map", var.name);
    };
    let (ny, nx) = (var.dims[y].len, var.dims[x].len);
    let size = (nx.min(N) as u32, ny.min(N) as u32);
    let sample = view(
        cat,
        &ViewSpec {
            size,
            ..spec.clone()
        },
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
    let y = cat.reader.read_slice(&var.name, &start, &count, &stride)?;
    let x = match cat.coord_var(dim) {
        Some(_) => cat.reader.read_slice(&dim.name, &[0], &[n], &[st])?,
        None => (0..n).map(|i| (i * st) as f32).collect(),
    };
    Ok(Series { x, y })
}

/// Single value of a variable at `index`.
pub fn value(cat: &Catalog, var: &str, index: &[usize]) -> Result<f32> {
    let v = cat.var(var)?;
    check_index(v, index, &[])?;
    let one = vec![1; index.len()];
    Ok(cat.reader.read_slice(var, index, &one, &one)?[0])
}

/// Source index for output index `i` of `n` when sampling `m` source points.
fn nearest(i: usize, n: usize, m: usize) -> usize {
    (((i as f64 + 0.5) * m as f64 / n as f64) as usize).min(m - 1)
}

/// Whether the y coordinate decreases (first row is the top). Without a
/// coordinate variable, index 0 is taken as the bottom row, like ncview.
fn y_decreasing(cat: &Catalog, ydim: &DimInfo) -> bool {
    if cat.coord_var(ydim).is_none() || ydim.len < 2 {
        return false;
    }
    match cat.reader.read_slice(&ydim.name, &[0], &[2], &[1]) {
        Ok(c) => c[1] < c[0],
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Reader, VarInfo};
    use std::path::PathBuf;

    /// In-memory reader: variable "f"(t, y, x) with f = 100 t + 10 y + x, and coordinate y.
    struct Mem {
        vars: Vec<VarInfo>,
        nt: usize,
        ny: usize,
        nx: usize,
        y_desc: bool,
    }

    impl Mem {
        fn new(nt: usize, ny: usize, nx: usize, y_desc: bool) -> Self {
            let d = |n: &str, len| DimInfo {
                name: n.into(),
                len,
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
                        out.push((100 * idx(0, t) + 10 * idx(1, y) + idx(2, x)) as f32);
                    }
                }
            }
            Ok(out)
        }
    }

    fn cat(m: Mem) -> Catalog {
        Catalog {
            path: PathBuf::from("mem"),
            reader: Box::new(m),
        }
    }

    #[test]
    fn full_resolution_flips_increasing_y() {
        let c = cat(Mem::new(2, 3, 4, false));
        let r = view(
            &c,
            &ViewSpec {
                var: "f".into(),
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
                var: "f".into(),
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
                var: "f".into(),
                index: vec![0, 0, 0],
                size: (4, 2),
            },
        )
        .unwrap();
        assert_eq!((r.w, r.h), (4, 2));
        assert_eq!(r.values, [0.0, 2.0, 4.0, 6.0, 40.0, 42.0, 44.0, 46.0]);
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
            var: "f".into(),
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
                var: "f".into(),
                index: vec![0, 0, 0],
                size: (4, 3),
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("no records"));
    }
}
