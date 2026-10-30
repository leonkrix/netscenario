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
