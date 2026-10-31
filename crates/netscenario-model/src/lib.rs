//! The scenario model: nodes, links, impairments and workloads as types, with strict units, validation, and
//! the JSON Schema that is generated from them (SCN-01 to SCN-04, release 0.1).
//!
//! This crate has no access to the system: it parses, checks and describes scenarios, nothing more. The
//! format is defined in `docs/SCENARIO-FORMAT.md`.
//!
//! ```
//! use netscenario_model::Scenario;
//!
//! let yaml = "
//! version: 0
//! name: example
//! duration: 10s
//! topology:
//!   nodes: [client, server]
//!   links:
//!     - id: access
//!       between: [client, server]
//!       both: { delay: 20ms }
//! workloads:
//!   - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
//! ";
//! let scenario = Scenario::from_yaml(yaml, "example.yaml")?;
//! assert_eq!(scenario.name.as_str(), "example");
//! # Ok::<(), netscenario_model::Error>(())
//! ```

mod builder;
mod command;
mod decimal;
pub mod error;
mod read;
mod required;
mod reserved;
mod scenario;
mod schema;
mod source;
mod suggest;
pub mod units;
mod validate;

pub use builder::ScenarioBuilder;
pub use command::{MAX_ARGUMENT_BYTES, MAX_ARGUMENTS, MAX_ENV_ENTRIES, RESERVED_ENV_PREFIX};
pub use error::{Error, Location, Problem};
pub use read::MAX_FILE_BYTES;
pub use scenario::{
    Backend, Change, Command, Direction, Edit, Event, FormatVersion, Impairments,
    ImpairmentsChange, Jitter, JitterDistribution, Link, Loss, Ready, Reorder, Scenario, Shutdown,
    Signal, Topology, True, UdpCbr, Workload, WorkloadKind,
};
pub use schema::json_schema;
pub use units::{Duration, Name, Probability, Rate, Size, ValueError};
pub use validate::{MAX_PACKET_SIZE, MAX_PACKETS_PER_SECOND, MIN_PACKET_SIZE};
