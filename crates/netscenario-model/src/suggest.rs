//! Hints for misspelled names: the one known name that is close enough to what was written.

/// The number of single-character edits (insert, delete, replace) that turn `a` into `b`.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == *cb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The name in `known` that `written` was most likely meant to be, if there is exactly one.
///
/// A name is close when it differs by at most one edit for short names and by a third of the length for
/// longer ones. If two names are equally close, nothing is suggested: a wrong hint is worse than none.
pub(crate) fn closest<'a>(written: &str, known: &[&'a str]) -> Option<&'a str> {
    let allowed = (written.chars().count() / 3).max(1);
    let mut best: Option<(usize, &'a str)> = None;
    let mut tie = false;
    for &name in known {
        let d = distance(written, name);
        if d == 0 || d > allowed {
            continue;
        }
        match best {
            Some((best_d, _)) if d > best_d => {}
            Some((best_d, _)) if d == best_d => tie = true,
            _ => {
                best = Some((d, name));
                tie = false;
            }
        }
    }
    if tie {
        None
    } else {
        best.map(|(_, name)| name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_edits() {
        assert_eq!(distance("server", "server"), 0);
        assert_eq!(distance("sever", "server"), 1);
        assert_eq!(distance("sevrer", "server"), 2);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("abc", ""), 3);
        assert_eq!(distance("kitten", "sitting"), 3);
    }

    #[test]
    fn a_close_name_is_suggested() {
        assert_eq!(closest("sever", &["client", "server"]), Some("server"));
        assert_eq!(closest("cliant", &["client", "server"]), Some("client"));
    }

    #[test]
    fn a_far_name_is_not_suggested() {
        assert_eq!(closest("router", &["client", "server"]), None);
        assert_eq!(closest("a", &["client", "server"]), None);
    }

    #[test]
    fn two_equally_close_names_give_no_hint() {
        assert_eq!(closest("node-c", &["node-a", "node-b"]), None);
    }

    #[test]
    fn the_exact_name_is_not_a_hint() {
        assert_eq!(closest("server", &["server"]), None);
    }

    /// Canary: the checks above can fail, because a wrong suggestion would be returned without the limit.
    #[test]
    fn the_limit_is_what_keeps_wrong_hints_away() {
        assert!(distance("router", "server") > 1);
        assert!(distance("a", "client") > 1);
    }
}
