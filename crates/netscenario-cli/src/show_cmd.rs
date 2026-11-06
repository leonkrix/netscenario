//! `netscenario expand` and `netscenario show` (CLI-07): look at the network of a scenario without running anything.
//!
//! `expand` writes the scenario in its long form (a shortcut becomes nodes and links), `show` the graph, the routes and the
//! path of each workload as text, Graphviz or JSON.

use std::fmt::Write as _;
use std::io::Write;

use netscenario_model::{Destination, Impairments, Scenario, Workload};
use serde_json::{Value, json};

use crate::args::{Parsed, Spec};
use crate::exit;
use crate::validate::{ALLOW_OUTSIDE_FILES, load};

pub(crate) const EXPAND_HELP: &str = "\
Usage: netscenario expand <file> [--seed <n>] [--json] [--allow-outside-files]

Prints a scenario in its long form: a shortcut of the topology (chain, star, dumbbell, multipath) becomes the nodes
and links it makes, with their names. Comments of the file are not kept. The long form has the same canonical text and
the same hash as the shortcut, so the run is the same. Use it to learn the names of what a shortcut made (for a route or
an event of the timeline) and to start from a shortcut and change one link.

  --seed <n>  also write the models of the links and the events that repeat as the events they make with this seed, which is
              what a run with that seed does (the seed of the file if the option is left out and the file has one)
  --json    print the canonical JSON that the hash of the scenario is made of (a table made from a file of samples is in it)
  --allow-outside-files  let the scenario read files from outside the directory of the scenario file

Exit codes: 0 done, 2 the scenario is not valid or the file cannot be read.
";

pub(crate) const SHOW_HELP: &str = "\
Usage: netscenario show <file> [--format text|dot|json] [--allow-outside-files]

Shows the network of a scenario: the nodes with their roles, the links with their values, the routes of every node (the
automatic ones and the listed ones) and the path that the packets of each workload take. Nothing is run. The addresses are
assigned when a run starts and are in its result.

  --format text   a readable listing (the default)
  --format dot    a Graphviz graph (pipe it to `dot -Tsvg`)
  --format json   all of it, for tools
  --allow-outside-files  let the scenario read files from outside the directory of the scenario file

Exit codes: 0 done, 2 the scenario is not valid or the file cannot be read.
";

const EXPAND_SPEC: Spec = Spec {
    values: &["seed"],
    flags: &["json", ALLOW_OUTSIDE_FILES],
};
const SHOW_SPEC: Spec = Spec {
    values: &["format"],
    flags: &[ALLOW_OUTSIDE_FILES],
};

/// Reads the file named by the only argument, or says why not.
fn scenario_of(
    parsed: &Parsed,
    command: &str,
    help: &str,
    err: &mut impl Write,
) -> Result<Scenario, u8> {
    let path = match parsed.single("the scenario file") {
        Ok(path) => path,
        Err(message) => return Err(crate::usage_error(err, command, &message, help)),
    };
    let loaded = match load(path, parsed.flag(ALLOW_OUTSIDE_FILES)) {
        Ok(loaded) => loaded,
        Err(message) => {
            let _ = writeln!(err, "netscenario: {message}");
            return Err(exit::USAGE);
        }
    };
    loaded.scenario.map_err(|error| {
        let _ = write!(err, "{}", error.render());
        exit::USAGE
    })
}

pub(crate) fn expand<S: AsRef<str>>(args: &[S], out: &mut impl Write, err: &mut impl Write) -> u8 {
    let parsed = match Parsed::parse(args, &EXPAND_SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return crate::usage_error(err, "expand", &message, EXPAND_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{EXPAND_HELP}");
        return exit::OK;
    }
    let scenario = match scenario_of(&parsed, "expand", EXPAND_HELP, err) {
        Ok(scenario) => scenario,
        Err(code) => return code,
    };
    let scenario = match parsed.value("seed") {
        None => scenario,
        Some(text) => {
            let Ok(seed) = text.parse::<u64>() else {
                return crate::usage_error(
                    err,
                    "expand",
                    &format!("--seed needs a whole number, found '{text}'"),
                    EXPAND_HELP,
                );
            };
            match netscenario_core::expand_models(&scenario, seed) {
                Ok(mut expanded) => {
                    expanded.seed = Some(seed);
                    expanded
                }
                Err(error) => {
                    let _ = writeln!(err, "netscenario expand: {error}");
                    return exit::USAGE;
                }
            }
        }
    };
    if parsed.flag("json") {
        return match serde_json::to_string_pretty(&scenario) {
            Ok(text) => {
                let _ = writeln!(out, "{text}");
                exit::OK
            }
            Err(error) => {
                let _ = writeln!(
                    err,
                    "netscenario expand: cannot write the scenario: {error}"
                );
                exit::RUN_FAILED
            }
        };
    }
    match scenario.to_yaml() {
        Ok(text) => {
            let _ = write!(out, "{text}");
            exit::OK
        }
        Err(error) => {
            let _ = writeln!(
                err,
                "netscenario expand: cannot write the scenario: {error}"
            );
            exit::RUN_FAILED
        }
    }
}

/// The values of a direction as one short line: `delay 5ms, rate 20mbit, loss {"model":"random","p":"1%"}`.
fn describe(impairments: &Impairments) -> String {
    let Ok(Value::Object(map)) = serde_json::to_value(impairments) else {
        return String::new();
    };
    map.iter()
        .map(|(key, value)| match value {
            Value::String(text) => format!("{key} {text}"),
            other => format!("{key} {other}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The values of the two directions of a link.
fn describe_link(link: &netscenario_model::Link) -> Vec<(&'static str, String)> {
    let mut found = Vec::new();
    if let Some(both) = &link.both {
        found.push(("both", describe(both)));
    }
    if let Some(forward) = &link.forward {
        found.push(("forward", describe(forward)));
    }
    if let Some(reverse) = &link.reverse {
        found.push(("reverse", describe(reverse)));
    }
    if let Some(model) = &link.model {
        found.push(("model", describe_model(model)));
    }
    found
}

/// The model of a link as one short line: `trace forward uplink.csv (csv, 12 rows, 1m), reverse down.up (mahimahi, 80 rows, 8s)`.
fn describe_model(model: &netscenario_model::LinkModel) -> String {
    use netscenario_model::{Direction, ModelKind};
    if model.kind != ModelKind::Trace {
        let what = model.kind.word();
        return format!("{what}, {} states", model.states.len());
    }
    let parts: Vec<String> = model
        .traces()
        .iter()
        .map(|(direction, trace)| {
            let side = match direction {
                Direction::Forward => "forward",
                Direction::Reverse => "reverse",
                Direction::Both => "both",
            };
            let file = trace.file.as_deref().unwrap_or("(in the scenario)");
            let format = trace.format_of_file().map_or("?", |format| format.word());
            match &trace.points {
                Some(points) => format!(
                    "{side} {file} ({format}, {} rows, {})",
                    points.len(),
                    points.period()
                ),
                None => format!("{side} {file} ({format}, not read)"),
            }
        })
        .collect();
    format!("trace {}", parts.join(", "))
}

pub(crate) fn show<S: AsRef<str>>(args: &[S], out: &mut impl Write, err: &mut impl Write) -> u8 {
    let parsed = match Parsed::parse(args, &SHOW_SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return crate::usage_error(err, "show", &message, SHOW_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{SHOW_HELP}");
        return exit::OK;
    }
    let format = parsed.value("format").unwrap_or("text").to_owned();
    if !["text", "dot", "json"].contains(&format.as_str()) {
        let message = format!("the format '{format}' is not one of text, dot, json");
        return crate::usage_error(err, "show", &message, SHOW_HELP);
    }
    let scenario = match scenario_of(&parsed, "show", SHOW_HELP, err) {
        Ok(scenario) => scenario,
        Err(code) => return code,
    };
    let text = match format.as_str() {
        "dot" => dot(&scenario),
        "json" => serde_json::to_string_pretty(&as_json(&scenario)).unwrap_or_default() + "\n",
        _ => listing(&scenario),
    };
    let _ = write!(out, "{text}");
    exit::OK
}

/// The path of each workload that sends packets: its name, the ends, and the path or why there is none.
fn paths(
    scenario: &Scenario,
) -> Vec<(
    String,
    String,
    String,
    Result<netscenario_model::Path, String>,
)> {
    let routes = scenario.topology.routes();
    scenario
        .workloads
        .iter()
        .filter_map(|workload| {
            let generator = match workload {
                Workload::Command(_) => return None,
                other => other.generator()?,
            };
            let path = scenario
                .topology
                .path(&routes, generator.from, generator.to)
                .map_err(|error| format!("{error:?}"));
            Some((
                workload.id().to_string(),
                generator.from.to_string(),
                generator.to.to_string(),
                path,
            ))
        })
        .collect()
}

fn listing(scenario: &Scenario) -> String {
    let topology = &scenario.topology;
    let mut text = String::new();
    let _ = writeln!(
        text,
        "{}: {} nodes, {} links, routing {}{}{}\n",
        scenario.name,
        topology.nodes.len(),
        topology.links.len(),
        match topology.routing {
            netscenario_model::Routing::Auto => "auto",
            netscenario_model::Routing::Manual => "manual",
        },
        if topology.ecmp { ", ecmp" } else { "" },
        match topology.ip {
            netscenario_model::IpMode::V4 => "",
            netscenario_model::IpMode::V6 => ", IPv6",
            netscenario_model::IpMode::Dual => ", IPv4 and IPv6",
        }
    );
    let _ = writeln!(text, "nodes");
    for node in &topology.nodes {
        let _ = writeln!(text, "  {:<24} {}", node.id, node.role.as_str());
    }
    let _ = writeln!(
        text,
        "\nlinks (forward is from the first node to the second)"
    );
    for link in &topology.links {
        let cost = link
            .cost
            .map_or(String::new(), |cost| format!("  cost {cost}"));
        let mtu = link.mtu.map_or(String::new(), |mtu| format!("  mtu {mtu}"));
        let _ = writeln!(
            text,
            "  {:<24} {} - {}{cost}{mtu}",
            link.id, link.between[0], link.between[1]
        );
        for (direction, values) in describe_link(link) {
            let _ = writeln!(text, "      {direction:<8} {values}");
        }
    }
    let routes = topology.routes();
    let _ = writeln!(
        text,
        "\nroutes (the smaller metric is used first; * is listed in the file)"
    );
    for entry in &routes.entries {
        let _ = writeln!(
            text,
            "  {:<24} to {:<24} via {:<24} next {:<16} metric {}{}{}",
            entry.node,
            entry.to,
            entry.link,
            entry.next_hop,
            entry.metric,
            if entry.weight == 1 {
                String::new()
            } else {
                format!(" weight {}", entry.weight)
            },
            if entry.given { " *" } else { "" }
        );
    }
    let _ = writeln!(text, "\npaths of the workloads");
    for (id, from, to, path) in paths(scenario) {
        match path {
            Ok(path) => {
                let mut line = String::new();
                for (index, node) in path.nodes.iter().enumerate() {
                    if index > 0 {
                        let _ = write!(line, " -[{}]-> ", path.links[index - 1]);
                    }
                    line.push_str(node.as_str());
                }
                let _ = writeln!(
                    text,
                    "  {id}: {line}{}",
                    if path.shared {
                        "  (shared with another route of the same metric)"
                    } else {
                        ""
                    }
                );
            }
            Err(why) => {
                let _ = writeln!(text, "  {id}: {from} to {to}: no path ({why})");
            }
        }
    }
    text
}

fn dot(scenario: &Scenario) -> String {
    let topology = &scenario.topology;
    let mut text = String::from(
        "graph netscenario {\n  rankdir=LR;\n  node [fontname=\"sans-serif\"];\n  edge [fontname=\"sans-serif\", fontsize=10];\n",
    );
    for node in &topology.nodes {
        let shape = match node.role {
            netscenario_model::Role::Host => "box",
            netscenario_model::Role::Router => "ellipse",
        };
        let _ = writeln!(text, "  \"{}\" [shape={shape}];", node.id);
    }
    for link in &topology.links {
        let mut label = link.id.to_string();
        for (direction, values) in describe_link(link) {
            if !values.is_empty() {
                let _ = write!(label, "\\n{direction}: {}", values.replace('"', "'"));
            }
        }
        let _ = writeln!(
            text,
            "  \"{}\" -- \"{}\" [label=\"{label}\"];",
            link.between[0], link.between[1]
        );
    }
    text.push_str("}\n");
    text
}

fn as_json(scenario: &Scenario) -> Value {
    let topology = &scenario.topology;
    let routes = topology.routes();
    json!({
        "name": scenario.name.as_str(),
        "routing": topology.routing,
        "ecmp": topology.ecmp,
        "ip": topology.ip,
        "nodes": topology.nodes,
        "links": topology.links,
        "routes": routes.entries.iter().map(|entry| json!({
            "node": entry.node.as_str(),
            "to": match &entry.to {
                Destination::Node(name) => name.as_str().to_owned(),
                Destination::Default => "default".to_owned(),
            },
            "link": entry.link.as_str(),
            "next_hop": entry.next_hop.as_str(),
            "metric": entry.metric,
            "weight": entry.weight,
            "given": entry.given,
        })).collect::<Vec<_>>(),
        "paths": paths(scenario).into_iter().map(|(workload, from, to, path)| match path {
            Ok(path) => json!({
                "workload": workload, "from": from, "to": to,
                "nodes": path.nodes.iter().map(|n| n.as_str().to_owned()).collect::<Vec<_>>(),
                "links": path.links.iter().map(|n| n.as_str().to_owned()).collect::<Vec<_>>(),
                "shared": path.shared,
            }),
            Err(why) => json!({ "workload": workload, "from": from, "to": to, "error": why }),
        }).collect::<Vec<_>>(),
    })
}
