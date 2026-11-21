//! Opened files: one file, a collection (folder or file list), or a comparison of two
//! files or two folders.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::{DimInfo, NetcdfReader, Reader, VarInfo};

/// A variable in a catalog: index of its source and its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VarRef {
    pub source: usize,
    pub name: String,
}

/// What a comparison shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub path: PathBuf,
    pub reader: Box<dyn Reader>,
    pub other: Option<Other>,
}

pub struct Catalog {
    pub sources: Vec<Source>,
    /// Two files or two folders were opened: reads follow `show`.
    pub compare: bool,
    pub show: Show,
    /// Files that were not opened, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
}

/// A directory entry for file browsing.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        })
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
        match (a.is_dir(), b.is_dir()) {
            (false, false) => {
                let mut s = Source::single(a)?;
                s.other = Some(Other {
                    path: b.to_path_buf(),
                    reader: open_reader(b)?,
                });
                Ok(Self::new(vec![s], true, vec![]))
            }
            (true, true) => Self::compare_dirs(a, b),
            _ => bail!("compare two files or two folders, not a file with a folder"),
        }
    }

    fn new(sources: Vec<Source>, compare: bool, skipped: Vec<(PathBuf, String)>) -> Self {
        Self {
            sources,
            compare,
            show: Show::Diff,
            skipped,
        }
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

    fn compare_dirs(a: &Path, b: &Path) -> Result<Self> {
        let a_files = netcdf_files(a)?;
        let b_files = netcdf_files(b)?;
        let (pairs, only_b) = pair_by_name(&a_files, &b_files);
        let firsts: Vec<PathBuf> = pairs.iter().map(|p| p.0.clone()).collect();
        let mut cat = Self::collection(&firsts, &a.display().to_string())?;
        cat.compare = true;
        for s in &mut cat.sources {
            let Some((_, Some(bp))) = pairs.iter().find(|p| p.0 == s.path) else {
                continue;
            };
            match open_reader(bp) {
                Ok(reader) => {
                    s.other = Some(Other {
                        path: bp.clone(),
                        reader,
                    })
                }
                Err(e) => cat.skipped.push((bp.clone(), format!("{e:#}"))),
            }
        }
        cat.skipped.extend(
            only_b
                .into_iter()
                .map(|p| (p, "only in the second folder".into())),
        );
        Ok(cat)
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
        let own = match s.path.parent().map(name) {
            Some(dir) if clash && !dir.is_empty() => format!("{dir}/{}", name(&s.path)),
            _ => name(&s.path),
        };
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

    /// Variables to offer for display: all but coordinate variables, source by source.
    pub fn data_vars(&self) -> Vec<(VarRef, &VarInfo)> {
        let mut out = vec![];
        for (i, s) in self.sources.iter().enumerate() {
            for v in s.reader.variables().iter().filter(|v| !v.is_coordinate()) {
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
        let var = self.var(r)?;
        let s = &self.sources[r.source];
        let a = || s.reader.read_slice(&r.name, start, count, stride);
        if !self.compare || self.show == Show::A || (self.show == Show::Diff && var.is_coordinate())
        {
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
        if self.show == Show::B {
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

/// Files of `a` with the file of the same name in `b`, if any, and the files only in `b`.
fn pair_by_name(a: &[PathBuf], b: &[PathBuf]) -> (Vec<(PathBuf, Option<PathBuf>)>, Vec<PathBuf>) {
    let pairs: Vec<_> = a
        .iter()
        .map(|p| {
            (
                p.clone(),
                b.iter().find(|q| q.file_name() == p.file_name()).cloned(),
            )
        })
        .collect();
    let only_b = b
        .iter()
        .filter(|q| !a.iter().any(|p| p.file_name() == q.file_name()))
        .cloned()
        .collect();
    (pairs, only_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_files_by_name() {
        let p = |s: &str| PathBuf::from(s);
        let (pairs, only_b) =
            pair_by_name(&[p("A/x.nc"), p("A/y.nc")], &[p("B/y.nc"), p("B/z.nc")]);
        assert_eq!(
            pairs,
            [(p("A/x.nc"), None), (p("A/y.nc"), Some(p("B/y.nc")))]
        );
        assert_eq!(only_b, [p("B/z.nc")]);
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
