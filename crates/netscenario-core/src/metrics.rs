//! Metrics from a program of your own (PRB-08, D-072): lines of JSON on its standard output.
//!
//! ```text
//! {"metric": "frames_per_second", "value": 29.7, "unit": "1/s"}
//! ```
//!
//! A line is a **metric line** if it starts with `{` and has the key `metric`; anything else the program prints is its
//! own business and is not looked at. A metric line must be a JSON object with a `metric` (a text of 1 to 128 characters
//! without control characters), a `value` (a finite number) and, optionally, a `unit` (a text). Other keys are refused: a
//! misspelled `valeu` should be seen and not read as a missing value. A metric line that is not valid is **counted and
//! the first ones are kept in the result**; it is not swallowed and it does not stop the run. The time of a reading is the
//! time the run read the line, which is within 50 ms of the time the program wrote it.

use std::collections::BTreeMap;
use std::io::Write;

use serde::Serialize;

/// The most metrics (different names) that one program may report.
pub const MAX_METRICS: usize = 256;
/// The most readings that are written to the file of a program; the summary counts all.
pub const MAX_READINGS_IN_FILE: u64 = 1_000_000;
/// How many invalid metric lines the result keeps as examples.
pub const MAX_BAD_EXAMPLES: usize = 5;
/// The longest example, in characters.
const EXAMPLE_LENGTH: usize = 200;
/// The longest name of a metric.
pub const MAX_NAME: usize = 128;

/// One reading.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// The name of the metric.
    pub metric: String,
    /// The value.
    pub value: f64,
    /// The unit, if the program gave one.
    pub unit: Option<String>,
}

/// What a line of standard output is.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// Not a metric line: the program's own output.
    Other,
    /// A valid metric.
    Metric(Reading),
    /// Looked like a metric line and is not valid; the text says why.
    Bad(String),
}

/// Reads a line.
#[must_use]
pub fn parse_line(line: &str) -> Line {
    let text = line.trim();
    if !text.starts_with('{') || !text.contains("\"metric\"") {
        return Line::Other;
    }
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => return Line::Bad(format!("not valid JSON: {error}")),
    };
    let Some(object) = value.as_object() else {
        return Line::Bad("not a JSON object".to_owned());
    };
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "metric" | "value" | "unit"))
    {
        return Line::Bad(format!(
            "the key `{key}` is not one of `metric`, `value` and `unit`"
        ));
    }
    let Some(metric) = object.get("metric").and_then(serde_json::Value::as_str) else {
        return Line::Bad("`metric` must be a text".to_owned());
    };
    if metric.is_empty()
        || metric.chars().count() > MAX_NAME
        || metric.chars().any(char::is_control)
    {
        return Line::Bad(format!(
            "`metric` must be 1 to {MAX_NAME} characters without control characters"
        ));
    }
    let Some(number) = object.get("value").and_then(serde_json::Value::as_f64) else {
        return Line::Bad("`value` must be a number".to_owned());
    };
    if !number.is_finite() {
        return Line::Bad("`value` must be finite".to_owned());
    }
    let unit = match object.get("unit") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(unit)) => Some(unit.clone()),
        Some(_) => return Line::Bad("`unit` must be a text".to_owned()),
    };
    Line::Metric(Reading {
        metric: metric.to_owned(),
        value: number,
        unit,
    })
}

/// What the readings of one metric came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// The unit of the first reading that had one.
    pub unit: Option<String>,
    /// How many readings.
    pub count: u64,
    /// The first value.
    pub first: f64,
    /// The last value.
    pub last: f64,
    /// The smallest.
    pub min: f64,
    /// The largest.
    pub max: f64,
}

/// A line of the file of readings.
#[derive(Serialize)]
struct FileLine<'a> {
    t_ns: i64,
    metric: &'a str,
    value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: &'a Option<String>,
}

/// The metrics of one program as they come in.
#[derive(Default)]
pub struct Collector {
    summaries: BTreeMap<String, Summary>,
    readings: u64,
    bad: u64,
    examples: Vec<String>,
    written: u64,
    file: Option<Box<dyn Write + Send>>,
}

impl std::fmt::Debug for Collector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Collector")
            .field("summaries", &self.summaries)
            .field("readings", &self.readings)
            .field("bad", &self.bad)
            .finish_non_exhaustive()
    }
}

impl Collector {
    /// A collector that also writes every reading (up to [`MAX_READINGS_IN_FILE`]) as a line of JSON to `file`.
    #[must_use]
    pub fn with_file(file: Box<dyn Write + Send>) -> Self {
        Self {
            file: Some(file),
            ..Self::default()
        }
    }

    /// Takes one line of standard output read at `t_ns` (nanoseconds from the start of the measured phase, negative
    /// before it).
    pub fn take(&mut self, line: &str, t_ns: i64) {
        match parse_line(line) {
            Line::Other => {}
            Line::Bad(why) => self.bad_line(line, &why),
            Line::Metric(reading) => self.reading(reading, t_ns),
        }
    }

    fn bad_line(&mut self, line: &str, why: &str) {
        self.bad += 1;
        if self.examples.len() < MAX_BAD_EXAMPLES {
            let shown: String = line.chars().take(EXAMPLE_LENGTH).collect();
            self.examples.push(format!("{why}: {shown}"));
        }
    }

    fn reading(&mut self, reading: Reading, t_ns: i64) {
        if !self.summaries.contains_key(&reading.metric) && self.summaries.len() >= MAX_METRICS {
            self.bad_line(
                &reading.metric,
                &format!("more than {MAX_METRICS} different metrics; this one is not kept"),
            );
            return;
        }
        self.readings += 1;
        if self.written < MAX_READINGS_IN_FILE
            && let Some(file) = &mut self.file
        {
            let line = FileLine {
                t_ns,
                metric: &reading.metric,
                value: reading.value,
                unit: &reading.unit,
            };
            let wrote = serde_json::to_writer(&mut *file, &line)
                .map_err(std::io::Error::from)
                .and_then(|()| file.write_all(b"\n"));
            if wrote.is_err() {
                self.file = None;
            } else {
                self.written += 1;
            }
        }
        self.summaries
            .entry(reading.metric)
            .and_modify(|summary| {
                summary.count += 1;
                summary.last = reading.value;
                summary.min = summary.min.min(reading.value);
                summary.max = summary.max.max(reading.value);
                if summary.unit.is_none() {
                    summary.unit.clone_from(&reading.unit);
                }
            })
            .or_insert(Summary {
                unit: reading.unit,
                count: 1,
                first: reading.value,
                last: reading.value,
                min: reading.value,
                max: reading.value,
            });
    }

    /// Flushes the file, if there is one.
    pub fn finish(&mut self) {
        if let Some(file) = &mut self.file {
            let _ = file.flush();
        }
    }

    /// The summaries by name.
    #[must_use]
    pub fn summaries(&self) -> &BTreeMap<String, Summary> {
        &self.summaries
    }

    /// How many valid readings were taken.
    #[must_use]
    pub fn readings(&self) -> u64 {
        self.readings
    }

    /// How many readings are in the file.
    #[must_use]
    pub fn written(&self) -> u64 {
        self.written
    }

    /// How many metric lines were not valid.
    #[must_use]
    pub fn bad(&self) -> u64 {
        self.bad
    }

    /// The first invalid metric lines, with the reason.
    #[must_use]
    pub fn examples(&self) -> &[String] {
        &self.examples
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn metric(line: &str) -> Reading {
        match parse_line(line) {
            Line::Metric(reading) => reading,
            other => panic!("{line}: {other:?}"),
        }
    }

    fn bad(line: &str) -> String {
        match parse_line(line) {
            Line::Bad(why) => why,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn a_metric_line_has_a_name_a_value_and_perhaps_a_unit() {
        assert_eq!(
            metric(r#"{"metric": "fps", "value": 29.7, "unit": "1/s"}"#),
            Reading {
                metric: "fps".to_owned(),
                value: 29.7,
                unit: Some("1/s".to_owned())
            }
        );
        let plain = metric(r#"  {"value": 3, "metric": "calls"}  "#);
        assert_eq!((plain.value, plain.unit), (3.0, None));
        assert_eq!(
            metric(r#"{"metric":"x","value":-1e3,"unit":null}"#).value,
            -1000.0
        );
    }

    #[test]
    fn what_a_program_prints_otherwise_is_not_looked_at() {
        for line in [
            "starting up",
            "",
            "{}",
            r#"{"event": "ready"}"#,
            "metric: 3",
            r#"["metric"]"#,
            r#"x {"metric": "a", "value": 1}"#,
        ] {
            assert_eq!(parse_line(line), Line::Other, "{line}");
        }
    }

    /// Canary: every way to be wrong is named, and none of them is read as a metric.
    #[test]
    fn a_metric_line_that_is_wrong_is_bad_and_says_why() {
        assert!(bad(r#"{"metric": "a", "value": 1"#).starts_with("not valid JSON"));
        assert!(bad(r#"{"metric": "a", "value": "fast"}"#).contains("`value` must be a number"));
        assert!(bad(r#"{"metric": "a"}"#).contains("`value` must be a number"));
        assert!(bad(r#"{"metric": 5, "value": 1}"#).contains("`metric` must be a text"));
        assert!(bad(r#"{"metric": "", "value": 1}"#).contains("1 to 128"));
        assert!(bad(r#"{"metric": "a\nb", "value": 1}"#).contains("control"));
        assert!(bad(r#"{"metric": "a", "valeu": 1, "value": 1}"#).contains("`valeu`"));
        assert!(bad(r#"{"metric": "a", "value": 1, "unit": 3}"#).contains("`unit` must be a text"));
        assert!(
            bad(r#"{"metric": "a", "value": 1e999}"#).contains("not valid JSON")
                || bad(r#"{"metric": "a", "value": 1e999}"#).contains("finite")
        );
        let long = format!(
            r#"{{"metric": "{}", "value": 1}}"#,
            "n".repeat(MAX_NAME + 1)
        );
        assert!(bad(&long).contains("1 to 128"));
    }

    #[test]
    fn the_summary_follows_the_readings_in_order() {
        let mut collector = Collector::default();
        for (value, t) in [(5.0, 10), (2.0, 20), (9.0, 30), (4.0, 40)] {
            collector.take(
                &format!(r#"{{"metric": "latency", "value": {value}, "unit": "ms"}}"#),
                t,
            );
        }
        collector.take(r#"{"metric": "frames", "value": 1}"#, 50);
        let latency = &collector.summaries()["latency"];
        assert_eq!(
            (
                latency.count,
                latency.first,
                latency.last,
                latency.min,
                latency.max
            ),
            (4, 5.0, 4.0, 2.0, 9.0)
        );
        assert_eq!(latency.unit.as_deref(), Some("ms"));
        assert_eq!(collector.summaries()["frames"].unit, None);
        assert_eq!((collector.readings(), collector.bad()), (5, 0));
    }

    #[test]
    fn bad_lines_are_counted_and_the_first_few_are_kept() {
        let mut collector = Collector::default();
        for n in 0..MAX_BAD_EXAMPLES + 3 {
            collector.take(&format!(r#"{{"metric": "a", "value": "{n}"}}"#), 0);
        }
        collector.take(r#"{"metric": "a", "value": 1}"#, 0);
        assert_eq!(collector.bad(), (MAX_BAD_EXAMPLES + 3) as u64);
        assert_eq!(collector.examples().len(), MAX_BAD_EXAMPLES);
        assert!(
            collector.examples()[0].contains(r#""value": "0""#),
            "{:?}",
            collector.examples()
        );
        // The good line after them is still read
        assert_eq!(collector.readings(), 1);
    }

    #[test]
    fn a_metric_past_the_limit_of_names_is_counted_as_bad() {
        let mut collector = Collector::default();
        for n in 0..=MAX_METRICS {
            collector.take(&format!(r#"{{"metric": "m{n}", "value": 1}}"#), 0);
        }
        assert_eq!(collector.summaries().len(), MAX_METRICS);
        assert_eq!(collector.bad(), 1);
        // A name that is kept goes on
        collector.take(r#"{"metric": "m0", "value": 2}"#, 0);
        assert_eq!(collector.summaries()["m0"].count, 2);
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn every_reading_is_a_line_of_the_file_with_the_time() {
        let sink = Sink::default();
        let mut collector = Collector::with_file(Box::new(sink.clone()));
        collector.take(r#"{"metric": "a", "value": 1.5, "unit": "s"}"#, -7);
        collector.take(r#"{"metric": "b", "value": 2}"#, 1_500_000_000);
        collector.take("not a metric", 3);
        collector.finish();
        let text = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            text,
            "{\"t_ns\":-7,\"metric\":\"a\",\"value\":1.5,\"unit\":\"s\"}\n{\"t_ns\":1500000000,\"metric\":\"b\",\"value\":2.0}\n"
        );
        assert_eq!(collector.written(), 2);
    }
}
