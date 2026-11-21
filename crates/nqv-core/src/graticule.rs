//! Meridians and parallels over a map in a projection, as line segments in view
//! pixels, drawn as contours are.

use crate::{ContourLine, Segment, Target};

/// Meridians (`level`: longitude) and parallels (`level`: latitude) of a `size` view of
/// `target`, at round steps giving a few lines across it.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Graticule {
    pub meridians: Vec<ContourLine>,
    pub parallels: Vec<ContourLine>,
}

/// Steps between lines, in degrees, the coarsest giving at least a few lines.
const STEPS: [f64; 10] = [1.0, 2.0, 5.0, 10.0, 15.0, 20.0, 30.0, 45.0, 60.0, 90.0];

pub fn graticule(target: &Target, size: (u32, u32)) -> Graticule {
    let (w, h) = (size.0.max(1), size.1.max(1));
    // what the view spans: latitudes, and longitudes (all of them around a pole)
    let mut lat = (f64::INFINITY, f64::NEG_INFINITY);
    let mut lons = vec![];
    const N: u32 = 32;
    for j in 0..=N {
        for i in 0..=N {
            let (x, y) = target.at(
                i as f64 * w as f64 / N as f64 - 0.5,
                j as f64 * h as f64 / N as f64 - 0.5,
                w,
                h,
            );
            if let Some((lo, la)) = target.proj.inverse(x, y) {
                lat = (lat.0.min(la), lat.1.max(la));
                lons.push(lo);
            }
        }
    }
    if lons.is_empty() {
        return Graticule::default();
    }
    let pole = |p: f64| {
        target.proj.forward(0.0, p).is_some_and(|(x, y)| {
            let (px, py) = target.pixel(x, y, w, h);
            (0.0..=w as f64).contains(&px) && (0.0..=h as f64).contains(&py)
        })
    };
    let lon_span = if pole(90.0) || pole(-90.0) {
        360.0
    } else {
        let c = lons[lons.len() / 2];
        let rel: Vec<f64> = lons
            .iter()
            .map(|l| (l - c + 180.0).rem_euclid(360.0) - 180.0)
            .collect();
        let (a, b) = rel
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                (a.min(v), b.max(v))
            });
        b - a
    };
    let step = |span: f64| *STEPS.iter().find(|&&s| span / s <= 7.0).unwrap_or(&90.0);
    // around a pole: every 30 degrees, as polar maps have them
    let dlon = if lon_span >= 360.0 {
        30.0
    } else {
        step(lon_span)
    };
    let dlat = step(lat.1 - lat.0);
    let trace = |pts: &mut dyn Iterator<Item = (f64, f64)>| -> Vec<Segment> {
        let px: Vec<Option<(f64, f64)>> = pts
            .map(|(lo, la)| {
                target
                    .proj
                    .forward(lo, la)
                    .map(|(x, y)| target.pixel(x, y, w, h))
            })
            .collect();
        trace(&px, (w, h))
    };
    let lines = |levels: Vec<f64>, along: &dyn Fn(f64) -> Vec<(f64, f64)>| {
        levels
            .into_iter()
            .filter_map(|level| {
                let segments = trace(&mut along(level).into_iter());
                (!segments.is_empty()).then_some(ContourLine { level, segments })
            })
            .collect::<Vec<_>>()
    };
    // Parallels within the view's latitudes, short of the poles; meridians up to the
    // last parallel, so they do not crowd at a pole.
    let first = (lat.0 / dlat).ceil() as i64;
    let last = (lat.1 / dlat).floor() as i64;
    let lats: Vec<f64> = (first..=last)
        .map(|k| k as f64 * dlat)
        .filter(|l| l.abs() < 90.0 - 1e-9)
        .collect();
    let lat_max = match pole(90.0) {
        true => lats.last().copied().unwrap_or(lat.1),
        false => lat.1,
    };
    let lat_min = match pole(-90.0) {
        true => lats.first().copied().unwrap_or(lat.0),
        false => lat.0,
    };
    const STEPS_ALONG: usize = 240;
    let meridians = lines(
        (0..(360.0 / dlon).round() as i64)
            .map(|k| k as f64 * dlon - 180.0)
            .collect(),
        &|lon| {
            (0..=STEPS_ALONG)
                .map(|i| {
                    (
                        lon,
                        lat_min + (lat_max - lat_min) * i as f64 / STEPS_ALONG as f64,
                    )
                })
                .collect()
        },
    );
    let parallels = lines(lats, &|la| {
        let n = 4 * STEPS_ALONG;
        (0..=n)
            .map(|i| (-180.0 + 360.0 * i as f64 / n as f64, la))
            .collect()
    });
    Graticule {
        meridians,
        parallels,
    }
}

/// A line through points along it, in pixels of a `size` view (None: not shown):
/// segments between neighbours both shown, at least one in view, not jumping across the
/// view (a seam).
pub(crate) fn trace(px: &[Option<(f64, f64)>], size: (u32, u32)) -> Vec<Segment> {
    let (w, h) = (size.0 as f64, size.1 as f64);
    let in_view = |p: (f64, f64)| p.0 >= -1.0 && p.0 <= w + 1.0 && p.1 >= -1.0 && p.1 <= h + 1.0;
    px.windows(2)
        .filter_map(|p| {
            let (a, b) = (p[0]?, p[1]?);
            let jump = (a.0 - b.0).abs() > w / 2.0 || (a.1 - b.1).abs() > h / 2.0;
            ((in_view(a) || in_view(b)) && !jump)
                .then_some([(a.0 as f32, a.1 as f32), (b.0 as f32, b.1 as f32)])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Projection;

    #[test]
    fn latlon_lines_are_straight_and_round() {
        let t = Target {
            proj: Projection::LatLon,
            bounds: [-80.0, -10.0, 55.0, 85.0],
        };
        let g = graticule(&t, (700, 300));
        let lons: Vec<f64> = g.meridians.iter().map(|l| l.level).collect();
        let lats: Vec<f64> = g.parallels.iter().map(|l| l.level).collect();
        assert_eq!(
            lons,
            [-80.0, -70.0, -60.0, -50.0, -40.0, -30.0, -20.0, -10.0]
        );
        assert_eq!(lats, [55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0]);
        // a meridian is vertical: x constant, at its pixel
        for s in &g.meridians[2].segments {
            assert!((s[0].0 - 200.0).abs() < 1e-3 && (s[1].0 - 200.0).abs() < 1e-3);
        }
    }

    #[test]
    fn polar_view_has_all_meridians() {
        let proj = Projection::parse("stere north").unwrap();
        let (x, _) = proj.forward(45.0, 60.0).unwrap();
        let r = x.abs() * 1.5;
        let t = Target {
            proj,
            bounds: [-r, r, -r, r],
        };
        let g = graticule(&t, (400, 400));
        assert_eq!(g.meridians.len(), 12, "every 30 degrees");
        assert!(
            g.parallels
                .iter()
                .all(|l| l.level >= 40.0 && l.level < 90.0)
        );
        assert!(!g.parallels.is_empty());
    }
}
