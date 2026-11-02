"""A set of runs of one scenario, and what it says about each metric (ASR-02, ASR-03).

``netscenario.run(scenario, repetitions=10)`` runs the scenario ten times, one after the other,
with seeds derived from one seed, and returns a :class:`RunSet`. For each metric that every run has
it holds the value of each run, the usual summary and an :class:`Interval` around the mean or the
median. The method, what the interval promises and what it does not are in ``docs/STATISTICS.md``.

The names of the metrics are those of the plugin and the reports: ``<workload>.<what>`` in SI
units (``media.delay.p99`` in seconds, ``media.loss_ratio`` from 0 to 1,
``media.throughput.mean`` in bits per second).

Example:
    >>> runs = ns.run("lte-like.yaml", repetitions=10)  # doctest: +SKIP
    >>> assert runs.metric("media.loss_ratio").ci95_high < 0.05  # doctest: +SKIP
"""

from __future__ import annotations

import difflib
import json
import math
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any

from netscenario.results import Result

_LEVEL_95 = 0.95


@dataclass(frozen=True)
class Interval:
    """An interval around the mean or the median of the values of a set of runs."""

    statistic: str
    """What the interval is around: ``"mean"`` or ``"median"``."""
    estimate: float
    """The statistic of the values themselves."""
    low: float
    """The lower end."""
    high: float
    """The upper end."""
    level: float
    """The level the interval is meant to have, from 0 to 1 (0.95 for 95 %)."""
    method: str
    """How the ends were chosen: ``"expanded-percentile"`` (the mean) or ``"percentile"``."""
    tail: float
    """The probability that was left out of the bootstrap draws at each end."""
    resamples: int
    """How many draws the bootstrap made."""
    runs: int
    """How many values the interval is made from."""

    @property
    def width(self) -> float:
        """``high - low``."""
        return self.high - self.low


@dataclass(frozen=True)
class SetMetric:
    """What the runs of a set say about one metric that every one of them has."""

    name: str
    """The name, for example ``"media.delay.p99"``."""
    values: tuple[float, ...]
    """The value of each run, in the order of the runs."""
    mean: float
    """The mean of the values."""
    median: float
    """The median of the values."""
    min: float
    """The smallest value."""
    max: float
    """The largest value."""
    stddev: float
    """The sample standard deviation (divided by ``n - 1``): how much one run varies."""
    interval: Interval
    """The interval around the statistic that the set was asked for (the mean by default)."""

    @property
    def low(self) -> float:
        """The lower end of the interval, for the level of the set."""
        return self.interval.low

    @property
    def high(self) -> float:
        """The upper end of the interval, for the level of the set."""
        return self.interval.high

    @property
    def ci95_low(self) -> float:
        """The lower end of the 95 % interval.

        Raises:
            ValueError: The set was made with another level; use :attr:`low`, which is for the level
                of the set, rather than a number that does not mean what its name says.
        """
        return self._at_95().low

    @property
    def ci95_high(self) -> float:
        """The upper end of the 95 % interval.

        Raises:
            ValueError: The set was made with another level (see :attr:`ci95_low`).
        """
        return self._at_95().high

    def _at_95(self) -> Interval:
        if not math.isclose(self.interval.level, _LEVEL_95, abs_tol=1e-9):
            message = (
                f"the interval of '{self.name}' has the level {self.interval.level:g}, not 0.95; "
                "use .low and .high, which are for the level of the set"
            )
            raise ValueError(message)
        return self.interval


@dataclass(frozen=True)
class PartialMetric:
    """A metric that only some runs have, so it has no interval.

    An example is the delay of a run in which no packet arrived. An interval from the runs that
    happen to have it would be a made-up number.
    """

    name: str
    """The name."""
    present_in: int
    """In how many runs it was there."""
    of: int
    """How many runs there were."""


@dataclass(frozen=True)
class SetRun:
    """One run of the set."""

    index: int
    """The number of the run, from 0."""
    seed: int
    """The seed of the run, derived from the seed of the set."""
    directory: Path
    """The directory of the run, with its ``result.json``."""
    loss_reproducible: bool
    """Whether the same seed gives the same lost packets in this run."""


@dataclass(frozen=True)
class SetSeed:
    """The seed of the set."""

    value: int
    """The seed from which the seeds of the runs and of the bootstrap were derived."""
    drawn: bool
    """Whether it was drawn because the scenario has none. Repeat the set with ``seed=value``."""


@dataclass(frozen=True)
class Confidence:
    """How the intervals of the set were made."""

    statistic: str
    """What they are around: ``"mean"`` or ``"median"``."""
    level: float
    """The level, from 0 to 1."""
    resamples: int
    """The number of draws of the bootstrap."""
    method: str
    """How the ends were chosen."""


@dataclass(frozen=True)
class RunSet:
    """The runs of a scenario repeated with derived seeds, and what they say together.

    ``runs.metric("media.loss_ratio")`` is the :class:`SetMetric` of that name. ``runs.results``
    reads the :class:`netscenario.Result` of each run.
    """

    scenario_name: str
    """The name of the scenario."""
    scenario_hash: str
    """``sha256:`` and the fingerprint of the scenario **without its seed**, because every run has a
    seed of its own."""
    seed: SetSeed
    """The seed of the set."""
    repetitions: int
    """How many runs."""
    confidence: Confidence
    """How the intervals were made."""
    runs: tuple[SetRun, ...]
    """The runs, in order."""
    metrics: Mapping[str, SetMetric]
    """The metrics that every run has, by name, sorted by name."""
    partial: tuple[PartialMetric, ...]
    """The metrics that only some runs have; they have no interval."""
    out_dir: Path
    """The directory of the set: ``runset.json`` and one directory for each run."""
    raw: Mapping[str, Any]
    """The decoded ``runset.json``, unchanged."""

    def metric(self, name: str) -> SetMetric:
        """The metric called ``name``.

        Raises:
            KeyError: No such metric, or not every run has it; the message says which, and names the
                metrics that look like it.
        """
        try:
            return self.metrics[name]
        except KeyError:
            for missing in self.partial:
                if missing.name == name:
                    message = (
                        f"'{name}' was there in {missing.present_in} of {missing.of} runs, so it "
                        "has no interval (for a delay: in a run in which no packet arrived)"
                    )
                    raise KeyError(message) from None
            close = difflib.get_close_matches(name, list(self.metrics), n=3)
            hint = f"; did you mean {', '.join(close)}?" if close else ""
            message = f"the set has no metric '{name}'{hint}"
            raise KeyError(message) from None

    def __iter__(self) -> Iterator[SetMetric]:
        """The metrics, sorted by name."""
        return iter(self.metrics.values())

    def __len__(self) -> int:
        """How many metrics."""
        return len(self.metrics)

    @property
    def results(self) -> tuple[Result, ...]:
        """The :class:`netscenario.Result` of each run, read from its directory.

        Each call reads the files again, so keep the tuple.
        """
        return tuple(
            Result.from_json(
                (run.directory / "result.json").read_text(encoding="utf-8"), run.directory
            )
            for run in self.runs
        )

    @classmethod
    def from_json(cls, text: str, out_dir: str | Path) -> RunSet:
        """Read the text of a ``runset.json``.

        Raises:
            ValueError: The text is not a set of a version this package reads.
        """
        raw: dict[str, Any] = json.loads(text)
        if raw.get("format") != "netscenario-runset":
            message = f"this is not a netscenario set of runs (format {raw.get('format')!r})"
            raise ValueError(message)
        if raw.get("version") != 0:
            message = f"this set has version {raw.get('version')}; this package reads version 0"
            raise ValueError(message)
        directory = Path(out_dir)
        confidence = raw["confidence"]
        metrics = {m["name"]: _metric(m) for m in raw["metrics"]}
        return cls(
            scenario_name=raw["scenario"]["name"],
            scenario_hash=raw["scenario"]["hash"],
            seed=SetSeed(value=raw["seed"]["value"], drawn=raw["seed"]["drawn"]),
            repetitions=raw["repetitions"],
            confidence=Confidence(
                statistic=confidence["statistic"],
                level=confidence["level"],
                resamples=confidence["resamples"],
                method=confidence["method"],
            ),
            runs=tuple(
                SetRun(
                    index=r["index"],
                    seed=r["seed"],
                    directory=directory / r["directory"],
                    loss_reproducible=r["loss_reproducible"],
                )
                for r in raw["runs"]
            ),
            metrics=MappingProxyType(metrics),
            partial=tuple(
                PartialMetric(name=p["name"], present_in=p["present_in"], of=p["of"])
                for p in raw.get("partial", ())
            ),
            out_dir=directory,
            raw=MappingProxyType(raw),
        )


def _metric(entry: dict[str, Any]) -> SetMetric:
    interval = entry["interval"]
    return SetMetric(
        name=entry["name"],
        values=tuple(entry["values"]),
        mean=entry["mean"],
        median=entry["median"],
        min=entry["min"],
        max=entry["max"],
        stddev=entry["stddev"],
        interval=Interval(
            statistic=interval["statistic"],
            estimate=interval["estimate"],
            low=interval["low"],
            high=interval["high"],
            level=interval["level"],
            method=interval["method"],
            tail=interval["tail"],
            resamples=interval["resamples"],
            runs=interval["runs"],
        ),
    )
