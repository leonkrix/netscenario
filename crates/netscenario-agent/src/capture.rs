//! The capture of one interface of a node into a pcap file (PRB-09, D-047, D-073).
//!
//! A packet socket (`AF_PACKET`) bound to the interface sees every frame the interface carries, in both directions, with the
//! time at which the kernel stamped it (`SO_TIMESTAMPNS`, the real-time clock). The agent writes the frames as they come and
//! checks itself against the counters of the interface: the frames it wrote must be as many as the interface counted
//! (received and sent) while the capture ran, and if fewer, the summary says how many are missing. A capture that lost
//! frames because its buffer was full is therefore never silently incomplete.
//!
//! The socket is made with `nix`, because `rustix` has no way to bind a packet socket to an interface or to read the
//! timestamps; nothing here is `unsafe`. It needs `CAP_NET_RAW` in the node. The buffer of the socket is as large as the
//! system allows (`net.core.rmem_max`), and the summary says how large it got.
//!
//! What a capture shows: the interface after its queue. A packet that the link dropped never reached the interface and is in
//! no capture; what a program sent is in the records of its agent.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufWriter, IoSliceMut, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nix::sys::socket::{
    AddressFamily, ControlMessageOwned, LinkAddr, MsgFlags, SockFlag, SockProtocol, SockType,
    getsockopt, recvmsg, setsockopt, socket, sockopt,
};
use nix::sys::time::{TimeVal, TimeValLike};

use crate::clock::monotonic_ns;
use crate::events::CaptureSummary;
use crate::pcap;

/// The buffer that the capture asks for; the kernel gives at most `rmem_max`.
const RECEIVE_BUFFER: usize = 64 * 1024 * 1024;

/// How long the capture keeps reading queued frames after the stop, at most.
const DRAIN: Duration = Duration::from_millis(200);

/// The largest frame the capture reads whole (a frame of a virtual device can be larger than the MTU).
const FRAME_BUFFER: usize = 65_536;

/// What a capture is to do.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// The interface in the node.
    pub interface: String,
    /// The number the run gave this capture.
    pub flow: u32,
    /// The receive buffer to ask for, in bytes; without it, as much as the system allows. A small one makes a capture that loses
    /// frames, which is how the test of the check against the counter of the interface gets one.
    pub receive_buffer: Option<usize>,
}

/// A capture that is open: frames are collected from here on.
#[derive(Debug)]
pub struct Capture {
    config: CaptureConfig,
    socket: OwnedFd,
    /// The size of the receive buffer that the kernel gave, in bytes.
    pub receive_buffer_bytes: u64,
    counted_before: Option<u64>,
    /// The real-time clock minus `CLOCK_MONOTONIC` at the start, in nanoseconds: what turns the time of a frame into the time
    /// of the records of the agents.
    pub realtime_minus_monotonic_ns: i64,
}

/// An error of the capture, as text for the person who reads the message of the run.
fn failed(what: &str, error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("{what}: {error}"))
}

/// The frames that an interface has counted, received and sent, from the text of `/proc/net/dev`.
#[must_use]
pub fn frames_counted(table: &str, interface: &str) -> Option<u64> {
    let line = table
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(interface)?.strip_prefix(':'))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .map(|field| field.parse().unwrap_or(0))
        .collect();
    // bytes packets errs drop fifo frame compressed multicast | bytes packets ...
    Some(fields.get(1)? + fields.get(9)?)
}

fn counter(interface: &str) -> Option<u64> {
    frames_counted(&std::fs::read_to_string("/proc/net/dev").ok()?, interface)
}

impl Capture {
    /// Opens the packet socket on `config.interface`.
    ///
    /// # Errors
    ///
    /// The reason the socket cannot be made (most likely `CAP_NET_RAW` is missing in the node, or the interface does not exist).
    pub fn open(config: CaptureConfig) -> io::Result<Self> {
        let socket = socket(
            AddressFamily::Packet,
            SockType::Raw,
            SockFlag::SOCK_CLOEXEC,
            SockProtocol::EthAll,
        )
        .map_err(|error| {
            failed(
                "cannot open a packet socket (the capture needs CAP_NET_RAW in the node)",
                error,
            )
        })?;
        setsockopt(
            &socket,
            sockopt::BindToDevice,
            &OsString::from(&config.interface),
        )
        .map_err(|error| {
            failed(
                &format!("cannot bind to the interface '{}'", config.interface),
                error,
            )
        })?;
        setsockopt(&socket, sockopt::ReceiveTimestampns, &true)
            .map_err(|error| failed("cannot ask for the timestamps of the kernel", error))?;
        // More than rmem_max is silently cut to it
        let _ = setsockopt(
            &socket,
            sockopt::RcvBuf,
            &config.receive_buffer.unwrap_or(RECEIVE_BUFFER),
        );
        let receive_buffer_bytes = getsockopt(&socket, sockopt::RcvBuf)
            .ok()
            .map_or(0, |bytes| bytes as u64);
        // A wait for a frame lasts 20 ms at most, and then the stop flag is looked at
        setsockopt(&socket, sockopt::ReceiveTimeout, &TimeVal::milliseconds(20))
            .map_err(|error| failed("cannot set the timeout", error))?;
        // Both clocks, one after the other: the offset is right to within the time between the two readings
        let before = monotonic_ns();
        let realtime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| i128::try_from(time.as_nanos()).unwrap_or(0));
        let after = monotonic_ns();
        let middle = i128::from(before) + (i128::from(after) - i128::from(before)) / 2;
        let offset = i64::try_from(realtime - middle).unwrap_or(0);
        // The socket is open before the counter is read: a frame between the two is captured and not counted, which is
        // harmless, whereas the other order could miss frames silently
        let counted_before = counter(&config.interface);
        Ok(Self {
            config,
            socket,
            receive_buffer_bytes,
            counted_before,
            realtime_minus_monotonic_ns: offset,
        })
    }

    /// Reads frames into `file` until `stop` is set and the queue is empty (at most `DRAIN` after the stop).
    ///
    /// # Errors
    ///
    /// The error of the socket or the file.
    pub fn run(self, stop: &AtomicBool, file: File) -> io::Result<CaptureSummary> {
        let mut out = BufWriter::new(file);
        pcap::write_header(&mut out)?;
        let mut frame = vec![0_u8; FRAME_BUFFER];
        let mut space = nix::cmsg_space!(nix::sys::time::TimeSpec);
        let (mut packets, mut bytes) = (0_u64, 0_u64);
        let (mut first, mut last) = (None, None);
        let mut drain_until: Option<Instant> = None;
        loop {
            let mut slices = [IoSliceMut::new(&mut frame)];
            let received = match recvmsg::<LinkAddr>(
                self.socket.as_raw_fd(),
                &mut slices,
                Some(&mut space),
                MsgFlags::empty(),
            ) {
                Ok(received) => received,
                Err(nix::errno::Errno::EAGAIN | nix::errno::Errno::EINTR) => {
                    // Nothing came within the timeout: the queue is empty
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    continue;
                }
                Err(error) => return Err(failed("cannot read the interface", error)),
            };
            let length = received.bytes;
            let mut stamp = None;
            if let Ok(messages) = received.cmsgs() {
                for message in messages {
                    if let ControlMessageOwned::ScmTimestampns(time) = message {
                        stamp = Some((
                            u64::try_from(time.tv_sec()).unwrap_or(0),
                            u64::try_from(time.tv_nsec()).unwrap_or(0),
                        ));
                    }
                }
            }
            // Every frame has a stamp of the kernel; a frame without one is stamped with the clock of the agent
            let (seconds, nanoseconds) = stamp.unwrap_or_else(|| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                (now.as_secs(), u64::from(now.subsec_nanos()))
            });
            pcap::write_record(
                &mut out,
                u32::try_from(seconds).unwrap_or(u32::MAX),
                u32::try_from(nanoseconds).unwrap_or(0),
                &frame[..length],
            )?;
            packets += 1;
            bytes += length as u64;
            let time_ns = seconds * 1_000_000_000 + nanoseconds;
            first.get_or_insert(time_ns);
            last = Some(time_ns);
            if stop.load(Ordering::SeqCst) {
                // Keep reading what is queued, but not for ever on an interface that never stops carrying frames
                let limit = *drain_until.get_or_insert_with(|| Instant::now() + DRAIN);
                if Instant::now() >= limit {
                    break;
                }
            }
        }
        out.flush()?;
        let counted_after = counter(&self.config.interface);
        let counted = self
            .counted_before
            .zip(counted_after)
            .map(|(before, after)| after.saturating_sub(before));
        Ok(CaptureSummary {
            flow: self.config.flow,
            interface: self.config.interface,
            packets,
            bytes,
            first_time_ns: first,
            last_time_ns: last,
            realtime_minus_monotonic_ns: self.realtime_minus_monotonic_ns,
            receive_buffer_bytes: self.receive_buffer_bytes,
            interface_frames: counted,
            missing: counted.map(|counted| counted.saturating_sub(packets)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:    1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0
 np0-a: 123456     789    0    0    0     0          0         0  654321     321    0    0    0     0       0          0
 np0-ab:   1       5    0    0    0     0          0         0     1       6    0    0    0     0       0          0
";

    #[test]
    fn the_frames_of_an_interface_are_those_received_and_sent() {
        assert_eq!(frames_counted(TABLE, "np0-a"), Some(789 + 321));
        assert_eq!(frames_counted(TABLE, "lo"), Some(20));
        // A name that is the beginning of another is not that other
        assert_eq!(frames_counted(TABLE, "np0-ab"), Some(11));
        assert_eq!(frames_counted(TABLE, "np0"), None);
        assert_eq!(frames_counted("", "lo"), None);
    }
}
