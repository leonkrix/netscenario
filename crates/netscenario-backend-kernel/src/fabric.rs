//! The trees of a link that is built through the fabric (D-079, D-082).
//!
//! A direction with a `queue` leaves its node through a `netem` with a delay of one microsecond, which holds the duplication, the
//! loss and the corruption, and frees the sending socket (without a delay the queue below would push back on the sender: the
//! kernel releases the packet only when a `netem` has a delay, a jitter or a rate). Below it is an `htb` class with the rate, and
//! the discipline is the leaf of the class. The packet then crosses the fabric, where the egress queue towards the other node
//! holds the delay, the jitter and the reordering. The order of the stages is the one of the direct build: loss, then the rate
//! with its queue, then the delay.
//!
//! These are lines for `tc`, made by functions that run nothing, so a test can say exactly what is asked of the kernel.

use netscenario_core::derive_stage_seed;
use netscenario_model::{Discipline, Duration, Impairments, Queue, QueueLimit};

use crate::netem;

/// The packets that the first `netem` of a tree may hold while it waits one microsecond for each of them.
const NODE_NETEM_LIMIT: u64 = 1000;

/// The handle of the leaf of the class: the qdisc whose counters are the statistics of the queue (PRB-10).
pub(crate) const LEAF_HANDLE: &str = "3:";

/// The `tc` lines of one direction: the tree in the node of the sender, and the queue in the fabric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueTree {
    /// For the interface of the sender in its node: the `netem`, the `htb`, its class and the leaf, in this order.
    pub(crate) node: Vec<String>,
    /// For the port of the fabric towards the receiver, the root first: the delay, the jitter and the reordering if the direction
    /// has any, and the blackout of an outage behind them.
    pub(crate) fabric: Vec<String>,
}

/// A time as `tc` takes it, in whole microseconds.
fn time(value: Duration) -> Result<String, String> {
    netem::microseconds(value)
}

/// The leaf of the class: the discipline with the parameters the scenario wrote and the defaults of the kernel for the rest.
/// The name of the kernel's discipline and its arguments.
fn leaf(queue: &Queue) -> Result<(&'static str, Vec<String>), String> {
    let words = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    let mut arguments = Vec::new();
    let ecn = |arguments: &mut Vec<String>| {
        if let Some(on) = queue.ecn {
            arguments.push(if on { "ecn" } else { "noecn" }.to_owned());
        }
    };
    let kind = match queue.discipline {
        Discipline::Fifo => match queue.limit {
            Some(QueueLimit::Bytes(size)) => {
                arguments.extend(words(&["limit"]));
                arguments.push(size.bytes().to_string());
                "bfifo"
            }
            Some(QueueLimit::Packets(packets)) => {
                arguments.extend(words(&["limit"]));
                arguments.push(packets.to_string());
                "pfifo"
            }
            // The kernel takes the length of the transmit queue of the interface, 1000 on a veth; written out so that the
            // queue does not depend on it
            None => {
                arguments.extend(words(&["limit", "1000"]));
                "pfifo"
            }
        },
        Discipline::Codel | Discipline::FqCodel | Discipline::Pie => {
            if let Some(QueueLimit::Packets(packets)) = queue.limit {
                arguments.extend(words(&["limit"]));
                arguments.push(packets.to_string());
            }
            if queue.discipline == Discipline::FqCodel {
                if let Some(flows) = queue.flows {
                    arguments.extend(words(&["flows"]));
                    arguments.push(flows.to_string());
                }
                if let Some(quantum) = queue.quantum {
                    arguments.extend(words(&["quantum"]));
                    arguments.push(quantum.bytes().to_string());
                }
            }
            if let Some(target) = queue.target {
                arguments.extend(words(&["target"]));
                arguments.push(time(target)?);
            }
            if queue.discipline == Discipline::Pie {
                if let Some(update) = queue.t_update {
                    arguments.extend(words(&["tupdate"]));
                    arguments.push(time(update)?);
                }
            } else if let Some(interval) = queue.interval {
                arguments.extend(words(&["interval"]));
                arguments.push(time(interval)?);
            }
            ecn(&mut arguments);
            match queue.discipline {
                Discipline::Codel => "codel",
                Discipline::FqCodel => "fq_codel",
                _ => "pie",
            }
        }
    };
    Ok((kind, arguments))
}

/// The rate of a direction whose rate is not set (yet or any more): a shaper that does not hold anything back.
const UNLIMITED_BITS: u64 = 100_000_000_000;

/// The qdisc of the fabric port that holds the blackout of an outage, as the child of the delaying `netem`.
const PORT_BLACKOUT: &str = "parent 1:1 handle 2:";

/// What the tree of a direction has to be able to do, from every state it goes through: the stages that exist from the start
/// to the end of the run, because a stage that is not there cannot be added without building the queue again, which drops the
/// packets in it and starts its random numbers over (D-059). The plan of a direction that never changes is made from its one
/// state.
// The flags are independent facts about the stages of the tree, not the states of one thing
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuePlan {
    /// Whether some state has a loss, a duplication or a corruption: the `netem` of the node is seeded when it is built, because a
    /// change keeps its generator.
    seeded_node: bool,
    /// Whether the fabric holds a `netem` for the delay, the jitter and the reordering: some state has one of them, or the
    /// direction has an outage.
    delaying: bool,
    /// Whether some state has a jitter or a reordering: the `netem` of the fabric draws numbers.
    seeded_fabric: bool,
    /// The longest queue that any state needs in the fabric, so that a change never makes it shorter than what it holds.
    fabric_limit: u64,
    /// Whether the port has a blackout behind the delay, for an outage (D-064).
    blackout: bool,
    /// Whether some state has a rate: without one a direction with a queue has nothing to wait for.
    any_rate: bool,
}

/// The lines for `tc` that change a built tree from one state to another: for the node, and for the port of the fabric.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TreeStep {
    /// For the interface of the sender in its node.
    pub(crate) node: Vec<String>,
    /// For the port of the fabric towards the receiver.
    pub(crate) fabric: Vec<String>,
}

impl QueuePlan {
    /// The plan for the states of a direction.
    ///
    /// # Errors
    ///
    /// Text if the states cannot be had in one tree: a jitter with a normal distribution in one and a uniform one in another.
    pub(crate) fn of(states: &[Impairments]) -> Result<Self, String> {
        netem::Layout::of(states, false)?;
        let delay_limit = |state: &Impairments| {
            netem::queue_limit(
                state.delay,
                state.jitter.as_ref().map(|jitter| jitter.amount),
            )
        };
        Ok(Self {
            seeded_node: states.iter().any(|state| {
                state.loss.is_some() || state.duplicate.is_some() || state.corrupt.is_some()
            }),
            delaying: states
                .iter()
                .any(|state| state.delay.is_some() || state.reorder.is_some()),
            seeded_fabric: states
                .iter()
                .any(|state| state.jitter.is_some() || state.reorder.is_some()),
            fabric_limit: states.iter().map(delay_limit).max().unwrap_or(0),
            blackout: false,
            any_rate: states.iter().any(|state| state.rate.is_some()),
        })
    }

    /// This plan with a blackout behind the delay, for an outage.
    pub(crate) fn with_blackout(mut self) -> Self {
        self.blackout = true;
        self.delaying = true;
        self
    }

    /// Whether a state fits the tree: a delay or a reordering needs the `netem` of the fabric, which is there only if the plan
    /// had a state with one.
    pub(crate) fn fits(&self, state: &Impairments) -> bool {
        (state.delay.is_none() && state.reorder.is_none()) || self.delaying
    }

    /// The first `netem` of the node: the stages before the rate. `whole` writes what a change keeps across it as zeros (D-048).
    fn node_netem(&self, state: &Impairments, seed: Option<u64>, whole: bool) -> Vec<String> {
        let mut words = vec![
            "limit".to_owned(),
            NODE_NETEM_LIMIT.to_string(),
            "delay".to_owned(),
            "1us".to_owned(),
        ];
        words.extend(netem::loss_arguments(state));
        words.extend(netem::duplicate_arguments(state));
        let corrupt = netem::corrupt_arguments(state);
        if corrupt.is_empty() && whole {
            words.extend(["corrupt".to_owned(), "0%".to_owned()]);
        } else {
            words.extend(corrupt);
        }
        if self.seeded_node {
            words.extend(netem::seed_arguments(seed));
        }
        words
    }

    /// The `netem` of the fabric port: the delay with its jitter and the reordering.
    fn port_netem(
        &self,
        state: &Impairments,
        seed: Option<u64>,
        whole: bool,
    ) -> Result<Vec<String>, String> {
        let mut words = vec!["limit".to_owned(), self.fabric_limit.to_string()];
        words.extend(netem::delay_arguments(state)?);
        let reorder = netem::reorder_arguments(state);
        if reorder.is_empty() && whole {
            words.extend(["reorder".to_owned(), "0%".to_owned()]);
        } else {
            words.extend(reorder);
        }
        if self.seeded_fabric {
            words.extend(netem::seed_arguments(
                seed.map(|seed| derive_stage_seed(seed, 1)),
            ));
        }
        Ok(words)
    }

    /// The class of the shaper at the rate of `state`.
    fn class_line(interface: &str, verb: &str, state: &Impairments) -> String {
        let rate = state
            .rate
            .map_or(UNLIMITED_BITS, netscenario_model::Rate::bits_per_second);
        // An `htb` class with the same `rate` and `ceil` is a shaper that keeps its tokens when the rate is changed (S19); the
        // burst is small so that the rate holds at the scale of a few packets, and the quantum is a frame
        format!(
            "class {verb} dev {interface} parent 2: classid 2:1 htb rate {rate}bit ceil {rate}bit burst 1600 cburst 1600 quantum 1514"
        )
    }

    /// The lines that build the tree of a direction in `state`: `interface` is the interface of the sender in its node, and `port`
    /// the port of the fabric that the packets leave the fabric by.
    ///
    /// # Errors
    ///
    /// Text if a value cannot be applied exactly (a time that is not a whole number of microseconds), or no state has a rate.
    pub(crate) fn build(
        &self,
        interface: &str,
        port: &str,
        state: &Impairments,
        seed: Option<u64>,
    ) -> Result<QueueTree, String> {
        if !self.any_rate {
            return Err("a queue needs a rate to wait for".to_owned());
        }
        let default = Queue::default();
        let (kind, arguments) = leaf(state.queue.as_ref().unwrap_or(&default))?;
        let mut node = vec![
            format!(
                "qdisc replace dev {interface} root handle 1: netem {}",
                self.node_netem(state, seed, false).join(" ")
            ),
            format!("qdisc replace dev {interface} parent 1:1 handle 2: htb default 1 r2q 10"),
            Self::class_line(interface, "replace", state),
        ];
        let mut leaf_line =
            format!("qdisc replace dev {interface} parent 2:1 handle {LEAF_HANDLE} {kind}");
        for argument in arguments {
            leaf_line.push(' ');
            leaf_line.push_str(&argument);
        }
        node.push(leaf_line);
        // The delay with its jitter and the reordering come after the queue, in the fabric (D-079), with the blackout of an
        // outage behind them
        let mut fabric = Vec::new();
        if self.delaying {
            let root = if self.blackout {
                "root handle 1:"
            } else {
                "root"
            };
            fabric.push(format!(
                "qdisc replace dev {port} {root} netem {}",
                self.port_netem(state, seed, false)?.join(" ")
            ));
            if self.blackout {
                fabric.push(Self::blackout(port, "replace", false));
            }
        }
        Ok(QueueTree { node, fabric })
    }

    /// The lines that change the tree from `from` to `to`: only the parts whose values differ, each written whole (D-048), and
    /// no seed, because a change keeps the generator. The leaf is not touched: the queue and what it holds stay.
    ///
    /// # Errors
    ///
    /// Text if a value cannot be applied exactly.
    pub(crate) fn step(
        &self,
        interface: &str,
        port: &str,
        from: &Impairments,
        to: &Impairments,
    ) -> Result<TreeStep, String> {
        let mut step = TreeStep::default();
        if (&from.loss, &from.duplicate, &from.corrupt) != (&to.loss, &to.duplicate, &to.corrupt) {
            step.node.push(format!(
                "qdisc change dev {interface} root handle 1: netem {}",
                self.node_netem(to, None, true).join(" ")
            ));
        }
        if from.rate != to.rate {
            step.node.push(Self::class_line(interface, "change", to));
        }
        if self.delaying
            && (&from.delay, &from.jitter, &from.reorder) != (&to.delay, &to.jitter, &to.reorder)
        {
            let root = if self.blackout {
                "root handle 1:"
            } else {
                "root"
            };
            step.fabric.push(format!(
                "qdisc change dev {port} {root} netem {}",
                self.port_netem(to, None, true)?.join(" ")
            ));
        }
        Ok(step)
    }

    /// The line that begins (`on`) or ends an outage on `port`: the blackout is a `netem` behind the delay that loses everything
    /// while it is on.
    pub(crate) fn blackout_line(&self, port: &str, on: bool) -> Option<String> {
        self.blackout.then(|| Self::blackout(port, "change", on))
    }

    fn blackout(port: &str, verb: &str, on: bool) -> String {
        let loss = if on { "100.000000%" } else { "0.000000%" };
        format!(
            "qdisc {verb} dev {port} {PORT_BLACKOUT} netem limit {NODE_NETEM_LIMIT} loss random {loss}"
        )
    }
}

/// The lines for `tc` that build a direction with a `queue` that does not change: `interface` is the interface of the sender in
/// its node, and `port` the port of the fabric that the packets leave the fabric by.
///
/// # Errors
///
/// Text if a value cannot be applied exactly (a time that is not a whole number of microseconds), or the direction has a
/// `queue` without a `rate`.
#[cfg(test)]
pub(crate) fn queue_tree(
    interface: &str,
    port: &str,
    impairments: &Impairments,
    seed: Option<u64>,
) -> Result<QueueTree, String> {
    if impairments.queue.is_none() {
        return Err("the direction has no queue".to_owned());
    }
    QueuePlan::of(std::slice::from_ref(impairments))?.build(interface, port, impairments, seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscenario_model::{Loss, Rate, Reorder};

    fn impairments(queue: Queue) -> Impairments {
        Impairments {
            rate: Some(Rate::from_bits_per_second(2_000_000).unwrap()),
            queue: Some(queue),
            ..Impairments::default()
        }
    }

    fn tree(impairments: &Impairments, seed: Option<u64>) -> QueueTree {
        queue_tree("l0", "nsf0b", impairments, seed).unwrap()
    }

    /// A `fifo` without parameters: the `netem` that frees the sender, the shaper at the rate, and a queue of the length the
    /// kernel gives an interface, written out.
    #[test]
    fn a_plain_queue_is_the_netem_of_one_microsecond_the_shaper_and_a_fifo() {
        let found = tree(&impairments(Queue::default()), None);
        assert_eq!(
            found.node,
            [
                "qdisc replace dev l0 root handle 1: netem limit 1000 delay 1us",
                "qdisc replace dev l0 parent 1:1 handle 2: htb default 1 r2q 10",
                "class replace dev l0 parent 2: classid 2:1 htb rate 2000000bit ceil 2000000bit burst 1600 cburst 1600 quantum 1514",
                "qdisc replace dev l0 parent 2:1 handle 3: pfifo limit 1000"
            ]
        );
        assert!(found.fabric.is_empty(), "nothing comes after the queue");
    }

    #[test]
    fn the_limit_of_a_fifo_is_packets_or_bytes() {
        let leaf_of = |limit| {
            let queue = Queue {
                limit: Some(limit),
                ..Queue::default()
            };
            tree(&impairments(queue), None).node.pop().unwrap()
        };
        assert!(leaf_of(QueueLimit::Packets(50)).ends_with("handle 3: pfifo limit 50"));
        assert!(
            leaf_of(QueueLimit::Bytes(netscenario_model::Size::from_bytes(
                64_000
            )))
            .ends_with("handle 3: bfifo limit 64000")
        );
    }

    /// The disciplines with the parameters of the scenario, in the order `tc` is documented in, and nothing that was not written.
    #[test]
    fn the_disciplines_are_the_leaf_with_the_parameters_that_were_written() {
        let ms = |text: &str| text.parse::<Duration>().unwrap();
        let codel = Queue {
            discipline: Discipline::Codel,
            limit: Some(QueueLimit::Packets(200)),
            target: Some(ms("5ms")),
            interval: Some(ms("100ms")),
            ecn: Some(true),
            ..Queue::default()
        };
        assert!(
            tree(&impairments(codel), None).node[3].ends_with(
                "parent 2:1 handle 3: codel limit 200 target 5000us interval 100000us ecn"
            )
        );
        let fq = Queue {
            discipline: Discipline::FqCodel,
            flows: Some(64),
            quantum: Some(netscenario_model::Size::from_bytes(1514)),
            ecn: Some(false),
            ..Queue::default()
        };
        assert!(
            tree(&impairments(fq), None).node[3]
                .ends_with("parent 2:1 handle 3: fq_codel flows 64 quantum 1514 noecn")
        );
        let pie = Queue {
            discipline: Discipline::Pie,
            target: Some(ms("15ms")),
            t_update: Some(ms("15ms")),
            ..Queue::default()
        };
        assert!(
            tree(&impairments(pie), None).node[3]
                .ends_with("parent 2:1 handle 3: pie target 15000us tupdate 15000us")
        );
        // A discipline without a parameter is the discipline with the defaults of the kernel
        assert!(
            tree(&impairments(Queue::of(Discipline::Codel)), None).node[3]
                .ends_with("parent 2:1 handle 3: codel")
        );
    }

    /// The loss, the duplication and the corruption stay in front of the rate, with the seed, and the delay, the jitter and the
    /// reordering go to the fabric with a seed of their own: the order of the stages of the direct build (D-030).
    #[test]
    fn the_stages_keep_their_order_loss_in_front_of_the_queue_and_the_delay_behind_it() {
        let state = Impairments {
            delay: Some("20ms".parse().unwrap()),
            jitter: Some(netscenario_model::Jitter::new(
                "5ms".parse().unwrap(),
                netscenario_model::JitterDistribution::Normal,
            )),
            loss: Some(Loss::Random {
                p: "1%".parse().unwrap(),
            }),
            reorder: Some(Reorder {
                share: "10%".parse().unwrap(),
                gap: Some(3),
            }),
            ..impairments(Queue::of(Discipline::Codel))
        };
        let found = tree(&state, Some(7));
        assert_eq!(
            found.node[0],
            "qdisc replace dev l0 root handle 1: netem limit 1000 delay 1us loss random 1.000000% seed 7"
        );
        let fabric = found.fabric[0].clone();
        assert!(
            fabric.starts_with("qdisc replace dev nsf0b root netem limit ")
                && fabric.contains(
                    " delay 20000us 5000us distribution normal reorder 10.000000% gap 3 seed "
                ),
            "{fabric}"
        );
        // The seed of the fabric is not the seed of the node: two queues must not draw the same numbers
        assert!(!fabric.ends_with("seed 7"), "{fabric}");
        // No loss, no seed on the first netem
        let found = tree(&impairments(Queue::default()), Some(7));
        assert!(!found.node[0].contains("seed"), "{}", found.node[0]);
    }

    /// Canary: a time that is not a whole number of microseconds is refused and not rounded, and a queue without a rate is not
    /// built.
    #[test]
    fn what_cannot_be_applied_exactly_is_refused() {
        let queue = Queue {
            discipline: Discipline::Codel,
            target: Some("1500ns".parse().unwrap()),
            ..Queue::default()
        };
        assert!(queue_tree("l0", "p", &impairments(queue), None).is_err());
        let no_rate = Impairments {
            queue: Some(Queue::default()),
            ..Impairments::default()
        };
        assert!(queue_tree("l0", "p", &no_rate, None).is_err());
    }

    fn plan_of(states: &[Impairments]) -> QueuePlan {
        QueuePlan::of(states).unwrap()
    }

    fn at_rate(bits: u64) -> Impairments {
        Impairments {
            rate: Rate::from_bits_per_second(bits),
            ..Impairments::default()
        }
    }

    /// A rate that changes is a tree whose class is changed and nothing else: the `netem` of the node keeps its generator, the
    /// leaf keeps its packets (D-091).
    #[test]
    fn a_step_of_the_rate_changes_the_class_and_nothing_else() {
        let (low, high) = (at_rate(2_000_000), at_rate(8_000_000));
        let plan = plan_of(&[low.clone(), high.clone()]);
        let step = plan.step("l0", "p", &low, &high).unwrap();
        assert_eq!(
            step.node,
            [
                "class change dev l0 parent 2: classid 2:1 htb rate 8000000bit ceil 8000000bit burst 1600 cburst 1600 quantum 1514"
            ]
        );
        assert_eq!(step.fabric, Vec::<String>::new());
        // Canary: the same state again changes nothing
        assert_eq!(
            plan.step("l0", "p", &high, &high).unwrap(),
            TreeStep::default()
        );
    }

    /// The parts that differ are written whole, with what a change keeps across it as zeros (D-048).
    #[test]
    fn a_step_writes_the_whole_part_with_zeros_for_what_a_change_keeps() {
        let first = Impairments {
            corrupt: Some("1%".parse().unwrap()),
            reorder: Some(Reorder {
                share: "10%".parse().unwrap(),
                gap: None,
            }),
            delay: Some("20ms".parse().unwrap()),
            ..at_rate(2_000_000)
        };
        let second = Impairments {
            delay: Some("30ms".parse().unwrap()),
            ..at_rate(2_000_000)
        };
        let plan = plan_of(&[first.clone(), second.clone()]);
        let step = plan.step("l0", "p", &first, &second).unwrap();
        assert_eq!(
            step.node,
            ["qdisc change dev l0 root handle 1: netem limit 1000 delay 1us corrupt 0%"]
        );
        assert_eq!(step.fabric.len(), 1);
        assert!(
            step.fabric[0].starts_with("qdisc change dev p root netem limit ")
                && step.fabric[0].contains(" delay 30000us ")
                && step.fabric[0].ends_with(" reorder 0%"),
            "{:?}",
            step.fabric
        );
        // No seed in a change: the generator goes on
        assert!(!step.fabric[0].contains("seed"));
    }

    /// The stages that a later state needs exist from the start: the node is seeded when only a later state has a loss, and a
    /// direction that gets a delay later has the delaying `netem` in the fabric from the start.
    #[test]
    fn the_stages_of_every_state_are_built_at_the_start() {
        let lossy = Impairments {
            loss: Some(Loss::Random {
                p: "1%".parse().unwrap(),
            }),
            delay: Some("10ms".parse().unwrap()),
            ..at_rate(2_000_000)
        };
        let plan = plan_of(&[at_rate(2_000_000), lossy]);
        let built = plan.build("l0", "p", &at_rate(2_000_000), Some(7)).unwrap();
        assert!(built.node[0].ends_with("seed 7"), "{:?}", built.node);
        assert_eq!(built.fabric.len(), 1, "the delaying netem is there");
        let without = plan_of(&[at_rate(1_000_000), at_rate(2_000_000)]);
        assert!(!without.fits(&Impairments {
            delay: Some("1ms".parse().unwrap()),
            ..at_rate(1_000_000)
        }));
    }

    /// A state with no rate is a class that holds nothing back, and an outage is a blackout behind the delay on the port.
    #[test]
    fn no_rate_is_an_open_class_and_an_outage_is_a_blackout_on_the_port() {
        let plan = plan_of(&[at_rate(2_000_000), Impairments::default()]).with_blackout();
        let step = plan
            .step("l0", "p", &at_rate(2_000_000), &Impairments::default())
            .unwrap();
        assert!(step.node[0].contains("htb rate 100000000000bit ceil 100000000000bit"));
        let built = plan.build("l0", "p", &at_rate(2_000_000), None).unwrap();
        assert_eq!(
            built.fabric,
            [
                "qdisc replace dev p root handle 1: netem limit 1000",
                "qdisc replace dev p parent 1:1 handle 2: netem limit 1000 loss random 0.000000%"
            ]
        );
        assert_eq!(
            plan.blackout_line("p", true).unwrap(),
            "qdisc change dev p parent 1:1 handle 2: netem limit 1000 loss random 100.000000%"
        );
        // Canary: a plan without an outage has no blackout line
        assert_eq!(plan_of(&[at_rate(1)]).blackout_line("p", true), None);
    }
}
