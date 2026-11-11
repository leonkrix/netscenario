//! Spike S22: netlink by a thin layer of our own.
//!
//! `netlink-packet-route` 0.34 has no `unsafe` and knows links, addresses, routes and a few qdiscs, but not `netem`, `htb`, `tbf`,
//! `codel`, `pie` or the fifos. Its `TcAttribute::Other` carries bytes that we encode ourselves, so this program builds the
//! messages for those qdiscs from the structures of `linux/pkt_sched.h`, applies them in a namespace, and compares what the kernel
//! reports (`tc -d qdisc show`) with what the `tc` command makes of the same settings. It runs inside `unshare -Urn` (`run.sh`).
//!
//! Each check prints PASS, FAIL or INFO. The last part times a change of a running `netem`.

use std::process::Command;
use std::time::Instant;

use netlink_packet_core::{
    DefaultNla, NetlinkHeader, NetlinkMessage, NetlinkPayload, NLM_F_ACK, NLM_F_CREATE, NLM_F_EXCL, NLM_F_REPLACE, NLM_F_REQUEST,
};
use netlink_packet_route::link::{InfoKind, LinkAttribute, LinkFlags, LinkInfo, LinkMessage};
use netlink_packet_route::tc::{TcAttribute, TcHandle, TcHeader, TcMessage, TcOption};
use netlink_packet_route::{AddressFamily, RouteNetlinkMessage};
use netlink_sys::{protocols::NETLINK_ROUTE, Socket, SocketAddr};

const TCA_OPTIONS: u16 = 2;
const NLA_F_NESTED: u16 = 1 << 15;

// ------------------------------------------------------------------------------------------------ the socket

struct Netlink {
    socket: Socket,
    sequence: u32,
}

impl Netlink {
    fn new() -> Self {
        let mut socket = Socket::new(NETLINK_ROUTE).expect("netlink socket");
        socket.bind_auto().expect("bind");
        socket.connect(&SocketAddr::new(0, 0)).expect("connect");
        Netlink { socket, sequence: 1 }
    }

    /// One request, one acknowledgement.
    fn request(&mut self, payload: RouteNetlinkMessage, flags: u16) -> Result<(), String> {
        let mut header = NetlinkHeader::default();
        header.flags = NLM_F_REQUEST | NLM_F_ACK | flags;
        header.sequence_number = self.sequence;
        self.sequence += 1;
        let mut message = NetlinkMessage::new(header, NetlinkPayload::from(payload));
        message.finalize();
        let mut buffer = vec![0u8; message.buffer_len()];
        message.serialize(&mut buffer);
        self.socket.send(&buffer, 0).map_err(|e| format!("send: {e}"))?;
        let mut receive = vec![0u8; 8192];
        let size = self.socket.recv(&mut &mut receive[..], 0).map_err(|e| format!("recv: {e}"))?;
        let reply = NetlinkMessage::<RouteNetlinkMessage>::deserialize(&receive[..size]).map_err(|e| format!("decode: {e}"))?;
        match reply.payload {
            NetlinkPayload::Error(error) => match error.code {
                None => Ok(()),
                Some(code) => Err(format!("kernel: {}", std::io::Error::from_raw_os_error(-code.get()))),
            },
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    fn add_dummy(&mut self, name: &str) -> Result<(), String> {
        let mut link = LinkMessage::default();
        link.attributes = vec![LinkAttribute::IfName(name.to_string()), LinkAttribute::LinkInfo(vec![LinkInfo::Kind(InfoKind::Dummy)])];
        self.request(RouteNetlinkMessage::NewLink(link), NLM_F_CREATE | NLM_F_EXCL)
    }

    fn set_up(&mut self, index: u32) -> Result<(), String> {
        let mut link = LinkMessage::default();
        link.header.index = index;
        link.header.flags = LinkFlags::Up;
        link.header.change_mask = LinkFlags::Up;
        self.request(RouteNetlinkMessage::SetLink(link), 0)
    }

    fn qdisc(&mut self, index: u32, parent: TcHandle, handle: TcHandle, kind: &str, options: Option<Vec<u8>>, replace: bool) -> Result<(), String> {
        let mut header = TcHeader::default();
        header.index = index as i32;
        header.parent = parent;
        header.handle = handle;
        let mut attributes = vec![TcAttribute::Kind(kind.to_string())];
        if let Some(bytes) = options {
            attributes.push(TcAttribute::Other(DefaultNla::new(TCA_OPTIONS, bytes)));
        }
        let message = TcMessage::from_parts(header, attributes);
        let flags = if replace { NLM_F_CREATE | NLM_F_REPLACE } else { NLM_F_CREATE | NLM_F_EXCL };
        self.request(RouteNetlinkMessage::NewQueueDiscipline(message), flags)
    }

    fn class(&mut self, index: u32, parent: TcHandle, handle: TcHandle, kind: &str, options: Vec<u8>) -> Result<(), String> {
        let mut header = TcHeader::default();
        header.index = index as i32;
        header.parent = parent;
        header.handle = handle;
        let attributes = vec![TcAttribute::Kind(kind.to_string()), TcAttribute::Other(DefaultNla::new(TCA_OPTIONS, options))];
        let message = TcMessage::from_parts(header, attributes);
        self.request(RouteNetlinkMessage::NewTrafficClass(message), NLM_F_CREATE | NLM_F_EXCL)
    }
}

// ----------------------------------------------------------------------------------------------- the encoders

fn nla(kind: u16, value: &[u8]) -> Vec<u8> {
    let length = 4 + value.len();
    let mut out = Vec::with_capacity(length + 3);
    out.extend_from_slice(&(length as u16).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(value);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}
fn nested(kind: u16, inner: &[u8]) -> Vec<u8> {
    nla(kind | NLA_F_NESTED, inner)
}
fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// A probability in percent as the kernel keeps it: a fraction of 2^32 - 1, rounded as `tc` does.
fn probability(percent: f64) -> u32 {
    (percent / 100.0 * 4_294_967_295.0).round() as u32
}

#[derive(Default, Clone)]
struct Netem {
    delay_us: u64,
    jitter_us: u64,
    limit: u32,
    loss_percent: f64,
    loss_correlation: f64,
    gemodel: Option<(f64, f64, f64, f64)>, // p r 1-h 1-k in percent, as `tc` prints them
    duplicate_percent: f64,
    corrupt_percent: f64,
    reorder: Option<(f64, f64, u32)>, // percent, correlation, gap
    rate_bps: u64,
    ecn: bool,
    seed: Option<u64>,
}

impl Netem {
    /// The options of a `netem` qdisc: `tc_netem_qopt`, then attributes (the one qdisc whose options begin with a plain struct).
    fn encode(&self) -> Vec<u8> {
        let mut out = u32s(&[
            0, // latency in psched ticks: the 64-bit attribute below carries it (the kernel reads that one when it is there)
            if self.limit == 0 { 1000 } else { self.limit },
            probability(self.loss_percent),
            self.reorder.map_or(0, |r| r.2),
            probability(self.duplicate_percent),
            0, // jitter in ticks, as the latency
        ]);
        if self.delay_us > 0 || self.jitter_us > 0 {
            out.extend(nla(10, &(self.delay_us * 1000).to_ne_bytes())); // TCA_NETEM_LATENCY64
            out.extend(nla(11, &(self.jitter_us * 1000).to_ne_bytes())); // TCA_NETEM_JITTER64
        }
        if self.loss_correlation > 0.0 {
            out.extend(nla(1, &u32s(&[0, probability(self.loss_correlation), 0]))); // TCA_NETEM_CORR (delay, loss, duplicate)
        }
        if let Some((p, c, _)) = self.reorder {
            out.extend(nla(3, &u32s(&[probability(p), probability(c)]))); // TCA_NETEM_REORDER
        }
        if self.corrupt_percent > 0.0 {
            out.extend(nla(4, &u32s(&[probability(self.corrupt_percent), 0]))); // TCA_NETEM_CORRUPT
        }
        if let Some((p, r, one_h, one_k)) = self.gemodel {
            // TCA_NETEM_LOSS { TCA_NETEM_LOSS_GE = 2: tc_netem_gemodel { p, r, h, k1 } }
            let ge = u32s(&[probability(p), probability(r), probability(100.0 - one_h), probability(one_k)]);
            out.extend(nested(5, &nla(2, &ge)));
        }
        if self.rate_bps > 0 {
            let bytes_per_second = self.rate_bps / 8;
            let mut rate = (bytes_per_second.min(u64::from(u32::MAX)) as u32).to_ne_bytes().to_vec();
            rate.extend(0i32.to_ne_bytes()); // packet_overhead
            rate.extend(0u32.to_ne_bytes()); // cell_size
            rate.extend(0i32.to_ne_bytes()); // cell_overhead
            out.extend(nla(6, &rate)); // TCA_NETEM_RATE
            if bytes_per_second > u64::from(u32::MAX) {
                out.extend(nla(8, &bytes_per_second.to_ne_bytes())); // TCA_NETEM_RATE64
            }
        }
        if self.ecn {
            out.extend(nla(7, &1u32.to_ne_bytes())); // TCA_NETEM_ECN
        }
        if let Some(seed) = self.seed {
            out.extend(nla(14, &seed.to_ne_bytes())); // TCA_NETEM_PRNG_SEED
        }
        out
    }

    /// The same settings as arguments of `tc`.
    fn tc_args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec!["netem".into()];
        if self.limit > 0 {
            args.extend(["limit".into(), self.limit.to_string()]);
        }
        if self.delay_us > 0 || self.jitter_us > 0 {
            args.extend(["delay".into(), format!("{}us", self.delay_us)]);
            if self.jitter_us > 0 {
                args.push(format!("{}us", self.jitter_us));
            }
        }
        if let Some((p, r, h, k)) = self.gemodel {
            args.extend(["loss".into(), "gemodel".into(), format!("{p}%"), format!("{r}%"), format!("{h}%"), format!("{k}%")]);
        } else if self.loss_percent > 0.0 {
            args.extend(["loss".into(), format!("{}%", self.loss_percent)]);
            if self.loss_correlation > 0.0 {
                args.push(format!("{}%", self.loss_correlation));
            }
        }
        if self.duplicate_percent > 0.0 {
            args.extend(["duplicate".into(), format!("{}%", self.duplicate_percent)]);
        }
        if self.corrupt_percent > 0.0 {
            args.extend(["corrupt".into(), format!("{}%", self.corrupt_percent)]);
        }
        if let Some((p, c, gap)) = self.reorder {
            args.extend(["reorder".into(), format!("{p}%"), format!("{c}%")]);
            if gap > 0 {
                args.extend(["gap".into(), gap.to_string()]);
            }
        }
        if self.rate_bps > 0 {
            args.extend(["rate".into(), format!("{}bit", self.rate_bps)]);
        }
        if self.ecn {
            args.push("ecn".into());
        }
        if let Some(seed) = self.seed {
            args.extend(["seed".into(), seed.to_string()]);
        }
        args
    }
}

/// `tc_ratespec`: cell_log, linklayer (1 = ethernet), overhead, cell_align, mpu, rate in bytes per second.
fn ratespec(bytes_per_second: u32) -> Vec<u8> {
    let mut out = vec![0u8, 1];
    out.extend(0u16.to_ne_bytes()); // overhead
    out.extend(0i16.to_ne_bytes()); // cell_align
    out.extend(0u16.to_ne_bytes()); // mpu
    out.extend(bytes_per_second.to_ne_bytes());
    out
}

/// The time to send `size` bytes at `bytes_per_second`, in the 64 ns ticks the scheduler counts in.
fn ticks(size: u32, bytes_per_second: u32) -> u32 {
    ((f64::from(size) / f64::from(bytes_per_second)) * 1e9 / 64.0) as u32
}

// --------------------------------------------------------------------------------------------------- the checks

fn tc(args: &[&str]) -> Result<String, String> {
    let path = format!("{}:/usr/sbin:/sbin", std::env::var("PATH").unwrap_or_default());
    let out = Command::new("tc").args(args).env("PATH", path).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn verdict(id: &str, text: &str, ok: bool, detail: &str) {
    println!("{} {id:<8} {text}{}", if ok { "PASS" } else { "FAIL" }, if ok { String::new() } else { format!("\n         {detail}") });
}

fn index_of(name: &str) -> u32 {
    // /sys shows the devices of the namespace it was mounted in, not of this one: ask `ip`
    let path = format!("{}:/usr/sbin:/sbin", std::env::var("PATH").unwrap_or_default());
    let out = Command::new("ip").args(["-o", "link", "show", name]).env("PATH", path).output().expect("ip");
    String::from_utf8_lossy(&out.stdout).split(':').next().unwrap().trim().parse().expect("ifindex")
}

/// Set up two dummy devices, `ta` (settings by `tc`) and `na` (the same by netlink), and compare the dumps of the kernel.
fn compare(nl: &mut Netlink, id: &str, text: &str, by_tc: &[&str], by_netlink: impl FnOnce(&mut Netlink, u32) -> Result<(), String>) {
    let _ = tc(&["qdisc", "del", "dev", "ta", "root"]);
    let _ = tc(&["qdisc", "del", "dev", "na", "root"]);
    let mut full = vec!["qdisc", "add", "dev", "ta"];
    full.extend_from_slice(by_tc);
    if let Err(e) = tc(&full) {
        println!("INFO {id:<8} tc itself refuses this here: {e}");
        return;
    }
    if let Err(e) = by_netlink(nl, index_of("na")) {
        verdict(id, text, false, &format!("netlink: {e}"));
        return;
    }
    let (a, b) = (tc_dump("ta"), tc_dump("na"));
    verdict(id, text, a == b, &format!("tc:      {a}\n         netlink: {b}"));
}

/// The dump of the kernel as one line, without what differs by chance: the handle that the kernel hands out when none was asked for
/// (four or more hex digits) and a seed that was drawn (a number above a million; the seeds asked for in the checks are small).
fn normalise(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w == "seed" && words.get(i + 1).and_then(|n| n.parse::<u64>().ok()).is_some_and(|n| n > 1_000_000) {
            i += 2;
            continue;
        }
        if w.ends_with(':') && w.len() >= 5 {
            out.push("AUTO:".to_string());
        } else {
            out.push(w.to_string());
        }
        i += 1;
    }
    out.join(" ")
}

fn tc_dump(device: &str) -> String {
    normalise(&tc(&["-d", "qdisc", "show", "dev", device]).unwrap_or_else(|e| format!("error: {e}")))
}

fn tc_dump_class(device: &str) -> String {
    normalise(&tc(&["-d", "class", "show", "dev", device]).unwrap_or_else(|e| format!("error: {e}")))
}

fn handle(major: u16, minor: u16) -> TcHandle {
    TcHandle { major, minor }
}

fn main() {
    let mut nl = Netlink::new();
    for name in ["ta", "na"] {
        nl.add_dummy(name).expect("dummy link");
        nl.set_up(index_of(name)).expect("up");
    }
    println!("INFO S22    netlink-packet-route 0.34.0, netlink-sys 0.9.0, no `unsafe` in the layer; tc: {}", tc(&["-V"]).unwrap_or_default().trim());

    // ---- netem
    let cases: Vec<(&str, &str, Netem)> = vec![
        ("N1", "delay 20 ms, jitter 5 ms, limit 1000", Netem { delay_us: 20_000, jitter_us: 5_000, limit: 1000, ..Netem::default() }),
        ("N2", "loss 10 % with seed 7", Netem { loss_percent: 10.0, seed: Some(7), ..Netem::default() }),
        ("N3", "loss 10 % with a correlation of 25 %", Netem { loss_percent: 10.0, loss_correlation: 25.0, ..Netem::default() }),
        ("N4", "Gilbert-Elliott loss", Netem { gemodel: Some((5.0, 20.0, 10.0, 0.5)), ..Netem::default() }),
        ("N5", "duplicate 5 %, corrupt 2 %", Netem { duplicate_percent: 5.0, corrupt_percent: 2.0, ..Netem::default() }),
        ("N6", "reorder 25 % 50 % gap 5 with a delay", Netem { delay_us: 10_000, reorder: Some((25.0, 50.0, 5)), ..Netem::default() }),
        ("N7", "rate 8 mbit with a delay", Netem { delay_us: 1_000, rate_bps: 8_000_000, ..Netem::default() }),
        ("N8", "ecn and a loss", Netem { loss_percent: 1.0, ecn: true, ..Netem::default() }),
        ("N9", "everything at once", Netem { delay_us: 30_000, jitter_us: 2_000, limit: 500, loss_percent: 3.0, duplicate_percent: 1.0, corrupt_percent: 0.5, rate_bps: 100_000_000, seed: Some(99), ..Netem::default() }),
    ];
    for (id, text, config) in cases {
        let args = config.tc_args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut by_tc = vec!["root"];
        by_tc.extend(args);
        let encoded = config.encode();
        compare(&mut nl, id, text, &by_tc, |nl, index| nl.qdisc(index, TcHandle::ROOT, handle(0, 0), "netem", Some(encoded), false));
    }

    // ---- the fifos, codel, pie, fq_codel
    compare(&mut nl, "F1", "pfifo limit 77", &["root", "pfifo", "limit", "77"], |nl, i| {
        nl.qdisc(i, TcHandle::ROOT, handle(0, 0), "pfifo", Some(u32s(&[77])), false)
    });
    compare(&mut nl, "F2", "bfifo limit 30000", &["root", "bfifo", "limit", "30000"], |nl, i| {
        nl.qdisc(i, TcHandle::ROOT, handle(0, 0), "bfifo", Some(u32s(&[30_000])), false)
    });
    compare(&mut nl, "C1", "codel target 5 ms interval 100 ms limit 500 ecn", &["root", "codel", "target", "5ms", "interval", "100ms", "limit", "500", "ecn"], |nl, i| {
        let mut o = nla(1, &5000u32.to_ne_bytes()); // TARGET in microseconds
        o.extend(nla(2, &500u32.to_ne_bytes())); // LIMIT
        o.extend(nla(3, &100_000u32.to_ne_bytes())); // INTERVAL in microseconds
        o.extend(nla(4, &1u32.to_ne_bytes())); // ECN
        nl.qdisc(i, TcHandle::ROOT, handle(0, 0), "codel", Some(o), false)
    });
    compare(&mut nl, "P1", "pie target 15 ms limit 200 tupdate 16 ms ecn", &["root", "pie", "target", "15ms", "limit", "200", "tupdate", "16ms", "ecn"], |nl, i| {
        let mut o = nla(1, &15_000u32.to_ne_bytes()); // TARGET in microseconds
        o.extend(nla(2, &200u32.to_ne_bytes())); // LIMIT
        o.extend(nla(3, &16_000u32.to_ne_bytes())); // TUPDATE in microseconds
        o.extend(nla(6, &1u32.to_ne_bytes())); // ECN
        nl.qdisc(i, TcHandle::ROOT, handle(0, 0), "pie", Some(o), false)
    });
    compare(&mut nl, "Q1", "fq_codel limit 1000 flows 256 target 5 ms interval 100 ms ecn", &["root", "fq_codel", "limit", "1000", "flows", "256", "target", "5ms", "interval", "100ms", "ecn"], |nl, i| {
        let mut o = nla(2, &1000u32.to_ne_bytes()); // LIMIT
        o.extend(nla(1, &5000u32.to_ne_bytes())); // TARGET
        o.extend(nla(3, &100_000u32.to_ne_bytes())); // INTERVAL
        o.extend(nla(4, &1u32.to_ne_bytes())); // ECN
        o.extend(nla(5, &256u32.to_ne_bytes())); // FLOWS
        nl.qdisc(i, TcHandle::ROOT, handle(0, 0), "fq_codel", Some(o), false)
    });

    // ---- tbf
    compare(&mut nl, "T1", "tbf rate 8 mbit burst 5000 limit 20000", &["root", "tbf", "rate", "8mbit", "burst", "5000", "limit", "20000"], |nl, i| {
        let rate = 1_000_000u32; // bytes per second
        let mut parms = ratespec(rate);
        parms.extend(ratespec(0)); // peakrate
        parms.extend(u32s(&[20_000, ticks(5000, rate), 0])); // limit, buffer in ticks, mtu
        let mut o = nla(1, &parms); // TCA_TBF_PARMS
        o.extend(nla(6, &5000u32.to_ne_bytes())); // TCA_TBF_BURST
        nl.qdisc(i, TcHandle::ROOT, handle(0, 0), "tbf", Some(o), false)
    });

    // ---- htb: a root, a class, a leaf
    {
        let _ = tc(&["qdisc", "del", "dev", "ta", "root"]);
        let _ = tc(&["qdisc", "del", "dev", "na", "root"]);
        let by_tc = [
            vec!["qdisc", "add", "dev", "ta", "root", "handle", "1:", "htb", "default", "1"],
            vec!["class", "add", "dev", "ta", "parent", "1:", "classid", "1:1", "htb", "rate", "8mbit", "ceil", "8mbit", "burst", "1600", "cburst", "1600"],
            vec!["qdisc", "add", "dev", "ta", "parent", "1:1", "handle", "10:", "pfifo", "limit", "50"],
        ];
        for args in &by_tc {
            tc(args).expect("tc htb");
        }
        let index = index_of("na");
        let rate = 1_000_000u32;
        let result = (|| -> Result<(), String> {
            let glob = u32s(&[3, 10, 1, 0, 0]); // tc_htb_glob: version, rate2quantum, defcls, debug, direct_pkts
            nl.qdisc(index, TcHandle::ROOT, handle(1, 0), "htb", Some(nla(2, &glob)), false)?; // TCA_HTB_INIT
            let mut opt = ratespec(rate);
            opt.extend(ratespec(rate));
            opt.extend(u32s(&[ticks(1600, rate), ticks(1600, rate), 0, 0, 0])); // buffer, cbuffer, quantum, level, prio
            nl.class(index, handle(1, 0), handle(1, 1), "htb", nla(1, &opt))?; // TCA_HTB_PARMS
            nl.qdisc(index, handle(1, 1), handle(0x10, 0), "pfifo", Some(u32s(&[50])), false)
        })();
        match result {
            Err(e) => verdict("H1", "htb root, class 8 mbit, pfifo leaf", false, &format!("netlink: {e}")),
            Ok(()) => {
                let (a, b) = (tc_dump("ta"), tc_dump("na"));
                let (c, d) = (tc_dump_class("ta"), tc_dump_class("na"));
                verdict("H1", "htb root, class 8 mbit, pfifo leaf", a == b && c == d, &format!("tc:      {a} / {c}\n         netlink: {b} / {d}"));
            }
        }
    }

    // ---- a change of a running netem, and its cost
    {
        let _ = tc(&["qdisc", "del", "dev", "na", "root"]);
        let index = index_of("na");
        let base = Netem { delay_us: 20_000, jitter_us: 1_000, limit: 1000, loss_percent: 1.0, seed: Some(5), ..Netem::default() };
        nl.qdisc(index, TcHandle::ROOT, handle(0, 0), "netem", Some(base.encode()), false).unwrap();
        let rounds = 20_000u32;
        let started = Instant::now();
        let mut failed = 0;
        for round in 0..rounds {
            let config = Netem { delay_us: 20_000 + u64::from(round % 100), ..base.clone() };
            if nl.qdisc(index, TcHandle::ROOT, handle(0, 0), "netem", Some(config.encode()), true).is_err() {
                failed += 1;
            }
        }
        let each = started.elapsed().as_secs_f64() / f64::from(rounds) * 1e6;
        verdict("L1", &format!("{rounds} changes of a running netem took {each:.1} microseconds each"), failed == 0, &format!("{failed} changes failed"));
    }
    let _ = TcOption::Other(DefaultNla::new(0, vec![]));
    let _ = AddressFamily::Inet;
}
