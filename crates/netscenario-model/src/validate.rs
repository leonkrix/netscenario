//! The rules that the types cannot state: names that must exist, ranges, and what the Kernel backend can apply
//! exactly. Every problem names the path of the value in the scenario.

use crate::error::Problem;
use crate::scenario::{
    Change, Direction, Event, Impairments, Jitter, JitterDistribution, Link, Loss, Scenario,
    UdpCbr, Workload,
};
use crate::suggest::closest;
use crate::units::{Duration, Name, Probability};

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
        self.check_timeline(&mut problems);
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
                    .with_help("add a workload of kind udp-cbr, or a command"),
            );
        }
        for (index, workload) in self.workloads.iter().enumerate() {
            let base = format!("workloads[{index}]");
            if self.workloads[..index]
                .iter()
                .any(|other| other.id() == workload.id())
            {
                problems.push(
                    Problem::new(format!(
                        "the workload name '{}' is used twice",
                        workload.id()
                    ))
                    .at_path(format!("{base}.id")),
                );
            }
            match workload {
                Workload::UdpCbr(workload) => {
                    self.check_udp_cbr(workload, &base, nodes, nodes_ok, problems);
                }
                Workload::Command(command) => {
                    crate::command::check(command, &base, nodes, nodes_ok, problems);
                }
            }
        }
    }

    fn check_udp_cbr(
        &self,
        workload: &UdpCbr,
        base: &str,
        nodes: &[Name],
        nodes_ok: bool,
        problems: &mut Vec<Problem>,
    ) {
        {
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
                check_window(self.duration, workload.start, workload.stop, base, problems);
            }
        }
    }
}

impl Scenario {
    /// The events: each one inside the measured phase, on a link that exists, doing exactly one thing, and
    /// every state that the steps lead to a valid set of impairments (D-050).
    fn check_timeline(&self, problems: &mut Vec<Problem>) {
        let ids: Vec<&str> = self
            .topology
            .links
            .iter()
            .map(|link| link.id.as_str())
            .collect();
        for (index, event) in self.timeline.iter().enumerate() {
            let base = format!("timeline[{index}]");
            if !self.duration.is_zero() && event.at >= self.duration {
                problems.push(
                    Problem::new(format!(
                        "the moment {} is not before the end of the measured phase ({})",
                        event.at, self.duration
                    ))
                    .at_path(format!("{base}.at"))
                    .with_help(
                        "an event at or after the end would change nothing that is measured",
                    ),
                );
            }
            if !ids.contains(&event.link.as_str()) {
                problems.push(unknown_link(&event.link, &ids, format!("{base}.link")));
            }
            check_actions(event, &base, problems);
        }
        // The steps in time order (events at one moment in the order of the file) lead through states
        let mut order: Vec<usize> = (0..self.timeline.len()).collect();
        order.sort_by_key(|index| (self.timeline[*index].at, *index));
        for link in &self.topology.links {
            self.walk_link(link, &order, problems);
        }
    }

    /// The states of one link after each step: the impairments must stay valid, an outage must not overlap
    /// another or a `loss` that is set inside it, and a link goes down only if it is up.
    fn walk_link(&self, link: &Link, order: &[usize], problems: &mut Vec<Problem>) {
        let start = |own: &Option<Impairments>| {
            own.clone()
                .or_else(|| link.both.clone())
                .unwrap_or_default()
        };
        let mut state = LinkState {
            forward: start(&link.forward),
            reverse: start(&link.reverse),
            is_down: false,
            windows: Vec::new(),
        };
        for &index in order {
            let event = &self.timeline[index];
            if event.link != link.id {
                continue;
            }
            let base = format!("timeline[{index}]");
            if let Some(change) = &event.set {
                state.step_set(event, change, &base, problems);
            }
            if let Some(length) = event.outage {
                state.step_outage(event, length, self.duration, index, problems);
            }
            if event.down.is_some() {
                if state.is_down {
                    problems.push(
                        Problem::new("the link is down already")
                            .at_path(format!("{base}.down"))
                            .with_help("bring it up with `up: true` first"),
                    );
                }
                state.is_down = true;
            }
            if event.up.is_some() {
                if !state.is_down {
                    problems.push(
                        Problem::new("the link is up already")
                            .at_path(format!("{base}.up"))
                            .with_help("`up: true` follows a `down: true` of the same link"),
                    );
                }
                state.is_down = false;
            }
        }
    }
}

/// An outage so far: the direction, from, to, and the index of the event.
type Window = (Direction, Duration, Duration, usize);

/// What the timeline has made of one link at a moment.
struct LinkState {
    forward: Impairments,
    reverse: Impairments,
    is_down: bool,
    windows: Vec<Window>,
}

impl LinkState {
    /// A `set`: no loss inside an outage, the impairments valid afterwards, and the step writes something.
    fn step_set(
        &mut self,
        event: &Event,
        change: &Change,
        base: &str,
        problems: &mut Vec<Problem>,
    ) {
        let mut earlier = Vec::new();
        check_impairments(&self.forward, &format!("{base}.set"), &mut earlier);
        check_impairments(&self.reverse, &format!("{base}.set"), &mut earlier);
        for (direction, change) in [
            (Direction::Forward, &change.forward),
            (Direction::Reverse, &change.reverse),
            (Direction::Both, &change.both),
        ] {
            let Some(change) = change else { continue };
            let key = direction_key(direction);
            if change.is_empty() {
                problems.push(
                    Problem::new("this step writes no key, so it changes nothing")
                        .at_path(format!("{base}.set.{key}"))
                        .with_help("write the keys that change, or take the step away"),
                );
            }
            if !change.loss.is_keep() {
                for (_, _, to, at) in self.windows_going_on(direction, event.at) {
                    problems.push(
                        Problem::new(format!(
                            "the loss is set while the outage of timeline[{at}] is going on (until {to})"
                        ))
                        .at_path(format!("{base}.set.{key}.loss"))
                        .with_help("an outage is a loss of 100%; set the loss before it starts or after it ends"),
                    );
                }
            }
            if matches!(direction, Direction::Forward | Direction::Both) {
                self.forward = change.applied_to(&self.forward);
            }
            if matches!(direction, Direction::Reverse | Direction::Both) {
                self.reverse = change.applied_to(&self.reverse);
            }
        }
        // Only what this step breaks: a problem of the link itself is reported with the link
        let mut found = Vec::new();
        check_impairments(&self.forward, &format!("{base}.set"), &mut found);
        if self.reverse != self.forward {
            check_impairments(&self.reverse, &format!("{base}.set"), &mut found);
        }
        for problem in found {
            if !earlier.iter().any(|e| e.message == problem.message) {
                let message = format!("{} after this step", problem.message);
                problems.push(Problem { message, ..problem });
            }
        }
    }

    /// The outages that are going on at `moment` in a direction that shares a side with `direction`.
    fn windows_going_on(&self, direction: Direction, moment: Duration) -> Vec<Window> {
        self.windows
            .iter()
            .copied()
            .filter(|(other, from, to, _)| {
                overlaps_direction(*other, direction) && *from <= moment && moment < *to
            })
            .collect()
    }

    /// An `outage`: longer than zero, over before the end, and not overlapping another.
    fn step_outage(
        &mut self,
        event: &Event,
        length: Duration,
        duration: Duration,
        index: usize,
        problems: &mut Vec<Problem>,
    ) {
        let base = format!("timeline[{index}]");
        let direction = event.direction.unwrap_or(Direction::Both);
        let end = Duration::from_nanos(event.at.as_nanos().saturating_add(length.as_nanos()));
        if length.is_zero() {
            problems.push(
                Problem::new("an outage must last longer than zero")
                    .at_path(format!("{base}.outage")),
            );
        }
        if end > duration {
            problems.push(
                Problem::new(format!(
                    "the outage ends at {end}, after the end of the measured phase ({duration})"
                ))
                .at_path(format!("{base}.outage"))
                .with_help("make it shorter, or start it earlier"),
            );
        }
        for (other, from, to, at) in &self.windows {
            if overlaps_direction(*other, direction) && event.at < *to && *from < end {
                problems.push(
                    Problem::new(format!(
                        "the outage overlaps the outage of timeline[{at}] ({from} to {to})"
                    ))
                    .at_path(format!("{base}.at"))
                    .with_help("two outages at once have no clear end of the first; make one outage of them"),
                );
            }
        }
        self.windows.push((direction, event.at, end, index));
    }
}

/// An event does exactly one thing, and `direction` belongs to the outage.
fn check_actions(event: &Event, base: &str, problems: &mut Vec<Problem>) {
    let named: Vec<&str> = [
        ("set", event.set.is_some()),
        ("outage", event.outage.is_some()),
        ("down", event.down.is_some()),
        ("up", event.up.is_some()),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(name, _)| name)
    .collect();
    if named.len() != 1 {
        let found = if named.is_empty() {
            "none".to_owned()
        } else {
            named.join(", ")
        };
        problems.push(
            Problem::new(format!(
                "an event does exactly one of `set`, `outage`, `down` and `up`, found {found}"
            ))
            .at_path(base.to_owned())
            .with_help("write one event for each thing that happens"),
        );
    }
    if event.direction.is_some() && event.outage.is_none() {
        problems.push(
            Problem::new("`direction` belongs to `outage`")
                .at_path(format!("{base}.direction"))
                .with_help("in `set` write `forward`, `reverse` or `both`; `down` and `up` act on the interface, which has both"),
        );
    }
}

fn direction_key(direction: Direction) -> &'static str {
    match direction {
        Direction::Forward => "forward",
        Direction::Reverse => "reverse",
        Direction::Both => "both",
    }
}

/// Whether two directions have a side in common.
fn overlaps_direction(first: Direction, second: Direction) -> bool {
    first == second || first == Direction::Both || second == Direction::Both
}

fn unknown_link(link: &Name, ids: &[&str], path: String) -> Problem {
    let list = format!("the links are: {}", ids.join(", "));
    let help = match closest(link.as_str(), ids) {
        Some(near) => format!("did you mean '{near}'? {list}"),
        None => list,
    };
    Problem::new(format!("the link '{link}' does not exist"))
        .at_path(path)
        .with_help(help)
}

pub(crate) fn unknown_node(node: &Name, nodes: &[Name], path: String) -> Problem {
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
    if let Some(Loss::Markov4 { p13, p14, .. }) = &impairments.loss {
        check_sum("p13", *p13, "p14", *p14, &format!("{base}.loss"), problems);
    }
    if let Some(Loss::Markov4 { p31, p32, .. }) = &impairments.loss {
        check_sum("p31", *p31, "p32", *p32, &format!("{base}.loss"), problems);
    }
    if let Some(reorder) = &impairments.reorder {
        if reorder.gap == Some(0) {
            problems.push(
                Problem::new("the `gap` of `reorder` must be at least 1")
                    .at_path(format!("{base}.reorder.gap"))
                    .with_help("leave `gap` out for the default of 1"),
            );
        }
        if impairments.delay.is_none() {
            problems.push(
                Problem::new(
                    "`reorder` needs a `delay`: packets are reordered by holding some back",
                )
                .at_path(format!("{base}.reorder"))
                .with_help("add `delay` next to `reorder`; without it `tc` refuses the setting"),
            );
        }
    }
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

/// Two transitions out of the same state of the four-state model are chances of one draw, so they cannot add
/// up to more than 100 %.
fn check_sum(
    first_name: &str,
    first: Probability,
    second_name: &str,
    second: Probability,
    path: &str,
    problems: &mut Vec<Problem>,
) {
    let sum = u64::from(first.micro_percent()) + u64::from(second.micro_percent());
    if sum > 100_000_000 {
        problems.push(
            Problem::new(format!(
                "`{first_name}` ({first}) and `{second_name}` ({second}) add up to more than 100%"
            ))
            .at_path(format!("{path}.{second_name}"))
            .with_help("they are the two ways out of one state, so together they are at most 100%"),
        );
    }
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
