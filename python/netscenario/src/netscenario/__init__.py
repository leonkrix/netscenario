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
from netscenario.results import (
    Address,
    Delay,
    DirectionAccount,
    Provenance,
    QueueCheck,
    Restriction,
    Result,
    SeedInfo,
    Sender,
    Throughput,
    Workload,
)
from netscenario.run import CancelToken, Check, DoctorReport, doctor, run
from netscenario.scenario import Impairments, Jitter, Loss, Scenario, ScenarioBuilder

__version__: str = pep440(_native.version())
"""The version of the package as pip knows it (``0.1.0rc1``).

The library and the programs write the same version as Cargo does (``0.1.0-rc.1``).
"""

__all__ = [
    "Address",
    "BackendError",
    "CancelToken",
    "Cancelled",
    "CapabilityError",
    "Check",
    "Delay",
    "DirectionAccount",
    "DoctorReport",
    "Impairments",
    "Jitter",
    "Loss",
    "NetscenarioError",
    "Provenance",
    "QueueCheck",
    "Refusal",
    "Restriction",
    "Result",
    "RunError",
    "Scenario",
    "ScenarioBuilder",
    "ScenarioError",
    "ScenarioProblem",
    "SeedInfo",
    "Sender",
    "Throughput",
    "Workload",
    "__version__",
    "doctor",
    "events",
    "run",
]
