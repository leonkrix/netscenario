//! The IP family of the addresses of a network and of a flow (D-081).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The family of the addresses that a flow uses.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum IpFamily {
    /// IPv4: a header of 20 bytes.
    V4,
    /// IPv6: a header of 40 bytes, and 8 more on a fragment.
    V6,
}

impl IpFamily {
    /// The word of the file: `v4` or `v6`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            IpFamily::V4 => "v4",
            IpFamily::V6 => "v6",
        }
    }
}

/// Which addresses the network has. IPv6 in a namespace is not quiet (a few packets of its own in the first second), so a
/// network has it only where the scenario asks (D-081).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IpMode {
    /// IPv4 only. The default.
    #[default]
    V4,
    /// IPv6 only: the links and the nodes have no IPv4 address.
    V6,
    /// Both: every link and every node has an address of each family.
    Dual,
}

impl IpMode {
    /// Whether the network has addresses of this family.
    #[must_use]
    pub fn has(self, family: IpFamily) -> bool {
        match (self, family) {
            (IpMode::Dual, _) | (IpMode::V4, IpFamily::V4) | (IpMode::V6, IpFamily::V6) => true,
            (IpMode::V4, IpFamily::V6) | (IpMode::V6, IpFamily::V4) => false,
        }
    }

    /// The family a flow uses that does not say: the only one of the network, IPv4 in a dual network.
    #[must_use]
    pub fn default_family(self) -> IpFamily {
        match self {
            IpMode::V6 => IpFamily::V6,
            IpMode::V4 | IpMode::Dual => IpFamily::V4,
        }
    }

    // serde's `skip_serializing_if` passes a reference
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn is_default(&self) -> bool {
        *self == IpMode::V4
    }
}
