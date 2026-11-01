//! A change writes the whole state into the queues of a direction (D-048, D-059): from any state to any other, the
//! queues are what a fresh queue in the target state is, and no setting of an earlier state is left over.
//!
//! `netem` keeps `corrupt`, `reorder`, `rate` and `slot` across a change that does not mention them, and the table
//! of the distribution of a jitter (measured on the machine of the maintainer and in `docs/DECISIONS.md`). A
//! test that goes from one state to the next only would find a leftover only for the pairs it tried, so this one
//! goes from every state to every state, and compares the queues with a fresh set built for the target.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::{Duration, Instant};

use netscenario_core::{Backend, Direction, LinkHandle, NodeHandle, Program};
use netscenario_model::{Impairments, Name};
use support::{assert_nothing_left, backend, batch_of, namespaces_available, scenario, tools};

/// The impairments written as the keys of a `both`.
fn state(text: &str) -> Impairments {
    scenario("", &format!("      both: {text}\n"))
        .topology
        .links[0]
        .both
        .clone()
        .unwrap()
}

/// What a program wrote to its standard output, once it has ended.
fn output(backend: &mut impl Backend, node: NodeHandle, argv: Vec<String>) -> String {
    let program = backend.start_program(node, &Program::new(argv)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut text = String::new();
    loop {
        let state = backend
            .poll_program(program, Duration::from_millis(100))
            .unwrap();
        for line in state.lines {
            text.push_str(&line);
            text.push('\n');
        }
        if let Some(exit) = state.exit {
            assert!(exit.success(), "{exit}: {}", state.stderr);
            return text;
        }
        assert!(Instant::now() < deadline, "the program did not end");
    }
}

/// The queues of an interface, without what differs from run to run: the kind, the place in the tree, and the
/// options without the seed and the handle.
fn queues(backend: &mut impl Backend, node: NodeHandle, interface: &str) -> Vec<serde_json::Value> {
    let tc = tools().tc.display().to_string();
    let text = output(
        backend,
        node,
        vec![
            tc,
            "-j".into(),
            "qdisc".into(),
            "show".into(),
            "dev".into(),
            interface.into(),
        ],
    );
    let all: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    all.into_iter()
        .map(|mut qdisc| {
            let mut options = qdisc["options"].take();
            if let Some(options) = options.as_object_mut() {
                options.remove("seed");
            }
            serde_json::json!({
                "kind": qdisc["kind"],
                "parent": qdisc.get("parent"),
                "options": options,
            })
        })
        .collect()
}

/// The states of a direction that one `netem` is built for.
fn single_queue_states() -> Vec<(&'static str, Impairments)> {
    vec![
        ("nothing", Impairments::default()),
        ("delay", state("{ delay: 20ms }")),
        (
            "delay with a jitter",
            state("{ delay: 20ms, jitter: { amount: 5ms } }"),
        ),
        ("loss", state("{ loss: { model: random, p: 5% } }")),
        (
            "burst loss",
            state(
                "{ loss: { model: gilbert-elliott, p: 1%, r: 20%, loss-bad: 100%, loss-good: 0% } }",
            ),
        ),
        (
            "four-state loss",
            state("{ loss: { model: markov4, p13: 2%, p31: 30%, p32: 20%, p23: 10%, p14: 1% } }"),
        ),
        ("duplication", state("{ duplicate: 10% }")),
        ("corruption", state("{ corrupt: 10% }")),
        (
            "reordering",
            state("{ delay: 10ms, reorder: { share: 25%, gap: 3 } }"),
        ),
        ("rate", state("{ rate: 2mbit }")),
        (
            "everything but a jitter with a rate",
            state(
                "{ delay: 10ms, loss: { model: random, p: 5% }, duplicate: 10%, corrupt: 10%, reorder: { share: 25% }, rate: 2mbit }",
            ),
        ),
    ]
}

/// What two queues are built for: the same and a jitter with a rate, a reordering with a rate.
fn two_queue_states() -> Vec<(&'static str, Impairments)> {
    let mut states = single_queue_states();
    states.retain(|(name, _)| *name != "everything but a jitter with a rate");
    states.extend([
        (
            "a jitter with a rate",
            state("{ delay: 20ms, jitter: { amount: 5ms }, rate: 2mbit }"),
        ),
        (
            "all of it",
            state("{ delay: 10ms, jitter: { amount: 2ms }, loss: { model: random, p: 5% }, duplicate: 10%, corrupt: 10%, reorder: { share: 25% }, rate: 2mbit }"),
        ),
    ]);
    states
}

/// A set of directions built fresh for each state, and one that is changed through all of them.
struct Walk {
    backend: netscenario_backend_kernel::KernelBackend,
    nodes: [NodeHandle; 2],
    /// The link whose forward direction is changed. The links built fresh in one state each follow it in the
    /// order of the states (`queues_of` reads them by their number).
    main: LinkHandle,
}

impl Walk {
    fn new(states: &[(&str, Impairments)]) -> Self {
        let mut backend = backend(|_| {});
        let nodes = [
            backend.create_node(&Name::new("a").unwrap()).unwrap(),
            backend.create_node(&Name::new("b").unwrap()).unwrap(),
        ];
        let all: Vec<Impairments> = states.iter().map(|(_, state)| state.clone()).collect();
        let build = |backend: &mut netscenario_backend_kernel::KernelBackend,
                     name: String,
                     first: &Impairments| {
            let link = backend
                .create_link(&Name::new(&name).unwrap(), nodes)
                .unwrap();
            backend.plan_states(link, Direction::Forward, &all).unwrap();
            // The same seed everywhere: it is in the queue, and the comparison leaves it out
            backend
                .apply(link, Direction::Forward, first, Some(1))
                .unwrap();
            link
        };
        let main = build(&mut backend, "main".to_owned(), &states[0].1);
        for (i, (_, state)) in states.iter().enumerate() {
            build(&mut backend, format!("fresh{i}"), state);
        }
        Self {
            backend,
            nodes,
            main,
        }
    }

    /// The queues of the forward direction of link number `index` (0 is the main one).
    fn queues_of(&mut self, index: usize) -> Vec<serde_json::Value> {
        queues(&mut self.backend, self.nodes[0], &format!("l{index}"))
    }
}

/// From every state to every state, the queues are those of a fresh queue in the target state.
fn walk_through_every_pair(states: &[(&str, Impairments)]) {
    let mut walk = Walk::new(states);
    let references: Vec<Vec<serde_json::Value>> =
        (0..states.len()).map(|i| walk.queues_of(i + 1)).collect();
    for (from, (from_name, from_state)) in states.iter().enumerate() {
        for (to, (to_name, to_state)) in states.iter().enumerate() {
            walk.backend
                .change(walk.main, Direction::Forward, from_state)
                .unwrap();
            walk.backend
                .change(walk.main, Direction::Forward, to_state)
                .unwrap();
            assert_eq!(
                walk.queues_of(0),
                references[to],
                "after '{from_name}' ({from}) and then '{to_name}' ({to}) the queues are not those of a fresh '{to_name}'"
            );
        }
    }
    // Every change went through the batch of the node and none had to be made the slow way (D-063): a batch that
    // could not be started, or stopped working, is a note
    assert_eq!(walk.backend.notes(), Vec::<String>::new());
    walk.backend.teardown().unwrap();
    assert_nothing_left(walk.backend.run_id(), "after the teardown");
}

#[test]
fn from_every_state_to_every_state_one_queue_is_a_fresh_queue() {
    if !namespaces_available() {
        return;
    }
    walk_through_every_pair(&single_queue_states());
}

#[test]
fn from_every_state_to_every_state_two_queues_are_fresh_queues() {
    if !namespaces_available() {
        return;
    }
    walk_through_every_pair(&two_queue_states());
}

/// Canary: the comparison can fail. A change that is made the way release 0.1 would have made it, with the keys of
/// the target only, leaves `corrupt`, `reorder` and `rate` of the state before, and the queues are not those of
/// a fresh one. If this passed, the test above would prove nothing.
#[test]
fn canary_a_change_without_the_zeros_leaves_the_settings_of_the_state_before() {
    if !namespaces_available() {
        return;
    }
    let states = single_queue_states();
    let mut walk = Walk::new(&states);
    let fresh_delay = {
        let index = states
            .iter()
            .position(|(name, _)| *name == "delay")
            .unwrap();
        walk.queues_of(index + 1)
    };
    // Corrupt, reorder and rate in force, then a change by hand that mentions only a delay
    let all = &states[states.len() - 1].1;
    walk.backend
        .change(walk.main, Direction::Forward, all)
        .unwrap();
    let tc = tools().tc.display().to_string();
    let _ = output(
        &mut walk.backend,
        walk.nodes[0],
        vec![
            tc,
            "qdisc".into(),
            "change".into(),
            "dev".into(),
            "l0".into(),
            "root".into(),
            "netem".into(),
            "limit".into(),
            "1000".into(),
            "delay".into(),
            "20ms".into(),
        ],
    );
    let left_over = walk.queues_of(0);
    assert_ne!(left_over, fresh_delay);
    let options = left_over[0]["options"].to_string();
    assert!(
        options.contains("corrupt") && options.contains("rate"),
        "{options}"
    );
    walk.backend.teardown().unwrap();
}

/// The distribution of the jitter cannot go back from normal to uniform in the same queue: the plan refuses the
/// states before anything is built, with the reason.
#[test]
fn a_jitter_that_is_normal_in_one_state_and_uniform_in_another_cannot_be_planned() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|_| {});
    let nodes = [
        backend.create_node(&Name::new("a").unwrap()).unwrap(),
        backend.create_node(&Name::new("b").unwrap()).unwrap(),
    ];
    let link = backend
        .create_link(&Name::new("l").unwrap(), nodes)
        .unwrap();
    let normal = state("{ delay: 20ms, jitter: { amount: 5ms, distribution: normal } }");
    let uniform = state("{ delay: 20ms, jitter: { amount: 5ms } }");
    let error = backend
        .plan_states(link, Direction::Forward, &[normal.clone(), uniform])
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("normal distribution in one state and a uniform one in another"),
        "{error}"
    );
    // Canary: the same distribution in two states is fine, and so is a state without a jitter in between
    let still_normal = state("{ delay: 40ms, jitter: { amount: 1ms, distribution: normal } }");
    backend
        .plan_states(
            link,
            Direction::Forward,
            &[normal, state("{ delay: 20ms }"), still_normal],
        )
        .unwrap();
    backend.teardown().unwrap();
}

/// A change of a direction that was not planned says so instead of guessing.
#[test]
fn a_change_without_a_plan_is_refused() {
    if !namespaces_available() {
        return;
    }
    let mut backend = backend(|_| {});
    let nodes = [
        backend.create_node(&Name::new("a").unwrap()).unwrap(),
        backend.create_node(&Name::new("b").unwrap()).unwrap(),
    ];
    let link = backend
        .create_link(&Name::new("l").unwrap(), nodes)
        .unwrap();
    backend
        .apply(link, Direction::Forward, &state("{ delay: 5ms }"), None)
        .unwrap();
    let error = backend
        .change(link, Direction::Forward, &state("{ delay: 10ms }"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("was not planned"), "{error}");
    backend.teardown().unwrap();
}

/// A `tc` that stops answering is replaced by a process for the change, the change is the same, and the result says
/// so (D-063). The batch is stopped with `SIGSTOP`, which is what a hang looks like from outside (S11 saw one on a
/// runner). The change that meets it waits for the time limit of the batch, so this test takes about two seconds.
#[test]
fn a_batch_that_stops_answering_is_replaced_by_a_process_and_the_state_is_the_same() {
    if !namespaces_available() {
        return;
    }
    let states = two_queue_states();
    let mut walk = Walk::new(&states);
    let references: Vec<Vec<serde_json::Value>> =
        (0..states.len()).map(|i| walk.queues_of(i + 1)).collect();
    let run_id = walk.backend.run_id().to_owned();
    let batch = batch_of(&run_id).expect("the node has a tc batch while the run stands");
    assert_eq!(walk.backend.notes(), Vec::<String>::new());
    // The first change works, the batch is stopped, and the next changes meet a tc that does not answer
    walk.backend
        .change(walk.main, Direction::Forward, &states[1].1)
        .unwrap();
    let stopped = std::process::Command::new("sh")
        .args(["-c", &format!("kill -STOP {batch}")])
        .status()
        .unwrap();
    assert!(stopped.success());
    let before = Instant::now();
    walk.backend
        .change(walk.main, Direction::Forward, &states[2].1)
        .unwrap();
    let waited = before.elapsed();
    // The wait is the limit of the batch (2 s) and then the change by a process
    assert!(
        waited >= Duration::from_millis(1900) && waited < Duration::from_secs(8),
        "{waited:?}"
    );
    assert_eq!(walk.queues_of(0), references[2], "after the replacement");
    // The batch is ended and the next changes go by a process, without a wait
    assert!(
        batch_of(&run_id).is_none(),
        "the stopped batch is still there"
    );
    let before = Instant::now();
    walk.backend
        .change(walk.main, Direction::Forward, &states[3].1)
        .unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(walk.queues_of(0), references[3]);
    // One note, not one for each change
    let notes = walk.backend.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("stopped working") && notes[0].contains("did not answer in time"),
        "{}",
        notes[0]
    );
    walk.backend.teardown().unwrap();
    assert_nothing_left(&run_id, "after the teardown");
}

/// One batch serves every interface of a node: the changes of two links that leave the same node go through the same
/// `tc` and each lands on its own link (D-063). The batch has no filter or parent left over from the line before:
/// `tc` keeps some between the lines of a batch, and this is the case where that would show.
#[test]
fn the_changes_of_two_links_of_one_node_go_through_one_batch_and_each_lands_on_its_own_link() {
    if !namespaces_available() {
        return;
    }
    let one = state("{ delay: 20ms, loss: { model: random, p: 10% } }");
    let two = state("{ delay: 30ms, jitter: { amount: 5ms }, rate: 5mbit }");
    let states = [one.clone(), two.clone()];
    let mut backend = backend(|_| {});
    let nodes = [
        backend.create_node(&Name::new("a").unwrap()).unwrap(),
        backend.create_node(&Name::new("b").unwrap()).unwrap(),
        backend.create_node(&Name::new("c").unwrap()).unwrap(),
    ];
    let mut links = Vec::new();
    for (name, other) in [("first", nodes[1]), ("second", nodes[2])] {
        let link = backend
            .create_link(&Name::new(name).unwrap(), [nodes[0], other])
            .unwrap();
        backend
            .plan_states(link, Direction::Forward, &states)
            .unwrap();
        backend
            .apply(link, Direction::Forward, &one, Some(1))
            .unwrap();
        links.push(link);
    }
    // The queues of the two links in the two states, from fresh queues
    let mut expected = Vec::new();
    for (i, link) in links.iter().enumerate() {
        let mut both = Vec::new();
        for target in &states {
            backend.change(*link, Direction::Forward, target).unwrap();
            both.push(queues(&mut backend, nodes[0], &format!("l{i}")));
        }
        expected.push(both);
    }
    // Alternating between the links and the states: each change lands where it was sent, and only there
    for round in 0..6_usize {
        let link = round % 2;
        let target = (round / 2) % 2;
        backend
            .change(links[link], Direction::Forward, &states[target])
            .unwrap();
        assert_eq!(
            queues(&mut backend, nodes[0], &format!("l{link}")),
            expected[link][target],
            "round {round}: link {link} after a change to state {target}"
        );
    }
    assert_eq!(backend.notes(), Vec::<String>::new());
    let run_id = backend.run_id().to_owned();
    backend.teardown().unwrap();
    assert_nothing_left(&run_id, "after the teardown");
}
