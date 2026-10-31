#!/usr/bin/env bash
# S12: do the loss models, reordering, duplication and corruption of netem do what their parameters say?
# The arithmetic is in s12_models.py; this script builds the two namespaces and runs it.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [ "${1:-outer}" = outer ]; then
  enter_ns S12.0 "$0"
fi
trap ns_stop EXIT
if ! pair_up; then
  fail S12.0 "the two namespaces and the veth pair could not be set up"
  exit 0
fi
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$here/s12_models.py" "$NS_PID" | tee -a "$RESULTS"
