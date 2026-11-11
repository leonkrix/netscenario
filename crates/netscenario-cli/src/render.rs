//! The summary of a run as text for people. The JSON is `result.json` itself (see `docs/RESULTS.md`).

use std::fmt::Write as _;
use std::path::Path;

use netscenario_core::{
    Comparison, MetricSet, Recorded, RunResult, RunSetResult, Verdict, WorkloadComparison,
    WorkloadResult,
};

/// Up to this many seconds the throughput of every second is listed; a longer run shows the average.
const MAX_LISTED_SECONDS: usize = 6;

/// Nanoseconds as milliseconds with one decimal.
fn ms(ns: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // delays are far below 2^52 nanoseconds
    let value = ns as f64 / 1e6;
    format!("{value:.1} ms")
}

/// Bits per second in the unit that reads best, without trailing zeros.
fn rate(bits: f64) -> String {
    let (value, unit) = if bits >= 1e9 {
        (bits / 1e9, "gbit/s")
    } else if bits >= 1e6 {
        (bits / 1e6, "mbit/s")
    } else if bits >= 1e3 {
        (bits / 1e3, "kbit/s")
    } else {
        (bits, "bit/s")
    };
    let number = format!("{value:.2}");
    let number = number.trim_end_matches('0').trim_end_matches('.');
    format!("{number} {unit}")
}

/// The text printed after a run: what ran, the seed, and for each workload the packets, the delay and the
/// throughput of each second.
pub(crate) fn summary(result: &RunResult, out_dir: &Path) -> String {
    let mut text = String::new();
    let details: Vec<String> = result
        .provenance
        .backend_details
        .values()
        .cloned()
        .collect();
    let _ = writeln!(
        text,
        "netscenario run: {}\n  backend {} ({})",
        result.scenario.name,
        result.provenance.backend,
        details.join(", ")
    );
    if let Some(ns) = result.timing.interrupted_ns {
        let _ = writeln!(
            text,
            "  interrupted after {} of {}: this is the result of the part that ran",
            seconds(ns),
            seconds(result.timing.duration_ns)
        );
    }
    match (result.seed.value, result.seed.drawn) {
        (Some(value), true) => {
            let _ = writeln!(
                text,
                "  seed {value} (drawn; repeat the run with --seed {value})"
            );
        }
        (Some(value), false) => {
            let _ = writeln!(text, "  seed {value}");
        }
        (None, _) => {
            let _ = writeln!(text, "  no seed (the backend cannot apply one)");
        }
    }
    if let Some(params) = result.scenario.params_line() {
        let _ = writeln!(text, "  parameters {params}");
    }
    for reason in &result.seed.not_reproducible_because {
        let _ = writeln!(
            text,
            "  warning: the loss of this run is not reproducible: {reason}"
        );
    }
    for accepted in &result.accepted_restrictions {
        let _ = writeln!(
            text,
            "  accepted restriction ({}): {}",
            accepted.feature, accepted.restriction
        );
    }
    for workload in &result.workloads {
        workload_text(&mut text, workload);
    }
    tcp_text(&mut text, result);
    programs_text(&mut text, result);
    captures_text(&mut text, result);
    directions_text(&mut text, result);
    events_text(&mut text, result);
    for note in &result.notes {
        let _ = writeln!(text, "  note: {note}");
    }
    if let Some(expect) = &result.expect {
        expect_text(&mut text, expect);
    }
    let _ = writeln!(text, "\nresult: {}", out_dir.join("result.json").display());
    text
}

/// A list of sequence numbers for a sentence: `4, 8, 17`, with `...` where it is cut.
fn sequences(listed: &[u64], total: u64) -> String {
    let mut text = listed
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if total > listed.len() as u64 {
        text.push_str(", ...");
    }
    text
}

/// What the replay found about one workload, in one or two lines.
fn replay_workload(text: &mut String, workload: &WorkloadComparison) {
    if workload.identical() {
        let _ = writeln!(
            text,
            "  {}: lost {} of the packets in both runs, the same ones",
            workload.id, workload.lost_original
        );
        return;
    }
    let _ = writeln!(
        text,
        "  {}: the original lost {}, the replay {}; {} packet{} differ",
        workload.id,
        workload.lost_original,
        workload.lost_replay,
        workload.differing,
        if workload.differing == 1 { "" } else { "s" }
    );
    if !workload.only_original.is_empty() {
        let _ = writeln!(
            text,
            "    lost only in the original: {}",
            sequences(&workload.only_original, workload.count_only_original)
        );
    }
    if !workload.only_replay.is_empty() {
        let _ = writeln!(
            text,
            "    lost only in the replay: {}",
            sequences(&workload.only_replay, workload.count_only_replay)
        );
    }
}

/// The text printed after a replay: the machines, the lost packets of each workload and the verdict.
pub(crate) fn replay_summary(
    recorded: &Recorded,
    replay_dir: &Path,
    comparison: &Comparison,
) -> String {
    let mut text = String::new();
    let original = &recorded.result;
    let _ = writeln!(
        text,
        "netscenario replay: {}\n  original {} (run {})\n  replay   {}",
        original.scenario.name,
        recorded.dir.display(),
        original.provenance.started_at,
        replay_dir.display()
    );
    match original.seed.value {
        Some(seed) => {
            let _ = writeln!(
                text,
                "  seed {seed}{}",
                if original.seed.drawn {
                    " (drawn by the original)"
                } else {
                    ""
                }
            );
        }
        None => {
            let _ = writeln!(text, "  no seed (the backend could not apply one)");
        }
    }
    if comparison.other_machine {
        let _ = writeln!(
            text,
            "  machine: NOT the one of the original (original -> replay):"
        );
    } else if comparison.provenance.is_empty() {
        let _ = writeln!(text, "  machine: the same as the original");
    } else {
        let _ = writeln!(text, "  machine: the same as the original, other versions:");
    }
    for difference in &comparison.provenance {
        let _ = writeln!(
            text,
            "    {}: {} -> {}",
            difference.path, difference.recorded, difference.other
        );
    }
    if !comparison.workloads.is_empty() {
        let _ = writeln!(text, "\nlost packets");
        for workload in &comparison.workloads {
            replay_workload(&mut text, workload);
        }
    }
    for reason in &comparison.reasons {
        let _ = writeln!(text, "  note: {reason}");
    }
    let verdict = match comparison.verdict {
        Verdict::Reproduced => "the replay lost the same packets as the original.".to_owned(),
        Verdict::Differs => {
            let mut line = "NOT REPRODUCED: the replay lost other packets than the original, although the loss repeats from the seed.".to_owned();
            if comparison.other_machine {
                line.push_str("\n  The machine is not the original's, which can explain it: the lost packets of a seed depend on the kernel's netem and on the version of tc.");
            }
            line
        }
        Verdict::NotPromised => "the lost packets differ, which the notes above said can happen: the loss of this scenario does not repeat from its seed.".to_owned(),
        Verdict::NotComparable => "the runs could not be compared (see the notes above).".to_owned(),
        _ => "the comparison has a verdict that this version does not know.".to_owned(),
    };
    let _ = writeln!(text, "\nresult: {verdict}");
    text
}

/// The metrics that the summary of a set shows for each workload, in this order, with how to write their numbers. The set
/// has all the others in `runset.json`.
const HEADLINE: [(&str, Unit); 5] = [
    ("loss_ratio", Unit::Percent),
    ("delay.p50", Unit::Milliseconds),
    ("delay.p95", Unit::Milliseconds),
    ("delay.p99", Unit::Milliseconds),
    ("throughput.mean", Unit::Rate),
];

#[derive(Clone, Copy)]
enum Unit {
    Percent,
    Milliseconds,
    Rate,
}

impl Unit {
    /// A value of the metric as text; the interval uses the same unit and writes it once.
    fn number(self, value: f64) -> String {
        match self {
            Self::Percent => format!("{:.2}", value * 100.0),
            Self::Milliseconds => format!("{:.3}", value * 1e3),
            Self::Rate => rate(value),
        }
    }

    /// The unit after the interval; the rate carries its own.
    fn suffix(self) -> &'static str {
        match self {
            Self::Percent => " %",
            Self::Milliseconds => " ms",
            Self::Rate => "",
        }
    }
}

/// The verdict of the checks of the file: how many hold, and one line for each that does not, or for each when all are small
/// enough to list (D-094).
fn expect_text(text: &mut String, expect: &netscenario_core::ExpectResult) {
    let (held, total) = expect.counts();
    let runs = if expect.runs > 1 {
        format!(" over {} runs", expect.runs)
    } else {
        String::new()
    };
    let _ = writeln!(
        text,
        "\nchecks of the file{runs}: {held} of {total} hold{}",
        if expect.passed { "" } else { ", NOT ALL" }
    );
    for check in &expect.checks {
        let _ = writeln!(
            text,
            "  {} {}",
            if check.passed { "ok  " } else { "FAIL" },
            check.line()
        );
    }
}

/// The text printed after a set of runs: what ran, the seed and how the intervals were made, and for each workload the
/// headline metrics with the value in the middle, the interval and the spread between the runs.
pub(crate) fn set_summary(set: &RunSetResult, out_dir: &Path) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "netscenario run: {}, {} repetitions\n  backend {}",
        set.scenario.name, set.repetitions, set.provenance.backend
    );
    if set.seed.drawn {
        let _ = writeln!(
            text,
            "  seed {} (drawn; repeat the set with --seed {})",
            set.seed.value, set.seed.value
        );
    } else {
        let _ = writeln!(text, "  seed {}", set.seed.value);
    }
    let _ = writeln!(
        text,
        "  interval of the {}: {} %, {} draws ({} method)",
        set.confidence.statistic.name(),
        set.confidence.level * 100.0,
        set.confidence.resamples,
        match set.confidence.method {
            netscenario_core::bootstrap::Method::Percentile => "percentile",
            netscenario_core::bootstrap::Method::ExpandedPercentile => "expanded percentile",
        }
    );
    let unreproducible = set.runs.iter().filter(|run| !run.loss_reproducible).count();
    if unreproducible > 0 {
        let _ = writeln!(
            text,
            "  warning: the loss of {unreproducible} of the runs is not reproducible from its seed (see result.json of the run)"
        );
    }
    let mut ids: Vec<&str> = Vec::new();
    for metric in &set.metrics {
        let id = metric.name.split('.').next().unwrap_or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    for id in ids {
        let rows: Vec<(&MetricSet, Unit)> = HEADLINE
            .iter()
            .filter_map(|(suffix, unit)| {
                set.metric(&format!("{id}.{suffix}"))
                    .map(|metric| (metric, *unit))
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(text, "\n{id}");
        for (metric, unit) in rows {
            let label = metric
                .name
                .strip_prefix(&format!("{id}."))
                .unwrap_or(&metric.name);
            let interval = &metric.interval;
            let _ = writeln!(
                text,
                "  {label:<16} {}{}  [{} to {}{}]  spread {}{}",
                unit.number(interval.estimate),
                unit.suffix(),
                unit.number(interval.low),
                unit.number(interval.high),
                unit.suffix(),
                unit.number(metric.stddev),
                unit.suffix(),
            );
        }
    }
    if !set.partial.is_empty() {
        let names: Vec<&str> = set
            .partial
            .iter()
            .take(3)
            .map(|p| p.name.as_str())
            .collect();
        let _ = writeln!(
            text,
            "\n  note: {} metrics are missing in some runs and have no interval ({}{})",
            set.partial.len(),
            names.join(", "),
            if set.partial.len() > 3 { ", ..." } else { "" }
        );
    }
    if let Some(expect) = &set.expect {
        expect_text(&mut text, expect);
    }
    let _ = writeln!(
        text,
        "\n{} metrics in all; the values of every run and every interval are in the set\nset: {}",
        set.metrics.len(),
        out_dir.join("runset.json").display()
    );
    text
}

/// Where the packets of a direction went, for each direction in which the receiving node dropped some. A
/// direction without such drops is not listed: the loss of the workload is the whole story there (D-040).
/// What the run did to the links while it ran (the timeline): one line for each step, with how late it was.
fn events_text(text: &mut String, result: &RunResult) {
    if result.events.is_empty() {
        return;
    }
    let _ = writeln!(text, "\ntimeline");
    // A model makes thousands of steps: the summary shows the first ones and the count, result.json has all of them
    for event in result.events.iter().take(MAX_STEPS_LISTED) {
        // A step at a barrier is done before a packet and says which, and how long that packet waited; a step by the
        // clock says how late it was
        let when = match (event.packet, event.held_ns) {
            (Some(packet), held) => format!(
                "before packet {packet}, which waited {}",
                ms_fine(held.unwrap_or(0))
            ),
            (None, _) => format!(
                "done {} late",
                ms(event.applied_ns.saturating_sub(event.scheduled_ns))
            ),
        };
        let _ = writeln!(
            text,
            "  at {}  link {}{} {}  {}  {when}",
            seconds(event.scheduled_ns),
            event.link,
            match (event.from_model, event.live) {
                (true, _) => " (model)",
                (_, true) => " (from code)",
                _ => "",
            },
            event.direction.as_deref().unwrap_or("both ways"),
            event.kind,
        );
    }
    if let Some(more) = result
        .events
        .len()
        .checked_sub(MAX_STEPS_LISTED)
        .filter(|more| *more > 0)
    {
        let _ = writeln!(
            text,
            "  and {more} more steps (all of them are in result.json)"
        );
    }
}

/// How many steps of the timeline the summary lists.
const MAX_STEPS_LISTED: usize = 20;

/// Which packets the kernel refused, for the warning: up to three runs by their numbers, else how many runs.
fn refused_text(refused: &[[u64; 2]]) -> String {
    match refused {
        [] => String::new(),
        runs if runs.len() <= 3 => {
            let runs: Vec<String> = runs
                .iter()
                .map(|[first, last]| {
                    if first == last {
                        format!("packet {first}")
                    } else {
                        format!("packets {first} to {last}")
                    }
                })
                .collect();
            format!(" ({})", runs.join(", "))
        }
        runs => format!(" (in {} separate runs)", runs.len()),
    }
}

/// Seconds with the decimals they need, from nanoseconds.
fn seconds(ns: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // moments are far below 2^52 nanoseconds
    let value = ns as f64 / 1e9;
    format!("{value:.3} s")
}

/// A time that may be before the start of the measured phase.
fn signed_seconds(ns: i64) -> String {
    #[allow(clippy::cast_precision_loss)] // moments are far below 2^52 nanoseconds
    let value = ns as f64 / 1e9;
    format!("{value:.3} s")
}

/// A number from a program: whole if it is, else with the digits it needs (up to 6).
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        let text = format!("{value:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// The programs of your own: how each ended, its output and its metrics (WRK-06, PRB-08). What went wrong is a
/// warning on a line of its own: a program that ended by itself, output that was dropped, metric lines that were
/// not valid.
/// The TCP workloads: what was sent and read, how long the transfers took, the goodput, what the kernel saw (round trip time,
/// retransmissions) and, for requests, how long they took (D-086). A run with TCP says that it is statistical.
fn tcp_text(text: &mut String, result: &RunResult) {
    if result.tcp.is_empty() {
        return;
    }
    for tcp in &result.tcp {
        let _ = writeln!(
            text,
            "\ntcp {}  {} -> {}  {}, {} connection{}{}",
            tcp.id,
            tcp.from,
            tcp.to,
            tcp.kind,
            tcp.connections,
            if tcp.connections == 1 { "" } else { "s" },
            tcp.congestion
                .as_ref()
                .map_or_else(String::new, |name| format!(", {name}"))
        );
        if tcp.failed_connections > 0 {
            let _ = writeln!(
                text,
                "  warning: {} of {} connections failed: {}",
                tcp.failed_connections,
                tcp.connections,
                tcp.first_error.as_deref().unwrap_or("no reason was given")
            );
        }
        if tcp.stopped {
            let _ = writeln!(text, "  the run was interrupted before the client was done");
        }
        match &tcp.requests {
            Some(requests) => requests_text(text, requests),
            None => transfers_text(text, tcp),
        }
        if let Some(rtt) = &tcp.rtt {
            let _ = writeln!(
                text,
                "  round trip time of the kernel  min {}  median {}  p95 {}  max {}",
                ms(rtt.min_ns),
                ms(rtt.p50_ns),
                ms(rtt.p95_ns),
                ms(rtt.max_ns)
            );
        }
        let _ = writeln!(
            text,
            "  retransmitted segments  {}",
            tcp.retransmitted_segments
        );
        if let Some(why) = &tcp.info_error {
            let _ = writeln!(text, "  note: no samples of the kernel: {why}");
        }
        recoveries_text(text, &tcp.recoveries);
    }
    let _ = writeln!(
        text,
        "\n  note: a run with TCP is {}: {}",
        result.reproducibility.class,
        result.reproducibility.because.join("; ")
    );
}

/// The requests of a `request-response` workload and how long they took.
fn requests_text(text: &mut String, requests: &netscenario_core::RequestInfo) {
    let _ = writeln!(
        text,
        "  requests  sent {}  answered {}  failed {}",
        requests.sent, requests.answered, requests.failed
    );
    for (what, summary) in [
        ("latency from the moment due", &requests.latency),
        ("round trip from the moment sent", &requests.round_trip),
    ] {
        if let Some(summary) = summary {
            let _ = writeln!(
                text,
                "  {what}  min {}  median {}  p95 {}  p99 {}  max {}",
                ms(summary.min_ns),
                ms(summary.p50_ns),
                ms(summary.p95_ns),
                ms(summary.p99_ns),
                ms(summary.max_ns)
            );
        }
    }
}

/// The bytes of a `tcp-bulk` workload, how long its transfers took and the goodput over them.
fn transfers_text(text: &mut String, tcp: &netscenario_core::TcpResult) {
    let _ = writeln!(
        text,
        "  sent {} bytes  received {} bytes",
        tcp.bytes_sent, tcp.bytes_received
    );
    let done: Vec<_> = tcp
        .transfers
        .iter()
        .filter(|transfer| transfer.ok)
        .collect();
    if let Some(longest) = done.iter().map(|transfer| transfer.duration_ns()).max() {
        let _ = writeln!(
            text,
            "  {} of {} transfers complete, the last after {}",
            done.len(),
            tcp.transfers.len(),
            seconds(longest)
        );
    }
    if let Some(mean) = tcp
        .goodput
        .as_ref()
        .and_then(|goodput| goodput.mean_bits_per_second)
    {
        #[allow(clippy::cast_precision_loss)] // a rate is far below 2^52 bits per second
        let mean = mean as f64;
        let _ = writeln!(
            text,
            "  goodput  {} on average over the transfer",
            rate(mean)
        );
    }
}

fn programs_text(text: &mut String, result: &RunResult) {
    for program in &result.programs {
        let how = match (program.exit_code, program.exit_signal) {
            (Some(code), _) => format!("exit code {code}"),
            (None, Some(signal)) => format!("signal {signal}"),
            (None, None) => "no exit status".to_owned(),
        };
        let _ = writeln!(
            text,
            "\nprogram {}  in {}  {}",
            program.id,
            program.node,
            program.run.join(" ")
        );
        let _ = writeln!(
            text,
            "  started {}, ready {}, ended {} ({}, {how})",
            signed_seconds(program.started_ns),
            signed_seconds(program.ready_ns),
            signed_seconds(program.ended_ns),
            program.ended
        );
        let _ = writeln!(
            text,
            "  output  {} bytes in {}, {} bytes of standard error in {}",
            program.output.stdout_bytes,
            program.stdout_file,
            program.output.stderr_bytes,
            program.stderr_file
        );
        for metric in &program.metrics {
            let unit = metric
                .unit
                .as_deref()
                .map_or_else(String::new, |unit| format!(" {unit}"));
            let _ = writeln!(
                text,
                "  metric {}  {} readings, last {}{unit}, min {}, max {}",
                metric.name,
                metric.count,
                number(metric.last),
                number(metric.min),
                number(metric.max)
            );
        }
        // A program that ends by itself with 0 has finished its work; one that ends with an error has crashed
        if program.ended == "on-its-own" && program.exit_code != Some(0) {
            let _ = writeln!(
                text,
                "  warning: it ended by itself with an error, before the run asked it to stop"
            );
        }
        if program.output.stdout_dropped_bytes > 0 || program.output.stderr_dropped_bytes > 0 {
            let _ = writeln!(
                text,
                "  warning: the files hold the first megabyte; {} bytes of standard output and {} of standard error are not in them",
                program.output.stdout_dropped_bytes, program.output.stderr_dropped_bytes
            );
        }
        if program.output.long_lines > 0 {
            let _ = writeln!(
                text,
                "  warning: {} lines of standard output were longer than 64 KiB and were cut",
                program.output.long_lines
            );
        }
        if program.lines_dropped {
            let _ = writeln!(
                text,
                "  warning: lines of standard output were dropped before the run read them; metrics in them are lost"
            );
        }
        if program.bad_metric_lines > 0 {
            let _ = writeln!(
                text,
                "  warning: {} lines looked like metrics and were not valid, the first: {}",
                program.bad_metric_lines,
                program
                    .bad_metric_examples
                    .first()
                    .map_or("", String::as_str)
            );
        }
    }
}

/// The captures of the interfaces (PRB-09): where the file is, how many frames, and whether the capture is whole. A capture
/// that lost frames, or could not check itself, is a warning on a line of its own.
fn captures_text(text: &mut String, result: &RunResult) {
    for capture in &result.captures {
        let _ = writeln!(
            text,
            "\ncapture link {} in {}  {} frames, {} bytes, in {}",
            capture.link, capture.node, capture.packets, capture.bytes, capture.file
        );
        if let (Some(first), Some(last)) = (capture.first_ns, capture.last_ns) {
            let _ = writeln!(
                text,
                "  from {} to {} of the measured phase",
                signed_seconds(first),
                signed_seconds(last)
            );
        }
        match (capture.missing, capture.interface_frames) {
            (Some(0), Some(counted)) => {
                let _ = writeln!(
                    text,
                    "  whole: the interface {} counted {counted} frames, as many as the file has",
                    capture.interface
                );
            }
            (Some(missing), Some(counted)) => {
                let _ = writeln!(
                    text,
                    "  warning: {missing} frames are missing: the interface {} counted {counted} and the file has {} (the receive buffer of the capture was {} bytes; `net.core.rmem_max` limits it)",
                    capture.interface, capture.packets, capture.receive_buffer_bytes
                );
            }
            _ => {
                let _ = writeln!(
                    text,
                    "  warning: the counters of the interface could not be read, so the capture could not check that it is whole"
                );
            }
        }
    }
}

fn directions_text(text: &mut String, result: &RunResult) {
    for tally in result
        .directions
        .iter()
        .filter(|tally| tally.dropped_corrupted > 0 || tally.receive_buffer_drops > 0)
    {
        let mut parts = Vec::new();
        if let Some(lost) = tally.lost_in_queue {
            parts.push(format!("{lost} were lost in the queue"));
        }
        parts.push(format!(
            "{} arrived damaged and were dropped by the node",
            tally.dropped_corrupted
        ));
        if tally.receive_buffer_drops > 0 {
            parts.push(format!(
                "{} were dropped because a receive buffer was full (the receiver's fault, not the network's)",
                tally.receive_buffer_drops
            ));
        }
        parts.push(format!("{} were delivered", tally.delivered));
        let _ = writeln!(
            text,
            "\nlink {} {}: of {} packets sent, {}",
            tally.link,
            tally.direction,
            tally.sent,
            parts.join(", ")
        );
    }
}

/// A time with three decimals, for the jitter, which is often a fraction of a millisecond.
fn ms_fine(ns: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // delays are far below 2^52 nanoseconds
    let value = ns as f64 / 1e6;
    format!("{value:.3} ms")
}

/// The lines about the pattern of the arrivals: the order (RFC 4737), the jitter (RFC 3550) and the runs of lost
/// packets, each only where there is something to say (PRB-04, PRB-06).
fn pattern_lines(text: &mut String, stats: &netscenario_core::stats::FlowStats) {
    if let Some(reordering) = &stats.reordering {
        let _ = writeln!(
            text,
            "  reordering  extent max {}  mean {:.1}  p95 {}  late time max {}  p95 {}",
            reordering.extent.max,
            reordering.extent.mean,
            reordering.extent.p95,
            ms(reordering.late_time.max_ns),
            ms(reordering.late_time.p95_ns)
        );
    }
    if let Some(jitter) = &stats.jitter {
        let _ = writeln!(
            text,
            "  jitter  mean {}  last {}  max {}  (RFC 3550)",
            ms_fine(jitter.mean_ns),
            ms_fine(jitter.last_ns),
            ms_fine(jitter.max_ns)
        );
    }
    let runs = &stats.loss_runs;
    if runs.runs > 0 {
        let _ = writeln!(
            text,
            "  loss runs  {} runs  longest {}  mean {:.2}",
            runs.runs, runs.longest, runs.mean
        );
    }
}

/// How many sequence numbers of damaged packets the summary shows; the result has up to 1,000 and the count.
const MAX_NAMED_SHOWN: usize = 8;

/// The damaged packets that the receiving node dropped, by name, where the direction corrupts (D-040, D-057); or why
/// they are not named.
fn damaged_names_text(text: &mut String, workload: &WorkloadResult) {
    match (
        &workload.corrupted_named,
        &workload.corrupted_seq_unavailable,
    ) {
        (Some(named), _) if *named > 0 => {
            let first: Vec<String> = workload
                .corrupted_seq
                .iter()
                .take(MAX_NAMED_SHOWN)
                .map(ToString::to_string)
                .collect();
            let more = if *named > MAX_NAMED_SHOWN as u64 {
                ", ..."
            } else {
                ""
            };
            let _ = writeln!(
                text,
                "  damaged packets named by sequence number: {named} ({}{more}); `--packets` marks all of them",
                first.join(", ")
            );
        }
        (None, Some(why)) => {
            let _ = writeln!(text, "  damaged packets could not be named: {why}");
        }
        _ => {}
    }
}

/// The part of the summary about one workload.
/// The time to recover after each step of the timeline that concerns the workload (D-070).
fn recoveries_text(text: &mut String, recoveries: &[netscenario_core::recovery::Recovery]) {
    for recovery in recoveries {
        let what = format!(
            "  recovery after {} (event {}, link {}{}):",
            recovery.kind,
            recovery.event,
            recovery.link,
            recovery
                .direction
                .as_deref()
                .map_or(String::new(), |direction| format!(" {direction}"))
        );
        match (recovery.time_ns, recovery.baseline, &recovery.reason) {
            // The level of a TCP workload is a rate (D-088), that of a stream of packets a share
            (Some(time), Some(baseline), _)
                if recovery.basis == netscenario_core::recovery::Basis::Goodput =>
            {
                let _ = writeln!(
                    text,
                    "{what} {} (baseline {}, back to {:.1} % for {} bins, by goodput)",
                    seconds(time),
                    rate(baseline),
                    recovery.fraction * 100.0,
                    recovery.window_bins
                );
            }
            (Some(time), Some(baseline), _) => {
                let _ = writeln!(
                    text,
                    "{what} {} (baseline {:.1} %, back to {:.1} % for {} bins, by {})",
                    seconds(time),
                    baseline * 100.0,
                    recovery.fraction * 100.0,
                    recovery.window_bins,
                    recovery.basis.word()
                );
            }
            (_, _, reason) => {
                let _ = writeln!(
                    text,
                    "{what} no time: {}",
                    reason.as_deref().unwrap_or("not recovered")
                );
            }
        }
    }
}

/// The packets of a workload with an echo that came back, and their round trip (D-087).
fn echo_text(text: &mut String, workload: &WorkloadResult) {
    let Some(echo) = &workload.echo else {
        return;
    };
    let stats = &echo.stats;
    let _ = writeln!(
        text,
        "  came back {} of {}  ({:.2} % lost on the way there or back)",
        stats.received,
        stats.sent,
        stats.loss_ratio * 100.0
    );
    match &stats.delay {
        Some(trip) => {
            let _ = writeln!(
                text,
                "  round trip  min {}  median {}  p95 {}  p99 {}  max {}",
                ms(trip.min_ns),
                ms(trip.p50_ns),
                ms(trip.p95_ns),
                ms(trip.p99_ns),
                ms(trip.max_ns)
            );
        }
        None => {
            let _ = writeln!(text, "  round trip  no packet came back");
        }
    }
    if echo.echo_errors > 0 {
        let _ = writeln!(
            text,
            "  warning: the far node could not send {} packets back",
            echo.echo_errors
        );
    }
}

fn workload_text(text: &mut String, workload: &WorkloadResult) {
    let stats = &workload.stats;
    // The rate of a constant bit rate stream is the one in the scenario; of the others it is a mean, and the kind is said
    let stated = rate(
        #[allow(clippy::cast_precision_loss)] // rates are far below 2^52 bits per second
        {
            workload.rate_bits_per_second as f64
        },
    );
    let shape = if workload.kind == "udp-cbr" {
        stated
    } else {
        format!("{}, {stated} on average", workload.kind)
    };
    // A trace has packets of different sizes
    let sizes = match &workload.trace {
        Some(trace) if trace.smallest_bytes != trace.largest_bytes => format!(
            "{} packets of {} to {} bytes",
            trace.packets, trace.smallest_bytes, trace.largest_bytes
        ),
        _ => format!("{} byte packets", workload.packet_size_bytes),
    };
    let _ = writeln!(
        text,
        "\nworkload {}  {} -> {}  {shape}, {sizes}",
        workload.id, workload.from, workload.to
    );
    let reordered = if stats.reordered > 0 {
        format!("  reordered {}", stats.reordered)
    } else {
        String::new()
    };
    // Without the UDP checksum a damaged packet arrives and is found by its CRC-32; it is lost to the stream all
    // the same, so it is said where the loss is
    let damaged = if workload.corrupted > 0 {
        format!(", {} of them arrived damaged", workload.corrupted)
    } else {
        String::new()
    };
    let _ = writeln!(
        text,
        "  sent {}  received {}  lost {} ({:.2} %{damaged}){reordered}",
        stats.sent,
        stats.received,
        stats.lost,
        stats.loss_ratio * 100.0
    );
    damaged_names_text(text, workload);
    match &stats.delay {
        Some(delay) => {
            let _ = writeln!(
                text,
                "  delay  min {}  median {}  p95 {}  p99 {}  max {}",
                ms(delay.min_ns),
                ms(delay.p50_ns),
                ms(delay.p95_ns),
                ms(delay.p99_ns),
                ms(delay.max_ns)
            );
        }
        None => {
            let _ = writeln!(text, "  delay  no packet arrived");
        }
    }
    pattern_lines(text, stats);
    echo_text(text, workload);
    let bins = &stats.throughput.bits_per_second;
    if bins.len() <= MAX_LISTED_SECONDS {
        let seconds: Vec<String> = bins.iter().map(|bits| rate(*bits)).collect();
        let _ = writeln!(text, "  throughput per second  {}", seconds.join("  "));
    } else {
        // A long run: the average over the time the workload sent; every second is in result.json
        #[allow(clippy::cast_precision_loss)] // counts and times are far below 2^52
        let average = if workload.duration_ns == 0 {
            0.0
        } else {
            (workload.payload_received_bytes() * 8) as f64 * 1e9 / workload.duration_ns as f64
        };
        let _ = writeln!(
            text,
            "  throughput  {} on average over {} s ({} bins of one second in result.json)",
            rate(average),
            workload.duration_ns / 1_000_000_000,
            bins.len()
        );
    }
    recoveries_text(text, &workload.recoveries);
    if workload.sender.send_errors > 0 {
        let _ = writeln!(
            text,
            "  warning: the kernel refused {} packets{}: {}",
            workload.sender.send_errors,
            refused_text(&workload.sender.refused),
            workload
                .sender
                .first_error
                .as_deref()
                .unwrap_or("no reason given")
        );
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A result as `netscenario run` writes it, reduced to one workload.
    pub(crate) const RESULT: &str = r#"{
      "format": "netscenario-result", "version": 0,
      "scenario": {"name": "lte-like-link", "hash": "sha256:00"},
      "provenance": {"netscenario": "0.0.0", "started_at": "2026-10-30T12:27:30Z", "backend": "kernel",
        "backend_details": {"rights": "user namespace", "tc": "iproute2 6.15.0"},
        "kernel": "6.12.113", "cpu_model": null, "cpus": 16},
      "seed": {"value": 42, "drawn": true, "loss_reproducible": true},
      "accepted_restrictions": [], "addresses": [],
      "timing": {"duration_ns": 3000000000, "settle_ns": 1120000000},
      "workloads": [{
        "id": "media", "kind": "udp-cbr", "from": "client", "to": "server",
        "from_address": "10.100.0.1", "to_address": "10.100.0.2:5000",
        "rate_bits_per_second": 2000000, "packet_size_bytes": 1200, "start_ns": 0, "duration_ns": 3000000000,
        "sent": 625, "received": 620, "lost": 5, "loss_ratio": 0.008, "duplicates": 0, "reordered": 0, "unexpected": 0,
        "delay": {"min_ns": 31168696, "mean_ns": 42869547, "p50_ns": 42615470, "p90_ns": 48602048,
          "p95_ns": 50225591, "p99_ns": 54035378, "p999_ns": 56984904, "max_ns": 56984904},
        "throughput": {"bin_ns": 1000000000, "bits_per_second": [1910400.0, 1977600.0, 1987200.0, 76800.0]},
        "sender": {"max_lateness_ns": 189, "send_errors": 0, "first_error": null},
        "foreign_datagrams": 0, "records_file": "flow-0.records", "packets_file": null
      }]
    }"#;

    fn result() -> RunResult {
        RunResult::from_json(RESULT).unwrap()
    }

    /// A set of five runs as `netscenario run --repetitions 5` writes it, reduced to the metrics the summary shows.
    const SET: &str = r#"{
      "format": "netscenario-runset", "version": 0,
      "scenario": {"name": "lte-like-link", "hash": "sha256:00"},
      "provenance": {"netscenario": "0.0.0", "started_at": "2026-11-02T12:00:00Z", "backend": "kernel",
        "backend_details": {}, "kernel": null, "cpu_model": null, "cpus": 4},
      "seed": {"value": 42, "drawn": true},
      "repetitions": 5,
      "confidence": {"statistic": "mean", "level": 0.95, "resamples": 10000, "method": "expanded-percentile"},
      "runs": [
        {"index": 0, "seed": 1, "directory": "run-001", "loss_reproducible": true},
        {"index": 1, "seed": 2, "directory": "run-002", "loss_reproducible": false}
      ],
      "metrics": [
        {"name": "media.delay.p99", "values": [0.05], "mean": 0.05, "median": 0.05, "min": 0.04, "max": 0.06, "stddev": 0.0071,
         "interval": {"statistic": "mean", "estimate": 0.0453, "low": 0.0411, "high": 0.0502, "level": 0.95,
                      "method": "expanded-percentile", "tail": 0.00095, "resamples": 10000, "runs": 5}},
        {"name": "media.loss_ratio", "values": [0.02], "mean": 0.0231, "median": 0.02, "min": 0.01, "max": 0.03, "stddev": 0.0052,
         "interval": {"statistic": "mean", "estimate": 0.0231, "low": 0.0180, "high": 0.0285, "level": 0.95,
                      "method": "expanded-percentile", "tail": 0.00095, "resamples": 10000, "runs": 5}},
        {"name": "media.sent", "values": [100.0], "mean": 100.0, "median": 100.0, "min": 100.0, "max": 100.0, "stddev": 0.0,
         "interval": {"statistic": "mean", "estimate": 100.0, "low": 100.0, "high": 100.0, "level": 0.95,
                      "method": "expanded-percentile", "tail": 0.00095, "resamples": 10000, "runs": 5}}
      ],
      "partial": [{"name": "media.delay.p50", "present_in": 4, "of": 5}]
    }"#;

    /// A replay that is identical to its original: lost 2 of the packets in both runs.
    const COMPARISON: &str = r#"{
      "verdict": "reproduced",
      "workloads": [{"id": "media", "lost_original": 2, "lost_replay": 2, "only_original": [], "only_replay": [],
                     "count_only_original": 0, "count_only_replay": 0, "differing": 0}],
      "provenance": [], "other_machine": false, "reasons": []
    }"#;

    fn recorded() -> Recorded {
        Recorded {
            dir: Path::new("original").to_path_buf(),
            result: RunResult::from_json(RESULT).unwrap(),
            scenario: netscenario_model::Scenario::from_yaml(
                "version: 0\nname: lte-like-link\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - { id: media, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n",
                "t.yaml",
            )
            .unwrap(),
        }
    }

    fn summary_of(comparison: &str) -> String {
        replay_summary(
            &recorded(),
            Path::new("replay"),
            &serde_json::from_str(comparison).unwrap(),
        )
    }

    /// The summary of a run says how many packets came back and their round trip, and only for a workload with an echo.
    #[test]
    fn the_summary_has_the_round_trip_of_an_echo() {
        let plain = summary(&result(), Path::new("out"));
        assert!(!plain.contains("round trip"), "{plain}");
        let echoed = RESULT.replace(
            r#""foreign_datagrams": 0, "records_file": "flow-0.records", "packets_file": null"#,
            r#""foreign_datagrams": 0, "records_file": "flow-0.records", "packets_file": null,
        "echo": {"echoed": 620, "echo_errors": 2, "foreign_datagrams": 0, "records_file": "flow-0.echo.records",
          "sent": 625, "received": 610, "lost": 15, "loss_ratio": 0.024, "duplicates": 0, "reordered": 0, "unexpected": 0,
          "delay": {"min_ns": 62000000, "mean_ns": 80000000, "p50_ns": 79000000, "p90_ns": 90000000,
            "p95_ns": 92000000, "p99_ns": 99000000, "p999_ns": 100000000, "max_ns": 100000000},
          "throughput": {"bin_ns": 1000000000, "bits_per_second": [1.0]}}"#,
        );
        let text = summary(&RunResult::from_json(&echoed).unwrap(), Path::new("out"));
        assert!(text.contains("came back 610 of 625"), "{text}");
        assert!(
            text.contains("round trip  min 62.0 ms  median 79.0 ms"),
            "{text}"
        );
        assert!(
            text.contains("the far node could not send 2 packets back"),
            "{text}"
        );
    }

    #[test]
    fn a_reproduced_replay_says_so_and_names_the_machine() {
        let text = summary_of(COMPARISON);
        for expected in [
            "netscenario replay: lte-like-link",
            "original original (run 2026-10-30T12:27:30Z)",
            "replay   replay",
            "seed 42 (drawn by the original)",
            "machine: the same as the original",
            "media: lost 2 of the packets in both runs, the same ones",
            "result: the replay lost the same packets as the original.",
        ] {
            assert!(text.contains(expected), "{expected:?} is not in\n{text}");
        }
        assert!(!text.contains("NOT REPRODUCED"), "{text}");
    }

    /// Canary: other packets are told with their numbers, the verdict says that the replay failed, and another machine is
    /// offered as the likely reason.
    #[test]
    fn a_failed_replay_names_the_packets_and_the_machine() {
        let text = summary_of(
            r#"{
              "verdict": "differs",
              "workloads": [{"id": "media", "lost_original": 3, "lost_replay": 3, "only_original": [4, 8],
                             "only_replay": [5, 9], "count_only_original": 2, "count_only_replay": 2, "differing": 4}],
              "provenance": [{"path": "kernel", "recorded": "6.12.1", "other": "6.8.0"},
                             {"path": "backend_details.tc", "recorded": "iproute2 6.15.0", "other": "iproute2 6.1.0"}],
              "other_machine": true, "reasons": []
            }"#,
        );
        for expected in [
            "machine: NOT the one of the original (original -> replay):",
            "kernel: 6.12.1 -> 6.8.0",
            "backend_details.tc: iproute2 6.15.0 -> iproute2 6.1.0",
            "media: the original lost 3, the replay 3; 4 packets differ",
            "lost only in the original: 4, 8",
            "lost only in the replay: 5, 9",
            "NOT REPRODUCED",
            "The machine is not the original's, which can explain it",
        ] {
            assert!(text.contains(expected), "{expected:?} is not in\n{text}");
        }
        assert!(!text.contains("same packets as the original"), "{text}");
    }

    #[test]
    fn a_cut_list_of_packets_ends_with_dots_and_a_complete_one_does_not() {
        assert_eq!(sequences(&[1, 2], 2), "1, 2");
        assert_eq!(sequences(&[1, 2], 40), "1, 2, ...");
        assert_eq!(sequences(&[], 0), "");
    }

    #[test]
    fn the_replay_of_a_loss_that_was_not_promised_and_of_runs_that_cannot_be_compared_say_so() {
        let text = summary_of(
            r#"{"verdict": "not-promised", "workloads": [{"id": "media", "lost_original": 1, "lost_replay": 2,
                "only_original": [], "only_replay": [7], "count_only_original": 0, "count_only_replay": 1, "differing": 1}],
                "provenance": [], "other_machine": false, "reasons": ["original: the scenario has an outage"]}"#,
        );
        assert!(
            text.contains("note: original: the scenario has an outage"),
            "{text}"
        );
        assert!(text.contains("does not repeat from its seed"), "{text}");
        assert!(!text.contains("NOT REPRODUCED"), "{text}");
        let text = summary_of(
            r#"{"verdict": "not-comparable", "workloads": [], "provenance": [], "other_machine": false,
                "reasons": ["the original run was interrupted"]}"#,
        );
        assert!(text.contains("could not be compared"), "{text}");
        assert!(
            !text.contains("lost packets\n"),
            "no workloads, no section: {text}"
        );
    }

    #[test]
    fn the_summary_of_a_set_shows_the_headline_metrics_with_their_intervals() {
        let set = RunSetResult::from_json(SET).unwrap();
        let text = set_summary(&set, Path::new("out"));
        for expected in [
            "netscenario run: lte-like-link, 5 repetitions",
            "seed 42 (drawn; repeat the set with --seed 42)",
            "interval of the mean: 95 %, 10000 draws (expanded percentile method)",
            "warning: the loss of 1 of the runs is not reproducible from its seed",
            "\nmedia\n",
            "  loss_ratio       2.31 %  [1.80 to 2.85 %]  spread 0.52 %",
            "  delay.p99        45.300 ms  [41.100 to 50.200 ms]  spread 7.100 ms",
            "note: 1 metrics are missing in some runs and have no interval (media.delay.p50)",
            "3 metrics in all",
            "set: out/runset.json",
        ] {
            assert!(text.contains(expected), "{expected:?} is not in\n{text}");
        }
        // The headline order is fixed, and a metric that is not a headline (the count of sent packets) is not listed
        assert!(text.find("loss_ratio").unwrap() < text.find("delay.p99").unwrap());
        assert!(!text.contains("sent "), "{text}");
    }

    /// A set of runs that all reproduce has no warning, and a seed from the file is not called drawn.
    #[test]
    fn a_set_without_trouble_has_no_warning() {
        let text = SET
            .replace(
                "\"loss_reproducible\": false",
                "\"loss_reproducible\": true",
            )
            .replace("\"drawn\": true", "\"drawn\": false");
        let text = set_summary(&RunSetResult::from_json(&text).unwrap(), Path::new("out"));
        assert!(!text.contains("warning"), "{text}");
        assert!(text.contains("  seed 42\n"), "{text}");
    }

    #[test]
    fn the_summary_tells_what_ran_and_what_came_of_it() {
        let text = summary(&result(), Path::new("out"));
        assert_eq!(
            text,
            "netscenario run: lte-like-link
  backend kernel (user namespace, iproute2 6.15.0)
  seed 42 (drawn; repeat the run with --seed 42)

workload media  client -> server  2 mbit/s, 1200 byte packets
  sent 625  received 620  lost 5 (0.80 %)
  delay  min 31.2 ms  median 42.6 ms  p95 50.2 ms  p99 54.0 ms  max 57.0 ms
  throughput per second  1.91 mbit/s  1.98 mbit/s  1.99 mbit/s  76.8 kbit/s

result: out/result.json
"
        );
    }

    /// A source whose rate is a mean says so, and names its kind; a constant bit rate is as it was.
    #[test]
    fn a_pattern_other_than_a_constant_rate_is_named_with_its_mean_rate() {
        let poisson = RESULT.replace("\"kind\": \"udp-cbr\"", "\"kind\": \"udp-poisson\"");
        let text = summary(&RunResult::from_json(&poisson).unwrap(), Path::new("out"));
        assert!(
            text.contains("workload media  client -> server  udp-poisson, 2 mbit/s on average, 1200 byte packets"),
            "{text}"
        );
        assert!(summary(&result(), Path::new("out")).contains("server  2 mbit/s, 1200 byte"));
    }

    /// Canary: a queue that counted other packets than the workloads sent is named, with the numbers.
    #[test]
    fn a_queue_that_counted_a_packet_too_many_is_named() {
        let mut result = result();
        result.seed.loss_reproducible = false;
        result.seed.queue_checks = vec![netscenario_core::QueueCheck::new(
            "access", "forward", 417, 418,
        )];
        result.seed.not_reproducible_because = vec![
            "the queue of link 'access' (forward) counted 418 packets and the workloads sent 417, so something else entered it and the lost packets are not those of the seed".to_owned(),
        ];
        let text = summary(&result, Path::new("out"));
        assert!(
            text.contains("the queue of link 'access' (forward) counted 418 packets and the workloads sent 417"),
            "{text}"
        );
        assert!(!text.contains("cannot apply a seed"), "{text}");
        // Queues that agree say nothing
        result.seed.loss_reproducible = true;
        result.seed.not_reproducible_because.clear();
        result.seed.queue_checks[0].counted = 417;
        assert!(!summary(&result, Path::new("out")).contains("warning"));
    }

    /// Canary: the things that must not be hidden are shown.
    #[test]
    fn a_run_that_cannot_be_repeated_and_packets_that_were_refused_are_warned_about() {
        let mut result = result();
        result.seed.value = None;
        result.seed.loss_reproducible = false;
        result.seed.not_reproducible_because =
            vec!["the backend cannot apply a seed (iproute2 6.6.0 or newer can)".to_owned()];
        result.workloads[0].sender.send_errors = 3;
        result.workloads[0].sender.first_error = Some("no buffer space".to_owned());
        result.workloads[0].sender.refused = vec![[10, 11], [20, 20]];
        let text = summary(&result, Path::new("out"));
        assert!(
            text.contains("no seed (the backend cannot apply one)"),
            "{text}"
        );
        assert!(
            text.contains("warning: the loss of this run is not reproducible"),
            "{text}"
        );
        assert!(
            text.contains(
                "warning: the kernel refused 3 packets (packets 10 to 11, packet 20): no buffer space"
            ),
            "{text}"
        );
    }

    /// The tally of a direction is shown where the node dropped something, so that a corrupted packet is not
    /// taken for a lost one; a direction with only loss in the queue says nothing more than the workload does.
    #[test]
    fn a_direction_in_which_the_node_dropped_packets_is_accounted_for() {
        let mut json: serde_json::Value = serde_json::from_str(RESULT).unwrap();
        json["directions"] = serde_json::json!([
            {"link": "access", "direction": "forward", "sent": 625, "reached_node": 600, "delivered": 560,
             "lost_in_queue": 25, "dropped_corrupted": 40, "receive_buffer_drops": 0},
            {"link": "access", "direction": "reverse", "sent": 10, "reached_node": 9, "delivered": 9,
             "lost_in_queue": 1, "dropped_corrupted": 0, "receive_buffer_drops": 0}
        ]);
        let result = RunResult::from_json(&json.to_string()).unwrap();
        let text = summary(&result, Path::new("out"));
        assert!(
            text.contains("link access forward: of 625 packets sent, 25 were lost in the queue, 40 arrived damaged and were dropped by the node, 560 were delivered"),
            "{text}"
        );
        assert!(!text.contains("link access reverse"), "{text}");
        // A full receive buffer is named as the receiver's fault
        json["directions"][0]["receive_buffer_drops"] = serde_json::json!(3);
        let text = summary(
            &RunResult::from_json(&json.to_string()).unwrap(),
            Path::new("out"),
        );
        assert!(
            text.contains("3 were dropped because a receive buffer was full"),
            "{text}"
        );
        // Without a tally nothing is added
        assert!(!summary(&self::result(), Path::new("out")).contains("link access"));
    }

    #[test]
    fn reordering_is_shown_only_when_there_was_some() {
        let mut result = result();
        assert!(!summary(&result, Path::new("out")).contains("reordered"));
        result.workloads[0].stats.reordered = 337;
        assert!(
            summary(&result, Path::new("out"))
                .contains("sent 625  received 620  lost 5 (0.80 %)  reordered 337")
        );
    }

    #[test]
    fn the_steps_of_the_timeline_are_listed_with_how_late_they_were_and_only_when_there_are_some() {
        let result = result();
        assert!(!summary(&result, Path::new("out")).contains("timeline"));
        let mut json = serde_json::to_value(&result).unwrap();
        json["events"] = serde_json::json!([
            {"event": 0, "kind": "set", "link": "access", "direction": "forward",
             "scheduled_ns": 3_000_000_000_u64, "started_ns": 3_000_100_000_u64, "applied_ns": 3_001_900_000_u64},
            {"event": 1, "kind": "down", "link": "access", "direction": null,
             "scheduled_ns": 5_000_000_000_u64, "started_ns": 5_000_000_000_u64, "applied_ns": 5_000_400_000_u64},
            {"event": 2, "kind": "set", "link": "access", "direction": "reverse",
             "scheduled_ns": 6_000_000_000_u64, "started_ns": 5_997_000_000_u64, "applied_ns": 5_999_000_000_u64,
             "packet": 2250, "held_ns": 1500}
        ]);
        let result = RunResult::from_json(&json.to_string()).unwrap();
        let text = summary(&result, Path::new("out"));
        assert!(
            text.contains("at 3.000 s  link access forward  set  done 1.9 ms late"),
            "{text}"
        );
        assert!(
            text.contains("at 5.000 s  link access both ways  down  done 0.4 ms late"),
            "{text}"
        );
        assert!(
            text.contains(
                "at 6.000 s  link access reverse  set  before packet 2250, which waited 0.002 ms"
            ),
            "{text}"
        );
    }

    #[test]
    fn the_time_to_recover_is_shown_for_each_event_and_the_reason_where_there_is_none() {
        let mut result = result();
        assert!(!summary(&result, Path::new("out")).contains("recovery after"));
        let mut json = serde_json::to_value(&result).unwrap();
        let base = serde_json::json!({
            "event": 0, "kind": "outage-end", "link": "access", "direction": "forward", "basis": "send",
            "from_ns": 13_000_000_000_u64, "disturbance_ns": 10_000_000_000_u64, "baseline": 1.0, "level": 0.9,
            "baseline_bins": 5, "window_bins": 3, "fraction": 0.9, "recovered_at_ns": 15_000_000_000_u64,
            "time_ns": 2_000_000_000_u64, "reason": null
        });
        let mut never = base.clone();
        never["event"] = 1.into();
        never["kind"] = "set".into();
        never["direction"] = serde_json::Value::Null;
        never["time_ns"] = serde_json::Value::Null;
        never["recovered_at_ns"] = serde_json::Value::Null;
        never["reason"] =
            "the level 0.900 was not reached again for 3 bins before the run ended".into();
        json["workloads"][0]["recoveries"] = serde_json::json!([base, never]);
        result = RunResult::from_json(&json.to_string()).unwrap();
        let text = summary(&result, Path::new("out"));
        assert!(
            text.contains("recovery after outage-end (event 0, link access forward): 2.000 s (baseline 100.0 %, back to 90.0 % for 3 bins, by send)"),
            "{text}"
        );
        assert!(
            text.contains("recovery after set (event 1, link access): no time: the level 0.900 was not reached again"),
            "{text}"
        );
    }

    #[test]
    fn an_interrupted_run_says_how_far_it_got_and_a_whole_one_says_nothing() {
        let mut result = result();
        assert!(!summary(&result, Path::new("out")).contains("interrupted"));
        result.timing.interrupted_ns = Some(12_340_000_000);
        let text = summary(&result, Path::new("out"));
        assert!(
            text.contains(
                "  interrupted after 12.340 s of 3.000 s: this is the result of the part that ran\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn what_the_backend_notes_about_the_run_is_shown_and_only_when_there_is_something() {
        let mut result = result();
        assert!(!summary(&result, Path::new("out")).contains("note:"));
        result.notes = vec!["the changes were made the slow way".to_owned()];
        assert!(
            summary(&result, Path::new("out"))
                .contains("  note: the changes were made the slow way\n")
        );
    }

    #[test]
    fn the_restrictions_the_scenario_accepted_are_shown_and_only_when_there_are_some() {
        let mut result = result();
        assert!(!summary(&result, Path::new("out")).contains("accepted restriction"));
        result.accepted_restrictions = vec![netscenario_core::AcceptedRestriction::new(
            "fast-rate-steps",
            "up to 2 % fewer bytes at steps of 20 ms",
        )];
        assert!(summary(&result, Path::new("out")).contains(
            "  accepted restriction (fast-rate-steps): up to 2 % fewer bytes at steps of 20 ms\n"
        ));
    }

    #[test]
    fn the_parameters_of_the_run_are_shown_and_only_when_there_are_some() {
        let mut result = result();
        assert!(!summary(&result, Path::new("out")).contains("parameters"));
        result.scenario.params = [("rtt", "40ms"), ("rate", "10mbit")]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        assert!(
            summary(&result, Path::new("out")).contains("\n  parameters rate=10mbit, rtt=40ms\n")
        );
    }

    #[test]
    fn damaged_packets_are_said_where_the_loss_is_and_only_when_there_were_some() {
        let mut result = result();
        assert!(!summary(&result, Path::new("out")).contains("damaged"));
        result.workloads[0].corrupted = 3;
        assert!(
            summary(&result, Path::new("out"))
                .contains("lost 5 (0.80 %, 3 of them arrived damaged)")
        );
    }

    #[test]
    fn damaged_packets_are_named_where_the_result_names_them_and_the_reason_is_shown_where_it_cannot()
     {
        let mut result = result();
        assert!(!summary(&result, Path::new("out")).contains("named"));
        result.workloads[0].corrupted_named = Some(12);
        result.workloads[0].corrupted_seq = (0..12).map(|n| n * 3 + 1).collect();
        let text = summary(&result, Path::new("out"));
        assert!(
            text.contains("named by sequence number: 12 (1, 4, 7, 10, 13, 16, 19, 22, ...)"),
            "{text}"
        );
        // Canary: none named is not mentioned, and a reason is
        result.workloads[0].corrupted_named = Some(0);
        assert!(!summary(&result, Path::new("out")).contains("named by sequence"));
        result.workloads[0].corrupted_named = None;
        result.workloads[0].corrupted_seq_unavailable = Some("CAP_NET_RAW is missing".to_owned());
        assert!(
            summary(&result, Path::new("out"))
                .contains("damaged packets could not be named: CAP_NET_RAW is missing")
        );
    }

    #[test]
    fn the_pattern_of_the_arrivals_is_shown_only_where_there_is_something_to_show() {
        let mut result = result();
        let plain = summary(&result, Path::new("out"));
        assert!(
            !plain.contains("reordering  extent") && !plain.contains("loss runs"),
            "{plain}"
        );
        let json = serde_json::to_value(&result).unwrap();
        let mut json = json;
        let stats = &mut json["workloads"][0];
        stats["reordering"] = serde_json::json!({
            "extent": {"max": 4, "mean": 1.5, "p50": 1, "p95": 4, "p99": 4},
            "late_time": {"max_ns": 3_000_000, "mean_ns": 1_000_000, "p50_ns": 1_000_000, "p95_ns": 3_000_000, "p99_ns": 3_000_000}
        });
        stats["jitter"] =
            serde_json::json!({"last_ns": 180_000, "mean_ns": 150_000, "max_ns": 210_000});
        stats["loss_runs"] = serde_json::json!({
            "runs": 3, "longest": 2, "mean": 1.67,
            "histogram": [{"length": 1, "runs": 2}, {"length": 2, "runs": 1}]
        });
        result = RunResult::from_json(&json.to_string()).unwrap();
        let text = summary(&result, Path::new("out"));
        assert!(
            text.contains(
                "reordering  extent max 4  mean 1.5  p95 4  late time max 3.0 ms  p95 3.0 ms"
            ),
            "{text}"
        );
        assert!(
            text.contains("jitter  mean 0.150 ms  last 0.180 ms  max 0.210 ms  (RFC 3550)"),
            "{text}"
        );
        assert!(
            text.contains("loss runs  3 runs  longest 2  mean 1.67"),
            "{text}"
        );
    }

    #[test]
    fn a_long_run_shows_the_average_and_not_every_second() {
        let mut result = result();
        result.workloads[0].stats.throughput.bits_per_second = vec![1.0; 54];
        result.workloads[0].duration_ns = 60_000_000_000;
        result.workloads[0].stats.received = 12_239;
        result.workloads[0].packet_size_bytes = 128;
        let text = summary(&result, Path::new("out"));
        // 12,239 packets of 128 bytes in 60 seconds: 12,532,736 bits, 208,878.9 bit/s
        assert!(
            text.contains(
                "  throughput  208.88 kbit/s on average over 60 s (54 bins of one second in result.json)"
            ),
            "{text}"
        );
        assert!(!text.contains("throughput per second"), "{text}");
    }

    #[test]
    fn a_workload_without_arrivals_says_so() {
        let mut result = result();
        result.workloads[0].stats.delay = None;
        result.workloads[0].stats.throughput.bits_per_second.clear();
        assert!(summary(&result, Path::new("out")).contains("delay  no packet arrived"));
    }

    #[test]
    fn rates_use_the_unit_that_reads_best() {
        assert_eq!(rate(0.0), "0 bit/s");
        assert_eq!(rate(999.0), "999 bit/s");
        assert_eq!(rate(1500.0), "1.5 kbit/s");
        assert_eq!(rate(2_000_000.0), "2 mbit/s");
        assert_eq!(rate(1_234_567_890.0), "1.23 gbit/s");
        assert_eq!(ms(20_016_000), "20.0 ms");
    }
    /// A real run with a TCP transfer, a program and the capture of a link (`tests/fixtures`): every section of the summary
    /// that a user reads for them, and then the warnings, each one from the field that raises it. The fixture is a result as
    /// `netscenario run` wrote it; a test changes the field and reads the summary again.
    const ALL: &str = include_str!("../tests/fixtures/all-sections.result.json");

    fn all_with(change: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut value: serde_json::Value = serde_json::from_str(ALL).unwrap();
        change(&mut value);
        summary(
            &RunResult::from_json(&value.to_string()).unwrap(),
            Path::new("out"),
        )
    }

    fn lacks_warning(text: &str) {
        assert!(!text.contains("warning:"), "{text}");
    }

    #[test]
    fn the_summary_of_a_run_with_tcp_a_program_and_a_capture_has_a_section_for_each() {
        let text = all_with(|_| {});
        for expected in [
            "tcp download  server -> client  tcp-bulk, 2 connections, cubic",
            "  sent 400000 bytes  received 400000 bytes",
            "  2 of 2 transfers complete, the last after ",
            "  goodput  2.59 gbit/s on average over the transfer",
            "  round trip time of the kernel  min 0.0 ms  median 0.0 ms  p95 0.0 ms  max 0.0 ms",
            "  retransmitted segments  0",
            "program tool  in server  sh -c echo 'metric latency 1.5'; echo hello; sleep 30",
            "(stopped, signal 15)",
            "  output  25 bytes in program-tool.stdout.log, 0 bytes of standard error in program-tool.stderr.log",
            "capture link access in client  560 frames, 437098 bytes, in capture-access-client.pcap",
            "  from 0.000 s to 0.002 s of the measured phase",
            "  whole: the interface l0 counted 560 frames, as many as the file has",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        lacks_warning(&text);
    }

    #[test]
    fn a_failed_or_interrupted_transfer_is_a_warning_with_the_reason() {
        let text = all_with(|v| {
            v["tcp"][0]["failed_connections"] = 1.into();
            v["tcp"][0]["first_error"] = "connection reset".into();
            v["tcp"][0]["stopped"] = true.into();
        });
        assert!(
            text.contains("  warning: 1 of 2 connections failed: connection reset"),
            "{text}"
        );
        assert!(
            text.contains("  the run was interrupted before the client was done"),
            "{text}"
        );
        let text = all_with(|v| v["tcp"][0]["failed_connections"] = 2.into());
        assert!(
            text.contains("  warning: 2 of 2 connections failed: no reason was given"),
            "{text}"
        );
        // One connection reads in the singular
        let text = all_with(|v| v["tcp"][0]["connections"] = 1.into());
        assert!(text.contains("tcp-bulk, 1 connection, cubic"), "{text}");
    }

    #[test]
    fn the_requests_of_a_workload_are_counted_and_timed() {
        let summary_of_delays = |ns: u64| {
            serde_json::json!({"min_ns": ns, "mean_ns": ns, "p50_ns": ns, "p90_ns": ns, "p95_ns": ns,
                "p99_ns": ns, "p999_ns": ns, "max_ns": ns})
        };
        let text = all_with(|v| {
            v["tcp"][0]["kind"] = "tcp-request".into();
            v["tcp"][0]["requests"] = serde_json::json!({
                "sent": 10, "answered": 9, "failed": 1, "max_wait_ns": 5_000_000,
                "latency": summary_of_delays(2_000_000), "round_trip": summary_of_delays(1_000_000)
            });
        });
        for expected in [
            "  requests  sent 10  answered 9  failed 1",
            "  latency from the moment due  min 2.0 ms  median 2.0 ms  p95 2.0 ms  p99 2.0 ms  max 2.0 ms",
            "  round trip from the moment sent  min 1.0 ms  median 1.0 ms  p95 1.0 ms  p99 1.0 ms  max 1.0 ms",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        // The bytes of a transfer are not shown for requests
        assert!(!text.contains("transfers complete"), "{text}");
    }

    #[test]
    fn what_went_wrong_with_a_program_is_a_warning_on_a_line_of_its_own() {
        let text = all_with(|v| {
            let program = &mut v["programs"][0];
            program["ended"] = "on-its-own".into();
            program["exit_code"] = 3.into();
            program["exit_signal"] = serde_json::Value::Null;
            program["output"]["stdout_dropped_bytes"] = 7.into();
            program["output"]["stderr_dropped_bytes"] = 9.into();
            program["output"]["long_lines"] = 2.into();
            program["lines_dropped"] = true.into();
            program["bad_metric_lines"] = 4.into();
            program["bad_metric_examples"] = serde_json::json!(["metric latency fast"]);
            program["metrics"] = serde_json::json!([{
                "name": "latency", "unit": "ms", "count": 3, "first": 1.0, "last": 1.5, "min": 1.0, "max": 2.0,
                "mean": 1.5, "sum": 4.5
            }]);
        });
        for expected in [
            "(on-its-own, exit code 3)",
            "  metric latency  3 readings, last 1.5 ms, min 1, max 2",
            "  warning: it ended by itself with an error, before the run asked it to stop",
            "  warning: the files hold the first megabyte; 7 bytes of standard output and 9 of standard error are not in them",
            "  warning: 2 lines of standard output were longer than 64 KiB and were cut",
            "  warning: lines of standard output were dropped before the run read them; metrics in them are lost",
            "  warning: 4 lines looked like metrics and were not valid, the first: metric latency fast",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        // A program that ends by itself with 0 has finished its work
        let text = all_with(|v| {
            v["programs"][0]["ended"] = "on-its-own".into();
            v["programs"][0]["exit_code"] = 0.into();
        });
        lacks_warning(&text);
        // No exit status at all
        let text = all_with(|v| v["programs"][0]["exit_signal"] = serde_json::Value::Null);
        assert!(text.contains("(stopped, no exit status)"), "{text}");
    }

    #[test]
    fn a_capture_that_lost_frames_or_could_not_check_itself_is_a_warning() {
        let text = all_with(|v| {
            v["captures"][0]["missing"] = 12.into();
            v["captures"][0]["packets"] = 548.into();
        });
        assert!(
            text.contains("  warning: 12 frames are missing: the interface l0 counted 560 and the file has 548 (the receive buffer of the capture was 425984 bytes; `net.core.rmem_max` limits it)"),
            "{text}"
        );
        let text = all_with(|v| {
            v["captures"][0]["missing"] = serde_json::Value::Null;
            v["captures"][0]["interface_frames"] = serde_json::Value::Null;
            v["captures"][0]["first_ns"] = serde_json::Value::Null;
        });
        assert!(
            text.contains("  warning: the counters of the interface could not be read, so the capture could not check that it is whole"),
            "{text}"
        );
        // Without a first frame there is no span to show for the client's file
        assert!(
            !text.contains("capture link access in client  560 frames, 437098 bytes, in capture-access-client.pcap\n  from"),
            "{text}"
        );
    }

    #[test]
    fn the_timeline_lists_its_steps_with_how_late_each_was_and_cuts_a_long_one() {
        let step = |index: u64, packet: Option<u64>, model: bool, live: bool| {
            serde_json::json!({
                "event": index, "kind": "set", "link": "access", "direction": if index == 1 { serde_json::Value::Null } else { "forward".into() },
                "scheduled_ns": index * 1_000_000_000, "started_ns": index * 1_000_000_000,
                "applied_ns": index * 1_000_000_000 + 2_500_000, "packet": packet,
                "held_ns": packet.map(|_| 1_200_000_u64), "from_model": model, "live": live
            })
        };
        let text = all_with(|v| {
            v["events"] = serde_json::json!([
                step(0, None, false, false),
                step(1, Some(42), true, false),
                step(2, None, false, true)
            ]);
        });
        for expected in [
            "timeline\n",
            "  at 0.000 s  link access forward  set  done 2.5 ms late",
            "  at 1.000 s  link access (model) both ways  set  before packet 42, which waited 1.200 ms",
            "  at 2.000 s  link access (from code) forward  set  done 2.5 ms late",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        // More than the summary lists: the first ones and the count
        let text = all_with(|v| {
            v["events"] =
                serde_json::Value::Array((0..25).map(|i| step(i, None, true, false)).collect());
        });
        assert!(
            text.contains("  and 5 more steps (all of them are in result.json)"),
            "{text}"
        );
        assert!(!text.contains("at 20.000 s"), "{text}");
        assert!(text.contains("at 19.000 s"), "{text}");
    }
}
