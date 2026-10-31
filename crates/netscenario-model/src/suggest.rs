//! Hints for misspelled names: the one known name that is close enough to what was written.

/// The number of edits that turn `a` into `b`: inserting, deleting or replacing a character, or swapping two
/// neighbours (`dealy` for `delay` is one edit, because that is the commonest slip of the fingers).
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    // The rows before the previous one, the previous one and the current one
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut row = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let replace = usize::from(a[i - 1] != b[j - 1]);
            row[j] = (previous[j] + 1)
                .min(row[j - 1] + 1)
                .min(previous[j - 1] + replace);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(before[j - 2] + 1);
            }
        }
        before = previous;
        previous = row;
    }
    previous[b.len()]
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
        assert_eq!(distance("sevrer", "server"), 1);
        assert_eq!(distance("sxxver", "server"), 2);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("abc", ""), 3);
        assert_eq!(distance("kitten", "sitting"), 3);
        // two neighbours that were swapped are one edit, two letters far apart are two
        assert_eq!(distance("dealy", "delay"), 1);
        assert_eq!(distance("ab", "ba"), 1);
        assert_eq!(distance("abc", "cba"), 2);
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
