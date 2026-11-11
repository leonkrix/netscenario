//! Judging `expect` (ASR-07, D-094): the checks of a scenario against the metrics of a run or of a set of runs.
//!
//! The verdict is part of the result (`result.json`, `runset.json`), so the reports, the exit code of `run` and the pytest plugin
//! read one judgment and not three. A check that cannot be judged (the metric is not there) fails and says why.

use std::collections::BTreeMap;

use netscenario_model::{Bound, Check, Expect, MetricUnit, Scenario, Statistic};
use serde::{Deserialize, Serialize};

use crate::bootstrap::Statistic as IntervalAround;
use crate::error::Error;
use crate::runset::{Confidence, MetricSet, PartialMetric, RunSetOptions};

/// The level of the interval that the keys `ci95-low` and `ci95-high` name.
pub const EXPECT_LEVEL: f64 = 0.95;

/// What the checks of a scenario found: one line for each bound of each check, in the order of the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExpectResult {
    /// Whether every check holds.
    pub passed: bool,
    /// How many runs the verdict is made of: 1 for a verdict on one run, the size of the set otherwise.
    pub runs: u32,
    /// The checks.
    pub checks: Vec<CheckResult>,
}

impl ExpectResult {
    /// The checks that do not hold.
    pub fn failures(&self) -> impl Iterator<Item = &CheckResult> {
        self.checks.iter().filter(|check| !check.passed)
    }

    /// How many checks hold, and how many there are.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        (
            self.checks.iter().filter(|check| check.passed).count(),
            self.checks.len(),
        )
    }
}

/// One bound of one check, judged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CheckResult {
    /// The name of the check (the name of the file, or the metric with the statistic and the bound), made unique for the bound.
    pub name: String,
    /// The metric.
    pub metric: String,
    /// What of the metric is compared.
    pub statistic: Statistic,
    /// The unit the numbers are in.
    pub unit: MetricUnit,
    /// The bound as the file writes it: `< 150ms`.
    pub bound: String,
    /// The bound in the unit of the metric: seconds, bits per second, a ratio or a count.
    pub threshold: f64,
    /// The value of the statistic. Absent when the metric is not in the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<f64>,
    /// How far the value is on the good side of the bound; negative by how much it is missed. Absent with `actual`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    /// Whether the bound holds. A bound that cannot be judged does not.
    pub passed: bool,
    /// Why it could not be judged, when the metric is not in the result or not in every run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl CheckResult {
    /// The value of the statistic in the unit of the metric, or `-` when the metric was not there.
    #[must_use]
    pub fn actual_text(&self) -> String {
        self.actual
            .map_or_else(|| "-".to_owned(), |value| show_value(self.unit, value))
    }

    /// How the bound is kept or missed: `0.6% to spare`, `misses by 50ms`, or why it could not be judged.
    #[must_use]
    pub fn margin_text(&self) -> String {
        match self.margin {
            Some(margin) if self.passed => format!("{} to spare", show_value(self.unit, margin)),
            Some(0.0) => "on the bound, which the operator excludes".to_owned(),
            Some(margin) => format!("misses by {}", show_value(self.unit, -margin)),
            None => self
                .note
                .clone()
                .unwrap_or_else(|| "could not be judged".to_owned()),
        }
    }

    /// One line for a list: `delay.p99 max: 200ms, bound <= 150ms (misses by 50ms)`.
    #[must_use]
    pub fn line(&self) -> String {
        match self.margin {
            Some(_) => format!(
                "{}: {} ({}, {})",
                self.name,
                self.actual_text(),
                self.bound,
                self.margin_text()
            ),
            None => format!("{}: {} ({})", self.name, self.bound, self.margin_text()),
        }
    }
}

/// What `expect` asks of a way to run the scenario, or why it cannot be had: the number of runs, and whether it is a set.
///
/// # Errors
///
/// [`Error::Run`] when the file judges one run and a set is asked for, when the set is smaller than the file needs, or when the
/// set is not around the mean at the level of the keys `ci95-*`.
pub fn check_set_options(scenario: &Scenario, set: &RunSetOptions) -> Result<(), Error> {
    let Some(expect) = &scenario.expect else {
        return Ok(());
    };
    let wrong = |message: String| Error::Run {
        action: "judge the checks of the scenario".to_owned(),
        message,
    };
    let Some(needed) = expect.repetitions else {
        return Err(wrong(format!(
            "the checks of the scenario judge one run (`value`), and {} runs were asked for; take `--repetitions` away, or give `expect` a `repetitions` and statistics of a set",
            set.repetitions
        )));
    };
    if set.repetitions < needed {
        return Err(wrong(format!(
            "the checks need at least {needed} runs, and {} were asked for",
            set.repetitions
        )));
    }
    let uses_interval = expect
        .bounds()
        .any(|(_, statistic, _)| matches!(statistic, Statistic::Ci95Low | Statistic::Ci95High));
    if uses_interval
        && (set.statistic != IntervalAround::Mean || (set.level - EXPECT_LEVEL).abs() > 1e-12)
    {
        return Err(wrong(
            "the checks use `ci95-low` and `ci95-high`, the 95 % interval around the mean, and the set was asked for another interval"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The runs a set has to make for the scenario: what `expect` needs, and `None` when it needs no set.
#[must_use]
pub fn needed_repetitions(scenario: &Scenario) -> Option<u32> {
    scenario
        .expect
        .as_ref()
        .and_then(|expect| expect.repetitions)
}

/// Judges the checks of the scenario against the metrics of one run. `None` when the scenario has no checks, or when its checks
/// judge a set of runs.
#[must_use]
pub fn judge_run(scenario: &Scenario, metrics: &BTreeMap<String, f64>) -> Option<ExpectResult> {
    let expect = scenario.expect.as_ref().filter(|expect| !expect.is_set())?;
    let lines = judged(scenario, expect, |check, _| {
        match metrics.get(&check.metric) {
            Some(value) => Ok(*value),
            None => Err(format!(
                "the run has no '{}': {}",
                check.metric,
                why_missing(&check.metric)
            )),
        }
    });
    Some(verdict(lines, 1))
}

/// Judges the checks of the scenario against the metrics of a set of runs. `None` when the scenario has no checks, or when its
/// checks judge one run.
#[must_use]
pub fn judge_set(
    scenario: &Scenario,
    metrics: &[MetricSet],
    partial: &[PartialMetric],
    confidence: &Confidence,
    repetitions: u32,
) -> Option<ExpectResult> {
    let expect = scenario.expect.as_ref().filter(|expect| expect.is_set())?;
    let lines = judged(scenario, expect, |check, statistic| {
        let Some(set) = metrics.iter().find(|set| set.name == check.metric) else {
            return Err(
                match partial.iter().find(|partial| partial.name == check.metric) {
                    Some(partial) => format!(
                        "only {} of {} runs have '{}', so it has no {statistic} over the runs",
                        partial.present_in, partial.of, check.metric
                    ),
                    None => format!(
                        "no run has '{}': {}",
                        check.metric,
                        why_missing(&check.metric)
                    ),
                },
            );
        };
        match statistic {
            Statistic::Mean => Ok(set.mean),
            Statistic::Median => Ok(set.median),
            Statistic::Min => Ok(set.min),
            Statistic::Max => Ok(set.max),
            Statistic::Ci95Low | Statistic::Ci95High
                if confidence.statistic != IntervalAround::Mean
                    || (confidence.level - EXPECT_LEVEL).abs() > 1e-12 =>
            {
                Err(
                    "the set was made with another interval than the 95 % interval around the mean"
                        .to_owned(),
                )
            }
            Statistic::Ci95Low => Ok(set.interval.low),
            Statistic::Ci95High => Ok(set.interval.high),
            Statistic::Value => Err("`value` is the value of one run".to_owned()),
            _ => Err("this statistic is not known to this version".to_owned()),
        }
    });
    Some(verdict(lines, repetitions))
}

/// What a metric that is not in a result usually means, for the note.
fn why_missing(metric: &str) -> &'static str {
    match metric.split('.').rev().nth(1) {
        Some("delay" | "jitter" | "round_trip" | "latency") => {
            "no packet or response arrived, so there is no delay to judge"
        }
        Some("rtt") => "the kernel gave no samples of the round trip time",
        _ if metric.ends_with(".completion") => "no connection did its work",
        _ => "the workload did not report it",
    }
}

/// Judges every bound with the `value` that `find` gives for the check and statistic.
fn judged(
    scenario: &Scenario,
    expect: &Expect,
    find: impl Fn(&Check, Statistic) -> Result<f64, String>,
) -> Vec<CheckResult> {
    let mut lines = Vec::new();
    for (check, statistic, bound) in expect.bounds() {
        let unit = scenario
            .metric_unit_of(&check.metric)
            .unwrap_or(MetricUnit::Number);
        // The model refused a bound that is not of the kind of its metric, so this is a scenario built in code that was not
        // validated: the check fails with that, and does not guess
        let threshold = bound.threshold(unit);
        let name = check.label(statistic, bound);
        let mut line = CheckResult {
            name,
            metric: check.metric.clone(),
            statistic,
            unit,
            bound: bound.to_string(),
            threshold: threshold.as_ref().copied().unwrap_or(f64::NAN),
            actual: None,
            margin: None,
            passed: false,
            note: None,
        };
        match (threshold, find(check, statistic)) {
            (Ok(threshold), Ok(actual)) => judge_one(&mut line, bound, threshold, actual),
            (Err(message), _) | (_, Err(message)) => line.note = Some(message),
        }
        lines.push(line);
    }
    lines
}

fn judge_one(line: &mut CheckResult, bound: &Bound, threshold: f64, actual: f64) {
    line.actual = Some(actual);
    line.margin = Some(bound.op().margin(actual, threshold));
    line.passed = bound.op().holds(actual, threshold);
}

fn verdict(checks: Vec<CheckResult>, runs: u32) -> ExpectResult {
    ExpectResult {
        passed: checks.iter().all(|check| check.passed),
        runs,
        checks,
    }
}

/// A value of a check in the unit it is written in: `0.150` seconds as `150ms`, a ratio as a percentage, a rate as `8.2mbit`.
#[must_use]
pub fn show_value(unit: MetricUnit, value: f64) -> String {
    if !value.is_finite() {
        return "n/a".to_owned();
    }
    match unit {
        MetricUnit::Seconds => {
            let magnitude = value.abs();
            if magnitude >= 1.0 || magnitude == 0.0 {
                format!("{}s", significant(value))
            } else if magnitude >= 1e-3 {
                format!("{}ms", significant(value * 1e3))
            } else if magnitude >= 1e-6 {
                format!("{}us", significant(value * 1e6))
            } else {
                format!("{}ns", significant(value * 1e9))
            }
        }
        MetricUnit::BitsPerSecond => {
            let magnitude = value.abs();
            if magnitude >= 1e9 {
                format!("{}gbit", significant(value / 1e9))
            } else if magnitude >= 1e6 {
                format!("{}mbit", significant(value / 1e6))
            } else if magnitude >= 1e3 {
                format!("{}kbit", significant(value / 1e3))
            } else {
                format!("{}bit", significant(value))
            }
        }
        MetricUnit::Ratio => format!("{}%", significant(value * 100.0)),
        _ => significant(value),
    }
}

/// Four significant digits, without trailing zeros.
fn significant(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let magnitude = value.abs().log10().floor();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let decimals = (3.0 - magnitude).clamp(0.0, 9.0) as usize;
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runset::{MetricSet, aggregate};

    fn scenario(expect: &str) -> Scenario {
        Scenario::from_yaml(
            &format!(
                "version: 0\nname: t\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {{ delay: 20ms }}\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n{expect}"
            ),
            "t.yaml",
        )
        .unwrap()
    }

    fn metrics(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect()
    }

    #[test]
    fn a_bound_that_holds_passes_with_its_margin_and_one_that_does_not_fails_by_it() {
        let scenario = scenario(
            "expect:\n  checks:\n    - { name: loss, metric: w.loss_ratio, value: \"< 1%\" }\n    - { name: delay, metric: w.delay.p99, value: \"<= 150ms\" }\n    - { name: rate, metric: w.throughput.mean, value: \">= 1mbit\" }\n",
        );
        let verdict = judge_run(
            &scenario,
            &metrics(&[
                ("w.loss_ratio", 0.004),
                ("w.delay.p99", 0.2),
                ("w.throughput.mean", 1.2e6),
            ]),
        )
        .unwrap();
        assert!(!verdict.passed);
        assert_eq!(verdict.counts(), (2, 3));
        let by = |name: &str| verdict.checks.iter().find(|c| c.name == name).unwrap();
        assert!(by("loss").passed);
        assert!((by("loss").margin.unwrap() - 0.006).abs() < 1e-12);
        assert!(!by("delay").passed);
        assert!(
            (by("delay").margin.unwrap() + 0.05).abs() < 1e-12,
            "{:?}",
            by("delay")
        );
        assert_eq!(by("delay").bound, "<= 150ms");
        assert!(by("rate").passed);
        assert_eq!(verdict.failures().count(), 1);
    }

    /// The edge of a bound: `<` fails on equality, `<=` holds.
    #[test]
    fn the_edge_belongs_to_the_inclusive_operators() {
        let scenario = scenario(
            "expect:\n  checks:\n    - { name: a, metric: w.sent, value: \"< 10\" }\n    - { name: b, metric: w.sent, value: \"<= 10\" }\n    - { name: c, metric: w.sent, value: \"> 10\" }\n    - { name: d, metric: w.sent, value: \">= 10\" }\n",
        );
        let verdict = judge_run(&scenario, &metrics(&[("w.sent", 10.0)])).unwrap();
        let held: Vec<bool> = verdict.checks.iter().map(|c| c.passed).collect();
        assert_eq!(held, [false, true, false, true]);
    }

    /// Canary: a claim that cannot be checked is not a claim that holds. A stream that lost every packet has no delay, and
    /// `delay.p99 < 150ms` must not pass on that.
    #[test]
    fn a_metric_that_is_not_in_the_result_fails_the_check_with_the_reason() {
        let scenario =
            scenario("expect:\n  checks:\n    - { metric: w.delay.p99, value: \"< 150ms\" }\n");
        let verdict = judge_run(&scenario, &metrics(&[("w.sent", 10.0)])).unwrap();
        assert!(!verdict.passed);
        let line = &verdict.checks[0];
        assert!(line.actual.is_none() && line.margin.is_none());
        assert!(
            line.note
                .as_deref()
                .unwrap()
                .contains("no packet or response arrived"),
            "{line:?}"
        );
    }

    #[test]
    fn a_scenario_without_checks_or_with_checks_of_the_other_kind_has_no_verdict() {
        assert!(judge_run(&scenario(""), &metrics(&[])).is_none());
        let set = scenario(
            "expect:\n  repetitions: 5\n  checks:\n    - { metric: w.sent, mean: \"> 1\" }\n",
        );
        assert!(judge_run(&set, &metrics(&[("w.sent", 3.0)])).is_none());
        let one = scenario("expect:\n  checks:\n    - { metric: w.sent, value: \"> 1\" }\n");
        let confidence = Confidence {
            statistic: IntervalAround::Mean,
            level: 0.95,
            resamples: 10_000,
            method: crate::bootstrap::Method::of(IntervalAround::Mean),
        };
        assert!(judge_set(&one, &[], &[], &confidence, 5).is_none());
    }

    fn set_of(values: &[f64]) -> (Vec<MetricSet>, Confidence) {
        use crate::result::tests::example;
        let results: Vec<_> = values
            .iter()
            .map(|loss| {
                let mut result = example();
                result.workloads[0].id = "w".to_owned();
                result.workloads[0].stats.loss_ratio = *loss;
                result
            })
            .collect();
        let options = RunSetOptions::new(u32::try_from(values.len()).unwrap());
        let (metrics, _) = aggregate(&results, 7, &options).unwrap();
        let confidence = Confidence {
            statistic: options.statistic,
            level: options.level,
            resamples: options.resamples,
            method: crate::bootstrap::Method::of(options.statistic),
        };
        (metrics, confidence)
    }

    /// Canary: a false claim about the interval fails and a true one passes, for the same set, and the ends are the ends of the
    /// interval of the set.
    #[test]
    fn the_interval_of_a_set_is_judged_with_its_ends() {
        let (metrics, confidence) = set_of(&[0.01, 0.012, 0.009, 0.011, 0.01, 0.013, 0.008, 0.01]);
        let set = metrics
            .iter()
            .find(|set| set.name == "w.loss_ratio")
            .unwrap();
        let loose = scenario(
            "expect:\n  repetitions: 8\n  checks:\n    - { name: high, metric: w.loss_ratio, ci95-high: \"< 5%\", ci95-low: \"> 0.5%\", mean: \"< 2%\", max: \"<= 1.3%\" }\n",
        );
        let verdict = judge_set(&loose, &metrics, &[], &confidence, 8).unwrap();
        assert!(verdict.passed, "{verdict:?}");
        assert_eq!(verdict.runs, 8);
        let high = verdict
            .checks
            .iter()
            .find(|check| check.statistic == Statistic::Ci95High)
            .unwrap();
        assert!((high.actual.unwrap() - set.interval.high).abs() < 1e-15);
        // The same set against a threshold below the mean: the claim is false and fails
        let tight = scenario(
            "expect:\n  repetitions: 8\n  checks:\n    - { name: high, metric: w.loss_ratio, ci95-high: \"< 1%\" }\n",
        );
        let verdict = judge_set(&tight, &metrics, &[], &confidence, 8).unwrap();
        assert!(!verdict.passed);
        assert!(verdict.checks[0].margin.unwrap() < 0.0);
    }

    #[test]
    fn a_metric_that_only_some_runs_have_fails_in_a_set() {
        let (metrics, confidence) = set_of(&[0.01; 5]);
        let scenario = scenario(
            "expect:\n  repetitions: 5\n  checks:\n    - { metric: w.delay.p99, max: \"< 1s\" }\n",
        );
        let partial = [PartialMetric {
            name: "w.delay.p99".to_owned(),
            present_in: 3,
            of: 5,
        }];
        let verdict = judge_set(&scenario, &metrics, &partial, &confidence, 5).unwrap();
        assert!(!verdict.passed);
        assert!(
            verdict.checks[0]
                .note
                .as_deref()
                .unwrap()
                .contains("only 3 of 5 runs")
        );
    }

    #[test]
    fn what_the_file_asks_of_the_set_is_checked_before_the_first_run() {
        let one = scenario("expect:\n  checks:\n    - { metric: w.sent, value: \"> 1\" }\n");
        let needs_ten = scenario(
            "expect:\n  repetitions: 10\n  checks:\n    - { metric: w.loss_ratio, ci95-high: \"< 5%\" }\n",
        );
        let none = scenario("");
        assert!(check_set_options(&none, &RunSetOptions::new(5)).is_ok());
        let error = check_set_options(&one, &RunSetOptions::new(5))
            .unwrap_err()
            .to_string();
        assert!(error.contains("judge one run"), "{error}");
        let error = check_set_options(&needs_ten, &RunSetOptions::new(9))
            .unwrap_err()
            .to_string();
        assert!(error.contains("at least 10 runs"), "{error}");
        assert!(check_set_options(&needs_ten, &RunSetOptions::new(10)).is_ok());
        assert!(check_set_options(&needs_ten, &RunSetOptions::new(40)).is_ok());
        let mut median = RunSetOptions::new(10);
        median.statistic = IntervalAround::Median;
        let error = check_set_options(&needs_ten, &median)
            .unwrap_err()
            .to_string();
        assert!(error.contains("another interval"), "{error}");
        let mut level = RunSetOptions::new(10);
        level.level = 0.9;
        assert!(check_set_options(&needs_ten, &level).is_err());
        assert_eq!(needed_repetitions(&needs_ten), Some(10));
        assert_eq!(needed_repetitions(&one), None);
    }

    #[test]
    fn values_are_shown_in_the_unit_of_their_metric() {
        assert_eq!(show_value(MetricUnit::Seconds, 0.15), "150ms");
        assert_eq!(show_value(MetricUnit::Seconds, 0.000_123_4), "123.4us");
        assert_eq!(show_value(MetricUnit::Seconds, 2.5), "2.5s");
        assert_eq!(show_value(MetricUnit::Seconds, 0.0), "0s");
        assert_eq!(
            show_value(MetricUnit::BitsPerSecond, 8_190_000.0),
            "8.19mbit"
        );
        assert_eq!(show_value(MetricUnit::Ratio, 0.0123), "1.23%");
        assert_eq!(show_value(MetricUnit::Number, 1200.0), "1200");
        assert_eq!(show_value(MetricUnit::Number, 0.5), "0.5");
        assert_eq!(show_value(MetricUnit::Ratio, f64::NAN), "n/a");
    }

    /// The median and the smallest of a set are judged with their own values, not with the mean or the largest. The mutation test
    /// found both statistics unchecked: a set of eight losses with an outlier tells them apart.
    #[test]
    fn the_median_and_the_minimum_of_a_set_are_judged_with_their_own_values() {
        let (metrics, confidence) = set_of(&[0.01, 0.01, 0.01, 0.01, 0.02, 0.02, 0.02, 0.09]);
        let set = metrics
            .iter()
            .find(|set| set.name == "w.loss_ratio")
            .unwrap();
        assert!(set.min < set.median && set.median < set.mean && set.mean < set.max);
        let scenario = scenario(
            "expect:\n  repetitions: 8\n  checks:\n    - { name: m, metric: w.loss_ratio, median: \"< 2%\", min: \">= 1%\", mean: \"> 2%\", max: \"<= 9%\" }\n",
        );
        let verdict = judge_set(&scenario, &metrics, &[], &confidence, 8).unwrap();
        assert!(verdict.passed, "{verdict:?}");
        let value_of = |statistic| {
            verdict
                .checks
                .iter()
                .find(|check| check.statistic == statistic)
                .unwrap()
                .actual
                .unwrap()
        };
        assert!((value_of(Statistic::Median) - set.median).abs() < 1e-15);
        assert!((value_of(Statistic::Min) - set.min).abs() < 1e-15);
        assert!((value_of(Statistic::Mean) - set.mean).abs() < 1e-15);
        assert!((value_of(Statistic::Max) - set.max).abs() < 1e-15);
    }

    /// Canary: the interval of a set that was made another way than the 95 % interval around the mean is not judged, whichever of the
    /// two differs, and the check fails with the reason.
    #[test]
    fn an_interval_of_another_level_or_around_the_median_is_not_judged_as_the_95_interval() {
        let (metrics, confidence) = set_of(&[0.01; 8]);
        let scenario = scenario(
            "expect:\n  repetitions: 8\n  checks:\n    - { metric: w.loss_ratio, ci95-high: \"< 5%\", ci95-low: \">= 0%\" }\n",
        );
        assert!(
            judge_set(&scenario, &metrics, &[], &confidence, 8)
                .unwrap()
                .passed
        );
        let other_level = Confidence {
            level: 0.9,
            ..confidence.clone()
        };
        let other_statistic = Confidence {
            statistic: IntervalAround::Median,
            ..confidence
        };
        for (name, other) in [("level", other_level), ("statistic", other_statistic)] {
            let verdict = judge_set(&scenario, &metrics, &[], &other, 8).unwrap();
            assert!(!verdict.passed, "{name}");
            assert_eq!(verdict.failures().count(), 2, "{name}: both ends");
            for check in verdict.failures() {
                assert!(
                    check
                        .note
                        .as_deref()
                        .unwrap()
                        .contains("another interval than the 95 %"),
                    "{name}: {check:?}"
                );
            }
        }
    }

    /// What a missing metric usually means is said for the metrics where that is known, and not for the others.
    #[test]
    fn a_missing_metric_says_what_it_usually_means() {
        assert_eq!(
            why_missing("w.delay.p99"),
            "no packet or response arrived, so there is no delay to judge"
        );
        assert_eq!(
            why_missing("f.rtt.p50"),
            "the kernel gave no samples of the round trip time"
        );
        assert_eq!(why_missing("f.completion"), "no connection did its work");
        assert_eq!(
            why_missing("w.loss_ratio"),
            "the workload did not report it"
        );
        // `completion` is a name only at the end: a delay of a workload with that name is a delay
        assert_eq!(
            why_missing("completion.delay.p99"),
            "no packet or response arrived, so there is no delay to judge"
        );
    }

    /// The edges of the units in which a value is shown: the value that is exactly one unit is shown in that unit.
    #[test]
    fn a_value_at_the_edge_of_a_unit_is_shown_in_that_unit() {
        let shown = [
            (MetricUnit::Seconds, 1.0, "1s"),
            (MetricUnit::Seconds, 0.001, "1ms"),
            (MetricUnit::Seconds, 0.000_001, "1us"),
            (MetricUnit::Seconds, 0.999, "999ms"),
            (MetricUnit::BitsPerSecond, 1e9, "1gbit"),
            (MetricUnit::BitsPerSecond, 1e6, "1mbit"),
            (MetricUnit::BitsPerSecond, 1e3, "1kbit"),
            (MetricUnit::BitsPerSecond, 999.0, "999bit"),
            (MetricUnit::BitsPerSecond, 999_000.0, "999kbit"),
        ];
        for (unit, value, text) in shown {
            assert_eq!(show_value(unit, value), text, "{value}");
        }
    }

    /// Four significant digits, whatever the size of the number: a large one has no decimals, a small one has as many as it needs.
    #[test]
    fn a_value_is_shown_with_four_significant_digits() {
        assert_eq!(show_value(MetricUnit::Number, 1234.5678), "1235");
        assert_eq!(show_value(MetricUnit::Number, 123.456_78), "123.5");
        assert_eq!(show_value(MetricUnit::Number, 12.3456), "12.35");
        assert_eq!(show_value(MetricUnit::Number, 1.234_56), "1.235");
        assert_eq!(show_value(MetricUnit::Number, 0.123_456), "0.1235");
        assert_eq!(show_value(MetricUnit::Number, 0.012_345_6), "0.01235");
        assert_eq!(show_value(MetricUnit::Number, 100.0), "100");
        assert_eq!(show_value(MetricUnit::Number, -12.3456), "-12.35");
    }
}
