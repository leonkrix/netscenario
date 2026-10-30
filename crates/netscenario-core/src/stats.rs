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

/// The width of a bin of the throughput series: one second.
pub const THROUGHPUT_BIN_NS: u64 = 1_000_000_000;

/// The one-way delay of the packets that arrived, in nanoseconds.
///
/// A percentile is the **nearest rank**: for `n` delays in ascending order, the percentile `p` is the delay at
/// rank `ceil(p * n / 100)` (the first is rank 1). It is always a delay that was measured, never an
/// interpolation between two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The payload that arrived in each second.
    pub throughput: Throughput,
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
    let mut seen = HashSet::with_capacity(records.len());
    let mut delays = Vec::with_capacity(records.len());
    let mut bins: Vec<u64> = Vec::new();
    let (mut duplicates, mut unexpected, mut reordered) = (0_u64, 0_u64, 0_u64);
    let mut highest: Option<u64> = None;
    for record in records {
        if record.sequence >= sent {
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
    use super::*;

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
