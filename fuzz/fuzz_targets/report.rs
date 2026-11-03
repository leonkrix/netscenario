//! The reports of a result file (D-076): whatever a result says, the Markdown, the JUnit and the HTML are made, never a
//! panic, the XML is closed, and no text of the file becomes markup in the page. The HTML is made without series and
//! without records (a result of an older version has none), which is what a file alone can give.
#![no_main]

use std::path::PathBuf;

use libfuzzer_sys::fuzz_target;
use netscenario_core::RunResult;
use netscenario_core::report::{Detail, Report, RunReport, html, junit, markdown};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(result) = RunResult::from_json(text) else {
        return;
    };
    let details = vec![Detail::default(); result.workloads.len()];
    let report = Report::Run(Box::new(RunReport {
        result,
        dir: PathBuf::new(),
        details,
    }));
    let _ = markdown(&report);
    let xml = junit(&report);
    assert_eq!(xml.matches("<testcase ").count(), xml.matches("</testcase>").count());
    assert_eq!(xml.matches("<properties>").count(), xml.matches("</properties>").count());
    let page = html(&report).expect("the HTML of a result with its details is made");
    assert!(!page.contains("<script"), "text became markup");
});
