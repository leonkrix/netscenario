//! The declaration of the Kernel backend is compared with the Kernel column of `docs/COVERAGE.md`: the page and
//! the code must not disagree about what is faithful.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

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
fn what_is_exact_here_is_a_check_mark_for_release_0_1_there() {
    let capabilities = capabilities(&Environment::complete());
    for feature in Feature::ALL {
        let cell = kernel_cell(COVERAGE, feature.label()).expect("a row");
        match capabilities.support(*feature) {
            Support::Exact => assert!(
                cell.starts_with("✔ 0.1"),
                "'{}' is exact in the code, but the page says '{cell}'",
                feature.label()
            ),
            Support::Limited { .. } => assert!(
                cell.starts_with('~'),
                "'{}' is limited in the code, but the page says '{cell}'",
                feature.label()
            ),
            Support::Unsupported { .. } => {
                panic!("'{}' is unsupported on a complete machine", feature.label())
            }
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
