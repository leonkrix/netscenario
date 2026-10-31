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
    ("timeline[]", "Event", &["at", "link"]),
    ("topology.links[]", "Link", &["id", "between"]),
    ("topology.links[].forward.jitter", "Jitter", &["amount"]),
    ("topology.links[].reverse.jitter", "Jitter", &["amount"]),
    ("topology.links[].both.jitter", "Jitter", &["amount"]),
    ("topology.links[].forward.reorder", "Reorder", &["share"]),
    ("topology.links[].reverse.reorder", "Reorder", &["share"]),
    ("topology.links[].both.reorder", "Reorder", &["share"]),
    (
        "workloads[]",
        "Workload",
        &["id", "kind", "from", "to", "rate", "packet-size"],
    ),
];

/// The mappings of a loss; the keys they need depend on the value of `model`, so they are not in the table above.
const LOSS_PATHS: &[&str] = &[
    "topology.links[].forward.loss",
    "topology.links[].reverse.loss",
    "topology.links[].both.loss",
];

/// The keys that each model needs besides `model` itself, in the order of the format. A test compares this
/// table with the variants of `Loss` in the JSON Schema.
pub(crate) const LOSS_MODELS: &[(&str, &[&str])] = &[
    ("random", &["p"]),
    ("gilbert-elliott", &["p", "r", "loss-bad", "loss-good"]),
    ("markov4", &["p13", "p31", "p32", "p23", "p14"]),
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
        let required: Vec<&str> = if let Some((_, _, required)) =
            REQUIRED.iter().find(|(pattern, _, _)| *pattern == generic)
        {
            required.to_vec()
        } else if LOSS_PATHS.contains(&generic.as_str()) {
            // `model` first; the rest only if the model is one that is known (an unknown one is reported
            // by the reader, with a hint)
            let model = index
                .get(&format!("{path}.model"))
                .and_then(|entry| entry.text.as_deref());
            let mut needed = vec!["model"];
            if let Some((_, keys)) = LOSS_MODELS.iter().find(|(name, _)| Some(*name) == model) {
                needed.extend(*keys);
            }
            needed
        } else {
            continue;
        };
        let present = keys.get(path.as_str());
        if LOSS_PATHS.contains(&generic.as_str()) {
            problems.extend(keys_of_another_model(path, &required, present, index));
        }
        for key in &required {
            let key = *key;
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

/// The keys of a loss that belong to another model (`r` in a `random` loss), each at its own place. A key that
/// no model has is left to the reader, which names the keys that exist.
fn keys_of_another_model(
    path: &str,
    required: &[&str],
    present: Option<&HashSet<&str>>,
    index: &SourceIndex,
) -> Vec<Problem> {
    let Some(present) = present else {
        return Vec::new();
    };
    let Some(model) = index
        .get(&format!("{path}.model"))
        .and_then(|entry| entry.text.as_deref())
        .filter(|model| LOSS_MODELS.iter().any(|(name, _)| name == model))
    else {
        return Vec::new();
    };
    let mut others: Vec<&str> = present
        .iter()
        .copied()
        .filter(|key| *key != "model" && !required.contains(key))
        .filter(|key| LOSS_MODELS.iter().any(|(_, keys)| keys.contains(key)))
        .collect();
    others.sort_unstable();
    others
        .into_iter()
        .filter_map(|key| {
            let (location, length) = index.locate(&format!("{path}.{key}"))?;
            let entry = index.get(&format!("{path}.{key}"))?;
            let (location, length) = entry.key.unwrap_or((location, length));
            let owners: Vec<&str> = LOSS_MODELS
                .iter()
                .filter(|(_, keys)| keys.contains(&key))
                .map(|(name, _)| *name)
                .collect();
            Some(
                Problem::new(format!(
                    "the key `{key}` is not used by the loss model `{model}`"
                ))
                .at_location(location, length)
                .at_path(format!("{path}.{key}"))
                .with_help(format!(
                    "the model `{model}` has {}; `{key}` belongs to {}",
                    required[1..]
                        .iter()
                        .map(|key| format!("`{key}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    owners.join(" and ")
                )),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{LOSS_MODELS, REQUIRED};
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
        // The keys of each loss model are those of the variants of `Loss`, and every variant is in the table
        let variants = schema["$defs"]["Loss"]["oneOf"].as_array().unwrap();
        assert_eq!(variants.len(), LOSS_MODELS.len());
        for variant in variants {
            let model = variant["properties"]["model"]["const"].as_str().unwrap();
            let mut in_schema: Vec<&str> = variant["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|key| key.as_str().unwrap())
                .filter(|key| *key != "model")
                .collect();
            in_schema.sort_unstable();
            let (_, keys) = LOSS_MODELS
                .iter()
                .find(|(name, _)| *name == model)
                .unwrap_or_else(|| panic!("the model {model} is not in the table"));
            let mut table: Vec<&str> = keys.to_vec();
            table.sort_unstable();
            assert_eq!(table, in_schema, "the required keys of {model} differ");
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
