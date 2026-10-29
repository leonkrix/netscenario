#!/usr/bin/env bash
# S2: does the `seed` option of netem give the same losses for the same seed?
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [ "${1:-outer}" = outer ]; then
  if [ "$(id -u)" -eq 0 ]; then exec unshare -n bash "$0" inner; fi
  exec unshare -Urn bash "$0" inner
fi

# Prints the sequence numbers of the echo requests that were lost (netem sits on v0, so on the way out).
lost_pattern() {
  local seed="$1"
  tc qdisc del dev v0 root 2>/dev/null || true
  if [ -n "$seed" ]; then
    tc qdisc add dev v0 root netem loss 20% seed "$seed" || return 1
  else
    tc qdisc add dev v0 root netem loss 20% || return 1
  fi
  ping -c 200 -i 0.01 -W 1 10.9.0.2 2>/dev/null | grep -o 'icmp_seq=[0-9]*' | cut -d= -f2 | sort -n >"$SPIKE_OUT/seen.txt"
  local seen
  seen="$(wc -l <"$SPIKE_OUT/seen.txt")"
  # With 20 % loss about 160 of 200 replies arrive; far fewer means that ping did not run as intended
  if [ "$seen" -lt 100 ]; then
    echo "INVALID(only $seen replies)"
    return 0
  fi
  seq 1 200 | grep -vxFf "$SPIKE_OUT/seen.txt" | tr '\n' ' '
}

if ! pair_up; then
  fail S2.0 "could not set up the pair of namespaces (run S1 first)"
  ns_stop
  exit 0
fi

if ! tc qdisc replace dev v0 root netem loss 1% seed 1 2>/dev/null; then
  fail S2.1 "tc or the kernel does not accept: netem loss ... seed N :: $(tc qdisc replace dev v0 root netem loss 1% seed 1 2>&1 | head -n 1)"
  ns_stop
  exit 0
fi
pass S2.1 "tc accepts the seed option"
a="$(lost_pattern 1234)"
b="$(lost_pattern 1234)"
c="$(lost_pattern 999)"
d="$(lost_pattern '')"
info S2.2 "lost with seed 1234 (run 1): $a"
info S2.2 "lost with seed 1234 (run 2): $b"
info S2.2 "lost with seed 999:          $c"
case "$a" in INVALID*) warn S2.3 "ping did not run as intended (minimum interval for non-root?): $a" ;; esac
if [ -n "$a" ] && [ "$a" = "$b" ] && [ "${a#INVALID}" = "$a" ]; then pass S2.3 "same seed, same lost packets"; else fail S2.3 "same seed, but different lost packets (or none lost)"; fi
if [ "$a" != "$c" ]; then pass S2.4 "another seed gives another pattern"; else fail S2.4 "another seed gave the same pattern"; fi
info S2.5 "without a seed: $d"

# Five more runs with the same seed. If they all agree with the first, the pattern is a function of the
# seed and of the number of packets that enter the qdisc. If some differ, something else sends packets.
same=0
others=5
for _ in $(seq 1 "$others"); do
  x="$(lost_pattern 1234)"
  if [ "$x" = "$a" ]; then same=$((same + 1)); else info S2.6 "differs from run 1: $x"; fi
done
if [ "$same" -eq "$others" ] && [ "${a#INVALID}" = "$a" ]; then
  pass S2.6 "same seed, same lost packets in $others further runs"
else
  warn S2.6 "same seed, same lost packets in only $same of $others further runs (a packet that nobody asked for shifts the pattern)"
fi
ns_stop
