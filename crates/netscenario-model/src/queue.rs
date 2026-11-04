//! The queue of a direction: its size and its discipline (LNK-09, D-079, D-082).

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::units::{Duration, Size};

/// How the packets that wait for the rate are kept and dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Discipline {
    /// A first-in first-out queue that drops what does not fit (`pfifo`, or `bfifo` with a limit in bytes). The default.
    #[default]
    Fifo,
    /// `CoDel` (RFC 8289): drops by the time a packet has waited.
    Codel,
    /// FQ-CoDel (RFC 8290): a queue for each flow, each kept short by `CoDel`.
    FqCodel,
    /// PIE (RFC 8033): drops by a probability that follows the delay of the queue.
    Pie,
}

impl Discipline {
    /// The word of the file.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Discipline::Fifo => "fifo",
            Discipline::Codel => "codel",
            Discipline::FqCodel => "fq-codel",
            Discipline::Pie => "pie",
        }
    }
}

/// The size of a queue: a number of packets (`100p`) or an amount of data (`64kb`). The packets that wait for the rate are
/// counted, not those in the delay behind it (D-079).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueLimit {
    /// A number of packets, at least one.
    Packets(u32),
    /// An amount of data, at least one byte. Only a `fifo` counts bytes.
    Bytes(Size),
}

impl fmt::Display for QueueLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueueLimit::Packets(packets) => write!(f, "{packets}p"),
            QueueLimit::Bytes(size) => write!(f, "{size}"),
        }
    }
}

impl FromStr for QueueLimit {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let example = "a number of packets such as 100p, or an amount of data such as 64kb";
        if let Some(number) = text.strip_suffix('p') {
            let packets: u32 = number
                .parse()
                .map_err(|_| format!("'{text}' is not a limit: write {example}"))?;
            if packets == 0 {
                return Err("a queue holds at least one packet".to_owned());
            }
            return Ok(QueueLimit::Packets(packets));
        }
        text.parse::<Size>()
            .map(QueueLimit::Bytes)
            .map_err(|_| format!("'{text}' is not a limit: write {example}"))
    }
}

impl Serialize for QueueLimit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for QueueLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for QueueLimit {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "QueueLimit".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^[0-9]+p$|^[0-9]+(\\.[0-9]+)?(b|kb|mb|gb|kib|mib|gib)$",
            "description": "The size of the queue: a number of packets (100p) or an amount of data (64kb). Only a fifo counts bytes.",
            "examples": ["100p", "64kb"]
        })
    }
}

/// The queue of a direction (D-082): how many packets wait for the rate and how they are dropped.
///
/// Everything that is not set is the default of the kernel for the discipline: 1000 packets for a `fifo`, `codel` and `pie`,
/// 10240 for `fq-codel`, a target of 5 ms and an interval of 100 ms for `CoDel`, 15 ms and a `t-update` of 15 ms for PIE. The
/// result records what was built.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Queue {
    /// How the queue drops. `fifo` (the default), `codel`, `fq-codel` or `pie`.
    #[serde(default, skip_serializing_if = "Queue::is_fifo")]
    pub discipline: Discipline,
    /// The size of the queue: packets (`100p`) or data (`64kb`, a `fifo` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<QueueLimit>,
    /// The delay the queue accepts (`codel`, `fq-codel`, `pie`): above it the queue starts to drop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Duration>,
    /// The time over which `codel` and `fq-codel` look for a delay that does not go below the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<Duration>,
    /// The time between two updates of the drop probability of `pie`.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "t-update")]
    pub t_update: Option<Duration>,
    /// Mark the packets of a flow that asked for it (ECN) instead of dropping them (`codel`, `fq-codel`, `pie`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecn: Option<bool>,
    /// The number of queues of `fq-codel`, one for each flow that hashes to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flows: Option<u32>,
    /// The bytes a queue of `fq-codel` may send in its turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantum: Option<Size>,
}

impl Queue {
    // serde's `skip_serializing_if` passes a reference
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_fifo(discipline: &Discipline) -> bool {
        *discipline == Discipline::Fifo
    }

    /// A queue of this discipline with the defaults of the kernel.
    #[must_use]
    pub fn of(discipline: Discipline) -> Self {
        Self {
            discipline,
            ..Self::default()
        }
    }

    /// The keys that this discipline does not have but the queue sets, for the validation.
    #[must_use]
    pub fn foreign_keys(&self) -> Vec<&'static str> {
        let mut found = Vec::new();
        let (codel, fq, pie) = (
            self.discipline == Discipline::Codel,
            self.discipline == Discipline::FqCodel,
            self.discipline == Discipline::Pie,
        );
        if self.target.is_some() && !(codel || fq || pie) {
            found.push("target");
        }
        if self.interval.is_some() && !(codel || fq) {
            found.push("interval");
        }
        if self.t_update.is_some() && !pie {
            found.push("t-update");
        }
        if self.ecn.is_some() && !(codel || fq || pie) {
            found.push("ecn");
        }
        if self.flows.is_some() && !fq {
            found.push("flows");
        }
        if self.quantum.is_some() && !fq {
            found.push("quantum");
        }
        found
    }
}
