//! The arguments of `tc ... netem` for the impairments of one direction.
//!
//! A direction has the stages **duplication, loss, corruption, rate, delay with jitter and reordering**, in
//! this order (the order of `netem_enqueue`, `docs/SCENARIO-FORMAT.md`): a packet may be duplicated, the copy
//! and the original may each be dropped or corrupted when they enter, the packets that are kept are sent one
//! after the other at the rate, and each of them then travels for its own delay, or, if it is chosen for
//! reordering, is sent on at once. `netem` does all of it in one qdisc, but not in this order. With a `rate`
//! it sends a packet at the later of its own moment and the moment of the packet in front of it, so a jitter
//! is cut off from below and the order is kept (measured: a configured jitter of 5 ms came out as 4.1 ms,
//! and the mean delay 1.8 ms too high, `docs/FIDELITY.md`); and a packet chosen for reordering is put at the
//! head of the queue and bypasses the rate altogether (`sch_netem.c`: the rate is worked out only for the
//! packets that are delayed). A direction with a rate and a jitter or a reordering is therefore built from
//! two `netem` qdiscs, the duplication, loss, corruption and rate in the first and the delay with its jitter
//! and the reordering in the second, which is its child. Every other direction needs only one.
//!
//! Duplication stays in the first qdisc: `netem` enqueues the copy at the root of the tree, so a copy made
//! in the second qdisc would pass the loss and the rate a second time. Both qdiscs get a seed of their own
//! where they draw numbers that the seed is to repeat (D-054).

use netscenario_core::derive_stage_seed;
use netscenario_model::{
    Duration, Impairments, JitterDistribution, Loss, MAX_PACKETS_PER_SECOND, Probability, Reorder,
};

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

/// A probability for `tc`: `P%`.
fn probability(value: Probability) -> String {
    percent(value.micro_percent())
}

/// The arguments of the loss. `random` is `loss random P%`; the Gilbert-Elliott model is `loss gemodel p r
/// 1-h 1-k`, whose third argument is the loss in the bad state and the fourth the loss in the good state
/// (measured in S12, D-048: the letters suggest the complements, the kernel does not); the four-state model
/// is `loss state p13 p31 p32 p23 p14`.
fn loss_arguments(impairments: &Impairments) -> Vec<String> {
    let words = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    match &impairments.loss {
        None => Vec::new(),
        Some(Loss::Random { p }) => {
            let mut arguments = words(&["loss", "random"]);
            arguments.push(probability(*p));
            arguments
        }
        Some(Loss::GilbertElliott {
            p,
            r,
            loss_bad,
            loss_good,
        }) => {
            let mut arguments = words(&["loss", "gemodel"]);
            arguments.extend([p, r, loss_bad, loss_good].map(|value| probability(*value)));
            arguments
        }
        Some(Loss::Markov4 {
            p13,
            p31,
            p32,
            p23,
            p14,
        }) => {
            let mut arguments = words(&["loss", "state"]);
            arguments.extend([p13, p31, p32, p23, p14].map(|value| probability(*value)));
            arguments
        }
    }
}

/// The arguments of the duplication: `duplicate P%`.
fn duplicate_arguments(impairments: &Impairments) -> Vec<String> {
    impairments
        .duplicate
        .map_or_else(Vec::new, |p| vec!["duplicate".to_owned(), probability(p)])
}

/// The arguments of the corruption: `corrupt P%`. `netem` flips one bit of one byte of the frame, the Ethernet
/// header included (S13b, D-040), so the share of the packets that a receiver drops is less than `P`.
fn corrupt_arguments(impairments: &Impairments) -> Vec<String> {
    impairments
        .corrupt
        .map_or_else(Vec::new, |p| vec!["corrupt".to_owned(), probability(p)])
}

/// The arguments of the reordering: `reorder P% [gap G]`. `P` is the share of the packets that are sent on at
/// once (measured, D-048).
fn reorder_arguments(impairments: &Impairments) -> Vec<String> {
    impairments
        .reorder
        .map_or_else(Vec::new, |Reorder { share, gap }| {
            let mut arguments = vec!["reorder".to_owned(), probability(share)];
            if let Some(gap) = gap {
                arguments.push("gap".to_owned());
                arguments.push(gap.to_string());
            }
            arguments
        })
}

/// `seed S`, if there is a seed.
fn seed_arguments(seed: Option<u64>) -> Vec<String> {
    seed.map_or_else(Vec::new, |seed| vec!["seed".to_owned(), seed.to_string()])
}

/// The arguments of the rate: `rate Nbit`.
fn rate_arguments(impairments: &Impairments) -> Vec<String> {
    impairments.rate.map_or_else(Vec::new, |rate| {
        vec!["rate".to_owned(), format!("{}bit", rate.bits_per_second())]
    })
}

/// The qdiscs for the impairments of a direction, or `None` if it impairs nothing.
///
/// `seed` is the seed of the direction. It goes to every qdisc that draws numbers the seed is to repeat: loss,
/// duplication and corruption in the first, reordering in the one with the delay (the second, or the only one),
/// the latter under a seed derived for the second stage. A delay with a jitter alone is not seeded (D-030).
///
/// # Errors
///
/// Text if a value cannot be applied exactly.
pub(crate) fn arguments(
    impairments: &Impairments,
    seed: Option<u64>,
) -> Result<Option<Stages>, String> {
    if impairments.delay.is_none()
        && impairments.loss.is_none()
        && impairments.rate.is_none()
        && impairments.duplicate.is_none()
        && impairments.corrupt.is_none()
    {
        return Ok(None);
    }
    let jitter = impairments.jitter.as_ref();
    let limit = |packets: u64| vec!["limit".to_owned(), packets.to_string()];
    let delay_limit = queue_limit(impairments.delay, jitter.map(|j| j.amount));
    if impairments.rate.is_some() && (jitter.is_some() || impairments.reorder.is_some()) {
        // The duplication, the loss and the rate first, then the delay with its jitter and the reordering
        let mut first = limit(LINK_QUEUE);
        first.extend(loss_arguments(impairments));
        first.extend(duplicate_arguments(impairments));
        first.extend(corrupt_arguments(impairments));
        if impairments.loss.is_some()
            || impairments.duplicate.is_some()
            || impairments.corrupt.is_some()
        {
            first.extend(seed_arguments(seed));
        }
        first.extend(rate_arguments(impairments));
        let mut second = limit(delay_limit);
        second.extend(delay_arguments(impairments)?);
        second.extend(reorder_arguments(impairments));
        if impairments.reorder.is_some() {
            second.extend(seed_arguments(seed.map(|seed| derive_stage_seed(seed, 1))));
        }
        return Ok(Some(Stages {
            first,
            second: Some(second),
        }));
    }
    let mut first = limit(delay_limit);
    first.extend(delay_arguments(impairments)?);
    first.extend(loss_arguments(impairments));
    first.extend(duplicate_arguments(impairments));
    first.extend(corrupt_arguments(impairments));
    first.extend(reorder_arguments(impairments));
    if impairments.is_random() {
        first.extend(seed_arguments(seed));
    }
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

    #[test]
    fn the_burst_models_are_passed_with_the_meaning_that_s12_measured() {
        // The third argument of `gemodel` is the loss in the bad state and the fourth the loss in the good one
        assert_eq!(
            text(
                "{ loss: { model: gilbert-elliott, p: 1%, r: 20%, loss-bad: 100%, loss-good: 0% } }",
                Some(5)
            ),
            "limit 1000 loss gemodel 1.000000% 20.000000% 100.000000% 0.000000% seed 5"
        );
        assert_eq!(
            text(
                "{ loss: { model: markov4, p13: 2%, p31: 30%, p32: 20%, p23: 10%, p14: 1% } }",
                None
            ),
            "limit 1000 loss state 2.000000% 30.000000% 20.000000% 10.000000% 1.000000%"
        );
    }

    #[test]
    fn duplication_and_reordering_are_passed_and_seeded() {
        assert_eq!(
            text("{ duplicate: 20% }", Some(3)),
            "limit 1000 duplicate 20.000000% seed 3"
        );
        assert_eq!(
            text("{ delay: 10ms, reorder: { share: 25% } }", Some(3)),
            "limit 2000 delay 10000us reorder 25.000000% seed 3"
        );
        assert_eq!(
            text("{ delay: 10ms, reorder: { share: 25%, gap: 5 } }", None),
            "limit 2000 delay 10000us reorder 25.000000% gap 5"
        );
        // A duplication alone is an impairment: it must not come out as nothing
        assert!(
            arguments(&impairments("{ duplicate: 1% }"), None)
                .unwrap()
                .is_some()
        );
    }

    /// Reordering puts a packet at the head of the queue, past the rate, so with a rate it belongs behind it
    /// (the measured reason is in the module). Duplication stays with the loss and the rate, and each
    /// queue that draws numbers has a seed of its own.
    #[test]
    fn a_reordering_with_a_rate_is_built_from_two_qdiscs_and_each_has_its_own_seed() {
        let (first, second) = two(
            "{ delay: 10ms, loss: { model: random, p: 1% }, duplicate: 5%, reorder: { share: 25%, gap: 3 }, rate: 2mbit }",
            Some(9),
        );
        assert_eq!(
            first,
            "limit 1000 loss random 1.000000% duplicate 5.000000% seed 9 rate 2000000bit"
        );
        let derived = derive_stage_seed(9, 1);
        assert_eq!(
            second,
            format!("limit 2000 delay 10000us reorder 25.000000% gap 3 seed {derived}")
        );
        assert_ne!(derived, 9);
        // the second queue draws no numbers that the seed is to repeat when it only delays
        let (_, second) = two(
            "{ delay: 20ms, jitter: { amount: 5ms }, duplicate: 5%, rate: 2mbit }",
            Some(9),
        );
        assert_eq!(second, "limit 3500 delay 20000us 5000us");
    }

    /// Canary: a duplication is never in the second queue, where the copy would pass the loss and the rate
    /// again, and a delay with a rate and no reordering keeps its one qdisc.
    #[test]
    fn duplication_is_not_built_into_the_second_qdisc() {
        for both in [
            "{ delay: 20ms, jitter: { amount: 5ms }, duplicate: 5%, rate: 2mbit }",
            "{ delay: 20ms, reorder: { share: 50% }, duplicate: 5%, rate: 2mbit }",
        ] {
            let stages = arguments(&impairments(both), Some(1)).unwrap().unwrap();
            let second = stages.second.expect("two qdiscs").join(" ");
            assert!(!second.contains("duplicate"), "{second}");
            assert!(stages.first.join(" ").contains("duplicate"), "{both}");
        }
        let stages = arguments(
            &impairments("{ delay: 20ms, duplicate: 5%, rate: 2mbit }"),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(stages.second, None);
    }

    #[test]
    fn corruption_is_passed_and_seeded() {
        assert_eq!(
            text("{ corrupt: 10% }", Some(4)),
            "limit 1000 corrupt 10.000000% seed 4"
        );
        assert_eq!(
            text("{ corrupt: 10% }", None),
            "limit 1000 corrupt 10.000000%"
        );
        // The order of netem: loss, then corruption, then the rate
        assert_eq!(
            text(
                "{ loss: { model: random, p: 1% }, corrupt: 2%, rate: 2mbit }",
                Some(4)
            ),
            "limit 1000 loss random 1.000000% corrupt 2.000000% seed 4 rate 2000000bit"
        );
    }

    /// Canary: a corruption alone is an impairment (it must not come out as nothing), and in the tree of two
    /// queues it stays in the first, where the loss and the rate are, and draws from the seed of that queue.
    #[test]
    fn a_corruption_is_never_left_out_and_stays_in_the_first_queue() {
        assert!(
            arguments(&impairments("{ corrupt: 1% }"), None)
                .unwrap()
                .is_some()
        );
        let (first, second) = two(
            "{ delay: 20ms, jitter: { amount: 5ms }, corrupt: 3%, rate: 2mbit }",
            Some(6),
        );
        assert_eq!(first, "limit 1000 corrupt 3.000000% seed 6 rate 2000000bit");
        assert_eq!(second, "limit 3500 delay 20000us 5000us");
    }
}
