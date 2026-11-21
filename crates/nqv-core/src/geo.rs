//! Where a map's cells lie on the Earth: the projection of its grid and the cells'
//! centre coordinates in it, from the file's CF metadata (a `grid_mapping` with
//! projected x/y coordinates, or latitude-longitude coordinates), else from a grid file
//! given with `--grid`.

use anyhow::{Context, Result};

use crate::{Kind, Projection, Reader, VarInfo};

/// A map grid's geography: its projection and the centre coordinates of its columns
/// (`x`) and rows (`y`), in the projection's units (metres; degrees for `LatLon`).
#[derive(Debug, Clone, PartialEq)]
pub struct Geo {
    pub proj: Projection,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
}

/// The geography of the map variable `var` of `reader`: from its CF `grid_mapping` and
/// the projected coordinates of its map dimensions, else from latitude and longitude
/// coordinates. None when the file says nothing usable (no grid mapping variable, no
/// coordinates); an error when it describes a projection nqv cannot use.
pub(crate) fn geo_of(reader: &dyn Reader, var: &VarInfo) -> Result<Option<Geo>> {
    let Kind::Map { y, x } = var.kind() else {
        return Ok(None);
    };
    let (ydim, xdim) = (&var.dims[y].name, &var.dims[x].name);
    let (Some((yv, yc)), Some((xv, xc))) = (coord(reader, ydim), coord(reader, xdim)) else {
        return Ok(None);
    };
    if let Some(gm) = &var.grid_mapping {
        let attrs = reader.attributes(gm);
        // A grid_mapping named but not in the file (as in CLIMBER-X's ice output) says
        // nothing: the coordinates or a grid file may.
        if !attrs.is_empty() {
            let proj =
                Projection::from_cf(&attrs).with_context(|| format!("{}: {gm}", var.name))?;
            return Ok(with_proj(proj, (&yv, yc), (&xv, xc)));
        }
    }
    if is_lat(&yv) && is_lon(&xv) {
        return Ok(Some(Geo {
            proj: Projection::LatLon,
            x: xc,
            y: yc,
        }));
    }
    Ok(None)
}

/// The map variable `var` of `reader` taken to be in `proj` (`--grid SPEC`), its own
/// coordinates the projected x and y. None without coordinates (or units nqv knows).
pub(crate) fn geo_as(reader: &dyn Reader, var: &VarInfo, proj: &Projection) -> Option<Geo> {
    let Kind::Map { y, x } = var.kind() else {
        return None;
    };
    let y = coord(reader, &var.dims[y].name)?;
    let x = coord(reader, &var.dims[x].name)?;
    with_proj(proj.clone(), (&y.0, y.1), (&x.0, x.1))
}

/// Coordinates in `proj`: degrees for latitude-longitude, else converted to metres.
fn with_proj(proj: Projection, y: (&VarInfo, Vec<f64>), x: (&VarInfo, Vec<f64>)) -> Option<Geo> {
    if proj == Projection::LatLon {
        return Some(Geo {
            proj,
            x: x.1,
            y: y.1,
        });
    }
    let (fy, fx) = (metres(y.0)?, metres(x.0)?);
    Some(Geo {
        proj,
        x: x.1.iter().map(|v| v * fx).collect(),
        y: y.1.iter().map(|v| v * fy).collect(),
    })
}

/// The coordinate variable of dimension `dim` and its values.
fn coord(reader: &dyn Reader, dim: &str) -> Option<(VarInfo, Vec<f64>)> {
    let v = reader
        .variables()
        .iter()
        .find(|v| v.name == dim && v.is_coordinate())?;
    let values = reader.read_slice(dim, &[0], &[v.dims[0].len], &[1]).ok()?;
    if values.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some((v.clone(), values.into_iter().map(f64::from).collect()))
}

/// Metres per unit of a projected coordinate: its units (m, km), or metres when it has
/// none. None for other units.
fn metres(v: &VarInfo) -> Option<f64> {
    let u = v.units.as_deref().unwrap_or("m").trim().to_lowercase();
    match u.as_str() {
        "m" | "meter" | "meters" | "metre" | "metres" | "1" => Some(1.0),
        "km" | "kilometer" | "kilometers" | "kilometre" | "kilometres" => Some(1000.0),
        _ => None,
    }
}

fn is_lat(v: &VarInfo) -> bool {
    let u = v.units.as_deref().unwrap_or("").to_lowercase();
    (u.starts_with("degree") && (u.ends_with("north") || u.ends_with("_n")))
        || matches!(v.name.to_lowercase().as_str(), "lat" | "latitude")
}

fn is_lon(v: &VarInfo) -> bool {
    let u = v.units.as_deref().unwrap_or("").to_lowercase();
    (u.starts_with("degree") && (u.ends_with("east") || u.ends_with("_e")))
        || matches!(v.name.to_lowercase().as_str(), "lon" | "longitude")
}
