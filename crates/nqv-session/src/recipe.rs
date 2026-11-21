//! Recipes: files of viewer commands (the `:` syntax, one per line, `#` comments) in
//! ~/.nqv/recipes, applied with `-r NAME` or `:recipe NAME`.

use std::path::{Path, PathBuf};

use crate::command::{self, Command};

/// ~/.nqv/recipes
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".nqv/recipes"))
}

/// ~/.nqv/defaults: settings applied whenever files are opened.
pub fn defaults_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".nqv/defaults"))
}

/// The commands of ~/.nqv/defaults (none without the file), and the lines that are
/// not commands (skipped), each with why.
pub fn defaults() -> (Vec<Command>, Vec<String>) {
    let Some(p) = defaults_path().filter(|p| p.is_file()) else {
        return (vec![], vec![]);
    };
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) => return (vec![], vec![format!("{}: {e}", p.display())]),
    };
    let (mut cmds, mut bad) = (vec![], vec![]);
    for (n, line) in text.lines().enumerate() {
        match parse(line) {
            Ok(c) => cmds.extend(c),
            Err((_, e)) => {
                // the reason, without the list of all commands
                let e = e.split("; commands:").next().unwrap_or(&e);
                bad.push(format!("~/.nqv/defaults line {}: {e}", n + 1));
            }
        }
    }
    (cmds, bad)
}

/// The recipe file for `name`: a path to a file, or a name in ~/.nqv/recipes.
pub fn path(name: &str) -> Result<PathBuf, String> {
    let p = Path::new(name);
    if name.contains('/') || p.is_file() {
        return match p.is_file() {
            true => Ok(p.to_path_buf()),
            false => Err(format!("recipe '{name}': no such file")),
        };
    }
    match dir().map(|d| d.join(name)) {
        Some(p) if p.is_file() => Ok(p),
        _ => Err(format!(
            "recipe '{name}' not found in ~/.nqv/recipes (nqv --recipes lists them)"
        )),
    }
}

/// The commands of a recipe; errors name the line.
pub fn load(name: &str) -> Result<Vec<Command>, String> {
    let p = path(name)?;
    let text = std::fs::read_to_string(&p).map_err(|e| format!("recipe {}: {e}", p.display()))?;
    parse(&text).map_err(|(line, e)| format!("recipe {} line {line}: {e}", p.display()))
}

/// The text of recipe `name` from this machine (a path, or a name in ~/.nqv/recipes),
/// or None if there is none.
pub fn text(name: &str) -> Result<Option<String>, String> {
    let Ok(p) = path(name) else { return Ok(None) };
    std::fs::read_to_string(&p)
        .map(Some)
        .map_err(|e| format!("recipe {}: {e}", p.display()))
}

/// Commands of recipe text found at `place` (for errors).
pub fn parse_at(text: &str, place: &str) -> Result<Vec<Command>, String> {
    parse(text).map_err(|(line, e)| format!("recipe {place} line {line}: {e}"))
}

/// Commands from recipe text, or the (1-based) line that failed and why.
pub fn parse(text: &str) -> Result<Vec<Command>, (usize, String)> {
    let mut out = vec![];
    for (n, line) in text.lines().enumerate() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        let cmd = command::parse(line).map_err(|e| (n + 1, e))?;
        if matches!(cmd, Command::Recipe(_) | Command::Save(_) | Command::Quit) {
            return Err((n + 1, format!("'{line}' is not allowed in a recipe")));
        }
        out.push(cmd);
    }
    Ok(out)
}

/// The line without a `#` comment (at the start, or after a space).
fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    match line.find(" #") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// First comment line of a recipe, as its description.
pub fn describe(p: &Path) -> String {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.trim().strip_prefix('#').map(|d| d.trim().to_string()))
        })
        .unwrap_or_default()
}

/// Recipes in ~/.nqv/recipes, sorted by name, with their descriptions.
pub fn list() -> Vec<(String, String)> {
    let Some(d) = dir() else { return vec![] };
    let Ok(entries) = std::fs::read_dir(&d) else {
        return vec![];
    };
    let mut out: Vec<(String, String)> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                describe(&e.path()),
            )
        })
        .filter(|(n, _)| !n.starts_with('.'))
        .collect();
    out.sort();
    out
}

/// Write `lines` as recipe `name` in ~/.nqv/recipes (or to a path; `defaults`: as
/// ~/.nqv/defaults); returns the file.
pub fn save(name: &str, lines: &[String]) -> Result<PathBuf, String> {
    let p = match name {
        "defaults" => defaults_path().ok_or("no home folder")?,
        n if n.contains('/') => PathBuf::from(n),
        n => dir().ok_or("no home folder")?.join(n),
    };
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text = format!("# {name} (saved by nqv)\n{}\n", lines.join("\n"));
    std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recipe_text() {
        let text = "# Ice: velocity on the ice\nv ice:uxy_s\n\nmask f_ice < 0.5   # ice only\ncontour z_srf step=500\n";
        let cmds = parse(text).unwrap();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], Command::Var("ice:uxy_s".into()));
        assert!(matches!(cmds[1], Command::Mask(Some(_))));
        assert_eq!(parse("v a\nbogus x").unwrap_err().0, 2);
        assert!(parse("recipe other").is_err());
    }

    #[test]
    fn saves_and_describes() {
        let d = std::env::temp_dir().join(format!("nqv-recipe-{}", std::process::id()));
        let p = save(d.join("ice").to_str().unwrap(), &["v H_ice".into()]).unwrap();
        assert_eq!(describe(&p), format!("{} (saved by nqv)", p.display()));
        assert_eq!(
            load(p.to_str().unwrap()).unwrap(),
            [Command::Var("H_ice".into())]
        );
        std::fs::remove_dir_all(&d).unwrap();
    }
}
