//! The run lifecycle: what a backend must provide, the check of a scenario against what it reproduces
//! faithfully, and the building of the network (see `docs/ARCHITECTURE.md`, "The life of a run").
//!
//! A backend implements [`Backend`] and declares its [`Capabilities`]. [`provision`] checks a scenario
//! against them, so that a scenario the backend cannot reproduce exactly is refused before anything is
//! created (BKD-03), and then builds the nodes, links and impairments through the trait.
//!
//! [`run`] then starts the agents in the nodes, lets the workloads run and returns a [`RunResult`] with the
//! statistics of [`stats`]. The timeline (`set`, `outage`, `down`, `up`) is done by [`run`] since release 0.2 (see
//! [`timeline`]); the link models arrive with the later releases.
//!
//! The check needs no machine: a backend is a set of declarations, and a scenario that asks for more is
//! refused with the path in the file and a way out.
//!
//! ```
//! use netscenario_core::{Capabilities, Feature, Support, check};
//! use netscenario_model::Scenario;
//!
//! let yaml = "
//! version: 0
//! name: seeded
//! seed: 1234
//! duration: 10s
//! topology:
//!   nodes: [client, server]
//!   links:
//!     - id: access
//!       between: [client, server]
//!       both: { loss: { model: random, p: 1% } }
//! workloads:
//!   - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
//! ";
//! let scenario = Scenario::from_yaml(yaml, "seeded.yaml")?;
//!
//! // A backend that does not reproduce a seeded loss pattern
//! let backend = Capabilities::new("example")
//!     .with(Feature::PointToPoint, Support::Exact)
//!     .with(Feature::Ipv4, Support::Exact)
//!     .with(Feature::UdpGenerators, Support::Exact)
//!     .with(Feature::IndependentLoss, Support::Exact)
//!     .with(
//!         Feature::SeededLoss,
//!         Support::Unsupported {
//!             reason: "this backend cannot repeat a loss pattern".to_owned(),
//!             fix: "leave out `seed`".to_owned(),
//!         },
//!     );
//!
//! let error = check(&scenario, &backend, false).unwrap_err();
//! assert!(error.to_string().contains("path: seed"));
//! # Ok::<(), netscenario_model::Error>(())
//! ```

mod backend;
mod barriers;
pub mod bootstrap;
mod capability;
mod captures;
mod drive;
mod error;
mod export;
mod feature;
mod hash;
mod machine;
pub mod metrics;
mod observe;
pub mod patterns;
mod programs;
mod provision;
pub mod recovery;
mod replay;
mod result;
mod run;
mod runset;
mod seed;
pub mod series;
mod special;
pub mod stats;
pub mod timeline;
mod traffic;

pub use backend::{
    Backend, BackendInfo, Direction, LINE_LIMIT, LOG_LIMIT, LinkHandle, NodeCounts, NodeHandle,
    OutputStats, PortKind, Program, ProgramExit, ProgramHandle, ProgramSignal, ProgramState,
};
pub use capability::{Accepted, Capabilities, CapabilityError, Refusal, Support, check};
pub use error::Error;
pub use export::{write_packets, write_packets_marked, write_packets_refused};
pub use feature::{Feature, Requirement, requirements};
pub use hash::sha256_hex;
pub use machine::{Provenance, collect as collect_provenance, rfc3339_utc};
pub use observe::{CancelToken, RunEvent, RunObserver};
pub use patterns::{ExtentSummary, Jitter, LateTimeSummary, LossRuns, Reordering};
pub use provision::{NodeAddress, Provisioned, impairments_of, provision};
pub use recovery::{Basis, Recovery, RecoveryOptions};
pub use replay::{
    Comparison, Difference, LISTED_SEQUENCES, Recorded, ReplayError, SCENARIO_FILE, Verdict,
    WorkloadComparison, compare, scenario_diff, scenario_hash, scenario_json,
};
pub use result::{
    AcceptedRestriction, AddressInfo, CaptureResult, DirectionTally, EventRecord, MetricSummary,
    ProgramOutput, ProgramResult, QueueCheck, RESULT_FORMAT, RESULT_VERSION, RunResult,
    ScenarioInfo, SeedInfo, SenderInfo, SeriesInfo, Timing, WorkloadResult,
};
pub use run::{RunOptions, agent_next_to, run};
pub use runset::{
    Confidence, MAX_REPETITIONS, MetricSet, PartialMetric, RUNSET_FILE, RUNSET_FORMAT,
    RUNSET_VERSION, Repetition, RepetitionInfo, RunSetOptions, RunSetResult, SetSeed, aggregate,
    metric_values, run_set,
};
pub use seed::{
    derive_bootstrap_seed, derive_repetition_seed, derive_seed, derive_stage_seed, draw_seed,
};
