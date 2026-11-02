//! The programs of your own in a real run (WRK-06, WRK-10, PRB-08; D-072): started in their node with the environment,
//! the directory and the placeholders of the file, waited for until they are ready, watched, stopped with the signal of
//! `shutdown` and `KILL`, and reported with their output and their metrics. Nothing is left behind, whatever they do.
//!
//! (What the backend does with a program of a node, below the run, is in `programs.rs`.)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use netscenario_core::{CancelToken, Error, RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

/// A script in the directory of the scenario (the programs run there), executable.
fn script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn scenario(duration: &str, link: &str, workloads: &str) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: programs-test\nseed: 5\nduration: {duration}\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n{link}workloads:\n{workloads}"
        ),
        "programs-test.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn options(dir: &Path) -> RunOptions {
    let mut options = RunOptions::new(agent_exe().unwrap(), dir);
    options.settle = Some(Duration::from_millis(300));
    options.working_dir = Some(dir.canonicalize().unwrap());
    options
}

fn go(dir: &Path, scenario: &Scenario) -> Result<RunResult, Error> {
    let mut backend = backend(|_| {});
    let result = run(&mut backend, scenario, &options(dir));
    assert_nothing_left(backend.run_id(), "after the run");
    result
}

fn ready() -> bool {
    namespaces_available() && agent_exe().is_some()
}

fn message(error: &Error) -> String {
    error.to_string()
}

/// One program through its whole life: the placeholder and the variables of the file and of the tool, the directory,
/// the order of the lines, ready by a line of standard output, the metrics with their times, a line that is not valid,
/// the files of the output, and the stop with `TERM`.
#[test]
fn a_program_is_started_with_its_environment_waited_for_read_and_stopped() {
    if !ready() {
        return;
    }
    let dir = work_dir("programs-life");
    script(
        &dir,
        "app.sh",
        r#"echo '{"metric":"early","value":1}'
echo "address $1 $NETSCENARIO_ADDR_SERVER $NETSCENARIO_ADDR_CLIENT node=$NETSCENARIO_NODE id=$NETSCENARIO_WORKLOAD mine=$MINE"
echo "dir $(pwd) group $(ps -o pgid= -p $$ | tr -d ' ') pid $$"
echo "run dir $NETSCENARIO_RUN_DIR seed $NETSCENARIO_SEED"
echo ready
sleep 1.5
echo '{"metric":"late","value":42.5,"unit":"ms"}'
echo '{"metric":"late","value":7}'
echo '{"metric":"broken","value":"x"}'
echo 'plain text'
echo 'trouble' >&2
sleep 60"#,
    );
    let result = go(
        &dir,
        &scenario(
            "4s",
            "",
            "  - { id: app, kind: command, node: server, run: [\"./app.sh\", \"{server.addr}\"], env: { MINE: \"on {client.addr}\" }, ready: { stdout-line: ready, within: 5s }, shutdown: { signal: TERM, within: 3s } }\n",
        ),
    )
    .unwrap();
    let program = &result.programs[0];
    assert_eq!(
        (program.id.as_str(), program.node.as_str()),
        ("app", "server")
    );
    // The placeholder is replaced in the arguments, and the result shows the program as it was run
    assert_eq!(program.run, ["./app.sh", "10.100.0.2"]);
    let stdout = std::fs::read_to_string(dir.join(&program.stdout_file)).unwrap();
    assert!(
        stdout.contains(
            "address 10.100.0.2 10.100.0.2 10.100.0.1 node=server id=app mine=on 10.100.0.1"
        ),
        "{stdout}"
    );
    // It runs in the directory of the scenario, and leads a process group of its own (the pid is the group)
    let dir_line = stdout.lines().find(|l| l.starts_with("dir ")).unwrap();
    let words: Vec<&str> = dir_line.split_whitespace().collect();
    assert_eq!(Path::new(words[1]), dir.canonicalize().unwrap());
    assert_eq!(
        words[3], words[5],
        "the program is not the leader of its group: {dir_line}"
    );
    assert!(stdout.contains(&format!(
        "run dir {}",
        dir.canonicalize().unwrap().display()
    )));
    assert!(stdout.contains("seed 5"));
    assert_eq!(
        std::fs::read_to_string(dir.join(&program.stderr_file)).unwrap(),
        "trouble\n"
    );
    // Stopped by the run with the signal of `shutdown`, after the packets
    assert_eq!(program.ended, "stopped");
    assert_eq!((program.exit_code, program.exit_signal), (None, Some(15)));
    assert!(program.started_ns < 0 && program.ready_ns >= program.started_ns);
    assert!(program.ended_ns >= 4_000_000_000);
    // The metrics: the summary, the unit, the line that is not valid, and the times
    let late = program.metrics.iter().find(|m| m.name == "late").unwrap();
    assert_eq!(
        (late.count, late.first, late.last, late.min, late.max),
        (2, 42.5, 7.0, 7.0, 42.5)
    );
    assert_eq!(late.unit.as_deref(), Some("ms"));
    assert_eq!(program.metric_readings, 3);
    assert_eq!(program.bad_metric_lines, 1);
    assert!(program.bad_metric_examples[0].contains("`value` must be a number"));
    let readings =
        std::fs::read_to_string(dir.join(program.metrics_file.as_ref().unwrap())).unwrap();
    let times: Vec<(String, i64)> = readings
        .lines()
        .map(|line| {
            let json: serde_json::Value = serde_json::from_str(line).unwrap();
            (
                json["metric"].as_str().unwrap().to_owned(),
                json["t_ns"].as_i64().unwrap(),
            )
        })
        .collect();
    // `early` was printed before the measured phase began: its time is negative. `late` was printed 1.5 s after the
    // script began, and a reading is never earlier than the line (a little room for the spawn before `started_ns`)
    assert!(times[0].0 == "early" && times[0].1 < 0, "{times:?}");
    assert!(
        times[1].1 >= program.started_ns + 1_450_000_000,
        "{times:?} started {}",
        program.started_ns
    );
    assert!(times[1].1 < program.started_ns + 3_000_000_000, "{times:?}");
    assert_eq!(program.output.stderr_bytes, 8);
    assert!(!program.lines_dropped);
    // A run with a program and no workload that sends has no workloads, and no tally of the packets
    assert!(result.workloads.is_empty() && result.directions.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// Every way of being ready: a UDP socket (a compiled program, the agent itself), a TCP socket (a script of Python), a
/// file that appears, and a delay. Each program starts only when the one before is ready, and the senders after all.
#[test]
fn every_way_of_being_ready_is_waited_for_in_the_order_of_the_file() {
    if !ready() {
        return;
    }
    let python = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok();
    let dir = work_dir("programs-ready");
    script(&dir, "file.sh", "sleep 0.6\ntouch flag\nsleep 60");
    script(
        &dir,
        "tcp.py",
        "exec python3 -c 'import socket,time; s=socket.socket(); s.bind((\"0.0.0.0\",4434)); s.listen(); time.sleep(60)'",
    );
    script(&dir, "delay.sh", "sleep 60");
    let agent = agent_exe().unwrap();
    let out = dir.join("echo.records");
    let tcp = if python {
        "  - { id: tcp, kind: command, node: server, run: [\"./tcp.py\"], ready: { tcp-port: 4434, within: 5s } }\n"
    } else {
        ""
    };
    let workloads = format!(
        "  - {{ id: udp, kind: command, node: server, run: [\"{}\", \"recv\", \"--bind\", \"{{server.addr}}:4433\", \"--out\", \"{}\"], ready: {{ udp-port: 4433, within: 5s }}, shutdown: {{ signal: TERM, within: 3s }} }}\n{tcp}  - {{ id: file, kind: command, node: client, run: [\"./file.sh\"], ready: {{ file: flag, within: 5s }} }}\n  - {{ id: delay, kind: command, node: client, run: [\"./delay.sh\"], ready: {{ delay: 400ms }} }}\n",
        agent.display(),
        out.display()
    );
    let result = go(&dir, &scenario("3s", "", &workloads)).unwrap();
    let ids: Vec<&str> = result.programs.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        if python {
            vec!["udp", "tcp", "file", "delay"]
        } else {
            vec!["udp", "file", "delay"]
        }
    );
    // Each started after the one before was ready
    for pair in result.programs.windows(2) {
        assert!(pair[1].started_ns >= pair[0].ready_ns, "{pair:?}");
    }
    let by_id = |id: &str| result.programs.iter().find(|p| p.id == id).unwrap();
    // The file appears after 0.6 s; the delay is 0.4 s; neither is ready earlier (the clock of the run)
    assert!(by_id("file").ready_ns - by_id("file").started_ns >= 550_000_000);
    assert!(by_id("delay").ready_ns - by_id("delay").started_ns >= 400_000_000);
    // The agent ended on `TERM` with its signal, as a program should
    assert_eq!(by_id("udp").exit_signal, Some(15));
    let _ = std::fs::remove_dir_all(dir);
}

/// A program that is not ready in time, one that ends first, and one that cannot be started, fail the run before any
/// packet, name the program, and say what it was waited for and what the program said; nothing is left.
#[test]
fn a_program_that_is_not_ready_ends_first_or_cannot_start_fails_the_run_with_the_reason() {
    if !ready() {
        return;
    }
    let dir = work_dir("programs-notready");
    script(&dir, "slow.sh", "echo 'warming up' >&2\nsleep 60");
    script(&dir, "dies.sh", "echo 'cannot bind' >&2\nexit 7");
    let fail = |workloads: &str| {
        go(&dir, &scenario("3s", "", workloads)).expect_err("the run should fail")
    };
    let late = message(&fail(
        "  - { id: app, kind: command, node: server, run: [\"./slow.sh\"], ready: { udp-port: 4433, within: 1s } }\n",
    ));
    assert!(
        late.contains("'app'")
            && late.contains("not ready within 1s")
            && late.contains("UDP socket on port 4433")
            && late.contains("warming up"),
        "{late}"
    );
    let ended = message(&fail(
        "  - { id: app, kind: command, node: server, run: [\"./dies.sh\"], ready: { tcp-port: 4433, within: 5s } }\n",
    ));
    assert!(
        ended.contains("exit code 7")
            && ended.contains("before it was ready")
            && ended.contains("cannot bind"),
        "{ended}"
    );
    let missing = message(&fail(
        "  - { id: app, kind: command, node: server, run: [\"./does-not-exist\"] }\n",
    ));
    assert!(missing.contains("'app'"), "{missing}");
    // A port on the loopback is not a port of the node: a program that binds only there is not ready
    script(
        &dir,
        "loopback.py",
        "exec python3 -c 'import socket,time; s=socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind((\"127.0.0.1\",4433)); time.sleep(60)'",
    );
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok()
    {
        let loopback = message(&fail(
            "  - { id: app, kind: command, node: server, run: [\"./loopback.py\"], ready: { udp-port: 4433, within: 1s } }\n",
        ));
        assert!(loopback.contains("not ready within"), "{loopback}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A program that ignores `TERM` and has a child: after `within` the group gets `KILL`, both are gone, and the result
/// says so. A program that obeys is `stopped`, and one that is gone by itself is `on-its-own`.
#[test]
fn a_program_that_ignores_the_signal_is_killed_with_its_children() {
    if !ready() {
        return;
    }
    let dir = work_dir("programs-kill");
    script(
        &dir,
        "stubborn.sh",
        "trap '' TERM\nsleep 61.7 &\necho started\nwhile :; do sleep 0.2; done",
    );
    let began = std::time::Instant::now();
    let result = go(
        &dir,
        &scenario(
            "2s",
            "",
            "  - { id: app, kind: command, node: server, run: [\"./stubborn.sh\"], ready: { stdout-line: started, within: 5s }, shutdown: { signal: TERM, within: 700ms } }\n",
        ),
    )
    .unwrap();
    let program = &result.programs[0];
    assert_eq!(program.ended, "killed");
    assert_eq!((program.exit_code, program.exit_signal), (None, Some(9)));
    // The run did not wait for the child: it ended 0.7 s after the stop, not after the 61.7 s of the child
    assert!(
        began.elapsed() < Duration::from_secs(20),
        "{:?}",
        began.elapsed()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A program that ends by itself with an error: the run goes on to its end and reports it (the default), or stops with
/// an error (`on-exit: fail`); one that ends with 0 is not a failure either way.
#[test]
fn a_program_that_ends_by_itself_is_reported_or_fails_the_run_as_the_file_says() {
    if !ready() {
        return;
    }
    let dir = work_dir("programs-crash");
    script(
        &dir,
        "crash.sh",
        "echo 'about to fail' >&2\nsleep 0.5\nexit 3",
    );
    script(&dir, "done.sh", "sleep 0.3\nexit 0");
    let line = |extra: &str, run: &str| {
        format!("  - {{ id: app, kind: command, node: server, run: [\"{run}\"]{extra} }}\n")
    };
    let kept = go(&dir, &scenario("3s", "", &line("", "./crash.sh"))).unwrap();
    let program = &kept.programs[0];
    assert_eq!(
        (program.ended.as_str(), program.exit_code),
        ("on-its-own", Some(3))
    );
    // It ended 0.5 s into its life, and the run went on to the end of the scenario
    assert!(program.ended_ns < 2_000_000_000, "{}", program.ended_ns);
    assert_eq!(kept.timing.duration_ns, 3_000_000_000);
    let error = go(
        &dir,
        &scenario("3s", "", &line(", on-exit: fail", "./crash.sh")),
    )
    .expect_err("fail");
    let text = message(&error);
    assert!(
        text.contains("'app'")
            && text.contains("exit code 3")
            && text.contains("on-exit: fail")
            && text.contains("about to fail"),
        "{text}"
    );
    for extra in ["", ", on-exit: fail"] {
        let fine = go(&dir, &scenario("2s", "", &line(extra, "./done.sh"))).unwrap();
        assert_eq!(fine.programs[0].exit_code, Some(0));
        assert_eq!(fine.programs[0].ended, "on-its-own");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Output that is far past the limits: three megabytes in one line. The program is never blocked, the file holds the
/// first megabyte, and what was dropped is counted in the result and not hidden; the line is cut and counted too.
#[test]
fn a_program_that_writes_a_lot_is_not_blocked_and_what_is_dropped_is_counted() {
    if !ready() {
        return;
    }
    let dir = work_dir("programs-loud");
    script(
        &dir,
        "loud.sh",
        "head -c 3000000 /dev/zero | tr '\\0' 'x'\necho\nhead -c 2000000 /dev/zero | tr '\\0' 'e' >&2\necho done\nsleep 60",
    );
    let result = go(
        &dir,
        &scenario(
            "2s",
            "",
            "  - { id: app, kind: command, node: server, run: [\"./loud.sh\"], ready: { stdout-line: done, within: 10s } }\n",
        ),
    )
    .unwrap();
    let program = &result.programs[0];
    let limit = 1024 * 1024;
    assert_eq!(program.output.stdout_bytes, 3_000_000 + 1 + 5);
    assert_eq!(program.output.stdout_dropped_bytes, 3_000_006 - limit);
    assert_eq!(program.output.stderr_bytes, 2_000_000);
    assert_eq!(program.output.stderr_dropped_bytes, 2_000_000 - limit);
    assert_eq!(program.output.long_lines, 1);
    assert_eq!(
        std::fs::metadata(dir.join(&program.stdout_file))
            .unwrap()
            .len(),
        limit
    );
    assert_eq!(
        std::fs::metadata(dir.join(&program.stderr_file))
            .unwrap()
            .len(),
        limit
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A program next to a workload that sends: the run counts only its own packets, so it keeps no tally of the directions and
/// does not hold the queues to the number it sent; and when the link draws random numbers the result says that the lost
/// packets are not promised to repeat.
#[test]
fn a_program_next_to_a_stream_keeps_the_run_honest_about_what_it_cannot_count() {
    if !ready() {
        return;
    }
    let dir = work_dir("programs-mixed");
    script(&dir, "app.sh", "echo ready\nsleep 60");
    let result = go(
        &dir,
        &scenario(
            "2s",
            "      forward: { delay: 5ms, loss: { model: random, p: 5% } }\n",
            "  - { id: media, kind: udp-cbr, from: client, to: server, rate: 800kbit, packet-size: 1000b }\n  - { id: app, kind: command, node: client, run: [\"./app.sh\"], ready: { stdout-line: ready, within: 5s } }\n",
        ),
    )
    .unwrap();
    assert_eq!(result.workloads.len(), 1);
    assert!(result.directions.is_empty() && result.seed.queue_checks.is_empty());
    assert!(!result.seed.loss_reproducible);
    assert!(
        result
            .seed
            .not_reproducible_because
            .iter()
            .any(|reason| reason.contains("the program 'app'")),
        "{:?}",
        result.seed.not_reproducible_because
    );
    // Canary: without the program the same run is reproducible and has the tally
    let plain = go(
        &dir,
        &scenario(
            "2s",
            "      forward: { delay: 5ms, loss: { model: random, p: 5% } }\n",
            "  - { id: media, kind: udp-cbr, from: client, to: server, rate: 800kbit, packet-size: 1000b }\n",
        ),
    )
    .unwrap();
    assert!(plain.seed.loss_reproducible && !plain.directions.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// A cancel while a program runs ends the program and its children, and leaves nothing.
#[test]
fn a_cancel_while_a_program_runs_leaves_nothing_behind() {
    if !ready() {
        return;
    }
    let dir = work_dir("programs-cancel");
    script(&dir, "app.sh", "sleep 61.3 &\necho ready\nwait");
    let token = CancelToken::new();
    let mut backend = backend(|_| {});
    let mut options = options(&dir);
    options.cancel = token.clone();
    let canceller = {
        let token = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2500));
            token.cancel();
        })
    };
    let result = run(
        &mut backend,
        &scenario(
            "20s",
            "",
            "  - { id: app, kind: command, node: server, run: [\"./app.sh\"], ready: { stdout-line: ready, within: 5s } }\n",
        ),
        &options,
    );
    canceller.join().unwrap();
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert_nothing_left(backend.run_id(), "after the cancel");
    let _ = std::fs::remove_dir_all(dir);
}
