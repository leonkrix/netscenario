//! A file of stream records, as the supervisor reads it after a run of TCP agents (they wrote it, but a file on disk can be cut,
//! damaged or swapped), and the hello that opens a connection of the TCP server (bytes from the network): read or refused, never a
//! panic, and what was read is what writing it again gives.
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_agent::streams::{self, Writer};
use netscenario_agent::tcp::{HELLO_LEN, Hello};

fuzz_target!(|data: &[u8]| {
    if let Ok(file) = streams::read(data) {
        let mut writer = Writer::new(Vec::new(), file.flow).expect("a vector can be written");
        for record in &file.records {
            writer.write(record).expect("a vector can be written");
        }
        let bytes = writer.finish().expect("a vector can be written");
        let again = streams::read(&bytes[..]).expect("written out and refused");
        assert_eq!(again, file, "written out and read again, not the same");
    }
    if let Some(bytes) = data.first_chunk::<HELLO_LEN>() {
        if let Ok(hello) = Hello::from_bytes(bytes) {
            assert_eq!(&hello.to_bytes(), bytes, "a hello that was read is written as it was");
        }
    }
});
