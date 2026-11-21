//! nqv-core: data access and view extraction. No display code.

mod netcdf;
mod view;

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

pub use crate::netcdf::NetcdfReader;
pub use crate::view::{
    RangeEstimate, Raster, Series, SeriesSpec, ViewSpec, finite_range, fit_size, range, series,
    value, view,
};

/// How a variable is shown, from its dimensions of length other than 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The last two such dimensions are (y, x).
    Map { y: usize, x: usize },
    /// One such dimension: a line plot along it.
    Series { along: usize },
    /// Every dimension has length 1.
    Scalar,
}

/// A dimension of a variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimInfo {
    pub name: String,
    pub len: usize,
}

/// Metadata of a numeric variable.
#[derive(Debug, Clone, PartialEq)]
pub struct VarInfo {
    pub name: String,
    pub dims: Vec<DimInfo>,
    pub long_name: Option<String>,
    pub units: Option<String>,
}

impl VarInfo {
    /// Dimensions of length 1 don't count, so `(time, lat=1, lon=1)` is a series.
    /// Empty dimensions (no records yet) do count, so they are reported, not hidden.
    pub fn kind(&self) -> Kind {
        let free: Vec<usize> = (0..self.dims.len())
            .filter(|&k| self.dims[k].len != 1)
            .collect();
        match free[..] {
            [] => Kind::Scalar,
            [along] => Kind::Series { along },
            [.., y, x] => Kind::Map { y, x },
        }
    }

    /// Dimensions shown by `kind()`.
    pub fn shown_dims(&self) -> Vec<usize> {
        match self.kind() {
            Kind::Map { y, x } => vec![y, x],
            Kind::Series { along } => vec![along],
            Kind::Scalar => vec![],
        }
    }

    /// Dimensions the user can step through: not shown, length above 1.
    pub fn step_dims(&self) -> Vec<usize> {
        let shown = self.shown_dims();
        (0..self.dims.len())
            .filter(|k| !shown.contains(k) && self.dims[*k].len > 1)
            .collect()
    }

    /// A coordinate variable: one dimension with the variable's own name.
    pub fn is_coordinate(&self) -> bool {
        self.dims.len() == 1 && self.dims[0].name == self.name
    }
}

/// Source of variables and hyperslabs. One implementation per file format.
pub trait Reader: Send + Sync {
    /// Numeric variables of the source.
    fn variables(&self) -> &[VarInfo];

    /// Read a strided hyperslab, CF-unpacked, with missing values as NaN.
    /// Returns `count.iter().product()` values in row-major order.
    fn read_slice(
        &self,
        var: &str,
        start: &[usize],
        count: &[usize],
        stride: &[usize],
    ) -> Result<Vec<f32>>;
}

/// An opened source with its variables. Single file for now; collections later.
pub struct Catalog {
    pub path: PathBuf,
    pub reader: Box<dyn Reader>,
}

impl Catalog {
    pub fn open(path: &Path) -> Result<Self> {
        let reader = NetcdfReader::open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            reader: Box::new(reader),
        })
    }

    pub fn var(&self, name: &str) -> Result<&VarInfo> {
        match self.reader.variables().iter().find(|v| v.name == name) {
            Some(v) => Ok(v),
            None => bail!("no variable '{name}' in {}", self.path.display()),
        }
    }

    /// Variables to offer for display: all but coordinate variables.
    pub fn data_vars(&self) -> Vec<&VarInfo> {
        self.reader
            .variables()
            .iter()
            .filter(|v| !v.is_coordinate())
            .collect()
    }

    /// The coordinate variable for `dim`, if the source has one.
    pub fn coord_var(&self, dim: &DimInfo) -> Option<&VarInfo> {
        self.reader
            .variables()
            .iter()
            .find(|v| v.name == dim.name && v.is_coordinate())
    }

    /// Values of the coordinate variable for `dim`, if the source has one.
    pub fn coord(&self, dim: &DimInfo) -> Option<Vec<f32>> {
        self.coord_var(dim)?;
        self.reader
            .read_slice(&dim.name, &[0], &[dim.len], &[1])
            .ok()
    }
}
