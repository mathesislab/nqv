//! Fuzzy matching for the variable picker, case-insensitive.

/// How well `query` matches `text`, lower is better: 0 exact, 1 prefix, 2 substring,
/// 3 letters in order (`hice` in `H_ice`). None when it doesn't match.
pub fn tier(query: &str, text: &str) -> Option<u8> {
    let (q, t) = (query.to_lowercase(), text.to_lowercase());
    if t == q {
        Some(0)
    } else if t.starts_with(&q) {
        Some(1)
    } else if t.contains(&q) {
        Some(2)
    } else if in_order(&q, &t) {
        Some(3)
    } else {
        None
    }
}

/// Whether the characters of `q` appear in `t` in the same order.
fn in_order(q: &str, t: &str) -> bool {
    let mut rest = t.chars();
    q.chars().all(|c| rest.any(|x| x == c))
}

/// Match tier of a variable: by name (0-3), else by long name containing the query (4).
pub fn var_tier(query: &str, name: &str, long_name: Option<&str>) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    tier(query, name).or_else(|| {
        long_name
            .filter(|l| l.to_lowercase().contains(&query.to_lowercase()))
            .map(|_| 4)
    })
}

/// Split a picker query into an optional file part and the variable part: `ice:vol`.
pub fn split(query: &str) -> (Option<&str>, &str) {
    match query.split_once(':') {
        Some((f, v)) => (Some(f), v),
        None => (None, query),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers() {
        assert_eq!(tier("h_ice", "H_ice"), Some(0));
        assert_eq!(tier("h_", "H_ice"), Some(1));
        assert_eq!(tier("ice", "H_ice"), Some(2));
        assert_eq!(tier("hice", "H_ice"), Some(3));
        assert_eq!(tier("hci", "H_ice"), None);
    }

    #[test]
    fn long_name_is_last_resort() {
        assert_eq!(var_tier("thick", "H_ice", Some("Ice thickness")), Some(4));
        assert_eq!(var_tier("thick", "H_ice", None), None);
        assert_eq!(var_tier("", "H_ice", None), Some(0));
    }

    #[test]
    fn file_part() {
        assert_eq!(split("ice:vol"), (Some("ice"), "vol"));
        assert_eq!(split("ice:"), (Some("ice"), ""));
        assert_eq!(split("vol"), (None, "vol"));
    }
}
