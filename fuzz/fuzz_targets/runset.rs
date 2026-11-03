//! The file of a set of runs (`runset.json`, D-074), which `report` and the Python layer read: read or refused, never a
//! panic, a set that was read can be written and read again, and both reports that need no other file can be made of it
//! and have what a reader of them relies on: the XML is closed and no text becomes markup.
#![no_main]

use std::path::PathBuf;

use libfuzzer_sys::fuzz_target;
use netscenario_core::RunSetResult;
use netscenario_core::report::{Report, html, junit, markdown};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(set) = RunSetResult::from_json(text) else {
        return;
    };
    let written = set.to_json();
    RunSetResult::from_json(&written)
        .unwrap_or_else(|error| panic!("written out and refused: {error}\n{written}"));
    let report = Report::Set(Box::new(set), PathBuf::new());
    let _ = markdown(&report);
    let xml = junit(&report);
    assert_eq!(xml.matches("<testcase ").count(), xml.matches("</testcase>").count());
    let page = html(&report).expect("the HTML of a set needs no other file");
    assert!(!page.contains("<script"), "text became markup");
});
