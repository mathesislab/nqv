//! Open a file, list map variables, and time a view of each: `probe FILE [W H]`.
use std::time::Instant;

use nqv_core::{Catalog, ViewSpec, range, view};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&args[1]);
    let w: u32 = args.get(2).map_or(Ok(400), |s| s.parse())?;
    let h: u32 = args.get(3).map_or(Ok(200), |s| s.parse())?;
    let t0 = Instant::now();
    let cat = Catalog::open(&path)?;
    println!("open: {:?}", t0.elapsed());
    for v in cat.map_vars() {
        let fixed = v
            .leading_dims()
            .iter()
            .map(|d| d.len.saturating_sub(1))
            .collect();
        let spec = ViewSpec {
            var: v.name.clone(),
            fixed,
            size: (w, h),
        };
        let t = Instant::now();
        match view(&cat, &spec).and_then(|r| Ok((r, range(&cat, &spec)?))) {
            Ok((r, rg)) => println!(
                "{:<24} {:?} view {:>9.2?}  raster {:?} {}x{}  range [{}, {}]",
                v.name,
                v.dims.iter().map(|d| d.len).collect::<Vec<_>>(),
                t.elapsed(),
                r.finite_range(),
                r.w,
                r.h,
                rg.min,
                rg.max
            ),
            Err(e) => println!("{:<24} error: {e:#}", v.name),
        }
    }
    Ok(())
}
