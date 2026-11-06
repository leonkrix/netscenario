//! Traces of a link: the rate, the delay and the loss over time, from a file (LMD-03, D-091).
//!
//! A trace is a model of a link (`model: { kind: trace }`, D-090) whose states come from a file. The file is read when the
//! scenario is loaded (D-083) and what it holds is written into the scenario as [`TracePoints`], so the canonical form and its
//! hash cover the content and a recorded scenario runs without the file. When a run begins the points are replaced by the
//! events of the timeline, like the states of any other model.
//!
//! Two formats are read. **CSV** is ours: a header with `time` and any of `rate`, `delay` and `loss`, and a row for each change,
//! every value with its unit. **Mahimahi** is the format of the trace-driven link of that tool: one line for each opportunity
//! to deliver a packet, the time in milliseconds. A window of it is turned into a rate.

use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Problem;
use crate::link_model::{LinkModel, MAX_EVENTS_PER_LINK, Writer};
use crate::scenario::{Direction, Edit, Event, ImpairmentsChange, Link, Loss};
use crate::units::{Duration, Probability, Rate, ValueError};

/// The most rows that a trace may have: a row is a change of the link, and a link makes no more events than
/// [`MAX_EVENTS_PER_LINK`].
pub const MAX_TRACE_ROWS: usize = MAX_EVENTS_PER_LINK;

/// The bytes of one delivery opportunity of a Mahimahi trace: an IP packet of 1,500 bytes and the Ethernet header, which is what
/// the shapers of the Kernel backend count (D-079). A link that delivers one opportunity per millisecond has 12.112 mbit.
pub const OPPORTUNITY_BYTES: u64 = 1514;

/// The window of a Mahimahi trace, when the file does not say: the finest that the Kernel backend follows without a restriction
/// (D-079).
pub const DEFAULT_WINDOW: Duration = Duration::from_nanos(100_000_000);

/// The format of the file of a trace of a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum LinkTraceFormat {
    /// A header and a row for each change, `time,rate,delay,loss`, every value with its unit.
    Csv,
    /// One line for each opportunity to deliver a packet, the time in whole milliseconds (the format of Mahimahi).
    Mahimahi,
}

impl LinkTraceFormat {
    /// The word of the file.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Mahimahi => "mahimahi",
        }
    }

    /// The format that the name of a file suggests: only `.csv` does; a Mahimahi trace has no usual extension.
    #[must_use]
    pub fn of_file(name: &str) -> Option<Self> {
        let extension = std::path::Path::new(name)
            .extension()?
            .to_string_lossy()
            .to_ascii_lowercase();
        (extension == "csv").then_some(Self::Csv)
    }
}

/// The trace of one direction of a link: the file it comes from and, once the file is read, the points.
///
/// In a scenario file `file` is written, with `format` unless the name ends in `.csv`, `window` for a Mahimahi trace and
/// `period` for a CSV file. The loader fills in the rest. `sha256` may be written as well: then the file must have exactly that
/// content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DirectionTrace {
    /// The file, a path relative to the directory of the scenario file. Read when the scenario is loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// `csv` or `mahimahi`. Left out, a name that ends in `.csv` is a CSV file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<LinkTraceFormat>,
    /// Mahimahi only: how many milliseconds of the trace make one rate. The opportunities of a window are counted and give its
    /// rate; a whole number of milliseconds. Default `100ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Duration>,
    /// CSV only: how long one pass of the trace is, from its start to where it begins again. Default: the last row lasts as long
    /// as the row before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<Duration>,
    /// The SHA-256 of the file. Written by the loader; if it is in the file, the content must match it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// The points. Written by the loader; a scenario with them needs no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<TracePoints>,
}

impl DirectionTrace {
    /// A reference to a file that is not read yet.
    #[must_use]
    pub fn from_file(file: &str) -> Self {
        Self {
            file: Some(file.to_owned()),
            format: None,
            window: None,
            period: None,
            sha256: None,
            points: None,
        }
    }

    /// A trace that holds its points, without a file.
    #[must_use]
    pub fn from_points(points: TracePoints) -> Self {
        Self {
            file: None,
            format: None,
            window: None,
            period: None,
            sha256: None,
            points: Some(points),
        }
    }

    /// The format of the file: the one written, or the one the name suggests.
    #[must_use]
    pub fn format_of_file(&self) -> Option<LinkTraceFormat> {
        self.format
            .or_else(|| self.file.as_deref().and_then(LinkTraceFormat::of_file))
    }
}

/// The rows of a trace: from the moment of a row on, until the next row, the link has the rate, the delay and the loss of the
/// row. A column that the file does not have is not written by the trace, and the link keeps what the file says for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TracePoints {
    /// The moment each row begins, in nanoseconds from the start of the trace. Rising; the first row need not begin at 0.
    #[serde(default)]
    at_ns: Vec<u64>,
    /// The rate of each row in bits per second. `0` is a row in which nothing is delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rate_bps: Option<Vec<u64>>,
    /// The delay of each row in nanoseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delay_ns: Option<Vec<u64>>,
    /// The loss of each row in millionths of a percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loss_micro_percent: Option<Vec<u32>>,
    /// Where one pass of the trace ends, in nanoseconds from its start: after the last row.
    #[serde(default)]
    period_ns: u64,
}

impl TracePoints {
    /// The points of a trace.
    ///
    /// # Errors
    ///
    /// A sentence if there is no row or no column, a column is not as long as the rows, a moment does not rise, the period is
    /// not after the last row, a loss is above 100 %, or there are more than [`MAX_TRACE_ROWS`] rows.
    pub fn new(
        at_ns: Vec<u64>,
        rate_bps: Option<Vec<u64>>,
        delay_ns: Option<Vec<u64>>,
        loss_micro_percent: Option<Vec<u32>>,
        period_ns: u64,
    ) -> Result<Self, String> {
        let points = Self {
            at_ns,
            rate_bps,
            delay_ns,
            loss_micro_percent,
            period_ns,
        };
        points.check()?;
        Ok(points)
    }

    /// The rules of the points: also for points that were read from a recorded scenario.
    ///
    /// # Errors
    ///
    /// A sentence, as for [`TracePoints::new`].
    pub fn check(&self) -> Result<(), String> {
        let rows = self.at_ns.len();
        if rows == 0 {
            return Err("the trace has no row".to_owned());
        }
        if rows > MAX_TRACE_ROWS {
            return Err(format!(
                "the trace has {rows} rows, more than the {MAX_TRACE_ROWS} that a trace may have"
            ));
        }
        if self.rate_bps.is_none() && self.delay_ns.is_none() && self.loss_micro_percent.is_none() {
            return Err("the trace writes no rate, no delay and no loss".to_owned());
        }
        for (name, length) in [
            ("rate", self.rate_bps.as_ref().map(Vec::len)),
            ("delay", self.delay_ns.as_ref().map(Vec::len)),
            ("loss", self.loss_micro_percent.as_ref().map(Vec::len)),
        ] {
            if length.is_some_and(|length| length != rows) {
                return Err(format!(
                    "the column '{name}' has {} values for {rows} rows",
                    length.unwrap_or(0)
                ));
            }
        }
        if let Some(at) = self.at_ns.windows(2).position(|pair| pair[1] <= pair[0]) {
            return Err(format!(
                "row {} does not begin after row {at}: the moments must rise",
                at + 1
            ));
        }
        if self.period_ns <= self.at_ns[rows - 1] {
            return Err("the period does not end after the last row".to_owned());
        }
        if self
            .loss_micro_percent
            .iter()
            .flatten()
            .any(|loss| Probability::from_micro_percent(*loss).is_none())
        {
            return Err("a loss is above 100 %".to_owned());
        }
        Ok(())
    }

    /// The number of rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.at_ns.len()
    }

    /// Whether there is no row. A trace that was checked has at least one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.at_ns.is_empty()
    }

    /// How long one pass of the trace is.
    #[must_use]
    pub fn period(&self) -> Duration {
        Duration::from_nanos(self.period_ns)
    }

    /// Whether the trace writes the rate.
    #[must_use]
    pub fn writes_rate(&self) -> bool {
        self.rate_bps.is_some()
    }

    /// Whether the trace writes the delay.
    #[must_use]
    pub fn writes_delay(&self) -> bool {
        self.delay_ns.is_some()
    }

    /// Whether the trace writes the loss.
    #[must_use]
    pub fn writes_loss(&self) -> bool {
        self.loss_micro_percent.is_some()
    }

    /// Whether some row delivers nothing (a rate of 0).
    #[must_use]
    pub fn has_stalls(&self) -> bool {
        self.rate_bps
            .as_ref()
            .is_some_and(|rates| rates.contains(&0))
    }

    /// The shortest time between two rows that have different rates: how fast the rate of the link has to be changed to follow
    /// the trace. `None` if the trace does not write the rate or never changes it. With `wrap` the step from the last row to the
    /// first of the next pass counts too.
    #[must_use]
    pub fn shortest_rate_step(&self, wrap: bool) -> Option<Duration> {
        let rates = self.rate_bps.as_ref()?;
        let length = |row: usize| self.row_length(row);
        let mut shortest: Option<u64> = None;
        for row in 0..rates.len() {
            let next = if row + 1 < rates.len() {
                rates[row + 1]
            } else if wrap {
                rates[0]
            } else {
                continue;
            };
            if next != rates[row] {
                shortest = Some(shortest.map_or(length(row), |s| s.min(length(row))));
            }
        }
        shortest.map(Duration::from_nanos)
    }

    /// How long row `row` lasts, in nanoseconds.
    fn row_length(&self, row: usize) -> u64 {
        let end = self.at_ns.get(row + 1).copied().unwrap_or(self.period_ns);
        end - self.at_ns[row]
    }

    /// What row `row` sets on the link. A row of rate 0 is the rate at which one opportunity is delivered in the row: a shaper
    /// cannot stop, and `StalledLink` of the backend says so.
    pub(crate) fn row_edit(&self, row: usize) -> ImpairmentsChange {
        let mut edit = ImpairmentsChange::default();
        if let Some(rates) = &self.rate_bps {
            let bits = if rates[row] == 0 {
                let frame_bits = u128::from(OPPORTUNITY_BYTES) * 8;
                u64::try_from(frame_bits * 1_000_000_000 / u128::from(self.row_length(row)))
                    .unwrap_or(u64::MAX)
                    .max(1)
            } else {
                rates[row]
            };
            edit.rate = Rate::from_bits_per_second(bits).map_or(Edit::Clear, Edit::Set);
        }
        if let Some(delays) = &self.delay_ns {
            edit.delay = Edit::Set(Duration::from_nanos(delays[row]));
        }
        if let Some(losses) = &self.loss_micro_percent {
            edit.loss = match Probability::from_micro_percent(losses[row]) {
                Some(p) if !p.is_zero() => Edit::Set(Loss::Random { p }),
                _ => Edit::Clear,
            };
        }
        edit
    }
}

/// Reads a CSV file: a header with `time` and any of `rate`, `delay`, `loss` (in any order), then a row for each change, every
/// value with its unit (`500ms,5mbit,40ms,0.1%`). Lines that start with `#` and empty lines are skipped. A `rate` of `0` is a row
/// in which nothing is delivered. The trace lasts `period`, or, left out, until the last row has lasted as long as the one before.
///
/// # Errors
///
/// A sentence with the line if something is not what the header says, the header is wrong, or the rows do not make a trace.
pub fn read_csv(text: &str, period: Option<Duration>) -> Result<TracePoints, String> {
    let mut columns: Option<Vec<&str>> = None;
    let mut at_ns: Vec<u64> = Vec::new();
    let (mut rates, mut delays, mut losses) = (Vec::new(), Vec::new(), Vec::new());
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let Some(header) = &columns else {
            let names: Vec<&str> = fields.iter().map(|name| name.trim()).collect();
            columns = Some(check_header(&names, number)?);
            continue;
        };
        if fields.len() != header.len() {
            return Err(format!(
                "line {number}: {} values for the {} columns of the header",
                fields.len(),
                header.len()
            ));
        }
        for (name, field) in header.iter().zip(&fields) {
            let bad = |why: ValueError| format!("line {number}: the {name} {why}");
            match *name {
                "time" => at_ns.push(Duration::from_str(field).map_err(bad)?.as_nanos()),
                "rate" => rates.push(match Rate::from_str(field) {
                    Ok(rate) => rate.bits_per_second(),
                    Err(ValueError::Zero { .. }) => 0,
                    Err(why) => return Err(bad(why)),
                }),
                "delay" => delays.push(Duration::from_str(field).map_err(bad)?.as_nanos()),
                _ => losses.push(Probability::from_str(field).map_err(bad)?.micro_percent()),
            }
        }
        if at_ns.len() > MAX_TRACE_ROWS {
            return Err(format!(
                "more than {MAX_TRACE_ROWS} rows (a trace may have at most that many)"
            ));
        }
    }
    let Some(header) = columns else {
        return Err("the file has no header: the first line names the columns, for example 'time,rate,delay,loss'".to_owned());
    };
    if at_ns.is_empty() {
        return Err("the file has a header and no row".to_owned());
    }
    if let Some(row) = at_ns.windows(2).position(|pair| pair[1] <= pair[0]) {
        return Err(format!(
            "row {} does not begin after row {}: the times must rise",
            row + 2,
            row + 1
        ));
    }
    let last = at_ns[at_ns.len() - 1];
    let period_ns = match period {
        Some(period) => period.as_nanos(),
        None if at_ns.len() >= 2 => last + (last - at_ns[at_ns.len() - 2]),
        None => {
            return Err("a trace of one row needs a `period`: how long the row lasts".to_owned());
        }
    };
    TracePoints::new(
        at_ns,
        header.contains(&"rate").then_some(rates),
        header.contains(&"delay").then_some(delays),
        header.contains(&"loss").then_some(losses),
        period_ns,
    )
}

/// The columns of a header, each once: `time` and at least one of the others.
fn check_header<'a>(names: &[&'a str], number: usize) -> Result<Vec<&'a str>, String> {
    let known = ["time", "rate", "delay", "loss"];
    let mut columns: Vec<&'a str> = Vec::new();
    for name in names {
        let Some(found) = known.iter().find(|k| k.eq_ignore_ascii_case(name)) else {
            return Err(format!(
                "line {number}: '{name}' is not a column of a trace: the columns are time, rate, delay and loss"
            ));
        };
        if columns.contains(found) {
            return Err(format!(
                "line {number}: the column '{found}' is written twice"
            ));
        }
        columns.push(found);
    }
    if !columns.contains(&"time") {
        return Err(format!("line {number}: the header has no 'time' column"));
    }
    if columns.len() < 2 {
        return Err(format!(
            "line {number}: the header names only 'time'; add rate, delay or loss"
        ));
    }
    Ok(columns)
}

/// Reads a Mahimahi trace: a line for each opportunity to deliver a packet, the time in whole milliseconds, not falling. The
/// opportunities in each `window` give the rate of that window; the trace lasts until its last line.
///
/// # Errors
///
/// A sentence with the line if a line is not a number of milliseconds or falls; or the window is not a whole number of
/// milliseconds; or the trace is empty or would need more rows than a trace may have.
pub fn read_mahimahi(text: &str, window: Duration) -> Result<TracePoints, String> {
    let window_ms = window.as_nanos() / 1_000_000;
    if !window.as_nanos().is_multiple_of(1_000_000) || window_ms == 0 {
        return Err(format!(
            "the window {window} is not a whole number of milliseconds, at least one"
        ));
    }
    let mut moments: Vec<u64> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let ms: u64 = line
            .parse()
            .map_err(|_| format!("line {number}: '{line}' is not a time in whole milliseconds"))?;
        if moments.last().is_some_and(|last| ms < *last) {
            return Err(format!(
                "line {number}: {ms} comes after {}: the times must not fall",
                moments[moments.len() - 1]
            ));
        }
        moments.push(ms);
    }
    let Some(&last) = moments.last() else {
        return Err("the trace has no line".to_owned());
    };
    if last == 0 {
        return Err("the last line is 0: the trace has no length".to_owned());
    }
    let rows = last.div_ceil(window_ms);
    if rows > MAX_TRACE_ROWS as u64 {
        let wanted = last.div_ceil(MAX_TRACE_ROWS as u64);
        return Err(format!(
            "the trace is {last} ms long, which makes {rows} windows of {window}: more than the {MAX_TRACE_ROWS} that a trace may have; use a `window` of at least {wanted}ms"
        ));
    }
    // An opportunity at the time t (in ms) is delivered in (t - 1, t]: it belongs to the window that ends at or after t
    let mut counts = vec![0_u64; usize::try_from(rows).unwrap_or(0)];
    for ms in &moments {
        let row = (ms.saturating_sub(1) / window_ms).min(rows - 1);
        counts[usize::try_from(row).unwrap_or(0)] += 1;
    }
    let mut at_ns = Vec::with_capacity(counts.len());
    let mut rate_bps = Vec::with_capacity(counts.len());
    for (row, count) in counts.iter().enumerate() {
        let begin_ms = row as u64 * window_ms;
        let width_ns = u128::from((last - begin_ms).min(window_ms)) * 1_000_000;
        let bits = u128::from(*count) * u128::from(OPPORTUNITY_BYTES) * 8;
        let rate = (bits * 1_000_000_000 + width_ns / 2) / width_ns;
        at_ns.push(begin_ms * 1_000_000);
        rate_bps.push(u64::try_from(rate).unwrap_or(u64::MAX));
    }
    TracePoints::new(at_ns, Some(rate_bps), None, None, last * 1_000_000)
}

/// The events of a trace model: for each direction the rows of its trace, again and again if the model repeats, in the order of
/// time. The link keeps what the file says for the keys that the trace does not write, and goes back to it when a trace that does
/// not repeat is over.
pub(crate) fn trace_events(
    link: &Link,
    model: &LinkModel,
    end: Duration,
) -> Result<Vec<Event>, Problem> {
    let mut writer = Writer::new(
        link,
        end,
        "use a longer `window`, or a shorter measured phase: every row is a change of the link, which takes a few milliseconds",
    );
    let begin = model.begin.map_or(0, Duration::as_nanos);
    let sources = [
        (Direction::Forward, &model.forward),
        (Direction::Reverse, &model.reverse),
        (Direction::Both, &model.both),
    ];
    // Row `Some(row)` of the trace of a direction begins, or, `None`, the trace is over
    let mut moves: Vec<(u64, Direction, Option<usize>, &TracePoints)> = Vec::new();
    for (direction, source) in sources {
        let Some(points) = source.as_ref().and_then(|source| source.points.as_ref()) else {
            continue;
        };
        let before = moves.len();
        let mut pass_start = begin;
        'passes: loop {
            for row in 0..points.len() {
                let at = pass_start.saturating_add(points.at_ns[row]);
                if at >= end.as_nanos() {
                    break 'passes;
                }
                if moves.len() - before >= MAX_EVENTS_PER_LINK {
                    return Err(writer.too_many());
                }
                moves.push((at, direction, Some(row), points));
            }
            pass_start = pass_start.saturating_add(points.period_ns);
            if !model.repeats() {
                if pass_start < end.as_nanos() {
                    moves.push((pass_start, direction, None, points));
                }
                break;
            }
            if pass_start >= end.as_nanos() {
                break;
            }
        }
    }
    // Stable: the rows that begin together keep the order forward, reverse, both
    moves.sort_by_key(|(at, ..)| *at);
    for (at, direction, row, points) in moves {
        let edit = row.map(|row| points.row_edit(row));
        writer.move_side(Duration::from_nanos(at), direction, edit.as_ref())?;
    }
    Ok(writer.events)
}

impl crate::scenario::Scenario {
    /// The files that traces of links of this scenario still have to read, each once, in the order of the scenario.
    #[must_use]
    pub fn unresolved_link_trace_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        for link in &self.topology.links {
            let Some(model) = &link.model else { continue };
            for (_, trace) in model.traces() {
                if trace.points.is_none()
                    && let Some(file) = &trace.file
                    && !files.contains(file)
                {
                    files.push(file.clone());
                }
            }
        }
        files
    }

    /// Reads the files of the traces of the links (D-083, D-091) and puts the points and the SHA-256 of the file into the
    /// scenario. `read` gives the content of a file by the path written in the scenario.
    ///
    /// Returns the problems found, with the path of each file in the scenario; the scenario is left as it was when there are any.
    pub fn resolve_link_traces(
        &mut self,
        mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
    ) -> Vec<Problem> {
        let mut problems = Vec::new();
        let mut resolved = self.clone();
        for (index, link) in resolved.topology.links.iter_mut().enumerate() {
            let Some(model) = &mut link.model else {
                continue;
            };
            for (name, source) in [
                ("forward", &mut model.forward),
                ("reverse", &mut model.reverse),
                ("both", &mut model.both),
            ] {
                let Some(trace) = source else { continue };
                let (Some(file), None) = (trace.file.clone(), &trace.points) else {
                    continue;
                };
                // Without a format the reader has said so
                let Some(format) = trace.format_of_file() else {
                    continue;
                };
                let path = format!("topology.links[{index}].model.{name}.file");
                let fail = |why: String| Problem::new(why).at_path(path.clone());
                let bytes = match read(&file) {
                    Ok(bytes) => bytes,
                    Err(why) => {
                        problems.push(fail(why));
                        continue;
                    }
                };
                let hash = crate::hash::sha256_hex(&bytes);
                if let Some(pinned) = &trace.sha256
                    && !pinned.eq_ignore_ascii_case(&hash)
                {
                    problems.push(
                        fail(format!(
                            "'{file}' is not the file the scenario was written for: its SHA-256 is {hash}, the scenario says {pinned}"
                        ))
                        .with_help("the file changed; check it, then write the new `sha256` or take it away"),
                    );
                    continue;
                }
                let made = String::from_utf8(bytes)
                    .map_err(|_| "it is not text in UTF-8".to_owned())
                    .and_then(|text| match format {
                        LinkTraceFormat::Csv => read_csv(&text, trace.period),
                        LinkTraceFormat::Mahimahi => {
                            read_mahimahi(&text, trace.window.unwrap_or(DEFAULT_WINDOW))
                        }
                    });
                match made {
                    Ok(points) => {
                        trace.format = Some(format);
                        trace.sha256 = Some(hash);
                        trace.points = Some(points);
                    }
                    Err(why) => problems.push(fail(format!("'{file}': {why}")).with_help(
                        match format {
                            LinkTraceFormat::Csv => "a CSV trace has a header (time,rate,delay,loss: time and at least one of the others) and a row for each change, every value with its unit (500ms,5mbit,40ms,0.1%)",
                            LinkTraceFormat::Mahimahi => "a Mahimahi trace has a line for each opportunity to deliver a packet: the time in whole milliseconds, not falling",
                        },
                    )),
                }
            }
        }
        if problems.is_empty() {
            *self = resolved;
        }
        problems
    }
}
