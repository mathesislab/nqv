//! Maps on the wire: values quantised to 16 bits between the finite minimum and
//! maximum (65535 for missing), LZ4-compressed. A step is 1/65534 of the range, far
//! below what colours show; exact values come from their own requests.

use anyhow::{Result, ensure};
use nqv_core::{Raster, finite_range};
use serde::{Deserialize, Serialize};

const MISSING: u16 = u16::MAX;
const STEPS: f32 = (u16::MAX - 1) as f32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodedRaster {
    pub w: u32,
    pub h: u32,
    pub lo: f32,
    pub hi: f32,
    /// LZ4 (size prepended) of the little-endian u16 values.
    pub data: Vec<u8>,
}

impl EncodedRaster {
    pub fn encode(r: &Raster) -> Self {
        let (lo, hi) = finite_range(&r.values).unwrap_or((0.0, 0.0));
        let scale = if hi > lo { STEPS / (hi - lo) } else { 0.0 };
        let mut bytes = Vec::with_capacity(2 * r.values.len());
        for &v in &r.values {
            let q = match v.is_finite() {
                true => ((v - lo) * scale).round().clamp(0.0, STEPS) as u16,
                false => MISSING,
            };
            bytes.extend_from_slice(&q.to_le_bytes());
        }
        Self {
            w: r.w,
            h: r.h,
            lo,
            hi,
            data: lz4_flex::compress_prepend_size(&bytes),
        }
    }

    pub fn decode(&self) -> Result<Raster> {
        let bytes = lz4_flex::decompress_size_prepended(&self.data)?;
        let n = (self.w * self.h) as usize;
        ensure!(
            bytes.len() == 2 * n,
            "map of {} bytes for {n} values",
            bytes.len()
        );
        let step = if self.hi > self.lo {
            (self.hi - self.lo) / STEPS
        } else {
            0.0
        };
        let values = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| match u16::from_le_bytes(*b) {
                MISSING => f32::NAN,
                q => self.lo + q as f32 * step,
            })
            .collect();
        Ok(Raster {
            w: self.w,
            h: self.h,
            values,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_within_one_step() {
        let values: Vec<f32> = (0..1000)
            .map(|i| (i as f32 * 0.37).sin() * 3000.0)
            .collect();
        let r = Raster {
            w: 50,
            h: 20,
            values,
        };
        let e = EncodedRaster::encode(&r);
        let d = e.decode().unwrap();
        let step = (e.hi - e.lo) / STEPS;
        assert!(
            r.values
                .iter()
                .zip(&d.values)
                .all(|(a, b)| (a - b).abs() <= step)
        );
        assert_eq!((e.lo, e.hi), finite_range(&r.values).unwrap());
    }

    #[test]
    fn missing_and_constant_values() {
        let r = Raster {
            w: 3,
            h: 1,
            values: vec![f32::NAN, 7.0, 7.0],
        };
        let d = EncodedRaster::encode(&r).decode().unwrap();
        assert!(d.values[0].is_nan());
        assert_eq!(&d.values[1..], &[7.0, 7.0]);
        let empty = Raster {
            w: 2,
            h: 1,
            values: vec![f32::NAN; 2],
        };
        assert!(
            EncodedRaster::encode(&empty)
                .decode()
                .unwrap()
                .values
                .iter()
                .all(|v| v.is_nan())
        );
    }

    #[test]
    fn compresses_smooth_fields() {
        let values: Vec<f32> = (0..100_000).map(|i| (i / 400) as f32).collect();
        let e = EncodedRaster::encode(&Raster {
            w: 400,
            h: 250,
            values,
        });
        assert!(e.data.len() < 50_000, "{} bytes", e.data.len());
    }
}
