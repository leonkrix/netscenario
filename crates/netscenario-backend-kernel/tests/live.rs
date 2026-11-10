//! A change of a link from test code while the run goes (TML-05, D-096): the code asks, the run holds the sender before its
//! next packet, does the step between two packets, and records it so that a replay does it at the same packet.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use netscenario_core::{LiveChange, Recorded, RunOptions, RunResult, channel, run};
use netscenario_model::{Change, Name, Scenario};
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

fn scenario() -> Scenario {
    let mut scenario = Scenario::from_yaml(
        "version: 0\nname: live-test\nseed: 5\nduration: 2s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n      forward: { delay: 5ms }\nworkloads:\n  - { id: media, kind: udp-cbr, from: client, to: server, rate: 3mbit, packet-size: 1000b }\n",
        "live-test.yaml",
    )
    .unwrap();
    scenario.measure.packets = true;
    scenario
}

fn access() -> Name {
    Name::new("access").unwrap()
}

fn loss_30() -> Change {
    serde_json::from_str(r#"{"forward":{"loss":{"model":"random","p":"30%"}}}"#).unwrap()
}

fn lost(dir: &std::path::Path) -> Vec<u64> {
    std::fs::read_to_string(dir.join("packets-media.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|packet| packet["lost"] == true)
        .map(|packet| packet["sequence"].as_u64().unwrap())
        .collect()
}

fn options(dir: &std::path::Path) -> RunOptions {
    let mut options = RunOptions::new(agent_exe().unwrap(), dir);
    options.settle = Some(Duration::from_secs(1));
    options
}

/// Runs the scenario, and has `act` change the link when the measured phase is `after` old.
fn live_run(
    label: &str,
    after: Duration,
    act: impl FnOnce(&netscenario_core::LiveHandle) + Send + 'static,
) -> (RunResult, std::path::PathBuf) {
    let dir = work_dir(label);
    let (handle, inbox) = channel(vec![access()]);
    let thread = std::thread::spawn(move || {
        handle.wait_until(after).unwrap();
        act(&handle);
    });
    let mut backend = backend(|_| {});
    let options = options(&dir).with_live(inbox);
    let result = run(&mut backend, &scenario(), &options).unwrap();
    thread.join().unwrap();
    assert_nothing_left(backend.run_id(), "after the run");
    (result, dir)
}

/// The link has no loss for a second and 30 % from the moment the code asks: the step is done before a packet (about 375 a
/// second, so near packet 375), no packet before it is lost, a second of the stream is lost by 30 %, the record says that
/// test code made it, and the run says it cannot be repeated by itself. A replay of the directory does the change at the same
/// packet and loses the same packets.
#[test]
fn a_change_from_test_code_lands_between_two_packets_and_a_replay_loses_the_same_packets() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (result, dir) = live_run("live-a", Duration::from_secs(1), |handle| {
        let records = handle.change(LiveChange::set(access(), loss_30())).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].live && records[0].packet.is_some());
    });
    assert_eq!(result.events.len(), 1);
    let step = &result.events[0];
    assert!(step.live && !step.from_model);
    let packet = step
        .packet
        .expect("a sender sends into the direction, so the step is at a packet");
    assert!((300..500).contains(&packet), "the step met packet {packet}");
    assert_eq!(
        (
            step.kind.as_str(),
            step.link.as_str(),
            step.direction.as_deref()
        ),
        ("set", "access", Some("forward"))
    );
    let live_lost = lost(&dir);
    assert!(
        live_lost.iter().all(|sequence| *sequence >= packet),
        "{live_lost:?}"
    );
    // 30 % of the 375 packets after the step, and the first second has none
    assert!(
        (60..170).contains(&live_lost.len()),
        "{} lost",
        live_lost.len()
    );
    assert!(!result.seed.loss_reproducible);
    assert!(
        result
            .seed
            .not_reproducible_because
            .iter()
            .any(|why| why.contains("code")),
        "{:?}",
        result.seed
    );
    assert_eq!(result.scenario.live_links, ["access"]);

    // The recorded scenario holds the change as an event, and a replay does it at the same packet
    let recorded = Recorded::load(&dir).unwrap();
    assert_eq!(recorded.scenario_to_replay().timeline.len(), 1);
    let replay_dir = work_dir("live-a-replay");
    let mut second = backend(|_| {});
    let again = run(
        &mut second,
        &recorded.scenario_to_replay(),
        &options(&replay_dir),
    )
    .unwrap();
    assert_eq!(
        again.events[0].packet,
        Some(packet),
        "the replay did the step at another packet"
    );
    assert!(!again.events[0].live);
    assert_eq!(
        lost(&replay_dir),
        live_lost,
        "the replay lost other packets"
    );

    // Canary: a change ten milliseconds later (4 packets) is another run, so the comparison above can fail
    let mut moved = recorded.scenario_to_replay();
    let at = moved.timeline[0].at.as_nanos() + 10_000_000;
    moved.timeline[0].at = netscenario_model::Duration::from_nanos(at);
    let moved_dir = work_dir("live-a-moved");
    let mut third = backend(|_| {});
    run(&mut third, &moved, &options(&moved_dir)).unwrap();
    assert_ne!(
        lost(&moved_dir),
        live_lost,
        "a step four packets later lost the same packets"
    );
    for d in [dir, replay_dir, moved_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

fn change_of(json: &str) -> Change {
    serde_json::from_str(json).unwrap()
}

/// A step of the file that comes after a change from test code keeps what the code set: the loss that the code switched on
/// at 1 s stays through the step of the file at 1.5 s that changes the delay. If the run worked the step out from the file
/// alone, the loss would go at 1.5 s, and the replay (which works out all the steps in order) would lose other packets.
#[test]
fn a_step_of_the_file_after_a_live_change_keeps_what_the_code_set() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let mut with_step = scenario();
    with_step.timeline = serde_yaml_ng_free_event();
    let dir = work_dir("live-b");
    let (handle, inbox) = channel(vec![access()]);
    let thread = std::thread::spawn(move || {
        handle.wait_until(Duration::from_secs(1)).unwrap();
        handle.change(LiveChange::set(access(), loss_30())).unwrap();
    });
    let mut first = backend(|_| {});
    let result = run(&mut first, &with_step, &options(&dir).with_live(inbox)).unwrap();
    thread.join().unwrap();
    // The planned step is the first event of the file; the live one has its own count
    assert_eq!(result.events.len(), 2);
    let live_lost = lost(&dir);
    let packet = result
        .events
        .iter()
        .find(|e| e.live)
        .unwrap()
        .packet
        .unwrap();
    // The loss goes on to the end: lost packets are also among the last ones (after 1.5 s, packet 560 and up)
    assert!(
        live_lost.iter().any(|sequence| *sequence > 600),
        "{live_lost:?}"
    );
    let recorded = Recorded::load(&dir).unwrap();
    let replay_dir = work_dir("live-b-replay");
    let mut second = backend(|_| {});
    let again = run(
        &mut second,
        &recorded.scenario_to_replay(),
        &options(&replay_dir),
    )
    .unwrap();
    assert_eq!(
        again
            .events
            .iter()
            .filter(|e| e.packet == Some(packet))
            .count(),
        1
    );
    assert_eq!(lost(&replay_dir), live_lost);
    for d in [dir, replay_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

fn serde_yaml_ng_free_event() -> Vec<netscenario_model::Event> {
    let mut event = netscenario_model::Event::new(
        netscenario_model::Duration::from_nanos(1_500_000_000),
        access(),
    );
    event.set = Some(change_of(r#"{"forward":{"delay":"20ms"}}"#));
    vec![event]
}

/// What the code may not do is refused to the code, before the link is touched, and the run goes on: a link that was not
/// declared, a change that is not valid for the state the link is in (a jitter with no delay), and a jitter whose distribution
/// the queue could not go back to (D-059). A refusal names what it refuses. Nothing is recorded for a change that was refused.
#[test]
fn a_change_that_is_not_allowed_is_refused_to_the_code_and_the_run_goes_on() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (result, dir) = live_run("live-c", Duration::from_millis(200), |handle| {
        let other = Name::new("elsewhere").unwrap();
        let error = handle
            .change(LiveChange::set(other, loss_30()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("was not declared live"), "{error}");
        let error = handle
            .change(LiveChange::set(
                access(),
                change_of(r#"{"forward":{"delay":null,"jitter":{"amount":"5ms"}}}"#),
            ))
            .unwrap_err();
        assert!(
            matches!(error, netscenario_core::Error::InvalidScenario(_)),
            "{error}"
        );
        // A normal jitter is allowed once, and a uniform one after it is not
        handle
            .change(LiveChange::set(
                access(),
                change_of(r#"{"forward":{"delay":"20ms","jitter":{"amount":"5ms","distribution":"normal"}}}"#),
            ))
            .unwrap();
        let error = handle
            .change(LiveChange::set(
                access(),
                change_of(r#"{"forward":{"jitter":{"amount":"5ms"}}}"#),
            ))
            .unwrap_err();
        assert!(
            matches!(error, netscenario_core::Error::Capability(_)),
            "{error}"
        );
        assert!(error.to_string().contains("distribution"), "{error}");
    });
    assert_eq!(
        result.events.len(),
        1,
        "only the change that was allowed is recorded"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A `set` of both directions is two steps, each held before the next packet of the sender that sends into it, or done by the
/// clock where no workload does (here: the reverse direction); the link going down and up is one step each; and the code asking
/// after the workloads are over gets an error and not a hang.
#[test]
fn both_directions_and_down_and_up_are_steps_and_a_late_request_is_refused() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (result, dir) = live_run("live-d", Duration::from_millis(300), |handle| {
        let records = handle
            .change(LiveChange::set(
                access(),
                change_of(r#"{"both":{"delay":"10ms"}}"#),
            ))
            .unwrap();
        let ways: Vec<(&str, bool)> = records
            .iter()
            .map(|r| (r.direction.as_deref().unwrap(), r.packet.is_some()))
            .collect();
        // The workload sends forward only: that step is at a packet, the reverse one by the clock
        assert_eq!(ways, [("forward", true), ("reverse", false)]);
        let down = handle.change(LiveChange::down(access())).unwrap();
        assert_eq!((down.len(), down[0].kind.as_str()), (1, "down"));
        assert!(down[0].packet.is_some());
        std::thread::sleep(Duration::from_millis(100));
        let up = handle.change(LiveChange::up(access())).unwrap();
        assert_eq!(up[0].kind, "up");
        // After the end of the measured phase nothing is taken
        handle
            .wait_until(Duration::from_millis(2_100))
            .unwrap_or(());
        assert!(handle.change(LiveChange::up(access())).is_err());
    });
    assert_eq!(result.events.iter().filter(|e| e.live).count(), 4);
    let _ = std::fs::remove_dir_all(dir);
}

/// A run that fails does not leave the code waiting: after `run` has returned, with its options still alive, a request is answered at
/// once with an error and a wait for a moment of the run ends.
#[test]
fn a_run_that_fails_does_not_leave_the_code_waiting() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let dir = work_dir("live-e");
    let (handle, inbox) = channel(vec![access()]);
    // The agent is not there, so the run fails after the network is built
    let options = RunOptions::new(dir.join("no-such-agent"), &dir).with_live(inbox);
    let mut failing = backend(|_| {});
    let error = run(&mut failing, &scenario(), &options).unwrap_err();
    assert!(error.to_string().contains("agent"), "{error}");
    let began = std::time::Instant::now();
    assert!(handle.change(LiveChange::down(access())).is_err());
    assert!(handle.wait_until(Duration::from_secs(3_600)).is_err());
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "the code waited for a run that is over"
    );
    drop(options);
    let _ = std::fs::remove_dir_all(dir);
}
