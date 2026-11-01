//! The run: from a built network to a result (steps 5 to 9 of the life of a run, `docs/ARCHITECTURE.md`).
//!
//! The order, for every workload at once: start the receiver in the node of `to` and wait until it says it is
//! ready; fix one moment a little ahead and start every sender with it, so that the workloads start together;
//! wait until the senders are done; wait for the packets that are still in the network; tell the receivers to
//! stop; read their records; tear the network down; compute the statistics and write the result.
//!
//! The network is torn down whatever happens. If the run fails, the first error is returned.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use netscenario_agent::clock::monotonic_ns;
use netscenario_agent::events::{Event, ReceiveSummary, SendSummary};
use netscenario_agent::records;
use netscenario_model::{Impairments, Scenario, UdpCbr, Workload};

use crate::backend::{
    Backend, Direction, LinkHandle, NodeCounts, NodeHandle, Program, ProgramHandle,
};
use crate::error::Error;
use crate::export::write_packets;
use crate::hash::sha256_hex;
use crate::machine::collect;
use crate::observe::{CancelToken, RunEvent, RunObserver};
use crate::provision::{Provisioned, impairments_of, provision};
use crate::result::{
    AcceptedRestriction, AddressInfo, DirectionAccount, QueueCheck, RESULT_FORMAT, RESULT_VERSION,
    RunResult, ScenarioInfo, SeedInfo, SenderInfo, Timing, WorkloadResult,
};
use crate::stats::analyse;

/// The port of the first workload; the next uses the next port.
const BASE_PORT: u16 = 5000;

/// How far ahead of the moment all the senders have to start, so that they are all ready when it comes.
const START_MARGIN: Duration = Duration::from_millis(500);

/// How long a program may take to end after it should have, before the run gives up on it.
const END_MARGIN: Duration = Duration::from_secs(30);

/// The settings of a run.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunOptions {
    /// The agent program (`netscenario-agent`), started by path in the nodes.
    pub agent: PathBuf,
    /// The directory for the result and the files that belong to it. It is created if it does not exist, and
    /// the agents in the nodes write into it, so it must be a directory that they can write to.
    pub out_dir: PathBuf,
    /// How long to wait after the last packet was sent for the packets still in the network. Without it:
    /// one second and twice the longest delay of the scenario with four times its jitter. A packet that
    /// needs longer counts as lost.
    pub settle: Option<Duration>,
    /// Whether to write the JSON Lines file with one line for each packet of each workload.
    pub export_packets: bool,
    /// The most address space an agent may use, in bytes (SAF-02). The default is 1 GiB.
    pub agent_memory_limit: u64,
    /// How long a receiver may take to say that it is ready.
    pub ready_timeout: Duration,
    /// Told how the run is going (see [`RunEvent`]). Without it nobody is told.
    pub observer: Option<Arc<dyn RunObserver>>,
    /// Cancel it to stop the run: the network is torn down and the run returns [`Error::Cancelled`]. The run
    /// looks at it at least every 200 milliseconds.
    pub cancel: CancelToken,
}

impl std::fmt::Debug for RunOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunOptions")
            .field("agent", &self.agent)
            .field("out_dir", &self.out_dir)
            .field("settle", &self.settle)
            .field("export_packets", &self.export_packets)
            .field("agent_memory_limit", &self.agent_memory_limit)
            .field("ready_timeout", &self.ready_timeout)
            .field("observer", &self.observer.as_ref().map(|_| "<observer>"))
            .field("cancel", &self.cancel)
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
            export_packets: false,
            agent_memory_limit: 1 << 30,
            ready_timeout: Duration::from_secs(10),
            observer: None,
            cancel: CancelToken::new(),
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
struct Control<'a> {
    cancel: &'a CancelToken,
    observer: Option<&'a dyn RunObserver>,
    workloads: RefCell<Option<Workloads>>,
}

impl<'a> Control<'a> {
    fn new(options: &'a RunOptions) -> Self {
        Self {
            cancel: &options.cancel,
            observer: options.observer.as_deref(),
            workloads: RefCell::new(None),
        }
    }

    fn emit(&self, event: &RunEvent) {
        if let Some(observer) = self.observer {
            observer.event(event);
        }
    }

    /// Called in every loop that waits and between the steps: stops the run if it was cancelled, and reports
    /// the progress of the workloads once a second.
    fn checkpoint(&self) -> Result<(), Error> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let report = {
            let mut workloads = self.workloads.borrow_mut();
            workloads.as_mut().and_then(|w| {
                let now = Instant::now();
                (now >= w.next_report).then(|| {
                    while w.next_report <= now {
                        w.next_report += PROGRESS_EVERY;
                    }
                    RunEvent::Progress {
                        elapsed: now.saturating_duration_since(w.begin).min(w.total),
                        total: w.total,
                    }
                })
            })
        };
        if let Some(event) = report {
            self.emit(&event);
        }
        Ok(())
    }

    /// The workloads begin at `begin` and last `total`.
    fn workloads_started(&self, begin: Instant, total: Duration) {
        *self.workloads.borrow_mut() = Some(Workloads {
            begin,
            total,
            next_report: begin + PROGRESS_EVERY,
        });
        self.emit(&RunEvent::Started { duration: total });
    }

    /// The workloads are over: a last report at the full length.
    fn workloads_done(&self) {
        let done = self.workloads.borrow_mut().take();
        if let Some(w) = done {
            self.emit(&RunEvent::Progress {
                elapsed: w.total,
                total: w.total,
            });
        }
    }
}

fn failure(action: impl Into<String>, message: impl Into<String>) -> Error {
    Error::Run {
        action: action.into(),
        message: message.into(),
    }
}

/// What one workload needs to be started.
struct Flow<'a> {
    index: usize,
    workload: &'a UdpCbr,
    from_node: NodeHandle,
    to_node: NodeHandle,
    to_address: String,
    records_file: PathBuf,
    start: Duration,
    duration: Duration,
}

/// What came back from one workload.
struct Measured {
    sent: SendSummary,
    received: ReceiveSummary,
    records: records::File,
}

/// What the phases of a run found out besides the workloads.
struct Measurement {
    measured: Vec<Measured>,
    settle: Duration,
    queue_checks: Vec<QueueCheck>,
    directions: Vec<DirectionAccount>,
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
    } = measured?;
    torn_down?;

    let mut workloads = Vec::new();
    for (index, (workload, flow)) in scenario
        .workloads
        .iter()
        .filter_map(Workload::as_udp_cbr)
        .zip(&measured)
        .enumerate()
    {
        workloads.push(workload_result(
            index,
            workload,
            flow,
            scenario,
            &provisioned,
            options,
        )?);
    }
    let result = RunResult {
        format: RESULT_FORMAT.to_owned(),
        version: RESULT_VERSION,
        scenario: ScenarioInfo {
            name: scenario.name.to_string(),
            hash: format!(
                "sha256:{}",
                sha256_hex(&serde_json::to_vec(scenario).unwrap_or_default())
            ),
        },
        provenance: collect(&backend.describe(), started),
        seed: SeedInfo {
            value: provisioned.seed,
            drawn: provisioned.seed_drawn,
            // The queues have counted the packets that drew a number for the loss; if one saw more or fewer than
            // the workloads sent, the pattern of lost packets is not the one of this seed (ENV-07)
            loss_reproducible: provisioned.loss_reproducible
                && queue_checks.iter().all(QueueCheck::agrees),
            queue_checks,
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
        timing: Timing {
            duration_ns: nanos(Duration::from(scenario.duration)),
            settle_ns: nanos(settle),
        },
        workloads,
        directions,
    };
    let path = options.out_dir.join("result.json");
    std::fs::write(&path, result.to_json())
        .map_err(|error| failure("write the result", format!("{}: {error}", path.display())))?;
    control.emit(&RunEvent::Finished);
    Ok(result)
}

/// The result of one workload: its statistics, and the files that belong to it.
fn workload_result(
    index: usize,
    workload: &UdpCbr,
    flow: &Measured,
    scenario: &Scenario,
    provisioned: &Provisioned,
    options: &RunOptions,
) -> Result<WorkloadResult, Error> {
    let start = workload.start.map_or(Duration::ZERO, Duration::from);
    let stop = workload
        .stop
        .map_or_else(|| Duration::from(scenario.duration), Duration::from);
    let first_send = flow.sent.first_send_ns.unwrap_or(0);
    let packets_file = if options.export_packets {
        let name = format!("packets-{}.jsonl", workload.id);
        let file = std::fs::File::create(options.out_dir.join(&name))
            .map_err(|error| failure("write the packets", format!("{name}: {error}")))?;
        write_packets(
            std::io::BufWriter::new(file),
            workload.id.as_str(),
            flow.sent.packets,
            &flow.records.records,
            first_send,
        )
        .map_err(|error| failure("write the packets", format!("{name}: {error}")))?;
        Some(name)
    } else {
        None
    };
    let (from_address, to_address) = addresses_of(provisioned, workload);
    Ok(WorkloadResult {
        id: workload.id.to_string(),
        kind: "udp-cbr".to_owned(),
        from: workload.from.to_string(),
        to: workload.to.to_string(),
        from_address,
        to_address: format!("{to_address}:{}", port(index)),
        rate_bits_per_second: workload.rate.bits_per_second(),
        packet_size_bytes: workload.packet_size.bytes(),
        start_ns: nanos(start),
        duration_ns: nanos(stop.saturating_sub(start)),
        stats: analyse(
            flow.sent.packets,
            &flow.records.records,
            workload.packet_size.bytes(),
            first_send,
        ),
        sender: SenderInfo {
            max_lateness_ns: flow.sent.max_lateness_ns,
            send_errors: flow.sent.errors,
            first_error: flow.sent.first_error.clone(),
        },
        foreign_datagrams: flow.received.foreign,
        records_file: records_name(index),
        packets_file,
    })
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn port(index: usize) -> u16 {
    BASE_PORT.saturating_add(u16::try_from(index).unwrap_or(0))
}

fn records_name(index: usize) -> String {
    format!("flow-{index}.records")
}

/// The address of the sending node and of the receiving node of a workload.
fn addresses_of(provisioned: &Provisioned, workload: &UdpCbr) -> (String, String) {
    let address = |node: &netscenario_model::Name| {
        provisioned
            .addresses
            .iter()
            .find(|a| &a.node == node)
            .map(|a| a.address.to_string())
            .unwrap_or_default()
    };
    (address(&workload.from), address(&workload.to))
}

/// The longest time a packet can be held back by the links of the scenario: the delay and four times the
/// jitter (a normal distribution has tails).
fn longest_hold(scenario: &Scenario) -> Duration {
    let hold = |impairments: &Impairments| {
        let delay = impairments.delay.map_or(Duration::ZERO, Duration::from);
        let jitter = impairments
            .jitter
            .map_or(Duration::ZERO, |j| Duration::from(j.amount) * 4);
        delay + jitter
    };
    scenario
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
        .unwrap_or(Duration::ZERO)
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
    scenario
        .workloads
        .iter()
        .filter_map(Workload::as_udp_cbr)
        .enumerate()
        .map(|(index, workload)| {
            let start = workload.start.map_or(Duration::ZERO, Duration::from);
            let stop = workload
                .stop
                .map_or_else(|| Duration::from(scenario.duration), Duration::from);
            let (_, to_address) = addresses_of(provisioned, workload);
            Ok(Flow {
                index,
                workload,
                from_node: node(&workload.from)?,
                to_node: node(&workload.to)?,
                to_address: format!("{to_address}:{}", port(index)),
                records_file: options.out_dir.join(records_name(index)),
                start,
                duration: stop.saturating_sub(start),
            })
        })
        .collect()
}

/// Waits for the first line of a program, and fails if it ends or takes too long.
fn wait_for_line(
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
fn wait_for_end(
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

fn event(line: &str, what: &str) -> Result<Event, Error> {
    serde_json::from_str(line).map_err(|error| {
        failure(
            what,
            format!("cannot read the agent's line '{line}': {error}"),
        )
    })
}

fn agent_program(options: &RunOptions, arguments: &[String]) -> Program {
    let mut argv = vec![options.agent.display().to_string()];
    argv.extend_from_slice(arguments);
    Program::new(argv).with_memory_limit(options.agent_memory_limit)
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// The receivers first: they open the ports and say when they are ready.
fn start_receivers(
    backend: &mut dyn Backend,
    flows: &[Flow<'_>],
    options: &RunOptions,
    control: &Control<'_>,
) -> Result<Vec<ProgramHandle>, Error> {
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
        let program = backend.start_program(flow.to_node, &agent_program(options, &arguments))?;
        let line = wait_for_line(backend, program, options.ready_timeout, &what, control)?;
        match event(&line, &what)? {
            Event::Ready { .. } => {}
            other => {
                return Err(failure(
                    what,
                    format!("expected ready, the agent said {other:?}"),
                ));
            }
        }
        receivers.push(program);
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
) -> Result<Vec<(ProgramHandle, String)>, Error> {
    let mut senders = Vec::new();
    for flow in flows {
        let what = format!("start the sender of '{}'", flow.workload.id);
        let mut arguments = strings(&["send", "--to", &flow.to_address, "--rate"]);
        arguments.push(flow.workload.rate.bits_per_second().to_string());
        arguments.push("--size".to_owned());
        arguments.push(flow.workload.packet_size.bytes().to_string());
        arguments.push("--duration-ns".to_owned());
        arguments.push(nanos(flow.duration).to_string());
        arguments.push("--flow".to_owned());
        arguments.push(flow.index.to_string());
        arguments.push("--start-at-ns".to_owned());
        arguments.push((begin + nanos(flow.start)).to_string());
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

    let receivers = start_receivers(backend, &flows, options, control)?;
    control.emit(&RunEvent::ReceiversReady {
        count: receivers.len(),
    });
    control.checkpoint()?;
    // What the nodes had counted before the first packet; the nodes are new, but a run reads the difference
    let before = node_counts(backend, scenario, provisioned)?;
    let begin_at = Instant::now() + START_MARGIN;
    let begin = monotonic_ns() + nanos(START_MARGIN);
    let senders = start_senders(backend, &flows, options, begin)?;

    let latest_end = flows
        .iter()
        .map(|flow| flow.start + flow.duration)
        .max()
        .unwrap_or(Duration::ZERO);
    control.workloads_started(begin_at, latest_end);
    let deadline = Instant::now() + START_MARGIN + latest_end + END_MARGIN;
    let mut sent = Vec::new();
    for (program, what) in &senders {
        let lines = wait_for_end(backend, *program, deadline, what, control)?;
        match lines.last().map(|line| event(line, what)).transpose()? {
            Some(Event::Sent(summary)) => sent.push(summary),
            other => {
                return Err(failure(
                    what.clone(),
                    format!("the sender should end with 'sent', it said {other:?}"),
                ));
            }
        }
    }
    control.workloads_done();

    // The packets that are still in the network arrive before the receivers are told to stop
    let settle = options
        .settle
        .unwrap_or_else(|| Duration::from_secs(1) + longest_hold(scenario) * 2);
    control.emit(&RunEvent::Settling { wait: settle });
    let settled = Instant::now() + settle;
    loop {
        control.checkpoint()?;
        let left = settled.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
    // After the wait, every packet has passed its queue; the receivers still run and the network still stands
    let queue_checks = check_queues(backend, scenario, provisioned, &flows, &sent)?;
    let after = node_counts(backend, scenario, provisioned)?;
    control.emit(&RunEvent::Collecting);
    let measured = stop_receivers(backend, &receivers, &flows, sent, control)?;
    let directions =
        account_for_directions(scenario, provisioned, &flows, &measured, &before, &after);
    Ok(Measurement {
        measured,
        settle,
        queue_checks,
        directions,
    })
}

/// Stops the receivers one after the other and reads what each of them recorded, with what its sender said.
fn stop_receivers(
    backend: &mut dyn Backend,
    receivers: &[ProgramHandle],
    flows: &[Flow<'_>],
    sent: Vec<SendSummary>,
    control: &Control<'_>,
) -> Result<Vec<Measured>, Error> {
    let mut measured = Vec::new();
    for ((receiver, flow), sent) in receivers.iter().zip(flows).zip(sent) {
        control.checkpoint()?;
        let what = format!("stop the receiver of '{}'", flow.workload.id);
        backend.send_line(*receiver, "stop")?;
        let lines = wait_for_end(
            backend,
            *receiver,
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
        measured.push(Measured {
            sent,
            received,
            records,
        });
    }
    Ok(measured)
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
    from: &'a netscenario_model::Name,
    to: &'a netscenario_model::Name,
}

/// The directions of the links, in the order of the scenario: for each link the forward one, then the reverse.
fn ways<'a>(scenario: &'a Scenario, provisioned: &'a Provisioned) -> impl Iterator<Item = Way<'a>> {
    scenario
        .topology
        .links
        .iter()
        .zip(&provisioned.links)
        .enumerate()
        .flat_map(|(index, (link, (name, handle)))| {
            [
                (Direction::Forward, "forward", 0, 1),
                (Direction::Reverse, "reverse", 1, 0),
            ]
            .map(move |(direction, word, from, to)| Way {
                index,
                name,
                handle: *handle,
                direction,
                word,
                from: &link.between[from],
                to: &link.between[to],
            })
        })
}

/// What the receiving node of every direction has counted so far, in the order of [`ways`]. `None` for
/// a direction whose backend cannot count, and for every direction in a scenario with a program of your own,
/// whose packets the agent does not count (the account of a direction would not add up).
fn node_counts(
    backend: &mut dyn Backend,
    scenario: &Scenario,
    provisioned: &Provisioned,
) -> Result<Vec<Option<NodeCounts>>, Error> {
    let has_program = scenario
        .workloads
        .iter()
        .any(|workload| workload.as_udp_cbr().is_none());
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

/// Sets the packets that the workloads sent into each direction against what the receiving node counted and
/// what its receivers got (D-040). A direction without a workload has no account.
fn account_for_directions(
    scenario: &Scenario,
    provisioned: &Provisioned,
    flows: &[Flow<'_>],
    measured: &[Measured],
    before: &[Option<NodeCounts>],
    after: &[Option<NodeCounts>],
) -> Vec<DirectionAccount> {
    let mut accounts = Vec::new();
    for (way, (before, after)) in ways(scenario, provisioned).zip(before.iter().zip(after)) {
        let (Some(before), Some(after)) = (before, after) else {
            continue;
        };
        let link = &scenario.topology.links[way.index];
        let carried: Vec<&Measured> = flows
            .iter()
            .zip(measured)
            .filter(|(flow, _)| flow.workload.from == *way.from && flow.workload.to == *way.to)
            .map(|(_, measured)| measured)
            .collect();
        if carried.is_empty() {
            continue;
        }
        let sent: u64 = carried.iter().map(|m| m.sent.packets).sum();
        let delivered: u64 = carried
            .iter()
            .map(|m| m.received.packets + m.received.foreign)
            .sum();
        let reached_node = after.frames_received.saturating_sub(before.frames_received);
        let receive_buffer_drops = after
            .receive_buffer_drops
            .saturating_sub(before.receive_buffer_drops);
        // The frames of a duplicating direction include the copies, so what the queue dropped cannot be told
        let duplicates = impairments_of(link, way.direction)
            .is_some_and(|impairments| impairments.duplicate.is_some());
        accounts.push(DirectionAccount {
            link: way.name.to_string(),
            direction: way.word.to_owned(),
            sent,
            reached_node,
            delivered,
            lost_in_queue: if duplicates {
                None
            } else {
                sent.checked_sub(reached_node)
            },
            dropped_corrupted: reached_node
                .saturating_sub(delivered)
                .saturating_sub(receive_buffer_drops),
            receive_buffer_drops,
        });
    }
    accounts
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
) -> Result<Vec<QueueCheck>, Error> {
    let mut checks = Vec::new();
    if provisioned.seed.is_none() {
        return Ok(checks);
    }
    for way in ways(scenario, provisioned) {
        let link = &scenario.topology.links[way.index];
        // A duplicating direction is not checked: the queue counts the copies it makes as well, and the count
        // cannot tell them from a packet that nobody asked for (D-054)
        if impairments_of(link, way.direction)
            .is_none_or(|impairments| !impairments.is_random() || impairments.duplicate.is_some())
        {
            continue;
        }
        let expected = flows
            .iter()
            .zip(sent)
            .filter(|(flow, _)| flow.workload.from == *way.from && flow.workload.to == *way.to)
            .map(|(_, summary)| summary.packets)
            .sum();
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
    }

    struct Played {
        lines: VecDeque<String>,
        exit: Option<ProgramExit>,
        stderr: String,
        stops_on_line: bool,
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
            }
        }

        fn with_frames(frames: [u64; 2], buffer_drops: u64) -> Self {
            Self {
                frames: Some((frames, buffer_drops)),
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
            Ok(LinkHandle(0))
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

        fn queue_packets(
            &mut self,
            _: LinkHandle,
            direction: Direction,
        ) -> Result<Option<u64>, Error> {
            self.calls.push(format!("count {direction:?}"));
            Ok(self.counts.map(|counts| match direction {
                Direction::Forward => counts[0],
                Direction::Reverse => counts[1],
            }))
        }

        fn node_counts(
            &mut self,
            _: LinkHandle,
            direction: Direction,
        ) -> Result<Option<NodeCounts>, Error> {
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
            let played = match argv[1].as_str() {
                "recv" if self.trouble == Some("recv-exits-early") => Played {
                    lines: VecDeque::new(),
                    exit: Some(ProgramExit::Code(1)),
                    stderr: "cannot listen on 10.100.0.2:5000: address in use".to_owned(),
                    stops_on_line: false,
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
                    Played {
                        lines: VecDeque::from([format!(
                            r#"{{"event":"ready","flow":0,"address":"{}"}}"#,
                            argument(argv, "--bind")
                        )]),
                        exit: None,
                        stderr: String::new(),
                        stops_on_line: true,
                    }
                }
                "send" if self.trouble == Some("send-fails") => Played {
                    lines: VecDeque::new(),
                    exit: Some(ProgramExit::Code(1)),
                    stderr: "cannot reach 10.100.0.2:5000: network is unreachable".to_owned(),
                    stops_on_line: false,
                },
                "send" if self.trouble == Some("send-refused") => {
                    // Two of ten sequence numbers were refused by the kernel: eight packets went into the link
                    Played {
                        lines: VecDeque::from([r#"{"event":"sent","flow":0,"packets":8,"first_send_ns":1000,"last_send_ns":900001000,"max_lateness_ns":1234,"errors":2,"first_error":"Network is unreachable"}"#.to_owned()]),
                        exit: Some(ProgramExit::Code(0)),
                        stderr: String::new(),
                        stops_on_line: false,
                    }
                }
                "send" => {
                    assert_eq!(argument(argv, "--rate"), "800000");
                    assert_eq!(argument(argv, "--size"), "1000");
                    assert_eq!(argument(argv, "--duration-ns"), "1000000000");
                    assert!(argument(argv, "--start-at-ns").parse::<u64>().unwrap() > 0);
                    Played {
                        lines: VecDeque::from([r#"{"event":"sent","flow":0,"packets":10,"first_send_ns":1000,"last_send_ns":900001000,"max_lateness_ns":1234,"errors":0,"first_error":null}"#.to_owned()]),
                        exit: Some(ProgramExit::Code(0)),
                        stderr: String::new(),
                        stops_on_line: false,
                    }
                }
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
            })
        }

        fn send_line(&mut self, program: ProgramHandle, line: &str) -> Result<(), Error> {
            self.calls.push(format!("line {line}"));
            let played = &mut self.programs[program.0 as usize];
            if played.stops_on_line && line == "stop" {
                played.lines.push_back(r#"{"event":"received","flow":0,"packets":8,"foreign":0,"first_receive_ns":1,"last_receive_ns":2}"#.to_owned());
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
        assert_eq!(workload.to_address, "10.100.0.2:5000");
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

    /// D-040: the packets that were sent are lost in the queue, dropped by the node, or delivered, and the
    /// result says how many of each. The fake sent 10 and the receiver got 8; the node counted 9 frames.
    #[test]
    fn every_packet_is_accounted_for_as_lost_in_the_queue_dropped_by_the_node_or_delivered() {
        let mut backend = Fake::with_frames([9, 0], 0);
        let options = options("account");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        let [account] = result.directions.as_slice() else {
            panic!("{:?}", result.directions);
        };
        assert_eq!(
            (account.link.as_str(), account.direction.as_str()),
            ("l", "forward")
        );
        assert_eq!(
            (
                account.sent,
                account.reached_node,
                account.delivered,
                account.lost_in_queue,
                account.dropped_corrupted,
                account.receive_buffer_drops
            ),
            (10, 9, 8, Some(1), 1, 0)
        );
        // lost_in_queue + dropped_corrupted + delivered = sent
        assert_eq!(
            account.lost_in_queue.unwrap() + account.dropped_corrupted + account.delivered,
            account.sent
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    /// Canary: a datagram that the node dropped for a full buffer is not called corruption, and a frame that
    /// arrived and was not delivered is not called lost.
    #[test]
    fn a_full_receive_buffer_is_not_corruption() {
        let mut backend = Fake::with_frames([9, 0], 1);
        let options = options("account-buffer");
        let result = run(&mut backend, &scenario(), &options).unwrap();
        let account = &result.directions[0];
        assert_eq!(
            (
                account.dropped_corrupted,
                account.receive_buffer_drops,
                account.lost_in_queue
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
        let options = options("account-duplicate");
        let result = run(&mut backend, &scenario, &options).unwrap();
        let account = &result.directions[0];
        assert_eq!(
            (account.lost_in_queue, account.dropped_corrupted),
            (None, 1)
        );
        let _ = std::fs::remove_dir_all(&options.out_dir);
    }

    #[test]
    fn a_backend_that_cannot_count_the_node_leaves_the_account_out() {
        let mut backend = Fake::new(None);
        let options = options("account-none");
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
        let mut options = options("export");
        options.export_packets = true;
        let result = run(&mut backend, &scenario(), &options).unwrap();
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
        // A run that is long over is clamped to its length
        control.workloads_started(
            Instant::now().checked_sub(Duration::from_secs(60)).unwrap(),
            Duration::from_secs(10),
        );
        control.checkpoint().unwrap();
        let last = recorder.events.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last,
            RunEvent::Progress {
                elapsed: Duration::from_secs(10),
                total: Duration::from_secs(10)
            }
        );
        control.workloads_done();
        control.workloads_done();
        let events = recorder.events.lock().unwrap();
        let done = events
            .iter()
            .filter(|e| matches!(e, RunEvent::Progress { elapsed, total } if elapsed == total))
            .count();
        // the clamped one and the one of `workloads_done`, which says it once
        assert_eq!(done, 2, "{events:?}");
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
}
