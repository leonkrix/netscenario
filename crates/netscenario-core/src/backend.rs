//! The interface that every backend implements (BKD-03). The engine of a run uses nothing else, so a new
//! backend means implementing this trait and filling its column in the coverage matrix.

use std::collections::BTreeMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::time::Duration;

use netscenario_model::{Impairments, IpFamily, Name, Scenario};

use crate::capability::{Capabilities, CapabilityError};
use crate::error::Error;

/// A node that a backend created. Only the backend knows what it is (a network namespace for the Kernel
/// backend).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeHandle(pub u32);

/// A link that a backend created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkHandle(pub u32);

/// A program that a backend started in a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramHandle(pub u32);

/// One of the two directions of a link. `Forward` goes from the first node of `between` to the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// From the first node of `between` to the second.
    Forward,
    /// From the second node of `between` to the first.
    Reverse,
}

/// A program to run in a node.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Program {
    /// The program and its arguments.
    pub argv: Vec<String>,
    /// The most address space the program may use, in bytes (SAF-02). Without it the program is not limited.
    /// A program that needs more fails to allocate and ends; it does not take the machine down with it.
    pub memory_limit: Option<u64>,
    /// Variables to add to the environment of the program, which otherwise gets that of the backend (D-072).
    pub env: Vec<(String, String)>,
    /// The directory the program runs in. Without it, the directory of the backend.
    pub working_dir: Option<PathBuf>,
    /// Whether the program is the leader of a process group of its own, so that [`Backend::signal_program`] reaches
    /// its children too (D-041). The agent is not; a program of your own is.
    pub own_group: bool,
    /// Where the backend writes what the program wrote to its standard output, as it comes. The first
    /// [`LOG_LIMIT`] bytes are kept; the rest is read and counted ([`OutputStats`]).
    pub stdout_file: Option<PathBuf>,
    /// The same for standard error.
    pub stderr_file: Option<PathBuf>,
}

/// How much of the standard output and of the standard error of a program is written to its files: the first megabyte.
/// What the program writes after that is read, so that it is never blocked, and counted (D-072).
pub const LOG_LIMIT: u64 = 1024 * 1024;

/// The longest line of standard output that the backend hands out and writes: the rest of a longer line is dropped
/// and the line is counted in [`OutputStats::long_lines`] (D-072).
pub const LINE_LIMIT: usize = 64 * 1024;

impl Program {
    /// A program without a memory limit.
    #[must_use]
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            memory_limit: None,
            env: Vec::new(),
            working_dir: None,
            own_group: false,
            stdout_file: None,
            stderr_file: None,
        }
    }

    /// The same program with more variables in its environment.
    #[must_use]
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// The same program in another directory.
    #[must_use]
    pub fn with_working_dir(mut self, directory: PathBuf) -> Self {
        self.working_dir = Some(directory);
        self
    }

    /// The same program as the leader of a process group of its own.
    #[must_use]
    pub fn in_own_group(mut self) -> Self {
        self.own_group = true;
        self
    }

    /// The same program with its output written to files.
    #[must_use]
    pub fn with_log_files(mut self, stdout: PathBuf, stderr: PathBuf) -> Self {
        self.stdout_file = Some(stdout);
        self.stderr_file = Some(stderr);
        self
    }

    /// The same program with a limit on its address space.
    #[must_use]
    pub fn with_memory_limit(mut self, bytes: u64) -> Self {
        self.memory_limit = Some(bytes);
        self
    }
}

/// How a program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramExit {
    /// It returned this exit code.
    Code(i32),
    /// The system ended it with this signal (for example 9 when it was killed, or 11 after a crash).
    Signal(i32),
}

impl ProgramExit {
    /// Whether the program ended with the exit code 0.
    #[must_use]
    pub fn success(self) -> bool {
        self == ProgramExit::Code(0)
    }
}

impl fmt::Display for ProgramExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProgramExit::Code(code) => write!(f, "exit code {code}"),
            ProgramExit::Signal(signal) => write!(f, "killed by signal {signal}"),
        }
    }
}

/// A signal for the process group of a program (D-041). `Kill` is what follows when the others did not end it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProgramSignal {
    /// `SIGTERM`.
    Term,
    /// `SIGINT`.
    Int,
    /// `SIGHUP`.
    Hup,
    /// `SIGQUIT`.
    Quit,
    /// `SIGUSR1`.
    Usr1,
    /// `SIGUSR2`.
    Usr2,
    /// `SIGKILL`.
    Kill,
}

impl From<netscenario_model::Signal> for ProgramSignal {
    fn from(signal: netscenario_model::Signal) -> Self {
        use netscenario_model::Signal;
        match signal {
            Signal::Term => Self::Term,
            Signal::Int => Self::Int,
            Signal::Hup => Self::Hup,
            Signal::Quit => Self::Quit,
            Signal::Usr1 => Self::Usr1,
            Signal::Usr2 => Self::Usr2,
        }
    }
}

/// Where a program's transport protocol is looked at when the run asks whether a port is taken (D-072).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortKind {
    /// A UDP socket that is bound.
    Udp,
    /// A TCP socket that listens.
    Tcp,
}

/// What the backend read from the output of a program so far, all of it since the start (D-072).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputStats {
    /// Bytes read from standard output.
    pub stdout_bytes: u64,
    /// Bytes of standard output that are not in its file (past the limit) or not in the lines (past the line limit).
    pub stdout_dropped_bytes: u64,
    /// Bytes read from standard error.
    pub stderr_bytes: u64,
    /// Bytes of standard error that are not in its file.
    pub stderr_dropped_bytes: u64,
    /// Lines of standard output that were longer than [`LINE_LIMIT`].
    pub long_lines: u64,
}

/// What a program has done since the last time it was asked about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProgramState {
    /// The lines the program wrote to its standard output since the last call, in order. The agent reports
    /// its events this way.
    pub lines: Vec<String>,
    /// How the program ended, once it has. All of its output is in `lines` by then.
    pub exit: Option<ProgramExit>,
    /// The end of what the program wrote to its standard error (at most the last 16 KiB), all of it so far.
    pub stderr: String,
    /// Whether lines were dropped because the program wrote more than the backend keeps (10,000 lines).
    pub truncated: bool,
    /// What was read and what was dropped, since the start.
    pub output: OutputStats,
}

/// What a backend says about itself for the provenance of a result (ENV-02): its name and the facts about the
/// machine side of it that a reader needs to judge a result, such as the version of `tc` or how the rights
/// were obtained.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendInfo {
    /// The name of the backend (`kernel`).
    pub name: String,
    /// Facts as name and value, for example `tc` and `iproute2 6.15.0`.
    pub details: BTreeMap<String, String>,
}

/// What the node at the receiving end of one direction of a link has counted since the network was built.
/// Two readings, one before the workloads start and one after, give what a run caused (D-040).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct NodeCounts {
    /// The frames that arrived at the interface of the link, whether or not the node went on to use them. A frame
    /// that the queue of the sending side dropped never arrives; a frame that was corrupted does, and the node
    /// then drops it (a bad checksum, an address that is not its own).
    pub frames_received: u64,
    /// The datagrams that the node dropped because the receive buffer of a socket was full. They are a
    /// fault of the receiver, not of the network, so they are not counted as corruption.
    pub receive_buffer_drops: u64,
}

impl NodeCounts {
    /// Counts with these values.
    #[must_use]
    pub fn new(frames_received: u64, receive_buffer_drops: u64) -> Self {
        Self {
            frames_received,
            receive_buffer_drops,
        }
    }
}

/// What the kernel counted in the queue of one direction (PRB-10, D-082): the leaf of the shaper, where the discipline drops.
/// The numbers are those of `tc -s` at the end of the run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct QueueStats {
    /// The discipline the queue has, as the kernel names it: `pfifo`, `bfifo`, `codel`, `fq_codel`, `pie`.
    pub kind: String,
    /// The packets that left the queue (were sent on).
    pub packets: u64,
    /// The bytes of those packets.
    pub bytes: u64,
    /// The packets the queue dropped: by its limit, by its discipline, and the ones it marked instead are not here.
    pub drops: u64,
    /// The packets that were over the limit of the queue (the kernel counts them with the drops).
    pub overlimits: u64,
    /// The packets that were put back for another try.
    pub requeues: u64,
    /// The packets waiting in the queue at the end.
    pub backlog_packets: u64,
    /// The bytes waiting in the queue at the end.
    pub backlog_bytes: u64,
    /// The counters that are special to the discipline, by the name the kernel gives them (`ecn_mark`, `drop_overlimit`,
    /// `maxpacket`, `new_flow_count`, ...).
    pub details: BTreeMap<String, u64>,
}

impl QueueStats {
    /// Statistics of a queue of the kind `kind` with the counters of the common part.
    #[must_use]
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ..Self::default()
        }
    }
}

/// One way out of a node for a route: a link, and the weight among the ways of the same metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextHop {
    /// The link the packets leave by.
    pub link: LinkHandle,
    /// The end of the link that the node is at: 0 for the first node of `between`, 1 for the second. The next hop is the
    /// address of the other end.
    pub end: usize,
    /// The share among the ways of the same route, 1 or more.
    pub weight: u32,
}

/// A route of a node as the backend gets it (D-078): to a node by its address, or to every address, out of one or several links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRoute {
    /// The family of the route: its destination and the next hops are addresses of this family.
    pub family: IpFamily,
    /// The address that the route leads to (a host route), or `None` for every address of the family that has no route of
    /// its own. The family of the address is `family`.
    pub destination: Option<IpAddr>,
    /// The preference: the smaller is used first.
    pub metric: u32,
    /// The ways out, one or several (a multipath route shares the packets among them by their weights).
    pub next_hops: Vec<NextHop>,
}

/// A way to make a network real: it reports what it can reproduce and builds what a scenario describes.
///
/// The methods are in the order of a run: create the nodes, connect them, apply the impairments, start
/// programs, tear everything down. They are synchronous: the Kernel backend calls tools and waits for them.
///
/// The trait is small on purpose and grows with the releases: waiting for a program and streaming its
/// records arrive with the agent (release 0.1), packet capture with release 0.2.
pub trait Backend {
    /// The name that a scenario uses in `backend:` (`kernel`).
    fn name(&self) -> &'static str;

    /// What this backend reproduces on this machine. It is a property of the backend as it was set up, not of
    /// a scenario, and it does not change during a run.
    fn capabilities(&self) -> Capabilities;

    /// Creates a node called `name`.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the node cannot be created.
    fn create_node(&mut self, name: &Name) -> Result<NodeHandle, Error>;

    /// Connects two nodes with a link called `name`.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link cannot be created.
    fn create_link(&mut self, name: &Name, between: [NodeHandle; 2]) -> Result<LinkHandle, Error>;

    /// Checks the values of a scenario against what this backend can apply exactly, for example a delay in
    /// nanoseconds where the kernel works in microseconds. It runs after the check of the features and before
    /// anything is created, and refuses in the same way. The default accepts everything.
    ///
    /// # Errors
    ///
    /// A [`CapabilityError`] with one refusal for each value that cannot be applied exactly.
    fn check_values(&self, _scenario: &Scenario) -> Result<(), CapabilityError> {
        Ok(())
    }

    /// Tells the backend every state that one direction of a link will have in the run, the first being the one
    /// that [`apply`](Backend::apply) is given next, before it is. A backend whose queues are built for a state can
    /// build ones that every state fits, so that [`change`](Backend::change) never has to rebuild them (D-059). It is
    /// called only for a direction that a `set` or an `outage` of the timeline touches. The default does nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist.
    fn plan_states(
        &mut self,
        _link: LinkHandle,
        _direction: Direction,
        _states: &[Impairments],
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Tells the backend that one direction of a link will have an outage in the run (D-064), so that it can build
    /// the queue that holds one before the direction is built, and not rebuild anything when the outage begins
    /// ([`set_outage`](Backend::set_outage)). It is called after [`plan_states`](Backend::plan_states) of the same
    /// direction and before [`apply`](Backend::apply). The default does nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist, or the direction was not planned with `plan_states`.
    fn plan_outage(&mut self, _link: LinkHandle, _direction: Direction) -> Result<(), Error> {
        Ok(())
    }

    /// Sets the impairments of one direction of a link. What is not set in `impairments` is not impaired.
    /// `seed` seeds the random loss of this direction (see [`derive_seed`](crate::derive_seed)); without it
    /// the loss is random from run to run.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the values cannot be applied.
    fn apply(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        impairments: &Impairments,
        seed: Option<u64>,
    ) -> Result<(), Error>;

    /// Changes the impairments of one direction of a link while the run is going: from now on the direction has
    /// `impairments`, the whole state and not the difference to the last one. What the last state had and this one
    /// does not is gone, and the queue and its random numbers go on (D-048, D-059). The state must be one of
    /// those given to [`plan_states`](Backend::plan_states). The default says that the backend cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the backend does not change impairments over time, or the change cannot be applied.
    fn change(
        &mut self,
        _link: LinkHandle,
        _direction: Direction,
        _impairments: &Impairments,
    ) -> Result<(), Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "change the impairments of a direction".to_owned(),
            message: "this backend does not change impairments over time".to_owned(),
        })
    }

    /// Begins (`on`) or ends an outage of one direction of a link (D-064): while it lasts nothing is delivered in
    /// that direction, not even a packet that is on its way, and the sender notices nothing. The impairments of the
    /// direction are not touched and act as they did. The direction must have been planned with [`plan_outage`](Backend::plan_outage). The
    /// default says that the backend cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the backend does not have outages, or the direction was not planned for one.
    fn set_outage(
        &mut self,
        _link: LinkHandle,
        _direction: Direction,
        _on: bool,
    ) -> Result<(), Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "begin or end an outage".to_owned(),
            message: "this backend does not have outages".to_owned(),
        })
    }

    /// Takes the interface of a link down (`up` false) or brings it up again (D-039, D-064). Both ends go: a
    /// sender on either side gets errors while it is down, which the agent counts, and the packets that were in
    /// flight are dealt with by the kernel. The impairments of the link are what they were when it comes back. The
    /// default says that the backend cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the backend does not take links down, or the link cannot be switched.
    fn set_link(&mut self, _link: LinkHandle, _up: bool) -> Result<(), Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "take a link down or bring it up".to_owned(),
            message: "this backend does not take links down".to_owned(),
        })
    }

    /// What the backend wants the result to say about how it did the run: facts, not errors, for example that a
    /// change was made the slow way after the fast way stopped working (D-063). Asked once, at the end of the
    /// measuring and before the teardown. The default has nothing to say.
    fn notes(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// Starts `program` in `node`. Its standard output is read by [`poll_program`](Backend::poll_program),
    /// its standard input is written by [`send_line`](Backend::send_line), and its standard error is kept.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the program cannot be started.
    fn start_program(
        &mut self,
        node: NodeHandle,
        program: &Program,
    ) -> Result<ProgramHandle, Error>;

    /// How many packets entered the queue that draws the random loss of one direction of a link, since the
    /// impairments were applied (ENV-07). The pattern of the lost packets depends on this number as well as on
    /// the seed, so a packet that was not sent by the workloads shifts it, and a run compares the number with
    /// the packets it sent. `None` if the backend cannot count them, which is the default; then no comparison
    /// is made.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist or the queue cannot be read.
    fn queue_packets(
        &mut self,
        _link: LinkHandle,
        _direction: Direction,
    ) -> Result<Option<u64>, Error> {
        Ok(None)
    }

    /// How many packets are in the queues of one direction of a link now: waiting for the rate, held back for the delay,
    /// or on their way through any other stage (D-067). The run uses it after the workloads, to wait until the packets
    /// that were sent have left the link, and not for a time it has to guess. A direction without a queue has none.
    /// `None` if the backend cannot say, which is the default; the run then waits for a time worked out from the
    /// delays of the scenario.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist or the queue cannot be read.
    fn queue_backlog(
        &mut self,
        _link: LinkHandle,
        _direction: Direction,
    ) -> Result<Option<u64>, Error> {
        Ok(None)
    }

    /// What the node that receives in one direction of a link has counted (see [`NodeCounts`]). The run reads it
    /// before and after the workloads and accounts for every packet that was sent: lost in the queue,
    /// dropped by the node, or delivered (D-040). `None` if the backend cannot count, which is the default;
    /// then the result has no tally for the direction.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist or the counters cannot be read.
    fn node_counts(
        &mut self,
        _link: LinkHandle,
        _direction: Direction,
    ) -> Result<Option<NodeCounts>, Error> {
        Ok(None)
    }

    /// Says which addresses the network has, before anything is created (D-081). IPv6 is only on where the scenario asks for
    /// it, because its interfaces send packets of their own. The default accepts IPv4 only.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the backend cannot build a network with IPv6.
    fn set_ip_families(&mut self, _v4: bool, v6: bool) -> Result<(), Error> {
        if v6 {
            return Err(Error::Backend {
                backend: self.name().to_owned(),
                action: "use IPv6".to_owned(),
                message: "this backend has no IPv6".to_owned(),
            });
        }
        Ok(())
    }

    /// Called when the addresses and the routes are set and before the first queue is built (D-081): the backend waits
    /// until the network is quiet, so that the packets that the interfaces send when they come up (IPv6 neighbour discovery and
    /// multicast listener reports) do not enter a queue and shift a seeded loss. The default does nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the wait fails.
    fn finish_network(&mut self) -> Result<(), Error> {
        Ok(())
    }

    /// Gives a node an address of its own that is not on a link (on its loopback, D-078): the address that other nodes route
    /// to and that programs use for it. The family is that of the address. The default says that the backend cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the node does not exist or the address cannot be set.
    fn set_node_address(&mut self, _node: NodeHandle, _address: IpAddr) -> Result<(), Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "give a node an address".to_owned(),
            message: "this backend has no addresses of nodes".to_owned(),
        })
    }

    /// Connects two nodes through the fabric of the run (D-079): the link a direction with a `queue` needs, with a queue of its own
    /// that holds the packets for the rate, and the delay behind it. Everything else about the link is as for
    /// [`Backend::create_link`]. The default says that the backend cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the nodes do not exist or the link cannot be made.
    fn create_link_through_fabric(
        &mut self,
        _name: &Name,
        _between: [NodeHandle; 2],
    ) -> Result<LinkHandle, Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "create a link with a queue".to_owned(),
            message: "this backend has no queues of its own".to_owned(),
        })
    }

    /// What the kernel counted in the queue that a direction of a link has (PRB-10): `None` for a direction without a `queue`,
    /// and for a backend that cannot say, which is the default.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist or the counters cannot be read.
    fn queue_stats(
        &mut self,
        _link: LinkHandle,
        _direction: Direction,
    ) -> Result<Option<QueueStats>, Error> {
        Ok(None)
    }

    /// Sets the MTU of the interfaces at both ends of a link, in bytes (D-081). The default says that the backend cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist or the MTU cannot be set.
    fn set_link_mtu(&mut self, _link: LinkHandle, _mtu: u32) -> Result<(), Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "set the MTU of a link".to_owned(),
            message: "this backend cannot set the MTU of a link".to_owned(),
        })
    }

    /// Makes a node forward packets between its links (a router) or not (a host, the default). The default says that the
    /// backend cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the node does not exist or forwarding cannot be switched.
    fn set_forwarding(&mut self, _node: NodeHandle, _on: bool) -> Result<(), Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "make a node forward packets".to_owned(),
            message: "this backend has no routers".to_owned(),
        })
    }

    /// Adds a route to a node. The links of its next hops are connected to the node. The default says that the backend cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the node or a link does not exist, a link is not at the node, or the route is refused.
    fn add_route(&mut self, _node: NodeHandle, _route: &NodeRoute) -> Result<(), Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "add a route".to_owned(),
            message: "this backend has no routes".to_owned(),
        })
    }

    /// The address of one end of a link, as the programs in the nodes use it. `end` is 0 for the first node of
    /// `between` and 1 for the second. The addresses are the backend's to assign (TOP-08) and are recorded in
    /// the result.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist.
    fn address(&self, link: LinkHandle, end: usize) -> Result<Ipv4Addr, Error>;

    /// The IPv6 address of one end of a link (D-081), when the network has IPv6. The default says that the backend has none.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist or the backend has no IPv6.
    fn address_v6(&self, _link: LinkHandle, _end: usize) -> Result<Ipv6Addr, Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "find an IPv6 address".to_owned(),
            message: "this backend has no IPv6".to_owned(),
        })
    }

    /// The name of the interface of one end of a link inside its node, as a capture asks for it (PRB-09). `end` is 0 for the
    /// first node of `between` and 1 for the second.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist, or the backend has no interfaces to name (the default).
    fn interface_name(&self, _link: LinkHandle, _end: usize) -> Result<String, Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "name an interface".to_owned(),
            message: "the backend cannot do it".to_owned(),
        })
    }

    /// What the backend says about itself for the provenance of a result.
    fn describe(&self) -> BackendInfo;

    /// Waits for news from a program: returns as soon as it has written a line or has ended, or after `wait`
    /// if neither happened (then the state is empty and `exit` is `None`).
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the program does not exist in this run.
    fn poll_program(
        &mut self,
        program: ProgramHandle,
        wait: Duration,
    ) -> Result<ProgramState, Error>;

    /// Sends `signal` to the process group of a program that was started with [`Program::own_group`]. A program that
    /// has ended is not an error.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the program does not exist in this run, was not started in a group of its own, or the
    /// backend cannot signal programs (the default).
    fn signal_program(
        &mut self,
        _program: ProgramHandle,
        _signal: ProgramSignal,
    ) -> Result<(), Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "signal a program".to_owned(),
            message: "the backend cannot do it".to_owned(),
        })
    }

    /// Whether a socket of `kind` is bound to `port` in `node`, to any address or to its own: how the run learns that a
    /// program of your own is ready (D-041, S14). It reads the sockets of the node and needs no help from the program.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the node does not exist or its sockets cannot be read, or the backend cannot do it
    /// (the default).
    fn port_bound(
        &mut self,
        _node: NodeHandle,
        _kind: PortKind,
        _port: u16,
    ) -> Result<bool, Error> {
        Err(Error::Backend {
            backend: self.name().to_owned(),
            action: "look at the ports of a node".to_owned(),
            message: "the backend cannot do it".to_owned(),
        })
    }

    /// Writes a line to the standard input of a program. The receiver of the agent stops when it reads
    /// `stop`. A program that has ended is not an error.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the program does not exist in this run.
    fn send_line(&mut self, program: ProgramHandle, line: &str) -> Result<(), Error>;

    /// Removes everything the backend created and ends the programs it started. It can be called after a
    /// failure at any point, and calling it twice does no harm.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if something could not be removed. The message says what is left.
    fn teardown(&mut self) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that does only what every backend must, and leaves the rest to the defaults of the trait. What a user gets from
    /// a backend that lacks a feature is what these defaults say, so each is checked: a default that says "yes" or says nothing
    /// would let a scenario run on a backend that cannot reproduce it.
    struct Bare;

    impl Backend for Bare {
        fn name(&self) -> &'static str {
            "bare"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::new("bare")
        }
        fn create_node(&mut self, _name: &Name) -> Result<NodeHandle, Error> {
            Ok(NodeHandle(0))
        }
        fn create_link(
            &mut self,
            _name: &Name,
            _between: [NodeHandle; 2],
        ) -> Result<LinkHandle, Error> {
            Ok(LinkHandle(0))
        }
        fn apply(
            &mut self,
            _link: LinkHandle,
            _direction: Direction,
            _impairments: &Impairments,
            _seed: Option<u64>,
        ) -> Result<(), Error> {
            Ok(())
        }
        fn start_program(
            &mut self,
            _node: NodeHandle,
            _program: &Program,
        ) -> Result<ProgramHandle, Error> {
            Ok(ProgramHandle(0))
        }
        fn address(&self, _link: LinkHandle, _end: usize) -> Result<Ipv4Addr, Error> {
            Ok(Ipv4Addr::LOCALHOST)
        }
        fn describe(&self) -> BackendInfo {
            BackendInfo {
                name: "bare".to_owned(),
                details: BTreeMap::new(),
            }
        }
        fn poll_program(
            &mut self,
            _program: ProgramHandle,
            _wait: Duration,
        ) -> Result<ProgramState, Error> {
            Ok(ProgramState::default())
        }
        fn send_line(&mut self, _program: ProgramHandle, _line: &str) -> Result<(), Error> {
            Ok(())
        }
        fn teardown(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    const LINK: LinkHandle = LinkHandle(1);
    const NODE: NodeHandle = NodeHandle(1);

    /// The three parts of a refusal, or a panic with what the call gave instead.
    fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> (String, String, String) {
        match result {
            Err(Error::Backend {
                backend,
                action,
                message,
            }) => (backend, action, message),
            other => panic!("a backend without the feature must refuse, and it gave {other:?}"),
        }
    }

    /// Every default that refuses says which backend refuses, what it was asked and why: the message is what the user reads
    /// ("the bare backend failed to ..."). Canary: three of them said "this" for the backend, which no reader can place.
    #[test]
    fn a_backend_without_a_feature_refuses_and_names_itself_and_the_action() {
        let mut bare = Bare;
        let route = NodeRoute {
            family: IpFamily::V4,
            destination: None,
            metric: 100,
            next_hops: Vec::new(),
        };
        let name = Name::new("a").unwrap();
        let refusals = [
            refusal(bare.change(LINK, Direction::Forward, &Impairments::default())),
            refusal(bare.set_outage(LINK, Direction::Forward, true)),
            refusal(bare.set_link(LINK, false)),
            refusal(bare.set_ip_families(true, true)),
            refusal(bare.set_node_address(NODE, IpAddr::V4(Ipv4Addr::LOCALHOST))),
            refusal(bare.create_link_through_fabric(&name, [NODE, NODE])),
            refusal(bare.set_link_mtu(LINK, 1400)),
            refusal(bare.set_forwarding(NODE, true)),
            refusal(bare.add_route(NODE, &route)),
            refusal(bare.address_v6(LINK, 0)),
            refusal(bare.interface_name(LINK, 0)),
            refusal(bare.signal_program(ProgramHandle(0), ProgramSignal::Term)),
            refusal(bare.port_bound(NODE, PortKind::Udp, 4433)),
        ];
        let mut actions = std::collections::BTreeSet::new();
        for (backend, action, message) in refusals {
            assert_eq!(backend, "bare", "{action}: {message}");
            assert!(!action.is_empty() && !message.is_empty());
            actions.insert(action);
        }
        assert_eq!(
            actions.len(),
            13,
            "each refusal says what it was asked: {actions:?}"
        );
        // What the user reads
        let (backend, action, message) = refusal(bare.set_link(LINK, false));
        let error = Error::Backend {
            backend,
            action,
            message,
        };
        assert_eq!(
            error.to_string(),
            "the bare backend failed to take a link down or bring it up: this backend does not take links down"
        );
    }

    /// The defaults that have nothing to say are neutral: they accept, or answer that the backend cannot count, and a run then
    /// leaves the comparison out instead of failing on it.
    #[test]
    fn a_backend_that_counts_nothing_says_so_and_accepts_the_steps_that_need_no_work() {
        let mut bare = Bare;
        let scenario = Scenario::from_yaml(
            "version: 0\nname: bare\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - { id: l, between: [a, b] }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }\n",
            "n.yaml",
        )
        .unwrap();
        assert!(bare.check_values(&scenario).is_ok());
        assert!(bare.plan_states(LINK, Direction::Forward, &[]).is_ok());
        assert!(bare.plan_outage(LINK, Direction::Reverse).is_ok());
        assert_eq!(bare.notes(), Vec::<String>::new());
        assert_eq!(bare.queue_packets(LINK, Direction::Forward).unwrap(), None);
        assert_eq!(bare.queue_backlog(LINK, Direction::Forward).unwrap(), None);
        assert!(
            bare.node_counts(LINK, Direction::Forward)
                .unwrap()
                .is_none()
        );
        assert!(
            bare.queue_stats(LINK, Direction::Forward)
                .unwrap()
                .is_none()
        );
        assert!(bare.set_ip_families(true, false).is_ok());
        assert!(bare.finish_network().is_ok());
    }

    #[test]
    fn a_signal_is_the_signal_of_the_model() {
        assert_eq!(
            ProgramSignal::from(netscenario_model::Signal::Int),
            ProgramSignal::Int
        );
        assert_eq!(
            ProgramSignal::from(netscenario_model::Signal::Term),
            ProgramSignal::Term
        );
    }
}
