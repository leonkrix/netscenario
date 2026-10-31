#!/usr/bin/env bash
# S15: does the kernel accept a duplicating netem together with another netem in the same qdisc tree?
#
# Some kernels refuse it: "netem: cannot mix duplicating netems with other netems in tree"
# (check_netem_in_tree(), added in 2025 against a soft lockup, CVE-2025-38553, and reverted upstream later; a
# distribution kernel may have either state). The Kernel backend builds two netems for a direction with a
# jitter and a rate (D-030: the rate first, then the delay in a child), so `duplicate` together with both
# would be refused on such a kernel. S12 measured `duplicate` on one netem only. The answer is a finding for
# each kernel, not a pass or a fail: it decides whether the backend can promise the combination (D-052).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [ "${1:-outer}" = outer ]; then
  enter_ns S15.0 "$0"
fi
trap ns_stop EXIT
ip link add v0 type veth peer name v1 || { fail S15.0 "no veth pair"; exit 0; }
ip link add v2 type veth peer name v3 || { fail S15.0 "no second veth pair"; exit 0; }
for dev in v0 v1 v2 v3; do ip link set "$dev" up; done
info S15.0 "kernel $(uname -r); $(tc -V 2>&1 | head -n 1)"

bare() { tc qdisc del dev "$1" root >/dev/null 2>&1; return 0; }

# try ID "text" command...   a refusal is a result: INFO with the message, an acceptance is PASS
try() {
  local id="$1" text="$2" err
  shift 2
  if err="$("$@" 2>&1)"; then
    pass "$id" "$text: accepted"
    ACCEPTED+=("$id")
  else
    info "$id" "$text: REFUSED :: $(printf '%s' "$err" | head -n 2 | tr '\n' ' ')"
    REFUSED+=("$id")
  fi
}
ACCEPTED=()
REFUSED=()

# Canary: a value that no kernel accepts is refused and shows up as REFUSED, so the cases below can say no
bare v0
try S15.1 "canary, duplicate 150 percent" tc qdisc add dev v0 root netem duplicate 150%
if printf '%s\n' "${REFUSED[@]:-}" | grep -q S15.1; then
  pass S15.1 "canary: a refusal is seen as a refusal"
else
  fail S15.1 "canary: an impossible value was accepted, the cases below cannot be trusted"
fi

bare v0
try S15.2 "one netem with duplicate (what S12 measured)" tc qdisc add dev v0 root netem limit 1000 duplicate 20%

bare v0
tc qdisc add dev v0 root handle 1: netem limit 1000 duplicate 20% rate 1mbit
try S15.3 "duplicate in the root netem (with the rate), a child netem with delay and jitter" \
  tc qdisc add dev v0 parent 1: handle 2: netem limit 1000 delay 10ms 2ms

bare v0
tc qdisc add dev v0 root handle 1: netem limit 1000 rate 1mbit
try S15.4 "a root netem with the rate, duplicate in the child with delay and jitter" \
  tc qdisc add dev v0 parent 1: handle 2: netem limit 1000 delay 10ms 2ms duplicate 20%

bare v0
tc qdisc add dev v0 root handle 1: netem limit 1000 rate 1mbit
tc qdisc add dev v0 parent 1: handle 2: netem limit 1000 delay 10ms 2ms
try S15.5 "two netems exist, the root is changed to duplicate" \
  tc qdisc change dev v0 handle 1: root netem limit 1000 rate 1mbit duplicate 20%

bare v0
tc qdisc add dev v0 root handle 1: netem limit 1000 rate 1mbit duplicate 20% 2>/dev/null
tc qdisc add dev v0 parent 1: handle 2: netem limit 1000 delay 10ms 2ms 2>/dev/null
try S15.6 "a tree that has a duplicating netem, the delay of the child is changed" \
  tc qdisc change dev v0 handle 2: parent 1: netem limit 1000 delay 20ms 2ms

# Two interfaces are two trees: a duplicating netem on one does not touch the other
bare v0
bare v2
tc qdisc add dev v0 root netem limit 1000 duplicate 20%
try S15.7 "duplicate on one interface, a plain netem on another" \
  tc qdisc add dev v2 root netem limit 1000 delay 10ms

# The summary the decision needs
if printf '%s\n' "${REFUSED[@]:-}" | grep -qE 'S15\.(3|4|5|6)'; then
  info S15.8 "this kernel refuses a duplicating netem in a tree with another netem: duplicate with a jitter AND a rate cannot be built here (cases: ${REFUSED[*]})"
else
  info S15.8 "this kernel accepts a duplicating netem in a tree with another netem (refused: ${REFUSED[*]:-none})"
fi
bare v0
bare v2
exit 0
