//! The HTML report: one file, inline CSS and inline SVG, nothing to fetch.

use std::fmt::Write as _;

use super::svg::{
    Bar, BarChart, IntervalPlot, Line, LineChart, Marker, bar_chart, interval_plot, line_chart,
    tick,
};
use super::text::{exit_text, workload_rows};
use super::{
    Detail, HEADLINE, Report, ReportError, RunReport, escape, failure_of, metric_value, ms, seconds,
};
use crate::result::{RunResult, WorkloadResult};
use crate::runset::RunSetResult;
use crate::series::Bin;

/// How many bins the table of the bins lists.
const LISTED: usize = 200;

/// How many bars the histogram of the delays has.
const BARS: usize = 40;

/// The stylesheet. The colours follow the scheme of the reader.
const STYLE: &str = "\
:root{--bg:#fff;--fg:#1b1f24;--muted:#57606a;--grid:#d8dee4;--card:#f6f8fa;--c1:#0b6cb3;--c2:#c2410c;--c3:#7a2fb0;--warn:#b42318}
@media (prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#e6edf3;--muted:#9da7b3;--grid:#30363d;--card:#161b22;--c1:#58a6ff;--c2:#f0883e;--c3:#d2a8ff;--warn:#ff7b72}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}
main{max-width:860px;margin:0 auto;padding:24px 16px 48px}
h1{font-size:1.5rem;margin:0 0 4px}h2{font-size:1.2rem;margin:32px 0 8px;border-bottom:1px solid var(--grid);padding-bottom:4px}h3{font-size:1rem;margin:20px 0 6px}
p{margin:6px 0}dl{display:grid;grid-template-columns:max-content 1fr;gap:2px 16px;margin:8px 0}dt{color:var(--muted)}dd{margin:0}
table{border-collapse:collapse;width:100%;margin:8px 0;font-variant-numeric:tabular-nums}
th,td{padding:4px 8px;border-bottom:1px solid var(--grid);text-align:left}th{color:var(--muted);font-weight:600}td.n,th.n{text-align:right}
.warn{border-left:4px solid var(--warn);background:var(--card);padding:6px 10px;margin:10px 0}
figure{margin:12px 0}figcaption{color:var(--muted);font-size:.85rem}
svg{width:100%;height:auto;display:block}
details{margin:10px 0}summary{cursor:pointer;color:var(--muted)}
code{font:.9em ui-monospace,monospace}
.grid{stroke:var(--grid)}.axis{stroke:var(--muted)}
.tick,.label,.legend,.marker-label{fill:var(--muted);font:11px system-ui,sans-serif}
.line{fill:none;stroke-width:1.6}
.line-recv,.line-p50{stroke:var(--c1)}.line-p95{stroke:var(--c2)}.line-p99{stroke:var(--c3)}
.line-offered{stroke:var(--muted);stroke-dasharray:4 3}.swatch{stroke:none}
.dot.line-recv,.dot.line-p50,.swatch.line-recv,.swatch.line-p50{fill:var(--c1)}
.dot.line-p95,.swatch.line-p95{fill:var(--c2)}.dot.line-p99,.swatch.line-p99{fill:var(--c3)}
.dot.line-offered,.swatch.line-offered{fill:var(--muted)}
.bar-lost{fill:var(--warn)}.bar-delay{fill:var(--c1)}
.marker{stroke:var(--warn);stroke-dasharray:3 3}.marker-p50{stroke:var(--c1)}.marker-p99{stroke:var(--c3)}
.interval{fill:var(--c1);opacity:.35}.estimate{stroke:var(--c1);stroke-width:2}.run{fill:var(--fg);opacity:.55}
";

/// A table: the headers (a `>` in front of a header makes the column right-aligned) and the rows (already escaped).
fn table(out: &mut String, headers: &[&str], rows: &[Vec<String>]) {
    out.push_str("<table><thead><tr>");
    for header in headers {
        match header.strip_prefix('>') {
            Some(name) => {
                let _ = write!(out, "<th class=\"n\">{}</th>", escape(name));
            }
            None => {
                let _ = write!(out, "<th>{}</th>", escape(header));
            }
        }
    }
    out.push_str("</tr></thead><tbody>");
    for row in rows {
        out.push_str("<tr>");
        for (index, value) in row.iter().enumerate() {
            let numeric = headers.get(index).is_some_and(|h| h.starts_with('>'));
            let _ = write!(
                out,
                "<td{}>{value}</td>",
                if numeric { " class=\"n\"" } else { "" }
            );
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>\n{STYLE}</style></head><body><main>\n{body}</main></body></html>\n",
        escape(title)
    )
}

/// The HTML report.
///
/// # Errors
///
/// [`ReportError`] for a run that was loaded without detail (see [`Report::load`]).
pub fn html(report: &Report) -> Result<String, ReportError> {
    match report {
        Report::Run(run) => html_run(run),
        Report::Set(set, _) => Ok(html_set(set)),
    }
}

fn html_run(run: &RunReport) -> Result<String, ReportError> {
    let result = &run.result;
    if run.details.len() != result.workloads.len() {
        return Err(ReportError::Bad {
            path: run.dir.clone(),
            message:
                "the report of a run as HTML needs the series and the records: load it with detail"
                    .to_owned(),
        });
    }
    let mut body = String::new();
    let _ = write!(
        body,
        "<h1>netscenario: {}</h1>",
        escape(&result.scenario.name)
    );
    summary(&mut body, result);
    for (workload, detail) in result.workloads.iter().zip(&run.details) {
        workload_section(&mut body, result, workload, detail);
    }
    timeline_section(&mut body, result);
    programs_section(&mut body, result);
    if !result.notes.is_empty() {
        body.push_str("<h2>Notes</h2><ul>");
        for note in &result.notes {
            let _ = write!(body, "<li>{}</li>", escape(note));
        }
        body.push_str("</ul>");
    }
    let _ = write!(
        body,
        "<p><small>Made from <code>result.json</code>. The scenario is <code>{}</code>.</small></p>",
        escape(&result.scenario.hash)
    );
    Ok(page(
        &format!("netscenario: {}", result.scenario.name),
        &body,
    ))
}

fn summary(out: &mut String, result: &RunResult) {
    let provenance = &result.provenance;
    let details: Vec<String> = provenance.backend_details.values().cloned().collect();
    let seed = match (result.seed.value, result.seed.drawn) {
        (Some(value), true) => format!("{value} (drawn)"),
        (Some(value), false) => value.to_string(),
        (None, _) => "none (the backend cannot apply one)".to_owned(),
    };
    out.push_str("<dl>");
    let _ = write!(
        out,
        "<dt>Backend</dt><dd>{} ({})</dd><dt>Seed</dt><dd>{}</dd><dt>Duration</dt><dd>{} s, then {} s for the packets in the network</dd><dt>Run</dt><dd>{}</dd><dt>Machine</dt><dd>{}, {}</dd>",
        escape(&provenance.backend),
        escape(&details.join(", ")),
        escape(&seed),
        seconds(result.timing.duration_ns),
        seconds(result.timing.settle_ns),
        escape(&provenance.started_at),
        escape(provenance.kernel.as_deref().unwrap_or("kernel unknown")),
        escape(
            provenance
                .cpu_model
                .as_deref()
                .unwrap_or("processor unknown")
        )
    );
    out.push_str("</dl>");
    if let Some(interrupted) = result.timing.interrupted_ns {
        let _ = write!(
            out,
            "<p class=\"warn\"><strong>Interrupted</strong> after {} s of {} s: this is the result of the part that ran.</p>",
            seconds(interrupted),
            seconds(result.timing.duration_ns)
        );
    }
    for reason in &result.seed.not_reproducible_because {
        let _ = write!(
            out,
            "<p class=\"warn\"><strong>Warning:</strong> the loss of this run does not repeat from its seed: {}</p>",
            escape(reason)
        );
    }
}

/// The marks of the events of the timeline: a dashed line at the moment the file gives, labelled with the number of the event.
fn event_marks(result: &RunResult) -> Vec<Marker> {
    result
        .events
        .iter()
        .map(|event| Marker {
            x: event.scheduled_ns as f64 / 1e9,
            label: event.event.to_string(),
            class: "marker",
        })
        .collect()
}

/// The width of a bin in seconds.
fn bin_seconds(workload: &WorkloadResult, bins: &[Bin]) -> f64 {
    let ns = workload
        .series
        .as_ref()
        .map(|series| series.bin_ns)
        .or_else(|| bins.get(1).map(|second| second.start_ns - bins[0].start_ns))
        .unwrap_or(1_000_000_000);
    ns as f64 / 1e9
}

fn workload_section(
    out: &mut String,
    result: &RunResult,
    workload: &WorkloadResult,
    detail: &Detail,
) {
    let _ = write!(
        out,
        "<h2>{}</h2><p>{} → {}, {} packets of {} bytes</p>",
        escape(&workload.id),
        escape(&workload.from),
        escape(&workload.to),
        workload.stats.sent,
        workload.packet_size_bytes
    );
    if let Some(reason) = failure_of(result, workload) {
        let _ = write!(out, "<p class=\"warn\">{}</p>", escape(&reason));
    }
    let rows: Vec<Vec<String>> = workload_rows(workload)
        .into_iter()
        .map(|(label, value)| vec![escape(label), escape(&value)])
        .collect();
    table(out, &["Measure", ">Value"], &rows);
    let marks = event_marks(result);
    let bin_s = bin_seconds(workload, &detail.bins);
    let span = detail
        .bins
        .last()
        .map_or(0.0, |bin| bin.start_ns as f64 / 1e9 + bin_s)
        .max(result.timing.duration_ns as f64 / 1e9);
    if detail.bins.is_empty() {
        out.push_str(
            "<p>This result has no series by time bin, so the charts over time are left out.</p>",
        );
    } else {
        throughput_chart(out, workload, &detail.bins, bin_s, span, &marks);
        delay_chart(out, workload, &detail.bins, bin_s, span, &marks);
        loss_chart(out, workload, &detail.bins, bin_s, span, &marks);
        bins_table(out, workload, &detail.bins, bin_s);
    }
    histogram(out, workload, detail);
}

fn throughput_chart(
    out: &mut String,
    workload: &WorkloadResult,
    bins: &[Bin],
    bin_s: f64,
    span: f64,
    marks: &[Marker],
) {
    let middle = |bin: &Bin| bin.start_ns as f64 / 1e9 + bin_s / 2.0;
    let received: Vec<f64> = bins
        .iter()
        .map(|b| b.by_arrival.bits as f64 / bin_s)
        .collect();
    let offered: Vec<f64> = bins
        .iter()
        .map(|b| b.by_send.sent as f64 * workload.packet_size_bytes as f64 * 8.0 / bin_s)
        .collect();
    let peak = received.iter().chain(&offered).copied().fold(0.0, f64::max);
    let (scale, unit) = if peak >= 1e9 {
        (1e9, "gbit/s")
    } else if peak >= 1e6 {
        (1e6, "mbit/s")
    } else {
        (1e3, "kbit/s")
    };
    let line = |name, class, values: &[f64]| Line {
        name,
        class,
        points: bins
            .iter()
            .zip(values)
            .map(|(b, v)| Some((middle(b), v / scale)))
            .collect(),
    };
    let chart = LineChart {
        title: &format!("{}: throughput over time", workload.id),
        description: format!(
            "The payload rate that was offered by the sender and the rate that arrived, in {unit}, in bins of {} s; the dashed vertical lines are the events of the timeline.",
            tick(bin_s)
        ),
        x_max: span,
        y_label: unit,
        y_format: &tick,
        lines: vec![
            line("arrived", "line-recv", &received),
            line("offered", "line-offered", &offered),
        ],
        marks,
    };
    figure(
        out,
        &line_chart(&chart),
        &format!(
            "Throughput: the payload that arrived in each bin and the payload that was sent in it. The last bin is usually part of a bin and is not scaled up. Bins of {} s.",
            tick(bin_s)
        ),
    );
}

fn delay_chart(
    out: &mut String,
    workload: &WorkloadResult,
    bins: &[Bin],
    bin_s: f64,
    span: f64,
    marks: &[Marker],
) {
    let line = |name, class, pick: fn(&crate::series::BinDelay) -> u64| Line {
        name,
        class,
        points: bins
            .iter()
            .map(|b| {
                b.by_arrival
                    .delay
                    .as_ref()
                    .map(|d| (b.start_ns as f64 / 1e9 + bin_s / 2.0, pick(d) as f64 / 1e6))
            })
            .collect(),
    };
    let chart = LineChart {
        title: &format!("{}: one-way delay over time", workload.id),
        description: "The median, the 95th and the 99th percentile of the one-way delay of the packets that arrived in each bin, in milliseconds; a gap is a bin in which nothing arrived.".to_owned(),
        x_max: span,
        y_label: "ms",
        y_format: &tick,
        lines: vec![
            line("p50", "line-p50", |d| d.p50_ns),
            line("p95", "line-p95", |d| d.p95_ns),
            line("p99", "line-p99", |d| d.p99_ns),
        ],
        marks,
    };
    figure(
        out,
        &line_chart(&chart),
        "Delay of the packets that arrived in each bin (nearest-rank percentiles). A gap means that nothing arrived in the bin.",
    );
}

fn loss_chart(
    out: &mut String,
    workload: &WorkloadResult,
    bins: &[Bin],
    bin_s: f64,
    span: f64,
    marks: &[Marker],
) {
    let bars: Vec<Bar> = bins
        .iter()
        .filter(|b| b.by_send.sent > 0)
        .map(|b| Bar {
            x0: b.start_ns as f64 / 1e9,
            x1: b.start_ns as f64 / 1e9 + bin_s,
            value: b.by_send.lost as f64 / b.by_send.sent as f64 * 100.0,
        })
        .collect();
    let lost_total: u64 = bins.iter().map(|b| b.by_send.lost).sum();
    let chart = BarChart {
        title: &format!("{}: lost packets over time", workload.id),
        description: format!(
            "The share of the packets sent in each bin that did not arrive, in percent; {lost_total} packets were lost in all."
        ),
        class: "bar-lost",
        x_range: (0.0, span),
        x_label: "seconds from the start of the measured phase",
        x_format: &tick,
        y_label: "% of the packets sent in the bin",
        y_format: &tick,
        bars,
        marks,
        y_floor: 1.0,
    };
    figure(
        out,
        &bar_chart(&chart),
        &format!(
            "Packets that did not arrive, by the bin in which they were sent ({lost_total} in all). No bar means that every packet of the bin arrived."
        ),
    );
}

fn bins_table(out: &mut String, workload: &WorkloadResult, bins: &[Bin], bin_s: f64) {
    let rows: Vec<Vec<String>> = bins
        .iter()
        .take(LISTED)
        .map(|b| {
            let delay = |pick: fn(&crate::series::BinDelay) -> u64| {
                b.by_arrival
                    .delay
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), |d| ms(pick(d)))
            };
            vec![
                tick(b.start_ns as f64 / 1e9),
                b.by_send.sent.to_string(),
                b.by_send.lost.to_string(),
                b.by_arrival.received.to_string(),
                format!("{:.1}", b.by_arrival.bits as f64 / bin_s / 1e3),
                delay(|d| d.p50_ns),
                delay(|d| d.p99_ns),
            ]
        })
        .collect();
    let _ = write!(
        out,
        "<details><summary>The bins of {} as a table</summary>",
        escape(&workload.id)
    );
    table(
        out,
        &[
            "Starts (s)",
            ">Sent",
            ">Lost",
            ">Arrived",
            ">kbit/s",
            ">p50",
            ">p99",
        ],
        &rows,
    );
    if bins.len() > LISTED {
        let _ = write!(
            out,
            "<p>The first {LISTED} of {} bins; the others are in <code>{}</code>.</p>",
            bins.len(),
            escape(
                &workload
                    .series
                    .as_ref()
                    .map_or_else(String::new, |s| s.file.clone())
            )
        );
    }
    out.push_str("</details>");
}

fn histogram(out: &mut String, workload: &WorkloadResult, detail: &Detail) {
    let Some(summary) = &workload.stats.delay else {
        out.push_str("<p>No packet arrived, so there is no distribution of delays.</p>");
        return;
    };
    if detail.delays_ns.is_empty() {
        return;
    }
    let low = summary.min_ns as f64 / 1e6;
    let mut high = summary.p999_ns as f64 / 1e6;
    if high <= low {
        high = low + 1.0;
    }
    let width = (high - low) / BARS as f64;
    let mut counts = [0_u64; BARS];
    for delay in &detail.delays_ns {
        let place = (((*delay as f64 / 1e6 - low) / width) as usize).min(BARS - 1);
        counts[place] += 1;
    }
    let bars: Vec<Bar> = counts
        .iter()
        .enumerate()
        .map(|(index, count)| Bar {
            x0: low + index as f64 * width,
            x1: low + (index + 1) as f64 * width,
            value: *count as f64,
        })
        .collect();
    let marks = [
        Marker {
            x: summary.p50_ns as f64 / 1e6,
            label: "p50".to_owned(),
            class: "marker-p50",
        },
        Marker {
            x: summary.p99_ns as f64 / 1e6,
            label: "p99".to_owned(),
            class: "marker-p99",
        },
    ];
    let chart = BarChart {
        title: &format!("{}: distribution of the one-way delay", workload.id),
        description: format!(
            "How many packets had which one-way delay, from {} to {}; the marks are the median and the 99th percentile.",
            ms(summary.min_ns),
            ms(summary.p999_ns)
        ),
        class: "bar-delay",
        x_range: (low, high),
        x_label: "one-way delay (ms)",
        x_format: &tick,
        y_label: "packets",
        y_format: &tick,
        bars,
        marks: &marks,
        y_floor: 1.0,
    };
    figure(
        out,
        &bar_chart(&chart),
        "Delays of the packets that arrived (the first arrival of each). Delays above the 99.9th percentile are counted in the last bar.",
    );
}

fn figure(out: &mut String, svg: &str, caption: &str) {
    let _ = write!(
        out,
        "<figure>{svg}<figcaption>{}</figcaption></figure>",
        escape(caption)
    );
}

fn timeline_section(out: &mut String, result: &RunResult) {
    if result.events.is_empty() {
        return;
    }
    out.push_str("<h2>Timeline</h2><p>The numbers on the dashed lines of the charts are the numbers of the events.</p>");
    let rows: Vec<Vec<String>> = result
        .events
        .iter()
        .map(|event| {
            let done = match (event.packet, event.held_ns) {
                (Some(packet), held) => format!(
                    "before packet {packet}, which waited {}",
                    ms(held.unwrap_or(0))
                ),
                (None, _) => format!(
                    "{} late",
                    ms(event.applied_ns.saturating_sub(event.scheduled_ns))
                ),
            };
            vec![
                event.event.to_string(),
                format!("{} s", seconds(event.scheduled_ns)),
                escape(&event.link),
                escape(event.direction.as_deref().unwrap_or("both")),
                escape(&event.kind),
                escape(&done),
            ]
        })
        .collect();
    table(
        out,
        &["Event", ">At", "Link", "Direction", "Step", "Done"],
        &rows,
    );
    let recoveries: Vec<Vec<String>> = result
        .workloads
        .iter()
        .flat_map(|w| w.recoveries.iter().map(move |r| (w, r)))
        .map(|(workload, recovery)| {
            let time = recovery.time_ns.map_or_else(
                || {
                    format!(
                        "not recovered ({})",
                        recovery.reason.as_deref().unwrap_or("no reason given")
                    )
                },
                |ns| format!("{} s", seconds(ns)),
            );
            vec![
                escape(&workload.id),
                format!("event {} ({})", recovery.event, escape(&recovery.kind)),
                escape(&time),
            ]
        })
        .collect();
    if !recoveries.is_empty() {
        out.push_str("<h3>Time to recover</h3>");
        table(out, &["Workload", "After", ">Time to recover"], &recoveries);
    }
}

fn programs_section(out: &mut String, result: &RunResult) {
    if result.programs.is_empty() {
        return;
    }
    out.push_str("<h2>Programs</h2>");
    let rows: Vec<Vec<String>> = result
        .programs
        .iter()
        .map(|p| {
            vec![
                escape(&p.id),
                escape(&p.node),
                escape(&p.ended),
                escape(&exit_text(p)),
            ]
        })
        .collect();
    table(out, &["Program", "Node", "Ended", "Exit"], &rows);
    let metrics: Vec<Vec<String>> = result
        .programs
        .iter()
        .flat_map(|p| p.metrics.iter().map(move |m| (p, m)))
        .map(|(p, m)| {
            vec![
                escape(&p.id),
                escape(&m.name),
                escape(&format!(
                    "{}{}",
                    m.last,
                    m.unit
                        .as_deref()
                        .map_or_else(String::new, |u| format!(" {u}"))
                )),
                m.min.to_string(),
                m.max.to_string(),
                m.count.to_string(),
            ]
        })
        .collect();
    if !metrics.is_empty() {
        table(
            out,
            &["Program", "Metric", ">Last", ">Min", ">Max", ">Readings"],
            &metrics,
        );
    }
}

fn html_set(set: &RunSetResult) -> String {
    let mut body = String::new();
    let _ = write!(
        body,
        "<h1>netscenario: {}, {} repetitions</h1>",
        escape(&set.scenario.name),
        set.repetitions
    );
    let method = match set.confidence.method {
        crate::bootstrap::Method::Percentile => "percentile",
        crate::bootstrap::Method::ExpandedPercentile => "expanded percentile",
    };
    let seed = if set.seed.drawn {
        format!(
            "{} (drawn; repeat the set with --seed {})",
            set.seed.value, set.seed.value
        )
    } else {
        set.seed.value.to_string()
    };
    let _ = write!(
        body,
        "<dl><dt>Backend</dt><dd>{}</dd><dt>Seed</dt><dd>{}</dd><dt>Intervals</dt><dd>around the {}, {}&nbsp;%, {} draws, {} method</dd></dl>",
        escape(&set.provenance.backend),
        escape(&seed),
        set.confidence.statistic.name(),
        tick(set.confidence.level * 100.0),
        set.confidence.resamples,
        method
    );
    let unreproducible = set.runs.iter().filter(|run| !run.loss_reproducible).count();
    if unreproducible > 0 {
        let _ = write!(
            body,
            "<p class=\"warn\"><strong>Warning:</strong> the loss of {unreproducible} of the runs does not repeat from its seed.</p>"
        );
    }
    body.push_str("<h2>Headline metrics</h2><p>Each dot is one run, the bar is the interval around the mean, the line is the mean.</p>");
    for metric in set
        .metrics
        .iter()
        .filter(|m| HEADLINE.iter().any(|suffix| m.name.ends_with(suffix)))
    {
        let name = metric.name.clone();
        let format = move |value: f64| {
            // Axis labels carry the unit once per label
            metric_value(&name, value)
        };
        let plot = IntervalPlot {
            title: &metric.name,
            description: format!(
                "The value of {} in each of {} runs, and its interval from {} to {}.",
                metric.name,
                metric.values.len(),
                metric_value(&metric.name, metric.interval.low),
                metric_value(&metric.name, metric.interval.high)
            ),
            values: &metric.values,
            estimate: metric.interval.estimate,
            interval: (metric.interval.low, metric.interval.high),
            format: &format,
        };
        let _ = write!(
            body,
            "<h3>{}</h3><p>{} (interval {} to {}, spread {})</p>",
            escape(&metric.name),
            escape(&metric_value(&metric.name, metric.interval.estimate)),
            escape(&metric_value(&metric.name, metric.interval.low)),
            escape(&metric_value(&metric.name, metric.interval.high)),
            escape(&metric_value(&metric.name, metric.stddev))
        );
        body.push_str(&interval_plot(&plot));
    }
    let rows: Vec<Vec<String>> = set
        .metrics
        .iter()
        .map(|m| {
            vec![
                escape(&m.name),
                escape(&metric_value(&m.name, m.mean)),
                escape(&metric_value(&m.name, m.median)),
                escape(&metric_value(&m.name, m.min)),
                escape(&metric_value(&m.name, m.max)),
                escape(&format!(
                    "{} to {}",
                    metric_value(&m.name, m.interval.low),
                    metric_value(&m.name, m.interval.high)
                )),
            ]
        })
        .collect();
    let _ = write!(
        body,
        "<details><summary>All {} metrics</summary>",
        set.metrics.len()
    );
    table(
        &mut body,
        &["Metric", ">Mean", ">Median", ">Min", ">Max", ">Interval"],
        &rows,
    );
    body.push_str("</details>");
    if !set.partial.is_empty() {
        body.push_str("<h2>Without an interval</h2><p>These metrics are missing in some runs, so an interval from the others would be a made-up number.</p><ul>");
        for missing in &set.partial {
            let _ = write!(
                body,
                "<li><code>{}</code>: in {} of {} runs</li>",
                escape(&missing.name),
                missing.present_in,
                missing.of
            );
        }
        body.push_str("</ul>");
    }
    body.push_str("<h2>Runs</h2>");
    let runs: Vec<Vec<String>> = set
        .runs
        .iter()
        .map(|run| {
            vec![
                escape(&run.directory),
                run.seed.to_string(),
                if run.loss_reproducible { "yes" } else { "no" }.to_owned(),
            ]
        })
        .collect();
    table(
        &mut body,
        &["Run", ">Seed", "Loss repeats from the seed"],
        &runs,
    );
    page(
        &format!(
            "netscenario: {}, {} repetitions",
            set.scenario.name, set.repetitions
        ),
        &body,
    )
}
