//! Opened files: one file, a collection (folder or file list), or a comparison of two
//! files or two folders.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::geo::{geo_as, geo_of};
use crate::{
    DimInfo, Geo, JoinedReader, Kind, NetcdfReader, Projection, Reader, VarInfo, record_dim,
};

/// A variable in a catalog: index of its source and its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VarRef {
    pub source: usize,
    pub name: String,
}

/// What a comparison shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Show {
    /// First minus second.
    Diff,
    A,
    B,
}

/// The file compared against, in a comparison.
pub struct Other {
    pub path: PathBuf,
    pub reader: Box<dyn Reader>,
}

/// One file, and in a comparison its counterpart (if found and readable).
pub struct Source {
    /// The file, or the first of the files joined.
    pub path: PathBuf,
    pub reader: Box<dyn Reader>,
    pub other: Option<Other>,
    /// Number of files joined into this source (1 for a plain file).
    pub joined: usize,
}

pub struct Catalog {
    pub sources: Vec<Source>,
    /// Two files or two folders were opened: reads follow `show`.
    pub compare: bool,
    pub show: Show,
    /// Files that were not opened, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
    /// Warnings worth showing, such as time steps out of order between joined files.
    pub notes: Vec<String>,
    /// Where maps whose own files lack their geography get it (`--grid`).
    pub grid: Option<Grid>,
    pub(crate) cache: crate::reproject::Cache,
}

/// Whether `v` is data to show: neither a coordinate variable nor the grid mapping of
/// another of `vars`.
pub fn is_data(v: &VarInfo, vars: &[VarInfo]) -> bool {
    !v.is_coordinate()
        && !vars
            .iter()
            .any(|o| o.grid_mapping.as_deref() == Some(v.name.as_str()))
}

/// The geography of maps whose files lack it (`--grid`).
pub enum Grid {
    /// The grids of another file, matched by size.
    File(PathBuf, Box<dyn Reader>),
    /// This projection, with the maps' own x/y coordinates.
    Proj(Projection),
}

/// A directory entry for file browsing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_netcdf: bool,
    pub size: u64,
}

pub fn is_netcdf(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("nc" | "nc4")
    )
}

/// Folders first, then files, each sorted by name. Hidden entries are left out.
pub fn list_dir(path: &Path) -> Result<Vec<DirEntry>> {
    let mut out = vec![];
    for e in std::fs::read_dir(path)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let meta = std::fs::metadata(&path)?; // follows symlinks
        out.push(DirEntry {
            is_netcdf: !meta.is_dir() && is_netcdf(&path),
            is_dir: meta.is_dir(),
            size: meta.len(),
            name,
            path,
        });
    }
    out.sort_by(|a, b| (!a.is_dir, &a.name).cmp(&(!b.is_dir, &b.name)));
    Ok(out)
}

/// NetCDF files in a folder, sorted by name.
pub fn netcdf_files(dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(list_dir(dir)?
        .into_iter()
        .filter(|e| e.is_netcdf)
        .map(|e| e.path)
        .collect())
}

fn open_reader(path: &Path) -> Result<Box<dyn Reader>> {
    Ok(Box::new(NetcdfReader::open(path)?))
}

impl Source {
    fn single(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            reader: open_reader(path)?,
            other: None,
            joined: 1,
        })
    }

    /// `files` joined along `dim` (default: the record dimension of the first file),
    /// with warnings. A single file is opened as it is.
    fn joined(files: &[PathBuf], dim: Option<&str>) -> Result<(Self, Vec<String>)> {
        if let [f] = files {
            return Ok((Self::single(f)?, vec![]));
        }
        let mut readers = vec![];
        for f in files {
            let r = open_reader(f).map_err(|e| anyhow::anyhow!("{}: {e:#}", f.display()))?;
            readers.push((f.clone(), r));
        }
        let dim = match dim {
            Some(d) => d.to_string(),
            None => match record_dim(readers[0].1.as_ref()) {
                Some(d) => d,
                None => bail!(
                    "{}: no record dimension to join along; name one with --join-dim",
                    files[0].display()
                ),
            },
        };
        let (reader, notes) = JoinedReader::new(readers, &dim)?;
        let source = Self {
            path: files[0].clone(),
            joined: reader.len(),
            reader: Box::new(reader),
            other: None,
        };
        Ok((source, notes))
    }
}

impl Catalog {
    /// One file, or a collection of the given files and the NetCDF files in the given
    /// folders. A single file must be readable; in a collection unreadable files are
    /// listed in `skipped`.
    pub fn open(paths: &[PathBuf]) -> Result<Self> {
        match paths {
            [] => bail!("nothing to open"),
            [p] if !p.is_dir() => Ok(Self::new(vec![Source::single(p)?], false, vec![])),
            _ => {
                let mut files = vec![];
                for p in paths {
                    if p.is_dir() {
                        files.extend(netcdf_files(p)?);
                    } else {
                        files.push(p.clone());
                    }
                }
                let what = match paths {
                    [p] => p.display().to_string(),
                    _ => "the given paths".into(),
                };
                Self::collection(&files, &what)
            }
        }
    }

    /// Comparison of two files, or of two folders file by file (paired by name).
    pub fn compare(a: &Path, b: &Path) -> Result<Self> {
        if a.is_dir() != b.is_dir() {
            bail!("compare two files or two folders, not a file with a folder");
        }
        Self::pair(
            Self::open(&[a.to_path_buf()])?,
            Self::open(&[b.to_path_buf()])?,
        )
    }

    /// Comparison of two opened catalogs (collections or joined files): sources paired
    /// by file name, or the two sources whatever their names when each side has one.
    /// Sources only in `b` are listed in `skipped`.
    pub fn pair(mut a: Catalog, b: Catalog) -> Result<Self> {
        let one_each = a.sources.len() == 1 && b.sources.len() == 1;
        let mut others: Vec<Option<Source>> = b.sources.into_iter().map(Some).collect();
        for s in &mut a.sources {
            let k = match one_each {
                true => Some(0),
                false => others.iter().position(|o| {
                    o.as_ref()
                        .is_some_and(|o| o.path.file_name() == s.path.file_name())
                }),
            };
            if let Some(o) = k.and_then(|k| others[k].take()) {
                s.other = Some(Other {
                    path: o.path,
                    reader: o.reader,
                });
            }
        }
        a.skipped.extend(b.skipped);
        a.skipped.extend(
            others
                .into_iter()
                .flatten()
                .map(|o| (o.path, "only in the second set of files".into())),
        );
        a.notes.extend(b.notes);
        a.compare = true;
        a.show = Show::Diff;
        Ok(a)
    }

    fn new(sources: Vec<Source>, compare: bool, skipped: Vec<(PathBuf, String)>) -> Self {
        Self {
            sources,
            compare,
            show: Show::Diff,
            skipped,
            notes: vec![],
            grid: None,
            cache: Default::default(),
        }
    }

    /// Take the geography of maps without one from the grids of `path` (`--grid`): a
    /// file with a CF grid mapping and projected coordinates, or latitude-longitude
    /// coordinates, on grids of the same size.
    pub fn set_grid(&mut self, path: &Path) -> Result<()> {
        let r = NetcdfReader::open(path)?;
        let any = r
            .variables()
            .iter()
            .any(|v| geo_of(&r, v).is_ok_and(|g| g.is_some()));
        if !any {
            bail!(
                "{}: no grid mapping with x/y coordinates, nor latitude and longitude",
                path.display()
            );
        }
        self.grid = Some(Grid::File(path.to_path_buf(), Box::new(r)));
        self.cache = Default::default();
        Ok(())
    }

    /// Take maps without a geography to be in `proj`, their x/y coordinates projected
    /// ones (as `--grid "stere north"` for files naming a grid mapping they lack).
    pub fn set_grid_proj(&mut self, proj: Projection) {
        self.grid = Some(Grid::Proj(proj));
        self.cache = Default::default();
    }

    /// Where the cells of map variable `r` lie: from its own file, else from `grid` (a
    /// grid file's grid of the same size, or a projection for its own coordinates). None
    /// when neither says; an error when the file names a projection nqv cannot use.
    pub fn geo(&self, r: &VarRef) -> Result<Option<Geo>> {
        let v = self.var(r)?;
        let reader = self.sources[r.source].reader.as_ref();
        if let Some(g) = geo_of(reader, v)? {
            return Ok(Some(g));
        }
        let (path, grid) = match &self.grid {
            None => return Ok(None),
            Some(Grid::Proj(p)) => return Ok(geo_as(reader, v, p)),
            Some(Grid::File(path, grid)) => (path, grid),
        };
        let Kind::Map { y, x } = v.kind() else {
            return Ok(None);
        };
        let size = (v.dims[y].len, v.dims[x].len);
        for gv in grid.variables() {
            let Kind::Map { y, x } = gv.kind() else {
                continue;
            };
            if (gv.dims[y].len, gv.dims[x].len) == size
                && let Some(g) =
                    geo_of(grid.as_ref(), gv).with_context(|| path.display().to_string())?
            {
                return Ok(Some(g));
            }
        }
        Ok(None)
    }

    /// Files joined along `dim` (default: the record dimension) into one source; or, for
    /// folders, the files of the same name across them joined, each name its own source.
    /// Joining given files fails on the first that does not fit; in folders such a file
    /// set falls back to its first file, the others listed in `skipped`.
    pub fn join(paths: &[PathBuf], dim: Option<&str>) -> Result<Self> {
        let dirs = paths.iter().filter(|p| p.is_dir()).count();
        if dirs == 0 {
            let (source, notes) = Source::joined(paths, dim)?;
            let mut cat = Self::new(vec![source], false, vec![]);
            cat.notes = notes;
            return Ok(cat);
        }
        if dirs != paths.len() {
            bail!("join either files or folders, not both");
        }
        let mut groups: Vec<Vec<PathBuf>> = vec![];
        for d in paths {
            for f in netcdf_files(d)? {
                match groups
                    .iter_mut()
                    .find(|g| g[0].file_name() == f.file_name())
                {
                    Some(g) => g.push(f),
                    None => groups.push(vec![f]),
                }
            }
        }
        let (mut sources, mut skipped, mut notes) = (vec![], vec![], vec![]);
        for g in groups {
            match Source::joined(&g, dim) {
                Ok((s, n)) => {
                    sources.push(s);
                    notes.extend(n);
                }
                Err(e) => {
                    let why = format!("not joined: {e:#}");
                    match Source::single(&g[0]) {
                        Ok(s) => sources.push(s),
                        Err(e) => skipped.push((g[0].clone(), format!("{e:#}"))),
                    }
                    skipped.extend(g[1..].iter().map(|f| (f.clone(), why.clone())));
                }
            }
        }
        if sources.is_empty() {
            bail!("no readable NetCDF files in the given folders");
        }
        let mut cat = Self::new(sources, false, skipped);
        cat.notes = notes;
        Ok(cat)
    }

    fn collection(files: &[PathBuf], what: &str) -> Result<Self> {
        let (mut sources, mut skipped) = (vec![], vec![]);
        for f in files {
            match Source::single(f) {
                Ok(s) => sources.push(s),
                Err(e) => skipped.push((f.clone(), format!("{e:#}"))),
            }
        }
        if sources.is_empty() {
            match skipped.first() {
                Some((p, e)) => bail!("no readable NetCDF files in {what}: {}: {e}", p.display()),
                None => bail!("no NetCDF files in {what}"),
            }
        }
        Ok(Self::new(sources, false, skipped))
    }

    /// In a comparison showing A or B, the other one: their colour ranges are shared.
    pub fn other_side(&self) -> Option<Show> {
        match (self.compare, self.show) {
            (true, Show::A) => Some(Show::B),
            (true, Show::B) => Some(Show::A),
            _ => None,
        }
    }

    /// File name of a source, with its folder when another source has the same file
    /// name; in a comparison "a.nc - b.nc" when the names differ.
    pub fn label(&self, source: usize) -> String {
        let name = |p: &Path| {
            p.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let s = &self.sources[source];
        let clash = self
            .sources
            .iter()
            .enumerate()
            .any(|(i, o)| i != source && name(&o.path) == name(&s.path));
        let mut own = match s.path.parent().map(name) {
            Some(dir) if clash && !dir.is_empty() => format!("{dir}/{}", name(&s.path)),
            _ => name(&s.path),
        };
        if s.joined > 1 {
            own = format!("{own} x{}", s.joined);
        }
        match &s.other {
            Some(o) if name(&o.path) != name(&s.path) => format!("{own} - {}", name(&o.path)),
            _ => own,
        }
    }

    /// Metadata of a variable, from the first file of its source.
    pub fn var(&self, r: &VarRef) -> Result<&VarInfo> {
        let Some(s) = self.sources.get(r.source) else {
            bail!("no source {}", r.source);
        };
        match s.reader.variables().iter().find(|v| v.name == r.name) {
            Some(v) => Ok(v),
            None => bail!("no variable '{}' in {}", r.name, s.path.display()),
        }
    }

    /// Variables to offer for display: all but coordinate and grid mapping variables,
    /// source by source.
    pub fn data_vars(&self) -> Vec<(VarRef, &VarInfo)> {
        let mut out = vec![];
        for (i, s) in self.sources.iter().enumerate() {
            let vars = s.reader.variables();
            for v in vars.iter().filter(|v| is_data(v, vars)) {
                out.push((
                    VarRef {
                        source: i,
                        name: v.name.clone(),
                    },
                    v,
                ));
            }
        }
        out
    }

    /// The coordinate variable for `dim` in a source, if it has one.
    pub fn coord_var(&self, source: usize, dim: &DimInfo) -> Option<&VarInfo> {
        self.sources[source]
            .reader
            .variables()
            .iter()
            .find(|v| v.name == dim.name && v.is_coordinate())
    }

    /// Values of the coordinate variable for `dim`, if the source has one.
    pub fn coord(&self, source: usize, dim: &DimInfo) -> Option<Vec<f32>> {
        self.coord_var(source, dim)?;
        let r = VarRef {
            source,
            name: dim.name.clone(),
        };
        self.read(&r, &[0], &[dim.len], &[1]).ok()
    }

    /// Read a strided hyperslab as `Reader::read_slice` does. In a comparison the
    /// values come from the first file, the second, or their difference, following
    /// `show`; coordinate variables are never differenced.
    pub fn read(
        &self,
        r: &VarRef,
        start: &[usize],
        count: &[usize],
        stride: &[usize],
    ) -> Result<Vec<f32>> {
        self.read_as(self.show, r, start, count, stride)
    }

    /// `read` with `show` given instead of taken from the catalog.
    pub fn read_as(
        &self,
        show: Show,
        r: &VarRef,
        start: &[usize],
        count: &[usize],
        stride: &[usize],
    ) -> Result<Vec<f32>> {
        let var = self.var(r)?;
        let s = &self.sources[r.source];
        let a = || s.reader.read_slice(&r.name, start, count, stride);
        if !self.compare || show == Show::A || (show == Show::Diff && var.is_coordinate()) {
            return a();
        }
        let Some(o) = &s.other else {
            bail!(
                "{}: no readable counterpart to compare with",
                s.path.display()
            );
        };
        let Some(bv) = o.reader.variables().iter().find(|v| v.name == r.name) else {
            bail!("'{}' is not in {}", r.name, o.path.display());
        };
        if bv.dims != var.dims {
            bail!(
                "'{}' differs in dimensions: {} vs {}",
                r.name,
                dims_text(&var.dims),
                dims_text(&bv.dims)
            );
        }
        let b = o.reader.read_slice(&r.name, start, count, stride)?;
        if show == Show::B {
            return Ok(b);
        }
        Ok(a()?.iter().zip(&b).map(|(x, y)| x - y).collect())
    }
}

fn dims_text(dims: &[DimInfo]) -> String {
    let d: Vec<String> = dims
        .iter()
        .map(|d| format!("{}={}", d.name, d.len))
        .collect();
    format!("({})", d.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reader with the given variable names, all scalars.
    struct Names(Vec<VarInfo>);

    impl Reader for Names {
        fn variables(&self) -> &[VarInfo] {
            &self.0
        }
        fn read_slice(&self, _: &str, _: &[usize], c: &[usize], _: &[usize]) -> Result<Vec<f32>> {
            Ok(vec![0.0; c.iter().product()])
        }
    }

    fn side(files: &[&str]) -> Catalog {
        let sources = files
            .iter()
            .map(|f| Source {
                path: PathBuf::from(f),
                reader: Box::new(Names(vec![])),
                other: None,
                joined: 1,
            })
            .collect();
        Catalog::new(sources, false, vec![])
    }

    #[test]
    fn pairs_sources_by_file_name() {
        let c = Catalog::pair(side(&["A/x.nc", "A/y.nc"]), side(&["B/y.nc", "B/z.nc"])).unwrap();
        assert!(c.compare);
        let others: Vec<Option<PathBuf>> = c
            .sources
            .iter()
            .map(|s| s.other.as_ref().map(|o| o.path.clone()))
            .collect();
        assert_eq!(others, [None, Some(PathBuf::from("B/y.nc"))]);
        assert_eq!(c.skipped.len(), 1);
        assert_eq!(c.skipped[0].0, PathBuf::from("B/z.nc"));
        // One source on each side: paired whatever the names.
        let c = Catalog::pair(side(&["a.nc"]), side(&["b.nc"])).unwrap();
        assert!(c.sources[0].other.is_some());
        assert_eq!(c.label(0), "a.nc - b.nc");
    }

    #[test]
    fn netcdf_extensions() {
        assert!(is_netcdf(Path::new("a/b.nc")));
        assert!(is_netcdf(Path::new("b.nc4")));
        assert!(!is_netcdf(Path::new("b.nml")));
        assert!(!is_netcdf(Path::new("nc")));
    }

    #[test]
    fn list_dir_sorts_and_classifies() {
        let d = std::env::temp_dir().join(format!("nqv-test-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("b.nc"), b"x").unwrap();
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        std::fs::write(d.join(".hidden.nc"), b"x").unwrap();
        let names: Vec<_> = list_dir(&d)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.is_dir, e.is_netcdf))
            .collect();
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(
            names,
            [
                ("sub".to_string(), true, false),
                ("a.txt".to_string(), false, false),
                ("b.nc".to_string(), false, true)
            ]
        );
    }
}
