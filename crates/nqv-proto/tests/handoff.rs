//! A view handed over a socket: the receiver opens it through the sender, which serves
//! the data on the same stream.
#![cfg(unix)]

use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use nqv_proto::handoff::{self, Answer, Handoff};
use nqv_proto::{Remote, serve};
use nqv_session::command::Command;
use nqv_session::{Backend, Local, OpenMode};

fn small() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/small.nc")
}

fn socket(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("nqv-test-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn sent() -> Handoff {
    Handoff {
        nqv: "test".into(),
        host: "cluster".into(),
        paths: vec![small()],
        mode: OpenMode::default(),
        commands: vec![Command::Var("temp".into())],
    }
}

#[test]
fn served_over_the_socket() {
    let path = socket("serve");
    let listener = UnixListener::bind(&path).unwrap();
    let desktop = std::thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let (h, conn) = handoff::accept(s, |_| false).unwrap();
        let rem = Remote::open(conn.unwrap(), &h.paths, &h.mode).unwrap();
        (h, rem.info().clone())
    });
    let missing = socket("missing");
    let (answer, stream) = handoff::offer(&[missing, path.clone()], &sent()).unwrap();
    assert_eq!(answer, Answer::Serve);
    let mut r = std::io::BufReader::new(stream.try_clone().unwrap());
    let mut w = std::io::BufWriter::new(stream);
    // Returns when the receiver closes the connection.
    serve(&mut r, &mut w, "test").unwrap();
    let (h, info) = desktop.join().unwrap();
    assert_eq!(h, sent());
    let local = Local::open(&[small()], &OpenMode::default()).unwrap();
    assert_eq!(&info, local.info());
    let _ = std::fs::remove_file(path);
}

#[test]
fn shown_locally() {
    let path = socket("local");
    let listener = UnixListener::bind(&path).unwrap();
    let desktop = std::thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let (h, conn) = handoff::accept(s, |h| h.host == "cluster").unwrap();
        assert!(conn.is_none());
        h
    });
    let (answer, _) = handoff::offer(std::slice::from_ref(&path), &sent()).unwrap();
    assert_eq!(answer, Answer::Done);
    assert_eq!(desktop.join().unwrap(), sent());
    let _ = std::fs::remove_file(path);
}

#[test]
fn stale_socket_removed() {
    let path = socket("stale");
    drop(UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    let e = handoff::offer(std::slice::from_ref(&path), &sent()).unwrap_err();
    assert!(format!("{e}").contains("nothing listening"), "{e}");
    assert!(!path.exists());
}
