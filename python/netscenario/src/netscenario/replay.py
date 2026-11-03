"""Run a recorded run again and compare it with the original (ENV-03).

``replay(directory)`` is ``netscenario replay`` as a function. It runs the scenario that the run
recorded (``scenario.json``) with the seed the run had, waits for the packets in the network as long
as the original did, and says whether the same packets were lost. A replay that does **not**
reproduce the original is a normal answer and not an exception: ``Replay.verdict`` says which
(``docs/CLI.md``, D-075).

Example:
    >>> result = ns.run("lte-like.yaml", seed=42)  # doctest: +SKIP
    >>> again = ns.replay(result.out_dir)  # doctest: +SKIP
    >>> assert again.reproduced, again.verdict  # doctest: +SKIP
"""

from __future__ import annotations

import json
import os
import tempfile
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Literal

from netscenario import _binaries, _native
from netscenario.errors import BackendError
from netscenario.events import RunEvent, make_event
from netscenario.results import Result

Verdict = Literal["reproduced", "differs", "not-promised", "not-comparable"]
"""How a replay came out: ``reproduced`` (the same packets were lost), ``differs`` (other packets
although both runs said that their loss repeats from the seed: the replay failed), ``not-promised``
(other packets, but the loss of a run does not repeat from its seed, which ``reasons`` explains) or
``not-comparable`` (the original was interrupted, or its records cannot be read)."""


@dataclass(frozen=True)
class Difference:
    """A field of the provenance that differs between the original and the replay."""

    path: str
    """Which field: ``kernel``, ``backend_details.tc``, ``cpu_model``, ..."""
    recorded: str
    """The value in the original."""
    other: str
    """The value in the replay."""


@dataclass(frozen=True)
class LostPackets:
    """The lost packets of one workload in the original and in the replay."""

    id: str
    """The workload."""
    lost_original: int
    """How many packets the original lost (never arrived intact)."""
    lost_replay: int
    """How many the replay lost."""
    only_original: tuple[int, ...]
    """The first sequence numbers lost in the original and not in the replay (at most 20)."""
    only_replay: tuple[int, ...]
    """The first sequence numbers lost in the replay and not in the original (at most 20)."""
    count_only_original: int
    """How many sequence numbers were lost only in the original."""
    count_only_replay: int
    """How many were lost only in the replay."""

    @property
    def differing(self) -> int:
        """How many sequence numbers were lost in one run and not in the other."""
        return self.count_only_original + self.count_only_replay

    @property
    def identical(self) -> bool:
        """Whether the same packets were lost."""
        return self.differing == 0


@dataclass(frozen=True)
class Replay:
    """A replay set against its original."""

    verdict: Verdict
    """How the replay came out."""
    original_dir: Path
    """The directory of the run that was replayed."""
    replay_dir: Path
    """The directory of the replay, a run of its own."""
    seed: int | None
    """The seed that both runs used (``None`` if the backend could not apply one)."""
    workloads: tuple[LostPackets, ...]
    """The lost packets of each workload; empty if the runs could not be compared."""
    provenance: tuple[Difference, ...]
    """The fields of the provenance that differ (the time of the run is not one of them)."""
    other_machine: bool
    """Whether the machine is not the one of the original: the backend, its details (the version of
    ``tc``), the kernel or the processor differ. The lost packets of a seed depend on them."""
    reasons: tuple[str, ...]
    """Why the loss of a run does not repeat from its seed, or why the runs cannot be compared."""
    raw: Mapping[str, Any]
    """The decoded JSON of the comparison, unchanged."""

    @property
    def reproduced(self) -> bool:
        """Whether the replay lost the same packets as the original."""
        return self.verdict == "reproduced"

    def result(self) -> Result:
        """The :class:`netscenario.Result` of the replay, read from its directory."""
        text = (self.replay_dir / "result.json").read_text(encoding="utf-8")
        return Result.from_json(text, self.replay_dir)

    @classmethod
    def from_json(cls, text: str) -> Replay:
        """Read the JSON that the native library makes of a replay.

        Raises:
            ValueError: The text is not the comparison of a replay.
        """
        raw: dict[str, Any] = json.loads(text)
        if raw.get("format") != "netscenario-replay" or raw.get("version") != 0:
            message = "this is not a netscenario replay of a version this package reads"
            raise ValueError(message)
        comparison = raw["comparison"]
        return cls(
            verdict=comparison["verdict"],
            original_dir=Path(raw["original"]),
            replay_dir=Path(raw["replay"]),
            seed=raw["seed"],
            workloads=tuple(
                LostPackets(
                    id=w["id"],
                    lost_original=w["lost_original"],
                    lost_replay=w["lost_replay"],
                    only_original=tuple(w["only_original"]),
                    only_replay=tuple(w["only_replay"]),
                    count_only_original=w["count_only_original"],
                    count_only_replay=w["count_only_replay"],
                )
                for w in comparison["workloads"]
            ),
            provenance=tuple(
                Difference(path=d["path"], recorded=d["recorded"], other=d["other"])
                for d in comparison["provenance"]
            ),
            other_machine=comparison["other_machine"],
            reasons=tuple(comparison["reasons"]),
            raw=MappingProxyType(raw),
        )


def replay(
    directory: str | os.PathLike[str],
    *,
    out: str | os.PathLike[str] | None = None,
    observer: Callable[[RunEvent], None] | None = None,
    cancel: _native.CancelToken | None = None,
    working_dir: str | os.PathLike[str] | None = None,
) -> Replay:
    """Run a recorded run again with its recorded seed and compare it with the original.

    The scenario is the one the run recorded (``scenario.json`` in its directory), never a file
    that may have changed since. The replay waits for the packets in the network as long as the
    original did, and the programs of your own run in ``working_dir`` (the original's directory is
    not recorded). The call blocks, gives up the GIL and can be interrupted like
    :func:`netscenario.run`.

    Args:
        directory: The directory of a run (``Result.out_dir``; for a run of a set, ``run-001`` and
            so on).
        out: The directory for the replay, which must not be the one that is replayed. Default: a
            new directory in the temporary directory of the system.
        observer: Called with each :class:`netscenario.events.RunEvent` of the replay.
        cancel: A :class:`netscenario.CancelToken` that stops the replay.
        working_dir: The directory the programs of your own run in. Default: the current directory.

    Returns:
        The :class:`Replay`. A replay that lost other packets is returned, with the verdict
        ``"differs"``; check :attr:`Replay.reproduced`.

    Raises:
        ValueError: The directory cannot be replayed (it is not a run, it is a set, it has no
            ``scenario.json`` because an older version made it, or the scenario is not the one of
            the result), or ``out`` is the directory that is replayed.
        CapabilityError: The backend cannot reproduce what the scenario asks for.
        BackendError: The machine does not allow the run (see :func:`netscenario.doctor`).
        RunError: The replay failed after it started.
        Cancelled: ``cancel`` was cancelled.
        KeyboardInterrupt: The replay was interrupted.
    """
    try:
        supervisor, agent = _binaries.find()
    except _binaries.ProgramsNotFoundError as error:
        raise BackendError(str(error)) from None
    original = Path(directory)
    target = Path(out) if out is not None else Path(tempfile.mkdtemp(prefix="netscenario-replay-"))
    if target.exists() and original.exists() and target.resolve() == original.resolve():
        message = "out is the directory of the run that is replayed; the replay would overwrite it"
        raise ValueError(message)

    def notify(kind: str, fields: dict[str, float]) -> None:
        event = make_event(kind, fields)
        if event is not None and observer is not None:
            observer(event)

    text = _native.replay_run(
        str(original),
        str(supervisor),
        str(agent),
        str(target),
        observer=notify if observer is not None else None,
        cancel=cancel,
        working_dir=str(Path(working_dir).resolve() if working_dir is not None else Path.cwd()),
    )
    return Replay.from_json(text)
