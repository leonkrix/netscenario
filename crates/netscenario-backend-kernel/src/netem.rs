//! The arguments of `tc ... netem` for the impairments of one direction.
//!
//! A direction has the stages **loss, rate, delay with jitter**, in this order: a packet may be dropped when it
//! enters, the packets that are kept are sent one after the other at the rate, and each of them then travels
//! for its own delay. `netem` does all of it in one qdisc, but not in this order: with a `rate` it sends a
//! packet at the later of its own moment and the moment of the packet in front of it, so a jitter is cut off
//! from below and the order is kept (measured: a configured jitter of 5 ms came out as 4.1 ms, and the mean
//! delay 1.8 ms too high, `docs/FIDELITY.md`). A direction with both a jitter and a rate is therefore built
//! from two `netem` qdiscs, the rate in the first and the delay with its jitter in the second, which is its
//! child. Every other direction needs only one.

use netscenario_model::{Duration, Impairments, JitterDistribution, Loss, MAX_PACKETS_PER_SECOND};

/// The most packets a queue may hold. A queue this long at the largest packet size takes 1.4 GB, so a sender
/// that overruns a slow link cannot use all the memory of the machine.
const MAX_QUEUE: u64 = 1_000_000;

/// The packets that the queue of a link holds while they wait for the rate, `netem`'s default.
const LINK_QUEUE: u64 = 1000;

/// What to tell `tc` for one direction: the arguments after `netem` of the first qdisc, and, for a direction
/// with a jitter and a rate, those of the second qdisc, which is attached to the first as its child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stages {
    /// The root qdisc.
    pub(crate) first: Vec<String>,
    /// The child of the root, if the delay and its jitter must act after the rate.
    pub(crate) second: Option<Vec<String>>,
}

/// The packets that `netem` may hold for the delay of a link.
///
/// `netem` drops a packet when its queue is full, and its default is 1000 packets. With a delay of 20 ms and
/// 100,000 packets per second (the most a workload may send, `MAX_PACKETS_PER_SECOND`) 2000 packets are in
/// flight, so the default would lose packets that nobody asked to lose. The limit covers the packets in flight
/// at the largest rate and leaves the default on top for queueing. The user's control of the queue comes with
/// release 0.3 (`docs/COVERAGE.md`, "Queue limit").
pub(crate) fn queue_limit(delay: Option<Duration>, jitter: Option<Duration>) -> u64 {
    let nanoseconds = delay.map_or(0, Duration::as_nanos) + jitter.map_or(0, Duration::as_nanos);
    let in_flight = u128::from(MAX_PACKETS_PER_SECOND) * u128::from(nanoseconds) / 1_000_000_000;
    u64::try_from(in_flight)
        .unwrap_or(MAX_QUEUE)
        .saturating_add(1000)
        .min(MAX_QUEUE)
}

/// A time as `netem` takes it: whole microseconds.
///
/// # Errors
///
/// Text if the time is not a whole number of microseconds: `netem` would round it.
pub(crate) fn microseconds(time: Duration) -> Result<String, String> {
    let nanoseconds = time.as_nanos();
    if nanoseconds.is_multiple_of(1000) {
        Ok(format!("{}us", nanoseconds / 1000))
    } else {
        Err(format!(
            "netem works in whole microseconds and {time} is not a multiple of 1us"
        ))
    }
}

/// A probability as a percentage with the six decimals that the format allows.
fn percent(micro_percent: u32) -> String {
    format!(
        "{}.{:06}%",
        micro_percent / 1_000_000,
        micro_percent % 1_000_000
    )
}

/// The arguments of the delay with its jitter: `delay D J [distribution normal]`.
fn delay_arguments(impairments: &Impairments) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    if let Some(delay) = impairments.delay {
        arguments.push("delay".to_owned());
        arguments.push(microseconds(delay)?);
        if let Some(jitter) = &impairments.jitter {
            arguments.push(microseconds(jitter.amount)?);
            if jitter.distribution == JitterDistribution::Normal {
                arguments.push("distribution".to_owned());
                arguments.push("normal".to_owned());
            }
        }
    }
    Ok(arguments)
}

/// The arguments of the loss: `loss random P% [seed S]`. The seed is used only with loss (D-013).
///
/// Only `random` is built so far; the other models and the other impairments arrive with step 4.2 of
/// release 0.2, and the capabilities of the backend say so before a run gets here.
fn loss_arguments(impairments: &Impairments, seed: Option<u64>) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    match &impairments.loss {
        None => {}
        Some(Loss::Random { p }) => {
            arguments.push("loss".to_owned());
            arguments.push("random".to_owned());
            arguments.push(percent(p.micro_percent()));
            if let Some(seed) = seed {
                arguments.push("seed".to_owned());
                arguments.push(seed.to_string());
            }
        }
        Some(_) => return Err(NOT_BUILT.to_owned()),
    }
    Ok(arguments)
}

/// What a direction is told that asks for an impairment that this build of the backend does not apply yet.
const NOT_BUILT: &str = "this loss model, reordering, duplication and corruption are built into the Kernel backend in step 4.2 of release 0.2 and are not applied yet";

/// The arguments of the rate: `rate Nbit`.
fn rate_arguments(impairments: &Impairments) -> Vec<String> {
    impairments.rate.map_or_else(Vec::new, |rate| {
        vec!["rate".to_owned(), format!("{}bit", rate.bits_per_second())]
    })
}

/// The qdiscs for the impairments of a direction, or `None` if it impairs nothing.
///
/// # Errors
///
/// Text if a value cannot be applied exactly.
pub(crate) fn arguments(
    impairments: &Impairments,
    seed: Option<u64>,
) -> Result<Option<Stages>, String> {
    if impairments.reorder.is_some()
        || impairments.duplicate.is_some()
        || impairments.corrupt.is_some()
    {
        return Err(NOT_BUILT.to_owned());
    }
    if impairments.delay.is_none() && impairments.loss.is_none() && impairments.rate.is_none() {
        return Ok(None);
    }
    let jitter = impairments.jitter.as_ref();
    let limit = |packets: u64| vec!["limit".to_owned(), packets.to_string()];
    if jitter.is_some() && impairments.rate.is_some() {
        // The rate first, with the loss in front of it, then the delay with its jitter
        let mut first = limit(LINK_QUEUE);
        first.extend(loss_arguments(impairments, seed)?);
        first.extend(rate_arguments(impairments));
        let mut second = limit(queue_limit(impairments.delay, jitter.map(|j| j.amount)));
        second.extend(delay_arguments(impairments)?);
        return Ok(Some(Stages {
            first,
            second: Some(second),
        }));
    }
    let mut first = limit(queue_limit(impairments.delay, jitter.map(|j| j.amount)));
    first.extend(delay_arguments(impairments)?);
    first.extend(loss_arguments(impairments, seed)?);
    first.extend(rate_arguments(impairments));
    Ok(Some(Stages {
        first,
        second: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscenario_model::Scenario;

    /// The impairments of the first link of a scenario with the text `both`.
    fn impairments(both: &str) -> Impairments {
        let yaml = format!(
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {both}\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n"
        );
        Scenario::from_yaml(&yaml, "t.yaml").unwrap().topology.links[0]
            .both
            .clone()
            .unwrap()
    }

    /// The arguments of the only qdisc of a direction that needs just one.
    fn text(both: &str, seed: Option<u64>) -> String {
        let stages = arguments(&impairments(both), seed).unwrap().unwrap();
        assert_eq!(stages.second, None, "{both} needs one qdisc");
        stages.first.join(" ")
    }

    /// The arguments of the two qdiscs of a direction with a jitter and a rate.
    fn two(both: &str, seed: Option<u64>) -> (String, String) {
        let stages = arguments(&impairments(both), seed).unwrap().unwrap();
        (
            stages.first.join(" "),
            stages.second.expect("a second qdisc").join(" "),
        )
    }

    #[test]
    fn delay_is_in_whole_microseconds() {
        assert_eq!(text("{ delay: 20ms }", None), "limit 3000 delay 20000us");
        assert_eq!(text("{ delay: 1500us }", None), "limit 1150 delay 1500us");
    }

    #[test]
    fn jitter_names_its_distribution_only_when_it_is_not_uniform() {
        assert_eq!(
            text("{ delay: 20ms, jitter: { amount: 5ms } }", None),
            "limit 3500 delay 20000us 5000us"
        );
        assert_eq!(
            text(
                "{ delay: 20ms, jitter: { amount: 5ms, distribution: normal } }",
                None
            ),
            "limit 3500 delay 20000us 5000us distribution normal"
        );
    }

    #[test]
    fn loss_has_six_decimals_and_a_seed_only_when_there_is_one() {
        assert_eq!(
            text("{ loss: { model: random, p: 0.5% } }", None),
            "limit 1000 loss random 0.500000%"
        );
        assert_eq!(
            text("{ loss: { model: random, p: 100% } }", Some(7)),
            "limit 1000 loss random 100.000000% seed 7"
        );
        assert_eq!(
            text("{ loss: { model: random, p: 0.000001% } }", None),
            "limit 1000 loss random 0.000001%"
        );
    }

    #[test]
    fn a_seed_without_loss_is_not_passed() {
        assert_eq!(text("{ delay: 5ms }", Some(7)), "limit 1500 delay 5000us");
    }

    #[test]
    fn the_rate_is_in_bits() {
        assert_eq!(text("{ rate: 2mbit }", None), "limit 1000 rate 2000000bit");
    }

    #[test]
    fn nothing_to_impair_gives_no_arguments() {
        assert_eq!(arguments(&Impairments::new(), None), Ok(None));
    }

    /// Canary: a time that `netem` would round is refused instead.
    #[test]
    fn a_time_below_a_microsecond_is_refused() {
        let error = arguments(&impairments("{ delay: 1500ns }"), None).unwrap_err();
        assert!(
            error.contains("whole microseconds") && error.contains("1500ns"),
            "{error}"
        );
    }

    #[test]
    fn the_queue_covers_the_packets_in_flight() {
        let ms = |n| Duration::from_millis(n);
        assert_eq!(queue_limit(None, None), 1000);
        assert_eq!(queue_limit(ms(20), None), 3000);
        assert_eq!(queue_limit(ms(500), ms(100)), 61_000);
        assert_eq!(queue_limit(Duration::from_secs(3600), None), 1_000_000);
    }

    /// The rate comes first and the jitter after it, so that the jitter is the configured one. The loss is in
    /// front of the rate, where `netem` has always applied it, and keeps its seed.
    #[test]
    fn a_jitter_with_a_rate_is_built_from_two_qdiscs_rate_first() {
        let (first, second) = two(
            "{ delay: 40ms, jitter: { amount: 5ms, distribution: normal }, rate: 10mbit }",
            None,
        );
        assert_eq!(first, "limit 1000 rate 10000000bit");
        assert_eq!(
            second,
            "limit 5500 delay 40000us 5000us distribution normal"
        );
        let (first, second) = two(
            "{ delay: 20ms, jitter: { amount: 5ms }, loss: { model: random, p: 1% }, rate: 2mbit }",
            Some(9),
        );
        assert_eq!(
            first,
            "limit 1000 loss random 1.000000% seed 9 rate 2000000bit"
        );
        assert_eq!(second, "limit 3500 delay 20000us 5000us");
    }

    /// Canary: only the combination needs two qdiscs. A jitter alone, a rate alone and a delay with a rate keep
    /// the one qdisc that they have always had, so nothing that was measured about them moves.
    #[test]
    fn every_other_direction_keeps_its_one_qdisc() {
        for both in [
            "{ delay: 20ms, jitter: { amount: 5ms } }",
            "{ rate: 2mbit }",
            "{ delay: 40ms, rate: 10mbit }",
            "{ delay: 40ms, loss: { model: random, p: 1% }, rate: 10mbit }",
            "{ loss: { model: random, p: 1% } }",
        ] {
            let stages = arguments(&impairments(both), None).unwrap().unwrap();
            assert_eq!(stages.second, None, "{both}");
        }
    }

    #[test]
    fn a_time_below_a_microsecond_in_the_second_qdisc_is_refused_too() {
        let error = arguments(
            &impairments("{ delay: 20ms, jitter: { amount: 1500ns }, rate: 2mbit }"),
            None,
        )
        .unwrap_err();
        assert!(error.contains("1500ns"), "{error}");
    }
}
