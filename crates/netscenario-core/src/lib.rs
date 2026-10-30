//! The run lifecycle: what a backend must provide, the check of a scenario against what it reproduces
//! faithfully, and the building of the network (see `docs/ARCHITECTURE.md`, "The life of a run").
//!
//! A backend implements [`Backend`] and declares its [`Capabilities`]. [`provision`] checks a scenario
//! against them, so that a scenario the backend cannot reproduce exactly is refused before anything is
//! created (BKD-03), and then builds the nodes, links and impairments through the trait.
//!
//! The timeline, the link models, statistics and results arrive with the later parts of release 0.1.

mod backend;
mod capability;
mod error;
mod feature;
mod provision;
mod seed;

pub use backend::{
    Backend, Direction, LinkHandle, NodeHandle, Program, ProgramExit, ProgramHandle, ProgramState,
};
pub use capability::{Accepted, Capabilities, CapabilityError, Refusal, Support, check};
pub use error::Error;
pub use feature::{Feature, Requirement, requirements};
pub use provision::{Provisioned, impairments_of, provision};
pub use seed::{derive_seed, draw_seed};
