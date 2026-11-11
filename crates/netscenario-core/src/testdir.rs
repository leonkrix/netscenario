//! Directories of the unit tests: named by the test and the process, and removed when the thread of the test ends.
//!
//! The tests used to leave their directories in the temporary directory (thousands after a few weeks of work). A directory
//! is removed when the test thread ends if the test passed; the directories of a test that failed are kept for a look (and
//! `NETSCENARIO_KEEP_TEST_DIRS=1` keeps all of them). Nothing is created here: the test makes the directory, or the code it
//! calls does. (A test that catches a panic on purpose, as a canary of a check, counts as failed and keeps its directory.)

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Once;

/// The directories this thread asked for.
struct Made(RefCell<Vec<PathBuf>>);

impl Drop for Made {
    fn drop(&mut self) {
        if std::env::var_os("NETSCENARIO_KEEP_TEST_DIRS").is_some() || FAILED.with(Cell::get) {
            return;
        }
        for path in self.0.get_mut().drain(..) {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

thread_local! {
    static MADE: Made = const { Made(RefCell::new(Vec::new())) };
    /// Whether this thread panicked: the test failed.
    static FAILED: Cell<bool> = const { Cell::new(false) };
}

/// Notes in the thread that panics that it did, before the message is printed.
fn watch_for_failure() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            FAILED.with(|failed| failed.set(true));
            before(info);
        }));
    });
}

/// The path of a directory for the test `name` (not made): `netscenario-<kind>-<name>-<pid>`. Asking again gives the same path.
pub(crate) fn path(kind: &str, name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("netscenario-{kind}-{name}-{}", std::process::id()));
    watch_for_failure();
    MADE.with(|made| {
        let mut made = made.0.borrow_mut();
        if !made.contains(&path) {
            made.push(path.clone());
        }
    });
    path
}

/// The same, and emptied if it is there.
pub(crate) fn fresh(kind: &str, name: &str) -> PathBuf {
    let path = path(kind, name);
    let _ = std::fs::remove_dir_all(&path);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory is gone when the thread of a passed test ends and stays when the test failed (a canary: with the wrong
    /// check, one of the two asserts fails).
    #[test]
    fn a_directory_goes_with_a_passed_test_and_stays_with_a_failed_one() {
        let made = |name: &'static str, fail: bool| {
            std::thread::spawn(move || {
                let dir = fresh("testdir", name);
                std::fs::create_dir_all(&dir).unwrap();
                assert!(!fail, "the test fails");
                dir
            })
            .join()
        };
        let passed = made("passed", false).unwrap();
        assert!(!passed.exists(), "the directory of a passed test is left");
        let _ = made("failed", true).unwrap_err();
        let kept =
            std::env::temp_dir().join(format!("netscenario-testdir-failed-{}", std::process::id()));
        assert!(kept.exists(), "the directory of a failed test is gone");
        std::fs::remove_dir_all(kept).unwrap();
    }
}
