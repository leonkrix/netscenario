//! Property tests: a value that is written and read again is the same value, and a scenario that is made of
//! valid parts is valid, whatever the parts are.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{
    Backend, Change, Direction, Duration, Edit, Event, FormatVersion, Impairments,
    ImpairmentsChange, Jitter, JitterDistribution, Link, Loss, MAX_PACKET_SIZE,
    MAX_PACKETS_PER_SECOND, MIN_PACKET_SIZE, Name, Probability, Rate, Reorder, Scenario, Size,
    Topology, Workload, WorkloadKind,
};
use proptest::prelude::*;
use proptest::strategy::ValueTree;

proptest! {
    #[test]
    fn a_duration_reads_back_as_itself(nanoseconds in any::<u64>()) {
        let duration = Duration::from_nanos(nanoseconds);
        prop_assert_eq!(duration.to_string().parse::<Duration>().unwrap(), duration);
    }

    #[test]
    fn a_rate_reads_back_as_itself(bits in 1..=u64::MAX) {
        let rate = Rate::from_bits_per_second(bits).unwrap();
        prop_assert_eq!(rate.to_string().parse::<Rate>().unwrap(), rate);
    }

    #[test]
    fn a_size_reads_back_as_itself(bytes in any::<u64>()) {
        let size = Size::from_bytes(bytes);
        prop_assert_eq!(size.to_string().parse::<Size>().unwrap(), size);
    }

    #[test]
    fn a_probability_reads_back_as_itself(millionths in 0..=100_000_000u32) {
        let probability = Probability::from_micro_percent(millionths).unwrap();
        prop_assert_eq!(probability.to_string().parse::<Probability>().unwrap(), probability);
    }

    #[test]
    fn a_name_reads_back_as_itself(text in "[a-z]([a-z0-9-]{0,30}[a-z0-9])?") {
        let name = Name::new(&text).unwrap();
        prop_assert_eq!(name.to_string(), text);
    }

    #[test]
    fn a_number_without_a_unit_is_never_a_duration(number in "[0-9]{1,12}(\\.[0-9]{1,6})?") {
        prop_assert!(number.parse::<Duration>().is_err());
        prop_assert!(number.parse::<Probability>().is_err());
    }

    #[test]
    fn a_scenario_made_of_valid_parts_is_valid_and_survives_the_text(scenario in valid_scenario()) {
        prop_assert_eq!(scenario.validate(), Vec::new());
        let written = serde_saphyr::to_string(&scenario).unwrap();
        let again = Scenario::from_yaml(&written, "generated.yaml");
        prop_assert!(again.is_ok(), "{}\n{}", again.unwrap_err(), written);
        prop_assert_eq!(again.unwrap(), scenario.clone());
        // JSON is a subset of YAML and is accepted as it is
        let json = serde_json::to_string(&scenario).unwrap();
        prop_assert_eq!(Scenario::from_yaml(&json, "generated.json").unwrap(), scenario);
    }
}

fn probability(millionths: u32) -> Probability {
    Probability::from_micro_percent(millionths).unwrap()
}

/// A loss of any model, with parameters that the format accepts: the two transitions out of one state of the
/// four-state model add up to at most 100 %.
fn loss() -> impl Strategy<Value = Loss> {
    const WHOLE: u32 = 100_000_000;
    prop_oneof![
        (0..=WHOLE).prop_map(|p| Loss::Random { p: probability(p) }),
        (0..=WHOLE, 0..=WHOLE, 0..=WHOLE, 0..=WHOLE).prop_map(|(p, r, loss_bad, loss_good)| {
            Loss::GilbertElliott {
                p: probability(p),
                r: probability(r),
                loss_bad: probability(loss_bad),
                loss_good: probability(loss_good),
            }
        }),
        (
            0..=WHOLE,
            any::<u32>(),
            0..=WHOLE,
            any::<u32>(),
            0..=WHOLE,
            0..=WHOLE
        )
            .prop_map(|(p13, p14_part, p31, p32_part, p23, _)| {
                Loss::Markov4 {
                    p13: probability(p13),
                    p14: probability(p14_part % (WHOLE - p13 + 1)),
                    p31: probability(p31),
                    p32: probability(p32_part % (WHOLE - p31 + 1)),
                    p23: probability(p23),
                }
            }),
    ]
}

fn impairments() -> impl Strategy<Value = Impairments> {
    (
        (
            proptest::option::of(1..=10_000_000_000u64),
            any::<bool>(),
            proptest::option::of(loss()),
            proptest::option::of(1..=u64::MAX / 2),
            any::<u32>(),
        ),
        (
            proptest::option::of((0..=100_000_000u32, proptest::option::of(1..=1000u32))),
            proptest::option::of(0..=100_000_000u32),
            proptest::option::of(0..=100_000_000u32),
        ),
    )
        .prop_map(
            |((delay, normal, loss, rate, jitter_part), (reorder, duplicate, corrupt))| {
                let delay = delay.map(Duration::from_nanos);
                // A jitter that stays within what the format accepts: at most the delay, or a quarter of it if normal
                let jitter = delay.map(|delay| {
                    let limit = if normal {
                        delay.as_nanos() / 4
                    } else {
                        delay.as_nanos()
                    };
                    let amount = u64::from(jitter_part) % (limit + 1);
                    Jitter {
                        amount: Duration::from_nanos(amount),
                        distribution: if normal {
                            JitterDistribution::Normal
                        } else {
                            JitterDistribution::Uniform
                        },
                    }
                });
                Impairments {
                    delay,
                    jitter,
                    loss,
                    // Reordering holds packets back for the delay, so it needs one
                    reorder: reorder
                        .filter(|_| delay.is_some())
                        .map(|(share, gap)| Reorder {
                            share: probability(share),
                            gap,
                        }),
                    duplicate: duplicate.map(probability),
                    corrupt: corrupt.map(probability),
                    rate: rate.map(|bits| Rate::from_bits_per_second(bits).unwrap()),
                }
            },
        )
}

fn workload(index: usize, duration_nanos: u64) -> impl Strategy<Value = Workload> {
    (
        MIN_PACKET_SIZE..=MAX_PACKET_SIZE,
        1..=MAX_PACKETS_PER_SECOND,
        any::<bool>(),
        0..duration_nanos,
        1..=duration_nanos,
    )
        .prop_map(move |(size, per_second, with_window, start, stop_part)| {
            // The rate that gives exactly `per_second` packets of `size` bytes per second
            let rate = Rate::from_bits_per_second(size * 8 * per_second).unwrap();
            let (start, stop) = if with_window {
                let stop = (start + 1 + stop_part % (duration_nanos - start)).min(duration_nanos);
                (
                    Some(Duration::from_nanos(start)),
                    Some(Duration::from_nanos(stop)),
                )
            } else {
                (None, None)
            };
            Workload {
                id: Name::new(&format!("stream-{index}")).unwrap(),
                kind: WorkloadKind::UdpCbr,
                from: Name::new("a").unwrap(),
                to: Name::new("b").unwrap(),
                rate,
                packet_size: Size::from_bytes(size),
                start,
                stop,
            }
        })
}

fn valid_scenario() -> impl Strategy<Value = Scenario> {
    (
        1..=3_600_000_000_000u64,
        proptest::option::of(any::<u64>()),
        0..3usize,
        any::<bool>(),
        proptest::collection::vec((0..3u8, 0..=100_000_000u32), 0..4),
    )
        .prop_flat_map(|(duration, seed, extra, kernel, steps)| {
            let workloads: Vec<_> = (0..=extra).map(|index| workload(index, duration)).collect();
            (
                Just((duration, seed, kernel, steps)),
                impairments(),
                impairments(),
                prop_oneof![Just(0u8), Just(1), Just(2)],
                workloads,
            )
        })
        .prop_map(
            |((duration, seed, kernel, steps), forward, reverse, shape, workloads)| {
                let (forward, reverse, both) = match shape {
                    0 => (Some(forward), Some(reverse), None),
                    1 => (None, None, Some(forward)),
                    _ => (Some(forward), None, None),
                };
                Scenario {
                    version: FormatVersion,
                    name: Name::new("generated").unwrap(),
                    seed,
                    duration: Duration::from_nanos(duration),
                    backend: if kernel {
                        Backend::Kernel
                    } else {
                        Backend::Auto
                    },
                    topology: Topology {
                        nodes: vec![Name::new("a").unwrap(), Name::new("b").unwrap()],
                        links: vec![Link {
                            id: Name::new("link").unwrap(),
                            between: [Name::new("a").unwrap(), Name::new("b").unwrap()],
                            forward,
                            reverse,
                            both,
                        }],
                    },
                    workloads,
                    timeline: timeline(duration, &steps),
                }
            },
        )
}

/// Events that are valid by construction: each in a slot of its own (an eighth of the duration), so that no
/// outage overlaps another, none ends after the end, and a link that goes down comes up in the next slot.
/// The steps change values that need nothing else (`duplicate`, `rate`) and take one away with `null`.
fn timeline(duration: u64, steps: &[(u8, u32)]) -> Vec<Event> {
    let slot = duration / 8;
    let link = Name::new("link").unwrap();
    let mut events = Vec::new();
    let mut at = 0u64;
    for (kind, value) in steps {
        if slot == 0 || at + 2 * slot >= duration {
            break;
        }
        let moment = Duration::from_nanos(at);
        match kind {
            0 => {
                let change = ImpairmentsChange {
                    duplicate: Edit::Set(probability(*value)),
                    rate: Edit::Clear,
                    ..ImpairmentsChange::default()
                };
                events.push(Event::set(
                    moment,
                    link.clone(),
                    Change {
                        both: Some(change),
                        ..Change::default()
                    },
                ));
            }
            1 => events.push(Event::outage(
                moment,
                link.clone(),
                Duration::from_nanos(slot),
                Some(Direction::Forward),
            )),
            _ => {
                events.push(Event::down(moment, link.clone()));
                events.push(Event::up(Duration::from_nanos(at + slot), link.clone()));
                at += slot;
            }
        }
        at += slot;
    }
    events
}

/// Canary: the property above must be able to fail. A valid scenario that is broken on purpose is refused.
#[test]
fn canary_a_broken_scenario_is_refused() {
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let mut scenario = valid_scenario().new_tree(&mut runner).unwrap().current();
    assert_eq!(scenario.validate(), Vec::new());
    scenario.workloads[0].packet_size = Size::from_bytes(1);
    assert_ne!(scenario.validate(), Vec::new());
}

/// Canary: the two rules that the generator keeps are rules of the format, so the property can fail on them.
#[test]
fn canary_reordering_without_a_delay_and_a_four_state_model_above_100_percent_are_refused() {
    let yaml = |both: &str| {
        format!(
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {both}\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n"
        )
    };
    let text = |both: &str| {
        Scenario::from_yaml(&yaml(both), "t.yaml")
            .unwrap_err()
            .to_string()
    };
    assert!(text("{ reorder: { share: 25% } }").contains("`reorder` needs a `delay`"));
    assert!(
        text("{ loss: { model: markov4, p13: 60%, p31: 30%, p32: 10%, p23: 20%, p14: 50% } }")
            .contains("`p13` (60%) and `p14` (50%) add up to more than 100%")
    );
    assert!(
        Scenario::from_yaml(
            &yaml("{ delay: 10ms, reorder: { share: 25%, gap: 5 } }"),
            "t.yaml"
        )
        .is_ok()
    );
}
