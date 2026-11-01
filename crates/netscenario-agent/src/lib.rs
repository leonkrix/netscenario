//! The agent that runs inside a node: it sends and receives the packets of a workload and writes one record
//! per packet (WRK-01, PRB-01; D-008).
//!
//! The agent measures, not the program under test (see `docs/AGENT.md` for the formats). Two programs in two
//! nodes cooperate: [`sender`] puts a sequence number and the send time into every packet of a constant bit
//! rate stream, [`receiver`] records the arrival time of each packet in a file. Both read the same clock,
//! `CLOCK_MONOTONIC` ([`clock`]), which all nodes of a machine share, so the one-way delay is exact and not
//! estimated.
//!
//! The pieces are libraries first so that they can be tested without starting a process: [`wire`] is the
//! packet, [`records`] the file, [`schedule`] the moments at which packets leave, and [`cli`] the command line
//! of the program `netscenario-agent`.

pub mod cli;
pub mod clock;
pub mod crc;
pub mod events;
pub mod receiver;
pub mod records;
pub mod schedule;
pub mod sender;
pub mod wire;
