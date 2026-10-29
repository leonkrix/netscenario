//! Spike S7: can a netlink library replace the calls to `ip` and `tc`?
//!
//! Runs inside `unshare -Urn` (see `run.sh`), so the host network is never touched. It builds the same
//! pair of namespaces as `spikes/lib.sh` (`pair_up`) with `nlink`, configures `netem` through netlink,
//! and compares the result with what the `tc` command makes of the same settings.
//!
//! Checks print `PASS`, `FAIL` or `INFO` lines like the shell spikes.

use std::net::Ipv4Addr;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nlink::netlink::addr::Ipv4Address;
use nlink::netlink::link::VethLink;
use nlink::netlink::namespace::NetnsRef;
use nlink::netlink::tc::{NetemConfig, NetemLossModel};
use nlink::{Connection, Packets, Percent, Rate, Route};

/// The first 26 losses of `netem loss 20% seed 1234` plus the tail, as measured by S2 with `tc`
/// (iproute2 6.15, 7.3.0) on three kernels. Only valid in a quiet namespace and 200 pings.
const EXPECTED_SEED_1234: &str = "4 5 35 39 42 44 50 52 67 71 79 81 82 85 87 90 92 93 100 105 138 150 153 158 166 168 169 172 174 178 183 187 196";

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn pass(id: &str, text: &str) {
    println!("PASS {id:<7} {text}");
}
fn fail(id: &str, text: &str) {
    println!("FAIL {id:<7} {text}");
}
fn info(id: &str, text: &str) {
    println!("INFO {id:<7} {text}");
}

/// Runs a command with `/usr/sbin` in the PATH (tc lives there on Debian) and returns stdout.
fn run(program: &str, args: &[&str]) -> Res<String> {
    let path = format!("{}:/usr/sbin:/sbin", std::env::var("PATH").unwrap_or_default());
    let out = Command::new(program).args(args).env("PATH", path).output()?;
    if !out.status.success() {
        return Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `tc qdisc show dev v0` without the seed (a random seed differs between two settings).
fn qdisc_text() -> Res<String> {
    let text = run("tc", &["qdisc", "show", "dev", "v0"])?;
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut kept = Vec::new();
    let mut skip = false;
    for (i, w) in words.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        // "root refcnt 2" and the handle ("8001:") change between two runs
        if *w == "seed" || *w == "refcnt" {
            skip = true;
            continue;
        }
        if i == 2 && w.ends_with(':') {
            continue;
        }
        kept.push(*w);
    }
    Ok(kept.join(" "))
}

/// A second network namespace held by a process (no `ip netns`, which needs a mount namespace).
fn start_peer_namespace() -> Res<Child> {
    let child = Command::new("unshare")
        .args(["-n", "sleep", "3600"])
        .stdout(Stdio::null())
        .spawn()?;
    for _ in 0..50 {
        let ready = Command::new("nsenter")
            .args(["-t", &child.id().to_string(), "-n", "true"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ready {
            return Ok(child);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("the second namespace did not start".into())
}

fn mac_of(conn_links: &[nlink::netlink::messages::LinkMessage], name: &str) -> Option<[u8; 6]> {
    conn_links
        .iter()
        .find(|l| l.name() == Some(name))
        .and_then(|l| l.address())
        .and_then(|a| a.try_into().ok())
}

/// The same set-up as `pair_up` in spikes/lib.sh, with netlink instead of `ip`.
async fn pair_up(peer_pid: u32) -> Res<(Connection<Route>, Connection<Route>)> {
    // IPv6 off before the interfaces exist, in both namespaces: a stray packet shifts the loss pattern.
    for dir in ["default", "all"] {
        std::fs::write(format!("/proc/sys/net/ipv6/conf/{dir}/disable_ipv6"), "1")?;
    }
    run(
        "nsenter",
        &["-t", &peer_pid.to_string(), "-n", "sysctl", "-qw", "net.ipv6.conf.default.disable_ipv6=1"],
    )?;
    run(
        "nsenter",
        &["-t", &peer_pid.to_string(), "-n", "sysctl", "-qw", "net.ipv6.conf.all.disable_ipv6=1"],
    )?;

    let a = Connection::<Route>::new()?;
    let b = Connection::<Route>::new_in(NetnsRef::Pid(peer_pid))?;

    a.add_link(VethLink::new("v0", "v1")).await?;
    a.set_link_netns_pid("v1", peer_pid).await?;
    a.add_address(Ipv4Address::new("v0", Ipv4Addr::new(10, 9, 0, 1), 24)).await?;
    b.add_address(Ipv4Address::new("v1", Ipv4Addr::new(10, 9, 0, 2), 24)).await?;

    // Fixed neighbour entries instead of ARP
    let links_a = a.get_links().await?;
    let links_b = b.get_links().await?;
    let mac0 = mac_of(&links_a, "v0").ok_or("no MAC for v0")?;
    let mac1 = mac_of(&links_b, "v1").ok_or("no MAC for v1")?;
    let idx0 = links_a.iter().find(|l| l.name() == Some("v0")).ok_or("no v0")?.ifindex();
    let idx1 = links_b.iter().find(|l| l.name() == Some("v1")).ok_or("no v1")?.ifindex();
    a.add_neighbor_v4_by_index(idx0, Ipv4Addr::new(10, 9, 0, 2), mac1).await?;
    b.add_neighbor_v4_by_index(idx1, Ipv4Addr::new(10, 9, 0, 1), mac0).await?;

    a.set_link_up("v0").await?;
    a.set_link_up("lo").await?;
    b.set_link_up("v1").await?;
    b.set_link_up("lo").await?;
    Ok((a, b))
}

fn ping_lost(count: u32) -> Res<String> {
    let out = Command::new("ping")
        .args(["-c", &count.to_string(), "-i", "0.01", "-W", "1", "10.9.0.2"])
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let seen: std::collections::BTreeSet<u32> = text
        .lines()
        .filter_map(|l| l.split("icmp_seq=").nth(1))
        .filter_map(|r| r.split_whitespace().next())
        .filter_map(|n| n.parse().ok())
        .collect();
    if seen.len() < 100 {
        return Ok(format!("INVALID(only {} replies)", seen.len()));
    }
    Ok((1..=count).filter(|n| !seen.contains(n)).map(|n| n.to_string()).collect::<Vec<_>>().join(" "))
}

/// Applies `config` through netlink, reads it back with `tc`, then applies the equivalent `tc` command
/// line and reads that back. Returns both texts; equal text means that netlink and `tc` made the same qdisc.
async fn compare_with_tc(a: &Connection<Route>, config: NetemConfig, tc_args: &[&str]) -> Res<(String, String)> {
    let _ = run("tc", &["qdisc", "del", "dev", "v0", "root"]);
    a.add_qdisc("v0", config).await?;
    let via_netlink = qdisc_text()?;
    run("tc", &["qdisc", "del", "dev", "v0", "root"])?;
    let mut args = vec!["qdisc", "add", "dev", "v0", "root", "netem"];
    args.extend_from_slice(tc_args);
    run("tc", &args)?;
    Ok((via_netlink, qdisc_text()?))
}

async fn same_as_tc(a: &Connection<Route>, id: &str, text: &str, config: NetemConfig, tc_args: &[&str]) {
    match compare_with_tc(a, config, tc_args).await {
        Ok((n, t)) if n == t => pass(id, &format!("{text}  [{n}]")),
        Ok((n, t)) => fail(id, &format!("{text} :: netlink '{n}' but tc '{t}'")),
        Err(e) => fail(id, &format!("{text} :: {e}")),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Res<()> {
    info("S7.0", &format!("nlink 0.33.0, rtnetlink 0.23.0; tc: {}", run("tc", &["-V"])?.trim()));

    let mut peer = start_peer_namespace()?;
    let started = Instant::now();
    let (a, _b) = match pair_up(peer.id()).await {
        Ok(pair) => pair,
        Err(e) => {
            fail("S7.1", &format!("pair of namespaces with netlink :: {e}"));
            let _ = peer.kill();
            return Ok(());
        }
    };
    let setup = started.elapsed();
    match Command::new("ping").args(["-c", "2", "-W", "1", "10.9.0.2"]).output() {
        Ok(o) if o.status.success() => pass(
            "S7.1",
            &format!("veth pair, move to the other namespace, addresses, neighbours, up: ping works (set-up {setup:?})"),
        ),
        _ => fail("S7.1", "ping does not work after the set-up with netlink"),
    }

    // Does the seed give the same losses as tc did in S2?
    let _ = run("tc", &["qdisc", "del", "dev", "v0", "root"]);
    let seeded = NetemConfig::new().loss(Percent::new(20.0)).seed(1234).build();
    match a.add_qdisc("v0", seeded).await {
        Ok(()) => {
            let lost = ping_lost(200)?;
            info("S7.2", &format!("lost with seed 1234 set by netlink: {lost}"));
            if lost == EXPECTED_SEED_1234 {
                pass("S7.2", "netlink seed 1234 gives exactly the pattern that tc gave in S2");
            } else {
                fail("S7.2", "netlink seed 1234 differs from the pattern of tc in S2 (or the namespace was not quiet)");
            }
        }
        Err(e) => fail("S7.2", &format!("netem with seed through netlink :: {e}")),
    }

    // Each setting through netlink must produce the qdisc that tc produces from the command line.
    same_as_tc(&a, "S7.3", "delay 20ms",
        NetemConfig::new().delay(Duration::from_millis(20)).build(), &["delay", "20ms"]).await;
    same_as_tc(&a, "S7.4", "delay 40ms 10ms distribution normal",
        NetemConfig::new().delay(Duration::from_millis(40)).jitter(Duration::from_millis(10))
            .distribution(nlink::netlink::tc::NetemDistribution::named("normal").expect("normal.dist")).build(),
        &["delay", "40ms", "10ms", "distribution", "normal"]).await;
    same_as_tc(&a, "S7.5", "loss 2%",
        NetemConfig::new().loss(Percent::new(2.0)).build(), &["loss", "2%"]).await;
    same_as_tc(&a, "S7.6", "loss gemodel 2% 30% 50% 0.1%",
        NetemConfig::new().loss_model(
            NetemLossModel::gilbert_elliot(Percent::new(2.0)).r(Percent::new(30.0))
                .loss_in_bad(Percent::new(50.0)).loss_in_good(Percent::new(0.1))).build(),
        &["loss", "gemodel", "2%", "30%", "50%", "0.1%"]).await;
    same_as_tc(&a, "S7.7", "loss state 2% 30% 10% 20% 0.5%",
        NetemConfig::new().loss_model(
            NetemLossModel::gilbert_intuitive(Percent::new(2.0)).p31(Percent::new(30.0))
                .p32(Percent::new(10.0)).p23(Percent::new(20.0)).p14(Percent::new(0.5))).build(),
        &["loss", "state", "2%", "30%", "10%", "20%", "0.5%"]).await;
    same_as_tc(&a, "S7.8", "delay 10ms reorder 25% 50%",
        NetemConfig::new().delay(Duration::from_millis(10))
            .reorder(Percent::new(25.0)).reorder_correlation(Percent::new(50.0)).build(),
        &["delay", "10ms", "reorder", "25%", "50%"]).await;
    same_as_tc(&a, "S7.9", "duplicate 1%",
        NetemConfig::new().duplicate(Percent::new(1.0)).build(), &["duplicate", "1%"]).await;
    same_as_tc(&a, "S7.A", "corrupt 0.1%",
        NetemConfig::new().corrupt(Percent::new(0.1)).build(), &["corrupt", "0.1%"]).await;
    same_as_tc(&a, "S7.B", "delay 10ms rate 10mbit limit 500",
        NetemConfig::new().delay(Duration::from_millis(10)).rate(Rate::mbit(10)).limit(Packets::new(500)).build(),
        &["delay", "10ms", "rate", "10mbit", "limit", "500"]).await;

    // Canary: the comparison must be able to fail. Netlink says 20 ms, the tc line says 30 ms.
    match compare_with_tc(&a, NetemConfig::new().delay(Duration::from_millis(20)).build(), &["delay", "30ms"]).await {
        Ok((n, t)) if n != t => pass("S7.Z", "canary: a deliberately different setting is detected"),
        _ => fail("S7.Z", "canary: the comparison did not notice a different setting"),
    }

    // How fast does a running netem change through netlink (S5 measured 1127 us per tc process, 11 us in a batch)?
    let _ = run("tc", &["qdisc", "del", "dev", "v0", "root"]);
    a.add_qdisc("v0", NetemConfig::new().delay(Duration::from_millis(10)).build()).await?;
    let n: u32 = 3000;
    let t = Instant::now();
    for i in 0..n {
        let cfg = NetemConfig::new()
            .delay(Duration::from_millis(u64::from(10 + i % 50)))
            .loss(Percent::new(f64::from(i % 5)))
            .build();
        a.change_qdisc("v0", nlink::TcHandle::ROOT, cfg).await?;
    }
    let per = t.elapsed().as_micros() as f64 / f64::from(n);
    info("S7.C", &format!("netlink change of a running netem: {per:.1} microseconds per change ({n} changes, one connection)"));
    if per < 200.0 {
        pass("S7.C", "a trace can change a link much faster than with one tc process per change");
    } else {
        fail("S7.C", "changing through netlink is not faster than expected");
    }

    let _ = peer.kill();
    let _ = peer.wait();
    Ok(())
}
