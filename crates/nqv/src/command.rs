//! Commands typed after `:` in the viewer. The command-line flags `-v`, `-t` and
//! `--range` are parsed by the same functions, so both share one vocabulary.

/// Position along the first steppable dimension (usually time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeSel {
    /// Nearest coordinate value (or 1-based index without a coordinate variable).
    Value(f64),
    /// 1-based index; negative counts from the end (`@-1` is the last).
    Index(isize),
}

/// How the colour (or y-axis) range is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RangeMode {
    /// From the data shown.
    Auto,
    /// From the data shown, symmetric around zero.
    Symmetric,
    /// Fixed by the user.
    Fixed(f32, f32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Variable, matched like the picker (`file:var`, fuzzy).
    Var(String),
    Time(TimeSel),
    Range(RangeMode),
    Quit,
}

pub const HELP: &str = "v NAME | t VALUE | t @INDEX | range MIN MAX | range auto | range sym | q";

/// Parse a command line such as `v H_ice`, `t 1990`, `t @-1`, `range -2 30`.
pub fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let (name, arg) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let arg = arg.trim();
    match name {
        "v" | "var" if !arg.is_empty() => Ok(Command::Var(arg.to_string())),
        "t" | "time" => parse_time(arg).map(Command::Time),
        "range" => parse_range(arg).map(Command::Range),
        "q" | "quit" => Ok(Command::Quit),
        "" => Err(format!("commands: {HELP}")),
        _ => Err(format!("unknown command '{line}'; commands: {HELP}")),
    }
}

/// `1990` (coordinate value) or `@12`, `@-1` (index).
pub fn parse_time(arg: &str) -> Result<TimeSel, String> {
    let bad = || format!("t: expected a value or @index, got '{arg}'");
    match arg.strip_prefix('@') {
        Some(i) => match i.parse::<isize>() {
            Ok(0) | Err(_) => Err(bad()),
            Ok(i) => Ok(TimeSel::Index(i)),
        },
        None => arg.parse().map(TimeSel::Value).map_err(|_| bad()),
    }
}

/// `auto`, `sym`, or two numbers separated by a space or a comma: `-2 30`, `-2,30`.
pub fn parse_range(arg: &str) -> Result<RangeMode, String> {
    match arg {
        "auto" => return Ok(RangeMode::Auto),
        "sym" | "symmetric" => return Ok(RangeMode::Symmetric),
        _ => {}
    }
    let nums: Vec<&str> = arg
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    let bad = || format!("range: expected MIN MAX, auto or sym, got '{arg}'");
    match nums[..] {
        [a, b] => {
            let (a, b): (f32, f32) = (a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?);
            if a < b {
                Ok(RangeMode::Fixed(a, b))
            } else {
                Err(format!("range: MIN must be below MAX, got {a} {b}"))
            }
        }
        _ => Err(bad()),
    }
}

/// Index along a dimension of `len` steps for `sel`, given its coordinate values.
pub fn time_index(sel: TimeSel, len: usize, coord: Option<&[f32]>) -> Result<usize, String> {
    if len == 0 {
        return Err("no steps".into());
    }
    let from_index = |i: isize| {
        let k = if i > 0 { i - 1 } else { len as isize + i };
        if (0..len as isize).contains(&k) {
            Ok(k as usize)
        } else {
            Err(format!("index {i} outside 1..{len}"))
        }
    };
    match (sel, coord) {
        (TimeSel::Index(i), _) => from_index(i),
        (TimeSel::Value(v), Some(c)) => c
            .iter()
            .enumerate()
            .filter(|(_, x)| x.is_finite())
            .min_by(|a, b| (*a.1 as f64 - v).abs().total_cmp(&(*b.1 as f64 - v).abs()))
            .map(|(i, _)| i)
            .ok_or_else(|| "no coordinate values".into()),
        (TimeSel::Value(v), None) if v.fract() == 0.0 => from_index(v as isize),
        (TimeSel::Value(v), None) => Err(format!("no coordinate variable; use @index, not {v}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse("v H_ice"), Ok(Command::Var("H_ice".into())));
        assert_eq!(parse(" var  ice:vol "), Ok(Command::Var("ice:vol".into())));
        assert_eq!(
            parse("t -21000"),
            Ok(Command::Time(TimeSel::Value(-21000.0)))
        );
        assert_eq!(parse("t @-1"), Ok(Command::Time(TimeSel::Index(-1))));
        assert_eq!(
            parse("range -2 30"),
            Ok(Command::Range(RangeMode::Fixed(-2.0, 30.0)))
        );
        assert_eq!(
            parse("range -2,30"),
            Ok(Command::Range(RangeMode::Fixed(-2.0, 30.0)))
        );
        assert_eq!(parse("range sym"), Ok(Command::Range(RangeMode::Symmetric)));
        assert_eq!(parse("q"), Ok(Command::Quit));
        assert!(parse("v").is_err());
        assert!(parse("t @0").is_err());
        assert!(parse("range 3 1").is_err());
        assert!(parse("frobnicate").is_err());
    }

    #[test]
    fn time_indices() {
        let c = [-20.0, -10.0, 0.0, 10.0];
        assert_eq!(time_index(TimeSel::Value(-12.0), 4, Some(&c)), Ok(1));
        assert_eq!(time_index(TimeSel::Value(99.0), 4, Some(&c)), Ok(3));
        assert_eq!(time_index(TimeSel::Index(1), 4, Some(&c)), Ok(0));
        assert_eq!(time_index(TimeSel::Index(-1), 4, None), Ok(3));
        assert_eq!(time_index(TimeSel::Value(2.0), 4, None), Ok(1));
        assert!(time_index(TimeSel::Index(5), 4, None).is_err());
        assert!(time_index(TimeSel::Value(2.5), 4, None).is_err());
    }
}
