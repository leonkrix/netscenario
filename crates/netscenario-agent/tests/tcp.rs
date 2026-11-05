//! The TCP agents as real programs over the loopback interface of the machine, without namespaces: a server, a bulk client and
//! a request client, their events on standard output and their files of records (D-086).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use netscenario_agent::events::{Event, ServeSummary, TcpSummary};
use netscenario_agent::streams::{self, Kind};

const AGENT: &str = env!("CARGO_BIN_EXE_netscenario-agent");

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "netscenario-agent-tcp-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A server that is listening: its address, and the means to stop it.
struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
    address: String,
}

fn serve(dir: &Path, extra: &[&str]) -> Server {
    let mut child = Command::new(AGENT)
        .args(["tcp-serve", "--bind", "127.0.0.1:0", "--flow", "6", "--out"])
        .arg(dir.join("server.streams"))
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let ready = lines.next().expect("the server said nothing").unwrap();
    let Event::Ready { flow, address, .. } = serde_json::from_str(&ready).unwrap() else {
        panic!("not a ready line: {ready}");
    };
    assert_eq!(flow, 6);
    Server {
        child,
        stdin,
        lines,
        address,
    }
}

impl Server {
    fn stop(mut self) -> ServeSummary {
        writeln!(self.stdin, "stop").unwrap();
        self.stdin.flush().unwrap();
        let line = self.lines.next().expect("no summary").unwrap();
        assert!(self.child.wait().unwrap().success());
        match serde_json::from_str(&line).unwrap() {
            Event::Served(summary) => summary,
            other => panic!("not a summary: {other:?}"),
        }
    }
}

/// Runs a client to its end and returns its summary, or the exit code and the message of a client that failed to start.
fn client(args: &[&str], dir: &Path, file: &str) -> Result<TcpSummary, (i32, String)> {
    let output = Command::new(AGENT)
        .args(args)
        .arg("--out")
        .arg(dir.join(file))
        .arg("--ignore-stdin")
        .output()
        .unwrap();
    if !output.status.success() {
        return Err((
            output.status.code().unwrap(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let text = String::from_utf8(output.stdout).unwrap();
    match serde_json::from_str(text.lines().last().unwrap()).unwrap() {
        Event::Tcp(summary) => Ok(summary),
        other => panic!("not a summary: {other:?}"),
    }
}

#[test]
fn a_bulk_client_and_a_request_client_work_against_a_server_as_programs() {
    let dir = work_dir("programs");
    let server = serve(&dir, &["--progress-interval-ns", "5000000"]);
    let bulk = client(
        &[
            "tcp-bulk",
            "--to",
            &server.address,
            "--flow",
            "6",
            "--bytes",
            "2000000",
            "--connections",
            "2",
            "--congestion",
            "reno",
        ],
        &dir,
        "bulk.streams",
    )
    .unwrap();
    assert_eq!(
        (bulk.connections, bulk.failed),
        (2, 0),
        "{:?}",
        bulk.first_error
    );
    assert_eq!(bulk.bytes_sent, 4_000_000);
    assert_eq!(bulk.congestion.as_deref(), Some("reno"));
    let requests = client(
        &[
            "tcp-request",
            "--to",
            &server.address,
            "--flow",
            "6",
            "--request-bytes",
            "200",
            "--response-bytes",
            "3000",
            "--duration-ns",
            "300000000",
            "--interval-ns",
            "10000000",
        ],
        &dir,
        "requests.streams",
    )
    .unwrap();
    assert_eq!(requests.failed, 0, "{:?}", requests.first_error);
    assert_eq!(requests.requests, 30);
    let summary = server.stop();
    assert_eq!(summary.bytes_received, 4_000_000 + 30 * 200);
    assert_eq!((summary.requests, summary.refused), (30, 0));
    // The three files read back, each with its flow in the header and the records of its kinds
    let read = |name: &str| streams::read(std::fs::File::open(dir.join(name)).unwrap()).unwrap();
    let server_file = read("server.streams");
    assert_eq!(server_file.flow, 6);
    assert!(server_file.records.iter().all(|r| r.kind == Kind::Progress));
    let bulk_file = read("bulk.streams");
    assert_eq!(
        bulk_file
            .records
            .iter()
            .filter(|r| r.kind == Kind::Connection)
            .count(),
        2
    );
    let request_file = read("requests.streams");
    assert_eq!(
        request_file
            .records
            .iter()
            .filter(|r| r.kind == Kind::Request)
            .count(),
        30
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Canary: a wrong command line is refused with the exit code 2 and says which option, before anything is opened.
#[test]
fn a_wrong_command_line_is_refused_and_names_the_option() {
    let dir = work_dir("usage");
    for (args, said) in [
        (
            vec!["tcp-bulk", "--to", "127.0.0.1:9"],
            "--bytes or --duration-ns",
        ),
        (
            vec![
                "tcp-bulk",
                "--to",
                "127.0.0.1:9",
                "--bytes",
                "5",
                "--duration-ns",
                "5",
            ],
            "alternatives",
        ),
        (
            vec![
                "tcp-bulk",
                "--to",
                "127.0.0.1:9",
                "--bytes",
                "5",
                "--connections",
                "0",
            ],
            "--connections is from 1",
        ),
        (
            vec![
                "tcp-bulk",
                "--to",
                "127.0.0.1:9",
                "--bytes",
                "5",
                "--chunk-bytes",
                "99999999",
            ],
            "--chunk-bytes",
        ),
        (
            vec![
                "tcp-request",
                "--to",
                "127.0.0.1:9",
                "--request-bytes",
                "0",
                "--response-bytes",
                "1",
                "--duration-ns",
                "5",
            ],
            "--request-bytes is from 1",
        ),
        (
            vec![
                "tcp-request",
                "--to",
                "127.0.0.1:9",
                "--request-bytes",
                "1",
                "--response-bytes",
                "1",
            ],
            "--duration-ns is required",
        ),
        (
            vec![
                "tcp-request",
                "--to",
                "127.0.0.1:9",
                "--request-bytes",
                "1",
                "--response-bytes",
                "1",
                "--duration-ns",
                "5",
                "--interval-ns",
                "0",
            ],
            "--interval-ns is from 1",
        ),
    ] {
        let (code, message) = client(&args, &dir, "never.streams").unwrap_err();
        assert_eq!(code, 2, "{args:?}: {message}");
        assert!(message.contains(said), "{args:?}: {message}");
    }
    assert!(
        !dir.join("never.streams").exists(),
        "a refused command line left a file"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A congestion control that the machine lacks is a failed connection that names it, and the program says so in its summary
/// and not in an exit code that would look like a crash.
#[test]
fn a_congestion_control_that_is_not_there_is_a_failure_in_the_summary() {
    let dir = work_dir("cc");
    let server = serve(&dir, &[]);
    let summary = client(
        &[
            "tcp-bulk",
            "--to",
            &server.address,
            "--bytes",
            "1000",
            "--congestion",
            "no-such-cc",
        ],
        &dir,
        "cc.streams",
    )
    .unwrap();
    assert_eq!((summary.connections, summary.failed), (1, 1));
    assert!(summary.first_error.unwrap().contains("no-such-cc"));
    let _ = server.stop();
    let _ = std::fs::remove_dir_all(dir);
}
