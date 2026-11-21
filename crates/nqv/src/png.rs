//! PNG output: maps and line plots with a title, axes, tick labels and a colour bar,
//! drawn with an 8x8 bitmap font at twice its size.

use font8x8::{BASIC_FONTS, LATIN_FONTS, UnicodeFonts};
use image::{Rgba as Px, RgbaImage};
use nqv_core::Series;
use nqv_render::{Cmap, Norm, Rgba};

use crate::ui::fmt;

/// Text scale: glyphs are 8 * SCALE pixels.
const SCALE: u32 = 2;
const GLYPH: u32 = 8 * SCALE;
const BLACK: Px<u8> = Px([0, 0, 0, 255]);
const GREY: Px<u8> = Px([110, 110, 110, 255]);
const LIGHT: Px<u8> = Px([200, 200, 200, 255]);
const LINE: Px<u8> = Px([33, 102, 172, 255]);
/// Margins around the plot area: left (y tick labels), right, top (two text lines),
/// and below a map (x tick labels, colour bar with its labels) or a line plot.
const LEFT: u32 = 120;
const RIGHT: u32 = 56;
/// Title, subtitle and the y axis name above the plot.
const TOP: u32 = 92;
const BELOW_MAP: u32 = 104;
const BELOW_LINE: u32 = 72;

/// Room left for the map image in a PNG of at most `max` pixels.
pub fn map_area(max: (u32, u32)) -> (u32, u32) {
    (
        max.0.saturating_sub(LEFT + RIGHT).max(16),
        max.1.saturating_sub(TOP + BELOW_MAP).max(16),
    )
}

pub struct Canvas {
    pub img: RgbaImage,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self {
            img: RgbaImage::from_pixel(w, h, Px([255, 255, 255, 255])),
        }
    }

    fn put(&mut self, x: i64, y: i64, c: Px<u8>) {
        if x >= 0 && y >= 0 && (x as u32) < self.img.width() && (y as u32) < self.img.height() {
            self.img.put_pixel(x as u32, y as u32, c);
        }
    }

    fn rect(&mut self, x: u32, y: u32, w: u32, h: u32, c: Px<u8>) {
        for j in y..y + h {
            for i in x..x + w {
                self.put(i as i64, j as i64, c);
            }
        }
    }

    /// Outline of the rectangle just outside (x, y, w, h).
    fn frame(&mut self, x: u32, y: u32, w: u32, h: u32, c: Px<u8>) {
        self.rect(x - 1, y - 1, w + 2, 1, c);
        self.rect(x - 1, y + h, w + 2, 1, c);
        self.rect(x - 1, y - 1, 1, h + 2, c);
        self.rect(x + w, y - 1, 1, h + 2, c);
    }

    /// Line two pixels wide (Bresenham, doubled vertically).
    fn line(&mut self, (x0, y0): (i64, i64), (x1, y1): (i64, i64), c: Px<u8>) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.put(x, y, c);
            self.put(x, y + 1, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Text with its top-left corner at (x, y); unknown characters show as '?'.
    fn text(&mut self, x: i64, y: i64, s: &str, c: Px<u8>) {
        for (n, ch) in s.chars().enumerate() {
            let glyph = BASIC_FONTS
                .get(ch)
                .or_else(|| LATIN_FONTS.get(ch))
                .or_else(|| BASIC_FONTS.get('?'))
                .unwrap_or([0; 8]);
            let gx = x + (n as u32 * GLYPH) as i64;
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (1 << col) != 0 {
                        for (i, j) in (0..SCALE).flat_map(|i| (0..SCALE).map(move |j| (i, j))) {
                            let px = gx + (col * SCALE + i) as i64;
                            let py = y + (row as u32 * SCALE + j) as i64;
                            self.put(px, py, c);
                        }
                    }
                }
            }
        }
    }

    /// Text ending at x (right-aligned) or centred on x.
    fn text_right(&mut self, x: i64, y: i64, s: &str, c: Px<u8>) {
        self.text(x - text_width(s) as i64, y, s, c);
    }

    fn text_centred(&mut self, x: i64, y: i64, s: &str, c: Px<u8>) {
        self.text(x - text_width(s) as i64 / 2, y, s, c);
    }

    /// Title and subtitle lines at the top, cut to the image width.
    fn titles(&mut self, title: &str, subtitle: &str) {
        let fit = |s: &str, w: u32| -> String {
            let n = (w.saturating_sub(16) / GLYPH) as usize;
            s.chars().take(n).collect()
        };
        let w = self.img.width();
        self.text(8, 8, &fit(title, w), BLACK);
        self.text(8, (8 + GLYPH + 6) as i64, &fit(subtitle, w), GREY);
    }
}

fn text_width(s: &str) -> u32 {
    s.chars().count() as u32 * GLYPH
}

/// About `n` round tick values (1, 2 or 5 times a power of ten apart) within [lo, hi].
pub fn nice_ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    if !(lo.is_finite() && hi.is_finite()) || hi <= lo || n == 0 {
        return vec![];
    }
    let raw = (hi - lo) / n as f64;
    let mag = 10f64.powf(raw.log10().floor());
    // The round step closest to the raw one (on a log scale).
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .min_by(|a, b| (a / raw).ln().abs().total_cmp(&(b / raw).ln().abs()))
        .unwrap_or(mag);
    let first = (lo / step).ceil() as i64;
    let last = (hi / step).floor() as i64;
    (first..=last).map(|k| k as f64 * step).collect()
}

/// A map image with tick labels (pixel position, text) along its edges.
pub struct MapPlot<'a> {
    pub title: String,
    pub subtitle: String,
    pub image: &'a Rgba,
    pub xticks: Vec<(u32, String)>,
    pub yticks: Vec<(u32, String)>,
    pub xlabel: String,
    pub ylabel: String,
    pub norm: Norm,
    pub cmap: &'a Cmap,
}

pub fn render_map(p: &MapPlot) -> RgbaImage {
    let (w, h) = (p.image.w, p.image.h);
    let mut c = Canvas::new(LEFT + w + RIGHT, TOP + h + BELOW_MAP);
    c.titles(&p.title, &p.subtitle);
    for (i, px) in p.image.data.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        c.img.put_pixel(LEFT + x, TOP + y, Px(*px));
    }
    c.frame(LEFT, TOP, w, h, BLACK);
    for (x, label) in &p.xticks {
        let x = (LEFT + x) as i64;
        c.rect(x as u32, TOP + h, 1, 6, BLACK);
        c.text_centred(x, (TOP + h + 8) as i64, label, BLACK);
    }
    for (y, label) in &p.yticks {
        let y = TOP + y;
        c.rect(LEFT - 6, y, 6, 1, BLACK);
        c.text_right((LEFT - 10) as i64, (y - GLYPH / 2) as i64, label, BLACK);
    }
    c.text_right(
        (LEFT + w) as i64,
        (TOP + h + 8 + GLYPH) as i64,
        &p.xlabel,
        GREY,
    );
    c.text(8, (TOP - GLYPH - 6) as i64, &p.ylabel, GREY);
    // Colour bar across the map width, with its range below.
    let bar = TOP + h + 2 * GLYPH + 20;
    for x in 0..w {
        let col = p.cmap.at((x as f32 + 0.5) / w as f32);
        c.rect(LEFT + x, bar, 1, GLYPH, Px([col[0], col[1], col[2], 255]));
    }
    c.frame(LEFT, bar, w, GLYPH, BLACK);
    let ly = (bar + GLYPH + 6) as i64;
    let mid = 0.5 * (p.norm.min + p.norm.max);
    c.text(LEFT as i64, ly, &fmt(p.norm.min), BLACK);
    c.text_centred((LEFT + w / 2) as i64, ly, &fmt(mid), BLACK);
    c.text_right((LEFT + w) as i64, ly, &fmt(p.norm.max), BLACK);
    c.img
}

pub struct LinePlot<'a> {
    pub title: String,
    pub subtitle: String,
    pub series: &'a Series,
    /// y axis range.
    pub y: (f32, f32),
    pub xlabel: String,
    pub ylabel: String,
}

pub fn render_line(p: &LinePlot, size: (u32, u32)) -> RgbaImage {
    let (w, h) = (
        size.0.saturating_sub(LEFT + RIGHT).max(16),
        size.1.saturating_sub(TOP + BELOW_LINE).max(16),
    );
    let mut c = Canvas::new(LEFT + w + RIGHT, TOP + h + BELOW_LINE);
    c.titles(&p.title, &p.subtitle);
    let xr = nqv_core::finite_range(&p.series.x).unwrap_or((0.0, 1.0));
    let (x0, x1) = widen(xr.0 as f64, xr.1 as f64);
    let (y0, y1) = widen(p.y.0 as f64, p.y.1 as f64);
    let px = |x: f64| LEFT as f64 + (x - x0) / (x1 - x0) * (w - 1) as f64;
    let py = |y: f64| TOP as f64 + (y1 - y) / (y1 - y0) * (h - 1) as f64;
    for t in nice_ticks(x0, x1, 6) {
        let x = px(t).round() as u32;
        c.rect(x, TOP, 1, h, LIGHT);
        c.rect(x, TOP + h, 1, 6, BLACK);
        c.text_centred(x as i64, (TOP + h + 8) as i64, &fmt(t as f32), BLACK);
    }
    for t in nice_ticks(y0, y1, 6) {
        let y = py(t).round() as u32;
        c.rect(LEFT, y, w, 1, LIGHT);
        c.rect(LEFT - 6, y, 6, 1, BLACK);
        c.text_right(
            (LEFT - 10) as i64,
            (y - GLYPH / 2) as i64,
            &fmt(t as f32),
            BLACK,
        );
    }
    c.frame(LEFT, TOP, w, h, BLACK);
    let mut prev: Option<(i64, i64)> = None;
    for (&x, &y) in p.series.x.iter().zip(&p.series.y) {
        if !(x.is_finite() && y.is_finite()) {
            prev = None;
            continue;
        }
        let pt = (px(x as f64).round() as i64, py(y as f64).round() as i64);
        match prev {
            Some(q) => c.line(q, pt, LINE),
            None => c.line(pt, pt, LINE),
        }
        prev = Some(pt);
    }
    c.text_right(
        (LEFT + w) as i64,
        (TOP + h + 8 + GLYPH) as i64,
        &p.xlabel,
        GREY,
    );
    c.text(8, (TOP - GLYPH - 6) as i64, &p.ylabel, GREY);
    c.img
}

/// A range widened when empty or a single value, like the terminal line plots.
fn widen(a: f64, b: f64) -> (f64, f64) {
    match (a, b) {
        (a, b) if a.is_finite() && b.is_finite() && b > a => (a, b),
        (a, _) if a.is_finite() => {
            let d = if a == 0.0 { 1.0 } else { 0.01 * a.abs() };
            (a - d, a + d)
        }
        _ => (0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_round() {
        assert_eq!(nice_ticks(0.0, 10.0, 5), [0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        assert_eq!(
            nice_ticks(-21000.0, 0.0, 4),
            [-20000.0, -15000.0, -10000.0, -5000.0, 0.0]
        );
        let t = nice_ticks(41.85, 42.85, 5);
        let want = [42.0, 42.2, 42.4, 42.6, 42.8];
        assert_eq!(t.len(), want.len());
        assert!(
            t.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-9),
            "{t:?}"
        );
        assert!(nice_ticks(1.0, 1.0, 5).is_empty());
    }

    #[test]
    fn text_draws_pixels() {
        let mut c = Canvas::new(40, 20);
        c.text(0, 0, "A", BLACK);
        let dark = c.img.pixels().filter(|p| p.0[0] == 0).count();
        assert!(dark > 20);
    }
}
