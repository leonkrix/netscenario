//! The time to recover after an event of the timeline (PRB-07, D-070): how long after a `set`, a `down`, an `up` or the end
//! of an `outage` a workload is delivered again at the level it had before.
//!
//! **The level** is the share of the packets that arrive: for each bin of the series, the packets that arrived divided
//! by the packets that were sent in the same bin. Which packets are counted as having arrived is the **basis**:
//!
//! - `send`: the packets sent in the bin that arrived (in the bin or later), the `by_send` view of the series. It does not
//!   depend on the delay of the link, and a queue that is still full does not hold it down.
//! - `arrival`: the packets that arrived in the bin, the `by_arrival` view, over the packets sent in the same bin, and
//!   never more than all of them: a queue that empties after a rate limit lets more arrive in a bin than was sent in
//!   it, and that surplus must not make up for the bins before it. It follows what the receiver sees, including the
//!   silence of an `outage`, which is a window at the receiver (D-064).
//!
//! **The baseline** is the median level of the `baseline_bins` bins in which the workload sent, just before the
//! disturbance began. The disturbance begins at the event, except for the end of an `outage` (it began with the start of
//! the outage) and for an `up` (it began with the `down` before it).
//!
//! **Recovered** is the first bin from which a window of `window_bins` bins, taken together, has a level of at least
//! `fraction` of the baseline. The window looks ahead: the level must hold for that long, so a single good bin between
//! bad ones is not a recovery. The time is from the event to the beginning of that bin (0 if the bin is the one that
//! holds the event or an earlier one), so it has the resolution of a bin; a smaller bin gives a finer answer. If no
//! complete window reaches the level before the run ends, or there is no baseline, the time is `null` and `reason` says
//! why.

// Counts of packets are far below 2^52, where a double stops being exact
#![allow(clippy::cast_precision_loss)]

use serde::{Deserialize, Serialize};

use crate::result::EventRecord;
use crate::series::Bin;

/// Which view of the series the level is made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Basis {
    /// The packets sent in the bin that arrived (`by_send`).
    Send,
    /// The packets that arrived in the bin over the packets sent in it (`by_arrival`).
    Arrival,
}

impl Basis {
    /// The word for the result and the command line.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Basis::Send => "send",
            Basis::Arrival => "arrival",
        }
    }

    /// The basis a word names: `send` or `arrival`.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "send" => Some(Basis::Send),
            "arrival" => Some(Basis::Arrival),
            _ => None,
        }
    }
}

/// The parameters of the time to recover. They are written into every result, so a number is never separated from the
/// definition it was made with.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RecoveryOptions {
    /// The view the level is made from. Default `send`.
    pub basis: Basis,
    /// How many bins before the disturbance make the baseline. Default 5.
    pub baseline_bins: u32,
    /// How many bins a window has. Default 3.
    pub window_bins: u32,
    /// The share of the baseline that has to be reached again, from 0 to 1. Default 0.9.
    pub fraction: f64,
}

impl Default for RecoveryOptions {
    fn default() -> Self {
        Self {
            basis: Basis::Send,
            baseline_bins: 5,
            window_bins: 3,
            fraction: 0.9,
        }
    }
}

impl RecoveryOptions {
    /// Whether the parameters make sense.
    ///
    /// # Errors
    ///
    /// Text that names the parameter, if a count is zero or the fraction is not in `(0, 1]`.
    pub fn check(&self) -> Result<(), String> {
        if self.baseline_bins == 0 {
            return Err("the baseline of the recovery needs at least one bin".to_owned());
        }
        if self.window_bins == 0 {
            return Err("the window of the recovery needs at least one bin".to_owned());
        }
        if !(self.fraction > 0.0 && self.fraction <= 1.0) {
            return Err(format!(
                "the fraction of the recovery is {} and must be above 0 and at most 1",
                self.fraction
            ));
        }
        Ok(())
    }
}

/// The time to recover of one workload after one event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Recovery {
    /// The place of the event in `timeline`.
    pub event: usize,
    /// `set`, `outage-end`, `down` or `up` (the start of an outage has none: its end has).
    pub kind: String,
    /// The link.
    pub link: String,
    /// The direction, or `null` for a step of the whole link.
    pub direction: Option<String>,
    /// The view the level was made from.
    pub basis: Basis,
    /// The moment from which the time is counted, in nanoseconds from the start of the measured phase.
    pub from_ns: u64,
    /// When the disturbance began, in the same nanoseconds: the baseline is taken before it.
    pub disturbance_ns: u64,
    /// The baseline level, or `null` if there is none.
    pub baseline: Option<f64>,
    /// The level that has to be reached again: `fraction` times the baseline.
    pub level: Option<f64>,
    /// How many bins make the baseline, how many a window, and which fraction was asked for.
    pub baseline_bins: u32,
    /// The bins of a window.
    pub window_bins: u32,
    /// The share of the baseline that was asked for.
    pub fraction: f64,
    /// Where the first window that reached the level begins, in nanoseconds from the start of the measured phase.
    pub recovered_at_ns: Option<u64>,
    /// The time from `from_ns` to `recovered_at_ns`, 0 if that is not later. `null` if the workload did not recover.
    pub time_ns: Option<u64>,
    /// Why there is no time: no baseline, or the level was not reached before the run ended.
    pub reason: Option<String>,
}

/// The packets of a bin that count as delivered on a basis: the arrivals are never more than the packets sent in the bin.
fn received_in(bin: &Bin, basis: Basis) -> u64 {
    match basis {
        Basis::Send => bin.by_send.received,
        Basis::Arrival => bin.by_arrival.received.min(bin.by_send.sent),
    }
}

/// The level of a bin on a basis, or `None` if the workload sent nothing in it.
fn level_of(bin: &Bin, basis: Basis) -> Option<f64> {
    if bin.by_send.sent == 0 {
        return None;
    }
    Some(received_in(bin, basis) as f64 / bin.by_send.sent as f64)
}

/// The median of `values` (the mean of the two in the middle if there are an even number), or `None` if there are none.
fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[middle]
    } else {
        f64::midpoint(values[middle - 1], values[middle])
    })
}

/// When the disturbance that `event` ends or belongs to began.
fn disturbance_of(event: &EventRecord, events: &[EventRecord]) -> u64 {
    match event.kind.as_str() {
        // The outage began with its own start, a step of the same event and direction
        "outage-end" => events
            .iter()
            .find(|other| {
                other.kind == "outage-start"
                    && other.event == event.event
                    && other.direction == event.direction
            })
            .map_or(event.scheduled_ns, |start| start.scheduled_ns),
        // The link went down at the last `down` of the link before it
        "up" => events
            .iter()
            .filter(|other| {
                other.kind == "down"
                    && other.link == event.link
                    && other.scheduled_ns <= event.scheduled_ns
            })
            .map(|down| down.scheduled_ns)
            .max()
            .unwrap_or(event.scheduled_ns),
        _ => event.scheduled_ns,
    }
}

/// The time to recover after each event of `events`, for the workload whose series is `bins` (each `bin_ns` wide).
/// `events` are the steps that concern the workload, in the order they were done; the start of an outage has no time of its
/// own, because the disturbance is over only at its end.
#[must_use]
pub fn recoveries(
    events: &[EventRecord],
    bins: &[Bin],
    bin_ns: u64,
    options: &RecoveryOptions,
) -> Vec<Recovery> {
    let window = options.window_bins as usize;
    events
        .iter()
        .filter(|event| event.kind != "outage-start")
        .map(|event| {
            let from_ns = event.scheduled_ns;
            let disturbance_ns = disturbance_of(event, events);
            let mut recovery = Recovery {
                event: event.event,
                kind: event.kind.clone(),
                link: event.link.clone(),
                direction: event.direction.clone(),
                basis: options.basis,
                from_ns,
                disturbance_ns,
                baseline: None,
                level: None,
                baseline_bins: options.baseline_bins,
                window_bins: options.window_bins,
                fraction: options.fraction,
                recovered_at_ns: None,
                time_ns: None,
                reason: None,
            };
            // The baseline: the bins in which the workload sent, just before the disturbance
            let began = usize::try_from(disturbance_ns / bin_ns).unwrap_or(usize::MAX);
            let first = began.saturating_sub(options.baseline_bins as usize);
            let before: Vec<f64> = bins
                .iter()
                .take(began)
                .skip(first)
                .filter_map(|bin| level_of(bin, options.basis))
                .collect();
            let Some(baseline) = median(before) else {
                recovery.reason = Some(
                    "there is no baseline: the workload sent nothing in the bins before the disturbance"
                        .to_owned(),
                );
                return recovery;
            };
            recovery.baseline = Some(baseline);
            if baseline <= 0.0 {
                recovery.reason = Some(
                    "there is no baseline: nothing was delivered in the bins before the disturbance"
                        .to_owned(),
                );
                return recovery;
            }
            let level = options.fraction * baseline;
            recovery.level = Some(level);
            // The first window, from the bin that holds the event on, that reaches the level
            let start = usize::try_from(from_ns / bin_ns).unwrap_or(usize::MAX);
            let found = (start..bins.len().saturating_sub(window - 1)).find(|&at| {
                let in_window = &bins[at..at + window];
                let sent: u64 = in_window.iter().map(|bin| bin.by_send.sent).sum();
                if in_window.iter().any(|bin| bin.by_send.sent == 0) || sent == 0 {
                    return false;
                }
                let received: u64 = in_window
                    .iter()
                    .map(|bin| received_in(bin, options.basis))
                    .sum();
                received as f64 / sent as f64 >= level
            });
            match found {
                Some(at) => {
                    let begins = bins[at].start_ns;
                    recovery.recovered_at_ns = Some(begins);
                    recovery.time_ns = Some(begins.saturating_sub(from_ns));
                }
                None => {
                    recovery.reason = Some(format!(
                        "the level {level:.3} was not reached again for {} bins before the run ended",
                        options.window_bins
                    ));
                }
            }
            recovery
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::series::{ByArrival, BySend};

    const SECOND: u64 = 1_000_000_000;

    /// A bin of a second: 100 packets sent, `delivered` of them arrived, and `arrived` packets arrived in it.
    fn bin(index: u64, delivered: u64, arrived: u64) -> Bin {
        Bin {
            bin: index,
            start_ns: index * SECOND,
            by_send: BySend {
                sent: 100,
                bits: 100 * 8,
                received: delivered,
                lost: 100 - delivered,
                delivered_ratio: Some(delivered as f64 / 100.0),
                delay: None,
            },
            by_arrival: ByArrival {
                received: arrived,
                bits: arrived * 8000,
                delay: None,
            },
        }
    }

    /// Bins from the delivered counts, with the arrivals the same.
    fn series(delivered: &[u64]) -> Vec<Bin> {
        delivered
            .iter()
            .enumerate()
            .map(|(i, d)| bin(i as u64, *d, *d))
            .collect()
    }

    fn event(kind: &str, event: usize, at_s: u64, direction: Option<&str>) -> EventRecord {
        let mut record = EventRecord::new(event, at_s * SECOND, at_s * SECOND, at_s * SECOND);
        kind.clone_into(&mut record.kind);
        record.direction = direction.map(str::to_owned);
        record
    }

    fn time_of(found: &[Recovery]) -> Vec<Option<u64>> {
        found
            .iter()
            .map(|r| r.time_ns.map(|t| t / SECOND))
            .collect()
    }

    /// An outage from 10 s to 13 s: nothing is delivered in the bins 10 to 12, and from 13 s on all of it is. The
    /// baseline is taken before the outage began and not before its end, which would be the outage itself.
    #[test]
    fn the_baseline_of_the_end_of_an_outage_is_taken_before_its_start() {
        let mut delivered = vec![100; 10];
        delivered.extend([0, 0, 0]);
        delivered.extend([100; 7]);
        let events = [
            event("outage-start", 0, 10, Some("forward")),
            event("outage-end", 0, 13, Some("forward")),
        ];
        let found = recoveries(
            &events,
            &series(&delivered),
            SECOND,
            &RecoveryOptions::default(),
        );
        assert_eq!(
            found.len(),
            1,
            "the start of an outage has no time of its own"
        );
        let r = &found[0];
        assert_eq!(
            (r.kind.as_str(), r.baseline, r.level),
            ("outage-end", Some(1.0), Some(0.9))
        );
        assert_eq!((r.disturbance_ns, r.from_ns), (10 * SECOND, 13 * SECOND));
        assert_eq!(time_of(&found), [Some(0)]);
        // Canary: with the baseline taken before the end there would be none (bins of 0)
        let wrong = recoveries(
            &[event("set", 0, 13, None)],
            &series(&delivered),
            SECOND,
            &RecoveryOptions {
                baseline_bins: 3,
                ..RecoveryOptions::default()
            },
        );
        assert_eq!(
            wrong[0]
                .reason
                .as_deref()
                .map(|r| r.contains("nothing was delivered")),
            Some(true)
        );
    }

    /// A slow recovery: 20 %, 50 %, 95 %, then all. With a window of three bins the first one that reaches 90 % is the one
    /// that begins at 15 s, and a single good bin between bad ones is not one.
    #[test]
    fn the_window_looks_ahead_so_that_a_single_good_bin_is_not_a_recovery() {
        let mut slow = vec![100; 10];
        slow.extend([0, 0, 0, 20, 50, 95, 100, 100, 100]);
        let events = [
            event("outage-start", 0, 10, None),
            event("outage-end", 0, 13, None),
        ];
        let found = recoveries(&events, &series(&slow), SECOND, &RecoveryOptions::default());
        // From 13 s: the windows 13..15 (0.55), 14..16 (0.817), 15..17 (0.983)
        assert_eq!(time_of(&found), [Some(2)]);
        assert_eq!(found[0].recovered_at_ns, Some(15 * SECOND));
        // One good bin, then a bad one: 100, 30, 100, 100, 100 after the event
        let mut flicker = vec![100; 10];
        flicker.extend([0, 0, 0, 100, 30, 100, 100, 100]);
        let found = recoveries(
            &events,
            &series(&flicker),
            SECOND,
            &RecoveryOptions::default(),
        );
        assert_eq!(
            time_of(&found),
            [Some(2)],
            "the good bin at 13 s is followed by a bad one"
        );
    }

    #[test]
    fn a_workload_that_does_not_recover_has_no_time_and_a_reason() {
        let mut delivered = vec![100; 10];
        delivered.extend([0; 10]);
        let events = [event("set", 0, 10, Some("forward"))];
        let found = recoveries(
            &events,
            &series(&delivered),
            SECOND,
            &RecoveryOptions::default(),
        );
        assert_eq!(found[0].time_ns, None);
        assert_eq!(found[0].recovered_at_ns, None);
        assert!(
            found[0]
                .reason
                .as_deref()
                .unwrap()
                .contains("was not reached again")
        );
        // A run that ends before a whole window is over has no recovery either
        let short = series(&[100; 11]);
        let found = recoveries(
            &[event("set", 0, 10, None)],
            &short,
            SECOND,
            &RecoveryOptions::default(),
        );
        assert!(found[0].reason.is_some());
    }

    /// The level is `>=`: exactly 90 % of the baseline is a recovery, and 89 % is not.
    #[test]
    fn the_level_is_reached_at_exactly_the_fraction_and_not_below() {
        let events = [event("set", 0, 5, None)];
        let mut at = vec![100; 5];
        at.extend([90; 5]);
        let found = recoveries(&events, &series(&at), SECOND, &RecoveryOptions::default());
        assert_eq!(time_of(&found), [Some(0)]);
        let mut below = vec![100; 5];
        below.extend([89; 5]);
        let found = recoveries(
            &events,
            &series(&below),
            SECOND,
            &RecoveryOptions::default(),
        );
        assert_eq!(found[0].time_ns, None);
        // A different fraction moves the line
        let strict = RecoveryOptions {
            fraction: 0.95,
            ..RecoveryOptions::default()
        };
        assert_eq!(
            recoveries(&events, &series(&at), SECOND, &strict)[0].time_ns,
            None
        );
    }

    /// An `up` is counted from its own moment and its baseline is taken before the `down` before it; a `down` with no
    /// `up` after it is counted from its own moment, and nothing comes back.
    #[test]
    fn an_up_pairs_with_the_down_before_it() {
        let mut delivered = vec![100; 8];
        delivered.extend([0; 4]);
        delivered.extend([100; 6]);
        let events = [event("down", 0, 8, None), event("up", 1, 12, None)];
        let found = recoveries(
            &events,
            &series(&delivered),
            SECOND,
            &RecoveryOptions::default(),
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].kind, "up");
        assert_eq!(found[1].disturbance_ns, 8 * SECOND);
        assert_eq!(found[1].baseline, Some(1.0));
        assert_eq!(found[1].time_ns, Some(0));
        // The `down` itself recovers when the link comes back, counted from the step: 4 s
        assert_eq!(found[0].kind, "down");
        assert_eq!(found[0].time_ns, Some(4 * SECOND));
    }

    /// An event early in the run has the bins that there are for a baseline, and one at the very start has none.
    #[test]
    fn an_event_early_in_the_run_has_a_short_baseline_and_at_the_start_none() {
        let delivered = [100, 100, 40, 100, 100, 100, 100];
        let found = recoveries(
            &[event("set", 0, 2, None)],
            &series(&delivered),
            SECOND,
            &RecoveryOptions::default(),
        );
        assert_eq!(found[0].baseline, Some(1.0));
        assert_eq!(found[0].time_ns, Some(SECOND));
        let found = recoveries(
            &[event("set", 0, 0, None)],
            &series(&delivered),
            SECOND,
            &RecoveryOptions::default(),
        );
        assert!(found[0].reason.as_deref().unwrap().contains("no baseline"));
    }

    /// The basis changes what is counted: the arrivals are silent in the receiver's window of an outage while the packets
    /// sent in the bins before it (a delay of one bin) are lost. Here the packets sent in bin 9 arrive in bin 10.
    #[test]
    fn the_basis_chooses_the_view_of_the_series() {
        // Sent in 0..9, 12..: delivered. Sent in 10 and 11: lost. Arrivals: a delay of 1 s moves them one bin later.
        let mut bins: Vec<Bin> = (0..20)
            .map(|i| {
                let delivered = if (10..12).contains(&i) { 0 } else { 100 };
                let arrived = if (11..13).contains(&i) { 0 } else { 100 };
                bin(i, delivered, arrived)
            })
            .collect();
        bins[10].by_arrival.received = 100;
        let events = [
            event("outage-start", 0, 10, None),
            event("outage-end", 0, 12, None),
        ];
        let send = recoveries(&events, &bins, SECOND, &RecoveryOptions::default());
        let arrival = recoveries(
            &events,
            &bins,
            SECOND,
            &RecoveryOptions {
                basis: Basis::Arrival,
                ..RecoveryOptions::default()
            },
        );
        // By the send view the packets sent from 12 s on are all delivered; by the arrival view the arrivals are back at
        // 13 s, a second after the end of the outage in the sender's frame
        assert_eq!(time_of(&send), [Some(0)]);
        assert_eq!(time_of(&arrival), [Some(1)]);
        assert_eq!(arrival[0].basis, Basis::Arrival);
    }

    /// The surplus of a queue that empties does not make up for the bins before it: with 30 % arriving in two bins and
    /// 250 % in the next, the window of three is not at the level, as an average without the cap would say.
    #[test]
    fn the_arrivals_of_a_bin_count_for_at_most_the_packets_sent_in_it() {
        let mut bins: Vec<Bin> = (0..5).map(|i| bin(i, 100, 100)).collect();
        bins.extend([bin(5, 100, 30), bin(6, 100, 30), bin(7, 100, 250)]);
        bins.extend((8..14).map(|i| bin(i, 100, 100)));
        let events = [event("set", 0, 5, None)];
        let arrival = RecoveryOptions {
            basis: Basis::Arrival,
            ..RecoveryOptions::default()
        };
        let found = recoveries(&events, &bins, SECOND, &arrival);
        // The windows 5..8 (0.53), 6..9 (0.77) and 7..10 (1.0): recovered at 7 s, 2 s after the event
        assert_eq!(time_of(&found), [Some(2)]);
        // Canary: by the send view nothing was undelivered
        let send = recoveries(&events, &bins, SECOND, &RecoveryOptions::default());
        assert_eq!(time_of(&send), [Some(0)]);
    }

    #[test]
    fn parameters_that_make_no_sense_are_refused_with_the_name() {
        let ok = RecoveryOptions::default();
        assert!(ok.check().is_ok());
        for (options, word) in [
            (
                RecoveryOptions {
                    baseline_bins: 0,
                    ..ok
                },
                "baseline",
            ),
            (
                RecoveryOptions {
                    window_bins: 0,
                    ..ok
                },
                "window",
            ),
            (
                RecoveryOptions {
                    fraction: 0.0,
                    ..ok
                },
                "fraction",
            ),
            (
                RecoveryOptions {
                    fraction: 1.5,
                    ..ok
                },
                "fraction",
            ),
            (
                RecoveryOptions {
                    fraction: f64::NAN,
                    ..ok
                },
                "fraction",
            ),
        ] {
            let error = options.check().unwrap_err();
            assert!(error.contains(word), "{error}");
        }
        assert_eq!(Basis::from_word("send"), Some(Basis::Send));
        assert_eq!(
            Basis::from_word("arrival").map(Basis::word),
            Some("arrival")
        );
        assert_eq!(Basis::from_word("sent"), None);
    }
}
