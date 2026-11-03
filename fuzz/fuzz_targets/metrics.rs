//! The lines that a program of your own writes to its standard output (PRB-08, D-072): a line is a metric, not one or
//! invalid, and never a panic. A line that is a metric has what the format promises: a name of 1 to 128 characters
//! without control characters, and a finite value.
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_core::metrics::{Line, MAX_NAME, parse_line};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    for line in text.lines() {
        if let Line::Metric(reading) = parse_line(line) {
            let length = reading.metric.chars().count();
            assert!((1..=MAX_NAME).contains(&length), "a name of {length} characters: {line:?}");
            assert!(
                !reading.metric.chars().any(char::is_control),
                "a control character in the name: {line:?}"
            );
            assert!(reading.value.is_finite(), "a value that is not finite: {line:?}");
        }
    }
});
