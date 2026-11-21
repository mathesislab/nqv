//! The server side: `nqv serve --stdio` answers requests on stdin/stdout with the files
//! of the machine it runs on.

use std::io::{Read, Write};

use anyhow::Result;
use nqv_session::{Backend, Files, Local, LocalFiles};

use crate::VERSION;
use crate::frame;
use crate::message::{Envelope, Request, Response};
use crate::raster::EncodedRaster;

/// Answer requests from `r` on `w` until the client closes the stream. `nqv` is the
/// server's version, reported in the handshake.
pub fn serve(r: &mut impl Read, w: &mut impl Write, nqv: &str) -> Result<()> {
    let Some(first) = frame::read::<Envelope<Request>>(r)? else {
        return Ok(());
    };
    let hello = Response::Hello {
        version: VERSION,
        nqv: nqv.to_string(),
    };
    frame::write(
        w,
        &Envelope {
            id: first.id,
            body: hello,
        },
    )?;
    match first.body {
        Request::Hello { version } if version == VERSION => {}
        // The client sees the versions in the handshake and gives up.
        _ => return Ok(()),
    }
    let mut local: Option<Local> = None;
    while let Some(req) = frame::read::<Envelope<Request>>(r)? {
        let body = answer(&mut local, req.body);
        frame::write(w, &Envelope { id: req.id, body })?;
    }
    Ok(())
}

fn answer(local: &mut Option<Local>, req: Request) -> Response {
    let err = |e: anyhow::Error| Response::Error(format!("{e:#}"));
    match req {
        Request::Hello { .. } => return Response::Error("hello twice".into()),
        Request::Open { paths, mode } => {
            return match Local::open(&paths, &mode) {
                Ok(l) => {
                    let info = l.info().clone();
                    *local = Some(l);
                    Response::Opened(info)
                }
                Err(e) => err(e),
            };
        }
        Request::ListDir(p) => return LocalFiles.list_dir(&p).map_or_else(err, Response::Dir),
        Request::Describe(p) => return LocalFiles.describe(&p).map_or_else(err, Response::Vars),
        _ => {}
    }
    let Some(b) = local else {
        return Response::Error("no files open".into());
    };
    match req {
        Request::SetShow(show) => {
            b.set_show(show);
            Response::Done
        }
        Request::View(spec) => b
            .view(&spec)
            .map_or_else(err, |r| Response::Raster(EncodedRaster::encode(&r))),
        Request::Range(spec) => b.range(&spec).map_or_else(err, Response::Range),
        Request::Stats(spec) => b.stats(&spec).map_or_else(err, Response::Stats),
        Request::Contours(spec, c) => b.contours(&spec, &c).map_or_else(err, Response::Contours),
        Request::CellAt(spec, px) => Response::Cell(b.cell_at(&spec, px)),
        Request::Series(spec) => b
            .series(&spec)
            .map_or_else(err, |(s, r)| Response::Series(s, r)),
        Request::Value { var, index, mask } => b
            .value(&var, &index, &mask)
            .map_or_else(err, Response::Value),
        Request::Coord { source, dim } => Response::Coord(b.coord(source, &dim)),
        Request::CheckOverlay { var, mask, contour } => b
            .check_overlay(&var, &mask, contour.as_ref())
            .map_or_else(err, |_| Response::Done),
        Request::MapData {
            spec,
            contour,
            with_stats,
        } => b
            .map_data(&spec, contour.as_ref(), with_stats)
            .map_or_else(err, |p| Response::Map {
                raster: EncodedRaster::encode(&p.raster),
                range: p.range,
                contours: p.contours,
                stats: p.stats,
            }),
        Request::Hello { .. }
        | Request::Open { .. }
        | Request::ListDir(_)
        | Request::Describe(_) => {
            unreachable!("answered above")
        }
    }
}
