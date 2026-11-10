//! Reading a scenario from a file.

use std::sync::Arc;

use serde_saphyr::{MergeKeyPolicy, Options};

use crate::error::{Error, Location, Problem};
use crate::expand::{self, LoadOptions, Origins};
use crate::required;
use crate::reserved;
use crate::scenario::Scenario;
use crate::source::{self, SourceIndex};
use crate::suggest::closest;

/// A scenario file may be at most this large.
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

fn options() -> Options {
    let mut options = Options::default();
    // The messages are rendered here, with the line of the file; the library's own rendering is not used
    options.with_snippet = false;
    options.merge_keys = MergeKeyPolicy::Error;
    options.reject_unsupported_tags = true;
    options.budget = serde_saphyr::budget! {
        max_anchors: 0,
        max_aliases: 0,
        flow_nesting_limit: 32,
        max_events: 200_000,
    };
    options
}

impl Scenario {
    /// Reads a scenario from YAML (or JSON) text and checks it. `source_name` is shown in messages, usually the
    /// name of the file.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] with every problem found if the text is not a valid scenario: a syntax error, an
    /// unknown or duplicate key, a value that is not valid (a number without a unit), a key of a later release,
    /// or a rule that the contents break (a node that does not exist). Each problem has the line and column,
    /// and those about the contents have the path of the value.
    pub fn from_yaml(text: &str, source_name: &str) -> Result<Self, Error> {
        Self::from_yaml_with(text, source_name, &LoadOptions::new())
    }

    /// Reads a scenario like [`Scenario::from_yaml`], with the values of its parameters and the source of the files it includes
    /// (SCN-05, D-093). A file that uses no parameter, template or include reads as it is; one that does is expanded first, and
    /// what is checked is the scenario that results. A problem is shown at the line where its value was written, which may be in
    /// a template or an included file, with the places of use that led to it.
    ///
    /// # Errors
    ///
    /// As [`Scenario::from_yaml`], and a parameter that has no value or is not declared, an expression that cannot be
    /// evaluated, a template or file that cannot be found or read, a cycle, and the limits of the expansion.
    pub fn from_yaml_with(
        text: &str,
        source_name: &str,
        load: &LoadOptions<'_>,
    ) -> Result<Self, Error> {
        let early = |problems: Vec<Problem>| Error::new(source_name, text, sorted(problems));

        if text.len() > MAX_FILE_BYTES {
            return Err(early(vec![
                Problem::new(format!(
                    "the file is larger than {MAX_FILE_BYTES} bytes ({} bytes)",
                    text.len()
                ))
                .with_help("a scenario does not need that much text"),
            ]));
        }
        if text.trim().is_empty() {
            return Err(early(vec![
                Problem::new("the file is empty").with_help("a scenario starts with `version: 0`"),
            ]));
        }

        // Parameters, templates and includes, if the file has them: what is checked below is the text they make
        let expansion = expand::expand(text, source_name, load)?;
        let (provenance, expanded) = match expansion {
            Some(expansion) => (Some(Arc::new(expansion.provenance)), Some(expansion.text)),
            None => (None, None),
        };
        let checked = expanded.as_deref().unwrap_or(text);
        let fail = |problems: Vec<Problem>| match &provenance {
            Some(provenance) => Error::with_sources(
                source_name,
                text,
                provenance.other_sources(),
                sorted(problems.into_iter().map(|p| provenance.apply(p)).collect()),
            ),
            None => early(problems),
        };

        let (index, mut problems) = source::index(checked);
        if problems.is_empty() && index.is_empty() && !index.is_broken() {
            return Err(fail(vec![
                Problem::new("the file has no content, only comments")
                    .with_help("a scenario starts with `version: 0`"),
            ]));
        }
        if !problems.is_empty() {
            return Err(fail(problems));
        }
        // With a syntax error the index is incomplete; the YAML library reports the error below
        if !index.is_broken() {
            problems = reserved::check(&index);
            problems.extend(required::check(&index));
            if !problems.is_empty() {
                return Err(fail(problems));
            }
        }

        let mut scenario: Scenario = match serde_saphyr::from_str_with_options(checked, options()) {
            Ok(scenario) => scenario,
            Err(error) => return Err(fail(vec![syntax_problem(&error, &index)])),
        };

        let mut problems = scenario.resolve_profiles();
        if problems.is_empty() {
            problems = scenario.validate();
        }
        let problems: Vec<Problem> = problems
            .into_iter()
            .map(|problem| locate(problem, &index))
            .collect();
        if problems.is_empty() {
            scenario.origins = Origins(provenance);
            Ok(scenario)
        } else {
            Err(fail(problems))
        }
    }

    /// Problems that were found after the scenario was read, such as a feature that a backend cannot reproduce, as an error with
    /// the place of each path in the file the scenario was read from; with a scenario that was expanded (D-093), in the template
    /// or the included file where the value was written. `None` if there are no problems.
    #[must_use]
    pub fn problems_in_source(
        &self,
        source_name: &str,
        source: &str,
        problems: Vec<Problem>,
    ) -> Option<Error> {
        let Some(provenance) = &self.origins.0 else {
            return Error::in_source(source_name, source, problems);
        };
        if problems.is_empty() {
            return None;
        }
        let (index, _) = crate::source::index(provenance.expanded());
        let located = problems
            .into_iter()
            .map(|problem| provenance.apply(locate(problem, &index)))
            .collect();
        Some(Error::with_sources(
            source_name,
            source,
            provenance.other_sources(),
            sorted(located),
        ))
    }
}

impl Scenario {
    /// Writes the scenario as YAML in its canonical long form: a shortcut of the topology is written as the nodes and links
    /// it made. [`Scenario::from_yaml`] reads the text back into an equal scenario. Comments of the original are not kept.
    ///
    /// # Errors
    ///
    /// The sentence of the YAML writer if the scenario cannot be written (it can always be, for a valid scenario).
    pub fn to_yaml(&self) -> Result<String, String> {
        serde_saphyr::to_string(self).map_err(|error| error.to_string())
    }
}

impl Error {
    /// Problems that were found after the file was read, such as a feature that a backend cannot reproduce,
    /// with the place in `source` of the path of each: the same kind of error as the reading of a file gives.
    /// `None` if there are no problems.
    #[must_use]
    pub fn in_source(source_name: &str, source: &str, problems: Vec<Problem>) -> Option<Self> {
        if problems.is_empty() {
            return None;
        }
        let (index, _) = crate::source::index(source);
        let located = problems
            .into_iter()
            .map(|problem| locate(problem, &index))
            .collect();
        Some(Self::new(source_name, source, sorted(located)))
    }
}

/// Puts a problem about the contents at the place of its path in the file.
fn locate(mut problem: Problem, index: &SourceIndex) -> Problem {
    // A node or a link that a shortcut made is shown at the shortcut, with the path in the file
    if let Some(source) = problem
        .path
        .as_deref()
        .and_then(|path| index.source_path(path))
    {
        problem.path = Some(source);
    }
    let found = problem.path.as_deref().and_then(|path| index.locate(path));
    match found {
        Some((location, length)) => problem.at_location(location, length),
        None => problem,
    }
}

fn sorted(mut problems: Vec<Problem>) -> Vec<Problem> {
    // The order of the file, the files that it includes after it; problems at the same place (the uses of a template share
    // the lines of its content) by their path in the scenario that results and by their text, so that the order never depends
    // on a hash
    problems.sort_by_cached_key(|problem| {
        let (line, column) = problem.location.map_or((0, 0), |l| (l.line, l.column));
        (
            problem.file().map(str::to_owned),
            line,
            column,
            problem.path.as_deref().map(natural),
            problem.chain().to_vec(),
            problem.message.clone(),
        )
    });
    // The links of a shortcut share the place of their values, so the same fault is found once for each link
    problems.dedup_by(|a, b| {
        a.path == b.path
            && a.location == b.location
            && a.file() == b.file()
            && a.message == b.message
    });
    problems
}

/// A path as a key that puts `links[2]` before `links[10]`: the text and the numbers of the path in turn.
fn natural(path: &str) -> Vec<(String, u64)> {
    let mut parts = Vec::new();
    let mut rest = path;
    while !rest.is_empty() {
        let text_end = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (text, after) = rest.split_at(text_end);
        let digits_end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        let (digits, next) = after.split_at(digits_end);
        parts.push((text.to_owned(), digits.parse().unwrap_or(0)));
        rest = next;
    }
    parts
}

/// Turns an error of the YAML library into a problem with the bare message, the position, and the length of the
/// value at that position.
fn syntax_problem(error: &serde_saphyr::Error, index: &SourceIndex) -> Problem {
    let (message, help) = clean_message(&error.to_string());
    let mut problem = Problem::new(message);
    if let Some(help) = help {
        problem = problem.with_help(help);
    }
    let Some(at) = error.location() else {
        return problem;
    };
    let location = Location {
        line: usize::try_from(at.line()).unwrap_or(0),
        column: usize::try_from(at.column()).unwrap_or(0),
    };
    // The mark covers the whole key or value that starts here. A mapping starts where its first key does, so
    // a key is preferred, then the longest value: the choice must not depend on the order of the index.
    let key_length = index
        .iter()
        .filter_map(|(_, entry)| {
            entry
                .key
                .filter(|(key, _)| *key == location)
                .map(|(_, length)| length)
        })
        .max();
    let value_length = index
        .iter()
        .filter(|(_, entry)| entry.value.0 == location)
        .map(|(_, entry)| entry.value.1)
        .max();
    let length = key_length.or(value_length).unwrap_or(1);
    problem.at_location(location, length)
}

/// The library writes "message at line N, column M" or "line N column M: message", with advice about its own
/// options. This keeps the message, and says it in the words of the scenario format where that helps.
fn clean_message(rendered: &str) -> (String, Option<String>) {
    let text = rendered.strip_prefix("error: ").unwrap_or(rendered);
    let text = text
        .strip_prefix("line ")
        .and_then(|rest| rest.split_once(": "))
        .map_or(text, |(_, message)| message);
    let text = text.lines().next().unwrap_or(text);
    let text = match text.rsplit_once(" at line ") {
        Some((message, position)) if position.contains(", column ") => message,
        _ => text,
    };
    if let Some(rest) = text.strip_prefix("duplicate mapping key: ") {
        let key = rest.split(',').next().unwrap_or(rest);
        return (
            format!("the key `{key}` appears twice in this mapping"),
            Some("remove one of them".to_owned()),
        );
    }
    if text.starts_with("multiple YAML documents") {
        return (
            "the file has more than one YAML document".to_owned(),
            Some(
                "a scenario file is one document: remove the `---` line and what follows"
                    .to_owned(),
            ),
        );
    }
    let text = text.trim();
    (text.to_owned(), unknown_name_hint(text))
}

/// For "unknown field `dealy`, expected one of delay, jitter" (or "unknown variant"): the name that was most
/// likely meant, if exactly one is close enough.
fn unknown_name_hint(message: &str) -> Option<String> {
    let rest = message
        .strip_prefix("unknown field `")
        .or_else(|| message.strip_prefix("unknown variant `"))?;
    let (written, rest) = rest.split_once('`')?;
    let list = rest.strip_prefix(", expected one of ")?;
    let known: Vec<&str> = list
        .split(", ")
        .map(|name| name.trim_matches('`'))
        .collect();
    closest(written, &known).map(|near| format!("did you mean '{near}'?"))
}

#[cfg(test)]
mod tests {
    use super::{Error, Problem};

    #[test]
    fn problems_found_later_are_shown_at_their_place_in_the_file() {
        let source = "version: 0\nseed: 5\nname: t\n";
        let problem = Problem::new("not here")
            .at_path("seed")
            .with_help("do this");
        let error = Error::in_source("t.yaml", source, vec![problem]).unwrap();
        assert_eq!(
            error.render(),
            "error: not here\n --> t.yaml:2:7\n  |\n1 | version: 0\n2 | seed: 5\n  |       ^\n  = path: seed\n  = help: do this\n"
        );
        assert!(Error::in_source("t.yaml", source, Vec::new()).is_none());
    }

    use super::{clean_message, unknown_name_hint};

    #[test]
    fn a_misspelled_field_or_value_is_named_and_a_far_one_is_not() {
        let hint = |written: &str| {
            unknown_name_hint(&format!(
                "unknown field `{written}`, expected one of delay, jitter, loss, rate"
            ))
        };
        assert_eq!(hint("dealy").as_deref(), Some("did you mean 'delay'?"));
        assert_eq!(hint("Delay").as_deref(), Some("did you mean 'delay'?"));
        assert_eq!(hint("speed"), None);
        assert_eq!(
            unknown_name_hint("unknown variant `kernal`, expected one of auto, kernel").as_deref(),
            Some("did you mean 'kernel'?")
        );
        // not a message of this kind
        assert_eq!(unknown_name_hint("invalid type: integer"), None);
        assert_eq!(unknown_name_hint("unknown field `x`"), None);
    }

    #[test]
    fn the_position_and_the_advice_about_options_are_removed() {
        assert_eq!(clean_message("error: line 7 column 16: boom").0, "boom");
        assert_eq!(clean_message("boom at line 7, column 16").0, "boom");
        assert_eq!(
            clean_message("it happened at line of fire").0,
            "it happened at line of fire"
        );
        let (message, help) = clean_message(
            "duplicate mapping key: name, set DuplicateKeyPolicy in Options if acceptable at line 3, column 1",
        );
        assert_eq!(message, "the key `name` appears twice in this mapping");
        assert!(help.is_some());
    }
}
