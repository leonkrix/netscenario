//! `expect` (ASR-07, D-094): a file says what it must show, and the reader refuses a check that cannot be judged.

// A test file: a failing unwrap is a failing test
#![allow(clippy::unwrap_used, clippy::expect_used)]

use netscenario_model::{Bound, LoadOptions, MAX_CHECK_NAME, MetricUnit, Op, Scenario, Statistic};

const HEAD: &str = "version: 0
name: t
duration: 5s
topology:
  nodes: [a, b]
  links:
    - id: l
      between: [a, b]
      both: { delay: 20ms }
workloads:
  - { id: media, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }
  - { id: bulk, kind: tcp-bulk, from: a, to: b, bytes: 1mb }
  - { id: echoed, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b, echo: true }
  - { id: app, kind: command, node: a, run: [\"true\"] }
";

fn read(expect: &str) -> Result<Scenario, String> {
    Scenario::from_yaml(&format!("{HEAD}{expect}"), "t.yaml").map_err(|error| error.to_string())
}

fn refused(expect: &str) -> String {
    match read(expect) {
        Ok(_) => panic!("accepted:\n{expect}"),
        Err(message) => message,
    }
}

#[test]
fn a_file_with_checks_on_one_run_is_read_and_written_back() {
    let scenario = read(
        "expect:
  checks:
    - name: the stream arrives
      metric: media.loss_ratio
      value: \"< 1%\"
    - { metric: media.delay.p99, value: \"<= 150ms\" }
    - { metric: bulk.goodput.mean, value: \">= 8mbit\" }
    - { metric: media.sent, value: \"> 0\" }
    - { metric: app.queue_depth.max, value: \"< 50\" }
",
    )
    .unwrap();
    let expect = scenario.expect.as_ref().unwrap();
    assert!(!expect.is_set());
    assert_eq!(expect.checks.len(), 5);
    let first = &expect.checks[0];
    assert_eq!(first.name.as_deref(), Some("the stream arrives"));
    let (statistic, bound) = first.bounds().next().unwrap();
    assert_eq!(statistic, Statistic::Value);
    assert_eq!(bound.op(), Op::Less);
    assert_eq!(bound.threshold(MetricUnit::Ratio).unwrap(), 0.01);
    // Written out and read again, it is the same scenario and the same text
    let written = scenario.to_yaml().unwrap();
    let again = Scenario::from_yaml(&written, "again.yaml").unwrap();
    assert_eq!(again, scenario);
    assert_eq!(again.to_yaml().unwrap(), written);
    assert!(written.contains("< 1%"), "{written}");
}

#[test]
fn a_file_with_repetitions_is_judged_by_a_set() {
    let scenario = read(
        "expect:
  repetitions: 20
  checks:
    - { metric: media.loss_ratio, ci95-high: \"< 2%\", mean: \"< 1.5%\" }
    - { metric: media.delay.p99, max: \"< 200ms\", median: \"< 100ms\", ci95-low: \"> 1ms\", min: \"> 0.5ms\" }
",
    )
    .unwrap();
    let expect = scenario.expect.as_ref().unwrap();
    assert!(expect.is_set());
    assert_eq!(expect.repetitions, Some(20));
    let statistics: Vec<Statistic> = expect.bounds().map(|(_, statistic, _)| statistic).collect();
    assert_eq!(
        statistics,
        [
            Statistic::Mean,
            Statistic::Ci95High,
            Statistic::Median,
            Statistic::Min,
            Statistic::Max,
            Statistic::Ci95Low,
        ]
    );
}

#[test]
fn a_check_without_a_name_is_called_by_its_metric_statistic_and_bound() {
    let scenario = read(
        "expect:
  checks:
    - { metric: media.loss_ratio, value: \"< 1%\" }
    - { name: both ends, metric: media.sent, value: \"> 10\" }
",
    )
    .unwrap();
    let checks = &scenario.expect.as_ref().unwrap().checks;
    let (statistic, bound) = checks[0].bounds().next().unwrap();
    assert_eq!(
        checks[0].label(statistic, bound),
        "media.loss_ratio value < 1%"
    );
    let (statistic, bound) = checks[1].bounds().next().unwrap();
    assert_eq!(checks[1].label(statistic, bound), "both ends");
}

#[test]
fn a_metric_that_does_not_exist_is_refused_with_a_hint() {
    let message =
        refused("expect:\n  checks:\n    - { metric: media.delay.maxx, value: \"< 1s\" }\n");
    assert!(
        message.contains("'delay.maxx' is not a metric of the workload 'media'"),
        "{message}"
    );
    assert!(
        message.contains("did you mean 'media.delay.max'?"),
        "{message}"
    );
    assert!(message.contains("expect.checks[0].metric"), "{message}");

    let message = refused("expect:\n  checks:\n    - { metric: medai.sent, value: \"> 1\" }\n");
    assert!(
        message.contains("there is no workload 'medai'"),
        "{message}"
    );
    assert!(message.contains("did you mean 'media'?"), "{message}");

    // The round trip belongs to a workload that echoes
    let message =
        refused("expect:\n  checks:\n    - { metric: media.round_trip.p99, value: \"< 1s\" }\n");
    assert!(message.contains("needs `echo: true`"), "{message}");
    read("expect:\n  checks:\n    - { metric: echoed.round_trip.p99, value: \"< 1s\" }\n").unwrap();

    // The metrics of a TCP workload are not those of a UDP one, and the reverse
    let message =
        refused("expect:\n  checks:\n    - { metric: bulk.delay.p99, value: \"< 1s\" }\n");
    assert!(
        message.contains("'delay.p99' is not a metric of the workload 'bulk'"),
        "{message}"
    );
    let message =
        refused("expect:\n  checks:\n    - { metric: media.goodput.mean, value: \"> 1mbit\" }\n");
    assert!(
        message.contains("'goodput.mean' is not a metric"),
        "{message}"
    );

    // A program names its own metrics, but only with the three statistics of the run
    let message =
        refused("expect:\n  checks:\n    - { metric: app.queue_depth, value: \"< 5\" }\n");
    assert!(
        message.contains("not a metric of the program 'app'"),
        "{message}"
    );
}

#[test]
fn a_bound_has_the_unit_of_its_metric() {
    for (check, wanted) in [
        (
            "{ metric: media.delay.p99, value: \"< 150\" }",
            "a duration",
        ),
        (
            "{ metric: media.delay.p99, value: \"< 8mbit\" }",
            "a duration",
        ),
        (
            "{ metric: bulk.goodput.mean, value: \"> 150ms\" }",
            "a rate",
        ),
        (
            "{ metric: media.loss_ratio, value: \"< 5\" }",
            "a percentage",
        ),
        (
            "{ metric: media.loss_ratio, value: \"< 150%\" }",
            "above 100 %",
        ),
        ("{ metric: media.sent, value: \"> 5%\" }", "a plain number"),
        (
            "{ metric: media.sent, value: \"> lots\" }",
            "a plain number",
        ),
        (
            "{ metric: media.sent, value: \"lots\" }",
            "begins with no operator",
        ),
        (
            "{ metric: media.sent, value: \"==\" }",
            "begins with no operator",
        ),
        ("{ metric: media.sent, value: \">\" }", "has no value"),
    ] {
        let message = refused(&format!("expect:\n  checks:\n    - {check}\n"));
        assert!(message.contains(wanted), "{check}: {message}");
        // Where it is wrong is shown, as a path when the model refuses it and at the line when the reader does
        assert!(message.contains("t.yaml:17"), "{check}: {message}");
    }
    // A number without an operator is not a bound either
    let message = refused("expect:\n  checks:\n    - { metric: media.sent, value: 5 }\n");
    assert!(message.contains("begins with no operator"), "{message}");
}

#[test]
fn the_statistics_fit_the_number_of_runs() {
    let message =
        refused("expect:\n  checks:\n    - { metric: media.loss_ratio, ci95-high: \"< 2%\" }\n");
    assert!(
        message.contains("a statistic of a set of runs, and this file is judged by one run"),
        "{message}"
    );
    assert!(message.contains("repetitions: 5"), "{message}");
    let message = refused(
        "expect:\n  repetitions: 10\n  checks:\n    - { metric: media.loss_ratio, value: \"< 2%\" }\n",
    );
    assert!(
        message.contains("the value of one run, and this file is judged by a set"),
        "{message}"
    );
    // A check has a bound, a file has a check, and a set has its bounds
    let message = refused("expect:\n  checks:\n    - { metric: media.loss_ratio }\n");
    assert!(message.contains("a check has no bound"), "{message}");
    let message = refused("expect:\n  checks: []\n");
    assert!(message.contains("`expect` has no check"), "{message}");
    for repetitions in [0, 4, 1001] {
        let message = refused(&format!(
            "expect:\n  repetitions: {repetitions}\n  checks:\n    - {{ metric: media.loss_ratio, mean: \"< 2%\" }}\n"
        ));
        assert!(message.contains("it is from 5"), "{repetitions}: {message}");
    }
}

#[test]
fn names_are_one_line_and_each_check_is_called_differently() {
    let message =
        refused("expect:\n  checks:\n    - { name: \"\", metric: media.sent, value: \"> 1\" }\n");
    assert!(
        message.contains("one line of 1 to 120 characters"),
        "{message}"
    );
    let long = "x".repeat(MAX_CHECK_NAME + 1);
    let message = refused(&format!(
        "expect:\n  checks:\n    - {{ name: {long}, metric: media.sent, value: \"> 1\" }}\n"
    ));
    assert!(
        message.contains("one line of 1 to 120 characters"),
        "{message}"
    );
    let message = refused(
        "expect:\n  checks:\n    - { name: same, metric: media.sent, value: \"> 1\" }\n    - { name: same, metric: media.lost, value: \"< 1\" }\n",
    );
    assert!(
        message.contains("two checks are called 'same'"),
        "{message}"
    );
    // The same metric twice without a name is told apart by its bound; the same bound twice is the same check
    read("expect:\n  checks:\n    - { metric: media.sent, value: \"> 1\" }\n    - { metric: media.sent, value: \"< 10\" }\n").unwrap();
    let message = refused(
        "expect:\n  checks:\n    - { metric: media.sent, value: \"> 1\" }\n    - { metric: media.sent, value: \"> 1\" }\n",
    );
    assert!(message.contains("two checks are called"), "{message}");
}

#[test]
fn unknown_keys_are_refused_with_the_keys_that_exist() {
    let message = refused("expect:\n  checks:\n    - { metric: media.sent, vaule: \"> 1\" }\n");
    assert!(message.contains("vaule"), "{message}");
    let message = refused("expect:\n  checks:\n    - { value: \"> 1\" }\n");
    assert!(message.contains("`metric` is missing"), "{message}");
    let message =
        refused("expect:\n  retries: 3\n  checks:\n    - { metric: media.sent, value: \"> 1\" }\n");
    assert!(message.contains("retries"), "{message}");
}

#[test]
fn a_bound_may_be_computed_from_the_parameters() {
    let yaml = HEAD.replacen("name: t\n", "name: t\nparams:\n  rtt: 40ms\n", 1)
        + "expect:
  checks:
    - { metric: media.delay.p99, value: \"< {rtt * 3 + 10ms}\" }
";
    let scenario = Scenario::from_yaml(&yaml, "t.yaml").unwrap_or_else(|error| panic!("{error}"));
    let check = &scenario.expect.as_ref().unwrap().checks[0];
    let bound = check.value.as_ref().unwrap();
    assert_eq!(bound.to_string(), "< 130ms");
    let other = Scenario::from_yaml_with(&yaml, "t.yaml", &LoadOptions::new().param("rtt", "10ms"))
        .unwrap();
    assert_eq!(
        other.expect.as_ref().unwrap().checks[0]
            .value
            .as_ref()
            .unwrap()
            .to_string(),
        "< 40ms"
    );
}

#[test]
fn a_bound_is_part_of_the_scenario_and_so_of_its_hash() {
    let one = read("expect:\n  checks:\n    - { metric: media.sent, value: \"> 1\" }\n").unwrap();
    let two = read("expect:\n  checks:\n    - { metric: media.sent, value: \"> 2\" }\n").unwrap();
    let none = read("").unwrap();
    // The hash is that of the canonical JSON, so what the JSON says is what the hash covers
    let json = |scenario: &Scenario| serde_json::to_string(scenario).unwrap();
    assert_ne!(json(&one), json(&two));
    assert_ne!(json(&one), json(&none));
    // A file without `expect` is written as before
    assert!(!none.to_yaml().unwrap().contains("expect"));
}

#[test]
fn the_names_of_the_metrics_are_those_of_the_scenario() {
    let scenario = read("").unwrap();
    let names = scenario.metric_names();
    // media: 12 + 8 delay, echoed: the same and the echo, bulk: the TCP ones, the program has none that are known beforehand
    for name in [
        "media.sent",
        "media.delay.p999",
        "media.jitter.last",
        "echoed.round_trip.min",
        "echoed.echo.loss_ratio",
        "bulk.goodput.mean",
        "bulk.rtt.p99",
    ] {
        assert!(names.iter().any(|known| known == name), "{name}");
    }
    assert!(!names.iter().any(|name| name.starts_with("app.")));
    assert!(!names.iter().any(|name| name == "media.round_trip.min"));
    assert_eq!(
        scenario.metric_unit_of("media.delay.p99"),
        Some(MetricUnit::Seconds)
    );
    assert_eq!(
        scenario.metric_unit_of("app.delay.max"),
        Some(MetricUnit::Number)
    );
    assert_eq!(scenario.metric_unit_of("media.nothing"), None);
}

#[test]
fn a_bound_can_be_built_in_code() {
    let bound = Bound::new(Op::GreaterOrEqual, "97%");
    assert_eq!(bound.to_string(), ">= 97%");
    assert!((bound.threshold(MetricUnit::Ratio).unwrap() - 0.97).abs() < 1e-12);
    assert!(bound.op().holds(0.98, 0.97));
    assert!(!bound.op().holds(0.96, 0.97));
    assert!(Op::Less.margin(0.8, 1.0) > 0.0);
    assert!(Op::Greater.margin(0.8, 1.0) < 0.0);
}
