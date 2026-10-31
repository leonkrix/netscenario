//! Keys that a mapping must have. The YAML library reports a missing key at the place where it noticed it,
//! which is the next key or the end of the file; this check reports it at the mapping that lacks the key.

use std::collections::{HashMap, HashSet};

use crate::error::Problem;
use crate::source::{self, Kind, SourceIndex};
use crate::suggest::closest;

/// The generic path of a mapping, the name of its type in the JSON Schema, and the keys that are required.
/// A test compares this table with the required keys of the schema, so it cannot drift from the types.
pub(crate) const REQUIRED: &[(&str, &str, &[&str])] = &[
    (
        "",
        "Scenario",
        &["version", "name", "duration", "topology", "workloads"],
    ),
    ("topology", "Topology", &["nodes", "links"]),
    ("topology.links[]", "Link", &["id", "between"]),
    ("topology.links[].forward.jitter", "Jitter", &["amount"]),
    ("topology.links[].reverse.jitter", "Jitter", &["amount"]),
    ("topology.links[].both.jitter", "Jitter", &["amount"]),
    ("topology.links[].forward.loss", "Loss", &["model", "p"]),
    ("topology.links[].reverse.loss", "Loss", &["model", "p"]),
    ("topology.links[].both.loss", "Loss", &["model", "p"]),
    (
        "workloads[]",
        "Workload",
        &["id", "kind", "from", "to", "rate", "packet-size"],
    ),
];

/// Finds the mappings in the file that lack a required key.
pub(crate) fn check(index: &SourceIndex) -> Vec<Problem> {
    // The keys that each mapping has
    let mut keys: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (path, entry) in index.iter() {
        if entry.key.is_some() {
            let name = path.rsplit(['.', ']']).next().unwrap_or("");
            keys.entry(source::parent(path)).or_default().insert(name);
        }
    }
    let mut problems = Vec::new();
    for (path, entry) in index.iter() {
        if entry.kind != Kind::Mapping {
            continue;
        }
        let generic = source::generic(path);
        let Some((_, _, required)) = REQUIRED.iter().find(|(pattern, _, _)| *pattern == generic)
        else {
            continue;
        };
        let present = keys.get(path.as_str());
        for key in *required {
            if !present.is_some_and(|present| present.contains(key)) {
                // A key of the mapping that is not required and is close to the missing one is probably it
                let written = present.and_then(|present| {
                    let mut others: Vec<&str> = present
                        .iter()
                        .copied()
                        .filter(|name| !required.contains(name))
                        .collect();
                    others.sort_unstable();
                    others
                        .into_iter()
                        .find(|name| closest(name, &[key]).is_some())
                });
                let help = match written {
                    Some(written) => format!(
                        "did you mean `{key}` instead of `{written}`? add `{key}:` to this mapping"
                    ),
                    None => format!("add `{key}:` to this mapping"),
                };
                let mut problem = Problem::new(format!("the key `{key}` is missing"))
                    .at_location(entry.value.0, entry.value.1)
                    .with_help(help);
                if !path.is_empty() {
                    problem = problem.at_path(path.clone());
                }
                problems.push(problem);
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::REQUIRED;
    use crate::json_schema;

    /// The keys that the schema requires, by the name of the type, must be the keys of the table.
    #[test]
    fn the_table_matches_the_required_keys_of_the_schema() {
        let schema: serde_json::Value = serde_json::from_str(&json_schema()).unwrap();
        let required_of = |definition: &str| -> Vec<String> {
            let node = if definition == "Scenario" {
                &schema
            } else {
                &schema["$defs"][definition]
            };
            let mut keys: Vec<String> = node["required"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|key| key.as_str().unwrap().to_owned())
                .collect();
            keys.sort();
            keys
        };
        for (_, definition, keys) in REQUIRED {
            let mut table: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
            table.sort();
            assert_eq!(
                table,
                required_of(definition),
                "the required keys of {definition} differ from the schema"
            );
        }
        // Every type of the schema that has required keys is in the table
        let defined = schema["$defs"].as_object().unwrap();
        for (name, definition) in defined {
            if definition["required"]
                .as_array()
                .is_some_and(|keys| !keys.is_empty())
            {
                assert!(
                    REQUIRED.iter().any(|(_, table_name, _)| table_name == name),
                    "{name} has required keys but is not in the table"
                );
            }
        }
    }
}
