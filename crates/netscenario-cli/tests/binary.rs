//! Tests of the real binary: the exit code and the streams as a shell sees them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use netscenario_core::RunResult;

fn netscenario(args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_netscenario"))
        .args(args)
        .output()
}

#[test]
fn version_exits_with_zero() -> io::Result<()> {
    let output = netscenario(&["--version"])?;
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("netscenario "));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    Ok(())
}

#[test]
fn no_command_exits_with_two() -> io::Result<()> {
    let output = netscenario(&[])?;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert!(String::from_utf8_lossy(&output.stderr).contains("no command given"));
    Ok(())
}

#[test]
fn doctor_exits_with_zero_or_three_and_says_why() -> io::Result<()> {
    let output = netscenario(&["doctor"])?;
    let text = String::from_utf8_lossy(&output.stdout);
    match output.status.code() {
        Some(0) => assert!(text.contains("scenarios can run here"), "{text}"),
        Some(3) => assert!(text.contains("FAIL") && text.contains("fix:"), "{text}"),
        other => panic!("unexpected exit code {other:?}\n{text}"),
    }
    Ok(())
}

#[test]
fn clean_with_nothing_to_do_exits_with_zero() -> io::Result<()> {
    let output = netscenario(&["clean", "--dry-run"])?;
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("netscenario clean"));
    Ok(())
}

// ---- run, validate and the hidden commands, with the real program ----

fn netscenario_in(dir: &Path, args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_netscenario"))
        .args(args)
        .current_dir(dir)
        .output()
}

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("netscenario-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A test that needs the namespaces of a run stops where the machine does not allow them, and fails on the CI.
fn can_run(test: &str) -> bool {
    let agent = Path::new(env!("CARGO_BIN_EXE_netscenario")).with_file_name("netscenario-agent");
    let probe = Command::new("unshare")
        .args([
            "--user",
            "--map-root-user",
            "--net",
            "--mount",
            "--pid",
            "--fork",
            "--kill-child",
            "--mount-proc",
            "true",
        ])
        .stdin(std::process::Stdio::null())
        .output();
    let problem = if !agent.is_file() {
        Some(format!(
            "the agent is not built at {} (use `just test`)",
            agent.display()
        ))
    } else if !probe.is_ok_and(|p| p.status.success()) {
        Some("the machine does not allow user namespaces".to_owned())
    } else {
        None
    };
    if let Some(problem) = problem {
        assert!(
            std::env::var_os("CI").is_none(),
            "{test} must run on the CI, but: {problem}"
        );
        eprintln!("SKIPPED {test}: {problem}");
        return false;
    }
    true
}

const SCENARIO: &str = "version: 0
name: cli-test
duration: 2s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward: { delay: 20ms }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }
";

fn write_scenario(dir: &Path, text: &str) -> String {
    let file = dir.join("scenario.yaml");
    std::fs::write(&file, text).unwrap();
    file.display().to_string()
}

#[test]
fn run_writes_the_result_and_prints_the_summary() {
    if !can_run("run_writes_the_result_and_prints_the_summary") {
        return;
    }
    let dir = work_dir("run");
    let file = write_scenario(&dir, SCENARIO);
    let out = dir.join("result");
    let output = netscenario_in(
        &dir,
        &[
            "run",
            &file,
            "--out",
            out.to_str().unwrap(),
            "--packets",
            "--settle",
            "300ms",
        ],
    )
    .unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("running cli-test for 2s"), "{stderr}");
    for expected in [
        "netscenario run: cli-test",
        "workload media  client -> server  2 mbit/s",
        "sent 417  received 417  lost 0 (0.00 %)",
        "result: ",
    ] {
        assert!(stdout.contains(expected), "{expected}\n{stdout}");
    }
    let result =
        RunResult::from_json(&std::fs::read_to_string(out.join("result.json")).unwrap()).unwrap();
    let media = &result.workloads[0];
    assert_eq!((media.stats.sent, media.stats.lost), (417, 0));
    assert!((20_000_000..22_000_000).contains(&media.stats.delay.as_ref().unwrap().p50_ns));
    assert_eq!(result.timing.settle_ns, 300_000_000);
    // --packets wrote one line for each packet
    let packets = std::fs::read_to_string(out.join("packets-media.jsonl")).unwrap();
    assert_eq!(packets.lines().count(), 417);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn json_prints_the_result_and_a_seed_can_be_given() {
    if !can_run("json_prints_the_result_and_a_seed_can_be_given") {
        return;
    }
    let dir = work_dir("json");
    let file = write_scenario(&dir, SCENARIO);
    let out = dir.join("result");
    let output = netscenario_in(
        &dir,
        &[
            "run",
            &file,
            "--out",
            out.to_str().unwrap(),
            "--json",
            "--seed",
            "7",
            "--settle",
            "300ms",
        ],
    )
    .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = RunResult::from_json(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let written =
        RunResult::from_json(&std::fs::read_to_string(out.join("result.json")).unwrap()).unwrap();
    assert_eq!(printed, written, "standard output is the file");
    assert_eq!(printed.seed.value, Some(7));
    assert!(!printed.seed.drawn);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_out_the_result_goes_into_a_directory_with_the_name_and_the_time() {
    if !can_run("without_out_the_result_goes_into_a_directory_with_the_name_and_the_time") {
        return;
    }
    let dir = work_dir("default-out");
    let file = write_scenario(&dir, &SCENARIO.replace("duration: 2s", "duration: 1s"));
    let output = netscenario_in(&dir, &["run", &file, "--settle", "200ms"]).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results: Vec<String> = std::fs::read_dir(dir.join("netscenario-results"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(results.len(), 1);
    // cli-test-20261030T122730Z
    assert!(
        results[0].starts_with("cli-test-20") && results[0].ends_with('Z'),
        "{results:?}"
    );
    assert!(
        dir.join("netscenario-results")
            .join(&results[0])
            .join("result.json")
            .is_file()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A scenario that is wrong is refused before anything is created, with the line of the file.
#[test]
fn a_faulty_scenario_is_refused_with_exit_2_and_creates_nothing() {
    let dir = work_dir("faulty");
    let file = write_scenario(&dir, &SCENARIO.replace("delay: 20ms", "delay: 20"));
    let out = dir.join("result");
    let output = netscenario_in(&dir, &["run", &file, "--out", out.to_str().unwrap()]).unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("error: "), "{stderr}");
    assert!(stderr.contains("scenario.yaml:9:"), "{stderr}");
    assert!(!out.exists(), "nothing was created");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let _ = std::fs::remove_dir_all(dir);
}

/// The backend refuses what it cannot apply exactly (here: nanoseconds, where `netem` works in microseconds),
/// with the line of the file, and creates nothing.
#[test]
fn what_the_backend_cannot_reproduce_is_refused_with_exit_2() {
    if !can_run("what_the_backend_cannot_reproduce_is_refused_with_exit_2") {
        return;
    }
    let dir = work_dir("refused");
    let file = write_scenario(&dir, &SCENARIO.replace("delay: 20ms", "delay: 1500ns"));
    let out = dir.join("result");
    let output = netscenario_in(&dir, &["run", &file, "--out", out.to_str().unwrap()]).unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("kernel backend cannot reproduce"),
        "{stderr}"
    );
    assert!(stderr.contains("scenario.yaml:9:"), "{stderr}");
    assert!(!out.join("result.json").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// The agent belongs next to the program. Without it the run says where it looked, with the exit code of an
/// environment problem.
#[test]
fn a_program_without_its_agent_says_so() {
    let dir = work_dir("noagent");
    let lonely = dir.join("netscenario");
    std::fs::copy(env!("CARGO_BIN_EXE_netscenario"), &lonely).unwrap();
    let file = write_scenario(&dir, SCENARIO);
    let output = Command::new(&lonely)
        .args(["run", &file, "--out", dir.join("result").to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("the agent") && stderr.contains("was not found"),
        "{stderr}"
    );
    assert!(
        stderr.contains(dir.to_str().unwrap()),
        "it names where it looked: {stderr}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The two hidden commands are the program as the supervisor of a run. Started on the host, outside the
/// namespaces of a run, the supervisor refuses before it does anything (SAF-01).
#[test]
fn the_supervisor_inside_the_program_refuses_to_run_on_the_host() {
    let output = netscenario(&["supervise", "refusal-test", "8"]).unwrap();
    assert_ne!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\":\"error\"") && stdout.contains("SAF-01"),
        "{stdout}"
    );
    // and the help does not know it
    let help = netscenario(&["--help"]).unwrap();
    assert!(!String::from_utf8_lossy(&help.stdout).contains("supervise"));
}

#[test]
fn validate_answers_with_the_exit_code() -> io::Result<()> {
    let good = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/minimal.yaml");
    let output = netscenario(&["validate", good.to_str().unwrap()])?;
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("ok: minimal"));
    let bad = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/invalid/empty.yaml");
    let output = netscenario(&["validate", bad.to_str().unwrap()])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("error: "));
    Ok(())
}

/// On a terminal the run shows one line that is written over, and clears it before the summary. A terminal is
/// made with `script` (util-linux), which runs the program in a pseudo-terminal.
#[test]
fn on_a_terminal_the_run_shows_a_progress_line_and_clears_it() {
    if !can_run("on_a_terminal_the_run_shows_a_progress_line_and_clears_it") {
        return;
    }
    if Command::new("script").arg("--version").output().is_err() {
        assert!(
            std::env::var_os("CI").is_none(),
            "this test must run on the CI, but `script` is not installed"
        );
        eprintln!("SKIPPED: `script` is not installed");
        return;
    }
    let dir = work_dir("terminal");
    let file = write_scenario(&dir, SCENARIO);
    let out = dir.join("result");
    let line = format!(
        "{} run {file} --out {} --settle 300ms",
        env!("CARGO_BIN_EXE_netscenario"),
        out.display()
    );
    let output = Command::new("script")
        .args(["--quiet", "--return", "--command", &line, "/dev/null"])
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{shown}");
    for phase in [
        "building the network",
        "receivers ready, starting the senders",
        "running, 0 of 2 s",
        "waiting 0.3 s for the packets in the network",
        "removing the network",
    ] {
        assert!(shown.contains(phase), "no '{phase}' in: {shown:?}");
    }
    // The line is erased before the summary, which starts at the beginning of a line
    assert!(shown.contains("\x1b[2K"), "{shown:?}");
    assert!(shown.contains("netscenario run: cli-test"), "{shown:?}");
    assert!(shown.contains("sent 417  received 417"), "{shown:?}");
    // On a terminal the plain line is not printed as well
    assert!(!shown.contains("running cli-test for 2s"), "{shown:?}");
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Ctrl-C (D-066) ----

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// A scenario that lasts long enough to be interrupted: 30 s at 208 packets a second.
const LONG: &str = "version: 0
name: cli-interrupt
duration: 30s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      forward: { delay: 20ms }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }
timeline:
  - { at: 25s, link: access, set: { forward: { delay: 40ms } } }
";

/// The command lines of the processes that contain `needle`, other than this one.
fn processes_with(needle: &str) -> Vec<String> {
    let me = std::process::id().to_string();
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name != me && name.chars().all(|c| c.is_ascii_digit())
        })
        .filter_map(|entry| std::fs::read(entry.path().join("cmdline")).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .filter(|line| line.contains(needle))
        .collect()
}

/// Starts `netscenario run` in a process group of its own, as a shell does, and waits until the workloads run.
fn start_long_run(
    dir: &Path,
    out: &Path,
) -> (std::process::Child, BufReader<std::process::ChildStderr>) {
    let file = write_scenario(dir, LONG);
    let mut child = Command::new(env!("CARGO_BIN_EXE_netscenario"))
        .args([
            "run",
            &file,
            "--out",
            out.to_str().unwrap(),
            "--settle",
            "300ms",
        ])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    assert!(line.contains("running cli-interrupt for 30s"), "{line}");
    // The network is built when the receiver of the run exists (its command line has the directory of this test); the
    // senders start right after it, and then the workloads go for a while
    let deadline = Instant::now() + Duration::from_secs(30);
    while !processes_with(&dir.display().to_string())
        .iter()
        .any(|line| line.contains(" recv "))
    {
        assert!(
            Instant::now() < deadline,
            "the receiver of the run did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(2000));
    (child, stderr)
}

/// What a terminal does for Ctrl-C: the signal goes to every process of the foreground group.
fn signal_group(child: &std::process::Child, signal: &str) {
    let status = Command::new("kill")
        .args([&format!("-{signal}"), "--", &format!("-{}", child.id())])
        .status()
        .unwrap();
    assert!(status.success());
}

/// Waits for the program to end and returns its exit code, what it wrote to standard output, and what it wrote to
/// standard error after the first line (`stderr` has read that).
fn finish(
    mut child: std::process::Child,
    mut stderr: BufReader<std::process::ChildStderr>,
    within: Duration,
) -> (Option<i32>, String, String) {
    let began = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            began.elapsed() < within,
            "the program did not end in {within:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut out).unwrap();
    let mut err = String::new();
    std::io::Read::read_to_string(&mut stderr, &mut err).unwrap();
    (status.code(), out, err)
}

/// Ctrl-C while the workloads run stops them, waits for the packets in the network, writes the result of the part that
/// ran and exits with 130; the supervisor, the nodes and the agents are not hit by the signal that goes to the group of
/// the terminal, and nothing is left.
#[test]
fn ctrl_c_stops_the_workloads_and_keeps_the_result() {
    if !can_run("ctrl_c_stops_the_workloads_and_keeps_the_result") {
        return;
    }
    let dir = work_dir("interrupt");
    let out = dir.join("result");
    let (child, stderr) = start_long_run(&dir, &out);
    signal_group(&child, "INT");
    let began = Instant::now();
    let (code, stdout, stderr) = finish(child, stderr, Duration::from_secs(15));
    assert_eq!(code, Some(130), "{stdout}\n{stderr}");
    assert!(
        began.elapsed() < Duration::from_secs(8),
        "{:?}",
        began.elapsed()
    );
    assert!(
        stderr.contains("interrupted; stopping the workloads"),
        "{stderr}"
    );
    assert!(stdout.contains("interrupted after "), "{stdout}");
    let result =
        RunResult::from_json(&std::fs::read_to_string(out.join("result.json")).unwrap()).unwrap();
    let interrupted = result
        .timing
        .interrupted_ns
        .expect("the result says it was interrupted");
    // About 2.5 s after the start of the workloads, far from the 30 s of the scenario
    assert!(
        (1_000_000_000..10_000_000_000).contains(&interrupted),
        "{interrupted} ns"
    );
    let media = &result.workloads[0];
    // 208 packets a second for that long: a part of the stream, all of it that was sent arrived, and the step at 25 s
    // was not done
    assert!(
        (200..2_500).contains(&media.stats.sent),
        "{} packets",
        media.stats.sent
    );
    assert_eq!(media.stats.lost, 0);
    assert_eq!(result.events.len(), 0, "{:?}", result.events);
    // Nothing of the run is left: no agent with the directory of this test in its command line
    let deadline = Instant::now() + Duration::from_secs(5);
    while !processes_with(&dir.display().to_string()).is_empty() {
        assert!(
            Instant::now() < deadline,
            "{:?}",
            processes_with(&dir.display().to_string())
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A second Ctrl-C quits at once, with the code of the signal, and the network is removed all the same.
#[test]
fn a_second_ctrl_c_quits_at_once_and_nothing_is_left() {
    if !can_run("a_second_ctrl_c_quits_at_once_and_nothing_is_left") {
        return;
    }
    let dir = work_dir("interrupt-twice");
    let out = dir.join("result");
    let (child, stderr) = start_long_run(&dir, &out);
    signal_group(&child, "INT");
    std::thread::sleep(Duration::from_millis(100));
    signal_group(&child, "INT");
    let (code, _stdout, stderr) = finish(child, stderr, Duration::from_secs(5));
    assert_eq!(code, Some(130), "{stderr}");
    assert!(stderr.contains("quitting at once"), "{stderr}");
    // The supervisor sees the pipe close and removes the network
    let deadline = Instant::now() + Duration::from_secs(10);
    let needle = dir.display().to_string();
    while !processes_with(&needle).is_empty() {
        assert!(Instant::now() < deadline, "{:?}", processes_with(&needle));
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// `kill` (SIGTERM) does the same as Ctrl-C, and the exit code says which signal it was.
#[test]
fn sigterm_stops_the_run_like_ctrl_c_with_the_code_143() {
    if !can_run("sigterm_stops_the_run_like_ctrl_c_with_the_code_143") {
        return;
    }
    let dir = work_dir("interrupt-term");
    let out = dir.join("result");
    let (child, stderr) = start_long_run(&dir, &out);
    signal_group(&child, "TERM");
    let (code, stdout, stderr) = finish(child, stderr, Duration::from_secs(15));
    assert_eq!(code, Some(143), "{stdout}\n{stderr}");
    let result =
        RunResult::from_json(&std::fs::read_to_string(out.join("result.json")).unwrap()).unwrap();
    assert!(result.timing.interrupted_ns.is_some());
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The gate of the doctor (D-068) ----

/// A machine where `tc` finds no table for the normal distribution (here `TC_LIB_DIR` points nowhere, for the program
/// only): a scenario with a normal jitter is refused before anything is built, with the line of the file and the way
/// out, and not after the network is up with an error of `tc`. A uniform jitter still runs.
#[test]
fn a_normal_jitter_is_refused_before_the_network_is_built_where_tc_has_no_table() {
    if !can_run("a_normal_jitter_is_refused_before_the_network_is_built_where_tc_has_no_table") {
        return;
    }
    let dir = work_dir("no-tables");
    let normal = SCENARIO.replace(
        "forward: { delay: 20ms }",
        "forward: { delay: 20ms, jitter: { amount: 5ms, distribution: normal } }",
    );
    let file = write_scenario(&dir, &normal);
    let output = Command::new(env!("CARGO_BIN_EXE_netscenario"))
        .args(["run", &file, "--out", dir.join("result").to_str().unwrap()])
        .env("TC_LIB_DIR", dir.join("nowhere"))
        .current_dir(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    for expected in [
        "cannot reproduce",
        "needs a table that `tc` cannot find",
        "path: topology.links[0].forward.jitter.distribution",
        "TC_LIB_DIR is set to",
    ] {
        assert!(stderr.contains(expected), "{expected}\n{stderr}");
    }
    // Nothing was built and no result was written
    assert!(!dir.join("result").join("result.json").exists());

    // Canary: the same machine runs a scenario with the uniform jitter
    let uniform = SCENARIO.replace(
        "forward: { delay: 20ms }",
        "forward: { delay: 20ms, jitter: { amount: 5ms } }",
    );
    let file = write_scenario(&dir, &uniform);
    let output = Command::new(env!("CARGO_BIN_EXE_netscenario"))
        .args([
            "run",
            &file,
            "--out",
            dir.join("result").to_str().unwrap(),
            "--settle",
            "300ms",
        ])
        .env("TC_LIB_DIR", dir.join("nowhere"))
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}
