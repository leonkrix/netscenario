//! The shortcuts for the common shapes (D-078): `chain`, `star`, `dumbbell` and `multipath`.
//!
//! A shortcut is written under `topology` instead of `nodes` and `links`, and is expanded when the file is read, so the
//! scenario that comes out has the canonical form and the hash of its long form. The names of what is generated are fixed
//! and documented: they are the names of the nodes with a `-` between them (`client-edge`), `router-left`,
//! `router-right` and `bottleneck` for the dumbbell.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::scenario::{Impairments, Link};
use crate::topology::{MAX_LINKS, MAX_NODES, Node};
use crate::units::Name;

/// The values of a link that a shortcut makes: what a link has besides its name and its ends.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkSpec {
    /// What the link costs the automatic routing. 1 by default.
    #[serde(default)]
    pub cost: Option<u32>,
    /// The MTU of the link in bytes. 1500 by default.
    #[serde(default)]
    pub mtu: Option<u32>,
    /// The impairments from the first node of the link to the second.
    #[serde(default)]
    pub forward: Option<Impairments>,
    /// The impairments from the second node to the first.
    #[serde(default)]
    pub reverse: Option<Impairments>,
    /// The same impairments in both directions.
    #[serde(default)]
    pub both: Option<Impairments>,
}

impl LinkSpec {
    fn link(&self, id: Name, between: [Name; 2]) -> Link {
        let mut link = Link::new(id, between);
        link.cost = self.cost;
        link.mtu = self.mtu;
        link.forward.clone_from(&self.forward);
        link.reverse.clone_from(&self.reverse);
        link.both.clone_from(&self.both);
        link
    }
}

/// A line of nodes: the two ends are hosts, the nodes between them are routers, each pair is joined by a link.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Chain {
    /// The nodes from one end to the other: at least two. The first link's forward direction is from the first node.
    pub nodes: Vec<Name>,
    /// The values of every link, unless `links` has its own.
    #[serde(default)]
    pub link: Option<LinkSpec>,
    /// The values of each link in turn (one fewer than nodes), instead of `link`.
    #[serde(default)]
    pub links: Vec<LinkSpec>,
}

/// A hub that is a router, with hosts around it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Star {
    /// The name of the hub (a router).
    pub hub: Name,
    /// The hosts around it: at least two. Each link is between a leaf and the hub, so its forward direction is towards the hub.
    pub leaves: Vec<Name>,
    /// The values of every link, unless `links` has its own.
    #[serde(default)]
    pub link: Option<LinkSpec>,
    /// The values of the link of each leaf in turn, instead of `link`.
    #[serde(default)]
    pub links: Vec<LinkSpec>,
}

/// Hosts on two sides of a shared link between two routers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dumbbell {
    /// The hosts on the left: at least one. They are joined to the router `router-left`.
    pub left: Vec<Name>,
    /// The hosts on the right: at least one. They are joined to the router `router-right`.
    pub right: Vec<Name>,
    /// The values of the link of each host to its router; its forward direction is from the host to the router.
    #[serde(default)]
    pub access: Option<LinkSpec>,
    /// The values of the link `bottleneck` between the two routers; its forward direction is from left to right.
    #[serde(default)]
    pub bottleneck: Option<LinkSpec>,
}

/// One path of a multipath: the routers it passes, in order, and the values of its links.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathSpec {
    /// The routers on the path from the first host to the second: at least one.
    pub routers: Vec<Name>,
    /// The values of every link of the path.
    #[serde(default)]
    pub link: Option<LinkSpec>,
}

/// Two hosts joined by several paths, each over routers of its own.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Multipath {
    /// The host that the forward direction of the links starts at.
    pub from: Name,
    /// The other host.
    pub to: Name,
    /// The paths: at least two.
    pub paths: Vec<PathSpec>,
}

/// What a shortcut makes: the nodes and the links, in a fixed order.
pub(crate) type Made = (Vec<Node>, Vec<Link>);

fn joined(first: &Name, second: &Name) -> Result<Name, String> {
    Name::new(&format!("{first}-{second}")).map_err(|error| {
        format!(
            "the link between '{first}' and '{second}' would be called '{first}-{second}': {error}; use shorter names, or write `nodes` and `links` (`netscenario expand` shows the long form)"
        )
    })
}

fn spec_or_none(spec: Option<&LinkSpec>) -> LinkSpec {
    spec.cloned().unwrap_or(LinkSpec {
        cost: None,
        mtu: None,
        forward: None,
        reverse: None,
        both: None,
    })
}

/// The spec of the link number `index`: its own from `links`, else the common `link`.
fn pick(link: Option<&LinkSpec>, links: &[LinkSpec], index: usize) -> LinkSpec {
    links
        .get(index)
        .cloned()
        .unwrap_or_else(|| spec_or_none(link))
}

fn check_count(what: &str, count: usize, expected: usize, key: &str) -> Result<(), String> {
    if count != 0 && count != expected {
        return Err(format!(
            "`{key}` has {count} entries for {expected} {what}: write one for each, or none and use the common one"
        ));
    }
    Ok(())
}

impl Chain {
    pub(crate) fn make(&self) -> Result<Made, String> {
        let count = self.nodes.len();
        if count < 2 {
            return Err(format!("a chain needs at least two nodes, found {count}"));
        }
        check_count("links", self.links.len(), count - 1, "links")?;
        let nodes = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, name)| {
                if index == 0 || index == count - 1 {
                    Node::host(name.clone())
                } else {
                    Node::router(name.clone())
                }
            })
            .collect();
        let mut links = Vec::new();
        for (index, pair) in self.nodes.windows(2).enumerate() {
            let spec = pick(self.link.as_ref(), &self.links, index);
            links.push(spec.link(
                joined(&pair[0], &pair[1])?,
                [pair[0].clone(), pair[1].clone()],
            ));
        }
        Ok((nodes, links))
    }
}

impl Star {
    pub(crate) fn make(&self) -> Result<Made, String> {
        let count = self.leaves.len();
        if count < 2 {
            return Err(format!(
                "a star needs at least two leaves, found {count}: with one it is a link"
            ));
        }
        check_count("leaves", self.links.len(), count, "links")?;
        let mut nodes = vec![Node::router(self.hub.clone())];
        nodes.extend(self.leaves.iter().cloned().map(Node::host));
        let mut links = Vec::new();
        for (index, leaf) in self.leaves.iter().enumerate() {
            let spec = pick(self.link.as_ref(), &self.links, index);
            links.push(spec.link(joined(leaf, &self.hub)?, [leaf.clone(), self.hub.clone()]));
        }
        Ok((nodes, links))
    }
}

/// The name of the left router of a dumbbell.
pub const ROUTER_LEFT: &str = "router-left";
/// The name of the right router of a dumbbell.
pub const ROUTER_RIGHT: &str = "router-right";
/// The name of the shared link of a dumbbell.
pub const BOTTLENECK: &str = "bottleneck";

impl Dumbbell {
    pub(crate) fn make(&self) -> Result<Made, String> {
        if self.left.is_empty() || self.right.is_empty() {
            return Err("a dumbbell needs at least one host on each side".to_owned());
        }
        let left_router = Name::new(ROUTER_LEFT).map_err(|e| e.to_string())?;
        let right_router = Name::new(ROUTER_RIGHT).map_err(|e| e.to_string())?;
        let access = spec_or_none(self.access.as_ref());
        let mut nodes: Vec<Node> = self.left.iter().cloned().map(Node::host).collect();
        nodes.push(Node::router(left_router.clone()));
        nodes.push(Node::router(right_router.clone()));
        nodes.extend(self.right.iter().cloned().map(Node::host));
        let mut links = Vec::new();
        for leaf in &self.left {
            links.push(access.link(
                joined(leaf, &left_router)?,
                [leaf.clone(), left_router.clone()],
            ));
        }
        links.push(spec_or_none(self.bottleneck.as_ref()).link(
            Name::new(BOTTLENECK).map_err(|e| e.to_string())?,
            [left_router.clone(), right_router.clone()],
        ));
        for leaf in &self.right {
            links.push(access.link(
                joined(leaf, &right_router)?,
                [leaf.clone(), right_router.clone()],
            ));
        }
        Ok((nodes, links))
    }
}

impl Multipath {
    pub(crate) fn make(&self) -> Result<Made, String> {
        if self.paths.len() < 2 {
            return Err(format!(
                "a multipath needs at least two paths, found {}: with one it is a chain",
                self.paths.len()
            ));
        }
        let mut nodes = vec![Node::host(self.from.clone())];
        let mut links = Vec::new();
        for path in &self.paths {
            if path.routers.is_empty() {
                return Err("a path needs at least one router".to_owned());
            }
            let mut previous = self.from.clone();
            let spec = spec_or_none(path.link.as_ref());
            for router in path.routers.iter().chain(std::iter::once(&self.to)) {
                links.push(spec.link(
                    joined(&previous, router)?,
                    [previous.clone(), router.clone()],
                ));
                previous = router.clone();
            }
            nodes.extend(path.routers.iter().cloned().map(Node::router));
        }
        nodes.push(Node::host(self.to.clone()));
        Ok((nodes, links))
    }
}

/// Whether the made network is within the bounds of a scenario and has no node twice, with a message that names the shortcut.
pub(crate) fn check_made(made: &Made, shortcut: &str) -> Result<(), String> {
    for (index, node) in made.0.iter().enumerate() {
        if made.0[..index].iter().any(|other| other.id == node.id) {
            let id = node.id.as_str();
            let help = if shortcut == "dumbbell" && (id == ROUTER_LEFT || id == ROUTER_RIGHT) {
                format!(
                    " ('{ROUTER_LEFT}' and '{ROUTER_RIGHT}' are the routers that a dumbbell makes)"
                )
            } else {
                String::new()
            };
            return Err(format!(
                "the name '{id}' is used twice in the {shortcut}{help}: every node needs its own name"
            ));
        }
    }
    if made.0.len() > MAX_NODES || made.1.len() > MAX_LINKS {
        return Err(format!(
            "the shortcut makes {} nodes and {} links, and a scenario may have at most {MAX_NODES} nodes and {MAX_LINKS} links",
            made.0.len(),
            made.1.len()
        ));
    }
    Ok(())
}
