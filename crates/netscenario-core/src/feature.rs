//! The features of a backend that a scenario can ask for, as the rows of the coverage matrix
//! (`docs/COVERAGE.md`), and which of them a scenario needs.

use netscenario_model::{JitterDistribution, LossModel, Scenario, WorkloadKind};

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
    /// A limit on the rate.
    RateLimit,
    /// The `udp-cbr` workload.
    UdpGenerators,
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
        Feature::RateLimit,
        Feature::UdpGenerators,
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
            Feature::RateLimit => "rate-limit",
            Feature::UdpGenerators => "udp-generators",
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
            Feature::RateLimit => "Rate limit",
            Feature::UdpGenerators => "UDP traffic generators",
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
    let mut has_loss = false;
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
            if impairments.delay.is_some() {
                need(Feature::FixedDelay, format!("{path}.delay"));
            }
            if let Some(jitter) = &impairments.jitter {
                match jitter.distribution {
                    JitterDistribution::Uniform | JitterDistribution::Normal => {
                        need(Feature::Jitter, format!("{path}.jitter"));
                    }
                }
            }
            if let Some(loss) = &impairments.loss {
                has_loss = true;
                match loss.model {
                    LossModel::Random => need(Feature::IndependentLoss, format!("{path}.loss")),
                }
            }
            if impairments.rate.is_some() {
                need(Feature::RateLimit, format!("{path}.rate"));
            }
        }
    }
    for (index, workload) in scenario.workloads.iter().enumerate() {
        match workload.kind {
            WorkloadKind::UdpCbr => need(Feature::UdpGenerators, format!("workloads[{index}]")),
        }
    }
    if has_loss && scenario.seed.is_some() {
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
