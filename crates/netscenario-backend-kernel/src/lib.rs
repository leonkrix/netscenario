//! The Kernel backend, the reference of the project (D-005): one network namespace per node, virtual
//! Ethernet pairs as links, `tc` with `netem` on each direction, and the cleanup guarantee (BKD-01 to
//! BKD-04, SAF-01 to SAF-03, release 0.1). It calls `ip` and `tc` (D-009).
//!
//! So far the crate declares what the backend reproduces ([`capabilities`]) and reads the version of the
//! tools that the declaration depends on. The provisioning of namespaces follows.

mod capabilities;
mod tools;

pub use capabilities::{Environment, capabilities};
pub use tools::{Tool, Version, parse_tc_version};
