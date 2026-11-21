//! Settings from ~/.nqv/defaults (here given directly): colours for every view but
//! differences, display options, lines that do not fit skipped with a note, and the
//! command line still saying all of it.

use std::path::PathBuf;

use nqv_render::cmap_index;
use nqv_session::command::{RangeMode, parse};
use nqv_session::recipe;
use nqv_session::{OpenMode, Session};

fn small() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../nqv-proto/tests/data/small.nc")
}

fn defaults(text: &str) -> Vec<nqv_session::command::Command> {
    recipe::parse(text).unwrap()
}

#[test]
fn defaults_set_colours_and_display() {
    let mut s = Session::open(&[small()], &OpenMode::default()).unwrap();
    s.apply_defaults(defaults(
        "cmap cividis\nrange log\nlabels off\ninterp bilinear\nv nosuchvar\n",
    ));
    assert_eq!(s.cmap, cmap_index("cividis").unwrap());
    assert_eq!(s.range_mode, RangeMode::Log(None));
    assert!(!s.labels);
    assert_eq!(s.interp, nqv_core::Interp::Bilinear);
    // a variable these files lack: skipped, and said
    assert_eq!(s.var.name, "temp");
    let m = s.startup_message().unwrap();
    assert!(m.contains("defaults") && m.contains("skipped"), "{m}");
    // the command line holds what the defaults set, for anyone without them
    let cl = s.command_line();
    for f in [
        "--cmap cividis",
        "--range log",
        "--labels off",
        "--interp bilinear",
    ] {
        assert!(cl.contains(f), "{f} not in {cl}");
    }
}

#[test]
fn differences_keep_their_diverging_colours() {
    let mode = OpenMode {
        diff: true,
        ..OpenMode::default()
    };
    let mut s = Session::open(&[small(), small()], &mode).unwrap();
    s.apply_defaults(defaults("cmap cividis\n"));
    assert_eq!(s.cmap, cmap_index("RdBu_r").unwrap(), "A - B");
    s.apply(parse("show a").unwrap()).unwrap();
    assert_eq!(s.cmap, cmap_index("cividis").unwrap(), "A");
    s.apply(parse("show diff").unwrap()).unwrap();
    assert_eq!(s.cmap, cmap_index("RdBu_r").unwrap());
}
