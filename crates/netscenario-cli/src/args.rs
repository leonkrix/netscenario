//! The options of a command: `--name value`, `--name=value`, flags without a value, and the words that are
//! not options. A small parser, so that the command line has no dependency and every message is ours.

use std::collections::{HashMap, HashSet};

/// What a command accepts.
pub(crate) struct Spec {
    /// Options that take a value.
    pub(crate) values: &'static [&'static str],
    /// Options without a value.
    pub(crate) flags: &'static [&'static str],
}

/// What was given.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    /// The words that are not options, in order.
    pub(crate) positionals: Vec<String>,
    values: HashMap<String, String>,
    flags: HashSet<String>,
    /// Whether `-h` or `--help` was given.
    pub(crate) help: bool,
}

impl Parsed {
    /// Reads `args` against `spec`.
    ///
    /// # Errors
    ///
    /// A sentence that says what is wrong: an unknown option (with the known ones), a missing value, an option
    /// given twice.
    pub(crate) fn parse<S: AsRef<str>>(args: &[S], spec: &Spec) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut args = args.iter().map(AsRef::as_ref);
        while let Some(arg) = args.next() {
            if arg == "-h" || arg == "--help" {
                parsed.help = true;
                continue;
            }
            let Some(option) = arg.strip_prefix("--") else {
                parsed.positionals.push(arg.to_owned());
                continue;
            };
            let (name, inline) = match option.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (option, None),
            };
            if spec.flags.contains(&name) {
                if inline.is_some() {
                    return Err(format!("the option --{name} takes no value"));
                }
                if !parsed.flags.insert(name.to_owned()) {
                    return Err(format!("the option --{name} is given twice"));
                }
            } else if spec.values.contains(&name) {
                let value = match inline {
                    Some(value) => value,
                    None => args
                        .next()
                        .ok_or_else(|| format!("the option --{name} needs a value"))?
                        .to_owned(),
                };
                if parsed.values.insert(name.to_owned(), value).is_some() {
                    return Err(format!("the option --{name} is given twice"));
                }
            } else {
                let known: Vec<String> = spec
                    .values
                    .iter()
                    .chain(spec.flags)
                    .map(|n| format!("--{n}"))
                    .collect();
                return Err(format!(
                    "unknown option '{arg}'; the options are: {}",
                    known.join(", ")
                ));
            }
        }
        Ok(parsed)
    }

    pub(crate) fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// The one word that is not an option, named `what` in the message if it is missing or if there are more.
    pub(crate) fn single(&self, what: &str) -> Result<&str, String> {
        match self.positionals.as_slice() {
            [one] => Ok(one),
            [] => Err(format!("{what} is missing")),
            [_, extra, ..] => Err(format!("unexpected extra argument '{extra}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: Spec = Spec {
        values: &["out", "seed"],
        flags: &["json"],
    };

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        Parsed::parse(args, &SPEC)
    }

    #[test]
    fn values_flags_and_words_are_told_apart() {
        let parsed = parse(&["file.yaml", "--out", "dir", "--seed=7", "--json"]).unwrap();
        assert_eq!(parsed.positionals, ["file.yaml"]);
        assert_eq!(parsed.value("out"), Some("dir"));
        assert_eq!(parsed.value("seed"), Some("7"));
        assert!(parsed.flag("json") && !parsed.help);
        assert_eq!(parsed.value("missing"), None);
        assert!(!parsed.flag("missing"));
    }

    #[test]
    fn options_may_come_before_the_word() {
        let parsed = parse(&["--json", "file.yaml"]).unwrap();
        assert_eq!(parsed.single("a file").unwrap(), "file.yaml");
    }

    /// Canary: every wrong command line is refused with a sentence that says what is wrong.
    #[test]
    fn wrong_command_lines_say_what_is_wrong() {
        for (args, expected) in [
            (
                &["--verbose"][..],
                "unknown option '--verbose'; the options are: --out, --seed, --json",
            ),
            (&["--out"][..], "the option --out needs a value"),
            (
                &["--out", "a", "--out", "b"][..],
                "the option --out is given twice",
            ),
            (
                &["--json", "--json"][..],
                "the option --json is given twice",
            ),
            (&["--json=yes"][..], "the option --json takes no value"),
        ] {
            assert_eq!(parse(args).unwrap_err(), expected, "{args:?}");
        }
    }

    #[test]
    fn exactly_one_word_is_required_when_asked_for() {
        assert_eq!(
            parse(&[]).unwrap().single("a file").unwrap_err(),
            "a file is missing"
        );
        assert_eq!(
            parse(&["a", "b"]).unwrap().single("a file").unwrap_err(),
            "unexpected extra argument 'b'"
        );
    }

    #[test]
    fn help_is_recognised_anywhere() {
        assert!(parse(&["x", "--help"]).unwrap().help);
        assert!(parse(&["-h"]).unwrap().help);
    }
}
