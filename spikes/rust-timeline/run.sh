#!/usr/bin/env bash
# S11: builds the scratch crate and runs it inside a user and network namespace (a plain one as root), so that the
# network of the host is never touched. It builds the same two-namespace set-up as the other spikes (lib.sh).
#   bash spikes/rust-timeline/run.sh            the whole run (about 10 minutes)
#   QUICK=1 bash spikes/rust-timeline/run.sh    fewer runs (about 2 minutes)
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ "${1:-outer}" = outer ]; then
  . "$here/../lib.sh"
  command -v cargo >/dev/null || { fail S11.0 "no cargo"; exit 0; }
  cargo build --release --quiet --manifest-path "$here/Cargo.toml" || { fail S11.0 "the crate does not build"; exit 0; }
  enter_ns S11.0 "$here/run.sh"
fi
. "$here/../lib.sh"
trap ns_stop EXIT
if ! pair_up; then
  fail S11.0 "the two namespaces and the veth pair could not be set up"
  exit 0
fi
export QUICK="${QUICK:-0}"
"$here/target/release/spike-timeline" all "$NS_PID" | tee -a "$RESULTS"
