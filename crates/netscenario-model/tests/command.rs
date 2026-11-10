//! The faults in a program of your own (`kind: command`, WRK-06, D-072) that the files of `scenarios/invalid/` do not show: the limits
//! (the number of arguments, the size of a text, the number of variables), braces, and the waits of `ready` and `shutdown`. Each is refused
//! with the place of the fault; a scenario that is a little less wrong is accepted, so the bound is where the message says.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{MAX_ARGUMENT_BYTES, MAX_ARGUMENTS, MAX_ENV_ENTRIES, Scenario};

/// A scenario with one program; `command` is the lines under the program (indented by 4 spaces).
fn with_program(command: &str) -> String {
    format!(
        "version: 0\nname: t\nduration: 5s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\nworkloads:\n  - id: app\n    kind: command\n    node: server\n{command}"
    )
}

/// What the reader says about a scenario, or `None` if it accepts it.
fn refusal(yaml: &str) -> Option<String> {
    Scenario::from_yaml(yaml, "t.yaml")
        .err()
        .map(|error| error.to_string())
}

fn refused(command: &str) -> String {
    refusal(&with_program(command)).unwrap_or_else(|| panic!("accepted:\n{command}"))
}

fn accepted(command: &str) {
    if let Some(message) = refusal(&with_program(command)) {
        panic!("refused:\n{command}\n{message}");
    }
}

#[test]
fn a_program_may_have_as_many_arguments_as_the_limit_and_not_one_more() {
    let list = |count: usize| format!("    run: [{}]\n", vec!["\"a\""; count].join(", "));
    accepted(&list(MAX_ARGUMENTS));
    let message = refused(&list(MAX_ARGUMENTS + 1));
    assert!(
        message.contains(&format!(
            "`run` has {} entries, at most {MAX_ARGUMENTS} are accepted",
            MAX_ARGUMENTS + 1
        )),
        "{message}"
    );
    assert!(message.contains("workloads[0].run"), "{message}");
}

#[test]
fn the_program_cannot_be_an_empty_text_but_an_argument_may() {
    let message = refused("    run: [\"\"]\n");
    assert!(
        message.contains("the program is an empty text"),
        "{message}"
    );
    assert!(message.contains("run[0]"), "{message}");
    accepted("    run: [\"./server\", \"\"]\n");
}

#[test]
fn a_text_may_have_as_many_bytes_as_the_limit_and_not_one_more() {
    let argument = |bytes: usize| format!("    run: [\"./server\", \"{}\"]\n", "x".repeat(bytes));
    accepted(&argument(MAX_ARGUMENT_BYTES));
    let message = refused(&argument(MAX_ARGUMENT_BYTES + 1));
    assert!(
        message.contains(&format!(
            "this text has {} bytes, at most {MAX_ARGUMENT_BYTES} are accepted",
            MAX_ARGUMENT_BYTES + 1
        )),
        "{message}"
    );
    assert!(message.contains("run[1]"), "{message}");
}

#[test]
fn a_program_may_have_as_many_variables_as_the_limit_and_not_one_more() {
    let variables = |count: usize| {
        let entries: Vec<String> = (0..count).map(|i| format!("V{i}: \"1\"")).collect();
        format!(
            "    run: [\"./server\"]\n    env: {{ {} }}\n",
            entries.join(", ")
        )
    };
    accepted(&variables(MAX_ENV_ENTRIES));
    let message = refused(&variables(MAX_ENV_ENTRIES + 1));
    assert!(
        message.contains(&format!(
            "`env` has {} variables, at most {MAX_ENV_ENTRIES} are accepted",
            MAX_ENV_ENTRIES + 1
        )),
        "{message}"
    );
}

#[test]
fn a_brace_is_a_placeholder_or_is_written_twice() {
    let message = refused("    run: [\"./server\", \"a}b\"]\n");
    assert!(
        message.contains("a closing brace without an opening one"),
        "{message}"
    );
    assert!(
        message.contains("`}}`"),
        "the help says how to write a brace: {message}"
    );
    let message = refused("    run: [\"./server\", \"a{b\"]\n");
    assert!(
        message.contains("an opening brace without a closing one"),
        "{message}"
    );
    // Twice is a brace; a placeholder of a node that exists is an address
    accepted("    run: [\"./server\", \"a}}b{{c\", \"{server.addr}\"]\n");
}

#[test]
fn a_way_to_be_ready_with_an_empty_text_or_a_wait_of_zero_is_refused() {
    let empty = refused("    run: [\"./server\"]\n    ready: { stdout-line: \"\", within: 5s }\n");
    assert!(empty.contains("this text is empty"), "{empty}");
    assert!(empty.contains("ready.stdout-line"), "{empty}");
    let file = refused("    run: [\"./server\"]\n    ready: { file: \"\", within: 5s }\n");
    assert!(file.contains("ready.file"), "{file}");
    let zero_delay = refused("    run: [\"./server\"]\n    ready: { delay: 0s }\n");
    assert!(
        zero_delay.contains("a delay of zero is no wait"),
        "{zero_delay}"
    );
    let zero_within =
        refused("    run: [\"./server\"]\n    ready: { udp-port: 4433, within: 0s }\n");
    assert!(
        zero_within.contains("`within` must be longer than zero"),
        "{zero_within}"
    );
    assert!(zero_within.contains("ready.within"), "{zero_within}");
    accepted("    run: [\"./server\"]\n    ready: { udp-port: 4433, within: 1ms }\n");
    accepted("    run: [\"./server\"]\n    ready: { delay: 1ms }\n");
}

#[test]
fn a_shutdown_that_waits_for_zero_seconds_is_refused() {
    let message = refused("    run: [\"./server\"]\n    shutdown: { signal: TERM, within: 0s }\n");
    assert!(
        message.contains("`within` must be longer than zero"),
        "{message}"
    );
    assert!(message.contains("shutdown.within"), "{message}");
    assert!(
        message.contains("`KILL`"),
        "the help says what within is: {message}"
    );
    accepted("    run: [\"./server\"]\n    shutdown: { signal: TERM, within: 1ms }\n");
}
