//! Spike S8: a packet engine prototype and the tools to measure it.
//!
//!   spike-engine engine IF_A IF_B DELAY_US LOSS_PERCENT SEED TIMER
//!       Forwards Ethernet frames between two interfaces (AF_PACKET sockets) with a fixed delay and a
//!       seeded random loss. TIMER is one of: epoll_ms, epoll_ns, spin. Stops on SIGTERM and prints
//!       its statistics as one JSON line.
//!   spike-engine send ADDRESS PORT PPS SECONDS PAYLOAD_BYTES
//!       UDP constant-rate sender; every packet carries its sequence number and its send time.
//!   spike-engine recv ADDRESS PORT MAX_SECONDS
//!       UDP receiver; prints one JSON line with loss, one-way delay and a hash of the lost sequence numbers.
//!
//! All clocks are CLOCK_MONOTONIC, shared by the namespaces on one machine, so one-way delay is exact.

use std::collections::VecDeque;
use std::net::UdpSocket;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn now_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// splitmix64: a small, well-known generator with a documented sequence for a given seed.
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

// ---------------------------------------------------------------------------------------------- engine

const MAX_FRAME: usize = 2048;

struct Pkt {
    release: u64,
    out: usize,
    len: usize,
    data: [u8; MAX_FRAME],
}

#[derive(Clone, Copy, PartialEq)]
enum Timer {
    EpollMs,
    EpollNs,
    Spin,
}

fn open_raw(name: &str) -> i32 {
    unsafe {
        let fd = libc::socket(
            libc::AF_PACKET,
            libc::SOCK_RAW | libc::SOCK_NONBLOCK,
            (libc::ETH_P_ALL as u16).to_be() as i32,
        );
        assert!(fd >= 0, "socket(AF_PACKET): {}", std::io::Error::last_os_error());
        let cname = std::ffi::CString::new(name).unwrap();
        let ifindex = libc::if_nametoindex(cname.as_ptr());
        assert!(ifindex != 0, "no interface {name}");
        // Do not see what this process sends out of the same interface
        let one: libc::c_int = 1;
        libc::setsockopt(fd, libc::SOL_PACKET, 23 /* PACKET_IGNORE_OUTGOING */, &one as *const _ as *const _, 4);
        let buf: libc::c_int = 4 * 1024 * 1024;
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVBUF, &buf as *const _ as *const _, 4);
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_SNDBUF, &buf as *const _ as *const _, 4);
        let mut sll: libc::sockaddr_ll = std::mem::zeroed();
        sll.sll_family = libc::AF_PACKET as u16;
        sll.sll_protocol = (libc::ETH_P_ALL as u16).to_be();
        sll.sll_ifindex = ifindex as i32;
        let r = libc::bind(fd, &sll as *const _ as *const _, std::mem::size_of::<libc::sockaddr_ll>() as u32);
        assert!(r == 0, "bind {name}: {}", std::io::Error::last_os_error());
        fd
    }
}

fn engine(args: &[String]) {
    let (if_a, if_b) = (&args[0], &args[1]);
    let delay_ns = args[2].parse::<u64>().unwrap() * 1000;
    let loss = args[3].parse::<f64>().unwrap() / 100.0;
    let mut rng = Rng(args[4].parse::<u64>().unwrap());
    let timer = match args[5].as_str() {
        "epoll_ms" => Timer::EpollMs,
        "epoll_ns" => Timer::EpollNs,
        "spin" => Timer::Spin,
        t => panic!("unknown timer {t}"),
    };
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as usize);
        libc::signal(libc::SIGINT, on_signal as *const () as usize);
    }
    let fds = [open_raw(if_a), open_raw(if_b)];
    let ep = unsafe { libc::epoll_create1(0) };
    for (i, fd) in fds.iter().enumerate() {
        let mut ev = libc::epoll_event { events: libc::EPOLLIN as u32, u64: i as u64 };
        unsafe { libc::epoll_ctl(ep, libc::EPOLL_CTL_ADD, *fd, &mut ev) };
    }

    let mut queue: VecDeque<Pkt> = VecDeque::with_capacity(1 << 16);
    let (mut forwarded, mut dropped, mut send_errors, mut max_queue, mut wakeups) = (0u64, 0u64, 0u64, 0usize, 0u64);
    let started = now_ns();
    let mut scratch = [0u8; MAX_FRAME];
    let mut events = [libc::epoll_event { events: 0, u64: 0 }; 4];

    while !STOP.load(Ordering::Relaxed) {
        // 1. send everything that is due (a fixed delay keeps the queue sorted by release time)
        let now = now_ns();
        while let Some(p) = queue.front() {
            if p.release > now {
                break;
            }
            let p = queue.pop_front().unwrap();
            let r = unsafe { libc::send(fds[p.out], p.data.as_ptr() as *const _, p.len, 0) };
            if r < 0 {
                send_errors += 1;
            } else {
                forwarded += 1;
            }
        }
        // 2. take in what has arrived, up to a batch from each side
        for side in 0..2 {
            for _ in 0..64 {
                let n = unsafe { libc::recv(fds[side], scratch.as_mut_ptr() as *mut _, MAX_FRAME, 0) };
                if n <= 0 {
                    break;
                }
                let t = now_ns();
                if loss > 0.0 && rng.unit() < loss {
                    dropped += 1;
                    continue;
                }
                let mut p = Pkt { release: t + delay_ns, out: 1 - side, len: n as usize, data: [0; MAX_FRAME] };
                p.data[..n as usize].copy_from_slice(&scratch[..n as usize]);
                queue.push_back(p);
            }
        }
        max_queue = max_queue.max(queue.len());
        // 3. wait for the next packet or the next release time
        match timer {
            Timer::Spin => {}
            Timer::EpollMs | Timer::EpollNs => {
                let now = now_ns();
                let wait_ns = match queue.front() {
                    Some(p) => p.release.saturating_sub(now),
                    None => 100_000_000, // look at the stop flag every 100 ms
                };
                wakeups += 1;
                if timer == Timer::EpollMs {
                    let ms = wait_ns.div_ceil(1_000_000) as i32;
                    unsafe { libc::epoll_wait(ep, events.as_mut_ptr(), 4, ms) };
                } else {
                    let ts = libc::timespec { tv_sec: (wait_ns / 1_000_000_000) as i64, tv_nsec: (wait_ns % 1_000_000_000) as i64 };
                    unsafe {
                        libc::syscall(libc::SYS_epoll_pwait2, ep, events.as_mut_ptr(), 4, &ts as *const _, std::ptr::null::<libc::sigset_t>(), 0usize)
                    };
                }
            }
        }
    }
    let wall = (now_ns() - started) as f64 / 1e9;
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let cpu = ru.ru_utime.tv_sec as f64 + ru.ru_utime.tv_usec as f64 / 1e6 + ru.ru_stime.tv_sec as f64 + ru.ru_stime.tv_usec as f64 / 1e6;
    println!(
        "{{\"role\":\"engine\",\"forwarded\":{forwarded},\"dropped\":{dropped},\"send_errors\":{send_errors},\"max_queue\":{max_queue},\"wakeups\":{wakeups},\"cpu_seconds\":{cpu:.2},\"wall_seconds\":{wall:.2}}}"
    );
}

// ------------------------------------------------------------------------------------------------ send

fn send(args: &[String]) {
    let (addr, port) = (&args[0], &args[1]);
    let pps: u64 = args[2].parse().unwrap();
    let secs: u64 = args[3].parse().unwrap();
    let payload: usize = args[4].parse().unwrap();
    let sock = UdpSocket::bind("0.0.0.0:0").unwrap();
    sock.connect(format!("{addr}:{port}")).unwrap();
    // No UDP checksum: the engine forwards frames that were handed to the veth unfinished (checksum offload)
    let one: libc::c_int = 1;
    if std::env::var_os("SPIKE_KEEP_CHECKSUM").is_none() {
        unsafe { libc::setsockopt(sock.as_raw_fd(), libc::SOL_SOCKET, libc::SO_NO_CHECK, &one as *const _ as *const _, 4) };
    }
    let mut buf = vec![0u8; payload.max(16)];
    let interval = 1_000_000_000 / pps;
    let total = pps * secs;
    let start = now_ns() + 50_000_000;
    let mut late = 0u64;
    for seq in 0..total {
        let due = start + seq * interval;
        loop {
            if now_ns() >= due {
                break;
            }
            std::hint::spin_loop();
        }
        buf[..8].copy_from_slice(&seq.to_be_bytes());
        buf[8..16].copy_from_slice(&now_ns().to_be_bytes());
        if sock.send(&buf).is_err() {
            late += 1;
        }
    }
    let wall = (now_ns() - start) as f64 / 1e9;
    println!("{{\"role\":\"send\",\"sent\":{total},\"send_errors\":{late},\"achieved_pps\":{:.0},\"target_pps\":{pps}}}", total as f64 / wall);
}

// ------------------------------------------------------------------------------------------------- recv

fn recv(args: &[String]) {
    let sock = UdpSocket::bind(format!("{}:{}", args[0], args[1])).unwrap();
    sock.set_nonblocking(true).unwrap();
    let buf_size: libc::c_int = 8 * 1024 * 1024;
    unsafe { libc::setsockopt(sock.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVBUF, &buf_size as *const _ as *const _, 4) };
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as usize);
        libc::signal(libc::SIGINT, on_signal as *const () as usize);
    }
    let mut buf = [0u8; 4096];
    let mut seen: Vec<(u64, u64)> = Vec::with_capacity(2_000_000); // (seq, delay in ns)
    let mut last_rx = now_ns();
    let mut first_rx = 0u64;
    let hard_stop = now_ns() + args[2].parse::<u64>().unwrap() * 1_000_000_000;
    while !STOP.load(Ordering::Relaxed) && now_ns() < hard_stop {
        match sock.recv(&mut buf) {
            Ok(n) if n >= 16 => {
                let t = now_ns();
                let seq = u64::from_be_bytes(buf[..8].try_into().unwrap());
                let sent = u64::from_be_bytes(buf[8..16].try_into().unwrap());
                if first_rx == 0 {
                    first_rx = t;
                }
                last_rx = t;
                seen.push((seq, t.saturating_sub(sent)));
            }
            Ok(_) => {}
            Err(_) => {
                // stop 1.5 s after the last packet
                if first_rx != 0 && now_ns() - last_rx > 1_500_000_000 {
                    break;
                }
                std::hint::spin_loop();
            }
        }
    }
    if seen.is_empty() {
        println!("{{\"role\":\"recv\",\"received\":0}}");
        return;
    }
    let max_seq = seen.iter().map(|s| s.0).max().unwrap();
    let expected = max_seq + 1;
    let mut present = vec![false; expected as usize];
    for (s, _) in &seen {
        present[*s as usize] = true;
    }
    let lost: Vec<u64> = (0..expected).filter(|s| !present[*s as usize]).collect();
    let mut hash = 0xcbf2_9ce4_8422_2325u64; // FNV-1a over the lost sequence numbers
    for s in &lost {
        for b in s.to_be_bytes() {
            hash = (hash ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    let mut d: Vec<f64> = seen.iter().map(|s| s.1 as f64 / 1e6).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |f: f64| d[((d.len() - 1) as f64 * f).round() as usize];
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    let var = d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / d.len() as f64;
    let active = (last_rx - first_rx) as f64 / 1e9;
    println!(
        "{{\"role\":\"recv\",\"received\":{},\"expected\":{expected},\"lost\":{},\"loss_percent\":{:.2},\"lost_hash\":\"{hash:016x}\",\"recv_pps\":{:.0},\"mean_ms\":{mean:.3},\"stdev_ms\":{:.3},\"min_ms\":{:.3},\"p50_ms\":{:.3},\"p99_ms\":{:.3},\"p999_ms\":{:.3},\"max_ms\":{:.3}}}",
        seen.len(),
        lost.len(),
        100.0 * lost.len() as f64 / expected as f64,
        seen.len() as f64 / active,
        var.sqrt(),
        d[0],
        pct(0.5),
        pct(0.99),
        pct(0.999),
        d[d.len() - 1],
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("engine") => engine(&args[2..]),
        Some("send") => send(&args[2..]),
        Some("recv") => recv(&args[2..]),
        _ => eprintln!("usage: spike-engine engine|send|recv ..."),
    }
}
