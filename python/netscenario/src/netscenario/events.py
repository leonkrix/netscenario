"""What a run reports about itself while it goes (see ``observer`` of :func:`netscenario.run`).

The events come in this order: :class:`Provisioned`, :class:`ReceiversReady`, :class:`Started`,
:class:`Progress` (once a second, and a last time at the full length), :class:`Settling`,
:class:`Collecting`, :class:`TearingDown` and :class:`Finished`; a :class:`StepDone` for each
step of the timeline comes between :class:`Started` and :class:`Settling`. :class:`TearingDown`
also comes after an error or a cancel; :class:`Finished` only after a run that succeeded. All
times are in seconds.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Provisioned:
    """The network is built: the nodes and links exist and the impairments are applied."""

    nodes: int
    """How many nodes."""
    links: int
    """How many links."""


@dataclass(frozen=True)
class ReceiversReady:
    """Every receiver listens. The senders start next."""

    count: int
    """How many receivers."""


@dataclass(frozen=True)
class Started:
    """The senders are started and the workloads begin shortly."""

    duration: float
    """How long the workloads run, from the start of the first to the end of the last (seconds)."""


@dataclass(frozen=True)
class Progress:
    """The workloads are running: reported once a second, and last with ``elapsed == total``."""

    elapsed: float
    """How long they have been running, in seconds."""
    total: float
    """How long they run, in seconds."""


@dataclass(frozen=True)
class Settling:
    """The senders are done; the run waits for the packets that are still in the network."""

    wait: float
    """How long it waits, in seconds."""


@dataclass(frozen=True)
class Collecting:
    """The receivers are stopped and their records are read."""


@dataclass(frozen=True)
class TearingDown:
    """The network is torn down. Reported on every path."""


@dataclass(frozen=True)
class Finished:
    """The run is done and its result is written."""


@dataclass(frozen=True)
class StepDone:
    """A step of the timeline was done: an impairment of a link changed."""

    event: int
    """The place of the event in ``timeline`` of the scenario."""
    lateness: float
    """How late the step was, in seconds: from the moment the file gives to its end."""


RunEvent = (
    Provisioned
    | ReceiversReady
    | Started
    | Progress
    | Settling
    | Collecting
    | TearingDown
    | Finished
    | StepDone
)
"""Any of the events of a run."""


def make_event(kind: str, fields: dict[str, float]) -> RunEvent | None:
    """Make the event that the native module reports (not part of the API)."""
    match kind:
        case "provisioned":
            return Provisioned(nodes=int(fields["nodes"]), links=int(fields["links"]))
        case "receivers_ready":
            return ReceiversReady(count=int(fields["count"]))
        case "started":
            return Started(duration=fields["duration"])
        case "progress":
            return Progress(elapsed=fields["elapsed"], total=fields["total"])
        case "settling":
            return Settling(wait=fields["wait"])
        case "collecting":
            return Collecting()
        case "tearing_down":
            return TearingDown()
        case "finished":
            return Finished()
        case "step_done":
            return StepDone(event=int(fields["event"]), lateness=fields["lateness"])
        case _:
            # An event of a newer core than this package knows: it is not passed on
            return None
