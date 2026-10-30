//! The command line of netscenario (CLI-01, CLI-06).
//!
//! [`run`] takes the arguments and two writers and returns the exit code, so that the behaviour of the
//! command line can be tested without starting a process. The binary in `main.rs` only connects it to
//! the real arguments and streams.

use std::io::Write;

/// The exit codes of the command line. They are part of the interface and documented.
pub mod exit {
    /// The command did what was asked.
    pub const OK: u8 = 0;
    /// A run finished, and an assertion about its result failed.
    pub const ASSERTION_FAILED: u8 = 1;
    /// The command line or the scenario is wrong.
    pub const USAGE: u8 = 2;
    /// The environment does not allow the run: a missing tool or permission.
    pub const ENVIRONMENT: u8 = 3;
    /// A run could not be completed.
    pub const RUN_FAILED: u8 = 4;
}

/// The commands that are planned for release 0.1 and do not exist yet.
const PLANNED: [&str; 3] = ["run", "validate", "schema"];

const USAGE_TEXT: &str = "\
Usage: netscenario <command> [options]

Commands:
  doctor     report what this machine can do (--json for machines)
  clean      end what a run without a supervisor left behind (--dry-run, --json)

Commands planned for release 0.1 (not implemented yet):
  run        run a scenario
  validate   check a scenario file
  schema     print the JSON Schema of the scenario format

Options:
  -h, --help     print this help
  -V, --version  print the version

Exit codes: 0 ok, 1 assertion failed, 2 usage or scenario error,
3 environment or permission error, 4 run failed.
";

/// Runs the command line with `args` (the first one is the program name), writes to `out` and `err`, and
/// returns the exit code (see [`exit`]).
pub fn run<I, S>(args: I, out: &mut impl Write, err: &mut impl Write) -> u8
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let _program = args.next();
    let Some(first) = args.next() else {
        let _ = write!(err, "netscenario: no command given\n\n{USAGE_TEXT}");
        return exit::USAGE;
    };
    match first.as_ref() {
        "-V" | "--version" => {
            let _ = writeln!(out, "netscenario {}", env!("CARGO_PKG_VERSION"));
            exit::OK
        }
        "-h" | "--help" => {
            let _ = write!(out, "{USAGE_TEXT}");
            exit::OK
        }
        "doctor" => doctor(args, out, err),
        "clean" => clean(args, out, err),
        command if PLANNED.contains(&command) => {
            let _ = writeln!(
                err,
                "netscenario: the command '{command}' is not implemented yet\n  it is planned for release 0.1, see FEATURES.md"
            );
            exit::USAGE
        }
        other => {
            let _ = writeln!(
                err,
                "netscenario: unknown command '{other}'\n  try: netscenario --help"
            );
            exit::USAGE
        }
    }
}

/// The options of a command that takes only flags: which of `allowed` were given, or the usage error.
fn flags<I, S>(
    command: &str,
    args: I,
    allowed: &[&str],
    err: &mut impl Write,
) -> Result<Vec<String>, u8>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let mut given = Vec::new();
    for arg in args {
        let arg = arg.as_ref();
        if allowed.contains(&arg) {
            given.push(arg.to_owned());
        } else {
            let _ = writeln!(
                err,
                "netscenario {command}: unknown option '{arg}'\n  the options are: {}",
                allowed.join(", ")
            );
            return Err(exit::USAGE);
        }
    }
    Ok(given)
}

/// `doctor`: what this machine can do for the Kernel backend (ENV-04). Exit code 3 if scenarios cannot run.
fn doctor<I, S>(args: I, out: &mut impl Write, err: &mut impl Write) -> u8
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let given = match flags("doctor", args, &["--json"], err) {
        Ok(given) => given,
        Err(code) => return code,
    };
    let report = netscenario_backend_kernel::doctor::diagnose();
    let _ = if given.is_empty() {
        write!(out, "{}", report.render())
    } else {
        writeln!(out, "{}", report.to_json())
    };
    if report.ok {
        exit::OK
    } else {
        exit::ENVIRONMENT
    }
}

/// `clean`: ends the processes of runs that have no supervisor. Exit code 3 if one could not be ended.
fn clean<I, S>(args: I, out: &mut impl Write, err: &mut impl Write) -> u8
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let given = match flags("clean", args, &["--dry-run", "--json"], err) {
        Ok(given) => given,
        Err(code) => return code,
    };
    let has = |flag: &str| given.iter().any(|g| g == flag);
    let report = netscenario_backend_kernel::clean::clean(has("--dry-run"));
    let _ = if has("--json") {
        writeln!(out, "{}", report.to_json())
    } else {
        write!(out, "{}", report.render())
    };
    if report.succeeded() {
        exit::OK
    } else {
        exit::ENVIRONMENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(args: &[&str]) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(args.iter().copied(), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn version_is_printed_with_the_package_version() {
        let (code, out, err) = call(&["netscenario", "--version"]);
        assert_eq!(code, exit::OK);
        assert_eq!(out, format!("netscenario {}\n", env!("CARGO_PKG_VERSION")));
        assert_eq!(err, "");
    }

    #[test]
    fn help_lists_the_commands_and_the_exit_codes() {
        let (code, out, _) = call(&["netscenario", "-h"]);
        assert_eq!(code, exit::OK);
        for word in ["run", "validate", "doctor", "clean", "schema", "Exit codes"] {
            assert!(out.contains(word), "help does not mention {word}");
        }
    }

    #[test]
    fn no_command_is_a_usage_error_with_the_help_on_stderr() {
        let (code, out, err) = call(&["netscenario"]);
        assert_eq!(code, exit::USAGE);
        assert_eq!(out, "");
        assert!(err.contains("no command given") && err.contains("Usage:"));
    }

    #[test]
    fn a_planned_command_says_that_it_does_not_exist_yet() {
        for command in PLANNED {
            let (code, out, err) = call(&["netscenario", command]);
            assert_eq!(code, exit::USAGE, "{command}");
            assert_eq!(out, "");
            assert!(err.contains("not implemented yet") && err.contains(command));
        }
    }

    #[test]
    fn an_unknown_command_names_itself_and_points_to_the_help() {
        let (code, _, err) = call(&["netscenario", "frobnicate"]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("unknown command 'frobnicate'") && err.contains("--help"));
    }

    #[test]
    fn the_doctor_prints_its_sections_and_its_exit_code_follows_the_result() {
        let (code, out, err) = call(&["netscenario", "doctor"]);
        assert_eq!(err, "");
        for heading in ["Tools", "Kernel and tc", "Result:"] {
            assert!(out.contains(heading), "{heading}\n{out}");
        }
        assert_eq!(
            code == exit::OK,
            out.contains("scenarios can run here"),
            "{out}"
        );
    }

    #[test]
    fn the_doctor_prints_json_with_the_same_verdict() {
        let (code, out, _) = call(&["netscenario", "doctor", "--json"]);
        let json: serde_json::Value = serde_json::from_str(&out).expect("json on stdout");
        assert_eq!(json["version"], 1);
        assert_eq!(json["ok"] == true, code == exit::OK, "{out}");
        assert!(json["checks"].as_array().is_some_and(|c| !c.is_empty()));
    }

    #[test]
    fn a_dry_run_of_clean_changes_nothing_and_succeeds() {
        let (code, out, err) = call(&["netscenario", "clean", "--dry-run"]);
        assert_eq!((code, err.as_str()), (exit::OK, ""));
        assert!(out.starts_with("netscenario clean\n"), "{out}");
        let (_, json, _) = call(&["netscenario", "clean", "--dry-run", "--json"]);
        let json: serde_json::Value = serde_json::from_str(&json).expect("json on stdout");
        assert_eq!(json["dry_run"], true);
        assert!(json["ended"].as_array().is_some_and(Vec::is_empty));
    }

    #[test]
    fn an_unknown_option_is_a_usage_error_that_lists_the_options() {
        for (command, option) in [("doctor", "--verbose"), ("clean", "--force")] {
            let (code, out, err) = call(&["netscenario", command, option]);
            assert_eq!(code, exit::USAGE, "{command}");
            assert_eq!(out, "");
            assert!(
                err.contains(&format!("unknown option '{option}'"))
                    && err.contains("the options are"),
                "{err}"
            );
        }
    }

    // Canary: the checks above must be able to fail. A wrong expectation has to be caught.
    #[test]
    #[should_panic(expected = "assertion")]
    fn canary_a_wrong_exit_code_is_detected() {
        let (code, _, _) = call(&["netscenario", "frobnicate"]);
        assert_eq!(code, exit::OK, "assertion: this must panic");
    }
}
