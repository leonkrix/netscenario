#![forbid(unsafe_code)]
//! Spike S13: packet capture in the agent, with no `unsafe` code of our own.
//!
//!   spike-capture capture <interface> <out.pcap> <idle ms>    reads every frame of the interface (AF_PACKET) with the
//!                                                             timestamp of the kernel, writes a pcap with nanoseconds
//!   spike-capture send <dst> <packets per second> <seconds> <size>   a paced stream of UDP packets to <dst>:5000
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufWriter, IoSliceMut, Write};
use std::net::UdpSocket;
use std::os::fd::AsRawFd;
use std::time::Duration;

use nix::sys::socket::{
    recvmsg, setsockopt, socket, sockopt, AddressFamily, ControlMessageOwned, LinkAddr, MsgFlags, SockFlag,
    SockProtocol, SockType,
};
use nix::sys::time::{TimeVal, TimeValLike};
use rustix::time::{clock_gettime, ClockId};

const PORT: u16 = 5000;

fn monotonic_ns() -> u64 {
    let t = clock_gettime(ClockId::Monotonic);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

fn capture(interface: &str, out: &str, idle_ms: i64, seq_file: Option<&str>) {
    let fd = socket(AddressFamily::Packet, SockType::Raw, SockFlag::SOCK_CLOEXEC, SockProtocol::EthAll)
        .expect("an AF_PACKET socket (needs CAP_NET_RAW in the user namespace)");
    setsockopt(&fd, sockopt::BindToDevice, &OsString::from(interface)).expect("bind to the interface");
    setsockopt(&fd, sockopt::ReceiveTimestampns, &true).expect("timestamps");
    // The most the kernel allows without CAP_NET_ADMIN in the first user namespace: net.core.rmem_max
    setsockopt(&fd, sockopt::RcvBuf, &(64 * 1024 * 1024)).expect("buffer size");
    let buffer = nix::sys::socket::getsockopt(&fd, sockopt::RcvBuf).unwrap_or(0);
    // A timeout ends the loop: 10 s before the first frame, `idle_ms` after
    let wait = |ms: i64| setsockopt(&fd, sockopt::ReceiveTimeout, &TimeVal::milliseconds(ms)).expect("timeout");
    wait(10_000);

    let mut file = BufWriter::new(File::create(out).expect("the pcap file"));
    // pcap with nanosecond timestamps, linktype Ethernet
    file.write_all(&0xa1b2_3c4d_u32.to_le_bytes()).unwrap();
    file.write_all(&[2, 0, 4, 0]).unwrap();
    file.write_all(&[0; 8]).unwrap();
    file.write_all(&262_144_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u32.to_le_bytes()).unwrap();

    let mut frame = vec![0u8; 65_536];
    let mut space = nix::cmsg_space!(nix::sys::time::TimeSpec);
    let (mut frames, mut outgoing, mut host, mut udp, mut otherhost) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut seen_text = String::new();
    let mut sequences = BTreeSet::new();
    let (mut first, mut last) = (None, 0u128);
    let mut late = Vec::new(); // how long after the kernel stamped a frame this loop had it
    loop {
        let mut slices = [IoSliceMut::new(&mut frame)];
        let received = match recvmsg::<LinkAddr>(fd.as_raw_fd(), &mut slices, Some(&mut space), MsgFlags::empty()) {
            Ok(received) => received,
            Err(nix::errno::Errno::EAGAIN) => break,
            Err(error) => panic!("recvmsg: {error}"),
        };
        let bytes = received.bytes;
        let mut stamp = None;
        for message in received.cmsgs().expect("control messages") {
            if let ControlMessageOwned::ScmTimestampns(t) = message {
                stamp = Some((t.tv_sec() as u128, t.tv_nsec() as u128));
            }
        }
        let (sec, nsec) = stamp.expect("every frame has a timestamp");
        match received.address.map(|a| a.pkttype()) {
            Some(4) => outgoing += 1, // PACKET_OUTGOING
            Some(3) => otherhost += 1, // PACKET_OTHERHOST: the destination MAC is not the interface's
            _ => host += 1,
        }
        frames += 1;
        let total = sec * 1_000_000_000 + nsec;
        first.get_or_insert(total);
        last = total;
        let bytes_ref = &frame[..bytes];
        // Ethernet II, IPv4, UDP to our port: the sequence number is the first 8 bytes of the payload
        if bytes >= 14 + 20 + 8 + 8 && bytes_ref[12] == 0x08 && bytes_ref[13] == 0x00 && bytes_ref[23] == 17 {
            let ihl = usize::from(bytes_ref[14] & 0x0f) * 4;
            let dport = u16::from_be_bytes([bytes_ref[14 + ihl + 2], bytes_ref[14 + ihl + 3]]);
            if dport == PORT {
                udp += 1;
                let at = 14 + ihl + 8;
                let seq = u64::from_le_bytes(bytes_ref[at..at + 8].try_into().unwrap());
                sequences.insert(seq);
                seen_text.push_str(&format!("{seq}\n"));
            }
        }
        file.write_all(&(sec as u32).to_le_bytes()).unwrap();
        file.write_all(&(nsec as u32).to_le_bytes()).unwrap();
        file.write_all(&(bytes as u32).to_le_bytes()).unwrap();
        file.write_all(&(bytes as u32).to_le_bytes()).unwrap();
        file.write_all(bytes_ref).unwrap();
        if frames == 1 {
            wait(idle_ms);
        }
        if frames % 64 == 0 {
            // The wall clock stamp is CLOCK_REALTIME: compare it with the realtime clock now
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            late.push(now.saturating_sub(total) as u64);
        }
    }
    file.flush().unwrap();
    if let Some(path) = seq_file {
        std::fs::write(path, seen_text).unwrap();
    }
    late.sort_unstable();
    let p = |q: f64| late.get(((late.len() as f64 * q).ceil() as usize).saturating_sub(1).min(late.len().saturating_sub(1))).copied().unwrap_or(0) as f64 / 1e3;
    println!(
        "frames={frames} outgoing={outgoing} host={host} otherhost={otherhost} udp={udp} distinct={} first_seq={:?} last_seq={:?} span_ms={:.1} rcvbuf={buffer} read_lag_us_p50={:.0} p99={:.0} max={:.0}",
        sequences.len(),
        sequences.first(),
        sequences.last(),
        (last - first.unwrap_or(0)) as f64 / 1e6,
        p(0.5),
        p(0.99),
        p(1.0),
    );
}

fn send(destination: &str, rate: u64, seconds: u64, size: usize) {
    let socket = UdpSocket::bind("10.9.0.1:0").unwrap();
    socket.connect((destination, PORT)).unwrap();
    let gap = 1_000_000_000 / rate;
    let start = monotonic_ns() + 50_000_000;
    let mut payload = vec![0xA5u8; size];
    let total = rate * seconds;
    for n in 0..total {
        let due = start + n * gap;
        while monotonic_ns() < due {
            std::hint::spin_loop();
        }
        payload[..8].copy_from_slice(&n.to_le_bytes());
        socket.send(&payload).unwrap();
    }
    std::thread::sleep(Duration::from_millis(50));
    println!("sent={total}");
}

/// Something that listens on the port, so that the sender does not get "connection refused" back
fn sink(idle_ms: u64, out: Option<&str>) {
    let socket = UdpSocket::bind("10.9.0.2:5000").unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut buffer = [0u8; 2048];
    let mut first = true;
    let mut delivered = String::new();
    while let Ok(n) = socket.recv(&mut buffer) {
        if n >= 8 {
            delivered.push_str(&format!("{}\n", u64::from_le_bytes(buffer[..8].try_into().unwrap())));
        }
        if first {
            socket.set_read_timeout(Some(Duration::from_millis(idle_ms))).unwrap();
            first = false;
        }
    }
    if let Some(path) = out {
        std::fs::write(path, delivered).unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("capture") => capture(&args[2], &args[3], args[4].parse().unwrap(), args.get(5).map(String::as_str)),
        Some("sink") => sink(args[2].parse().unwrap(), args.get(3).map(String::as_str)),
        Some("send") => send(&args[2], args[3].parse().unwrap(), args[4].parse().unwrap(), args[5].parse().unwrap()),
        _ => eprintln!("usage: capture <interface> <out.pcap> <idle ms> | send <dst> <pps> <seconds> <size>"),
    }
}
