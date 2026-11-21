//! Masks: hide values where conditions on other variables hold.

use anyhow::{Result, bail};

use crate::{Catalog, Kind, Show, VarInfo, VarRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Equal to any of the values.
    Eq,
    /// Equal to none of the values.
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    pub fn symbol(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
        }
    }
}

/// `var op values`, e.g. `mask_ice != 2` or `basins = 1,3`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cond {
    pub var: VarRef,
    pub op: Op,
    pub values: Vec<f32>,
}

impl Cond {
    /// Whether the condition holds for `x`; never for missing values.
    pub fn holds(&self, x: f32) -> bool {
        if !x.is_finite() {
            return false;
        }
        let v = self.values.first().copied().unwrap_or(f32::NAN);
        match self.op {
            Op::Eq => self.values.contains(&x),
            Op::Ne => !self.values.contains(&x),
            Op::Lt => x < v,
            Op::Le => x <= v,
            Op::Gt => x > v,
            Op::Ge => x >= v,
        }
    }
}

/// Values are hidden where all conditions hold. No conditions: nothing hidden.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mask {
    pub conds: Vec<Cond>,
}

impl Mask {
    pub fn is_empty(&self) -> bool {
        self.conds.is_empty()
    }

    /// Set `values[i]` to NaN where all conditions hold; `cond_values[c][i]` is the value
    /// of the variable of condition `c` at element `i`.
    pub(crate) fn apply(&self, values: &mut [f32], cond_values: &[Vec<f32>]) {
        for (i, x) in values.iter_mut().enumerate() {
            if self
                .conds
                .iter()
                .zip(cond_values)
                .all(|(c, v)| c.holds(v[i]))
            {
                *x = f32::NAN;
            }
        }
    }
}

/// The comparison side a mask variable is read from: masks of a difference come from A.
pub(crate) fn mask_show(show: Show) -> Show {
    match show {
        Show::Diff => Show::A,
        s => s,
    }
}

/// Indices into the mask variable `m` for the shown variable `v` at `index`: shared
/// dimensions (same name and length) follow `index`, others must have length 1.
pub(crate) fn mask_index(v: &VarInfo, index: &[usize], m: &VarInfo) -> Result<Vec<usize>> {
    m.dims
        .iter()
        .map(|d| match v.dims.iter().position(|e| e.name == d.name) {
            Some(k) if v.dims[k].len == d.len => Ok(index[k]),
            Some(_) => bail!(
                "mask: '{}' has {} = {}, '{}' has {}",
                m.name,
                d.name,
                d.len,
                v.name,
                v.dims
                    .iter()
                    .find(|e| e.name == d.name)
                    .map_or(0, |e| e.len)
            ),
            None if d.len == 1 => Ok(0),
            None => bail!(
                "mask: '{}' has dimension '{}', which '{}' does not",
                m.name,
                d.name,
                v.name
            ),
        })
        .collect()
}

/// Check that `mask` can be applied to `var`: every mask variable's dimensions are
/// dimensions of `var` or have length 1, and for a map it lies on the same map grid.
pub fn check_mask(cat: &Catalog, var: &VarRef, mask: &Mask) -> Result<()> {
    let v = cat.var(var)?;
    for c in &mask.conds {
        let m = cat.var(&c.var)?;
        mask_index(v, &vec![0; v.dims.len()], m)?;
        if let Kind::Map { y, x } = v.kind() {
            let same = match m.kind() {
                Kind::Map { y: my, x: mx } => m.dims[my] == v.dims[y] && m.dims[mx] == v.dims[x],
                _ => false,
            };
            if !same {
                bail!(
                    "mask: '{}' is not on the grid of '{}' ({}, {})",
                    m.name,
                    v.name,
                    v.dims[y].name,
                    v.dims[x].name
                );
            }
        }
    }
    Ok(())
}
