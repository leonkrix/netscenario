//! The packet of the agent (the wire format, version 1).
//!
//! The payload of every UDP packet starts with a header of 16 bytes, in network byte order; the rest of the
//! payload is zeros, up to the packet size of the workload.
//!
//! | Bytes | Field | Meaning |
//! | --- | --- | --- |
//! | 0 to 1 | magic | `0x4E53` (`NS`). A datagram without it did not come from an agent and is not counted as a packet. |
//! | 2 | version | `1`. Another version is not read. |
//! | 3 | flags | `0`. Reserved; a packet with a flag set is not read. |
//! | 4 to 7 | sequence | The number of the packet in its stream, from 0, one more for each packet. |
//! | 8 to 15 | send time | `CLOCK_MONOTONIC` in nanoseconds, taken right before the packet was handed to the kernel. |

/// The length of the header, and so the smallest payload of a packet. The scenario model refuses a smaller
/// `packet-size` (`MIN_PACKET_SIZE`).
pub const HEADER_LEN: usize = 16;

/// The first two bytes of every packet: `NS`.
pub const MAGIC: u16 = 0x4E53;

/// The version of this format.
pub const VERSION: u8 = 1;

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
    /// A flag is set, and version 1 has none.
    UnknownFlags(u8),
}

impl Header {
    /// Writes the header into the start of `payload`.
    ///
    /// # Panics
    ///
    /// If `payload` is shorter than [`HEADER_LEN`]; the sender checks the packet size before it starts.
    pub fn write(&self, payload: &mut [u8]) {
        payload[0..2].copy_from_slice(&MAGIC.to_be_bytes());
        payload[2] = VERSION;
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
        let Some(head) = datagram.get(..HEADER_LEN) else {
            return Err(Refusal::TooShort);
        };
        if head[0..2] != MAGIC.to_be_bytes() {
            return Err(Refusal::WrongMagic);
        }
        if head[2] != VERSION {
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
}
