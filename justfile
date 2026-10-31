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
# The fidelity tests are left out here: they take a minute, say the same on every run, and the CI runs them
# in the first pass already. `just fidelity` runs them.
test-kernel:
    cargo nextest run -p netscenario-backend-kernel -p netscenario-agent --all-features --no-tests=pass -E 'not binary(fidelity)'

# The fidelity tests (QA-06) and a rewrite of the measured block in docs/FIDELITY.md with what this machine
# measured. Commit the file only from a machine that you want to be the reference.
fidelity:
    rm -rf target/tmp/fidelity
    UPDATE_FIDELITY=1 cargo nextest run -p netscenario-backend-kernel -p netscenario-agent --all-features -E 'binary(fidelity)'

# Unit and doc tests (nextest does not run doc tests). The Python extension is a cdylib: cargo has no doc
# tests for it and would say so on every run, so it is left out of the doc tests.
test:
    cargo nextest run --workspace --all-features --no-tests=pass
    cargo test --workspace --exclude netscenario-py --all-features --doc

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

# The two programs that a run needs (the supervisor and the agent); the Python package finds them in
# target/ in a checkout (netscenario/_binaries.py)
py-programs:
    cargo build -p netscenario-cli -p netscenario-agent

# Python: format, lints, types (pyright, strict) and tests
py-check: py-sync py-programs
    uv run ruff format --check python examples
    uv run ruff check python examples
    uv run pyright
    uv run pytest

# Format the Python code and fix what ruff can fix
py-fmt: py-sync
    uv run ruff check --fix python examples
    uv run ruff format python examples

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

# A wheel of this platform with the two programs inside, and the plugin; a proof of the packaging, not a
# release: it is tagged `linux`, not manylinux (the CI builds those in the manylinux containers)
wheel:
    cargo build --release -p netscenario-cli -p netscenario-agent
    rm -rf python/netscenario/src/netscenario/_bin dist/wheels
    mkdir -p python/netscenario/src/netscenario/_bin dist/wheels
    cp target/release/netscenario target/release/netscenario-agent python/netscenario/src/netscenario/_bin/
    cd python/netscenario && uvx maturin@1.16.0 build --release --compatibility linux -o ../../dist/wheels
    uv build --package pytest-netscenario --out-dir dist/wheels

# Install the wheels into a clean environment and run a scenario from there (python is the version)
wheel-smoke python="3.11": wheel
    rm -rf target/wheel-venv
    uv venv --python {{python}} target/wheel-venv
    uv pip install --python target/wheel-venv/bin/python dist/wheels/netscenario-*.whl dist/wheels/pytest_netscenario-*.whl
    cd target && wheel-venv/bin/python -I ../python/smoke/wheel_smoke.py


# Prepare a release (docs/RELEASING.md): the version everywhere, the lock files and the changelog
release-prepare version:
    python3 python/release/version.py set {{version}}
    cargo update --workspace
    uv lock
    python3 python/release/version.py check
    just changelog {{version}}

# Write the section of a version into CHANGELOG.md (the first one creates the file, later ones are put on top)
changelog version:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -f CHANGELOG.md ]; then
        uvx git-cliff@2.14.2 --unreleased --tag v{{version}} --prepend CHANGELOG.md
    else
        uvx git-cliff@2.14.2 --unreleased --tag v{{version}} --output CHANGELOG.md
    fi

# Everything that cargo checks before it uploads the crates, in dependency order (nothing is uploaded).
# It builds in a directory of its own that is emptied first: cargo treats the sources of a registry as
# unchangeable, so a crate that was built in an earlier run under the same version number is not built again,
# and the check would run against the code of that earlier run.
publish-dry-run:
    rm -rf target/publish-check
    CARGO_TARGET_DIR=target/publish-check cargo publish --workspace --dry-run --locked --allow-dirty

# Tag the release commit on main and push the tag, which starts the release workflow (docs/RELEASING.md)
release-tag:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "$(git branch --show-current)" = main ] || { echo "not on main"; exit 1; }
    [ -z "$(git status --porcelain)" ] || { echo "there are local changes"; exit 1; }
    git fetch --quiet origin main
    [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || { echo "main is not the same as origin/main"; exit 1; }
    python3 python/release/version.py check
    version="$(python3 python/release/version.py show)"
    grep -q "^## ${version//./\\.} (" CHANGELOG.md || { echo "CHANGELOG.md has no section for $version"; exit 1; }
    git tag --annotate "v$version" --message "v$version"
    git push origin "v$version"
    echo "pushed v$version: the workflow Release is running, and waits for your approval"
