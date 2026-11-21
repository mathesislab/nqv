# Data built into nqv

`coast50.bin`: coastlines of [Natural Earth](https://www.naturalearthdata.com/) 1:50m
(`ne_50m_coastline.geojson` from github.com/nvkelso/natural-earth-vector, downloaded
2026-11-21), public domain. Made with

```bash
cargo run -p nqv-core --example coastlines -- ne_50m_coastline.geojson crates/nqv-core/data/coast50.bin
```

Format: see `examples/coastlines.rs`.
