//! Spike S21: a packet engine data path. Everything is safe Rust except `sys`, the one module that binds a packet socket.
//!
//!   s21 --links C1:S1,C2:S2 --delay-us N --jitter-us N --loss PERCENT --seed N --timer T --threads N --batch N
//!
//! Forwards Ethernet frames between the two ends of each link (AF_PACKET sockets) with a fixed delay, a uniform jitter (a sorted
//! queue: a jitter larger than the gap reorders) and a seeded loss. T is one of epoll_ms, epoll_ns, timerfd, spin, spinsleep.
//! Stops on SIGTERM and prints one JSON line with the lateness of every release (the time between the moment a packet was due
//! and the moment it was handed to the interface), the CPU time and the counters.

#![deny(unsafe_code)]

mod sys;

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::socket::{recv, send, MsgFlags};
use rustix::event::epoll;
use rustix::event::epoll::{CreateFlags, EventData, EventFlags};
use rustix::time::{
    clock_gettime, timerfd_create, timerfd_settime, ClockId, Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags,
    Timespec,
};

const MAX_FRAME: usize = 2100; // a frame of up to 2048 bytes and the 10 byte header of --vnet-hdr
const BUCKETS: usize = 20_000; // one microsecond each, up to 20 ms

fn now_ns() -> u64 {
    let t = clock_gettime(ClockId::Monotonic);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Timer {
    EpollMs,
    EpollNs,
    Timerfd,
    Spin,
    SpinSleep,
}

#[derive(Clone)]
struct Config {
    delay_ns: u64,
    jitter_ns: u64,
    loss: f64,
    seed: u64,
    timer: Timer,
}

#[derive(Default, Clone)]
struct Stats {
    forwarded: u64,
    dropped: u64,
    send_errors: u64,
    reordered: u64,
    max_queue: usize,
    wakeups: u64,
    late_over: u64,
    hist: Vec<u64>,
    late_max_ns: u64,
    late_sum_ns: u128,
    cpu_seconds: f64,
}

struct Slot {
    data: [u8; MAX_FRAME],
    len: usize,
}

/// One thread: the links it owns, a heap of packets ordered by the moment they are due, and a timer.
fn run_thread(links: Vec<(OwnedFd, OwnedFd)>, config: Config, stop: Arc<AtomicBool>) -> Stats {
    // A timer that is allowed to fire late by 50 microseconds by default; one nanosecond asks for the exact moment
    let _ = nix::sys::prctl::set_timerslack(1);
    let mut fds: Vec<OwnedFd> = Vec::new();
    for (a, b) in links {
        fds.push(a);
        fds.push(b);
    }
    let poll = epoll::create(CreateFlags::CLOEXEC).expect("epoll");
    for (i, fd) in fds.iter().enumerate() {
        epoll::add(&poll, fd, EventData::new_u64(i as u64), EventFlags::IN).expect("epoll add");
    }
    let tfd = timerfd_create(TimerfdClockId::Monotonic, TimerfdFlags::NONBLOCK | TimerfdFlags::CLOEXEC).expect("timerfd");
    epoll::add(&poll, &tfd, EventData::new_u64(u64::MAX), EventFlags::IN).expect("epoll add timerfd");

    let mut slab: Vec<Slot> = Vec::new();
    let mut free: Vec<u32> = Vec::new();
    let mut heap: BinaryHeap<Reverse<(u64, u64, u32, u32)>> = BinaryHeap::new(); // (due, number, slot, out)
    let mut stats = Stats { hist: vec![0; BUCKETS], ..Stats::default() };
    let mut rng = Rng(config.seed);
    let mut number = 0u64;
    let mut last_released = 0u64;
    let mut events: Vec<epoll::Event> = Vec::with_capacity(64);

    while !stop.load(Ordering::Relaxed) {
        // 1. hand over what is due
        let now = now_ns();
        while let Some(&Reverse((due, n, slot, out))) = heap.peek() {
            if due > now {
                break;
            }
            heap.pop();
            let s = &slab[slot as usize];
            match send(std::os::fd::AsRawFd::as_raw_fd(&fds[out as usize]), &s.data[..s.len], MsgFlags::empty()) {
                Ok(_) => stats.forwarded += 1,
                Err(_) => stats.send_errors += 1,
            }
            let late = now_ns().saturating_sub(due);
            let bucket = (late / 1000) as usize;
            if bucket < BUCKETS {
                stats.hist[bucket] += 1;
            } else {
                stats.late_over += 1;
            }
            stats.late_max_ns = stats.late_max_ns.max(late);
            stats.late_sum_ns += late as u128;
            if n < last_released {
                stats.reordered += 1;
            }
            last_released = n;
            free.push(slot);
        }
        // 2. take in what has arrived
        for side in 0..fds.len() {
            for _ in 0..64 {
                let raw = std::os::fd::AsRawFd::as_raw_fd(&fds[side]);
                let slot = free.pop().unwrap_or_else(|| {
                    slab.push(Slot { data: [0; MAX_FRAME], len: 0 });
                    (slab.len() - 1) as u32
                });
                match recv(raw, &mut slab[slot as usize].data, MsgFlags::empty()) {
                    Ok(0) | Err(Errno::EAGAIN) => {
                        free.push(slot);
                        break;
                    }
                    Ok(n) => {
                        let arrival = now_ns();
                        if config.loss > 0.0 && rng.unit() < config.loss {
                            stats.dropped += 1;
                            free.push(slot);
                            continue;
                        }
                        let jitter = if config.jitter_ns > 0 {
                            let j = (rng.unit() * 2.0 - 1.0) * config.jitter_ns as f64;
                            j as i64
                        } else {
                            0
                        };
                        let due = (arrival as i64 + config.delay_ns as i64 + jitter).max(arrival as i64) as u64;
                        slab[slot as usize].len = n;
                        number += 1;
                        // the other end of the link: 0 <-> 1, 2 <-> 3, ...
                        heap.push(Reverse((due, number, slot, (side ^ 1) as u32)));
                    }
                    Err(_) => {
                        free.push(slot);
                        break;
                    }
                }
            }
        }
        stats.max_queue = stats.max_queue.max(heap.len());
        // 3. wait for a frame or for the next due moment
        let wait_ns = heap.peek().map(|Reverse((due, ..))| due.saturating_sub(now_ns()));
        match config.timer {
            Timer::Spin => {}
            Timer::SpinSleep => {
                stats.wakeups += 1;
                match wait_ns {
                    Some(w) if w > 150_000 => {
                        let t = Timespec { tv_sec: 0, tv_nsec: (w - 100_000) as _ };
                        wait_report(epoll::wait(&poll, rustix::buffer::spare_capacity(&mut events), Some(&t)));
                    }
                    Some(_) => {}
                    None => {
                        let t = Timespec { tv_sec: 0, tv_nsec: 50_000_000 };
                        wait_report(epoll::wait(&poll, rustix::buffer::spare_capacity(&mut events), Some(&t)));
                    }
                }
            }
            Timer::EpollMs | Timer::EpollNs => {
                stats.wakeups += 1;
                let w = wait_ns.unwrap_or(50_000_000).max(1);
                let w = if config.timer == Timer::EpollMs { w.div_ceil(1_000_000) * 1_000_000 } else { w };
                let t = Timespec { tv_sec: (w / 1_000_000_000) as _, tv_nsec: (w % 1_000_000_000) as _ };
                wait_report(epoll::wait(&poll, rustix::buffer::spare_capacity(&mut events), Some(&t)));
            }
            Timer::Timerfd => {
                stats.wakeups += 1;
                let w = wait_ns.unwrap_or(50_000_000).max(1);
                let spec = Itimerspec {
                    it_interval: Timespec { tv_sec: 0, tv_nsec: 0 },
                    it_value: Timespec { tv_sec: (w / 1_000_000_000) as _, tv_nsec: (w % 1_000_000_000) as _ },
                };
                let _ = timerfd_settime(&tfd, TimerfdTimerFlags::empty(), &spec);
                wait_report(epoll::wait(&poll, rustix::buffer::spare_capacity(&mut events), None));
                let mut sink = [0u8; 8];
                let _ = rustix::io::read(tfd.as_fd(), &mut sink);
            }
        }
        events.clear();
    }
    if let Ok(usage) = nix::sys::resource::getrusage(nix::sys::resource::UsageWho::RUSAGE_THREAD) {
        let (user, system) = (usage.user_time(), usage.system_time());
        stats.cpu_seconds = user.tv_sec() as f64 + user.tv_usec() as f64 / 1e6 + system.tv_sec() as f64 + system.tv_usec() as f64 / 1e6;
    }
    stats
}

fn wait_report<T>(result: rustix::io::Result<T>) {
    use std::sync::atomic::AtomicBool;
    static SAID: AtomicBool = AtomicBool::new(false);
    if let Err(e) = result {
        if !SAID.swap(true, Ordering::Relaxed) {
            eprintln!("epoll_wait failed: {e}");
        }
    }
}

fn percentile(hist: &[u64], over: u64, p: f64) -> f64 {
    let total: u64 = hist.iter().sum::<u64>() + over;
    if total == 0 {
        return 0.0;
    }
    let target = (total as f64 * p).ceil() as u64;
    let mut seen = 0;
    for (i, c) in hist.iter().enumerate() {
        seen += c;
        if seen >= target {
            return i as f64 + 1.0; // upper edge of the bucket, in microseconds
        }
    }
    f64::INFINITY
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |name: &str, default: &str| -> String {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
    };
    let links_arg = get("--links", "");
    let threads: usize = get("--threads", "1").parse().unwrap();
    let vnet = args.iter().any(|a| a == "--vnet-hdr");
    let timer = match get("--timer", "epoll_ns").as_str() {
        "epoll_ms" => Timer::EpollMs,
        "epoll_ns" => Timer::EpollNs,
        "timerfd" => Timer::Timerfd,
        "spin" => Timer::Spin,
        "spinsleep" => Timer::SpinSleep,
        other => panic!("unknown timer {other}"),
    };
    let config = Config {
        delay_ns: get("--delay-us", "0").parse::<u64>().unwrap() * 1000,
        jitter_ns: get("--jitter-us", "0").parse::<u64>().unwrap() * 1000,
        loss: get("--loss", "0").parse::<f64>().unwrap() / 100.0,
        seed: get("--seed", "1").parse().unwrap(),
        timer,
    };
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&stop)).expect("signal");
    }
    let mut groups: Vec<Vec<(OwnedFd, OwnedFd)>> = (0..threads).map(|_| Vec::new()).collect();
    for (i, pair) in links_arg.split(',').filter(|s| !s.is_empty()).enumerate() {
        let (a, b) = pair.split_once(':').expect("links are A:B");
        groups[i % threads].push((sys::open_packet(a, vnet), sys::open_packet(b, vnet)));
    }
    let mut handles = Vec::new();
    for (i, links) in groups.into_iter().enumerate() {
        let mut c = config.clone();
        c.seed = config.seed.wrapping_add(i as u64);
        let stop = Arc::clone(&stop);
        handles.push(std::thread::spawn(move || run_thread(links, c, stop)));
    }
    let started = now_ns();
    let all: Vec<Stats> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let wall = (now_ns() - started) as f64 / 1e9;
    let mut total = Stats { hist: vec![0; BUCKETS], ..Stats::default() };
    for s in &all {
        total.forwarded += s.forwarded;
        total.dropped += s.dropped;
        total.send_errors += s.send_errors;
        total.reordered += s.reordered;
        total.max_queue = total.max_queue.max(s.max_queue);
        total.wakeups += s.wakeups;
        total.late_over += s.late_over;
        total.late_max_ns = total.late_max_ns.max(s.late_max_ns);
        total.late_sum_ns += s.late_sum_ns;
        total.cpu_seconds += s.cpu_seconds;
        for (i, c) in s.hist.iter().enumerate() {
            total.hist[i] += c;
        }
    }
    let n = total.forwarded.max(1);
    println!(
        "{{\"role\":\"engine\",\"timer\":\"{:?}\",\"threads\":{threads},\"forwarded\":{},\"dropped\":{},\"send_errors\":{},\"reordered\":{},\"max_queue\":{},\"wakeups\":{},\"cpu_seconds\":{:.2},\"wall_seconds\":{wall:.2},\"late_mean_us\":{:.1},\"late_p50_us\":{},\"late_p99_us\":{},\"late_p999_us\":{},\"late_max_us\":{:.0},\"late_over_20ms\":{}}}",
        config.timer,
        total.forwarded,
        total.dropped,
        total.send_errors,
        total.reordered,
        total.max_queue,
        total.wakeups,
        total.cpu_seconds,
        total.late_sum_ns as f64 / n as f64 / 1000.0,
        percentile(&total.hist, total.late_over, 0.5),
        percentile(&total.hist, total.late_over, 0.99),
        percentile(&total.hist, total.late_over, 0.999),
        total.late_max_ns as f64 / 1000.0,
        total.late_over,
    );
    let _ = Duration::from_secs(0);
}
