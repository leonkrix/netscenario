//! Parameters, templates and includes (SCN-05, D-093): a file that uses them reads as the scenario it describes, and every fault is
//! shown where the value was written.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fmt::Write as _;

use netscenario_model::{Error, FileSource, LoadOptions, Problem, Scenario};

/// Files by path; every read is noted.
struct Files {
    files: Vec<(&'static str, String)>,
    reads: RefCell<Vec<String>>,
}

impl Files {
    fn new(files: &[(&'static str, &str)]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(path, text)| (*path, (*text).to_owned()))
                .collect(),
            reads: RefCell::new(Vec::new()),
        }
    }
}

impl FileSource for Files {
    fn read(&self, path: &str) -> Result<String, String> {
        self.reads.borrow_mut().push(path.to_owned());
        self.files
            .iter()
            .find(|(known, _)| *known == path)
            .map(|(_, text)| text.clone())
            .ok_or_else(|| format!("cannot read '{path}': no such file"))
    }
}

const HEAD: &str = "version: 0\nname: t\nduration: 5s\n";
const WORKLOADS: &str = "workloads:\n  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }\n";

fn plain(delay: &str, rate: &str) -> Scenario {
    Scenario::from_yaml(
        &format!("{HEAD}topology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n      both: {{ delay: {delay}, rate: {rate} }}\n{WORKLOADS}"),
        "plain.yaml",
    )
    .unwrap()
}

const WITH_PARAMS: &str = "version: 0
name: t
duration: 5s
params:
  rtt: 40ms
  rate: 10mbit
  half: \"{rtt / 2}\"
topology:
  nodes: [client, server]
  links:
    - id: access
      between: [client, server]
      both: { delay: \"{half}\", rate: \"{rate * 2}\" }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
";

fn read(text: &str, options: &LoadOptions<'_>) -> Result<Scenario, Error> {
    Scenario::from_yaml_with(text, "scenario.yaml", options)
}

fn refused(text: &str, options: &LoadOptions<'_>) -> String {
    read(text, options).expect_err("a refused file").render()
}

/// The scenario without the record of its parameters, to compare it with the one written out.
fn without_record(mut scenario: Scenario) -> Scenario {
    scenario.params.clear();
    scenario
}

#[test]
fn a_file_without_parameters_reads_as_before() {
    let text = format!(
        "{HEAD}topology:\n  nodes: [client, server]\n  links:\n    - id: access\n      between: [client, server]\n      both: {{ delay: 20ms }}\n{WORKLOADS}"
    );
    let scenario = Scenario::from_yaml(&text, "t.yaml").unwrap();
    assert!(scenario.params.is_empty());
    // The same file with a value given for a parameter that it does not declare is refused
    let error = refused(&text, &LoadOptions::new().param("rtt", "10ms"));
    assert!(
        error.contains("`rtt` is not a parameter of this scenario"),
        "{error}"
    );
}

#[test]
fn defaults_are_used_and_the_file_reads_as_the_scenario_written_out() {
    let scenario = read(WITH_PARAMS, &LoadOptions::new()).unwrap();
    assert_eq!(
        scenario
            .params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>(),
        [("half", "20ms"), ("rate", "10mbit"), ("rtt", "40ms")]
    );
    assert_eq!(without_record(scenario), plain("20ms", "20mbit"));
}

#[test]
fn a_given_value_replaces_the_default_and_the_defaults_that_use_it_follow() {
    let options = LoadOptions::new()
        .param("rtt", "100ms")
        .param("rate", "1.5mbit");
    let scenario = read(WITH_PARAMS, &options).unwrap();
    assert_eq!(scenario.params["half"], "50ms");
    assert_eq!(without_record(scenario), plain("50ms", "3mbit"));
    // A default that is derived can be given too
    let scenario = read(WITH_PARAMS, &LoadOptions::new().param("half", "7ms")).unwrap();
    assert_eq!(without_record(scenario), plain("7ms", "20mbit"));
}

#[test]
fn a_value_of_the_wrong_kind_or_for_no_parameter_is_refused_with_the_way_out() {
    let wrong = refused(WITH_PARAMS, &LoadOptions::new().param("rate", "50ms"));
    assert!(
        wrong.contains("`rate` of this scenario is a rate, and a duration was given"),
        "{wrong}"
    );
    assert!(wrong.contains("the default is `10mbit`"), "{wrong}");
    let unknown = refused(WITH_PARAMS, &LoadOptions::new().param("rt", "50ms"));
    assert!(
        unknown.contains("`rt` is not a parameter of this scenario"),
        "{unknown}"
    );
    assert!(unknown.contains("did you mean `rtt`?"), "{unknown}");
    let none = refused(WITH_PARAMS, &LoadOptions::new().param("zzzzzz", "1"));
    assert!(none.contains("it declares rtt, rate, half"), "{none}");
}

#[test]
fn the_value_of_a_parameter_is_not_calculated() {
    let text = WITH_PARAMS.replace("rate: 10mbit", "rate: 10mbit\n  note: x");
    // A value that would be an expression when the recorded scenario is read again is refused now
    let error = refused(&text, &LoadOptions::new().param("note", "{rate}"));
    assert!(
        error.contains("the value `{rate}` of `note` would be read as an expression again"),
        "{error}"
    );
    // Braces that are not an expression are values
    let scenario = read(
        &text,
        &LoadOptions::new().param("note", "--json={\"a\": 1}"),
    )
    .unwrap();
    assert_eq!(scenario.params["note"], "--json={\"a\": 1}");
    assert_eq!(
        Scenario::from_yaml(&scenario.to_yaml().unwrap(), "again.yaml").unwrap(),
        scenario
    );
}

#[test]
fn an_empty_value_is_not_a_value() {
    let error = refused(WITH_PARAMS, &LoadOptions::new().param("flows", ""));
    assert!(
        error.contains("the value given for `flows` is empty"),
        "{error}"
    );
    assert!(error.contains("--param flows=<value>"), "{error}");
}

#[test]
fn a_parameter_that_is_missing_is_reported_once_and_not_where_it_is_used() {
    let text = WITH_PARAMS.replace("rtt: 40ms", "rtt:");
    let error = read(&text, &LoadOptions::new()).unwrap_err();
    // `half` and the delay use `rtt`; they say nothing more
    assert_eq!(error.problems().len(), 1, "{error}");
    assert!(
        error.problems()[0]
            .message
            .contains("needs a value for `rtt`")
    );
    // Canary: a name that is no parameter at all is still reported where it is used
    let typo = WITH_PARAMS.replace("{half}", "{halv}");
    assert!(refused(&typo, &LoadOptions::new()).contains("`halv` is not a parameter"));
}

#[test]
fn a_parameter_without_a_default_must_be_given() {
    let text = WITH_PARAMS.replace("rtt: 40ms", "rtt:");
    let missing = refused(&text, &LoadOptions::new());
    assert!(
        missing.contains("this scenario needs a value for `rtt`"),
        "{missing}"
    );
    assert!(missing.contains("--param rtt=<value>"), "{missing}");
    assert!(missing.contains("--> scenario.yaml:5:3"), "{missing}");
    assert!(read(&text, &LoadOptions::new().param("rtt", "10ms")).is_ok());
    // A text takes whatever is given, a number does not take a duration
    let words = "version: 0\nname: t\nduration: 5s\nparams:\n  label: a\n  n: 2\nworkloads: []\n";
    assert!(
        refused(words, &LoadOptions::new().param("n", "2ms"))
            .contains("is a number, and a duration was given")
    );
}

#[test]
fn an_expression_that_is_wrong_is_shown_at_its_value() {
    let text = WITH_PARAMS.replace("\"{rate * 2}\"", "\"{rate + rtt}\"");
    let error = refused(&text, &LoadOptions::new());
    assert_eq!(
        error,
        "error: cannot add a rate and a duration\n  --> scenario.yaml:13:38\n   |\n12 |       between: [client, server]\n13 |       both: { delay: \"{half}\", rate: \"{rate + rtt}\" }\n   |                                      ^^^^^^^^^^^^^^\n   = help: both sides of + and - need the same kind of value\n"
    );
    let unknown = refused(
        &WITH_PARAMS.replace("{half}", "{halv}"),
        &LoadOptions::new(),
    );
    assert!(unknown.contains("`halv` is not a parameter"), "{unknown}");
    assert!(unknown.contains("did you mean `half`?"), "{unknown}");
    let fraction = refused(
        &WITH_PARAMS.replace("{rate * 2}", "{rate / 3}"),
        &LoadOptions::new(),
    );
    assert!(
        fraction.contains("not a whole number of bits per second"),
        "{fraction}"
    );
}

#[test]
fn an_unquoted_expression_is_told_to_be_quoted() {
    let text = WITH_PARAMS.replace("rate: \"{rate * 2}\"", "rate: {rate}");
    let error = refused(&text, &LoadOptions::new());
    assert!(error.contains("`{rate}` has to be in quotes"), "{error}");
}

#[test]
fn the_placeholders_of_commands_are_not_expressions() {
    let text = "version: 0
name: t
duration: 5s
params:
  port: 4433
topology:
  nodes: [client, server]
  links:
    - { id: a, between: [client, server] }
workloads:
  - id: echo
    kind: command
    node: server
    run: [\"echo\", \"{server.addr}\", \"{port}\", \"{{literal}}\", \"{server.addr}:{port}\"]
";
    let scenario = read(text, &LoadOptions::new());
    // The pass leaves `{server.addr}` and `{{...}}` alone and puts the parameter in, whole or inside a longer text
    let rendered = match scenario {
        Ok(scenario) => scenario.to_yaml().unwrap(),
        Err(error) => error.render(),
    };
    assert!(rendered.contains("{server.addr}"), "{rendered}");
    assert!(rendered.contains("{{literal}}"), "{rendered}");
    assert!(rendered.contains("{server.addr}:4433"), "{rendered}");
    assert!(!rendered.contains("{port}"), "{rendered}");
    assert!(!rendered.contains("is not a parameter"), "{rendered}");
}

#[test]
fn the_expanded_scenario_is_the_one_that_is_recorded_and_replayed() {
    let scenario = read(WITH_PARAMS, &LoadOptions::new().param("rtt", "60ms")).unwrap();
    let json = serde_json::to_string(&scenario).unwrap();
    // A replay reads the recorded file: it expands to itself
    let again = Scenario::from_yaml(&json, "scenario.json").unwrap();
    assert_eq!(again, scenario);
    assert_eq!(serde_json::to_string(&again).unwrap(), json);
    // Other values are another scenario, and so another hash
    let other = read(WITH_PARAMS, &LoadOptions::new().param("rtt", "61ms")).unwrap();
    assert_ne!(serde_json::to_string(&other).unwrap(), json);
}

const WITH_TEMPLATE: &str = "version: 0
name: t
duration: 5s
templates:
  access:
    params:
      id:
      delay: 10ms
      rate: 10mbit
    content:
      id: \"{id}\"
      between: [client, server]
      both: { delay: \"{delay}\", rate: \"{rate}\" }
  pair:
    params:
      rate: 10mbit
    content:
      - use: access
        with: { id: first, delay: 1ms, rate: \"{rate}\" }
      - use: access
        with: { id: second, delay: \"{2 * 1ms}\", rate: \"{rate / 2}\" }
topology:
  nodes: [client, server]
  links:
    - use: pair
      with: { rate: 8mbit }
    - use: access
      with: { id: third }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
";

#[test]
fn a_template_is_replaced_by_its_content_and_a_list_is_spliced() {
    let scenario = read(WITH_TEMPLATE, &LoadOptions::new()).unwrap();
    let written = Scenario::from_yaml(
        "version: 0
name: t
duration: 5s
topology:
  nodes: [client, server]
  links:
    - { id: first, between: [client, server], both: { delay: 1ms, rate: 8mbit } }
    - { id: second, between: [client, server], both: { delay: 2ms, rate: 4mbit } }
    - { id: third, between: [client, server], both: { delay: 10ms, rate: 10mbit } }
workloads:
  - { id: media, kind: udp-cbr, from: client, to: server, rate: 1mbit, packet-size: 1200b }
",
        "written.yaml",
    )
    .unwrap();
    assert_eq!(scenario, written);
}

#[test]
fn a_template_sees_its_own_parameters_and_nothing_of_the_caller() {
    // `rtt` is a parameter of the scenario and not of the template that uses it
    let text = format!(
        "{HEAD}params:\n  rtt: 5ms\ntemplates:\n  access:\n    content:\n      id: a\n      between: [client, server]\n      both: {{ delay: \"{{rtt}}\" }}\ntopology:\n  nodes: [client, server]\n  links:\n    - use: access\n{WORKLOADS}"
    );
    let error = refused(&text, &LoadOptions::new());
    assert!(error.contains("`rtt` is not a parameter"), "{error}");
    assert!(error.contains("no parameter is declared here"), "{error}");
    assert!(
        error.contains("used from scenario.yaml:15:7 (template `access`)"),
        "{error}"
    );
}

#[test]
fn a_fault_inside_a_template_names_the_template_and_each_place_that_used_it() {
    let text = WITH_TEMPLATE.replace("rate: \"{rate / 2}\"", "rate: \"{rate / 0}\"");
    let error = refused(&text, &LoadOptions::new());
    assert!(
        error.contains("error: division by zero\n  --> scenario.yaml:"),
        "{error}"
    );
    assert!(error.contains("rate: \"{rate / 0}\""), "{error}");
    // The inner use of `access` is in `pair`, which the topology used
    assert!(
        error.contains("= used from scenario.yaml:25:7 (template `pair`)"),
        "{error}"
    );
}

#[test]
fn templates_are_checked_when_used_and_unused_ones_are_not_expanded() {
    let broken = WITH_TEMPLATE.replace(
        "      - use: access\n        with: { id: first",
        "      - use: nowhere\n        with: { id: first",
    );
    let error = refused(&broken, &LoadOptions::new());
    assert!(error.contains("there is no template `nowhere`"), "{error}");
    assert!(
        error.contains("the templates of this file are access, pair"),
        "{error}"
    );
    let near = refused(&broken.replace("nowhere", "acces"), &LoadOptions::new());
    assert!(near.contains("did you mean `access`?"), "{near}");
    // A template that is never used is not expanded, so the `{id}` without a value in it does no harm
    let unused = format!(
        "{HEAD}templates:\n  never:\n    params:\n      x: 1\n    content:\n      a: \"{{x / 0}}\"\ntopology:\n  nodes: [client, server]\n  links: []\n{WORKLOADS}"
    );
    assert!(!refused(&unused, &LoadOptions::new()).contains("division by zero"));
}

#[test]
fn a_template_that_uses_itself_is_refused_with_the_chain() {
    let text = format!(
        "{HEAD}templates:\n  loop:\n    content:\n      use: other\n  other:\n    content:\n      use: loop\ntopology:\n  nodes: [client, server]\n  links:\n    - use: loop\n{WORKLOADS}"
    );
    let error = refused(&text, &LoadOptions::new());
    assert!(
        error.contains("template `loop` uses itself: loop -> other -> loop"),
        "{error}"
    );
}

#[test]
fn arguments_are_checked_against_the_declarations() {
    let missing = refused(
        &WITH_TEMPLATE.replace("with: { id: third }", "with: { delay: 3ms }"),
        &LoadOptions::new(),
    );
    assert!(
        missing.contains("template `access` needs a value for `id`"),
        "{missing}"
    );
    assert!(missing.contains("`with: { id: ... }`"), "{missing}");
    let unknown = refused(
        &WITH_TEMPLATE.replace("with: { id: third }", "with: { id: third, rat: 1mbit }"),
        &LoadOptions::new(),
    );
    assert!(
        unknown.contains("`rat` is not a parameter of template `access`"),
        "{unknown}"
    );
    assert!(unknown.contains("did you mean `rate`?"), "{unknown}");
    let kind = refused(
        &WITH_TEMPLATE.replace("with: { id: third }", "with: { id: third, delay: 5mbit }"),
        &LoadOptions::new(),
    );
    assert!(
        kind.contains("is a duration, and a rate was given"),
        "{kind}"
    );
    let compound = refused(
        &WITH_TEMPLATE.replace("with: { id: third }", "with: { id: [a, b] }"),
        &LoadOptions::new(),
    );
    assert!(compound.contains("has to be a single value"), "{compound}");
    let beside = refused(
        &WITH_TEMPLATE.replace(
            "    - use: access\n      with: { id: third }",
            "    - use: access\n      id: third",
        ),
        &LoadOptions::new(),
    );
    assert!(
        beside.contains("`id` has no place beside `use` or `include`"),
        "{beside}"
    );
}

const LINK_FILE: &str = "params:
  id:
  rate: 10mbit
content:
  id: \"{id}\"
  between: [client, server]
  both: { rate: \"{rate}\" }
";

fn including(link: &str) -> String {
    format!("{HEAD}topology:\n  nodes: [client, server]\n  links:\n{link}{WORKLOADS}")
}

#[test]
fn an_included_file_is_a_template_and_is_read_once() {
    let files = Files::new(&[("access.yaml", LINK_FILE)]);
    let text = including(
        "    - include: access.yaml\n      with: { id: a }\n    - include: ./access.yaml\n      with: { id: b, rate: 2mbit }\n",
    );
    let scenario = read(&text, &LoadOptions::new().files(&files)).unwrap();
    assert_eq!(scenario.topology.links.len(), 2);
    assert_eq!(scenario.topology.links[1].id.as_str(), "b");
    assert_eq!(*files.reads.borrow(), ["access.yaml"]);
}

#[test]
fn includes_are_relative_to_the_file_that_includes_and_may_be_nested() {
    let files = Files::new(&[
        (
            "parts/pair.yaml",
            "content:\n  - include: single.yaml\n    with: { id: one }\n  - include: ../top.yaml\n    with: { id: two }\n",
        ),
        ("parts/single.yaml", LINK_FILE),
        ("top.yaml", LINK_FILE),
    ]);
    let text = including("    - include: parts/pair.yaml\n");
    let scenario = read(&text, &LoadOptions::new().files(&files)).unwrap();
    let ids: Vec<&str> = scenario
        .topology
        .links
        .iter()
        .map(|l| l.id.as_str())
        .collect();
    assert_eq!(ids, ["one", "two"]);
}

#[test]
fn a_whole_section_can_be_a_file() {
    let files = Files::new(&[(
        "topology.yaml",
        "params:\n  delay: 5ms\ncontent:\n  nodes: [client, server]\n  links:\n    - { id: a, between: [client, server], both: { delay: \"{delay}\" } }\n",
    )]);
    let text =
        format!("{HEAD}topology:\n  include: topology.yaml\n  with: {{ delay: 7ms }}\n{WORKLOADS}");
    let scenario = read(&text, &LoadOptions::new().files(&files)).unwrap();
    assert_eq!(scenario.topology.links.len(), 1);
}

#[test]
fn a_fault_in_an_included_file_is_shown_in_that_file_with_the_place_of_the_include() {
    let files = Files::new(&[(
        "access.yaml",
        &LINK_FILE.replace(
            "both: { rate: \"{rate}\" }",
            "both: { rate: \"{rate / 0}\" }",
        ),
    )]);
    let text = including("    - include: access.yaml\n      with: { id: a }\n");
    let error = read(&text, &LoadOptions::new().files(&files)).unwrap_err();
    assert_eq!(
        error.render(),
        "error: division by zero\n --> access.yaml:7:17\n  |\n6 |   between: [client, server]\n7 |   both: { rate: \"{rate / 0}\" }\n  |                 ^^^^^^^^^^^^\n  = used from scenario.yaml:7:7 (file 'access.yaml')\n"
    );
    let problem: &Problem = &error.problems()[0];
    assert_eq!(problem.file(), Some("access.yaml"));
    assert_eq!(problem.chain().len(), 1);
}

#[test]
fn a_fault_of_the_contents_in_a_template_is_shown_at_the_template() {
    // `rate: 0bit` is a value the format refuses; the typed reader finds it in the expanded scenario
    let files = Files::new(&[("access.yaml", LINK_FILE)]);
    let text = including("    - include: access.yaml\n      with: { id: a, rate: 0bit }\n");
    let error = read(&text, &LoadOptions::new().files(&files)).unwrap_err();
    let rendered = error.render();
    assert!(rendered.contains("access.yaml:7:"), "{rendered}");
    assert!(
        rendered.contains("used from scenario.yaml:7:7 (file 'access.yaml')"),
        "{rendered}"
    );
}

#[test]
fn what_a_backend_finds_later_is_shown_at_the_value_too() {
    let files = Files::new(&[("access.yaml", LINK_FILE)]);
    let text = including("    - include: access.yaml\n      with: { id: a }\n");
    let scenario = read(&text, &LoadOptions::new().files(&files)).unwrap();
    let problem = Problem::new("the backend cannot do this").at_path("topology.links[0].both.rate");
    let error = scenario
        .problems_in_source("scenario.yaml", &text, vec![problem])
        .unwrap();
    let rendered = error.render();
    assert!(rendered.contains("--> access.yaml:7:"), "{rendered}");
    assert!(
        rendered.contains("used from scenario.yaml:7:7 (file 'access.yaml')"),
        "{rendered}"
    );
    // A scenario that was not expanded is located in its own text, as before
    let plain_text = format!(
        "{HEAD}topology:\n  nodes: [client, server]\n  links:\n    - id: a\n      between: [client, server]\n      both: {{ rate: 1mbit }}\n{WORKLOADS}"
    );
    let plain_scenario = Scenario::from_yaml(&plain_text, "p.yaml").unwrap();
    let problem = Problem::new("no").at_path("topology.links[0].both.rate");
    let error = plain_scenario
        .problems_in_source("p.yaml", &plain_text, vec![problem])
        .unwrap();
    assert!(
        error.render().contains("--> p.yaml:9:"),
        "{}",
        error.render()
    );
}

#[test]
fn files_that_cannot_be_used_are_refused_with_the_place_of_the_include() {
    let none = refused(
        &including("    - include: access.yaml\n      with: { id: a }\n"),
        &LoadOptions::new(),
    );
    assert!(
        none.contains("there is no directory to look for it in"),
        "{none}"
    );
    let files = Files::new(&[
        ("scenario.yaml", "x"),
        ("syntax.yaml", "content: [a, b\n"),
        ("whole.yaml", "version: 0\nname: x\n"),
        ("list.yaml", "- a\n- b\n"),
        ("nocontent.yaml", "params:\n  a: 1\n"),
        (
            "anchor.yaml",
            "content: &x { id: a, between: [client, server] }\n",
        ),
    ]);
    let options = LoadOptions::new().files(&files);
    let missing = refused(&including("    - include: nothing.yaml\n"), &options);
    assert!(
        missing.contains("cannot read 'nothing.yaml': no such file"),
        "{missing}"
    );
    assert!(missing.contains("--> scenario.yaml:7:"), "{missing}");
    let syntax = refused(&including("    - include: syntax.yaml\n"), &options);
    assert!(syntax.contains("--> syntax.yaml"), "{syntax}");
    let whole = refused(&including("    - include: whole.yaml\n"), &options);
    assert!(
        whole.contains("an included file is not a scenario"),
        "{whole}"
    );
    assert!(
        refused(&including("    - include: list.yaml\n"), &options).contains("has to be a mapping")
    );
    assert!(
        refused(&including("    - include: nocontent.yaml\n"), &options)
            .contains("has no `content:`")
    );
    // The YAML features that the format refuses are refused in an included file too, in that file
    let anchor = refused(&including("    - include: anchor.yaml\n"), &options);
    assert!(
        anchor.contains("anchors are not part of the scenario format"),
        "{anchor}"
    );
    assert!(anchor.contains("--> anchor.yaml:1:"), "{anchor}");
    // A path that leads out of the directory is the business of the source, and it is asked for with the `..` in it
    let outside = refused(&including("    - include: ../x.yaml\n"), &options);
    assert!(outside.contains("cannot read '../x.yaml'"), "{outside}");
}

#[test]
fn files_that_include_each_other_are_refused_with_the_chain() {
    let files = Files::new(&[
        ("a.yaml", "content:\n  include: b.yaml\n"),
        ("b.yaml", "content:\n  include: a.yaml\n"),
    ]);
    let error = refused(
        &including("    - include: a.yaml\n"),
        &LoadOptions::new().files(&files),
    );
    assert!(
        error.contains("file 'a.yaml' uses itself: a.yaml -> b.yaml -> a.yaml"),
        "{error}"
    );
}

#[test]
fn the_depth_the_number_of_uses_and_the_size_are_limited() {
    // A chain of files one deeper than allowed
    let mut list = Vec::new();
    for level in 0..10 {
        list.push((
            Box::leak(format!("f{level}.yaml").into_boxed_str()) as &'static str,
            format!("content:\n  include: f{}.yaml\n", level + 1),
        ));
    }
    list.push(("f10.yaml", "content:\n  id: end\n".to_owned()));
    let files = Files {
        files: list,
        reads: RefCell::new(Vec::new()),
    };
    let error = refused(
        &including("    - include: f0.yaml\n"),
        &LoadOptions::new().files(&files),
    );
    assert!(error.contains("more than 8 levels deep"), "{error}");
    // More places of use than allowed
    let many: String = "    - use: t\n".repeat(10_001);
    let text = format!(
        "{HEAD}templates:\n  t:\n    content:\n      id: x\ntopology:\n  nodes: [client, server]\n  links:\n{many}{WORKLOADS}"
    );
    let error = refused(&text, &LoadOptions::new());
    assert!(error.contains("more than 10000 places of use"), "{error}");
}

#[test]
fn the_names_are_checked() {
    let text = format!(
        "{HEAD}params:\n  Bad-Name: 1\n  ok: 2\n  ok: 3\ntemplates:\n  Bad_Template:\n    content: x\ntopology:\n  nodes: [client, server]\n  links: []\n{WORKLOADS}"
    );
    let error = refused(&text, &LoadOptions::new());
    assert!(
        error.contains("`Bad-Name` is not a name of a parameter"),
        "{error}"
    );
    assert!(error.contains("`ok` is declared twice"), "{error}");
    assert!(
        error.contains("`Bad_Template` is not a name of a template"),
        "{error}"
    );
}

/// Nothing in the pass may panic, whatever the text: made-up files from the parts of this format.
#[test]
fn garbage_never_panics_and_what_is_read_is_a_valid_scenario() {
    let parts = [
        "{",
        "}",
        "\"{x}\"",
        "\"{rtt / 2}\"",
        "use: t",
        "include: f.yaml",
        "with: { a: 1 }",
        "params:",
        "templates:",
        "content:",
        "  ",
        "\n",
        ": ",
        "- ",
        "x",
        "1ms",
        "{{",
        "\"{1/0}\"",
        "\"{-(-(1))}\"",
        "[",
        "]",
        "~",
        "'",
        "\"",
    ];
    let files = Files::new(&[
        ("f.yaml", "content:\n  use: t\n"),
        ("g.yaml", "content: [\n"),
    ]);
    let mut state = 0x1234_5678_9abc_def0_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % 1_000_003).unwrap()
    };
    for _ in 0..3000 {
        let mut text = String::from("version: 0\nname: t\nduration: 5s\n");
        for _ in 0..(next() % 14) {
            text.push_str(parts[next() % parts.len()]);
            if next() % 3 == 0 {
                text.push('\n');
            }
        }
        if let Ok(scenario) = Scenario::from_yaml_with(
            &text,
            "g.yaml",
            &LoadOptions::new().files(&files).param("x", "1"),
        ) {
            // A scenario that was read is valid, and can be written and read again to the same scenario and the same hash
            assert_eq!(scenario.validate(), Vec::new(), "{text:?}");
            let again = Scenario::from_yaml(&scenario.to_yaml().unwrap(), "again.yaml").unwrap();
            assert_eq!(again, scenario, "{text:?}");
            assert_eq!(
                serde_json::to_string(&again).unwrap(),
                serde_json::to_string(&scenario).unwrap()
            );
        }
    }
}

#[test]
fn an_expression_inside_a_longer_text_is_replaced_by_its_value() {
    let text = format!(
        "{HEAD}params:\n  n: 3\n  rtt: 40ms\ntopology:\n  nodes: [client, server]\n  links:\n    - id: \"access-{{n}}\"\n      between: [client, server]\n      both: {{ delay: \"{{rtt / 2}}\" }}\n    - id: access-{{n + 1}}-b\n      between: [client, server]\n{WORKLOADS}"
    );
    let scenario = read(&text, &LoadOptions::new()).unwrap();
    let ids: Vec<&str> = scenario
        .topology
        .links
        .iter()
        .map(|l| l.id.as_str())
        .collect();
    assert_eq!(ids, ["access-3", "access-4-b"]);
    // A broken expression in the text is shown at the text
    let error = refused(&text.replace("{n + 1}", "{n + 1ms}"), &LoadOptions::new());
    assert!(
        error.contains("cannot add a number and a duration"),
        "{error}"
    );
    // A brace written twice and a placeholder of a command are kept
    let kept = format!(
        "{HEAD}topology:\n  nodes: [client, server]\n  links:\n    - {{ id: a, between: [client, server] }}\nworkloads:\n  - id: echo\n    kind: command\n    node: server\n    run: [\"echo\", \"{{{{x}}}}\", \"{{server.addr}}\"]\nparams:\n  p: 1\n"
    );
    let rendered = read(&kept, &LoadOptions::new()).unwrap().to_yaml().unwrap();
    assert!(
        rendered.contains("{{x}}") && rendered.contains("{server.addr}"),
        "{rendered}"
    );
}

/// The uses of a template share the lines of its content, so their faults are found at one place. Their order is that of the
/// uses (`links[2]` before `links[10]`) and the same on every read; the fuzz target found it reading a file twice (the order
/// was that of a hash map).
#[test]
fn faults_at_one_place_come_in_the_order_of_the_uses() {
    let mut yaml = String::from(
        "version: 0\nname: t\nduration: 5s\ntemplates:\n  access:\n    params: { id: , node: }\n    content:\n      id: \"{id}\"\n      betweon: [\"{node}\", hub]\ntopology:\n  nodes: [a, hub]\n  links:\n",
    );
    for index in 0..12 {
        writeln!(
            yaml,
            "    - use: access\n      with: {{ id: l{index}, node: a }}"
        )
        .unwrap();
    }
    let first = Scenario::from_yaml(&yaml, "t.yaml").unwrap_err();
    let uses: Vec<String> = first
        .problems()
        .iter()
        .filter(|problem| problem.message.contains("`between` is missing"))
        .map(|problem| problem.path.clone().unwrap())
        .collect();
    let expected: Vec<String> = (0..12)
        .map(|index| format!("topology.links[{index}]"))
        .collect();
    assert_eq!(uses, expected);
    // A hash map is different on every read
    for _ in 0..20 {
        assert_eq!(
            Scenario::from_yaml(&yaml, "t.yaml").unwrap_err().render(),
            first.render()
        );
    }
}
