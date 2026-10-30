//! The rules that the types cannot state: names that must exist, ranges, and what the Kernel backend can apply
//! exactly. Every problem names the path of the value in the scenario.

use crate::error::Problem;
use crate::scenario::{Impairments, Jitter, JitterDistribution, Scenario};
use crate::suggest::closest;
use crate::units::{Duration, Name};

/// The packets per second that a workload may send: what the agent was measured to deliver (spike S8).
pub const MAX_PACKETS_PER_SECOND: u64 = 100_000;
/// The smallest payload: the header of the agent.
pub const MIN_PACKET_SIZE: u64 = 16;
/// The largest payload that fits the MTU of 1500 without fragmentation (IPv4 and UDP headers take 28 bytes).
pub const MAX_PACKET_SIZE: u64 = 1472;

impl Scenario {
    /// Checks the rules that the types cannot state and returns every problem found (none if the scenario is
    /// valid). A scenario that was read from a file has been through this check already; one that is built in
    /// code should be checked before it is run.
    #[must_use]
    pub fn validate(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        if self.duration.is_zero() {
            problems
                .push(Problem::new("the duration must be greater than zero").at_path("duration"));
        }
        let before = problems.len();
        let nodes = self.check_nodes(&mut problems);
        // A wrong list of nodes would make every reference to a node a second problem
        let nodes_ok = problems.len() == before;
        self.check_links(&nodes, nodes_ok, &mut problems);
        self.check_workloads(&nodes, nodes_ok, &mut problems);
        problems
    }

    fn check_nodes(&self, problems: &mut Vec<Problem>) -> Vec<Name> {
        let nodes = &self.topology.nodes;
        if nodes.len() != 2 {
            problems.push(
                Problem::new(format!(
                    "release 0.1 needs exactly two nodes, found {}",
                    nodes.len()
                ))
                .at_path("topology.nodes")
                .with_help("more nodes (star, chain, dumbbell) arrive with release 0.3"),
            );
        }
        for (index, node) in nodes.iter().enumerate() {
            if nodes[..index].contains(node) {
                problems.push(
                    Problem::new(format!("the node name '{node}' is used twice"))
                        .at_path(format!("topology.nodes[{index}]"))
                        .with_help("every node needs its own name"),
                );
            }
        }
        nodes.clone()
    }

    fn check_links(&self, nodes: &[Name], nodes_ok: bool, problems: &mut Vec<Problem>) {
        let links = &self.topology.links;
        if links.len() != 1 {
            problems.push(
                Problem::new(format!(
                    "release 0.1 needs exactly one link, found {}",
                    links.len()
                ))
                .at_path("topology.links")
                .with_help("more links arrive with release 0.3"),
            );
        }
        for (index, link) in links.iter().enumerate() {
            let base = format!("topology.links[{index}]");
            if links[..index].iter().any(|other| other.id == link.id) {
                problems.push(
                    Problem::new(format!("the link name '{}' is used twice", link.id))
                        .at_path(format!("{base}.id")),
                );
            }
            let [first, second] = &link.between;
            if first == second {
                problems.push(
                    Problem::new(format!(
                        "a link needs two different nodes, both are '{first}'"
                    ))
                    .at_path(format!("{base}.between[1]")),
                );
            }
            for (side, node) in link.between.iter().enumerate() {
                if nodes_ok && !nodes.contains(node) {
                    problems.push(unknown_node(node, nodes, format!("{base}.between[{side}]")));
                }
            }
            if link.both.is_some() && (link.forward.is_some() || link.reverse.is_some()) {
                problems.push(
                    Problem::new("`both` cannot be combined with `forward` or `reverse`")
                        .at_path(format!("{base}.both"))
                        .with_help("use `both` for the same values in both directions, or `forward` and `reverse` for different ones"),
                );
            }
            for (name, impairments) in [
                ("forward", &link.forward),
                ("reverse", &link.reverse),
                ("both", &link.both),
            ] {
                if let Some(impairments) = impairments {
                    check_impairments(impairments, &format!("{base}.{name}"), problems);
                }
            }
        }
    }

    fn check_workloads(&self, nodes: &[Name], nodes_ok: bool, problems: &mut Vec<Problem>) {
        if self.workloads.is_empty() {
            problems.push(
                Problem::new("a scenario needs at least one workload, found none")
                    .at_path("workloads")
                    .with_help("add a workload of kind udp-cbr"),
            );
        }
        for (index, workload) in self.workloads.iter().enumerate() {
            let base = format!("workloads[{index}]");
            if self.workloads[..index]
                .iter()
                .any(|other| other.id == workload.id)
            {
                problems.push(
                    Problem::new(format!("the workload name '{}' is used twice", workload.id))
                        .at_path(format!("{base}.id")),
                );
            }
            for (key, node) in [("from", &workload.from), ("to", &workload.to)] {
                if nodes_ok && !nodes.contains(node) {
                    problems.push(unknown_node(node, nodes, format!("{base}.{key}")));
                }
            }
            if workload.from == workload.to {
                problems.push(
                    Problem::new(format!(
                        "a workload needs two different nodes, both are '{}'",
                        workload.from
                    ))
                    .at_path(format!("{base}.to")),
                );
            }
            let size = workload.packet_size.bytes();
            if !(MIN_PACKET_SIZE..=MAX_PACKET_SIZE).contains(&size) {
                problems.push(
                    Problem::new(format!(
                        "the packet size {size} bytes is outside {MIN_PACKET_SIZE} to {MAX_PACKET_SIZE} bytes"
                    ))
                    .at_path(format!("{base}.packet-size"))
                    .with_help(format!(
                        "the smallest payload is the header of the agent ({MIN_PACKET_SIZE} bytes); the largest fits the MTU of 1500 without fragmentation ({MAX_PACKET_SIZE} bytes)"
                    )),
                );
            } else if u128::from(workload.rate.bits_per_second())
                > u128::from(MAX_PACKETS_PER_SECOND) * u128::from(size) * 8
            {
                let per_second =
                    u128::from(workload.rate.bits_per_second()) / (u128::from(size) * 8);
                problems.push(
                    Problem::new(format!(
                        "the rate {} with packets of {} is {per_second} packets per second; at most {MAX_PACKETS_PER_SECOND} are supported",
                        workload.rate, workload.packet_size
                    ))
                    .at_path(format!("{base}.rate"))
                    .with_help("use bigger packets or a lower rate: the agent was measured up to that many packets per second"),
                );
            }
            if !self.duration.is_zero() {
                check_window(
                    self.duration,
                    workload.start,
                    workload.stop,
                    &base,
                    problems,
                );
            }
        }
    }
}

fn unknown_node(node: &Name, nodes: &[Name], path: String) -> Problem {
    let known: Vec<&str> = nodes.iter().map(Name::as_str).collect();
    let list = format!("the nodes are: {}", known.join(", "));
    let help = match closest(node.as_str(), &known) {
        Some(near) => format!("did you mean '{near}'? {list}"),
        None => list,
    };
    Problem::new(format!("the node '{node}' does not exist"))
        .at_path(path)
        .with_help(help)
}

fn check_window(
    duration: Duration,
    start: Option<Duration>,
    stop: Option<Duration>,
    base: &str,
    problems: &mut Vec<Problem>,
) {
    let start_value = start.unwrap_or(Duration::from_nanos(0));
    let stop_value = stop.unwrap_or(duration);
    if stop_value > duration {
        problems.push(
            Problem::new(format!(
                "`stop` ({stop_value}) is after the end of the measured phase ({duration})"
            ))
            .at_path(format!("{base}.stop")),
        );
    }
    if start_value >= stop_value.min(duration) {
        let path = if stop.is_some() {
            format!("{base}.stop")
        } else {
            format!("{base}.start")
        };
        problems.push(
            Problem::new(format!(
                "`start` ({start_value}) must be before `stop` ({})",
                stop_value.min(duration)
            ))
            .at_path(path),
        );
    }
}

fn check_impairments(impairments: &Impairments, base: &str, problems: &mut Vec<Problem>) {
    let Some(jitter) = &impairments.jitter else {
        return;
    };
    let path = format!("{base}.jitter");
    let Some(delay) = impairments.delay else {
        problems.push(
            Problem::new("`jitter` needs a `delay` to vary")
                .at_path(path)
                .with_help("add `delay` next to `jitter`"),
        );
        return;
    };
    problems.extend(check_jitter(jitter, delay, &path));
}

/// A delay below zero cannot exist, and `netem` would silently clamp it. The format refuses a jitter that
/// reaches below zero instead.
fn check_jitter(jitter: &Jitter, delay: Duration, path: &str) -> Option<Problem> {
    match jitter.distribution {
        JitterDistribution::Uniform if jitter.amount > delay => Some(
            Problem::new(format!(
                "the jitter ({}) is longer than the delay ({delay}): some delays would be below zero",
                jitter.amount
            ))
            .at_path(format!("{path}.amount"))
            .with_help("use a jitter that is at most the delay"),
        ),
        JitterDistribution::Normal if delay.as_nanos() / 4 < jitter.amount.as_nanos() => Some(
            Problem::new(format!(
                "with a normal distribution the delay ({delay}) must be at least four times the jitter ({}): some delays would be below zero",
                jitter.amount
            ))
            .at_path(format!("{path}.amount"))
            .with_help("the normal distribution of netem reaches about four standard deviations: use a smaller jitter or a longer delay"),
        ),
        _ => None,
    }
}
