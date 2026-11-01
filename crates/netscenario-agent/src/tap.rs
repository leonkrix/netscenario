//! The tap: reads the frames that reach the interface of the receiving node, and records the packets of the
//! agent among them, whether or not the kernel delivered them (D-040, D-047).
//!
//! A packet socket (`AF_PACKET`) sees a frame before the stack judges it. A frame that the node then drops
//! because a bit in it was flipped (`corrupt`) is still seen, and its sequence number is still readable unless
//! the bit hit the part of it that is read. So the sequences that were seen here and never delivered to the UDP
//! socket are the packets that arrived damaged: this is how a result can name them. The receiver of a stream
//! writes the delivered packets to its record file; the tap writes the packets it saw to a second file of the
//! same format, and the run takes the difference.
//!
//! **What is named.** A frame counts only if everything that says whose it is is intact: the type of Ethernet
//! (IPv4), the version, the header length and the protocol of IP (UDP), the destination address of IP and the
//! destination port of UDP (those of the receiver of this stream), and the magic, the version, the flags and the
//! sequence of the agent. A flipped bit in any of these 18 bytes leaves the frame unnamed. It is a stricter
//! rule than the sequence alone would need: a frame with a damaged destination port cannot be given to a
//! stream with certainty, and a wrong name is worse than none. A bit in the sequence number itself that gives
//! another number in the range of the stream names that other packet; the run judges the range, not the tap.
//!
//! The socket is not bound to an interface: a node has the interface of its link and the loopback, and the
//! destination address and port select the stream. It needs the capability `CAP_NET_RAW` in the node, like the
//! sender of a stream without the checksum, and it does not use `unsafe`.

use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rustix::net::sockopt::Timeout;
use rustix::net::{AddressFamily, RecvFlags, SocketType, eth};

use crate::clock::monotonic_ns;
use crate::records::{Record, Writer};
use crate::wire::{Header, UdpChecksum};

/// The length of the header of Ethernet.
const ETHERNET: usize = 14;
/// The length of the header of IPv4 without options.
const IPV4: usize = 20;
/// The length of the header of UDP.
const UDP: usize = 8;
/// Where the payload of the packet begins in a frame.
const PAYLOAD_AT: usize = ETHERNET + IPV4 + UDP;

/// How long a wait for a frame lasts before the tap looks at the stop flag again.
const POLL: Duration = Duration::from_millis(20);

/// How long the tap keeps reading queued frames after the stop, at most, in nanoseconds (as the receiver does).
const DRAIN_NS: u64 = 100_000_000;

/// The size of the receive buffer that the tap asks for. The kernel may give less (`rmem_max`).
const RECEIVE_BUFFER: usize = 4 * 1024 * 1024;

/// The header of the agent in `frame`, if `frame` is an IPv4 UDP packet to `to` whose headers and agent header
/// are intact. The CRC-32 of a stream without the checksum is not checked: a frame that fails it is the kind
/// that this is looking for.
#[must_use]
pub fn packet_in_frame(frame: &[u8], to: (Ipv4Addr, u16), checksum: UdpChecksum) -> Option<Header> {
    if frame.len() < PAYLOAD_AT + checksum.header_len() {
        return None;
    }
    // Ethernet: the type is IPv4. The addresses are not looked at: a frame for another address is dropped by
    // the interface, and it is still a frame of this stream.
    if frame[12..14] != [0x08, 0x00] {
        return None;
    }
    let ip = &frame[ETHERNET..ETHERNET + IPV4];
    // IPv4 without options, UDP, to the address of the receiver
    if ip[0] != 0x45 || ip[9] != 17 || ip[16..20] != to.0.octets() {
        return None;
    }
    let udp = &frame[ETHERNET + IPV4..PAYLOAD_AT];
    if udp[2..4] != to.1.to_be_bytes() {
        return None;
    }
    Header::peek(&frame[PAYLOAD_AT..], checksum).ok()
}

/// What the tap did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapSummary {
    /// The frames that were read, of any kind.
    pub frames: u64,
    /// The packets of the stream among them, which are in the file.
    pub packets: u64,
}

/// A packet socket that is open and not yet reading.
#[derive(Debug)]
pub struct Tap {
    socket: OwnedFd,
    to: (Ipv4Addr, u16),
    checksum: UdpChecksum,
}

impl Tap {
    /// Opens the packet socket for the stream that is received at `to`.
    ///
    /// # Errors
    ///
    /// The error of the system: the capability `CAP_NET_RAW` is missing (`Operation not permitted`), or `to` is
    /// not an IPv4 address with a port.
    pub fn open(to: SocketAddr, checksum: UdpChecksum) -> io::Result<Self> {
        let SocketAddr::V4(to) = to else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the tap reads IPv4 only",
            ));
        };
        let socket = rustix::net::socket(AddressFamily::PACKET, SocketType::RAW, Some(eth::ALL))?;
        rustix::net::sockopt::set_socket_timeout(&socket, Timeout::Recv, Some(POLL))?;
        // Not every system allows the size: a smaller buffer still works
        let _ = rustix::net::sockopt::set_socket_recv_buffer_size(&socket, RECEIVE_BUFFER);
        Ok(Self {
            socket,
            to: (*to.ip(), to.port()),
            checksum,
        })
    }

    /// Records the packets of the stream into `out` (a record file of the same format as the receiver's, with
    /// the flow `flow`) until `stop` is set, then returns what was seen. Frames that are already queued when
    /// the stop comes are still read, for at most 100 ms.
    ///
    /// # Errors
    ///
    /// The error of the socket or of `out`.
    pub fn run(self, flow: u32, stop: &AtomicBool, out: impl Write) -> io::Result<TapSummary> {
        let mut writer = Writer::new(io::BufWriter::with_capacity(1 << 20, out), flow)?;
        let mut summary = TapSummary {
            frames: 0,
            packets: 0,
        };
        let mut buffer = [0_u8; 2048];
        let mut drain_end: Option<u64> = None;
        loop {
            if drain_end.is_none() && stop.load(Ordering::SeqCst) {
                drain_end = Some(monotonic_ns().saturating_add(DRAIN_NS));
            }
            if drain_end.is_some_and(|end| monotonic_ns() >= end) {
                break;
            }
            match rustix::net::recv(&self.socket, &mut buffer, RecvFlags::empty()) {
                Ok((length, _)) => {
                    let seen = monotonic_ns();
                    summary.frames += 1;
                    if let Some(header) = packet_in_frame(&buffer[..length], self.to, self.checksum)
                    {
                        writer.write(&Record {
                            sequence: u64::from(header.sequence),
                            send_ns: header.send_ns,
                            receive_ns: seen,
                        })?;
                        summary.packets += 1;
                    }
                }
                Err(error)
                    if error == rustix::io::Errno::AGAIN || error == rustix::io::Errno::INTR =>
                {
                    // The queue is empty: after the stop there is nothing left to read
                    if drain_end.is_some() {
                        break;
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        writer.finish()?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TO: (Ipv4Addr, u16) = (Ipv4Addr::new(10, 100, 0, 2), 5000);

    /// A frame as `netem` and the veth pair carry it: Ethernet, IPv4 (20 bytes), UDP, and the agent's packet.
    fn frame(sequence: u32, checksum: UdpChecksum, payload: usize) -> Vec<u8> {
        let mut frame = vec![0_u8; PAYLOAD_AT + payload];
        // Ethernet: the destination and source addresses, then the type
        frame[0..6].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        frame[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame[12..14].copy_from_slice(&[0x08, 0x00]);
        // IPv4: version 4 and 5 words, UDP, from 10.100.0.1 to the receiver
        frame[14] = 0x45;
        frame[23] = 17;
        frame[26..30].copy_from_slice(&[10, 100, 0, 1]);
        frame[30..34].copy_from_slice(&TO.0.octets());
        // UDP: a source port, the destination port
        frame[34..36].copy_from_slice(&40_000_u16.to_be_bytes());
        frame[36..38].copy_from_slice(&TO.1.to_be_bytes());
        let header = Header {
            sequence,
            send_ns: 123,
        };
        match checksum {
            UdpChecksum::On => header.write(&mut frame[PAYLOAD_AT..]),
            UdpChecksum::Off => header.write_v2(&mut frame[PAYLOAD_AT..]),
        }
        frame
    }

    #[test]
    fn the_sequence_of_an_intact_frame_is_read() {
        for checksum in [UdpChecksum::On, UdpChecksum::Off] {
            let seen = packet_in_frame(&frame(77, checksum, 64), TO, checksum).unwrap();
            assert_eq!((seen.sequence, seen.send_ns), (77, 123));
        }
    }

    /// The frames that this exists for: a bit is flipped, and the frame is still named as long as the bit is
    /// not in the 18 bytes that say whose it is. Every byte of a frame of 106 bytes is flipped in turn; the
    /// ones that must be refused are exactly those.
    #[test]
    fn a_flipped_bit_leaves_a_frame_unnamed_only_in_the_bytes_that_say_whose_it_is() {
        let intact = frame(5, UdpChecksum::On, 64);
        // Type of Ethernet (12, 13), version and header length of IP (14), protocol (23), destination (30 to
        // 33), destination port (36, 37), magic (42, 43), version (44), flags (45), sequence (46 to 49)
        let identity: Vec<usize> = [12, 13, 14, 23]
            .into_iter()
            .chain(30..34)
            .chain(36..38)
            .chain(42..46)
            .chain(46..50)
            .collect();
        assert_eq!(identity.len(), 18);
        for position in 0..intact.len() {
            let mut damaged = intact.clone();
            damaged[position] ^= 0x04;
            let seen = packet_in_frame(&damaged, TO, UdpChecksum::On);
            if identity.contains(&position) {
                // A bit in the sequence gives another number, or the sequence is not read at all
                if (46..50).contains(&position) {
                    assert_ne!(
                        seen.map(|header| header.sequence),
                        Some(5),
                        "byte {position}"
                    );
                } else {
                    assert_eq!(seen, None, "byte {position} says whose the frame is");
                }
            } else {
                assert_eq!(
                    seen.map(|header| header.sequence),
                    Some(5),
                    "byte {position} is not read"
                );
            }
        }
    }

    /// Canary: a frame of another stream, another kind or another size is not taken for this one.
    #[test]
    fn a_frame_of_another_stream_or_kind_is_refused() {
        let good = frame(1, UdpChecksum::On, 64);
        // Another port, another address
        assert_eq!(packet_in_frame(&good, (TO.0, 5001), UdpChecksum::On), None);
        assert_eq!(
            packet_in_frame(&good, (Ipv4Addr::new(10, 100, 0, 9), TO.1), UdpChecksum::On),
            None
        );
        // Too short for the headers and the header of the agent
        assert_eq!(packet_in_frame(&good[..57], TO, UdpChecksum::On), None);
        assert_eq!(packet_in_frame(&[], TO, UdpChecksum::On), None);
        // A packet of the other version is not one of this stream
        assert_eq!(packet_in_frame(&good, TO, UdpChecksum::Off), None);
        let checked = frame(1, UdpChecksum::Off, 64);
        assert_eq!(packet_in_frame(&checked, TO, UdpChecksum::On), None);
        // IP options (a header of 6 words) shift the payload: not read
        let mut options = good.clone();
        options[14] = 0x46;
        assert_eq!(packet_in_frame(&options, TO, UdpChecksum::On), None);
        // TCP
        let mut tcp = good;
        tcp[23] = 6;
        assert_eq!(packet_in_frame(&tcp, TO, UdpChecksum::On), None);
    }

    /// A stream without the checksum: the frame with a wrong CRC-32 is named, because that is the damaged kind.
    #[test]
    fn the_crc_of_a_stream_without_the_checksum_is_not_a_condition() {
        let mut damaged = frame(9, UdpChecksum::Off, 200);
        damaged[PAYLOAD_AT + 100] ^= 0x01;
        let seen = packet_in_frame(&damaged, TO, UdpChecksum::Off).unwrap();
        assert_eq!(seen.sequence, 9);
    }
}
