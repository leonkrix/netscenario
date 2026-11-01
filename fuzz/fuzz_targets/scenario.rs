//! A scenario file of any bytes: the reader must answer, never panic, and what it accepts must be valid and
//! must survive being written out and read again. A refusal must be printable (the renderer cuts the lines of
//! the file at the marked place, which is where a character boundary can go wrong).
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_model::Scenario;

fuzz_target!(|data: &[u8]| {
    #[cfg(feature = "canary")]
    assert!(!data.starts_with(b"CRASH"), "the canary input was found");
    // The reader takes text; a file that is not UTF-8 is refused by whoever reads it from disk
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    match Scenario::from_yaml(text, "fuzz.yaml") {
        Ok(scenario) => {
            assert_eq!(scenario.validate(), Vec::new(), "accepted, yet not valid");
            // JSON is a subset of YAML and is accepted as it is (the same oracle as the property test)
            let json = serde_json::to_string(&scenario).expect("a scenario can be written");
            let again = Scenario::from_yaml(&json, "again.json")
                .unwrap_or_else(|error| panic!("written out and refused: {error}\n{json}"));
            assert_eq!(again, scenario, "written out and read again, not the same");
        }
        Err(error) => {
            assert!(!error.render().is_empty());
            assert!(!error.to_string().is_empty());
        }
    }
});
