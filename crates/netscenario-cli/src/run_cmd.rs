//! `netscenario run`: run a scenario file and write its result.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

use netscenario_backend_kernel::{KernelBackend, KernelOptions};
use netscenario_core::{
    Basis, CancelToken, Error, RecoveryOptions, RunEvent, RunObserver, RunOptions, agent_next_to,
    rfc3339_utc, run,
};
use netscenario_model::Duration;

use crate::args::{Parsed, Spec};
use crate::exit;
use crate::interrupt::{Watch, code_of};
use crate::render::summary;
use crate::validate::{Loaded, load};

pub(crate) const RUN_HELP: &str = "\
Usage: netscenario run <file> [--out <dir>] [--seed <n>] [--settle <duration>] [--bin <duration>] [--packets] [--json]
                       [--recovery-basis send|arrival] [--recovery-baseline <bins>] [--recovery-window <bins>]
                       [--recovery-fraction <percent>]

Runs a scenario: builds the network in network namespaces, starts the agents in the nodes, lets the
workloads run for the duration of the scenario, and writes the result. Nothing outside the namespaces
that it creates is touched, and everything is removed afterwards, also after an error.

  --out <dir>          where to write the result (default: netscenario-results/<name>-<time>)
  --seed <n>           use this seed instead of the one in the file (to repeat a run that drew its own)
  --settle <duration>  how long to wait after the last packet for the packets still in the network
                       (for example 2s; default: until the queues of the links are empty, and 0.3 s more)
  --bin <duration>     the width of a bin of the series of each workload (for example 500ms; default 1s). The
                       series is written to series-<id>.jsonl; see docs/RESULTS.md
  --recovery-basis     the view the time to recover is made from: send (default) or arrival
  --recovery-baseline  how many bins before the disturbance make the baseline level (default 5)
  --recovery-window    how many bins a window has; the level has to hold for that long (default 3)
  --recovery-fraction  the share of the baseline that has to be reached again, in percent (default 90)
  --packets            also write one line of JSON for each packet of each workload
  --json               print result.json on standard output instead of the summary

The result is described in docs/RESULTS.md. Run `netscenario doctor` first on a new machine.

Ctrl-C (or SIGTERM) stops the workloads at once, waits for the packets that are still in the network and
writes the result of the part that ran, which says how long it ran (`timing.interrupted_ns`). Ctrl-C a
second time quits at once; the network is removed all the same.

Exit codes: 0 the run finished, 2 the scenario or the command line is wrong (or the backend cannot
reproduce what the scenario asks for), 3 the machine does not allow the run (a tool or a permission is
missing), 4 the run failed, 130 or 143 the run was interrupted by SIGINT or SIGTERM (128 and the number of
the signal; with a result if the workloads had begun).
";

pub(crate) const RUN_SPEC: Spec = Spec {
    values: &[
        "out",
        "seed",
        "settle",
        "bin",
        "recovery-basis",
        "recovery-baseline",
        "recovery-window",
        "recovery-fraction",
    ],
    flags: &["packets", "json"],
};

/// The width of a bin and the parameters of the recovery from the command line (D-070), or what is wrong with them.
fn series_options(parsed: &Parsed) -> Result<(std::time::Duration, RecoveryOptions), String> {
    let mut recovery = RecoveryOptions::default();
    let bin = match parsed.value("bin").map(Duration::from_str).transpose() {
        Ok(bin) => bin.map_or(std::time::Duration::from_secs(1), std::time::Duration::from),
        Err(error) => return Err(format!("--bin: {error}")),
    };
    if let Some(word) = parsed.value("recovery-basis") {
        recovery.basis = Basis::from_word(word)
            .ok_or_else(|| format!("--recovery-basis is send or arrival, not '{word}'"))?;
    }
    let count = |name: &str| -> Result<Option<u32>, String> {
        parsed
            .value(name)
            .map(str::parse::<u32>)
            .transpose()
            .map_err(|_| format!("--{name} needs a whole number of bins"))
    };
    if let Some(bins) = count("recovery-baseline")? {
        recovery.baseline_bins = bins;
    }
    if let Some(bins) = count("recovery-window")? {
        recovery.window_bins = bins;
    }
    if let Some(text) = parsed.value("recovery-fraction") {
        let percent: f64 = text.trim_end_matches('%').parse().map_err(|_| {
            "--recovery-fraction needs a number of percent, for example 90".to_owned()
        })?;
        recovery.fraction = percent / 100.0;
    }
    recovery.check()?;
    Ok((bin, recovery))
}

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

/// What the progress line says about an event, or nothing for the end.
fn describe(event: &RunEvent) -> Option<String> {
    let seconds = |time: std::time::Duration| format!("{:.0}", time.as_secs_f64().ceil());
    Some(match event {
        RunEvent::Provisioned { nodes, links } => {
            format!("network built ({nodes} nodes, {links} link), starting the receivers")
        }
        RunEvent::ReceiversReady { .. } => "receivers ready, starting the senders".to_owned(),
        RunEvent::Started { duration } => format!("running, 0 of {} s", seconds(*duration)),
        RunEvent::Progress { elapsed, total } => {
            format!("running, {} of {} s", elapsed.as_secs(), seconds(*total))
        }
        RunEvent::Settling { wait } => {
            format!(
                "waiting {:.1} s for the packets in the network",
                wait.as_secs_f64()
            )
        }
        RunEvent::Collecting => "reading the records".to_owned(),
        RunEvent::TearingDown => "removing the network".to_owned(),
        RunEvent::Interrupted => "interrupted, waiting for the packets in the network".to_owned(),
        // The line is cleared when the run returns; `Finished` and later events say nothing new
        _ => return None,
    })
}

/// The progress line on a terminal: one line that is written over again (D-031).
struct ProgressLine;

impl ProgressLine {
    /// Erases the line, so that the summary or the error starts at the beginning of a line.
    fn clear() {
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "\r\x1b[2K");
        let _ = err.flush();
    }

    fn show(text: &str) {
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "\r\x1b[2K  {text}");
        let _ = err.flush();
    }
}

impl RunObserver for ProgressLine {
    fn event(&self, event: &RunEvent) {
        if let Some(text) = describe(event) {
            Self::show(&text);
        }
    }
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

/// What the options with numbers say.
struct Numbers {
    seed: Option<u64>,
    settle: Option<std::time::Duration>,
    /// The width of a bin of the series.
    bin: std::time::Duration,
    recovery: RecoveryOptions,
}

/// Reads the seed, the settle time, and the bin and the recovery of the series.
fn numbers(parsed: &Parsed) -> Result<Numbers, String> {
    let Ok(seed) = parsed.value("seed").map(str::parse::<u64>).transpose() else {
        return Err("--seed needs a whole number from 0 to 18446744073709551615".to_owned());
    };
    let settle = parsed
        .value("settle")
        .map(Duration::from_str)
        .transpose()
        .map_err(|error| format!("--settle: {error}"))?
        .map(std::time::Duration::from);
    let (bin, recovery) = series_options(parsed)?;
    Ok(Numbers {
        seed,
        settle,
        bin,
        recovery,
    })
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
    let Numbers {
        seed,
        settle,
        bin,
        recovery,
    } = match numbers(&parsed) {
        Ok(numbers) => numbers,
        Err(message) => return crate::usage_error(err, "run", &message, RUN_HELP),
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
    options.bin = bin;
    options.recovery = recovery;
    // Ctrl-C stops the workloads and keeps what was recorded (D-066)
    let token = CancelToken::new();
    options = options.with_cancel(token.clone()).with_partial_on_cancel();

    let mut backend = match KernelBackend::new(KernelOptions::for_scenario(&exe, &scenario)) {
        Ok(backend) => backend,
        Err(error) => return report(&error, &loaded, err),
    };
    // On a terminal one line shows how far the run is; elsewhere (a file, a CI log) one line says what runs
    let on_terminal = std::io::stderr().is_terminal();
    if on_terminal {
        options = options.with_observer(Arc::new(ProgressLine));
        ProgressLine::show("building the network");
    } else {
        let _ = writeln!(
            err,
            "running {} for {} ...",
            scenario.name, scenario.duration
        );
    }
    let watch = Watch::start(token, on_terminal);
    let outcome = run(&mut backend, &scenario, &options);
    if on_terminal {
        ProgressLine::clear();
    }
    let signal = watch.as_ref().and_then(Watch::signal);
    conclude(
        outcome,
        &Ending {
            json: parsed.flag("json"),
            out_dir: &out_dir,
            signal,
        },
        &loaded,
        out,
        err,
    )
}

/// How the run ended, as far as the printing and the exit code are concerned.
struct Ending<'a> {
    /// Whether standard output is the JSON of the result.
    json: bool,
    /// Where the result is.
    out_dir: &'a std::path::Path,
    /// The signal that interrupted the run, if one did.
    signal: Option<i32>,
}

/// Prints the result or the error of a run and returns the exit code.
fn conclude(
    outcome: Result<netscenario_core::RunResult, Error>,
    ending: &Ending<'_>,
    loaded: &Loaded,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    // 128 and the number of the signal; a run is only interrupted by a signal, and SIGINT is the one a person sends
    let by_signal = || ending.signal.map_or(code_of(2), code_of);
    match outcome {
        Ok(result) => {
            if ending.json {
                let _ = write!(out, "{}", result.to_json());
            } else {
                let _ = write!(out, "{}", summary(&result, ending.out_dir));
            }
            // A run that was interrupted has a result, and the exit code says that it is not the whole one
            if result.timing.interrupted_ns.is_some() {
                by_signal()
            } else {
                exit::OK
            }
        }
        Err(Error::Cancelled) if ending.signal.is_some() => {
            let _ = writeln!(
                err,
                "netscenario: interrupted before the workloads began, so there is no result"
            );
            by_signal()
        }
        Err(error) => report(&error, loaded, err),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn parsed(args: &[&str]) -> Parsed {
        let mut all = vec!["a.yaml"];
        all.extend_from_slice(args);
        Parsed::parse(&all, &RUN_SPEC).unwrap()
    }

    /// The defaults are those of the library, and every option moves its own parameter (D-070).
    #[test]
    fn the_options_of_the_series_and_the_recovery_are_read() {
        let (bin, recovery) = series_options(&parsed(&[])).unwrap();
        assert_eq!(bin, std::time::Duration::from_secs(1));
        assert_eq!(recovery, RecoveryOptions::default());
        let (bin, recovery) = series_options(&parsed(&[
            "--bin",
            "250ms",
            "--recovery-basis",
            "arrival",
            "--recovery-baseline",
            "8",
            "--recovery-window",
            "4",
            "--recovery-fraction",
            "95%",
        ]))
        .unwrap();
        assert_eq!(bin, std::time::Duration::from_millis(250));
        assert_eq!(
            (recovery.basis, recovery.baseline_bins, recovery.window_bins),
            (Basis::Arrival, 8, 4)
        );
        assert!((recovery.fraction - 0.95).abs() < 1e-12);
        // Canary: a value without the percent sign is the same percent
        let (_, plain) = series_options(&parsed(&["--recovery-fraction", "95"])).unwrap();
        assert!((plain.fraction - 0.95).abs() < 1e-12);
    }

    #[test]
    fn every_phase_of_a_run_has_a_line_and_the_end_has_none() {
        let s = Duration::from_secs;
        for (event, expected) in [
            (
                RunEvent::Provisioned { nodes: 2, links: 1 },
                Some("network built (2 nodes, 1 link), starting the receivers"),
            ),
            (
                RunEvent::ReceiversReady { count: 1 },
                Some("receivers ready, starting the senders"),
            ),
            (
                RunEvent::Started { duration: s(30) },
                Some("running, 0 of 30 s"),
            ),
            (
                RunEvent::Progress {
                    elapsed: Duration::from_millis(12_900),
                    total: Duration::from_millis(29_500),
                },
                Some("running, 12 of 30 s"),
            ),
            (
                RunEvent::Settling {
                    wait: Duration::from_millis(1120),
                },
                Some("waiting 1.1 s for the packets in the network"),
            ),
            (RunEvent::Collecting, Some("reading the records")),
            (RunEvent::TearingDown, Some("removing the network")),
            (
                RunEvent::Interrupted,
                Some("interrupted, waiting for the packets in the network"),
            ),
            (RunEvent::Finished, None),
        ] {
            assert_eq!(describe(&event).as_deref(), expected, "{event:?}");
        }
    }
}
