//! Models of a link over time and repeating events (LMD-04, LMD-05, TML-03; D-090): what the reader accepts and refuses, and the
//! events that a model makes (`Scenario::compile`).

// A test file: a failing unwrap is a failing test
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    // Times of seconds to hours in nanoseconds are far below the precision of a double
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use netscenario_model::{Edit, Event, MAX_EVENTS_PER_LINK, Scenario, Uniform};

fn head(duration: &str, link: &str, timeline: &str) -> String {
    format!(
        "version: 0\nname: t\nduration: {duration}\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {{ delay: 10ms }}\n{link}workloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n{timeline}"
    )
}

fn read(yaml: &str) -> Scenario {
    Scenario::from_yaml(yaml, "t.yaml").unwrap_or_else(|error| panic!("{error}"))
}

fn refused(yaml: &str) -> String {
    match Scenario::from_yaml(yaml, "t.yaml") {
        Ok(_) => panic!("the scenario was accepted:\n{yaml}"),
        Err(error) => error.to_string(),
    }
}

/// A source of numbers for the links of a model that draws none.
fn none(_: &netscenario_model::Name) -> Uniform {
    Box::new(|| panic!("a periodic model draws no number"))
}

fn compile(scenario: &Scenario) -> Scenario {
    scenario.compile(&mut none).unwrap().scenario
}

/// `SplitMix64` as a source of numbers in `(0, 1]`: any good generator will do for these tests.
fn numbers(seed: u64) -> Uniform {
    let mut state = seed;
    Box::new(move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        ((z >> 11) + 1) as f64 / (1_u64 << 53) as f64
    })
}

fn at(event: &Event) -> f64 {
    event.at.as_nanos() as f64 / 1e9
}

const PERIODIC: &str = "      model:\n        kind: periodic\n        states:\n          - { for: 2s, set: { both: { rate: 1mbit } } }\n          - { for: 3s, set: { both: { loss: { model: random, p: 5% } } } }\n";

#[test]
fn a_periodic_model_makes_a_step_at_the_start_of_every_state_and_repeats() {
    let compiled = compile(&read(&head("12s", PERIODIC, "")));
    let moments: Vec<f64> = compiled.timeline.iter().map(at).collect();
    assert_eq!(moments, [0.0, 2.0, 5.0, 7.0, 10.0]);
    assert!(compiled.topology.links[0].model.is_none());
    // Entering the first state: the rate is set. The second state: the rate goes back to what the link says (none) and the loss
    // is set. The first again: the loss goes back.
    let both = |index: usize| {
        compiled.timeline[index]
            .set
            .as_ref()
            .unwrap()
            .both
            .clone()
            .unwrap()
    };
    assert_eq!(both(0).rate, Edit::Set("1mbit".parse().unwrap()));
    assert_eq!(both(0).loss, Edit::Keep);
    assert_eq!(both(1).rate, Edit::Clear);
    assert!(matches!(both(1).loss, Edit::Set(_)));
    assert_eq!(both(2).loss, Edit::Clear);
    assert_eq!(both(2).rate, Edit::Set("1mbit".parse().unwrap()));
    assert_eq!(compiled.validate(), Vec::new());
}

/// The period is exact: the moments are sums of the lengths, with no draw and no drift.
#[test]
fn the_period_is_exact_over_a_long_run() {
    let yaml = head(
        "1000s",
        "      model:\n        kind: periodic\n        states:\n          - { for: 333ms, set: { both: { rate: 1mbit } } }\n          - { for: 667ms }\n",
        "",
    );
    let compiled = compile(&read(&yaml));
    // 1000 s hold 1000 periods of 1 s, and a step at the start of each state
    assert_eq!(compiled.timeline.len(), 2000);
    for (index, event) in compiled.timeline.iter().enumerate() {
        let expected = (index / 2) as u64 * 1_000_000_000 + (index % 2) as u64 * 333_000_000;
        assert_eq!(event.at.as_nanos(), expected, "event {index}");
    }
}

#[test]
fn a_state_means_the_same_whatever_came_before_it() {
    // The first state writes the loss, the second the rate, the third nothing: the third is the link as it is written
    let yaml = head(
        "9s",
        "      model:\n        kind: periodic\n        states:\n          - { for: 3s, set: { both: { loss: { model: random, p: 5% } } } }\n          - { for: 3s, set: { both: { rate: 1mbit } } }\n          - { for: 3s }\n",
        "",
    );
    let compiled = compile(&read(&yaml));
    let last = compiled.timeline.last().unwrap();
    assert_eq!(at(last), 6.0);
    let change = last.set.as_ref().unwrap().both.as_ref().unwrap();
    assert_eq!((change.rate, change.loss), (Edit::Clear, Edit::Keep));
}

#[test]
fn a_model_that_does_not_repeat_leaves_the_link_as_the_file_says_after_its_last_state() {
    let yaml = head(
        "20s",
        "      model:\n        kind: periodic\n        repeat: false\n        begin: 1s\n        states:\n          - { for: 2s, set: { both: { rate: 1mbit } } }\n          - { for: 3s, set: { both: { rate: 2mbit } } }\n",
        "",
    );
    let compiled = compile(&read(&yaml));
    let moments: Vec<f64> = compiled.timeline.iter().map(at).collect();
    assert_eq!(moments, [1.0, 3.0, 6.0]);
    let end = compiled.timeline[2]
        .set
        .as_ref()
        .unwrap()
        .both
        .clone()
        .unwrap();
    assert_eq!(end.rate, Edit::Clear);
}

#[test]
fn an_outage_state_is_an_outage_for_the_stay_and_a_down_state_takes_the_interface_down_for_it() {
    let yaml = head(
        "10s",
        "      model:\n        kind: periodic\n        states:\n          - { for: 3s }\n          - { for: 2s, outage: true, direction: forward }\n          - { for: 1s, down: true }\n",
        "",
    );
    let compiled = compile(&read(&yaml));
    let list: Vec<(f64, &str)> = compiled
        .timeline
        .iter()
        .map(|event| {
            let what = if event.outage.is_some() {
                "outage"
            } else if event.down.is_some() {
                "down"
            } else if event.up.is_some() {
                "up"
            } else {
                "set"
            };
            (at(event), what)
        })
        .collect();
    assert_eq!(
        list,
        [(3.0, "outage"), (5.0, "down"), (6.0, "up"), (9.0, "outage")]
    );
    assert_eq!(compiled.timeline[0].outage, Some("2s".parse().unwrap()));
    assert_eq!(compiled.validate(), Vec::new());
}

#[test]
fn an_outage_that_reaches_past_the_end_is_cut_there() {
    let yaml = head(
        "10s",
        "      model:\n        kind: periodic\n        states:\n          - { for: 8s }\n          - { for: 5s, outage: true }\n",
        "",
    );
    let compiled = compile(&read(&yaml));
    assert_eq!(compiled.timeline.len(), 1);
    assert_eq!(compiled.timeline[0].outage, Some("2s".parse().unwrap()));
    assert_eq!(compiled.validate(), Vec::new());
}

#[test]
fn an_event_that_repeats_is_its_copies() {
    let timeline = |event: &str| format!("timeline:\n  - {{ at: 2s, link: l, {event} }}\n");
    let copies = |event: &str, duration: &str| -> Vec<f64> {
        compile(&read(&head(duration, "", &timeline(event))))
            .timeline
            .iter()
            .map(at)
            .collect()
    };
    let outage = "outage: 100ms, every: 3s";
    // Until the end of the measured phase
    assert_eq!(copies(outage, "10s"), [2.0, 5.0, 8.0]);
    // Until a moment (before it), or a number of times
    assert_eq!(
        copies("outage: 100ms, every: 3s, until: 8s", "10s"),
        [2.0, 5.0]
    );
    assert_eq!(
        copies("outage: 100ms, every: 3s, count: 2", "30s"),
        [2.0, 5.0]
    );
    // An event that does not repeat is one event
    assert_eq!(copies("outage: 100ms", "10s"), [2.0]);
}

#[test]
fn the_reader_refuses_what_makes_a_repeat_unclear() {
    let timeline = |event: &str| format!("timeline:\n  - {{ at: 2s, link: l, {event} }}\n");
    let message = |event: &str| refused(&head("10s", "", &timeline(event)));
    assert!(message("outage: 1s, every: 0s").contains("`every` must be longer than zero"));
    assert!(
        message("outage: 1s, every: 3s, until: 8s, count: 2")
            .contains("`until` and `count` both say when the repeating ends")
    );
    assert!(message("outage: 1s, count: 2").contains("`count` belongs to an event that repeats"));
    assert!(message("outage: 1s, every: 3s, until: 1s").contains("`until` (1s) must be after"));
    // The copies are checked as events: two outages that overlap are one problem of the copies
    assert!(message("outage: 4s, every: 3s").contains("overlaps the outage"));
}

#[test]
fn a_repeat_that_would_make_too_many_events_is_refused() {
    let timeline =
        "timeline:\n  - { at: 0s, link: l, set: { both: { rate: 1mbit } }, every: 1ms }\n";
    let message = refused(&head("60s", "", timeline));
    assert!(
        message.contains(&format!("more than {MAX_EVENTS_PER_LINK} times")),
        "{message}"
    );
}

#[test]
fn a_periodic_model_that_would_make_too_many_events_is_refused_when_the_file_is_read() {
    let link = "      model:\n        kind: periodic\n        states:\n          - { for: 1ms, set: { both: { rate: 1mbit } } }\n          - { for: 1ms }\n";
    let message = refused(&head("60s", link, ""));
    assert!(
        message.contains(&format!("more than {MAX_EVENTS_PER_LINK} events")),
        "{message}"
    );
    // Canary: the same model over a measured phase that fits is accepted
    let _ = read(&head("5s", link, ""));
}

#[test]
fn the_keys_of_the_other_kind_are_refused_each_at_its_own_line() {
    let periodic = "      model:\n        kind: periodic\n        start: good\n        states:\n          - { for: 1s, stay: 2s }\n";
    let message = refused(&head("10s", periodic, ""));
    assert!(
        message.contains("`start` belongs to a markov model, and this one is periodic"),
        "{message}"
    );
    assert!(
        message.contains("`stay` belongs to a markov model"),
        "{message}"
    );
    let markov = "      model:\n        kind: markov\n        repeat: false\n        start: good\n        states:\n          - { id: good, stay: 1s, for: 1s, to: { bad: 100% } }\n          - { id: bad, stay: 1s, to: { good: 100% } }\n";
    let message = refused(&head("10s", markov, ""));
    assert!(
        message.contains("`repeat` belongs to a periodic model"),
        "{message}"
    );
    assert!(
        message.contains("`for` belongs to a periodic model"),
        "{message}"
    );
}

#[test]
fn the_chances_of_a_chain_are_checked() {
    let chain = |good: &str, bad: &str, start: &str| {
        format!(
            "      model:\n        kind: markov\n        start: {start}\n        states:\n          - {{ id: good, stay: 1s, to: {good} }}\n          - {{ id: bad, stay: 1s, to: {bad} }}\n"
        )
    };
    let message =
        |good: &str, bad: &str, start: &str| refused(&head("10s", &chain(good, bad, start), ""));
    assert!(message("{ bad: 60% }", "{ good: 100% }", "good").contains("add up to 60%, not 100%"));
    assert!(
        message("{ good: 100% }", "{ good: 100% }", "good").contains("does not lead to itself")
    );
    let unknown = message("{ bat: 100% }", "{ good: 100% }", "good");
    assert!(
        unknown.contains("the state 'bat' does not exist")
            && unknown.contains("did you mean 'bad'?"),
        "{unknown}"
    );
    assert!(
        message("{ bad: 100% }", "{ good: 100% }", "goood")
            .contains("the start state 'goood' does not exist")
    );
    // Canary: the chain that is right is accepted
    let _ = read(&head(
        "10s",
        &chain("{ bad: 100% }", "{ good: 100% }", "good"),
        "",
    ));
}

#[test]
fn a_state_must_be_a_valid_link_on_its_own() {
    let link = "      model:\n        kind: periodic\n        states:\n          - { for: 1s, set: { both: { delay: null } } }\n";
    // The link has a delay and no jitter: clearing the delay is fine. A jitter with the delay cleared is not.
    let _ = read(&head("10s", link, ""));
    let yaml = head("10s", link, "").replace(
        "both: { delay: 10ms }",
        "both: { delay: 10ms, jitter: { amount: 2ms } }",
    );
    let message = refused(&yaml);
    assert!(
        message.contains("`jitter` needs a `delay` to vary while this state lasts"),
        "{message}"
    );
}

#[test]
fn a_link_with_a_model_keeps_the_keys_of_its_states_for_itself() {
    let timeline = "timeline:\n  - { at: 5s, link: l, set: { both: { rate: 2mbit } } }\n";
    let message = refused(&head("12s", PERIODIC, timeline));
    assert!(message.contains("writes `rate` in its states"), "{message}");
    // A key that no state writes is the file's own
    let timeline = "timeline:\n  - { at: 5s, link: l, set: { both: { duplicate: 2% } } }\n";
    let _ = read(&head("12s", PERIODIC, timeline));
    // A delay, a jitter and a reorder depend on each other: one of them takes all three
    let link = "      model:\n        kind: periodic\n        states:\n          - { for: 1s, set: { both: { reorder: { share: 25% } } } }\n";
    let timeline = "timeline:\n  - { at: 5s, link: l, set: { both: { delay: 20ms } } }\n";
    let message = refused(&head("12s", link, timeline));
    assert!(
        message.contains("writes `delay` in its states"),
        "{message}"
    );
}

#[test]
fn an_outage_or_a_down_of_the_file_is_refused_where_the_model_has_them() {
    let outages = "      model:\n        kind: periodic\n        states:\n          - { for: 3s }\n          - { for: 2s, outage: true }\n";
    let timeline = "timeline:\n  - { at: 1s, link: l, outage: 100ms }\n";
    let message = refused(&head("12s", outages, timeline));
    assert!(message.contains("has states with an outage"), "{message}");
    // A `down` of the file is fine where the model has outages only
    let timeline =
        "timeline:\n  - { at: 1s, link: l, down: true }\n  - { at: 2s, link: l, up: true }\n";
    let _ = read(&head("12s", outages, timeline));
    let downs = "      model:\n        kind: periodic\n        states:\n          - { for: 3s }\n          - { for: 2s, down: true }\n";
    let message = refused(&head("12s", downs, timeline));
    assert!(message.contains("has states with `down`"), "{message}");
}

#[test]
fn a_model_is_written_out_and_read_again() {
    let yaml = head("12s", PERIODIC, "");
    let scenario = read(&yaml);
    let written = serde_saphyr::to_string(&scenario).unwrap();
    assert_eq!(
        Scenario::from_yaml(&written, "again.yaml").unwrap(),
        scenario
    );
    let markov = "      model:\n        kind: markov\n        start: good\n        states:\n          - { id: good, stay: 5s, to: { bad: 100% }, set: { both: { rate: 8mbit } } }\n          - { id: bad, stay: 500ms, distribution: fixed, to: { good: 100% }, set: { both: { loss: { model: random, p: 20% } } } }\n";
    let scenario = read(&head("12s", markov, ""));
    let written = serde_saphyr::to_string(&scenario).unwrap();
    assert_eq!(
        Scenario::from_yaml(&written, "again.yaml").unwrap(),
        scenario
    );
}

const CHAIN: &str = "      model:\n        kind: markov\n        start: short\n        states:\n          - { id: short, stay: 1s, to: { long: 100% }, set: { both: { rate: 1mbit } } }\n          - { id: long, stay: 3s, to: { short: 100% }, set: { both: { rate: 2mbit } } }\n";

/// The time in each state, from the events: the state is the rate that the last step set.
fn shares(compiled: &Scenario) -> (f64, f64) {
    let end = compiled.duration.as_nanos() as f64;
    let (mut short, mut long) = (0.0, 0.0);
    let mut state: Option<bool> = None;
    let mut since = 0.0;
    for event in &compiled.timeline {
        let now = event.at.as_nanos() as f64;
        match state {
            Some(true) => short += now - since,
            Some(false) => long += now - since,
            None => {}
        }
        let rate = event
            .set
            .as_ref()
            .and_then(|change| change.both.as_ref())
            .map(|both| both.rate);
        state = match rate {
            Some(Edit::Set(rate)) => Some(rate == "1mbit".parse().unwrap()),
            _ => state,
        };
        since = now;
    }
    match state {
        Some(true) => short += end - since,
        Some(false) => long += end - since,
        None => {}
    }
    (short / end, long / end)
}

#[test]
fn the_same_numbers_make_the_same_events_and_other_numbers_other_ones() {
    let scenario = read(&head("600s", CHAIN, ""));
    let first = scenario.compile(&mut |_| numbers(7)).unwrap().scenario;
    let again = scenario.compile(&mut |_| numbers(7)).unwrap().scenario;
    let other = scenario.compile(&mut |_| numbers(8)).unwrap().scenario;
    assert_eq!(first.timeline, again.timeline);
    assert_ne!(first.timeline, other.timeline);
    assert!(first.timeline.len() > 100);
    assert_eq!(first.validate(), Vec::new());
}

/// The model of the share of time: a chain with the stays 1 s and 3 s (exponential) that alternates spends `1/4` of the time in
/// the first state and `3/4` in the second. The count of the states alone would say `1/2`. The variance of the share over a
/// time `T` is `2 a² b² / ((a + b)³ T)` (an alternating renewal process): `1.9e-5` for `T = 15,000 s`, a deviation of `0.0043`, so
/// four deviations are `0.017`. A chain that draws its stays wrongly (their mean is not the `stay`) or ignores them is far outside.
#[test]
fn the_share_of_time_in_each_state_follows_the_stationary_distribution() {
    let scenario = read(&head("15000s", CHAIN, ""));
    let compiled = scenario.compile(&mut |_| numbers(11)).unwrap().scenario;
    let (short, long) = shares(&compiled);
    assert!((short - 0.25).abs() <= 0.017, "short {short}");
    assert!((long - 0.75).abs() <= 0.017, "long {long}");
}

/// Canary: a chain whose stays are all the same spends half of the time in each state, which the claim above refuses.
#[test]
fn canary_equal_stays_do_not_give_the_shares_of_unequal_ones() {
    let equal = CHAIN
        .replace("stay: 3s", "stay: 2s")
        .replace("stay: 1s", "stay: 2s");
    let scenario = read(&head("15000s", &equal, ""));
    let compiled = scenario.compile(&mut |_| numbers(11)).unwrap().scenario;
    let (short, _) = shares(&compiled);
    assert!((short - 0.5).abs() <= 0.017, "short {short}");
    assert!((short - 0.25).abs() > 0.017);
}

#[test]
fn a_chain_with_three_states_follows_its_stationary_distribution() {
    // From A the chain goes to B or C with 30 % and 70 %, from B and C it goes back to A. The embedded chain visits A half the time,
    // B 0.15 and C 0.35 of the visits; with the stays 2 s, 1 s and 4 s the shares of time are 1.0 : 0.15 : 1.4 over 2.55
    let chain = "      model:\n        kind: markov\n        start: a\n        states:\n          - { id: a, stay: 2s, to: { b: 30%, c: 70% }, set: { both: { rate: 1mbit } } }\n          - { id: b, stay: 1s, to: { a: 100% }, set: { both: { rate: 2mbit } } }\n          - { id: c, stay: 4s, to: { a: 100% }, set: { both: { rate: 3mbit } } }\n";
    let scenario = read(&head("20000s", chain, ""));
    let compiled = scenario.compile(&mut |_| numbers(5)).unwrap().scenario;
    let end = compiled.duration.as_nanos() as f64;
    let rate = |event: &Event| -> Option<String> {
        match &event.set.as_ref()?.both.as_ref()?.rate {
            Edit::Set(rate) => Some(rate.to_string()),
            _ => None,
        }
    };
    let mut time = std::collections::BTreeMap::<String, f64>::new();
    let mut previous: Option<(String, f64)> = None;
    for event in &compiled.timeline {
        let now = event.at.as_nanos() as f64;
        if let Some((state, since)) = previous.take() {
            *time.entry(state).or_default() += now - since;
        }
        if let Some(state) = rate(event) {
            previous = Some((state, now));
        }
    }
    if let Some((state, since)) = previous {
        *time.entry(state).or_default() += end - since;
    }
    let share = |state: &str| time[state] / end;
    // The deviation of a share is 0.007 at this length (a regenerative process with the visits of `a` as cycles of 5.1 s); the bounds are 0.03
    let (a, b, c) = (1.0 / 2.55, 0.15 / 2.55, 1.4 / 2.55);
    for (name, expected) in [("1mbit", a), ("2mbit", b), ("3mbit", c)] {
        let got = share(name);
        assert!(
            (got - expected).abs() <= 0.03,
            "{name}: {got} against {expected}"
        );
    }
    // The visits alone would give 0.5 : 0.15 : 0.35
    assert!((share("1mbit") - 0.5).abs() > 0.03);
}

#[test]
fn a_chain_that_would_make_too_many_events_is_refused_with_the_bound() {
    let chain = CHAIN
        .replace("stay: 1s", "stay: 10ms")
        .replace("stay: 3s", "stay: 10ms");
    let scenario = read(&head("3000s", &chain, ""));
    let problem = scenario.compile(&mut |_| numbers(1)).unwrap_err();
    assert!(
        problem
            .message
            .contains(&format!("more than {MAX_EVENTS_PER_LINK} events")),
        "{}",
        problem.message
    );
}

/// Every example scenario with a model or an event that repeats compiles to a scenario that is valid, with a constant source of
/// numbers and with a varying one (the oracle of the fuzz target of the reader).
#[test]
fn the_examples_compile_to_valid_scenarios() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let mut found = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "yaml") {
            continue;
        }
        let Ok(scenario) =
            Scenario::from_yaml(&std::fs::read_to_string(&path).unwrap(), "example.yaml")
        else {
            continue;
        };
        if !scenario.has_models() {
            continue;
        }
        found += 1;
        for seed in [1_u64, 2, 3] {
            let compiled = scenario.compile(&mut |_| numbers(seed)).unwrap();
            assert_eq!(
                compiled.scenario.validate(),
                Vec::new(),
                "{}",
                path.display()
            );
            assert!(!compiled.scenario.has_models());
        }
        let constant = scenario.compile(&mut |_| Box::new(|| 0.5)).unwrap();
        assert_eq!(
            constant.scenario.validate(),
            Vec::new(),
            "{}",
            path.display()
        );
    }
    assert!(found >= 3, "the examples with a model: {found}");
}
