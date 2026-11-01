//! The patterns in the arrivals of a workload: how far packets are out of order (RFC 4737), how much the spacing
//! of the packets varies (RFC 3550), and how the lost packets are grouped (RFC 3357) (PRB-04, PRB-06).
//!
//! The functions take the **first arrivals** in the order they happened (a packet that arrives twice counts once,
//! at its first arrival, and a number the sender never used is not an arrival; [`first_arrivals`]) and do plain
//! arithmetic, so that they can be tested against the worked examples of the RFCs and against series that are
//! worked out by hand. Times are nanoseconds on the one clock of the machine, which every node shares, so a
//! difference of a send time and a receive time is exact and not an estimate.

// Counts and times are far below 2^52, where a double stops being exact; the rounding of a mean is meant
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::{BTreeMap, HashSet};

use netscenario_agent::records::Record;
use serde::{Deserialize, Serialize};

/// The first arrival of every packet that the sender sent, in the order of arrival.
///
/// A second arrival of a packet is left out (RFC 4737, section 3.6: only the first copy is analysed), and so is
/// a record with a sequence number from `sent` on, which a correct sender and receiver never produce.
#[must_use]
pub fn first_arrivals(sent: u64, records: &[Record]) -> Vec<Record> {
    let mut seen = HashSet::with_capacity(records.len());
    records
        .iter()
        .filter(|record| record.sequence < sent && seen.insert(record.sequence))
        .copied()
        .collect()
}

/// How far the reordered packets were out of place (RFC 4737, section 4.2) and how long a receiver would have
/// had to hold them to restore the order (section 4.3).
///
/// A packet is **reordered** if it arrives after a packet with a higher sequence number. Its **extent** is the
/// number of arrivals between it and the earliest-arrived packet with a higher number: a packet that arrives
/// right after the packet that overtook it has the extent 1. Its **late time** is the time between those two
/// arrivals. The summaries are over the reordered packets only; they are absent where nothing was reordered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Reordering {
    /// The extent of the reordered packets, in packets.
    pub extent: ExtentSummary,
    /// The late time of the reordered packets.
    pub late_time: LateTimeSummary,
}

/// The extents of the reordered packets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExtentSummary {
    /// The largest extent: a buffer that restores the order needs at least this many packets of room (or fewer,
    /// see RFC 4737, section 7.2).
    pub max: u64,
    /// The mean extent.
    pub mean: f64,
    /// The median (nearest rank, as for the delays).
    pub p50: u64,
    /// Percentile 95.
    pub p95: u64,
    /// Percentile 99.
    pub p99: u64,
}

/// The late times of the reordered packets, in nanoseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LateTimeSummary {
    /// The largest late time: how long the receiver would have waited, at most, for the packet that was missing.
    pub max_ns: u64,
    /// The mean, rounded down.
    pub mean_ns: u64,
    /// The median (nearest rank).
    pub p50_ns: u64,
    /// Percentile 95.
    pub p95_ns: u64,
    /// Percentile 99.
    pub p99_ns: u64,
}

/// The delay variation as RFC 3550 (appendix A.8) estimates it for RTP: for two packets that arrive one after
/// the other, `D = (R_j - R_i) - (S_j - S_i)` is the difference of their transit times, and the estimate moves a
/// sixteenth of the way towards `|D|` with every packet: `J = J + (|D| - J) / 16`.
///
/// The packets are paired **in the order of arrival**, not of sequence, as the RFC says, so a reordered packet
/// shows up as a large `D`. The RFC defines the running estimate; a test of a link wants one number for the run, so
/// the result has the estimate after the last packet (what an RTCP report at the end would carry), its mean over
/// the run, which does not depend on the last sixteen packets, and its largest value. It is absent for fewer
/// than two arrivals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Jitter {
    /// The estimate after the last arrival, in nanoseconds, rounded.
    pub last_ns: u64,
    /// The mean of the estimate over the arrivals, rounded.
    pub mean_ns: u64,
    /// The largest value the estimate had.
    pub max_ns: u64,
}

/// The runs of lost packets (RFC 3357, "loss period"): a run is a stretch of consecutive sequence numbers none of
/// which arrived. A link with independent loss has mostly runs of length 1; a link with bursts has longer ones.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LossRuns {
    /// How many runs there were.
    pub runs: u64,
    /// The length of the longest run, 0 if no packet was lost.
    pub longest: u64,
    /// The mean length of a run, `lost / runs`, 0 if no packet was lost.
    pub mean: f64,
    /// For each length that occurred, how many runs had it, by ascending length.
    pub histogram: Vec<LossRunLength>,
}

/// How many runs of lost packets had one length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LossRunLength {
    /// The length of the run, in packets.
    pub length: u64,
    /// How many runs had it.
    pub runs: u64,
}

/// The nearest rank: the value at rank `ceil(permille * n / 1000)` of the ascending `sorted`.
fn nearest_rank(sorted: &[u64], permille: u64) -> u64 {
    let n = sorted.len() as u64;
    let rank = (permille * n).div_ceil(1000).max(1);
    sorted[usize::try_from(rank - 1).unwrap_or(0).min(sorted.len() - 1)]
}

/// The reordering of `arrivals` (see [`first_arrivals`]), or `None` if no packet arrived after a higher one.
///
/// The earliest arrival with a number above a packet's own is always a **new highest** at the moment it arrived
/// (everything before it was lower than the packet, hence lower than it), so it is found among the arrivals
/// that raised the highest number so far, which are in ascending order: a binary search, `O(n log n)` for the run.
#[must_use]
pub fn reordering(arrivals: &[Record]) -> Option<Reordering> {
    // (sequence, position of the arrival) of every arrival that raised the highest number
    let mut raised: Vec<(u64, usize)> = Vec::new();
    let mut extents = Vec::new();
    let mut late_times = Vec::new();
    for (position, arrival) in arrivals.iter().enumerate() {
        if raised
            .last()
            .is_some_and(|&(highest, _)| arrival.sequence < highest)
        {
            let first_above = raised.partition_point(|&(sequence, _)| sequence <= arrival.sequence);
            let (_, overtaking) = raised[first_above];
            extents.push((position - overtaking) as u64);
            late_times.push(
                arrival
                    .receive_ns
                    .saturating_sub(arrivals[overtaking].receive_ns),
            );
        } else {
            raised.push((arrival.sequence, position));
        }
    }
    if extents.is_empty() {
        return None;
    }
    let count = extents.len() as u128;
    let mean_extent = extents.iter().map(|&e| e as f64).sum::<f64>() / extents.len() as f64;
    let mean_late = late_times.iter().map(|&t| u128::from(t)).sum::<u128>() / count;
    extents.sort_unstable();
    late_times.sort_unstable();
    Some(Reordering {
        extent: ExtentSummary {
            max: extents[extents.len() - 1],
            mean: mean_extent,
            p50: nearest_rank(&extents, 500),
            p95: nearest_rank(&extents, 950),
            p99: nearest_rank(&extents, 990),
        },
        late_time: LateTimeSummary {
            max_ns: late_times[late_times.len() - 1],
            mean_ns: u64::try_from(mean_late).unwrap_or(u64::MAX),
            p50_ns: nearest_rank(&late_times, 500),
            p95_ns: nearest_rank(&late_times, 950),
            p99_ns: nearest_rank(&late_times, 990),
        },
    })
}

/// The interarrival jitter of `arrivals` (see [`first_arrivals`]), or `None` for fewer than two.
#[must_use]
pub fn interarrival_jitter(arrivals: &[Record]) -> Option<Jitter> {
    if arrivals.len() < 2 {
        return None;
    }
    let (mut estimate, mut sum, mut largest) = (0.0_f64, 0.0_f64, 0.0_f64);
    for pair in arrivals.windows(2) {
        // The two differences are exact in integers; their difference is the change of the transit time
        let received = i128::from(pair[1].receive_ns) - i128::from(pair[0].receive_ns);
        let sent = i128::from(pair[1].send_ns) - i128::from(pair[0].send_ns);
        let difference = (received - sent).unsigned_abs() as f64;
        estimate += (difference - estimate) / 16.0;
        sum += estimate;
        largest = largest.max(estimate);
    }
    let pairs = (arrivals.len() - 1) as f64;
    Some(Jitter {
        last_ns: estimate.round() as u64,
        mean_ns: (sum / pairs).round() as u64,
        max_ns: largest.round() as u64,
    })
}

/// The runs of lost packets among the `sent` sequence numbers, given the `arrivals` (see [`first_arrivals`]).
#[must_use]
pub fn loss_runs(sent: u64, arrivals: &[Record]) -> LossRuns {
    let mut present: Vec<u64> = arrivals.iter().map(|record| record.sequence).collect();
    present.sort_unstable();
    let mut counts: BTreeMap<u64, u64> = BTreeMap::new();
    let mut note = |length: u64| {
        if length > 0 {
            *counts.entry(length).or_insert(0) += 1;
        }
    };
    // The gap before each arrival, then the gap after the last one
    let mut next_missing = 0_u64;
    for sequence in present {
        note(sequence - next_missing);
        next_missing = sequence + 1;
    }
    note(sent - next_missing);
    let runs: u64 = counts.values().sum();
    let lost: u64 = counts.iter().map(|(length, count)| length * count).sum();
    LossRuns {
        runs,
        longest: counts.keys().next_back().copied().unwrap_or(0),
        mean: if runs == 0 {
            0.0
        } else {
            lost as f64 / runs as f64
        },
        histogram: counts
            .into_iter()
            .map(|(length, runs)| LossRunLength { length, runs })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arrival of packet `sequence`, received at `receive_ms`; sent `sequence` milliseconds after the start.
    fn arrival(sequence: u64, receive_ms: u64) -> Record {
        Record {
            sequence,
            send_ns: sequence * 1_000_000,
            receive_ns: receive_ms * 1_000_000,
        }
    }

    /// Arrivals in the order given, one millisecond apart.
    fn in_order_of(sequences: &[u64]) -> Vec<Record> {
        sequences
            .iter()
            .zip(1..)
            .map(|(&sequence, at)| arrival(sequence, at))
            .collect()
    }

    /// An order of arrival and the extent of each reordered packet that the RFC gives, as (sequence, extent).
    type Example<'a> = (&'a [u64], &'a [(u64, u64)]);

    fn extents_of(sequences: &[u64]) -> Vec<(u64, u64)> {
        // (sequence, extent) of every reordered packet, by the definition of the RFC, one at a time and slowly:
        // the extent of the arrival at position i is i - j for the smallest j with a higher sequence number
        let mut found = Vec::new();
        for (i, &sequence) in sequences.iter().enumerate() {
            if let Some(j) = sequences[..i].iter().position(|&s| s > sequence) {
                found.push((sequence, (i - j) as u64));
            }
        }
        found
    }

    /// The worked examples of RFC 4737, section 7: the order of arrival and the extents that the RFC gives.
    #[test]
    fn the_extents_of_the_examples_of_the_rfc_are_those_of_the_rfc() {
        let examples: [Example<'_>; 5] = [
            // 7.1: one packet late by four places
            (&[1, 2, 3, 5, 6, 7, 8, 4, 9, 10], &[(4, 4)]),
            // 7.2: two packets behind one that went ahead
            (&[1, 2, 3, 4, 7, 5, 6, 8, 9, 10], &[(5, 1), (6, 2)]),
            // 7.3: three packets behind four that went ahead
            (
                &[1, 2, 3, 7, 8, 9, 10, 4, 5, 6, 11],
                &[(4, 4), (5, 5), (6, 6)],
            ),
            // 7.4: three reordered packets in two places
            (
                &[1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 12, 13, 11, 14, 15, 16],
                &[(4, 2), (5, 3), (11, 2)],
            ),
            // in order: nothing
            (&[1, 2, 3, 4, 5], &[]),
        ];
        for (order, expected) in examples {
            // The slow definition agrees with the RFC (so the table above is read right) ...
            assert_eq!(extents_of(order), expected, "{order:?}");
            // ... and so does the code
            let summary = reordering(&in_order_of(order));
            match (expected.is_empty(), summary) {
                (true, None) => {}
                (false, Some(summary)) => {
                    let mut extents: Vec<u64> = expected.iter().map(|&(_, e)| e).collect();
                    extents.sort_unstable();
                    assert_eq!(summary.extent.max, extents[extents.len() - 1], "{order:?}");
                    assert!(
                        (summary.extent.mean
                            - extents.iter().sum::<u64>() as f64 / extents.len() as f64)
                            .abs()
                            < 1e-12,
                        "{order:?}"
                    );
                    assert_eq!(summary.extent.p50, nearest_rank(&extents, 500), "{order:?}");
                }
                (expected_none, got) => panic!("{order:?}: {expected_none} but {got:?}"),
            }
        }
    }

    /// The late time of the examples of RFC 4737, sections 7.1 and 7.3: the time between the arrival of the
    /// packet and the arrival of the first packet that overtook it.
    #[test]
    fn the_late_times_of_the_examples_of_the_rfc_are_those_of_the_rfc() {
        // 7.1: packet 5 arrived at 148 ms, packet 4 at 210 ms: 62 ms
        let mut order = in_order_of(&[1, 2, 3, 5, 6, 7, 8, 4, 9, 10]);
        order[3].receive_ns = 148_000_000;
        for (n, record) in order.iter_mut().enumerate().take(3) {
            record.receive_ns = (n as u64 + 1) * 40_000_000;
        }
        for (n, record) in order.iter_mut().enumerate().skip(4).take(3) {
            record.receive_ns = 148_000_000 + (n as u64) * 1_000_000;
        }
        order[7].receive_ns = 210_000_000;
        order[8].receive_ns = 211_000_000;
        order[9].receive_ns = 212_000_000;
        let summary = reordering(&order).unwrap();
        assert_eq!(summary.late_time.max_ns, 62_000_000);
        assert_eq!(summary.late_time.mean_ns, 62_000_000);
        // 7.3: packets 4, 5 and 6 arrive 62, 64 and 68 ms after packet 7
        let mut order = in_order_of(&[1, 2, 3, 7, 8, 9, 10, 4, 5, 6, 11]);
        order[3].receive_ns = 148_000_000;
        for (i, late) in [(7, 62), (8, 64), (9, 68)] {
            order[i].receive_ns = (148 + late) * 1_000_000;
        }
        order[4].receive_ns = 149_000_000;
        order[5].receive_ns = 150_000_000;
        order[6].receive_ns = 151_000_000;
        order[10].receive_ns = 220_000_000;
        let summary = reordering(&order).unwrap();
        assert_eq!(summary.late_time.max_ns, 68_000_000);
        assert_eq!(summary.late_time.mean_ns, 64_666_666);
        assert_eq!(summary.late_time.p50_ns, 64_000_000);
    }

    /// The fast form against the slow definition on series that are not from the RFC: every permutation of six
    /// packets, and a long series with scattered displacements.
    #[test]
    fn the_fast_extent_equals_the_definition_on_every_order_of_six_packets_and_on_a_long_one() {
        fn permutations(items: &mut Vec<u64>, from: usize, all: &mut Vec<Vec<u64>>) {
            if from == items.len() {
                all.push(items.clone());
                return;
            }
            for i in from..items.len() {
                items.swap(from, i);
                permutations(items, from + 1, all);
                items.swap(from, i);
            }
        }
        let mut all = Vec::new();
        permutations(&mut (0..6).collect(), 0, &mut all);
        assert_eq!(all.len(), 720);
        let mut long: Vec<u64> = (0..2000).collect();
        // A deterministic shuffle of neighbours at several distances
        for i in 0..1990_usize {
            if (i * 7 + i / 3) % 5 == 0 {
                long.swap(i, i + 1 + (i % 9));
            }
        }
        all.push(long);
        for order in all {
            let slow = extents_of(&order);
            let fast = reordering(&in_order_of(&order));
            assert_eq!(fast.is_some(), !slow.is_empty(), "{order:?}");
            if let Some(fast) = fast {
                let mut extents: Vec<u64> = slow.iter().map(|&(_, e)| e).collect();
                extents.sort_unstable();
                assert_eq!(fast.extent.max, extents[extents.len() - 1], "{order:?}");
                assert_eq!(fast.extent.p95, nearest_rank(&extents, 950), "{order:?}");
                let mean = extents.iter().sum::<u64>() as f64 / extents.len() as f64;
                assert!((fast.extent.mean - mean).abs() < 1e-9, "{order:?}");
            }
        }
    }

    /// A packet that arrives twice counts once, at its first arrival; a number the sender never used does not
    /// count; and the order of arrival is kept.
    #[test]
    fn only_the_first_arrival_of_a_packet_counts() {
        let records = [
            arrival(0, 1),
            arrival(2, 2),
            arrival(1, 3),
            arrival(2, 4),
            arrival(99, 5),
            arrival(1, 6),
        ];
        let firsts = first_arrivals(3, &records);
        let sequences: Vec<u64> = firsts.iter().map(|r| r.sequence).collect();
        assert_eq!(sequences, [0, 2, 1]);
        assert_eq!(reordering(&firsts).unwrap().extent.max, 1);
        // Canary: a late copy of an older packet would be a reordered packet if copies were not left out
        let with_copy = [
            arrival(0, 1),
            arrival(1, 2),
            arrival(2, 3),
            arrival(3, 4),
            arrival(1, 5),
        ];
        assert!(
            reordering(&with_copy).is_some(),
            "the copy alone is out of order"
        );
        assert_eq!(reordering(&first_arrivals(4, &with_copy)), None);
    }

    /// RFC 3550, appendix A.8, worked by hand: transit times of 10 and 20 ms in turn make `|D|` 10 ms every time,
    /// and `J` approaches 10 ms like `10 * (1 - (15/16)^n)`.
    #[test]
    fn the_jitter_of_a_series_with_a_known_estimate_is_the_estimate_of_the_rfc() {
        // Packet n is sent at n ms and travels 10 ms (even n) or 20 ms (odd n)
        let arrivals: Vec<Record> = (0..40_u64)
            .map(|n| Record {
                sequence: n,
                send_ns: n * 1_000_000,
                receive_ns: n * 1_000_000 + if n % 2 == 0 { 10_000_000 } else { 20_000_000 },
            })
            .collect();
        let jitter = interarrival_jitter(&arrivals).unwrap();
        // 39 pairs: J_n = 10 ms * (1 - (15/16)^n)
        let estimate = |n: i32| 10_000_000.0 * (1.0 - (15.0_f64 / 16.0).powi(n));
        assert_eq!(jitter.last_ns, estimate(39).round() as u64);
        let mean = (1..=39).map(estimate).sum::<f64>() / 39.0;
        assert_eq!(jitter.mean_ns, mean.round() as u64);
        assert_eq!(
            jitter.max_ns, jitter.last_ns,
            "the estimate only rises here"
        );
        // The first steps, by hand: 0.625 ms, then 1.2109375 ms
        let two = interarrival_jitter(&arrivals[..3]).unwrap();
        assert_eq!(two.last_ns, 1_210_938);
        let one = interarrival_jitter(&arrivals[..2]).unwrap();
        assert_eq!(
            (one.last_ns, one.mean_ns, one.max_ns),
            (625_000, 625_000, 625_000)
        );
    }

    /// A constant transit time has no jitter, however the packets are spaced; one packet has none to measure;
    /// and the pairs are those of the order of arrival, so a reordered packet makes a large difference.
    #[test]
    fn a_steady_stream_has_no_jitter_and_reordering_shows_in_it() {
        let steady: Vec<Record> = (0..100_u64)
            .map(|n| Record {
                sequence: n,
                send_ns: n * n * 1_000,
                receive_ns: n * n * 1_000 + 5_000_000,
            })
            .collect();
        let jitter = interarrival_jitter(&steady).unwrap();
        assert_eq!((jitter.last_ns, jitter.mean_ns, jitter.max_ns), (0, 0, 0));
        assert_eq!(interarrival_jitter(&steady[..1]), None);
        assert_eq!(interarrival_jitter(&[]), None);
        // Packets 1 and 2 swap, all with the same transit time except that 2 is held 2 ms: D = 2 ms and then 2 ms
        // in the other sense, and in the order of arrival the pairs are (0,2), (2,1), (1,3)
        let mut swapped = vec![
            arrival(0, 10),
            arrival(2, 12),
            arrival(1, 13),
            arrival(3, 14),
        ];
        // Transit: 0: 10 ms, 2: 10 ms, 1: 12 ms, 3: 11 ms
        swapped[0].send_ns = 0;
        swapped[0].receive_ns = 10_000_000;
        swapped[1].send_ns = 2_000_000;
        swapped[1].receive_ns = 12_000_000;
        swapped[2].send_ns = 1_000_000;
        swapped[2].receive_ns = 13_000_000;
        swapped[3].send_ns = 3_000_000;
        swapped[3].receive_ns = 14_000_000;
        // D: (0,2) = 0; (2,1) = 12 - 10 = 2; (1,3) = 11 - 12 = -1
        // J: 0; 0 + 2/16 = 0.125; 0.125 + (1 - 0.125)/16 = 0.1796875 ms
        let jitter = interarrival_jitter(&swapped).unwrap();
        assert_eq!(jitter.last_ns, 179_688);
        assert_eq!(jitter.max_ns, 179_688);
    }

    /// The runs of lost packets: a hand-made series with every kind of run, the ends included.
    #[test]
    fn the_runs_of_lost_packets_are_counted_by_length() {
        // Sent 0..=14; arrived 2, 3, 6, 10, 11, 12: lost 0-1 (2), 4-5 (2), 7-9 (3), 13-14 (2)
        let arrived = in_order_of(&[2, 3, 6, 10, 11, 12]);
        let runs = loss_runs(15, &arrived);
        assert_eq!(runs.runs, 4);
        assert_eq!(runs.longest, 3);
        assert!((runs.mean - 9.0 / 4.0).abs() < 1e-12);
        assert_eq!(
            runs.histogram
                .iter()
                .map(|bucket| (bucket.length, bucket.runs))
                .collect::<Vec<_>>(),
            [(2, 3), (3, 1)]
        );
        // The order of arrival does not matter, only the numbers
        let shuffled = in_order_of(&[12, 2, 10, 6, 3, 11]);
        assert_eq!(loss_runs(15, &shuffled), runs);
        // Nothing lost, everything lost, nothing sent
        let all = in_order_of(&(0..5).collect::<Vec<_>>());
        assert_eq!(loss_runs(5, &all), LossRuns::default());
        let none = loss_runs(5, &[]);
        assert_eq!((none.runs, none.longest, none.mean), (1, 5, 5.0));
        assert_eq!(loss_runs(0, &[]), LossRuns::default());
    }

    /// Canary: the runs are not the lost packets. Independent losses of every other packet are 5 runs of 1,
    /// and the same number of lost packets in a block is 1 run of 5: the statistic tells a burst from scatter.
    #[test]
    fn a_burst_and_scattered_loss_of_the_same_size_have_different_runs() {
        let scattered = in_order_of(&[1, 3, 5, 7, 9]);
        let burst = in_order_of(&[5, 6, 7, 8, 9]);
        let (scattered, burst) = (loss_runs(10, &scattered), loss_runs(10, &burst));
        assert_eq!((scattered.runs, scattered.longest), (5, 1));
        assert_eq!((burst.runs, burst.longest), (1, 5));
        assert!((scattered.mean - 1.0).abs() < 1e-12 && (burst.mean - 5.0).abs() < 1e-12);
    }
}
