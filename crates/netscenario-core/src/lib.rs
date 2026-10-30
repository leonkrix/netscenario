//! The run lifecycle: what a backend must provide, the check of a scenario against what it reproduces
//! faithfully, and the building of the network (see `docs/ARCHITECTURE.md`, "The life of a run").
//!
//! A backend implements [`Backend`] and declares its [`Capabilities`]. [`provision`] checks a scenario
//! against them, so that a scenario the backend cannot reproduce exactly is refused before anything is
//! created (BKD-03), and then builds the nodes, links and impairments through the trait.
//!
//! [`run`] then starts the agents in the nodes, lets the workloads run and returns a [`RunResult`] with the
//! statistics of [`stats`]. The timeline and the link models arrive with the later releases.

mod backend;
mod capability;
mod error;
mod export;
mod feature;
mod hash;
mod machine;
mod provision;
mod result;
mod run;
mod seed;
pub mod stats;

pub use backend::{
    Backend, BackendInfo, Direction, LinkHandle, NodeHandle, Program, ProgramExit, ProgramHandle,
    ProgramState,
};
pub use capability::{Accepted, Capabilities, CapabilityError, Refusal, Support, check};
pub use error::Error;
pub use export::write_packets;
pub use feature::{Feature, Requirement, requirements};
pub use hash::sha256_hex;
pub use machine::{Provenance, collect as collect_provenance};
pub use provision::{NodeAddress, Provisioned, impairments_of, provision};
pub use result::{
    AcceptedRestriction, AddressInfo, RESULT_FORMAT, RESULT_VERSION, RunResult, ScenarioInfo,
    SeedInfo, SenderInfo, Timing, WorkloadResult,
};
pub use run::{RunOptions, agent_next_to, run};
pub use seed::{derive_seed, draw_seed};
