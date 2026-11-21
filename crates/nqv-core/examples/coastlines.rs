//! Turn Natural Earth coastlines (GeoJSON, public domain) into the file nqv embeds
//! (`data/coast50.bin`, read by `nqv_core::coastlines`):
//!
//! ```text
//! cargo run -p nqv-core --example coastlines -- ne_50m_coastline.geojson crates/nqv-core/data/coast50.bin
//! ```
//!
//! Format, little-endian: "NQVCOAST", u32 number of lines; per line u32 number of points,
//! then the points as i32 longitude and latitude in 1e-5 degrees. Only the GeoJSON's
//! "coordinates" arrays are read (LineString and MultiLineString), with a parser of
//! nested number arrays: no JSON library for one file made once.

use std::io::Write;

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = &args[..] else {
        eprintln!("usage: coastlines IN.geojson OUT.bin");
        std::process::exit(2);
    };
    let text = std::fs::read(input)?;
    let mut lines: Vec<Vec<(f64, f64)>> = vec![];
    let key = b"\"coordinates\"";
    let mut i = 0;
    while let Some(k) = find(&text[i..], key) {
        i += k + key.len();
        while text[i] != b'[' {
            i += 1;
        }
        let value = parse(&text, &mut i);
        collect(&value, &mut lines);
    }
    let points: usize = lines.iter().map(Vec::len).sum();
    let mut out = std::io::BufWriter::new(std::fs::File::create(output)?);
    out.write_all(b"NQVCOAST")?;
    out.write_all(&(lines.len() as u32).to_le_bytes())?;
    for l in &lines {
        out.write_all(&(l.len() as u32).to_le_bytes())?;
        for &(lon, lat) in l {
            for v in [lon, lat] {
                out.write_all(&((v * 1e5).round() as i32).to_le_bytes())?;
            }
        }
    }
    out.flush()?;
    eprintln!("{} lines, {points} points -> {output}", lines.len());
    Ok(())
}

/// A JSON value of the "coordinates" kind: a number or an array of them, nested.
enum Node {
    Num(f64),
    List(Vec<Node>),
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse(s: &[u8], i: &mut usize) -> Node {
    while s[*i].is_ascii_whitespace() {
        *i += 1;
    }
    if s[*i] == b'[' {
        *i += 1;
        let mut items = vec![];
        loop {
            while s[*i].is_ascii_whitespace() || s[*i] == b',' {
                *i += 1;
            }
            if s[*i] == b']' {
                *i += 1;
                return Node::List(items);
            }
            items.push(parse(s, i));
        }
    }
    let start = *i;
    while matches!(s[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
        *i += 1;
    }
    let t = std::str::from_utf8(&s[start..*i]).expect("ascii number");
    Node::Num(t.parse().unwrap_or_else(|_| panic!("not a number: {t}")))
}

/// Lines (arrays of [lon, lat] points) found in `n`, at any depth.
fn collect(n: &Node, lines: &mut Vec<Vec<(f64, f64)>>) {
    let Node::List(items) = n else { return };
    let point = |p: &Node| match p {
        Node::List(c) => match c.as_slice() {
            [Node::Num(x), Node::Num(y), ..] => Some((*x, *y)),
            _ => None,
        },
        Node::Num(_) => None,
    };
    let pts: Option<Vec<(f64, f64)>> = items.iter().map(point).collect();
    match pts {
        Some(p) if p.len() >= 2 => lines.push(p),
        Some(_) => {}
        None => items.iter().for_each(|c| collect(c, lines)),
    }
}
