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
const PLANNED: [&str; 5] = ["run", "validate", "doctor", "clean", "schema"];

const USAGE_TEXT: &str = "\
Usage: netscenario <command> [options]

Commands planned for release 0.1 (none is implemented yet):
  run        run a scenario
  validate   check a scenario file
  doctor     report what this machine can do
  clean      remove what a crashed run left behind
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

    // Canary: the checks above must be able to fail. A wrong expectation has to be caught.
    #[test]
    #[should_panic(expected = "assertion")]
    fn canary_a_wrong_exit_code_is_detected() {
        let (code, _, _) = call(&["netscenario", "frobnicate"]);
        assert_eq!(code, exit::OK, "assertion: this must panic");
    }
}
