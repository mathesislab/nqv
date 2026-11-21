//! Viewer state and key handling.

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use image::{DynamicImage, RgbaImage};
use nqv_core::{
    Catalog, Kind, RangeEstimate, Series, SeriesSpec, VarInfo, ViewSpec, cell_at, finite_range,
    fit_size, range, series, value, view,
};
use nqv_render::{CMAPS, Norm, Rgba, colorize};
use ratatui::DefaultTerminal;
use ratatui::layout::{Position, Rect, Size};
use ratatui_image::{Resize, protocol::Protocol};

use crate::Output;
use crate::ui;

/// What the main area shows for one state and screen area.
pub enum Plot {
    Map(Map),
    Line(Series),
    Scalar(f32),
    Error(String),
}

/// A map image, the view it shows and the screen cells it covers.
pub struct Map {
    pub image: MapImage,
    pub spec: ViewSpec,
    pub rect: Rect,
}

pub enum MapImage {
    /// Encoded for an image protocol.
    Pixels(Protocol),
    /// Drawn as half-blocks, two pixels per cell.
    Blocks(Rgba),
}

pub struct Frame {
    pub key: (String, Vec<usize>, usize, bool, Rect),
    pub plot: Plot,
    pub range: Option<RangeEstimate>,
    pub norm: Option<Norm>,
}

/// A clicked map cell.
pub struct Probe {
    /// Indices of the cell in the map dimensions (y, x).
    pub cell: (usize, usize),
    /// Coordinate values of the cell, where the dimensions have coordinate variables.
    pub coords: (Option<f32>, Option<f32>),
    /// Value at the cell, read at the indices `at`.
    pub value: Result<f32, String>,
    at: Vec<usize>,
}

/// Variable picker: query typed so far and highlighted row.
pub struct VarPicker {
    pub query: String,
    pub sel: usize,
}

pub struct App {
    pub cat: Catalog,
    pub var: String,
    /// One index per dimension of `var`; entries at shown dimensions are unused.
    pub index: Vec<usize>,
    pub cmap: usize,
    pub symmetric: bool,
    pub picker: Option<VarPicker>,
    pub frame: Option<Frame>,
    /// Clicked map cell, if any.
    pub probe: Option<Probe>,
    /// Series at the clicked cell, shown below the map.
    pub panel: Option<Frame>,
    /// Set when the screen must be cleared before the next draw (image protocols
    /// leave images behind when the plot type changes).
    clear: bool,
}

impl App {
    pub fn new(cat: Catalog, var: Option<&str>) -> Result<Self> {
        let first = match var {
            Some(v) => cat.var(v)?.name.clone(),
            None => match default_var(&cat) {
                Some(v) => v.name.clone(),
                None => bail!("{}: no data variables", cat.path.display()),
            },
        };
        let mut app = Self {
            cat,
            var: String::new(),
            index: vec![],
            cmap: 0,
            symmetric: false,
            picker: None,
            frame: None,
            probe: None,
            panel: None,
            clear: false,
        };
        app.set_var(&first);
        Ok(app)
    }

    pub fn var_info(&self) -> &VarInfo {
        self.cat.var(&self.var).expect("current variable exists")
    }

    /// Dimension of the series shown for a clicked cell: the one ←→ steps.
    pub fn series_dim(&self) -> Option<usize> {
        self.var_info().step_dims().first().copied()
    }

    /// Whether the series at the clicked cell is shown below the map.
    pub fn panel_open(&self) -> bool {
        self.probe.is_some() && self.series_dim().is_some()
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal, out: Output) -> Result<()> {
        loop {
            if std::mem::take(&mut self.clear) {
                terminal.clear()?;
            }
            terminal.draw(|f| {
                let areas = ui::layout(f.area(), self.panel_open());
                self.update_frame(&out, areas.plot);
                self.update_probe(areas.panel);
                ui::draw(f, self, &out, &areas);
            })?;
            match next_event()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if !self.handle_key(k.code) {
                        return Ok(());
                    }
                }
                Event::Mouse(m) => self.click(&out, m.column, m.row),
                Event::Resize(..) => self.clear = true,
                _ => {}
            }
        }
    }

    /// Probe the map cell under screen cell (`col`, `row`); clicks beside the map do nothing.
    fn click(&mut self, out: &Output, col: u16, row: u16) {
        if self.picker.is_some() {
            return;
        }
        let Some(Frame {
            plot: Plot::Map(m), ..
        }) = &self.frame
        else {
            return;
        };
        let Some(cell) =
            pixel_at(out, m.rect, col, row).and_then(|px| cell_at(&self.cat, &m.spec, px))
        else {
            return;
        };
        let Kind::Map { y, x } = self.var_info().kind() else {
            return;
        };
        let dims = &self.var_info().dims;
        let coord = |k: usize, i: usize| self.cat.coord(&dims[k]).and_then(|c| c.get(i).copied());
        let coords = (coord(y, cell.0), coord(x, cell.1));
        let at = self.index_at(cell);
        let value = self.value_at(&at);
        // Opening the panel shrinks the map; image protocols would leave the old one behind.
        let was_open = self.panel_open();
        self.probe = Some(Probe {
            cell,
            coords,
            value,
            at,
        });
        self.clear |= self.panel_open() != was_open;
    }

    fn close_probe(&mut self) {
        self.clear |= self.panel_open();
        self.probe = None;
    }

    /// Returns false to quit.
    fn handle_key(&mut self, code: KeyCode) -> bool {
        if let Some(p) = &mut self.picker {
            let names = filtered_vars(&self.cat, &p.query);
            match code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Enter => {
                    if let Some(name) = names.get(p.sel).map(|v| v.name.clone()) {
                        self.set_var(&name);
                    }
                    self.picker = None;
                }
                KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                KeyCode::Down => p.sel = (p.sel + 1).min(names.len().saturating_sub(1)),
                KeyCode::Backspace => {
                    p.query.pop();
                    p.sel = 0;
                }
                KeyCode::Char(c) => {
                    p.query.push(c);
                    p.sel = 0;
                }
                _ => {}
            }
            // The picker overlays the plot; redraw everything when it closes.
            self.clear |= self.picker.is_none();
            return true;
        }
        match code {
            KeyCode::Esc if self.probe.is_some() => self.close_probe(),
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Right => self.step(0, 1),
            KeyCode::Left => self.step(0, -1),
            KeyCode::Up => self.step(1, 1),
            KeyCode::Down => self.step(1, -1),
            KeyCode::Home => self.step(0, isize::MIN),
            KeyCode::End => self.step(0, isize::MAX),
            KeyCode::Char('v') => {
                self.picker = Some(VarPicker {
                    query: String::new(),
                    sel: 0,
                })
            }
            KeyCode::Char('c') => self.cmap = (self.cmap + 1) % CMAPS.len(),
            KeyCode::Char('r') => self.symmetric = !self.symmetric,
            _ => {}
        }
        true
    }

    /// Move the `k`-th steppable dimension by `delta`, clamped to its length.
    fn step(&mut self, k: usize, delta: isize) {
        let v = self.var_info();
        let Some(&d) = v.step_dims().get(k) else {
            return;
        };
        let len = v.dims[d].len;
        let i = self.index[d] as isize;
        self.index[d] = i
            .saturating_add(delta)
            .clamp(0, len.saturating_sub(1) as isize) as usize;
    }

    fn set_var(&mut self, name: &str) {
        let n = self.cat.var(name).map(|v| v.dims.len()).unwrap_or(0);
        self.var = name.to_string();
        self.index = vec![0; n];
        self.probe = None;
        self.clear = true;
    }

    /// Current indices with the map dimensions set to `cell`.
    fn index_at(&self, cell: (usize, usize)) -> Vec<usize> {
        let mut index = self.index.clone();
        if let Kind::Map { y, x } = self.var_info().kind() {
            (index[y], index[x]) = cell;
        }
        index
    }

    fn value_at(&self, index: &[usize]) -> Result<f32, String> {
        value(&self.cat, &self.var, index).map_err(|e| format!("{e:#}"))
    }

    /// Re-read the value at the clicked cell when the indices changed, and recompute
    /// the series below the map when it or its `area` changed.
    fn update_probe(&mut self, area: Option<Rect>) {
        let Some(cell) = self.probe.as_ref().map(|p| p.cell) else {
            self.panel = None;
            return;
        };
        let at = self.index_at(cell);
        if self.probe.as_ref().is_some_and(|p| p.at != at) {
            let value = self.value_at(&at);
            if let Some(p) = &mut self.probe {
                (p.at, p.value) = (at.clone(), value);
            }
        }
        let (Some(area), Some(along)) = (area, self.series_dim()) else {
            self.panel = None;
            return;
        };
        // The series does not depend on the step along it.
        let mut index = at;
        index[along] = 0;
        let key = (
            self.var.clone(),
            index.clone(),
            self.cmap,
            self.symmetric,
            area,
        );
        if self.panel.as_ref().is_some_and(|f| f.key == key) {
            return;
        }
        let (plot, range, norm) = self.line(area, along, index);
        self.panel = Some(Frame {
            key,
            plot,
            range,
            norm,
        });
    }

    /// Recompute the plot when the state or the area changed.
    fn update_frame(&mut self, out: &Output, area: Rect) {
        let key = (
            self.var.clone(),
            self.index.clone(),
            self.cmap,
            self.symmetric,
            area,
        );
        if self.frame.as_ref().is_some_and(|f| f.key == key) {
            return;
        }
        let (plot, range, norm) = match self.var_info().kind() {
            Kind::Map { y, x } => self.map(out, area, y, x),
            Kind::Series { along } => self.line(area, along, self.index.clone()),
            Kind::Scalar => match value(&self.cat, &self.var, &self.index) {
                Ok(v) => (Plot::Scalar(v), None, None),
                Err(e) => (Plot::Error(format!("{e:#}")), None, None),
            },
        };
        self.frame = Some(Frame {
            key,
            plot,
            range,
            norm,
        });
    }

    fn norm(&self, min: f32, max: f32) -> Norm {
        if self.symmetric {
            Norm::symmetric(min, max)
        } else {
            Norm { min, max }
        }
    }

    fn map(
        &self,
        out: &Output,
        area: Rect,
        y: usize,
        x: usize,
    ) -> (Plot, Option<RangeEstimate>, Option<Norm>) {
        let dims = &self.var_info().dims;
        let (nx, ny) = (dims[x].len.max(1), dims[y].len.max(1));
        let (max_w, max_h) = match out {
            Output::Pixels(p) => {
                let fs = p.font_size();
                (
                    area.width as u32 * fs.width as u32,
                    area.height as u32 * fs.height as u32,
                )
            }
            Output::Blocks { .. } => (area.width as u32, area.height as u32 * 2),
        };
        let spec = ViewSpec {
            var: self.var.clone(),
            index: self.index.clone(),
            size: fit_size(nx, ny, max_w, max_h),
        };
        match view(&self.cat, &spec).and_then(|r| Ok((r, range(&self.cat, &spec)?))) {
            Err(e) => (Plot::Error(format!("{e:#}")), None, None),
            Ok((raster, rg)) => {
                let norm = self.norm(rg.min, rg.max);
                let rgba = colorize(raster.w, raster.h, &raster.values, &norm, &CMAPS[self.cmap]);
                let (w, h) = (rgba.w, rgba.h);
                let (image, rect) = match out {
                    Output::Blocks { .. } => (
                        MapImage::Blocks(rgba),
                        ui::centered(area, w as u16, h.div_ceil(2) as u16),
                    ),
                    Output::Pixels(p) => {
                        let fs = p.font_size();
                        let cols = w.div_ceil(fs.width as u32) as u16;
                        let rows = h.div_ceil(fs.height as u32) as u16;
                        match pixels(p, rgba, area) {
                            Ok(proto) => (MapImage::Pixels(proto), ui::centered(area, cols, rows)),
                            Err(e) => return (Plot::Error(e), None, None),
                        }
                    }
                };
                let plot = Plot::Map(Map { image, spec, rect });
                (plot, Some(rg), Some(norm))
            }
        }
    }

    /// Series along `along` at `index`, sampled for the width of `area`.
    fn line(
        &self,
        area: Rect,
        along: usize,
        index: Vec<usize>,
    ) -> (Plot, Option<RangeEstimate>, Option<Norm>) {
        // Braille gives two points per cell column.
        let spec = SeriesSpec {
            var: self.var.clone(),
            along,
            index,
            max_points: 2 * area.width as usize,
        };
        match series(&self.cat, &spec) {
            Err(e) => (Plot::Error(format!("{e:#}")), None, None),
            Ok(s) => {
                let (min, max) = finite_range(&s.y).unwrap_or((f32::NAN, f32::NAN));
                let exact = s.y.len() == self.var_info().dims[along].len;
                let norm = self.norm(min, max);
                (
                    Plot::Line(s),
                    Some(RangeEstimate { min, max, exact }),
                    Some(norm),
                )
            }
        }
    }
}

fn pixels(p: &ratatui_image::picker::Picker, rgba: Rgba, area: Rect) -> Result<Protocol, String> {
    let Some(img) = RgbaImage::from_raw(rgba.w, rgba.h, rgba.data) else {
        return Err("image buffer size mismatch".into());
    };
    let size = Size::new(area.width, area.height);
    p.new_protocol(DynamicImage::ImageRgba8(img), size, Resize::Fit(None))
        .map_err(|e| format!("image encoding failed: {e}"))
}

/// Next event that may change the view: mouse events other than a left click are skipped.
fn next_event() -> Result<Event> {
    loop {
        match event::read()? {
            Event::Mouse(m) if m.kind != MouseEventKind::Down(MouseButton::Left) => {}
            e => return Ok(e),
        }
    }
}

/// Pixel of the map image under screen cell (`col`, `row`), if that cell is in `rect`:
/// the top pixel of a half-block cell, the centre pixel of a cell for image protocols.
fn pixel_at(out: &Output, rect: Rect, col: u16, row: u16) -> Option<(u32, u32)> {
    if !rect.contains(Position::new(col, row)) {
        return None;
    }
    let (c, r) = ((col - rect.x) as u32, (row - rect.y) as u32);
    Some(match out {
        Output::Blocks { .. } => (c, 2 * r),
        Output::Pixels(p) => {
            let fs = p.font_size();
            let (fw, fh) = (fs.width as u32, fs.height as u32);
            (c * fw + fw / 2, r * fh + fh / 2)
        }
    })
}

/// First variable of the most common kind, maps winning ties: a time-series file
/// opens on a series even if it also holds a few maps.
fn default_var(cat: &Catalog) -> Option<&VarInfo> {
    let vars = cat.data_vars();
    let rank = |k: Kind| match k {
        Kind::Map { .. } => 0,
        Kind::Series { .. } => 1,
        Kind::Scalar => 2,
    };
    let mut counts = [0usize; 3];
    for v in &vars {
        counts[rank(v.kind())] += 1;
    }
    let best = (0..3).max_by_key(|&r| (counts[r], 3 - r))?;
    vars.into_iter().find(|v| rank(v.kind()) == best)
}

/// Data variables whose name or long name contains `query` (case-insensitive).
pub fn filtered_vars<'a>(cat: &'a Catalog, query: &str) -> Vec<&'a VarInfo> {
    let q = query.to_lowercase();
    cat.data_vars()
        .into_iter()
        .filter(|v| {
            v.name.to_lowercase().contains(&q)
                || v.long_name
                    .as_ref()
                    .is_some_and(|l| l.to_lowercase().contains(&q))
        })
        .collect()
}
