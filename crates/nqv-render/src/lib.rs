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
    /// Logarithmic: equal ratios get equal colour steps (needs 0 < min < max).
    pub log: bool,
}

impl Norm {
    pub fn linear(min: f32, max: f32) -> Self {
        Self {
            min,
            max,
            log: false,
        }
    }

    /// Range symmetric around zero that covers `min` and `max`.
    pub fn symmetric(min: f32, max: f32) -> Self {
        let a = min.abs().max(max.abs());
        Self::linear(-a, a)
    }

    /// Logarithmic from `min` to `max`; linear when they are not both positive.
    pub fn log(min: f32, max: f32) -> Self {
        Self {
            min,
            max,
            log: min > 0.0 && max > min,
        }
    }

    /// Position of `v` in the range, 0 to 1. On a log scale values at or below zero
    /// count as the minimum.
    pub fn apply(&self, v: f32) -> f32 {
        if self.min.is_nan() || self.max.is_nan() || self.max <= self.min {
            return 0.5;
        }
        let t = if self.log {
            if v <= 0.0 {
                return 0.0;
            }
            (v.ln() - self.min.ln()) / (self.max.ln() - self.min.ln())
        } else {
            (v - self.min) / (self.max - self.min)
        };
        t.clamp(0.0, 1.0)
    }
}

/// Whether data from `lo` to `hi` fall below and above the range of `n`, so the colour
/// bar should show that its end colours are saturated.
pub fn extends(n: &Norm, lo: f32, hi: f32) -> (bool, bool) {
    let tol = 1e-6 * (n.max - n.min).abs();
    (lo < n.min - tol, hi > n.max + tol)
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
        let n = Norm::linear(0.0, 1.0);
        let img = colorize(2, 1, &[f32::NAN, 1.0], &n, &GRAYS);
        assert_eq!(img.data, [48, 48, 48, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn symmetric_norm() {
        assert_eq!(Norm::symmetric(-2.0, 5.0), Norm::linear(-5.0, 5.0));
        assert_eq!(Norm::linear(1.0, 1.0).apply(1.0), 0.5);
        let l = Norm::log(1.0, 100.0);
        assert!((l.apply(10.0) - 0.5).abs() < 1e-6);
        assert_eq!(l.apply(-3.0), 0.0);
        assert!(!Norm::log(0.0, 10.0).log);
    }

    #[test]
    fn saturated_ends() {
        let n = Norm::linear(0.0, 10.0);
        assert_eq!(extends(&n, 0.0, 10.0), (false, false));
        assert_eq!(extends(&n, -1.0, 12.0), (true, true));
        assert_eq!(extends(&Norm::log(1.0, 100.0), 0.0, 50.0), (true, false));
    }

    #[test]
    fn ansi256_picks_cube_and_grey() {
        assert_eq!(ansi256([255, 0, 0]), 196);
        assert_eq!(ansi256([128, 128, 128]), 244);
        assert_eq!(ansi256([0, 0, 0]), 16);
    }
}

/// A line segment from one point to another, in pixels (x right, y down).
pub type Segment = [(f32, f32); 2];

/// Relative luminance of an RGB colour, 0 to 1.
pub fn luminance(c: [u8; 3]) -> f32 {
    (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0
}

/// Black on light colours, white on dark ones.
pub fn contrast(c: [u8; 3]) -> [u8; 3] {
    if luminance(c) > 0.45 {
        [0, 0, 0]
    } else {
        [255, 255, 255]
    }
}

/// Pixels a segment passes through, one per step along its longer axis.
fn line_pixels(s: &Segment, mut put: impl FnMut(i64, i64)) {
    let [(x0, y0), (x1, y1)] = *s;
    let n = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as usize;
    for k in 0..=n {
        let t = k as f32 / n as f32;
        put(
            (x0 + t * (x1 - x0)).floor() as i64,
            (y0 + t * (y1 - y0)).floor() as i64,
        );
    }
}

/// Draw segments one pixel wide into `img`, each pixel black or white for contrast with
/// the colour under it.
pub fn draw_lines(img: &mut Rgba, segments: &[Segment]) {
    let (w, h) = (img.w as i64, img.h as i64);
    // Contrast with the map, not with lines already drawn (segments share pixels).
    let map = img.data.clone();
    for s in segments {
        line_pixels(s, |x, y| {
            if (0..w).contains(&x) && (0..h).contains(&y) {
                let i = 4 * (y * w + x) as usize;
                let c = contrast([map[i], map[i + 1], map[i + 2]]);
                img.data[i..i + 3].copy_from_slice(&c);
            }
        });
    }
}

/// Braille patterns for segments over a `w` by `h` pixel image drawn as half-blocks
/// (one cell = 1 x 2 pixels = 2 x 4 braille dots): one byte of dot bits per cell, row by
/// row (`w` cells per row, `h.div_ceil(2)` rows); 0 where no line passes. The character
/// is U+2800 plus the byte.
pub fn braille_cells(w: u32, h: u32, segments: &[Segment]) -> Vec<u8> {
    let rows = h.div_ceil(2) as i64;
    let (w, dw, dh) = (w as i64, 2 * w as i64, 4 * rows);
    let mut cells = vec![0u8; (w * rows) as usize];
    // Bit of dot (column, row) within a cell, per the Unicode braille layout.
    const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    for s in segments {
        // Two dots per pixel across, two per pixel down.
        let d = [(s[0].0 * 2.0, s[0].1 * 2.0), (s[1].0 * 2.0, s[1].1 * 2.0)];
        line_pixels(&d, |x, y| {
            if (0..dw).contains(&x) && (0..dh).contains(&y) {
                let cell = (y / 4 * w + x / 2) as usize;
                cells[cell] |= BITS[(x % 2) as usize][(y % 4) as usize];
            }
        });
    }
    cells
}

#[cfg(test)]
mod line_tests {
    use super::*;

    #[test]
    fn lines_contrast_with_the_background() {
        let mut img = colorize(4, 1, &[0.0, 0.0, 1.0, 1.0], &Norm::linear(0.0, 1.0), &GRAYS);
        // Two segments sharing their middle pixels: still white on black, black on white.
        draw_lines(
            &mut img,
            &[[(0.5, 0.5), (2.5, 0.5)], [(1.5, 0.5), (3.5, 0.5)]],
        );
        assert_eq!(&img.data[0..3], &[255, 255, 255]); // white on black
        assert_eq!(&img.data[12..15], &[0, 0, 0]); // black on white
    }

    #[test]
    fn braille_dots_follow_a_vertical_line() {
        // One cell (1 x 2 pixels): a vertical line at x = 0.25 sets the left dot column.
        let cells = braille_cells(1, 2, &[[(0.25, 0.0), (0.25, 1.99)]]);
        assert_eq!(cells, [0x01 | 0x02 | 0x04 | 0x40]);
        assert_eq!(braille_cells(2, 2, &[]), [0, 0]);
    }
}
