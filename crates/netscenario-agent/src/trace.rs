//! A trace of packets: when each leaves and how large it is (WRK-03, D-085).
//!
//! The sender walks a trace like any other schedule: packet `n` leaves at the `n`th moment and carries the `n`th size. The
//! run writes the trace for the agent as a text file of one line per packet, `<offset in nanoseconds> <payload bytes>`, after
//! a first line that says what it is. The offsets only grow (packets may leave at the same moment; they are sent one after the
//! other).

use std::fmt::Write as _;

/// The first line of a trace file.
pub const FIRST_LINE: &str = "# netscenario trace v1: offset in nanoseconds, payload in bytes";

/// The most packets in a trace.
pub const MAX_PACKETS: usize = 100_000;

/// The moments and the sizes of the packets of a trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    moments_ns: Vec<u64>,
    sizes: Vec<u32>,
}

impl Trace {
    /// A trace of packets that leave at `moments_ns` (from the start, not falling) with the payloads `sizes`.
    ///
    /// # Errors
    ///
    /// A sentence if the lists differ in length, are empty or longer than [`MAX_PACKETS`], if a moment is before the one
    /// before it, or a size is zero.
    pub fn new(moments_ns: Vec<u64>, sizes: Vec<u32>) -> Result<Self, String> {
        if moments_ns.len() != sizes.len() {
            return Err(format!(
                "{} moments and {} sizes",
                moments_ns.len(),
                sizes.len()
            ));
        }
        if moments_ns.is_empty() {
            return Err("a trace has at least one packet".to_owned());
        }
        if moments_ns.len() > MAX_PACKETS {
            return Err(format!(
                "{} packets are more than the {MAX_PACKETS} of a trace",
                moments_ns.len()
            ));
        }
        if let Some(at) = moments_ns.windows(2).position(|pair| pair[1] < pair[0]) {
            return Err(format!("packet {} leaves before packet {at}", at + 1));
        }
        if let Some(at) = sizes.iter().position(|size| *size == 0) {
            return Err(format!("packet {at} has no payload"));
        }
        Ok(Self { moments_ns, sizes })
    }

    /// How many packets.
    #[must_use]
    pub fn len(&self) -> usize {
        self.moments_ns.len()
    }

    /// Whether there are no packets (never, for a trace that was made with [`Trace::new`]).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.moments_ns.is_empty()
    }

    /// The moments, in nanoseconds from the start.
    #[must_use]
    pub fn moments_ns(&self) -> &[u64] {
        &self.moments_ns
    }

    /// The payloads, in bytes.
    #[must_use]
    pub fn sizes(&self) -> &[u32] {
        &self.sizes
    }

    /// The largest payload.
    #[must_use]
    pub fn largest(&self) -> u64 {
        self.sizes.iter().copied().max().map_or(0, u64::from)
    }

    /// The smallest payload.
    #[must_use]
    pub fn smallest(&self) -> u64 {
        self.sizes.iter().copied().min().map_or(0, u64::from)
    }

    /// The payload of all the packets, in bits.
    #[must_use]
    pub fn total_bits(&self) -> u128 {
        self.sizes.iter().map(|size| u128::from(*size) * 8).sum()
    }

    /// The file for the agent.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = String::with_capacity(self.len() * 16 + FIRST_LINE.len() + 1);
        text.push_str(FIRST_LINE);
        text.push('\n');
        for (moment, size) in self.moments_ns.iter().zip(&self.sizes) {
            let _ = writeln!(text, "{moment} {size}");
        }
        text
    }

    /// Reads the file for the agent.
    ///
    /// # Errors
    ///
    /// A sentence with the line if something is not `<offset> <size>`, or the reason [`Trace::new`] gives.
    pub fn from_text(text: &str) -> Result<Self, String> {
        let mut moments = Vec::new();
        let mut sizes = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let number = index + 1;
            let (moment, size) = line
                .split_once(' ')
                .ok_or_else(|| format!("line {number}: '{line}' is not '<offset> <size>'"))?;
            moments.push(moment.parse::<u64>().map_err(|_| {
                format!("line {number}: '{moment}' is not a number of nanoseconds")
            })?);
            sizes.push(
                size.trim()
                    .parse::<u32>()
                    .map_err(|_| format!("line {number}: '{size}' is not a size in bytes"))?,
            );
            if moments.len() > MAX_PACKETS {
                return Err(format!("more than {MAX_PACKETS} packets"));
            }
        }
        Self::new(moments, sizes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trace_is_written_and_read_back() {
        let trace = Trace::new(
            vec![0, 1_500, 1_500, 9_000_000_000],
            vec![100, 1200, 20, 1472],
        )
        .unwrap();
        let text = trace.to_text();
        assert_eq!(text.lines().next(), Some(FIRST_LINE));
        assert_eq!(Trace::from_text(&text).unwrap(), trace);
        assert_eq!((trace.smallest(), trace.largest()), (20, 1472));
        assert_eq!(trace.total_bits(), (100 + 1200 + 20 + 1472) * 8);
    }

    /// The rules that make a trace a schedule. Each is a canary: the same trace without the fault is accepted.
    #[test]
    fn a_trace_that_is_not_a_schedule_is_refused_with_the_reason() {
        assert!(Trace::new(vec![0, 5], vec![10, 10]).is_ok());
        assert!(
            Trace::new(vec![0, 5], vec![10])
                .unwrap_err()
                .contains("2 moments and 1 sizes")
        );
        assert!(
            Trace::new(vec![], vec![])
                .unwrap_err()
                .contains("at least one")
        );
        assert_eq!(
            Trace::new(vec![0, 5, 4], vec![10, 10, 10]).unwrap_err(),
            "packet 2 leaves before packet 1"
        );
        assert_eq!(
            Trace::new(vec![0, 5], vec![10, 0]).unwrap_err(),
            "packet 1 has no payload"
        );
        let long = vec![0_u64; MAX_PACKETS + 1];
        assert!(
            Trace::new(long, vec![1; MAX_PACKETS + 1])
                .unwrap_err()
                .contains("more than")
        );
    }

    #[test]
    fn a_file_with_a_bad_line_names_the_line() {
        assert_eq!(
            Trace::from_text("# x\n0 100\n5\n").unwrap_err(),
            "line 3: '5' is not '<offset> <size>'"
        );
        assert!(
            Trace::from_text("0 100\n5 x\n")
                .unwrap_err()
                .contains("line 2: 'x' is not a size")
        );
        assert!(
            Trace::from_text("-1 100\n")
                .unwrap_err()
                .contains("not a number of nanoseconds")
        );
    }
}
