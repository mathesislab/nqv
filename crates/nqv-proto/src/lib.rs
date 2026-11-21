//! nqv-proto: the protocol between a session and a server that reads the data (`nqv
//! serve --stdio`, usually over ssh). Messages are postcard-encoded, framed with their
//! length; maps travel quantised to 16 bits and LZ4-compressed.

pub mod frame;
pub mod message;
pub mod raster;
pub mod remote;
pub mod server;

pub use crate::message::{Envelope, Request, Response};
pub use crate::raster::EncodedRaster;
pub use crate::remote::{Connection, DEFAULT_REMOTE_CMD, Remote};
pub use crate::server::serve;

/// Protocol version; client and server must agree.
pub const VERSION: u32 = 1;
