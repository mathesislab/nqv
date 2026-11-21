//! Commands typed after `:` in the viewer. Each has a command-line flag parsed by the
//! same function (`COMMANDS` lists the pairs), so both share one vocabulary and
//! everything shown in the viewer can be reproduced from the command line.

use nqv_core::{Levels, Op, Show};

/// Each `:` command with its flag and what it does, for help screens.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "v NAME",
        "-v NAME",
        "variable (fuzzy like the picker; file:var picks the file)",
    ),
    (
        "t VALUE | t @INDEX",
        "-t VALUE",
        "step along the first stepped dimension (usually time)",
    ),
    (
        "at DIM=VALUE",
        "--at DIM=VALUE",
        "any dimension; map dimensions pick the clicked cell",
    ),
    (
        "range MIN MAX | auto | sym",
        "--range MIN,MAX",
        "colour range (y range for line plots)",
    ),
    (
        "cmap NAME",
        "--cmap NAME",
        "colour map: viridis, RdBu_r, grays",
    ),
    (
        "mask VAR OP VALUES [and ...] | off",
        "--mask \"...\"",
        "hide where all conditions hold; OP = != < <= > >=",
    ),
    (
        "contour VAR [step=S | n=N | levels=A,B,..] | off",
        "--contour \"...\"",
        "contour lines of VAR over the map (about 10 round levels by default)",
    ),
    (
        "recipe NAME",
        "-r NAME",
        "apply a recipe: commands in ~/.nqv/recipes/NAME",
    ),
    ("save NAME", "", "save this view's settings as recipe NAME"),
    (
        "show diff | a | b",
        "--show diff|a|b",
        "in a comparison: A - B, A or B",
    ),
    ("q", "", "quit"),
];

/// Position along a dimension: VALUE or @INDEX.
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
    /// Mask conditions (all must hold to hide a value); None removes the mask.
    Mask(Option<Vec<MaskCond>>),
    /// Contour lines of another variable; None removes them.
    Contour(Option<ContourCmd>),
    /// Apply the recipe of this name (or path).
    Recipe(String),
    /// Save the current settings as a recipe of this name.
    Save(String),
    /// Position along a named dimension.
    At(String, TimeSel),
    Cmap(String),
    Show(Show),
    Quit,
}

/// Contours as typed: the variable is resolved by the viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct ContourCmd {
    pub var: String,
    pub levels: Levels,
}

impl ContourCmd {
    pub fn text(&self) -> String {
        let nums = |l: &[f64]| {
            l.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        match &self.levels {
            Levels::Auto(10) => self.var.clone(),
            Levels::Auto(n) => format!("{} n={n}", self.var),
            Levels::Step(s) => format!("{} step={s}", self.var),
            Levels::List(l) => format!("{} levels={}", self.var, nums(l)),
        }
    }
}

/// A mask condition as typed: the variable is resolved by the viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskCond {
    pub var: String,
    pub op: Op,
    pub values: Vec<f32>,
}

impl MaskCond {
    pub fn text(&self) -> String {
        let v: Vec<String> = self.values.iter().map(|v| v.to_string()).collect();
        format!("{} {} {}", self.var, self.op.symbol(), v.join(","))
    }
}

pub const HELP: &str = "v NAME | t VALUE|@INDEX | at DIM=VALUE | range MIN MAX|auto|sym | cmap NAME | mask VAR OP VALUES [and ...]|off | contour VAR [step=S|n=N|levels=..]|off | show diff|a|b | recipe NAME | save NAME | q   (? help)";

/// Parse a command line such as `v H_ice`, `t 1990`, `t @-1`, `range -2 30`.
pub fn parse(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let (name, arg) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let arg = arg.trim();
    match name {
        "v" | "var" if !arg.is_empty() => Ok(Command::Var(arg.to_string())),
        "t" | "time" => parse_time(arg).map(Command::Time),
        "range" => parse_range(arg).map(Command::Range),
        "mask" => parse_mask(arg).map(Command::Mask),
        "contour" => parse_contour(arg).map(Command::Contour),
        "recipe" if !arg.is_empty() => Ok(Command::Recipe(arg.to_string())),
        "save" if !arg.is_empty() => Ok(Command::Save(arg.to_string())),
        "at" => parse_at(arg).map(|(d, p)| Command::At(d, p)),
        "cmap" if !arg.is_empty() => Ok(Command::Cmap(arg.to_string())),
        "show" => parse_show(arg).map(Command::Show),
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

/// `DIM=VALUE` or `DIM=@INDEX`.
pub fn parse_at(arg: &str) -> Result<(String, TimeSel), String> {
    let bad = || format!("at: expected DIM=VALUE or DIM=@INDEX, got '{arg}'");
    let (dim, value) = arg.split_once('=').ok_or_else(bad)?;
    let dim = dim.trim();
    if dim.is_empty() || dim.contains(char::is_whitespace) {
        return Err(bad());
    }
    let pos = parse_time(value.trim()).map_err(|_| bad())?;
    Ok((dim.to_string(), pos))
}

/// `diff`, `a` or `b`.
pub fn parse_show(arg: &str) -> Result<Show, String> {
    match arg.to_lowercase().as_str() {
        "diff" | "a-b" => Ok(Show::Diff),
        "a" => Ok(Show::A),
        "b" => Ok(Show::B),
        _ => Err(format!("show: expected diff, a or b, got '{arg}'")),
    }
}

/// Quote `s` for a POSIX shell when it holds anything beyond plain path characters.
pub fn shell_quote(s: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_-./:@=,+%".contains(c);
    if !s.is_empty() && s.chars().all(plain) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `off`, or `VAR [step=S | n=N | levels=A,B,...]`.
pub fn parse_contour(arg: &str) -> Result<Option<ContourCmd>, String> {
    let arg = arg.trim();
    if arg == "off" || arg == "none" {
        return Ok(None);
    }
    let bad =
        |why: &str| format!("contour: {why}; expected VAR [step=S | n=N | levels=A,B,...] or off");
    let (var, rest) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
    if var.is_empty() {
        return Err(bad("no variable"));
    }
    let rest: String = rest.split_whitespace().collect();
    let levels = match rest.split_once('=') {
        None if rest.is_empty() => Levels::Auto(10),
        Some(("step", v)) => match v.parse::<f64>() {
            Ok(s) if s > 0.0 => Levels::Step(s),
            _ => return Err(bad(&format!("step '{v}' is not a positive number"))),
        },
        Some(("n", v)) => match v.parse::<usize>() {
            Ok(n) if n > 0 => Levels::Auto(n),
            _ => return Err(bad(&format!("n '{v}' is not a positive whole number"))),
        },
        Some(("levels", v)) => {
            let l: Result<Vec<f64>, _> = v.split(',').map(|x| x.parse::<f64>()).collect();
            match l {
                Ok(l) if !l.is_empty() => Levels::List(l),
                _ => return Err(bad(&format!("levels '{v}' are not numbers"))),
            }
        }
        _ => return Err(bad(&format!("cannot read '{rest}'"))),
    };
    Ok(Some(ContourCmd {
        var: var.to_string(),
        levels,
    }))
}

/// `off`, or conditions joined by `and`: `mask_ice != 2`, `basins = 1,3`, `H_ice < 10`.
pub fn parse_mask(arg: &str) -> Result<Option<Vec<MaskCond>>, String> {
    match arg.trim() {
        "off" | "none" => return Ok(None),
        "" => return Err("mask: expected VAR OP VALUES, e.g. mask_ice != 2, or off".into()),
        _ => {}
    }
    arg.split(" and ")
        .map(parse_cond)
        .collect::<Result<_, _>>()
        .map(Some)
}

fn parse_cond(text: &str) -> Result<MaskCond, String> {
    // Two-character operators first, so `<=` is not read as `<`.
    const OPS: [(&str, Op); 7] = [
        ("!=", Op::Ne),
        ("<=", Op::Le),
        (">=", Op::Ge),
        ("==", Op::Eq),
        ("=", Op::Eq),
        ("<", Op::Lt),
        (">", Op::Gt),
    ];
    let bad = || {
        format!(
            "mask: cannot read '{}'; expected VAR OP VALUES",
            text.trim()
        )
    };
    let (pos, sym, op) = OPS
        .iter()
        .filter_map(|&(sym, op)| text.find(sym).map(|p| (p, sym, op)))
        .min_by_key(|&(p, sym, _)| (p, std::cmp::Reverse(sym.len())))
        .ok_or_else(bad)?;
    let var = text[..pos].trim();
    let values: Vec<f32> = text[pos + sym.len()..]
        .split(',')
        .map(|v| v.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    if var.is_empty() || var.contains(char::is_whitespace) {
        return Err(bad());
    }
    if values.len() > 1 && !matches!(op, Op::Eq | Op::Ne) {
        return Err(format!("mask: '{}' takes one value", sym));
    }
    Ok(MaskCond {
        var: var.to_string(),
        op,
        values,
    })
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
    fn parses_at_show_cmap() {
        assert_eq!(
            parse("at zeta=@3"),
            Ok(Command::At("zeta".into(), TimeSel::Index(3)))
        );
        assert_eq!(
            parse("at lat = 70.5"),
            Ok(Command::At("lat".into(), TimeSel::Value(70.5)))
        );
        assert!(parse("at zeta").is_err());
        assert!(parse("at =3").is_err());
        assert_eq!(parse("show B"), Ok(Command::Show(Show::B)));
        assert!(parse("show c").is_err());
        assert_eq!(parse("cmap RdBu_r"), Ok(Command::Cmap("RdBu_r".into())));
    }

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("run/ice.nc"), "run/ice.nc");
        assert_eq!(shell_quote("H_ice < 10"), "'H_ice < 10'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn parses_contours() {
        let c = |var: &str, levels| {
            Ok(Some(ContourCmd {
                var: var.into(),
                levels,
            }))
        };
        assert_eq!(parse_contour("z_srf"), c("z_srf", Levels::Auto(10)));
        assert_eq!(
            parse_contour("z_srf step=500"),
            c("z_srf", Levels::Step(500.0))
        );
        assert_eq!(parse_contour("z_srf n=5"), c("z_srf", Levels::Auto(5)));
        assert_eq!(
            parse_contour("z_srf levels=0, 1000,2000"),
            c("z_srf", Levels::List(vec![0.0, 1000.0, 2000.0]))
        );
        assert_eq!(parse_contour("off"), Ok(None));
        assert!(parse_contour("z_srf step=-1").is_err());
        assert!(parse_contour("z_srf every 5").is_err());
        assert_eq!(parse("recipe ice"), Ok(Command::Recipe("ice".into())));
        assert!(parse("save").is_err());
        let t = ContourCmd {
            var: "z".into(),
            levels: Levels::Step(500.0),
        };
        assert_eq!(parse_contour(&t.text()), Ok(Some(t)));
    }

    #[test]
    fn parses_masks() {
        let c = |var: &str, op, values: &[f32]| MaskCond {
            var: var.into(),
            op,
            values: values.to_vec(),
        };
        assert_eq!(
            parse_mask("mask_ice != 2"),
            Ok(Some(vec![c("mask_ice", Op::Ne, &[2.0])]))
        );
        assert_eq!(
            parse_mask("basins=1, 3"),
            Ok(Some(vec![c("basins", Op::Eq, &[1.0, 3.0])]))
        );
        assert_eq!(
            parse_mask("H_ice <= 10 and z_bed>-100"),
            Ok(Some(vec![
                c("H_ice", Op::Le, &[10.0]),
                c("z_bed", Op::Gt, &[-100.0])
            ]))
        );
        assert_eq!(
            parse_mask("ice.nc:H == 0"),
            Ok(Some(vec![c("ice.nc:H", Op::Eq, &[0.0])]))
        );
        assert_eq!(parse_mask("off"), Ok(None));
        assert!(parse_mask("H_ice < 1,2").is_err());
        assert!(parse_mask("H_ice 3").is_err());
        assert!(parse_mask("< 3").is_err());
        assert_eq!(parse("mask off"), Ok(Command::Mask(None)));
    }

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
