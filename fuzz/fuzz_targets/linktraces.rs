//! The file of the trace of a link (LMD-03, D-091), in both formats, can be anything from anybody who writes a scenario. It is read
//! or refused with a sentence, never a panic, and a trace that was read has what the format promises: rows that rise, a period after
//! the last row, a form that is written and read back to the same rows, and, as the model of a link, events that make a valid scenario
//! or a refusal for too many of them.
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_model::{
    DEFAULT_WINDOW, DirectionTrace, Duration, LinkModel, MAX_TRACE_ROWS, Scenario, TracePoints,
    read_link_trace_csv, read_link_trace_mahimahi,
};

const SCENARIO: &str = "version: 0\nname: f\nduration: 30s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: { delay: 10ms }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";

fn holds(points: &TracePoints) {
    assert!((1..=MAX_TRACE_ROWS).contains(&points.len()));
    assert!(points.check().is_ok());
    let json = serde_json::to_string(points).expect("points can be written");
    let back: TracePoints = serde_json::from_str(&json).expect("points read back");
    assert_eq!(&back, points);
    let _ = points.shortest_rate_step(true);
    let _ = points.has_stalls();
    // As the trace of a link, in each direction, the events make a valid scenario or are refused
    let mut scenario = Scenario::from_yaml(SCENARIO, "f.yaml").expect("a valid scenario");
    for model in [
        LinkModel::trace(Some(DirectionTrace::from_points(points.clone())), None, None),
        LinkModel::trace(None, None, Some(DirectionTrace::from_points(points.clone()))),
    ] {
        scenario.topology.links[0].model = Some(model);
        if !scenario.validate().is_empty() {
            continue;
        }
        if let Ok(compiled) = scenario.compile(&mut |_| Box::new(|| 0.5)) {
            assert_eq!(compiled.scenario.validate(), Vec::new());
            assert!(compiled.scenario.timeline.windows(2).all(|pair| pair[0].at <= pair[1].at));
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    for period in [None, Some(Duration::from_nanos(60_000_000_000))] {
        if let Ok(points) = read_link_trace_csv(text, period) {
            holds(&points);
        }
    }
    for window in [1_000_000, 20_000_000, DEFAULT_WINDOW.as_nanos(), 1_000_000_000] {
        if let Ok(points) = read_link_trace_mahimahi(text, Duration::from_nanos(window)) {
            holds(&points);
        }
    }
});
