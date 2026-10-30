//! Where every value of a file is.
//!
//! The YAML library reports an error with the line and column of the place where it failed, but it does not
//! tell where a value that was read correctly came from, and a check of the contents (a node that does not
//! exist, a stop before the start) needs that. This module reads the file once more with the parser that the
//! YAML library is built on, only to note the position of every key and every value under its path.
//!
//! It also finds what the format refuses in YAML itself: anchors, aliases, tags and merge keys, and nesting
//! that is too deep.

use std::collections::HashMap;

use granit_parser::{Event, Parser, Span};

use crate::error::{Location, Problem};

/// How deep the nesting of a file may be.
const MAX_DEPTH: usize = 32;

const REUSE_HELP: &str =
    "write the value out instead; reuse comes with parameters and includes in release 0.3";

/// What kind of node a value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Scalar,
    Mapping,
    Sequence,
}

/// The position of a key and of the value under it.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// Position and length of the key, if the value is the value of a mapping entry.
    pub(crate) key: Option<(Location, usize)>,
    /// Position and length of the value (the whole text of a scalar, the first character of a collection).
    pub(crate) value: (Location, usize),
    pub(crate) kind: Kind,
    /// The text of a scalar.
    pub(crate) text: Option<String>,
}

/// The positions in a file, by path.
#[derive(Debug, Default)]
pub(crate) struct SourceIndex {
    entries: HashMap<String, Entry>,
    /// The file has a syntax error, so the index stops where the parser did and is not complete.
    broken: bool,
}

impl SourceIndex {
    #[cfg(test)]
    pub(crate) fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.get(path)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether the parser stopped at a syntax error. The checks of the contents are skipped then, because they
    /// would report keys as missing that are only beyond the error.
    pub(crate) fn is_broken(&self) -> bool {
        self.broken
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &Entry)> {
        self.entries.iter()
    }

    /// The position of the value at `path`, or of the nearest enclosing value that has one.
    pub(crate) fn locate(&self, path: &str) -> Option<(Location, usize)> {
        let mut current = path.to_owned();
        loop {
            if let Some(entry) = self.entries.get(&current) {
                return Some(entry.value);
            }
            if current.is_empty() {
                return None;
            }
            current = parent(&current).to_owned();
        }
    }
}

/// The path of the enclosing value: `a.b[2].c` has the parent `a.b[2]`, and `a.b[2]` has `a.b`.
pub(crate) fn parent(path: &str) -> &str {
    if path.ends_with(']') {
        path.rfind('[').map_or("", |at| &path[..at])
    } else {
        path.rfind(['.', '[']).map_or("", |at| &path[..at])
    }
}

/// A path with every index replaced by `[]`, to match rules for all elements of a list.
pub(crate) fn generic(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut in_index = false;
    for c in path.chars() {
        match c {
            '[' => {
                in_index = true;
                out.push_str("[]");
            }
            ']' => in_index = false,
            _ if in_index => {}
            _ => out.push(c),
        }
    }
    out
}

enum Frame {
    Mapping {
        path: String,
        pending_key: Option<(String, Location, usize)>,
    },
    Sequence {
        path: String,
        next: usize,
    },
}

fn location(span: &Span) -> Location {
    Location {
        line: span.start.line(),
        column: span.start.col() + 1,
    }
}

fn length(span: &Span) -> usize {
    if span.start.line() == span.end.line() {
        span.end.col().saturating_sub(span.start.col()).max(1)
    } else {
        1
    }
}

fn child_key(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_owned()
    } else {
        format!("{parent}.{key}")
    }
}

/// Walks the events of a file and builds the index.
#[derive(Default)]
struct Indexer {
    index: SourceIndex,
    problems: Vec<Problem>,
    stack: Vec<Frame>,
    too_deep_reported: bool,
}

impl Indexer {
    fn refuse(&mut self, what: &str, help: &str, at: (Location, usize)) {
        self.problems.push(
            Problem::new(format!("{what} are not part of the scenario format"))
                .with_help(help)
                .at_location(at.0, at.1),
        );
    }

    fn refuse_features(&mut self, anchor: usize, has_tag: bool, at: (Location, usize)) {
        if anchor != 0 {
            self.refuse("anchors", REUSE_HELP, at);
        }
        if has_tag {
            self.refuse(
                "tags",
                "remove the tag; the type of a value follows from where it is",
                at,
            );
        }
    }

    /// The path of the node that starts now, and the position of its key if it is the value of an entry.
    fn next_node(&mut self) -> (String, Option<(Location, usize)>) {
        match self.stack.last_mut() {
            None => (String::new(), None),
            Some(Frame::Sequence { path, next }) => {
                let child = format!("{path}[{next}]");
                *next += 1;
                (child, None)
            }
            Some(Frame::Mapping { path, pending_key }) => match pending_key.take() {
                Some((key, at, len)) => (child_key(path, &key), Some((at, len))),
                None => (path.clone(), None),
            },
        }
    }

    fn scalar(&mut self, value: &str, anchor: usize, has_tag: bool, at: (Location, usize)) {
        self.refuse_features(anchor, has_tag, at);
        // A scalar is a key if it comes where a mapping expects one
        if let Some(Frame::Mapping {
            pending_key: pending @ None,
            ..
        }) = self.stack.last_mut()
        {
            *pending = Some((value.to_owned(), at.0, at.1));
            if value == "<<" {
                self.refuse("merge keys (<<)", REUSE_HELP, at);
            }
            return;
        }
        let (path, key) = self.next_node();
        self.index.entries.insert(
            path,
            Entry {
                key,
                value: at,
                kind: Kind::Scalar,
                text: Some(value.to_owned()),
            },
        );
    }

    fn collection(&mut self, kind: Kind, anchor: usize, has_tag: bool, at: (Location, usize)) {
        self.refuse_features(anchor, has_tag, at);
        let (path, key) = self.next_node();
        self.index.entries.insert(
            path.clone(),
            Entry {
                key,
                value: at,
                kind,
                text: None,
            },
        );
        self.stack.push(match kind {
            Kind::Mapping => Frame::Mapping {
                path,
                pending_key: None,
            },
            _ => Frame::Sequence { path, next: 0 },
        });
        if self.stack.len() > MAX_DEPTH && !self.too_deep_reported {
            self.too_deep_reported = true;
            self.problems.push(
                Problem::new(format!(
                    "the file is nested more than {MAX_DEPTH} levels deep"
                ))
                .with_help("a scenario does not need that much nesting")
                .at_location(at.0, at.1),
            );
        }
    }
}

/// Reads `text` and returns the positions and the refusals of YAML features. A syntax error ends the reading
/// without a problem: the YAML library reports it with a better message.
pub(crate) fn index(text: &str) -> (SourceIndex, Vec<Problem>) {
    let mut indexer = Indexer::default();
    for item in Parser::new_from_str(text) {
        let Ok((event, span)) = item else {
            indexer.index.broken = true;
            break;
        };
        let at = (location(&span), length(&span));
        match event {
            Event::Alias(_) => indexer.refuse("aliases", REUSE_HELP, at),
            Event::Scalar(value, _style, anchor, tag) => {
                indexer.scalar(&value, anchor, tag.is_some(), at);
            }
            Event::MappingStart(_, anchor, tag) => {
                indexer.collection(Kind::Mapping, anchor, tag.is_some(), at);
            }
            Event::SequenceStart(_, anchor, tag) => {
                indexer.collection(Kind::Sequence, anchor, tag.is_some(), at);
            }
            Event::MappingEnd | Event::SequenceEnd => {
                indexer.stack.pop();
            }
            _ => {}
        }
    }
    (indexer.index, indexer.problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "version: 0\nname: t\ntopology:\n  nodes: [client, server]\n  links:\n    - id: a\n      between: [client, server]\n";

    #[test]
    fn every_value_is_found_by_its_path() {
        let (index, problems) = index(FILE);
        assert_eq!(problems, Vec::new());
        let entry = index.get("topology.links[0].between[1]").unwrap();
        assert_eq!(entry.text.as_deref(), Some("server"));
        assert_eq!(
            entry.value.0,
            Location {
                line: 7,
                column: 25
            }
        );
        assert_eq!(entry.value.1, 6);
        let key = index.get("topology.links[0].between").unwrap().key.unwrap();
        assert_eq!(key.0, Location { line: 7, column: 7 });
        assert_eq!(
            index.get("name").unwrap().value.0,
            Location { line: 2, column: 7 }
        );
    }

    #[test]
    fn the_nearest_value_with_a_position_is_used() {
        let (index, _) = index(FILE);
        assert_eq!(
            index.locate("topology.links[0].between[9]").unwrap().0,
            Location {
                line: 7,
                column: 16
            }
        );
        assert_eq!(
            index.locate("nothing.at.all").unwrap().0,
            Location { line: 1, column: 1 }
        );
    }

    #[test]
    fn paths_have_parents_and_generic_forms() {
        assert_eq!(parent("a.b[2].c"), "a.b[2]");
        assert_eq!(parent("a.b[2]"), "a.b");
        assert_eq!(parent("a"), "");
        assert_eq!(
            generic("topology.links[3].forward.delay"),
            "topology.links[].forward.delay"
        );
    }

    #[test]
    fn anchors_aliases_tags_and_merge_keys_are_refused() {
        for (text, what) in [
            ("a: &x 1\n", "anchors"),
            ("a: &x 1\nb: *x\n", "aliases"),
            ("a: !!str 1\n", "tags"),
            ("base: {x: 1}\nb:\n  <<: {y: 2}\n", "merge keys"),
        ] {
            let (_, problems) = index(text);
            assert!(
                problems.iter().any(|p| p.message.contains(what)),
                "{text:?}: {problems:?}"
            );
        }
    }

    #[test]
    fn deep_nesting_is_refused() {
        let text = format!("{}1{}", "[".repeat(40), "]".repeat(40));
        let (_, problems) = index(&text);
        assert!(
            problems
                .iter()
                .any(|p| p.message.contains("nested more than 32"))
        );
    }

    #[test]
    fn a_file_without_content_has_an_empty_index() {
        assert!(index("# only a comment\n").0.is_empty());
    }
}
