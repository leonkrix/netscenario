//! The keys and values of later releases. A scenario that uses one is told in which release it arrives, not
//! that it is unknown.

use crate::error::Problem;
use crate::source::{self, Kind, SourceIndex};

/// Keys by the path of the mapping they stand in (indices as `[]`): key and the release.
const RESERVED_KEYS: &[(&str, &str, &str)] = &[
    ("", "allow-limited", "0.3"),
    ("", "expect", "0.3"),
    ("", "params", "0.3"),
    ("", "include", "0.3"),
    ("", "matrix", "0.5"),
    ("", "mobility", "0.6"),
    ("topology.links[]", "model", "0.3"),
    ("topology.links[]", "address", "a later release"),
];

/// Keys that look like keys of the scenario and are not: they are options of a run, which is what makes a run of the scenario
/// comparable with another one (the hash of the scenario is the same). Parent, key, what is wrong and what to do.
const OPTIONS_OF_A_RUN: &[(&str, &str, &str, &str)] = &[
    (
        "timeline[].set.forward",
        "queue",
        "a step cannot change the `queue` of a direction",
        "write the `queue` in the link: the queue is built with the link, and a rate that changes needs the traces of a later release",
    ),
    (
        "timeline[].set.reverse",
        "queue",
        "a step cannot change the `queue` of a direction",
        "write the `queue` in the link: the queue is built with the link, and a rate that changes needs the traces of a later release",
    ),
    (
        "timeline[].set.both",
        "queue",
        "a step cannot change the `queue` of a direction",
        "write the `queue` in the link: the queue is built with the link, and a rate that changes needs the traces of a later release",
    ),
    (
        "",
        "repetitions",
        "`repetitions` is an option of a run and not a key of the scenario",
        "run the scenario several times with `netscenario run --repetitions n`, or `run(..., repetitions=n)` in Python (docs/STATISTICS.md)",
    ),
];

/// Values by the generic path of the key they belong to: the value and the release.
const RESERVED_VALUES: &[(&str, &str, &str)] = &[
    ("workloads[].kind", "trace-replay", "0.3"),
    ("workloads[].kind", "tcp-bulk", "0.3"),
    ("workloads[].kind", "request-response", "0.3"),
    ("workloads[].kind", "container", "0.5"),
    ("topology.links[].forward.loss.model", "trace", "0.3"),
    ("topology.links[].reverse.loss.model", "trace", "0.3"),
    ("topology.links[].both.loss.model", "trace", "0.3"),
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
        // An option of a run that was written as a key
        if let Some((key_at, key_len)) = entry.key {
            let parent = source::generic(source::parent(path));
            let key = path.rsplit(['.', ']']).next().unwrap_or("");
            if let Some((_, _, message, help)) = OPTIONS_OF_A_RUN
                .iter()
                .find(|(p, k, _, _)| *p == parent && *k == key)
            {
                problems.push(
                    Problem::new((*message).to_owned())
                        .at_path(path.clone())
                        .at_location(key_at, key_len)
                        .with_help((*help).to_owned()),
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
    }
    problems
}
