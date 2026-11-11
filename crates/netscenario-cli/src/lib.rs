//! The command line of netscenario (CLI-01, CLI-04, CLI-06).
//!
//! [`run`] takes the arguments and two writers and returns the exit code, so that the behaviour of the
//! command line can be tested without starting a process. The binary in `main.rs` connects it to the real
//! arguments and streams, and also answers the two hidden commands `supervise` and `hold` that a run starts
//! in its namespaces (the program is its own supervisor, D-022).

mod args;
mod interrupt;
mod profiles_cmd;
mod render;
mod replay_cmd;
mod report_cmd;
mod run_cmd;
mod show_cmd;
mod validate;

use std::io::Write;

use args::{Parsed, Spec};

/// The exit codes of the command line. They are part of the interface and documented.
pub mod exit {
    /// The command did what was asked.
    pub const OK: u8 = 0;
    /// A run finished, and an assertion about its result failed. (Assertions come with release 0.1's
    /// `expect` and the Python layer; no command returns this yet.)
    pub const ASSERTION_FAILED: u8 = 1;
    /// A replay lost other packets than the original, although the loss repeats from its seed (`replay`). It is the
    /// same number as [`ASSERTION_FAILED`]: a check that the command was asked to make did not hold.
    pub const NOT_REPRODUCED: u8 = 1;
    /// The command line or the scenario is wrong, or the backend cannot reproduce what the scenario asks for.
    pub const USAGE: u8 = 2;
    /// The environment does not allow the run: a missing tool or permission.
    pub const ENVIRONMENT: u8 = 3;
    /// A run could not be completed.
    pub const RUN_FAILED: u8 = 4;
}

const USAGE_TEXT: &str = "\
Usage: netscenario <command> [options]

Commands:
  run <file>       run a scenario and write its result
  replay <dir>     run a recorded run again and compare the lost packets
  report <dir>     make Markdown, JUnit or HTML from a result directory
  validate <file>  check a scenario file without running it
  expand <file>    print a scenario in its long form (a shortcut becomes nodes and links)
  show <file>      show the network: nodes, links, routes and the path of each workload
  schema           print the JSON Schema of the scenario format
  profiles [<id>]  list the link profiles, or show one with its values and sources
  capabilities     what the Kernel backend reproduces on this machine, feature by feature
  doctor           report what this machine can do (--json for machines)
  clean            end what a run without a supervisor left behind (--dry-run, --json)

Options:
  -h, --help     print this help (also after a command, for its options)
  -V, --version  print the version

Exit codes: 0 ok, 1 assertion failed or a replay not reproduced, 2 usage or scenario error,
3 environment or permission error, 4 run failed.
";

const DOCTOR_HELP: &str = "\
Usage: netscenario doctor [--timers] [--json]

Reports what this machine can do for the Kernel backend: the tools, the rights to create network
namespaces, what the kernel and `tc` accept, and the features that follow. For everything that is missing
it says how to fix it. See docs/DOCTOR.md.

  --timers  also measure the clock, how late sleeps wake, the load, and what a change of a qdisc
            costs (about two seconds); a late timer is a warning, never a failure
  --json    print the checks as JSON

Exit codes: 0 scenarios can run here, 3 something has to be fixed first.
";

const CLEAN_HELP: &str = "\
Usage: netscenario clean [--dry-run] [--json]

Ends the processes of runs that lost their supervisor. A run needs no cleaning, because the kernel ends
everything when its supervisor dies; this is the safety net for what that does not cover. A run with a
living supervisor is never touched. See docs/DOCTOR.md.

  --dry-run    only report what would be ended
  --json       print the report as JSON

Exit codes: 0 done, 3 a process could not be ended.
";

/// Writes the message of a usage error with the help of the command, and returns the exit code.
pub(crate) fn usage_error(err: &mut impl Write, command: &str, message: &str, help: &str) -> u8 {
    let _ = write!(err, "netscenario {command}: {message}\n\n{help}");
    exit::USAGE
}

/// Runs the command line with `args` (the first one is the program name), writes to `out` and `err`, and
/// returns the exit code (see [`exit`]).
pub fn run<I, S>(args: I, out: &mut impl Write, err: &mut impl Write) -> u8
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .skip(1)
        .map(|arg| arg.as_ref().to_owned())
        .collect();
    let Some((first, rest)) = args.split_first() else {
        let _ = write!(err, "netscenario: no command given\n\n{USAGE_TEXT}");
        return exit::USAGE;
    };
    match first.as_str() {
        "-V" | "--version" => {
            let _ = writeln!(out, "netscenario {}", env!("CARGO_PKG_VERSION"));
            exit::OK
        }
        "-h" | "--help" => {
            let _ = write!(out, "{USAGE_TEXT}");
            exit::OK
        }
        "run" => run_cmd::run_command(rest, out, err),
        "replay" => replay_cmd::replay_command(rest, out, err),
        "report" => report_cmd::report_command(rest, out, err),
        "validate" => validate::validate(rest, out, err),
        "expand" => show_cmd::expand(rest, out, err),
        "show" => show_cmd::show(rest, out, err),
        "schema" => validate::schema(rest, out, err),
        "profiles" => profiles_cmd::profiles(rest, out, err),
        "capabilities" => profiles_cmd::capabilities(rest, out, err),
        "doctor" => doctor(rest, out, err),
        "clean" => clean(rest, out, err),
        other => {
            let _ = writeln!(
                err,
                "netscenario: unknown command '{other}'\n  try: netscenario --help"
            );
            exit::USAGE
        }
    }
}

/// `doctor`: what this machine can do for the Kernel backend (ENV-04). Exit code 3 if scenarios cannot run.
fn doctor(args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    const SPEC: Spec = Spec {
        values: &[],
        flags: &["json", "timers"],
    };
    let parsed = match Parsed::parse(args, &SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, "doctor", &message, DOCTOR_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{DOCTOR_HELP}");
        return exit::OK;
    }
    if let Some(extra) = parsed.positionals.first() {
        let message = format!("unexpected extra argument '{extra}'");
        return usage_error(err, "doctor", &message, DOCTOR_HELP);
    }
    let mut report = netscenario_backend_kernel::doctor::diagnose();
    if parsed.flag("timers") {
        report = report.with_timers(netscenario_backend_kernel::timers::measure());
    }
    let _ = if parsed.flag("json") {
        writeln!(out, "{}", report.to_json())
    } else {
        write!(out, "{}", report.render())
    };
    if report.ok {
        exit::OK
    } else {
        exit::ENVIRONMENT
    }
}

/// `clean`: ends the processes of runs that have no supervisor. Exit code 3 if one could not be ended.
fn clean(args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    const SPEC: Spec = Spec {
        values: &[],
        flags: &["dry-run", "json"],
    };
    let parsed = match Parsed::parse(args, &SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, "clean", &message, CLEAN_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{CLEAN_HELP}");
        return exit::OK;
    }
    if let Some(extra) = parsed.positionals.first() {
        let message = format!("unexpected extra argument '{extra}'");
        return usage_error(err, "clean", &message, CLEAN_HELP);
    }
    let report = netscenario_backend_kernel::clean::clean(parsed.flag("dry-run"));
    let _ = if parsed.flag("json") {
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

    fn scenarios() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")
    }

    #[test]
    fn version_is_printed_with_the_package_version() {
        let (code, out, err) = call(&["netscenario", "--version"]);
        assert_eq!(code, exit::OK);
        assert_eq!(out, format!("netscenario {}\n", env!("CARGO_PKG_VERSION")));
        assert_eq!(err, "");
    }

    /// A directory that is not a run is a wrong input, not a wrong command line: exit 2 with the reason and no help.
    #[test]
    fn a_directory_that_cannot_be_replayed_is_exit_2_with_the_reason() {
        let dir =
            std::env::temp_dir().join(format!("netscenario-cli-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (code, _, err) = call(&["netscenario", "replay", dir.to_str().unwrap()]);
        assert_eq!(code, exit::USAGE);
        assert!(
            err.contains("has no result.json: it is not the directory of a run"),
            "{err}"
        );
        assert!(!err.contains("Usage:"), "{err}");
        std::fs::write(dir.join("runset.json"), "{}").unwrap();
        let (code, _, err) = call(&["netscenario", "replay", dir.to_str().unwrap()]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("is the directory of a set of runs"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn help_lists_the_commands_and_the_exit_codes() {
        let (code, out, _) = call(&["netscenario", "-h"]);
        assert_eq!(code, exit::OK);
        for word in [
            "run",
            "replay",
            "report",
            "validate",
            "doctor",
            "clean",
            "schema",
            "profiles",
            "capabilities",
            "Exit codes",
        ] {
            assert!(out.contains(word), "help does not mention {word}");
        }
        assert!(
            !out.contains("supervise"),
            "the hidden commands stay hidden"
        );
        assert!(!out.contains("not implemented"));
    }

    #[test]
    fn every_command_has_a_help_of_its_own() {
        for (command, word) in [
            ("run", "--packets"),
            ("validate", "--json"),
            ("schema", "JSON Schema"),
            ("doctor", "--json"),
            ("clean", "--dry-run"),
            ("profiles", "docs/PROFILES.md"),
            ("capabilities", "--json"),
        ] {
            let (code, out, err) = call(&["netscenario", command, "--help"]);
            assert_eq!(code, exit::OK, "{command}");
            assert!(
                out.starts_with(&format!("Usage: netscenario {command}")),
                "{out}"
            );
            assert!(out.contains(word), "{command}: {out}");
            assert_eq!(err, "");
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
    fn an_unknown_command_names_itself_and_points_to_the_help() {
        let (code, _, err) = call(&["netscenario", "frobnicate"]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("unknown command 'frobnicate'") && err.contains("--help"));
    }

    #[test]
    fn a_wrong_command_line_is_a_usage_error_for_every_command() {
        for (args, expected) in [
            (&["run"][..], "the scenario file is missing"),
            (
                &["run", "a.yaml", "b.yaml"][..],
                "unexpected extra argument 'b.yaml'",
            ),
            (
                &["run", "a.yaml", "--seed", "x"][..],
                "--seed needs a whole number",
            ),
            (&["run", "a.yaml", "--settle", "soon"][..], "--settle:"),
            (&["run", "a.yaml", "--bin", "soon"][..], "--bin:"),
            (
                &["run", "a.yaml", "--recovery-basis", "sent"][..],
                "--recovery-basis is send or arrival",
            ),
            (
                &["run", "a.yaml", "--recovery-window", "0"][..],
                "--recovery-window needs at least 1 bin",
            ),
            (
                &["run", "a.yaml", "--recovery-fraction", "150"][..],
                "--recovery-fraction is a percent from above 0 to 100",
            ),
            (
                &["run", "a.yaml", "--capture", "bad name"][..],
                "--capture: 'bad name'",
            ),
            (&["replay"][..], "the directory of a run is missing"),
            (&["report"][..], "the result directory is missing"),
            (
                &["report", "d", "--format", "pdf"][..],
                "--format: the format is md, html or junit, not 'pdf'",
            ),
            (&["report", "a", "b"][..], "unexpected extra argument 'b'"),
            (&["replay", "a", "b"][..], "unexpected extra argument 'b'"),
            (
                &["run", "a.yaml", "--repetitions", "x"][..],
                "--repetitions needs a whole number",
            ),
            (
                &["run", "a.yaml", "--repetitions", "4"][..],
                "repetitions is 4; it is from 5",
            ),
            (
                &["run", "a.yaml", "--repetitions", "5", "--confidence", "100"][..],
                "level of an interval",
            ),
            (
                &["run", "a.yaml", "--repetitions", "5", "--statistic", "mode"][..],
                "--statistic: the statistic is mean or median, not 'mode'",
            ),
            (
                &["run", "a.yaml", "--statistic", "median"][..],
                "--statistic needs --repetitions",
            ),
            (
                &["run", "a.yaml", "--confidence", "90"][..],
                "--confidence needs --repetitions",
            ),
            (
                &["run", "a.yaml", "--recovery-baseline", "x"][..],
                "--recovery-baseline needs a whole number of bins",
            ),
            (
                &["run", "a.yaml", "--verbose"][..],
                "unknown option '--verbose'",
            ),
            (&["validate"][..], "the scenario file is missing"),
            (
                &["validate", "a", "--out", "x"][..],
                "unknown option '--out'",
            ),
            (
                &["schema", "extra"][..],
                "unexpected extra argument 'extra'",
            ),
            (
                &["doctor", "extra"][..],
                "unexpected extra argument 'extra'",
            ),
            (&["clean", "--force"][..], "unknown option '--force'"),
        ] {
            let mut full = vec!["netscenario"];
            full.extend_from_slice(args);
            let (code, out, err) = call(&full);
            assert_eq!(code, exit::USAGE, "{args:?}");
            assert_eq!(out, "", "{args:?}");
            assert!(err.contains(expected), "{args:?}: {err}");
            assert!(
                err.contains("Usage: netscenario"),
                "{args:?}: the help follows the message"
            );
        }
    }

    #[test]
    fn the_examples_are_valid_and_the_answer_says_what_they_need() {
        for file in ["minimal", "lte-like", "satellite", "lossy-both-ways"] {
            let path = scenarios().join(format!("{file}.yaml"));
            let path = path.to_str().unwrap();
            let (code, out, err) = call(&["netscenario", "validate", path]);
            assert_eq!((code, err.as_str()), (exit::OK, ""), "{file}");
            assert!(out.starts_with("ok: "), "{out}");
            assert!(out.contains("needs: point-to-point, ipv4"), "{out}");
        }
        let lte = scenarios().join("lte-like.yaml");
        let (_, out, _) = call(&["netscenario", "validate", lte.to_str().unwrap(), "--json"]);
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(json["valid"], true);
        assert_eq!(json["nodes"], 2);
        assert!(
            json["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "jitter")
        );
    }

    /// `expand` writes the long form of a shortcut: read back it is the same scenario, and it names what the shortcut made.
    #[test]
    fn expand_writes_the_long_form_of_a_shortcut_that_reads_back_the_same() {
        let path = scenarios().join("dumbbell.yaml");
        let path = path.to_str().unwrap();
        let (code, out, err) = call(&["netscenario", "expand", path]);
        assert_eq!((code, err.as_str()), (exit::OK, ""));
        assert!(!out.contains("dumbbell:"), "{out}");
        assert!(
            out.contains("router-left") && out.contains("bottleneck"),
            "{out}"
        );
        let original = netscenario_model::Scenario::from_yaml(
            &std::fs::read_to_string(path).unwrap(),
            "original",
        )
        .unwrap();
        let again = netscenario_model::Scenario::from_yaml(&out, "expanded").unwrap();
        assert_eq!(original, again);
        let (code, json, _) = call(&["netscenario", "expand", path, "--json"]);
        assert_eq!(code, exit::OK);
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["topology"]["links"].as_array().unwrap().len(), 5);
    }

    /// `expand --seed` writes the events that a model or a repeating event makes with that seed, which is what a run with it
    /// does: the file with `every:` has more steps, and the same seed gives the same text. A seed that is no number, and a
    /// help or a missing file, are told apart from a good call.
    #[test]
    fn expand_with_a_seed_writes_the_events_that_the_repeats_make() {
        let path = scenarios().join("flapping-link.yaml");
        let path = path.to_str().unwrap();
        let (code, plain, err) = call(&["netscenario", "expand", path]);
        assert_eq!((code, err.as_str()), (exit::OK, ""));
        let (code, seeded, err) = call(&["netscenario", "expand", path, "--seed", "4"]);
        assert_eq!((code, err.as_str()), (exit::OK, ""));
        let steps = |text: &str| text.matches("- at:").count();
        assert!(
            steps(&seeded) > steps(&plain),
            "{} steps against {}",
            steps(&seeded),
            steps(&plain)
        );
        assert!(seeded.contains("seed: 4"), "{seeded}");
        assert!(!seeded.contains("every:"), "{seeded}");
        let (_, again, _) = call(&["netscenario", "expand", path, "--seed", "4"]);
        assert_eq!(seeded, again);
        // Canary: the seed has to be a number, and a call with no file is a mistake of the command line
        let (code, out, err) = call(&["netscenario", "expand", path, "--seed", "four"]);
        assert_eq!((code, out.as_str()), (exit::USAGE, ""));
        assert!(
            err.contains("--seed needs a whole number, found 'four'"),
            "{err}"
        );
        let (code, _, err) = call(&["netscenario", "expand"]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("the scenario file is missing"), "{err}");
        let (code, out, _) = call(&["netscenario", "expand", "--help"]);
        assert_eq!(code, exit::OK);
        assert!(out.starts_with("Usage: netscenario expand"), "{out}");
        let (code, _, err) = call(&["netscenario", "expand", "/nonexistent/x.yaml"]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("cannot read '/nonexistent/x.yaml'"), "{err}");
    }

    /// `show` names the model of a link: the files of a trace with their format and rows, and the states of a chain.
    #[test]
    fn show_names_the_model_of_a_link() {
        let show = |name: &str| {
            let path = scenarios().join(name);
            let (code, out, err) = call(&["netscenario", "show", path.to_str().unwrap()]);
            assert_eq!((code, err.as_str()), (exit::OK, ""), "{name}");
            out
        };
        let trace = show("cell-drive.yaml");
        assert!(
            trace.contains("trace both cell-drive.csv (csv, "),
            "{trace}"
        );
        assert!(trace.contains(" rows, "), "{trace}");
        let walk = show("mahimahi-walk.yaml");
        assert!(walk.contains("(mahimahi, "), "{walk}");
        let chain = show("signal-states.yaml");
        assert!(chain.contains(" states"), "{chain}");
        let (code, out, _) = call(&["netscenario", "show", "--help"]);
        assert_eq!(code, exit::OK);
        assert!(out.starts_with("Usage: netscenario show"), "{out}");
        let (code, _, err) = call(&["netscenario", "show"]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("the scenario file is missing"), "{err}");
    }

    /// `show` lists the nodes, the links, the routes and the path of each workload; as Graphviz and as JSON too.
    #[test]
    fn show_lists_the_network_the_routes_and_the_paths() {
        let path = scenarios().join("two-paths.yaml");
        let path = path.to_str().unwrap();
        let (code, out, err) = call(&["netscenario", "show", path]);
        assert_eq!((code, err.as_str()), (exit::OK, ""));
        assert!(out.contains("fast") && out.contains("router"), "{out}");
        assert!(
            out.contains("media: client -[client-fast]-> fast -[fast-server]-> server"),
            "{out}"
        );
        assert!(
            out.contains("metric 10 *") && out.contains("metric 20 *"),
            "{out}"
        );
        let (_, dot, _) = call(&["netscenario", "show", path, "--format", "dot"]);
        assert!(
            dot.starts_with("graph netscenario {") && dot.contains("\"fast\" [shape=ellipse]"),
            "{dot}"
        );
        let (_, json, _) = call(&["netscenario", "show", path, "--format=json"]);
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            json["paths"][0]["links"],
            serde_json::json!(["client-fast", "fast-server"])
        );
        assert_eq!(json["routes"][0]["given"], true);
        // Canary: a format that does not exist, and a file that is not a scenario, are refused
        let (code, _, err) = call(&["netscenario", "show", path, "--format", "svg"]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("text, dot, json"), "{err}");
        let invalid = scenarios().join("invalid/routes-loop.yaml");
        let (code, _, err) = call(&["netscenario", "show", invalid.to_str().unwrap()]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("lead in a circle"), "{err}");
    }

    /// Canary: validation can fail. Every faulty file of the corpus is refused with exit code 2 and a message
    /// that names a place in the file.
    #[test]
    fn every_faulty_file_is_refused_with_its_lines() {
        let mut count = 0;
        for entry in std::fs::read_dir(scenarios().join("invalid")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            count += 1;
            let shown = path.to_str().unwrap();
            let (code, out, err) = call(&["netscenario", "validate", shown]);
            assert_eq!(code, exit::USAGE, "{shown}");
            assert_eq!(out, "");
            assert!(err.starts_with("error: "), "{shown}: {err}");
            let (code, out, err) = call(&["netscenario", "validate", shown, "--json"]);
            assert_eq!((code, err.as_str()), (exit::USAGE, ""), "{shown}");
            let json: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(json["valid"], false);
            assert!(!json["problems"].as_array().unwrap().is_empty(), "{shown}");
        }
        assert!(count >= 40, "only {count} faulty files were found");
    }

    #[test]
    fn a_file_that_cannot_be_read_says_so() {
        let (code, out, err) = call(&["netscenario", "validate", "/nonexistent/x.yaml"]);
        assert_eq!(code, exit::USAGE);
        assert_eq!(out, "");
        assert!(err.contains("cannot read '/nonexistent/x.yaml'"), "{err}");
    }

    #[test]
    fn a_problem_in_json_has_its_place_and_its_help() {
        let dir = std::env::temp_dir().join(format!("netscenario-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bad.yaml");
        std::fs::write(
            &file,
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, server]\n  links:\n    - id: l\n      between: [a, sever]\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: server, rate: 1mbit, packet-size: 1200b }\n",
        )
        .unwrap();
        let (code, out, _) = call(&["netscenario", "validate", file.to_str().unwrap(), "--json"]);
        assert_eq!(code, exit::USAGE);
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        let problem = &json["problems"][0];
        assert_eq!(problem["line"], 8);
        assert_eq!(problem["path"], "topology.links[0].between[1]");
        assert!(
            problem["help"]
                .as_str()
                .unwrap()
                .contains("did you mean 'server'"),
            "{problem}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_schema_is_the_one_of_the_model_and_is_json() {
        let (code, out, err) = call(&["netscenario", "schema"]);
        assert_eq!((code, err.as_str()), (exit::OK, ""));
        assert_eq!(out, netscenario_model::json_schema());
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(json["title"], "netscenario scenario");
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

    /// The timers are measured only when asked for (D-097): `doctor` alone has no section for them, and with `--timers` the
    /// sections, the checks and the measurements are there. A late timer is a warning, so the exit code is the one of the
    /// rest of the report.
    #[test]
    fn the_timers_are_in_the_report_only_with_the_flag() {
        let (plain_code, plain, _) = call(&["netscenario", "doctor"]);
        assert!(!plain.contains("Timers"), "{plain}");
        let (code, out, err) = call(&["netscenario", "doctor", "--timers"]);
        assert_eq!(err, "");
        assert_eq!(code, plain_code, "{out}");
        for word in [
            "\nTimers\n",
            "clock",
            "sleep of 200 us",
            "sleep of 1 ms",
            "load",
        ] {
            assert!(out.contains(word), "{word}\n{out}");
        }
        let (_, json, _) = call(&["netscenario", "doctor", "--timers", "--json"]);
        let json: serde_json::Value = serde_json::from_str(&json).expect("json on stdout");
        let ids: Vec<&str> = json["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["section"] == "timers")
            .filter_map(|c| c["id"].as_str())
            .collect();
        for id in [
            "timer-clock",
            "timer-sleep-200us",
            "timer-sleep-1ms",
            "timer-load",
            "timer-tc-change",
        ] {
            assert!(ids.contains(&id), "{id} in {ids:?}");
        }
        let sleeps = json["timers"]["timers"]["sleeps"].as_array().unwrap();
        assert_eq!(sleeps.len(), 2, "{json}");
        assert!(sleeps[0]["p99_ns"].as_u64().is_some() && sleeps[0]["samples"] == 1000);
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

    // Canary: the checks above must be able to fail. A wrong expectation has to be caught.
    #[test]
    #[should_panic(expected = "assertion")]
    fn canary_a_wrong_exit_code_is_detected() {
        let (code, _, _) = call(&["netscenario", "frobnicate"]);
        assert_eq!(code, exit::OK, "assertion: this must panic");
    }
}
