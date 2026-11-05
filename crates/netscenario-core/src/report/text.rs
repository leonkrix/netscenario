//! The Markdown and the JUnit report.

use std::fmt::Write as _;

use super::{
    HEADLINE, Report, escape, failure_of, link_shut_on_purpose, metric_value, ms, percent, rate,
    seconds,
};
use crate::result::{ProgramResult, RunResult, WorkloadResult};
use crate::runset::RunSetResult;

// ---- Markdown ----

/// The Markdown report.
#[must_use]
pub fn markdown(report: &Report) -> String {
    match report {
        Report::Run(run) => markdown_run(&run.result),
        Report::Set(set, _) => markdown_set(set),
    }
}

/// A cell of a table: the pipe would end it.
pub(crate) fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn markdown_run(result: &RunResult) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "## netscenario: {}\n", result.scenario.name);
    let provenance = &result.provenance;
    let details: Vec<String> = provenance.backend_details.values().cloned().collect();
    let seed = match (result.seed.value, result.seed.drawn) {
        (Some(value), true) => format!("{value} (drawn)"),
        (Some(value), false) => value.to_string(),
        (None, _) => "none (the backend cannot apply one)".to_owned(),
    };
    let _ = writeln!(text, "| | |\n| --- | --- |");
    let _ = writeln!(
        text,
        "| Backend | {} ({}) |",
        cell(&provenance.backend),
        cell(&details.join(", "))
    );
    let _ = writeln!(text, "| Seed | {seed} |");
    let _ = writeln!(
        text,
        "| Duration | {} s, then {} s for the packets in the network |",
        seconds(result.timing.duration_ns),
        seconds(result.timing.settle_ns)
    );
    let _ = writeln!(text, "| Run | {} |", cell(&provenance.started_at));
    let _ = writeln!(
        text,
        "| Machine | {}, {} |",
        cell(provenance.kernel.as_deref().unwrap_or("kernel unknown")),
        cell(
            provenance
                .cpu_model
                .as_deref()
                .unwrap_or("processor unknown")
        )
    );
    if let Some(interrupted) = result.timing.interrupted_ns {
        let _ = writeln!(
            text,
            "\n> **Interrupted** after {} s of {} s: this is the result of the part that ran.",
            seconds(interrupted),
            seconds(result.timing.duration_ns)
        );
    }
    for reason in &result.seed.not_reproducible_because {
        let _ = writeln!(
            text,
            "\n> **Warning:** the loss of this run does not repeat from its seed: {reason}"
        );
    }
    markdown_workloads(&mut text, result);
    markdown_tcp(&mut text, result);
    markdown_timeline(&mut text, result);
    markdown_programs(&mut text, result);
    if !result.notes.is_empty() {
        let _ = writeln!(text, "\n### Notes\n");
        for note in &result.notes {
            let _ = writeln!(text, "- {note}");
        }
    }
    text
}

/// The rows of the table of workloads: a label and the value of each workload.
pub(crate) fn workload_rows(workload: &WorkloadResult) -> Vec<(&'static str, String)> {
    let stats = &workload.stats;
    let mut rows = vec![
        ("Sent", stats.sent.to_string()),
        ("Received", stats.received.to_string()),
        (
            "Lost",
            format!("{} ({})", stats.lost, percent(stats.loss_ratio)),
        ),
        ("Reordered", stats.reordered.to_string()),
        ("Duplicates", stats.duplicates.to_string()),
    ];
    let delay = |pick: fn(&crate::stats::DelaySummary) -> u64| {
        stats
            .delay
            .as_ref()
            .map_or_else(|| "no packet arrived".to_owned(), |d| ms(pick(d)))
    };
    rows.push(("Delay p50", delay(|d| d.p50_ns)));
    rows.push(("Delay p95", delay(|d| d.p95_ns)));
    rows.push(("Delay p99", delay(|d| d.p99_ns)));
    rows.push(("Delay max", delay(|d| d.max_ns)));
    rows.push((
        "Jitter (RFC 3550)",
        stats
            .jitter
            .as_ref()
            .map_or_else(|| "n/a".to_owned(), |j| ms(j.mean_ns)),
    ));
    let throughput = if workload.duration_ns > 0 {
        rate(workload.payload_received_bytes() as f64 * 8.0 / (workload.duration_ns as f64 / 1e9))
    } else {
        "n/a".to_owned()
    };
    rows.push(("Throughput (mean)", throughput));
    rows.push((
        "Longest loss run",
        format!("{} packets", stats.loss_runs.longest),
    ));
    rows
}

fn markdown_workloads(text: &mut String, result: &RunResult) {
    // A scenario of TCP workloads only has its section below
    if result.workloads.is_empty() && !result.tcp.is_empty() {
        return;
    }
    let _ = writeln!(text, "\n### Workloads\n");
    if result.workloads.is_empty() {
        let _ = writeln!(text, "None.");
        return;
    }
    let ids: Vec<String> = result.workloads.iter().map(|w| cell(&w.id)).collect();
    let _ = writeln!(text, "| | {} |", ids.join(" | "));
    let _ = writeln!(text, "| --- |{}", " ---: |".repeat(ids.len()));
    let flows: Vec<String> = result
        .workloads
        .iter()
        .map(|w| cell(&format!("{} → {}", w.from, w.to)))
        .collect();
    let _ = writeln!(text, "| Flow | {} |", flows.join(" | "));
    let columns: Vec<Vec<(&str, String)>> = result.workloads.iter().map(workload_rows).collect();
    for (index, (label, _)) in columns[0].iter().enumerate() {
        let values: Vec<String> = columns
            .iter()
            .map(|column| cell(&column[index].1))
            .collect();
        let _ = writeln!(text, "| {label} | {} |", values.join(" | "));
    }
    for workload in &result.workloads {
        if let Some(reason) = failure_of(result, workload) {
            let _ = writeln!(text, "\n> **{}:** {reason}", cell(&workload.id));
        }
    }
    markdown_echo(text, result);
}

/// The round trip of the workloads that were echoed, one column each (D-087).
fn markdown_echo(text: &mut String, result: &RunResult) {
    let echoed: Vec<&WorkloadResult> = result
        .workloads
        .iter()
        .filter(|workload| workload.echo.is_some())
        .collect();
    if echoed.is_empty() {
        return;
    }
    let _ = writeln!(text, "\n### Echo\n");
    let ids: Vec<String> = echoed.iter().map(|w| cell(&w.id)).collect();
    let _ = writeln!(text, "| | {} |", ids.join(" | "));
    let _ = writeln!(text, "| --- |{}", " ---: |".repeat(ids.len()));
    let columns: Vec<Vec<(&str, String)>> = echoed.iter().map(|w| echo_rows(w)).collect();
    for (index, (label, _)) in columns[0].iter().enumerate() {
        let values: Vec<String> = columns
            .iter()
            .map(|column| cell(&column[index].1))
            .collect();
        let _ = writeln!(text, "| {label} | {} |", values.join(" | "));
    }
}

/// The rows of the table of echoes: the packets that came back and their round trip.
fn echo_rows(workload: &WorkloadResult) -> Vec<(&'static str, String)> {
    let Some(echo) = &workload.echo else {
        return Vec::new();
    };
    let stats = &echo.stats;
    let trip = |pick: fn(&crate::stats::DelaySummary) -> u64| {
        stats
            .delay
            .as_ref()
            .map_or_else(|| "no packet came back".to_owned(), |d| ms(pick(d)))
    };
    vec![
        ("Came back", stats.received.to_string()),
        (
            "Not back",
            format!("{} ({})", stats.lost, percent(stats.loss_ratio)),
        ),
        ("Round trip min", trip(|d| d.min_ns)),
        ("Round trip p50", trip(|d| d.p50_ns)),
        ("Round trip p95", trip(|d| d.p95_ns)),
        ("Round trip p99", trip(|d| d.p99_ns)),
        ("Round trip max", trip(|d| d.max_ns)),
    ]
}

/// The TCP workloads, one column each (D-086).
fn markdown_tcp(text: &mut String, result: &RunResult) {
    if result.tcp.is_empty() {
        return;
    }
    let _ = writeln!(text, "\n### TCP workloads\n");
    let ids: Vec<String> = result.tcp.iter().map(|t| cell(&t.id)).collect();
    let _ = writeln!(text, "| | {} |", ids.join(" | "));
    let _ = writeln!(text, "| --- |{}", " ---: |".repeat(ids.len()));
    let columns: Vec<Vec<(&str, String)>> = result.tcp.iter().map(tcp_rows).collect();
    for (index, (label, _)) in columns[0].iter().enumerate() {
        let values: Vec<String> = columns
            .iter()
            .map(|column| cell(&column[index].1))
            .collect();
        let _ = writeln!(text, "| {label} | {} |", values.join(" | "));
    }
    for tcp in result.tcp.iter().filter(|tcp| tcp.failed_connections > 0) {
        let _ = writeln!(
            text,
            "\n> **{}:** {} of {} connections failed: {}",
            cell(&tcp.id),
            tcp.failed_connections,
            tcp.connections,
            cell(tcp.first_error.as_deref().unwrap_or("no reason was given"))
        );
    }
    let _ = writeln!(
        text,
        "\n> A run with TCP is {}: {}",
        result.reproducibility.class,
        cell(&result.reproducibility.because.join("; "))
    );
}

/// The rows of one TCP workload.
fn tcp_rows(tcp: &crate::result::TcpResult) -> Vec<(&'static str, String)> {
    let delay = |summary: &Option<crate::stats::DelaySummary>,
                 pick: fn(&crate::stats::DelaySummary) -> u64| {
        summary
            .as_ref()
            .map_or_else(|| "n/a".to_owned(), |d| ms(pick(d)))
    };
    let mut rows = vec![
        ("Flow", format!("{} → {}", tcp.from, tcp.to)),
        ("Kind", tcp.kind.clone()),
        (
            "Congestion control",
            tcp.congestion.clone().unwrap_or_else(|| "n/a".to_owned()),
        ),
        (
            "Connections (failed)",
            format!("{} ({})", tcp.connections, tcp.failed_connections),
        ),
        ("Payload sent", format!("{} bytes", tcp.bytes_sent)),
        ("Payload received", format!("{} bytes", tcp.bytes_received)),
    ];
    if let Some(requests) = &tcp.requests {
        rows.push((
            "Requests answered",
            format!("{} of {}", requests.answered, requests.sent),
        ));
        rows.push(("Latency p50", delay(&requests.latency, |d| d.p50_ns)));
        rows.push(("Latency p99", delay(&requests.latency, |d| d.p99_ns)));
        rows.push(("Round trip p50", delay(&requests.round_trip, |d| d.p50_ns)));
    } else {
        let done = tcp
            .transfers
            .iter()
            .filter(|t| t.ok)
            .map(crate::result::TransferInfo::duration_ns)
            .max();
        rows.push((
            "Completed after",
            done.map_or_else(
                || "not completed".to_owned(),
                |ns| format!("{} s", seconds(ns)),
            ),
        ));
        rows.push((
            "Goodput (mean)",
            tcp.goodput
                .as_ref()
                .and_then(|g| g.mean_bits_per_second)
                .map_or_else(|| "n/a".to_owned(), |bits| rate(bits as f64)),
        ));
    }
    rows.push((
        "Round trip time of the kernel p50",
        delay(&tcp.rtt, |d| d.p50_ns),
    ));
    rows.push((
        "Retransmitted segments",
        tcp.retransmitted_segments.to_string(),
    ));
    rows
}

fn markdown_timeline(text: &mut String, result: &RunResult) {
    if result.events.is_empty() {
        return;
    }
    let _ = writeln!(text, "\n### Timeline\n");
    let _ = writeln!(
        text,
        "| At | Link | Direction | Step | Done |\n| ---: | --- | --- | --- | --- |"
    );
    for event in &result.events {
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
        let _ = writeln!(
            text,
            "| {} s | {} | {} | {} | {} |",
            seconds(event.scheduled_ns),
            cell(&event.link),
            event.direction.as_deref().unwrap_or("both"),
            cell(&event.kind),
            done
        );
    }
    let recoveries: Vec<_> = result
        .workloads
        .iter()
        .flat_map(|w| w.recoveries.iter().map(move |r| (w, r)))
        .collect();
    if recoveries.is_empty() {
        return;
    }
    let _ = writeln!(text, "\n### Time to recover\n");
    let _ = writeln!(
        text,
        "| Workload | After | Link | Time to recover |\n| --- | --- | --- | ---: |"
    );
    for (workload, recovery) in recoveries {
        let time = recovery.time_ns.map_or_else(
            || {
                format!(
                    "not recovered ({})",
                    cell(recovery.reason.as_deref().unwrap_or("no reason given"))
                )
            },
            |ns| format!("{} s", seconds(ns)),
        );
        let _ = writeln!(
            text,
            "| {} | event {} ({}) | {} | {} |",
            cell(&workload.id),
            recovery.event,
            cell(&recovery.kind),
            cell(&recovery.link),
            time
        );
    }
}

fn markdown_programs(text: &mut String, result: &RunResult) {
    if result.programs.is_empty() {
        return;
    }
    let _ = writeln!(text, "\n### Programs\n");
    let _ = writeln!(
        text,
        "| Program | Node | Ended | Exit |\n| --- | --- | --- | --- |"
    );
    for program in &result.programs {
        let _ = writeln!(
            text,
            "| {} | {} | {} | {} |",
            cell(&program.id),
            cell(&program.node),
            cell(&program.ended),
            exit_text(program)
        );
    }
    let with_metrics: Vec<&ProgramResult> = result
        .programs
        .iter()
        .filter(|p| !p.metrics.is_empty())
        .collect();
    if with_metrics.is_empty() {
        return;
    }
    let _ = writeln!(
        text,
        "\n| Program | Metric | Last | Min | Max | Readings |\n| --- | --- | ---: | ---: | ---: | ---: |"
    );
    for program in with_metrics {
        for metric in &program.metrics {
            let unit = metric
                .unit
                .as_deref()
                .map_or_else(String::new, |u| format!(" {u}"));
            let _ = writeln!(
                text,
                "| {} | {} | {}{unit} | {} | {} | {} |",
                cell(&program.id),
                cell(&metric.name),
                metric.last,
                metric.min,
                metric.max,
                metric.count
            );
        }
    }
}

pub(crate) fn exit_text(program: &ProgramResult) -> String {
    match (program.exit_code, program.exit_signal) {
        (Some(code), _) => format!("code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "none".to_owned(),
    }
}

fn markdown_set(set: &RunSetResult) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "## netscenario: {}, {} repetitions\n",
        set.scenario.name, set.repetitions
    );
    let seed = if set.seed.drawn {
        format!(
            "{} (drawn; repeat the set with `--seed {}`)",
            set.seed.value, set.seed.value
        )
    } else {
        set.seed.value.to_string()
    };
    let method = match set.confidence.method {
        crate::bootstrap::Method::Percentile => "percentile",
        crate::bootstrap::Method::ExpandedPercentile => "expanded percentile",
    };
    let _ = writeln!(text, "| | |\n| --- | --- |");
    let _ = writeln!(text, "| Backend | {} |", cell(&set.provenance.backend));
    let _ = writeln!(text, "| Seed | {seed} |");
    let _ = writeln!(
        text,
        "| Intervals | around the {}, {} %, {} draws, {method} |",
        set.confidence.statistic.name(),
        trim_float(set.confidence.level * 100.0),
        set.confidence.resamples
    );
    let unreproducible = set.runs.iter().filter(|run| !run.loss_reproducible).count();
    if unreproducible > 0 {
        let _ = writeln!(
            text,
            "\n> **Warning:** the loss of {unreproducible} of the runs does not repeat from its seed."
        );
    }
    let _ = writeln!(text, "\n### Headline metrics\n");
    let _ = writeln!(
        text,
        "| Metric | Mean | Interval | Spread (stddev) |\n| --- | ---: | ---: | ---: |"
    );
    let mut any = false;
    for metric in &set.metrics {
        if HEADLINE.iter().any(|suffix| metric.name.ends_with(suffix)) {
            any = true;
            let interval = &metric.interval;
            let _ = writeln!(
                text,
                "| {} | {} | {} to {} | {} |",
                cell(&metric.name),
                metric_value(&metric.name, interval.estimate),
                metric_value(&metric.name, interval.low),
                metric_value(&metric.name, interval.high),
                metric_value(&metric.name, metric.stddev)
            );
        }
    }
    if !any {
        let _ = writeln!(text, "| (none) | | | |");
    }
    let _ = writeln!(
        text,
        "\n<details><summary>All {} metrics</summary>\n\n| Metric | Mean | Median | Min | Max | Interval |\n| --- | ---: | ---: | ---: | ---: | ---: |",
        set.metrics.len()
    );
    for metric in &set.metrics {
        let _ = writeln!(
            text,
            "| {} | {} | {} | {} | {} | {} to {} |",
            cell(&metric.name),
            metric_value(&metric.name, metric.mean),
            metric_value(&metric.name, metric.median),
            metric_value(&metric.name, metric.min),
            metric_value(&metric.name, metric.max),
            metric_value(&metric.name, metric.interval.low),
            metric_value(&metric.name, metric.interval.high)
        );
    }
    let _ = writeln!(text, "\n</details>");
    if !set.partial.is_empty() {
        let _ = writeln!(text, "\n### Without an interval\n");
        let _ = writeln!(
            text,
            "These metrics are missing in some runs, so an interval from the others would be a made-up number:\n"
        );
        for missing in &set.partial {
            let _ = writeln!(
                text,
                "- `{}`: in {} of {} runs",
                missing.name, missing.present_in, missing.of
            );
        }
    }
    let _ = writeln!(text, "\n### Runs\n");
    let _ = writeln!(
        text,
        "| Run | Seed | Loss repeats from the seed |\n| --- | ---: | --- |"
    );
    for run in &set.runs {
        let _ = writeln!(
            text,
            "| {} | {} | {} |",
            cell(&run.directory),
            run.seed,
            if run.loss_reproducible { "yes" } else { "no" }
        );
    }
    text
}

fn trim_float(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

// ---- JUnit ----

/// The names and values of the properties of a workload, `netscenario.<scenario>.<workload>.<name>`. The first seven names are
/// those of the pytest plugin, the others are added (D-076).
fn workload_properties(scenario: &str, workload: &WorkloadResult) -> Vec<(String, String)> {
    let prefix = format!("netscenario.{scenario}.{}", workload.id);
    let stats = &workload.stats;
    let mut found: Vec<(String, String)> = vec![
        (format!("{prefix}.sent"), stats.sent.to_string()),
        (format!("{prefix}.received"), stats.received.to_string()),
        (format!("{prefix}.lost"), stats.lost.to_string()),
        (format!("{prefix}.loss_ratio"), stats.loss_ratio.to_string()),
        (format!("{prefix}.reordered"), stats.reordered.to_string()),
        (format!("{prefix}.duplicates"), stats.duplicates.to_string()),
    ];
    let secs = |ns: u64| (ns as f64 / 1e9).to_string();
    if let Some(delay) = &stats.delay {
        found.push((format!("{prefix}.delay_min_s"), secs(delay.min_ns)));
        found.push((format!("{prefix}.delay_p50_s"), secs(delay.p50_ns)));
        found.push((format!("{prefix}.delay_p95_s"), secs(delay.p95_ns)));
        found.push((format!("{prefix}.delay_p99_s"), secs(delay.p99_ns)));
        found.push((format!("{prefix}.delay_max_s"), secs(delay.max_ns)));
    }
    if let Some(jitter) = &stats.jitter {
        found.push((format!("{prefix}.jitter_mean_s"), secs(jitter.mean_ns)));
    }
    if workload.duration_ns > 0 {
        let bits = workload.payload_received_bytes() as f64 * 8.0;
        found.push((
            format!("{prefix}.throughput_mean_bps"),
            (bits / (workload.duration_ns as f64 / 1e9)).to_string(),
        ));
    }
    found.push((
        format!("{prefix}.loss_runs_longest"),
        stats.loss_runs.longest.to_string(),
    ));
    if let Some(echo) = &workload.echo {
        found.push((
            format!("{prefix}.echo_received"),
            echo.stats.received.to_string(),
        ));
        found.push((
            format!("{prefix}.echo_loss_ratio"),
            echo.stats.loss_ratio.to_string(),
        ));
        if let Some(trip) = &echo.stats.delay {
            found.push((format!("{prefix}.round_trip_min_s"), secs(trip.min_ns)));
            found.push((format!("{prefix}.round_trip_p50_s"), secs(trip.p50_ns)));
            found.push((format!("{prefix}.round_trip_p95_s"), secs(trip.p95_ns)));
            found.push((format!("{prefix}.round_trip_p99_s"), secs(trip.p99_ns)));
            found.push((format!("{prefix}.round_trip_max_s"), secs(trip.max_ns)));
        }
    }
    found
}

fn properties_xml(out: &mut String, indent: &str, properties: &[(String, String)]) {
    if properties.is_empty() {
        return;
    }
    let _ = writeln!(out, "{indent}<properties>");
    for (name, value) in properties {
        let _ = writeln!(
            out,
            "{indent}  <property name=\"{}\" value=\"{}\"/>",
            escape(name),
            escape(value)
        );
    }
    let _ = writeln!(out, "{indent}</properties>");
}

/// The JUnit XML report.
#[must_use]
pub fn junit(report: &Report) -> String {
    match report {
        Report::Run(run) => junit_run(&run.result),
        Report::Set(set, _) => junit_set(set),
    }
}

/// One test case: a name, the time it took, properties, perhaps a failure, and the text of the case.
struct Case {
    name: String,
    seconds: f64,
    properties: Vec<(String, String)>,
    failure: Option<String>,
    out: String,
}

fn write_suite(
    out: &mut String,
    name: &str,
    timestamp: &str,
    suite_properties: &[(String, String)],
    cases: &[Case],
) {
    let failures = cases.iter().filter(|case| case.failure.is_some()).count();
    let time: f64 = cases.iter().map(|case| case.seconds).sum();
    let _ = writeln!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        out,
        "<testsuites name=\"netscenario\" tests=\"{}\" failures=\"{failures}\" errors=\"0\" time=\"{time:.3}\">",
        cases.len()
    );
    let _ = writeln!(
        out,
        "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\" errors=\"0\" skipped=\"0\" time=\"{time:.3}\" timestamp=\"{}\">",
        escape(name),
        cases.len(),
        escape(timestamp)
    );
    properties_xml(out, "    ", suite_properties);
    for case in cases {
        let _ = writeln!(
            out,
            "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\">",
            escape(name),
            escape(&case.name),
            case.seconds
        );
        properties_xml(out, "      ", &case.properties);
        if let Some(message) = &case.failure {
            let _ = writeln!(
                out,
                "      <failure message=\"{}\" type=\"netscenario.RunFailed\">{}</failure>",
                escape(message),
                escape(message)
            );
        }
        if !case.out.is_empty() {
            let _ = writeln!(
                out,
                "      <system-out>{}</system-out>",
                escape(case.out.trim_end())
            );
        }
        let _ = writeln!(out, "    </testcase>");
    }
    let _ = writeln!(out, "  </testsuite>\n</testsuites>");
}

/// The properties of a program of your own: how it ended, its exit code and the last value of each metric it reported.
fn program_properties(scenario: &str, program: &ProgramResult) -> Vec<(String, String)> {
    let mut properties = vec![(
        format!("netscenario.{scenario}.{}.ended", program.id),
        program.ended.clone(),
    )];
    if let Some(code) = program.exit_code {
        properties.push((
            format!("netscenario.{scenario}.{}.exit_code", program.id),
            code.to_string(),
        ));
    }
    for metric in &program.metrics {
        properties.push((
            format!("netscenario.{scenario}.{}.{}", program.id, metric.name),
            metric.last.to_string(),
        ));
    }
    properties
}

/// The properties that a run gives to its test cases in a JUnit report: those of each workload, then those of each program, as
/// `(name, value)` with the value as text. The pytest plugin takes its properties from here, so that its names and those of the
/// report cannot differ (D-077).
#[must_use]
pub fn run_properties(result: &RunResult) -> Vec<(String, String)> {
    let scenario = &result.scenario.name;
    let mut found: Vec<(String, String)> = result
        .workloads
        .iter()
        .flat_map(|workload| workload_properties(scenario, workload))
        .collect();
    for program in &result.programs {
        found.extend(program_properties(scenario, program));
    }
    found
}

fn junit_run(result: &RunResult) -> String {
    let scenario = &result.scenario.name;
    let provenance = &result.provenance;
    let mut suite_properties = vec![
        (
            "netscenario.version".to_owned(),
            provenance.netscenario.clone(),
        ),
        ("netscenario.backend".to_owned(), provenance.backend.clone()),
        (
            "netscenario.scenario_hash".to_owned(),
            result.scenario.hash.clone(),
        ),
        (
            "netscenario.loss_reproducible".to_owned(),
            result.seed.loss_reproducible.to_string(),
        ),
    ];
    if let Some(seed) = result.seed.value {
        suite_properties.push(("netscenario.seed".to_owned(), seed.to_string()));
    }
    let mut cases = Vec::new();
    for workload in &result.workloads {
        let mut out = format!(
            "{} -> {}: sent {}, received {}, lost {} ({})",
            workload.from,
            workload.to,
            workload.stats.sent,
            workload.stats.received,
            workload.stats.lost,
            percent(workload.stats.loss_ratio)
        );
        if let Some(delay) = &workload.stats.delay {
            let _ = write!(
                out,
                ", delay p50 {}, p99 {}",
                ms(delay.p50_ns),
                ms(delay.p99_ns)
            );
        }
        for reason in &result.seed.not_reproducible_because {
            let _ = write!(
                out,
                "\nwarning: the loss does not repeat from its seed: {reason}"
            );
        }
        if workload.sender.send_errors > 0 && link_shut_on_purpose(result) {
            let _ = write!(
                out,
                "\nnote: the kernel refused {} packets while a link was shut on purpose",
                workload.sender.send_errors
            );
        }
        cases.push(Case {
            name: workload.id.clone(),
            seconds: workload.duration_ns as f64 / 1e9,
            properties: workload_properties(scenario, workload),
            failure: failure_of(result, workload),
            out,
        });
    }
    for program in &result.programs {
        let properties = program_properties(scenario, program);
        cases.push(Case {
            name: program.id.clone(),
            seconds: program.ended_ns.saturating_sub(program.started_ns).max(0) as f64 / 1e9,
            properties,
            failure: result
                .timing
                .interrupted_ns
                .map(|ns| format!("the run was interrupted after {} s", seconds(ns))),
            out: format!(
                "{}: node {}, {}, exit {}",
                program.id,
                program.node,
                program.ended,
                exit_text(program)
            ),
        });
    }
    let mut out = String::new();
    write_suite(
        &mut out,
        scenario,
        &provenance.started_at,
        &suite_properties,
        &cases,
    );
    out
}

/// The properties that a set gives to the suite of its JUnit report: the number of runs, the level, the seed, and the mean and the
/// ends of the interval of each headline metric. The pytest plugin takes its properties for a set from here (D-077).
#[must_use]
pub fn set_properties(set: &RunSetResult) -> Vec<(String, String)> {
    let scenario = &set.scenario.name;
    let prefix = format!("netscenario.{scenario}");
    let mut properties = vec![
        (format!("{prefix}.repetitions"), set.repetitions.to_string()),
        (
            format!("{prefix}.confidence"),
            set.confidence.level.to_string(),
        ),
        (format!("{prefix}.seed"), set.seed.value.to_string()),
    ];
    for metric in &set.metrics {
        if HEADLINE.iter().any(|suffix| metric.name.ends_with(suffix)) {
            properties.push((
                format!("{prefix}.{}.mean", metric.name),
                metric.mean.to_string(),
            ));
            properties.push((
                format!("{prefix}.{}.low", metric.name),
                metric.interval.low.to_string(),
            ));
            properties.push((
                format!("{prefix}.{}.high", metric.name),
                metric.interval.high.to_string(),
            ));
        }
    }
    properties
}

fn junit_set(set: &RunSetResult) -> String {
    let scenario = &set.scenario.name;
    let prefix = format!("netscenario.{scenario}");
    let suite_properties = set_properties(set);
    let cases: Vec<Case> = set
        .runs
        .iter()
        .map(|run| Case {
            name: run.directory.clone(),
            seconds: 0.0,
            properties: vec![(
                format!("{prefix}.{}.seed", run.directory),
                run.seed.to_string(),
            )],
            failure: None,
            out: if run.loss_reproducible {
                String::new()
            } else {
                "warning: the loss of this run does not repeat from its seed".to_owned()
            },
        })
        .collect();
    let mut out = String::new();
    write_suite(
        &mut out,
        scenario,
        &set.provenance.started_at,
        &suite_properties,
        &cases,
    );
    out
}
