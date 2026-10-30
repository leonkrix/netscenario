//! What the caller and the supervisor say to each other: one JSON object per line, a request and then its
//! response.

use netscenario_core::Direction;
use netscenario_model::Impairments;
use serde::{Deserialize, Serialize};

/// A direction on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    Apply {
        link: u32,
        direction: Way,
        impairments: Impairments,
        seed: Option<u64>,
    },
    StartProgram {
        node: u32,
        argv: Vec<String>,
        /// The most address space the program may use, in bytes.
        #[serde(default)]
        memory_limit: Option<u64>,
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
}

/// The answer to a request, and the first line that the supervisor sends when it is ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Response {
    /// Done. `id` is the handle of what was created.
    Ok { id: Option<u32> },
    /// Refused or failed. The text says what and why.
    Error { message: String },
    /// What a program has done since the last poll.
    Program {
        lines: Vec<String>,
        exit: Option<Exit>,
        stderr: String,
        truncated: bool,
    },
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
                impairments: delay,
                seed: Some(7),
            },
            Request::StartProgram {
                node: 1,
                argv: vec!["ping".to_owned(), "-c1".to_owned()],
                memory_limit: Some(1 << 30),
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
            },
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
                memory_limit: None
            }
        );
    }

    #[test]
    fn an_unknown_request_is_refused() {
        assert!(serde_json::from_str::<Request>(r#"{"op":"format_disk"}"#).is_err());
    }
}
