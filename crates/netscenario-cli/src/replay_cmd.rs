//! `netscenario replay`: run a recorded run again and say whether it came out the same (ENV-03, D-075).

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use netscenario_backend_kernel::{KernelBackend, KernelOptions};
use netscenario_core::{
    CancelToken, Error, Recorded, RunOptions, Verdict, agent_next_to, compare, replay_json, run,
    scenario_diff,
};

use crate::args::{Parsed, Spec};
use crate::exit;
use crate::interrupt::{Watch, code_of};
use crate::run_cmd::{ProgressLine, default_out, report};
use crate::validate::{Loaded, load};

pub(crate) const REPLAY_HELP: &str = "\
Usage: netscenario replay <dir> [--out <dir>] [--scenario <file>] [--json]

Runs a recorded run again, with the scenario it was made with (scenario.json in its directory) and the seed it
had, and says whether the same packets were lost. <dir> is the directory of a run: the one that `netscenario run`
wrote. See docs/STATISTICS.md for runs of a set (replay one of its run-NNN directories).

The replay is checked before it starts: scenario.json has to be the scenario that result.json was made from, by its
fingerprint. It waits as long as the original for the packets still in the network. Delays and rates are not compared,
because they vary from run to run; the lost packets of a seed are, and so are the machines (the kernel, `tc`, the
processor), because the lost packets of a seed depend on them.

  --out <dir>        where to write the result of the replay (default: netscenario-results/<name>-replay-<time>)
  --scenario <file>  also compare this scenario file with the recorded scenario, field by field, and replay nothing
                     if they differ (the replay always runs the recorded scenario, never the file)
  --json             print the comparison as JSON on standard output instead of the summary

Exit codes: 0 the replay lost the same packets (or the loss of one of the runs was not promised to repeat, which the
summary explains), 1 the replay lost other packets although the loss repeats from the seed, 2 the directory or the
file is wrong (or the scenario file differs), 3 the machine does not allow the run, 4 the run failed, 130 or 143
interrupted.
";

pub(crate) const REPLAY_SPEC: Spec = Spec {
    values: &["out", "scenario"],
    flags: &["json"],
};

/// The differences between a scenario file and the recorded scenario, as lines for a message.
fn file_differences(recorded: &Recorded, path: &str, err: &mut impl Write) -> Result<(), u8> {
    let loaded = match load(path) {
        Ok(loaded) => loaded,
        Err(message) => {
            let _ = writeln!(err, "netscenario replay: {message}");
            return Err(exit::USAGE);
        }
    };
    let file = match &loaded.scenario {
        Ok(scenario) => scenario,
        Err(error) => {
            let _ = write!(err, "{}", error.render());
            return Err(exit::USAGE);
        }
    };
    let found = scenario_diff(recorded, file);
    if found.is_empty() {
        return Ok(());
    }
    let _ = writeln!(
        err,
        "netscenario replay: {path} is not the scenario of this run: {} field{} differ:",
        found.len(),
        if found.len() == 1 { "" } else { "s" }
    );
    for difference in found.iter().take(20) {
        let _ = writeln!(
            err,
            "  {}: the run had {}, the file has {}",
            difference.path, difference.recorded, difference.other
        );
    }
    if found.len() > 20 {
        let _ = writeln!(err, "  ... and {} more", found.len() - 20);
    }
    let _ = writeln!(
        err,
        "nothing is replayed. The replay runs the recorded scenario ({}/scenario.json); to run the file, use `netscenario run`.",
        recorded.dir.display()
    );
    Err(exit::USAGE)
}

pub(crate) fn replay_command<S: AsRef<str>>(
    args: &[S],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let parsed = match Parsed::parse(args, &REPLAY_SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return crate::usage_error(err, "replay", &message, REPLAY_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{REPLAY_HELP}");
        return exit::OK;
    }
    let dir = match parsed.single("the directory of a run") {
        Ok(dir) => PathBuf::from(dir),
        Err(message) => return crate::usage_error(err, "replay", &message, REPLAY_HELP),
    };
    let recorded = match Recorded::load(&dir) {
        Ok(recorded) => recorded,
        Err(error) => {
            let _ = writeln!(err, "netscenario replay: {error}");
            return exit::USAGE;
        }
    };
    if let Some(path) = parsed.value("scenario")
        && let Err(code) = file_differences(&recorded, path, err)
    {
        return code;
    }
    let out_dir = parsed.value("out").map_or_else(
        || default_out(&format!("{}-replay", recorded.scenario.name)),
        PathBuf::from,
    );
    if same_directory(&out_dir, &dir) {
        let _ = writeln!(
            err,
            "netscenario replay: --out is the directory of the run that is replayed; the replay would overwrite it"
        );
        return exit::USAGE;
    }
    replay_run(&recorded, &out_dir, parsed.flag("json"), out, err)
}

/// Runs the recorded scenario again into `out_dir` and prints the comparison.
fn replay_run(
    recorded: &Recorded,
    out_dir: &Path,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let dir = recorded.dir.as_path();
    let scenario = recorded.scenario_to_replay();
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
    // The text of scenario.json stands in for a file in the messages of a refusal
    let loaded = Loaded {
        name: dir
            .join(netscenario_core::SCENARIO_FILE)
            .display()
            .to_string(),
        text: std::fs::read_to_string(dir.join(netscenario_core::SCENARIO_FILE))
            .unwrap_or_default(),
        scenario: Ok(scenario.clone()),
    };
    let mut options = RunOptions::new(agent, out_dir);
    // As long as the original, so that a packet is late or lost for the same reason
    options.settle = Some(recorded.settle());
    // The programs of your own run where the command is run: the directory of the original is not recorded
    options.working_dir = std::env::current_dir().ok();
    let token = CancelToken::new();
    options = options.with_cancel(token.clone()).with_partial_on_cancel();
    let mut backend = match KernelBackend::new(KernelOptions::for_scenario(&exe, &scenario)) {
        Ok(backend) => backend,
        Err(error) => return report(&error, &loaded, err),
    };
    let on_terminal = std::io::stderr().is_terminal();
    if on_terminal {
        options = options.with_observer(Arc::new(ProgressLine));
        ProgressLine::show("building the network");
    } else {
        let _ = writeln!(
            err,
            "replaying {} for {} (seed {}) ...",
            scenario.name,
            scenario.duration,
            recorded
                .result
                .seed
                .value
                .map_or_else(|| "none".to_owned(), |seed| seed.to_string())
        );
    }
    let watch = Watch::start(token, on_terminal);
    let outcome = run(&mut backend, &scenario, &options);
    if on_terminal {
        ProgressLine::clear();
    }
    let signal = watch.as_ref().and_then(Watch::signal);
    match outcome {
        Ok(replayed) => {
            if replayed.timing.interrupted_ns.is_some() {
                let _ = writeln!(
                    err,
                    "netscenario replay: interrupted, so the replay is not compared; its part is in {}",
                    out_dir.display()
                );
                return signal.map_or(code_of(2), code_of);
            }
            let comparison = compare(&recorded.result, dir, &replayed, out_dir);
            if json {
                let _ = writeln!(out, "{}", replay_json(recorded, out_dir, &comparison));
            } else {
                let _ = write!(
                    out,
                    "{}",
                    crate::render::replay_summary(recorded, out_dir, &comparison)
                );
            }
            match comparison.verdict {
                Verdict::Differs => exit::NOT_REPRODUCED,
                _ => exit::OK,
            }
        }
        Err(Error::Cancelled) if signal.is_some() => {
            let _ = writeln!(
                err,
                "netscenario replay: interrupted before the workloads began, so there is nothing to compare"
            );
            signal.map_or(code_of(2), code_of)
        }
        Err(error) => report(&error, &loaded, err),
    }
}

/// Whether two paths name the same directory (also if one of them does not exist yet).
fn same_directory(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
