//! The moments at which the packets of a workload leave (WRK-01, WRK-02).
//!
//! A [`Schedule`] is one of four patterns: a constant bit rate, a Poisson process, an on/off source and bursts. The
//! constant bit rate is closed-form ([`Stream`]); the others are made by walking a seeded generator ([`crate::rng`]) from
//! the start, because the moment of packet `n` depends on the gaps before it. The sender walks it once, in order; the run
//! walks it again after the fact (the series, the barriers of the timeline) and gets the same moments, because the walk is
//! a function of the schedule and its seed alone (D-071).
//!
//! For the constant bit rate, packet `n` leaves at `start + n * size * 8 / rate` seconds. The moment is computed from `n`, not by
//! adding an interval to the last moment, so a rate whose interval is not a whole number of nanoseconds
//! (3 mbit with 1000 byte packets: 2.666... ms) does not drift: after a million packets the stream is exactly
//! where the arithmetic puts it. The computation is in integers.

// Moments are below 2^53 nanoseconds (104 days), where a double is exact; a draw is rounded down to a moment on purpose
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::rng::Xoshiro256PlusPlus;

/// A constant bit rate stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    /// Payload bits per second.
    pub bits_per_second: u64,
    /// Payload bytes per packet.
    pub packet_size: u64,
}

impl Stream {
    /// How long after the start packet `n` leaves, in nanoseconds (rounded down).
    ///
    /// # Panics
    ///
    /// If the rate is zero; the model refuses such a rate, and the command line of the agent too.
    #[must_use]
    pub fn offset_ns(&self, n: u64) -> u64 {
        let bits = u128::from(n) * u128::from(self.packet_size) * 8;
        let ns = bits * 1_000_000_000 / u128::from(self.bits_per_second);
        u64::try_from(ns).unwrap_or(u64::MAX)
    }

    /// How many packets leave in the first `duration_ns` nanoseconds: those with an offset below it.
    #[must_use]
    pub fn packets_in(&self, duration_ns: u64) -> u64 {
        // The offset of packet n is below d exactly when n * size * 8 * 1e9 < d * rate, rounded down
        let numerator = u128::from(duration_ns) * u128::from(self.bits_per_second);
        let per_packet = u128::from(self.packet_size) * 8 * 1_000_000_000;
        let whole = numerator / per_packet;
        let count = if numerator % per_packet == 0 {
            whole
        } else {
            whole + 1
        };
        u64::try_from(count).unwrap_or(u64::MAX)
    }
}

/// Whether the lengths of the phases of an on/off source, and the pauses between bursts, are exactly what was asked for
/// or exponentially distributed with that mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distribution {
    /// Every phase lasts exactly the given time.
    Fixed,
    /// The lengths are drawn from an exponential distribution with the given mean (a Markov source).
    Exponential,
}

/// The moments at which the packets of one workload leave, counted from its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// A packet every `size * 8 / rate` seconds.
    Cbr(Stream),
    /// Gaps between the packets drawn from an exponential distribution with the mean `size * 8 / rate`; the first packet
    /// leaves after the first gap. The rate is the mean rate.
    Poisson {
        /// The mean rate and the size.
        stream: Stream,
        /// The seed of the generator ([`crate::rng::derive_seed`] of the seed of the run).
        seed: u64,
    },
    /// A phase of `on_ns` in which packets leave at the constant rate of `stream`, then a phase of `off_ns` with none, and
    /// so on from the start, which is the start of an on phase. A packet leaves at the start of an on phase, and at the
    /// rate's intervals after it while they fall inside the phase.
    OnOff {
        /// The rate while on, and the size.
        stream: Stream,
        /// The length of an on phase, or its mean.
        on_ns: u64,
        /// The length of an off phase, or its mean.
        off_ns: u64,
        /// Whether the lengths are exact or drawn.
        distribution: Distribution,
        /// The seed of the generator.
        seed: u64,
    },
    /// Bursts of `packets` packets `gap_ns` apart, with a pause after the last packet of a burst before the first of the
    /// next. The first burst starts at the start.
    Bursty {
        /// The payload of a packet.
        packet_size: u64,
        /// The packets in a burst.
        packets: u64,
        /// The time between two packets of a burst.
        gap_ns: u64,
        /// The pause after a burst, or its mean.
        pause_ns: u64,
        /// Whether the pauses are exact or drawn.
        distribution: Distribution,
        /// The seed of the generator.
        seed: u64,
    },
}

impl From<Stream> for Schedule {
    fn from(stream: Stream) -> Self {
        Self::Cbr(stream)
    }
}

impl Schedule {
    /// The payload of a packet in bytes.
    #[must_use]
    pub fn packet_size(&self) -> u64 {
        match self {
            Self::Cbr(stream) | Self::Poisson { stream, .. } | Self::OnOff { stream, .. } => {
                stream.packet_size
            }
            Self::Bursty { packet_size, .. } => *packet_size,
        }
    }

    /// The mean payload rate in bits per second, rounded down: the rate of a constant bit rate or Poisson source, the rate
    /// while on times the share of the time that is on, the payload of a burst over the time from one burst to the next
    /// (the pause is its mean when it is drawn).
    #[must_use]
    pub fn mean_bits_per_second(&self) -> u64 {
        match self {
            Self::Cbr(stream) | Self::Poisson { stream, .. } => stream.bits_per_second,
            Self::OnOff {
                stream,
                on_ns,
                off_ns,
                ..
            } => {
                let cycle = u128::from(*on_ns) + u128::from(*off_ns);
                if cycle == 0 {
                    return 0;
                }
                u64::try_from(u128::from(stream.bits_per_second) * u128::from(*on_ns) / cycle)
                    .unwrap_or(u64::MAX)
            }
            Self::Bursty {
                packet_size,
                packets,
                gap_ns,
                pause_ns,
                ..
            } => {
                let cycle = u128::from(packets.saturating_sub(1)) * u128::from(*gap_ns)
                    + u128::from(*pause_ns);
                if cycle == 0 {
                    return 0;
                }
                let bits = u128::from(*packets) * u128::from(*packet_size) * 8;
                u64::try_from(bits * 1_000_000_000 / cycle).unwrap_or(u64::MAX)
            }
        }
    }

    /// Whether the schedule can make packets at all: a rate and a size above zero; an on/off source that is on for some
    /// time; bursts with a packet in them and some time from one burst to the next. A schedule that is not well formed has
    /// no packets, and a sender refuses it.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        match self {
            Self::Cbr(stream) | Self::Poisson { stream, .. } => {
                stream.bits_per_second > 0 && stream.packet_size > 0
            }
            Self::OnOff {
                stream,
                on_ns,
                off_ns,
                ..
            } => {
                stream.bits_per_second > 0
                    && stream.packet_size > 0
                    && *on_ns > 0
                    && on_ns.checked_add(*off_ns).is_some()
            }
            Self::Bursty {
                packet_size,
                packets,
                gap_ns,
                pause_ns,
                ..
            } => {
                *packet_size > 0
                    && *packets > 0
                    && (packets - 1)
                        .checked_mul(*gap_ns)
                        .and_then(|burst| burst.checked_add(*pause_ns))
                        .is_some_and(|cycle| cycle > 0)
            }
        }
    }

    /// The moments, in nanoseconds from the start, of the packets that leave before `duration_ns`, in order. The first
    /// is packet 0. The iterator ends at the first moment that is not before the end, because the moments only grow.
    #[must_use]
    pub fn offsets(&self, duration_ns: u64) -> Offsets {
        Offsets::new(*self, duration_ns)
    }

    /// How many packets leave before `duration_ns`.
    #[must_use]
    pub fn packets_in(&self, duration_ns: u64) -> u64 {
        match self {
            Self::Cbr(stream) => stream.packets_in(duration_ns),
            _ => self.offsets(duration_ns).count() as u64,
        }
    }

    /// For each of the `moments`, which must be in ascending order, how many packets leave before it: the number of the
    /// first packet at or after the moment. One walk for all of them.
    #[must_use]
    pub fn packets_before_each(&self, moments: &[u64]) -> Vec<u64> {
        if let Self::Cbr(stream) = self {
            return moments.iter().map(|m| stream.packets_in(*m)).collect();
        }
        let Some(&last) = moments.last() else {
            return Vec::new();
        };
        let mut counts = Vec::with_capacity(moments.len());
        let mut passed = 0_u64;
        let mut offsets = self.offsets(last);
        let mut next = offsets.next();
        for moment in moments {
            while let Some(offset) = next {
                if offset >= *moment {
                    break;
                }
                passed += 1;
                next = offsets.next();
            }
            counts.push(passed);
        }
        counts
    }
}

/// The moments of a [`Schedule`], one by one.
#[derive(Debug, Clone)]
pub struct Offsets {
    duration_ns: u64,
    state: State,
}

#[derive(Debug, Clone)]
enum State {
    Cbr {
        stream: Stream,
        n: u64,
    },
    Poisson {
        rng: Xoshiro256PlusPlus,
        mean_gap_ns: f64,
        time: f64,
    },
    OnOff {
        stream: Stream,
        rng: Xoshiro256PlusPlus,
        on_ns: f64,
        off_ns: f64,
        distribution: Distribution,
        phase_start: f64,
        on_length: f64,
        i: u64,
    },
    Bursty {
        rng: Xoshiro256PlusPlus,
        packets: u64,
        gap_ns: u64,
        pause_ns: f64,
        distribution: Distribution,
        burst_start: f64,
        i: u64,
    },
}

/// A length for a phase or a pause: exact, or drawn with the given mean.
fn length(rng: &mut Xoshiro256PlusPlus, distribution: Distribution, mean: f64) -> f64 {
    match distribution {
        Distribution::Fixed => mean,
        Distribution::Exponential => rng.exponential(mean),
    }
}

impl Offsets {
    fn new(schedule: Schedule, duration_ns: u64) -> Self {
        // A schedule that cannot make a packet makes none, instead of walking for ever
        let duration_ns = if schedule.is_well_formed() {
            duration_ns
        } else {
            0
        };
        let state = match schedule {
            Schedule::Cbr(stream) => State::Cbr { stream, n: 0 },
            Schedule::Poisson { stream, seed } => State::Poisson {
                rng: Xoshiro256PlusPlus::from_seed(seed),
                // Nanoseconds between two packets at the mean rate; the numbers are far below 2^53
                mean_gap_ns: (stream.packet_size as f64) * 8.0 * 1e9
                    / (stream.bits_per_second as f64),
                time: 0.0,
            },
            Schedule::OnOff {
                stream,
                on_ns,
                off_ns,
                distribution,
                seed,
            } => {
                let mut rng = Xoshiro256PlusPlus::from_seed(seed);
                let on_ns = on_ns as f64;
                let on_length = length(&mut rng, distribution, on_ns);
                State::OnOff {
                    stream,
                    rng,
                    on_ns,
                    off_ns: off_ns as f64,
                    distribution,
                    phase_start: 0.0,
                    on_length,
                    i: 0,
                }
            }
            Schedule::Bursty {
                packets,
                gap_ns,
                pause_ns,
                distribution,
                seed,
                ..
            } => State::Bursty {
                rng: Xoshiro256PlusPlus::from_seed(seed),
                packets,
                gap_ns,
                pause_ns: pause_ns as f64,
                distribution,
                burst_start: 0.0,
                i: 0,
            },
        };
        Self { duration_ns, state }
    }
}

/// A moment as whole nanoseconds, rounded down; a number that is too large is the largest moment.
fn whole(nanoseconds: f64) -> u64 {
    nanoseconds as u64
}

impl Iterator for Offsets {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        let duration = self.duration_ns;
        match &mut self.state {
            State::Cbr { stream, n } => {
                let offset = stream.offset_ns(*n);
                if offset >= duration {
                    return None;
                }
                *n += 1;
                Some(offset)
            }
            State::Poisson {
                rng,
                mean_gap_ns,
                time,
            } => {
                *time += rng.exponential(*mean_gap_ns);
                let offset = whole(*time);
                (offset < duration).then_some(offset)
            }
            State::OnOff {
                stream,
                rng,
                on_ns,
                off_ns,
                distribution,
                phase_start,
                on_length,
                i,
            } => loop {
                let base = whole(*phase_start);
                if base >= duration {
                    return None;
                }
                let inside = stream.offset_ns(*i);
                if (inside as f64) < *on_length {
                    let offset = base.saturating_add(inside);
                    if offset >= duration {
                        return None;
                    }
                    *i += 1;
                    return Some(offset);
                }
                // The phase is over: the off phase, then the next on phase
                *phase_start += *on_length + length(rng, *distribution, *off_ns);
                *on_length = length(rng, *distribution, *on_ns);
                *i = 0;
            },
            State::Bursty {
                rng,
                packets,
                gap_ns,
                pause_ns,
                distribution,
                burst_start,
                i,
            } => loop {
                let base = whole(*burst_start);
                if base >= duration {
                    return None;
                }
                if *i < *packets {
                    let offset = base.saturating_add(*i * *gap_ns);
                    if offset >= duration {
                        return None;
                    }
                    *i += 1;
                    return Some(offset);
                }
                let burst = (*packets - 1) * *gap_ns;
                *burst_start += (burst as f64) + length(rng, *distribution, *pause_ns);
                *i = 0;
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn stream(bits_per_second: u64, packet_size: u64) -> Stream {
        Stream {
            bits_per_second,
            packet_size,
        }
    }

    #[test]
    fn an_even_interval_is_exact() {
        // 2 mbit with 1200 byte packets: 9,600 bits every 4.8 ms
        let s = stream(2_000_000, 1200);
        assert_eq!(s.offset_ns(0), 0);
        assert_eq!(s.offset_ns(1), 4_800_000);
        assert_eq!(s.offset_ns(1_000), 4_800_000_000);
    }

    #[test]
    fn an_uneven_interval_does_not_drift() {
        // 3 mbit with 1000 byte packets: 8,000 bits every 2.666... ms. Three packets are exactly 8 ms.
        let s = stream(3_000_000, 1000);
        assert_eq!(s.offset_ns(1), 2_666_666);
        assert_eq!(s.offset_ns(3), 8_000_000);
        assert_eq!(s.offset_ns(3_000_000), 8_000_000_000_000);
    }

    #[test]
    fn the_count_matches_the_offsets() {
        for (rate, size, duration) in [
            (2_000_000, 1200, 1_000_000_000),
            (3_000_000, 1000, 1_000_000_000),
            (1_000_000, 1472, 123_456_789),
            (800_000_000, 1000, 10_000_000),
            (1_000_000, 16, 5_000_000),
        ] {
            let s = stream(rate, size);
            let count = s.packets_in(duration);
            if count > 0 {
                assert!(
                    s.offset_ns(count - 1) < duration,
                    "{rate} {size} {duration}"
                );
            }
            assert!(s.offset_ns(count) >= duration, "{rate} {size} {duration}");
        }
    }

    #[test]
    fn a_duration_of_zero_has_no_packets() {
        assert_eq!(stream(1_000_000, 1000).packets_in(0), 0);
    }

    #[test]
    fn the_rate_of_the_payload_is_what_arrives() {
        // 1 mbit, 1000 byte payload: 125 packets in one second
        assert_eq!(stream(1_000_000, 1000).packets_in(1_000_000_000), 125);
    }

    const MBIT: u64 = 1_000_000;
    const SECOND: u64 = 1_000_000_000;

    fn poisson(seed: u64) -> Schedule {
        // 1 mbit with 125 byte packets: a mean gap of 1 ms
        Schedule::Poisson {
            stream: stream(MBIT, 125),
            seed,
        }
    }

    /// 8 mbit with 100 byte packets, a packet every 100 us while on, for `on_ns` and then off for 15 ms.
    fn on_off_for(on_ns: u64, distribution: Distribution, seed: u64) -> Schedule {
        Schedule::OnOff {
            stream: stream(8 * MBIT, 100),
            on_ns,
            off_ns: 15_000_000,
            distribution,
            seed,
        }
    }

    /// Bursts of `packets` packets of 100 bytes 100 us apart, with `pause_ns` after each.
    fn bursts_of(packets: u64, pause_ns: u64, distribution: Distribution, seed: u64) -> Schedule {
        Schedule::Bursty {
            packet_size: 100,
            packets,
            gap_ns: 100_000,
            pause_ns,
            distribution,
            seed,
        }
    }

    fn on_off(distribution: Distribution, seed: u64) -> Schedule {
        on_off_for(5_000_000, distribution, seed)
    }

    fn bursts(distribution: Distribution, seed: u64) -> Schedule {
        bursts_of(4, 2_000_000, distribution, seed)
    }

    fn all(schedule: &Schedule, duration_ns: u64) -> Vec<u64> {
        schedule.offsets(duration_ns).collect()
    }

    /// The pinned moments come from an independent implementation of the same walk in Python (the generator, the
    /// logarithm and the arithmetic written out again), so that a change of the walk is a change of these numbers.
    #[test]
    fn a_poisson_source_has_pinned_moments() {
        assert_eq!(
            all(&poisson(42), 10_000_000)[..6],
            [
                205_420, 1_348_545, 1_364_782, 1_719_836, 1_951_132, 2_481_993
            ]
        );
        assert_eq!(poisson(42).packets_in(SECOND), 1022);
    }

    #[test]
    fn an_on_off_source_with_exact_phases_is_a_constant_stream_in_bursts_of_time() {
        // 8 mbit with 100 bytes: every 100 us; on for 3 ms, off for 7 ms
        let schedule = Schedule::OnOff {
            stream: stream(8 * MBIT, 100),
            on_ns: 3_000_000,
            off_ns: 7_000_000,
            distribution: Distribution::Fixed,
            seed: 1,
        };
        let moments = all(&schedule, 25_000_000);
        let expected: Vec<u64> = [0, 10_000_000, 20_000_000]
            .iter()
            .flat_map(|start| (0..30).map(move |k| start + k * 100_000))
            .collect();
        assert_eq!(moments, expected);
        // The seed does not matter when nothing is drawn
        let other = Schedule::OnOff {
            stream: stream(8 * MBIT, 100),
            on_ns: 3_000_000,
            off_ns: 7_000_000,
            distribution: Distribution::Fixed,
            seed: 99,
        };
        assert_eq!(all(&other, 25_000_000), expected);
        assert_eq!(schedule.mean_bits_per_second(), 2_400_000);
    }

    #[test]
    fn an_on_off_source_with_drawn_phases_has_pinned_moments_and_the_mean_share_on() {
        let moments = all(&on_off(Distribution::Exponential, 42), 100_000_000);
        assert_eq!(
            moments[..12],
            [
                0, 100_000, 200_000, 300_000, 400_000, 500_000, 600_000, 700_000, 800_000, 900_000,
                1_000_000, 18_173_980
            ]
        );
        // The share of the time that is on is 5 / (5 + 15), and a packet leaves every 100 us in it; a phase of length L
        // holds ceil(L / 100 us) packets, so a cycle of 20 ms holds 50.5 on average: 2,525 packets a second. The packets
        // of 1,000 s: their standard deviation is sqrt(T / mean cycle * 2,835) = 3,765 for 100 s, so 11,900 for 1,000 s,
        // and four of them are 47,600.
        let total = on_off(Distribution::Exponential, 7).packets_in(1000 * SECOND);
        assert!(
            total.abs_diff(2_525_000) < 47_600,
            "{total} packets in 1,000 s"
        );
    }

    #[test]
    fn a_burst_source_sends_its_bursts_with_the_pause_after_the_last_packet() {
        let schedule = Schedule::Bursty {
            packet_size: 100,
            packets: 3,
            gap_ns: 100_000,
            pause_ns: 1_000_000,
            distribution: Distribution::Fixed,
            seed: 1,
        };
        assert_eq!(
            all(&schedule, 3_000_000),
            [
                0, 100_000, 200_000, 1_200_000, 1_300_000, 1_400_000, 2_400_000, 2_500_000,
                2_600_000
            ]
        );
        // 3 packets of 100 bytes every 1.2 ms
        assert_eq!(schedule.mean_bits_per_second(), 2_000_000);
        let drawn = all(&bursts(Distribution::Exponential, 42), 20_000_000);
        assert_eq!(
            drawn[..12],
            [
                0, 100_000, 200_000, 300_000, 710_840, 810_840, 910_840, 1_010_840, 3_297_090,
                3_397_090, 3_497_090, 3_597_090
            ]
        );
    }

    #[test]
    fn the_pauses_between_drawn_bursts_have_the_mean_that_was_asked_for() {
        let moments = all(&bursts(Distribution::Exponential, 3), 400 * SECOND);
        // Bursts of 4: the pause k is between packet 4k + 3 and packet 4k + 4
        let pauses: Vec<f64> = (4..moments.len())
            .step_by(4)
            .map(|next| (moments[next] - moments[next - 1]) as f64)
            .collect();
        let n = pauses.len() as f64;
        let mean = pauses.iter().sum::<f64>() / n;
        // 400 s hold about 130,000 bursts; the standard deviation of a pause is its mean, 2 ms, so the mean of N has
        // 2 ms / sqrt(N), 6.3 us here, and four of them are the bound
        assert!(n > 100_000.0, "{n} pauses");
        assert!(
            (mean - 2_000_000.0).abs() < 4.0 * 2e6 / n.sqrt(),
            "mean pause {mean} ns over {n}"
        );
    }

    #[test]
    fn the_gaps_of_a_poisson_source_are_exponential_and_its_counts_are_dispersed_like_a_poisson_count()
     {
        for seed in [1, 2, 3] {
            let moments = all(&poisson(seed), 100 * SECOND);
            // 100 s at 1,000 packets a second: 100,000 with a standard deviation of 316, four of them 1,265
            assert!(
                moments.len().abs_diff(100_000) < 1265,
                "seed {seed}: {}",
                moments.len()
            );
            // A gap is below its mean with the probability 1 - 1/e = 0.6321; the standard deviation of the share is 0.00153
            let below = moments
                .windows(2)
                .filter(|w| w[1] - w[0] < 1_000_000)
                .count();
            let share = below as f64 / (moments.len() - 1) as f64;
            assert!((share - 0.6321).abs() < 0.0062, "seed {seed}: {share}");
            // The packets in 1,000 windows of 100 ms have the variance of their mean (100) for a Poisson count; its own
            // standard error is 100 * sqrt(2 / 999) = 4.5, and four of them are 18
            let mut counts = vec![0_u32; 1000];
            for moment in &moments {
                counts[(*moment / 100_000_000) as usize] += 1;
            }
            let mean = counts.iter().map(|c| f64::from(*c)).sum::<f64>() / 1000.0;
            let variance = counts
                .iter()
                .map(|c| (f64::from(*c) - mean).powi(2))
                .sum::<f64>()
                / 999.0;
            assert!(
                (variance - 100.0).abs() < 18.0,
                "seed {seed}: variance {variance}"
            );
        }
        // Canary: a constant bit rate stream has no distribution at all, and the same judgement refuses it
        let cbr = all(&Schedule::Cbr(stream(MBIT, 125)), 100 * SECOND);
        let mut counts = vec![0_u32; 1000];
        for moment in &cbr {
            counts[(*moment / 100_000_000) as usize] += 1;
        }
        let mean = counts.iter().map(|c| f64::from(*c)).sum::<f64>() / 1000.0;
        let variance = counts
            .iter()
            .map(|c| (f64::from(*c) - mean).powi(2))
            .sum::<f64>()
            / 999.0;
        assert!((variance - 100.0).abs() >= 18.0, "{variance}");
    }

    #[test]
    fn the_same_seed_gives_the_same_moments_and_another_seed_gives_others() {
        for make in [
            |seed| poisson(seed),
            |seed| on_off(Distribution::Exponential, seed),
            |seed| bursts(Distribution::Exponential, seed),
        ] {
            assert_eq!(all(&make(5), SECOND), all(&make(5), SECOND));
            assert_ne!(all(&make(5), SECOND), all(&make(6), SECOND));
        }
    }

    #[test]
    fn the_moments_grow_and_stay_inside_the_duration() {
        let duration = 3 * SECOND;
        for schedule in [
            Schedule::Cbr(stream(MBIT, 125)),
            poisson(9),
            on_off(Distribution::Fixed, 9),
            on_off(Distribution::Exponential, 9),
            bursts(Distribution::Fixed, 9),
            bursts(Distribution::Exponential, 9),
        ] {
            let moments = all(&schedule, duration);
            assert!(moments.len() > 10, "{schedule:?}");
            assert!(moments.windows(2).all(|w| w[0] <= w[1]), "{schedule:?}");
            assert!(moments.iter().all(|m| *m < duration), "{schedule:?}");
            // A shorter duration is a prefix of a longer one: the walk does not depend on where it ends
            let shorter = all(&schedule, duration / 3);
            assert_eq!(moments[..shorter.len()], shorter[..], "{schedule:?}");
        }
    }

    #[test]
    fn counting_the_packets_before_moments_agrees_with_counting_the_moments() {
        let duration = 2 * SECOND;
        let moments_to_ask = [
            0,
            1,
            999_999,
            1_000_000,
            1_234_567,
            SECOND,
            duration,
            3 * SECOND,
        ];
        for schedule in [
            Schedule::Cbr(stream(MBIT, 125)),
            poisson(4),
            on_off(Distribution::Exponential, 4),
            bursts(Distribution::Fixed, 4),
        ] {
            let all_moments = all(&schedule, 4 * SECOND);
            let by_walk = schedule.packets_before_each(&moments_to_ask);
            let by_count: Vec<u64> = moments_to_ask
                .iter()
                .map(|m| all_moments.iter().filter(|o| *o < m).count() as u64)
                .collect();
            assert_eq!(by_walk, by_count, "{schedule:?}");
            assert_eq!(
                schedule.packets_in(duration),
                all_moments.iter().filter(|o| **o < duration).count() as u64
            );
        }
        assert_eq!(poisson(4).packets_before_each(&[]), Vec::<u64>::new());
    }

    #[test]
    fn a_schedule_that_cannot_make_a_packet_makes_none_instead_of_walking_for_ever() {
        let stopped = [
            on_off_for(0, Distribution::Fixed, 1),
            bursts_of(0, 2_000_000, Distribution::Fixed, 1),
            bursts_of(1, 0, Distribution::Fixed, 1),
            Schedule::Poisson {
                stream: stream(0, 125),
                seed: 1,
            },
        ];
        for schedule in stopped {
            assert!(!schedule.is_well_formed(), "{schedule:?}");
            assert_eq!(all(&schedule, SECOND), Vec::<u64>::new(), "{schedule:?}");
        }
        assert!(poisson(1).is_well_formed());
        assert!(bursts(Distribution::Exponential, 1).is_well_formed());
    }

    #[test]
    fn the_mean_rates_of_the_patterns() {
        assert_eq!(poisson(1).mean_bits_per_second(), MBIT);
        // 8 mbit for 5 ms in 20 ms
        assert_eq!(
            on_off(Distribution::Exponential, 1).mean_bits_per_second(),
            2 * MBIT
        );
        // 4 packets of 100 bytes (3,200 bits) every 0.3 ms + 2 ms
        assert_eq!(
            bursts(Distribution::Fixed, 1).mean_bits_per_second(),
            3_200 * 1_000_000_000 / 2_300_000
        );
    }
}
