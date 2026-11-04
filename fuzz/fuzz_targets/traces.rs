//! The CSV file of a trace (WRK-03, D-085) can be anything from anybody who writes a scenario. It is read or refused with a
//! sentence, never a panic, and a trace that was read has what the format promises: packets in the order of their moments, no
//! packet smaller than asked for, and a form that is written and read back to the same packets.
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_model::{MAX_TRACE_PACKETS, TraceData, read_csv};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // The smallest payload of a stream with the checksum and of one without
    for smallest in [16, 20] {
        let Ok(read) = read_csv(text, smallest) else {
            continue;
        };
        let packets = read.data;
        assert!((1..=MAX_TRACE_PACKETS).contains(&packets.len()));
        assert!(packets.moments_ns().windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(packets.smallest() >= smallest, "a packet of {} bytes", packets.smallest());
        let json = serde_json::to_string(&packets).expect("a trace can be written");
        let back: TraceData = serde_json::from_str(&json).expect("a trace reads back");
        assert_eq!(back, packets);
    }
});
