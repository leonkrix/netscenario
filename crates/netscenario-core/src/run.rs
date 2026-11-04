//! The run: from a built network to a result (steps 5 to 9 of the life of a run, `docs/ARCHITECTURE.md`).
//!
//! The order, for every workload at once: start the receiver in the node of `to` and wait until it says it is
//! ready; fix one moment a little ahead and start every sender with it, so that the workloads start together;
//! wait until the senders are done; wait for the packets that are still in the network; tell the receivers to
//! stop; read their records; tear the network down; compute the statistics and write the result.
//!
//! The network is torn down whatever happens. If the run fails, the first error is returned.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use netscenario_agent::clock::monotonic_ns;
use netscenario_agent::events::{Event, ReceiveSummary, SendSummary};
use netscenario_agent::records;
use netscenario_agent::schedule::Schedule;
use netscenario_model::{Generator, Impairments, Scenario, Workload};

use crate::backend::{
    Backend, Direction, LinkHandle, NodeCounts, NodeHandle, Program, ProgramHandle,
};
use crate::captures::Captures;
use crate::error::Error;
use crate::export::write_packets_refused;
use crate::machine::collect;
use crate::observe::{CancelToken, RunEvent, RunObserver};
use crate::programs::Programs;
use crate::provision::{Provisioned, impairments_of, provision};
use crate::recovery::{self, Basis, Recovery, RecoveryOptions};
use crate::result::{
    AcceptedRestriction, AddressInfo, CaptureResult, DirectionTally, EventRecord, NodeInfo,
    PathInfo, ProgramResult, QueueCheck, QueueInfo, RESULT_FORMAT, RESULT_VERSION, RouteInfo,
    RunResult, ScenarioInfo, SeedInfo, SenderInfo, SeriesInfo, Timing, TraceInfo, WorkloadResult,
};
use crate::series::{self, DEFAULT_BIN_NS};
use crate::stats::analyse_refused;
use crate::timeline;
use crate::{barriers, drive, traffic};

/// The port of the first workload; the next uses the next port.
const BASE_PORT: u16 = 5000;

/// How many sequence numbers of damaged packets `corrupted_seq` of a workload lists; the count is complete, and the
/// export of every packet (`--packets`) names them all.
const MAX_LISTED_SEQUENCES: usize = 1000;

/// How far ahead of the moment all the senders have to start, so that they are all ready when it comes.
const START_MARGIN: Duration = Duration::from_millis(500);

/// How long a program may take to end after it should have, before the run gives up on it.
pub(crate) const END_MARGIN: Duration = Duration::from_secs(30);

/// The settings of a run.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunOptions {
    /// The agent program (`netscenario-agent`), started by path in the nodes.
    pub agent: PathBuf,
    /// The directory for the result and the files that belong to it. It is created if it does not exist, and
    /// the agents in the nodes write into it, so it must be a directory that they can write to.
    pub out_dir: PathBuf,
    /// How long to wait after the last packet was sent for the packets still in the network. Without it the run
    /// waits until the queues of the links are empty and then 300 ms for the receivers to read (D-067), at most two
    /// minutes; a backend that cannot count the packets in its queues gets one second and twice the longest delay of
    /// the scenario with four times its jitter. With it, that long: a packet that needs longer counts as lost.
    pub settle: Option<Duration>,
    /// The most address space an agent may use, in bytes (SAF-02). The default is 1 GiB.
    pub agent_memory_limit: u64,
    /// How long a receiver may take to say that it is ready.
    pub ready_timeout: Duration,
    /// Told how the run is going (see [`RunEvent`]). Without it nobody is told.
    pub observer: Option<Arc<dyn RunObserver>>,
    /// Cancel it to stop the run: the network is torn down and the run returns [`Error::Cancelled`]. The run
    /// looks at it at least every 200 milliseconds.
    pub cancel: CancelToken,
    /// Whether a step of the timeline is done at a barrier of the workload that sends into its direction, so that the
    /// same seed gives the same lost packets (D-038). `false` does every step by the clock, as 0.2 first did; the
    /// result then says that the lost packets are not promised to repeat, if a step changes what is drawn. It
    /// exists for the tests that show what the barrier is for. Default `true`.
    pub barriers: bool,
    /// What a cancel does while the workloads run (D-066). `false`, the default: the run ends with
    /// [`Error::Cancelled`] as before. `true`: the senders are told to stop, the run waits for the packets that are
    /// in the network, collects the records and returns a result of the part that ran, with
    /// `timing.interrupted_ns` set. A cancel before the workloads begin, or after they ended, is
    /// [`Error::Cancelled`] either way: there is nothing to keep, or the result is whole.
    pub partial_on_cancel: bool,
    /// The directory the programs of your own run in, and that a relative path of `ready: { file: ... }` is counted from
    /// (D-041): the directory of the scenario file. Without it, the directory of the supervisor.
    pub working_dir: Option<PathBuf>,
}

impl std::fmt::Debug for RunOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunOptions")
            .field("agent", &self.agent)
            .field("out_dir", &self.out_dir)
            .field("settle", &self.settle)
            .field("agent_memory_limit", &self.agent_memory_limit)
            .field("ready_timeout", &self.ready_timeout)
            .field("observer", &self.observer.as_ref().map(|_| "<observer>"))
            .field("cancel", &self.cancel)
            .field("barriers", &self.barriers)
            .field("partial_on_cancel", &self.partial_on_cancel)
            .field("working_dir", &self.working_dir)
            .finish()
    }
}

impl RunOptions {
    /// The defaults for the agent at `agent` and a result in `out_dir`.
    #[must_use]
    pub fn new(agent: impl Into<PathBuf>, out_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent: agent.into(),
            out_dir: out_dir.into(),
            settle: None,
            agent_memory_limit: 1 << 30,
            ready_timeout: Duration::from_secs(10),
            observer: None,
            cancel: CancelToken::new(),
            barriers: true,
            partial_on_cancel: false,
            working_dir: None,
        }
    }

    /// The same options, with an observer.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn RunObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// The same options, with a token that cancels the run.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// The same options, with a cancel while the workloads run giving the result of the part that ran
    /// ([`partial_on_cancel`](RunOptions::partial_on_cancel)).
    #[must_use]
    pub fn with_partial_on_cancel(mut self) -> Self {
        self.partial_on_cancel = true;
        self
    }
}

/// How often the progress of the workloads is reported.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// While the workloads run: when they started, how long they last, and when the next report is due.
struct Workloads {
    begin: Instant,
    total: Duration,
    next_report: Instant,
}

/// What a run looks at between its steps: whether it was cancelled, who is watching, and the progress.
pub(crate) struct Control<'a> {
    cancel: &'a CancelToken,
    observer: Option<&'a dyn RunObserver>,
    workloads: RefCell<Option<Workloads>>,
    /// A cancel while the workloads run ends them and keeps the result (D-066).
    partial: bool,
    /// Whether the workloads are running.
    measuring: Cell<bool>,
    /// Whether a cancel was taken as an interruption: from then on a cancel is not an error any more.
    interrupted: Cell<bool>,
}

impl<'a> Control<'a> {
    fn new(options: &'a RunOptions) -> Self {
        Self {
            cancel: &options.cancel,
            observer: options.observer.as_deref(),
            workloads: RefCell::new(None),
            partial: options.partial_on_cancel,
            measuring: Cell::new(false),
            interrupted: Cell::new(false),
        }
    }

    /// Whether the run was cancelled while the workloads run and it keeps what it has: `true` once, at the first time
    /// it is asked after the cancel, and the observer is told. The loop that runs the workloads asks it.
    pub(crate) fn take_interruption(&self) -> bool {
        if self.partial
            && self.measuring.get()
            && !self.interrupted.get()
            && self.cancel.is_cancelled()
        {
            self.interrupted.set(true);
            self.emit(&RunEvent::Interrupted);
            return true;
        }
        false
    }

    pub(crate) fn emit(&self, event: &RunEvent) {
        if let Some(observer) = self.observer {
            observer.event(event);
        }
    }

    /// Called in every loop that waits and between the steps: stops the run if it was cancelled, and reports
    /// the progress of the workloads once a second.
    pub(crate) fn checkpoint(&self) -> Result<(), Error> {
        // A cancel that is taken as an interruption is not an error: the run goes on to its end with what it has
        if self.cancel.is_cancelled()
            && !(self.partial && (self.measuring.get() || self.interrupted.get()))
        {
            return Err(Error::Cancelled);
        }
        let report = {
            let mut workloads = self.workloads.borrow_mut();
            workloads.as_mut().and_then(|w| {
                let now = Instant::now();
                (now >= w.next_report)
                    .then(|| {
                        while w.next_report <= now {
                            w.next_report += PROGRESS_EVERY;
                        }
                        // The report of the whole is made once, when the workloads are done; a run that holds on for its programs
                        // does not say it twice
                        let elapsed = now.saturating_duration_since(w.begin);
                        (elapsed < w.total).then_some(RunEvent::Progress {
                            elapsed,
                            total: w.total,
                        })
                    })
                    .flatten()
            })
        };
        if let Some(event) = report {
            self.emit(&event);
        }
        Ok(())
    }

    /// The workloads begin at `begin` and last `total`.
    fn workloads_started(&self, begin: Instant, total: Duration) {
        self.measuring.set(true);
        *self.workloads.borrow_mut() = Some(Workloads {
            begin,
            total,
            next_report: begin + PROGRESS_EVERY,
        });
        self.emit(&RunEvent::Started { duration: total });
    }

    /// The workloads are over: a last report at the full length.
    fn workloads_done(&self) {
        self.measuring.set(false);
        let done = self.workloads.borrow_mut().take();
        if let Some(w) = done {
            self.emit(&RunEvent::Progress {
                elapsed: w.total,
                total: w.total,
            });
        }
    }
}

pub(crate) fn failure(action: impl Into<String>, message: impl Into<String>) -> Error {
    Error::Run {
        action: action.into(),
        message: message.into(),
    }
}

/// What one workload needs to be started.
struct Flow<'a> {
    index: usize,
    workload: Generator<'a>,
    /// When its packets leave.
    schedule: Schedule,
    from_node: NodeHandle,
    to_node: NodeHandle,
    to_address: String,
    /// The nodes and links its packets pass (D-078).
    path: netscenario_model::Path,
    /// The frames a packet makes on the first link: more than one if the sender cuts it into fragments (D-081). A trace has packets
    /// of different sizes, so it counts them one by one (D-085).
    frames: Frames,
    /// Why the damaged packets of this flow cannot be named by sequence number, where the path corrupts them (D-081): the tap
    /// of the receiver reads IPv4 frames, and a fragment other than the first shows no sequence number.
    wire_unavailable: Option<&'static str>,
    records_file: PathBuf,
    /// Where the tap of the receiver writes the packets it sees on the wire, for a workload whose direction
    /// corrupts packets (they are named by sequence number, D-040).
    wire_file: Option<PathBuf>,
    start: Duration,
    duration: Duration,
}

/// A receiver that said `ready`, and the reason why its tap could not be opened, if it could not.
struct Started {
    program: ProgramHandle,
    wire_error: Option<String>,
}

/// What came of naming the damaged packets of a workload.
enum Naming {
    /// The direction does not corrupt, so there is nothing to name.
    NotAsked,
    /// It was asked for and cannot be said, and why.
    Unavailable(String),
    /// The sequence numbers that were seen on the wire of the receiving node and not delivered intact.
    Named(Vec<u64>),
}

/// What came back from one workload.
struct Measured {
    sent: SendSummary,
    received: ReceiveSummary,
    records: records::File,
    naming: Naming,
}

/// What the phases of a run found out besides the workloads.
struct Measurement {
    measured: Vec<Measured>,
    settle: Duration,
    queue_checks: Vec<QueueCheck>,
    directions: Vec<DirectionTally>,
    queues: Vec<QueueInfo>,
    events: Vec<EventRecord>,
    notes: Vec<String>,
    /// When the workloads were told to stop, if the run was interrupted (D-066).
    interrupted_ns: Option<u64>,
    /// The start of the measured phase on the clock of the records.
    begin_ns: u64,
    /// Why the steps of the timeline may not give the same lost packets for the same seed (see [`barriers::plan`]).
    clock_reasons: Vec<String>,
    /// What came of the programs of your own.
    programs: Vec<ProgramResult>,
    /// What came of the captures of the interfaces.
    captures: Vec<CaptureResult>,
}

/// A scenario that refers to a file must have it read before it runs (D-083): the table is made when the scenario is loaded.
fn check_files_are_read(scenario: &Scenario) -> Result<(), Error> {
    let unread = scenario.unresolved_files();
    if unread.is_empty() {
        return Ok(());
    }
    Err(Error::InvalidScenario(
        unread
            .iter()
            .map(|file| {
                netscenario_model::Problem::new(format!(
                    "the scenario refers to the file '{file}', which has not been read"
                ))
                .with_help("read the files with `resolve_files` after `Scenario::from_yaml`; the command line and the Python package do both")
            })
            .collect(),
    ))
}

/// Runs `scenario` with `backend` and returns the result, which is also written to `result.json` in
/// `options.out_dir`.
///
/// # Errors
///
/// - the errors of [`provision`] (the scenario is refused, or the network cannot be built),
/// - [`Error::Run`] if an agent does not start, does not answer in time or ends badly, or a file cannot be
///   written,
/// - [`Error::Backend`] if the backend fails while the run is going,
/// - [`Error::Cancelled`] if `options.cancel` was cancelled; the network is torn down all the same.
pub fn run(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    options: &RunOptions,
) -> Result<RunResult, Error> {
    let started = SystemTime::now();
    let control = Control::new(options);
    // A run that was cancelled before it began builds nothing
    control.checkpoint()?;
    std::fs::create_dir_all(&options.out_dir).map_err(|error| {
        failure(
            "create the directory for the result",
            format!("{}: {error}", options.out_dir.display()),
        )
    })?;
    check_files_are_read(scenario)?;
    let provisioned = provision(backend, scenario)?;
    control.emit(&RunEvent::Provisioned {
        nodes: provisioned.nodes.len(),
        links: provisioned.links.len(),
    });
    let measured = measure(backend, scenario, options, &provisioned, &control);
    control.emit(&RunEvent::TearingDown);
    let torn_down = backend.teardown();
    let Measurement {
        measured,
        settle,
        queue_checks,
        directions,
        queues,
        events,
        notes,
        interrupted_ns,
        begin_ns,
        clock_reasons,
        programs,
        captures,
    } = measured?;
    torn_down?;

    let timeline = Timeline {
        begin_ns,
        events: &events,
    };
    let workloads = workload_results(scenario, &measured, &provisioned, options, &timeline)?;
    let reasons = not_reproducible_because(scenario, &provisioned, &queue_checks, clock_reasons);
    let result = RunResult {
        format: RESULT_FORMAT.to_owned(),
        version: RESULT_VERSION,
        scenario: ScenarioInfo {
            name: scenario.name.to_string(),
            hash: crate::replay::scenario_hash(scenario),
        },
        provenance: collect(&backend.describe(), started),
        seed: SeedInfo {
            value: provisioned.seed,
            drawn: provisioned.seed_drawn,
            loss_reproducible: reasons.is_empty(),
            queue_checks,
            not_reproducible_because: reasons,
        },
        accepted_restrictions: provisioned
            .accepted
            .iter()
            .map(|accepted| AcceptedRestriction {
                feature: accepted.feature.id().to_owned(),
                restriction: accepted.restriction.clone(),
            })
            .collect(),
        addresses: provisioned
            .addresses
            .iter()
            .map(|a| AddressInfo {
                node: a.node.to_string(),
                link: a.link.to_string(),
                address: a.address.to_string(),
            })
            .collect(),
        nodes: node_infos(scenario, &provisioned),
        routes: route_infos(scenario),
        timing: Timing {
            duration_ns: nanos(Duration::from(scenario.duration)),
            settle_ns: nanos(settle),
            interrupted_ns,
        },
        workloads,
        directions,
        queues,
        events,
        notes,
        programs,
        captures,
    };
    // The scenario as it ran, so that the run can be replayed (ENV-03, D-075): it is written first, so that a directory with a
    // result always has it
    let scenario_path = options.out_dir.join(crate::replay::SCENARIO_FILE);
    std::fs::write(&scenario_path, crate::replay::scenario_json(scenario)).map_err(|error| {
        failure(
            "write the scenario",
            format!("{}: {error}", scenario_path.display()),
        )
    })?;
    let path = options.out_dir.join("result.json");
    std::fs::write(&path, result.to_json())
        .map_err(|error| failure("write the result", format!("{}: {error}", path.display())))?;
    control.emit(&RunEvent::Finished);
    Ok(result)
}

/// The nodes of the network with their roles and the addresses the workloads use for them (D-078).
fn node_infos(scenario: &Scenario, provisioned: &Provisioned) -> Vec<NodeInfo> {
    scenario
        .topology
        .nodes
        .iter()
        .zip(&provisioned.node_addresses)
        .map(|(node, (_, ips))| NodeInfo {
            node: node.id.to_string(),
            role: node.role.as_str().to_owned(),
            address: ips.primary().map(|a| a.to_string()).unwrap_or_default(),
            address_v6: ips.v4.and(ips.v6).map(|a| a.to_string()),
        })
        .collect()
}

/// The routes the run used, the given ones and the ones it computed (D-078).
fn route_infos(scenario: &Scenario) -> Vec<RouteInfo> {
    scenario
        .topology
        .routes()
        .entries
        .iter()
        .map(|entry| RouteInfo {
            node: entry.node.to_string(),
            to: entry.to.to_string(),
            link: entry.link.to_string(),
            next_hop: entry.next_hop.to_string(),
            metric: entry.metric,
            weight: entry.weight,
            given: entry.given,
        })
        .collect()
}

/// Why the same seed may not give the same lost packets in this run, one sentence for each reason. The queues have
/// counted the packets that drew a number for the loss: if one saw more or fewer than the workloads sent, the
/// pattern of lost packets is not the one of this seed (ENV-07).
/// A program of your own may send packets into the queue of a direction that draws random numbers. The run does not
/// count or order them, so which packet draws which number, and so which packets are lost, can differ from run to run with
/// the same seed (D-072), as it does with two workloads in one direction (D-065).
fn programs_into_random_queues(scenario: &Scenario) -> Vec<String> {
    let mut reasons = Vec::new();
    for command in scenario.workloads.iter().filter_map(Workload::as_command) {
        for (index, link) in scenario.topology.links.iter().enumerate() {
            for (direction, word, from) in [
                (Direction::Forward, "forward", &link.between[0]),
                (Direction::Reverse, "reverse", &link.between[1]),
            ] {
                // A program on the node at the start of the link sends into it; in a network of several nodes its packets can
                // also reach any link on the way to another node
                let can_reach = *from == command.node || scenario.topology.nodes.len() > 2;
                if can_reach
                    && timeline::states_of(scenario, index, direction)
                        .iter()
                        .any(Impairments::is_random)
                {
                    reasons.push(format!(
                        "the program '{}' may send packets into link '{}' ({word}), whose queue draws random numbers: the run does not count or order them, so which packets are lost can differ from run to run with the same seed",
                        command.id, link.id
                    ));
                }
            }
        }
    }
    reasons
}

fn not_reproducible_because(
    scenario: &Scenario,
    provisioned: &Provisioned,
    queue_checks: &[QueueCheck],
    clock_reasons: Vec<String>,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if !provisioned.loss_reproducible {
        reasons.push("the backend cannot apply a seed (iproute2 6.6.0 or newer can)".to_owned());
    }
    for check in queue_checks.iter().filter(|check| !check.agrees()) {
        reasons.push(format!(
            "the queue of link '{}' ({}) counted {} packets and the workloads sent {}, so something else entered it and the lost packets are not those of the seed",
            check.link, check.direction, check.counted, check.expected
        ));
    }
    for index in timeline::links_that_go_down(scenario) {
        let link = &scenario.topology.links[index];
        let draws = [Direction::Forward, Direction::Reverse]
            .into_iter()
            .any(|direction| {
                timeline::states_of(scenario, index, direction)
                    .iter()
                    .any(Impairments::is_random)
            });
        if draws {
            reasons.push(format!(
                "link '{}' goes down while a queue of it draws random numbers: the kernel drops the packets in flight, which are not the same packets in every run, and the queue cannot be checked against the packets that were sent",
                link.id
            ));
        }
    }
    if !timeline::outage_directions(scenario).is_empty() {
        reasons.push(
            "the scenario has an outage: it begins and ends in the time of the receiver and is placed by the clock, so the packet at each edge can be lost in one run and delivered in another (the packets that the seed loses are the same)"
                .to_owned(),
        );
    }
    let corrupts = scenario.topology.links.iter().any(|link| {
        [&link.forward, &link.reverse, &link.both]
            .into_iter()
            .flatten()
            .any(|impairments| impairments.corrupt.is_some())
    });
    reasons.extend(clock_reasons);
    reasons.extend(shared_directions(scenario));
    reasons.extend(programs_into_random_queues(scenario));
    if corrupts {
        reasons.push(
            "the scenario corrupts packets: the seed fixes which packets are corrupted, but not where `netem` flips the bit, and a bit in the source address of the Ethernet header harms nothing, so a few corrupted packets arrive in one run and are dropped in another (the packets lost in the queue are the same)"
                .to_owned(),
        );
    }
    reasons
}

/// The directions that two or more workloads send into while their queue draws random numbers, one sentence for each
/// (D-038, D-065), and the directions whose random queue gets packets that the run cannot account for or cannot put in a
/// fixed order. Which packet draws which number depends on the order in which the packets reach the queue, and with two
/// senders that order is not fixed on a real kernel: their packets can be microseconds apart. The number of packets that are
/// lost agrees, the packets do not. After the first link of a path the packets that enter a queue are those that the links
/// before it let through (D-078): the run counts them when one workload alone crosses the link and the link before it, and
/// the order is fixed when the links before it keep the order of the packets.
fn shared_directions(scenario: &Scenario) -> Vec<String> {
    let mut reasons = Vec::new();
    let generators: Vec<Generator<'_>> = scenario
        .workloads
        .iter()
        .filter_map(Workload::generator)
        .collect();
    let paths = paths_of(scenario);
    let path_refs: Vec<Option<&netscenario_model::Path>> =
        paths.iter().map(Option::as_ref).collect();
    for (index, link) in scenario.topology.links.iter().enumerate() {
        for (direction, word) in [
            (Direction::Forward, "forward"),
            (Direction::Reverse, "reverse"),
        ] {
            if !timeline::states_of(scenario, index, direction)
                .iter()
                .any(Impairments::is_random)
            {
                continue;
            }
            let crossing = crossings(scenario, &path_refs, (index, direction));
            match feed(scenario, &path_refs, (index, direction)) {
                Feed::Idle => {}
                Feed::Known { .. } if crossing.len() > 1 => {
                    let names: Vec<String> = crossing
                        .iter()
                        .map(|c| format!("'{}'", generators[c.workload].id))
                        .collect();
                    reasons.push(format!(
                        "the workloads {} send into link '{}' ({word}) and its queue draws random numbers: the order in which their packets reach the queue is not fixed on a real kernel, so which packet draws which number, and so which packets are lost, can differ from run to run with the same seed",
                        names.join(", "),
                        link.id
                    ));
                }
                Feed::Known { .. } => {
                    let [only] = crossing.as_slice() else {
                        continue;
                    };
                    let Some(path) = &paths[only.workload] else {
                        continue;
                    };
                    let keys = way_keys(scenario, path);
                    let shuffles = keys[..only.position].iter().find(|(before, way)| {
                        timeline::states_of(scenario, *before, *way)
                            .iter()
                            .any(may_reorder)
                    });
                    if let Some((before, _)) = shuffles {
                        reasons.push(format!(
                            "the packets of '{}' reach the queue of link '{}' ({word}), which draws random numbers, after link '{}', which can change the order of packets (a jitter or a reorder): which packet draws which number, and so which packets are lost, can differ from run to run with the same seed",
                            generators[only.workload].id,
                            link.id,
                            scenario.topology.links[*before].id
                        ));
                    }
                }
                Feed::Unknown => {
                    let names: Vec<String> = crossing
                        .iter()
                        .map(|c| format!("'{}'", generators[c.workload].id))
                        .collect();
                    reasons.push(format!(
                        "the packets of {} reach the queue of link '{}' ({word}), which draws random numbers, together with the packets of other workloads or by one of several routes: the run does not know which packets enter it, so it cannot hold the queue to them and cannot say whether the lost packets are those of the seed",
                        names.join(", "),
                        link.id
                    ));
                }
            }
        }
    }
    reasons
}

/// What the result of a workload needs to know of the timeline and the clock.
struct Timeline<'a> {
    /// The start of the measured phase on the clock of the records.
    begin_ns: u64,
    /// The steps that were done.
    events: &'a [EventRecord],
}

/// The series of a workload by time bin, written to `series-<id>.jsonl`, and the time to recover after each step of the
/// timeline that concerns it (D-070).
fn series_of(
    workload: &Generator<'_>,
    schedule: Schedule,
    flow: &Measured,
    scenario: &Scenario,
    options: &RunOptions,
    timeline: &Timeline<'_>,
    start: Duration,
) -> Result<(SeriesInfo, Vec<Recovery>), Error> {
    let bin_ns = nanos(series_bin(scenario));
    let bins = series::bins(
        &series::Input {
            schedule,
            start_ns: nanos(start),
            origin_ns: timeline.begin_ns,
            slots: flow.sent.packets + flow.sent.errors,
            refused: &flow.sent.refused,
            records: &flow.records.records,
        },
        bin_ns,
    );
    let name = format!("series-{}.jsonl", workload.id);
    let file = std::fs::File::create(options.out_dir.join(&name))
        .map_err(|error| failure("write the series", format!("{name}: {error}")))?;
    series::write_series(std::io::BufWriter::new(file), workload.id.as_str(), &bins)
        .map_err(|error| failure("write the series", format!("{name}: {error}")))?;
    // The steps that act on a direction this workload sends in, along its path
    let on_the_way: Vec<(String, &'static str)> = paths_of(scenario)
        .into_iter()
        .flatten()
        .find(|path| {
            path.nodes.first() == Some(workload.from) && path.nodes.last() == Some(workload.to)
        })
        .map(|path| {
            way_keys(scenario, &path)
                .into_iter()
                .map(|(index, direction)| {
                    (
                        scenario.topology.links[index].id.to_string(),
                        match direction {
                            Direction::Forward => "forward",
                            Direction::Reverse => "reverse",
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let concerning: Vec<EventRecord> = timeline
        .events
        .iter()
        .filter(|event| {
            on_the_way.iter().any(|(link, word)| {
                event.link == *link
                    && event
                        .direction
                        .as_deref()
                        .is_none_or(|direction| direction == *word)
            })
        })
        .cloned()
        .collect();
    let recoveries = recovery::recoveries(&concerning, &bins, bin_ns, &recovery_options(scenario));
    Ok((
        SeriesInfo {
            bin_ns,
            bins: bins.len() as u64,
            file: name,
        },
        recoveries,
    ))
}

/// The results of all the workloads, in the order of the scenario.
fn workload_results(
    scenario: &Scenario,
    measured: &[Measured],
    provisioned: &Provisioned,
    options: &RunOptions,
    timeline: &Timeline<'_>,
) -> Result<Vec<WorkloadResult>, Error> {
    scenario
        .workloads
        .iter()
        .filter_map(Workload::generator)
        .zip(measured)
        .enumerate()
        .map(|(index, (workload, flow))| {
            let schedule =
                traffic::schedule_of(&workload, provisioned.seed.unwrap_or_default(), index)?;
            workload_result(
                index,
                (&workload, schedule),
                flow,
                scenario,
                provisioned,
                options,
                timeline,
            )
        })
        .collect()
}

/// The result of one workload: its statistics, and the files that belong to it.
fn workload_result(
    index: usize,
    (workload, schedule): (&Generator<'_>, Schedule),
    flow: &Measured,
    scenario: &Scenario,
    provisioned: &Provisioned,
    options: &RunOptions,
    timeline: &Timeline<'_>,
) -> Result<WorkloadResult, Error> {
    let start = workload.start.map_or(Duration::ZERO, Duration::from);
    let stop = workload
        .stop
        .map_or_else(|| Duration::from(scenario.duration), Duration::from);
    let first_send = flow.sent.first_send_ns.unwrap_or(0);
    let packets_file = if scenario.measure.packets {
        let name = format!("packets-{}.jsonl", workload.id);
        let file = std::fs::File::create(options.out_dir.join(&name))
            .map_err(|error| failure("write the packets", format!("{name}: {error}")))?;
        write_packets_refused(
            std::io::BufWriter::new(file),
            workload.id.as_str(),
            flow.sent.packets + flow.sent.errors,
            &flow.sent.refused,
            &flow.records.records,
            first_send,
            match &flow.naming {
                Naming::Named(names) => names,
                _ => &[],
            },
        )
        .map_err(|error| failure("write the packets", format!("{name}: {error}")))?;
        Some(name)
    } else {
        None
    };
    let (series, recoveries) = series_of(
        workload,
        schedule.clone(),
        flow,
        scenario,
        options,
        timeline,
        start,
    )?;
    let (from_address, to_address) =
        addresses_of(provisioned, workload, family_of(scenario, workload));
    Ok(WorkloadResult {
        id: workload.id.to_string(),
        kind: workload.kind.as_str().to_owned(),
        from: workload.from.to_string(),
        to: workload.to.to_string(),
        from_address,
        to_address: socket_text(&to_address, port(index)),
        frames_per_packet: paths_of(scenario)
            .get(index)
            .cloned()
            .flatten()
            .map_or(1, |path| {
                frames_of(scenario, workload, &schedule, &path).most()
            }),
        path: paths_of(scenario)
            .get(index)
            .cloned()
            .flatten()
            .map(|path| PathInfo {
                nodes: path.nodes.iter().map(ToString::to_string).collect(),
                links: path.links.iter().map(ToString::to_string).collect(),
                shared: path.shared,
            }),
        rate_bits_per_second: schedule.mean_bits_per_second(),
        packet_size_bytes: workload.packet_size.bytes(),
        trace: trace_info(workload),
        start_ns: nanos(start),
        duration_ns: nanos(stop.saturating_sub(start)),
        stats: analyse_refused(
            flow.sent.packets + flow.sent.errors,
            &flow.sent.refused,
            &flow.records.records,
            &|sequence| schedule.size_of(sequence),
            first_send,
        ),
        sender: SenderInfo {
            max_lateness_ns: flow.sent.max_lateness_ns,
            send_errors: flow.sent.errors,
            first_error: flow.sent.first_error.clone(),
            refused: flow.sent.refused.clone(),
        },
        foreign_datagrams: flow.received.foreign,
        corrupted: flow.received.corrupted,
        corrupted_named: match &flow.naming {
            Naming::Named(names) => Some(names.len() as u64),
            _ => None,
        },
        corrupted_seq: match &flow.naming {
            Naming::Named(names) => names.iter().take(MAX_LISTED_SEQUENCES).copied().collect(),
            _ => Vec::new(),
        },
        corrupted_seq_unavailable: match &flow.naming {
            Naming::Unavailable(why) => Some(why.clone()),
            _ => None,
        },
        records_file: records_name(index),
        packets_file,
        series: Some(series),
        recoveries,
    })
}

/// What the result says about the trace of a `trace-replay` workload.
fn trace_info(workload: &Generator<'_>) -> Option<TraceInfo> {
    let trace = workload.trace?;
    let data = trace.packets.as_ref()?;
    Some(TraceInfo {
        file: trace.file.clone(),
        sha256: trace.sha256.clone(),
        packets: data.len() as u64,
        padded: u64::from(trace.padded.unwrap_or(0)),
        smallest_bytes: data.smallest(),
        largest_bytes: data.largest(),
        span_ns: data.span_ns(),
    })
}

/// The width of a bin of the series: the one in the scenario, or a second (D-070, D-073).
fn series_bin(scenario: &Scenario) -> Duration {
    scenario
        .measure
        .series
        .as_ref()
        .and_then(|series| series.bin)
        .map_or(Duration::from_nanos(DEFAULT_BIN_NS), Duration::from)
}

/// The definition of the time to recover: the keys of the scenario, and the defaults for the others (D-070, D-073).
fn recovery_options(scenario: &Scenario) -> RecoveryOptions {
    let mut options = RecoveryOptions::default();
    if let Some(settings) = &scenario.measure.recovery {
        if let Some(basis) = settings.basis {
            options.basis = match basis {
                netscenario_model::RecoveryBasis::Send => Basis::Send,
                netscenario_model::RecoveryBasis::Arrival => Basis::Arrival,
            };
        }
        if let Some(bins) = settings.baseline_bins {
            options.baseline_bins = bins;
        }
        if let Some(bins) = settings.window_bins {
            options.window_bins = bins;
        }
        if let Some(fraction) = settings.fraction {
            options.fraction = f64::from(fraction.micro_percent()) / 100_000_000.0;
        }
    }
    options
}

/// The path of the packets of each workload that sends packets, in the order of the workloads (`None` where there is none,
/// which the validation of the scenario has refused already).
fn paths_of(scenario: &Scenario) -> Vec<Option<netscenario_model::Path>> {
    let routes = scenario.topology.routes();
    scenario
        .workloads
        .iter()
        .filter_map(Workload::generator)
        .map(|workload| {
            scenario
                .topology
                .path(&routes, workload.from, workload.to)
                .ok()
        })
        .collect()
}

/// The directions of the links along a path: the place of the link in the scenario and the direction the packets go in.
fn way_keys(scenario: &Scenario, path: &netscenario_model::Path) -> Vec<(usize, Direction)> {
    path.nodes
        .windows(2)
        .zip(&path.links)
        .filter_map(|(pair, link_name)| {
            let index = scenario
                .topology
                .links
                .iter()
                .position(|link| link.id == *link_name)?;
            let direction = if scenario.topology.links[index].between[0] == pair[0] {
                Direction::Forward
            } else {
                Direction::Reverse
            };
            Some((index, direction))
        })
        .collect()
}

/// A workload that crosses a direction: which one, at which place of its path, and whether that is the last link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Crossing {
    workload: usize,
    position: usize,
    last: bool,
}

/// The workloads whose packets cross one direction of a link, with the place of the direction on their paths.
fn crossings(
    scenario: &Scenario,
    paths: &[Option<&netscenario_model::Path>],
    key: (usize, Direction),
) -> Vec<Crossing> {
    let mut found = Vec::new();
    for (workload, path) in paths.iter().enumerate() {
        let Some(path) = path else { continue };
        let keys = way_keys(scenario, path);
        if let Some(position) = keys.iter().position(|other| *other == key) {
            found.push(Crossing {
                workload,
                position,
                last: position + 1 == keys.len(),
            });
        }
    }
    found
}

/// Where the packets that enter a direction come from, so that the run can account for them (D-040, ENV-07).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Feed {
    /// No workload sends into it.
    Idle,
    /// The packets that enter it are known: those the `senders` put in (workloads whose path begins with this link), plus
    /// those that arrived at the node before it by the `feeders` (links earlier on the paths), which forwards them all. This
    /// holds when every workload that crosses a feeder goes on into this direction, so that all the frames that arrived by
    /// the feeder enter this queue (D-078): a node makes no packet and takes none but by a damaged header, which a link that
    /// corrupts makes unaccountable (the tally says so).
    Known {
        senders: Vec<usize>,
        feeders: Vec<(usize, Direction)>,
    },
    /// Workloads of other directions share a feeder with it, or the packets may take one of several routes: the run does not
    /// know which of the packets that arrive enter it.
    Unknown,
}

fn feed(
    scenario: &Scenario,
    paths: &[Option<&netscenario_model::Path>],
    key: (usize, Direction),
) -> Feed {
    let crossing = crossings(scenario, paths, key);
    if crossing.is_empty() {
        return Feed::Idle;
    }
    if crossing
        .iter()
        .any(|c| paths[c.workload].is_some_and(|path| path.shared))
    {
        return Feed::Unknown;
    }
    let mut senders = Vec::new();
    let mut feeders: Vec<(usize, Direction)> = Vec::new();
    for c in &crossing {
        if c.position == 0 {
            senders.push(c.workload);
            continue;
        }
        let Some(path) = paths[c.workload] else {
            return Feed::Unknown;
        };
        let before = way_keys(scenario, path)[c.position - 1];
        if !feeders.contains(&before) {
            feeders.push(before);
        }
    }
    // Every workload that crosses a feeder must go on into this direction, or some of the frames that arrived are not for it
    for feeder in &feeders {
        let beyond = crossings(scenario, paths, *feeder);
        if beyond
            .iter()
            .any(|other| !crossing.iter().any(|c| c.workload == other.workload))
        {
            return Feed::Unknown;
        }
    }
    Feed::Known { senders, feeders }
}

/// The packets that entered a direction, when the run knows them: what the `senders` put in (by `sent_of`) and what arrived
/// at the node before it by each feeder (the difference of its counts before and after the workloads).
fn entered_packets(
    senders: &[usize],
    feeders: &[(usize, Direction)],
    sent_of: impl Fn(usize) -> u64, // the frames a sender put in
    before: &[Option<NodeCounts>],
    after: &[Option<NodeCounts>],
) -> Option<u64> {
    let mut total: u64 = senders.iter().map(|&workload| sent_of(workload)).sum();
    for feeder in feeders {
        let place = way_place(*feeder);
        let (Some(Some(was)), Some(Some(is))) = (before.get(place), after.get(place)) else {
            return None;
        };
        total += is.frames_received.saturating_sub(was.frames_received);
    }
    Some(total)
}

/// The place of a direction in the order of [`ways`].
fn way_place(key: (usize, Direction)) -> usize {
    key.0 * 2 + usize::from(key.1 == Direction::Reverse)
}

/// Whether the packets of a workload may come out of a link in another order than they went in: a jitter or a reorder.
fn may_reorder(impairments: &Impairments) -> bool {
    impairments.jitter.is_some() || impairments.reorder.is_some()
}

/// The frames a packet of the workload makes on the first link of its path (D-081).
fn frames_on_the_first_link(
    scenario: &Scenario,
    workload: &Generator<'_>,
    path: &netscenario_model::Path,
) -> u64 {
    // A sender that must not cut a packet sends it whole or not at all
    if workload.dont_fragment {
        return 1;
    }
    let mtu = netscenario_model::mtus_of(scenario, &path.links)
        .first()
        .copied()
        .unwrap_or(netscenario_model::DEFAULT_MTU);
    netscenario_model::frames_per_packet(
        workload.packet_size.bytes(),
        mtu,
        family_of(scenario, workload),
    )
}

/// How many frames the packets of a workload make on the first link (D-081): the same for every packet, or, for a trace, one number
/// for each packet because their sizes differ (D-085).
#[derive(Debug, Clone)]
enum Frames {
    /// Every packet makes this many.
    Each(u64),
    /// The frames of packet `n`, for each `n`.
    PerPacket(Vec<u32>),
}

impl Frames {
    /// The most frames any packet makes: more than one if some packet is cut into fragments.
    fn most(&self) -> u64 {
        match self {
            Self::Each(frames) => *frames,
            Self::PerPacket(frames) => frames.iter().copied().max().map_or(1, u64::from),
        }
    }

    /// The frames of the packets the sender put in: the sequence numbers it used, less the ones the kernel refused.
    fn of_sent(&self, sent: &SendSummary) -> u64 {
        match self {
            Self::Each(frames) => sent.packets * frames,
            Self::PerPacket(frames) => {
                let slots = usize::try_from(sent.packets + sent.errors).unwrap_or(usize::MAX);
                let refused = |sequence: u64| {
                    sent.refused
                        .iter()
                        .any(|[first, last]| (*first..=*last).contains(&sequence))
                };
                frames
                    .iter()
                    .take(slots)
                    .enumerate()
                    .filter(|(sequence, _)| !refused(*sequence as u64))
                    .map(|(_, frames)| u64::from(*frames))
                    .sum()
            }
        }
    }
}

/// The frames of the packets of a workload on the first link of its path.
fn frames_of(
    scenario: &Scenario,
    workload: &Generator<'_>,
    schedule: &Schedule,
    path: &netscenario_model::Path,
) -> Frames {
    let Schedule::Trace(trace) = schedule else {
        return Frames::Each(frames_on_the_first_link(scenario, workload, path));
    };
    if workload.dont_fragment {
        return Frames::Each(1);
    }
    let mtu = netscenario_model::mtus_of(scenario, &path.links)
        .first()
        .copied()
        .unwrap_or(netscenario_model::DEFAULT_MTU);
    let family = family_of(scenario, workload);
    Frames::PerPacket(
        trace
            .sizes()
            .iter()
            .map(|size| {
                u32::try_from(netscenario_model::frames_per_packet(
                    u64::from(*size),
                    mtu,
                    family,
                ))
                .unwrap_or(u32::MAX)
            })
            .collect(),
    )
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn port(index: usize) -> u16 {
    BASE_PORT.saturating_add(u16::try_from(index).unwrap_or(0))
}

fn trace_name(index: usize) -> String {
    format!("flow-{index}.trace")
}

fn records_name(index: usize) -> String {
    format!("flow-{index}.records")
}

fn wire_name(index: usize) -> String {
    format!("flow-{index}.wire")
}

/// Whether a direction on the path of a workload corrupts packets, so that the receiver reads the wire to name the damaged
/// ones.
fn corrupts_on_the_way(scenario: &Scenario, path: &netscenario_model::Path) -> bool {
    way_keys(scenario, path)
        .into_iter()
        .any(|(index, direction)| {
            impairments_of(&scenario.topology.links[index], direction)
                .is_some_and(|impairments| impairments.corrupt.is_some())
        })
}

/// The address of the sending node and of the receiving node of a workload: the addresses of the nodes on their loopbacks
/// (D-078), which do not depend on the path.
fn addresses_of(
    provisioned: &Provisioned,
    workload: &Generator<'_>,
    family: netscenario_model::IpFamily,
) -> (String, String) {
    let address = |node: &netscenario_model::Name| {
        provisioned
            .node_addresses
            .iter()
            .find(|(name, _)| name == node)
            .and_then(|(_, ips)| ips.of(family))
            .map(|address| address.to_string())
            .unwrap_or_default()
    };
    (address(workload.from), address(workload.to))
}

/// The family of the addresses a workload uses: its own `ip`, or the only one of the network (D-081).
fn family_of(scenario: &Scenario, workload: &Generator<'_>) -> netscenario_model::IpFamily {
    workload
        .ip
        .unwrap_or_else(|| scenario.topology.ip.default_family())
}

/// An address and a port as the agent reads them: `10.200.0.2:5000`, `[fd00:200::2]:5000`.
fn socket_text(address: &str, port: u16) -> String {
    if address.contains(':') {
        format!("[{address}]:{port}")
    } else {
        format!("{address}:{port}")
    }
}

/// The longest time a packet can be held back by the links of the scenario: the delay and four times the
/// jitter (a normal distribution has tails), added up along the path of each workload, and at least the worst link.
fn longest_hold(scenario: &Scenario) -> Duration {
    let hold = |impairments: &Impairments| {
        let delay = impairments.delay.map_or(Duration::ZERO, Duration::from);
        let jitter = impairments
            .jitter
            .as_ref()
            .map_or(Duration::ZERO, |j| Duration::from(j.amount) * 4);
        delay + jitter
    };
    let worst_link = scenario
        .topology
        .links
        .iter()
        .flat_map(|link| {
            [
                impairments_of(link, crate::backend::Direction::Forward),
                impairments_of(link, crate::backend::Direction::Reverse),
            ]
        })
        .flatten()
        .map(hold)
        .max()
        .unwrap_or(Duration::ZERO);
    let worst_path = paths_of(scenario)
        .into_iter()
        .flatten()
        .map(|path| {
            way_keys(scenario, &path)
                .into_iter()
                .filter_map(|(index, direction)| {
                    impairments_of(&scenario.topology.links[index], direction).map(hold)
                })
                .sum::<Duration>()
        })
        .max()
        .unwrap_or(Duration::ZERO);
    worst_link.max(worst_path)
}

fn plan<'a>(
    scenario: &'a Scenario,
    provisioned: &Provisioned,
    options: &RunOptions,
) -> Result<Vec<Flow<'a>>, Error> {
    let node = |name: &netscenario_model::Name| {
        provisioned
            .nodes
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, handle)| *handle)
            .ok_or_else(|| failure("find a node", format!("the node '{name}' was not built")))
    };
    let paths = paths_of(scenario);
    scenario
        .workloads
        .iter()
        .filter_map(Workload::generator)
        .enumerate()
        .map(|(index, workload)| {
            let path = paths.get(index).cloned().flatten().ok_or_else(|| {
                failure(
                    "find the path of a workload",
                    format!(
                        "there is no path from '{}' to '{}'",
                        workload.from, workload.to
                    ),
                )
            })?;
            let start = workload.start.map_or(Duration::ZERO, Duration::from);
            let stop = workload
                .stop
                .map_or_else(|| Duration::from(scenario.duration), Duration::from);
            let (_, to_address) =
                addresses_of(provisioned, &workload, family_of(scenario, &workload));
            let schedule =
                traffic::schedule_of(&workload, provisioned.seed.unwrap_or_default(), index)?;
            let frames = frames_of(scenario, &workload, &schedule, &path);
            Ok(Flow {
                index,
                schedule,
                workload,
                from_node: node(workload.from)?,
                to_node: node(workload.to)?,
                to_address: socket_text(&to_address, port(index)),
                records_file: options.out_dir.join(records_name(index)),
                // The tap names a damaged packet by its sequence number, which only the first fragment of a packet shows, and it
                // reads IPv4 frames
                wire_file: (corrupts_on_the_way(scenario, &path)
                    && frames.most() == 1
                    && family_of(scenario, &workload) == netscenario_model::IpFamily::V4)
                    .then(|| options.out_dir.join(wire_name(index))),
                wire_unavailable: if !corrupts_on_the_way(scenario, &path) {
                    None
                } else if family_of(scenario, &workload) == netscenario_model::IpFamily::V6 {
                    Some("the tap of the receiver reads IPv4 frames only, so the damaged packets of an IPv6 flow are counted but not named")
                } else if frames.most() > 1 {
                    Some("a packet that is cut into fragments shows its sequence number in the first fragment only, so its damaged packets are counted but not named")
                } else {
                    None
                },
                frames,
                path,
                start,
                duration: stop.saturating_sub(start),
            })
        })
        .collect()
}

/// Waits for the first line of a program, and fails if it ends or takes too long.
pub(crate) fn wait_for_line(
    backend: &mut dyn Backend,
    program: ProgramHandle,
    within: Duration,
    what: &str,
    control: &Control<'_>,
) -> Result<String, Error> {
    let deadline = Instant::now() + within;
    loop {
        control.checkpoint()?;
        let state = backend.poll_program(program, Duration::from_millis(200))?;
        if let Some(line) = state.lines.into_iter().next() {
            return Ok(line);
        }
        if let Some(exit) = state.exit {
            return Err(failure(
                what,
                format!(
                    "the agent ended ({exit}) before it said anything: {}",
                    state.stderr.trim()
                ),
            ));
        }
        if Instant::now() >= deadline {
            return Err(failure(
                what,
                format!("the agent said nothing within {within:?}"),
            ));
        }
    }
}

/// Waits until a program has ended with the exit code 0 and returns what it printed.
pub(crate) fn wait_for_end(
    backend: &mut dyn Backend,
    program: ProgramHandle,
    deadline: Instant,
    what: &str,
    control: &Control<'_>,
) -> Result<Vec<String>, Error> {
    let mut lines = Vec::new();
    loop {
        control.checkpoint()?;
        let state = backend.poll_program(program, Duration::from_millis(200))?;
        lines.extend(state.lines);
        if let Some(exit) = state.exit {
            return if exit.success() {
                Ok(lines)
            } else {
                Err(failure(
                    what,
                    format!("the agent ended with {exit}: {}", state.stderr.trim()),
                ))
            };
        }
        if Instant::now() >= deadline {
            return Err(failure(what, "the agent did not end in time"));
        }
    }
}

pub(crate) fn event(line: &str, what: &str) -> Result<Event, Error> {
    serde_json::from_str(line).map_err(|error| {
        failure(
            what,
            format!("cannot read the agent's line '{line}': {error}"),
        )
    })
}

pub(crate) fn agent_program(options: &RunOptions, arguments: &[String]) -> Program {
    let mut argv = vec![options.agent.display().to_string()];
    argv.extend_from_slice(arguments);
    Program::new(argv).with_memory_limit(options.agent_memory_limit)
}

/// `--udp-checksum off` for a workload that sends without the checksum of UDP, nothing for one that does not.
fn checksum_argument(workload: &Generator<'_>) -> Vec<String> {
    if workload.udp_checksum.is_on() {
        Vec::new()
    } else {
        strings(&["--udp-checksum", "off"])
    }
}

pub(crate) fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// The receivers first: they open the ports and say when they are ready.
fn start_receivers(
    backend: &mut dyn Backend,
    flows: &[Flow<'_>],
    options: &RunOptions,
    control: &Control<'_>,
) -> Result<Vec<Started>, Error> {
    let mut receivers = Vec::new();
    for flow in flows {
        control.checkpoint()?;
        let what = format!("start the receiver of '{}'", flow.workload.id);
        let mut arguments = strings(&["recv", "--bind", &flow.to_address, "--flow"]);
        arguments.push(flow.index.to_string());
        arguments.extend(strings(&[
            "--out",
            &flow.records_file.display().to_string(),
        ]));
        arguments.extend(checksum_argument(&flow.workload));
        if let Some(wire) = &flow.wire_file {
            arguments.extend(strings(&["--wire-out", &wire.display().to_string()]));
        }
        let program = backend.start_program(flow.to_node, &agent_program(options, &arguments))?;
        let line = wait_for_line(backend, program, options.ready_timeout, &what, control)?;
        let wire_error = match event(&line, &what)? {
            Event::Ready { wire_error, .. } => wire_error,
            other => {
                return Err(failure(
                    what,
                    format!("expected ready, the agent said {other:?}"),
                ));
            }
        };
        receivers.push(Started {
            program,
            wire_error,
        });
    }
    Ok(receivers)
}

/// The senders, all with one moment to start at, so that the workloads start together. Each is paired with the
/// words for its error messages.
fn start_senders(
    backend: &mut dyn Backend,
    flows: &[Flow<'_>],
    options: &RunOptions,
    begin: u64,
    bindings: &[Option<drive::Binding>],
) -> Result<Vec<(ProgramHandle, String)>, Error> {
    let mut senders = Vec::new();
    for (index, flow) in flows.iter().enumerate() {
        let what = format!("start the sender of '{}'", flow.workload.id);
        let mut arguments = strings(&["send", "--to", &flow.to_address]);
        // A trace is a file for the agent, written here before the sender starts
        let trace_file = match traffic::trace_text(&flow.schedule) {
            Some(text) => {
                let path = std::path::absolute(options.out_dir.join(trace_name(flow.index)))
                    .unwrap_or_else(|_| options.out_dir.join(trace_name(flow.index)));
                std::fs::write(&path, text).map_err(|error| {
                    failure(
                        "write the trace of a workload",
                        format!("{}: {error}", path.display()),
                    )
                })?;
                Some(path)
            }
            None => None,
        };
        arguments.extend(traffic::arguments(&flow.schedule, trace_file.as_deref()));
        arguments.push("--duration-ns".to_owned());
        arguments.push(nanos(flow.duration).to_string());
        arguments.push("--flow".to_owned());
        arguments.push(flow.index.to_string());
        arguments.push("--start-at-ns".to_owned());
        arguments.push((begin + nanos(flow.start)).to_string());
        arguments.extend(checksum_argument(&flow.workload));
        if flow.workload.dont_fragment {
            arguments.push("--dont-fragment".to_owned());
        }
        // The steps this sender waits for: before which packet, and under which number the run answers
        let barriers: Vec<String> = bindings
            .iter()
            .enumerate()
            .filter_map(|(id, binding)| {
                binding
                    .filter(|binding| binding.sender == index)
                    .map(|binding| format!("{}:{id}", binding.packet))
            })
            .collect();
        if !barriers.is_empty() {
            arguments.extend(["--barriers".to_owned(), barriers.join(",")]);
        }
        senders.push((
            backend.start_program(flow.from_node, &agent_program(options, &arguments))?,
            what,
        ));
    }
    Ok(senders)
}

/// Steps 5 to 7: start the agents, let the workloads run, stop the receivers and read what they recorded.
fn measure(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    options: &RunOptions,
    provisioned: &Provisioned,
    control: &Control<'_>,
) -> Result<Measurement, Error> {
    let mut programs = Programs::default();
    let mut captures = Captures::default();
    let measured = measure_with(
        backend,
        scenario,
        options,
        provisioned,
        control,
        (&mut programs, &mut captures),
    );
    if measured.is_err() {
        // The process groups of the programs are not ended by the teardown of the network: they are ended here
        programs.kill_all(backend);
        captures.kill_all(backend);
    }
    measured
}

/// Steps 5 to 7 with the programs of your own in `programs`.
fn measure_with(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    options: &RunOptions,
    provisioned: &Provisioned,
    control: &Control<'_>,
    (programs, captures): (&mut Programs, &mut Captures),
) -> Result<Measurement, Error> {
    control.checkpoint()?;
    if !options.agent.is_file() {
        return Err(failure(
            "find the agent",
            format!(
                "{} does not exist; build it with `cargo build -p netscenario-agent` or put it next to the netscenario program",
                options.agent.display()
            ),
        ));
    }
    let flows = plan(scenario, provisioned, options)?;

    // The captures first of all, so that no frame is missed; then the programs of your own, each when the one before is
    // ready: a server has to be there before the packets
    captures.start(backend, scenario, provisioned, options, control)?;
    programs.start(backend, scenario, provisioned, options, control)?;
    let receivers = start_receivers(backend, &flows, options, control)?;
    control.emit(&RunEvent::ReceiversReady {
        count: receivers.len(),
    });
    control.checkpoint()?;
    // What the nodes had counted before the first packet; the nodes are new, but a run reads the difference
    let before = node_counts(backend, scenario, provisioned)?;
    let begin_at = Instant::now() + START_MARGIN;
    let begin = monotonic_ns() + nanos(START_MARGIN);
    programs.set_begin(begin);
    // The steps of the timeline, and which of them a sender waits for (D-038)
    let steps = timeline::steps(scenario);
    let sending: Vec<barriers::Sending<'_>> = flows
        .iter()
        .map(|flow| barriers::Sending {
            from: flow.workload.from,
            to: flow.workload.to,
            schedule: flow.schedule.clone(),
            start_ns: nanos(flow.start),
            duration_ns: nanos(flow.duration),
        })
        .collect();
    let plan = barriers::plan(scenario, &sending, &steps, options.barriers);
    let senders = start_senders(backend, &flows, options, begin, &plan.bindings)?;

    let latest_end = flows
        .iter()
        .map(|flow| flow.start + flow.duration)
        .max()
        .unwrap_or(Duration::ZERO);
    control.workloads_started(begin_at, latest_end);
    // The events are part of the scenario up to its end, also where every workload has stopped before
    let run_for = latest_end.max(Duration::from(scenario.duration));
    let deadline = Instant::now() + START_MARGIN + run_for + END_MARGIN;
    let mut sampler = queue_sampler(scenario, provisioned);
    let driven = drive::drive(
        backend,
        scenario,
        provisioned,
        &senders,
        &steps,
        &plan.bindings,
        drive::Begin { ns: begin },
        deadline,
        drive::Watch {
            programs,
            hold_until: begin_at + Duration::from(scenario.duration),
            queues: &mut sampler,
        },
        control,
    )?;
    let sent = driven.sent;
    let events = driven.events;
    let interrupted_ns = driven.interrupted_ns;
    control.workloads_done();

    // The packets that are still in the network arrive before the receivers are told to stop
    let (settle, settle_note) = settle(backend, scenario, provisioned, options, control)?;
    // The programs end after the packets have, with their last words
    programs.stop_all(backend)?;
    captures.stop_all(backend, control)?;
    // After the wait, every packet has passed its queue; the receivers still run and the network still stands
    let after = node_counts(backend, scenario, provisioned)?;
    let queue_checks = if programs.is_empty() {
        check_queues(
            backend,
            scenario,
            provisioned,
            &flows,
            &sent,
            &before,
            &after,
        )?
    } else {
        // A program sends packets of its own into the queues; the run did not count them and cannot hold the queue to its own
        Vec::new()
    };
    control.emit(&RunEvent::Collecting);
    let mut measured = stop_receivers(backend, &receivers, &flows, sent, control)?;
    let directions = tally_directions(scenario, provisioned, &flows, &measured, &before, &after);
    refuse_names_after_buffer_drops(scenario, provisioned, &flows, &directions, &mut measured);
    let queues = queue_infos(backend, scenario, provisioned, &sampler)?;
    Ok(Measurement {
        measured,
        settle,
        queue_checks,
        directions,
        queues,
        events,
        notes: backend.notes().into_iter().chain(settle_note).collect(),
        interrupted_ns,
        begin_ns: begin,
        clock_reasons: plan.reasons,
        programs: std::mem::take(programs).into_results(begin),
        captures: std::mem::take(captures).into_results(begin),
    })
}

/// How long the run gives the receivers, after the queues are empty, to read the packets that the links let go.
const DRAIN_GRACE: Duration = Duration::from_millis(300);

/// How often the run looks at the queues while it waits for them to empty.
const DRAIN_POLL: Duration = Duration::from_millis(50);

/// The longest the run waits for the queues to empty when the wait is not set. A queue that takes longer is a queue of
/// a rate so low or a limit so high that the packets in it are the point of the scenario: set the wait.
const DRAIN_CEILING: Duration = Duration::from_secs(120);

/// Waits for the packets that are still in the network, and returns how long it waited, and a sentence if the wait
/// ended with packets in the queues (D-067).
///
/// With a wait that is set (`options.settle`) it is that long. Without one the run waits until the queues of the links
/// are empty and then [`DRAIN_GRACE`], because a link with a rate holds the packets that are sent faster than it
/// lets them go for as long as that takes, and a packet that is still in the queue when the receivers stop counts as
/// lost although it is not. A backend that cannot count the packets in its queues gets the time that follows from the
/// delays of the scenario: one second and twice the longest hold.
fn settle(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
    options: &RunOptions,
    control: &Control<'_>,
) -> Result<(Duration, Option<String>), Error> {
    settle_with(
        backend,
        scenario,
        provisioned,
        options,
        control,
        DRAIN_CEILING,
    )
}

/// [`settle`] with the longest wait for the queues given, so that a test can reach it.
fn settle_with(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
    options: &RunOptions,
    control: &Control<'_>,
    ceiling: Duration,
) -> Result<(Duration, Option<String>), Error> {
    let estimate = options
        .settle
        .unwrap_or_else(|| Duration::from_secs(1) + longest_hold(scenario) * 2);
    control.emit(&RunEvent::Settling { wait: estimate });
    let began = Instant::now();
    let wait_until = |moment: Instant| -> Result<(), Error> {
        loop {
            control.checkpoint()?;
            let left = moment.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    };
    if options.settle.is_some() {
        wait_until(began + estimate)?;
        return Ok((estimate, None));
    }
    let mut empty_since: Option<Instant> = None;
    loop {
        control.checkpoint()?;
        let Some(backlog) = backlog(backend, scenario, provisioned)? else {
            // The backend cannot say: the time that the delays give
            wait_until(began + estimate)?;
            return Ok((began.elapsed(), None));
        };
        if backlog == 0 {
            let since = *empty_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= DRAIN_GRACE {
                return Ok((began.elapsed(), None));
            }
        } else {
            empty_since = None;
            if began.elapsed() >= ceiling {
                let note = format!(
                    "the wait for the queues to empty ended after {:.0} s with {backlog} packets still in them; they count as lost. Set the wait (`--settle`) to a longer time if they are not lost",
                    ceiling.as_secs_f64()
                );
                return Ok((began.elapsed(), Some(note)));
            }
        }
        std::thread::sleep(DRAIN_POLL);
    }
}

/// The packets in the queues of all directions of all links, or `None` if the backend cannot say for one of them.
fn backlog(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
) -> Result<Option<u64>, Error> {
    let mut total = 0_u64;
    for way in ways(scenario, provisioned) {
        let Some(packets) = backend.queue_backlog(way.handle, way.direction)? else {
            return Ok(None);
        };
        total += packets;
    }
    Ok(Some(total))
}

/// Stops the receivers one after the other and reads what each of them recorded, with what its sender said.
fn stop_receivers(
    backend: &mut dyn Backend,
    receivers: &[Started],
    flows: &[Flow<'_>],
    sent: Vec<SendSummary>,
    control: &Control<'_>,
) -> Result<Vec<Measured>, Error> {
    let mut measured = Vec::new();
    for ((receiver, flow), sent) in receivers.iter().zip(flows).zip(sent) {
        control.checkpoint()?;
        let what = format!("stop the receiver of '{}'", flow.workload.id);
        backend.send_line(receiver.program, "stop")?;
        let lines = wait_for_end(
            backend,
            receiver.program,
            Instant::now() + END_MARGIN,
            &what,
            control,
        )?;
        let received = match lines.last().map(|line| event(line, &what)).transpose()? {
            Some(Event::Received(summary)) => summary,
            other => {
                return Err(failure(
                    what,
                    format!("the receiver should end with 'received', it said {other:?}"),
                ));
            }
        };
        let file = std::fs::File::open(&flow.records_file).map_err(|error| {
            failure(
                "read the records",
                format!("{}: {error}", flow.records_file.display()),
            )
        })?;
        let records = records::read(std::io::BufReader::new(file)).map_err(|error| {
            failure(
                "read the records",
                format!("{}: {error}", flow.records_file.display()),
            )
        })?;
        let naming = name_from_the_wire(flow, receiver, &received, &sent, &records);
        measured.push(Measured {
            sent,
            received,
            records,
            naming,
        });
    }
    Ok(measured)
}

/// Reads the file of the tap of a receiver and names the damaged packets, or says why it cannot (D-040).
fn name_from_the_wire(
    flow: &Flow<'_>,
    started: &Started,
    received: &ReceiveSummary,
    sent: &SendSummary,
    delivered: &records::File,
) -> Naming {
    if let Some(why) = flow.wire_unavailable {
        return Naming::Unavailable(why.to_owned());
    }
    let Some(path) = &flow.wire_file else {
        return Naming::NotAsked;
    };
    if let Some(error) = &started.wire_error {
        return Naming::Unavailable(format!(
            "{error}; the capability CAP_NET_RAW is needed in the node (`netscenario doctor` tests it)"
        ));
    }
    if received.wire_packets.is_none() {
        return Naming::Unavailable(
            "the tap of the receiver did not finish, so the file of what it saw cannot be trusted"
                .to_owned(),
        );
    }
    let seen = std::fs::File::open(path)
        .map_err(|error| error.to_string())
        .and_then(|file| {
            records::read(std::io::BufReader::new(file)).map_err(|error| error.to_string())
        });
    match seen {
        Ok(seen) => Naming::Named(crate::stats::name_damaged_refused(
            sent.packets + sent.errors,
            &sent.refused,
            &delivered.records,
            &seen.records,
        )),
        Err(error) => Naming::Unavailable(format!("{}: {error}", path.display())),
    }
}

/// A node that dropped datagrams because the buffer of a socket was full did not deliver packets that were not
/// damaged, and those would be named as damaged: where that happened, the names are withdrawn (D-040).
fn refuse_names_after_buffer_drops(
    scenario: &Scenario,
    provisioned: &Provisioned,
    flows: &[Flow<'_>],
    directions: &[DirectionTally],
    measured: &mut [Measured],
) {
    for (flow, measured) in flows.iter().zip(measured) {
        if !matches!(measured.naming, Naming::Named(_)) {
            continue;
        }
        let on_the_way = way_keys(scenario, &flow.path);
        let overflowed = ways(scenario, provisioned)
            .filter(|way| on_the_way.contains(&(way.index, way.direction)))
            .any(|way| {
                directions.iter().any(|tally| {
                    tally.link == way.name.as_str()
                        && tally.direction == way.word
                        && tally.receive_buffer_drops > 0
                })
            });
        if overflowed {
            measured.naming = Naming::Unavailable(
                "the node dropped datagrams because the receive buffer of a socket was full, so a packet that is missing may not be a damaged one"
                    .to_owned(),
            );
        }
    }
}

/// One direction of a link of the scenario.
struct Way<'a> {
    /// The place of the link in the scenario.
    index: usize,
    name: &'a netscenario_model::Name,
    handle: LinkHandle,
    direction: Direction,
    /// `forward` or `reverse`, as the result says it.
    word: &'static str,
}

/// The directions of the links, in the order of the scenario: for each link the forward one, then the reverse.
fn ways<'a>(scenario: &'a Scenario, provisioned: &'a Provisioned) -> impl Iterator<Item = Way<'a>> {
    scenario
        .topology
        .links
        .iter()
        .zip(&provisioned.links)
        .enumerate()
        .flat_map(|(index, (_link, (name, handle)))| {
            [
                (Direction::Forward, "forward"),
                (Direction::Reverse, "reverse"),
            ]
            .map(move |(direction, word)| Way {
                index,
                name,
                handle: *handle,
                direction,
                word,
            })
        })
}

/// What the receiving node of every direction has counted so far, in the order of [`ways`]. `None` for
/// a direction whose backend cannot count, and for every direction in a scenario with a program of your own,
/// whose packets the agent does not count (the tally of a direction would not add up).
fn node_counts(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
) -> Result<Vec<Option<NodeCounts>>, Error> {
    let has_program = scenario
        .workloads
        .iter()
        .any(|workload| workload.generator().is_none());
    ways(scenario, provisioned)
        .map(|way| {
            if has_program {
                Ok(None)
            } else {
                backend.node_counts(way.handle, way.direction)
            }
        })
        .collect()
}

/// Sets the packets that entered each direction against what the node at its end counted and what its receivers got (D-040).
/// What enters the first link of a path is what the workloads sent; what enters a later link is what arrived at the node
/// before it, which forwards it (D-078), so a packet is accounted for link by link: lost in this queue, dropped by the node,
/// or delivered. A direction without a workload has no tally, and neither has one that packets of several workloads reach
/// after the first link, or that they reach by one of several routes: the run cannot tell which packets those are.
fn tally_directions(
    scenario: &Scenario,
    provisioned: &Provisioned,
    flows: &[Flow<'_>],
    measured: &[Measured],
    before: &[Option<NodeCounts>],
    after: &[Option<NodeCounts>],
) -> Vec<DirectionTally> {
    let paths: Vec<Option<&netscenario_model::Path>> =
        flows.iter().map(|flow| Some(&flow.path)).collect();
    let mut tallies = Vec::new();
    for (way, (counted_before, counted_after)) in
        ways(scenario, provisioned).zip(before.iter().zip(after))
    {
        let (Some(counted_before), Some(counted_after)) = (counted_before, counted_after) else {
            continue;
        };
        let link = &scenario.topology.links[way.index];
        let key = (way.index, way.direction);
        let entered = match feed(scenario, &paths, key) {
            Feed::Known { senders, feeders } => {
                let Some(total) = entered_packets(
                    &senders,
                    &feeders,
                    |workload| flows[workload].frames.of_sent(&measured[workload].sent),
                    before,
                    after,
                ) else {
                    continue;
                };
                total
            }
            Feed::Idle | Feed::Unknown => continue,
        };
        let crossing = crossings(scenario, &paths, key);
        // The tally is by packet and a queue sees frames: a packet that is cut into fragments is not accounted for here (D-081);
        // its queue is still checked, by frames
        if crossing.iter().any(|c| flows[c.workload].frames.most() > 1) {
            continue;
        }
        // The receivers are at the end of the last link only; at a node in the middle the packets are forwarded, and what
        // happens to them there shows on the next link
        let at_the_end = crossing.iter().all(|c| c.last);
        if !at_the_end && crossing.iter().any(|c| c.last) {
            continue;
        }
        let delivered: u64 = if at_the_end {
            crossing
                .iter()
                .map(|c| {
                    let m = &measured[c.workload];
                    m.received.packets + m.received.foreign + m.received.corrupted
                })
                .sum()
        } else {
            counted_after
                .frames_received
                .saturating_sub(counted_before.frames_received)
        };
        let reached_node = counted_after
            .frames_received
            .saturating_sub(counted_before.frames_received);
        let receive_buffer_drops = counted_after
            .receive_buffer_drops
            .saturating_sub(counted_before.receive_buffer_drops);
        // The frames of a duplicating direction include the copies, so what the queue dropped cannot be told; nor can it
        // after a link that corrupts: a router drops a packet whose header was damaged, and that packet did not enter this
        // queue though it arrived at the node before it
        let duplicates = timeline::states_of(scenario, way.index, way.direction)
            .iter()
            .any(|impairments| impairments.duplicate.is_some())
            || impairments_of(link, way.direction)
                .is_some_and(|impairments| impairments.duplicate.is_some());
        let damaged_before = flows.iter().any(|flow| {
            let keys = way_keys(scenario, &flow.path);
            keys.iter()
                .position(|other| *other == key)
                .is_some_and(|at| {
                    keys[..at].iter().any(|(index, direction)| {
                        timeline::states_of(scenario, *index, *direction)
                            .iter()
                            .any(|impairments| impairments.corrupt.is_some())
                    })
                })
        });
        tallies.push(DirectionTally {
            link: way.name.to_string(),
            direction: way.word.to_owned(),
            sent: entered,
            reached_node,
            delivered,
            lost_in_queue: if duplicates || damaged_before {
                None
            } else {
                entered.checked_sub(reached_node)
            },
            dropped_corrupted: if at_the_end {
                reached_node
                    .saturating_sub(delivered)
                    .saturating_sub(receive_buffer_drops)
            } else {
                0
            },
            receive_buffer_drops,
        });
    }
    tallies
}

/// Asks the backend how many packets entered the queue of each direction with random loss, and sets it
/// against the packets that the senders put into the direction (ENV-07). A sender that the kernel refused a
/// packet for did not put that packet in. Nothing is checked without a seed (the loss is not repeatable
/// anyway) or when the backend cannot count.
fn check_queues(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
    flows: &[Flow<'_>],
    sent: &[SendSummary],
    before: &[Option<NodeCounts>],
    after: &[Option<NodeCounts>],
) -> Result<Vec<QueueCheck>, Error> {
    let mut checks = Vec::new();
    if provisioned.seed.is_none() {
        return Ok(checks);
    }
    let paths: Vec<Option<&netscenario_model::Path>> =
        flows.iter().map(|flow| Some(&flow.path)).collect();
    // A link that goes down drops the packets that are in its queue without counting them, so the queue cannot be
    // held to the number of packets that were sent; the run says so (D-064)
    let goes_down = timeline::links_that_go_down(scenario);
    for way in ways(scenario, provisioned) {
        if goes_down.contains(&way.index) {
            continue;
        }
        let link = &scenario.topology.links[way.index];
        // A duplicating direction is not checked: the queue counts the copies it makes as well, and the count
        // cannot tell them from a packet that nobody asked for (D-054)
        if impairments_of(link, way.direction)
            .is_none_or(|impairments| !impairments.is_random() || impairments.duplicate.is_some())
        {
            continue;
        }
        // What enters the queue: what the senders put in, or, after the first link of a path, what arrived at the node before
        // it (a link that goes down on the way is left out above only for itself: the node after it counts what it got)
        let expected = match feed(scenario, &paths, (way.index, way.direction)) {
            Feed::Idle => 0,
            Feed::Known { senders, feeders } => {
                let Some(total) = entered_packets(
                    &senders,
                    &feeders,
                    |workload| flows[workload].frames.of_sent(&sent[workload]),
                    before,
                    after,
                ) else {
                    continue;
                };
                total
            }
            Feed::Unknown => continue,
        };
        if let Some(counted) = backend.queue_packets(way.handle, way.direction)? {
            checks.push(QueueCheck {
                link: way.name.to_string(),
                direction: way.word.to_owned(),
                expected,
                counted,
            });
        }
    }
    Ok(checks)
}

/// The sampler of the queues of the scenario: one look a second at every direction that has a `queue` (PRB-10).
fn queue_sampler(scenario: &Scenario, provisioned: &Provisioned) -> crate::queues::Sampler {
    crate::queues::Sampler::new(ways(scenario, provisioned).filter_map(|way| {
        let link = &scenario.topology.links[way.index];
        impairments_of(link, way.direction)
            .and_then(|impairments| impairments.queue.as_ref())
            .map(|_| (way.name.to_string(), way.word, way.handle, way.direction))
    }))
}

/// What the kernel counted in the queue of every direction that has a `queue` (PRB-10, D-082). A backend that cannot say leaves
/// the list empty.
fn queue_infos(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
    sampler: &crate::queues::Sampler,
) -> Result<Vec<QueueInfo>, Error> {
    let mut found = Vec::new();
    for way in ways(scenario, provisioned) {
        let link = &scenario.topology.links[way.index];
        let Some(queue) = impairments_of(link, way.direction).and_then(|i| i.queue.as_ref()) else {
            continue;
        };
        let Some(stats) = backend.queue_stats(way.handle, way.direction)? else {
            continue;
        };
        found.push(QueueInfo {
            link: way.name.to_string(),
            direction: way.word.to_owned(),
            build: "fabric".to_owned(),
            discipline: queue.discipline.as_str().to_owned(),
            kind: stats.kind,
            packets: stats.packets,
            bytes: stats.bytes,
            drops: stats.drops,
            overlimits: stats.overlimits,
            requeues: stats.requeues,
            backlog_packets: stats.backlog_packets,
            backlog_bytes: stats.backlog_bytes,
            details: stats.details,
            samples: sampler.samples_of(way.name.as_str(), way.word),
        });
    }
    Ok(found)
}

/// The path of the agent that belongs to the program at `exe`: the file `netscenario-agent` next to it.
#[must_use]
pub fn agent_next_to(exe: &Path) -> PathBuf {
    exe.with_file_name("netscenario-agent")
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::net::Ipv4Addr;

    use netscenario_agent::records::{Record, Writer};

    use super::*;
    use crate::backend::{BackendInfo, Direction, LinkHandle, ProgramExit, ProgramState};
    use crate::capability::{Capabilities, Support};
    use crate::feature::Feature;

    /// A backend that plays the agent: the receiver writes a record file of its own and says what the real
    /// one says, the sender reports what it was told to send.
    struct Fake {
        calls: Vec<String>,
        programs: Vec<Played>,
        /// A failure to play: `recv-exits-early` or `send-fails`.
        trouble: Option<&'static str>,
        /// What the queues of the forward and the reverse direction say they counted, if they can count.
        counts: Option<[u64; 2]>,
        /// The frames that the receiving nodes of the forward and the reverse direction counted by the end,
        /// and the datagrams the first of them dropped for a full buffer. A node counts nothing before the
        /// first reading.
        frames: Option<([u64; 2], u64)>,
        readings: [u32; 2],
        /// For a network of several links: what the queue and the receiving node of one direction of a link count, by the
        /// place of the link and `true` for the reverse direction. Where a link is not in it, `counts` and `frames` speak.
        per_link: std::collections::HashMap<(u32, bool), (u64, u64)>,
        links_made: u32,
        /// The packets the queue of the forward direction says it dropped, if the fake has a queue.
        queue_drops: Option<u64>,
        link_readings: std::collections::HashMap<(u32, bool), u32>,
        /// The value of `--udp-checksum` each agent was started with, as `recv off`, in the order of starting.
        checksum: Vec<String>,
        /// How many datagrams the receiver reports as damaged: 2 when it was told that the checksum is off,
        /// because the two packets that the fake never records are the ones that arrived damaged.
        damaged: u64,
        /// How many packets the tap of the receiver reports, when it was asked for one and could be opened.
        wire_packets: Option<u64>,
        /// The barriers the senders were started with, as (packet, id).
        barriers: Vec<(u64, u32)>,
        /// How many packets the queue of the forward direction holds at each look after the workloads (D-067); then
        /// it is `last` for ever. `None`: the fake cannot say.
        backlog: Option<(VecDeque<u64>, u64)>,
    }

    /// The place of a link and `true` for its reverse direction, then what the queue of it counted and how many frames
    /// reached the node at its end.
    type LinkCount = ((u32, bool), (u64, u64));

    struct Played {
        lines: VecDeque<String>,
        exit: Option<ProgramExit>,
        stderr: String,
        stops_on_line: bool,
        /// A sender that goes on until it is told to stop (D-066).
        ends_on_stop: bool,
    }

    impl Fake {
        fn new(trouble: Option<&'static str>) -> Self {
            Self {
                calls: Vec::new(),
                programs: Vec::new(),
                trouble,
                counts: None,
                frames: None,
                readings: [0; 2],
                per_link: std::collections::HashMap::new(),
                links_made: 0,
                queue_drops: None,
                link_readings: std::collections::HashMap::new(),
                checksum: Vec::new(),
                damaged: 0,
                wire_packets: None,
                barriers: Vec::new(),
                backlog: None,
            }
        }

        /// A fake whose forward queue holds `script` packets at the looks after the workloads, and `last` after that.
        fn with_backlog(script: &[u64], last: u64) -> Self {
            let mut fake = Self::new(None);
            fake.backlog = Some((script.iter().copied().collect(), last));
            fake
        }

        /// What the fake sender does: it asks for each of its barriers, and its summary says how long it waited.
        fn play_sender(&mut self, argv: &[String]) -> Played {
            assert_eq!(argument(argv, "--rate"), "800000");
            assert_eq!(argument(argv, "--size"), "1000");
            assert_eq!(argument(argv, "--duration-ns"), "1000000000");
            assert!(argument(argv, "--start-at-ns").parse::<u64>().unwrap() > 0);
            // A sender with barriers asks for each of them, and its summary says how long it waited
            let barriers: Vec<(u64, u32)> = argv
                .iter()
                .position(|a| a == "--barriers")
                .map(|at| {
                    argv[at + 1]
                        .split(',')
                        .map(|part| {
                            let (packet, id) = part.split_once(':').unwrap();
                            (packet.parse().unwrap(), id.parse().unwrap())
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.barriers.extend(barriers.iter().copied());
            let mut lines: VecDeque<String> = barriers
                .iter()
                .map(|(packet, id)| {
                    format!(r#"{{"event":"barrier","flow":0,"id":{id},"packet":{packet}}}"#)
                })
                .collect();
            let done: Vec<String> = barriers
                .iter()
                .map(|(packet, id)| {
                    format!(
                        r#"{{"id":{id},"packet":{packet},"asked_ns":1,"held_ns":{}}}"#,
                        1000 * (u64::from(*id) + 1)
                    )
                })
                .collect();
            lines.push_back(format!(
                r#"{{"event":"sent","flow":0,"packets":10,"first_send_ns":1000,"last_send_ns":900001000,"max_lateness_ns":1234,"errors":0,"first_error":null,"barriers":[{}]}}"#,
                done.join(",")
            ));
            Played {
                lines,
                exit: Some(ProgramExit::Code(0)),
                stderr: String::new(),
                stops_on_line: false,
                ends_on_stop: false,
            }
        }

        fn with_frames(frames: [u64; 2], buffer_drops: u64) -> Self {
            Self {
                frames: Some((frames, buffer_drops)),
                ..Self::new(None)
            }
        }

        /// A fake for a network of several links: for each (place of the link, reverse) what the queue of the direction counted
        /// and how many frames reached the node at its end.
        fn with_links(counts: &[LinkCount]) -> Self {
            Self {
                per_link: counts.iter().copied().collect(),
                ..Self::new(None)
            }
        }

        fn counting(counts: [u64; 2]) -> Self {
            Self {
                counts: Some(counts),
                ..Self::new(None)
            }
        }
    }

    fn argument<'a>(argv: &'a [String], name: &str) -> &'a str {
        let at = argv.iter().position(|a| a == name).unwrap();
        &argv[at + 1]
    }

    impl Backend for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn capabilities(&self) -> Capabilities {
            Feature::ALL
                .iter()
                .fold(Capabilities::new("fake"), |caps, feature| {
                    caps.with(*feature, Support::Exact)
                })
        }

        fn create_node(&mut self, name: &netscenario_model::Name) -> Result<NodeHandle, Error> {
            self.calls.push(format!("node {name}"));
            Ok(NodeHandle(u32::try_from(self.calls.len()).unwrap()))
        }

        fn create_link(
            &mut self,
            name: &netscenario_model::Name,
            _: [NodeHandle; 2],
        ) -> Result<LinkHandle, Error> {
            self.calls.push(format!("link {name}"));
            self.links_made += 1;
            Ok(LinkHandle(self.links_made - 1))
        }

        fn create_link_through_fabric(
            &mut self,
            name: &netscenario_model::Name,
            between: [NodeHandle; 2],
        ) -> Result<LinkHandle, Error> {
            self.calls.push(format!("fabric link {name}"));
            self.links_made += 1;
            let _ = between;
            Ok(LinkHandle(self.links_made - 1))
        }

        fn queue_stats(
            &mut self,
            _: LinkHandle,
            direction: Direction,
        ) -> Result<Option<crate::backend::QueueStats>, Error> {
            // The fake counts a queue on the forward direction only
            Ok(
                (direction == Direction::Forward && self.queue_drops.is_some()).then(|| {
                    let mut stats = crate::backend::QueueStats::new("codel");
                    stats.packets = 8;
                    stats.bytes = 8_000;
                    stats.drops = self.queue_drops.unwrap_or(0);
                    stats.backlog_packets = 1;
                    stats.details.insert("ecn_mark".to_owned(), 0);
                    stats
                }),
            )
        }

        fn set_ip_families(&mut self, _: bool, _: bool) -> Result<(), Error> {
            Ok(())
        }

        fn address_v6(&self, link: LinkHandle, end: usize) -> Result<std::net::Ipv6Addr, Error> {
            Ok(std::net::Ipv6Addr::new(
                0xfd00,
                0x100,
                u16::try_from(link.0).unwrap(),
                0,
                0,
                0,
                0,
                1 + u16::try_from(end).unwrap(),
            ))
        }

        fn set_node_address(&mut self, _: NodeHandle, _: std::net::IpAddr) -> Result<(), Error> {
            Ok(())
        }

        fn set_link_mtu(&mut self, _: LinkHandle, _: u32) -> Result<(), Error> {
            Ok(())
        }

        fn set_forwarding(&mut self, _: NodeHandle, _: bool) -> Result<(), Error> {
            Ok(())
        }

        fn add_route(&mut self, _: NodeHandle, _: &crate::backend::NodeRoute) -> Result<(), Error> {
            Ok(())
        }

        fn apply(
            &mut self,
            _: LinkHandle,
            _: Direction,
            _: &Impairments,
            _: Option<u64>,
        ) -> Result<(), Error> {
            self.calls.push("apply".to_owned());
            Ok(())
        }

        fn change(
            &mut self,
            _: LinkHandle,
            direction: Direction,
            impairments: &Impairments,
        ) -> Result<(), Error> {
            if self.trouble == Some("change-fails") {
                return Err(Error::Backend {
                    backend: "fake".to_owned(),
                    action: "change the impairments of a direction".to_owned(),
                    message: "refused".to_owned(),
                });
            }
            let rate = impairments
                .rate
                .map_or("no rate".to_owned(), |rate| rate.to_string());
            self.calls.push(format!("change {direction:?} {rate}"));
            Ok(())
        }

        fn set_outage(
            &mut self,
            _: LinkHandle,
            direction: Direction,
            on: bool,
        ) -> Result<(), Error> {
            self.calls.push(format!(
                "outage {direction:?} {}",
                if on { "on" } else { "off" }
            ));
            Ok(())
        }

        fn queue_packets(
            &mut self,
            link: LinkHandle,
            direction: Direction,
        ) -> Result<Option<u64>, Error> {
            self.calls.push(format!("count {direction:?}"));
            if let Some((queue, _)) = self
                .per_link
                .get(&(link.0, direction == Direction::Reverse))
            {
                return Ok(Some(*queue));
            }
            Ok(self.counts.map(|counts| match direction {
                Direction::Forward => counts[0],
                Direction::Reverse => counts[1],
            }))
        }

        fn queue_backlog(
            &mut self,
            _: LinkHandle,
            direction: Direction,
        ) -> Result<Option<u64>, Error> {
            let Some((script, last)) = &mut self.backlog else {
                return Ok(None);
            };
            // The look is the one of the forward direction; the reverse has no queue
            Ok(Some(match direction {
                Direction::Forward => script.pop_front().unwrap_or(*last),
                Direction::Reverse => 0,
            }))
        }

        fn node_counts(
            &mut self,
            link: LinkHandle,
            direction: Direction,
        ) -> Result<Option<NodeCounts>, Error> {
            let key = (link.0, direction == Direction::Reverse);
            if let Some((_, frames)) = self.per_link.get(&key) {
                let reading = self.link_readings.entry(key).or_default();
                *reading += 1;
                return Ok(Some(if *reading == 1 {
                    NodeCounts::new(0, 0)
                } else {
                    NodeCounts::new(*frames, 0)
                }));
            }
            let way = usize::from(direction == Direction::Reverse);
            let reading = self.readings[way];
            self.readings[way] += 1;
            Ok(self.frames.map(|(frames, buffer_drops)| {
                if reading == 0 {
                    NodeCounts::new(0, 0)
                } else {
                    NodeCounts::new(frames[way], if way == 0 { buffer_drops } else { 0 })
                }
            }))
        }

        fn address(&self, link: LinkHandle, end: usize) -> Result<Ipv4Addr, Error> {
            Ok(Ipv4Addr::new(
                10,
                100,
                u8::try_from(link.0).unwrap(),
                1 + u8::try_from(end).unwrap(),
            ))
        }

        fn describe(&self) -> BackendInfo {
            BackendInfo {
                name: "fake".to_owned(),
                details: std::iter::once(("tc".to_owned(), "fake 1".to_owned())).collect(),
            }
        }

        fn start_program(
            &mut self,
            _: NodeHandle,
            program: &Program,
        ) -> Result<ProgramHandle, Error> {
            let argv = &program.argv;
            assert_eq!(program.memory_limit, Some(1 << 30), "agents are limited");
            self.calls
                .push(format!("start {} {}", argv[1], argument(argv, "--flow")));
            if argv.iter().any(|argument| argument == "--udp-checksum") {
                self.checksum
                    .push(format!("{} {}", argv[1], argument(argv, "--udp-checksum")));
                if argv[1] == "recv" {
                    self.damaged = 2;
                }
            }
            let played = match argv[1].as_str() {
                "recv" if self.trouble == Some("recv-exits-early") => Played {
                    lines: VecDeque::new(),
                    exit: Some(ProgramExit::Code(1)),
                    stderr: "cannot listen on 10.100.0.2:5000: address in use".to_owned(),
                    stops_on_line: false,
                    ends_on_stop: false,
                },
                "recv" => {
                    // Ten packets sent, the ones with sequence numbers 3 and 7 never arrived
                    let mut writer =
                        Writer::new(std::fs::File::create(argument(argv, "--out")).unwrap(), 0)
                            .unwrap();
                    for sequence in (0..10).filter(|s| *s != 3 && *s != 7) {
                        writer
                            .write(&Record {
                                sequence,
                                send_ns: 1_000 + sequence * 100_000_000,
                                receive_ns: 1_000 + sequence * 100_000_000 + 20_000_000,
                            })
                            .unwrap();
                    }
                    writer.finish().unwrap();
                    // The tap sees the packets that were delivered and the two that were not (3 and 7, one of
                    // them twice), and a frame whose sequence number was damaged into one that was never sent
                    let mut wire_error = String::new();
                    if argv.iter().any(|argument| argument == "--wire-out") {
                        if self.trouble == Some("no-tap") {
                            wire_error = r#","wire_error":"cannot read the frames of the interface: Operation not permitted (os error 1)""#.to_owned();
                        } else {
                            let mut tap = Writer::new(
                                std::fs::File::create(argument(argv, "--wire-out")).unwrap(),
                                0,
                            )
                            .unwrap();
                            for sequence in [0, 1, 2, 3, 3, 4, 5, 6, 7, 8, 9, 99] {
                                tap.write(&Record {
                                    sequence,
                                    send_ns: 1_000,
                                    receive_ns: 2_000,
                                })
                                .unwrap();
                            }
                            tap.finish().unwrap();
                            self.wire_packets = Some(12);
                        }
                    }
                    Played {
                        lines: VecDeque::from([format!(
                            r#"{{"event":"ready","flow":0,"address":"{}"{wire_error}}}"#,
                            argument(argv, "--bind")
                        )]),
                        exit: None,
                        stderr: String::new(),
                        stops_on_line: true,
                        ends_on_stop: false,
                    }
                }
                "send" if self.trouble == Some("send-fails") => Played {
                    lines: VecDeque::new(),
                    exit: Some(ProgramExit::Code(1)),
                    stderr: "cannot reach 10.100.0.2:5000: network is unreachable".to_owned(),
                    stops_on_line: false,
                    ends_on_stop: false,
                },
                "send" if self.trouble == Some("send-refused") => {
                    // Two of ten sequence numbers were refused by the kernel: eight packets went into the link
                    Played {
                        lines: VecDeque::from([r#"{"event":"sent","flow":0,"packets":8,"first_send_ns":1000,"last_send_ns":900001000,"max_lateness_ns":1234,"errors":2,"first_error":"Network is unreachable"}"#.to_owned()]),
                        exit: Some(ProgramExit::Code(0)),
                        stderr: String::new(),
                        stops_on_line: false,
                        ends_on_stop: false,
                    }
                }
                "send" if self.trouble == Some("sender-waits-for-stop") => Played {
                    lines: VecDeque::new(),
                    exit: None,
                    stderr: String::new(),
                    stops_on_line: false,
                    ends_on_stop: true,
                },
                "send" => self.play_sender(argv),
                other => panic!("unexpected {other}"),
            };
            self.programs.push(played);
            Ok(ProgramHandle(
                u32::try_from(self.programs.len() - 1).unwrap(),
            ))
        }

        fn poll_program(
            &mut self,
            program: ProgramHandle,
            _: Duration,
        ) -> Result<ProgramState, Error> {
            let played = &mut self.programs[program.0 as usize];
            Ok(ProgramState {
                lines: played.lines.drain(..).collect(),
                exit: played.exit,
                stderr: played.stderr.clone(),
                truncated: false,
                output: crate::backend::OutputStats::default(),
            })
        }

        fn send_line(&mut self, program: ProgramHandle, line: &str) -> Result<(), Error> {
            self.calls.push(format!("line {line}"));
            let played = &mut self.programs[program.0 as usize];
            if played.ends_on_stop && line == "stop" {
                played.lines.push_back(r#"{"event":"sent","flow":0,"packets":5,"first_send_ns":1000,"last_send_ns":400001000,"max_lateness_ns":1234,"errors":0,"first_error":null,"stopped":true}"#.to_owned());
                played.exit = Some(ProgramExit::Code(0));
            }
            if played.stops_on_line && line == "stop" {
                let wire = self.wire_packets.map_or_else(String::new, |packets| {
                    format!(r#","wire_packets":{packets}"#)
                });
                played.lines.push_back(format!(
                    r#"{{"event":"received","flow":0,"packets":8,"foreign":0,"corrupted":{}{wire},"first_receive_ns":1,"last_receive_ns":2}}"#,
                    self.damaged
                ));
                played.exit = Some(ProgramExit::Code(0));
            }
            Ok(())
        }

        fn teardown(&mut self) -> Result<(), Error> {
            self.calls.push("teardown".to_owned());
            Ok(())
        }
    }

    fn scenario() -> Scenario {
        Scenario::from_yaml(
            "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: { delay: 20ms, loss: { model: random, p: 20% } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n",
            "t.yaml",
        )
        .unwrap()
    }

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("netscenario-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn options(name: &str) -> RunOptions {
        let mut options = RunOptions::new(std::env::current_exe().unwrap(), dir(name));
        options.settle = Some(Duration::from_millis(1));
        options
    }

    /// The run records the scenario it ran with, so that it can be replayed (ENV-03): the file is the canonical scenario, its
    /// fingerprint is the one in the result, and a drawn seed is in the result and not in the scenario.
    #[test]
    fn a_run_records_the_scenario_it_ran_with() {
        let mut backend = Fake::new(None);
        let options = options("records-the-scenario");
        let mut drawn = scenario();
        drawn.seed = None;
        let result = run(&mut backend, &drawn, &options).unwrap();
        let recorded = crate::Recorded::load(&options.out_dir).unwrap();
        assert_eq!(recorded.scenario, drawn);
        assert_eq!(recorded.result.scenario.hash, result.scenario.hash);
        assert_eq!(recorded.scenario.seed, None);
        assert!(result.seed.drawn);
        assert_eq!(recorded.scenario_to_replay().seed, result.seed.value);
        assert!(options.out_dir.join(crate::SCENARIO_FILE).is_file());
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn the_receiver_starts_first_and_the_sender_after_it_and_the_network_is_torn_down_last() {
        let mut backend = Fake::new(None);
        let options = options("order");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert_eq!(
            backend.calls,
            [
                "node a",
                "node b",
                "link l",
                "apply",
                "apply",
                "start recv 0",
                "start send 0",
                "count Forward",
                "count Reverse",
                "line stop",
                "teardown"
            ]
        );
        assert_eq!(result.format, "netscenario-result");
        assert_eq!(result.seed.value, Some(5));
        assert!(!result.seed.drawn && result.seed.loss_reproducible);
        assert_eq!(result.provenance.backend, "fake");
        assert_eq!(result.addresses.len(), 2);
        assert!(result.scenario.hash.starts_with("sha256:") && result.scenario.hash.len() == 71);
        let workload = &result.workloads[0];
        // The address of the node on its loopback (D-078), the second node of the scenario
        assert_eq!(workload.to_address, "10.200.0.2:5000");
        assert_eq!(workload.path.as_ref().unwrap().links, ["l"]);
        assert_eq!(result.nodes.len(), 2);
        assert!(result.routes.iter().all(|route| !route.given));
        assert_eq!(
            (
                workload.stats.sent,
                workload.stats.received,
                workload.stats.lost
            ),
            (10, 8, 2)
        );
        assert!((workload.stats.loss_ratio - 0.2).abs() < f64::EPSILON);
        assert_eq!(workload.stats.delay.as_ref().unwrap().p50_ns, 20_000_000);
        assert_eq!(workload.sender.max_lateness_ns, 1234);
        assert_eq!(result.timing.settle_ns, 1_000_000);
        // The file on disk is the result that was returned
        let written = std::fs::read_to_string(options.out_dir.join("result.json")).unwrap();
        assert_eq!(RunResult::from_json(&written).unwrap(), result);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn a_queue_that_saw_the_packets_of_the_workloads_and_nothing_else_keeps_the_run_reproducible() {
        // The fake sender sent 10 packets from a to b: the forward queue saw those, the reverse queue none
        let mut backend = Fake::counting([10, 0]);
        let options = options("queues-agree");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert!(result.seed.loss_reproducible, "{:?}", result.seed);
        assert_eq!(
            result.seed.queue_checks,
            [
                QueueCheck {
                    link: "l".to_owned(),
                    direction: "forward".to_owned(),
                    expected: 10,
                    counted: 10
                },
                QueueCheck {
                    link: "l".to_owned(),
                    direction: "reverse".to_owned(),
                    expected: 0,
                    counted: 0
                }
            ]
        );
        // The counting happens after the senders are done and before the receivers are stopped
        let calls = backend.calls.join(",");
        assert!(
            calls.contains("start send 0,count Forward,count Reverse,line stop"),
            "{calls}"
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canary: a queue that counts a packet more, or fewer, or one in the direction where nothing was sent,
    /// makes the run not reproducible and says which queue.
    #[test]
    fn canary_a_packet_that_the_workloads_did_not_send_makes_the_run_not_reproducible() {
        for (counts, wrong) in [
            ([11, 0], "forward"),
            ([9, 0], "forward"),
            ([10, 1], "reverse"),
        ] {
            let mut backend = Fake::counting(counts);
            let options = options("queues-differ");
            let result = run(&mut backend, &scenario(), &options).unwrap();
            assert!(!result.seed.loss_reproducible, "{counts:?}");
            let differing: Vec<&str> = result
                .seed
                .queue_checks
                .iter()
                .filter(|check| !check.agrees())
                .map(|check| check.direction.as_str())
                .collect();
            assert_eq!(differing, [wrong], "{counts:?}");
            let _ = std::fs::remove_dir_all(&options.out_dir);
        }
    }

    /// The result says why a run cannot be repeated exactly: a queue that counted a packet too many, and a
    /// scenario with corruption, where the kernel takes the place of the flipped bit from its own generator.
    #[test]
    fn a_run_that_cannot_be_repeated_exactly_says_why() {
        let mut backend = Fake::counting([11, 0]);
        let options = options("reasons-queue");
        let seed = run(&mut backend, &scenario(), &options).unwrap().seed;
        assert!(!seed.loss_reproducible);
        assert_eq!(seed.not_reproducible_because.len(), 1);
        assert!(
            seed.not_reproducible_because[0]
                .contains("counted 11 packets and the workloads sent 10"),
            "{:?}",
            seed.not_reproducible_because
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);

        let yaml = "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { corrupt: 10% }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n";
        let mut backend = Fake::new(None);
        let options = self::options("reasons-corrupt");
        let seed = run(
            &mut backend,
            &Scenario::from_yaml(yaml, "t.yaml").unwrap(),
            &options,
        )
        .unwrap()
        .seed;
        assert!(!seed.loss_reproducible);
        assert!(
            seed.not_reproducible_because
                .iter()
                .any(|reason| reason.contains("corrupts packets")),
            "{:?}",
            seed.not_reproducible_because
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// An outage is placed by the clock in the time of the receiver, and the result says that the packet at its edges
    /// can differ (D-064). The run does the two steps of it, and a scenario without an outage gives no such reason.
    #[test]
    fn an_outage_is_two_steps_and_the_result_says_that_its_edges_are_placed_by_the_clock() {
        let yaml = "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { delay: 10ms }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\ntimeline:\n  - { at: 300ms, link: l, outage: 300ms, direction: forward }\n";
        let mut backend = Fake::new(None);
        let options = self::options("outage-reason");
        let result = run(
            &mut backend,
            &Scenario::from_yaml(yaml, "t.yaml").unwrap(),
            &options,
        )
        .unwrap();
        let outage: Vec<&String> = backend
            .calls
            .iter()
            .filter(|call| call.starts_with("outage"))
            .collect();
        assert_eq!(outage, ["outage Forward on", "outage Forward off"]);
        let kinds: Vec<(&str, Option<u64>)> = result
            .events
            .iter()
            .map(|event| (event.kind.as_str(), event.packet))
            .collect();
        // By the clock, never at a barrier: no packet of the sender waits for the window of the receiver
        assert_eq!(kinds, [("outage-start", None), ("outage-end", None)]);
        assert!(!result.seed.loss_reproducible);
        assert!(
            result
                .seed
                .not_reproducible_because
                .iter()
                .any(|reason| reason.contains("has an outage")),
            "{:?}",
            result.seed
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canary: a run that can be repeated gives no reason, and a scenario without corruption gives none for it.
    #[test]
    fn a_run_that_can_be_repeated_gives_no_reason() {
        let mut backend = Fake::counting([10, 0]);
        let options = options("reasons-none");
        let seed = run(&mut backend, &scenario(), &options).unwrap().seed;
        assert!(seed.loss_reproducible);
        assert_eq!(seed.not_reproducible_because.len(), 0);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A packet that the kernel refused to take did not go into the queue, and is not in `packets` either, so the
    /// queue is expected to have counted `packets` and no fewer (the check used to subtract the errors again).
    #[test]
    fn a_packet_that_the_kernel_refused_does_not_make_the_queue_check_disagree() {
        let mut backend = Fake::new(Some("send-refused"));
        backend.counts = Some([8, 0]);
        let options = options("queues-refused");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert_eq!(result.seed.queue_checks[0].expected, 8);
        assert!(result.seed.loss_reproducible, "{:?}", result.seed);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A chain of two links with a router in the middle and a random loss on the second, one workload from end to end.
    fn chain(first: &str, workloads: &str) -> Scenario {
        Scenario::from_yaml(
            &format!(
                "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, {{ id: r, role: router }}, b]\n  links:\n    - {{ id: l1, between: [a, r]{first} }}\n    - {{ id: l2, between: [r, b], both: {{ loss: {{ model: random, p: 20% }} }} }}\nworkloads:\n{workloads}"
            ),
            "chain.yaml",
        )
        .unwrap()
    }

    const CHAIN_FLOW: &str =
        "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n";

    /// What each queue and each node of the chain counts when the fake sender sent 10 packets: 9 reached the router, the
    /// second queue got those 9 and let 8 through.
    const COUNTED: [LinkCount; 4] = [
        ((0, false), (0, 9)),
        ((0, true), (0, 0)),
        ((1, false), (9, 8)),
        ((1, true), (0, 0)),
    ];

    /// D-040 along a path: what enters the second link is what arrived at the router, so the packet is accounted for link
    /// by link (lost in the first queue, lost in the second, delivered), the queue of the second link is held to the packets
    /// that arrived at the router, and the run stays reproducible.
    #[test]
    fn a_packet_is_accounted_for_link_by_link_along_its_path() {
        let mut backend = Fake::with_links(&COUNTED);
        let options = options("path-tally");
        let result = run(&mut backend, &chain("", CHAIN_FLOW), &options).unwrap();
        let tallies: Vec<_> = result
            .directions
            .iter()
            .map(|t| {
                (
                    t.link.as_str(),
                    t.direction.as_str(),
                    t.sent,
                    t.reached_node,
                    t.delivered,
                    t.lost_in_queue,
                    t.dropped_corrupted,
                )
            })
            .collect();
        // The router takes what arrives and forwards it, so the first link has no packet dropped by a node; the receivers
        // are at the end of the second
        assert_eq!(
            tallies,
            [
                ("l1", "forward", 10, 9, 9, Some(1), 0),
                ("l2", "forward", 9, 8, 8, Some(1), 0)
            ]
        );
        assert!(result.seed.loss_reproducible, "{:?}", result.seed);
        assert_eq!(
            result.seed.queue_checks,
            [
                QueueCheck {
                    link: "l2".to_owned(),
                    direction: "forward".to_owned(),
                    expected: 9,
                    counted: 9
                },
                QueueCheck {
                    link: "l2".to_owned(),
                    direction: "reverse".to_owned(),
                    expected: 0,
                    counted: 0
                }
            ]
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canary for the test above: a second queue that counted a packet more than arrived at the router, or fewer, is a queue
    /// that something else entered (or that a packet did not enter), and the run says so.
    #[test]
    fn canary_a_queue_after_the_first_link_that_counted_another_number_is_not_reproducible() {
        for queue in [8, 10] {
            let mut counted = COUNTED;
            counted[2] = ((1, false), (queue, 8));
            let mut backend = Fake::with_links(&counted);
            let options = options("path-canary");
            let result = run(&mut backend, &chain("", CHAIN_FLOW), &options).unwrap();
            assert!(!result.seed.loss_reproducible, "{queue}");
            assert!(
                result
                    .seed
                    .not_reproducible_because
                    .iter()
                    .any(|why| why.contains("link 'l2'") && why.contains("counted")),
                "{:?}",
                result.seed.not_reproducible_because
            );
            let _ = std::fs::remove_dir_all(&options.out_dir);
        }
    }

    /// Two workloads that meet in the second link and have shared the first: everything that arrived at the router enters the
    /// second queue, so it has a tally and a check, but the order in which the two reach it is not fixed, and the result says
    /// why the loss is not reproducible.
    #[test]
    fn workloads_that_share_two_links_are_accounted_but_not_reproducible() {
        let two = format!(
            "{CHAIN_FLOW}  - {{ id: v, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }}\n"
        );
        let mut backend = Fake::with_links(&COUNTED);
        let options = options("path-share");
        let result = run(&mut backend, &chain("", &two), &options).unwrap();
        let second = result
            .directions
            .iter()
            .find(|t| (t.link.as_str(), t.direction.as_str()) == ("l2", "forward"))
            .unwrap();
        // 9 frames arrived at the router, all for the second link
        assert_eq!((second.sent, second.reached_node), (9, 8));
        assert!(
            result
                .seed
                .queue_checks
                .iter()
                .any(|c| c.link == "l2" && c.direction == "forward" && c.expected == 9),
            "{:?}",
            result.seed.queue_checks
        );
        assert!(!result.seed.loss_reproducible);
        assert!(
            result
                .seed
                .not_reproducible_because
                .iter()
                .any(|why| why.contains("'w', 'v'") && why.contains("link 'l2'")),
            "{:?}",
            result.seed.not_reproducible_because
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Two workloads that share the first link and part at the router: the frames that arrive there are for two links, so
    /// the run cannot tell which enter the second queue. It has no tally and no check, and the result says why.
    #[test]
    fn workloads_that_part_at_a_router_are_not_accounted_after_it_and_not_reproducible() {
        let yaml = format!(
            "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, {{ id: r, role: router }}, b, c]\n  links:\n    - {{ id: l1, between: [a, r] }}\n    - {{ id: l2, between: [r, b], both: {{ loss: {{ model: random, p: 20% }} }} }}\n    - {{ id: l3, between: [r, c] }}\nworkloads:\n{CHAIN_FLOW}  - {{ id: v, kind: udp-cbr, from: a, to: c, rate: 800kbit, packet-size: 1000b }}\n"
        );
        let scenario = Scenario::from_yaml(&yaml, "part.yaml").unwrap();
        let mut backend = Fake::with_links(&[
            ((0, false), (0, 18)),
            ((0, true), (0, 0)),
            ((1, false), (9, 8)),
            ((1, true), (0, 0)),
            ((2, false), (0, 8)),
            ((2, true), (0, 0)),
        ]);
        let options = options("path-part");
        let result = run(&mut backend, &scenario, &options).unwrap();
        assert!(
            result.directions.iter().all(|t| t.link != "l2"),
            "{:?}",
            result.directions
        );
        assert!(
            result
                .seed
                .queue_checks
                .iter()
                .all(|c| (c.link.as_str(), c.direction.as_str()) != ("l2", "forward")),
            "{:?}",
            result.seed.queue_checks
        );
        assert!(!result.seed.loss_reproducible);
        assert!(
            result
                .seed
                .not_reproducible_because
                .iter()
                .any(|why| why.contains("link 'l2'") && why.contains("other workloads")),
            "{:?}",
            result.seed.not_reproducible_because
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A link before the queue that can change the order of the packets (a jitter) makes the packet that draws each number
    /// depend on the timing: the queue is counted, but the run is not reproducible, and the reason names the link.
    #[test]
    fn a_jitter_before_a_random_queue_makes_the_run_not_reproducible() {
        let mut backend = Fake::with_links(&COUNTED);
        let options = options("path-jitter");
        let result = run(
            &mut backend,
            &chain(
                ", forward: { delay: 20ms, jitter: { amount: 5ms } }",
                CHAIN_FLOW,
            ),
            &options,
        )
        .unwrap();
        assert!(!result.seed.loss_reproducible);
        assert!(
            result
                .seed
                .not_reproducible_because
                .iter()
                .any(|why| why.contains("after link 'l1'") && why.contains("order")),
            "{:?}",
            result.seed.not_reproducible_because
        );
        // The count still agrees; it is the order that is not fixed
        assert!(result.seed.queue_checks.iter().all(QueueCheck::agrees));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// After a link that corrupts, a router drops a packet whose header was damaged, and that packet arrived at the node but
    /// did not enter the next queue: what the next queue lost cannot be told, and the result says `None` instead of a number.
    #[test]
    fn after_a_corrupting_link_the_loss_of_the_next_queue_is_not_told() {
        let mut backend = Fake::with_links(&COUNTED);
        let options = options("path-corrupt");
        let result = run(
            &mut backend,
            &chain(", forward: { corrupt: 1% }", CHAIN_FLOW),
            &options,
        )
        .unwrap();
        let tally = |link: &str| {
            result
                .directions
                .iter()
                .find(|t| t.link == link)
                .unwrap()
                .lost_in_queue
        };
        assert_eq!(tally("l1"), Some(1));
        assert_eq!(tally("l2"), None);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A link with a `queue` is built through the fabric, and the result lists what the kernel counted in its queue (PRB-10): the
    /// build, the discipline of the scenario and of the kernel, and the counters. A link without a queue is not listed.
    #[test]
    fn a_link_with_a_queue_is_built_through_the_fabric_and_the_result_has_its_counters() {
        let queued = Scenario::from_yaml(
            "version: 0\nname: t\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { rate: 2mbit, queue: { discipline: codel, limit: 100p } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n",
            "queued.yaml",
        )
        .unwrap();
        let mut backend = Fake::new(None);
        backend.queue_drops = Some(2);
        let options = options("queue-fabric");
        let result = run(&mut backend, &queued, &options).unwrap();
        assert!(
            backend.calls.contains(&"fabric link l".to_owned()),
            "{:?}",
            backend.calls
        );
        let [queue] = result.queues.as_slice() else {
            panic!("{:?}", result.queues);
        };
        assert_eq!(
            (
                queue.link.as_str(),
                queue.direction.as_str(),
                queue.build.as_str()
            ),
            ("l", "forward", "fabric")
        );
        assert_eq!(
            (queue.discipline.as_str(), queue.kind.as_str()),
            ("codel", "codel")
        );
        assert_eq!(
            (queue.packets, queue.drops, queue.backlog_packets),
            (8, 2, 1)
        );
        assert_eq!(queue.details["ecn_mark"], 0);
        let _ = std::fs::remove_dir_all(&options.out_dir);

        // A link without a queue is built directly and has no entry
        let mut backend = Fake::new(None);
        backend.queue_drops = Some(2);
        let options = self::options("queue-direct");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert_eq!(result.queues, []);
        assert!(!backend.calls.iter().any(|call| call.starts_with("fabric")));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// The counters of a queue are looked at once per interval while the workloads go, in the order of time, and never earlier
    /// than one interval after the last look (PRB-10). A look before the first interval has passed reads nothing.
    #[test]
    fn the_queues_are_looked_at_once_per_interval() {
        use std::time::Duration;
        const INTERVAL_NS: u64 = 40_000_000;
        let mut backend = Fake::new(None);
        backend.queue_drops = Some(1);
        let mut sampler = crate::queues::Sampler::new([(
            "l".to_owned(),
            "forward",
            LinkHandle(0),
            Direction::Forward,
        )])
        .with_interval(INTERVAL_NS);
        let begin_ns = netscenario_agent::clock::monotonic_ns();
        sampler.look(&mut backend, begin_ns).unwrap();
        assert!(sampler.samples_of("l", "forward").is_empty(), "too early");
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(25));
            sampler.look(&mut backend, begin_ns).unwrap();
        }
        let seen = sampler.samples_of("l", "forward");
        // 100 ms at 40 ms: the looks at 40 ms and 80 ms, never more than three even when the machine stalls
        assert!((1..=3).contains(&seen.len()), "{seen:?}");
        assert!(seen[0].t_ns >= INTERVAL_NS, "{seen:?}");
        assert!(
            seen.windows(2)
                .all(|pair| pair[1].t_ns >= pair[0].t_ns + INTERVAL_NS),
            "one look per interval: {seen:?}"
        );
        assert!(seen.iter().all(|sample| sample.drops == 1), "{seen:?}");
        assert_eq!(sampler.samples_of("l", "reverse"), Vec::new());
    }

    /// A backend that cannot say has no samples, and a sampler without a queue never asks.
    #[test]
    fn a_backend_without_counters_leaves_the_samples_empty() {
        let mut backend = Fake::new(None);
        let mut sampler = crate::queues::Sampler::new([(
            "l".to_owned(),
            "forward",
            LinkHandle(0),
            Direction::Forward,
        )])
        .with_interval(1);
        sampler.look(&mut backend, 0).unwrap();
        assert_eq!(sampler.samples_of("l", "forward"), Vec::new());
    }

    /// A queue sees frames (D-081): packets of 1,400 bytes over an MTU of 600 make 3 fragments each, so the queue of the link
    /// has counted 30 for the 10 packets the fake sent, the check agrees, and the result says how many frames a packet makes.
    #[test]
    fn the_queue_of_a_link_that_cuts_the_packets_is_held_to_frames() {
        let cut = Scenario::from_yaml(
            "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      mtu: 600\n      both: { loss: { model: random, p: 20% } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n",
            "cut.yaml",
        )
        .unwrap();
        let mut backend = Fake::counting([20, 0]);
        let options = options("frames");
        let result = run(&mut backend, &cut, &options).unwrap();
        // 1000 bytes and 8 of UDP in fragments of 576: 2 frames, so 10 packets are 20 frames
        assert_eq!(result.workloads[0].frames_per_packet, 2);
        assert!(result.seed.loss_reproducible, "{:?}", result.seed);
        assert_eq!(result.seed.queue_checks[0].expected, 20);
        // Not accounted by packets: no tally
        assert!(result.directions.is_empty(), "{:?}", result.directions);
        let _ = std::fs::remove_dir_all(&options.out_dir);

        // Canary: a queue that counted the packets and not the frames is not the queue of this scenario
        let mut backend = Fake::counting([10, 0]);
        let options = self::options("frames-wrong");
        let result = run(&mut backend, &cut, &options).unwrap();
        assert!(!result.seed.loss_reproducible);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// In a network with both families a workload uses the family it asks for, and IPv4 where it says nothing (D-081). The
    /// address of an IPv6 receiver is written with brackets, as the agent reads it, and the result lists the addresses of
    /// both families.
    #[test]
    fn a_workload_uses_its_own_family_in_a_dual_stack_network() {
        let dual = |workload: &str| {
            Scenario::from_yaml(
                &format!("version: 0\nname: t\nduration: 1s\ntopology:\n  ip: dual\n  nodes: [a, b]\n  links:\n    - {{ id: l, between: [a, b] }}\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b{workload} }}\n"),
                "dual.yaml",
            )
            .unwrap()
        };
        let mut backend = Fake::new(None);
        let options = options("family-v6");
        let result = run(&mut backend, &dual(", ip: v6"), &options).unwrap();
        let workload = &result.workloads[0];
        assert_eq!(
            (workload.from_address.as_str(), workload.to_address.as_str()),
            ("fd00:200::1", "[fd00:200::2]:5000")
        );
        assert_eq!(result.nodes[0].address, "10.200.0.1");
        assert_eq!(result.nodes[0].address_v6.as_deref(), Some("fd00:200::1"));
        assert_eq!(result.addresses.len(), 4, "{:?}", result.addresses);
        assert!(
            backend
                .calls
                .iter()
                .any(|call| call.starts_with("start recv"))
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);

        let mut backend = Fake::new(None);
        let options = self::options("family-default");
        let result = run(&mut backend, &dual(""), &options).unwrap();
        assert_eq!(result.workloads[0].to_address, "10.200.0.2:5000");
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// D-040: the packets that were sent are lost in the queue, dropped by the node, or delivered, and the
    /// result says how many of each. The fake sent 10 and the receiver got 8; the node counted 9 frames.
    #[test]
    fn every_packet_is_accounted_for_as_lost_in_the_queue_dropped_by_the_node_or_delivered() {
        let mut backend = Fake::with_frames([9, 0], 0);
        let options = options("tally");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        let [tally] = result.directions.as_slice() else {
            panic!("{:?}", result.directions);
        };
        assert_eq!(
            (tally.link.as_str(), tally.direction.as_str()),
            ("l", "forward")
        );
        assert_eq!(
            (
                tally.sent,
                tally.reached_node,
                tally.delivered,
                tally.lost_in_queue,
                tally.dropped_corrupted,
                tally.receive_buffer_drops
            ),
            (10, 9, 8, Some(1), 1, 0)
        );
        // lost_in_queue + dropped_corrupted + delivered = sent
        assert_eq!(
            tally.lost_in_queue.unwrap() + tally.dropped_corrupted + tally.delivered,
            tally.sent
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canary: a datagram that the node dropped for a full buffer is not called corruption, and a frame that
    /// arrived and was not delivered is not called lost.
    const WITH_TIMELINE: &str = "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { rate: 4mbit }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\ntimeline:\n  - { at: 150ms, link: l, set: { forward: { rate: 1mbit } } }\n  - { at: 300ms, link: l, set: { both: { rate: null } } }\n";

    /// With the barriers off every step is done by the clock, in order at its moment, also after every sender has
    /// ended (the events are part of the scenario up to its end); each is recorded with when it was due and when it was
    /// done, and the observer is told. Nothing is done before its moment.
    #[test]
    fn with_the_barriers_off_the_steps_of_the_timeline_are_done_in_order_at_their_moments() {
        let scenario = Scenario::from_yaml(WITH_TIMELINE, "t.yaml").unwrap();
        let told = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = {
            let told = Arc::clone(&told);
            Arc::new(move |event: &RunEvent| {
                if let RunEvent::StepDone { event, lateness } = event {
                    told.lock().unwrap().push((*event, *lateness));
                }
            })
        };
        let mut backend = Fake::new(None);
        let mut options = options("steps").with_observer(observer);
        options.barriers = false;
        let result = run(&mut backend, &scenario, &options).unwrap();
        assert!(backend.barriers.is_empty(), "no sender was given a barrier");
        assert!(result.events.iter().all(|e| e.packet.is_none()));
        let changes: Vec<&String> = backend
            .calls
            .iter()
            .filter(|call| call.starts_with("change"))
            .collect();
        assert_eq!(
            changes,
            [
                "change Forward 1mbit",
                "change Forward no rate",
                "change Reverse no rate"
            ]
        );
        let events = &result.events;
        assert_eq!(
            events
                .iter()
                .map(|e| (e.event, e.kind.as_str(), e.direction.as_deref()))
                .collect::<Vec<_>>(),
            [
                (0, "set", Some("forward")),
                (1, "set", Some("forward")),
                (1, "set", Some("reverse"))
            ]
        );
        assert_eq!(
            events.iter().map(|e| e.scheduled_ns).collect::<Vec<_>>(),
            [150_000_000, 300_000_000, 300_000_000]
        );
        for event in events {
            // Never early, and on a quiet machine a few milliseconds at most; the room is for a busy one
            assert!(event.started_ns >= event.scheduled_ns, "{event:?}");
            assert!(event.applied_ns >= event.started_ns, "{event:?}");
            assert!(
                event.applied_ns - event.scheduled_ns < 500_000_000,
                "{event:?}"
            );
        }
        assert_eq!(
            told.lock()
                .unwrap()
                .iter()
                .map(|(e, _)| *e)
                .collect::<Vec<_>>(),
            [0, 1, 1]
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// The steps of a direction that a workload sends into are done at barriers (D-038): the sender is told before
    /// which packet (100 packets a second, so the moment 150 ms is packet 15 and 300 ms is packet 30), the loop does
    /// the step when the sender asks, and the record says which packet and how long the sender waited. A step of a
    /// direction that no workload sends into is done by the clock, and the loop waits for it.
    #[test]
    fn the_steps_of_a_direction_with_a_workload_are_done_at_barriers_and_the_others_by_the_clock() {
        let scenario = Scenario::from_yaml(WITH_TIMELINE, "t.yaml").unwrap();
        let mut backend = Fake::new(None);
        let options = options("barriers");
        let result = run(&mut backend, &scenario, &options).unwrap();
        assert_eq!(backend.barriers, [(15, 0), (30, 1)]);
        let answered: Vec<&String> = backend
            .calls
            .iter()
            .filter(|call| call.starts_with("line go"))
            .collect();
        assert_eq!(answered, ["line go 0", "line go 1"]);
        let events = &result.events;
        assert_eq!(
            events
                .iter()
                .map(|e| (e.event, e.direction.as_deref(), e.packet, e.held_ns))
                .collect::<Vec<_>>(),
            [
                (0, Some("forward"), Some(15), Some(1000)),
                (1, Some("forward"), Some(30), Some(2000)),
                // No workload sends the other way: the clock does it at 300 ms
                (1, Some("reverse"), None, None)
            ]
        );
        assert!(events[2].started_ns >= events[2].scheduled_ns);
        // A change of the rate draws nothing differently, so nothing is said against the lost packets
        assert!(result.seed.loss_reproducible, "{:?}", result.seed);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    fn with_loss_step(workloads: &str, step: &str) -> Scenario {
        Scenario::from_yaml(
            &format!(
                "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {{ loss: {{ model: random, p: 5% }} }}\nworkloads:\n{workloads}timeline:\n  - {{ at: 150ms, link: l, set: {{ {step}: {{ loss: {{ model: random, p: 20% }} }} }} }}\n"
            ),
            "t.yaml",
        )
        .unwrap()
    }

    const ONE: &str =
        "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n";

    /// What the run says about the lost packets of a seed when a step changes the loss: nothing against it where a barrier
    /// holds the sender (the guarantee of D-038), and a reason where the step is left to the clock: no workload sends
    /// into the direction, two do, or the barriers are off.
    #[test]
    fn a_step_that_changes_the_loss_is_reproducible_only_where_a_sender_waits_for_it() {
        let reasons = |scenario: &Scenario, barriers: bool, name: &str| {
            let mut backend = Fake::new(None);
            let mut options = options(name);
            options.barriers = barriers;
            let result = run(&mut backend, scenario, &options).unwrap();
            let _ = std::fs::remove_dir_all(&options.out_dir);
            (
                result.seed.loss_reproducible,
                result.seed.not_reproducible_because,
            )
        };
        // One workload sends into the direction that the step changes: the barrier holds, nothing is said
        let (reproducible, because) = reasons(&with_loss_step(ONE, "forward"), true, "r1");
        assert!(reproducible && because.is_empty(), "{because:?}");
        // The other direction has no workload
        let (reproducible, because) = reasons(&with_loss_step(ONE, "reverse"), true, "r2");
        assert!(!reproducible);
        assert!(
            because[0].contains("no workload of the scenario sends into that direction"),
            "{because:?}"
        );
        // Two workloads in one direction
        let two = format!(
            "{ONE}  - {{ id: v, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }}\n"
        );
        let (reproducible, because) = reasons(&with_loss_step(&two, "forward"), true, "r3");
        assert!(!reproducible);
        assert!(
            because
                .iter()
                .any(|why| why.contains("send into link 'l' (forward)")),
            "{because:?}"
        );
        // The barriers off: the clock does it, and the run says so
        let (reproducible, because) = reasons(&with_loss_step(ONE, "forward"), false, "r4");
        assert!(!reproducible);
        assert!(
            because
                .iter()
                .any(|why| why.contains("barriers are switched off")),
            "{because:?}"
        );
    }

    /// Without a wait that is set, the run waits until the queues are empty and then for the receivers to read (D-067):
    /// the fake queue holds 5, 3 and 1 packets at the first three looks and none after that, so the wait is the three
    /// looks and the grace, far less than the second and more that the delays give. It does not stop at the first
    /// empty look when a packet comes back (a queue that is empty for a moment).
    #[test]
    fn the_run_waits_until_the_queues_are_empty_and_then_for_the_grace() {
        let mut backend = Fake::with_backlog(&[5, 3, 1, 0, 2], 0);
        let mut options = options("settle-drain");
        options.settle = None;
        let result = run(&mut backend, &scenario(), &options).unwrap();
        let waited = Duration::from_nanos(result.timing.settle_ns);
        // Five looks of 50 ms with the last two apart, then 300 ms of empty queue
        assert!(
            waited >= Duration::from_millis(5 * 50 + 300 - 100),
            "{waited:?}"
        );
        assert!(waited < Duration::from_secs(1), "{waited:?}");
        assert!(result.notes.is_empty(), "{:?}", result.notes);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canaries: a backend that cannot count the packets in its queues gets the time that the delays give (one second
    /// and twice the longest hold), and a wait that is set is that long whatever the queues say.
    #[test]
    fn a_backend_that_cannot_count_gets_the_time_of_the_delays_and_a_set_wait_is_kept() {
        let mut backend = Fake::new(None);
        let mut options = options("settle-estimate");
        options.settle = None;
        let result = run(&mut backend, &scenario(), &options).unwrap();
        // The scenario has a delay of 20 ms: 1 s and twice 20 ms
        assert_eq!(result.timing.settle_ns / 1_000_000, 1040);
        let _ = std::fs::remove_dir_all(&options.out_dir);

        let mut backend = Fake::with_backlog(&[], 7);
        let mut options = self::options("settle-set");
        options.settle = Some(Duration::from_millis(150));
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert_eq!(result.timing.settle_ns, 150_000_000);
        assert_eq!(result.notes, Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A queue that does not empty ends the wait at the ceiling, and the result says how many packets were left.
    #[test]
    fn a_queue_that_does_not_empty_ends_the_wait_at_the_ceiling_and_the_result_says_so() {
        let mut backend = Fake::with_backlog(&[], 7);
        let mut options = options("settle-ceiling");
        options.settle = None;
        let scenario = scenario();
        let provisioned = provision(&mut backend, &scenario).unwrap();
        let control = Control::new(&options);
        let (waited, note) = settle_with(
            &mut backend,
            &scenario,
            &provisioned,
            &options,
            &control,
            Duration::from_millis(400),
        )
        .unwrap();
        assert!(
            waited >= Duration::from_millis(400) && waited < Duration::from_secs(2),
            "{waited:?}"
        );
        let note = note.expect("a note");
        assert!(note.contains("7 packets still in them"), "{note}");
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// The series of a workload is written to a file of its own, the result says where and how it is cut, and the
    /// numbers add up to the statistics (D-070).
    #[test]
    fn the_series_is_in_a_file_of_its_own_and_adds_up_to_the_statistics() {
        let mut backend = Fake::new(None);
        let options = options("series-file");
        let mut quarter = scenario();
        quarter.measure.series = Some(netscenario_model::SeriesSettings::default());
        quarter.measure.series.as_mut().unwrap().bin = Some("250ms".parse().unwrap());
        let result = run(&mut backend, &quarter, &options).unwrap();
        let workload = &result.workloads[0];
        let info = workload.series.as_ref().expect("the series");
        assert_eq!(
            (info.bin_ns, info.file.as_str()),
            (250_000_000, "series-w.jsonl")
        );
        let text = std::fs::read_to_string(options.out_dir.join(&info.file)).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len() as u64, info.bins);
        assert!(lines.iter().all(|line| line["workload"] == "w"));
        let sent: u64 = lines
            .iter()
            .map(|l| l["by_send"]["sent"].as_u64().unwrap())
            .sum();
        let received: u64 = lines
            .iter()
            .map(|l| l["by_send"]["received"].as_u64().unwrap())
            .sum();
        assert_eq!(
            (sent, received),
            (workload.stats.sent, workload.stats.received)
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Settings of the section `measure` that make no sense end the run before anything is built, with the path of the key
    /// (the model checks them, D-073).
    #[test]
    fn measure_settings_that_make_no_sense_are_refused_before_the_run() {
        let rejected = |change: &dyn Fn(&mut Scenario), word: &str| {
            let mut backend = Fake::new(None);
            let options = options("series-refused");
            let mut bad = scenario();
            change(&mut bad);
            let error = run(&mut backend, &bad, &options).unwrap_err().to_string();
            assert!(error.contains(word), "{error}");
            assert!(backend.calls.is_empty(), "{:?}", backend.calls);
            let _ = std::fs::remove_dir_all(&options.out_dir);
        };
        let bin = |scenario: &mut Scenario, text: &str| {
            let mut series = netscenario_model::SeriesSettings::default();
            series.bin = Some(text.parse().unwrap());
            scenario.measure.series = Some(series);
        };
        rejected(&|s| bin(s, "500us"), "measure.series.bin");
        rejected(
            &|s| {
                let mut recovery = netscenario_model::RecoverySettings::default();
                recovery.window_bins = Some(0);
                s.measure.recovery = Some(recovery);
            },
            "measure.recovery.window-bins",
        );
        rejected(
            &|s| s.measure.capture = vec!["nowhere".parse().unwrap()],
            "measure.capture[0]",
        );
    }

    /// The time to recover is worked out for the steps that concern the workload: a step of the direction it sends in
    /// and a step of the whole link, not a step of the other direction; and the start of an outage has none.
    #[test]
    fn the_recoveries_are_of_the_steps_that_concern_the_workload() {
        let yaml = "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: { delay: 5ms }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\ntimeline:\n  - { at: 200ms, link: l, set: { forward: { delay: 10ms } } }\n  - { at: 300ms, link: l, set: { reverse: { delay: 10ms } } }\n  - { at: 500ms, link: l, outage: 100ms, direction: forward }\nmeasure:\n  series: { bin: 100ms }\n";
        let mut backend = Fake::new(None);
        let options = self::options("series-recoveries");
        let result = run(
            &mut backend,
            &Scenario::from_yaml(yaml, "t.yaml").unwrap(),
            &options,
        )
        .unwrap();
        let kinds: Vec<(usize, &str)> = result.workloads[0]
            .recoveries
            .iter()
            .map(|r| (r.event, r.kind.as_str()))
            .collect();
        // The set of the forward direction (event 0) and the end of the outage (event 2); not the set of the reverse
        assert_eq!(kinds, [(0, "set"), (2, "outage-end")]);
        let end = &result.workloads[0].recoveries[1];
        assert_eq!(end.disturbance_ns, 500_000_000);
        assert_eq!(end.from_ns, 600_000_000);
        assert_eq!((end.baseline_bins, end.window_bins), (5, 3));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A cancel that keeps the result (D-066): the senders are told to stop, the run goes on through the settling and
    /// the collecting, and the result says when the workloads were stopped. The sender here never ends by itself.
    #[test]
    fn a_cancel_that_keeps_the_result_stops_the_senders_and_returns_what_was_recorded() {
        let token = CancelToken::new();
        let heard = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let observer = {
            let (token, heard) = (token.clone(), Arc::clone(&heard));
            Arc::new(move |event: &RunEvent| {
                let name = match event {
                    RunEvent::Started { .. } => {
                        // The user presses Ctrl-C just as the workloads begin
                        token.cancel();
                        "started"
                    }
                    RunEvent::Interrupted => "interrupted",
                    RunEvent::Settling { .. } => "settling",
                    RunEvent::Collecting => "collecting",
                    RunEvent::Finished => "finished",
                    _ => return,
                };
                heard.lock().unwrap().push(name);
            })
        };
        let mut backend = Fake::new(Some("sender-waits-for-stop"));
        let options = self::options("interrupt-kept")
            .with_observer(observer)
            .with_cancel(token)
            .with_partial_on_cancel();
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert!(result.timing.interrupted_ns.is_some());
        assert_eq!(
            *heard.lock().unwrap(),
            [
                "started",
                "interrupted",
                "settling",
                "collecting",
                "finished"
            ]
        );
        // The sender was told to stop once, and what it said is in the result
        let stops = backend
            .calls
            .iter()
            .filter(|call| *call == "line stop")
            .count();
        assert_eq!(
            stops, 2,
            "one for the sender and one for the receiver at the end"
        );
        assert_eq!(result.workloads[0].stats.sent, 5);
        assert_eq!(backend.calls.last().map(String::as_str), Some("teardown"));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canaries: without the option the same cancel is an error, and with it a cancel before the workloads begin has
    /// nothing to keep and is an error too.
    #[test]
    fn a_cancel_without_the_option_or_before_the_workloads_is_an_error() {
        let token = CancelToken::new();
        let observer = {
            let token = token.clone();
            Arc::new(move |event: &RunEvent| {
                if matches!(event, RunEvent::Started { .. }) {
                    token.cancel();
                }
            })
        };
        let mut backend = Fake::new(Some("sender-waits-for-stop"));
        let options = self::options("interrupt-error")
            .with_observer(observer)
            .with_cancel(token);
        let error = run(&mut backend, &scenario(), &options).unwrap_err();
        assert!(matches!(error, Error::Cancelled), "{error}");
        assert_eq!(backend.calls.last().map(String::as_str), Some("teardown"));
        let _ = std::fs::remove_dir_all(&options.out_dir);

        let token = CancelToken::new();
        let observer = {
            let token = token.clone();
            Arc::new(move |event: &RunEvent| {
                if matches!(event, RunEvent::ReceiversReady { .. }) {
                    token.cancel();
                }
            })
        };
        let mut backend = Fake::new(Some("sender-waits-for-stop"));
        let options = self::options("interrupt-early")
            .with_observer(observer)
            .with_cancel(token)
            .with_partial_on_cancel();
        let error = run(&mut backend, &scenario(), &options).unwrap_err();
        assert!(matches!(error, Error::Cancelled), "{error}");
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Two workloads that send into one direction whose queue draws random numbers are not a repeatable experiment,
    /// also without a timeline (D-038, D-065): the order of their packets at the queue is not fixed. The result says
    /// so and names the workloads. It says nothing where the second workload goes the other way, where nothing is
    /// drawn, or where there is one workload.
    #[test]
    fn two_workloads_in_a_direction_with_a_loss_are_not_reproducible_and_one_each_way_are() {
        let run_with = |link: &str, workloads: &str, name: &str| {
            let yaml = format!(
                "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{link}workloads:\n{workloads}"
            );
            let mut backend = Fake::new(None);
            let options = options(name);
            let result = run(
                &mut backend,
                &Scenario::from_yaml(&yaml, "t.yaml").unwrap(),
                &options,
            )
            .unwrap();
            let _ = std::fs::remove_dir_all(&options.out_dir);
            (
                result.seed.loss_reproducible,
                result.seed.not_reproducible_because,
            )
        };
        let lossy = "      both: { delay: 5ms, loss: { model: random, p: 5% } }\n";
        let w = "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n";
        let v = "  - { id: v, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n";
        let back =
            "  - { id: v, kind: udp-cbr, from: b, to: a, rate: 800kbit, packet-size: 1000b }\n";
        // Two in one direction: the reason names both and the link and the direction
        let (reproducible, because) = run_with(lossy, &format!("{w}{v}"), "shared-1");
        assert!(!reproducible);
        assert_eq!(because.len(), 1, "{because:?}");
        assert!(
            because[0].contains("'w', 'v'") && because[0].contains("link 'l' (forward)"),
            "{because:?}"
        );
        // Canaries: one each way, one alone, and two where nothing is drawn say nothing
        let (reproducible, because) = run_with(lossy, &format!("{w}{back}"), "shared-2");
        assert!(reproducible && because.is_empty(), "{because:?}");
        let (reproducible, because) = run_with(lossy, w, "shared-3");
        assert!(reproducible && because.is_empty(), "{because:?}");
        let (reproducible, because) = run_with(
            "      both: { delay: 5ms }\n",
            &format!("{w}{v}"),
            "shared-4",
        );
        assert!(reproducible && because.is_empty(), "{because:?}");
        // A draw that only a later step brings counts as well
        let (reproducible, because) = {
            let yaml = format!(
                "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {{ delay: 5ms }}\nworkloads:\n{w}{v}timeline:\n  - {{ at: 500ms, link: l, set: {{ forward: {{ duplicate: 5% }} }} }}\n"
            );
            let mut backend = Fake::new(None);
            let options = options("shared-5");
            let result = run(
                &mut backend,
                &Scenario::from_yaml(&yaml, "t.yaml").unwrap(),
                &options,
            )
            .unwrap();
            let _ = std::fs::remove_dir_all(&options.out_dir);
            (
                result.seed.loss_reproducible,
                result.seed.not_reproducible_because,
            )
        };
        assert!(!reproducible);
        assert!(
            because.iter().any(|why| why.contains("link 'l' (forward)")),
            "{because:?}"
        );
    }

    /// A step that the backend cannot do ends the run with the error of the backend: a run in which the link did
    /// not change would measure something the scenario did not say.
    #[test]
    fn a_step_that_fails_ends_the_run_with_the_error_of_the_backend() {
        let scenario = Scenario::from_yaml(WITH_TIMELINE, "t.yaml").unwrap();
        let mut backend = Fake::new(Some("change-fails"));
        let options = options("step-fails");
        let error = run(&mut backend, &scenario, &options).unwrap_err();
        assert!(
            matches!(&error, Error::Backend { message, .. } if message == "refused"),
            "{error}"
        );
        assert_eq!(backend.calls.last().map(String::as_str), Some("teardown"));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canary: a scenario without a timeline has no events and no step is asked of the backend.
    #[test]
    fn a_scenario_without_a_timeline_does_no_steps() {
        let mut backend = Fake::new(None);
        let options = options("no-steps");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert_eq!(result.events.len(), 0);
        assert!(backend.calls.iter().all(|call| !call.starts_with("change")));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A stream without the checksum of UDP: both agents are told so, the damaged packets are counted in the
    /// result, they are in `lost` (an intact packet is what `received` counts), and they are among the datagrams
    /// that were delivered, so the node is not blamed for dropping them.
    #[test]
    fn a_stream_without_the_checksum_starts_both_agents_that_way_and_counts_what_arrived_damaged() {
        let yaml = "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { corrupt: 20% }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b, udp-checksum: false }\n";
        let scenario = Scenario::from_yaml(yaml, "t.yaml").unwrap();
        let mut backend = Fake::with_frames([10, 0], 0);
        let options = options("no-checksum");
        let result = run(&mut backend, &scenario, &options).unwrap();
        assert_eq!(backend.checksum, ["recv off", "send off"]);
        let workload = &result.workloads[0];
        assert_eq!(
            (
                workload.stats.sent,
                workload.stats.received,
                workload.stats.lost,
                workload.corrupted
            ),
            (10, 8, 2, 2)
        );
        let tally = &result.directions[0];
        assert_eq!(
            (
                tally.delivered,
                tally.dropped_corrupted,
                tally.lost_in_queue
            ),
            (10, 0, Some(0)),
            "the two damaged packets reached the receiver, so the node dropped nothing"
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    const CORRUPTING: &str = "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { corrupt: 20% }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n";

    /// A direction that corrupts is read on the wire, and the packets that the tap saw and the socket did not get
    /// are named: 3 (seen twice, named once) and 7. The number 99 was never sent and is not named, and the
    /// export of every packet marks the two.
    #[test]
    fn the_damaged_packets_of_a_direction_that_corrupts_are_named_from_the_wire() {
        let mut scenario = Scenario::from_yaml(CORRUPTING, "t.yaml").unwrap();
        scenario.measure.packets = true;
        let mut backend = Fake::new(None);
        let options = options("named");
        let result = run(&mut backend, &scenario, &options).unwrap();
        let workload = &result.workloads[0];
        assert_eq!(workload.corrupted_named, Some(2));
        assert_eq!(workload.corrupted_seq, [3, 7]);
        assert_eq!(workload.corrupted_seq_unavailable, None);
        let packets = std::fs::read_to_string(
            options
                .out_dir
                .join(workload.packets_file.as_ref().unwrap()),
        )
        .unwrap();
        let marked: Vec<&str> = packets
            .lines()
            .filter(|line| line.contains(r#""corrupted":true"#))
            .collect();
        assert_eq!(marked.len(), 2, "{packets}");
        assert!(marked[0].contains(r#""sequence":3"#) && marked[0].contains(r#""lost":true"#));
        assert!(marked[1].contains(r#""sequence":7"#));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Where the tap cannot be opened the run goes on, without names and with the reason; a direction that does not
    /// corrupt does not ask for a tap at all.
    #[test]
    fn a_tap_that_cannot_be_opened_leaves_the_names_out_and_says_why() {
        let scenario = Scenario::from_yaml(CORRUPTING, "t.yaml").unwrap();
        let mut backend = Fake::new(Some("no-tap"));
        let options = options("no-tap");
        let result = run(&mut backend, &scenario, &options).unwrap();
        let workload = &result.workloads[0];
        assert_eq!(
            (workload.corrupted_named, workload.corrupted_seq.len()),
            (None, 0)
        );
        let why = workload.corrupted_seq_unavailable.as_deref().unwrap();
        assert!(
            why.contains("Operation not permitted") && why.contains("CAP_NET_RAW"),
            "{why}"
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
        // Canary: no corruption, no tap, nothing to say
        let mut backend = Fake::new(None);
        let options = self::options("no-tap-plain");
        let result = run(&mut backend, &self::scenario(), &options).unwrap();
        let workload = &result.workloads[0];
        assert_eq!(
            (
                workload.corrupted_named,
                workload.corrupted_seq_unavailable.as_ref()
            ),
            (None, None)
        );
        assert!(!options.out_dir.join("flow-0.wire").exists());
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A node that dropped datagrams for a full buffer did not deliver packets that were whole, and those would be
    /// named as damaged: the names are withdrawn.
    #[test]
    fn names_are_withdrawn_where_the_node_dropped_datagrams_for_a_full_buffer() {
        let scenario = Scenario::from_yaml(CORRUPTING, "t.yaml").unwrap();
        let mut backend = Fake::with_frames([9, 0], 1);
        let options = options("named-buffer");
        let result = run(&mut backend, &scenario, &options).unwrap();
        let workload = &result.workloads[0];
        assert_eq!(workload.corrupted_named, None);
        assert!(
            workload
                .corrupted_seq_unavailable
                .as_deref()
                .is_some_and(|why| why.contains("receive buffer")),
            "{workload:?}"
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canary for the test above: a stream with the checksum on starts its agents without the argument and
    /// reports no damaged packet.
    #[test]
    fn a_stream_with_the_checksum_starts_its_agents_without_the_argument() {
        let mut backend = Fake::new(None);
        let options = options("checksum");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert!(backend.checksum.is_empty(), "{:?}", backend.checksum);
        assert_eq!(result.workloads[0].corrupted, 0);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn a_full_receive_buffer_is_not_corruption() {
        let mut backend = Fake::with_frames([9, 0], 1);
        let options = options("tally-buffer");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        let tally = &result.directions[0];
        assert_eq!(
            (
                tally.dropped_corrupted,
                tally.receive_buffer_drops,
                tally.lost_in_queue
            ),
            (0, 1, Some(1))
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// The frames of a duplicating direction include the copies, so the packets the queue dropped cannot be
    /// told; the node's own drops still can.
    #[test]
    fn a_duplicating_direction_has_no_count_of_the_packets_the_queue_dropped() {
        let yaml = "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: { duplicate: 20% }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n";
        let scenario = Scenario::from_yaml(yaml, "t.yaml").unwrap();
        let mut backend = Fake::with_frames([9, 0], 0);
        let options = options("tally-duplicate");
        let result = run(&mut backend, &scenario, &options).unwrap();
        let tally = &result.directions[0];
        assert_eq!((tally.lost_in_queue, tally.dropped_corrupted), (None, 1));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn a_backend_that_cannot_count_the_node_leaves_the_tally_out() {
        let mut backend = Fake::new(None);
        let options = options("tally-none");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert_eq!(result.directions.len(), 0);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn a_backend_that_cannot_count_leaves_the_result_as_it_was() {
        let mut backend = Fake::new(None);
        let options = options("queues-unknown");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        assert!(result.seed.loss_reproducible && result.seed.queue_checks.is_empty());
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn a_direction_without_loss_is_not_counted() {
        let without_loss = Scenario::from_yaml(
            "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { delay: 20ms, loss: { model: random, p: 20% } }\n      reverse: { delay: 20ms }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n",
            "t.yaml",
        )
        .unwrap();
        let mut backend = Fake::counting([10, 99]);
        let options = options("queues-no-loss");
        let result = run(&mut backend, &without_loss, &options).unwrap();
        assert_eq!(result.seed.queue_checks.len(), 1);
        assert_eq!(result.seed.queue_checks[0].direction, "forward");
        assert!(result.seed.loss_reproducible);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn the_packets_can_be_exported_with_the_lost_ones_marked() {
        let mut backend = Fake::new(None);
        let options = options("export");
        let mut exported = scenario();
        exported.measure.packets = true;
        let result = run(&mut backend, &exported, &options).unwrap();
        assert_eq!(
            result.workloads[0].packets_file.as_deref(),
            Some("packets-w.jsonl")
        );
        let text = std::fs::read_to_string(options.out_dir.join("packets-w.jsonl")).unwrap();
        let lost: Vec<u64> = text
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|v| v["lost"] == true)
            .map(|v| v["sequence"].as_u64().unwrap())
            .collect();
        assert_eq!(lost, [3, 7]);
        assert_eq!(text.lines().count(), 10);
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canary: a run that cannot start its agent fails and says why, and the network is still torn down.
    #[test]
    fn a_missing_agent_is_an_error_and_the_network_is_torn_down() {
        let mut backend = Fake::new(None);
        let mut options = options("noagent");
        options.agent = PathBuf::from("/nonexistent/netscenario-agent");
        let error = run(&mut backend, &scenario(), &options).unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
        assert_eq!(backend.calls.last().map(String::as_str), Some("teardown"));
        assert!(!backend.calls.iter().any(|c| c.starts_with("start")));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn a_receiver_that_ends_before_it_is_ready_says_what_it_said() {
        let mut backend = Fake::new(Some("recv-exits-early"));
        let options = options("early");
        let error = run(&mut backend, &scenario(), &options).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("start the receiver of 'w'"), "{text}");
        assert!(text.contains("address in use"), "{text}");
        assert!(!backend.calls.iter().any(|c| c.starts_with("start send")));
        assert_eq!(backend.calls.last().map(String::as_str), Some("teardown"));
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn a_sender_that_fails_stops_the_run_with_its_message() {
        let mut backend = Fake::new(Some("send-fails"));
        let options = options("sendfail");
        let error = run(&mut backend, &scenario(), &options).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("start the sender of 'w'") && text.contains("network is unreachable"),
            "{text}"
        );
        assert_eq!(backend.calls.last().map(String::as_str), Some("teardown"));
        assert!(!options.out_dir.join("result.json").exists());
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// An observer that keeps what it is told, and cancels when it hears `cancel_at`.
    #[derive(Default)]
    struct Recorder {
        events: std::sync::Mutex<Vec<RunEvent>>,
        cancel_at: Option<(RunEvent, CancelToken)>,
    }

    impl RunObserver for Recorder {
        fn event(&self, event: &RunEvent) {
            self.events.lock().unwrap().push(event.clone());
            if let Some((when, token)) = &self.cancel_at
                && when == event
            {
                token.cancel();
            }
        }
    }

    fn observed(
        name: &str,
        cancel_at: Option<RunEvent>,
    ) -> (RunOptions, Arc<Recorder>, CancelToken) {
        let token = CancelToken::new();
        let recorder = Arc::new(Recorder {
            events: std::sync::Mutex::default(),
            cancel_at: cancel_at.map(|event| (event, token.clone())),
        });
        let options = options(name)
            .with_observer(recorder.clone())
            .with_cancel(token.clone());
        (options, recorder, token)
    }

    #[test]
    fn a_run_tells_its_phases_in_order() {
        let mut backend = Fake::new(None);
        let (options, recorder, _) = observed("events", None);
        run(&mut backend, &scenario(), &options).unwrap();
        assert_eq!(
            *recorder.events.lock().unwrap(),
            [
                RunEvent::Provisioned { nodes: 2, links: 1 },
                RunEvent::ReceiversReady { count: 1 },
                RunEvent::Started {
                    duration: Duration::from_secs(1)
                },
                // The fake finishes at once, so only the last report comes
                RunEvent::Progress {
                    elapsed: Duration::from_secs(1),
                    total: Duration::from_secs(1)
                },
                RunEvent::Settling {
                    wait: Duration::from_millis(1)
                },
                RunEvent::Collecting,
                RunEvent::TearingDown,
                RunEvent::Finished,
            ]
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// A run that was cancelled before it began builds nothing and says nothing.
    #[test]
    fn a_cancel_before_the_start_builds_nothing() {
        let mut backend = Fake::new(None);
        let (options, recorder, token) = observed("cancel-early", None);
        token.cancel();
        let error = run(&mut backend, &scenario(), &options).unwrap_err();
        assert!(matches!(error, Error::Cancelled), "{error}");
        assert_eq!(error.to_string(), "the run was cancelled");
        assert!(backend.calls.is_empty(), "{:?}", backend.calls);
        assert!(recorder.events.lock().unwrap().is_empty());
        assert!(!options.out_dir.join("result.json").exists());
    }

    /// Canary: a cancel in any phase of the run stops it, tears the network down, writes no result and does
    /// not say that it finished.
    #[test]
    fn a_cancel_in_any_phase_tears_down_and_returns_cancelled() {
        for phase in [
            RunEvent::Provisioned { nodes: 2, links: 1 },
            RunEvent::ReceiversReady { count: 1 },
            RunEvent::Started {
                duration: Duration::from_secs(1),
            },
            RunEvent::Progress {
                elapsed: Duration::from_secs(1),
                total: Duration::from_secs(1),
            },
            RunEvent::Settling {
                wait: Duration::from_millis(1),
            },
            RunEvent::Collecting,
        ] {
            let mut backend = Fake::new(None);
            let (options, recorder, _) = observed("cancel-phase", Some(phase.clone()));
            let error = run(&mut backend, &scenario(), &options).unwrap_err();
            assert!(matches!(error, Error::Cancelled), "{phase:?}: {error}");
            assert_eq!(
                backend.calls.iter().filter(|c| *c == "teardown").count(),
                1,
                "{phase:?}: {:?}",
                backend.calls
            );
            assert_eq!(backend.calls.last().map(String::as_str), Some("teardown"));
            let events = recorder.events.lock().unwrap();
            assert_eq!(events.last(), Some(&RunEvent::TearingDown), "{phase:?}");
            assert!(!events.contains(&RunEvent::Finished), "{phase:?}");
            assert!(!options.out_dir.join("result.json").exists(), "{phase:?}");
            let _ = std::fs::remove_dir_all(&options.out_dir);
        }
    }

    /// The progress is reported once a second, from the start of the workloads, and never past the end.
    #[test]
    fn the_progress_is_reported_once_a_second() {
        let (options, recorder, _) = observed("progress", None);
        let control = Control::new(&options);
        // Nothing before the workloads
        control.checkpoint().unwrap();
        assert!(recorder.events.lock().unwrap().is_empty());
        control.workloads_started(
            Instant::now()
                .checked_sub(Duration::from_millis(2500))
                .unwrap(),
            Duration::from_secs(10),
        );
        control.checkpoint().unwrap();
        control.checkpoint().unwrap();
        {
            let events = recorder.events.lock().unwrap();
            assert_eq!(
                events.len(),
                2,
                "one Started and one Progress, not two: {events:?}"
            );
            let RunEvent::Progress { elapsed, total } = events[1] else {
                panic!("{:?}", events[1]);
            };
            assert!((2.4..2.8).contains(&elapsed.as_secs_f64()), "{elapsed:?}");
            assert_eq!(total, Duration::from_secs(10));
        }
        // A run that is long over says nothing at a checkpoint: the report of the whole is made once, by `workloads_done`
        control.workloads_started(
            Instant::now().checked_sub(Duration::from_secs(60)).unwrap(),
            Duration::from_secs(10),
        );
        let before = recorder.events.lock().unwrap().len();
        control.checkpoint().unwrap();
        let after = recorder.events.lock().unwrap().len();
        assert_eq!(
            before, after,
            "a checkpoint says nothing of a run that is over"
        );
        control.workloads_done();
        control.workloads_done();
        let events = recorder.events.lock().unwrap();
        let done = events
            .iter()
            .filter(|e| matches!(e, RunEvent::Progress { elapsed, total } if elapsed == total))
            .count();
        // the one of `workloads_done`, which says it once however often it is called
        assert_eq!(done, 1, "{events:?}");
    }

    #[test]
    fn the_agent_is_looked_for_next_to_the_program() {
        assert_eq!(
            agent_next_to(Path::new("/home/x/.cargo/bin/netscenario")),
            PathBuf::from("/home/x/.cargo/bin/netscenario-agent")
        );
    }

    #[test]
    fn the_settle_time_follows_the_slowest_link_unless_it_is_set() {
        let scenario = Scenario::from_yaml(
            "version: 0\nname: t\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { delay: 100ms, jitter: { amount: 10ms } }\n      reverse: { delay: 300ms }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n",
            "t.yaml",
        )
        .unwrap();
        // forward: 100 ms + 4 * 10 ms = 140 ms; reverse: 300 ms
        assert_eq!(longest_hold(&scenario), Duration::from_millis(300));
    }

    /// The frames of a trace are counted for each packet: the packets that the sender put in, less the ones the kernel refused. A
    /// packet that is cut makes more than one frame, and the most of any packet tells whether the tally by packet can be kept.
    #[test]
    fn the_frames_of_a_trace_are_counted_for_each_packet_that_was_sent() {
        let sent = |packets: u64, errors: u64, refused: Vec<[u64; 2]>| SendSummary {
            flow: 0,
            packets,
            first_send_ns: None,
            last_send_ns: None,
            max_lateness_ns: 0,
            errors,
            first_error: None,
            refused,
            refused_unrecorded: 0,
            stopped: false,
            barriers: Vec::new(),
            barrier_timeouts: 0,
        };
        // Packets 0 to 5 make 1, 2, 1, 3, 1, 1 frames
        let frames = Frames::PerPacket(vec![1, 2, 1, 3, 1, 1]);
        assert_eq!(frames.most(), 3);
        assert_eq!(frames.of_sent(&sent(6, 0, Vec::new())), 9);
        // Four were sent and the kernel refused packets 1 and 2: 0 and 3 are the ones that left (and 4, 5 never had a slot)
        assert_eq!(frames.of_sent(&sent(2, 2, vec![[1, 2]])), 1 + 3);
        // Canary: the same count for a constant size is the packets times the frames
        let each = Frames::Each(2);
        assert_eq!(
            (each.most(), each.of_sent(&sent(6, 0, Vec::new()))),
            (2, 12)
        );
    }
}
