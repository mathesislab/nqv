//! Requests (one per `Backend` method, plus opening) and their responses.

use std::path::PathBuf;

use nqv_core::{
    ContourLine, ContourSpec, DirEntry, Mask, Projection, RangeEstimate, Segment, Series,
    SeriesSpec, Show, Stats, VarInfo, VarRef, ViewSpec,
};
use nqv_session::{CatalogInfo, MapExtras, OpenMode};
use serde::{Deserialize, Serialize};

use crate::raster::EncodedRaster;

/// A message with the id of the request it belongs to (responses echo it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub id: u64,
    pub body: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// First message: the client's protocol version.
    Hello {
        version: u32,
    },
    /// Open paths (again, on the same connection) as the command line does.
    Open {
        paths: Vec<PathBuf>,
        mode: OpenMode,
    },
    SetShow(Show),
    View(ViewSpec),
    Range(ViewSpec),
    Stats(ViewSpec),
    Contours(ViewSpec, ContourSpec),
    CellAt(ViewSpec, (u32, u32)),
    Series(SeriesSpec),
    Value {
        var: VarRef,
        index: Vec<usize>,
        mask: Mask,
    },
    Coord {
        source: usize,
        dim: String,
    },
    CheckOverlay {
        var: VarRef,
        mask: Mask,
        contour: Option<ContourSpec>,
    },
    MapData {
        spec: ViewSpec,
        extras: MapExtras,
    },
    Coast(ViewSpec),
    ListDir(PathBuf),
    Describe(PathBuf),
    /// The text of a recipe in the server's ~/.nqv/recipes.
    Recipe(String),
    Bounds(VarRef, Projection),
    LonLat(VarRef, (usize, usize)),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    /// The server's protocol version and nqv version.
    Hello {
        version: u32,
        nqv: String,
    },
    Opened(CatalogInfo),
    Done,
    Raster(EncodedRaster),
    Range(RangeEstimate),
    Stats(Stats),
    Contours(Vec<ContourLine>),
    Cell(Option<(usize, usize)>),
    Series(Series, Option<(f32, f32)>),
    Value(Option<f32>),
    Coord(Option<Vec<f32>>),
    Map {
        raster: EncodedRaster,
        range: RangeEstimate,
        contours: Vec<ContourLine>,
        stats: Option<Stats>,
        coast: Vec<Segment>,
    },
    Coast(Vec<Segment>),
    Dir(Vec<DirEntry>),
    Vars(Vec<VarInfo>),
    Text(Option<String>),
    Bounds([f64; 4]),
    LonLat(Option<(f64, f64)>),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame;
    use nqv_core::{Interp, Op};

    #[test]
    fn requests_round_trip() {
        let spec = ViewSpec {
            var: VarRef {
                source: 1,
                name: "H_ice".into(),
            },
            index: vec![3, 0, 0],
            size: (400, 300),
            mask: Mask {
                conds: vec![nqv_core::Cond {
                    var: VarRef {
                        source: 1,
                        name: "f_ice".into(),
                    },
                    op: Op::Lt,
                    values: vec![0.5],
                }],
            },
            interp: Interp::Bilinear,
            region: None,
            proj: None,
        };
        let msgs = [
            Request::Hello { version: 1 },
            Request::Open {
                paths: vec!["run/".into()],
                mode: OpenMode {
                    join: true,
                    ..OpenMode::default()
                },
            },
            Request::MapData {
                spec: spec.clone(),
                extras: MapExtras {
                    stats: true,
                    coast: true,
                    ..MapExtras::default()
                },
            },
            Request::CellAt(spec, (10, 20)),
        ];
        let mut buf = vec![];
        for (id, body) in msgs.iter().enumerate() {
            let e = Envelope {
                id: id as u64,
                body: body.clone(),
            };
            frame::write(&mut buf, &e).unwrap();
        }
        let mut r = buf.as_slice();
        for (id, body) in msgs.iter().enumerate() {
            let e: Envelope<Request> = frame::read(&mut r).unwrap().unwrap();
            assert_eq!((e.id, &e.body), (id as u64, body));
        }
    }
}
