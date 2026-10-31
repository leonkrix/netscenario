//! What the Kernel backend reproduces faithfully (BKD-03). The declaration follows the Kernel column of
//! `docs/COVERAGE.md`, and a test keeps the two in step.

use netscenario_core::{Capabilities, Feature, Support};

use crate::tools::{Tool, Version};

/// The first iproute2 that knows the `seed` of `netem` (commit `a67cf9a2`, D-013).
const SEED_SINCE: Version = Version::new(6, 6, 0);

/// What the declaration depends on: the tools of this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// `tc`, from iproute2.
    pub tc: Tool,
}

impl Environment {
    /// A machine with a current iproute2: everything the backend can do is available.
    #[must_use]
    pub fn complete() -> Self {
        Self {
            tc: Tool::Found(Version::new(6, 15, 0)),
        }
    }
}

/// The features that `netem` and `tc` provide.
const NEEDS_TC: &[Feature] = &[
    Feature::DifferentValuesPerDirection,
    Feature::FixedDelay,
    Feature::Jitter,
    Feature::IndependentLoss,
    Feature::RateLimit,
];

/// The features that the Kernel backend gets in step 4.2 of release 0.2: the format knows them, the backend does
/// not apply them yet, and says so instead of leaving them out silently. The page `docs/COVERAGE.md` says
/// `✔ 0.2` for them.
const PLANNED: &[Feature] = &[
    Feature::BurstLoss,
    Feature::Reordering,
    Feature::Duplication,
    Feature::Corruption,
];

/// What the Kernel backend reproduces on a machine with `environment`.
#[must_use]
pub fn capabilities(environment: &Environment) -> Capabilities {
    let mut capabilities = Capabilities::new("kernel")
        .with(Feature::PointToPoint, Support::Exact)
        .with(Feature::Ipv4, Support::Exact)
        .with(Feature::UdpGenerators, Support::Exact);
    for feature in NEEDS_TC {
        capabilities = capabilities.with(*feature, tc_support(&environment.tc, None));
    }
    for feature in PLANNED {
        capabilities = capabilities.with(
            *feature,
            Support::Unsupported {
                reason: "the Kernel backend applies this from release 0.2, and this build does not yet"
                    .to_owned(),
                fix: "leave it out of the scenario for now (see docs/COVERAGE.md for the release that has it)"
                    .to_owned(),
            },
        );
    }
    capabilities.with(
        Feature::SeededLoss,
        tc_support(&environment.tc, Some(SEED_SINCE)),
    )
}

/// `Exact` if `tc` is there (and at least `since`), else the reason and the way out.
fn tc_support(tc: &Tool, since: Option<Version>) -> Support {
    match (tc, since) {
        (Tool::Missing, _) => Support::Unsupported {
            reason: "the tool `tc` was not found".to_owned(),
            fix: "install iproute2 (package `iproute2`) and run `netscenario doctor`".to_owned(),
        },
        (Tool::Unreadable(printed), _) => Support::Unsupported {
            reason: format!("the version of `tc` cannot be read (it printed '{printed}')"),
            fix: "install a release of iproute2 (6.6.0 or newer) and run `netscenario doctor`"
                .to_owned(),
        },
        (Tool::Found(version), Some(needed)) if *version < needed => Support::Unsupported {
            reason: format!(
                "the `tc` of this machine is iproute2 {version}, and `seed` needs {needed} or newer"
            ),
            fix: format!(
                "install iproute2 {needed} or newer, leave out `seed` (the run is then not reproducible and says so), or use the Engine backend (planned for release 0.4)"
            ),
        },
        (Tool::Found(_), _) => Support::Exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn support(tc: Tool, feature: Feature) -> Support {
        capabilities(&Environment { tc }).support(feature)
    }

    #[test]
    fn a_complete_machine_supports_every_feature() {
        let capabilities = capabilities(&Environment::complete());
        for feature in Feature::ALL {
            if PLANNED.contains(feature) {
                continue;
            }
            assert_eq!(
                capabilities.support(*feature),
                Support::Exact,
                "{feature:?}"
            );
        }
    }

    /// Canary for the list above: a feature that is planned is refused with a reason that says when, and it
    /// is refused whatever `tc` is there. Without the declaration it would be "not declared", which reads as a
    /// bug.
    #[test]
    fn a_planned_feature_is_refused_with_the_release_that_brings_it() {
        for feature in PLANNED {
            let Support::Unsupported { reason, fix } =
                support(Tool::Found(Version::new(6, 15, 0)), *feature)
            else {
                panic!("{feature:?} is not built yet");
            };
            assert!(reason.contains("release 0.2"), "{reason}");
            assert!(fix.contains("COVERAGE.md"), "{fix}");
        }
    }

    #[test]
    fn an_old_tc_keeps_delay_and_loss_but_not_the_seed() {
        let old = Tool::Found(Version::new(5, 15, 0));
        assert_eq!(support(old.clone(), Feature::FixedDelay), Support::Exact);
        assert_eq!(
            support(old.clone(), Feature::IndependentLoss),
            Support::Exact
        );
        let Support::Unsupported { reason, fix } = support(old, Feature::SeededLoss) else {
            panic!("an old tc cannot seed");
        };
        assert!(
            reason.contains("5.15.0") && reason.contains("6.6.0"),
            "{reason}"
        );
        assert!(
            fix.contains("leave out `seed`") && fix.contains("Engine"),
            "{fix}"
        );
    }

    #[test]
    fn the_seed_starts_with_iproute2_6_6_0() {
        assert!(matches!(
            support(Tool::Found(Version::new(6, 5, 9)), Feature::SeededLoss),
            Support::Unsupported { .. }
        ));
        assert_eq!(
            support(Tool::Found(Version::new(6, 6, 0)), Feature::SeededLoss),
            Support::Exact
        );
    }

    #[test]
    fn without_tc_nothing_that_needs_it_is_supported() {
        for feature in NEEDS_TC.iter().chain(&[Feature::SeededLoss]) {
            assert!(
                matches!(
                    support(Tool::Missing, *feature),
                    Support::Unsupported { .. }
                ),
                "{feature:?}"
            );
        }
        assert_eq!(
            support(Tool::Missing, Feature::PointToPoint),
            Support::Exact
        );
    }

    #[test]
    fn a_version_that_cannot_be_read_is_not_trusted() {
        let unreadable = Tool::Unreadable("iproute2-ss200602".to_owned());
        let Support::Unsupported { reason, .. } = support(unreadable, Feature::SeededLoss) else {
            panic!("an unreadable version cannot be trusted");
        };
        assert!(reason.contains("iproute2-ss200602"), "{reason}");
    }

    #[test]
    fn every_feature_is_declared() {
        // A feature that was added to the list without a decision here would be refused as "not implemented"
        let capabilities = capabilities(&Environment::complete());
        let declared: Vec<Feature> = capabilities.iter().map(|(feature, _)| feature).collect();
        for feature in Feature::ALL {
            assert!(declared.contains(feature), "{feature:?} is not declared");
        }
    }
}
