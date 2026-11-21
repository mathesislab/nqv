//! Map geography from real files: Yelmo's CF polar stereographic grid (every 5th
//! point of GRL-16KM), and the same grid without its grid mapping, as CLIMBER-X writes
//! it, given by a grid file.

use std::path::PathBuf;

use nqv_core::{Catalog, Projection, VarRef};

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
