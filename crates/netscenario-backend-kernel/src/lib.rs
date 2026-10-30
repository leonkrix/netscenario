//! The Kernel backend, the reference of the project (D-005): one network namespace per node, virtual
//! Ethernet pairs as links, `tc` with `netem` on each direction, and the cleanup guarantee (BKD-01 to
//! BKD-04, SAF-01 to SAF-03, release 0.1). It calls `ip`, `tc`, `unshare` and `nsenter` (D-009, D-022).
//!
//! A run is owned by a supervisor process, the first process of a PID namespace in a network namespace of
//! its own. When it dies, the kernel ends everything that belongs to the run. [`KernelBackend`] starts the
//! supervisor and implements [`netscenario_core::Backend`]; [`capabilities()`] declares what it reproduces.

mod capabilities;
pub mod clean;
mod client;
mod command;
pub mod doctor;
mod netem;
mod protocol;
mod provisioner;
pub mod supervisor;
mod tools;

pub use capabilities::{Environment, capabilities};
pub use client::{KernelBackend, KernelOptions, Mode};
pub use command::Toolbox;
pub use tools::{Tool, Version, parse_tc_version};
