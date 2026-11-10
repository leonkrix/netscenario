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
            // 0.1, 0.2 or 0.3: the release in which the Kernel backend got it
            Support::Exact => assert!(
                cell.starts_with("✔ 0.1") || cell.starts_with("✔ 0.2") || cell.starts_with("✔ 0.3"),
                "'{}' is exact in the code, but the page says '{cell}'",
                feature.label()
            ),
            Support::Limited { .. } => assert!(
                cell.starts_with('~'),
                "'{}' is limited in the code, but the page says '{cell}'",
                feature.label()
            ),
            // Planned: the format has it, the backend gets it in release 0.3. Or refused by design: the page says so
            Support::Unsupported { reason, .. } => assert!(
                (cell.starts_with("✔ 0.3") && reason.contains("release 0.3"))
                    || (cell == "✘" && !reason.contains("release 0.3")),
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
            cell.starts_with("✔ 0.1") || cell.starts_with("✔ 0.2") || cell.starts_with("✔ 0.3"),
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

/// One row of a table of the page: the section it is in, its label and its three cells (Kernel, Engine, Proxy).
struct Row {
    section: String,
    label: String,
    cells: Vec<String>,
}

fn rows_of(markdown: &str) -> Vec<Row> {
    let mut section = String::new();
    let mut rows = Vec::new();
    for line in markdown.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            title.trim().clone_into(&mut section);
        }
        let Some(inner) = line.strip_prefix('|') else {
            continue;
        };
        let cells: Vec<String> = inner
            .trim_end_matches('|')
            .split('|')
            .map(|cell| cell.trim().to_owned())
            .collect();
        let separator = cells
            .iter()
            .all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
        if cells.len() >= 4 && !separator && cells[1] != "Kernel" {
            rows.push(Row {
                section: section.clone(),
                label: cells[0].clone(),
                cells: cells[1..4].to_vec(),
            });
        }
    }
    rows
}

/// The release of a faithful mark (`✔ 0.2 (...)` is `Some(2)`), or `None` for any other cell.
fn faithful_release(cell: &str) -> Option<u32> {
    let rest = cell.strip_prefix("✔ 0.")?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Every link of a cell: `[text](target)` gives `target`.
fn links_in(cell: &str) -> Vec<&str> {
    cell.split("](")
        .skip(1)
        .filter_map(|part| part.split(')').next())
        .collect()
}

fn is_a_test(target: &str) -> bool {
    let path = target.split('#').next().unwrap_or(target);
    path.split('/').any(|part| part == "tests")
        || path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("test_"))
}

/// A faithful mark of a release that is out is proven: its Kernel cell links to a test, for every row of the page and not only for
/// the rows that the code declares (`Feature`). A mark of a later release is a plan (the page says so) and has no link yet; the
/// release pull request raises the version of the crates, and from then on the marks of that release need theirs. Links in any
/// cell of any column must lead to a file.
#[test]
fn every_faithful_mark_of_a_released_version_links_to_a_test_and_no_link_leads_nowhere() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    let minor: u32 = env!("CARGO_PKG_VERSION")
        .split('.')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let rows = rows_of(COVERAGE);
    assert!(
        rows.len() > 50,
        "the table was not read: {} rows",
        rows.len()
    );
    for row in &rows {
        for cell in &row.cells {
            for target in links_in(cell) {
                let path = target.split('#').next().unwrap();
                if !path.starts_with("http") && !path.is_empty() {
                    assert!(
                        docs.join(path).exists(),
                        "'{}' links to {target}, which is not there",
                        row.label
                    );
                }
            }
        }
        let kernel = &row.cells[0];
        if faithful_release(kernel).is_some_and(|release| release <= minor) {
            let links = links_in(kernel);
            assert!(
                !links.is_empty(),
                "'{}' is faithful in release 0.{minor} and has no link: {kernel}",
                row.label
            );
            // Where a run works, the proof is the CI; for a feature it is a test
            if row.section != "Where it can run" {
                assert!(
                    links.iter().any(|link| is_a_test(link)),
                    "'{}' links to no test: {kernel}",
                    row.label
                );
            }
        }
    }
}

/// Canary: the reader of the page and the rules find what they are for.
#[test]
fn the_rules_for_the_marks_find_a_plan_a_proof_and_a_missing_test() {
    let page = "## Topology\n| Feature | Kernel | Engine | Proxy |\n| --- | --- | --- | --- |\n| A | ✔ 0.1 ([t](../crates/x/tests/a.rs)) | ✔ 0.4 | ✘ |\n| B | ✔ 0.6 | ✔ 0.6 | ✘ |\n## Where it can run\n| Place | Kernel | Engine | Proxy |\n| P | ✔ 0.1 ([ci](../.github/workflows/ci.yml)) | · | · |\n";
    let rows = rows_of(page);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].section, "Topology");
    assert_eq!(rows[2].section, "Where it can run");
    assert_eq!(faithful_release(&rows[0].cells[0]), Some(1));
    assert_eq!(faithful_release(&rows[1].cells[0]), Some(6));
    assert_eq!(faithful_release("~ 0.2"), None);
    assert_eq!(faithful_release("✘"), None);
    assert_eq!(links_in(&rows[0].cells[0]), ["../crates/x/tests/a.rs"]);
    assert_eq!(links_in(&rows[1].cells[0]), Vec::<&str>::new());
    assert!(is_a_test("../crates/x/tests/a.rs#L3"));
    assert!(is_a_test("../python/netscenario/tests/test_a.py"));
    assert!(!is_a_test("decisions/D-083.md"));
    assert!(!is_a_test("../.github/workflows/ci.yml"));
}
