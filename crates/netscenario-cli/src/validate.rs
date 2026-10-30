//! `netscenario validate` and `netscenario schema`: look at a scenario file without running anything.

use std::io::Write;
use std::path::Path;

use netscenario_core::requirements;
use netscenario_model::{Error as ModelError, Scenario};
use serde_json::{Value, json};

use crate::args::{Parsed, Spec};
use crate::exit;

pub(crate) const VALIDATE_HELP: &str = "\
Usage: netscenario validate <file> [--json]

Checks a scenario file: the syntax, the keys, every value with its unit, and the rules between values (a
node that exists, a window inside the duration). Nothing is run and nothing is created.

  --json    print the answer as JSON on standard output

Exit codes: 0 the scenario is valid, 2 it is not (the problems are listed with their lines) or the file
cannot be read.
";

pub(crate) const SCHEMA_HELP: &str = "\
Usage: netscenario schema

Prints the JSON Schema of the scenario format on standard output, for editors and other tools.
";

pub(crate) const VALIDATE_SPEC: Spec = Spec {
    values: &[],
    flags: &["json"],
};

/// A scenario file that has been read: the text and the parsed scenario, or the problems.
pub(crate) struct Loaded {
    /// The name shown in messages: the path as the person typed it.
    pub(crate) name: String,
    /// The text of the file, kept for the lines in messages.
    pub(crate) text: String,
    /// The scenario, or why there is none.
    pub(crate) scenario: Result<Scenario, ModelError>,
}

/// Reads and checks a scenario file.
///
/// # Errors
///
/// A sentence if the file cannot be read.
pub(crate) fn load(path: &str) -> Result<Loaded, String> {
    let bytes =
        std::fs::read(Path::new(path)).map_err(|error| format!("cannot read '{path}': {error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("'{path}' is not text in UTF-8, which a scenario must be"))?;
    let scenario = Scenario::from_yaml(&text, path);
    Ok(Loaded {
        name: path.to_owned(),
        text,
        scenario,
    })
}

fn problems_json(error: &ModelError) -> Vec<Value> {
    error
        .problems()
        .iter()
        .map(|problem| {
            json!({
                "message": problem.message,
                "path": problem.path,
                "line": problem.location.map(|l| l.line),
                "column": problem.location.map(|l| l.column),
                "help": problem.help,
            })
        })
        .collect()
}

pub(crate) fn validate<S: AsRef<str>>(
    args: &[S],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let parsed = match Parsed::parse(args, &VALIDATE_SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return crate::usage_error(err, "validate", &message, VALIDATE_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{VALIDATE_HELP}");
        return exit::OK;
    }
    let path = match parsed.single("the scenario file") {
        Ok(path) => path,
        Err(message) => return crate::usage_error(err, "validate", &message, VALIDATE_HELP),
    };
    let loaded = match load(path) {
        Ok(loaded) => loaded,
        Err(message) => {
            let _ = writeln!(err, "netscenario: {message}");
            return exit::USAGE;
        }
    };
    let json = parsed.flag("json");
    match &loaded.scenario {
        Ok(scenario) => {
            let features: Vec<&str> = requirements(scenario)
                .iter()
                .map(|r| r.feature.id())
                .collect();
            let duration = scenario.duration.to_string();
            if json {
                let answer = json!({
                    "valid": true,
                    "file": loaded.name,
                    "name": scenario.name.as_str(),
                    "duration": duration,
                    "nodes": scenario.topology.nodes.len(),
                    "links": scenario.topology.links.len(),
                    "workloads": scenario.workloads.len(),
                    "features": features,
                });
                let _ = writeln!(out, "{answer}");
            } else {
                let _ = writeln!(
                    out,
                    "ok: {} ({})\n  {} nodes, {} link, {} workload, duration {duration}\n  needs: {}",
                    scenario.name,
                    loaded.name,
                    scenario.topology.nodes.len(),
                    scenario.topology.links.len(),
                    scenario.workloads.len(),
                    features.join(", ")
                );
            }
            exit::OK
        }
        Err(error) => {
            if json {
                let answer = json!({
                    "valid": false,
                    "file": loaded.name,
                    "problems": problems_json(error),
                });
                let _ = writeln!(out, "{answer}");
            } else {
                let _ = write!(err, "{}", error.render());
            }
            exit::USAGE
        }
    }
}

pub(crate) fn schema<S: AsRef<str>>(args: &[S], out: &mut impl Write, err: &mut impl Write) -> u8 {
    let parsed = match Parsed::parse(
        args,
        &Spec {
            values: &[],
            flags: &[],
        },
    ) {
        Ok(parsed) => parsed,
        Err(message) => return crate::usage_error(err, "schema", &message, SCHEMA_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{SCHEMA_HELP}");
        return exit::OK;
    }
    if let Some(extra) = parsed.positionals.first() {
        return crate::usage_error(
            err,
            "schema",
            &format!("unexpected extra argument '{extra}'"),
            SCHEMA_HELP,
        );
    }
    let _ = write!(out, "{}", netscenario_model::json_schema());
    exit::OK
}
