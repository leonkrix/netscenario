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
//!
//! A direction that has an **outage** in its timeline gets one more `netem`, the **blackout**, as the child of the
//! last one (D-064). A `netem` hands a packet to its child when the delay of the packet is over, so a loss of 100 %
//! in the blackout drops what is on its way at that moment and what leaves the link in the window, and nothing
//! else: the qdiscs in front of it go on as they did, with their random numbers. It has no delay and draws no
//! number that the seed is to repeat, so it has no seed.

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
/// with a jitter and a rate, those of the second qdisc, which is attached to the first as its child, and, for a
/// direction with an outage, those of the blackout, which is the child of the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stages {
    /// The root qdisc.
    pub(crate) first: Vec<String>,
    /// The child of the root, if the delay and its jitter must act after the rate.
    pub(crate) second: Option<Vec<String>>,
    /// The blackout, if the direction has an outage: the child of the last qdisc above.
    pub(crate) blackout: Option<Vec<String>>,
}

impl Stages {
    /// These stages if no argument holds a white space, else the text that says which. The lines for `tc` are made by
    /// joining the arguments with spaces and may be cut at them again, so an argument must not hold one.
    fn checked(self) -> Result<Self, String> {
        let queues = [
            Some(&self.first),
            self.second.as_ref(),
            self.blackout.as_ref(),
        ];
        let with_space = queues
            .into_iter()
            .flatten()
            .flatten()
            .find(|argument| argument.contains(char::is_whitespace))
            .cloned();
        match with_space {
            Some(argument) => Err(format!("the argument '{argument}' for `tc` holds a space")),
            None => Ok(self),
        }
    }

    /// The `tc` lines that put these stages on `interface`, one for each qdisc, the root first: `replace` builds
    /// them (the root gets the handle `1:` if it has a child), `change` writes into qdiscs that are there. The
    /// qdisc number `n` (from 1) has the handle `n:`, and the parent `n-1:1` unless it is the root.
    pub(crate) fn lines(&self, interface: &str, verb: &str) -> Vec<String> {
        let mut queues: Vec<&Vec<String>> = vec![&self.first];
        queues.extend(self.second.iter());
        queues.extend(self.blackout.iter());
        queues
            .iter()
            .enumerate()
            .map(|(place, arguments)| {
                let position = if place == 0 {
                    if queues.len() > 1 && verb == "replace" {
                        "root handle 1:".to_owned()
                    } else {
                        "root".to_owned()
                    }
                } else {
                    format!("parent {place}:1 handle {}:", place + 1)
                };
                format!(
                    "qdisc {verb} dev {interface} {position} netem {}",
                    arguments.join(" ")
                )
            })
            .collect()
    }
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

/// How the queues of a direction are built, fixed for the whole run (D-059).
///
/// A direction is one `netem` or two (the rate in the first, the delay with its jitter and the reordering in
/// the second, D-030), and which one it is must not change while packets are in it: rebuilding a queue drops
/// the packets that are in flight, and a queue that is built again starts its random numbers again, which
/// is the seed that the run promises to repeat. So the layout is made from **every state** that the
/// direction goes through, and a change only writes a state into queues that are there. A direction whose
/// timeline never has a rate together with a jitter or a reordering is one queue all the time; one whose
/// timeline has it once is two queues all the time, also while the rate is not set. A direction with an outage
/// has the blackout queue behind them all the time, also while no outage is going on.
// The four flags are independent facts about the queues of a direction, not the states of one thing
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    /// Whether the direction is two queues.
    two_stages: bool,
    /// Whether the first (or only) queue draws numbers that the seed repeats, in some state: it is seeded when it
    /// is built, because a change keeps the generator.
    seeded_first: bool,
    /// The same for the second queue.
    seeded_second: bool,
    /// The limit of the first (or only) queue. The longest that any state needs, so that a change never makes it
    /// smaller than the packets that it holds.
    first_limit: u64,
    /// The limit of the second queue.
    second_limit: u64,
    /// Whether the direction has a blackout queue behind the others, for an outage (D-064).
    blackout: bool,
}

impl Layout {
    /// The layout for the states of a direction, with a blackout queue behind them if `blackout` (the direction has
    /// an outage).
    ///
    /// # Errors
    ///
    /// Text if the states cannot be had in one set of queues: a jitter with a normal distribution in one and a
    /// uniform one in another. `netem` keeps the table of the distribution that was last loaded when a change
    /// does not name one (measured, D-059), so a queue that has been normal cannot be made uniform again
    /// without being built again.
    pub(crate) fn of(states: &[Impairments], blackout: bool) -> Result<Self, String> {
        let mut distributions: Vec<JitterDistribution> = states
            .iter()
            .filter_map(|state| state.jitter.as_ref().map(|jitter| jitter.distribution))
            .collect();
        distributions.dedup();
        if distributions.len() > 1 {
            return Err(
                "the jitter has a normal distribution in one state and a uniform one in another; `netem` keeps the distribution that was loaded last, so the queue would have to be built again, which drops the packets that are in flight and starts its random numbers over"
                    .to_owned(),
            );
        }
        let two_stages = states.iter().any(|state| {
            state.rate.is_some() && (state.jitter.is_some() || state.reorder.is_some())
        });
        let longest =
            |limit: &dyn Fn(&Impairments) -> u64| states.iter().map(limit).max().unwrap_or(0);
        let delay_limit = |state: &Impairments| {
            queue_limit(
                state.delay,
                state.jitter.as_ref().map(|jitter| jitter.amount),
            )
        };
        let draws_before_the_rate = |state: &Impairments| {
            state.loss.is_some() || state.duplicate.is_some() || state.corrupt.is_some()
        };
        Ok(if two_stages {
            Self {
                two_stages,
                seeded_first: states.iter().any(draws_before_the_rate),
                seeded_second: states.iter().any(|state| state.reorder.is_some()),
                first_limit: LINK_QUEUE,
                second_limit: longest(&delay_limit),
                blackout,
            }
        } else {
            Self {
                two_stages,
                seeded_first: states.iter().any(Impairments::is_random),
                seeded_second: false,
                first_limit: longest(&delay_limit),
                second_limit: 0,
                blackout,
            }
        })
    }

    /// This layout with a blackout queue behind its queues, for an outage (D-064).
    pub(crate) fn with_blackout(mut self) -> Self {
        self.blackout = true;
        self
    }

    /// Whether the direction is two or more queues in one tree: the rate in the first and the delay in the second,
    /// or a blackout behind them. Some kernels refuse a duplicating `netem` next to another one (D-052).
    pub(crate) fn is_a_tree(&self) -> bool {
        self.two_stages || self.blackout
    }

    /// The `tc` line that begins (`on`) or ends an outage of this direction on `interface`, or `None` if the
    /// direction has no blackout queue. The blackout is the last queue of the tree.
    pub(crate) fn blackout_line(&self, interface: &str, on: bool) -> Option<String> {
        self.blackout.then(|| {
            let place = if self.two_stages { 2 } else { 1 };
            format!(
                "qdisc change dev {interface} parent {place}:1 handle {}: netem {}",
                place + 1,
                blackout_arguments(on).join(" ")
            )
        })
    }

    /// Whether a state fits the queues of this layout: it needs a second queue only if there is one, and it does not
    /// draw numbers in a queue that was built without a seed for them.
    pub(crate) fn fits(&self, state: &Impairments) -> bool {
        let needs_two = state.rate.is_some() && (state.jitter.is_some() || state.reorder.is_some());
        if needs_two && !self.two_stages {
            return false;
        }
        if self.two_stages {
            let draws_first =
                state.loss.is_some() || state.duplicate.is_some() || state.corrupt.is_some();
            (!draws_first || self.seeded_first) && (state.reorder.is_none() || self.seeded_second)
        } else {
            !state.is_random() || self.seeded_first
        }
    }
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
    let layout = Layout::of(std::slice::from_ref(impairments), false)?;
    build(&layout, impairments, seed, false).map(Some)
}

/// The arguments for building the queues of a layout in `state`, with `seed`.
///
/// # Errors
///
/// Text if a value cannot be applied exactly.
pub(crate) fn create(
    layout: &Layout,
    state: &Impairments,
    seed: Option<u64>,
) -> Result<Stages, String> {
    build(layout, state, seed, false)
}

/// The arguments that write `state` into queues that are built: the whole state, with the settings that `netem`
/// keeps across a change written as zeros where the state has none (D-048: `corrupt`, `reorder`, `rate`; what
/// else a change leaves out is reset by `netem` itself, measured), and no seed, because a change keeps the
/// generator.
///
/// # Errors
///
/// Text if a value cannot be applied exactly.
pub(crate) fn change(layout: &Layout, state: &Impairments) -> Result<Stages, String> {
    build(layout, state, None, true)
}

/// The arguments of the blackout queue: no delay, nothing to draw, and a loss of 100 % while an outage is on and of
/// 0 % while it is not. The state is written whole each time, so nothing of the one before is left.
fn blackout_arguments(on: bool) -> Vec<String> {
    let loss = if on { "100.000000%" } else { "0.000000%" };
    vec![
        "limit".to_owned(),
        LINK_QUEUE.to_string(),
        "loss".to_owned(),
        "random".to_owned(),
        loss.to_owned(),
    ]
}

/// A setting that is written as `zero` when `whole` is asked for and the state has none.
fn or_zero(arguments: Vec<String>, whole: bool, zero: &[&str]) -> Vec<String> {
    if arguments.is_empty() && whole {
        zero.iter().map(|word| (*word).to_owned()).collect()
    } else {
        arguments
    }
}

fn build(
    layout: &Layout,
    state: &Impairments,
    seed: Option<u64>,
    whole: bool,
) -> Result<Stages, String> {
    let limit = |packets: u64| vec!["limit".to_owned(), packets.to_string()];
    let corrupt = |arguments: Vec<String>| or_zero(arguments, whole, &["corrupt", "0%"]);
    let reorder = |arguments: Vec<String>| or_zero(arguments, whole, &["reorder", "0%"]);
    let rate = |arguments: Vec<String>| or_zero(arguments, whole, &["rate", "0bit"]);
    if layout.two_stages {
        // The duplication, the loss and the rate first, then the delay with its jitter and the reordering
        let mut first = limit(layout.first_limit);
        first.extend(loss_arguments(state));
        first.extend(duplicate_arguments(state));
        first.extend(corrupt(corrupt_arguments(state)));
        if layout.seeded_first {
            first.extend(seed_arguments(seed));
        }
        first.extend(rate(rate_arguments(state)));
        let mut second = limit(layout.second_limit);
        second.extend(delay_arguments(state)?);
        second.extend(reorder(reorder_arguments(state)));
        if layout.seeded_second {
            second.extend(seed_arguments(seed.map(|seed| derive_stage_seed(seed, 1))));
        }
        return Stages {
            first,
            second: Some(second),
            blackout: blackout_stage(layout, whole),
        }
        .checked();
    }
    let mut first = limit(layout.first_limit);
    first.extend(delay_arguments(state)?);
    first.extend(loss_arguments(state));
    first.extend(duplicate_arguments(state));
    first.extend(corrupt(corrupt_arguments(state)));
    first.extend(reorder(reorder_arguments(state)));
    if layout.seeded_first {
        first.extend(seed_arguments(seed));
    }
    first.extend(rate(rate_arguments(state)));
    Stages {
        first,
        second: None,
        blackout: blackout_stage(layout, whole),
    }
    .checked()
}

/// The blackout queue when the queues are built, off. A change of the impairments does not write it: an outage that
/// is going on is not the business of a `set`.
fn blackout_stage(layout: &Layout, whole: bool) -> Option<Vec<String>> {
    (layout.blackout && !whole).then(|| blackout_arguments(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscenario_model::Scenario;

    /// A direction with an outage has the blackout queue behind the others, and the lines put it where it belongs:
    /// the child of the only queue, or of the second (D-064).
    #[test]
    fn the_blackout_is_the_last_queue_of_the_tree_and_starts_off() {
        let one = impairments("{ delay: 20ms, loss: { model: random, p: 5% } }");
        let layout = Layout::of(std::slice::from_ref(&one), true).unwrap();
        assert!(layout.is_a_tree());
        let lines = create(&layout, &one, Some(7))
            .unwrap()
            .lines("l0", "replace");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("qdisc replace dev l0 root handle 1: netem limit 3000 delay 20000us loss random 5.000000%"));
        assert_eq!(
            lines[1],
            "qdisc replace dev l0 parent 1:1 handle 2: netem limit 1000 loss random 0.000000%"
        );
        assert_eq!(
            layout.blackout_line("l0", true).unwrap(),
            "qdisc change dev l0 parent 1:1 handle 2: netem limit 1000 loss random 100.000000%"
        );
        assert_eq!(
            layout.blackout_line("l0", false).unwrap(),
            "qdisc change dev l0 parent 1:1 handle 2: netem limit 1000 loss random 0.000000%"
        );

        let two = impairments("{ delay: 20ms, jitter: { amount: 5ms }, rate: 2mbit }");
        let layout = Layout::of(std::slice::from_ref(&two), true).unwrap();
        let lines = create(&layout, &two, None).unwrap().lines("l1", "replace");
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("root handle 1: netem") && lines[0].contains("rate 2000000bit"));
        assert!(lines[1].starts_with("qdisc replace dev l1 parent 1:1 handle 2: netem"));
        assert_eq!(
            lines[2],
            "qdisc replace dev l1 parent 2:1 handle 3: netem limit 1000 loss random 0.000000%"
        );
        assert_eq!(
            layout.blackout_line("l1", true).unwrap(),
            "qdisc change dev l1 parent 2:1 handle 3: netem limit 1000 loss random 100.000000%"
        );
    }

    /// A change of the impairments never writes the blackout: an outage that is going on is not the business of a
    /// `set`, and writing it would end the outage. Canary: building does write it, off.
    #[test]
    fn a_change_of_the_impairments_leaves_the_blackout_alone() {
        let state = impairments("{ delay: 20ms }");
        let layout = Layout::of(std::slice::from_ref(&state), true).unwrap();
        let changed = change(&layout, &state).unwrap();
        assert_eq!(changed.blackout, None);
        assert_eq!(changed.lines("l0", "change").len(), 1);
        assert!(!changed.lines("l0", "change")[0].contains("handle"));
        assert!(create(&layout, &state, None).unwrap().blackout.is_some());
    }

    /// Without an outage there is no blackout queue and nothing about the layout changes.
    #[test]
    fn a_direction_without_an_outage_has_no_blackout_queue() {
        let state = impairments("{ delay: 20ms }");
        let layout = Layout::of(std::slice::from_ref(&state), false).unwrap();
        assert!(!layout.is_a_tree());
        assert_eq!(layout.blackout_line("l0", true), None);
        let lines = create(&layout, &state, None)
            .unwrap()
            .lines("l0", "replace");
        assert_eq!(
            lines,
            ["qdisc replace dev l0 root netem limit 3000 delay 20000us"]
        );
    }

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
