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
class Extent:
    """How far the reordered packets were out of place, in packets (RFC 4737, section 4.2).

    The extent of a packet is the number of arrivals between it and the earliest-arrived packet
    with a higher sequence number: a packet that arrives right after the one that overtook it has
    extent 1.
    """

    max: int
    """The largest extent: a buffer that restores the order needs about this much room."""
    mean: float
    """The mean extent."""
    p50: int
    """The median (nearest rank)."""
    p95: int
    """Percentile 95."""
    p99: int
    """Percentile 99."""


@dataclass(frozen=True)
class LateTime:
    """How long the reordered packets arrived after the packet that overtook them (RFC 4737, 4.3).

    In seconds: how long a receiver would have waited to restore the order.
    """

    max: float
    """The largest late time."""
    mean: float
    """The mean."""
    p50: float
    """The median (nearest rank)."""
    p95: float
    """Percentile 95."""
    p99: float
    """Percentile 99."""


@dataclass(frozen=True)
class Reordering:
    """How the reordered packets were out of place. Absent (``None``) where none was reordered."""

    extent: Extent
    """In packets."""
    late_time: LateTime
    """In seconds."""


@dataclass(frozen=True)
class InterarrivalJitter:
    """The interarrival jitter of RFC 3550 (appendix A.8), in seconds.

    For two packets that arrive one after the other, ``D = (R_j - R_i) - (S_j - S_i)`` is the
    difference of their transit times, and the estimate moves a sixteenth of the way towards
    ``|D|`` with every packet. The packets are paired in the order of arrival, so a reordered
    packet makes a large ``D``.
    """

    last: float
    """The estimate after the last packet (what an RTCP report at the end would carry)."""
    mean: float
    """The mean of the estimate over the run: one number that does not depend on the last sixteen
    packets."""
    max: float
    """The largest value the estimate had."""


@dataclass(frozen=True)
class LossRun:
    """How many runs of lost packets had one length."""

    length: int
    """The length of the run, in packets."""
    runs: int
    """How many runs had it."""


@dataclass(frozen=True)
class LossRuns:
    """The runs of lost packets (the "loss period" of RFC 3357).

    A run is a stretch of consecutive sequence numbers none of which arrived.

    Independent loss gives mostly runs of length 1; a link with bursts gives longer ones.
    """

    runs: int
    """How many runs there were."""
    longest: int
    """The length of the longest run (0 if no packet was lost)."""
    mean: float
    """The mean length, ``lost / runs`` (0 if no packet was lost)."""
    histogram: tuple[LossRun, ...]
    """For each length that occurred, how many runs had it, by ascending length."""


@dataclass(frozen=True)
class TimelineStep:
    """One step of the timeline as the run did it, in seconds from the start of the workloads.

    An event on ``both`` is two steps, one for each direction.
    """

    event: int
    """The place of the event in ``timeline`` of the scenario."""
    kind: str
    """``set``, ``outage-start``, ``outage-end``, ``down`` or ``up``."""
    link: str
    """The link."""
    direction: str | None
    """``forward`` or ``reverse``, or ``None`` for a step of the whole link."""
    scheduled: float
    """When the file says it happens."""
    started: float
    """When the run asked the backend to do it."""
    applied: float
    """When the backend had done it."""
    packet: int | None = None
    """For a step done at a barrier: the number of packets that met the old state, which is the
    number of the first packet that met the new one. ``None`` for a step done by the clock."""
    held: float | None = None
    """For a step done at a barrier: how long the sender waited for it at the slot of that packet,
    in seconds (0 if it was done by then). ``None`` for a step done by the clock."""

    @property
    def lateness(self) -> float:
        """How late the step was: the clock, the change itself and what the machine added.

        Never negative: a step is not done before its moment.
        """
        return max(0.0, self.applied - self.scheduled)

    @property
    def at_barrier(self) -> bool:
        """Whether a sender waited for the step, so that the same seed keeps the lost packets."""
        return self.packet is not None


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
    reordering: Reordering | None
    """How far the reordered packets were out of place, or ``None`` if none was."""
    jitter: InterarrivalJitter | None
    """The interarrival jitter (RFC 3550), or ``None`` for fewer than two arrivals."""
    loss_runs: LossRuns
    """The runs of lost packets."""
    records_file: Path
    """The binary file of per-packet records (``docs/AGENT.md``)."""
    packets_file: Path | None
    """The JSON Lines file with a line for each packet, if it was asked for."""
    corrupted_named: int | None = None
    """How many damaged packets the receiving node saw on the wire and did not deliver intact, with
    their sequence numbers readable; ``None`` if the direction does not corrupt or they could not be
    named (see ``corrupted_seq_unavailable``). A count of numbers read from frames: a frame with a
    damaged header is not named, and a bit flipped in a number can name a neighbour. The exact count
    for the node is ``dropped_corrupted`` of ``directions``."""
    corrupted_seq: tuple[int, ...] = ()
    """The first 1,000 of those sequence numbers, ascending. ``result.packets_file`` (``--packets``)
    has all of them."""
    corrupted_seq_unavailable: str | None = None
    """Why the damaged packets of a corrupting direction could not be named, if they could not."""
    corrupted: int = 0
    """Datagrams that reached the receiver damaged, in a workload with ``udp_checksum=False``:
    the CRC-32 of the packet did not match. They are not in ``received``, and they are in
    ``lost``: ``lost - corrupted`` is what never arrived. Always 0 with the checksum on, where the
    kernel drops a damaged packet and the ``directions`` entry of the node counts it
    (``dropped_corrupted``)."""

    @property
    def delivery_ratio(self) -> float:
        """``received / sent``, a number between 0 and 1 (1 if nothing was sent)."""
        return 1.0 if self.sent == 0 else self.received / self.sent


@dataclass(frozen=True)
class QueueCheck:
    """The packets of one direction of a link that drew a number for the random loss.

    Counted by the queue, and sent by the workloads.
    """

    link: str
    """The link."""
    direction: str
    """``"forward"`` (from the first node of ``between`` to the second) or ``"reverse"``."""
    expected: int
    """The packets the workloads sent into the direction (those the kernel took)."""
    counted: int
    """The packets that entered the queue of the direction."""

    @property
    def agrees(self) -> bool:
        """Whether the queue saw exactly the packets of the workloads."""
        return self.expected == self.counted


@dataclass(frozen=True)
class DirectionTally:
    """Where the packets of one direction of a link went.

    Lost in the queue of the link, dropped by the receiving node because they arrived damaged, or
    delivered. The three add up to what was sent, so a corrupted packet is not mistaken for a lost
    one. The counts are those of the interface and of the node, so they hold for all the workloads
    in the direction together.
    """

    link: str
    """The link."""
    direction: str
    """``"forward"`` (from the first node of ``between`` to the second) or ``"reverse"``."""
    sent: int
    """The packets the workloads put into the direction (those the kernel took)."""
    reached_node: int
    """The frames that arrived at the interface of the receiving node."""
    delivered: int
    """The datagrams that reached the sockets of the receivers."""
    lost_in_queue: int | None
    """The packets the queue of the link dropped (``sent`` minus ``reached_node``). ``None`` in a
    direction that duplicates, where the frames include the copies."""
    dropped_corrupted: int
    """The frames that arrived and that the node dropped because a bit in them was flipped."""
    receive_buffer_drops: int
    """The datagrams the node dropped because the receive buffer of a socket was full. A fault of
    the receiver, not of the network."""


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
    """Whether the same seed gives the same lost packets in this run.

    ``False`` if the backend cannot apply a seed, or if a queue counted packets that the workloads
    did not send.
    """
    queue_checks: tuple[QueueCheck, ...] = ()
    """For each direction with random loss, the packets its queue counted against the packets the
    workloads sent. The pattern of lost packets depends on this number as well as on the seed. Empty
    if the backend cannot count, or the run has no seed."""
    not_reproducible_because: tuple[str, ...] = ()
    """Why the run is not reproducible, one sentence for each reason. Empty when
    ``loss_reproducible`` is ``True``."""


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
    directions: tuple[DirectionTally, ...] = ()
    """For each direction of a link that carried a workload: where its packets went. Empty if the
    backend cannot count, or the scenario runs a program of your own."""
    steps: tuple[TimelineStep, ...] = ()
    """What the run did to the links while it ran, in the order it was done (the ``events`` of
    ``result.json``). Empty for a scenario without a timeline."""
    notes: tuple[str, ...] = ()
    """What the backend says about how it did the run besides the result, one sentence for each
    fact: for example that the changes of a link were made the slow way after the fast way stopped
    working. Empty if the run went as usual."""

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
            seed=SeedInfo(
                seed["value"],
                seed["drawn"],
                seed["loss_reproducible"],
                tuple(
                    QueueCheck(c["link"], c["direction"], c["expected"], c["counted"])
                    for c in seed.get("queue_checks", ())
                ),
                tuple(seed.get("not_reproducible_because", ())),
            ),
            accepted_restrictions=tuple(
                Restriction(r["feature"], r["restriction"]) for r in raw["accepted_restrictions"]
            ),
            addresses=tuple(Address(a["node"], a["link"], a["address"]) for a in raw["addresses"]),
            duration=_seconds(raw["timing"]["duration_ns"]),
            settle=_seconds(raw["timing"]["settle_ns"]),
            workloads=MappingProxyType(workloads),
            directions=tuple(
                DirectionTally(
                    link=d["link"],
                    direction=d["direction"],
                    sent=int(d["sent"]),
                    reached_node=int(d["reached_node"]),
                    delivered=int(d["delivered"]),
                    lost_in_queue=None if d["lost_in_queue"] is None else int(d["lost_in_queue"]),
                    dropped_corrupted=int(d["dropped_corrupted"]),
                    receive_buffer_drops=int(d["receive_buffer_drops"]),
                )
                for d in raw.get("directions", ())
            ),
            steps=tuple(
                TimelineStep(
                    event=int(e["event"]),
                    kind=e["kind"],
                    link=e["link"],
                    direction=e["direction"],
                    scheduled=_seconds(e["scheduled_ns"]),
                    started=_seconds(e["started_ns"]),
                    applied=_seconds(e["applied_ns"]),
                    packet=e.get("packet"),
                    held=None if e.get("held_ns") is None else _seconds(e["held_ns"]),
                )
                for e in raw.get("events", ())
            ),
            notes=tuple(raw.get("notes", ())),
            out_dir=directory,
            raw=MappingProxyType(raw),
        )


def _reordering(data: dict[str, Any] | None) -> Reordering | None:
    if data is None:
        return None
    extent, late = data["extent"], data["late_time"]
    return Reordering(
        extent=Extent(
            max=int(extent["max"]),
            mean=float(extent["mean"]),
            p50=int(extent["p50"]),
            p95=int(extent["p95"]),
            p99=int(extent["p99"]),
        ),
        late_time=LateTime(
            max=_seconds(late["max_ns"]),
            mean=_seconds(late["mean_ns"]),
            p50=_seconds(late["p50_ns"]),
            p95=_seconds(late["p95_ns"]),
            p99=_seconds(late["p99_ns"]),
        ),
    )


def _jitter(data: dict[str, Any] | None) -> InterarrivalJitter | None:
    if data is None:
        return None
    return InterarrivalJitter(
        last=_seconds(data["last_ns"]),
        mean=_seconds(data["mean_ns"]),
        max=_seconds(data["max_ns"]),
    )


def _loss_runs(data: dict[str, Any] | None) -> LossRuns:
    # A result from before the runs were counted has none: "no run", as for a run without loss
    if data is None:
        return LossRuns(runs=0, longest=0, mean=0.0, histogram=())
    return LossRuns(
        runs=int(data["runs"]),
        longest=int(data["longest"]),
        mean=float(data["mean"]),
        histogram=tuple(
            LossRun(length=int(b["length"]), runs=int(b["runs"])) for b in data["histogram"]
        ),
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
        reordering=_reordering(entry.get("reordering")),
        jitter=_jitter(entry.get("jitter")),
        loss_runs=_loss_runs(entry.get("loss_runs")),
        records_file=directory / entry["records_file"],
        packets_file=None if entry["packets_file"] is None else directory / entry["packets_file"],
        corrupted=int(entry.get("corrupted", 0)),
        corrupted_named=None
        if entry.get("corrupted_named") is None
        else int(entry["corrupted_named"]),
        corrupted_seq=tuple(int(n) for n in entry.get("corrupted_seq", ())),
        corrupted_seq_unavailable=entry.get("corrupted_seq_unavailable"),
    )
