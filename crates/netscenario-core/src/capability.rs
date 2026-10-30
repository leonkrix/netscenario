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
    /// Only in part. The scenario must accept the restriction (`allow-limited`, release 0.3), and the result
    /// records that it did.
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
/// Returns the restrictions that were accepted (none unless `allow_limited` is set), or every feature that
/// the backend cannot reproduce. Nothing is created here.
///
/// # Errors
///
/// A [`CapabilityError`] lists each unsupported feature with the path in the scenario, the reason and a way
/// out. A feature that is supported only in part is listed too, unless `allow_limited` is set.
pub fn check(
    scenario: &Scenario,
    capabilities: &Capabilities,
    allow_limited: bool,
) -> Result<Vec<Accepted>, CapabilityError> {
    let mut refusals = Vec::new();
    let mut accepted = Vec::new();
    for requirement in requirements(scenario) {
        match capabilities.support(requirement.feature) {
            Support::Exact => {}
            Support::Limited { restriction } if allow_limited => accepted.push(Accepted {
                feature: requirement.feature,
                restriction,
            }),
            Support::Limited { restriction } => refusals.push(Refusal {
                feature: requirement.feature,
                path: requirement.path,
                reason: format!("it is reproduced only in part: {restriction}"),
                fix: "accept the restriction with `allow-limited` (planned for release 0.3), or use a backend that reproduces it exactly (see docs/COVERAGE.md)".to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(check(&scenario(), &all_exact(), false), Ok(Vec::new()));
    }

    /// Canary: the check can fail. Without the declaration nothing is supported.
    #[test]
    fn a_feature_that_was_never_declared_is_refused() {
        let error = check(&scenario(), &Capabilities::new("fake"), false).unwrap_err();
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
        let error = check(&scenario(), &caps, false).unwrap_err();
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
        let refused = check(&scenario(), &caps, false).unwrap_err();
        assert!(refused.refusals()[0].reason.contains("only in part"));
        assert!(refused.refusals()[0].fix.contains("allow-limited"));
        let accepted = check(&scenario(), &caps, true).unwrap();
        assert_eq!(
            accepted,
            [Accepted {
                feature: Feature::FixedDelay,
                restriction: "steps of 1 ms".to_owned()
            }]
        );
    }
}
