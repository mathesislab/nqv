//! Open a file and time a view of each variable: `probe FILE [W H]`.
use std::time::Instant;

use nqv_core::{Catalog, Kind, SeriesSpec, ViewSpec, finite_range, series, value, view};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&args[1]);
    let w: u32 = args.get(2).map_or(Ok(400), |s| s.parse())?;
    let h: u32 = args.get(3).map_or(Ok(200), |s| s.parse())?;
    let t0 = Instant::now();
    let cat = Catalog::open(&path)?;
    println!("open: {:?}", t0.elapsed());
    for v in cat.data_vars() {
        let index = v
            .dims
            .iter()
            .map(|d| d.len.saturating_sub(1))
            .collect::<Vec<_>>();
        let t = Instant::now();
        let res = match v.kind() {
            Kind::Map { .. } => view(
                &cat,
                &ViewSpec {
                    var: v.name.clone(),
                    index,
                    size: (w, h),
                },
            )
            .map(|r| format!("map {}x{} {:?}", r.w, r.h, finite_range(&r.values))),
            Kind::Series { along } => {
                let spec = SeriesSpec {
                    var: v.name.clone(),
                    along,
                    index,
                    max_points: 2000,
                };
                series(&cat, &spec)
                    .map(|s| format!("series {} {:?}", s.y.len(), finite_range(&s.y)))
            }
            Kind::Scalar => value(&cat, &v.name, &index).map(|x| format!("scalar {x}")),
        };
        let dims: Vec<usize> = v.dims.iter().map(|d| d.len).collect();
        match res {
            Ok(s) => println!("{:<24} {:?} {:>9.2?}  {s}", v.name, dims, t.elapsed()),
            Err(e) => println!("{:<24} {:?} error: {e:#}", v.name, dims),
        }
    }
    Ok(())
}
