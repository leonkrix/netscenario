//! `expect`: what a scenario must show (ASR-07, D-094). A list of named checks on the metrics of a run or of a set of runs, each
//! with bounds written as `"< 150ms"`. The core judges them when the run is over; this module holds the types, the names of the
//! metrics a scenario can report with their units, and the reading of a bound.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::decimal;
use crate::error::Problem;
use crate::scenario::{Scenario, Workload};
use crate::suggest::closest;
use crate::units::{Dimension, read_quantity};

/// The fewest runs of a set (an interval from fewer says too little about the spread) and the most (the bounds of a set of runs).
pub const MIN_EXPECT_REPETITIONS: u32 = 5;
/// The most repetitions that `expect` may ask for.
pub const MAX_EXPECT_REPETITIONS: u32 = 1_000;
/// The longest name of a check, in characters.
pub const MAX_CHECK_NAME: usize = 120;
/// The most checks in a file.
pub const MAX_CHECKS: usize = 200;

/// What a scenario must show: the checks, and how many runs they need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Expect {
    /// How many runs the checks need, from 5 to 1,000. Without it the file is judged by one run and its checks use `value`; with
    /// it the file is judged by a set of that many runs (at least: a run may ask for more) and its checks use the statistics of a
    /// set. `netscenario run` makes that many runs on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repetitions: Option<u32>,
    /// The checks. At least one.
    pub checks: Vec<Check>,
}

impl Expect {
    /// Whether the file is judged by a set of runs.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.repetitions.is_some()
    }

    /// Every bound of every check, in the order of the file: the check, the statistic, the bound.
    pub fn bounds(&self) -> impl Iterator<Item = (&Check, Statistic, &Bound)> {
        self.checks.iter().flat_map(|check| {
            check
                .bounds()
                .map(move |(statistic, bound)| (check, statistic, bound))
        })
    }
}

/// One check: a metric and the bounds that a statistic of it must keep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Check {
    /// What the check is called in the reports. Default: the metric, the statistic and the bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The metric, as STATISTICS.md names them: `<workload>.delay.p99`, `<workload>.loss_ratio`, `<tcp>.goodput.mean`.
    pub metric: String,
    /// The value of the metric in the run (a file without `repetitions`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Bound>,
    /// The mean of the metric over the runs of the set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<Bound>,
    /// The median of the metric over the runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median: Option<Bound>,
    /// The smallest value of the runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Bound>,
    /// The largest value of the runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Bound>,
    /// The lower end of the 95 % interval around the mean (D-074).
    #[serde(default, rename = "ci95-low", skip_serializing_if = "Option::is_none")]
    pub ci95_low: Option<Bound>,
    /// The upper end of the 95 % interval around the mean.
    #[serde(default, rename = "ci95-high", skip_serializing_if = "Option::is_none")]
    pub ci95_high: Option<Bound>,
}

impl Check {
    /// A check of `metric` without a bound yet; set the field of a statistic on the result.
    #[must_use]
    pub fn new(metric: impl Into<String>) -> Self {
        Self {
            name: None,
            metric: metric.into(),
            value: None,
            mean: None,
            median: None,
            min: None,
            max: None,
            ci95_low: None,
            ci95_high: None,
        }
    }

    /// The bounds the check sets, each with its statistic, in the order of [`Statistic::ALL`].
    pub fn bounds(&self) -> impl Iterator<Item = (Statistic, &Bound)> {
        Statistic::ALL
            .into_iter()
            .filter_map(|statistic| self.bound(statistic).map(|bound| (statistic, bound)))
    }

    /// The bound of one statistic, if the check sets one.
    #[must_use]
    pub fn bound(&self, statistic: Statistic) -> Option<&Bound> {
        match statistic {
            Statistic::Value => self.value.as_ref(),
            Statistic::Mean => self.mean.as_ref(),
            Statistic::Median => self.median.as_ref(),
            Statistic::Min => self.min.as_ref(),
            Statistic::Max => self.max.as_ref(),
            Statistic::Ci95Low => self.ci95_low.as_ref(),
            Statistic::Ci95High => self.ci95_high.as_ref(),
        }
    }

    /// The name the reports use for the bound of `statistic`: the name of the check, with the statistic if the check has more than
    /// one bound; without a name, the metric, the statistic and the bound.
    #[must_use]
    pub fn label(&self, statistic: Statistic, bound: &Bound) -> String {
        match &self.name {
            Some(name) if self.bounds().count() == 1 => name.clone(),
            Some(name) => format!("{name} ({statistic})"),
            None => format!("{} {statistic} {bound}", self.metric),
        }
    }
}

/// What a bound is about: the value of a run, or a statistic of a set of runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Statistic {
    /// The value in the run.
    Value,
    /// The mean over the runs.
    Mean,
    /// The median over the runs.
    Median,
    /// The smallest over the runs.
    Min,
    /// The largest over the runs.
    Max,
    /// The lower end of the 95 % interval around the mean.
    #[serde(rename = "ci95-low")]
    Ci95Low,
    /// The upper end of the 95 % interval around the mean.
    #[serde(rename = "ci95-high")]
    Ci95High,
}

impl Statistic {
    /// All of them, in the order a report lists them.
    pub const ALL: [Self; 7] = [
        Self::Value,
        Self::Mean,
        Self::Median,
        Self::Min,
        Self::Max,
        Self::Ci95Low,
        Self::Ci95High,
    ];

    /// The key of the file.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::Mean => "mean",
            Self::Median => "median",
            Self::Min => "min",
            Self::Max => "max",
            Self::Ci95Low => "ci95-low",
            Self::Ci95High => "ci95-high",
        }
    }

    /// Whether it is judged on one run (`value`) and not on a set.
    #[must_use]
    pub const fn is_run(self) -> bool {
        matches!(self, Self::Value)
    }
}

impl fmt::Display for Statistic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key())
    }
}

/// The comparison of a bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub enum Op {
    /// `<`
    #[serde(rename = "<")]
    Less,
    /// `<=`
    #[serde(rename = "<=")]
    LessOrEqual,
    /// `>`
    #[serde(rename = ">")]
    Greater,
    /// `>=`
    #[serde(rename = ">=")]
    GreaterOrEqual,
}

impl Op {
    /// The operator as the file writes it.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
        }
    }

    /// Whether `value` keeps the bound `threshold`.
    #[must_use]
    pub fn holds(self, value: f64, threshold: f64) -> bool {
        match self {
            Self::Less => value < threshold,
            Self::LessOrEqual => value <= threshold,
            Self::Greater => value > threshold,
            Self::GreaterOrEqual => value >= threshold,
        }
    }

    /// How far `value` is on the good side of `threshold`: positive when the bound holds, negative by how much it is missed.
    #[must_use]
    pub fn margin(self, value: f64, threshold: f64) -> f64 {
        match self {
            Self::Less | Self::LessOrEqual => threshold - value,
            Self::Greater | Self::GreaterOrEqual => value - threshold,
        }
    }
}

/// A bound: an operator and a value in the unit of the metric, written `"< 150ms"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    op: Op,
    value: String,
}

impl Bound {
    /// Reads `"< 150ms"`: an operator, spaces if you like, and a value. The value is read against the unit of the metric by
    /// [`Bound::threshold`].
    ///
    /// # Errors
    ///
    /// The text has no operator or no value.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (op, rest) = if let Some(rest) = text.strip_prefix("<=") {
            (Op::LessOrEqual, rest)
        } else if let Some(rest) = text.strip_prefix(">=") {
            (Op::GreaterOrEqual, rest)
        } else if let Some(rest) = text.strip_prefix('<') {
            (Op::Less, rest)
        } else if let Some(rest) = text.strip_prefix('>') {
            (Op::Greater, rest)
        } else {
            return Err(format!(
                "'{text}' begins with no operator; a bound is `<`, `<=`, `>` or `>=` and a value, such as \"< 150ms\""
            ));
        };
        let value = rest.trim();
        if value.is_empty() {
            return Err(format!(
                "'{text}' has no value after the operator; write it like \"{} 150ms\"",
                op.symbol()
            ));
        }
        Ok(Self {
            op,
            value: value.to_owned(),
        })
    }

    /// A bound of `op` and the value `value` (written as in the file, with its unit).
    #[must_use]
    pub fn new(op: Op, value: impl Into<String>) -> Self {
        Self {
            op,
            value: value.into(),
        }
    }

    /// The comparison.
    #[must_use]
    pub const fn op(&self) -> Op {
        self.op
    }

    /// The value as written, with its unit.
    #[must_use]
    pub fn value_text(&self) -> &str {
        &self.value
    }

    /// The value in the unit the metric is stored in (seconds, bits per second, a ratio from 0 to 1, a count), read with the unit of
    /// the metric.
    ///
    /// # Errors
    ///
    /// The value is not of the kind the metric has, in the words of a message.
    pub fn threshold(&self, unit: MetricUnit) -> Result<f64, String> {
        let text = self.value.as_str();
        let wrong = || match unit {
            MetricUnit::Seconds => {
                format!("'{text}' is not a duration; this metric is a time, written like 150ms")
            }
            MetricUnit::BitsPerSecond => {
                format!("'{text}' is not a rate; this metric is a rate, written like 8mbit")
            }
            MetricUnit::Ratio => format!(
                "'{text}' is not a percentage or a number from 0 to 1; this metric is a share, written like 5% or 0.05"
            ),
            _ => format!(
                "'{text}' is not a plain number; this metric is a count or a value without a unit"
            ),
        };
        let plain = || -> Option<f64> {
            let (number, rest) = decimal::split(text).ok()?;
            rest.is_empty().then_some(())?;
            let (numerator, denominator) = number.ratio()?;
            Some(ratio_to_f64(numerator, denominator))
        };
        match (unit, read_quantity(text)) {
            (MetricUnit::Seconds, Some((Dimension::Duration, (nanos, per)))) => {
                Ok(ratio_to_f64(nanos, per) / 1e9)
            }
            (MetricUnit::BitsPerSecond, Some((Dimension::Rate, (bits, per)))) => {
                Ok(ratio_to_f64(bits, per))
            }
            (MetricUnit::Ratio, Some((Dimension::Percent, (millionths, per)))) => {
                let ratio = ratio_to_f64(millionths, per) / 1e8;
                if ratio <= 1.0 {
                    Ok(ratio)
                } else {
                    Err(format!("'{text}' is above 100 %, which a ratio never is"))
                }
            }
            (MetricUnit::Ratio, None) => match plain() {
                Some(ratio) if ratio <= 1.0 => Ok(ratio),
                Some(_) => Err(format!(
                    "'{text}' is not a ratio from 0 to 1; write a share as a percentage, such as 5%"
                )),
                None => Err(wrong()),
            },
            (MetricUnit::Number, None) => plain().ok_or_else(wrong),
            _ => Err(wrong()),
        }
    }
}

/// A fraction of two whole numbers as a double: a threshold, not a measurement, so the rounding of the last digit is of no
/// account.
#[allow(clippy::cast_precision_loss)]
fn ratio_to_f64(numerator: u128, denominator: u128) -> f64 {
    numerator as f64 / denominator as f64
}

impl fmt::Display for Bound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.op.symbol(), self.value)
    }
}

impl Serialize for Bound {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Bound {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoundVisitor;
        impl Visitor<'_> for BoundVisitor {
            type Value = Bound;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a bound written as a string: an operator and a value, such as \"< 150ms\"",
                )
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Bound, E> {
                Bound::parse(text).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(BoundVisitor)
    }
}

impl JsonSchema for Bound {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Bound".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^\\s*(<=|>=|<|>)\\s*\\S.*$",
            "description": "An operator (<, <=, >, >=) and a value in the unit of the metric: a duration for a delay, a rate for a throughput, a percentage or a number from 0 to 1 for a ratio, a number for a count.",
            "examples": ["< 150ms"]
        })
    }
}

/// What a number of a metric is: the unit the core stores it in, and the kind of value a bound for it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum MetricUnit {
    /// A number: a count of packets, a mean length of a run, the metric of a program.
    Number,
    /// Seconds: a delay, a jitter, a round trip, a completion time.
    Seconds,
    /// Bits per second: a throughput or a goodput.
    BitsPerSecond,
    /// A share from 0 to 1.
    Ratio,
}

impl MetricUnit {
    /// The kind of value, in the words of a message.
    #[must_use]
    pub const fn what(self) -> &'static str {
        match self {
            Self::Number => "a number",
            Self::Seconds => "a duration (in seconds)",
            Self::BitsPerSecond => "a rate (in bits per second)",
            Self::Ratio => "a ratio from 0 to 1",
        }
    }

    /// The unit of a metric of a workload by its name: the part after the workload tells it. (A metric of a program of your own
    /// is a number whatever it is called; [`Scenario::metric_unit_of`] knows the programs.)
    fn of(metric: &str) -> Self {
        let mut parts = metric.split('.').skip(1);
        let (first, last) = (parts.next(), metric.rsplit('.').next());
        match (first, last) {
            (_, Some("loss_ratio")) => Self::Ratio,
            (Some("delay" | "jitter" | "round_trip" | "rtt" | "latency" | "completion"), _) => {
                Self::Seconds
            }
            (Some("throughput" | "goodput"), _) => Self::BitsPerSecond,
            _ => Self::Number,
        }
    }
}

const SUMMARY: [&str; 8] = ["min", "mean", "p50", "p90", "p95", "p99", "p999", "max"];

/// The names after the workload, for each kind of source of metrics: the `udp-*` workloads, the echo of those, the TCP workloads and
/// the requests of `request-response`.
fn udp_metrics() -> Vec<String> {
    let mut names: Vec<String> = [
        "sent",
        "received",
        "lost",
        "duplicates",
        "reordered",
        "loss_ratio",
        "throughput.mean",
        "loss_runs.longest",
        "loss_runs.mean",
        "jitter.last",
        "jitter.mean",
        "jitter.max",
    ]
    .map(str::to_owned)
    .to_vec();
    names.extend(SUMMARY.map(|name| format!("delay.{name}")));
    names
}

fn echo_metrics() -> Vec<String> {
    let mut names: Vec<String> = ["echo.received", "echo.lost", "echo.loss_ratio"]
        .map(str::to_owned)
        .to_vec();
    names.extend(SUMMARY.map(|name| format!("round_trip.{name}")));
    names
}

fn tcp_metrics() -> Vec<String> {
    let mut names: Vec<String> = [
        "goodput.mean",
        "completion",
        "bytes_sent",
        "bytes_received",
        "connections",
        "failed_connections",
        "retransmitted_segments",
    ]
    .map(str::to_owned)
    .to_vec();
    names.extend(SUMMARY.map(|name| format!("rtt.{name}")));
    names
}

fn request_metrics() -> Vec<String> {
    let mut names: Vec<String> = ["sent", "answered", "failed"].map(str::to_owned).to_vec();
    names.extend(SUMMARY.map(|name| format!("latency.{name}")));
    names.extend(SUMMARY.map(|name| format!("round_trip.{name}")));
    names
}

/// Why a name is not the name of a metric of the scenario.
pub(crate) struct UnknownMetric {
    pub(crate) message: String,
    pub(crate) help: String,
}

impl Scenario {
    /// The names of the metrics a run of this scenario can report, without the metrics a program of your own reports (those are
    /// `<program>.<name>.last`, `.min` and `.max`, and are known when the program has run). A metric can be missing from a run (no
    /// packet arrived, so there is no delay); the name is still one of the scenario.
    #[must_use]
    pub fn metric_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for workload in &self.workloads {
            let id = workload.id();
            let own = match workload {
                Workload::Command(_) => continue,
                Workload::TcpBulk(_) => tcp_metrics(),
                Workload::RequestResponse(_) => {
                    let mut own = tcp_metrics();
                    own.extend(request_metrics());
                    own
                }
                _ => {
                    let mut own = udp_metrics();
                    if workload.generator().is_some_and(|generator| generator.echo) {
                        own.extend(echo_metrics());
                    }
                    own
                }
            };
            names.extend(own.into_iter().map(|name| format!("{id}.{name}")));
        }
        names
    }

    /// The unit of the metric `name` of this scenario, or `None` if it is not one of its metrics.
    #[must_use]
    pub fn metric_unit_of(&self, name: &str) -> Option<MetricUnit> {
        self.metric_unit(name).ok()
    }

    /// The unit of the metric `name` of this scenario, or why it is not one.
    pub(crate) fn metric_unit(&self, name: &str) -> Result<MetricUnit, UnknownMetric> {
        let (id, rest) = name.split_once('.').unwrap_or((name, ""));
        let Some(workload) = self.workloads.iter().find(|w| w.id().as_str() == id) else {
            let ids: Vec<&str> = self.workloads.iter().map(|w| w.id().as_str()).collect();
            let help = match closest(id, &ids) {
                Some(near) => format!(
                    "did you mean '{near}'? the workloads are: {}",
                    ids.join(", ")
                ),
                None => format!(
                    "a metric is written `<workload>.<metric>`; the workloads are: {}",
                    ids.join(", ")
                ),
            };
            return Err(UnknownMetric {
                message: format!("'{name}' is not a metric: there is no workload '{id}'"),
                help,
            });
        };
        if matches!(workload, Workload::Command(_)) {
            // The metrics of a program are named by the program when it runs
            let ends = rest.rsplit('.').next().unwrap_or("");
            return if rest.contains('.') && matches!(ends, "last" | "min" | "max") {
                Ok(MetricUnit::Number)
            } else {
                Err(UnknownMetric {
                    message: format!("'{name}' is not a metric of the program '{id}'"),
                    help: format!(
                        "a metric of a program is `{id}.<name>.last`, `{id}.<name>.min` or `{id}.<name>.max`, with the name the program reports"
                    ),
                })
            };
        }
        let known: Vec<String> = self
            .metric_names()
            .into_iter()
            .filter_map(|full| full.strip_prefix(&format!("{id}.")).map(str::to_owned))
            .collect();
        if known.iter().any(|own| own == rest) {
            return Ok(MetricUnit::of(name));
        }
        let mut all = udp_metrics();
        all.extend(echo_metrics());
        let help = if all.iter().any(|own| own == rest) && !known.iter().any(|own| own == rest) {
            "this metric needs `echo: true` on the workload (D-087)".to_owned()
        } else {
            let names: Vec<&str> = known.iter().map(String::as_str).collect();
            match closest(rest, &names) {
                Some(near) => format!("did you mean '{id}.{near}'?"),
                None => format!(
                    "the metrics of '{id}' begin with: {} (all are in docs/STATISTICS.md)",
                    beginnings(&known)
                ),
            }
        };
        Err(UnknownMetric {
            message: format!("'{rest}' is not a metric of the workload '{id}'"),
            help,
        })
    }

    /// The rules of `expect`: the metrics exist, the bounds are of the kind of their metric, the statistics fit the number of runs,
    /// and the names are fit for a report.
    pub(crate) fn check_expect(&self, problems: &mut Vec<Problem>) {
        let Some(expect) = &self.expect else {
            return;
        };
        if expect.checks.is_empty() {
            problems.push(
                Problem::new("`expect` has no check")
                    .at_path("expect.checks")
                    .with_help("write a check, such as `- { metric: media.loss_ratio, value: \"< 1%\" }`, or leave `expect` out"),
            );
        }
        if expect.checks.len() > MAX_CHECKS {
            problems.push(
                Problem::new(format!(
                    "{} checks are more than the {MAX_CHECKS} that a file may have",
                    expect.checks.len()
                ))
                .at_path("expect.checks"),
            );
        }
        if let Some(repetitions) = expect.repetitions
            && !(MIN_EXPECT_REPETITIONS..=MAX_EXPECT_REPETITIONS).contains(&repetitions)
        {
            problems.push(
                Problem::new(format!(
                    "`repetitions` is {repetitions}; it is from {MIN_EXPECT_REPETITIONS} (fewer runs say too little about the spread for an interval) to {MAX_EXPECT_REPETITIONS}"
                ))
                .at_path("expect.repetitions"),
            );
        }
        let mut seen: Vec<String> = Vec::new();
        for (index, check) in expect.checks.iter().enumerate() {
            let base = format!("expect.checks[{index}]");
            self.check_one(expect, check, &base, problems);
            for (statistic, bound) in check.bounds() {
                let label = check.label(statistic, bound);
                if seen.contains(&label) {
                    problems.push(
                        Problem::new(format!("two checks are called '{label}'"))
                            .at_path(format!("{base}.{statistic}"))
                            .with_help("give each check a `name` of its own: the reports tell the checks apart by it"),
                    );
                }
                seen.push(label);
            }
        }
    }

    fn check_one(&self, expect: &Expect, check: &Check, base: &str, problems: &mut Vec<Problem>) {
        if let Some(name) = &check.name
            && (name.trim().is_empty()
                || name.chars().count() > MAX_CHECK_NAME
                || name.chars().any(char::is_control))
        {
            problems.push(
                Problem::new(format!(
                    "the name of a check is one line of 1 to {MAX_CHECK_NAME} characters"
                ))
                .at_path(format!("{base}.name")),
            );
        }
        let unit = match self.metric_unit(&check.metric) {
            Ok(unit) => Some(unit),
            Err(unknown) => {
                problems.push(
                    Problem::new(unknown.message)
                        .at_path(format!("{base}.metric"))
                        .with_help(unknown.help),
                );
                None
            }
        };
        if check.bounds().next().is_none() {
            let help = if expect.is_set() {
                "add a bound such as `ci95-high: \"< 5%\"`; the statistics of a set are mean, median, min, max, ci95-low and ci95-high"
            } else {
                "add a bound such as `value: \"< 5%\"`"
            };
            problems.push(
                Problem::new("a check has no bound")
                    .at_path(base.to_owned())
                    .with_help(help),
            );
        }
        for (statistic, bound) in check.bounds() {
            let path = format!("{base}.{statistic}");
            if statistic.is_run() == expect.is_set() {
                let (message, help) = if expect.is_set() {
                    (
                        "`value` is the value of one run, and this file is judged by a set of runs".to_owned(),
                        "use `mean`, `median`, `min`, `max`, `ci95-low` or `ci95-high`, or take `repetitions` out of `expect`".to_owned(),
                    )
                } else {
                    (
                        format!(
                            "`{statistic}` is a statistic of a set of runs, and this file is judged by one run"
                        ),
                        format!(
                            "add `repetitions: {MIN_EXPECT_REPETITIONS}` or more to `expect`, or use `value`"
                        ),
                    )
                };
                problems.push(Problem::new(message).at_path(path.clone()).with_help(help));
            }
            if let Some(unit) = unit
                && let Err(message) = bound.threshold(unit)
            {
                problems.push(Problem::new(message).at_path(path));
            }
        }
    }
}

/// The first parts of the names, for a message: `delay.`, `jitter.`, `sent`.
fn beginnings(names: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for name in names {
        let part = name
            .split_once('.')
            .map_or_else(|| name.clone(), |(head, _)| format!("{head}."));
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_is_an_operator_and_a_value() {
        let bound = Bound::parse(" <=  150ms ").unwrap();
        assert_eq!(bound.op(), Op::LessOrEqual);
        assert_eq!(bound.value_text(), "150ms");
        assert_eq!(bound.to_string(), "<= 150ms");
        for text in ["150ms", "< ", "=> 3", "== 3", ""] {
            assert!(Bound::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn the_value_is_read_with_the_unit_of_the_metric() {
        let at = |text: &str, unit| Bound::parse(&format!("< {text}")).unwrap().threshold(unit);
        assert!((at("150ms", MetricUnit::Seconds).unwrap() - 0.15).abs() < 1e-12);
        assert!((at("2.5s", MetricUnit::Seconds).unwrap() - 2.5).abs() < 1e-12);
        assert!((at("8mbit", MetricUnit::BitsPerSecond).unwrap() - 8e6).abs() < 1e-6);
        assert!((at("5%", MetricUnit::Ratio).unwrap() - 0.05).abs() < 1e-12);
        assert!((at("0.05", MetricUnit::Ratio).unwrap() - 0.05).abs() < 1e-12);
        assert!((at("0", MetricUnit::Number).unwrap()).abs() < 1e-12);
        assert!((at("1500", MetricUnit::Number).unwrap() - 1500.0).abs() < 1e-9);
        // The wrong kind of value is refused, with what the metric is
        for (text, unit) in [
            ("150", MetricUnit::Seconds),
            ("8mbit", MetricUnit::Seconds),
            ("150ms", MetricUnit::BitsPerSecond),
            ("150ms", MetricUnit::Ratio),
            ("5", MetricUnit::Ratio),
            ("101%", MetricUnit::Ratio),
            ("5%", MetricUnit::Number),
            ("1kb", MetricUnit::Number),
            ("abc", MetricUnit::Number),
        ] {
            assert!(at(text, unit).is_err(), "{text} as {unit:?}");
        }
    }

    #[test]
    fn the_unit_of_a_metric_comes_from_its_name() {
        for (name, unit) in [
            ("w.loss_ratio", MetricUnit::Ratio),
            ("w.echo.loss_ratio", MetricUnit::Ratio),
            ("w.delay.p99", MetricUnit::Seconds),
            ("w.jitter.mean", MetricUnit::Seconds),
            ("w.round_trip.max", MetricUnit::Seconds),
            ("w.rtt.p50", MetricUnit::Seconds),
            ("w.latency.p99", MetricUnit::Seconds),
            ("w.completion", MetricUnit::Seconds),
            ("w.throughput.mean", MetricUnit::BitsPerSecond),
            ("w.goodput.mean", MetricUnit::BitsPerSecond),
            ("w.sent", MetricUnit::Number),
            ("w.loss_runs.mean", MetricUnit::Number),
        ] {
            assert_eq!(MetricUnit::of(name), unit, "{name}");
        }
    }
}
