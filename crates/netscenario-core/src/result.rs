//! The result of a run (RPT-01, ENV-02, ENV-06). The fields are described in `docs/RESULTS.md`.

use serde::{Deserialize, Serialize};

use crate::machine::Provenance;
use crate::stats::FlowStats;

/// The value of `format` in every result file.
pub const RESULT_FORMAT: &str = "netscenario-result";

/// The version of the result format. It changes when a field changes its meaning or goes away; a new field
/// does not change it. Before 1.0 a version may break its predecessor and says so in the changelog.
pub const RESULT_VERSION: u32 = 0;

/// Everything a run found out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RunResult {
    /// Always `netscenario-result`.
    pub format: String,
    /// The version of the format, [`RESULT_VERSION`].
    pub version: u32,
    /// Which scenario was run.
    pub scenario: ScenarioInfo,
    /// Where, when and with what it was run.
    pub provenance: Provenance,
    /// The seed, and whether the run can be repeated.
    pub seed: SeedInfo,
    /// The limits of the backend that the scenario accepted.
    pub accepted_restrictions: Vec<AcceptedRestriction>,
    /// The address of each end of each link.
    pub addresses: Vec<AddressInfo>,
    /// The times of the run.
    pub timing: Timing,
    /// One entry for each workload, in the order of the scenario.
    pub workloads: Vec<WorkloadResult>,
}

/// Which scenario a result belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScenarioInfo {
    /// The name in the scenario.
    pub name: String,
    /// `sha256:` and the SHA-256 of the scenario in its canonical JSON form (the keys in the order of the
    /// format, defaults left out), so a change of the comments or the layout of the file does not change it.
    pub hash: String,
}

/// The seed of the run (ENV-01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SeedInfo {
    /// The seed from which the seeds of all random parts were derived, or none if the backend cannot apply one.
    pub value: Option<u64>,
    /// Whether the run drew the seed because the scenario has none.
    pub drawn: bool,
    /// Whether the same seed gives the same lost packets in this run. `false` for a scenario with loss on a
    /// backend that cannot apply a seed, and for a run in which a queue counted packets that the workloads
    /// did not send (see `queue_checks`).
    pub loss_reproducible: bool,
    /// For each direction with random loss: the packets that entered its queue, counted by the queue, against
    /// the packets that the workloads sent into it (ENV-07). The pattern of lost packets is a function of the
    /// seed and of this number, so the same seed repeats it only if the two agree. Empty if the backend cannot
    /// count, or the run has no seed.
    #[serde(default)]
    pub queue_checks: Vec<QueueCheck>,
}

/// The packets of one direction of a link that drew a number for the random loss: counted by the queue, and
/// sent by the workloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QueueCheck {
    /// The link.
    pub link: String,
    /// `forward` (from the first node of `between` to the second) or `reverse`.
    pub direction: String,
    /// The packets that the workloads sent into the direction (those the kernel took).
    pub expected: u64,
    /// The packets that entered the queue of the direction.
    pub counted: u64,
}

impl QueueCheck {
    /// A check of one direction: the packets the workloads sent into it against the packets its queue counted.
    #[must_use]
    pub fn new(
        link: impl Into<String>,
        direction: impl Into<String>,
        expected: u64,
        counted: u64,
    ) -> Self {
        Self {
            link: link.into(),
            direction: direction.into(),
            expected,
            counted,
        }
    }

    /// Whether the queue saw exactly the packets of the workloads.
    #[must_use]
    pub fn agrees(&self) -> bool {
        self.expected == self.counted
    }
}

/// A restriction of the backend that the scenario accepted (`allow-limited`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcceptedRestriction {
    /// The id of the feature.
    pub feature: String,
    /// What is different from the ideal.
    pub restriction: String,
}

/// The address of a node on a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AddressInfo {
    /// The node.
    pub node: String,
    /// The link.
    pub link: String,
    /// The IPv4 address.
    pub address: String,
}

/// The times of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Timing {
    /// The length of the measured phase in the scenario, in nanoseconds.
    pub duration_ns: u64,
    /// How long the run waited after the last packet was sent for the packets still in the network. A packet
    /// that needs longer counts as lost.
    pub settle_ns: u64,
}

/// What the sending side did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SenderInfo {
    /// The most that a packet left later than its moment, in nanoseconds. A value that is large against the
    /// interval between packets means that the machine, not the network, shaped the stream.
    pub max_lateness_ns: u64,
    /// How many packets the kernel refused to take.
    pub send_errors: u64,
    /// The first such error.
    pub first_error: Option<String>,
}

/// The result of one workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WorkloadResult {
    /// The id in the scenario.
    pub id: String,
    /// The kind, `udp-cbr`.
    pub kind: String,
    /// The sending node.
    pub from: String,
    /// The receiving node.
    pub to: String,
    /// The address the packets were sent from (the address of `from`).
    pub from_address: String,
    /// The address the packets were sent to, and the port.
    pub to_address: String,
    /// The rate of the payload in the scenario.
    pub rate_bits_per_second: u64,
    /// The payload of a packet in bytes.
    pub packet_size_bytes: u64,
    /// When the workload started, from the start of the measured phase, in nanoseconds.
    pub start_ns: u64,
    /// How long the workload sent, in nanoseconds.
    pub duration_ns: u64,
    /// The packets, delays and throughput.
    #[serde(flatten)]
    pub stats: FlowStats,
    /// The sending side.
    pub sender: SenderInfo,
    /// How many datagrams reached the receiver that were not packets of the agent.
    pub foreign_datagrams: u64,
    /// The file of per-packet records of the receiver (binary, see `docs/AGENT.md`), relative to the result.
    pub records_file: String,
    /// The JSON Lines file with one line for each packet, if it was asked for, relative to the result.
    pub packets_file: Option<String>,
}

impl RunResult {
    /// The result as JSON text, indented, with a line feed at the end.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self)
            .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"));
        text.push('\n');
        text
    }

    /// Reads a result from JSON text.
    ///
    /// # Errors
    ///
    /// The text is not JSON, is not a result, or is a result of a version this program does not read.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let result: Self = serde_json::from_str(text)
            .map_err(|error| format!("this is not a netscenario result: {error}"))?;
        if result.format != RESULT_FORMAT {
            return Err(format!(
                "this is not a netscenario result (format '{}')",
                result.format
            ));
        }
        if result.version != RESULT_VERSION {
            return Err(format!(
                "this result has version {}; this program reads version {RESULT_VERSION}",
                result.version
            ));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::stats::{FlowStats, Throughput};

    fn example() -> RunResult {
        RunResult {
            format: RESULT_FORMAT.to_owned(),
            version: RESULT_VERSION,
            scenario: ScenarioInfo {
                name: "t".to_owned(),
                hash: "sha256:00".to_owned(),
            },
            provenance: Provenance {
                netscenario: "0.0.0".to_owned(),
                started_at: "2026-01-01T00:00:00Z".to_owned(),
                backend: "kernel".to_owned(),
                backend_details: BTreeMap::new(),
                kernel: None,
                cpu_model: None,
                cpus: Some(4),
            },
            seed: SeedInfo {
                value: Some(7),
                drawn: false,
                loss_reproducible: true,
                queue_checks: Vec::new(),
            },
            accepted_restrictions: Vec::new(),
            addresses: Vec::new(),
            timing: Timing {
                duration_ns: 1,
                settle_ns: 2,
            },
            workloads: vec![WorkloadResult {
                id: "w".to_owned(),
                kind: "udp-cbr".to_owned(),
                from: "a".to_owned(),
                to: "b".to_owned(),
                from_address: "10.100.0.1".to_owned(),
                to_address: "10.100.0.2:5000".to_owned(),
                rate_bits_per_second: 1_000_000,
                packet_size_bytes: 1000,
                start_ns: 0,
                duration_ns: 1,
                stats: FlowStats {
                    sent: 2,
                    received: 1,
                    lost: 1,
                    loss_ratio: 0.5,
                    duplicates: 0,
                    reordered: 0,
                    unexpected: 0,
                    delay: None,
                    throughput: Throughput {
                        bin_ns: 1_000_000_000,
                        bits_per_second: vec![8000.0],
                    },
                },
                sender: SenderInfo {
                    max_lateness_ns: 3,
                    send_errors: 0,
                    first_error: None,
                },
                foreign_datagrams: 0,
                records_file: "flow-0.records".to_owned(),
                packets_file: None,
            }],
        }
    }

    #[test]
    fn a_result_survives_the_json() {
        let result = example();
        let text = result.to_json();
        assert!(text.ends_with("}\n"));
        assert_eq!(RunResult::from_json(&text).unwrap(), result);
    }

    /// The statistics are fields of the workload, not a nested object, as `docs/RESULTS.md` says.
    #[test]
    fn the_statistics_sit_in_the_workload() {
        let json: serde_json::Value = serde_json::from_str(&example().to_json()).unwrap();
        let workload = &json["workloads"][0];
        assert_eq!(workload["sent"], 2);
        assert_eq!(workload["lost"], 1);
        assert_eq!(workload["loss_ratio"], 0.5);
        assert!(workload["delay"].is_null());
        assert_eq!(workload["throughput"]["bits_per_second"][0], 8000.0);
        assert_eq!(json["format"], "netscenario-result");
    }

    /// Canary: a file that is not a result, or is of another version, is refused and says so.
    #[test]
    fn other_files_are_refused() {
        assert!(
            RunResult::from_json("{}")
                .unwrap_err()
                .contains("not a netscenario result")
        );
        assert!(RunResult::from_json("nonsense").is_err());
        let mut result = example();
        result.version = 1;
        let error = RunResult::from_json(&result.to_json()).unwrap_err();
        assert!(
            error.contains("version 1") && error.contains("version 0"),
            "{error}"
        );
        let mut result = example();
        result.format = "something-else".to_owned();
        assert!(
            RunResult::from_json(&result.to_json())
                .unwrap_err()
                .contains("something-else")
        );
    }
}
