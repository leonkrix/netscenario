//! A file of delay samples that a scenario refers to (LNK-02, D-084) can be anything from anybody who writes a scenario. It is
//! read or refused with a sentence, never a panic, and a table that is made of it has what the format promises: 4,096 entries
//! in rising order, a scale between 1 and 16 (4,096 entries of equal chance cannot hold a value further than 64 deviations),
//! and a form that reads back to the same entries.
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_model::{MAX_SAMPLES, TABLE_ENTRIES, TableValues, parse_samples};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(samples) = parse_samples(text) else {
        return;
    };
    assert!(samples.len() <= MAX_SAMPLES, "{} samples", samples.len());
    assert!(samples.iter().all(|sample| sample.is_finite()));
    let Ok((table, scale)) = TableValues::from_samples(&samples) else {
        return;
    };
    assert_eq!(table.entries().len(), TABLE_ENTRIES);
    assert!(
        table.entries().windows(2).all(|pair| pair[0] <= pair[1]),
        "the entries are not in rising order"
    );
    assert!((1000..=16_100).contains(&scale), "a scale of {scale}");
    let json = serde_json::to_string(&table).expect("a table can be written");
    let back: TableValues = serde_json::from_str(&json).expect("a table reads back");
    assert_eq!(back, table);
    let file = table.to_dist_file("fuzz");
    assert_eq!(TableValues::from_dist_file(&file).expect("the file reads back"), table);
});
