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
    /// Each node with its role and the address of its own, which the workloads and programs use (D-078). Empty in a result of
    /// release 0.2 or earlier.
    #[serde(default)]
    pub nodes: Vec<NodeInfo>,
    /// The routes of every node as the run had them: the automatic ones and the listed ones (D-078). Empty in a result of
    /// release 0.2 or earlier.
    #[serde(default)]
    pub routes: Vec<RouteInfo>,
    /// The times of the run.
    pub timing: Timing,
    /// One entry for each workload, in the order of the scenario.
    pub workloads: Vec<WorkloadResult>,
    /// For each direction of a link that carried a workload: where its packets went (D-040). Empty if the
    /// backend cannot count, or the scenario runs a program of your own, whose packets the agent does not count.
    #[serde(default)]
    pub directions: Vec<DirectionTally>,
    /// For each direction with a `queue`: how the link was built and what the kernel counted in the queue (PRB-10, D-082).
    #[serde(default)]
    pub queues: Vec<QueueInfo>,
    /// What the run did to the links while it ran, one entry for each step of the timeline, in the order they were
    /// done (D-038). Empty for a scenario without a timeline.
    #[serde(default)]
    pub events: Vec<EventRecord>,
    /// What the backend says about how it did the run, besides the result: one sentence for each fact, for example
    /// that the changes of a link were made the slow way after the fast way stopped working (D-063). Empty if the
    /// run went as usual.
    #[serde(default)]
    pub notes: Vec<String>,
    /// One entry for each program of your own (`kind: command`), in the order of the scenario (D-072). Empty for a
    /// scenario without one.
    #[serde(default)]
    pub programs: Vec<ProgramResult>,
    /// One entry for each captured interface (`measure.capture`), two for each link, in the order of the list and, for a
    /// link, of `between` (PRB-09). Empty if nothing is captured.
    #[serde(default)]
    pub captures: Vec<CaptureResult>,
    /// One entry for each TCP workload (`tcp-bulk`, `request-response`), in the order of the scenario (D-086). Empty for a
    /// scenario without one.
    #[serde(default)]
    pub tcp: Vec<TcpResult>,
    /// Whether the run can be repeated exactly or has to be repeated and read with an interval (D-086).
    #[serde(default)]
    pub reproducibility: Reproducibility,
    /// What the checks of `expect` found, if the scenario has checks on one run (ASR-07, D-094). Absent for a scenario without
    /// them, for a scenario whose checks judge a set of runs, and for a run that was cut short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<crate::expect::ExpectResult>,
}

/// Whether a run can be repeated exactly (D-086).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Reproducibility {
    /// `exact`: what is seeded and driven by the packet clock, which is every workload that sends UDP; the same seed gives the
    /// same lost packets (and `seed.loss_reproducible` says where that is not so for another reason). `statistical`: anything
    /// driven by feedback, which is TCP: the same file gives a different run, and a claim is made over repetitions.
    pub class: String,
    /// Why, for `statistical`: one sentence for each reason. Empty for `exact`.
    pub because: Vec<String>,
}

impl Reproducibility {
    /// The class of a run that can be repeated exactly.
    pub const EXACT: &'static str = "exact";
    /// The class of a run that has to be repeated and read with an interval.
    pub const STATISTICAL: &'static str = "statistical";

    /// A run that can be repeated exactly.
    #[must_use]
    pub fn exact() -> Self {
        Self {
            class: Self::EXACT.to_owned(),
            because: Vec::new(),
        }
    }

    /// A run that cannot, and why.
    #[must_use]
    pub fn statistical(because: Vec<String>) -> Self {
        Self {
            class: Self::STATISTICAL.to_owned(),
            because,
        }
    }
}

impl Default for Reproducibility {
    /// A result of an older release has no field: its runs were UDP, which is exact.
    fn default() -> Self {
        Self::exact()
    }
}

/// What came of one TCP workload (WRK-04, WRK-05, PRB-12; D-086). Times are in nanoseconds from the start of the measured phase;
/// they are what the agents saw on `CLOCK_MONOTONIC`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TcpResult {
    /// The id in the scenario.
    pub id: String,
    /// `tcp-bulk` or `request-response`.
    pub kind: String,
    /// The node that opened the connections.
    pub from: String,
    /// The node that listened.
    pub to: String,
    /// The address the connections were made from.
    pub from_address: String,
    /// The address and port they were made to.
    pub to_address: String,
    /// The congestion control the sockets had, as the kernel named it.
    pub congestion: Option<String>,
    /// How many connections were started.
    pub connections: u32,
    /// How many of them failed: could not be made, broke, got no answer in time, or were stopped before they were done.
    pub failed_connections: u32,
    /// When the workload was to start, from the start of the measured phase.
    pub start_ns: u64,
    /// How long its window is (`stop` minus `start`).
    pub window_ns: u64,
    /// The payload bytes that the client sent.
    pub bytes_sent: u64,
    /// The payload bytes that arrived: at the server for `tcp-bulk`, at the client (the responses) for `request-response`.
    pub bytes_received: u64,
    /// The goodput: the payload that arrived at the receiving end, by time: at the server for `tcp-bulk`, at the client (the
    /// responses) for `request-response`.
    pub goodput: Option<Goodput>,
    /// One entry for each connection.
    pub transfers: Vec<TransferInfo>,
    /// The segments retransmitted over the life of the connections, from the last sample of `TCP_INFO` of each.
    pub retransmitted_segments: u64,
    /// The smoothed round trip time of the kernel, from the samples of `TCP_INFO` taken during the transfer.
    pub rtt: Option<crate::stats::DelaySummary>,
    /// The requests of a `request-response` workload.
    pub requests: Option<RequestInfo>,
    /// The first reason for a failed connection, as text.
    pub first_error: Option<String>,
    /// Why the samples of `TCP_INFO` could not be taken, if they could not.
    pub info_error: Option<String>,
    /// Whether the run was interrupted before the client was done.
    pub stopped: bool,
    /// The file of the client, relative to `result.json`: the life of the connections, the samples of `TCP_INFO`, the requests
    /// (the format is in `docs/AGENT.md`).
    pub client_file: String,
    /// The file of the server: the payload by interval.
    pub server_file: String,
    /// How long the workload took to be back at its goodput after each event of the timeline on its path, in either direction
    /// (D-088); empty without a timeline, and in a result of an older run.
    #[serde(default)]
    pub recoveries: Vec<crate::recovery::Recovery>,
}

impl TcpResult {
    /// The completion time of the workload in nanoseconds: from the first connect to the end of the last connection that did its
    /// work. `None` if no connection did.
    #[must_use]
    pub fn completion_ns(&self) -> Option<u64> {
        let done = || self.transfers.iter().filter(|transfer| transfer.ok);
        let first = done().map(|transfer| transfer.started_ns).min()?;
        let last = done().map(|transfer| transfer.done_ns).max()?;
        u64::try_from(last.saturating_sub(first)).ok()
    }
}

/// The payload that arrived at the receiving end, by time bin (D-086, D-088): at the server for `tcp-bulk`, and for
/// `request-response` the responses at the client, each in the bin of the last byte of its response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Goodput {
    /// The width of a bin, `measure.series.bin`. The server counts in intervals of at most 10 ms and puts each interval into
    /// the bin that holds its start, so a bin is right to within one interval. The responses of a `request-response` workload
    /// are counted at the moment their last byte was read.
    pub bin_ns: u64,
    /// The payload bits per second that arrived in each bin, from the start of the measured phase to the last bin with data. The
    /// last bin is not scaled.
    pub bits_per_second: Vec<u64>,
    /// The payload that arrived divided by the time from the start of the first connection to the end of the last that was
    /// done: the goodput of the whole transfer, with the setup and the end in it. `None` if no connection was done.
    pub mean_bits_per_second: Option<u64>,
}

/// One TCP connection (D-086).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TransferInfo {
    /// The number of the connection in its workload, from 0.
    pub connection: u32,
    /// When the connect was started.
    pub started_ns: i64,
    /// When it was established.
    pub established_ns: i64,
    /// When it was done: for `tcp-bulk` when the server had read everything (the completion of the transfer), for
    /// `request-response` when the connection was closed. For a connection that failed, when it failed.
    pub done_ns: i64,
    /// The payload bytes the client sent.
    pub bytes_sent: u64,
    /// The payload bytes the client received (responses).
    pub bytes_received: u64,
    /// Whether the connection did its work.
    pub ok: bool,
}

impl TransferInfo {
    /// How long the connection took from the connect to its end, in nanoseconds.
    #[must_use]
    pub fn duration_ns(&self) -> u64 {
        u64::try_from(self.done_ns.saturating_sub(self.started_ns)).unwrap_or(0)
    }
}

/// The requests of a `request-response` workload and how long they took (PRB-12; D-086).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestInfo {
    /// The requests that were sent.
    pub sent: u64,
    /// The requests that were answered.
    pub answered: u64,
    /// The requests that failed: no answer in time, or the connection broke.
    pub failed: u64,
    /// From the moment a request was **due** to the last byte of its response: in a closed loop the moment it was sent, in an
    /// open loop the moment on the grid, so a request that waited for a busy connection has the wait in it. Answered requests only.
    pub latency: Option<crate::stats::DelaySummary>,
    /// From the moment a request was sent to the last byte of its response: the round trip of the network and the server, without
    /// the wait for a free connection.
    pub round_trip: Option<crate::stats::DelaySummary>,
    /// The longest a request was sent after it was due, in nanoseconds: the wait for a connection, or the lateness of the client.
    pub max_wait_ns: u64,
}

/// A capture of one interface (PRB-09, D-047, D-073). Times are in nanoseconds from the start of the measured phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CaptureResult {
    /// The link.
    pub link: String,
    /// The node whose interface it is.
    pub node: String,
    /// The name of the interface in the node.
    pub interface: String,
    /// The pcap file, relative to `result.json`.
    pub file: String,
    /// The frames in the file, both directions.
    pub packets: u64,
    /// The bytes of the frames (without the headers of the file).
    pub bytes: u64,
    /// The time of the first frame, if there is one.
    pub first_ns: Option<i64>,
    /// The time of the last frame.
    pub last_ns: Option<i64>,
    /// The real-time clock minus `CLOCK_MONOTONIC` at the start of the capture: the timestamps in the file are on the
    /// real-time clock, and this turns one into a time of the records (`time - offset - start of the measured phase`).
    pub realtime_minus_monotonic_ns: i64,
    /// The size of the receive buffer of the capture that the kernel gave, in bytes (`net.core.rmem_max` decides).
    pub receive_buffer_bytes: u64,
    /// The frames the interface counted (received and sent) while the capture ran, if its counters could be read.
    pub interface_frames: Option<u64>,
    /// How many of those frames are not in the file. `Some(0)` is a whole capture, `None` that it could not check itself.
    pub missing: Option<u64>,
}

/// What came of a program of your own (WRK-06, WRK-10, PRB-08; D-072). Times are in nanoseconds from the start of the
/// measured phase, the zero of `events`; a program is started before it, so its start is negative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProgramResult {
    /// The id in the scenario.
    pub id: String,
    /// The node it ran in.
    pub node: String,
    /// The program and its arguments, with `{node.addr}` replaced.
    pub run: Vec<String>,
    /// When the run started it.
    pub started_ns: i64,
    /// When the run found it ready: the start for a program with no `ready`, else the moment the condition held.
    pub ready_ns: i64,
    /// When it ended.
    pub ended_ns: i64,
    /// How it ended: `on-its-own` (before the run asked it to stop), `stopped` (after the signal of `shutdown`, within
    /// the time) or `killed` (it was still there after the time, and `KILL` ended it).
    pub ended: String,
    /// The exit code, if it returned one.
    pub exit_code: Option<i32>,
    /// The signal that ended it, if one did.
    pub exit_signal: Option<i32>,
    /// The file with its standard output, relative to `result.json` (the first megabyte).
    pub stdout_file: String,
    /// The file with its standard error.
    pub stderr_file: String,
    /// How the output was read: bytes in all, bytes past the limit of the file, and lines longer than the limit of a
    /// line (they were cut). Nothing that was dropped is hidden.
    pub output: ProgramOutput,
    /// Whether lines of standard output were dropped because the run could not read them fast enough (more than
    /// 10,000 between two looks). Metrics in them are lost, and the result says so.
    pub lines_dropped: bool,
    /// The metrics the program reported, by name (PRB-08).
    pub metrics: Vec<MetricSummary>,
    /// All the readings, one line each, relative to `result.json`; `None` if there was none.
    pub metrics_file: Option<String>,
    /// How many valid readings in all.
    pub metric_readings: u64,
    /// How many lines looked like metrics and were not valid.
    pub bad_metric_lines: u64,
    /// The first of them, each with the reason.
    pub bad_metric_examples: Vec<String>,
}

/// What was read from the output of a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProgramOutput {
    /// Bytes of standard output.
    pub stdout_bytes: u64,
    /// Bytes of standard output that are not in its file.
    pub stdout_dropped_bytes: u64,
    /// Bytes of standard error.
    pub stderr_bytes: u64,
    /// Bytes of standard error that are not in its file.
    pub stderr_dropped_bytes: u64,
    /// Lines of standard output longer than the limit of a line, which were cut.
    pub long_lines: u64,
}

/// What the readings of one metric came to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MetricSummary {
    /// The name.
    pub name: String,
    /// The unit, if a reading gave one.
    pub unit: Option<String>,
    /// How many readings.
    pub count: u64,
    /// The first value.
    pub first: f64,
    /// The last value.
    pub last: f64,
    /// The smallest.
    pub min: f64,
    /// The largest.
    pub max: f64,
}

impl EventRecord {
    /// A record of a `set` of the forward direction of the link `l`, for a test of something that judges records.
    #[must_use]
    pub fn new(event: usize, scheduled_ns: u64, started_ns: u64, applied_ns: u64) -> Self {
        Self {
            event,
            kind: "set".to_owned(),
            link: "l".to_owned(),
            direction: Some("forward".to_owned()),
            scheduled_ns,
            started_ns,
            applied_ns,
            packet: None,
            held_ns: None,
            from_model: false,
            live: false,
        }
    }
}

/// One step of the timeline as the run did it: which event of the file it comes from, when it was due and when the
/// backend had done it. The times are nanoseconds from the start of the measured phase, the moment at which the
/// workloads start. An event on `both` is two steps, one for each direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EventRecord {
    /// The place of the event in `timeline` of the scenario, as the run made it: an event that repeats is its copies one after the
    /// other, and the events of the models come last (D-090).
    pub event: usize,
    /// What the step was: `set`, `outage-start`, `outage-end`, `down` or `up`.
    pub kind: String,
    /// The link.
    pub link: String,
    /// The direction (`forward`, `reverse`), or `null` for a step of the whole link (`down`, `up`).
    pub direction: Option<String>,
    /// When the file says it happens.
    pub scheduled_ns: u64,
    /// When the run asked the backend to do it.
    pub started_ns: u64,
    /// When the backend had done it. For a step done by the clock the difference to `scheduled_ns` is how late the
    /// step was: the wait for the clock, the time of the change itself, and what the machine added; a packet
    /// that left in between met the old state. A step done at a barrier (`packet` is set) is done when the sender
    /// has sent the packet before its moment, so it can be done up to one gap before `scheduled_ns`.
    pub applied_ns: u64,
    /// For a step done at a barrier of the workload that sends into the direction (D-038): the number of packets
    /// that met the old state, which is the number of the first packet that met the new one. `null` for a step done by
    /// the clock.
    #[serde(default)]
    pub packet: Option<u64>,
    /// For a step done at a barrier: how long the sender waited at the slot of that packet for the change to be done,
    /// in nanoseconds; 0 if the change was done by then. The packet left that much late.
    #[serde(default)]
    pub held_ns: Option<u64>,
    /// Whether a model of the link made the event (D-090) and not the file: `event` is then its place in the timeline that the
    /// run made from the models, after the events of the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_model: bool,
    /// Whether test code made the step while the run went (D-096) and not the file: `event` is then its place among the
    /// live changes, and `scheduled_ns` the moment the code asked. The recorded scenario holds it as an event at the slot of
    /// the packet it met (`packet`), so a replay does it there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub live: bool,
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
    /// The parameters of the scenario and the values the run was made with (D-093); empty for a scenario without parameters.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: std::collections::BTreeMap<String, String>,
    /// The profile of each link that has one (D-095): the id of the link and the profile. The values of the profile are in the scenario
    /// the run was made with; this is the record of where they came from.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub profiles: std::collections::BTreeMap<String, String>,
    /// The version of the profile library the profiles were resolved with; none if no link has a profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_library: Option<u32>,
    /// The links that were declared live (D-096): the queues of these links were built so that any state fits, and code could change
    /// them while the run went. A replay builds them the same way. Empty for a run that was not changed from code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_links: Vec<String>,
}

impl ScenarioInfo {
    /// The profiles as `link=profile, link=profile` in the order of the link names, with the version of the library, or `None`
    /// for a scenario without profiles.
    #[must_use]
    pub fn profiles_line(&self) -> Option<String> {
        (!self.profiles.is_empty()).then(|| {
            let list = self
                .profiles
                .iter()
                .map(|(link, profile)| format!("{link}={profile}"))
                .collect::<Vec<_>>()
                .join(", ");
            match self.profile_library {
                Some(version) => format!("{list} (library {version})"),
                None => list,
            }
        })
    }

    /// The parameters as `name=value, name=value` in the order of their names, or `None` for a scenario without parameters.
    #[must_use]
    pub fn params_line(&self) -> Option<String> {
        (!self.params.is_empty()).then(|| {
            self.params
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join(", ")
        })
    }
}

/// What a result records of the scenario: the name, the hash, the parameters and the profiles with the version of the library.
pub(crate) fn scenario_info(
    name: String,
    hash: String,
    scenario: &netscenario_model::Scenario,
) -> ScenarioInfo {
    ScenarioInfo {
        name,
        hash,
        params: scenario.params.clone(),
        profiles: scenario
            .topology
            .links
            .iter()
            .filter_map(|link| Some((link.id.to_string(), link.profile.clone()?)))
            .collect(),
        profile_library: scenario.profile_library,
        live_links: scenario
            .live_links
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
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
    /// Why the run is not reproducible, one sentence for each reason; empty when `loss_reproducible` is `true`.
    /// A backend that cannot apply a seed, a queue that counted a packet the workloads did not send, and a
    /// scenario with corruption (where `netem` flips a bit is not seeded), and a scenario with an outage (its edges
    /// are placed by the clock, D-064) are the reasons.
    #[serde(default)]
    pub not_reproducible_because: Vec<String>,
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

/// Where the packets of one direction of a link went: lost in the queue of the link, dropped by the receiving
/// node because they arrived damaged, or delivered to the receiver (D-040). The three add up to what was sent,
/// so a packet that vanished is accounted for and a corrupted packet is not mistaken for a lost one.
///
/// The counts are those of the interface and of the node, so they hold for all the workloads in the
/// direction together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DirectionTally {
    /// The link.
    pub link: String,
    /// `forward` (from the first node of `between` to the second) or `reverse`.
    pub direction: String,
    /// The packets that the workloads put into the direction (those the kernel took).
    pub sent: u64,
    /// The frames that arrived at the interface of the receiving node, counted by the interface.
    pub reached_node: u64,
    /// The datagrams that reached the sockets of the receivers: the packets of the agent and the others.
    pub delivered: u64,
    /// The packets that the queue of the link dropped (the loss, a queue that was full): `sent` minus
    /// `reached_node`. `None` where that cannot be told: in a direction that duplicates (the frames include the
    /// copies), or when more frames arrived than were sent.
    pub lost_in_queue: Option<u64>,
    /// The frames that arrived and that the node dropped because a bit in them was flipped (a bad checksum,
    /// an address that is not the node's): `reached_node` minus `delivered` minus `receive_buffer_drops`.
    pub dropped_corrupted: u64,
    /// The datagrams that the node dropped because the receive buffer of a socket was full. This is a fault of
    /// the receiver and not of the network.
    pub receive_buffer_drops: u64,
}

/// The queue of a direction as the run built it and what the kernel counted in it (PRB-10, D-082).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QueueInfo {
    /// The link.
    pub link: String,
    /// `forward` or `reverse`.
    pub direction: String,
    /// How the link was built: `fabric` (a link with a queue goes through the fabric of the run, D-079).
    pub build: String,
    /// The discipline of the scenario: `fifo`, `codel`, `fq-codel` or `pie`.
    pub discipline: String,
    /// The discipline as the kernel names it (`pfifo`, `bfifo`, `codel`, `fq_codel`, `pie`).
    pub kind: String,
    /// The packets that left the queue.
    pub packets: u64,
    /// The bytes of those packets.
    pub bytes: u64,
    /// The packets the queue dropped, by its limit or its discipline.
    pub drops: u64,
    /// The packets over the limit.
    pub overlimits: u64,
    /// The packets put back for another try.
    pub requeues: u64,
    /// The packets waiting at the end of the run.
    pub backlog_packets: u64,
    /// The bytes waiting at the end of the run.
    pub backlog_bytes: u64,
    /// The counters special to the discipline, by the name of the kernel (`ecn_mark`, `drop_overlimit`, ...).
    #[serde(default)]
    pub details: std::collections::BTreeMap<String, u64>,
    /// The counters once a second while the workloads went (PRB-10), from the start of the measured phase. Empty when a sender
    /// may wait for a step of the timeline (the loop that answers it is not held up for a look at the queue), and for a run
    /// shorter than a second.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<QueueSample>,
}

/// The counters of a queue at a moment of the run (PRB-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QueueSample {
    /// When, in nanoseconds from the start of the measured phase.
    pub t_ns: u64,
    /// The packets that left the queue until then.
    pub packets: u64,
    /// The packets the queue dropped until then.
    pub drops: u64,
    /// The packets waiting in the queue then.
    pub backlog_packets: u64,
    /// The bytes waiting in the queue then.
    pub backlog_bytes: u64,
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

impl AcceptedRestriction {
    /// A restriction of the feature with the id `feature`.
    #[must_use]
    pub fn new(feature: &str, restriction: &str) -> Self {
        Self {
            feature: feature.to_owned(),
            restriction: restriction.to_owned(),
        }
    }
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

/// A node of the network: its role and the address of its own (D-078).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NodeInfo {
    /// The node.
    pub node: String,
    /// `host` or `router`.
    pub role: String,
    /// The address on its loopback: the IPv4 one where the network has it, else the IPv6 one.
    pub address: String,
    /// The IPv6 address on its loopback, when the network has both families (D-081).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_v6: Option<String>,
}

/// A route of a node (D-078).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RouteInfo {
    /// The node that has the route.
    pub node: String,
    /// The destination: a node, or `default`.
    pub to: String,
    /// The link the packets leave by.
    pub link: String,
    /// The node at the other end of the link.
    pub next_hop: String,
    /// The preference: the smaller is used first.
    pub metric: u32,
    /// The share among the routes of the same metric.
    pub weight: u32,
    /// Whether the scenario lists it (`true`) or the tool computed it.
    pub given: bool,
}

/// The nodes and the links that the packets of a workload pass (D-078).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PathInfo {
    /// The nodes from the sender to the receiver, both included.
    pub nodes: Vec<String>,
    /// The links between them: one fewer than the nodes.
    pub links: Vec<String>,
    /// Whether a node on the way shares the packets among several routes of the same metric (multipath): the path is then the
    /// first of them by the order of the links.
    pub shared: bool,
}

fn one() -> u64 {
    1
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_one(value: &u64) -> bool {
    *value == 1
}

/// The times of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Timing {
    /// The length of the measured phase in the scenario, in nanoseconds.
    pub duration_ns: u64,
    /// How long the run waited after the last packet was sent for the packets still in the network. The wait that
    /// was set, or, without one, the time the queues of the links needed to empty and 300 ms more (D-067). A packet
    /// that needs longer counts as lost.
    pub settle_ns: u64,
    /// If the run was interrupted while the workloads ran (D-066): how long after the start of the measured phase the
    /// senders were told to stop, in nanoseconds. The workloads ended there, the steps of the timeline after it were
    /// not done, and the result is of the part that ran. `null` for a run that went to its end.
    #[serde(default)]
    pub interrupted_ns: Option<u64>,
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
    /// The sequence numbers of the packets that the kernel refused, as runs `[first, last]`, both included, in
    /// ascending order (D-064). A refused packet was never sent: it is not in `sent` and not in `lost`. A link that
    /// is down refuses a run of them for as long as it lasts.
    #[serde(default)]
    pub refused: Vec<[u64; 2]>,
}

/// Where the series of a workload is and how it is cut (D-070). The series itself is in a file of its own,
/// `series-<id>.jsonl`, one line for each bin (see [`series`](crate::series)), so that this result stays small.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SeriesInfo {
    /// The width of a bin, in nanoseconds.
    pub bin_ns: u64,
    /// How many bins (lines) the file has.
    pub bins: u64,
    /// The file, relative to the result.
    pub file: String,
}

/// The trace a `trace-replay` workload replayed (D-085).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TraceInfo {
    /// The file the trace was read from, as the scenario names it.
    pub file: Option<String>,
    /// The SHA-256 of that file.
    pub sha256: Option<String>,
    /// How many packets the trace has (the ones that the window of the workload cuts off are among them).
    pub packets: u64,
    /// The packets of a capture that were raised to the smallest payload (0 for a CSV file).
    pub padded: u64,
    /// The smallest payload in the trace, in bytes.
    pub smallest_bytes: u64,
    /// The largest payload in the trace, in bytes.
    pub largest_bytes: u64,
    /// The moment of the last packet, in nanoseconds from the start of the workload.
    pub span_ns: u64,
}

/// The packets of a workload with `echo: true` that came back to the node of the sender (PRB-12, D-087).
///
/// The far node sends every packet it recorded back, unchanged, through the reverse direction, and a receiver in the sending
/// node records it with the send time of the first trip. So `delay` is the **round trip** of a packet, from the moment it was
/// sent to the moment its echo arrived, `received` is the number of packets that came back, and `lost` the packets that did not,
/// whether the way there or the way back lost them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EchoResult {
    /// How many packets the far node sent back.
    pub echoed: u64,
    /// How many packets the system of the far node refused to send back.
    pub echo_errors: u64,
    /// How many datagrams reached the collector that were not packets of the agent.
    pub foreign_datagrams: u64,
    /// The file of per-packet records of the collector (binary, see `docs/AGENT.md`), relative to the result.
    pub records_file: String,
    /// The packets that came back: `sent` is the number of the workload, `received` the packets that came back, and `delay` their
    /// round trip.
    #[serde(flatten)]
    pub stats: FlowStats,
}

/// The result of one workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WorkloadResult {
    /// The id in the scenario.
    pub id: String,
    /// The kind: `udp-cbr`, `udp-poisson`, `udp-onoff` or `udp-bursty`.
    pub kind: String,
    /// The sending node.
    pub from: String,
    /// The receiving node.
    pub to: String,
    /// The address the packets were sent from (the address of `from`).
    pub from_address: String,
    /// The address the packets were sent to, and the port.
    pub to_address: String,
    /// The nodes and links the packets pass (D-078). Absent in a result of release 0.2 or earlier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathInfo>,
    /// The frames a packet makes on the first link of its path: 1 if it fits the MTU, else the fragments the sender cuts it
    /// into (D-081). A queue sees frames, and a lost frame loses the packet. 1 in a result of release 0.2 or earlier.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub frames_per_packet: u64,
    /// The rate of the payload that the schedule is meant to give: the rate of the scenario for `udp-cbr` and `udp-poisson`
    /// (for the second it is the mean), and the mean over the phases or the bursts for `udp-onoff` and `udp-bursty`, rounded
    /// down.
    pub rate_bits_per_second: u64,
    /// The payload of a packet in bytes; for a trace, the largest.
    pub packet_size_bytes: u64,
    /// The trace of a `trace-replay` workload (D-085). Absent for the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<TraceInfo>,
    /// The packets that came back, with their round trip, for a workload with `echo: true` (D-087). Absent for the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echo: Option<EchoResult>,
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
    /// How many datagrams reached the receiver damaged, in a workload with `udp-checksum: false`: the CRC-32 of
    /// the packet did not match, or its header was not one. They are not in `received` and not in the delays,
    /// and they are in `lost`, because a damaged packet is not an intact one; `lost` minus `corrupted` is what
    /// never arrived. With the checksum on the kernel drops a damaged packet before the receiver sees it, so
    /// this is always 0 there, and the packet is told apart from the others only per node, in `directions`
    /// (`dropped_corrupted`).
    #[serde(default)]
    pub corrupted: u64,
    /// How many packets the receiving node saw on the wire and did not deliver intact, with their sequence
    /// numbers readable (D-040): the damaged packets that can be named. `None` if the direction does not corrupt,
    /// or if they could not be named (`corrupted_seq_unavailable` says why). It is a count of sequence numbers
    /// read from frames, so a bit flipped in a sequence number can name a neighbour, and a frame whose headers
    /// or sequence are damaged is not named at all; `dropped_corrupted` of `directions` is the exact count for the
    /// node. With a loss in the same direction a name can also be a packet that the queue dropped.
    #[serde(default)]
    pub corrupted_named: Option<u64>,
    /// The first 1,000 of those sequence numbers, in ascending order. The export of every packet (`--packets`)
    /// has `corrupted` on all of them.
    #[serde(default)]
    pub corrupted_seq: Vec<u64>,
    /// Why the damaged packets of a direction that corrupts could not be named, if they could not: the tap of
    /// the receiver needs `CAP_NET_RAW` in the node, and it is not trusted where the node dropped datagrams for
    /// a full receive buffer.
    #[serde(default)]
    pub corrupted_seq_unavailable: Option<String>,
    /// The file of per-packet records of the receiver (binary, see `docs/AGENT.md`), relative to the result.
    pub records_file: String,
    /// The JSON Lines file with one line for each packet, if it was asked for, relative to the result.
    pub packets_file: Option<String>,
    /// The series by time bin: where the file is and how it is cut (D-070). `None` in a result of an older run.
    #[serde(default)]
    pub series: Option<SeriesInfo>,
    /// The time to recover after each event of the timeline that concerns this workload (D-070); empty without a
    /// timeline, and in a result of an older run.
    #[serde(default)]
    pub recoveries: Vec<crate::recovery::Recovery>,
}

impl WorkloadResult {
    /// The payload of the packets that arrived, in bytes. A result of release 0.2 or earlier has no sum, and every packet had the
    /// size `packet_size_bytes`.
    #[must_use]
    pub fn payload_received_bytes(&self) -> u64 {
        if self.stats.received_bytes > 0 || self.stats.received == 0 {
            self.stats.received_bytes
        } else {
            self.stats.received * self.packet_size_bytes
        }
    }
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
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::stats::{FlowStats, Throughput};

    /// A small result for tests of code that reads results.
    #[allow(clippy::too_many_lines)] // a fixture that fills every field
    pub(crate) fn example() -> RunResult {
        RunResult {
            format: RESULT_FORMAT.to_owned(),
            version: RESULT_VERSION,
            scenario: ScenarioInfo {
                name: "t".to_owned(),
                hash: "sha256:00".to_owned(),
                params: std::collections::BTreeMap::new(),
                profiles: std::collections::BTreeMap::new(),
                profile_library: None,
                live_links: Vec::new(),
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
                not_reproducible_because: Vec::new(),
            },
            accepted_restrictions: Vec::new(),
            addresses: Vec::new(),
            nodes: Vec::new(),
            routes: Vec::new(),
            timing: Timing {
                duration_ns: 1,
                settle_ns: 2,
                interrupted_ns: None,
            },
            workloads: vec![WorkloadResult {
                id: "w".to_owned(),
                kind: "udp-cbr".to_owned(),
                from: "a".to_owned(),
                to: "b".to_owned(),
                from_address: "10.100.0.1".to_owned(),
                to_address: "10.100.0.2:5000".to_owned(),
                path: None,
                frames_per_packet: 1,
                rate_bits_per_second: 1_000_000,
                packet_size_bytes: 1000,
                trace: None,
                echo: None,
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
                    reordering: None,
                    jitter: None,
                    loss_runs: crate::patterns::LossRuns::default(),
                    throughput: Throughput {
                        bin_ns: 1_000_000_000,
                        bits_per_second: vec![8000.0],
                    },
                    received_bytes: 1000,
                },
                sender: SenderInfo {
                    max_lateness_ns: 3,
                    send_errors: 2,
                    first_error: Some("Network is unreachable".to_owned()),
                    refused: vec![[5, 6]],
                },
                foreign_datagrams: 0,
                corrupted: 0,
                corrupted_named: None,
                corrupted_seq: Vec::new(),
                corrupted_seq_unavailable: None,
                records_file: "flow-0.records".to_owned(),
                packets_file: None,
                series: Some(SeriesInfo {
                    bin_ns: 1_000_000_000,
                    bins: 3,
                    file: "series-media.jsonl".to_owned(),
                }),
                recoveries: Vec::new(),
            }],
            events: Vec::new(),
            notes: vec!["a note".to_owned()],
            programs: Vec::new(),
            captures: Vec::new(),
            tcp: Vec::new(),
            reproducibility: Reproducibility::exact(),
            expect: None,
            queues: Vec::new(),
            directions: vec![DirectionTally {
                link: "l".to_owned(),
                direction: "forward".to_owned(),
                sent: 2,
                reached_node: 1,
                delivered: 1,
                lost_in_queue: Some(1),
                dropped_corrupted: 0,
                receive_buffer_drops: 0,
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
