//! The precision of the timers of this machine (ENV-05, D-097): how fine the clock is, how late a sleep wakes up, and how
//! busy the machine is while that is measured.
//!
//! The senders of a run wait for the slot of a packet by sleeping and then spinning for the last 150 microseconds, and a
//! timeline step waits for the clock the same way. How late a sleeping core wakes depends on the processor, its idle
//! states, the timer slack of the process and the load (D-015), so a run records what the machine does at its start and
//! never promises a figure. The lateness a run actually met is in `max_lateness_ns` of each sender and in the steps of
//! `events` (`applied_ns - scheduled_ns`).
//!
//! The measurement is made by the process that starts the run, just before it builds the network. It uses `std` only.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The sleeps that are measured, in nanoseconds: one that is about the stretch a sender sleeps for between two packets of a
/// fast stream, and one that is a millisecond, the grain of a slow one.
pub const SLEEPS_NS: [u64; 2] = [200_000, 1_000_000];

/// How late a wake-up may be before a sender, which spins for the last 150 microseconds, sends late (`sender.rs` of the
/// agent). A report calls a machine whose 99th percentile is above this one worth a warning.
pub const SPIN_MARGIN_NS: u64 = 150_000;

/// How much to measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    /// About 40 milliseconds: what every run records at its start. The 99th percentile of so few samples is close to the
    /// largest, which is why each entry says how many there were.
    Quick,
    /// About a second: `netscenario doctor --timers`.
    Thorough,
}

impl Effort {
    /// How many sleeps of each length of [`SLEEPS_NS`].
    const fn samples(self) -> [u32; 2] {
        match self {
            Self::Quick => [50, 20],
            Self::Thorough => [1000, 300],
        }
    }
}

/// The timers of a machine as measured at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Timers {
    /// The clock source of the kernel (`tsc`, `kvm-clock`, `hpet`, ...), if it could be read. `CLOCK_MONOTONIC` is read
    /// from it.
    pub clock_source: Option<String>,
    /// What one reading of `CLOCK_MONOTONIC` costs, in nanoseconds (the median of many).
    pub clock_read_ns: u64,
    /// The smallest difference seen between two different readings of `CLOCK_MONOTONIC`, in nanoseconds: the grain of the
    /// clock as a program sees it.
    pub clock_step_ns: u64,
    /// The timer slack of the process, in nanoseconds: how far the kernel may delay a timer to batch it with others (50
    /// microseconds unless it was changed), if it could be read.
    pub timer_slack_ns: Option<u64>,
    /// The governor of the first processor (`performance`, `powersave`, ...), if there is one. It is not the cause of every
    /// late wake-up, but a machine that clocks down while idle wakes later.
    pub cpu_governor: Option<String>,
    /// The load average of the last minute, in hundredths (`52` for 0.52), if it could be read. It is a whole number so
    /// that the provenance stays comparable.
    pub load_1m_hundredths: Option<u32>,
    /// The tasks that were runnable when the measurement began, as the kernel counts them: the process that measures is
    /// one of them.
    pub runnable_tasks: Option<u32>,
    /// How late sleeps woke, for each length in [`SLEEPS_NS`].
    pub sleeps: Vec<SleepLateness>,
}

/// The spread of a number of measurements, in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Spread {
    /// How many measurements.
    pub samples: u32,
    /// The median.
    pub p50_ns: u64,
    /// The 99th percentile (the nearest rank).
    pub p99_ns: u64,
    /// The largest.
    pub max_ns: u64,
}

impl Spread {
    /// Sums up `samples`. `None` if there are none.
    #[must_use]
    pub fn from_samples(samples: &[u64]) -> Option<Self> {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let last = sorted.len().checked_sub(1)?;
        // The nearest rank: the smallest value that at least 99 % of the samples do not exceed
        let rank_99 = (sorted.len() * 99).div_ceil(100).saturating_sub(1);
        Some(Self {
            samples: u32::try_from(sorted.len()).unwrap_or(u32::MAX),
            p50_ns: sorted[last / 2],
            p99_ns: sorted[rank_99.min(last)],
            max_ns: sorted[last],
        })
    }
}

/// How late sleeps of one length woke up: the time a sleep took beyond the length asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SleepLateness {
    /// The length that was asked for, in nanoseconds.
    pub asked_ns: u64,
    /// The lateness: how many sleeps, the median, the 99th percentile and the largest, all in nanoseconds.
    #[serde(flatten)]
    pub late: Spread,
}

/// Nanoseconds as microseconds with one digit (`59.0 us`).
#[must_use]
pub fn micros(ns: u64) -> String {
    format!("{}.{} us", ns / 1000, (ns % 1000) / 100)
}

/// A length in nanoseconds as a name: `200 us`, `1 ms`.
#[must_use]
pub fn length_name(ns: u64) -> String {
    if ns >= 1_000_000 {
        format!("{} ms", ns / 1_000_000)
    } else {
        format!("{} us", ns / 1000)
    }
}

impl Timers {
    /// One line for a report: the clock, how late sleeps woke and the load.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "clock {} ({} ns a reading)",
            self.clock_source.as_deref().unwrap_or("unknown"),
            self.clock_read_ns
        )];
        for sleep in &self.sleeps {
            parts.push(format!(
                "a sleep of {} wakes {} late (99th percentile of {})",
                length_name(sleep.asked_ns),
                micros(sleep.late.p99_ns),
                sleep.late.samples
            ));
        }
        if let Some(load) = self.load_1m_hundredths {
            parts.push(format!("load {}.{:02}", load / 100, load % 100));
        }
        parts.join("; ")
    }
}

/// Measures the timers of this machine. It sleeps for the time the samples take (see [`Effort`]).
#[must_use]
pub fn measure(effort: Effort) -> Timers {
    // The load is read first: it is the machine as it was before the measurement added its own
    let (load_1m_hundredths, runnable_tasks) = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|text| parse_loadavg(&text))
        .unzip();
    let (clock_read_ns, clock_step_ns) = clock_facts();
    let sleeps = SLEEPS_NS
        .iter()
        .zip(effort.samples())
        .filter_map(|(&asked_ns, count)| {
            let late: Vec<u64> = (0..count).map(|_| lateness_of_a_sleep(asked_ns)).collect();
            Spread::from_samples(&late).map(|late| SleepLateness { asked_ns, late })
        })
        .collect();
    Timers {
        clock_source: read_line("/sys/devices/system/clocksource/clocksource0/current_clocksource"),
        clock_read_ns,
        clock_step_ns,
        timer_slack_ns: read_line("/proc/self/timerslack_ns").and_then(|text| text.parse().ok()),
        cpu_governor: read_line("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        load_1m_hundredths,
        runnable_tasks,
        sleeps,
    }
}

/// How much longer than `asked_ns` one `thread::sleep` took.
fn lateness_of_a_sleep(asked_ns: u64) -> u64 {
    let asked = Duration::from_nanos(asked_ns);
    let begin = Instant::now();
    std::thread::sleep(asked);
    u64::try_from(begin.elapsed().saturating_sub(asked).as_nanos()).unwrap_or(u64::MAX)
}

/// The cost of a reading of the clock and the smallest step between two different readings, both in nanoseconds.
fn clock_facts() -> (u64, u64) {
    const READINGS: usize = 4000;
    let mut costs = Vec::with_capacity(READINGS);
    let mut smallest_step = u64::MAX;
    let mut previous = Instant::now();
    for _ in 0..READINGS {
        let now = Instant::now();
        let step = u64::try_from(now.duration_since(previous).as_nanos()).unwrap_or(u64::MAX);
        costs.push(step);
        if step > 0 {
            smallest_step = smallest_step.min(step);
        }
        previous = now;
    }
    costs.sort_unstable();
    let median = costs[costs.len() / 2];
    // A clock whose readings never differed has a grain beyond what was looked at
    let step = if smallest_step == u64::MAX {
        0
    } else {
        smallest_step
    };
    (median, step)
}

/// The first line of a file, trimmed, if the file can be read and is not empty.
fn read_line(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

/// The load of the last minute in hundredths and the runnable tasks, from the text of `/proc/loadavg`
/// (`0.52 0.41 0.30 2/1234 5678`).
#[must_use]
pub(crate) fn parse_loadavg(text: &str) -> Option<(u32, u32)> {
    let mut fields = text.split_whitespace();
    let load = fields.next()?;
    let runnable = fields.nth(2)?.split_once('/')?.0.parse().ok()?;
    let (whole, fraction) = load.split_once('.').unwrap_or((load, ""));
    let fraction = format!("{fraction:0<2}");
    let hundredths = whole
        .parse::<u32>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fraction.get(..2)?.parse().ok()?)?;
    Some((hundredths, runnable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_the_99th_percentile_and_the_largest_are_picked_by_rank() {
        let samples: Vec<u64> = (1..=100).rev().collect();
        let sums = Spread::from_samples(&samples).unwrap();
        assert_eq!((sums.p50_ns, sums.p99_ns, sums.max_ns), (50, 99, 100));
        assert_eq!(sums.samples, 100);
        // A single outlier among 50 is the 99th percentile (rank 50 of 50), not hidden below it
        let mut few = vec![10; 49];
        few.push(5000);
        let sums = Spread::from_samples(&few).unwrap();
        assert_eq!((sums.p50_ns, sums.p99_ns, sums.max_ns), (10, 5000, 5000));
        assert_eq!(Spread::from_samples(&[]), None);
        let one = Spread::from_samples(&[7]).unwrap();
        assert_eq!((one.p50_ns, one.p99_ns, one.max_ns), (7, 7, 7));
    }

    #[test]
    fn microseconds_are_cut_not_rounded_and_lengths_are_named() {
        assert_eq!(micros(59_049), "59.0 us");
        assert_eq!(micros(59_999), "59.9 us");
        assert_eq!(micros(0), "0.0 us");
        assert_eq!(length_name(200_000), "200 us");
        assert_eq!(length_name(1_000_000), "1 ms");
    }

    #[test]
    fn the_summary_names_the_clock_the_sleeps_and_the_load() {
        let timers: Timers = serde_json::from_value(serde_json::json!({
            "clock_source": "tsc", "clock_read_ns": 30, "clock_step_ns": 20,
            "timer_slack_ns": 50_000, "cpu_governor": null,
            "load_1m_hundredths": 52, "runnable_tasks": 2,
            "sleeps": [{"asked_ns": 200_000, "samples": 50, "p50_ns": 54_000, "p99_ns": 59_000, "max_ns": 64_000}],
        }))
        .unwrap();
        assert_eq!(
            timers.summary(),
            "clock tsc (30 ns a reading); a sleep of 200 us wakes 59.0 us late (99th percentile of 50); load 0.52"
        );
    }

    #[test]
    fn the_load_is_read_in_hundredths() {
        assert_eq!(parse_loadavg("0.52 0.41 0.30 2/1234 5678\n"), Some((52, 2)));
        assert_eq!(parse_loadavg("12.5 1 1 17/1 2"), Some((1250, 17)));
        assert_eq!(parse_loadavg("3 1 1 1/1 2"), Some((300, 1)));
        assert_eq!(parse_loadavg("0.05 0 0 1/1 2"), Some((5, 1)));
        assert_eq!(parse_loadavg(""), None);
        assert_eq!(parse_loadavg("x 1 1 1/1 2"), None);
        assert_eq!(parse_loadavg("0.5 1 1 many 2"), None);
    }

    /// The measurement itself: each sleep is at least as long as asked (so the lateness is no wrap-around of an unsigned
    /// number), the order of the summary holds, and the clock has a grain. Nothing here depends on how late a sleep is.
    #[test]
    fn a_quick_measurement_has_the_samples_asked_for_in_order() {
        let timers = measure(Effort::Quick);
        assert_eq!(timers.sleeps.len(), SLEEPS_NS.len());
        for (sleep, (asked, count)) in timers
            .sleeps
            .iter()
            .zip(SLEEPS_NS.into_iter().zip(Effort::Quick.samples()))
        {
            assert_eq!(sleep.asked_ns, asked);
            let late = sleep.late;
            assert_eq!(late.samples, count);
            assert!(late.p50_ns <= late.p99_ns && late.p99_ns <= late.max_ns);
            // A lateness beyond a minute is a subtraction that wrapped, not a machine
            assert!(late.max_ns < 60_000_000_000, "{sleep:?}");
        }
        assert!(timers.clock_read_ns < 1_000_000, "{timers:?}");
        assert!(timers.clock_step_ns > 0, "{timers:?}");
    }

    /// Canary: the lateness is measured. A sleep that is asked for 20 ms and measured against 200 microseconds is late by
    /// about 20 ms, and the summary shows it.
    #[test]
    fn a_sleep_that_took_longer_than_asked_shows_as_late() {
        let late = lateness_of_a_sleep(0);
        assert!(late < 60_000_000_000);
        let begin = Instant::now();
        std::thread::sleep(Duration::from_millis(20));
        let took = u64::try_from(begin.elapsed().as_nanos()).unwrap();
        let sums = Spread::from_samples(&[took - 200_000]).unwrap();
        assert!(sums.max_ns >= 19_000_000, "{sums:?}");
    }

    #[test]
    fn the_timers_survive_their_json() {
        let timers = measure(Effort::Quick);
        let again: Timers = serde_json::from_str(&serde_json::to_string(&timers).unwrap()).unwrap();
        assert_eq!(timers, again);
    }
}
