//! `Reader` for NetCDF classic and NetCDF-4 files via the pure-Rust `netcdf-reader`.

use std::path::Path;

use anyhow::{Context, Result};
use netcdf_reader::{NcFile, NcSliceInfo, NcSliceInfoElem, NcType, NcVariable};

use crate::{DimInfo, Reader, VarInfo};

pub struct NetcdfReader {
    file: NcFile,
    vars: Vec<VarInfo>,
}

impl NetcdfReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = NcFile::open(path).with_context(|| format!("opening {}", path.display()))?;
        let vars = file
            .variables()
            .context("reading variable list")?
            .iter()
            .filter(|v| is_numeric(v.dtype()))
            .map(var_info)
            .collect();
        Ok(Self { file, vars })
    }
}

impl Reader for NetcdfReader {
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
        let selections = start
            .iter()
            .zip(count)
            .zip(stride)
            .map(|((&s, &c), &st)| NcSliceInfoElem::Slice {
                start: s as u64,
                end: (s + (c.max(1) - 1) * st + 1) as u64,
                step: st as u64,
            })
            .collect();
        let data = self
            .file
            .read_variable_slice_unpacked_masked(var, &NcSliceInfo { selections })
            .with_context(|| format!("reading {var}"))?;
        Ok(data.iter().map(|&v| v as f32).collect())
    }
}

fn is_numeric(t: &NcType) -> bool {
    use NcType::*;
    matches!(
        t,
        Byte | Short | Int | Float | Double | UByte | UShort | UInt | Int64 | UInt64
    )
}

fn var_info(v: &NcVariable) -> VarInfo {
    let text = |name: &str| v.attribute(name).and_then(|a| a.value.as_string());
    VarInfo {
        name: v.name().to_string(),
        dims: v
            .dimensions()
            .iter()
            .map(|d| DimInfo {
                name: d.name.clone(),
                len: d.size as usize,
                unlimited: d.is_unlimited,
            })
            .collect(),
        long_name: text("long_name"),
        units: text("units"),
        cell_measures: text("cell_measures"),
    }
}
