//! The sender: a constant bit rate stream of packets to one address (WRK-01).

use std::fmt;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::os::fd::OwnedFd;
use std::time::Duration;

use rustix::net::{AddressFamily, SendFlags, SocketType, ipproto};

use crate::clock::monotonic_ns;
use crate::events::{BarrierDone, SendSummary};
use crate::schedule::Stream;
use crate::wire::{HEADER_LEN, Header, UdpChecksum};

/// The largest payload of an IPv4 UDP datagram. The scenario model allows far less (1472, the MTU).
const MAX_PAYLOAD: u64 = 65_507;

/// What to send.
#[derive(Debug, Clone)]
pub struct SendConfig {
    /// Where the packets go.
    pub target: SocketAddr,
    /// The workload, written to the summary.
    pub flow: u32,
    /// The rate and the size of the packets.
    pub stream: Stream,
    /// How long the stream lasts, counted from its start, in nanoseconds.
    pub duration_ns: u64,
    /// The moment the first packet leaves, on `CLOCK_MONOTONIC`. Without it, now.
    pub start_at_ns: Option<u64>,
    /// Whether the packets carry the checksum of UDP. Off needs a raw socket (`CAP_NET_RAW`) and IPv4.
    pub udp_checksum: UdpChecksum,
    /// The barriers of the timeline (D-038): before which packet the run does something to the link. In the order
    /// of the packets, and for one packet in the order of the ids.
    pub barriers: Vec<Barrier>,
}

/// A point of the stream where the sender waits for the run: before `packet` the run does the thing `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Barrier {
    /// The first packet that meets what the run does.
    pub packet: u64,
    /// The number that the run gave it.
    pub id: u32,
}

/// How the sender talks to the run about its barriers (D-038). The sender asks right after it has sent the
/// packet before the barrier, so that the run has the time between two packets to do its thing, and waits for the
/// answer at the slot of the packet, where it costs nothing if the answer is there already.
pub trait Barriers {
    /// Tells the run that the barrier `id` is reached: the packets before `packet` are sent.
    fn ask(&mut self, id: u32, packet: u64);

    /// Waits for the answer to `id`. `true` if it came, `false` if the sender gave up waiting (it goes on, and the
    /// summary says so, because the guarantee of D-038 does not hold for that barrier).
    fn wait(&mut self, id: u32) -> bool;
}

/// Asks the barriers of `planned` from index `from` on that belong to `packet`, and notes when.
fn ask(
    planned: &[Barrier],
    from: usize,
    packet: u64,
    asked: &mut Vec<(Barrier, u64)>,
    port: &mut dyn Barriers,
) {
    for barrier in planned[from..].iter().take_while(|b| b.packet == packet) {
        port.ask(barrier.id, barrier.packet);
        asked.push((*barrier, monotonic_ns()));
    }
}

/// A stream without barriers, as in release 0.1.
struct NoBarriers;

impl Barriers for NoBarriers {
    fn ask(&mut self, _id: u32, _packet: u64) {}

    fn wait(&mut self, _id: u32) -> bool {
        true
    }
}

/// The length of the header of UDP.
const UDP_HEADER_LEN: usize = 8;

/// The source port of the packets of a stream without the checksum. A raw socket binds no port; the receiver
/// does not look at it. The number is in the range of the ports that the system hands out.
fn source_port(flow: u32) -> u16 {
    let offset = u16::try_from(flow % 16_384).unwrap_or(0);
    49_152 + offset
}

/// Where the packets go out: a UDP socket, or a raw IPv4 socket on which the program writes the header of UDP
/// itself, because that is the way to a checksum of `0` in safe code (`SO_NO_CHECK` has no safe wrapper, and the
/// workspace forbids `unsafe`, D-025). The datagram of a raw socket starts with the 8 bytes of the header of
/// UDP, so `offset` is where the payload begins in the buffer that is sent.
enum Outlet {
    Udp(UdpSocket),
    Raw(OwnedFd),
}

impl Outlet {
    fn open(config: &SendConfig) -> Result<Self, SendError> {
        match config.udp_checksum {
            UdpChecksum::On => {
                let bind: SocketAddr = if config.target.is_ipv4() {
                    ([0, 0, 0, 0], 0).into()
                } else {
                    ([0_u16; 8], 0).into()
                };
                let socket = UdpSocket::bind(bind)
                    .map_err(|error| SendError(format!("cannot open a socket: {error}")))?;
                socket.connect(config.target).map_err(|error| {
                    SendError(format!("cannot reach {}: {error}", config.target))
                })?;
                Ok(Self::Udp(socket))
            }
            UdpChecksum::Off => {
                let SocketAddr::V4(target) = config.target else {
                    return Err(SendError(
                        "a stream without the checksum of UDP needs IPv4: IPv6 requires the checksum"
                            .to_owned(),
                    ));
                };
                let socket = rustix::net::socket(
                    AddressFamily::INET,
                    SocketType::RAW,
                    Some(ipproto::UDP),
                )
                .map_err(|error| {
                    SendError(format!(
                        "cannot open a raw socket, which a stream without the checksum of UDP needs: {error}. The sender needs the capability CAP_NET_RAW in its node (a user namespace and root have it)"
                    ))
                })?;
                // The port of a raw socket is not used; the system picks the route and the source address
                rustix::net::connect(&socket, &std::net::SocketAddrV4::new(*target.ip(), 0))
                    .map_err(|error| SendError(format!("cannot reach {target}: {error}")))?;
                Ok(Self::Raw(socket))
            }
        }
    }

    /// How many bytes come before the payload in the buffer that `send` takes.
    fn offset(&self) -> usize {
        match self {
            Self::Udp(_) => 0,
            Self::Raw(_) => UDP_HEADER_LEN,
        }
    }

    fn send(&self, datagram: &[u8]) -> io::Result<()> {
        match self {
            Self::Udp(socket) => socket.send(datagram).map(|_| ()),
            Self::Raw(socket) => rustix::net::send(socket, datagram, SendFlags::empty())
                .map(|_| ())
                .map_err(io::Error::from),
        }
    }
}

/// Why the sender did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError(pub String);

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SendError {}

/// Waits until `CLOCK_MONOTONIC` reads `target_ns`: it sleeps while the moment is far away and spins for the
/// last stretch, because a sleep wakes up some tens of microseconds late.
fn wait_until(target_ns: u64) {
    const SPIN_NS: u64 = 150_000;
    loop {
        let now = monotonic_ns();
        if now >= target_ns {
            return;
        }
        let remaining = target_ns - now;
        if remaining > SPIN_NS * 2 {
            std::thread::sleep(Duration::from_nanos(remaining - SPIN_NS));
        } else {
            std::hint::spin_loop();
        }
    }
}

/// Sends the stream and returns what was done.
///
/// # Errors
///
/// A [`SendError`] if the settings are not valid or the socket cannot be made. Packets that the kernel
/// refuses later are counted in the summary and do not stop the stream.
pub fn run(config: &SendConfig) -> Result<SendSummary, SendError> {
    run_with(config, &mut NoBarriers)
}

/// Sends the stream, asking the run at the barriers of `config` through `barriers`.
///
/// # Errors
///
/// As [`run`].
#[allow(clippy::too_many_lines)] // one loop, with the checks before it
pub fn run_with(
    config: &SendConfig,
    barriers: &mut dyn Barriers,
) -> Result<SendSummary, SendError> {
    let SendConfig {
        target,
        flow,
        stream,
        duration_ns,
        start_at_ns,
        udp_checksum,
        barriers: planned,
    } = config;
    if stream.bits_per_second == 0 {
        return Err(SendError("the rate must be more than zero".to_owned()));
    }
    let header = u64::try_from(udp_checksum.header_len()).unwrap_or(16);
    if !(header..=MAX_PAYLOAD).contains(&stream.packet_size) {
        return Err(SendError(format!(
            "the packet size {} is outside {header} to {MAX_PAYLOAD} bytes",
            stream.packet_size
        )));
    }
    let count = stream.packets_in(*duration_ns);
    if count > u64::from(u32::MAX) + 1 {
        return Err(SendError(format!(
            "{count} packets do not fit the 32 bit sequence number of the wire format (at most {})",
            u64::from(u32::MAX) + 1
        )));
    }
    let outlet = Outlet::open(config)?;
    // The header of UDP (raw socket only) and the payload in one buffer, which is what is sent
    let offset = outlet.offset();
    let size = usize::try_from(stream.packet_size).unwrap_or(HEADER_LEN);
    let mut datagram = vec![0_u8; offset + size];
    if matches!(outlet, Outlet::Raw(_)) {
        let length = u16::try_from(datagram.len()).unwrap_or(u16::MAX);
        let port = target.port();
        datagram[0..2].copy_from_slice(&source_port(*flow).to_be_bytes());
        datagram[2..4].copy_from_slice(&port.to_be_bytes());
        datagram[4..6].copy_from_slice(&length.to_be_bytes());
        // The checksum field stays 0: that is the point
    }
    let start = start_at_ns.unwrap_or_else(monotonic_ns);
    let mut summary = SendSummary {
        flow: *flow,
        packets: 0,
        first_send_ns: None,
        last_send_ns: None,
        max_lateness_ns: 0,
        errors: 0,
        first_error: None,
        barriers: Vec::new(),
        barrier_timeouts: 0,
    };
    // The barriers are asked one packet ahead and answered at the slot of the packet they belong to
    let mut asked: Vec<(Barrier, u64)> = Vec::new();
    let mut next_barrier = 0_usize;
    ask(planned, 0, 0, &mut asked, barriers);
    for n in 0..count {
        let due = start.saturating_add(stream.offset_ns(n));
        wait_until(due);
        // What the run is to do before this packet is done by now, or the packet waits for it
        while let Some((barrier, asked_at)) = asked.get(next_barrier).copied() {
            if barrier.packet != n {
                break;
            }
            let began = monotonic_ns();
            if !barriers.wait(barrier.id) {
                summary.barrier_timeouts += 1;
            }
            summary.barriers.push(BarrierDone {
                id: barrier.id,
                packet: barrier.packet,
                asked_ns: asked_at,
                held_ns: monotonic_ns().saturating_sub(began),
            });
            next_barrier += 1;
        }
        let now = monotonic_ns();
        let Ok(sequence) = u32::try_from(n) else {
            break;
        };
        let header = Header {
            sequence,
            send_ns: now,
        };
        match udp_checksum {
            UdpChecksum::On => header.write(&mut datagram[offset..]),
            UdpChecksum::Off => header.write_v2(&mut datagram[offset..]),
        }
        match outlet.send(&datagram) {
            Ok(()) => {
                summary.packets += 1;
                summary.first_send_ns.get_or_insert(now);
                summary.last_send_ns = Some(now);
            }
            Err(error) => {
                summary.errors += 1;
                summary.first_error.get_or_insert_with(|| error.to_string());
            }
        }
        summary.max_lateness_ns = summary.max_lateness_ns.max(now.saturating_sub(due));
        // The barriers of the next packet are asked right now: the run has the time until its slot
        ask(
            planned,
            planned.partition_point(|b| b.packet <= n),
            n + 1,
            &mut asked,
            barriers,
        );
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(rate: u64, size: u64, duration_ns: u64) -> SendConfig {
        SendConfig {
            target: ([127, 0, 0, 1], 9).into(),
            flow: 0,
            stream: Stream {
                bits_per_second: rate,
                packet_size: size,
            },
            duration_ns,
            start_at_ns: None,
            udp_checksum: UdpChecksum::On,
            barriers: Vec::new(),
        }
    }

    #[test]
    fn nonsense_is_refused_before_anything_is_sent() {
        let zero = run(&config(0, 1000, 1_000_000)).unwrap_err();
        assert!(zero.0.contains("more than zero"), "{zero}");
        let small = run(&config(1_000_000, 15, 1_000_000)).unwrap_err();
        assert!(small.0.contains("outside 16 to 65507"), "{small}");
        let huge = run(&config(1_000_000, 70_000, 1_000_000)).unwrap_err();
        assert!(huge.0.contains("outside 16 to 65507"), "{huge}");
    }

    /// Without the checksum the packet carries a CRC-32 and needs four bytes more, and the stream is IPv4:
    /// both are refused before a raw socket is asked for (which needs a capability).
    #[test]
    fn a_stream_without_the_checksum_is_refused_where_it_cannot_work() {
        let mut small = config(1_000_000, 19, 1_000_000);
        small.udp_checksum = UdpChecksum::Off;
        let error = run(&small).unwrap_err();
        assert!(error.0.contains("outside 20 to 65507"), "{error}");
        let mut six = config(1_000_000, 100, 1_000_000);
        six.udp_checksum = UdpChecksum::Off;
        six.target = "[::1]:9".parse().unwrap();
        let error = run(&six).unwrap_err();
        assert!(error.0.contains("needs IPv4"), "{error}");
    }

    #[test]
    fn a_stream_that_overflows_the_sequence_number_is_refused() {
        // 100,000 packets per second for a day is 8.6 billion packets
        let error = run(&config(800_000_000, 1000, 86_400 * 1_000_000_000)).unwrap_err();
        assert!(error.0.contains("32 bit sequence"), "{error}");
    }

    #[test]
    fn waiting_ends_at_the_moment_and_not_before() {
        let target = monotonic_ns() + 3_000_000;
        wait_until(target);
        let now = monotonic_ns();
        assert!(now >= target);
        assert!(
            now - target < 50_000_000,
            "woke up {} ns late",
            now - target
        );
        // A moment in the past returns at once
        wait_until(1);
    }
}
