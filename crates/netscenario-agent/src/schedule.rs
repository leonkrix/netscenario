//! The moments at which the packets of a constant bit rate stream leave.
//!
//! Packet `n` leaves at `start + n * size * 8 / rate` seconds. The moment is computed from `n`, not by
//! adding an interval to the last moment, so a rate whose interval is not a whole number of nanoseconds
//! (3 mbit with 1000 byte packets: 2.666... ms) does not drift: after a million packets the stream is exactly
//! where the arithmetic puts it. The computation is in integers.

/// A constant bit rate stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    /// Payload bits per second.
    pub bits_per_second: u64,
    /// Payload bytes per packet.
    pub packet_size: u64,
}

impl Stream {
    /// How long after the start packet `n` leaves, in nanoseconds (rounded down).
    ///
    /// # Panics
    ///
    /// If the rate is zero; the model refuses such a rate, and the command line of the agent too.
    #[must_use]
    pub fn offset_ns(&self, n: u64) -> u64 {
        let bits = u128::from(n) * u128::from(self.packet_size) * 8;
        let ns = bits * 1_000_000_000 / u128::from(self.bits_per_second);
        u64::try_from(ns).unwrap_or(u64::MAX)
    }

    /// How many packets leave in the first `duration_ns` nanoseconds: those with an offset below it.
    #[must_use]
    pub fn packets_in(&self, duration_ns: u64) -> u64 {
        // The offset of packet n is below d exactly when n * size * 8 * 1e9 < d * rate, rounded down
        let numerator = u128::from(duration_ns) * u128::from(self.bits_per_second);
        let per_packet = u128::from(self.packet_size) * 8 * 1_000_000_000;
        let whole = numerator / per_packet;
        let count = if numerator % per_packet == 0 {
            whole
        } else {
            whole + 1
        };
        u64::try_from(count).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn stream(bits_per_second: u64, packet_size: u64) -> Stream {
        Stream {
            bits_per_second,
            packet_size,
        }
    }

    #[test]
    fn an_even_interval_is_exact() {
        // 2 mbit with 1200 byte packets: 9,600 bits every 4.8 ms
        let s = stream(2_000_000, 1200);
        assert_eq!(s.offset_ns(0), 0);
        assert_eq!(s.offset_ns(1), 4_800_000);
        assert_eq!(s.offset_ns(1_000), 4_800_000_000);
    }

    #[test]
    fn an_uneven_interval_does_not_drift() {
        // 3 mbit with 1000 byte packets: 8,000 bits every 2.666... ms. Three packets are exactly 8 ms.
        let s = stream(3_000_000, 1000);
        assert_eq!(s.offset_ns(1), 2_666_666);
        assert_eq!(s.offset_ns(3), 8_000_000);
        assert_eq!(s.offset_ns(3_000_000), 8_000_000_000_000);
    }

    #[test]
    fn the_count_matches_the_offsets() {
        for (rate, size, duration) in [
            (2_000_000, 1200, 1_000_000_000),
            (3_000_000, 1000, 1_000_000_000),
            (1_000_000, 1472, 123_456_789),
            (800_000_000, 1000, 10_000_000),
            (1_000_000, 16, 5_000_000),
        ] {
            let s = stream(rate, size);
            let count = s.packets_in(duration);
            if count > 0 {
                assert!(
                    s.offset_ns(count - 1) < duration,
                    "{rate} {size} {duration}"
                );
            }
            assert!(s.offset_ns(count) >= duration, "{rate} {size} {duration}");
        }
    }

    #[test]
    fn a_duration_of_zero_has_no_packets() {
        assert_eq!(stream(1_000_000, 1000).packets_in(0), 0);
    }

    #[test]
    fn the_rate_of_the_payload_is_what_arrives() {
        // 1 mbit, 1000 byte payload: 125 packets in one second
        assert_eq!(stream(1_000_000, 1000).packets_in(1_000_000_000), 125);
    }
}
