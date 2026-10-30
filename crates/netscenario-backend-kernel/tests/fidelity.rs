//! Fidelity of the Kernel backend (QA-06): does the network do what the scenario says, and by how much does
//! it deviate? Every case runs a scenario with a known answer in real namespaces, measures it with the agent,
//! and judges the measurement against bounds that are written down here with the reason for each.
//!
//! How the bounds are chosen, in the order of strength:
//! - **Physics never moves.** A delay cannot be shorter than the link makes it; the lower bound is exact.
//! - **Noise only goes one way.** A busy shared machine makes packets later, not earlier, so the upper bounds
//!   of delays leave room (0.5 ms for a median) and are the place where a runner may differ from a laptop.
//! - **Statistics get four standard deviations.** For a count (lost packets) or a mean over thousands of
//!   packets, a fair run is outside four standard deviations with a probability of about 6 in 100,000.
//! - **Known mechanisms are named.** Where the backend does something a reader might not expect (the rate
//!   counts Ethernet frames, jitter reorders, jitter with a rate raises the mean), the case states the number
//!   that the mechanism predicts and the bound is around that, with the explanation in `docs/FIDELITY.md`.
//!
//! Each case writes a part of the report to `target/tmp/fidelity/`; with `UPDATE_FIDELITY=1` (`just fidelity`)
//! they are put together into `docs/FIDELITY.md`. The canaries run scenarios that are wrong on purpose
//! (a rate, a delay, a loss that is not applied) and prove that the judging fails for them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Counts of packets and delays of a few milliseconds are far below the precision of a double
    clippy::cast_precision_loss
)]

mod support;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use netscenario_agent::records;
use netscenario_core::{RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, backend, namespaces_available, work_dir};

/// A number that a case measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    MeanDelayMs,
    MedianDelayMs,
    MinDelayMs,
    P99DelayMs,
    StdevDelayMs,
    LossRatio,
    ReorderedRatio,
    SteadyGoodputBps,
}

impl Metric {
    fn label(self) -> &'static str {
        match self {
            Metric::MeanDelayMs => "mean delay",
            Metric::MedianDelayMs => "median delay",
            Metric::MinDelayMs => "smallest delay",
            Metric::P99DelayMs => "99th percentile of the delay",
            Metric::StdevDelayMs => "standard deviation of the delay",
            Metric::LossRatio => "loss",
            Metric::ReorderedRatio => "reordered packets",
            Metric::SteadyGoodputBps => "steady payload rate",
        }
    }

    fn show(self, value: f64) -> String {
        match self {
            Metric::LossRatio | Metric::ReorderedRatio => format!("{:.2} %", value * 100.0),
            Metric::SteadyGoodputBps => format!("{:.4} mbit/s", value / 1e6),
            _ => format!("{value:.3} ms"),
        }
    }
}

/// What a case expects of one number: the value that the configuration (or a named mechanism) predicts, the
/// bounds around it, and why the bounds are where they are.
struct Bound {
    metric: Metric,
    expected: f64,
    lo: f64,
    hi: f64,
    reason: &'static str,
}

/// What a run measured.
#[derive(Debug)]
struct Measurement {
    delays_ms: Vec<f64>,
    loss_ratio: f64,
    reordered_ratio: f64,
    steady_goodput_bps: f64,
    sent: u64,
    kernel: String,
    tc: String,
}

impl Measurement {
    fn value(&self, metric: Metric) -> f64 {
        let n = self.delays_ms.len() as f64;
        let mean = self.delays_ms.iter().sum::<f64>() / n;
        match metric {
            Metric::MeanDelayMs => mean,
            Metric::MedianDelayMs => self.delays_ms[self.delays_ms.len() / 2],
            Metric::MinDelayMs => self.delays_ms[0],
            // Nearest rank, as in the results (docs/RESULTS.md)
            Metric::P99DelayMs => self.delays_ms[(self.delays_ms.len() * 99).div_ceil(100) - 1],
            Metric::StdevDelayMs => (self
                .delays_ms
                .iter()
                .map(|d| (d - mean).powi(2))
                .sum::<f64>()
                / n)
                .sqrt(),
            Metric::LossRatio => self.loss_ratio,
            Metric::ReorderedRatio => self.reordered_ratio,
            Metric::SteadyGoodputBps => self.steady_goodput_bps,
        }
    }
}

/// The answer of the judging for one bound.
struct Verdict<'a> {
    bound: &'a Bound,
    value: f64,
}

impl Verdict<'_> {
    fn passed(&self) -> bool {
        (self.bound.lo..=self.bound.hi).contains(&self.value)
    }
}

/// Judges a measurement against the bounds of a case. It is the one place that decides, for the cases and for
/// the canaries.
fn judge<'a>(bounds: &'a [Bound], measurement: &Measurement) -> Vec<Verdict<'a>> {
    bounds
        .iter()
        .map(|bound| Verdict {
            bound,
            value: measurement.value(bound.metric),
        })
        .collect()
}

/// A scenario with a known answer.
struct Case {
    id: &'static str,
    title: &'static str,
    /// The impairments of the forward direction, as in the file.
    link: &'static str,
    /// The rate of the payload that is sent, and the size of a payload in bytes.
    offered: &'static str,
    size: u64,
    seconds: u64,
    settle_seconds: u64,
    bounds: Vec<Bound>,
}

impl Case {
    fn scenario(&self) -> Scenario {
        let link = if self.link.is_empty() {
            String::new()
        } else {
            format!("      forward: {{ {} }}\n", self.link)
        };
        Scenario::from_yaml(
            &format!(
                "version: 0\nname: fidelity-{}\nseed: 7\nduration: {}s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {{ id: media, kind: udp-cbr, from: client, to: server, rate: {}, packet-size: {}b }}\n",
                self.id, self.seconds, self.offered, self.size
            ),
            "fidelity.yaml",
        )
        .unwrap()
    }
}

// A table of data: one entry for each case, each with its bounds and their reasons
#[allow(clippy::too_many_lines)]
/// The cases. The numbers in `expected` come from the configuration or from the named mechanism; the bounds
/// and their reasons are the judgement.
fn cases() -> Vec<Case> {
    vec![
        Case {
            id: "1-delay",
            title: "A fixed delay of 20 ms",
            link: "delay: 20ms",
            offered: "2mbit",
            size: 1200,
            seconds: 6,
            settle_seconds: 1,
            bounds: vec![
                Bound {
                    metric: Metric::MinDelayMs,
                    expected: 20.0,
                    lo: 20.0,
                    hi: 20.5,
                    reason: "no packet can be faster than the link; the path adds 0.016 ms (spike S4)",
                },
                Bound {
                    metric: Metric::MedianDelayMs,
                    expected: 20.0,
                    lo: 20.0,
                    hi: 20.5,
                    reason: "`netem` is exact to the microsecond; 0.5 ms leave room for a busy shared runner",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a link without loss loses nothing",
                },
                Bound {
                    metric: Metric::ReorderedRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a fixed delay keeps the order",
                },
            ],
        },
        Case {
            id: "2-loss",
            title: "A random loss of 5 %",
            link: "loss: { model: random, p: 5% }",
            offered: "8mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 1,
            bounds: vec![
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.05,
                    lo: 0.0387,
                    hi: 0.0613,
                    reason: "binomial: 6,000 packets, a standard deviation of 0.28 percentage points, four of them on each side",
                },
                Bound {
                    metric: Metric::MedianDelayMs,
                    expected: 0.016,
                    lo: 0.0,
                    hi: 0.5,
                    reason: "a link without a delay adds only the path (0.016 ms); 0.5 ms for noise",
                },
                Bound {
                    metric: Metric::ReorderedRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "loss does not reorder",
                },
            ],
        },
        Case {
            id: "3-rate",
            title: "A rate limit of 2 mbit, offered 3 mbit",
            link: "rate: 2mbit",
            offered: "3mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 5,
            bounds: vec![
                // 2 mbit of frames; a frame is the payload and 42 bytes (8 UDP + 20 IP + 14 Ethernet)
                Bound {
                    metric: Metric::SteadyGoodputBps,
                    expected: 2_000_000.0 * 1000.0 / 1042.0,
                    lo: 1_881_000.0,
                    hi: 1_958_000.0,
                    reason: "`netem rate` counts the Ethernet frame, so 1,000 bytes of payload use 1,042; 2 % for the edges of the one-second bins (about 240 packets each)",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "the backlog (about 800 packets) stays below the queue of 1,000, so a rate limit alone loses nothing here",
                },
                Bound {
                    metric: Metric::ReorderedRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a rate limit keeps the order",
                },
            ],
        },
        Case {
            id: "4-jitter-uniform",
            title: "A delay of 20 ms with a uniform jitter of 5 ms",
            link: "delay: 20ms, jitter: { amount: 5ms }",
            offered: "8mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 1,
            bounds: vec![
                Bound {
                    metric: Metric::MinDelayMs,
                    expected: 15.0,
                    lo: 15.0,
                    hi: 15.5,
                    reason: "uniform jitter reaches down to the delay minus the amount and no further",
                },
                Bound {
                    metric: Metric::P99DelayMs,
                    expected: 24.9,
                    lo: 24.5,
                    hi: 25.5,
                    reason: "the upper end of a uniform spread, 15 + 0.99 * 10 ms. The largest delay is not used: one packet disturbed by the machine would fail it, 60 are needed to fail the 99th percentile",
                },
                Bound {
                    metric: Metric::MeanDelayMs,
                    expected: 20.0,
                    lo: 19.85,
                    hi: 20.5,
                    reason: "the mean of 6,000 uniform delays has a standard deviation of 0.04 ms; four of them below, noise above",
                },
                Bound {
                    metric: Metric::StdevDelayMs,
                    expected: 2.887,
                    lo: 2.74,
                    hi: 3.04,
                    reason: "5 / sqrt(3) for a uniform distribution of width 10 ms; the estimate has a standard deviation of 0.02 ms, the bound is wider because the measurement adds noise",
                },
                Bound {
                    metric: Metric::ReorderedRatio,
                    expected: 0.58,
                    lo: 0.3,
                    hi: 0.8,
                    reason: "KNOWN: jitter larger than the gap between packets (1 ms here) reorders them, because `netem` sends each packet at its own time; 58 % were seen",
                },
            ],
        },
        Case {
            id: "5-jitter-normal",
            title: "A delay of 20 ms with a normal jitter of 5 ms",
            link: "delay: 20ms, jitter: { amount: 5ms, distribution: normal }",
            offered: "8mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 1,
            bounds: vec![
                Bound {
                    metric: Metric::MeanDelayMs,
                    expected: 20.0,
                    lo: 19.7,
                    hi: 20.5,
                    reason: "the mean of 6,000 normal delays has a standard deviation of 0.07 ms; four of them below, noise above",
                },
                Bound {
                    metric: Metric::StdevDelayMs,
                    expected: 5.0,
                    lo: 4.7,
                    hi: 5.3,
                    reason: "the amount is the standard deviation (spike S4 measured 5.03); the table of `netem` is cut off at four standard deviations",
                },
                Bound {
                    metric: Metric::P99DelayMs,
                    expected: 31.6,
                    lo: 30.4,
                    hi: 32.6,
                    reason: "the mean plus 2.326 standard deviations; the estimate has a standard deviation of 0.24 ms, four of them on each side, with noise on the upper side",
                },
                Bound {
                    metric: Metric::ReorderedRatio,
                    expected: 0.72,
                    lo: 0.4,
                    hi: 0.9,
                    reason: "KNOWN: as with the uniform jitter; 72 % were seen",
                },
            ],
        },
        Case {
            id: "6-jitter-and-rate",
            title: "A delay of 40 ms with a normal jitter of 5 ms and a rate limit of 10 mbit",
            link: "delay: 40ms, jitter: { amount: 5ms, distribution: normal }, rate: 10mbit",
            offered: "2mbit",
            size: 1200,
            seconds: 8,
            settle_seconds: 1,
            bounds: vec![
                // The rate stage keeps the order: a packet leaves at the later of its own moment and the moment of
                // its predecessor, plus its serialisation (0.994 ms for a frame of 1,242 bytes). A model of that
                // gives a mean of 42.6 ms for these numbers.
                Bound {
                    metric: Metric::MeanDelayMs,
                    expected: 42.6,
                    lo: 41.8,
                    hi: 43.6,
                    reason: "KNOWN: not 40 + 1.0 ms. Because the rate stage forces the order, delays can only grow: a model of `sch_netem` gives 42.6 ms (see docs/FIDELITY.md)",
                },
                Bound {
                    metric: Metric::ReorderedRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "KNOWN: with a rate limit the packets stay in order, jitter or not",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "the offered 2 mbit is far below the 10 mbit of the link",
                },
            ],
        },
    ]
}

/// Runs the scenario of a case and measures it.
fn measure(case: &Case) -> Measurement {
    let dir = work_dir(&format!("fid-{}", case.id));
    let mut options = RunOptions::new(agent_exe().unwrap(), &dir);
    options.settle = Some(Duration::from_secs(case.settle_seconds));
    let mut backend = backend(|_| {});
    let result = run(&mut backend, &case.scenario(), &options).unwrap();
    let measurement = measurement_of(&result, &dir);
    let _ = std::fs::remove_dir_all(dir);
    measurement
}

fn measurement_of(result: &RunResult, dir: &Path) -> Measurement {
    let workload = &result.workloads[0];
    let file =
        records::read(std::fs::File::open(dir.join(&workload.records_file)).unwrap()).unwrap();
    let mut delays_ms: Vec<f64> = file
        .records
        .iter()
        .map(|r| r.delay_ns() as f64 / 1e6)
        .collect();
    delays_ms.sort_by(f64::total_cmp);
    // The bins from the second to the one before the last: the first holds the start, the last is not full
    let bins = &workload.stats.throughput.bits_per_second;
    let mut inner: Vec<f64> = bins
        .get(1..bins.len().saturating_sub(1))
        .unwrap_or(&[])
        .to_vec();
    inner.sort_by(f64::total_cmp);
    let steady = inner.get(inner.len() / 2).copied().unwrap_or(0.0);
    Measurement {
        delays_ms,
        loss_ratio: workload.stats.loss_ratio,
        reordered_ratio: workload.stats.reordered as f64 / workload.stats.received.max(1) as f64,
        steady_goodput_bps: steady,
        sent: workload.stats.sent,
        kernel: result.provenance.kernel.clone().unwrap_or_default(),
        tc: result
            .provenance
            .backend_details
            .get("tc")
            .cloned()
            .unwrap_or_default(),
    }
}

/// The part of the report for one case.
fn fragment(case: &Case, measurement: &Measurement, verdicts: &[Verdict<'_>]) -> String {
    let mut text = format!(
        "#### {}\n\n`forward: {{ {} }}`, offered {} with payloads of {} bytes for {} s ({} packets)\n\n| Quantity | Expected | Measured | Deviation | Allowed | Why |\n| --- | --- | --- | --- | --- | --- |\n",
        case.title,
        if case.link.is_empty() {
            "none"
        } else {
            case.link
        },
        case.offered,
        case.size,
        case.seconds,
        measurement.sent
    );
    for verdict in verdicts {
        let bound = verdict.bound;
        let metric = bound.metric;
        let deviation = match metric {
            Metric::LossRatio | Metric::ReorderedRatio => {
                format!("{:+.2} pp", (verdict.value - bound.expected) * 100.0)
            }
            Metric::SteadyGoodputBps => {
                format!("{:+.2} %", (verdict.value / bound.expected - 1.0) * 100.0)
            }
            _ => format!("{:+.3} ms", verdict.value - bound.expected),
        };
        let _ = writeln!(
            text,
            "| {} | {} | {} {} | {} | {} to {} | {} |",
            metric.label(),
            metric.show(bound.expected),
            metric.show(verdict.value),
            if verdict.passed() { "✔" } else { "✘" },
            deviation,
            metric.show(bound.lo),
            metric.show(bound.hi),
            bound.reason
        );
    }
    text.push('\n');
    text
}

fn report_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("fidelity")
}

/// Writes the part of a case, and the description of the machine, for the report.
fn write_report(case: &Case, measurement: &Measurement, verdicts: &[Verdict<'_>]) {
    let dir = report_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("0-machine.md"),
        format!(
            "Measured on kernel {}, iproute2 `tc`: {}.\n\n",
            measurement.kernel, measurement.tc
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("{}.md", case.id)),
        fragment(case, measurement, verdicts),
    )
    .unwrap();
    if std::env::var_os("UPDATE_FIDELITY").is_some() {
        update_document(&dir);
    }
}

/// Puts the parts together into the block of `docs/FIDELITY.md` between the markers. The file is replaced in
/// one step, so that tests that finish together do not leave half of it.
fn update_document(dir: &Path) {
    let mut parts: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    parts.sort();
    let generated: String = parts
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    let document = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/FIDELITY.md");
    let text = std::fs::read_to_string(&document).unwrap();
    let (Some(begin), Some(end)) = (
        text.find("<!-- fidelity:begin -->"),
        text.find("<!-- fidelity:end -->"),
    ) else {
        panic!("docs/FIDELITY.md has no markers for the generated block");
    };
    let new = format!(
        "{}<!-- fidelity:begin -->\n{}{}",
        &text[..begin],
        generated,
        &text[end..]
    );
    let temporary = document.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temporary, new).unwrap();
    std::fs::rename(temporary, document).unwrap();
}

fn run_and_judge(id: &str) {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let case = cases().into_iter().find(|c| c.id == id).unwrap();
    let measurement = measure(&case);
    let verdicts = judge(&case.bounds, &measurement);
    write_report(&case, &measurement, &verdicts);
    let failed: Vec<String> = verdicts
        .iter()
        .filter(|v| !v.passed())
        .map(|v| {
            format!(
                "{}: {} is outside {} to {}",
                v.bound.metric.label(),
                v.bound.metric.show(v.value),
                v.bound.metric.show(v.bound.lo),
                v.bound.metric.show(v.bound.hi)
            )
        })
        .collect();
    assert!(
        failed.is_empty(),
        "{}\n{}\n{}",
        case.title,
        failed.join("\n"),
        fragment(&case, &measurement, &verdicts)
    );
}

#[test]
fn a_fixed_delay_is_exact() {
    run_and_judge("1-delay");
}

#[test]
fn a_random_loss_is_the_configured_ratio() {
    run_and_judge("2-loss");
}

#[test]
fn a_rate_limit_counts_ethernet_frames() {
    run_and_judge("3-rate");
}

#[test]
fn a_uniform_jitter_spreads_the_delay_evenly_and_reorders() {
    run_and_judge("4-jitter-uniform");
}

#[test]
fn a_normal_jitter_has_the_configured_deviation_and_reorders() {
    run_and_judge("5-jitter-normal");
}

#[test]
fn jitter_with_a_rate_limit_raises_the_mean_and_keeps_the_order() {
    run_and_judge("6-jitter-and-rate");
}

// ---- the model behind the case of a jitter with a rate limit ----

/// A model of `sch_netem` with a delay, a normal jitter and a rate limit: a packet leaves at the later of its
/// own moment and the moment of its predecessor, plus its serialisation, while the predecessor is still
/// queued (`netem_enqueue` in `net/sched/sch_netem.c`). Returns the mean delay in milliseconds over `n` packets.
/// The numbers are drawn from a small generator with a fixed seed, so the answer is always the same.
fn model_mean_delay_ms(
    n: usize,
    interval_ms: f64,
    mean_ms: f64,
    sigma_ms: f64,
    serialisation_ms: f64,
) -> f64 {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut uniform = move || {
        // xorshift64*, mapped to (0, 1)
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        ((state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1_u64 << 53) as f64
    };
    let mut last_leaves: Option<f64> = None;
    let mut total = 0.0;
    for i in 0..n {
        let arrives = i as f64 * interval_ms;
        // Box-Muller; the table of netem is cut off at four standard deviations
        let normal =
            (-2.0 * uniform().ln()).sqrt() * (2.0 * std::f64::consts::PI * uniform()).cos();
        let drawn = (mean_ms + sigma_ms * normal.clamp(-4.0, 4.0)).max(0.0);
        let leaves = match last_leaves {
            Some(before) if before > arrives => before.max(arrives + drawn) + serialisation_ms,
            _ => arrives + drawn + serialisation_ms,
        };
        last_leaves = Some(leaves);
        total += leaves - arrives;
    }
    total / n as f64
}

/// The 42.6 ms of the case are not a guess: the model gives them, and without the rate stage it gives the
/// configured 40 ms.
#[test]
fn the_model_of_netem_gives_the_mean_of_a_jitter_with_a_rate_limit() {
    // 1,200 bytes of payload and 42 of headers at 10 mbit take 0.9936 ms; a packet every 4.8 ms
    let with_rate = model_mean_delay_ms(200_000, 4.8, 40.0, 5.0, 0.9936);
    assert!((42.5..42.7).contains(&with_rate), "{with_rate}");
    // Canary: the model can say something else. Without the rate stage the jitter alone leaves the mean at 40
    let spaced_out = model_mean_delay_ms(200_000, 400.0, 40.0, 5.0, 0.0);
    assert!((39.95..40.05).contains(&spaced_out), "{spaced_out}");
}

// ---- the judging itself ----

fn synthetic(delays_ms: &[f64], loss: f64, reordered: f64, goodput: f64) -> Measurement {
    let mut delays_ms = delays_ms.to_vec();
    delays_ms.sort_by(f64::total_cmp);
    Measurement {
        delays_ms,
        loss_ratio: loss,
        reordered_ratio: reordered,
        steady_goodput_bps: goodput,
        sent: 0,
        kernel: String::new(),
        tc: String::new(),
    }
}

fn failing_metrics(case: &str, measurement: &Measurement) -> Vec<Metric> {
    let case = cases().into_iter().find(|c| c.id == case).unwrap();
    judge(&case.bounds, measurement)
        .iter()
        .filter(|v| !v.passed())
        .map(|v| v.bound.metric)
        .collect()
}

#[test]
fn the_judging_accepts_a_faithful_measurement_and_names_what_is_wrong_with_others() {
    // A faithful delay, and one that is too short, too long, lossy and reordered
    assert_eq!(
        failing_metrics("1-delay", &synthetic(&[20.01, 20.02, 20.03], 0.0, 0.0, 0.0)),
        []
    );
    assert_eq!(
        failing_metrics("1-delay", &synthetic(&[19.9, 20.02, 20.03], 0.0, 0.0, 0.0)),
        [Metric::MinDelayMs]
    );
    assert_eq!(
        failing_metrics("1-delay", &synthetic(&[21.0, 21.0, 21.0], 0.01, 0.1, 0.0)),
        [
            Metric::MinDelayMs,
            Metric::MedianDelayMs,
            Metric::LossRatio,
            Metric::ReorderedRatio
        ]
    );
    // A loss on either side of the band fails
    for loss in [0.03, 0.07] {
        assert!(
            failing_metrics("2-loss", &synthetic(&[0.02], loss, 0.0, 0.0))
                .contains(&Metric::LossRatio)
        );
    }
    assert_eq!(
        failing_metrics("2-loss", &synthetic(&[0.02], 0.049, 0.0, 0.0)),
        []
    );
    // The rate: the configured 2 mbit as payload (no frame overhead counted) is outside the band
    assert_eq!(
        failing_metrics("3-rate", &synthetic(&[1.0], 0.0, 0.0, 2_000_000.0)),
        [Metric::SteadyGoodputBps]
    );
    assert_eq!(
        failing_metrics("3-rate", &synthetic(&[1.0], 0.0, 0.0, 1_919_000.0)),
        []
    );
}

#[test]
fn every_bound_has_a_reason_and_an_expectation_inside_it() {
    for case in cases() {
        assert!(!case.bounds.is_empty(), "{}", case.id);
        for bound in &case.bounds {
            assert!(!bound.reason.is_empty(), "{}", case.id);
            assert!(
                bound.lo <= bound.hi,
                "{}: {}",
                case.id,
                bound.metric.label()
            );
            // A case expects what its bounds allow: otherwise the faithful network itself would fail
            assert!(
                (bound.lo..=bound.hi).contains(&bound.expected),
                "{}: the expected {} of '{}' is outside its bounds",
                case.id,
                bound.expected,
                bound.metric.label()
            );
        }
    }
}

// ---- canaries: networks that are wrong on purpose must fail ----

/// Runs a case with something taken away and judges it with the bounds of the real case.
fn wrong_on_purpose(
    id: &str,
    link: &'static str,
    seconds: u64,
    settle_seconds: u64,
) -> Vec<Metric> {
    let real = cases().into_iter().find(|c| c.id == id).unwrap();
    let wrong = Case {
        link,
        seconds,
        settle_seconds,
        bounds: Vec::new(),
        ..cases().into_iter().find(|c| c.id == id).unwrap()
    };
    let measurement = measure(&wrong);
    judge(&real.bounds, &measurement)
        .iter()
        .filter(|v| !v.passed())
        .map(|v| v.bound.metric)
        .collect()
}

/// A rate that is not applied: the link carries all of the offered 3 mbit, and the case for 2 mbit says so.
#[test]
fn canary_a_rate_that_is_not_applied_fails_the_rate_case() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let failed = wrong_on_purpose("3-rate", "", 4, 1);
    assert!(failed.contains(&Metric::SteadyGoodputBps), "{failed:?}");
}

/// A delay that is not applied.
#[test]
fn canary_a_delay_that_is_not_applied_fails_the_delay_case() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let failed = wrong_on_purpose("1-delay", "", 2, 1);
    assert!(
        failed.contains(&Metric::MinDelayMs) && failed.contains(&Metric::MedianDelayMs),
        "{failed:?}"
    );
}

/// A loss that is not applied, and one that is applied at the wrong ratio.
#[test]
fn canary_a_loss_that_is_not_applied_or_is_wrong_fails_the_loss_case() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    assert!(wrong_on_purpose("2-loss", "", 2, 1).contains(&Metric::LossRatio));
    let wrong = wrong_on_purpose("2-loss", "loss: { model: random, p: 10% }", 3, 1);
    assert!(wrong.contains(&Metric::LossRatio), "{wrong:?}");
}
