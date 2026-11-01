//! Which step of the timeline a sender waits for, and what is left to the clock (D-038).
//!
//! A step is bound to the first workload that sends into its direction (a `down` or an `up`, to the first that sends
//! over the link), and to the first packet of that workload at or after the moment of the step: the sender knows its
//! packet clock, so the packet number is plain arithmetic on integers, the same that the sender uses for the moments
//! at which its packets leave. A step that no sender can wait for is left to the clock: no workload sends into its
//! direction, or its moment lies after the last packet, or it is an end of an outage, which is a moment in the time
//! of the receiver (D-064). And the run says, for such a step and for a direction that a second workload also sends
//! into, that the lost packets of a seed are not promised to repeat, if the step changes what the queue draws random
//! numbers for.

use netscenario_agent::schedule::Stream;
use netscenario_model::{Name, Scenario};

use crate::backend::Direction;
use crate::drive::Binding;
use crate::timeline::{Action, Step, steps_that_change_what_is_drawn};

/// What the plan needs to know of a workload.
pub(crate) struct Sending<'a> {
    pub(crate) from: &'a Name,
    pub(crate) to: &'a Name,
    pub(crate) stream: Stream,
    /// When it starts and how long it sends, in nanoseconds from the start of the measured phase.
    pub(crate) start_ns: u64,
    pub(crate) duration_ns: u64,
}

/// The barriers of a run.
pub(crate) struct Plan {
    /// For each step, the sender that waits for it, or `None` for a step done by the clock.
    pub(crate) bindings: Vec<Option<Binding>>,
    /// Why the lost packets of the seed may differ from run to run, one sentence for each reason.
    pub(crate) reasons: Vec<String>,
}

/// Plans the barriers of `steps` for the workloads `sending`. With `enabled` false every step is left to the clock.
pub(crate) fn plan(
    scenario: &Scenario,
    sending: &[Sending<'_>],
    steps: &[Step],
    enabled: bool,
) -> Plan {
    let drawn = steps_that_change_what_is_drawn(scenario);
    let mut reasons: Vec<String> = Vec::new();
    let mut bindings = Vec::with_capacity(steps.len());
    for (place, step) in steps.iter().enumerate() {
        let link = &scenario.topology.links[step.link];
        let (ends, word) = match &step.action {
            // The window of an outage is in the time of the receiver, and no packet of the sender is to wait for it
            Action::Blackout { .. } => {
                bindings.push(None);
                continue;
            }
            Action::Impair {
                direction: Direction::Forward,
                ..
            } => (vec![(&link.between[0], &link.between[1])], "forward"),
            Action::Impair {
                direction: Direction::Reverse,
                ..
            } => (vec![(&link.between[1], &link.between[0])], "reverse"),
            // A link that goes down or comes up is met by a workload in either direction
            Action::Down | Action::Up => (
                vec![
                    (&link.between[0], &link.between[1]),
                    (&link.between[1], &link.between[0]),
                ],
                "both ways",
            ),
        };
        let candidates: Vec<usize> = sending
            .iter()
            .enumerate()
            .filter(|(_, other)| ends.contains(&(other.from, other.to)))
            .map(|(index, _)| index)
            .collect();
        let matters = drawn.contains(&place);
        let mut note = |reason: String| {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        };
        let Some(&sender) = candidates.first() else {
            if matters {
                note(format!(
                    "the timeline changes what link '{}' ({word}) draws random numbers for, and no workload of the scenario sends into that direction, so the step is done by the clock and lands before a different packet in every run",
                    link.id
                ));
            }
            bindings.push(None);
            continue;
        };
        if candidates.len() > 1 && matters {
            note(format!(
                "two workloads send into link '{}' ({word}) and the timeline changes what it draws random numbers for: the order in which their packets reach the queue is not fixed on a real kernel, so the lost packets of one seed can differ",
                link.id
            ));
        }
        let first = &sending[sender];
        let packet = packet_at(first, step.at.as_nanos());
        if packet >= first.stream.packets_in(first.duration_ns) {
            // After the last packet: nothing of the workload meets the step, so the clock does it as well as a barrier
            bindings.push(None);
        } else if enabled {
            bindings.push(Some(Binding { sender, packet }));
        } else {
            if matters {
                note(format!(
                    "the barriers are switched off, so the step on link '{}' ({word}) that changes what is drawn is done by the clock and lands before a different packet in every run",
                    link.id
                ));
            }
            bindings.push(None);
        }
    }
    Plan { bindings, reasons }
}

/// The number of the first packet of `sending` at or after the moment `at` (nanoseconds from the start of the measured
/// phase): the packets that leave before it are those whose moment, counted from the start of the workload, is less.
fn packet_at(sending: &Sending<'_>, at: u64) -> u64 {
    let since_start = at.saturating_sub(sending.start_ns);
    if since_start == 0 {
        0
    } else {
        sending.stream.packets_in(since_start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(bits_per_second: u64, packet_size: u64) -> Stream {
        Stream {
            bits_per_second,
            packet_size,
        }
    }

    /// 800 kbit with 1000 byte packets is 100 packets a second, a packet every 10 ms. The first packet at or after
    /// 1,000 ms is packet 100; at 1,001 ms it is packet 101; at exactly a moment of a packet it is that packet.
    #[test]
    fn the_first_packet_at_or_after_a_moment_is_plain_arithmetic() {
        let name = Name::new("a").unwrap();
        let sending = Sending {
            from: &name,
            to: &name,
            stream: stream(800_000, 1000),
            start_ns: 0,
            duration_ns: 5_000_000_000,
        };
        assert_eq!(packet_at(&sending, 1_000_000_000), 100);
        assert_eq!(packet_at(&sending, 1_000_000_001), 101);
        assert_eq!(packet_at(&sending, 999_999_999), 100);
        assert_eq!(packet_at(&sending, 10_000_000), 1);
        assert_eq!(packet_at(&sending, 0), 0);
        // A workload that starts at 2 s: a step before it meets the first packet, one at 2.5 s the 50th
        let late = Sending {
            start_ns: 2_000_000_000,
            ..sending
        };
        assert_eq!(packet_at(&late, 500_000_000), 0);
        assert_eq!(packet_at(&late, 2_000_000_000), 0);
        assert_eq!(packet_at(&late, 2_500_000_000), 50);
    }
}
