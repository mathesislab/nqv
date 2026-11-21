//! The state of a view and everything done to it, for any front end: which variable,
//! steps and clicked cell, colours, mask and contours; commands, recipes, switching
//! files; and the data to draw (values, not colours).

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use nqv_core::{
    Cond, ContourLine, ContourSpec, Interp, Kind, Mask, RangeEstimate, Raster, Series, SeriesSpec,
    Show, Stats, VarInfo, VarRef, ViewSpec,
};
use nqv_render::{CMAPS, Norm, cmap_index};

use crate::backend::{Backend, CatalogInfo, Local, OpenMode};
use crate::command::{self, Command, ContourCmd, MaskCond, RangeMode, TimeSel};
use crate::{fuzzy, recipe};

/// The paths and mode the current files were opened with, for the command line that
/// reproduces the view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Origin {
    pub paths: Vec<PathBuf>,
    pub mode: OpenMode,
}

/// What a command asks of the front end.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    Quit,
    /// Something to tell the user.
    Message(String),
}

/// Mask and contours resolved for the current variable, with why not where they can't
/// be drawn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Overlays {
    pub mask: Mask,
    pub mask_error: Option<String>,
    pub contour: Option<ContourSpec>,
    pub contour_error: Option<String>,
}

/// A map to draw: the values on the view's pixel grid and how to colour them.
pub struct MapData {
    pub spec: ViewSpec,
    pub raster: Raster,
    /// The (sampled) range of the values, and the colour range from it.
    pub range: RangeEstimate,
    pub norm: Norm,
    pub contours: Vec<ContourLine>,
    pub stats: Option<Stats>,
}

/// A line plot: the series and its y range.
pub struct SeriesData {
    pub series: Series,
    pub range: RangeEstimate,
    pub norm: Norm,
}

/// A clicked map cell: its indices (y, x), coordinate values and value.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub cell: (usize, usize),
    pub coords: (Option<f32>, Option<f32>),
    /// The value, or why there is none ("masked", a read error).
    pub value: Result<f32, String>,
}

pub struct Session {
    pub backend: Box<dyn Backend>,
    pub origin: Origin,
    pub var: VarRef,
    /// One index per dimension of `var`; entries at shown dimensions are unused.
    pub index: Vec<usize>,
    pub cmap: usize,
    pub range_mode: RangeMode,
    pub interp: Interp,
    /// Colour-blind mode: `next_cmap` offers only colour maps suited to it.
    pub cvd: bool,
    /// Mask conditions and contours as typed; resolved by `overlays`.
    pub mask: Vec<MaskCond>,
    pub contour: Option<ContourCmd>,
    /// Clicked map cell (y, x).
    pub probe: Option<(usize, usize)>,
}

impl Session {
    /// Open `paths` as the command line does with `mode`, on this machine.
    pub fn open(paths: &[PathBuf], mode: &OpenMode) -> Result<Self> {
        let backend = Local::open(paths, mode)?;
        let origin = Origin {
            paths: paths.to_vec(),
            mode: mode.clone(),
        };
        Self::new(Box::new(backend), origin)
    }

    /// A session on the first variable of the most common kind, with default colours.
    pub fn new(backend: Box<dyn Backend>, origin: Origin) -> Result<Self> {
        let mut s = Self {
            backend,
            origin,
            var: VarRef {
                source: 0,
                name: String::new(),
            },
            index: vec![],
            cmap: 0,
            range_mode: RangeMode::Auto,
            interp: Interp::Nearest,
            cvd: std::env::var("NQV_CVD").is_ok_and(|v| !v.is_empty() && v != "0"),
            mask: vec![],
            contour: None,
            probe: None,
        };
        s.reset()?;
        Ok(s)
    }

    /// The default variable and colours of the opened files.
    fn reset(&mut self) -> Result<()> {
        let info = self.info();
        let Some(first) = default_var(&info.data_vars()).cloned() else {
            bail!("no data variables");
        };
        (self.cmap, self.range_mode) =
            colour_defaults(if info.compare { info.show } else { Show::A });
        self.set_var(first);
        Ok(())
    }

    pub fn info(&self) -> &CatalogInfo {
        self.backend.info()
    }

    pub fn var_info(&self) -> &VarInfo {
        self.info().var(&self.var).expect("current variable exists")
    }

    /// Notes and skipped files from opening, to show once.
    pub fn startup_message(&self) -> Option<String> {
        let info = self.info();
        match (info.notes.first(), info.skipped.first()) {
            (Some(note), _) => Some(format!("{note} ({} warning(s))", info.notes.len())),
            (None, Some((path, why))) => Some(format!(
                "{} file(s) not opened, e.g. {}: {why}",
                info.skipped.len(),
                path.display()
            )),
            (None, None) => None,
        }
    }

    /// Data variables matching a picker query, best matches first.
    pub fn filtered_vars(&self, query: &str) -> Vec<(VarRef, &VarInfo)> {
        filtered_vars(self.info(), query)
    }

    pub fn set_var(&mut self, r: VarRef) {
        let n = self.info().var(&r).map(|v| v.dims.len()).unwrap_or(0);
        self.var = r;
        self.index = vec![0; n];
        self.probe = None;
    }

    /// Dimension of the series shown for a clicked cell: the one stepped first.
    pub fn series_dim(&self) -> Option<usize> {
        self.var_info().step_dims().first().copied()
    }

    /// Move the `k`-th steppable dimension by `delta`, clamped to its length.
    pub fn step(&mut self, k: usize, delta: isize) {
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

    /// Next colour map; in colour-blind mode only those suited to it.
    pub fn next_cmap(&mut self) {
        for k in 1..=CMAPS.len() {
            let i = (self.cmap + k) % CMAPS.len();
            if !self.cvd || CMAPS[i].cvd_safe {
                self.cmap = i;
                return;
            }
        }
    }

    /// Range auto -> symmetric -> log -> auto; a fixed range goes back to auto.
    pub fn cycle_range(&mut self) {
        self.range_mode = match self.range_mode {
            RangeMode::Auto => RangeMode::Symmetric,
            RangeMode::Symmetric => RangeMode::Log(None),
            RangeMode::Log(_) | RangeMode::Fixed(..) => RangeMode::Auto,
        }
    }

    pub fn toggle_interp(&mut self) {
        self.interp = match self.interp {
            Interp::Nearest => Interp::Bilinear,
            Interp::Bilinear => Interp::Nearest,
        }
    }

    /// In a comparison: A - B -> A -> B, with the colour defaults of each.
    pub fn cycle_show(&mut self) -> Result<(), String> {
        let show = match self.info().show {
            Show::Diff => Show::A,
            Show::A => Show::B,
            Show::B => Show::Diff,
        };
        self.set_show(show)
    }

    fn set_show(&mut self, show: Show) -> Result<(), String> {
        if !self.info().compare {
            return Err("d: start with --diff A B to compare two files or folders".into());
        }
        self.backend.set_show(show);
        (self.cmap, self.range_mode) = colour_defaults(show);
        self.probe = None;
        Ok(())
    }

    /// Absolute path of the current variable's (first) file, where the data is.
    pub fn source_path(&self) -> PathBuf {
        self.info().sources[self.var.source].path.clone()
    }

    /// Open `paths` (where the data is) as a plain collection, keeping the variable
    /// (from the same file if possible) with its steps, the colours outside
    /// comparisons, and the mask.
    pub fn reopen(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        let (file, old, index) = (
            self.source_path(),
            self.var_info().clone(),
            self.index.clone(),
        );
        let (compare, colours) = (self.info().compare, (self.cmap, self.range_mode));
        let mode = OpenMode::default();
        self.backend
            .reopen(paths, &mode)
            .map_err(|e| format!("{e:#}"))?;
        self.origin = Origin {
            paths: paths.to_vec(),
            mode,
        };
        self.reset().map_err(|e| format!("{e:#}"))?;
        let candidates: Vec<VarRef> = self
            .info()
            .data_vars()
            .into_iter()
            .filter(|(_, v)| v.name == old.name)
            .map(|(r, _)| r)
            .collect();
        let keep = candidates
            .iter()
            .find(|r| self.info().sources[r.source].path == file)
            .or(candidates.first())
            .cloned();
        if let Some(r) = keep {
            self.keep_var(r, &old, &index);
        }
        if !compare && !self.info().compare {
            (self.cmap, self.range_mode) = colours;
        }
        Ok(())
    }

    /// Select `r`, keeping `index` when `r` has the same dimensions as `old`.
    fn keep_var(&mut self, r: VarRef, old: &VarInfo, index: &[usize]) {
        let same = self.info().var(&r).is_ok_and(|v| v.dims == old.dims);
        self.set_var(r);
        if same {
            self.index = index.to_vec();
        }
    }

    /// Previous/next file: within a collection or folder comparison its sources, else
    /// the NetCDF files next to the current file. The variable stays where it exists.
    pub fn neighbour(&mut self, delta: isize) -> Result<(), String> {
        let n = self.info().sources.len();
        if n > 1 {
            let s = (self.var.source as isize + delta).rem_euclid(n as isize) as usize;
            let vars: Vec<(VarRef, &VarInfo)> = self
                .info()
                .data_vars()
                .into_iter()
                .filter(|(r, _)| r.source == s)
                .collect();
            let same = vars.iter().find(|(_, v)| v.name == self.var.name);
            let Some(r) = same.map(|(r, _)| r).or(default_var(&vars)).cloned() else {
                return Err(format!("{}: no data variables", self.info().label(s)));
            };
            let (old, index) = (self.var_info().clone(), self.index.clone());
            self.keep_var(r, &old, &index);
            return Ok(());
        }
        if self.info().compare {
            return Err("[ ]: comparing two files; compare folders to step through files".into());
        }
        let file = self.source_path();
        let files: Vec<PathBuf> = self
            .backend
            .list_dir(&dir_of(&file))
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.is_netcdf)
            .map(|e| e.path)
            .collect();
        let Some(pos) = files.iter().position(|f| f.file_name() == file.file_name()) else {
            return Ok(());
        };
        let next = (pos as isize + delta).rem_euclid(files.len() as isize) as usize;
        self.reopen(&[files[next].clone()])
    }

    /// Carry out a command from the `:` line, a recipe or the command line.
    pub fn apply(&mut self, cmd: Command) -> Result<Effect, String> {
        match cmd {
            Command::Quit => return Ok(Effect::Quit),
            Command::Var(q) => match self.filtered_vars(&q).first() {
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
                let coord = self.backend.coord(self.var.source, &dim.name);
                let i = command::time_index(sel, dim.len, coord.as_deref())
                    .map_err(|e| format!("t: {}: {e}", dim.name))?;
                self.index[d] = i;
            }
            Command::Range(m) => self.range_mode = m,
            Command::Mask(m) => self.mask = m.unwrap_or_default(),
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
                return Ok(self.cvd_warning());
            }
            Command::Interp(i) => self.interp = i,
            Command::Cvd(on) => {
                self.cvd = on;
                return Ok(self.cvd_warning());
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
                return Ok(Effect::Message(format!(
                    "saved recipe {name} ({})",
                    p.display()
                )));
            }
            Command::Show(show) => {
                if !self.info().compare {
                    return Err("show: only in a comparison (--diff)".into());
                }
                self.set_show(show)?;
            }
        }
        Ok(Effect::None)
    }

    /// In colour-blind mode, a note when the colour map does not suit it.
    fn cvd_warning(&self) -> Effect {
        let c = &CMAPS[self.cmap];
        match self.cvd && !c.cvd_safe {
            true => Effect::Message(format!(
                "{} is hard to read with colour vision deficiencies; c picks suitable maps",
                c.name
            )),
            false => Effect::None,
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
        let coord = self.backend.coord(self.var.source, &d.name);
        let i = command::time_index(pos, d.len, coord.as_deref())
            .map_err(|e| format!("at: {dim}: {e}"))?;
        match v.kind() {
            Kind::Map { y, x } if k == y || k == x => {
                let mid = (v.dims[y].len / 2, v.dims[x].len / 2);
                let mut cell = self.probe.unwrap_or(mid);
                if k == y {
                    cell.0 = i;
                } else {
                    cell.1 = i;
                }
                self.probe = Some(cell);
            }
            Kind::Series { along } if k == along => {
                return Err(format!("at: '{dim}' is the plotted dimension"));
            }
            _ => self.index[k] = i,
        }
        Ok(())
    }

    /// Variable named exactly `name` (or `file:name`), preferring the current file.
    fn resolve(&self, name: &str) -> Result<VarRef, String> {
        let info = self.info();
        let vars = info.data_vars();
        let found = match name.split_once(':') {
            Some((file, var)) => vars.iter().find(|(r, v)| {
                v.name == var
                    && (info.label(r.source) == file
                        || info.sources[r.source]
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

    /// The typed mask and contours resolved for the current variable.
    pub fn overlays(&self) -> Overlays {
        let mut o = Overlays::default();
        let mask = self
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
                self.backend
                    .check_overlay(&self.var, &m, None)
                    .map(|_| m)
                    .map_err(|e| format!("{e:#}"))
            });
        match mask {
            Ok(m) => o.mask = m,
            Err(e) => o.mask_error = Some(e),
        }
        if let Some(c) = &self.contour {
            let spec = self.resolve(&c.var).and_then(|var| {
                let spec = ContourSpec {
                    var,
                    levels: c.levels.clone(),
                };
                self.backend
                    .check_overlay(&self.var, &Mask::default(), Some(&spec))
                    .map(|_| spec)
                    .map_err(|e| format!("{e:#}"))
            });
            match spec {
                Ok(s) => o.contour = Some(s),
                Err(e) => o.contour_error = Some(e),
            }
        }
        o
    }

    /// Colour (or y) range for data from `min` to `max` in the current range mode.
    pub fn norm(&self, min: f32, max: f32) -> Norm {
        match self.range_mode {
            RangeMode::Auto => Norm::linear(min, max),
            RangeMode::Symmetric => Norm::symmetric(min, max),
            RangeMode::Fixed(min, max) => Norm::linear(min, max),
            // From the smallest positive value shown, but at most four decades.
            RangeMode::Log(None) => Norm::log(min.max(max * 1e-4), max),
            RangeMode::Log(Some((min, max))) => Norm::log(min, max),
        }
    }

    /// The view of the current map at `size` pixels.
    pub fn map_spec(&self, size: (u32, u32)) -> ViewSpec {
        ViewSpec {
            var: self.var.clone(),
            index: self.index.clone(),
            size,
            mask: self.overlays().mask,
            interp: self.interp,
        }
    }

    /// The current map at `size` pixels, with its contours and (if asked) statistics.
    pub fn map(&self, size: (u32, u32), with_stats: bool) -> Result<MapData> {
        let o = self.overlays();
        let spec = ViewSpec {
            var: self.var.clone(),
            index: self.index.clone(),
            size,
            mask: o.mask,
            interp: self.interp,
        };
        let p = self
            .backend
            .map_data(&spec, o.contour.as_ref(), with_stats)?;
        Ok(MapData {
            norm: self.norm(p.range.min, p.range.max),
            spec,
            raster: p.raster,
            range: p.range,
            contours: p.contours,
            stats: p.stats,
        })
    }

    /// The series along `along` at `index`, at about `max_points` points.
    pub fn series_at(
        &self,
        index: Vec<usize>,
        along: usize,
        max_points: usize,
    ) -> Result<SeriesData> {
        let spec = SeriesSpec {
            var: self.var.clone(),
            along,
            index,
            max_points,
            mask: self.overlays().mask,
        };
        let (series, r) = self.backend.series(&spec)?;
        let (min, max) = r.unwrap_or((f32::NAN, f32::NAN));
        let exact = series.y.len() == self.var_info().dims[along].len;
        Ok(SeriesData {
            series,
            range: RangeEstimate { min, max, exact },
            norm: self.norm(min, max),
        })
    }

    /// The value of the current variable at `index`; Err("masked") where the mask
    /// hides it.
    pub fn value_at(&self, index: &[usize]) -> Result<f32, String> {
        match self.backend.value(&self.var, index, &self.overlays().mask) {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err("masked".into()),
            Err(e) => Err(format!("{e:#}")),
        }
    }

    /// Map cell under pixel `px` of the map view `spec`.
    pub fn cell_at(&self, spec: &ViewSpec, px: (u32, u32)) -> Option<(usize, usize)> {
        self.backend.cell_at(spec, px)
    }

    /// Coordinate value at step `i` of dimension `k` of the current variable.
    pub fn coord_at(&self, k: usize, i: usize) -> Option<f32> {
        let dim = &self.var_info().dims[k].name;
        self.backend
            .coord(self.var.source, dim)
            .and_then(|c| c.get(i).copied())
    }

    /// Current indices with the map dimensions set to `cell`.
    pub fn index_at(&self, cell: (usize, usize)) -> Vec<usize> {
        let mut index = self.index.clone();
        if let Kind::Map { y, x } = self.var_info().kind() {
            (index[y], index[x]) = cell;
        }
        index
    }

    /// The clicked cell with its coordinates and value, if there is one.
    pub fn probe_info(&self) -> Option<Probe> {
        let cell = self.probe?;
        let Kind::Map { y, x } = self.var_info().kind() else {
            return None;
        };
        Some(Probe {
            cell,
            coords: (self.coord_at(y, cell.0), self.coord_at(x, cell.1)),
            value: self.value_at(&self.index_at(cell)),
        })
    }

    /// This view's settings as recipe lines (`:` commands) with the equivalent flags:
    /// variable, steps, clicked cell, comparison side, colours, mask and contours.
    /// Settings at their defaults are left out.
    pub fn settings(&self) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = vec![];
        let mut add = |cmd: &str, arg: String, flag: &str, value: String| {
            out.push((format!("{cmd} {arg}"), vec![flag.to_string(), value]));
        };
        let info = self.info();
        let v = self.var_info();
        let var = match info.sources.len() {
            1 => v.name.clone(),
            _ => {
                let file = info.sources[self.var.source].path.file_name();
                let file = file
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{file}:{}", v.name)
            }
        };
        add("v", var.clone(), "-v", var);
        let show = if info.compare { info.show } else { Show::A };
        if info.compare && show != Show::Diff {
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
        if let (Some(p), Kind::Map { y, x }) = (self.probe, v.kind()) {
            for (k, i) in [(y, p.0), (x, p.1)] {
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

    /// The command line that opens this view.
    pub fn command_line(&self) -> String {
        let mut a: Vec<String> = vec!["nqv".into()];
        let m = &self.origin.mode;
        if m.diff {
            a.push("--diff".into());
        }
        if m.join {
            a.push("-j".into());
        }
        if let Some(d) = &m.join_dim {
            a.extend(["--join-dim".into(), d.clone()]);
        }
        a.extend(self.origin.paths.iter().map(|p| p.display().to_string()));
        if !m.vs.is_empty() {
            a.push("--vs".into());
            a.extend(m.vs.iter().map(|p| p.display().to_string()));
        }
        a.extend(self.settings().into_iter().flat_map(|(_, flags)| flags));
        let quoted: Vec<String> = a.iter().map(|s| command::shell_quote(s)).collect();
        quoted.join(" ")
    }

    /// Keep the command for this view in ~/.cache/nqv/last-command; errors are ignored.
    pub fn save_command(&self) {
        if let Some(home) = std::env::var_os("HOME") {
            let dir = PathBuf::from(home).join(".cache/nqv");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("last-command"), self.command_line() + "\n");
        }
    }
}

/// Folder of `file`, "." for a bare file name.
pub fn dir_of(file: &Path) -> PathBuf {
    match file.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Colour map index and range mode for what is shown: differences read best on a
/// diverging map around zero, plain fields on viridis with their own range.
pub fn colour_defaults(show: Show) -> (usize, RangeMode) {
    match show {
        Show::Diff => (cmap_index("RdBu_r").unwrap_or(0), RangeMode::Symmetric),
        Show::A | Show::B => (cmap_index("viridis").unwrap_or(0), RangeMode::Auto),
    }
}

/// First variable of the most common kind, maps winning ties: a time-series file
/// opens on a series even if it also holds a few maps.
pub fn default_var<'a>(vars: &'a [(VarRef, &VarInfo)]) -> Option<&'a VarRef> {
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
pub fn filtered_vars<'a>(info: &'a CatalogInfo, query: &str) -> Vec<(VarRef, &'a VarInfo)> {
    let (file_q, var_q) = fuzzy::split(query);
    let mut hits: Vec<(u8, usize, (VarRef, &VarInfo))> = info
        .data_vars()
        .into_iter()
        .filter(|(r, _)| file_q.is_none_or(|f| fuzzy::tier(f, info.label(r.source)).is_some()))
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
