//! Replay: run a recorded run again and say whether it came out the same (ENV-03, D-075).
//!
//! A run directory holds `scenario.json`, the scenario exactly as it ran (the canonical JSON that the fingerprint in
//! `result.json` is made of), next to `result.json`. [`Recorded::load`] reads both and checks that they belong together:
//! the fingerprint of `scenario.json` has to be the one in the result. The run is then made again with the recorded scenario
//! and the **recorded seed** (a seed that the original run drew is in the result, not in the scenario), and
//! [`compare`] sets the two runs side by side:
//!
//! - **the lost packets**, workload by workload: the sequence numbers that never arrived. For a scenario whose loss repeats
//!   from its seed (`seed.loss_reproducible`) they have to be the same, and a difference is a failed replay. A run whose loss was
//!   not promised to repeat (an outage, corruption, a program that sends, a queue that counted a packet nobody sent) may
//!   differ, and the comparison says so and does not call it a failure.
//! - **the machine**: the provenance of the two runs, field by field. The lost packets of a seed are a function of the
//!   kernel's `netem` and the version of `tc`, so a replay on another machine is told as such.
//!
//! Delays and rates are not compared: they vary from run to run on one machine.
//!
//! [`scenario_diff`] compares a scenario file with the recorded scenario field by field, for the question "has my file changed
//! since that run?".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use netscenario_agent::records;
use netscenario_model::Scenario;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::result::{RunResult, WorkloadResult};
use crate::runset::RUNSET_FILE;
use netscenario_model::sha256_hex;

/// The name of the file with the recorded scenario in the directory of a run.
pub const SCENARIO_FILE: &str = "scenario.json";

/// How many sequence numbers a difference lists; the count is always exact.
pub const LISTED_SEQUENCES: usize = 20;

/// The fingerprint of a scenario: `sha256:` and the SHA-256 of its canonical JSON (the keys in the order of the format,
/// defaults left out).
#[must_use]
pub fn scenario_hash(scenario: &Scenario) -> String {
    format!(
        "sha256:{}",
        sha256_hex(&serde_json::to_vec(scenario).unwrap_or_default())
    )
}

/// The text of `scenario.json`: the canonical JSON, indented, with a line feed at the end.
#[must_use]
pub fn scenario_json(scenario: &Scenario) -> String {
    let mut text = serde_json::to_string_pretty(scenario)
        .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"));
    text.push('\n');
    text
}

/// Why a directory cannot be replayed. All of these are wrong input, not a failed run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// There is no `result.json` in the directory.
    #[error("{0} has no result.json: it is not the directory of a run")]
    NotARun(PathBuf),
    /// The directory is the directory of a set of runs.
    #[error("{0} is the directory of a set of runs: replay one run of it, for example {0}/run-001")]
    IsASet(PathBuf),
    /// The result cannot be read.
    #[error("{path}: {message}")]
    BadResult {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },
    /// The run was made before the scenario was recorded.
    #[error(
        "{0} has no scenario.json: the run was made by a version that did not record the scenario, so it cannot be replayed (run the scenario file again)"
    )]
    NoScenario(PathBuf),
    /// The recorded scenario is not valid.
    #[error("{path}: the recorded scenario is not valid: {message}")]
    BadScenario {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },
    /// `scenario.json` is not the scenario of `result.json`.
    #[error(
        "{path} is not the scenario that this run was made with: its fingerprint is {found}, the result says {recorded}. One of the two files was changed after the run; nothing is replayed"
    )]
    HashMismatch {
        /// The file.
        path: PathBuf,
        /// The fingerprint in `result.json`.
        recorded: String,
        /// The fingerprint of the scenario in `scenario.json`.
        found: String,
    },
}

/// A recorded run: its result and the scenario it was made with, checked against each other.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// The directory of the run.
    pub dir: PathBuf,
    /// The result of the run.
    pub result: RunResult,
    /// The scenario as it ran. Its seed is the one the run was given, which is none if the run drew its own: use
    /// [`Recorded::scenario_to_replay`].
    pub scenario: Scenario,
}

impl Recorded {
    /// Reads and checks the run in `dir`.
    ///
    /// # Errors
    ///
    /// [`ReplayError`]: not a run, a set, a result or scenario that cannot be read, a run without a recorded scenario, or a
    /// scenario whose fingerprint is not the one in the result.
    pub fn load(dir: &Path) -> Result<Self, ReplayError> {
        let result_path = dir.join("result.json");
        let text = match std::fs::read_to_string(&result_path) {
            Ok(text) => text,
            Err(_) if dir.join(RUNSET_FILE).is_file() => {
                return Err(ReplayError::IsASet(dir.to_path_buf()));
            }
            Err(_) => return Err(ReplayError::NotARun(dir.to_path_buf())),
        };
        let result = RunResult::from_json(&text).map_err(|message| ReplayError::BadResult {
            path: result_path.clone(),
            message,
        })?;
        let scenario_path = dir.join(SCENARIO_FILE);
        let text = std::fs::read_to_string(&scenario_path)
            .map_err(|_| ReplayError::NoScenario(dir.to_path_buf()))?;
        let scenario = Scenario::from_yaml(&text, SCENARIO_FILE).map_err(|error| {
            ReplayError::BadScenario {
                path: scenario_path.clone(),
                message: error.to_string(),
            }
        })?;
        let found = scenario_hash(&scenario);
        if found != result.scenario.hash {
            return Err(ReplayError::HashMismatch {
                path: scenario_path,
                recorded: result.scenario.hash,
                found,
            });
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            result,
            scenario,
        })
    }

    /// The scenario to run again: the recorded one with the recorded seed (the run's own, also if it drew it).
    #[must_use]
    pub fn scenario_to_replay(&self) -> Scenario {
        let mut scenario = self.scenario.clone();
        if let Some(seed) = self.result.seed.value {
            scenario.seed = Some(seed);
        }
        // The links that code changed while the run went are built the same way (D-096)
        scenario.live_links = scenario
            .topology
            .links
            .iter()
            .filter(|link| {
                self.result
                    .scenario
                    .live_links
                    .iter()
                    .any(|name| *name == link.id.as_str())
            })
            .map(|link| link.id.clone())
            .collect();
        scenario
    }

    /// How long the original waited for the packets still in the network. The replay waits as long, so that a packet is late
    /// or lost in the replay for the same reason as in the original.
    #[must_use]
    pub fn settle(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.result.timing.settle_ns)
    }
}

/// A field that differs between two runs or between a file and a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Difference {
    /// Where: a path such as `topology.links[0].both.delay`, or a field of the provenance.
    pub path: String,
    /// The value on the recorded side, or `(none)`.
    pub recorded: String,
    /// The value on the other side, or `(none)`.
    pub other: String,
}

fn show(value: Option<&Value>) -> String {
    value.map_or_else(
        || "(none)".to_owned(),
        |value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        },
    )
}

fn diff_values(
    path: &str,
    recorded: Option<&Value>,
    other: Option<&Value>,
    out: &mut Vec<Difference>,
) {
    match (recorded, other) {
        (Some(Value::Object(a)), Some(Value::Object(b))) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let inner = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                diff_values(&inner, a.get(key), b.get(key), out);
            }
        }
        (Some(Value::Array(a)), Some(Value::Array(b))) => {
            for index in 0..a.len().max(b.len()) {
                diff_values(&format!("{path}[{index}]"), a.get(index), b.get(index), out);
            }
        }
        (a, b) if a == b => {}
        (a, b) => out.push(Difference {
            path: path.to_owned(),
            recorded: show(a),
            other: show(b),
        }),
    }
}

/// The fields in which a scenario file differs from the recorded scenario, in the order of the format (`other` is the file).
///
/// A file without a `seed` does not differ from a run that drew its seed: the seed is in the result then, not in the
/// scenario. Every other difference is listed, also the keys of `measure` that an option of the run replaced.
#[must_use]
pub fn scenario_diff(recorded: &Recorded, file: &Scenario) -> Vec<Difference> {
    let mut file = file.clone();
    if file.seed.is_none() && recorded.result.seed.drawn {
        file.seed = recorded.scenario.seed;
    }
    let (a, b) = (
        serde_json::to_value(&recorded.scenario).unwrap_or(Value::Null),
        serde_json::to_value(&file).unwrap_or(Value::Null),
    );
    let mut out = Vec::new();
    diff_values("", Some(&a), Some(&b), &mut out);
    out
}

/// The comparison as JSON (`format` `netscenario-replay`): the two directories, the seed, and the comparison. The command line
/// prints it with `--json`, and the Python layer reads it.
#[must_use]
pub fn replay_json(recorded: &Recorded, replay_dir: &Path, comparison: &Comparison) -> String {
    let value = serde_json::json!({
        "format": "netscenario-replay",
        "version": 0,
        "original": recorded.dir.display().to_string(),
        "replay": replay_dir.display().to_string(),
        "seed": recorded.result.seed.value,
        "listed_sequences": LISTED_SEQUENCES,
        "comparison": comparison,
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// What the replay says about the lost packets of one workload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WorkloadComparison {
    /// The id of the workload.
    pub id: String,
    /// How many packets the original lost (never arrived intact).
    pub lost_original: u64,
    /// How many the replay lost.
    pub lost_replay: u64,
    /// The first sequence numbers lost in the original and not in the replay.
    pub only_original: Vec<u64>,
    /// The first sequence numbers lost in the replay and not in the original.
    pub only_replay: Vec<u64>,
    /// How many sequence numbers are lost in the original and not in the replay (`only_original` lists the first ones).
    pub count_only_original: u64,
    /// How many are lost in the replay and not in the original.
    pub count_only_replay: u64,
    /// How many sequence numbers are lost in one run and not in the other, both ways.
    pub differing: u64,
}

impl WorkloadComparison {
    /// Whether the same packets were lost.
    #[must_use]
    pub fn identical(&self) -> bool {
        self.differing == 0
    }
}

/// How the replay came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Verdict {
    /// The same packets were lost in every workload.
    Reproduced,
    /// Other packets were lost, although the loss was promised to repeat from the seed: the replay failed.
    Differs,
    /// Other packets were lost, and one of the two runs said that its loss does not repeat from the seed (an outage,
    /// corruption, a program that sends, ...): that is no failure, and the reasons are in the comparison.
    NotPromised,
    /// The runs cannot be compared (the original was interrupted, or its records cannot be read); the reason is in the
    /// comparison.
    NotComparable,
}

/// A replay set against its original.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Comparison {
    /// The verdict.
    pub verdict: Verdict,
    /// The lost packets of each workload, in the order of the scenario. Empty if the runs are not comparable.
    pub workloads: Vec<WorkloadComparison>,
    /// The fields of the provenance that differ (the time of the run is not one of them).
    pub provenance: Vec<Difference>,
    /// Whether the machine is not the one of the original: the backend, its details, the kernel or the processor differ.
    pub other_machine: bool,
    /// Why the loss of a run does not repeat from its seed, or why the runs cannot be compared; one sentence each.
    pub reasons: Vec<String>,
}

/// The sequence numbers of the packets that never arrived intact in a workload: below `sent` and in no record.
fn lost_sequences(directory: &Path, workload: &WorkloadResult) -> Result<Vec<u64>, String> {
    let path = directory.join(&workload.records_file);
    let file =
        std::fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let records = records::read(std::io::BufReader::new(file))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let sent = workload.stats.sent;
    let sent_usize = usize::try_from(sent).map_err(|_| "too many packets".to_owned())?;
    let mut arrived = vec![false; sent_usize];
    for record in &records.records {
        if record.sequence < sent {
            // Checked against `sent`, which fits in usize
            #[allow(clippy::cast_possible_truncation)]
            {
                arrived[record.sequence as usize] = true;
            }
        }
    }
    Ok((0..sent)
        .zip(arrived)
        .filter_map(|(sequence, arrived)| (!arrived).then_some(sequence))
        .collect())
}

fn provenance_differences(original: &RunResult, replay: &RunResult) -> (Vec<Difference>, bool) {
    let (a, b) = (&original.provenance, &replay.provenance);
    let mut out = Vec::new();
    let mut machine = false;
    let mut field = |path: &str, left: String, right: String, is_machine: bool| {
        if left != right {
            machine |= is_machine;
            out.push(Difference {
                path: path.to_owned(),
                recorded: left,
                other: right,
            });
        }
    };
    field(
        "netscenario",
        a.netscenario.clone(),
        b.netscenario.clone(),
        false,
    );
    field("backend", a.backend.clone(), b.backend.clone(), true);
    let keys: Vec<&String> = a
        .backend_details
        .keys()
        .chain(b.backend_details.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for key in keys {
        let get = |details: &BTreeMap<String, String>| {
            details
                .get(key)
                .cloned()
                .unwrap_or_else(|| "(none)".to_owned())
        };
        field(
            &format!("backend_details.{key}"),
            get(&a.backend_details),
            get(&b.backend_details),
            true,
        );
    }
    let optional = |value: &Option<String>| value.clone().unwrap_or_else(|| "(unknown)".to_owned());
    field("kernel", optional(&a.kernel), optional(&b.kernel), true);
    field(
        "cpu_model",
        optional(&a.cpu_model),
        optional(&b.cpu_model),
        true,
    );
    let cpus =
        |value: Option<usize>| value.map_or_else(|| "(unknown)".to_owned(), |n| n.to_string());
    field("cpus", cpus(a.cpus), cpus(b.cpus), true);
    (out, machine)
}

/// Sets a replay against its original. `original_dir` and `replay_dir` are the directories of the two runs, where the records
/// of the workloads are.
#[must_use]
pub fn compare(
    original: &RunResult,
    original_dir: &Path,
    replay: &RunResult,
    replay_dir: &Path,
) -> Comparison {
    let (provenance, other_machine) = provenance_differences(original, replay);
    let mut reasons: Vec<String> = Vec::new();
    let unreadable = |reasons: Vec<String>| Comparison {
        verdict: Verdict::NotComparable,
        workloads: Vec::new(),
        provenance: provenance.clone(),
        other_machine,
        reasons,
    };
    if original.timing.interrupted_ns.is_some() {
        return unreadable(vec![
            "the original run was interrupted, so it did not send the packets that the replay sends".to_owned(),
        ]);
    }
    if replay.timing.interrupted_ns.is_some() {
        return unreadable(vec!["the replay was interrupted".to_owned()]);
    }
    if original.workloads.len() != replay.workloads.len() {
        return unreadable(vec![format!(
            "the original has {} workloads and the replay {}",
            original.workloads.len(),
            replay.workloads.len()
        )]);
    }
    let mut workloads = Vec::new();
    for (before, after) in original.workloads.iter().zip(&replay.workloads) {
        let lost = (
            lost_sequences(original_dir, before),
            lost_sequences(replay_dir, after),
        );
        let (Ok(lost_original), Ok(lost_replay)) = lost else {
            let reason = [lost.0.err(), lost.1.err()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ");
            return unreadable(vec![format!(
                "the records of '{}' cannot be read: {reason}",
                before.id
            )]);
        };
        let set = |list: &[u64]| {
            list.iter()
                .copied()
                .collect::<std::collections::BTreeSet<u64>>()
        };
        let (a, b) = (set(&lost_original), set(&lost_replay));
        let only_original: Vec<u64> = a.difference(&b).copied().collect();
        let only_replay: Vec<u64> = b.difference(&a).copied().collect();
        workloads.push(WorkloadComparison {
            id: before.id.clone(),
            lost_original: lost_original.len() as u64,
            lost_replay: lost_replay.len() as u64,
            count_only_original: only_original.len() as u64,
            count_only_replay: only_replay.len() as u64,
            differing: (only_original.len() + only_replay.len()) as u64,
            only_original: only_original.into_iter().take(LISTED_SEQUENCES).collect(),
            only_replay: only_replay.into_iter().take(LISTED_SEQUENCES).collect(),
        });
    }
    let identical = workloads.iter().all(WorkloadComparison::identical);
    let promised = original.seed.loss_reproducible && replay.seed.loss_reproducible;
    if !original.seed.loss_reproducible {
        reasons.extend(
            original
                .seed
                .not_reproducible_because
                .iter()
                .map(|reason| format!("original: {reason}")),
        );
    }
    if !replay.seed.loss_reproducible {
        reasons.extend(
            replay
                .seed
                .not_reproducible_because
                .iter()
                .map(|reason| format!("replay: {reason}")),
        );
    }
    let verdict = match (identical, promised) {
        (true, _) => Verdict::Reproduced,
        (false, true) => Verdict::Differs,
        (false, false) => Verdict::NotPromised,
    };
    Comparison {
        verdict,
        workloads,
        provenance,
        other_machine,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use netscenario_agent::records::{Record, Writer};

    use super::*;
    use crate::result::tests::example;

    const YAML: &str = "version: 0\nname: t\nseed: 5\nduration: 1s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: { delay: 20ms, loss: { model: random, p: 20% } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 800kbit, packet-size: 1000b }\n";

    fn scenario() -> Scenario {
        Scenario::from_yaml(YAML, "t.yaml").unwrap()
    }

    fn directory(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("netscenario-replay-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A directory of a run of `scenario`: its `scenario.json` and a result that carries the fingerprint of it.
    fn recorded_run(name: &str, scenario: &Scenario) -> PathBuf {
        let dir = directory(name);
        std::fs::write(dir.join(SCENARIO_FILE), scenario_json(scenario)).unwrap();
        let mut result = example();
        result.scenario.hash = scenario_hash(scenario);
        std::fs::write(dir.join("result.json"), result.to_json()).unwrap();
        dir
    }

    #[test]
    fn a_recorded_run_is_read_and_its_scenario_matches_the_fingerprint() {
        let dir = recorded_run("load", &scenario());
        let recorded = Recorded::load(&dir).unwrap();
        assert_eq!(recorded.scenario, scenario());
        assert_eq!(recorded.result.scenario.hash, scenario_hash(&scenario()));
        // The file is the canonical JSON: it reads back to the same fingerprint
        assert!(
            std::fs::read_to_string(dir.join(SCENARIO_FILE))
                .unwrap()
                .ends_with("}\n")
        );
    }

    /// The scenario of the original is run with the seed the original had, and a seed that was drawn is the one in the result.
    #[test]
    fn the_recorded_seed_is_the_one_that_is_replayed() {
        let mut drawn = scenario();
        drawn.seed = None;
        let dir = directory("seed");
        std::fs::write(dir.join(SCENARIO_FILE), scenario_json(&drawn)).unwrap();
        let mut result = example();
        result.scenario.hash = scenario_hash(&drawn);
        result.seed.value = Some(987);
        result.seed.drawn = true;
        std::fs::write(dir.join("result.json"), result.to_json()).unwrap();
        let recorded = Recorded::load(&dir).unwrap();
        assert_eq!(recorded.scenario.seed, None, "the scenario is as it ran");
        assert_eq!(recorded.scenario_to_replay().seed, Some(987));
        // The wait is the one the original made
        assert_eq!(
            recorded.settle(),
            std::time::Duration::from_nanos(recorded.result.timing.settle_ns)
        );
    }

    /// Canary: every way a directory can be wrong is named, and nothing is guessed.
    #[test]
    fn a_directory_that_cannot_be_replayed_says_why() {
        let nothing = directory("nothing");
        assert!(matches!(
            Recorded::load(&nothing),
            Err(ReplayError::NotARun(_))
        ));

        let set = directory("set");
        std::fs::write(set.join(RUNSET_FILE), "{}").unwrap();
        let error = Recorded::load(&set).unwrap_err();
        assert!(matches!(error, ReplayError::IsASet(_)));
        assert!(error.to_string().contains("run-001"), "{error}");

        let old = recorded_run("old", &scenario());
        std::fs::remove_file(old.join(SCENARIO_FILE)).unwrap();
        let error = Recorded::load(&old).unwrap_err();
        assert!(matches!(error, ReplayError::NoScenario(_)));
        assert!(
            error.to_string().contains("did not record the scenario"),
            "{error}"
        );

        let broken = recorded_run("broken", &scenario());
        std::fs::write(broken.join(SCENARIO_FILE), "{\"version\": 0}").unwrap();
        assert!(matches!(
            Recorded::load(&broken),
            Err(ReplayError::BadScenario { .. })
        ));

        let bad_result = recorded_run("badresult", &scenario());
        std::fs::write(bad_result.join("result.json"), "{}").unwrap();
        assert!(matches!(
            Recorded::load(&bad_result),
            Err(ReplayError::BadResult { .. })
        ));
    }

    #[test]
    fn a_scenario_that_was_changed_after_the_run_is_refused_with_both_fingerprints() {
        let dir = recorded_run("changed", &scenario());
        let mut changed = scenario();
        changed.duration = "2s".parse().unwrap();
        std::fs::write(dir.join(SCENARIO_FILE), scenario_json(&changed)).unwrap();
        let error = Recorded::load(&dir).unwrap_err();
        let ReplayError::HashMismatch {
            recorded, found, ..
        } = &error
        else {
            panic!("{error}");
        };
        assert_eq!(recorded, &scenario_hash(&scenario()));
        assert_eq!(found, &scenario_hash(&changed));
        assert_ne!(recorded, found);
        assert!(error.to_string().contains("nothing is replayed"), "{error}");
    }

    #[test]
    fn the_fingerprint_does_not_depend_on_the_layout_of_the_file() {
        let dir = recorded_run("layout", &scenario());
        // Compact instead of indented: the same scenario, so the same fingerprint
        std::fs::write(
            dir.join(SCENARIO_FILE),
            serde_json::to_string(&scenario()).unwrap(),
        )
        .unwrap();
        assert!(Recorded::load(&dir).is_ok());
    }

    fn recorded(scenario: &Scenario, drawn: bool) -> Recorded {
        let mut result = example();
        result.scenario.hash = scenario_hash(scenario);
        result.seed.drawn = drawn;
        Recorded {
            dir: PathBuf::from("run"),
            result,
            scenario: scenario.clone(),
        }
    }

    #[test]
    fn a_file_that_is_the_recorded_scenario_has_no_difference() {
        assert_eq!(
            scenario_diff(&recorded(&scenario(), false), &scenario()),
            []
        );
    }

    #[test]
    fn a_changed_field_is_named_by_its_path_with_both_values() {
        let changed = Scenario::from_yaml(
            &YAML
                .replace("delay: 20ms", "delay: 30ms")
                .replace("rate: 800kbit", "rate: 900kbit"),
            "t.yaml",
        )
        .unwrap();
        let found = scenario_diff(&recorded(&scenario(), false), &changed);
        let paths: Vec<&str> = found.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(
            paths,
            ["topology.links[0].both.delay", "workloads[0].rate"],
            "{found:?}"
        );
        assert_eq!(found[0].recorded, "20ms");
        assert_eq!(found[0].other, "30ms");
    }

    /// A file without a seed does not differ from a run that drew one; a file with another seed does.
    #[test]
    fn the_seed_of_a_run_that_drew_it_is_not_a_difference() {
        let mut ran = scenario();
        ran.seed = None;
        let mut file_with_seed = scenario();
        file_with_seed.seed = Some(9);
        assert_eq!(scenario_diff(&recorded(&ran, true), &ran), []);
        let found = scenario_diff(&recorded(&ran, true), &file_with_seed);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            (
                found[0].path.as_str(),
                found[0].recorded.as_str(),
                found[0].other.as_str()
            ),
            ("seed", "(none)", "9")
        );
        // A run with the seed 5 in the file, against a file without a seed: that is a change of the file
        let mut without = scenario();
        without.seed = None;
        let found = scenario_diff(&recorded(&scenario(), false), &without);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "seed");
    }

    #[test]
    fn an_added_workload_is_a_difference_at_its_index() {
        let more = Scenario::from_yaml(
            &format!("{YAML}  - {{ id: v, kind: udp-cbr, from: b, to: a, rate: 100kbit, packet-size: 500b }}\n"),
            "t.yaml",
        )
        .unwrap();
        let found = scenario_diff(&recorded(&scenario(), false), &more);
        assert!(
            found
                .iter()
                .any(|d| d.path.starts_with("workloads[1]") && d.recorded == "(none)"),
            "{found:?}"
        );
    }

    /// A run directory whose workload `w` sent `sent` packets and of which the packets in `arrived` arrived.
    fn run_with_records(name: &str, sent: u64, arrived: &[u64]) -> (RunResult, PathBuf) {
        let dir = directory(name);
        let mut writer = Writer::new(
            std::fs::File::create(dir.join("flow-0.records")).unwrap(),
            0,
        )
        .unwrap();
        for sequence in arrived {
            writer
                .write(&Record {
                    sequence: *sequence,
                    send_ns: 0,
                    receive_ns: 1,
                })
                .unwrap();
        }
        writer.finish().unwrap();
        let mut result = example();
        result.workloads[0].stats.sent = sent;
        result.workloads[0].records_file = "flow-0.records".to_owned();
        (result, dir)
    }

    fn all_but(sent: u64, lost: &[u64]) -> Vec<u64> {
        (0..sent)
            .filter(|sequence| !lost.contains(sequence))
            .collect()
    }

    #[test]
    fn the_same_lost_packets_are_a_reproduced_replay() {
        let (a, a_dir) = run_with_records("same-a", 10, &all_but(10, &[2, 7]));
        let (b, b_dir) = run_with_records("same-b", 10, &all_but(10, &[7, 2]));
        let found = compare(&a, &a_dir, &b, &b_dir);
        assert_eq!(found.verdict, Verdict::Reproduced);
        assert_eq!(
            (
                found.workloads[0].lost_original,
                found.workloads[0].lost_replay
            ),
            (2, 2)
        );
        assert!(found.workloads[0].identical());
        assert_eq!(found.provenance, []);
        assert!(!found.other_machine);
    }

    /// Canary: other lost packets must not pass as the same, however alike the counts are.
    #[test]
    fn other_lost_packets_with_a_promised_loss_are_a_failed_replay_and_name_the_packets() {
        let (a, a_dir) = run_with_records("diff-a", 10, &all_but(10, &[2, 7]));
        let (b, b_dir) = run_with_records("diff-b", 10, &all_but(10, &[2, 8]));
        let found = compare(&a, &a_dir, &b, &b_dir);
        assert_eq!(found.verdict, Verdict::Differs);
        let workload = &found.workloads[0];
        assert_eq!(
            (workload.lost_original, workload.lost_replay),
            (2, 2),
            "the counts are alike"
        );
        assert_eq!(
            (
                workload.only_original.as_slice(),
                workload.only_replay.as_slice()
            ),
            (&[7][..], &[8][..])
        );
        assert_eq!(workload.differing, 2);
    }

    #[test]
    fn a_difference_is_no_failure_where_the_loss_was_not_promised_to_repeat() {
        let (mut a, a_dir) = run_with_records("np-a", 10, &all_but(10, &[2]));
        let (b, b_dir) = run_with_records("np-b", 10, &all_but(10, &[3]));
        a.seed.loss_reproducible = false;
        a.seed.not_reproducible_because = vec!["the scenario has an outage".to_owned()];
        let found = compare(&a, &a_dir, &b, &b_dir);
        assert_eq!(found.verdict, Verdict::NotPromised);
        assert_eq!(found.reasons, ["original: the scenario has an outage"]);
        assert!(!found.workloads[0].identical());
    }

    #[test]
    fn the_numbers_of_a_long_difference_are_exact_and_the_list_is_cut() {
        let lost: Vec<u64> = (0..100).collect();
        let (a, a_dir) = run_with_records("long-a", 200, &all_but(200, &lost));
        let (b, b_dir) = run_with_records("long-b", 200, &[]);
        let found = compare(&a, &a_dir, &b, &b_dir);
        let workload = &found.workloads[0];
        assert_eq!(
            (
                workload.lost_original,
                workload.lost_replay,
                workload.differing
            ),
            (100, 200, 100)
        );
        assert_eq!(workload.only_replay.len(), LISTED_SEQUENCES);
        assert_eq!(
            workload.only_replay[0], 100,
            "the first of the sequence numbers that only the replay lost"
        );
    }

    #[test]
    fn another_machine_is_told_field_by_field() {
        let (a, a_dir) = run_with_records("machine-a", 4, &[0, 1, 2, 3]);
        let (mut b, b_dir) = run_with_records("machine-b", 4, &[0, 1, 2, 3]);
        let mut a = a;
        a.provenance.kernel = Some("6.12.1".to_owned());
        b.provenance.kernel = Some("6.8.0".to_owned());
        a.provenance.backend_details =
            BTreeMap::from([("tc".to_owned(), "iproute2 6.15.0".to_owned())]);
        b.provenance.backend_details =
            BTreeMap::from([("tc".to_owned(), "iproute2 6.1.0".to_owned())]);
        b.provenance.netscenario = "0.2.0".to_owned();
        b.provenance.started_at = "2030-01-01T00:00:00Z".to_owned();
        let found = compare(&a, &a_dir, &b, &b_dir);
        assert!(found.other_machine);
        let by_path: BTreeMap<&str, (&str, &str)> = found
            .provenance
            .iter()
            .map(|d| (d.path.as_str(), (d.recorded.as_str(), d.other.as_str())))
            .collect();
        assert_eq!(by_path["kernel"], ("6.12.1", "6.8.0"));
        assert_eq!(
            by_path["backend_details.tc"],
            ("iproute2 6.15.0", "iproute2 6.1.0")
        );
        assert_eq!(by_path["netscenario"], ("0.0.0", "0.2.0"));
        assert!(
            !by_path.contains_key("started_at"),
            "the time of a run is not a difference"
        );
        // A version of netscenario alone is not another machine
        let (c, c_dir) = run_with_records("machine-c", 4, &[0, 1, 2, 3]);
        let (mut d, d_dir) = run_with_records("machine-d", 4, &[0, 1, 2, 3]);
        d.provenance.netscenario = "9.9.9".to_owned();
        let found = compare(&c, &c_dir, &d, &d_dir);
        assert!(!found.other_machine);
        assert_eq!(found.provenance.len(), 1);
    }

    #[test]
    fn runs_that_cannot_be_compared_say_why() {
        let (mut a, a_dir) = run_with_records("nc-a", 10, &all_but(10, &[1]));
        let (b, b_dir) = run_with_records("nc-b", 10, &all_but(10, &[1]));
        a.timing.interrupted_ns = Some(5);
        let found = compare(&a, &a_dir, &b, &b_dir);
        assert_eq!(found.verdict, Verdict::NotComparable);
        assert!(
            found.reasons[0].contains("interrupted"),
            "{:?}",
            found.reasons
        );
        // Records that are gone
        let (c, _) = run_with_records("nc-c", 10, &[]);
        let found = compare(&c, &directory("nc-nothing"), &b, &b_dir);
        assert_eq!(found.verdict, Verdict::NotComparable);
        assert!(
            found.reasons[0].contains("cannot be read"),
            "{:?}",
            found.reasons
        );
    }
}
