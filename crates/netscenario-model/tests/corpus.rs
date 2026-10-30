//! The example scenarios in `scenarios/` must be valid and survive being written out and read again, and
//! every faulty file in `scenarios/invalid/` must be refused with the message in its snapshot.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use netscenario_model::Scenario;

fn scenarios_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")
}

fn yaml_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .collect();
    files.sort();
    files
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn every_example_scenario_is_valid_and_survives_a_round_trip() {
    let files = yaml_files(&scenarios_dir());
    assert!(files.len() >= 4, "the examples are missing: {files:?}");
    for path in files {
        let name = file_name(&path);
        let text = fs::read_to_string(&path).unwrap();
        let scenario = Scenario::from_yaml(&text, &name)
            .unwrap_or_else(|error| panic!("{name} is not valid:\n{error}"));

        let written = serde_saphyr::to_string(&scenario).unwrap();
        let again = Scenario::from_yaml(&written, &name).unwrap_or_else(|error| {
            panic!("{name} does not read back:\n{error}\n--- written:\n{written}")
        });
        assert_eq!(
            scenario, again,
            "{name} changed on the way through the text:\n{written}"
        );
    }
}

#[test]
fn every_faulty_scenario_is_refused_with_the_message_in_its_snapshot() {
    let files = yaml_files(&scenarios_dir().join("invalid"));
    // A corpus that has silently become empty would test nothing
    assert!(
        files.len() >= 40,
        "the faulty files are missing: found {}",
        files.len()
    );
    insta::glob!("../../../scenarios/invalid", "*.yaml", |path| {
        let name = file_name(path);
        let text = fs::read_to_string(path).unwrap();
        let error =
            Scenario::from_yaml(&text, &name).expect_err(&format!("{name} must be refused"));
        insta::assert_snapshot!(error.render());
    });
}

#[test]
fn a_valid_scenario_that_is_made_faulty_is_refused() {
    // Canary: the checks above must be able to fail
    let valid = fs::read_to_string(scenarios_dir().join("lte-like.yaml")).unwrap();
    assert!(Scenario::from_yaml(&valid, "lte-like.yaml").is_ok());
    let broken = valid.replace("delay: 40ms\n        jitter", "delay: 40\n        jitter");
    assert_ne!(valid, broken);
    assert!(Scenario::from_yaml(&broken, "broken.yaml").is_err());
}
