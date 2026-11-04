//! A capture (`pcap` or `pcapng`) as the trace of a workload (WRK-03, D-085): the packets of one flow, at the moments they were
//! captured, with the size of their transport payload.
//!
//! Only what a trace needs is read: the timestamp of a packet and its headers down to the transport layer. Captures of Ethernet
//! (with VLAN tags), raw IP, Linux cooked captures (SLL and SLL2) and the BSD loopback are understood. A packet that is not UDP or
//! TCP over IPv4 or IPv6, or is a fragment after the first, is not part of any flow.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::trace::{FlowProtocol, FlowSelection, MAX_TRACE_PACKETS, ReadTrace, TraceData};

/// How many flows an error lists.
const LISTED_FLOWS: usize = 8;

/// A flow of a capture: the packets that go from one endpoint to the other with one protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Flow {
    src: IpAddr,
    dst: IpAddr,
    src_port: u16,
    dst_port: u16,
    protocol: FlowProtocol,
}

impl Flow {
    fn describe(&self) -> String {
        let protocol = match self.protocol {
            FlowProtocol::Udp => "udp",
            FlowProtocol::Tcp => "tcp",
        };
        format!(
            "{}:{} to {}:{} ({protocol})",
            self.src, self.src_port, self.dst, self.dst_port
        )
    }

    fn matches(&self, select: &FlowSelection) -> bool {
        select.src.is_none_or(|src| src == self.src)
            && select.dst.is_none_or(|dst| dst == self.dst)
            && select.src_port.is_none_or(|port| port == self.src_port)
            && select.dst_port.is_none_or(|port| port == self.dst_port)
            && select
                .protocol
                .is_none_or(|protocol| protocol == self.protocol)
    }
}

/// One packet of the capture that belongs to a flow.
struct Seen {
    flow: Flow,
    timestamp_ns: u128,
    payload: u64,
}

/// A packet as the file holds it: its time, the link type of its interface, and the bytes that were captured.
struct Frame<'a> {
    timestamp_ns: u128,
    link_type: u32,
    data: &'a [u8],
}

fn u16_at(bytes: &[u8], at: usize, big: bool) -> Option<u16> {
    let pair: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(if big {
        u16::from_be_bytes(pair)
    } else {
        u16::from_le_bytes(pair)
    })
}

fn u32_at(bytes: &[u8], at: usize, big: bool) -> Option<u32> {
    let quad: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(if big {
        u32::from_be_bytes(quad)
    } else {
        u32::from_le_bytes(quad)
    })
}

/// The packets of a classic `pcap` file.
fn classic_frames(bytes: &[u8]) -> Result<Vec<Frame<'_>>, String> {
    let magic = u32_at(bytes, 0, true).ok_or("the file is too short to be a capture")?;
    let (big, nanoseconds) = match magic {
        0xa1b2_c3d4 => (true, false),
        0xd4c3_b2a1 => (false, false),
        0xa1b2_3c4d => (true, true),
        0x4d3c_b2a1 => (false, true),
        _ => return Err("the file is not a capture".to_owned()),
    };
    let link_type =
        u32_at(bytes, 20, big).ok_or("the header of the capture is cut short")? & 0x0fff_ffff;
    let mut frames = Vec::new();
    let mut at = 24;
    while at < bytes.len() {
        let seconds =
            u32_at(bytes, at, big).ok_or("the capture ends in the middle of a packet header")?;
        let fraction = u32_at(bytes, at + 4, big)
            .ok_or("the capture ends in the middle of a packet header")?;
        let captured = u32_at(bytes, at + 8, big)
            .ok_or("the capture ends in the middle of a packet header")?;
        let start = at + 16;
        let end = start
            .checked_add(captured as usize)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| format!("the capture ends in the middle of the packet at byte {at}"))?;
        let fraction_ns = if nanoseconds {
            u128::from(fraction)
        } else {
            u128::from(fraction) * 1000
        };
        frames.push(Frame {
            timestamp_ns: u128::from(seconds) * 1_000_000_000 + fraction_ns,
            link_type,
            data: &bytes[start..end],
        });
        at = end;
    }
    Ok(frames)
}

/// The ticks of a second of an interface of a `pcapng` file, from its option `if_tsresol`.
fn ticks_per_second(options: &[u8], big: bool) -> u128 {
    let mut at = 0;
    while let (Some(code), Some(length)) = (u16_at(options, at, big), u16_at(options, at + 2, big))
    {
        if code == 0 {
            break;
        }
        let length = usize::from(length);
        if code == 9 && length >= 1 {
            let value = options.get(at + 4).copied().unwrap_or(6);
            let exponent = u32::from(value & 0x7f);
            return if value & 0x80 == 0 {
                10_u128.checked_pow(exponent).unwrap_or(1_000_000)
            } else {
                1_u128.checked_shl(exponent).unwrap_or(1_000_000)
            };
        }
        at += 4 + length.div_ceil(4) * 4;
    }
    1_000_000
}

/// The packets of a `pcapng` file.
fn ng_frames(bytes: &[u8]) -> Result<Vec<Frame<'_>>, String> {
    let mut frames = Vec::new();
    // The interfaces of the section: the link type and the ticks of a second
    let mut interfaces: Vec<(u32, u128)> = Vec::new();
    let mut big = false;
    let mut at = 0;
    while at < bytes.len() {
        // The byte order of a section is in its header; the type of a block is read in that order, except the header's own
        let is_header = bytes.get(at..at + 4) == Some(&[0x0a, 0x0d, 0x0d, 0x0a]);
        if is_header {
            big = match u32_at(bytes, at + 8, true) {
                Some(0x1a2b_3c4d) => true,
                Some(0x4d3c_2b1a) => false,
                _ => return Err("the section header of the capture has no byte order".to_owned()),
            };
            interfaces.clear();
        }
        let kind = if is_header {
            0x0a0d_0d0a
        } else {
            u32_at(bytes, at, big).ok_or("the capture ends in the middle of a block")?
        };
        let length =
            u32_at(bytes, at + 4, big).ok_or("the capture ends in the middle of a block")? as usize;
        if length < 12 || !length.is_multiple_of(4) || at + length > bytes.len() {
            return Err(format!(
                "the block at byte {at} has a length of {length}, which is not a block"
            ));
        }
        let body = &bytes[at + 8..at + length - 4];
        match kind {
            // Interface description
            1 => {
                let link_type =
                    u32::from(u16_at(body, 0, big).ok_or("an interface block is cut short")?);
                interfaces.push((
                    link_type,
                    ticks_per_second(body.get(8..).unwrap_or(&[]), big),
                ));
            }
            // Enhanced packet
            6 => {
                let interface = u32_at(body, 0, big).ok_or("a packet block is cut short")? as usize;
                let &(link_type, ticks) = interfaces.get(interface).ok_or_else(|| {
                    format!(
                        "a packet is on interface {interface}, which the capture does not describe"
                    )
                })?;
                let high = u32_at(body, 4, big).ok_or("a packet block is cut short")?;
                let low = u32_at(body, 8, big).ok_or("a packet block is cut short")?;
                let captured = u32_at(body, 12, big).ok_or("a packet block is cut short")? as usize;
                let data = body
                    .get(20..20 + captured)
                    .ok_or("a packet block is shorter than the packet it holds")?;
                let counted = (u128::from(high) << 32) | u128::from(low);
                frames.push(Frame {
                    timestamp_ns: counted * 1_000_000_000 / ticks.max(1),
                    link_type,
                    data,
                });
            }
            // Simple packet: no timestamp, so no trace
            3 => {
                return Err(
                    "the capture has packets without timestamps (simple packet blocks), which a trace cannot use".to_owned(),
                );
            }
            _ => {}
        }
        at += length;
    }
    Ok(frames)
}

/// The IP packet in a frame of a link type, or `None` for something that is not IP.
fn ip_packet(link_type: u32, data: &[u8]) -> Result<Option<&[u8]>, String> {
    let ethertype_ip = |ethertype: u16| matches!(ethertype, 0x0800 | 0x86dd);
    Ok(match link_type {
        // Ethernet, with any number of VLAN tags
        1 => {
            let mut at = 12;
            let mut ethertype = u16_at(data, at, true);
            while matches!(ethertype, Some(0x8100 | 0x88a8)) {
                at += 4;
                ethertype = u16_at(data, at, true);
            }
            ethertype
                .filter(|kind| ethertype_ip(*kind))
                .and_then(|_| data.get(at + 2..))
        }
        // The loopback of BSD: four bytes of family
        0 => data.get(4..),
        // Raw IP
        101 | 12 => Some(data),
        // Linux cooked captures
        113 => u16_at(data, 14, true)
            .filter(|kind| ethertype_ip(*kind))
            .and_then(|_| data.get(16..)),
        276 => u16_at(data, 0, true)
            .filter(|kind| ethertype_ip(*kind))
            .and_then(|_| data.get(20..)),
        other => {
            return Err(format!(
                "the capture has packets of link type {other}, which a trace cannot read (Ethernet, raw IP, Linux cooked and loopback captures are understood)"
            ));
        }
    })
}

/// The flow and the transport payload of an IP packet, or `None` for a packet that belongs to no flow.
fn decode(ip: &[u8]) -> Option<(Flow, u64)> {
    match ip.first()? >> 4 {
        4 => {
            let header = usize::from(ip[0] & 0x0f) * 4;
            let total = usize::from(u16_at(ip, 2, true)?);
            // After the first fragment there is no transport header to read
            if u16_at(ip, 6, true)? & 0x1fff != 0 || header < 20 {
                return None;
            }
            let src = IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(ip.get(12..16)?).ok()?));
            let dst = IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(ip.get(16..20)?).ok()?));
            let transport = ip.get(header..)?;
            let available = total.saturating_sub(header);
            transport_of(ip[9], transport, available, src, dst)
        }
        6 => {
            let src = IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(ip.get(8..24)?).ok()?));
            let dst = IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(ip.get(24..40)?).ok()?));
            let mut next = *ip.get(6)?;
            let mut at = 40;
            let mut available = usize::from(u16_at(ip, 4, true)?);
            // The extension headers that can come before the transport header
            loop {
                match next {
                    0 | 43 | 60 => {
                        let length = (usize::from(*ip.get(at + 1)?) + 1) * 8;
                        next = *ip.get(at)?;
                        at += length;
                        available = available.saturating_sub(length);
                    }
                    44 => {
                        if u16_at(ip, at + 2, true)? >> 3 != 0 {
                            return None;
                        }
                        next = *ip.get(at)?;
                        at += 8;
                        available = available.saturating_sub(8);
                    }
                    _ => break,
                }
            }
            transport_of(next, ip.get(at..)?, available, src, dst)
        }
        _ => None,
    }
}

fn transport_of(
    protocol: u8,
    transport: &[u8],
    available: usize,
    src: IpAddr,
    dst: IpAddr,
) -> Option<(Flow, u64)> {
    let (protocol, header) = match protocol {
        17 => (FlowProtocol::Udp, 8),
        6 => (FlowProtocol::Tcp, usize::from(*transport.get(12)? >> 4) * 4),
        _ => return None,
    };
    let src_port = u16_at(transport, 0, true)?;
    let dst_port = u16_at(transport, 2, true)?;
    // A UDP datagram says how long it is; a TCP segment is what the IP packet holds after its header
    let length = if protocol == FlowProtocol::Udp {
        usize::from(u16_at(transport, 4, true)?)
    } else {
        available
    };
    Some((
        Flow {
            src,
            dst,
            src_port,
            dst_port,
            protocol,
        },
        length.saturating_sub(header) as u64,
    ))
}

fn list(flows: &BTreeMap<Flow, u64>) -> String {
    let mut ordered: Vec<(&Flow, &u64)> = flows.iter().collect();
    ordered.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let mut lines: Vec<String> = ordered
        .iter()
        .take(LISTED_FLOWS)
        .map(|(flow, count)| format!("{}, {count} packets", flow.describe()))
        .collect();
    if ordered.len() > LISTED_FLOWS {
        lines.push(format!("and {} more", ordered.len() - LISTED_FLOWS));
    }
    lines.join("; ")
}

/// Reads the capture in `bytes` and makes the trace of the flow that `select` names: the moment of each packet since the first
/// of the flow, and its transport payload, raised to `smallest` bytes where it is smaller (`padded` counts those).
///
/// # Errors
///
/// A sentence if the file is not a capture, has packets that cannot be read, has no flow that matches, or has several (they are
/// listed, with the keys that pick one).
pub fn read_capture(
    bytes: &[u8],
    select: &FlowSelection,
    smallest: u64,
) -> Result<ReadTrace, String> {
    let frames = match u32_at(bytes, 0, true) {
        Some(0x0a0d_0d0a) => ng_frames(bytes)?,
        _ => classic_frames(bytes)?,
    };
    let mut seen = Vec::new();
    let mut flows: BTreeMap<Flow, u64> = BTreeMap::new();
    for frame in &frames {
        let Some(ip) = ip_packet(frame.link_type, frame.data)? else {
            continue;
        };
        if let Some((flow, payload)) = decode(ip) {
            *flows.entry(flow).or_default() += 1;
            seen.push(Seen {
                flow,
                timestamp_ns: frame.timestamp_ns,
                payload,
            });
        }
    }
    if flows.is_empty() {
        return Err("the capture has no UDP or TCP packet over IP".to_owned());
    }
    let matching: BTreeMap<Flow, u64> = flows
        .iter()
        .filter(|(flow, _)| flow.matches(select))
        .map(|(flow, count)| (*flow, *count))
        .collect();
    let chosen = match matching.len() {
        0 => {
            return Err(format!(
                "no flow of the capture matches `select`; the flows are: {}",
                list(&flows)
            ));
        }
        1 => *matching
            .keys()
            .next()
            .unwrap_or_else(|| unreachable!("one flow")),
        _ => {
            return Err(format!(
                "the capture has {} flows{}: {}; name one with `select` (src, dst, src-port, dst-port, protocol)",
                matching.len(),
                if *select == FlowSelection::default() {
                    ""
                } else {
                    " that match `select`"
                },
                list(&matching)
            ));
        }
    };
    let mut packets: Vec<(u128, u64)> = seen
        .iter()
        .filter(|packet| packet.flow == chosen)
        .map(|packet| (packet.timestamp_ns, packet.payload))
        .collect();
    if packets.len() > MAX_TRACE_PACKETS {
        return Err(format!(
            "the flow has {} packets, and a trace may have at most {MAX_TRACE_PACKETS}",
            packets.len()
        ));
    }
    // Captures are in the order of arrival; a packet whose time is earlier than the one before it is put where its time says
    packets.sort_by_key(|(timestamp, _)| *timestamp);
    let first = packets.first().map_or(0, |(timestamp, _)| *timestamp);
    let mut padded = 0_u32;
    let (mut moments, mut sizes) = (
        Vec::with_capacity(packets.len()),
        Vec::with_capacity(packets.len()),
    );
    for (timestamp, payload) in packets {
        moments.push(
            u64::try_from(timestamp - first)
                .map_err(|_| "the capture lasts too long".to_owned())?,
        );
        let size = if payload < smallest {
            padded += 1;
            smallest
        } else {
            payload
        };
        sizes.push(u32::try_from(size).map_err(|_| "a packet is too large".to_owned())?);
    }
    Ok(ReadTrace {
        data: TraceData::new(moments, sizes)?,
        padded,
    })
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)] // the frames of the tests are a few hundred bytes
mod tests {
    use super::*;

    /// A UDP packet over IPv4 in an Ethernet frame: `payload` bytes of data.
    fn udp_frame(src: [u8; 4], sport: u16, dport: u16, payload: usize) -> Vec<u8> {
        let mut frame = vec![0_u8; 12];
        frame.extend([0x08, 0x00]);
        let total = 20 + 8 + payload;
        let mut ip = vec![
            0x45,
            0,
            (total >> 8) as u8,
            (total & 0xff) as u8,
            0,
            0,
            0,
            0,
            64,
            17,
            0,
            0,
        ];
        ip.extend(src);
        ip.extend([10, 0, 0, 2]);
        frame.extend(ip);
        frame.extend(sport.to_be_bytes());
        frame.extend(dport.to_be_bytes());
        frame.extend(((8 + payload) as u16).to_be_bytes());
        frame.extend([0, 0]);
        frame.extend(vec![0xab; payload]);
        frame
    }

    /// A TCP segment over IPv4 in an Ethernet frame.
    fn tcp_frame(sport: u16, dport: u16, payload: usize) -> Vec<u8> {
        let mut frame = vec![0_u8; 12];
        frame.extend([0x08, 0x00]);
        let total = 20 + 20 + payload;
        let mut ip = vec![
            0x45,
            0,
            (total >> 8) as u8,
            (total & 0xff) as u8,
            0,
            0,
            0,
            0,
            64,
            6,
            0,
            0,
        ];
        ip.extend([10, 0, 0, 1, 10, 0, 0, 2]);
        frame.extend(ip);
        frame.extend(sport.to_be_bytes());
        frame.extend(dport.to_be_bytes());
        frame.extend([0; 8]);
        frame.extend([0x50, 0x10, 0, 0, 0, 0, 0, 0]);
        frame.extend(vec![0xcd; payload]);
        frame
    }

    /// A classic capture of `(seconds, microseconds, frame)`, little endian.
    fn classic(frames: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
        let mut file = vec![0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0];
        file.extend([0; 8]);
        file.extend(65_535_u32.to_le_bytes());
        file.extend(1_u32.to_le_bytes());
        for (seconds, micros, frame) in frames {
            file.extend(seconds.to_le_bytes());
            file.extend(micros.to_le_bytes());
            file.extend((frame.len() as u32).to_le_bytes());
            file.extend((frame.len() as u32).to_le_bytes());
            file.extend(frame);
        }
        file
    }

    fn nothing() -> FlowSelection {
        FlowSelection::default()
    }

    /// A capture of one flow: the moments are since the first packet and the sizes are the payloads of UDP.
    #[test]
    fn the_packets_of_one_flow_are_the_trace() {
        let frames: Vec<_> = [
            (100, 500_000, 160),
            (100, 520_000, 160),
            (100, 580_500, 40),
            (101, 0, 1200),
        ]
        .into_iter()
        .map(|(s, us, size)| (s, us, udp_frame([10, 0, 0, 1], 5004, 5004, size)))
        .collect();
        let read = read_capture(&classic(&frames), &nothing(), 16).unwrap();
        assert_eq!(
            read.data.moments_ns(),
            [0, 20_000_000, 80_500_000, 500_000_000]
        );
        assert_eq!(read.data.sizes(), [160, 160, 40, 1200]);
        assert_eq!(read.padded, 0);
    }

    /// A packet below the smallest payload is raised to it, and counted: an acknowledgement has no data at all.
    #[test]
    fn packets_below_the_smallest_payload_are_raised_and_counted() {
        let frames = vec![
            (1, 0, tcp_frame(443, 50_000, 0)),
            (1, 1000, tcp_frame(443, 50_000, 1448)),
            (1, 2000, tcp_frame(443, 50_000, 8)),
        ];
        let read = read_capture(&classic(&frames), &nothing(), 16).unwrap();
        assert_eq!(read.data.sizes(), [16, 1448, 16]);
        assert_eq!(read.data.moments_ns(), [0, 1_000_000, 2_000_000]);
        assert_eq!(read.padded, 2);
        // Canary: for a stream without the checksum the smallest payload is larger, so more packets are raised
        assert_eq!(
            read_capture(&classic(&frames), &nothing(), 20)
                .unwrap()
                .padded,
            2
        );
        assert_eq!(
            read_capture(&classic(&frames), &nothing(), 1448)
                .unwrap()
                .padded,
            2
        );
        assert_eq!(
            read_capture(&classic(&frames), &nothing(), 1449)
                .unwrap()
                .padded,
            3
        );
    }

    /// Several flows are an error that lists them, and `select` picks one; a selection that matches nothing lists the flows too.
    #[test]
    fn a_capture_of_several_flows_needs_a_selection() {
        let frames = vec![
            (1, 0, udp_frame([10, 0, 0, 1], 5004, 5004, 100)),
            (1, 1, udp_frame([10, 0, 0, 1], 5004, 5004, 100)),
            (1, 2, udp_frame([10, 0, 0, 3], 6000, 5004, 300)),
            (1, 3, tcp_frame(443, 50_000, 500)),
        ];
        let file = classic(&frames);
        let error = read_capture(&file, &nothing(), 16).unwrap_err();
        assert!(error.contains("the capture has 3 flows"), "{error}");
        assert!(
            error.contains("10.0.0.1:5004 to 10.0.0.2:5004 (udp), 2 packets"),
            "{error}"
        );
        let by_port = FlowSelection {
            src_port: Some(6000),
            ..nothing()
        };
        assert_eq!(
            read_capture(&file, &by_port, 16).unwrap().data.sizes(),
            [300]
        );
        let by_protocol = FlowSelection {
            protocol: Some(FlowProtocol::Tcp),
            ..nothing()
        };
        assert_eq!(
            read_capture(&file, &by_protocol, 16).unwrap().data.sizes(),
            [500]
        );
        let by_address = FlowSelection {
            src: Some("10.0.0.1".parse().unwrap()),
            ..nothing()
        };
        let still_two = read_capture(&file, &by_address, 16).unwrap_err();
        assert!(
            still_two.contains("flows that match `select`"),
            "{still_two}"
        );
        let none = FlowSelection {
            dst_port: Some(1),
            ..nothing()
        };
        let none = read_capture(&file, &none, 16).unwrap_err();
        assert!(
            none.contains("no flow of the capture matches `select`"),
            "{none}"
        );
    }

    #[test]
    fn a_file_that_is_not_a_capture_or_is_cut_short_is_refused() {
        assert!(
            read_capture(b"hello world", &nothing(), 16)
                .unwrap_err()
                .contains("not a capture")
        );
        assert!(
            read_capture(&[], &nothing(), 16)
                .unwrap_err()
                .contains("too short")
        );
        let frames = vec![(1, 0, udp_frame([10, 0, 0, 1], 1, 2, 100))];
        let mut file = classic(&frames);
        assert!(read_capture(&file, &nothing(), 16).is_ok());
        file.truncate(file.len() - 10);
        assert!(
            read_capture(&file, &nothing(), 16)
                .unwrap_err()
                .contains("ends in the middle of the packet")
        );
        let empty = classic(&[]);
        assert!(
            read_capture(&empty, &nothing(), 16)
                .unwrap_err()
                .contains("no UDP or TCP packet")
        );
        // A link type that is not understood is named
        let mut other = classic(&frames);
        other[20..24].copy_from_slice(&147_u32.to_le_bytes());
        assert!(
            read_capture(&other, &nothing(), 16)
                .unwrap_err()
                .contains("link type 147")
        );
    }

    /// Packets whose times are out of order are put in order of time; a fragment after the first, and a packet that is not IP,
    /// are in no flow.
    #[test]
    fn packets_are_ordered_by_time_and_what_is_not_a_flow_is_left_out() {
        let mut fragment = udp_frame([10, 0, 0, 1], 5004, 5004, 100);
        fragment[14 + 6] = 0x00;
        fragment[14 + 7] = 0x10;
        let mut arp = vec![0_u8; 12];
        arp.extend([0x08, 0x06]);
        arp.extend([0; 28]);
        let frames = vec![
            (5, 300, udp_frame([10, 0, 0, 1], 5004, 5004, 50)),
            (5, 100, udp_frame([10, 0, 0, 1], 5004, 5004, 60)),
            (5, 200, fragment),
            (5, 250, arp),
        ];
        let read = read_capture(&classic(&frames), &nothing(), 16).unwrap();
        assert_eq!(read.data.sizes(), [60, 50]);
        assert_eq!(read.data.moments_ns(), [0, 200_000]);
    }

    /// A `pcapng` file with a nanosecond resolution and one with the default, in the byte order of the writer.
    #[test]
    fn a_pcapng_capture_is_read_with_its_resolution() {
        fn block(kind: u32, body: &[u8]) -> Vec<u8> {
            let padded = body.len().div_ceil(4) * 4;
            let total = (12 + padded) as u32;
            let mut out = Vec::new();
            out.extend(kind.to_le_bytes());
            out.extend(total.to_le_bytes());
            out.extend(body);
            out.extend(vec![0; padded - body.len()]);
            out.extend(total.to_le_bytes());
            out
        }
        let section = |body_options: &[u8]| {
            let mut file = block(0x0a0d_0d0a, &{
                let mut body = Vec::new();
                body.extend(0x1a2b_3c4d_u32.to_le_bytes());
                body.extend([1, 0, 0, 0]);
                body.extend((-1_i64).to_le_bytes());
                body
            });
            let mut interface = Vec::new();
            interface.extend(1_u16.to_le_bytes());
            interface.extend([0, 0]);
            interface.extend(0_u32.to_le_bytes());
            interface.extend(body_options);
            file.extend(block(1, &interface));
            for (ticks, size) in [(2_000_000_000_u64, 100), (2_000_500_000, 200)] {
                let frame = udp_frame([10, 0, 0, 1], 9, 9, size);
                let mut body = Vec::new();
                body.extend(0_u32.to_le_bytes());
                body.extend(((ticks >> 32) as u32).to_le_bytes());
                body.extend((ticks as u32).to_le_bytes());
                body.extend((frame.len() as u32).to_le_bytes());
                body.extend((frame.len() as u32).to_le_bytes());
                body.extend(&frame);
                file.extend(block(6, &body));
            }
            file
        };
        // The default is microseconds: 500,000 ticks are half a second
        let micros = read_capture(&section(&[]), &nothing(), 16).unwrap();
        assert_eq!(micros.data.moments_ns(), [0, 500_000_000]);
        assert_eq!(micros.data.sizes(), [100, 200]);
        // Nanoseconds (if_tsresol = 9): 500,000 ticks are half a millisecond
        let mut option = Vec::new();
        option.extend(9_u16.to_le_bytes());
        option.extend(1_u16.to_le_bytes());
        option.extend([9, 0, 0, 0]);
        option.extend([0; 4]);
        let nanos = read_capture(&section(&option), &nothing(), 16).unwrap();
        assert_eq!(nanos.data.moments_ns(), [0, 500_000]);
        // Canary: a truncated block is refused with its place
        let mut cut = section(&[]);
        cut.truncate(cut.len() - 6);
        assert!(
            read_capture(&cut, &nothing(), 16)
                .unwrap_err()
                .contains("is not a block")
        );
    }

    /// IPv6 with an extension header, and a VLAN tag in the Ethernet frame.
    #[test]
    fn ipv6_and_vlan_tags_are_read() {
        let mut ip = vec![0x60, 0, 0, 0];
        let payload: usize = 8 + 8 + 50;
        ip.extend((payload as u16).to_be_bytes());
        ip.extend([60, 64]);
        ip.extend([0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        ip.extend([0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
        // A destination options header of 8 bytes, then UDP
        ip.extend([17, 0, 1, 4, 0, 0, 0, 0]);
        ip.extend(7_u16.to_be_bytes());
        ip.extend(8_u16.to_be_bytes());
        ip.extend(((8 + 50) as u16).to_be_bytes());
        ip.extend([0, 0]);
        ip.extend(vec![1; 50]);
        let mut frame = vec![0_u8; 12];
        frame.extend([0x81, 0x00, 0x00, 0x05, 0x86, 0xdd]);
        frame.extend(ip);
        let read = read_capture(&classic(&[(1, 0, frame)]), &nothing(), 16).unwrap();
        assert_eq!(read.data.sizes(), [50]);
    }
}
