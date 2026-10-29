//! The Kernel backend, the reference of the project (D-005): one network namespace per node, virtual
//! Ethernet pairs as links, `tc` with `netem` on each direction, and the cleanup guarantee (BKD-01 to
//! BKD-04, SAF-01 to SAF-03, release 0.1). It calls `ip` and `tc` (D-009).
//!
//! The crate is a skeleton. The first code is the provisioning of a pair of namespaces.
