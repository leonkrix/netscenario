//! From a workload of the scenario to the schedule of the agent, and to the words that start the sender (WRK-02, D-071).
//!
//! The scenario says what a workload is ("a Poisson process of 800 kbit"); the agent walks a [`Schedule`] made from it and
//! a seed. The seed of the workload is derived from the seed of the run and the number of the workload, so the same seed
//! of a scenario gives the same packets at the same moments, and two workloads of one run draw unrelated sequences.

use netscenario_agent::rng::derive_seed;
use netscenario_agent::schedule::{Distribution, Schedule, Stream};
use netscenario_model::{Generator, Pattern, PhaseDistribution};

use crate::Error;
use crate::run::failure;

fn distribution(distribution: PhaseDistribution) -> Distribution {
    match distribution {
        PhaseDistribution::Exponential => Distribution::Exponential,
        PhaseDistribution::Fixed => Distribution::Fixed,
    }
}

/// The schedule of the `index`th sending workload of a run whose seed is `run_seed`.
///
/// # Errors
///
/// [`Error::Run`] for a pattern of a later version of the model that this version does not know.
pub(crate) fn schedule_of(
    generator: &Generator<'_>,
    run_seed: u64,
    index: usize,
) -> Result<Schedule, Error> {
    let seed = derive_seed(run_seed, index as u64);
    let packet_size = generator.packet_size.bytes();
    let stream = |rate: netscenario_model::Rate| Stream {
        bits_per_second: rate.bits_per_second(),
        packet_size,
    };
    Ok(match generator.pattern {
        Pattern::Cbr { rate } => Schedule::Cbr(stream(rate)),
        Pattern::Poisson { rate } => Schedule::Poisson {
            stream: stream(rate),
            seed,
        },
        Pattern::OnOff {
            rate,
            on,
            off,
            distribution: phases,
        } => Schedule::OnOff {
            stream: stream(rate),
            on_ns: on.as_nanos(),
            off_ns: off.as_nanos(),
            distribution: distribution(phases),
            seed,
        },
        Pattern::Bursty {
            packets,
            gap,
            pause,
            distribution: phases,
        } => Schedule::Bursty {
            packet_size,
            packets: u64::from(packets),
            gap_ns: gap.as_nanos(),
            pause_ns: pause.as_nanos(),
            distribution: distribution(phases),
            seed,
        },
        _ => {
            return Err(failure(
                "build the schedule of a workload",
                format!(
                    "the workload '{}' has a pattern that this version does not know",
                    generator.id
                ),
            ));
        }
    })
}

/// The options of `netscenario-agent send` that describe the schedule: the pattern and what it needs, and the size.
pub(crate) fn arguments(schedule: &Schedule) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut add = |name: &str, value: String| {
        words.push(format!("--{name}"));
        words.push(value);
    };
    let phases = |distribution: Distribution| match distribution {
        Distribution::Fixed => "fixed",
        Distribution::Exponential => "exponential",
    };
    add("size", schedule.packet_size().to_string());
    match *schedule {
        Schedule::Cbr(stream) => add("rate", stream.bits_per_second.to_string()),
        Schedule::Poisson { stream, seed } => {
            add("pattern", "poisson".to_owned());
            add("rate", stream.bits_per_second.to_string());
            add("seed", seed.to_string());
        }
        Schedule::OnOff {
            stream,
            on_ns,
            off_ns,
            distribution,
            seed,
        } => {
            add("pattern", "onoff".to_owned());
            add("rate", stream.bits_per_second.to_string());
            add("on-ns", on_ns.to_string());
            add("off-ns", off_ns.to_string());
            add("distribution", phases(distribution).to_owned());
            add("seed", seed.to_string());
        }
        Schedule::Bursty {
            packets,
            gap_ns,
            pause_ns,
            distribution,
            seed,
            ..
        } => {
            add("pattern", "bursty".to_owned());
            add("burst-packets", packets.to_string());
            add("gap-ns", gap_ns.to_string());
            add("pause-ns", pause_ns.to_string());
            add("distribution", phases(distribution).to_owned());
            add("seed", seed.to_string());
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscenario_model::{Scenario, Workload};

    fn generators(yaml: &str) -> Vec<Schedule> {
        let scenario = Scenario::from_yaml(
            &format!("version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n{yaml}"),
            "t.yaml",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        scenario
            .workloads
            .iter()
            .filter_map(Workload::generator)
            .enumerate()
            .map(|(index, generator)| schedule_of(&generator, 42, index).unwrap())
            .collect()
    }

    const KINDS: &str = "  - { id: c, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 125b }
  - { id: p, kind: udp-poisson, from: a, to: b, rate: 1mbit, packet-size: 125b }
  - { id: o, kind: udp-onoff, from: a, to: b, rate: 8mbit, packet-size: 100b, on: 5ms, off: 15ms, distribution: exponential }
  - { id: x, kind: udp-bursty, from: b, to: a, packet-size: 100b, burst-packets: 4, gap: 100us, pause: 2ms }
";

    #[test]
    fn a_workload_becomes_the_schedule_of_its_pattern_with_a_seed_of_its_own() {
        let schedules = generators(KINDS);
        let stream = Stream {
            bits_per_second: 1_000_000,
            packet_size: 125,
        };
        assert_eq!(schedules[0], Schedule::Cbr(stream));
        assert_eq!(
            schedules[1],
            Schedule::Poisson {
                stream,
                seed: derive_seed(42, 1)
            }
        );
        assert_eq!(
            schedules[2],
            Schedule::OnOff {
                stream: Stream {
                    bits_per_second: 8_000_000,
                    packet_size: 100
                },
                on_ns: 5_000_000,
                off_ns: 15_000_000,
                distribution: Distribution::Exponential,
                seed: derive_seed(42, 2),
            }
        );
        assert_eq!(
            schedules[3],
            Schedule::Bursty {
                packet_size: 100,
                packets: 4,
                gap_ns: 100_000,
                pause_ns: 2_000_000,
                distribution: Distribution::Fixed,
                seed: derive_seed(42, 3),
            }
        );
    }

    /// The words that start the sender, read back by the agent's own parser, give the schedule they were made from. A
    /// word that is lost or misspelled on the way would send another pattern.
    #[test]
    fn the_arguments_describe_the_same_schedule_to_the_agent() {
        for schedule in generators(KINDS) {
            let mut words = vec![
                "agent".to_owned(),
                "send".to_owned(),
                "--to".to_owned(),
                "127.0.0.1:9".to_owned(),
                "--duration-ns".to_owned(),
                "1".to_owned(),
            ];
            words.extend(arguments(&schedule));
            let parsed = netscenario_agent::cli::schedule_from_words(&words[2..]).unwrap();
            assert_eq!(parsed, schedule, "{words:?}");
        }
    }
}
