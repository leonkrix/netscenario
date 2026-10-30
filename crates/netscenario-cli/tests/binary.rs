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
