# Releasing

A release is a tag. What a version promises is in [VERSIONING.md](VERSIONING.md); the reasons for the design are D-034 in [DECISIONS.md](DECISIONS.md). One version covers everything: the five crates on crates.io (`netscenario-model`, `-core`, `-backend-kernel`, `-agent`, `-cli`), the two Python packages on PyPI (`netscenario` as wheels for Linux x86_64 and aarch64, `pytest-netscenario`), and a GitHub release.

## The short version

```bash
# Release day, from an up-to-date main
git switch main && git pull
git switch -c release/0.1.0
just release-prepare 0.1.0     # the version in every file, the lock files, CHANGELOG.md
git add -A && git commit -m "chore: release 0.1.0"
git push -u origin release/0.1.0   # pull request: wait for "CI passed", read CHANGELOG.md, squash and merge

git switch main && git pull    # the squash commit is a new commit: tag that one
just release-tag               # checks that it is main, clean and in step with origin; tags and pushes v0.1.0
```

Then, on GitHub, **Actions, Release**: the checks and the builds run, and the two publishing jobs wait for you. Open the run and press **Review deployments**, tick the environment, **Approve and deploy**, twice one after the other: `crates-io`, then `pypi` (the wheels and then the plugin, in one job). When both are through, the job `GitHub release` creates the release with the notes of that version. The deployments (on the main page of the repository) link to the published versions.

## What each step does

**`just release-prepare X.Y.Z`** runs `python/release/version.py set` (the version in `Cargo.toml`, the internal dependencies, the plugin and its exact pin on `netscenario`), refreshes `Cargo.lock` and `uv.lock`, checks that every place agrees, and writes the section of the version into `CHANGELOG.md` with [git-cliff](https://git-cliff.org) (`cliff.toml`). The first release creates the file; later ones put their section on top and keep what is there.

The changelog is made of the **titles of the pull requests** (squash merge: the title is the commit). `feat` and `fix` are listed, `docs`, `test`, `ci`, `chore`, `refactor`, `build` and `style` are not, unless they have an exclamation mark (`feat!:`), which makes a line **Breaking**. A title that is not a Conventional Commit appears under "Other" so that you see it. **Read the file before you merge**: the section is yours to edit (titles that GitHub cut off with "…", lines that need an explanation), and an edit stays, because the next release only adds on top. The section is also the text of the GitHub release, so links in what you write are absolute (`https://github.com/leonkrix/netscenario/...`); a relative link works in the file and not on the release page.

**The pull request** runs the CI as always, including the job *Packages*, which does what `cargo publish` does before it uploads (`just publish-dry-run`): it packages every crate in dependency order and builds each from its package.

**`just release-tag`** refuses unless you are on `main`, with no local changes, equal to `origin/main`, every file has one version, and `CHANGELOG.md` has the section. A tag made on the branch of the pull request is wrong: a squash merge makes a **new** commit that only `main` has, and the workflow refuses a commit that is not on `main`. If that happens: `git push origin :refs/tags/v0.1.0 && git tag -d v0.1.0`, then tag on `main`.

**The workflow `Release`** (`.github/workflows/release.yml`):

1. *Verify*: the tag names the version in every file, the commit is on `main`, `CHANGELOG.md` has the section, and the CI of this very commit passed (`CI passed`).
2. *Wheels*: the same workflow as the CI (`wheels.yml`) builds the wheels in the manylinux containers on both architectures and installs them into clean environments; the plugin is built; the four files (two wheels, plugin wheel, plugin source distribution) are checked for names and metadata (`twine check --strict`).
3. *Package the crates*: `cargo publish --workspace --dry-run`.
4. *Publish the crates* (after your approval): trusted publishing, in dependency order.
5. *Publish to PyPI* (after your approval, after the crates): the two wheels of `netscenario`, then the plugin `pytest-netscenario`, which needs exactly these wheels, so it comes second. Trusted publishing with attestations (PEP 740: the provenance of each file).
6. *GitHub release*: the notes of the version, the four files attached; a version with a hyphen (`0.2.0-rc.1`) is marked as a pre-release.

No token is stored anywhere. Each registry accepts an upload from this workflow, in its own environment (`crates-io` for crates.io, `pypi` for PyPI), and from nothing else; the environment needs your approval. Two environments mean that the job for one registry cannot ask for the token of the other.

## A rehearsal

**Actions, Release, Run workflow**, on any branch, does steps 1 (without the checks about the tag), 2 and 3 and stops before the publishing: it needs no tag and no approval. Do this once before the first real release, and again when `wheels.yml`, `release.yml` or the packaging changed. For a rehearsal that includes the publishing, use a pre-release version (`0.1.0-rc.1`): crates.io and PyPI keep it, but nobody gets it by default.

## When something goes wrong

- **A check fails before the publishing**: nothing is out. Fix it with a pull request; if the release commit changes, delete the tag (above) and tag again. A tag that was pushed but never approved can also stay: reject the deployment.
- **The crates job fails halfway**: some crates may be on crates.io and others not. Run the job again; if cargo refuses because a crate with this version exists, publish the rest by hand in dependency order (`netscenario-agent`, `netscenario-model`, `netscenario-core`, `netscenario-backend-kernel`, `netscenario-cli`) with `cargo publish -p <crate>`, using a token you create for the purpose and delete afterwards.
- **The PyPI job fails**: run the failed job again. Files that are already there are skipped, so if the wheels are out and the plugin is not, the repeat uploads only the plugin.
- **A release is faulty**: **yank** it (`cargo yank --version X.Y.Z <crate>` for each crate, and *Yank* in the project's release page on pypi.org). A yanked version stays installable for those who pinned it. Fix with a patch release; a version number is never reused.

## One-time setup

### GitHub

1. **Settings, General**: allow only squash merging; automatically delete head branches.
2. **Settings, Rules, Rulesets**: `protect-main` as for the other repositories (pull request required, squash only, the check **`CI passed`**, no bypass), and a second ruleset for **tags** `v*`: restrict creation to yourself and block deletion and updates, so that nobody else can start a release.
3. **Settings, Environments, New environment**, twice: **`crates-io`** and **`pypi`**. For each: *Required reviewers*: you; *Deployment branches and tags*: only tags matching `v*`. (A single maintainer cannot approve their own run unless "Prevent self-review" is off, which it is by default.)
4. **Settings, Actions, General**: workflow permissions read-only.

### PyPI

1. An account with two-factor authentication.
2. For each of the two projects, `netscenario` and `pytest-netscenario`, a trusted publisher: GitHub; owner `leonkrix`; repository `netscenario`; workflow `release.yml`; environment **`pypi`**.
   - A project that does not exist yet gets a *pending publisher* (**Your account, Publishing**, section *Add a new pending publisher*); it does not reserve the name, the first upload does, and the upload creates the project. Check on the day that the names are still free.
   - **PyPI lets only one pending publisher exist for a combination of repository, workflow and environment** (the form refuses the second one with "already been registered for a different project name"). Two new projects therefore cannot be registered before their first release; register the second one after the first upload of the first. Once both projects exist, the same combination may be registered for both (**Manage the project, Publishing**), which is the state of this repository.

### crates.io

Trusted publishing exists only for a crate that has been published once, by hand. So the five names are taken with the version that the repository has before the first release, `0.0.0`, and then closed for tokens:

1. An account (GitHub login) with a verified email address, and an API token with the scopes *publish-new*, *publish-update* and *yank* for the crates `netscenario-*`, valid for a day. (Without *yank* the command in step 3 answers 403 Forbidden; the same can be done on the page of each crate, *Versions*, *Yank*.)
2. From a clean checkout of `main` (after the pull request with this setup is merged):

   ```bash
   cargo login                       # paste the token
   cargo publish --workspace         # 0.0.0, in dependency order
   ```

3. Take the version back from the index so that nobody installs a half-built `0.0.0`: `cargo yank --version 0.0.0 <crate>` for each of the five crates. The names stay yours.
4. Delete the token (**Account settings, API tokens**).
5. For each of the five crates: **Settings, Trusted Publishing, Add**: GitHub; repository owner `leonkrix`; repository `netscenario`; workflow `release.yml`; environment **`crates-io`**.
6. Check on the day that the names are still free, in case the placeholder step is late.

### Then

Rehearse (above), and release a candidate (`0.1.0-rc.1`, `-rc.2`, ...) and then `0.1.0`.

## The first release: texts that say "not published yet"

The README of the repository is the README of the five crates on crates.io (the page of each crate shows it as it is in the release), so what it says on the day of the tag is what the page of `0.1.0` says for good. Therefore, **in the release pull request of `0.1.0`** (the one with `just release-prepare`), not after it:

- `README.md`: remove the box "Release 0.1.0 is not published yet" and the words that go with it ("what will work after the release"), and the line "From source" can stay.
- `docs/PYTHON.md` ("Installing") and `docs/PYTEST.md` (the sentence "There is no release on PyPI yet"): replace by the `pip install` line.

(A release candidate keeps the box: its page says that it is one.) After both registries have the release, in a small pull request: the badges for PyPI (`https://img.shields.io/pypi/v/netscenario`), crates.io (`https://img.shields.io/crates/v/netscenario-cli`) and the latest release in `README.md`, because a badge of a package that does not exist shows an error. Then check the installation as a stranger would: a clean virtual environment, `pip install netscenario pytest-netscenario`, `netscenario doctor`, a run; and `cargo install netscenario-cli netscenario-agent` into an empty `--root`.

## What is not in a release yet

Static binaries for download (the agent is not static, D-025), musllinux and macOS wheels (Linux first), a source distribution of `netscenario` (it needs Rust and the two programs, so the wheel is the way to install it), signing of the GitHub release, and Software Bills of Materials.
