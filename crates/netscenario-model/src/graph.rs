//! The routes of a topology and the path of a packet over them (D-078).
//!
//! The same function computes the routes for the validation (is there a path for every workload?), for the backend (which
//! routes does it install?) and for the result (which path did a workload take?), so they cannot disagree.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use crate::topology::{DEFAULT_DESTINATION, DEFAULT_METRIC, Role, Routing, Topology};
use crate::units::Name;

/// Where a route leads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Destination {
    /// A node.
    Node(Name),
    /// Every destination that has no route of its own.
    Default,
}

impl std::fmt::Display for Destination {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Node(name) => formatter.write_str(name.as_str()),
            Self::Default => formatter.write_str(DEFAULT_DESTINATION),
        }
    }
}

/// One route of one node: out of a link, to a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    /// The node that has the route.
    pub node: Name,
    /// The destination.
    pub to: Destination,
    /// The link the packets leave by.
    pub link: Name,
    /// The node at the other end of the link.
    pub next_hop: Name,
    /// The preference: the smaller is used first.
    pub metric: u32,
    /// The share among routes with the same metric.
    pub weight: u32,
    /// Whether the scenario lists it (`true`) or the tool computed it (`false`).
    pub given: bool,
}

/// The routes of all nodes, in a fixed order: by node (the order of the file), then destination, then metric, then link.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Routes {
    /// Every route.
    pub entries: Vec<RouteEntry>,
}

/// Why a packet cannot get from one node to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// A node on the way has no route to the destination.
    NoRoute {
        /// The node without a route.
        at: Name,
    },
    /// A node on the way is a host and does not forward.
    NotARouter {
        /// The host that the route leads through.
        at: Name,
    },
    /// The routes lead in a circle.
    Loop {
        /// The node that was reached twice.
        at: Name,
    },
}

/// The nodes and the links that a packet passes, and whether there was a choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    /// The nodes from the sender to the receiver, both included.
    pub nodes: Vec<Name>,
    /// The links between them: one fewer than the nodes.
    pub links: Vec<Name>,
    /// Whether a node on the way has several routes of the best metric (multipath): the first by link order is shown, and
    /// the packets of a real run are shared among them.
    pub shared: bool,
}

impl Topology {
    /// The routes of every node: the automatic ones (unless the routing is manual) and the listed ones.
    ///
    /// The scenario must have passed the checks of the names (a link that does not exist is skipped). The automatic route
    /// of a node to a destination is the first hop of the shortest path (the smallest sum of `cost`, 1 by default); only a
    /// router is passed through. Equally short paths: the first link in the order of the file, or all of them with
    /// `ecmp`. A listed route of a node and a destination replaces the automatic ones of that node and destination.
    #[must_use]
    pub fn routes(&self) -> Routes {
        let mut entries = Vec::new();
        for node in &self.nodes {
            let listed: Vec<&crate::topology::Route> = self
                .routes
                .iter()
                .filter(|route| route.node == node.id)
                .collect();
            if self.routing == Routing::Auto {
                for target in &self.nodes {
                    if target.id == node.id {
                        continue;
                    }
                    let replaced = listed.iter().any(|route| route.to == target.id);
                    if replaced {
                        continue;
                    }
                    entries.extend(self.automatic(&node.id, &target.id));
                }
            }
            let mut mine: Vec<RouteEntry> = listed
                .iter()
                .filter_map(|route| {
                    let link = self.link(&route.via)?;
                    let next_hop = other_end(link, &node.id)?;
                    Some(RouteEntry {
                        node: node.id.clone(),
                        to: if route.is_default() {
                            Destination::Default
                        } else {
                            Destination::Node(route.to.clone())
                        },
                        link: link.id.clone(),
                        next_hop,
                        metric: route.metric(),
                        weight: route.weight(),
                        given: true,
                    })
                })
                .collect();
            entries.append(&mut mine);
        }
        let order = |name: &Name| self.nodes.iter().position(|node| node.id == *name);
        let link_order = |name: &Name| self.links.iter().position(|link| link.id == *name);
        entries.sort_by(|a, b| {
            (order(&a.node), &a.to, a.metric, link_order(&a.link)).cmp(&(
                order(&b.node),
                &b.to,
                b.metric,
                link_order(&b.link),
            ))
        });
        Routes { entries }
    }

    /// The automatic routes of `from` to `to`: one, or several with `ecmp`; none if `to` cannot be reached.
    fn automatic(&self, from: &Name, to: &Name) -> Vec<RouteEntry> {
        let distance = self.distances_to(to);
        let at = |name: &Name| self.nodes.iter().position(|node| node.id == *name);
        let Some(from_index) = at(from) else {
            return Vec::new();
        };
        let Some(best) = distance[from_index] else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for link in &self.links {
            let Some(next) = other_end(link, from) else {
                continue;
            };
            let Some(next_index) = at(&next) else {
                continue;
            };
            // Only a router (or the destination itself) is passed through
            if next != *to && self.nodes[next_index].role != Role::Router {
                continue;
            }
            let Some(rest) = distance[next_index] else {
                continue;
            };
            if rest + u64::from(link.cost()) == best {
                found.push(RouteEntry {
                    node: from.clone(),
                    to: Destination::Node(to.clone()),
                    link: link.id.clone(),
                    next_hop: next,
                    metric: DEFAULT_METRIC,
                    weight: 1,
                    given: false,
                });
            }
        }
        if !self.ecmp {
            found.truncate(1);
        }
        found
    }

    /// The length of the shortest path from every node to `target`, through routers only. A host gets a distance (it can
    /// start a path) but is not passed through.
    fn distances_to(&self, target: &Name) -> Vec<Option<u64>> {
        let count = self.nodes.len();
        let at = |name: &Name| self.nodes.iter().position(|node| node.id == *name);
        let mut distance: Vec<Option<u64>> = vec![None; count];
        let Some(start) = at(target) else {
            return distance;
        };
        distance[start] = Some(0);
        let mut queue = BinaryHeap::new();
        queue.push(Reverse((0_u64, start)));
        let mut done: HashSet<usize> = HashSet::new();
        while let Some(Reverse((here, index))) = queue.pop() {
            if !done.insert(index) {
                continue;
            }
            // Only the destination and the routers extend a path
            let passes = index == start || self.nodes[index].role == Role::Router;
            if !passes {
                continue;
            }
            let name = &self.nodes[index].id;
            for link in &self.links {
                let Some(other) = other_end(link, name) else {
                    continue;
                };
                let Some(other_index) = at(&other) else {
                    continue;
                };
                let candidate = here + u64::from(link.cost());
                if distance[other_index].is_none_or(|known| candidate < known) {
                    distance[other_index] = Some(candidate);
                    queue.push(Reverse((candidate, other_index)));
                }
            }
        }
        distance
    }

    /// The path of a packet from `from` to `to` over `routes`: at each node the route of the best metric to the
    /// destination (a route to the node itself, else the default route).
    ///
    /// # Errors
    ///
    /// Returns why the packet does not arrive: a node without a route, a host in the middle, or a loop.
    pub fn path(&self, routes: &Routes, from: &Name, to: &Name) -> Result<Path, PathError> {
        let mut nodes = vec![from.clone()];
        let mut links = Vec::new();
        let mut shared = false;
        let mut seen: HashSet<Name> = HashSet::new();
        let mut here = from.clone();
        while here != *to {
            if !seen.insert(here.clone()) {
                return Err(PathError::Loop { at: here });
            }
            if here != *from
                && self
                    .node(&here)
                    .is_some_and(|node| node.role != Role::Router)
            {
                return Err(PathError::NotARouter { at: here });
            }
            let wanted = Destination::Node(to.clone());
            let own: Vec<&RouteEntry> = routes
                .entries
                .iter()
                .filter(|entry| entry.node == here && entry.to == wanted)
                .collect();
            let candidates = if own.is_empty() {
                routes
                    .entries
                    .iter()
                    .filter(|entry| entry.node == here && entry.to == Destination::Default)
                    .collect()
            } else {
                own
            };
            let Some(best_metric) = candidates.iter().map(|entry| entry.metric).min() else {
                return Err(PathError::NoRoute { at: here });
            };
            let best: Vec<&&RouteEntry> = candidates
                .iter()
                .filter(|entry| entry.metric == best_metric)
                .collect();
            shared |= best.len() > 1;
            // The entries are in the order of the links, so the first is the first by link order
            let Some(entry) = best.first() else {
                return Err(PathError::NoRoute { at: here });
            };
            links.push(entry.link.clone());
            here = entry.next_hop.clone();
            nodes.push(here.clone());
        }
        Ok(Path {
            nodes,
            links,
            shared,
        })
    }
}

/// The node at the other end of `link` from `node`, if `node` is at one end.
fn other_end(link: &crate::scenario::Link, node: &Name) -> Option<Name> {
    let [first, second] = &link.between;
    if first == node {
        Some(second.clone())
    } else if second == node {
        Some(first.clone())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Link;
    use crate::topology::{Node, Route};

    fn name(text: &str) -> Name {
        Name::new(text).unwrap()
    }

    /// A topology from `nodes` (a name, `true` for a router) and `links` (id, first, second, cost).
    fn topo(nodes: &[(&str, bool)], links: &[(&str, &str, &str, u32)]) -> Topology {
        let nodes = nodes
            .iter()
            .map(|(n, router)| {
                if *router {
                    Node::router(name(n))
                } else {
                    Node::host(name(n))
                }
            })
            .collect();
        let links = links
            .iter()
            .map(|(id, a, b, cost)| {
                let mut link = Link::new(name(id), [name(a), name(b)]);
                if *cost != 1 {
                    link.cost = Some(*cost);
                }
                link
            })
            .collect();
        Topology::new(nodes, links)
    }

    fn routes_of(routes: &Routes, node: &str, to: &str) -> Vec<(String, u32)> {
        routes
            .entries
            .iter()
            .filter(|entry| entry.node.as_str() == node && entry.to.to_string() == to)
            .map(|entry| (entry.link.to_string(), entry.metric))
            .collect()
    }

    fn path_links(topology: &Topology, from: &str, to: &str) -> Result<Vec<String>, PathError> {
        let routes = topology.routes();
        topology
            .path(&routes, &name(from), &name(to))
            .map(|path| path.links.iter().map(ToString::to_string).collect())
    }

    /// client - a - server and client - b - server, both routers: a diamond.
    fn diamond(first_cost: u32, second_cost: u32) -> Topology {
        topo(
            &[
                ("client", false),
                ("a", true),
                ("b", true),
                ("server", false),
            ],
            &[
                ("ca", "client", "a", first_cost),
                ("as", "a", "server", 1),
                ("cb", "client", "b", second_cost),
                ("bs", "b", "server", 1),
            ],
        )
    }

    #[test]
    fn the_shortest_path_is_chosen_and_every_node_has_a_route_to_every_other() {
        let chain = topo(
            &[("a", false), ("r1", true), ("r2", true), ("b", false)],
            &[
                ("x", "a", "r1", 1),
                ("y", "r1", "r2", 1),
                ("z", "r2", "b", 1),
            ],
        );
        assert_eq!(path_links(&chain, "a", "b").unwrap(), ["x", "y", "z"]);
        assert_eq!(path_links(&chain, "b", "a").unwrap(), ["z", "y", "x"]);
        // A route to every other node, from every node
        let routes = chain.routes();
        for from in ["a", "r1", "r2", "b"] {
            for to in ["a", "r1", "r2", "b"] {
                if from != to {
                    assert_eq!(routes_of(&routes, from, to).len(), 1, "{from} to {to}");
                }
            }
        }
    }

    /// Canary for the choice: a cost on the first link sends the packets over the second, and without it the first (the
    /// first by the order of the links) is taken. A routing that ignored the cost, or the order, would fail one of them.
    #[test]
    fn the_cost_decides_and_a_tie_goes_to_the_first_link_of_the_file() {
        assert_eq!(
            path_links(&diamond(1, 1), "client", "server").unwrap(),
            ["ca", "as"]
        );
        assert_eq!(
            path_links(&diamond(5, 1), "client", "server").unwrap(),
            ["cb", "bs"]
        );
        let mut swapped = diamond(1, 1);
        swapped.links.swap(0, 2);
        swapped.links.swap(1, 3);
        assert_eq!(
            path_links(&swapped, "client", "server").unwrap(),
            ["cb", "bs"]
        );
    }

    #[test]
    fn a_host_is_never_passed_through() {
        let hosts = topo(
            &[("a", false), ("h", false), ("b", false)],
            &[("x", "a", "h", 1), ("y", "h", "b", 1)],
        );
        assert_eq!(
            path_links(&hosts, "a", "b"),
            Err(PathError::NoRoute { at: name("a") })
        );
        // But it is reached, and it reaches
        assert_eq!(path_links(&hosts, "a", "h").unwrap(), ["x"]);
        assert_eq!(path_links(&hosts, "h", "b").unwrap(), ["y"]);
        // Canary: with a router in its place the path exists
        let router = topo(
            &[("a", false), ("h", true), ("b", false)],
            &[("x", "a", "h", 1), ("y", "h", "b", 1)],
        );
        assert_eq!(path_links(&router, "a", "b").unwrap(), ["x", "y"]);
    }

    #[test]
    fn a_node_without_a_chain_of_links_to_another_has_no_route() {
        let split = topo(
            &[("a", false), ("b", false), ("c", false), ("d", false)],
            &[("x", "a", "b", 1), ("y", "c", "d", 1)],
        );
        assert!(
            split
                .routes()
                .entries
                .iter()
                .all(|e| e.to.to_string() != "c" || e.node.as_str() == "d")
        );
        assert!(matches!(
            path_links(&split, "a", "d"),
            Err(PathError::NoRoute { .. })
        ));
    }

    #[test]
    fn ecmp_installs_every_equally_short_first_hop_and_off_it_installs_one() {
        let mut one = diamond(1, 1);
        let routes = one.routes();
        assert_eq!(
            routes_of(&routes, "client", "server"),
            [("ca".to_owned(), 100)]
        );
        one.ecmp = true;
        let routes = one.routes();
        assert_eq!(
            routes_of(&routes, "client", "server"),
            [("ca".to_owned(), 100), ("cb".to_owned(), 100)]
        );
        let path = one.path(&routes, &name("client"), &name("server")).unwrap();
        assert!(path.shared, "the packets are shared at the client");
        assert_eq!(path.links, [name("ca"), name("as")]);
        // Unequal costs: only the shorter, even with ecmp
        let mut unequal = diamond(2, 1);
        unequal.ecmp = true;
        assert_eq!(
            routes_of(&unequal.routes(), "client", "server"),
            [("cb".to_owned(), 100)]
        );
    }

    #[test]
    fn a_listed_route_replaces_the_automatic_ones_of_its_node_and_destination_only() {
        let mut topology = diamond(1, 1);
        topology
            .routes
            .push(Route::new(name("client"), name("server"), name("cb")));
        let routes = topology.routes();
        let own = routes_of(&routes, "client", "server");
        assert_eq!(
            own,
            [("cb".to_owned(), 100)],
            "the automatic route over ca is gone"
        );
        assert!(
            routes
                .entries
                .iter()
                .any(|e| e.node.as_str() == "client" && e.to.to_string() == "a" && !e.given)
        );
        assert!(
            routes
                .entries
                .iter()
                .any(|e| e.given && e.link.as_str() == "cb")
        );
        assert_eq!(
            path_links(&topology, "client", "server").unwrap(),
            ["cb", "bs"]
        );
    }

    #[test]
    fn with_manual_routing_only_the_listed_routes_exist() {
        let mut topology = diamond(1, 1);
        topology.routing = Routing::Manual;
        assert_eq!(topology.routes().entries, Vec::<RouteEntry>::new());
        assert_eq!(
            path_links(&topology, "client", "server"),
            Err(PathError::NoRoute { at: name("client") })
        );
        topology.routes = vec![
            Route::new(name("client"), name("server"), name("ca")),
            Route::new(name("a"), name("server"), name("as")),
        ];
        assert_eq!(
            path_links(&topology, "client", "server").unwrap(),
            ["ca", "as"]
        );
        // The way back has no routes
        assert!(matches!(
            path_links(&topology, "server", "client"),
            Err(PathError::NoRoute { .. })
        ));
    }

    #[test]
    fn the_smallest_metric_is_used_first_and_the_default_route_covers_the_rest() {
        let mut topology = diamond(1, 1);
        topology.routing = Routing::Manual;
        let mut slow = Route::new(name("client"), name("server"), name("cb"));
        slow.metric = Some(20);
        let mut fast = Route::new(name("client"), name("server"), name("ca"));
        fast.metric = Some(10);
        topology.routes = vec![
            slow,
            fast,
            Route::new(name("a"), name("server"), name("as")),
            Route::new(name("b"), name("server"), name("bs")),
        ];
        assert_eq!(
            path_links(&topology, "client", "server").unwrap(),
            ["ca", "as"]
        );
        // Canary: swapping the metrics swaps the path
        for route in &mut topology.routes[..2] {
            route.metric = Some(30 - route.metric.unwrap());
        }
        assert_eq!(
            path_links(&topology, "client", "server").unwrap(),
            ["cb", "bs"]
        );
        // A default route answers for a destination with no route of its own
        let mut with_default = diamond(1, 1);
        with_default.routing = Routing::Manual;
        with_default.routes = vec![
            Route::new(name("client"), name(DEFAULT_DESTINATION), name("ca")),
            Route::new(name("a"), name("server"), name("as")),
        ];
        assert_eq!(
            path_links(&with_default, "client", "server").unwrap(),
            ["ca", "as"]
        );
    }

    #[test]
    fn routes_that_lead_in_a_circle_or_through_a_host_are_found() {
        let mut topology = topo(
            &[("a", false), ("r", true), ("b", false)],
            &[("x", "a", "r", 1), ("y", "r", "b", 1)],
        );
        topology.routing = Routing::Manual;
        topology.routes = vec![
            Route::new(name("a"), name("b"), name("x")),
            Route::new(name("r"), name("b"), name("x")),
        ];
        assert_eq!(
            path_links(&topology, "a", "b"),
            Err(PathError::Loop { at: name("a") })
        );
        // Through a host: the route of `a` leads to `h`, a host, which then has a route on
        let mut through = topo(
            &[("a", false), ("h", false), ("b", false)],
            &[("x", "a", "h", 1), ("y", "h", "b", 1)],
        );
        through.routing = Routing::Manual;
        through.routes = vec![
            Route::new(name("a"), name("b"), name("x")),
            Route::new(name("h"), name("b"), name("y")),
        ];
        assert_eq!(
            path_links(&through, "a", "b"),
            Err(PathError::NotARouter { at: name("h") })
        );
    }

    /// A ring of six routers: between any two the path is the shorter way round, and the routes do not depend on how often
    /// they are computed.
    #[test]
    fn on_a_ring_every_pair_takes_the_shorter_way_and_the_result_is_stable() {
        let names = ["n0", "n1", "n2", "n3", "n4", "n5"];
        let nodes: Vec<(&str, bool)> = names.iter().map(|n| (*n, true)).collect();
        let ids: Vec<String> = (0..6).map(|i| format!("l{i}")).collect();
        let links: Vec<(&str, &str, &str, u32)> = (0..6)
            .map(|i| (ids[i].as_str(), names[i], names[(i + 1) % 6], 1))
            .collect();
        let ring = topo(&nodes, &links);
        assert_eq!(ring.routes(), ring.routes());
        for from in 0..6_usize {
            for to in 0..6_usize {
                if from == to {
                    continue;
                }
                let path = path_links(&ring, names[from], names[to]).unwrap();
                let clockwise = (to + 6 - from) % 6;
                assert_eq!(path.len(), clockwise.min(6 - clockwise), "{from} to {to}");
            }
        }
    }
}
