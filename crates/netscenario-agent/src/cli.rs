//! The command line of `netscenario-agent`.
//!
//! ```text
//! netscenario-agent send --to <addr> --rate <bits/s> --size <bytes> --duration-ns <ns>
//!                        [--flow <n>] [--start-at-ns <monotonic ns>] [--udp-checksum <on|off>]
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

use crate::events::Event;
use crate::receiver::{ReceiveConfig, Receiver};
use crate::schedule::Stream;
use crate::sender::{Barrier, Barriers, SendConfig, run as send, run_with as send_with};
use crate::tap::Tap;

/// The exit code for success.
pub const OK: u8 = 0;
/// The exit code for a failure while running.
pub const FAILED: u8 = 1;
/// The exit code for a wrong command line.
pub const USAGE: u8 = 2;

const USAGE_TEXT: &str = "\
Usage:
  netscenario-agent send --to <addr> --rate <bits/s> --size <bytes> --duration-ns <ns>
                         [--flow <n>] [--start-at-ns <monotonic ns>] [--udp-checksum <on|off>]
                         [--barriers <packet>:<id>,...]
  netscenario-agent recv --bind <addr> --out <file> [--flow <n>] [--max-time-ns <ns>] [--ignore-stdin]
                         [--udp-checksum <on|off>] [--wire-out <file>]

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
        "send" => Options::parse(
            rest,
            &[
                "to",
                "rate",
                "size",
                "duration-ns",
                "flow",
                "start-at-ns",
                "udp-checksum",
                "barriers",
            ],
            &[],
        )
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
            ],
            &["ignore-stdin"],
        )
        .map_err(Problem::Usage)
        .and_then(|options| recv_command(&options, stdin, out)),
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

/// The answers of the run to the barriers of a sender: the lines `go <id>` on its standard input (D-038).
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

#[derive(Default)]
struct GateState {
    released: HashSet<u32>,
    /// The input has ended: nobody will answer any more, so nothing is waited for (a sender whose run died does
    /// not wait for ever).
    closed: bool,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GateState::default()),
            changed: Condvar::new(),
        })
    }

    /// Reads the answers from `input` until it ends.
    fn listen(self: &Arc<Self>, input: impl BufRead + Send + 'static) {
        let gate = Arc::clone(self);
        std::thread::spawn(move || {
            for line in input.lines() {
                let Ok(line) = line else { break };
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
            if state.released.contains(&id) || state.closed {
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
        stream: Stream {
            bits_per_second: options.required("rate")?,
            packet_size: options.required("size")?,
        },
        duration_ns: options.required("duration-ns")?,
        start_at_ns: options.optional("start-at-ns")?,
        udp_checksum: options.optional("udp-checksum")?.unwrap_or_default(),
        barriers,
    };
    let summary = if config.barriers.is_empty() {
        send(&config)
    } else {
        let gate = Gate::new();
        gate.listen(stdin);
        let mut port = StandardBarriers {
            flow: config.flow,
            out: &mut *out,
            gate,
        };
        send_with(&config, &mut port)
    }
    .map_err(|error| Problem::Failed(error.to_string()))?;
    say(out, &Event::Sent(summary));
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
