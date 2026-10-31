//! Spike S11: how exactly can a timeline be applied to a running `netem`?
//!
//! Runs inside the namespace that holds `v0` (10.9.0.1); `recv` runs in the one that holds `v1` (10.9.0.2).
//! Three questions:
//!   S11.1 How long does one change take until it is applied (a `tc` process each time, a persistent
//!         `tc -batch -`), with the machine idle and busy?
//!   S11.2 Does an event land on the packet it was meant for? A change that the sender asks for at a packet
//!         ("barrier") against one that a timer applies at the moment of that packet ("clock").
//!   S11.3 Does the same seed give the same lost packets when the parameters change during the run?
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::UdpSocket;
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use rustix::thread::clock_nanosleep_absolute;
use rustix::time::{clock_gettime, ClockId, Timespec};

const PACKET: usize = 64;
const SECOND: u64 = 1_000_000_000;

fn now_ns() -> u64 {
    let t = clock_gettime(ClockId::Monotonic);
    t.tv_sec as u64 * SECOND + t.tv_nsec as u64
}

fn sleep_until(ns: u64) {
    let t = Timespec { tv_sec: (ns / SECOND) as _, tv_nsec: (ns % SECOND) as _ };
    // An interrupted sleep returns early: sleep again until the moment has come
    while now_ns() < ns {
        let _ = clock_nanosleep_absolute(ClockId::Monotonic, &t);
    }
}

fn report(kind: &str, id: &str, text: String) {
    println!("{kind} {id:<7} {text}");
}

// ---------------------------------------------------------------------------------------------------------
// Appliers: how a change gets to the kernel
// ---------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// One `tc` process for each change
    Process,
    /// One `tc -batch -` that lives for the whole run; `qdisc show` after the change is the acknowledgement
    Batch,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Process => "a tc process for each change",
            Kind::Batch => "a persistent tc -batch -",
        }
    }
}

enum Applier {
    Process,
    Batch { child: Child, stdin: ChildStdin, lines: Receiver<String> },
}

/// How long `tc -batch -` may take to answer before the spike says it does not
const ANSWER_WITHIN: Duration = Duration::from_secs(5);

impl Applier {
    fn new(kind: Kind) -> Applier {
        Self::try_new(kind).unwrap_or_else(|error| panic!("{error}"))
    }

    fn try_new(kind: Kind) -> Result<Applier, String> {
        match kind {
            Kind::Process => Ok(Applier::Process),
            Kind::Batch => {
                // `stdbuf -oL` makes the answer of tc leave at the end of each line even where a version of tc
                // would hold it back in a pipe
                let mut command = if Command::new("stdbuf").arg("--version").output().is_ok() {
                    let mut c = Command::new("stdbuf");
                    c.args(["-oL", "tc", "-batch", "-"]);
                    c
                } else {
                    let mut c = Command::new("tc");
                    c.args(["-batch", "-"]);
                    c
                };
                let mut child = command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .map_err(|e| format!("cannot start tc -batch: {e}"))?;
                let stdin = child.stdin.take().unwrap();
                let stdout = child.stdout.take().unwrap();
                let (tx, lines) = mpsc::channel();
                thread::spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                });
                let mut applier = Applier::Batch { child, stdin, lines };
                // The first line pays for the start of the process; a run should not
                applier.try_apply("delay 1ms limit 10000")?;
                Ok(applier)
            }
        }
    }

    /// Returns when the kernel has the change
    fn apply(&mut self, params: &str) {
        self.try_apply(params).unwrap_or_else(|error| panic!("{error}"));
    }

    fn try_apply(&mut self, params: &str) -> Result<(), String> {
        match self {
            Applier::Process => {
                let status = Command::new("tc")
                    .args(["qdisc", "change", "dev", "v0", "root", "netem"])
                    .args(params.split_whitespace())
                    .status()
                    .map_err(|e| format!("tc does not run: {e}"))?;
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("tc refused: {params}"))
                }
            }
            Applier::Batch { stdin, lines, .. } => {
                // `show ... root` would fail the second time (tc keeps the parent between lines of a batch)
                write!(stdin, "qdisc change dev v0 root netem {params}\nqdisc show dev v0\n").map_err(|e| format!("tc -batch is gone: {e}"))?;
                stdin.flush().map_err(|e| format!("tc -batch is gone: {e}"))?;
                loop {
                    match lines.recv_timeout(ANSWER_WITHIN) {
                        Ok(line) if line.contains("netem") => return Ok(()),
                        Ok(line) => return Err(format!("tc -batch answered {line:?} to {params}")),
                        Err(RecvTimeoutError::Timeout) => {
                            let version = Command::new("tc").arg("-V").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
                            return Err(format!("tc -batch gave no answer within {} s to {params} ({version})", ANSWER_WITHIN.as_secs()));
                        }
                        Err(RecvTimeoutError::Disconnected) => return Err(format!("tc -batch ended without an answer to {params}")),
                    }
                }
            }
        }
    }
}

impl Drop for Applier {
    fn drop(&mut self) {
        if let Applier::Batch { child, .. } = self {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The kind of applier that the runs use for "a persistent tc": the batch if it answers, else a process each time
static BATCH_KIND: OnceLock<Kind> = OnceLock::new();

fn batch_kind() -> Kind {
    *BATCH_KIND.get().unwrap_or(&Kind::Batch)
}

fn tc(args: &[&str]) {
    let status = Command::new("tc").args(args).status().expect("tc runs");
    assert!(status.success(), "tc {args:?} failed");
}

// ---------------------------------------------------------------------------------------------------------
// Load: a busy machine
// ---------------------------------------------------------------------------------------------------------

struct Busy(Arc<AtomicBool>);

impl Busy {
    fn start(threads: usize) -> Busy {
        let stop = Arc::new(AtomicBool::new(false));
        for _ in 0..threads {
            let stop = stop.clone();
            thread::spawn(move || {
                let mut x = 1u64;
                while !stop.load(Ordering::Relaxed) {
                    for _ in 0..10_000 {
                        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
                    }
                }
            });
        }
        Busy(stop)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------------------------------------
// A run: one stream of packets, a list of events
// ---------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    /// The sender asks for the change at the slot of the packet, and waits for the answer before it sends
    BarrierAtSlot,
    /// The sender asks right after the previous packet, and only waits (if at all) at the slot of the packet
    BarrierAhead,
    /// A timer applies the change at the moment of the packet, whatever the sender is doing
    Clock,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::BarrierAtSlot => "barrier at the slot",
            Mode::BarrierAhead => "barrier asked ahead",
            Mode::Clock => "clock",
        }
    }
}

struct Plan {
    initial: &'static str,
    /// (packet number, parameters of the `netem change`)
    events: Vec<(u64, &'static str)>,
    packets: u64,
    gap_ns: u64,
    seed: u64,
}

#[derive(Default)]
struct Outcome {
    /// Packets that arrived: (sequence, sent, received) in CLOCK_MONOTONIC nanoseconds
    arrived: Vec<(u64, u64, u64)>,
    /// How much later than its slot each packet was sent
    lateness: Vec<u64>,
    /// Barrier: the time from the question to the answer
    barrier_wait: Vec<u64>,
    /// Clock: how late the timer woke (negative: early), and how long the change took
    timer_late: Vec<i64>,
    apply_took: Vec<u64>,
}

fn run_once(plan: &Plan, mode: Mode, kind: Kind, ns_pid: &str) -> Outcome {
    // The applier warms up with a change of its own, so it is made first; the parameters of the run and the
    // seed come after it, or the warm-up would overwrite them
    // A fresh qdisc: netem keeps some settings across a change (D-048)
    let _ = Command::new("tc").args(["qdisc", "del", "dev", "v0", "root"]).output();
    tc(&["qdisc", "add", "dev", "v0", "root", "netem", "limit", "10000"]);
    let applier = Applier::new(kind);
    let seed = plan.seed.to_string();
    let initial: Vec<&str> = plan.initial.split_whitespace().collect();
    let mut args = vec!["qdisc", "replace", "dev", "v0", "root", "netem", "limit", "10000"];
    args.extend(initial);
    args.extend(["seed", &seed]);
    tc(&args);

    let out = format!("{}/spike-timeline-{}.recv", std::env::temp_dir().display(), std::process::id());
    let mut receiver = Command::new("nsenter")
        .args(["-t", ns_pid, "-n"])
        .arg(std::env::current_exe().unwrap())
        .args(["recv", &out])
        .stdout(Stdio::piped())
        .spawn()
        .expect("nsenter starts the receiver");
    let mut ready = String::new();
    BufReader::new(receiver.stdout.take().unwrap()).read_line(&mut ready).unwrap();
    assert_eq!(ready.trim(), "ready", "the receiver did not start");

    let sock = UdpSocket::bind("10.9.0.1:0").unwrap();
    sock.connect("10.9.0.2:5000").unwrap();
    let t0 = now_ns() + 100_000_000;
    let mut outcome = Outcome::default();
    let events = plan.events.clone();

    // The party that applies the changes
    let (mut asker, answerer) = UnixStream::pair().unwrap();
    let helper = {
        let events = events.clone();
        let gap = plan.gap_ns;
        thread::spawn(move || {
            let mut applier = applier;
            let mut answerer = answerer;
            let mut took = Vec::new();
            let mut late = Vec::new();
            let mut one = [0u8; 1];
            for (packet, params) in events {
                if mode == Mode::Clock {
                    let at = t0 + packet * gap;
                    sleep_until(at);
                    let start = now_ns();
                    late.push(start as i64 - at as i64);
                    applier.apply(params);
                    took.push(now_ns() - start);
                } else {
                    answerer.read_exact(&mut one).unwrap();
                    let start = now_ns();
                    applier.apply(params);
                    took.push(now_ns() - start);
                    answerer.write_all(&[1]).unwrap();
                }
            }
            (took, late)
        })
    };

    let mut next_event = 0;
    let mut asked = false;
    let mut payload = [0u8; PACKET];
    let mut one = [0u8; 1];
    for n in 0..plan.packets {
        let slot = t0 + n * plan.gap_ns;
        let due = next_event < events.len() && events[next_event].0 == n;
        if mode == Mode::BarrierAhead && due && !asked {
            asker.write_all(&[0]).unwrap(); // (the event of packet 0 is asked before the loop would have)
            asked = true;
        }
        sleep_until(slot);
        if due && mode != Mode::Clock {
            let start = now_ns();
            if mode == Mode::BarrierAtSlot {
                asker.write_all(&[0]).unwrap();
            }
            asker.read_exact(&mut one).unwrap();
            outcome.barrier_wait.push(now_ns() - start);
            next_event += 1;
            asked = false;
        } else if due {
            next_event += 1;
        }
        let sent = now_ns();
        payload[..8].copy_from_slice(&n.to_le_bytes());
        payload[8..16].copy_from_slice(&sent.to_le_bytes());
        sock.send(&payload).unwrap();
        outcome.lateness.push(sent - slot);
        // Ask for the next event now: the packet is in the queue
        if mode == Mode::BarrierAhead && next_event < events.len() && events[next_event].0 == n + 1 {
            asker.write_all(&[0]).unwrap();
            asked = true;
        }
    }
    let (took, late) = helper.join().unwrap();
    outcome.apply_took = took;
    outcome.timer_late = late;
    receiver.wait().unwrap();
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    for line in text.lines() {
        let mut p = line.split_whitespace().map(|x| x.parse::<u64>().unwrap());
        outcome.arrived.push((p.next().unwrap(), p.next().unwrap(), p.next().unwrap()));
    }
    outcome
}

/// The receiver: runs in the namespace of `v1`, writes `sequence sent received` for each datagram
fn receive(out: &str) {
    let sock = UdpSocket::bind("10.9.0.2:5000").unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    println!("ready");
    std::io::stdout().flush().unwrap();
    let mut buf = [0u8; 2048];
    let mut text = String::new();
    let mut first = true;
    while let Ok(n) = sock.recv(&mut buf) {
        let at = now_ns();
        if n != PACKET {
            continue;
        }
        let seq = u64::from_le_bytes(buf[..8].try_into().unwrap());
        let sent = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        text.push_str(&format!("{seq} {sent} {at}\n"));
        if first {
            // The packets come every millisecond at least; this ends the run when they stop
            sock.set_read_timeout(Some(Duration::from_millis(400))).unwrap();
            first = false;
        }
    }
    std::fs::write(out, text).unwrap();
}

// ---------------------------------------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------------------------------------

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn summary(values: &[u64]) -> String {
    let mut v = values.to_vec();
    v.sort_unstable();
    let ms = |x: u64| x as f64 / 1e6;
    format!(
        "n={} p50={:.3} ms p90={:.3} ms p99={:.3} ms max={:.3} ms",
        v.len(),
        ms(percentile(&v, 50.0)),
        ms(percentile(&v, 90.0)),
        ms(percentile(&v, 99.0)),
        ms(v.last().copied().unwrap_or(0))
    )
}

fn lost(outcome: &Outcome, packets: u64) -> BTreeSet<u64> {
    let got: BTreeSet<u64> = outcome.arrived.iter().map(|a| a.0).collect();
    (0..packets).filter(|n| !got.contains(n)).collect()
}

/// The first packet whose delay is above `threshold_ns`; the packets that arrived out of order are sorted first
fn first_slow(outcome: &Outcome, threshold_ns: u64) -> Option<u64> {
    let mut a = outcome.arrived.clone();
    a.sort_unstable();
    a.iter().find(|(_, sent, recv)| recv - sent > threshold_ns).map(|a| a.0)
}

// ---------------------------------------------------------------------------------------------------------
// The three questions
// ---------------------------------------------------------------------------------------------------------

fn latency(quick: bool, threads: usize) {
    let n = if quick { 100 } else { 400 };
    for busy in [false, true] {
        let _load = busy.then(|| Busy::start(threads));
        thread::sleep(Duration::from_millis(200));
        for kind in [Kind::Process, Kind::Batch] {
            tc(&["qdisc", "replace", "dev", "v0", "root", "netem", "delay", "10ms", "limit", "10000"]);
            let mut applier = match Applier::try_new(kind) {
                Ok(applier) => applier,
                Err(error) => {
                    report("FAIL", "S11.1", format!("{}: {error}", kind.name()));
                    continue;
                }
            };
            let mut took = Vec::new();
            for i in 0..n {
                let params = if i % 2 == 0 { "delay 11ms limit 10000" } else { "delay 12ms loss 1% limit 10000" };
                let start = now_ns();
                if let Err(error) = applier.try_apply(params) {
                    report("FAIL", "S11.1", format!("{}: {error}", kind.name()));
                    break;
                }
                took.push(now_ns() - start);
                // not back to back: a real timeline has gaps, and the next change meets a cold cache
                thread::sleep(Duration::from_micros(300));
            }
            report("INFO", "S11.1", format!("apply, {}, machine {}: {}", kind.name(), if busy { "busy" } else { "idle" }, summary(&took)));
        }
    }
}

fn boundaries(quick: bool, ns_pid: &str, threads: usize) {
    let runs = if quick { 6 } else { 20 };
    let plan = Plan { initial: "delay 5ms", events: vec![(300, "delay 25ms")], packets: 600, gap_ns: 1_000_000, seed: 1, };
    let mut barrier_exact = true;
    let mut clock_exact = true;
    for (busy, kind) in [(false, batch_kind()), (true, batch_kind()), (false, Kind::Process)] {
        for mode in [Mode::BarrierAtSlot, Mode::BarrierAhead, Mode::Clock] {
            let _load = busy.then(|| Busy::start(threads));
            thread::sleep(Duration::from_millis(100));
            let mut histogram: BTreeMap<i64, u32> = BTreeMap::new();
            let mut late_at_event = Vec::new();
            let mut late_all = Vec::new();
            let mut waits = Vec::new();
            let mut timer = Vec::new();
            let mut took = Vec::new();
            let mut none = 0;
            for _ in 0..runs {
                let o = run_once(&plan, mode, kind, ns_pid);
                match first_slow(&o, 15_000_000) {
                    Some(first) => *histogram.entry(first as i64 - 300).or_default() += 1,
                    None => none += 1,
                }
                late_at_event.push(o.lateness[300]);
                late_all.extend(o.lateness.iter().copied());
                waits.extend(o.barrier_wait);
                timer.extend(o.timer_late);
                took.extend(o.apply_took);
            }
            let exact = histogram.len() == 1 && histogram.contains_key(&0) && none == 0;
            match mode {
                Mode::Clock => clock_exact &= exact,
                _ => barrier_exact &= exact,
            }
            let hist: Vec<String> = histogram.iter().map(|(k, v)| format!("{k:+}: {v}")).collect();
            let timer_text = if timer.is_empty() {
                String::new()
            } else {
                let mut t: Vec<i64> = timer.clone();
                t.sort_unstable();
                format!(", timer woke {:.3} ms .. {:.3} ms after the moment", t[0] as f64 / 1e6, t[t.len() - 1] as f64 / 1e6)
            };
            report(
                "INFO",
                "S11.2",
                format!(
                    "{} / {} / machine {}: packets between the intended and the observed first slow packet over {runs} runs [{}]{}; the packet of the event left {} late; all packets {}; change took {}",
                    mode.name(),
                    kind.name(),
                    if busy { "busy" } else { "idle" },
                    hist.join(", "),
                    timer_text,
                    summary(&late_at_event),
                    summary(&late_all),
                    summary(&took),
                ),
            );
            if !waits.is_empty() {
                report("INFO", "S11.2", format!("   question to answer: {}", summary(&waits)));
            }
        }
    }
    if barrier_exact {
        report("PASS", "S11.2", "with a barrier the change landed on the intended packet in every run, idle and busy".into());
    } else {
        report("FAIL", "S11.2", "with a barrier the change did NOT always land on the intended packet".into());
    }
    if clock_exact {
        report("WARN", "S11.2", "the clock event also landed exactly in every run here: this machine does not show the difference".into());
    } else {
        report("PASS", "S11.2", "canary: the clock event misses the intended packet in some runs, so the test can tell a barrier from a timer".into());
    }
}

fn reproducible(quick: bool, ns_pid: &str, threads: usize) {
    let runs = if quick { 4 } else { 10 };
    struct Case {
        name: &'static str,
        initial: &'static str,
        events: Vec<(u64, &'static str)>,
    }
    let cases = [
        Case { name: "control: constant loss 30%", initial: "loss 30%", events: vec![] },
        Case { name: "loss 0 -> 30% at 300", initial: "loss 0%", events: vec![(300, "loss 30% limit 10000")] },
        Case { name: "loss 10% -> 30% at 300", initial: "loss 10%", events: vec![(300, "loss 30% limit 10000")] },
        Case { name: "loss 30% -> 0 at 300 -> 30% at 350", initial: "loss 30%", events: vec![(300, "loss 0% limit 10000"), (350, "loss 30% limit 10000")] },
        Case { name: "loss 30% -> 100% (outage) at 300 -> 30% at 350", initial: "loss 30%", events: vec![(300, "loss 100% limit 10000"), (350, "loss 30% limit 10000")] },
        Case { name: "loss 30% with duplicate 0 -> 10% at 300", initial: "loss 30%", events: vec![(300, "loss 30% duplicate 10% limit 10000")] },
    ];
    let mut all_barrier_same = true;
    let mut control_same = true;
    let mut clock_differs = false;
    for busy in [false, true] {
        for case in &cases {
            for mode in [Mode::BarrierAhead, Mode::Clock] {
                let _load = busy.then(|| Busy::start(threads));
                thread::sleep(Duration::from_millis(100));
                let plan = Plan { initial: case.initial, events: case.events.clone(), packets: 1000, gap_ns: 1_000_000, seed: 1234 };
                let sets: Vec<BTreeSet<u64>> = (0..runs).map(|_| lost(&run_once(&plan, mode, batch_kind(), ns_pid), plan.packets)).collect();
                let distinct: BTreeSet<&BTreeSet<u64>> = sets.iter().collect();
                let mean_lost = sets.iter().map(|s| s.len()).sum::<usize>() as f64 / runs as f64;
                // How far apart two runs are: packets that were lost in one and not in the other
                let mut diffs = Vec::new();
                for i in 1..sets.len() {
                    diffs.push(sets[0].symmetric_difference(&sets[i]).count());
                }
                let first_diff = sets[1..].iter().filter_map(|s| sets[0].symmetric_difference(s).next().copied()).min();
                let same = distinct.len() == 1;
                if case.events.is_empty() {
                    control_same &= same;
                } else if mode == Mode::BarrierAhead {
                    all_barrier_same &= same;
                } else if !same {
                    clock_differs = true;
                }
                report(
                    "INFO",
                    "S11.3",
                    format!(
                        "{} / {} / machine {}: {} different sets of lost packets in {runs} runs (about {:.0} lost); packets that differ from run 1: {:?}; first at {:?}",
                        case.name,
                        mode.name(),
                        if busy { "busy" } else { "idle" },
                        distinct.len(),
                        mean_lost,
                        diffs,
                        first_diff
                    ),
                );
            }
        }
    }
    if control_same {
        report("PASS", "S11.3", "control: without events the same seed gave the same lost packets in every run (the set-up is quiet)".into());
    } else {
        report("FAIL", "S11.3", "control: without events the same seed gave different lost packets: the set-up is not quiet, the other results mean nothing".into());
    }
    if all_barrier_same {
        report("PASS", "S11.3", "with a barrier the same seed gave the same lost packets in every run, for every kind of change".into());
    } else {
        report("FAIL", "S11.3", "with a barrier some change gave different lost packets in different runs".into());
    }
    if clock_differs {
        report("PASS", "S11.3", "canary: with the clock some change gave different lost packets in different runs, so the test can tell the two apart".into());
    } else {
        report("WARN", "S11.3", "the clock gave the same lost packets in every run here: this machine does not show the difference".into());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let quick = std::env::var("QUICK").map(|v| v == "1").unwrap_or(false);
    let threads = thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    match args.get(1).map(String::as_str) {
        Some("recv") => receive(&args[2]),
        Some("all") => {
            let ns_pid = &args[2];
            report("INFO", "S11.0", format!("{threads} processors, quick={quick}"));
            tc(&["qdisc", "replace", "dev", "v0", "root", "netem", "limit", "10000"]);
            match Applier::try_new(Kind::Batch) {
                Ok(_) => {
                    let _ = BATCH_KIND.set(Kind::Batch);
                }
                Err(error) => {
                    report("FAIL", "S11.0", format!("a persistent tc -batch does not work here: {error}; the runs below use a tc process for each change"));
                    let _ = BATCH_KIND.set(Kind::Process);
                }
            }
            latency(quick, threads);
            boundaries(quick, ns_pid, threads);
            reproducible(quick, ns_pid, threads);
        }
        _ => eprintln!("usage: spike-timeline all <pid of the second namespace> | recv <file>"),
    }
}
