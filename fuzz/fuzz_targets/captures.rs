//! A capture (`pcap` or `pcapng`) that a scenario names as a trace (WRK-03, D-085) can be anything. It is read or refused with a
//! sentence, never a panic, and a trace made of it has what the format promises: packets in the order of their moments, the first
//! at 0, none smaller than asked for, and a form that is written and read back to the same packets.
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_model::{FlowSelection, MAX_TRACE_PACKETS, TraceData, read_capture};

fuzz_target!(|data: &[u8]| {
    let select = FlowSelection::default();
    let Ok(read) = read_capture(data, &select, 16) else {
        return;
    };
    let packets = read.data;
    assert!((1..=MAX_TRACE_PACKETS).contains(&packets.len()));
    assert_eq!(packets.moments_ns()[0], 0);
    assert!(packets.moments_ns().windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(packets.smallest() >= 16, "a packet of {} bytes", packets.smallest());
    assert!(usize::try_from(read.padded).is_ok_and(|padded| padded <= packets.len()));
    let json = serde_json::to_string(&packets).expect("a trace can be written");
    let back: TraceData = serde_json::from_str(&json).expect("a trace reads back");
    assert_eq!(back, packets);
});
