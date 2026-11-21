//! Viewer state and key handling.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use image::{DynamicImage, RgbaImage};
use nqv_core::{
    Catalog, Kind, RangeEstimate, Series, SeriesSpec, Show, VarInfo, VarRef, ViewSpec, cell_at,
    fit_size, netcdf_files, range, series, series_range, value, view,
};
use nqv_render::{CMAPS, Norm, Rgba, colorize};
use ratatui::DefaultTerminal;
use ratatui::layout::{Position, Rect, Size};
use ratatui_image::{Resize, protocol::Protocol};

use crate::Output;
use crate::browser::{Action, Browser};
use crate::fuzzy;
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

/// Everything a cached plot depends on.
type FrameKey = (VarRef, Vec<usize>, usize, bool, Show, Rect);

pub struct Frame {
    pub key: FrameKey,
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
    pub var: VarRef,
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
    /// Shown instead of the help line until the next key.
    pub message: Option<String>,
    /// File browser, drawn over everything else.
    pub browser: Option<Browser>,
    /// Set when the screen must be cleared before the next draw (image protocols
    /// leave images behind when the plot type changes).
    clear: bool,
}

impl App {
    pub fn new(cat: Catalog, var: Option<&str>) -> Result<Self> {
        let vars = cat.data_vars();
        let first = match var {
            Some(name) => match vars.iter().find(|(_, v)| v.name == name) {
                Some((r, _)) => r.clone(),
                None => bail!("no variable '{name}'"),
            },
            None => match default_var(&vars) {
                Some(r) => r.clone(),
                None => bail!("no data variables"),
            },
        };
        let message = match cat.skipped.len() {
            0 => None,
            n => Some(format!(
                "{n} file(s) not opened, e.g. {}: {}",
                cat.skipped[0].0.display(),
                cat.skipped[0].1
            )),
        };
        let (cmap, symmetric) = if cat.compare {
            colour_defaults(cat.show)
        } else {
            colour_defaults(Show::A)
        };
        let mut app = Self {
            cat,
            var: first.clone(),
            index: vec![],
            cmap,
            symmetric,
            picker: None,
            frame: None,
            probe: None,
            panel: None,
            message,
            browser: None,
            clear: false,
        };
        app.set_var(first);
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
                if let Some(b) = &self.browser {
                    let area = f.area();
                    b.draw(f, area);
                }
            })?;
            match next_event()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    self.message = None;
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
        if self.picker.is_some() || self.browser.is_some() {
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
        let src = self.var.source;
        let coord = |k: usize, i: usize| {
            self.cat
                .coord(src, &dims[k])
                .and_then(|c| c.get(i).copied())
        };
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
        if let Some(b) = &mut self.browser {
            match b.handle_key(code) {
                Action::None => {}
                Action::Close => {
                    self.browser = None;
                    self.clear = true;
                }
                Action::Open(paths) => self.open_paths(&paths),
            }
            return true;
        }
        if let Some(p) = &mut self.picker {
            let names = filtered_vars(&self.cat, &p.query);
            match code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Enter => {
                    if let Some(r) = names.get(p.sel).map(|(r, _)| r.clone()) {
                        self.set_var(r);
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
            KeyCode::Char('o') => {
                let file = self.source_path();
                self.browser = Some(Browser::new(&dir_of(&file), Some(&file)));
            }
            KeyCode::Char('O') if self.cat.compare => {
                self.message = Some("O: not in a comparison; use o".into())
            }
            KeyCode::Char('O') => self.open_paths(&[dir_of(&self.source_path())]),
            KeyCode::Char(']') => self.neighbour(1),
            KeyCode::Char('[') => self.neighbour(-1),
            KeyCode::Char('r') => self.symmetric = !self.symmetric,
            KeyCode::Char('d') if self.cat.compare => {
                self.cat.show = match self.cat.show {
                    Show::Diff => Show::A,
                    Show::A => Show::B,
                    Show::B => Show::Diff,
                };
                (self.cmap, self.symmetric) = colour_defaults(self.cat.show);
                self.probe = None;
                self.clear = true;
            }
            KeyCode::Char('d') => {
                self.message =
                    Some("d: start with --diff A B to compare two files or folders".into())
            }
            _ => {}
        }
        true
    }

    /// Absolute path of the current variable's (first) file.
    fn source_path(&self) -> PathBuf {
        let p = &self.cat.sources[self.var.source].path;
        std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())
    }

    /// Open `paths` as on the command line, keeping the variable where possible.
    /// On failure the message goes to the browser if it is open.
    fn open_paths(&mut self, paths: &[PathBuf]) {
        let res = Catalog::open(paths).and_then(|cat| self.switch(cat));
        if let Err(e) = res {
            let m = Some(format!("{e:#}"));
            match &mut self.browser {
                Some(b) => b.message = m,
                None => self.message = m,
            }
        }
    }

    /// Replace the catalog. The current variable stays (preferably from the same file)
    /// with its indices if the dimensions match, and so do the colours outside comparisons.
    fn switch(&mut self, cat: Catalog) -> Result<()> {
        let file = self.source_path();
        let mut new = App::new(cat, None)?;
        let same_file = |r: &VarRef| {
            std::fs::canonicalize(&new.cat.sources[r.source].path).is_ok_and(|p| p == file)
        };
        let candidates: Vec<VarRef> = new
            .cat
            .data_vars()
            .into_iter()
            .filter(|(_, v)| v.name == self.var.name)
            .map(|(r, _)| r)
            .collect();
        let keep = candidates
            .iter()
            .find(|r| same_file(r))
            .or(candidates.first())
            .cloned();
        if let Some(r) = keep {
            new.keep_var(r, self.var_info(), &self.index);
        }
        if !self.cat.compare && !new.cat.compare {
            (new.cmap, new.symmetric) = (self.cmap, self.symmetric);
        }
        new.clear = true;
        *self = new;
        Ok(())
    }

    /// Select `r`, keeping `index` when `r` has the same dimensions as `old`.
    fn keep_var(&mut self, r: VarRef, old: &VarInfo, index: &[usize]) {
        let same = self.cat.var(&r).is_ok_and(|v| v.dims == old.dims);
        self.set_var(r);
        if same {
            self.index = index.to_vec();
        }
    }

    /// Previous/next file: within a collection or folder comparison its sources, else
    /// the NetCDF files next to the current file. The variable stays where it exists.
    fn neighbour(&mut self, delta: isize) {
        let n = self.cat.sources.len();
        if n > 1 {
            let s = (self.var.source as isize + delta).rem_euclid(n as isize) as usize;
            let vars: Vec<(VarRef, &VarInfo)> = self
                .cat
                .data_vars()
                .into_iter()
                .filter(|(r, _)| r.source == s)
                .collect();
            let same = vars.iter().find(|(_, v)| v.name == self.var.name);
            let Some(r) = same.map(|(r, _)| r).or(default_var(&vars)).cloned() else {
                self.message = Some(format!("{}: no data variables", self.cat.label(s)));
                return;
            };
            let (old, index) = (self.var_info().clone(), self.index.clone());
            self.keep_var(r, &old, &index);
            return;
        }
        if self.cat.compare {
            self.message =
                Some("[ ]: comparing two files; compare folders to step through files".into());
            return;
        }
        let file = self.source_path();
        let files = netcdf_files(&dir_of(&file)).unwrap_or_default();
        let Some(pos) = files.iter().position(|f| f.file_name() == file.file_name()) else {
            return;
        };
        let next = (pos as isize + delta).rem_euclid(files.len() as isize) as usize;
        self.open_paths(&[files[next].clone()]);
    }

    /// File browser on its own, before anything is open; None when it is closed.
    pub fn pick(terminal: &mut DefaultTerminal, dir: &Path) -> Result<Option<App>> {
        let mut b = Browser::new(dir, None);
        loop {
            terminal.draw(|f| {
                let area = f.area();
                b.draw(f, area);
            })?;
            let Event::Key(k) = next_event()? else {
                continue;
            };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match b.handle_key(k.code) {
                Action::None => {}
                Action::Close => return Ok(None),
                Action::Open(paths) => {
                    match Catalog::open(&paths).and_then(|c| App::new(c, None)) {
                        Ok(app) => return Ok(Some(app)),
                        Err(e) => b.message = Some(format!("{e:#}")),
                    }
                }
            }
        }
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

    fn set_var(&mut self, r: VarRef) {
        let n = self.cat.var(&r).map(|v| v.dims.len()).unwrap_or(0);
        self.var = r;
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
            self.cat.show,
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
            self.cat.show,
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
                let (min, max) = series_range(&self.cat, &spec, &s).unwrap_or((f32::NAN, f32::NAN));
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

/// Folder of `file`, "." for a bare file name.
fn dir_of(file: &Path) -> PathBuf {
    match file.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Colour map index and symmetric range for what is shown: differences read best on a
/// diverging map around zero, plain fields on viridis with their own range.
fn colour_defaults(show: Show) -> (usize, bool) {
    match show {
        Show::Diff => (1, true),
        Show::A | Show::B => (0, false),
    }
}

/// First variable of the most common kind, maps winning ties: a time-series file
/// opens on a series even if it also holds a few maps.
fn default_var<'a>(vars: &'a [(VarRef, &VarInfo)]) -> Option<&'a VarRef> {
    let rank = |k: Kind| match k {
        Kind::Map { .. } => 0,
        Kind::Series { .. } => 1,
        Kind::Scalar => 2,
    };
    let mut counts = [0usize; 3];
    for (_, v) in vars {
        counts[rank(v.kind())] += 1;
    }
    let best = (0..3).max_by_key(|&r| (counts[r], 3 - r))?;
    vars.iter()
        .find(|(_, v)| rank(v.kind()) == best)
        .map(|(r, _)| r)
}

/// Data variables matching a picker query, best matches first (see `fuzzy`).
/// `file:var` limits the list to files whose label matches `file`.
pub fn filtered_vars<'a>(cat: &'a Catalog, query: &str) -> Vec<(VarRef, &'a VarInfo)> {
    let (file_q, var_q) = fuzzy::split(query);
    let mut hits: Vec<(u8, usize, (VarRef, &VarInfo))> = cat
        .data_vars()
        .into_iter()
        .filter(|(r, _)| file_q.is_none_or(|f| fuzzy::tier(f, &cat.label(r.source)).is_some()))
        .filter_map(|(r, v)| {
            let t = fuzzy::var_tier(var_q, &v.name, v.long_name.as_deref())?;
            // Shorter names first within a tier; file order when nothing is typed.
            let len = if var_q.is_empty() { 0 } else { v.name.len() };
            Some((t, len, (r, v)))
        })
        .collect();
    hits.sort_by_key(|h| (h.0, h.1));
    hits.into_iter().map(|h| h.2).collect()
}
