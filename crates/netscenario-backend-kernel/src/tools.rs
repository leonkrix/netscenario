//! The tools that the backend calls, and their versions.

use std::fmt;

/// A version of the form `major.minor.patch`, ordered by its numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// The first number.
    pub major: u32,
    /// The second number.
    pub minor: u32,
    /// The third number (0 if the version has only two).
    pub patch: u32,
}

impl Version {
    /// A version from its three numbers.
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What is known about a tool on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    /// The tool is not installed, or not on the `PATH`.
    Missing,
    /// The tool runs, but its version could not be read. The text is what it printed.
    Unreadable(String),
    /// The tool runs and has this version.
    Found(Version),
}

/// Reads the version of iproute2 from the output of `tc -V`, for example `tc utility, iproute2-6.15.0, libbpf
/// 1.5.0`. Builds of a snapshot (`iproute2-ss200602`) have no version number: they give `None`.
#[must_use]
pub fn parse_tc_version(output: &str) -> Option<Version> {
    let rest = &output[output.find("iproute2-")? + "iproute2-".len()..];
    let mut numbers = [0_u32; 3];
    let mut count = 0;
    for part in rest.split('.') {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || count == 3 {
            break;
        }
        numbers[count] = digits.parse().ok()?;
        count += 1;
        // A suffix after the digits (`-1ubuntu2`, `,`) ends the version
        if digits.len() != part.len() {
            break;
        }
    }
    (count >= 2).then(|| Version::new(numbers[0], numbers[1], numbers[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_read_from_the_output_of_tc() {
        for (output, expected) in [
            (
                "tc utility, iproute2-6.15.0, libbpf 1.5.0",
                Version::new(6, 15, 0),
            ),
            (
                "tc utility, iproute2-5.15.0, libbpf 0.5.0\n",
                Version::new(5, 15, 0),
            ),
            ("tc utility, iproute2-6.1.0", Version::new(6, 1, 0)),
            ("tc utility, iproute2-6.6", Version::new(6, 6, 0)),
            (
                "tc utility, iproute2-7.3.0-12-gd1341d07",
                Version::new(7, 3, 0),
            ),
            (
                "tc utility, iproute2-5.15.0-1ubuntu2",
                Version::new(5, 15, 0),
            ),
        ] {
            assert_eq!(parse_tc_version(output), Some(expected), "{output:?}");
        }
    }

    #[test]
    fn output_without_a_version_gives_none() {
        for output in [
            "",
            "tc utility, iproute2-ss200602",
            "BusyBox v1.36 multi-call binary",
            "tc utility, iproute2-",
            "tc utility, iproute2-6",
        ] {
            assert_eq!(parse_tc_version(output), None, "{output:?}");
        }
    }

    #[test]
    fn versions_are_ordered_by_their_numbers() {
        assert!(Version::new(6, 15, 0) > Version::new(6, 6, 0));
        assert!(Version::new(6, 1, 0) < Version::new(6, 6, 0));
        assert!(Version::new(10, 0, 0) > Version::new(9, 99, 99));
    }
}
