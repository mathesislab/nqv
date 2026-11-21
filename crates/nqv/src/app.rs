//! The terminal front end: frames drawn from the session's data, overlays (picker,
//! browser, help, command line), keys and mouse.

use std::path::Path;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use image::{DynamicImage, RgbaImage};
use nqv_core::{
    ContourSpec, Interp, Kind, Mask, RangeEstimate, Segment, Series, Show, Stats, VarRef, ViewSpec,
    fit_size,
};
use nqv_render::{CMAPS, Norm, Rgba, braille_cells, colorize, draw_lines};
use nqv_session::command::{self, RangeMode};
use nqv_session::session::dir_of;
use nqv_session::{Effect, LocalFiles, Overlays, Probe, Session};
use ratatui::DefaultTerminal;
use ratatui::layout::{Position, Rect, Size};
use ratatui_image::{Resize, protocol::Protocol};

use crate::Output;
use crate::browser::{Action, Browser};
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

/// Variable picker: query typed so far and highlighted row.
pub struct VarPicker {
    pub query: String,
    pub sel: usize,
}

pub struct App {
    pub s: Session,
    /// Mask and contours resolved for the frame drawn, with their errors.
    pub overlays: Overlays,
    /// Key and command overview, drawn over everything else.
    pub help: bool,
    pub picker: Option<VarPicker>,
    /// Text typed after `:`, while the command line is open.
    pub cmdline: Option<String>,
    pub frame: Option<Frame>,
    /// The clicked cell as last read, and the indices it was read at.
    pub probe: Option<Probe>,
    probe_at: Vec<usize>,
    /// Whether the last update drew a new frame (the clicked value is then re-read).
    frame_changed: bool,
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
    pub fn new(s: Session) -> Self {
        Self {
            message: s.startup_message(),
            s,
            overlays: Overlays::default(),
            help: false,
            picker: None,
            cmdline: None,
            frame: None,
            probe: None,
            probe_at: vec![],
            frame_changed: true,
            panel: None,
            browser: None,
            clear: false,
        }
    }

    /// Whether the series at the clicked cell is shown below the map.
    pub fn panel_open(&self) -> bool {
        self.s.probe.is_some() && self.s.series_dim().is_some()
    }

    /// Apply a command; returns false to quit.
    pub fn apply(&mut self, cmd: command::Command) -> Result<bool, String> {
        let was_open = self.panel_open();
        let effect = self.s.apply(cmd)?;
        self.clear |= self.panel_open() != was_open;
        Ok(self.effect(effect))
    }

    /// Show what a command asks for; false means quit.
    fn effect(&mut self, e: Effect) -> bool {
        match e {
            Effect::None => true,
            Effect::Quit => false,
            Effect::Message(m) => {
                self.message = Some(m);
                true
            }
        }
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
                        self.s.save_command();
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
        let Some(cell) = pixel_at(out, m.rect, col, row).and_then(|px| self.s.cell_at(&m.spec, px))
        else {
            return;
        };
        let was_open = self.panel_open();
        self.s.probe = Some(cell);
        // Opening the panel shrinks the map; image protocols would leave the old one behind.
        self.clear |= self.panel_open() != was_open;
    }

    fn close_probe(&mut self) {
        self.clear |= self.panel_open();
        self.s.probe = None;
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
                        Ok(go_on) => return go_on,
                        Err(e) => self.message = Some(e),
                    }
                }
                _ => {}
            }
            return true;
        }
        if let Some(b) = &mut self.browser {
            match b.handle_key(self.s.backend.as_ref(), code) {
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
            let names = self.s.filtered_vars(&p.query);
            match code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Enter => {
                    if let Some(r) = names.get(p.sel).map(|(r, _)| r.clone()) {
                        self.s.set_var(r);
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
            KeyCode::Esc if self.s.probe.is_some() => self.close_probe(),
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Right => self.s.step(0, 1),
            KeyCode::Left => self.s.step(0, -1),
            KeyCode::Up => self.s.step(1, 1),
            KeyCode::Down => self.s.step(1, -1),
            KeyCode::Home => self.s.step(0, isize::MIN),
            KeyCode::End => self.s.step(0, isize::MAX),
            KeyCode::Char('v') => {
                self.picker = Some(VarPicker {
                    query: String::new(),
                    sel: 0,
                })
            }
            KeyCode::Char('c') => self.s.next_cmap(),
            KeyCode::Char('i') => self.s.toggle_interp(),
            KeyCode::Char('o') => {
                let file = self.s.source_path();
                let files = self.s.backend.as_ref();
                self.browser = Some(Browser::new(files, &dir_of(&file), Some(&file)));
            }
            KeyCode::Char('O') if self.s.info().compare => {
                self.message = Some("O: not in a comparison; use o".into())
            }
            KeyCode::Char('O') => self.open_paths(&[dir_of(&self.s.source_path())]),
            KeyCode::Char(']') => self.neighbour(1),
            KeyCode::Char('[') => self.neighbour(-1),
            KeyCode::Char('r') => self.s.cycle_range(),
            KeyCode::Char(':') => self.cmdline = Some(String::new()),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('p') => self.message = Some(self.s.command_line()),
            KeyCode::Char('d') => match self.s.cycle_show() {
                Ok(()) => self.clear = true,
                Err(e) => self.message = Some(e),
            },
            _ => {}
        }
        true
    }

    /// Open `paths` as on the command line, keeping the variable where possible. On
    /// failure the message goes to the browser if it is open.
    fn open_paths(&mut self, paths: &[std::path::PathBuf]) {
        match self.s.reopen(paths) {
            Ok(()) => {
                self.after_switch();
            }
            Err(e) => match &mut self.browser {
                Some(b) => b.message = Some(e),
                None => self.message = Some(e),
            },
        }
    }

    fn neighbour(&mut self, delta: isize) {
        let source = (self.s.source_path(), self.s.var.clone());
        match self.s.neighbour(delta) {
            Ok(()) if (self.s.source_path(), self.s.var.clone()) != source => self.after_switch(),
            Ok(()) => {}
            Err(e) => self.message = Some(e),
        }
    }

    /// After other files were opened: close the browser, redraw, show their notes.
    fn after_switch(&mut self) {
        self.browser = None;
        self.clear = true;
        self.message = self.s.startup_message();
    }

    /// File browser on its own, before anything is open; None when it is closed.
    pub fn pick(terminal: &mut DefaultTerminal, dir: &Path) -> Result<Option<App>> {
        let mut b = Browser::new(&LocalFiles, dir, None);
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
            match b.handle_key(&LocalFiles, k.code) {
                Action::None => {}
                Action::Close => return Ok(None),
                Action::Open(paths) => {
                    match Session::open(&paths, &nqv_session::OpenMode::default()) {
                        Ok(s) => return Ok(Some(App::new(s))),
                        Err(e) => b.message = Some(format!("{e:#}")),
                    }
                }
            }
        }
    }

    /// Re-read the value at the clicked cell when the indices or mask changed, and
    /// recompute the series below the map when it or its `area` changed.
    fn update_probe(&mut self, area: Option<Rect>) {
        let Some(cell) = self.s.probe else {
            (self.probe, self.panel) = (None, None);
            return;
        };
        let at = self.s.index_at(cell);
        let stale = self.probe.as_ref().is_none_or(|p| p.cell != cell) || self.probe_at != at;
        if stale || self.frame_changed {
            self.probe = self.s.probe_info();
            self.probe_at = at.clone();
        }
        let (Some(area), Some(along)) = (area, self.s.series_dim()) else {
            self.panel = None;
            return;
        };
        // The series does not depend on the step along it.
        let mut index = at;
        index[along] = 0;
        let key = self.key(index.clone(), area);
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

    /// The key of a frame of the current state at `index` over `area`.
    fn key(&self, index: Vec<usize>, area: Rect) -> FrameKey {
        (
            self.s.var.clone(),
            index,
            self.s.cmap,
            self.s.range_mode,
            self.s.info().show,
            self.overlays.mask.clone(),
            self.overlays.contour.clone(),
            self.s.interp,
            area,
        )
    }

    /// Recompute the plot when the state or the area changed.
    fn update_frame(&mut self, out: &Output, area: Rect) {
        self.overlays = self.s.overlays();
        let key = self.key(self.s.index.clone(), area);
        self.frame_changed = self.frame.as_ref().is_none_or(|f| f.key != key);
        if !self.frame_changed {
            return;
        }
        let (plot, range, norm) = match self.s.var_info().kind() {
            Kind::Map { y, x } => self.map(out, area, y, x),
            Kind::Series { along } => self.line(area, along, self.s.index.clone()),
            Kind::Scalar => match self.s.value_at(&self.s.index) {
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

    fn map(
        &self,
        out: &Output,
        area: Rect,
        y: usize,
        x: usize,
    ) -> (Plot, Option<RangeEstimate>, Option<Norm>) {
        let dims = &self.s.var_info().dims;
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
        let size = fit_size(nx, ny, max_w, max_h);
        // Room for the statistics: beside the map when it leaves room, else one line below.
        let (cols, rows) = match out {
            Output::Blocks { .. } => (size.0 as u16, size.1.div_ceil(2) as u16),
            Output::Pixels(p) => {
                let fs = p.font_size();
                (
                    size.0.div_ceil(fs.width as u32) as u16,
                    size.1.div_ceil(fs.height as u32) as u16,
                )
            }
        };
        let side = area.width.saturating_sub(cols) >= ui::STATS_WIDTH + 2;
        let below = !side && area.height >= 4;
        let m = match self.s.map(size, side || below) {
            Ok(m) => m,
            Err(e) => return (Plot::Error(format!("{e:#}")), None, None),
        };
        let mut rgba = colorize(
            m.raster.w,
            m.raster.h,
            &m.raster.values,
            &m.norm,
            &CMAPS[self.s.cmap],
        );
        let (w, h) = (rgba.w, rgba.h);
        let segments: Vec<Segment> = m.contours.into_iter().flat_map(|l| l.segments).collect();
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
        let stats = m.stats.zip(place);
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
            spec: m.spec,
            rect,
            stats,
        });
        (plot, Some(m.range), Some(m.norm))
    }

    /// Series along `along` at `index`, sampled for the width of `area`.
    fn line(
        &self,
        area: Rect,
        along: usize,
        index: Vec<usize>,
    ) -> (Plot, Option<RangeEstimate>, Option<Norm>) {
        // Braille gives two points per cell column.
        match self.s.series_at(index, along, 2 * area.width as usize) {
            Err(e) => (Plot::Error(format!("{e:#}")), None, None),
            Ok(d) => (Plot::Line(d.series), Some(d.range), Some(d.norm)),
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
