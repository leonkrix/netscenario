//! The declaration of the Kernel backend is compared with the Kernel column of `docs/COVERAGE.md`: the page and
//! the code must not disagree about what is faithful.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use netscenario_backend_kernel::{Environment, capabilities};
use netscenario_core::{Feature, Support};

const COVERAGE: &str = include_str!("../../../docs/COVERAGE.md");

/// The cell of the Kernel column in the row called `label`, if the page has exactly one such row.
fn kernel_cell(markdown: &str, label: &str) -> Option<String> {
    let mut cells = markdown.lines().filter_map(|line| {
        let mut parts = line.strip_prefix('|')?.split('|').map(str::trim);
        (parts.next()? == label).then(|| parts.next().map(str::to_owned))?
    });
    let first = cells.next()?;
    cells.next().is_none().then_some(first)
}

#[test]
fn every_feature_has_one_row_in_the_page() {
    for feature in Feature::ALL {
        assert!(
            kernel_cell(COVERAGE, feature.label()).is_some(),
            "no single row '{}' in docs/COVERAGE.md",
            feature.label()
        );
    }
}

#[test]
fn what_is_exact_here_is_a_check_mark_there() {
    let capabilities = capabilities(&Environment::complete());
    for feature in Feature::ALL {
        let cell = kernel_cell(COVERAGE, feature.label()).expect("a row");
        match capabilities.support(*feature) {
            // 0.1 or 0.2: the release in which the Kernel backend got it
            Support::Exact => assert!(
                cell.starts_with("✔ 0.1") || cell.starts_with("✔ 0.2"),
                "'{}' is exact in the code, but the page says '{cell}'",
                feature.label()
            ),
            Support::Limited { .. } => assert!(
                cell.starts_with('~'),
                "'{}' is limited in the code, but the page says '{cell}'",
                feature.label()
            ),
            // Planned: the format has it, the backend gets it in release 0.2
            Support::Unsupported { reason, .. } => assert!(
                cell.starts_with("✔ 0.2") && reason.contains("release 0.2"),
                "'{}' is unsupported on a complete machine, but the page says '{cell}': {reason}",
                feature.label()
            ),
        }
    }
}

/// Canary: the comparison can fail. A row that the page does not have, and a duplicate row, are found.
#[test]
fn the_reader_of_the_page_finds_missing_and_duplicate_rows() {
    let page = "| Feature | Kernel |\n| --- | --- |\n| A | ✔ 0.1 |\n| B | ~ 0.2 |\n| B | ✘ |\n";
    assert_eq!(kernel_cell(page, "A").as_deref(), Some("✔ 0.1"));
    assert_eq!(kernel_cell(page, "C"), None);
    assert_eq!(kernel_cell(page, "B"), None);
}

/// The target of the first link `[text](target)` in a cell, if there is one.
fn link_in(cell: &str) -> Option<&str> {
    let start = cell.find("](")? + 2;
    let end = start + cell[start..].find(')')?;
    Some(&cell[start..end])
}

/// "No test, no check mark" (the rules at the end of the page): every faithful cell of the Kernel column, and
/// every place where a run works, links to a file that exists.
#[test]
fn every_faithful_mark_of_the_kernel_links_to_a_test_that_exists() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    let places = [
        "Linux, as root",
        "Linux, unprivileged user namespace",
        "GitHub-hosted runner",
    ];
    let capabilities = capabilities(&Environment::complete());
    // What is exact here, and the places where a run works; what is not built yet has no test and no check mark
    let labels = Feature::ALL
        .iter()
        .filter(|feature| capabilities.support(**feature) == Support::Exact)
        .map(|feature| feature.label())
        .chain(places);
    for label in labels {
        let cell = kernel_cell(COVERAGE, label).unwrap_or_else(|| panic!("no row '{label}'"));
        assert!(
            cell.starts_with("✔ 0.1") || cell.starts_with("✔ 0.2"),
            "'{label}': {cell}"
        );
        let target = link_in(&cell).unwrap_or_else(|| panic!("'{label}' has no link: {cell}"));
        assert!(
            docs.join(target).is_file(),
            "'{label}' links to {target}, which is not a file"
        );
    }
}

/// Canary: a cell without a link, and a link to a file that is not there, are found.
#[test]
fn the_reader_of_links_finds_a_missing_link_and_a_missing_file() {
    assert_eq!(link_in("✔ 0.1"), None);
    assert_eq!(
        link_in("✔ 0.1 ([test](../crates/x/tests/a.rs); note)"),
        Some("../crates/x/tests/a.rs")
    );
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    assert!(!docs.join("../crates/no-such-crate/tests/a.rs").is_file());
}
