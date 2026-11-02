//! The series of a workload by time bin (PRB-05, PRB-07, D-070): what was sent, what arrived and what was lost in each
//! stretch of the run, seen from the sender and seen from the receiver.
//!
//! A packet has two moments, and both are useful. A test that asks "what did the sender get through in the second after
//! the link came back" counts packets by the moment they were **sent**; a test that asks "was there anything at the
//! receiver in that second" counts them by the moment they **arrived**. The two differ by the delay of the link, and the
//! events of the timeline live in both frames: a `set`, a `down` and an `up` are done at a packet of the sender, and an
//! `outage` is a window at the receiver (D-064). So each bin has both views, side by side, with the same bounds:
//!
//! - `by_send`: the packets whose slot (the moment the sender is to send them, `start + n * interval`) lies in the bin:
//!   how many, how many of them arrived, how many did not, the share that arrived, and the delay of those that arrived.
//!   A packet that the kernel refused (D-064) is not sent and is not counted.
//! - `by_arrival`: the packets that arrived in the bin (the first arrival of each): how many, the payload bits, and
//!   their delay.
//!
//! The bins are counted from the start of the measured phase, the moment from which the times of `events` are counted, so
//! a step of the timeline lies in a bin that can be named. They begin there and not at the first packet. A bin is not
//! scaled: the last one is usually part of a bin.

// Counts of packets are far below 2^52, where a double stops being exact
#![allow(clippy::cast_precision_loss)]

use std::io::{self, Write};

use netscenario_agent::records::Record;
use netscenario_agent::schedule::Stream;
use serde::{Deserialize, Serialize};

use crate::stats::nearest_rank;

/// The default width of a bin: one second.
pub const DEFAULT_BIN_NS: u64 = 1_000_000_000;

/// The delay of the packets of one bin that arrived, in nanoseconds (nearest rank, as in
/// [`DelaySummary`](crate::stats::DelaySummary)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BinDelay {
    /// The mean, rounded down.
    pub mean_ns: u64,
    /// The median.
    pub p50_ns: u64,
    /// Percentile 95.
    pub p95_ns: u64,
    /// Percentile 99.
    pub p99_ns: u64,
    /// The largest.
    pub max_ns: u64,
}

/// The packets of a bin seen from the sender: counted in the bin in which they were to be sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BySend {
    /// How many packets were sent in the bin.
    pub sent: u64,
    /// How many of them arrived (in the bin or later).
    pub received: u64,
    /// How many did not: `sent - received`.
    pub lost: u64,
    /// `received / sent`, or `null` if nothing was sent in the bin.
    pub delivered_ratio: Option<f64>,
    /// The delay of those that arrived, or `null` if none did.
    pub delay: Option<BinDelay>,
}

/// The packets of a bin seen from the receiver: counted in the bin in which they arrived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ByArrival {
    /// How many packets arrived in the bin (the first arrival of each).
    pub received: u64,
    /// The payload bits of them.
    pub bits: u64,
    /// The delay of those packets, or `null` if none arrived.
    pub delay: Option<BinDelay>,
}

/// One bin of the series of a workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Bin {
    /// The number of the bin, from 0.
    pub bin: u64,
    /// Where the bin begins, in nanoseconds from the start of the measured phase.
    pub start_ns: u64,
    /// The packets sent in the bin.
    pub by_send: BySend,
    /// The packets that arrived in the bin.
    pub by_arrival: ByArrival,
}

/// What the series is made from.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    /// The rate and the size of the packets of the workload.
    pub stream: Stream,
    /// When the workload begins, in nanoseconds from the start of the measured phase.
    pub start_ns: u64,
    /// The start of the measured phase on the clock of the records (`CLOCK_MONOTONIC`).
    pub origin_ns: u64,
    /// The sequence numbers the sender used: the packets it sent and the ones the kernel refused.
    pub slots: u64,
    /// The sequence numbers the kernel refused, as runs `[first, last]` in ascending order.
    pub refused: &'a [[u64; 2]],
    /// The arrivals, in the order they happened.
    pub records: &'a [Record],
}

/// The bins of a workload, each `bin_ns` wide, from the start of the measured phase to the last packet sent or arrived.
///
/// A record with a number that was not used, one that was refused, and a second arrival of a packet are left out, as in
/// [`analyse_refused`](crate::stats::analyse_refused).
///
/// # Panics
///
/// If `bin_ns` is zero.
#[must_use]
pub fn bins(input: &Input<'_>, bin_ns: u64) -> Vec<Bin> {
    assert!(bin_ns > 0, "a bin has a width");
    let Input {
        stream,
        start_ns,
        origin_ns,
        slots,
        refused,
        records,
    } = *input;
    let slot_bin = |n: u64| -> usize {
        usize::try_from((start_ns + stream.offset_ns(n)) / bin_ns).unwrap_or(usize::MAX)
    };
    let was_refused = |n: u64| {
        let place = refused.partition_point(|[_, last]| *last < n);
        refused.get(place).is_some_and(|[first, _]| *first <= n)
    };
    // The packets that were sent, in the bin of their slot
    let mut sent: Vec<u64> = Vec::new();
    for n in 0..slots {
        if was_refused(n) {
            continue;
        }
        let bin = slot_bin(n);
        if sent.len() <= bin {
            sent.resize(bin + 1, 0);
        }
        sent[bin] += 1;
    }
    // The first arrival of each packet that was sent
    let mut seen = vec![false; usize::try_from(slots).unwrap_or(usize::MAX)];
    let mut received: Vec<u64> = vec![0; sent.len()];
    let mut send_delays: Vec<Vec<u64>> = vec![Vec::new(); sent.len()];
    let mut arrived: Vec<u64> = Vec::new();
    let mut arrival_delays: Vec<Vec<u64>> = Vec::new();
    for record in records {
        if record.sequence >= slots || was_refused(record.sequence) {
            continue;
        }
        let seen = &mut seen[usize::try_from(record.sequence).unwrap_or(usize::MAX)];
        if std::mem::replace(seen, true) {
            continue;
        }
        let delay = record.delay_ns();
        let bin = slot_bin(record.sequence);
        received[bin] += 1;
        send_delays[bin].push(delay);
        let at = usize::try_from(record.receive_ns.saturating_sub(origin_ns) / bin_ns)
            .unwrap_or(usize::MAX);
        if arrived.len() <= at {
            arrived.resize(at + 1, 0);
            arrival_delays.resize(at + 1, Vec::new());
        }
        arrived[at] += 1;
        arrival_delays[at].push(delay);
    }
    let count = sent.len().max(arrived.len());
    (0..count)
        .map(|index| {
            let sent_here = sent.get(index).copied().unwrap_or(0);
            let received_here = received.get(index).copied().unwrap_or(0);
            let arrived_here = arrived.get(index).copied().unwrap_or(0);
            Bin {
                bin: index as u64,
                start_ns: index as u64 * bin_ns,
                by_send: BySend {
                    sent: sent_here,
                    received: received_here,
                    lost: sent_here - received_here,
                    delivered_ratio: (sent_here > 0)
                        .then(|| received_here as f64 / sent_here as f64),
                    delay: summarize(send_delays.get_mut(index)),
                },
                by_arrival: ByArrival {
                    received: arrived_here,
                    bits: arrived_here * stream.packet_size * 8,
                    delay: summarize(arrival_delays.get_mut(index)),
                },
            }
        })
        .collect()
}

/// The delay summary of the delays of one bin, or `None` if there are none.
fn summarize(delays: Option<&mut Vec<u64>>) -> Option<BinDelay> {
    let delays = delays.filter(|delays| !delays.is_empty())?;
    delays.sort_unstable();
    let sum: u128 = delays.iter().map(|&d| u128::from(d)).sum();
    Some(BinDelay {
        mean_ns: u64::try_from(sum / delays.len() as u128).unwrap_or(u64::MAX),
        p50_ns: nearest_rank(delays, 500),
        p95_ns: nearest_rank(delays, 950),
        p99_ns: nearest_rank(delays, 990),
        max_ns: delays[delays.len() - 1],
    })
}

/// One line of the file of the series: a bin and the workload it belongs to.
#[derive(Serialize)]
struct Line<'a> {
    workload: &'a str,
    #[serde(flatten)]
    bin: &'a Bin,
}

/// Writes the bins as JSON Lines, one line for each bin, in order (the file `series-<id>.jsonl`).
///
/// # Errors
///
/// The error of `out`.
pub fn write_series(mut out: impl Write, workload: &str, bins: &[Bin]) -> io::Result<()> {
    for bin in bins {
        serde_json::to_writer(&mut out, &Line { workload, bin }).map_err(io::Error::from)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 800 kbit with 1000-byte packets: 100 packets a second, a packet every 10 ms.
    const STREAM: Stream = Stream {
        bits_per_second: 800_000,
        packet_size: 1000,
    };

    /// An arrival of packet `n`, sent at its slot (10 ms apart, from `start` ms) and received `delay_ms` later.
    fn arrival(n: u64, start_ms: u64, delay_ms: u64) -> Record {
        Record {
            sequence: n,
            send_ns: (start_ms + n * 10) * 1_000_000,
            receive_ns: (start_ms + n * 10 + delay_ms) * 1_000_000,
        }
    }

    fn input<'a>(slots: u64, refused: &'a [[u64; 2]], records: &'a [Record]) -> Input<'a> {
        Input {
            stream: STREAM,
            start_ns: 0,
            origin_ns: 0,
            slots,
            refused,
            records,
        }
    }

    /// 300 packets over 3 s, a bin of 1 s: 100 sent in each; a delay of 100 ms moves the arrivals of the last ten
    /// packets of each bin into the next, and not the sends. The two views differ exactly by that.
    #[test]
    fn the_two_views_count_the_same_packets_in_different_bins() {
        let records: Vec<Record> = (0..300).map(|n| arrival(n, 0, 100)).collect();
        let found = bins(&input(300, &[], &records), 1_000_000_000);
        assert_eq!(
            found.len(),
            4,
            "the arrivals of the last packets are in a fourth bin"
        );
        let sent: Vec<u64> = found.iter().map(|b| b.by_send.sent).collect();
        let by_send_received: Vec<u64> = found.iter().map(|b| b.by_send.received).collect();
        let arrived: Vec<u64> = found.iter().map(|b| b.by_arrival.received).collect();
        assert_eq!(sent, [100, 100, 100, 0]);
        assert_eq!(by_send_received, [100, 100, 100, 0]);
        assert_eq!(arrived, [90, 100, 100, 10]);
        // Canary: the views are not the same series
        assert_ne!(sent, arrived);
        assert_eq!(found[1].start_ns, 1_000_000_000);
        assert_eq!(found[1].by_arrival.bits, 100 * 1000 * 8);
    }

    /// A packet that did not arrive is lost in the bin in which it was sent, and the ratio says so.
    #[test]
    fn a_lost_packet_belongs_to_the_bin_in_which_it_was_sent() {
        // The packets 120 to 139 are lost
        let records: Vec<Record> = (0..300)
            .filter(|n| !(120..140).contains(n))
            .map(|n| arrival(n, 0, 20))
            .collect();
        let found = bins(&input(300, &[], &records), 1_000_000_000);
        // A fourth bin holds the arrivals of the last packets, 20 ms after the end of the third second
        assert_eq!(
            found.iter().map(|b| b.by_send.lost).collect::<Vec<_>>(),
            [0, 20, 0, 0]
        );
        assert_eq!(found[1].by_send.delivered_ratio, Some(0.8));
        assert_eq!(found[0].by_send.delivered_ratio, Some(1.0));
        // The arrivals miss them too, 20 ms later
        assert_eq!(found[1].by_arrival.received, 80);
    }

    /// A refused packet was never sent: it is in no `sent`, not in `lost`, and an arrival with its number is left out.
    #[test]
    fn a_refused_packet_is_neither_sent_nor_lost() {
        let records: Vec<Record> = (0..100)
            .filter(|n| !(40..60).contains(n))
            .map(|n| arrival(n, 0, 5))
            .collect();
        let found = bins(&input(100, &[[40, 59]], &records), 1_000_000_000);
        assert_eq!(found[0].by_send.sent, 80);
        assert_eq!(found[0].by_send.lost, 0);
        assert_eq!(found[0].by_send.received, 80);
        // Canary: counted as sent, they would be lost
        let wrong = bins(&input(100, &[], &records), 1_000_000_000);
        assert_eq!(wrong[0].by_send.lost, 20);
    }

    /// The bins begin at the start of the measured phase, not at the first packet; a workload that starts late has empty
    /// bins in front of it, which have no ratio.
    #[test]
    fn a_workload_that_starts_late_has_empty_bins_in_front() {
        let mut late = input(100, &[], &[]);
        let records: Vec<Record> = (0..100).map(|n| arrival(n, 2_000, 10)).collect();
        late.records = &records;
        late.start_ns = 2_000_000_000;
        let found = bins(&late, 1_000_000_000);
        // Two empty seconds, the second of the sends, and the arrivals of the last ten milliseconds after it
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].by_send.sent, 0);
        assert_eq!(found[0].by_send.delivered_ratio, None);
        assert_eq!(found[0].by_send.delay, None);
        assert_eq!(found[2].by_send.sent, 100);
    }

    /// The delay of a bin is the nearest rank of the delays of its packets, and a second arrival of a packet is left out.
    #[test]
    fn the_delay_of_a_bin_is_the_nearest_rank_and_a_copy_is_not_counted() {
        // Ten packets in one bin with delays of 1 to 10 ms, and a copy of the first
        let mut records: Vec<Record> = (0..10).map(|n| arrival(n, 0, n + 1)).collect();
        records.push(arrival(0, 0, 50));
        let found = bins(&input(10, &[], &records), 1_000_000_000);
        let delay = found[0].by_send.delay.clone().unwrap();
        assert_eq!(
            (delay.p50_ns, delay.p95_ns, delay.p99_ns, delay.max_ns),
            (5_000_000, 10_000_000, 10_000_000, 10_000_000)
        );
        assert_eq!(delay.mean_ns, 5_500_000);
        assert_eq!(found[0].by_send.received, 10);
        assert_eq!(found[0].by_arrival.received, 10);
    }

    /// A smaller bin splits the same packets, and the counts add up to the same.
    #[test]
    fn a_smaller_bin_splits_the_same_packets() {
        let records: Vec<Record> = (0..300).map(|n| arrival(n, 0, 20)).collect();
        let coarse = bins(&input(300, &[], &records), 1_000_000_000);
        let fine = bins(&input(300, &[], &records), 250_000_000);
        assert_eq!(
            coarse.iter().map(|b| b.by_send.sent).sum::<u64>(),
            fine.iter().map(|b| b.by_send.sent).sum::<u64>()
        );
        assert_eq!(fine[0].by_send.sent, 25);
        assert_eq!(
            fine.len(),
            13,
            "twelve of the slots and the arrivals after them"
        );
    }

    #[test]
    fn nothing_sent_is_no_bins_and_the_file_has_a_line_for_each_bin() {
        assert_eq!(bins(&input(0, &[], &[]), 1_000_000_000).len(), 0);
        let records: Vec<Record> = (0..100).map(|n| arrival(n, 0, 5)).collect();
        let found = bins(&input(100, &[], &records), 500_000_000);
        let mut out = Vec::new();
        write_series(&mut out, "media", &found).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), found.len());
        let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["workload"], "media");
        assert_eq!(first["bin"], 0);
        assert_eq!(first["by_send"]["sent"], 50);
        assert_eq!(first["by_send"]["delivered_ratio"], 1.0);
        assert_eq!(first["by_arrival"]["bits"], 50 * 8000);
    }
}
