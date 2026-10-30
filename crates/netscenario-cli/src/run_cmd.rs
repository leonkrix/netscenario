//! `netscenario run`: run a scenario file and write its result.

use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::SystemTime;

use netscenario_backend_kernel::{KernelBackend, KernelOptions};
use netscenario_core::{Error, RunOptions, agent_next_to, rfc3339_utc, run};
use netscenario_model::Duration;

use crate::args::{Parsed, Spec};
use crate::exit;
use crate::render::summary;
use crate::validate::{Loaded, load};

pub(crate) const RUN_HELP: &str = "\
Usage: netscenario run <file> [--out <dir>] [--seed <n>] [--settle <duration>] [--packets] [--json]

Runs a scenario: builds the network in network namespaces, starts the agents in the nodes, lets the
workloads run for the duration of the scenario, and writes the result. Nothing outside the namespaces
that it creates is touched, and everything is removed afterwards, also after an error.

  --out <dir>          where to write the result (default: netscenario-results/<name>-<time>)
  --seed <n>           use this seed instead of the one in the file (to repeat a run that drew its own)
  --settle <duration>  how long to wait after the last packet for the packets still in the network
                       (for example 2s; default: one second and twice the longest delay)
  --packets            also write one line of JSON for each packet of each workload
  --json               print result.json on standard output instead of the summary

The result is described in docs/RESULTS.md. Run `netscenario doctor` first on a new machine.

Exit codes: 0 the run finished, 2 the scenario or the command line is wrong (or the backend cannot
reproduce what the scenario asks for), 3 the machine does not allow the run (a tool or a permission is
missing), 4 the run failed.
";

pub(crate) const RUN_SPEC: Spec = Spec {
    values: &["out", "seed", "settle"],
    flags: &["packets", "json"],
};

/// The directory for a result when none is given: the name of the scenario and the time, so that two runs do
/// not overwrite each other.
fn default_out(name: &str) -> PathBuf {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let stamp: String = rfc3339_utc(seconds)
        .chars()
        .filter(|c| *c != '-' && *c != ':')
        .collect();
    PathBuf::from("netscenario-results").join(format!("{name}-{stamp}"))
}

/// Writes what went wrong and returns the exit code that fits.
fn report(error: &Error, loaded: &Loaded, err: &mut impl Write) -> u8 {
    match error {
        Error::Capability(capability) => {
            let rendered = capability.render(&loaded.name, &loaded.text);
            if rendered.trim().is_empty() {
                let _ = write!(err, "{capability}");
            } else {
                let _ = write!(err, "{rendered}");
            }
            exit::USAGE
        }
        Error::InvalidScenario(_) => {
            let _ = writeln!(err, "netscenario: {error}");
            exit::USAGE
        }
        Error::Backend { .. } => {
            let _ = writeln!(
                err,
                "netscenario: {error}\n  run `netscenario doctor` to see what this machine can do and how to fix what it cannot"
            );
            exit::ENVIRONMENT
        }
        _ => {
            let _ = writeln!(err, "netscenario: {error}");
            exit::RUN_FAILED
        }
    }
}

pub(crate) fn run_command<S: AsRef<str>>(
    args: &[S],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let parsed = match Parsed::parse(args, &RUN_SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return crate::usage_error(err, "run", &message, RUN_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{RUN_HELP}");
        return exit::OK;
    }
    let path = match parsed.single("the scenario file") {
        Ok(path) => path,
        Err(message) => return crate::usage_error(err, "run", &message, RUN_HELP),
    };
    let Ok(seed) = parsed.value("seed").map(str::parse::<u64>).transpose() else {
        return crate::usage_error(
            err,
            "run",
            "--seed needs a whole number from 0 to 18446744073709551615",
            RUN_HELP,
        );
    };
    let settle = match parsed.value("settle").map(Duration::from_str).transpose() {
        Ok(settle) => settle.map(std::time::Duration::from),
        Err(error) => {
            return crate::usage_error(err, "run", &format!("--settle: {error}"), RUN_HELP);
        }
    };
    let loaded = match load(path) {
        Ok(loaded) => loaded,
        Err(message) => {
            let _ = writeln!(err, "netscenario: {message}");
            return exit::USAGE;
        }
    };
    let mut scenario = match &loaded.scenario {
        Ok(scenario) => scenario.clone(),
        Err(error) => {
            let _ = write!(err, "{}", error.render());
            return exit::USAGE;
        }
    };
    if seed.is_some() {
        scenario.seed = seed;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            let _ = writeln!(err, "netscenario: cannot find my own program: {error}");
            return exit::ENVIRONMENT;
        }
    };
    let agent = agent_next_to(&exe);
    if !agent.is_file() {
        let _ = writeln!(
            err,
            "netscenario: the agent {} was not found; it belongs next to the netscenario program (`cargo install` and `cargo build` put it there)",
            agent.display()
        );
        return exit::ENVIRONMENT;
    }
    let out_dir = parsed
        .value("out")
        .map_or_else(|| default_out(scenario.name.as_str()), PathBuf::from);
    let mut options = RunOptions::new(agent, &out_dir);
    options.settle = settle;
    options.export_packets = parsed.flag("packets");

    let mut backend = match KernelBackend::new(KernelOptions::for_scenario(&exe, &scenario)) {
        Ok(backend) => backend,
        Err(error) => return report(&error, &loaded, err),
    };
    let _ = writeln!(
        err,
        "running {} for {} ...",
        scenario.name, scenario.duration
    );
    match run(&mut backend, &scenario, &options) {
        Ok(result) => {
            if parsed.flag("json") {
                let _ = write!(out, "{}", result.to_json());
            } else {
                let _ = write!(out, "{}", summary(&result, &out_dir));
            }
            exit::OK
        }
        Err(error) => report(&error, &loaded, err),
    }
}
