//! A session through the protocol (client and server in one process, over pipes) sees
//! the same as a session on the files directly.

use std::path::PathBuf;

use nqv_core::{ContourSpec, Interp, Levels, Mask, SeriesSpec, Show, VarRef, ViewSpec};
use nqv_proto::{Connection, Remote};
use nqv_session::{Backend, Files, Local, OpenMode};

fn small() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/small.nc")
}

/// A Remote talking to a server thread through two pipes.
fn remote(paths: &[PathBuf], mode: &OpenMode) -> Remote {
    let (cr, sw) = std::io::pipe().unwrap();
    let (sr, cw) = std::io::pipe().unwrap();
    std::thread::spawn(move || {
        let (mut r, mut w) = (std::io::BufReader::new(sr), std::io::BufWriter::new(sw));
        nqv_proto::serve(&mut r, &mut w, "test").unwrap();
    });
    let conn = Connection::over(Box::new(cr), Box::new(cw), "pipe").unwrap();
    assert_eq!(conn.server_version, "test");
    Remote::open(conn, paths, mode).unwrap()
}

#[test]
fn remote_matches_local() {
    let paths = [small()];
    let mode = OpenMode::default();
    let local = Local::open(&paths, &mode).unwrap();
    let rem = remote(&paths, &mode);
    assert_eq!(local.info(), rem.info());

    let var = VarRef {
        source: 0,
        name: "temp".into(),
    };
    let spec = ViewSpec {
        var: var.clone(),
        index: vec![1, 0, 0],
        size: (16, 12),
        mask: Mask::default(),
        interp: Interp::Bilinear,
        region: None,
    };
    let contour = ContourSpec {
        var: var.clone(),
        levels: Levels::Step(30.0),
    };
    let (a, b) = (
        local.map_data(&spec, Some(&contour), true).unwrap(),
        rem.map_data(&spec, Some(&contour), true).unwrap(),
    );
    // Maps agree to one 16-bit step; everything else exactly.
    let step = (a.range.max - a.range.min) / 65534.0;
    assert_eq!(a.raster.values.len(), b.raster.values.len());
    for (x, y) in a.raster.values.iter().zip(&b.raster.values) {
        assert!(
            x.is_nan() == y.is_nan() && (x.is_nan() || (x - y).abs() <= step),
            "{x} {y}"
        );
    }
    assert_eq!(
        (a.range, a.contours, a.stats),
        (b.range, b.contours, b.stats)
    );

    let s = SeriesSpec {
        var: var.clone(),
        along: 0,
        index: vec![0, 2, 3],
        max_points: 10,
        mask: Mask::default(),
    };
    assert_eq!(local.series(&s).unwrap(), rem.series(&s).unwrap());
    // The missing cell (lat 75, lon 315) is NaN on both sides.
    let missing = |b: &dyn Backend| b.value(&var, &[2, 5, 7], &Mask::default()).unwrap();
    assert!(missing(&local).unwrap().is_nan() && missing(&rem).unwrap().is_nan());
    assert_eq!(
        local.value(&var, &[1, 2, 3], &Mask::default()).unwrap(),
        rem.value(&var, &[1, 2, 3], &Mask::default()).unwrap()
    );
    assert_eq!(local.coord(0, "lat"), rem.coord(0, "lat"));
    assert_eq!(local.cell_at(&spec, (3, 4)), rem.cell_at(&spec, (3, 4)));
    let dir = small().parent().unwrap().to_path_buf();
    assert_eq!(local.list_dir(&dir).unwrap(), rem.list_dir(&dir).unwrap());
    assert_eq!(
        local.describe(&small()).unwrap(),
        rem.describe(&small()).unwrap()
    );
}

#[test]
fn errors_and_reopening_travel() {
    let rem = remote(&[small()], &OpenMode::default());
    let bad = ViewSpec {
        var: VarRef {
            source: 0,
            name: "nope".into(),
        },
        index: vec![],
        size: (4, 4),
        mask: Mask::default(),
        interp: Interp::Nearest,
        region: None,
    };
    let e = rem.view(&bad).unwrap_err().to_string();
    assert!(e.contains("nope"), "{e}");
    // Comparing the file with itself, then reopening it plain, on the same connection.
    let mut rem = remote(
        &[small(), small()],
        &OpenMode {
            diff: true,
            ..OpenMode::default()
        },
    );
    assert!(rem.info().compare);
    rem.set_show(Show::A);
    assert_eq!(rem.info().show, Show::A);
    rem.reopen(&[small()], &OpenMode::default()).unwrap();
    assert!(!rem.info().compare);
}

#[test]
fn ssh_without_a_terminal_says_why() {
    // ssh's own message becomes the error (needs ssh installed; .invalid never resolves)
    let e = Connection::ssh("nqv-test.invalid", None, nqv_proto::Prompt::Batch)
        .err()
        .expect("no such host");
    let e = format!("{e:#}");
    assert!(
        e.contains("ssh nqv-test.invalid:") && e.contains("resolve"),
        "{e}"
    );
}
