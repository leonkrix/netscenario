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
];

/// Mappings whose keys depend on the value of one key, the tag: the loss (`model`) and the workload (`kind`).
pub(crate) struct Tagged {
    /// The generic paths of the mappings.
    pub(crate) paths: &'static [&'static str],
    /// The key whose value picks the variant. It is required.
    pub(crate) tag: &'static str,
    /// The name of the type in the JSON Schema, whose variants a test compares with this table.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by the test that compares the table with the schema"
        )
    )]
    pub(crate) schema: &'static str,
    /// What the tag chooses, in the words of a message.
    pub(crate) what: &'static str,
    /// For each value of the tag: the keys that are required besides it, and the keys that may be written.
    pub(crate) variants: &'static [(
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
    )],
}

pub(crate) const TAGGED: &[Tagged] = &[
    Tagged {
        paths: &[
            "topology.links[].forward.loss",
            "topology.links[].reverse.loss",
            "topology.links[].both.loss",
        ],
        tag: "model",
        schema: "Loss",
        what: "loss model",
        variants: &[
            ("random", &["p"], &[]),
            ("gilbert-elliott", &["p", "r", "loss-bad", "loss-good"], &[]),
            ("markov4", &["p13", "p31", "p32", "p23", "p14"], &[]),
        ],
    },
    Tagged {
        paths: &["workloads[]"],
        tag: "kind",
        schema: "Workload",
        what: "workload kind",
        variants: &[
            (
                "udp-cbr",
                &["id", "from", "to", "rate", "packet-size"],
                &["start", "stop", "udp-checksum"],
            ),
            (
                "command",
                &["id", "node", "run"],
                &["env", "ready", "shutdown"],
            ),
        ],
    },
];

impl Tagged {
    /// The keys of a variant (required first), without the tag.
    fn keys_of(&self, value: &str) -> Option<Vec<&'static str>> {
        self.variants
            .iter()
            .find(|(name, _, _)| *name == value)
            .map(|(_, required, optional)| {
                required.iter().chain(optional.iter()).copied().collect()
            })
    }
}

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
        let tagged = TAGGED
            .iter()
            .find(|tagged| tagged.paths.contains(&generic.as_str()));
        let (required, allowed): (Vec<&str>, Vec<&str>) = if let Some((_, _, required)) =
            REQUIRED.iter().find(|(pattern, _, _)| *pattern == generic)
        {
            (required.to_vec(), required.to_vec())
        } else if let Some(tagged) = tagged {
            // The tag first; the rest only if its value is one that is known (an unknown one is reported
            // by the reader, with a hint)
            let value = index
                .get(&format!("{path}.{}", tagged.tag))
                .and_then(|entry| entry.text.as_deref());
            let mut needed = vec![tagged.tag];
            let mut allowed = vec![tagged.tag];
            if let Some((_, required, optional)) = tagged
                .variants
                .iter()
                .find(|(name, _, _)| Some(*name) == value)
            {
                needed.extend(*required);
                allowed.extend(required.iter().chain(optional.iter()));
            }
            (needed, allowed)
        } else {
            continue;
        };
        let present = keys.get(path.as_str());
        if let Some(tagged) = tagged {
            problems.extend(keys_of_another_variant(tagged, path, present, index));
        }
        for key in &required {
            let key = *key;
            if !present.is_some_and(|present| present.contains(key)) {
                // A key of the mapping that is not required and is close to the missing one is probably it
                let written = present.and_then(|present| {
                    let mut others: Vec<&str> = present
                        .iter()
                        .copied()
                        .filter(|name| !allowed.contains(name))
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

/// The keys of a mapping that belong to another variant (`r` in a `random` loss, `rate` in a `command`), each
/// at its own place. A key that no variant has is left to the reader, which names the keys that exist.
fn keys_of_another_variant(
    tagged: &Tagged,
    path: &str,
    present: Option<&HashSet<&str>>,
    index: &SourceIndex,
) -> Vec<Problem> {
    let Some(present) = present else {
        return Vec::new();
    };
    let Some((value, own)) = index
        .get(&format!("{path}.{}", tagged.tag))
        .and_then(|entry| entry.text.as_deref())
        .and_then(|value| tagged.keys_of(value).map(|keys| (value, keys)))
    else {
        return Vec::new();
    };
    let belongs_to_some = |key: &str| {
        tagged
            .variants
            .iter()
            .any(|(_, required, optional)| required.contains(&key) || optional.contains(&key))
    };
    let mut others: Vec<&str> = present
        .iter()
        .copied()
        .filter(|key| *key != tagged.tag && !own.contains(key) && belongs_to_some(key))
        .collect();
    others.sort_unstable();
    others
        .into_iter()
        .filter_map(|key| {
            let entry = index.get(&format!("{path}.{key}"))?;
            let (location, length) = entry.key.unwrap_or(entry.value);
            let owners: Vec<&str> = tagged
                .variants
                .iter()
                .filter(|(_, required, optional)| {
                    required.contains(&key) || optional.contains(&key)
                })
                .map(|(name, _, _)| *name)
                .collect();
            let what = tagged.what;
            Some(
                Problem::new(format!(
                    "the key `{key}` is not used by the {what} `{value}`"
                ))
                .at_location(location, length)
                .at_path(format!("{path}.{key}"))
                .with_help(format!(
                    "the {what} `{value}` has {}; `{key}` belongs to {}",
                    own.iter()
                        .filter(|key| **key != "id")
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
    use std::collections::BTreeSet;

    use super::{REQUIRED, TAGGED};
    use crate::json_schema;

    fn strings(value: &serde_json::Value) -> BTreeSet<&str> {
        value
            .as_array()
            .map(|keys| keys.iter().map(|key| key.as_str().unwrap()).collect())
            .unwrap_or_default()
    }

    /// The keys that the schema requires, by the name of the type, must be the keys of the table.
    #[test]
    fn the_table_matches_the_required_keys_of_the_schema() {
        let schema: serde_json::Value = serde_json::from_str(&json_schema()).unwrap();
        let required_of = |definition: &str| -> BTreeSet<String> {
            let node = if definition == "Scenario" {
                &schema
            } else {
                &schema["$defs"][definition]
            };
            strings(&node["required"])
                .into_iter()
                .map(str::to_owned)
                .collect()
        };
        for (_, definition, keys) in REQUIRED {
            let table: BTreeSet<String> = keys.iter().map(|key| (*key).to_owned()).collect();
            assert_eq!(
                table,
                required_of(definition),
                "the required keys of {definition} differ from the schema"
            );
        }
        // The variants of each tagged type are those of the table, with the same required and allowed keys
        let mut variant_types = BTreeSet::new();
        for tagged in TAGGED {
            let node = &schema["$defs"][tagged.schema];
            let variants = node["oneOf"]
                .as_array()
                .or_else(|| node["anyOf"].as_array())
                .unwrap_or_else(|| panic!("{} has no variants", tagged.schema));
            assert_eq!(variants.len(), tagged.variants.len(), "{}", tagged.schema);
            for variant in variants {
                let variant = match variant["$ref"].as_str() {
                    Some(reference) => {
                        let name = reference.rsplit('/').next().unwrap();
                        variant_types.insert(name.to_owned());
                        &schema["$defs"][name]
                    }
                    None => variant,
                };
                let mut property = &variant["properties"][tagged.tag];
                if let Some(reference) = property["$ref"].as_str() {
                    property = &schema["$defs"][reference.rsplit('/').next().unwrap()];
                }
                let value = property["const"]
                    .as_str()
                    .or_else(|| property["enum"][0].as_str())
                    .unwrap_or_else(|| {
                        panic!("a variant of {} has no {}", tagged.schema, tagged.tag)
                    });
                let (_, required, optional) = tagged
                    .variants
                    .iter()
                    .find(|(name, _, _)| *name == value)
                    .unwrap_or_else(|| panic!("{value} is not in the table"));
                let in_schema_required: BTreeSet<&str> = strings(&variant["required"])
                    .into_iter()
                    .filter(|key| *key != tagged.tag)
                    .collect();
                let table_required: BTreeSet<&str> = required.iter().copied().collect();
                assert_eq!(
                    table_required, in_schema_required,
                    "the required keys of {value}"
                );
                let in_schema_all: BTreeSet<&str> = variant["properties"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .filter(|key| *key != tagged.tag)
                    .collect();
                let table_all: BTreeSet<&str> =
                    required.iter().chain(optional.iter()).copied().collect();
                assert_eq!(table_all, in_schema_all, "the keys of {value}");
            }
        }
        // Every type of the schema that has required keys is in a table
        let defined = schema["$defs"].as_object().unwrap();
        for (name, definition) in defined {
            if definition["required"]
                .as_array()
                .is_some_and(|keys| !keys.is_empty())
            {
                assert!(
                    REQUIRED.iter().any(|(_, table_name, _)| table_name == name)
                        || variant_types.contains(name)
                        || TAGGED.iter().any(|tagged| tagged.schema == name),
                    "{name} has required keys but is not in a table"
                );
            }
        }
    }
}
