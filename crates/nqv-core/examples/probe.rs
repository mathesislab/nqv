//! Open paths as `nqv` does and time a view of each variable: `probe [--diff] PATH...`.
//! With `--diff`, two files or two folders are compared (first minus second). Output size from
//! `PROBE_SIZE=WxH` (default 400x200).
use std::time::Instant;

use nqv_core::{Catalog, Kind, SeriesSpec, ViewSpec, finite_range, series, value, view};

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let diff = args.first().is_some_and(|a| a == "--diff");
    if diff {
        args.remove(0);
    }
    let paths: Vec<std::path::PathBuf> = args.into_iter().map(Into::into).collect();
    let size = std::env::var("PROBE_SIZE").unwrap_or_else(|_| "400x200".into());
    let (w, h) = size.split_once('x').expect("PROBE_SIZE=WxH");
    let size: (u32, u32) = (w.parse()?, h.parse()?);
    let t0 = Instant::now();
    let cat = match (diff, &paths[..]) {
        (true, [a, b]) => Catalog::compare(a, b)?,
        (true, _) => anyhow::bail!("--diff needs two paths"),
        (false, _) => Catalog::open(&paths)?,
    };
    println!("open: {:?}, compare: {}", t0.elapsed(), cat.compare);
    for (p, why) in &cat.skipped {
        println!("skipped {}: {why}", p.display());
    }
    for (r, v) in cat.data_vars() {
        let index: Vec<usize> = v.dims.iter().map(|d| d.len.saturating_sub(1)).collect();
        let t = Instant::now();
        let res = match v.kind() {
            Kind::Map { .. } => {
                let spec = ViewSpec {
                    var: r.clone(),
                    index,
                    size,
                };
                view(&cat, &spec)
                    .map(|r| format!("map {}x{} {:?}", r.w, r.h, finite_range(&r.values)))
            }
            Kind::Series { along } => {
                let spec = SeriesSpec {
                    var: r.clone(),
                    along,
                    index,
                    max_points: 2000,
                };
                series(&cat, &spec)
                    .map(|s| format!("series {} {:?}", s.y.len(), finite_range(&s.y)))
            }
            Kind::Scalar => value(&cat, &r, &index).map(|x| format!("scalar {x}")),
        };
        let dims: Vec<usize> = v.dims.iter().map(|d| d.len).collect();
        let name = format!("{}:{}", cat.label(r.source), v.name);
        match res {
            Ok(s) => println!("{name:<40} {dims:?} {:>9.2?}  {s}", t.elapsed()),
            Err(e) => println!("{name:<40} {dims:?} error: {e:#}"),
        }
    }
    Ok(())
}
