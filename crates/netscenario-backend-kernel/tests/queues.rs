//! Queues in front of the rate (LNK-09, PRB-10, D-079, D-082): a link with a `queue` is built through the fabric, its queue drops
//! as the discipline says, the delay of the link comes after it, and the kernel's counters of the queue are in the result. These
//! need `netem` (the first stage of the tree is a `netem` with a delay of 1 microsecond), so they are judged by the CI where the
//! machine has none.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

mod support;

use std::path::Path;

use netscenario_core::{RunOptions, RunResult, run};
use netscenario_model::Scenario;
use support::{agent_exe, assert_nothing_left, backend, namespaces_available, work_dir};

fn options(dir: &Path) -> RunOptions {
    RunOptions::new(agent_exe().unwrap(), dir)
}

/// Two nodes, a link of 2 mbit with `queue` and `extra` in its forward direction, and a flow of 3 mbit with payloads of 1,000
/// bytes (375 packets a second) for `seconds`.
fn overloaded(queue: &str, extra: &str, seconds: u32) -> Scenario {
    Scenario::from_yaml(
        &format!(
            "version: 0\nname: queues\nseed: 3\nduration: {seconds}s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n      forward: {{ rate: 2mbit, {extra}queue: {queue} }}\nworkloads:\n  - {{ id: media, kind: udp-cbr, from: client, to: server, rate: 3mbit, packet-size: 1000b }}\n"
        ),
        "queues.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn run_it(name: &str, scenario: &Scenario) -> (RunResult, std::path::PathBuf) {
    let dir = work_dir(name);
    let mut backend = backend(|_| {});
    let result = run(&mut backend, scenario, &options(&dir)).unwrap();
    assert_nothing_left(backend.run_id(), "after the run");
    (result, dir)
}

/// The tail drop of a `fifo` of 50 packets in front of 2 mbit, offered 3 mbit for 4 s (1,500 packets).
///
/// The model: a frame is 1,042 bytes (1,000 of payload, 8 of UDP, 20 of IP, 14 of Ethernet) and `htb` counts frames, so the link
/// serves 2,000,000 / (1,042 x 8) = 239.9 packets a second. In 4 s it serves 959.6, and the 50 packets that wait when the
/// sender stops are served after that, so 1,009.6 packets get through and 490 are dropped (32.7 %). The bounds leave 10 packets
/// for the edges of the burst (1,600 bytes, 1.5 packets) and of the start.
///
/// The queue is full all the time, so a packet that is accepted waits for 50 frames and is served itself, 51 times 4.168 ms
/// = 212.6 ms; the largest delay is between 200 ms (50 frames less the burst) and 225 ms.
///
/// The sender is not held back by the queue (the first stage is a `netem` with a delay, D-079): it sends all 1,500 packets at
/// its pace, and the queue drops what does not fit. A sender that is pushed back would send fewer packets, or late.
#[test]
fn a_fifo_drops_the_tail_and_the_delay_is_the_queue_of_frames() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (result, dir) = run_it("queues-fifo", &overloaded("{ limit: 50p }", "", 4));
    let media = &result.workloads[0];
    assert_eq!(media.stats.sent, 1500, "{:?}", media.sender);
    assert_eq!(media.sender.send_errors, 0);
    assert!(
        media.sender.max_lateness_ns < 50_000_000,
        "the sender was held back: {:?}",
        media.sender
    );
    assert!(
        (1000..=1020).contains(&media.stats.received),
        "{} of 1500 got through, the model says 1,010",
        media.stats.received
    );
    let delay = media.stats.delay.as_ref().unwrap();
    assert!(
        (200_000_000..=225_000_000).contains(&delay.max_ns),
        "the largest delay is {} ns, the model says 212.6 ms",
        delay.max_ns
    );
    // What the kernel counted in the queue is what the run lost, and the result names the build
    let [queue] = result.queues.as_slice() else {
        panic!("{:?}", result.queues);
    };
    assert_eq!(
        (
            queue.link.as_str(),
            queue.direction.as_str(),
            queue.build.as_str()
        ),
        ("access", "forward", "fabric")
    );
    assert_eq!(
        (queue.discipline.as_str(), queue.kind.as_str()),
        ("fifo", "pfifo")
    );
    assert_eq!(queue.drops, media.stats.lost, "{queue:?}");
    assert_eq!(queue.packets, media.stats.received, "{queue:?}");
    assert_eq!(queue.backlog_packets, 0, "the queue is empty at the end");
    let _ = std::fs::remove_dir_all(dir);
}

/// The delay of the link comes after the queue: with a delay of 20 ms behind it no packet is faster than the delay and the
/// time it takes to send the first frame, 20 + 4.168 ms (the physical lower bound), and the queue still drops.
#[test]
fn the_delay_of_the_link_comes_after_the_queue() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let (result, dir) = run_it(
        "queues-delay",
        &overloaded("{ limit: 50p }", "delay: 20ms, ", 4),
    );
    let media = &result.workloads[0];
    let delay = media.stats.delay.as_ref().unwrap();
    assert!(
        delay.min_ns >= 24_100_000,
        "the smallest delay is {} ns, no packet can be faster than 24.168 ms",
        delay.min_ns
    );
    // The same queue, the same drops: the delay behind the queue does not change what the queue drops
    assert!(
        (1000..=1020).contains(&media.stats.received),
        "{} got through",
        media.stats.received
    );
    assert_eq!(result.queues[0].drops, media.stats.lost);
    let _ = std::fs::remove_dir_all(dir);
}

/// `CoDel`, `FQ-CoDel` and `PIE` each drop under the overload; the counters of the kernel agree with the lost packets, the discipline
/// is named as the kernel names it, and the run is accounted for. How well each keeps the delay down needs a flow that answers
/// to drops, which comes with the TCP workload.
#[test]
fn each_discipline_drops_under_an_overload_and_the_counters_agree_with_the_lost_packets() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    for (discipline, kind) in [("codel", "codel"), ("fq-codel", "fq_codel"), ("pie", "pie")] {
        let (result, dir) = run_it(
            &format!("queues-{discipline}"),
            &overloaded(
                &format!("{{ discipline: {discipline}, limit: 200p }}"),
                "",
                4,
            ),
        );
        let media = &result.workloads[0];
        let [queue] = result.queues.as_slice() else {
            panic!("{discipline}: {:?}", result.queues);
        };
        assert_eq!(queue.kind, kind, "{discipline}");
        assert!(
            media.stats.lost > 0,
            "{discipline} dropped nothing under an overload"
        );
        assert_eq!(
            queue.drops, media.stats.lost,
            "{discipline}: the kernel counted other drops than the run lost: {queue:?}"
        );
        assert_eq!(
            media.stats.sent,
            media.stats.received + media.stats.lost,
            "{discipline}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A link with a queue goes down and comes up like any link: the sender is refused while it is down, the packets before and after
/// it arrive, and the queue is as it was (the interfaces of the nodes go down, the fabric stays up, D-079).
#[test]
fn a_link_with_a_queue_goes_down_and_comes_back() {
    if !namespaces_available() || agent_exe().is_none() {
        return;
    }
    let mut scenario = Scenario::from_yaml(
        "version: 0\nname: queues-down\nduration: 4s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n      both: { rate: 10mbit, queue: { limit: 100p } }\nworkloads:\n  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1000b }\ntimeline:\n  - { at: 1s, link: access, down: true }\n  - { at: 2s, link: access, up: true }\n",
        "queues-down.yaml",
    )
    .unwrap();
    scenario.seed = Some(1);
    let (result, dir) = run_it("queues-down", &scenario);
    let media = &result.workloads[0];
    // 125 packets a second for 4 s; about a second of them is refused, the rest arrives
    assert!(media.sender.send_errors >= 100, "{:?}", media.sender);
    assert!(
        media.stats.received + 10 >= media.stats.sent,
        "{} of {} arrived",
        media.stats.received,
        media.stats.sent
    );
    assert!(media.stats.received > 340, "{}", media.stats.received);
    let _ = std::fs::remove_dir_all(dir);
}
