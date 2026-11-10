"""A run that can be talked to while it goes.

Change a link from test code, iterate the events, use it from ``asyncio``.

:func:`start` starts a run and returns a :class:`Run` at once; the run goes on a thread of its
own, as :func:`netscenario.run` does. With ``live`` the code can change the links it names while
the run goes::

    with netscenario.start(scenario, live=["access"]) as run:
        run.wait_until(1.5)  # seconds into the measured phase
        run.link("access").set(loss="2%")
    result = run.result()

A change is a step of the timeline like the steps of the file. The run holds the sender that sends
into the direction before its next packet and does the step between two packets, so the result
knows exactly which packet met the old state (``TimelineStep.packet``). The recorded scenario holds
the change as an event at the slot of that packet, so :func:`netscenario.replay` does it at the
same packet and loses the same packets. Two live runs differ, because the moment is the code's;
the result says so (``Result.reproducibility`` and ``seed.not_reproducible_because``).

:func:`start_async` and :func:`run_async` are the same for ``asyncio``: nothing blocks the loop, a
cancelled task cancels the run and returns when the network is torn down, and no thread is left
behind.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import queue
import tempfile
import threading
import time
from collections.abc import AsyncIterator, Awaitable, Callable, Iterable, Iterator, Mapping
from pathlib import Path
from types import TracebackType
from typing import Any, Final, Protocol, Self, TypeVar, cast

from netscenario import _binaries, _native
from netscenario.errors import BackendError, RunError
from netscenario.events import RunEvent, make_event
from netscenario.results import Result, TimelineStep, step_from_record
from netscenario.run import resolve_working_dir
from netscenario.scenario import Edit, Impairments, Jitter, Loss, ParamValue, Scenario

T = TypeVar("T")


class _Unset:
    """The type of the marker for a key that is not given."""

    def __repr__(self) -> str:
        return "UNSET"


UNSET: Final = _Unset()
"""A key of a change that is not given: the link keeps its value. ``None`` takes it away."""


def _edit_data(edit: Edit) -> dict[str, Any]:
    if isinstance(edit, Impairments):
        return edit.to_data()
    return {key: _value(value) for key, value in edit.items()}


def _value(value: object) -> object:
    if isinstance(value, (Jitter, Loss)):
        return value.to_data()
    if isinstance(value, Mapping):
        return dict(cast("Mapping[str, object]", value))
    return value


class LinkHandle:
    """A link of a run that is going, declared live, as ``run.link("access")`` returns it."""

    def __init__(
        self, send: Callable[[str, str, str | None], tuple[TimelineStep, ...]], link: str
    ) -> None:
        """Not made directly: ask the run (:meth:`Run.link`)."""
        self._send = send
        self.id: Final = link
        """The id of the link."""

    def set(
        self,
        *,
        both: Edit | None = None,
        forward: Edit | None = None,
        reverse: Edit | None = None,
        delay: str | _Unset | None = UNSET,
        jitter: Jitter | str | Mapping[str, Any] | _Unset | None = UNSET,
        loss: Loss | str | Mapping[str, Any] | _Unset | None = UNSET,
        rate: str | _Unset | None = UNSET,
        reorder: Mapping[str, Any] | _Unset | None = UNSET,
        duplicate: str | _Unset | None = UNSET,
        corrupt: str | _Unset | None = UNSET,
    ) -> tuple[TimelineStep, ...]:
        """Change the values of the link from now on, as a ``set`` of the timeline does.

        What is not given stays as it is, a key given as ``None`` takes the impairment away, and a
        value replaces the old one as a whole (a ``loss`` replaces the loss, it does not merge into
        it). The keys ``delay`` to ``corrupt`` change both directions; ``both``, ``forward`` and
        ``reverse`` take a change for a direction, as the file does.

        The call returns once the backend has done the change. A change of both directions is two
        steps, and the result is both.

        Args:
            both: A change of both directions.
            forward: A change of the direction from the first node of the link.
            reverse: A change of the way back.
            delay: The delay, ``"40ms"``.
            jitter: The jitter; a string is the amount of a uniform jitter. Needs a delay.
            loss: The loss; a string is the probability of a random loss.
            rate: The rate limit, ``"10mbit"``.
            reorder: The reordering, ``{"share": "5%"}``.
            duplicate: The duplication, ``"1%"``.
            corrupt: The corruption, ``"1%"``.

        Returns:
            The steps the change made, as they are in ``Result.steps``, with ``live`` set.

        Raises:
            ScenarioError: The change is not valid for the state the link is in (a jitter with no
                delay).
            CapabilityError: The backend cannot do it exactly (a jitter whose distribution the
                queue could not go back to, for example); nothing was changed.
            RunError: The link was not declared live, the measured phase is over, or the run ended.
            ValueError: Nothing is given, or keys are combined with ``both``.
        """
        keys = {
            "delay": delay,
            "jitter": jitter,
            "loss": loss,
            "rate": rate,
            "reorder": reorder,
            "duplicate": duplicate,
            "corrupt": corrupt,
        }
        given = {key: _value(value) for key, value in keys.items() if value is not UNSET}
        # A string is the short form of the value that is most often written
        if isinstance(jitter, str):
            given["jitter"] = Jitter(jitter).to_data()
        if isinstance(loss, str):
            given["loss"] = Loss(loss).to_data()
        if given and both is not None:
            raise ValueError("give the keys of the change or `both`, not both")
        data: dict[str, Any] = {}
        if both is not None:
            data["both"] = _edit_data(both)
        elif given:
            data["both"] = given
        if forward is not None:
            data["forward"] = _edit_data(forward)
        if reverse is not None:
            data["reverse"] = _edit_data(reverse)
        if not data:
            raise ValueError("give at least one key to change")
        return self._send(self.id, "set", json.dumps(data))

    def down(self) -> TimelineStep:
        """Take the interface of the link down: the sender gets errors until :meth:`up`."""
        return self._send(self.id, "down", None)[0]

    def up(self) -> TimelineStep:
        """Bring the interface of the link up again."""
        return self._send(self.id, "up", None)[0]


class Run:
    """A run that is going. Made by :func:`start`; use it as a context manager.

    Leaving the ``with`` block waits for the run to end (and raises what went wrong in it); leaving
    it with an exception cancels the run, waits until the network is torn down, and lets the
    exception go on.
    """

    def __init__(
        self,
        native: _native.LiveRun,
        directory: Path,
        live: tuple[str, ...],
        events: queue.SimpleQueue[RunEvent],
        on_result: Callable[[Result], None] | None = None,
    ) -> None:
        """Not made directly: call :func:`start`."""
        self._on_result = on_result
        self._lock = threading.Lock()
        self._native = native
        self._directory = directory
        self._live = live
        self._events = events
        self._result: Result | None = None
        self._error: BaseException | None = None
        self._finished = False

    @property
    def live(self) -> tuple[str, ...]:
        """The links that may be changed."""
        return self._live

    @property
    def out_dir(self) -> Path:
        """The directory of the result."""
        return self._directory

    @property
    def elapsed(self) -> float | None:
        """Seconds since the workloads started, or ``None`` before.

        The scenario's own clock: a step at ``at: 3s`` of the file is done when this reads 3.
        """
        return self._native.elapsed()

    @property
    def done(self) -> bool:
        """Whether the run has ended."""
        return self._native.is_done()

    def link(self, link: str) -> LinkHandle:
        """The handle of a link that was declared live.

        Raises:
            ValueError: The link was not named in ``live`` when the run was started.
        """
        if link not in self._live:
            raise ValueError(
                f"link '{link}' was not declared live; start the run with live=[{link!r}]"
            )
        return LinkHandle(self._change, link)

    def wait_until(self, seconds: float) -> None:
        """Wait until the measured phase is ``seconds`` old (see :attr:`elapsed`).

        Raises:
            RunError: The run ended before that moment. If it ended because something went wrong,
                that error is raised instead.
        """
        while True:
            now = self._native.elapsed()
            if now is not None and now >= seconds:
                return
            if self._native.is_done():
                self.result()
                raise RunError(f"the run ended before {seconds:g} s of the measured phase")
            left = 0.01 if now is None else min(0.01, max(0.0, seconds - now))
            time.sleep(left)

    def events(self) -> Iterator[RunEvent]:
        """Take the events of the run as they come, until the run has ended.

        ``observer`` of :func:`start` is called as well. Call this from one thread.
        """
        while True:
            try:
                yield self._events.get(timeout=0.05)
            except queue.Empty:
                if self._native.is_done():
                    break
        while not self._events.empty():
            yield self._events.get_nowait()

    def poll_event(self) -> RunEvent | None:
        """Take the next event if there is one, without waiting; ``None`` if there is none now."""
        try:
            return self._events.get_nowait()
        except queue.Empty:
            return None

    def cancel(self) -> None:
        """Ask the run to stop. :meth:`result` raises :class:`netscenario.Cancelled`."""
        self._native.cancel()

    def result(self) -> Result:
        """Wait for the run to end and return its result.

        Raises:
            ScenarioError, CapabilityError, BackendError, RunError, Cancelled: As
                :func:`netscenario.run`. The same error is raised on every call.
        """
        # Whoever comes first waits for the run; the others wait for that, and all get the same
        with self._lock:
            if not self._finished:
                self._finished = True
                try:
                    self._result = Result.from_json(self._native.finish(), self._directory)
                except BaseException as error:
                    self._error = error
                else:
                    if self._on_result is not None:
                        self._on_result(self._result)
        if self._error is not None:
            raise self._error
        if self._result is None:  # pragma: no cover - finish() either returned or raised
            raise RunError("the run has no result")
        return self._result

    def _change(self, link: str, action: str, change: str | None) -> tuple[TimelineStep, ...]:
        text = self._native.change(link, action, change)
        return tuple(step_from_record(record) for record in json.loads(text))

    def __enter__(self) -> Self:
        """Return the run."""
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        """Wait for the run, or cancel it if the block raised."""
        if exc_type is None:
            self.result()
            return
        self.cancel()
        # The exception of the block is the one that matters
        with contextlib.suppress(BaseException):
            self.result()


def _prepare(
    scenario: Scenario | str | os.PathLike[str],
    *,
    params: Mapping[str, ParamValue] | None,
    out: str | os.PathLike[str] | None,
    working_dir: str | os.PathLike[str] | None,
) -> tuple[Scenario, str, str, Path, Path]:
    if isinstance(scenario, Scenario):
        loaded = scenario.with_params(**params) if params else scenario
    else:
        loaded = Scenario.load(scenario, params=params)
    try:
        supervisor, agent = _binaries.find()
    except _binaries.ProgramsNotFoundError as error:
        raise BackendError(str(error)) from None
    directory = Path(out) if out is not None else Path(tempfile.mkdtemp(prefix="netscenario-"))
    return loaded, str(supervisor), str(agent), directory, resolve_working_dir(loaded, working_dir)


def start(
    scenario: Scenario | str | os.PathLike[str],
    *,
    live: Iterable[str] = (),
    out: str | os.PathLike[str] | None = None,
    seed: int | None = None,
    settle: float | None = None,
    packets: bool | None = None,
    capture: Iterable[str] | None = None,
    observer: Callable[[RunEvent], None] | None = None,
    cancel: _native.CancelToken | None = None,
    bin: float | None = None,
    recovery_basis: str | None = None,
    recovery_baseline: int | None = None,
    recovery_window: int | None = None,
    recovery_fraction: float | None = None,
    working_dir: str | os.PathLike[str] | None = None,
    params: Mapping[str, ParamValue] | None = None,
    on_result: Callable[[Result], None] | None = None,
) -> Run:
    """Start a run and return at once, with a :class:`Run` that can be talked to while it goes.

    The arguments are those of :func:`netscenario.run` (without ``repetitions``), and ``live``: the
    ids of the links that the code may change with :meth:`LinkHandle.set` while the run goes. A
    direction is built once and not again (its queues keep their random numbers and the packets in
    flight), so the queues of a live link are built to fit any state: two queues and a rate stage
    even while the link has no rate. A link that is not named cannot be changed from the code.

    Use the result as a context manager, or call :meth:`Run.result` to wait for the end.
    ``on_result`` is called with the result the first time it is had (on the thread that asks for
    it); a test framework uses it to put the result into its report.

    Raises:
        ScenarioError: The scenario is not valid.
        BackendError: The machine does not allow the run (see :func:`netscenario.doctor`).
        ValueError: ``live`` names a link the scenario does not have, or the checks of the scenario
            need a set of runs (``expect: {repetitions: n}``), which a single run cannot give.
    """
    loaded, supervisor, agent, directory, work = _prepare(
        scenario, params=params, out=out, working_dir=working_dir
    )
    if loaded.expect_repetitions is not None:
        message = (
            f"the checks of the scenario need a set of {loaded.expect_repetitions} runs, and a run "
            "that is changed while it goes is one run: use netscenario.run()"
        )
        raise ValueError(message)
    links = tuple(live)
    events: queue.SimpleQueue[RunEvent] = queue.SimpleQueue()

    def notify(kind: str, fields: dict[str, float]) -> None:
        event = make_event(kind, fields)
        if event is None:
            return
        events.put(event)
        if observer is not None:
            observer(event)

    native = _native.start_scenario(
        loaded.source,
        loaded.to_json(),
        loaded.source_name,
        supervisor,
        agent,
        str(directory),
        list(links),
        seed=seed,
        settle=settle,
        observer=notify,
        cancel=cancel,
        bin=bin,
        recovery_basis=recovery_basis,
        recovery_baseline=recovery_baseline,
        recovery_window=recovery_window,
        recovery_fraction=recovery_fraction,
        working_dir=str(work),
        capture=None if capture is None else list(capture),
        packets=packets,
        reading=loaded.native_reading(),
    )
    return Run(native, directory, links, events, on_result)


class _Blocking(Protocol):
    """Runs a blocking call off the loop and awaits it."""

    def __call__(self, call: Callable[[], T], /) -> Awaitable[T]: ...


class AsyncLinkHandle:
    """A live link of an :class:`AsyncRun`: the calls of :class:`LinkHandle` as coroutines."""

    def __init__(
        self,
        blocking: _Blocking,
        handle: LinkHandle,
    ) -> None:
        """Not made directly: ask the run (:meth:`AsyncRun.link`)."""
        self._blocking = blocking
        self._handle = handle
        self.id: Final = handle.id
        """The id of the link."""

    async def set(
        self,
        *,
        both: Edit | None = None,
        forward: Edit | None = None,
        reverse: Edit | None = None,
        delay: str | _Unset | None = UNSET,
        jitter: Jitter | str | Mapping[str, Any] | _Unset | None = UNSET,
        loss: Loss | str | Mapping[str, Any] | _Unset | None = UNSET,
        rate: str | _Unset | None = UNSET,
        reorder: Mapping[str, Any] | _Unset | None = UNSET,
        duplicate: str | _Unset | None = UNSET,
        corrupt: str | _Unset | None = UNSET,
    ) -> tuple[TimelineStep, ...]:
        """As :meth:`LinkHandle.set`, without blocking the loop."""
        return await self._blocking(
            lambda: self._handle.set(
                both=both,
                forward=forward,
                reverse=reverse,
                delay=delay,
                jitter=jitter,
                loss=loss,
                rate=rate,
                reorder=reorder,
                duplicate=duplicate,
                corrupt=corrupt,
            )
        )

    async def down(self) -> TimelineStep:
        """As :meth:`LinkHandle.down`."""
        return await self._blocking(self._handle.down)

    async def up(self) -> TimelineStep:
        """As :meth:`LinkHandle.up`."""
        return await self._blocking(self._handle.up)


class AsyncRun:
    """A run that is going, for ``asyncio``. Made by :func:`start_async`; use ``async with``.

    Leaving the block waits for the run to end; leaving it with an exception (a cancelled task is
    one) cancels the run and returns once the network is torn down.
    """

    def __init__(self, run: Run, loop: asyncio.AbstractEventLoop) -> None:
        """Not made directly: call :func:`start_async`."""
        self._run = run
        self._loop = loop

    @property
    def run(self) -> Run:
        """The same run for code that does not use the loop (to check :attr:`Run.done`)."""
        return self._run

    @property
    def live(self) -> tuple[str, ...]:
        """The links that may be changed."""
        return self._run.live

    @property
    def out_dir(self) -> Path:
        """The directory of the result."""
        return self._run.out_dir

    @property
    def elapsed(self) -> float | None:
        """Seconds since the workloads started, or ``None`` before."""
        return self._run.elapsed

    @property
    def done(self) -> bool:
        """Whether the run has ended."""
        return self._run.done

    def link(self, link: str) -> AsyncLinkHandle:
        """The handle of a link that was declared live (``ValueError`` if it was not)."""
        return AsyncLinkHandle(self._blocking, self._run.link(link))

    async def wait_until(self, seconds: float) -> None:
        """As :meth:`Run.wait_until`: the loop is free meanwhile."""
        while True:
            now = self._run.elapsed
            if now is not None and now >= seconds:
                return
            if self._run.done:
                await self.result()
                raise RunError(f"the run ended before {seconds:g} s of the measured phase")
            await asyncio.sleep(0.01 if now is None else min(0.01, max(0.0, seconds - now)))

    async def events(self) -> AsyncIterator[RunEvent]:
        """The events of the run as they come, until it has ended."""
        while True:
            event = self._run.poll_event()
            if event is not None:
                yield event
            elif self._run.done:
                break
            else:
                await asyncio.sleep(0.02)
        while (event := self._run.poll_event()) is not None:
            yield event

    def cancel(self) -> None:
        """Ask the run to stop."""
        self._run.cancel()

    async def result(self) -> Result:
        """Wait for the run to end and return its result (errors as :meth:`Run.result`).

        If the task is cancelled while it waits, the run is cancelled, this returns only when the
        network is torn down, and then raises ``asyncio.CancelledError``.
        """
        return await self._blocking(self._run.result)

    async def _blocking(self, call: Callable[[], T]) -> T:
        future: Awaitable[T] = self._loop.run_in_executor(None, call)
        try:
            return await asyncio.shield(future)
        except asyncio.CancelledError:
            self._run.cancel()
            with contextlib.suppress(BaseException):
                await future
            raise

    async def __aenter__(self) -> Self:
        """Return the run."""
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        """Wait for the run, or cancel it if the block raised."""
        if exc_type is None:
            await self.result()
            return
        self.cancel()
        # The exception of the block is the one that matters
        with contextlib.suppress(BaseException):
            await asyncio.shield(self._loop.run_in_executor(None, self._run.result))


async def start_async(
    scenario: Scenario | str | os.PathLike[str],
    *,
    live: Iterable[str] = (),
    out: str | os.PathLike[str] | None = None,
    seed: int | None = None,
    settle: float | None = None,
    packets: bool | None = None,
    capture: Iterable[str] | None = None,
    observer: Callable[[RunEvent], None] | None = None,
    cancel: _native.CancelToken | None = None,
    bin: float | None = None,
    recovery_basis: str | None = None,
    recovery_baseline: int | None = None,
    recovery_window: int | None = None,
    recovery_fraction: float | None = None,
    working_dir: str | os.PathLike[str] | None = None,
    params: Mapping[str, ParamValue] | None = None,
    on_result: Callable[[Result], None] | None = None,
) -> AsyncRun:
    """Start a run for ``asyncio`` and return its :class:`AsyncRun`.

    Use it as ``async with await start_async(scenario) as run``.

    The arguments are those of :func:`start`. The scenario is read and the run is started without
    blocking the loop. ``observer`` is called on the thread of the run; use
    :meth:`AsyncRun.events` to take the events on the loop.
    """
    loop = asyncio.get_running_loop()
    run = await loop.run_in_executor(
        None,
        lambda: start(
            scenario,
            live=live,
            out=out,
            seed=seed,
            settle=settle,
            packets=packets,
            capture=capture,
            observer=observer,
            cancel=cancel,
            bin=bin,
            recovery_basis=recovery_basis,
            recovery_baseline=recovery_baseline,
            recovery_window=recovery_window,
            recovery_fraction=recovery_fraction,
            working_dir=working_dir,
            params=params,
            on_result=on_result,
        ),
    )
    return AsyncRun(run, loop)


async def run_async(
    scenario: Scenario | str | os.PathLike[str],
    *,
    out: str | os.PathLike[str] | None = None,
    seed: int | None = None,
    settle: float | None = None,
    packets: bool | None = None,
    capture: Iterable[str] | None = None,
    observer: Callable[[RunEvent], None] | None = None,
    cancel: _native.CancelToken | None = None,
    bin: float | None = None,
    recovery_basis: str | None = None,
    recovery_baseline: int | None = None,
    recovery_window: int | None = None,
    recovery_fraction: float | None = None,
    working_dir: str | os.PathLike[str] | None = None,
    params: Mapping[str, ParamValue] | None = None,
) -> Result:
    """Run a scenario without blocking the event loop and return the result.

    The arguments and the errors are those of :func:`netscenario.run` (without ``repetitions``).
    If the task is cancelled, the run is cancelled, the call returns when the network is torn down,
    and ``asyncio.CancelledError`` is raised. The run is done on threads that end with it.
    """
    run = await start_async(
        scenario,
        out=out,
        seed=seed,
        settle=settle,
        packets=packets,
        capture=capture,
        observer=observer,
        cancel=cancel,
        bin=bin,
        recovery_basis=recovery_basis,
        recovery_baseline=recovery_baseline,
        recovery_window=recovery_window,
        recovery_fraction=recovery_fraction,
        working_dir=working_dir,
        params=params,
    )
    async with run:
        return await run.result()


__all__ = [
    "UNSET",
    "AsyncLinkHandle",
    "AsyncRun",
    "LinkHandle",
    "Run",
    "run_async",
    "start",
    "start_async",
]
