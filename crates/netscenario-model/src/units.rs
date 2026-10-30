//! The values of the scenario format: durations, rates, sizes, probabilities and names.
//!
//! Every value is a string with a number and a unit, read strictly (see `docs/SCENARIO-FORMAT.md`, "Values"):
//! the unit is lower case and follows the number directly, the number is a whole number of the smallest unit
//! of its kind, and a probability always has a percent sign. A value prints in a canonical form that reads back
//! as the same value, so a scenario that is written out and read again does not change.

use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::decimal::{self, NotWhole, SplitError};

/// Why a text is not a value of the format. The messages say what was found, what is expected, and give an
/// example.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ValueError {
    /// There is no number at the start of the text.
    #[error("'{text}' is not a {what}: write a number and a unit, for example {example}")]
    NotANumber {
        /// The text that was read.
        text: String,
        /// What was expected, such as "duration".
        what: &'static str,
        /// An example of a valid value.
        example: &'static str,
    },
    /// The text has a character that cannot be part of a value, such as a space.
    #[error(
        "'{text}' is not a {what}: it contains '{found}'; write the number and the unit together, for example {example}"
    )]
    BadCharacter {
        /// The text that was read.
        text: String,
        /// What was expected.
        what: &'static str,
        /// The offending character.
        found: char,
        /// An example of a valid value.
        example: &'static str,
    },
    /// The number has no unit.
    #[error("'{text}' has no unit (use {units})")]
    NoUnit {
        /// The text that was read.
        text: String,
        /// The units that are accepted, as a list.
        units: &'static str,
    },
    /// The unit does not exist.
    #[error("unknown unit '{unit}' in '{text}' (use {units})")]
    UnknownUnit {
        /// The text that was read.
        text: String,
        /// The unit that was found.
        unit: String,
        /// The units that are accepted, as a list.
        units: &'static str,
    },
    /// The value is not a whole number of the smallest unit.
    #[error("'{text}' is not a whole number of {smallest}")]
    NotWhole {
        /// The text that was read.
        text: String,
        /// The smallest unit of the kind, such as "nanoseconds".
        smallest: &'static str,
    },
    /// The value does not fit.
    #[error("'{text}' is too large")]
    TooLarge {
        /// The text that was read.
        text: String,
    },
    /// A rate of zero.
    #[error("'{text}' is zero: a rate must be greater than zero")]
    Zero {
        /// The text that was read.
        text: String,
    },
    /// A probability without a percent sign.
    #[error("'{text}' has no percent sign: write probabilities as percentages, for example 2%")]
    NoPercentSign {
        /// The text that was read.
        text: String,
    },
    /// A probability above 100 %.
    #[error("'{text}' is more than 100%")]
    AboveHundredPercent {
        /// The text that was read.
        text: String,
    },
    /// A name that does not follow the rules.
    #[error(
        "'{text}' is not a valid name: {reason} (a name has 1 to 32 characters: lower case letters, digits and '-', starting with a letter)"
    )]
    BadName {
        /// The text that was read.
        text: String,
        /// What is wrong with it.
        reason: String,
    },
}

/// The table of a kind of value: the units with their factor to the smallest unit.
struct Kind {
    what: &'static str,
    example: &'static str,
    smallest: &'static str,
    /// The accepted units for messages, such as "ns, us, ms, s, m or h".
    list: &'static str,
    /// Units and their factor, in the order from large to small (used for printing).
    units: &'static [(&'static str, u64)],
}

const DURATION: Kind = Kind {
    what: "duration",
    example: "40ms",
    smallest: "nanoseconds",
    list: "ns, us, ms, s, m or h",
    units: &[
        ("h", 3_600_000_000_000),
        ("m", 60_000_000_000),
        ("s", 1_000_000_000),
        ("ms", 1_000_000),
        ("us", 1_000),
        ("ns", 1),
    ],
};

const RATE: Kind = Kind {
    what: "rate",
    example: "10mbit",
    smallest: "bits per second",
    list: "bit, kbit, mbit, gbit or tbit",
    units: &[
        ("tbit", 1_000_000_000_000),
        ("gbit", 1_000_000_000),
        ("mbit", 1_000_000),
        ("kbit", 1_000),
        ("bit", 1),
    ],
};

const SIZE: Kind = Kind {
    what: "size",
    example: "1200b",
    smallest: "bytes",
    list: "b, kb, mb, gb, kib, mib or gib",
    units: &[
        ("gib", 1_073_741_824),
        ("gb", 1_000_000_000),
        ("mib", 1_048_576),
        ("mb", 1_000_000),
        ("kib", 1_024),
        ("kb", 1_000),
        ("b", 1),
    ],
};

/// Reads `text` as a number and one of the units of `kind` and returns the value in the smallest unit.
fn parse(text: &str, kind: &Kind) -> Result<u64, ValueError> {
    let owned = || text.to_owned();
    let (number, unit) = decimal::split(text).map_err(|error| match error {
        SplitError::NotANumber => ValueError::NotANumber {
            text: owned(),
            what: kind.what,
            example: kind.example,
        },
        SplitError::TooLong => ValueError::TooLarge { text: owned() },
        SplitError::BadCharacter(found) => ValueError::BadCharacter {
            text: owned(),
            what: kind.what,
            found,
            example: kind.example,
        },
    })?;
    if unit.is_empty() {
        return Err(ValueError::NoUnit {
            text: owned(),
            units: kind.list,
        });
    }
    let Some((_, factor)) = kind.units.iter().find(|(name, _)| *name == unit) else {
        return Err(ValueError::UnknownUnit {
            text: owned(),
            unit: unit.to_owned(),
            units: kind.list,
        });
    };
    number.times_whole(*factor).map_err(|error| match error {
        NotWhole::Fraction => ValueError::NotWhole {
            text: owned(),
            smallest: kind.smallest,
        },
        NotWhole::TooLarge => ValueError::TooLarge { text: owned() },
    })
}

/// Prints `value` (in the smallest unit) with the largest unit that divides it exactly.
fn format(value: u64, kind: &Kind) -> String {
    if value == 0 {
        let (smallest, _) = kind.units[kind.units.len() - 1];
        return format!("0{smallest}");
    }
    for (name, factor) in kind.units {
        if value.is_multiple_of(*factor) {
            return format!("{}{name}", value / factor);
        }
    }
    unreachable!("the smallest unit has factor 1 and divides every value")
}

/// Reads a string value, and says "has no unit" for a bare number instead of "invalid type".
fn deserialize_value<'de, D, T>(
    deserializer: D,
    expecting: &'static str,
    parse: impl Fn(&str) -> Result<T, ValueError>,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
{
    struct ValueVisitor<F> {
        expecting: &'static str,
        parse: F,
    }
    impl<T, F: Fn(&str) -> Result<T, ValueError>> Visitor<'_> for ValueVisitor<F> {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.expecting)
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<T, E> {
            (self.parse)(text).map_err(E::custom)
        }

        // A bare number is read as a number by YAML. Parsing its text gives the useful message ("has no unit").
        fn visit_u64<E: de::Error>(self, number: u64) -> Result<T, E> {
            (self.parse)(&number.to_string()).map_err(E::custom)
        }

        fn visit_i64<E: de::Error>(self, number: i64) -> Result<T, E> {
            (self.parse)(&number.to_string()).map_err(E::custom)
        }

        fn visit_f64<E: de::Error>(self, number: f64) -> Result<T, E> {
            (self.parse)(&number.to_string()).map_err(E::custom)
        }
    }
    deserializer.deserialize_any(ValueVisitor { expecting, parse })
}

macro_rules! value_impls {
    ($type:ident, $expecting:expr, $pattern:expr, $description:expr, $example:expr) => {
        impl fmt::Display for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.canonical())
            }
        }

        impl FromStr for $type {
            type Err = ValueError;

            fn from_str(text: &str) -> Result<Self, ValueError> {
                Self::parse(text)
            }
        }

        impl Serialize for $type {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.canonical())
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserialize_value(deserializer, $expecting, Self::parse)
            }
        }

        impl JsonSchema for $type {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($type).into()
            }

            fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "pattern": $pattern,
                    "description": $description,
                    "examples": [$example]
                })
            }
        }
    };
}

/// A length of time, a whole number of nanoseconds. Written `40ms`, `2.5s`, `1m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration(u64);

impl Duration {
    /// A duration of `nanoseconds`.
    #[must_use]
    pub const fn from_nanos(nanoseconds: u64) -> Self {
        Self(nanoseconds)
    }

    /// A duration of `milliseconds`, or `None` if that does not fit.
    #[must_use]
    pub const fn from_millis(milliseconds: u64) -> Option<Self> {
        match milliseconds.checked_mul(1_000_000) {
            Some(nanoseconds) => Some(Self(nanoseconds)),
            None => None,
        }
    }

    /// A duration of `seconds`, or `None` if that does not fit.
    #[must_use]
    pub const fn from_secs(seconds: u64) -> Option<Self> {
        match seconds.checked_mul(1_000_000_000) {
            Some(nanoseconds) => Some(Self(nanoseconds)),
            None => None,
        }
    }

    /// The duration in nanoseconds.
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Whether the duration is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    fn parse(text: &str) -> Result<Self, ValueError> {
        parse(text, &DURATION).map(Self)
    }

    fn canonical(self) -> String {
        format(self.0, &DURATION)
    }
}

impl From<Duration> for std::time::Duration {
    fn from(duration: Duration) -> Self {
        Self::from_nanos(duration.0)
    }
}

value_impls!(
    Duration,
    "a duration with a unit, such as 40ms",
    r"^[0-9]+(\.[0-9]+)?(ns|us|ms|s|m|h)$",
    "A length of time: a number and a unit (ns, us, ms, s, m, h), a whole number of nanoseconds.",
    "40ms"
);

/// A data rate in bits per second, greater than zero. Written `10mbit`, `500kbit`; decimal, so `kbit` is 1,000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rate(u64);

impl Rate {
    /// A rate of `bits` per second, or `None` if that is zero.
    #[must_use]
    pub const fn from_bits_per_second(bits: u64) -> Option<Self> {
        if bits == 0 { None } else { Some(Self(bits)) }
    }

    /// The rate in bits per second.
    #[must_use]
    pub const fn bits_per_second(self) -> u64 {
        self.0
    }

    fn parse(text: &str) -> Result<Self, ValueError> {
        let bits = parse(text, &RATE)?;
        Self::from_bits_per_second(bits).ok_or_else(|| ValueError::Zero {
            text: text.to_owned(),
        })
    }

    fn canonical(self) -> String {
        format(self.0, &RATE)
    }
}

value_impls!(
    Rate,
    "a rate with a unit, such as 10mbit",
    r"^[0-9]+(\.[0-9]+)?(bit|kbit|mbit|gbit|tbit)$",
    "A data rate in bits per second, greater than zero: a number and a decimal unit (bit, kbit, mbit, gbit, tbit).",
    "10mbit"
);

/// An amount of data in bytes. Written `1200b`, `64kb` (1,000 bytes per `kb`) or `1mib` (1,048,576 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Size(u64);

impl Size {
    /// A size of `bytes`.
    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The size in bytes.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0
    }

    fn parse(text: &str) -> Result<Self, ValueError> {
        parse(text, &SIZE).map(Self)
    }

    fn canonical(self) -> String {
        format(self.0, &SIZE)
    }
}

value_impls!(
    Size,
    "a size with a unit, such as 1200b",
    r"^[0-9]+(\.[0-9]+)?(b|kb|mb|gb|kib|mib|gib)$",
    "An amount of data in bytes: a number and a unit (b, kb, mb, gb decimal; kib, mib, gib binary), a whole number of bytes.",
    "1200b"
);

/// A probability from `0%` to `100%` with at most six digits after the point. Written `2%`, `0.1%`.
///
/// It is stored in millionths of a percent, so every accepted value is exact. A number without the percent
/// sign is refused: `1` could mean 1 % or 100 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Probability(u32);

/// 100 % in millionths of a percent.
const HUNDRED_PERCENT: u32 = 100_000_000;

impl Probability {
    /// A probability of `millionths` of a percent (100,000,000 is 100 %), or `None` if that is above 100 %.
    #[must_use]
    pub const fn from_micro_percent(millionths: u32) -> Option<Self> {
        if millionths > HUNDRED_PERCENT {
            None
        } else {
            Some(Self(millionths))
        }
    }

    /// The probability in millionths of a percent.
    #[must_use]
    pub const fn micro_percent(self) -> u32 {
        self.0
    }

    /// The probability as a fraction from 0 to 1 (rounded to the nearest floating point number).
    #[must_use]
    pub fn as_fraction(self) -> f64 {
        f64::from(self.0) / f64::from(HUNDRED_PERCENT)
    }

    /// Whether the probability is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    fn parse(text: &str) -> Result<Self, ValueError> {
        let owned = || text.to_owned();
        let (number, unit) = decimal::split(text).map_err(|error| match error {
            SplitError::NotANumber => ValueError::NotANumber {
                text: owned(),
                what: "probability",
                example: "2%",
            },
            SplitError::TooLong => ValueError::TooLarge { text: owned() },
            SplitError::BadCharacter(found) => ValueError::BadCharacter {
                text: owned(),
                what: "probability",
                found,
                example: "2%",
            },
        })?;
        if unit.is_empty() {
            return Err(ValueError::NoPercentSign { text: owned() });
        }
        if unit != "%" {
            return Err(ValueError::UnknownUnit {
                text: owned(),
                unit: unit.to_owned(),
                units: "%",
            });
        }
        let millionths = number.times_whole(1_000_000).map_err(|error| match error {
            NotWhole::Fraction => ValueError::NotWhole {
                text: owned(),
                smallest: "millionths of a percent",
            },
            NotWhole::TooLarge => ValueError::AboveHundredPercent { text: owned() },
        })?;
        u32::try_from(millionths)
            .ok()
            .and_then(Self::from_micro_percent)
            .ok_or_else(|| ValueError::AboveHundredPercent { text: owned() })
    }

    fn canonical(self) -> String {
        let whole = self.0 / 1_000_000;
        let fraction = self.0 % 1_000_000;
        if fraction == 0 {
            format!("{whole}%")
        } else {
            let digits = format!("{fraction:06}");
            format!("{whole}.{}%", digits.trim_end_matches('0'))
        }
    }
}

value_impls!(
    Probability,
    "a probability with a percent sign, such as 2%",
    r"^[0-9]+(\.[0-9]+)?%$",
    "A probability from 0% to 100% with at most six digits after the point. Always with the percent sign.",
    "2%"
);

/// The name of a node, a link or a workload: 1 to 32 characters, lower case letters, digits and `-`, starting
/// with a letter and not ending with `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

impl Name {
    /// Checks `text` and makes a name of it.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::BadName`] with the reason if `text` does not follow the rules.
    pub fn new(text: &str) -> Result<Self, ValueError> {
        Self::parse(text)
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(text: &str) -> Result<Self, ValueError> {
        let bad = |reason: String| ValueError::BadName {
            text: text.to_owned(),
            reason,
        };
        if text.is_empty() {
            return Err(bad("it is empty".to_owned()));
        }
        if text.chars().count() > 32 {
            return Err(bad("it is longer than 32 characters".to_owned()));
        }
        if let Some(found) = text
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
        {
            return Err(bad(format!("it contains '{found}'")));
        }
        if !text.starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(bad("it does not start with a letter".to_owned()));
        }
        if text.ends_with('-') {
            return Err(bad("it ends with '-'".to_owned()));
        }
        Ok(Self(text.to_owned()))
    }

    fn canonical(&self) -> String {
        self.0.clone()
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

value_impls!(
    Name,
    "a name: lower case letters, digits and '-'",
    r"^[a-z]([a-z0-9-]{0,30}[a-z0-9])?$",
    "A name of 1 to 32 characters: lower case letters, digits and '-', starting with a letter and not ending with '-'.",
    "client"
);

#[cfg(test)]
mod tests {
    use super::*;

    fn duration(text: &str) -> Result<Duration, ValueError> {
        text.parse()
    }

    #[test]
    fn durations_are_exact_and_print_in_the_largest_exact_unit() {
        assert_eq!(duration("40ms").unwrap().as_nanos(), 40_000_000);
        assert_eq!(duration("2.5s").unwrap().as_nanos(), 2_500_000_000);
        assert_eq!(duration("1m").unwrap().to_string(), "1m");
        assert_eq!(duration("90s").unwrap().to_string(), "90s");
        assert_eq!(duration("2.5s").unwrap().to_string(), "2500ms");
        assert_eq!(duration("0ms").unwrap().to_string(), "0ns");
    }

    #[test]
    fn a_duration_that_is_not_whole_or_not_clear_is_refused() {
        assert_eq!(
            duration("0.5ns").unwrap_err().to_string(),
            "'0.5ns' is not a whole number of nanoseconds"
        );
        assert_eq!(
            duration("40").unwrap_err().to_string(),
            "'40' has no unit (use ns, us, ms, s, m or h)"
        );
        assert_eq!(
            duration("40min").unwrap_err().to_string(),
            "unknown unit 'min' in '40min' (use ns, us, ms, s, m or h)"
        );
        assert_eq!(
            duration("40MS").unwrap_err().to_string(),
            "unknown unit 'MS' in '40MS' (use ns, us, ms, s, m or h)"
        );
        assert!(
            duration("40 ms")
                .unwrap_err()
                .to_string()
                .contains("contains ' '")
        );
        assert!(
            duration("ms")
                .unwrap_err()
                .to_string()
                .contains("not a duration")
        );
        assert!(duration("1e3ms").is_err());
    }

    #[test]
    fn rates_are_decimal_and_never_zero() {
        assert_eq!(
            "10mbit".parse::<Rate>().unwrap().bits_per_second(),
            10_000_000
        );
        assert_eq!("1.5kbit".parse::<Rate>().unwrap().bits_per_second(), 1_500);
        assert_eq!(
            "0mbit".parse::<Rate>().unwrap_err().to_string(),
            "'0mbit' is zero: a rate must be greater than zero"
        );
        assert!("10Mbit".parse::<Rate>().is_err());
        assert!("10bps".parse::<Rate>().is_err());
        assert!("0.5bit".parse::<Rate>().is_err());
    }

    #[test]
    fn sizes_distinguish_decimal_and_binary_units() {
        assert_eq!("1kb".parse::<Size>().unwrap().bytes(), 1_000);
        assert_eq!("1kib".parse::<Size>().unwrap().bytes(), 1_024);
        assert_eq!("1.5kib".parse::<Size>().unwrap().bytes(), 1_536);
        assert_eq!(Size::from_bytes(1_048_576).to_string(), "1mib");
        assert_eq!(Size::from_bytes(1_000_000).to_string(), "1mb");
        assert_eq!(Size::from_bytes(1_200).to_string(), "1200b");
    }

    #[test]
    fn probabilities_need_the_percent_sign_and_stay_within_range() {
        assert_eq!(
            "2%".parse::<Probability>().unwrap().micro_percent(),
            2_000_000
        );
        assert_eq!(
            "0.1%".parse::<Probability>().unwrap().micro_percent(),
            100_000
        );
        assert_eq!(
            "100%".parse::<Probability>().unwrap().micro_percent(),
            100_000_000
        );
        assert_eq!(
            "0.02".parse::<Probability>().unwrap_err().to_string(),
            "'0.02' has no percent sign: write probabilities as percentages, for example 2%"
        );
        assert_eq!(
            "1".parse::<Probability>().unwrap_err().to_string(),
            "'1' has no percent sign: write probabilities as percentages, for example 2%"
        );
        assert_eq!(
            "100.000001%"
                .parse::<Probability>()
                .unwrap_err()
                .to_string(),
            "'100.000001%' is more than 100%"
        );
        assert!("0.0000001%".parse::<Probability>().is_err());
        assert_eq!(
            Probability::from_micro_percent(100_000)
                .unwrap()
                .to_string(),
            "0.1%"
        );
        assert_eq!(
            Probability::from_micro_percent(12_345_000)
                .unwrap()
                .to_string(),
            "12.345%"
        );
    }

    #[test]
    fn names_follow_the_rules() {
        assert!(Name::new("client").is_ok());
        assert!(Name::new("a").is_ok());
        assert!(Name::new("access-1").is_ok());
        assert!(Name::new(&"a".repeat(32)).is_ok());
        for bad in ["", "Client", "1node", "-a", "a-", "a_b", "a b", "ä"] {
            assert!(Name::new(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(
            Name::new(&"a".repeat(33))
                .unwrap_err()
                .to_string()
                .contains("longer than 32")
        );
        assert!(
            Name::new("Client")
                .unwrap_err()
                .to_string()
                .contains("contains 'C'")
        );
    }

    // Canary: a check that cannot fail proves nothing. A wrong value must be noticed.
    #[test]
    #[should_panic(expected = "assertion")]
    fn canary_a_wrong_value_is_detected() {
        assert_eq!(
            duration("40ms").unwrap().as_nanos(),
            40_000,
            "assertion: this must panic"
        );
    }
}
