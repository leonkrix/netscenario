//! `netscenario report`: Markdown, `JUnit` or HTML from a result directory (CLI-02, RPT-02, RPT-03, RPT-04; D-076).

use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use netscenario_core::report::{Format, Report};

use crate::args::{Parsed, Spec};
use crate::exit;

pub(crate) const REPORT_HELP: &str = "\
Usage: netscenario report <dir> [--format md|html|junit] [--out <file>]

Makes a report from a result directory: the directory of a run (the `--out` of `netscenario run`) or of a set of runs
(`--repetitions`). The report is made from the files of the directory and from nothing else, so the same directory gives
the same bytes. See docs/REPORTS.md.

  --format <name>  md (default), the summary of a run or of a set as Markdown, for the summary of a CI job; junit,
                   one test case for each workload with the metrics as properties; html, one file with inline
                   charts, nothing to fetch
  --out <file>     write the report to this file (default: standard output, for every format)

A JUnit case fails if the run was interrupted, or if the kernel refused packets of the workload and no link was shut
on purpose. The command itself exits with 0 whenever it could make the report; the failures are in the report.

Exit codes: 0 the report was made, 2 the directory or an option is wrong, or a file of the directory cannot be read.
";

pub(crate) const REPORT_SPEC: Spec = Spec {
    values: &["format", "out"],
    flags: &[],
};

pub(crate) fn report_command<S: AsRef<str>>(
    args: &[S],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let parsed = match Parsed::parse(args, &REPORT_SPEC) {
        Ok(parsed) => parsed,
        Err(message) => return crate::usage_error(err, "report", &message, REPORT_HELP),
    };
    if parsed.help {
        let _ = write!(out, "{REPORT_HELP}");
        return exit::OK;
    }
    let dir = match parsed.single("the result directory") {
        Ok(dir) => dir.to_owned(),
        Err(message) => return crate::usage_error(err, "report", &message, REPORT_HELP),
    };
    let format = match parsed.value("format").map(Format::from_str).transpose() {
        Ok(format) => format.unwrap_or(Format::Markdown),
        Err(message) => {
            return crate::usage_error(err, "report", &format!("--format: {message}"), REPORT_HELP);
        }
    };
    let text = match Report::load(Path::new(&dir), format == Format::Html)
        .and_then(|report| report.render(format))
    {
        Ok(text) => text,
        Err(error) => {
            let _ = writeln!(err, "netscenario report: {error}");
            return exit::USAGE;
        }
    };
    match parsed.value("out") {
        Some(path) => {
            if let Err(error) = std::fs::write(path, &text) {
                let _ = writeln!(err, "netscenario report: cannot write {path}: {error}");
                return exit::USAGE;
            }
        }
        None => {
            let _ = write!(out, "{text}");
        }
    }
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::tests::RESULT;

    fn call(args: &[&str]) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = report_command(args, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// A directory with a `result.json` (the one of the tests of the summary) and nothing else.
    fn directory(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "netscenario-cli-report-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("result.json"), RESULT).unwrap();
        dir
    }

    #[test]
    fn markdown_is_the_default_and_goes_to_standard_output() {
        let dir = directory("md");
        let (code, out, err) = call(&[dir.to_str().unwrap()]);
        assert_eq!((code, err.as_str()), (exit::OK, ""));
        assert!(out.starts_with("## netscenario: lte-like-link\n"), "{out}");
        assert!(out.contains("### Workloads"), "{out}");
        let (code, junit, _) = call(&[dir.to_str().unwrap(), "--format", "junit"]);
        assert_eq!(code, exit::OK);
        assert!(
            junit.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites "),
            "{junit}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_is_written_with_out_and_standard_output_stays_empty() {
        let dir = directory("out");
        let file = dir.join("summary.md");
        let (code, out, _) = call(&[
            dir.to_str().unwrap(),
            "--format",
            "md",
            "--out",
            file.to_str().unwrap(),
        ]);
        assert_eq!((code, out.as_str()), (exit::OK, ""));
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("## netscenario: lte-like-link")
        );
        let (code, _, err) = call(&[
            dir.to_str().unwrap(),
            "--out",
            dir.join("no/such/dir/x.md").to_str().unwrap(),
        ]);
        assert_eq!(code, exit::USAGE);
        assert!(err.contains("cannot write"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The HTML needs the series and the records, and says which file is missing instead of drawing an empty page.
    #[test]
    fn html_of_a_directory_without_its_records_names_the_missing_file() {
        let dir = directory("html");
        let (code, out, err) = call(&[dir.to_str().unwrap(), "--format", "html"]);
        assert_eq!((code, out.as_str()), (exit::USAGE, ""));
        assert!(
            err.contains("flow-0.records") || err.contains("series"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_is_not_a_result_directory_is_exit_2_with_the_reason() {
        let dir = std::env::temp_dir().join(format!(
            "netscenario-cli-report-none-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (code, _, err) = call(&[dir.to_str().unwrap()]);
        assert_eq!(code, exit::USAGE);
        assert!(
            err.contains("has neither result.json nor runset.json"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
