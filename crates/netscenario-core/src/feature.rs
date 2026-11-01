//! The features of a backend that a scenario can ask for, as the rows of the coverage matrix
//! (`docs/COVERAGE.md`), and which of them a scenario needs.

use netscenario_model::{Impairments, Loss, Scenario, WorkloadKind};

/// Something a backend reproduces faithfully, in part, or not at all. Each feature is a row of the coverage
/// matrix; [`Feature::label`] is the text of that row, and a test keeps the two in step.
///
/// The list grows with the format: a release adds the features that its scenarios can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Feature {
    /// Two nodes and one link between them.
    PointToPoint,
    /// IPv4 addresses on the links.
    Ipv4,
    /// `forward` and `reverse` with their own values.
    DifferentValuesPerDirection,
    /// A constant delay.
    FixedDelay,
    /// A delay that varies (uniform or normal).
    Jitter,
    /// Each packet is lost with the same probability.
    IndependentLoss,
    /// Losses in bursts: the Gilbert-Elliott model and the four-state model.
    BurstLoss,
    /// Packets that arrive after later ones (`reorder`).
    Reordering,
    /// Packets that arrive twice (`duplicate`).
    Duplication,
    /// Packets with a flipped bit (`corrupt`).
    Corruption,
    /// A limit on the rate.
    RateLimit,
    /// Values of a link that change at moments of the run (`timeline` with `set`).
    ScheduledSteps,
    /// Windows in which nothing is delivered (`timeline` with `outage`).
    Outages,
    /// The interface of a link goes down and comes up (`timeline` with `down` and `up`).
    LinkDownUp,
    /// The `udp-cbr` workload.
    UdpGenerators,
    /// A `udp-cbr` workload without the checksum of UDP (`udp-checksum: false`), so that a corrupted packet
    /// arrives and is counted.
    UdpWithoutChecksum,
    /// A program of your own as a node (`kind: command`).
    Program,
    /// A `seed` is written in the file and the scenario has loss: the same seed must give the same lost packets.
    SeededLoss,
}

impl Feature {
    /// Every feature, in the order of the coverage matrix.
    pub const ALL: &'static [Feature] = &[
        Feature::PointToPoint,
        Feature::Ipv4,
        Feature::DifferentValuesPerDirection,
        Feature::FixedDelay,
        Feature::Jitter,
        Feature::IndependentLoss,
        Feature::BurstLoss,
        Feature::Reordering,
        Feature::Duplication,
        Feature::Corruption,
        Feature::RateLimit,
        Feature::ScheduledSteps,
        Feature::Outages,
        Feature::LinkDownUp,
        Feature::UdpGenerators,
        Feature::UdpWithoutChecksum,
        Feature::Program,
        Feature::SeededLoss,
    ];

    /// A short stable name, for results and logs.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Feature::PointToPoint => "point-to-point",
            Feature::Ipv4 => "ipv4",
            Feature::DifferentValuesPerDirection => "different-values-per-direction",
            Feature::FixedDelay => "fixed-delay",
            Feature::Jitter => "jitter",
            Feature::IndependentLoss => "independent-loss",
            Feature::BurstLoss => "burst-loss",
            Feature::Reordering => "reordering",
            Feature::Duplication => "duplication",
            Feature::Corruption => "corruption",
            Feature::RateLimit => "rate-limit",
            Feature::ScheduledSteps => "scheduled-steps",
            Feature::Outages => "outages",
            Feature::LinkDownUp => "link-down-up",
            Feature::UdpGenerators => "udp-generators",
            Feature::UdpWithoutChecksum => "udp-without-checksum",
            Feature::Program => "program",
            Feature::SeededLoss => "seeded-loss",
        }
    }

    /// The text of the row in `docs/COVERAGE.md`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Feature::PointToPoint => "Point to point",
            Feature::Ipv4 => "IPv4",
            Feature::DifferentValuesPerDirection => "Different values per direction",
            Feature::FixedDelay => "Fixed delay",
            Feature::Jitter => "Jitter (uniform, normal)",
            Feature::IndependentLoss => "Independent loss",
            Feature::BurstLoss => "Burst loss (Gilbert-Elliott, four-state)",
            Feature::Reordering => "Reordering",
            Feature::Duplication => "Duplication",
            Feature::Corruption => "Corruption",
            Feature::RateLimit => "Rate limit",
            Feature::ScheduledSteps => "Scheduled steps",
            Feature::Outages => "Outages",
            Feature::LinkDownUp => "Link down and up",
            Feature::UdpGenerators => "UDP traffic generators",
            Feature::UdpWithoutChecksum => "UDP without checksum",
            Feature::Program => "Your program as a node",
            Feature::SeededLoss => "Same seed, same loss pattern",
        }
    }
}

/// A feature that a scenario needs, and the first place in the scenario where it is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The feature that is needed.
    pub feature: Feature,
    /// The path in the scenario, for example `topology.links[0].forward.jitter`.
    pub path: String,
}

/// The features that a set of impairments needs, at `path`.
fn needs_of(impairments: &Impairments, path: &str, need: &mut dyn FnMut(Feature, String)) {
    if impairments.delay.is_some() {
        need(Feature::FixedDelay, format!("{path}.delay"));
    }
    if impairments.jitter.is_some() {
        need(Feature::Jitter, format!("{path}.jitter"));
    }
    match &impairments.loss {
        None => {}
        Some(Loss::Random { .. }) => need(Feature::IndependentLoss, format!("{path}.loss")),
        Some(Loss::GilbertElliott { .. } | Loss::Markov4 { .. }) => {
            need(Feature::BurstLoss, format!("{path}.loss"));
        }
    }
    if impairments.reorder.is_some() {
        need(Feature::Reordering, format!("{path}.reorder"));
    }
    if impairments.duplicate.is_some() {
        need(Feature::Duplication, format!("{path}.duplicate"));
    }
    if impairments.corrupt.is_some() {
        need(Feature::Corruption, format!("{path}.corrupt"));
    }
    if impairments.rate.is_some() {
        need(Feature::RateLimit, format!("{path}.rate"));
    }
}

/// The features that `scenario` needs, each once, in the order of their first use.
#[must_use]
pub fn requirements(scenario: &Scenario) -> Vec<Requirement> {
    let mut found = Vec::new();
    let mut need = |feature: Feature, path: String| {
        if !found.iter().any(|r: &Requirement| r.feature == feature) {
            found.push(Requirement { feature, path });
        }
    };
    need(Feature::PointToPoint, "topology".to_owned());
    need(Feature::Ipv4, "topology".to_owned());
    for (index, link) in scenario.topology.links.iter().enumerate() {
        let base = format!("topology.links[{index}]");
        for (name, impairments) in [
            ("forward", &link.forward),
            ("reverse", &link.reverse),
            ("both", &link.both),
        ] {
            let Some(impairments) = impairments else {
                continue;
            };
            let path = format!("{base}.{name}");
            if name != "both" {
                need(Feature::DifferentValuesPerDirection, path.clone());
            }
            needs_of(impairments, &path, &mut need);
        }
    }
    // What a `set` of the timeline leads to is asked for as well: a link that has nothing at the start and gets
    // a reordering at 5 s needs a backend that reorders. An outage is a loss of all packets and needs `Outages`.
    for step in crate::timeline::steps(scenario) {
        if let crate::timeline::Action::Impair {
            impairments,
            outage: false,
            ..
        } = &step.action
        {
            needs_of(
                impairments,
                &format!("timeline[{}].set", step.event),
                &mut need,
            );
        }
    }
    for (index, event) in scenario.timeline.iter().enumerate() {
        let path = format!("timeline[{index}]");
        if event.set.is_some() {
            need(Feature::ScheduledSteps, path.clone());
        }
        if event.outage.is_some() {
            need(Feature::Outages, path.clone());
        }
        if event.down.is_some() || event.up.is_some() {
            need(Feature::LinkDownUp, path);
        }
    }
    for (index, workload) in scenario.workloads.iter().enumerate() {
        match workload.kind() {
            WorkloadKind::UdpCbr => need(Feature::UdpGenerators, format!("workloads[{index}]")),
            WorkloadKind::Command => need(Feature::Program, format!("workloads[{index}]")),
        }
        if workload
            .as_udp_cbr()
            .is_some_and(|workload| !workload.udp_checksum.is_on())
        {
            need(
                Feature::UdpWithoutChecksum,
                format!("workloads[{index}].udp-checksum"),
            );
        }
    }
    if crate::timeline::draws_random_numbers(scenario) && scenario.seed.is_some() {
        need(Feature::SeededLoss, "seed".to_owned());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(yaml: &str) -> Scenario {
        Scenario::from_yaml(yaml, "test.yaml").expect("a valid scenario")
    }

    const HEAD: &str = "version: 0\nname: t\nduration: 10s\n";
    const TAIL: &str = "workloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";

    fn with_link(link: &str, extra_head: &str) -> Scenario {
        scenario(&format!(
            "{HEAD}{extra_head}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{link}{TAIL}"
        ))
    }

    fn features(scenario: &Scenario) -> Vec<Feature> {
        requirements(scenario).iter().map(|r| r.feature).collect()
    }

    #[test]
    fn a_plain_link_needs_only_the_basics() {
        let s = with_link("", "");
        assert_eq!(
            features(&s),
            [Feature::PointToPoint, Feature::Ipv4, Feature::UdpGenerators]
        );
    }

    #[test]
    fn every_impairment_names_its_feature_and_its_place() {
        let s = with_link(
            "      both: { delay: 20ms, jitter: { amount: 5ms }, loss: { model: random, p: 1% }, rate: 2mbit }\n",
            "",
        );
        let found = requirements(&s);
        let at = |f: Feature| {
            found
                .iter()
                .find(|r| r.feature == f)
                .map(|r| r.path.as_str())
        };
        assert_eq!(
            at(Feature::FixedDelay),
            Some("topology.links[0].both.delay")
        );
        assert_eq!(at(Feature::Jitter), Some("topology.links[0].both.jitter"));
        assert_eq!(
            at(Feature::IndependentLoss),
            Some("topology.links[0].both.loss")
        );
        assert_eq!(at(Feature::RateLimit), Some("topology.links[0].both.rate"));
        assert_eq!(at(Feature::DifferentValuesPerDirection), None);
        assert_eq!(at(Feature::SeededLoss), None);
    }

    #[test]
    fn the_impairments_of_release_0_2_name_their_features_and_their_places() {
        let s = with_link(
            "      forward: { delay: 20ms, loss: { model: gilbert-elliott, p: 2%, r: 30%, loss-bad: 50%, loss-good: 0.1% }, reorder: { share: 25%, gap: 5 }, duplicate: 1%, corrupt: 0.5% }\n      reverse: { loss: { model: markov4, p13: 2%, p31: 30%, p32: 10%, p23: 20%, p14: 0.5% } }\n",
            "",
        );
        let found = requirements(&s);
        let at = |f: Feature| {
            found
                .iter()
                .find(|r| r.feature == f)
                .map(|r| r.path.as_str())
        };
        // the first place that asks for the feature
        assert_eq!(
            at(Feature::BurstLoss),
            Some("topology.links[0].forward.loss")
        );
        assert_eq!(
            at(Feature::Reordering),
            Some("topology.links[0].forward.reorder")
        );
        assert_eq!(
            at(Feature::Duplication),
            Some("topology.links[0].forward.duplicate")
        );
        assert_eq!(
            at(Feature::Corruption),
            Some("topology.links[0].forward.corrupt")
        );
        assert_eq!(at(Feature::IndependentLoss), None);
        // each feature once
        assert_eq!(
            found
                .iter()
                .filter(|r| r.feature == Feature::BurstLoss)
                .count(),
            1
        );
    }

    #[test]
    fn the_events_of_the_timeline_name_their_features_and_their_places() {
        let s = scenario(&format!(
            "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\ntimeline:\n  - {{ at: 1s, link: l, outage: 1s }}\n  - {{ at: 3s, link: l, set: {{ both: {{ rate: 2mbit }} }} }}\n  - {{ at: 5s, link: l, down: true }}\n  - {{ at: 6s, link: l, up: true }}\n{TAIL}"
        ));
        let found = requirements(&s);
        let at = |f: Feature| {
            found
                .iter()
                .find(|r| r.feature == f)
                .map(|r| r.path.as_str())
        };
        assert_eq!(at(Feature::Outages), Some("timeline[0]"));
        assert_eq!(at(Feature::ScheduledSteps), Some("timeline[1]"));
        // the first event that asks for it, down or up
        assert_eq!(at(Feature::LinkDownUp), Some("timeline[2]"));
    }

    /// A stream without the checksum of UDP needs a backend that can send one; a plain stream does not ask.
    #[test]
    fn a_workload_without_the_checksum_names_its_feature_and_its_place() {
        let without = scenario(&format!(
            "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b, udp-checksum: false }}\n"
        ));
        let found = requirements(&without);
        let place = found
            .iter()
            .find(|r| r.feature == Feature::UdpWithoutChecksum)
            .map(|r| r.path.as_str());
        assert_eq!(place, Some("workloads[0].udp-checksum"));
        // Canary: the plain workload, and one that writes the default, do not ask for it
        assert!(!features(&with_link("", "")).contains(&Feature::UdpWithoutChecksum));
        let explicit = scenario(&format!(
            "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b, udp-checksum: true }}\n"
        ));
        assert!(!features(&explicit).contains(&Feature::UdpWithoutChecksum));
    }

    /// What a `set` leads to is asked for, with the path of the step: a link with nothing at the start that gets a
    /// reordering and a rate at 5 s needs a backend that does both. An outage needs `Outages` and no loss.
    #[test]
    fn what_a_step_leads_to_is_a_requirement_with_the_path_of_the_step() {
        let s = scenario(&format!(
            "{HEAD}topology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{TAIL}timeline:\n  - {{ at: 5s, link: l, set: {{ forward: {{ delay: 10ms, reorder: {{ share: 25% }}, rate: 2mbit }} }} }}\n  - {{ at: 7s, link: l, outage: 1s }}\n"
        ));
        let found = requirements(&s);
        let at = |f: Feature| {
            found
                .iter()
                .find(|r| r.feature == f)
                .map(|r| r.path.as_str())
        };
        assert_eq!(at(Feature::Reordering), Some("timeline[0].set.reorder"));
        assert_eq!(at(Feature::RateLimit), Some("timeline[0].set.rate"));
        assert_eq!(at(Feature::Outages), Some("timeline[1]"));
        // Canary: the loss of the outage is not a loss the scenario asks for
        assert_eq!(at(Feature::IndependentLoss), None);
    }

    /// A seed matters for a loss that a step switches on, and not for an outage alone.
    #[test]
    fn a_loss_that_comes_with_a_step_needs_the_seed_and_an_outage_does_not() {
        let with = |timeline: &str| {
            scenario(&format!(
                "{HEAD}seed: 3\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n{TAIL}timeline:\n{timeline}"
            ))
        };
        let lossy =
            with("  - { at: 5s, link: l, set: { forward: { loss: { model: random, p: 5% } } } }\n");
        assert!(features(&lossy).contains(&Feature::SeededLoss));
        let outage = with("  - { at: 5s, link: l, outage: 1s }\n");
        assert!(!features(&outage).contains(&Feature::SeededLoss));
    }

    #[test]
    fn forward_or_reverse_needs_values_per_direction() {
        let s = with_link("      reverse: { delay: 20ms }\n", "");
        let found = requirements(&s);
        let r = found
            .iter()
            .find(|r| r.feature == Feature::DifferentValuesPerDirection)
            .expect("the feature is needed");
        assert_eq!(r.path, "topology.links[0].reverse");
    }

    #[test]
    fn a_seed_matters_only_with_loss() {
        let lossy = with_link(
            "      both: { loss: { model: random, p: 1% } }\n",
            "seed: 7\n",
        );
        assert!(features(&lossy).contains(&Feature::SeededLoss));
        let without_loss = with_link("      both: { delay: 5ms }\n", "seed: 7\n");
        assert!(!features(&without_loss).contains(&Feature::SeededLoss));
        let without_seed = with_link("      both: { loss: { model: random, p: 1% } }\n", "");
        assert!(!features(&without_seed).contains(&Feature::SeededLoss));
    }

    #[test]
    fn ids_and_labels_are_unique() {
        for (i, a) in Feature::ALL.iter().enumerate() {
            for b in &Feature::ALL[i + 1..] {
                assert_ne!(a.id(), b.id());
                assert_ne!(a.label(), b.label());
            }
        }
    }
}
