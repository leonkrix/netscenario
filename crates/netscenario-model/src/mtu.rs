//! The MTU of a link and the frames a datagram makes on it (D-081).
//!
//! A datagram that does not fit the first link of its path is cut into fragments by the sender, and every later link carries
//! those whole; a datagram that fits the first link but not a later one would be cut (IPv4) or dropped (IPv6) by a router,
//! in sizes the run cannot account for, so a scenario that has it is refused.

use crate::scenario::{Link, Scenario};

/// The MTU of a link that does not say.
pub const DEFAULT_MTU: u32 = 1500;
/// The smallest MTU of a link (the smallest MTU an IPv4 host must accept).
pub const MIN_MTU: u32 = 68;
/// The largest MTU of a link.
pub const MAX_MTU: u32 = 65_535;
/// The smallest MTU of a link in a network with IPv6.
pub const MIN_MTU_V6: u32 = 1280;
/// The largest payload of a UDP datagram over IPv4.
pub const MAX_UDP_PAYLOAD_V4: u64 = 65_507;
/// The largest payload of a UDP datagram over IPv6 without a jumbogram.
pub const MAX_UDP_PAYLOAD_V6: u64 = 65_527 - 8;

/// The family of the addresses that a flow uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IpFamily {
    /// IPv4: a header of 20 bytes.
    V4,
    /// IPv6: a header of 40 bytes, and 8 more on a fragment.
    V6,
}

impl IpFamily {
    /// The bytes of the IP header, with the fragment header where the datagram is cut.
    #[must_use]
    fn header(self, fragmented: bool) -> u64 {
        match self {
            IpFamily::V4 => 20,
            IpFamily::V6 => 40 + if fragmented { 8 } else { 0 },
        }
    }

    /// The length of the frame's IP packet that a payload of `payload` bytes makes whole (IP header, UDP header, payload).
    #[must_use]
    pub fn packet_length(self, payload: u64) -> u64 {
        self.header(false) + 8 + payload
    }
}

/// The frames that a datagram with a payload of `payload` bytes makes on a link with this MTU: 1 if it fits, else the
/// fragments the sender cuts (a multiple of 8 bytes each but the last).
#[must_use]
pub fn frames_per_packet(payload: u64, mtu: u32, family: IpFamily) -> u64 {
    if family.packet_length(payload) <= u64::from(mtu) {
        return 1;
    }
    let carried = (u64::from(mtu).saturating_sub(family.header(true)) / 8).max(1) * 8;
    (payload + 8).div_ceil(carried)
}

/// What is wrong with a datagram of `payload` bytes on the links of a path, in the order of the path, or `None`.
///
/// `mtus` are the MTUs of the links the datagram crosses. The first decides whether it is cut; the later ones must carry
/// whole what comes out of the first.
#[must_use]
pub fn path_problem(payload: u64, mtus: &[u32], family: IpFamily) -> Option<(usize, String)> {
    let first = *mtus.first()?;
    let frame = if family.packet_length(payload) <= u64::from(first) {
        family.packet_length(payload)
    } else {
        // The largest fragment the sender makes
        let carried = (u64::from(first).saturating_sub(family.header(true)) / 8).max(1) * 8;
        family.header(true) + carried.min(payload + 8)
    };
    mtus.iter().enumerate().find_map(|(at, mtu)| {
        (frame > u64::from(*mtu)).then(|| {
            (
                at,
                format!(
                    "{frame} bytes of packet do not fit the MTU {mtu} of the link {}",
                    at + 1
                ),
            )
        })
    })
}

/// The MTUs of the links of a path, in order.
#[must_use]
pub fn mtus_of(scenario: &Scenario, links: &[crate::Name]) -> Vec<u32> {
    mtus_of_topology(&scenario.topology, links)
}

/// The MTUs of the links of a path of a topology, in order.
#[must_use]
pub fn mtus_of_topology(topology: &crate::Topology, links: &[crate::Name]) -> Vec<u32> {
    links
        .iter()
        .map(|name| {
            topology
                .links
                .iter()
                .find(|link: &&Link| link.id == *name)
                .map_or(DEFAULT_MTU, Link::mtu)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_datagram_that_fits_is_one_frame_and_one_that_does_not_is_cut_in_multiples_of_eight() {
        assert_eq!(frames_per_packet(1472, 1500, IpFamily::V4), 1);
        // 1473 bytes of payload: 1481 bytes of UDP, 1480 in a fragment: two
        assert_eq!(frames_per_packet(1473, 1500, IpFamily::V4), 2);
        // MTU 1000: 980 bytes in a fragment, rounded down to 976; 1400 + 8 = 1408 bytes of UDP make 2
        assert_eq!(frames_per_packet(1400, 1000, IpFamily::V4), 2);
        assert_eq!(frames_per_packet(3000, 1000, IpFamily::V4), 4);
        // The largest datagram over a small MTU
        assert_eq!(frames_per_packet(65_507, 576, IpFamily::V4), 119);
        // IPv6: the fragment header takes 8 bytes more, and the minimum MTU is 1280
        assert_eq!(frames_per_packet(1232, 1280, IpFamily::V6), 1);
        assert_eq!(frames_per_packet(1233, 1280, IpFamily::V6), 2);
        assert_eq!(frames_per_packet(3000, 1280, IpFamily::V6), 3);
    }

    #[test]
    fn a_later_link_that_is_smaller_than_what_the_first_lets_through_is_refused() {
        // Fits the first link and every later one
        assert_eq!(path_problem(1000, &[1500, 1500, 1500], IpFamily::V4), None);
        // Fits the first link (1500) but not the second (1000): a router would cut it
        let (at, why) = path_problem(1400, &[1500, 1000], IpFamily::V4).unwrap();
        assert_eq!(at, 1);
        assert!(why.contains("1428") && why.contains("1000"), "{why}");
        // Cut by the first link (1000): its fragments, 996 bytes, fit a later link of 1000 and of 1500, not of 600
        assert_eq!(path_problem(3000, &[1000, 1500, 1000], IpFamily::V4), None);
        assert!(path_problem(3000, &[1000, 600], IpFamily::V4).is_some());
    }
}
