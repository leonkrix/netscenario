//! What a workload's records say: how many packets arrived, how long they took, and how much came through
//! per second (PRB-02, PRB-03, PRB-05).
//!
//! The functions take plain data and do plain arithmetic, so that they can be tested against numbers that are
//! worked out by hand. Times are nanoseconds; a rate is bits per second of **payload**.

// Counts of packets and bits are far below 2^52, where a double stops being exact
#![allow(clippy::cast_precision_loss)]

use std::collections::HashSet;

use netscenario_agent::records::Record;
use serde::{Deserialize, Serialize};

use crate::patterns::{self, Jitter, LossRuns, Reordering};

/// The width of a bin of the throughput series: one second.
pub const THROUGHPUT_BIN_NS: u64 = 1_000_000_000;

/// The one-way delay of the packets that arrived, in nanoseconds.
///
/// A percentile is the **nearest rank**: for `n` delays in ascending order, the percentile `p` is the delay at
/// rank `ceil(p * n / 100)` (the first is rank 1). It is always a delay that was measured, never an
/// interpolation between two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DelaySummary {
    /// The smallest delay.
    pub min_ns: u64,
    /// The mean, rounded down.
    pub mean_ns: u64,
    /// The median (percentile 50).
    pub p50_ns: u64,
    /// Percentile 90.
    pub p90_ns: u64,
    /// Percentile 95.
    pub p95_ns: u64,
    /// Percentile 99.
    pub p99_ns: u64,
    /// Percentile 99.9.
    pub p999_ns: u64,
    /// The largest delay.
    pub max_ns: u64,
}

/// The payload that came through in each second.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Throughput {
    /// The width of a bin in nanoseconds (always one second in format version 0).
    pub bin_ns: u64,
    /// Payload bits per second received in each bin, counted from the first packet sent. A bin is not scaled:
    /// the last one is usually only partly filled, and the packets still in flight at the end of the stream
    /// fall into a bin of their own.
    pub bits_per_second: Vec<f64>,
}

/// What came of one workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FlowStats {
    /// How many packets the sender handed to the kernel.
    pub sent: u64,
    /// How many different packets arrived (a packet that arrived twice counts once).
    pub received: u64,
    /// `sent - received`: the packets that did not arrive.
    pub lost: u64,
    /// `lost / sent`, 0 if nothing was sent.
    pub loss_ratio: f64,
    /// Arrivals of a packet that had arrived before.
    pub duplicates: u64,
    /// Packets that arrived after a packet with a higher sequence number (RFC 4737, type-P-reordered). A
    /// packet that is late by a lot counts once, however many packets overtook it. Duplicates are not counted.
    pub reordered: u64,
    /// Records whose sequence number is not below `sent`. This cannot happen with a correct sender and receiver.
    pub unexpected: u64,
    /// The delays of the packets that arrived (the first arrival of each), or `None` if none did.
    pub delay: Option<DelaySummary>,
    /// How far the reordered packets were out of place (RFC 4737), or `None` if none was reordered.
    #[serde(default)]
    pub reordering: Option<Reordering>,
    /// The interarrival jitter (RFC 3550), or `None` for fewer than two arrivals.
    #[serde(default)]
    pub jitter: Option<Jitter>,
    /// The runs of lost packets.
    #[serde(default)]
    pub loss_runs: LossRuns,
    /// The payload that arrived in each second.
    pub throughput: Throughput,
}

/// The packets that were seen on the wire of the receiving node and never delivered intact: the sequence numbers in
/// `seen` (what the tap read from the frames) that are below `sent` and in none of the `delivered` records, once
/// each, in ascending order (D-040).
///
/// These are the packets that arrived damaged: the node dropped them (the UDP checksum caught the bit), or a
/// stream without the checksum found them by their CRC-32. It names what the frame shows. A bit in the
/// sequence number of a frame makes another number, which is named if it is in range and was not delivered: with
/// a link that only corrupts that is another packet that was damaged too, and with a loss it can be a packet
/// that the queue dropped. The run says so where a loss and a corruption are together.
#[must_use]
pub fn name_damaged(sent: u64, delivered: &[Record], seen: &[Record]) -> Vec<u64> {
    name_damaged_refused(sent, &[], delivered, seen)
}

/// As [`name_damaged`], for a sender that used `slots` sequence numbers of which the kernel refused those in `refused`
/// (runs `[first, last]`, ascending and apart): a number that was never sent is not named, whatever a flipped bit made
/// of a frame.
#[must_use]
pub fn name_damaged_refused(
    slots: u64,
    refused: &[[u64; 2]],
    delivered: &[Record],
    seen: &[Record],
) -> Vec<u64> {
    let delivered: HashSet<u64> = delivered.iter().map(|record| record.sequence).collect();
    let was_refused = |sequence: u64| {
        let place = refused.partition_point(|[_, last]| *last < sequence);
        refused
            .get(place)
            .is_some_and(|[first, _]| *first <= sequence)
    };
    let mut named: Vec<u64> = seen
        .iter()
        .map(|record| record.sequence)
        .filter(|sequence| {
            *sequence < slots && !was_refused(*sequence) && !delivered.contains(sequence)
        })
        .collect();
    named.sort_unstable();
    named.dedup();
    named
}

/// The delay at the nearest rank for `permille` (500 is the median, 999 is percentile 99.9) in `sorted`.
fn nearest_rank(sorted: &[u64], permille: u64) -> u64 {
    let n = sorted.len() as u64;
    let rank = (permille * n).div_ceil(1000).max(1);
    sorted[usize::try_from(rank - 1).unwrap_or(0).min(sorted.len() - 1)]
}

/// Computes the statistics of one workload.
///
/// `sent` is what the sender said it sent. `records` are the arrivals in the order they happened.
/// `packet_size` is the payload of a packet in bytes. `start_ns` is the moment the first packet was sent, on the
/// clock of the records, from which the throughput bins are counted.
#[must_use]
pub fn analyse(sent: u64, records: &[Record], packet_size: u64, start_ns: u64) -> FlowStats {
    analyse_refused(sent, &[], records, packet_size, start_ns)
}

/// As [`analyse`], for a sender that used `slots` sequence numbers of which the kernel refused those in `refused`
/// (runs `[first, last]`, ascending and apart). A refused packet was never sent: it is not in `sent`, not lost, and
/// an arrival with its number is `unexpected`. `sent` of the result is `slots` less the refused ones.
#[must_use]
pub fn analyse_refused(
    slots: u64,
    refused: &[[u64; 2]],
    records: &[Record],
    packet_size: u64,
    start_ns: u64,
) -> FlowStats {
    let sent = patterns::sent_between(refused, 0, slots);
    let was_refused = |sequence: u64| {
        let place = refused.partition_point(|[_, last]| *last < sequence);
        refused
            .get(place)
            .is_some_and(|[first, _]| *first <= sequence)
    };
    let mut seen = HashSet::with_capacity(records.len());
    let mut delays = Vec::with_capacity(records.len());
    let mut bins: Vec<u64> = Vec::new();
    let (mut duplicates, mut unexpected, mut reordered) = (0_u64, 0_u64, 0_u64);
    let mut highest: Option<u64> = None;
    for record in records {
        if record.sequence >= slots || was_refused(record.sequence) {
            unexpected += 1;
            continue;
        }
        if !seen.insert(record.sequence) {
            duplicates += 1;
            continue;
        }
        if highest.is_some_and(|top| record.sequence < top) {
            reordered += 1;
        }
        highest = Some(highest.map_or(record.sequence, |top| top.max(record.sequence)));
        delays.push(record.delay_ns());
        let bin = usize::try_from(record.receive_ns.saturating_sub(start_ns) / THROUGHPUT_BIN_NS)
            .unwrap_or(usize::MAX);
        if bins.len() <= bin {
            bins.resize(bin + 1, 0);
        }
        bins[bin] += 1;
    }
    let received = delays.len() as u64;
    let lost = sent - received;
    let arrivals = patterns::first_arrivals(slots, records);
    let arrivals: Vec<Record> = arrivals
        .into_iter()
        .filter(|record| !was_refused(record.sequence))
        .collect();
    delays.sort_unstable();
    let delay = (!delays.is_empty()).then(|| DelaySummary {
        min_ns: delays[0],
        mean_ns: u64::try_from(
            delays.iter().map(|&d| u128::from(d)).sum::<u128>() / delays.len() as u128,
        )
        .unwrap_or(u64::MAX),
        p50_ns: nearest_rank(&delays, 500),
        p90_ns: nearest_rank(&delays, 900),
        p95_ns: nearest_rank(&delays, 950),
        p99_ns: nearest_rank(&delays, 990),
        p999_ns: nearest_rank(&delays, 999),
        max_ns: delays[delays.len() - 1],
    });
    let seconds = THROUGHPUT_BIN_NS as f64 / 1e9;
    FlowStats {
        sent,
        received,
        lost,
        loss_ratio: if sent == 0 {
            0.0
        } else {
            lost as f64 / sent as f64
        },
        duplicates,
        reordered,
        unexpected,
        delay,
        reordering: patterns::reordering(&arrivals),
        jitter: patterns::interarrival_jitter(&arrivals),
        loss_runs: patterns::loss_runs_refused(slots, refused, &arrivals),
        throughput: Throughput {
            bin_ns: THROUGHPUT_BIN_NS,
            bits_per_second: bins
                .iter()
                .map(|&count| (count * packet_size * 8) as f64 / seconds)
                .collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    /// The statistics of a workload carry the patterns: one series with a reordered packet, a lost run and a
    /// duplicate, worked out by hand.
    #[test]
    fn the_statistics_of_a_workload_carry_the_extent_the_jitter_and_the_runs() {
        // Sent 0..=7. Arrivals: 0, 2, 1 (late by one), 1 again (a copy), 5, 6. Lost: 3-4 (a run of 2) and 7 (1).
        let records = [
            Record {
                sequence: 0,
                send_ns: 0,
                receive_ns: 10_000_000,
            },
            Record {
                sequence: 2,
                send_ns: 2_000_000,
                receive_ns: 12_000_000,
            },
            Record {
                sequence: 1,
                send_ns: 1_000_000,
                receive_ns: 13_000_000,
            },
            Record {
                sequence: 1,
                send_ns: 1_000_000,
                receive_ns: 14_000_000,
            },
            Record {
                sequence: 5,
                send_ns: 5_000_000,
                receive_ns: 15_000_000,
            },
            Record {
                sequence: 6,
                send_ns: 6_000_000,
                receive_ns: 16_000_000,
            },
        ];
        let stats = analyse(8, &records, 100, 0);
        assert_eq!(
            (
                stats.received,
                stats.lost,
                stats.duplicates,
                stats.reordered
            ),
            (5, 3, 1, 1)
        );
        let reordering = stats.reordering.unwrap();
        assert_eq!(
            (reordering.extent.max, reordering.late_time.max_ns),
            (1, 1_000_000)
        );
        // Transit times 10, 10, 12, 10, 10: D = 0, 2, -2, 0 ms in the order of arrival
        let jitter = stats.jitter.unwrap();
        assert_eq!(jitter.max_ns, jitter.max_ns.max(jitter.last_ns));
        assert!(jitter.max_ns > 0 && jitter.last_ns > 0, "{jitter:?}");
        assert_eq!((stats.loss_runs.runs, stats.loss_runs.longest), (2, 2));
        assert!((stats.loss_runs.mean - 1.5).abs() < 1e-12);
        // Canary: nothing out of order, nothing lost, a steady stream has none of the three
        let steady: Vec<Record> = (0..10_u64)
            .map(|n| Record {
                sequence: n,
                send_ns: n * 1_000_000,
                receive_ns: n * 1_000_000 + 7_000_000,
            })
            .collect();
        let calm = analyse(10, &steady, 100, 0);
        assert_eq!((calm.reordering, calm.loss_runs.runs), (None, 0));
        assert_eq!(calm.jitter.unwrap().max_ns, 0);
    }

    fn at(sequence: u64) -> Record {
        Record {
            sequence,
            send_ns: 0,
            receive_ns: 1,
        }
    }

    /// The names are the sequences that the wire showed and the socket did not get, in range, once each.
    #[test]
    fn a_packet_seen_on_the_wire_and_not_delivered_is_named() {
        let delivered = [at(0), at(1), at(3)];
        // 2 and 4 were seen and not delivered (4 twice, a duplicated frame); 3 was both; 99 is not a packet
        // of this stream (a bit in the sequence number); 7 was never seen
        let seen = [at(0), at(1), at(2), at(3), at(4), at(4), at(99)];
        assert_eq!(name_damaged(5, &delivered, &seen), [2, 4]);
        // Canary: nothing is named where everything that was seen was delivered, or nothing was seen
        assert_eq!(
            name_damaged(5, &delivered, &[at(0), at(3)]),
            Vec::<u64>::new()
        );
        assert_eq!(name_damaged(5, &delivered, &[]), Vec::<u64>::new());
        // and a packet that was delivered late (after it was seen) is not named, whatever the order
        assert_eq!(name_damaged(5, &[at(2)], &[at(2)]), Vec::<u64>::new());
    }

    use super::*;

    /// The sender used 10 sequence numbers and the kernel refused 4 to 6 (a link that was down): 7 were sent. Two of
    /// them are lost (1 and 8), the others arrived. Refused packets are not lost, are not in `sent`, and do not make a
    /// run of lost packets longer: 1 is a run of one, 8 is a run of one.
    #[test]
    fn refused_packets_were_never_sent_so_they_are_not_lost() {
        let records: Vec<Record> = [0, 2, 3, 7, 9]
            .into_iter()
            .map(|sequence| arrival(sequence, sequence * 10, sequence * 10 + 5))
            .collect();
        let stats = analyse_refused(10, &[[4, 6]], &records, 100, 0);
        assert_eq!((stats.sent, stats.received, stats.lost), (7, 5, 2));
        assert!((stats.loss_ratio - 2.0 / 7.0).abs() < 1e-12);
        assert_eq!(stats.unexpected, 0);
        assert_eq!((stats.loss_runs.runs, stats.loss_runs.longest), (2, 1));
    }

    /// Canary: the same arrivals read as if the sender had used 7 numbers (what the statistics did before they knew
    /// about refused packets) count the arrivals 7 and 9 as unexpected and invent 2 lost packets more.
    #[test]
    fn canary_ignoring_the_refused_packets_gives_the_wrong_numbers() {
        let records: Vec<Record> = [0, 2, 3, 7, 9]
            .into_iter()
            .map(|sequence| arrival(sequence, sequence * 10, sequence * 10 + 5))
            .collect();
        let wrong = analyse(7, &records, 100, 0);
        assert_eq!((wrong.unexpected, wrong.lost), (2, 4));
        let right = analyse_refused(10, &[[4, 6]], &records, 100, 0);
        assert_ne!(
            (wrong.unexpected, wrong.lost),
            (right.unexpected, right.lost)
        );
    }

    /// A packet that arrives with the number of a refused one cannot come from the sender, and is counted as such. A
    /// lost run that has a refused run inside it is one run of the packets that were sent.
    #[test]
    fn an_arrival_of_a_refused_packet_is_unexpected_and_a_lost_run_goes_through_a_refusal() {
        let stats = analyse_refused(
            8,
            &[[3, 4]],
            &[arrival(4, 0, 5), arrival(0, 0, 5), arrival(7, 0, 5)],
            100,
            0,
        );
        assert_eq!(stats.unexpected, 1);
        // Sent: 0 1 2 5 6 7; arrived 0 and 7; lost 1 2 5 6 are one run of four
        assert_eq!((stats.sent, stats.received, stats.lost), (6, 2, 4));
        assert_eq!((stats.loss_runs.runs, stats.loss_runs.longest), (1, 4));
    }

    #[test]
    fn a_refused_packet_is_not_named_as_damaged() {
        let at = |sequence| arrival(sequence, 0, 5);
        assert_eq!(
            name_damaged_refused(8, &[[3, 4]], &[at(0)], &[at(0), at(2), at(3), at(6), at(9)]),
            [2, 6]
        );
    }

    fn arrival(sequence: u64, send_ns: u64, delay_ns: u64) -> Record {
        Record {
            sequence,
            send_ns,
            receive_ns: send_ns + delay_ns,
        }
    }

    /// 100 packets with the delays 1 to 100 ms, one every 10 ms.
    fn hundred() -> Vec<Record> {
        (0..100)
            .map(|n| arrival(n, 1_000 + n * 10_000_000, (n + 1) * 1_000_000))
            .collect()
    }

    #[test]
    fn nothing_lost_gives_a_ratio_of_zero_and_the_delays_of_all() {
        let stats = analyse(100, &hundred(), 1000, 1_000);
        assert_eq!((stats.sent, stats.received, stats.lost), (100, 100, 0));
        assert!(stats.loss_ratio.abs() < f64::EPSILON);
        assert_eq!((stats.duplicates, stats.unexpected), (0, 0));
        let delay = stats.delay.unwrap();
        // The delays are 1 ms to 100 ms: the nearest rank of p percent of 100 delays is the delay p
        assert_eq!(delay.min_ns, 1_000_000);
        assert_eq!(delay.p50_ns, 50_000_000);
        assert_eq!(delay.p90_ns, 90_000_000);
        assert_eq!(delay.p95_ns, 95_000_000);
        assert_eq!(delay.p99_ns, 99_000_000);
        assert_eq!(delay.p999_ns, 100_000_000, "ceil(99.9) is the last rank");
        assert_eq!(delay.max_ns, 100_000_000);
        // (1 + 2 + ... + 100) / 100 = 50.5 ms
        assert_eq!(delay.mean_ns, 50_500_000);
    }

    /// Canary: the statistics can say that something is wrong. Packets that never arrive are lost, and the
    /// ratio is exact.
    #[test]
    fn missing_sequence_numbers_are_the_loss() {
        let records: Vec<Record> = hundred()
            .into_iter()
            .filter(|r| r.sequence % 4 != 0)
            .collect();
        let stats = analyse(100, &records, 1000, 1_000);
        assert_eq!((stats.received, stats.lost), (75, 25));
        assert!((stats.loss_ratio - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn everything_lost_has_a_ratio_of_one_and_no_delays() {
        let stats = analyse(10, &[], 1000, 0);
        assert_eq!((stats.received, stats.lost), (0, 10));
        assert!((stats.loss_ratio - 1.0).abs() < f64::EPSILON);
        assert_eq!(stats.delay, None);
        assert_eq!(stats.throughput.bits_per_second, Vec::<f64>::new());
    }

    #[test]
    fn a_stream_without_packets_has_no_loss() {
        let stats = analyse(0, &[], 1000, 0);
        assert_eq!((stats.sent, stats.lost), (0, 0));
        assert!(stats.loss_ratio.abs() < f64::EPSILON);
    }

    #[test]
    fn a_packet_that_arrives_twice_counts_once_with_its_first_delay() {
        let records = vec![
            arrival(0, 0, 5_000_000),
            arrival(1, 10, 6_000_000),
            arrival(0, 0, 9_000_000),
        ];
        let stats = analyse(2, &records, 1000, 0);
        assert_eq!((stats.received, stats.lost, stats.duplicates), (2, 0, 1));
        let delay = stats.delay.unwrap();
        assert_eq!((delay.min_ns, delay.max_ns), (5_000_000, 6_000_000));
    }

    /// Packets 0 1 3 2 5 4: 2 arrived after 3, and 4 after 5. A packet that is overtaken by several still counts
    /// once, and an in-order stream with losses has none.
    #[test]
    fn a_packet_that_arrives_after_a_higher_one_is_reordered() {
        let order = |sequences: &[u64]| {
            let records: Vec<Record> = sequences.iter().map(|&s| arrival(s, 0, 1)).collect();
            analyse(10, &records, 1000, 0).reordered
        };
        assert_eq!(order(&[0, 1, 3, 2, 5, 4]), 2);
        assert_eq!(
            order(&[0, 5, 1, 2, 3, 4]),
            4,
            "each of the four was overtaken by packet 5"
        );
        assert_eq!(order(&[0, 2, 4, 6, 8]), 0, "losses are not reordering");
        assert_eq!(order(&[]), 0);
        // A second arrival of a packet is a duplicate, not a reordering
        assert_eq!(order(&[0, 1, 2, 1]), 0);
    }

    #[test]
    fn a_sequence_number_the_sender_never_used_is_reported_not_counted() {
        let records = vec![arrival(0, 0, 1), arrival(7, 0, 1)];
        let stats = analyse(2, &records, 1000, 0);
        assert_eq!((stats.received, stats.lost, stats.unexpected), (1, 1, 1));
    }

    #[test]
    fn a_single_delay_is_every_percentile() {
        let stats = analyse(1, &[arrival(0, 0, 42)], 1000, 0);
        let delay = stats.delay.unwrap();
        for value in [
            delay.min_ns,
            delay.mean_ns,
            delay.p50_ns,
            delay.p99_ns,
            delay.p999_ns,
            delay.max_ns,
        ] {
            assert_eq!(value, 42);
        }
    }

    #[test]
    fn the_nearest_rank_is_a_measured_delay() {
        // Four delays: the median is the second one (rank 2), not the mean of the middle two
        let sorted = [10, 20, 30, 40];
        assert_eq!(nearest_rank(&sorted, 500), 20);
        assert_eq!(nearest_rank(&sorted, 900), 40);
        assert_eq!(nearest_rank(&sorted, 10), 10);
    }

    #[test]
    fn the_throughput_counts_payload_bits_in_the_second_of_arrival() {
        // 1000 byte payloads. Three arrive in the first second after the start, none in the second, one in the third.
        let start = 5_000_000_000;
        let records = vec![
            Record {
                sequence: 0,
                send_ns: start,
                receive_ns: start + 100,
            },
            Record {
                sequence: 1,
                send_ns: start,
                receive_ns: start + 500_000_000,
            },
            Record {
                sequence: 2,
                send_ns: start,
                receive_ns: start + 999_999_999,
            },
            Record {
                sequence: 3,
                send_ns: start,
                receive_ns: start + 2_000_000_000,
            },
        ];
        let stats = analyse(4, &records, 1000, start);
        assert_eq!(stats.throughput.bits_per_second, [24_000.0, 0.0, 8_000.0]);
        assert_eq!(stats.throughput.bin_ns, 1_000_000_000);
    }
}
