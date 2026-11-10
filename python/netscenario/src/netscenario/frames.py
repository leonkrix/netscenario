"""Tables of the numbers of a result, for pandas and polars (PYT-05).

The per-packet records of a workload (:meth:`netscenario.Workload.packets_table`), its series
(:meth:`netscenario.Series.table`) and the steps of the timeline
(:meth:`netscenario.Result.steps_table`) are a :class:`Table`: columns of plain Python values with
the units of the rest of the package (seconds, bits per second, ratios from 0 to 1). A table
becomes a frame with :meth:`Table.to_pandas` or :meth:`Table.to_polars`.

Neither library is a dependency of the package: they are the extras ``netscenario[pandas]`` and
``netscenario[polars]``, and they are imported only when the method is called. Without the library
the method raises an :class:`ImportError` that says which extra to install.
"""

from __future__ import annotations

import importlib
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class Table:
    """Columns of equal length, by name, in the order of the table."""

    columns: Mapping[str, Sequence[Any]]
    """The values of each column; a missing value is ``None``."""

    def __post_init__(self) -> None:
        """Check that the columns have one length."""
        lengths = {len(values) for values in self.columns.values()}
        if len(lengths) > 1:
            message = f"the columns of a table have one length, not {sorted(lengths)}"
            raise ValueError(message)

    @property
    def names(self) -> tuple[str, ...]:
        """The names of the columns."""
        return tuple(self.columns)

    def __len__(self) -> int:
        """The number of rows."""
        return len(next(iter(self.columns.values()), ()))

    def __getitem__(self, name: str) -> Sequence[Any]:
        """The values of the column ``name``."""
        return self.columns[name]

    def rows(self) -> Iterator[dict[str, Any]]:
        """The rows as dictionaries, in order."""
        names = self.names
        for values in zip(*self.columns.values(), strict=True):
            yield dict(zip(names, values, strict=True))

    def to_dicts(self) -> list[dict[str, Any]]:
        """The rows as a list of dictionaries."""
        return list(self.rows())

    def to_pandas(self) -> Any:  # noqa: ANN401 - the type belongs to a library that may not be there
        """The table as a ``pandas.DataFrame``. Needs ``pip install netscenario[pandas]``."""
        pandas = _library("pandas", "pandas")
        return pandas.DataFrame({name: list(values) for name, values in self.columns.items()})

    def to_polars(self) -> Any:  # noqa: ANN401 - the type belongs to a library that may not be there
        """The table as a ``polars.DataFrame``. Needs ``pip install netscenario[polars]``."""
        polars = _library("polars", "polars")
        return polars.DataFrame({name: list(values) for name, values in self.columns.items()})


def _library(module: str, extra: str) -> Any:  # noqa: ANN401 - a module
    try:
        return importlib.import_module(module)
    except ImportError as error:
        message = f"this needs {module}, which is not installed: pip install 'netscenario[{extra}]'"
        raise ImportError(message) from error
