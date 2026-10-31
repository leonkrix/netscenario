#!/usr/bin/env bash
# S14: what a program of the user needs in a node: readiness probes from the outside, a stop that really stops it,
# the user it runs as, and a limit on its output. The checks are in s14_program.py; this script builds the node.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [ "${1:-outer}" = outer ]; then
  enter_ns S14.0 "$0"
fi
trap ns_stop EXIT
if ! pair_up; then
  fail S14.0 "the two namespaces and the veth pair could not be set up"
  exit 0
fi
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$here/s14_program.py" "$NS_PID" | tee -a "$RESULTS"
