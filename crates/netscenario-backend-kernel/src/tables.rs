//! The tables of the jitter that `tc` loads by name (D-084).
//!
//! `tc` looks for the table of `distribution NAME` in the file `NAME.dist` in the directory `TC_LIB_DIR`, or in the directory of the
//! system. A table made from a file of samples does not exist there, so the run keeps a directory of its own with the tables it
//! made and with links to the tables of the system (`normal`, `pareto`, `paretonormal`), and every `tc` of the run gets that
//! directory as `TC_LIB_DIR`. The directory is made when the first table is needed and removed at the end of the run.

use std::path::{Path, PathBuf};

use netscenario_model::{DelayTable, sha256_hex};

/// The tables of iproute2 that a scenario can name.
const SHIPPED: [&str; 4] = ["normal", "pareto", "paretonormal", "experimental"];

/// The directories where the system keeps its tables, after `TC_LIB_DIR` (Debian and Ubuntu keep them under the triplet of the
/// architecture, other systems directly under `lib`).
const SYSTEM_DIRECTORIES: [&str; 6] = [
    "/usr/lib/tc",
    "/usr/lib/x86_64-linux-gnu/tc",
    "/usr/lib/aarch64-linux-gnu/tc",
    "/usr/lib64/tc",
    "/lib/tc",
    "/usr/share/iproute2",
];

/// The name `tc` is given for a table made from samples: it is the content that names it, so two directions with the same table
/// share one file, and a step that changes only the amount names the same table.
#[must_use]
pub(crate) fn table_name(table: &DelayTable) -> Option<String> {
    let values = table.values.as_ref()?;
    let mut bytes = Vec::with_capacity(values.entries().len() * 2);
    for entry in values.entries() {
        bytes.extend_from_slice(&entry.to_be_bytes());
    }
    Some(format!("ns{}", &sha256_hex(&bytes)[..16]))
}

/// The directory of one run.
pub(crate) struct TableDirectory {
    path: PathBuf,
}

impl TableDirectory {
    /// Makes the directory, with links to the tables of the system. `system` is the value of `TC_LIB_DIR` of this process, which
    /// is looked in first.
    ///
    /// # Errors
    ///
    /// A sentence if the directory cannot be made.
    pub(crate) fn create(run_id: &str, system: Option<&str>) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("netscenario-tables-{run_id}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).map_err(|error| {
            format!(
                "cannot make the directory {} for the tables of the jitter: {error}",
                path.display()
            )
        })?;
        let directory = Self { path };
        directory.link_system_tables(system);
        Ok(directory)
    }

    /// The directory, to be given to `tc` as `TC_LIB_DIR`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn link_system_tables(&self, system: Option<&str>) {
        let candidates: Vec<PathBuf> = system
            .into_iter()
            .map(PathBuf::from)
            .chain(SYSTEM_DIRECTORIES.iter().map(PathBuf::from))
            .collect();
        for name in SHIPPED {
            let file = format!("{name}.dist");
            if let Some(found) = candidates
                .iter()
                .map(|directory| directory.join(&file))
                .find(|path| path.is_file())
            {
                // A link that cannot be made leaves the table out, and `tc` says so when a run asks for it
                let _ = std::os::unix::fs::symlink(found, self.path.join(&file));
            }
        }
    }

    /// Writes the table, if it is not there yet, and returns the name that `tc` is given.
    ///
    /// # Errors
    ///
    /// A sentence if the table has no entries or the file cannot be written.
    pub(crate) fn add(&self, table: &DelayTable) -> Result<String, String> {
        let name = table_name(table).ok_or_else(|| "a table without entries".to_owned())?;
        let file = self.path.join(format!("{name}.dist"));
        if !file.exists() {
            let values = table
                .values
                .as_ref()
                .ok_or_else(|| "a table without entries".to_owned())?;
            let from = table.file.as_deref().unwrap_or("a scenario");
            std::fs::write(
                &file,
                values.to_dist_file(&format!("made by netscenario from {from}")),
            )
            .map_err(|error| format!("cannot write {}: {error}", file.display()))?;
        }
        Ok(name)
    }

    /// Removes the directory.
    pub(crate) fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use netscenario_model::TableValues;

    use super::*;

    fn table(shift: f64) -> DelayTable {
        let samples: Vec<f64> = (0..500)
            .map(|i| f64::from(i).powi(2) + shift * f64::from(i))
            .collect();
        let (values, scale) = TableValues::from_samples(&samples).unwrap();
        DelayTable {
            scale: Some(scale),
            ..DelayTable::from_values(values)
        }
    }

    /// The name is the content: the same table gets the same name whatever else is in the scenario, another table another one.
    /// Canary: two tables that differ in one entry differ in the name.
    #[test]
    fn the_name_of_a_table_is_its_content() {
        let first = table(0.0);
        assert_eq!(table_name(&first), table_name(&first.clone()));
        let name = table_name(&first).unwrap();
        assert_eq!(name.len(), 18);
        assert!(name.starts_with("ns") && name[2..].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(table_name(&first), table_name(&table(7.0)));
        assert_eq!(table_name(&DelayTable::from_file("x.txt")), None);
    }

    #[test]
    fn a_table_is_written_once_in_the_form_of_the_files_of_iproute2() {
        let directory =
            TableDirectory::create(&format!("test-{}", std::process::id()), Some("/nowhere"))
                .unwrap();
        let table = table(3.0);
        let name = directory.add(&table).unwrap();
        assert_eq!(directory.add(&table).unwrap(), name);
        let text = std::fs::read_to_string(directory.path().join(format!("{name}.dist"))).unwrap();
        assert!(text.starts_with("# made by netscenario"));
        assert_eq!(
            TableValues::from_dist_file(&text).unwrap(),
            table.values.clone().unwrap()
        );
        directory.remove();
        assert!(!directory.path().exists());
    }

    /// The directory is what `tc` is given as `TC_LIB_DIR`: it finds the table of the run and, through the links, the tables of
    /// the system. `tc` answers "No distribution data" when it finds no table; any other answer (the kernel's, on a machine
    /// without `netem`) means it was found. Canary: a name that was not written is not found.
    #[test]
    fn tc_finds_the_table_of_the_run_and_the_tables_of_the_system_in_the_directory() {
        let Some(tc) = crate::command::find("tc") else {
            return;
        };
        let directory =
            TableDirectory::create(&format!("tc-{}", std::process::id()), None).unwrap();
        let own = directory.add(&table(1.0)).unwrap();
        let ask = |name: &str| -> Option<String> {
            let output = std::process::Command::new("unshare")
                .args(["--user", "--map-root-user", "--net", "--"])
                .arg(&tc)
                .args([
                    "qdisc",
                    "replace",
                    "dev",
                    "lo",
                    "root",
                    "netem",
                    "delay",
                    "10ms",
                    "2ms",
                    "distribution",
                    name,
                ])
                .env("TC_LIB_DIR", directory.path())
                .env("LC_ALL", "C")
                .output()
                .ok()?;
            Some(String::from_utf8_lossy(&output.stderr).into_owned())
        };
        let Some(unknown) = ask("nosuchtable") else {
            return;
        };
        if !unknown.contains("No distribution data") {
            // No `unshare` that works here
            directory.remove();
            return;
        }
        assert!(unknown.contains("nosuchtable"), "{unknown}");
        for name in [own.as_str(), "normal", "pareto", "paretonormal"] {
            let answer = ask(name).unwrap();
            // `normal` and the others need the tables of the system: where there are none, the machine has nothing to find
            let system = [
                "/usr/lib/tc",
                "/usr/lib/x86_64-linux-gnu/tc",
                "/usr/lib/aarch64-linux-gnu/tc",
                "/usr/lib64/tc",
                "/lib/tc",
                "/usr/share/iproute2",
            ]
            .iter()
            .any(|d| {
                std::path::Path::new(d)
                    .join(format!("{name}.dist"))
                    .is_file()
            });
            if name == own || system {
                assert!(!answer.contains("No distribution data"), "{name}: {answer}");
            }
        }
        directory.remove();
    }
}
