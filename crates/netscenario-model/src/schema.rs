//! The JSON Schema of the scenario format.

use schemars::schema_for;

use crate::scenario::Scenario;

/// The JSON Schema of a scenario file, generated from the types, as pretty-printed JSON with a final newline.
///
/// The committed copy is `docs/schema/scenario-v0.json`; a test fails when it differs from this.
#[must_use]
pub fn json_schema() -> String {
    // `{:#}` is the pretty-printed form of a JSON value
    format!("{:#}\n", schema_for!(Scenario).as_value())
}
