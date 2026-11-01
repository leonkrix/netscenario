//! The command line of `netscenario-agent`.
//!
//! ```text
//! netscenario-agent send --to <addr> --rate <bits/s> --size <bytes> --duration-ns <ns>
//!                        [--flow <n>] [--start-at-ns <monotonic ns>] [--udp-checksum <on|off>]
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

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::events::Event;
use crate::receiver::{ReceiveConfig, Receiver};
use crate::schedule::Stream;
use crate::sender::{SendConfig, run as send};
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
            ],
            &[],
        )
        .map_err(Problem::Usage)
        .and_then(|options| send_command(&options, out)),
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

fn send_command(options: &Options, out: &mut impl Write) -> Result<u8, Problem> {
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
    };
    let summary = send(&config).map_err(|error| Problem::Failed(error.to_string()))?;
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
