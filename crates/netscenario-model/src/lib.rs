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
mod generators;
mod graph;
mod hash;
mod ip;
mod measure;
mod mtu;
mod queue;
mod read;
mod required;
mod reserved;
mod scenario;
mod schema;
mod shortcut;
mod source;
mod suggest;
mod table;
mod topology;
mod trace;
mod trace_pcap;
pub mod units;
mod validate;

pub use builder::ScenarioBuilder;
pub use command::{MAX_ARGUMENT_BYTES, MAX_ARGUMENTS, MAX_ENV_ENTRIES, RESERVED_ENV_PREFIX};
pub use error::{Error, Location, Problem};
pub use generators::{
    Generator, Pattern, PhaseDistribution, TraceReplay, UdpBursty, UdpOnOff, UdpPoisson,
};
pub use graph::{Destination, Path, PathError, RouteEntry, Routes};
pub use hash::sha256_hex;
pub use ip::{IpFamily, IpMode};
pub use measure::{
    MAX_BINS, MIN_BIN, Measure, MeasureOverrides, RecoveryBasis, RecoverySettings, SeriesSettings,
};
pub use mtu::{
    DEFAULT_MTU, MAX_MTU, MAX_PACKET_SIZE_V6, MAX_UDP_PAYLOAD_V4, MAX_UDP_PAYLOAD_V6, MIN_MTU,
    MIN_MTU_V6, frames_per_packet, mtus_of, mtus_of_topology, path_problem,
};
pub use queue::{Discipline, Queue, QueueLimit};
pub use read::MAX_FILE_BYTES;
pub use scenario::{
    Backend, Change, Command, Direction, Edit, Event, FormatVersion, Impairments,
    ImpairmentsChange, Jitter, JitterDistribution, Link, Loss, OnExit, Ready, Reorder, Scenario,
    Shutdown, Signal, True, UdpCbr, UdpChecksum, Workload, WorkloadKind,
};
pub use schema::json_schema;
pub use shortcut::{
    BOTTLENECK, Chain, Dumbbell, LinkSpec, Multipath, PathSpec, ROUTER_LEFT, ROUTER_RIGHT, Star,
};
pub use table::{
    DelayTable, MAX_REFERENCED_FILE_BYTES, MAX_SAMPLES, MIN_SAMPLES, TABLE_ENTRIES, TABLE_SCALE,
    TableValues, parse_samples,
};
pub use topology::{
    DEFAULT_DESTINATION, DEFAULT_METRIC, MAX_LINKS, MAX_NODES, MAX_ROUTES, Node, Role, Route,
    Routing, Topology,
};
pub use trace::{
    FlowProtocol, FlowSelection, MAX_TRACE_PACKETS, PacketTrace, ReadTrace, TraceData, TraceFormat,
    read_csv,
};
pub use trace_pcap::read_capture;
pub use units::{Duration, Name, Probability, Rate, Size, ValueError};
pub use validate::{
    MAX_BURST_PACKETS, MAX_COST, MAX_PACKET_SIZE, MAX_PACKETS_PER_SECOND, MAX_QUEUE_BYTES,
    MAX_QUEUE_PACKETS, MAX_WEIGHT, MIN_PACKET_SIZE, MIN_PACKET_SIZE_WITHOUT_CHECKSUM,
};
