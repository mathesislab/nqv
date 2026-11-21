//! nqv-render: values to colours. Pure functions, no I/O, no dependencies.

/// RGBA8 image, row-major, row 0 at the top.
#[derive(Debug, Clone, PartialEq)]
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// Linear normalisation of values to [0, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Norm {
    pub min: f32,
    pub max: f32,
}

impl Norm {
    /// Range symmetric around zero that covers `min` and `max`.
    pub fn symmetric(min: f32, max: f32) -> Self {
        let a = min.abs().max(max.abs());
        Self { min: -a, max: a }
    }

    pub fn apply(&self, v: f32) -> f32 {
        if self.max > self.min {
            ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.5
        }
    }
}

/// A colour map: colours at evenly spaced stops, interpolated linearly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cmap {
    pub name: &'static str,
    stops: &'static [[u8; 3]],
}

/// Colour of missing values.
pub const MISSING: [u8; 3] = [48, 48, 48];

pub const VIRIDIS: Cmap = Cmap {
    name: "viridis",
    stops: &[
        [0x44, 0x01, 0x54],
        [0x47, 0x2c, 0x7a],
        [0x3b, 0x51, 0x8b],
        [0x2c, 0x71, 0x8e],
        [0x21, 0x90, 0x8d],
        [0x27, 0xad, 0x81],
        [0x5c, 0xc8, 0x63],
        [0xaa, 0xdc, 0x32],
        [0xfd, 0xe7, 0x25],
    ],
};

/// Diverging blue (low) to red (high), white at the centre.
pub const RDBU_R: Cmap = Cmap {
    name: "RdBu_r",
    stops: &[
        [0x05, 0x30, 0x61],
        [0x21, 0x66, 0xac],
        [0x43, 0x93, 0xc3],
        [0x92, 0xc5, 0xde],
        [0xd1, 0xe5, 0xf0],
        [0xf7, 0xf7, 0xf7],
        [0xfd, 0xdb, 0xc7],
        [0xf4, 0xa5, 0x82],
        [0xd6, 0x60, 0x4d],
        [0xb2, 0x18, 0x2b],
        [0x67, 0x00, 0x1f],
    ],
};

pub const GRAYS: Cmap = Cmap {
    name: "grays",
    stops: &[[0, 0, 0], [255, 255, 255]],
};

pub const CMAPS: [Cmap; 3] = [VIRIDIS, RDBU_R, GRAYS];

impl Cmap {
    /// Colour at `t` in [0, 1].
    pub fn at(&self, t: f32) -> [u8; 3] {
        let n = self.stops.len() - 1;
        let x = t.clamp(0.0, 1.0) * n as f32;
        let i = (x as usize).min(n - 1);
        let f = x - i as f32;
        let (a, b) = (self.stops[i], self.stops[i + 1]);
        std::array::from_fn(|k| (a[k] as f32 + f * (b[k] as f32 - a[k] as f32)).round() as u8)
    }
}

/// Colour a field of `w` by `h` values (row 0 at the top). NaN gets `MISSING`.
pub fn colorize(w: u32, h: u32, values: &[f32], norm: &Norm, cmap: &Cmap) -> Rgba {
    assert_eq!(values.len(), (w * h) as usize);
    let mut data = Vec::with_capacity(values.len() * 4);
    for &v in values {
        let c = if v.is_finite() {
            cmap.at(norm.apply(v))
        } else {
            MISSING
        };
        data.extend([c[0], c[1], c[2], 255]);
    }
    Rgba { w, h, data }
}

/// Nearest xterm 256-colour index for an RGB colour (6x6x6 cube or grey ramp).
pub fn ansi256(c: [u8; 3]) -> u8 {
    let level = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            (v - 35) / 40
        }
    };
    let cube = |l: u8| if l == 0 { 0 } else { 55 + 40 * l as i32 };
    let (r, g, b) = (level(c[0]), level(c[1]), level(c[2]));
    let cube_idx = 16 + 36 * r + 6 * g + b;
    let cube_err = dist(c, [cube(r), cube(g), cube(b)]);
    let avg = (c[0] as i32 + c[1] as i32 + c[2] as i32) / 3;
    let gi = ((avg - 8).max(0) / 10).min(23);
    let gv = 8 + 10 * gi;
    if dist(c, [gv, gv, gv]) < cube_err {
        232 + gi as u8
    } else {
        cube_idx
    }
}

fn dist(c: [u8; 3], d: [i32; 3]) -> i32 {
    (0..3).map(|k| (c[k] as i32 - d[k]).pow(2)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmap_endpoints_and_middle() {
        assert_eq!(VIRIDIS.at(0.0), [0x44, 0x01, 0x54]);
        assert_eq!(VIRIDIS.at(1.0), [0xfd, 0xe7, 0x25]);
        assert_eq!(RDBU_R.at(0.5), [0xf7, 0xf7, 0xf7]);
        assert_eq!(GRAYS.at(0.5), [128, 128, 128]);
    }

    #[test]
    fn colorize_marks_missing() {
        let n = Norm { min: 0.0, max: 1.0 };
        let img = colorize(2, 1, &[f32::NAN, 1.0], &n, &GRAYS);
        assert_eq!(img.data, [48, 48, 48, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn symmetric_norm() {
        assert_eq!(
            Norm::symmetric(-2.0, 5.0),
            Norm {
                min: -5.0,
                max: 5.0
            }
        );
        assert_eq!(Norm { min: 1.0, max: 1.0 }.apply(1.0), 0.5);
    }

    #[test]
    fn ansi256_picks_cube_and_grey() {
        assert_eq!(ansi256([255, 0, 0]), 196);
        assert_eq!(ansi256([128, 128, 128]), 244);
        assert_eq!(ansi256([0, 0, 0]), 16);
    }
}
