//! The clock of the agent: `CLOCK_MONOTONIC` in nanoseconds.
//!
//! Every node of a run is a network namespace on one machine, so they all see the same monotonic clock. The
//! send time that the sender writes into a packet and the arrival time that the receiver reads are therefore
//! comparable, and the difference is the one-way delay. The clock does not jump when the time is adjusted
//! (unlike the wall clock), which is why it is used. It counts from an arbitrary point (usually the boot), so
//! a time means something only next to another time from the same machine.

use rustix::time::{ClockId, clock_gettime};

/// Nanoseconds on `CLOCK_MONOTONIC`.
#[must_use]
pub fn monotonic_ns() -> u64 {
    let now = clock_gettime(ClockId::Monotonic);
    // The clock is never negative; a negative value would be a broken kernel, and 0 is as good as any answer
    u64::try_from(now.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::try_from(now.tv_nsec).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_never_goes_back() {
        let mut last = monotonic_ns();
        assert!(last > 0);
        for _ in 0..10_000 {
            let now = monotonic_ns();
            assert!(now >= last);
            last = now;
        }
    }

    #[test]
    fn the_clock_counts_real_time() {
        let before = monotonic_ns();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let passed = monotonic_ns() - before;
        assert!(
            (20_000_000..2_000_000_000).contains(&passed),
            "{passed} ns passed in a sleep of 20 ms"
        );
    }
}
