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
        rate(
            stats.received as f64 * workload.packet_size_bytes as f64 * 8.0
                / (workload.duration_ns as f64 / 1e9),
        )
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
        let bits = stats.received as f64 * workload.packet_size_bytes as f64 * 8.0;
        found.push((
            format!("{prefix}.throughput_mean_bps"),
            (bits / (workload.duration_ns as f64 / 1e9)).to_string(),
        ));
    }
    found.push((
        format!("{prefix}.loss_runs_longest"),
        stats.loss_runs.longest.to_string(),
    ));
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

fn junit_set(set: &RunSetResult) -> String {
    let scenario = &set.scenario.name;
    let prefix = format!("netscenario.{scenario}");
    let mut suite_properties = vec![
        (format!("{prefix}.repetitions"), set.repetitions.to_string()),
        (
            format!("{prefix}.confidence"),
            set.confidence.level.to_string(),
        ),
        (format!("{prefix}.seed"), set.seed.value.to_string()),
    ];
    for metric in &set.metrics {
        if HEADLINE.iter().any(|suffix| metric.name.ends_with(suffix)) {
            suite_properties.push((
                format!("{prefix}.{}.mean", metric.name),
                metric.mean.to_string(),
            ));
            suite_properties.push((
                format!("{prefix}.{}.low", metric.name),
                metric.interval.low.to_string(),
            ));
            suite_properties.push((
                format!("{prefix}.{}.high", metric.name),
                metric.interval.high.to_string(),
            ));
        }
    }
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
