//! The summary of a run as text for people. The JSON is `result.json` itself (see `docs/RESULTS.md`).

use std::fmt::Write as _;
use std::path::Path;

use netscenario_core::{RunResult, WorkloadResult};

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
    for reason in &result.seed.not_reproducible_because {
        let _ = writeln!(
            text,
            "  warning: the loss of this run is not reproducible: {reason}"
        );
    }
    for workload in &result.workloads {
        workload_text(&mut text, workload);
    }
    directions_text(&mut text, result);
    events_text(&mut text, result);
    for note in &result.notes {
        let _ = writeln!(text, "  note: {note}");
    }
    let _ = writeln!(text, "\nresult: {}", out_dir.join("result.json").display());
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
    for event in &result.events {
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
            "  at {}  link {} {}  {}  {when}",
            seconds(event.scheduled_ns),
            event.link,
            event.direction.as_deref().unwrap_or("both ways"),
            event.kind,
        );
    }
}

/// Seconds with the decimals they need, from nanoseconds.
fn seconds(ns: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // moments are far below 2^52 nanoseconds
    let value = ns as f64 / 1e9;
    format!("{value:.3} s")
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
fn workload_text(text: &mut String, workload: &WorkloadResult) {
    let stats = &workload.stats;
    let _ = writeln!(
        text,
        "\nworkload {}  {} -> {}  {}, {} byte packets",
        workload.id,
        workload.from,
        workload.to,
        rate(
            #[allow(clippy::cast_precision_loss)] // rates are far below 2^52 bits per second
            {
                workload.rate_bits_per_second as f64
            }
        ),
        workload.packet_size_bytes
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
            (stats.received * workload.packet_size_bytes * 8) as f64 * 1e9
                / workload.duration_ns as f64
        };
        let _ = writeln!(
            text,
            "  throughput  {} on average over {} s ({} bins of one second in result.json)",
            rate(average),
            workload.duration_ns / 1_000_000_000,
            bins.len()
        );
    }
    if workload.sender.send_errors > 0 {
        let _ = writeln!(
            text,
            "  warning: the kernel refused {} packets: {}",
            workload.sender.send_errors,
            workload
                .sender
                .first_error
                .as_deref()
                .unwrap_or("no reason given")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A result as `netscenario run` writes it, reduced to one workload.
    const RESULT: &str = r#"{
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
            text.contains("warning: the kernel refused 3 packets: no buffer space"),
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
}
