//! Frames: a 4-byte big-endian length, then a postcard-encoded message.

use std::io::{ErrorKind, Read, Write};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Largest frame accepted (a map of a few thousand pixels square fits easily).
const MAX_FRAME: usize = 1 << 30;

pub fn write<T: Serialize>(w: &mut impl Write, msg: &T) -> Result<()> {
    let bytes = postcard::to_allocvec(msg).context("encoding a message")?;
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}

/// The next message, or None at the end of the stream (between frames).
pub fn read<T: DeserializeOwned>(r: &mut impl Read) -> Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        bail!("frame of {n} bytes is too large (not an nqv server?)");
    }
    let mut bytes = vec![0u8; n];
    r.read_exact(&mut bytes).context("reading a frame")?;
    Ok(Some(
        postcard::from_bytes(&bytes).context("decoding a message")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = vec![];
        write(&mut buf, &("hello".to_string(), 42u32)).unwrap();
        write(&mut buf, &vec![1.5f32, f32::NAN]).unwrap();
        let mut r = buf.as_slice();
        let a: (String, u32) = read(&mut r).unwrap().unwrap();
        assert_eq!(a, ("hello".to_string(), 42));
        let b: Vec<f32> = read(&mut r).unwrap().unwrap();
        assert_eq!(b[0], 1.5);
        assert!(b[1].is_nan());
        assert!(read::<u32>(&mut r).unwrap().is_none());
    }

    #[test]
    fn rejects_garbage() {
        let mut r: &[u8] = &[0xff, 0xff, 0xff, 0xff, 0];
        assert!(read::<u32>(&mut r).is_err());
    }
}
