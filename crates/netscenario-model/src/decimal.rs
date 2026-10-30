//! Exact decimal numbers for the values of the scenario format.
//!
//! The format needs "a whole number of the smallest unit" (`0.5ns` is an error, `2.5s` is 2,500,000,000 ns),
//! and floating point cannot decide that. A number is therefore read as digits and a count of fractional
//! digits and converted with integer arithmetic.

/// A non-negative decimal number: `digits / 10^scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decimal {
    digits: u128,
    scale: u32,
}

/// The number is too long to be a value of this format.
const MAX_DIGITS: usize = 30;

/// Why a text could not be split into a number and a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitError {
    /// There is no digit at the start.
    NotANumber,
    /// The number has more digits than the format allows.
    TooLong,
    /// A space or another character that cannot be part of a number with a unit.
    BadCharacter(char),
}

/// Splits `text` into the number at its start and the unit after it. The unit may be empty.
pub(crate) fn split(text: &str) -> Result<(Decimal, &str), SplitError> {
    if let Some(bad) = text
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '%')))
    {
        return Err(SplitError::BadCharacter(bad));
    }
    let integer_len = text.bytes().take_while(u8::is_ascii_digit).count();
    if integer_len == 0 {
        return Err(SplitError::NotANumber);
    }
    let mut end = integer_len;
    let mut scale = 0u32;
    let rest = &text[integer_len..];
    if let Some(after_point) = rest.strip_prefix('.') {
        let fraction_len = after_point.bytes().take_while(u8::is_ascii_digit).count();
        if fraction_len == 0 {
            return Err(SplitError::NotANumber);
        }
        end += 1 + fraction_len;
        scale = u32::try_from(fraction_len).map_err(|_| SplitError::TooLong)?;
    }
    let number = &text[..end];
    let digit_text: String = number.chars().filter(char::is_ascii_digit).collect();
    if digit_text.len() > MAX_DIGITS {
        return Err(SplitError::TooLong);
    }
    let digits = digit_text
        .parse::<u128>()
        .map_err(|_| SplitError::TooLong)?;
    Ok((Decimal { digits, scale }, &text[end..]))
}

impl Decimal {
    /// `self * factor` as a whole number, or `None` if it is not whole or does not fit into 64 bits.
    pub(crate) fn times_whole(self, factor: u64) -> Result<u64, NotWhole> {
        let product = self
            .digits
            .checked_mul(u128::from(factor))
            .ok_or(NotWhole::TooLarge)?;
        let divisor = 10u128.checked_pow(self.scale).ok_or(NotWhole::TooLarge)?;
        if product % divisor != 0 {
            return Err(NotWhole::Fraction);
        }
        u64::try_from(product / divisor).map_err(|_| NotWhole::TooLarge)
    }
}

/// Why a product is not a value of the format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotWhole {
    /// It has a fractional part in the smallest unit.
    Fraction,
    /// It does not fit.
    TooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_number_and_its_unit() {
        let (n, unit) = split("2.5s").unwrap();
        assert_eq!(unit, "s");
        assert_eq!(n.times_whole(1_000_000_000), Ok(2_500_000_000));
    }

    #[test]
    fn a_fraction_of_the_smallest_unit_is_not_whole() {
        let (n, _) = split("0.5ns").unwrap();
        assert_eq!(n.times_whole(1), Err(NotWhole::Fraction));
    }

    #[test]
    fn trailing_zeros_of_the_fraction_are_still_whole() {
        let (n, _) = split("1.000000000ns").unwrap();
        assert_eq!(n.times_whole(1), Ok(1));
    }

    #[test]
    fn overflow_is_reported_and_not_wrapped() {
        let (n, _) = split("99999999999999999999ns").unwrap();
        assert_eq!(n.times_whole(1), Err(NotWhole::TooLarge));
    }

    #[test]
    fn rejects_what_is_not_a_plain_number() {
        assert_eq!(split("ms"), Err(SplitError::NotANumber));
        assert_eq!(split(".5s"), Err(SplitError::NotANumber));
        assert_eq!(split("5.s"), Err(SplitError::NotANumber));
        assert_eq!(split("5 s"), Err(SplitError::BadCharacter(' ')));
        assert_eq!(split("-5s"), Err(SplitError::BadCharacter('-')));
        assert_eq!(split("1_000s"), Err(SplitError::BadCharacter('_')));
        assert_eq!(
            split(&format!("{}s", "9".repeat(31))),
            Err(SplitError::TooLong)
        );
    }
}
