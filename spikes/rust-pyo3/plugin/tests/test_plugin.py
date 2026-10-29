def test_fixture_comes_from_the_plugin(scenario_runner):
    assert scenario_runner.parse_duration_ns("2ms") == 2_000_000
