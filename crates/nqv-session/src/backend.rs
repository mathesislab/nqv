//! Where the data comes from: the files opened here (`Local`), or later a server over
//! ssh. A session sees only `CatalogInfo` (metadata) and asks a `Backend` for data, so
//! the two are interchangeable.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use nqv_core::{
    Catalog, ContourLine, ContourSpec, DimInfo, DirEntry, Mask, NetcdfReader, RangeEstimate,
    Raster, Reader, Series, SeriesSpec, Show, Stats, VarInfo, VarRef, ViewSpec, cell_at,
    check_contour, check_mask, contours, list_dir, masked, range, series, series_range, stats,
    value, view,
};

/// How paths are opened (the command-line options that decide it).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpenMode {
    /// Two files or two folders compared.
    pub diff: bool,
    /// Files joined along time (or `join_dim`).
    pub join: bool,
    pub join_dim: Option<String>,
    /// Side B of a comparison of two sets of paths.
    pub vs: Vec<PathBuf>,
    /// A file giving the geography of maps whose files lack it (`--grid`).
    pub grid: Option<PathBuf>,
}

/// One source (a file, files joined, or a pair compared): its label and variables.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceInfo {
    pub path: PathBuf,
    pub label: String,
    pub vars: Vec<VarInfo>,
    /// The file compared against, in a comparison.
    pub other: Option<PathBuf>,
}

/// What a session knows about the opened files, without their data.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CatalogInfo {
    pub sources: Vec<SourceInfo>,
    pub compare: bool,
    pub show: Show,
    /// Files not opened, with the reason; warnings such as joined steps out of order.
    pub skipped: Vec<(PathBuf, String)>,
    pub notes: Vec<String>,
}

impl CatalogInfo {
    pub fn from_catalog(cat: &Catalog) -> Self {
        Self {
            sources: cat
                .sources
                .iter()
                .enumerate()
                .map(|(i, s)| SourceInfo {
                    // Absolute, so a client never needs the server's working folder.
                    path: std::fs::canonicalize(&s.path).unwrap_or_else(|_| s.path.clone()),
                    label: cat.label(i),
                    vars: s.reader.variables().to_vec(),
                    other: s.other.as_ref().map(|o| o.path.clone()),
                })
                .collect(),
            compare: cat.compare,
            show: cat.show,
            skipped: cat.skipped.clone(),
            notes: cat.notes.clone(),
        }
    }

    pub fn var(&self, r: &VarRef) -> Result<&VarInfo> {
        let Some(s) = self.sources.get(r.source) else {
            bail!("no source {}", r.source);
        };
        match s.vars.iter().find(|v| v.name == r.name) {
            Some(v) => Ok(v),
            None => bail!("no variable '{}' in {}", r.name, s.path.display()),
        }
    }

    /// Variables to offer for display: all but coordinate and grid mapping variables,
    /// source by source.
    pub fn data_vars(&self) -> Vec<(VarRef, &VarInfo)> {
        let mut out = vec![];
        for (i, s) in self.sources.iter().enumerate() {
            for v in s.vars.iter().filter(|v| nqv_core::is_data(v, &s.vars)) {
                let r = VarRef {
                    source: i,
                    name: v.name.clone(),
                };
                out.push((r, v));
            }
        }
        out
    }

    /// The coordinate variable for `dim` in a source, if it has one.
    pub fn coord_var(&self, source: usize, dim: &DimInfo) -> Option<&VarInfo> {
        self.sources[source]
            .vars
            .iter()
            .find(|v| v.name == dim.name && v.is_coordinate())
    }

    pub fn label(&self, source: usize) -> &str {
        &self.sources[source].label
    }
}

/// A map with everything drawn over it, in one request.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MapParts {
    pub raster: Raster,
    pub range: RangeEstimate,
    pub contours: Vec<ContourLine>,
    pub stats: Option<Stats>,
}

/// Browsing the file system the data is on.
pub trait Files {
    /// Folders first, then files, by name; hidden entries left out.
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>>;
    /// The variables of a NetCDF file.
    fn describe(&self, path: &Path) -> Result<Vec<VarInfo>>;
    /// The text of a recipe in ~/.nqv/recipes there, if there is one.
    fn recipe(&self, name: &str) -> Result<Option<String>>;
}

/// The file system of this machine.
pub struct LocalFiles;

impl Files for LocalFiles {
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        list_dir(path)
    }

    fn describe(&self, path: &Path) -> Result<Vec<VarInfo>> {
        Ok(NetcdfReader::open(path)?.variables().to_vec())
    }

    fn recipe(&self, name: &str) -> Result<Option<String>> {
        crate::recipe::text(name).map_err(anyhow::Error::msg)
    }
}

/// Data for a session. Every method is one request of the protocol a remote backend
/// speaks.
pub trait Backend: Files + Send {
    /// Open other paths in place of the current ones (the same connection, remotely).
    fn reopen(&mut self, paths: &[PathBuf], mode: &OpenMode) -> Result<()>;
    fn info(&self) -> &CatalogInfo;
    /// In a comparison, which side reads return.
    fn set_show(&mut self, show: Show);
    fn view(&self, spec: &ViewSpec) -> Result<Raster>;
    fn range(&self, spec: &ViewSpec) -> Result<RangeEstimate>;
    fn stats(&self, spec: &ViewSpec) -> Result<Stats>;
    fn contours(&self, spec: &ViewSpec, c: &ContourSpec) -> Result<Vec<ContourLine>>;
    fn cell_at(&self, spec: &ViewSpec, px: (u32, u32)) -> Option<(usize, usize)>;
    /// A series and its range (shared by A and B in a comparison).
    fn series(&self, spec: &SeriesSpec) -> Result<(Series, Option<(f32, f32)>)>;
    /// The value at `index`, or None where `mask` hides it.
    fn value(&self, var: &VarRef, index: &[usize], mask: &Mask) -> Result<Option<f32>>;
    fn coord(&self, source: usize, dim: &str) -> Option<Vec<f32>>;
    /// Whether `mask` and contours can be drawn over `var`.
    fn check_overlay(&self, var: &VarRef, mask: &Mask, contour: Option<&ContourSpec>)
    -> Result<()>;

    /// The map in `spec` with its range, contours and (if asked) statistics; one round
    /// trip for a remote backend. Contours or statistics that fail are left out.
    fn map_data(
        &self,
        spec: &ViewSpec,
        contour: Option<&ContourSpec>,
        with_stats: bool,
    ) -> Result<MapParts> {
        Ok(MapParts {
            raster: self.view(spec)?,
            range: self.range(spec)?,
            contours: contour
                .map(|c| self.contours(spec, c).unwrap_or_default())
                .unwrap_or_default(),
            stats: if with_stats {
                self.stats(spec).ok()
            } else {
                None
            },
        })
    }
}

/// Files read here.
pub struct Local {
    cat: Catalog,
    info: CatalogInfo,
}

impl Local {
    pub fn new(cat: Catalog) -> Self {
        let info = CatalogInfo::from_catalog(&cat);
        Self { cat, info }
    }

    /// Open `paths` as the command line does with `mode`.
    pub fn open(paths: &[PathBuf], mode: &OpenMode) -> Result<Self> {
        if let Some(p) = paths.iter().chain(&mode.vs).find(|p| !p.exists()) {
            bail!("{}: no such file or folder", p.display());
        }
        let side = |p: &[PathBuf]| match mode.join {
            true => Catalog::join(p, mode.join_dim.as_deref()),
            false => Catalog::open(p),
        };
        let mut cat = match (mode.diff, mode.vs.is_empty(), paths) {
            (true, false, _) => bail!("use --diff A B or A... --vs B..., not both"),
            (true, true, _) if mode.join => bail!("to compare joined files use -j A... --vs B..."),
            (true, true, [a, b]) => Catalog::compare(a, b)?,
            (true, true, _) => bail!("--diff needs two files or two folders"),
            (false, false, _) => Catalog::pair(side(paths)?, side(&mode.vs)?)?,
            (false, true, _) => side(paths)?,
        };
        if let Some(g) = &mode.grid {
            cat.set_grid(g)?;
        }
        Ok(Self::new(cat))
    }
}

impl Files for Local {
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        LocalFiles.list_dir(path)
    }

    fn describe(&self, path: &Path) -> Result<Vec<VarInfo>> {
        LocalFiles.describe(path)
    }

    fn recipe(&self, name: &str) -> Result<Option<String>> {
        LocalFiles.recipe(name)
    }
}

impl Backend for Local {
    fn reopen(&mut self, paths: &[PathBuf], mode: &OpenMode) -> Result<()> {
        *self = Local::open(paths, mode)?;
        Ok(())
    }

    fn info(&self) -> &CatalogInfo {
        &self.info
    }

    fn set_show(&mut self, show: Show) {
        self.cat.show = show;
        self.info.show = show;
    }

    fn view(&self, spec: &ViewSpec) -> Result<Raster> {
        view(&self.cat, spec)
    }

    fn range(&self, spec: &ViewSpec) -> Result<RangeEstimate> {
        range(&self.cat, spec)
    }

    fn stats(&self, spec: &ViewSpec) -> Result<Stats> {
        stats(&self.cat, spec)
    }

    fn contours(&self, spec: &ViewSpec, c: &ContourSpec) -> Result<Vec<ContourLine>> {
        contours(&self.cat, spec, c)
    }

    fn cell_at(&self, spec: &ViewSpec, px: (u32, u32)) -> Option<(usize, usize)> {
        cell_at(&self.cat, spec, px)
    }

    fn series(&self, spec: &SeriesSpec) -> Result<(Series, Option<(f32, f32)>)> {
        let s = series(&self.cat, spec)?;
        let r = series_range(&self.cat, spec, &s);
        Ok((s, r))
    }

    fn value(&self, var: &VarRef, index: &[usize], mask: &Mask) -> Result<Option<f32>> {
        if masked(&self.cat, var, index, mask)? {
            return Ok(None);
        }
        value(&self.cat, var, index).map(Some)
    }

    fn coord(&self, source: usize, dim: &str) -> Option<Vec<f32>> {
        let v = self
            .info
            .sources
            .get(source)?
            .vars
            .iter()
            .find(|v| v.name == dim)?;
        self.cat.coord(source, &v.dims[0])
    }

    fn check_overlay(
        &self,
        var: &VarRef,
        mask: &Mask,
        contour: Option<&ContourSpec>,
    ) -> Result<()> {
        check_mask(&self.cat, var, mask)?;
        if let Some(c) = contour {
            check_contour(&self.cat, var, c)?;
        }
        Ok(())
    }
}
