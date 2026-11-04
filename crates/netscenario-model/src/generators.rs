//! The generators of traffic besides the constant bit rate: Poisson, on/off and bursts (WRK-02, D-071).
//!
//! They send UDP packets of one size from one node to another like [`UdpCbr`], and differ in when the packets leave.
//! The moments are drawn from a seeded generator whose algorithm is written down (`docs/AGENT.md`), so the same seed of
//! the scenario gives the same packets at the same moments in every run. [`Generator`] is the view that code which only
//! needs "a sender of UDP" uses, whatever the pattern.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, Serializer};

use crate::ip::IpFamily;
use crate::scenario::{UdpCbr, UdpChecksum, WorkloadKind, checksum_is_on};
use crate::units::{Duration, Name, Rate, Size};

/// Whether the lengths of the phases of an on/off source, and the pauses between bursts, are exactly what the file says
/// or are drawn from an exponential distribution with that mean.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PhaseDistribution {
    /// Every phase lasts exactly the time given. The default: the source is a pattern you can calculate by hand.
    #[default]
    Fixed,
    /// The lengths are exponentially distributed with the time given as the mean (a Markov source).
    Exponential,
}

impl PhaseDistribution {
    /// For `skip_serializing_if`, which passes a reference.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "the signature that `skip_serializing_if` calls"
    )]
    fn is_fixed(&self) -> bool {
        *self == Self::Fixed
    }
}

/// A workload that sends UDP packets as a Poisson process: the gaps between the packets are exponentially distributed,
/// and `rate` is the mean rate of the payload.
///
/// New keys are added to this type in later releases, so it cannot be built with a struct expression outside this
/// crate: use [`UdpPoisson::new`] or read a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UdpPoisson {
    /// The name of the workload. Metrics are named after it.
    pub id: Name,
    /// The node that sends.
    pub from: Name,
    /// The node that receives.
    pub to: Name,
    /// The mean rate of the payload.
    pub rate: Rate,
    /// The payload of one packet.
    pub packet_size: Size,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    pub start: Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase.
    pub stop: Option<Duration>,
    /// Whether the packets carry the UDP checksum. Default: on.
    pub udp_checksum: UdpChecksum,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network, `v4` in a `dual` one.
    pub ip: Option<IpFamily>,
    /// Whether the sender refuses a packet that does not fit the MTU instead of cutting it (`dont-fragment: true`, D-081).
    /// Default: off.
    pub dont_fragment: bool,
}

impl UdpPoisson {
    /// A workload with the default window and the UDP checksum on.
    #[must_use]
    pub fn new(id: Name, from: Name, to: Name, rate: Rate, packet_size: Size) -> Self {
        Self {
            id,
            from,
            to,
            rate,
            packet_size,
            start: None,
            stop: None,
            udp_checksum: UdpChecksum::On,
            ip: None,
            dont_fragment: false,
        }
    }
}

/// A workload that sends at a constant rate during `on` phases and nothing during `off` phases, starting with an on
/// phase.
///
/// New keys are added to this type in later releases: use [`UdpOnOff::new`] or read a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UdpOnOff {
    /// The name of the workload. Metrics are named after it.
    pub id: Name,
    /// The node that sends.
    pub from: Name,
    /// The node that receives.
    pub to: Name,
    /// The rate of the payload while the source is on.
    pub rate: Rate,
    /// The payload of one packet.
    pub packet_size: Size,
    /// How long an on phase lasts, or its mean (see `distribution`).
    pub on: Duration,
    /// How long an off phase lasts, or its mean.
    pub off: Duration,
    /// Whether the phases are exact or drawn. Default: exact.
    pub distribution: PhaseDistribution,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    pub start: Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase.
    pub stop: Option<Duration>,
    /// Whether the packets carry the UDP checksum. Default: on.
    pub udp_checksum: UdpChecksum,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network, `v4` in a `dual` one.
    pub ip: Option<IpFamily>,
    /// Whether the sender refuses a packet that does not fit the MTU instead of cutting it (`dont-fragment: true`, D-081).
    /// Default: off.
    pub dont_fragment: bool,
}

impl UdpOnOff {
    /// A workload with exact phases, the default window and the UDP checksum on.
    #[must_use]
    pub fn new(
        id: Name,
        from: Name,
        to: Name,
        rate: Rate,
        packet_size: Size,
        on: Duration,
        off: Duration,
    ) -> Self {
        Self {
            id,
            from,
            to,
            rate,
            packet_size,
            on,
            off,
            distribution: PhaseDistribution::Fixed,
            start: None,
            stop: None,
            udp_checksum: UdpChecksum::On,
            ip: None,
            dont_fragment: false,
        }
    }
}

/// A workload that sends bursts of `burst-packets` packets, `gap` apart, with a `pause` from the last packet of a burst to
/// the first of the next. The first burst starts when the workload does.
///
/// New keys are added to this type in later releases: use [`UdpBursty::new`] or read a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UdpBursty {
    /// The name of the workload. Metrics are named after it.
    pub id: Name,
    /// The node that sends.
    pub from: Name,
    /// The node that receives.
    pub to: Name,
    /// The payload of one packet.
    pub packet_size: Size,
    /// The packets in a burst.
    pub burst_packets: u32,
    /// The time between two packets of a burst.
    pub gap: Duration,
    /// The time from the last packet of a burst to the first of the next, or its mean (see `distribution`).
    pub pause: Duration,
    /// Whether the pauses are exact or drawn. Default: exact.
    pub distribution: PhaseDistribution,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    pub start: Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase.
    pub stop: Option<Duration>,
    /// Whether the packets carry the UDP checksum. Default: on.
    pub udp_checksum: UdpChecksum,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network, `v4` in a `dual` one.
    pub ip: Option<IpFamily>,
    /// Whether the sender refuses a packet that does not fit the MTU instead of cutting it (`dont-fragment: true`, D-081).
    /// Default: off.
    pub dont_fragment: bool,
}

impl UdpBursty {
    /// A workload with exact pauses, the default window and the UDP checksum on.
    #[must_use]
    pub fn new(
        id: Name,
        from: Name,
        to: Name,
        packet_size: Size,
        burst_packets: u32,
        gap: Duration,
        pause: Duration,
    ) -> Self {
        Self {
            id,
            from,
            to,
            packet_size,
            burst_packets,
            gap,
            pause,
            distribution: PhaseDistribution::Fixed,
            start: None,
            stop: None,
            udp_checksum: UdpChecksum::On,
            ip: None,
            dont_fragment: false,
        }
    }
}

/// When the packets of a [`Generator`] leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pattern {
    /// A packet every `packet size / rate`.
    Cbr {
        /// The rate of the payload.
        rate: Rate,
    },
    /// Exponential gaps with the mean `packet size / rate`.
    Poisson {
        /// The mean rate of the payload.
        rate: Rate,
    },
    /// Phases of a constant rate and phases of silence.
    OnOff {
        /// The rate while on.
        rate: Rate,
        /// The length of an on phase, or its mean.
        on: Duration,
        /// The length of an off phase, or its mean.
        off: Duration,
        /// Whether the lengths are exact or drawn.
        distribution: PhaseDistribution,
    },
    /// Bursts of packets with a pause between them.
    Bursty {
        /// The packets in a burst.
        packets: u32,
        /// The time between two packets of a burst.
        gap: Duration,
        /// The pause after a burst, or its mean.
        pause: Duration,
        /// Whether the pauses are exact or drawn.
        distribution: PhaseDistribution,
    },
}

impl Pattern {
    /// Whether the moments of the packets are drawn from the seed of the scenario: a Poisson process always, an on/off source
    /// and bursts when their phases or pauses are exponential.
    #[must_use]
    pub fn is_random(&self) -> bool {
        match self {
            Self::Poisson { .. } => true,
            Self::OnOff { distribution, .. } | Self::Bursty { distribution, .. } => {
                *distribution == PhaseDistribution::Exponential
            }
            Self::Cbr { .. } => false,
        }
    }
}

/// A workload that sends UDP packets, seen without regard to its pattern: what every kind has in common, and the
/// pattern. Made by [`Workload::generator`](crate::Workload::generator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Generator<'a> {
    /// The kind of the workload.
    pub kind: WorkloadKind,
    /// The name of the workload.
    pub id: &'a Name,
    /// The node that sends.
    pub from: &'a Name,
    /// The node that receives.
    pub to: &'a Name,
    /// The payload of one packet.
    pub packet_size: Size,
    /// When the workload starts, from the start of the measured phase.
    pub start: Option<Duration>,
    /// When the workload stops.
    pub stop: Option<Duration>,
    /// Whether the packets carry the UDP checksum.
    pub udp_checksum: UdpChecksum,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network, `v4` in a `dual` one.
    pub ip: Option<IpFamily>,
    /// Whether the sender refuses a packet that does not fit the MTU instead of cutting it (`dont-fragment: true`, D-081).
    /// Default: off.
    pub dont_fragment: bool,
    /// When the packets leave.
    pub pattern: Pattern,
}

impl<'a> From<&'a UdpCbr> for Generator<'a> {
    fn from(workload: &'a UdpCbr) -> Self {
        Self {
            kind: WorkloadKind::UdpCbr,
            id: &workload.id,
            from: &workload.from,
            to: &workload.to,
            packet_size: workload.packet_size,
            start: workload.start,
            stop: workload.stop,
            udp_checksum: workload.udp_checksum,
            ip: workload.ip,
            dont_fragment: workload.dont_fragment,
            pattern: Pattern::Cbr {
                rate: workload.rate,
            },
        }
    }
}

impl<'a> From<&'a UdpPoisson> for Generator<'a> {
    fn from(workload: &'a UdpPoisson) -> Self {
        Self {
            kind: WorkloadKind::UdpPoisson,
            id: &workload.id,
            from: &workload.from,
            to: &workload.to,
            packet_size: workload.packet_size,
            start: workload.start,
            stop: workload.stop,
            udp_checksum: workload.udp_checksum,
            ip: workload.ip,
            dont_fragment: workload.dont_fragment,
            pattern: Pattern::Poisson {
                rate: workload.rate,
            },
        }
    }
}

impl<'a> From<&'a UdpOnOff> for Generator<'a> {
    fn from(workload: &'a UdpOnOff) -> Self {
        Self {
            kind: WorkloadKind::UdpOnoff,
            id: &workload.id,
            from: &workload.from,
            to: &workload.to,
            packet_size: workload.packet_size,
            start: workload.start,
            stop: workload.stop,
            udp_checksum: workload.udp_checksum,
            ip: workload.ip,
            dont_fragment: workload.dont_fragment,
            pattern: Pattern::OnOff {
                rate: workload.rate,
                on: workload.on,
                off: workload.off,
                distribution: workload.distribution,
            },
        }
    }
}

impl<'a> From<&'a UdpBursty> for Generator<'a> {
    fn from(workload: &'a UdpBursty) -> Self {
        Self {
            kind: WorkloadKind::UdpBursty,
            id: &workload.id,
            from: &workload.from,
            to: &workload.to,
            packet_size: workload.packet_size,
            start: workload.start,
            stop: workload.stop,
            udp_checksum: workload.udp_checksum,
            ip: workload.ip,
            dont_fragment: workload.dont_fragment,
            pattern: Pattern::Bursty {
                packets: workload.burst_packets,
                gap: workload.gap,
                pause: workload.pause,
                distribution: workload.distribution,
            },
        }
    }
}

// The files of the kinds: the keys as they are written, with `kind` (see the note at `UdpCbrFile`).

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum PoissonKind {
    UdpPoisson,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum OnOffKind {
    UdpOnoff,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum BurstyKind {
    UdpBursty,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename = "UdpPoisson")]
struct UdpPoissonFile<'a> {
    /// The name of the workload. Metrics are named after it.
    id: &'a Name,
    /// `udp-poisson`: UDP packets with exponentially distributed gaps.
    kind: PoissonKind,
    /// The node that sends.
    from: &'a Name,
    /// The node that receives.
    to: &'a Name,
    /// The mean rate of the payload.
    rate: &'a Rate,
    /// The payload of one packet.
    #[serde(rename = "packet-size")]
    packet_size: &'a Size,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    start: &'a Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: &'a Option<Duration>,
    /// `false`: the packets are sent without the UDP checksum and carry a CRC-32 (D-040). Default `true`.
    #[serde(
        default,
        rename = "udp-checksum",
        skip_serializing_if = "checksum_is_on"
    )]
    udp_checksum: UdpChecksum,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network.
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: &'a Option<IpFamily>,
    /// `true`: the sender refuses a packet that does not fit the MTU instead of cutting it. Default `false`.
    #[serde(
        default,
        rename = "dont-fragment",
        skip_serializing_if = "std::ops::Not::not"
    )]
    dont_fragment: bool,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename = "UdpOnOff")]
struct UdpOnOffFile<'a> {
    /// The name of the workload. Metrics are named after it.
    id: &'a Name,
    /// `udp-onoff`: UDP at a constant rate in on phases and silence in off phases.
    kind: OnOffKind,
    /// The node that sends.
    from: &'a Name,
    /// The node that receives.
    to: &'a Name,
    /// The rate of the payload while the source is on.
    rate: &'a Rate,
    /// The payload of one packet.
    #[serde(rename = "packet-size")]
    packet_size: &'a Size,
    /// How long an on phase lasts (its mean with `distribution: exponential`).
    on: &'a Duration,
    /// How long an off phase lasts (its mean with `distribution: exponential`).
    off: &'a Duration,
    /// `fixed` (default): the phases last exactly `on` and `off`. `exponential`: their lengths are drawn.
    #[serde(default, skip_serializing_if = "PhaseDistribution::is_fixed")]
    distribution: PhaseDistribution,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    start: &'a Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: &'a Option<Duration>,
    /// `false`: the packets are sent without the UDP checksum and carry a CRC-32 (D-040). Default `true`.
    #[serde(
        default,
        rename = "udp-checksum",
        skip_serializing_if = "checksum_is_on"
    )]
    udp_checksum: UdpChecksum,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network.
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: &'a Option<IpFamily>,
    /// `true`: the sender refuses a packet that does not fit the MTU instead of cutting it. Default `false`.
    #[serde(
        default,
        rename = "dont-fragment",
        skip_serializing_if = "std::ops::Not::not"
    )]
    dont_fragment: bool,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename = "UdpBursty")]
struct UdpBurstyFile<'a> {
    /// The name of the workload. Metrics are named after it.
    id: &'a Name,
    /// `udp-bursty`: bursts of UDP packets with a pause between them.
    kind: BurstyKind,
    /// The node that sends.
    from: &'a Name,
    /// The node that receives.
    to: &'a Name,
    /// The payload of one packet.
    #[serde(rename = "packet-size")]
    packet_size: &'a Size,
    /// The packets in a burst.
    #[serde(rename = "burst-packets")]
    burst_packets: u32,
    /// The time between two packets of a burst.
    gap: &'a Duration,
    /// The time from the last packet of a burst to the first of the next (its mean with `distribution: exponential`).
    pause: &'a Duration,
    /// `fixed` (default): every pause lasts exactly `pause`. `exponential`: the pauses are drawn.
    #[serde(default, skip_serializing_if = "PhaseDistribution::is_fixed")]
    distribution: PhaseDistribution,
    /// When the workload starts, from the start of the measured phase. Default `0s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    start: &'a Option<Duration>,
    /// When the workload stops. Default: the end of the measured phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: &'a Option<Duration>,
    /// `false`: the packets are sent without the UDP checksum and carry a CRC-32 (D-040). Default `true`.
    #[serde(
        default,
        rename = "udp-checksum",
        skip_serializing_if = "checksum_is_on"
    )]
    udp_checksum: UdpChecksum,
    /// The family of the addresses of the flow: `v4` or `v6`. Default: the only family of the network.
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: &'a Option<IpFamily>,
    /// `true`: the sender refuses a packet that does not fit the MTU instead of cutting it. Default `false`.
    #[serde(
        default,
        rename = "dont-fragment",
        skip_serializing_if = "std::ops::Not::not"
    )]
    dont_fragment: bool,
}

impl Serialize for UdpPoisson {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        UdpPoissonFile {
            id: &self.id,
            kind: PoissonKind::UdpPoisson,
            from: &self.from,
            to: &self.to,
            rate: &self.rate,
            packet_size: &self.packet_size,
            start: &self.start,
            stop: &self.stop,
            udp_checksum: self.udp_checksum,
            ip: &self.ip,
            dont_fragment: self.dont_fragment,
        }
        .serialize(serializer)
    }
}

impl Serialize for UdpOnOff {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        UdpOnOffFile {
            id: &self.id,
            kind: OnOffKind::UdpOnoff,
            from: &self.from,
            to: &self.to,
            rate: &self.rate,
            packet_size: &self.packet_size,
            on: &self.on,
            off: &self.off,
            distribution: self.distribution,
            start: &self.start,
            stop: &self.stop,
            udp_checksum: self.udp_checksum,
            ip: &self.ip,
            dont_fragment: self.dont_fragment,
        }
        .serialize(serializer)
    }
}

impl Serialize for UdpBursty {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        UdpBurstyFile {
            id: &self.id,
            kind: BurstyKind::UdpBursty,
            from: &self.from,
            to: &self.to,
            packet_size: &self.packet_size,
            burst_packets: self.burst_packets,
            gap: &self.gap,
            pause: &self.pause,
            distribution: self.distribution,
            start: &self.start,
            stop: &self.stop,
            udp_checksum: self.udp_checksum,
            ip: &self.ip,
            dont_fragment: self.dont_fragment,
        }
        .serialize(serializer)
    }
}

impl JsonSchema for UdpPoisson {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UdpPoisson".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        UdpPoissonFile::json_schema(generator)
    }
}

impl JsonSchema for UdpOnOff {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UdpOnOff".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        UdpOnOffFile::json_schema(generator)
    }
}

impl JsonSchema for UdpBursty {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UdpBursty".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        UdpBurstyFile::json_schema(generator)
    }
}
