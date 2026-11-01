//! The receiver: records the arrival of every packet of the agent (PRB-01).

use std::io::{self, Write};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::clock::monotonic_ns;
use crate::events::ReceiveSummary;
use crate::records::{Record, Writer};
use crate::wire::{Header, UdpChecksum};

/// How long the receiver keeps reading queued packets after the stop, at most, in nanoseconds.
const DRAIN_NS: u64 = 100_000_000;

/// How long a wait for a packet lasts before the receiver looks at the stop flag again.
const POLL: Duration = Duration::from_millis(20);

/// The size of the receive buffer that the receiver asks for. The kernel may give less (`rmem_max`). A
/// large one keeps a burst from being lost in the receiver, where it would look like loss in the network.
const RECEIVE_BUFFER: usize = 4 * 1024 * 1024;

/// What to receive.
#[derive(Debug, Clone)]
pub struct ReceiveConfig {
    /// The address to listen on. Port 0 lets the system choose.
    pub bind: SocketAddr,
    /// The workload, written to the file and the summary.
    pub flow: u32,
    /// Stop after this many nanoseconds even if nobody said stop. A safety net.
    pub max_time_ns: Option<u64>,
    /// Whether the stream carries the checksum of UDP. Off: the packets are version 2, and a datagram that
    /// is not a good one is counted as `corrupted`.
    pub udp_checksum: UdpChecksum,
}

/// A receiver that is bound and not yet running.
#[derive(Debug)]
pub struct Receiver {
    socket: UdpSocket,
    flow: u32,
    max_time_ns: Option<u64>,
    udp_checksum: UdpChecksum,
}

impl Receiver {
    /// Binds the socket.
    ///
    /// # Errors
    ///
    /// The error of the system if the address cannot be used.
    pub fn bind(config: &ReceiveConfig) -> io::Result<Self> {
        let socket = UdpSocket::bind(config.bind)?;
        socket.set_read_timeout(Some(POLL))?;
        // Not every system allows the size: a smaller buffer still works
        let _ = rustix::net::sockopt::set_socket_recv_buffer_size(&socket, RECEIVE_BUFFER);
        Ok(Self {
            socket,
            flow: config.flow,
            max_time_ns: config.max_time_ns,
            udp_checksum: config.udp_checksum,
        })
    }

    /// The address that the receiver listens on, with the port the system chose.
    ///
    /// # Errors
    ///
    /// The error of the system.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Records packets into `out` (the record file) until `stop` is set or the time is up, then returns what
    /// was seen. Packets that are already queued when the stop comes are still recorded.
    ///
    /// # Errors
    ///
    /// The error of the socket or of `out`.
    pub fn run(self, stop: &AtomicBool, out: impl Write) -> io::Result<ReceiveSummary> {
        let mut writer = Writer::new(io::BufWriter::with_capacity(1 << 20, out), self.flow)?;
        let mut summary = ReceiveSummary {
            flow: self.flow,
            packets: 0,
            foreign: 0,
            corrupted: 0,
            wire_packets: None,
            first_receive_ns: None,
            last_receive_ns: None,
        };
        let deadline = self
            .max_time_ns
            .map(|limit| monotonic_ns().saturating_add(limit));
        let mut buffer = [0_u8; 2048];
        // When the stop has come, the packets that are already queued are still read, but not for long
        let mut drain_end: Option<u64> = None;
        loop {
            if drain_end.is_none()
                && (stop.load(Ordering::SeqCst) || deadline.is_some_and(|d| monotonic_ns() >= d))
            {
                drain_end = Some(monotonic_ns().saturating_add(DRAIN_NS));
            }
            if drain_end.is_some_and(|end| monotonic_ns() >= end) {
                break;
            }
            match self.socket.recv(&mut buffer) {
                Ok(length) => {
                    let received = monotonic_ns();
                    let datagram = &buffer[..length];
                    let header = match self.udp_checksum {
                        UdpChecksum::On => Header::read(datagram),
                        UdpChecksum::Off => Header::read_v2(datagram),
                    };
                    if let Ok(header) = header {
                        writer.write(&Record {
                            sequence: u64::from(header.sequence),
                            send_ns: header.send_ns,
                            receive_ns: received,
                        })?;
                        summary.packets += 1;
                        summary.first_receive_ns.get_or_insert(received);
                        summary.last_receive_ns = Some(received);
                    } else if self.udp_checksum == UdpChecksum::Off {
                        summary.corrupted += 1;
                    } else {
                        summary.foreign += 1;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    // The queue is empty: after the stop there is nothing left to read
                    if drain_end.is_some() {
                        break;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        writer.finish()?;
        Ok(summary)
    }
}
