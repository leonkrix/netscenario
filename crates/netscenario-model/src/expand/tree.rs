//! The YAML tree of a file with the place of every node, and the text of a tree.
//!
//! The pass that expands parameters and includes (D-093) works on this tree and not on the typed scenario, because what it
//! moves around are the nodes of a file; each keeps where it was written so that an error in an expanded scenario can name the
//! file and the line of the template that made the faulty value.

use granit_parser::{Event, Parser, ScalarStyle};

use crate::error::Location;
use crate::source::{length, location};

/// Where a node was written: the file, the position, and the places of use that led to it (an index into the chains of the
/// expansion; 0 is none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) file: u32,
    pub(crate) at: Location,
    pub(crate) length: u32,
    pub(crate) chain: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub(crate) body: Body,
    pub(crate) place: Place,
}

#[derive(Debug, Clone)]
pub(crate) enum Body {
    Scalar { text: String, quoted: bool },
    Sequence(Vec<Node>),
    Mapping(Vec<Pair>),
}

#[derive(Debug, Clone)]
pub(crate) struct Pair {
    pub(crate) key: String,
    pub(crate) key_place: Place,
    pub(crate) value: Node,
}

impl Node {
    pub(crate) fn scalar(text: impl Into<String>, quoted: bool, place: Place) -> Self {
        Self {
            body: Body::Scalar {
                text: text.into(),
                quoted,
            },
            place,
        }
    }

    pub(crate) fn null(place: Place) -> Self {
        Self::scalar("", false, place)
    }

    /// The value of a key of a mapping.
    pub(crate) fn get(&self, key: &str) -> Option<&Pair> {
        match &self.body {
            Body::Mapping(pairs) => pairs.iter().find(|pair| pair.key == key),
            _ => None,
        }
    }
}

/// Why a file is not a tree.
#[derive(Debug)]
pub(crate) struct ParseFailure {
    pub(crate) message: String,
    pub(crate) at: Location,
    pub(crate) length: usize,
}

enum Frame {
    Sequence(Place, Vec<Node>),
    Mapping(Place, Vec<Pair>, Option<(String, Place)>),
}

/// Reads `text` as one YAML document. Anchors, aliases and tags are not looked for here: the reader of the format refuses them
/// before (`source::index`).
pub(crate) fn parse(text: &str, file: u32) -> Result<Node, ParseFailure> {
    let place = |at: (Location, usize)| Place {
        file,
        at: at.0,
        length: u32::try_from(at.1).unwrap_or(u32::MAX),
        chain: 0,
    };
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Node> = None;
    let mut documents = 0;
    let failure = |message: &str, at: (Location, usize)| ParseFailure {
        message: message.to_owned(),
        at: at.0,
        length: at.1,
    };
    for item in Parser::new_from_str(text) {
        let (event, span) = item.map_err(|error| ParseFailure {
            message: error.info(),
            at: Location {
                line: error.marker().line(),
                column: error.marker().col() + 1,
            },
            length: 1,
        })?;
        let at = (location(&span), length(&span));
        let finished = match event {
            Event::DocumentStart(..) => {
                documents += 1;
                if documents > 1 {
                    return Err(failure("the file has more than one YAML document", at));
                }
                None
            }
            Event::Alias(_) => {
                return Err(failure("aliases are not part of the scenario format", at));
            }
            Event::Scalar(value, style, _, _) => {
                if let Some(Frame::Mapping(_, _, pending @ None)) = stack.last_mut() {
                    *pending = Some((value.into_owned(), place(at)));
                    None
                } else {
                    // An empty value is read as `~` by the parser; both are the null of the file
                    let plain = style == ScalarStyle::Plain;
                    let text = if plain && matches!(&*value, "~" | "null") {
                        String::new()
                    } else {
                        value.into_owned()
                    };
                    Some(Node::scalar(text, !plain, place(at)))
                }
            }
            Event::SequenceStart(..) | Event::MappingStart(..) => {
                if let Some(Frame::Mapping(_, _, None)) = stack.last() {
                    return Err(failure("a key must be a single word", at));
                }
                stack.push(if matches!(event, Event::SequenceStart(..)) {
                    Frame::Sequence(place(at), Vec::new())
                } else {
                    Frame::Mapping(place(at), Vec::new(), None)
                });
                None
            }
            Event::SequenceEnd => match stack.pop() {
                Some(Frame::Sequence(place, items)) => Some(Node {
                    body: Body::Sequence(items),
                    place,
                }),
                _ => return Err(failure("the file is not well formed", at)),
            },
            Event::MappingEnd => match stack.pop() {
                Some(Frame::Mapping(place, pairs, _)) => Some(Node {
                    body: Body::Mapping(pairs),
                    place,
                }),
                _ => return Err(failure("the file is not well formed", at)),
            },
            _ => None,
        };
        if let Some(node) = finished {
            match stack.last_mut() {
                None => root = Some(node),
                Some(Frame::Sequence(_, items)) => items.push(node),
                Some(Frame::Mapping(_, pairs, pending)) => {
                    let Some((key, key_place)) = pending.take() else {
                        return Err(failure("the file is not well formed", at));
                    };
                    pairs.push(Pair {
                        key,
                        key_place,
                        value: node,
                    });
                }
            }
        }
    }
    root.ok_or_else(|| ParseFailure {
        message: "the file has no content".to_owned(),
        at: Location { line: 1, column: 1 },
        length: 1,
    })
}

/// Whether `text` is a number that YAML reads as a number when it is not in quotes.
fn is_number(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
    !whole.is_empty()
        && !fraction.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
}

/// Whether a plain scalar with this text reads back as the same text (and the same type) when it is written without quotes.
fn plain_is_safe(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    if is_number(text) || matches!(text, "true" | "false" | "null" | "~") {
        return true;
    }
    !(first.is_whitespace()
        || text.ends_with(char::is_whitespace)
        || "-?:,[]{}#&*!|>'\"%@`".contains(first)
        || text.contains(": ")
        || text.contains(" #")
        || text.ends_with(':')
        || text.chars().any(char::is_control))
}

fn scalar_text(text: &str, quoted: bool) -> String {
    if text.is_empty() && !quoted {
        return "null".to_owned();
    }
    if !quoted && plain_is_safe(text) {
        return text.to_owned();
    }
    // JSON strings are YAML double-quoted scalars
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

fn key_text(key: &str) -> String {
    if plain_is_safe(key) && !matches!(key, "true" | "false" | "null" | "~") {
        key.to_owned()
    } else {
        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_owned())
    }
}

fn write(node: &Node, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match &node.body {
        Body::Scalar { text, quoted } => {
            out.push_str(&pad);
            out.push_str(&scalar_text(text, *quoted));
            out.push('\n');
        }
        Body::Sequence(items) => {
            for item in items {
                out.push_str(&pad);
                out.push('-');
                write_value(item, indent + 2, out);
            }
        }
        Body::Mapping(pairs) => {
            for pair in pairs {
                out.push_str(&pad);
                out.push_str(&key_text(&pair.key));
                out.push(':');
                write_value(&pair.value, indent + 2, out);
            }
        }
    }
}

/// What follows `key:` or `-`: a scalar on the same line, a collection below, an empty one in the flow form.
fn write_value(node: &Node, indent: usize, out: &mut String) {
    match &node.body {
        Body::Scalar { text, quoted } => {
            out.push(' ');
            out.push_str(&scalar_text(text, *quoted));
            out.push('\n');
        }
        Body::Sequence(items) if items.is_empty() => out.push_str(" []\n"),
        Body::Mapping(pairs) if pairs.is_empty() => out.push_str(" {}\n"),
        _ => {
            out.push('\n');
            write(node, indent, out);
        }
    }
}

/// The tree as the text of a file that reads back as the same tree.
pub(crate) fn to_text(root: &Node) -> String {
    let mut out = String::new();
    match &root.body {
        Body::Sequence(items) if items.is_empty() => out.push_str("[]\n"),
        Body::Mapping(pairs) if pairs.is_empty() => out.push_str("{}\n"),
        _ => write(root, 0, &mut out),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(text: &str) -> Node {
        parse(text, 0).unwrap()
    }

    #[test]
    fn a_file_is_read_with_the_places_of_its_nodes() {
        let root = tree("a: 1\nb:\n  - x\n  - \"y z\"\n");
        let b = root.get("b").unwrap();
        assert_eq!(b.key_place.at, Location { line: 2, column: 1 });
        let Body::Sequence(items) = &b.value.body else {
            panic!("a sequence")
        };
        assert_eq!(items[1].place.at, Location { line: 4, column: 5 });
        assert!(matches!(&items[1].body, Body::Scalar { text, quoted: true } if text == "y z"));
        assert!(matches!(&items[0].body, Body::Scalar { quoted: false, .. }));
    }

    #[test]
    fn what_is_written_reads_back_as_the_same_tree() {
        let text = "version: 0\nname: t\nflag: true\nn: -5\nx: 0.5\nempty:\nlist:\n  - a\n  -\n    k: v\n    l:\n      - 1\n      - []\n  - {}\nquoted: \"10\"\nodd: \"a: b\"\nmulti: |\n  one\n  two\n";
        let first = tree(text);
        let written = to_text(&first);
        let second = tree(&written);
        assert_eq!(to_text(&second), written);
        // Types survive: the quoted number stays a string, the plain one a number
        let reread = |key: &str| match &second.get(key).unwrap().value.body {
            Body::Scalar { text, quoted } => (text.clone(), *quoted),
            _ => panic!("a scalar"),
        };
        assert_eq!(reread("quoted"), ("10".to_owned(), true));
        assert_eq!(reread("n"), ("-5".to_owned(), false));
        assert_eq!(reread("odd"), ("a: b".to_owned(), true));
        assert_eq!(reread("multi"), ("one\ntwo\n".to_owned(), true));
        assert_eq!(reread("empty"), (String::new(), false));
    }

    #[test]
    fn the_text_of_a_tree_is_a_valid_scenario_file_for_the_typed_reader() {
        let text = "version: 0\nname: t\nduration: 5s\ntopology:\n  nodes: [client, server]\n  links:\n    - id: a\n      between: [client, server]\n      both: { delay: 20ms }\nworkloads:\n  - { id: m, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }\n";
        let written = to_text(&tree(text));
        assert_eq!(
            crate::Scenario::from_yaml(&written, "w.yaml").unwrap(),
            crate::Scenario::from_yaml(text, "t.yaml").unwrap()
        );
    }

    #[test]
    fn syntax_errors_and_odd_files_are_reported_with_a_place() {
        let error = parse("a: [1, 2\nb: 3\n", 0).unwrap_err();
        assert!(error.at.line >= 1, "{error:?}");
        assert!(
            parse("a: 1\n---\nb: 2\n", 0)
                .unwrap_err()
                .message
                .contains("more than one")
        );
        assert!(
            parse("? [a]\n: 1\n", 0)
                .unwrap_err()
                .message
                .contains("single word")
        );
        assert!(parse("", 0).is_err());
    }
}
