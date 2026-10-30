//! The interface that every backend implements (BKD-03). The engine of a run uses nothing else, so a new
//! backend means implementing this trait and filling its column in the coverage matrix.

use netscenario_model::{Impairments, Name};

use crate::capability::Capabilities;
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
pub struct Program {
    /// The program and its arguments.
    pub argv: Vec<String>,
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

    /// Sets the impairments of one direction of a link. What is not set in `impairments` is not impaired.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the values cannot be applied.
    fn apply(
        &mut self,
        link: LinkHandle,
        direction: Direction,
        impairments: &Impairments,
    ) -> Result<(), Error>;

    /// Starts `program` in `node`.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if the program cannot be started.
    fn start_program(
        &mut self,
        node: NodeHandle,
        program: &Program,
    ) -> Result<ProgramHandle, Error>;

    /// Removes everything the backend created and ends the programs it started. It can be called after a
    /// failure at any point, and calling it twice does no harm.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] if something could not be removed. The message says what is left.
    fn teardown(&mut self) -> Result<(), Error>;
}
