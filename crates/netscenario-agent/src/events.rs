//! What the agent says on its standard output: one JSON object per line, each with an `event`.
//!
//! The program that starts the agent (the supervisor of a run) reads these lines. The types here are used by
//! both sides, so they cannot drift apart.

use serde::{Deserialize, Serialize};

/// A line of the standard output of the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The receiver is bound and waits for packets. The sender starts after this.
    Ready {
        /// The workload the receiver serves.
        flow: u32,
        /// The address it is bound to, with the port that the system chose if the port was 0.
        address: String,
        /// Why the tap that was asked for (`--wire-out`) could not be opened, if it could not. The receiver
        /// runs without it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire_error: Option<String>,
    },
    /// The sender is done.
    Sent(SendSummary),
    /// The receiver is done.
    Received(ReceiveSummary),
    /// The sender asks the run to do something to the link before it sends packet `packet` (D-038): it has sent
    /// the packet before, and it waits at the slot of `packet` for the line `go <id>` on its standard input. It is the
    /// last variant: the order of the others is the order of the lines of a run.
    Barrier {
        /// The workload.
        flow: u32,
        /// The number that the run gave the barrier; it answers with `go <id>`.
        id: u32,
        /// The first packet that meets what the run does.
        packet: u64,
    },
}

/// What the sender did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendSummary {
    /// The workload.
    pub flow: u32,
    /// How many packets were handed to the kernel. A packet that the kernel refused (`errors`) uses its sequence
    /// number all the same, so the sequence numbers are `0` up to `packets + errors` minus one, without the ones in
    /// `refused`.
    pub packets: u64,
    /// When the first packet was sent (`CLOCK_MONOTONIC`, nanoseconds), if one was.
    pub first_send_ns: Option<u64>,
    /// When the last packet was sent, if one was.
    pub last_send_ns: Option<u64>,
    /// The largest amount by which a packet left later than the schedule says, in nanoseconds. It tells how
    /// much of a delay or a gap in the result comes from the machine and not from the network.
    pub max_lateness_ns: u64,
    /// How many packets the kernel refused to take (their sequence numbers are used up all the same).
    pub errors: u64,
    /// The first such error, as text.
    pub first_error: Option<String>,
    /// The sequence numbers of the packets that the kernel refused, as runs: `[first, last]`, both included, in
    /// ascending order. A link that is down refuses a run of packets, and the run needs to know which ones they were
    /// to tell a packet that was never sent from one that was lost.
    #[serde(default)]
    pub refused: Vec<[u64; 2]>,
    /// How many refused packets are not in `refused` because there were more than `MAX_REFUSED_RUNS` separate runs of
    /// them. The run does not go on with statistics that cannot say which packets were sent.
    #[serde(default)]
    pub refused_unrecorded: u64,
    /// The barriers that were asked and answered (D-038), in the order they were asked.
    #[serde(default)]
    pub barriers: Vec<BarrierDone>,
    /// How many barriers were not answered in time: the sender went on without the answer, so the guarantee of
    /// D-038 does not hold for them. The run fails on it.
    #[serde(default)]
    pub barrier_timeouts: u64,
}

/// A barrier as the sender lived it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarrierDone {
    /// The number that the run gave it.
    pub id: u32,
    /// The first packet that met what the run did: the number of packets that met the old state.
    pub packet: u64,
    /// When it was asked (`CLOCK_MONOTONIC`, nanoseconds), right after the packet before was sent.
    pub asked_ns: u64,
    /// How long the sender waited for the answer at the slot of the packet. 0 if the answer was there already.
    pub held_ns: u64,
}

/// What the receiver saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveSummary {
    /// The workload.
    pub flow: u32,
    /// How many packets were recorded.
    pub packets: u64,
    /// How many datagrams were not packets of the agent (wrong magic, version or flags, or too short). They
    /// are not recorded. A receiver of a stream without the UDP checksum counts none as foreign: a bit that
    /// is flipped in the magic cannot be told from a stranger, so every datagram that is not a good packet
    /// is `corrupted`.
    pub foreign: u64,
    /// How many datagrams arrived damaged, in a stream without the UDP checksum (the CRC-32 does not match, or
    /// the header is not one). They are not recorded. Always 0 with the checksum on, where the kernel drops
    /// a damaged packet.
    #[serde(default)]
    pub corrupted: u64,
    /// How many packets of the stream the tap saw on the interface, delivered or not, if it ran (`--wire-out`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_packets: Option<u64>,
    /// When the first packet arrived, if one did.
    pub first_receive_ns: Option<u64>,
    /// When the last packet arrived, if one did.
    pub last_receive_ns: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_one_line_with_an_event_name() {
        let ready = Event::Ready {
            flow: 2,
            address: "127.0.0.1:4000".to_owned(),
            wire_error: None,
        };
        assert_eq!(
            serde_json::to_string(&ready).unwrap(),
            r#"{"event":"ready","flow":2,"address":"127.0.0.1:4000"}"#
        );
        let sent = Event::Sent(SendSummary {
            flow: 1,
            packets: 3,
            first_send_ns: Some(10),
            last_send_ns: Some(30),
            max_lateness_ns: 5,
            errors: 2,
            first_error: Some("Network is unreachable".to_owned()),
            refused: vec![[1, 2]],
            refused_unrecorded: 0,
            barriers: Vec::new(),
            barrier_timeouts: 0,
        });
        let text = serde_json::to_string(&sent).unwrap();
        assert!(
            text.starts_with(r#"{"event":"sent","flow":1,"packets":3"#),
            "{text}"
        );
        assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), sent);
    }
}
