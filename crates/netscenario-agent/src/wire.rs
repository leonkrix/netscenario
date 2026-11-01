//! The packet of the agent (the wire format, versions 1 and 2).
//!
//! The payload of every UDP packet starts with a header of 16 bytes (20 in version 2), in network byte order;
//! the rest of the payload is zeros, up to the packet size of the workload.
//!
//! | Bytes | Field | Meaning |
//! | --- | --- | --- |
//! | 0 to 1 | magic | `0x4E53` (`NS`). A datagram without it did not come from an agent and is not counted as a packet. |
//! | 2 | version | `1`, or `2` for a stream without the UDP checksum. Another version is not read. |
//! | 3 | flags | `0`. Reserved; a packet with a flag set is not read. |
//! | 4 to 7 | sequence | The number of the packet in its stream, from 0, one more for each packet. |
//! | 8 to 15 | send time | `CLOCK_MONOTONIC` in nanoseconds, taken right before the packet was handed to the kernel. |
//! | 16 to 19 | CRC-32 | Version 2 only: the CRC-32 ([`crate::crc`]) of every other byte of the payload. |
//!
//! **Version 2 is for `udp-checksum: false` (D-040).** The sender then leaves the checksum of UDP at `0`, which
//! IPv4 allows, so a bit that the network flips in the payload is not caught by the receiving kernel, and the
//! CRC-32 lets the receiver tell a damaged packet from a good one. The CRC covers the whole payload except its
//! own four bytes, the zeros at the end included: a flipped bit anywhere in the payload is found (a CRC-32
//! finds every single flipped bit, and every burst of up to 32 bits).

use crate::crc::Crc32;

/// The length of the header, and so the smallest payload of a packet. The scenario model refuses a smaller
/// `packet-size` (`MIN_PACKET_SIZE`).
pub const HEADER_LEN: usize = 16;

/// The length of the header of version 2, which has the CRC-32 after the fields of version 1. The scenario
/// model refuses a smaller `packet-size` with `udp-checksum: false` (`MIN_PACKET_SIZE_WITHOUT_CHECKSUM`).
pub const HEADER_LEN_V2: usize = 20;

/// The first two bytes of every packet: `NS`.
pub const MAGIC: u16 = 0x4E53;

/// The version of this format: the packet with the UDP checksum.
pub const VERSION: u8 = 1;

/// The version of the packet without the UDP checksum, which carries a CRC-32.
pub const VERSION_V2: u8 = 2;

/// Where the CRC-32 is in a packet of version 2.
const CRC_AT: std::ops::Range<usize> = 16..20;

/// Whether the packets of a stream carry the checksum of UDP.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UdpChecksum {
    /// The checksum of UDP is on, and the packet is version 1.
    #[default]
    On,
    /// The checksum is `0` and the packet is version 2 with a CRC-32.
    Off,
}

impl UdpChecksum {
    /// The length of the header of a packet in this form, and so the smallest payload.
    #[must_use]
    pub const fn header_len(self) -> usize {
        match self {
            Self::On => HEADER_LEN,
            Self::Off => HEADER_LEN_V2,
        }
    }
}

impl std::str::FromStr for UdpChecksum {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            _ => Err(()),
        }
    }
}

/// What the header of a packet says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The number of the packet in its stream.
    pub sequence: u32,
    /// When the packet was sent, in nanoseconds on `CLOCK_MONOTONIC`.
    pub send_ns: u64,
}

/// Why a datagram is not a packet of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Shorter than the header.
    TooShort,
    /// The first two bytes are not `NS`.
    WrongMagic,
    /// A version this program does not read.
    UnknownVersion(u8),
    /// A flag is set, and the versions have none.
    UnknownFlags(u8),
    /// Version 2: the CRC-32 does not match the payload, so a bit of it is not as it was sent.
    BadChecksum,
}

impl Header {
    /// Writes the header into the start of `payload`.
    ///
    /// # Panics
    ///
    /// If `payload` is shorter than [`HEADER_LEN`]; the sender checks the packet size before it starts.
    pub fn write(&self, payload: &mut [u8]) {
        self.write_fields(payload, VERSION);
    }

    /// Writes the header of version 2 into the start of `payload`, and the CRC-32 of the whole payload after
    /// it. Everything after the header must be in place already: the CRC covers it.
    ///
    /// # Panics
    ///
    /// If `payload` is shorter than [`HEADER_LEN_V2`]; the sender checks the packet size before it starts.
    pub fn write_v2(&self, payload: &mut [u8]) {
        self.write_fields(payload, VERSION_V2);
        let crc = crc_of(payload);
        payload[CRC_AT].copy_from_slice(&crc.to_be_bytes());
    }

    fn write_fields(&self, payload: &mut [u8], version: u8) {
        payload[0..2].copy_from_slice(&MAGIC.to_be_bytes());
        payload[2] = version;
        payload[3] = 0;
        payload[4..8].copy_from_slice(&self.sequence.to_be_bytes());
        payload[8..16].copy_from_slice(&self.send_ns.to_be_bytes());
    }

    /// Reads the header at the start of `datagram`.
    ///
    /// # Errors
    ///
    /// A [`Refusal`] if the datagram is not a packet of version 1.
    pub fn read(datagram: &[u8]) -> Result<Self, Refusal> {
        Self::read_fields(datagram, VERSION, HEADER_LEN)
    }

    /// Reads the header of version 2 at the start of `datagram` and checks the CRC-32 over the whole of it.
    ///
    /// # Errors
    ///
    /// A [`Refusal`] if the datagram is not a packet of version 2 or its CRC-32 does not match, which is
    /// what a bit flipped on the way looks like.
    pub fn read_v2(datagram: &[u8]) -> Result<Self, Refusal> {
        let header = Self::read_fields(datagram, VERSION_V2, HEADER_LEN_V2)?;
        let mut sent = [0_u8; 4];
        sent.copy_from_slice(&datagram[CRC_AT]);
        if u32::from_be_bytes(sent) != crc_of(datagram) {
            return Err(Refusal::BadChecksum);
        }
        Ok(header)
    }

    fn read_fields(datagram: &[u8], version: u8, header_len: usize) -> Result<Self, Refusal> {
        let Some(head) = datagram.get(..header_len) else {
            return Err(Refusal::TooShort);
        };
        if head[0..2] != MAGIC.to_be_bytes() {
            return Err(Refusal::WrongMagic);
        }
        if head[2] != version {
            return Err(Refusal::UnknownVersion(head[2]));
        }
        if head[3] != 0 {
            return Err(Refusal::UnknownFlags(head[3]));
        }
        let mut sequence = [0_u8; 4];
        sequence.copy_from_slice(&head[4..8]);
        let mut send_ns = [0_u8; 8];
        send_ns.copy_from_slice(&head[8..16]);
        Ok(Self {
            sequence: u32::from_be_bytes(sequence),
            send_ns: u64::from_be_bytes(send_ns),
        })
    }
}

/// The CRC-32 of a packet of version 2 without the four bytes of the CRC itself.
fn crc_of(packet: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(&packet[..CRC_AT.start]);
    crc.update(&packet[CRC_AT.end..]);
    crc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes are part of the format. This test pins them: a change that breaks it breaks every reader.
    #[test]
    fn the_header_has_these_exact_bytes() {
        let mut payload = [0xFF_u8; 20];
        Header {
            sequence: 0x0102_0304,
            send_ns: 0x1112_1314_1516_1718,
        }
        .write(&mut payload);
        assert_eq!(
            payload,
            [
                0x4E, 0x53, 1, 0, 0x01, 0x02, 0x03, 0x04, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
                0x18, 0xFF, 0xFF, 0xFF, 0xFF
            ],
            "the bytes after the header are not touched"
        );
    }

    #[test]
    fn a_header_survives_the_round_trip() {
        for (sequence, send_ns) in [(0, 0), (1, 1), (u32::MAX, u64::MAX), (123_456, 987_654_321)] {
            let header = Header { sequence, send_ns };
            let mut payload = vec![0; HEADER_LEN];
            header.write(&mut payload);
            assert_eq!(Header::read(&payload), Ok(header));
        }
    }

    /// Canary: the reader can say no.
    #[test]
    fn what_is_not_a_packet_is_refused_and_says_why() {
        let mut good = [0_u8; 16];
        Header {
            sequence: 5,
            send_ns: 6,
        }
        .write(&mut good);
        assert_eq!(Header::read(&good[..15]), Err(Refusal::TooShort));
        assert_eq!(Header::read(&[]), Err(Refusal::TooShort));
        let mut changed = good;
        changed[0] = b'X';
        assert_eq!(Header::read(&changed), Err(Refusal::WrongMagic));
        let mut changed = good;
        changed[2] = 2;
        assert_eq!(Header::read(&changed), Err(Refusal::UnknownVersion(2)));
        let mut changed = good;
        changed[3] = 0x80;
        assert_eq!(Header::read(&changed), Err(Refusal::UnknownFlags(0x80)));
    }

    #[test]
    fn the_payload_after_the_header_is_ignored() {
        let mut payload = vec![7_u8; 1200];
        let header = Header {
            sequence: 9,
            send_ns: 10,
        };
        header.write(&mut payload);
        assert_eq!(Header::read(&payload), Ok(header));
    }

    /// The bytes of version 2 are part of the format. The CRC was worked out by another program (`zlib.crc32`
    /// of the bytes 0 to 15 and 20 to 23), not by this code.
    #[test]
    fn the_header_of_version_2_has_these_exact_bytes() {
        let mut payload = [0xFF_u8; 24];
        payload[16..20].fill(0);
        Header {
            sequence: 0x0102_0304,
            send_ns: 0x1112_1314_1516_1718,
        }
        .write_v2(&mut payload);
        assert_eq!(
            payload,
            [
                0x4E, 0x53, 2, 0, 0x01, 0x02, 0x03, 0x04, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
                0x18, 0x9D, 0x14, 0x88, 0x8E, 0xFF, 0xFF, 0xFF, 0xFF
            ]
        );
    }

    #[test]
    fn a_packet_of_version_2_survives_the_round_trip() {
        for size in [HEADER_LEN_V2, 21, 64, 1200, 1472] {
            let header = Header {
                sequence: 77,
                send_ns: 123_456_789,
            };
            let mut payload = vec![0_u8; size];
            header.write_v2(&mut payload);
            assert_eq!(Header::read_v2(&payload), Ok(header), "{size}");
        }
    }

    /// Canary: a bit flipped anywhere in the payload is found, whatever its position and the size. This is
    /// what the receiver of a stream without the UDP checksum relies on.
    #[test]
    fn every_flipped_bit_of_a_packet_of_version_2_is_found() {
        for size in [HEADER_LEN_V2, 64, 200] {
            let mut payload = vec![0_u8; size];
            Header {
                sequence: 5,
                send_ns: 6,
            }
            .write_v2(&mut payload);
            for position in 0..size {
                for bit in 0..8 {
                    payload[position] ^= 1 << bit;
                    assert!(
                        Header::read_v2(&payload).is_err(),
                        "byte {position} bit {bit} of {size} was not found"
                    );
                    payload[position] ^= 1 << bit;
                }
            }
            assert!(Header::read_v2(&payload).is_ok(), "the packet is restored");
        }
    }

    #[test]
    fn the_two_versions_are_not_mistaken_for_each_other() {
        let header = Header {
            sequence: 1,
            send_ns: 2,
        };
        let mut v1 = vec![0_u8; 64];
        header.write(&mut v1);
        let mut v2 = vec![0_u8; 64];
        header.write_v2(&mut v2);
        assert_eq!(Header::read_v2(&v1), Err(Refusal::UnknownVersion(1)));
        assert_eq!(Header::read(&v2), Err(Refusal::UnknownVersion(2)));
        assert_eq!(Header::read_v2(&v2[..19]), Err(Refusal::TooShort));
        // The damage is in the CRC itself
        let mut changed = v2.clone();
        changed[17] ^= 1;
        assert_eq!(Header::read_v2(&changed), Err(Refusal::BadChecksum));
        // The damage is in the zeros after the header
        let mut changed = v2;
        changed[63] ^= 0x80;
        assert_eq!(Header::read_v2(&changed), Err(Refusal::BadChecksum));
    }
}
