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
    /// Packets that arrived twice, as a share of the packets sent.
    DuplicateRatio,
    /// The share of the packets that arrived within half of the configured delay: those that `netem`
    /// chose to send on at once.
    EarlyShare,
    /// The mean length of a run of lost packets with consecutive numbers.
    MeanLossRun,
    /// The frames that arrived and that the receiving node dropped as damaged, as a share of the frames that
    /// arrived (the tally of the direction, D-040).
    NodeDropRatio,
    /// The packets that the queue of the link dropped, as a share of the packets sent (the same tally).
    LostInQueueRatio,
    /// The mean of the interarrival jitter of RFC 3550 over the run, in milliseconds.
    JitterMeanMs,
    /// The mean extent of the reordered packets (RFC 4737), in packets.
    ExtentMean,
    /// The mean late time of the reordered packets (RFC 4737), in milliseconds.
    LateTimeMeanMs,
    /// The damaged packets that the result names by sequence number, as a share of the frames the node dropped
    /// as damaged: what the tap in the receiving node could read.
    NamedShare,
    /// The names beyond the frames the node dropped (a name that no dropped frame stands behind): 0 on a link
    /// that only corrupts.
    NamedExcess,
    /// The packets that arrived damaged and were found by the CRC-32, as a share of the packets sent: only a
    /// stream without the checksum of UDP has them.
    DamagedRatio,
    /// The packets that are neither lost in the queue, nor dropped by the node, nor delivered: the tally of
    /// the direction must add up to what was sent.
    Unaccounted,
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
            Metric::DuplicateRatio => "packets that arrived twice",
            Metric::EarlyShare => "packets sent on at once",
            Metric::MeanLossRun => "mean run of lost packets",
            Metric::NodeDropRatio => "frames dropped by the node as damaged",
            Metric::LostInQueueRatio => "packets lost in the queue",
            Metric::DamagedRatio => "packets that arrived damaged",
            Metric::NamedShare => "dropped packets named by sequence number",
            Metric::JitterMeanMs => "interarrival jitter (mean of RFC 3550)",
            Metric::ExtentMean => "mean extent of the reordered packets",
            Metric::LateTimeMeanMs => "mean late time of the reordered packets",
            Metric::NamedExcess => "names without a dropped frame",
            Metric::Unaccounted => "packets not accounted for",
        }
    }

    fn show(self, value: f64) -> String {
        match self {
            Metric::LossRatio
            | Metric::ReorderedRatio
            | Metric::DuplicateRatio
            | Metric::NodeDropRatio
            | Metric::LostInQueueRatio
            | Metric::DamagedRatio
            | Metric::NamedShare
            | Metric::EarlyShare => format!("{:.2} %", value * 100.0),
            Metric::Unaccounted | Metric::NamedExcess => format!("{value:.0} packets"),
            Metric::MeanLossRun | Metric::ExtentMean => format!("{value:.2} packets"),
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
    duplicate_ratio: f64,
    early_share: f64,
    mean_loss_run: f64,
    node_drop_ratio: f64,
    lost_in_queue_ratio: f64,
    damaged_ratio: f64,
    named_share: f64,
    named_excess: f64,
    jitter_mean_ms: f64,
    extent_mean: f64,
    late_time_mean_ms: f64,
    unaccounted: f64,
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
            Metric::DuplicateRatio => self.duplicate_ratio,
            Metric::EarlyShare => self.early_share,
            Metric::MeanLossRun => self.mean_loss_run,
            Metric::NodeDropRatio => self.node_drop_ratio,
            Metric::LostInQueueRatio => self.lost_in_queue_ratio,
            Metric::DamagedRatio => self.damaged_ratio,
            Metric::NamedShare => self.named_share,
            Metric::NamedExcess => self.named_excess,
            Metric::JitterMeanMs => self.jitter_mean_ms,
            Metric::ExtentMean => self.extent_mean,
            Metric::LateTimeMeanMs => self.late_time_mean_ms,
            Metric::Unaccounted => self.unaccounted,
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
    /// Whether the stream carries the checksum of UDP (`udp-checksum` in the file).
    udp_checksum: bool,
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
                "version: 0\nname: fidelity-{}\nseed: 7\nduration: {}s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n  - {{ id: media, kind: udp-cbr, from: client, to: server, rate: {}, packet-size: {}b{} }}\n",
                self.id,
                self.seconds,
                self.offered,
                self.size,
                if self.udp_checksum {
                    ""
                } else {
                    ", udp-checksum: false"
                }
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
            udp_checksum: true,
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
            udp_checksum: true,
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
            udp_checksum: true,
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
            udp_checksum: true,
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
                Bound {
                    metric: Metric::JitterMeanMs,
                    expected: 3.543,
                    lo: 3.39,
                    hi: 3.69,
                    reason: "MODEL (RFC 3550, appendix A.8): the delays are independent and uniform over 10 ms, a packet every millisecond, and the estimate pairs packets in the order of arrival, so the pair is not independent of its delays (the mean of |D| for neighbours in sequence would be 10 / 3 = 3.33 ms); a simulation of 120 runs of 6,000 packets, written apart from the code under test, gave 3.543 ms with a standard deviation of 0.032 ms between runs; four of them on each side, and 0.02 ms for the machine",
                },
                Bound {
                    metric: Metric::ExtentMean,
                    expected: 3.70,
                    lo: 3.57,
                    hi: 3.83,
                    reason: "MODEL (RFC 4737, section 4.2): the same simulation, the mean over the reordered packets of the number of arrivals since the first packet that overtook them: 3.70 with a standard deviation of 0.032 between runs, four of them on each side",
                },
                Bound {
                    metric: Metric::LateTimeMeanMs,
                    expected: 3.185,
                    lo: 3.05,
                    hi: 3.32,
                    reason: "MODEL (RFC 4737, section 4.3): the same simulation, the mean time between a reordered packet and the first packet that overtook it: 3.185 ms with a standard deviation of 0.032 ms between runs, four of them on each side, and 0.01 ms for the machine",
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
            udp_checksum: true,
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
            udp_checksum: true,
            bounds: vec![
                // The rate first, then the delay with its jitter: the serialisation of a frame of 1,242 bytes at
                // 10 mbit is 0.994 ms, and the jitter is the configured one
                Bound {
                    metric: Metric::MeanDelayMs,
                    expected: 41.0,
                    lo: 40.5,
                    hi: 41.8,
                    reason: "40 ms plus the 0.994 ms that a frame needs at 10 mbit. The mean of 1,667 delays has a standard deviation of 0.12 ms. One qdisc for both gave 42.6 ms, because its rate stage cuts the jitter off (D-030)",
                },
                Bound {
                    metric: Metric::StdevDelayMs,
                    expected: 5.0,
                    lo: 4.65,
                    hi: 5.35,
                    reason: "the amount is the standard deviation; the estimate over 1,667 delays has a standard deviation of 0.09 ms. One qdisc for both gave 4.1 ms",
                },
                Bound {
                    metric: Metric::ReorderedRatio,
                    expected: 0.29,
                    lo: 0.15,
                    hi: 0.40,
                    reason: "as for any jitter, packets overtake each other when the jitter is larger than the gap (4.8 ms here): the model of the series of the two stages gives 29 %",
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
        Case {
            id: "7-burst-gilbert-elliott",
            title: "Loss in bursts: Gilbert-Elliott, 1 % to the bad state, 20 % back, all lost there",
            link: "loss: { model: gilbert-elliott, p: 1%, r: 20%, loss-bad: 100%, loss-good: 0% }",
            offered: "4mbit",
            size: 200,
            seconds: 12,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.04762,
                    lo: 0.0352,
                    hi: 0.0600,
                    reason: "the share of time in the bad state, p / (p + r) = 1 / 21 (S12: 4.61 % at 100,000 packets); the losses come in runs, so the spread is wider than a binomial: 30,000 packets, a standard deviation of 0.31 percentage points (S12 saw 0.17 at 100,000), four of them on each side",
                },
                Bound {
                    metric: Metric::MeanLossRun,
                    expected: 5.0,
                    lo: 3.94,
                    hi: 6.06,
                    reason: "all packets in the bad state are lost, so a run is a stay in it: 1 / r = 5 packets, about 290 runs, a standard deviation of the mean of 0.26, four of them on each side (S12: 4.99)",
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
            id: "8-burst-four-state",
            title: "Loss in bursts: the four-state model, 2 % 30 % 20 % 10 % 1 %",
            link: "loss: { model: markov4, p13: 2%, p31: 30%, p32: 20%, p23: 10%, p14: 1% }",
            offered: "4mbit",
            size: 200,
            seconds: 12,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.06336,
                    lo: 0.0463,
                    hi: 0.0804,
                    reason: "the stationary loss of the chain of four states (S12: 6.37 % at 100,000 packets); 30,000 packets, a standard deviation of 0.42 percentage points, four of them on each side",
                },
                Bound {
                    metric: Metric::MeanLossRun,
                    expected: 1.769,
                    lo: 1.52,
                    hi: 2.02,
                    reason: "the mean length of a run of lost packets of the chain (S12: 1.76); a standard deviation of the mean of 0.06, four of them on each side",
                },
            ],
        },
        Case {
            id: "9-reorder",
            title: "Reordering: a delay of 10 ms, 25 % of the packets sent on at once",
            link: "delay: 10ms, reorder: { share: 25% }",
            offered: "8mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::EarlyShare,
                    expected: 0.25,
                    lo: 0.228,
                    hi: 0.272,
                    reason: "`reorder P` sends the share P on at once and holds the rest for the delay (D-048, S12: 25.10 %); binomial, 6,000 packets, a standard deviation of 0.56 percentage points, four of them on each side",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "reordering loses nothing",
                },
                Bound {
                    metric: Metric::ReorderedRatio,
                    expected: 0.7,
                    lo: 0.55,
                    hi: 0.85,
                    reason: "KNOWN: the packets that are held arrive after later ones that went on at once; about 0.75 x (1 - 0.75^10) of the packets when 10 packets are sent within the delay (S12 saw 74.9 % at another rate)",
                },
            ],
        },
        Case {
            id: "10-reorder-gap",
            title: "Reordering with a gap: 25 % sent on at once, but never two within 5 packets",
            link: "delay: 10ms, reorder: { share: 25%, gap: 5 }",
            offered: "8mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![Bound {
                metric: Metric::EarlyShare,
                expected: 0.125,
                lo: 0.108,
                hi: 0.142,
                reason: "after one packet is sent on at once the next four are held, then each is chosen with 25 %: 1 / (gap - 1 + 1 / share) = 12.5 % (D-048, S12: 12.49 %); a standard deviation of 0.43 percentage points, four of them on each side",
            }],
        },
        Case {
            id: "11-duplicate",
            title: "Duplication of 20 %",
            link: "duplicate: 20%",
            offered: "8mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::DuplicateRatio,
                    expected: 0.20,
                    lo: 0.179,
                    hi: 0.221,
                    reason: "`duplicate P` copies a packet with the probability P (S12: 20.14 %); binomial, 6,000 packets, a standard deviation of 0.52 percentage points, four of them on each side",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "duplication loses nothing",
                },
            ],
        },
        Case {
            id: "12-duplicate-and-loss",
            title: "Duplication of 20 % with a loss of 10 %: each copy is lost on its own",
            link: "duplicate: 20%, loss: { model: random, p: 10% }",
            offered: "8mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::DuplicateRatio,
                    expected: 0.162,
                    lo: 0.143,
                    hi: 0.181,
                    reason: "KNOWN (D-048): a packet arrives twice if it is copied and both survive, 0.2 x 0.9 x 0.9 = 16.2 %, not 20 %; a standard deviation of 0.48 percentage points, four of them on each side",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.082,
                    lo: 0.068,
                    hi: 0.096,
                    reason: "KNOWN: a packet is lost if every copy is: 0.8 x 0.1 + 0.2 x 0.1 x 0.1 = 8.2 %, below the 10 % of a copy; a standard deviation of 0.35 percentage points, four of them on each side",
                },
            ],
        },
        Case {
            id: "13-dup-jitter-rate",
            title: "Duplication of 20 % in a direction with a jitter and a rate: two queues in one tree",
            link: "delay: 20ms, jitter: { amount: 5ms }, rate: 10mbit, duplicate: 20%",
            offered: "4mbit",
            size: 500,
            seconds: 6,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::DuplicateRatio,
                    expected: 0.20,
                    lo: 0.179,
                    hi: 0.221,
                    reason: "the copy is made in the first queue, where the loss and the rate are, and the tree of two `netem` is accepted by the kernel (S15, D-052); binomial, 6,000 packets, four standard deviations",
                },
                Bound {
                    metric: Metric::MinDelayMs,
                    expected: 15.0,
                    lo: 15.0,
                    hi: 15.6,
                    reason: "the second queue still holds the delay and its jitter: uniform jitter reaches down to the delay minus the amount; the rate adds the time of one frame (0.4 ms)",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "the offered 4 mbit and its copies are far below the 10 mbit of the link",
                },
            ],
        },
        Case {
            id: "14-reorder-with-rate",
            title: "Reordering with a rate limit of 2 mbit, offered 3 mbit: the rate holds",
            link: "delay: 10ms, reorder: { share: 25% }, rate: 2mbit",
            offered: "3mbit",
            size: 1000,
            seconds: 6,
            settle_seconds: 5,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::SteadyGoodputBps,
                    expected: 2_000_000.0 * 1000.0 / 1042.0,
                    lo: 1_881_000.0,
                    hi: 1_958_000.0,
                    reason: "as for the rate alone: 2 mbit of frames; a packet chosen for reordering would go past the rate if it were in the same queue (S12.9: 25 % of the packets did), so the rate is in a queue of its own in front of it (D-054)",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "the backlog stays below the queue of 1,000 packets, as for the rate alone",
                },
            ],
        },
        Case {
            id: "15-corrupt-64",
            title: "A corruption of 10 % with a payload of 64 bytes: the node drops the damaged packets",
            link: "corrupt: 10%",
            offered: "2048kbit",
            size: 64,
            seconds: 5,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::NodeDropRatio,
                    expected: 0.0943,
                    lo: 0.0861,
                    hi: 0.1026,
                    reason: "KNOWN (D-040, S13b): `netem` flips a bit in a random byte of the whole frame (106 bytes with the Ethernet header), and the node drops the frame unless the bit is in the source address of the Ethernet header (6 bytes), so 10 % x 100 / 106 = 9.43 %, not 10 %; binomial, about 20,000 frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::LostInQueueRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a corrupted packet arrives at the node, so the queue loses nothing: this is the difference between a corrupted and a lost packet",
                },
                Bound {
                    metric: Metric::Unaccounted,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "every packet is lost in the queue, dropped by the node or delivered, to the packet",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0943,
                    lo: 0.0861,
                    hi: 0.1026,
                    reason: "with the UDP checksum on, a damaged packet is lost to the program, so the loss the program sees is the share that the node dropped",
                },
                Bound {
                    metric: Metric::NamedShare,
                    expected: 0.8218,
                    lo: 0.7865,
                    hi: 0.8571,
                    reason: "KNOWN (D-040): of the 100 bytes of a 106 byte frame whose flipped bit gets it dropped, the tap can read the frame unless the bit is in the 18 bytes that say whose it is (the type of Ethernet, the version, header length and protocol of IP, its destination address, the destination port of UDP, the magic, version, flags and sequence number of the agent): (100 - 18) / 100, and the few sequence numbers that a flipped bit turns into another number that was not delivered; binomial, about 1886 dropped frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::NamedExcess,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "on a link that only corrupts every name stands for a frame that the node dropped, so there are not more names than dropped frames",
                },
            ],
        },
        Case {
            id: "16-corrupt-200",
            title: "A corruption of 10 % with a payload of 200 bytes",
            link: "corrupt: 10%",
            offered: "6400kbit",
            size: 200,
            seconds: 5,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::NodeDropRatio,
                    expected: 0.0975,
                    lo: 0.0891,
                    hi: 0.1059,
                    reason: "KNOWN (D-040, S13b): `netem` flips a bit in a random byte of the whole frame (242 bytes with the Ethernet header), and the node drops the frame unless the bit is in the source address of the Ethernet header (6 bytes), so 10 % x 236 / 242 = 9.75 %, not 10 %; binomial, about 20,000 frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::LostInQueueRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a corrupted packet arrives at the node, so the queue loses nothing: this is the difference between a corrupted and a lost packet",
                },
                Bound {
                    metric: Metric::Unaccounted,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "every packet is lost in the queue, dropped by the node or delivered, to the packet",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0975,
                    lo: 0.0891,
                    hi: 0.1059,
                    reason: "with the UDP checksum on, a damaged packet is lost to the program, so the loss the program sees is the share that the node dropped",
                },
                Bound {
                    metric: Metric::NamedShare,
                    expected: 0.9245,
                    lo: 0.9005,
                    hi: 0.9485,
                    reason: "KNOWN (D-040): of the 236 bytes of a 242 byte frame whose flipped bit gets it dropped, the tap can read the frame unless the bit is in the 18 bytes that say whose it is (the type of Ethernet, the version, header length and protocol of IP, its destination address, the destination port of UDP, the magic, version, flags and sequence number of the agent): (236 - 18) / 236, and the few sequence numbers that a flipped bit turns into another number that was not delivered; binomial, about 1950 dropped frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::NamedExcess,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "on a link that only corrupts every name stands for a frame that the node dropped, so there are not more names than dropped frames",
                },
            ],
        },
        Case {
            id: "17-corrupt-1200",
            title: "A corruption of 10 % with a payload of 1,200 bytes",
            link: "corrupt: 10%",
            offered: "38400kbit",
            size: 1200,
            seconds: 5,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::NodeDropRatio,
                    expected: 0.0995,
                    lo: 0.091,
                    hi: 0.108,
                    reason: "KNOWN (D-040, S13b): `netem` flips a bit in a random byte of the whole frame (1242 bytes with the Ethernet header), and the node drops the frame unless the bit is in the source address of the Ethernet header (6 bytes), so 10 % x 1236 / 1242 = 9.95 %, not 10 %; binomial, about 20,000 frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::LostInQueueRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a corrupted packet arrives at the node, so the queue loses nothing: this is the difference between a corrupted and a lost packet",
                },
                Bound {
                    metric: Metric::Unaccounted,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "every packet is lost in the queue, dropped by the node or delivered, to the packet",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0995,
                    lo: 0.091,
                    hi: 0.108,
                    reason: "with the UDP checksum on, a damaged packet is lost to the program, so the loss the program sees is the share that the node dropped",
                },
                Bound {
                    metric: Metric::NamedShare,
                    expected: 0.9856,
                    lo: 0.9748,
                    hi: 0.9963,
                    reason: "KNOWN (D-040): of the 1236 bytes of a 1242 byte frame whose flipped bit gets it dropped, the tap can read the frame unless the bit is in the 18 bytes that say whose it is (the type of Ethernet, the version, header length and protocol of IP, its destination address, the destination port of UDP, the magic, version, flags and sequence number of the agent): (1236 - 18) / 1236, and the few sequence numbers that a flipped bit turns into another number that was not delivered; binomial, about 1990 dropped frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::NamedExcess,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "on a link that only corrupts every name stands for a frame that the node dropped, so there are not more names than dropped frames",
                },
            ],
        },
        Case {
            id: "18-corrupt-and-loss",
            title: "A corruption of 10 % with a loss of 10 %: the result tells the two apart",
            link: "loss: { model: random, p: 10% }, corrupt: 10%",
            offered: "6400kbit",
            size: 200,
            seconds: 5,
            settle_seconds: 1,
            udp_checksum: true,
            bounds: vec![
                Bound {
                    metric: Metric::LostInQueueRatio,
                    expected: 0.1,
                    lo: 0.0915,
                    hi: 0.1085,
                    reason: "the loss comes first and drops 10 % of the packets in the queue; binomial, 20,000 packets, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::NodeDropRatio,
                    expected: 0.0975,
                    lo: 0.0887,
                    hi: 0.1064,
                    reason: "the corruption works on the 90 % that the loss let through: 10 % x 236 / 242 = 9.75 % of them are dropped by the node; binomial, about 18,000 frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::Unaccounted,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "every packet is lost in the queue, dropped by the node or delivered, to the packet",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.1878,
                    lo: 0.1767,
                    hi: 0.1988,
                    reason: "the program sees both as loss: 0.1 + 0.9 x 0.0975 = 18.78 %; the account of the direction is what tells the two causes apart",
                },
            ],
        },
        Case {
            id: "19-no-checksum-64",
            title: "A corruption of 10 % in a stream without the checksum of UDP, payload of 64 bytes: the damaged packet arrives",
            link: "corrupt: 10%",
            offered: "2048kbit",
            size: 64,
            seconds: 5,
            settle_seconds: 1,
            udp_checksum: false,
            bounds: vec![
                Bound {
                    metric: Metric::DamagedRatio,
                    expected: 0.0606,
                    lo: 0.0538,
                    hi: 0.0674,
                    reason: "KNOWN (D-040, S12): with the checksum off the kernel does not drop a flipped bit of the payload, so it arrives and the CRC-32 finds it; `netem` flips a bit in a random byte of the whole frame (106 bytes with the Ethernet header), 64 of which are payload, and the two bytes of the length field of UDP count 2/8 of a byte, because only a bit that is set there makes the datagram shorter, and a shorter datagram fails the CRC; 10 % x (64 + 2/8) / 106 = 6.06 %; binomial, 20,000 packets, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::NodeDropRatio,
                    expected: 0.0318,
                    lo: 0.0268,
                    hi: 0.0369,
                    reason: "the node still drops what is damaged in the headers of Ethernet, IP and UDP (destination address, type, the checksum of IP, the destination port, the checksum field that is no longer 0, a longer length): 10 % x (34 - 2/8) / 106 = 3.18 %; binomial, 20,000 frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::LostInQueueRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a corrupted packet arrives at the node, so the queue loses nothing",
                },
                Bound {
                    metric: Metric::Unaccounted,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "every packet is lost in the queue, dropped by the node or delivered, a damaged one included, to the packet",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0925,
                    lo: 0.0842,
                    hi: 0.1007,
                    reason: "a damaged packet is not an intact one, so it is lost to the stream as it is to the program; what is harmless is the bit in the source address of Ethernet (6 bytes) and in the source port of UDP (2 bytes), which the CRC-32 over the payload does not cover: 10 % x (106 - 8) / 106 = 9.25 %; binomial, 20,000 packets, four standard deviations on each side",
                },
            ],
        },
        Case {
            id: "20-no-checksum-200",
            title: "The same with a payload of 200 bytes",
            link: "corrupt: 10%",
            offered: "6400kbit",
            size: 200,
            seconds: 5,
            settle_seconds: 1,
            udp_checksum: false,
            bounds: vec![
                Bound {
                    metric: Metric::DamagedRatio,
                    expected: 0.0828,
                    lo: 0.075,
                    hi: 0.0906,
                    reason: "KNOWN (D-040, S12): with the checksum off the kernel does not drop a flipped bit of the payload, so it arrives and the CRC-32 finds it; `netem` flips a bit in a random byte of the whole frame (242 bytes with the Ethernet header), 200 of which are payload, and the two bytes of the length field of UDP count 3/8 of a byte, because only a bit that is set there makes the datagram shorter, and a shorter datagram fails the CRC; 10 % x (200 + 3/8) / 242 = 8.28 %; binomial, 20,000 packets, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::NodeDropRatio,
                    expected: 0.0139,
                    lo: 0.0105,
                    hi: 0.0173,
                    reason: "the node still drops what is damaged in the headers of Ethernet, IP and UDP (destination address, type, the checksum of IP, the destination port, the checksum field that is no longer 0, a longer length): 10 % x (34 - 3/8) / 242 = 1.39 %; binomial, 20,000 frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::LostInQueueRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a corrupted packet arrives at the node, so the queue loses nothing",
                },
                Bound {
                    metric: Metric::Unaccounted,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "every packet is lost in the queue, dropped by the node or delivered, a damaged one included, to the packet",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0967,
                    lo: 0.0883,
                    hi: 0.1051,
                    reason: "a damaged packet is not an intact one, so it is lost to the stream as it is to the program; what is harmless is the bit in the source address of Ethernet (6 bytes) and in the source port of UDP (2 bytes), which the CRC-32 over the payload does not cover: 10 % x (242 - 8) / 242 = 9.67 %; binomial, 20,000 packets, four standard deviations on each side",
                },
            ],
        },
        Case {
            id: "21-no-checksum-1200",
            title: "The same with a payload of 1,200 bytes",
            link: "corrupt: 10%",
            offered: "38400kbit",
            size: 1200,
            seconds: 5,
            settle_seconds: 1,
            udp_checksum: false,
            bounds: vec![
                Bound {
                    metric: Metric::DamagedRatio,
                    expected: 0.0967,
                    lo: 0.0883,
                    hi: 0.1051,
                    reason: "KNOWN (D-040, S12): with the checksum off the kernel does not drop a flipped bit of the payload, so it arrives and the CRC-32 finds it; `netem` flips a bit in a random byte of the whole frame (1242 bytes with the Ethernet header), 1200 of which are payload, and the two bytes of the length field of UDP count 5/8 of a byte, because only a bit that is set there makes the datagram shorter, and a shorter datagram fails the CRC; 10 % x (1200 + 5/8) / 1242 = 9.67 %; binomial, 20,000 packets, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::NodeDropRatio,
                    expected: 0.0027,
                    lo: 0.0012,
                    hi: 0.0042,
                    reason: "the node still drops what is damaged in the headers of Ethernet, IP and UDP (destination address, type, the checksum of IP, the destination port, the checksum field that is no longer 0, a longer length): 10 % x (34 - 5/8) / 1242 = 0.27 %; binomial, 20,000 frames, four standard deviations on each side",
                },
                Bound {
                    metric: Metric::LostInQueueRatio,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "a corrupted packet arrives at the node, so the queue loses nothing",
                },
                Bound {
                    metric: Metric::Unaccounted,
                    expected: 0.0,
                    lo: 0.0,
                    hi: 0.0,
                    reason: "every packet is lost in the queue, dropped by the node or delivered, a damaged one included, to the packet",
                },
                Bound {
                    metric: Metric::LossRatio,
                    expected: 0.0994,
                    lo: 0.0908,
                    hi: 0.1079,
                    reason: "a damaged packet is not an intact one, so it is lost to the stream as it is to the program; what is harmless is the bit in the source address of Ethernet (6 bytes) and in the source port of UDP (2 bytes), which the CRC-32 over the payload does not cover: 10 % x (1242 - 8) / 1242 = 9.94 %; binomial, 20,000 packets, four standard deviations on each side",
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
    // The packets that arrived within 5 ms: with a delay of 10 ms, those that `netem` sent on at once
    let early_share = delays_ms.iter().filter(|d| **d < 5.0).count() as f64 / n_or_one(&delays_ms);
    let received: std::collections::BTreeSet<u64> =
        file.records.iter().map(|r| r.sequence).collect();
    // The runs of lost packets in the result against the oracle of this file, which counts them its own way
    let oracle = mean_loss_run(&received, workload.stats.sent);
    assert!(
        (workload.stats.loss_runs.mean - oracle).abs() < 1e-9,
        "the result says a mean run of {} lost packets and the test counts {oracle}",
        workload.stats.loss_runs.mean
    );
    let tally = &result.directions[0];
    // A duplicating direction has no count of the packets the queue dropped: the measures that need it are not
    // numbers there, and a bound on one of them fails, as it should
    let lost_in_queue = tally.lost_in_queue.map(|lost| lost as f64);
    Measurement {
        delays_ms,
        loss_ratio: workload.stats.loss_ratio,
        reordered_ratio: workload.stats.reordered as f64 / workload.stats.received.max(1) as f64,
        steady_goodput_bps: steady,
        duplicate_ratio: workload.stats.duplicates as f64 / workload.stats.sent.max(1) as f64,
        early_share,
        mean_loss_run: mean_loss_run(&received, workload.stats.sent),
        node_drop_ratio: tally.dropped_corrupted as f64 / tally.reached_node.max(1) as f64,
        lost_in_queue_ratio: lost_in_queue.map_or(f64::NAN, |lost| lost / tally.sent.max(1) as f64),
        damaged_ratio: workload.corrupted as f64 / workload.stats.sent.max(1) as f64,
        // NaN where the result has nothing, so that a bound on it fails
        jitter_mean_ms: workload
            .stats
            .jitter
            .as_ref()
            .map_or(f64::NAN, |jitter| jitter.mean_ns as f64 / 1e6),
        extent_mean: workload
            .stats
            .reordering
            .as_ref()
            .map_or(f64::NAN, |reordering| reordering.extent.mean),
        late_time_mean_ms: workload
            .stats
            .reordering
            .as_ref()
            .map_or(f64::NAN, |reordering| {
                reordering.late_time.mean_ns as f64 / 1e6
            }),
        named_share: workload.corrupted_named.map_or(f64::NAN, |named| {
            named as f64 / tally.dropped_corrupted.max(1) as f64
        }),
        named_excess: workload.corrupted_named.map_or(f64::NAN, |named| {
            named.saturating_sub(tally.dropped_corrupted) as f64
        }),
        unaccounted: lost_in_queue.map_or(f64::NAN, |lost| {
            tally.sent as f64
                - (lost + (tally.dropped_corrupted + tally.delivered) as f64)
                - tally.receive_buffer_drops as f64
        }),
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

fn n_or_one(values: &[f64]) -> f64 {
    values.len().max(1) as f64
}

/// The mean length of the runs of consecutive sequence numbers in `0..sent` that never arrived; 0 if none was lost.
fn mean_loss_run(received: &std::collections::BTreeSet<u64>, sent: u64) -> f64 {
    let (mut runs, mut lost, mut in_run) = (0_u64, 0_u64, false);
    for sequence in 0..sent {
        if received.contains(&sequence) {
            in_run = false;
        } else {
            lost += 1;
            if !in_run {
                runs += 1;
                in_run = true;
            }
        }
    }
    if runs == 0 {
        0.0
    } else {
        lost as f64 / runs as f64
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
            Metric::LossRatio
            | Metric::ReorderedRatio
            | Metric::DuplicateRatio
            | Metric::NodeDropRatio
            | Metric::LostInQueueRatio
            | Metric::DamagedRatio
            | Metric::NamedShare
            | Metric::EarlyShare => {
                format!("{:+.2} pp", (verdict.value - bound.expected) * 100.0)
            }
            Metric::MeanLossRun
            | Metric::Unaccounted
            | Metric::NamedExcess
            | Metric::ExtentMean => {
                format!("{:+.2} packets", verdict.value - bound.expected)
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
    // By the number at the start of the name (`0-machine`, `1-delay`, ..., `10-reorder-gap`), not by the letters
    parts.sort_by_key(|part| {
        let name = part.file_name().unwrap().to_string_lossy().into_owned();
        let number = name
            .split('-')
            .next()
            .and_then(|number| number.parse::<u32>().ok())
            .unwrap_or(u32::MAX);
        (number, name)
    });
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
fn a_corruption_is_dropped_by_the_node_and_not_lost_in_the_queue_at_64_bytes() {
    run_and_judge("15-corrupt-64");
}

#[test]
fn a_corruption_is_dropped_by_the_node_and_not_lost_in_the_queue_at_200_bytes() {
    run_and_judge("16-corrupt-200");
}

#[test]
fn a_corruption_is_dropped_by_the_node_and_not_lost_in_the_queue_at_1200_bytes() {
    run_and_judge("17-corrupt-1200");
}

#[test]
fn a_corruption_and_a_loss_are_told_apart() {
    run_and_judge("18-corrupt-and-loss");
}

#[test]
fn a_damaged_packet_arrives_and_is_found_without_the_checksum_at_64_bytes() {
    run_and_judge("19-no-checksum-64");
}

#[test]
fn a_damaged_packet_arrives_and_is_found_without_the_checksum_at_200_bytes() {
    run_and_judge("20-no-checksum-200");
}

#[test]
fn a_damaged_packet_arrives_and_is_found_without_the_checksum_at_1200_bytes() {
    run_and_judge("21-no-checksum-1200");
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

#[test]
fn a_loss_in_bursts_has_the_stationary_loss_and_the_burst_length_of_gilbert_elliott() {
    run_and_judge("7-burst-gilbert-elliott");
}

#[test]
fn a_loss_in_bursts_has_the_stationary_loss_and_the_burst_length_of_the_four_state_model() {
    run_and_judge("8-burst-four-state");
}

#[test]
fn a_reordering_sends_the_configured_share_on_at_once() {
    run_and_judge("9-reorder");
}

#[test]
fn a_reordering_with_a_gap_sends_a_smaller_share_on_at_once() {
    run_and_judge("10-reorder-gap");
}

#[test]
fn a_duplication_copies_the_configured_share() {
    run_and_judge("11-duplicate");
}

#[test]
fn each_copy_of_a_duplicated_packet_is_lost_on_its_own() {
    run_and_judge("12-duplicate-and-loss");
}

#[test]
fn a_duplication_works_in_a_direction_with_a_jitter_and_a_rate() {
    run_and_judge("13-dup-jitter-rate");
}

#[test]
fn a_reordering_does_not_get_past_the_rate_limit() {
    run_and_judge("14-reorder-with-rate");
}

// ---- the model behind the case of a jitter with a rate limit ----

/// What a model of a direction with a delay, a normal jitter and a rate limit gives for a stream.
#[derive(Debug)]
struct Modelled {
    /// The delay of every packet, in the order of the stream.
    delays_ms: Vec<f64>,
    mean_ms: f64,
    stdev_ms: f64,
    reordered_ratio: f64,
}

/// How the rate and the delay with its jitter are put together.
#[derive(Clone, Copy)]
enum Composition {
    /// One `netem` qdisc for both: a packet leaves at the later of its own moment and the moment of the packet
    /// in front of it, plus its serialisation (`netem_enqueue` in `net/sched/sch_netem.c`). The rate stage
    /// acts behind the jitter and keeps the order.
    OneQdisc,
    /// The rate first, in a queue of its own, then each packet travels for its own delay. This is what the
    /// scenario says, and what the backend builds with two qdiscs.
    Series,
}

/// Runs the model for `n` packets a gap of `interval_ms` apart. The numbers are drawn from a small generator
/// with a fixed seed, so the answer is always the same.
fn model(
    composition: Composition,
    n: usize,
    interval_ms: f64,
    mean_ms: f64,
    sigma_ms: f64,
    serialisation_ms: f64,
) -> Modelled {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut uniform = move || {
        // xorshift64*, mapped to (0, 1)
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        ((state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1_u64 << 53) as f64
    };
    let (mut last_leaves, mut serialised_until): (Option<f64>, f64) = (None, 0.0);
    let (mut delays, mut highest_arrival, mut reordered) =
        (Vec::with_capacity(n), f64::MIN, 0_usize);
    for i in 0..n {
        let enters = i as f64 * interval_ms;
        // Box-Muller; the table of netem is cut off at four standard deviations
        let normal =
            (-2.0 * uniform().ln()).sqrt() * (2.0 * std::f64::consts::PI * uniform()).cos();
        let drawn = (mean_ms + sigma_ms * normal.clamp(-4.0, 4.0)).max(0.0);
        let arrives = match composition {
            Composition::OneQdisc => {
                let leaves = match last_leaves {
                    Some(before) if before > enters => {
                        before.max(enters + drawn) + serialisation_ms
                    }
                    _ => enters + drawn + serialisation_ms,
                };
                last_leaves = Some(leaves);
                leaves
            }
            Composition::Series => {
                serialised_until = serialised_until.max(enters) + serialisation_ms;
                serialised_until + drawn
            }
        };
        if arrives < highest_arrival {
            reordered += 1;
        }
        highest_arrival = highest_arrival.max(arrives);
        delays.push(arrives - enters);
    }
    let mean = delays.iter().sum::<f64>() / n as f64;
    let variance = delays.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    Modelled {
        delays_ms: delays,
        mean_ms: mean,
        stdev_ms: variance.sqrt(),
        reordered_ratio: reordered as f64 / n as f64,
    }
}

/// The numbers of the case of a jitter with a rate limit are not guesses: a model gives them. One qdisc for
/// both gives a mean 1.6 ms too high, a jitter of 4.1 ms instead of 5 and no reordering, which is what was
/// measured before the backend built two qdiscs. The series of the two stages gives what the scenario says.
#[test]
fn the_model_gives_the_numbers_of_a_jitter_with_a_rate_limit() {
    // 1,200 bytes of payload and 42 of headers at 10 mbit take 0.9936 ms; a packet every 4.8 ms
    let one = model(Composition::OneQdisc, 200_000, 4.8, 40.0, 5.0, 0.9936);
    assert!((42.5..42.7).contains(&one.mean_ms), "{one:?}");
    assert!((4.0..4.3).contains(&one.stdev_ms), "{one:?}");
    assert!(one.reordered_ratio.abs() < f64::EPSILON, "{one:?}");
    let series = model(Composition::Series, 200_000, 4.8, 40.0, 5.0, 0.9936);
    assert!((40.95..41.05).contains(&series.mean_ms), "{series:?}");
    assert!((4.95..5.05).contains(&series.stdev_ms), "{series:?}");
    assert!((0.2..0.35).contains(&series.reordered_ratio), "{series:?}");
    // Canary: the model can say something else. With a gap far longer than the jitter, one qdisc is as good
    // as two, and with no rate the mean is the configured one
    let spaced = model(Composition::OneQdisc, 200_000, 400.0, 40.0, 5.0, 0.0);
    assert!((39.95..40.05).contains(&spaced.mean_ms), "{spaced:?}");
}

/// Canary: the bounds of the case would have caught the backend as it was before it built two qdiscs. The
/// numbers of one qdisc for both (the model agrees with what was measured: a mean of 42.7 ms, a jitter of 4.2
/// ms, no reordering) are judged with the bounds of the case, and every one of the three fails.
#[test]
fn canary_the_bounds_of_the_case_catch_a_rate_stage_behind_the_jitter() {
    let one = model(Composition::OneQdisc, 1_667, 4.8, 40.0, 5.0, 0.9936);
    let mut delays = one.delays_ms.clone();
    delays.sort_by(f64::total_cmp);
    let measurement = synthetic(&delays, 0.0, one.reordered_ratio, 0.0);
    let failed = failing_metrics("6-jitter-and-rate", &measurement);
    assert!(failed.contains(&Metric::MeanDelayMs), "{one:?} {failed:?}");
    assert!(failed.contains(&Metric::StdevDelayMs), "{one:?} {failed:?}");
    assert!(
        failed.contains(&Metric::ReorderedRatio),
        "{one:?} {failed:?}"
    );
    // and the series of the two stages passes, so the canary is not just a case that fails for everything
    let series = model(Composition::Series, 1_667, 4.8, 40.0, 5.0, 0.9936);
    let mut delays = series.delays_ms.clone();
    delays.sort_by(f64::total_cmp);
    let measurement = synthetic(&delays, 0.0, series.reordered_ratio, 0.0);
    assert_eq!(failing_metrics("6-jitter-and-rate", &measurement), []);
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
        duplicate_ratio: 0.0,
        early_share: 0.0,
        mean_loss_run: 0.0,
        node_drop_ratio: 0.0,
        lost_in_queue_ratio: 0.0,
        damaged_ratio: 0.0,
        named_share: 0.0,
        named_excess: 0.0,
        jitter_mean_ms: 0.0,
        extent_mean: 0.0,
        late_time_mean_ms: 0.0,
        unaccounted: 0.0,
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

/// Loss that is not in bursts: the same mean loss as the Gilbert-Elliott case, but every packet on its own. The
/// loss ratio fits, the length of the runs does not, which is what the burst metric is for.
#[test]
fn canary_loss_without_bursts_fails_the_burst_length_of_the_burst_case() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let failed = wrong_on_purpose(
        "7-burst-gilbert-elliott",
        "loss: { model: random, p: 4.762% }",
        12,
        1,
    );
    assert!(failed.contains(&Metric::MeanLossRun), "{failed:?}");
    assert!(
        !failed.contains(&Metric::LossRatio),
        "the same mean loss must pass: {failed:?}"
    );
}

/// A reordering and a duplication that are not applied, and a reordering with another share.
#[test]
fn canary_a_reordering_and_a_duplication_that_are_wrong_fail_their_cases() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    assert!(wrong_on_purpose("9-reorder", "delay: 10ms", 4, 1).contains(&Metric::EarlyShare));
    let share = wrong_on_purpose("9-reorder", "delay: 10ms, reorder: { share: 50% }", 4, 1);
    assert!(share.contains(&Metric::EarlyShare), "{share:?}");
    let no_gap = wrong_on_purpose(
        "10-reorder-gap",
        "delay: 10ms, reorder: { share: 25% }",
        4,
        1,
    );
    assert!(no_gap.contains(&Metric::EarlyShare), "{no_gap:?}");
    assert!(wrong_on_purpose("11-duplicate", "", 3, 1).contains(&Metric::DuplicateRatio));
    // A duplication of 20 % that the loss does not touch is the mistake that the case of D-048 is about
    let untouched = wrong_on_purpose("12-duplicate-and-loss", "duplicate: 20%", 4, 1);
    assert!(untouched.contains(&Metric::DuplicateRatio), "{untouched:?}");
}

/// A rate that is not applied in front of a reordering: the offered 3 mbit pass.
#[test]
fn canary_a_rate_that_is_missing_fails_the_case_of_the_rate_with_reordering() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let failed = wrong_on_purpose(
        "14-reorder-with-rate",
        "delay: 10ms, reorder: { share: 25% }",
        4,
        1,
    );
    assert!(failed.contains(&Metric::SteadyGoodputBps), "{failed:?}");
}

/// A corruption that is not applied, and a loss of the same size in its place: the loss the program sees
/// is about the same as for a corruption, and only the tally of the node tells them apart.
#[test]
fn canary_a_corruption_that_is_missing_or_is_a_loss_fails_the_corruption_case() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let none = wrong_on_purpose("16-corrupt-200", "", 3, 1);
    assert!(
        none.contains(&Metric::NodeDropRatio) && none.contains(&Metric::LossRatio),
        "{none:?}"
    );
    assert!(
        none.contains(&Metric::NamedShare),
        "a link that does not corrupt has no names: {none:?}"
    );
    let as_loss = wrong_on_purpose("16-corrupt-200", "loss: { model: random, p: 9.75% }", 5, 1);
    assert!(
        as_loss.contains(&Metric::NodeDropRatio) && as_loss.contains(&Metric::LostInQueueRatio),
        "{as_loss:?}"
    );
    assert!(
        !as_loss.contains(&Metric::LossRatio),
        "the loss that the program sees is the same, so the tally must be what fails: {as_loss:?}"
    );
}

/// A stream with the checksum on over the same link is not the stream without it: the kernel drops the damaged
/// packets, so none arrives for the CRC-32 to find, and the node drops more. And a link that does not corrupt
/// finds none either. Both would pass if the cases only looked at the loss the program sees.
#[test]
fn canary_a_checksum_that_is_on_or_a_corruption_that_is_missing_fails_the_case_without_it() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let real = cases()
        .into_iter()
        .find(|case| case.id == "20-no-checksum-200")
        .unwrap();
    let with_checksum = Case {
        udp_checksum: true,
        bounds: Vec::new(),
        ..cases()
            .into_iter()
            .find(|case| case.id == "20-no-checksum-200")
            .unwrap()
    };
    let failing: Vec<Metric> = judge(&real.bounds, &measure(&with_checksum))
        .iter()
        .filter(|verdict| !verdict.passed())
        .map(|verdict| verdict.bound.metric)
        .collect();
    assert!(
        failing.contains(&Metric::DamagedRatio) && failing.contains(&Metric::NodeDropRatio),
        "{failing:?}"
    );
    let none = wrong_on_purpose("20-no-checksum-200", "", 3, 1);
    assert!(
        none.contains(&Metric::DamagedRatio) && none.contains(&Metric::LossRatio),
        "{none:?}"
    );
}

/// A link without a jitter has no jitter and reorders nothing: the cases for the jitter fail it, with the jitter
/// as NaN for the extent (nothing was reordered) and near zero for the estimate.
#[test]
fn canary_a_link_without_a_jitter_fails_the_jitter_and_the_extent() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let none = wrong_on_purpose("4-jitter-uniform", "delay: 20ms", 3, 1);
    for metric in [
        Metric::JitterMeanMs,
        Metric::ExtentMean,
        Metric::LateTimeMeanMs,
    ] {
        assert!(none.contains(&metric), "{metric:?} passed: {none:?}");
    }
}

/// The tally must add up. A measurement in which a packet is missing from it fails the judging.
#[test]
fn canary_a_tally_that_does_not_add_up_fails() {
    let mut missing = synthetic(&[1.0], 0.0, 0.0, 0.0);
    missing.unaccounted = 3.0;
    assert_eq!(
        failing_metrics("16-corrupt-200", &missing),
        [
            Metric::NodeDropRatio,
            Metric::Unaccounted,
            Metric::LossRatio,
            Metric::NamedShare
        ]
    );
}

/// The naming can fail too. A result with more names than the node dropped frames, and one that names nothing
/// (no tap, or a tap that read nothing), are both judged wrong; a result with no names at all is NaN and fails.
#[test]
fn canary_names_without_a_dropped_frame_or_no_names_at_all_fail_the_naming() {
    let mut faithful = synthetic(&[1.0], 0.0975, 0.0, 0.0);
    faithful.node_drop_ratio = 0.0975;
    faithful.named_share = 0.9245;
    assert_eq!(
        failing_metrics("16-corrupt-200", &faithful),
        Vec::<Metric>::new()
    );
    let mut too_many = synthetic(&[1.0], 0.0975, 0.0, 0.0);
    too_many.node_drop_ratio = 0.0975;
    too_many.named_share = 0.9245;
    too_many.named_excess = 2.0;
    assert_eq!(
        failing_metrics("16-corrupt-200", &too_many),
        [Metric::NamedExcess]
    );
    let mut nothing = synthetic(&[1.0], 0.0975, 0.0, 0.0);
    nothing.node_drop_ratio = 0.0975;
    nothing.named_share = f64::NAN;
    nothing.named_excess = f64::NAN;
    assert_eq!(
        failing_metrics("16-corrupt-200", &nothing),
        [Metric::NamedShare, Metric::NamedExcess]
    );
    let mut all = synthetic(&[1.0], 0.0975, 0.0, 0.0);
    all.node_drop_ratio = 0.0975;
    all.named_share = 1.0;
    assert_eq!(
        failing_metrics("16-corrupt-200", &all),
        [Metric::NamedShare]
    );
}
