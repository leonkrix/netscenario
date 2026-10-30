# The commands of the project. `just` lists them; `just check` is what the CI runs, in the same order.
# Needs: rustup (stable), just, cargo-nextest and cargo-deny.

set shell := ["bash", "-euo", "pipefail", "-c"]

# List the commands
default:
    @just --list

# Everything the CI checks: format, lints, tests, documentation and the supply chain, for Rust and Python
check: fmt-check clippy test doc deny py-check

# Format the code
fmt:
    cargo fmt --all

# Check the format without changing anything
fmt-check:
    cargo fmt --all --check

# Lints; every warning is an error
clippy:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# The tests of the Kernel backend alone. The CI runs them a second time as root (`sudo`); on a machine
# without sudo, `unshare --user --map-root-user just test-kernel` takes the same code path as root.
test-kernel:
    cargo nextest run -p netscenario-backend-kernel --all-features --no-tests=pass

# Unit and doc tests (nextest does not run doc tests)
test:
    cargo nextest run --workspace --all-features --no-tests=pass
    cargo test --workspace --all-features --doc

# The API documentation; a broken link or a missing item is an error
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Licences, advisories, sources and bans (deny.toml)
deny:
    cargo deny check

# Check that the code builds with the lowest supported Rust (rust-version in Cargo.toml). One-time: rustup toolchain install 1.89.0 --profile minimal
msrv:
    cargo +1.89.0 check --workspace --all-targets

# Set up the Python environment (builds the native module with maturin)
py-sync:
    uv sync

# Python: format, lints, types (pyright, strict) and tests
py-check: py-sync
    uv run ruff format --check python
    uv run ruff check python
    uv run pyright
    uv run pytest

# Format the Python code and fix what ruff can fix
py-fmt: py-sync
    uv run ruff check --fix python
    uv run ruff format python

# Run the Python tests with the lowest supported Python (3.11, uv fetches it)
py-test-min:
    uv run --python 3.11 --isolated --with-editable python/netscenario --with-editable python/pytest-netscenario --with pytest pytest

# Write docs/schema/scenario-v0.json from the types (after a change to the model)
schema:
    UPDATE_SCHEMA=1 cargo nextest run -p netscenario-model --test schema

# Accept the changed error messages in the snapshots (read the diff first: `git diff crates/netscenario-model/tests/snapshots`)
snapshots:
    INSTA_UPDATE=always cargo nextest run -p netscenario-model --test corpus

# Run the spikes of Phase 0 (needs iproute2, python3 and, for S1 to S5, user namespaces)
spikes:
    bash spikes/run-all.sh

# Turn on the git hooks of this repository (once per clone)
hooks:
    git config core.hooksPath .githooks
    @echo "git hooks are on: .githooks/pre-commit checks the format, the lints and the tests of what you commit"
