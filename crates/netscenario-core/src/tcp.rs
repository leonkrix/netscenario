//! The TCP workloads of a run: the servers and the clients in the nodes, and what they wrote turned into a result (WRK-04,
//! WRK-05, PRB-12; D-086).
//!
//! The agents do the work (`docs/AGENT.md`); this module plans them from the scenario, starts them in the right order, waits for
//! them, and reads their files. A TCP workload is driven by feedback, so it is not packet-synchronous and the run says that it is
//! `statistical` (see [`Reproducibility`](crate::result::Reproducibility)).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use netscenario_agent::events::{Event, ServeSummary, TcpSummary};
use netscenario_agent::streams::{self, Kind, STATUS_FAILED, STATUS_LAST};
use netscenario_model::{DEFAULT_CONGESTION, DEFAULT_TIMEOUT, IpFamily, Scenario, Workload};

use crate::backend::{Backend, NodeHandle, ProgramHandle};
use crate::error::Error;
use crate::provision::Provisioned;
use crate::result::{Goodput, RequestInfo, TcpResult, TransferInfo};
use crate::run::{
    Control, RunOptions, agent_program, event, failure, strings, wait_for_end, wait_for_line,
};
use crate::stats::DelaySummary;

/// The first port of the TCP servers: workload `n` listens on `TCP_BASE_PORT + n`, next to the ports of the UDP receivers.
const TCP_BASE_PORT: u16 = 6000;

/// The time between two samples of `TCP_INFO`.
const INFO_INTERVAL: Duration = Duration::from_millis(100);

/// The longest interval in which a server counts the payload it read: a bin of the goodput is right to within this.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(10);

/// How long a client may take, after the end of the measured phase, to be done: the data in flight is still delivered and the
/// server says that it has read it, and the agent gives a connection two minutes for that.
const CLIENT_FINISH: Duration = Duration::from_secs(130);

/// How long a server may take to answer `stop`.
const SERVER_STOP: Duration = Duration::from_secs(30);

/// What a workload sends.
#[derive(Debug, Clone)]
enum Spec {
    Bulk {
        bytes: Option<u64>,
    },
    Exchange {
        request: u64,
        response: u64,
        interval_ns: Option<u64>,
        timeout_ns: u64,
    },
}

/// What the two TCP kinds of workload have in common, for the plan.
struct Shape<'a> {
    id: &'a str,
    from: &'a netscenario_model::Name,
    to: &'a netscenario_model::Name,
    ip: Option<IpFamily>,
    start: Option<netscenario_model::Duration>,
    stop: Option<netscenario_model::Duration>,
    connections: u32,
    cc: &'a Option<netscenario_model::Name>,
    spec: Spec,
}

impl<'a> Shape<'a> {
    /// The shape of a TCP workload, or `None` for any other.
    fn of(workload: &'a Workload) -> Option<Self> {
        Some(match workload {
            Workload::TcpBulk(bulk) => Self {
                id: bulk.id.as_str(),
                from: &bulk.from,
                to: &bulk.to,
                ip: bulk.ip,
                start: bulk.start,
                stop: bulk.stop,
                connections: bulk.connections,
                cc: &bulk.cc,
                spec: Spec::Bulk {
                    bytes: bulk.bytes.map(netscenario_model::Size::bytes),
                },
            },
            Workload::RequestResponse(exchange) => Self {
                id: exchange.id.as_str(),
                from: &exchange.from,
                to: &exchange.to,
                ip: exchange.ip,
                start: exchange.start,
                stop: exchange.stop,
                connections: exchange.connections,
                cc: &exchange.cc,
                spec: Spec::Exchange {
                    request: exchange.request.bytes(),
                    response: exchange.response.bytes(),
                    interval_ns: exchange.interval.map(netscenario_model::Duration::as_nanos),
                    timeout_ns: exchange.timeout.unwrap_or(DEFAULT_TIMEOUT).as_nanos(),
                },
            },
            _ => return None,
        })
    }
}

/// One TCP workload as the run plans it.
#[derive(Debug, Clone)]
struct Plan {
    index: usize,
    id: String,
    spec: Spec,
    from: String,
    to: String,
    from_node: NodeHandle,
    to_node: NodeHandle,
    from_address: String,
    /// The address and the port the server listens on, as the agent reads it.
    to_address: String,
    congestion: String,
    connections: u32,
    start: Duration,
    window: Duration,
    client_file: PathBuf,
    server_file: PathBuf,
}

/// A workload that is running: its programs and what they said when they ended.
#[derive(Debug)]
struct Running {
    plan: Plan,
    server: Option<ProgramHandle>,
    client: Option<ProgramHandle>,
    served: Option<ServeSummary>,
    client_said: Option<TcpSummary>,
}

/// The TCP workloads of a run.
#[derive(Debug, Default)]
pub(crate) struct Tcp {
    running: Vec<Running>,
    bin: Duration,
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Tcp {
    /// Plans the TCP workloads of `scenario`. `bin` is the width of a bin of the goodput.
    pub(crate) fn plan(
        scenario: &Scenario,
        provisioned: &Provisioned,
        options: &RunOptions,
        bin: Duration,
    ) -> Result<Self, Error> {
        let mut running = Vec::new();
        for workload in &scenario.workloads {
            let Some(shape) = Shape::of(workload) else {
                continue;
            };
            let index = running.len();
            let node = |name: &netscenario_model::Name| {
                provisioned
                    .nodes
                    .iter()
                    .find(|(candidate, _)| candidate == name)
                    .map(|(_, handle)| *handle)
                    .ok_or_else(|| {
                        failure("find a node", format!("the node '{name}' was not built"))
                    })
            };
            let family = shape
                .ip
                .unwrap_or_else(|| scenario.topology.ip.default_family());
            let address = |name: &netscenario_model::Name| {
                provisioned
                    .node_addresses
                    .iter()
                    .find(|(candidate, _)| candidate == name)
                    .and_then(|(_, ips)| ips.of(family))
                    .map(|address| address.to_string())
                    .ok_or_else(|| {
                        failure(
                            "find an address",
                            format!(
                                "the node '{name}' has no {} address",
                                if family == IpFamily::V6 {
                                    "IPv6"
                                } else {
                                    "IPv4"
                                }
                            ),
                        )
                    })
            };
            let start = shape.start.map_or(Duration::ZERO, Duration::from);
            let stop = shape
                .stop
                .map_or_else(|| Duration::from(scenario.duration), Duration::from);
            let port = TCP_BASE_PORT + u16::try_from(index).unwrap_or(0);
            running.push(Running {
                plan: Plan {
                    index,
                    id: shape.id.to_owned(),
                    spec: shape.spec,
                    from: shape.from.to_string(),
                    to: shape.to.to_string(),
                    from_node: node(shape.from)?,
                    to_node: node(shape.to)?,
                    from_address: address(shape.from)?,
                    to_address: crate::run::socket_text(&address(shape.to)?, port),
                    congestion: shape
                        .cc
                        .as_ref()
                        .map_or(DEFAULT_CONGESTION, netscenario_model::Name::as_str)
                        .to_owned(),
                    connections: shape.connections,
                    start,
                    window: stop.saturating_sub(start),
                    client_file: absolute(
                        &options.out_dir.join(format!("tcp-{index}.client.streams")),
                    ),
                    server_file: absolute(
                        &options.out_dir.join(format!("tcp-{index}.server.streams")),
                    ),
                },
                server: None,
                client: None,
                served: None,
                client_said: None,
            });
        }
        Ok(Self { running, bin })
    }

    /// How many servers there are.
    pub(crate) fn servers(&self) -> usize {
        self.running.len()
    }

    /// Whether the scenario has no TCP workload.
    pub(crate) fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// Why a run with TCP workloads is not exact: one sentence for each reason, none without TCP workloads.
    pub(crate) fn reasons(&self) -> Vec<String> {
        if self.is_empty() {
            return Vec::new();
        }
        vec![
            "TCP is driven by feedback (the windows, the timers and the acknowledgements of the kernel of this machine), so the same file gives a different run each time; repeat the run and read the interval".to_owned(),
        ]
    }

    /// Starts the servers and waits until each listens.
    pub(crate) fn start_servers(
        &mut self,
        backend: &mut dyn Backend,
        options: &RunOptions,
        control: &Control<'_>,
    ) -> Result<(), Error> {
        let interval = self.bin.clamp(Duration::from_millis(1), PROGRESS_INTERVAL);
        for running in &mut self.running {
            control.checkpoint()?;
            let plan = &running.plan;
            let what = format!("start the server of '{}'", plan.id);
            let mut arguments = strings(&["tcp-serve", "--bind", &plan.to_address, "--flow"]);
            arguments.push(plan.index.to_string());
            arguments.extend(strings(&[
                "--out",
                &plan.server_file.display().to_string(),
                "--congestion",
                &plan.congestion,
                "--progress-interval-ns",
                &nanos(interval).to_string(),
            ]));
            let program =
                backend.start_program(plan.to_node, &agent_program(options, &arguments))?;
            running.server = Some(program);
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
        }
        Ok(())
    }

    /// Starts the clients, all with one moment to begin at (`begin` on the clock of the records, plus the `start` of each).
    pub(crate) fn start_clients(
        &mut self,
        backend: &mut dyn Backend,
        options: &RunOptions,
        begin: u64,
    ) -> Result<(), Error> {
        for running in &mut self.running {
            let plan = &running.plan;
            let mut arguments = match &plan.spec {
                Spec::Bulk { bytes } => {
                    let mut arguments = strings(&["tcp-bulk"]);
                    if let Some(bytes) = bytes {
                        arguments.extend(["--bytes".to_owned(), bytes.to_string()]);
                    }
                    arguments
                }
                Spec::Exchange {
                    request,
                    response,
                    interval_ns,
                    timeout_ns,
                } => {
                    let mut arguments = strings(&["tcp-request"]);
                    arguments.extend([
                        "--request-bytes".to_owned(),
                        request.to_string(),
                        "--response-bytes".to_owned(),
                        response.to_string(),
                        "--timeout-ns".to_owned(),
                        timeout_ns.to_string(),
                    ]);
                    if let Some(interval) = interval_ns {
                        arguments.extend(["--interval-ns".to_owned(), interval.to_string()]);
                    }
                    arguments
                }
            };
            arguments.extend(strings(&["--to", &plan.to_address, "--flow"]));
            arguments.push(plan.index.to_string());
            arguments.extend(strings(&[
                "--out",
                &plan.client_file.display().to_string(),
                "--congestion",
                &plan.congestion,
                "--connections",
                &plan.connections.to_string(),
                "--duration-ns",
                &nanos(plan.window).to_string(),
                "--start-at-ns",
                &(begin + nanos(plan.start)).to_string(),
                "--info-interval-ns",
                &nanos(INFO_INTERVAL).to_string(),
            ]));
            running.client =
                Some(backend.start_program(plan.from_node, &agent_program(options, &arguments))?);
        }
        Ok(())
    }

    /// Waits until the clients are done (they end by themselves; a run that was interrupted tells them to stop), then stops the
    /// servers and takes what they said.
    pub(crate) fn finish(
        &mut self,
        backend: &mut dyn Backend,
        control: &Control<'_>,
        interrupted: bool,
    ) -> Result<(), Error> {
        let deadline = Instant::now() + CLIENT_FINISH;
        for running in &mut self.running {
            let Some(client) = running.client else {
                continue;
            };
            let what = format!("wait for the client of '{}'", running.plan.id);
            if interrupted {
                backend.send_line(client, "stop")?;
            }
            let lines = wait_for_end(backend, client, deadline, &what, control)?;
            running.client_said = match lines.last().map(|line| event(line, &what)).transpose()? {
                Some(Event::Tcp(summary)) => Some(summary),
                other => {
                    return Err(failure(
                        what,
                        format!("the client should end with 'tcp', it said {other:?}"),
                    ));
                }
            };
        }
        for running in &mut self.running {
            let Some(server) = running.server else {
                continue;
            };
            let what = format!("stop the server of '{}'", running.plan.id);
            backend.send_line(server, "stop")?;
            let lines = wait_for_end(
                backend,
                server,
                Instant::now() + SERVER_STOP,
                &what,
                control,
            )?;
            running.served = match lines.last().map(|line| event(line, &what)).transpose()? {
                Some(Event::Served(summary)) => Some(summary),
                other => {
                    return Err(failure(
                        what,
                        format!("the server should end with 'served', it said {other:?}"),
                    ));
                }
            };
        }
        Ok(())
    }

    /// Ends every program of the TCP workloads that is still there, after an error.
    pub(crate) fn kill_all(&mut self, backend: &mut dyn Backend) {
        for running in &self.running {
            for program in [running.client, running.server].into_iter().flatten() {
                let _ = backend.send_line(program, "stop");
            }
        }
    }

    /// Reads the files and makes the results, with times from `begin`.
    pub(crate) fn into_results(self, begin: u64, out_dir: &Path) -> Result<Vec<TcpResult>, Error> {
        let bin = self.bin;
        self.running
            .into_iter()
            .map(|running| result_of(running, begin, bin, out_dir))
            .collect()
    }
}

fn read_streams(path: &Path) -> Result<streams::File, Error> {
    let file = std::fs::File::open(path)
        .map_err(|error| failure("read the records", format!("{}: {error}", path.display())))?;
    streams::read(std::io::BufReader::new(file))
        .map_err(|error| failure("read the records", format!("{}: {error}", path.display())))
}

fn relative_name(path: &Path, out_dir: &Path) -> String {
    let base = absolute(out_dir);
    path.strip_prefix(&base)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn result_of(
    running: Running,
    begin: u64,
    bin: Duration,
    out_dir: &Path,
) -> Result<TcpResult, Error> {
    let Running {
        plan,
        client_said,
        served,
        ..
    } = running;
    let client = read_streams(&plan.client_file)?;
    let server = read_streams(&plan.server_file)?;
    let said = client_said.ok_or_else(|| {
        failure(
            "read the summary of a client",
            format!("the client of '{}' said nothing", plan.id),
        )
    })?;
    let transfers = transfers_of(&client, begin);
    let (retransmitted_segments, rtt) = kernel_view(&client);
    let goodput = goodput_of(&server, &transfers, begin, bin);
    let requests = matches!(plan.spec, Spec::Exchange { .. }).then(|| requests_of(&client));
    let bytes_received = match plan.spec {
        Spec::Bulk { .. } => served.map_or(0, |served| served.bytes_received),
        Spec::Exchange { .. } => said.bytes_received,
    };
    Ok(TcpResult {
        id: plan.id,
        kind: match plan.spec {
            Spec::Bulk { .. } => "tcp-bulk",
            Spec::Exchange { .. } => "request-response",
        }
        .to_owned(),
        from: plan.from,
        to: plan.to,
        from_address: plan.from_address,
        to_address: plan.to_address,
        congestion: said.congestion.clone(),
        connections: said.connections,
        failed_connections: said.failed,
        start_ns: nanos(plan.start),
        window_ns: nanos(plan.window),
        bytes_sent: said.bytes_sent,
        bytes_received,
        goodput,
        transfers,
        retransmitted_segments,
        rtt,
        requests,
        first_error: said.first_error.clone(),
        info_error: said.info_error.clone(),
        stopped: said.stopped,
        client_file: relative_name(&plan.client_file, out_dir),
        server_file: relative_name(&plan.server_file, out_dir),
    })
}

/// A moment of the records as nanoseconds from `begin`.
fn since(begin: u64, t: u64) -> i64 {
    i64::try_from(t).unwrap_or(i64::MAX) - i64::try_from(begin).unwrap_or(0)
}

/// The life of each connection, from the records of the client.
fn transfers_of(client: &streams::File, begin: u64) -> Vec<TransferInfo> {
    let mut transfers: Vec<TransferInfo> = client
        .records
        .iter()
        .filter(|record| record.kind == Kind::Connection)
        .map(|record| TransferInfo {
            connection: record.connection,
            started_ns: since(begin, record.t),
            established_ns: since(begin, record.a),
            done_ns: since(begin, record.b),
            bytes_sent: record.c,
            bytes_received: record.d,
            ok: record.status != STATUS_FAILED,
        })
        .collect();
    transfers.sort_by_key(|transfer| transfer.connection);
    transfers
}

/// What the kernel saw: the segments retransmitted in all (the last sample of each connection counts them), and the smoothed round
/// trip time of the samples taken during the transfer, in nanoseconds.
fn kernel_view(client: &streams::File) -> (u64, Option<DelaySummary>) {
    // For each connection the latest sample, a last one (status 1) before any other
    let mut last: std::collections::BTreeMap<u32, (u8, u64, u64)> =
        std::collections::BTreeMap::new();
    for record in client
        .records
        .iter()
        .filter(|record| record.kind == Kind::Info)
    {
        let rank = u8::from(record.status == STATUS_LAST);
        let entry = last
            .entry(record.connection)
            .or_insert((rank, record.t, record.c));
        if (rank, record.t) >= (entry.0, entry.1) {
            *entry = (rank, record.t, record.c);
        }
    }
    let retransmitted = last.values().map(|(_, _, segments)| *segments).sum();
    let mut rtts: Vec<u64> = client
        .records
        .iter()
        .filter(|record| record.kind == Kind::Info && record.status != STATUS_LAST && record.a > 0)
        .map(|record| record.a.saturating_mul(1000))
        .collect();
    (retransmitted, DelaySummary::of(&mut rtts))
}

/// The payload that the server read, by time bin, and the mean over the transfer.
fn goodput_of(
    server: &streams::File,
    transfers: &[TransferInfo],
    begin: u64,
    bin: Duration,
) -> Option<Goodput> {
    let bin_ns = nanos(bin).max(1);
    let progress: Vec<_> = server
        .records
        .iter()
        .filter(|record| record.kind == Kind::Progress)
        .collect();
    if progress.is_empty() {
        return None;
    }
    let index = |t: u64| usize::try_from(t.saturating_sub(begin) / bin_ns).unwrap_or(usize::MAX);
    let bins = progress
        .iter()
        .map(|record| index(record.t))
        .max()
        .map_or(0, |last| last + 1);
    let mut bytes = vec![0_u128; bins];
    for record in &progress {
        bytes[index(record.t)] += u128::from(record.a);
    }
    let to_bits = |bytes: u128, over_ns: u128| {
        u64::try_from(bytes * 8 * 1_000_000_000 / over_ns.max(1)).unwrap_or(u64::MAX)
    };
    let done: Vec<&TransferInfo> = transfers.iter().filter(|transfer| transfer.ok).collect();
    let mean = done
        .iter()
        .map(|transfer| transfer.started_ns)
        .min()
        .zip(done.iter().map(|transfer| transfer.done_ns).max())
        .filter(|(first, last)| last > first)
        .map(|(first, last)| {
            let total: u128 = done
                .iter()
                .map(|transfer| u128::from(transfer.bytes_sent))
                .sum();
            to_bits(total, u128::from(u64::try_from(last - first).unwrap_or(0)))
        });
    Some(Goodput {
        bin_ns,
        bits_per_second: bytes
            .iter()
            .map(|&b| to_bits(b, u128::from(bin_ns)))
            .collect(),
        mean_bits_per_second: mean,
    })
}

/// The requests of a client and how long they took.
fn requests_of(client: &streams::File) -> RequestInfo {
    let all: Vec<_> = client
        .records
        .iter()
        .filter(|record| record.kind == Kind::Request)
        .collect();
    let answered: Vec<_> = all
        .iter()
        .filter(|record| record.status != STATUS_FAILED)
        .collect();
    let mut latency: Vec<u64> = answered
        .iter()
        .map(|record| record.b.saturating_sub(record.t))
        .collect();
    let mut round_trip: Vec<u64> = answered
        .iter()
        .map(|record| record.b.saturating_sub(record.a))
        .collect();
    RequestInfo {
        sent: all.len() as u64,
        answered: answered.len() as u64,
        failed: (all.len() - answered.len()) as u64,
        latency: DelaySummary::of(&mut latency),
        round_trip: DelaySummary::of(&mut round_trip),
        max_wait_ns: all
            .iter()
            .map(|record| record.a.saturating_sub(record.t))
            .max()
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscenario_agent::streams::{File, Record, STATUS_OK};

    fn record(kind: Kind, status: u8, connection: u32, t: u64, rest: [u64; 4]) -> Record {
        Record {
            kind,
            status,
            connection,
            t,
            a: rest[0],
            b: rest[1],
            c: rest[2],
            d: rest[3],
        }
    }

    fn file(records: Vec<Record>) -> File {
        File { flow: 0, records }
    }

    const BEGIN: u64 = 1_000_000_000;

    /// The bytes of the intervals add up to the bits of the bins, each interval in the bin that holds its start, and the bins run
    /// to the last one with data.
    #[test]
    fn the_goodput_puts_each_interval_in_the_bin_of_its_start() {
        let progress =
            |t: u64, bytes: u64| record(Kind::Progress, STATUS_OK, 0, BEGIN + t, [bytes, 1, 0, 0]);
        let server = file(vec![
            progress(0, 125_000),
            progress(10_000_000, 125_000),
            // The next bin of 100 ms
            progress(100_000_000, 250_000),
            // Two bins later: a bin with nothing in it between
            progress(310_000_000, 125_000),
        ]);
        let transfers = vec![TransferInfo {
            connection: 0,
            started_ns: 0,
            established_ns: 1,
            done_ns: 400_000_000,
            bytes_sent: 625_000,
            bytes_received: 0,
            ok: true,
        }];
        let goodput = goodput_of(&server, &transfers, BEGIN, Duration::from_millis(100)).unwrap();
        // 250,000 bytes in 100 ms is 20 mbit/s, 250,000 again, none, then 125,000
        assert_eq!(
            goodput.bits_per_second,
            [20_000_000, 20_000_000, 0, 10_000_000]
        );
        // 625,000 bytes in 400 ms from the first connect to the last end is 12.5 mbit/s
        assert_eq!(goodput.mean_bits_per_second, Some(12_500_000));
        assert_eq!(goodput.bin_ns, 100_000_000);
    }

    /// Canary: nothing read is no goodput, and a transfer that failed is not in the mean.
    #[test]
    fn no_data_is_no_goodput_and_a_failed_connection_is_left_out_of_the_mean() {
        assert_eq!(
            goodput_of(&file(Vec::new()), &[], BEGIN, Duration::from_secs(1)),
            None
        );
        let server = file(vec![record(
            Kind::Progress,
            STATUS_OK,
            0,
            BEGIN,
            [1_000, 1, 0, 0],
        )]);
        let transfer = |ok: bool, done_ns: i64| TransferInfo {
            connection: 0,
            started_ns: 0,
            established_ns: 0,
            done_ns,
            bytes_sent: 1_000,
            bytes_received: 0,
            ok,
        };
        let goodput = goodput_of(
            &server,
            &[
                transfer(true, 1_000_000_000),
                transfer(false, 50_000_000_000),
            ],
            BEGIN,
            Duration::from_secs(1),
        )
        .unwrap();
        // 1,000 bytes in the one second of the connection that did its work, not in the 50 s of the one that failed
        assert_eq!(goodput.mean_bits_per_second, Some(8_000));
        assert_eq!(
            goodput_of(
                &server,
                &[transfer(false, 1)],
                BEGIN,
                Duration::from_secs(1)
            )
            .unwrap()
            .mean_bits_per_second,
            None
        );
    }

    /// The retransmissions are those of the last sample of each connection, a sample marked last before any other, and the round
    /// trip time comes from the samples of the transfer in microseconds turned into nanoseconds.
    #[test]
    fn the_kernel_view_counts_the_last_sample_of_each_connection() {
        let info = |status: u8, connection: u32, t: u64, rtt_us: u64, retransmitted: u64| {
            record(
                Kind::Info,
                status,
                connection,
                t,
                [rtt_us, 10, retransmitted, 0],
            )
        };
        let client = file(vec![
            info(STATUS_OK, 0, 10, 40_000, 1),
            info(STATUS_OK, 0, 30, 50_000, 3),
            // The last sample, though an earlier one has a later time in the file
            info(STATUS_LAST, 0, 40, 0, 4),
            info(STATUS_OK, 0, 50, 60_000, 9),
            info(STATUS_OK, 1, 20, 42_000, 2),
            info(STATUS_OK, 1, 60, 44_000, 5),
        ]);
        let (retransmitted, rtt) = kernel_view(&client);
        // Connection 0: 4 (the last sample), connection 1: 5 (its latest)
        assert_eq!(retransmitted, 9);
        let rtt = rtt.unwrap();
        assert_eq!((rtt.min_ns, rtt.max_ns), (40_000_000, 60_000_000));
        // No samples: nothing retransmitted that anyone knows of, and no round trip time
        assert_eq!(kernel_view(&file(Vec::new())), (0, None));
    }

    /// Latency counts from the moment due, the round trip from the moment sent, and a request that failed is in neither.
    #[test]
    fn a_request_is_timed_from_when_it_was_due_and_a_failed_one_is_not_timed() {
        let request = |status: u8, due: u64, sent: u64, end: u64| {
            record(Kind::Request, status, 0, due, [sent, end, 100, 100])
        };
        let client = file(vec![
            request(STATUS_OK, 1_000, 1_000, 61_000),
            // Due at 2,000, sent at 12,000 because the connection was busy, answered at 72,000
            request(STATUS_OK, 2_000, 12_000, 72_000),
            request(STATUS_FAILED, 3_000, 3_500, 30_000_000),
        ]);
        let info = requests_of(&client);
        assert_eq!((info.sent, info.answered, info.failed), (3, 2, 1));
        let latency = info.latency.unwrap();
        let trip = info.round_trip.unwrap();
        assert_eq!((latency.min_ns, latency.max_ns), (60_000, 70_000));
        assert_eq!((trip.min_ns, trip.max_ns), (60_000, 60_000));
        // The longest a request was sent after it was due
        assert_eq!(info.max_wait_ns, 10_000);
        // A client that sent nothing has nothing to say about time
        let none = requests_of(&file(Vec::new()));
        assert_eq!((none.sent, none.latency, none.round_trip), (0, None, None));
    }

    /// The connections come back in the order of their numbers, with their moments from the start of the measured phase, and a
    /// connection that failed is marked.
    #[test]
    fn the_connections_are_in_order_and_a_failed_one_is_marked() {
        let client = file(vec![
            record(
                Kind::Connection,
                STATUS_FAILED,
                1,
                BEGIN + 10,
                [BEGIN + 20, BEGIN + 30, 5, 0],
            ),
            record(
                Kind::Connection,
                STATUS_OK,
                0,
                BEGIN + 1,
                [BEGIN + 2, BEGIN + 3, 7, 0],
            ),
        ]);
        let transfers = transfers_of(&client, BEGIN);
        assert_eq!(
            transfers.iter().map(|t| t.connection).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(
            (
                transfers[0].started_ns,
                transfers[0].established_ns,
                transfers[0].done_ns,
                transfers[0].ok
            ),
            (1, 2, 3, true)
        );
        assert_eq!(transfers[0].duration_ns(), 2);
        assert!(!transfers[1].ok);
        assert_eq!(transfers[0].bytes_sent, 7);
    }
}
