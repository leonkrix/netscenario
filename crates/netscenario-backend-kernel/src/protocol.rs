//! What the caller and the supervisor say to each other: one JSON object per line, a request and then its
//! response.

use netscenario_core::Direction;
use netscenario_model::Impairments;
use serde::{Deserialize, Serialize};

/// A direction on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Way {
    Forward,
    Reverse,
}

impl From<Direction> for Way {
    fn from(direction: Direction) -> Self {
        match direction {
            Direction::Forward => Self::Forward,
            Direction::Reverse => Self::Reverse,
        }
    }
}

/// One way out of a route on the wire: the link, the end of it that the node is at, and the weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WireHop {
    pub(crate) link: u32,
    pub(crate) end: usize,
    pub(crate) weight: u32,
}

/// What the caller asks the supervisor to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Request {
    CreateNode {
        name: String,
    },
    CreateLink {
        name: String,
        between: [u32; 2],
    },
    /// Says which addresses the network has, before the first node is made (D-081).
    SetFamilies {
        v4: bool,
        v6: bool,
    },
    /// Waits until the network is quiet, before the first queue is built (D-081).
    FinishNetwork,
    /// Gives a node an address on its loopback (D-078): of the family the text is.
    SetNodeAddress {
        node: u32,
        address: String,
    },
    /// Sets the MTU of both interfaces of a link (D-081).
    SetMtu {
        link: u32,
        mtu: u32,
    },
    /// Makes a node forward packets or not (D-078).
    SetForwarding {
        node: u32,
        on: bool,
    },
    /// Adds a route to a node (D-078): to an address (`None` is every address), out of one or several links.
    AddRoute {
        node: u32,
        /// The route is an IPv6 route (the destination, if any, and the next hops are IPv6 addresses).
        #[serde(default)]
        v6: bool,
        destination: Option<String>,
        metric: u32,
        next_hops: Vec<WireHop>,
    },
    Apply {
        link: u32,
        direction: Way,
        impairments: Impairments,
        seed: Option<u64>,
    },
    /// Every state that one direction will have in the run, before it is built (D-059).
    Plan {
        link: u32,
        direction: Way,
        states: Vec<Impairments>,
    },
    /// The direction has an outage in the run, so that its tree gets the blackout queue (D-064). After `Plan`.
    PlanOutage {
        link: u32,
        direction: Way,
    },
    /// Begins or ends an outage of a direction (D-064).
    SetOutage {
        link: u32,
        direction: Way,
        on: bool,
    },
    /// Writes a state into the queues of one direction while the run is going (D-048, D-059).
    Change {
        link: u32,
        direction: Way,
        impairments: Impairments,
    },
    /// How many packets entered the queue of one direction, which draws its random loss.
    /// Takes both interfaces of a link down or brings them up (D-064).
    SetLink {
        link: u32,
        up: bool,
    },
    QueuePackets {
        link: u32,
        direction: Way,
    },
    /// What the node at the receiving end of one direction has counted: the frames of the interface and the
    /// datagrams dropped for a full buffer.
    /// How many packets are in the queues of a direction now (D-067).
    QueueBacklog {
        link: u32,
        direction: Way,
    },
    NodeCounts {
        link: u32,
        direction: Way,
    },
    StartProgram {
        node: u32,
        argv: Vec<String>,
        /// The most address space the program may use, in bytes.
        #[serde(default)]
        memory_limit: Option<u64>,
        /// Variables added to the environment, the directory, a process group of its own, and the files that get its
        /// output (D-072). An older caller leaves them out.
        #[serde(default)]
        env: Vec<(String, String)>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        own_group: bool,
        #[serde(default)]
        stdout_file: Option<String>,
        #[serde(default)]
        stderr_file: Option<String>,
    },
    /// Sends a signal (its number) to the process group of a program that has one.
    Signal {
        program: u32,
        signal: i32,
    },
    /// Whether a socket is bound (`udp`) or listens (`tcp`) on a port in a node (D-041).
    PortBound {
        node: u32,
        udp: bool,
        port: u16,
    },
    /// Waits up to `wait_ms` for a line or the end of a program and returns what is new.
    Poll {
        program: u32,
        wait_ms: u64,
    },
    /// Writes a line to the standard input of a program.
    SendLine {
        program: u32,
        line: String,
    },
    Teardown,
    /// What the backend says about the run besides its result (D-063).
    Notes,
}

/// The answer to a request, and the first line that the supervisor sends when it is ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Response {
    /// Done. `id` is the handle of what was created.
    Ok { id: Option<u32> },
    /// Refused or failed. The text says what and why.
    Error { message: String },
    /// The answer to `QueuePackets`.
    Count { packets: u64 },
    /// The answer to `NodeCounts`.
    NodeCounts {
        frames_received: u64,
        receive_buffer_drops: u64,
    },
    /// What a program has done since the last poll.
    /// The answer to `Notes`.
    Notes { lines: Vec<String> },
    Program {
        lines: Vec<String>,
        exit: Option<Exit>,
        stderr: String,
        truncated: bool,
        #[serde(default)]
        output: OutputCounts,
    },
    /// The answer to `PortBound`.
    Bool { value: bool },
}

/// What was read from a program and what was dropped (`netscenario_core::OutputStats` on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct OutputCounts {
    pub(crate) stdout_bytes: u64,
    pub(crate) stdout_dropped_bytes: u64,
    pub(crate) stderr_bytes: u64,
    pub(crate) stderr_dropped_bytes: u64,
    pub(crate) long_lines: u64,
}

/// How a program ended: its exit code, or the signal that ended it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Exit {
    Code(i32),
    Signal(i32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_survive_the_wire() {
        let delay =
            Impairments::new().with_delay(netscenario_model::Duration::from_millis(20).unwrap());
        for request in [
            Request::CreateNode {
                name: "client".to_owned(),
            },
            Request::CreateLink {
                name: "access".to_owned(),
                between: [0, 1],
            },
            Request::Apply {
                link: 0,
                direction: Way::Reverse,
                impairments: delay.clone(),
                seed: Some(7),
            },
            Request::Plan {
                link: 0,
                direction: Way::Forward,
                states: vec![delay.clone(), delay.clone()],
            },
            Request::PlanOutage {
                link: 0,
                direction: Way::Forward,
            },
            Request::SetOutage {
                link: 0,
                direction: Way::Reverse,
                on: true,
            },
            Request::Change {
                link: 0,
                direction: Way::Forward,
                impairments: delay,
            },
            Request::StartProgram {
                node: 1,
                argv: vec!["ping".to_owned(), "-c1".to_owned()],
                memory_limit: Some(1 << 30),
                env: vec![("A".to_owned(), "1".to_owned())],
                cwd: Some("/tmp".to_owned()),
                own_group: true,
                stdout_file: Some("/tmp/out.log".to_owned()),
                stderr_file: None,
            },
            Request::Signal {
                program: 2,
                signal: 15,
            },
            Request::PortBound {
                node: 1,
                udp: true,
                port: 4433,
            },
            Request::Poll {
                program: 0,
                wait_ms: 250,
            },
            Request::SendLine {
                program: 0,
                line: "stop".to_owned(),
            },
            Request::Teardown,
            Request::SetLink { link: 0, up: false },
            Request::QueueBacklog {
                link: 0,
                direction: Way::Reverse,
            },
            Request::Notes,
        ] {
            let text = serde_json::to_string(&request).unwrap();
            assert!(!text.contains('\n'));
            assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request);
        }
    }

    #[test]
    fn responses_survive_the_wire() {
        for response in [
            Response::Ok { id: Some(3) },
            Response::Ok { id: None },
            Response::Program {
                lines: vec!["{\"event\":\"ready\"}".to_owned()],
                exit: Some(Exit::Signal(9)),
                stderr: "boom\n".to_owned(),
                truncated: false,
                output: OutputCounts {
                    stdout_bytes: 5,
                    stdout_dropped_bytes: 1,
                    stderr_bytes: 5,
                    stderr_dropped_bytes: 0,
                    long_lines: 2,
                },
            },
            Response::Bool { value: true },
            Response::Error {
                message: "no \"good\"\n".to_owned(),
            },
        ] {
            let text = serde_json::to_string(&response).unwrap();
            assert_eq!(serde_json::from_str::<Response>(&text).unwrap(), response);
        }
    }

    /// A caller that does not know about limits (an older one) still starts programs.
    #[test]
    fn a_start_without_a_limit_is_unlimited() {
        let request: Request =
            serde_json::from_str(r#"{"op":"start_program","node":0,"argv":["true"]}"#).unwrap();
        assert_eq!(
            request,
            Request::StartProgram {
                node: 0,
                argv: vec!["true".to_owned()],
                memory_limit: None,
                env: Vec::new(),
                cwd: None,
                own_group: false,
                stdout_file: None,
                stderr_file: None,
            }
        );
    }

    #[test]
    fn an_unknown_request_is_refused() {
        assert!(serde_json::from_str::<Request>(r#"{"op":"format_disk"}"#).is_err());
    }
}
