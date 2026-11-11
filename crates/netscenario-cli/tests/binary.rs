//! Tests of the real binary: the exit code and the streams as a shell sees them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use netscenario_core::{RunResult, RunSetResult, derive_repetition_seed};

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

/// A scenario whose loss is random, so that the runs of a set differ from each other and a seed matters.
const LOSSY: &str = "version: 0
name: cli-set
duration: 1s
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      both: { delay: 5ms, loss: { model: random, p: 10% } }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1200b }
";

/// Runs the set of `LOSSY` with the seed 7 into `out` and reads `runset.json`.
fn run_lossy_set(dir: &Path, out: &Path) -> (Output, Option<RunSetResult>) {
    let file = write_scenario(dir, LOSSY);
    let output = netscenario_in(
        dir,
        &[
            "run",
            &file,
            "--out",
            out.to_str().unwrap(),
            "--seed",
            "7",
            "--repetitions",
            "5",
            "--settle",
            "300ms",
        ],
    )
    .unwrap();
    let set = std::fs::read_to_string(out.join("runset.json"))
        .ok()
        .map(|text| RunSetResult::from_json(&text).unwrap());
    (output, set)
}

/// ASR-02 and ASR-03 with the real program: every repetition has a directory and a seed derived from the seed of the set, the runs
/// differ from each other, the same seed gives the same set, and the summary shows the intervals.
///
/// A run of 1 s at 2 mbit/s sends 208 packets at 10 % loss: the lost packets of a run are binomial with mean 20.8 and standard
/// deviation 4.3, so five runs have the same count with a probability below 1e-5 (the chance that two runs of a set differ is
/// what the check on distinct counts below relies on).
#[test]
fn a_set_of_repetitions_derives_its_seeds_and_repeats_from_the_seed() {
    if !can_run("a_set_of_repetitions_derives_its_seeds_and_repeats_from_the_seed") {
        return;
    }
    let dir = work_dir("set");
    let (output, set) = run_lossy_set(&dir, &dir.join("first"));
    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("running cli-set 5 times for 1s each"),
        "{stderr}"
    );
    assert!(stderr.contains("repetition 5 of 5 (seed"), "{stderr}");
    for expected in [
        "netscenario run: cli-set, 5 repetitions",
        "seed 7\n",
        "interval of the mean: 95 %",
        "media\n",
        "loss_ratio",
        "set: ",
    ] {
        assert!(stdout.contains(expected), "{expected}\n{stdout}");
    }
    let set = set.expect("runset.json");
    assert_eq!(
        (set.seed.value, set.seed.drawn, set.runs.len()),
        (7, false, 5)
    );
    for (index, run) in set.runs.iter().enumerate() {
        let index = u32::try_from(index).unwrap();
        assert_eq!(run.seed, derive_repetition_seed(7, index));
        assert_eq!(run.directory, format!("run-{:03}", index + 1));
        let result = RunResult::from_json(
            &std::fs::read_to_string(dir.join("first").join(&run.directory).join("result.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result.seed.value, Some(run.seed));
    }
    let lost = set.metric("media.lost").expect("media.lost");
    let mut distinct = lost.values.clone();
    distinct.sort_by(f64::total_cmp);
    distinct.dedup();
    assert!(
        distinct.len() >= 2,
        "the five runs lost the same: {:?}",
        lost.values
    );
    assert!(lost.interval.low <= lost.mean && lost.mean <= lost.interval.high);
    // The same seed gives the same set: the lost packets, run by run
    let (again, second) = run_lossy_set(&dir, &dir.join("second"));
    assert_eq!(again.status.code(), Some(0));
    let second = second.expect("runset.json of the second set");
    assert_eq!(second.metric("media.lost").unwrap().values, lost.values);
    assert_eq!(
        second.metric("media.loss_ratio").unwrap().interval,
        set.metric("media.loss_ratio").unwrap().interval
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Runs `LOSSY` with the seed 7 into `dir/first` and returns the path of the scenario file and the directory.
fn first_run(dir: &Path) -> (String, PathBuf) {
    let file = write_scenario(dir, LOSSY);
    let first = dir.join("first");
    let output = netscenario_in(
        dir,
        &[
            "run",
            &file,
            "--out",
            first.to_str().unwrap(),
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
    (file, first)
}

/// ENV-03 with the real program: a run records its scenario, a replay with the recorded seed loses the same packets, and a
/// scenario file that differs from the recorded scenario is refused with the fields.
#[test]
fn a_replay_loses_the_same_packets_and_a_file_that_differs_is_refused() {
    if !can_run("a_replay_loses_the_same_packets_and_a_file_that_differs_is_refused") {
        return;
    }
    let dir = work_dir("replay");
    let (file, first) = first_run(&dir);
    assert!(
        first.join("scenario.json").is_file(),
        "the run records its scenario"
    );
    let first_text = first.to_str().unwrap();

    // The replay: the same seed, the same lost packets, exit 0
    let again = dir.join("again");
    let output = netscenario_in(
        &dir,
        &["replay", first_text, "--out", again.to_str().unwrap()],
    )
    .unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(
        stderr.contains("replaying cli-set for 1s (seed 7)"),
        "{stderr}"
    );
    for expected in [
        "netscenario replay: cli-set",
        "machine: the same as the original",
        "the same ones",
        "result: the replay lost the same packets as the original.",
    ] {
        assert!(stdout.contains(expected), "{expected}\n{stdout}");
    }
    let replayed =
        RunResult::from_json(&std::fs::read_to_string(again.join("result.json")).unwrap()).unwrap();
    assert_eq!(replayed.seed.value, Some(7));
    // The wait of the original is the wait of the replay
    assert_eq!(replayed.timing.settle_ns, 300_000_000);

    // The file without a seed is not the scenario of a run that was given the seed 7 with --seed
    let refused = dir.join("refused");
    let compare = |file: &str| {
        let output = netscenario_in(
            &dir,
            &[
                "replay",
                first_text,
                "--out",
                refused.to_str().unwrap(),
                "--scenario",
                file,
            ],
        )
        .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let (code, stderr) = compare(&file);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("seed: the run had 7, the file has (none)"),
        "{stderr}"
    );
    // The file with the seed of the run in it is the recorded scenario: the replay runs
    let with_seed = LOSSY.replace("name: cli-set", "name: cli-set\nseed: 7");
    std::fs::write(&file, &with_seed).unwrap();
    let (code, stderr) = compare(&file);
    assert_eq!(code, Some(0), "{stderr}");
    let _ = std::fs::remove_dir_all(&refused);
    // A file with another rate: the field is named with both values, and nothing runs
    std::fs::write(&file, with_seed.replace("rate: 2mbit", "rate: 3mbit")).unwrap();
    let (code, stderr) = compare(&file);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("workloads[0].rate: the run had 2mbit, the file has 3mbit"),
        "{stderr}"
    );
    assert!(stderr.contains("nothing is replayed"), "{stderr}");
    assert!(!refused.exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// A scenario.json that was changed after the run is refused by its fingerprint, and (the canary of the comparison) a replay
/// with another recorded seed loses other packets and is told as failed, with exit code 1.
#[test]
fn a_changed_record_is_refused_and_another_seed_is_a_failed_replay() {
    if !can_run("a_changed_record_is_refused_and_another_seed_is_a_failed_replay") {
        return;
    }
    let dir = work_dir("replay-canary");
    let (_, first) = first_run(&dir);
    let first_text = first.to_str().unwrap();
    let out = dir.join("out");
    let replay = || {
        netscenario_in(
            &dir,
            &["replay", first_text, "--out", out.to_str().unwrap()],
        )
        .unwrap()
    };

    let scenario_file = first.join("scenario.json");
    let original = std::fs::read_to_string(&scenario_file).unwrap();
    std::fs::write(
        &scenario_file,
        original.replace("\"duration\": \"1s\"", "\"duration\": \"2s\""),
    )
    .unwrap();
    let output = replay();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("is not the scenario that this run was made with"),
        "{stderr}"
    );
    assert!(!out.exists(), "nothing is replayed");
    std::fs::write(&scenario_file, original).unwrap();

    let result_file = first.join("result.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&result_file).unwrap()).unwrap();
    value["seed"]["value"] = serde_json::json!(8);
    std::fs::write(&result_file, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    let output = replay();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("NOT REPRODUCED"), "{stdout}");
    assert!(stdout.contains("lost only in the replay"), "{stdout}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The three reports of a real run (CLI-02, RPT-02, RPT-03, RPT-04): the Markdown for a CI summary, the `JUnit` with the
/// metrics of the plugin as properties, and the HTML with its charts and nothing to fetch.
#[test]
fn the_reports_of_a_real_run_are_made_from_its_directory() {
    if !can_run("the_reports_of_a_real_run_are_made_from_its_directory") {
        return;
    }
    let dir = work_dir("report");
    let (_, first) = first_run(&dir);
    let first_text = first.to_str().unwrap();
    let report = |format: &str| {
        let output = netscenario_in(&dir, &["report", first_text, "--format", format]).unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let markdown = report("md");
    assert!(
        markdown.starts_with("## netscenario: cli-set\n"),
        "{markdown}"
    );
    assert!(markdown.contains("| Sent | 209 |"), "{markdown}");
    let junit = report("junit");
    assert!(
        junit.contains("<testcase classname=\"cli-set\" name=\"media\""),
        "{junit}"
    );
    assert!(
        junit.contains("name=\"netscenario.cli-set.media.delay_p50_s\""),
        "{junit}"
    );
    assert!(!junit.contains("<failure"), "{junit}");
    let html = report("html");
    assert!(
        html.contains("<h1>netscenario: cli-set</h1>") && html.contains("<svg "),
        "{html}"
    );
    for needle in ["http://", "https://", "<script", "src=", "href=", "url("] {
        assert!(!html.contains(needle), "the page contains '{needle}'");
    }
    // The same directory gives the same bytes
    assert_eq!(report("html"), html);
    let _ = std::fs::remove_dir_all(dir);
}

/// Too few repetitions is a wrong command line: exit code 2 and nothing runs, with or without namespaces.
#[test]
fn too_few_repetitions_are_refused_before_anything_runs() {
    let dir = work_dir("set-few");
    let file = write_scenario(&dir, SCENARIO);
    let out = dir.join("result");
    let output = netscenario_in(
        &dir,
        &[
            "run",
            &file,
            "--out",
            out.to_str().unwrap(),
            "--repetitions",
            "4",
        ],
    )
    .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("repetitions is 4; it is from 5"),
        "{stderr}"
    );
    assert!(!out.exists(), "nothing was run, so nothing was written");
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
    std::thread::sleep(Duration::from_secs(2));
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

// ---- the files a scenario refers to (D-083, D-084) ----

const TABLE_SCENARIO: &str = "version: 0\nname: tables\nduration: 5s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { delay: 100ms, jitter: { amount: 10ms, distribution: table, table: { file: FILE } } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";

fn delay_samples() -> String {
    (0..400).fold(String::new(), |mut text, i| {
        text.push_str(&(20 + i % 37).to_string());
        text.push('\n');
        text
    })
}

/// A file of samples next to the scenario becomes the table in the scenario: `expand` shows it with the fingerprint of the file,
/// and a path that leads out of the directory is refused until `--allow-outside-files` says it is wanted.
#[test]
fn the_file_of_samples_next_to_a_scenario_is_read_and_one_outside_is_refused() -> io::Result<()> {
    let root = work_dir("table-files");
    let directory = root.join("scenario");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("delays.txt"), delay_samples())?;
    std::fs::write(root.join("elsewhere.txt"), delay_samples())?;
    std::fs::write(
        directory.join("near.yaml"),
        TABLE_SCENARIO.replace("FILE", "delays.txt"),
    )?;
    std::fs::write(
        directory.join("far.yaml"),
        TABLE_SCENARIO.replace("FILE", "../elsewhere.txt"),
    )?;

    let valid = netscenario_in(&directory, &["validate", "near.yaml"])?;
    assert_eq!(
        valid.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    let expanded = netscenario_in(&directory, &["expand", "near.yaml", "--json"])?;
    assert_eq!(expanded.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&expanded.stdout).unwrap();
    let table = &json["topology"]["links"][0]["forward"]["jitter"]["table"];
    assert_eq!(table["file"], "delays.txt");
    assert_eq!(table["samples"], 400);
    assert_eq!(table["sha256"].as_str().map(str::len), Some(64));
    assert_eq!(table["values"].as_str().map(str::len), Some(4096 * 4));

    // A path that leaves the directory: refused, with the line, and allowed when asked for
    let refused = netscenario_in(&directory, &["validate", "far.yaml"])?;
    assert_eq!(refused.status.code(), Some(2));
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(
        message.contains("'../elsewhere.txt' is outside the directory of the scenario file"),
        "{message}"
    );
    assert!(message.contains("far.yaml:9:"), "{message}");
    assert!(message.contains("--allow-outside-files"), "{message}");
    let allowed = netscenario_in(
        &directory,
        &["validate", "far.yaml", "--allow-outside-files"],
    )?;
    assert_eq!(
        allowed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );

    // Called from another directory the file is still looked for next to the scenario, not in the working directory
    let from_root = netscenario_in(&root, &["validate", "scenario/near.yaml"])?;
    assert_eq!(
        from_root.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&from_root.stderr)
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The expanded scenario is whole: it holds the table, so reading it needs no file (a recorded scenario replays).
#[test]
fn the_expanded_scenario_needs_no_file() -> io::Result<()> {
    let root = work_dir("table-expanded");
    std::fs::write(root.join("delays.txt"), delay_samples())?;
    std::fs::write(
        root.join("near.yaml"),
        TABLE_SCENARIO.replace("FILE", "delays.txt"),
    )?;
    let expanded = netscenario_in(&root, &["expand", "near.yaml"])?;
    assert_eq!(expanded.status.code(), Some(0));
    let again = root.join("again");
    std::fs::create_dir_all(&again)?;
    std::fs::write(again.join("whole.yaml"), &expanded.stdout)?;
    let valid = netscenario_in(&again, &["validate", "whole.yaml"])?;
    assert_eq!(
        valid.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    // Canary: the same scenario without its entries does need the file, and says so
    std::fs::write(
        again.join("bare.yaml"),
        TABLE_SCENARIO.replace("FILE", "delays.txt"),
    )?;
    let missing = netscenario_in(&again, &["validate", "bare.yaml"])?;
    assert_eq!(missing.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("cannot read 'delays.txt': no such file")
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

// ---- parameters, templates and includes (SCN-05, D-093) ----

const PARAMETERS: &str = "version: 0
name: parameters
duration: 5s
params:
  rtt: 40ms
  flows:
topology:
  nodes: [client, server]
  links:
    - include: parts/access.yaml
      with: { id: access, delay: \"{rtt / 2}\" }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: \"{2mbit / flows}\", packet-size: 1200b }
";

const ACCESS: &str = "params:
  id:
  delay: 10ms
content:
  id: \"{id}\"
  between: [client, server]
  both: { delay: \"{delay}\" }
";

#[test]
fn a_parameter_is_given_on_the_command_line_and_the_expanded_scenario_is_shown() -> io::Result<()> {
    let dir = work_dir("params");
    std::fs::create_dir_all(dir.join("parts"))?;
    std::fs::write(dir.join("parts/access.yaml"), ACCESS)?;
    std::fs::write(dir.join("scenario.yaml"), PARAMETERS)?;
    // It must be given
    let missing = netscenario_in(&dir, &["validate", "scenario.yaml"])?;
    assert_eq!(missing.status.code(), Some(2));
    let text = String::from_utf8_lossy(&missing.stderr);
    assert!(
        text.contains("this scenario needs a value for `flows`"),
        "{text}"
    );
    assert!(text.contains("--param flows=<value>"), "{text}");
    // The long form has the values in it, and the record of the parameters
    let shown = netscenario_in(
        &dir,
        &[
            "expand",
            "scenario.yaml",
            "--param",
            "flows=4",
            "--param=rtt=100ms",
        ],
    )?;
    assert_eq!(
        shown.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let text = String::from_utf8_lossy(&shown.stdout);
    assert!(text.contains("delay: 50ms"), "{text}");
    assert!(text.contains("rate: 500kbit"), "{text}");
    assert!(
        text.contains("flows: '4'") || text.contains("flows: \"4\""),
        "{text}"
    );
    // A wrong value, an unknown name and a word that is no name=value are refused with the way out
    let wrong = netscenario_in(
        &dir,
        &[
            "validate",
            "scenario.yaml",
            "--param",
            "flows=4",
            "--param",
            "rtt=3mbit",
        ],
    )?;
    assert_eq!(wrong.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("is a duration, and a rate was given"));
    let unknown = netscenario_in(
        &dir,
        &[
            "validate",
            "scenario.yaml",
            "--param",
            "flows=4",
            "--param",
            "rt=1ms",
        ],
    )?;
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("did you mean `rtt`?"));
    let bare = netscenario_in(&dir, &["validate", "scenario.yaml", "--param", "flows"])?;
    assert_eq!(bare.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bare.stderr).contains("is not name=value"));
    // Canary: the same command line without the parameter is the one that failed above
    let given = netscenario_in(&dir, &["validate", "scenario.yaml", "--param", "flows=4"])?;
    assert_eq!(
        given.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&given.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

#[test]
fn a_fault_in_an_included_file_is_shown_in_that_file_with_the_place_of_the_include()
-> io::Result<()> {
    let dir = work_dir("include-fault");
    std::fs::create_dir_all(dir.join("parts"))?;
    std::fs::write(
        dir.join("parts/access.yaml"),
        ACCESS.replace("{delay}", "{delay / 0}"),
    )?;
    std::fs::write(dir.join("scenario.yaml"), PARAMETERS)?;
    let output = netscenario_in(&dir, &["validate", "scenario.yaml", "--param", "flows=2"])?;
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("error: division by zero"), "{text}");
    assert!(text.contains("--> parts/access.yaml:7:"), "{text}");
    assert!(
        text.contains("= used from scenario.yaml:10:7 (file 'parts/access.yaml')"),
        "{text}"
    );
    // With --json the file and the chain are in the problem
    let json = netscenario_in(
        &dir,
        &["validate", "scenario.yaml", "--param", "flows=2", "--json"],
    )?;
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let problem = &value["problems"][0];
    assert_eq!(problem["file"], "parts/access.yaml");
    assert!(
        problem["chain"][0]
            .as_str()
            .unwrap()
            .starts_with("used from scenario.yaml:10:7")
    );
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

// ---- the checks of the file (ASR-07, D-094) ----

/// The same scenario with a check that holds and one that cannot (the delay is that of the link, 20 ms).
fn checked(bound: &str) -> String {
    format!(
        "{SCENARIO}expect:
  checks:
    - {{ name: nothing is lost, metric: media.loss_ratio, value: \"<= 0%\" }}
    - {{ name: the delay, metric: media.delay.p99, value: \"< {bound}\" }}
"
    )
}

#[test]
fn a_check_of_the_file_that_holds_exits_with_zero_and_one_that_does_not_exits_with_one() {
    if !can_run(
        "a_check_of_the_file_that_holds_exits_with_zero_and_one_that_does_not_exits_with_one",
    ) {
        return;
    }
    let dir = work_dir("expect-run");
    let out = dir.join("result");
    for (bound, code, word) in [("1s", 0, "ok  "), ("1ns", 1, "FAIL")] {
        let file = write_scenario(&dir, &checked(bound));
        let output = netscenario_in(&dir, &["run", &file, "--out", out.to_str().unwrap()]).unwrap();
        let (stdout, stderr) = (
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert_eq!(
            output.status.code(),
            Some(code),
            "{bound}\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains(&format!("{word} the delay")),
            "{bound}\n{stdout}"
        );
        let result =
            RunResult::from_json(&std::fs::read_to_string(out.join("result.json")).unwrap())
                .unwrap();
        let expect = result.expect.expect("the verdict is in the result");
        assert_eq!(expect.passed, code == 0);
        // The reports read the same verdict from the directory
        let report = netscenario_in(
            &dir,
            &["report", out.to_str().unwrap(), "--format", "junit"],
        )
        .unwrap();
        let junit = String::from_utf8_lossy(&report.stdout);
        assert_eq!(
            junit.matches("netscenario.CheckFailed").count(),
            usize::from(code == 1),
            "{junit}"
        );
        assert_eq!(
            report.status.code(),
            Some(0),
            "a report is made whatever it says"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_whose_checks_need_a_set_is_run_as_a_set_and_refuses_fewer_runs() {
    let dir = work_dir("expect-set");
    let text = format!(
        "{SCENARIO}expect:\n  repetitions: 6\n  checks:\n    - {{ metric: media.loss_ratio, ci95-high: \"< 5%\" }}\n"
    );
    let file = write_scenario(&dir, &text);
    let out = dir.join("result");
    // Fewer runs than the file needs, and a single run for a set: a wrong command line, whatever the machine
    let output = netscenario_in(
        &dir,
        &[
            "run",
            &file,
            "--out",
            out.to_str().unwrap(),
            "--repetitions",
            "5",
        ],
    )
    .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("at least 6 runs"));
    let one = write_scenario(
        &dir,
        &format!(
            "{SCENARIO}expect:\n  checks:\n    - {{ metric: media.loss_ratio, value: \"< 5%\" }}\n"
        ),
    );
    let output = netscenario_in(
        &dir,
        &[
            "run",
            &one,
            "--out",
            out.to_str().unwrap(),
            "--repetitions",
            "5",
        ],
    )
    .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("judge one run"));
    assert!(!out.exists(), "nothing was run, so nothing was written");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_set_judged_by_the_file_exits_with_the_verdict() {
    if !can_run("a_set_judged_by_the_file_exits_with_the_verdict") {
        return;
    }
    let dir = work_dir("expect-set-run");
    let out = dir.join("result");
    // Five repetitions are the least a set may have; one second each is enough for a verdict (the exit code follows `passed`).
    // Two sets of five runs of two seconds were the slowest test of the command line (33 s in a full run)
    let short = SCENARIO.replace("duration: 2s", "duration: 1s");
    for (bound, code) in [("5%", 0), ("0%", 1)] {
        let text = format!(
            "{short}expect:\n  repetitions: 5\n  checks:\n    - {{ metric: media.loss_ratio, max: \"< {bound}\" }}\n"
        );
        let file = write_scenario(&dir, &text);
        let output = netscenario_in(&dir, &["run", &file, "--out", out.to_str().unwrap()]).unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(code), "{bound}\n{stderr}");
        let set =
            RunSetResult::from_json(&std::fs::read_to_string(out.join("runset.json")).unwrap())
                .unwrap();
        assert_eq!(set.repetitions, 5);
        assert_eq!(
            set.expect.expect("the verdict is in the set").passed,
            code == 0
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}
