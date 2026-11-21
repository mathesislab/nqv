//! Coastlines over maps whose geography is known: Natural Earth 1:50m (public domain),
//! built into nqv (`data/coast50.bin`, see `examples/coastlines.rs`), drawn on maps in a
//! projection and on maps on their own grid alike.

use std::sync::OnceLock;

use crate::graticule::trace;
use crate::reproject::Axis;
use crate::view::cell_pixel;
use crate::{Catalog, Projection, Segment, ViewSpec};

const DATA: &[u8] = include_bytes!("../data/coast50.bin");

/// The coastlines: longitude and latitude of their points, degrees.
fn lines() -> &'static [Vec<(f64, f64)>] {
    static LINES: OnceLock<Vec<Vec<(f64, f64)>>> = OnceLock::new();
    LINES.get_or_init(|| {
        let mut at = 8; // past "NQVCOAST"
        let mut u32_ = || {
            let v = u32::from_le_bytes(DATA[at..at + 4].try_into().expect("4 bytes"));
            at += 4;
            v
        };
        let n = u32_() as usize;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let m = u32_() as usize;
            let line = (0..m)
                .map(|_| {
                    let lon = u32_() as i32 as f64 * 1e-5;
                    let lat = u32_() as i32 as f64 * 1e-5;
                    (lon, lat)
                })
                .collect();
            out.push(line);
        }
        out
    })
}

/// Longitude and latitude to a pixel of the view, or None where not shown.
type ToPixel<'a> = Box<dyn Fn(f64, f64) -> Option<(f64, f64)> + 'a>;

/// Coastlines over the map of `spec`, as segments in its pixels: through its target
/// projection, or into the cells of its own grid. Empty where the map's geography is
/// not known.
pub fn coastlines(cat: &Catalog, spec: &ViewSpec) -> Vec<Segment> {
    let (w, h) = spec.size;
    let to_pixel: ToPixel<'_> = match &spec.proj {
        Some(t) => {
            let t = t.clone();
            Box::new(move |lon, lat| t.proj.forward(lon, lat).map(|(x, y)| t.pixel(x, y, w, h)))
        }
        None => {
            let (Ok(Some(geo)), Some(px)) = (cat.geo_cached(&spec.var), cell_pixel(cat, spec))
            else {
                return vec![];
            };
            let lon = geo.proj == Projection::LatLon;
            let (ax, ay) = (Axis::new(&geo.x, lon), Axis::new(&geo.y, false));
            Box::new(move |lo, la| {
                let (x, y) = geo.proj.forward(lo, la)?;
                Some(px(ay.frac(y)?, ax.frac(x)?))
            })
        }
    };
    let mut out = vec![];
    for line in lines() {
        let px: Vec<Option<(f64, f64)>> = line.iter().map(|&(lo, la)| to_pixel(lo, la)).collect();
        out.extend(trace(&px, (w, h)));
    }
    out
}
