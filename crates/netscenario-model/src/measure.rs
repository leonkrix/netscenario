//! What is measured besides the built-in statistics, and how the numbers are derived (D-073): the width of the bins of the
//! series, the definition of the time to recover, the packets to capture and the per-packet export.
//!
//! These keys belong to the scenario file and not to the options of a run, because a scenario is the whole experiment:
//! what is run **and what is measured**. A threshold on the time to recover (`expect`, later) only means something with the
//! definition of the time to recover next to it, and the hash of the scenario in the result tells whether two results were
//! measured the same way. A command line option or an argument of the Python call with the same name overrides the key for
//! one run, and the run records what it used.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::units::{Duration, Name, Probability};

/// The view the level of a bin is made from for the time to recover (see `docs/RESULTS.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryBasis {
    /// Of the packets sent in a bin, the share that arrived (in the bin or later).
    Send,
    /// Of the packets sent in a bin, the share the receiver got in that bin (at most all of them).
    Arrival,
}

/// The series of each workload by time bin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SeriesSettings {
    /// The width of a bin. Default `1s`. At least `1ms`, and the measured phase may have at most a million bins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<Duration>,
}

/// How the time to recover after an event is worked out. Every key has a default, so a section can name only what it changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RecoverySettings {
    /// The view the level of a bin is made from: `send` (default) or `arrival`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<RecoveryBasis>,
    /// How many bins before the disturbance make the baseline level. Default 5.
    #[serde(
        default,
        rename = "baseline-bins",
        skip_serializing_if = "Option::is_none"
    )]
    pub baseline_bins: Option<u32>,
    /// How many bins in a row have to be back at the level. Default 3.
    #[serde(
        default,
        rename = "window-bins",
        skip_serializing_if = "Option::is_none"
    )]
    pub window_bins: Option<u32>,
    /// The share of the baseline that has to be reached again, above 0 % and at most 100 %. Default 90 %.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<Probability>,
}

/// What is measured and kept besides the statistics of each workload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Measure {
    /// The series of each workload by time bin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<SeriesSettings>,
    /// The time to recover after each event of the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoverySettings>,
    /// The links whose interfaces are captured, at both ends, as pcap files (PRB-09). A capture shows what the interface
    /// carried after the queue, so a packet that the link dropped is in no file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capture: Vec<Name>,
    /// Whether to write `packets-<id>.jsonl` with one line for each packet of each workload. Default `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub packets: bool,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "the signature that `skip_serializing_if` calls"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl Measure {
    /// Whether nothing is asked for: the section can be left out of the file.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The most bins the series of a workload may have.
pub const MAX_BINS: u64 = 1_000_000;
/// The shortest bin.
pub const MIN_BIN: Duration = Duration::from_nanos(1_000_000);

/// What one run asks for besides the file: the same keys as [`Measure`], each only if it was given, from the command line, the
/// Python call or a program. [`Measure::apply`] puts them over the file, and the run records what it used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MeasureOverrides {
    /// `measure.series.bin`.
    pub bin: Option<Duration>,
    /// `measure.recovery.basis`.
    pub basis: Option<RecoveryBasis>,
    /// `measure.recovery.baseline-bins`.
    pub baseline_bins: Option<u32>,
    /// `measure.recovery.window-bins`.
    pub window_bins: Option<u32>,
    /// `measure.recovery.fraction`.
    pub fraction: Option<Probability>,
    /// `measure.capture`, replacing the list of the file.
    pub capture: Option<Vec<Name>>,
    /// `measure.packets`: `Some(true)` switches the export on, `Some(false)` off, and `None` leaves the value of the file.
    pub packets: Option<bool>,
}

impl Measure {
    /// Puts the settings of a run over the section of the file. Check the scenario again afterwards
    /// ([`Scenario::validate`](crate::Scenario::validate)): an override can be as wrong as a key.
    pub fn apply(&mut self, overrides: &MeasureOverrides) {
        if let Some(bin) = overrides.bin {
            self.series.get_or_insert_with(SeriesSettings::default).bin = Some(bin);
        }
        if overrides.basis.is_some()
            || overrides.baseline_bins.is_some()
            || overrides.window_bins.is_some()
            || overrides.fraction.is_some()
        {
            let recovery = self.recovery.get_or_insert_with(RecoverySettings::default);
            recovery.basis = overrides.basis.or(recovery.basis);
            recovery.baseline_bins = overrides.baseline_bins.or(recovery.baseline_bins);
            recovery.window_bins = overrides.window_bins.or(recovery.window_bins);
            recovery.fraction = overrides.fraction.or(recovery.fraction);
        }
        if let Some(capture) = &overrides.capture {
            self.capture.clone_from(capture);
        }
        if let Some(packets) = overrides.packets {
            self.packets = packets;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_replaces_what_it_names_and_leaves_the_rest() {
        let mut measure = Measure {
            recovery: Some(RecoverySettings {
                basis: Some(RecoveryBasis::Arrival),
                window_bins: Some(4),
                ..RecoverySettings::default()
            }),
            packets: true,
            ..Measure::default()
        };
        let overrides = MeasureOverrides {
            window_bins: Some(2),
            bin: Some(Duration::from_nanos(500_000_000)),
            capture: Some(vec!["l".parse().unwrap()]),
            ..MeasureOverrides::default()
        };
        measure.apply(&overrides);
        let recovery = measure.recovery.as_ref().unwrap();
        // The window is replaced, the basis of the file stays, and the export of the file is not switched off by silence
        assert_eq!(
            (recovery.window_bins, recovery.basis),
            (Some(2), Some(RecoveryBasis::Arrival))
        );
        assert_eq!(
            measure.series.as_ref().unwrap().bin,
            Some(Duration::from_nanos(500_000_000))
        );
        assert_eq!(measure.capture.len(), 1);
        assert!(measure.packets);
        // Canary: nothing asked for changes nothing
        let before = measure.clone();
        measure.apply(&MeasureOverrides::default());
        assert_eq!(measure, before);
    }
}
