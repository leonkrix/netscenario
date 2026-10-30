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
    if !result.seed.loss_reproducible {
        let _ = writeln!(
            text,
            "  warning: the loss of this run is not reproducible: the backend cannot apply a seed (iproute2 6.6.0 or newer can)"
        );
    }
    for workload in &result.workloads {
        workload_text(&mut text, workload);
    }
    let _ = writeln!(text, "\nresult: {}", out_dir.join("result.json").display());
    text
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
    let _ = writeln!(
        text,
        "  sent {}  received {}  lost {} ({:.2} %)",
        stats.sent,
        stats.received,
        stats.lost,
        stats.loss_ratio * 100.0
    );
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
        "sent": 625, "received": 620, "lost": 5, "loss_ratio": 0.008, "duplicates": 0, "unexpected": 0,
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

    /// Canary: the things that must not be hidden are shown.
    #[test]
    fn a_run_that_cannot_be_repeated_and_packets_that_were_refused_are_warned_about() {
        let mut result = result();
        result.seed.value = None;
        result.seed.loss_reproducible = false;
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
