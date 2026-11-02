//! Confidence intervals over the runs of a set (ASR-03, D-074): the percentile bootstrap, seeded.
//!
//! A set of runs gives one value of a metric for each run. The interval says where the mean (or the median) of that
//! metric lies for the scenario, as far as `n` runs can tell: the values are drawn again and again with replacement
//! (`resamples` times, each draw as large as the set), the statistic is taken of each draw, and the interval is the stretch
//! that holds the middle `level` of those statistics.
//!
//! - **The random numbers** are those of the schedules ([`netscenario_agent::rng`]): published algorithms written out in
//!   the crate, so the same seed gives the same interval on every machine and in every version. An index below `n` is the
//!   top half of the product of a 64 bit output and `n`; its bias is below `n / 2^64`.
//! - **The interval** is `[s[k], s[B - 1 - k]]` of the sorted statistics `s` of the `B` draws, with `k = round(B * tail)`.
//!   `tail` is the probability that is left out at each end.
//! - **For the median** `tail` is `(1 - level) / 2`, the plain percentile interval: 250 of 10,000 draws at 95 %.
//! - **For the mean** the plain interval is too narrow for a small set, because the bootstrap draws from the values and not from
//!   the distribution, so a set of `n` values shows less spread than the distribution has by the factor `sqrt((n - 1) / n)`,
//!   and the values of a skewed metric are rarely the whole tail. The **expanded percentile** interval (Hesterberg, "What
//!   teachers should know about the bootstrap", 2015) corrects that: `tail` is the normal probability beyond `z = t * sqrt(n /
//!   (n - 1))`, where `t` is the critical value of Student's t with `n - 1` degrees of freedom for the level. For `n = 5` at 95 %
//!   that is `z = 3.10` and `tail = 0.00095`, for `n = 30` it is `0.0188`, for `n = 100` `0.0231` (instead of 0.025 at each). It needs no other source of random
//!   numbers. The module `special` (private) has the two functions.
//! - **The median** of an even number of values is the mean of the two in the middle.
//! - **The standard library** computes `exp` and `ln` for the tail; a machine whose maths library rounds the last bit differently
//!   could choose another index only if `B * tail` lay within 1e-9 of the middle between two integers.
//! - **It refuses what it cannot answer honestly**: fewer than [`MIN_RUNS`] values (five values of a skewed metric say too
//!   little to put an interval around), a value that is not a finite number, fewer than [`MIN_RESAMPLES`] draws, a level
//!   outside (0, 1).
//!
//! **What the interval does not promise.** It is an approximation: the share of intervals that hold the true value is below
//! `level` for a small set and a skewed metric. The tests of this module measure that share on simulated data with known
//! distributions, and the numbers are in `docs/STATISTICS.md`.

use netscenario_agent::rng::Xoshiro256PlusPlus;
use serde::{Deserialize, Serialize};

/// The fewest values an interval is computed from. Below it the library refuses (D-074).
pub const MIN_RUNS: usize = 5;
/// The most draws that are made when none is asked for.
pub const DEFAULT_RESAMPLES: u32 = 10_000;
/// The fewest draws: with fewer the two tails of 2.5 % hold a handful of values and the ends of the interval are noise.
pub const MIN_RESAMPLES: u32 = 1_000;
/// The level when none is asked for.
pub const DEFAULT_LEVEL: f64 = 0.95;

/// What the interval is around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Statistic {
    /// The mean of the values of the runs.
    Mean,
    /// The median of the values of the runs.
    Median,
}

impl Statistic {
    /// The word for the statistic in the result and on the command line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mean => "mean",
            Self::Median => "median",
        }
    }

    /// The statistic of `values`, which must not be empty. The median of an even number of values is the mean of the two in
    /// the middle.
    #[must_use]
    pub fn of(self, values: &[f64]) -> f64 {
        match self {
            Self::Mean => mean(values),
            Self::Median => {
                let mut sorted = values.to_vec();
                sorted.sort_by(f64::total_cmp);
                median_of_sorted(&sorted)
            }
        }
    }
}

impl std::str::FromStr for Statistic {
    type Err = String;

    fn from_str(word: &str) -> Result<Self, String> {
        match word {
            "mean" => Ok(Self::Mean),
            "median" => Ok(Self::Median),
            other => Err(format!("the statistic is mean or median, not '{other}'")),
        }
    }
}

/// How the ends were chosen from the sorted draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    /// The ends are the percentiles `(1 - level) / 2` and `1 - (1 - level) / 2` of the draws.
    Percentile,
    /// The expanded percentile interval of Hesterberg (2015): the percentiles are moved outwards by the small-sample
    /// correction of the module, the more so the fewer the runs.
    ExpandedPercentile,
}

impl Method {
    /// The method for a statistic: the expanded interval for the mean, the plain one for the median (whose measured coverage
    /// is near the level without it).
    #[must_use]
    pub const fn of(statistic: Statistic) -> Self {
        match statistic {
            Statistic::Mean => Self::ExpandedPercentile,
            Statistic::Median => Self::Percentile,
        }
    }
}

/// An interval around the statistic of the values of a set of runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Interval {
    /// What the interval is around.
    pub statistic: Statistic,
    /// The statistic of the values themselves (not of a draw).
    pub estimate: f64,
    /// The lower end.
    pub low: f64,
    /// The upper end.
    pub high: f64,
    /// The share of the draws that the interval is meant to hold, as a number from 0 to 1 (0.95 for "95 %").
    pub level: f64,
    /// How the ends were chosen.
    pub method: Method,
    /// The probability that is left out of the draws at each end. For `Percentile` it is `(1 - level) / 2`, and for
    /// `ExpandedPercentile` it is less, by the correction.
    pub tail: f64,
    /// How many draws were made.
    pub resamples: u32,
    /// How many values the interval is made from.
    pub runs: usize,
}

/// Why no interval was computed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum BootstrapError {
    /// There are fewer values than [`MIN_RUNS`].
    #[error(
        "an interval needs at least {MIN_RUNS} runs and there are {have}: with fewer the bootstrap says too little about the spread (use `repetitions` of {MIN_RUNS} or more)"
    )]
    TooFewRuns {
        /// How many values there are.
        have: usize,
    },
    /// A value is not a finite number.
    #[error("the value of run {run} (counted from 1) is not a finite number")]
    NotFinite {
        /// The run, counted from 1.
        run: usize,
    },
    /// The level is not between 0 and 1, both excluded.
    #[error("the level of an interval is a number between 0 and 1 (0.95 for 95 %), not {0}")]
    BadLevel(f64),
    /// There are fewer draws than [`MIN_RESAMPLES`].
    #[error("the bootstrap needs at least {MIN_RESAMPLES} draws, not {0}")]
    TooFewResamples(u32),
}

fn mean(values: &[f64]) -> f64 {
    // Counts of runs are far below 2^52, where a double is exact
    #[allow(clippy::cast_precision_loss)]
    let n = values.len() as f64;
    values.iter().sum::<f64>() / n
}

fn median_of_sorted(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        f64::midpoint(sorted[n / 2 - 1], sorted[n / 2])
    }
}

/// A number below `bound` (which is not 0), drawn without a division: the top 64 bits of the 128 bit product.
fn below(rng: &mut Xoshiro256PlusPlus, bound: usize) -> usize {
    let product = u128::from(rng.next_u64()) * (bound as u128);
    // The product is below bound * 2^64, so its top half is below bound
    usize::try_from(product >> 64).unwrap_or(0)
}

/// Checks the level and the number of draws, so that a set of runs can refuse a wrong one before it starts the first run.
///
/// # Errors
///
/// [`BootstrapError::BadLevel`] or [`BootstrapError::TooFewResamples`].
pub fn check_parameters(level: f64, resamples: u32) -> Result<(), BootstrapError> {
    if !(level > 0.0 && level < 1.0) {
        return Err(BootstrapError::BadLevel(level));
    }
    if resamples < MIN_RESAMPLES {
        return Err(BootstrapError::TooFewResamples(resamples));
    }
    Ok(())
}

/// The interval of the `statistic` of `values` at `level`, from `resamples` draws with the generator started at `seed`.
///
/// # Errors
///
/// See [`BootstrapError`]: too few values or draws, a value that is not finite, a level outside (0, 1).
pub fn confidence_interval(
    values: &[f64],
    statistic: Statistic,
    level: f64,
    resamples: u32,
    seed: u64,
) -> Result<Interval, BootstrapError> {
    if values.len() < MIN_RUNS {
        return Err(BootstrapError::TooFewRuns { have: values.len() });
    }
    if let Some(index) = values.iter().position(|value| !value.is_finite()) {
        return Err(BootstrapError::NotFinite { run: index + 1 });
    }
    check_parameters(level, resamples)?;
    let mut rng = Xoshiro256PlusPlus::from_seed(seed);
    let n = values.len();
    let mut draw = vec![0.0_f64; n];
    let mut statistics = Vec::with_capacity(resamples as usize);
    for _ in 0..resamples {
        for slot in &mut draw {
            *slot = values[below(&mut rng, n)];
        }
        statistics.push(statistic.of(&draw));
    }
    statistics.sort_by(f64::total_cmp);
    let method = Method::of(statistic);
    let tail = tail_of(method, n, level);
    let (low, high) = ends(&statistics, tail);
    Ok(Interval {
        statistic,
        estimate: statistic.of(values),
        low,
        high,
        level,
        method,
        tail,
        resamples,
        runs: n,
    })
}

/// The probability left out at each end for `n` values (see the module).
fn tail_of(method: Method, n: usize, level: f64) -> f64 {
    let plain = (1.0 - level) / 2.0;
    match method {
        Method::Percentile => plain,
        Method::ExpandedPercentile => {
            // Counts of runs are far below 2^52
            #[allow(clippy::cast_precision_loss)]
            let n = n as f64;
            let t = crate::special::t_upper_quantile(plain, n - 1.0);
            crate::special::normal_upper_tail(t * (n / (n - 1.0)).sqrt())
        }
    }
}

/// The ends of the sorted statistics: `s[k]` and `s[B - 1 - k]`, `k = round(B * tail)`.
fn ends(sorted: &[f64], tail: f64) -> (f64, f64) {
    let b = sorted.len();
    // The number of draws is below 2^32, and the product is a small non-negative number
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let k = ((b as f64) * tail).round() as usize;
    let k = k.min((b - 1) / 2);
    (sorted[k], sorted[b - 1 - k])
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the counts in the tests are small"
)]
mod tests {
    use super::*;

    /// A value from the exponential distribution with mean 1, a skewed one that the bootstrap finds hard.
    fn exponential_sample(rng: &mut Xoshiro256PlusPlus, n: usize) -> Vec<f64> {
        (0..n).map(|_| rng.exponential(1.0)).collect()
    }

    fn interval(values: &[f64], seed: u64) -> Interval {
        confidence_interval(values, Statistic::Mean, 0.95, 2_000, seed).unwrap()
    }

    #[test]
    fn the_mean_and_the_median_are_worked_out_by_hand() {
        assert!((Statistic::Mean.of(&[1.0, 2.0, 6.0]) - 3.0).abs() < 1e-12);
        // An odd number: the one in the middle, however the values are ordered
        assert!((Statistic::Median.of(&[9.0, 1.0, 4.0]) - 4.0).abs() < 1e-12);
        // An even number: the mean of the two in the middle
        assert!((Statistic::Median.of(&[4.0, 1.0, 9.0, 2.0]) - 3.0).abs() < 1e-12);
    }

    /// The index of an end is worked out for a few sizes, since a shifted index would make every interval wrong a little.
    #[test]
    fn the_ends_are_the_tails_of_the_sorted_statistics() {
        let sorted: Vec<f64> = (0..10_000).map(f64::from).collect();
        // 10,000 draws, 95 %: k = 250, so the 251st smallest and the 251st largest
        assert_eq!(ends(&sorted, 0.025), (250.0, 9_749.0));
        assert_eq!(ends(&sorted, 0.05), (500.0, 9_499.0));
        let thousand: Vec<f64> = (0..1_000).map(f64::from).collect();
        assert_eq!(ends(&thousand, 0.025), (25.0, 974.0));
        // A tail so large that the two ends would cross in the middle never does
        let (low, high) = ends(&thousand, 0.499_9);
        assert!(low <= high, "{low} {high}");
    }

    #[test]
    fn the_same_seed_gives_the_same_interval_and_another_seed_a_close_other_one() {
        let values = [3.1, 2.7, 4.4, 3.9, 2.2, 5.0, 3.3, 3.8];
        let first = interval(&values, 5);
        assert_eq!(first, interval(&values, 5));
        let other = interval(&values, 6);
        assert_ne!((first.low, first.high), (other.low, other.high));
        // The ends move by the noise of the draws only: far less than the width
        let width = first.high - first.low;
        assert!(
            (first.low - other.low).abs() < 0.2 * width,
            "{first:?} {other:?}"
        );
        assert!(
            (first.high - other.high).abs() < 0.2 * width,
            "{first:?} {other:?}"
        );
    }

    /// The interval holds the estimate, the ends lie inside the range of the values, and it records what it was made of.
    #[test]
    fn the_interval_holds_the_estimate_and_says_what_it_is() {
        let values = [3.1, 2.7, 4.4, 3.9, 2.2, 5.0, 3.3, 3.8];
        let found = interval(&values, 11);
        assert!(
            found.low <= found.estimate && found.estimate <= found.high,
            "{found:?}"
        );
        assert!(found.low >= 2.2 && found.high <= 5.0, "{found:?}");
        assert!((found.estimate - Statistic::Mean.of(&values)).abs() < 1e-12);
        assert_eq!((found.runs, found.resamples), (8, 2_000));
        assert!((found.level - 0.95).abs() < 1e-12);
        assert_eq!(found.statistic, Statistic::Mean);
        assert_eq!(found.method, Method::ExpandedPercentile);
        assert!(found.tail < 0.025, "{found:?}");
        let median = confidence_interval(&values, Statistic::Median, 0.95, 2_000, 11).unwrap();
        assert_eq!(median.statistic, Statistic::Median);
        assert_eq!(median.method, Method::Percentile);
        assert!((median.tail - 0.025).abs() < 1e-15, "{median:?}");
        // Sorted: 2.2 2.7 3.1 3.3 3.8 3.9 4.4 5.0, so the median is (3.3 + 3.8) / 2
        assert!((median.estimate - 3.55).abs() < 1e-12, "{median:?}");
    }

    /// The tails of the expanded interval, worked out from the tables of Student's t and of the normal distribution:
    /// `z = t(0.975; n - 1) * sqrt(n / (n - 1))` and the upper tail beyond it.
    ///
    /// - n = 5: 2.7764 * 1.1180 = 3.1041, tail 0.000954 (the table has 0.000968 at 3.10 and 0.000933 at 3.11)
    /// - n = 8: 2.3646 * 1.0690 = 2.5278, tail 0.00574
    /// - n = 30: 2.0452 * 1.0171 = 2.0802, tail 0.01875
    #[test]
    fn the_tail_of_the_expanded_interval_is_worked_out_by_hand() {
        for (n, expected) in [(5, 0.000_954), (8, 0.005_74), (30, 0.018_75)] {
            let found = tail_of(Method::ExpandedPercentile, n, 0.95);
            assert!(
                (found - expected).abs() <= 0.005 * expected,
                "n={n}: {found}"
            );
        }
        assert!((tail_of(Method::Percentile, 5, 0.95) - 0.025).abs() < 1e-15);
        // The correction fades with n: the tail grows towards the plain 0.025 and never passes it
        let tails: Vec<f64> = [5, 10, 20, 50, 100, 1_000]
            .iter()
            .map(|n| tail_of(Method::ExpandedPercentile, *n, 0.95))
            .collect();
        assert!(tails.windows(2).all(|pair| pair[0] < pair[1]), "{tails:?}");
        assert!(tails.iter().all(|tail| *tail < 0.025), "{tails:?}");
        assert!(tails[5] > 0.0245, "{tails:?}");
    }

    /// Values that are all the same have no spread: the interval is a point.
    #[test]
    fn equal_values_give_a_point() {
        let found = interval(&[2.5; 6], 1);
        assert_eq!((found.low, found.high), (2.5, 2.5));
    }

    /// Canary for the refusals: each is a case where an interval would be a made-up number.
    #[test]
    fn what_cannot_be_answered_is_refused() {
        let ok = [1.0, 2.0, 3.0, 4.0, 5.0];
        let ask = |values: &[f64], level: f64, resamples: u32| {
            confidence_interval(values, Statistic::Mean, level, resamples, 1)
        };
        assert!(ask(&ok, 0.95, 1_000).is_ok());
        assert_eq!(
            ask(&ok[..4], 0.95, 1_000),
            Err(BootstrapError::TooFewRuns { have: 4 })
        );
        assert_eq!(
            ask(&[], 0.95, 1_000),
            Err(BootstrapError::TooFewRuns { have: 0 })
        );
        assert_eq!(
            ask(&[1.0, 2.0, f64::NAN, 4.0, 5.0], 0.95, 1_000),
            Err(BootstrapError::NotFinite { run: 3 })
        );
        assert_eq!(
            ask(&[1.0, 2.0, 3.0, f64::INFINITY, 5.0], 0.95, 1_000),
            Err(BootstrapError::NotFinite { run: 4 })
        );
        for level in [0.0, 1.0, -0.5, 1.5, f64::NAN] {
            assert!(
                matches!(ask(&ok, level, 1_000), Err(BootstrapError::BadLevel(_))),
                "{level}"
            );
        }
        assert_eq!(
            ask(&ok, 0.95, 999),
            Err(BootstrapError::TooFewResamples(999))
        );
        let message = ask(&ok[..2], 0.95, 1_000).unwrap_err().to_string();
        assert!(
            message.contains("at least 5 runs and there are 2"),
            "{message}"
        );
    }

    /// The first intervals for fixed inputs are pinned, so that a change of the generator, of the index or of the ends cannot
    /// pass unseen: they would change every interval ever published.
    #[test]
    fn an_interval_for_fixed_inputs_is_pinned() {
        let values = [3.1, 2.7, 4.4, 3.9, 2.2, 5.0, 3.3, 3.8];
        let found = confidence_interval(&values, Statistic::Mean, 0.95, 10_000, 42).unwrap();
        assert!((found.low - PINNED_LOW).abs() < 1e-9, "{found:?}");
        assert!((found.high - PINNED_HIGH).abs() < 1e-9, "{found:?}");
    }

    const PINNED_LOW: f64 = 2.8125;
    const PINNED_HIGH: f64 = 4.3;

    /// An interval maker: the ends for the values of one trial, from a seed of that trial.
    type Maker = fn(&[f64], u64) -> (f64, f64);

    fn expanded_mean(values: &[f64], seed: u64) -> (f64, f64) {
        let found = confidence_interval(values, Statistic::Mean, 0.95, 1_000, seed).unwrap();
        (found.low, found.high)
    }

    fn plain_median(values: &[f64], seed: u64) -> (f64, f64) {
        let found = confidence_interval(values, Statistic::Median, 0.95, 1_000, seed).unwrap();
        (found.low, found.high)
    }

    /// The plain percentile interval of the mean, which this module used to be before the correction: for the canary.
    fn plain_mean(values: &[f64], seed: u64) -> (f64, f64) {
        let mut rng = Xoshiro256PlusPlus::from_seed(seed);
        let n = values.len();
        let mut means: Vec<f64> = (0..1_000)
            .map(|_| (0..n).map(|_| values[below(&mut rng, n)]).sum::<f64>() / n as f64)
            .collect();
        means.sort_by(f64::total_cmp);
        ends(&means, 0.025)
    }

    /// A wrong interval that is the point estimate: it holds the truth almost never.
    fn a_point(values: &[f64], _seed: u64) -> (f64, f64) {
        let mean = Statistic::Mean.of(values);
        (mean, mean)
    }

    /// An interval of level 50 % that is called one of 95 %.
    fn half_the_level(values: &[f64], seed: u64) -> (f64, f64) {
        let found = confidence_interval(values, Statistic::Mean, 0.5, 1_000, seed).unwrap();
        (found.low, found.high)
    }

    /// How many of `trials` intervals, each made by `maker` from `n` fresh values of a distribution, hold `truth`.
    fn covered(
        n: usize,
        trials: usize,
        truth: f64,
        seed: u64,
        draw: impl Fn(&mut Xoshiro256PlusPlus) -> f64,
        maker: Maker,
    ) -> usize {
        let mut rng = Xoshiro256PlusPlus::from_seed(seed);
        (0..trials)
            .filter(|trial| {
                let values: Vec<f64> = (0..n).map(|_| draw(&mut rng)).collect();
                let (low, high) = maker(&values, seed ^ (*trial as u64).wrapping_mul(0x9e37_79b9));
                low <= truth && truth <= high
            })
            .count()
    }

    const TRIALS: usize = 1_000;

    /// The bounds of a count of `TRIALS` intervals around the share `model` that the procedure holds the truth in: four
    /// standard deviations of a binomial each way (the run is seeded, so it cannot flake, but the bound is the one that a
    /// different seed would also keep with a probability above 0.99993).
    fn bounds(model: f64) -> (usize, usize) {
        let sigma = (model * (1.0 - model) / TRIALS as f64).sqrt();
        let share = |x: f64| (x * TRIALS as f64).round() as usize;
        (share(model - 4.0 * sigma), share(model + 4.0 * sigma))
    }

    /// The coverage of the expanded interval of the mean, measured on simulated data with known distributions.
    ///
    /// The model shares are measured with 20,000 trials for each (`measure_coverage`, below), so that their own uncertainty
    /// (below 0.003) is far under that of the 1,000 trials here:
    ///
    /// - a uniform distribution (mean 0.5), n = 20: 95.0 %
    /// - an exponential distribution (mean 1, skewed), n = 10: 90.6 %, and n = 5: 86.3 %
    ///
    /// The exponential one is the hard case, which is why its shares are below 95 %: the interval does not reach the level
    /// for a skewed metric from few runs (D-074, `docs/STATISTICS.md`). The bounds are for the measured share, not for
    /// 95 %, and a broken interval fails them for the reason in the canary below.
    #[test]
    fn the_interval_of_the_mean_covers_as_measured() {
        for (name, n, model, truth, draw) in [
            (
                "uniform",
                20,
                0.950,
                0.5,
                (|r: &mut Xoshiro256PlusPlus| r.unit()) as fn(&mut _) -> f64,
            ),
            (
                "exponential",
                10,
                0.906,
                1.0,
                |r: &mut Xoshiro256PlusPlus| r.exponential(1.0),
            ),
            (
                "exponential",
                5,
                0.863,
                1.0,
                |r: &mut Xoshiro256PlusPlus| r.exponential(1.0),
            ),
        ] {
            let count = covered(n, TRIALS, truth, 11 + n as u64, draw, expanded_mean);
            let (low, high) = bounds(model);
            assert!(
                (low..=high).contains(&count),
                "{name} n={n}: {count} of {TRIALS}, expected {low} to {high} (model {model})"
            );
        }
    }

    /// The median needs no correction: its plain interval holds the truth about 93 to 94 % of the time already (exponential
    /// distribution, median ln 2, n = 20: 93.7 % in 20,000 trials).
    #[test]
    fn the_interval_of_the_median_covers_as_measured() {
        let count = covered(
            20,
            TRIALS,
            std::f64::consts::LN_2,
            21,
            |r| r.exponential(1.0),
            plain_median,
        );
        let (low, high) = bounds(0.937);
        assert!(
            (low..=high).contains(&count),
            "{count} of {TRIALS}, expected {low} to {high}"
        );
    }

    /// Canary for the two tests above: each broken interval is outside the bounds of the case it breaks, for the reason that it
    /// is broken.
    ///
    /// - The plain percentile interval of the mean (no correction) holds the truth 78.1 % of the time for the exponential
    ///   distribution and n = 5 (20,000 trials), below the lower bound 0.819 of the corrected one: it is the correction that the
    ///   test shows.
    /// - A point is outside every interval but by chance, and an interval of level 50 % holds the truth about half of the time.
    #[test]
    fn broken_intervals_fail_the_coverage_bounds() {
        let exponential = |r: &mut Xoshiro256PlusPlus| r.exponential(1.0);
        let uniform = |r: &mut Xoshiro256PlusPlus| r.unit();
        let (low, high) = bounds(0.863);
        let plain = covered(5, TRIALS, 1.0, 16, exponential, plain_mean);
        assert!(
            plain < low,
            "{plain} against the lower bound {low} ({high})"
        );
        let (low, _) = bounds(0.950);
        for (name, maker) in [
            ("a point", a_point as Maker),
            ("half the level", half_the_level),
        ] {
            let count = covered(20, TRIALS, 0.5, 31, uniform, maker);
            assert!(count < low, "{name}: {count} against the lower bound {low}");
        }
    }

    /// The interval narrows with more runs. The expanded interval of the mean is the width of the classical t interval, `2 t(n - 1)
    /// s / sqrt(n)`, so for normal data the mean widths at 40 and 10 runs have the ratio `(2.0227 / 6.3246) / (2.2622 / 3.1623) =
    /// 0.447`. The exponential data are skewed and a small sample shows less spread than a large one (`E[s]` is about 0.88 at 10
    /// runs and 0.97 at 40), which moves the ratio to about `0.447 * 0.97 / 0.88 = 0.49`; 0.5017 was measured. The mean of 200
    /// widths has a relative standard deviation of about 3 % at 10 runs and 1.5 % at 40, so four standard deviations of the
    /// ratio are about 12 %, and the bound 0.40 to 0.60 holds them. An interval that did not narrow would give 1, one that
    /// narrowed with `1 / n` would give 0.25.
    #[test]
    fn the_interval_narrows_with_more_runs() {
        let mut rng = Xoshiro256PlusPlus::from_seed(77);
        let mut width_of = |n: usize| -> f64 {
            (0..200_u64)
                .map(|trial| {
                    let values = exponential_sample(&mut rng, n);
                    let (low, high) = expanded_mean(&values, 1_000 + trial);
                    high - low
                })
                .sum::<f64>()
                / 200.0
        };
        let (narrow, wide) = (width_of(40), width_of(10));
        let ratio = narrow / wide;
        assert!(
            (0.40..=0.60).contains(&ratio),
            "{narrow} / {wide} = {ratio}"
        );
    }

    #[test]
    #[ignore = "measures the coverage with 20,000 trials, to find the model shares of the tests above (run it with --release)"]
    fn measure_coverage() {
        for n in [5_usize, 10, 20, 30, 50] {
            let exp = covered(n, 20_000, 1.0, 1, |r| r.exponential(1.0), expanded_mean);
            let uni = covered(n, 20_000, 0.5, 2, Xoshiro256PlusPlus::unit, expanded_mean);
            let med = covered(
                n,
                20_000,
                std::f64::consts::LN_2,
                3,
                |r| r.exponential(1.0),
                plain_median,
            );
            let plain = covered(n, 20_000, 1.0, 4, |r| r.exponential(1.0), plain_mean);
            println!(
                "n={n}: exp mean {exp}, uniform mean {uni}, exp median {med}, exp mean without the correction {plain} of 20000"
            );
        }
    }

    /// The sample that the tests above use has the mean it is told to: the mean of 20,000 values has a standard deviation of
    /// 1 / sqrt(20,000) = 0.0071, and four of them are 0.03.
    #[test]
    fn the_sample_has_the_mean_it_is_told_to() {
        let mut rng = Xoshiro256PlusPlus::from_seed(9);
        let values = exponential_sample(&mut rng, 20_000);
        assert!((Statistic::Mean.of(&values) - 1.0).abs() < 0.03);
    }
}
