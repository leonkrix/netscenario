//! A file of records, as the supervisor reads it after a run (the receiver wrote it, but a file on disk can be
//! cut, damaged or swapped): read or refused, never a panic, and what was read is what writing it again
//! gives.
#![no_main]

use libfuzzer_sys::fuzz_target;
use netscenario_agent::records::{self, Writer};

fuzz_target!(|data: &[u8]| {
    if let Ok(file) = records::read(data) {
        let mut writer = Writer::new(Vec::new(), file.flow).expect("a vector can be written");
        for record in &file.records {
            writer.write(record).expect("a vector can be written");
        }
        let bytes = writer.finish().expect("a vector can be written");
        let again = records::read(&bytes[..]).expect("written out and refused");
        assert_eq!(again, file, "written out and read again, not the same");
    }
});
