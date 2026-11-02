//! What the Kernel backend reproduces faithfully (BKD-03). The declaration follows the Kernel column of
//! `docs/COVERAGE.md`, and a test keeps the two in step.

use netscenario_core::{Capabilities, Feature, Support};

use crate::doctor::Step;
use crate::tools::{Tool, Version};

/// What the fix of a refusal ends with.
const ENGINE: &str = "or the Engine backend (planned for release 0.4)";

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
    Feature::BurstLoss,
    Feature::Reordering,
    Feature::Duplication,
    Feature::Corruption,
    Feature::RateLimit,
    Feature::ScheduledSteps,
    Feature::Outages,
];

/// The features that the Kernel backend gets in later steps of release 0.2: the format knows them, the backend does
/// not apply them yet, and says so instead of leaving them out silently. The page `docs/COVERAGE.md` says
/// `✔ 0.2` for them.
const PLANNED: &[Feature] = &[Feature::Program];

/// What the probe of `doctor` found out about the kernel and `tc` of this machine (D-068): for each thing a scenario
/// can need, whether it was accepted or what was answered. The version of `tc` says what it should know; the probe says
/// what the kernel does with it, and it is the probe that counts. A `Probed` that has no steps knows nothing, and the
/// declaration then follows the version of `tc` alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probed {
    steps: Vec<Step>,
}

impl Probed {
    /// The steps that the probe ran (see [`doctor::parse_probe`](crate::doctor::parse_probe)).
    #[must_use]
    pub fn new(steps: Vec<Step>) -> Self {
        Self { steps }
    }

    /// The steps, as they were run.
    pub(crate) fn steps(&self) -> &[Step] {
        &self.steps
    }

    fn step(&self, name: &str) -> Option<&Result<(), String>> {
        self.steps
            .iter()
            .find(|step| step.name == name)
            .map(|step| &step.result)
    }

    /// What was answered if the virtual Ethernet pair could not be made.
    fn veth_refused(&self) -> Option<&str> {
        self.step("veth")?.as_ref().err().map(|e| answer(e))
    }

    /// What was answered if `name` was refused. Nothing if it was accepted or not tried, and nothing if there was no
    /// interface to try it on: that is not the fault of `tc`.
    pub(crate) fn refused(&self, name: &str) -> Option<&str> {
        if self.veth_refused().is_some() {
            return None;
        }
        self.step(name)?.as_ref().err().map(|e| answer(e))
    }
}

/// The part of an error that the tool answered.
fn answer(error: &str) -> &str {
    error.rsplit_once("): ").map_or(error, |(_, a)| a).trim()
}

/// What the Kernel backend reproduces on a machine with `environment`, as far as the version of `tc` tells.
#[must_use]
pub fn capabilities(environment: &Environment) -> Capabilities {
    capabilities_probed(environment, &Probed::default())
}

/// What the Kernel backend reproduces on a machine with `environment`, where the probe says that the kernel or `tc`
/// refuses something that the version of `tc` promises (D-068): a kernel without `netem`, a model of loss, the
/// reordering, the duplication or the corruption that is refused, a seed that is not accepted, no virtual Ethernet
/// pair. A scenario that needs it is refused before anything is built, with the answer of the machine.
#[must_use]
pub fn capabilities_probed(environment: &Environment, probed: &Probed) -> Capabilities {
    let mut capabilities = by_version(environment);
    let unsupported = |reason: String, fix: &str| Support::Unsupported {
        reason,
        fix: fix.to_owned(),
    };
    if let Some(why) = probed.veth_refused() {
        capabilities = capabilities.with(
            Feature::PointToPoint,
            unsupported(
                format!("a virtual Ethernet pair cannot be created on this machine: {why}"),
                &format!("the kernel needs the module `veth` (`sudo modprobe veth`), {ENGINE}"),
            ),
        );
    }
    if let Some(why) = probed.refused("netem") {
        let support = unsupported(
            format!("the kernel or `tc` refuses `netem`: {why}"),
            &format!(
                "the kernel needs the module `sch_netem` (`sudo modprobe sch_netem`; on Ubuntu cloud kernels it is in the package `linux-modules-extra-$(uname -r)`), {ENGINE}"
            ),
        );
        for feature in NEEDS_TC.iter().chain(&[Feature::SeededLoss]) {
            capabilities = capabilities.with(*feature, support.clone());
        }
        return capabilities;
    }
    // One thing that the probe tried for one feature: the feature is refused if the kernel refused it
    for (features, steps, what) in [
        (
            &[Feature::BurstLoss][..],
            &["burst", "chain"][..],
            "the models of loss in bursts",
        ),
        (&[Feature::Reordering][..], &["reorder"][..], "reordering"),
        (
            &[Feature::Duplication][..],
            &["duplicate"][..],
            "duplication",
        ),
        (&[Feature::Corruption][..], &["corrupt"][..], "corruption"),
    ] {
        if let Some(why) = steps.iter().find_map(|step| probed.refused(step)) {
            for feature in features {
                capabilities = capabilities.with(
                    *feature,
                    unsupported(
                        format!("the kernel or `tc` refuses {what}: {why}"),
                        &format!("use a kernel and an iproute2 that accept it, {ENGINE}"),
                    ),
                );
            }
        }
    }
    // The version of `tc` may say that it knows the seed and the kernel not accept it
    if let Some(why) = probed.refused("seed")
        && capabilities.support(Feature::SeededLoss) == Support::Exact
    {
        capabilities = capabilities.with(
            Feature::SeededLoss,
            unsupported(
                format!("the kernel or `tc` refuses a seed: {why}"),
                &format!("use iproute2 {SEED_SINCE} or newer on a kernel that accepts the seed, leave out `seed` (the run is then not reproducible and says so), {ENGINE}"),
            ),
        );
    }
    capabilities
}

/// What the Kernel backend reproduces on a machine with `environment`, from the version of `tc` alone.
fn by_version(environment: &Environment) -> Capabilities {
    let mut capabilities = Capabilities::new("kernel")
        .with(Feature::PointToPoint, Support::Exact)
        .with(Feature::Ipv4, Support::Exact)
        .with(Feature::UdpGenerators, Support::Exact)
        .with(Feature::UdpWithoutChecksum, Support::Exact);
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
    // A link goes down by `ip`, which every run needs already
    capabilities = capabilities.with(Feature::LinkDownUp, Support::Exact);
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

    /// Steps of the probe: every step is accepted but the ones named, which answer as given.
    fn probed(failing: &[(&str, &str)]) -> Probed {
        Probed::new(
            [
                "veth",
                "netem",
                "tables",
                "seed",
                "burst",
                "chain",
                "reorder",
                "duplicate",
                "corrupt",
            ]
            .into_iter()
            .map(|name| Step {
                name: name.to_owned(),
                result: failing
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or(Ok(()), |(_, why)| Err((*why).to_owned())),
            })
            .collect(),
        )
    }

    fn declared(probed: &Probed) -> Capabilities {
        capabilities_probed(&Environment::complete(), probed)
    }

    fn reason(capabilities: &Capabilities, feature: Feature) -> String {
        match capabilities.support(feature) {
            Support::Unsupported { reason, .. } => reason,
            other => panic!("{feature:?} is {other:?}"),
        }
    }

    /// A probe that found nothing wrong, or none at all, changes nothing: the declaration is the one of the version.
    #[test]
    fn a_probe_that_found_nothing_wrong_leaves_the_declaration_as_the_version_gives_it() {
        let by_version = capabilities(&Environment::complete());
        for probed in [Probed::default(), probed(&[])] {
            let now = declared(&probed);
            for feature in Feature::ALL {
                assert_eq!(
                    now.support(*feature),
                    by_version.support(*feature),
                    "{feature:?}"
                );
            }
        }
    }

    /// The gate (D-068): what the kernel refuses is not declared, with its answer, though the version of `tc` promises it.
    #[test]
    fn what_the_kernel_refuses_is_not_declared_exact_and_the_reason_is_its_answer() {
        let no_netem = declared(&probed(&[(
            "netem",
            "RTNETLINK answers: No such file or directory",
        )]));
        for feature in NEEDS_TC.iter().chain(&[Feature::SeededLoss]) {
            let why = reason(&no_netem, *feature);
            assert!(
                why.contains("refuses `netem`") && why.contains("No such file"),
                "{why}"
            );
        }
        // What does not need netem is as it was
        assert_eq!(no_netem.support(Feature::Ipv4), Support::Exact);
        assert_eq!(no_netem.support(Feature::UdpGenerators), Support::Exact);

        let one = |step: &str, feature: Feature| {
            let found = declared(&probed(&[(
                step,
                "Error: Specified qdisc kind is unknown.",
            )]));
            assert!(reason(&found, feature).contains("Specified qdisc kind is unknown"));
            // And only that feature: the others are as they were
            let by_version = capabilities(&Environment::complete());
            for other in Feature::ALL.iter().filter(|other| **other != feature) {
                assert_eq!(
                    found.support(*other),
                    by_version.support(*other),
                    "{step}: {other:?}"
                );
            }
        };
        one("burst", Feature::BurstLoss);
        one("chain", Feature::BurstLoss);
        one("reorder", Feature::Reordering);
        one("duplicate", Feature::Duplication);
        one("corrupt", Feature::Corruption);
        one("seed", Feature::SeededLoss);
        one("veth", Feature::PointToPoint);
    }

    /// Canaries: when no pair could be created the later steps say nothing about `tc`, and a seed that the old `tc`
    /// does not know is told by the version and not overwritten by the probe.
    #[test]
    fn a_step_without_an_interface_is_not_blamed_and_the_version_keeps_its_reason() {
        let no_veth = declared(&probed(&[
            ("veth", "RTNETLINK answers: Operation not supported"),
            ("netem", "Cannot find device np0"),
            ("reorder", "Cannot find device np0"),
        ]));
        assert!(reason(&no_veth, Feature::PointToPoint).contains("Operation not supported"));
        assert_eq!(no_veth.support(Feature::FixedDelay), Support::Exact);
        assert_eq!(no_veth.support(Feature::Reordering), Support::Exact);

        let old = capabilities_probed(
            &Environment {
                tc: Tool::Found(Version::new(5, 15, 0)),
            },
            &probed(&[("seed", "What a seed?")]),
        );
        let why = reason(&old, Feature::SeededLoss);
        assert!(
            why.contains("5.15.0") && !why.contains("What a seed"),
            "{why}"
        );
    }

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
