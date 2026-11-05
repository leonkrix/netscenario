//! The work inside the namespaces: nodes as network namespaces held by a process, links as virtual Ethernet
//! pairs, impairments as `netem`. It runs in the supervisor, which is the first process of a PID namespace and
//! lives in a network namespace of its own (D-022), so nothing here can reach the network of the host (SAF-01).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use netscenario_model::Impairments;

use crate::batch::{ANSWER_WITHIN, Batch, Failure};
use crate::command::{Toolbox, run, run_with_env};
use crate::netem;
use crate::programs::{Output, read_stderr, read_stdout, socket_is_bound};
use crate::protocol::{OutputCounts, Way};
use crate::tables::TableDirectory;

/// The most links a run can address.
const MAX_LINKS: usize = 200;

/// The MTU of a link that does not say otherwise (D-081).
const DEFAULT_MTU: u32 = 1500;

/// The header of an Ethernet frame, which the largest segment of a device counts besides the MTU.
const ETHERNET_HEADER: u32 = 14;

/// The address of one end of a link: link `n` uses `10.100.n.0/30`, the first node `.1` and the second `.2`.
/// The caller and the supervisor both compute it, so that the caller needs no question to the supervisor.
pub(crate) fn interface_name(link: u32) -> String {
    format!("l{link}")
}

/// The address of one end of a link: link `n` uses `10.100.n.0/30`, the first node `.1` and the second `.2`.
pub(crate) fn link_address(link: u32, end: usize) -> std::net::Ipv4Addr {
    let third = u8::try_from(link).unwrap_or(u8::MAX);
    let host = if end == 0 { 1 } else { 2 };
    std::net::Ipv4Addr::new(10, 100, third, host)
}

/// The IPv6 address of one end of a link: link `n` uses `fd00:100:n::/126`, the first node `::1` and the second `::2` (D-081).
pub(crate) fn link_address_v6(link: u32, end: usize) -> std::net::Ipv6Addr {
    let link = u16::try_from(link).unwrap_or(u16::MAX);
    std::net::Ipv6Addr::new(
        0xfd00,
        0x100,
        link,
        0,
        0,
        0,
        0,
        if end == 0 { 1 } else { 2 },
    )
}

/// How long a network with IPv6 waits for the packets that its interfaces send when they come up (neighbour discovery,
/// multicast listener reports): they are over within a second (spike S16), and the first queue is built after (D-081).
const QUIET_WAIT: Duration = Duration::from_millis(1500);

/// A failpoint of the crash tests: `NETSCENARIO_FAIL_AT=link:created` ends the supervisor, without cleaning
/// up, the first time it passes that point; `link:created#2` the second time. It exists to test the cleanup
/// guarantee (BKD-04) and does nothing when the variable is not set.
#[derive(Debug, Default)]
pub(crate) struct Failpoints {
    wanted: Option<(String, usize)>,
    seen: HashMap<String, usize>,
}

impl Failpoints {
    pub(crate) fn new(wanted: Option<&str>) -> Self {
        let wanted = wanted.map(|text| match text.split_once('#') {
            Some((point, count)) => (point.to_owned(), count.parse().unwrap_or(1)),
            None => (text.to_owned(), 1),
        });
        Self {
            wanted,
            seen: HashMap::new(),
        }
    }

    pub(crate) fn hit(&mut self, point: &str) {
        let count = self.seen.entry(point.to_owned()).or_insert(0);
        *count += 1;
        if self.wanted.as_ref() == Some(&(point.to_owned(), *count)) {
            // A crash, not an orderly exit: no destructor runs and nothing is cleaned up
            std::process::abort();
        }
    }
}

struct Node {
    name: String,
    holder: Child,
    pid: u32,
}

/// One end of a link: the node and the name of the interface in it, and the fixed neighbours on it (the address and the
/// hardware address of the other end, one for each family), which the kernel removes when the interface goes down.
struct End {
    node: usize,
    interface: String,
    neighbours: Vec<[String; 2]>,
    /// The IPv6 address of the interface with its prefix length: the kernel removes it when the interface goes down (it keeps an
    /// IPv4 address), and with it the route to the other end, so it is put back at `up`.
    v6_address: Option<String>,
}

/// The fixed neighbours of one end of a new link and its IPv6 address with the prefix length.
type EndAddresses = (Vec<[String; 2]>, Option<String>);

/// What `set_link` needs of one end of a link: the process of the node, the interface, the fixed neighbours and the IPv6 address.
type LinkEnd = (u32, String, Vec<[String; 2]>, Option<String>);

struct Link {
    ends: [End; 2],
    /// The ports of the fabric of a link built through it (D-079): the one towards the first node and the one towards the second.
    fabric: Option<[String; 2]>,
}

/// A route that was added to a node (D-078), kept so that it can be added again: the kernel removes the routes that lead out
/// of an interface when the interface goes down, and does not bring them back when it comes up.
struct Route {
    node: u32,
    v6: bool,
    destination: Option<String>,
    metric: u32,
    next_hops: Vec<crate::protocol::WireHop>,
}

/// How long a program that has ended may take to deliver the rest of its output, if a process that it started
/// keeps the pipe open.
const OUTPUT_GRACE: Duration = Duration::from_millis(500);

/// A program in a node, with the threads that read its standard output and standard error.
struct RunningProgram {
    child: Child,
    stdin: Option<ChildStdin>,
    output: Arc<Output>,
    /// Whether it leads a process group of its own.
    own_group: bool,
}

/// What a poll reports.
pub(crate) struct ProgramReport {
    pub(crate) lines: Vec<String>,
    pub(crate) exit: Option<crate::protocol::Exit>,
    pub(crate) stderr: String,
    pub(crate) truncated: bool,
    pub(crate) output: OutputCounts,
}

/// What a program is started with (D-072).
pub(crate) struct ProgramSpec {
    pub(crate) argv: Vec<String>,
    pub(crate) memory_limit: Option<u64>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) cwd: Option<String>,
    pub(crate) own_group: bool,
    pub(crate) stdout_file: Option<String>,
    pub(crate) stderr_file: Option<String>,
}

pub(crate) struct Provisioner {
    tools: Toolbox,
    /// This program, which also holds the namespaces of the nodes (`hold`).
    exe: PathBuf,
    run_id: String,
    max_nodes: usize,
    failpoints: Failpoints,
    nodes: Vec<Node>,
    links: Vec<Link>,
    /// The process that holds the namespace of the fabric, once a link needs it (D-079). The holder is in `nodes` as well, after the
    /// nodes of the scenario, so that the teardown ends it with them.
    fabric: Option<u32>,
    /// The directions whose queues are the tree of a queue (D-082), by link and direction.
    queued: std::collections::HashSet<(u32, Way)>,
    /// Which addresses the network has (D-081): IPv4 only until the run says otherwise.
    v4: bool,
    v6: bool,
    /// The routes of the nodes, in the order they were added, and the links that are down at the moment.
    routes: Vec<Route>,
    /// The address of each node on its loopback, by node and family (`true` for IPv6), which its routes name as the source.
    node_addresses: std::collections::HashMap<(u32, bool), String>,
    down: std::collections::HashSet<u32>,
    programs: Vec<RunningProgram>,
    /// The queues planned for the directions that change in the run (D-059), by link and direction.
    layouts: std::collections::HashMap<(u32, Way), netem::Layout>,
    /// The directions that have queues, because `apply` built them. A direction that impairs nothing has none.
    built: std::collections::HashSet<(u32, Way)>,
    /// The `tc -batch` that makes the changes of the node whose holder has this process id (D-063). `None`: there is
    /// none, because it could not be started or stopped working, and a process makes the changes of that node.
    batches: HashMap<u32, Option<Batch>>,
    /// What the backend says about the run besides its result: where it did something other than the usual way.
    notes: Vec<String>,
    /// The tables of the jitter that the run made from samples, once there is one (D-084). Every `tc` of the run gets this
    /// directory as `TC_LIB_DIR` from then on.
    tables: Option<TableDirectory>,
}

fn id(index: usize) -> Result<u32, String> {
    u32::try_from(index).map_err(|_| "too many objects".to_owned())
}

impl Provisioner {
    pub(crate) fn new(
        tools: Toolbox,
        run_id: String,
        max_nodes: usize,
        failpoints: Failpoints,
    ) -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|error| format!("cannot find the supervisor's own program: {error}"))?;
        Ok(Self {
            tools,
            exe,
            run_id,
            max_nodes,
            failpoints,
            nodes: Vec::new(),
            links: Vec::new(),
            fabric: None,
            queued: std::collections::HashSet::new(),
            v4: true,
            v6: false,
            routes: Vec::new(),
            node_addresses: std::collections::HashMap::new(),
            down: std::collections::HashSet::new(),
            programs: Vec::new(),
            layouts: std::collections::HashMap::new(),
            built: std::collections::HashSet::new(),
            batches: HashMap::new(),
            notes: Vec::new(),
            tables: None,
        })
    }

    /// SAF-01: the supervisor touches the network only if it is the first process of a PID namespace and sees
    /// nothing but the loopback interface, which means it was started by `unshare` for this run and is not
    /// in the network namespace of the host.
    pub(crate) fn check_isolated(&mut self) -> Result<(), String> {
        let pid = std::process::id();
        if pid != 1 {
            return Err(format!(
                "the supervisor is process {pid}, not the first process of a PID namespace; it refuses to run (SAF-01)"
            ));
        }
        let listing = run(&self.tools.ip, &["-o", "link", "show"])?;
        let interfaces: Vec<&str> = listing
            .lines()
            .filter_map(|line| line.split(": ").nth(1))
            .map(|name| name.split('@').next().unwrap_or(name))
            .collect();
        if interfaces != ["lo"] {
            return Err(format!(
                "the supervisor sees the network interfaces {}: it is not in a network namespace of its own and refuses to touch the network (SAF-01)",
                interfaces.join(", ")
            ));
        }
        // New interfaces start without IPv6, so that no packet that nobody asked for shifts the loss pattern
        // of `netem` (spike S2, `docs/COVERAGE.md`)
        disable_ipv6()
    }

    /// Says which addresses the network has, before the first node is made (D-081).
    pub(crate) fn set_families(&mut self, v4: bool, v6: bool) -> Result<(), String> {
        if !self.nodes.is_empty() {
            return Err(
                "the addresses of the network are chosen before the first node is made".to_owned(),
            );
        }
        if !v4 && !v6 {
            return Err("a network needs addresses of at least one family".to_owned());
        }
        self.v4 = v4;
        self.v6 = v6;
        Ok(())
    }

    /// Writes kernel settings (`net.ipv4.ip_forward=1`) in the network namespace of a node, by the program itself (the
    /// hidden call `sysctl`).
    fn sysctl(&self, pid: u32, settings: &[&str]) -> Result<(), String> {
        let namespace = format!("--net=/proc/{pid}/ns/net");
        let exe = self.exe.to_string_lossy().into_owned();
        let mut arguments = vec![namespace.as_str(), "--", exe.as_str(), "sysctl"];
        arguments.extend_from_slice(settings);
        run(&self.tools.nsenter, &arguments).map(|_| ())
    }

    /// Waits until the packets that the interfaces of a network with IPv6 send of their own are over (D-081), so that the
    /// first queue counts only the packets of the workloads. A network with IPv4 only has none to wait for.
    pub(crate) fn finish_network(&mut self) {
        if self.v6 {
            self.failpoints.hit("quiet:start");
            std::thread::sleep(QUIET_WAIT);
            self.failpoints.hit("quiet:end");
        }
    }

    /// Starts the process that holds a new network namespace, and waits until it says that it does. The holder is put in `nodes`
    /// as soon as it is known, so that a failure leaves it to the teardown.
    fn spawn_holder(&mut self, name: &str) -> Result<u32, String> {
        let mut holder = Command::new(&self.tools.unshare)
            .args(["--net", "--"])
            .arg(&self.exe)
            .args(["hold", &self.run_id])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("cannot start the holder of the node '{name}': {error}"))?;
        let pid = holder.id();
        let stdout = holder.stdout.take();
        // From here on the holder is known, so a failure leaves it to the teardown
        self.nodes.push(Node {
            name: name.to_owned(),
            holder,
            pid,
        });
        self.failpoints.hit("node:spawned");
        let mut ready = String::new();
        if let Some(stdout) = stdout {
            BufReader::new(stdout)
                .read_line(&mut ready)
                .map_err(|error| format!("cannot read from the holder of '{name}': {error}"))?;
        }
        if ready.trim() != "ready" {
            return Err(format!(
                "the holder of the node '{name}' did not start (it said '{}')",
                ready.trim()
            ));
        }
        Ok(pid)
    }

    pub(crate) fn create_node(&mut self, name: &str) -> Result<u32, String> {
        self.failpoints.hit("node:start");
        if self.nodes.len() >= self.max_nodes {
            return Err(format!(
                "a run may use at most {} network namespaces (SAF-02) and the scenario needs more",
                self.max_nodes
            ));
        }
        if self.nodes.iter().any(|node| node.name == name) {
            return Err(format!("the node '{name}' exists already"));
        }
        let pid = self.spawn_holder(name)?;
        self.in_node(pid, &self.tools.ip, &["link", "set", "lo", "up"])?;
        // A route through an interface whose carrier is gone is not used (the other path takes over, D-078), and a packet is
        // not dropped because it arrives on another interface than the one the reverse route leaves by
        let namespace = format!("--net=/proc/{pid}/ns/net");
        let exe = self.exe.to_string_lossy().into_owned();
        run(
            &self.tools.nsenter,
            &[
                namespace.as_str(),
                "--",
                exe.as_str(),
                "sysctl",
                "net.ipv4.conf.all.ignore_routes_with_linkdown=1",
                "net.ipv4.conf.all.rp_filter=0",
                "net.ipv4.conf.default.rp_filter=0",
            ],
        )?;
        if self.v6 {
            // IPv6 in a namespace is not quiet: a node of a network with IPv6 has no duplicate address detection, no router
            // advertisements and no solicitations, no autoconfiguration, so that the only packets it sends of its own are the
            // few that the wait of `finish_network` lets pass. The settings are those for the interfaces that come later
            // (`default`), and for the ones that are there (`all`, `lo`).
            let mut settings: Vec<String> = Vec::new();
            for scope in ["default", "all", "lo"] {
                for (key, value) in [
                    ("disable_ipv6", "0"),
                    ("accept_dad", "0"),
                    ("dad_transmits", "0"),
                    ("accept_ra", "0"),
                    ("autoconf", "0"),
                    ("router_solicitations", "0"),
                ] {
                    settings.push(format!("net.ipv6.conf.{scope}.{key}={value}"));
                }
            }
            settings.push("net.ipv6.conf.all.ignore_routes_with_linkdown=1".to_owned());
            let settings: Vec<&str> = settings.iter().map(String::as_str).collect();
            self.sysctl(pid, &settings)?;
        }
        self.failpoints.hit("node:end");
        id(self.nodes.len() - 1)
    }

    /// Gives both ends of a new link the addresses of each family the network has, fixes the neighbours, and brings the
    /// interfaces up. Returns the fixed neighbours of each end.
    fn address_the_ends(
        &self,
        number: u32,
        pids: [u32; 2],
        inside: &str,
        macs: &[String; 2],
    ) -> Result<[EndAddresses; 2], String> {
        // The addresses of each family the network has, as (address, prefix length) for each end
        let mut families: Vec<[String; 2]> = Vec::new();
        let mut prefixes: Vec<&str> = Vec::new();
        if self.v4 {
            families.push([0, 1].map(|end| link_address(number, end).to_string()));
            prefixes.push("30");
        }
        if self.v6 {
            families.push([0, 1].map(|end| link_address_v6(number, end).to_string()));
            prefixes.push("126");
        }
        let mut neighbours: [(Vec<[String; 2]>, Option<String>); 2] =
            [(Vec::new(), None), (Vec::new(), None)];
        for end in 0..2 {
            let other = 1 - end;
            let pid = pids[end];
            for (addresses, prefix) in families.iter().zip(&prefixes) {
                let address = format!("{}/{prefix}", addresses[end]);
                // IPv6: no duplicate address detection (`nodad`), the address is there at once
                let mut add = vec!["addr", "add", address.as_str(), "dev", &inside];
                if addresses[end].contains(':') {
                    add.push("nodad");
                }
                self.in_node(pid, &self.tools.ip, &add)?;
                // The neighbour is fixed instead of learned: no ARP or neighbour solicitation may shift the loss pattern
                self.in_node(
                    pid,
                    &self.tools.ip,
                    &[
                        "neigh",
                        "replace",
                        &addresses[other],
                        "lladdr",
                        &macs[other],
                        "dev",
                        inside,
                        "nud",
                        "permanent",
                    ],
                )?;
                neighbours[end]
                    .0
                    .push([addresses[other].clone(), macs[other].clone()]);
                if addresses[end].contains(':') {
                    neighbours[end].1 = Some(address.clone());
                }
            }
            self.in_node(pid, &self.tools.ip, &["link", "set", inside, "up"])?;
            self.limit_segments(pid, inside, DEFAULT_MTU)?;
        }
        Ok(neighbours)
    }

    /// Keeps the stack of a node from building segments larger than one frame for this interface (D-086). With the offloads of the
    /// veth on, TCP hands `netem` a super-packet of up to 44 segments, which `netem` drops or delays as one: reno ran 36 to 79 %
    /// above the model of Mathis, and with the limit 0.97 to 1.16 times it (measured). `ethtool` would need a tool that the
    /// machine may not have; the size and the number of segments that the device accepts are properties of the link that `ip`
    /// sets.
    fn limit_segments(&self, pid: u32, interface: &str, mtu: u32) -> Result<(), String> {
        let size = (mtu + ETHERNET_HEADER).to_string();
        self.in_node(
            pid,
            &self.tools.ip,
            &[
                "link",
                "set",
                interface,
                "gso_max_size",
                &size,
                "gso_max_segs",
                "1",
            ],
        )?;
        Ok(())
    }

    /// A veth pair between two nodes, moved into them (the direct build of a link).
    fn join_directly(
        &mut self,
        outside: &[String; 2],
        inside: &str,
        macs: &[String; 2],
        pids: [u32; 2],
    ) -> Result<(), String> {
        run(
            &self.tools.ip,
            &[
                "link",
                "add",
                &outside[0],
                "address",
                &macs[0],
                "type",
                "veth",
                "peer",
                "name",
                &outside[1],
                "address",
                &macs[1],
            ],
        )?;
        self.failpoints.hit("link:created");
        for (end, pid) in pids.iter().enumerate() {
            run(
                &self.tools.ip,
                &[
                    "link",
                    "set",
                    &outside[end],
                    "netns",
                    &pid.to_string(),
                    "name",
                    inside,
                ],
            )?;
        }
        Ok(())
    }

    /// Two veth pairs, one from each node to the fabric, and in the fabric the redirect that sends every frame that arrives at
    /// one port out of the other (D-079). The frames cross unchanged, so the addresses, the MACs and the fixed neighbours of the
    /// two ends are as in the direct build. Returns the ports of the fabric: the one towards the first node and the one towards
    /// the second.
    fn join_through_fabric(
        &mut self,
        index: usize,
        outside: &[String; 2],
        inside: &str,
        macs: &[String; 2],
        pids: [u32; 2],
    ) -> Result<[String; 2], String> {
        let fabric = self.fabric_pid()?;
        let ports = [format!("nsf{index}a"), format!("nsf{index}b")];
        run(
            &self.tools.ip,
            &[
                "link",
                "add",
                &outside[0],
                "address",
                &macs[0],
                "type",
                "veth",
                "peer",
                "name",
                &ports[0],
            ],
        )?;
        run(
            &self.tools.ip,
            &[
                "link",
                "add",
                &ports[1],
                "type",
                "veth",
                "peer",
                "name",
                &outside[1],
                "address",
                &macs[1],
            ],
        )?;
        self.failpoints.hit("link:created");
        for (end, pid) in pids.iter().enumerate() {
            run(
                &self.tools.ip,
                &[
                    "link",
                    "set",
                    &outside[end],
                    "netns",
                    &pid.to_string(),
                    "name",
                    inside,
                ],
            )?;
        }
        for port in &ports {
            run(
                &self.tools.ip,
                &["link", "set", port, "netns", &fabric.to_string()],
            )?;
        }
        for (from, to) in [(0, 1), (1, 0)] {
            self.in_node(fabric, &self.tools.ip, &["link", "set", &ports[from], "up"])?;
            self.in_node(
                fabric,
                &self.tools.tc,
                &["qdisc", "add", "dev", &ports[from], "ingress"],
            )?;
            self.in_node(
                fabric,
                &self.tools.tc,
                &[
                    "filter",
                    "add",
                    "dev",
                    &ports[from],
                    "parent",
                    "ffff:",
                    "protocol",
                    "all",
                    "u32",
                    "match",
                    "u32",
                    "0",
                    "0",
                    "action",
                    "mirred",
                    "egress",
                    "redirect",
                    "dev",
                    &ports[to],
                ],
            )?;
        }
        Ok(ports)
    }

    /// The process that holds the namespace of the fabric: made when the first link needs it (D-079).
    fn fabric_pid(&mut self) -> Result<u32, String> {
        if let Some(pid) = self.fabric {
            return Ok(pid);
        }
        self.failpoints.hit("fabric:start");
        let pid = self.spawn_holder("(fabric)")?;
        self.fabric = Some(pid);
        self.failpoints.hit("fabric:end");
        Ok(pid)
    }

    pub(crate) fn create_link(
        &mut self,
        name: &str,
        between: [u32; 2],
        through_fabric: bool,
    ) -> Result<u32, String> {
        self.failpoints.hit("link:start");
        let index = self.links.len();
        if index >= MAX_LINKS {
            return Err(format!("a run may have at most {MAX_LINKS} links"));
        }
        let [first, second] = between.map(|node| node as usize);
        if first == second || first >= self.nodes.len() || second >= self.nodes.len() {
            return Err(format!(
                "the link '{name}' needs two different nodes of this run"
            ));
        }
        let outside = [format!("nsc{index}a"), format!("nsc{index}b")];
        let inside = interface_name(u32::try_from(index).unwrap_or(u32::MAX));
        let macs = [0, 1].map(|end| format!("02:4e:53:00:{index:02x}:{:02x}", end + 1));
        let pids = [self.nodes[first].pid, self.nodes[second].pid];
        let ports = if through_fabric {
            Some(self.join_through_fabric(index, &outside, &inside, &macs, pids)?)
        } else {
            self.join_directly(&outside, &inside, &macs, pids)?;
            None
        };
        self.failpoints.hit("link:moved");
        let neighbours = self.address_the_ends(id(index).unwrap_or(0), pids, &inside, &macs)?;
        let [(first_neighbours, first_v6), (second_neighbours, second_v6)] = neighbours;
        self.links.push(Link {
            fabric: ports,
            ends: [
                End {
                    node: first,
                    interface: inside.clone(),
                    neighbours: first_neighbours,
                    v6_address: first_v6,
                },
                End {
                    node: second,
                    interface: inside,
                    neighbours: second_neighbours,
                    v6_address: second_v6,
                },
            ],
        });
        self.failpoints.hit("link:end");
        id(index)
    }

    /// Gives a node an address on its loopback (D-078): the address that the routes of the other nodes lead to.
    pub(crate) fn set_node_address(&mut self, node: u32, address: &str) -> Result<(), String> {
        let pid = self.node_pid(node)?;
        self.failpoints.hit("address:start");
        if address.contains(':') {
            self.in_node(
                pid,
                &self.tools.ip,
                &[
                    "addr",
                    "add",
                    &format!("{address}/128"),
                    "dev",
                    "lo",
                    "nodad",
                ],
            )?;
        } else {
            self.in_node(
                pid,
                &self.tools.ip,
                &["addr", "add", &format!("{address}/32"), "dev", "lo"],
            )?;
        }
        self.node_addresses
            .insert((node, address.contains(':')), address.to_owned());
        self.failpoints.hit("address:end");
        Ok(())
    }

    /// Sets the MTU of the interfaces at both ends of a link (D-081). The kernel drops a frame that is longer than the MTU of the
    /// interface that receives it, so the two ends are always the same.
    pub(crate) fn set_mtu(&mut self, link: u32, mtu: u32) -> Result<(), String> {
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        let ends: Vec<(u32, String)> = link
            .ends
            .iter()
            .map(|end| (self.nodes[end.node].pid, end.interface.clone()))
            .collect();
        self.failpoints.hit("mtu:start");
        for (pid, interface) in &ends {
            self.in_node(
                *pid,
                &self.tools.ip,
                &["link", "set", interface, "mtu", &mtu.to_string()],
            )?;
            self.limit_segments(*pid, interface, mtu)?;
        }
        self.failpoints.hit("mtu:end");
        Ok(())
    }

    /// Makes a node forward packets between its links, or not (D-078).
    pub(crate) fn set_forwarding(&mut self, node: u32, on: bool) -> Result<(), String> {
        let pid = self.node_pid(node)?;
        self.failpoints.hit("forwarding:start");
        // Both families, when the network has IPv6: a host must not carry IPv6 packets either
        let v4 = format!("net.ipv4.ip_forward={}", u8::from(on));
        let v6 = format!("net.ipv6.conf.all.forwarding={}", u8::from(on));
        if self.v6 {
            self.sysctl(pid, &[v4.as_str(), v6.as_str()])?;
        } else {
            self.sysctl(pid, &[v4.as_str()])?;
        }
        self.failpoints.hit("forwarding:end");
        Ok(())
    }

    /// Adds a route to a node (D-078): to an address (or every address) out of one or several links, with the other end of
    /// each link as the next hop. Several ways form a multipath route shared by their weights.
    pub(crate) fn add_route(
        &mut self,
        node: u32,
        v6: bool,
        destination: Option<&str>,
        metric: u32,
        next_hops: &[crate::protocol::WireHop],
    ) -> Result<(), String> {
        self.failpoints.hit("route:start");
        let route = Route {
            node,
            v6,
            destination: destination.map(str::to_owned),
            metric,
            next_hops: next_hops.to_vec(),
        };
        self.install(&route, "add", &route.next_hops)?;
        self.routes.push(route);
        self.failpoints.hit("route:end");
        Ok(())
    }

    /// Runs `ip route <verb>` for a route with the given ways out (all of its own, or those that are up).
    fn install(
        &self,
        route: &Route,
        verb: &str,
        next_hops: &[crate::protocol::WireHop],
    ) -> Result<(), String> {
        let node = route.node;
        let pid = self.node_pid(node)?;
        if next_hops.is_empty() {
            return Err("a route needs a way out".to_owned());
        }
        let mut arguments: Vec<String> = Vec::new();
        if route.v6 {
            arguments.push("-6".into());
        }
        arguments.extend(["route".to_owned(), verb.to_owned()]);
        let host_prefix = if route.v6 { 128 } else { 32 };
        arguments.push(
            route
                .destination
                .as_ref()
                .map_or_else(|| "default".to_owned(), |a| format!("{a}/{host_prefix}")),
        );
        arguments.extend(["metric".to_owned(), route.metric.to_string()]);
        // A packet that the node sends over the route has the address of the node as its source, not the address of the link it
        // leaves by: the other nodes have a route to the node address only, so a reply to a link address would have none
        // (a TCP connection through a router never got its answer)
        if let Some(address) = self.node_addresses.get(&(node, route.v6)) {
            arguments.extend(["src".to_owned(), address.clone()]);
        }
        for hop in next_hops {
            let link = self
                .links
                .get(hop.link as usize)
                .ok_or_else(|| format!("the link {} does not exist in this run", hop.link))?;
            let end = link
                .ends
                .get(hop.end)
                .ok_or_else(|| format!("a link has two ends, not {}", hop.end + 1))?;
            if end.node != node as usize {
                return Err(format!("the link {} is not at the node {node}", hop.link));
            }
            let gateway = if route.v6 {
                link_address_v6(hop.link, 1 - hop.end).to_string()
            } else {
                link_address(hop.link, 1 - hop.end).to_string()
            };
            if next_hops.len() > 1 {
                arguments.push("nexthop".into());
            }
            arguments.extend([
                "via".to_owned(),
                gateway,
                "dev".to_owned(),
                end.interface.clone(),
            ]);
            if next_hops.len() > 1 {
                arguments.extend(["weight".to_owned(), hop.weight.to_string()]);
            }
        }
        let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
        if next_hops.len() > 1 {
            // The kernel chooses a way of a multipath route by the addresses only unless told to look at the ports as well,
            // so every flow between two nodes would take the same way; with the ports the flows share the ways (D-078)
            let namespace = format!("--net=/proc/{pid}/ns/net");
            let exe = self.exe.to_string_lossy().into_owned();
            run(
                &self.tools.nsenter,
                &[
                    namespace.as_str(),
                    "--",
                    exe.as_str(),
                    "sysctl",
                    if route.v6 {
                        "net.ipv6.fib_multipath_hash_policy=1"
                    } else {
                        "net.ipv4.fib_multipath_hash_policy=1"
                    },
                ],
            )?;
        }
        self.in_node(pid, &self.tools.ip, &words).map(|_| ())
    }

    /// Puts the routes back that lead out of a link that came up. The kernel removed them when the interface went down (the
    /// address and the route to the other end of the link it keeps), and a route is put back with the ways out that are up
    /// now: the link that is still down of a multipath route comes with its own `up`.
    fn restore_routes(&self, link: u32) -> Result<(), String> {
        for route in &self.routes {
            if !route.next_hops.iter().any(|hop| hop.link == link) {
                continue;
            }
            let ways: Vec<crate::protocol::WireHop> = route
                .next_hops
                .iter()
                .filter(|hop| !self.down.contains(&hop.link))
                .copied()
                .collect();
            if !ways.is_empty() {
                self.install(route, "replace", &ways)?;
            }
        }
        Ok(())
    }

    fn node_pid(&self, node: u32) -> Result<u32, String> {
        self.nodes
            .get(node as usize)
            .map(|node| node.pid)
            .ok_or_else(|| format!("the node {node} does not exist in this run"))
    }

    /// Plans the queues of a direction for every state it will have (D-059). It must come before `apply`, which
    /// builds them.
    pub(crate) fn plan(
        &mut self,
        link: u32,
        way: Way,
        states: &[Impairments],
    ) -> Result<(), String> {
        if self.links.get(link as usize).is_none() {
            return Err(format!("the link {link} does not exist in this run"));
        }
        let layout = netem::Layout::of(states, false)?;
        self.layouts.insert((link, way), layout);
        Ok(())
    }

    /// Adds the blackout queue of an outage (D-064) to the layout that `plan` made. It must come before `apply`, which
    /// builds the queues, and after `plan`.
    pub(crate) fn plan_outage(&mut self, link: u32, way: Way) -> Result<(), String> {
        let layout = self.layouts.remove(&(link, way)).ok_or_else(|| {
            format!("the direction of link {link} has to be planned with its states before it is planned for an outage")
        })?;
        self.layouts.insert((link, way), layout.with_blackout());
        Ok(())
    }

    /// Writes a state into the queues of a direction that `apply` built from a plan: the whole state, in the
    /// queues that are there, so no packet is lost to a rebuild and the random numbers go on (D-048, D-059).
    pub(crate) fn change(
        &mut self,
        link: u32,
        way: Way,
        impairments: &Impairments,
    ) -> Result<(), String> {
        let layout = self.layouts.get(&(link, way)).ok_or_else(|| {
            format!("the direction of link {link} was not planned for changes, so its queues may not fit the new state")
        })?;
        if !layout.fits(impairments) {
            return Err(
                "the new state needs a queue that this direction was not built with".to_owned(),
            );
        }
        let stages = netem::change(layout, impairments)?;
        let (pid, interface) = self.interface_of(link, way)?;
        self.run_lines(pid, &stages.lines(&interface, "change"))
    }

    /// Begins or ends an outage of a direction that `apply` built with a blackout queue (D-064): the loss of 100 %
    /// of that queue is switched on or off, and nothing else is touched.
    pub(crate) fn set_outage(&mut self, link: u32, way: Way, on: bool) -> Result<(), String> {
        let (pid, interface) = self.interface_of(link, way)?;
        let command = self
            .layouts
            .get(&(link, way))
            .and_then(|layout| layout.blackout_line(&interface, on))
            .ok_or_else(|| {
                format!("the direction of link {link} was not planned for an outage, so it has no queue to hold one")
            })?;
        self.run_lines(pid, &[command])
    }

    /// Makes the changes in `lines` in the node whose holder is `pid`: through the `tc` that lives for the run if
    /// the node has one that works, else by one process for each line. The arguments of a line are put into one
    /// text for the batch, so none may hold a space.
    fn run_lines(&mut self, pid: u32, lines: &[String]) -> Result<(), String> {
        if self.change_through_batch(pid, lines)? {
            return Ok(());
        }
        for line in lines {
            let words: Vec<&str> = line.split(' ').collect();
            self.in_node(pid, &self.tools.tc, &words)?;
        }
        Ok(())
    }

    /// Makes a change through the batch of the node, if it has one that works. `Ok(false)`: there is none, or it
    /// stopped working just now and has been ended, and the caller makes the change with a process (the change is
    /// the whole state, so doing it again is the same as doing it once).
    fn change_through_batch(&mut self, pid: u32, commands: &[String]) -> Result<bool, String> {
        let Some(Some(batch)) = self.batches.get_mut(&pid) else {
            return Ok(false);
        };
        match batch.run(commands, ANSWER_WITHIN) {
            Ok(()) => Ok(true),
            Err(Failure::Refused(message)) => Err(format!(
                "`tc` refused the change `{}`: {message}",
                commands.join("; ")
            )),
            Err(Failure::Broken(why)) => {
                batch.end();
                self.batches.insert(pid, None);
                self.notes.push(format!(
                    "the `tc` that makes the changes of a node stopped working ({why}); a `tc` process for each change did the rest, which takes about 2 ms instead of 0.03 ms"
                ));
                Ok(false)
            }
        }
    }

    /// Starts the batch of a node, once. A batch that cannot be started is a note and not an error: the changes
    /// are made by a process then, which is slower and just as exact.
    fn start_batch(&mut self, pid: u32) {
        if self.batches.contains_key(&pid) {
            return;
        }
        let tc = self.tools.tc.to_string_lossy().into_owned();
        let arguments = vec![
            format!("--net=/proc/{pid}/ns/net"),
            "--".to_owned(),
            tc,
            "-force".to_owned(),
            "-batch".to_owned(),
            "-".to_owned(),
        ];
        let directory = self
            .tables
            .as_ref()
            .map(|tables| tables.path().to_string_lossy().into_owned());
        let environment: Vec<(&str, &str)> = directory
            .iter()
            .map(|directory| ("TC_LIB_DIR", directory.as_str()))
            .collect();
        let batch = match Batch::start(&self.tools.nsenter, &arguments, &environment, ANSWER_WITHIN)
        {
            Ok(batch) => Some(batch),
            Err(why) => {
                self.notes.push(format!(
                    "the `tc` that makes the changes of a node could not be started ({why}); a `tc` process for each change did them, which takes about 2 ms instead of 0.03 ms"
                ));
                None
            }
        };
        self.batches.insert(pid, batch);
    }

    /// What the backend says about the run besides its result (D-063).
    pub(crate) fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }

    /// Takes both interfaces of a link down or brings them up (D-064). Down is the state of the interface, not the
    /// loss of the carrier: a sender on either side gets an error from the kernel while it lasts, and the addresses,
    /// the queues stay as they are. The neighbours (they are fixed, not learned) and the routes that lead out of the interface
    /// do not: the kernel removes them, so they are put back when the link comes up (D-064, D-078). The packets in the queues are
    /// dropped by the kernel when the interface goes down.
    pub(crate) fn set_link(&mut self, link: u32, up: bool) -> Result<(), String> {
        let link_index = link;
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        let state = if up { "up" } else { "down" };
        let ends: Vec<LinkEnd> = link
            .ends
            .iter()
            .map(|end| {
                (
                    self.nodes[end.node].pid,
                    end.interface.clone(),
                    end.neighbours.clone(),
                    end.v6_address.clone(),
                )
            })
            .collect();
        // One end after the other: for a moment a sender on the first side is refused and one on the second is not
        for (pid, interface, _, _) in &ends {
            self.in_node(*pid, &self.tools.ip, &["link", "set", interface, state])?;
        }
        if up {
            // The kernel removed the fixed neighbours with the down (a fixed one as well as a learned one); without them the
            // first packet in each direction waits for an ARP exchange that crosses the impaired link
            // IPv6: the kernel removed the address with the down (an IPv4 address stays), and the route to the other end of the link
            // with it, so the address comes first
            for (pid, interface, _, v6_address) in &ends {
                if let Some(address) = v6_address {
                    self.in_node(
                        *pid,
                        &self.tools.ip,
                        &["addr", "replace", address, "dev", interface, "nodad"],
                    )?;
                }
            }
            for (pid, interface, neighbours, _) in &ends {
                for [address, mac] in neighbours {
                    self.in_node(
                        *pid,
                        &self.tools.ip,
                        &[
                            "neigh",
                            "replace",
                            address,
                            "lladdr",
                            mac,
                            "dev",
                            interface,
                            "nud",
                            "permanent",
                        ],
                    )?;
                }
            }
            self.down.remove(&link_index);
            self.restore_routes(link_index)?;
        } else {
            self.down.insert(link_index);
        }
        Ok(())
    }

    /// The process that holds the namespace of the node that sends in a direction, and its interface.
    fn interface_of(&self, link: u32, way: Way) -> Result<(u32, String), String> {
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        let end = match way {
            Way::Forward => &link.ends[0],
            Way::Reverse => &link.ends[1],
        };
        Ok((self.nodes[end.node].pid, end.interface.clone()))
    }

    pub(crate) fn apply(
        &mut self,
        link: u32,
        way: Way,
        impairments: &Impairments,
        seed: Option<u64>,
    ) -> Result<(), String> {
        self.failpoints.hit("apply:start");
        self.prepare_tables(impairments)?;
        let link_index = link;
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        // The traffic of a direction leaves through the interface of its sender
        let end = match way {
            Way::Forward => &link.ends[0],
            Way::Reverse => &link.ends[1],
        };
        // A direction with a `queue` is built as the tree of a queue, with the delay in the fabric (D-079, D-082)
        if impairments.queue.is_some() {
            let ports = link.fabric.clone().ok_or_else(|| {
                "a direction with a queue needs a link through the fabric".to_owned()
            })?;
            let (interface, pid) = (end.interface.clone(), self.nodes[end.node].pid);
            // The packets leave the fabric by the port towards the receiver
            let port = match way {
                Way::Forward => &ports[1],
                Way::Reverse => &ports[0],
            };
            let tree = crate::fabric::queue_tree(&interface, port, impairments, seed)?;
            for line in &tree.node {
                let words: Vec<&str> = line.split(' ').collect();
                self.in_node(pid, &self.tools.tc, &words)?;
            }
            if let Some(line) = &tree.fabric {
                let fabric = self.fabric_pid()?;
                let words: Vec<&str> = line.split(' ').collect();
                self.in_node(fabric, &self.tools.tc, &words)?;
            }
            self.built.insert((link_index, way));
            self.queued.insert((link_index, way));
            self.failpoints.hit("apply:end");
            return Ok(());
        }
        // A direction that is planned for changes is built for all its states, also if this one impairs nothing
        let planned = self.layouts.contains_key(&(link_index, way));
        let stages = if let Some(layout) = self.layouts.get(&(link_index, way)) {
            netem::create(layout, impairments, seed)?
        } else if let Some(stages) = netem::arguments(impairments, seed)? {
            stages
        } else {
            return Ok(());
        };
        let pid = self.nodes[end.node].pid;
        // `apply` builds the direction once, at time zero, the root first and then its children
        for line in stages.lines(end.interface.as_str(), "replace") {
            let words: Vec<&str> = line.split(' ').collect();
            self.in_node(pid, &self.tools.tc, &words)?;
        }
        self.built.insert((link_index, way));
        // A direction that changes in the run gets the `tc` that makes its changes before the workloads start, so
        // that its first change does not pay for the start of a process
        if planned {
            self.start_batch(pid);
        }
        self.failpoints.hit("apply:end");
        Ok(())
    }

    /// How many packets entered the queue that holds the loss of a direction: the root queue of the interface
    /// of its sender (D-036). The packets that were sent on, the ones the queue dropped and the ones it still
    /// holds are all packets that drew a number for the loss, so together they are the packets that entered.
    pub(crate) fn queue_packets(&self, link: u32, way: Way) -> Result<u64, String> {
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        let end = match way {
            Way::Forward => &link.ends[0],
            Way::Reverse => &link.ends[1],
        };
        let pid = self.nodes[end.node].pid;
        let shown = self.in_node(
            pid,
            &self.tools.tc,
            &["-j", "-s", "qdisc", "show", "dev", end.interface.as_str()],
        )?;
        packets_entered(&shown)
    }

    /// How many packets are in the queues of a direction now: the `qlen` of the root queue of the interface of its
    /// sender, which counts the packets that wait for the rate, are held back for the delay, and are in any queue
    /// behind the root (measured, D-067). A direction without queues has none.
    pub(crate) fn queue_backlog(&self, link: u32, way: Way) -> Result<u64, String> {
        if !self.built.contains(&(link, way)) {
            return Ok(0);
        }
        let (pid, interface) = self.interface_of(link, way)?;
        let shown = self.in_node(
            pid,
            &self.tools.tc,
            &["-j", "-s", "qdisc", "show", "dev", interface.as_str()],
        )?;
        let mut backlog = backlog_of(&shown)?;
        // A direction with a queue holds packets for the delay in the fabric as well
        if self.queued.contains(&(link, way))
            && let (Some(fabric), Some(ports)) = (
                self.fabric,
                self.links
                    .get(link as usize)
                    .and_then(|link| link.fabric.as_ref()),
            )
        {
            let port = match way {
                Way::Forward => &ports[1],
                Way::Reverse => &ports[0],
            };
            let shown = self.in_node(
                fabric,
                &self.tools.tc,
                &["-j", "-s", "qdisc", "show", "dev", port.as_str()],
            )?;
            backlog += backlog_of(&shown)?;
        }
        Ok(backlog)
    }

    /// What the kernel counted in the leaf of the queue of a direction (PRB-10, D-082): `None` for a direction without a queue.
    pub(crate) fn queue_stats(
        &self,
        link: u32,
        way: Way,
    ) -> Result<Option<crate::protocol::WireQueueStats>, String> {
        if !self.queued.contains(&(link, way)) {
            return Ok(None);
        }
        let (pid, interface) = self.interface_of(link, way)?;
        let shown = self.in_node(
            pid,
            &self.tools.tc,
            &["-j", "-s", "qdisc", "show", "dev", interface.as_str()],
        )?;
        leaf_stats(&shown).map(Some)
    }

    /// What the node at the receiving end of a direction has counted (D-040): the frames that arrived at the
    /// interface of the link, and the UDP datagrams that the node dropped for a full receive buffer. Both are
    /// read from `/proc` of the process that holds the namespace of the node, which shows that namespace.
    pub(crate) fn node_counts(&self, link: u32, way: Way) -> Result<(u64, u64), String> {
        let link = self
            .links
            .get(link as usize)
            .ok_or_else(|| format!("the link {link} does not exist in this run"))?;
        let end = match way {
            Way::Forward => &link.ends[1],
            Way::Reverse => &link.ends[0],
        };
        let pid = self.nodes[end.node].pid;
        let read = |file: &str| {
            fs::read_to_string(format!("/proc/{pid}/net/{file}"))
                .map_err(|error| format!("cannot read /proc/{pid}/net/{file}: {error}"))
        };
        Ok((
            frames_received(&read("dev")?, &end.interface)?,
            udp_receive_buffer_errors(&read("snmp")?)?,
        ))
    }

    pub(crate) fn start_program(&mut self, node: u32, spec: ProgramSpec) -> Result<u32, String> {
        let ProgramSpec {
            argv,
            memory_limit,
            env,
            cwd,
            own_group,
            stdout_file,
            stderr_file,
        } = spec;
        self.failpoints.hit("program:start");
        let node = self
            .nodes
            .get(node as usize)
            .ok_or_else(|| format!("the node {node} does not exist in this run"))?;
        let Some((program, arguments)) = argv.split_first() else {
            return Err("a program needs at least its name".to_owned());
        };
        // The files are made before the program, so that a path that cannot be written is an error of the start
        let create = |path: Option<String>| -> Result<Option<fs::File>, String> {
            path.map(|path| {
                fs::File::create(&path).map_err(|error| format!("cannot create {path}: {error}"))
            })
            .transpose()
        };
        let (stdout_log, stderr_log) = (create(stdout_file)?, create(stderr_file)?);
        let mut command = Command::new(&self.tools.nsenter);
        command
            .arg(format!("--net=/proc/{}/ns/net", node.pid))
            .arg("--");
        command.envs(env);
        if let Some(directory) = &cwd {
            command.current_dir(directory);
        }
        if own_group {
            // The program leads a process group of its own, so that a signal reaches its children too (D-041)
            command.process_group(0);
        }
        if let Some(bytes) = memory_limit {
            // `prlimit` sets the limit and then becomes the program: nothing stays between it and us
            command
                .arg(&self.tools.prlimit)
                .arg(format!("--as={bytes}"))
                .arg("--");
        }
        let mut child = command
            .arg(program)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "cannot start '{program}' in the node '{}': {error}",
                    node.name
                )
            })?;
        let output = Arc::new(Output::default());
        if let Some(pipe) = child.stdout.take() {
            let output = Arc::clone(&output);
            std::thread::spawn(move || read_stdout(pipe, &output, stdout_log));
        } else {
            output.lock().stdout_ended = true;
        }
        if let Some(pipe) = child.stderr.take() {
            let output = Arc::clone(&output);
            std::thread::spawn(move || read_stderr(pipe, &output, stderr_log));
        } else {
            output.lock().stderr_ended = true;
        }
        let stdin = child.stdin.take();
        self.programs.push(RunningProgram {
            child,
            stdin,
            output,
            own_group,
        });
        self.failpoints.hit("program:end");
        id(self.programs.len() - 1)
    }

    /// Waits up to `wait` for a line or the end of a program and returns what is new since the last call.
    pub(crate) fn poll(&mut self, program: u32, wait: Duration) -> Result<ProgramReport, String> {
        let running = self
            .programs
            .get_mut(program as usize)
            .ok_or_else(|| format!("the program {program} does not exist in this run"))?;
        let deadline = Instant::now() + wait;
        loop {
            let exit = running
                .child
                .try_wait()
                .map_err(|error| format!("cannot ask the program {program}: {error}"))?;
            let mut state = running.output.lock();
            if exit.is_some() {
                // Everything the program wrote is read before it is reported as ended
                let grace = Instant::now() + OUTPUT_GRACE;
                while !(state.stdout_ended && state.stderr_ended) && Instant::now() < grace {
                    state = running
                        .output
                        .news
                        .wait_timeout(state, Duration::from_millis(10))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
            let now = Instant::now();
            if !state.lines.is_empty() || exit.is_some() || now >= deadline {
                return Ok(ProgramReport {
                    lines: std::mem::take(&mut state.lines),
                    exit: exit.map(|status| match (status.code(), status.signal()) {
                        (Some(code), _) => crate::protocol::Exit::Code(code),
                        (None, Some(signal)) => crate::protocol::Exit::Signal(signal),
                        (None, None) => crate::protocol::Exit::Code(-1),
                    }),
                    stderr: state.stderr.clone(),
                    truncated: state.truncated,
                    output: OutputCounts {
                        stdout_bytes: state.stats.stdout_bytes,
                        stdout_dropped_bytes: state.stats.stdout_dropped_bytes,
                        stderr_bytes: state.stats.stderr_bytes,
                        stderr_dropped_bytes: state.stats.stderr_dropped_bytes,
                        long_lines: state.stats.long_lines,
                    },
                });
            }
            // A line wakes this up at once; the end of the program is looked for every few milliseconds
            let slice = (deadline - now).min(Duration::from_millis(10));
            drop(
                running
                    .output
                    .news
                    .wait_timeout(state, slice)
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
    }

    /// Sends a signal to the process group of a program. A group that is gone is not an error.
    pub(crate) fn signal(&mut self, program: u32, number: i32) -> Result<(), String> {
        let running = self
            .programs
            .get(program as usize)
            .ok_or_else(|| format!("the program {program} does not exist in this run"))?;
        if !running.own_group {
            return Err(format!(
                "the program {program} was not started in a process group of its own"
            ));
        }
        let signal = match number {
            1 => rustix::process::Signal::HUP,
            2 => rustix::process::Signal::INT,
            3 => rustix::process::Signal::QUIT,
            9 => rustix::process::Signal::KILL,
            10 => rustix::process::Signal::USR1,
            12 => rustix::process::Signal::USR2,
            15 => rustix::process::Signal::TERM,
            other => return Err(format!("the signal {other} is not one that a run sends")),
        };
        let group = i32::try_from(running.child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| format!("the program {program} has no process id"))?;
        match rustix::process::kill_process_group(group, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(error) => Err(format!(
                "cannot signal the group of the program {program}: {error}"
            )),
        }
    }

    /// Whether a socket is bound (UDP) or listens (TCP) on `port` in a node, read from the tables of its namespace.
    pub(crate) fn port_bound(&self, node: u32, udp: bool, port: u16) -> Result<bool, String> {
        let node = self
            .nodes
            .get(node as usize)
            .ok_or_else(|| format!("the node {node} does not exist in this run"))?;
        let (files, state) = if udp {
            (["udp", "udp6"], "07")
        } else {
            (["tcp", "tcp6"], "0A")
        };
        let mut read_one = false;
        for file in files {
            // IPv6 may be switched off in the node: its table is then missing, which says nothing about IPv4
            if let Ok(table) = fs::read_to_string(format!("/proc/{}/net/{file}", node.pid)) {
                read_one = true;
                if socket_is_bound(&table, port, state) {
                    return Ok(true);
                }
            }
        }
        if read_one {
            Ok(false)
        } else {
            Err(format!(
                "cannot read the sockets of the node '{}' (/proc/{}/net)",
                node.name, node.pid
            ))
        }
    }

    /// Writes a line to the standard input of a program. A program that has ended is not an error.
    pub(crate) fn send_line(&mut self, program: u32, line: &str) -> Result<(), String> {
        let running = self
            .programs
            .get_mut(program as usize)
            .ok_or_else(|| format!("the program {program} does not exist in this run"))?;
        if let Some(stdin) = &mut running.stdin {
            // A broken pipe means that the program is gone, which is what the caller wanted anyway
            let _ = writeln!(stdin, "{line}").and_then(|()| stdin.flush());
        }
        Ok(())
    }

    /// Ends the programs and the holders of the nodes. It can be called at any point and twice. What is left
    /// (the interfaces, the namespaces) goes with the holders, and the kernel removes the rest when the
    /// supervisor ends.
    pub(crate) fn teardown(&mut self) -> Result<(), String> {
        self.failpoints.hit("teardown:start");
        let mut left = Vec::new();
        for (_, batch) in self.batches.drain() {
            if let Some(mut batch) = batch {
                batch.end();
            }
        }
        for mut program in self.programs.drain(..) {
            // A program that ended already is not an error
            let _ = program.child.kill();
            let _ = program.child.wait();
        }
        for mut node in self.nodes.drain(..) {
            if node.holder.kill().is_err() && node.holder.try_wait().ok().flatten().is_none() {
                left.push(node.name.clone());
            }
            let _ = node.holder.wait();
        }
        self.links.clear();
        if let Some(tables) = self.tables.take() {
            tables.remove();
        }
        self.failpoints.hit("teardown:end");
        if left.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "the holders of the nodes {} could not be ended",
                left.join(", ")
            ))
        }
    }

    /// Runs `program` with `arguments` in the network namespace held by process `pid`.
    fn in_node(
        &self,
        pid: u32,
        program: &std::path::Path,
        arguments: &[&str],
    ) -> Result<String, String> {
        let namespace = format!("--net=/proc/{pid}/ns/net");
        let program = program.to_string_lossy();
        let mut command = vec![namespace.as_str(), "--", program.as_ref()];
        command.extend_from_slice(arguments);
        match &self.tables {
            Some(tables) => run_with_env(
                &self.tools.nsenter,
                &command,
                &[("TC_LIB_DIR", &tables.path().to_string_lossy())],
            ),
            None => run(&self.tools.nsenter, &command),
        }
    }

    /// Writes the tables that `impairments` names (D-084) before a `tc` needs them. The first table makes the directory; a batch
    /// of `tc` that was started before has not got it in its environment, so it is started again.
    fn prepare_tables(&mut self, impairments: &Impairments) -> Result<(), String> {
        let Some(table) = impairments
            .jitter
            .as_ref()
            .and_then(|jitter| jitter.table.as_ref())
        else {
            return Ok(());
        };
        if self.tables.is_none() {
            let system = std::env::var("TC_LIB_DIR").ok();
            self.tables = Some(TableDirectory::create(&self.run_id, system.as_deref())?);
            let started: Vec<u32> = self.batches.keys().copied().collect();
            for pid in started {
                if let Some(Some(mut batch)) = self.batches.remove(&pid) {
                    batch.end();
                }
                self.start_batch(pid);
            }
        }
        if let Some(tables) = &self.tables {
            tables.add(table)?;
        }
        Ok(())
    }
}

/// The counters of the leaf of a queue (the qdisc with the handle `3:`), from the JSON of `tc -j -s qdisc show dev <interface>`.
/// What every qdisc has is `bytes`, `packets`, `drops`, `overlimits`, `requeues`, `backlog` (bytes) and `qlen` (packets); the
/// rest that is a whole number or a boolean is special to the discipline (`ecn_mark`, `drop_overlimit`, `maxpacket`, ...).
fn leaf_stats(json: &str) -> Result<crate::protocol::WireQueueStats, String> {
    let qdiscs: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("cannot read the statistics of the queue: {error}"))?;
    let leaf = qdiscs
        .as_array()
        .and_then(|qdiscs| {
            qdiscs.iter().find(|qdisc| {
                qdisc.get("handle").and_then(serde_json::Value::as_str)
                    == Some(crate::fabric::LEAF_HANDLE)
            })
        })
        .ok_or_else(|| "the interface has no queue with the handle 3:".to_owned())?;
    let number = |key: &str| {
        leaf.get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let common = [
        "bytes",
        "packets",
        "drops",
        "overlimits",
        "requeues",
        "backlog",
        "qlen",
    ];
    let mut details = std::collections::BTreeMap::new();
    if let Some(object) = leaf.as_object() {
        for (key, value) in object {
            if common.contains(&key.as_str()) {
                continue;
            }
            if let Some(number) = value.as_u64() {
                details.insert(key.clone(), number);
            } else if let Some(flag) = value.as_bool() {
                details.insert(key.clone(), u64::from(flag));
            }
        }
    }
    Ok(crate::protocol::WireQueueStats {
        kind: leaf
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        packets: number("packets"),
        bytes: number("bytes"),
        drops: number("drops"),
        overlimits: number("overlimits"),
        requeues: number("requeues"),
        backlog_packets: number("qlen"),
        backlog_bytes: number("backlog"),
        details,
    })
}

/// The packets that the root queue holds, from the JSON of `tc -j -s qdisc show dev <interface>`: its `qlen`, which
/// includes the packets in the queues behind it.
fn backlog_of(json: &str) -> Result<u64, String> {
    let qdiscs: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("cannot read the statistics of the queue: {error}"))?;
    qdiscs
        .as_array()
        .and_then(|qdiscs| {
            qdiscs
                .iter()
                .find(|qdisc| qdisc.get("root").and_then(serde_json::Value::as_bool) == Some(true))
        })
        .ok_or_else(|| "the interface has no root queue".to_owned())?
        .get("qlen")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "the statistics of the queue have no 'qlen'".to_owned())
}

/// The packets that entered the root queue, from the JSON of `tc -j -s qdisc show dev <interface>`: what it
/// sent on (`packets`), dropped (`drops`) and still holds (`qlen`). A child queue (the delay and jitter stage
/// of a direction that also has a rate) is behind the root and does not draw the loss, so it is not counted.
fn packets_entered(json: &str) -> Result<u64, String> {
    let qdiscs: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("cannot read the statistics of the queue: {error}"))?;
    let root = qdiscs
        .as_array()
        .and_then(|qdiscs| {
            qdiscs
                .iter()
                .find(|qdisc| qdisc.get("root").and_then(serde_json::Value::as_bool) == Some(true))
        })
        .ok_or_else(|| "the interface has no root queue".to_owned())?;
    if root.get("kind").and_then(serde_json::Value::as_str) != Some("netem") {
        return Err("the root queue of the interface is not a netem queue".to_owned());
    }
    let count = |name: &str| {
        root.get(name)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("the statistics of the queue have no '{name}'"))
    };
    Ok(count("packets")? + count("drops")? + count("qlen")?)
}

/// The frames that arrived at `interface`, from the text of `/proc/net/dev`: the second number after the name.
fn frames_received(text: &str, interface: &str) -> Result<u64, String> {
    text.lines()
        .find_map(|line| {
            let (name, counters) = line.split_once(':')?;
            (name.trim() == interface).then_some(counters)
        })
        .ok_or_else(|| format!("/proc/net/dev has no interface '{interface}'"))?
        .split_whitespace()
        .nth(1)
        .and_then(|packets| packets.parse().ok())
        .ok_or_else(|| format!("cannot read the frames of '{interface}' in /proc/net/dev"))
}

/// The UDP datagrams dropped for a full receive buffer, from the text of `/proc/net/snmp`: the column
/// `RcvbufErrors` of the line `Udp:`, whose names come on the line before the numbers.
fn udp_receive_buffer_errors(text: &str) -> Result<u64, String> {
    let mut lines = text.lines().filter(|line| line.starts_with("Udp:"));
    let (Some(names), Some(values)) = (lines.next(), lines.next()) else {
        return Err("/proc/net/snmp has no counters for UDP".to_owned());
    };
    names
        .split_whitespace()
        .position(|name| name == "RcvbufErrors")
        .and_then(|at| values.split_whitespace().nth(at))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "/proc/net/snmp has no RcvbufErrors for UDP".to_owned())
}

/// Switches IPv6 off for the interfaces that are created from now on in the network namespace of the caller.
pub(crate) fn disable_ipv6() -> Result<(), String> {
    for file in [
        "/proc/sys/net/ipv6/conf/all/disable_ipv6",
        "/proc/sys/net/ipv6/conf/default/disable_ipv6",
    ] {
        match fs::write(file, "1") {
            Ok(()) => {}
            // The kernel has no IPv6, so there is nothing to switch off
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot switch off IPv6 ({file}): {error}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `tc -j -s qdisc show` printed for a root queue with a child behind it (a direction with a loss, a
    /// rate and a jitter), after 3 packets had entered the root: 1 sent on, 2 dropped.
    const ROOT_AND_CHILD: &str = r#"[{"kind":"netem","handle":"1:","root":true,"refcnt":17,"options":{"limit":1000},"bytes":90,"packets":1,"drops":2,"overlimits":0,"requeues":0,"backlog":0,"qlen":0},{"kind":"netem","handle":"2:","parent":"1:1","options":{"limit":1000},"bytes":90,"packets":1,"drops":0,"overlimits":0,"requeues":0,"backlog":0,"qlen":0}]"#;

    /// The counters of the leaf of a queue are read by its handle `3:`, the common ones are fields of their own, and the rest that
    /// is a number or a flag is kept by the name the kernel gives it (PRB-10).
    #[test]
    fn the_counters_of_the_leaf_are_read_by_its_handle() {
        let shown = r#"[
          {"kind":"netem","handle":"1:","root":true,"options":{"limit":1000},"bytes":99,"packets":9,"drops":0,"overlimits":0,"requeues":0,"backlog":0,"qlen":0},
          {"kind":"htb","handle":"2:","parent":"1:1","options":{"r2q":10},"bytes":98,"packets":8,"drops":0,"overlimits":0,"requeues":0,"backlog":0,"qlen":0},
          {"kind":"codel","handle":"3:","parent":"2:1","options":{"limit":200,"target":5000,"interval":100000,"ecn":false},"bytes":12000,"packets":10,"drops":3,"overlimits":1,"requeues":2,"backlog":2400,"qlen":2,"maxpacket":1200,"drop_overlimit":1,"ecn_mark":0,"dropping":true,"count":4}
        ]"#;
        let found = leaf_stats(shown).unwrap();
        assert_eq!(found.kind, "codel");
        assert_eq!(
            (
                found.packets,
                found.bytes,
                found.drops,
                found.overlimits,
                found.requeues,
                found.backlog_packets,
                found.backlog_bytes
            ),
            (10, 12000, 3, 1, 2, 2, 2400)
        );
        let details: Vec<(&str, u64)> = found
            .details
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect();
        assert_eq!(
            details,
            [
                ("count", 4),
                ("drop_overlimit", 1),
                ("dropping", 1),
                ("ecn_mark", 0),
                ("maxpacket", 1200)
            ]
        );
    }

    /// Canary: an interface without the leaf (a direction that has no queue, or the tree is not built) is an error, not zeros.
    #[test]
    fn canary_an_interface_without_the_leaf_has_no_counters() {
        let shown = r#"[{"kind":"netem","handle":"1:","root":true,"bytes":0,"packets":0}]"#;
        assert!(leaf_stats(shown).unwrap_err().contains("3:"));
        assert!(leaf_stats("not json").is_err());
    }

    #[test]
    fn the_packets_that_entered_are_those_sent_on_dropped_and_held() {
        assert_eq!(packets_entered(ROOT_AND_CHILD), Ok(3));
        // A packet that still waits in the queue entered it too
        let waiting = ROOT_AND_CHILD.replace(r#""qlen":0},{"kind""#, r#""qlen":4},{"kind""#);
        assert_eq!(packets_entered(&waiting), Ok(7));
    }

    #[test]
    fn canary_the_child_is_not_counted_and_what_is_not_a_netem_root_is_refused() {
        // The child has 1 packet and no drops: counting it as well would give 4
        assert_ne!(packets_entered(ROOT_AND_CHILD), Ok(4));
        let only_child =
            r#"[{"kind":"netem","handle":"2:","parent":"1:1","packets":1,"drops":0,"qlen":0}]"#;
        assert!(
            packets_entered(only_child)
                .unwrap_err()
                .contains("no root queue")
        );
        let other =
            r#"[{"kind":"fq_codel","handle":"1:","root":true,"packets":1,"drops":0,"qlen":0}]"#;
        assert!(packets_entered(other).unwrap_err().contains("not a netem"));
        let without = r#"[{"kind":"netem","handle":"1:","root":true,"packets":1,"qlen":0}]"#;
        assert!(packets_entered(without).unwrap_err().contains("'drops'"));
        assert!(
            packets_entered("not json")
                .unwrap_err()
                .contains("cannot read")
        );
        assert!(packets_entered("[]").unwrap_err().contains("no root queue"));
    }

    /// What `/proc/net/dev` printed in a node with a loopback and one end of a link: 12 frames arrived at `np1`.
    const NET_DEV: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0
  np1:    1176      12    0    0    0     0          0         0      588       6    0    0    0     0       0          0
";

    #[test]
    fn the_frames_of_an_interface_are_the_second_number_after_its_name() {
        assert_eq!(frames_received(NET_DEV, "np1"), Ok(12));
        assert_eq!(frames_received(NET_DEV, "lo"), Ok(0));
    }

    /// Canary: another interface is not mistaken for the asked one (a name that is a prefix of another), and an
    /// interface that is not there is an error and not a zero.
    #[test]
    fn canary_a_missing_interface_is_an_error_and_a_prefix_is_not_a_match() {
        assert!(
            frames_received(NET_DEV, "np")
                .unwrap_err()
                .contains("no interface 'np'")
        );
        assert!(frames_received(NET_DEV, "np0").is_err());
        assert!(frames_received("", "np1").is_err());
        assert!(
            frames_received("  np1: 1 x", "np1")
                .unwrap_err()
                .contains("cannot read")
        );
    }

    /// What `/proc/net/snmp` printed: 3 datagrams dropped for a full buffer (the column is the fifth).
    const SNMP: &str = "Ip: Forwarding DefaultTTL InReceives
Ip: 1 64 40
Udp: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors InCsumErrors IgnoredMulti MemErrors
Udp: 100 1 4 90 3 0 1 0 0
UdpLite: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors InCsumErrors IgnoredMulti MemErrors
UdpLite: 0 0 0 0 9 0 0 0 0
";

    #[test]
    fn the_datagrams_dropped_for_a_full_buffer_are_read_by_the_name_of_their_column() {
        assert_eq!(udp_receive_buffer_errors(SNMP), Ok(3));
        // Not the line of UdpLite, and not the column next to it
        assert_ne!(udp_receive_buffer_errors(SNMP), Ok(9));
        assert_ne!(udp_receive_buffer_errors(SNMP), Ok(0));
        assert!(
            udp_receive_buffer_errors("Ip: A\nIp: 1\n")
                .unwrap_err()
                .contains("no counters for UDP")
        );
        assert!(
            udp_receive_buffer_errors("Udp: InDatagrams\nUdp: 5\n")
                .unwrap_err()
                .contains("no RcvbufErrors")
        );
    }

    #[test]
    fn a_failpoint_fires_only_at_its_point_and_count() {
        // `hit` would abort the test process, so only the bookkeeping is checked
        let mut failpoints = Failpoints::new(Some("link:end#2"));
        assert_eq!(failpoints.wanted, Some(("link:end".to_owned(), 2)));
        failpoints.hit("link:end");
        failpoints.hit("node:end");
        assert_eq!(failpoints.seen["link:end"], 1);
        assert_eq!(Failpoints::new(Some("a")).wanted, Some(("a".to_owned(), 1)));
        assert_eq!(Failpoints::new(None).wanted, None);
    }
}
