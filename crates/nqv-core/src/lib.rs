//! nqv-core: data access and view extraction. No display code.

mod catalog;
mod contour;
mod join;
mod mask;
mod netcdf;
mod proj;
mod view;

use anyhow::Result;

pub use crate::catalog::{
    Catalog, DirEntry, Other, Show, Source, VarRef, is_netcdf, list_dir, netcdf_files,
};
pub use crate::contour::{
    ContourLine, ContourSpec, Levels, Segment, check_contour, contours, nice_ticks,
};
pub use crate::join::{JoinedReader, record_dim};
pub use crate::mask::{Cond, Mask, Op, check_mask};
pub use crate::netcdf::NetcdfReader;
pub use crate::proj::{Ellipsoid, Projection, StereScale};
pub use crate::view::{
    Interp, RangeEstimate, Raster, Region, Series, SeriesSpec, Stats, ViewSpec, cell_at,
    finite_range, fit_size, masked, range, series, series_range, stats, value, view,
};

/// A value of a NetCDF attribute.
#[derive(Debug, Clone, PartialEq)]
pub enum Attr {
    Text(String),
    Num(Vec<f64>),
}

/// How a variable is shown, from its dimensions of length other than 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DimInfo {
    pub name: String,
    pub len: usize,
    /// The record dimension, which can grow (and along which files are joined).
    pub unlimited: bool,
}

/// Metadata of a numeric variable.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VarInfo {
    pub name: String,
    pub dims: Vec<DimInfo>,
    pub long_name: Option<String>,
    pub units: Option<String>,
    /// CF `cell_measures`, e.g. "area: cell_area".
    pub cell_measures: Option<String>,
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
