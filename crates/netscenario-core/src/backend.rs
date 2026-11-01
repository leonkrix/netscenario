//! The interface that every backend implements (BKD-03). The engine of a run uses nothing else, so a new
//! backend means implementing this trait and filling its column in the coverage matrix.

use std::collections::BTreeMap;
use std::fmt;
use std::net::Ipv4Addr;
use std::time::Duration;

use netscenario_model::{Impairments, Name, Scenario};

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
}

impl Program {
    /// A program without a memory limit.
    #[must_use]
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            memory_limit: None,
        }
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

    /// The address of one end of a link, as the programs in the nodes use it. `end` is 0 for the first node of
    /// `between` and 1 for the second. The addresses are the backend's to assign (TOP-08) and are recorded in
    /// the result.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the link does not exist.
    fn address(&self, link: LinkHandle, end: usize) -> Result<Ipv4Addr, Error>;

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
