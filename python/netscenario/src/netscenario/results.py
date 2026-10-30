"""The result of a run, as typed objects in SI units.

Times are in seconds, rates in bits per second, sizes in bytes, ratios from 0 to 1. The numbers are
those of ``result.json`` (``docs/RESULTS.md``); the original text is in :attr:`Result.raw`.

The percentiles are the *nearest rank*: for ``n`` delays in ascending order the percentile ``p`` is
the delay at rank ``ceil(p * n / 100)``. It is always a delay that was measured.
"""

from __future__ import annotations

import json
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any

_NANOSECONDS = 1e9


def _seconds(nanoseconds: int) -> float:
    return nanoseconds / _NANOSECONDS


@dataclass(frozen=True)
class Delay:
    """The one-way delay of the packets that arrived (the first arrival of each), in seconds."""

    min: float
    """The smallest delay."""
    mean: float
    """The mean."""
    p50: float
    """The median."""
    p90: float
    """Percentile 90."""
    p95: float
    """Percentile 95."""
    p99: float
    """Percentile 99."""
    p999: float
    """Percentile 99.9."""
    max: float
    """The largest delay."""

    @property
    def median(self) -> float:
        """The median, the same as :attr:`p50`."""
        return self.p50


@dataclass(frozen=True)
class Throughput:
    """The payload that arrived in each second, counted from the first packet that was sent."""

    bin: float
    """The width of a bin in seconds (one second)."""
    bits_per_second: tuple[float, ...]
    """Payload bits per second in each bin. A bin is not scaled: the last one is usually only partly
    filled."""


@dataclass(frozen=True)
class Sender:
    """What the sending side did."""

    max_lateness: float
    """The most that a packet left after its moment, in seconds.

    A value that is large against the gap between packets means that the machine, not the network,
    shaped the stream.
    """
    send_errors: int
    """How many packets the kernel refused to take."""
    first_error: str | None
    """The first such error."""


@dataclass(frozen=True)
class Workload:
    """The result of one workload."""

    id: str
    """The name in the scenario."""
    kind: str
    """The kind, ``"udp-cbr"``."""
    from_node: str
    """The sending node."""
    to_node: str
    """The receiving node."""
    from_address: str
    """The address the packets were sent from."""
    to_address: str
    """The address and the port the packets went to."""
    rate: float
    """The rate of the payload in the scenario, in bits per second."""
    packet_size: int
    """The payload of a packet in bytes."""
    start: float
    """When the workload started, from the start of the measured phase, in seconds."""
    duration: float
    """How long it sent, in seconds."""
    sent: int
    """The packets the sender handed to the kernel."""
    received: int
    """The different packets that arrived."""
    lost: int
    """``sent - received``."""
    loss_ratio: float
    """``lost / sent``, a number between 0 and 1 (0 if nothing was sent)."""
    duplicates: int
    """Arrivals of a packet that had arrived before."""
    reordered: int
    """Packets that arrived after a packet with a higher sequence number (RFC 4737)."""
    unexpected: int
    """Records with a sequence number the sender never used; always 0 unless there is a bug."""
    delay: Delay | None
    """The one-way delay, or ``None`` if no packet arrived."""
    throughput: Throughput
    """The payload per second."""
    sender: Sender
    """The sending side."""
    foreign_datagrams: int
    """Datagrams that reached the receiver and were not packets of the agent."""
    records_file: Path
    """The binary file of per-packet records (``docs/AGENT.md``)."""
    packets_file: Path | None
    """The JSON Lines file with a line for each packet, if it was asked for."""

    @property
    def delivery_ratio(self) -> float:
        """``received / sent``, a number between 0 and 1 (1 if nothing was sent)."""
        return 1.0 if self.sent == 0 else self.received / self.sent


@dataclass(frozen=True)
class SeedInfo:
    """The seed of the run."""

    value: int | None
    """The seed from which the seeds of all random parts were derived, or ``None`` if the backend
    cannot apply one."""
    drawn: bool
    """Whether the run drew the seed because the scenario has none. Pass ``value`` as ``seed`` to
    repeat it."""
    loss_reproducible: bool
    """Whether the same seed gives the same lost packets in this run."""


@dataclass(frozen=True)
class Provenance:
    """Where, when and with what the result was made."""

    netscenario: str
    """The version of netscenario."""
    started_at: str
    """When the run started, UTC (``2026-10-30T12:00:00Z``)."""
    backend: str
    """The backend that ran it."""
    backend_details: Mapping[str, str]
    """What the backend says about itself (the version of ``tc``, how the rights were obtained)."""
    kernel: str | None
    """The release of the kernel."""
    cpu_model: str | None
    """The model of the processor."""
    cpus: int | None
    """How many processors the run could use."""


@dataclass(frozen=True)
class Restriction:
    """A limit of the backend that the scenario accepted."""

    feature: str
    """The id of the feature."""
    restriction: str
    """What is different from the ideal."""


@dataclass(frozen=True)
class Address:
    """The address of a node on a link."""

    node: str
    link: str
    address: str


@dataclass(frozen=True)
class Result:
    """What a run found out. ``result["media"]`` is the :class:`Workload` called ``media``."""

    scenario_name: str
    """The name of the scenario."""
    scenario_hash: str
    """``sha256:`` and the fingerprint of the scenario (comments and layout do not change it)."""
    provenance: Provenance
    """Where, when and with what."""
    seed: SeedInfo
    """The seed, and whether the run can be repeated."""
    accepted_restrictions: tuple[Restriction, ...]
    """The limits of the backend that the scenario accepted."""
    addresses: tuple[Address, ...]
    """The address of each end of each link."""
    duration: float
    """The length of the measured phase, in seconds."""
    settle: float
    """How long the run waited for the packets still in the network, in seconds."""
    workloads: Mapping[str, Workload]
    """The workloads by id, in the order of the scenario."""
    out_dir: Path
    """The directory with ``result.json`` and the files that belong to it."""
    raw: Mapping[str, Any]
    """The decoded ``result.json``, unchanged."""

    def __getitem__(self, workload_id: str) -> Workload:
        """The workload with this id."""
        try:
            return self.workloads[workload_id]
        except KeyError:
            known = ", ".join(self.workloads)
            message = f"the run has no workload '{workload_id}'; the workloads are: {known}"
            raise KeyError(message) from None

    def __iter__(self) -> Iterator[Workload]:
        """The workloads, in the order of the scenario."""
        return iter(self.workloads.values())

    def __len__(self) -> int:
        """How many workloads."""
        return len(self.workloads)

    @classmethod
    def from_json(cls, text: str, out_dir: str | Path) -> Result:
        """Read the text of a ``result.json``.

        Raises:
            ValueError: The text is not a result of the version this package reads.
        """
        raw: dict[str, Any] = json.loads(text)
        if raw.get("format") != "netscenario-result":
            message = f"this is not a netscenario result (format {raw.get('format')!r})"
            raise ValueError(message)
        if raw.get("version") != 0:
            message = f"this result has version {raw.get('version')}; this package reads version 0"
            raise ValueError(message)
        directory = Path(out_dir)
        workloads = {entry["id"]: _workload(entry, directory) for entry in raw["workloads"]}
        provenance = raw["provenance"]
        seed = raw["seed"]
        return cls(
            scenario_name=raw["scenario"]["name"],
            scenario_hash=raw["scenario"]["hash"],
            provenance=Provenance(
                netscenario=provenance["netscenario"],
                started_at=provenance["started_at"],
                backend=provenance["backend"],
                backend_details=MappingProxyType(dict(provenance["backend_details"])),
                kernel=provenance["kernel"],
                cpu_model=provenance["cpu_model"],
                cpus=provenance["cpus"],
            ),
            seed=SeedInfo(seed["value"], seed["drawn"], seed["loss_reproducible"]),
            accepted_restrictions=tuple(
                Restriction(r["feature"], r["restriction"]) for r in raw["accepted_restrictions"]
            ),
            addresses=tuple(Address(a["node"], a["link"], a["address"]) for a in raw["addresses"]),
            duration=_seconds(raw["timing"]["duration_ns"]),
            settle=_seconds(raw["timing"]["settle_ns"]),
            workloads=MappingProxyType(workloads),
            out_dir=directory,
            raw=MappingProxyType(raw),
        )


def _workload(entry: dict[str, Any], directory: Path) -> Workload:
    delay = entry["delay"]
    throughput = entry["throughput"]
    sender = entry["sender"]
    return Workload(
        id=entry["id"],
        kind=entry["kind"],
        from_node=entry["from"],
        to_node=entry["to"],
        from_address=entry["from_address"],
        to_address=entry["to_address"],
        rate=float(entry["rate_bits_per_second"]),
        packet_size=int(entry["packet_size_bytes"]),
        start=_seconds(entry["start_ns"]),
        duration=_seconds(entry["duration_ns"]),
        sent=int(entry["sent"]),
        received=int(entry["received"]),
        lost=int(entry["lost"]),
        loss_ratio=float(entry["loss_ratio"]),
        duplicates=int(entry["duplicates"]),
        reordered=int(entry["reordered"]),
        unexpected=int(entry["unexpected"]),
        delay=None
        if delay is None
        else Delay(
            min=_seconds(delay["min_ns"]),
            mean=_seconds(delay["mean_ns"]),
            p50=_seconds(delay["p50_ns"]),
            p90=_seconds(delay["p90_ns"]),
            p95=_seconds(delay["p95_ns"]),
            p99=_seconds(delay["p99_ns"]),
            p999=_seconds(delay["p999_ns"]),
            max=_seconds(delay["max_ns"]),
        ),
        throughput=Throughput(
            bin=_seconds(throughput["bin_ns"]),
            bits_per_second=tuple(float(b) for b in throughput["bits_per_second"]),
        ),
        sender=Sender(
            max_lateness=_seconds(sender["max_lateness_ns"]),
            send_errors=int(sender["send_errors"]),
            first_error=sender["first_error"],
        ),
        foreign_datagrams=int(entry["foreign_datagrams"]),
        records_file=directory / entry["records_file"],
        packets_file=None if entry["packets_file"] is None else directory / entry["packets_file"],
    )
