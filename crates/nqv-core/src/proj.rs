//! Map projections: longitude and latitude (degrees) to projected x and y (metres) and
//! back, for the grids of the files (CF `grid_mapping`) and the views asked for
//! (`--proj`). Written out from Snyder (1987), "Map projections: a working manual",
//! rather than bound to PROJ, so nqv stays one static binary; checked against PROJ.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use anyhow::{Context, Result, bail};

use crate::Attr;

/// The figure of the Earth: semi-major axis (metres) and flattening (0 for a sphere).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ellipsoid {
    pub a: f64,
    pub f: f64,
}

impl Ellipsoid {
    pub const WGS84: Self = Self {
        a: 6_378_137.0,
        f: 1.0 / 298.257_223_563,
    };
    /// The sphere of orthographic (globe) views.
    pub const SPHERE: Self = Self {
        a: 6_371_000.0,
        f: 0.0,
    };

    /// Eccentricity.
    fn e(&self) -> f64 {
        (2.0 * self.f - self.f * self.f).sqrt()
    }
}

/// How polar stereographic scale is set (CF's two variants).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StereScale {
    /// True scale along this latitude (degrees; variant B).
    StandardParallel(f64),
    /// Scale factor at the pole (variant A).
    K0(f64),
}

/// A map projection. `LatLon` is the identity (x = longitude, y = latitude, degrees);
/// the others give metres.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Projection {
    LatLon,
    PolarStereo {
        north: bool,
        /// The meridian pointing down (north) or up (south) from the pole.
        lon0: f64,
        scale: StereScale,
        ellipsoid: Ellipsoid,
        false_easting: f64,
        false_northing: f64,
    },
    /// Lambert azimuthal equal-area, centred on (`lon0`, `lat0`).
    Laea {
        lon0: f64,
        lat0: f64,
        ellipsoid: Ellipsoid,
        false_easting: f64,
        false_northing: f64,
    },
    /// The globe seen from far away, centred on (`lon0`, `lat0`); a sphere.
    Ortho {
        lon0: f64,
        lat0: f64,
    },
}

impl Projection {
    /// Projected x, y of a point; None where the projection does not show it (the far
    /// side of a globe, the opposite pole).
    pub fn forward(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        if !(lon.is_finite() && lat.is_finite()) || lat.abs() > 90.0 {
            return None;
        }
        match *self {
            Projection::LatLon => Some((lon, lat)),
            Projection::PolarStereo {
                north,
                lon0,
                scale,
                ellipsoid,
                false_easting,
                false_northing,
            } => {
                let s = if north { 1.0 } else { -1.0 };
                let phi = (s * lat).to_radians();
                if phi <= -FRAC_PI_2 + 1e-12 {
                    return None;
                }
                let rho = stere_rho_per_t(scale, ellipsoid, s) * stere_t(phi, ellipsoid.e());
                let dl = (lon - lon0).to_radians();
                Some((
                    false_easting + rho * dl.sin(),
                    false_northing - s * rho * dl.cos(),
                ))
            }
            Projection::Laea {
                lon0,
                lat0,
                ellipsoid,
                false_easting,
                false_northing,
            } => {
                let (x, y) = laea_forward(lon - lon0, lat, lat0, ellipsoid)?;
                Some((false_easting + x, false_northing + y))
            }
            Projection::Ortho { lon0, lat0 } => {
                let (phi, phi0) = (lat.to_radians(), lat0.to_radians());
                let dl = (lon - lon0).to_radians();
                let cos_c = phi0.sin() * phi.sin() + phi0.cos() * phi.cos() * dl.cos();
                if cos_c < 0.0 {
                    return None;
                }
                let r = Ellipsoid::SPHERE.a;
                Some((
                    r * phi.cos() * dl.sin(),
                    r * (phi0.cos() * phi.sin() - phi0.sin() * phi.cos() * dl.cos()),
                ))
            }
        }
    }

    /// Longitude (in -180..180 around the central meridian) and latitude of a projected
    /// point; None off the projection's area.
    pub fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        match *self {
            Projection::LatLon => (y.abs() <= 90.0).then_some((x, y)),
            Projection::PolarStereo {
                north,
                lon0,
                scale,
                ellipsoid,
                false_easting,
                false_northing,
            } => {
                let s = if north { 1.0 } else { -1.0 };
                let (x, y) = (x - false_easting, y - false_northing);
                let rho = x.hypot(y);
                let t = rho / stere_rho_per_t(scale, ellipsoid, s);
                let phi = phi_from_t(t, ellipsoid.e());
                let lon = if rho == 0.0 {
                    lon0
                } else {
                    lon0 + x.atan2(-s * y).to_degrees()
                };
                Some((wrap(lon, lon0), s * phi.to_degrees()))
            }
            Projection::Laea {
                lon0,
                lat0,
                ellipsoid,
                false_easting,
                false_northing,
            } => {
                let (dl, lat) =
                    laea_inverse(x - false_easting, y - false_northing, lat0, ellipsoid)?;
                Some((wrap(lon0 + dl, lon0), lat))
            }
            Projection::Ortho { lon0, lat0 } => {
                let r = Ellipsoid::SPHERE.a;
                let rho = x.hypot(y);
                if rho > r {
                    return None;
                }
                if rho == 0.0 {
                    return Some((lon0, lat0));
                }
                let phi0 = lat0.to_radians();
                let c = (rho / r).asin();
                let phi = (c.cos() * phi0.sin() + y * c.sin() * phi0.cos() / rho).asin();
                let dl = (x * c.sin()).atan2(rho * phi0.cos() * c.cos() - y * phi0.sin() * c.sin());
                Some((wrap(lon0 + dl.to_degrees(), lon0), phi.to_degrees()))
            }
        }
    }

    /// The projection a CF `grid_mapping` variable describes, from its attributes.
    pub fn from_cf(attrs: &[(String, Attr)]) -> Result<Self> {
        let text = |k: &str| {
            attrs.iter().find_map(|(n, a)| match a {
                Attr::Text(t) if n == k => Some(t.trim().to_string()),
                _ => None,
            })
        };
        let num = |k: &str| {
            attrs.iter().find_map(|(n, a)| match a {
                Attr::Num(v) if n == k => v.first().copied(),
                _ => None,
            })
        };
        let name = text("grid_mapping_name").context("no grid_mapping_name")?;
        let ellipsoid = cf_ellipsoid(&num);
        let fe = num("false_easting").unwrap_or(0.0);
        let fnorth = num("false_northing").unwrap_or(0.0);
        let need = |k: &str| num(k).with_context(|| format!("{name}: no {k}"));
        match name.as_str() {
            "latitude_longitude" => Ok(Projection::LatLon),
            "polar_stereographic" | "stereographic" => {
                let lat0 = need("latitude_of_projection_origin")?;
                if (lat0.abs() - 90.0).abs() > 1e-9 {
                    bail!("{name} centred at latitude {lat0}: only polar ones are supported");
                }
                let lon0 = num("straight_vertical_longitude_from_pole")
                    .or_else(|| num("longitude_of_projection_origin"))
                    .unwrap_or(0.0);
                let scale = match (
                    num("standard_parallel"),
                    num("scale_factor_at_projection_origin"),
                ) {
                    (Some(p), _) => StereScale::StandardParallel(p),
                    (None, Some(k)) => StereScale::K0(k),
                    (None, None) => StereScale::K0(1.0),
                };
                Ok(Projection::PolarStereo {
                    north: lat0 > 0.0,
                    lon0,
                    scale,
                    ellipsoid,
                    false_easting: fe,
                    false_northing: fnorth,
                })
            }
            "lambert_azimuthal_equal_area" => Ok(Projection::Laea {
                lon0: need("longitude_of_projection_origin")?,
                lat0: need("latitude_of_projection_origin")?,
                ellipsoid,
                false_easting: fe,
                false_northing: fnorth,
            }),
            "orthographic" => Ok(Projection::Ortho {
                lon0: need("longitude_of_projection_origin")?,
                lat0: need("latitude_of_projection_origin")?,
            }),
            other => bail!("grid_mapping {other} is not supported yet"),
        }
    }

    /// A projection to view maps in, as `--proj` and `:proj` give it:
    /// `latlon`, `stere north|south [lon0=L] [lat_ts=T | k0=K]` (WGS84; defaults as
    /// EPSG:3413 and 3031), `laea north|south [lon0=L]` (WGS84; as EASE-Grid 2.0),
    /// `laea LON,LAT`, `ortho LON,LAT`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let words: Vec<&str> = spec.split_whitespace().collect();
        let usage = "expected latlon | stere north|south [lon0=L] [lat_ts=T] | \
                     laea north|south [lon0=L] | laea LON,LAT | ortho LON,LAT";
        let Some((&kind, rest)) = words.split_first() else {
            return Err(usage.into());
        };
        let option = |key: &str| -> Result<Option<f64>, String> {
            for w in rest.iter().skip(1) {
                match w.split_once('=') {
                    Some((k, v)) if k == key => {
                        return v
                            .parse()
                            .map(Some)
                            .map_err(|_| format!("{key}: not a number: {v}"));
                    }
                    Some((k, _)) if ["lon0", "lat_ts", "k0"].contains(&k) => {}
                    _ => return Err(format!("unknown option {w}; {usage}")),
                }
            }
            Ok(None)
        };
        let pole = || match rest.first() {
            Some(&"north") => Ok(true),
            Some(&"south") => Ok(false),
            _ => Err(format!("{kind}: north or south, or LON,LAT; {usage}")),
        };
        let centre = || -> Result<(f64, f64), String> {
            let c = rest.first().ok_or(usage)?;
            let (lon, lat) = c.split_once(',').ok_or(format!("{c}: expected LON,LAT"))?;
            let n = |s: &str| {
                s.trim()
                    .parse::<f64>()
                    .map_err(|_| format!("not a number: {s}"))
            };
            let (lon, lat) = (n(lon)?, n(lat)?);
            if lat.abs() > 90.0 {
                return Err(format!("latitude {lat} beyond ±90"));
            }
            if rest.len() > 1 {
                return Err(format!("unexpected {}; {usage}", rest[1]));
            }
            Ok((lon, lat))
        };
        match kind {
            "latlon" if rest.is_empty() => Ok(Projection::LatLon),
            "stere" => {
                let north = pole()?;
                let lon0 = option("lon0")?.unwrap_or(if north { -45.0 } else { 0.0 });
                let scale = match (option("lat_ts")?, option("k0")?) {
                    (Some(_), Some(_)) => return Err("stere: lat_ts or k0, not both".into()),
                    (None, Some(k)) => StereScale::K0(k),
                    (t, None) => {
                        StereScale::StandardParallel(t.unwrap_or(if north { 70.0 } else { -71.0 }))
                    }
                };
                Ok(Projection::PolarStereo {
                    north,
                    lon0,
                    scale,
                    ellipsoid: Ellipsoid::WGS84,
                    false_easting: 0.0,
                    false_northing: 0.0,
                })
            }
            "laea" if matches!(rest.first(), Some(&"north" | &"south")) => {
                let north = pole()?;
                if option("lat_ts")?.is_some() || option("k0")?.is_some() {
                    return Err("laea takes only lon0".into());
                }
                Ok(Projection::Laea {
                    lon0: option("lon0")?.unwrap_or(0.0),
                    lat0: if north { 90.0 } else { -90.0 },
                    ellipsoid: Ellipsoid::WGS84,
                    false_easting: 0.0,
                    false_northing: 0.0,
                })
            }
            "laea" => {
                let (lon0, lat0) = centre()?;
                Ok(Projection::Laea {
                    lon0,
                    lat0,
                    ellipsoid: Ellipsoid::WGS84,
                    false_easting: 0.0,
                    false_northing: 0.0,
                })
            }
            "ortho" => {
                let (lon0, lat0) = centre()?;
                Ok(Projection::Ortho { lon0, lat0 })
            }
            _ => Err(usage.into()),
        }
    }

    /// The `parse` syntax for this projection (false easting and northing, and the
    /// ellipsoid, are not part of it).
    pub fn text(&self) -> String {
        let n = |v: f64| format!("{}", (v * 1e6).round() / 1e6);
        match *self {
            Projection::LatLon => "latlon".into(),
            Projection::PolarStereo {
                north, lon0, scale, ..
            } => {
                let pole = if north { "north" } else { "south" };
                let (dlon, dts) = if north { (-45.0, 70.0) } else { (0.0, -71.0) };
                let mut s = format!("stere {pole}");
                if lon0 != dlon {
                    s += &format!(" lon0={}", n(lon0));
                }
                match scale {
                    StereScale::StandardParallel(t) if t != dts => {
                        s += &format!(" lat_ts={}", n(t))
                    }
                    StereScale::K0(k) => s += &format!(" k0={}", n(k)),
                    StereScale::StandardParallel(_) => {}
                }
                s
            }
            Projection::Laea { lon0, lat0, .. } if lat0.abs() == 90.0 => {
                let pole = if lat0 > 0.0 { "north" } else { "south" };
                match lon0 {
                    0.0 => format!("laea {pole}"),
                    _ => format!("laea {pole} lon0={}", n(lon0)),
                }
            }
            Projection::Laea { lon0, lat0, .. } => format!("laea {},{}", n(lon0), n(lat0)),
            Projection::Ortho { lon0, lat0 } => format!("ortho {},{}", n(lon0), n(lat0)),
        }
    }
}

/// `lon` brought within 180 degrees of `centre`.
fn wrap(lon: f64, centre: f64) -> f64 {
    centre + (lon - centre + 180.0).rem_euclid(360.0) - 180.0
}

/// The ellipsoid CF attributes describe (`earth_radius`, or `semi_major_axis` with
/// `inverse_flattening` or `semi_minor_axis`); WGS84 when they say nothing.
fn cf_ellipsoid(num: &dyn Fn(&str) -> Option<f64>) -> Ellipsoid {
    if let Some(r) = num("earth_radius") {
        return Ellipsoid { a: r, f: 0.0 };
    }
    let Some(a) = num("semi_major_axis") else {
        return Ellipsoid::WGS84;
    };
    let f = match (num("inverse_flattening"), num("semi_minor_axis")) {
        (Some(inv), _) if inv != 0.0 => 1.0 / inv,
        (_, Some(b)) => (a - b) / a,
        _ => 0.0,
    };
    Ellipsoid { a, f }
}

/// Snyder (15-9): t of latitude `phi` (radians), from the pole.
fn stere_t(phi: f64, e: f64) -> f64 {
    let es = e * phi.sin();
    (FRAC_PI_4 - phi / 2.0).tan() / ((1.0 - es) / (1.0 + es)).powf(e / 2.0)
}

/// Polar stereographic: rho / t (rho = distance from the pole), for either variant.
/// `s` is 1 north, -1 south (a southern standard parallel is given negative).
fn stere_rho_per_t(scale: StereScale, el: Ellipsoid, s: f64) -> f64 {
    let e = el.e();
    match scale {
        StereScale::StandardParallel(ts) => {
            let phic = (s * ts).to_radians();
            if (phic - FRAC_PI_2).abs() < 1e-12 {
                return stere_rho_per_t(StereScale::K0(1.0), el, s);
            }
            let mc = phic.cos() / (1.0 - (e * phic.sin()).powi(2)).sqrt();
            el.a * mc / stere_t(phic, e)
        }
        StereScale::K0(k0) => {
            2.0 * el.a * k0 / ((1.0 + e).powf(1.0 + e) * (1.0 - e).powf(1.0 - e)).sqrt()
        }
    }
}

/// Snyder (7-9): latitude (radians) from t, by iteration.
fn phi_from_t(t: f64, e: f64) -> f64 {
    let mut phi = FRAC_PI_2 - 2.0 * t.atan();
    for _ in 0..15 {
        let es = e * phi.sin();
        let next = FRAC_PI_2 - 2.0 * (t * ((1.0 - es) / (1.0 + es)).powf(e / 2.0)).atan();
        if (next - phi).abs() < 1e-14 {
            return next;
        }
        phi = next;
    }
    phi
}

/// Snyder (3-12): q of latitude `phi` (radians), for authalic latitudes.
fn q(phi: f64, e: f64) -> f64 {
    let s = phi.sin();
    if e < 1e-12 {
        return 2.0 * s;
    }
    let es = e * s;
    (1.0 - e * e) * (s / (1.0 - es * es) - (1.0 / (2.0 * e)) * ((1.0 - es) / (1.0 + es)).ln())
}

/// Snyder (3-16): latitude (radians) from q, by iteration.
fn phi_from_q(q_: f64, e: f64) -> f64 {
    let mut phi = (q_ / 2.0).clamp(-1.0, 1.0).asin();
    if e < 1e-12 {
        return phi;
    }
    let e2 = e * e;
    for _ in 0..15 {
        let s = phi.sin();
        let es = e * s;
        let d = (1.0 - es * es).powi(2) / (2.0 * phi.cos())
            * (q_ / (1.0 - e2) - s / (1.0 - es * es)
                + (1.0 / (2.0 * e)) * ((1.0 - es) / (1.0 + es)).ln());
        phi += d;
        if d.abs() < 1e-14 {
            break;
        }
    }
    phi
}

/// Lambert azimuthal equal-area (Snyder 24): x, y of longitude `dl` from the centre
/// and latitude `lat`, centred at latitude `lat0` (degrees).
fn laea_forward(dl: f64, lat: f64, lat0: f64, el: Ellipsoid) -> Option<(f64, f64)> {
    let e = el.e();
    let (phi, dl) = (lat.to_radians(), dl.to_radians());
    let (qp, qq) = (q(FRAC_PI_2, e), q(phi, e));
    if (lat0.abs() - 90.0).abs() < 1e-12 {
        // polar aspect
        let s = lat0.signum();
        let rho2 = qp - s * qq;
        if rho2 <= 1e-15 && s * lat < 0.0 {
            return None;
        }
        let rho = el.a * rho2.max(0.0).sqrt();
        return Some((rho * dl.sin(), -s * rho * dl.cos()));
    }
    let phi1 = lat0.to_radians();
    let rq = el.a * (qp / 2.0).sqrt();
    let beta = (qq / qp).clamp(-1.0, 1.0).asin();
    let beta1 = (q(phi1, e) / qp).clamp(-1.0, 1.0).asin();
    let m1 = phi1.cos() / (1.0 - (e * phi1.sin()).powi(2)).sqrt();
    let d = el.a * m1 / (rq * beta1.cos());
    let den = 1.0 + beta1.sin() * beta.sin() + beta1.cos() * beta.cos() * dl.cos();
    if den <= 1e-15 {
        return None; // the antipode
    }
    let b = rq * (2.0 / den).sqrt();
    Some((
        b * d * beta.cos() * dl.sin(),
        (b / d) * (beta1.cos() * beta.sin() - beta1.sin() * beta.cos() * dl.cos()),
    ))
}

/// The inverse of `laea_forward`: longitude from the centre and latitude (degrees).
fn laea_inverse(x: f64, y: f64, lat0: f64, el: Ellipsoid) -> Option<(f64, f64)> {
    let e = el.e();
    let qp = q(FRAC_PI_2, e);
    if (lat0.abs() - 90.0).abs() < 1e-12 {
        let s = lat0.signum();
        let rho = x.hypot(y);
        let qq = s * (qp - (rho / el.a).powi(2));
        if qq.abs() > qp + 1e-12 {
            return None;
        }
        let lat = phi_from_q(qq.clamp(-qp, qp), e).to_degrees();
        let dl = if rho == 0.0 {
            0.0
        } else {
            x.atan2(-s * y).to_degrees()
        };
        return Some((dl, lat));
    }
    let phi1 = lat0.to_radians();
    let rq = el.a * (qp / 2.0).sqrt();
    let beta1 = (q(phi1, e) / qp).clamp(-1.0, 1.0).asin();
    let m1 = phi1.cos() / (1.0 - (e * phi1.sin()).powi(2)).sqrt();
    let d = el.a * m1 / (rq * beta1.cos());
    let rho = (x / d).hypot(d * y);
    if rho > 2.0 * rq {
        return None;
    }
    if rho == 0.0 {
        return Some((0.0, lat0));
    }
    let ce = 2.0 * (rho / (2.0 * rq)).asin();
    let qq = qp * (ce.cos() * beta1.sin() + d * y * ce.sin() * beta1.cos() / rho);
    let dl =
        (x * ce.sin()).atan2(d * rho * beta1.cos() * ce.cos() - d * d * y * beta1.sin() * ce.sin());
    Some((dl.to_degrees(), phi_from_q(qq, e).to_degrees()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(spec: &str) -> Projection {
        Projection::parse(spec).unwrap()
    }

    /// (lon, lat) and PROJ's (x, y) for it.
    type Case = ((f64, f64), (f64, f64));

    /// Forward against PROJ (cs2cs, 9.x) to a millimetre, and back to 1e-9 degrees.
    fn check(proj: &Projection, cases: &[Case]) {
        for &((lon, lat), (x, y)) in cases {
            let (px, py) = proj.forward(lon, lat).unwrap();
            assert!(
                (px - x).abs() < 1e-3 && (py - y).abs() < 1e-3,
                "{proj:?} at {lon},{lat}: {px},{py}, PROJ {x},{y}"
            );
            let (blon, blat) = proj.inverse(px, py).unwrap();
            assert!((blat - lat).abs() < 1e-9, "{proj:?}: lat {blat} for {lat}");
            if lat.abs() < 89.99 {
                let dlon = (blon - lon + 180.0).rem_euclid(360.0) - 180.0;
                assert!(dlon.abs() < 1e-9, "{proj:?}: lon {blon} for {lon}");
            }
        }
    }

    #[test]
    fn polar_stereographic_matches_proj() {
        // +proj=stere +lat_0=90 +lat_ts=70 +lon_0=-45 +ellps=WGS84 (EPSG:3413)
        check(
            &p("stere north"),
            &[
                ((-45.0, 60.0), (0.0, -3323160.270641)),
                ((10.0, 75.0), (1338395.730475, -937154.779586)),
                ((120.0, 85.5), (126229.554705, 471095.111577)),
                ((-170.0, 70.25), (-1769407.261397, 1238952.302593)),
                ((0.0, 89.999), (76.598983, -76.598983)),
            ],
        );
        // +proj=stere +lat_0=-90 +lat_ts=-71 +lon_0=0 +ellps=WGS84 (EPSG:3031)
        check(
            &p("stere south"),
            &[
                ((0.0, -60.0), (0.0, 3333134.027630)),
                ((120.0, -75.0), (1419227.915757, -819391.619204)),
                ((-100.0, -85.5), (-481745.613342, -84944.749470)),
                ((45.0, -70.0), (1551741.763759, 1551741.763759)),
            ],
        );
        // +proj=stere +lat_0=90 +k_0=0.994 +lon_0=0 +x_0=2000000 +y_0=2000000 (UPS north)
        let ups = Projection::PolarStereo {
            north: true,
            lon0: 0.0,
            scale: StereScale::K0(0.994),
            ellipsoid: Ellipsoid::WGS84,
            false_easting: 2e6,
            false_northing: 2e6,
        };
        check(
            &ups,
            &[
                ((-45.0, 60.0), (-408321.351166, -408321.351166)),
                ((10.0, 75.0), (2290782.589550, 350889.986423)),
                ((-170.0, 70.25), (1615574.965070, 4180182.711604)),
            ],
        );
    }

    #[test]
    fn lambert_azimuthal_equal_area_matches_proj() {
        // +proj=laea +lat_0=90 +lon_0=0 +ellps=WGS84 (EPSG:6931, EASE-Grid 2.0 North)
        check(
            &p("laea north"),
            &[
                ((-45.0, 60.0), (-2340395.849030, -2340395.849030)),
                ((10.0, 75.0), (290035.871117, -1644875.162903)),
                ((120.0, 85.5), (435163.306929, 251241.652397)),
                ((-170.0, 70.25), (-381018.543554, 2160863.538997)),
            ],
        );
        check(
            &p("laea south"),
            &[
                ((0.0, -60.0), (0.0, 3309819.551020)),
                ((120.0, -75.0), (1446478.942485, -835125.006821)),
                ((45.0, -70.0), (1570958.550113, 1570958.550113)),
            ],
        );
        // EPSG:3035 (GRS80): EPSG's own example, 50N 5E -> 3962799.45, 2999718.85
        let etrs = Projection::Laea {
            lon0: 10.0,
            lat0: 52.0,
            ellipsoid: Ellipsoid {
                a: 6_378_137.0,
                f: 1.0 / 298.257_222_101,
            },
            false_easting: 4_321_000.0,
            false_northing: 3_210_000.0,
        };
        check(
            &etrs,
            &[
                ((5.0, 50.0), (3962799.450955, 2999718.853160)),
                ((-10.0, 40.0), (2626483.081788, 2103594.616627)),
                ((30.0, 65.0), (5255425.632492, 4790028.961062)),
            ],
        );
    }

    #[test]
    fn orthographic_matches_proj() {
        // +proj=ortho +lat_0=60 +lon_0=-40 +R=6371000
        let o = p("ortho -40,60");
        check(
            &o,
            &[
                ((0.0, 45.0), (2895743.592249, -736176.127485)),
                ((-100.0, 30.0), (-4778250.0, -796375.0)),
            ],
        );
        // the far side is not shown
        assert!(o.forward(140.0, -60.0).is_none());
        assert!(o.inverse(7e6, 0.0).is_none());
    }

    #[test]
    fn specs_round_trip() {
        for s in [
            "latlon",
            "stere north",
            "stere south",
            "stere north lon0=-39 lat_ts=71",
            "stere south k0=0.994",
            "laea north",
            "laea south lon0=180",
            "laea 10,52",
            "ortho -40,60",
        ] {
            assert_eq!(p(s).text(), s);
        }
        for bad in [
            "",
            "mercator",
            "stere",
            "stere up",
            "ortho 1",
            "laea 0,95",
            "stere north k=1",
        ] {
            assert!(Projection::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn from_cf_attributes() {
        let a = |k: &str, v: f64| (k.to_string(), Attr::Num(vec![v]));
        // as Yelmo writes it
        let attrs = vec![
            (
                "grid_mapping_name".to_string(),
                Attr::Text("polar_stereographic".into()),
            ),
            a("latitude_of_projection_origin", 90.0),
            a("straight_vertical_longitude_from_pole", -45.0),
            a("standard_parallel", 70.0),
            a("semi_major_axis", 6378137.0),
            a("inverse_flattening", 298.257223563),
            a("false_easting", 0.0),
            a("false_northing", 0.0),
        ];
        assert_eq!(Projection::from_cf(&attrs).unwrap(), p("stere north"));
        let bad = vec![(
            "grid_mapping_name".to_string(),
            Attr::Text("transverse_mercator".into()),
        )];
        assert!(Projection::from_cf(&bad).is_err());
    }
}
