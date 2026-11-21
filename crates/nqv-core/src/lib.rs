//! nqv-core: data access and view extraction. No display code.

mod netcdf;
mod view;

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

pub use crate::netcdf::NetcdfReader;
pub use crate::view::{RangeEstimate, Raster, ViewSpec, fit_size, range, view};

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
    /// A variable is shown as a map when it has at least two dimensions;
    /// the last two are (y, x).
    pub fn is_map(&self) -> bool {
        self.dims.len() >= 2
    }

    /// Dimensions not displayed in a map view (all but the last two).
    pub fn leading_dims(&self) -> &[DimInfo] {
        &self.dims[..self.dims.len().saturating_sub(2)]
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

    /// Variables that can be shown as maps, coordinate variables excluded.
    pub fn map_vars(&self) -> Vec<&VarInfo> {
        self.reader
            .variables()
            .iter()
            .filter(|v| v.is_map())
            .collect()
    }

    /// Values of the coordinate variable for `dim`, if the source has one.
    pub fn coord(&self, dim: &DimInfo) -> Option<Vec<f32>> {
        let v = self
            .reader
            .variables()
            .iter()
            .find(|v| v.name == dim.name)?;
        if v.dims.len() != 1 || v.dims[0].name != dim.name {
            return None;
        }
        self.reader
            .read_slice(&dim.name, &[0], &[dim.len], &[1])
            .ok()
    }
}
