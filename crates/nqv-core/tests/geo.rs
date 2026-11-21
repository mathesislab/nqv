//! Map geography from real files: Yelmo's CF polar stereographic grid (every 5th
//! point of GRL-16KM), and the same grid without its grid mapping, as CLIMBER-X writes
//! it, given by a grid file.

use std::path::PathBuf;

use nqv_core::{
    Catalog, Interp, Mask, Projection, Target, VarRef, ViewSpec, cell_at, coastlines,
    default_bounds, stats, view,
};

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn var(name: &str) -> VarRef {
    VarRef {
        source: 0,
        name: name.into(),
    }
}

#[test]
fn yelmo_grid_agrees_with_its_latitudes_and_longitudes() {
    let cat = Catalog::open(&[data("stere.nc")]).unwrap();
    let g = cat.geo(&var("dist")).unwrap().unwrap();
    assert_eq!(g.proj, Projection::parse("stere north").unwrap());
    assert_eq!((g.x.len(), g.y.len()), (22, 37));
    // km in the file, metres here
    assert!((g.x[1] - g.x[0] - 80_000.0).abs() < 1e-6, "{:?}", &g.x[..2]);

    let read = |n: &str| cat.read(&var(n), &[0, 0], &[37, 22], &[1, 1]).unwrap();
    let (lat, lon) = (read("lat2D"), read("lon2D"));
    let mut worst: f64 = 0.0;
    for (j, &y) in g.y.iter().enumerate() {
        for (i, &x) in g.x.iter().enumerate() {
            let (plon, plat) = g.proj.inverse(x, y).unwrap();
            let k = j * 22 + i;
            let dlon = (plon - lon[k] as f64 + 180.0).rem_euclid(360.0) - 180.0;
            worst = worst.max((plat - lat[k] as f64).abs()).max(dlon.abs());
        }
    }
    // f32 in the reader: about 1e-5 degrees
    assert!(worst < 1e-4, "worst difference {worst} degrees");
}

#[test]
fn grid_mapping_variables_are_not_data() {
    let cat = Catalog::open(&[data("stere.nc")]).unwrap();
    let names: Vec<String> = cat.data_vars().into_iter().map(|(r, _)| r.name).collect();
    assert_eq!(names, ["lat2D", "lon2D", "dist"]);
}

#[test]
fn a_grid_file_gives_what_the_file_lacks() {
    let mut cat = Catalog::open(&[data("nocrs.nc")]).unwrap();
    // grid_mapping = "crs", but no crs variable
    assert_eq!(cat.geo(&var("dist")).unwrap(), None);
    cat.set_grid(&data("stere.nc")).unwrap();
    let own = Catalog::open(&[data("stere.nc")]).unwrap();
    assert_eq!(
        cat.geo(&var("dist")).unwrap(),
        own.geo(&var("dist")).unwrap()
    );
    // a grid file must have a grid
    assert!(cat.set_grid(&data("nocrs.nc")).is_err());
}

fn small() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../nqv-proto/tests/data/small.nc")
}

fn spec(name: &str, index: Vec<usize>, size: (u32, u32), proj: Option<Target>) -> ViewSpec {
    ViewSpec {
        var: var(name),
        index,
        size,
        mask: Mask::default(),
        interp: Interp::Nearest,
        region: None,
        proj,
    }
}

/// The grid's own projection, bounds on its cell edges, one pixel per cell: the same
/// map as on its grid, the same cells under the pointer.
#[test]
fn own_projection_reproduces_the_grid() {
    let cat = Catalog::open(&[data("stere.nc")]).unwrap();
    let g = cat.geo(&var("dist")).unwrap().unwrap();
    let (dx, dy) = (g.x[1] - g.x[0], g.y[1] - g.y[0]);
    let target = Target {
        proj: g.proj.clone(),
        bounds: [
            g.x[0] - dx / 2.0,
            g.x[21] + dx / 2.0,
            g.y[0] - dy / 2.0,
            g.y[36] + dy / 2.0,
        ],
    };
    let native = spec("dist", vec![0, 0], (22, 37), None);
    let projected = spec("dist", vec![0, 0], (22, 37), Some(target));
    assert_eq!(
        view(&cat, &projected).unwrap(),
        view(&cat, &native).unwrap()
    );
    for py in 0..37 {
        for px in 0..22 {
            assert_eq!(
                cell_at(&cat, &projected, (px, py)),
                cell_at(&cat, &native, (px, py))
            );
        }
    }
    // statistics are of the cells shown, as on the grid
    assert_eq!(
        stats(&cat, &projected).unwrap(),
        stats(&cat, &native).unwrap()
    );
}

/// Yelmo's latitudes shown in latitude-longitude: each pixel shows about its own
/// latitude (to within the cells, 80 km here).
#[test]
fn polar_stereographic_in_latitude_longitude() {
    let cat = Catalog::open(&[data("stere.nc")]).unwrap();
    let proj = Projection::LatLon;
    let bounds = default_bounds(&cat, &var("lat2D"), &proj).unwrap();
    // Greenland, around 60-85 N, 75-10 W
    assert!(bounds[0] < -60.0 && bounds[1] > -20.0 && bounds[2] < 62.0 && bounds[3] > 80.0);
    let target = Target { proj, bounds };
    let (w, h) = (240, 160);
    let r = view(
        &cat,
        &spec("lat2D", vec![0, 0], (w, h), Some(target.clone())),
    )
    .unwrap();
    let mut shown = 0;
    for py in 0..h {
        for px in 0..w {
            let v = r.values[(py * w + px) as usize];
            if v.is_finite() {
                let (_, lat) = target.at(px as f64, py as f64, w, h);
                assert!((v as f64 - lat).abs() < 0.8, "lat {lat}, shown {v}");
                shown += 1;
            }
        }
    }
    assert!(shown > (w * h / 4) as usize, "{shown} pixels shown");
}

/// A global latitude-longitude grid (45 degrees, longitudes 0-315) in polar
/// stereographic: every pixel shows its nearest cell, across the seam at 0/360 too.
#[test]
fn global_grid_in_polar_stereographic() {
    let cat = Catalog::open(&[small()]).unwrap();
    let proj = Projection::parse("stere north").unwrap();
    let bounds = default_bounds(&cat, &var("temp"), &proj).unwrap();
    let target = Target { proj, bounds };
    let (w, h) = (120, 120);
    let r = view(
        &cat,
        &spec("temp", vec![0, 0, 0], (w, h), Some(target.clone())),
    )
    .unwrap();
    let (mut checked, mut seam) = (0, 0);
    for py in 0..h {
        for px in 0..w {
            let (x, y) = target.at(px as f64, py as f64, w, h);
            let Some((lon, lat)) = target.proj.inverse(x, y) else {
                continue;
            };
            let lon = lon.rem_euclid(360.0);
            // away from cell edges (half-way between centres)
            let near_edge = |v: f64| ((v / 45.0).fract() - 0.5).abs() < 0.03;
            if near_edge(lon) || near_edge(lat + 75.0 + 22.5) || lat < -97.5 {
                continue;
            }
            let lat_c = (((lat + 75.0) / 30.0).round() * 30.0 - 75.0).clamp(-75.0, 75.0);
            let lon_c = ((lon / 45.0).round() * 45.0) % 360.0;
            let expect = lat_c + lon_c / 100.0;
            let v = r.values[(py * w + px) as usize];
            if lat_c == 75.0 && lon_c == 315.0 {
                assert!(v.is_nan(), "missing value shown as {v}");
            } else {
                assert!(
                    (v as f64 - expect).abs() < 1e-3,
                    "{lon},{lat}: {v}, expected {expect}"
                );
            }
            checked += 1;
            seam += (lon > 337.5 || lon < 22.5) as usize;
        }
    }
    assert!(
        checked > 5000 && seam > 300,
        "{checked} pixels checked, {seam} at the seam"
    );

    // blended: within the values around, the same pixels missing as nearest
    let mut b = spec("temp", vec![0, 0, 0], (w, h), Some(target));
    b.interp = Interp::Bilinear;
    let rb = view(&cat, &b).unwrap();
    for (n, s) in r.values.iter().zip(&rb.values) {
        assert_eq!(n.is_nan(), s.is_nan());
        assert!(s.is_nan() || (-75.0..=78.2).contains(s), "{s}");
    }
}

/// Or a projection for the file's own x/y: CLIMBER-X's NH grids are `stere north`.
#[test]
fn a_projection_for_the_files_own_coordinates() {
    let mut cat = Catalog::open(&[data("nocrs.nc")]).unwrap();
    cat.set_grid_proj(Projection::parse("stere north").unwrap());
    let own = Catalog::open(&[data("stere.nc")]).unwrap();
    assert_eq!(
        cat.geo(&var("dist")).unwrap(),
        own.geo(&var("dist")).unwrap()
    );
}

/// Coastlines into the grid's own cells agree with coastlines through its projection
/// (bounds on the cell edges, a pixel per cell): two ways to the same pixels.
#[test]
fn coastlines_on_the_grid_and_projected_agree() {
    let cat = Catalog::open(&[data("stere.nc")]).unwrap();
    let g = cat.geo(&var("dist")).unwrap().unwrap();
    let (dx, dy) = (g.x[1] - g.x[0], g.y[1] - g.y[0]);
    let target = Target {
        proj: g.proj.clone(),
        bounds: [
            g.x[0] - dx / 2.0,
            g.x[21] + dx / 2.0,
            g.y[0] - dy / 2.0,
            g.y[36] + dy / 2.0,
        ],
    };
    let size = (220, 370);
    let native = coastlines(&cat, &spec("dist", vec![0, 0], size, None));
    let projected = coastlines(&cat, &spec("dist", vec![0, 0], size, Some(target)));
    assert!(native.len() > 200, "{} segments", native.len());
    // the native path drops points beyond the outer cell centres: a few segments fewer
    assert!(projected.len() >= native.len());
    for s in &native {
        let found = projected.iter().any(|p| {
            (0..2).all(|k| (p[k].0 - s[k].0).abs() < 0.05 && (p[k].1 - s[k].1).abs() < 0.05)
        });
        assert!(found, "{s:?} not among the projected segments");
        for (x, y) in s {
            assert!((-1.0..=221.0).contains(x) && (-1.0..=371.0).contains(y));
        }
    }
}

/// A global latitude-longitude grid on its own grid: every coast, none across the seam.
#[test]
fn coastlines_on_a_global_grid() {
    let cat = Catalog::open(&[small()]).unwrap();
    let c = coastlines(&cat, &spec("temp", vec![0, 0, 0], (360, 180), None));
    assert!(c.len() > 20_000, "{} segments", c.len());
    assert!(c.iter().all(|s| (s[0].0 - s[1].0).abs() < 180.0));
    // a grid nqv cannot place gets none
    let cat = Catalog::open(&[data("nocrs.nc")]).unwrap();
    assert!(coastlines(&cat, &spec("dist", vec![0, 0], (22, 37), None)).is_empty());
}
