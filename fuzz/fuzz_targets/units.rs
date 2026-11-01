//! The values of the format (durations, rates, sizes, probabilities, names) from any text: an answer, never a
//! panic, and a value that was accepted prints as text that reads back as the same value. The first byte
//! chooses the kind of value.
#![no_main]

use std::fmt::Display;
use std::str::FromStr;

use libfuzzer_sys::fuzz_target;
use netscenario_model::{Duration, Name, Probability, Rate, Size};

fn round_trip<T>(text: &str)
where
    T: FromStr + Display + PartialEq + std::fmt::Debug,
    T::Err: std::fmt::Debug,
{
    if let Ok(value) = text.parse::<T>() {
        let printed = value.to_string();
        let again = printed
            .parse::<T>()
            .unwrap_or_else(|error| panic!("'{text}' is {printed}, which is refused: {error:?}"));
        assert_eq!(again, value, "'{text}' printed as '{printed}'");
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((kind, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    match kind % 5 {
        0 => round_trip::<Duration>(text),
        1 => round_trip::<Rate>(text),
        2 => round_trip::<Size>(text),
        3 => round_trip::<Probability>(text),
        _ => round_trip::<Name>(text),
    }
});
