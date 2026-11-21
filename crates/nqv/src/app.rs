//! Viewer state and key handling.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use image::{DynamicImage, RgbaImage};
use nqv_core::{
    Catalog, Cond, ContourLine, ContourSpec, Interp, Kind, Mask, RangeEstimate, Segment, Series,
    SeriesSpec, Show, Stats, VarInfo, VarRef, ViewSpec, cell_at, check_contour, check_mask,
    contours, fit_size, masked, netcdf_files, range, series, series_range, stats, value, view,
};
use nqv_render::{CMAPS, Norm, Rgba, braille_cells, cmap_index, colorize, draw_lines, extends};
use ratatui::DefaultTerminal;
use ratatui::layout::{Position, Rect, Size};
use ratatui_image::{Resize, protocol::Protocol};

use crate::Output;
use crate::browser::{Action, Browser};
use crate::png;
use crate::ui;
use nqv_session::command::{self, Command, ContourCmd, MaskCond, RangeMode, TimeSel};
use nqv_session::fuzzy;
use nqv_session::recipe;

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
    /// Statistics of the slice, and where to show them (beside the map, when it leaves
    /// room).
    pub stats: Option<(Stats, Rect)>,
}

pub enum MapImage {
    /// Encoded for an image protocol.
    Pixels(Protocol),
    /// Drawn as half-blocks, two pixels per cell; cells crossed by contour lines show
    /// these braille patterns instead (0: none).
    Blocks { img: Rgba, braille: Vec<u8> },
}

/// Everything a cached plot depends on.
type FrameKey = (
    VarRef,
    Vec<usize>,
    usize,
    RangeMode,
    Show,
    Mask,
    Option<ContourSpec>,
    Interp,
    Rect,
);

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

/// How the current files were opened, for the command line that reproduces the view.
#[derive(Debug, Clone, Default)]
pub struct Origin {
    pub paths: Vec<PathBuf>,
    pub diff: bool,
    pub join: bool,
    pub join_dim: Option<String>,
    /// Side B of a comparison given with `--vs`.
    pub vs: Vec<PathBuf>,
}

pub struct App {
    pub origin: Origin,
    /// Key and command overview, drawn over everything else.
    pub help: bool,
    /// Colour-blind mode: `c` cycles only colour maps that suit colour vision deficiencies.
    pub cvd: bool,
    /// Resampling of maps to the screen: nearest cell, or bilinear when upsampling.
    pub interp: Interp,
    pub cat: Catalog,
    pub var: VarRef,
    /// One index per dimension of `var`; entries at shown dimensions are unused.
    pub index: Vec<usize>,
    pub cmap: usize,
    pub range_mode: RangeMode,
    /// Mask conditions as typed (`:mask`), resolved to `active_mask` for each draw.
    pub mask: Vec<MaskCond>,
    /// The mask applied to the current variable; empty when `mask_error` says why not.
    pub active_mask: Mask,
    pub mask_error: Option<String>,
    /// Contours as typed (`:contour`), resolved to `active_contour` for each draw.
    pub contour: Option<ContourCmd>,
    pub active_contour: Option<ContourSpec>,
    pub contour_error: Option<String>,
    pub picker: Option<VarPicker>,
    /// Text typed after `:`, while the command line is open.
    pub cmdline: Option<String>,
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
            Some(q) => match filtered_vars(&cat, q).first() {
                Some((r, _)) => r.clone(),
                None => bail!("no variable matching '{q}'"),
            },
            None => match default_var(&vars) {
                Some(r) => r.clone(),
                None => bail!("no data variables"),
            },
        };
        let message = match (cat.notes.first(), cat.skipped.first()) {
            (Some(note), _) => Some(format!("{note} ({} warning(s))", cat.notes.len())),
            (None, Some((path, why))) => Some(format!(
                "{} file(s) not opened, e.g. {}: {why}",
                cat.skipped.len(),
                path.display()
            )),
            (None, None) => None,
        };
        let (cmap, range_mode) = if cat.compare {
            colour_defaults(cat.show)
        } else {
            colour_defaults(Show::A)
        };
        let mut app = Self {
            cat,
            var: first.clone(),
            index: vec![],
            cmap,
            range_mode,
            picker: None,
            cmdline: None,
            mask: vec![],
            active_mask: Mask::default(),
            mask_error: None,
            contour: None,
            active_contour: None,
            contour_error: None,
            origin: Origin::default(),
            help: false,
            cvd: std::env::var("NQV_CVD").is_ok_and(|v| !v.is_empty() && v != "0"),
            interp: Interp::Nearest,
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
                        self.save_command();
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
        self.probe_cell(cell);
    }

    /// Show the value at map cell `cell` (y, x indices) and the series there.
    fn probe_cell(&mut self, cell: (usize, usize)) {
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
        if let Some(line) = &mut self.cmdline {
            match code {
                KeyCode::Esc => self.cmdline = None,
                KeyCode::Backspace if line.is_empty() => self.cmdline = None,
                KeyCode::Backspace => {
                    line.pop();
                }
                KeyCode::Char(c) => line.push(c),
                KeyCode::Enter => {
                    let line = self.cmdline.take().unwrap_or_default();
                    match command::parse(&line).and_then(|c| self.apply(c)) {
                        Ok(true) => return false,
                        Ok(false) => {}
                        Err(e) => self.message = Some(e),
                    }
                }
                _ => {}
            }
            return true;
        }
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
        if self.help {
            self.help = false;
            self.clear = true;
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
            KeyCode::Char('c') => self.next_cmap(),
            KeyCode::Char('i') => {
                self.interp = match self.interp {
                    Interp::Nearest => Interp::Bilinear,
                    Interp::Bilinear => Interp::Nearest,
                }
            }
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
            KeyCode::Char('r') => {
                self.range_mode = match self.range_mode {
                    RangeMode::Auto => RangeMode::Symmetric,
                    RangeMode::Symmetric => RangeMode::Log(None),
                    RangeMode::Log(_) | RangeMode::Fixed(..) => RangeMode::Auto,
                }
            }
            KeyCode::Char(':') => self.cmdline = Some(String::new()),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('p') => self.message = Some(self.command_line()),
            KeyCode::Char('d') if self.cat.compare => {
                self.cat.show = match self.cat.show {
                    Show::Diff => Show::A,
                    Show::A => Show::B,
                    Show::B => Show::Diff,
                };
                (self.cmap, self.range_mode) = colour_defaults(self.cat.show);
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
        if res.is_ok() {
            self.origin = Origin {
                paths: paths.to_vec(),
                ..Origin::default()
            };
        }
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
            (new.cmap, new.range_mode) = (self.cmap, self.range_mode);
        }
        new.mask = self.mask.clone();
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
                        Ok(mut app) => {
                            app.origin.paths = paths;
                            return Ok(Some(app));
                        }
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
        let hidden = masked(&self.cat, &self.var, index, &self.active_mask);
        if hidden.map_err(|e| format!("{e:#}"))? {
            return Err("masked".into());
        }
        value(&self.cat, &self.var, index).map_err(|e| format!("{e:#}"))
    }

    /// Resolve the typed mask for the current variable: names in the current file
    /// first, then anywhere in the collection; `file:var` names the file.
    fn update_mask(&mut self) {
        let resolved = self
            .mask
            .iter()
            .map(|c| {
                Ok(Cond {
                    var: self.resolve(&c.var)?,
                    op: c.op,
                    values: c.values.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()
            .map(|conds| Mask { conds })
            .and_then(|m| {
                check_mask(&self.cat, &self.var, &m)
                    .map(|_| m)
                    .map_err(|e| format!("{e:#}"))
            });
        match resolved {
            Ok(m) => (self.active_mask, self.mask_error) = (m, None),
            Err(e) => (self.active_mask, self.mask_error) = (Mask::default(), Some(e)),
        }
        let contour = self.contour.as_ref().map(|c| {
            let spec = ContourSpec {
                var: self.resolve(&c.var)?,
                levels: c.levels.clone(),
            };
            check_contour(&self.cat, &self.var, &spec)
                .map(|_| spec)
                .map_err(|e| format!("{e:#}"))
        });
        match contour.transpose() {
            Ok(c) => (self.active_contour, self.contour_error) = (c, None),
            Err(e) => (self.active_contour, self.contour_error) = (None, Some(e)),
        }
    }

    /// Contour segments over the map `spec`, if contours are on; errors leave them out.
    fn contour_lines(&self, spec: &ViewSpec) -> Vec<ContourLine> {
        let Some(c) = &self.active_contour else {
            return vec![];
        };
        contours(&self.cat, spec, c).unwrap_or_default()
    }

    /// All segments of the contour lines over the map `spec`.
    fn contour_segments(&self, spec: &ViewSpec) -> Vec<Segment> {
        let lines = self.contour_lines(spec);
        lines.into_iter().flat_map(|l| l.segments).collect()
    }

    /// Variable named exactly `name` (or `file:name`), preferring the current file.
    fn resolve(&self, name: &str) -> Result<VarRef, String> {
        let vars = self.cat.data_vars();
        let found = match name.split_once(':') {
            Some((file, var)) => vars.iter().find(|(r, v)| {
                v.name == var
                    && (self.cat.label(r.source) == file
                        || self.cat.sources[r.source]
                            .path
                            .file_name()
                            .is_some_and(|f| f == file))
            }),
            None => vars
                .iter()
                .find(|(r, v)| v.name == name && r.source == self.var.source)
                .or_else(|| vars.iter().find(|(_, v)| v.name == name)),
        };
        found
            .map(|(r, _)| r.clone())
            .ok_or_else(|| format!("no variable '{name}'"))
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
            self.range_mode,
            self.cat.show,
            self.active_mask.clone(),
            self.active_contour.clone(),
            self.interp,
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
        self.update_mask();
        let key = (
            self.var.clone(),
            self.index.clone(),
            self.cmap,
            self.range_mode,
            self.cat.show,
            self.active_mask.clone(),
            self.active_contour.clone(),
            self.interp,
            area,
        );
        if self.frame.as_ref().is_some_and(|f| f.key == key) {
            return;
        }
        let (plot, range, norm) = match self.var_info().kind() {
            Kind::Map { y, x } => self.map(out, area, y, x),
            Kind::Series { along } => self.line(area, along, self.index.clone()),
            Kind::Scalar => match self.value_at(&self.index) {
                Ok(v) => (Plot::Scalar(v), None, None),
                Err(e) => (Plot::Error(e), None, None),
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
        match self.range_mode {
            RangeMode::Auto => Norm::linear(min, max),
            RangeMode::Symmetric => Norm::symmetric(min, max),
            RangeMode::Fixed(min, max) => Norm::linear(min, max),
            // From the smallest positive value shown, but at most four decades.
            RangeMode::Log(None) => Norm::log(min.max(max * 1e-4), max),
            RangeMode::Log(Some((min, max))) => Norm::log(min, max),
        }
    }

    /// Carry out a command from the `:` line or the command line; true means quit.
    pub fn apply(&mut self, cmd: Command) -> Result<bool, String> {
        match cmd {
            Command::Quit => return Ok(true),
            Command::Var(q) => match filtered_vars(&self.cat, &q).first() {
                Some((r, _)) => {
                    let r = r.clone();
                    self.set_var(r);
                }
                None => return Err(format!("no variable matching '{q}'")),
            },
            Command::Time(sel) => {
                let v = self.var_info();
                let Some(&d) = v.step_dims().first() else {
                    return Err(format!("{} has no dimension to step through", v.name));
                };
                let dim = &v.dims[d];
                let coord = self.cat.coord(self.var.source, dim);
                let i = command::time_index(sel, dim.len, coord.as_deref())
                    .map_err(|e| format!("t: {}: {e}", dim.name))?;
                self.index[d] = i;
            }
            Command::Range(m) => self.range_mode = m,
            Command::Mask(m) => {
                self.mask = m.unwrap_or_default();
                // Re-read the clicked value under the new mask.
                if let Some(p) = &mut self.probe {
                    p.at.clear();
                }
            }
            Command::At(dim, pos) => self.set_at(&dim, pos)?,
            Command::Cmap(name) => {
                let Some(i) = cmap_index(&name) else {
                    let names: Vec<&str> = CMAPS.iter().map(|c| c.name).collect();
                    return Err(format!(
                        "cmap: '{name}' unknown; one of {}",
                        names.join(", ")
                    ));
                };
                self.cmap = i;
                self.warn_cvd();
            }
            Command::Interp(i) => self.interp = i,
            Command::Cvd(on) => {
                self.cvd = on;
                self.warn_cvd();
            }
            Command::Contour(c) => self.contour = c,
            Command::Recipe(name) => {
                for c in recipe::load(&name)? {
                    self.apply(c)?;
                }
            }
            Command::Save(name) => {
                let lines: Vec<String> = self.settings().into_iter().map(|(l, _)| l).collect();
                let p = recipe::save(&name, &lines)?;
                self.message = Some(format!("saved recipe {} ({})", name, p.display()));
            }
            Command::Show(show) => {
                if !self.cat.compare {
                    return Err("show: only in a comparison (--diff)".into());
                }
                self.cat.show = show;
                (self.cmap, self.range_mode) = colour_defaults(show);
                self.probe = None;
                self.clear = true;
            }
        }
        Ok(false)
    }

    /// Next colour map; in colour-blind mode only those suited to it.
    fn next_cmap(&mut self) {
        for k in 1..=CMAPS.len() {
            let i = (self.cmap + k) % CMAPS.len();
            if !self.cvd || CMAPS[i].cvd_safe {
                self.cmap = i;
                return;
            }
        }
    }

    /// In colour-blind mode, say so when the colour map does not suit it.
    fn warn_cvd(&mut self) {
        let c = &CMAPS[self.cmap];
        if self.cvd && !c.cvd_safe {
            self.message = Some(format!(
                "{} is hard to read with colour vision deficiencies; c picks suitable maps",
                c.name
            ));
        }
    }

    /// Position along dimension `dim`: a step for stepped dimensions, the clicked cell
    /// for map dimensions (the other map dimension stays, or starts in the middle).
    fn set_at(&mut self, dim: &str, pos: TimeSel) -> Result<(), String> {
        let v = self.var_info();
        let Some(k) = v.dims.iter().position(|d| d.name == dim) else {
            let names: Vec<&str> = v.dims.iter().map(|d| d.name.as_str()).collect();
            return Err(format!(
                "at: '{}' has no dimension '{dim}' (it has {})",
                v.name,
                names.join(", ")
            ));
        };
        let d = &v.dims[k];
        let coord = self.cat.coord(self.var.source, d);
        let i = command::time_index(pos, d.len, coord.as_deref())
            .map_err(|e| format!("at: {dim}: {e}"))?;
        match v.kind() {
            Kind::Map { y, x } if k == y || k == x => {
                let mid = (v.dims[y].len / 2, v.dims[x].len / 2);
                let mut cell = self.probe.as_ref().map_or(mid, |p| p.cell);
                if k == y {
                    cell.0 = i;
                } else {
                    cell.1 = i;
                }
                self.probe_cell(cell);
            }
            Kind::Series { along } if k == along => {
                return Err(format!("at: '{dim}' is the plotted dimension"));
            }
            _ => self.index[k] = i,
        }
        Ok(())
    }

    /// The command line that opens this view: files, variable, steps, clicked cell,
    /// comparison side, colours and mask. Settings at their defaults are left out.
    pub fn command_line(&self) -> String {
        let mut a: Vec<String> = vec!["nqv".into()];
        let o = &self.origin;
        if o.diff {
            a.push("--diff".into());
        }
        if o.join {
            a.push("-j".into());
        }
        if let Some(d) = &o.join_dim {
            a.extend(["--join-dim".into(), d.clone()]);
        }
        a.extend(o.paths.iter().map(|p| p.display().to_string()));
        if !o.vs.is_empty() {
            a.push("--vs".into());
            a.extend(o.vs.iter().map(|p| p.display().to_string()));
        }
        a.extend(self.settings().into_iter().flat_map(|(_, flags)| flags));
        let quoted: Vec<String> = a.iter().map(|s| command::shell_quote(s)).collect();
        quoted.join(" ")
    }

    /// This view's settings as recipe lines (`:` commands) with the equivalent flags:
    /// variable, steps, clicked cell, comparison side, colours, mask and contours.
    /// Settings at their defaults are left out.
    fn settings(&self) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = vec![];
        let mut add = |cmd: &str, arg: String, flag: &str, value: String| {
            out.push((format!("{cmd} {arg}"), vec![flag.to_string(), value]));
        };
        let v = self.var_info();
        let var = match self.cat.sources.len() {
            1 => v.name.clone(),
            _ => {
                let file = self.cat.sources[self.var.source].path.file_name();
                let file = file
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{file}:{}", v.name)
            }
        };
        add("v", var.clone(), "-v", var);
        let show = if self.cat.compare {
            self.cat.show
        } else {
            Show::A
        };
        if self.cat.compare && show != Show::Diff {
            let side = if show == Show::A { "a" } else { "b" };
            add("show", side.into(), "--show", side.into());
        }
        for (n, &k) in v.step_dims().iter().enumerate() {
            if self.index[k] == 0 {
                continue;
            }
            let at = format!("@{}", self.index[k] + 1);
            let dim_at = format!("{}={at}", v.dims[k].name);
            match n {
                0 => add("t", at.clone(), "-t", at),
                _ => add("at", dim_at.clone(), "--at", dim_at),
            }
        }
        if let (Some(p), Kind::Map { y, x }) = (&self.probe, v.kind()) {
            for (k, i) in [(y, p.cell.0), (x, p.cell.1)] {
                let dim_at = format!("{}=@{}", v.dims[k].name, i + 1);
                add("at", dim_at.clone(), "--at", dim_at);
            }
        }
        let (cmap, range) = colour_defaults(show);
        if self.cmap != cmap {
            let name = CMAPS[self.cmap].name.to_string();
            add("cmap", name.clone(), "--cmap", name);
        }
        if self.range_mode != range {
            let (line, flag) = match self.range_mode {
                RangeMode::Auto => ("auto".into(), "auto".into()),
                RangeMode::Symmetric => ("sym".into(), "sym".into()),
                RangeMode::Fixed(lo, hi) => (format!("{lo} {hi}"), format!("{lo},{hi}")),
                RangeMode::Log(None) => ("log".into(), "log".into()),
                RangeMode::Log(Some((lo, hi))) => {
                    (format!("log {lo} {hi}"), format!("log,{lo},{hi}"))
                }
            };
            add("range", line, "--range", flag);
        }
        if !self.mask.is_empty() {
            let m: Vec<String> = self.mask.iter().map(|c| c.text()).collect();
            add("mask", m.join(" and "), "--mask", m.join(" and "));
        }
        if self.interp == Interp::Bilinear {
            add("interp", "bilinear".into(), "--interp", "bilinear".into());
        }
        if let Some(c) = &self.contour {
            add("contour", c.text(), "--contour", c.text());
        }
        out
    }

    /// Keep the command for the last view in ~/.cache/nqv/last-command; errors are ignored.
    fn save_command(&self) {
        if let Some(home) = std::env::var_os("HOME") {
            let dir = PathBuf::from(home).join(".cache/nqv");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("last-command"), self.command_line() + "\n");
        }
    }

    /// Write the current view as a PNG of at most `max` pixels: a map with a colour bar
    /// (data aspect kept, small grids scaled up), or a line plot. Returns a summary.
    pub fn write_png(&mut self, path: &Path, max: (u32, u32)) -> Result<String> {
        self.update_mask();
        if let Some(e) = &self.mask_error {
            bail!("mask: {e}");
        }
        if let Some(e) = &self.contour_error {
            bail!("contour: {e}");
        }
        let v = self.var_info().clone();
        let src = self.var.source;
        let label = |k: usize| {
            let d = &v.dims[k];
            match self.cat.coord_var(src, d).and_then(|c| c.units.clone()) {
                Some(u) => format!("{} [{u}]", d.name),
                None => d.name.clone(),
            }
        };
        let title = format!(
            "{}:{}{}  {}",
            self.cat.label(src),
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
            .map(|&k| {
                let c = self
                    .cat
                    .coord(src, &v.dims[k])
                    .and_then(|c| c.get(self.index[k]).copied());
                ui::dim_at(&v.dims[k], self.index[k], c)
            })
            .collect();
        if self.cat.compare {
            sub.push(match self.cat.show {
                Show::Diff => "A - B".into(),
                Show::A => "A".into(),
                Show::B => "B".into(),
            });
        }
        if !self.mask.is_empty() {
            let m: Vec<String> = self.mask.iter().map(|c| c.text()).collect();
            sub.push(format!("mask: {}", m.join(" and ")));
        }
        if let (Some(c), Kind::Map { .. }) = (&self.contour, v.kind()) {
            sub.push(format!("contours: {}", c.text()));
        }
        let subtitle = sub.join("   ");
        let cmap = &CMAPS[self.cmap];
        let img = match v.kind() {
            Kind::Map { y, x } => {
                let area = png::map_area(max);
                let spec = ViewSpec {
                    var: self.var.clone(),
                    index: self.index.clone(),
                    size: fit_size(v.dims[x].len.max(1), v.dims[y].len.max(1), area.0, area.1),
                    mask: self.active_mask.clone(),
                    interp: self.interp,
                };
                let raster = view(&self.cat, &spec)?;
                // The sampled range, widened to every value drawn (it is all on the page).
                let rg = range(&self.cat, &spec)?;
                let (lo, hi) = nqv_core::finite_range(&raster.values).unwrap_or((rg.min, rg.max));
                let norm = self.norm(rg.min.min(lo), rg.max.max(hi));
                let mut image = colorize(raster.w, raster.h, &raster.values, &norm, cmap);
                let lines = self.contour_lines(&spec);
                let segments: Vec<Segment> = lines
                    .iter()
                    .flat_map(|l| l.segments.iter().copied())
                    .collect();
                draw_lines(&mut image, &segments);
                // Round coordinate values (or 1-based indices) as ticks, placed linearly
                // between the cells shown at the two ends of each edge (regular grids).
                let at = |k: usize, i: usize| {
                    let c = self
                        .cat
                        .coord(src, &v.dims[k])
                        .and_then(|c| c.get(i).copied());
                    c.unwrap_or((i + 1) as f32) as f64
                };
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
                            (p.round() as u32, ui::fmt(t as f32))
                        })
                        .collect::<Vec<_>>()
                };
                let xticks = ticks(raster.w, &|p| {
                    cell_at(&self.cat, &spec, (p, 0)).map(|c| at(x, c.1))
                });
                let yticks = ticks(raster.h, &|p| {
                    cell_at(&self.cat, &spec, (0, p)).map(|c| at(y, c.0))
                });
                png::render_map(&png::MapPlot {
                    title,
                    subtitle,
                    image: &image,
                    xticks,
                    yticks,
                    xlabel: label(x),
                    ylabel: label(y),
                    norm,
                    cmap,
                    contours: &lines,
                    extend: nqv_core::finite_range(&raster.values)
                        .map_or((false, false), |(lo, hi)| extends(&norm, lo, hi)),
                })
            }
            Kind::Series { along } => {
                let spec = SeriesSpec {
                    var: self.var.clone(),
                    along,
                    index: self.index.clone(),
                    max_points: max.0 as usize,
                    mask: self.active_mask.clone(),
                };
                let s = series(&self.cat, &spec)?;
                let (lo, hi) = series_range(&self.cat, &spec, &s).unwrap_or((f32::NAN, f32::NAN));
                let norm = self.norm(lo, hi);
                png::render_line(
                    &png::LinePlot {
                        title,
                        subtitle,
                        series: &s,
                        norm,
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
            self.cat.label(src),
            v.name
        ))
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
            mask: self.active_mask.clone(),
            interp: self.interp,
        };
        match view(&self.cat, &spec).and_then(|r| Ok((r, range(&self.cat, &spec)?))) {
            Err(e) => (Plot::Error(format!("{e:#}")), None, None),
            Ok((raster, rg)) => {
                let norm = self.norm(rg.min, rg.max);
                let mut rgba =
                    colorize(raster.w, raster.h, &raster.values, &norm, &CMAPS[self.cmap]);
                let (w, h) = (rgba.w, rgba.h);
                let segments = self.contour_segments(&spec);
                // Leave room for the statistics beside the map when it is narrower than the area.
                let (cols, rows) = match out {
                    Output::Blocks { .. } => (w as u16, h.div_ceil(2) as u16),
                    Output::Pixels(p) => {
                        let fs = p.font_size();
                        (
                            w.div_ceil(fs.width as u32) as u16,
                            h.div_ceil(fs.height as u32) as u16,
                        )
                    }
                };
                // Beside the map when it leaves room, else on one line below it.
                let side = area.width.saturating_sub(cols) >= ui::STATS_WIDTH + 2;
                let below = !side && area.height >= 4;
                let map_area = match (side, below) {
                    (true, _) => Rect {
                        width: area.width - ui::STATS_WIDTH - 1,
                        ..area
                    },
                    (_, true) => Rect {
                        height: area.height - 1,
                        ..area
                    },
                    _ => area,
                };
                let rect = ui::centered(map_area, cols, rows);
                let place = match (side, below) {
                    (true, _) => Some(Rect::new(
                        rect.x + rect.width + 2,
                        rect.y,
                        ui::STATS_WIDTH,
                        rect.height.min(9),
                    )),
                    (_, true) => Some(Rect::new(area.x, rect.y + rect.height, area.width, 1)),
                    _ => None,
                };
                let stats = place.and_then(|r| stats(&self.cat, &spec).ok().map(|st| (st, r)));
                let image = match out {
                    Output::Blocks { .. } => MapImage::Blocks {
                        img: rgba,
                        braille: braille_cells(w, h, &segments),
                    },
                    Output::Pixels(p) => {
                        draw_lines(&mut rgba, &segments);
                        match pixels(p, rgba, map_area) {
                            Ok(proto) => MapImage::Pixels(proto),
                            Err(e) => return (Plot::Error(e), None, None),
                        }
                    }
                };
                let plot = Plot::Map(Map {
                    image,
                    spec,
                    rect,
                    stats,
                });
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
            mask: self.active_mask.clone(),
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

/// Colour map index and range mode for what is shown: differences read best on a
/// diverging map around zero, plain fields on viridis with their own range.
fn colour_defaults(show: Show) -> (usize, RangeMode) {
    match show {
        Show::Diff => (cmap_index("RdBu_r").unwrap_or(0), RangeMode::Symmetric),
        Show::A | Show::B => (cmap_index("viridis").unwrap_or(0), RangeMode::Auto),
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
