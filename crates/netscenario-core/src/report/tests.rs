//! Tests of the reports: a fixed run is built from synthetic records, and each format is compared with a snapshot (QA-04).
//!
//! The run is made without a machine: records from a small generator, the series from the library function that a real run
//! uses, the statistics from `stats::analyse`. The snapshots are in `snapshots/` (`INSTA_UPDATE=always` rewrites them; read
//! the diff first). The sample report in `docs/sample-report/` is what the code writes for this run
//! (`UPDATE_SAMPLE=1 cargo test -p netscenario-core --lib report` rewrites it).

use std::path::{Path, PathBuf};

use netscenario_agent::records::{Record, Writer};
use netscenario_agent::schedule::{Schedule, Stream};

use super::*;
use crate::result::tests::example;
use crate::result::{EventRecord, MetricSummary, ProgramOutput, ProgramResult, SeriesInfo};
use crate::series::{self, Input};
use crate::stats;

const RATE: u64 = 2_000_000;
const SIZE: u64 = 1200;
const ORIGIN: u64 = 5_000_000_000;
const SECOND: u64 = 1_000_000_000;

/// A small deterministic generator (a linear congruential one): the same numbers on every machine.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// The arrivals of 10 s at 2 mbit/s: a delay of 40 ms and up to 4 ms more (less than a gap, so the packets keep their order), a queue that builds up while the rate drops
/// from 4 s to 6 s, an outage at 8 s for 300 ms, and 1 % loss at random.
fn arrivals() -> (u64, Vec<Record>) {
    let schedule = Stream {
        bits_per_second: RATE,
        packet_size: SIZE,
    };
    let gap = SIZE * 8 * SECOND / RATE;
    let slots = 10 * SECOND / gap;
    let mut random = Lcg(42);
    let mut records = Vec::new();
    for n in 0..slots {
        let slot = schedule.offset_ns(n);
        let mut delay = 40_000_000 + random.next() % 4_000_000;
        let lost_at_random = random.next().is_multiple_of(100);
        // The queue builds up while the rate is low and drains in the second after it: the delay rises and falls by
        // less than a gap a packet, so the packets keep their order
        if (4 * SECOND..6 * SECOND).contains(&slot) {
            delay += (slot - 4 * SECOND) / 14;
        } else if (6 * SECOND..7 * SECOND).contains(&slot) {
            delay += (7 * SECOND - slot) / 7;
        }
        if (8 * SECOND..8 * SECOND + 300_000_000).contains(&slot) || lost_at_random {
            continue;
        }
        records.push(Record {
            sequence: n,
            send_ns: ORIGIN + slot,
            receive_ns: ORIGIN + slot + delay,
        });
    }
    // The packets that leave the queue are not in the order of their numbers: swap a few neighbours
    for place in (50..records.len() - 1).step_by(397) {
        let (a, b) = (records[place].receive_ns, records[place + 1].receive_ns);
        records[place].receive_ns = b;
        records[place + 1].receive_ns = a;
    }
    records.sort_by_key(|r| r.receive_ns);
    (slots, records)
}

/// The run of the snapshots, written to a directory: `result.json`, the series and the records.
pub(super) fn sample_run(dir: &Path) -> RunResult {
    std::fs::create_dir_all(dir).unwrap();
    let (slots, records) = arrivals();
    let mut writer = Writer::new(
        std::fs::File::create(dir.join("flow-0.records")).unwrap(),
        0,
    )
    .unwrap();
    for record in &records {
        writer.write(record).unwrap();
    }
    writer.finish().unwrap();
    let bins = series::bins(
        &Input {
            schedule: Schedule::Cbr(Stream {
                bits_per_second: RATE,
                packet_size: SIZE,
            }),
            start_ns: 0,
            origin_ns: ORIGIN,
            slots,
            refused: &[],
            records: &records,
        },
        SECOND,
    );
    let mut file = std::fs::File::create(dir.join("series-media.jsonl")).unwrap();
    series::write_series(&mut file, "media", &bins).unwrap();

    let mut result = example();
    result.scenario.name = "lte-like-link".to_owned();
    result.scenario.hash =
        "sha256:6771b8f9c6e4a4503da551740c3f9c375a478d5c67908b7b0f28e73da9dca1a9".to_owned();
    result.provenance.netscenario = "0.2.0".to_owned();
    result.provenance.started_at = "2026-11-03T10:00:00Z".to_owned();
    result.provenance.backend_details =
        std::collections::BTreeMap::from([("tc".to_owned(), "iproute2 6.15.0".to_owned())]);
    result.provenance.kernel = Some("6.12.1".to_owned());
    result.provenance.cpu_model = Some("Example CPU".to_owned());
    result.seed.value = Some(42);
    result.seed.drawn = false;
    result.timing.duration_ns = 10 * SECOND;
    result.timing.settle_ns = 1_120_000_000;
    let workload = &mut result.workloads[0];
    workload.id = "media".to_owned();
    workload.from = "client".to_owned();
    workload.to = "server".to_owned();
    workload.rate_bits_per_second = RATE;
    workload.packet_size_bytes = SIZE;
    workload.duration_ns = 10 * SECOND;
    workload.stats = stats::analyse(slots, &records, SIZE, ORIGIN);
    workload.records_file = "flow-0.records".to_owned();
    workload.sender.send_errors = 0;
    workload.sender.first_error = None;
    workload.sender.refused = Vec::new();
    workload.series = Some(SeriesInfo {
        bin_ns: SECOND,
        bins: bins.len() as u64,
        file: "series-media.jsonl".to_owned(),
    });
    let mut drop = EventRecord::new(0, 4 * SECOND, 4 * SECOND, 4 * SECOND + 1_800_000);
    drop.link = "access".to_owned();
    let mut outage = EventRecord::new(1, 8 * SECOND, 8 * SECOND, 8 * SECOND + 2_100_000);
    outage.link = "access".to_owned();
    outage.kind = "outage-start".to_owned();
    result.events = vec![drop, outage];
    result.notes = Vec::new();
    std::fs::write(dir.join("result.json"), result.to_json()).unwrap();
    result
}

fn directory(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("netscenario-report-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn load(name: &str, detail: bool) -> Report {
    let dir = directory(name);
    sample_run(&dir);
    Report::load(&dir, detail).unwrap()
}

// ---- the sample run itself ----

#[test]
fn the_sample_run_has_the_events_it_was_made_with() {
    let Report::Run(run) = load("sample", true) else {
        panic!("a run")
    };
    let stats = &run.result.workloads[0].stats;
    // 10 s at 2 mbit/s with 1200 byte payloads: a packet every 4.8 ms
    assert_eq!(stats.sent, 2083);
    // The outage of 300 ms (62 packets at 4.8 ms) and the loss at random took packets, but not all
    assert!((60..200).contains(&stats.lost), "{}", stats.lost);
    assert!(stats.reordered > 0, "the swapped neighbours are reordered");
    assert!(
        stats.reordered < 40,
        "the queue drains without reordering: {}",
        stats.reordered
    );
    let detail = &run.details[0];
    assert_eq!(
        detail.bins.len(),
        11,
        "ten bins of the run and one for the last arrivals"
    );
    assert_eq!(detail.delays_ns.len() as u64, stats.received);
    // The outage is in the bin of 8 s: nothing was lost before, and the loss there is in the order of 300 ms of 1 s
    assert!(
        detail.bins[8].by_send.lost >= 60,
        "{:?}",
        detail.bins[8].by_send
    );
}

// ---- Markdown ----

#[test]
fn the_markdown_of_a_run() {
    insta::assert_snapshot!(markdown(&load("md", false)));
}

#[test]
fn markdown_warns_where_a_run_was_cut_short_or_cannot_repeat() {
    let Report::Run(mut run) = load("md-warn", false) else {
        panic!("a run")
    };
    run.result.timing.interrupted_ns = Some(4 * SECOND);
    run.result.seed.loss_reproducible = false;
    run.result.seed.not_reproducible_because = vec!["the scenario has an outage".to_owned()];
    let text = markdown(&Report::Run(run));
    assert!(
        text.contains("> **Interrupted** after 4 s of 10 s"),
        "{text}"
    );
    assert!(text.contains("> **Warning:** the loss of this run does not repeat from its seed: the scenario has an outage"), "{text}");
    assert!(
        text.contains("> **media:** the run was interrupted"),
        "{text}"
    );
}

/// A restriction that the scenario accepted is said in the text and in the page, next to the numbers it limits, and only when there
/// is one (D-092).
#[test]
fn the_restrictions_a_scenario_accepted_are_said_in_both_reports() {
    let Report::Run(mut run) = load("md-limited", true) else {
        panic!("a run")
    };
    assert!(!markdown(&Report::Run(run.clone())).contains("Accepted restriction"));
    assert!(
        !html(&Report::Run(run.clone()))
            .unwrap()
            .contains("Accepted restriction")
    );
    run.result.accepted_restrictions = vec![crate::result::AcceptedRestriction::new(
        "fast-rate-steps",
        "up to 2 % fewer bytes <at> steps of 20 ms",
    )];
    let text = markdown(&Report::Run(run.clone()));
    assert!(
        text.contains("> **Accepted restriction** (fast-rate-steps): up to 2 % fewer bytes <at> steps of 20 ms"),
        "{text}"
    );
    let page = html(&Report::Run(run)).unwrap();
    assert!(
        page.contains("<strong>Accepted restriction</strong> (fast-rate-steps): up to 2 % fewer bytes &lt;at&gt; steps of 20 ms"),
        "{page}"
    );
}

/// The parameters of the scenario are said in both reports, in the table with the other facts of the run, and only when there
/// are some (D-093).
#[test]
fn the_parameters_of_a_scenario_are_said_in_both_reports() {
    let Report::Run(mut run) = load("md-params", true) else {
        panic!("a run")
    };
    assert!(!markdown(&Report::Run(run.clone())).contains("| Parameters |"));
    assert!(
        !html(&Report::Run(run.clone()))
            .unwrap()
            .contains("<dt>Parameters</dt>")
    );
    run.result.scenario.params = [("rtt", "40ms"), ("label", "<a|b>")]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    let text = markdown(&Report::Run(run.clone()));
    assert!(
        text.contains("| Parameters | label=<a\\|b>, rtt=40ms |"),
        "{text}"
    );
    let page = html(&Report::Run(run)).unwrap();
    assert!(
        page.contains("<dt>Parameters</dt><dd>label=&lt;a|b&gt;, rtt=40ms</dd>"),
        "{page}"
    );
}

/// A TCP workload with two connections of which one failed, a goodput by bin and a recovery.
fn a_tcp() -> crate::result::TcpResult {
    crate::result::TcpResult {
        id: "down".to_owned(),
        kind: "tcp-bulk".to_owned(),
        from: "client".to_owned(),
        to: "server".to_owned(),
        from_address: "10.200.0.1".to_owned(),
        to_address: "10.200.0.2:6000".to_owned(),
        congestion: Some("reno".to_owned()),
        connections: 2,
        failed_connections: 1,
        start_ns: 0,
        window_ns: SECOND,
        bytes_sent: 1_000,
        bytes_received: 900,
        goodput: Some(crate::result::Goodput {
            bin_ns: SECOND,
            bits_per_second: vec![8_000_000, 6_000_000, 0, 9_000_000],
            mean_bits_per_second: Some(7_000_000),
        }),
        transfers: vec![crate::result::TransferInfo {
            connection: 0,
            started_ns: 0,
            established_ns: 1,
            done_ns: 2 * i64::try_from(SECOND).unwrap(),
            bytes_sent: 1_000,
            bytes_received: 0,
            ok: true,
        }],
        retransmitted_segments: 4,
        rtt: None,
        requests: None,
        first_error: Some("the server closed the connection".to_owned()),
        info_error: None,
        stopped: false,
        client_file: "c".to_owned(),
        server_file: "s".to_owned(),
        recoveries: vec![crate::recovery::Recovery {
            event: 0,
            kind: "set".to_owned(),
            link: "access".to_owned(),
            direction: Some("forward".to_owned()),
            basis: crate::recovery::Basis::Goodput,
            from_ns: SECOND,
            disturbance_ns: SECOND,
            baseline: Some(8_000_000.0),
            level: Some(7_200_000.0),
            baseline_bins: 5,
            window_bins: 3,
            fraction: 0.9,
            recovered_at_ns: Some(3 * SECOND),
            time_ns: Some(2 * SECOND),
            reason: None,
        }],
    }
}

/// A TCP workload has a section of its own, and a run of TCP workloads only does not say that it has no workload.
#[test]
fn markdown_has_a_section_for_the_tcp_workloads_and_says_the_run_is_statistical() {
    let Report::Run(mut run) = load("md-tcp", false) else {
        panic!("a run")
    };
    run.result.workloads.clear();
    run.result.tcp = vec![a_tcp()];
    run.result.reproducibility =
        crate::result::Reproducibility::statistical(vec!["feedback".to_owned()]);
    let text = markdown(&Report::Run(run));
    assert!(text.contains("### TCP workloads"), "{text}");
    assert!(!text.contains("None."), "{text}");
    assert!(text.contains("| Congestion control | reno |"), "{text}");
    assert!(text.contains("| Connections (failed) | 2 (1) |"), "{text}");
    assert!(text.contains("| Completed after | 2 s |"), "{text}");
    assert!(
        text.contains("> **down:** 1 of 2 connections failed: the server closed the connection"),
        "{text}"
    );
    assert!(
        text.contains("> A run with TCP is statistical: feedback"),
        "{text}"
    );
}

/// A workload with an echo has a table of its own for the round trip, and the JUnit properties name it.
#[test]
fn the_round_trip_of_an_echo_is_in_the_markdown_and_in_the_properties() {
    let Report::Run(mut run) = load("md-echo", false) else {
        panic!("a run")
    };
    let stats = run.result.workloads[0].stats.clone();
    let mut back = stats.clone();
    back.received = stats.received - 3;
    back.lost = stats.lost + 3;
    back.delay = back.delay.map(|mut trip| {
        trip.min_ns = 80_000_000;
        trip.p50_ns = 85_000_000;
        trip
    });
    run.result.workloads[0].echo = Some(crate::result::EchoResult {
        echoed: stats.received,
        echo_errors: 0,
        foreign_datagrams: 0,
        records_file: "flow-0.echo.records".to_owned(),
        stats: back,
    });
    let text = markdown(&Report::Run(run.clone()));
    assert!(text.contains("### Echo"), "{text}");
    assert!(text.contains("| Round trip min | 80.00 ms |"), "{text}");
    assert!(text.contains("| Round trip p50 | 85.00 ms |"), "{text}");
    assert!(text.contains("| Came back | 2000 |"), "{text}");
    let junit = junit(&Report::Run(run));
    assert!(
        junit.contains(".round_trip_min_s\" value=\"0.08\""),
        "{junit}"
    );
    // A run without an echo has neither
    let Report::Run(plain) = load("md-no-echo", false) else {
        panic!("a run")
    };
    assert!(!markdown(&Report::Run(plain)).contains("### Echo"));
}

/// The TCP workloads are in the HTML with their goodput over time, in the table of the times to recover and in the properties of
/// the JUnit file; a recovery of a TCP workload is in the Markdown too.
#[test]
fn the_tcp_workloads_are_in_the_html_the_recoveries_and_the_properties() {
    let Report::Run(mut run) = load("html-tcp", true) else {
        panic!("a run")
    };
    run.result.workloads.clear();
    run.details.clear();
    run.result.tcp = vec![a_tcp()];
    run.result.reproducibility =
        crate::result::Reproducibility::statistical(vec!["feedback".to_owned()]);
    let page = html(&Report::Run(run.clone())).unwrap();
    assert!(page.contains("<h2>down</h2>"), "{page}");
    assert!(page.contains("down: goodput over time"), "{page}");
    assert!(
        page.contains("A run with TCP is statistical: feedback"),
        "{page}"
    );
    // The time to recover: the row of the workload and its time
    assert!(page.contains("<h3>Time to recover</h3>"), "{page}");
    assert!(page.contains("event 0 (set)"), "{page}");
    assert_no_external_reference(&page);
    let text = markdown(&Report::Run(run.clone()));
    assert!(text.contains("### Time to recover"), "{text}");
    assert!(
        text.contains("| down | event 0 (set) | access | 2 s |"),
        "{text}"
    );
    let junit = junit(&Report::Run(run));
    // A connection that failed with no link shut on purpose fails the case of the workload
    assert!(
        junit.contains("<testcase classname=\"lte-like-link\" name=\"down\""),
        "{junit}"
    );
    assert!(
        junit.contains("1 of 2 connections of this workload failed"),
        "{junit}"
    );
    for name in ["goodput_mean_bps", "completion_s", "failed_connections"] {
        assert!(
            junit.contains(&format!("name=\"netscenario.lte-like-link.down.{name}\"")),
            "{name} is not in\n{junit}"
        );
    }
}

/// The numbers of a TCP workload under the names of a set of runs (D-088): seconds, bits per second and counts.
#[test]
fn the_names_of_the_metrics_of_a_tcp_workload() {
    let mut tcp = a_tcp();
    tcp.requests = Some(crate::result::RequestInfo {
        sent: 10,
        answered: 9,
        failed: 1,
        latency: crate::stats::DelaySummary::of(&mut [10_000_000, 20_000_000, 90_000_000]),
        round_trip: crate::stats::DelaySummary::of(&mut [10_000_000, 20_000_000]),
        max_wait_ns: 0,
    });
    let Report::Run(mut run) = load("metrics-tcp", false) else {
        panic!("a run")
    };
    run.result.tcp = vec![tcp];
    let values = crate::runset::metric_values(&run.result);
    let at = |name: &str| {
        *values
            .get(name)
            .unwrap_or_else(|| panic!("no {name} in {values:?}"))
    };
    assert!((at("down.goodput.mean") - 7_000_000.0).abs() < 1e-6);
    // From the first connect, 0, to the end of the last connection that did its work, 2 s
    assert!((at("down.completion") - 2.0).abs() < 1e-9);
    assert!((at("down.failed_connections") - 1.0).abs() < 1e-12);
    assert!((at("down.sent") - 10.0).abs() < 1e-12);
    assert!((at("down.failed") - 1.0).abs() < 1e-12);
    assert!((at("down.latency.max") - 0.090).abs() < 1e-12);
    // The nearest rank of the median of two times is the smaller
    assert!((at("down.round_trip.p50") - 0.010).abs() < 1e-12);
    // No round trip time of the kernel in the sample: no name for it
    assert!(!values.contains_key("down.rtt.p50"));
}

#[test]
fn a_pipe_in_a_name_does_not_end_the_cell_of_a_markdown_table() {
    let Report::Run(mut run) = load("md-pipe", false) else {
        panic!("a run")
    };
    run.result.workloads[0].id = "a|b".to_owned();
    let text = markdown(&Report::Run(run));
    assert!(text.contains("a\\|b"), "{text}");
}

/// A program of your own that reported one metric.
fn a_program() -> ProgramResult {
    ProgramResult {
        id: "app".to_owned(),
        node: "server".to_owned(),
        run: vec!["./app".to_owned()],
        started_ns: -500_000_000,
        ready_ns: -400_000_000,
        ended_ns: 10_100_000_000,
        ended: "stopped".to_owned(),
        exit_code: Some(0),
        exit_signal: None,
        stdout_file: "program-app.stdout.log".to_owned(),
        stderr_file: "program-app.stderr.log".to_owned(),
        output: ProgramOutput {
            stdout_bytes: 10,
            stdout_dropped_bytes: 0,
            stderr_bytes: 0,
            stderr_dropped_bytes: 0,
            long_lines: 0,
        },
        lines_dropped: false,
        metrics: vec![MetricSummary {
            name: "frames".to_owned(),
            unit: Some("1/s".to_owned()),
            count: 10,
            first: 29.0,
            last: 29.7,
            min: 28.0,
            max: 30.0,
        }],
        metrics_file: None,
        metric_readings: 10,
        bad_metric_lines: 0,
        bad_metric_examples: Vec::new(),
    }
}

// ---- JUnit ----

#[test]
fn the_junit_of_a_run() {
    insta::assert_snapshot!(junit(&load("junit", false)));
}

/// The XML is well formed: every tag is closed and the names and values of the properties are escaped.
#[test]
fn junit_escapes_what_a_name_may_contain() {
    let Report::Run(mut run) = load("junit-escape", false) else {
        panic!("a run")
    };
    run.result.scenario.name = "a<b&\"c\"".to_owned();
    run.result.workloads[0].id = "w'<>".to_owned();
    let text = junit(&Report::Run(run));
    assert!(!text.contains("a<b"), "{text}");
    assert!(text.contains("a&lt;b&amp;&quot;c&quot;"), "{text}");
    assert!(text.contains("w&#39;&lt;&gt;"), "{text}");
    let opening = text.matches("<testcase ").count();
    assert_eq!(opening, text.matches("</testcase>").count());
}

/// A case fails if the run was interrupted, or if the kernel refused packets and no link was shut on purpose; the canaries
/// are the same run without the cause, which passes.
#[test]
fn a_junit_case_fails_only_for_the_reasons_of_the_decision() {
    let fails = |run: &RunReport| junit(&Report::Run(Box::new(run.clone()))).contains("<failure ");
    let Report::Run(run) = load("junit-fail", false) else {
        panic!("a run")
    };
    assert!(!fails(&run), "a clean run passes");

    let mut interrupted = (*run).clone();
    interrupted.result.timing.interrupted_ns = Some(3 * SECOND);
    assert!(fails(&interrupted));
    assert!(
        junit(&Report::Run(Box::new(interrupted)))
            .contains("the run was interrupted after 3 s of 10 s")
    );

    let mut refused = (*run).clone();
    refused.result.workloads[0].sender.send_errors = 5;
    refused.result.workloads[0].sender.first_error = Some("Network is unreachable".to_owned());
    assert!(fails(&refused), "refused packets without a down step fail");
    assert!(
        junit(&Report::Run(Box::new(refused.clone()))).contains("the kernel refused 5 packets")
    );

    // The same refusals while a link is shut on purpose are what the step is for
    let mut down = refused;
    down.result.events[0].kind = "down".to_owned();
    assert!(!fails(&down));
    assert!(junit(&Report::Run(Box::new(down))).contains("while a link was shut on purpose"));

    // A warning about reproducibility is no failure
    let mut warned = (*run).clone();
    warned.result.seed.loss_reproducible = false;
    warned.result.seed.not_reproducible_because = vec!["an outage".to_owned()];
    assert!(!fails(&warned));
}

#[test]
fn the_properties_have_the_names_of_the_plugin() {
    let text = junit(&load("junit-names", false));
    for name in [
        "netscenario.lte-like-link.media.sent",
        "netscenario.lte-like-link.media.received",
        "netscenario.lte-like-link.media.lost",
        "netscenario.lte-like-link.media.loss_ratio",
        "netscenario.lte-like-link.media.reordered",
        "netscenario.lte-like-link.media.delay_p50_s",
        "netscenario.lte-like-link.media.delay_p99_s",
    ] {
        assert!(
            text.contains(&format!("name=\"{name}\"")),
            "{name} is not in\n{text}"
        );
    }
}

#[test]
fn a_program_is_a_case_with_its_metrics_as_properties() {
    let Report::Run(mut run) = load("junit-program", false) else {
        panic!("a run")
    };
    run.result.programs = vec![a_program()];
    let text = junit(&Report::Run(run));
    assert!(
        text.contains("<testcase classname=\"lte-like-link\" name=\"app\""),
        "{text}"
    );
    assert!(
        text.contains("name=\"netscenario.lte-like-link.app.frames\""),
        "{text}"
    );
}

// ---- HTML ----

#[test]
fn the_charts_of_a_run() {
    let Report::Run(run) = load("charts", true) else {
        panic!("a run")
    };
    let page = html(&Report::Run(run)).unwrap();
    // Each drawing alone is a snapshot, so that a change of one shows as a diff of that drawing
    let drawings: Vec<&str> = page
        .split("<svg ")
        .skip(1)
        .map(|part| part.split("</svg>").next().unwrap())
        .collect();
    assert_eq!(
        drawings.len(),
        4,
        "throughput, delay, loss and the distribution of the delays"
    );
    for (name, drawing) in ["throughput", "delay", "loss", "distribution"]
        .iter()
        .zip(&drawings)
    {
        insta::assert_snapshot!(*name, format!("<svg {drawing}</svg>"));
    }
}

/// The page is one file with nothing to fetch (RPT-03): no address, no script, no link, no image, no import, no font.
#[test]
fn the_html_has_no_external_reference() {
    let Report::Run(run) = load("html-offline", true) else {
        panic!("a run")
    };
    let page = html(&Report::Run(run)).unwrap();
    assert_no_external_reference(&page);
    // The check can fail: a page with a reference is found
    for bad in [
        "<img src=\"x.png\">",
        "<script>1</script>",
        "<link rel=\"stylesheet\" href=\"a.css\">",
        "<a href=\"https://example.org\">",
        "<style>@import url(a.css);</style>",
        "<svg xmlns=\"http://www.w3.org/2000/svg\">",
        "<iframe></iframe>",
    ] {
        let found = std::panic::catch_unwind(|| assert_no_external_reference(bad));
        assert!(found.is_err(), "{bad} should have been found");
    }
}

fn assert_no_external_reference(page: &str) {
    for needle in [
        "http://",
        "https://",
        "//cdn",
        "src=",
        "href=",
        "<script",
        "<link",
        "<img",
        "<iframe",
        "@import",
        "url(",
        "@font-face",
        "<object",
        "<embed",
    ] {
        assert!(!page.contains(needle), "the page contains '{needle}'");
    }
}

#[test]
fn the_html_of_a_run_loaded_without_detail_is_refused() {
    let error = html(&load("html-nodetail", false)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("needs the series and the records"),
        "{error}"
    );
}

#[test]
fn the_html_names_the_events_and_the_warnings() {
    let Report::Run(mut run) = load("html-text", true) else {
        panic!("a run")
    };
    run.result.seed.loss_reproducible = false;
    run.result.seed.not_reproducible_because = vec!["the scenario has an outage".to_owned()];
    let page = html(&Report::Run(run)).unwrap();
    for expected in [
        "<h1>netscenario: lte-like-link</h1>",
        "<h2>media</h2>",
        "<h2>Timeline</h2>",
        "outage-start",
        "the loss of this run does not repeat from its seed: the scenario has an outage",
        "prefers-color-scheme:dark",
    ] {
        assert!(page.contains(expected), "{expected} is not in the page");
    }
}

#[test]
fn text_in_the_html_is_escaped() {
    let Report::Run(mut run) = load("html-escape", true) else {
        panic!("a run")
    };
    run.result.scenario.name = "<b>x</b>".to_owned();
    run.result.workloads[0].from = "a&b".to_owned();
    let page = html(&Report::Run(run)).unwrap();
    assert!(!page.contains("<b>x</b>"));
    assert!(page.contains("&lt;b&gt;x&lt;/b&gt;"));
    assert!(page.contains("a&amp;b"));
}

#[test]
fn a_result_without_a_series_has_the_page_without_the_charts_over_time() {
    let Report::Run(mut run) = load("html-noseries", true) else {
        panic!("a run")
    };
    run.details[0].bins.clear();
    let page = html(&Report::Run(run)).unwrap();
    assert!(page.contains("has no series by time bin"), "{page}");
    assert_eq!(
        page.matches("<svg ").count(),
        1,
        "only the distribution of the delays"
    );
}

// ---- sets ----

fn a_set() -> Report {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../python/netscenario/tests/fixtures/runset.json"),
    )
    .unwrap();
    Report::Set(
        Box::new(RunSetResult::from_json(&text).unwrap()),
        PathBuf::from("set"),
    )
}

#[test]
fn the_markdown_of_a_set() {
    insta::assert_snapshot!(markdown(&a_set()));
}

#[test]
fn the_junit_of_a_set() {
    insta::assert_snapshot!(junit(&a_set()));
}

#[test]
fn the_html_of_a_set_draws_the_runs_against_the_interval_and_has_no_external_reference() {
    let page = html(&a_set()).unwrap();
    assert_no_external_reference(&page);
    assert!(
        page.contains("<h1>netscenario: t, 6 repetitions</h1>"),
        "{page}"
    );
    assert!(
        page.contains("Without an interval"),
        "a metric that some runs lack is listed"
    );
    // One drawing for each headline metric that every run has: the loss and the throughput
    assert_eq!(page.matches("<svg ").count(), 2, "{page}");
    assert_eq!(
        page.matches("class=\"run\"").count(),
        2 * 6,
        "a dot for every run"
    );
    let Report::Set(set, _) = a_set() else {
        panic!("a set")
    };
    let loss = set.metric("w.loss_ratio").unwrap();
    let plot = svg::interval_plot(&svg::IntervalPlot {
        title: "w.loss_ratio",
        description: String::new(),
        values: &loss.values,
        estimate: loss.interval.estimate,
        interval: (loss.interval.low, loss.interval.high),
        format: &|v| tick_for_test(v),
    });
    insta::assert_snapshot!("interval_plot", plot);
}

fn tick_for_test(value: f64) -> String {
    svg::tick(value)
}

// ---- the directory ----

#[test]
fn a_directory_that_is_no_result_is_refused_and_a_set_is_found() {
    let nothing = directory("nothing");
    std::fs::create_dir_all(&nothing).unwrap();
    assert!(matches!(
        Report::load(&nothing, false),
        Err(ReportError::NotAResult(_))
    ));
    let set = directory("set-dir");
    std::fs::create_dir_all(&set).unwrap();
    std::fs::write(set.join("runset.json"), "{}").unwrap();
    assert!(
        matches!(Report::load(&set, false), Err(ReportError::Bad { .. })),
        "a set that cannot be read is named"
    );
    let broken = directory("broken");
    sample_run(&broken);
    std::fs::write(broken.join("result.json"), "{}").unwrap();
    let error = Report::load(&broken, false).unwrap_err();
    assert!(error.to_string().contains("result.json"), "{error}");
    let truncated = directory("truncated");
    sample_run(&truncated);
    std::fs::write(truncated.join("series-media.jsonl"), "{\"bin\": 0\n").unwrap();
    let error = Report::load(&truncated, true).unwrap_err();
    assert!(
        error.to_string().contains("series-media.jsonl: line 1"),
        "{error}"
    );
}

#[test]
fn the_formats_are_read_from_their_names() {
    use std::str::FromStr;
    assert_eq!(Format::from_str("md"), Ok(Format::Markdown));
    assert_eq!(Format::from_str("markdown"), Ok(Format::Markdown));
    assert_eq!(Format::from_str("junit"), Ok(Format::Junit));
    assert_eq!(Format::from_str("html"), Ok(Format::Html));
    assert!(
        Format::from_str("pdf")
            .unwrap_err()
            .contains("md, html or junit")
    );
}

// ---- the sample report in the repository ----

/// `docs/sample-report/` is what the code writes for the sample run. Rewrite it with `UPDATE_SAMPLE=1`.
#[test]
fn the_sample_report_in_the_repository_is_what_the_code_writes() {
    let dir = directory("sample-report");
    sample_run(&dir);
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/sample-report");
    for (format, file) in [
        (Format::Markdown, "report.md"),
        (Format::Junit, "junit.xml"),
        (Format::Html, "report.html"),
    ] {
        let written = report(&dir, format).unwrap();
        let path = target.join(file);
        if std::env::var_os("UPDATE_SAMPLE").is_some() {
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(&path, &written).unwrap();
        }
        let found = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "{}: {error} (write it with UPDATE_SAMPLE=1)",
                path.display()
            )
        });
        assert!(
            found == written,
            "{} is not what the code writes: run `UPDATE_SAMPLE=1 cargo test -p netscenario-core --lib report` and read the diff",
            path.display()
        );
    }
}

/// The names of the metrics that the model lists for a scenario (they are what `expect` is checked against when the file is read)
/// and the names that a run reports (`metric_values`) are one set: a metric of a run that the model does not know could not be
/// checked, and the reverse would let a typo through. Every name of a run with every kind of workload is known to its scenario.
#[test]
fn the_metrics_a_run_reports_are_metrics_of_its_scenario() {
    use crate::result::RequestInfo;
    use crate::stats::DelaySummary;

    let summary = DelaySummary {
        min_ns: 1,
        mean_ns: 2,
        p50_ns: 2,
        p90_ns: 3,
        p95_ns: 4,
        p99_ns: 5,
        p999_ns: 6,
        max_ns: 7,
    };
    let mut result = example();
    result.workloads[0].duration_ns = SECOND;
    result.workloads[0].stats.delay = Some(summary.clone());
    result.workloads[0].stats.jitter = Some(crate::patterns::Jitter {
        last_ns: 1,
        mean_ns: 2,
        max_ns: 3,
    });
    result.workloads[0].echo = Some(crate::result::EchoResult {
        echoed: 1,
        echo_errors: 0,
        foreign_datagrams: 0,
        records_file: "flow-0.echo.records".to_owned(),
        stats: result.workloads[0].stats.clone(),
    });
    let mut bulk = a_tcp();
    bulk.rtt = Some(summary.clone());
    let mut asking = a_tcp();
    asking.id = "ask".to_owned();
    asking.kind = "request-response".to_owned();
    asking.rtt = Some(summary.clone());
    asking.requests = Some(RequestInfo {
        sent: 3,
        answered: 2,
        failed: 1,
        latency: Some(summary.clone()),
        round_trip: Some(summary),
        max_wait_ns: 0,
    });
    result.tcp = vec![bulk, asking];
    let id = result.workloads[0].id.clone();
    let scenario = netscenario_model::Scenario::from_yaml(
        &format!(
            "version: 0\nname: t\nduration: 1s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\nworkloads:\n  - {{ id: {id}, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b, echo: true }}\n  - {{ id: down, kind: tcp-bulk, from: client, to: server, bytes: 1mb }}\n  - {{ id: ask, kind: request-response, from: client, to: server, request: 100b, response: 1kb, interval: 100ms }}\n"
        ),
        "t.yaml",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let known = scenario.metric_names();
    let reported = crate::runset::metric_values(&result);
    assert!(reported.len() > 60, "{} names", reported.len());
    for name in reported.keys() {
        assert!(
            known.contains(name),
            "{name} is reported by a run and unknown to the scenario"
        );
    }
    // The units agree too: the value of a delay is in seconds in the run, and the model says seconds
    for name in reported.keys() {
        let unit = scenario.metric_unit_of(name).expect(name);
        if name.contains(".delay.") {
            assert_eq!(unit, netscenario_model::MetricUnit::Seconds, "{name}");
        }
    }
    // And the model lists no name that a run with all of them does not report, except the ones that are not in this run
    let missing: Vec<&String> = known
        .iter()
        .filter(|name| !reported.contains_key(*name))
        .collect();
    assert!(missing.is_empty(), "{missing:?}");
}

// ---- the checks of the file (D-094) ----

/// A verdict with a check that holds, one that is missed and one that could not be judged.
fn a_verdict(runs: u32) -> crate::expect::ExpectResult {
    use crate::expect::{CheckResult, ExpectResult};
    use netscenario_model::{MetricUnit, Statistic};
    let line = |name: &str,
                unit,
                bound: &str,
                threshold,
                actual: Option<f64>,
                margin: Option<f64>,
                passed,
                note: Option<&str>| {
        CheckResult {
            name: name.to_owned(),
            metric: "w.loss_ratio".to_owned(),
            statistic: if runs > 1 {
                Statistic::Ci95High
            } else {
                Statistic::Value
            },
            unit,
            bound: bound.to_owned(),
            threshold,
            actual,
            margin,
            passed,
            note: note.map(str::to_owned),
        }
    };
    ExpectResult {
        passed: false,
        runs,
        checks: vec![
            line(
                "loss stays small",
                MetricUnit::Ratio,
                "< 5%",
                0.05,
                Some(0.012),
                Some(0.038),
                true,
                None,
            ),
            line(
                "the delay | p99",
                MetricUnit::Seconds,
                "<= 150ms",
                0.15,
                Some(0.2),
                Some(-0.05),
                false,
                None,
            ),
            line(
                "a metric that is not there",
                MetricUnit::Seconds,
                "< 1s",
                1.0,
                None,
                None,
                false,
                Some("the run has no 'w.delay.p99'"),
            ),
        ],
    }
}

#[test]
fn the_checks_of_a_run_are_in_all_three_reports_and_a_missed_one_fails_its_case() {
    let Report::Run(mut run) = load("checks-run", true) else {
        panic!("a run")
    };
    let plain_junit = junit(&Report::Run(run.clone()));
    assert!(
        !plain_junit.contains("expect:"),
        "a result without checks has no case for them"
    );
    assert!(!markdown(&Report::Run(run.clone())).contains("Checks of the file"));
    run.result.expect = Some(a_verdict(1));
    let report = Report::Run(run);

    let text = markdown(&report);
    assert!(
        text.contains("### Checks of the file: 1 of 3 hold"),
        "{text}"
    );
    assert!(
        text.contains("| loss stays small | 1.2% | < 5% | holds, 3.8% to spare |"),
        "{text}"
    );
    assert!(
        text.contains("| the delay \\| p99 | 200ms | <= 150ms | **fails**, misses by 50ms |"),
        "{text}"
    );
    assert!(
        text.contains("**fails**: the run has no 'w.delay.p99'"),
        "{text}"
    );

    let xml = junit(&report);
    // One case for each check; only the two that do not hold are failures, of the type of a check and not of a run
    assert_eq!(xml.matches("name=\"expect: ").count(), 3, "{xml}");
    assert_eq!(
        xml.matches("type=\"netscenario.CheckFailed\"").count(),
        2,
        "{xml}"
    );
    assert!(!xml.contains("netscenario.RunFailed"), "{xml}");
    assert!(
        xml.contains("<failure message=\"the delay | p99: 200ms (&lt;= 150ms, misses by 50ms)\""),
        "{xml}"
    );
    assert!(
        xml.contains(
            "name=\"netscenario.lte-like-link.expect.w.loss_ratio.value.bound\" value=\"&lt; 5%\""
        ),
        "{xml}"
    );
    assert!(xml.contains("failures=\"2\""), "{xml}");

    let page = html(&report).unwrap();
    assert_no_external_reference(&page);
    assert!(
        page.contains("<h2>Checks of the file: 1 of 3 hold</h2>"),
        "{page}"
    );
    assert!(
        page.contains("<span class=\"fail\">fails</span>, misses by 50ms"),
        "{page}"
    );
    assert!(page.contains("Not all checks hold."), "{page}");
}

#[test]
fn the_checks_of_a_set_are_in_all_three_reports() {
    let Report::Set(mut set, directory) = a_set() else {
        panic!("a set")
    };
    assert!(!junit(&Report::Set(set.clone(), directory.clone())).contains("expect:"));
    set.expect = Some(a_verdict(6));
    let report = Report::Set(set, directory);
    let text = markdown(&report);
    assert!(
        text.contains("### Checks of the file over 6 runs: 1 of 3 hold"),
        "{text}"
    );
    let xml = junit(&report);
    // The cases of the runs, then those of the checks
    assert_eq!(xml.matches("name=\"expect: ").count(), 3, "{xml}");
    assert_eq!(xml.matches("netscenario.CheckFailed").count(), 2, "{xml}");
    let page = html(&report).unwrap();
    assert_no_external_reference(&page);
    assert!(
        page.contains("Checks of the file over 6 runs: 1 of 3 hold"),
        "{page}"
    );
}
