//! Several files read as one, joined along a dimension (usually the record dimension,
//! time): for runs written in pieces or restarted in segments.

use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::{DimInfo, Reader, VarInfo};

/// The record (unlimited) dimension of a reader, if it has one.
pub fn record_dim(r: &dyn Reader) -> Option<String> {
    r.variables()
        .iter()
        .flat_map(|v| &v.dims)
        .find(|d| d.unlimited)
        .map(|d| d.name.clone())
}

struct Part {
    reader: Box<dyn Reader>,
    /// Position of this file's first step along the joined dimension.
    start: usize,
    len: usize,
}

pub struct JoinedReader {
    dim: String,
    parts: Vec<Part>,
    vars: Vec<VarInfo>,
}

impl JoinedReader {
    /// Join `files` along `dim`. All files must have the same variables with the same
    /// dimensions, apart from the length of `dim`. Files are put in order of their first
    /// coordinate value along `dim` (when there is a coordinate variable); files with no
    /// steps along `dim` are left out. Returns the reader and warnings (steps out of order).
    pub fn new(files: Vec<(PathBuf, Box<dyn Reader>)>, dim: &str) -> Result<(Self, Vec<String>)> {
        let Some((first_path, first)) = files.first() else {
            bail!("nothing to join");
        };
        let template = first.variables().to_vec();
        let first_name = first_path.display().to_string();
        let mut parts: Vec<(PathBuf, Part, Option<(f32, f32)>)> = vec![];
        let mut notes = vec![];
        for (path, reader) in files {
            check_same(&template, reader.variables(), dim).map_err(|e| {
                anyhow::anyhow!("{}: {e} (compared with {first_name})", path.display())
            })?;
            let Some(len) = dim_len(reader.variables(), dim) else {
                bail!("{}: no dimension '{dim}' to join along", path.display());
            };
            if len == 0 {
                notes.push(format!(
                    "{}: no steps along '{dim}', left out",
                    path.display()
                ));
                continue;
            }
            let span = coord_span(reader.as_ref(), dim, len);
            parts.push((
                path,
                Part {
                    reader,
                    start: 0,
                    len,
                },
                span,
            ));
        }
        if parts.is_empty() {
            bail!("no steps along '{dim}' in any file");
        }
        // Stable: files without a coordinate keep their order.
        parts.sort_by(|a, b| match (a.2, b.2) {
            (Some(x), Some(y)) => x.0.total_cmp(&y.0),
            _ => std::cmp::Ordering::Equal,
        });
        for w in parts.windows(2) {
            if let (Some(a), Some(b)) = (w[0].2, w[1].2)
                && b.0 <= a.1
            {
                notes.push(format!(
                    "'{dim}' does not increase from {} ({}) to {} ({})",
                    w[0].0.display(),
                    a.1,
                    w[1].0.display(),
                    b.0
                ));
            }
        }
        let mut parts: Vec<Part> = parts.into_iter().map(|p| p.1).collect();
        let mut total = 0;
        for p in &mut parts {
            p.start = total;
            total += p.len;
        }
        let vars = template
            .into_iter()
            .map(|mut v| {
                for d in v.dims.iter_mut().filter(|d| d.name == dim) {
                    d.len = total;
                }
                v
            })
            .collect();
        let joined = Self {
            dim: dim.to_string(),
            parts,
            vars,
        };
        Ok((joined, notes))
    }

    /// Number of files joined.
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

impl Reader for JoinedReader {
    fn variables(&self) -> &[VarInfo] {
        &self.vars
    }

    fn read_slice(
        &self,
        var: &str,
        start: &[usize],
        count: &[usize],
        stride: &[usize],
    ) -> Result<Vec<f32>> {
        let Some(v) = self.vars.iter().find(|v| v.name == var) else {
            bail!("no variable '{var}'");
        };
        let Some(k) = v.dims.iter().position(|d| d.name == self.dim) else {
            // Not along the joined dimension: the same in every file.
            return self.parts[0].reader.read_slice(var, start, count, stride);
        };
        let outer: usize = count[..k].iter().product();
        let inner: usize = count[k + 1..].iter().product();
        let mut out = vec![f32::NAN; outer * count[k] * inner];
        let (s, st) = (start[k], stride[k].max(1));
        for p in &self.parts {
            // Requested steps j with s + j * st in [p.start, p.start + p.len).
            let j0 = p.start.saturating_sub(s).div_ceil(st);
            let end = p.start + p.len;
            if s + j0 * st >= end || j0 >= count[k] {
                continue;
            }
            let n = ((end - 1 - (s + j0 * st)) / st + 1).min(count[k] - j0);
            let (mut ls, mut lc) = (start.to_vec(), count.to_vec());
            ls[k] = s + j0 * st - p.start;
            lc[k] = n;
            let data = p.reader.read_slice(var, &ls, &lc, stride)?;
            for o in 0..outer {
                let dst = (o * count[k] + j0) * inner;
                let src = o * n * inner;
                out[dst..dst + n * inner].copy_from_slice(&data[src..src + n * inner]);
            }
        }
        Ok(out)
    }
}

/// Length of dimension `dim` in the variables, if any variable has it.
fn dim_len(vars: &[VarInfo], dim: &str) -> Option<usize> {
    vars.iter()
        .flat_map(|v| &v.dims)
        .find(|d| d.name == dim)
        .map(|d| d.len)
}

/// First and last coordinate value along `dim`, if there is a coordinate variable.
fn coord_span(r: &dyn Reader, dim: &str, len: usize) -> Option<(f32, f32)> {
    r.variables()
        .iter()
        .find(|v| v.name == dim && v.is_coordinate())?;
    let first = r.read_slice(dim, &[0], &[1], &[1]).ok()?;
    let last = r.read_slice(dim, &[len - 1], &[1], &[1]).ok()?;
    Some((first[0], last[0]))
}

/// Same variables with the same dimensions, apart from the length of `dim`.
fn check_same(
    template: &[VarInfo],
    vars: &[VarInfo],
    dim: &str,
) -> std::result::Result<(), String> {
    let shape = |ds: &[DimInfo]| -> Vec<(String, Option<usize>)> {
        ds.iter()
            .map(|d| (d.name.clone(), (d.name != dim).then_some(d.len)))
            .collect()
    };
    for t in template {
        let Some(v) = vars.iter().find(|v| v.name == t.name) else {
            return Err(format!("no variable '{}'", t.name));
        };
        if shape(&v.dims) != shape(&t.dims) {
            return Err(format!("'{}' has other dimensions", t.name));
        }
    }
    if let Some(v) = vars
        .iter()
        .find(|v| !template.iter().any(|t| t.name == v.name))
    {
        return Err(format!("extra variable '{}'", v.name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "v"(time, x) = 10 * global time + x, and coordinate "time" = global time, for
    /// global steps `t0..t0 + nt`.
    struct Piece {
        vars: Vec<VarInfo>,
        t0: usize,
    }

    fn piece(t0: usize, nt: usize, nx: usize) -> Box<dyn Reader> {
        let d = |n: &str, len, unlimited| DimInfo {
            name: n.into(),
            len,
            unlimited,
        };
        let v = |n: &str, dims| VarInfo {
            name: n.into(),
            dims,
            long_name: None,
            units: None,
        };
        Box::new(Piece {
            vars: vec![
                v("v", vec![d("time", nt, true), d("x", nx, false)]),
                v("time", vec![d("time", nt, true)]),
                v("x", vec![d("x", nx, false)]),
            ],
            t0,
        })
    }

    impl Reader for Piece {
        fn variables(&self) -> &[VarInfo] {
            &self.vars
        }
        fn read_slice(
            &self,
            var: &str,
            s: &[usize],
            c: &[usize],
            st: &[usize],
        ) -> Result<Vec<f32>> {
            let t = |j: usize| (self.t0 + s[0] + j * st[0]) as f32;
            Ok(match var {
                "time" => (0..c[0]).map(t).collect(),
                "x" => (0..c[0]).map(|j| (s[0] + j * st[0]) as f32).collect(),
                _ => (0..c[0])
                    .flat_map(|j| (0..c[1]).map(move |i| 10.0 * t(j) + (s[1] + i * st[1]) as f32))
                    .collect(),
            })
        }
    }

    fn files(pieces: Vec<(usize, usize)>) -> Vec<(PathBuf, Box<dyn Reader>)> {
        pieces
            .into_iter()
            .map(|(t0, nt)| (PathBuf::from(format!("p{t0}.nc")), piece(t0, nt, 3)))
            .collect()
    }

    #[test]
    fn joins_in_coordinate_order_with_strides() {
        // Given out of order: steps 3..5, 0..3, 5..9.
        let (j, notes) = JoinedReader::new(files(vec![(3, 2), (0, 3), (5, 4)]), "time").unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(j.len(), 3);
        assert_eq!(j.variables()[0].dims[0].len, 9);
        let time = j.read_slice("time", &[0], &[9], &[1]).unwrap();
        assert_eq!(time, (0..9).map(|t| t as f32).collect::<Vec<_>>());
        // Steps 1, 3, 5, 7 cross every file boundary; x = 2 only.
        let v = j.read_slice("v", &[1, 2], &[4, 1], &[2, 1]).unwrap();
        assert_eq!(v, [12.0, 32.0, 52.0, 72.0]);
        // Two x values per step keep their layout.
        let v = j.read_slice("v", &[2, 0], &[2, 2], &[1, 2]).unwrap();
        assert_eq!(v, [20.0, 22.0, 30.0, 32.0]);
        // Variables without the joined dimension come from the first file.
        assert_eq!(
            j.read_slice("x", &[0], &[3], &[1]).unwrap(),
            [0.0, 1.0, 2.0]
        );
    }

    #[test]
    fn reports_overlap_and_mismatch() {
        let (_, notes) = JoinedReader::new(files(vec![(0, 3), (2, 3)]), "time").unwrap();
        assert_eq!(notes.len(), 1, "{notes:?}");
        let mut f = files(vec![(0, 3)]);
        f.push((PathBuf::from("other.nc"), piece(3, 2, 4)));
        let e = JoinedReader::new(f, "time").err().unwrap().to_string();
        assert!(
            e.contains("other.nc") && e.contains("other dimensions"),
            "{e}"
        );
    }

    #[test]
    fn finds_record_dim() {
        assert_eq!(record_dim(piece(0, 1, 1).as_ref()).as_deref(), Some("time"));
    }
}
