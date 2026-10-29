#!/usr/bin/env bash
# S9: builds a wheel with maturin in a throwaway virtual environment, installs it together with a pytest plugin
# from a second distribution, and runs the tests. Needs python3 with venv and network access (PyPI).
#   bash spikes/rust-pyo3/run.sh
set -eu
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="${S9_WORK:-$(mktemp -d)}"
echo "# S9 work directory: $work"
python3 -m venv "$work/venv"
"$work/venv/bin/python" -m pip install --quiet --upgrade pip
"$work/venv/bin/python" -m pip install --quiet maturin pytest
"$work/venv/bin/maturin" --version
# The wheel is built from a copy so that no build output lands in the repository
rm -rf "$work/src" && mkdir -p "$work/src" && cp -r "$here/." "$work/src/" && rm -rf "$work/src/target"
(cd "$work/src" && "$work/venv/bin/maturin" build --release --out "$work/wheels")
ls -la "$work/wheels"
"$work/venv/bin/python" -m pip install --quiet "$work"/wheels/*.whl
"$work/venv/bin/python" -m pip install --quiet "$work/src/plugin"
"$work/venv/bin/python" -m pip list --format=freeze | grep -i -E "netscenario|pytest"
cd "$work" && "$work/venv/bin/python" -m pytest -q "$work/src/tests" "$work/src/plugin/tests"
