//! Models over time at the start of a run (D-090): the models of the links and the events that repeat become the events of the
//! timeline, drawn from the seed of the run, and everything after that is the run of a scenario with a timeline.

use std::borrow::Cow;

use netscenario_agent::rng::Xoshiro256PlusPlus;
use netscenario_model::{Scenario, Uniform};

use crate::error::Error;
use crate::seed::{derive_model_seed, draw_seed};

/// The scenario a run does.
#[derive(Debug)]
pub(crate) struct Compiled<'a> {
    /// The scenario with the models and the repeating events replaced by their events, and the seed that was drawn for the models
    /// if the file has none. The scenario itself if it has neither.
    pub(crate) scenario: Cow<'a, Scenario>,
    /// Whether the seed was drawn for the models: the result says so, as it does for any seed that the file does not have.
    pub(crate) seed_drawn: bool,
    /// The place in the timeline of the scenario of the first event that a model made.
    pub(crate) first_generated: usize,
}

/// Compiles the models of `scenario`, which must be valid ([`Scenario::validate`]).
///
/// # Errors
///
/// A model that would make more events than the bound allows.
pub(crate) fn compile(scenario: &Scenario) -> Result<Compiled<'_>, Error> {
    if !scenario.has_models() {
        return Ok(Compiled {
            scenario: Cow::Borrowed(scenario),
            seed_drawn: false,
            first_generated: scenario.timeline.len(),
        });
    }
    let (seed, seed_drawn) = match scenario.seed {
        Some(seed) => (seed, false),
        None if scenario.has_random_models() => (draw_seed(), true),
        None => (0, false),
    };
    let compiled = compile_seeded(scenario, seed)?;
    let first_generated = compiled.first_generated;
    let mut compiled = compiled.scenario;
    if seed_drawn {
        compiled.seed = Some(seed);
    }
    Ok(Compiled {
        scenario: Cow::Owned(compiled),
        seed_drawn,
        first_generated,
    })
}

/// The scenario with its models and repeating events replaced by the events they make with `seed` (what `netscenario expand
/// --seed` shows, and what a run with that seed does). The scenario must be valid ([`Scenario::validate`]).
///
/// # Errors
///
/// A model that would make more events than the bound allows.
pub fn expand_models(scenario: &Scenario, seed: u64) -> Result<Scenario, Error> {
    Ok(compile_seeded(scenario, seed)?.scenario)
}

fn compile_seeded(scenario: &Scenario, seed: u64) -> Result<netscenario_model::Compiled, Error> {
    scenario
        .compile(&mut |link| {
            let mut generator =
                Xoshiro256PlusPlus::from_seed(derive_model_seed(seed, link.as_str()));
            let numbers: Uniform = Box::new(move || generator.unit());
            numbers
        })
        .map_err(|problem| Error::InvalidScenario(vec![problem]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline;

    const HEAD: &str =
        "version: 0\nname: t\nduration: 30s\ntopology:\n  nodes: [a, b, c]\n  links:\n";
    const TAIL: &str = "workloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";
    const PERIODIC: &str = "      model:\n        kind: periodic\n        states:\n          - { for: 4s, set: { both: { rate: 1mbit, loss: { model: random, p: 2% } } } }\n          - { for: 2s, outage: true }\n          - { for: 3s, down: true }\n";
    const CHAIN: &str = "      model:\n        kind: markov\n        start: one\n        states:\n          - { id: one, stay: 2s, to: { two: 100% }, set: { both: { rate: 1mbit } } }\n          - { id: two, stay: 1s, to: { one: 70%, three: 30% }, set: { both: { rate: 2mbit } } }\n          - { id: three, stay: 500ms, to: { one: 100% } }\n";

    fn scenario(seed: &str, links: &str) -> Scenario {
        // Every node has a link: the third node hangs on a plain one unless the test brings its own
        let tail = if links.contains("[b, c]") {
            ""
        } else {
            "    - { id: tail, between: [b, c] }\n"
        };
        Scenario::from_yaml(&format!("{seed}{HEAD}{links}{tail}{TAIL}"), "t.yaml")
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn link(id: &str, between: &str, model: &str) -> String {
        format!("    - id: {id}\n      between: {between}\n      both: {{ delay: 10ms }}\n{model}")
    }

    /// A model is nothing but the events it makes: the steps of the run are the steps of a scenario in which the same events are
    /// written by hand, in the order the model makes them (the end of a `down`, the change of the values, the outage or `down`).
    #[test]
    fn the_steps_of_a_model_are_the_steps_of_the_events_written_by_hand() {
        let modelled = scenario("", &link("l", "[a, b]", PERIODIC));
        let by_hand = Scenario::from_yaml(
            &format!(
                "{HEAD}{}    - {{ id: tail, between: [b, c] }}\n{TAIL}timeline:\n  - {{ at: 0s, link: l, set: {{ both: {{ rate: 1mbit, loss: {{ model: random, p: 2% }} }} }} }}\n  - {{ at: 4s, link: l, set: {{ both: {{ rate: null, loss: null }} }} }}\n  - {{ at: 4s, link: l, outage: 2s }}\n  - {{ at: 6s, link: l, down: true }}\n  - {{ at: 9s, link: l, up: true }}\n  - {{ at: 9s, link: l, set: {{ both: {{ rate: 1mbit, loss: {{ model: random, p: 2% }} }} }} }}\n  - {{ at: 13s, link: l, set: {{ both: {{ rate: null, loss: null }} }} }}\n  - {{ at: 13s, link: l, outage: 2s }}\n  - {{ at: 15s, link: l, down: true }}\n  - {{ at: 18s, link: l, up: true }}\n  - {{ at: 18s, link: l, set: {{ both: {{ rate: 1mbit, loss: {{ model: random, p: 2% }} }} }} }}\n  - {{ at: 22s, link: l, set: {{ both: {{ rate: null, loss: null }} }} }}\n  - {{ at: 22s, link: l, outage: 2s }}\n  - {{ at: 24s, link: l, down: true }}\n  - {{ at: 27s, link: l, up: true }}\n  - {{ at: 27s, link: l, set: {{ both: {{ rate: 1mbit, loss: {{ model: random, p: 2% }} }} }} }}\n",
                link("l", "[a, b]", "")
            ),
            "t.yaml",
        )
        .unwrap();
        let compiled = compile(&modelled).unwrap();
        assert!(!compiled.seed_drawn);
        let key = |scenario: &Scenario| {
            timeline::steps(scenario)
                .into_iter()
                .map(|step| (step.at, step.kind, step.action))
                .collect::<Vec<_>>()
        };
        let generated = key(&compiled.scenario);
        let written = key(&by_hand);
        assert_eq!(generated.len(), written.len());
        for (generated, written) in generated.iter().zip(&written) {
            assert_eq!(generated, written);
        }
    }

    #[test]
    fn a_scenario_without_models_is_run_as_it_is() {
        let plain = scenario("", &link("l", "[a, b]", ""));
        let compiled = compile(&plain).unwrap();
        assert!(matches!(compiled.scenario, Cow::Borrowed(_)));
        assert_eq!(compiled.first_generated, 0);
    }

    #[test]
    fn a_seed_is_drawn_for_a_chain_and_not_for_a_pattern() {
        let chain = scenario("", &link("l", "[a, b]", CHAIN));
        let compiled = compile(&chain).unwrap();
        assert!(compiled.seed_drawn);
        assert!(compiled.scenario.seed.is_some());
        let pattern = scenario("", &link("l", "[a, b]", PERIODIC));
        let compiled = compile(&pattern).unwrap();
        assert!(!compiled.seed_drawn);
        assert_eq!(compiled.scenario.seed, None);
        // A seed in the file is the seed, and is not drawn
        let seeded = scenario("seed: 9\n", &link("l", "[a, b]", CHAIN));
        let compiled = compile(&seeded).unwrap();
        assert!(!compiled.seed_drawn);
        assert_eq!(compiled.scenario.seed, Some(9));
    }

    #[test]
    fn the_same_seed_makes_the_same_events_and_another_seed_others() {
        let chain = scenario("", &link("l", "[a, b]", CHAIN));
        let events = |seed| expand_models(&chain, seed).unwrap().timeline;
        assert_eq!(events(3), events(3));
        assert_ne!(events(3), events(4));
        assert!(events(3).len() > 5);
    }

    /// The numbers of a link come from the seed and its name: a model on another link, or a link before it, does not change them.
    #[test]
    fn the_events_of_a_link_do_not_depend_on_the_other_links() {
        let both = format!(
            "{}{}",
            link("first", "[a, b]", CHAIN),
            link("second", "[b, c]", CHAIN)
        );
        let alone = format!(
            "{}{}",
            link("first", "[a, b]", ""),
            link("second", "[b, c]", CHAIN)
        );
        let of = |links: &str, name: &str| -> Vec<_> {
            expand_models(&scenario("", links), 11)
                .unwrap()
                .timeline
                .into_iter()
                .filter(|event| event.link.as_str() == name)
                .collect()
        };
        assert_eq!(of(&both, "second"), of(&alone, "second"));
        // Canary: two links with the same model do not draw the same numbers
        let first = of(&both, "first");
        let second = of(&both, "second");
        assert!(first.iter().map(|e| e.at).ne(second.iter().map(|e| e.at)));
    }
}
