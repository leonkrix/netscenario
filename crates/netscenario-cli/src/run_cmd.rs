//! `netscenario run`: run a scenario file and write its result.

use std::cell::Cell;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use netscenario_backend_kernel::{KernelBackend, KernelOptions};
use netscenario_core::bootstrap::Statistic;
use netscenario_core::{
    CancelToken, Error, Repetition, RunEvent, RunObserver, RunOptions, RunSetOptions, RunSetResult,
    agent_next_to, rfc3339_utc, run, run_set,
};
use netscenario_model::{Duration, MeasureOverrides, Name, Probability, RecoveryBasis, Scenario};

use crate::args::{Parsed, Spec};
use crate::exit;
use crate::interrupt::{Watch, code_of};
use crate::render::{set_summary, summary};
use crate::validate::{Loaded, load};

pub(crate) const RUN_HELP: &str = "\
Usage: netscenario run <file> [--out <dir>] [--seed <n>] [--settle <duration>] [--json] [--packets]
                       [--repetitions <n>] [--statistic mean|median] [--confidence <percent>]
                       [--bin <duration>] [--capture all|<link>,...] [--recovery-basis send|arrival]
                       [--recovery-baseline <bins>] [--recovery-window <bins>] [--recovery-fraction <percent>]
                       [--param <name>=<value>]... [--allow-outside-files]

Runs a scenario: builds the network in network namespaces, starts the agents in the nodes, lets the
workloads run for the duration of the scenario, and writes the result. Nothing outside the namespaces
that it creates is touched, and everything is removed afterwards, also after an error.

  --out <dir>          where to write the result (default: netscenario-results/<name>-<time>)
  --param <name>=<value>  give a value to a parameter of the scenario (`params:` in the file, as often as there are
                       parameters); the result says which values the run was made with (docs/SCENARIO-FORMAT.md)
  --seed <n>           use this seed instead of the one in the file (to repeat a run that drew its own)
  --settle <duration>  how long to wait after the last packet for the packets still in the network
                       (for example 2s; default: until the queues of the links are empty, and 0.3 s more)
  --json               print result.json (or runset.json, with --repetitions) on standard output instead of the summary
  --allow-outside-files  let the scenario read files (a table of delay samples) from outside the directory of the
                       scenario file; without it a path that leads out of that directory is refused

Repetitions run the scenario several times, one after the other, and say how much each metric varies between the runs
(see docs/STATISTICS.md). Run number i uses a seed derived from the seed of the scenario (or of --seed), so the runs draw
unrelated loss patterns and the whole set can be repeated; each run is in a directory run-001, run-002, ... of --out, and
runset.json holds the values of every run and an interval for each metric:

  --repetitions <n>    run the scenario n times, from 5 to 1000 (fewer runs say too little for an interval)
  --statistic <name>   what the interval is around: mean (default) or median
  --confidence <p>     the level of the interval in percent, above 0 and below 100 (default 95)

These options name keys of the section `measure` of the file and replace them for this run; the result says what
was used (see docs/SCENARIO-FORMAT.md):

  --bin <duration>     measure.series.bin: the width of a bin of the series (for example 500ms; default 1s)
  --capture <links>    measure.capture: capture the interfaces of these links (`all`, or ids separated by commas) as
                       pcap files, at both ends
  --recovery-basis     measure.recovery.basis: the view the time to recover is made from: send (default) or arrival
  --recovery-baseline  measure.recovery.baseline-bins: bins before the disturbance for the baseline (default 5)
  --recovery-window    measure.recovery.window-bins: bins in a row that have to be back at the level (default 3)
  --recovery-fraction  measure.recovery.fraction: the share of the baseline to reach again, in percent (default 90)
  --packets            measure.packets: also write one line of JSON for each packet of each workload

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
        "capture",
        "recovery-basis",
        "recovery-baseline",
        "recovery-window",
        "recovery-fraction",
        "repetitions",
        "statistic",
        "confidence",
        crate::args::PARAM,
    ],
    flags: &["packets", "json", crate::validate::ALLOW_OUTSIDE_FILES],
};

/// What the command line asks for in the section `measure` of the file (D-070, D-073), or what is wrong with it. `links` are
/// the ids of the links of the scenario, for `--capture all`.
fn measure_overrides(parsed: &Parsed, links: &[Name]) -> Result<MeasureOverrides, String> {
    let mut overrides = MeasureOverrides::default();
    overrides.bin = parsed
        .value("bin")
        .map(Duration::from_str)
        .transpose()
        .map_err(|error| format!("--bin: {error}"))?;
    if let Some(word) = parsed.value("recovery-basis") {
        overrides.basis = Some(match word {
            "send" => RecoveryBasis::Send,
            "arrival" => RecoveryBasis::Arrival,
            other => {
                return Err(format!(
                    "--recovery-basis is send or arrival, not '{other}'"
                ));
            }
        });
    }
    let count = |name: &str| -> Result<Option<u32>, String> {
        parsed
            .value(name)
            .map(str::parse::<u32>)
            .transpose()
            .map_err(|_| format!("--{name} needs a whole number of bins"))
    };
    overrides.baseline_bins = count("recovery-baseline")?;
    overrides.window_bins = count("recovery-window")?;
    for (name, value) in [
        ("recovery-baseline", overrides.baseline_bins),
        ("recovery-window", overrides.window_bins),
    ] {
        if value == Some(0) {
            return Err(format!("--{name} needs at least 1 bin"));
        }
    }
    if let Some(text) = parsed.value("recovery-fraction") {
        let percent: f64 = text.trim_end_matches('%').parse().map_err(|_| {
            "--recovery-fraction needs a number of percent, for example 90".to_owned()
        })?;
        overrides.fraction = fraction_of(percent);
        if overrides.fraction.is_none() {
            return Err("--recovery-fraction is a percent from above 0 to 100".to_owned());
        }
    }
    if let Some(text) = parsed.value("capture") {
        overrides.capture = Some(if text == "all" {
            links.to_vec()
        } else {
            text.split(',')
                .map(|id| {
                    Name::new(id.trim())
                        .map_err(|error| format!("--capture: '{}': {error}", id.trim()))
                })
                .collect::<Result<Vec<Name>, String>>()?
        });
    }
    if parsed.flag("packets") {
        overrides.packets = Some(true);
    }
    Ok(overrides)
}

/// A percent from above 0 to 100 as a probability, to the millionth of a percent; `None` outside it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is checked to lie between 0 and 100,000,000 before the cast"
)]
fn fraction_of(percent: f64) -> Option<Probability> {
    let micro = (percent * 1_000_000.0).round();
    (0.0..=100_000_000.0)
        .contains(&micro)
        .then(|| Probability::from_micro_percent(micro as u32))
        .flatten()
}

/// What the command line asks for in repetitions, or nothing for a single run. `--statistic` and `--confidence` mean nothing
/// without `--repetitions`, and saying so is better than ignoring them.
fn set_options(parsed: &Parsed) -> Result<Option<RunSetOptions>, String> {
    let Some(text) = parsed.value("repetitions") else {
        for name in ["statistic", "confidence"] {
            if parsed.value(name).is_some() {
                return Err(format!("--{name} needs --repetitions"));
            }
        }
        return Ok(None);
    };
    let repetitions: u32 = text
        .parse()
        .map_err(|_| "--repetitions needs a whole number, for example 10".to_owned())?;
    let mut set = RunSetOptions::new(repetitions);
    if let Some(word) = parsed.value("statistic") {
        set.statistic =
            Statistic::from_str(word).map_err(|message| format!("--statistic: {message}"))?;
    }
    if let Some(text) = parsed.value("confidence") {
        let percent: f64 = text
            .trim_end_matches('%')
            .parse()
            .map_err(|_| "--confidence needs a number of percent, for example 95".to_owned())?;
        set.level = percent / 100.0;
    }
    set.check().map_err(|error| match error {
        Error::Run { message, .. } => format!("--repetitions, --confidence: {message}"),
        other => other.to_string(),
    })?;
    Ok(Some(set))
}

/// The set that the checks of the file need, when the command line asked for none, and the refusal of a set that cannot judge them
/// (a file that judges one run, fewer runs than the file needs, another interval than the one the keys `ci95-*` name).
fn with_needed_runs(
    set: Option<RunSetOptions>,
    scenario: &Scenario,
) -> Result<Option<RunSetOptions>, String> {
    let set = match (set, netscenario_core::needed_repetitions(scenario)) {
        (None, Some(needed)) => Some(RunSetOptions::new(needed)),
        (set, _) => set,
    };
    if let Some(set) = &set {
        netscenario_core::check_set_options(scenario, set).map_err(|error| match error {
            Error::Run { message, .. } => message,
            other => other.to_string(),
        })?;
    }
    Ok(set)
}

/// The directory for a result when none is given: the name of the scenario and the time, so that two runs do
/// not overwrite each other.
/// The directory of the scenario file, as an absolute path: where the programs of your own run (D-041).
fn scenario_directory(path: &str) -> Option<PathBuf> {
    let directory = Path::new(path).parent()?;
    let directory = if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        directory
    };
    std::fs::canonicalize(directory).ok()
}

pub(crate) fn default_out(name: &str) -> PathBuf {
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
pub(crate) struct ProgressLine;

impl ProgressLine {
    /// Erases the line, so that the summary or the error starts at the beginning of a line.
    pub(crate) fn clear() {
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "\r\x1b[2K");
        let _ = err.flush();
    }

    pub(crate) fn show(text: &str) {
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
pub(crate) fn report(error: &Error, loaded: &Loaded, err: &mut impl Write) -> u8 {
    match error {
        Error::Capability(capability) => {
            let rendered = match &loaded.scenario {
                Ok(scenario) => capability.render_for(scenario, &loaded.name, &loaded.text),
                Err(_) => capability.render(&loaded.name, &loaded.text),
            };
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
}

/// Reads the seed and the settle time.
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
    Ok(Numbers { seed, settle })
}

/// The scenario of the file with the options of the run over it: the seed, and the keys of `measure` that an option names
/// replace the keys of the file for this run (D-073). Checked again afterwards, because an option can be as wrong as a key.
fn prepared(
    parsed: &Parsed,
    loaded: &Loaded,
    seed: Option<u64>,
    err: &mut impl Write,
) -> Result<Scenario, u8> {
    let mut scenario = match &loaded.scenario {
        Ok(scenario) => scenario.clone(),
        Err(error) => {
            let _ = write!(err, "{}", error.render());
            return Err(exit::USAGE);
        }
    };
    if seed.is_some() {
        scenario.seed = seed;
    }
    let links: Vec<Name> = scenario
        .topology
        .links
        .iter()
        .map(|l| l.id.clone())
        .collect();
    match measure_overrides(parsed, &links) {
        Ok(overrides) => scenario.measure.apply(&overrides),
        Err(message) => return Err(crate::usage_error(err, "run", &message, RUN_HELP)),
    }
    if let Some(problem) = scenario.validate().first() {
        let place = problem.path.as_deref().unwrap_or("the scenario");
        let _ = writeln!(
            err,
            "netscenario: the options of the run make the scenario invalid: {place}: {}",
            problem.message
        );
        return Err(exit::USAGE);
    }
    Ok(scenario)
}

/// The scenario file with the values of its parameters from the command line.
fn load_of(parsed: &Parsed, path: &str) -> Result<Loaded, String> {
    load(
        path,
        parsed.flag(crate::validate::ALLOW_OUTSIDE_FILES),
        parsed.all(crate::args::PARAM),
    )
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
    let Numbers { seed, settle } = match numbers(&parsed) {
        Ok(numbers) => numbers,
        Err(message) => return crate::usage_error(err, "run", &message, RUN_HELP),
    };
    // A wrong option is a wrong command line whether or not the file can be read; the links come with the file
    if let Err(message) = measure_overrides(&parsed, &[]) {
        return crate::usage_error(err, "run", &message, RUN_HELP);
    }
    let set = match set_options(&parsed) {
        Ok(set) => set,
        Err(message) => return crate::usage_error(err, "run", &message, RUN_HELP),
    };
    let loaded = match load_of(&parsed, path) {
        Ok(loaded) => loaded,
        Err(message) => {
            let _ = writeln!(err, "netscenario: {message}");
            return exit::USAGE;
        }
    };
    let scenario = match prepared(&parsed, &loaded, seed, err) {
        Ok(scenario) => scenario,
        Err(code) => return code,
    };
    // The checks of the file say how many runs they need (D-094): that many are made when the command line does not ask for more
    let set = match with_needed_runs(set, &scenario) {
        Ok(set) => set,
        Err(message) => return crate::usage_error(err, "run", &message, RUN_HELP),
    };

    let (exe, agent) = match programs(err) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let out_dir = parsed
        .value("out")
        .map_or_else(|| default_out(scenario.name.as_str()), PathBuf::from);
    let mut options = RunOptions::new(agent, &out_dir);
    options.settle = settle;
    // The programs of your own run where the scenario file is (D-041)
    options.working_dir = scenario_directory(path);
    // Ctrl-C stops the workloads and keeps what was recorded (D-066)
    let token = CancelToken::new();
    options = options.with_cancel(token.clone()).with_partial_on_cancel();

    if let Some(set) = set {
        let target = Repeating {
            set: &set,
            scenario: &scenario,
            exe: &exe,
            json: parsed.flag("json"),
            out_dir: &out_dir,
        };
        return run_repetitions(&target, options, token, &loaded, out, err);
    }
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

/// This program and the agent next to it, or the exit code and the message for the one that is missing.
fn programs(err: &mut impl Write) -> Result<(PathBuf, PathBuf), u8> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            let _ = writeln!(err, "netscenario: cannot find my own program: {error}");
            return Err(exit::ENVIRONMENT);
        }
    };
    let agent = agent_next_to(&exe);
    if !agent.is_file() {
        let _ = writeln!(
            err,
            "netscenario: the agent {} was not found; it belongs next to the netscenario program (`cargo install` and `cargo build` put it there)",
            agent.display()
        );
        return Err(exit::ENVIRONMENT);
    }
    Ok((exe, agent))
}

/// What a set of runs is run with.
struct Repeating<'a> {
    set: &'a RunSetOptions,
    scenario: &'a Scenario,
    exe: &'a Path,
    json: bool,
    out_dir: &'a Path,
}

/// The progress line of a set: what the run says, behind the number of the repetition.
struct SetProgress(Arc<Mutex<String>>);

impl RunObserver for SetProgress {
    fn event(&self, event: &RunEvent) {
        if let Some(text) = describe(event) {
            let prefix = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            ProgressLine::show(&format!("{prefix}{text}"));
        }
    }
}

/// Runs the scenario once for each repetition and prints the set (ASR-02, ASR-03).
fn run_repetitions(
    target: &Repeating<'_>,
    mut options: RunOptions,
    token: CancelToken,
    loaded: &Loaded,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let on_terminal = std::io::stderr().is_terminal();
    let prefix = Arc::new(Mutex::new(String::new()));
    if on_terminal {
        options = options.with_observer(Arc::new(SetProgress(Arc::clone(&prefix))));
        ProgressLine::show("building the network");
    } else {
        let _ = writeln!(
            err,
            "running {} {} times for {} each ...",
            target.scenario.name, target.set.repetitions, target.scenario.duration
        );
    }
    let watch = Watch::start(token, on_terminal);
    // The place of the repetition that is running, for the message when one fails
    let current = Cell::new(0_u32);
    let outcome = run_set(
        target.scenario,
        &options,
        target.set,
        &mut |scenario, run_options| {
            let mut backend =
                KernelBackend::new(KernelOptions::for_scenario(target.exe, scenario))?;
            run(&mut backend, scenario, run_options)
        },
        &mut |repetition: &Repetition| {
            current.set(repetition.index + 1);
            let label = format!(
                "repetition {} of {}",
                repetition.index + 1,
                repetition.total
            );
            if on_terminal {
                let mut prefix = prefix.lock().unwrap_or_else(PoisonError::into_inner);
                *prefix = format!("{label}: ");
                ProgressLine::show(&format!("{label}: building the network"));
            } else {
                let _ = writeln!(err, "{label} (seed {}) ...", repetition.seed);
            }
        },
    );
    if on_terminal {
        ProgressLine::clear();
    }
    let signal = watch.as_ref().and_then(Watch::signal);
    conclude_set(outcome, target, current.get(), signal, loaded, out, err)
}

/// Prints the set or the error of a set and returns the exit code that fits.
fn conclude_set(
    outcome: Result<RunSetResult, Error>,
    target: &Repeating<'_>,
    current: u32,
    signal: Option<i32>,
    loaded: &Loaded,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    match outcome {
        Ok(set) => {
            if target.json {
                let _ = write!(out, "{}", set.to_json());
            } else {
                let _ = write!(out, "{}", set_summary(&set, target.out_dir));
            }
            // A check of the file that does not hold is a failed assertion (D-094)
            if set.expect.as_ref().is_some_and(|expect| !expect.passed) {
                exit::ASSERTION_FAILED
            } else {
                exit::OK
            }
        }
        Err(Error::Cancelled) => {
            let _ = writeln!(
                err,
                "netscenario: interrupted during repetition {current} of {}: there is no set, because an interval from fewer runs than asked for would be a made-up number; the runs that finished are in {}",
                target.set.repetitions,
                target.out_dir.display()
            );
            ending_code(signal)
        }
        Err(error) => {
            let code = report(&error, loaded, err);
            if current > 1 {
                let _ = writeln!(
                    err,
                    "  repetition {current} of {} failed; the {} runs before it are in {}",
                    target.set.repetitions,
                    current - 1,
                    target.out_dir.display()
                );
            }
            code
        }
    }
}

/// 128 and the number of the signal that interrupted, or that of SIGINT.
fn ending_code(signal: Option<i32>) -> u8 {
    signal.map_or(code_of(2), code_of)
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
            } else if result.expect.as_ref().is_some_and(|expect| !expect.passed) {
                // A check of the file that does not hold is a failed assertion (D-094)
                exit::ASSERTION_FAILED
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
    fn the_options_of_the_measure_section_are_read() {
        let links: Vec<Name> = vec!["access".parse().unwrap()];
        let none = measure_overrides(&parsed(&[]), &links).unwrap();
        assert_eq!(none, MeasureOverrides::default());
        let given = measure_overrides(
            &parsed(&[
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
                "--capture",
                "all",
                "--packets",
            ]),
            &links,
        )
        .unwrap();
        assert_eq!(given.bin, Some("250ms".parse().unwrap()));
        assert_eq!(given.basis, Some(RecoveryBasis::Arrival));
        assert_eq!((given.baseline_bins, given.window_bins), (Some(8), Some(4)));
        assert_eq!(given.fraction.unwrap().micro_percent(), 95_000_000);
        assert_eq!(given.capture.as_deref(), Some(&links[..]));
        assert_eq!(given.packets, Some(true));
        // A value without the percent sign is the same percent, and a list names the links
        let plain = measure_overrides(
            &parsed(&["--recovery-fraction", "95", "--capture", "a,b"]),
            &links,
        )
        .unwrap();
        assert_eq!(plain.fraction.unwrap().micro_percent(), 95_000_000);
        assert_eq!(plain.capture.unwrap().len(), 2);
    }

    /// Repetitions are read with their defaults and each option moves its own parameter.
    #[test]
    fn the_options_of_repetitions_are_read() {
        assert_eq!(set_options(&parsed(&[])).unwrap(), None);
        let plain = set_options(&parsed(&["--repetitions", "10"]))
            .unwrap()
            .unwrap();
        assert_eq!(plain, RunSetOptions::new(10));
        assert_eq!((plain.statistic, plain.level), (Statistic::Mean, 0.95));
        let given = set_options(&parsed(&[
            "--repetitions",
            "20",
            "--statistic",
            "median",
            "--confidence",
            "90%",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(
            (given.repetitions, given.statistic),
            (20, Statistic::Median)
        );
        assert!((given.level - 0.9).abs() < 1e-12);
    }

    /// Canary: each wrong value is refused with the option named, and nothing is read as a default instead.
    #[test]
    fn wrong_options_of_repetitions_are_refused_with_the_option_named() {
        for (args, expected) in [
            (
                &["--repetitions", "x"][..],
                "--repetitions needs a whole number",
            ),
            (
                &["--repetitions", "-1"][..],
                "--repetitions needs a whole number",
            ),
            (&["--repetitions", "4"][..], "repetitions is 4"),
            (&["--repetitions", "1001"][..], "repetitions is 1001"),
            (
                &["--repetitions", "5", "--confidence", "0"][..],
                "level of an interval",
            ),
            (
                &["--repetitions", "5", "--confidence", "nine"][..],
                "--confidence needs a number",
            ),
            (
                &["--repetitions", "5", "--statistic", "p99"][..],
                "--statistic:",
            ),
            (
                &["--statistic", "mean"][..],
                "--statistic needs --repetitions",
            ),
            (
                &["--confidence", "95"][..],
                "--confidence needs --repetitions",
            ),
        ] {
            let message = set_options(&parsed(args)).unwrap_err();
            assert!(message.contains(expected), "{args:?}: {message}");
        }
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
