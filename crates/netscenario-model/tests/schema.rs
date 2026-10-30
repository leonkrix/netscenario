//! The JSON Schema is generated from the types. The committed copy must be the generated one, and the
//! example scenarios must satisfy it, so that the schema and the parser cannot drift apart.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;

use netscenario_model::json_schema;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_committed_schema_is_the_generated_one() {
    let path = root().join("docs/schema/scenario-v0.json");
    let generated = json_schema();
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "{} is missing: run the tests with UPDATE_SCHEMA=1 to write it",
            path.display()
        )
    });
    assert_eq!(
        committed, generated,
        "docs/schema/scenario-v0.json is out of date with the types: run the tests with UPDATE_SCHEMA=1 to write it"
    );
}

fn as_json(yaml: &str) -> serde_json::Value {
    serde_saphyr::from_str(yaml).unwrap()
}

fn validator() -> jsonschema::Validator {
    let schema: serde_json::Value = serde_json::from_str(&json_schema()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}

#[test]
fn every_example_scenario_satisfies_the_schema() {
    let validator = validator();
    let mut paths = Vec::new();
    let mut folders = vec![root().join("scenarios")];
    for entry in fs::read_dir(root().join("examples")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            folders.push(path);
        }
    }
    for folder in folders {
        for entry in fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_some_and(|extension| extension == "yaml")
            {
                paths.push(path);
            }
        }
    }
    assert!(paths.len() >= 6, "the examples are missing: {paths:?}");
    for path in paths {
        let instance = as_json(&fs::read_to_string(&path).unwrap());
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|error| error.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "{} does not satisfy the schema: {errors:?}",
            path.display()
        );
    }
}

/// Faulty files whose fault is in the shape, so the schema has to refuse them as well. (Faults of the
/// contents, such as a node that does not exist, are beyond what a schema can say.)
#[test]
fn the_schema_refuses_faults_of_the_shape() {
    let validator = validator();
    for name in [
        "unknown-key",
        "misspelled-key",
        "number-without-unit",
        "unknown-unit",
        "upper-case-unit",
        "probability-without-percent",
        "name-upper-case",
        "wrong-type",
        "wrong-version",
        "missing-required-key",
        "missing-loss-probability",
    ] {
        let text =
            fs::read_to_string(root().join(format!("scenarios/invalid/{name}.yaml"))).unwrap();
        let instance = as_json(&text);
        assert!(
            !validator.is_valid(&instance),
            "the schema accepts {name}.yaml, which is faulty"
        );
    }
}
