//! What a backend reproduces faithfully, and the check of a scenario against it (BKD-03).
//!
//! The rule behind this module is the first principle of the project: a backend never approximates silently.
//! A scenario that asks for something the backend cannot do exactly is refused before anything is created, with
//! an error that names the feature, the backend and a way out.

use std::collections::BTreeMap;
use std::fmt;

use netscenario_model::{Problem, Scenario};

use crate::feature::{Feature, requirements};

/// How well a backend reproduces one feature on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// Faithful: the behaviour is what the scenario says.
    Exact,
    /// Only in part. The scenario must accept the restriction (`allow-limited`, D-092), and the result records that it did.
    Limited {
        /// What is different, in a sentence.
        restriction: String,
    },
    /// Not possible here. The text says why and how to get it.
    Unsupported {
        /// Why this backend cannot do it (on this machine).
        reason: String,
        /// What to do: another tool version, another setting, another backend.
        fix: String,
    },
}

/// The declaration of a backend: for each feature, how well it is supported. A feature that is not declared is
/// unsupported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    backend: String,
    entries: BTreeMap<Feature, Support>,
}

impl Capabilities {
    /// An empty declaration for the backend called `backend` (the name used in `backend:` of a scenario).
    #[must_use]
    pub fn new(backend: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            entries: BTreeMap::new(),
        }
    }

    /// Declares how `feature` is supported.
    #[must_use]
    pub fn with(mut self, feature: Feature, support: Support) -> Self {
        self.entries.insert(feature, support);
        self
    }

    /// The name of the backend.
    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// How `feature` is supported. A feature that was never declared is not implemented.
    #[must_use]
    pub fn support(&self, feature: Feature) -> Support {
        self.entries
            .get(&feature)
            .cloned()
            .unwrap_or_else(|| Support::Unsupported {
                reason: format!(
                    "the {} backend does not implement it in this release",
                    self.backend
                ),
                fix: "see docs/COVERAGE.md for the backends and releases that do".to_owned(),
            })
    }

    /// The declared features and their support, in the order of [`Feature`].
    pub fn iter(&self) -> impl Iterator<Item = (Feature, &Support)> {
        self.entries
            .iter()
            .map(|(feature, support)| (*feature, support))
    }
}

/// A feature of a scenario that the backend does not reproduce exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The feature that is refused.
    pub feature: Feature,
    /// Where the scenario asks for it.
    pub path: String,
    /// Why the backend cannot do it.
    pub reason: String,
    /// What to do about it.
    pub fix: String,
}

/// A restriction that the scenario accepted: the backend reproduces the feature only in part, and the result
/// has to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// The feature that is reproduced only in part.
    pub feature: Feature,
    /// What is different.
    pub restriction: String,
}

/// The scenario asks for features that the backend does not reproduce exactly. Every one is listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityError {
    backend: String,
    refusals: Vec<Refusal>,
}

impl CapabilityError {
    /// An error for `backend` with the given refusals.
    #[must_use]
    pub fn new(backend: impl Into<String>, refusals: Vec<Refusal>) -> Self {
        Self {
            backend: backend.into(),
            refusals,
        }
    }

    /// The name of the backend.
    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// What was refused.
    #[must_use]
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }

    /// The error as a person sees it when the text of the scenario is at hand: every refusal with the line
    /// of the file and a mark under the place (the same form as the errors of reading a file). [`Display`]
    /// shows the path only.
    ///
    /// [`Display`]: std::fmt::Display
    #[must_use]
    pub fn render(&self, source_name: &str, source: &str) -> String {
        netscenario_model::Error::in_source(source_name, source, self.problems())
            .map_or_else(String::new, |error| error.render())
    }

    /// Like [`render`](Self::render), for a scenario that was read from the file: when the scenario used parameters, templates or
    /// includes (D-093), each refusal is shown at the line where its value was written, in the template or the included file.
    #[must_use]
    pub fn render_for(&self, scenario: &Scenario, source_name: &str, source: &str) -> String {
        scenario
            .problems_in_source(source_name, source, self.problems())
            .map_or_else(String::new, |error| error.render())
    }

    /// The refusals as problems with a path, in the form that the reading of a file uses.
    #[must_use]
    pub fn problems(&self) -> Vec<Problem> {
        self.refusals
            .iter()
            .map(|refusal| {
                Problem::new(format!(
                    "the {} backend cannot reproduce '{}': {}",
                    self.backend,
                    refusal.feature.label(),
                    refusal.reason
                ))
                .at_path(refusal.path.clone())
                .with_help(refusal.fix.clone())
            })
            .collect()
    }
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, problem) in self.problems().iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            writeln!(f, "error: {}", problem.message)?;
            if let Some(path) = &problem.path {
                writeln!(f, "  = path: {path}")?;
            }
            if let Some(help) = &problem.help {
                writeln!(f, "  = help: {help}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for CapabilityError {}

/// Checks `scenario` against `capabilities`.
///
/// Returns the restrictions that were accepted (the features that the backend reproduces only in part and that the scenario
/// lists in `allow-limited`, D-092), or every feature that the backend cannot reproduce. Nothing is created here.
///
/// # Errors
///
/// A [`CapabilityError`] lists each unsupported feature with the path in the scenario, the reason and a way
/// out. A feature that is supported only in part is listed too, unless the scenario accepts it.
pub fn check(
    scenario: &Scenario,
    capabilities: &Capabilities,
) -> Result<Vec<Accepted>, CapabilityError> {
    let mut refusals = Vec::new();
    let mut accepted = Vec::new();
    for requirement in requirements(scenario) {
        match capabilities.support(requirement.feature) {
            Support::Exact => {}
            Support::Limited { restriction }
                if scenario
                    .allow_limited
                    .iter()
                    .any(|name| name.as_str() == requirement.feature.id()) =>
            {
                accepted.push(Accepted {
                    feature: requirement.feature,
                    restriction,
                });
            }
            Support::Limited { restriction } => refusals.push(Refusal {
                feature: requirement.feature,
                path: requirement.path,
                reason: format!("it is reproduced only in part: {restriction}"),
                fix: format!(
                    "accept the restriction with `allow-limited: [{}]` in the scenario, or use a backend that reproduces it exactly (see docs/COVERAGE.md)",
                    requirement.feature.id()
                ),
            }),
            Support::Unsupported { reason, fix } => refusals.push(Refusal {
                feature: requirement.feature,
                path: requirement.path,
                reason,
                fix,
            }),
        }
    }
    if refusals.is_empty() {
        Ok(accepted)
    } else {
        Err(CapabilityError {
            backend: capabilities.backend.clone(),
            refusals,
        })
    }
}

/// The entries of `allow-limited` that name no feature, each as a problem at its place in the scenario.
#[must_use]
pub(crate) fn unknown_allowed_features(scenario: &Scenario) -> Vec<Problem> {
    let ids: Vec<&str> = Feature::ALL.iter().map(|feature| feature.id()).collect();
    scenario
        .allow_limited
        .iter()
        .enumerate()
        .filter(|(_, name)| !ids.contains(&name.as_str()))
        .map(|(index, name)| {
            let help = match netscenario_model::closest(name.as_str(), &ids) {
                Some(near) => format!("did you mean '{near}'?"),
                None => {
                    "the id of a feature is in the message of the refusal, and in docs/COVERAGE.md"
                        .to_owned()
                }
            };
            Problem::new(format!("'{name}' is not the id of a feature"))
                .at_path(format!("allow-limited[{index}]"))
                .with_help(help)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscenario_model::Name;

    const FILE: &str = "version: 0\nname: t\nduration: 10s\nseed: 3\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: { delay: 20ms, loss: { model: random, p: 1% } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";

    fn scenario() -> Scenario {
        Scenario::from_yaml(FILE, "t.yaml").expect("valid")
    }

    fn all_exact() -> Capabilities {
        Feature::ALL
            .iter()
            .fold(Capabilities::new("fake"), |caps, feature| {
                caps.with(*feature, Support::Exact)
            })
    }

    #[test]
    fn a_backend_that_does_everything_accepts_the_scenario() {
        assert_eq!(check(&scenario(), &all_exact()), Ok(Vec::new()));
    }

    /// Canary: the check can fail. Without the declaration nothing is supported.
    #[test]
    fn a_feature_that_was_never_declared_is_refused() {
        let error = check(&scenario(), &Capabilities::new("fake")).unwrap_err();
        let features: Vec<Feature> = error.refusals().iter().map(|r| r.feature).collect();
        assert!(features.contains(&Feature::FixedDelay));
        assert!(features.contains(&Feature::SeededLoss));
        assert_eq!(error.backend(), "fake");
    }

    #[test]
    fn every_refusal_is_listed_with_its_place() {
        let caps = all_exact().with(
            Feature::SeededLoss,
            Support::Unsupported {
                reason: "tc 6.1.0 does not know `seed`".to_owned(),
                fix: "install iproute2 6.6.0 or newer".to_owned(),
            },
        );
        let error = check(&scenario(), &caps).unwrap_err();
        assert_eq!(error.refusals().len(), 1);
        assert_eq!(error.refusals()[0].path, "seed");
        assert_eq!(
            error.to_string(),
            "error: the fake backend cannot reproduce 'Same seed, same loss pattern': tc 6.1.0 does not know `seed`\n  = path: seed\n  = help: install iproute2 6.6.0 or newer\n"
        );
    }

    #[test]
    fn a_restriction_must_be_accepted() {
        let caps = all_exact().with(
            Feature::FixedDelay,
            Support::Limited {
                restriction: "steps of 1 ms".to_owned(),
            },
        );
        let refused = check(&scenario(), &caps).unwrap_err();
        assert!(refused.refusals()[0].reason.contains("only in part"));
        assert!(
            refused.refusals()[0]
                .fix
                .contains("`allow-limited: [fixed-delay]`")
        );
        // Canary: another feature in the list accepts nothing
        let mut other = scenario();
        other.allow_limited = vec![Name::new("jitter").unwrap()];
        assert!(check(&other, &caps).is_err());
        let mut accepting = scenario();
        accepting.allow_limited = vec![Name::new("fixed-delay").unwrap()];
        let accepted = check(&accepting, &caps).unwrap();
        assert_eq!(
            accepted,
            [Accepted {
                feature: Feature::FixedDelay,
                restriction: "steps of 1 ms".to_owned()
            }]
        );
    }

    /// An entry of `allow-limited` that names no feature is a problem at its place, with a suggestion where one is close
    /// (D-092); a real id is none.
    #[test]
    fn an_id_that_names_no_feature_is_a_problem_with_a_suggestion() {
        let mut scenario = scenario();
        scenario.allow_limited = vec![
            Name::new("fast-rate-steps").unwrap(),
            Name::new("fast-rate-step").unwrap(),
            Name::new("nonsense").unwrap(),
        ];
        let problems = unknown_allowed_features(&scenario);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(problems[0].path.as_deref(), Some("allow-limited[1]"));
        assert_eq!(
            problems[0].help.as_deref(),
            Some("did you mean 'fast-rate-steps'?")
        );
        assert_eq!(problems[1].path.as_deref(), Some("allow-limited[2]"));
        // The way a run and `provision` meet them: as an invalid scenario, before anything is created
        assert_ne!(crate::provision::checked(&scenario), Vec::new());
        assert_eq!(unknown_allowed_features(&self::scenario()), Vec::new());
    }
}
