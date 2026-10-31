//! The rules of a `command` workload (D-041, D-051): the node must exist, the arguments and the environment
//! stay within limits and use only the placeholder `{node.addr}`, and `ready` and `shutdown` mean something.

use crate::error::Problem;
use crate::scenario::{Command, Ready, Shutdown};
use crate::suggest::closest;
use crate::units::Name;
use crate::validate::unknown_node;

/// The most arguments of a program (the kernel's limit is far higher; a scenario file has no reason to come
/// near this).
pub const MAX_ARGUMENTS: usize = 256;
/// The most bytes of one argument or one value of the environment.
pub const MAX_ARGUMENT_BYTES: usize = 4096;
/// The most variables of the environment that a scenario sets.
pub const MAX_ENV_ENTRIES: usize = 64;
/// The variables that the tool sets start with this; a scenario cannot set them.
pub const RESERVED_ENV_PREFIX: &str = "NETSCENARIO_";

pub(crate) fn check(
    command: &Command,
    base: &str,
    nodes: &[Name],
    nodes_ok: bool,
    problems: &mut Vec<Problem>,
) {
    if nodes_ok && !nodes.contains(&command.node) {
        problems.push(unknown_node(&command.node, nodes, format!("{base}.node")));
    }
    check_run(command, base, nodes, nodes_ok, problems);
    check_env(command, base, nodes, nodes_ok, problems);
    if let Some(ready) = &command.ready {
        check_ready(ready, &format!("{base}.ready"), problems);
    }
    if let Some(shutdown) = &command.shutdown {
        check_shutdown(shutdown, &format!("{base}.shutdown"), problems);
    }
}

fn check_run(
    command: &Command,
    base: &str,
    nodes: &[Name],
    nodes_ok: bool,
    problems: &mut Vec<Problem>,
) {
    let path = format!("{base}.run");
    if command.run.is_empty() {
        problems.push(
            Problem::new("`run` is empty: it needs the program and its arguments")
                .at_path(path.clone())
                .with_help("write the program first, for example `run: [\"./server\", \"--port\", \"4433\"]`"),
        );
    }
    if command.run.len() > MAX_ARGUMENTS {
        problems.push(
            Problem::new(format!(
                "`run` has {} entries, at most {MAX_ARGUMENTS} are accepted",
                command.run.len()
            ))
            .at_path(path.clone()),
        );
    }
    for (index, argument) in command.run.iter().enumerate() {
        let at = format!("{path}[{index}]");
        if index == 0 && argument.is_empty() {
            problems.push(Problem::new("the program is an empty text").at_path(at.clone()));
        }
        check_text(argument, &at, nodes, nodes_ok, problems);
    }
}

fn check_env(
    command: &Command,
    base: &str,
    nodes: &[Name],
    nodes_ok: bool,
    problems: &mut Vec<Problem>,
) {
    let path = format!("{base}.env");
    if command.env.len() > MAX_ENV_ENTRIES {
        problems.push(
            Problem::new(format!(
                "`env` has {} variables, at most {MAX_ENV_ENTRIES} are accepted",
                command.env.len()
            ))
            .at_path(path.clone()),
        );
    }
    for (key, value) in &command.env {
        let at = format!("{path}.{key}");
        let mut characters = key.chars();
        let valid = characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && characters.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            problems.push(
                Problem::new(format!(
                    "'{key}' is not the name of an environment variable"
                ))
                .at_path(at.clone())
                .with_help("letters, digits and '_', not starting with a digit"),
            );
        } else if key.starts_with(RESERVED_ENV_PREFIX) {
            problems.push(
                Problem::new(format!(
                    "the variables that start with {RESERVED_ENV_PREFIX} are set by the tool"
                ))
                .at_path(at.clone())
                .with_help("choose another name; the program finds the addresses of the nodes in the variables of the tool"),
            );
        }
        check_text(value, &at, nodes, nodes_ok, problems);
    }
}

/// An argument or a value: not too long, no NUL (a program cannot be given one), and the braces mean
/// something.
fn check_text(text: &str, path: &str, nodes: &[Name], nodes_ok: bool, problems: &mut Vec<Problem>) {
    if text.len() > MAX_ARGUMENT_BYTES {
        problems.push(
            Problem::new(format!(
                "this text has {} bytes, at most {MAX_ARGUMENT_BYTES} are accepted",
                text.len()
            ))
            .at_path(path.to_owned()),
        );
        return;
    }
    if text.contains('\0') {
        problems.push(
            Problem::new("this text contains a NUL character, which a program cannot be given")
                .at_path(path.to_owned()),
        );
        return;
    }
    let mut rest = text;
    while let Some(position) = rest.find(['{', '}']) {
        let after = &rest[position + 1..];
        if rest.as_bytes()[position] == b'}' {
            if let Some(next) = after.strip_prefix('}') {
                rest = next;
            } else {
                problems.push(
                    Problem::new("a closing brace without an opening one")
                        .at_path(path.to_owned())
                        .with_help("a brace that is meant as a brace is written twice: `}}`"),
                );
                rest = after;
            }
        } else if let Some(next) = after.strip_prefix('{') {
            rest = next;
        } else if let Some(end) = after.find('}') {
            check_placeholder(&after[..end], path, nodes, nodes_ok, problems);
            rest = &after[end + 1..];
        } else {
            problems.push(
                Problem::new("an opening brace without a closing one")
                    .at_path(path.to_owned())
                    .with_help("a brace that is meant as a brace is written twice: `{{`"),
            );
            return;
        }
    }
}

/// The one placeholder there is: `{node.addr}`, the address of a node.
fn check_placeholder(
    inside: &str,
    path: &str,
    nodes: &[Name],
    nodes_ok: bool,
    problems: &mut Vec<Problem>,
) {
    let Some(node) = inside.strip_suffix(".addr") else {
        problems.push(
            Problem::new(format!("'{{{inside}}}' is not a placeholder"))
                .at_path(path.to_owned())
                .with_help("the only placeholder is `{node.addr}`, the address of a node; a brace that is meant as a brace is written twice"),
        );
        return;
    };
    if !nodes_ok {
        return;
    }
    let known: Vec<&str> = nodes.iter().map(Name::as_str).collect();
    if !known.contains(&node) {
        let list = format!("the nodes are: {}", known.join(", "));
        let help = match closest(node, &known) {
            Some(near) => format!("did you mean '{near}'? {list}"),
            None => list,
        };
        problems.push(
            Problem::new(format!(
                "the node '{node}' in '{{{inside}}}' does not exist"
            ))
            .at_path(path.to_owned())
            .with_help(help),
        );
    }
}

fn check_ready(ready: &Ready, path: &str, problems: &mut Vec<Problem>) {
    let ways: Vec<&str> = [
        ("udp-port", ready.udp_port.is_some()),
        ("tcp-port", ready.tcp_port.is_some()),
        ("stdout-line", ready.stdout_line.is_some()),
        ("file", ready.file.is_some()),
        ("delay", ready.delay.is_some()),
    ]
    .into_iter()
    .filter(|(_, written)| *written)
    .map(|(name, _)| name)
    .collect();
    if ways.len() != 1 {
        let found = if ways.is_empty() {
            "none".to_owned()
        } else {
            ways.join(", ")
        };
        problems.push(
            Problem::new(format!(
                "`ready` needs exactly one of `udp-port`, `tcp-port`, `stdout-line`, `file` and `delay`, found {found}"
            ))
            .at_path(path.to_owned())
            .with_help("leave `ready` out if the program can be used when it is started"),
        );
    }
    for (key, port) in [("udp-port", ready.udp_port), ("tcp-port", ready.tcp_port)] {
        if port == Some(0) {
            problems.push(
                Problem::new("port 0 is not a port that a program listens on")
                    .at_path(format!("{path}.{key}")),
            );
        }
    }
    for (key, text) in [("stdout-line", &ready.stdout_line), ("file", &ready.file)] {
        if text.as_deref().is_some_and(str::is_empty) {
            problems.push(Problem::new("this text is empty").at_path(format!("{path}.{key}")));
        }
        if text.as_deref().is_some_and(|text| text.contains('\0')) {
            problems.push(
                Problem::new("this text contains a NUL character").at_path(format!("{path}.{key}")),
            );
        }
    }
    match (ready.delay, ready.within) {
        (Some(_), Some(_)) => problems.push(
            Problem::new("`within` does not go with `delay`: the delay is the whole wait")
                .at_path(format!("{path}.within"))
                .with_help("take `within` away, or use another way to be ready"),
        ),
        (Some(delay), None) if delay.is_zero() => problems.push(
            Problem::new("a delay of zero is no wait")
                .at_path(format!("{path}.delay"))
                .with_help("leave `ready` out if the program can be used when it is started"),
        ),
        (None, None) if ways.len() == 1 => problems.push(
            Problem::new("`ready` needs `within`: how long to wait for the program")
                .at_path(path.to_owned())
                .with_help("a program that is not ready after that time fails the run; add for example `within: 10s`"),
        ),
        (None, Some(within)) if within.is_zero() => problems.push(
            Problem::new("`within` must be longer than zero").at_path(format!("{path}.within")),
        ),
        _ => {}
    }
}

fn check_shutdown(shutdown: &Shutdown, path: &str, problems: &mut Vec<Problem>) {
    if shutdown.within.is_some_and(crate::units::Duration::is_zero) {
        problems.push(
            Problem::new("`within` must be longer than zero")
                .at_path(format!("{path}.within"))
                .with_help("it is the time between the signal and `KILL`"),
        );
    }
}
