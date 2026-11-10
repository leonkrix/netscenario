//! The command line of `netscenario-agent`.
//!
//! ```text
//! netscenario-agent send --to <addr> --rate <bits/s> --size <bytes> --duration-ns <ns>
//!                        [--flow <n>] [--start-at-ns <monotonic ns>] [--udp-checksum <on|off>] [--dont-fragment]
//!                        [--barriers <packet>:<id>,...]
//! netscenario-agent recv --bind <addr> --out <file> [--flow <n>] [--max-time-ns <ns>] [--ignore-stdin]
//!                        [--udp-checksum <on|off>] [--wire-out <file>]
//! ```
//!
//! Numbers are plain integers (bits per second, bytes, nanoseconds): the units of a scenario file are
//! resolved by the program that starts the agent, so the agent has no second parser for them. Both commands
//! print one JSON line (see [`events`](crate::events)) when they are done; `recv` prints one more when it is
//! ready. `recv` runs until a line `stop` or the end of its standard input arrives, or until `--max-time-ns`.
//!
//! Exit codes: 0 done, 1 failed (the reason is on standard error), 2 the command line is wrong.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::capture::{Capture, CaptureConfig};
use crate::events::Event;
use crate::receiver::{ReceiveConfig, Receiver};
use crate::schedule::{Distribution, Schedule, Stream};
use crate::sender::{Barrier, Barriers, SendConfig, run_with as send_with};
use crate::tap::Tap;
use crate::tcp::{
    self, Amount, BulkConfig, ClientConfig, RequestConfig, ServeConfig, Server, run_bulk,
    run_requests,
};

/// The exit code for success.
pub const OK: u8 = 0;
/// The exit code for a failure while running.
pub const FAILED: u8 = 1;
/// The exit code for a wrong command line.
pub const USAGE: u8 = 2;

const USAGE_TEXT: &str = "\
Usage:
  netscenario-agent send --to <addr> --size <bytes> --duration-ns <ns> [--rate <bits/s>]
                         [--pattern cbr|poisson|onoff|bursty|trace] [--trace <file>] [--seed <n>] [--distribution fixed|exponential]
                         [--on-ns <ns> --off-ns <ns>] [--burst-packets <n> --gap-ns <ns> --pause-ns <ns>]
                         [--flow <n>] [--start-at-ns <monotonic ns>] [--udp-checksum <on|off>] [--dont-fragment]
                         [--barriers <packet>:<id>,...]
  netscenario-agent recv --bind <addr> --out <file> [--flow <n>] [--max-time-ns <ns>] [--ignore-stdin]
                         [--udp-checksum <on|off>] [--wire-out <file>] [--echo-to <addr>]
  netscenario-agent capture --interface <name> --out <file.pcap> [--flow <n>] [--receive-buffer <bytes>]
  netscenario-agent tcp-serve --bind <addr> --out <file> [--flow <n>] [--congestion <name>] [--progress-interval-ns <ns>]
                              [--max-time-ns <ns>] [--ignore-stdin]
  netscenario-agent tcp-bulk --to <addr> --out <file> (--bytes <n> | --duration-ns <ns> | both) [--flow <n>] [--connections <n>]
                             [--congestion <name>] [--chunk-bytes <n>] [--start-at-ns <monotonic ns>]
                             [--info-interval-ns <ns>] [--finish-timeout-ns <ns>] [--ignore-stdin]
  netscenario-agent tcp-request --to <addr> --out <file> --request-bytes <n> --response-bytes <n> --duration-ns <ns>
                                [--interval-ns <ns>] [--flow <n>] [--connections <n>] [--congestion <name>]
                                [--start-at-ns <monotonic ns>] [--info-interval-ns <ns>] [--timeout-ns <ns>] [--ignore-stdin]

The agent is started by netscenario inside a node; see docs/AGENT.md.
";

/// The options of a command: `--name value` pairs and the flags that take no value.
struct Options(HashMap<String, String>);

impl Options {
    fn parse(args: &[String], with_value: &[&str], flags: &[&str]) -> Result<Self, String> {
        let mut map = HashMap::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let name = arg.strip_prefix("--").unwrap_or(arg);
            if flags.contains(&name) && arg.starts_with("--") {
                map.insert(name.to_owned(), String::new());
            } else if with_value.contains(&name) && arg.starts_with("--") {
                let value = args
                    .next()
                    .ok_or_else(|| format!("the option --{name} needs a value"))?;
                if map.insert(name.to_owned(), value.clone()).is_some() {
                    return Err(format!("the option --{name} is given twice"));
                }
            } else {
                let known: Vec<String> = with_value
                    .iter()
                    .chain(flags)
                    .map(|n| format!("--{n}"))
                    .collect();
                return Err(format!(
                    "unknown option '{arg}'; the options are: {}",
                    known.join(", ")
                ));
            }
        }
        Ok(Self(map))
    }

    fn required<T: std::str::FromStr>(&self, name: &str) -> Result<T, String> {
        self.optional(name)?
            .ok_or_else(|| format!("the option --{name} is required"))
    }

    fn optional<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, String> {
        self.0
            .get(name)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| format!("'{value}' is not a valid value for --{name}"))
            })
            .transpose()
    }

    fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

/// Runs the agent with `args` (the first is the program name) and returns the exit code. `stdin` is where
/// `recv` listens for `stop`.
pub fn run<I, S>(
    args: I,
    stdin: impl BufRead + Send + 'static,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .skip(1)
        .map(|a| a.as_ref().to_owned())
        .collect();
    let Some((command, rest)) = args.split_first() else {
        let _ = write!(err, "netscenario-agent: no command given\n\n{USAGE_TEXT}");
        return USAGE;
    };
    let result: Result<u8, Problem> = match command.as_str() {
        "-V" | "--version" => {
            let _ = writeln!(out, "netscenario-agent {}", env!("CARGO_PKG_VERSION"));
            return OK;
        }
        "-h" | "--help" => {
            let _ = write!(out, "{USAGE_TEXT}");
            return OK;
        }
        "send" => Options::parse(rest, SEND_OPTIONS, &["dont-fragment"])
            .map_err(Problem::Usage)
            .and_then(|options| send_command(&options, stdin, out)),
        "recv" => Options::parse(
            rest,
            &[
                "bind",
                "out",
                "flow",
                "max-time-ns",
                "udp-checksum",
                "wire-out",
                "echo-to",
            ],
            &["ignore-stdin"],
        )
        .map_err(Problem::Usage)
        .and_then(|options| recv_command(&options, stdin, out)),
        "capture" => Options::parse(rest, &["interface", "out", "flow", "receive-buffer"], &[])
            .map_err(Problem::Usage)
            .and_then(|options| capture_command(&options, stdin, out)),
        "tcp-serve" => Options::parse(rest, TCP_SERVE_OPTIONS, &["ignore-stdin"])
            .map_err(Problem::Usage)
            .and_then(|options| tcp_serve_command(&options, stdin, out)),
        "tcp-bulk" => Options::parse(rest, TCP_BULK_OPTIONS, &["ignore-stdin"])
            .map_err(Problem::Usage)
            .and_then(|options| tcp_bulk_command(&options, stdin, out)),
        "tcp-request" => Options::parse(rest, TCP_REQUEST_OPTIONS, &["ignore-stdin"])
            .map_err(Problem::Usage)
            .and_then(|options| tcp_request_command(&options, stdin, out)),
        other => Err(Problem::Usage(format!("unknown command '{other}'"))),
    };
    match result {
        Ok(code) => code,
        Err(Problem::Usage(message)) => {
            let _ = writeln!(err, "netscenario-agent: {message}\n\n{USAGE_TEXT}");
            USAGE
        }
        Err(Problem::Failed(message)) => {
            let _ = writeln!(err, "netscenario-agent: {message}");
            FAILED
        }
    }
}

/// What went wrong: the command line, or the work.
enum Problem {
    Usage(String),
    Failed(String),
}

impl From<String> for Problem {
    fn from(message: String) -> Self {
        Problem::Usage(message)
    }
}

fn say(out: &mut impl Write, event: &Event) {
    if let Ok(line) = serde_json::to_string(event) {
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// The answers of the run to the barriers of a sender: the lines `go <id>` on its standard input (D-038), the line
/// `stop` that ends the stream early (D-066), and the lines `hold <id>` with which the run asks the sender to stop
/// before its next packet (D-096).
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
    /// The run said `stop`. Looked at before every packet, so it is not behind the lock.
    stopped: AtomicBool,
    /// The run asked for a hold that the sender has not taken yet. Looked at before every packet, so it is not
    /// behind the lock.
    hold_asked: AtomicBool,
}

#[derive(Default)]
struct GateState {
    released: HashSet<u32>,
    /// The holds that the run asked for and the sender has not taken, in the order they were asked.
    holds: std::collections::VecDeque<u32>,
    /// The input has ended: nobody will answer any more, so nothing is waited for (a sender whose run died does
    /// not wait for ever).
    closed: bool,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GateState::default()),
            changed: Condvar::new(),
            stopped: AtomicBool::new(false),
            hold_asked: AtomicBool::new(false),
        })
    }

    /// Reads the answers from `input` until it ends.
    fn listen(self: &Arc<Self>, input: impl BufRead + Send + 'static) {
        let gate = Arc::clone(self);
        std::thread::spawn(move || {
            for line in input.lines() {
                let Ok(line) = line else { break };
                if line.trim() == "stop" {
                    // A sender that waits for an answer is let go, and the stream ends before its next packet
                    gate.stopped.store(true, Ordering::SeqCst);
                    let _held = gate.state.lock().unwrap_or_else(PoisonError::into_inner);
                    gate.changed.notify_all();
                    continue;
                }
                if let Some(id) = line
                    .trim()
                    .strip_prefix("hold ")
                    .and_then(|id| id.trim().parse::<u32>().ok())
                {
                    let mut state = gate.state.lock().unwrap_or_else(PoisonError::into_inner);
                    state.holds.push_back(id);
                    gate.hold_asked.store(true, Ordering::SeqCst);
                    gate.changed.notify_all();
                    continue;
                }
                if let Some(id) = line
                    .trim()
                    .strip_prefix("go ")
                    .and_then(|id| id.trim().parse::<u32>().ok())
                {
                    let mut state = gate.state.lock().unwrap_or_else(PoisonError::into_inner);
                    state.released.insert(id);
                    gate.changed.notify_all();
                }
            }
            let mut state = gate.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.closed = true;
            gate.changed.notify_all();
        });
    }

    /// Waits until `id` is answered, the input has ended, or `limit` has passed. `true` if it was answered or the
    /// input has ended.
    fn wait(&self, id: u32, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if state.released.contains(&id) || state.closed || self.stopped.load(Ordering::SeqCst) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// How long a sender waits for the answer to a barrier before it goes on without it. The run answers in the
/// time of a change (milliseconds); this is the time after which something is broken.
const BARRIER_TIMEOUT: Duration = Duration::from_secs(10);

/// The barriers of a sender over the standard streams: it asks with a line on `out`, the run answers on the input.
struct StandardBarriers<'a, W: Write> {
    flow: u32,
    out: &'a mut W,
    gate: Arc<Gate>,
}

impl<W: Write> Barriers for StandardBarriers<'_, W> {
    fn ask(&mut self, id: u32, packet: u64) {
        say(
            self.out,
            &Event::Barrier {
                flow: self.flow,
                id,
                packet,
            },
        );
    }

    fn wait(&mut self, id: u32) -> bool {
        self.gate.wait(id, BARRIER_TIMEOUT)
    }

    fn stopped(&self) -> bool {
        self.gate.stopped.load(Ordering::SeqCst)
    }

    fn hold_asked(&self) -> bool {
        self.gate.hold_asked.load(Ordering::SeqCst)
    }

    fn take_hold(&mut self) -> Option<u32> {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let id = state.holds.pop_front();
        if state.holds.is_empty() {
            self.gate.hold_asked.store(false, Ordering::SeqCst);
        }
        id
    }
}

/// `12:0,40:1` as the barriers before packet 12 (number 0) and before packet 40 (number 1), in the order of the
/// packets.
fn parse_barriers(text: &str) -> Result<Vec<Barrier>, String> {
    let mut barriers = Vec::new();
    for part in text.split(',').filter(|part| !part.is_empty()) {
        let invalid = || format!("'{part}' is not a barrier (packet:id)");
        let (packet, id) = part.split_once(':').ok_or_else(invalid)?;
        barriers.push(Barrier {
            packet: packet.parse().map_err(|_| invalid())?,
            id: id.parse().map_err(|_| invalid())?,
        });
    }
    barriers.sort_by_key(|barrier| (barrier.packet, barrier.id));
    Ok(barriers)
}

/// The options of `send` that take a value.
const SEND_OPTIONS: &[&str] = &[
    "to",
    "rate",
    "size",
    "duration-ns",
    "pattern",
    "seed",
    "distribution",
    "on-ns",
    "off-ns",
    "burst-packets",
    "gap-ns",
    "pause-ns",
    "flow",
    "start-at-ns",
    "udp-checksum",
    "barriers",
    "trace",
];

/// The schedule that the words after `send` describe, as the agent reads them. The run uses this to check that the words
/// it writes say what it means.
///
/// # Errors
///
/// Text that names the option that is wrong or missing.
pub fn schedule_from_words(words: &[String]) -> Result<Schedule, String> {
    schedule_of(&Options::parse(words, SEND_OPTIONS, &[])?)
}

/// The schedule that the options of `send` describe: `--pattern` (constant bit rate by default) and what that pattern needs.
fn schedule_of(options: &Options) -> Result<Schedule, String> {
    let pattern: String = options
        .optional("pattern")?
        .unwrap_or_else(|| "cbr".to_owned());
    let seed: u64 = options.optional("seed")?.unwrap_or(0);
    // A trace has a size for each packet; the other patterns have one size
    if pattern == "trace" {
        let path: String = options.required("trace")?;
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read the trace {path}: {error}"))?;
        let trace =
            crate::trace::Trace::from_text(&text).map_err(|why| format!("{path}: {why}"))?;
        return Ok(Schedule::Trace(std::sync::Arc::new(trace)));
    }
    let packet_size: u64 = options.required("size")?;
    let distribution = match options.optional::<String>("distribution")?.as_deref() {
        None | Some("fixed") => Distribution::Fixed,
        Some("exponential") => Distribution::Exponential,
        Some(other) => {
            return Err(format!(
                "--distribution is fixed or exponential, not '{other}'"
            ));
        }
    };
    let stream = || -> Result<Stream, String> {
        Ok(Stream {
            bits_per_second: options.required("rate")?,
            packet_size,
        })
    };
    Ok(match pattern.as_str() {
        "cbr" => Schedule::Cbr(stream()?),
        "poisson" => Schedule::Poisson {
            stream: stream()?,
            seed,
        },
        "onoff" => Schedule::OnOff {
            stream: stream()?,
            on_ns: options.required("on-ns")?,
            off_ns: options.required("off-ns")?,
            distribution,
            seed,
        },
        "bursty" => Schedule::Bursty {
            packet_size,
            packets: options.required("burst-packets")?,
            gap_ns: options.required("gap-ns")?,
            pause_ns: options.required("pause-ns")?,
            distribution,
            seed,
        },
        other => {
            return Err(format!(
                "--pattern is cbr, poisson, onoff, bursty or trace, not '{other}'"
            ));
        }
    })
}

fn send_command(
    options: &Options,
    stdin: impl BufRead + Send + 'static,
    out: &mut impl Write,
) -> Result<u8, Problem> {
    let barriers = options
        .optional::<String>("barriers")?
        .map(|text| parse_barriers(&text))
        .transpose()?
        .unwrap_or_default();
    let config = SendConfig {
        target: options.required::<SocketAddr>("to")?,
        flow: options.optional("flow")?.unwrap_or(0),
        schedule: schedule_of(options)?,
        duration_ns: options.required("duration-ns")?,
        start_at_ns: options.optional("start-at-ns")?,
        udp_checksum: options.optional("udp-checksum")?.unwrap_or_default(),
        dont_fragment: options.has("dont-fragment"),
        barriers,
    };
    // The standard input is read for the answers to the barriers and for `stop`, also by a stream without barriers
    let gate = Gate::new();
    gate.listen(stdin);
    let mut port = StandardBarriers {
        flow: config.flow,
        out: &mut *out,
        gate,
    };
    let summary =
        send_with(&config, &mut port).map_err(|error| Problem::Failed(error.to_string()))?;
    say(out, &Event::Sent(summary));
    Ok(OK)
}

/// `capture --interface <name> --out <file>`: writes the frames of an interface of the node as pcap until it reads `stop` (or its
/// input ends), and says what it wrote and whether it is whole (D-073).
fn capture_command(
    options: &Options,
    stdin: impl BufRead + Send + 'static,
    out: &mut impl Write,
) -> Result<u8, Problem> {
    let config = CaptureConfig {
        interface: options.required("interface")?,
        flow: options.optional("flow")?.unwrap_or(0),
        receive_buffer: options.optional("receive-buffer")?,
    };
    let path: String = options.required("out")?;
    let file = std::fs::File::create(&path)
        .map_err(|error| Problem::Failed(format!("cannot create {path}: {error}")))?;
    let capture =
        Capture::open(config.clone()).map_err(|error| Problem::Failed(error.to_string()))?;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            for line in stdin.lines() {
                if line.is_ok_and(|line| line.trim() == "stop") {
                    break;
                }
            }
            // `stop` or the end of the input
            stop.store(true, Ordering::SeqCst);
        });
    }
    say(
        out,
        &Event::CaptureReady {
            flow: config.flow,
            interface: config.interface,
            receive_buffer_bytes: capture.receive_buffer_bytes,
        },
    );
    let summary = capture
        .run(&stop, file)
        .map_err(|error| Problem::Failed(format!("cannot capture: {error}")))?;
    say(out, &Event::Captured(summary));
    Ok(OK)
}

fn recv_command(
    options: &Options,
    stdin: impl BufRead + Send + 'static,
    out: &mut impl Write,
) -> Result<u8, Problem> {
    let config = ReceiveConfig {
        bind: options.required::<SocketAddr>("bind")?,
        flow: options.optional("flow")?.unwrap_or(0),
        max_time_ns: options.optional("max-time-ns")?,
        udp_checksum: options.optional("udp-checksum")?.unwrap_or_default(),
        echo_to: options.optional("echo-to")?,
    };
    let path: String = options.required("out")?;
    let file = std::fs::File::create(&path)
        .map_err(|error| Problem::Failed(format!("cannot create {path}: {error}")))?;
    let receiver = Receiver::bind(&config)
        .map_err(|error| Problem::Failed(format!("cannot listen on {}: {error}", config.bind)))?;
    let address = receiver
        .local_addr()
        .map_err(|error| Problem::Failed(error.to_string()))?;
    let stop = Arc::new(AtomicBool::new(false));
    // The tap that names the damaged packets is a help: where it cannot be opened (no `CAP_NET_RAW`), the
    // receiver runs without it and says why
    let mut wire_error = None;
    let tap = match options.optional::<String>("wire-out")? {
        None => None,
        Some(path) => match open_tap(&config, &path) {
            Ok(opened) => Some(opened),
            Err(error) => {
                wire_error = Some(error);
                None
            }
        },
    };
    let tap = tap.map(|(tap, file)| {
        let stop = Arc::clone(&stop);
        let flow = config.flow;
        std::thread::spawn(move || tap.run(flow, &stop, file))
    });
    if !options.has("ignore-stdin") {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            for line in stdin.lines() {
                if line.is_ok_and(|line| line.trim() == "stop") {
                    break;
                }
            }
            // `stop` or the end of the input
            stop.store(true, Ordering::SeqCst);
        });
    }
    say(
        out,
        &Event::Ready {
            flow: config.flow,
            address: address.to_string(),
            wire_error,
        },
    );
    let mut summary = receiver
        .run(&stop, file)
        .map_err(|error| Problem::Failed(format!("cannot record: {error}")))?;
    if let Some(tap) = tap {
        // The receiver ends on the stop flag and so does the tap; a tap that failed while running leaves the
        // packets of its file as they were, and the run sees a file with fewer records than the interface counted
        stop.store(true, Ordering::SeqCst);
        summary.wire_packets = tap.join().ok().and_then(Result::ok).map(|tap| tap.packets);
    }
    say(out, &Event::Received(summary));
    Ok(OK)
}

/// The options of `tcp-serve` that take a value.
const TCP_SERVE_OPTIONS: &[&str] = &[
    "bind",
    "out",
    "flow",
    "congestion",
    "progress-interval-ns",
    "max-time-ns",
];

/// The options of `tcp-bulk` that take a value.
const TCP_BULK_OPTIONS: &[&str] = &[
    "to",
    "out",
    "flow",
    "connections",
    "bytes",
    "duration-ns",
    "congestion",
    "chunk-bytes",
    "start-at-ns",
    "info-interval-ns",
    "finish-timeout-ns",
];

/// The options of `tcp-request` that take a value.
const TCP_REQUEST_OPTIONS: &[&str] = &[
    "to",
    "out",
    "flow",
    "connections",
    "request-bytes",
    "response-bytes",
    "duration-ns",
    "interval-ns",
    "congestion",
    "start-at-ns",
    "info-interval-ns",
    "timeout-ns",
];

/// The most connections of one TCP workload. Each is a thread and a socket in the node.
const MAX_TCP_CONNECTIONS: u32 = 1024;

/// The largest request or response, in bytes: what the hello can say and a buffer can hold.
const MAX_MESSAGE_BYTES: u32 = 1 << 30;

/// Sets `stop` on the line `stop` or the end of the input, unless the command was told to ignore its input.
fn stop_on_input(options: &Options, stdin: impl BufRead + Send + 'static, stop: &Arc<AtomicBool>) {
    if options.has("ignore-stdin") {
        return;
    }
    let stop = Arc::clone(stop);
    std::thread::spawn(move || {
        for line in stdin.lines() {
            if line.is_ok_and(|line| line.trim() == "stop") {
                break;
            }
        }
        stop.store(true, Ordering::SeqCst);
    });
}

fn create_file(options: &Options) -> Result<std::fs::File, Problem> {
    let path: String = options.required("out")?;
    std::fs::File::create(&path)
        .map_err(|error| Problem::Failed(format!("cannot create {path}: {error}")))
}

/// A number in `from..=to`, or the usage error that names the option.
fn in_range<T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy>(
    options: &Options,
    name: &str,
    default: Option<T>,
    from: T,
    to: T,
) -> Result<T, String> {
    let given = options.optional::<T>(name)?;
    let Some(value) = given.or(default) else {
        return Err(format!("the option --{name} is required"));
    };
    if value < from || value > to {
        return Err(format!("--{name} is from {from} to {to}, not {value}"));
    }
    Ok(value)
}

/// What the TCP clients have in common, from the options.
fn client_config(options: &Options) -> Result<ClientConfig, String> {
    Ok(ClientConfig {
        target: options.required::<SocketAddr>("to")?,
        flow: options.optional("flow")?.unwrap_or(0),
        connections: in_range(options, "connections", Some(1), 1, MAX_TCP_CONNECTIONS)?,
        congestion: options.optional("congestion")?,
        start_at_ns: options.optional("start-at-ns")?,
        info_interval_ns: options
            .optional("info-interval-ns")?
            .unwrap_or(tcp::DEFAULT_INFO_INTERVAL_NS),
    })
}

/// `tcp-serve --bind <addr> --out <file>`: serves the connections of the TCP clients until it reads `stop` (or its input ends),
/// and writes what it read as records (D-086).
fn tcp_serve_command(
    options: &Options,
    stdin: impl BufRead + Send + 'static,
    out: &mut impl Write,
) -> Result<u8, Problem> {
    let config = ServeConfig {
        bind: options.required::<SocketAddr>("bind")?,
        flow: options.optional("flow")?.unwrap_or(0),
        progress_interval_ns: in_range(
            options,
            "progress-interval-ns",
            Some(tcp::DEFAULT_PROGRESS_INTERVAL_NS),
            1_000_000,
            60_000_000_000,
        )?,
        congestion: options.optional("congestion")?,
        max_time_ns: options.optional("max-time-ns")?,
        hello_patience_ns: tcp::DEFAULT_HELLO_PATIENCE_NS,
    };
    let file = create_file(options)?;
    let server = Server::bind(&config)
        .map_err(|error| Problem::Failed(format!("cannot listen on {}: {error}", config.bind)))?;
    let address = server
        .local_addr()
        .map_err(|error| Problem::Failed(error.to_string()))?;
    let stop = Arc::new(AtomicBool::new(false));
    stop_on_input(options, stdin, &stop);
    say(
        out,
        &Event::Ready {
            flow: config.flow,
            address: address.to_string(),
            wire_error: None,
        },
    );
    let summary = server
        .run(&stop, file)
        .map_err(|error| Problem::Failed(format!("cannot serve: {error}")))?;
    say(out, &Event::Served(summary));
    Ok(OK)
}

/// `tcp-bulk --to <addr> --out <file> --bytes <n>` (or `--duration-ns <ns>`): sends data over one or more connections to a
/// `tcp-serve`, and writes the life of the connections and the samples of `TCP_INFO` as records (D-086).
fn tcp_bulk_command(
    options: &Options,
    stdin: impl BufRead + Send + 'static,
    out: &mut impl Write,
) -> Result<u8, Problem> {
    let amount = match (
        options.optional::<u64>("bytes")?,
        options.optional::<u64>("duration-ns")?,
    ) {
        (Some(0), _) | (_, Some(0)) => {
            return Err(Problem::Usage(
                "--bytes and --duration-ns are greater than 0".to_owned(),
            ));
        }
        (None, None) => {
            return Err(Problem::Usage(
                "give --bytes or --duration-ns (or both: the transfer ends at whichever comes first)"
                    .to_owned(),
            ));
        }
        (bytes, duration_ns) => Amount { bytes, duration_ns },
    };
    let config = BulkConfig {
        common: client_config(options)?,
        amount,
        chunk_bytes: in_range(options, "chunk-bytes", Some(65_536), 1, tcp::MAX_CHUNK)?,
        finish_timeout_ns: options
            .optional("finish-timeout-ns")?
            .unwrap_or(tcp::DEFAULT_FINISH_TIMEOUT_NS),
    };
    let file = create_file(options)?;
    let stop = Arc::new(AtomicBool::new(false));
    stop_on_input(options, stdin, &stop);
    let summary = run_bulk(&config, &stop, file)
        .map_err(|error| Problem::Failed(format!("cannot write the records: {error}")))?;
    say(out, &Event::Tcp(summary));
    Ok(OK)
}

/// `tcp-request --to <addr> --out <file> --request-bytes <n> --response-bytes <n> --duration-ns <ns>`: sends requests to a
/// `tcp-serve` and reads the responses, in a closed loop or (with `--interval-ns`) on a grid of moments, and writes one record
/// for each request (D-086).
fn tcp_request_command(
    options: &Options,
    stdin: impl BufRead + Send + 'static,
    out: &mut impl Write,
) -> Result<u8, Problem> {
    let config = RequestConfig {
        common: client_config(options)?,
        request_bytes: in_range(options, "request-bytes", None, 1, MAX_MESSAGE_BYTES)?,
        response_bytes: in_range(options, "response-bytes", None, 1, MAX_MESSAGE_BYTES)?,
        duration_ns: in_range(options, "duration-ns", None, 1, u64::MAX)?,
        interval_ns: options
            .optional::<u64>("interval-ns")?
            .map(|_| in_range(options, "interval-ns", None, 1, u64::MAX))
            .transpose()?,
        timeout_ns: options
            .optional("timeout-ns")?
            .unwrap_or(tcp::DEFAULT_REQUEST_TIMEOUT_NS),
    };
    let file = create_file(options)?;
    let stop = Arc::new(AtomicBool::new(false));
    stop_on_input(options, stdin, &stop);
    let summary = run_requests(&config, &stop, file)
        .map_err(|error| Problem::Failed(format!("cannot write the records: {error}")))?;
    say(out, &Event::Tcp(summary));
    Ok(OK)
}

/// The tap and the file it writes, or the reason for the `ready` line why there is none.
fn open_tap(config: &ReceiveConfig, path: &str) -> Result<(Tap, std::fs::File), String> {
    let tap = Tap::open(config.bind, config.udp_checksum)
        .map_err(|error| format!("cannot read the frames of the interface: {error}"))?;
    let file =
        std::fs::File::create(path).map_err(|error| format!("cannot create {path}: {error}"))?;
    Ok((tap, file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule(args: &[&str]) -> Result<Schedule, String> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let options = Options::parse(
            &args,
            &[
                "rate",
                "size",
                "pattern",
                "seed",
                "distribution",
                "on-ns",
                "off-ns",
                "burst-packets",
                "gap-ns",
                "pause-ns",
            ],
            &[],
        )?;
        schedule_of(&options)
    }

    #[test]
    fn the_pattern_options_describe_the_four_schedules() {
        let cbr = Stream {
            bits_per_second: 1_000_000,
            packet_size: 125,
        };
        assert_eq!(
            schedule(&["--rate", "1000000", "--size", "125"]).unwrap(),
            Schedule::Cbr(cbr)
        );
        assert_eq!(
            schedule(&[
                "--pattern",
                "poisson",
                "--rate",
                "1000000",
                "--size",
                "125",
                "--seed",
                "9"
            ])
            .unwrap(),
            Schedule::Poisson {
                stream: cbr,
                seed: 9
            }
        );
        assert_eq!(
            schedule(&[
                "--pattern",
                "onoff",
                "--rate",
                "1000000",
                "--size",
                "125",
                "--on-ns",
                "5",
                "--off-ns",
                "7",
                "--distribution",
                "exponential",
                "--seed",
                "3",
            ])
            .unwrap(),
            Schedule::OnOff {
                stream: cbr,
                on_ns: 5,
                off_ns: 7,
                distribution: Distribution::Exponential,
                seed: 3
            }
        );
        assert_eq!(
            schedule(&[
                "--pattern",
                "bursty",
                "--size",
                "125",
                "--burst-packets",
                "4",
                "--gap-ns",
                "100",
                "--pause-ns",
                "900",
            ])
            .unwrap(),
            Schedule::Bursty {
                packet_size: 125,
                packets: 4,
                gap_ns: 100,
                pause_ns: 900,
                distribution: Distribution::Fixed,
                seed: 0
            }
        );
    }

    #[test]
    fn a_pattern_that_lacks_what_it_needs_or_is_unknown_is_refused_with_the_name() {
        for (args, expected) in [
            (&["--size", "125"][..], "--rate is required"),
            (
                &[
                    "--pattern",
                    "onoff",
                    "--rate",
                    "1",
                    "--size",
                    "125",
                    "--on-ns",
                    "5",
                ][..],
                "--off-ns is required",
            ),
            (
                &[
                    "--pattern",
                    "bursty",
                    "--size",
                    "125",
                    "--gap-ns",
                    "1",
                    "--pause-ns",
                    "1",
                ][..],
                "--burst-packets is required",
            ),
            (
                &["--pattern", "wave", "--rate", "1", "--size", "125"][..],
                "--pattern is cbr, poisson, onoff, bursty or trace, not 'wave'",
            ),
            (
                &["--distribution", "normal", "--rate", "1", "--size", "125"][..],
                "--distribution is fixed or exponential, not 'normal'",
            ),
        ] {
            let error = schedule(args).unwrap_err();
            assert!(error.contains(expected), "{args:?}: {error}");
        }
    }

    fn call(args: &[&str]) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(args.iter().copied(), std::io::empty(), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn help_and_version_work() {
        let (code, out, _) = call(&["agent", "--help"]);
        assert_eq!(code, OK);
        assert!(out.contains("send --to") && out.contains("recv --bind"));
        let (code, out, _) = call(&["agent", "--version"]);
        assert_eq!(code, OK);
        assert!(out.starts_with("netscenario-agent "));
    }

    #[test]
    fn a_wrong_command_line_is_a_usage_error_that_says_what_is_wrong() {
        for (args, expected) in [
            (&["agent"][..], "no command given"),
            (&["agent", "frobnicate"][..], "unknown command 'frobnicate'"),
            (&["agent", "send"][..], "--to is required"),
            (&["agent", "send", "--to"][..], "--to needs a value"),
            (
                &[
                    "agent",
                    "send",
                    "--to",
                    "127.0.0.1:9",
                    "--rate",
                    "fast",
                    "--size",
                    "1000",
                    "--duration-ns",
                    "1",
                ][..],
                "'fast' is not a valid value for --rate",
            ),
            (
                &[
                    "agent",
                    "send",
                    "--to",
                    "127.0.0.1:9",
                    "--to",
                    "127.0.0.1:9",
                ][..],
                "--to is given twice",
            ),
            (
                &[
                    "agent",
                    "recv",
                    "--bind",
                    "127.0.0.1:0",
                    "--out",
                    "x",
                    "--udp-checksum",
                    "maybe",
                ][..],
                "'maybe' is not a valid value for --udp-checksum",
            ),
            (
                &["agent", "recv", "--verbose"][..],
                "unknown option '--verbose'",
            ),
            (
                &["agent", "recv", "--bind", "127.0.0.1:0"][..],
                "--out is required",
            ),
        ] {
            let (code, out, err) = call(args);
            assert_eq!(code, USAGE, "{args:?}");
            assert_eq!(out, "");
            assert!(err.contains(expected), "{args:?}: {err}");
        }
    }

    /// The barriers are read in the order of the packets, and the number of a barrier is its own.
    #[test]
    fn barriers_are_read_in_the_order_of_the_packets_and_nonsense_is_refused() {
        let barriers = parse_barriers("40:3,12:0,40:1").unwrap();
        assert_eq!(
            barriers
                .iter()
                .map(|b| (b.packet, b.id))
                .collect::<Vec<_>>(),
            [(12, 0), (40, 1), (40, 3)]
        );
        assert_eq!(parse_barriers("").unwrap(), Vec::new());
        for bad in ["12", "12:", ":3", "a:1", "12:x", "12:1:2"] {
            let error = parse_barriers(bad).unwrap_err();
            assert!(error.contains("is not a barrier"), "{bad}: {error}");
        }
    }

    #[test]
    fn a_sender_that_cannot_run_fails_with_the_reason() {
        let (code, out, err) = call(&[
            "agent",
            "send",
            "--to",
            "127.0.0.1:9",
            "--rate",
            "0",
            "--size",
            "1000",
            "--duration-ns",
            "1000",
        ]);
        assert_eq!(code, FAILED);
        assert_eq!(out, "");
        assert!(err.contains("more than zero"), "{err}");
    }
}
