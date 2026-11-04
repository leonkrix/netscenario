"""Scenarios: read from a file, or built in code.

A scenario is the same model whichever way it is made. Reading, checking and every rule live in
the Rust core (``docs/SCENARIO-FORMAT.md``); this module only holds the text and the answer. Values
with a unit are strings, as in the file: ``"40ms"``, ``"10mbit"``, ``"1200b"``, ``"1%"``. A bare
number is refused by the core, because ``40`` could be 40 seconds or 40 milliseconds.
"""

from __future__ import annotations

import json
import os
from collections.abc import Sequence
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
    def from_yaml(
        cls,
        text: str,
        *,
        name: str = "<string>",
        base_dir: str | os.PathLike[str] | None = None,
        allow_outside_files: bool = False,
    ) -> Self:
        """Read a scenario from YAML (or JSON) text.

        Args:
            text: The scenario.
            name: What messages call the text, usually the name of a file.
            base_dir: The directory that the files named in the scenario are relative to (a table
                of delay samples). Needed only for a scenario that refers to a file; without it
                such a scenario is refused. :meth:`load` gives the directory of the file.
            allow_outside_files: Let the scenario read files from outside ``base_dir``. A path that
                leaves the directory is refused by default, so that a scenario from elsewhere
                cannot read what is not next to it.

        Raises:
            ScenarioError: The scenario is not valid, or a file it refers to cannot be used. Every
                problem is listed, with line, column, path and a hint.
        """
        base = None if base_dir is None else str(Path(base_dir))
        return cls(
            text,
            name,
            _native.parse_scenario(text, name, base, allow_outside_files),
        )

    @classmethod
    def load(cls, path: str | os.PathLike[str], *, allow_outside_files: bool = False) -> Self:
        """Read a scenario from a file.

        The files that the scenario refers to are read from the directory of the file. What is
        made of them (the table of a jitter) becomes part of the scenario, and
        :meth:`to_json` holds it, so a recorded scenario needs no file.

        Args:
            path: The scenario file.
            allow_outside_files: Let the scenario read files from outside its directory.

        Raises:
            ScenarioError: The scenario is not valid, or a file it refers to cannot be used.
            OSError: The file cannot be read.
        """
        file = Path(path)
        return cls.from_yaml(
            file.read_text(encoding="utf-8"),
            name=str(path),
            base_dir=file.resolve().parent,
            allow_outside_files=allow_outside_files,
        )

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
        udp_checksum: bool = True,
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
            udp_checksum: ``False`` sends the packets without the checksum of UDP, with a CRC-32
                in the payload instead, so that a packet the link corrupts arrives and is counted
                (``corrupted`` in the result) and is not dropped by the receiving kernel. The
                payload must then be at least 20 bytes.
        """
        return self._udp_workload(
            "udp-cbr",
            id,
            source,
            destination,
            {"rate": rate, "packet-size": packet_size},
            start,
            stop,
            udp_checksum,
        )

    def udp_poisson(
        self,
        id: str,
        source: str,
        destination: str,
        *,
        rate: str,
        packet_size: str,
        start: str | None = None,
        stop: str | None = None,
        udp_checksum: bool = True,
    ) -> Self:
        """Add a workload of UDP packets as a Poisson process.

        The gaps between the packets are exponentially distributed, drawn from the seed of the
        scenario (the same seed gives the same packets at the same moments).

        Args:
            id: The name of the workload; the metrics are named after it.
            source: The node that sends.
            destination: The node that receives.
            rate: The mean rate of the payload, for example ``"800kbit"``.
            packet_size: The payload of one packet, for example ``"400b"``.
            start: When the workload starts (default ``"0s"``).
            stop: When it stops (default: the end of the measured phase).
            udp_checksum: See :meth:`udp_cbr`.
        """
        return self._udp_workload(
            "udp-poisson",
            id,
            source,
            destination,
            {"rate": rate, "packet-size": packet_size},
            start,
            stop,
            udp_checksum,
        )

    def udp_onoff(
        self,
        id: str,
        source: str,
        destination: str,
        *,
        rate: str,
        packet_size: str,
        on: str,
        off: str,
        distribution: str = "fixed",
        start: str | None = None,
        stop: str | None = None,
        udp_checksum: bool = True,
    ) -> Self:
        """Add a workload that sends at a constant rate in on phases and nothing in off phases.

        It starts with an on phase.

        Args:
            id: The name of the workload; the metrics are named after it.
            source: The node that sends.
            destination: The node that receives.
            rate: The rate of the payload while on, for example ``"400kbit"``.
            packet_size: The payload of one packet, for example ``"200b"``.
            on: How long an on phase lasts, or its mean with ``distribution="exponential"``.
            off: How long an off phase lasts, or its mean.
            distribution: ``"fixed"`` (default): the phases last exactly ``on`` and ``off``.
                ``"exponential"``: their lengths are drawn from the seed of the scenario.
            start: When the workload starts (default ``"0s"``).
            stop: When it stops (default: the end of the measured phase).
            udp_checksum: See :meth:`udp_cbr`.
        """
        phases: dict[str, Any] = {
            "rate": rate,
            "packet-size": packet_size,
            "on": on,
            "off": off,
        }
        if distribution != "fixed":
            phases["distribution"] = distribution
        return self._udp_workload(
            "udp-onoff", id, source, destination, phases, start, stop, udp_checksum
        )

    def udp_bursty(
        self,
        id: str,
        source: str,
        destination: str,
        *,
        packet_size: str,
        burst_packets: int,
        gap: str,
        pause: str,
        distribution: str = "fixed",
        start: str | None = None,
        stop: str | None = None,
        udp_checksum: bool = True,
    ) -> Self:
        """Add a workload that sends bursts of packets with a pause between them.

        The first burst starts when the workload does.

        Args:
            id: The name of the workload; the metrics are named after it.
            source: The node that sends.
            destination: The node that receives.
            packet_size: The payload of one packet, for example ``"1200b"``.
            burst_packets: The packets in a burst.
            gap: The time between two packets of a burst, for example ``"200us"`` (at least 10 us).
            pause: The time from the last packet of a burst to the first of the next, or its mean
                with ``distribution="exponential"``.
            distribution: ``"fixed"`` (default) or ``"exponential"`` (the pauses are drawn from the
                seed of the scenario).
            start: When the workload starts (default ``"0s"``).
            stop: When it stops (default: the end of the measured phase).
            udp_checksum: See :meth:`udp_cbr`.
        """
        keys: dict[str, Any] = {
            "packet-size": packet_size,
            "burst-packets": burst_packets,
            "gap": gap,
            "pause": pause,
        }
        if distribution != "fixed":
            keys["distribution"] = distribution
        return self._udp_workload(
            "udp-bursty", id, source, destination, keys, start, stop, udp_checksum
        )

    def _udp_workload(
        self,
        kind: str,
        id: str,
        source: str,
        destination: str,
        keys: dict[str, Any],
        start: str | None,
        stop: str | None,
        udp_checksum: bool,
    ) -> Self:
        workload: dict[str, Any] = {
            "id": id,
            "kind": kind,
            "from": source,
            "to": destination,
            **keys,
        }
        if start is not None:
            workload["start"] = start
        if stop is not None:
            workload["stop"] = stop
        if not udp_checksum:
            workload["udp-checksum"] = False
        self._workloads.append(workload)
        return self

    def measure(
        self,
        *,
        bin: str | None = None,
        recovery_basis: Literal["send", "arrival"] | None = None,
        recovery_baseline_bins: int | None = None,
        recovery_window_bins: int | None = None,
        recovery_fraction: str | None = None,
        capture: Sequence[str] = (),
        packets: bool = False,
    ) -> Self:
        """Say what is measured besides the statistics: the section ``measure`` of the file.

        Args:
            bin: The width of a bin of the series, for example ``"500ms"`` (default ``"1s"``).
            recovery_basis: The view the time to recover is made from (default ``"send"``).
            recovery_baseline_bins: Bins before the disturbance that make the baseline (default 5).
            recovery_window_bins: Bins in a row that have to be back at the level (default 3).
            recovery_fraction: The share of the baseline to reach again, for example ``"90%"``.
            capture: The ids of the links whose interfaces are captured as pcap files, at both ends.
            packets: Write one line of JSON for each packet of each workload.
        """
        measure: dict[str, Any] = {}
        if bin is not None:
            measure["series"] = {"bin": bin}
        recovery = {
            "basis": recovery_basis,
            "baseline-bins": recovery_baseline_bins,
            "window-bins": recovery_window_bins,
            "fraction": recovery_fraction,
        }
        recovery = {key: value for key, value in recovery.items() if value is not None}
        if recovery:
            measure["recovery"] = recovery
        if capture:
            measure["capture"] = list(capture)
        if packets:
            measure["packets"] = True
        if measure:
            self._data["measure"] = measure
        else:
            self._data.pop("measure", None)
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
