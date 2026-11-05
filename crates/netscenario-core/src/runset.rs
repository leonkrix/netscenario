//! A set of runs of one scenario with seeds derived from one seed, and what the set says about each metric (ASR-02, ASR-03,
//! D-074).
//!
//! [`run_set`] runs the scenario `repetitions` times, one after the other. Repetition `i` (from 0) runs the scenario with
//! the seed `derive_repetition_seed(set seed, i)`, so the repetitions draw unrelated loss patterns and traffic, and the
//! set can be repeated from its seed alone. The seed of the set is the one in the scenario; without one the set draws one and
//! records it. Every run is written to a directory of its own, `run-001`, `run-002`, and so on, in the directory of the set,
//! with the files described in `docs/RESULTS.md`; the set itself is `runset.json`.
//!
//! The set looks at the same numbers as the Python layer and the report: [`metric_values`] gives the metrics of one run
//! under their names (`media.delay.p99`, `media.loss_ratio`, ...), in SI units. For each name that every run has, the set
//! records the values of the runs, the usual summary and an [`Interval`] around the mean or the median
//! ([`crate::bootstrap`]).
//!
//! **What it refuses.** Fewer than [`MIN_RUNS`] or more than [`MAX_REPETITIONS`] repetitions, a level or a number of draws
//! the bootstrap would refuse: all before the first run. A repetition that fails ends the set with that error (the finished
//! runs stay on disk), and so does a cancel or a run that was cut short; there is no interval from fewer runs than were asked
//! for, because that would be a silent approximation. A metric that some runs have and others do not (the delay of a run in
//! which no packet arrived) gets no interval and is listed under `partial` with the number of runs that have it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::bootstrap::{
    DEFAULT_LEVEL, DEFAULT_RESAMPLES, Interval, MIN_RUNS, Method, Statistic, check_parameters,
    confidence_interval,
};
use crate::error::Error;
use crate::machine::Provenance;
use crate::result::{RunResult, ScenarioInfo};
use crate::run::RunOptions;
use crate::seed::{derive_bootstrap_seed, derive_repetition_seed, draw_seed};
use netscenario_model::Scenario;

/// Always `netscenario-runset`.
pub const RUNSET_FORMAT: &str = "netscenario-runset";
/// The version of the format of the set, like that of the result: 0 until 1.0.
pub const RUNSET_VERSION: u32 = 0;
/// The name of the file of the set in the directory of the set.
pub const RUNSET_FILE: &str = "runset.json";
/// The most repetitions of a set; a run takes seconds and writes files, so more is a sweep and not a repetition.
pub const MAX_REPETITIONS: u32 = 1_000;

/// What a set asks for.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RunSetOptions {
    /// How many times the scenario is run: from [`MIN_RUNS`] to [`MAX_REPETITIONS`].
    pub repetitions: u32,
    /// What the interval is around. Default: the mean.
    pub statistic: Statistic,
    /// The level of the interval, from 0 to 1 (exclusive). Default 0.95.
    pub level: f64,
    /// How many draws the bootstrap makes. Default 10,000.
    pub resamples: u32,
}

impl RunSetOptions {
    /// `repetitions` runs, an interval of 95 % around the mean from 10,000 draws.
    #[must_use]
    pub fn new(repetitions: u32) -> Self {
        Self {
            repetitions,
            statistic: Statistic::Mean,
            level: DEFAULT_LEVEL,
            resamples: DEFAULT_RESAMPLES,
        }
    }

    /// Checks the options; the set does it before the first run.
    ///
    /// # Errors
    ///
    /// [`Error::Run`] naming the option that is wrong.
    pub fn check(&self) -> Result<(), Error> {
        let wrong = |message: String| Error::Run {
            action: "start the set of runs".to_owned(),
            message,
        };
        if (self.repetitions as usize) < MIN_RUNS || self.repetitions > MAX_REPETITIONS {
            return Err(wrong(format!(
                "repetitions is {}; it is from {MIN_RUNS} (fewer runs say too little about the spread for an interval) to {MAX_REPETITIONS}",
                self.repetitions
            )));
        }
        check_parameters(self.level, self.resamples).map_err(|error| wrong(error.to_string()))
    }
}

/// What the set tells the caller before each run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Repetition {
    /// The place of the run in the set, from 0.
    pub index: u32,
    /// How many runs the set has.
    pub total: u32,
    /// The seed of the run.
    pub seed: u64,
    /// The directory the run writes to.
    pub directory: PathBuf,
}

/// The seed of the set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SetSeed {
    /// The seed from which the seeds of the runs and of the bootstrap were derived.
    pub value: u64,
    /// Whether the set drew it because the scenario has none. Repeat the set with `--seed value`.
    pub drawn: bool,
}

/// How the intervals were made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Confidence {
    /// What the intervals are around.
    pub statistic: Statistic,
    /// The level, from 0 to 1.
    pub level: f64,
    /// The number of draws of the bootstrap.
    pub resamples: u32,
    /// How the ends were chosen.
    pub method: Method,
}

/// One run of the set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RepetitionInfo {
    /// The place of the run in the set, from 0.
    pub index: u32,
    /// The seed of the run.
    pub seed: u64,
    /// The directory of the run, relative to the directory of the set.
    pub directory: String,
    /// Whether the same seed gives the same lost packets in this run (`seed.loss_reproducible` of its result).
    pub loss_reproducible: bool,
}

/// What the runs of the set say about one metric that every run has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MetricSet {
    /// The name, as [`metric_values`] gives it.
    pub name: String,
    /// The value of each run, in the order of the runs.
    pub values: Vec<f64>,
    /// The mean of the values.
    pub mean: f64,
    /// The median of the values.
    pub median: f64,
    /// The smallest value.
    pub min: f64,
    /// The largest value.
    pub max: f64,
    /// The sample standard deviation (divided by `n - 1`).
    pub stddev: f64,
    /// The interval around the statistic of the set.
    pub interval: Interval,
}

/// A metric that some runs have and others do not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PartialMetric {
    /// The name.
    pub name: String,
    /// In how many runs it was there.
    pub present_in: usize,
    /// How many runs there were.
    pub of: usize,
}

/// What a set of runs found out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RunSetResult {
    /// Always [`RUNSET_FORMAT`].
    pub format: String,
    /// The version of the format, [`RUNSET_VERSION`].
    pub version: u32,
    /// The scenario: its name, and the hash of the scenario **without its seed**, because every run has a seed of its own.
    pub scenario: ScenarioInfo,
    /// Where, when and with what the first run was made (the runs of a set are made one after the other on one machine).
    pub provenance: Provenance,
    /// The seed of the set.
    pub seed: SetSeed,
    /// How many runs.
    pub repetitions: u32,
    /// How the intervals were made.
    pub confidence: Confidence,
    /// The runs, in order.
    pub runs: Vec<RepetitionInfo>,
    /// The metrics that every run has, by name.
    pub metrics: Vec<MetricSet>,
    /// The metrics that only some runs have; they have no interval.
    #[serde(default)]
    pub partial: Vec<PartialMetric>,
}

impl RunSetResult {
    /// The metric called `name`, if every run has it.
    #[must_use]
    pub fn metric(&self, name: &str) -> Option<&MetricSet> {
        self.metrics.iter().find(|metric| metric.name == name)
    }

    /// The result as JSON text, indented, with a line feed at the end.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self)
            .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"));
        text.push('\n');
        text
    }

    /// Reads a set from JSON text.
    ///
    /// # Errors
    ///
    /// The text is not JSON, is not a set of runs, or has a version this program does not read.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let set: Self = serde_json::from_str(text)
            .map_err(|error| format!("this is not a netscenario set of runs: {error}"))?;
        if set.format != RUNSET_FORMAT {
            return Err(format!(
                "this is not a netscenario set of runs (format '{}')",
                set.format
            ));
        }
        if set.version != RUNSET_VERSION {
            return Err(format!(
                "this set has version {}; this program reads version {RUNSET_VERSION}",
                set.version
            ));
        }
        Ok(set)
    }
}

/// The statistics of a time under `prefix`, in seconds.
#[allow(clippy::cast_precision_loss)] // times of a run are far below 2^52 nanoseconds
fn delay_values(
    put: &mut impl FnMut(String, f64),
    prefix: &str,
    delay: &crate::stats::DelaySummary,
) {
    for (name, value) in [
        ("min", delay.min_ns),
        ("mean", delay.mean_ns),
        ("p50", delay.p50_ns),
        ("p90", delay.p90_ns),
        ("p95", delay.p95_ns),
        ("p99", delay.p99_ns),
        ("p999", delay.p999_ns),
        ("max", delay.max_ns),
    ] {
        put(format!("{prefix}.{name}"), value as f64 / 1e9);
    }
}

/// A nanosecond count of a run is far below 2^52, where a double is exact.
#[allow(clippy::cast_precision_loss)]
fn count_of(value: u64) -> f64 {
    value as f64
}

/// The numbers of one TCP workload under their names (D-088).
fn tcp_values(put: &mut impl FnMut(String, f64), tcp: &crate::result::TcpResult) {
    let count = count_of;
    let seconds = |nanoseconds: u64| count_of(nanoseconds) / 1e9;
    let id = &tcp.id;
    if let Some(mean) = tcp.goodput.as_ref().and_then(|g| g.mean_bits_per_second) {
        put(format!("{id}.goodput.mean"), count(mean));
    }
    if let Some(completion) = tcp.completion_ns() {
        put(format!("{id}.completion"), seconds(completion));
    }
    for (name, value) in [
        ("bytes_sent", tcp.bytes_sent),
        ("bytes_received", tcp.bytes_received),
        ("connections", u64::from(tcp.connections)),
        ("failed_connections", u64::from(tcp.failed_connections)),
        ("retransmitted_segments", tcp.retransmitted_segments),
    ] {
        put(format!("{id}.{name}"), count(value));
    }
    if let Some(rtt) = &tcp.rtt {
        delay_values(put, &format!("{id}.rtt"), rtt);
    }
    if let Some(requests) = &tcp.requests {
        put(format!("{id}.sent"), count(requests.sent));
        put(format!("{id}.answered"), count(requests.answered));
        put(format!("{id}.failed"), count(requests.failed));
        if let Some(latency) = &requests.latency {
            delay_values(put, &format!("{id}.latency"), latency);
        }
        if let Some(trip) = &requests.round_trip {
            delay_values(put, &format!("{id}.round_trip"), trip);
        }
    }
}

/// The metrics of one run under their names, in SI units: seconds, bits per second, ratios and counts.
///
/// | Name | What |
/// | --- | --- |
/// | `<workload>.sent`, `.received`, `.lost`, `.duplicates`, `.reordered` | counts of packets |
/// | `<workload>.loss_ratio` | `lost / sent`, 0 to 1 |
/// | `<workload>.delay.min`, `.mean`, `.p50`, `.p90`, `.p95`, `.p99`, `.p999`, `.max` | one-way delay in seconds; absent if no packet arrived |
/// | `<workload>.jitter.last`, `.mean`, `.max` | interarrival jitter (RFC 3550) in seconds; absent for fewer than two arrivals |
/// | `<workload>.throughput.mean` | the payload that arrived over the duration of the workload, in bits per second |
/// | `<workload>.loss_runs.longest`, `.mean` | the runs of lost packets, in packets |
/// | `<workload>.round_trip.min`, `.mean`, `.p50`, `.p90`, `.p95`, `.p99`, `.p999`, `.max` | for a workload with `echo: true`: the round trip of the packets that came back, in seconds; absent if none did (D-087) |
/// | `<workload>.echo.received`, `.lost`, `.loss_ratio` | for a workload with `echo: true`: the packets that came back, those that did not (lost on the way there or back), and their share |
/// | `<tcp>.goodput.mean` | a TCP workload: the payload that arrived over the transfer, in bits per second (D-086) |
/// | `<tcp>.completion` | the time from the first connect to the end of the last connection that did its work, in seconds |
/// | `<tcp>.bytes_sent`, `.bytes_received`, `.connections`, `.failed_connections`, `.retransmitted_segments` | counts |
/// | `<tcp>.rtt.min`, `.mean`, `.p50`, `.p90`, `.p95`, `.p99`, `.p999`, `.max` | the smoothed round trip time of the kernel, in seconds |
/// | `<tcp>.sent`, `.answered`, `.failed` | `request-response`: the requests |
/// | `<tcp>.latency.*`, `<tcp>.round_trip.*` | `request-response`: from the moment a request was due, and from the moment it was sent, to the last byte of its response, in seconds, with the same statistics as the delay; answered requests only |
/// | `<program>.<metric>.last`, `.min`, `.max` | a metric that a program of your own reported (the unit is the program's) |
#[must_use]
pub fn metric_values(result: &RunResult) -> BTreeMap<String, f64> {
    // Counts and nanoseconds of a run are far below 2^52, where a double is exact
    #[allow(clippy::cast_precision_loss)]
    fn count(value: u64) -> f64 {
        value as f64
    }
    fn seconds(nanoseconds: u64) -> f64 {
        count(nanoseconds) / 1e9
    }
    let mut values = BTreeMap::new();
    let mut put = |name: String, value: f64| {
        values.insert(name, value);
    };
    for workload in &result.workloads {
        let id = &workload.id;
        let stats = &workload.stats;
        for (name, value) in [
            ("sent", stats.sent),
            ("received", stats.received),
            ("lost", stats.lost),
            ("duplicates", stats.duplicates),
            ("reordered", stats.reordered),
        ] {
            put(format!("{id}.{name}"), count(value));
        }
        put(format!("{id}.loss_ratio"), stats.loss_ratio);
        if let Some(delay) = &stats.delay {
            for (name, value) in [
                ("min", delay.min_ns),
                ("mean", delay.mean_ns),
                ("p50", delay.p50_ns),
                ("p90", delay.p90_ns),
                ("p95", delay.p95_ns),
                ("p99", delay.p99_ns),
                ("p999", delay.p999_ns),
                ("max", delay.max_ns),
            ] {
                put(format!("{id}.delay.{name}"), seconds(value));
            }
        }
        if let Some(jitter) = &stats.jitter {
            for (name, value) in [
                ("last", jitter.last_ns),
                ("mean", jitter.mean_ns),
                ("max", jitter.max_ns),
            ] {
                put(format!("{id}.jitter.{name}"), seconds(value));
            }
        }
        if workload.duration_ns > 0 {
            let bits = count(workload.payload_received_bytes()) * 8.0;
            put(
                format!("{id}.throughput.mean"),
                bits / seconds(workload.duration_ns),
            );
        }
        put(
            format!("{id}.loss_runs.longest"),
            count(stats.loss_runs.longest),
        );
        put(format!("{id}.loss_runs.mean"), stats.loss_runs.mean);
        if let Some(echo) = &workload.echo {
            put(format!("{id}.echo.received"), count(echo.stats.received));
            put(format!("{id}.echo.lost"), count(echo.stats.lost));
            put(format!("{id}.echo.loss_ratio"), echo.stats.loss_ratio);
            if let Some(trip) = &echo.stats.delay {
                delay_values(&mut put, &format!("{id}.round_trip"), trip);
            }
        }
    }
    for tcp in &result.tcp {
        tcp_values(&mut put, tcp);
    }
    for program in &result.programs {
        for metric in &program.metrics {
            let name = format!("{}.{}", program.id, metric.name);
            put(format!("{name}.last"), metric.last);
            put(format!("{name}.min"), metric.min);
            put(format!("{name}.max"), metric.max);
        }
    }
    values
}

/// The summary of the values of every metric that all runs have, and the list of the ones that only some have.
///
/// The bootstrap of a metric is seeded from `set_seed` and the name of the metric ([`derive_bootstrap_seed`]).
///
/// # Errors
///
/// [`Error::Run`] if an interval cannot be computed: too few runs, or a value that is not a finite number.
pub fn aggregate(
    results: &[RunResult],
    set_seed: u64,
    options: &RunSetOptions,
) -> Result<(Vec<MetricSet>, Vec<PartialMetric>), Error> {
    let per_run: Vec<BTreeMap<String, f64>> = results.iter().map(metric_values).collect();
    let mut names: Vec<&String> = per_run.iter().flat_map(BTreeMap::keys).collect();
    names.sort();
    names.dedup();
    let mut metrics = Vec::new();
    let mut partial = Vec::new();
    for name in names {
        let values: Vec<f64> = per_run
            .iter()
            .filter_map(|run| run.get(name).copied())
            .collect();
        if values.len() < per_run.len() {
            partial.push(PartialMetric {
                name: name.clone(),
                present_in: values.len(),
                of: per_run.len(),
            });
            continue;
        }
        let interval = confidence_interval(
            &values,
            options.statistic,
            options.level,
            options.resamples,
            derive_bootstrap_seed(set_seed, name),
        )
        .map_err(|error| Error::Run {
            action: format!("compute the interval of '{name}'"),
            message: error.to_string(),
        })?;
        metrics.push(summary(name, values, interval));
    }
    Ok((metrics, partial))
}

fn summary(name: &str, values: Vec<f64>, interval: Interval) -> MetricSet {
    // The number of runs is at most MAX_REPETITIONS
    #[allow(clippy::cast_precision_loss)]
    let n = values.len() as f64;
    let mean = Statistic::Mean.of(&values);
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    MetricSet {
        name: name.to_owned(),
        median: Statistic::Median.of(&values),
        min: values.iter().copied().fold(f64::INFINITY, f64::min),
        max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        stddev: variance.sqrt(),
        mean,
        values,
        interval,
    }
}

/// The fingerprint of the scenario as the set records it: the hash of the scenario without its seed.
fn hash_without_seed(scenario: &Scenario) -> String {
    let mut plain = scenario.clone();
    plain.seed = None;
    crate::replay::scenario_hash(&plain)
}

/// Runs `scenario` `set.repetitions` times with `runner`, writes the runs and the set under `options.out_dir`, and returns
/// the set.
///
/// `runner` runs one scenario with the options it is given (the seed of the scenario and the directory are those of the
/// repetition); the usual one builds a backend and calls [`crate::run`]. `before` is told about each repetition before it
/// starts. The runs are one after the other: they share the machine, and a run in parallel would measure the other.
///
/// # Errors
///
/// - [`Error::Run`] for options that are wrong (nothing has run then) or if a file cannot be written,
/// - [`Error::Cancelled`] if `options.cancel` was cancelled before a run, or a run was cut short by it (its directory stays),
/// - the error of the first run that fails, unchanged; the runs before it stay on disk.
pub fn run_set(
    scenario: &Scenario,
    options: &RunOptions,
    set: &RunSetOptions,
    runner: &mut dyn FnMut(&Scenario, &RunOptions) -> Result<RunResult, Error>,
    before: &mut dyn FnMut(&Repetition),
) -> Result<RunSetResult, Error> {
    set.check()?;
    let failed = |action: &str, error: &std::io::Error| Error::Run {
        action: action.to_owned(),
        message: error.to_string(),
    };
    std::fs::create_dir_all(&options.out_dir)
        .map_err(|error| failed("create the directory for the set", &error))?;
    let drawn = scenario.seed.is_none();
    let set_seed = scenario.seed.unwrap_or_else(draw_seed);
    let mut results: Vec<RunResult> = Vec::new();
    let mut runs = Vec::new();
    for index in 0..set.repetitions {
        if options.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let seed = derive_repetition_seed(set_seed, index);
        let directory = format!("run-{:03}", index + 1);
        let mut run_options = options.clone();
        run_options.out_dir = options.out_dir.join(&directory);
        before(&Repetition {
            index,
            total: set.repetitions,
            seed,
            directory: run_options.out_dir.clone(),
        });
        let mut repetition = scenario.clone();
        repetition.seed = Some(seed);
        let result = runner(&repetition, &run_options)?;
        // A run that was cut short is a part of a run: the set does not use it and does not go on
        if result.timing.interrupted_ns.is_some() {
            return Err(Error::Cancelled);
        }
        runs.push(RepetitionInfo {
            index,
            seed,
            directory,
            loss_reproducible: result.seed.loss_reproducible,
        });
        results.push(result);
    }
    let (metrics, partial) = aggregate(&results, set_seed, set)?;
    let first = &results[0];
    let result = RunSetResult {
        format: RUNSET_FORMAT.to_owned(),
        version: RUNSET_VERSION,
        scenario: ScenarioInfo {
            name: scenario.name.to_string(),
            hash: hash_without_seed(scenario),
        },
        provenance: first.provenance.clone(),
        seed: SetSeed {
            value: set_seed,
            drawn,
        },
        repetitions: set.repetitions,
        confidence: Confidence {
            statistic: set.statistic,
            level: set.level,
            resamples: set.resamples,
            method: Method::of(set.statistic),
        },
        runs,
        metrics,
        partial,
    };
    std::fs::write(options.out_dir.join(RUNSET_FILE), result.to_json())
        .map_err(|error| failed("write the result of the set", &error))?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::result::tests::example;
    use crate::stats::DelaySummary;

    fn scenario(seed: Option<u64>) -> Scenario {
        let seed_line = seed.map_or_else(String::new, |seed| format!("seed: {seed}\n"));
        Scenario::from_yaml(
            &format!(
                "version: 0\nname: t\n{seed_line}duration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {{ delay: 20ms }}\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }}\n"
            ),
            "t.yaml",
        )
        .unwrap()
    }

    /// Where the set of the test `name` writes.
    fn written_to(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("netscenario-set-{name}-{}", std::process::id()))
    }

    /// The same, emptied first.
    fn directory(name: &str) -> PathBuf {
        let dir = written_to(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn options(name: &str) -> RunOptions {
        RunOptions::new("agent", directory(name))
    }

    /// A result whose loss ratio is `loss` and whose p99 delay is `p99_ms` milliseconds.
    fn result_with(loss: f64, p99_ms: u64) -> RunResult {
        let mut result = example();
        let stats = &mut result.workloads[0].stats;
        stats.loss_ratio = loss;
        stats.delay = Some(DelaySummary {
            min_ns: 1_000_000,
            mean_ns: 2_000_000,
            p50_ns: 2_000_000,
            p90_ns: 3_000_000,
            p95_ns: 4_000_000,
            p99_ns: p99_ms * 1_000_000,
            p999_ns: 9_000_000,
            max_ns: 10_000_000,
        });
        result
    }

    /// The metrics of the sample result, worked out by hand: 2 sent, 1 received, 1 lost of 2; 1000 byte packets; a duration of
    /// one nanosecond is too short to be a rate, so the sample says 1 second for the throughput.
    #[test]
    fn the_names_and_the_units_of_the_metrics_of_a_run() {
        let mut result = result_with(0.5, 7);
        result.workloads[0].duration_ns = 2_000_000_000;
        let values = metric_values(&result);
        let at = |name: &str| {
            *values
                .get(name)
                .unwrap_or_else(|| panic!("no {name} in {values:?}"))
        };
        assert!((at("w.sent") - 2.0).abs() < 1e-12);
        assert!((at("w.lost") - 1.0).abs() < 1e-12);
        assert!((at("w.loss_ratio") - 0.5).abs() < 1e-12);
        // Seconds: 7 ms is 0.007
        assert!((at("w.delay.p99") - 0.007).abs() < 1e-12);
        assert!((at("w.delay.max") - 0.010).abs() < 1e-12);
        // One packet of 1000 bytes arrived in 2 seconds: 8000 bits over 2 s
        assert!((at("w.throughput.mean") - 4_000.0).abs() < 1e-9);
        assert!((at("w.loss_runs.longest") - 0.0).abs() < 1e-12);
        // No jitter in the sample (fewer than two arrivals): no name for it, which the set treats as partial
        assert!(!values.contains_key("w.jitter.mean"));
    }

    /// The round trip of an echo is under `round_trip`, in seconds, and its packets under `echo` (D-087).
    #[test]
    fn the_names_of_the_metrics_of_an_echo() {
        let mut result = result_with(0.5, 7);
        let mut back = result.workloads[0].stats.clone();
        back.loss_ratio = 0.75;
        back.received = 1;
        back.lost = 3;
        back.delay = back.delay.map(|mut trip| {
            trip.p50_ns = 140_000_000;
            trip
        });
        result.workloads[0].echo = Some(crate::result::EchoResult {
            echoed: 1,
            echo_errors: 0,
            foreign_datagrams: 0,
            records_file: "flow-0.echo.records".to_owned(),
            stats: back,
        });
        let values = metric_values(&result);
        assert!((values["w.echo.loss_ratio"] - 0.75).abs() < 1e-12);
        assert!((values["w.echo.lost"] - 3.0).abs() < 1e-12);
        assert!((values["w.round_trip.p50"] - 0.140).abs() < 1e-12);
        // The one-way delay is not the round trip
        assert!((values["w.delay.p50"] - 0.002).abs() < 1e-12);
        // A workload without an echo has neither
        assert!(!metric_values(&result_with(0.5, 7)).contains_key("w.round_trip.p50"));
    }

    #[test]
    fn a_run_without_delays_has_no_delay_metrics() {
        let values = metric_values(&example());
        assert!(
            values.keys().all(|name| !name.contains(".delay.")),
            "{values:?}"
        );
    }

    /// The summary of five runs, worked out by hand: the losses 0.1, 0.2, 0.3, 0.4, 0.5 have the mean 0.3, the median 0.3 and
    /// the sample variance (0.04 + 0.01 + 0 + 0.01 + 0.04) / 4 = 0.025.
    #[test]
    fn the_summary_of_five_runs_is_worked_out_by_hand() {
        let results: Vec<RunResult> = [0.1, 0.2, 0.3, 0.4, 0.5]
            .iter()
            .map(|loss| result_with(*loss, 5))
            .collect();
        let (metrics, partial) = aggregate(&results, 1, &RunSetOptions::new(5)).unwrap();
        assert_eq!(partial, Vec::<PartialMetric>::new());
        let loss = metrics.iter().find(|m| m.name == "w.loss_ratio").unwrap();
        assert_eq!(loss.values, [0.1, 0.2, 0.3, 0.4, 0.5]);
        assert!((loss.mean - 0.3).abs() < 1e-12 && (loss.median - 0.3).abs() < 1e-12);
        assert!((loss.stddev - 0.025_f64.sqrt()).abs() < 1e-12);
        assert!((loss.min - 0.1).abs() < 1e-12 && (loss.max - 0.5).abs() < 1e-12);
        assert!(
            loss.interval.low < 0.3 && 0.3 < loss.interval.high,
            "{:?}",
            loss.interval
        );
        assert!(
            loss.interval.low >= 0.1 && loss.interval.high <= 0.5,
            "{:?}",
            loss.interval
        );
        // A metric that is the same in all runs has a point as its interval and no spread
        let sent = metrics.iter().find(|m| m.name == "w.sent").unwrap();
        assert_eq!(
            (sent.interval.low, sent.interval.high, sent.stddev),
            (2.0, 2.0, 0.0)
        );
        // The metrics are sorted by name
        let names: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    /// Canary: a metric that a run lacks must not get an interval from the other runs.
    #[test]
    fn a_metric_that_some_runs_lack_is_listed_and_has_no_interval() {
        let mut results: Vec<RunResult> = (0..5).map(|i| result_with(0.1, 3 + i)).collect();
        results[2].workloads[0].stats.delay = None;
        let (metrics, partial) = aggregate(&results, 1, &RunSetOptions::new(5)).unwrap();
        assert!(metrics.iter().all(|m| !m.name.contains(".delay.")));
        let p99 = partial.iter().find(|p| p.name == "w.delay.p99").unwrap();
        assert_eq!((p99.present_in, p99.of), (4, 5));
        // What every run has is still there
        assert!(metrics.iter().any(|m| m.name == "w.loss_ratio"));
    }

    #[test]
    fn the_same_inputs_give_the_same_summary_and_another_set_seed_another_interval() {
        // Twelve irregular values: the draws have many different means, so the ends of two bootstraps differ in the noise
        let losses = [
            0.02, 0.05, 0.11, 0.06, 0.31, 0.08, 0.04, 0.17, 0.09, 0.03, 0.22, 0.07,
        ];
        let results: Vec<RunResult> = losses.iter().map(|loss| result_with(*loss, 4)).collect();
        let options = RunSetOptions::new(12);
        let first = aggregate(&results, 9, &options).unwrap();
        assert_eq!(first, aggregate(&results, 9, &options).unwrap());
        let other = aggregate(&results, 10, &options).unwrap();
        let interval = |set: &(Vec<MetricSet>, Vec<PartialMetric>)| {
            set.0
                .iter()
                .find(|m| m.name == "w.loss_ratio")
                .unwrap()
                .interval
                .clone()
        };
        assert_ne!(interval(&first), interval(&other));
    }

    /// What the runner of a test sees, and what it answers.
    struct Script {
        seen: RefCell<Vec<(u64, PathBuf)>>,
    }

    impl Script {
        fn new() -> Self {
            Self {
                seen: RefCell::new(Vec::new()),
            }
        }

        fn runner(
            &self,
            fail_at: Option<usize>,
        ) -> impl FnMut(&Scenario, &RunOptions) -> Result<RunResult, Error> + '_ {
            move |scenario, options| {
                let place = self.seen.borrow().len();
                self.seen
                    .borrow_mut()
                    .push((scenario.seed.unwrap(), options.out_dir.clone()));
                if fail_at == Some(place) {
                    return Err(Error::Run {
                        action: "x".to_owned(),
                        message: "boom".to_owned(),
                    });
                }
                #[allow(clippy::cast_precision_loss)]
                let mut result = result_with(0.01 * (place as f64 + 1.0), 4 + place as u64);
                result.seed.value = scenario.seed;
                Ok(result)
            }
        }
    }

    fn run_it(
        name: &str,
        seed: Option<u64>,
        set: &RunSetOptions,
    ) -> (Result<RunSetResult, Error>, Script, Vec<u32>) {
        let script = Script::new();
        let told = RefCell::new(Vec::new());
        let result = run_set(
            &scenario(seed),
            &options(name),
            set,
            &mut script.runner(None),
            &mut |repetition| told.borrow_mut().push(repetition.index),
        );
        let told = told.into_inner();
        (result, script, told)
    }

    #[test]
    fn every_repetition_gets_its_own_seed_and_directory() {
        let (result, script, told) = run_it("seeds", Some(1234), &RunSetOptions::new(6));
        let set = result.unwrap();
        assert_eq!(told, [0, 1, 2, 3, 4, 5]);
        let seen = script.seen.borrow();
        assert_eq!(seen.len(), 6);
        for (index, (seed, out_dir)) in seen.iter().enumerate() {
            assert_eq!(
                *seed,
                derive_repetition_seed(1234, u32::try_from(index).unwrap())
            );
            assert_eq!(
                out_dir.file_name().unwrap(),
                format!("run-{:03}", index + 1).as_str()
            );
            assert_eq!(set.runs[index].seed, *seed);
            assert_eq!(set.runs[index].directory, format!("run-{:03}", index + 1));
        }
        // Canary for the line above: the seeds really differ from run to run and from the seed of the set
        let mut seeds: Vec<u64> = seen.iter().map(|(seed, _)| *seed).collect();
        seeds.sort_unstable();
        seeds.dedup();
        assert_eq!(seeds.len(), 6);
        assert!(!seeds.contains(&1234));
        assert_eq!((set.seed.value, set.seed.drawn), (1234, false));
        assert_eq!(set.repetitions, 6);
        assert_eq!(set.confidence.method, Method::ExpandedPercentile);
        // The set is on disk and reads back as it was
        let text = std::fs::read_to_string(written_to("seeds").join(RUNSET_FILE)).unwrap();
        assert_eq!(RunSetResult::from_json(&text).unwrap(), set);
        assert!(set.metric("w.loss_ratio").is_some());
    }

    #[test]
    fn a_scenario_without_a_seed_gets_a_drawn_one_that_is_recorded() {
        let (result, script, _) = run_it("drawn", None, &RunSetOptions::new(5));
        let set = result.unwrap();
        assert!(set.seed.drawn);
        let seen = script.seen.borrow();
        assert_eq!(seen[3].0, derive_repetition_seed(set.seed.value, 3));
        // The hash is that of the scenario without its seed: the same as for a scenario that has one
        let (other, _, _) = run_it("drawn2", Some(5), &RunSetOptions::new(5));
        assert_eq!(set.scenario.hash, other.unwrap().scenario.hash);
    }

    #[test]
    fn wrong_options_are_refused_before_the_first_run() {
        for set in [
            RunSetOptions::new(4),
            RunSetOptions::new(0),
            RunSetOptions::new(MAX_REPETITIONS + 1),
            RunSetOptions {
                level: 1.0,
                ..RunSetOptions::new(5)
            },
            RunSetOptions {
                resamples: 10,
                ..RunSetOptions::new(5)
            },
        ] {
            let (result, script, told) = run_it("refused", Some(1), &set);
            assert!(matches!(result, Err(Error::Run { .. })), "{set:?}");
            assert!(
                script.seen.borrow().is_empty() && told.is_empty(),
                "{set:?}"
            );
        }
        let message = run_it("refused2", Some(1), &RunSetOptions::new(4))
            .0
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("repetitions is 4") && message.contains("from 5"),
            "{message}"
        );
        assert!(!written_to("refused2").join(RUNSET_FILE).exists());
    }

    #[test]
    fn a_failing_run_ends_the_set_with_its_error_and_keeps_no_set_file() {
        let script = Script::new();
        let mut told = 0;
        let error = run_set(
            &scenario(Some(1)),
            &options("failing"),
            &RunSetOptions::new(6),
            &mut script.runner(Some(2)),
            &mut |_| told += 1,
        )
        .unwrap_err();
        assert!(error.to_string().contains("boom"), "{error}");
        assert_eq!(
            script.seen.borrow().len(),
            3,
            "no run after the failing one"
        );
        assert_eq!(told, 3);
        assert!(!written_to("failing").join(RUNSET_FILE).exists());
    }

    #[test]
    fn a_cancel_stops_the_set_between_runs_and_a_cut_run_stops_it_too() {
        let script = Script::new();
        let mut options = options("cancel");
        let token = crate::CancelToken::new();
        options.cancel = token.clone();
        let mut runner = script.runner(None);
        let mut count = 0;
        let error = run_set(
            &scenario(Some(1)),
            &options,
            &RunSetOptions::new(6),
            &mut |scenario, run_options| {
                count += 1;
                if count == 2 {
                    token.cancel();
                }
                runner(scenario, run_options)
            },
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(error, Error::Cancelled), "{error}");
        assert_eq!(
            script.seen.borrow().len(),
            2,
            "no third run after the cancel"
        );
        // A result that says it was cut short ends the set with Cancelled as well
        let cut = run_set(
            &scenario(Some(1)),
            &self::options("cut"),
            &RunSetOptions::new(5),
            &mut |_, _| {
                let mut result = result_with(0.1, 3);
                result.timing.interrupted_ns = Some(5);
                Ok(result)
            },
            &mut |_| {},
        );
        assert!(matches!(cut, Err(Error::Cancelled)));
    }

    /// The set that the Python tests read (`python/netscenario/tests/fixtures/runset.json`) is what the set writes today, so that
    /// a change of the format fails here and not in the field. The file is rewritten with `UPDATE_FIXTURE=1`. The probability
    /// left out at each end (`tail`) is compared to 9 digits: it is made with `exp` and `ln` of the maths library of the machine.
    #[test]
    fn the_fixture_that_python_reads_is_what_the_set_writes() {
        fn rounded(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(map) => {
                    for (key, inner) in map.iter_mut() {
                        if key == "tail" {
                            if let Some(tail) = inner.as_f64() {
                                *inner = serde_json::json!((tail * 1e9).round() / 1e9);
                            }
                        } else {
                            rounded(inner);
                        }
                    }
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(rounded),
                _ => {}
            }
        }
        let mut place = 0_usize;
        let set = run_set(
            &scenario(Some(7)),
            &options("fixture"),
            &RunSetOptions::new(6),
            &mut |scenario, _| {
                place += 1;
                let steps = u32::try_from(place).unwrap();
                let mut result = result_with(0.02 * f64::from(steps), 3 + u64::from(steps));
                result.seed.value = scenario.seed;
                // The delay is missing in the third run: its metrics are listed as partial
                if place == 3 {
                    result.workloads[0].stats.delay = None;
                }
                Ok(result)
            },
            &mut |_| {},
        )
        .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../python/netscenario/tests/fixtures/runset.json");
        if std::env::var_os("UPDATE_FIXTURE").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, set.to_json()).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "{}: {error} (write it with UPDATE_FIXTURE=1)",
                path.display()
            )
        });
        let (mut written, mut wanted): (serde_json::Value, serde_json::Value) = (
            serde_json::from_str(&text).unwrap(),
            serde_json::from_str(&set.to_json()).unwrap(),
        );
        rounded(&mut written);
        rounded(&mut wanted);
        assert_eq!(
            written, wanted,
            "run `UPDATE_FIXTURE=1 cargo test -p netscenario-core --lib runset` and read the diff"
        );
    }

    #[test]
    fn the_format_is_checked_when_a_set_is_read() {
        let (result, ..) = run_it("format", Some(1), &RunSetOptions::new(5));
        let text = result.unwrap().to_json();
        assert!(RunSetResult::from_json(&text).is_ok());
        assert!(RunSetResult::from_json(&text.replace("netscenario-runset", "other")).is_err());
        assert!(
            RunSetResult::from_json(&text.replace("\"version\": 0", "\"version\": 7")).is_err()
        );
        assert!(RunSetResult::from_json("{}").is_err());
    }
}
