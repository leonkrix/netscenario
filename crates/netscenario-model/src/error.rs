//! What goes wrong when a scenario is read or checked, and how it is shown.
//!
//! An [`Error`] holds one or more [`Problem`]s. Every problem has a message; those that come from a file also
//! have a [`Location`] (line and column), and those about the contents have the path of the value, such as
//! `topology.links[0].between[1]`. Rendering shows the line of the file with a mark under the place.

use std::fmt::{self, Write as _};

/// A place in a file: 1-based line and column (counted in characters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Location {
    /// The line, starting at 1.
    pub line: usize,
    /// The column, starting at 1.
    pub column: usize,
}

/// One thing that is wrong with a scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The path of the value in the scenario (`topology.links[0].between[1]`), if the problem is about a
    /// value of the contents.
    pub path: Option<String>,
    /// Where it is in the file, if the scenario was read from a file.
    pub location: Option<Location>,
    /// How many characters the mark under the place covers.
    pub length: usize,
    /// What is wrong: what was found and what was expected.
    pub message: String,
    /// How to fix it, if there is something useful to say.
    pub help: Option<String>,
}

impl Problem {
    /// A problem with a message and nothing else.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            path: None,
            location: None,
            length: 1,
            message: message.into(),
            help: None,
        }
    }

    /// The same problem, about the value at `path`.
    #[must_use]
    pub fn at_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// The same problem, with a hint on how to fix it.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// The same problem, at `location` in the file.
    #[must_use]
    pub fn at_location(mut self, location: Location, length: usize) -> Self {
        self.location = Some(location);
        self.length = length.max(1);
        self
    }
}

/// A scenario that cannot be used: one or more problems, with the file they were found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    source_name: String,
    source: String,
    problems: Vec<Problem>,
}

impl Error {
    pub(crate) fn new(source_name: &str, source: &str, problems: Vec<Problem>) -> Self {
        debug_assert!(!problems.is_empty(), "an error has at least one problem");
        Self {
            source_name: source_name.to_owned(),
            source: source.to_owned(),
            problems,
        }
    }

    /// The problems, in the order of the file.
    #[must_use]
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The text shown to a person: every problem with the line of the file and a mark under the place.
    #[must_use]
    pub fn render(&self) -> String {
        let lines: Vec<&str> = self.source.lines().collect();
        let mut out = String::new();
        for (index, problem) in self.problems.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            render_problem(&mut out, problem, &self.source_name, &lines);
        }
        out
    }
}

fn render_problem(out: &mut String, problem: &Problem, source_name: &str, lines: &[&str]) {
    out.push_str("error: ");
    out.push_str(&problem.message);
    out.push('\n');
    let mut notes: Vec<String> = Vec::new();
    if let Some(path) = &problem.path {
        notes.push(format!("path: {path}"));
    }
    if let Some(help) = &problem.help {
        notes.push(format!("help: {help}"));
    }
    let Some(location) = problem.location else {
        for note in &notes {
            let _ = writeln!(out, "  = {note}");
        }
        return;
    };
    let shown_before = location.line.saturating_sub(1).max(1).min(location.line);
    let width = location.line.to_string().len();
    let _ = writeln!(
        out,
        "{:>width$}--> {source_name}:{}:{}",
        "", location.line, location.column
    );
    let _ = writeln!(out, "{:>width$} |", "");
    for number in shown_before..=location.line {
        let text = lines.get(number - 1).copied().unwrap_or("");
        let _ = writeln!(out, "{number:>width$} | {text}");
    }
    let padding = " ".repeat(location.column.saturating_sub(1));
    let _ = writeln!(
        out,
        "{:>width$} | {padding}{}",
        "",
        "^".repeat(problem.length)
    );
    for note in &notes {
        let _ = writeln!(out, "{:>width$} = {note}", "");
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.render().trim_end())
    }
}

impl std::error::Error for Error {}
