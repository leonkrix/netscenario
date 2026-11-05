"""netscenario: declarative network scenarios for tests.

Describe a network (nodes, links, impairments), run it in network namespaces, measure per packet,
and assert. The semantics live in the Rust core; this package is its typed Python interface
(``docs/ARCHITECTURE.md``).

Example:
    >>> import netscenario as ns
    >>> scenario = ns.Scenario.load("scenarios/lte-like.yaml")  # doctest: +SKIP
    >>> result = ns.run(scenario)  # doctest: +SKIP
    >>> assert result["media"].delay.p99 < 0.150  # seconds  # doctest: +SKIP
"""

from netscenario import _native, events
from netscenario._version import pep440
from netscenario.errors import (
    BackendError,
    Cancelled,
    CapabilityError,
    NetscenarioError,
    Refusal,
    RunError,
    ScenarioError,
    ScenarioProblem,
)
from netscenario.replay import Difference, LostPackets, Replay, Verdict, replay
from netscenario.reports import ReportFormat, junit_properties, report
from netscenario.results import (
    Address,
    Bin,
    BinDelay,
    ByArrival,
    BySend,
    Capture,
    Delay,
    DirectionTally,
    Echo,
    Extent,
    InterarrivalJitter,
    LateTime,
    LossRun,
    LossRuns,
    Metric,
    Program,
    ProgramOutput,
    Provenance,
    QueueCheck,
    QueueSample,
    QueueStats,
    Recovery,
    Reordering,
    Reproducibility,
    Restriction,
    Result,
    SeedInfo,
    Sender,
    Series,
    Tcp,
    TcpGoodput,
    TcpRequests,
    TcpTransfer,
    Throughput,
    TimelineStep,
    TraceInfo,
    Workload,
)
from netscenario.run import CancelToken, Check, DoctorReport, doctor, run
from netscenario.runset import (
    Confidence,
    Interval,
    PartialMetric,
    RunSet,
    SetMetric,
    SetRun,
    SetSeed,
)
from netscenario.scenario import Impairments, Jitter, Loss, Scenario, ScenarioBuilder

__version__: str = pep440(_native.version())
"""The version of the package as pip knows it (``0.1.0rc1``).

The library and the programs write the same version as Cargo does (``0.1.0-rc.1``).
"""

__all__ = [
    "Address",
    "BackendError",
    "Bin",
    "BinDelay",
    "ByArrival",
    "BySend",
    "CancelToken",
    "Cancelled",
    "CapabilityError",
    "Capture",
    "Check",
    "Confidence",
    "Delay",
    "Difference",
    "DirectionTally",
    "DoctorReport",
    "Echo",
    "Extent",
    "Impairments",
    "InterarrivalJitter",
    "Interval",
    "Jitter",
    "LateTime",
    "Loss",
    "LossRun",
    "LossRuns",
    "LostPackets",
    "Metric",
    "NetscenarioError",
    "PartialMetric",
    "Program",
    "ProgramOutput",
    "Provenance",
    "QueueCheck",
    "QueueSample",
    "QueueStats",
    "Recovery",
    "Refusal",
    "Reordering",
    "Replay",
    "ReportFormat",
    "Reproducibility",
    "Restriction",
    "Result",
    "RunError",
    "RunSet",
    "Scenario",
    "ScenarioBuilder",
    "ScenarioError",
    "ScenarioProblem",
    "SeedInfo",
    "Sender",
    "Series",
    "SetMetric",
    "SetRun",
    "SetSeed",
    "Tcp",
    "TcpGoodput",
    "TcpRequests",
    "TcpTransfer",
    "Throughput",
    "TimelineStep",
    "TraceInfo",
    "Verdict",
    "Workload",
    "__version__",
    "doctor",
    "events",
    "junit_properties",
    "replay",
    "report",
    "run",
]
