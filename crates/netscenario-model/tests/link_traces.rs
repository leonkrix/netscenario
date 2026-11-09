//! Traces of a link (LMD-03, D-091): the two formats of the file, what the reader refuses, and the events that a trace makes.

// A test file: a failing unwrap is a failing test
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Indices of a short list of pieces
    clippy::cast_possible_truncation,
    // Text for a file is built a piece at a time
    clippy::format_push_string,
    clippy::range_plus_one
)]

use netscenario_model::{
    DEFAULT_WINDOW, Duration, Edit, Event, MAX_TRACE_ROWS, OPPORTUNITY_BYTES, Scenario, Uniform,
    read_link_trace_csv, read_link_trace_mahimahi,
};

fn head(duration: &str, link: &str) -> String {
    format!(
        "version: 0\nname: t\nduration: {duration}\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {{ delay: 10ms }}\n{link}workloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n"
    )
}

/// A scenario whose link has this trace model, with the files by name.
fn load(model: &str, duration: &str, files: &[(&str, &str)]) -> Scenario {
    let yaml = head(duration, &format!("      model:\n{model}"));
    let mut scenario = Scenario::from_yaml(&yaml, "t.yaml").unwrap_or_else(|e| panic!("{e}"));
    let problems = scenario.resolve_files(|name| {
        files
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, content)| content.as_bytes().to_vec())
            .ok_or_else(|| format!("cannot read '{name}': no such file"))
    });
    assert!(
        problems.is_empty(),
        "{:?}",
        problems.iter().map(|p| &p.message).collect::<Vec<_>>()
    );
    scenario
}

fn refused(yaml: &str) -> String {
    match Scenario::from_yaml(yaml, "t.yaml") {
        Ok(_) => panic!("the scenario was accepted:\n{yaml}"),
        Err(error) => error.to_string(),
    }
}

fn no_numbers(_: &netscenario_model::Name) -> Uniform {
    Box::new(|| panic!("a trace draws no number"))
}

fn compile(scenario: &Scenario) -> Vec<Event> {
    scenario.compile(&mut no_numbers).unwrap().scenario.timeline
}

fn at_ms(event: &Event) -> u64 {
    event.at.as_nanos() / 1_000_000
}

fn rate_of(edit: &netscenario_model::ImpairmentsChange) -> u64 {
    match edit.rate {
        Edit::Set(rate) => rate.bits_per_second(),
        other => panic!("not a rate: {other:?}"),
    }
}

const CSV: &str =
    "time,rate,delay,loss\n0ms,5mbit,40ms,0%\n100ms,2mbit,60ms,1%\n250ms,8mbit,30ms,0.5%\n";

/// The rows of a CSV file become one step each, at the moment of the row, and the trace begins again when its period is over.
#[test]
fn a_csv_trace_makes_a_step_for_each_row_and_repeats() {
    let scenario = load(
        "        kind: trace\n        both: { file: cell.csv }\n",
        "1s",
        &[("cell.csv", CSV)],
    );
    let events = compile(&scenario);
    // The last row lasts as long as the one before it (150 ms), so a pass is 400 ms
    let moments: Vec<u64> = events.iter().map(at_ms).collect();
    assert_eq!(moments, [0, 100, 250, 400, 500, 650, 800, 900]);
    let first = events[0].set.as_ref().unwrap().both.clone().unwrap();
    assert_eq!(rate_of(&first), 5_000_000);
    assert_eq!(first.delay, Edit::Set("40ms".parse().unwrap()));
    // A loss of 0 % is no loss, which is what the link has: not a change
    assert_eq!(first.loss, Edit::Keep);
    let second = events[1].set.as_ref().unwrap().both.clone().unwrap();
    assert_eq!(rate_of(&second), 2_000_000);
    assert!(matches!(second.loss, Edit::Set(_)));
    assert_eq!(scenario.validate(), Vec::new());
}

/// A trace whose file has not been read has no rows: compiling it would drop the model and leave the link with a `queue` and
/// no `rate`. It is refused with the file named (the fuzz target of the reader found it on the example `cell-drive`).
#[test]
fn a_trace_that_was_not_read_is_not_compiled() {
    let yaml = head(
        "1s",
        "      model:\n        kind: trace\n        both: { file: cell.csv }\n",
    );
    let mut scenario = Scenario::from_yaml(&yaml, "t.yaml").unwrap();
    // Valid as it is: the file is read when the scenario is loaded
    assert_eq!(scenario.validate(), Vec::new());
    let error = scenario.compile(&mut no_numbers).unwrap_err();
    assert!(error.message.contains("'cell.csv'"), "{}", error.message);
    // Canary: once the file is read the same scenario compiles, so the refusal is about the missing rows
    assert_eq!(
        scenario.resolve_files(|_| Ok(CSV.as_bytes().to_vec())),
        Vec::new()
    );
    assert_ne!(compile(&scenario), Vec::new());
}

/// A trace that does not repeat gives the link back to what the file says when its period is over.
#[test]
fn a_trace_that_does_not_repeat_ends_with_the_link_as_the_file_writes_it() {
    let scenario = load(
        "        kind: trace\n        repeat: false\n        both: { file: cell.csv, period: 400ms }\n",
        "1s",
        &[("cell.csv", CSV)],
    );
    let events = compile(&scenario);
    let moments: Vec<u64> = events.iter().map(at_ms).collect();
    assert_eq!(moments, [0, 100, 250, 400]);
    let last = events[3].set.as_ref().unwrap().both.clone().unwrap();
    assert_eq!(last.rate, Edit::Clear, "the link has no rate in the file");
    assert_eq!(
        last.delay,
        Edit::Set("10ms".parse().unwrap()),
        "the delay of the file"
    );
}

/// `begin` moves the trace; before it the link is what the file says.
#[test]
fn begin_moves_the_trace() {
    let scenario = load(
        "        kind: trace\n        begin: 300ms\n        both: { file: cell.csv, period: 400ms }\n",
        "1s",
        &[("cell.csv", CSV)],
    );
    let moments: Vec<u64> = compile(&scenario).iter().map(at_ms).collect();
    assert_eq!(moments, [300, 400, 550, 700, 800, 950]);
}

/// Two directions have traces of their own, with periods of their own, and a row changes only its direction.
#[test]
fn each_direction_follows_its_own_trace() {
    let scenario = load(
        "        kind: trace\n        forward: { file: up.csv }\n        reverse: { file: down.csv }\n",
        "1s",
        &[
            ("up.csv", "time,rate\n0ms,1mbit\n300ms,2mbit\n"),
            ("down.csv", "time,rate\n0ms,10mbit\n500ms,20mbit\n"),
        ],
    );
    let events = compile(&scenario);
    let mut forward = Vec::new();
    let mut reverse = Vec::new();
    for event in &events {
        let change = event.set.as_ref().unwrap();
        assert!(change.both.is_none());
        if let Some(part) = &change.forward {
            forward.push((at_ms(event), rate_of(part)));
        }
        if let Some(part) = &change.reverse {
            reverse.push((at_ms(event), rate_of(part)));
        }
    }
    // The forward trace is 600 ms long, the reverse one 1000 ms
    assert_eq!(
        forward,
        [
            (0, 1_000_000),
            (300, 2_000_000),
            (600, 1_000_000),
            (900, 2_000_000)
        ]
    );
    assert_eq!(reverse, [(0, 10_000_000), (500, 20_000_000)]);
}

/// A Mahimahi trace of one opportunity in every millisecond is the rate of a frame of 1,514 bytes per millisecond, in every window.
#[test]
fn a_mahimahi_trace_of_a_known_rate_gives_that_rate() {
    let text: String = (1..=1000).fold(String::new(), |mut text, ms| {
        text.push_str(&format!("{ms}\n"));
        text
    });
    let scenario = load(
        "        kind: trace\n        forward: { file: steady.up, format: mahimahi }\n",
        "2s",
        &[("steady.up", &text)],
    );
    let events = compile(&scenario);
    // Twenty windows of 100 ms, all with the same rate: the first is a change from "no rate", the others set what is set already
    // and make no step
    assert_eq!(events.len(), 1, "{events:?}");
    let part = events[0].set.as_ref().unwrap().forward.clone().unwrap();
    assert_eq!(rate_of(&part), OPPORTUNITY_BYTES * 8 * 1000);
    assert_eq!(at_ms(&events[0]), 0);
}

/// The opportunities of a window give its rate: 3 in a window of 100 ms, then none, then 1. A window without an opportunity is a
/// row in which nothing is delivered.
#[test]
fn a_mahimahi_window_counts_the_opportunities_that_end_in_it() {
    let points = read_link_trace_mahimahi("10\n20\n100\n250\n", DEFAULT_WINDOW).unwrap();
    assert_eq!(points.len(), 3);
    assert_eq!(points.period(), Duration::from_nanos(250_000_000));
    assert!(points.has_stalls(), "the second window has none");
    // 3 opportunities (10, 20, 100) in (0, 100], none in (100, 200], 1 in (200, 250] which is half a window
    let text = "10\n20\n100\n250\n";
    let scenario = load(
        "        kind: trace\n        forward: { file: t.mm, format: mahimahi }\n",
        "250ms",
        &[("t.mm", text)],
    );
    let events = compile(&scenario);
    let rates: Vec<u64> = events
        .iter()
        .map(|event| rate_of(&event.set.as_ref().unwrap().forward.clone().unwrap()))
        .collect();
    let frame = OPPORTUNITY_BYTES * 8;
    assert_eq!(rates[0], 3 * frame * 10);
    // The empty window is the rate at which one opportunity is delivered in the window
    assert_eq!(rates[1], frame * 10);
    // One opportunity in 50 ms
    assert_eq!(rates[2], frame * 20);
}

/// A trace with an empty window says so, for the capabilities of a backend.
#[test]
fn an_empty_window_is_a_stall() {
    let points = read_link_trace_mahimahi("10\n250\n", DEFAULT_WINDOW).unwrap();
    assert!(points.has_stalls());
    let full = read_link_trace_mahimahi("50\n150\n250\n", DEFAULT_WINDOW).unwrap();
    assert!(
        !full.has_stalls(),
        "canary: windows with an opportunity each"
    );
}

/// The shortest time between two different rates is what a backend has to follow.
#[test]
fn the_shortest_rate_step_is_the_shortest_row_between_two_rates() {
    let points = read_link_trace_csv(CSV, None).unwrap();
    assert_eq!(
        points.shortest_rate_step(true),
        Some(Duration::from_nanos(100_000_000))
    );
    let same =
        read_link_trace_csv("time,rate\n0ms,1mbit\n10ms,1mbit\n500ms,2mbit\n", None).unwrap();
    assert_eq!(
        same.shortest_rate_step(false),
        Some(Duration::from_nanos(490_000_000)),
        "two rows with the same rate are no step"
    );
    let flat = read_link_trace_csv("time,delay\n0ms,1ms\n10ms,2ms\n", None).unwrap();
    assert_eq!(flat.shortest_rate_step(true), None);
}

/// A trace of many windows is refused with the window that would make it fit, not cut short.
#[test]
fn a_trace_with_too_many_windows_is_refused_with_a_wider_window() {
    let text = format!("{}\n", MAX_TRACE_ROWS as u64 * 10 + 5);
    let error = read_link_trace_mahimahi(&text, Duration::from_nanos(1_000_000)).unwrap_err();
    assert!(error.contains("use a `window` of at least 11ms"), "{error}");
    // Canary: the window the message asks for makes it fit
    assert!(read_link_trace_mahimahi(&text, Duration::from_nanos(11_000_000)).is_ok());
}

#[test]
fn the_files_that_are_not_traces_are_refused_with_the_line() {
    for (text, wanted) in [
        ("time,speed\n0ms,1mbit\n", "'speed' is not a column"),
        ("time\n0ms\n", "names only 'time'"),
        ("rate\n1mbit\n", "no 'time' column"),
        (
            "time,rate\n0ms,1mbit\n0ms,2mbit\n",
            "row 2 does not begin after row 1",
        ),
        ("time,rate\n0ms,1mbit,3\n", "line 2: 3 values"),
        ("time,rate\n0ms,fast\n", "line 2: the rate"),
        ("time,rate,rate\n0ms,1mbit,2mbit\n", "written twice"),
        ("time,loss\n0ms,120%\n", "line 2: the loss"),
        ("time,rate\n", "no row"),
        ("0ms,1mbit\n", "'0ms' is not a column"),
    ] {
        let error = read_link_trace_csv(text, None).unwrap_err();
        assert!(error.contains(wanted), "{text:?}: {error}");
    }
    for (text, wanted) in [
        ("12\n11\n", "line 2: 11 comes after 12"),
        ("1.5\n", "line 1: '1.5' is not a time in whole milliseconds"),
        ("", "no line"),
        ("0\n", "no length"),
    ] {
        let error = read_link_trace_mahimahi(text, DEFAULT_WINDOW).unwrap_err();
        assert!(error.contains(wanted), "{text:?}: {error}");
    }
    assert!(read_link_trace_mahimahi("5\n", Duration::from_nanos(1_500_000)).is_err());
}

#[test]
fn the_reader_refuses_a_trace_that_is_not_written_right() {
    let case = |model: &str| refused(&head("1s", &format!("      model:\n{model}")));
    let error = case("        kind: trace\n");
    assert!(error.contains("needs a file for a direction"), "{error}");
    let error = case(
        "        kind: trace\n        both: { file: a.csv }\n        forward: { file: b.csv }\n",
    );
    assert!(error.contains("cannot be combined"), "{error}");
    let error = case("        kind: trace\n        both: { file: link.up }\n");
    assert!(error.contains("cannot tell the format"), "{error}");
    let error = case("        kind: trace\n        both: { file: a.csv, window: 10ms }\n");
    assert!(
        error.contains("`window` belongs to a mahimahi trace"),
        "{error}"
    );
    let error =
        case("        kind: trace\n        both: { file: a, format: mahimahi, period: 1s }\n");
    assert!(error.contains("`period` belongs to a csv trace"), "{error}");
    let error = case(
        "        kind: trace\n        states: [ { for: 1s } ]\n        both: { file: a.csv }\n",
    );
    assert!(error.contains("`states` belong to a periodic"), "{error}");
    let error = case(
        "        kind: periodic\n        both: { file: a.csv }\n        states: [ { for: 1s } ]\n",
    );
    assert!(error.contains("`both` belongs to a trace model"), "{error}");
    let error =
        case("        kind: markov\n        repeat: false\n        start: a\n        states: []\n");
    assert!(error.contains("`repeat` belongs"), "{error}");
}

/// The file that was pinned must have exactly the pinned content; a trace that is not found says where.
#[test]
fn a_pinned_hash_and_a_missing_file_are_refused() {
    let yaml = head(
        "1s",
        "      model:\n        kind: trace\n        both: { file: cell.csv, sha256: 0000000000000000000000000000000000000000000000000000000000000000 }\n",
    );
    let mut scenario = Scenario::from_yaml(&yaml, "t.yaml").unwrap();
    let problems = scenario.resolve_files(|_| Ok(CSV.as_bytes().to_vec()));
    assert!(
        problems[0]
            .message
            .contains("is not the file the scenario was written for"),
        "{problems:?}"
    );
    let problems =
        scenario.resolve_files(|name| Err(format!("cannot read '{name}': no such file")));
    assert!(problems[0].message.contains("no such file"));
    assert!(scenario.unresolved_files().contains(&"cell.csv".to_owned()));
}

/// What the loader makes of the file is in the scenario with the hash, so a recorded scenario needs no file.
#[test]
fn the_points_and_the_hash_are_in_the_scenario_and_survive_a_round_trip() {
    let scenario = load(
        "        kind: trace\n        both: { file: cell.csv }\n",
        "1s",
        &[("cell.csv", CSV)],
    );
    assert_eq!(scenario.unresolved_files(), Vec::<String>::new());
    let json = serde_json::to_string(&scenario).unwrap();
    let back: Scenario = serde_json::from_str(&json).unwrap();
    assert_eq!(back, scenario);
    assert!(json.contains(&netscenario_model::sha256_hex(CSV.as_bytes())));
    assert_eq!(back.unresolved_files(), Vec::<String>::new());
}

/// A trace that would make more events than a link may have is refused, not cut short.
#[test]
fn a_trace_that_makes_too_many_events_is_refused() {
    let mut text = String::from("time,rate\n");
    for row in 0..3000 {
        text.push_str(&format!("{}ms,{}kbit\n", row * 2, 1000 + row % 7));
    }
    // 3000 rows of 2 ms, repeating for 60 s: 30 passes
    let yaml = head(
        "60s",
        "      model:\n        kind: trace\n        both: { file: fast.csv }\n",
    );
    let problems = read_scenario(&yaml).resolve_files(|_| Ok(text.as_bytes().to_vec()));
    assert!(
        problems[0].message.contains("makes more than 10000 events"),
        "{problems:?}"
    );
    // Canary: a measured phase of 6 s needs 3,000 events only
    let short = load(
        "        kind: trace\n        both: { file: fast.csv }\n",
        "6s",
        &[("fast.csv", &text)],
    );
    assert!(short.compile(&mut no_numbers).is_ok());
}

/// A key a trace writes cannot be set by the timeline of the file on the same link: the reader knows it when the file is read.
#[test]
fn the_timeline_may_not_touch_the_keys_of_a_trace() {
    let yaml = format!(
        "{}timeline:\n  - {{ at: 1s, link: l, set: {{ both: {{ rate: 3mbit }} }} }}\n",
        head(
            "5s",
            "      model:\n        kind: trace\n        both: { file: cell.csv }\n"
        )
    );
    let mut scenario = read_scenario(&yaml);
    let problems = scenario.resolve_files(|_| Ok(CSV.as_bytes().to_vec()));
    assert!(
        problems
            .iter()
            .any(|p| p.message.contains("would take this change back")),
        "{problems:?}"
    );
    // Canary: the same timeline on a key that the trace does not write
    let mut scenario = read_scenario(&yaml.replace("rate: 3mbit", "duplicate: 1%"));
    let problems =
        scenario.resolve_files(|_| Ok("time,rate\n0ms,1mbit\n10ms,2mbit\n".as_bytes().to_vec()));
    assert_eq!(problems, Vec::new());
}

/// Where the rate changes, and how fast: the timeline, a periodic model, a trace.
#[test]
fn the_rate_changes_of_a_scenario_are_found_with_the_shortest_step() {
    let scenario = load(
        "        kind: trace\n        both: { file: cell.csv }\n",
        "1s",
        &[("cell.csv", CSV)],
    );
    let changes = scenario.rate_changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "topology.links[0].model");
    assert_eq!(changes[0].shortest, Some(Duration::from_nanos(100_000_000)));
    let yaml = format!(
        "{}timeline:\n  - {{ at: 1s, link: l, set: {{ both: {{ rate: 3mbit }} }} }}\n  - {{ at: 1010ms, link: l, set: {{ both: {{ rate: 1mbit }} }} }}\n",
        head("5s", "")
    );
    let changes = read_scenario(&yaml).rate_changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "timeline[0]");
    assert_eq!(changes[0].shortest, Some(Duration::from_nanos(10_000_000)));
    // Canary: a timeline that changes only the loss changes no rate
    let loss = format!(
        "{}timeline:\n  - {{ at: 1s, link: l, set: {{ both: {{ loss: {{ model: random, p: 1% }} }} }} }}\n",
        head("5s", "")
    );
    assert_eq!(read_scenario(&loss).rate_changes(), []);
    // A rate set once is a change without a step
    let once = format!(
        "{}timeline:\n  - {{ at: 1s, link: l, set: {{ both: {{ rate: 3mbit }} }} }}\n",
        head("5s", "")
    );
    assert_eq!(read_scenario(&once).rate_changes()[0].shortest, None);
}

fn read_scenario(yaml: &str) -> Scenario {
    Scenario::from_yaml(yaml, "t.yaml").unwrap_or_else(|e| panic!("{e}"))
}

/// A `queue` waits for a rate, which a trace may bring; one that does not write a rate leaves the queue without, and the reader
/// says so once the file is read. The same for a step of the timeline.
#[test]
fn a_queue_may_wait_for_a_rate_that_a_trace_or_a_step_brings() {
    let link = "      both: { queue: { limit: 100p } }\n      model:\n        kind: trace\n        both: { file: cell.csv }\n";
    let mut with_rate = read_scenario(&no_base(&head("5s", link)));
    let problems = with_rate.resolve_files(|_| Ok(CSV.as_bytes().to_vec()));
    assert_eq!(problems, Vec::new());
    // Canary: a trace of delays brings no rate
    let mut without = read_scenario(&no_base(&head("5s", link)));
    let problems = without.resolve_files(|_| Ok(b"time,delay\n0ms,10ms\n100ms,20ms\n".to_vec()));
    assert!(
        problems[0].message.contains("this direction has no `rate`"),
        "{problems:?}"
    );
    let step = format!(
        "{}timeline:\n  - {{ at: 1s, link: l, set: {{ both: {{ rate: 3mbit }} }} }}\n",
        no_base(&head("5s", "      both: { queue: { limit: 100p } }\n"))
    );
    assert_eq!(read_scenario(&step).validate(), Vec::new());
    // Canary: a queue with no rate anywhere is refused as before
    let none = no_base(&head("5s", "      both: { queue: { limit: 100p } }\n"));
    assert!(refused(&none).contains("this direction has no `rate`"));
}

/// The scenario of `head` without the delay it gives the link.
fn no_base(yaml: &str) -> String {
    yaml.replace("      both: { delay: 10ms }\n", "")
}

/// `SplitMix64`: any good generator will do for these tests.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The readers of both formats are given garbage and near-misses made from the pieces of the format: they read or refuse with a
/// sentence and never panic (the oracle of the fuzz target `linktraces`), and what they read holds what the format promises and
/// makes a valid scenario or a refusal for too many events.
#[test]
fn the_readers_never_panic_and_what_they_read_makes_a_valid_scenario() {
    let pieces = [
        "time",
        "rate",
        "delay",
        "loss",
        ",",
        "\n",
        "\n",
        "0",
        "5",
        "100",
        "0ms",
        "10ms",
        "1s",
        "2.5s",
        "5mbit",
        "0mbit",
        "1kbit",
        "100%",
        "0.5%",
        "-1",
        "x",
        " ",
        "#",
        "1.5",
        "99999999999999999999",
        "mbit",
        "ms",
    ];
    let mut state = 7_u64;
    let mut read = 0;
    for round in 0..1000 {
        let mut text = String::new();
        if round % 2 == 0 {
            text.push_str("time,rate,delay,loss\n");
        }
        for _ in 0..(next(&mut state) % 60) {
            text.push_str(pieces[(next(&mut state) % pieces.len() as u64) as usize]);
        }
        for period in [None, Some(Duration::from_nanos(60_000_000_000))] {
            if let Ok(points) = read_link_trace_csv(&text, period) {
                holds(&points);
                read += 1;
            }
        }
        if let Ok(points) = read_link_trace_mahimahi(&text, DEFAULT_WINDOW) {
            holds(&points);
            read += 1;
        }
    }
    // Files that are made by the grammar of the formats, with the faults the pieces above make now and then
    for round in 0..250 {
        let mut text = String::new();
        let columns: Vec<&str> = ["rate", "delay", "loss"]
            .into_iter()
            .filter(|_| !next(&mut state).is_multiple_of(3))
            .collect();
        let columns = if columns.is_empty() {
            vec!["rate"]
        } else {
            columns
        };
        text.push_str(&format!("time,{}\n", columns.join(",")));
        let mut at = 0_u64;
        for _ in 0..1 + next(&mut state) % 40 {
            let values: Vec<String> = columns
                .iter()
                .map(|column| match *column {
                    "rate" => format!("{}kbit", next(&mut state) % 3 * 1000),
                    "delay" => format!("{}ms", next(&mut state) % 300),
                    _ => format!("{}%", next(&mut state) % 101),
                })
                .collect();
            text.push_str(&format!("{at}ms,{}\n", values.join(",")));
            at += 1 + next(&mut state) % 2000;
        }
        if round % 7 == 0 {
            text.push_str(pieces[(next(&mut state) % pieces.len() as u64) as usize]);
        }
        if let Ok(points) = read_link_trace_csv(&text, None) {
            holds(&points);
            read += 1;
        }
        let mut mahimahi = String::new();
        let mut ms = 0_u64;
        for _ in 0..1 + next(&mut state) % 300 {
            ms += next(&mut state) % 40;
            mahimahi.push_str(&format!("{ms}\n"));
        }
        for window in [1, 20, 100, 1000] {
            if let Ok(points) =
                read_link_trace_mahimahi(&mahimahi, Duration::from_nanos(window * 1_000_000))
            {
                holds(&points);
                read += 1;
            }
        }
    }
    // Canary: the generators make input that is read, so the checks above are made
    assert!(read > 300, "only {read} inputs were read");
}

fn holds(points: &netscenario_model::TracePoints) {
    assert!((1..=MAX_TRACE_ROWS).contains(&points.len()));
    assert!(points.check().is_ok());
    let json = serde_json::to_string(points).unwrap();
    let back: netscenario_model::TracePoints = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, points);
    let mut scenario = read_scenario(&head("30s", ""));
    for model in [
        netscenario_model::LinkModel::trace(
            Some(netscenario_model::DirectionTrace::from_points(
                points.clone(),
            )),
            None,
            None,
        ),
        netscenario_model::LinkModel::trace(
            None,
            None,
            Some(netscenario_model::DirectionTrace::from_points(
                points.clone(),
            )),
        ),
    ] {
        scenario.topology.links[0].model = Some(model);
        if !scenario.validate().is_empty() {
            continue;
        }
        if let Ok(compiled) = scenario.compile(&mut no_numbers) {
            assert_eq!(compiled.scenario.validate(), Vec::new());
            assert!(
                compiled
                    .scenario
                    .timeline
                    .windows(2)
                    .all(|pair| pair[0].at <= pair[1].at)
            );
        }
    }
}
