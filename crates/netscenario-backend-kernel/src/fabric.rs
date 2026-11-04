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
    /// For the port of the fabric towards the receiver: the delay, the jitter and the reordering, if the direction has any.
    pub(crate) fabric: Option<String>,
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

/// The lines for `tc` that build a direction with a `queue`: `interface` is the interface of the sender in its node, and
/// `port` the port of the fabric that the packets leave the fabric by.
///
/// # Errors
///
/// Text if a value cannot be applied exactly (a time that is not a whole number of microseconds), or the direction has a
/// `queue` without a `rate`.
pub(crate) fn queue_tree(
    interface: &str,
    port: &str,
    impairments: &Impairments,
    seed: Option<u64>,
) -> Result<QueueTree, String> {
    let queue = impairments
        .queue
        .as_ref()
        .ok_or_else(|| "the direction has no queue".to_owned())?;
    let rate = impairments
        .rate
        .ok_or_else(|| "a queue needs a rate to wait for".to_owned())?
        .bits_per_second();
    // The first `netem`: the stages that come before the rate. It draws numbers for the loss, the duplication and the
    // corruption, so it gets the seed where the direction has one of them.
    let mut first = vec![
        "limit".to_owned(),
        NODE_NETEM_LIMIT.to_string(),
        "delay".to_owned(),
        "1us".to_owned(),
    ];
    first.extend(netem::loss_arguments(impairments));
    first.extend(netem::duplicate_arguments(impairments));
    first.extend(netem::corrupt_arguments(impairments));
    if impairments.loss.is_some()
        || impairments.duplicate.is_some()
        || impairments.corrupt.is_some()
    {
        first.extend(netem::seed_arguments(seed));
    }
    let (kind, arguments) = leaf(queue)?;
    let mut node = vec![
        format!(
            "qdisc replace dev {interface} root handle 1: netem {}",
            first.join(" ")
        ),
        format!("qdisc replace dev {interface} parent 1:1 handle 2: htb default 1 r2q 10"),
        // An `htb` class with the same `rate` and `ceil` is a shaper that keeps its tokens when the rate is changed (S19); the
        // burst is small so that the rate holds at the scale of a few packets, and the quantum is a frame
        format!(
            "class replace dev {interface} parent 2: classid 2:1 htb rate {rate}bit ceil {rate}bit burst 1600 cburst 1600 quantum 1514"
        ),
    ];
    let mut leaf_line =
        format!("qdisc replace dev {interface} parent 2:1 handle {LEAF_HANDLE} {kind}");
    for argument in arguments {
        leaf_line.push(' ');
        leaf_line.push_str(&argument);
    }
    node.push(leaf_line);
    // The delay with its jitter and the reordering come after the queue, in the fabric (D-079)
    let delays = impairments.delay.is_some() || impairments.reorder.is_some();
    let fabric = if delays {
        let mut words = vec![
            "limit".to_owned(),
            netem::queue_limit(
                impairments.delay,
                impairments.jitter.as_ref().map(|jitter| jitter.amount),
            )
            .to_string(),
        ];
        words.extend(netem::delay_arguments(impairments)?);
        words.extend(netem::reorder_arguments(impairments));
        if impairments.jitter.is_some() || impairments.reorder.is_some() {
            words.extend(netem::seed_arguments(
                seed.map(|seed| derive_stage_seed(seed, 1)),
            ));
        }
        Some(format!(
            "qdisc replace dev {port} root netem {}",
            words.join(" ")
        ))
    } else {
        None
    };
    Ok(QueueTree { node, fabric })
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
        assert_eq!(found.fabric, None, "nothing comes after the queue");
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
            jitter: Some(netscenario_model::Jitter {
                amount: "5ms".parse().unwrap(),
                distribution: netscenario_model::JitterDistribution::Normal,
            }),
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
        let fabric = found.fabric.unwrap();
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
}
