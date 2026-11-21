//! Projected views in a session: `:proj`, zooming by coordinates or a lon/lat box, the
//! wheel and a drag, and the command line that brings it all back.

use std::path::PathBuf;

use nqv_core::Projection;
use nqv_session::command::parse;
use nqv_session::{OpenMode, Session};

fn yelmo() -> Session {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../nqv-core/tests/data/stere.nc");
    let mut s = Session::open(&[p], &OpenMode::default()).unwrap();
    s.apply(parse("v dist").unwrap()).unwrap();
    s
}

#[test]
fn proj_then_back_to_the_grid() {
    let mut s = yelmo();
    s.apply(parse("proj latlon").unwrap()).unwrap();
    let t = s.target.clone().expect("projected");
    assert_eq!(t.proj, Projection::LatLon);
    // a wide map: Greenland spans more longitude than latitude
    let (w, h) = s.map_size(1000, 1000).unwrap();
    assert!(w > h, "{w}x{h}");
    let m = s.map((w, h), true).unwrap();
    assert!(m.raster.values.iter().any(|v| v.is_finite()));
    assert!(m.stats.is_some());
    assert!(s.graticule((w, h)).is_some_and(|g| !g.meridians.is_empty()));
    s.apply(parse("proj native").unwrap()).unwrap();
    assert!(s.target.is_none() && s.graticule((w, h)).is_none());
    assert!(parse("proj mercator").is_err());
}

#[test]
fn zoom_by_coordinates_and_lon_lat_box() {
    let mut s = yelmo();
    s.apply(parse("proj stere north").unwrap()).unwrap();
    s.apply(parse("zoom x=-200:600 y=-3000:-2000").unwrap())
        .unwrap();
    assert_eq!(
        s.target.as_ref().unwrap().bounds,
        [-200e3, 600e3, -3000e3, -2000e3]
    );
    s.apply(parse("zoom lon=-55:-30 lat=60:70").unwrap())
        .unwrap();
    let b = s.target.as_ref().unwrap().bounds;
    // that box's corners are inside the bounds
    let p = Projection::parse("stere north").unwrap();
    for (lon, lat) in [(-55.0, 60.0), (-30.0, 60.0), (-55.0, 70.0), (-30.0, 70.0)] {
        let (x, y) = p.forward(lon, lat).unwrap();
        assert!(x >= b[0] - 1.0 && x <= b[1] + 1.0 && y >= b[2] - 1.0 && y <= b[3] + 1.0);
    }
    assert!(s.apply(parse("zoom xc=@1:@5").unwrap()).is_err());
    s.apply(parse("zoom off").unwrap()).unwrap();
    let first = s.target.as_ref().unwrap().bounds;
    assert!(first[1] - first[0] > b[1] - b[0]);
}

#[test]
fn command_line_brings_the_view_back() {
    let mut s = yelmo();
    for c in ["proj laea north", "zoom lon=-60:-20 lat=58:84"] {
        s.apply(parse(c).unwrap()).unwrap();
    }
    let lines: Vec<String> = s.settings().into_iter().map(|(l, _)| l).collect();
    assert!(lines.contains(&"proj laea north".to_string()), "{lines:?}");
    let zoom = lines
        .iter()
        .find(|l| l.starts_with("zoom x="))
        .expect("zoom");
    let mut t = yelmo();
    for l in &lines {
        t.apply(parse(l).unwrap()).unwrap();
    }
    let (a, b) = (s.target.clone().unwrap(), t.target.unwrap());
    assert_eq!(a.proj, b.proj);
    for k in 0..4 {
        assert!((a.bounds[k] - b.bounds[k]).abs() < 1.0, "{zoom}");
    }
    assert!(
        s.command_line().contains("--proj 'laea north'"),
        "{}",
        s.command_line()
    );
}

#[test]
fn wheel_and_drag() {
    let mut s = yelmo();
    s.apply(parse("proj stere north").unwrap()).unwrap();
    let size = s.map_size(400, 400).unwrap();
    let spec = s.map_spec(size);
    let before = s.target.clone().unwrap();
    // in around the centre: half as wide, the centre kept
    s.zoom_at(&spec, size.0 / 2, size.1 / 2, 0.5);
    let z = s.target.clone().unwrap();
    let wid = |b: [f64; 4]| b[1] - b[0];
    assert!((wid(z.bounds) / wid(before.bounds) - 0.5).abs() < 0.01);
    // a drag to the right moves the view west (smaller x)
    let spec = s.map_spec(size);
    s.pan(&spec, 40, 0);
    assert!(s.target.as_ref().unwrap().bounds[0] < z.bounds[0]);
    // out past the whole map: the first view again
    let spec = s.map_spec(size);
    s.zoom_at(&spec, 10, 10, 8.0);
    assert_eq!(s.target.unwrap().bounds, before.bounds);
}
