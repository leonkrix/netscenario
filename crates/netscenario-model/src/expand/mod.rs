//! Parameters, templates and includes (SCN-05, D-093).
//!
//! A file is turned into the scenario it describes by a pass over its YAML tree, before the checks of the contents. The pass
//! replaces the expressions `"{...}"` by their values, the places of use (`use: name` or `include: path`, with `with:`) by the
//! content of a template or of a file, and writes the result as the text of an ordinary scenario file. Every node of that text
//! remembers where it was written, so a problem found later is shown at the line of the template and the places of use that
//! led to it.

mod expr;
mod tree;

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use crate::error::{Error, Location, Problem};
use crate::read::MAX_FILE_BYTES;
use crate::source;
use crate::suggest::closest;
use crate::units::Name;

use expr::Binding;
use tree::{Body, Node, Pair, Place};

/// Templates and includes may be used inside each other this deep.
pub const MAX_USE_DEPTH: usize = 8;

/// A scenario may have this many places of use in all, the uses inside templates included.
pub const MAX_USES: usize = 10_000;

/// The expanded scenario may have this many nodes.
pub const MAX_EXPANDED_NODES: usize = 1_000_000;

/// Problems are reported up to this number; after that the pass stops.
const MAX_PROBLEMS: usize = 50;

/// Where the files that a scenario includes are read from.
pub trait FileSource {
    /// Reads the file at `path`, which is relative to the directory of the scenario file and may lead out of it with `..`;
    /// whether that is allowed is up to the source (D-083).
    ///
    /// # Errors
    ///
    /// A sentence that says why the file cannot be used. It names the path as the scenario wrote it.
    fn read(&self, path: &str) -> Result<String, String>;
}

/// What a scenario file is read with: the values of its parameters and where its includes are found.
#[derive(Default)]
#[non_exhaustive]
pub struct LoadOptions<'a> {
    /// The values given for parameters, in the order they were given.
    pub params: Vec<(String, String)>,
    /// Where `include:` finds files. Without it a scenario that includes a file is refused.
    pub files: Option<&'a dyn FileSource>,
}

impl<'a> LoadOptions<'a> {
    /// Options with no parameters and no files.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The same options with a value for the parameter `name`. The value is read like a scalar of the file: `20ms`, `4`, `true`.
    #[must_use]
    pub fn param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.push((name.into(), value.into()));
        self
    }

    /// The same options with the source of the included files.
    #[must_use]
    pub fn files(mut self, files: &'a dyn FileSource) -> Self {
        self.files = Some(files);
        self
    }
}

/// A file that took part in a scenario: the name it is shown with and its text.
#[derive(Debug)]
struct SourceFile {
    name: String,
    text: String,
}

/// A place of use that led to a node: where it is, and what it uses.
#[derive(Debug)]
struct Use {
    place: Place,
    what: String,
}

/// The files that took part in a scenario and the chains of uses that led to its nodes.
#[derive(Debug)]
struct Registry {
    files: Vec<SourceFile>,
    /// The chains of uses, outermost first; chain 0 is empty.
    chains: Vec<Vec<Use>>,
}

impl Registry {
    fn file_name(&self, file: u32) -> &str {
        self.files
            .get(file as usize)
            .map_or("", |file| file.name.as_str())
    }

    /// Puts a problem at `place`: the file, the position, and the places of use that led there.
    fn place(&self, problem: Problem, place: Place) -> Problem {
        problem
            .at_location(place.at, place.length as usize)
            .written_in(
                (place.file != 0).then(|| self.file_name(place.file).to_owned()),
                self.chain_lines(place.chain),
            )
    }

    fn chain_lines(&self, chain: u32) -> Vec<String> {
        self.chains
            .get(chain as usize)
            .map(|uses| {
                uses.iter()
                    .rev()
                    .map(|used| {
                        format!(
                            "used from {}:{}:{} ({})",
                            self.file_name(used.place.file),
                            used.place.at.line,
                            used.place.at.column,
                            used.what
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Where the nodes of an expanded scenario were written. A scenario read from a file that used parameters or includes keeps
/// it, to show a problem that a backend finds later at the right line.
#[derive(Debug)]
pub(crate) struct Provenance {
    registry: Registry,
    /// The text of the expanded scenario, which is what was checked.
    expanded: String,
    /// The origin of what stands at a position of the expanded text.
    places: HashMap<Location, Place>,
}

impl Provenance {
    /// The files other than the first, with their text.
    pub(crate) fn other_sources(&self) -> Vec<(String, String)> {
        other_sources(&self.registry.files)
    }

    pub(crate) fn expanded(&self) -> &str {
        &self.expanded
    }

    /// Moves a problem that has a position in the expanded text to where its value was written.
    pub(crate) fn apply(&self, problem: Problem) -> Problem {
        match problem
            .location
            .and_then(|at| self.places.get(&at))
            .copied()
        {
            Some(place) => self.registry.place(problem, place),
            None => problem,
        }
    }
}

fn other_sources(files: &[SourceFile]) -> Vec<(String, String)> {
    files
        .iter()
        .skip(1)
        .map(|file| (file.name.clone(), file.text.clone()))
        .collect()
}

/// The origin table that a scenario keeps. It is not part of the scenario: it is not compared, written or hashed.
#[derive(Clone, Default)]
pub(crate) struct Origins(pub(crate) Option<Arc<Provenance>>);

impl std::fmt::Debug for Origins {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(if self.0.is_some() {
            "Origins(..)"
        } else {
            "Origins"
        })
    }
}

impl PartialEq for Origins {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Origins {}

/// The result of the pass: the text of the scenario and where its nodes came from.
pub(crate) struct Expansion {
    pub(crate) text: String,
    pub(crate) provenance: Provenance,
}

/// One parameter of a scenario or of a template.
struct Declaration {
    name: String,
    /// The default: a scalar of the file, or `None` for a parameter that must be given.
    default: Option<(String, bool)>,
    place: Place,
}

/// A template: what a place of use is replaced by.
struct Template {
    /// What it is called in a message: the template or the file.
    what: String,
    params: Vec<Declaration>,
    content: Node,
    /// The file it is written in; its templates are the ones that its content can use.
    file: u32,
}

/// The values a node of a template can use.
struct Bindings {
    values: BTreeMap<String, Binding>,
    /// Parameters that are declared and have no value because the binding failed; the failure is reported once.
    unbound: Vec<String>,
}

impl expr::Scope for Bindings {
    fn is_unbound(&self, name: &str) -> bool {
        self.unbound.iter().any(|unbound| unbound == name)
    }

    fn lookup(&self, name: &str) -> Option<&Binding> {
        self.values.get(name)
    }

    fn names(&self) -> Vec<&str> {
        self.values.keys().map(String::as_str).collect()
    }
}

/// A file that was read: its templates and, if it was included, the template that it is.
struct FileData {
    templates: HashMap<String, Rc<Template>>,
    main: Option<Rc<Template>>,
    /// The directory of the file below the directory of the scenario, with `/` and no trailing one.
    directory: String,
}

struct Failure {
    message: String,
    help: Option<String>,
    place: Option<Place>,
}

struct Expander<'a> {
    options: &'a LoadOptions<'a>,
    registry: Registry,
    data: Vec<Option<FileData>>,
    by_path: HashMap<String, u32>,
    /// What is being expanded, innermost last: the identity, to find a cycle, and the name for the message.
    active: Vec<(String, String)>,
    failures: Vec<Failure>,
    uses: usize,
    nodes: usize,
    stopped: bool,
}

fn valid_parameter_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(|c| c.is_ascii_lowercase())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && name.len() <= 64
}

/// A place of use: a mapping with `use` or `include`.
fn is_use(node: &Node) -> bool {
    node.get("use").is_some() || node.get("include").is_some()
}

/// The text between the braces of a scalar that is an expression `{...}` as a whole. `{{...}}` (a brace written twice) and
/// `{node.addr}` (a name, a dot, a word: the address of a node in a command) belong to the commands of the workloads (D-093).
/// An expression inside a longer text is found by `evaluate_scalar`.
fn expression_of(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('{')?.strip_suffix('}')?;
    is_candidate(inner).then_some(inner)
}

/// Whether the text between braces can be an expression, as far as the characters go. Other text (`{"a": 1}`, a brace inside) is
/// not ours and stays for the commands, whose message is better for it.
fn is_candidate(inner: &str) -> bool {
    !inner.is_empty()
        && !is_placeholder(inner)
        && inner.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | ' ' | '+' | '-' | '*' | '/' | '(' | ')' | '.' | '%')
        })
}

/// Whether `inner` is a placeholder of a command: `server.addr`, `edge-1.addr`. An expression has no dot after a letter.
fn is_placeholder(inner: &str) -> bool {
    inner.starts_with(|c: char| c.is_ascii_lowercase())
        && inner.contains('.')
        && inner
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// Whether `text` has a `{...}` in it that the pass would take for an expression (a brace written twice and a placeholder of a
/// command are not).
fn has_expression(text: &str) -> bool {
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        rest = &rest[open..];
        if let Some(escaped) = rest.strip_prefix("{{") {
            rest = escaped;
            continue;
        }
        let Some(close) = rest.find('}') else {
            return false;
        };
        if is_candidate(&rest[1..close]) {
            return true;
        }
        rest = &rest[close + 1..];
    }
    false
}

/// Where a problem with a value is shown: at the use if there is one, else at the declaration.
fn place_of_value(_value: &Binding, site: Option<Place>, declaration: Place) -> Option<Place> {
    site.or(Some(declaration))
}

/// Whether anything in the tree needs the pass.
fn needs_pass(node: &Node) -> bool {
    match &node.body {
        Body::Scalar { text, .. } => text.contains('{'),
        Body::Sequence(items) => items.iter().any(needs_pass),
        Body::Mapping(pairs) => is_use(node) || pairs.iter().any(|pair| needs_pass(&pair.value)),
    }
}

/// `directory` and `reference` joined, with `.` and `..` resolved as far as the names allow. A `..` that leaves the directory
/// of the scenario stays at the front; the source decides whether that is allowed.
fn join_path(directory: &str, reference: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let absolute = reference.starts_with('/');
    let start = if absolute { "" } else { directory };
    for part in start.split('/').chain(reference.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

impl Expander<'_> {
    fn fail(&mut self, message: impl Into<String>, help: Option<String>, place: Option<Place>) {
        if self.failures.len() >= MAX_PROBLEMS {
            self.stopped = true;
            return;
        }
        self.failures.push(Failure {
            message: message.into(),
            help,
            place,
        });
    }

    fn fail_expr(&mut self, error: expr::ExprError, place: Place) {
        if !error.silent {
            self.fail(error.message, error.help, Some(place));
        }
    }

    fn with_chain(place: Place, chain: u32) -> Place {
        Place { chain, ..place }
    }

    /// Reads the declarations of a `params:` mapping.
    fn declarations(&mut self, node: &Node, chain: u32) -> Vec<Declaration> {
        let mut out: Vec<Declaration> = Vec::new();
        match &node.body {
            Body::Mapping(pairs) => {
                for pair in pairs {
                    let place = Self::with_chain(pair.key_place, chain);
                    if !valid_parameter_name(&pair.key) {
                        self.fail(
                            format!("`{}` is not a name of a parameter", pair.key),
                            Some("a parameter has lower case letters, digits and `_`, and starts with a letter (a `-` would be a minus in an expression)".to_owned()),
                            Some(place),
                        );
                        continue;
                    }
                    if out.iter().any(|d| d.name == pair.key) {
                        self.fail(format!("the parameter `{}` is declared twice", pair.key), None, Some(place));
                        continue;
                    }
                    let default = match &pair.value.body {
                        Body::Scalar { text, quoted } if text.is_empty() && !quoted => None,
                        Body::Scalar { text, quoted } => Some((text.clone(), *quoted)),
                        _ => {
                            self.fail(
                                format!("the default of `{}` has to be a single value", pair.key),
                                Some("a parameter is a number, a quantity, a word or true or false".to_owned()),
                                Some(Self::with_chain(pair.value.place, chain)),
                            );
                            continue;
                        }
                    };
                    out.push(Declaration {
                        name: pair.key.clone(),
                        default,
                        place,
                    });
                }
            }
            Body::Scalar { text, quoted: false } if text.is_empty() => {}
            _ => self.fail(
                "`params` has to be a mapping of names to defaults",
                Some("write `params:` and below it `name: default`, or `name:` for a value that must be given".to_owned()),
                Some(Self::with_chain(node.place, chain)),
            ),
        }
        out
    }

    /// The value of a scalar of a file or template: the scalar itself; the value of the expression `{...}` that it is as a whole;
    /// or the text with each expression in it replaced by its value (`link-{n}`). `None` after the problem is reported.
    fn evaluate_scalar(
        &mut self,
        text: &str,
        quoted: bool,
        place: Place,
        scope: &dyn expr::Scope,
    ) -> Option<Binding> {
        if let Some(inner) = expression_of(text) {
            return match expr::evaluate(inner, scope) {
                Ok(binding) => Some(binding),
                Err(error) => {
                    self.fail_expr(error, place);
                    None
                }
            };
        }
        if !text.contains('{') {
            return Some(Binding::from_scalar(text, quoted));
        }
        // Expressions inside a longer text: `link-{n}`, `--rate={rate}`. A brace written twice, and `{node.addr}`, stay.
        let mut out = String::new();
        let mut rest = text;
        let mut failed = false;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            rest = &rest[open..];
            if let Some(escaped) = rest.strip_prefix("{{") {
                out.push_str("{{");
                rest = escaped;
                continue;
            }
            let Some(close) = rest.find('}') else {
                break;
            };
            let inner = &rest[1..close];
            if is_candidate(inner) {
                match expr::evaluate(inner, scope) {
                    Ok(binding) => out.push_str(&binding.written.text),
                    Err(error) => {
                        self.fail_expr(error, place);
                        failed = true;
                    }
                }
            } else {
                out.push_str(&rest[..=close]);
            }
            rest = &rest[close + 1..];
        }
        out.push_str(rest);
        (!failed).then(|| Binding::from_scalar(&out, quoted))
    }

    /// Binds the parameters of a scope: the given values, else the defaults, which may use the parameters before them.
    /// `given` holds the value and the place where it was written, if it was written in a file.
    fn bind(
        &mut self,
        declarations: &[Declaration],
        mut given: Vec<(String, Binding, Option<Place>)>,
        owner: &str,
        site: Option<Place>,
    ) -> Bindings {
        let mut bindings = Bindings {
            values: BTreeMap::new(),
            unbound: Vec::new(),
        };
        for declaration in declarations {
            let default = match &declaration.default {
                Some((text, quoted)) => {
                    self.evaluate_scalar(text, *quoted, declaration.place, &bindings)
                }
                None => None,
            };
            let at = given
                .iter()
                .position(|(name, _, _)| *name == declaration.name);
            let value = match (at.map(|at| given.remove(at)), default) {
                (Some((_, binding, place)), default) => {
                    if let Some(default) = &default
                        && default.value.kind() != binding.value.kind()
                        && default.value.kind() != expr::Kind::Text
                    {
                        self.fail(
                            format!(
                                "`{}` of {owner} is {}, and {} was given",
                                declaration.name,
                                default.value.kind().what(),
                                binding.value.kind().what()
                            ),
                            Some(format!(
                                "the default is `{}`; give a value of the same kind",
                                default.written.text
                            )),
                            place.or(site),
                        );
                    }
                    binding
                }
                (None, Some(default)) => default,
                (None, None) => {
                    bindings.unbound.push(declaration.name.clone());
                    if declaration.default.is_none() {
                        self.fail(
                            format!("{owner} needs a value for `{}`", declaration.name),
                            Some(if site.is_some() {
                                format!(
                                    "give it in `with:`: `with: {{ {}: ... }}`",
                                    declaration.name
                                )
                            } else {
                                format!("give it with --param {}=<value>", declaration.name)
                            }),
                            site.or(Some(declaration.place)),
                        );
                    }
                    continue;
                }
            };
            if has_expression(&value.written.text) {
                self.fail(
                    format!(
                        "the value `{}` of `{}` would be read as an expression again",
                        value.written.text, declaration.name
                    ),
                    Some("the value of a parameter is used as it is and is not calculated; leave out the braces, or write them twice".to_owned()),
                    place_of_value(&value, site, declaration.place),
                );
            }
            bindings.values.insert(declaration.name.clone(), value);
        }
        for (name, _, place) in given {
            let names: Vec<&str> = declarations.iter().map(|d| d.name.as_str()).collect();
            let help = match closest(&name, &names) {
                Some(near) => format!("did you mean `{near}`?"),
                None if names.is_empty() => format!("{owner} declares no parameter"),
                None => format!("it declares {}", names.join(", ")),
            };
            self.fail(
                format!("`{name}` is not a parameter of {owner}"),
                Some(help),
                place.or(site),
            );
        }
        bindings
    }

    /// Reads the `templates:` of a file.
    fn templates(&mut self, node: &Node, file: u32) -> HashMap<String, Rc<Template>> {
        let mut out = HashMap::new();
        let Body::Mapping(pairs) = &node.body else {
            if !matches!(&node.body, Body::Scalar { text, quoted: false } if text.is_empty()) {
                self.fail(
                    "`templates` has to be a mapping of names to templates",
                    Some(
                        "write `templates:` and below it `name:` with `params:` and `content:`"
                            .to_owned(),
                    ),
                    Some(node.place),
                );
            }
            return out;
        };
        for pair in pairs {
            if Name::new(&pair.key).is_err() {
                self.fail(
                    format!("`{}` is not a name of a template", pair.key),
                    Some(
                        "a template has a name like a node: lower case letters, digits and `-`"
                            .to_owned(),
                    ),
                    Some(pair.key_place),
                );
                continue;
            }
            let what = format!("template `{}`", pair.key);
            if let Some(template) = self.template(&pair.value, file, what) {
                out.insert(pair.key.clone(), Rc::new(template));
            }
        }
        out
    }

    /// A `{ params, content }` mapping.
    fn template(&mut self, node: &Node, file: u32, what: String) -> Option<Template> {
        let Body::Mapping(pairs) = &node.body else {
            self.fail(
                "a template is a mapping with `params:` and `content:`",
                None,
                Some(node.place),
            );
            return None;
        };
        for pair in pairs {
            if !matches!(pair.key.as_str(), "params" | "content" | "templates") {
                let help = closest(&pair.key, &["params", "content"])
                    .map(|near| format!("did you mean `{near}`?"));
                self.fail(
                    format!("`{}` is not a key of a template", pair.key),
                    help.or_else(|| Some("a template has `params:` and `content:`".to_owned())),
                    Some(pair.key_place),
                );
            }
        }
        let Some(content) = node.get("content") else {
            self.fail(
                format!("{what} has no `content:`"),
                Some("`content:` is what a place of use is replaced by".to_owned()),
                Some(node.place),
            );
            return None;
        };
        let params = match node.get("params") {
            Some(pair) => self.declarations(&pair.value, 0),
            None => Vec::new(),
        };
        Some(Template {
            what,
            params,
            content: content.value.clone(),
            file,
        })
    }

    /// Reads and parses an included file, once.
    fn load(&mut self, path: &str, site: Place) -> Option<u32> {
        if let Some(&index) = self.by_path.get(path) {
            return Some(index);
        }
        let Some(source) = self.options.files else {
            self.fail(
                format!("cannot include '{path}': this scenario was read from text, and there is no directory to look for it in"),
                Some("read the scenario from its file (`netscenario` does; in Python `Scenario.load(path)`)".to_owned()),
                Some(site),
            );
            return None;
        };
        let text = match source.read(path) {
            Ok(text) => text,
            Err(message) => {
                self.fail(message, None, Some(site));
                return None;
            }
        };
        let index = u32::try_from(self.registry.files.len()).ok()?;
        if text.len() > MAX_FILE_BYTES {
            self.fail(
                format!(
                    "'{path}' is larger than {MAX_FILE_BYTES} bytes ({} bytes)",
                    text.len()
                ),
                Some("a scenario does not need that much text".to_owned()),
                Some(site),
            );
            return None;
        }
        self.registry.files.push(SourceFile {
            name: path.to_owned(),
            text: text.clone(),
        });
        self.data.push(None);
        self.by_path.insert(path.to_owned(), index);
        let root = self.parse_file(&text, index, site)?;
        let directory = path
            .rsplit_once('/')
            .map_or(String::new(), |(directory, _)| directory.to_owned());
        let Body::Mapping(pairs) = &root.body else {
            self.fail(
                format!("'{path}' has to be a mapping with `params:` and `content:`"),
                None,
                Some(root.place),
            );
            return None;
        };
        for pair in pairs {
            if !matches!(pair.key.as_str(), "params" | "templates" | "content") {
                let help = if matches!(
                    pair.key.as_str(),
                    "version" | "topology" | "workloads" | "name"
                ) {
                    "an included file is not a scenario: it holds `params:`, `templates:` and `content:`".to_owned()
                } else {
                    "an included file holds `params:`, `templates:` and `content:`".to_owned()
                };
                self.fail(
                    format!("`{}` is not a key of an included file", pair.key),
                    Some(help),
                    Some(pair.key_place),
                );
            }
        }
        let templates = match root.get("templates") {
            Some(pair) => self.templates(&pair.value, index),
            None => HashMap::new(),
        };
        let main = self
            .template(&root, index, format!("file '{path}'"))
            .map(Rc::new);
        self.data[index as usize] = Some(FileData {
            templates,
            main,
            directory,
        });
        Some(index)
    }

    /// Parses a file and reports what the reader of the format refuses in the YAML itself.
    fn parse_file(&mut self, text: &str, file: u32, site: Place) -> Option<Node> {
        let (_, problems) = source::index(text);
        for problem in problems {
            let place = problem.location.map_or(site, |at| Place {
                file,
                at,
                length: u32::try_from(problem.length).unwrap_or(1),
                chain: 0,
            });
            self.fail(problem.message, problem.help, Some(place));
        }
        match tree::parse(text, file) {
            Ok(root) => Some(root),
            Err(error) => {
                self.fail(
                    error.message,
                    None,
                    Some(Place {
                        file,
                        at: error.at,
                        length: u32::try_from(error.length).unwrap_or(1),
                        chain: 0,
                    }),
                );
                None
            }
        }
    }

    /// Expands a node of the file or template in `scope`; `chain` is the chain of uses that led here.
    fn node(&mut self, node: &Node, scope: &Bindings, file: u32, chain: u32, depth: usize) -> Node {
        let place = Self::with_chain(node.place, chain);
        self.nodes += 1;
        if self.nodes > MAX_EXPANDED_NODES {
            if !self.stopped {
                self.fail(
                    format!("the scenario would have more than {MAX_EXPANDED_NODES} values after the templates are expanded"),
                    None,
                    Some(place),
                );
            }
            self.stopped = true;
        }
        if self.stopped {
            return Node::null(place);
        }
        match &node.body {
            Body::Scalar { text, quoted } => self.scalar(text, *quoted, place, scope),
            Body::Sequence(items) => {
                let mut out = Vec::new();
                for item in items {
                    if is_use(item) {
                        let used = self.use_site(item, scope, file, chain, depth);
                        match used.body {
                            Body::Sequence(spliced) => out.extend(spliced),
                            _ => out.push(used),
                        }
                    } else {
                        out.push(self.node(item, scope, file, chain, depth));
                    }
                }
                Node {
                    body: Body::Sequence(out),
                    place,
                }
            }
            Body::Mapping(pairs) => {
                if is_use(node) {
                    return self.use_site(node, scope, file, chain, depth);
                }
                self.unquoted_expression(pairs, place, scope);
                let pairs = pairs
                    .iter()
                    .map(|pair| Pair {
                        key: pair.key.clone(),
                        key_place: Self::with_chain(pair.key_place, chain),
                        value: self.node(&pair.value, scope, file, chain, depth),
                    })
                    .collect();
                Node {
                    body: Body::Mapping(pairs),
                    place,
                }
            }
        }
    }

    /// `delay: {rtt / 2}` is a mapping with one key to YAML. Say so when the key is an expression.
    fn unquoted_expression(&mut self, pairs: &[Pair], place: Place, scope: &Bindings) {
        let [pair] = pairs else { return };
        let empty =
            matches!(&pair.value.body, Body::Scalar { text, quoted: false } if text.is_empty());
        if empty && !pair.key.contains(':') && expr::evaluate(&pair.key, scope).is_ok() {
            self.fail(
                format!("`{{{}}}` has to be in quotes", pair.key),
                Some(format!(
                    "write \"{{{}}}\": YAML reads an unquoted `{{` as a mapping",
                    pair.key
                )),
                Some(place),
            );
        }
    }

    fn scalar(&mut self, text: &str, quoted: bool, place: Place, scope: &Bindings) -> Node {
        match self.evaluate_scalar(text, quoted, place, scope) {
            Some(Binding { written, .. }) => Node::scalar(written.text, written.quoted, place),
            None => Node::null(place),
        }
    }

    /// Replaces a place of use by the content of its template.
    fn use_site(
        &mut self,
        site: &Node,
        scope: &Bindings,
        file: u32,
        chain: u32,
        depth: usize,
    ) -> Node {
        let place = Self::with_chain(site.place, chain);
        let Some(target) = self.target_of(site, chain) else {
            return Node::null(place);
        };
        if !self.enter(place, depth) {
            return Node::null(place);
        }
        let Some((template, identity)) = self.find(&target, file) else {
            return Node::null(place);
        };
        if self.active.iter().any(|(active, _)| *active == identity) {
            let mut names: Vec<String> = self.active.iter().map(|(_, name)| name.clone()).collect();
            names.push(target.name.clone());
            self.fail(
                format!("{} uses itself: {}", template.what, names.join(" -> ")),
                Some("a template or a file cannot be used inside itself".to_owned()),
                Some(place),
            );
            return Node::null(place);
        }
        // The arguments are in the scope of the caller
        let given = self.arguments(site, scope, chain);
        let bindings = self.bind(&template.params, given, &template.what, Some(place));
        // The uses inside the template come from this one
        let mut uses: Vec<Use> = self.registry.chains[chain as usize]
            .iter()
            .map(|used| Use {
                place: used.place,
                what: used.what.clone(),
            })
            .collect();
        uses.push(Use {
            place,
            what: if target.kind == "use" {
                format!("template `{}`", target.name)
            } else {
                format!("file '{}'", target.name)
            },
        });
        self.registry.chains.push(uses);
        let inner = u32::try_from(self.registry.chains.len() - 1).unwrap_or(0);
        self.active.push((identity, target.name));
        let content = self.node(
            &template.content,
            &bindings,
            template.file,
            inner,
            depth + 1,
        );
        self.active.pop();
        content
    }

    /// What a place of use names: `use` or `include`, and the template or the path. `None` after the problem is reported.
    fn target_of(&mut self, site: &Node, chain: u32) -> Option<Target> {
        let Body::Mapping(pairs) = &site.body else {
            return None;
        };
        for pair in pairs {
            if !matches!(pair.key.as_str(), "use" | "include" | "with") {
                self.fail(
                    format!("`{}` has no place beside `use` or `include`", pair.key),
                    Some("a place of use has only `use:` or `include:` and `with:`; what it is replaced by comes from the template".to_owned()),
                    Some(Self::with_chain(pair.key_place, chain)),
                );
            }
        }
        let (kind, pair) = match (site.get("use"), site.get("include")) {
            (Some(_), Some(both)) => {
                self.fail(
                    "a place has `use` or `include`, not both",
                    None,
                    Some(Self::with_chain(both.key_place, chain)),
                );
                return None;
            }
            (Some(pair), None) => ("use", pair),
            (None, Some(pair)) => ("include", pair),
            (None, None) => return None,
        };
        let place = Self::with_chain(pair.value.place, chain);
        match &pair.value.body {
            Body::Scalar { text, .. } if !text.is_empty() => Some(Target {
                kind,
                name: text.clone(),
                place,
            }),
            _ => {
                self.fail(
                    format!(
                        "`{kind}` is followed by {}",
                        if kind == "use" {
                            "the name of a template"
                        } else {
                            "the path of a file"
                        }
                    ),
                    None,
                    Some(place),
                );
                None
            }
        }
    }

    /// Counts a use and checks the depth and the number of uses.
    fn enter(&mut self, place: Place, depth: usize) -> bool {
        if depth + 1 > MAX_USE_DEPTH {
            self.fail(
                format!("templates and includes are used inside each other more than {MAX_USE_DEPTH} levels deep"),
                Some("this is usually a template that uses itself, directly or through others".to_owned()),
                Some(place),
            );
            return false;
        }
        self.uses += 1;
        if self.uses > MAX_USES {
            if !self.stopped {
                self.fail(
                    format!("the scenario has more than {MAX_USES} places of use"),
                    None,
                    Some(place),
                );
            }
            self.stopped = true;
            return false;
        }
        true
    }

    /// The template that a place of use names, and what identifies it for the check for a cycle.
    fn find(&mut self, target: &Target, file: u32) -> Option<(Rc<Template>, String)> {
        if target.kind == "include" {
            let directory = self.data[file as usize]
                .as_ref()
                .map_or(String::new(), |data| data.directory.clone());
            let path = join_path(&directory, &target.name);
            let index = self.load(&path, target.place)?;
            let main = self.data[index as usize].as_ref()?.main.clone()?;
            return Some((main, format!("file:{path}")));
        }
        let templates = &self.data[file as usize].as_ref()?.templates;
        if let Some(template) = templates.get(&target.name) {
            return Some((Rc::clone(template), format!("{file}#{}", target.name)));
        }
        let mut names: Vec<&str> = templates.keys().map(String::as_str).collect();
        names.sort_unstable();
        let help = match closest(&target.name, &names) {
            Some(near) => format!("did you mean `{near}`?"),
            None if names.is_empty() => {
                "`templates:` at the top of the file defines them".to_owned()
            }
            None => format!("the templates of this file are {}", names.join(", ")),
        };
        self.fail(
            format!("there is no template `{}`", target.name),
            Some(help),
            Some(target.place),
        );
        None
    }

    /// The values of `with:`, evaluated in the scope of the caller.
    fn arguments(
        &mut self,
        site: &Node,
        scope: &Bindings,
        chain: u32,
    ) -> Vec<(String, Binding, Option<Place>)> {
        let mut given = Vec::new();
        let Some(with) = site.get("with") else {
            return given;
        };
        match &with.value.body {
            Body::Mapping(arguments) => {
                for argument in arguments {
                    let place = Self::with_chain(argument.value.place, chain);
                    let Body::Scalar { text, quoted } = &argument.value.body else {
                        self.fail(
                            format!("the argument `{}` has to be a single value", argument.key),
                            Some(
                                "an argument is a number, a quantity, a word or true or false"
                                    .to_owned(),
                            ),
                            Some(place),
                        );
                        continue;
                    };
                    if let Some(binding) = self.evaluate_scalar(text, *quoted, place, scope) {
                        given.push((argument.key.clone(), binding, Some(place)));
                    }
                }
            }
            Body::Scalar {
                text,
                quoted: false,
            } if text.is_empty() => {}
            _ => self.fail(
                "`with` has to be a mapping of parameters to values",
                Some("write `with: { name: value }`".to_owned()),
                Some(Self::with_chain(with.value.place, chain)),
            ),
        }
        given
    }
}

/// What a place of use names.
struct Target {
    /// `use` or `include`.
    kind: &'static str,
    /// The name of the template or the path of the file, as written.
    name: String,
    place: Place,
}

/// Walks the expanded tree to give every path its origin.
fn collect_places(
    node: &Node,
    path: &str,
    out: &mut HashMap<String, (Place, Option<Place>)>,
    key: Option<Place>,
) {
    out.insert(path.to_owned(), (node.place, key));
    match &node.body {
        Body::Scalar { .. } => {}
        Body::Sequence(items) => {
            for (at, item) in items.iter().enumerate() {
                collect_places(item, &format!("{path}[{at}]"), out, None);
            }
        }
        Body::Mapping(pairs) => {
            for pair in pairs {
                let child = if path.is_empty() {
                    pair.key.clone()
                } else {
                    format!("{path}.{}", pair.key)
                };
                collect_places(&pair.value, &child, out, Some(pair.key_place));
            }
        }
    }
}

/// Expands the parameters, templates and includes of a scenario file.
///
/// `Ok(None)` if the file uses none of them and there are no values for parameters: the file is read as it is. A file that
/// cannot be parsed is also left to the reader of the format, which reports it better.
pub(crate) fn expand(
    text: &str,
    source_name: &str,
    options: &LoadOptions<'_>,
) -> Result<Option<Expansion>, Error> {
    let (_, index_problems) = source::index(text);
    if !index_problems.is_empty() {
        return Ok(None);
    }
    let Ok(root) = tree::parse(text, 0) else {
        return Ok(None);
    };
    let Body::Mapping(pairs) = &root.body else {
        return Ok(None);
    };
    let has_keys = pairs
        .iter()
        .any(|p| p.key == "params" || p.key == "templates");
    if !has_keys && options.params.is_empty() && !needs_pass(&root) {
        return Ok(None);
    }
    let mut expander = Expander {
        options,
        registry: Registry {
            files: vec![SourceFile {
                name: source_name.to_owned(),
                text: text.to_owned(),
            }],
            chains: vec![Vec::new()],
        },
        data: vec![None],
        by_path: HashMap::new(),
        active: Vec::new(),
        failures: Vec::new(),
        uses: 0,
        nodes: 0,
        stopped: false,
    };
    let result = expander.run(&root, pairs);
    if expander.failures.is_empty() {
        expander.finish(&result, source_name, text)
    } else {
        Err(failed(&expander, source_name, text))
    }
}

impl Expander<'_> {
    /// Expands the sections of the scenario file: its own parameters and templates first, then each section in its scope.
    fn run(&mut self, root: &Node, pairs: &[Pair]) -> Node {
        let templates = match root.get("templates") {
            Some(pair) => self.templates(&pair.value, 0),
            None => HashMap::new(),
        };
        self.data[0] = Some(FileData {
            templates,
            main: None,
            directory: String::new(),
        });
        let declarations = match root.get("params") {
            Some(pair) => self.declarations(&pair.value, 0),
            None => Vec::new(),
        };
        let mut given = Vec::new();
        for (name, value) in &self.options.params {
            if value.is_empty() {
                self.fail(
                    format!("the value given for `{name}` is empty"),
                    Some(format!("write a value: --param {name}=<value>")),
                    None,
                );
            } else {
                given.push((name.clone(), Binding::from_scalar(value, false), None));
            }
        }
        let scope = self.bind(&declarations, given, "this scenario", None);
        if is_use(root) {
            self.fail(
                "the file itself is a place of use",
                Some("a scenario has `version:` and the sections below it; use a template inside a section".to_owned()),
                Some(root.place),
            );
        }
        let mut sections = Vec::new();
        for pair in pairs {
            match pair.key.as_str() {
                "templates" => {}
                "params" => {
                    // The record holds the text of every value; a file that has it reads the kinds again
                    let values: Vec<Pair> = declarations
                        .iter()
                        .filter_map(|declaration| {
                            scope.values.get(&declaration.name).map(|binding| Pair {
                                key: declaration.name.clone(),
                                key_place: declaration.place,
                                value: Node::scalar(
                                    binding.written.text.clone(),
                                    true,
                                    declaration.place,
                                ),
                            })
                        })
                        .collect();
                    if !values.is_empty() {
                        sections.push(Pair {
                            key: pair.key.clone(),
                            key_place: pair.key_place,
                            value: Node {
                                body: Body::Mapping(values),
                                place: pair.value.place,
                            },
                        });
                    }
                }
                _ => {
                    let value = self.node(&pair.value, &scope, 0, 0, 0);
                    sections.push(Pair {
                        key: pair.key.clone(),
                        key_place: pair.key_place,
                        value,
                    });
                }
            }
        }
        Node {
            body: Body::Mapping(sections),
            place: root.place,
        }
    }

    /// The text of the expanded scenario and which node of it was written where.
    fn finish(
        mut self,
        result: &Node,
        source_name: &str,
        text: &str,
    ) -> Result<Option<Expansion>, Error> {
        let expanded = tree::to_text(result);
        if expanded.len() > MAX_FILE_BYTES {
            self.fail(
                format!(
                    "the expanded scenario is larger than {MAX_FILE_BYTES} bytes ({} bytes)",
                    expanded.len()
                ),
                Some("a scenario does not need that much text".to_owned()),
                Some(result.place),
            );
            return Err(failed(&self, source_name, text));
        }
        let mut by_path = HashMap::new();
        collect_places(result, "", &mut by_path, None);
        let (index, _) = source::index(&expanded);
        let mut places = HashMap::new();
        for (path, entry) in index.iter() {
            if let Some((value, _)) = by_path.get(path) {
                places.insert(entry.value.0, *value);
            }
        }
        // A key and the mapping that starts with it are at the same position: the key wins
        for (path, entry) in index.iter() {
            if let (Some((_, Some(key))), Some((at, _))) = (by_path.get(path), entry.key) {
                places.insert(at, *key);
            }
        }
        Ok(Some(Expansion {
            text: expanded.clone(),
            provenance: Provenance {
                registry: self.registry,
                expanded,
                places,
            },
        }))
    }
}

/// The error for the failures of a pass.
fn failed(expander: &Expander<'_>, source_name: &str, text: &str) -> Error {
    let registry = &expander.registry;
    let mut problems: Vec<Problem> = expander
        .failures
        .iter()
        .map(|failure| {
            let mut problem = Problem::new(failure.message.clone());
            problem.help.clone_from(&failure.help);
            match failure.place {
                Some(place) => registry.place(problem, place),
                None => problem,
            }
        })
        .collect();
    problems.dedup();
    Error::with_sources(source_name, text, other_sources(&registry.files), problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_joined_and_cleaned_lexically() {
        assert_eq!(join_path("", "a.yaml"), "a.yaml");
        assert_eq!(join_path("x", "a.yaml"), "x/a.yaml");
        assert_eq!(join_path("x/y", "../a.yaml"), "x/a.yaml");
        assert_eq!(join_path("x", "../../a.yaml"), "../a.yaml");
        assert_eq!(join_path("", "./b/./a.yaml"), "b/a.yaml");
        assert_eq!(join_path("x", "/etc/a.yaml"), "/etc/a.yaml");
        assert_eq!(join_path("", "../a.yaml"), "../a.yaml");
    }

    #[test]
    fn only_a_scalar_that_is_an_expression_as_a_whole_is_one() {
        assert_eq!(expression_of("{rtt / 2}"), Some("rtt / 2"));
        assert_eq!(expression_of("{x}"), Some("x"));
        assert_eq!(expression_of("{rate * 1.5}"), Some("rate * 1.5"));
        assert_eq!(expression_of("{1.5s}"), Some("1.5s"));
        // The placeholders and the escape of the commands of the workloads stay as they are
        for kept in [
            "{server.addr}",
            "{edge-1.addr}",
            "{{server.addr}}",
            "{{x}}",
            "a{x}",
            "{x}b",
            "x",
            "{x",
        ] {
            assert_eq!(expression_of(kept), None, "{kept}");
        }
    }

    #[test]
    fn a_value_with_an_expression_in_it_is_found() {
        assert!(has_expression("{x}"));
        assert!(has_expression("a-{x + 1}-b"));
        for no in ["x", "{{x}}", "{server.addr}:80", "{\"a\": 1}", "{x", ""] {
            assert!(!has_expression(no), "{no}");
        }
    }

    #[test]
    fn names_of_parameters() {
        for good in ["rtt", "a1", "half_rtt", "x"] {
            assert!(valid_parameter_name(good), "{good}");
        }
        for bad in ["", "1a", "Rtt", "half-rtt", "_x", "a b"] {
            assert!(!valid_parameter_name(bad), "{bad}");
        }
    }
}
