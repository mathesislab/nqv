//! Contour lines of a variable over a map view (marching squares).

use anyhow::Result;

use crate::mask::{check_overlay, mask_index};
use crate::view::view;
use crate::{Catalog, Interp, Kind, VarRef, ViewSpec, finite_range};

/// Which values to draw contours at.
#[derive(Debug, Clone, PartialEq)]
pub enum Levels {
    /// About this many round values over the visible range.
    Auto(usize),
    /// Every multiple of this step within the visible range.
    Step(f64),
    List(Vec<f64>),
}

/// Contours of `var` with `levels`.
#[derive(Debug, Clone, PartialEq)]
pub struct ContourSpec {
    pub var: VarRef,
    pub levels: Levels,
}

/// A segment from one point to another, in view pixels (x right, y down; pixel `i`
/// covers `i..i + 1`).
pub type Segment = [(f32, f32); 2];

/// The line segments of one contour level.
#[derive(Debug, Clone, PartialEq)]
pub struct ContourLine {
    pub level: f64,
    pub segments: Vec<Segment>,
}

/// Most levels drawn at once.
const MAX_LEVELS: usize = 100;
/// The contoured field is read at up to this many points per view pixel and direction.
const OVERSAMPLE: u32 = 4;

/// About `n` round values (1, 2 or 5 times a power of ten apart) within [lo, hi].
pub fn nice_ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    if !(lo.is_finite() && hi.is_finite()) || hi <= lo || n == 0 {
        return vec![];
    }
    let raw = (hi - lo) / n as f64;
    let mag = 10f64.powf(raw.log10().floor());
    // The round step closest to the raw one (on a log scale).
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .min_by(|a, b| (a / raw).ln().abs().total_cmp(&(b / raw).ln().abs()))
        .unwrap_or(mag);
    steps(lo, hi, step)
}

/// Multiples of `step` within [lo, hi].
fn steps(lo: f64, hi: f64, step: f64) -> Vec<f64> {
    let (first, last) = ((lo / step).ceil() as i64, (hi / step).floor() as i64);
    (first..=last).map(|k| k as f64 * step).collect()
}

/// Check that contours of `c.var` can be drawn over `var` (same grid, compatible
/// dimensions), without reading data.
pub fn check_contour(cat: &Catalog, var: &VarRef, c: &ContourSpec) -> Result<()> {
    check_overlay(cat.var(var)?, cat.var(&c.var)?, "contour")
}

/// Contour lines of `c.var` over the map in `view` (same grid, same steps along shared
/// dimensions, same mask). The field is read at a finer resolution than the view, up to
/// its own resolution, so lines are smoother than the view's pixels.
pub fn contours(cat: &Catalog, view_spec: &ViewSpec, c: &ContourSpec) -> Result<Vec<ContourLine>> {
    let v = cat.var(&view_spec.var)?;
    let cv = cat.var(&c.var)?;
    check_overlay(v, cv, "contour")?;
    let Kind::Map { y, x } = cv.kind() else {
        return Ok(vec![]);
    };
    let (w, h) = view_spec.size;
    let (nx, ny) = (cv.dims[x].len as u32, cv.dims[y].len as u32);
    let f = OVERSAMPLE.min(nx / w.max(1)).min(ny / h.max(1)).max(1);
    let spec = ViewSpec {
        var: c.var.clone(),
        index: mask_index(v, &view_spec.index, cv)?,
        size: (w * f, h * f),
        mask: view_spec.mask.clone(),
        // Bilinear, so lines over coarse grids are smooth rather than stair steps.
        interp: Interp::Bilinear,
    };
    let field = view(cat, &spec)?;
    let Some((lo, hi)) = finite_range(&field.values) else {
        return Ok(vec![]);
    };
    let (lo, hi) = (lo as f64, hi as f64);
    let mut levels = match &c.levels {
        Levels::Auto(n) => nice_ticks(lo, hi, *n),
        Levels::Step(s) if *s > 0.0 => steps(lo, hi, *s),
        Levels::Step(_) => vec![],
        Levels::List(l) => l
            .iter()
            .copied()
            .filter(|l| (lo..=hi).contains(l))
            .collect(),
    };
    levels.truncate(MAX_LEVELS);
    let scale = 1.0 / f as f32;
    Ok(levels
        .into_iter()
        .map(|level| ContourLine {
            level,
            segments: march(
                &field.values,
                field.w as usize,
                field.h as usize,
                level as f32,
            )
            .into_iter()
            .map(|s| s.map(|(px, py)| (px * scale, py * scale)))
            .collect(),
        })
        .collect())
}

/// Marching squares on a grid of `w` by `h` values (row 0 at the top) whose points sit
/// at pixel centres. Squares with a missing corner are skipped; saddles are resolved by
/// the mean of the four corners.
fn march(values: &[f32], w: usize, h: usize, level: f32) -> Vec<Segment> {
    let mut out = vec![];
    if w < 2 || h < 2 {
        return out;
    }
    let at = |i: usize, j: usize| values[j * w + i];
    for j in 0..h - 1 {
        for i in 0..w - 1 {
            // Corners clockwise from the top left.
            let c = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            if c.iter().any(|v| !v.is_finite()) {
                continue;
            }
            let p = [
                (i as f32 + 0.5, j as f32 + 0.5),
                (i as f32 + 1.5, j as f32 + 0.5),
                (i as f32 + 1.5, j as f32 + 1.5),
                (i as f32 + 0.5, j as f32 + 1.5),
            ];
            let above: Vec<bool> = c.iter().map(|&v| v >= level).collect();
            // Crossing point on edge k (corner k to corner k + 1).
            let cross = |k: usize| {
                let (a, b) = (k, (k + 1) % 4);
                let t = (level - c[a]) / (c[b] - c[a]);
                let t = if t.is_finite() {
                    t.clamp(0.0, 1.0)
                } else {
                    0.5
                };
                (
                    p[a].0 + t * (p[b].0 - p[a].0),
                    p[a].1 + t * (p[b].1 - p[a].1),
                )
            };
            let edges: Vec<usize> = (0..4).filter(|&k| above[k] != above[(k + 1) % 4]).collect();
            match edges[..] {
                [a, b] => out.push([cross(a), cross(b)]),
                [e0, e1, e2, e3] => {
                    // Saddle: pair the edges so the centre stays on its side.
                    let centre_above = c.iter().sum::<f32>() / 4.0 >= level;
                    if centre_above == above[0] {
                        out.push([cross(e0), cross(e1)]);
                        out.push([cross(e2), cross(e3)]);
                    } else {
                        out.push([cross(e3), cross(e0)]);
                        out.push([cross(e1), cross(e2)]);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line_through_a_ramp() {
        // Values equal the column: level 1.5 crosses between columns 1 and 2, at x = 2.
        let (w, h) = (4, 3);
        let v: Vec<f32> = (0..w * h).map(|k| (k % w) as f32).collect();
        let segs = march(&v, w, h, 1.5);
        assert_eq!(segs.len(), h - 1);
        assert!(
            segs.iter().flatten().all(|p| (p.0 - 2.0).abs() < 1e-6),
            "{segs:?}"
        );
    }

    #[test]
    fn missing_values_break_lines() {
        let (w, h) = (4, 3);
        let mut v: Vec<f32> = (0..w * h).map(|k| (k % w) as f32).collect();
        v[w + 1] = f32::NAN; // a corner of both squares around row 1, column 1..2
        assert_eq!(march(&v, w, h, 1.5).len(), 0);
    }

    #[test]
    fn closed_contour_around_a_peak() {
        // A single high point in the middle of a 3x3 grid: four segments around it.
        let v = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(march(&v, 3, 3, 0.5).len(), 4);
    }

    #[test]
    fn ticks_are_round() {
        assert_eq!(nice_ticks(0.0, 10.0, 5), [0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        assert_eq!(
            nice_ticks(-21000.0, 0.0, 4),
            [-20000.0, -15000.0, -10000.0, -5000.0, 0.0]
        );
        assert!(nice_ticks(1.0, 1.0, 5).is_empty());
        assert_eq!(steps(-120.0, 1100.0, 500.0), [0.0, 500.0, 1000.0]);
    }
}
