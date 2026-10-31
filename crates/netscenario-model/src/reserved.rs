//! The keys and values of later releases. A scenario that uses one is told in which release it arrives, not
//! that it is unknown.

use crate::error::Problem;
use crate::source::{self, Kind, SourceIndex};

/// Keys by the path of the mapping they stand in (indices as `[]`): key and the release.
const RESERVED_KEYS: &[(&str, &str, &str)] = &[
    ("", "timeline", "0.2"),
    ("", "repetitions", "0.2"),
    ("", "probes", "0.2"),
    ("", "allow-limited", "0.3"),
    ("", "expect", "0.3"),
    ("", "params", "0.3"),
    ("", "include", "0.3"),
    ("", "matrix", "0.5"),
    ("", "mobility", "0.6"),
    ("topology.links[]", "model", "0.2 and 0.3"),
    ("topology.links[]", "address", "a later release"),
    ("topology.links[].forward", "queue", "0.3"),
    ("topology.links[].reverse", "queue", "0.3"),
    ("topology.links[].both", "queue", "0.3"),
];

/// Values by the generic path of the key they belong to: the value and the release.
const RESERVED_VALUES: &[(&str, &str, &str)] = &[
    ("workloads[].kind", "command", "0.2"),
    ("workloads[].kind", "udp-poisson", "0.3"),
    ("workloads[].kind", "udp-onoff", "0.3"),
    ("workloads[].kind", "trace-replay", "0.3"),
    ("workloads[].kind", "tcp-bulk", "0.3"),
    ("workloads[].kind", "request-response", "0.3"),
    ("workloads[].kind", "container", "0.5"),
    ("topology.links[].forward.loss.model", "trace", "0.3"),
    ("topology.links[].reverse.loss.model", "trace", "0.3"),
    ("topology.links[].both.loss.model", "trace", "0.3"),
    (
        "topology.links[].forward.jitter.distribution",
        "pareto",
        "0.3",
    ),
    (
        "topology.links[].reverse.jitter.distribution",
        "pareto",
        "0.3",
    ),
    ("topology.links[].both.jitter.distribution", "pareto", "0.3"),
];

fn planned(what: &str, release: &str) -> String {
    format!("{what} is planned for release {release} (see FEATURES.md)")
}

/// Finds everything in the file that belongs to a later release.
pub(crate) fn check(index: &SourceIndex) -> Vec<Problem> {
    let mut problems = Vec::new();
    for (path, entry) in index.iter() {
        let generic = source::generic(path);
        // A key of a later release
        if let Some((key_at, key_len)) = entry.key {
            let parent = source::generic(source::parent(path));
            let key = path.rsplit(['.', ']']).next().unwrap_or("");
            if let Some((_, _, release)) = RESERVED_KEYS
                .iter()
                .find(|(p, k, _)| *p == parent && *k == key)
            {
                problems.push(
                    Problem::new(planned(&format!("`{key}`"), release))
                        .at_path(path.clone())
                        .at_location(key_at, key_len),
                );
            }
        }
        // A value of a later release
        if let (Kind::Scalar, Some(text)) = (entry.kind, entry.text.as_deref())
            && let Some((_, _, release)) = RESERVED_VALUES
                .iter()
                .find(|(p, v, _)| *p == generic && *v == text)
        {
            let key = generic.rsplit('.').next().unwrap_or("");
            problems.push(
                Problem::new(planned(&format!("`{key}: {text}`"), release))
                    .at_path(path.clone())
                    .at_location(entry.value.0, entry.value.1),
            );
        }
        // A node written as a mapping (the longer form with a role)
        if generic == "topology.nodes[]" && entry.kind == Kind::Mapping {
            problems.push(
                Problem::new(planned("a node written as a mapping, with a role,", "0.3"))
                    .at_path(path.clone())
                    .at_location(entry.value.0, entry.value.1)
                    .with_help("write the node as its name"),
            );
        }
    }
    problems
}
