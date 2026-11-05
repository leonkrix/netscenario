//! Reports from result directories: Markdown, JUnit and HTML (CLI-02, RPT-02, RPT-03, RPT-04, QA-04; D-043, D-076).
//!
//! A report is made from a directory that `netscenario run` (a run) or `netscenario run --repetitions` (a set of runs)
//! wrote, and from nothing else: no clock, no machine, no random numbers, so the same directory gives the same bytes and the
//! reports can be tested against snapshots.
//!
//! - **Markdown** is made for the summary of a CI job (`$GITHUB_STEP_SUMMARY`): one table for the workloads, the timeline,
//!   the programs, and for a set the headline metrics with their intervals.
//! - **JUnit** has one test case for each workload (and for each program) with the metrics as properties under the names
//!   of the pytest plugin (`netscenario.<scenario>.<workload>.<metric>`). A case fails if the run was interrupted, or if the
//!   kernel refused packets of the workload although no link was shut on purpose (D-076).
//! - **HTML** is one file: inline CSS and inline SVG, no script, no font, no image and no address to fetch, so it can be
//!   attached to a CI run and opened anywhere. It draws the throughput and the delay over time with the events of the
//!   timeline marked, the lost packets by time, and the distribution of the delays; for a set, the value of every run
//!   and the interval of each metric.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    reason = "report code: counts and times are far below 2^52, a drawing or a page is a long list of parts, and JUnit is a name"
)]

mod html;
mod svg;
mod text;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use netscenario_agent::records;

use crate::result::{RunResult, WorkloadResult};
use crate::runset::{RUNSET_FILE, RunSetResult};
use crate::series::Bin;

pub use html::html;
pub use text::{junit, markdown, run_properties, set_properties};

/// A format of a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Markdown, for the summary of a CI job.
    Markdown,
    /// JUnit XML, for the test report of a CI.
    Junit,
    /// One HTML file with inline SVG.
    Html,
}

impl Format {
    /// The word for the format on the command line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Junit => "junit",
            Self::Html => "html",
        }
    }
}

impl std::str::FromStr for Format {
    type Err = String;

    fn from_str(word: &str) -> Result<Self, String> {
        match word {
            "md" | "markdown" => Ok(Self::Markdown),
            "junit" => Ok(Self::Junit),
            "html" => Ok(Self::Html),
            other => Err(format!("the format is md, html or junit, not '{other}'")),
        }
    }
}

/// Why a report cannot be made. All of these are wrong input.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReportError {
    /// The directory holds neither a result nor a set.
    #[error("{0} has neither result.json nor runset.json: it is not a result directory")]
    NotAResult(PathBuf),
    /// A file cannot be read or is not what it should be.
    #[error("{path}: {message}")]
    Bad {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },
}

/// What is read of one workload for the drawings: the series by time bin and the delay of every packet that arrived.
#[derive(Debug, Clone, Default)]
pub struct Detail {
    /// The bins of the series (empty if the result has none, as results of older versions).
    pub bins: Vec<Bin>,
    /// The one-way delay of the first arrival of every packet, in nanoseconds, in the order of arrival.
    pub delays_ns: Vec<u64>,
}

/// A run, as the reports see it.
#[derive(Debug, Clone)]
pub struct RunReport {
    /// The result.
    pub result: RunResult,
    /// The directory it was read from.
    pub dir: PathBuf,
    /// For each workload, in the order of the result: the data of the drawings. Empty if the report was loaded without
    /// detail (Markdown and JUnit need none).
    pub details: Vec<Detail>,
}

/// A report's input: a run or a set of runs.
#[derive(Debug, Clone)]
pub enum Report {
    /// A run (`result.json`).
    Run(Box<RunReport>),
    /// A set of runs (`runset.json`).
    Set(Box<RunSetResult>, PathBuf),
}

fn bad(path: &Path, message: impl Into<String>) -> ReportError {
    ReportError::Bad {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

impl Report {
    /// Reads the directory. With `detail` the series and the records of every workload are read as well, which the HTML needs.
    ///
    /// # Errors
    ///
    /// [`ReportError`]: not a result directory, or a file that cannot be read.
    pub fn load(dir: &Path, detail: bool) -> Result<Self, ReportError> {
        let result_path = dir.join("result.json");
        if result_path.is_file() {
            let text = std::fs::read_to_string(&result_path)
                .map_err(|error| bad(&result_path, error.to_string()))?;
            let result =
                RunResult::from_json(&text).map_err(|message| bad(&result_path, message))?;
            let details = if detail {
                result
                    .workloads
                    .iter()
                    .map(|workload| read_detail(dir, workload))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            return Ok(Self::Run(Box::new(RunReport {
                result,
                dir: dir.to_path_buf(),
                details,
            })));
        }
        let set_path = dir.join(RUNSET_FILE);
        if set_path.is_file() {
            let text = std::fs::read_to_string(&set_path)
                .map_err(|error| bad(&set_path, error.to_string()))?;
            let set = RunSetResult::from_json(&text).map_err(|message| bad(&set_path, message))?;
            return Ok(Self::Set(Box::new(set), dir.to_path_buf()));
        }
        Err(ReportError::NotAResult(dir.to_path_buf()))
    }

    /// The report in a format.
    ///
    /// # Errors
    ///
    /// [`ReportError`] for an HTML report of a run that was loaded without detail.
    pub fn render(&self, format: Format) -> Result<String, ReportError> {
        match format {
            Format::Markdown => Ok(markdown(self)),
            Format::Junit => Ok(junit(self)),
            Format::Html => html(self),
        }
    }
}

/// Reads the series and the delays of a workload. A series that is not there is no error (older results have none).
fn read_detail(dir: &Path, workload: &WorkloadResult) -> Result<Detail, ReportError> {
    let mut detail = Detail::default();
    if let Some(series) = &workload.series {
        let path = dir.join(&series.file);
        let text = std::fs::read_to_string(&path).map_err(|error| bad(&path, error.to_string()))?;
        for (number, line) in text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
        {
            let bin: Bin = serde_json::from_str(line)
                .map_err(|error| bad(&path, format!("line {}: {error}", number + 1)))?;
            detail.bins.push(bin);
        }
    }
    let path = dir.join(&workload.records_file);
    let file = std::fs::File::open(&path).map_err(|error| bad(&path, error.to_string()))?;
    let records = records::read(std::io::BufReader::new(file))
        .map_err(|error| bad(&path, error.to_string()))?;
    let mut seen = HashSet::new();
    detail.delays_ns = records
        .records
        .iter()
        .filter(|record| seen.insert(record.sequence))
        .map(records::Record::delay_ns)
        .collect();
    Ok(detail)
}

/// Makes the report of a directory in one call.
///
/// # Errors
///
/// [`ReportError`]: not a result directory, or a file that cannot be read.
pub fn report(dir: &Path, format: Format) -> Result<String, ReportError> {
    Report::load(dir, format == Format::Html)?.render(format)
}

// ---- helpers the three formats share ----

/// Escapes text for HTML, XML and attribute values.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for letter in text.chars() {
        match letter {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // Control characters other than tab and line feeds are not allowed in XML 1.0
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// Nanoseconds as seconds with up to three decimals, without trailing zeros: `2`, `0.5`, `1.12`.
pub(crate) fn seconds(ns: u64) -> String {
    trim(format!("{:.3}", ns as f64 / 1e9))
}

fn trim(mut number: String) -> String {
    if number.contains('.') {
        while number.ends_with('0') {
            number.pop();
        }
        if number.ends_with('.') {
            number.pop();
        }
    }
    number
}

/// Nanoseconds as milliseconds with two decimals: `20.31 ms`.
pub(crate) fn ms(ns: u64) -> String {
    format!("{:.2} ms", ns as f64 / 1e6)
}

/// A share from 0 to 1 as a percentage with two decimals.
pub(crate) fn percent(ratio: f64) -> String {
    format!("{:.2} %", ratio * 100.0)
}

/// Bits per second in the unit that reads best.
pub(crate) fn rate(bits: f64) -> String {
    let (value, unit) = if bits >= 1e9 {
        (bits / 1e9, "gbit/s")
    } else if bits >= 1e6 {
        (bits / 1e6, "mbit/s")
    } else if bits >= 1e3 {
        (bits / 1e3, "kbit/s")
    } else {
        (bits, "bit/s")
    };
    format!("{} {unit}", trim(format!("{value:.2}")))
}

/// A value of a metric of a set as text, in the unit that reads best for its name.
pub(crate) fn metric_value(name: &str, value: f64) -> String {
    if name.ends_with("loss_ratio") {
        percent(value)
    } else if name.contains(".delay.") || name.contains(".jitter.") {
        format!("{:.3} ms", value * 1e3)
    } else if name.ends_with("throughput.mean") {
        rate(value)
    } else {
        trim(format!("{value:.4}"))
    }
}

/// The metrics of a workload that the short views show for a set, in this order.
pub(crate) const HEADLINE: [&str; 5] = [
    "loss_ratio",
    "delay.p50",
    "delay.p95",
    "delay.p99",
    "throughput.mean",
];

/// Whether a link was shut on purpose in the run: a `down` step of the timeline. The kernel refuses the packets sent into a
/// link that is down, and that is the point of the step.
pub(crate) fn link_shut_on_purpose(result: &RunResult) -> bool {
    result.events.iter().any(|event| event.kind == "down")
}

/// Why a workload counts as failed in a report, if it does (D-076): the run was cut short, or the kernel refused packets that
/// no `down` step explains.
pub(crate) fn failure_of(result: &RunResult, workload: &WorkloadResult) -> Option<String> {
    if let Some(interrupted) = result.timing.interrupted_ns {
        return Some(format!(
            "the run was interrupted after {} s of {} s, so this is the result of the part that ran",
            seconds(interrupted),
            seconds(result.timing.duration_ns)
        ));
    }
    if workload.sender.send_errors > 0 && !link_shut_on_purpose(result) {
        return Some(format!(
            "the kernel refused {} packets of this workload ({}) and no link was shut on purpose",
            workload.sender.send_errors,
            workload
                .sender
                .first_error
                .as_deref()
                .unwrap_or("no reason given")
        ));
    }
    None
}

/// Why a TCP workload fails its case in the JUnit file: the run was interrupted, or connections failed although no link was shut on
/// purpose (D-077, D-086). A connection that breaks during an outage of the scenario is what the scenario asks for.
pub(crate) fn tcp_failure_of(result: &RunResult, tcp: &crate::result::TcpResult) -> Option<String> {
    if let Some(interrupted) = result.timing.interrupted_ns {
        return Some(format!(
            "the run was interrupted after {} s of {} s, so this is the result of the part that ran",
            seconds(interrupted),
            seconds(result.timing.duration_ns)
        ));
    }
    if tcp.failed_connections > 0 && !link_shut_on_purpose(result) {
        return Some(format!(
            "{} of {} connections of this workload failed and no link was shut on purpose: {}",
            tcp.failed_connections,
            tcp.connections,
            tcp.first_error.as_deref().unwrap_or("no reason given")
        ));
    }
    None
}

#[cfg(test)]
mod tests;
