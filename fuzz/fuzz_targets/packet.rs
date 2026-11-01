//! A datagram that reaches the port of a receiver can be anything, from anybody who can send to it. The header
//! is read or refused, never a panic, and a header that was read is the one that is written back.
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_agent::wire::{HEADER_LEN, Header};

fuzz_target!(|data: &[u8]| {
    if let Ok(header) = Header::read(data) {
        let mut packet = vec![0_u8; HEADER_LEN];
        header.write(&mut packet);
        assert_eq!(Header::read(&packet), Ok(header));
        // What the receiver compares: the bytes of the header are those it read
        assert_eq!(packet[..], data[..HEADER_LEN]);
    }
});
