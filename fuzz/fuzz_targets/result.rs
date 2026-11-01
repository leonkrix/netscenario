//! A result file, as `report` and `replay` will read it (D-043) and as the Python layer reads it today: read or
//! refused, never a panic, and a result that was read can be written and read again. (The values are not
//! compared: a floating point number may differ in its last digit after the trip through text.)
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_core::RunResult;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(result) = RunResult::from_json(text) {
        let written = result.to_json();
        RunResult::from_json(&written)
            .unwrap_or_else(|error| panic!("written out and refused: {error}\n{written}"));
    }
});
