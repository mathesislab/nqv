//! PNG output: maps and line plots with a title, axes, tick labels and a colour bar,
//! drawn with an 8x8 bitmap font at twice its size.

use font8x8::{BASIC_FONTS, LATIN_FONTS, UnicodeFonts};
use image::{Rgba as Px, RgbaImage};
use nqv_core::{ContourLine, Series, nice_ticks};
use nqv_render::{Cmap, Norm, Rgba};

use std::path::Path;

use anyhow::{Result, bail};
use nqv_core::{Kind, Segment, Show, fit_size};
use nqv_render::{CMAPS, colorize, draw_faint_lines, draw_lines, extends};

use crate::format::{dim_at, fmt};
use crate::session::Session;

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

impl Session {
    /// Write the current view as a PNG of at most `max` pixels: a map with a colour bar
    /// (data aspect kept, small grids scaled up), or a line plot. Returns a summary.
    pub fn write_png(&self, path: &Path, max: (u32, u32)) -> Result<String> {
        let o = self.overlays();
        if let Some(e) = &o.mask_error {
            bail!("mask: {e}");
        }
        if let Some(e) = &o.contour_error {
            bail!("contour: {e}");
        }
        let s = self;
        let v = s.var_info().clone();
        let src = s.var.source;
        let info = s.info();
        let label = |k: usize| {
            let d = &v.dims[k];
            match info.coord_var(src, d).and_then(|c| c.units.clone()) {
                Some(u) => format!("{} [{u}]", d.name),
                None => d.name.clone(),
            }
        };
        let title = format!(
            "{}:{}{}  {}",
            info.label(src),
            v.name,
            v.units
                .as_ref()
                .map(|u| format!(" [{u}]"))
                .unwrap_or_default(),
            v.long_name.as_deref().unwrap_or("")
        );
        let mut sub: Vec<String> = v
            .step_dims()
            .iter()
            .map(|&k| dim_at(&v.dims[k], s.index[k], s.coord_at(k, s.index[k])))
            .collect();
        if info.compare {
            sub.push(match info.show {
                Show::Diff => "A - B".into(),
                Show::A => "A".into(),
                Show::B => "B".into(),
            });
        }
        if !s.mask.is_empty() {
            let m: Vec<String> = s.mask.iter().map(|c| c.text()).collect();
            sub.push(format!("mask: {}", m.join(" and ")));
        }
        if let (Some(c), Kind::Map { .. }) = (&s.contour, v.kind()) {
            sub.push(format!("contours: {}", c.text()));
        }
        let subtitle = sub.join("   ");
        let cmap = &CMAPS[s.cmap];
        let img = match v.kind() {
            Kind::Map { y, x } => {
                let area = map_area(max);
                let size = s.map_size(area.0, area.1).unwrap_or_else(|| {
                    fit_size(v.dims[x].len.max(1), v.dims[y].len.max(1), area.0, area.1)
                });
                let m = s.map(size, false)?;
                // The sampled range, widened to every value drawn (it is all on the page).
                let (lo, hi) =
                    nqv_core::finite_range(&m.raster.values).unwrap_or((m.range.min, m.range.max));
                let norm = s.norm(m.range.min.min(lo), m.range.max.max(hi));
                let mut image = colorize(m.raster.w, m.raster.h, &m.raster.values, &norm, cmap);
                let grid = s.graticule((m.raster.w, m.raster.h));
                if let Some(g) = &grid {
                    let lines: Vec<Segment> = g
                        .meridians
                        .iter()
                        .chain(&g.parallels)
                        .flat_map(|l| l.segments.iter().copied())
                        .collect();
                    draw_faint_lines(&mut image, &lines);
                }
                let segments: Vec<Segment> = m
                    .contours
                    .iter()
                    .flat_map(|l| l.segments.iter().copied())
                    .collect();
                draw_lines(&mut image, &segments);
                // Round coordinate values (or 1-based indices) as ticks, placed linearly
                // between the cells shown at the two ends of each edge (regular grids).
                let at = |k: usize, i: usize| s.coord_at(k, i).unwrap_or((i + 1) as f32) as f64;
                let ticks = |n: u32, end: &dyn Fn(u32) -> Option<f64>| {
                    let (Some(a), Some(b)) = (end(0), end(n - 1)) else {
                        return vec![];
                    };
                    nqv_core::nice_ticks(a.min(b), a.max(b), 5)
                        .into_iter()
                        .map(|t| {
                            let p = if b == a {
                                0.0
                            } else {
                                (t - a) / (b - a) * (n - 1) as f64
                            };
                            (p.round() as u32, fmt(t as f32))
                        })
                        .collect::<Vec<_>>()
                };
                // Projected: longitudes where meridians leave the bottom edge, latitudes
                // where parallels leave the left one.
                let (xticks, yticks, xlabel, ylabel) = match &grid {
                    Some(g) => {
                        let (w, h) = (m.raster.w as f32, m.raster.h as f32);
                        let xs = edge_ticks(
                            &g.meridians,
                            |p| (p.1 - h).abs() < 1.5,
                            |p| p.0,
                            w,
                            'E',
                            'W',
                        );
                        let ys =
                            edge_ticks(&g.parallels, |p| p.0.abs() < 1.5, |p| p.1, h, 'N', 'S');
                        (xs, ys, String::new(), String::new())
                    }
                    None => (
                        ticks(m.raster.w, &|p| {
                            s.cell_at(&m.spec, (p, 0)).map(|c| at(x, c.1))
                        }),
                        ticks(m.raster.h, &|p| {
                            s.cell_at(&m.spec, (0, p)).map(|c| at(y, c.0))
                        }),
                        label(x),
                        label(y),
                    ),
                };
                render_map(&MapPlot {
                    title,
                    subtitle,
                    image: &image,
                    xticks,
                    yticks,
                    xlabel,
                    ylabel,
                    norm,
                    cmap,
                    contours: &m.contours,
                    extend: nqv_core::finite_range(&m.raster.values)
                        .map_or((false, false), |(lo, hi)| extends(&norm, lo, hi)),
                })
            }
            Kind::Series { along } => {
                let d = s.series_at(s.index.clone(), along, max.0 as usize)?;
                render_line(
                    &LinePlot {
                        title,
                        subtitle,
                        series: &d.series,
                        norm: d.norm,
                        xlabel: label(along),
                        ylabel: v.units.clone().unwrap_or_default(),
                    },
                    max,
                )
            }
            Kind::Scalar => bail!("--png: '{}' is a single value", v.name),
        };
        img.save_with_format(path, image::ImageFormat::Png)?;
        Ok(format!(
            "wrote {} ({}x{}): {}:{}",
            path.display(),
            img.width(),
            img.height(),
            info.label(src),
            v.name
        ))
    }
}

/// Room left for the map image in a PNG of at most `max` pixels.
pub fn map_area(max: (u32, u32)) -> (u32, u32) {
    (
        max.0.saturating_sub(LEFT + RIGHT).max(16),
        max.1.saturating_sub(TOP + BELOW_MAP).max(16),
    )
}

pub struct Canvas {
    pub img: RgbaImage,
    /// When set, drawing is limited to this rectangle (x0, y0, x1, y1), ends excluded.
    clip: Option<(i64, i64, i64, i64)>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self {
            img: RgbaImage::from_pixel(w, h, Px([255, 255, 255, 255])),
            clip: None,
        }
    }

    fn put(&mut self, x: i64, y: i64, c: Px<u8>) {
        let (x0, y0, x1, y1) =
            self.clip
                .unwrap_or((0, 0, self.img.width() as i64, self.img.height() as i64));
        if x >= x0.max(0)
            && y >= y0.max(0)
            && x < x1.min(self.img.width() as i64)
            && y < y1.min(self.img.height() as i64)
        {
            self.img.put_pixel(x as u32, y as u32, c);
        }
    }

    /// Filled triangle with corners `p`, and its outline in `edge`.
    fn triangle(&mut self, p: [(i64, i64); 3], fill: Px<u8>, edge: Px<u8>) {
        let (x0, x1) = (
            p.iter().map(|q| q.0).min().unwrap(),
            p.iter().map(|q| q.0).max().unwrap(),
        );
        let (y0, y1) = (
            p.iter().map(|q| q.1).min().unwrap(),
            p.iter().map(|q| q.1).max().unwrap(),
        );
        let side = |a: (i64, i64), b: (i64, i64), x: i64, y: i64| {
            (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
        };
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = [
                    side(p[0], p[1], x, y),
                    side(p[1], p[2], x, y),
                    side(p[2], p[0], x, y),
                ];
                if d.iter().all(|&v| v >= 0) || d.iter().all(|&v| v <= 0) {
                    self.put(x, y, fill);
                }
            }
        }
        for k in 0..3 {
            self.line_thin(p[k], p[(k + 1) % 3], edge);
        }
    }

    /// Line one pixel wide.
    fn line_thin(&mut self, (x0, y0): (i64, i64), (x1, y1): (i64, i64), c: Px<u8>) {
        let n = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for k in 0..=n {
            let x = x0 + (x1 - x0) * k / n;
            let y = y0 + (y1 - y0) * k / n;
            self.put(x, y, c);
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
        self.text_scaled(x, y, s, SCALE, c);
    }

    /// Text at `scale` times the 8x8 font.
    fn text_scaled(&mut self, x: i64, y: i64, s: &str, scale: u32, c: Px<u8>) {
        for (n, ch) in s.chars().enumerate() {
            let glyph = BASIC_FONTS
                .get(ch)
                .or_else(|| LATIN_FONTS.get(ch))
                .or_else(|| BASIC_FONTS.get('?'))
                .unwrap_or([0; 8]);
            let gx = x + (n as u32 * 8 * scale) as i64;
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (1 << col) != 0 {
                        for (i, j) in (0..scale).flat_map(|i| (0..scale).map(move |j| (i, j))) {
                            let px = gx + (col * scale + i) as i64;
                            let py = y + (row as u32 * scale + j) as i64;
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

/// Tick labels for graticule lines where they meet an edge (`at_edge` of a segment end),
/// at `along` of that point within `0..n`: degrees with `pos` or `neg` (70°N, 40°W).
fn edge_ticks(
    lines: &[ContourLine],
    at_edge: impl Fn((f32, f32)) -> bool,
    along: impl Fn((f32, f32)) -> f32,
    n: f32,
    pos: char,
    neg: char,
) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = vec![];
    for l in lines {
        let hit = l
            .segments
            .iter()
            .flat_map(|s| [s[0], s[1]])
            .find(|&p| at_edge(p) && (0.0..n).contains(&along(p)));
        if let Some(p) = hit {
            // longitudes within -180..180; latitudes as they are
            let v = match pos {
                'N' => l.level,
                _ => (l.level + 180.0).rem_euclid(360.0) - 180.0,
            };
            let side = match v {
                0.0 => String::new(),
                v if v.abs() == 180.0 => String::new(),
                v if v < 0.0 => neg.to_string(),
                _ => pos.to_string(),
            };
            out.push((
                along(p).round() as u32,
                format!("{}°{side}", fmt(v.abs() as f32)),
            ));
        }
    }
    out
}

fn text_width(s: &str) -> u32 {
    s.chars().count() as u32 * GLYPH
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
    /// Values fall below / above the colour range: triangles at the colour bar ends.
    pub extend: (bool, bool),
    /// Contour lines (already drawn into `image`), for their labels.
    pub contours: &'a [ContourLine],
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
    // Ticks every time; a label only clear of the one before.
    let mut right_end = i64::MIN;
    for (x, label) in &p.xticks {
        let x = (LEFT + x) as i64;
        c.rect(x as u32, TOP + h, 1, 6, BLACK);
        let left = x - text_width(label) as i64 / 2;
        if left > right_end + 8 {
            c.text_centred(x, (TOP + h + 8) as i64, label, BLACK);
            right_end = left + text_width(label) as i64;
        }
    }
    let mut last_y = i64::MIN;
    for (y, label) in &p.yticks {
        let y = TOP + y;
        c.rect(LEFT - 6, y, 6, 1, BLACK);
        if (y as i64 - last_y).abs() > (GLYPH + 4) as i64 {
            c.text_right((LEFT - 10) as i64, (y - GLYPH / 2) as i64, label, BLACK);
            last_y = y as i64;
        }
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
    let col = |t: f32| {
        let c = p.cmap.at(t);
        Px([c[0], c[1], c[2], 255])
    };
    let (top, bottom, mid) = (
        (bar - 1) as i64,
        (bar + GLYPH) as i64,
        (bar + GLYPH / 2) as i64,
    );
    if p.extend.0 {
        let x = LEFT as i64 - 1;
        c.triangle(
            [(x, top), (x - GLYPH as i64, mid), (x, bottom)],
            col(0.0),
            BLACK,
        );
    }
    if p.extend.1 {
        let x = (LEFT + w) as i64;
        c.triangle(
            [(x, top), (x + GLYPH as i64, mid), (x, bottom)],
            col(1.0),
            BLACK,
        );
    }
    let ly = (bar + GLYPH + 6) as i64;
    let mut right_end = i64::MIN;
    for (t, label) in bar_ticks(&p.norm) {
        let x = (LEFT as f32 + t * (w - 1) as f32).round() as i64;
        c.rect(x as u32, bar + GLYPH, 1, 4, BLACK);
        // Centred, but kept inside the bar's width and clear of the previous label.
        let tw = text_width(&label) as i64;
        // (a label wider than the bar starts at its left end)
        let right = ((LEFT + w) as i64 - tw).max(LEFT as i64);
        let left = (x - tw / 2).clamp(LEFT as i64, right);
        if left > right_end + 8 {
            c.text(left, ly, &label, BLACK);
            right_end = left + tw;
        }
    }
    for (x, y, label) in contour_labels(p.contours, w, h) {
        let tw = (label.chars().count() * 8) as i64;
        let (bx, by) = (
            (LEFT as f32 + x) as i64 - tw / 2 - 2,
            (TOP as f32 + y) as i64 - 6,
        );
        c.rect(
            bx.max(0) as u32,
            by.max(0) as u32,
            tw as u32 + 4,
            12,
            Px([255, 255, 255, 255]),
        );
        c.text_scaled(bx + 2, by + 2, &label, 1, BLACK);
    }
    c.img
}

/// Colour bar ticks as (position 0..1, label): round values on a linear scale, powers
/// of ten (with 2 and 5 times them over short ranges) on a log scale; the ends if none.
pub fn bar_ticks(n: &Norm) -> Vec<(f32, String)> {
    let (lo, hi) = (n.min as f64, n.max as f64);
    let values: Vec<f64> = if n.log {
        let decades = (lo.log10().floor() as i32)..=(hi.log10().floor() as i32);
        let mults: &[f64] = if hi / lo < 100.0 {
            &[1.0, 2.0, 5.0]
        } else {
            &[1.0]
        };
        decades
            .flat_map(|k| mults.iter().map(move |m| m * 10f64.powi(k)))
            .filter(|v| (lo..=hi).contains(v))
            .collect()
    } else {
        nice_ticks(lo, hi, 4)
    };
    let values = if values.len() < 2 {
        vec![lo, hi]
    } else {
        values
    };
    values
        .into_iter()
        .map(|v| (n.apply(v as f32), fmt(v as f32)))
        .collect()
}

/// Up to three labels per contour level, at segment midpoints inside the `w` by `h`
/// map and at least 120 pixels from any other label: (x, y, text).
pub fn contour_labels(lines: &[ContourLine], w: u32, h: u32) -> Vec<(f32, f32, String)> {
    let mut out: Vec<(f32, f32, String)> = vec![];
    for line in lines {
        let text = fmt(line.level as f32);
        let half = (text.chars().count() * 4 + 4) as f32;
        let mut n = 0;
        for [a, b] in &line.segments {
            let (x, y) = (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1));
            let inside = x > half && x < w as f32 - half && y > 8.0 && y < h as f32 - 8.0;
            let clear = out
                .iter()
                .all(|(ox, oy, _)| (ox - x).hypot(oy - y) >= 120.0);
            if inside && clear {
                out.push((x, y, text.clone()));
                n += 1;
                if n == 3 {
                    break;
                }
            }
        }
    }
    out
}

pub struct LinePlot<'a> {
    pub title: String,
    pub subtitle: String,
    pub series: &'a Series,
    /// y axis range and scale.
    pub norm: Norm,
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
    let (y0, y1) = widen(p.norm.min as f64, p.norm.max as f64);
    let px = |x: f64| LEFT as f64 + (x - x0) / (x1 - x0) * (w - 1) as f64;
    // Log scale: through the norm (values at or below zero are gaps); else linear.
    let py = |y: f64| match p.norm.log {
        true if y > 0.0 => TOP as f64 + (1.0 - p.norm.apply(y as f32) as f64) * (h - 1) as f64,
        true => f64::NAN,
        false => TOP as f64 + (y1 - y) / (y1 - y0) * (h - 1) as f64,
    };
    for t in nice_ticks(x0, x1, 6) {
        let x = px(t).round() as u32;
        c.rect(x, TOP, 1, h, LIGHT);
        c.rect(x, TOP + h, 1, 6, BLACK);
        c.text_centred(x as i64, (TOP + h + 8) as i64, &fmt(t as f32), BLACK);
    }
    let yticks: Vec<(f64, String)> = match p.norm.log {
        true => bar_ticks(&p.norm)
            .into_iter()
            .map(|(t, l)| (TOP as f64 + (1.0 - t as f64) * (h - 1) as f64, l))
            .collect(),
        false => nice_ticks(y0, y1, 6)
            .into_iter()
            .map(|t| (py(t), fmt(t as f32)))
            .collect(),
    };
    for (y, label) in yticks {
        let y = y.round() as u32;
        c.rect(LEFT, y, w, 1, LIGHT);
        c.rect(LEFT - 6, y, 6, 1, BLACK);
        c.text_right((LEFT - 10) as i64, (y - GLYPH / 2) as i64, &label, BLACK);
    }
    c.frame(LEFT, TOP, w, h, BLACK);
    // Points outside a fixed y range are cut at the axes box.
    c.clip = Some((LEFT as i64, TOP as i64, (LEFT + w) as i64, (TOP + h) as i64));
    let mut prev: Option<(i64, i64)> = None;
    for (&x, &y) in p.series.x.iter().zip(&p.series.y) {
        let (fx, fy) = (px(x as f64), py(y as f64));
        if !(fx.is_finite() && fy.is_finite()) {
            prev = None;
            continue;
        }
        let pt = (fx.round() as i64, fy.round() as i64);
        match prev {
            Some(q) => c.line(q, pt, LINE),
            None => c.line(pt, pt, LINE),
        }
        prev = Some(pt);
    }
    c.clip = None;
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
    fn colour_bar_ticks() {
        let t: Vec<String> = bar_ticks(&Norm::log(0.3, 3000.0))
            .into_iter()
            .map(|t| t.1)
            .collect();
        assert_eq!(t, ["1", "10", "100", "1000"]);
        let t: Vec<String> = bar_ticks(&Norm::linear(0.0, 300.0))
            .into_iter()
            .map(|t| t.1)
            .collect();
        assert_eq!(t, ["0", "100", "200", "300"]);
        let t: Vec<String> = bar_ticks(&Norm::linear(500.0, 2000.0))
            .into_iter()
            .map(|t| t.1)
            .collect();
        assert_eq!(t, ["500", "1000", "1500", "2000"]);
        let t: Vec<String> = bar_ticks(&Norm::log(4.3, 21.0))
            .into_iter()
            .map(|t| t.1)
            .collect();
        assert_eq!(t, ["5", "10", "20"]);
        let p = bar_ticks(&Norm::log(1.0, 100.0));
        assert!((p.iter().find(|t| t.1 == "10").unwrap().0 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn line_plot_stays_in_its_box() {
        let s = Series {
            x: vec![0.0, 1.0, 2.0],
            y: vec![0.0, 100.0, 0.0],
        };
        let p = LinePlot {
            title: String::new(),
            subtitle: String::new(),
            series: &s,
            norm: Norm::linear(0.0, 10.0),
            xlabel: String::new(),
            ylabel: String::new(),
        };
        let img = render_line(&p, (400, 300));
        // Nothing drawn in the line colour above the box.
        let above = (0..TOP - 2)
            .flat_map(|y| (LEFT..LEFT + 100).map(move |x| (x, y)))
            .any(|(x, y)| *img.get_pixel(x, y) == LINE);
        assert!(!above);
    }

    #[test]
    fn text_draws_pixels() {
        let mut c = Canvas::new(40, 20);
        c.text(0, 0, "A", BLACK);
        let dark = c.img.pixels().filter(|p| p.0[0] == 0).count();
        assert!(dark > 20);
    }
}
