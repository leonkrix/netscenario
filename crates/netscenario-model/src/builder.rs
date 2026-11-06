//! Building a scenario in code (SCN-04). The result is the same model that a file produces, so a scenario
//! built here and one read from a file are equal if they say the same.
//!
//! ```
//! use netscenario_model::{Duration, Impairments, Probability, Rate, Scenario, Size};
//!
//! let scenario = Scenario::builder("lte-like")
//!     .duration(Duration::from_secs(30).unwrap())
//!     .seed(42)
//!     .nodes("client", "server")
//!     .link(
//!         "access",
//!         Some(Impairments::new().with_delay("40ms".parse().unwrap()).with_loss_random("1%".parse().unwrap())),
//!         None,
//!     )
//!     .udp_cbr("media", "client", "server", "2mbit".parse().unwrap(), Size::from_bytes(1200))
//!     .build()?;
//! assert_eq!(scenario.workloads.len(), 1);
//! # Ok::<(), Vec<netscenario_model::Problem>>(())
//! ```

use crate::error::Problem;
use crate::generators::{PhaseDistribution, UdpBursty, UdpOnOff, UdpPoisson};
use crate::measure::Measure;
use crate::scenario::{
    Backend, Change, Command, Direction, Event, FormatVersion, Impairments, Jitter,
    JitterDistribution, Link, Loss, Reorder, Scenario, True, UdpCbr, UdpChecksum, Workload,
};
use crate::tcp::{RequestResponse, TcpBulk};
use crate::topology::{Node, Role, Route, Routing, Topology};
use crate::units::{Duration, Name, Probability, Rate, Size};

impl Impairments {
    /// No impairment at all.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a packet draws random numbers in this direction: loss (all models), duplication, corruption
    /// and reordering do, a delay with a jitter does too but is not counted, because the pattern of lost
    /// packets does not depend on it. These are the impairments for which the seed of a run matters.
    #[must_use]
    pub fn is_random(&self) -> bool {
        self.loss.is_some()
            || self.duplicate.is_some()
            || self.corrupt.is_some()
            || self.reorder.is_some()
    }

    /// With a fixed delay.
    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// With a variation of the delay (needs a delay; checked by [`Scenario::validate`]).
    #[must_use]
    pub fn with_jitter(mut self, amount: Duration, distribution: JitterDistribution) -> Self {
        self.jitter = Some(Jitter::new(amount, distribution));
        self
    }

    /// With independent packet loss.
    #[must_use]
    pub fn with_loss_random(mut self, p: Probability) -> Self {
        self.loss = Some(Loss::Random { p });
        self
    }

    /// With bursts of loss from two states (Gilbert-Elliott): `p` and `r` are the chances to go to the bad and
    /// back to the good state, `loss_bad` and `loss_good` the chances that a packet is lost in each.
    #[must_use]
    pub fn with_loss_gilbert_elliott(
        mut self,
        p: Probability,
        r: Probability,
        loss_bad: Probability,
        loss_good: Probability,
    ) -> Self {
        self.loss = Some(Loss::GilbertElliott {
            p,
            r,
            loss_bad,
            loss_good,
        });
        self
    }

    /// With bursts of loss from four states, the five transitions in the order of `netem loss state`.
    #[must_use]
    pub fn with_loss_markov4(
        mut self,
        p13: Probability,
        p31: Probability,
        p32: Probability,
        p23: Probability,
        p14: Probability,
    ) -> Self {
        self.loss = Some(Loss::Markov4 {
            p13,
            p31,
            p32,
            p23,
            p14,
        });
        self
    }

    /// With reordering (needs a delay; checked by [`Scenario::validate`]).
    #[must_use]
    pub fn with_reorder(mut self, share: Probability, gap: Option<u32>) -> Self {
        self.reorder = Some(Reorder { share, gap });
        self
    }

    /// With duplication.
    #[must_use]
    pub fn with_duplicate(mut self, p: Probability) -> Self {
        self.duplicate = Some(p);
        self
    }

    /// With corruption.
    #[must_use]
    pub fn with_corrupt(mut self, p: Probability) -> Self {
        self.corrupt = Some(p);
        self
    }

    /// With a rate limit.
    #[must_use]
    pub fn with_rate(mut self, rate: Rate) -> Self {
        self.rate = Some(rate);
        self
    }
}

impl Event {
    /// An event with no action yet; the constructors below give it one.
    fn at(at: Duration, link: Name) -> Self {
        Self {
            at,
            link,
            direction: None,
            set: None,
            outage: None,
            down: None,
            up: None,
            every: None,
            until: None,
            count: None,
        }
    }

    /// Repeats the event every `every`, until the end of the measured phase (TML-03).
    #[must_use]
    pub fn repeating(mut self, every: Duration) -> Self {
        self.every = Some(every);
        self
    }

    /// With a repeating event: stops before `until`.
    #[must_use]
    pub fn until(mut self, until: Duration) -> Self {
        self.until = Some(until);
        self
    }

    /// With a repeating event: happens `count` times in all.
    #[must_use]
    pub fn times(mut self, count: u32) -> Self {
        self.count = Some(count);
        self
    }

    /// At `at`, change values of `link`.
    #[must_use]
    pub fn set(at: Duration, link: Name, change: Change) -> Self {
        Self {
            set: Some(change),
            ..Self::at(at, link)
        }
    }

    /// At `at`, nothing is delivered on `link` (in `direction`, or both) for `length`, not even a packet that is
    /// on its way (D-064).
    #[must_use]
    pub fn outage(
        at: Duration,
        link: Name,
        length: Duration,
        direction: Option<Direction>,
    ) -> Self {
        Self {
            outage: Some(length),
            direction,
            ..Self::at(at, link)
        }
    }

    /// At `at`, the interface of `link` goes down.
    #[must_use]
    pub fn down(at: Duration, link: Name) -> Self {
        Self {
            down: Some(True),
            ..Self::at(at, link)
        }
    }

    /// At `at`, the interface of `link` comes up.
    #[must_use]
    pub fn up(at: Duration, link: Name) -> Self {
        Self {
            up: Some(True),
            ..Self::at(at, link)
        }
    }
}

/// Builds a [`Scenario`] step by step. Names are given as text and checked when the scenario is built, so a
/// chain of calls does not need a `?` after each one.
#[derive(Debug, Clone)]
pub struct ScenarioBuilder {
    name: Option<Name>,
    seed: Option<u64>,
    duration: Option<Duration>,
    backend: Backend,
    nodes: Vec<Node>,
    routing: Routing,
    ecmp: bool,
    ip: crate::IpMode,
    routes: Vec<Route>,
    links: Vec<Link>,
    workloads: Vec<Workload>,
    timeline: Vec<Event>,
    measure: Measure,
    problems: Vec<Problem>,
}

impl Scenario {
    /// Starts building a scenario with the given name.
    #[must_use]
    pub fn builder(name: &str) -> ScenarioBuilder {
        let mut builder = ScenarioBuilder {
            name: None,
            seed: None,
            duration: None,
            backend: Backend::Auto,
            nodes: Vec::new(),
            routing: Routing::Auto,
            ecmp: false,
            ip: crate::IpMode::V4,
            routes: Vec::new(),
            links: Vec::new(),
            workloads: Vec::new(),
            timeline: Vec::new(),
            measure: Measure::default(),
            problems: Vec::new(),
        };
        builder.name = builder.name_at(name, "name");
        builder
    }
}

impl ScenarioBuilder {
    fn name_at(&mut self, text: &str, path: &str) -> Option<Name> {
        match Name::new(text) {
            Ok(name) => Some(name),
            Err(error) => {
                self.problems
                    .push(Problem::new(error.to_string()).at_path(path));
                None
            }
        }
    }

    /// The length of the measured phase.
    #[must_use]
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// The seed of the scenario.
    #[must_use]
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// The backend.
    #[must_use]
    pub fn backend(mut self, backend: Backend) -> Self {
        self.backend = backend;
        self
    }

    /// The two hosts of a point to point network. For more nodes use [`ScenarioBuilder::node`] and
    /// [`ScenarioBuilder::router`].
    #[must_use]
    pub fn nodes(mut self, first: &str, second: &str) -> Self {
        for text in [first, second] {
            self = self.node(text);
        }
        self
    }

    /// A host.
    #[must_use]
    pub fn node(mut self, name: &str) -> Self {
        let path = format!("topology.nodes[{}]", self.nodes.len());
        if let Some(name) = self.name_at(name, &path) {
            self.nodes.push(Node::host(name));
        }
        self
    }

    /// A router: a node that forwards packets.
    #[must_use]
    pub fn router(mut self, name: &str) -> Self {
        let path = format!("topology.nodes[{}]", self.nodes.len());
        if let Some(name) = self.name_at(name, &path) {
            self.nodes.push(Node {
                id: name,
                role: Role::Router,
            });
        }
        self
    }

    /// A link between the two nodes, with impairments for the direction from the first to the second and for
    /// the way back. For the same values in both directions use [`ScenarioBuilder::link_both`]. Only for a
    /// network of two nodes; for more use [`ScenarioBuilder::link_between`].
    #[must_use]
    pub fn link(
        self,
        id: &str,
        forward: Option<Impairments>,
        reverse: Option<Impairments>,
    ) -> Self {
        if let [first, second] = self.nodes.as_slice() {
            let (first, second) = (first.id.as_str().to_owned(), second.id.as_str().to_owned());
            self.link_between(id, &first, &second, forward, reverse)
        } else {
            let mut this = self;
            let index = this.links.len();
            this.problems.push(
                Problem::new(
                    "call `nodes` with two valid names before `link`, or use `link_between`",
                )
                .at_path(format!("topology.links[{index}]")),
            );
            this
        }
    }

    /// A link between `first` and `second` with impairments for the direction from the first to the second and
    /// for the way back.
    #[must_use]
    pub fn link_between(
        mut self,
        id: &str,
        first: &str,
        second: &str,
        forward: Option<Impairments>,
        reverse: Option<Impairments>,
    ) -> Self {
        let index = self.links.len();
        let id = self.name_at(id, &format!("topology.links[{index}].id"));
        let first = self.name_at(first, &format!("topology.links[{index}].between[0]"));
        let second = self.name_at(second, &format!("topology.links[{index}].between[1]"));
        if let (Some(id), Some(first), Some(second)) = (id, first, second) {
            let mut link = Link::new(id, [first, second]);
            link.forward = forward;
            link.reverse = reverse;
            self.links.push(link);
        }
        self
    }

    /// What the link that was added last costs the automatic routing (1 by default).
    #[must_use]
    pub fn link_cost(mut self, cost: u32) -> Self {
        if let Some(link) = self.links.last_mut() {
            link.cost = Some(cost);
        }
        self
    }

    /// The addresses of the network: IPv4 (the default), IPv6 or both (D-081).
    #[must_use]
    pub fn ip(mut self, mode: crate::IpMode) -> Self {
        self.ip = mode;
        self
    }

    /// The MTU of the link that was added last, in bytes (1500 by default).
    #[must_use]
    pub fn link_mtu(mut self, mtu: u32) -> Self {
        if let Some(link) = self.links.last_mut() {
            link.mtu = Some(mtu);
        }
        self
    }

    /// The routing: `Routing::Auto` (the default) or `Routing::Manual`.
    #[must_use]
    pub fn routing(mut self, routing: Routing) -> Self {
        self.routing = routing;
        self
    }

    /// Install every equally short path as a multipath route (with the automatic routing).
    #[must_use]
    pub fn ecmp(mut self) -> Self {
        self.ecmp = true;
        self
    }

    /// A route of `node` to `to` (a node, or `default`) out of the link `via`.
    #[must_use]
    pub fn route(mut self, node: &str, to: &str, via: &str) -> Self {
        let index = self.routes.len();
        let node = self.name_at(node, &format!("topology.routes[{index}].node"));
        let to = self.name_at(to, &format!("topology.routes[{index}].to"));
        let via = self.name_at(via, &format!("topology.routes[{index}].via"));
        if let (Some(node), Some(to), Some(via)) = (node, to, via) {
            self.routes.push(Route::new(node, to, via));
        }
        self
    }

    /// The metric of the route that was added last.
    #[must_use]
    pub fn route_metric(mut self, metric: u32) -> Self {
        if let Some(route) = self.routes.last_mut() {
            route.metric = Some(metric);
        }
        self
    }

    /// The weight of the route that was added last.
    #[must_use]
    pub fn route_weight(mut self, weight: u32) -> Self {
        if let Some(route) = self.routes.last_mut() {
            route.weight = Some(weight);
        }
        self
    }

    /// A link with the same impairments in both directions.
    #[must_use]
    pub fn link_both(mut self, id: &str, both: Impairments) -> Self {
        self = self.link(id, None, None);
        if let Some(link) = self.links.last_mut() {
            link.both = Some(both);
        }
        self
    }

    /// A workload that sends UDP at a constant bit rate from one node to the other.
    #[must_use]
    pub fn udp_cbr(
        mut self,
        id: &str,
        from: &str,
        to: &str,
        rate: Rate,
        packet_size: Size,
    ) -> Self {
        let index = self.workloads.len();
        let id = self.name_at(id, &format!("workloads[{index}].id"));
        let from = self.name_at(from, &format!("workloads[{index}].from"));
        let to = self.name_at(to, &format!("workloads[{index}].to"));
        if let (Some(id), Some(from), Some(to)) = (id, from, to) {
            self.workloads.push(Workload::UdpCbr(UdpCbr::new(
                id,
                from,
                to,
                rate,
                packet_size,
            )));
        }
        self
    }

    /// A workload that sends data over TCP from one node to the other, as fast as the network allows (`tcp-bulk`, D-086). One
    /// connection that sends from the start to the end of the measured phase; [`bytes_each`](Self::bytes_each),
    /// [`connections`](Self::connections) and [`congestion`](Self::congestion) change that.
    #[must_use]
    pub fn tcp_bulk(mut self, id: &str, from: &str, to: &str) -> Self {
        let index = self.workloads.len();
        let id = self.name_at(id, &format!("workloads[{index}].id"));
        let from = self.name_at(from, &format!("workloads[{index}].from"));
        let to = self.name_at(to, &format!("workloads[{index}].to"));
        if let (Some(id), Some(from), Some(to)) = (id, from, to) {
            self.workloads
                .push(Workload::TcpBulk(TcpBulk::new(id, from, to)));
        }
        self
    }

    /// A workload that sends requests of one size and reads responses of another over TCP (`request-response`, D-086). One
    /// connection in a closed loop; [`connections`](Self::connections), [`request_interval`](Self::request_interval) and
    /// [`request_timeout`](Self::request_timeout) change that.
    #[must_use]
    pub fn request_response(
        mut self,
        id: &str,
        from: &str,
        to: &str,
        request: Size,
        response: Size,
    ) -> Self {
        let index = self.workloads.len();
        let id = self.name_at(id, &format!("workloads[{index}].id"));
        let from = self.name_at(from, &format!("workloads[{index}].from"));
        let to = self.name_at(to, &format!("workloads[{index}].to"));
        if let (Some(id), Some(from), Some(to)) = (id, from, to) {
            self.workloads
                .push(Workload::RequestResponse(RequestResponse::new(
                    id, from, to, request, response,
                )));
        }
        self
    }

    /// Makes each connection of the last `tcp-bulk` workload send this much and end.
    #[must_use]
    pub fn bytes_each(mut self, bytes: Size) -> Self {
        if let Some(Workload::TcpBulk(workload)) = self.workloads.last_mut() {
            workload.bytes = Some(bytes);
        }
        self
    }

    /// Sets how many connections the last TCP workload has.
    #[must_use]
    pub fn connections(mut self, connections: u32) -> Self {
        match self.workloads.last_mut() {
            Some(Workload::TcpBulk(workload)) => workload.connections = connections,
            Some(Workload::RequestResponse(workload)) => workload.connections = connections,
            _ => {}
        }
        self
    }

    /// Sets the congestion control of the last TCP workload, for example `reno` (the default is `cubic`).
    #[must_use]
    pub fn congestion(mut self, name: &str) -> Self {
        let index = self.workloads.len().saturating_sub(1);
        let name = self.name_at(name, &format!("workloads[{index}].cc"));
        match (self.workloads.last_mut(), name) {
            (Some(Workload::TcpBulk(workload)), Some(name)) => workload.cc = Some(name),
            (Some(Workload::RequestResponse(workload)), Some(name)) => workload.cc = Some(name),
            _ => {}
        }
        self
    }

    /// Makes the last `request-response` workload an open loop: a request is due every `interval`, whichever connection takes it.
    #[must_use]
    pub fn request_interval(mut self, interval: Duration) -> Self {
        if let Some(Workload::RequestResponse(workload)) = self.workloads.last_mut() {
            workload.interval = Some(interval);
        }
        self
    }

    /// Sets how long a request of the last `request-response` workload waits for its response.
    #[must_use]
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        if let Some(Workload::RequestResponse(workload)) = self.workloads.last_mut() {
            workload.timeout = Some(timeout);
        }
        self
    }

    /// A workload that sends UDP packets as a Poisson process with the mean rate `rate` (`udp-poisson`).
    #[must_use]
    pub fn udp_poisson(
        mut self,
        id: &str,
        from: &str,
        to: &str,
        rate: Rate,
        packet_size: Size,
    ) -> Self {
        if let Some((id, from, to)) = self.workload_names(id, from, to) {
            self.workloads.push(Workload::UdpPoisson(UdpPoisson::new(
                id,
                from,
                to,
                rate,
                packet_size,
            )));
        }
        self
    }

    /// A workload that sends at `rate` for `on`, then nothing for `off`, from the start (`udp-onoff`, exact phases;
    /// [`ScenarioBuilder::drawn_phases`] makes them exponential).
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the keys of the kind, all of them required"
    )]
    pub fn udp_onoff(
        mut self,
        id: &str,
        from: &str,
        to: &str,
        rate: Rate,
        packet_size: Size,
        on: Duration,
        off: Duration,
    ) -> Self {
        if let Some((id, from, to)) = self.workload_names(id, from, to) {
            self.workloads.push(Workload::UdpOnOff(UdpOnOff::new(
                id,
                from,
                to,
                rate,
                packet_size,
                on,
                off,
            )));
        }
        self
    }

    /// A workload that sends bursts of `burst_packets` packets `gap` apart with a `pause` after each (`udp-bursty`,
    /// exact pauses; [`ScenarioBuilder::drawn_phases`] makes them exponential).
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the keys of the kind, all of them required"
    )]
    pub fn udp_bursty(
        mut self,
        id: &str,
        from: &str,
        to: &str,
        packet_size: Size,
        burst_packets: u32,
        gap: Duration,
        pause: Duration,
    ) -> Self {
        if let Some((id, from, to)) = self.workload_names(id, from, to) {
            self.workloads.push(Workload::UdpBursty(UdpBursty::new(
                id,
                from,
                to,
                packet_size,
                burst_packets,
                gap,
                pause,
            )));
        }
        self
    }

    /// Makes the phases of the last `udp-onoff` workload, or the pauses of the last `udp-bursty` one, exponentially
    /// distributed with the time that was given as their mean (`distribution: exponential`).
    #[must_use]
    pub fn drawn_phases(mut self) -> Self {
        match self.workloads.last_mut() {
            Some(Workload::UdpOnOff(workload)) => {
                workload.distribution = PhaseDistribution::Exponential;
            }
            Some(Workload::UdpBursty(workload)) => {
                workload.distribution = PhaseDistribution::Exponential;
            }
            _ => {}
        }
        self
    }

    /// The names of the two nodes and the workload, checked, or `None` with the problems noted.
    fn workload_names(&mut self, id: &str, from: &str, to: &str) -> Option<(Name, Name, Name)> {
        let index = self.workloads.len();
        let id = self.name_at(id, &format!("workloads[{index}].id"));
        let from = self.name_at(from, &format!("workloads[{index}].from"));
        let to = self.name_at(to, &format!("workloads[{index}].to"));
        Some((id?, from?, to?))
    }

    /// A workload that starts a program of your own in `node`: `run` is the program and its arguments (no
    /// shell), and `{node.addr}` in them is the address of a node. Readiness, environment and shutdown can
    /// be set on the [`Command`] with [`ScenarioBuilder::workload`].
    #[must_use]
    pub fn command(mut self, id: &str, node: &str, run: &[&str]) -> Self {
        let index = self.workloads.len();
        let id = self.name_at(id, &format!("workloads[{index}].id"));
        let node = self.name_at(node, &format!("workloads[{index}].node"));
        if let (Some(id), Some(node)) = (id, node) {
            self.workloads.push(Workload::Command(Command::new(
                id,
                node,
                run.iter().map(|argument| (*argument).to_owned()).collect(),
            )));
        }
        self
    }

    /// Adds a workload that was made by hand.
    #[must_use]
    pub fn workload(mut self, workload: Workload) -> Self {
        self.workloads.push(workload);
        self
    }

    /// Sets what is measured besides the statistics: the bins of the series, the time to recover, the links to capture.
    #[must_use]
    pub fn measure(mut self, measure: Measure) -> Self {
        self.measure = measure;
        self
    }

    /// Adds an event to the timeline (checked when the scenario is built).
    #[must_use]
    pub fn event(mut self, event: Event) -> Self {
        self.timeline.push(event);
        self
    }

    /// Sets when the last workload starts and stops, from the start of the measured phase.
    #[must_use]
    pub fn window(mut self, start: Duration, stop: Duration) -> Self {
        match self.workloads.last_mut() {
            Some(Workload::UdpCbr(workload)) => {
                workload.start = Some(start);
                workload.stop = Some(stop);
            }
            Some(Workload::UdpPoisson(workload)) => {
                workload.start = Some(start);
                workload.stop = Some(stop);
            }
            Some(Workload::UdpOnOff(workload)) => {
                workload.start = Some(start);
                workload.stop = Some(stop);
            }
            Some(Workload::UdpBursty(workload)) => {
                workload.start = Some(start);
                workload.stop = Some(stop);
            }
            Some(Workload::TcpBulk(workload)) => {
                workload.start = Some(start);
                workload.stop = Some(stop);
            }
            Some(Workload::RequestResponse(workload)) => {
                workload.start = Some(start);
                workload.stop = Some(stop);
            }
            _ => {}
        }
        self
    }

    /// Sends the packets of the last UDP workload without the UDP checksum (`udp-checksum: false`), so that
    /// a corrupted packet arrives and is counted (D-040). The packet size must then be at least
    /// [`MIN_PACKET_SIZE_WITHOUT_CHECKSUM`](crate::MIN_PACKET_SIZE_WITHOUT_CHECKSUM).
    #[must_use]
    pub fn without_udp_checksum(mut self) -> Self {
        match self.workloads.last_mut() {
            Some(Workload::UdpCbr(workload)) => workload.udp_checksum = UdpChecksum::Off,
            Some(Workload::UdpPoisson(workload)) => workload.udp_checksum = UdpChecksum::Off,
            Some(Workload::UdpOnOff(workload)) => workload.udp_checksum = UdpChecksum::Off,
            Some(Workload::UdpBursty(workload)) => workload.udp_checksum = UdpChecksum::Off,
            _ => {}
        }
        self
    }

    /// Has the receiving node send every packet of the last UDP workload back (`echo: true`, D-087), so that the result has
    /// the round trip. Not with [`without_udp_checksum`](Self::without_udp_checksum).
    #[must_use]
    pub fn with_echo(mut self) -> Self {
        match self.workloads.last_mut() {
            Some(Workload::UdpCbr(workload)) => workload.echo = true,
            Some(Workload::UdpPoisson(workload)) => workload.echo = true,
            Some(Workload::UdpOnOff(workload)) => workload.echo = true,
            Some(Workload::UdpBursty(workload)) => workload.echo = true,
            Some(Workload::TraceReplay(workload)) => workload.echo = true,
            _ => {}
        }
        self
    }

    /// Builds the scenario and checks it with [`Scenario::validate`].
    ///
    /// # Errors
    ///
    /// Returns every problem found: names that are not valid, a missing name or duration, and the rules of
    /// [`Scenario::validate`].
    pub fn build(mut self) -> Result<Scenario, Vec<Problem>> {
        if self.duration.is_none() {
            self.problems
                .push(Problem::new("the duration is missing: call `duration`").at_path("duration"));
        }
        let (Some(name), Some(duration)) = (self.name, self.duration) else {
            return Err(self.problems);
        };
        if !self.problems.is_empty() {
            return Err(self.problems);
        }
        let scenario = Scenario {
            version: FormatVersion,
            name,
            seed: self.seed,
            duration,
            backend: self.backend,
            topology: {
                let mut topology = Topology::new(self.nodes, self.links);
                topology.routing = self.routing;
                topology.ecmp = self.ecmp;
                topology.ip = self.ip;
                topology.routes = self.routes;
                topology
            },
            workloads: self.workloads,
            timeline: self.timeline,
            measure: self.measure,
        };
        let problems = scenario.validate();
        if problems.is_empty() {
            Ok(scenario)
        } else {
            Err(problems)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_scenario_equals_the_same_scenario_read_from_a_file() {
        let built = Scenario::builder("minimal")
            .duration(Duration::from_secs(5).unwrap())
            .nodes("client", "server")
            .link("access", None, None)
            .udp_cbr(
                "media",
                "client",
                "server",
                "1mbit".parse().unwrap(),
                Size::from_bytes(1200),
            )
            .build()
            .unwrap();
        let text = include_str!("../../../scenarios/minimal.yaml");
        assert_eq!(built, Scenario::from_yaml(text, "minimal.yaml").unwrap());
    }

    #[test]
    fn every_problem_is_reported_by_the_build() {
        let problems = Scenario::builder("Bad Name")
            .nodes("a", "b")
            .link("access", None, None)
            .udp_cbr(
                "media",
                "a",
                "z",
                "1mbit".parse().unwrap(),
                Size::from_bytes(8),
            )
            .build()
            .unwrap_err();
        assert!(problems.iter().any(|p| p.path.as_deref() == Some("name")));
        assert!(
            problems
                .iter()
                .any(|p| p.path.as_deref() == Some("duration"))
        );
    }

    #[test]
    fn the_rules_of_the_file_apply_to_a_built_scenario() {
        let problems = Scenario::builder("t")
            .duration(Duration::from_secs(10).unwrap())
            .nodes("a", "b")
            .link("access", None, None)
            .udp_cbr(
                "media",
                "a",
                "z",
                "1mbit".parse().unwrap(),
                Size::from_bytes(8),
            )
            .build()
            .unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("the node 'z' does not exist"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("packet size 8 bytes"))
        );
    }
}
