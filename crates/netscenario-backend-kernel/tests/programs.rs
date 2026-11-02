//! What the backend does with the programs of a node: their output, their end, their input and their limits.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::{Duration, Instant};

use netscenario_backend_kernel::KernelBackend;
use netscenario_core::{
    Backend, PortKind, Program, ProgramExit, ProgramHandle, ProgramSignal, ProgramState, provision,
};
use support::{backend, namespaces_available, scenario};

fn sh(script: &str) -> Program {
    Program::new(vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()])
}

/// A run with two nodes and the first node, for a program.
fn run_with_node() -> (KernelBackend, netscenario_core::NodeHandle) {
    let mut backend = backend(|_| {});
    let built = provision(&mut backend, &scenario("", "")).unwrap();
    let node = built.nodes[0].1;
    (backend, node)
}

/// Polls until the program has ended and returns everything it wrote and how it ended.
fn until_exit(
    backend: &mut KernelBackend,
    program: ProgramHandle,
) -> (Vec<String>, ProgramExit, ProgramState) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut lines = Vec::new();
    loop {
        let state = backend
            .poll_program(program, Duration::from_millis(200))
            .unwrap();
        lines.extend(state.lines.clone());
        if let Some(exit) = state.exit {
            return (lines, exit, state);
        }
        assert!(Instant::now() < deadline, "the program did not end");
    }
}

#[test]
fn the_output_and_the_exit_code_of_a_program_come_back() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, node) = run_with_node();
    let program = backend
        .start_program(node, &sh("echo one; echo two; echo oops >&2; exit 3"))
        .unwrap();
    let (lines, exit, state) = until_exit(&mut backend, program);
    assert_eq!(lines, ["one", "two"]);
    assert_eq!(exit, ProgramExit::Code(3));
    assert!(!exit.success());
    assert_eq!(state.stderr.trim(), "oops");
    assert!(!state.truncated);
    backend.teardown().unwrap();
}

/// The end of a program is reported only after all of its output: the last lines are not lost.
#[test]
fn a_program_that_ends_at_once_does_not_lose_its_last_lines() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, node) = run_with_node();
    for round in 0..20 {
        let program = backend
            .start_program(node, &sh("echo a; echo b; echo c"))
            .unwrap();
        let (lines, exit, _) = until_exit(&mut backend, program);
        assert_eq!(lines, ["a", "b", "c"], "round {round}");
        assert!(exit.success());
    }
    backend.teardown().unwrap();
}

#[test]
fn a_program_killed_by_a_signal_says_which() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, node) = run_with_node();
    let program = backend.start_program(node, &sh("kill -9 $$")).unwrap();
    let (_, exit, _) = until_exit(&mut backend, program);
    assert_eq!(exit, ProgramExit::Signal(9));
    assert_eq!(exit.to_string(), "killed by signal 9");
    backend.teardown().unwrap();
}

#[test]
fn a_line_written_to_a_program_reaches_its_standard_input() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, node) = run_with_node();
    let program = backend
        .start_program(node, &sh("read word; echo got $word"))
        .unwrap();
    backend.send_line(program, "hello").unwrap();
    let (lines, exit, _) = until_exit(&mut backend, program);
    assert_eq!(lines, ["got hello"]);
    assert!(exit.success());
    // Writing to a program that has ended is not an error
    backend.send_line(program, "late").unwrap();
    backend.teardown().unwrap();
}

#[test]
fn waiting_for_a_quiet_program_returns_after_the_time_with_nothing() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, node) = run_with_node();
    let program = backend.start_program(node, &sh("sleep 30")).unwrap();
    let began = Instant::now();
    let state = backend
        .poll_program(program, Duration::from_millis(300))
        .unwrap();
    assert_eq!(state, ProgramState::default());
    assert!(began.elapsed() >= Duration::from_millis(290));
    assert!(began.elapsed() < Duration::from_secs(5));
    backend.teardown().unwrap();
}

#[test]
fn asking_about_a_program_that_does_not_exist_is_an_error() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, _) = run_with_node();
    let error = backend
        .poll_program(ProgramHandle(9), Duration::from_millis(1))
        .unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");
    let error = backend.send_line(ProgramHandle(9), "x").unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");
    backend.teardown().unwrap();
}

/// More output than the supervisor keeps is dropped, and the caller is told.
#[test]
fn a_program_that_writes_too_much_is_cut_and_marked() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, node) = run_with_node();
    let program = backend
        .start_program(node, &sh("yes line | head -n 10500"))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut count, mut truncated) = (0, false);
    loop {
        let state = backend
            .poll_program(program, Duration::from_millis(200))
            .unwrap();
        count += state.lines.len();
        truncated |= state.truncated;
        if state.exit.is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    assert_eq!(count, 10_000);
    assert!(truncated);
    backend.teardown().unwrap();
}

/// SAF-02: the limit is set on the program, and it holds.
#[test]
fn the_memory_limit_is_set_on_the_program() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, node) = run_with_node();
    let limit = 512 * 1024 * 1024;
    let limited = backend
        .start_program(
            node,
            &sh("grep 'Max address space' /proc/self/limits").with_memory_limit(limit),
        )
        .unwrap();
    let (lines, _, _) = until_exit(&mut backend, limited);
    assert!(
        lines.iter().any(|l| l.contains("536870912")),
        "the limit is not in /proc/self/limits: {lines:?}"
    );
    // Canary: without a limit the same program says unlimited
    let free = backend
        .start_program(node, &sh("grep 'Max address space' /proc/self/limits"))
        .unwrap();
    let (lines, _, _) = until_exit(&mut backend, free);
    assert!(lines.iter().any(|l| l.contains("unlimited")), "{lines:?}");
    backend.teardown().unwrap();
}

#[test]
fn a_program_that_needs_more_than_its_limit_does_not_get_it() {
    if !namespaces_available() {
        return;
    }
    let (mut backend, node) = run_with_node();
    // The shell keeps the 150 MB it reads in memory
    let script = "x=$(head -c 150000000 /dev/zero | tr '\\0' a); echo ${#x}";
    let free = backend.start_program(node, &sh(script)).unwrap();
    let (lines, exit, _) = until_exit(&mut backend, free);
    assert_eq!(lines, ["150000000"], "without a limit it works ({exit})");
    let limited = backend
        .start_program(node, &sh(script).with_memory_limit(64 * 1024 * 1024))
        .unwrap();
    let (lines, _, _) = until_exit(&mut backend, limited);
    assert!(
        !lines.iter().any(|l| l == "150000000"),
        "the limit of 64 MiB did not hold: {lines:?}"
    );
    backend.teardown().unwrap();
}

/// What a program of your own is started with (D-072): the variables, the directory, the files for its output, and a process
/// group of its own that a signal reaches with its children.
#[test]
fn a_program_gets_its_variables_its_directory_and_files_and_a_group_that_a_signal_reaches() {
    if !namespaces_available() {
        return;
    }
    let directory = support::work_dir("program-spec");
    let (mut backend, node) = run_with_node();
    let (out, err) = (directory.join("out.log"), directory.join("err.log"));
    let program = backend
        .start_program(
            node,
            &sh("echo \"$ONE $(pwd)\"; echo trouble >&2; sleep 61.9 & echo $$ $(ps -o pgid= -p $$); wait")
                .with_env(vec![("ONE".to_owned(), "1".to_owned())])
                .with_working_dir(directory.clone())
                .in_own_group()
                .with_log_files(out.clone(), err.clone()),
        )
        .unwrap();
    let mut lines = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while lines.len() < 2 {
        lines.extend(
            backend
                .poll_program(program, Duration::from_millis(100))
                .unwrap()
                .lines,
        );
        assert!(Instant::now() < deadline, "{lines:?}");
    }
    assert_eq!(lines[0], format!("1 {}", directory.display()));
    // The shell is the leader of its group: its pid is its group
    let ids: Vec<&str> = lines[1].split_whitespace().collect();
    assert_eq!(ids[0], ids[1], "{lines:?}");
    // The signal goes to the group, so the child `sleep` ends with the shell
    backend
        .signal_program(program, ProgramSignal::Term)
        .unwrap();
    let (_, exit, state) = until_exit(&mut backend, program);
    assert_eq!(exit, ProgramExit::Signal(15));
    assert_eq!(state.output.stderr_bytes, 8);
    assert_eq!(std::fs::read_to_string(&err).unwrap(), "trouble\n");
    assert!(std::fs::read_to_string(&out).unwrap().starts_with("1 "));
    // A signal for a program that has ended is not an error
    backend
        .signal_program(program, ProgramSignal::Kill)
        .unwrap();
    // Canary: a program that was not started in a group of its own cannot be signalled this way
    let plain = backend.start_program(node, &sh("sleep 61.8")).unwrap();
    assert!(backend.signal_program(plain, ProgramSignal::Term).is_err());
    backend.teardown().unwrap();
    let _ = std::fs::remove_dir_all(directory);
}

/// The sockets of a node are read from outside: a bound UDP socket is found, another port is not, and a node that has
/// none says so.
#[test]
fn a_port_that_a_program_has_bound_is_found_from_outside() {
    if !namespaces_available()
        || std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
    {
        return;
    }
    let (mut backend, node) = run_with_node();
    assert!(!backend.port_bound(node, PortKind::Udp, 4455).unwrap());
    let binder = Program::new(vec![
        "python3".to_owned(),
        "-c".to_owned(),
        "import socket,time; s=socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(('0.0.0.0',4455)); print('bound', flush=True); time.sleep(30)".to_owned(),
    ]);
    let program = backend.start_program(node, &binder).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let state = backend
            .poll_program(program, Duration::from_millis(100))
            .unwrap();
        if state.lines.iter().any(|line| line == "bound") {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    assert!(backend.port_bound(node, PortKind::Udp, 4455).unwrap());
    // Another port, and the same port as TCP, are not bound
    assert!(!backend.port_bound(node, PortKind::Udp, 4456).unwrap());
    assert!(!backend.port_bound(node, PortKind::Tcp, 4455).unwrap());
    // The other node does not have it
    let other = netscenario_core::NodeHandle(node.0 + 1);
    assert!(!backend.port_bound(other, PortKind::Udp, 4455).unwrap());
    backend.teardown().unwrap();
}
