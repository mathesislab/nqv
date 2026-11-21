//! Text for numbers and positions, the same in every front end.

use nqv_core::DimInfo;

/// Position `i` along dimension `d`, 1-based, with its coordinate value if known.
pub fn dim_at(d: &DimInfo, i: usize, coord: Option<f32>) -> String {
    let val = coord.map(|c| format!(" ({})", fmt(c))).unwrap_or_default();
    format!("{}={}/{}{val}", d.name, i + 1, d.len)
}

/// A longitude and latitude as `72.31°N 38.45°W`.
pub fn lonlat(lon: f64, lat: f64) -> String {
    let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
    let ns = if lat < 0.0 { 'S' } else { 'N' };
    let ew = if lon < 0.0 { 'W' } else { 'E' };
    format!("{:.2}°{ns} {:.2}°{ew}", lat.abs(), lon.abs())
}

/// Compact number format for labels.
pub fn fmt(v: f32) -> String {
    let a = v.abs();
    if !v.is_finite() {
        format!("{v}")
    } else if a != 0.0 && !(1e-3..1e5).contains(&a) {
        format!("{v:.3e}")
    } else {
        // Four significant digits.
        let decimals = if a == 0.0 {
            0
        } else {
            (3 - a.log10().floor() as i32).max(0) as usize
        };
        let s = format!("{v:.decimals$}");
        if !s.contains('.') {
            return if s == "-0" { "0".into() } else { s };
        }
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" { "0".into() } else { s.into() }
    }
}
