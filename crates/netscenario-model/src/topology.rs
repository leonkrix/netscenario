//! The network of a scenario: nodes with a role, links between them, and the routes (D-078).
//!
//! A scenario writes one canonical form, nodes and links. A node is written as its name (a host) or as a mapping with a
//! role; a node that is written as a name serialises as a name again, so a scenario of release 0.1 or 0.2 keeps the text
//! and the hash it had.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::scenario::Link;
use crate::shortcut::{Chain, Dumbbell, Multipath, Star};
use crate::units::Name;

/// The most nodes a scenario may have. The Kernel backend refuses fewer by default (the doctor says why); this is the
/// bound at which a file is not a scenario any more.
pub const MAX_NODES: usize = 256;
/// The most links a scenario may have.
pub const MAX_LINKS: usize = 1024;
/// The most routes a scenario may list.
pub const MAX_ROUTES: usize = 8192;
/// The word that stands for "every destination" in a route. No node and no link is called so.
pub const DEFAULT_DESTINATION: &str = "default";
/// The metric of a route that has none: the automatic routes have it too, so that a given route with a smaller metric
/// is preferred and one with a larger metric is the second choice.
pub const DEFAULT_METRIC: u32 = 100;

/// What a node does with packets that are not for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// An end system: it sends and receives, and does not forward. The default.
    #[default]
    Host,
    /// It forwards packets between its links.
    Router,
}

impl Role {
    /// The name in the file.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Router => "router",
        }
    }
}

/// A node: a name and a role. Written as `client` (a host) or as `{ id: r1, role: router }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Node {
    /// The name of the node.
    pub id: Name,
    /// What it does with packets that are not for it.
    pub role: Role,
}

impl Node {
    /// A host.
    #[must_use]
    pub fn host(id: Name) -> Self {
        Self {
            id,
            role: Role::Host,
        }
    }

    /// A router.
    #[must_use]
    pub fn router(id: Name) -> Self {
        Self {
            id,
            role: Role::Router,
        }
    }
}

/// The long form of a node in the schema.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "describes the long form for the schema")]
struct NodeLong {
    /// The name of the node.
    id: Name,
    /// What the node does with packets that are not for it. A host by default.
    #[serde(default)]
    role: Role,
}

impl JsonSchema for Node {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Node".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let name = generator.subschema_for::<Name>();
        let long = generator.subschema_for::<NodeLong>();
        json_schema!({
            "description": "A node: its name (a host), or a mapping with the name and a role.",
            "anyOf": [name, long]
        })
    }
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.role == Role::Host {
            return self.id.serialize(serializer);
        }
        let mut state = serializer.serialize_struct("Node", 2)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("role", &self.role)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NodeVisitor;

        impl<'de> Visitor<'de> for NodeVisitor {
            type Value = Node;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a node name, or a mapping with `id` and `role`")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Node, E> {
                Name::new(text).map(Node::host).map_err(E::custom)
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
                const FIELDS: &[&str] = &["id", "role"];
                let mut id: Option<Name> = None;
                let mut role: Option<Role> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" => {
                            if id.replace(map.next_value()?).is_some() {
                                return Err(de::Error::duplicate_field("id"));
                            }
                        }
                        "role" => {
                            if role.replace(map.next_value()?).is_some() {
                                return Err(de::Error::duplicate_field("role"));
                            }
                        }
                        other => return Err(de::Error::unknown_field(other, FIELDS)),
                    }
                }
                let id = id.ok_or_else(|| de::Error::missing_field("id"))?;
                Ok(Node {
                    id,
                    role: role.unwrap_or_default(),
                })
            }
        }

        deserializer.deserialize_any(NodeVisitor)
    }
}

/// How the routes of the nodes come about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Routing {
    /// The tool computes a route from every node to every node by the shortest path (the sum of the `cost` of the links,
    /// 1 by default), and the routes that are listed add to those or replace the routes of the same node and
    /// destination. The default.
    #[default]
    Auto,
    /// Only the listed routes exist.
    Manual,
}

impl Routing {
    // serde's `skip_serializing_if` passes a reference
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::Auto
    }
}

/// A route of a node: to a destination, out of one of its links. Several routes of the same node and destination form a
/// failover (different metrics: the smallest is used while its link is up) or a multipath (the same metric, shared by
/// `weight`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// The node that has the route.
    pub node: Name,
    /// The destination: a node (its address on the loopback of the node) or `default` for every destination that has no
    /// route of its own.
    pub to: Name,
    /// The link that the packets leave by; the node at its other end is the next hop. A link and not a neighbour, so that
    /// two links between the same nodes are told apart.
    pub via: Name,
    /// The preference: the smaller is used first. 100 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<u32>,
    /// The share of a multipath route, 1 by default. Routes with the same metric share the packets by their weights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
}

impl Route {
    /// A route with the default metric and weight.
    #[must_use]
    pub fn new(node: Name, to: Name, via: Name) -> Self {
        Self {
            node,
            to,
            via,
            metric: None,
            weight: None,
        }
    }

    /// The metric, with the default if none is written.
    #[must_use]
    pub fn metric(&self) -> u32 {
        self.metric.unwrap_or(DEFAULT_METRIC)
    }

    /// The weight, with the default if none is written.
    #[must_use]
    pub fn weight(&self) -> u32 {
        self.weight.unwrap_or(1)
    }

    /// Whether the destination is every destination.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.to.as_str() == DEFAULT_DESTINATION
    }
}

/// The nodes, the links and the routes of a scenario.
///
/// In a file the nodes and links are written out, or given as one of the shortcuts `chain`, `star`, `dumbbell` and
/// `multipath` (D-078), which are expanded when the file is read: what is held and written again is always the long form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "TopologyForm")]
#[schemars(with = "TopologyForm")]
#[non_exhaustive]
pub struct Topology {
    /// The nodes: a name for a host, or `{ id, role }`.
    pub nodes: Vec<Node>,
    /// The links between the nodes. Two nodes may have several.
    pub links: Vec<Link>,
    /// How the routes come about: `auto` (the default) or `manual`.
    #[serde(default, skip_serializing_if = "Routing::is_default")]
    pub routing: Routing,
    /// With `auto`: install every equally short path as a multipath route, not only the first. Off by default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ecmp: bool,
    /// Routes of single nodes: they add to the automatic routes, or replace those of the same node and destination, or
    /// (with `routing: manual`) are the only ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
}

/// The topology as a file writes it: nodes and links, or one shortcut, and the routing.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Topology")]
struct TopologyForm {
    /// The nodes: a name for a host, or `{ id, role }`. Not together with a shortcut.
    #[serde(default)]
    nodes: Option<Vec<Node>>,
    /// The links between the nodes. Two nodes may have several. Not together with a shortcut.
    #[serde(default)]
    links: Option<Vec<Link>>,
    /// A line of nodes: hosts at the ends, routers between.
    #[serde(default)]
    chain: Option<Chain>,
    /// A router with hosts around it.
    #[serde(default)]
    star: Option<Star>,
    /// Hosts on two sides of a link between two routers.
    #[serde(default)]
    dumbbell: Option<Dumbbell>,
    /// Two hosts joined by several paths over routers of their own.
    #[serde(default)]
    multipath: Option<Multipath>,
    /// How the routes come about: `auto` (the default) or `manual`.
    #[serde(default)]
    routing: Routing,
    /// With `auto`: install every equally short path as a multipath route, not only the first. Off by default.
    #[serde(default)]
    ecmp: bool,
    /// Routes of single nodes.
    #[serde(default)]
    routes: Vec<Route>,
}

impl TryFrom<TopologyForm> for Topology {
    type Error = String;

    fn try_from(form: TopologyForm) -> Result<Self, String> {
        let shortcuts: Vec<&str> = [
            ("chain", form.chain.is_some()),
            ("star", form.star.is_some()),
            ("dumbbell", form.dumbbell.is_some()),
            ("multipath", form.multipath.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect();
        let (nodes, links) = if shortcuts.is_empty() {
            match (form.nodes, form.links) {
                (Some(nodes), Some(links)) => (nodes, links),
                (None, _) => {
                    return Err(
                        "a topology needs `nodes` and `links`, or one of the shortcuts `chain`, `star`, `dumbbell` and `multipath`"
                            .to_owned(),
                    );
                }
                (Some(_), None) => {
                    return Err("a topology with `nodes` needs `links` as well".to_owned());
                }
            }
        } else {
            if shortcuts.len() > 1 {
                return Err(format!(
                    "a topology is one shortcut, found {}: {}",
                    shortcuts.len(),
                    shortcuts.join(", ")
                ));
            }
            if form.nodes.is_some() || form.links.is_some() {
                return Err(format!(
                    "`{}` cannot be combined with `nodes` or `links`: it makes them; to change what it makes, write the long form (`netscenario expand` prints it)",
                    shortcuts[0]
                ));
            }
            let made = if let Some(chain) = &form.chain {
                chain.make()?
            } else if let Some(star) = &form.star {
                star.make()?
            } else if let Some(dumbbell) = &form.dumbbell {
                dumbbell.make()?
            } else if let Some(multipath) = &form.multipath {
                multipath.make()?
            } else {
                unreachable!("one shortcut is present")
            };
            crate::shortcut::check_made(&made, shortcuts[0])?;
            made
        };
        Ok(Self {
            nodes,
            links,
            routing: form.routing,
            ecmp: form.ecmp,
            routes: form.routes,
        })
    }
}

impl Topology {
    /// Nodes and links, with the automatic routing.
    #[must_use]
    pub fn new(nodes: Vec<Node>, links: Vec<Link>) -> Self {
        Self {
            nodes,
            links,
            routing: Routing::Auto,
            ecmp: false,
            routes: Vec::new(),
        }
    }

    /// The node called `name`.
    #[must_use]
    pub fn node(&self, name: &Name) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == *name)
    }

    /// The link called `name`.
    #[must_use]
    pub fn link(&self, name: &Name) -> Option<&Link> {
        self.links.iter().find(|link| link.id == *name)
    }

    /// Whether this is the network of release 0.1: two nodes, both hosts, and one link.
    #[must_use]
    pub fn is_point_to_point(&self) -> bool {
        self.nodes.len() == 2
            && self.links.len() == 1
            && self.nodes.iter().all(|node| node.role == Role::Host)
            && self.routes.is_empty()
            && self.routing == Routing::Auto
    }
}
