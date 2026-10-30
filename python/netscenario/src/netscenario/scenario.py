"""Scenarios: read from a file, or built in code.

A scenario is the same model whichever way it is made. Reading, checking and every rule live in
the Rust core (``docs/SCENARIO-FORMAT.md``); this module only holds the text and the answer. Values
with a unit are strings, as in the file: ``"40ms"``, ``"10mbit"``, ``"1200b"``, ``"1%"``. A bare
number is refused by the core, because ``40`` could be 40 seconds or 40 milliseconds.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Literal, Self

from netscenario import _native
from netscenario.errors import ScenarioError, ScenarioProblem

JitterDistribution = Literal["uniform", "normal"]
"""The distribution of a jitter."""


@dataclass(frozen=True)
class Jitter:
    """How much the delay varies."""

    amount: str
    """The amount, for example ``"5ms"``.

    The delay lies within this amount above and below the fixed delay (``uniform``) or has this
    standard deviation (``normal``).
    """
    distribution: JitterDistribution = "uniform"
    """``"uniform"`` (the default) or ``"normal"``."""

    def to_data(self) -> dict[str, str]:
        """The jitter as the plain data of the file format."""
        data = {"amount": self.amount}
        if self.distribution != "uniform":
            data["distribution"] = self.distribution
        return data


@dataclass(frozen=True)
class Loss:
    """Packet loss: each packet is lost with the same probability."""

    p: str
    """The probability, always with the percent sign: ``"1%"``, ``"0.1%"``."""

    def to_data(self) -> dict[str, str]:
        """The loss as the plain data of the file format."""
        return {"model": "random", "p": self.p}


@dataclass(frozen=True)
class Impairments:
    """What a link does to the packets of one direction.

    The stages act in this order, whatever order they are written in: first the loss, then the rate,
    then the delay with its jitter (``docs/SCENARIO-FORMAT.md``).
    """

    delay: str | None = None
    """A fixed delay, for example ``"40ms"``."""
    jitter: Jitter | str | None = None
    """A jitter. A string is the amount of a uniform jitter: ``"5ms"``. Needs ``delay``."""
    loss: Loss | str | None = None
    """A loss. A string is the probability of a random loss: ``"1%"``."""
    rate: str | None = None
    """A limit on the rate of the link, for example ``"10mbit"``."""

    def to_data(self) -> dict[str, Any]:
        """The impairments as the plain data of the file format; what is not set is left out."""
        data: dict[str, Any] = {}
        if self.delay is not None:
            data["delay"] = self.delay
        if self.jitter is not None:
            jitter = Jitter(self.jitter) if isinstance(self.jitter, str) else self.jitter
            data["jitter"] = jitter.to_data()
        if self.loss is not None:
            loss = Loss(self.loss) if isinstance(self.loss, str) else self.loss
            data["loss"] = loss.to_data()
        if self.rate is not None:
            data["rate"] = self.rate
        return data


def _without_places(error: ScenarioError) -> ScenarioError:
    """Make the error of a scenario that was built in code.

    The lines of the generated text mean nothing, so the problems keep their paths and lose their
    places.
    """
    problems = tuple(replace(problem, line=None, column=None) for problem in error.problems)
    lines: list[str] = []
    for problem in problems:
        lines.append(f"error: {problem.message}")
        if problem.path:
            lines.append(f"  = path: {problem.path}")
        if problem.help:
            lines.append(f"  = help: {problem.help}")
    return ScenarioError("\n".join(lines), problems)


class Scenario:
    """A scenario that has been read and checked.

    Make one with :meth:`load`, :meth:`from_yaml` or :meth:`builder`. It is immutable;
    :meth:`with_seed` gives a changed copy.
    """

    def __init__(self, source: str, source_name: str, canonical: str) -> None:
        """Hold a scenario that the core has checked.

        Use :meth:`load`, :meth:`from_yaml` or :meth:`builder` to make one.
        """
        self._source = source
        self._source_name = source_name
        self._canonical = canonical
        self._data: dict[str, Any] = json.loads(canonical)

    @classmethod
    def from_yaml(cls, text: str, *, name: str = "<string>") -> Self:
        """Read a scenario from YAML (or JSON) text.

        Args:
            text: The scenario.
            name: What messages call the text, usually the name of a file.

        Raises:
            ScenarioError: The scenario is not valid. Every problem is listed, with line, column,
                path and a hint.
        """
        return cls(text, name, _native.parse_scenario(text, name))

    @classmethod
    def load(cls, path: str | os.PathLike[str]) -> Self:
        """Read a scenario from a file.

        Raises:
            ScenarioError: The scenario is not valid.
            OSError: The file cannot be read.
        """
        file = Path(path)
        return cls.from_yaml(file.read_text(encoding="utf-8"), name=str(path))

    @staticmethod
    def builder(
        name: str,
        *,
        duration: str,
        seed: int | None = None,
        backend: Literal["auto", "kernel"] | None = None,
    ) -> ScenarioBuilder:
        """Start building a scenario in code.

        Args:
            name: The name of the scenario; it names the files of the result.
            duration: The length of the measured phase, for example ``"30s"``.
            seed: The seed from which the seeds of all random parts are derived. Without it a run
                draws one and records it.
            backend: ``"auto"`` (the default) or ``"kernel"``.
        """
        return ScenarioBuilder(name, duration=duration, seed=seed, backend=backend)

    @property
    def name(self) -> str:
        """The name of the scenario."""
        return str(self._data["name"])

    @property
    def seed(self) -> int | None:
        """The seed written in the scenario, or ``None``."""
        seed = self._data.get("seed")
        return None if seed is None else int(seed)

    @property
    def data(self) -> dict[str, Any]:
        """The scenario as plain data in the canonical form: the keys of the format, no defaults.

        It is a copy; changing it changes nothing.
        """
        copy: dict[str, Any] = json.loads(self._canonical)
        return copy

    @property
    def source(self) -> str:
        """The text the scenario was read from (the text that errors refer to)."""
        return self._source

    @property
    def source_name(self) -> str:
        """The name of the text, usually a file name."""
        return self._source_name

    def needs(self) -> tuple[str, ...]:
        """The ids of the features that a backend has to reproduce for this scenario.

        They are the rows of ``docs/COVERAGE.md``, in the order of their first use.
        """
        return tuple(_native.needs(self._source, self._source_name))

    def to_json(self) -> str:
        """The scenario as canonical JSON, which is also valid YAML."""
        return self._canonical

    def with_seed(self, seed: int | None) -> Scenario:
        """A copy of the scenario with another seed (``None`` removes it)."""
        data = self.data
        if seed is None:
            data.pop("seed", None)
        else:
            data["seed"] = seed
        text = json.dumps(data, indent=2)
        return Scenario(text, self._source_name, _native.parse_scenario(text, self._source_name))

    def __repr__(self) -> str:
        """Name and source of the scenario."""
        return f"Scenario(name={self.name!r}, source={self._source_name!r})"


class ScenarioBuilder:
    """Builds a scenario in code. The rules are those of the file; :meth:`build` asks the core.

    Example:
        >>> scenario = (
        ...     Scenario.builder("lte-like", duration="30s", seed=42)
        ...     .nodes("client", "server")
        ...     .link(
        ...         "access", "client", "server",
        ...         forward=Impairments(
        ...             delay="40ms", jitter=Jitter("5ms", "normal"), loss="1%", rate="10mbit"
        ...         ),
        ...         reverse=Impairments(delay="40ms", rate="20mbit"),
        ...     )
        ...     .udp_cbr("media", "client", "server", rate="2mbit", packet_size="1200b")
        ...     .build()
        ... )
    """

    def __init__(
        self,
        name: str,
        *,
        duration: str,
        seed: int | None = None,
        backend: Literal["auto", "kernel"] | None = None,
    ) -> None:
        """Start a scenario (use :meth:`Scenario.builder`)."""
        self._data: dict[str, Any] = {"version": 0, "name": name, "duration": duration}
        if seed is not None:
            self._data["seed"] = seed
        if backend is not None:
            self._data["backend"] = backend
        self._nodes: list[str] = []
        self._links: list[dict[str, Any]] = []
        self._workloads: list[dict[str, Any]] = []

    def nodes(self, *names: str) -> Self:
        """Add nodes. Release 0.1 has exactly two."""
        self._nodes.extend(names)
        return self

    def link(
        self,
        id: str,
        first: str,
        second: str,
        *,
        both: Impairments | None = None,
        forward: Impairments | None = None,
        reverse: Impairments | None = None,
    ) -> Self:
        """Add a link between two nodes.

        Args:
            id: The name of the link.
            first: The first node. ``forward`` is the direction from here.
            second: The second node.
            both: The same values in both directions. It cannot be combined with ``forward`` or
                ``reverse``.
            forward: The values for the direction from ``first`` to ``second``.
            reverse: The values for the way back.
        """
        link: dict[str, Any] = {"id": id, "between": [first, second]}
        for key, impairments in (("both", both), ("forward", forward), ("reverse", reverse)):
            if impairments is not None:
                link[key] = impairments.to_data()
        self._links.append(link)
        return self

    def udp_cbr(
        self,
        id: str,
        source: str,
        destination: str,
        *,
        rate: str,
        packet_size: str,
        start: str | None = None,
        stop: str | None = None,
    ) -> Self:
        """Add a workload of UDP packets at a constant bit rate.

        Args:
            id: The name of the workload; the metrics are named after it.
            source: The node that sends.
            destination: The node that receives.
            rate: The rate of the payload, for example ``"2mbit"``.
            packet_size: The payload of one packet, for example ``"1200b"``.
            start: When the workload starts, from the start of the measured phase (default
                ``"0s"``).
            stop: When it stops (default: the end of the measured phase).
        """
        workload: dict[str, Any] = {
            "id": id,
            "kind": "udp-cbr",
            "from": source,
            "to": destination,
            "rate": rate,
            "packet-size": packet_size,
        }
        if start is not None:
            workload["start"] = start
        if stop is not None:
            workload["stop"] = stop
        self._workloads.append(workload)
        return self

    def build(self) -> Scenario:
        """Check the scenario with the core and return it.

        Raises:
            ScenarioError: The scenario is not valid. The problems have paths, but no lines,
                because there is no file.
        """
        data = dict(self._data)
        data["topology"] = {"nodes": list(self._nodes), "links": list(self._links)}
        data["workloads"] = list(self._workloads)
        text = json.dumps(data, indent=2)
        try:
            return Scenario(text, "<builder>", _native.parse_scenario(text, "<builder>"))
        except ScenarioError as error:
            raise _without_places(error) from None


__all__ = [
    "Impairments",
    "Jitter",
    "JitterDistribution",
    "Loss",
    "Scenario",
    "ScenarioBuilder",
    "ScenarioProblem",
]
