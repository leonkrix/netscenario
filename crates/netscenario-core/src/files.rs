//! The files that a scenario refers to (D-083): where they may be, how large they may be, and how they are read.
//!
//! A scenario names a file by a path relative to the directory of the scenario file. The files are read when the scenario is
//! loaded, and what they hold (the table made from a file of samples, with the SHA-256 of the file) becomes part of the
//! scenario, so the canonical form and its hash cover the content and a recorded scenario needs no file. A scenario from
//! elsewhere must not read what is not next to it, so a path that leads out of the directory is refused, symbolic links
//! included, unless the caller says it is wanted.

use std::io::Read;
use std::path::{Path, PathBuf};

use netscenario_model::{Error as ModelError, MAX_REFERENCED_FILE_BYTES, Scenario};

/// Reads the files that `scenario` refers to, relative to `base` (the directory of the scenario file), and fills in what is
/// made of them.
///
/// With `allow_outside` false a file must lie inside `base`, after links and `..` are resolved; an absolute path is refused
/// either way unless `allow_outside` is set.
///
/// # Errors
///
/// The problems, as an error that renders with the lines of `text`, if a file cannot be read, is too large, is outside
/// `base`, or does not make a table.
pub fn resolve_files(
    scenario: &mut Scenario,
    source_name: &str,
    text: &str,
    base: &Path,
    allow_outside: bool,
) -> Result<(), ModelError> {
    let problems =
        scenario.resolve_tables(|reference| read_referenced(base, reference, allow_outside));
    match ModelError::in_source(source_name, text, problems) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// The content of the file that `reference` names, by the rules above.
///
/// # Errors
///
/// A sentence that names the path as the scenario wrote it. The content of a file that is refused is never shown.
pub fn read_referenced(
    base: &Path,
    reference: &str,
    allow_outside: bool,
) -> Result<Vec<u8>, String> {
    let wanted = Path::new(reference);
    if wanted.is_absolute() && !allow_outside {
        return Err(outside(reference, "it is an absolute path"));
    }
    let root = base.canonicalize().map_err(|error| {
        format!(
            "cannot read '{reference}': the directory {} is not usable ({error})",
            base.display()
        )
    })?;
    let path: PathBuf = root.join(wanted);
    let found = path
        .canonicalize()
        .map_err(|error| format!("cannot read '{reference}': {}", reason(&error)))?;
    if !allow_outside && !found.starts_with(&root) {
        return Err(outside(reference, "it leads out of that directory"));
    }
    let file = std::fs::File::open(&found)
        .map_err(|error| format!("cannot read '{reference}': {}", reason(&error)))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot read '{reference}': {}", reason(&error)))?;
    if !metadata.is_file() {
        return Err(format!("'{reference}' is not a file"));
    }
    if metadata.len() > MAX_REFERENCED_FILE_BYTES {
        return Err(format!(
            "'{reference}' is larger than {MAX_REFERENCED_FILE_BYTES} bytes ({} bytes)",
            metadata.len()
        ));
    }
    // The size was checked, but the file may grow while it is read
    let mut bytes = Vec::new();
    file.take(MAX_REFERENCED_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read '{reference}': {}", reason(&error)))?;
    if bytes.len() as u64 > MAX_REFERENCED_FILE_BYTES {
        return Err(format!(
            "'{reference}' is larger than {MAX_REFERENCED_FILE_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

fn outside(reference: &str, why: &str) -> String {
    format!(
        "'{reference}' is outside the directory of the scenario file: {why}; a scenario may read only files next to it (the option --allow-outside-files lifts this)"
    )
}

/// The sentence of the system without the number of the error.
fn reason(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "no such file".to_owned(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_owned(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("netscenario-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// A file next to the scenario, in a folder below it, and through a link that stays inside are all fine.
    #[test]
    fn files_inside_the_directory_are_read() {
        let base = dir("inside");
        fs::create_dir_all(base.join("data")).unwrap();
        fs::write(base.join("data/a.txt"), "1\n2\n").unwrap();
        symlink(base.join("data/a.txt"), base.join("link.txt")).unwrap();
        for name in [
            "data/a.txt",
            "./data/a.txt",
            "link.txt",
            "data/../data/a.txt",
        ] {
            assert_eq!(
                read_referenced(&base, name, false).unwrap(),
                b"1\n2\n",
                "{name}"
            );
        }
        let _ = fs::remove_dir_all(base);
    }

    /// The rules that keep a scenario from the internet from reading what is not next to it. Each way out is refused, and
    /// each is a canary: the same call with `allow_outside` reads the file, so it is the rule that refuses.
    #[test]
    fn every_way_out_of_the_directory_is_refused_and_allowed_on_request() {
        let root = dir("outside");
        let base = root.join("scenario");
        fs::create_dir_all(&base).unwrap();
        fs::write(root.join("secret.txt"), "top secret\n").unwrap();
        symlink(root.join("secret.txt"), base.join("link.txt")).unwrap();
        symlink(&root, base.join("up")).unwrap();
        let absolute = root.join("secret.txt").to_string_lossy().into_owned();
        for reference in [
            "../secret.txt",
            "link.txt",
            "up/secret.txt",
            absolute.as_str(),
        ] {
            let refused = read_referenced(&base, reference, false).unwrap_err();
            assert!(
                refused.contains("is outside the directory of the scenario file"),
                "{reference}: {refused}"
            );
            assert!(
                !refused.contains("top secret"),
                "the content is never shown: {refused}"
            );
            assert_eq!(
                read_referenced(&base, reference, true).unwrap(),
                b"top secret\n",
                "{reference}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_that_is_missing_too_large_or_not_a_file_is_refused_with_a_sentence() {
        let base = dir("bad");
        let missing = read_referenced(&base, "nothing.txt", false).unwrap_err();
        assert_eq!(missing, "cannot read 'nothing.txt': no such file");
        fs::create_dir_all(base.join("folder")).unwrap();
        assert_eq!(
            read_referenced(&base, "folder", false).unwrap_err(),
            "'folder' is not a file"
        );
        let big = fs::File::create(base.join("big.txt")).unwrap();
        big.set_len(MAX_REFERENCED_FILE_BYTES + 1).unwrap();
        let refused = read_referenced(&base, "big.txt", false).unwrap_err();
        assert!(refused.contains("is larger than"), "{refused}");
        // Canary: a file of exactly the limit is read
        let edge = fs::File::create(base.join("edge.txt")).unwrap();
        edge.set_len(MAX_REFERENCED_FILE_BYTES).unwrap();
        assert_eq!(
            read_referenced(&base, "edge.txt", false).unwrap().len() as u64,
            MAX_REFERENCED_FILE_BYTES
        );
        let _ = fs::remove_dir_all(base);
    }

    /// The whole way: a scenario file, a file of samples next to it, and the table in the scenario that comes out.
    #[test]
    fn a_scenario_gets_its_table_from_the_file_next_to_it() {
        let base = dir("whole");
        let samples = (0..300).fold(String::new(), |mut text, i| {
            text.push_str(&(10 + i % 40).to_string());
            text.push('\n');
            text
        });
        fs::write(base.join("delays.txt"), samples).unwrap();
        let text = "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      forward: { delay: 100ms, jitter: { amount: 20ms, distribution: table, table: { file: delays.txt } } }\nworkloads:\n  - { id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }\n";
        let mut scenario = Scenario::from_yaml(text, "t.yaml").unwrap();
        resolve_files(&mut scenario, "t.yaml", text, &base, false).unwrap();
        assert_eq!(scenario.unresolved_table_files(), Vec::<String>::new());
        // A file that is not there is an error at the table, with its line
        fs::remove_file(base.join("delays.txt")).unwrap();
        let mut again = Scenario::from_yaml(text, "t.yaml").unwrap();
        let error = resolve_files(&mut again, "t.yaml", text, &base, false)
            .unwrap_err()
            .render();
        assert!(
            error.contains("cannot read 'delays.txt': no such file"),
            "{error}"
        );
        assert!(error.contains("t.yaml:9:"), "{error}");
        let _ = fs::remove_dir_all(base);
    }
}
