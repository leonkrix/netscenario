import threading
import time

import pytest

import netscenario_spike as ns


def test_parse_units():
    assert ns.parse_duration_ns("20ms") == 20_000_000
    assert ns.parse_duration_ns("1.5s") == 1_500_000_000
    assert ns.parse_duration_ns("250us") == 250_000
    assert ns.seconds("20ms") == pytest.approx(0.02)


def test_errors_name_the_input():
    with pytest.raises(ValueError, match="no unit"):
        ns.parse_duration_ns("20")
    with pytest.raises(ValueError, match="unknown unit 'min'"):
        ns.parse_duration_ns("3min")


def test_canary_the_assertion_can_fail():
    with pytest.raises(AssertionError):
        assert ns.parse_duration_ns("20ms") == 21_000_000


def test_gil_is_released():
    ticks = []
    stop = threading.Event()

    def tick():
        while not stop.is_set():
            ticks.append(time.monotonic())
            time.sleep(0.005)

    t = threading.Thread(target=tick)
    t.start()
    waited = ns.busy_wait_ms(200)
    stop.set()
    t.join()
    assert waited >= 200
    assert len(ticks) > 10  # another thread ran while Rust waited
