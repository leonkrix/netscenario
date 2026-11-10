//! The profile library (SCN-06, D-095): named sets of sourced values for a link.
//!
//! A profile sets only what a source states, or what follows from the sources by arithmetic that `docs/PROFILES.md` shows; what no
//! source gives is left to the author. A link writes `profile: lte-typical`; when the scenario is read the profile is replaced by
//! its values (the keys the file writes win), the link keeps the name as a record, and the scenario records the version of the
//! library (`profile-library`). A scenario that has that key is not resolved again, so a replay does not take the numbers of a
//! later library.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::Problem;
use crate::link_model::LinkModel;
use crate::scenario::{Impairments, Scenario};
use crate::suggest::closest;

/// The library, embedded in the binary: the version and the profiles.
const LIBRARY_FILE: &str = include_str!("../profiles/library.yaml");

/// What a profile sets for a link, and where the numbers come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Profile {
    /// The id, `family-tier`: what is written after `profile:`. Lower case letters, digits and `-`.
    pub id: String,
    /// The kind of link: `3g`, `lte`, `5g`, `dsl`, `geo`, `leo`, `datacenter`.
    pub family: String,
    /// The tier of the family: what the source measured, defined in `docs/PROFILES.md`.
    pub tier: String,
    /// One sentence: what the tier is.
    pub summary: String,
    /// The ids of the sources in `docs/PROFILES.md`. At least one.
    pub sources: Vec<String>,
    /// What the terminal sends to the network (`forward` of a link whose first node is the terminal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up: Option<Impairments>,
    /// What the network sends to the terminal (`reverse`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down: Option<Impairments>,
    /// The same in both directions; `up` and `down` win over it key by key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub both: Option<Impairments>,
    /// How the link changes over time. None of the profiles of version 1 has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<LinkModel>,
}

impl Profile {
    /// The names of the keys the profile sets in at least one direction, in the order of the format.
    #[must_use]
    pub fn keys(&self) -> Vec<&'static str> {
        let (up, down) = (self.forward(), self.reverse());
        let set = |f: fn(&Impairments) -> bool| f(&up) || f(&down);
        [
            ("delay", set(|i| i.delay.is_some())),
            ("jitter", set(|i| i.jitter.is_some())),
            ("loss", set(|i| i.loss.is_some())),
            ("reorder", set(|i| i.reorder.is_some())),
            ("duplicate", set(|i| i.duplicate.is_some())),
            ("corrupt", set(|i| i.corrupt.is_some())),
            ("rate", set(|i| i.rate.is_some())),
            ("queue", set(|i| i.queue.is_some())),
            ("model", self.model.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect()
    }

    /// What a link with this profile gets, as YAML: `forward` (up) and `reverse` (down).
    ///
    /// # Errors
    ///
    /// The message of the writer, which cannot fail for the values of the library.
    pub fn values_yaml(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Values {
            forward: Impairments,
            reverse: Impairments,
        }
        serde_saphyr::to_string(&Values {
            forward: self.forward(),
            reverse: self.reverse(),
        })
        .map_err(|error| error.to_string())
    }

    /// The impairments of the direction from the terminal to the network.
    #[must_use]
    pub fn forward(&self) -> Impairments {
        direction(self.up.as_ref(), self.both.as_ref())
    }

    /// The impairments of the direction from the network to the terminal.
    #[must_use]
    pub fn reverse(&self) -> Impairments {
        direction(self.down.as_ref(), self.both.as_ref())
    }
}

fn direction(own: Option<&Impairments>, both: Option<&Impairments>) -> Impairments {
    let both = both.cloned().unwrap_or_default();
    match own {
        Some(own) => own.with_defaults_from(&both),
        None => both,
    }
}

/// The profiles and the version of the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    version: u32,
    profiles: Vec<Profile>,
}

impl Library {
    /// The version of the library: a scenario records the one it was resolved with.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// All profiles, in the order of the file.
    #[must_use]
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// The profile with this id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// The id that is close to a mistyped one, if there is exactly one.
    #[must_use]
    pub fn closest(&self, written: &str) -> Option<&str> {
        let known: Vec<&str> = self.profiles.iter().map(|p| p.id.as_str()).collect();
        closest(written, &known)
    }
}

/// The library of this release.
///
/// # Panics
///
/// Never in a build that passed its tests: the file is part of the source and a test reads it.
#[must_use]
// The file is part of the source and a test reads it; a panic here would be a build that was never tested
#[allow(clippy::expect_used)]
pub fn library() -> &'static Library {
    static LIBRARY: OnceLock<Library> = OnceLock::new();
    LIBRARY.get_or_init(|| {
        serde_saphyr::from_str(LIBRARY_FILE)
            .expect("the profile library is valid (a test reads it)")
    })
}

impl Impairments {
    /// These impairments, with every key they do not set taken from `base`.
    #[must_use]
    pub fn with_defaults_from(&self, base: &Impairments) -> Impairments {
        Impairments {
            delay: self.delay.or(base.delay),
            jitter: self.jitter.clone().or_else(|| base.jitter.clone()),
            loss: self.loss.or(base.loss),
            reorder: self.reorder.or(base.reorder),
            duplicate: self.duplicate.or(base.duplicate),
            corrupt: self.corrupt.or(base.corrupt),
            rate: self.rate.or(base.rate),
            queue: self.queue.clone().or_else(|| base.queue.clone()),
        }
    }
}

impl Scenario {
    /// Replaces the `profile` of every link by its values (D-095), unless the scenario records a library already, in which case
    /// its values are the ones the run used and are left alone. Returns the problems: a profile that does not exist.
    pub(crate) fn resolve_profiles(&mut self) -> Vec<Problem> {
        let mut problems = Vec::new();
        if self.profile_library.is_some() || self.topology.links.iter().all(|l| l.profile.is_none())
        {
            return problems;
        }
        let library = library();
        for (index, link) in self.topology.links.iter_mut().enumerate() {
            let Some(id) = link.profile.clone() else {
                continue;
            };
            let Some(profile) = library.get(&id) else {
                let help = match library.closest(&id) {
                    Some(close) => {
                        format!("did you mean '{close}'? `netscenario profiles` lists the profiles")
                    }
                    None => "`netscenario profiles` lists the profiles".to_owned(),
                };
                problems.push(
                    Problem::new(format!("there is no profile called '{id}'"))
                        .at_path(format!("topology.links[{index}].profile"))
                        .with_help(help),
                );
                continue;
            };
            // `both` with `forward` or `reverse` is an error of its own, which the checks report
            if link.both.is_some() && (link.forward.is_some() || link.reverse.is_some()) {
                continue;
            }
            let (own_forward, own_reverse) = match link.both.take() {
                Some(both) => (Some(both.clone()), Some(both)),
                None => (link.forward.take(), link.reverse.take()),
            };
            link.forward = Some(
                own_forward
                    .unwrap_or_default()
                    .with_defaults_from(&profile.forward()),
            );
            link.reverse = Some(
                own_reverse
                    .unwrap_or_default()
                    .with_defaults_from(&profile.reverse()),
            );
            if link.model.is_none() {
                link.model.clone_from(&profile.model);
            }
        }
        if problems.is_empty() {
            self.profile_library = Some(library.version());
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canary for the whole library: it is read by the reader of the format, so a number with a wrong unit or a key that does not
    /// exist is a failing test and not a panic in a user's run.
    #[test]
    fn the_library_is_valid_and_every_profile_has_a_source_and_a_value() {
        let library = library();
        assert!(library.version() >= 1);
        assert_ne!(library.profiles().len(), 0);
        let mut ids = std::collections::BTreeSet::new();
        for profile in library.profiles() {
            assert!(ids.insert(&profile.id), "{} twice", profile.id);
            assert_eq!(profile.id, format!("{}-{}", profile.family, profile.tier));
            assert!(
                profile
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{}",
                profile.id
            );
            assert!(!profile.sources.is_empty(), "{} has no source", profile.id);
            assert!(!profile.summary.is_empty(), "{}", profile.id);
            assert!(
                profile.forward() != Impairments::default()
                    || profile.reverse() != Impairments::default(),
                "{} sets nothing",
                profile.id
            );
        }
    }

    #[test]
    fn up_and_down_win_over_both_key_by_key() {
        let library = library();
        let geo = library.get("geo-good").unwrap();
        assert_eq!(geo.forward().rate, None);
        assert_eq!(geo.reverse().rate.unwrap().to_string(), "144mbit");
        assert_eq!(geo.forward().delay, geo.reverse().delay);
        assert!(geo.forward().loss.is_some());
        let lte = library.get("lte-good").unwrap();
        assert_ne!(lte.forward().delay, lte.reverse().delay);
    }
}
