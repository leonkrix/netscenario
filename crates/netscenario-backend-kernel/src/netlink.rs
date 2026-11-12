//! Netlink instead of the tools `ip` and `tc` (BKD-05, D-101): links, addresses, neighbours and routes in the network namespace
//! of a node, and the kernel settings of that namespace.
//!
//! The messages are built by `netlink-packet-route`, which has no `unsafe`; the socket is `netlink-sys`. A node is a network
//! namespace held by a process, so the socket of a node is made by a thread that has entered that namespace with `setns`
//! (the call changes the namespace of the calling thread only). A socket stays in the namespace it was made in, so the thread
//! can end at once and the socket is used from any thread afterwards.

// The structures of `netlink-packet-route` are `non_exhaustive`: outside that crate they can only be made by `default()` and then
// filled in, so the lint that asks for a struct expression cannot be followed
#![allow(clippy::field_reassign_with_default)]

use std::fs;
use std::net::IpAddr;

use netlink_packet_core::{
    NLM_F_ACK, NLM_F_CREATE, NLM_F_DUMP, NLM_F_EXCL, NLM_F_REPLACE, NLM_F_REQUEST, NetlinkHeader,
    NetlinkMessage, NetlinkPayload,
};
use netlink_packet_route::address::{AddressAttribute, AddressFlags, AddressMessage};
use netlink_packet_route::link::{
    InfoData, InfoKind, InfoVeth, LinkAttribute, LinkFlags, LinkInfo, LinkMessage,
};
use netlink_packet_route::neighbour::{
    NeighbourAddress, NeighbourAttribute, NeighbourMessage, NeighbourState,
};
use netlink_packet_route::route::{
    RouteAddress, RouteAttribute, RouteHeader, RouteMessage, RouteNextHop, RouteProtocol,
    RouteScope, RouteType,
};
use netlink_packet_route::{AddressFamily, RouteNetlinkMessage};
use netlink_sys::protocols::NETLINK_ROUTE;
use netlink_sys::{Socket, SocketAddr};
use rustix::thread::{LinkNameSpaceType, move_into_link_name_space};

/// The table `main`, where `ip route` puts a route that does not say otherwise.
const TABLE_MAIN: u8 = 254;

/// The size of the buffer for one reply. A message about one link is about 2 KB.
const REPLY_BUFFER: usize = 32 * 1024;

/// A way out of a route: the address of the next hop, the interface it is reached by and the weight among several ways.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hop {
    /// The address of the other end of the link.
    pub(crate) gateway: IpAddr,
    /// The name of the interface the route leaves by.
    pub(crate) interface: String,
    /// The share of this way in a route with several (`ip route ... nexthop ... weight`).
    pub(crate) weight: u32,
}

/// A route of the main table (D-078): to one address or every address, with a metric, a source address and one or several ways.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteSpec {
    /// The address the route leads to, or `None` for the default route.
    pub(crate) destination: Option<IpAddr>,
    /// The metric (`ip route ... metric`).
    pub(crate) metric: u32,
    /// The source address of the packets that the node sends over the route.
    pub(crate) source: Option<IpAddr>,
    /// The ways out. One is a plain route, several a multipath route.
    pub(crate) hops: Vec<Hop>,
    /// The family of the route when it has no destination.
    pub(crate) v6: bool,
}

/// How a message treats an object that is there already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    /// Make a new one; it is an error if it exists (`ip ... add`).
    Add,
    /// Make a new one or replace the one that is there (`ip ... replace`).
    Replace,
}

impl Verb {
    fn flags(self) -> u16 {
        match self {
            Self::Add => NLM_F_CREATE | NLM_F_EXCL,
            Self::Replace => NLM_F_CREATE | NLM_F_REPLACE,
        }
    }
}

/// A netlink socket in one network namespace.
pub(crate) struct Netlink {
    socket: Socket,
    sequence: u32,
}

impl std::fmt::Debug for Netlink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Netlink")
            .field("sequence", &self.sequence)
            .finish_non_exhaustive()
    }
}

impl Netlink {
    /// A socket in the network namespace of the calling thread.
    pub(crate) fn here() -> Result<Self, String> {
        let mut socket = Socket::new(NETLINK_ROUTE)
            .map_err(|error| format!("cannot open a netlink socket: {error}"))?;
        socket
            .bind_auto()
            .map_err(|error| format!("cannot bind the netlink socket: {error}"))?;
        socket
            .connect(&SocketAddr::new(0, 0))
            .map_err(|error| format!("cannot connect the netlink socket to the kernel: {error}"))?;
        Ok(Self {
            socket,
            sequence: 1,
        })
    }

    /// A socket in the network namespace that process `pid` holds. A thread enters the namespace, makes the socket and ends;
    /// the process itself stays where it is.
    pub(crate) fn in_namespace(pid: u32) -> Result<Self, String> {
        let path = format!("/proc/{pid}/ns/net");
        let namespace = fs::File::open(&path)
            .map_err(|error| format!("cannot open the network namespace {path}: {error}"))?;
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    move_into_link_name_space(
                        std::os::fd::AsFd::as_fd(&namespace),
                        Some(LinkNameSpaceType::Network),
                    )
                    .map_err(|error| {
                        format!("cannot enter the network namespace {path}: {error}")
                    })?;
                    Self::here()
                })
                .join()
                .unwrap_or_else(|_| Err("the thread that enters a namespace panicked".to_owned()))
        })
    }

    /// Sends one request and reads the answer: the acknowledgement of the kernel, or its error.
    fn request(&mut self, payload: RouteNetlinkMessage, flags: u16) -> Result<(), String> {
        let sequence = self.send(payload, NLM_F_ACK | flags)?;
        match self.receive(sequence)?.payload {
            NetlinkPayload::Error(error) => match error.code {
                None => Ok(()),
                Some(code) => Err(os_error(-code.get())),
            },
            other => Err(format!("the kernel answered with {other:?}")),
        }
    }

    fn send(&mut self, payload: RouteNetlinkMessage, flags: u16) -> Result<u32, String> {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        let mut header = NetlinkHeader::default();
        header.flags = NLM_F_REQUEST | flags;
        header.sequence_number = sequence;
        let mut message = NetlinkMessage::new(header, NetlinkPayload::from(payload));
        message.finalize();
        let mut buffer = vec![0_u8; message.buffer_len()];
        message.serialize(&mut buffer);
        self.socket
            .send(&buffer, 0)
            .map_err(|error| format!("cannot send to the kernel: {error}"))?;
        Ok(sequence)
    }

    fn receive(&mut self, sequence: u32) -> Result<NetlinkMessage<RouteNetlinkMessage>, String> {
        let mut buffer = vec![0_u8; REPLY_BUFFER];
        let mut view: &mut [u8] = &mut buffer;
        let size = self
            .socket
            .recv(&mut view, 0)
            .map_err(|error| format!("cannot read from the kernel: {error}"))?;
        let message = NetlinkMessage::<RouteNetlinkMessage>::deserialize(&buffer[..size])
            .map_err(|error| format!("cannot read the answer of the kernel: {error}"))?;
        if message.header.sequence_number != sequence {
            return Err(format!(
                "the kernel answered request {} to request {sequence}",
                message.header.sequence_number
            ));
        }
        Ok(message)
    }

    /// The index of the interface with this name, or why there is none.
    pub(crate) fn link_index(&mut self, name: &str) -> Result<u32, String> {
        let mut link = LinkMessage::default();
        link.attributes.push(LinkAttribute::IfName(name.to_owned()));
        let sequence = self.send(RouteNetlinkMessage::GetLink(link), 0)?;
        match self.receive(sequence)?.payload {
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(link)) => {
                Ok(link.header.index)
            }
            NetlinkPayload::Error(error) => Err(match error.code {
                Some(code) => format!("the interface {name}: {}", os_error(-code.get())),
                None => format!("the kernel did not describe the interface {name}"),
            }),
            other => Err(format!("the kernel answered with {other:?}")),
        }
    }

    /// The names of the interfaces of the namespace, in the order of their indexes.
    pub(crate) fn link_names(&mut self) -> Result<Vec<String>, String> {
        let sequence = self.send(
            RouteNetlinkMessage::GetLink(LinkMessage::default()),
            NLM_F_DUMP,
        )?;
        let mut names = Vec::new();
        loop {
            let mut buffer = vec![0_u8; REPLY_BUFFER];
            let mut view: &mut [u8] = &mut buffer;
            let size = self
                .socket
                .recv(&mut view, 0)
                .map_err(|error| format!("cannot read from the kernel: {error}"))?;
            let mut offset = 0;
            while offset < size {
                let message =
                    NetlinkMessage::<RouteNetlinkMessage>::deserialize(&buffer[offset..size])
                        .map_err(|error| {
                            format!("cannot read the answer of the kernel: {error}")
                        })?;
                if message.header.sequence_number != sequence {
                    return Err("the kernel answered another request".to_owned());
                }
                let length = message.header.length as usize;
                match message.payload {
                    NetlinkPayload::Done(_) => return Ok(names),
                    NetlinkPayload::Error(error) => {
                        return Err(match error.code {
                            Some(code) => os_error(-code.get()),
                            None => "the kernel ended the list of interfaces".to_owned(),
                        });
                    }
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(link)) => {
                        names.extend(link.attributes.into_iter().find_map(|attribute| {
                            match attribute {
                                LinkAttribute::IfName(name) => Some(name),
                                _ => None,
                            }
                        }));
                    }
                    _ => {}
                }
                if length == 0 {
                    break;
                }
                // Messages are aligned to four bytes
                offset += length.div_ceil(4) * 4;
            }
        }
    }

    /// Makes a virtual Ethernet pair: `name` with the hardware address `mac` and its peer.
    pub(crate) fn add_veth(
        &mut self,
        name: &str,
        mac: Option<&str>,
        peer: &str,
        peer_mac: Option<&str>,
    ) -> Result<(), String> {
        let mut peer_message = LinkMessage::default();
        peer_message
            .attributes
            .push(LinkAttribute::IfName(peer.to_owned()));
        if let Some(peer_mac) = peer_mac {
            peer_message
                .attributes
                .push(LinkAttribute::Address(parse_mac(peer_mac)?));
        }
        let mut link = LinkMessage::default();
        link.attributes.push(LinkAttribute::IfName(name.to_owned()));
        if let Some(mac) = mac {
            link.attributes
                .push(LinkAttribute::Address(parse_mac(mac)?));
        }
        link.attributes.push(LinkAttribute::LinkInfo(vec![
            LinkInfo::Kind(InfoKind::Veth),
            LinkInfo::Data(InfoData::Veth(InfoVeth::Peer(peer_message))),
        ]));
        self.request(RouteNetlinkMessage::NewLink(link), Verb::Add.flags())
            .map_err(|error| format!("cannot make the pair {name} and {peer}: {error}"))
    }

    /// Moves the interface `name` into the network namespace that process `pid` holds, and names it `new_name` there (the same
    /// name when it is `None`).
    pub(crate) fn move_link(
        &mut self,
        name: &str,
        pid: u32,
        new_name: Option<&str>,
    ) -> Result<(), String> {
        let index = self.link_index(name)?;
        let mut link = LinkMessage::default();
        link.header.index = index;
        link.attributes.push(LinkAttribute::NetNsPid(pid));
        if let Some(new_name) = new_name {
            link.attributes
                .push(LinkAttribute::IfName(new_name.to_owned()));
        }
        self.request(RouteNetlinkMessage::SetLink(link), 0)
            .map_err(|error| {
                format!("cannot move {name} into the namespace of process {pid}: {error}")
            })
    }

    /// Brings the interface up or takes it down (`ip link set ... up|down`).
    pub(crate) fn set_state(&mut self, name: &str, up: bool) -> Result<(), String> {
        let index = self.link_index(name)?;
        let mut link = LinkMessage::default();
        link.header.index = index;
        link.header.change_mask = LinkFlags::Up;
        if up {
            link.header.flags = LinkFlags::Up;
        }
        self.request(RouteNetlinkMessage::SetLink(link), 0)
            .map_err(|error| {
                format!(
                    "cannot set {name} {}: {error}",
                    if up { "up" } else { "down" }
                )
            })
    }

    /// Sets the MTU of an interface.
    pub(crate) fn set_mtu(&mut self, name: &str, mtu: u32) -> Result<(), String> {
        let index = self.link_index(name)?;
        let mut link = LinkMessage::default();
        link.header.index = index;
        link.attributes.push(LinkAttribute::Mtu(mtu));
        self.request(RouteNetlinkMessage::SetLink(link), 0)
            .map_err(|error| format!("cannot set the MTU of {name} to {mtu}: {error}"))
    }

    /// Sets the largest segment the stack builds for the interface and how many segments one has (`gso_max_size`,
    /// `gso_max_segs`).
    pub(crate) fn set_segments(
        &mut self,
        name: &str,
        size: u32,
        segments: u32,
    ) -> Result<(), String> {
        let index = self.link_index(name)?;
        let mut link = LinkMessage::default();
        link.header.index = index;
        link.attributes = vec![
            LinkAttribute::GsoMaxSize(size),
            LinkAttribute::GsoMaxSegs(segments),
        ];
        self.request(RouteNetlinkMessage::SetLink(link), 0)
            .map_err(|error| format!("cannot limit the segments of {name}: {error}"))
    }

    /// Gives an interface an address (`ip addr add|replace ADDRESS/PREFIX dev NAME`). An IPv6 address is there at once, without
    /// the duplicate address detection (`nodad`).
    pub(crate) fn set_address(
        &mut self,
        name: &str,
        address: IpAddr,
        prefix: u8,
        verb: Verb,
    ) -> Result<(), String> {
        let index = self.link_index(name)?;
        let mut message = AddressMessage::default();
        message.header.family = family(address);
        message.header.prefix_len = prefix;
        message.header.index = index;
        message.attributes.push(AddressAttribute::Local(address));
        // For a point to point address `Address` is the peer; for the others the kernel wants the same as `Local`
        message.attributes.push(AddressAttribute::Address(address));
        if address.is_ipv6() {
            message
                .attributes
                .push(AddressAttribute::Flags(AddressFlags::Nodad));
        }
        self.request(RouteNetlinkMessage::NewAddress(message), verb.flags())
            .map_err(|error| format!("cannot give {name} the address {address}/{prefix}: {error}"))
    }

    /// Fixes the hardware address of a neighbour on an interface (`ip neigh replace ADDRESS lladdr MAC dev NAME nud permanent`).
    pub(crate) fn set_neighbour(
        &mut self,
        name: &str,
        address: IpAddr,
        mac: &str,
    ) -> Result<(), String> {
        let index = self.link_index(name)?;
        let mut message = NeighbourMessage::default();
        message.header.family = family(address);
        message.header.ifindex = index;
        message.header.state = NeighbourState::Permanent;
        message.attributes = vec![
            NeighbourAttribute::Destination(match address {
                IpAddr::V4(address) => NeighbourAddress::Inet(address),
                IpAddr::V6(address) => NeighbourAddress::Inet6(address),
            }),
            NeighbourAttribute::LinkLayerAddress(parse_mac(mac)?),
        ];
        self.request(
            RouteNetlinkMessage::NewNeighbour(message),
            Verb::Replace.flags(),
        )
        .map_err(|error| format!("cannot fix the neighbour {address} on {name}: {error}"))
    }

    /// Puts a route in the main table.
    pub(crate) fn set_route(&mut self, route: &RouteSpec, verb: Verb) -> Result<(), String> {
        if route.hops.is_empty() {
            return Err("a route needs a way out".to_owned());
        }
        let v6 = route
            .destination
            .map_or(route.v6, |address| address.is_ipv6());
        let mut message = RouteMessage::default();
        let mut header = RouteHeader::default();
        header.address_family = if v6 {
            AddressFamily::Inet6
        } else {
            AddressFamily::Inet
        };
        header.destination_prefix_length = match route.destination {
            None => 0,
            Some(IpAddr::V4(_)) => 32,
            Some(IpAddr::V6(_)) => 128,
        };
        header.table = TABLE_MAIN;
        header.protocol = RouteProtocol::Boot;
        header.scope = RouteScope::Universe;
        header.kind = RouteType::Unicast;
        message.header = header;
        if let Some(destination) = route.destination {
            message
                .attributes
                .push(RouteAttribute::Destination(route_address(destination)));
        }
        message
            .attributes
            .push(RouteAttribute::Priority(route.metric));
        if let Some(source) = route.source {
            message
                .attributes
                .push(RouteAttribute::PrefSource(route_address(source)));
        }
        if let [hop] = route.hops.as_slice() {
            let index = self.link_index(&hop.interface)?;
            message
                .attributes
                .push(RouteAttribute::Gateway(route_address(hop.gateway)));
            message.attributes.push(RouteAttribute::Oif(index));
        } else {
            let mut hops = Vec::with_capacity(route.hops.len());
            for hop in &route.hops {
                let index = self.link_index(&hop.interface)?;
                let mut next = RouteNextHop::default();
                // `ip route ... weight W` is sent as W - 1
                next.hops = u8::try_from(hop.weight.saturating_sub(1)).unwrap_or(u8::MAX);
                next.interface_index = index;
                next.attributes = vec![RouteAttribute::Gateway(route_address(hop.gateway))];
                hops.push(next);
            }
            message.attributes.push(RouteAttribute::MultiPath(hops));
        }
        self.request(RouteNetlinkMessage::NewRoute(message), verb.flags())
            .map_err(|error| {
                let to = route.destination.map_or_else(
                    || "the default route".to_owned(),
                    |address| address.to_string(),
                );
                format!("cannot put the route to {to}: {error}")
            })
    }
}

fn family(address: IpAddr) -> AddressFamily {
    match address {
        IpAddr::V4(_) => AddressFamily::Inet,
        IpAddr::V6(_) => AddressFamily::Inet6,
    }
}

fn route_address(address: IpAddr) -> RouteAddress {
    match address {
        IpAddr::V4(address) => RouteAddress::Inet(address),
        IpAddr::V6(address) => RouteAddress::Inet6(address),
    }
}

/// An address in the text of the tools (`10.100.0.1`, `fd00:100:0::1`).
pub(crate) fn parse_ip(text: &str) -> Result<IpAddr, String> {
    text.parse()
        .map_err(|_| format!("'{text}' is not an address"))
}

/// An address with its prefix length (`10.100.0.1/30`).
pub(crate) fn parse_cidr(text: &str) -> Result<(IpAddr, u8), String> {
    let (address, prefix) = text
        .split_once('/')
        .ok_or_else(|| format!("'{text}' is not an address with a prefix length"))?;
    let prefix = prefix
        .parse()
        .map_err(|_| format!("'{text}' is not an address with a prefix length"))?;
    Ok((parse_ip(address)?, prefix))
}

/// The six bytes of `02:4e:53:00:00:01`.
pub(crate) fn parse_mac(text: &str) -> Result<Vec<u8>, String> {
    let bytes: Result<Vec<u8>, _> = text
        .split(':')
        .map(|part| u8::from_str_radix(part, 16))
        .collect();
    match bytes {
        Ok(bytes) if bytes.len() == 6 => Ok(bytes),
        _ => Err(format!("'{text}' is not a hardware address")),
    }
}

/// What the kernel said, in the words of the system (`File exists`, `Operation not permitted`).
fn os_error(code: i32) -> String {
    std::io::Error::from_raw_os_error(code).to_string()
}

/// Writes kernel settings (`net.ipv4.ip_forward=1`) in the network namespace that process `pid` holds, or in the namespace of the
/// caller when `pid` is `None`. A thread enters the namespace and writes the files of `/proc/sys`, which show the tables of the
/// namespace of the thread that opens them. Only names of the networking tables are accepted (D-078).
pub(crate) fn write_settings(pid: Option<u32>, settings: &[&str]) -> Result<(), String> {
    let mut writes = Vec::with_capacity(settings.len());
    for setting in settings {
        let (name, value) = split_setting(setting)?;
        writes.push((format!("/proc/sys/{}", name.replace('.', "/")), value));
    }
    let write_all = move || -> Result<(), String> {
        for (path, value) in &writes {
            fs::write(path, value)
                .map_err(|error| format!("cannot write {value} to {path}: {error}"))?;
        }
        Ok(())
    };
    let Some(pid) = pid else {
        return write_all();
    };
    let path = format!("/proc/{pid}/ns/net");
    let namespace = fs::File::open(&path)
        .map_err(|error| format!("cannot open the network namespace {path}: {error}"))?;
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                move_into_link_name_space(
                    std::os::fd::AsFd::as_fd(&namespace),
                    Some(LinkNameSpaceType::Network),
                )
                .map_err(|error| format!("cannot enter the network namespace {path}: {error}"))?;
                write_all()
            })
            .join()
            .unwrap_or_else(|_| Err("the thread that enters a namespace panicked".to_owned()))
    })
}

/// The name and the value of `net.ipv4.ip_forward=1`, or why it is refused: it has to be `name=value`, and the name has to be in
/// the tables of the network with parts that are plain words (no empty part, no `..`, nothing that is not a letter, a digit, `_`
/// or `-`), so that the path in `/proc/sys` cannot leave them.
pub(crate) fn split_setting(setting: &str) -> Result<(&str, &str), String> {
    let Some((name, value)) = setting.split_once('=') else {
        return Err(format!("'{setting}' is not name=value"));
    };
    if !name.starts_with("net.")
        || name.split('.').any(|part| {
            part.is_empty()
                || part == ".."
                || !part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
    {
        return Err(format!("'{name}' is not the name of a network setting"));
    }
    Ok((name, value))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fmt::Write as _;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use super::*;

    /// Set in the copy of the test binary that runs inside the namespaces.
    const INSIDE: &str = "NETSCENARIO_NETLINK_TEST_INSIDE";

    /// Runs the test `name` again in a user namespace of its own with a network namespace (`unshare --user --map-root-user
    /// --net`), where the test may make namespaces and interfaces without touching the machine (SAF-01). `false` in the copy
    /// that runs inside, where the body goes on; `true` in the first copy, which has asserted that the second passed. Where the
    /// machine does not allow user namespaces the test has nothing to show and returns at once: the CI is the judge there.
    fn rerun_inside(name: &str) -> bool {
        if std::env::var_os(INSIDE).is_some() {
            return false;
        }
        let probe = Command::new("unshare")
            .args(["--user", "--map-root-user", "--net", "--", "true"])
            .stderr(Stdio::null())
            .status();
        if !probe.is_ok_and(|status| status.success()) {
            return true;
        }
        let output = Command::new("unshare")
            .args(["--user", "--map-root-user", "--net", "--"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env(INSIDE, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name} failed inside the namespaces:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    /// A network namespace held by a process that sleeps; it ends with the value.
    struct Node(Child);

    impl Node {
        fn new() -> Self {
            let before = fs::read_link("/proc/self/ns/net").unwrap();
            let child = Command::new("unshare")
                .args(["--net", "--", "sleep", "300"])
                .spawn()
                .unwrap();
            let path = format!("/proc/{}/ns/net", child.id());
            let start = Instant::now();
            // `unshare` is the process until the namespace is made
            while fs::read_link(&path).ok().as_ref() == Some(&before) {
                assert!(start.elapsed() < Duration::from_secs(10), "no namespace");
                std::thread::sleep(Duration::from_millis(5));
            }
            Self(child)
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }

        /// What `program` prints in the namespace of the node.
        fn show(&self, program: &str, arguments: &[&str]) -> String {
            let namespace = format!("--net=/proc/{}/ns/net", self.pid());
            let output = Command::new("nsenter")
                .arg(namespace)
                .arg("--")
                .arg(program)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{program} {arguments:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }

        /// Runs the tool `ip` in the namespace: the oracle that the layer is compared with.
        fn ip(&self, arguments: &[&str]) {
            self.show("ip", arguments);
        }
    }

    impl Drop for Node {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Everything the layer sets, as `ip` shows it.
    fn state(node: &Node) -> String {
        let mut text = String::new();
        for arguments in [
            &["-d", "-o", "link", "show"][..],
            &["-o", "address", "show"],
            &["-6", "-o", "address", "show"],
            &["-d", "route", "show"],
            &["-d", "-6", "route", "show"],
            &["neigh", "show"],
            &["-6", "neigh", "show"],
        ] {
            writeln!(text, "$ ip {}", arguments.join(" ")).unwrap();
            let mut lines: Vec<String> = node
                .show("ip", arguments)
                .lines()
                .map(|line| {
                    // The times of the addresses and the neighbours differ between two namespaces made at different moments
                    line.split(" valid_lft")
                        .next()
                        .unwrap_or(line)
                        .split(" used ")
                        .next()
                        .unwrap_or(line)
                        .trim_end()
                        .to_owned()
                })
                .collect();
            lines.sort();
            text.push_str(&lines.join("\n"));
            text.push('\n');
        }
        text
    }

    /// A setting is written only into the tables of the network (D-078): a name outside them, with a part that could leave the
    /// directory or with a character that is no part of a name, is refused before a file is opened; a plain name is taken as it
    /// is, which is the canary for the refusals (a check that refuses everything would pass the rest).
    #[test]
    fn a_setting_outside_the_network_tables_is_refused_before_a_file_is_opened() {
        for (setting, why) in [
            (
                "kernel.hostname=x",
                "'kernel.hostname' is not the name of a network setting",
            ),
            (
                "net..ipv4.ip_forward=1",
                "'net..ipv4.ip_forward' is not the name of a network setting",
            ),
            (
                "net.ipv4.ip_forward.=1",
                "'net.ipv4.ip_forward.' is not the name of a network setting",
            ),
            (
                "net.ipv4./.x=1",
                "'net.ipv4./.x' is not the name of a network setting",
            ),
            (
                "net.ipv4.ip forward=1",
                "'net.ipv4.ip forward' is not the name of a network setting",
            ),
            ("net.=1", "'net.' is not the name of a network setting"),
            (
                "net.ipv4.ip_forward",
                "'net.ipv4.ip_forward' is not name=value",
            ),
        ] {
            assert_eq!(split_setting(setting), Err(why.to_owned()), "{setting}");
        }
        for (setting, parts) in [
            ("net.ipv4.ip_forward=1", ("net.ipv4.ip_forward", "1")),
            (
                "net.ipv6.conf.all.disable_ipv6=0",
                ("net.ipv6.conf.all.disable_ipv6", "0"),
            ),
            (
                "net.ipv4.conf.veth-1.rp_filter=",
                ("net.ipv4.conf.veth-1.rp_filter", ""),
            ),
        ] {
            assert_eq!(split_setting(setting), Ok(parts), "{setting}");
        }
        // A refusal stops the rest, and a good name that is not there fails at the write, with no file made
        assert!(
            write_settings(None, &["kernel.x=1", "net.netscenario_no_such_setting=1"]).is_err()
        );
        assert!(write_settings(None, &["net.netscenario_no_such_setting.x=1"]).is_err());
    }

    /// The first thing the layer needs: a socket made by a thread that entered the namespace of another process works in that
    /// namespace from every thread, and the process that asked stays in its own.
    #[test]
    fn a_socket_made_by_a_thread_in_a_node_sees_that_node_and_nothing_else() {
        if rerun_inside(
            "netlink::tests::a_socket_made_by_a_thread_in_a_node_sees_that_node_and_nothing_else",
        ) {
            return;
        }
        let node = Node::new();
        let mut ours = Netlink::here().unwrap();
        ours.add_veth(
            "here0",
            Some("02:4e:53:00:00:01"),
            "here1",
            Some("02:4e:53:00:00:02"),
        )
        .unwrap();
        let mut theirs = Netlink::in_namespace(node.pid()).unwrap();
        // The interfaces of this namespace are not in the node, and the node has none of them
        assert!(theirs.link_index("here0").is_err());
        assert!(theirs.link_index("lo").is_ok());
        // Used from another thread than the one that made it
        let mut theirs = std::thread::spawn(move || {
            theirs.set_state("lo", true).unwrap();
            theirs
        })
        .join()
        .unwrap();
        assert!(
            node.show("ip", &["-o", "link", "show", "lo"])
                .contains("LOOPBACK,UP")
        );
        // The calling thread did not move
        assert!(ours.link_index("here0").is_ok());
        assert!(ours.link_index("lo").is_ok());
        // A link goes into the node and is renamed there
        ours.move_link("here1", node.pid(), Some("l0")).unwrap();
        assert!(ours.link_index("here1").is_err());
        assert!(theirs.link_index("l0").is_ok());
        // An error of the kernel comes back with its words
        let error = ours
            .add_veth("here0", Some("02:4e:53:00:00:03"), "here9", None)
            .unwrap_err();
        assert!(error.contains("File exists"), "{error}");
        let error = theirs.link_index("nothing").unwrap_err();
        assert!(
            error.contains("nothing") && error.contains("No such device"),
            "{error}"
        );
    }

    /// What a link of the provisioner gets, made once by the tool `ip` in one node and once by the layer in another: the dumps
    /// of `ip` of the two are equal. This is the criterion of the layer (BKD-05): the same state as the old path, for both
    /// families, a plain route and a multipath route, and a down and up again.
    #[test]
    #[allow(clippy::too_many_lines)] // two lists of the same steps, one for each way, side by side
    fn the_layer_leaves_the_state_that_ip_leaves() {
        if rerun_inside("netlink::tests::the_layer_leaves_the_state_that_ip_leaves") {
            return;
        }
        let by_tool = Node::new();
        let by_layer = Node::new();
        let (mac_a, mac_b) = ("02:4e:53:00:00:01", "02:4e:53:00:00:02");
        let (mac_c, mac_d) = ("02:4e:53:00:01:01", "02:4e:53:00:01:02");

        // The old path
        for (inside, peer, own, other, a, b, v4, v6) in [
            (
                "l0",
                "p0",
                mac_a,
                mac_b,
                "10.100.0.1/30",
                "10.100.0.2",
                "fd00:100:0::1/126",
                "fd00:100:0::2",
            ),
            (
                "l1",
                "p1",
                mac_c,
                mac_d,
                "10.100.1.1/30",
                "10.100.1.2",
                "fd00:100:1::1/126",
                "fd00:100:1::2",
            ),
        ] {
            by_tool.ip(&[
                "link", "add", inside, "address", own, "type", "veth", "peer", "name", peer,
                "address", other,
            ]);
            by_tool.ip(&["addr", "add", a, "dev", inside]);
            by_tool.ip(&["addr", "add", v4, "dev", inside, "nodad"]);
            by_tool.ip(&[
                "neigh",
                "replace",
                b,
                "lladdr",
                other,
                "dev",
                inside,
                "nud",
                "permanent",
            ]);
            by_tool.ip(&[
                "neigh",
                "replace",
                v6,
                "lladdr",
                other,
                "dev",
                inside,
                "nud",
                "permanent",
            ]);
            by_tool.ip(&["link", "set", inside, "up"]);
            by_tool.ip(&[
                "link",
                "set",
                inside,
                "gso_max_size",
                "1514",
                "gso_max_segs",
                "1",
            ]);
            by_tool.ip(&["link", "set", inside, "mtu", "1400"]);
        }
        by_tool.ip(&[
            "route",
            "add",
            "10.9.9.9/32",
            "metric",
            "7",
            "src",
            "10.100.0.1",
            "via",
            "10.100.0.2",
            "dev",
            "l0",
        ]);
        by_tool.ip(&[
            "-6",
            "route",
            "add",
            "fd00:9::9/128",
            "metric",
            "7",
            "via",
            "fd00:100:0::2",
            "dev",
            "l0",
        ]);
        by_tool.ip(&[
            "route",
            "add",
            "default",
            "metric",
            "9",
            "nexthop",
            "via",
            "10.100.0.2",
            "dev",
            "l0",
            "weight",
            "1",
            "nexthop",
            "via",
            "10.100.1.2",
            "dev",
            "l1",
            "weight",
            "3",
        ]);
        by_tool.ip(&["link", "set", "l0", "down"]);
        by_tool.ip(&["link", "set", "l0", "up"]);
        by_tool.ip(&["addr", "replace", "10.100.0.1/30", "dev", "l0"]);

        // The layer
        let mut layer = Netlink::in_namespace(by_layer.pid()).unwrap();
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        for (inside, peer, own, other, v4, v6, number) in [
            (
                "l0",
                "p0",
                mac_a,
                mac_b,
                ("10.100.0.1", "10.100.0.2"),
                ("fd00:100:0::1", "fd00:100:0::2"),
                0,
            ),
            (
                "l1",
                "p1",
                mac_c,
                mac_d,
                ("10.100.1.1", "10.100.1.2"),
                ("fd00:100:1::1", "fd00:100:1::2"),
                1,
            ),
        ] {
            let _ = number;
            layer
                .add_veth(inside, Some(own), peer, Some(other))
                .unwrap();
            layer.set_address(inside, ip(v4.0), 30, Verb::Add).unwrap();
            layer.set_address(inside, ip(v6.0), 126, Verb::Add).unwrap();
            layer.set_neighbour(inside, ip(v4.1), other).unwrap();
            layer.set_neighbour(inside, ip(v6.1), other).unwrap();
            layer.set_state(inside, true).unwrap();
            layer.set_segments(inside, 1514, 1).unwrap();
            layer.set_mtu(inside, 1400).unwrap();
        }
        let hop = |gateway: &str, interface: &str, weight| Hop {
            gateway: ip(gateway),
            interface: interface.to_owned(),
            weight,
        };
        layer
            .set_route(
                &RouteSpec {
                    destination: Some(ip("10.9.9.9")),
                    metric: 7,
                    source: Some(ip("10.100.0.1")),
                    hops: vec![hop("10.100.0.2", "l0", 1)],
                    v6: false,
                },
                Verb::Add,
            )
            .unwrap();
        layer
            .set_route(
                &RouteSpec {
                    destination: Some(ip("fd00:9::9")),
                    metric: 7,
                    source: None,
                    hops: vec![hop("fd00:100:0::2", "l0", 1)],
                    v6: true,
                },
                Verb::Add,
            )
            .unwrap();
        layer
            .set_route(
                &RouteSpec {
                    destination: None,
                    metric: 9,
                    source: None,
                    hops: vec![hop("10.100.0.2", "l0", 1), hop("10.100.1.2", "l1", 3)],
                    v6: false,
                },
                Verb::Add,
            )
            .unwrap();
        layer.set_state("l0", false).unwrap();
        layer.set_state("l0", true).unwrap();
        layer
            .set_address("l0", ip("10.100.0.1"), 30, Verb::Replace)
            .unwrap();

        let expected = state(&by_tool);
        let found = state(&by_layer);
        assert!(
            expected.contains("nexthop via 10.100.1.2 dev l1 weight 3"),
            "{expected}"
        );
        assert_eq!(found, expected);
        // The canary: one more attribute in the layer's node and the comparison sees it
        layer.set_mtu("l1", 1300).unwrap();
        assert_ne!(state(&by_layer), expected);

        // The settings of the kernel in the namespace of a node
        // A new namespace takes the value of the machine, so both start from a value that is written
        write_settings(Some(by_tool.pid()), &["net.ipv4.ip_forward=0"]).unwrap();
        write_settings(
            Some(by_layer.pid()),
            &["net.ipv4.ip_forward=1", "net.ipv4.conf.all.rp_filter=0"],
        )
        .unwrap();
        assert_eq!(
            by_layer
                .show("cat", &["/proc/sys/net/ipv4/ip_forward"])
                .trim(),
            "1"
        );
        assert_eq!(
            by_tool
                .show("cat", &["/proc/sys/net/ipv4/ip_forward"])
                .trim(),
            "0"
        );
        // The names are checked before a file is opened
        for setting in [
            "kernel.hostname=x",
            "net..ipv4.ip_forward=1",
            "net.ipv4.ip_forward",
        ] {
            assert!(
                write_settings(Some(by_layer.pid()), &[setting]).is_err(),
                "{setting}"
            );
        }
    }
}
