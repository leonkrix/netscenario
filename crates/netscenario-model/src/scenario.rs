//! The scenario: the types that a file describes and that a program can build.
//!
//! The same types are read from a file ([`Scenario::from_yaml`]), built in code, written out again, and
//! described by the JSON Schema. The rules that a type system cannot state (a node must exist, a stop must come
//! after the start) are checked by [`Scenario::validate`], which both the file and the code go through.

use schemars::JsonSchema;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::units::{Duration, Name, Probability, Rate, Size};

/// The version of the scenario format that this crate reads: the integer `0`.
///
/// Version 0 is the series before 1.0 and may change in any minor release (see `docs/SCENARIO-FORMAT.md`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FormatVersion;

impl FormatVersion {
    /// The number that the file carries.
    pub const NUMBER: u64 = 0;
}

impl Serialize for FormatVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(Self::NUMBER)
    }
}

impl<'de> Deserialize<'de> for FormatVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct VersionVisitor;
        impl Visitor<'_> for VersionVisitor {
            type Value = FormatVersion;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("the format version, the number 0")
            }

            fn visit_u64<E: de::Error>(self, number: u64) -> Result<FormatVersion, E> {
                if number == FormatVersion::NUMBER {
                    Ok(FormatVersion)
                } else {
                    Err(E::custom(format!(
                        "this version of netscenario reads version {} of the scenario format, the file says version {number}",
                        FormatVersion::NUMBER
                    )))
                }
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<FormatVersion, E> {
                Err(E::custom(format!(
                    "the format version is the number {}, not the text '{text}'",
                    FormatVersion::NUMBER
                )))
            }

            fn visit_i64<E: de::Error>(self, number: i64) -> Result<FormatVersion, E> {
                u64::try_from(number).map_or_else(
                    |_| {
                        Err(E::custom(format!(
                            "the format version is the number 0, not {number}"
                        )))
                    },
                    |number| self.visit_u64(number),
                )
            }
        }
        deserializer.deserialize_any(VersionVisitor)
    }
}

impl JsonSchema for FormatVersion {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FormatVersion".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "const": 0,
            "description": "The version of the scenario format. Version 0 is the series before 1.0 and may change in any minor release."
        })
    }
}

/// The backend that makes the network real. Only the Kernel backend exists in release 0.1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    /// The backend is chosen by the tool: the Kernel backend in release 0.1.
    #[default]
    Auto,
    /// Network namespaces, virtual Ethernet pairs and `netem`.
    Kernel,
}

impl Backend {
    // serde's `skip_serializing_if` passes a reference
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_default(&self) -> bool {
        *self == Self::Auto
    }
}

/// A scenario: a network, what runs in it, and for how long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "netscenario scenario")]
pub struct Scenario {
    /// The format version: 0.
    pub version: FormatVersion,
    /// The name of the scenario. Appears in the result and names its files.
    pub name: Name,
    /// The seed from which the seeds of all random parts are derived. Without it the tool draws one and
    /// records it in the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The length of the measured phase, from the start of the workloads. Greater than zero.
    pub duration: Duration,
    /// The backend. `auto` (the default) or `kernel`.
    #[serde(default, skip_serializing_if = "Backend::is_default")]
    pub backend: Backend,
    /// The nodes and the link between them.
    pub topology: Topology,
    /// What sends traffic. At least one.
    pub workloads: Vec<Workload>,
}

/// The nodes and the links. In release 0.1: two nodes and one link (point to point).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    /// The names of the nodes.
    pub nodes: Vec<Name>,
    /// The links between the nodes.
    pub links: Vec<Link>,
}

/// A link between two nodes, with the impairments of each direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// The name of the link.
    pub id: Name,
    /// The two nodes. `forward` is the direction from the first to the second.
    pub between: [Name; 2],
    /// The impairments from the first node to the second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<Impairments>,
    /// The impairments from the second node to the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<Impairments>,
    /// The same impairments in both directions. Cannot be combined with `forward` or `reverse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub both: Option<Impairments>,
}

/// What a direction of a link does to the packets. Everything that is not set is not impaired.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Impairments {
    /// A fixed delay (`netem delay`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<Duration>,
    /// Variation of the delay. Needs `delay`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter: Option<Jitter>,
    /// Packet loss: independent, or in bursts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss: Option<Loss>,
    /// Packets that are sent on at once while the others are held back for the delay (`netem reorder`).
    /// Needs `delay`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reorder: Option<Reorder>,
    /// The chance that a packet is sent twice (`netem duplicate`). The copy is impaired on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate: Option<Probability>,
    /// The chance that one bit of a packet is flipped (`netem corrupt`). A receiver with the UDP checksum
    /// on drops the packet (D-040).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrupt: Option<Probability>,
    /// A limit on the data rate (`netem rate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<Rate>,
}

/// Variation of the delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Jitter {
    /// How much the delay varies: the delay lies within this amount above and below the fixed delay
    /// (`uniform`) or has this standard deviation (`normal`).
    pub amount: Duration,
    /// The distribution. `uniform` (the default) or `normal`.
    #[serde(default, skip_serializing_if = "JitterDistribution::is_default")]
    pub distribution: JitterDistribution,
}

/// The distribution of the variation of the delay.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum JitterDistribution {
    /// Every delay between delay minus amount and delay plus amount is equally likely.
    #[default]
    Uniform,
    /// A bell curve around the delay, cut at about four standard deviations.
    Normal,
}

impl JitterDistribution {
    // serde's `skip_serializing_if` passes a reference
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_default(&self) -> bool {
        *self == Self::Uniform
    }
}

/// Packet loss. The model is chosen by `model`, and the parameters are those of `netem`, without aliases
/// (D-048).
///
/// It is read by hand (see `LossFields`), not by a derived reader for a tagged enum: that one collects the
/// mapping first, and an error in a value is then reported at the start of the mapping and not at the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(
    tag = "model",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case",
    deny_unknown_fields
)]
pub enum Loss {
    /// Every packet is lost with the same chance, independent of the others (`netem loss random`).
    Random {
        /// The chance that a packet is lost.
        p: Probability,
    },
    /// Two states, a good and a bad one, with a loss in each (`netem loss gemodel p r 1-h 1-k`). The mean
    /// time in the bad state is `1 / r` packets. All four are required: a missing value would be a default
    /// of `tc` that the file does not show.
    GilbertElliott {
        /// The chance, for each packet, to go from the good to the bad state.
        p: Probability,
        /// The chance, for each packet, to go from the bad to the good state.
        r: Probability,
        /// The chance that a packet is lost while in the bad state (the third argument of `gemodel`).
        loss_bad: Probability,
        /// The chance that a packet is lost while in the good state (the fourth argument of `gemodel`).
        loss_good: Probability,
    },
    /// Four states (`netem loss state p13 p31 p32 p23 p14`): 1 receiving in a gap, 2 receiving inside a
    /// burst, 3 losing inside a burst, 4 losing a single packet in a gap. The numbers are the transitions
    /// between the states they name. From state 1 the chances of `p13` and `p14` add up, and from state 3
    /// those of `p31` and `p32`; neither sum may be above 100 %.
    Markov4 {
        /// From state 1 to state 3.
        p13: Probability,
        /// From state 3 to state 1.
        p31: Probability,
        /// From state 3 to state 2.
        p32: Probability,
        /// From state 2 to state 3.
        p23: Probability,
        /// From state 1 to state 4.
        p14: Probability,
    },
}

/// The name of a loss model in the file.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum LossName {
    Random,
    GilbertElliott,
    Markov4,
}

/// Every key that a loss can have, for the reader: which of them belong to the model is checked after.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct LossFields {
    model: LossName,
    p: Option<Probability>,
    r: Option<Probability>,
    loss_bad: Option<Probability>,
    loss_good: Option<Probability>,
    p13: Option<Probability>,
    p31: Option<Probability>,
    p32: Option<Probability>,
    p23: Option<Probability>,
    p14: Option<Probability>,
}

impl<'de> Deserialize<'de> for Loss {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let fields = LossFields::deserialize(deserializer)?;
        let given = [
            ("p", fields.p),
            ("r", fields.r),
            ("loss-bad", fields.loss_bad),
            ("loss-good", fields.loss_good),
            ("p13", fields.p13),
            ("p31", fields.p31),
            ("p32", fields.p32),
            ("p23", fields.p23),
            ("p14", fields.p14),
        ];
        let (name, wanted): (&str, &[&str]) = match fields.model {
            LossName::Random => ("random", &["p"]),
            LossName::GilbertElliott => ("gilbert-elliott", &["p", "r", "loss-bad", "loss-good"]),
            LossName::Markov4 => ("markov4", &["p13", "p31", "p32", "p23", "p14"]),
        };
        // Every key of the model, and no other (a file is checked for both before it gets here, with the place)
        for (key, value) in given {
            match (value.is_some(), wanted.contains(&key)) {
                (true, false) => {
                    return Err(Error::custom(format!(
                        "the key `{key}` is not used by the loss model `{name}`"
                    )));
                }
                (false, true) => return Err(Error::missing_field(key)),
                _ => {}
            }
        }
        let value = |key: &str| {
            given
                .iter()
                .find(|(name, _)| *name == key)
                .and_then(|(_, value)| *value)
                .ok_or_else(|| Error::missing_field("loss"))
        };
        Ok(match fields.model {
            LossName::Random => Loss::Random { p: value("p")? },
            LossName::GilbertElliott => Loss::GilbertElliott {
                p: value("p")?,
                r: value("r")?,
                loss_bad: value("loss-bad")?,
                loss_good: value("loss-good")?,
            },
            LossName::Markov4 => Loss::Markov4 {
                p13: value("p13")?,
                p31: value("p31")?,
                p32: value("p32")?,
                p23: value("p23")?,
                p14: value("p14")?,
            },
        })
    }
}

/// Reordering by holding packets back (`netem reorder P gap G`).
///
/// With a `delay`, a share of the packets is sent on at once and the others wait for the delay, so the ones
/// that wait arrive after later packets. Measured in D-048: `share` is the share sent on at once, and with
/// `gap` it is `1 / (gap - 1 + 1 / share)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reorder {
    /// The chance that a packet is sent on at once.
    pub share: Probability,
    /// After a packet that was sent on at once, the next `gap - 1` packets are held back before another one
    /// can be sent on at once; 1 (the default) means no such rule. At least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<u32>,
}

/// A workload: something that sends traffic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    /// The name of the workload. Metrics are named after it.
    pub id: Name,
    /// The kind of workload. Only `udp-cbr` in release 0.1.
    pub kind: WorkloadKind,
    /// The node that sends.
    pub from: Name,
    /// The node that receives.
    pub to: Name,
    /// The rate of the payload.
    pub rate: Rate,
    /// The payload of one packet.
    #[serde(rename = "packet-size")]
    pub packet_size: Size,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Duration>,
}

/// The kind of a workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum WorkloadKind {
    /// UDP at a constant bit rate.
    UdpCbr,
}
