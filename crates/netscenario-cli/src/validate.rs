//! `netscenario validate` and `netscenario schema`: look at a scenario file without running anything.

use std::io::Write;
use std::path::Path;

use netscenario_core::requirements;
use netscenario_model::{Error as ModelError, Scenario};
use serde_json::{Value, json};

use crate::args::{PARAM, Parsed, Spec};
use crate::exit;

pub(crate) const VALIDATE_HELP: &str = "\
Usage: netscenario validate <file> [--param <name>=<value>]... [--json] [--allow-outside-files]

Checks a scenario file: the syntax, the keys, every value with its unit, and the rules between values (a
node that exists, a window inside the duration). The files that the scenario refers to are read and checked
too (a file of delay samples for a jitter with a table). Nothing is run and nothing is created.

  --param <name>=<value>  give a value to a parameter of the scenario (`params:`), as often as there are parameters
  --json    print the answer as JSON on standard output
  --allow-outside-files  let the scenario read files from outside the directory of the scenario file

Exit codes: 0 the scenario is valid, 2 it is not (the problems are listed with their lines) or the file
cannot be read.
";

pub(crate) const SCHEMA_HELP: &str = "\
Usage: netscenario schema

Prints the JSON Schema of the scenario format on standard output, for editors and other tools.
";

pub(crate) const VALIDATE_SPEC: Spec = Spec {
    values: &[PARAM],
    flags: &["json", ALLOW_OUTSIDE_FILES],
};

/// The flag that lets a scenario read files from outside its own directory (D-083).
pub(crate) const ALLOW_OUTSIDE_FILES: &str = "allow-outside-files";

/// A scenario file that has been read: the text and the parsed scenario, or the problems.
pub(crate) struct Loaded {
    /// The name shown in messages: the path as the person typed it.
    pub(crate) name: String,
    /// The text of the file, kept for the lines in messages.
    pub(crate) text: String,
    /// The scenario, or why there is none.
    pub(crate) scenario: Result<Scenario, ModelError>,
}

/// Reads and checks a scenario file, with the values of its parameters (`--param name=value`, D-093).
///
/// # Errors
///
/// A sentence if the file cannot be read or a `--param` is not `name=value`.
pub(crate) fn load(
    path: &str,
    allow_outside_files: bool,
    params: &[String],
) -> Result<Loaded, String> {
    let bytes =
        std::fs::read(Path::new(path)).map_err(|error| format!("cannot read '{path}': {error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("'{path}' is not text in UTF-8, which a scenario must be"))?;
    let mut given = Vec::new();
    for param in params {
        let (name, value) = param.split_once('=').ok_or_else(|| {
            format!("--param '{param}' is not name=value, for example --param rtt=40ms")
        })?;
        given.push((name.to_owned(), value.to_owned()));
    }
    // The files are next to the scenario file (D-083)
    let base = Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let scenario = netscenario_core::load_scenario(&text, path, base, allow_outside_files, &given);
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
                "file": problem.file(),
                "chain": problem.chain(),
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
    let loaded = match load(path, parsed.flag(ALLOW_OUTSIDE_FILES), parsed.all(PARAM)) {
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
                    "ok: {} ({})\n  {} nodes, {} {}, {} {}, duration {duration}\n  needs: {}",
                    scenario.name,
                    loaded.name,
                    scenario.topology.nodes.len(),
                    scenario.topology.links.len(),
                    if scenario.topology.links.len() == 1 {
                        "link"
                    } else {
                        "links"
                    },
                    scenario.workloads.len(),
                    if scenario.workloads.len() == 1 {
                        "workload"
                    } else {
                        "workloads"
                    },
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
