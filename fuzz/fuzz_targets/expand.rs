//! Parameters, templates and includes (SCN-05, D-093) can be anything from anybody who writes a scenario: a file that uses them
//! is read or refused with a sentence, never a panic, and one that is read has what the format promises: it is a valid
//! scenario, it is read the same twice, and the text it is written as reads back to the same scenario and the same hash.
//!
//! The input is the scenario file, then up to four files that it can include (`f0.yaml` to `f3.yaml`), each after a line with
//! `====`; a line of the scenario file that begins with `#param ` gives the value of a parameter (`#param rtt=40ms`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_model::{FileSource, LoadOptions, Scenario};

struct Files(Vec<(String, String)>);

impl FileSource for Files {
    fn read(&self, path: &str) -> Result<String, String> {
        self.0
            .iter()
            .find(|(known, _)| known == path)
            .map(|(_, text)| text.clone())
            .ok_or_else(|| format!("cannot read '{path}': no such file"))
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut parts = text.split("\n====\n");
    let root = parts.next().unwrap_or("");
    let files = Files(
        parts
            .take(4)
            .enumerate()
            .map(|(index, text)| (format!("f{index}.yaml"), text.to_owned()))
            .collect(),
    );
    let mut options = LoadOptions::new().files(&files);
    for line in root.lines() {
        if let Some((name, value)) = line.strip_prefix("#param ").and_then(|rest| rest.split_once('=')) {
            options = options.param(name, value);
        }
    }
    let first = Scenario::from_yaml_with(root, "root.yaml", &options);
    let second = Scenario::from_yaml_with(root, "root.yaml", &options);
    match (first, second) {
        (Ok(scenario), Ok(again)) => {
            assert_eq!(scenario, again, "a file is not read the same twice");
            assert_eq!(scenario.validate(), Vec::new());
            // The text it is written as is a scenario without parameters to expand: it reads back to the same one
            let written = scenario.to_yaml().expect("a scenario can be written");
            let back = Scenario::from_yaml(&written, "again.yaml").expect("a written scenario reads back");
            assert_eq!(back, scenario);
            assert_eq!(
                serde_json::to_string(&back).expect("canonical JSON"),
                serde_json::to_string(&scenario).expect("canonical JSON")
            );
        }
        (Err(error), Err(again)) => {
            // Every problem can be shown, and in the same words
            assert_eq!(error.render(), again.render());
            assert!(!error.problems().is_empty());
        }
        _ => panic!("a file is refused once and read once"),
    }
});
