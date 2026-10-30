//! The sender: a constant bit rate stream of packets to one address (WRK-01).

use std::fmt;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use crate::clock::monotonic_ns;
use crate::events::SendSummary;
use crate::schedule::Stream;
use crate::wire::{HEADER_LEN, Header};

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
    let SendConfig {
        target,
        flow,
        stream,
        duration_ns,
        start_at_ns,
    } = config;
    if stream.bits_per_second == 0 {
        return Err(SendError("the rate must be more than zero".to_owned()));
    }
    let header = u64::try_from(HEADER_LEN).unwrap_or(16);
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
    let bind: SocketAddr = if target.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        ([0_u16; 8], 0).into()
    };
    let socket = UdpSocket::bind(bind)
        .map_err(|error| SendError(format!("cannot open a socket: {error}")))?;
    socket
        .connect(target)
        .map_err(|error| SendError(format!("cannot reach {target}: {error}")))?;
    let mut payload = vec![0_u8; usize::try_from(stream.packet_size).unwrap_or(HEADER_LEN)];
    let start = start_at_ns.unwrap_or_else(monotonic_ns);
    let mut summary = SendSummary {
        flow: *flow,
        packets: 0,
        first_send_ns: None,
        last_send_ns: None,
        max_lateness_ns: 0,
        errors: 0,
        first_error: None,
    };
    for n in 0..count {
        let due = start.saturating_add(stream.offset_ns(n));
        wait_until(due);
        let now = monotonic_ns();
        let Ok(sequence) = u32::try_from(n) else {
            break;
        };
        Header {
            sequence,
            send_ns: now,
        }
        .write(&mut payload);
        match socket.send(&payload) {
            Ok(_) => {
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
