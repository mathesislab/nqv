//! Viewer state and key handling.

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use image::{DynamicImage, RgbaImage};
use nqv_core::{Catalog, RangeEstimate, VarInfo, ViewSpec, fit_size, range, view};
use nqv_render::{CMAPS, Norm, Rgba, colorize};
use ratatui::DefaultTerminal;
use ratatui::layout::{Rect, Size};
use ratatui_image::{Resize, protocol::Protocol};

use crate::Output;
use crate::ui;

/// The map as drawn for one state and screen area.
pub enum Map {
    Pixels { proto: Protocol, w: u32, h: u32 },
    Blocks(Rgba),
    Error(String),
}

pub struct Frame {
    pub key: (ViewSpec, usize, bool, Rect),
    pub map: Map,
    pub range: Option<RangeEstimate>,
    pub norm: Option<Norm>,
}

/// Variable picker: query typed so far and highlighted row.
pub struct VarPicker {
    pub query: String,
    pub sel: usize,
}

pub struct App {
    pub cat: Catalog,
    pub var: String,
    pub fixed: Vec<usize>,
    pub cmap: usize,
    pub symmetric: bool,
    pub picker: Option<VarPicker>,
    pub frame: Option<Frame>,
}

impl App {
    pub fn new(cat: Catalog, var: Option<&str>) -> Result<Self> {
        let first = match var {
            Some(v) => cat.var(v)?.clone(),
            None => match cat.map_vars().first() {
                Some(v) => (*v).clone(),
                None => bail!(
                    "{}: no variables with two or more dimensions",
                    cat.path.display()
                ),
            },
        };
        if !first.is_map() {
            bail!("{} has fewer than two dimensions", first.name);
        }
        Ok(Self {
            fixed: vec![0; first.leading_dims().len()],
            var: first.name,
            cat,
            cmap: 0,
            symmetric: false,
            picker: None,
            frame: None,
        })
    }

    pub fn var_info(&self) -> &VarInfo {
        self.cat.var(&self.var).expect("current variable exists")
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal, out: Output) -> Result<()> {
        loop {
            terminal.draw(|f| {
                let areas = ui::layout(f.area());
                self.update_frame(&out, areas.map);
                ui::draw(f, self, &out, &areas);
            })?;
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                if !self.handle_key(k.code) {
                    return Ok(());
                }
            }
        }
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
            return true;
        }
        match code {
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

    /// Move the index of leading dimension `k` by `delta`, clamped to its length.
    fn step(&mut self, k: usize, delta: isize) {
        let Some(len) = self.var_info().leading_dims().get(k).map(|d| d.len) else {
            return;
        };
        let i = self.fixed[k] as isize;
        self.fixed[k] = i
            .saturating_add(delta)
            .clamp(0, len.saturating_sub(1) as isize) as usize;
    }

    fn set_var(&mut self, name: &str) {
        let n = self
            .cat
            .var(name)
            .map(|v| v.leading_dims().len())
            .unwrap_or(0);
        self.var = name.to_string();
        self.fixed = vec![0; n];
    }

    /// Recompute the map when the state or the area changed.
    fn update_frame(&mut self, out: &Output, area: Rect) {
        let (nx, ny) = {
            let d = &self.var_info().dims;
            (d[d.len() - 1].len.max(1), d[d.len() - 2].len.max(1))
        };
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
            fixed: self.fixed.clone(),
            size: fit_size(nx, ny, max_w, max_h),
        };
        let key = (spec.clone(), self.cmap, self.symmetric, area);
        if self.frame.as_ref().is_some_and(|f| f.key == key) {
            return;
        }
        let built = view(&self.cat, &spec).and_then(|r| Ok((r, range(&self.cat, &spec)?)));
        let (map, rg, norm) = match built {
            Err(e) => (Map::Error(format!("{e:#}")), None, None),
            Ok((raster, rg)) => {
                let norm = if self.symmetric {
                    Norm::symmetric(rg.min, rg.max)
                } else {
                    Norm {
                        min: rg.min,
                        max: rg.max,
                    }
                };
                let rgba = colorize(raster.w, raster.h, &raster.values, &norm, &CMAPS[self.cmap]);
                let map = match out {
                    Output::Blocks { .. } => Map::Blocks(rgba),
                    Output::Pixels(p) => pixels(p, rgba, area),
                };
                (map, Some(rg), Some(norm))
            }
        };
        self.frame = Some(Frame {
            key,
            map,
            range: rg,
            norm,
        });
    }
}

fn pixels(p: &ratatui_image::picker::Picker, rgba: Rgba, area: Rect) -> Map {
    let (w, h) = (rgba.w, rgba.h);
    let Some(img) = RgbaImage::from_raw(w, h, rgba.data) else {
        return Map::Error("image buffer size mismatch".into());
    };
    let size = Size::new(area.width, area.height);
    match p.new_protocol(DynamicImage::ImageRgba8(img), size, Resize::Fit(None)) {
        Ok(proto) => Map::Pixels { proto, w, h },
        Err(e) => Map::Error(format!("image encoding failed: {e}")),
    }
}

/// Map variables whose name or long name contains `query` (case-insensitive).
pub fn filtered_vars<'a>(cat: &'a Catalog, query: &str) -> Vec<&'a VarInfo> {
    let q = query.to_lowercase();
    cat.map_vars()
        .into_iter()
        .filter(|v| {
            v.name.to_lowercase().contains(&q)
                || v.long_name
                    .as_ref()
                    .is_some_and(|l| l.to_lowercase().contains(&q))
        })
        .collect()
}
