//! Extraction of screen-sized rasters from variables on regular grids.

use anyhow::{Result, bail, ensure};

use crate::{Catalog, DimInfo};

/// What to show: a variable, indices for its leading dimensions, and the output size in pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewSpec {
    pub var: String,
    /// One index per leading dimension (all but the last two).
    pub fixed: Vec<usize>,
    pub size: (u32, u32),
}

/// Values on the view's pixel grid, row 0 at the top. NaN marks missing data.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub w: u32,
    pub h: u32,
    pub values: Vec<f32>,
}

impl Raster {
    /// Min and max of the finite values, if any.
    pub fn finite_range(&self) -> Option<(f32, f32)> {
        self.values
            .iter()
            .filter(|v| v.is_finite())
            .fold(None, |acc, &v| match acc {
                None => Some((v, v)),
                Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeEstimate {
    pub min: f32,
    pub max: f32,
    /// True when every value of the slice was looked at.
    pub exact: bool,
}

/// Largest size with the aspect ratio of an `nx` by `ny` grid that fits in `max_w` by `max_h`.
pub fn fit_size(nx: usize, ny: usize, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f64 / nx as f64).min(max_h as f64 / ny as f64);
    let w = ((nx as f64 * scale).round() as u32).clamp(1, max_w.max(1));
    let h = ((ny as f64 * scale).round() as u32).clamp(1, max_h.max(1));
    (w, h)
}

/// Read the slice in `spec` with a stride matched to the output size, then
/// resample to exactly `spec.size` by nearest neighbour.
pub fn view(cat: &Catalog, spec: &ViewSpec) -> Result<Raster> {
    let var = cat.var(&spec.var)?;
    ensure!(var.is_map(), "{} has fewer than two dimensions", var.name);
    let leading = var.leading_dims();
    ensure!(
        spec.fixed.len() == leading.len(),
        "{}: expected {} fixed indices, got {}",
        var.name,
        leading.len(),
        spec.fixed.len()
    );
    for (d, &i) in leading.iter().zip(&spec.fixed) {
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
    let ydim = &var.dims[var.dims.len() - 2];
    let xdim = &var.dims[var.dims.len() - 1];
    let (ny, nx) = (ydim.len, xdim.len);
    ensure!(nx > 0 && ny > 0, "{}: empty horizontal grid", var.name);
    let (w, h) = (spec.size.0.max(1) as usize, spec.size.1.max(1) as usize);

    let sx = (nx / w).max(1);
    let sy = (ny / h).max(1);
    let mx = nx.div_ceil(sx);
    let my = ny.div_ceil(sy);

    let mut start = spec.fixed.clone();
    start.extend([0, 0]);
    let mut count = vec![1; leading.len()];
    count.extend([my, mx]);
    let mut stride = vec![1; leading.len()];
    stride.extend([sy, sx]);
    let data = cat.reader.read_slice(&var.name, &start, &count, &stride)?;
    ensure!(
        data.len() == my * mx,
        "{}: read {} values, expected {}",
        var.name,
        data.len(),
        my * mx
    );

    let flip = !y_decreasing(cat, ydim);
    let cols: Vec<usize> = (0..w).map(|px| nearest(px, w, mx)).collect();
    let mut values = Vec::with_capacity(w * h);
    for py in 0..h {
        let r = nearest(py, h, my);
        let r = if flip { my - 1 - r } else { r };
        values.extend(cols.iter().map(|&c| data[r * mx + c]));
    }
    Ok(Raster {
        w: w as u32,
        h: h as u32,
        values,
    })
}

/// Colour range from a sample of at most 256 x 256 points of the slice.
pub fn range(cat: &Catalog, spec: &ViewSpec) -> Result<RangeEstimate> {
    const N: usize = 256;
    let var = cat.var(&spec.var)?;
    ensure!(var.is_map(), "{} has fewer than two dimensions", var.name);
    let ny = var.dims[var.dims.len() - 2].len;
    let nx = var.dims[var.dims.len() - 1].len;
    let size = (nx.min(N) as u32, ny.min(N) as u32);
    let sample = view(
        cat,
        &ViewSpec {
            size,
            ..spec.clone()
        },
    )?;
    let (min, max) = sample.finite_range().unwrap_or((f32::NAN, f32::NAN));
    Ok(RangeEstimate {
        min,
        max,
        exact: nx <= N && ny <= N,
    })
}

/// Source index for output index `i` of `n` when sampling `m` source points.
fn nearest(i: usize, n: usize, m: usize) -> usize {
    (((i as f64 + 0.5) * m as f64 / n as f64) as usize).min(m - 1)
}

/// Whether the y coordinate decreases (first row is the top). Without a
/// coordinate variable, index 0 is taken as the bottom row, like ncview.
fn y_decreasing(cat: &Catalog, ydim: &DimInfo) -> bool {
    let is_coord = cat
        .reader
        .variables()
        .iter()
        .any(|v| v.name == ydim.name && v.dims.len() == 1 && v.dims[0].name == ydim.name);
    if !is_coord || ydim.len < 2 {
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
    use crate::{Reader, VarInfo};
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
                fixed: vec![1],
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
                fixed: vec![0],
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
                fixed: vec![0],
                size: (4, 2),
            },
        )
        .unwrap();
        assert_eq!((r.w, r.h), (4, 2));
        assert_eq!(r.values, [0.0, 2.0, 4.0, 6.0, 40.0, 42.0, 44.0, 46.0]);
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
                fixed: vec![0],
                size: (4, 3),
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("no records"));
    }
}
