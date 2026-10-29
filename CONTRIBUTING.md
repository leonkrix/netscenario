# Contributing

Thank you for looking at this. Bug reports, ideas, measurements from your own setup and code are all welcome.

The project is in its planning phase (see [FEATURES.md](FEATURES.md)). That makes ideas, review of the design documents and results from the [spikes](spikes/README.md) the most useful contributions right now.

## How a change gets in

1. **Issue first, for anything bigger than a small fix.** A bug with a reproduction, an idea for a feature, a scenario you cannot describe: open an [issue](https://github.com/leonkrix/netscenario/issues) and say what you test. It saves you from writing code that does not fit.
2. **Small fixes and documentation: straight to a pull request.**
3. **Fork, branch, pull request** against `main`. You do not need write access.
4. **The CI runs.** The first time you contribute, a maintainer has to approve the run (GitHub asks for that for every new contributor). The check `CI passed` has to be green.
5. **The maintainer reviews and merges.** Pull requests are squash-merged, so the title of the pull request becomes the commit message: write it as a [Conventional Commit](https://www.conventionalcommits.org) (`feat: ...`, `fix: ...`, `docs: ...`, `chore: ...`).

## A new feature starts in the ledger

Every feature has a line in [FEATURES.md](FEATURES.md) with an identifier, a priority, a release and a status. A pull request that adds a feature moves its line forward and, where it changes what a backend can do faithfully, updates [docs/COVERAGE.md](docs/COVERAGE.md). A pull request that adds a feature nobody planned adds the line first. This is how nothing gets forgotten and nothing is built twice.

## Decisions are written down

A choice that is hard to undo (a file format, a dependency of the core, a public name) gets an entry in [docs/DECISIONS.md](docs/DECISIONS.md) with the reason and the alternatives. If you disagree with one, open an issue that refers to its number.

## Development

The code is a Cargo workspace in `crates/`; the Python packages follow. You need Linux (the emulation uses network namespaces), [rustup](https://rustup.rs) and a few tools:

```bash
rustup toolchain install 1.85.0 --profile minimal   # the lowest supported Rust, for `just msrv`
cargo install --locked just cargo-nextest cargo-deny
just hooks                                          # once per clone: checks run before every commit
```

The development toolchain is the current stable Rust (`rust-toolchain.toml` installs it, with `rustfmt` and `clippy`). The lowest supported version is `rust-version` in `Cargo.toml`.

| Command | What it does |
| --- | --- |
| `just check` | Everything the CI checks: format, lints (every warning is an error), tests, documentation, supply chain |
| `just fmt` | Format the code |
| `just clippy`, `just test`, `just doc`, `just deny` | One of the checks alone |
| `just msrv` | Build with the lowest supported Rust |
| `just spikes` | Run the experiments of [spikes/](spikes/) (they need `iproute2`, `python3` and user namespaces) |

The crates and what they are for are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Library code does not print and does not use `unwrap`; a crate that needs `unsafe` says why in its own `Cargo.toml`. A new dependency is a decision for the supply chain: `deny.toml` lists the allowed licences.

By contributing you agree that your contribution is licensed under the terms in the README (Apache-2.0 or MIT, at the choice of the user).
