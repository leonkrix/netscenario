//! The edges of the rules of `validate.rs`: for each limit the value on the edge is accepted and the next one is refused with the
//! reason. A rule that is tested only with a value far outside its limit passes when its comparison is off by one, which the mutation
//! test of the reader showed for 56 of them. A refusal is looked for by its words, so that another fault of the generated scenario
//! cannot stand for it.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;

use netscenario_model::{
    MAX_BINS, MAX_BURST_PACKETS, MAX_LINKS, MAX_NODES, MAX_PACKETS_PER_SECOND, MAX_QUEUE_BYTES,
    MAX_QUEUE_PACKETS, MAX_ROUTES, Scenario,
};

/// What the reader says about a scenario: empty if it accepts it.
fn said(yaml: &str) -> String {
    Scenario::from_yaml(yaml, "t.yaml")
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

/// A scenario of two nodes and a link; `link` are more lines of the link (6 spaces deep), `workloads` the lines of the workloads
/// (2 spaces deep, each one a line that starts with `- `), `rest` more top-level keys, and `duration` how long it runs.
fn scenario(duration: &str, link: &str, workloads: &str, rest: &str) -> String {
    format!(
        "version: 0\nname: t\nduration: {duration}\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{link}workloads:\n{workloads}{rest}"
    )
}

const STREAM: &str =
    "  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1000b }\n";

/// A refusal that has to be there at one value and has to be absent at the value just before.
fn edge(words: &str, accepted: &str, refused: &str) {
    let before = said(accepted);
    assert!(
        !before.contains(words),
        "the value on the edge was refused with '{words}':\n{before}\n{accepted}"
    );
    let after = said(refused);
    assert!(
        after.contains(words),
        "the value after the edge was not refused with '{words}':\n{after}\n{refused}"
    );
}

#[test]
fn the_number_of_nodes_has_an_edge() {
    let nodes = |count: usize| {
        let names: Vec<String> = (0..count).map(|i| format!("n{i}")).collect();
        format!(
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [{}]\n  links:\n    - {{ id: l, between: [n0, n1] }}\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: n0, to: n1, rate: 1mbit, packet-size: 1000b }}\n",
            names.join(", ")
        )
    };
    edge(
        &format!("at most {MAX_NODES} nodes"),
        &nodes(MAX_NODES),
        &nodes(MAX_NODES + 1),
    );
}

#[test]
fn the_number_of_links_has_an_edge() {
    let links = |count: usize| {
        let list: Vec<String> = (0..count)
            .map(|i| format!("    - {{ id: l{i}, between: [a, b] }}\n"))
            .collect();
        format!(
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n{}workloads:\n{STREAM}",
            list.concat()
        )
    };
    edge(
        &format!("at most {MAX_LINKS} links"),
        &links(MAX_LINKS),
        &links(MAX_LINKS + 1),
    );
}

/// It takes about 12 seconds (two scenarios of more than 8,000 routes, which the reader checks one against the other): the edge of
/// the limit cannot be shown with fewer routes, and a limit that is off by one is what the mutation test found here.
#[test]
fn the_number_of_routes_has_an_edge() {
    let routes = |count: usize| {
        let list: Vec<String> = (0..count)
            .map(|_| "    - { node: a, to: b, via: l }\n".to_owned())
            .collect();
        scenario("10s", "", STREAM, "").replace(
            "workloads:",
            &format!("  routes:\n{}workloads:", list.concat()),
        )
    };
    edge(
        &format!("at most {MAX_ROUTES} routes"),
        &routes(MAX_ROUTES),
        &routes(MAX_ROUTES + 1),
    );
}

#[test]
fn a_request_every_millisecond_is_the_fastest_there_is() {
    let requests = |interval: &str| {
        scenario(
            "10s",
            "",
            &format!(
                "  - {{ id: api, kind: request-response, from: a, to: b, request: 100b, response: 1kb, interval: {interval} }}\n"
            ),
            "",
        )
    };
    edge(
        "between requests is below",
        &requests("1ms"),
        &requests("999us"),
    );
}

#[test]
fn a_hash_is_64_hexadecimal_digits_and_nothing_else() {
    let hash = |text: &str| {
        scenario(
            "10s",
            "",
            &format!(
                "  - {{ id: call, kind: trace-replay, from: a, to: b, trace: {{ file: call.csv, sha256: \"{text}\" }} }}\n"
            ),
            "",
        )
    };
    let good = "ab".repeat(32);
    let words = "`sha256` is 64 hexadecimal digits";
    edge(words, &hash(&good), &hash(&good[..63]));
    // 64 characters that are not all digits of hexadecimal, and few characters that are all
    assert!(said(&hash(&"g".repeat(64))).contains(words));
    assert!(said(&hash("abc")).contains(words));
    assert!(said(&hash(&format!("{good}0"))).contains(words));
    // The same for the hash of the table of a jitter
    let table = |text: &str| {
        scenario(
            "10s",
            &format!(
                "      both:\n        delay: 40ms\n        jitter: {{ amount: 9ms, distribution: table, table: {{ file: samples.txt, sha256: \"{text}\" }} }}\n"
            ),
            STREAM,
            "",
        )
    };
    edge(words, &table(&good), &table(&good[..63]));
    assert!(said(&table(&"g".repeat(64))).contains(words));
    assert!(said(&table("abc")).contains(words));
}

#[test]
fn the_packets_per_second_of_a_pattern_have_an_edge() {
    // 100,000 packets of 1000 bytes a second are 800 mbit/s
    let at = |rate: &str, kind: &str| {
        scenario(
            "10s",
            "",
            &format!(
                "  - {{ id: w, kind: {kind}, from: a, to: b, rate: {rate}, packet-size: 1000b }}\n"
            ),
            "",
        )
    };
    assert_eq!(MAX_PACKETS_PER_SECOND * 1000 * 8, 800_000_000);
    for kind in ["udp-cbr", "udp-poisson"] {
        edge(
            "packets per second",
            &at("800000000bit", kind),
            &at("800000001bit", kind),
        );
    }
    // A burst: ten microseconds between two packets is 100,000 a second
    let burst = |packets: u32, gap: &str| {
        scenario(
            "10s",
            "",
            &format!(
                "  - {{ id: w, kind: udp-bursty, from: a, to: b, packet-size: 1000b, burst-packets: {packets}, gap: {gap}, pause: 100ms }}\n"
            ),
            "",
        )
    };
    edge(
        "between the packets of a burst",
        &burst(10, "10us"),
        &burst(10, "9us"),
    );
    // One packet has no gap to another
    assert!(!said(&burst(1, "1us")).contains("between the packets of a burst"));
    edge(
        "outside 1 to",
        &burst(MAX_BURST_PACKETS, "10us"),
        &burst(MAX_BURST_PACKETS + 1, "10us"),
    );
}

#[test]
fn a_run_may_have_a_million_bins() {
    let bins =
        |duration: &str| scenario(duration, "", STREAM, "measure:\n  series: { bin: 1ms }\n");
    edge(
        &format!("more than {MAX_BINS} bins"),
        &bins("1000s"),
        &bins("1000001ms"),
    );
}

#[test]
fn the_limits_of_a_queue_have_edges() {
    let queue = |keys: &str| {
        scenario(
            "10s",
            &format!("      both: {{ rate: 10mbit, queue: {{ {keys} }} }}\n"),
            STREAM,
            "",
        )
    };
    edge(
        "outside 1 byte to 1gib",
        &queue(&format!("discipline: fifo, limit: {MAX_QUEUE_BYTES}b")),
        &queue(&format!(
            "discipline: fifo, limit: {}b",
            MAX_QUEUE_BYTES + 1
        )),
    );
    assert!(!said(&queue("discipline: fifo, limit: 1b")).contains("outside 1 byte"));
    edge(
        "more than 1000000 packets",
        &queue(&format!("discipline: codel, limit: {MAX_QUEUE_PACKETS}p")),
        &queue(&format!(
            "discipline: codel, limit: {}p",
            MAX_QUEUE_PACKETS + 1
        )),
    );
    edge(
        "is an amount of data",
        &queue("discipline: fifo, limit: 100kb"),
        &queue("discipline: codel, limit: 100kb"),
    );
    // The times of a discipline: more than zero, at most ten seconds
    for key in ["target", "interval"] {
        edge(
            "is outside 1 microsecond to 10 seconds",
            &queue(&format!("discipline: codel, {key}: 10s")),
            &queue(&format!("discipline: codel, {key}: 10001ms")),
        );
        assert!(
            said(&queue(&format!("discipline: codel, {key}: 1us"))).is_empty()
                || !said(&queue(&format!("discipline: codel, {key}: 1us")))
                    .contains("outside 1 microsecond")
        );
    }
    edge(
        "is outside 1 microsecond to 10 seconds",
        &queue("discipline: pie, t-update: 10s"),
        &queue("discipline: pie, t-update: 10001ms"),
    );
    // `flows` and `quantum` of fq-codel
    edge(
        "outside 1 to 65536",
        &queue("discipline: fq-codel, flows: 65536"),
        &queue("discipline: fq-codel, flows: 65537"),
    );
    assert!(said(&queue("discipline: fq-codel, flows: 0")).contains("outside 1 to 65536"));
    edge(
        "outside 1 byte to 1mib",
        &queue("discipline: fq-codel, quantum: 1mib"),
        &queue("discipline: fq-codel, quantum: 1048577b"),
    );
    // A key of another discipline names the discipline it belongs to
    let foreign = said(&queue("discipline: fifo, t-update: 10ms"));
    assert!(foreign.contains("belongs to `pie`"), "{foreign}");
    let foreign = said(&queue("discipline: codel, flows: 4"));
    assert!(
        foreign.contains("`flows` and `quantum` belong to `fq-codel`"),
        "{foreign}"
    );
    let foreign = said(&queue("discipline: fifo, interval: 10ms"));
    assert!(
        foreign.contains("`interval` belongs to `codel` and `fq-codel`"),
        "{foreign}"
    );
}

#[test]
fn the_chances_out_of_one_state_add_up_to_at_most_a_hundred_percent() {
    let loss = |p13: &str, p14: &str, p31: &str, p32: &str| {
        scenario(
            "10s",
            &format!(
                "      both: {{ delay: 10ms, loss: {{ model: markov4, p13: {p13}, p31: {p31}, p32: {p32}, p23: 20%, p14: {p14} }} }}\n"
            ),
            STREAM,
            "",
        )
    };
    edge(
        "add up to more than 100%",
        &loss("60%", "40%", "10%", "10%"),
        &loss("60%", "40.000001%", "10%", "10%"),
    );
    edge(
        "add up to more than 100%",
        &loss("2%", "1%", "50%", "50%"),
        &loss("2%", "1%", "50%", "50.000001%"),
    );
}

#[test]
fn a_jitter_may_reach_the_delay_and_not_go_below_it() {
    let jitter = |amount: &str, distribution: &str, delay: &str| {
        scenario(
            "10s",
            &format!(
                "      both:\n        delay: {delay}\n        jitter: {{ amount: {amount}, distribution: {distribution} }}\n"
            ),
            STREAM,
            "",
        )
    };
    // Uniform: up to the delay
    edge(
        "some delays would be below zero",
        &jitter("10ms", "uniform", "10ms"),
        &jitter("10001us", "uniform", "10ms"),
    );
    // Normal: the delay is at least four times the jitter
    edge(
        "some delays would be below zero",
        &jitter("10ms", "normal", "40ms"),
        &jitter("10001us", "normal", "40ms"),
    );
}

// ---- outages ----

#[test]
fn two_outages_overlap_only_if_their_directions_and_their_times_do() {
    let outages = |first: &str, second: &str| {
        scenario(
            "10s",
            "",
            STREAM,
            &format!(
                "timeline:\n  - {{ at: 1s, link: l, outage: 2s{first} }}\n  - {{ at: {second} }}\n"
            ),
        )
    };
    let overlap = "overlaps the outage of timeline[0]";
    // Directions: the same direction, or `both` with any, overlap; forward and reverse do not
    let direction = |first: &str, second: &str| {
        said(&outages(
            &format!(", direction: {first}"),
            &format!("2s, link: l, outage: 2s, direction: {second}"),
        ))
        .contains(overlap)
    };
    assert!(direction("forward", "forward"));
    assert!(direction("reverse", "reverse"));
    assert!(direction("both", "forward"));
    assert!(direction("both", "reverse"));
    assert!(direction("forward", "both"));
    assert!(direction("reverse", "both"));
    assert!(direction("both", "both"));
    assert!(!direction("forward", "reverse"));
    assert!(!direction("reverse", "forward"));
    // No direction written is both
    assert!(said(&outages("", "2s, link: l, outage: 2s, direction: forward")).contains(overlap));
    // Times: an outage that begins when the other ends does not overlap it, one that begins a nanosecond before does
    let times = |second: &str| {
        said(&outages("", &format!("{second}, link: l, outage: 1s"))).contains(overlap)
    };
    assert!(!times("3s"), "an outage may begin when the other ends");
    assert!(times("2999999999ns"));
    assert!(times("1s"));
    // The outage that begins before the other and ends when it begins
    let before = said(&scenario(
        "10s",
        "",
        STREAM,
        "timeline:\n  - { at: 1s, link: l, outage: 2s }\n  - { at: 0s, link: l, outage: 1s }\n",
    ));
    assert!(!before.contains(overlap), "{before}");
    let before = said(&scenario(
        "10s",
        "",
        STREAM,
        "timeline:\n  - { at: 1s, link: l, outage: 2s }\n  - { at: 0s, link: l, outage: 1000000001ns }\n",
    ));
    assert!(
        before.contains("overlaps the outage of timeline["),
        "{before}"
    );
}

#[test]
fn an_outage_may_end_with_the_run_and_not_after_it() {
    let outage = |length: &str| {
        scenario(
            "10s",
            "",
            STREAM,
            &format!("timeline:\n  - {{ at: 8s, link: l, outage: {length} }}\n"),
        )
    };
    edge(
        "after the end of the measured phase",
        &outage("2s"),
        &outage("2000000001ns"),
    );
    assert!(said(&outage("0s")).contains("must last longer than zero"));
}

// ---- a queue waits for a rate that a step of the timeline may bring ----

/// A queue of a direction needs a rate to wait for, and a step of the timeline may set that rate: for `forward` a step on `forward`
/// or on `both`, for `reverse` the same, and for the queue written on `both` a step on `both` or the two directions in one step.
#[test]
fn a_queue_without_a_rate_waits_for_the_step_that_brings_one() {
    let needs = "holds the packets that wait for the rate";
    let case = |side: &str, steps: &[&str]| {
        let timeline: String = steps.iter().fold(String::new(), |mut text, step| {
            let _ = writeln!(text, "  - {{ at: 1s, link: l, set: {{ {step} }} }}");
            text
        });
        let rest = if steps.is_empty() {
            String::new()
        } else {
            format!("timeline:\n{timeline}")
        };
        said(&scenario(
            "10s",
            &format!("      {side}: {{ queue: {{ limit: 10p }} }}\n"),
            STREAM,
            &rest,
        ))
        .contains(needs)
    };
    let rate = "rate: 1mbit";
    for side in ["forward", "reverse"] {
        let other = if side == "forward" {
            "reverse"
        } else {
            "forward"
        };
        assert!(case(side, &[]), "{side}: no rate");
        assert!(
            !case(side, &[&format!("{side}: {{ {rate} }}")]),
            "{side}: its own step"
        );
        assert!(
            !case(side, &[&format!("both: {{ {rate} }}")]),
            "{side}: both"
        );
        assert!(
            case(side, &[&format!("{other}: {{ {rate} }}")]),
            "{side}: the other direction"
        );
    }
    // A queue on both directions needs a rate in both
    assert!(case("both", &[]));
    assert!(!case("both", &[&format!("both: {{ {rate} }}")]));
    assert!(case("both", &[&format!("forward: {{ {rate} }}")]));
    assert!(case("both", &[&format!("reverse: {{ {rate} }}")]));
    assert!(!case(
        "both",
        &[&format!("forward: {{ {rate} }}, reverse: {{ {rate} }}")]
    ));
    // Two steps that each set one direction are not one step that sets both: the rule looks at one step at a time
    assert!(case(
        "both",
        &[
            &format!("forward: {{ {rate} }}"),
            &format!("reverse: {{ {rate} }}")
        ]
    ));
}

// ---- paths, packets and the MTU ----

/// Two islands of two nodes: a workload from one to the other has no route, whatever it sends.
#[test]
fn a_workload_to_a_node_that_cannot_be_reached_is_refused_with_the_missing_route() {
    let to_c = |workload: &str| {
        format!(
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b, c, d]\n  links:\n    - {{ id: l, between: [a, b] }}\n    - {{ id: m, between: [c, d] }}\nworkloads:\n  - {workload}\n"
        )
    };
    for workload in [
        "{ id: w, kind: tcp-bulk, from: a, to: c, bytes: 1mb }",
        "{ id: w, kind: request-response, from: a, to: c, request: 100b, response: 1kb }",
        "{ id: w, kind: udp-cbr, from: a, to: c, rate: 1mbit, packet-size: 1000b }",
    ] {
        let text = said(&to_c(workload));
        assert!(
            text.contains("'a' has no route to 'c'"),
            "{workload}: {text}"
        );
    }
    // To a node it reaches, it has one
    let reached = to_c("{ id: w, kind: tcp-bulk, from: a, to: b, bytes: 1mb }");
    assert!(
        !said(&reached).contains("has no route"),
        "{}",
        said(&reached)
    );
}

/// A packet that the first link cuts into fragments must be carried whole by the later links: a router that would cut it again is
/// refused. A sender that must not cut (`dont-fragment`) refuses a packet that does not fit the first link, so nothing reaches a
/// router; but a packet that fits the first link exactly and not the second is cut by the router, `dont-fragment` or not.
#[test]
fn a_packet_that_a_router_would_cut_is_refused_unless_the_sender_refuses_it_first() {
    let cut = "would be cut by a router";
    let packet = |size: u32, dont_fragment: bool| {
        format!(
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [client, {{ id: edge, role: router }}, server]\n  links:\n    - {{ id: access, between: [client, edge], mtu: 1000 }}\n    - {{ id: core, between: [edge, server], mtu: 600 }}\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: {size}b, dont-fragment: {dont_fragment} }}\n"
        )
    };
    // 1400 bytes do not fit the first link (1000): the sender cuts them to 1000-byte frames, which the second link (600) cannot carry
    assert!(said(&packet(1400, false)).contains(cut));
    // ... unless the sender must not cut: it refuses the packet, and no router sees it
    assert!(!said(&packet(1400, true)).contains(cut));
    // 972 bytes of payload are exactly 1000 bytes of packet: they fit the first link and not the second
    assert!(
        said(&packet(972, true)).contains(cut),
        "a packet that fits the first link exactly"
    );
    assert!(said(&packet(972, false)).contains(cut));
    // One byte more does not fit the first link any more
    assert!(!said(&packet(973, true)).contains(cut));
    // A packet that fits both is no problem
    assert!(
        said(&packet(500, true)).is_empty(),
        "{}",
        said(&packet(500, true))
    );
}

/// The smallest payload is the header of the agent, and with `udp-checksum: false` it is four bytes more, for the CRC-32.
#[test]
fn the_smallest_packet_of_a_stream_depends_on_the_checksum() {
    let small = |size: u32, checksum: bool| {
        scenario(
            "10s",
            "",
            &format!(
                "  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: {size}b, udp-checksum: {checksum} }}\n"
            ),
            "",
        )
    };
    edge("is outside 16 to", &small(16, true), &small(15, true));
    edge(
        "is too small for `udp-checksum: false`",
        &small(20, false),
        &small(19, false),
    );
    // With the checksum on 19 bytes are fine
    assert_eq!(said(&small(19, true)), "");
}

/// The same for the packets of a trace in the scenario (the form that a file makes when it is read, and that a replay reads): the
/// smallest of them is judged like a size. A CSV file with a packet below the smallest is refused when it is read; here the packets
/// are in the scenario already, and the stream is told not to use the checksum afterwards.
#[test]
fn the_smallest_packet_of_a_trace_in_the_scenario_is_judged_like_a_size() {
    let canonical = |smallest: u32| {
        let text = "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - { id: call, kind: trace-replay, from: a, to: b, trace: { file: call.csv } }\n";
        let mut parsed = Scenario::from_yaml(text, "t.yaml").unwrap();
        let file = format!("time,size\n0ms,{smallest}b\n20ms,200b\n40ms,300b\n").into_bytes();
        let found = parsed.resolve_files(|_| Ok(file.clone()));
        assert!(found.is_empty(), "{found:?}");
        parsed.to_yaml().unwrap()
    };
    let without_checksum =
        |yaml: String| yaml.replacen("  to: b\n", "  to: b\n  udp-checksum: false\n", 1);
    let below = "below the smallest payload";
    // With the checksum on the smallest payload is 16 bytes, and 16 is allowed
    assert_eq!(said(&canonical(16)), "");
    assert_eq!(said(&canonical(17)), "");
    // Without it 20, and 20 is allowed; 19 is not, and 16 is not
    assert_eq!(said(&without_checksum(canonical(20))), "");
    let message = said(&without_checksum(canonical(19)));
    assert!(
        message.contains(
            "a packet of 19 bytes, below the smallest payload of 20 bytes for `udp-checksum: false`"
        ),
        "{message}"
    );
    assert!(said(&without_checksum(canonical(16))).contains(below));
}

// ---- what a step of the timeline makes of a link ----

/// A step is judged on the state it makes, for each direction: a step on `reverse` that makes the reverse direction wrong is a fault of
/// the step even if the forward direction is fine, and a fault the link had before the step is not blamed on the step.
#[test]
fn a_step_is_blamed_for_the_direction_it_breaks_and_not_for_a_fault_that_was_there() {
    let after = "after this step";
    let with_step = |link: &str, step: &str| {
        said(&scenario(
            "10s",
            link,
            STREAM,
            &format!("timeline:\n  - {{ at: 1s, link: l, set: {{ {step} }} }}\n"),
        ))
    };
    let link = "      both: { delay: 10ms }\n";
    // The reverse direction gets a jitter that is longer than its delay; the forward direction is untouched and fine
    let jitter = "jitter: { amount: 20ms, distribution: uniform }";
    let message = with_step(link, &format!("reverse: {{ {jitter} }}"));
    assert!(
        message.contains("longer than the delay") && message.contains(after),
        "{message}"
    );
    // The same on the forward direction, and on both (one problem, not two)
    assert!(with_step(link, &format!("forward: {{ {jitter} }}")).contains(after));
    let both = with_step(link, &format!("both: {{ {jitter} }}"));
    assert_eq!(both.matches("longer than the delay").count(), 1, "{both}");
    // A link that is wrong from the start: the step that only sets a rate is not blamed
    let wrong = "      both:\n        delay: 10ms\n        jitter: { amount: 20ms, distribution: uniform }\n";
    let message = with_step(wrong, "both: { rate: 1mbit }");
    assert!(
        message.contains("longer than the delay"),
        "the link itself is wrong: {message}"
    );
    assert!(
        !message.contains(after),
        "the step is not blamed: {message}"
    );
}

/// An event with both `until` and `count` is wrong as it is; it is not made into its copies on top of that, which would add the
/// faults of the copies (here outages that overlap) to the one fault that matters.
#[test]
fn an_event_with_until_and_count_is_not_expanded_into_copies() {
    let message = said(&scenario(
        "20s",
        "",
        STREAM,
        "timeline:\n  - { at: 1s, link: l, outage: 4s, every: 3s, until: 9s, count: 3 }\n",
    ));
    assert!(
        message.contains("until") && message.contains("count"),
        "{message}"
    );
    assert!(!message.contains("overlaps the outage"), "{message}");
    // The same event with one of the two keys is expanded, and its copies overlap
    let copies = said(&scenario(
        "20s",
        "",
        STREAM,
        "timeline:\n  - { at: 1s, link: l, outage: 4s, every: 3s, count: 3 }\n",
    ));
    assert!(copies.contains("overlaps the outage"), "{copies}");
}

/// A jitter from a table without a table says that the table is missing, and nothing about delays below zero, which it cannot work out.
#[test]
fn a_jitter_from_a_table_that_is_not_there_is_not_judged_against_the_delay() {
    let message = said(&scenario(
        "10s",
        "      both:\n        delay: 1ms\n        jitter: { amount: 9ms, distribution: table }\n",
        STREAM,
        "",
    ));
    assert!(message.contains("table"), "{message}");
    assert!(
        !message.contains("some delays would be below zero"),
        "{message}"
    );
}

/// The limits are the ones that the documents state; the tests above use the names, and these are the numbers.
#[test]
fn the_limits_are_the_documented_numbers() {
    assert_eq!((MAX_NODES, MAX_LINKS, MAX_ROUTES), (256, 1024, 8192));
    assert_eq!(MAX_PACKETS_PER_SECOND, 100_000);
    assert_eq!(MAX_BURST_PACKETS, 100_000);
    assert_eq!(MAX_QUEUE_PACKETS, 1_000_000);
    assert_eq!(MAX_QUEUE_BYTES, 1_073_741_824);
    assert_eq!(MAX_BINS, 1_000_000);
}
